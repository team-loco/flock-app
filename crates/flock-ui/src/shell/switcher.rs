use std::rc::Rc;
use std::time::Instant;

use flock_core::scope::{self, Check, Scope};
use flock_core::settings::ClusterAction;
use flock_core::switcher::{self, ADD_CLUSTER, ALL_CLUSTERS, Entry};
use flock_core::{ClusterId, ClusterSummary, Command};
use gpui::{
    AnyElement, App, Context, ElementId, EventEmitter, FocusHandle, FontWeight, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, svg,
};

use super::Shell;
use crate::assets::{CHECK, CHEVRON_UP, CLUSTER, CLUSTERS, DASH, GEAR};
use crate::kit::list_keys::{Nav, TypeAhead, find, typed};
use crate::kit::{
    ConfirmDialog, Dismissal, MenuItem, Tooltip, above_right, appear, confirm, surface,
};
use crate::screens::settings::DISCONNECT;
use crate::theme::{
    ActiveTheme, FONT_MONO, FONT_SANS, ICON, Palette, RADIUS, ROW_CLUSTER, ROW_NAV, RULE, SPACE_1,
    SPACE_2, SPACE_3, SPACE_4, SPACE_5, SPACE_6, SWITCHER_CHECK, SWITCHER_CHEVRON, SWITCHER_GEAR,
    SWITCHER_ICON, SWITCHER_LIST, SWITCHER_MENU, SWITCHER_TICK, SWITCHER_WIDTH, TEXT_BODY,
    TEXT_LABEL, TEXT_UI,
};

pub const SWITCH_KEY: &str = "⌘⇧C";
const ID: &str = "cluster-switcher";
const MORE_ID: &str = "cluster-switcher-more";
const TOOLTIP: &str = "Clusters";
const ADD_GLYPH: &str = "+";
const GEAR_TOOLTIP: &str = "Cluster settings";
const ACTIONS_KEY: &str = ".";
const ENTER: &str = "enter";
const SPACE: &str = "space";
const ESCAPE: &str = "escape";
const MENU_ID: &str = "switcher-menu";
const SETTINGS: &str = "Settings…";
const RENAME: &str = "Rename…";
const SWITCH_CONTEXT: &str = "Switch context…";
const REMOVE: &str = "Remove";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwitcherEvent {
    Select(Scope),
    AddCluster,
    Run(Command),
    Remove(ClusterId),
    Rename(ClusterId),
    SwitchContext(ClusterId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Settings,
    Rename,
    SwitchContext,
    Connect,
    Disconnect,
    Remove,
}

impl Action {
    pub(super) fn for_cluster(cluster: &ClusterSummary) -> [Self; 5] {
        let connection = if cluster.is_connected() {
            Self::Disconnect
        } else {
            Self::Connect
        };
        [
            Self::Settings,
            Self::Rename,
            Self::SwitchContext,
            connection,
            Self::Remove,
        ]
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Settings => SETTINGS,
            Self::Rename => RENAME,
            Self::SwitchContext => SWITCH_CONTEXT,
            Self::Connect => ClusterAction::Connect.label(),
            Self::Disconnect => DISCONNECT,
            Self::Remove => REMOVE,
        }
    }

    const fn opens_dialog(self) -> bool {
        matches!(self, Self::Rename | Self::SwitchContext)
    }

    pub(super) fn event(self, id: &ClusterId) -> SwitcherEvent {
        match self {
            Self::Settings => SwitcherEvent::Run(Command::ShowClusterSettings(id.clone())),
            Self::Connect => SwitcherEvent::Run(Command::Connect(id.clone())),
            Self::Disconnect => SwitcherEvent::Run(Command::Disconnect(id.clone())),
            Self::Rename => SwitcherEvent::Rename(id.clone()),
            Self::SwitchContext => SwitcherEvent::SwitchContext(id.clone()),
            Self::Remove => SwitcherEvent::Remove(id.clone()),
        }
    }
}

pub(crate) type Run = Rc<dyn Fn(Command, &mut App)>;

fn remove_dialog(id: ClusterId, run: Run) -> ConfirmDialog {
    let title = format!("Remove {id}");
    let body = format!(
        "Flock disconnects from {id} and stops watching it. Your kubeconfig file is not \
         changed, so you can add the context again from Add cluster."
    );
    ConfirmDialog::new(title, body, REMOVE, move |_, cx: &mut App| {
        run(Command::RemoveCluster(id.clone()), cx);
    })
}

