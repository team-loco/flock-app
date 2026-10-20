use std::path::PathBuf;

use crate::app_settings::Refresh;
use crate::crd::CrdKey;
use crate::helm::HelmKey;
use crate::history::Location;
use crate::resource::page_for_kind;
use crate::tabs::{Panes, SavedWindow, TabId};
use crate::{
    AddClusterStep, CheckLine, ClusterId, ContextRef, ExecRequest, FluxAction, FluxKey, FluxKind,
    KubeconfigFile, LogRequest, LogTarget, MultiLogRequest, NewCluster, Notice, Page, Preferences,
    ResourceRef, Seen, Sink, Snapshot, Uid, Verdict, Workload,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    ForWindow(u64, Box<Command>),
    WindowClosed(u64),
    SelectCluster(Option<ClusterId>),
    SelectPage(Page),
    NewTab(Box<Command>),
    OpenCommit(String),
    OpenClusterChanges(ClusterId),
    ToggleSidebar,
    OpenResource(ResourceRef),
    InspectResource(ResourceRef),
    CloseInspector,
    OpenCrd {
        cluster: ClusterId,
        crd: CrdKey,
    },
    OpenHelmRelease(HelmKey),
    FollowLogs(LogTarget),
    OpenLogs {
        pod: ResourceRef,
        container: String,
    },
    Compare(ResourceRef),
    EditYaml(ResourceRef),
    Edit(Box<crate::edit::EditRequest>),
    CompareWith {
        base: ResourceRef,
        other: ClusterId,
    },
    PortForward {
        service: ResourceRef,
        local: u16,
        remote: u16,
    },
    StopPortForward {
        local: u16,
    },
    Cordon(Seen),
    Uncordon(Seen),
    Drain(Seen),
    Delete(Seen),
    OpenAddCluster(AddClusterStep),
    CheckContexts(Vec<ContextRef>),
    AddClusters(Vec<NewCluster>),
    SaveKubeconfig {
        name: String,
        yaml: String,
    },
    WatchFolder(PathBuf),
    RemoveCluster(ClusterId),
    WatchWorkload {
        target: ResourceRef,
        sink: Sink<Workload>,
    },
    StreamLogs(LogRequest),
    StreamPodLogs(MultiLogRequest),
    Exec(ExecRequest),
    RestartRollout(Seen),
    Scale {
        target: Seen,
        replicas: u32,
    },
    Rollback {
        target: Seen,
        revision: u64,
    },
    OpenFluxObject(FluxKey),
    Flux(FluxKey, Uid, FluxAction),
    #[cfg(feature = "loco")]
    Loco(crate::loco::LocoCommand),
    CopySpec {
        from: ClusterId,
        to: Seen,
        path: String,
    },
    ShowClusterSettings(ClusterId),
    SetPreferences(ClusterId, Preferences),
    SetRefresh(Refresh),
    Connect(ClusterId),
    Reauthenticate(ClusterId),
    ConfirmContext(ClusterId),
    Disconnect(ClusterId),
    Refresh(Option<ClusterId>),
    CheckForUpdates,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    Snapshot(Box<Snapshot>),
    Contexts(Vec<KubeconfigFile>),
    KubeconfigRejected(String),
    Check(ContextRef, CheckLine),
    Checked(ContextRef, Verdict),
    Notice(Notice),
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppState {
    pub snapshot: Snapshot,
    pub sidebar_open: bool,
    panes: Panes,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            snapshot: Snapshot::default(),
            sidebar_open: true,
            panes: Panes::default(),
        }
    }
}

impl AppState {
    pub fn location(&self) -> &Location {
        self.panes.active().location()
    }

    pub fn panes(&self) -> &Panes {
        &self.panes
    }

    pub fn panes_mut(&mut self) -> &mut Panes {
        &mut self.panes
    }

    pub fn restore(&mut self, saved: &SavedWindow) {
        self.panes = Panes::restore(saved);
    }

    pub fn window_commands(&self, command: &Command) -> Option<Vec<Command>> {
        match command {
            Command::SelectCluster(_) => Some(vec![command.clone()]),
            Command::SelectPage(page) => {
                let mut commands = Vec::new();
                if !page.is_flock() {
                    commands.push(Command::SelectCluster(self.location().cluster.clone()));
                }
                commands.push(command.clone());
                Some(commands)
            }
            Command::OpenResource(_) => Some(vec![command.clone()]),
            Command::OpenCrd { .. } => Some(vec![command.clone()]),
            Command::OpenHelmRelease(_) => Some(vec![command.clone()]),
            Command::FollowLogs(_) => Some(vec![command.clone()]),
            Command::OpenLogs { .. } => Some(vec![command.clone()]),
            Command::Compare(_) => Some(vec![command.clone()]),
            Command::EditYaml(_) => Some(vec![command.clone()]),
            Command::CompareWith { .. } => Some(vec![command.clone()]),
            Command::OpenFluxObject(_) => Some(vec![command.clone()]),
            Command::OpenCommit(_) => Some(vec![command.clone()]),
            Command::OpenClusterChanges(_) => Some(vec![command.clone()]),
            Command::ShowClusterSettings(_) => Some(vec![command.clone()]),
            Command::NewTab(inner) => self.window_commands(inner),
            _ => None,
        }
    }

    pub fn apply(&mut self, command: &Command) {
        match command {
            Command::NewTab(inner) => {
                self.open_tab(inner);
                return;
            }
            Command::ToggleSidebar => self.sidebar_open = !self.sidebar_open,
            _ => {}
        }
        let snapshot = &self.snapshot;
        let tab = self.panes.active_mut();
        navigate(tab.location_mut(), command, snapshot);
        if is_inspection(command) {
            tab.amend();
        } else {
            tab.record();
        }
    }

