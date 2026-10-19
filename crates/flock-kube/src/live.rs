use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use flock_core::add_cluster::home_relative;
use flock_core::app_settings::DEFAULT_SNAPSHOTS;
use flock_core::home::problems_badge;
use flock_core::resource::page_for_kind;
use flock_core::settings::{Auth, ClusterSettings, Connection, ContextChange};
use flock_core::stream::Sink;
use flock_core::{
    Backend, ChangesLog, CheckLine, ClusterCatalog, ClusterId, ClusterSummary, Command, Comparison,
    ContextChoice, Counts, FlockHome, GitOps, KubeconfigFile, NewCluster, Notice, Page,
    Preferences, Receiver, ResourceRef, Seen, Sender, Snapshot, Status, Update, Verdict,
    WatchState, Workload, WorkloadKind,
};
use jiff::Timestamp;
use k8s_openapi::api::core::v1::Node;
use tokio::time::MissedTickBehavior;

use crate::actions::pending::Mutation;
use crate::actions::{Actions, cluster_of};
use crate::assemble::{self, Extras, Prometheus, Selection};
use crate::checks::{self, CheckResult, run_checks_streaming};
use crate::discovery::{
    ContextRef, Discovery, DiscoveryEvent, DiscoverySources, KubeContext, discover,
};
use crate::gitops::GitOpsState;
use crate::inspect;
use crate::manager::{ClusterEvent, ClusterHandle, ClusterManager, ConnectionStatus, Subscription};
use crate::metrics::{NodeUsage, PodUsage};
use crate::metrics_history;
use crate::paste;
use crate::probe::find_prometheus;
use crate::problems::order;
use crate::provider::AuthKind;
use crate::settings::KubeSettings;
use crate::shell_env::{ShellEnv, capture, user_shell};
use crate::snapshot::{BACKGROUND_KINDS, page_kinds};
use crate::store::{ClusterStore, Pin, SavedCluster, SavedPreferences};
use crate::task::TaskGuard;
use crate::throttle::Throttle;
use crate::usage::prometheus_utilization;
use crate::watch::KubeconfigWatcher;
use flock_core::settings::MetricsSource;

const THREAD_NAME: &str = "flock-kube";
pub const SNAPSHOT_INTERVAL: Duration = DEFAULT_SNAPSHOTS.duration();
const EXTENSION_POLL_INTERVAL: Duration = SNAPSHOT_INTERVAL;
const DEFAULT_WINDOW: u64 = 0;
const MISSING_STATE: &str = "Not in kubeconfig";
const CHANGED_STATE: &str = "Context changed";
const DISCONNECTED_STATE: &str = "Disconnected";
const SET_ASIDE_TITLE: &str = "Saved clusters could not be read";
const MISSING_CONTEXT: &str = "This context is no longer in its kubeconfig file.";

pub struct LiveBackend {
    settings: KubeSettings,
    sources: DiscoverySources,
    store: ClusterStore,
}

impl LiveBackend {
    pub fn new(settings: KubeSettings, mut sources: DiscoverySources, store: ClusterStore) -> Self {
        sources.folders = scan_folders(&store);
        Self {
            settings,
            sources,
            store,
        }
    }
}

impl Backend for LiveBackend {
    fn start(self: Box<Self>, updates: Sender<Update>, commands: Receiver<Command>) {
        let spawned = thread::Builder::new()
            .name(THREAD_NAME.into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .thread_name(THREAD_NAME)
                    .build();
                match runtime {
                    Ok(runtime) => runtime.block_on(run(*self, updates, commands)),
                    Err(error) => tracing::error!(%error, "live backend runtime failed to start"),
                }
            });
        if let Err(error) = spawned {
            tracing::error!(%error, "live backend thread failed to start");
        }
    }
}

enum Fetched {
    NodeUsage(ContextRef, Option<Vec<NodeUsage>>),
    PodUsage(ContextRef, Option<Vec<PodUsage>>),
    Prometheus(ContextRef, Option<Prometheus>),
    Yaml(u64, ResourceRef, inspect::Inspected),
    Comparison(u64, Box<Comparison>),
}

struct Connected {
    id: ClusterId,
    handle: ClusterHandle,
    _background: Vec<Subscription>,
    extras: Extras,
    usage_task: Option<TaskGuard>,
    pod_usage_task: Option<TaskGuard>,
    prometheus_task: Option<TaskGuard>,
}

impl Connected {
    fn watching(id: ClusterId, handle: ClusterHandle) -> Self {
        let background = BACKGROUND_KINDS
            .iter()
            .map(|kind| handle.subscribe(kind.clone()))
            .collect();
        Self {
            id,
            handle,
            _background: background,
            extras: Extras::default(),
            usage_task: None,
            pod_usage_task: None,
            prometheus_task: None,
        }
    }
}

struct Inspecting {
    target: ResourceRef,
    shown: Option<String>,
}

struct Navigation {
    selected: Option<ClusterId>,
    page: Page,
    yaml: Option<(ResourceRef, String)>,
    inspecting: Option<Inspecting>,
    yaml_task: Option<TaskGuard>,
    workload_task: Option<TaskGuard>,
    comparing: Option<Comparing>,
    comparison: Option<Comparison>,
    #[cfg(feature = "loco")]
    loco: crate::loco::LocoBackend,
}

struct Comparing {
    target: ResourceRef,
    _task: TaskGuard,
}

impl Navigation {
    fn new(updates: &Sender<Update>) -> Self {
        #[cfg(not(feature = "loco"))]
        let _ = updates;
        Self {
            selected: None,
            page: Page::Overview,
            yaml: None,
            inspecting: None,
            yaml_task: None,
            workload_task: None,
            comparing: None,
            comparison: None,
            #[cfg(feature = "loco")]
            loco: crate::loco::LocoBackend::new(
                crate::loco::LocoSettings::default(),
                updates.clone(),
            ),
        }
    }
}

struct Live {
    window_id: u64,
    navigation: Navigation,
    windows: BTreeMap<u64, Navigation>,
    manager: ClusterManager,
    store: ClusterStore,
    discovery: Discovery,
    sources: DiscoverySources,
    sources_changed: bool,
    env: Arc<ShellEnv>,
    settings: KubeSettings,
    updates: Sender<Update>,
    home: Option<PathBuf>,
    clusters: BTreeMap<ContextRef, Connected>,
    disconnected: BTreeSet<ClusterId>,
    actions: Actions,
    gitops: GitOpsState,
    fetched: Sender<Fetched>,
}

fn reachable(status: &ConnectionStatus) -> bool {
    matches!(
        status,
        ConnectionStatus::Connected | ConnectionStatus::Degraded(_)
    )
}

impl Live {
    fn for_each_window(&mut self, mut action: impl FnMut(&mut Self)) {
        action(self);
        let windows = std::mem::take(&mut self.windows);
        for (id, mut navigation) in windows {
            std::mem::swap(&mut self.navigation, &mut navigation);
            let previous = std::mem::replace(&mut self.window_id, id);
            action(self);
            self.window_id = previous;
            std::mem::swap(&mut self.navigation, &mut navigation);
            self.windows.insert(id, navigation);
        }
    }

    fn navigations(&self) -> impl Iterator<Item = &Navigation> {
        std::iter::once(&self.navigation).chain(self.windows.values())
    }

    fn start(&mut self) {
        if let Some(notice) = set_aside_notice(&self.store) {
            self.publish(Update::Notice(notice));
        }
        self.reconcile();
    }

    fn pin_new(&mut self) {
        let unpinned: Vec<(ClusterId, Pin)> = self
            .store
            .clusters()
            .iter()
            .filter(|saved| saved.pin.is_none())
            .filter_map(|saved| {
                let context = self.discovery.get(&saved.context_ref())?;
                Some((saved.id(), Pin::of(context)))
            })
            .collect();
        for (id, pin) in unpinned {
            self.save_pin(&id, pin);
        }
    }

    fn save_pin(&mut self, id: &ClusterId, pin: Pin) {
        if let Err(error) = self.store.pin(id, pin) {
            tracing::error!(%error, path = %self.store.path().display(), "saving the cluster's server failed");
        }
    }

    fn confirm_context(&mut self, id: &ClusterId) {
        let context = self
            .store
            .get(id)
            .and_then(|saved| self.discovery.get(&saved.context_ref()));
        if let Some(pin) = context.map(Pin::of) {
            self.save_pin(id, pin);
        }
        self.reconcile();
    }

    fn reconcile(&mut self) {
        self.pin_new();
        let ids = wanted(&self.store, &self.discovery, &self.disconnected);
        let wanted: BTreeSet<&ContextRef> = ids.keys().collect();
        let gone: Vec<ContextRef> = self
            .clusters
            .keys()
            .filter(|id| !wanted.contains(id))
            .cloned()
            .collect();
        for id in gone {
            if let Some(cluster) = self.clusters.remove(&id) {
                self.save_history(&cluster);
            }
            self.manager.disconnect(&id);
        }
        for (context_ref, id) in ids {
            let Some(context) = self.discovery.get(&context_ref).cloned() else {
                continue;
            };
            let context = &context;
            let unchanged = self
                .clusters
                .get(&context.id)
                .is_some_and(|existing| existing.handle.context() == context && existing.id == id);
            if unchanged {
                continue;
            }
            self.manager.disconnect(&context.id);
            let handle = self.manager.connect(context.clone());
            if let Some(cluster) = self.clusters.remove(&context.id) {
                self.save_history(&cluster);
            }
            let mut cluster = Connected::watching(id, handle);
            if let Some(path) = self.history_path(&cluster.id) {
                cluster.extras.ring = metrics_history::load(&path, Timestamp::now());
            }
            self.clusters.insert(context.id.clone(), cluster);
        }
        self.gitops
            .sync(self.clusters.values().map(|c| (&c.id, &c.handle)));
        self.for_each_window(Self::page_changed);
    }

