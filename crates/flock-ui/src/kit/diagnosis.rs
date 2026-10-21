use flock_core::{Condition, Diagnosis, Status};
use gpui::{
    AnyElement, App, InteractiveElement, IntoElement, ParentElement, RenderOnce, SharedString,
    Styled, Window, div, relative,
};

use super::selectable::SelectableText;
use super::status::StatusGlyph;
use crate::theme::{
    ActiveTheme, CONDITION_AGE, CONDITION_VALUE, FONT_MONO, GLYPH_SLOT, LINE_PROSE, SPACE_1,
    SPACE_3, SPACE_4, TEXT_LABEL, TEXT_MONO, TEXT_SMALL,
};

pub const CONDITIONS: &str = "Conditions";
pub const EVENTS: &str = "Events";
const NO_CONDITIONS: &str = "No conditions reported.";
const MISSING: &str = "—";
const HEADERS: [&str; 3] = ["Type", "Status", "Age"];

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
