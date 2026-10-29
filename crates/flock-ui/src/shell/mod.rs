mod keys;
mod load;
mod page;
mod pane;
mod sidebar;
mod status_bar;
pub(crate) mod switcher;
mod tab_motion;
mod tabs;

use flock_core::history::Location;
use flock_core::palette::{COMPARE_KEY, LOGS_KEY, OWNER_KEY, resource_commands};
use flock_core::resource::short_kind;
use flock_core::scope::Scope;
use flock_core::settings::NamespaceSelection;
use flock_core::{
    AddClusterStep, AppState, ClusterId, ClusterSummary, Command, Page, Preferences, Sender,
    Status, Update,
};
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, ParentElement, Render, Styled,
    Subscription, Task, Window, div,
};

use crate::Store;
use crate::actions::{
    CompareAcrossClusters, FocusFilter, FollowLogs, OpenSettings, RefreshView, ShowAllClusters,
    ShowFluxOwner, ToggleClusterSwitcher, ToggleCommandPalette, ToggleSidebar,
};
use crate::kit::{FilterBar, LogView, Split, Toast, toast};
use crate::screens::ScreenEvent;
use crate::screens::add_cluster::{AddClusterDialog, AddClusterEvent};
use crate::screens::cluster_edit::{ClusterEditEvent, RenameDialog, SwitchContextDialog};
use crate::screens::command::{CommandPalette, PaletteEvent};
use crate::screens::workload::WorkloadScreen;
use crate::theme::{self, ActiveTheme, TEXT_UI};
use pane::PaneScreens;
use switcher::{ClusterSwitcher, SwitcherEvent};

pub(crate) const KEY_CONTEXT: &str = "Shell";
const PANES_SPLIT: &str = "panes";

pub struct NewWindowRequest(pub Vec<Command>);

impl gpui::EventEmitter<NewWindowRequest> for Shell {}

pub struct Shell {
    state: AppState,
    keys: keys::Keyboard,
    window_id: u64,
    commands: Sender<Command>,
    pending: Vec<Command>,
    new_tab_gesture: bool,
    announce: bool,
    saved: Option<flock_core::tabs::SavedWindow>,
    sidebar_generation: usize,
    sidebar_drag: Option<sidebar::SidebarDrag>,
    focus: FocusHandle,
    palette: Entity<CommandPalette>,
    panes: Vec<PaneScreens>,
    loaded: bool,
    add_cluster: Option<(Entity<AddClusterDialog>, Subscription)>,
    rename: Option<(Entity<RenameDialog>, Subscription)>,
    switch_context: Option<(Entity<SwitchContextDialog>, Subscription)>,
    switcher: Entity<ClusterSwitcher>,
    tab_menu: Entity<crate::kit::ActionMenu>,
    _switcher_events: Subscription,
    _updates: Task<()>,
    _store: Entity<Store>,
    _window_closed: Subscription,
    _focus_recovery: Subscription,
    _appearance: Subscription,
    _palette_events: Subscription,
    _updater: Subscription,
}

