use std::rc::Rc;

use flock_core::Status;
use flock_core::issue::Issue;
use flock_core::pods::PodEvent;
use gpui::{
    AnyElement, App, ElementId, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, svg,
};

use super::diagnosis::EVENTS;
use super::message::RawMessage;
use super::selectable::{SelectableText, SelectableWords};
use super::status::StatusText;
use super::text::SectionLabel;
use super::tooltip::Tooltip;
use crate::assets::{CHEVRON_DOWN, CHEVRON_RIGHT};
use crate::theme::{
    ActiveTheme, ERROR_BAR, EVENT_AGE, FONT_MONO, GLYPH_SLOT, PANEL_HEADER, Palette, RADIUS, RULE,
    SECTION_HEADER, SECTION_INDENT, SPACE_1, SPACE_2, SPACE_3, SPACE_4, SPACE_5, SPACE_6,
    TEXT_BODY, TEXT_LABEL, TEXT_SMALL, TEXT_UI,
};

pub const EVENTS_EMPTY: &str = "No events in the last 1h";
const EVENTS_WINDOW: &str = "last 1h";
const NO_LABELS: &str = "None";
const LABELS: &str = "Labels";
const COUNT_PREFIX: &str = "×";
const SAVE_FAILED: &str = "Could not save the section state";

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

pub fn events_count(shown: usize, total: usize) -> String {
    if shown < total {
        format!("{shown} of {total} · {EVENTS_WINDOW}")
    } else {
        format!("{total} · {EVENTS_WINDOW}")
    }
}

pub fn section_id(panel: &str, label: &str) -> String {
    let slug = label.to_lowercase().replace(' ', "-");
    format!("{panel}-{slug}")
}

pub fn section_open(id: &str, default: bool, cx: &App) -> bool {
    crate::settings::current(cx)
        .open_sections
        .get(id)
        .copied()
        .unwrap_or(default)
}

fn save_open(id: &str, open: bool, cx: &mut App) {
    let saved = crate::settings::update(cx, |settings| {
        settings.open_sections.insert(id.to_owned(), open);
    });
    if let Err(error) = saved {
        let notice = super::overlay::Toast::new(Status::Err, SAVE_FAILED).detail(error);
        super::overlay::toast(notice, cx);
    }
}

#[derive(IntoElement)]
pub struct PanelSection {
    id: SharedString,
    label: SharedString,
    count: Option<SharedString>,
    count_color: Option<Hsla>,
    trailing: Option<AnyElement>,
    open_by_default: bool,
    body: Vec<AnyElement>,
}

impl PanelSection {
    pub fn new(panel: &str, label: impl Into<SharedString>) -> Self {
        let label = label.into();
        Self {
            id: section_id(panel, &label).into(),
            label,
            count: None,
            count_color: None,
            trailing: None,
            open_by_default: true,
            body: Vec::new(),
        }
    }

    pub fn count(mut self, count: impl Into<SharedString>) -> Self {
        self.count = Some(count.into());
        self
    }

    pub fn count_color(mut self, color: Hsla) -> Self {
        self.count_color = Some(color);
        self
    }

    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }

    pub fn closed(mut self) -> Self {
        self.open_by_default = false;
        self
    }

    pub fn events(panel: &str, events: Vec<PodEvent>) -> Self {
        let count = events_count(events.len(), events.len());
        Self::new(panel, EVENTS)
            .count(count)
            .child(EventList::new(events))
    }

    pub fn labels(panel: &str, labels: Vec<String>) -> Self {
        let count = labels.len().to_string();
        Self::new(panel, LABELS)
            .count(count)
            .child(LabelChips::new(labels))
    }
}

impl ParentElement for PanelSection {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.body.extend(elements);
    }
}

