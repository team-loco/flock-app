use std::sync::Arc;

use flock_core::cluster::DetailObject;
use flock_core::crd::CrdKey;
use flock_core::related::{AUTOSCALER, Fact, GATEWAY_API, HTTP_ROUTE, Key, PERSISTENT_VOLUME};
use flock_core::resources::Resource;
use flock_core::{ResourceRef, Status};
use jiff::Timestamp;
use k8s_openapi::api::autoscaling::v2::HorizontalPodAutoscaler;
use k8s_openapi::api::core::v1::PersistentVolume;
use kube::api::DynamicObject;
use serde::Deserialize;

use crate::clock::age;
use crate::compare::autoscaler_label;
use crate::lists::row;
use crate::manager::{ClusterHandle, Subscription};
use crate::resources::{CustomKind, WatchKind};

pub const KINDS: [&str; 2] = [PERSISTENT_VOLUME, AUTOSCALER];
pub const WATCHES: [WatchKind; 2] = [
    WatchKind::PersistentVolumes,
    WatchKind::HorizontalPodAutoscalers,
];
const SEPARATOR: &str = " · ";
const REPLICAS: &str = "replicas";
const CONDITION_FALSE: &str = "False";
const CONDITION_TRUE: &str = "True";
const ACCEPTED: &str = "Accepted";
const SERVICE_KIND: &str = "Service";
const CORE_GROUP: &str = "";
const SCALING_CONDITIONS: [&str; 2] = ["AbleToScale", "ScalingActive"];
const BOUND: &str = "Bound";
const AVAILABLE: &str = "Available";
const PENDING: &str = "Pending";
const RELEASED: &str = "Released";

fn autoscaler_status(item: &HorizontalPodAutoscaler) -> Status {
    let stuck = item
        .status
        .as_ref()
        .and_then(|status| status.conditions.as_ref())
        .into_iter()
        .flatten()
        .filter(|condition| SCALING_CONDITIONS.contains(&condition.type_.as_str()))
        .any(|condition| condition.status == CONDITION_FALSE);
    if stuck { Status::Warn } else { Status::Ok }
}

pub fn autoscaler_row(item: &HorizontalPodAutoscaler, now: Timestamp) -> Resource {
    let current = item
        .status
        .as_ref()
        .and_then(|status| status.current_replicas)
        .unwrap_or_default();
    let note = format!("{current} {REPLICAS}{SEPARATOR}{}", autoscaler_label(item));
    row(&item.metadata, autoscaler_status(item), vec![note], now)
}

pub fn autoscaler(item: &HorizontalPodAutoscaler, now: Timestamp) -> Fact {
    let row = autoscaler_row(item, now);
    let target = &item.spec.scale_target_ref;
    Fact {
        status: row.status,
        note: row.cells.first().cloned().unwrap_or_default(),
        age: row.age,
        labels: row.labels,
        uses: vec![Key::new(target.kind.clone(), target.name.clone())],
        ..Fact::new(AUTOSCALER, item.metadata.namespace.as_deref(), &row.name)
    }
}

fn volume_status(phase: &str) -> Status {
    match phase {
        BOUND => Status::Ok,
        AVAILABLE => Status::Ok,
        PENDING => Status::Progress,
        RELEASED => Status::Warn,
        _ => Status::Err,
    }
}

pub fn volume_row(item: &PersistentVolume, now: Timestamp) -> Resource {
    let phase = item
        .status
        .as_ref()
        .and_then(|status| status.phase.clone())
        .unwrap_or_default();
    let status = volume_status(&phase);
    row(&item.metadata, status, vec![phase], now)
}

