use flock_core::tabs::{Tab, TabId, TabTitle, title};
use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, Context, ElementId, Hsla, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, svg,
};

use super::Shell;
use super::tab_motion::{self, Ghost};
use crate::assets::SPLIT;
use crate::kit::{MenuItem, Tooltip, status_symbols};
use crate::theme::{
    ActiveTheme, FONT_MONO, FONT_STATUS, ICON_SPLIT, Palette, RULE, SPACE_3, SPACE_4, SPACE_6,
    SPLIT_BUTTON, TAB_CLOSE, TAB_ENTER, TEXT_BODY, TEXT_GLYPH, TEXT_LABEL, TEXT_UI, TITLE_ROW,
};

const CLOSE_GLYPH: &str = "×";
const CLOSE_TOOLTIP: &str = "Close tab  ⌘W";
const SPLIT_TOOLTIP: &str = "Split right  ⌘\\";
pub(super) const TAB_MENU: &str = "tab-menu";
const MOVE_TO_OTHER: &str = "Move to the other pane";
const MOVE_TO_OTHER_DESCRIPTION: &str = "Keep this tab and show it beside the other pane's tabs.";
const MOVE_TO_SPLIT: &str = "Move to a split on the right";
const MOVE_TO_SPLIT_DESCRIPTION: &str = "Show this tab in its own pane beside the others.";
const CLOSE_ITEM: &str = "Close tab";
const CLOSE_KEYS: &str = "⌘W";

#[derive(Clone)]
pub(super) struct DraggedTab {
    id: TabId,
    title: TabTitle,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        div()
            .flex()
            .h(TITLE_ROW)
            .bg(palette.editor)
            .border(RULE)
            .border_color(palette.line2)
            .text_color(palette.fg)
            .child(tab_content(&self.title, true, &palette))
    }
}

