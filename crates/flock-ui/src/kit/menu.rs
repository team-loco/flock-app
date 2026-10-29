use std::rc::Rc;

use gpui::{
    App, Context, ElementId, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, ParentElement, Pixels, Point, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Window, canvas, div, prelude::FluentBuilder, relative, svg,
};

use super::button::{Button, IconButton};
use super::popover::{self, Dismissal, Step};
use crate::assets::CHEVRON_DOWN;
use crate::theme::{
    ActiveTheme, CHECK_SLOT, CONTROL, FONT_MONO, ICON, MENU_WIDTH, RADIUS, ROW_MENU_PAD_Y, RULE,
    SPACE_1, SPACE_3, SPACE_4, SPACE_5, TEXT_BODY, TEXT_LABEL, TEXT_MONO, TEXT_SMALL,
};

const CHECK: &str = "✓";
const MORE: &str = "⋯";
const MORE_TOOLTIP: &str = "More actions";
const DESCRIPTION_LINE: f32 = 1.35;

pub type MenuHandler = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone)]
pub struct MenuItem {
    label: SharedString,
    description: Option<SharedString>,
    shortcut: Option<SharedString>,
    destructive: bool,
    disabled: bool,
    checked: Option<bool>,
    handler: MenuHandler,
}

impl MenuItem {
    pub fn new(
        label: impl Into<SharedString>,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            description: None,
            shortcut: None,
            destructive: false,
            disabled: false,
            checked: None,
            handler: Rc::new(handler),
        }
    }

    pub fn description(mut self, text: impl Into<SharedString>) -> Self {
        self.description = Some(text.into());
        self
    }

    pub fn shortcut(mut self, keys: impl Into<SharedString>) -> Self {
        self.shortcut = Some(keys.into());
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

enum Trigger {
    Hidden,
    More,
    Label {
        label: SharedString,
        shortcut: Option<SharedString>,
    },
    Prefixed {
        icon: &'static str,
        prefix: SharedString,
        label: SharedString,
    },
}

pub struct ActionMenu {
    id: SharedString,
    trigger: Trigger,
    width: Pixels,
    bordered: bool,
    items: Vec<MenuItem>,
    anchor: Option<Point<Pixels>>,
    open: bool,
    highlighted: Option<usize>,
    generation: u64,
    dismissal: Dismissal,
    focus: FocusHandle,
    restore: Option<FocusHandle>,
    scroll: ScrollHandle,
    fit: popover::Fit,
}

impl ActionMenu {
    pub fn new(id: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            id: id.into(),
            trigger: Trigger::More,
            width: MENU_WIDTH,
            bordered: true,
            items: Vec::new(),
            anchor: None,
            open: false,
            highlighted: None,
            generation: 0,
            dismissal: Dismissal::default(),
            focus: cx.focus_handle(),
            restore: None,
            scroll: ScrollHandle::default(),
            fit: popover::Fit::default(),
        }
    }

    pub fn labeled(
        mut self,
        label: impl Into<SharedString>,
        shortcut: Option<SharedString>,
    ) -> Self {
        self.trigger = Trigger::Label {
            label: label.into(),
            shortcut,
        };
        self
    }

    pub fn prefixed(
        mut self,
        icon: &'static str,
        prefix: impl Into<SharedString>,
        label: impl Into<SharedString>,
    ) -> Self {
        self.trigger = Trigger::Prefixed {
            icon,
            prefix: prefix.into(),
            label: label.into(),
        };
        self
    }

    pub fn set_label(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        let text = text.into();
        match &mut self.trigger {
            Trigger::Label { label, .. } => *label = text,
            Trigger::Prefixed { label, .. } => *label = text,
            Trigger::Hidden => return,
            Trigger::More => return,
        }
        cx.notify();
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    pub fn context(id: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        let mut menu = Self::new(id, cx);
        menu.trigger = Trigger::Hidden;
        menu
    }

    pub fn show_at(
        &mut self,
        position: Point<Pixels>,
        items: Vec<MenuItem>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.items = items;
        self.anchor = Some(position);
        self.show(window, cx);
    }

    pub fn plain(mut self) -> Self {
        self.bordered = false;
        self
    }

    pub fn items(mut self, items: Vec<MenuItem>) -> Self {
        self.items = items;
        self
    }

    pub fn set_items(&mut self, items: Vec<MenuItem>, cx: &mut Context<Self>) {
        self.items = items;
        self.highlighted = None;
        cx.notify();
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.close(window, cx);
        } else {
            self.show(window, cx);
        }
    }

    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.generation += 1;
        self.highlighted = None;
        self.restore = window.focused(cx);
        let focus = self.focus.clone();
        window.defer(cx, move |window, cx| window.focus(&focus, cx));
        cx.notify();
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        if let Some(previous) = self.restore.take() {
            window.focus(&previous, cx);
        }
        cx.notify();
    }

    fn enabled(&self, ix: usize) -> bool {
        self.items.get(ix).is_some_and(|item| !item.disabled)
    }

    fn move_highlight(&mut self, direction: Step, cx: &mut Context<Self>) {
        let next = popover::step(
            self.highlighted,
            self.items.len(),
            |ix| self.enabled(ix),
            direction,
        );
        self.highlighted = next;
        if let Some(ix) = next {
            self.scroll.scroll_to_item(ix);
        }
        cx.notify();
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.items.get(ix).filter(|item| !item.disabled) else {
            return;
        };
        let handler = item.handler.clone();
        self.close(window, cx);
        handler(window, cx);
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "down" => self.move_highlight(Step::Next, cx),
            "up" => self.move_highlight(Step::Previous, cx),
            "enter" => {
                if let Some(ix) = self.highlighted {
                    self.activate(ix, window, cx);
                }
            }
            "escape" => self.close(window, cx),
            key => match self.shortcut_item(key, event.keystroke.modifiers) {
                Some(ix) => self.activate(ix, window, cx),
                None => return,
            },
        }
        cx.stop_propagation();
    }

    fn shortcut_item(&self, key: &str, modifiers: gpui::Modifiers) -> Option<usize> {
        if modifiers.modified() {
            return None;
        }
        self.items
            .iter()
            .position(|item| !item.disabled && item.shortcut.as_deref() == Some(key))
    }

    fn render_item(&self, ix: usize, item: &MenuItem, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let label_color = if item.disabled {
            palette.mute
        } else if item.destructive {
            palette.err
        } else {
            palette.fg
        };
        let highlighted = self.highlighted == Some(ix) && !item.disabled;
        let top = div()
            .flex()
            .items_center()
            .gap(SPACE_5)
            .when_some(item.checked, |row, checked| {
                row.child(
                    div()
                        .flex_none()
                        .w(CHECK_SLOT)
                        .text_size(TEXT_BODY)
                        .text_color(label_color)
                        .child(if checked { CHECK } else { "" }),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(TEXT_BODY)
                    .text_color(label_color)
                    .child(item.label.clone()),
            )
            .when_some(item.shortcut.clone(), |row, keys| {
                row.child(
                    div()
                        .font_family(FONT_MONO)
                        .text_size(TEXT_LABEL)
                        .text_color(palette.mute)
                        .child(keys),
                )
            });
        div()
            .debug_selector(|| format!("{}-item-{ix}", self.id))
            .id(ix)
            .flex_none()
            .flex()
            .flex_col()
            .gap(SPACE_1 / 2.)
            .px(SPACE_4)
            .py(ROW_MENU_PAD_Y)
            .rounded(RADIUS)
            .when(highlighted, |row| row.bg(palette.hover))
            .when(!item.disabled, Styled::cursor_pointer)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.enabled(ix) {
                    this.highlighted = Some(ix);
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, window, cx| this.activate(ix, window, cx)))
            .child(top)
            .when_some(item.description.clone(), |row, text| {
                row.child(
                    div()
                        .text_size(TEXT_SMALL)
                        .line_height(relative(DESCRIPTION_LINE))
                        .text_color(palette.mute)
                        .child(text),
                )
            })
    }

    fn render_trigger(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let id = ElementId::Name(self.id.clone());
        let click = cx.listener(|this: &mut Self, _, window, cx| {
            if this.dismissal.take_suppressed() {
                return;
            }
            this.toggle(window, cx);
        });
        match &self.trigger {
            Trigger::Hidden => div().into_any_element(),
            Trigger::More => IconButton::glyph(id, MORE, MORE_TOOLTIP)
                .when(self.bordered, IconButton::bordered)
                .selected(self.open)
                .on_click(click)
                .into_any_element(),
            Trigger::Label { label, shortcut } => {
                let button = Button::new(id, label.clone())
                    .selected(self.open)
                    .on_click(click);
                let button = match shortcut {
                    Some(keys) => button.shortcut(keys.clone()),
                    None => button,
                };
                button.into_any_element()
            }
            Trigger::Prefixed {
                icon,
                prefix,
                label,
            } => {
                let palette = *cx.palette();
                let hover = palette.hover;
                let glyph = |path: &'static str| svg().path(path).size(ICON).flex_none();
                div()
                    .debug_selector(|| self.id.to_string())
                    .id(id)
                    .flex()
                    .items_center()
                    .gap(SPACE_3)
                    .h(CONTROL)
                    .px(SPACE_4)
                    .rounded(RADIUS)
                    .border(RULE)
                    .border_color(palette.line2)
                    .font_family(FONT_MONO)
                    .text_size(TEXT_MONO)
                    .text_color(palette.fg)
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .when(self.open, |trigger| trigger.bg(palette.sel))
                    .when(!self.open, |trigger| {
                        trigger.hover(move |style| style.bg(hover))
                    })
                    .child(glyph(icon).text_color(palette.mute))
                    .child(div().text_color(palette.mute).child(prefix.clone()))
                    .child(label.clone())
                    .child(glyph(CHEVRON_DOWN).text_color(palette.mute))
                    .on_click(click)
                    .into_any_element()
            }
        }
    }
}

