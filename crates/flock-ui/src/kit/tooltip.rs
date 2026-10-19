use gpui::{
    AnyView, App, AppContext, Context, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, Styled, Window, div,
};

use crate::theme::{
    ActiveTheme, FONT_SANS, RADIUS, RULE, SPACE_2, SPACE_4, TEXT_SMALL, TOOLTIP_MAX_WIDTH,
};

pub const TOOLTIP_SELECTOR: &str = "tooltip";

pub struct Tooltip {
    text: SharedString,
}

impl Tooltip {
    pub fn text(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView {
        let text = text.into();
        move |_, cx| {
            let text = text.clone();
            cx.new(|_| Self { text }).into()
        }
    }
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.palette();
        div()
            .debug_selector(|| TOOLTIP_SELECTOR.to_owned())
            .max_w(TOOLTIP_MAX_WIDTH)
            .px(SPACE_4)
            .py(SPACE_2)
            .rounded(RADIUS)
            .border(RULE)
            .border_color(palette.line2)
            .bg(palette.raise)
            .font_family(FONT_SANS)
            .text_size(TEXT_SMALL)
            .text_color(palette.fg)
            .child(self.text.clone())
    }
}