    fn history_path(&self, id: &ClusterId) -> Option<PathBuf> {
        let home = self.home.as_ref()?;
        Some(metrics_history::path(&metrics_history::cache_dir(home), id))
    }

    fn save_history(&self, cluster: &Connected) {
        let Some(path) = self.history_path(&cluster.id) else {
            return;
        };
        if let Err(error) = metrics_history::save(&path, &cluster.extras.ring) {
            tracing::warn!(%error, path = %path.display(), "saving metrics history failed");
        }
    }

    fn save_histories(&self) {
        for cluster in self.clusters.values() {
            self.save_history(cluster);
        }
    }

    fn selected_key(&self) -> Option<ContextRef> {
        let selected = self.navigation.selected.as_ref()?;
        self.clusters
            .iter()
            .find(|(_, cluster)| &cluster.id == selected)
            .map(|(key, _)| key.clone())
    }

    fn page_changed(&mut self) {
        self.poll_page();
        #[cfg(feature = "loco")]
        self.navigation.loco.sync(
            self.clusters.values().map(|c| (&c.id, &c.handle)),
            self.navigation.selected.as_ref(),
            self.navigation.page,
        );
    }

    fn poll_page(&mut self) {
        match self.navigation.page {
            Page::ClusterOverview => self.poll_prometheus(),
            Page::Namespaces => self.poll_pod_usage(),
            Page::Pods => self.poll_pod_usage(),
            _ => {}
        }
    }

    fn poll_usage(&mut self) {
        let timeout = self.settings.check_timeout;
        for (key, cluster) in &mut self.clusters {
            if !reachable(&cluster.handle.status()) {
                continue;
            }
            let handle = cluster.handle.clone();
            let fetched = self.fetched.clone();
            let key = key.clone();
            cluster.usage_task = Some(TaskGuard::spawn(async move {
                let usage = tokio::time::timeout(timeout, handle.node_usage()).await;
                let usage = usage.ok().and_then(Result::ok);
                let _ = fetched.send(Fetched::NodeUsage(key, usage)).await;
            }));
        }
        self.for_each_window(|live| {
            if matches!(live.navigation.page, Page::Namespaces | Page::Pods) {
                live.poll_pod_usage();
            }
        });
    }

    fn poll_pod_usage(&mut self) {
        let timeout = self.settings.check_timeout;
        let fetched = self.fetched.clone();
        let Some(key) = self.selected_key() else {
            return;
        };
        let Some(cluster) = self.clusters.get_mut(&key) else {
            return;
        };
        let handle = cluster.handle.clone();
        cluster.pod_usage_task = Some(TaskGuard::spawn(async move {
            let usage = tokio::time::timeout(timeout, handle.pod_usage(None)).await;
            let usage = usage.ok().and_then(Result::ok);
            let _ = fetched.send(Fetched::PodUsage(key, usage)).await;
        }));
    }

    fn poll_prometheus(&mut self) {
        let timeout = self.settings.check_timeout;
        let fetched = self.fetched.clone();
        let Some(key) = self.selected_key() else {
            return;
        };
        let Some(cluster) = self.clusters.get_mut(&key) else {
            return;
        };
        let source = self
            .store
            .get(&cluster.id)
            .map(|saved| Preferences::from(&saved.preferences).metrics)
            .unwrap_or_default();
        if source == MetricsSource::MetricsServer {
            cluster.extras.prometheus = None;
            return;
        }
        let Ok(client) = cluster.handle.client() else {
            return;
        };
        let handle = cluster.handle.clone();
        let known = cluster
            .extras
            .prometheus
            .as_ref()
            .and_then(|p| p.target.clone());
        cluster.prometheus_task = Some(TaskGuard::spawn(async move {
            let target = match known {
                Some(target) => Some(target),
                None => find_prometheus(&client).await,
            };
            let found = match target {
                Some(target) => {
                    let ranges = prometheus_utilization(&handle, &target, Timestamp::now());
                    let utilization = tokio::time::timeout(timeout, ranges).await;
                    let (utilization, error) = match utilization {
                        Ok(Ok(ranges)) => (ranges, None),
                        Ok(Err(error)) => (Vec::new(), Some(error.to_string())),
                        Err(_) => (
                            Vec::new(),
                            Some(format!("request timed out after {timeout:?}")),
                        ),
                    };
                    Some(Prometheus {
                        target: Some(target),
                        utilization,
                        error,
                    })
                }
                None if source == MetricsSource::Prometheus => Some(Prometheus {
                    target: None,
                    utilization: Vec::new(),
                    error: Some(
                        "No Prometheus service found; check the cluster metrics source".into(),
                    ),
                }),
                None => None,
            };
            let _ = fetched.send(Fetched::Prometheus(key, found)).await;
        }));
    }

    fn cluster_event(&mut self, event: &ClusterEvent) {
        if let ClusterEvent::Changed { id, .. } = event {
            self.for_each_window(|live| live.refresh_yaml(id));
        }
    }

    fn fetched(&mut self, fetched: Fetched) {
        match fetched {
            Fetched::NodeUsage(key, usage) => {
                if let Some(cluster) = self.clusters.get_mut(&key) {
                    let nodes = cluster.handle.objects::<Node>();
                    cluster.extras.record(&nodes, usage);
                }
            }
            Fetched::PodUsage(key, usage) => {
                if let Some(cluster) = self.clusters.get_mut(&key) {
                    cluster.extras.pod_usage = usage;
                }
            }
            Fetched::Prometheus(key, found) => {
                if let Some(cluster) = self.clusters.get_mut(&key) {
                    cluster.extras.prometheus = found;
                }
            }
            Fetched::Comparison(window, comparison) => {
                let navigation = if window == self.window_id {
                    Some(&mut self.navigation)
                } else {
                    self.windows.get_mut(&window)
                };
                if let Some(navigation) = navigation
                    && navigation
                        .comparing
                        .as_ref()
                        .is_some_and(|comparing| comparison.matches(&comparing.target))
                {
                    navigation.comparison = Some(*comparison);
                }
            }
            Fetched::Yaml(window, target, inspected) => {
                let navigation = if window == self.window_id {
                    Some(&mut self.navigation)
                } else {
                    self.windows.get_mut(&window)
                };
                if let Some(navigation) = navigation
                    && let Some(inspecting) = navigation.inspecting.as_mut()
                    && inspecting.target == target
                {
                    inspecting.shown = inspected.version;
                    navigation.yaml = Some((target, inspected.yaml));
                }
            }
        }
    }

    fn compare(&mut self, target: &ResourceRef, other: Option<&ClusterId>) {
        let Some(kind) = WorkloadKind::from_kind(&target.kind) else {
            return;
        };
        self.navigation.selected = Some(target.cluster.clone());
        self.navigation.page = Page::Compare;
        self.page_changed();
        let same = self
            .navigation
            .comparing
            .as_ref()
            .is_some_and(|comparing| &comparing.target == target);
        if !same {
            self.navigation.comparison = None;
        }
        let clusters: Vec<(ClusterId, ClusterHandle)> = self
            .clusters
            .values()
            .map(|cluster| (cluster.id.clone(), cluster.handle.clone()))
            .collect();
        let fetched = self.fetched.clone();
        let window = self.window_id;
        let refresh = self.settings.workload_refresh;
        let task_target = target.clone();
        let other = other.cloned();
        let task = TaskGuard::spawn(async move {
            let _subscriptions: Vec<Subscription> = clusters
                .iter()
                .flat_map(|(_, handle)| {
                    crate::workload::watch_kinds(kind)
                        .into_iter()
                        .map(|kind| handle.subscribe(kind))
                })
                .collect();
            loop {
                let built = crate::compare::compare(
                    &clusters,
                    &task_target,
                    other.as_ref(),
                    Timestamp::now(),
                )
                .await;
                if let Some(comparison) = built {
                    let sent = fetched
                        .send(Fetched::Comparison(window, Box::new(comparison)))
                        .await;
                    if sent.is_err() {
                        return;
                    }
                }
                tokio::time::sleep(refresh).await;
            }
        });
        self.navigation.comparing = Some(Comparing {
            target: target.clone(),
            _task: task,
        });
    }

    fn comparisons(&self) -> Vec<Comparison> {
        self.navigations()
            .filter_map(|navigation| navigation.comparison.clone())
            .collect()
    }

    fn stop_comparing(&mut self) {
        self.navigation.comparing = None;
        self.navigation.comparison = None;
    }

