use std::rc::Rc;

use gpui::{
    App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce, SharedString,
    Styled, Window, div,
};

use super::button::{Button, IconButton};
use super::popover::shadow;
use crate::theme::{
    ActiveTheme, BULK_BAR, BULK_BAR_OFFSET, BULK_RULE, FONT_MONO, RADIUS_BULK, RULE, SPACE_2,
    SPACE_3, SPACE_6, TEXT_MONO,
};

const CLEAR: &str = "×";
const CLEAR_TOOLTIP: &str = "Clear selection";

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

pub struct BulkAction {
    label: SharedString,
    handler: Handler,
}

impl BulkAction {
    pub fn new(
        label: impl Into<SharedString>,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            handler: Rc::new(handler),
        }
    }
}

#[derive(IntoElement)]
pub struct BulkBar {
    id: SharedString,
    count: usize,
    actions: Vec<BulkAction>,
    clear: Handler,
}

impl BulkBar {
    pub fn new(
        id: impl Into<SharedString>,
        count: usize,
        clear: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            count,
            actions: Vec::new(),
            clear: Rc::new(clear),
        }
    }

    pub fn action(mut self, action: BulkAction) -> Self {
        self.actions.push(action);
        self
    }
}

impl RenderOnce for BulkBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let id = self.id.clone();
        let buttons = self.actions.into_iter().enumerate().map(|(ix, action)| {
            let handler = action.handler;
            let id = ElementId::Name(format!("{id}-action-{ix}").into());
            Button::new(id, action.label)
                .ghost()
                .small()
                .on_click(move |_, window, cx| handler(window, cx))
        });
        let clear = self.clear;
        let clear_id = ElementId::Name(format!("{}-clear", self.id).into());
        let close = IconButton::glyph(clear_id, CLEAR, CLEAR_TOOLTIP)
            .on_click(move |_, window, cx| clear(window, cx));
        let count = format!("{} selected", self.count);
        let bar = div()
            .flex()
            .items_center()
            .gap(SPACE_3)
            .h(BULK_BAR)
            .pl(SPACE_6)
            .pr(SPACE_3)
            .border(RULE)
            .border_color(palette.line2)
            .rounded(RADIUS_BULK)
            .bg(palette.panel)
            .shadow(shadow(&palette))
            .whitespace_nowrap()
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(TEXT_MONO)
                    .text_color(palette.fg)
                    .child(count),
            )
            .child(div().w(RULE).h(BULK_RULE).mx(SPACE_2).bg(palette.line2))
            .children(buttons)
            .child(close);
        div()
            .debug_selector(|| self.id.to_string())
            .absolute()
            .left_0()
            .right_0()
            .bottom(BULK_BAR_OFFSET)
            .flex()
            .justify_center()
            .child(bar)
    }
}