pub(crate) fn cluster_items(cluster: &ClusterSummary, run: &Run) -> Vec<MenuItem> {
    Action::for_cluster(cluster)
        .into_iter()
        .filter(|action| !action.opens_dialog())
        .map(|action| {
            let id = cluster.id.clone();
            let run = run.clone();
            let item = MenuItem::new(action.label(), move |window, cx| match action.event(&id) {
                SwitcherEvent::Run(command) => run(command, cx),
                SwitcherEvent::Remove(id) => confirm(remove_dialog(id, run.clone()), window, cx),
                SwitcherEvent::Select(_) => {}
                SwitcherEvent::AddCluster => {}
                SwitcherEvent::Rename(_) => {}
                SwitcherEvent::SwitchContext(_) => {}
            });
            match action {
                Action::Remove => item.destructive(),
                Action::Settings => item,
                Action::Rename => item,
                Action::SwitchContext => item,
                Action::Connect => item,
                Action::Disconnect => item,
            }
        })
        .collect()
}

fn starts_group(rows: &[Entry], ix: usize) -> bool {
    let Some(row) = rows.get(ix) else {
        return false;
    };
    ix.checked_sub(1)
        .and_then(|before| rows.get(before))
        .is_some_and(|previous| std::mem::discriminant(previous) != std::mem::discriminant(row))
}

fn child_index(rows: &[Entry], ix: usize) -> usize {
    let rules = (0..=ix).filter(|&at| starts_group(rows, at)).count();
    ix + rules
}

pub struct ClusterSwitcher {
    clusters: Vec<ClusterSummary>,
    scope: Scope,
    rows: Vec<Entry>,
    highlighted: Option<usize>,
    steered: bool,
    actions_for: Option<ClusterId>,
    action_highlighted: Option<usize>,
    menu_pressed: bool,
    open: bool,
    generation: u64,
    dismissal: Dismissal,
    restore: Option<FocusHandle>,
    focus: FocusHandle,
    type_ahead: TypeAhead,
    scroll: ScrollHandle,
}

impl EventEmitter<SwitcherEvent> for ClusterSwitcher {}

impl ClusterSwitcher {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            clusters: Vec::new(),
            scope: Scope::All,
            rows: Vec::new(),
            highlighted: None,
            steered: false,
            actions_for: None,
            action_highlighted: None,
            menu_pressed: false,
            open: false,
            generation: 0,
            dismissal: Dismissal::default(),
            restore: None,
            focus: cx.focus_handle(),
            type_ahead: TypeAhead::default(),
            scroll: ScrollHandle::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn label(&self) -> String {
        let (label, more) = self.scope.trigger(&self.clusters);
        more.map_or_else(|| label.clone(), |more| format!("{label} {more}"))
    }