impl RenderOnce for PanelSection {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = cx.palette();
        let id = self.id.to_string();
        let open = section_open(&id, self.open_by_default, cx);
        let chevron = if open { CHEVRON_DOWN } else { CHEVRON_RIGHT };
        let count = self.count.map(|count| {
            div()
                .min_w_0()
                .truncate()
                .font_family(FONT_MONO)
                .text_size(TEXT_SMALL)
                .text_color(self.count_color.unwrap_or(palette.mute))
                .child(count)
        });
        let toggle_id = format!("{id}-toggle");
        let toggled = id.clone();
        let toggle = div()
            .id(ElementId::Name(toggle_id.clone().into()))
            .debug_selector(move || toggle_id)
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_center()
            .gap(SPACE_4)
            .cursor_pointer()
            .on_click(move |_, window, cx| {
                save_open(&toggled, !open, cx);
                window.refresh();
            })
            .child(
                svg()
                    .path(chevron)
                    .flex_none()
                    .size(GLYPH_SLOT)
                    .text_color(palette.mute),
            )
            .child(SectionLabel::new(self.label))
            .children(count);
        let header = div()
            .flex()
            .items_center()
            .gap(SPACE_4)
            .h(SECTION_HEADER)
            .child(toggle)
            .children(self.trailing.map(|t| div().flex_none().child(t)));
        let body_id = format!("{id}-body");
        let body = open.then(|| {
            div()
                .debug_selector(move || body_id)
                .ml(SECTION_INDENT)
                .pb(SPACE_5)
                .flex()
                .flex_col()
                .gap(SPACE_4)
                .min_w_0()
                .children(self.body)
        });
        div()
            .debug_selector(move || id)
            .flex()
            .flex_col()
            .px(SPACE_6)
            .border_b(RULE)
            .border_color(palette.line)
            .child(header)
            .children(body)
    }
}

#[derive(IntoElement)]
pub struct PanelHeader {
    id: SharedString,
    title: SharedString,
    mono: bool,
    status: Option<(Status, SharedString)>,
    actions: Vec<AnyElement>,
}

impl PanelHeader {
    pub fn new(panel: &str, title: impl Into<SharedString>) -> Self {
        Self {
            id: format!("{panel}-header").into(),
            title: title.into(),
            mono: false,
            status: None,
            actions: Vec::new(),
        }
    }

    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    pub fn status(mut self, status: Status, label: impl Into<SharedString>) -> Self {
        self.status = Some((status, label.into()));
        self
    }

    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }
}

impl RenderOnce for PanelHeader {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = cx.palette();
        let title_id = format!("{}-title", self.id);
        let tooltip = self.title.clone();
        let title = div()
            .id(ElementId::Name(title_id.clone().into()))
            .debug_selector(move || title_id)
            .flex_initial()
            .min_w_0()
            .truncate()
            .text_size(TEXT_UI)
            .font_weight(FontWeight::SEMIBOLD)
            .when(self.mono, |title| title.font_family(FONT_MONO))
            .tooltip(Tooltip::text(tooltip))
            .child(SelectableText::new(self.title));
        let status_id = format!("{}-status", self.id);
        let status = self.status.map(|(status, label)| {
            div()
                .debug_selector(move || status_id)
                .flex_none()
                .text_size(TEXT_BODY)
                .child(StatusText::new(status, label))
        });
        let actions = (!self.actions.is_empty()).then(|| {
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(SPACE_4)
                .children(self.actions)
        });
        let id = self.id.to_string();
        div()
            .debug_selector(move || id)
            .flex()
            .flex_none()
            .items_center()
            .gap(SPACE_5)
            .h(PANEL_HEADER)
            .px(SPACE_6)
            .border_b(RULE)
            .border_color(palette.line)
            .child(title)
            .children(status)
            .child(div().flex_1())
            .children(actions)
    }
}

#[derive(IntoElement)]
pub struct ErrorBlock {
    ix: usize,
    issue: Issue,
}

impl ErrorBlock {
    pub fn new(ix: usize, issue: Issue) -> Self {
        Self { ix, issue }
    }
}