impl Focusable for ActionMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ActionMenu {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let trigger = self.render_trigger(cx);
        let popover = self.open.then(|| {
            let items: Vec<_> = self
                .items
                .iter()
                .enumerate()
                .map(|(ix, item)| self.render_item(ix, item, cx).into_any_element())
                .collect();
            let owner = cx.entity().downgrade();
            let scroll = self.scroll.clone();
            let count = self.items.len();
            let measure = canvas(
                move |_, _, cx| {
                    let heights: Vec<Pixels> = (0..count)
                        .filter_map(|ix| scroll.bounds_for_item(ix))
                        .map(|bounds| bounds.size.height)
                        .collect();
                    owner
                        .update(cx, |menu, cx| {
                            if menu.fit.observe(&heights) {
                                cx.notify();
                            }
                        })
                        .ok();
                },
                |_, (), _, _| {},
            )
            .absolute()
            .size_0();
            let surface = popover::surface("menu", &palette)
                .debug_selector(|| format!("{}-surface", self.id))
                .w(self.width)
                .max_h(self.fit.height())
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .track_focus(&self.focus)
                .on_key_down(cx.listener(Self::key_down))
                .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                    this.dismissal.clicked_outside();
                    this.close(window, cx);
                }))
                .children(items)
                .child(measure);
            let key = ElementId::NamedInteger(self.id.clone(), self.generation);
            let shown = popover::appear(surface, key);
            match self.anchor {
                Some(position) => popover::at(position, shown).into_any_element(),
                None => popover::below_right(CONTROL, shown).into_any_element(),
            }
        });
        div()
            .id(ElementId::NamedInteger(self.id.clone(), 0))
            .relative()
            .flex_none()
            .on_hover(cx.listener(|this, hovered: &bool, _, _| {
                this.dismissal.hover(*hovered);
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(trigger)
            .children(popover)
    }
}
