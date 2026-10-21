use std::any::Any;
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::{Arc, Mutex};

use futures::{StreamExt, TryStreamExt};
use jiff::Timestamp;
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet};
use k8s_openapi::api::autoscaling::v2::HorizontalPodAutoscaler;
use k8s_openapi::api::batch::v1::{CronJob, Job};
use k8s_openapi::api::core::v1::{
    ConfigMap, Event, Namespace, Node, PersistentVolume, PersistentVolumeClaim, Pod, ResourceQuota,
    Secret, Service, ServiceAccount,
};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use k8s_openapi::api::networking::v1::{Ingress, NetworkPolicy};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{FieldsV1, ManagedFieldsEntry};
use kube::api::{DynamicObject, PartialObjectMeta};
use kube::core::{ApiResource, GroupVersionKind};
use kube::runtime::reflector::{self, ObjectRef, Store};
use kube::runtime::{WatchStreamExt, watcher};
use kube::{Api, Client, Resource};
use serde::de::DeserializeOwned;
use tokio::runtime::Handle;
use tokio::sync::mpsc::UnboundedSender;

use crate::detail::last_seen;
use crate::error::Error;
use crate::manager::lock;
use crate::task::TaskGuard;

const LAST_APPLIED: &str = "kubectl.kubernetes.io/last-applied-configuration";
const SPEC_FIELD: &str = "f:spec";
const SUSPEND_KEY: &str = "f:suspend";
const SUSPEND_FIELD: &str = "/f:spec/f:suspend";
const LISTING_SLACK: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CustomKind {
    pub group: String,
    pub version: String,
    pub kind: String,
}

impl CustomKind {
    pub fn new(group: &str, version: &str, kind: &str) -> Self {
        Self {
            group: group.to_owned(),
            version: version.to_owned(),
            kind: kind.to_owned(),
        }
    }