impl RenderOnce for ErrorBlock {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let color = palette.status(self.issue.status);
        let Issue {
            reason,
            message,
            object,
            ..
        } = self.issue;
        let reason = (!reason.is_empty()).then(|| {
            div()
                .min_w_0()
                .font_family(FONT_MONO)
                .text_size(TEXT_BODY)
                .font_weight(FontWeight::MEDIUM)
                .text_color(color)
                .child(SelectableWords::new(reason))
        });
        let message_selector = format!("error-block-{}-message", self.ix);
        let message = (!message.is_empty()).then(|| {
            div()
                .debug_selector(move || message_selector)
                .min_w_0()
                .child(RawMessage::new(message).color(palette.fg))
        });
        let object = (!object.is_empty()).then(|| {
            div()
                .min_w_0()
                .font_family(FONT_MONO)
                .text_size(TEXT_LABEL)
                .text_color(palette.mute)
                .child(SelectableText::new(object))
        });
        let selector = format!("error-block-{}", self.ix);
        div()
            .id(("error-block", self.ix))
            .debug_selector(move || selector)
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .min_w_0()
            .px(SPACE_6)
            .py(SPACE_4)
            .border_l(ERROR_BAR)
            .border_color(color)
            .bg(palette.raise)
            .children(reason)
            .children(message)
            .children(object)
    }
}

#[derive(IntoElement)]
pub struct ErrorBlocks {
    issues: Vec<Issue>,
}

impl ErrorBlocks {
    pub fn new(issues: Vec<Issue>) -> Self {
        Self { issues }
    }
}

impl RenderOnce for ErrorBlocks {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.issues.is_empty() {
            return div().into_any_element();
        }
        let blocks = self
            .issues
            .into_iter()
            .enumerate()
            .map(|(ix, issue)| ErrorBlock::new(ix, issue));
        div()
            .debug_selector(|| "error-blocks".into())
            .flex()
            .flex_col()
            .gap(SPACE_4)
            .px(SPACE_6)
            .py(SPACE_5)
            .border_b(RULE)
            .border_color(cx.palette().line)
            .children(blocks)
            .into_any_element()
    }
}

pub fn issue_stack(issues: Vec<Issue>) -> Option<AnyElement> {
    (!issues.is_empty()).then(|| {
        let blocks = issues
            .into_iter()
            .enumerate()
            .map(|(ix, issue)| ErrorBlock::new(ix, issue));
        div()
            .flex()
            .flex_col()
            .gap(SPACE_4)
            .children(blocks)
            .into_any_element()
    })
}

#[derive(IntoElement)]
pub struct LabelChips {
    labels: Vec<String>,
}

impl LabelChips {
    pub fn new(labels: impl IntoIterator<Item = String>) -> Self {
        Self {
            labels: labels.into_iter().collect(),
        }
    }
}

impl RenderOnce for LabelChips {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        if self.labels.is_empty() {
            return div()
                .text_color(palette.mute)
                .child(NO_LABELS)
                .into_any_element();
        }
        let chips = self.labels.into_iter().map(|label| {
            div()
                .min_w_0()
                .max_w_full()
                .px(SPACE_3)
                .py(SPACE_1)
                .rounded(RADIUS)
                .bg(palette.raise)
                .font_family(FONT_MONO)
                .text_size(TEXT_SMALL)
                .text_color(palette.fg)
                .child(SelectableWords::new(label))
        });
        div()
            .debug_selector(|| "label-chips".into())
            .flex()
            .flex_wrap()
            .gap(SPACE_2)
            .children(chips)
            .into_any_element()
    }
}

fn reason_color(status: Status, palette: &Palette) -> Hsla {
    match status {
        Status::Err => palette.err,
        Status::Warn => palette.warn,
        Status::Ok => palette.mute,
        Status::Progress => palette.mute,
        Status::Suspended => palette.mute,
        Status::Offline => palette.mute,
    }
}