pub fn volume(item: &PersistentVolume, now: Timestamp) -> Fact {
    let row = volume_row(item, now);
    Fact {
        status: row.status,
        note: row.cells.first().cloned().unwrap_or_default(),
        age: row.age,
        labels: row.labels,
        ..Fact::new(PERSISTENT_VOLUME, None, &row.name)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Route {
    spec: RouteSpec,
    status: RouteStatus,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RouteSpec {
    rules: Vec<RouteRule>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct RouteRule {
    backend_refs: Vec<BackendRef>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BackendRef {
    group: Option<String>,
    kind: Option<String>,
    name: String,
    namespace: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RouteStatus {
    parents: Vec<ParentStatus>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ParentStatus {
    conditions: Vec<RouteCondition>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RouteCondition {
    #[serde(rename = "type")]
    kind: String,
    status: String,
    reason: String,
}

impl BackendRef {
    fn service_in(&self, namespace: Option<&str>) -> Option<&str> {
        let core = self
            .group
            .as_deref()
            .is_none_or(|group| group == CORE_GROUP);
        let service = self.kind.as_deref().is_none_or(|kind| kind == SERVICE_KIND);
        let local = self.namespace.is_none() || self.namespace.as_deref() == namespace;
        (core && service && local && !self.name.is_empty()).then_some(self.name.as_str())
    }
}

fn acceptance(status: &RouteStatus) -> (Status, String) {
    let accepted: Vec<&RouteCondition> = status
        .parents
        .iter()
        .flat_map(|parent| parent.conditions.iter())
        .filter(|condition| condition.kind == ACCEPTED)
        .collect();
    let refused = accepted
        .iter()
        .find(|condition| condition.status != CONDITION_TRUE);
    match (refused, accepted.is_empty()) {
        (Some(condition), _) => (Status::Err, condition.reason.clone()),
        (None, true) => (Status::Progress, String::new()),
        (None, false) => (Status::Ok, ACCEPTED.to_owned()),
    }
}

pub fn route_key() -> CrdKey {
    CrdKey::new(GATEWAY_API, HTTP_ROUTE)
}

pub fn route(item: &DynamicObject, now: Timestamp) -> Fact {
    let meta = &item.metadata;
    let namespace = meta.namespace.as_deref();
    let parsed = serde_json::from_value::<Route>(item.data.clone());
    let route = parsed.unwrap_or_default();
    let mut uses: Vec<Key> = Vec::new();
    let services = route
        .spec
        .rules
        .iter()
        .flat_map(|rule| rule.backend_refs.iter())
        .filter_map(|backend| backend.service_in(namespace));
    for service in services {
        let key = Key::new(SERVICE_KIND, service);
        if !uses.contains(&key) {
            uses.push(key);
        }
    }
    let (status, note) = acceptance(&route.status);
    Fact {
        status,
        note,
        age: age(meta, now),
        labels: meta.labels.clone().unwrap_or_default(),
        owner: crate::related::owner(meta),
        uses,
        ..Fact::new(
            &route_key().qualified(),
            namespace,
            meta.name.as_deref().unwrap_or_default(),
        )
    }
}

pub fn route_kind(handle: &ClusterHandle) -> Option<CustomKind> {
    crate::crd::find(handle, &route_key()).map(|crd| crate::crd::custom_kind(&crd))
}

pub fn watch_routes(handle: &ClusterHandle) -> Option<Subscription> {
    let crd = crate::crd::find(handle, &route_key())?;
    let kind = crate::crd::custom_kind(&crd);
    Some(handle.subscribe_resource(kind, crate::crd::api_resource(&crd)))
}

pub fn routes(handle: &ClusterHandle) -> Vec<Arc<DynamicObject>> {
    route_kind(handle).map_or_else(Vec::new, |kind| handle.custom_objects(&kind))
}

fn named<K: kube::Resource>(items: &[Arc<K>], target: &ResourceRef) -> Option<Arc<K>> {
    items
        .iter()
        .find(|item| {
            let meta = item.meta();
            meta.name.as_deref() == Some(target.name.as_str())
                && meta.namespace.as_deref() == target.namespace.as_deref()
        })
        .cloned()
}

pub fn objects(
    handle: &ClusterHandle,
    targets: &[&ResourceRef],
    yaml: &[&(ResourceRef, String)],
    now: Timestamp,
) -> Vec<DetailObject> {
    let mut built: Vec<DetailObject> = Vec::new();
    for target in targets.iter().copied() {
        let row = match target.kind.as_str() {
            PERSISTENT_VOLUME => named(&handle.objects::<PersistentVolume>(), target)
                .map(|item| volume_row(&item, now)),
            AUTOSCALER => named(&handle.objects::<HorizontalPodAutoscaler>(), target)
                .map(|item| autoscaler_row(&item, now)),
            _ => None,
        };
        let Some(mut row) = row else {
            continue;
        };
        if let Some((_, text)) = yaml.iter().find(|(shown, _)| shown == target) {
            row.yaml.clone_from(text);
        }
        if !built.iter().any(|object| &object.target == target) {
            built.push(DetailObject {
                target: target.clone(),
                row,
            });
        }
    }
    built
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::problems::tests::{now, parse};

    #[test]
    fn an_autoscaler_points_at_its_scale_target_and_reports_stuck_scaling() {
        let hpa: Arc<HorizontalPodAutoscaler> = parse(json!({
            "metadata": { "name": "podinfo", "namespace": "web" },
            "spec": {
                "scaleTargetRef": { "apiVersion": "apps/v1", "kind": "Deployment", "name": "podinfo" },
                "minReplicas": 2, "maxReplicas": 4
            },
            "status": {
                "currentReplicas": 2, "desiredReplicas": 2,
                "conditions": [{ "type": "ScalingActive", "status": "False", "reason": "FailedGetResourceMetric" }]
            }
        }));
        let fact = autoscaler(&hpa, now());
        assert_eq!(fact.kind, AUTOSCALER);
        assert_eq!(fact.namespace.as_deref(), Some("web"));
        assert_eq!(fact.uses, [Key::new("Deployment", "podinfo")]);
        assert_eq!(fact.note, "2 replicas · 2–4");
        assert_eq!(fact.status, Status::Warn);
    }

    #[test]
    fn volumes_are_cluster_scoped_and_read_their_phase() {
        let pv: Arc<PersistentVolume> = parse(json!({
            "metadata": { "name": "pv-1" },
            "status": { "phase": "Released" }
        }));
        let fact = volume(&pv, now());
        assert_eq!(
            (fact.kind.as_str(), fact.namespace, fact.note.as_str()),
            (PERSISTENT_VOLUME, None, "Released")
        );
        assert_eq!(fact.status, Status::Warn);
        assert_eq!(volume_status(BOUND), Status::Ok);
        assert_eq!(volume_status("Failed"), Status::Err);
    }

    fn http_route(status: &serde_json::Value) -> DynamicObject {
        let parsed = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": { "name": "web", "namespace": "shop" },
            "spec": {
                "parentRefs": [{ "name": "eg", "namespace": "envoy-gateway-system" }],
                "rules": [
                    { "backendRefs": [{ "name": "cart", "port": 80 }, { "kind": "Service", "name": "web", "port": 80 }] },
                    { "backendRefs": [
                        { "name": "cart", "port": 80 },
                        { "name": "elsewhere", "namespace": "other", "port": 80 },
                        { "group": "example.com", "kind": "Bucket", "name": "assets" }
                    ] }
                ]
            },
            "status": status
        }));
        let Ok(route) = parsed else {
            panic!("route fixture");
        };
        route
    }

    #[test]
    fn a_route_uses_its_local_backend_services_under_its_qualified_kind() {
        let accepted = json!({ "parents": [{ "conditions": [{ "type": "Accepted", "status": "True", "reason": "Accepted" }] }] });
        let fact = route(&http_route(&accepted), now());
        assert_eq!(fact.kind, "HTTPRoute.gateway.networking.k8s.io");
        assert_eq!(
            fact.uses,
            [
                Key::new(SERVICE_KIND, "cart"),
                Key::new(SERVICE_KIND, "web")
            ]
        );
        assert_eq!((fact.status, fact.note.as_str()), (Status::Ok, ACCEPTED));
        let refused = json!({ "parents": [
            { "conditions": [{ "type": "Accepted", "status": "True", "reason": "Accepted" }] },
            { "conditions": [{ "type": "Accepted", "status": "False", "reason": "NotAllowedByListeners" }] }
        ] });
        let fact = route(&http_route(&refused), now());
        assert_eq!(
            (fact.status, fact.note.as_str()),
            (Status::Err, "NotAllowedByListeners")
        );
        let fact = route(&http_route(&json!({})), now());
        assert_eq!(fact.status, Status::Progress);
    }
}