    pub fn set_context(
        &mut self,
        clusters: &[ClusterSummary],
        scope: Scope,
        cx: &mut Context<Self>,
    ) {
        let moved = self.scope.cluster() != scope.cluster();
        self.clusters = clusters.to_vec();
        self.scope = scope;
        let gone = self
            .actions_for
            .as_ref()
            .is_some_and(|id| self.cluster(id).is_none());
        if gone {
            self.close_actions();
        }
        let kept = self
            .highlighted
            .filter(|_| !moved && self.steered)
            .and_then(|ix| self.rows.get(ix))
            .cloned();
        self.rows = switcher::rows(&self.clusters);
        let position = kept.and_then(|entry| self.rows.iter().position(|r| *r == entry));
        self.highlighted = position.or_else(|| self.initial());
        cx.notify();
    }

    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.close(window, cx);
        } else {
            self.show(window, cx);
        }
    }

    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            self.restore = window.focused(cx);
            self.generation += 1;
        }
        self.open = true;
        self.steered = false;
        self.close_actions();
        self.rows = switcher::rows(&self.clusters);
        self.highlighted = self.initial();
        self.scroll_to_highlight();
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        self.close_actions();
        if let Some(previous) = self.restore.take() {
            window.focus(&previous, cx);
        }
        cx.notify();
    }

    fn cluster(&self, id: &ClusterId) -> Option<&ClusterSummary> {
        self.clusters.iter().find(|cluster| &cluster.id == id)
    }

    fn initial(&self) -> Option<usize> {
        switcher::initial_highlight(&self.rows, self.scope.cluster())
    }

    fn actions(&self) -> Option<(ClusterId, [Action; 5])> {
        let id = self.actions_for.as_ref()?;
        let cluster = self.cluster(id)?;
        Some((id.clone(), Action::for_cluster(cluster)))
    }

    fn scroll_to_highlight(&self) {
        if let Some(ix) = self.highlighted {
            self.scroll.scroll_to_item(child_index(&self.rows, ix));
        }
    }

    fn jump(&mut self, nav: Nav, cx: &mut Context<Self>) {
        if let Some((_, actions)) = self.actions() {
            self.action_highlighted = nav.target(self.action_highlighted, actions.len());
        } else {
            self.highlighted = nav.target(self.highlighted, self.rows.len());
            self.steered = true;
            self.scroll_to_highlight();
        }
        cx.notify();
    }

    fn type_to(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.actions_for.is_some() {
            return;
        }
        let query = self.type_ahead.push(text, Instant::now()).to_owned();
        let labels = self
            .rows
            .iter()
            .map(|row| switcher::label(row, &self.clusters));
        if let Some(ix) = find(&query, labels) {
            self.highlighted = Some(ix);
            self.steered = true;
            self.scroll_to_highlight();
            cx.notify();
        }
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let event = self.rows.get(ix).map(|row| match row {
            Entry::AllClusters => SwitcherEvent::Select(Scope::All),
            Entry::Cluster(id) => SwitcherEvent::Select(Scope::One(id.clone())),
            Entry::AddCluster => SwitcherEvent::AddCluster,
        });
        self.emit_and_close(event, window, cx);
    }

    fn toggle_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let all = scope::ids(&self.clusters);
        let next = match self.rows.get(ix) {
            Some(Entry::AllClusters) => self.scope.toggle_all(&all),
            Some(Entry::Cluster(id)) => self.scope.toggle(id, &all),
            Some(Entry::AddCluster) => {
                self.emit_and_close(Some(SwitcherEvent::AddCluster), window, cx);
                return;
            }
            None => return,
        };
        self.highlighted = Some(ix);
        self.steered = true;
        if next != self.scope {
            self.scope = next.clone();
            cx.emit(SwitcherEvent::Select(next));
        }
        cx.notify();
    }

    fn activate_action(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let event = self
            .actions()
            .and_then(|(id, actions)| actions.get(ix).map(|action| action.event(&id)));
        self.emit_and_close(event, window, cx);
    }

    fn emit_and_close(
        &mut self,
        event: Option<SwitcherEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(event) = event else {
            return;
        };
        self.close(window, cx);
        cx.emit(event);
    }

    fn show_actions(&mut self, id: ClusterId, cx: &mut Context<Self>) {
        let entry = Entry::Cluster(id.clone());
        if let Some(row) = self.rows.iter().position(|row| *row == entry) {
            self.highlighted = Some(row);
        }
        self.actions_for = Some(id);
        self.action_highlighted = Some(0);
        cx.notify();
    }

    fn toggle_actions(&mut self, id: ClusterId, cx: &mut Context<Self>) {
        if self.actions_for.as_ref() == Some(&id) {
            self.close_actions();
            cx.notify();
        } else {
            self.show_actions(id, cx);
        }
    }

    fn close_actions(&mut self) {
        self.actions_for = None;
        self.action_highlighted = None;
    }

    fn show_actions_for_highlighted(&mut self, cx: &mut Context<Self>) {
        let entry = self.highlighted.and_then(|ix| self.rows.get(ix)).cloned();
        if let Some(Entry::Cluster(id)) = entry {
            self.show_actions(id, cx);
        }
    }

    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.actions_for.is_some() {
            self.close_actions();
            cx.notify();
        } else {
            self.close(window, cx);
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let plain = !keystroke.modifiers.modified();
        if keystroke.modifiers.platform && keystroke.key == ACTIONS_KEY {
            self.show_actions_for_highlighted(cx);
        } else if plain && keystroke.key == ENTER {
            if self.actions_for.is_some() {
                if let Some(ix) = self.action_highlighted {
                    self.activate_action(ix, window, cx);
                }
            } else if let Some(ix) = self.highlighted {
                self.activate(ix, window, cx);
            }
        } else if plain && keystroke.key == SPACE {
            if self.actions_for.is_none()
                && let Some(ix) = self.highlighted
            {
                self.toggle_row(ix, window, cx);
            }
        } else if plain && keystroke.key == ESCAPE {
            self.escape(window, cx);
        } else if let Some(nav) = Nav::parse(keystroke) {
            self.jump(nav, cx);
        } else if let Some(text) = typed(keystroke) {
            let text = text.to_owned();
            self.type_to(&text, cx);
        } else {
            return;
        }
        cx.stop_propagation();
    }

    fn render_trigger(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let (label, more) = self.scope.trigger(&self.clusters);
        let icon = if self.scope.cluster().is_some() {
            CLUSTER
        } else {
            CLUSTERS
        };
        let more = more.map(|more| {
            div()
                .debug_selector(|| MORE_ID.to_owned())
                .font_family(FONT_MONO)
                .text_size(TEXT_LABEL)
                .font_weight(FontWeight::NORMAL)
                .text_color(palette.mute)
                .child(more)
        });
        let hover = palette.hover;
        div()
            .debug_selector(|| ID.to_owned())
            .id(ID)
            .flex()
            .items_center()
            .gap(SPACE_4)
            .h_full()
            .pl(SPACE_6)
            .pr(SPACE_5)
            .border_l(RULE)
            .border_color(palette.line)
            .cursor_pointer()
            .font_family(FONT_SANS)
            .text_size(TEXT_BODY)
            .font_weight(FontWeight::MEDIUM)
            .text_color(palette.fg)
            .when(self.open, |trigger| trigger.bg(palette.sel))
            .when(!self.open, |trigger| {
                trigger.hover(move |style| style.bg(hover))
            })
            .tooltip(Tooltip::text(format!("{TOOLTIP}  {SWITCH_KEY}")))
            .on_click(cx.listener(|this, _, window, cx| {
                if this.dismissal.take_suppressed() {
                    return;
                }
                this.toggle(window, cx);
            }))
            .child(
                svg()
                    .path(icon)
                    .size(SWITCHER_ICON)
                    .text_color(palette.mute),
            )
            .child(label)
            .children(more)
            .child(
                svg()
                    .path(CHEVRON_UP)
                    .size(SWITCHER_CHEVRON)
                    .text_color(palette.mute),
            )
            .into_any_element()
    }

    fn render_rows(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let mut children: Vec<AnyElement> = Vec::new();
        for (ix, row) in self.rows.iter().enumerate() {
            if starts_group(&self.rows, ix) {
                children.push(rule(palette).into_any_element());
            }
            children.push(self.render_row(ix, row, palette, cx));
        }
        div()
            .id("switcher-list")
            .flex()
            .flex_col()
            .max_h(SWITCHER_LIST)
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(children)
            .into_any_element()
    }

    fn render_row(
        &self,
        ix: usize,
        row: &Entry,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let highlighted = self.highlighted == Some(ix);
        let base = div()
            .debug_selector(move || format!("switcher-row-{ix}"))
            .id(ElementId::NamedInteger("switcher-row".into(), ix as u64))
            .flex()
            .flex_none()
            .items_center()
            .gap(SPACE_4)
            .h(ROW_CLUSTER)
            .pl(SPACE_4)
            .rounded(RADIUS)
            .cursor_pointer()
            .text_size(TEXT_UI)
            .text_color(palette.fg)
            .when(highlighted, |row| row.bg(palette.hover))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.actions_for.is_none() {
                    this.highlighted = Some(ix);
                    this.steered = true;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, window, cx| this.toggle_row(ix, window, cx)));
        let label = |text: SharedString| div().flex_1().min_w_0().truncate().child(text);
        let element = match row {
            Entry::AllClusters => {
                let tally = self.scope.tally(&scope::ids(&self.clusters));
                base.pr(SPACE_4)
                    .child(check_box(self.scope.all_check(), palette))
                    .child(label(ALL_CLUSTERS.into()))
                    .child(meta(tally.into(), palette))
            }
            Entry::AddCluster => base
                .pr(SPACE_4)
                .text_color(palette.fg2)
                .child(add_glyph(palette))
                .child(label(ADD_CLUSTER.into())),
            Entry::Cluster(id) => match self.cluster(id) {
                Some(cluster) => {
                    let actions_id = id.clone();
                    let base = base.on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _, _, cx| {
                            this.show_actions(actions_id.clone(), cx);
                        }),
                    );
                    let shown = highlighted || self.actions_for.as_ref() == Some(id);
                    let gear = gear_button(ix, id.clone(), shown, palette, cx);
                    let details = switcher::meta(cluster);
                    base.pr(SPACE_1)
                        .when(!cluster.is_connected(), |row| row.text_color(palette.mute))
                        .child(check_box(self.scope.check(id), palette))
                        .child(label(cluster.name.clone().into()))
                        .child(meta(details.into(), palette))
                        .child(gear)
                }
                None => base,
            },
        };
        element.into_any_element()
    }

    fn render_menu(
        &self,
        id: &ClusterId,
        actions: [Action; 5],
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name: SharedString = self
            .cluster(id)
            .map_or_else(|| id.to_string(), |cluster| cluster.name.clone())
            .into();
        let header = div()
            .pt(SPACE_3)
            .pb(SPACE_2)
            .px(SPACE_4)
            .font_family(FONT_MONO)
            .text_size(TEXT_LABEL)
            .text_color(palette.mute)
            .truncate()
            .child(name);
        let items: Vec<AnyElement> = actions
            .into_iter()
            .enumerate()
            .map(|(ix, action)| self.render_action(ix, action, palette, cx))
            .collect();
        surface(MENU_ID, palette)
            .debug_selector(|| MENU_ID.to_owned())
            .absolute()
            .bottom_0()
            .right(SWITCHER_WIDTH + SPACE_2)
            .w(SWITCHER_MENU)
            .capture_any_mouse_down(cx.listener(|this, _, _, _| this.menu_pressed = true))
            .child(header)
            .children(items)
            .into_any_element()
    }

    fn render_action(
        &self,
        ix: usize,
        action: Action,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let highlighted = self.action_highlighted == Some(ix);
        let color = if action == Action::Remove {
            palette.err
        } else {
            palette.fg
        };
        div()
            .debug_selector(move || format!("switcher-action-{ix}"))
            .id(ElementId::NamedInteger("switcher-action".into(), ix as u64))
            .flex()
            .items_center()
            .h(ROW_NAV)
            .px(SPACE_4)
            .rounded(RADIUS)
            .cursor_pointer()
            .text_size(TEXT_UI)
            .text_color(color)
            .when(highlighted, |row| row.bg(palette.hover))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.action_highlighted = Some(ix);
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.activate_action(ix, window, cx);
            }))
            .child(action.label())
            .into_any_element()
    }
}

