use std::collections::BTreeSet;

use flock_core::filter::{
    FilterCandidates, LabelValues, TextFilter, applied_token, extract_tokens,
};
use gpui::{
    AnyView, AppContext as _, Context, Entity, EventEmitter, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window, div, prelude::FluentBuilder,
};

use super::button::{Button, Segmented};
use super::input::{
    InputEvent, TextField, TextFieldBackspace, TextFieldDown, TextFieldEnter, TextFieldEscape,
    TextFieldState, TextFieldTab, TextFieldUp,
};
use super::{Kbd, popover};
use crate::theme::{
    ActiveTheme, CONTROL, FILTER_MIN, FILTER_WIDTH, FONT_MONO, MATCH_NOTE, MENU_MAX_HEIGHT,
    Palette, RADIUS, ROW_TABLE, RULE, SPACE_4, SPACE_8, TEXT_MONO, TEXT_SMALL,
};

const CLEAR: &str = "Clear";
const REMOVE_TOKEN: &str = "×";
const WIDEST_DIGIT: char = '0';

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
    chips: Vec<SharedString>,
    chip_digits: usize,
    chip: usize,
    matches: Option<(usize, usize)>,
    query: TextFilter,
    tokens: Vec<String>,
    candidates: FilterCandidates,
    suggestions: Vec<String>,
    highlighted: usize,
    _subscription: Subscription,
}

impl EventEmitter<FilterEvent> for FilterBar {}