    fn copy_spec(&mut self, from: &ClusterId, to: &Seen, path: &str) {
        let source = ResourceRef {
            cluster: from.clone(),
            ..to.target.clone()
        };
        let template = self
            .handle(from)
            .and_then(|handle| crate::compare::spec_of(handle, &source))
            .map(|(template, _)| template);
        let Some(template) = template else {
            self.publish(Update::Notice(Notice::failed(
                format!("Apply {from} {path} failed"),
                format!("{from} · {source} is not loaded"),
            )));
            return;
        };
        let mutation = Mutation::CopySpec {
            target: to.target.clone(),
            uid: to.uid.clone(),
            from: from.clone(),
            path: path.to_owned(),
            template,
        };
        let handle = self.handle(&to.target.cluster).cloned();
        self.actions.copy_spec(mutation, handle);
    }

    fn watch_workload(&mut self, target: &ResourceRef, sink: &Sink<Workload>) {
        self.navigation.workload_task = None;
        let Some(handle) = self.handle(&target.cluster).cloned() else {
            return;
        };
        let refresh = self.settings.workload_refresh;
        let watch = crate::workload::watch(handle.clone(), target.clone(), sink.clone(), refresh);
        self.navigation.workload_task = Some(TaskGuard::spawn(async move {
            handle.until_closed(watch).await;
        }));
    }

    fn inspect(&mut self, target: &ResourceRef) {
        self.navigation.yaml = None;
        self.navigation.yaml_task = None;
        self.navigation.inspecting = Some(Inspecting {
            target: target.clone(),
            shown: None,
        });
        self.fetch_yaml();
    }

    fn refresh_yaml(&mut self, changed: &ContextRef) {
        let Some(inspecting) = self.navigation.inspecting.as_ref() else {
            return;
        };
        let Some(cluster) = self.clusters.get(changed) else {
            return;
        };
        if cluster.id != inspecting.target.cluster {
            return;
        }
        let cached = inspect::cached_version(&cluster.handle, &inspecting.target);
        if inspect::stale(inspecting.shown.as_deref(), cached.as_deref()) {
            self.fetch_yaml();
        }
    }

    fn fetch_yaml(&mut self) {
        let Some(target) = self
            .navigation
            .inspecting
            .as_ref()
            .map(|i| i.target.clone())
        else {
            return;
        };
        let Some(handle) = self.handle(&target.cluster).cloned() else {
            return;
        };
        let fetched = self.fetched.clone();
        let window = self.window_id;
        self.navigation.yaml_task = Some(TaskGuard::spawn(async move {
            let inspected = match inspect::yaml(&handle, &target).await {
                Ok(inspected) => inspected,
                Err(error) => inspect::Inspected {
                    version: None,
                    yaml: format!("# {error}"),
                },
            };
            let _ = fetched.send(Fetched::Yaml(window, target, inspected)).await;
        }));
    }

    fn apply(&mut self, command: &Command) -> bool {
        match command {
            Command::ForWindow(id, command) => self.apply_window(*id, command),
            Command::WindowClosed(id) => {
                self.windows.remove(id);
                self.gitops.close_window(*id);
                true
            }
            _ => self.apply_current(command),
        }
    }

    fn apply_window(&mut self, id: u64, command: &Command) -> bool {
        if id == DEFAULT_WINDOW
            || matches!(command, Command::ForWindow(..) | Command::WindowClosed(_))
        {
            return false;
        }
        let mut navigation = self
            .windows
            .remove(&id)
            .unwrap_or_else(|| Navigation::new(&self.updates));
        std::mem::swap(&mut self.navigation, &mut navigation);
        let previous = std::mem::replace(&mut self.window_id, id);
        self.windows.insert(previous, navigation);
        let changed = self.apply_current(command);
        let Some(mut navigation) = self.windows.remove(&previous) else {
            unreachable!("the previous window navigation must survive a command");
        };
        self.window_id = previous;
        std::mem::swap(&mut self.navigation, &mut navigation);
        self.windows.insert(id, navigation);
        changed
    }

    fn apply_current(&mut self, command: &Command) -> bool {
        self.gitops.command(self.window_id, command);
        if leaves_compare(command) {
            self.stop_comparing();
        }
        match command {
            Command::ForWindow(..) => unreachable!(),
            Command::WindowClosed(_) => unreachable!(),
            Command::SelectCluster(id) => {
                self.navigation.selected.clone_from(id);
                self.navigation.page = if id.is_some() {
                    Page::ClusterOverview
                } else {
                    Page::Overview
                };
                self.page_changed();
            }
            Command::SelectPage(page) => {
                if page.is_flock() {
                    self.navigation.selected = None;
                }
                self.navigation.page = *page;
                self.page_changed();
            }
            Command::OpenCommit(_) => {
                self.navigation.selected = None;
                self.navigation.page = Page::Changes;
                self.page_changed();
            }
            Command::OpenFluxObject(key) => {
                self.navigation.selected = Some(key.cluster.clone());
                self.navigation.page = key.kind.page();
                self.page_changed();
            }
            Command::Flux(..) => self.act(command),
            Command::ToggleSidebar => {}
            Command::OpenResource(resource) => self.open(resource),
            Command::InspectResource(resource) => self.open(resource),
            Command::FollowLogs(_) => {}
            Command::OpenLogs { pod, .. } => self.open(pod),
            Command::NewTab(command) => return self.apply_current(command),
            Command::Compare(target) => self.compare(target, None),
            Command::CompareWith { base, other } => self.compare(base, Some(other)),
            Command::EditYaml(resource) => {
                self.navigation.selected = Some(resource.cluster.clone());
                self.navigation.page = Page::EditYaml;
                self.page_changed();
            }
            Command::Edit(_) => self.act(command),
            Command::PortForward { .. } => self.act(command),
            Command::StopPortForward { .. } => self.act(command),
            Command::Cordon(_) => self.act(command),
            Command::Uncordon(_) => self.act(command),
            Command::Drain(_) => self.act(command),
            Command::Delete(_) => self.act(command),
            Command::OpenAddCluster(_) => self.publish_contexts(),
            Command::SaveKubeconfig { name, yaml } => self.save_kubeconfig(name, yaml),
            Command::WatchFolder(folder) => self.watch_folder(folder),
            Command::CheckContexts(ids) => self.check(ids),
            Command::AddClusters(added) => {
                self.add(added);
                return true;
            }
            Command::RemoveCluster(id) => {
                self.remove(id);
                return true;
            }
            Command::WatchWorkload { target, sink } => self.watch_workload(target, sink),
            Command::StreamLogs(_) => self.act(command),
            Command::StreamPodLogs(_) => self.act(command),
            Command::Exec(_) => self.act(command),
            Command::RestartRollout(_) => self.act(command),
            Command::Scale { .. } => self.act(command),
            Command::Rollback { .. } => self.act(command),
            #[cfg(feature = "loco")]
            Command::Loco(command) => {
                let cluster = command.seen().target.cluster.clone();
                let handle = self.clusters.values().find(|c| c.id == cluster);
                self.navigation
                    .loco
                    .command(command, handle.map(|c| &c.handle));
            }
            Command::CopySpec { from, to, path } => self.copy_spec(from, to, path),
            Command::ShowClusterSettings(_) => {}
            Command::SetPreferences(id, preferences) => {
                return self.set_preferences(id, preferences);
            }
            Command::CheckForUpdates => {}
            Command::SetRefresh(refresh) => self.settings.apply_refresh(refresh),
            Command::Connect(id) => self.reconnect(id),
            Command::Reauthenticate(id) => self.reconnect(id),
            Command::Disconnect(id) => {
                self.disconnected.insert(id.clone());
                self.reconcile();
            }
            Command::ConfirmContext(id) => {
                self.confirm_context(id);
                return true;
            }
        }
        false
    }

    fn publish(&self, update: Update) {
        if self.updates.try_send(update).is_err() {
            tracing::debug!("UI stopped listening for updates");
        }
    }

    async fn rescan_sources(
        &mut self,
    ) -> Option<(Option<KubeconfigWatcher>, Receiver<DiscoveryEvent>)> {
        if !std::mem::take(&mut self.sources_changed) {
            return None;
        }
        self.discovery = scan(&self.sources).await;
        self.reconcile();
        self.publish_contexts();
        Some(watch_kubeconfigs(
            &self.sources,
            &self.discovery,
            &self.settings,
        ))
    }

    fn publish_contexts(&self) {
        let files = kubeconfig_files(&self.discovery, &self.store, self.home.as_deref());
        self.publish(Update::Contexts(files));
    }

    fn save_kubeconfig(&mut self, name: &str, yaml: &str) {
        let dir = self.store.kubeconfigs_dir();
        match paste::save(&dir, name, yaml) {
            Ok(_) => self.sources_changed = true,
            Err(error) => self.publish(Update::KubeconfigRejected(error.to_string())),
        }
    }

    fn watch_folder(&mut self, folder: &Path) {
        match self.store.add_folder(folder) {
            Ok(_) => {
                self.sources.folders = scan_folders(&self.store);
                self.sources_changed = true;
            }
            Err(error) => self.publish(Update::KubeconfigRejected(error.to_string())),
        }
    }

    fn add(&mut self, added: &[NewCluster]) {
        if let Err(error) = self.store.add(added) {
            tracing::error!(%error, path = %self.store.path().display(), "saving clusters failed");
        }
        self.reconcile();
    }

    fn reconnect(&mut self, id: &ClusterId) {
        self.disconnected.remove(id);
        let existing: Vec<ContextRef> = self
            .clusters
            .iter()
            .filter(|(_, cluster)| &cluster.id == id)
            .map(|(key, _)| key.clone())
            .collect();
        for key in existing {
            self.clusters.remove(&key);
            self.manager.disconnect(&key);
        }
        self.reconcile();
    }