#[derive(IntoElement)]
pub struct EventRow {
    event: PodEvent,
    id: Option<SharedString>,
    on_click: Option<Handler>,
}

impl EventRow {
    pub fn new(event: PodEvent) -> Self {
        Self {
            event,
            id: None,
            on_click: None,
        }
    }

    pub fn on_click(
        mut self,
        id: impl Into<SharedString>,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.id = Some(id.into());
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for EventRow {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let event = self.event;
        let count = (event.count > 1).then(|| {
            div()
                .text_color(palette.mute)
                .child(format!("{COUNT_PREFIX}{}", event.count))
        });
        let message = (!event.message.is_empty())
            .then(|| div().min_w_0().child(RawMessage::new(event.message)));
        let row = div()
            .flex()
            .gap(SPACE_4)
            .min_w_0()
            .child(
                div()
                    .w(EVENT_AGE)
                    .flex_none()
                    .font_family(FONT_MONO)
                    .text_size(TEXT_SMALL)
                    .text_color(palette.mute)
                    .child(flock_core::time::age(event.age)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(SPACE_1)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(SPACE_2)
                            .font_family(FONT_MONO)
                            .text_size(TEXT_SMALL)
                            .child(
                                div()
                                    .text_color(reason_color(event.status, &palette))
                                    .child(SelectableText::new(event.reason)),
                            )
                            .children(count),
                    )
                    .children(message),
            );
        match (self.id, self.on_click) {
            (Some(id), Some(handler)) => {
                let selector = id.to_string();
                row.id(ElementId::Name(id))
                    .debug_selector(move || selector)
                    .cursor_pointer()
                    .on_click(move |_, window, cx| handler(window, cx))
                    .into_any_element()
            }
            _ => row.into_any_element(),
        }
    }
}

#[derive(IntoElement)]
pub struct EventList {
    events: Vec<PodEvent>,
}

impl EventList {
    pub fn new(events: Vec<PodEvent>) -> Self {
        Self { events }
    }
}

impl RenderOnce for EventList {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.events.is_empty() {
            return events_empty(cx.palette()).into_any_element();
        }
        div()
            .debug_selector(|| "object-events".into())
            .flex()
            .flex_col()
            .gap(SPACE_4)
            .children(self.events.into_iter().map(EventRow::new))
            .into_any_element()
    }
}

pub fn events_empty(palette: &Palette) -> impl IntoElement + use<> {
    div()
        .debug_selector(|| "events-empty".into())
        .font_family(FONT_MONO)
        .text_size(TEXT_SMALL)
        .text_color(palette.mute)
        .child(EVENTS_EMPTY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_ids_join_the_panel_and_a_slug_of_the_label() {
        assert_eq!(
            section_id("pod-inspector", "Events"),
            "pod-inspector-events"
        );
        assert_eq!(
            section_id("namespace-inspector", "Network policies"),
            "namespace-inspector-network-policies"
        );
        assert_eq!(events_count(4, 4), "4 · last 1h");
        assert_eq!(events_count(4, 11), "4 of 11 · last 1h");
    }

    #[gpui::test]
    fn sections_fall_back_to_their_default_and_remember_a_toggle(cx: &mut gpui::TestAppContext) {
        use flock_core::AppSettings;
        cx.update(|cx| {
            crate::init(None, cx).unwrap_or_else(|error| panic!("UI init: {error}"));
            crate::settings::install(
                AppSettings::default(),
                Rc::new(|_: &AppSettings| Ok(())),
                false,
                cx,
            );
            assert!(section_open("pod-inspector-labels", true, cx));
            assert!(!section_open("pod-inspector-events", false, cx));
            save_open("pod-inspector-events", true, cx);
            assert!(section_open("pod-inspector-events", false, cx));
            save_open("pod-inspector-labels", false, cx);
            assert!(!section_open("pod-inspector-labels", true, cx));
        });
    }
}
