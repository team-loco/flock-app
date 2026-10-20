mod logs;
mod map;
mod pending;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use flock_core::Update;
use flock_core::action::Notice;
use flock_core::loco::{LocoCluster, LocoCommand};
use flock_core::{ClusterId, ClusterSummary, Page, ResourceRef, Seen, Sender, Uid};
use flock_loco::platform::{
    Observation, WorkloadHealth, agent_sync, component_catalog, deployment_health,
};
use flock_loco::{API_VERSION, BUILD_NAMESPACE, WORKSPACE_PREFIX};
use futures::future::join_all;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::{Pod, Secret, Service, ServiceAccount};
use k8s_openapi::api::networking::v1::NetworkPolicy;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{DynamicObject, PartialObjectMeta, PropagationPolicy};
use kube::{Api, Client, Resource};
use serde::de::DeserializeOwned;

use crate::actions::{failed_notice, mutate};
use crate::error::Error;
use crate::manager::{ClusterHandle, Shared, Subscription, lock};
use crate::ops::{LogRequest, ObjectRef, PatchBody, restart_patch};
use crate::resources::{CustomKind, WatchKind, Watched};
use crate::task::TaskGuard;

use self::logs::Tail;
use self::map::{
    AGENT_DEPLOYMENT, BUILD_CONTAINERS, Inputs, LOCO_NAMESPACE, build_pod, loco_cluster, nav_counts,
};
use self::pending::{Pending, apply_pending};