impl Shell {
    pub fn new(
        store: Entity<Store>,
        initial: Vec<Command>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let commands = store.read(cx).commands();
        let updates = store.update(cx, |store, _| store.subscribe());
        let window_id = cx.entity_id().as_u64();
        let window_closed = Self::watch_close(window, window_id, cx);
        let listen = cx.spawn_in(window, async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                let applied = this.update_in(cx, |shell, window, cx| {
                    shell.receive(update, window, cx);
                });
                if applied.is_err() {
                    break;
                }
            }
        });
        let appearance = cx.observe_window_appearance(window, |_, window, cx| {
            theme::follow_window(window, cx);
            cx.notify();
        });
        let updater_changes = cx.observe_global::<crate::updater::Updater>(|_, cx| cx.notify());
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let focus_recovery = cx.on_focus_lost(window, |this, window, cx| {
            window.focus(&this.focus, cx);
        });
        theme::follow_window(window, cx);
        let (palette, palette_events) = Self::new_palette(window, cx);
        let (switcher, switcher_events) = Self::new_switcher(window, cx);
        let tab_menu = cx.new(|cx| crate::kit::ActionMenu::context(tabs::TAB_MENU, cx));
        let mut state = AppState::default();
        state.sidebar_open = !crate::settings::current(cx).sidebar_closed;
        let first = PaneScreens::new(state.panes().focused().id(), commands.clone(), window, cx);
        Self {
            state,
            keys: keys::Keyboard::new(cx),
            window_id,
            commands,
            pending: initial,
            new_tab_gesture: false,
            announce: false,
            saved: None,
            sidebar_generation: 0,
            sidebar_drag: None,
            focus,
            palette,
            panes: vec![first],
            loaded: false,
            add_cluster: None,
            rename: None,
            switch_context: None,
            switcher,
            tab_menu,
            _switcher_events: switcher_events,
            _updates: listen,
            _store: store,
            _window_closed: window_closed,
            _focus_recovery: focus_recovery,
            _appearance: appearance,
            _palette_events: palette_events,
            _updater: updater_changes,
        }
    }

    fn new_palette(
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<CommandPalette>, Subscription) {
        let palette = cx.new(|cx| CommandPalette::new(window, cx));
        let events = cx.subscribe_in(&palette, window, |this, palette, event, window, cx| {
            this.palette_event(palette, event, window, cx);
        });
        (palette, events)
    }

    fn new_switcher(
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<ClusterSwitcher>, Subscription) {
        let switcher = cx.new(ClusterSwitcher::new);
        let events = cx.subscribe_in(
            &switcher,
            window,
            |this, _, event: &SwitcherEvent, window, cx| this.switcher_event(event, window, cx),
        );
        (switcher, events)
    }

    fn watch_close(window: &Window, window_id: u64, cx: &mut Context<Self>) -> Subscription {
        let handle = window.window_handle();
        let shell = cx.entity().downgrade();
        cx.on_window_closed(move |cx, closed| {
            if closed != handle.window_id() {
                return;
            }
            if let Some(shell) = shell.upgrade() {
                shell.update(cx, |shell, _| {
                    for screens in &mut shell.panes {
                        screens.scope.close();
                    }
                });
            }
            crate::session::closed(window_id, cx);
        })
    }

    fn palette_event(
        &mut self,
        palette: &Entity<CommandPalette>,
        event: &PaletteEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PaletteEvent::Run(command) => {
                palette.update(cx, |palette, cx| palette.close(window, cx));
                self.dispatch(command.clone(), cx);
            }
            PaletteEvent::NewTab(command) => {
                if self.state.window_commands(command).is_some() {
                    palette.update(cx, |palette, cx| palette.close(window, cx));
                    self.dispatch(Command::NewTab(Box::new(command.clone())), cx);
                }
            }
            PaletteEvent::NewWindow(command) => {
                if let Some(commands) = self.state.window_commands(command) {
                    palette.update(cx, |palette, cx| palette.close(window, cx));
                    cx.emit(NewWindowRequest(commands));
                }
            }
            PaletteEvent::Split(command) => {
                if self.state.open_split(command) {
                    palette.update(cx, |palette, cx| palette.close(window, cx));
                    self.panes_changed(window, cx);
                }
            }
        }
    }

    fn receive(&mut self, update: Update, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((dialog, _)) = &self.add_cluster {
            dialog.update(cx, |dialog, cx| dialog.receive(&update, window, cx));
        }
        if let Some((dialog, _)) = &self.switch_context {
            dialog.update(cx, |dialog, cx| dialog.receive(&update, cx));
        }
        if let Update::Snapshot(_) = update {
            self.loaded = true;
        }
        self.state.update(update);
        self.run_pending(window, cx);
        if self.loaded && std::mem::take(&mut self.announce) {
            self.show_all(cx);
        }
        if self.loaded {
            self.save_tabs(cx);
        }
        for screens in &self.panes {
            screens.set_snapshot(&self.state.snapshot, cx);
        }
        self.sync_page(cx);
        self.track_load(cx);
        self.sync_palette(cx);
        cx.notify();
    }

    fn run_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ready = self
            .pending
            .iter()
            .take_while(|command| self.state.ready_for(command))
            .count();
        let pending: Vec<Command> = self.pending.drain(..ready).collect();
        for command in pending {
            if let Command::OpenAddCluster(step) = command {
                self.open_add_cluster(step, window, cx);
            } else {
                self.dispatch(command, cx);
            }
        }
    }

    pub fn restore_tabs(
        &mut self,
        saved: &flock_core::tabs::SavedWindow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.restore(saved);
        self.sync_panes(window, cx);
        if self.loaded {
            self.show_all(cx);
        } else {
            self.announce = true;
        }
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    pub fn settled(&self) -> bool {
        self.loaded && self.pending.is_empty()
    }

    pub fn open_palette(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_palette(cx);
        self.palette
            .update(cx, |palette, cx| palette.open(query, window, cx));
    }

    fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.read(cx).is_open() {
            self.palette
                .update(cx, |palette, cx| palette.close(window, cx));
        } else {
            self.open_palette("", window, cx);
        }
    }

    fn sync_palette(&self, cx: &mut Context<Self>) {
        let snapshot = &self.state.snapshot;
        let current = self.state.location().cluster.as_ref();
        self.palette.update(cx, |palette, cx| {
            palette.set_context(snapshot, current, cx);
        });
        let scope = self.state.location().scope();
        self.switcher.update(cx, |switcher, cx| {
            switcher.set_context(&snapshot.clusters, scope, cx);
        });
    }

    pub fn palette_open(&self, cx: &App) -> bool {
        self.palette.read(cx).is_open()
    }

    pub fn pod_logs(&self, cx: &App) -> Option<(Vec<String>, String)> {
        let (screen, _) = self.focused_screens().multi_logs.as_ref()?;
        let view = screen.read(cx).view().read(cx);
        let legend = view
            .legend()
            .iter()
            .filter(|entry| !entry.hidden)
            .map(|entry| entry.label.to_string())
            .collect();
        let text = view.logs().read(cx).export();
        Some((legend, text))
    }

    pub fn log_view(&self, cx: &App) -> Option<Entity<LogView>> {
        let screens = self.focused_screens();
        if let Some((screen, _)) = &screens.logs {
            return Some(screen.read(cx).logs().clone());
        }
        let (screen, _) = screens.multi_logs.as_ref()?;
        Some(screen.read(cx).view().read(cx).logs().clone())
    }

    pub fn custom_empty_sentence(&self, cx: &App) -> Option<String> {
        self.focused_screens().pages.custom_empty_sentence(cx)
    }

    pub fn switcher_open(&self, cx: &App) -> bool {
        self.switcher.read(cx).is_open()
    }

    pub fn problem_clusters(&self, cx: &App) -> Vec<ClusterId> {
        self.focused_screens().problems.read(cx).problem_clusters()
    }

    pub fn switcher_label(&self, cx: &App) -> String {
        self.switcher.read(cx).label()
    }

    pub fn cluster_settings_selection(&self, cx: &App) -> Option<ClusterId> {
        self.focused_screens().settings.read(cx).selected().cloned()
    }

    pub fn changes_cluster(&self, cx: &App) -> Option<ClusterId> {
        self.focused_screens().changes.read(cx).cluster().cloned()
    }

    pub fn open_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.switcher
            .update(cx, |switcher, cx| switcher.show(window, cx));
    }

    fn cluster_of(&self, location: &Location) -> Option<&ClusterSummary> {
        let id = location.cluster.as_ref()?;
        self.state.snapshot.cluster(id)
    }

    fn preferences_of(&self, location: &Location) -> Preferences {
        self.cluster_of(location)
            .and_then(|cluster| self.state.snapshot.catalog.get(&cluster.id))
            .map(|settings| settings.preferences.clone())
            .unwrap_or_default()
    }

    fn screen_event(&mut self, event: &ScreenEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            ScreenEvent::Command(command) => self.dispatch(command.clone(), cx),
            ScreenEvent::Navigate(page) => self.dispatch(Command::SelectPage(*page), cx),
            ScreenEvent::ShowEvents(filter) => {
                self.dispatch(Command::SelectPage(Page::Events), cx);
                self.focused_screens().pages.show_event_filter(*filter, cx);
            }
            ScreenEvent::ShowEvent(key) => {
                self.dispatch(Command::SelectPage(Page::Events), cx);
                self.focused_screens().pages.show_event(key.clone(), cx);
            }
            ScreenEvent::NeedsAttention { page, namespace } => {
                self.show_attention(*page, namespace.as_deref(), cx);
            }
            ScreenEvent::Filter { page, tokens } => {
                self.open_list(*page, None, cx);
                if let Some(bar) = self.filter_bar(cx) {
                    bar.update(cx, |bar, cx| bar.preset(tokens.clone(), window, cx));
                }
            }
        }
    }

    fn show_attention(&mut self, page: Page, namespace: Option<&str>, cx: &mut Context<Self>) {
        self.open_list(page, namespace, cx);
        let chip = flock_core::resources::attention_chip(page);
        if let Some(bar) = self.filter_bar(cx) {
            bar.update(cx, |bar, cx| bar.select_chip(chip, cx));
        }
    }

    fn open_list(&mut self, page: Page, namespace: Option<&str>, cx: &mut Context<Self>) {
        let location = self.state.location().clone();
        let list = flock_core::resources::namespace_list(page);
        if let (Some(list), Some(cluster)) = (list, location.cluster.clone()) {
            let mut preferences = self.preferences_of(&location);
            preferences.select_namespace(list, NamespaceSelection::of(namespace));
            self.dispatch(Command::SetPreferences(cluster, preferences), cx);
        }
        self.dispatch(Command::SelectPage(page), cx);
    }

    fn send(&self, command: Command) {
        self.focused_screens().scope.send(command);
    }

    pub(super) fn open_add_cluster(
        &mut self,
        step: AddClusterStep,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = self.state.snapshot.clusters.iter().map(|c| c.name.clone());
        let existing: Vec<String> = existing.collect();
        let dialog = cx.new(|cx| AddClusterDialog::new(step, existing, window, cx));
        let subscription = cx.subscribe_in(
            &dialog,
            window,
            |this, _, event: &AddClusterEvent, window, cx| match event {
                AddClusterEvent::Send(command) => this.send(command.clone()),
                AddClusterEvent::Close => {
                    this.add_cluster = None;
                    window.focus(&this.focus, cx);
                    cx.notify();
                }
            },
        );
        self.add_cluster = Some((dialog, subscription));
        self.send(Command::OpenAddCluster(step));
        cx.notify();
    }

    pub(super) fn open_rename(
        &mut self,
        id: ClusterId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(cluster) = self.state.snapshot.cluster(&id) else {
            return;
        };
        let current = cluster.name.clone();
        let dialog = cx.new(|cx| RenameDialog::new(id, &current, window, cx));
        let subscription = cx.subscribe_in(
            &dialog,
            window,
            Self::cluster_edit_event(|this| {
                this.rename = None;
            }),
        );
        self.rename = Some((dialog, subscription));
        cx.notify();
    }

    pub(super) fn open_switch_context(
        &mut self,
        id: ClusterId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dialog = cx.new(|cx| SwitchContextDialog::new(id, window, cx));
        let subscription = cx.subscribe_in(
            &dialog,
            window,
            Self::cluster_edit_event(|this| {
                this.switch_context = None;
            }),
        );
        self.switch_context = Some((dialog, subscription));
        self.send(Command::OpenAddCluster(AddClusterStep::Pick));
        cx.notify();
    }

    fn cluster_edit_event<T>(
        clear: fn(&mut Self),
    ) -> impl Fn(&mut Self, &Entity<T>, &ClusterEditEvent, &mut Window, &mut Context<Self>) {
        move |this, _, event, window, cx| match event {
            ClusterEditEvent::Send(command) => this.dispatch(command.clone(), cx),
            ClusterEditEvent::Close => {
                clear(this);
                window.focus(&this.focus, cx);
                cx.notify();
            }
        }
    }

    pub fn dispatch(&mut self, command: Command, cx: &mut Context<Self>) {
        let command = self.with_gesture(command);
        let before = self.state.location().clone();
        if let Command::NewTab(inner) = &command {
            self.ui_effects(inner, cx);
            self.state.apply(&command);
            self.show_active(cx);
            return;
        }
        self.ui_effects(&command, cx);
        self.state.apply(&command);
        if let Some(target) = revealed(&command) {
            self.focused_screens().pages.reveal(target, cx);
        }
        let moved = Location {
            inspecting: before.inspecting.clone(),
            ..self.state.location().clone()
        } != before;
        self.keys.refocus |= moved;
        self.sync_page(cx);
        self.track_load(cx);
        self.send(command);
        self.sync_palette(cx);
        self.save_tabs(cx);
        cx.notify();
    }

    fn with_gesture(&mut self, command: Command) -> Command {
        let navigates = self.state.window_commands(&command).is_some();
        let fresh = !matches!(command, Command::NewTab(_));
        if navigates && fresh && std::mem::take(&mut self.new_tab_gesture) {
            return Command::NewTab(Box::new(command));
        }
        command
    }

    fn announce_pane(&self, ix: usize, reveal: bool, cx: &mut Context<Self>) {
        let location = self.location_at(ix);
        let screens = self.screens(ix);
        for command in location.commands() {
            screens.scope.send(command);
        }
        let shown = location.inspecting.as_ref().or(location.resource.as_ref());
        if let Some(resource) = shown.filter(|_| reveal) {
            screens.pages.reveal(resource, cx);
        }
    }

    pub(super) fn show_active(&mut self, cx: &mut Context<Self>) {
        self.announce_pane(self.state.panes().focused_index(), true, cx);
        self.shown(cx);
    }

    fn show_all(&mut self, cx: &mut Context<Self>) {
        let all: Vec<usize> = (0..self.panes.len()).collect();
        self.show_panes(&all, cx);
    }

    pub(super) fn show_panes(&mut self, revealed: &[usize], cx: &mut Context<Self>) {
        for ix in 0..self.panes.len() {
            self.announce_pane(ix, revealed.contains(&ix), cx);
        }
        self.shown(cx);
    }

    fn shown(&mut self, cx: &mut Context<Self>) {
        self.keys.refocus = true;
        self.sync_page(cx);
        self.track_load(cx);
        self.sync_palette(cx);
        self.save_tabs(cx);
        cx.notify();
    }

    pub(super) fn save_tabs(&mut self, cx: &mut Context<Self>) {
        let saved = self.state.panes().saved();
        if self.saved.as_ref() == Some(&saved) {
            return;
        }
        self.saved = Some(saved.clone());
        crate::session::record(self.window_id, saved, cx);
    }

    pub(super) fn retarget(&mut self, scope: &Scope, cx: &mut Context<Self>) {
        self.state.retarget(scope);
        self.show_active(cx);
    }

    pub(super) fn close_tab(
        &mut self,
        id: flock_core::tabs::TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.panes().is_last(id) {
            window.remove_window();
            return;
        }
        let active = self.state.panes().active().id();
        if !self.state.panes_mut().close(id) {
            return;
        }
        self.sync_panes(window, cx);
        if self.state.panes().active().id() == active {
            self.save_tabs(cx);
            cx.notify();
        } else {
            self.show_active(cx);
        }
    }

    fn ui_effects(&mut self, command: &Command, cx: &mut Context<Self>) {
        match command {
            Command::ForWindow(_, _) => {}
            Command::WindowClosed(_) => {}
            Command::ToggleSidebar => self.toggle_sidebar(cx),
            Command::SelectCluster(_) => {}
            Command::SelectPage(_) => {}
            Command::OpenResource(_) => {}
            Command::InspectResource(_) => {}
            Command::CloseInspector => {}
            Command::OpenCrd { .. } => {}
            Command::OpenHelmRelease(_) => {}
            Command::FollowLogs(_) => {}
            Command::Compare(_) => {}
            Command::EditYaml(_) => {}
            Command::Edit(_) => {}
            Command::CompareWith { .. } => {}
            #[cfg(feature = "loco")]
            Command::Loco(_) => {}
            Command::CopySpec { .. } => {}
            Command::NewTab(_) => {}
            Command::OpenLogs { .. } => {}
            Command::ShowClusterSettings(cluster) => {
                self.focused_screens()
                    .settings
                    .update(cx, |settings, cx| settings.open(cluster, cx));
            }
            Command::SetPreferences(cluster, preferences) => {
                self.state.set_preferences(cluster, preferences);
            }
            Command::SetRefresh(_) => {}
            Command::Connect(_) => {}
            Command::Reauthenticate(_) => {}
            Command::ConfirmContext(_) => {}
            Command::Disconnect(_) => {}
            Command::Refresh(_) => {}
            Command::PortForward {
                service,
                local,
                remote,
            } => {
                let title = format!(
                    "Forwarding localhost:{local} → {}/{}:{remote}",
                    short_kind(&service.kind),
                    service.name
                );
                let detail = format!(
                    "{} · {}",
                    service.cluster,
                    service.namespace.as_deref().unwrap_or_default()
                );
                toast(Toast::new(Status::Ok, title).detail(detail), cx);
            }
            Command::StopPortForward { .. } => {}
            Command::OpenClusterChanges(cluster) => {
                self.focused_screens()
                    .changes
                    .update(cx, |changes, cx| changes.show_cluster(cluster, cx));
            }
            Command::OpenCommit(sha) => {
                self.focused_screens()
                    .changes
                    .update(cx, |changes, cx| changes.open(sha, cx));
            }
            Command::Cordon(_) => {}
            Command::Uncordon(_) => {}
            Command::Drain(_) => {}
            Command::Delete(_) => {}
            Command::OpenAddCluster(_) => {}
            Command::CheckContexts(_) => {}
            Command::AddClusters(_) => {}
            Command::SaveKubeconfig { .. } => {}
            Command::RemoveCluster(_) => {}
            Command::RenameCluster(..) => {}
            Command::SwitchContext(..) => {}
            Command::WatchWorkload { .. } => {}
            Command::StreamLogs(_) => {}
            Command::StreamPodLogs(_) => {}
            Command::Exec(_) => {}
            Command::NodeShell(_) => {}
            Command::RestartRollout(_) => {}
            Command::Scale { .. } => {}
            Command::Rollback { .. } => {}
            Command::OpenFluxObject(_) => {}
            Command::Flux(..) => {}
            Command::CheckForUpdates => crate::updater::check_now(cx),
        }
    }

    fn run_shortcut(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(resource) = self.state.location().resource.as_ref() else {
            return;
        };
        let command = resource_commands(&self.state.snapshot, resource)
            .into_iter()
            .find(|hit| hit.shortcut == Some(key))
            .map(|hit| hit.command);
        if let Some(command) = command {
            self.dispatch(command, cx);
        }
    }

    fn sync_page(&mut self, cx: &mut Context<Self>) {
        let source = &self.state.snapshot;
        let panes = self.state.panes().panes();
        for (pane, screens) in panes.iter().zip(self.panes.iter_mut()) {
            screens.show_scope(source, pane.active().location(), cx);
        }
        for ix in 0..self.panes.len() {
            self.sync_pane(ix, cx);
        }
    }

    fn sync_pane(&self, ix: usize, cx: &mut Context<Self>) {
        let location = self.location_at(ix);
        let screens = self.screens(ix);
        let snapshot = &self.state.snapshot;
        let page = location.page;
        let cluster = self.cluster_of(location);
        let confirm_destructive = cluster
            .and_then(|c| snapshot.catalog.get(&c.id))
            .is_none_or(|settings| settings.preferences.confirm_destructive);
        let preferences = self.preferences_of(location);
        screens.pages.sync(
            page,
            cluster,
            snapshot,
            confirm_destructive,
            &preferences,
            cx,
        );
        if page == Page::CustomResources {
            let crd = location.crd.as_ref();
            screens
                .pages
                .sync_custom(cluster, crd, snapshot, &preferences, cx);
        }
        let object = location
            .flux_object
            .as_ref()
            .and_then(|key| snapshot.flux_page(key));
        let helm = location.helm.as_ref();
        screens
            .helm_release
            .update(cx, |screen, cx| screen.set(helm, snapshot, cx));
        let compact = self.full_view(ix, cx);
        screens.flux_object.update(cx, |screen, cx| {
            screen.set_page(object, cx);
            screen.set_compact(compact, cx);
        });
        #[cfg(feature = "loco")]
        screens
            .loco
            .update(cx, |loco, cx| loco.set_context(location, snapshot, cx));
        let resource = location.resource.as_ref();
        screens.compare.update(cx, |compare, cx| {
            compare.set_snapshot(snapshot, resource, cx);
        });
        let cluster = location.cluster.as_ref();
        let pod = location.inspecting.as_ref().or(resource);
        let on_pods = location.page == Page::Pods;
        screens.pods.update(cx, |pods, cx| {
            pods.set_context(cluster, pod, &preferences, cx);
            pods.sync_inspection(on_pods, resource, cx);
        });
        if let Some(bar) = self.filter_bar_at(ix, location, cx) {
            let namespaces = snapshot
                .details
                .iter()
                .filter(|(id, _)| cluster.is_none_or(|cluster| cluster == *id))
                .flat_map(|(_, detail)| {
                    detail
                        .namespaces
                        .iter()
                        .map(|namespace| namespace.name.clone())
                })
                .collect();
            bar.update(cx, |bar, cx| bar.set_namespaces(namespaces, cx));
        }
        if let Some((screen, _)) = &screens.workload {
            self.sync_workload_yaml(screen, cx);
        }
        if let Some((screen, _)) = &screens.detail {
            self.sync_detail(screen, location, cx);
        }
        if let Some((screen, _)) = &screens.namespace {
            self.sync_namespace(screen, location, cx);
        }
    }

    pub(super) fn sync_detail(
        &self,
        screen: &Entity<crate::screens::detail::DetailScreen>,
        location: &Location,
        cx: &mut App,
    ) {
        let preferences = self.preferences_of(location);
        let snapshot = &self.state.snapshot;
        screen.update(cx, |screen, cx| screen.set(snapshot, &preferences, cx));
    }

    pub(super) fn sync_namespace(
        &self,
        screen: &Entity<crate::screens::namespace::NamespaceScreen>,
        location: &Location,
        cx: &mut App,
    ) {
        let preferences = self.preferences_of(location);
        let snapshot = &self.state.snapshot;
        screen.update(cx, |screen, cx| screen.set(snapshot, &preferences, cx));
    }

    pub(super) fn sync_workload_yaml(&self, screen: &Entity<WorkloadScreen>, cx: &mut App) {
        let target = screen.read(cx).target().clone();
        let snapshot = &self.state.snapshot;
        let yaml = snapshot.yaml(&target);
        let related = snapshot
            .detail(&target.cluster)
            .and_then(|detail| detail.related(&target));
        let name = flock_core::tabs::cluster_name(&target.cluster, snapshot);
        screen.update(cx, |screen, cx| {
            screen.set_cluster_name(&name, cx);
            screen.set_yaml(yaml, cx);
            screen.set_related(related, cx);
        });
    }

    pub fn filter_bar(&self, cx: &App) -> Option<Entity<FilterBar>> {
        let location = self.state.location();
        Self::screen_filter_bar(self.focused_screens(), location, cx)
    }

    pub(super) fn filter_bar_at(
        &self,
        ix: usize,
        location: &Location,
        cx: &App,
    ) -> Option<Entity<FilterBar>> {
        Self::screen_filter_bar(self.screens(ix), location, cx)
    }

    fn screen_filter_bar(
        screens: &PaneScreens,
        location: &Location,
        cx: &App,
    ) -> Option<Entity<FilterBar>> {
        if location.workload().is_some()
            || location.detail().is_some()
            || location.flux_object.is_some()
            || location.helm.is_some()
        {
            return None;
        }
        #[cfg(feature = "loco")]
        if crate::screens::loco::owns(location.page) {
            return screens.loco.read(cx).filter_bar(location.page, cx);
        }
        match location.page {
            Page::Pods => Some(screens.pods.read(cx).filter_bar().clone()),
            Page::Problems => Some(screens.problems.read(cx).filter_bar().clone()),
            Page::Changes => Some(screens.changes.read(cx).filter_bar().clone()),
            Page::GitOps => Some(screens.gitops.read(cx).filter_bar().clone()),
            Page::FluxEvents => Some(screens.flux_events.read(cx).filter_bar().clone()),
            page => screens.pages.filter_bar(page, cx),
        }
    }

    fn focus_filter(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(bar) = self.filter_bar(cx) {
            bar.update(cx, |bar, cx| bar.focus(window, cx));
        }
    }

    pub fn open_node_shell(
        &mut self,
        node: flock_core::ResourceRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focused_screens()
            .pages
            .open_node_shell(node, window, cx);
    }

    pub fn show_pod_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focused_screens()
            .pods
            .update(cx, |pods, cx| pods.open_terminal(window, cx));
    }

    fn pane_views(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let mut views = Vec::new();
        for ix in 0..self.panes.len() {
            let leading = if ix == 0 {
                self.window_controls(cx)
            } else {
                None
            };
            let strip = self.tab_strip(ix, leading, cx);
            let page = self.page(ix, window, cx);
            let view = div()
                .debug_selector(move || format!("pane-{ix}"))
                .flex()
                .flex_col()
                .size_full()
                .min_w_0()
                .capture_any_mouse_down(cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.focus_pane(ix, cx);
                    if let MouseButton::Navigate(direction) = event.button {
                        let direction = match direction {
                            gpui::NavigationDirection::Back => flock_core::history::Direction::Back,
                            gpui::NavigationDirection::Forward => {
                                flock_core::history::Direction::Forward
                            }
                        };
                        if this.state.jump_history(direction, 1) {
                            this.show_active(cx);
                        }
                        cx.stop_propagation();
                    }
                }))
                .child(strip)
                .child(div().flex().flex_1().min_h_0().child(page));
            views.push(view.into_any_element());
        }
        let mut views = views.into_iter();
        let (Some(first), second) = (views.next(), views.next()) else {
            return div().into_any_element();
        };
        let Some(second) = second else {
            return first;
        };
        let palette = *cx.palette();
        let shell = cx.weak_entity();
        let released = shell.clone();
        Split::horizontal(PANES_SPLIT, first, second)
            .ratio(self.state.panes().divider().share())
            .line(palette.line2)
            .on_resize(move |share, _, cx| {
                shell
                    .update(cx, |shell, cx| shell.resize_panes(share, cx))
                    .ok();
            })
            .on_resize_end(move |_, cx| {
                released.update(cx, Shell::finish_resize_panes).ok();
            })
            .into_any_element()
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        self.keep_keyboard_focus(window, cx);
        let panes = self.pane_views(window, cx);
        let root = div()
            .id("shell")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &FocusFilter, window, cx| {
                this.focus_filter(window, cx);
            }))
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, _| {
                this.new_tab_gesture =
                    event.modifiers.platform || event.button == MouseButton::Middle;
            }))
            .capture_key_down(cx.listener(|this, _: &KeyDownEvent, _, _| {
                this.new_tab_gesture = false;
            }));
        Self::key_actions(root, cx)
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| {
                this.dispatch(Command::SelectPage(Page::Settings), cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.dispatch(Command::ToggleSidebar, cx);
            }))
            .on_action(cx.listener(|this, _: &RefreshView, _, cx| {
                this.refresh_view(cx);
            }))
            .on_action(cx.listener(|this, _: &ShowAllClusters, _, cx| {
                this.retarget(&Scope::All, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleClusterSwitcher, window, cx| {
                this.switcher
                    .update(cx, |switcher, cx| switcher.toggle(window, cx));
            }))
            .on_action(cx.listener(|this, _: &ToggleCommandPalette, window, cx| {
                this.toggle_palette(window, cx);
            }))
            .on_action(cx.listener(|this, _: &FollowLogs, _, cx| {
                this.run_shortcut(LOGS_KEY, cx);
            }))
            .on_action(cx.listener(|this, _: &CompareAcrossClusters, _, cx| {
                this.run_shortcut(COMPARE_KEY, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowFluxOwner, _, cx| {
                this.run_shortcut(OWNER_KEY, cx);
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(palette.editor)
            .text_color(palette.fg)
            .font_family(theme::FONT_SANS)
            .text_size(TEXT_UI)
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.sidebar(window, cx))
                    .child(div().flex().flex_1().min_w_0().child(panes))
                    .children(self.sidebar_reopen(cx))
                    .child(Self::sidebar_drag_tracker(cx)),
            )
            .child(self.status_bar(cx))
            .child(self.palette.clone())
            .child(self.tab_menu.clone())
            .children(self.add_cluster.as_ref().map(|(dialog, _)| dialog.clone()))
            .children(self.rename.as_ref().map(|(dialog, _)| dialog.clone()))
            .children(
                self.switch_context
                    .as_ref()
                    .map(|(dialog, _)| dialog.clone()),
            )
            .child(crate::kit::overlay_layer(cx))
    }
}

fn revealed(command: &Command) -> Option<&flock_core::ResourceRef> {
    match command {
        Command::OpenResource(target) => Some(target),
        Command::InspectResource(target) => Some(target),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{Root, Store};
    use flock_core::ClusterId;
    use gpui::{AnyWindowHandle, KeyBinding, TestAppContext, WindowOptions};

    fn open_window(
        store: &Entity<Store>,
        initial: Vec<Command>,
        cx: &mut App,
    ) -> (Entity<Shell>, AnyWindowHandle) {
        let mut shell = None;
        let opened = cx.open_window(WindowOptions::default(), |window, cx| {
            let view = cx.new(|cx| Shell::new(store.clone(), initial, window, cx));
            shell = Some(view.clone());
            cx.new(|cx| Root::new(view, window, cx))
        });
        let handle = opened.unwrap_or_else(|error| panic!("test window: {error}"));
        let shell = shell.unwrap_or_else(|| panic!("test shell missing"));
        (shell, handle.into())
    }

    #[gpui::test(iterations = 20)]
    fn palette_new_window_keeps_source_selection_and_shares_snapshots(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::init(Some(theme::Appearance::Dark), cx)
                .unwrap_or_else(|error| panic!("UI init: {error}"));
            cx.bind_keys([
                KeyBinding::new(
                    "alt-enter",
                    crate::actions::OpenSelectedNewWindow,
                    Some("CommandPalette"),
                ),
                KeyBinding::new("cmd-shift-w", crate::actions::CloseWindow, None),
            ]);
        });
        let (backend, updates) = async_channel::unbounded();
        let (commands, received) = async_channel::unbounded();
        let store = cx.new(|cx| Store::new(updates, commands, cx));
        let (first, first_window) = cx.update(|cx| open_window(&store, Vec::new(), cx));
        let (requests, opened) = async_channel::unbounded();
        cx.update(|cx| {
            cx.subscribe(&first, move |_, request: &NewWindowRequest, _| {
                assert!(requests.try_send(request.0.clone()).is_ok());
            })
            .detach();
        });
        assert!(backend.try_send(Update::Snapshot(Arc::default())).is_ok());
        cx.run_until_parked();
        let before = first.read_with(cx, |shell, _| shell.state.clone());
        assert!(
            first_window
                .update(cx, |_, window, cx| {
                    first.update(cx, |shell, cx| shell.open_palette("GitOps", window, cx));
                })
                .is_ok()
        );
        cx.simulate_keystrokes(first_window, "alt-enter");
        let initial = opened
            .try_recv()
            .unwrap_or_else(|error| panic!("new window request: {error}"));
        assert_eq!(initial, vec![Command::SelectPage(Page::GitOps)]);
        let (second, second_window) = cx.update(|cx| open_window(&store, initial, cx));
        cx.run_until_parked();
        assert_eq!(first.read_with(cx, |shell, _| shell.state.clone()), before);
        assert_eq!(
            second.read_with(cx, |shell, _| shell.state.location().page),
            Page::GitOps
        );
        let mut snapshot = flock_core::Snapshot::default();
        snapshot.home.problems.push(flock_core::home::Problem {
            target: flock_core::ResourceRef {
                cluster: ClusterId::new("test"),
                kind: "Pod".into(),
                namespace: None,
                name: "unavailable".into(),
            },
            status: Status::Err,
            reason: "Unavailable".into(),
            message: "unavailable".into(),
            condition: String::new(),
            age: std::time::Duration::ZERO,
            events: Vec::new(),
        });
        let snapshot = Arc::new(snapshot);
        let sent = Update::Snapshot(Arc::clone(&snapshot));
        assert!(backend.try_send(sent).is_ok());
        cx.run_until_parked();
        let shares = |shell: &Entity<Shell>, cx: &mut TestAppContext| {
            shell.read_with(cx, |shell, _| Arc::ptr_eq(&shell.state.snapshot, &snapshot))
        };
        assert!(shares(&first, cx));
        assert!(shares(&second, cx));
        cx.simulate_keystrokes(second_window, "cmd-shift-w");
        cx.run_until_parked();
        assert_eq!(cx.read(|cx| cx.windows().len()), 1);
        drop(second);
        cx.run_until_parked();
        let sent: Vec<_> = std::iter::from_fn(|| received.try_recv().ok()).collect();
        assert!(
            sent.iter()
                .any(|command| matches!(command, Command::WindowClosed(_)))
        );
    }

    #[gpui::test]
    fn palette_new_window_does_not_run_panel_commands(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::init(Some(theme::Appearance::Light), cx)
                .unwrap_or_else(|error| panic!("UI init: {error}"));
            cx.bind_keys([KeyBinding::new(
                "alt-enter",
                crate::actions::OpenSelectedNewWindow,
                Some("CommandPalette"),
            )]);
        });
        let (_, updates) = async_channel::unbounded();
        let (commands, received) = async_channel::unbounded();
        let store = cx.new(|cx| Store::new(updates, commands, cx));
        let (shell, window) = cx.update(|cx| open_window(&store, Vec::new(), cx));
        assert!(
            window
                .update(cx, |_, window, cx| {
                    shell.update(cx, |shell, cx| {
                        shell.open_palette(">toggle sidebar", window, cx);
                    });
                })
                .is_ok()
        );
        cx.simulate_keystrokes(window, "alt-enter");
        cx.run_until_parked();
        assert!(received.is_empty());
        assert!(shell.read_with(cx, |shell, cx| shell.palette.read(cx).is_open()));
        assert!(shell.read_with(cx, |shell, _| shell.state.sidebar_open));
    }

    #[gpui::test]
    fn a_new_window_opens_the_sidebar_as_last_saved(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::init(None, cx).unwrap_or_else(|error| panic!("UI init: {error}"));
        });
        let (_, updates) = async_channel::unbounded();
        let (commands, _received) = async_channel::unbounded();
        let store = cx.new(|cx| Store::new(updates, commands, cx));
        let (first, _) = cx.update(|cx| open_window(&store, Vec::new(), cx));
        assert!(first.read_with(cx, |shell, _| shell.state.sidebar_open));
        first.update(cx, |shell, cx| shell.dispatch(Command::ToggleSidebar, cx));
        cx.run_until_parked();
        let closed = cx.update(|cx| crate::settings::current(cx).sidebar_closed);
        assert!(closed);
        let (second, _) = cx.update(|cx| open_window(&store, Vec::new(), cx));
        assert!(!second.read_with(cx, |shell, _| shell.state.sidebar_open));
    }

    fn scope_of(sent: &[Command], wanted: &Command) -> Option<u64> {
        sent.iter().find_map(|command| match command {
            Command::ForWindow(scope, inner) if inner.as_ref() == wanted => Some(*scope),
            _ => None,
        })
    }

    #[gpui::test]
    fn each_pane_reaches_the_backend_under_its_own_scope(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::init(Some(theme::Appearance::Dark), cx)
                .unwrap_or_else(|error| panic!("UI init: {error}"));
            cx.bind_keys(crate::keymap::key_bindings());
        });
        let (backend, updates) = async_channel::unbounded();
        let (commands, received) = async_channel::unbounded();
        let store = cx.new(|cx| Store::new(updates, commands, cx));
        let (shell, window) = cx.update(|cx| open_window(&store, Vec::new(), cx));
        assert!(backend.try_send(Update::Snapshot(Arc::default())).is_ok());
        cx.run_until_parked();
        let saved = flock_core::tabs::SavedWindow {
            panes: [Page::GitOps, Page::Changes]
                .into_iter()
                .map(|page| flock_core::tabs::SavedPane {
                    tabs: vec![Location::flock(page)],
                    active: 0,
                })
                .collect(),
            focused: 1,
            ..flock_core::tabs::SavedWindow::default()
        };
        assert!(
            window
                .update(cx, |_, window, cx| {
                    shell.update(cx, |shell, cx| shell.restore_tabs(&saved, window, cx));
                })
                .is_ok()
        );
        cx.run_until_parked();
        let sent: Vec<_> = std::iter::from_fn(|| received.try_recv().ok()).collect();
        let left = scope_of(&sent, &Command::SelectPage(Page::GitOps));
        let right = scope_of(&sent, &Command::SelectPage(Page::Changes));
        assert!(left.is_some() && right.is_some() && left != right);
        cx.simulate_keystrokes(window, "cmd-w");
        cx.run_until_parked();
        let sent: Vec<_> = std::iter::from_fn(|| received.try_recv().ok()).collect();
        let closed = right.map(Command::WindowClosed);
        assert!(sent.iter().any(|command| Some(command) == closed.as_ref()));
        assert_eq!(shell.read_with(cx, |shell, _| shell.panes.len()), 1);
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.state.location().page),
            Page::GitOps
        );
    }

    #[gpui::test]
    fn the_refresh_shortcut_asks_the_backend_to_relist_the_current_scope(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::init(Some(theme::Appearance::Dark), cx)
                .unwrap_or_else(|error| panic!("UI init: {error}"));
            cx.bind_keys(crate::keymap::key_bindings());
        });
        let (backend, updates) = async_channel::unbounded();
        let (commands, received) = async_channel::unbounded();
        let store = cx.new(|cx| Store::new(updates, commands, cx));
        let (_shell, window) = cx.update(|cx| open_window(&store, Vec::new(), cx));
        assert!(backend.try_send(Update::Snapshot(Arc::default())).is_ok());
        cx.run_until_parked();
        while received.try_recv().is_ok() {}
        cx.simulate_keystrokes(window, "cmd-alt-r");
        cx.run_until_parked();
        let sent: Vec<_> = std::iter::from_fn(|| received.try_recv().ok()).collect();
        assert!(scope_of(&sent, &Command::Refresh(None)).is_some());
    }

    #[gpui::test]
    fn tabs_are_saved_per_window_and_restore_into_a_new_one(cx: &mut TestAppContext) {
        let saved = std::rc::Rc::new(std::cell::RefCell::new(flock_core::tabs::Session::default()));
        let sink = saved.clone();
        cx.update(|cx| {
            crate::init(Some(theme::Appearance::Dark), cx)
                .unwrap_or_else(|error| panic!("UI init: {error}"));
            cx.bind_keys(crate::keymap::key_bindings());
            crate::session::install(
                std::rc::Rc::new(move |session| {
                    sink.replace(session.clone());
                    Ok(())
                }),
                cx,
            );
        });
        let (backend, updates) = async_channel::unbounded();
        let (commands, _received) = async_channel::unbounded();
        let store = cx.new(|cx| Store::new(updates, commands, cx));
        let (first, first_window) = cx.update(|cx| open_window(&store, Vec::new(), cx));
        assert!(backend.try_send(Update::Snapshot(Arc::default())).is_ok());
        cx.run_until_parked();
        cx.simulate_keystrokes(first_window, "cmd-t cmd-t cmd-1");
        cx.run_until_parked();
        let written = saved.borrow().clone();
        let expected = first.read_with(cx, |shell, _| shell.state.panes().saved());
        assert_eq!(written.windows, std::slice::from_ref(&expected));
        assert_eq!(expected.panes.first().map(|p| p.tabs.len()), Some(3));
        let (second, second_window) = cx.update(|cx| open_window(&store, Vec::new(), cx));
        cx.run_until_parked();
        assert!(
            second_window
                .update(cx, |_, window, cx| {
                    second.update(cx, |shell, cx| shell.restore_tabs(&expected, window, cx));
                })
                .is_ok()
        );
        cx.run_until_parked();
        assert_eq!(
            second.read_with(cx, |shell, _| shell.state.panes().saved()),
            expected
        );
        assert_eq!(saved.borrow().windows.len(), 2);
        cx.simulate_keystrokes(second_window, "cmd-shift-w");
        cx.run_until_parked();
        assert_eq!(saved.borrow().windows, [expected]);
    }
}