impl Shell {
    pub(super) fn tab_strip(
        &mut self,
        pane_ix: usize,
        leading: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = *cx.palette();
        let Some(pane) = self.state.panes().panes().get(pane_ix) else {
            return div().into_any_element();
        };
        let focused = pane_ix == self.state.panes().focused_index();
        let active = pane.active_index();
        let active_id = pane.active().id();
        let tabs: Vec<(usize, Tab)> = pane.tabs().iter().cloned().enumerate().collect();
        let current: Vec<(TabId, TabTitle)> = tabs
            .iter()
            .map(|(_, tab)| (tab.id(), title(tab.location(), &self.state.snapshot)))
            .collect();
        let animate = !cx.reduce_motion();
        let screens = self.screens_mut(pane_ix);
        let changed = screens.motion.track(&current, animate);
        if screens.shown != Some(active_id) {
            screens.shown = Some(active_id);
            let child = screens.motion.child_index(active);
            screens.strip.scroll_to_item(child);
        }
        let scroll = screens.strip.clone();
        if changed {
            self.settle_strip(pane_ix, cx);
        }
        let end = tabs.len();
        let mut ghosts = self.screens(pane_ix).motion.ghosts.clone();
        let mut children: Vec<AnyElement> = Vec::new();
        for (ix, tab) in tabs {
            while let Some(position) = ghosts.iter().position(|ghost| ghost.before <= ix) {
                let gone = ghosts.remove(position);
                children.push(leaving_tab(pane_ix, &gone, &palette));
            }
            let shown = Shown {
                pane: pane_ix,
                ix,
                active: ix == active,
                focused,
            };
            let element = self.tab(shown, &tab, &palette, cx);
            let slot = slot_selector(pane_ix, ix);
            children.push(self.screens(pane_ix).motion.wrap(tab.id(), slot, element));
        }
        for gone in ghosts {
            children.push(leaving_tab(pane_ix, &gone, &palette));
        }
        let rest = div()
            .id(("tab-strip-rest", pane_ix))
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .drag_over::<DraggedTab>(move |style, _, _, _| style.bg(palette.hover))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                this.drop_tab(dragged.id, pane_ix, end, window, cx);
            }))
            .child(div().flex_1())
            .child(div().h(RULE).bg(palette.line));
        let tabs = div()
            .id(("tab-strip-tabs", pane_ix))
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_x_scroll()
            .track_scroll(&scroll)
            .children(children)
            .child(rest);
        let split = split_button(pane_ix, &palette, cx);
        div()
            .debug_selector(move || strip_selector(pane_ix))
            .flex()
            .flex_none()
            .w_full()
            .h(TITLE_ROW)
            .bg(palette.chrome)
            .children(leading)
            .child(tabs)
            .child(split)
            .into_any_element()
    }

    fn settle_strip(&mut self, pane_ix: usize, cx: &mut Context<Self>) {
        let pane = self.screens(pane_ix).pane;
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TAB_ENTER).await;
            this.update(cx, |shell, cx| {
                if let Some(screens) = shell.panes.iter_mut().find(|screens| screens.pane == pane) {
                    screens.motion.finish();
                    screens.shown = None;
                }
                cx.notify();
            })
            .ok();
        });
        self.screens_mut(pane_ix).motion.timer = Some(timer);
    }

    fn open_tab_menu(
        &mut self,
        id: TabId,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panes = self.state.panes();
        let Some(pane_ix) = panes.pane_of(id) else {
            return;
        };
        let shell = cx.weak_entity();
        let run = move |change: fn(&mut Shell, TabId, &mut Window, &mut Context<Shell>)| {
            let shell = shell.clone();
            move |window: &mut Window, cx: &mut App| {
                shell
                    .update(cx, |shell, cx| change(shell, id, window, cx))
                    .ok();
            }
        };
        let mut items = Vec::new();
        if panes.panes().len() > 1 {
            items.push(
                MenuItem::new(MOVE_TO_OTHER, run(Shell::move_to_other_pane))
                    .description(MOVE_TO_OTHER_DESCRIPTION),
            );
        } else {
            let alone = panes
                .panes()
                .get(pane_ix)
                .is_none_or(|p| p.tabs().len() < 2);
            items.push(
                MenuItem::new(MOVE_TO_SPLIT, run(Shell::move_to_split))
                    .description(MOVE_TO_SPLIT_DESCRIPTION)
                    .disabled(alone),
            );
        }
        items.push(MenuItem::new(CLOSE_ITEM, run(Shell::close_tab)).shortcut(CLOSE_KEYS));
        self.tab_menu
            .update(cx, |menu, cx| menu.show_at(position, items, window, cx));
    }

    fn move_to_other_pane(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let panes = self.state.panes();
        let Some(from) = panes.pane_of(id) else {
            return;
        };
        let to = (from + 1) % panes.panes().len();
        let end = panes.panes().get(to).map_or(0, |pane| pane.tabs().len());
        if self.state.panes_mut().move_tab(id, to, end) {
            self.panes_changed(window, cx);
        }
    }

    fn move_to_split(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.panes_mut().split_off(id) {
            self.panes_changed(window, cx);
        }
    }

    fn tab(
        &self,
        shown: Shown,
        tab: &Tab,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Shown {
            pane,
            ix,
            active,
            focused,
        } = shown;
        let id = tab.id();
        let title = title(tab.location(), &self.state.snapshot);
        let hover = palette.hover;
        let top = if active && focused {
            palette.acc
        } else {
            gpui::transparent_black()
        };
        let bottom = if active { palette.editor } else { palette.line };
        let dragged = DraggedTab {
            id,
            title: title.clone(),
        };
        let close = close_button(shown, id, palette, cx);
        div()
            .debug_selector(move || tab_selector(pane, ix))
            .id(ElementId::NamedInteger("tab".into(), id.as_u64()))
            .flex()
            .flex_none()
            .h_full()
            .cursor_pointer()
            .text_color(palette.mute)
            .when(active, |tab| tab.bg(palette.editor).text_color(palette.fg))
            .when(!active, |tab| tab.hover(move |style| style.bg(hover)))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if this.state.panes_mut().activate(id) {
                    this.show_active(cx);
                }
            }))
            .on_aux_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if event.is_middle_click() {
                    this.close_tab(id, window, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.open_tab_menu(id, event.position, window, cx);
                }),
            )
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .drag_over::<DraggedTab>(move |style, _, _, _| style.bg(hover))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                this.drop_tab(dragged.id, pane, ix, window, cx);
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .h_full()
                    .child(div().h(RULE).bg(top))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .items_center()
                            .child(tab_content(&title, active, palette))
                            .child(close),
                    )
                    .child(div().h(RULE).bg(bottom)),
            )
            .child(div().w(RULE).h_full().bg(palette.line))
            .into_any_element()
    }

    fn drop_tab(
        &mut self,
        id: TabId,
        pane: usize,
        to: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let across = self.state.panes().pane_of(id) != Some(pane);
        if !self.state.panes_mut().move_tab(id, pane, to) {
            return;
        }
        if across {
            self.panes_changed(window, cx);
        } else {
            self.save_tabs(cx);
            cx.notify();
        }
    }
}

#[derive(Clone, Copy)]
struct Shown {
    pane: usize,
    ix: usize,
    active: bool,
    focused: bool,
}

