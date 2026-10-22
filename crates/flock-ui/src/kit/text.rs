use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, HighlightStyle, Hsla, InteractiveElement,
    IntoElement, ParentElement, Pixels, RenderOnce, SharedString, StatefulInteractiveElement,
    Styled, StyledText, Window, div, prelude::FluentBuilder,
};

use flock_core::units::Amount;

use super::selectable::SelectableText;
use crate::theme::{
    ActiveTheme, FONT_MONO, KV_LABEL, RADIUS, RULE, SPACE_2, SPACE_3, SPACE_4, SPACE_5, SPACE_7,
    SPACE_8, TEXT_LABEL, TEXT_SMALL,
};

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct AmountText {
    amount: Amount,
    color: Option<Hsla>,
}

impl AmountText {
    pub fn new(amount: Amount) -> Self {
        Self {
            amount,
            color: None,
        }
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for AmountText {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let mute = cx.palette().mute;
        let text = self.amount.labelled();
        let unit = self.amount.unit.as_ref().map(|_| {
            let style = HighlightStyle {
                color: Some(mute),
                ..HighlightStyle::default()
            };
            (self.amount.figures.len()..text.len(), style)
        });
        div()
            .when_some(self.color, Styled::text_color)
            .child(StyledText::new(text).with_highlights(unit))
    }
}

#[derive(IntoElement)]
pub struct SectionLabel {
    text: SharedString,
}

impl SectionLabel {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

impl RenderOnce for SectionLabel {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .flex_none()
            .text_size(TEXT_LABEL)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(cx.palette().mute)
            .whitespace_nowrap()
            .child(self.text.to_uppercase())
    }
}

#[derive(IntoElement)]
pub struct SectionHeader {
    label: SharedString,
    count: Option<SharedString>,
    annotation: Option<AnyElement>,
    end: Option<AnyElement>,
}

impl SectionHeader {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            count: None,
            annotation: None,
            end: None,
        }
    }

    pub fn count(mut self, count: impl Into<SharedString>) -> Self {
        self.count = Some(count.into());
        self
    }

    pub fn annotation(mut self, annotation: impl IntoElement) -> Self {
        self.annotation = Some(annotation.into_any_element());
        self
    }

    pub fn end(mut self, end: impl IntoElement) -> Self {
        self.end = Some(end.into_any_element());
        self
    }
}

impl RenderOnce for SectionHeader {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let mute = cx.palette().mute;
        div()
            .flex()
            .items_center()
            .gap(SPACE_4)
            .px(SPACE_8)
            .pt(SPACE_7)
            .pb(SPACE_3)
            .child(SectionLabel::new(self.label))
            .when_some(self.count, |row, count| {
                row.child(
                    div()
                        .font_family(FONT_MONO)
                        .text_size(TEXT_SMALL)
                        .text_color(mute)
                        .truncate()
                        .child(count),
                )
            })
            .children(self.annotation)
            .child(div().flex_1())
            .children(self.end)
    }
}

#[derive(IntoElement)]
pub struct Rule {
    vertical: bool,
}

impl Rule {
    pub fn horizontal() -> Self {
        Self { vertical: false }
    }

    pub fn vertical() -> Self {
        Self { vertical: true }
    }
}

impl RenderOnce for Rule {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let line = div().flex_none().bg(cx.palette().line);
        if self.vertical {
            line.w(RULE).h_full()
        } else {
            line.h(RULE).w_full()
        }
    }
}

#[derive(IntoElement)]
pub struct Link {
    id: ElementId,
    text: SharedString,
    mono: bool,
    color: Option<Hsla>,
    on_click: Option<ClickHandler>,
}

impl Link {
    pub fn new(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            mono: false,
            color: None,
            on_click: None,
        }
    }

    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Link {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = self.color.unwrap_or(cx.palette().acc);
        let selector = self.id.to_string();
        div()
            .debug_selector(|| selector)
            .id(self.id)
            .text_color(color)
            .cursor_pointer()
            .whitespace_nowrap()
            .when(self.mono, |link| link.font_family(FONT_MONO))
            .hover(Styled::underline)
            .when_some(self.on_click, |link, handler| {
                link.on_click(move |event, window, cx| handler(event, window, cx))
            })
            .child(self.text)
    }
}

#[derive(IntoElement)]
pub struct Kbd {
    keys: SharedString,
}

impl Kbd {
    pub fn new(keys: impl Into<SharedString>) -> Self {
        Self { keys: keys.into() }
    }
}

impl RenderOnce for Kbd {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = cx.palette();
        div()
            .flex_none()
            .px(SPACE_2)
            .py(RULE)
            .rounded(RADIUS)
            .border(RULE)
            .border_color(palette.line)
            .font_family(FONT_MONO)
            .text_size(TEXT_LABEL)
            .text_color(palette.mute)
            .child(self.keys)
    }
}

#[derive(IntoElement)]
pub struct KeyValue {
    label_width: Pixels,
    mono: bool,
    rows: Vec<(SharedString, AnyElement)>,
}

impl Default for KeyValue {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyValue {
    pub fn new() -> Self {
        Self {
            label_width: KV_LABEL,
            mono: true,
            rows: Vec::new(),
        }
    }

    pub fn label_width(mut self, width: Pixels) -> Self {
        self.label_width = width;
        self
    }

    pub fn sans(mut self) -> Self {
        self.mono = false;
        self
    }

    pub fn row(mut self, label: impl Into<SharedString>, value: impl IntoElement) -> Self {
        self.rows.push((label.into(), value.into_any_element()));
        self
    }

    pub fn text(self, label: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        self.row(label, SelectableText::new(value))
    }
}

impl RenderOnce for KeyValue {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = cx.palette();
        let mute = palette.mute;
        let rows = self.rows.into_iter().map(|(label, value)| {
            div()
                .flex()
                .gap(SPACE_5)
                .child(
                    div()
                        .w(self.label_width)
                        .flex_none()
                        .text_color(mute)
                        .truncate()
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .when(self.mono, |value| {
                            value.font_family(FONT_MONO).text_size(TEXT_SMALL)
                        })
                        .child(value),
                )
        });
        div().flex().flex_col().gap(SPACE_3).children(rows)
    }
}

#[derive(IntoElement)]
pub struct Highlighted {
    text: SharedString,
    ranges: Vec<Range<usize>>,
}

impl Highlighted {
    pub fn new(text: impl Into<SharedString>, ranges: Vec<Range<usize>>) -> Self {
        Self {
            text: text.into(),
            ranges,
        }
    }
}

impl RenderOnce for Highlighted {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let style = HighlightStyle {
            color: Some(cx.palette().acc),
            ..HighlightStyle::default()
        };
        let ranges = valid_ranges(&self.text, self.ranges);
        StyledText::new(self.text).with_highlights(ranges.into_iter().map(|range| (range, style)))
    }
}

fn valid_ranges(text: &str, ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges
        .into_iter()
        .filter(|range| {
            range.start < range.end
                && range.end <= text.len()
                && text.is_char_boundary(range.start)
                && text.is_char_boundary(range.end)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_ranges_outside_the_text_are_dropped() {
        let ranges = valid_ranges("a → b", vec![0..1, 3..4, 2..5, 6..9, 4..4]);
        assert_eq!(ranges, vec![0..1, 2..5]);
    }
}
