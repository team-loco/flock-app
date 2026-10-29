use flock_core::filter::TextFilter;
use flock_core::query::{Chip, Facets, Suggestion, add_term, extract_terms, last_token, suggest};
use gpui::{
    Anchor, AnyView, AppContext as _, Context, Entity, EventEmitter, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window, anchored, deferred, div, prelude::FluentBuilder, svg,
};

use super::button::{Button, Segmented};
use super::input::{
    InputEvent, TextField, TextFieldBackspace, TextFieldDown, TextFieldEnter, TextFieldEscape,
    TextFieldState, TextFieldTab, TextFieldUp,
};
use super::{Kbd, popover};
use crate::assets::SEARCH;
use crate::theme::{
    ActiveTheme, FILTER_MIN, FILTER_WIDTH, FONT_MONO, ICON, MATCH_NOTE, Palette, QUERY_BAR,
    QUERY_CHIP, QUERY_CHIP_REMOVE, QUERY_INPUT_MIN, QUERY_MENU_HEIGHT, QUERY_MENU_WIDTH, RADIUS,
    ROW_TABLE, RULE, SPACE_1, SPACE_2, SPACE_3, SPACE_4, SPACE_8, TEXT_LABEL, TEXT_MONO,
    TEXT_SMALL,
};

pub const FILTER_PLACEHOLDER: &str = "Filter";
pub const GROUP_SLOT: &str = "filter-group";
const CLEAR: &str = "Clear";
const REMOVE_TOKEN: &str = "×";
const EXCLUDE: &str = "−";
const WIDEST_DIGIT: char = '0';
const CHIP_SEPARATOR: &str = " ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterEvent {
    Changed,
}

pub fn chip_label(label: &str, count: usize) -> SharedString {
    format!("{label} {count}").into()
}

pub fn widest_label(label: &str, digits: usize) -> String {
    let name = label.trim_end_matches(|c: char| c.is_ascii_digit());
    if name.len() == label.len() {
        return label.to_owned();
    }
    let count: String = std::iter::repeat_n(WIDEST_DIGIT, digits).collect();
    format!("{name}{count}")
}

fn count_digits(label: &str) -> usize {
    label.len() - label.trim_end_matches(|c: char| c.is_ascii_digit()).len()
}