    fn set_preferences(&mut self, id: &ClusterId, preferences: &Preferences) -> bool {
        let metrics_changed = self.store.get(id).is_some_and(|saved| {
            Preferences::from(&saved.preferences).metrics != preferences.metrics
        });
        match self.store.set_preferences(id, preferences) {
            Ok(changed) => {
                if changed && metrics_changed {
                    for cluster in self
                        .clusters
                        .values_mut()
                        .filter(|cluster| &cluster.id == id)
                    {
                        cluster.prometheus_task = None;
                        cluster.extras.prometheus = None;
                    }
                    self.for_each_window(|live| {
                        if live.navigation.selected.as_ref() == Some(id)
                            && live.navigation.page == Page::ClusterOverview
                        {
                            live.poll_prometheus();
                        }
                    });
                }
                changed
            }
            Err(error) => {
                tracing::error!(%error, path = %self.store.path().display(), "saving preferences failed");
                false
            }
        }
    }

    fn remove(&mut self, id: &ClusterId) {
        if let Err(error) = self.store.remove(id) {
            tracing::error!(%error, path = %self.store.path().display(), "saving clusters failed");
        }
        if self.navigation.selected.as_ref() == Some(id) {
            self.navigation.selected = None;
            self.navigation.page = Page::Overview;
        }
        self.reconcile();
    }

    fn check(&self, ids: &[ContextRef]) {
        for id in ids {
            let Some(context) = self.discovery.get(id).cloned() else {
                let line = CheckLine {
                    status: flock_core::CheckStatus::Err,
                    detail: MISSING_CONTEXT.to_owned(),
                    remedy: None,
                    duration: Duration::ZERO,
                };
                self.publish(Update::Check(id.clone(), line));
                self.publish(Update::Checked(id.clone(), Verdict::CannotReach));
                continue;
            };
            let id = id.clone();
            let env = self.env.clone();
            let settings = self.settings.clone();
            let updates = self.updates.clone();
            tokio::spawn(async move {
                let (progress, results) = async_channel::unbounded::<CheckResult>();
                let run = async move {
                    let report = run_checks_streaming(&context, &env, &settings, &progress).await;
                    drop(progress);
                    report
                };
                let forward = async {
                    while let Ok(result) = results.recv().await {
                        let line = check_line(&result);
                        if updates.send(Update::Check(id.clone(), line)).await.is_err() {
                            return;
                        }
                    }
                };
                let (report, ()) = tokio::join!(run, forward);
                let verdict = verdict(report.verdict);
                if updates.send(Update::Checked(id, verdict)).await.is_err() {
                    tracing::debug!("UI stopped listening for check results");
                }
            });
        }
    }

    fn handle(&self, id: &ClusterId) -> Option<&ClusterHandle> {
        self.clusters
            .values()
            .find(|cluster| &cluster.id == id)
            .map(|cluster| &cluster.handle)
    }

    fn act(&self, command: &Command) {
        let handle = cluster_of(command).and_then(|id| self.handle(id)).cloned();
        self.actions.run(command, handle);
    }

    fn open(&mut self, resource: &ResourceRef) {
        let page = page_for_kind(&resource.kind);
        let moved = self.navigation.selected.as_ref() != Some(&resource.cluster)
            || self.navigation.page != page;
        self.navigation.selected = Some(resource.cluster.clone());
        self.navigation.page = page;
        if moved {
            self.page_changed();
        }
        self.inspect(resource);
    }

    #[cfg(feature = "loco")]
    fn loco_snapshot(
        &self,
        clusters: &mut [ClusterSummary],
    ) -> BTreeMap<ClusterId, flock_core::loco::LocoCluster> {
        let mut loco = self.navigation.loco.snapshot(clusters, None);
        for navigation in self.windows.values() {
            if crate::loco::is_loco_page(navigation.page)
                && let Some(selected) = navigation.selected.as_ref()
            {
                loco.extend(navigation.loco.snapshot(clusters, Some(selected)));
            }
        }
        loco
    }

    fn viewing(&self, id: &ClusterId) -> Option<bool> {
        let mut selecting = self
            .navigations()
            .filter(|navigation| navigation.selected.as_ref() == Some(id))
            .peekable();
        selecting.peek()?;
        Some(selecting.any(|navigation| navigation.page == Page::Pods))
    }

    fn away(&self, saved: &SavedCluster) -> &'static str {
        if self.disconnected.contains(&saved.id()) {
            return DISCONNECTED_STATE;
        }
        match self.discovery.get(&saved.context_ref()) {
            Some(context) if !pinned(saved, context) => CHANGED_STATE,
            Some(_) => MISSING_STATE,
            None => MISSING_STATE,
        }
    }

    fn page_states(&self) -> BTreeMap<ClusterId, BTreeMap<Page, WatchState>> {
        let mut pages: BTreeMap<ClusterId, BTreeMap<Page, WatchState>> = BTreeMap::new();
        for navigation in self.navigations() {
            let Some(selected) = navigation.selected.as_ref() else {
                continue;
            };
            let Some(cluster) = self.clusters.values().find(|c| &c.id == selected) else {
                continue;
            };
            let page = navigation.page;
            let state = self.gitops.page_state(selected, page).or_else(|| {
                let kinds = page_kinds(page);
                let states = kinds.iter().map(|kind| cluster.handle.watch_state(kind));
                (!kinds.is_empty()).then(|| WatchState::combine(states))
            });
            if let Some(state) = state {
                pages
                    .entry(selected.clone())
                    .or_default()
                    .insert(page, state);
            }
        }
        pages
    }

    fn snapshot(&self) -> Snapshot {
        let now = Timestamp::now();
        let mut connected = Vec::new();
        let mut problems = Vec::new();
        let mut details = BTreeMap::new();
        let mut flux = Counts::default();
        let mut pods = Vec::new();
        for cluster in self.clusters.values() {
            let viewing = self.viewing(&cluster.id);
            let yaml: Vec<_> = self
                .navigations()
                .filter_map(|navigation| navigation.yaml.as_ref())
                .filter(|(target, _)| target.cluster == cluster.id)
                .collect();
            let selection = viewing.map(|pod_rows| Selection {
                pod_rows,
                event_limit: self.settings.event_limit,
                yaml: &yaml,
            });
            let health = self.gitops.health(&cluster.id, now);
            let built = assemble::cluster(
                &cluster.id,
                &cluster.handle,
                &cluster.extras,
                health.as_deref(),
                selection.as_ref(),
                now,
            );
            connected.push(built.summary);
            pods.extend(built.pods);
            problems.extend(built.problems);
            flux = flux + built.flux;
            if let Some(detail) = built.detail {
                details.insert(cluster.id.clone(), detail);
            }
        }
        order(&mut problems);
        let flagged = problems_badge(&problems);
        let missing = self
            .store
            .clusters()
            .iter()
            .filter(|saved| !self.clusters.contains_key(&saved.context_ref()))
            .map(|saved| offline_summary(saved, self.away(saved)));
        let mut clusters: Vec<_> = connected.into_iter().chain(missing).collect();
        clusters.sort_by(|a, b| a.name.cmp(&b.name));
        #[cfg(feature = "loco")]
        let loco = self.loco_snapshot(&mut clusters);
        let connected_settings = self.clusters.values().map(|cluster| {
            let expired = matches!(cluster.handle.status(), ConnectionStatus::AuthExpired(_));
            let saved = self.store.get(&cluster.id).map(|c| &c.preferences);
            let mut settings =
                cluster_settings(cluster.id.clone(), cluster.handle.context(), expired, saved);
            settings.prometheus = cluster
                .extras
                .prometheus
                .as_ref()
                .and_then(|found| found.target.as_ref())
                .map(|target| format!("{}:{}", target.label(), target.port));
            settings
        });
        let disconnected_settings = self
            .store
            .clusters()
            .iter()
            .filter(|saved| !self.clusters.contains_key(&saved.context_ref()))
            .filter_map(|saved| {
                let context = self.discovery.get(&saved.context_ref())?;
                let mut settings =
                    cluster_settings(saved.id(), context, false, Some(&saved.preferences));
                settings.context_change = context_change(saved, context);
                Some(settings)
            });
        let mut settings: Vec<ClusterSettings> =
            connected_settings.chain(disconnected_settings).collect();
        settings.sort_by(|a, b| a.cluster.cmp(&b.cluster));
        let sources: BTreeSet<&Path> = self.clusters.keys().map(|id| id.file.as_path()).collect();
        let mut snapshot = Snapshot {
            clusters,
            flock_nav: BTreeMap::from([(Page::Problems, flagged), (Page::GitOps, flux)]),
            home: FlockHome { problems },
            resources: Vec::new(),
            changes: ChangesLog::default(),
            details,
            pages: self.page_states(),
            gitops: GitOps::default(),
            flux: Vec::new(),
            #[cfg(feature = "loco")]
            loco,
            comparisons: self.comparisons(),
            catalog: ClusterCatalog {
                sources: sources.len(),
                clusters: settings,
            },
            pods,
            forwards: Vec::new(),
        };
        self.gitops.fill(&mut snapshot);
        self.actions.overlay(&mut snapshot);
        snapshot
    }
}