fn split_button(pane: usize, palette: &Palette, cx: &mut Context<Shell>) -> AnyElement {
    let hover = palette.hover;
    let selector = if pane == 0 {
        "split-right".to_owned()
    } else {
        format!("pane-{pane}-split-right")
    };
    div()
        .debug_selector(move || selector.clone())
        .id(("split-right", pane))
        .flex()
        .flex_col()
        .flex_none()
        .w(SPLIT_BUTTON)
        .h_full()
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .tooltip(Tooltip::text(SPLIT_TOOLTIP))
        .on_mouse_down(MouseButton::Left, |_, _, cx: &mut App| {
            cx.stop_propagation();
        })
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.focus_pane(pane, cx);
            this.split_right(window, cx);
        }))
        .child(
            div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(svg().path(SPLIT).size(ICON_SPLIT).text_color(palette.mute)),
        )
        .child(div().h(RULE).bg(palette.line))
        .into_any_element()
}

fn strip_selector(pane: usize) -> String {
    if pane == 0 {
        "tab-strip".to_owned()
    } else {
        format!("tab-strip-{pane}")
    }
}

fn tab_selector(pane: usize, ix: usize) -> String {
    if pane == 0 {
        format!("tab-{ix}")
    } else {
        format!("pane-{pane}-tab-{ix}")
    }
}

fn leaving_tab(pane: usize, leaving: &Ghost, palette: &Palette) -> AnyElement {
    let content = div()
        .flex()
        .flex_none()
        .h_full()
        .text_color(palette.mute)
        .child(
            div()
                .flex()
                .flex_col()
                .h_full()
                .child(div().h(RULE))
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .items_center()
                        .child(tab_content(&leaving.title, false, palette))
                        .child(div().w(TAB_CLOSE).mr(SPACE_4)),
                )
                .child(div().h(RULE).bg(palette.line)),
        )
        .child(div().w(RULE).h_full().bg(palette.line))
        .into_any_element();
    tab_motion::ghost(leaving, leaving_selector(pane, leaving.before), content)
}

fn slot_selector(pane: usize, ix: usize) -> String {
    if pane == 0 {
        format!("tab-slot-{ix}")
    } else {
        format!("pane-{pane}-tab-slot-{ix}")
    }
}

fn leaving_selector(pane: usize, ix: usize) -> String {
    if pane == 0 {
        format!("tab-leaving-{ix}")
    } else {
        format!("pane-{pane}-tab-leaving-{ix}")
    }
}

fn tab_content(title: &TabTitle, active: bool, palette: &Palette) -> impl IntoElement {
    let glyph = title.status.map(|status| {
        div()
            .font_family(FONT_STATUS)
            .text_size(TEXT_GLYPH)
            .text_color(palette.status(status))
            .child(status_symbols(status.glyph()))
    });
    let label_color: Hsla = if active { palette.fg } else { palette.mute };
    div()
        .flex()
        .items_center()
        .gap(SPACE_3)
        .pl(SPACE_6)
        .pr(SPACE_3)
        .whitespace_nowrap()
        .text_size(TEXT_BODY)
        .children(glyph)
        .children((!title.kind.is_empty()).then(|| {
            div()
                .font_family(FONT_MONO)
                .text_size(TEXT_LABEL)
                .text_color(palette.mute)
                .child(SharedString::from(title.kind.clone()))
        }))
        .child(
            div()
                .text_color(label_color)
                .child(SharedString::from(title.label.clone())),
        )
        .children(title.cluster.clone().map(|cluster| {
            div()
                .text_color(palette.mute)
                .child(SharedString::from(cluster))
        }))
}

fn close_button(shown: Shown, id: TabId, palette: &Palette, cx: &mut Context<Shell>) -> AnyElement {
    let Shown {
        pane, ix, active, ..
    } = shown;
    let fg = palette.fg;
    let selector = if pane == 0 {
        format!("tab-close-{ix}")
    } else {
        format!("pane-{pane}-tab-close-{ix}")
    };
    div()
        .debug_selector(move || selector.clone())
        .id(ElementId::NamedInteger("tab-close".into(), id.as_u64()))
        .flex()
        .items_center()
        .justify_center()
        .w(TAB_CLOSE)
        .mr(SPACE_4)
        .text_size(TEXT_UI)
        .text_color(palette.mute)
        .when(!active, Styled::invisible)
        .hover(move |style| style.text_color(fg))
        .tooltip(Tooltip::text(CLOSE_TOOLTIP))
        .on_mouse_down(MouseButton::Left, |_, _, cx: &mut App| {
            cx.stop_propagation();
        })
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            cx.stop_propagation();
            this.close_tab(id, window, cx);
        }))
        .child(CLOSE_GLYPH)
        .into_any_element()
}
