use std::time::Instant;

use flock_core::history::Location;
use flock_core::palette::COMMAND_PREFIX;
use flock_core::tabs::Cycle;
use flock_core::{Command, Page};
use gpui::{
    App, Context, Div, Entity, FocusHandle, Focusable, InteractiveElement, KeyDownEvent, Stateful,
    Window,
};

use super::Shell;
use crate::actions::{
    ActivateTab, CloseTab, CloseWindow, FocusContent, FocusPane, FocusSidebar, GoBack, GoForward,
    NewTab, NextTab, PreviousTab, QuickOpen, ShowCommands, SplitRight, ToggleDock,
};
use crate::kit::DockState;
use crate::kit::list_keys::{Nav, TypeAhead, find, typed};
use crate::screens::pods::PodsScreen;
use crate::screens::workload::WorkloadScreen;

const ESCAPE: &str = "escape";
pub(super) const SIDEBAR_CONTEXT: &str = "FlockSidebar";

#[derive(Clone, Debug, PartialEq)]
pub(super) struct NavCursor {
    page: Page,
    shown: Location,
}

pub(super) struct Keyboard {
    pub(super) refocus: bool,
    pub(super) sidebar_focus: FocusHandle,
    nav_cursor: Option<NavCursor>,
    pub(super) nav_type_ahead: TypeAhead,
}

impl Keyboard {
    pub(super) fn new(cx: &mut Context<Shell>) -> Self {
        Self {
            refocus: false,
            sidebar_focus: cx.focus_handle().tab_stop(true),
            nav_cursor: None,
            nav_type_ahead: TypeAhead::default(),
        }
    }
}