fn check_box(check: Check, palette: &Palette) -> impl IntoElement {
    let mark = match check {
        Check::On => Some(CHECK),
        Check::Mixed => Some(DASH),
        Check::Off => None,
    };
    let base = div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(SWITCHER_CHECK)
        .rounded(RADIUS)
        .border(RULE);
    match mark {
        Some(path) => base.bg(palette.acc).border_color(palette.acc).child(
            svg()
                .path(path)
                .size(SWITCHER_TICK)
                .text_color(palette.onacc),
        ),
        None => base.border_color(palette.line2),
    }
}

fn add_glyph(palette: &Palette) -> impl IntoElement {
    div()
        .w(SWITCHER_CHECK)
        .flex_none()
        .flex()
        .justify_center()
        .text_color(palette.mute)
        .child(ADD_GLYPH)
}

fn meta(text: SharedString, palette: &Palette) -> impl IntoElement {
    div()
        .flex_none()
        .font_family(FONT_MONO)
        .text_size(TEXT_LABEL)
        .text_color(palette.mute)
        .child(text)
}

fn rule(palette: &Palette) -> impl IntoElement {
    div()
        .flex_none()
        .h(RULE)
        .mx(SPACE_1)
        .my(SPACE_2)
        .bg(palette.line)
}

fn gear_button(
    ix: usize,
    id: ClusterId,
    shown: bool,
    palette: &Palette,
    cx: &mut Context<ClusterSwitcher>,
) -> AnyElement {
    let hover = palette.line;
    div()
        .debug_selector(move || format!("switcher-gear-{ix}"))
        .id(ElementId::NamedInteger("switcher-gear".into(), ix as u64))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(SWITCHER_GEAR)
        .h(crate::theme::ICON_BUTTON)
        .rounded(RADIUS)
        .when(!shown, Styled::invisible)
        .hover(move |style| style.bg(hover))
        .tooltip(Tooltip::text(GEAR_TOOLTIP))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            this.toggle_actions(id.clone(), cx);
        }))
        .child(svg().path(GEAR).size(ICON).text_color(palette.mute))
        .into_any_element()
}