pub fn cluster_settings(
    cluster: ClusterId,
    context: &KubeContext,
    auth_expired: bool,
    saved: Option<&SavedPreferences>,
) -> ClusterSettings {
    let auth = match &context.auth {
        AuthKind::Exec(exec) => Auth::Exec(exec.summary()),
        other => Auth::Other(other.to_string()),
    };
    let mut preferences = saved.map(Preferences::from).unwrap_or_default();
    if preferences.default_namespace.is_none() {
        preferences.default_namespace.clone_from(&context.namespace);
    }
    ClusterSettings {
        context_change: None,
        cluster,
        connection: Connection {
            context: context.id.context.clone(),
            server: context.server.clone().unwrap_or_default(),
            auth,
            kubeconfig: context.id.file.display().to_string(),
        },
        auth_expired,
        prometheus: None,
        preferences,
    }
}

fn scan_folders(store: &ClusterStore) -> Vec<PathBuf> {
    let managed = store.kubeconfigs_dir();
    std::iter::once(managed)
        .chain(store.folders().iter().cloned())
        .collect()
}

fn watch_kubeconfigs(
    sources: &DiscoverySources,
    discovery: &Discovery,
    settings: &KubeSettings,
) -> (Option<KubeconfigWatcher>, Receiver<DiscoveryEvent>) {
    let started = KubeconfigWatcher::start(
        sources.clone(),
        discovery.clone(),
        settings.discovery_debounce,
    );
    match started {
        Ok((watcher, events)) => (Some(watcher), events),
        Err(error) => {
            tracing::warn!(%error, "kubeconfig files are not watched");
            (None, async_channel::unbounded().1)
        }
    }
}

async fn scan(sources: &DiscoverySources) -> Discovery {
    let sources = sources.clone();
    tokio::task::spawn_blocking(move || discover(&sources))
        .await
        .unwrap_or_default()
}

#[cfg(feature = "loco")]
fn refresh_extensions(live: &mut Live) -> bool {
    let mut changed = false;
    live.for_each_window(|live| {
        changed |= live.navigation.loco.refresh();
    });
    changed
}

#[cfg(not(feature = "loco"))]
fn refresh_extensions(_: &mut Live) -> bool {
    false
}

fn ticker(period: Duration) -> tokio::time::Interval {
    let mut interval = tokio::time::interval(period);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    interval
}

async fn run(backend: LiveBackend, updates: Sender<Update>, commands: Receiver<Command>) {
    let LiveBackend {
        settings,
        sources,
        store,
    } = backend;
    let shell = user_shell();
    let (env, discovery) =
        tokio::join!(capture(&shell, settings.shell_env_timeout), scan(&sources));
    let env = Arc::new(env);
    let (mut _watcher, mut discovery_events) = watch_kubeconfigs(&sources, &discovery, &settings);
    let runtime = tokio::runtime::Handle::current();
    let (fetched, fetched_events) = async_channel::unbounded();
    let [mut usage_tick, mut prometheus_tick] =
        [settings.metrics_interval, settings.prometheus_interval].map(ticker);
    let (manager, cluster_events) = ClusterManager::new(runtime, settings.clone(), env.clone());
    let (actions, action_changes) = Actions::new(updates.clone());
    let gitops = GitOpsState::new(settings.git_timeout);
    let home = sources.home.clone();
    let mut live = Live {
        window_id: DEFAULT_WINDOW,
        navigation: Navigation::new(&updates),
        windows: BTreeMap::new(),
        manager,
        store,
        discovery,
        sources,
        sources_changed: false,
        env,
        settings,
        updates: updates.clone(),
        home,
        clusters: BTreeMap::new(),
        disconnected: BTreeSet::new(),
        actions,
        gitops,
        fetched,
    };
    let gitops_changed = live.gitops.changed();
    live.start();
    let mut dirty = true;
    let mut publish = Throttle::new(live.settings.snapshot_interval);
    let mut poll = ticker(EXTENSION_POLL_INTERVAL);
    let mut history_tick = ticker(metrics_history::SAVE_INTERVAL);
    loop {
        tokio::select! {
            event = discovery_events.recv(), if !discovery_events.is_closed() => {
                if let Ok(event) = event {
                    apply_discovery(&mut live.discovery, event);
                    live.reconcile();
                    dirty = true;
                }
            }
            event = cluster_events.recv() => {
                if let Ok(event) = event {
                    live.cluster_event(&event);
                    dirty = true;
                }
            }
            changed = action_changes.recv() => dirty |= changed.is_ok(),
            changed = gitops_changed.recv() => dirty |= changed.is_ok(),
            fetched = fetched_events.recv() => {
                if let Ok(fetched) = fetched {
                    live.fetched(fetched);
                    dirty = true;
                }
            }
            _ = usage_tick.tick() => live.poll_usage(),
            _ = history_tick.tick() => live.save_histories(),
            _ = prometheus_tick.tick() => {
                live.for_each_window(|live| {
                    if live.navigation.page == Page::ClusterOverview { live.poll_prometheus(); }
                });
            }
            command = commands.recv() => {
                let Ok(command) = command else { live.save_histories(); return };
                live.apply(&command);
                retick(&mut usage_tick, live.settings.metrics_interval);
                retick(&mut prometheus_tick, live.settings.prometheus_interval);
                publish.set_period(live.settings.snapshot_interval);
                if let Some(restarted) = live.rescan_sources().await {
                    (_watcher, discovery_events) = restarted;
                }
                dirty = true;
            }
            _ = poll.tick() => dirty |= refresh_extensions(&mut live),
            () = publish.ready(), if dirty => {
                publish.fired();
                dirty = false;
                if updates.send(Update::Snapshot(Box::new(live.snapshot()))).await.is_err() {
                    live.save_histories();
                    return;
                }
            }
        }
    }
}

fn retick(interval: &mut tokio::time::Interval, period: Duration) {
    if interval.period() != period {
        *interval = ticker(period);
    }
}

fn leaves_compare(command: &Command) -> bool {
    matches!(
        command,
        Command::SelectCluster(_)
            | Command::SelectPage(_)
            | Command::OpenLogs { .. }
            | Command::OpenCommit(_)
            | Command::OpenFluxObject(_)
            | Command::OpenResource(_)
            | Command::EditYaml(_)
            | Command::ShowClusterSettings(_)
    )
}

fn pinned(saved: &SavedCluster, context: &KubeContext) -> bool {
    saved.pin.as_ref() == Some(&Pin::of(context))
}

fn context_change(saved: &SavedCluster, context: &KubeContext) -> Option<ContextChange> {
    if pinned(saved, context) {
        return None;
    }
    let pinned = saved.pin.as_ref().and_then(|pin| pin.server.clone());
    Some(ContextChange {
        pinned: pinned.unwrap_or_default(),
        current: context.server.clone().unwrap_or_default(),
    })
}

fn wanted(
    store: &ClusterStore,
    discovery: &Discovery,
    disconnected: &BTreeSet<ClusterId>,
) -> BTreeMap<ContextRef, ClusterId> {
    store
        .clusters()
        .iter()
        .filter(|saved| !disconnected.contains(&saved.id()))
        .filter(|saved| {
            discovery
                .get(&saved.context_ref())
                .is_some_and(|context| pinned(saved, context))
        })
        .map(|saved| (saved.context_ref(), saved.id()))
        .collect()
}

fn set_aside_notice(store: &ClusterStore) -> Option<Notice> {
    let kept = store.set_aside()?;
    Some(Notice {
        status: Status::Warn,
        title: SET_ASIDE_TITLE.to_owned(),
        detail: Some(format!("Kept as {}", kept.display())),
    })
}

fn offline_summary(saved: &SavedCluster, state: &str) -> ClusterSummary {
    ClusterSummary {
        id: saved.id(),
        name: saved.name.clone(),
        status: Status::Offline,
        state: state.to_owned(),
        distro: String::new(),
        region: String::new(),
        version: None,
        nodes: None,
        problems: Counts::default(),
        nav: BTreeMap::new(),
        metrics: None,
    }
}

pub fn auth_label(auth: &AuthKind) -> String {
    match auth.exec() {
        Some(exec) => exec.summary(),
        None => auth.to_string(),
    }
}

pub fn kubeconfig_files(
    discovery: &Discovery,
    store: &ClusterStore,
    home: Option<&Path>,
) -> Vec<KubeconfigFile> {
    let mut files: Vec<(PathBuf, KubeconfigFile)> = Vec::new();
    let available = discovery
        .contexts
        .iter()
        .filter(|context| !store.contains(&context.id));
    for context in available {
        let choice = ContextChoice {
            id: context.id.clone(),
            server: context.server.clone(),
            auth: auth_label(&context.auth),
            selected: false,
        };
        if let Some((_, file)) = files.iter_mut().find(|(path, _)| path == &context.id.file) {
            file.contexts.push(choice);
        } else {
            let file = KubeconfigFile {
                label: home_relative(&context.id.file, home),
                contexts: vec![choice],
            };
            files.push((context.id.file.clone(), file));
        }
    }
    files.into_iter().map(|(_, file)| file).collect()
}

pub fn check_line(result: &CheckResult) -> CheckLine {
    let status = match result.status {
        checks::CheckStatus::Ok => flock_core::CheckStatus::Ok,
        checks::CheckStatus::Warn => flock_core::CheckStatus::Warn,
        checks::CheckStatus::Err => flock_core::CheckStatus::Err,
        checks::CheckStatus::Info => flock_core::CheckStatus::Info,
    };
    CheckLine {
        status,
        detail: result.detail.clone(),
        remedy: result.remedy.clone(),
        duration: result.duration,
    }
}