impl Shell {
    pub(super) fn key_actions(root: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        root.on_action(cx.listener(|_, _: &CloseWindow, window, _| {
            window.remove_window();
        }))
        .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
            let active = this.state.panes().active().id();
            this.close_tab(active, window, cx);
        }))
        .on_action(cx.listener(|this, _: &SplitRight, window, cx| {
            this.split_right(window, cx);
        }))
        .on_action(cx.listener(|this, action: &FocusPane, _, cx| {
            this.focus_pane(action.0, cx);
            this.keys.refocus = true;
        }))
        .on_action(cx.listener(|this, _: &NewTab, _, cx| {
            this.state.new_tab();
            this.show_active(cx);
        }))
        .on_action(cx.listener(|this, _: &NextTab, _, cx| {
            if this.state.panes_mut().cycle(Cycle::Next) {
                this.show_active(cx);
            }
        }))
        .on_action(cx.listener(|this, _: &PreviousTab, _, cx| {
            if this.state.panes_mut().cycle(Cycle::Previous) {
                this.show_active(cx);
            }
        }))
        .on_action(cx.listener(|this, action: &ActivateTab, _, cx| {
            if this.state.panes_mut().activate_nth(action.0) {
                this.show_active(cx);
            }
        }))
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            let keystroke = &event.keystroke;
            if keystroke.key == ESCAPE
                && !keystroke.modifiers.modified()
                && this.close_inspector(cx)
            {
                cx.stop_propagation();
                if let Some(focus) = this.list_focus(cx) {
                    window.focus(&focus, cx);
                }
            }
        }))
        .on_action(cx.listener(|this, _: &QuickOpen, window, cx| {
            this.open_palette("", window, cx);
        }))
        .on_action(cx.listener(|this, _: &ShowCommands, window, cx| {
            this.open_palette(COMMAND_PREFIX, window, cx);
        }))
        .on_action(cx.listener(|this, _: &ToggleDock, _, cx| {
            if let Some(dock) = this.dock(cx) {
                dock.update(cx, DockState::toggle);
            }
        }))
        .on_action(cx.listener(|this, _: &GoBack, _, cx| {
            if this.state.back() {
                this.show_active(cx);
            }
        }))
        .on_action(cx.listener(|this, _: &GoForward, _, cx| {
            if this.state.forward() {
                this.show_active(cx);
            }
        }))
        .on_action(cx.listener(|this, _: &FocusSidebar, window, cx| {
            this.focus_sidebar(window, cx);
        }))
        .on_action(cx.listener(|this, _: &FocusContent, window, cx| {
            this.focus_content(window, cx);
        }))
    }

    pub(super) fn keep_keyboard_focus(&mut self, window: &Window, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.keys.refocus) || self.keys.sidebar_focus.is_focused(window) {
            return;
        }
        let shell = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            let focus = shell
                .read_with(cx, |shell, cx| {
                    shell.page_focus(cx).unwrap_or_else(|| shell.focus.clone())
                })
                .ok();
            if let Some(focus) = focus {
                window.focus(&focus, cx);
            }
        });
    }

    fn page_focus(&self, cx: &App) -> Option<FocusHandle> {
        let screens = self.focused_screens();
        if let Some((screen, _)) = &screens.workload {
            return Some(screen.focus_handle(cx));
        }
        if let Some((screen, _)) = &screens.detail {
            return Some(screen.focus_handle(cx));
        }
        if let Some((screen, _)) = &screens.namespace {
            return Some(screen.focus_handle(cx));
        }
        self.state
            .location()
            .flux_object
            .as_ref()
            .map(|_| screens.flux_object.focus_handle(cx))
    }

    fn close_inspector(&mut self, cx: &mut Context<Self>) -> bool {
        let screens = self.focused_screens();
        if let Some((screen, _)) = &screens.workload {
            return screen.update(cx, WorkloadScreen::close_inspector);
        }
        if self.state.location().page == Page::Pods {
            return screens.pods.update(cx, PodsScreen::close_inspector);
        }
        screens
            .pages
            .close_inspector(self.state.location().page, cx)
    }

    pub fn dock_open(&self, cx: &App) -> Option<bool> {
        self.dock(cx).map(|dock| dock.read(cx).is_open())
    }

    fn dock(&self, cx: &App) -> Option<Entity<DockState>> {
        let screens = self.focused_screens();
        if let Some((screen, _)) = &screens.workload {
            return Some(screen.read(cx).dock().clone());
        }
        let location = self.state.location();
        let pods = location.page == Page::Pods && self.has_detail(location);
        pods.then(|| screens.pods.read(cx).dock().clone())
    }

    fn list_focus(&self, cx: &mut Context<Self>) -> Option<FocusHandle> {
        let screens = self.focused_screens();
        if let Some((screen, _)) = &screens.workload {
            return Some(screen.read(cx).list_focus(cx));
        }
        if let Some((screen, _)) = &screens.detail {
            return Some(screen.focus_handle(cx));
        }
        if let Some((screen, _)) = &screens.namespace {
            return Some(screen.focus_handle(cx));
        }
        if self.state.location().flux_object.is_some() {
            return Some(screens.flux_object.focus_handle(cx));
        }
        let page = self.state.location().page;
        match page {
            Page::Overview => Some(screens.flock.read(cx).list_focus(page, cx)),
            Page::Problems => Some(screens.flock.read(cx).list_focus(page, cx)),
            Page::Changes => Some(screens.changes.read(cx).list_focus(cx)),
            Page::FluxEvents => Some(screens.flux_events.read(cx).list_focus(cx)),
            Page::ClusterSettings => Some(screens.settings.read(cx).list_focus(cx)),
            Page::Pods => Some(screens.pods.read(cx).list_focus(cx)),
            page => screens.pages.list_focus(page, cx),
        }
    }

    fn focus_content(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(focus) = self.list_focus(cx) {
            window.focus(&focus, cx);
        } else {
            window.focus(&self.keys.sidebar_focus, cx);
            window.focus_next(cx);
        }
        cx.notify();
    }

    fn focus_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.sidebar_open {
            self.dispatch(Command::ToggleSidebar, cx);
        }
        self.move_nav_cursor(self.state.location().page);
        window.focus(&self.keys.sidebar_focus, cx);
        cx.notify();
    }

    fn move_nav_cursor(&mut self, page: Page) {
        let shown = self.state.location().clone();
        self.keys.nav_cursor = Some(NavCursor { page, shown });
    }

    pub(super) fn forget_stale_nav_cursor(&mut self) {
        if self.nav_cursor().is_none() {
            self.keys.nav_cursor = None;
        }
    }

    fn nav_cursor(&self) -> Option<Page> {
        let cursor = self.keys.nav_cursor.as_ref()?;
        (&cursor.shown == self.state.location()).then_some(cursor.page)
    }

    pub fn sidebar_highlight(&self, window: &Window) -> Page {
        let shown = self.state.location().page;
        if !self.keys.sidebar_focus.is_focused(window) {
            return shown;
        }
        self.nav_cursor().unwrap_or(shown)
    }

    pub(super) fn nav_pages(&self) -> Vec<Page> {
        self.state
            .snapshot
            .nav_sections(self.state.location().cluster.as_ref())
            .iter()
            .flat_map(|section| section.pages.iter().copied())
            .collect()
    }

    pub(super) fn sidebar_key(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        let pages = self.nav_pages();
        let current = self
            .nav_cursor()
            .or(Some(self.state.location().page))
            .and_then(|page| pages.iter().position(|p| *p == page));
        let opens =
            matches!(keystroke.key.as_str(), "enter" | "space") && !keystroke.modifiers.modified();
        if opens {
            if let Some(page) = current.and_then(|ix| pages.get(ix).copied()) {
                self.dispatch(Command::SelectPage(page), cx);
            }
            cx.stop_propagation();
            return;
        }
        let target = match (Nav::parse(keystroke), typed(keystroke)) {
            (Some(nav), _) => nav.target(current, pages.len()),
            (None, Some(text)) => {
                let query = self
                    .keys
                    .nav_type_ahead
                    .push(text, Instant::now())
                    .to_owned();
                find(&query, pages.iter().map(|page| page.label()))
            }
            (None, None) => return,
        };
        cx.stop_propagation();
        if let Some(page) = target.and_then(|ix| pages.get(ix).copied()) {
            self.move_nav_cursor(page);
            cx.notify();
        }
    }
}