const LOCO_GROUP: &str = flock_core::loco::GROUP;
const LOCO_VERSION: &str = "v1alpha1";
const GATEWAY_GROUP: &str = "gateway.networking.k8s.io";
const GATEWAY_VERSION: &str = "v1";
const DISCOVERY_GROUP: &str = "discovery.k8s.io";
const DISCOVERY_VERSION: &str = "v1";
const NOT_FOUND: u16 = 404;
const RESTART: &str = "Restart";
const NOT_LOADED: &str = "the Application is not loaded yet";
const BUILTIN_KINDS: [WatchKind; 9] = [
    WatchKind::Deployments,
    WatchKind::DaemonSets,
    WatchKind::StatefulSets,
    WatchKind::Jobs,
    WatchKind::Pods,
    WatchKind::Services,
    WatchKind::Secrets,
    WatchKind::ServiceAccounts,
    WatchKind::NetworkPolicies,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocoSettings {
    pub probe_interval: Duration,
    pub probe_retry: Duration,
    pub agent_log_tail: i64,
    pub log_line_limit: usize,
    pub build_log_limit: usize,
    pub restart_grace: Duration,
    pub restart_timeout: Duration,
}

impl Default for LocoSettings {
    fn default() -> Self {
        Self {
            probe_interval: Duration::from_secs(30),
            probe_retry: Duration::from_secs(2),
            agent_log_tail: 200,
            log_line_limit: 2000,
            build_log_limit: 4,
            restart_grace: Duration::from_secs(3),
            restart_timeout: Duration::from_mins(5),
        }
    }
}

fn application_kind() -> CustomKind {
    CustomKind::new(LOCO_GROUP, LOCO_VERSION, "Application")
}

fn custom_kinds() -> [CustomKind; 5] {
    [
        application_kind(),
        CustomKind::new(LOCO_GROUP, LOCO_VERSION, "Build"),
        CustomKind::new(GATEWAY_GROUP, GATEWAY_VERSION, "Gateway"),
        CustomKind::new(GATEWAY_GROUP, GATEWAY_VERSION, "HTTPRoute"),
        CustomKind::new(DISCOVERY_GROUP, DISCOVERY_VERSION, "EndpointSlice"),
    ]
}

pub fn is_loco_page(page: Page) -> bool {
    matches!(
        page,
        Page::LocoPlatform | Page::LocoApplications | Page::LocoBuilds | Page::LocoRoutes
    )
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Observed {
    installed: bool,
    builds_enabled: Option<bool>,
    agent: Option<WorkloadHealth>,
}

struct Probe {
    shared: Weak<Shared>,
    observed: Arc<Mutex<Observed>>,
    _task: TaskGuard,
}

struct PageWatch {
    handle: ClusterHandle,
    _builtin: Vec<Subscription>,
    _custom: TaskGuard,
    agent_log: Option<(String, Tail)>,
    container_logs: BTreeMap<(String, String), Tail>,
}

struct Selection {
    cluster: ClusterId,
    page: Page,
}

pub struct LocoBackend {
    settings: LocoSettings,
    probes: BTreeMap<ClusterId, Probe>,
    clusters: BTreeMap<ClusterId, ClusterHandle>,
    selection: Option<Selection>,
    watches: BTreeMap<ClusterId, PageWatch>,
    pending: Arc<Mutex<BTreeMap<ResourceRef, Pending>>>,
    changed: Arc<AtomicBool>,
    updates: Sender<Update>,
}

impl LocoBackend {
    pub fn new(settings: LocoSettings, updates: Sender<Update>) -> Self {
        Self {
            updates,
            settings,
            probes: BTreeMap::new(),
            clusters: BTreeMap::new(),
            selection: None,
            watches: BTreeMap::new(),
            pending: Arc::new(Mutex::new(BTreeMap::new())),
            changed: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn sync<'a>(
        &mut self,
        clusters: impl Iterator<Item = (&'a ClusterId, &'a ClusterHandle)>,
        selected: Option<&ClusterId>,
        page: Page,
    ) {
        let clusters: Vec<(&ClusterId, &ClusterHandle)> = clusters.collect();
        self.probes.retain(|id, probe| {
            clusters.iter().any(|(candidate, handle)| {
                *candidate == id && Weak::ptr_eq(&probe.shared, &Arc::downgrade(&handle.shared))
            })
        });
        for (id, handle) in &clusters {
            if !self.probes.contains_key(*id) {
                let probe = self.probe(handle);
                self.probes.insert((*id).clone(), probe);
            }
        }
        self.selection = selected
            .and_then(|id| clusters.iter().find(|(candidate, _)| *candidate == id))
            .map(|(id, _)| Selection {
                cluster: (*id).clone(),
                page,
            });
        self.clusters = clusters
            .into_iter()
            .map(|(id, handle)| (id.clone(), handle.clone()))
            .collect();
        self.update_watches();
    }

    pub fn refresh(&mut self) -> bool {
        self.update_watches();
        self.update_logs();
        self.changed.swap(false, Ordering::Relaxed)
    }

    fn probe(&self, handle: &ClusterHandle) -> Probe {
        let observed = Arc::new(Mutex::new(Observed::default()));
        let shared = Arc::downgrade(&handle.shared);
        let task = TaskGuard::spawn(probe_loop(
            shared.clone(),
            observed.clone(),
            self.changed.clone(),
            self.settings.clone(),
        ));
        Probe {
            shared,
            observed,
            _task: task,
        }
    }

    fn observed(&self, id: &ClusterId) -> Option<Observed> {
        self.probes
            .get(id)
            .map(|probe| lock(&probe.observed).clone())
    }

    fn update_watches(&mut self) {
        let installed: BTreeMap<&ClusterId, &ClusterHandle> = self
            .clusters
            .iter()
            .filter(|(id, _)| self.observed(id).is_some_and(|observed| observed.installed))
            .collect();
        self.watches.retain(|id, watch| {
            installed
                .get(id)
                .is_some_and(|handle| Arc::ptr_eq(&watch.handle.shared, &handle.shared))
        });
        for (id, handle) in installed {
            if !self.watches.contains_key(id) {
                self.watches.insert(id.clone(), start_watch(handle));
                self.changed.store(true, Ordering::Relaxed);
            }
        }
    }

    fn update_logs(&mut self) {
        let selected = self
            .selection
            .as_ref()
            .filter(|selection| is_loco_page(selection.page));
        let page = selected.map(|selection| selection.page);
        let cluster = selected.map(|selection| selection.cluster.clone());
        for (id, watch) in &mut self.watches {
            if Some(id) != cluster.as_ref() {
                watch.agent_log = None;
                watch.container_logs.clear();
            }
        }
        let Some(watch) = cluster.and_then(|id| self.watches.get_mut(&id)) else {
            return;
        };
        let settings = &self.settings;
        let changed = &self.changed;
        if page == Some(Page::LocoPlatform) {
            let pod = agent_pod(&watch.handle);
            let same = watch.agent_log.as_ref().map(|(name, _)| name) == pod.as_ref();
            if !same {
                watch.agent_log = pod.map(|pod| {
                    let request = LogRequest {
                        namespace: LOCO_NAMESPACE.to_owned(),
                        pod: pod.clone(),
                        follow: true,
                        tail_lines: Some(settings.agent_log_tail),
                        ..LogRequest::default()
                    };
                    let tail = Tail::start(
                        &watch.handle,
                        request,
                        settings.log_line_limit,
                        changed.clone(),
                    );
                    (pod, tail)
                });
                changed.store(true, Ordering::Relaxed);
            }
        } else {
            watch.agent_log = None;
        }
        let wanted = if page == Some(Page::LocoBuilds) {
            build_containers(&watch.handle, settings.build_log_limit)
        } else {
            BTreeSet::new()
        };
        watch.container_logs.retain(|key, _| wanted.contains(key));
        for key in wanted {
            if watch.container_logs.contains_key(&key) {
                continue;
            }
            let request = LogRequest {
                namespace: BUILD_NAMESPACE.to_owned(),
                pod: key.0.clone(),
                container: Some(key.1.clone()),
                follow: true,
                ..LogRequest::default()
            };
            let tail = Tail::start(
                &watch.handle,
                request,
                settings.log_line_limit,
                changed.clone(),
            );
            watch.container_logs.insert(key, tail);
        }
    }

    pub fn snapshot(
        &self,
        clusters: &mut [ClusterSummary],
        only: Option<&ClusterId>,
    ) -> BTreeMap<ClusterId, LocoCluster> {
        let now = Timestamp::now();
        let zone = TimeZone::system();
        let mut pending = lock(&self.pending);
        let mut loco = BTreeMap::new();
        let probes = self
            .probes
            .iter()
            .filter(|(id, _)| only.is_none_or(|only| only == *id));
        for (id, probe) in probes {
            let observed = lock(&probe.observed).clone();
            if !observed.installed {
                continue;
            }
            let watch = self.watches.get(id);
            let inputs = match watch {
                Some(watch) => gather(id, watch, &observed),
                None => Inputs {
                    cluster: id.clone(),
                    agent: observed.agent.clone(),
                    builds_enabled: observed.builds_enabled,
                    ..Inputs::default()
                },
            };
            let mut cluster = loco_cluster(&inputs, now, &zone);
            apply_pending(
                &mut cluster,
                id,
                &mut pending,
                Instant::now(),
                &self.settings,
            );
            if watch.is_some()
                && let Some(summary) = clusters.iter_mut().find(|summary| &summary.id == id)
            {
                summary.nav.extend(nav_counts(&cluster));
            }
            loco.insert(id.clone(), cluster);
        }
        loco
    }

    pub fn command(&self, command: &LocoCommand, handle: Option<&ClusterHandle>) {
        let Some(handle) = handle else {
            return;
        };
        match command {
            LocoCommand::RestartApplication(application) => self.restart(application, handle),
            LocoCommand::CancelBuild(build) => self.cancel(build, handle),
        }
    }

    fn restart(&self, seen: &Seen, handle: &ClusterHandle) {
        let application = &seen.target;
        let found = handle
            .custom_objects(&application_kind())
            .into_iter()
            .filter(|object| {
                object.metadata.namespace == application.namespace
                    && object.metadata.name.as_deref() == Some(application.name.as_str())
            })
            .find_map(|object| typed::<flock_loco::generated::applications::Application>(&object));
        let model =
            found.and_then(|found| flock_loco::applications::application_model(&found).ok());
        let Some(model) = model else {
            tracing::warn!(application = %application, "restart skipped: Application not loaded");
            let title = format!("{RESTART} {} failed", application.name);
            let detail = format!("{} · {NOT_LOADED}", application.cluster);
            let notice = Notice::failed(title, detail);
            if self.updates.try_send(Update::Notice(notice)).is_err() {
                tracing::debug!("UI stopped listening for updates");
            }
            return;
        };
        let target = ObjectRef {
            api_version: "apps/v1".to_owned(),
            kind: "Deployment".to_owned(),
            namespace: Some(model.workspace_namespace),
            name: model.workload_name,
        };
        let since = Instant::now();
        lock(&self.pending).insert(application.clone(), Pending::Restarting { since });
        self.changed.store(true, Ordering::Relaxed);
        let handle = handle.clone();
        let pending = self.pending.clone();
        let changed = self.changed.clone();
        let updates = self.updates.clone();
        let application = application.clone();
        let uid = seen.uid.clone();
        tokio::spawn(async move {
            if let Err(error) = restart_workload(&handle, &target, &uid).await {
                tracing::warn!(%error, application = %application, "restart failed");
                lock(&pending).remove(&application);
                changed.store(true, Ordering::Relaxed);
                let notice = failed_notice(RESTART, &application, &error);
                if updates.send(Update::Notice(notice)).await.is_err() {
                    tracing::debug!("UI stopped listening for updates");
                }
            }
        });
    }

    fn cancel(&self, seen: &Seen, handle: &ClusterHandle) {
        let build = &seen.target;
        let target = ObjectRef {
            api_version: "batch/v1".to_owned(),
            kind: "Job".to_owned(),
            namespace: build.namespace.clone(),
            name: build.name.clone(),
        };
        lock(&self.pending).insert(build.clone(), Pending::Canceling);
        self.changed.store(true, Ordering::Relaxed);
        let handle = handle.clone();
        let pending = self.pending.clone();
        let changed = self.changed.clone();
        let build = build.clone();
        let uid = seen.uid.clone();
        tokio::spawn(async move {
            let outcome = cancel_job(&handle, &target, &uid).await;
            let Err(error) = outcome else {
                return;
            };
            if not_found(&error) {
                return;
            }
            tracing::warn!(%error, build = %build, "cancel failed");
            lock(&pending).insert(build, Pending::CancelFailed(error.to_string()));
            changed.store(true, Ordering::Relaxed);
        });
    }
}

async fn restart_workload(
    handle: &ClusterHandle,
    target: &ObjectRef,
    uid: &Uid,
) -> Result<(), Error> {
    let uid = mutate::known(uid)?;
    let body = PatchBody::Merge(restart_patch(Timestamp::now())).guarded(uid);
    match handle.patch(target, body).await {
        Ok(_) => Ok(()),
        Err(error) => Err(mutate::explain(handle, target, uid, error).await),
    }
}

async fn cancel_job(handle: &ClusterHandle, target: &ObjectRef, uid: &Uid) -> Result<(), Error> {
    let uid = mutate::known(uid)?;
    match handle
        .delete(target, PropagationPolicy::Background, Some(uid))
        .await
    {
        Ok(()) => Ok(()),
        Err(error) => Err(mutate::explain(handle, target, uid, error).await),
    }
}

fn not_found(error: &Error) -> bool {
    matches!(error, Error::Kube(kube::Error::Api(status)) if status.code == NOT_FOUND)
}

async fn probe_loop(
    shared: Weak<Shared>,
    observed: Arc<Mutex<Observed>>,
    changed: Arc<AtomicBool>,
    settings: LocoSettings,
) {
    loop {
        let Some(alive) = shared.upgrade() else {
            return;
        };
        let client = alive.current_client();
        drop(alive);
        let delay = match client {
            Some(client) => {
                if let Some(next) = observe(&client).await {
                    let mut current = lock(&observed);
                    if *current != next {
                        *current = next;
                        changed.store(true, Ordering::Relaxed);
                    }
                }
                settings.probe_interval
            }
            None => settings.probe_retry,
        };
        tokio::time::sleep(delay).await;
    }
}

async fn observe(client: &Client) -> Option<Observed> {
    let deployments: Api<Deployment> = Api::namespaced(client.clone(), LOCO_NAMESPACE);
    let (resources, agent) = tokio::join!(
        client.list_api_group_resources(API_VERSION),
        deployments.get_opt(AGENT_DEPLOYMENT)
    );
    let resources = match resources {
        Ok(resources) => resources,
        Err(kube::Error::Api(status)) if status.code == NOT_FOUND => {
            return Some(Observed::default());
        }
        Err(error) => {
            tracing::debug!(%error, "loco discovery failed");
            return None;
        }
    };
    let builds_enabled = match agent_sync(Some(&resources)).builds_enabled {
        Observation::Observed(enabled) => Some(enabled),
        Observation::Unavailable => None,
    };
    let agent = match agent {
        Ok(found) => found.and_then(|deployment| deployment_health(&deployment).ok()),
        Err(error) => {
            tracing::debug!(%error, "loco agent deployment unreadable");
            None
        }
    };
    Some(Observed {
        installed: true,
        builds_enabled,
        agent,
    })
}

fn start_watch(handle: &ClusterHandle) -> PageWatch {
    let builtin = BUILTIN_KINDS
        .iter()
        .map(|kind| handle.subscribe(kind.clone()))
        .collect();
    let subscriber = handle.clone();
    let custom = TaskGuard::spawn(async move {
        let kinds = custom_kinds();
        let outcomes = join_all(
            kinds
                .iter()
                .map(|kind| subscriber.subscribe_custom(kind.clone())),
        )
        .await;
        let mut held = Vec::new();
        for (kind, outcome) in kinds.iter().zip(outcomes) {
            match outcome {
                Ok(subscription) => held.push(subscription),
                Err(error) => tracing::debug!(%error, kind = %kind.kind, "loco watch unavailable"),
            }
        }
        drop(subscriber);
        std::future::pending::<()>().await;
        drop(held);
    });
    PageWatch {
        handle: handle.clone(),
        _builtin: builtin,
        _custom: custom,
        agent_log: None,
        container_logs: BTreeMap::new(),
    }
}

fn owned<K: Watched>(handle: &ClusterHandle, keep: impl Fn(&str) -> bool) -> Vec<K>
where
    K::DynamicType: Eq + std::hash::Hash + Clone,
{
    handle
        .objects::<K>()
        .into_iter()
        .filter(|object| object.meta().namespace.as_deref().is_some_and(&keep))
        .map(|object| (*object).clone())
        .collect()
}

fn metadata<K>(handle: &ClusterHandle, keep: impl Fn(&str) -> bool) -> Vec<ObjectMeta>
where
    K: Resource<DynamicType = ()> + Clone + std::fmt::Debug + Send + Sync + 'static,
    PartialObjectMeta<K>: Watched + Resource<DynamicType = ()>,
{
    handle
        .objects::<PartialObjectMeta<K>>()
        .into_iter()
        .filter(|object| object.metadata.namespace.as_deref().is_some_and(&keep))
        .map(|object| object.metadata.clone())
        .collect()
}

fn typed<T: DeserializeOwned>(object: &DynamicObject) -> Option<T> {
    let parsed = serde_json::to_value(object).and_then(serde_json::from_value);
    match parsed {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::debug!(%error, name = ?object.metadata.name, "skipping an unreadable object");
            None
        }
    }
}

fn custom<T: DeserializeOwned>(
    handle: &ClusterHandle,
    kind: &CustomKind,
    keep: impl Fn(&str) -> bool,
) -> Vec<T> {
    handle
        .custom_objects(kind)
        .iter()
        .filter(|object| object.metadata.namespace.as_deref().is_some_and(&keep))
        .filter_map(|object| typed(object))
        .collect()
}

fn gather(cluster: &ClusterId, watch: &PageWatch, observed: &Observed) -> Inputs {
    let handle = &watch.handle;
    let catalog = component_catalog();
    let platform: BTreeSet<&str> = catalog.iter().map(|c| c.namespace.as_str()).collect();
    let pod_namespaces: BTreeSet<&str> = catalog
        .iter()
        .filter(|component| component.kind == "Pod")
        .map(|component| component.namespace.as_str())
        .chain([BUILD_NAMESPACE, LOCO_NAMESPACE])
        .collect();
    let workspace = |namespace: &str| namespace.starts_with(WORKSPACE_PREFIX);
    let any = |_: &str| true;
    let [applications, builds, gateways, routes, slices] = custom_kinds();
    Inputs {
        cluster: cluster.clone(),
        applications: custom(handle, &applications, any),
        builds: custom(handle, &builds, any),
        gateways: custom(handle, &gateways, any),
        routes: custom(handle, &routes, any),
        endpoint_slices: custom(handle, &slices, workspace),
        services: owned::<Service>(handle, workspace),
        deployments: owned::<Deployment>(handle, |ns| workspace(ns) || platform.contains(ns)),
        daemonsets: owned::<DaemonSet>(handle, |ns| platform.contains(ns)),
        statefulsets: owned::<StatefulSet>(handle, |ns| platform.contains(ns)),
        jobs: owned::<Job>(handle, |ns| ns == BUILD_NAMESPACE),
        pods: owned::<Pod>(handle, |ns| pod_namespaces.contains(ns)),
        secrets: metadata::<Secret>(handle, workspace),
        service_accounts: metadata::<ServiceAccount>(handle, workspace),
        network_policies: metadata::<NetworkPolicy>(handle, workspace),
        workloads_watched: true,
        agent: observed.agent.clone(),
        builds_enabled: observed.builds_enabled,
        agent_log: watch
            .agent_log
            .as_ref()
            .map(|(_, tail)| tail.lines())
            .unwrap_or_default(),
        container_logs: watch
            .container_logs
            .iter()
            .map(|(key, tail)| (key.clone(), tail.lines()))
            .collect(),
    }
}

fn agent_pod(handle: &ClusterHandle) -> Option<String> {
    let deployment = handle
        .objects::<Deployment>()
        .into_iter()
        .find(|deployment| {
            deployment.metadata.namespace.as_deref() == Some(LOCO_NAMESPACE)
                && deployment.metadata.name.as_deref() == Some(AGENT_DEPLOYMENT)
        })?;
    let selector = deployment.spec.as_ref()?.selector.match_labels.clone()?;
    let mut pods: Vec<Arc<Pod>> = handle
        .objects::<Pod>()
        .into_iter()
        .filter(|pod| {
            pod.metadata.namespace.as_deref() == Some(LOCO_NAMESPACE)
                && pod.metadata.labels.as_ref().is_some_and(|labels| {
                    selector
                        .iter()
                        .all(|(key, value)| labels.get(key) == Some(value))
                })
        })
        .collect();
    pods.sort_by_key(|pod| {
        let running = pod
            .status
            .as_ref()
            .and_then(|status| status.phase.as_deref())
            == Some("Running");
        let created = pod.metadata.creation_timestamp.as_ref().map(|time| time.0);
        std::cmp::Reverse((running, created))
    });
    pods.first().and_then(|pod| pod.metadata.name.clone())
}

fn build_containers(handle: &ClusterHandle, limit: usize) -> BTreeSet<(String, String)> {
    let mut pods: Vec<Arc<Pod>> = handle
        .objects::<Pod>()
        .into_iter()
        .filter(|pod| build_pod(pod))
        .collect();
    pods.sort_by_key(|pod| {
        std::cmp::Reverse(pod.metadata.creation_timestamp.as_ref().map(|time| time.0))
    });
    pods.iter()
        .take(limit)
        .flat_map(|pod| {
            let name = pod.metadata.name.clone().unwrap_or_default();
            let started: Vec<String> = pod
                .status
                .iter()
                .flat_map(|status| {
                    status
                        .init_container_statuses
                        .iter()
                        .flatten()
                        .chain(status.container_statuses.iter().flatten())
                })
                .filter(|container| {
                    BUILD_CONTAINERS.contains(&container.name.as_str())
                        && container.state.as_ref().is_some_and(|state| {
                            state.running.is_some() || state.terminated.is_some()
                        })
                })
                .map(|container| container.name.clone())
                .collect();
            started
                .into_iter()
                .map(move |container| (name.clone(), container))
        })
        .filter(|(pod, _)| !pod.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROUND_TRIP: Duration = Duration::from_secs(1);

    #[tokio::test(start_paused = true)]
    async fn discovery_and_agent_are_probed_together() {
        let group = format!("/apis/{API_VERSION}");
        let listing = format!(
            r#"{{"kind":"APIResourceList","apiVersion":"v1","groupVersion":"{API_VERSION}","resources":[]}}"#
        );
        let (client, traffic) = crate::fake::client(ROUND_TRIP, move |_, path| {
            (path == group).then(|| (http::StatusCode::OK, listing.clone()))
        });
        let started = tokio::time::Instant::now();
        let observed = observe(&client).await;
        assert!(started.elapsed() < ROUND_TRIP * 2);
        assert_eq!(traffic.peak(), 2);
        assert_eq!(observed.map(|o| o.installed), Some(true));
    }

    fn installed_cluster() -> (crate::manager::ClusterManager, ClusterHandle) {
        let group = format!("/apis/{API_VERSION}");
        let listing = format!(
            r#"{{"kind":"APIResourceList","apiVersion":"v1","groupVersion":"{API_VERSION}","resources":[]}}"#
        );
        let (client, _) = crate::fake::client(Duration::ZERO, move |_, path| {
            (path == group).then(|| (http::StatusCode::OK, listing.clone()))
        });
        crate::fake::handle(&client)
    }

    fn summary(id: &ClusterId) -> ClusterSummary {
        ClusterSummary {
            id: id.clone(),
            name: id.to_string(),
            status: flock_core::Status::Ok,
            state: String::new(),
            distro: String::new(),
            region: String::new(),
            version: None,
            nodes: None,
            problems: flock_core::Counts::default(),
            nav: BTreeMap::new(),
            metrics: None,
        }
    }

    const PROBE_PATIENCE: Duration = Duration::from_secs(10);
    const PROBE_POLL: Duration = Duration::from_millis(10);

    #[tokio::test]
    async fn every_cluster_with_loco_keeps_its_counts_off_the_loco_pages() {
        let ids = [ClusterId::new("a"), ClusterId::new("b")];
        let clusters = [installed_cluster(), installed_cluster()];
        let (updates, _) = async_channel::unbounded();
        let mut backend = LocoBackend::new(LocoSettings::default(), updates);
        let handles = ids.iter().zip(clusters.iter().map(|(_, handle)| handle));
        backend.sync(handles, ids.first(), Page::ClusterOverview);
        let deadline = Instant::now() + PROBE_PATIENCE;
        while ids
            .iter()
            .any(|id| !backend.observed(id).is_some_and(|o| o.installed))
        {
            assert!(Instant::now() < deadline, "Loco was never detected");
            tokio::time::sleep(PROBE_POLL).await;
        }
        backend.refresh();
        assert_eq!(backend.watches.len(), ids.len());
        let mut summaries: Vec<ClusterSummary> = ids.iter().map(summary).collect();
        let loco = backend.snapshot(&mut summaries, None);
        assert_eq!(loco.len(), ids.len());
        for summary in &summaries {
            assert!(
                summary.nav.contains_key(&Page::LocoApplications),
                "{summary:?}"
            );
        }
        let only = backend.snapshot(&mut summaries, ids.last());
        assert_eq!(only.keys().collect::<Vec<_>>(), [&ids[1]]);
    }

    fn target(kind: &str, api_version: &str) -> ObjectRef {
        ObjectRef {
            api_version: api_version.to_owned(),
            kind: kind.to_owned(),
            namespace: Some("apps".to_owned()),
            name: "podinfo".to_owned(),
        }
    }

    #[tokio::test]
    async fn a_restart_never_touches_a_recreated_workload() {
        let (_manager, handle, traffic, changed) = crate::fake::recreated("Deployment");
        let deployment = target("Deployment", "apps/v1");
        let outcome = restart_workload(&handle, &deployment, &Uid::new(crate::fake::SEEN)).await;
        assert!(
            matches!(outcome, Err(Error::Replaced { .. })),
            "{outcome:?}"
        );
        assert!(!*lock(&changed));
        let sent: Vec<Option<String>> = traffic
            .bodies(
                &http::Method::PATCH,
                "/apis/apps/v1/namespaces/apps/deployments/podinfo",
            )
            .iter()
            .map(|body| crate::fake::sent_uid(body, "/metadata/uid"))
            .collect();
        assert_eq!(sent, [Some(crate::fake::SEEN.to_owned())]);
    }

    #[tokio::test]
    async fn a_cancel_never_deletes_a_recreated_job() {
        let (_manager, handle, traffic, changed) = crate::fake::recreated("Job");
        let job = target("Job", "batch/v1");
        let outcome = cancel_job(&handle, &job, &Uid::new(crate::fake::SEEN)).await;
        assert!(
            matches!(outcome, Err(Error::Replaced { .. })),
            "{outcome:?}"
        );
        assert!(!*lock(&changed));
        let sent: Vec<Option<String>> = traffic
            .bodies(
                &http::Method::DELETE,
                "/apis/batch/v1/namespaces/apps/jobs/podinfo",
            )
            .iter()
            .map(|body| crate::fake::sent_uid(body, "/preconditions/uid"))
            .collect();
        assert_eq!(sent, [Some(crate::fake::SEEN.to_owned())]);
    }

    #[tokio::test]
    async fn loco_writes_without_a_uid_are_refused_before_any_request() {
        let (_manager, handle, traffic, _) = crate::fake::recreated("Job");
        let job = target("Job", "batch/v1");
        let deployment = target("Deployment", "apps/v1");
        let cancel = cancel_job(&handle, &job, &Uid::default()).await;
        let restart = restart_workload(&handle, &deployment, &Uid::default()).await;
        assert!(matches!(cancel, Err(Error::MissingField("metadata.uid"))));
        assert!(matches!(restart, Err(Error::MissingField("metadata.uid"))));
        assert_eq!(traffic.requests(), Vec::new());
    }
}