pub fn match_note(shown: usize, total: usize) -> Option<String> {
    (shown != total).then(|| format!("{shown} of {total}"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SuggestionKey {
    Up,
    Down,
    Complete,
    Close,
}

pub struct FilterBar {
    id: SharedString,
    placeholder: SharedString,
    field: Entity<TextFieldState>,
    scope: Option<AnyView>,
    aside: Option<AnyView>,
    group: Option<AnyView>,
    segments: Vec<SharedString>,
    chip_digits: usize,
    chip: usize,
    matches: Option<(usize, usize)>,
    query: TextFilter,
    tokens: Vec<Chip>,
    facets: Facets,
    suggestions: Vec<Suggestion>,
    highlighted: Option<usize>,
    open: bool,
    inline: bool,
    _subscription: Subscription,
}

impl EventEmitter<FilterEvent> for FilterBar {}

impl FilterBar {
    pub fn new(id: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = cx.new(|cx| TextFieldState::new(window, cx));
        let subscription =
            cx.subscribe_in(&field, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::Change = event {
                    this.typed(window, cx);
                }
            });
        Self {
            id: id.into(),
            placeholder: FILTER_PLACEHOLDER.into(),
            field,
            scope: None,
            aside: None,
            group: None,
            segments: Vec::new(),
            chip_digits: 0,
            chip: 0,
            matches: None,
            query: TextFilter::default(),
            tokens: Vec::new(),
            facets: Facets::default(),
            suggestions: Vec::new(),
            highlighted: None,
            open: false,
            inline: false,
            _subscription: subscription,
        }
    }

    pub fn inline(mut self) -> Self {
        self.inline = true;
        self
    }

    pub fn set_placeholder(&mut self, placeholder: impl Into<SharedString>) {
        self.placeholder = placeholder.into();
    }

    pub fn set_facets(&mut self, facets: Facets, cx: &mut Context<Self>) {
        if self.facets != facets {
            self.facets = facets;
            self.suggestions = self.suggest(cx);
            let last = self.suggestions.len().checked_sub(1);
            self.highlighted = self.highlighted.zip(last).map(|(ix, last)| ix.min(last));
            cx.notify();
        }
    }

    pub fn facets(&self) -> &Facets {
        &self.facets
    }

    fn typed_text(&self, cx: &gpui::App) -> String {
        self.field.read(cx).value().to_string()
    }

    fn suggest(&self, cx: &gpui::App) -> Vec<Suggestion> {
        suggest(&self.facets, &self.tokens, &self.typed_text(cx))
    }

    fn resuggest(&mut self, cx: &mut Context<Self>) {
        self.suggestions = self.suggest(cx);
        let typing = !self.typed_text(cx).is_empty() && !self.suggestions.is_empty();
        self.highlighted = typing.then_some(0);
    }

    fn set_typed(
        &mut self,
        text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = text.into();
        self.field
            .update(cx, |field, cx| field.set_value(text, window, cx));
    }

    fn typed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (terms, remaining) = extract_terms(&self.typed_text(cx));
        let extracted = !terms.is_empty();
        if extracted {
            for term in terms {
                add_term(&mut self.tokens, term);
            }
            self.set_typed(remaining, window, cx);
        }
        self.open = true;
        self.resuggest(cx);
        let query = TextFilter::from_chips(&self.tokens, &self.typed_text(cx));
        if extracted || query != self.query {
            self.refresh_query(cx);
        } else {
            cx.notify();
        }
    }

    fn refresh_query(&mut self, cx: &mut Context<Self>) {
        self.query = TextFilter::from_chips(&self.tokens, &self.typed_text(cx));
        cx.emit(FilterEvent::Changed);
        cx.notify();
    }

    fn choose(&mut self, ix: usize, exclude: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(suggestion) = self.suggestions.get(ix).cloned() else {
            return;
        };
        let typed = self.typed_text(cx);
        let before = typed
            .get(..typed.len() - last_token(&typed).len())
            .unwrap_or_default()
            .to_owned();
        match suggestion {
            Suggestion::Text(text) => {
                self.tokens.push(Chip::Text(text));
                self.set_typed("", window, cx);
            }
            Suggestion::Facet { .. } => {
                self.set_typed(format!("{before}{}", suggestion.key()), window, cx);
                self.resuggest(cx);
                cx.notify();
                return;
            }
            Suggestion::Value { mut term, .. } => {
                term.exclude |= exclude;
                add_term(&mut self.tokens, term);
                self.set_typed(before, window, cx);
            }
        }
        self.resuggest(cx);
        self.refresh_query(cx);
    }

    fn commit_typed(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let typed = self.typed_text(cx);
        let typed = typed.trim();
        if typed.is_empty() {
            return false;
        }
        match Chip::parse(typed) {
            Chip::Term(term) => add_term(&mut self.tokens, term),
            text @ Chip::Text(_) => self.tokens.push(text),
        }
        self.set_typed("", window, cx);
        self.resuggest(cx);
        self.refresh_query(cx);
        true
    }

    fn remove_token(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.tokens.len() {
            self.tokens.remove(ix);
            self.focus(window, cx);
            self.refresh_query(cx);
        }
    }

    fn clear_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tokens.clear();
        self.set_typed("", window, cx);
        self.focus(window, cx);
        self.refresh_query(cx);
    }

    fn menu_shown(&self, window: &Window, cx: &gpui::App) -> bool {
        self.open && !self.suggestions.is_empty() && self.is_focused(window, cx)
    }

    fn suggestion_key(&mut self, key: SuggestionKey, window: &mut Window, cx: &mut Context<Self>) {
        if !self.menu_shown(window, cx) {
            return;
        }
        let count = self.suggestions.len();
        match key {
            SuggestionKey::Up => {
                self.highlighted =
                    popover::step(self.highlighted, count, |_| true, popover::Step::Previous);
            }
            SuggestionKey::Down => {
                self.highlighted =
                    popover::step(self.highlighted, count, |_| true, popover::Step::Next);
            }
            SuggestionKey::Complete => {
                self.choose(self.highlighted.unwrap_or_default(), false, window, cx);
            }
            SuggestionKey::Close => self.open = false,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_focused(window, cx) {
            return;
        }
        if let Some(ix) = self.highlighted.filter(|_| self.menu_shown(window, cx)) {
            self.choose(ix, false, window, cx);
            cx.stop_propagation();
            return;
        }
        if self.commit_typed(window, cx) {
            cx.stop_propagation();
        }
    }

    fn backspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let empty = self.field.read(cx).value().is_empty();
        if !empty || self.tokens.is_empty() || !self.is_focused(window, cx) {
            return;
        }
        self.tokens.pop();
        self.resuggest(cx);
        self.refresh_query(cx);
        cx.stop_propagation();
    }

    pub fn scope(mut self, picker: impl Into<AnyView>) -> Self {
        self.scope = Some(picker.into());
        self
    }

    pub fn aside(mut self, view: impl Into<AnyView>) -> Self {
        self.aside = Some(view.into());
        self
    }

    pub fn group(mut self, picker: impl Into<AnyView>) -> Self {
        self.group = Some(picker.into());
        self
    }

    pub fn set_group(&mut self, picker: impl Into<AnyView>, cx: &mut Context<Self>) {
        let picker = picker.into();
        if self.group.as_ref().map(AnyView::entity_id) != Some(picker.entity_id()) {
            self.group = Some(picker);
            cx.notify();
        }
    }

    pub fn set_scope(&mut self, picker: Option<AnyView>, cx: &mut Context<Self>) {
        self.scope = picker;
        cx.notify();
    }

    pub fn query(&self) -> &TextFilter {
        &self.query
    }

    pub fn tokens(&self) -> &[Chip] {
        &self.tokens
    }

    pub fn placeholder(&self) -> &str {
        &self.placeholder
    }

    pub fn words(&self) -> Vec<String> {
        self.query.words.clone()
    }

    pub fn text(&self, cx: &gpui::App) -> String {
        let typed = self.typed_text(cx);
        self.tokens
            .iter()
            .map(ToString::to_string)
            .chain([typed])
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(CHIP_SEPARATOR)
    }

    pub fn chip(&self) -> usize {
        self.chip
    }

    pub fn is_active(&self) -> bool {
        !self.query.is_empty() || !self.tokens.is_empty() || self.chip != 0
    }

    pub fn set_chips(&mut self, chips: Vec<SharedString>, cx: &mut Context<Self>) {
        if self.segments != chips {
            let digits = chips.iter().map(|chip| count_digits(chip)).max();
            self.chip_digits = self.chip_digits.max(digits.unwrap_or_default());
            self.chip = self.chip.min(chips.len().saturating_sub(1));
            self.segments = chips;
            cx.notify();
        }
    }

    pub fn select_chip(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.segments.len().max(1) && ix != self.chip {
            self.chip = ix;
            cx.emit(FilterEvent::Changed);
            cx.notify();
        }
    }

    pub fn set_matches(&mut self, shown: usize, total: usize, cx: &mut Context<Self>) {
        let matches = Some((shown, total));
        if self.matches != matches {
            self.matches = matches;
            cx.notify();
        }
    }

    pub fn matches(&self) -> Option<(usize, usize)> {
        self.matches
    }

    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.field.update(cx, |field, cx| field.focus(window, cx));
        self.open = true;
        self.resuggest(cx);
        cx.notify();
    }

    pub fn is_focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.field
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
    }

    pub fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_active() || !self.typed_text(cx).is_empty() {
            self.preset(&[], window, cx);
        }
    }

    pub fn preset(&mut self, tokens: &[String], window: &mut Window, cx: &mut Context<Self>) {
        self.tokens = tokens.iter().map(|token| Chip::parse(token)).collect();
        self.set_typed("", window, cx);
        self.open = false;
        self.resuggest(cx);
        self.chip = 0;
        self.refresh_query(cx);
    }
}

