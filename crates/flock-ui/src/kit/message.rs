use std::ops::Range;

use gpui::{App, Hsla, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div};

use super::selectable::SelectableWords;
use super::text::Highlighted;
use crate::theme::{
    ActiveTheme, FONT_MONO, LINE_PROSE, RADIUS_SMALL, RULE, SPACE_1, SPACE_3, TEXT_SMALL,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    Block,
    Line,
}

#[derive(IntoElement)]
pub struct RawMessage {
    text: SharedString,
    color: Option<Hsla>,
    layout: Layout,
    highlights: Vec<Range<usize>>,
}

impl RawMessage {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            color: None,
            layout: Layout::Block,
            highlights: Vec::new(),
        }
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    pub fn highlights(mut self, ranges: Vec<Range<usize>>) -> Self {
        self.highlights = ranges;
        self
    }

    pub fn line(mut self) -> Self {
        self.layout = Layout::Line;
        self
    }
}

impl RenderOnce for RawMessage {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let frame = div()
            .min_w_0()
            .max_w_full()
            .px(SPACE_3)
            .py(SPACE_1)
            .rounded(RADIUS_SMALL)
            .border(RULE)
            .border_color(palette.line2)
            .bg(palette.raise)
            .font_family(FONT_MONO)
            .text_size(TEXT_SMALL)
            .line_height(gpui::relative(LINE_PROSE))
            .text_color(self.color.unwrap_or(palette.fg2));
        match self.layout {
            Layout::Block => frame.child(SelectableWords::new(self.text)),
            Layout::Line if self.highlights.is_empty() => {
                frame.overflow_hidden().truncate().child(self.text)
            }
            Layout::Line => frame
                .overflow_hidden()
                .truncate()
                .child(Highlighted::new(self.text, self.highlights)),
        }
    }
}
