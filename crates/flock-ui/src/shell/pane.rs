use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use flock_core::tabs::{Divider, PaneId, TabId};
use flock_core::{Command, Sender, Snapshot};
use gpui::{AppContext as _, Context, Entity, EventEmitter, ScrollHandle, Subscription, Window};

use super::Shell;
use super::load::LoadClock;
use crate::screens::app_settings::AppSettingsScreen;
use crate::screens::changes::ChangesScreen;
use crate::screens::compare::CompareScreen;
use crate::screens::detail::DetailScreen;
use crate::screens::edit::EditScreen;
use crate::screens::flock::FlockScreen;
use crate::screens::flux_object::FluxObjectPage;
use crate::screens::gitops::GitOpsScreen;
use crate::screens::logs::LogsScreen;
use crate::screens::multilog::MultiLogsScreen;
use crate::screens::pods::PodsScreen;
use crate::screens::settings::SettingsScreen;
use crate::screens::workload::WorkloadScreen;
use crate::screens::{ClusterPages, FullView, ScreenEvent};

const FIRST_SCOPE: u64 = 1;

static NEXT_SCOPE: AtomicU64 = AtomicU64::new(FIRST_SCOPE);

pub(super) struct Scope {
    id: u64,
    commands: Sender<Command>,
    closed: bool,
}

impl Scope {
    fn new(commands: Sender<Command>) -> Self {
        Self {
            id: NEXT_SCOPE.fetch_add(1, Ordering::Relaxed),
            commands,
            closed: false,
        }
    }

    pub(super) fn close(&mut self) {
        if !std::mem::replace(&mut self.closed, true) {
            let _ = self.commands.try_send(Command::WindowClosed(self.id));
        }
    }

    pub(super) fn send(&self, command: Command) {
        let scoped = Command::ForWindow(self.id, Box::new(command));
        if let Err(error) = self.commands.try_send(scoped) {
            tracing::warn!(%error, "backend stopped accepting commands");
        }
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        self.close();
    }
}

pub(super) struct PaneScreens {
    pub(super) pane: PaneId,
    pub(super) navigation: Entity<crate::kit::HeaderNavigation>,
    pub(super) scope: Scope,
    pub(super) flock: Entity<FlockScreen>,
    pub(super) changes: Entity<ChangesScreen>,
    pub(super) gitops: Entity<GitOpsScreen>,
    pub(super) flux_events: Entity<crate::screens::FluxEventsScreen>,
    pub(super) pages: ClusterPages,
    pub(super) flux_object: Entity<FluxObjectPage>,
    pub(super) helm_release: Entity<crate::screens::helm::HelmReleaseScreen>,
    #[cfg(feature = "loco")]
    pub(super) loco: Entity<crate::screens::loco::LocoScreens>,
    pub(super) compare: Entity<CompareScreen>,
    pub(super) settings: Entity<SettingsScreen>,
    pub(super) app_settings: Entity<AppSettingsScreen>,
    pub(super) pods: Entity<PodsScreen>,
    pub(super) workload: Option<(Entity<WorkloadScreen>, Subscription)>,
    pub(super) detail: Option<(Entity<DetailScreen>, Subscription)>,
    pub(super) logs: Option<(Entity<LogsScreen>, Subscription)>,
    pub(super) multi_logs: Option<(Entity<MultiLogsScreen>, Subscription)>,
    pub(super) edits: Vec<(TabId, Entity<EditScreen>, Subscription)>,
    pub(super) load: LoadClock,
    pub(super) strip: ScrollHandle,
    pub(super) shown: Option<TabId>,
    pub(super) motion: super::tab_motion::StripMotion,
    _subscriptions: Vec<Subscription>,
}