    pub fn gvk(&self) -> GroupVersionKind {
        GroupVersionKind::gvk(&self.group, &self.version, &self.kind)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WatchKind {
    Namespaces,
    Nodes,
    Pods,
    Deployments,
    StatefulSets,
    DaemonSets,
    ReplicaSets,
    Jobs,
    CronJobs,
    Services,
    EndpointSlices,
    Ingresses,
    ConfigMaps,
    Secrets,
    ServiceAccounts,
    NetworkPolicies,
    ResourceQuotas,
    Events,
    PersistentVolumeClaims,
    PersistentVolumes,
    HorizontalPodAutoscalers,
    CustomResourceDefinitions,
    HelmStorage,
    Custom(CustomKind),
}

impl WatchKind {
    pub fn name(&self) -> &str {
        match self {
            Self::Namespaces => "Namespace",
            Self::Nodes => "Node",
            Self::Pods => "Pod",
            Self::Deployments => "Deployment",
            Self::StatefulSets => "StatefulSet",
            Self::DaemonSets => "DaemonSet",
            Self::ReplicaSets => "ReplicaSet",
            Self::Jobs => "Job",
            Self::CronJobs => "CronJob",
            Self::Services => "Service",
            Self::EndpointSlices => "EndpointSlice",
            Self::Ingresses => "Ingress",
            Self::ConfigMaps => "ConfigMap",
            Self::Secrets => "Secret",
            Self::ServiceAccounts => "ServiceAccount",
            Self::NetworkPolicies => "NetworkPolicy",
            Self::ResourceQuotas => "ResourceQuota",
            Self::Events => "Event",
            Self::PersistentVolumeClaims => "PersistentVolumeClaim",
            Self::PersistentVolumes => "PersistentVolume",
            Self::HorizontalPodAutoscalers => "HorizontalPodAutoscaler",
            Self::CustomResourceDefinitions => "CustomResourceDefinition",
            Self::HelmStorage => "Helm release",
            Self::Custom(kind) => &kind.kind,
        }
    }
}

pub trait Watched: Resource + Clone + Debug + Send + Sync + 'static {
    const KIND: WatchKind;
}

macro_rules! watched {
    ($($ty:ty => $kind:ident),* $(,)?) => {
        $(impl Watched for $ty {
            const KIND: WatchKind = WatchKind::$kind;
        })*
    };
}

watched! {
    Namespace => Namespaces,
    Node => Nodes,
    Pod => Pods,
    Deployment => Deployments,
    StatefulSet => StatefulSets,
    DaemonSet => DaemonSets,
    ReplicaSet => ReplicaSets,
    Job => Jobs,
    CronJob => CronJobs,
    Service => Services,
    EndpointSlice => EndpointSlices,
    Ingress => Ingresses,
    PartialObjectMeta<ConfigMap> => ConfigMaps,
    PartialObjectMeta<Secret> => Secrets,
    PartialObjectMeta<ServiceAccount> => ServiceAccounts,
    NetworkPolicy => NetworkPolicies,
    ResourceQuota => ResourceQuotas,
    Event => Events,
    PersistentVolumeClaim => PersistentVolumeClaims,
    PersistentVolume => PersistentVolumes,
    HorizontalPodAutoscaler => HorizontalPodAutoscalers,
    CustomResourceDefinition => CustomResourceDefinitions,
    Secret => HelmStorage,
}

#[derive(Debug)]
pub enum SignalEvent {
    Changed,
    Failed(Error),
    Dropped,
}

#[derive(Debug)]
pub struct Signal {
    pub kind: WatchKind,
    pub event: SignalEvent,
}

pub struct RunningWatch {
    store: Arc<dyn Any + Send + Sync>,
    _task: TaskGuard,
}

fn into_error(err: watcher::Error) -> Error {
    match err {
        watcher::Error::InitialListFailed(inner) => Error::Kube(inner),
        watcher::Error::WatchStartFailed(inner) => Error::Kube(inner),
        watcher::Error::WatchFailed(inner) => Error::Kube(inner),
        watcher::Error::WatchError(status) => Error::Kube(kube::Error::Api(status)),
        watcher::Error::NoResourceVersion => Error::MissingField("resourceVersion"),
    }
}

async fn forward<K, S>(stream: S, kind: WatchKind, signals: UnboundedSender<Signal>)
where
    S: futures::Stream<Item = Result<watcher::Event<K>, watcher::Error>> + Send,
{
    let mut stream = std::pin::pin!(stream);
    while let Some(item) = stream.next().await {
        let event = match item {
            Ok(watcher::Event::Init | watcher::Event::InitApply(_)) => continue,
            Ok(_) => SignalEvent::Changed,
            Err(err) => SignalEvent::Failed(into_error(err)),
        };
        let signal = Signal {
            kind: kind.clone(),
            event,
        };
        if signals.send(signal).is_err() {
            return;
        }
    }
}

fn spawn_full<K>(runtime: &Handle, client: Client, signals: UnboundedSender<Signal>) -> RunningWatch
where
    K: Watched + DeserializeOwned + Resource<DynamicType = ()>,
    K::Scope: Send,
{
    spawn_api(runtime, Api::<K>::all(client), signals)
}

fn spawn_api<K>(runtime: &Handle, api: Api<K>, signals: UnboundedSender<Signal>) -> RunningWatch
where
    K: Watched + DeserializeOwned + Resource<DynamicType = ()>,
    K::Scope: Send,
{
    spawn_trimmed(runtime, api, signals, trim_metadata)
}

fn spawn_trimmed<K>(
    runtime: &Handle,
    api: Api<K>,
    signals: UnboundedSender<Signal>,
    trim: fn(&mut K),
) -> RunningWatch
where
    K: Watched + DeserializeOwned + Resource<DynamicType = ()>,
    K::Scope: Send,
{
    spawn_selected(runtime, api, watcher::Config::default(), signals, trim)
}

fn spawn_selected<K>(
    runtime: &Handle,
    api: Api<K>,
    config: watcher::Config,
    signals: UnboundedSender<Signal>,
    trim: fn(&mut K),
) -> RunningWatch
where
    K: Watched + DeserializeOwned + Resource<DynamicType = ()>,
    K::Scope: Send,
{
    let writer = reflector::store::Writer::<K>::default();
    let store = writer.as_reader();
    let stream = reflector::reflector(writer, watcher(api, config).default_backoff().modify(trim));
    let _guard = runtime.enter();
    RunningWatch {
        store: Arc::new(store),
        _task: TaskGuard::spawn(forward(stream, K::KIND, signals)),
    }
}

pub fn spawn_custom(
    runtime: &Handle,
    client: Client,
    kind: &CustomKind,
    resource: &ApiResource,
    signals: UnboundedSender<Signal>,
) -> RunningWatch {
    let writer = reflector::store::Writer::<DynamicObject>::new(resource.clone());
    let store = writer.as_reader();
    let api: Api<DynamicObject> = Api::all_with(client, resource);
    let stream = reflector::reflector(
        writer,
        watcher(api, watcher::Config::default())
            .default_backoff()
            .modify(trim_metadata),
    );
    let _guard = runtime.enter();
    RunningWatch {
        store: Arc::new(store),
        _task: TaskGuard::spawn(forward(stream, WatchKind::Custom(kind.clone()), signals)),
    }
}

pub fn spawn_builtin(
    kind: &WatchKind,
    event_window: usize,
    runtime: &Handle,
    client: Client,
    signals: UnboundedSender<Signal>,
) -> Option<RunningWatch> {
    let running = match kind {
        WatchKind::Namespaces => spawn_full::<Namespace>(runtime, client, signals),
        WatchKind::Nodes => spawn_full::<Node>(runtime, client, signals),
        WatchKind::Pods => spawn_full::<Pod>(runtime, client, signals),
        WatchKind::Deployments => spawn_full::<Deployment>(runtime, client, signals),
        WatchKind::StatefulSets => spawn_full::<StatefulSet>(runtime, client, signals),
        WatchKind::DaemonSets => spawn_full::<DaemonSet>(runtime, client, signals),
        WatchKind::ReplicaSets => spawn_full::<ReplicaSet>(runtime, client, signals),
        WatchKind::Jobs => spawn_full::<Job>(runtime, client, signals),
        WatchKind::CronJobs => spawn_full::<CronJob>(runtime, client, signals),
        WatchKind::Services => spawn_full::<Service>(runtime, client, signals),
        WatchKind::EndpointSlices => spawn_full::<EndpointSlice>(runtime, client, signals),
        WatchKind::Ingresses => spawn_full::<Ingress>(runtime, client, signals),
        WatchKind::ConfigMaps => {
            spawn_full::<PartialObjectMeta<ConfigMap>>(runtime, client, signals)
        }
        WatchKind::Secrets => spawn_full::<PartialObjectMeta<Secret>>(runtime, client, signals),
        WatchKind::ServiceAccounts => {
            spawn_full::<PartialObjectMeta<ServiceAccount>>(runtime, client, signals)
        }
        WatchKind::NetworkPolicies => spawn_full::<NetworkPolicy>(runtime, client, signals),
        WatchKind::ResourceQuotas => spawn_full::<ResourceQuota>(runtime, client, signals),
        WatchKind::Events => spawn_events(runtime, client, event_window, signals),
        WatchKind::PersistentVolumeClaims => {
            spawn_full::<PersistentVolumeClaim>(runtime, client, signals)
        }
        WatchKind::PersistentVolumes => spawn_full::<PersistentVolume>(runtime, client, signals),
        WatchKind::HorizontalPodAutoscalers => {
            spawn_full::<HorizontalPodAutoscaler>(runtime, client, signals)
        }
        WatchKind::CustomResourceDefinitions => {
            spawn_trimmed(runtime, Api::all(client), signals, trim_definition)
        }
        WatchKind::HelmStorage => {
            let config = watcher::Config::default().fields(crate::helm::FIELD_SELECTOR);
            spawn_selected::<Secret>(runtime, Api::all(client), config, signals, trim_metadata)
        }
        WatchKind::Custom(_) => return None,
    };
    Some(running)
}

pub fn snapshot<K: Watched>(running: &RunningWatch) -> Vec<Arc<K>>
where
    K::DynamicType: Eq + std::hash::Hash + Clone,
{
    if let Some(store) = running.store.downcast_ref::<Store<K>>() {
        return store.state();
    }
    let Some(window) = running.store.downcast_ref::<EventWindow>() else {
        return Vec::new();
    };
    let events: Box<dyn Any> = Box::new(window.state());
    events
        .downcast::<Vec<Arc<K>>>()
        .map(|events| *events)
        .unwrap_or_default()
}

#[derive(Default)]
struct WindowState {
    live: Events,
    listing: Option<Events>,
}

pub struct EventWindow {
    limit: usize,
    state: Mutex<WindowState>,
}

impl EventWindow {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            state: Mutex::new(WindowState::default()),
        }
    }

    pub fn state(&self) -> Vec<Arc<Event>> {
        lock(&self.state).live.values().cloned().collect()
    }

    pub fn apply(&self, event: &watcher::Event<Event>) {
        let mut state = lock(&self.state);
        match event {
            watcher::Event::Init => state.listing = Some(HashMap::new()),
            watcher::Event::InitApply(object) => {
                let listing = state.listing.get_or_insert_default();
                listing.insert(ObjectRef::from_obj(object), Arc::new(object.clone()));
                if listing.len() > self.limit.saturating_mul(LISTING_SLACK) {
                    keep_newest(listing, self.limit);
                }
            }
            watcher::Event::InitDone => {
                if let Some(mut listed) = state.listing.take() {
                    keep_newest(&mut listed, self.limit);
                    state.live = listed;
                }
            }
            watcher::Event::Apply(object) => {
                let live = &mut state.live;
                live.insert(ObjectRef::from_obj(object), Arc::new(object.clone()));
                if live.len() > self.limit {
                    drop_oldest(live);
                }
            }
            watcher::Event::Delete(object) => {
                state.live.remove(&ObjectRef::from_obj(object));
            }
        }
    }
}