    pub fn open_tab(&mut self, command: &Command) -> Option<TabId> {
        let commands = self.window_commands(command)?;
        let mut location = self.location().clone();
        for command in &commands {
            navigate(&mut location, command, &self.snapshot);
        }
        let tab = match command {
            Command::EditYaml(_) => self.panes.open_from(self.location().clone(), location),
            _ => self.panes.open(location),
        };
        Some(tab)
    }

    pub fn open_split(&mut self, command: &Command) -> bool {
        let Some(commands) = self.window_commands(command) else {
            return false;
        };
        let mut location = self.location().clone();
        for command in &commands {
            navigate(&mut location, command, &self.snapshot);
        }
        self.panes.split_to(location);
        true
    }

    pub fn new_tab(&mut self) -> TabId {
        self.panes.add(Location::default())
    }

    pub fn jump_history(&mut self, direction: crate::history::Direction, steps: usize) -> bool {
        let tab = self.panes.active_mut();
        let Some(target) = tab.history_mut().jump(direction, steps) else {
            return false;
        };
        *tab.location_mut() = target;
        true
    }

    pub fn back(&mut self) -> bool {
        let tab = self.panes.active_mut();
        let Some(target) = tab.history_mut().back() else {
            return false;
        };
        *tab.location_mut() = target;
        true
    }

    pub fn forward(&mut self) -> bool {
        let tab = self.panes.active_mut();
        let Some(target) = tab.history_mut().forward() else {
            return false;
        };
        *tab.location_mut() = target;
        true
    }

    pub fn retarget(&mut self, cluster: Option<&ClusterId>) {
        let target = retargeted(self.location(), cluster, &self.snapshot);
        let tab = self.panes.active_mut();
        *tab.location_mut() = target;
        tab.record();
    }

    pub fn ready_for(&self, command: &Command) -> bool {
        let has_cluster = |id: &ClusterId| self.snapshot.cluster(id).is_some();
        match command {
            Command::SelectCluster(Some(id)) => has_cluster(id),
            Command::OpenResource(resource) => has_cluster(&resource.cluster),
            Command::OpenCrd { cluster, .. } => has_cluster(cluster),
            Command::OpenHelmRelease(key) => has_cluster(&key.cluster),
            Command::FollowLogs(target) => has_cluster(target.cluster()),
            Command::OpenLogs { pod, .. } => has_cluster(&pod.cluster),
            Command::Compare(resource) => has_cluster(&resource.cluster),
            Command::EditYaml(resource) => has_cluster(&resource.cluster),
            Command::CompareWith { base, .. } => has_cluster(&base.cluster),
            Command::ShowClusterSettings(id) => has_cluster(id),
            Command::OpenFluxObject(key) => self.snapshot.flux_object(key).is_some(),
            Command::NewTab(inner) => self.ready_for(inner),
            _ => true,
        }
    }

    pub fn update(&mut self, update: Update) {
        match update {
            Update::Snapshot(snapshot) => {
                let previous = std::mem::replace(&mut self.snapshot, *snapshot);
                for tab in self.panes.tabs_mut() {
                    let location = tab.location_mut();
                    let gone = location.flux_object.as_ref().is_some_and(|key| {
                        previous.flux_object(key).is_some()
                            && self.snapshot.flux_object(key).is_none()
                    });
                    if gone {
                        location.flux_object = None;
                    }
                }
                self.snapshot.keep_loaded(previous);
            }
            Update::Contexts(_) => {}
            Update::KubeconfigRejected(_) => {}
            Update::Check(..) => {}
            Update::Checked(..) => {}
            Update::Notice(_) => {}
        }
    }

    pub fn cluster_available(&self) -> bool {
        self.available(self.location())
    }

    pub fn available(&self, location: &Location) -> bool {
        location
            .cluster
            .as_ref()
            .is_none_or(|id| self.snapshot.cluster(id).is_some())
    }

    pub fn workload(&self) -> Option<&ResourceRef> {
        self.location().workload()
    }
}

fn navigate(location: &mut Location, command: &Command, snapshot: &Snapshot) {
    let has_cluster = |id: &ClusterId| snapshot.cluster(id).is_some();
    match command {
        Command::SelectCluster(Some(id)) => {
            if has_cluster(id) {
                *location = Location::page_on(id.clone(), Page::ClusterOverview);
            }
        }
        Command::SelectCluster(None) => *location = Location::default(),
        Command::SelectPage(page) => select_page(location, *page),
        Command::OpenCommit(_) => select_page(location, Page::Changes),
        Command::OpenClusterChanges(_) => select_page(location, Page::Changes),
        Command::OpenResource(resource) => open_resource(location, resource, snapshot),
        Command::InspectResource(target) => {
            if location.cluster.as_ref() == Some(&target.cluster) {
                location.inspecting = Some(target.clone());
            }
        }
        Command::CloseInspector => location.inspecting = None,
        Command::OpenCrd { cluster, crd } => {
            if has_cluster(cluster) {
                *location = Location::crd_on(cluster.clone(), crd.clone());
            }
        }
        Command::OpenHelmRelease(key) => {
            if has_cluster(&key.cluster) {
                *location = Location::helm_release(key.clone());
            }
        }
        Command::FollowLogs(target) => {
            if has_cluster(target.cluster()) {
                *location = Location {
                    logs: Some(target.clone()),
                    ..Location::page_on(target.cluster().clone(), Page::Logs)
                };
            }
        }
        Command::OpenLogs { pod, container } => {
            if has_cluster(&pod.cluster) {
                *location = Location {
                    resource: Some(pod.clone()),
                    container: Some(container.clone()),
                    ..Location::page_on(pod.cluster.clone(), Page::Logs)
                };
            }
        }
        Command::Compare(resource) => compare(location, resource, snapshot),
        Command::EditYaml(resource) => edit_yaml(location, resource, snapshot),
        Command::CompareWith { base, .. } => compare(location, base, snapshot),
        Command::OpenFluxObject(key) => open_flux_object(location, key, snapshot),
        Command::ShowClusterSettings(_) => select_page(location, Page::ClusterSettings),
        _ => {}
    }
}