impl PaneScreens {
    pub(super) fn new(
        pane: PaneId,
        commands: Sender<Command>,
        window: &mut Window,
        cx: &mut Context<Shell>,
    ) -> Self {
        let flock = cx.new(FlockScreen::new);
        let changes = cx.new(|cx| ChangesScreen::new(window, cx));
        let gitops = cx.new(|cx| GitOpsScreen::new(window, cx));
        let flux_events = cx.new(|cx| crate::screens::FluxEventsScreen::new(window, cx));
        #[cfg(feature = "loco")]
        let loco = cx.new(|cx| crate::screens::loco::LocoScreens::new(window, cx));
        let compare = cx.new(CompareScreen::new);
        let settings = cx.new(|cx| SettingsScreen::new(window, cx));
        let app_settings = cx.new(|cx| AppSettingsScreen::new(window, cx));
        let pods = cx.new(|cx| PodsScreen::new(window, cx));
        let flux_object = cx.new(FluxObjectPage::new);
        let helm_release = cx.new(crate::screens::helm::HelmReleaseScreen::new);
        let navigation = cx.new(crate::kit::HeaderNavigation::new);
        changes.update(cx, |screen, _| screen.navigation = Some(navigation.clone()));
        compare.update(cx, |screen, _| screen.navigation = Some(navigation.clone()));
        settings.update(cx, |screen, _| screen.navigation = Some(navigation.clone()));
        app_settings.update(cx, |screen, _| screen.navigation = Some(navigation.clone()));
        flux_object.update(cx, |screen, _| screen.navigation = Some(navigation.clone()));
        helm_release.update(cx, |screen, _| screen.navigation = Some(navigation.clone()));
        #[cfg(feature = "loco")]
        loco.update(cx, |screen, cx| {
            screen.set_navigation(navigation.clone(), cx);
        });
        let pages = ClusterPages::new(window, cx, move |shell: &mut Shell, event, cx| {
            shell.pane_screen_event(pane, event, cx);
        });
        #[cfg_attr(not(feature = "loco"), allow(unused_mut))]
        let mut subscriptions = vec![
            cx.subscribe(
                &navigation,
                move |shell, _, jump: &crate::kit::HistoryJump, cx| {
                    shell.focus_pane_id(pane, cx);
                    if shell.state.panes().active().id() == jump.tab
                        && shell.state.jump_history(jump.direction, jump.steps)
                    {
                        shell.show_active(cx);
                    }
                },
            ),
            forward(pane, &flock, cx),
            forward(pane, &gitops, cx),
            forward(pane, &flux_events, cx),
            forward(pane, &compare, cx),
            forward(pane, &settings, cx),
            forward(pane, &app_settings, cx),
            forward(pane, &pods, cx),
            forward(pane, &flux_object, cx),
            forward(pane, &helm_release, cx),
        ];
        #[cfg(feature = "loco")]
        subscriptions.push(forward(pane, &loco, cx));
        Self {
            pane,
            navigation,
            scope: Scope::new(commands),
            flock,
            changes,
            gitops,
            flux_events,
            pages,
            flux_object,
            helm_release,
            #[cfg(feature = "loco")]
            loco,
            compare,
            settings,
            app_settings,
            pods,
            workload: None,
            detail: None,
            logs: None,
            multi_logs: None,
            edits: Vec::new(),
            load: LoadClock::default(),
            strip: ScrollHandle::default(),
            shown: None,
            motion: super::tab_motion::StripMotion::default(),
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn set_snapshot(&self, snapshot: &Snapshot, cx: &mut Context<Shell>) {
        self.flock
            .update(cx, |flock, cx| flock.set_snapshot(snapshot, cx));
        self.changes
            .update(cx, |changes, cx| changes.set_snapshot(snapshot, cx));
        self.gitops
            .update(cx, |gitops, cx| gitops.set_snapshot(snapshot, cx));
        self.flux_events
            .update(cx, |screen, cx| screen.set_snapshot(snapshot, cx));
        self.settings
            .update(cx, |settings, cx| settings.set_snapshot(snapshot, cx));
        self.pods
            .update(cx, |pods, cx| pods.set_snapshot(snapshot, cx));
    }
}

fn forward<T: EventEmitter<Command>>(
    pane: PaneId,
    screen: &Entity<T>,
    cx: &mut Context<Shell>,
) -> Subscription {
    cx.subscribe(screen, move |shell, _, command: &Command, cx| {
        shell.pane_command(pane, command.clone(), cx);
    })
}

impl Shell {
    pub(super) fn pane_command(&mut self, pane: PaneId, command: Command, cx: &mut Context<Self>) {
        if self.state.window_commands(&command).is_some() {
            self.focus_pane_id(pane, cx);
            self.dispatch(command, cx);
            return;
        }
        let Some(ix) = self.panes.iter().position(|screens| screens.pane == pane) else {
            return;
        };
        self.ui_effects(&command, cx);
        self.screens(ix).scope.send(command);
        cx.notify();
    }

    pub(super) fn pane_screen_event(
        &mut self,
        pane: PaneId,
        event: &ScreenEvent,
        cx: &mut Context<Self>,
    ) {
        self.focus_pane_id(pane, cx);
        self.screen_event(event, cx);
    }

    fn focus_pane_id(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        let ix = self.panes.iter().position(|screens| screens.pane == pane);
        if let Some(ix) = ix {
            self.focus_pane(ix, cx);
        }
    }

    pub(super) fn focus_pane(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.state.panes_mut().focus(ix) {
            self.sync_palette(cx);
            self.save_tabs(cx);
            cx.notify();
        }
    }

    pub(super) fn sync_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<usize> {
        let ids: Vec<PaneId> = self
            .state
            .panes()
            .panes()
            .iter()
            .map(flock_core::tabs::Pane::id)
            .collect();
        if self
            .panes
            .iter()
            .map(|screens| screens.pane)
            .eq(ids.iter().copied())
        {
            return Vec::new();
        }
        let mut previous = std::mem::take(&mut self.panes);
        let mut created = Vec::new();
        for (ix, id) in ids.into_iter().enumerate() {
            if let Some(at) = previous.iter().position(|screens| screens.pane == id) {
                self.panes.push(previous.remove(at));
                continue;
            }
            created.push(ix);
            let screens = PaneScreens::new(id, self.commands.clone(), window, cx);
            screens.set_snapshot(&self.state.snapshot, cx);
            self.panes.push(screens);
        }
        created
    }

    pub(super) fn panes_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let created = self.sync_panes(window, cx);
        self.show_panes(&created, cx);
    }

    pub(super) fn split_right(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let page = self.state.location().page;
        let inspecting = self.focused_screens().pages.inspecting(page, cx);
        self.state.panes_mut().split();
        let created = self.sync_panes(window, cx);
        let revealed = if inspecting { created } else { Vec::new() };
        self.show_panes(&revealed, cx);
    }

    pub(super) fn resize_panes(&mut self, share: f32, cx: &mut Context<Self>) {
        if self.state.panes_mut().set_divider(Divider::at(share)) {
            cx.notify();
        }
    }

    pub(super) fn finish_resize_panes(&mut self, cx: &mut Context<Self>) {
        self.save_tabs(cx);
    }

    pub(super) fn full_view(&self, ix: usize, cx: &Context<Self>) -> Option<FullView> {
        if self.panes.len() < 2 {
            return None;
        }
        let pane = self.screens(ix).pane;
        let shell = cx.weak_entity();
        let open: FullView = Rc::new(move |window, cx| {
            shell
                .update(cx, |shell, cx| shell.open_full_view(pane, window, cx))
                .ok();
        });
        Some(open)
    }

    fn open_full_view(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self
            .state
            .panes()
            .panes()
            .iter()
            .find(|candidate| candidate.id() == pane)
            .map(|candidate| candidate.active().id());
        if let Some(tab) = tab
            && self.state.panes_mut().join(tab)
        {
            self.panes_changed(window, cx);
        }
    }

    pub(super) fn screens(&self, ix: usize) -> &PaneScreens {
        match self.panes.get(ix) {
            Some(screens) => screens,
            None => unreachable!("every pane has its screens"),
        }
    }

    pub(super) fn screens_mut(&mut self, ix: usize) -> &mut PaneScreens {
        match self.panes.get_mut(ix) {
            Some(screens) => screens,
            None => unreachable!("every pane has its screens"),
        }
    }

    pub(super) fn focused_screens(&self) -> &PaneScreens {
        self.screens(self.state.panes().focused_index())
    }

    pub(super) fn location_at(&self, ix: usize) -> &flock_core::history::Location {
        match self.state.panes().panes().get(ix) {
            Some(pane) => pane.active().location(),
            None => unreachable!("every pane has its screens"),
        }
    }
}