type Events = HashMap<ObjectRef<Event>, Arc<Event>>;

fn keep_newest(events: &mut Events, limit: usize) {
    if events.len() <= limit {
        return;
    }
    let mut ranked: Vec<(Option<Timestamp>, ObjectRef<Event>)> = events
        .iter()
        .map(|(key, event)| (last_seen(event), key.clone()))
        .collect();
    ranked.sort_unstable_by_key(|ranked| std::cmp::Reverse(ranked.0));
    for (_, key) in ranked.into_iter().skip(limit) {
        events.remove(&key);
    }
}

fn drop_oldest(events: &mut Events) {
    let oldest = events
        .iter()
        .min_by_key(|(_, event)| last_seen(event))
        .map(|(key, _)| key.clone());
    if let Some(oldest) = oldest {
        events.remove(&oldest);
    }
}

fn spawn_events(
    runtime: &Handle,
    client: Client,
    limit: usize,
    signals: UnboundedSender<Signal>,
) -> RunningWatch {
    let api: Api<Event> = Api::all(client);
    let window = Arc::new(EventWindow::new(limit));
    let writer = window.clone();
    let stream = watcher(api, watcher::Config::default())
        .default_backoff()
        .modify(trim_metadata)
        .inspect_ok(move |event| writer.apply(event));
    let _guard = runtime.enter();
    RunningWatch {
        store: window,
        _task: TaskGuard::spawn(forward(stream, WatchKind::Events, signals)),
    }
}

