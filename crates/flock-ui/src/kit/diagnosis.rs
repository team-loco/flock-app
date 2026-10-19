use flock_core::pods::PodEvent;
use flock_core::{Condition, Diagnosis, Status};
use gpui::{
    AnyElement, App, FontWeight, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, Styled, Window, div, prelude::FluentBuilder, relative,
};

use super::selectable::SelectableText;
use super::status::StatusGlyph;
use crate::theme::{
    ActiveTheme, CONDITION_AGE, CONDITION_VALUE, FONT_MONO, GLYPH_SLOT, LINE_PROSE, SPACE_1,
    SPACE_3, SPACE_4, SPACE_5, TEXT_LABEL, TEXT_MONO, TEXT_SMALL, WHATS_WRONG_BAR,
};

pub const WHATS_WRONG: &str = "What’s wrong";
pub const CONDITIONS: &str = "Conditions";
pub const EVENTS: &str = "Events";
const NO_CONDITIONS: &str = "No conditions reported.";
const NO_EVENTS: &str = "No recent events.";
const MISSING: &str = "—";
const HEADERS: [&str; 3] = ["Type", "Status", "Age"];

#[derive(IntoElement)]
pub struct WhatsWrong {
    status: Status,
    diagnosis: Diagnosis,
}

impl WhatsWrong {
    pub fn new(status: Status, diagnosis: Diagnosis) -> Self {
        Self { status, diagnosis }
    }
}

impl RenderOnce for WhatsWrong {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = cx.palette();
        let color = palette.status(self.status);
        let fg = palette.fg;
        let message = self.diagnosis.message;
        div()
            .debug_selector(|| "whats-wrong".into())
            .flex()
            .flex_col()
            .gap(SPACE_3)
            .pl(SPACE_5)
            .border_l(WHATS_WRONG_BAR)
            .border_color(color)
            .child(
                div()
                    .font_family(FONT_MONO)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color)
                    .child(SelectableText::new(self.diagnosis.reason)),
            )
            .when(!message.is_empty(), |block| {
                block.child(
                    div()
                        .min_w_0()
                        .text_color(fg)
                        .line_height(relative(LINE_PROSE))
                        .child(SelectableText::new(message)),
                )
            })
    }
}

#[derive(IntoElement)]
pub struct ConditionTable {
    conditions: Vec<Condition>,
}

impl ConditionTable {
    pub fn new(conditions: &[Condition]) -> Self {
        Self {
            conditions: conditions.to_vec(),
        }
    }
}

fn cell(width: gpui::Pixels) -> gpui::Div {
    div().w(width).flex_none().min_w_0()
}

impl RenderOnce for ConditionTable {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        if self.conditions.is_empty() {
            return div()
                .text_color(palette.mute)
                .child(NO_CONDITIONS)
                .into_any_element();
        }
        let [kind, value, age] = HEADERS;
        let header = div()
            .flex()
            .gap(SPACE_4)
            .text_size(TEXT_LABEL)
            .text_color(palette.mute)
            .child(div().w(GLYPH_SLOT).flex_none())
            .child(div().flex_1().min_w_0().child(kind))
            .child(cell(CONDITION_VALUE).child(value))
            .child(cell(CONDITION_AGE).flex().justify_end().child(age));
        let rows = self.conditions.into_iter().map(|condition| {
            let status = condition.status();
            let reason_color = if condition.is_healthy() {
                palette.fg2
            } else {
                palette.status(status)
            };
            let age = condition
                .age
                .map_or_else(|| MISSING.to_owned(), flock_core::time::age);
            let reason = (!condition.reason.is_empty()).then(|| {
                div()
                    .font_family(FONT_MONO)
                    .text_size(TEXT_MONO)
                    .text_color(reason_color)
                    .child(SelectableText::new(condition.reason.clone()))
            });
            let message = (!condition.message.is_empty()).then(|| {
                div()
                    .text_size(TEXT_SMALL)
                    .text_color(palette.fg2)
                    .line_height(relative(LINE_PROSE))
                    .child(SelectableText::new(condition.message.clone()))
            });
            let detail = (reason.is_some() || message.is_some()).then(|| {
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_1)
                    .pl(GLYPH_SLOT + SPACE_4)
                    .min_w_0()
                    .children(reason)
                    .children(message)
            });
            div()
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_4)
                        .child(StatusGlyph::new(status))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(SelectableText::new(condition.kind)),
                        )
                        .child(
                            cell(CONDITION_VALUE)
                                .font_family(FONT_MONO)
                                .text_size(TEXT_MONO)
                                .text_color(palette.fg2)
                                .child(condition.value),
                        )
                        .child(
                            cell(CONDITION_AGE)
                                .flex()
                                .justify_end()
                                .font_family(FONT_MONO)
                                .text_size(TEXT_MONO)
                                .text_color(palette.mute)
                                .child(age),
                        ),
                )
                .children(detail)
        });
        div()
            .debug_selector(|| "condition-table".into())
            .flex()
            .flex_col()
            .gap(SPACE_4)
            .child(header)
            .children(rows)
            .into_any_element()
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
        let palette = *cx.palette();
        if self.events.is_empty() {
            return div()
                .text_color(palette.mute)
                .child(NO_EVENTS)
                .into_any_element();
        }
        let rows: Vec<AnyElement> = self
            .events
            .into_iter()
            .map(|event| {
                let when = SharedString::from(format!(
                    "{} · {}",
                    flock_core::units::times(event.count),
                    flock_core::time::age(event.age)
                ));
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_1)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(SPACE_3)
                            .font_family(FONT_MONO)
                            .text_size(TEXT_MONO)
                            .child(StatusGlyph::new(event.status))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(SelectableText::new(event.reason)),
                            )
                            .child(div().flex_none().text_color(palette.mute).child(when)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_color(palette.fg2)
                            .line_height(relative(LINE_PROSE))
                            .child(SelectableText::new(event.message)),
                    )
                    .into_any_element()
            })
            .collect();
        div()
            .debug_selector(|| "object-events".into())
            .flex()
            .flex_col()
            .gap(SPACE_5)
            .children(rows)
            .into_any_element()
    }
}

pub fn reason_cell(
    status: Status,
    diagnosis: Option<&Diagnosis>,
    palette: &crate::theme::Palette,
) -> AnyElement {
    let Some(diagnosis) = diagnosis else {
        return div().into_any_element();
    };
    let message = (!diagnosis.message.is_empty()).then(|| {
        div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_color(palette.mute)
            .child(diagnosis.message.clone())
    });
    div()
        .debug_selector(|| "reason-cell".into())
        .flex()
        .items_center()
        .gap(SPACE_3)
        .min_w_0()
        .overflow_hidden()
        .child(
            div()
                .flex_none()
                .font_family(FONT_MONO)
                .text_size(TEXT_MONO)
                .text_color(palette.status(status))
                .child(diagnosis.reason.clone()),
        )
        .children(message)
        .into_any_element()
}

pub fn reason_tooltip(diagnosis: Option<&Diagnosis>) -> SharedString {
    diagnosis.map(Diagnosis::text).unwrap_or_default().into()
}