impl Render for ClusterSwitcher {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let trigger = self.render_trigger(&palette, cx);
        let popover = self.open.then(|| {
            let body = self.render_rows(&palette, cx);
            let menu = self
                .actions()
                .map(|(id, actions)| self.render_menu(&id, actions, &palette, cx));
            let panel = surface("switcher", &palette)
                .w(SWITCHER_WIDTH)
                .overflow_hidden()
                .track_focus(&self.focus)
                .on_key_down(cx.listener(Self::key_down))
                .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                    if std::mem::take(&mut this.menu_pressed) {
                        return;
                    }
                    this.dismissal.clicked_outside();
                    this.close(window, cx);
                }))
                .child(body);
            let anchor = div()
                .relative()
                .font_family(FONT_SANS)
                .children(menu)
                .child(panel);
            let key = ElementId::NamedInteger(ID.into(), self.generation);
            above_right(appear(anchor, key))
        });
        div()
            .id(ElementId::NamedInteger(ID.into(), 0))
            .relative()
            .flex_none()
            .h_full()
            .on_hover(cx.listener(|this, hovered: &bool, _, _| {
                this.dismissal.hover(*hovered);
            }))
            .child(trigger)
            .children(popover)
    }
}

impl Shell {
    pub(super) fn switcher_event(
        &mut self,
        event: &SwitcherEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SwitcherEvent::Select(scope) => self.retarget(scope, cx),
            SwitcherEvent::AddCluster => {
                self.open_add_cluster(flock_core::AddClusterStep::Pick, window, cx);
            }
            SwitcherEvent::Run(command) => self.dispatch(command.clone(), cx),
            SwitcherEvent::Remove(id) => Self::confirm_remove(id.clone(), window, cx),
            SwitcherEvent::Rename(id) => self.open_rename(id.clone(), window, cx),
            SwitcherEvent::SwitchContext(id) => self.open_switch_context(id.clone(), window, cx),
        }
    }

    fn confirm_remove(id: ClusterId, window: &mut Window, cx: &mut Context<Self>) {
        let shell = cx.entity().downgrade();
        let run: Run = Rc::new(move |command, cx| {
            shell
                .update(cx, |shell, cx| shell.dispatch(command, cx))
                .ok();
        });
        confirm(remove_dialog(id, run), window, cx);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use flock_core::{Counts, Status};

    use super::*;

    fn cluster(id: &str, status: Status) -> ClusterSummary {
        ClusterSummary {
            id: ClusterId::new(id),
            name: id.to_owned(),
            status,
            state: String::new(),
            distro: String::new(),
            region: String::new(),
            version: None,
            nodes: None,
            problems: Counts::default(),
            nav: BTreeMap::new(),
            metrics: None,
        }
    }

    #[test]
    fn connected_clusters_offer_disconnect_and_offline_ones_connect() {
        let live = Action::for_cluster(&cluster("a", Status::Warn));
        let offline = Action::for_cluster(&cluster("b", Status::Offline));
        let expected = |connection| {
            [
                Action::Settings,
                Action::Rename,
                Action::SwitchContext,
                connection,
                Action::Remove,
            ]
        };
        assert_eq!(live, expected(Action::Disconnect));
        assert_eq!(offline, expected(Action::Connect));
    }

    #[test]
    fn actions_become_shell_events() {
        let id = ClusterId::new("a");
        assert_eq!(
            Action::Settings.event(&id),
            SwitcherEvent::Run(Command::ShowClusterSettings(id.clone()))
        );
        assert_eq!(
            Action::Disconnect.event(&id),
            SwitcherEvent::Run(Command::Disconnect(id.clone()))
        );
        assert_eq!(
            Action::Connect.event(&id),
            SwitcherEvent::Run(Command::Connect(id.clone()))
        );
        assert_eq!(Action::Rename.event(&id), SwitcherEvent::Rename(id.clone()));
        assert_eq!(
            Action::SwitchContext.event(&id),
            SwitcherEvent::SwitchContext(id.clone())
        );
        assert_eq!(Action::Remove.event(&id), SwitcherEvent::Remove(id));
    }

    #[test]
    fn rules_separate_all_clusters_the_clusters_and_add_cluster() {
        let clusters = [cluster("a", Status::Ok), cluster("b", Status::Ok)];
        let rows = switcher::rows(&clusters);
        let starts: Vec<bool> = (0..rows.len()).map(|ix| starts_group(&rows, ix)).collect();
        assert_eq!(starts, [false, true, false, true]);
        assert_eq!(child_index(&rows, 0), 0);
        assert_eq!(child_index(&rows, 1), 2);
        assert_eq!(child_index(&rows, 3), 5);
    }
}
