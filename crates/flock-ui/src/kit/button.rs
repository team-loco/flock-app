use std::rc::Rc;

use flock_core::scope::Check;
use gpui::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, Hsla, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Pixels, RenderOnce, SharedString, StatefulInteractiveElement,
    Styled, Window, div, prelude::FluentBuilder, svg,
};

use super::tooltip::Tooltip;
use crate::assets;
use crate::theme::{
    ActiveTheme, CHECKBOX, CHECKBOX_TICK, CONTROL, CONTROL_SMALL, FONT_MONO, ICON, ICON_BUTTON,
    MORE_BUTTON, Palette, RADIUS, RADIUS_SMALL, RULE, SEGMENT, SPACE_3, SPACE_4, SPACE_5,
    TEXT_CONTROL, TEXT_GLYPH, TEXT_LABEL, TEXT_SMALL, TEXT_UI, TOGGLE_HEIGHT, TOGGLE_KNOB,
    TOGGLE_WIDTH,
};

const SHORTCUT_OPACITY: f32 = 0.75;
const DISABLED_OPACITY: f32 = 0.5;

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonVariant {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonSize {
    Regular,
    Small,
}

impl ButtonSize {
    fn height(self) -> Pixels {
        match self {
            Self::Regular => CONTROL,
            Self::Small => CONTROL_SMALL,
        }
    }

    fn padding(self) -> Pixels {
        match self {
            Self::Regular => SPACE_5,
            Self::Small => SPACE_3,
        }
    }

    fn text(self) -> Pixels {
        match self {
            Self::Regular => TEXT_CONTROL,
            Self::Small => TEXT_SMALL,
        }
    }
}

struct Look {
    bg: Option<Hsla>,
    border: Option<Hsla>,
    fg: Hsla,
    hover: Option<Hsla>,
    weight: FontWeight,
}

fn look(variant: ButtonVariant, selected: bool, palette: &Palette) -> Look {
    if selected {
        return Look {
            bg: Some(palette.sel),
            border: Some(palette.selline),
            fg: palette.selfg,
            hover: None,
            weight: FontWeight::NORMAL,
        };
    }
    match variant {
        ButtonVariant::Primary => Look {
            bg: Some(palette.acc),
            border: Some(palette.acc),
            fg: palette.onacc,
            hover: None,
            weight: FontWeight::MEDIUM,
        },
        ButtonVariant::Secondary => Look {
            bg: None,
            border: Some(palette.line2),
            fg: palette.fg2,
            hover: Some(palette.hover),
            weight: FontWeight::NORMAL,
        },
        ButtonVariant::Ghost => Look {
            bg: None,
            border: None,
            fg: palette.acc,
            hover: Some(palette.hover),
            weight: FontWeight::NORMAL,
        },
        ButtonVariant::Danger => Look {
            bg: Some(palette.err),
            border: Some(palette.err),
            fg: palette.onacc,
            hover: None,
            weight: FontWeight::MEDIUM,
        },
    }
}

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    variant: ButtonVariant,
    size: ButtonSize,
    shortcut: Option<SharedString>,
    selected: bool,
    disabled: bool,
    tooltip: Option<SharedString>,
    on_click: Option<ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            variant: ButtonVariant::Secondary,
            size: ButtonSize::Regular,
            shortcut: None,
            selected: false,
            disabled: false,
            tooltip: None,
            on_click: None,
        }
    }

    pub fn primary(mut self) -> Self {
        self.variant = ButtonVariant::Primary;
        self
    }

    pub fn ghost(mut self) -> Self {
        self.variant = ButtonVariant::Ghost;
        self
    }

    pub fn danger(mut self) -> Self {
        self.variant = ButtonVariant::Danger;
        self
    }

    pub fn small(mut self) -> Self {
        self.size = ButtonSize::Small;
        self
    }

    pub fn shortcut(mut self, keys: impl Into<SharedString>) -> Self {
        self.shortcut = Some(keys.into());
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
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

impl RenderOnce for Button {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let look = look(self.variant, self.selected, &palette);
        let shortcut_color = match self.variant {
            ButtonVariant::Primary => look.fg.opacity(SHORTCUT_OPACITY),
            ButtonVariant::Danger => look.fg.opacity(SHORTCUT_OPACITY),
            ButtonVariant::Secondary => palette.mute,
            ButtonVariant::Ghost => palette.mute,
        };
        let enabled = !self.disabled;
        div()
            .debug_selector(|| self.id.to_string())
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(SPACE_4)
            .h(self.size.height())
            .px(self.size.padding())
            .rounded(RADIUS)
            .text_size(self.size.text())
            .font_weight(look.weight)
            .text_color(look.fg)
            .whitespace_nowrap()
            .when_some(look.bg, Styled::bg)
            .when_some(look.border, |button, border| {
                button.border(RULE).border_color(border)
            })
            .when(self.disabled, |button| button.opacity(DISABLED_OPACITY))
            .when(enabled, Styled::cursor_pointer)
            .when_some(look.hover.filter(|_| enabled), |button, hover| {
                button.hover(move |style| style.bg(hover))
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(self.label)
            .when_some(self.shortcut, |button, keys| {
                button.child(
                    div()
                        .font_family(FONT_MONO)
                        .text_size(TEXT_LABEL)
                        .font_weight(FontWeight::NORMAL)
                        .text_color(shortcut_color)
                        .child(keys),
                )
            })
            .when_some(self.tooltip, |button, text| {
                button.tooltip(Tooltip::text(text))
            })
            .when_some(self.on_click.filter(|_| enabled), |button, handler| {
                button.on_click(move |event, window, cx| handler(event, window, cx))
            })
    }
}

enum IconContent {
    Glyph(SharedString),
    Svg(SharedString),
}

#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    content: IconContent,
    tooltip: SharedString,
    bordered: bool,
    selected: bool,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

impl IconButton {
    pub fn glyph(
        id: impl Into<ElementId>,
        glyph: impl Into<SharedString>,
        tooltip: impl Into<SharedString>,
    ) -> Self {
        Self::build(id, IconContent::Glyph(glyph.into()), tooltip)
    }

    pub fn svg(
        id: impl Into<ElementId>,
        path: impl Into<SharedString>,
        tooltip: impl Into<SharedString>,
    ) -> Self {
        Self::build(id, IconContent::Svg(path.into()), tooltip)
    }

    fn build(
        id: impl Into<ElementId>,
        content: IconContent,
        tooltip: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            content,
            tooltip: tooltip.into(),
            bordered: false,
            selected: false,
            disabled: false,
            on_click: None,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn bordered(mut self) -> Self {
        self.bordered = true;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
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

impl RenderOnce for IconButton {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let content = match self.content {
            IconContent::Glyph(glyph) => div().child(glyph).into_any_element(),
            IconContent::Svg(path) => svg()
                .path(path)
                .size(ICON)
                .text_color(palette.fg2)
                .into_any_element(),
        };
        let hover = palette.hover;
        let (width, height) = if self.bordered {
            (MORE_BUTTON, CONTROL)
        } else {
            (ICON_BUTTON, ICON_BUTTON)
        };
        div()
            .debug_selector(|| self.id.to_string())
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(width)
            .h(height)
            .rounded(RADIUS)
            .when(!self.disabled, Styled::cursor_pointer)
            .when(self.disabled, |button| button.opacity(DISABLED_OPACITY))
            .text_size(TEXT_UI)
            .text_color(palette.fg2)
            .when(self.bordered, |button| {
                button.border(RULE).border_color(palette.line2)
            })
            .when(self.selected, |button| button.bg(palette.sel))
            .when(!self.selected && !self.disabled, |button| {
                button.hover(move |style| style.bg(hover))
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(!self.selected, |button| {
                button.tooltip(Tooltip::text(self.tooltip))
            })
            .when_some(
                self.on_click.filter(|_| !self.disabled),
                |button, handler| {
                    button.on_click(move |event, window, cx| handler(event, window, cx))
                },
            )
            .child(content)
    }
}

type SelectHandler = Rc<dyn Fn(&usize, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct Segmented {
    id: ElementId,
    options: Vec<(SharedString, Option<SharedString>)>,
    selected: usize,
    small: bool,
    mono: bool,
    on_select: Option<SelectHandler>,
}

impl Segmented {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            options: Vec::new(),
            selected: 0,
            small: false,
            mono: false,
            on_select: None,
        }
    }

    pub fn option(mut self, label: impl Into<SharedString>) -> Self {
        self.options.push((label.into(), None));
        self
    }

    pub fn reserved_option(
        mut self,
        label: impl Into<SharedString>,
        widest: impl Into<SharedString>,
    ) -> Self {
        self.options.push((label.into(), Some(widest.into())));
        self
    }

    pub fn selected(mut self, index: usize) -> Self {
        self.selected = index;
        self
    }

    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    pub fn on_select(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Segmented {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let (height, padding, text) = if self.small {
            (CONTROL_SMALL, SPACE_4, TEXT_SMALL)
        } else {
            (SEGMENT, SPACE_5, TEXT_CONTROL)
        };
        let group = self.id.to_string();
        let segments = self
            .options
            .into_iter()
            .enumerate()
            .map(|(ix, (label, widest))| {
                let selected = ix == self.selected;
                let handler = self.on_select.clone();
                let hover = palette.hover;
                let selector = format!("{group}-{ix}");
                div()
                    .debug_selector(|| selector)
                    .id(ix)
                    .flex()
                    .items_center()
                    .h(height)
                    .px(padding)
                    .cursor_pointer()
                    .whitespace_nowrap()
                    .when(ix > 0, |segment| {
                        segment.border_l(RULE).border_color(palette.line2)
                    })
                    .when(selected, |segment| {
                        segment.bg(palette.sel).text_color(palette.selfg)
                    })
                    .when(!selected, |segment| {
                        segment
                            .text_color(palette.fg2)
                            .hover(move |style| style.bg(hover))
                    })
                    .when_some(handler, |segment, handler| {
                        segment.on_click(move |_, window, cx| handler(&ix, window, cx))
                    })
                    .child(segment_label(label, widest))
            });
        div()
            .debug_selector(|| self.id.to_string())
            .id(self.id)
            .flex()
            .flex_none()
            .rounded(RADIUS)
            .border(RULE)
            .border_color(palette.line2)
            .overflow_hidden()
            .text_size(text)
            .when(self.mono, |group| group.font_family(FONT_MONO))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(segments)
    }
}

fn segment_label(label: SharedString, widest: Option<SharedString>) -> AnyElement {
    match widest {
        Some(widest) => div()
            .relative()
            .child(div().invisible().child(widest))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .child(label),
            )
            .into_any_element(),
        None => label.into_any_element(),
    }
}

type ToggleHandler = Rc<dyn Fn(&bool, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct Toggle {
    id: ElementId,
    on: bool,
    label: Option<SharedString>,
    handler: Option<ToggleHandler>,
}

impl Toggle {
    pub fn new(id: impl Into<ElementId>, on: bool) -> Self {
        Self {
            id: id.into(),
            on,
            label: None,
            handler: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn on_toggle(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.handler = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Toggle {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let inset = (TOGGLE_HEIGHT - TOGGLE_KNOB) / 2. - RULE;
        let (track, border, knob) = if self.on {
            (palette.acc, palette.acc, palette.onacc)
        } else {
            (palette.editor, palette.line2, palette.mute)
        };
        let switch = div()
            .flex()
            .flex_none()
            .items_center()
            .when(self.on, Styled::justify_end)
            .w(TOGGLE_WIDTH)
            .h(TOGGLE_HEIGHT)
            .px(inset)
            .rounded(RADIUS)
            .border(RULE)
            .border_color(border)
            .bg(track)
            .child(div().size(TOGGLE_KNOB).rounded(RADIUS_SMALL).bg(knob));
        let on = self.on;
        div()
            .debug_selector(|| self.id.to_string())
            .id(self.id)
            .flex()
            .items_center()
            .gap(SPACE_4)
            .cursor_pointer()
            .child(switch)
            .when_some(self.label, |toggle, label| {
                toggle.child(div().text_color(palette.fg2).child(label))
            })
            .when_some(self.handler, |toggle, handler| {
                toggle.on_click(move |_, window, cx| handler(&!on, window, cx))
            })
    }
}

#[derive(IntoElement)]
pub struct Checkbox {
    id: ElementId,
    check: Check,
    label: Option<SharedString>,
    handler: Option<ToggleHandler>,
}

impl Checkbox {
    pub fn new(id: impl Into<ElementId>, checked: bool) -> Self {
        let check = if checked { Check::On } else { Check::Off };
        Self::tristate(id, check)
    }

    pub fn tristate(id: impl Into<ElementId>, check: Check) -> Self {
        Self {
            id: id.into(),
            check,
            label: None,
            handler: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn on_toggle(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.handler = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Checkbox {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let box_ = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(CHECKBOX)
            .rounded(RADIUS_SMALL)
            .border(RULE)
            .text_size(TEXT_GLYPH);
        let mark = match self.check {
            Check::On => Some(assets::CHECK),
            Check::Mixed => Some(assets::DASH),
            Check::Off => None,
        };
        let box_ = match mark {
            Some(path) => box_
                .bg(palette.acc)
                .border_color(palette.acc)
                .text_color(palette.onacc)
                .child(
                    svg()
                        .path(path)
                        .size(CHECKBOX_TICK)
                        .text_color(palette.onacc),
                ),
            None => box_.bg(palette.editor).border_color(palette.line2),
        };
        let checked = self.check == Check::On;
        div()
            .debug_selector(|| self.id.to_string())
            .id(self.id)
            .flex()
            .items_center()
            .gap(SPACE_4)
            .cursor_pointer()
            .child(box_)
            .when_some(self.label, |checkbox, label| {
                checkbox.child(div().text_color(palette.fg2).child(label))
            })
            .when_some(self.handler, |checkbox, handler| {
                checkbox.on_click(move |_, window, cx| handler(&!checked, window, cx))
            })
    }
}