const fn is_inspection(command: &Command) -> bool {
    matches!(
        command,
        Command::InspectResource(_) | Command::CloseInspector
    )
}

fn select_page(location: &mut Location, page: Page) {
    if page.is_flock() {
        *location = Location::flock(page);
    } else if let Some(cluster) = location.cluster.clone() {
        *location = Location::page_on(cluster, page);
    }
}

fn compare(location: &mut Location, resource: &ResourceRef, snapshot: &Snapshot) {
    if snapshot.cluster(&resource.cluster).is_none() {
        return;
    }
    *location = Location {
        resource: Some(resource.clone()),
        ..Location::page_on(resource.cluster.clone(), Page::Compare)
    };
}

fn edit_yaml(location: &mut Location, resource: &ResourceRef, snapshot: &Snapshot) {
    if snapshot.cluster(&resource.cluster).is_none() {
        return;
    }
    *location = Location {
        resource: Some(resource.clone()),
        ..Location::page_on(resource.cluster.clone(), Page::EditYaml)
    };
}

fn open_resource(location: &mut Location, resource: &ResourceRef, snapshot: &Snapshot) {
    let flux = FluxKind::from_label(&resource.kind).zip(resource.namespace.as_ref());
    let key = flux.map(|(kind, namespace)| FluxKey {
        cluster: resource.cluster.clone(),
        kind,
        namespace: namespace.clone(),
        name: resource.name.clone(),
    });
    if let Some(key) = key.filter(|key| snapshot.flux_object(key).is_some()) {
        open_flux_object(location, &key, snapshot);
        return;
    }
    if snapshot.cluster(&resource.cluster).is_none() {
        return;
    }
    *location = Location {
        resource: Some(resource.clone()),
        crd: CrdKey::parse(&resource.kind),
        ..Location::page_on(resource.cluster.clone(), page_for_kind(&resource.kind))
    };
}

fn open_flux_object(location: &mut Location, key: &FluxKey, snapshot: &Snapshot) {
    if snapshot.flux_object(key).is_none() {
        return;
    }
    *location = Location {
        flux_object: Some(key.clone()),
        ..Location::page_on(key.cluster.clone(), key.kind.page())
    };
}

fn exists(resource: &ResourceRef, snapshot: &Snapshot) -> bool {
    snapshot.pods.iter().any(|pod| pod.is(resource))
        || snapshot.resources.iter().any(|r| &r.resource == resource)
}