pub fn custom_snapshot(running: &RunningWatch) -> Vec<Arc<DynamicObject>> {
    running
        .store
        .downcast_ref::<Store<DynamicObject>>()
        .map(Store::state)
        .unwrap_or_default()
}

fn trim_metadata<K: Resource>(object: &mut K) {
    let metadata = object.meta_mut();
    let suspenders: Vec<ManagedFieldsEntry> = metadata
        .managed_fields
        .take()
        .unwrap_or_default()
        .into_iter()
        .filter(owns_suspend)
        .map(|entry| ManagedFieldsEntry {
            fields_v1: Some(FieldsV1(suspend_fields())),
            time: entry.time,
            manager: entry.manager,
            ..ManagedFieldsEntry::default()
        })
        .collect();
    metadata.managed_fields = (!suspenders.is_empty()).then_some(suspenders);
    if let Some(annotations) = &mut metadata.annotations {
        annotations.remove(LAST_APPLIED);
    }
}

fn trim_definition(definition: &mut CustomResourceDefinition) {
    trim_metadata(definition);
    for version in &mut definition.spec.versions {
        version.schema = None;
    }
}

pub fn owns_suspend(entry: &ManagedFieldsEntry) -> bool {
    entry
        .fields_v1
        .as_ref()
        .is_some_and(|fields| fields.0.pointer(SUSPEND_FIELD).is_some())
}