impl FilterBar {
    fn token_chips(
        &self,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<impl IntoElement + use<>> {
        self.tokens
            .iter()
            .enumerate()
            .map(|(ix, token)| {
                let (key, value, exclude) = match token {
                    Chip::Text(text) => (String::new(), text.clone(), false),
                    Chip::Term(term) => (term.key(), term.value.clone(), term.exclude),
                };
                let (key_color, background) = if exclude {
                    (palette.err, palette.errbg)
                } else {
                    (palette.acc, palette.accbg)
                };
                div()
                    .id(("token", ix))
                    .debug_selector(|| format!("{}-token-{ix}", self.id))
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(QUERY_CHIP)
                    .pl(SPACE_3)
                    .pr(SPACE_1)
                    .rounded(RADIUS)
                    .bg(background)
                    .text_size(TEXT_SMALL)
                    .whitespace_nowrap()
                    .child(div().text_color(key_color).child(key))
                    .child(div().text_color(palette.fg).child(value))
                    .child(
                        div()
                            .id(("remove", ix))
                            .debug_selector(|| format!("{}-remove-{ix}", self.id))
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(QUERY_CHIP_REMOVE)
                            .ml(SPACE_1)
                            .cursor_pointer()
                            .text_size(TEXT_MONO)
                            .text_color(palette.mute)
                            .hover(|style| style.text_color(palette.fg))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.remove_token(ix, window, cx);
                            }))
                            .child(REMOVE_TOKEN),
                    )
            })
            .collect()
    }

    fn suggestion_row(
        &self,
        ix: usize,
        suggestion: &Suggestion,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let highlighted = self.highlighted == Some(ix);
        let excludable = matches!(suggestion, Suggestion::Value { .. });
        let exclude = div()
            .id(("exclude", ix))
            .debug_selector(|| format!("{}-exclude-{ix}", self.id))
            .flex()
            .flex_none()
            .items_center()
            .h(QUERY_CHIP)
            .px(SPACE_2)
            .rounded(RADIUS)
            .border(RULE)
            .border_color(palette.line2)
            .text_size(TEXT_LABEL)
            .text_color(palette.mute)
            .hover(|style| style.text_color(palette.err).border_color(palette.err))
            .when(!highlighted, Styled::invisible)
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.choose(ix, true, window, cx);
            }))
            .child(EXCLUDE);
        let count = suggestion.count().map(|count| {
            div()
                .flex_none()
                .text_size(TEXT_LABEL)
                .text_color(palette.mute)
                .child(count.to_string())
        });
        div()
            .id(("suggestion", ix))
            .debug_selector(|| format!("{}-suggestion-{ix}", self.id))
            .flex()
            .flex_none()
            .items_center()
            .gap(SPACE_4)
            .h(ROW_TABLE)
            .pl(SPACE_4)
            .pr(SPACE_3)
            .rounded(RADIUS)
            .cursor_pointer()
            .when(highlighted, |row| row.bg(palette.hover))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.highlighted = Some(ix);
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, window, cx| this.choose(ix, false, window, cx)))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(
                        div()
                            .flex_none()
                            .text_color(palette.acc)
                            .child(suggestion.key()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(palette.fg)
                            .child(suggestion.value()),
                    ),
            )
            .children(count)
            .when(excludable, |row| row.child(exclude))
    }

    fn suggestion_menu(
        &self,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let mut rows = Vec::new();
        let mut header = None;
        for (ix, suggestion) in self.suggestions.iter().enumerate() {
            let next = suggestion.header();
            if next != header {
                if let Some(label) = next.clone() {
                    rows.push(
                        div()
                            .flex_none()
                            .pt(SPACE_3)
                            .pb(SPACE_1)
                            .px(SPACE_4)
                            .text_size(TEXT_LABEL)
                            .text_color(palette.mute)
                            .child(label)
                            .into_any_element(),
                    );
                }
                header = next;
            }
            rows.push(
                self.suggestion_row(ix, suggestion, palette, cx)
                    .into_any_element(),
            );
        }
        let surface = popover::surface("filter-suggestions", palette)
            .debug_selector(|| format!("{}-menu", self.id))
            .w(QUERY_MENU_WIDTH)
            .max_h(QUERY_MENU_HEIGHT)
            .overflow_y_scroll()
            .font_family(FONT_MONO)
            .text_size(TEXT_MONO)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.open = false;
                cx.notify();
            }))
            .children(rows);
        div().absolute().left_0().top_full().pt(SPACE_2).child(
            deferred(
                anchored()
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(SPACE_4)
                    .child(surface),
            )
            .priority(1),
        )
    }

    fn field_element(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let palette = *cx.palette();
        let focused = self.is_focused(window, cx);
        let typed = !self.field.read(cx).value().is_empty();
        let empty = !typed && self.tokens.is_empty();
        let clear = div()
            .id("filter-clear-query")
            .debug_selector(|| format!("{}-clear-query", self.id))
            .flex_none()
            .px(SPACE_1)
            .cursor_pointer()
            .text_size(TEXT_MONO)
            .text_color(palette.mute)
            .hover(|style| style.text_color(palette.fg))
            .on_click(cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                this.clear_query(window, cx);
            }))
            .child(REMOVE_TOKEN);
        div()
            .id("filter-field")
            .debug_selector(|| format!("{}-field", self.id))
            .on_click(cx.listener(|this, _, window, cx| this.focus(window, cx)))
            .relative()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(SPACE_2)
            .max_w(FILTER_WIDTH)
            .when(self.inline, Styled::max_w_full)
            .flex_1()
            .min_w(FILTER_MIN)
            .min_h(QUERY_BAR)
            .px(SPACE_3)
            .py(SPACE_1)
            .rounded(RADIUS)
            .border(RULE)
            .border_color(if focused { palette.acc } else { palette.line2 })
            .bg(palette.editor)
            .cursor_text()
            .font_family(FONT_MONO)
            .text_size(TEXT_MONO)
            .child(
                svg()
                    .path(SEARCH)
                    .flex_none()
                    .size(ICON)
                    .text_color(palette.mute),
            )
            .children(self.token_chips(&palette, cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w(QUERY_INPUT_MIN)
                    .child(TextField::bare(&self.field, TEXT_MONO))
                    .when(empty, |input| {
                        input.child(
                            div()
                                .debug_selector(|| format!("{}-placeholder", self.id))
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .child(
                                    div()
                                        .w_full()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_color(palette.mute)
                                        .child(self.placeholder.clone()),
                                ),
                        )
                    }),
            )
            .when(empty, |field| {
                field.child(
                    div()
                        .id("filter-shortcut")
                        .debug_selector(|| format!("{}-shortcut", self.id))
                        .child(Kbd::new("/")),
                )
            })
            .when(!empty, |field| field.child(clear))
            .when(self.menu_shown(window, cx), |field| {
                field.child(self.suggestion_menu(&palette, cx))
            })
    }
}

