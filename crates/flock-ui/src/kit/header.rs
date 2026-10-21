use flock_core::Status;
use gpui::{
    AnyElement, App, FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, relative,
};

use super::selectable::SelectableText;
use super::status::StatusText;
use super::tooltip::Tooltip;
use crate::theme::{
    ActiveTheme, CONTROL, FONT_MONO, LINE_PROSE, PAGE_HEADER, RULE, SPACE_4, SPACE_5, SPACE_7,
    SPACE_8, TEXT_BODY, TEXT_HEADING, TEXT_MONO, TEXT_SMALL, TEXT_TITLE, TEXT_UI,
};

const META_LINE: &str = "meta-line";
const PAGE_TITLE: &str = "page-title";

#[derive(IntoElement)]
pub struct PageHeader {
    title: SharedString,
    kicker: Option<SharedString>,
    mono: bool,
    status: Option<(Status, SharedString)>,
    actions: Vec<AnyElement>,
    leading: Option<AnyElement>,
    detail: Option<AnyElement>,
    meta: Option<AnyElement>,
    footer: Option<AnyElement>,
}

impl PageHeader {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            kicker: None,
            mono: false,
            status: None,
            actions: Vec::new(),
            leading: None,
            detail: None,
            meta: None,
            footer: None,
        }
    }

    pub fn leading(mut self, element: Option<impl IntoElement>) -> Self {
        self.leading = element.map(IntoElement::into_any_element);
        self
    }

    pub fn kicker(mut self, kicker: impl Into<SharedString>) -> Self {
        self.kicker = Some(kicker.into());
        self
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

    pub fn detail(mut self, detail: Option<impl IntoElement>) -> Self {
        self.detail = detail.map(IntoElement::into_any_element);
        self
    }

    pub fn meta(mut self, meta: impl IntoElement) -> Self {
        self.meta = Some(meta.into_any_element());
        self
    }

    pub fn footer(mut self, footer: impl IntoElement) -> Self {
        self.footer = Some(footer.into_any_element());
        self
    }
}

impl RenderOnce for PageHeader {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = cx.palette();
        let (fg2, mute, line) = (palette.fg2, palette.mute, palette.line);
        let tooltip = self.title.clone();
        let title = div()
            .id(PAGE_TITLE)
            .flex_initial()
            .min_w_0()
            .text_size(TEXT_HEADING)
            .font_weight(FontWeight::SEMIBOLD)
            .truncate()
            .when(self.mono, |title| title.font_family(FONT_MONO))
            .tooltip(Tooltip::text(tooltip))
            .child(SelectableText::new(self.title));
        let top = div()
            .flex()
            .items_center()
            .gap(SPACE_5)
            .h(CONTROL)
            .children(self.leading)
            .when_some(self.kicker, |row, kicker| {
                row.child(
                    div()
                        .flex_none()
                        .font_family(FONT_MONO)
                        .text_size(TEXT_SMALL)
                        .text_color(mute)
                        .child(kicker),
                )
            })
            .child(title)
            .when_some(self.status, |row, (status, label)| {
                row.child(
                    div()
                        .flex_none()
                        .text_size(TEXT_BODY)
                        .child(StatusText::new(status, label)),
                )
            })
            .child(div().flex_1())
            .children(
                self.actions
                    .into_iter()
                    .map(|action| div().flex_none().child(action)),
            );
        div()
            .flex()
            .flex_col()
            .gap(SPACE_4)
            .px(SPACE_8)
            .pt(SPACE_8)
            .pb(SPACE_7)
            .border_b(RULE)
            .border_color(line)
            .child(top)
            .when_some(self.detail, |header, detail| {
                header.child(
                    div()
                        .text_size(TEXT_UI)
                        .line_height(relative(LINE_PROSE))
                        .text_color(fg2)
                        .child(detail),
                )
            })
            .when_some(self.meta, |header, meta| {
                header.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .font_family(FONT_MONO)
                        .text_size(TEXT_MONO)
                        .text_color(mute)
                        .child(meta),
                )
            })
            .when_some(self.footer, |header, footer| {
                header.child(div().text_size(TEXT_UI).child(footer))
            })
    }
}

#[derive(IntoElement)]
pub struct MetaLine {
    text: SharedString,
}

impl MetaLine {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

impl RenderOnce for MetaLine {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = cx.palette();
        div()
            .debug_selector(|| META_LINE.to_owned())
            .flex_none()
            .px(SPACE_8)
            .py(SPACE_5)
            .border_b(RULE)
            .border_color(palette.line)
            .font_family(FONT_MONO)
            .text_size(TEXT_MONO)
            .text_color(palette.mute)
            .truncate()
            .child(self.text)
    }
}

#[derive(IntoElement)]
pub struct PageTitle {
    title: SharedString,
    detail: Option<SharedString>,
    end: Vec<AnyElement>,
    leading: Option<AnyElement>,
}

impl PageTitle {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            detail: None,
            end: Vec::new(),
            leading: None,
        }
    }

    pub fn leading(mut self, element: Option<impl IntoElement>) -> Self {
        self.leading = element.map(IntoElement::into_any_element);
        self
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn end(mut self, element: impl IntoElement) -> Self {
        self.end.push(element.into_any_element());
        self
    }
}

impl RenderOnce for PageTitle {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = cx.palette();
        div()
            .flex()
            .items_center()
            .flex_none()
            .gap(SPACE_5)
            .h(PAGE_HEADER)
            .px(SPACE_8)
            .border_b(RULE)
            .border_color(palette.line)
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .children(self.leading)
            .child(
                div()
                    .flex_none()
                    .text_size(TEXT_TITLE)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.title),
            )
            .when_some(self.detail, |row, detail| {
                row.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(FONT_MONO)
                        .text_size(TEXT_SMALL)
                        .text_color(palette.mute)
                        .child(detail),
                )
            })
            .child(div().flex_1())
            .children(self.end)
    }
}