fn suspend_fields() -> serde_json::Value {
    serde_json::json!({ SPEC_FIELD: { SUSPEND_KEY: {} } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cached_metadata_drops_apply_payload_but_keeps_identity_and_flux_annotations() {
        let parsed = serde_json::from_value::<PartialObjectMeta<ConfigMap>>(json!({
            "metadata": {
                "name": "config", "namespace": "scale", "uid": "uid", "resourceVersion": "42",
                "labels": {"app": "scale"},
                "annotations": {
                    "kubectl.kubernetes.io/last-applied-configuration": "large payload",
                    "reconcile.fluxcd.io/requestedAt": "now"
                },
                "managedFields": [{"manager": "kubectl", "operation": "Update", "apiVersion": "v1"}]
            }
        }));
        let Ok(mut object) = parsed else {
            panic!("fixture failed")
        };
        trim_metadata(&mut object);
        assert!(object.metadata.managed_fields.is_none());
        let Some(annotations) = object.metadata.annotations else {
            panic!("annotations lost")
        };
        assert!(!annotations.contains_key("kubectl.kubernetes.io/last-applied-configuration"));
        assert_eq!(
            annotations
                .get("reconcile.fluxcd.io/requestedAt")
                .map(String::as_str),
            Some("now")
        );
        assert_eq!(object.metadata.name.as_deref(), Some("config"));
        assert_eq!(object.metadata.namespace.as_deref(), Some("scale"));
        assert_eq!(object.metadata.uid.as_deref(), Some("uid"));
        assert_eq!(object.metadata.resource_version.as_deref(), Some("42"));
        let Some(labels) = object.metadata.labels else {
            panic!("labels lost")
        };
        assert_eq!(labels.get("app").map(String::as_str), Some("scale"));
    }

    #[test]
    fn cached_metadata_keeps_only_who_last_wrote_spec_suspend_and_when() {
        let parsed = serde_json::from_value::<DynamicObject>(json!({
            "apiVersion": "kustomize.toolkit.fluxcd.io/v1", "kind": "Kustomization",
            "metadata": {
                "name": "apps", "namespace": "flux-system",
                "managedFields": [
                    {"manager": "kubectl", "operation": "Update", "time": "2026-10-01T10:00:00Z",
                     "fieldsType": "FieldsV1",
                     "fieldsV1": {"f:spec": {"f:interval": {}, "f:path": {}}}},
                    {"manager": "flux", "operation": "Update", "time": "2026-10-12T09:30:00Z",
                     "fieldsType": "FieldsV1",
                     "fieldsV1": {"f:spec": {"f:suspend": {}}}},
                    {"manager": "kustomize-controller", "operation": "Update",
                     "subresource": "status", "time": "2026-10-14T18:00:00Z",
                     "fieldsType": "FieldsV1", "fieldsV1": {"f:status": {"f:conditions": {}}}}
                ]
            },
            "spec": {"suspend": true}
        }));
        let Ok(mut object) = parsed else {
            panic!("fixture failed")
        };
        trim_metadata(&mut object);
        let kept = object.metadata.managed_fields.unwrap_or_default();
        let summary: Vec<(Option<&str>, Option<String>, bool)> = kept
            .iter()
            .map(|entry| {
                let time = entry.time.as_ref().map(|t| t.0.to_string());
                (entry.manager.as_deref(), time, owns_suspend(entry))
            })
            .collect();
        assert_eq!(
            summary,
            [(Some("flux"), Some("2026-10-12T09:30:00Z".to_owned()), true)]
        );
    }

    fn event(name: &str, minute: u32) -> Event {
        let parsed = serde_json::from_value(json!({
            "metadata": { "name": name, "namespace": "apps" },
            "involvedObject": {},
            "lastTimestamp": format!("2026-10-14T10:{minute:02}:00Z")
        }));
        let Ok(event) = parsed else {
            panic!("event fixture")
        };
        event
    }

    fn names(window: &EventWindow) -> Vec<String> {
        let mut names: Vec<String> = window
            .state()
            .iter()
            .filter_map(|event| event.metadata.name.clone())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_event_window_keeps_only_the_newest_events() {
        let window = EventWindow::new(2);
        window.apply(&watcher::Event::Init);
        for (name, minute) in [("old", 1), ("newer", 5), ("newest", 9), ("older", 2)] {
            window.apply(&watcher::Event::InitApply(event(name, minute)));
        }
        assert_eq!(names(&window), Vec::<String>::new());
        window.apply(&watcher::Event::InitDone);
        assert_eq!(names(&window), ["newer", "newest"]);
        window.apply(&watcher::Event::Apply(event("latest", 12)));
        assert_eq!(names(&window), ["latest", "newest"]);
        window.apply(&watcher::Event::Apply(event("newest", 13)));
        assert_eq!(names(&window), ["latest", "newest"]);
        window.apply(&watcher::Event::Delete(event("latest", 12)));
        assert_eq!(names(&window), ["newest"]);
    }

    #[test]
    fn a_relist_replaces_the_window_only_when_it_completes() {
        let window = EventWindow::new(3);
        window.apply(&watcher::Event::Apply(event("kept", 1)));
        window.apply(&watcher::Event::Init);
        window.apply(&watcher::Event::InitApply(event("relisted", 2)));
        assert_eq!(names(&window), ["kept"]);
        window.apply(&watcher::Event::InitDone);
        assert_eq!(names(&window), ["relisted"]);
    }
}