impl Render for FilterBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let chips = (!self.segments.is_empty()).then(|| {
            self.segments
                .iter()
                .fold(Segmented::new(format!("{}-chips", self.id)), |seg, chip| {
                    seg.reserved_option(chip.clone(), widest_label(chip, self.chip_digits))
                })
                .selected(self.chip)
                .on_select(cx.listener(|this, ix: &usize, _, cx| this.select_chip(*ix, cx)))
        });
        let active = self.is_active();
        let slot = format!("{}-clear-slot", self.id);
        let clear = div().debug_selector(move || slot).child(
            div().when(!active, Styled::invisible).child(
                Button::new(format!("{}-clear", self.id), CLEAR)
                    .ghost()
                    .small()
                    .disabled(!active)
                    .on_click(cx.listener(|this, _, window, cx| this.clear(window, cx))),
            ),
        );
        let note = self
            .matches
            .and_then(|(shown, total)| match_note(shown, total))
            .map(|note| {
                div()
                    .font_family(FONT_MONO)
                    .text_size(TEXT_SMALL)
                    .text_color(palette.mute)
                    .child(note)
            });
        let selector = self.id.to_string();
        let field = self.field_element(window, cx);
        let bar = div()
            .debug_selector(move || selector)
            .id(self.id.clone())
            .flex()
            .items_center()
            .capture_action(cx.listener(|this, _: &TextFieldUp, window, cx| {
                this.suggestion_key(SuggestionKey::Up, window, cx);
            }))
            .capture_action(cx.listener(|this, _: &TextFieldDown, window, cx| {
                this.suggestion_key(SuggestionKey::Down, window, cx);
            }))
            .capture_action(cx.listener(|this, _: &TextFieldTab, window, cx| {
                this.suggestion_key(SuggestionKey::Complete, window, cx);
            }))
            .capture_action(cx.listener(|this, action: &TextFieldEnter, window, cx| {
                if !action.secondary && !action.shift {
                    this.enter(window, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &TextFieldEscape, window, cx| {
                this.suggestion_key(SuggestionKey::Close, window, cx);
            }))
            .capture_action(
                cx.listener(|this, _: &TextFieldBackspace, window, cx| this.backspace(window, cx)),
            )
            .on_action(cx.listener(|this, _: &TextFieldEscape, window, cx| {
                this.clear(window, cx);
            }));
        if self.inline {
            return bar.w_full().child(field);
        }
        bar.flex_none()
            .gap(SPACE_4)
            .px(SPACE_8)
            .py(SPACE_4)
            .border_b(RULE)
            .border_color(palette.line)
            .when_some(self.scope.clone(), ParentElement::child)
            .child(field)
            .children(chips)
            .child(clear)
            .children(self.aside.clone())
            .child(
                div()
                    .debug_selector(|| format!("{}-note", self.id))
                    .ml_auto()
                    .w(MATCH_NOTE)
                    .flex_none()
                    .flex()
                    .justify_end()
                    .whitespace_nowrap()
                    .children(note),
            )
            .children(self.group.clone().map(|group| {
                div()
                    .debug_selector(|| GROUP_SLOT.to_owned())
                    .flex_none()
                    .child(group)
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_only_a_narrowed_list() {
        assert_eq!(match_note(12, 48).as_deref(), Some("12 of 48"));
        assert_eq!(match_note(48, 48), None);
        assert_eq!(widest_label("Running 3", 3), "Running 000");
        assert_eq!(widest_label("Running 398", 3), "Running 000");
        assert_eq!(widest_label("Warnings", 3), "Warnings");
        assert_eq!(count_digits("All 412"), 3);
        assert_eq!(
            chip_label("Running", 398),
            SharedString::from("Running 398")
        );
    }
}
