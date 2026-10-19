use flock_core::Snapshot;
use flock_core::history::Direction;
use flock_core::tabs::{Tab, TabId};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, Render, SharedString, Styled, Window, div,
};

use super::{ActionMenu, IconButton, MenuItem};
use crate::assets::{CHEVRON_LEFT, CHEVRON_RIGHT};
use crate::theme::{ICON_BUTTON_INSET, SPACE_2};

#[derive(Clone, Copy)]
pub struct HistoryJump {
    pub tab: TabId,
    pub direction: Direction,
    pub steps: usize,
}

pub struct HeaderNavigation {
    pane: usize,
    tab: Option<TabId>,
    back: Vec<String>,
    forward: Vec<String>,
    menu: Entity<ActionMenu>,
}

impl EventEmitter<HistoryJump> for HeaderNavigation {}

impl HeaderNavigation {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            pane: 0,
            tab: None,
            back: Vec::new(),
            forward: Vec::new(),
            menu: cx.new(|cx| ActionMenu::context("history-menu", cx)),
        }
    }

    pub fn set_history(
        &mut self,
        pane: usize,
        tab: &Tab,
        snapshot: &Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let back = tab
            .history()
            .entries(Direction::Back)
            .map(|entry| entry.history_label(snapshot))
            .collect();
        let forward = tab
            .history()
            .entries(Direction::Forward)
            .map(|entry| entry.history_label(snapshot))
            .collect();
        let tab = tab.id();
        if self.pane == pane
            && self.tab == Some(tab)
            && self.back == back
            && self.forward == forward
        {
            return;
        }
        self.menu.update(cx, |menu, cx| menu.close(window, cx));
        self.pane = pane;
        self.tab = Some(tab);
        self.back = back;
        self.forward = forward;
        cx.notify();
    }

    fn entries(&self, direction: Direction) -> &[String] {
        match direction {
            Direction::Back => &self.back,
            Direction::Forward => &self.forward,
        }
    }

    fn jump(&self, direction: Direction, steps: usize, cx: &mut Context<Self>) {
        if let Some(tab) = self.tab {
            cx.emit(HistoryJump {
                tab,
                direction,
                steps,
            });
        }
    }

    fn open_history(
        &self,
        direction: Direction,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        let Some(tab) = self.tab else {
            return;
        };
        if self.entries(direction).is_empty() {
            return;
        }
        let control = cx.weak_entity();
        let items = self
            .entries(direction)
            .iter()
            .enumerate()
            .map(|(ix, label)| {
                let control = control.clone();
                let jump = HistoryJump {
                    tab,
                    direction,
                    steps: ix + 1,
                };
                MenuItem::new(label.clone(), move |_, cx| {
                    control.update(cx, |_, cx| cx.emit(jump)).ok();
                })
            })
            .collect();
        self.menu.update(cx, |menu, cx| {
            menu.show_at(event.position, items, window, cx);
        });
    }

    fn button(&self, direction: Direction, cx: &mut Context<Self>) -> AnyElement {
        let (name, path, shortcut) = match direction {
            Direction::Back => ("Back", CHEVRON_LEFT, "⌘["),
            Direction::Forward => ("Forward", CHEVRON_RIGHT, "⌘]"),
        };
        let entries = self.entries(direction);
        let tooltip = entries.first().map_or_else(
            || format!("{name} {shortcut}"),
            |label| format!("{name} to {label} {shortcut}"),
        );
        let id = SharedString::from(format!("pane-{}-{}", self.pane, name.to_lowercase()));
        let button = IconButton::svg(id, path, tooltip)
            .disabled(entries.is_empty())
            .on_click(cx.listener(move |this, _, _, cx| this.jump(direction, 1, cx)));
        div()
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event, window, cx| {
                    this.open_history(direction, event, window, cx);
                }),
            )
            .child(button)
            .into_any_element()
    }
}

impl Render for HeaderNavigation {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let back = self.button(Direction::Back, cx);
        let forward = self.button(Direction::Forward, cx);
        div()
            .flex()
            .flex_none()
            .ml(-ICON_BUTTON_INSET)
            .child(div().flex().gap(SPACE_2).child(back).child(forward))
            .child(self.menu.clone())
    }
}