fn retargeted(location: &Location, cluster: Option<&ClusterId>, snapshot: &Snapshot) -> Location {
    let Some(id) = cluster else {
        return Location::default();
    };
    if snapshot.cluster(id).is_none() {
        return location.clone();
    }
    let overview = Location::page_on(id.clone(), Page::ClusterOverview);
    if location.page.is_flock() {
        return overview;
    }
    if let Some(key) = &location.flux_object {
        let moved = key.on(id);
        if snapshot.flux_object(&moved).is_none() {
            return overview;
        }
        return Location {
            flux_object: Some(moved),
            ..Location::page_on(id.clone(), location.page)
        };
    }
    if let Some(target) = &location.logs {
        let moved = target.on(id);
        let present = match &moved {
            LogTarget::Workload(workload) => exists(workload, snapshot),
            LogTarget::Selector { .. } => true,
            LogTarget::Pods { .. } => false,
        };
        if !present {
            return overview;
        }
        return Location {
            logs: Some(moved),
            ..Location::page_on(id.clone(), Page::Logs)
        };
    }
    if location.helm.is_some() {
        return Location::page_on(id.clone(), Page::Releases);
    }
    if let Some(crd) = &location.crd {
        if snapshot.crd(id, crd).is_none() {
            return overview;
        }
        return Location::crd_on(id.clone(), crd.clone());
    }
    if let Some(resource) = &location.resource {
        let moved = ResourceRef {
            cluster: id.clone(),
            ..resource.clone()
        };
        if !exists(&moved, snapshot) {
            return overview;
        }
        return Location {
            cluster: Some(id.clone()),
            resource: Some(moved),
            ..location.clone()
        };
    }
    let listed = snapshot
        .nav_sections(Some(id))
        .iter()
        .any(|section| section.pages.contains(&location.page));
    if listed {
        Location::page_on(id.clone(), location.page)
    } else {
        overview
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flock::tests::sample;

    fn loaded() -> AppState {
        let mut state = AppState::default();
        state.update(Update::Snapshot(Box::new(sample())));
        state
    }

    #[test]
    fn new_window_selection_is_independent_and_preserves_page_scope() {
        let mut source = loaded();
        source.apply(&Command::SelectCluster(Some(ClusterId::new("a"))));
        let before = source.clone();
        let commands = source
            .window_commands(&Command::SelectPage(Page::Pods))
            .unwrap_or_default();
        let mut destination = loaded();
        for command in commands {
            destination.apply(&command);
        }
        assert_eq!(destination.location().cluster, source.location().cluster);
        assert_eq!(destination.location().page, Page::Pods);
        destination.apply(&Command::OpenResource(deployment("b")));
        assert_eq!(source, before);
        assert_eq!(destination.location().resource, Some(deployment("b")));
        destination.update(Update::Snapshot(Box::default()));
        assert_eq!(destination.location().resource, Some(deployment("b")));
        assert!(!destination.cluster_available());
        assert_eq!(source, before);
    }

    #[test]
    fn new_window_flux_selection_does_not_change_the_source_window() {
        let (source, key) = with_flux();
        let before = source.clone();
        let mut destination = source.clone();
        let commands = source
            .window_commands(&Command::OpenFluxObject(key.clone()))
            .unwrap_or_default();
        for command in commands {
            destination.apply(&command);
        }
        assert_eq!(destination.location().flux_object, Some(key));
        assert_eq!(source, before);
        destination.update(Update::Snapshot(Box::new(sample())));
        assert_eq!(destination.location().flux_object, None);
        assert_eq!(source, before);
    }

    #[test]
    fn new_windows_open_flock_targets_and_never_execute_writes() {
        let mut source = loaded();
        source.apply(&Command::SelectCluster(Some(ClusterId::new("a"))));
        assert_eq!(
            source.window_commands(&Command::SelectPage(Page::GitOps)),
            Some(vec![Command::SelectPage(Page::GitOps)])
        );
        assert_eq!(
            source.window_commands(&Command::RestartRollout(Seen::new(
                deployment("a"),
                Uid::new("u1")
            ))),
            None
        );
        assert_eq!(source.window_commands(&Command::ToggleSidebar), None);
    }

    #[test]
    fn opening_a_result_in_a_split_navigates_only_the_new_pane() {
        let mut state = loaded();
        state.apply(&Command::SelectCluster(Some(ClusterId::new("a"))));
        assert!(state.open_split(&Command::SelectPage(Page::Pods)));
        assert_eq!(state.panes().panes().len(), 2);
        assert_eq!(state.location().page, Page::Pods);
        assert_eq!(state.location().cluster, Some(ClusterId::new("a")));
        let left = state
            .panes()
            .panes()
            .first()
            .map(|pane| pane.active().location().page);
        assert_eq!(left, Some(Page::ClusterOverview));
        assert!(!state.open_split(&Command::ToggleSidebar));
        assert_eq!(state.panes().panes().len(), 2);
    }

    #[test]
    fn selecting_a_cluster_opens_its_overview() {
        let mut state = loaded();
        state.apply(&Command::SelectCluster(Some(ClusterId::new("a"))));
        assert_eq!(state.location().cluster, Some(ClusterId::new("a")));
        assert_eq!(state.location().page, Page::ClusterOverview);
    }

    #[test]
    fn selecting_an_unknown_cluster_is_ignored() {
        let mut state = loaded();
        state.apply(&Command::SelectCluster(Some(ClusterId::new("nope"))));
        assert_eq!(state, loaded());
    }

    #[test]
    fn cluster_pages_need_a_cluster() {
        let mut state = loaded();
        state.apply(&Command::SelectPage(Page::Pods));
        assert_eq!(state.location().page, Page::Overview);
        state.apply(&Command::SelectCluster(Some(ClusterId::new("b"))));
        state.apply(&Command::SelectPage(Page::Pods));
        assert_eq!(state.location().page, Page::Pods);
    }

    #[test]
    fn flock_pages_clear_the_cluster() {
        let mut state = loaded();
        state.apply(&Command::SelectCluster(Some(ClusterId::new("a"))));
        state.apply(&Command::SelectPage(Page::GitOps));
        assert_eq!(state.location().cluster, None);
        assert_eq!(state.location().page, Page::GitOps);
    }

    fn deployment(cluster: &str) -> ResourceRef {
        ResourceRef {
            cluster: ClusterId::new(cluster),
            kind: "Deployment".into(),
            namespace: Some("payments".into()),
            name: "checkout-api".into(),
        }
    }

    #[test]
    fn opening_a_workload_selects_its_cluster_and_list_page() {
        let mut state = loaded();
        state.apply(&Command::OpenResource(deployment("b")));
        assert_eq!(state.location().cluster, Some(ClusterId::new("b")));
        assert_eq!(state.location().page, Page::Deployments);
        assert_eq!(state.location().resource, Some(deployment("b")));
        state.apply(&Command::SelectPage(Page::Pods));
        assert_eq!(state.location().resource, None);
    }

    #[test]
    fn opening_a_namespace_or_config_object_shows_its_detail_page() {
        let mut state = loaded();
        let namespace = ResourceRef {
            cluster: ClusterId::new("a"),
            kind: "Namespace".into(),
            namespace: None,
            name: "payments".into(),
        };
        state.apply(&Command::OpenResource(namespace.clone()));
        assert_eq!(state.location().page, Page::Namespaces);
        assert_eq!(state.location().detail(), Some(&namespace));
        assert_eq!(state.workload(), None);
        let config = ResourceRef {
            kind: "ConfigMap".into(),
            namespace: Some("payments".into()),
            name: "checkout-api-config".into(),
            ..namespace
        };
        state.apply(&Command::OpenResource(config.clone()));
        assert_eq!(state.location().page, Page::ConfigMaps);
        assert_eq!(state.location().detail(), Some(&config));
        assert!(state.back());
        assert_eq!(state.location().page, Page::Namespaces);
        state.apply(&Command::EditYaml(config));
        assert_eq!(state.location().detail(), None);
        state.apply(&Command::OpenResource(deployment("a")));
        assert_eq!(state.location().detail(), None);
        assert!(state.workload().is_some());
    }

    #[test]
    fn inspecting_a_listed_workload_stays_on_the_list() {
        let mut state = loaded();
        state.apply(&Command::SelectCluster(Some(ClusterId::new("b"))));
        state.apply(&Command::SelectPage(Page::Deployments));
        let list = state.location().clone();
        let back = state
            .panes()
            .active()
            .history()
            .entries(crate::history::Direction::Back)
            .len();
        state.apply(&Command::InspectResource(deployment("b")));
        let inspecting = Location {
            inspecting: Some(deployment("b")),
            ..list.clone()
        };
        assert_eq!(state.location(), &inspecting);
        let history = state.panes().active().history();
        assert_eq!(history.entries(crate::history::Direction::Back).len(), back);
        assert_eq!(history.current(), Some(&inspecting));
        state.apply(&Command::InspectResource(deployment("a")));
        assert_eq!(state.location(), &inspecting);
        state.apply(&Command::OpenResource(deployment("b")));
        assert_eq!(state.location().resource, Some(deployment("b")));
        assert_eq!(state.location().inspecting, None);
        assert!(state.back());
        assert_eq!(state.location(), &inspecting);
        state.apply(&Command::CloseInspector);
        assert_eq!(state.location(), &list);
        assert!(state.forward());
        assert!(state.back());
        assert_eq!(state.location(), &list);
    }

    #[test]
    fn opening_a_commit_shows_changes_for_the_flock() {
        let mut state = loaded();
        state.apply(&Command::SelectCluster(Some(ClusterId::new("a"))));
        state.apply(&Command::OpenCommit("a41e07c".into()));
        assert_eq!(state.location().cluster, None);
        assert_eq!(state.location().page, Page::Changes);
    }

    #[test]
    fn opening_a_resource_selects_its_cluster_and_list_page() {
        let mut state = loaded();
        let pod = ResourceRef {
            cluster: ClusterId::new("b"),
            kind: "Pod".into(),
            namespace: Some("shop".into()),
            name: "cart-7f9c".into(),
        };
        state.apply(&Command::OpenResource(pod.clone()));
        assert_eq!(state.location().cluster, Some(ClusterId::new("b")));
        assert_eq!(state.location().page, Page::Pods);
        assert_eq!(state.location().resource, Some(pod));
        state.apply(&Command::SelectPage(Page::Nodes));
        assert_eq!(state.location().resource, None);
    }

    #[test]
    fn opening_a_resource_on_an_unknown_cluster_is_ignored() {
        let mut state = loaded();
        let ghost = ResourceRef {
            cluster: ClusterId::new("nope"),
            kind: "Pod".into(),
            namespace: None,
            name: "x".into(),
        };
        state.apply(&Command::FollowLogs(LogTarget::Workload(ghost)));
        assert_eq!(state, loaded());
    }

    #[test]
    fn following_a_workloads_logs_opens_a_merged_logs_tab_and_retargets_it() {
        let mut state = loaded();
        let deploy = ResourceRef {
            cluster: ClusterId::new("a"),
            kind: "Deployment".into(),
            namespace: Some("shop".into()),
            name: "cart".into(),
        };
        let target = LogTarget::Workload(deploy);
        state.apply(&Command::NewTab(Box::new(Command::FollowLogs(
            target.clone(),
        ))));
        let location = state.location().clone();
        assert_eq!(location.page, Page::Logs);
        assert_eq!(location.logs, Some(target));
        assert_eq!(location.workload(), None);
        let title = crate::tabs::title(&location, &state.snapshot);
        assert_eq!(
            (title.kind.as_str(), title.label.as_str()),
            (crate::tabs::LOGS_KIND, "cart")
        );
        state.retarget(Some(&ClusterId::new("b")));
        assert_eq!(state.location().page, Page::ClusterOverview);
        assert_eq!(state.location().logs, None);
        let Ok(selector) = crate::multilog::LabelSelector::parse("app=cart") else {
            panic!("selector parses");
        };
        let selected = LogTarget::Selector {
            cluster: ClusterId::new("a"),
            namespace: None,
            selector,
        };
        state.apply(&Command::FollowLogs(selected.clone()));
        state.retarget(Some(&ClusterId::new("b")));
        assert_eq!(
            state.location().logs,
            Some(selected.on(&ClusterId::new("b")))
        );
    }

    #[test]
    fn compare_opens_the_compare_page_for_the_resource() {
        let mut state = loaded();
        let deploy = ResourceRef {
            cluster: ClusterId::new("a"),
            kind: "Deployment".into(),
            namespace: Some("shop".into()),
            name: "cart".into(),
        };
        state.apply(&Command::Compare(deploy.clone()));
        assert_eq!(state.location().page, Page::Compare);
        assert_eq!(state.location().cluster, Some(ClusterId::new("a")));
        assert_eq!(state.location().resource, Some(deploy));
    }

    #[test]
    fn comparing_a_workload_shows_compare_not_the_workload_page() {
        let mut state = loaded();
        state.apply(&Command::OpenResource(deployment("a")));
        assert_eq!(state.workload(), Some(&deployment("a")));
        state.apply(&Command::Compare(deployment("a")));
        assert_eq!(state.location().page, Page::Compare);
        assert_eq!(state.workload(), None);
        state.apply(&Command::CompareWith {
            base: deployment("b"),
            other: ClusterId::new("a"),
        });
        assert_eq!(state.location().resource, Some(deployment("b")));
        assert_eq!(state.workload(), None);
    }

    #[test]
    fn the_editor_tab_goes_back_to_the_page_it_was_opened_from() {
        let mut state = loaded();
        state.apply(&Command::OpenResource(deployment("a")));
        let origin = state.location().clone();
        state.open_tab(&Command::EditYaml(deployment("a")));
        assert_eq!(state.location().page, Page::EditYaml);
        assert!(state.back());
        assert_eq!(state.location(), &origin);
        assert!(state.forward());
        assert_eq!(state.location().page, Page::EditYaml);
    }

    #[test]
    fn edit_yaml_opens_an_editor_tab_that_reopens_itself() {
        let mut state = loaded();
        state.apply(&Command::OpenResource(deployment("a")));
        let opened = state.open_tab(&Command::EditYaml(deployment("a")));
        assert!(opened.is_some());
        let location = state.location().clone();
        assert_eq!(location.page, Page::EditYaml);
        assert_eq!(location.resource, Some(deployment("a")));
        assert_eq!(state.workload(), None);
        assert_eq!(
            location.commands(),
            vec![Command::EditYaml(deployment("a"))]
        );
        assert!(
            location
                .history_label(&state.snapshot)
                .starts_with("Edit · ")
        );
        let title = crate::tabs::title(&location, &state.snapshot);
        assert_eq!(title.kind, crate::tabs::EDIT_KIND);
        assert!(!state.ready_for(&Command::EditYaml(deployment("gone"))));
    }

    fn with_crd(clusters: &[&str]) -> (AppState, CrdKey) {
        let key = CrdKey::new("cert-manager.io", "Certificate");
        let mut snapshot = sample();
        for cluster in clusters {
            let crd = crate::crd::Crd {
                key: key.clone(),
                plural: "certificates".into(),
                version: "v1".into(),
                namespaced: true,
                columns: Vec::new(),
            };
            snapshot.crds.insert(ClusterId::new(*cluster), vec![crd]);
        }
        let mut state = AppState::default();
        state.update(Update::Snapshot(Box::new(snapshot)));
        (state, key)
    }

    #[test]
    fn a_crd_opens_its_object_list_restores_and_follows_the_cluster_that_has_it() {
        let (mut state, key) = with_crd(&["a", "b"]);
        let open = Command::OpenCrd {
            cluster: ClusterId::new("a"),
            crd: key.clone(),
        };
        assert!(state.ready_for(&open));
        state.apply(&open);
        let location = state.location().clone();
        assert_eq!(location.page, Page::CustomResources);
        assert_eq!(location.crd, Some(key.clone()));
        assert_eq!(location.commands(), vec![open]);
        let title = crate::tabs::title(&location, &state.snapshot);
        assert_eq!(
            (title.kind.as_str(), title.label.as_str()),
            (crate::tabs::CRD_KIND, "Certificate")
        );
        let object = key.object(ClusterId::new("a"), Some("web".into()), "tls".into());
        state.apply(&Command::OpenResource(object.clone()));
        assert_eq!(state.location().page, Page::CustomResources);
        assert_eq!(state.location().crd, Some(key.clone()));
        assert_eq!(state.location().resource, Some(object.clone()));
        assert_eq!(state.workload(), None);
        assert_eq!(state.location().detail(), None);
        let title = crate::tabs::title(state.location(), &state.snapshot);
        assert_eq!(title.kind, "Certificate");
        state.retarget(Some(&ClusterId::new("b")));
        assert_eq!(
            state.location(),
            &Location::crd_on(ClusterId::new("b"), key.clone())
        );
        state.retarget(Some(&ClusterId::new("c")));
        assert_eq!(state.location().page, Page::ClusterOverview);
        assert_eq!(state.location().crd, None);
    }

    #[test]
    fn a_helm_release_opens_its_page_restores_and_retargets_to_the_list() {
        let mut state = loaded();
        let key = HelmKey::new(ClusterId::new("a"), "payments", "checkout-api");
        let open = Command::OpenHelmRelease(key.clone());
        assert!(state.ready_for(&open));
        assert!(!state.ready_for(&Command::OpenHelmRelease(key.on(&ClusterId::new("x")))));
        state.apply(&Command::NewTab(Box::new(open.clone())));
        let location = state.location().clone();
        assert_eq!(location.page, Page::Releases);
        assert_eq!(location.helm, Some(key.clone()));
        assert_eq!(location.commands(), vec![open]);
        assert!(
            location
                .history_label(&state.snapshot)
                .starts_with("payments/checkout-api")
        );
        let title = crate::tabs::title(&location, &state.snapshot);
        assert_eq!(
            (title.kind.as_str(), title.label.as_str()),
            (crate::tabs::HELM_KIND, "checkout-api")
        );
        state.retarget(Some(&ClusterId::new("b")));
        assert_eq!(
            state.location(),
            &Location::page_on(ClusterId::new("b"), Page::Releases)
        );
        state.apply(&Command::SelectPage(Page::Releases));
        assert_eq!(state.location().helm, None);
    }

    #[test]
    fn cluster_settings_is_a_flock_page() {
        let mut state = loaded();
        state.apply(&Command::SelectCluster(Some(ClusterId::new("a"))));
        state.apply(&Command::ShowClusterSettings(ClusterId::new("b")));
        assert_eq!(state.location().page, Page::ClusterSettings);
        assert_eq!(state.location().cluster, None);
    }

    #[test]
    fn toggle_flips_the_sidebar_and_keeps_the_selection() {
        let mut state = loaded();
        state.apply(&Command::SelectCluster(Some(ClusterId::new("a"))));
        state.apply(&Command::ToggleSidebar);
        assert!(!state.sidebar_open);
        assert_eq!(state.location().cluster, Some(ClusterId::new("a")));
        state.apply(&Command::ToggleSidebar);
        assert!(state.sidebar_open);
    }

    #[test]
    fn switching_clusters_keeps_the_sidebar_and_leaves_the_flock_home() {
        let mut state = loaded();
        state.apply(&Command::ToggleSidebar);
        state.apply(&Command::SelectCluster(Some(ClusterId::new("b"))));
        state.apply(&Command::SelectCluster(Some(ClusterId::new("c"))));
        assert_eq!(state.location().cluster, Some(ClusterId::new("c")));
        assert_eq!(state.location().page, Page::ClusterOverview);
        assert!(!state.sidebar_open);
        state.apply(&Command::SelectCluster(None));
        assert_eq!(state.location().cluster, None);
        assert_eq!(state.location().page, Page::Overview);
    }

    fn with_flux() -> (AppState, FluxKey) {
        let mut snapshot = sample();
        let object = crate::flux::tests::object("b", crate::flux::Phase::Ready, "a41e07c");
        let key = object.key.clone();
        snapshot.flux = vec![object];
        let mut state = AppState::default();
        state.update(Update::Snapshot(Box::new(snapshot)));
        (state, key)
    }

    #[test]
    fn opening_a_flux_object_selects_its_cluster_and_nav_page() {
        let (mut state, key) = with_flux();
        state.apply(&Command::SelectCluster(Some(ClusterId::new("a"))));
        state.apply(&Command::OpenFluxObject(key.clone()));
        assert_eq!(state.location().cluster, Some(ClusterId::new("b")));
        assert_eq!(state.location().page, Page::Kustomizations);
        assert_eq!(state.location().flux_object, Some(key));
        state.apply(&Command::SelectPage(Page::Kustomizations));
        assert_eq!(state.location().flux_object, None);
    }

    #[test]
    fn opening_a_flux_resource_shows_its_object_page() {
        let (mut state, key) = with_flux();
        let target = ResourceRef {
            cluster: key.cluster.clone(),
            kind: "Kustomization".into(),
            namespace: Some(key.namespace.clone()),
            name: key.name.clone(),
        };
        state.apply(&Command::OpenResource(target.clone()));
        assert_eq!(state.location().flux_object, Some(key));
        state.apply(&Command::OpenResource(ResourceRef {
            kind: "Deployment".into(),
            ..target
        }));
        assert_eq!(state.location().flux_object, None);
        assert_eq!(state.location().page, Page::Deployments);
    }

    #[test]
    fn opening_an_unknown_flux_object_is_ignored() {
        let (mut state, key) = with_flux();
        let before = state.clone();
        state.apply(&Command::OpenFluxObject(key.on(&ClusterId::new("a"))));
        assert_eq!(state, before);
    }

    #[test]
    fn flux_object_selection_clears_when_it_disappears() {
        let (mut state, key) = with_flux();
        state.apply(&Command::OpenFluxObject(key));
        state.update(Update::Snapshot(Box::new(sample())));
        assert_eq!(state.location().flux_object, None);
        assert_eq!(state.location().page, Page::Kustomizations);
    }

    #[test]
    fn startup_commands_wait_for_what_they_name() {
        let (state, key) = with_flux();
        assert!(state.ready_for(&Command::SelectCluster(Some(ClusterId::new("a")))));
        assert!(!state.ready_for(&Command::SelectCluster(Some(ClusterId::new("nope")))));
        assert!(state.ready_for(&Command::OpenFluxObject(key.clone())));
        assert!(!state.ready_for(&Command::OpenFluxObject(key.on(&ClusterId::new("a")))));
        assert!(!state.ready_for(&Command::OpenResource(deployment("nope"))));
        assert!(state.ready_for(&Command::Compare(deployment("b"))));
        assert!(state.ready_for(&Command::SelectPage(Page::GitOps)));
        assert!(!AppState::default().ready_for(&Command::ShowClusterSettings(ClusterId::new("a"))));
    }

    #[test]
    fn a_tab_keeps_a_cluster_that_left_and_reports_it_unavailable() {
        let mut state = loaded();
        state.apply(&Command::SelectCluster(Some(ClusterId::new("a"))));
        state.apply(&Command::SelectPage(Page::Nodes));
        assert!(state.cluster_available());
        state.update(Update::Snapshot(Box::default()));
        assert_eq!(state.location().cluster, Some(ClusterId::new("a")));
        assert_eq!(state.location().page, Page::Nodes);
        assert!(!state.cluster_available());
        state.update(Update::Snapshot(Box::new(sample())));
        assert!(state.cluster_available());
    }

    fn summary(resource: ResourceRef) -> crate::ResourceSummary {
        crate::ResourceSummary {
            resource,
            status: None,
            detail: String::new(),
            labels: std::collections::BTreeMap::new(),
            ports: Vec::new(),
            owner: None,
        }
    }

    #[test]
    fn each_tab_keeps_its_own_location_and_history() {
        let mut state = loaded();
        state.apply(&Command::SelectCluster(Some(ClusterId::new("a"))));
        state.apply(&Command::SelectPage(Page::Nodes));
        let first = state.panes().active().id();
        let second = state.new_tab();
        assert_eq!(state.location(), &Location::default());
        assert!(!state.panes().active().can_go_back());
        state.apply(&Command::SelectPage(Page::GitOps));
        assert!(state.panes_mut().activate(first));
        assert_eq!(state.location().page, Page::Nodes);
        assert!(state.back());
        assert_eq!(
            state.location(),
            &Location::page_on(ClusterId::new("a"), Page::ClusterOverview)
        );
        assert!(state.panes_mut().activate(second));
        assert!(state.back());
        assert_eq!(state.location(), &Location::default());
        assert!(!state.back());
        assert!(state.forward());
        assert_eq!(state.location().page, Page::GitOps);
        assert!(state.panes_mut().activate(first));
        assert!(state.forward());
        assert_eq!(state.location().page, Page::Nodes);
    }

    #[test]
    fn new_tab_commands_open_beside_the_active_tab_and_dedupe() {
        let mut state = loaded();
        state.apply(&Command::SelectCluster(Some(ClusterId::new("a"))));
        let pods = Command::NewTab(Box::new(Command::SelectPage(Page::Pods)));
        state.apply(&pods);
        assert_eq!(state.panes().focused().tabs().len(), 2);
        assert_eq!(
            state.location(),
            &Location::page_on(ClusterId::new("a"), Page::Pods)
        );
        let pods_tab = state.panes().active().id();
        assert!(state.panes_mut().activate_nth(0));
        state.apply(&pods);
        assert_eq!(state.panes().focused().tabs().len(), 2);
        assert_eq!(state.panes().active().id(), pods_tab);
        state.apply(&Command::NewTab(Box::new(Command::RestartRollout(
            Seen::new(deployment("a"), Uid::new("u1")),
        ))));
        assert_eq!(state.panes().focused().tabs().len(), 2);
        assert_eq!(state.open_tab(&Command::ToggleSidebar), None);
        let flock = state.new_tab();
        assert_ne!(state.new_tab(), flock);
        assert_eq!(state.panes().focused().tabs().len(), 4);
    }

    #[test]
    fn log_tabs_carry_the_pod_and_container() {
        let mut state = loaded();
        let pod = ResourceRef {
            kind: "Pod".into(),
            name: "checkout-api-7f9c6d-x2lqp".into(),
            ..deployment("a")
        };
        let open = Command::OpenLogs {
            pod: pod.clone(),
            container: "api".into(),
        };
        assert!(state.ready_for(&open));
        state.apply(&Command::NewTab(Box::new(open)));
        let location = state.location();
        assert_eq!(location.page, Page::Logs);
        assert_eq!(location.resource, Some(pod));
        assert_eq!(location.container.as_deref(), Some("api"));
        assert_eq!(state.workload(), None);
    }

    #[test]
    fn retargeting_keeps_the_page_or_object_when_the_new_cluster_has_it() {
        let mut snapshot = sample();
        snapshot.resources = vec![summary(deployment("a")), summary(deployment("b"))];
        let mut state = AppState::default();
        state.update(Update::Snapshot(Box::new(snapshot)));
        state.apply(&Command::SelectCluster(Some(ClusterId::new("a"))));
        state.apply(&Command::SelectPage(Page::Events));
        state.retarget(Some(&ClusterId::new("b")));
        assert_eq!(
            state.location(),
            &Location::page_on(ClusterId::new("b"), Page::Events)
        );
        state.apply(&Command::OpenResource(deployment("a")));
        state.retarget(Some(&ClusterId::new("b")));
        assert_eq!(state.location().resource, Some(deployment("b")));
        assert_eq!(state.location().page, Page::Deployments);
        state.retarget(Some(&ClusterId::new("c")));
        assert_eq!(
            state.location(),
            &Location::page_on(ClusterId::new("c"), Page::ClusterOverview)
        );
        state.retarget(Some(&ClusterId::new("nope")));
        assert_eq!(state.location().cluster, Some(ClusterId::new("c")));
        state.retarget(None);
        assert_eq!(state.location(), &Location::default());
        state.retarget(Some(&ClusterId::new("a")));
        assert_eq!(
            state.location(),
            &Location::page_on(ClusterId::new("a"), Page::ClusterOverview)
        );
        assert!(state.panes().active().can_go_back());
    }

    #[test]
    fn retargeting_a_flux_object_falls_back_to_the_overview_when_it_is_missing() {
        let (mut state, key) = with_flux();
        state.apply(&Command::OpenFluxObject(key.clone()));
        state.retarget(Some(&ClusterId::new("a")));
        assert_eq!(
            state.location(),
            &Location::page_on(ClusterId::new("a"), Page::ClusterOverview)
        );
        state.retarget(Some(&key.cluster));
        state.apply(&Command::OpenFluxObject(key.clone()));
        state.retarget(Some(&key.cluster));
        assert_eq!(state.location().flux_object, Some(key));
    }

    #[test]
    fn restored_tabs_wait_for_objects_that_have_not_loaded_yet() {
        let (_, key) = with_flux();
        let flux = Location {
            flux_object: Some(key.clone()),
            ..Location::page_on(key.cluster.clone(), Page::Kustomizations)
        };
        let saved = SavedWindow {
            panes: vec![crate::tabs::SavedPane {
                tabs: vec![Location::default(), flux.clone()],
                active: 1,
            }],
            focused: 0,
            ..SavedWindow::default()
        };
        let mut state = AppState::default();
        state.restore(&saved);
        state.update(Update::Snapshot(Box::default()));
        assert_eq!(state.location(), &flux);
        assert!(!state.cluster_available());
        state.update(Update::Snapshot(Box::new(sample())));
        assert_eq!(state.location(), &flux);
        assert_eq!(state.panes().saved(), saved);
    }
}