impl FilterBar {
    pub fn new(
        id: impl Into<SharedString>,
        placeholder: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = cx.new(|cx| TextFieldState::new(window, cx));
        let subscription =
            cx.subscribe_in(&field, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::Change = event {
                    this.typed(window, cx);
                }
            });
        Self {
            id: id.into(),
            placeholder: placeholder.into(),
            field,
            scope: None,
            aside: None,
            chips: Vec::new(),
            chip_digits: 0,
            chip: 0,
            matches: None,
            query: TextFilter::default(),
            tokens: Vec::new(),
            candidates: FilterCandidates::default(),
            suggestions: Vec::new(),
            highlighted: 0,
            _subscription: subscription,
        }
    }

    pub fn set_namespaces(&mut self, namespaces: BTreeSet<String>, cx: &mut Context<Self>) {
        if self.candidates.namespaces != namespaces {
            self.candidates.namespaces = namespaces;
            self.resuggest(cx);
        }
    }

    pub fn set_labels(&mut self, labels: LabelValues, cx: &mut Context<Self>) {
        if self.candidates.labels != labels {
            self.candidates.labels = labels;
            self.resuggest(cx);
        }
    }

    fn resuggest(&mut self, cx: &mut Context<Self>) {
        if self.suggestions.is_empty() {
            return;
        }
        self.suggestions = self.candidates.suggest(&self.field.read(cx).value());
        self.highlighted = self
            .highlighted
            .min(self.suggestions.len().saturating_sub(1));
        cx.notify();
    }

    fn typed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.field.read(cx).value().to_string();
        let (tokens, remaining) = extract_tokens(&value);
        if !tokens.is_empty() {
            self.tokens.extend(tokens);
            self.field
                .update(cx, |field, cx| field.set_value(remaining, window, cx));
        }
        self.suggestions = self.candidates.suggest(&self.field.read(cx).value());
        self.highlighted = 0;
        self.refresh_query(cx);
    }

    fn refresh_query(&mut self, cx: &mut Context<Self>) {
        self.query = TextFilter::parse(&self.text(cx));
        cx.emit(FilterEvent::Changed);
        cx.notify();
    }

    fn choose(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(suggestion) = self.suggestions.get(ix).cloned() else {
            return;
        };
        let value = self.field.read(cx).value().to_string();
        let start = value.rfind(char::is_whitespace).map_or(0, |ix| ix + 1);
        let mut remaining = value[..start].to_owned();
        if applied_token(&suggestion) {
            self.tokens.push(suggestion);
        } else {
            remaining.push_str(&suggestion);
        }
        self.field
            .update(cx, |field, cx| field.set_value(remaining, window, cx));
        self.suggestions = self.candidates.suggest(&self.field.read(cx).value());
        self.highlighted = 0;
        self.refresh_query(cx);
    }

    fn remove_token(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.tokens.len() {
            self.tokens.remove(ix);
            self.refresh_query(cx);
        }
    }

    fn suggestion_key(&mut self, key: SuggestionKey, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.suggestions.len();
        if count == 0 || !self.is_focused(window, cx) {
            return;
        }
        match key {
            SuggestionKey::Up => self.highlighted = (self.highlighted + count - 1) % count,
            SuggestionKey::Down => self.highlighted = (self.highlighted + 1) % count,
            SuggestionKey::Complete => self.choose(self.highlighted, window, cx),
            SuggestionKey::Close => self.suggestions.clear(),
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn backspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let empty = self.field.read(cx).value().is_empty();
        if !empty || self.tokens.is_empty() || !self.is_focused(window, cx) {
            return;
        }
        self.tokens.pop();
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

    pub fn set_scope(&mut self, picker: Option<AnyView>, cx: &mut Context<Self>) {
        self.scope = picker;
        cx.notify();
    }

    pub fn query(&self) -> &TextFilter {
        &self.query
    }

    pub fn words(&self) -> Vec<String> {
        self.query.words.clone()
    }

    pub fn text(&self, cx: &gpui::App) -> String {
        let value = self.field.read(cx).value();
        self.tokens
            .iter()
            .map(String::as_str)
            .chain([value.as_ref()])
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn chip(&self) -> usize {
        self.chip
    }

    pub fn is_active(&self) -> bool {
        !self.query.is_empty() || self.chip != 0
    }

    pub fn set_chips(&mut self, chips: Vec<SharedString>, cx: &mut Context<Self>) {
        if self.chips != chips {
            let digits = chips.iter().map(|chip| count_digits(chip)).max();
            self.chip_digits = self.chip_digits.max(digits.unwrap_or_default());
            self.chip = self.chip.min(chips.len().saturating_sub(1));
            self.chips = chips;
            cx.notify();
        }
    }

    pub fn select_chip(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.chips.len().max(1) && ix != self.chip {
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

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.field.update(cx, |field, cx| field.focus(window, cx));
    }

    pub fn is_focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.field
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
    }

    pub fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_active() {
            return;
        }
        self.tokens.clear();
        self.field
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.suggestions.clear();
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
                div()
                    .id(("token", ix))
                    .debug_selector(|| format!("{}-token-{ix}", self.id))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(SPACE_4)
                    .px(SPACE_4)
                    .rounded(RADIUS)
                    .bg(palette.chrome)
                    .text_color(palette.fg2)
                    .child(token.clone())
                    .child(
                        Button::new(format!("{}-remove-{ix}", self.id), REMOVE_TOKEN)
                            .ghost()
                            .small()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove_token(ix, cx);
                            })),
                    )
            })
            .collect()
    }

    fn suggestion_popover(
        &self,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let rows = self
            .suggestions
            .iter()
            .enumerate()
            .map(|(ix, suggestion)| {
                div()
                    .id(ix)
                    .debug_selector(|| format!("{}-suggestion-{ix}", self.id))
                    .h(ROW_TABLE)
                    .px(SPACE_4)
                    .cursor_pointer()
                    .when(ix == self.highlighted, |row| {
                        row.bg(palette.sel).text_color(palette.selfg)
                    })
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.highlighted = ix;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| this.choose(ix, window, cx)))
                    .child(suggestion.clone())
            })
            .collect::<Vec<_>>();
        let surface = popover::surface("filter-suggestions", palette)
            .w(FILTER_WIDTH)
            .max_h(MENU_MAX_HEIGHT)
            .overflow_y_scroll()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.suggestions.clear();
                cx.notify();
            }))
            .children(rows)
            .into_any_element();
        popover::below_left(CONTROL, surface)
    }

    fn field_element(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let palette = *cx.palette();
        let focused = self.is_focused(window, cx);
        let empty = self.field.read(cx).value().is_empty() && self.tokens.is_empty();
        div()
            .id("filter-field")
            .debug_selector(|| format!("{}-field", self.id))
            .on_click(cx.listener(|this, _, window, cx| this.focus(window, cx)))
            .relative()
            .flex()
            .items_center()
            .gap(SPACE_4)
            .max_w(FILTER_WIDTH)
            .flex_1()
            .min_w(FILTER_MIN)
            .h(CONTROL)
            .px(SPACE_4)
            .rounded(RADIUS)
            .border(RULE)
            .border_color(if focused {
                palette.selline
            } else {
                palette.line2
            })
            .bg(palette.editor)
            .font_family(FONT_MONO)
            .text_size(TEXT_MONO)
            .children(self.token_chips(&palette, cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
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
            .when(focused && !self.suggestions.is_empty(), |field| {
                field.child(self.suggestion_popover(&palette, cx))
            })
    }
}

impl Render for FilterBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let chips = (!self.chips.is_empty()).then(|| {
            self.chips
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
        div()
            .debug_selector(move || selector)
            .id(self.id.clone())
            .flex()
            .flex_none()
            .items_center()
            .gap(SPACE_4)
            .px(SPACE_8)
            .py(SPACE_4)
            .border_b(RULE)
            .border_color(palette.line)
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
                    this.suggestion_key(SuggestionKey::Complete, window, cx);
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
            }))
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