pub fn verdict(verdict: checks::Verdict) -> Verdict {
    match verdict {
        checks::Verdict::Ready => Verdict::Ready,
        checks::Verdict::CannotReach => Verdict::CannotReach,
        checks::Verdict::CannotAuthenticate => Verdict::CannotAuthenticate,
    }
}

fn apply_discovery(discovery: &mut Discovery, event: DiscoveryEvent) {
    match event {
        DiscoveryEvent::Added(context) => discovery.contexts.push(context),
        DiscoveryEvent::Changed(context) => {
            if let Some(existing) = discovery
                .contexts
                .iter_mut()
                .find(|existing| existing.id == context.id)
            {
                *existing = context;
            }
        }
        DiscoveryEvent::Removed(id) => discovery.contexts.retain(|context| context.id != id),
        DiscoveryEvent::FileError(error) => {
            tracing::warn!(file = %error.file.display(), message = %error.message, "kubeconfig unreadable");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::provider::{ExecCommand, Provider};

    fn context(file: &str, name: &str) -> KubeContext {
        KubeContext {
            trust: crate::discovery::Fingerprint::default(),
            credentials: crate::discovery::Fingerprint::default(),
            id: ContextRef {
                file: PathBuf::from(file),
                context: name.to_owned(),
            },
            cluster: name.to_owned(),
            user: None,
            namespace: None,
            server: None,
            provider: Provider::Generic,
            auth: AuthKind::None,
            is_current: false,
        }
    }

    #[tokio::test]
    async fn metrics_preferences_invalidate_history_source_without_namespace_flicker()
    -> crate::Result<()> {
        let directory = tempfile::tempdir()?;
        let mut live = test_live(directory.path());
        let (client, _) = crate::fake::client(Duration::ZERO, |_, _| None);
        let (_manager, handle) = crate::fake::handle(&client);
        let key = handle.context().id.clone();
        let id = ClusterId::new("metrics-cluster");
        let added = NewCluster {
            context: key.clone(),
            name: id.to_string(),
        };
        assert!(live.store.add(&[added]).is_ok());
        let mut cluster = Connected::watching(id.clone(), handle);
        cluster.extras.prometheus = Some(Prometheus {
            target: None,
            utilization: Vec::new(),
            error: Some("forbidden".into()),
        });
        live.clusters.insert(key.clone(), cluster);
        let mut preferences = Preferences {
            default_namespace: Some("apps".into()),
            ..Preferences::default()
        };
        assert!(live.set_preferences(&id, &preferences));
        assert!(
            live.clusters
                .get(&key)
                .is_some_and(|cluster| cluster.extras.prometheus.is_some())
        );
        preferences.metrics = MetricsSource::MetricsServer;
        assert!(live.set_preferences(&id, &preferences));
        assert!(
            live.clusters
                .get(&key)
                .is_some_and(|cluster| cluster.extras.prometheus.is_none())
        );
        Ok(())
    }

    #[tokio::test]
    async fn histories_save_on_the_timer_path_and_before_disconnect() -> crate::Result<()> {
        let directory = tempfile::tempdir()?;
        let mut live = test_live(&directory.path().join("support"));
        live.home = Some(directory.path().to_path_buf());
        let (client, _) = crate::fake::client(Duration::ZERO, |_, _| None);
        let (_manager, handle) = crate::fake::handle(&client);
        let key = handle.context().id.clone();
        let id = ClusterId::new("metrics-cluster");
        let mut cluster = Connected::watching(id.clone(), handle);
        let now = Timestamp::now();
        cluster.extras.ring.push(
            now,
            crate::usage::Sample {
                cpu: 12.,
                memory: 34.,
            },
        );
        live.clusters.insert(key, cluster);
        live.save_histories();
        let cache = metrics_history::cache_dir(directory.path());
        let path = metrics_history::path(&cache, &id);
        assert_eq!(
            metrics_history::load(&path, now)
                .latest()
                .map(|sample| sample.cpu),
            Some(12.)
        );
        assert!(std::fs::remove_file(&path).is_ok());
        live.reconcile();
        assert!(live.clusters.is_empty());
        assert_eq!(
            metrics_history::load(&path, now)
                .latest()
                .map(|sample| sample.cpu),
            Some(12.)
        );
        Ok(())
    }

    const FIRST_WINDOW: u64 = 1;
    const SECOND_WINDOW: u64 = 2;

    fn test_live(path: &Path) -> Live {
        let settings = KubeSettings::default();
        let env = Arc::new(ShellEnv {
            vars: BTreeMap::new(),
            path: Vec::new(),
            origin: crate::shell_env::EnvOrigin::Fallback {
                reason: "test".into(),
            },
        });
        let (updates, _) = async_channel::unbounded();
        let (manager, _) = ClusterManager::new(
            tokio::runtime::Handle::current(),
            settings.clone(),
            env.clone(),
        );
        let (actions, _) = Actions::new(updates.clone());
        let gitops = GitOpsState::new(settings.git_timeout);
        Live {
            window_id: DEFAULT_WINDOW,
            navigation: Navigation::new(&updates),
            windows: BTreeMap::new(),
            manager,
            store: ClusterStore::empty(path),
            discovery: Discovery::default(),
            sources: DiscoverySources::default(),
            sources_changed: false,
            env,
            settings,
            updates,
            home: None,
            clusters: BTreeMap::new(),
            disconnected: BTreeSet::new(),
            actions,
            gitops,
            fetched: async_channel::unbounded().0,
        }
    }

    fn window_command(window: u64, command: Command) -> Command {
        Command::ForWindow(window, Box::new(command))
    }

    #[tokio::test]
    async fn pod_rows_are_built_only_while_some_window_shows_the_pods_page() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let mut live = test_live(dir.path());
        let cluster = ClusterId::new("a");
        for window in [FIRST_WINDOW, SECOND_WINDOW] {
            let select = Command::SelectCluster(Some(cluster.clone()));
            live.apply(&window_command(window, select));
            let page = Command::SelectPage(Page::ConfigMaps);
            live.apply(&window_command(window, page));
        }
        assert_eq!(live.viewing(&cluster), Some(false));
        let pods = Command::SelectPage(Page::Pods);
        live.apply(&window_command(SECOND_WINDOW, pods));
        assert_eq!(live.viewing(&cluster), Some(true));
        assert_eq!(live.viewing(&ClusterId::new("b")), None);
    }

    #[tokio::test]
    async fn refresh_settings_apply_at_runtime_from_any_window() {
        use flock_core::app_settings::{PollInterval, Refresh, SnapshotCap};
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let mut live = test_live(dir.path());
        let refresh = Refresh {
            metrics: PollInterval::Seconds5,
            prometheus: PollInterval::Seconds30,
            snapshots: SnapshotCap::Seconds1,
        };
        live.apply(&window_command(FIRST_WINDOW, Command::SetRefresh(refresh)));
        assert_eq!(live.settings, KubeSettings::with_refresh(&refresh));
        let mut usage = ticker(Duration::from_secs(15));
        retick(&mut usage, live.settings.metrics_interval);
        assert_eq!(usage.period(), Duration::from_secs(5));
    }

    #[tokio::test]
    async fn switching_clusters_keeps_every_cluster_connected() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let mut live = test_live(dir.path());
        for name in ["a", "b"] {
            let context = context("/missing-shell-test-kubeconfig", name);
            let handle = live.manager.connect(context.clone());
            let extras = Extras::default();
            live.clusters.insert(
                context.id,
                Connected {
                    id: ClusterId::new(name),
                    handle,
                    extras,
                    _background: Vec::new(),
                    usage_task: None,
                    pod_usage_task: None,
                    prometheus_task: None,
                },
            );
        }
        for selected in [Some("a"), Some("b"), None] {
            let command = Command::SelectCluster(selected.map(ClusterId::new));
            live.apply(&window_command(FIRST_WINDOW, command));
            assert_eq!(live.clusters.len(), 2);
            assert!(live.disconnected.is_empty());
        }
    }

    const LIST_PATIENCE: Duration = Duration::from_secs(10);
    const LIST_POLL: Duration = Duration::from_millis(20);
    const EMPTY_LIST: &str =
        r#"{"kind":"List","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[]}"#;
    const DEPLOYMENTS_PATH: &str = "/apis/apps/v1/deployments";
    const PODS_PATH: &str = "/api/v1/pods";
    const EVENTS_PATH: &str = "/api/v1/events";

    fn list(items: &[serde_json::Value]) -> String {
        serde_json::json!({
            "kind": "List",
            "apiVersion": "v1",
            "metadata": { "resourceVersion": "1" },
            "items": items,
        })
        .to_string()
    }

    fn failing_cluster() -> (ClusterManager, ClusterHandle, Receiver<ClusterEvent>) {
        use crate::problems::tests::{at, crash_status, replica_pod};
        let pod = replica_pod(
            "checkout-api-7f9c-x2lqp",
            "checkout-api",
            "7f9c",
            &crash_status(),
        );
        let pods = list(&[serde_json::to_value(&*pod).unwrap_or_default()]);
        let deployments = list(&[serde_json::json!({
            "metadata": { "name": "checkout-api", "namespace": "payments", "creationTimestamp": at(60) },
            "spec": { "replicas": 1, "selector": {}, "template": {} },
            "status": { "readyReplicas": 0 }
        })]);
        let events = list(&[serde_json::json!({
            "metadata": { "name": "checkout-api.1", "namespace": "payments" },
            "involvedObject": { "kind": "Pod", "name": "checkout-api-7f9c-x2lqp" },
            "type": "Warning",
            "reason": "BackOff",
            "lastTimestamp": at(1)
        })]);
        let (client, _) = crate::fake::client(LIST_POLL, move |method, path| {
            if method != http::Method::GET {
                return None;
            }
            if path.contains("watch=true") {
                return Some((http::StatusCode::OK, String::new()));
            }
            let body = if path.starts_with(DEPLOYMENTS_PATH) {
                deployments.clone()
            } else if path.starts_with(PODS_PATH) {
                pods.clone()
            } else if path.starts_with(EVENTS_PATH) {
                events.clone()
            } else {
                EMPTY_LIST.to_owned()
            };
            Some((http::StatusCode::OK, body))
        });
        crate::fake::supervised(&client)
    }

    #[tokio::test]
    async fn a_cluster_nobody_opened_still_counts_and_flags_every_nav_kind() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let mut live = test_live(dir.path());
        let (_manager, handle, _events) = failing_cluster();
        let id = ClusterId::new("a");
        let key = handle.id().clone();
        let cluster = Connected::watching(id.clone(), handle);
        live.clusters.insert(key, cluster);
        let deadline = tokio::time::Instant::now() + LIST_PATIENCE;
        let nav = loop {
            let snapshot = live.snapshot();
            let nav = snapshot
                .cluster(&id)
                .map(|c| c.nav.clone())
                .unwrap_or_default();
            let listed = [
                Page::Deployments,
                Page::Pods,
                Page::Events,
                Page::ConfigMaps,
            ];
            if listed.iter().all(|page| nav.contains_key(page)) {
                break nav;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "nav never filled: {nav:?}"
            );
            tokio::time::sleep(LIST_POLL).await;
        };
        assert!(live.snapshot().details.is_empty());
        assert_eq!(
            nav.get(&Page::Deployments),
            Some(&Counts::total(1).with_err(1))
        );
        assert_eq!(nav.get(&Page::Pods), Some(&Counts::total(1).with_err(1)));
        assert_eq!(nav.get(&Page::Events), Some(&Counts::default().with_err(1)));
        assert_eq!(nav.get(&Page::ConfigMaps), Some(&Counts::total(0)));
        let pages = Command::SelectPage(Page::Deployments);
        for command in [Command::SelectCluster(Some(id.clone())), pages] {
            live.apply(&window_command(FIRST_WINDOW, command));
        }
        let shown = live.snapshot();
        let Some(detail) = shown.detail(&id) else {
            panic!("the opened cluster has no detail");
        };
        let listed = shown
            .cluster(&id)
            .map(|c| c.nav.clone())
            .unwrap_or_default();
        for kind in flock_core::ResourceKind::ALL {
            let rows = Counts::of(detail.resources(kind).iter().map(|row| row.status));
            assert_eq!(listed.get(&kind.page()), Some(&rows), "{kind:?}");
        }
        live.apply(&window_command(
            FIRST_WINDOW,
            Command::SelectPage(Page::Overview),
        ));
        let after = live.snapshot();
        let flagged = after
            .cluster(&id)
            .and_then(|c| c.nav.get(&Page::Deployments));
        assert_eq!(flagged, Some(&Counts::total(1).with_err(1)));
    }

    #[tokio::test]
    async fn every_window_reports_the_watch_state_of_its_cluster_page() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let mut live = test_live(dir.path());
        let context = context("/missing-page-state-kubeconfig", "a");
        let handle = live.manager.connect(context.clone());
        let id = ClusterId::new("a");
        live.gitops.sync([(&id, &handle)]);
        let extras = Extras::default();
        live.clusters.insert(
            context.id,
            Connected {
                id: id.clone(),
                handle,
                extras,
                _background: Vec::new(),
                usage_task: None,
                pod_usage_task: None,
                prometheus_task: None,
            },
        );
        let pages = [
            (FIRST_WINDOW, Page::Deployments),
            (SECOND_WINDOW, Page::HelmReleases),
        ];
        for (window, page) in pages {
            let select = Command::SelectCluster(Some(id.clone()));
            live.apply(&window_command(window, select));
            live.apply(&window_command(window, Command::SelectPage(page)));
        }
        let states = live.page_states();
        let Some(reported) = states.get(&id) else {
            panic!("no page states for the selected cluster: {states:?}");
        };
        let shown: Vec<Page> = reported.keys().copied().collect();
        assert_eq!(shown, vec![Page::Deployments, Page::HelmReleases]);
        assert!(
            reported.values().all(|state| *state != WatchState::Synced),
            "{reported:?}"
        );
        let flock = Command::SelectPage(Page::Settings);
        live.apply(&window_command(SECOND_WINDOW, flock));
        let states = live.page_states();
        let shown: Vec<Page> = states
            .get(&id)
            .map(|pages| pages.keys().copied().collect())
            .unwrap_or_default();
        assert_eq!(shown, vec![Page::Deployments]);
    }

    #[tokio::test]
    async fn windows_keep_cluster_page_watches_and_yaml_independent() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let mut live = test_live(dir.path());
        for name in ["a", "b"] {
            let context = context("/missing-c77-test-kubeconfig", name);
            let handle = live.manager.connect(context.clone());
            let id = ClusterId::new(name);
            let extras = Extras::default();
            live.clusters.insert(
                context.id,
                Connected {
                    id,
                    handle,
                    extras,
                    _background: Vec::new(),
                    usage_task: None,
                    pod_usage_task: None,
                    prometheus_task: None,
                },
            );
        }
        live.apply(&window_command(
            FIRST_WINDOW,
            Command::SelectCluster(Some(ClusterId::new("a"))),
        ));
        live.apply(&window_command(
            FIRST_WINDOW,
            Command::SelectPage(Page::Secrets),
        ));
        live.apply(&window_command(
            SECOND_WINDOW,
            Command::SelectCluster(Some(ClusterId::new("b"))),
        ));
        live.apply(&window_command(
            SECOND_WINDOW,
            Command::SelectPage(Page::Services),
        ));
        let first = live
            .windows
            .get(&FIRST_WINDOW)
            .unwrap_or_else(|| panic!("first navigation"));
        assert_eq!(first.selected, Some(ClusterId::new("a")));
        assert_eq!(first.page, Page::Secrets);
        let second = live
            .windows
            .get(&SECOND_WINDOW)
            .unwrap_or_else(|| panic!("second navigation"));
        assert_eq!(second.page, Page::Services);
        let target = ResourceRef {
            cluster: ClusterId::new("a"),
            kind: "Secret".into(),
            namespace: Some("default".into()),
            name: "test".into(),
        };
        live.apply(&window_command(
            FIRST_WINDOW,
            Command::InspectResource(target.clone()),
        ));
        live.fetched(Fetched::Yaml(
            FIRST_WINDOW,
            target.clone(),
            inspect::Inspected {
                version: Some("1".into()),
                yaml: "kind: Secret".into(),
            },
        ));
        let first = live
            .windows
            .get(&FIRST_WINDOW)
            .unwrap_or_else(|| panic!("first navigation"));
        assert_eq!(first.yaml, Some((target, "kind: Secret".into())));
        assert!(
            live.windows
                .get(&SECOND_WINDOW)
                .is_some_and(|window| window.yaml.is_none())
        );
        let snapshot = live.snapshot();
        assert!(snapshot.details.contains_key(&ClusterId::new("a")));
        assert!(snapshot.details.contains_key(&ClusterId::new("b")));
        live.apply(&Command::WindowClosed(FIRST_WINDOW));
        assert!(!live.windows.contains_key(&FIRST_WINDOW));
        let snapshot = live.snapshot();
        assert!(!snapshot.details.contains_key(&ClusterId::new("a")));
        assert!(snapshot.details.contains_key(&ClusterId::new("b")));
    }

    #[tokio::test]
    async fn malformed_window_commands_cannot_nest_or_close_another_window() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let mut live = test_live(dir.path());
        live.apply(&window_command(
            FIRST_WINDOW,
            Command::SelectPage(Page::GitOps),
        ));
        assert!(!live.apply(&window_command(
            SECOND_WINDOW,
            Command::WindowClosed(FIRST_WINDOW)
        )));
        assert!(!live.apply(&window_command(
            SECOND_WINDOW,
            window_command(FIRST_WINDOW, Command::SelectPage(Page::Changes))
        )));
        assert_eq!(live.windows.len(), 1);
        assert!(
            live.windows
                .get(&FIRST_WINDOW)
                .is_some_and(|window| window.page == Page::GitOps)
        );
    }

    #[test]
    fn picker_lists_unadded_contexts_by_file() {
        let Ok(dir) = tempfile::tempdir() else {
            panic!("tempdir");
        };
        let mut store = ClusterStore::empty(dir.path());
        let added = NewCluster {
            context: context("/h/.kube/config", "kind-fleet-a").id,
            name: "fleet-a".to_owned(),
        };
        assert!(store.add(&[added]).is_ok());
        let mut eks = context("/h/.kube/eks/sandbox.yaml", "sandbox");
        eks.server = Some("https://x.eks.amazonaws.com".to_owned());
        eks.auth = AuthKind::Exec(ExecCommand {
            command: "aws".to_owned(),
            args: ["eks", "get-token", "--cluster-name", "sandbox"]
                .map(str::to_owned)
                .to_vec(),
        });
        let discovery = Discovery {
            files: Vec::new(),
            contexts: vec![
                context("/h/.kube/config", "kind-fleet-a"),
                context("/h/.kube/config", "kind-fleet-b"),
                eks,
            ],
            errors: Vec::new(),
        };
        let files = kubeconfig_files(&discovery, &store, Some(Path::new("/h")));
        let labels: Vec<&str> = files.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(labels, ["~/.kube/config", "~/.kube/eks/sandbox.yaml"]);
        let shown: Vec<(String, String, bool)> = files
            .iter()
            .flat_map(|f| &f.contexts)
            .map(|c| (c.id.context.clone(), c.meta(), c.selected))
            .collect();
        assert_eq!(
            shown,
            [
                ("kind-fleet-b".to_owned(), "none".to_owned(), false),
                (
                    "sandbox".to_owned(),
                    "https://x.eks.amazonaws.com · aws eks get-token".to_owned(),
                    false
                ),
            ]
        );
    }

    #[test]
    fn saved_clusters_without_a_context_show_offline() {
        let saved = SavedCluster {
            pin: None,
            file: PathBuf::from("/gone"),
            context: "old".to_owned(),
            name: "old".to_owned(),
            preferences: SavedPreferences::default(),
        };
        let summary = offline_summary(&saved, MISSING_STATE);
        assert_eq!(summary.status, Status::Offline);
        assert_eq!(summary.state, MISSING_STATE);
    }

    fn served(name: &str, server: &str) -> KubeContext {
        KubeContext {
            server: Some(server.to_owned()),
            ..context("/h/.kube/config", name)
        }
    }

    #[tokio::test]
    async fn a_retargeted_context_is_held_until_its_new_server_is_trusted() {
        let Ok(dir) = tempfile::tempdir() else {
            panic!("tempdir");
        };
        let mut live = test_live(dir.path());
        let lab = ClusterId::new("lab");
        live.discovery.contexts = vec![served("lab", "https://a:6443")];
        live.add(&[NewCluster {
            context: served("lab", "https://a:6443").id,
            name: "lab".to_owned(),
        }]);
        let Some(first) = live.handle(&lab).cloned() else {
            panic!("pinned on add and connected");
        };

        apply_discovery(
            &mut live.discovery,
            DiscoveryEvent::Changed(served("lab", "https://b:6443")),
        );
        live.reconcile();

        assert!(live.handle(&lab).is_none());
        assert!(first.is_closed());
        let snapshot = live.snapshot();
        let row = snapshot.cluster(&lab).map(|row| row.state.clone());
        assert_eq!(row.as_deref(), Some(CHANGED_STATE));
        let change = snapshot
            .catalog
            .get(&lab)
            .and_then(|settings| settings.context_change.clone());
        assert_eq!(
            change,
            Some(ContextChange {
                pinned: "https://a:6443".into(),
                current: "https://b:6443".into(),
            })
        );

        assert!(live.apply(&Command::ConfirmContext(lab.clone())));
        let server = live
            .handle(&lab)
            .and_then(|handle| handle.context().server.clone());
        assert_eq!(server.as_deref(), Some("https://b:6443"));
        let saved = live.store.get(&lab).and_then(|saved| saved.pin.clone());
        assert_eq!(
            saved.and_then(|pin| pin.server).as_deref(),
            Some("https://b:6443")
        );
    }

    #[tokio::test]
    async fn rotated_credentials_reconnect_with_the_new_material() {
        let Ok(dir) = tempfile::tempdir() else {
            panic!("tempdir");
        };
        let mut live = test_live(dir.path());
        let lab = ClusterId::new("lab");
        live.discovery.contexts = vec![served("lab", "https://a:6443")];
        live.add(&[NewCluster {
            context: served("lab", "https://a:6443").id,
            name: "lab".to_owned(),
        }]);
        let Some(first) = live.handle(&lab).cloned() else {
            panic!("connected");
        };
        let rotated = KubeContext {
            credentials: crate::discovery::Fingerprint::of(&[b"restricted"]),
            ..served("lab", "https://a:6443")
        };
        apply_discovery(
            &mut live.discovery,
            DiscoveryEvent::Changed(rotated.clone()),
        );
        live.reconcile();

        assert!(first.is_closed());
        let current = live.handle(&lab).map(|handle| handle.context().clone());
        assert_eq!(current, Some(rotated));
    }

    #[test]
    fn disconnected_clusters_stay_saved_but_are_not_watched() {
        let Ok(dir) = tempfile::tempdir() else {
            panic!("tempdir");
        };
        let mut store = ClusterStore::empty(dir.path());
        let added = ["kind-fleet-a", "kind-fleet-b", "gone"].map(|name| NewCluster {
            context: context("/h/.kube/config", name).id,
            name: name.to_owned(),
        });
        assert!(store.add(&added).is_ok());
        let discovery = Discovery {
            files: Vec::new(),
            contexts: vec![
                context("/h/.kube/config", "kind-fleet-a"),
                context("/h/.kube/config", "kind-fleet-b"),
            ],
            errors: Vec::new(),
        };
        for context in &discovery.contexts {
            let id = ClusterId::new(&context.id.context);
            assert!(store.pin(&id, Pin::of(context)).is_ok());
        }
        let names = |disconnected: &BTreeSet<ClusterId>| -> Vec<String> {
            wanted(&store, &discovery, disconnected)
                .into_values()
                .map(|id| id.to_string())
                .collect()
        };
        assert_eq!(names(&BTreeSet::new()), ["kind-fleet-a", "kind-fleet-b"]);
        let off = BTreeSet::from([ClusterId::new("kind-fleet-b")]);
        assert_eq!(names(&off), ["kind-fleet-a"]);
        assert_eq!(store.clusters().len(), 3);
    }

    #[test]
    fn a_set_aside_store_raises_a_notice() {
        let Ok(dir) = tempfile::tempdir() else {
            panic!("tempdir");
        };
        assert_eq!(set_aside_notice(&ClusterStore::empty(dir.path())), None);
        assert!(std::fs::write(dir.path().join("clusters.json"), "{").is_ok());
        let Ok(store) = ClusterStore::open(dir.path()) else {
            panic!("open");
        };
        let notice = set_aside_notice(&store);
        assert_eq!(notice.as_ref().map(|n| n.status), Some(Status::Warn));
        assert_eq!(notice.map(|n| n.title), Some(SET_ASIDE_TITLE.to_owned()));
    }

    #[test]
    fn check_results_map_to_core_lines() {
        let result = CheckResult {
            kind: checks::CheckKind::Authenticate,
            status: checks::CheckStatus::Err,
            detail: "ExpiredToken".to_owned(),
            remedy: Some("aws sso login".to_owned()),
            duration: Duration::from_millis(5),
        };
        let line = check_line(&result);
        assert_eq!(line.status, flock_core::CheckStatus::Err);
        assert_eq!(line.remedy.as_deref(), Some("aws sso login"));
        assert_eq!(
            verdict(checks::Verdict::CannotAuthenticate),
            Verdict::CannotAuthenticate
        );
    }

    #[test]
    fn discovery_events_update_the_context_list() {
        let mut discovery = Discovery::default();
        apply_discovery(&mut discovery, DiscoveryEvent::Added(context("/a", "x")));
        let mut changed = context("/a", "x");
        changed.namespace = Some("apps".to_owned());
        apply_discovery(&mut discovery, DiscoveryEvent::Changed(changed.clone()));
        assert_eq!(discovery.contexts, vec![changed]);
        apply_discovery(
            &mut discovery,
            DiscoveryEvent::Removed(context("/a", "x").id),
        );
        assert_eq!(discovery.contexts, Vec::new());
    }

    #[test]
    fn settings_carry_the_kubeconfig_connection() {
        let mut gke = context("/h/.kube/config", "gke_qa");
        gke.server = Some("https://10.0.0.1".to_owned());
        gke.namespace = Some("payments".to_owned());
        gke.auth = AuthKind::Exec(ExecCommand {
            command: "/usr/local/bin/gke-gcloud-auth-plugin".to_owned(),
            args: Vec::new(),
        });
        let settings = cluster_settings(ClusterId::new("gke_qa"), &gke, false, None);
        assert_eq!(
            settings.connection.auth.label(),
            "exec · gke-gcloud-auth-plugin"
        );
        assert_eq!(settings.connection.server, "https://10.0.0.1");
        assert_eq!(settings.connection.kubeconfig, "/h/.kube/config");
        assert_eq!(
            settings.preferences.default_namespace.as_deref(),
            Some("payments")
        );
        assert!(!settings.auth_expired);
    }
}

#[cfg(test)]
mod performance_tests {
    use super::*;
    use crate::manager::COALESCE_INTERVAL;

    const UPDATE_BUDGET: Duration = Duration::from_millis(500);

    #[test]
    fn coalescing_leaves_time_for_network_and_snapshot_building() {
        assert!(COALESCE_INTERVAL + SNAPSHOT_INTERVAL < UPDATE_BUDGET);
    }
}
