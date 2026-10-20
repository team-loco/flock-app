use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use flock_core::cluster_changes::{ChangeKind, ClusterChange};
use flock_core::overview::TimeRange;
use flock_core::{ClusterId, ResourceRef, Status};
use jiff::Timestamp;
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use k8s_openapi::api::core::v1::{Event, Namespace, Node, PodTemplateSpec};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use crate::clock::between;
use crate::detail::last_seen;
use crate::pods::Clock;

const WINDOW: Duration = TimeRange::Week.duration();
const DAY: Duration = TimeRange::Day.duration();
const CLOCK_FORMAT: &str = "%H:%M";
const DATE_FORMAT: &str = "%b %-d";
const OBSERVED_LIMIT: usize = 200;
const REVISION_ANNOTATION: &str = "deployment.kubernetes.io/revision";
const PROGRESSING: &str = "Progressing";
const ROLLED_OUT: &str = "NewReplicaSetAvailable";
const ROLLING: &str = "ReplicaSetUpdated";
const STALLED: &str = "ProgressDeadlineExceeded";
const NOT_SCHEDULABLE: &str = "NodeNotSchedulable";
const SCHEDULABLE: &str = "NodeSchedulable";
const NODE: &str = "Node";
const INSTANCE_TYPE: &str = "node.kubernetes.io/instance-type";
const FLUX_KUSTOMIZATION: &str = "kustomize.toolkit.fluxcd.io/name";
const FLUX_HELM: &str = "helm.toolkit.fluxcd.io/name";
const MANAGED_BY: &str = "app.kubernetes.io/managed-by";
const FLUX: &str = "flux";
const IMAGE_SEPARATOR: &str = ", ";

pub struct Change {
    pub at: Timestamp,
    pub kind: ChangeKind,
    pub status: Option<Status>,
    pub target: ResourceRef,
    pub what: String,
    pub by: String,
}

impl Change {
    pub fn stamp(self, clock: &Clock<'_>) -> Option<ClusterChange> {
        let age = between(self.at, clock.now);
        if age > WINDOW {
            return None;
        }
        let format = if age < DAY { CLOCK_FORMAT } else { DATE_FORMAT };
        let at = self
            .at
            .to_zoned(clock.zone.clone())
            .strftime(format)
            .to_string();
        Some(ClusterChange {
            age,
            at,
            kind: self.kind,
            status: self.status,
            target: self.target,
            what: self.what,
            by: self.by,
        })
    }
}

pub fn target(cluster: &ClusterId, kind: &str, meta: &ObjectMeta) -> ResourceRef {
    ResourceRef {
        cluster: cluster.clone(),
        kind: kind.to_owned(),
        namespace: meta.namespace.clone(),
        name: meta.name.clone().unwrap_or_default(),
    }
}

fn label<'a>(meta: &'a ObjectMeta, key: &str) -> Option<&'a String> {
    meta.labels.as_ref().and_then(|labels| labels.get(key))
}

fn by(meta: &ObjectMeta) -> String {
    let flux = label(meta, FLUX_KUSTOMIZATION).or_else(|| label(meta, FLUX_HELM));
    if let Some(owner) = flux {
        return format!("{FLUX} · {owner}");
    }
    label(meta, MANAGED_BY).map_or_else(String::new, |value| value.to_lowercase())
}

fn created(meta: &ObjectMeta) -> Option<Timestamp> {
    meta.creation_timestamp.as_ref().map(|time| time.0)
}

fn rollout(cluster: &ClusterId, item: &Deployment) -> Option<Change> {
    let meta = &item.metadata;
    let condition = item
        .status
        .as_ref()
        .and_then(|status| status.conditions.as_ref())
        .into_iter()
        .flatten()
        .find(|condition| condition.type_ == PROGRESSING)?;
    let at = condition
        .last_update_time
        .as_ref()
        .or(condition.last_transition_time.as_ref())?
        .0;
    let (suffix, status) = match condition.reason.as_deref()? {
        ROLLED_OUT => ("", None),
        ROLLING => (" rolling out", None),
        STALLED => (" stalled", Some(Status::Err)),
        _ => return None,
    };
    let name = meta.name.clone().unwrap_or_default();
    let revision = meta
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.get(REVISION_ANNOTATION))
        .map(|revision| format!(" rev {revision}"))
        .unwrap_or_default();
    let namespace = meta.namespace.clone().unwrap_or_default();
    Some(Change {
        at,
        kind: ChangeKind::Rollout,
        status,
        target: target(cluster, "Deployment", meta),
        what: format!("deploy/{name}{revision}{suffix} · {namespace}"),
        by: by(meta),
    })
}

pub fn rollouts(cluster: &ClusterId, deployments: &[Arc<Deployment>]) -> Vec<Change> {
    deployments
        .iter()
        .filter_map(|item| rollout(cluster, item))
        .collect()
}

fn joined(cluster: &ClusterId, node: &Node) -> Option<Change> {
    let meta = &node.metadata;
    let name = meta.name.clone().unwrap_or_default();
    let what = match label(meta, INSTANCE_TYPE) {
        Some(instance) => format!("{name} joined · {instance}"),
        None => format!("{name} joined"),
    };
    Some(Change {
        at: created(meta)?,
        kind: ChangeKind::Node,
        status: None,
        target: target(cluster, NODE, meta),
        what,
        by: String::new(),
    })
}

fn schedulability(cluster: &ClusterId, event: &Event) -> Option<Change> {
    let object = &event.involved_object;
    if object.kind.as_deref() != Some(NODE) {
        return None;
    }
    let (verb, status) = match event.reason.as_deref()? {
        NOT_SCHEDULABLE => ("cordoned", Some(Status::Warn)),
        SCHEDULABLE => ("uncordoned", None),
        _ => return None,
    };
    let name = object.name.clone()?;
    let source = event
        .source
        .as_ref()
        .and_then(|source| source.component.clone())
        .or_else(|| event.reporting_component.clone())
        .unwrap_or_default();
    Some(Change {
        at: last_seen(event)?,
        kind: ChangeKind::Node,
        status,
        target: ResourceRef {
            cluster: cluster.clone(),
            kind: NODE.to_owned(),
            namespace: None,
            name: name.clone(),
        },
        what: format!("{name} {verb}"),
        by: source,
    })
}

pub fn nodes(cluster: &ClusterId, nodes: &[Arc<Node>], events: &[Arc<Event>]) -> Vec<Change> {
    let joins = nodes.iter().filter_map(|node| joined(cluster, node));
    let cordons = events
        .iter()
        .filter_map(|event| schedulability(cluster, event));
    joins.chain(cordons).collect()
}

fn creation(
    cluster: &ClusterId,
    kind: ChangeKind,
    object: &str,
    meta: &ObjectMeta,
) -> Option<Change> {
    let name = meta.name.clone().unwrap_or_default();
    Some(Change {
        at: created(meta)?,
        kind,
        status: None,
        target: target(cluster, object, meta),
        what: format!("{name} created"),
        by: by(meta),
    })
}

pub fn namespaces(cluster: &ClusterId, namespaces: &[Arc<Namespace>]) -> Vec<Change> {
    namespaces
        .iter()
        .filter_map(|ns| creation(cluster, ChangeKind::Namespace, "Namespace", &ns.metadata))
        .collect()
}

pub fn definitions(cluster: &ClusterId, crds: &[Arc<CustomResourceDefinition>]) -> Vec<Change> {
    crds.iter()
        .filter_map(|crd| {
            creation(
                cluster,
                ChangeKind::Crd,
                "CustomResourceDefinition",
                &crd.metadata,
            )
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Shape {
    images: Vec<String>,
    replicas: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Workload {
    Deployment,
    StatefulSet,
    DaemonSet,
}

impl Workload {
    const fn kind(self) -> &'static str {
        match self {
            Self::Deployment => "Deployment",
            Self::StatefulSet => "StatefulSet",
            Self::DaemonSet => "DaemonSet",
        }
    }

    const fn short(self) -> &'static str {
        match self {
            Self::Deployment => "deploy",
            Self::StatefulSet => "sts",
            Self::DaemonSet => "ds",
        }
    }

    const fn rollout_condition(self) -> bool {
        matches!(self, Self::Deployment)
    }
}

type Key = (Workload, String, String);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Observation {
    at: Timestamp,
    kind: ChangeKind,
    workload: Key,
    what: String,
    by: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Observed {
    seen: BTreeMap<Key, Shape>,
    changes: VecDeque<Observation>,
}

fn images(template: Option<&PodTemplateSpec>) -> Vec<String> {
    template
        .and_then(|template| template.spec.as_ref())
        .into_iter()
        .flat_map(|spec| &spec.containers)
        .filter_map(|container| container.image.clone())
        .collect()
}

fn key(workload: Workload, meta: &ObjectMeta) -> Key {
    let namespace = meta.namespace.clone().unwrap_or_default();
    let name = meta.name.clone().unwrap_or_default();
    (workload, namespace, name)
}

pub struct Workloads<'a> {
    pub deployments: &'a [Arc<Deployment>],
    pub stateful_sets: &'a [Arc<StatefulSet>],
    pub daemon_sets: &'a [Arc<DaemonSet>],
}

impl Workloads<'_> {
    fn shapes(&self) -> Vec<(Key, Shape, String)> {
        let deployments = self.deployments.iter().map(|item| {
            let spec = item.spec.as_ref();
            let shape = Shape {
                images: images(spec.map(|s| &s.template)),
                replicas: spec.and_then(|s| s.replicas),
            };
            let meta = &item.metadata;
            (key(Workload::Deployment, meta), shape, by(meta))
        });
        let stateful = self.stateful_sets.iter().map(|item| {
            let spec = item.spec.as_ref();
            let shape = Shape {
                images: images(spec.map(|s| &s.template)),
                replicas: spec.and_then(|s| s.replicas),
            };
            let meta = &item.metadata;
            (key(Workload::StatefulSet, meta), shape, by(meta))
        });
        let daemons = self.daemon_sets.iter().map(|item| {
            let spec = item.spec.as_ref();
            let shape = Shape {
                images: images(spec.map(|s| &s.template)),
                replicas: None,
            };
            let meta = &item.metadata;
            (key(Workload::DaemonSet, meta), shape, by(meta))
        });
        deployments.chain(stateful).chain(daemons).collect()
    }
}

fn differences(key: &Key, before: &Shape, after: &Shape) -> Vec<(ChangeKind, String)> {
    let (workload, namespace, name) = key;
    let object = format!("{}/{name}", workload.short());
    let mut found = Vec::new();
    if before.images != after.images && !workload.rollout_condition() {
        let old = before.images.join(IMAGE_SEPARATOR);
        let new = after.images.join(IMAGE_SEPARATOR);
        found.push((
            ChangeKind::Rollout,
            format!("{object} {old} → {new} · {namespace}"),
        ));
    }
    if let (Some(old), Some(new)) = (before.replicas, after.replicas)
        && old != new
    {
        found.push((
            ChangeKind::Scale,
            format!("{object} {old} → {new} replicas · {namespace}"),
        ));
    }
    found
}

impl Observed {
    pub fn observe(&mut self, workloads: &Workloads<'_>, now: Timestamp) {
        let shapes = workloads.shapes();
        let mut seen = BTreeMap::new();
        for (key, shape, by) in shapes {
            if let Some(before) = self.seen.get(&key) {
                for (kind, what) in differences(&key, before, &shape) {
                    self.changes.push_back(Observation {
                        at: now,
                        kind,
                        workload: key.clone(),
                        what,
                        by: by.clone(),
                    });
                }
            }
            seen.insert(key, shape);
        }
        while self.changes.len() > OBSERVED_LIMIT {
            self.changes.pop_front();
        }
        self.seen = seen;
    }

    pub fn changes(&self, cluster: &ClusterId) -> Vec<Change> {
        self.changes
            .iter()
            .map(|observation| {
                let (workload, namespace, name) = &observation.workload;
                Change {
                    at: observation.at,
                    kind: observation.kind,
                    status: None,
                    target: ResourceRef {
                        cluster: cluster.clone(),
                        kind: workload.kind().to_owned(),
                        namespace: Some(namespace.clone()),
                        name: name.clone(),
                    },
                    what: observation.what.clone(),
                    by: observation.by.clone(),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use jiff::tz::TimeZone;
    use serde_json::json;

    use super::*;

    const NOW: i64 = 1_791_873_360;
    const MINUTE: i64 = 60;

    fn now() -> Timestamp {
        Timestamp::from_second(NOW).unwrap_or_default()
    }

    fn stamp(seconds_ago: i64) -> String {
        Timestamp::from_second(NOW - seconds_ago)
            .unwrap_or_default()
            .to_string()
    }

    fn parse<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Arc<T> {
        Arc::new(serde_json::from_value(value).unwrap_or_else(|e| panic!("{e}")))
    }

    fn cluster() -> ClusterId {
        ClusterId::new("prod")
    }

    fn stamped(changes: Vec<Change>) -> Vec<ClusterChange> {
        let zone = TimeZone::UTC;
        let clock = Clock {
            now: now(),
            zone: &zone,
        };
        changes
            .into_iter()
            .filter_map(|change| change.stamp(&clock))
            .collect()
    }

    fn deployment(reason: &str, updated: i64) -> Arc<Deployment> {
        parse(json!({
            "metadata": {
                "name": "search-indexer",
                "namespace": "search",
                "annotations": {"deployment.kubernetes.io/revision": "52"},
                "labels": {"kustomize.toolkit.fluxcd.io/name": "apps-search"}
            },
            "spec": {"selector": {}, "template": {}},
            "status": {"conditions": [{
                "type": "Progressing",
                "status": "True",
                "reason": reason,
                "lastUpdateTime": stamp(updated),
                "lastTransitionTime": stamp(MINUTE * 600)
            }]}
        }))
    }

    #[test]
    fn deployment_rollout_reads_the_progressing_update_time() {
        let changes = stamped(rollouts(&cluster(), &[deployment(ROLLED_OUT, MINUTE * 12)]));
        let [change] = changes.as_slice() else {
            panic!("{changes:?}");
        };
        assert_eq!(change.kind, ChangeKind::Rollout);
        assert_eq!(change.age, Duration::from_mins(12));
        assert_eq!(change.what, "deploy/search-indexer rev 52 · search");
        assert_eq!(change.by, "flux · apps-search");
        assert_eq!(change.status, None);
    }

    #[test]
    fn stalled_rollout_is_an_error_and_unknown_reasons_are_skipped() {
        let stalled = deployment(STALLED, MINUTE);
        let other = deployment("FoundNewReplicaSet", MINUTE);
        let changes = stamped(rollouts(&cluster(), &[stalled, other]));
        let [change] = changes.as_slice() else {
            panic!("{changes:?}");
        };
        assert_eq!(change.status, Some(Status::Err));
        assert!(change.what.contains("stalled"), "{}", change.what);
    }

    #[test]
    fn changes_older_than_a_week_are_dropped_and_old_ones_show_a_date() {
        let week = 7 * 24 * 60 * MINUTE;
        let changes = stamped(rollouts(
            &cluster(),
            &[
                deployment(ROLLED_OUT, week + MINUTE),
                deployment(ROLLED_OUT, 2 * 24 * 60 * MINUTE),
            ],
        ));
        let labels: Vec<&str> = changes.iter().map(|c| c.at.as_str()).collect();
        assert_eq!(labels, ["Oct 11"]);
    }

    #[test]
    fn nodes_join_and_cordon_from_events() {
        let node: Arc<Node> = parse(json!({
            "metadata": {
                "name": "ip-10-4-31-12",
                "creationTimestamp": stamp(MINUTE * 30),
                "labels": {"node.kubernetes.io/instance-type": "m7g.4xlarge"}
            }
        }));
        let cordon: Arc<Event> = parse(json!({
            "metadata": {"name": "e", "namespace": "default"},
            "involvedObject": {"kind": "Node", "name": "ip-10-4-31-12"},
            "reason": "NodeNotSchedulable",
            "source": {"component": "node-controller"},
            "lastTimestamp": stamp(MINUTE * 5)
        }));
        let noise: Arc<Event> = parse(json!({
            "metadata": {"name": "f", "namespace": "default"},
            "involvedObject": {"kind": "Pod", "name": "p"},
            "reason": "NodeNotSchedulable",
            "lastTimestamp": stamp(MINUTE)
        }));
        let changes = stamped(nodes(&cluster(), &[node], &[cordon, noise]));
        let rows: Vec<(&str, &str, Option<Status>)> = changes
            .iter()
            .map(|c| (c.what.as_str(), c.by.as_str(), c.status))
            .collect();
        assert_eq!(
            rows,
            [
                ("ip-10-4-31-12 joined · m7g.4xlarge", "", None),
                (
                    "ip-10-4-31-12 cordoned",
                    "node-controller",
                    Some(Status::Warn)
                ),
            ]
        );
    }

    #[test]
    fn namespaces_and_definitions_report_their_creation() {
        let namespace: Arc<Namespace> = parse(json!({
            "metadata": {"name": "search", "creationTimestamp": stamp(MINUTE)}
        }));
        let crd: Arc<CustomResourceDefinition> = parse(json!({
            "metadata": {"name": "widgets.example.com", "creationTimestamp": stamp(MINUTE)},
            "spec": {
                "group": "example.com",
                "names": {"kind": "Widget", "plural": "widgets"},
                "scope": "Namespaced",
                "versions": []
            }
        }));
        let mut changes = namespaces(&cluster(), &[namespace]);
        changes.extend(definitions(&cluster(), &[crd]));
        let rows: Vec<(ChangeKind, String)> = stamped(changes)
            .into_iter()
            .map(|c| (c.kind, c.what))
            .collect();
        assert_eq!(
            rows,
            [
                (ChangeKind::Namespace, "search created".to_owned()),
                (ChangeKind::Crd, "widgets.example.com created".to_owned()),
            ]
        );
    }

    fn stateful(image: &str, replicas: i32) -> Arc<StatefulSet> {
        parse(json!({
            "metadata": {"name": "db", "namespace": "data"},
            "spec": {
                "replicas": replicas,
                "selector": {},
                "serviceName": "db",
                "template": {"spec": {"containers": [{"name": "db", "image": image}]}}
            }
        }))
    }

    fn scaled(replicas: i32, image: &str) -> Arc<Deployment> {
        parse(json!({
            "metadata": {"name": "api", "namespace": "apps"},
            "spec": {
                "replicas": replicas,
                "selector": {},
                "template": {"spec": {"containers": [{"name": "api", "image": image}]}}
            }
        }))
    }

    #[test]
    fn observed_image_and_replica_changes_after_the_first_look() {
        let mut observed = Observed::default();
        let first = Workloads {
            deployments: &[scaled(3, "api:1")],
            stateful_sets: &[stateful("pg:15", 1)],
            daemon_sets: &[],
        };
        observed.observe(&first, now());
        assert!(observed.changes(&cluster()).is_empty());
        let second = Workloads {
            deployments: &[scaled(5, "api:2")],
            stateful_sets: &[stateful("pg:16", 1)],
            daemon_sets: &[],
        };
        observed.observe(&second, now());
        let rows: Vec<(ChangeKind, String)> = stamped(observed.changes(&cluster()))
            .into_iter()
            .map(|c| (c.kind, c.what))
            .collect();
        assert_eq!(
            rows,
            [
                (
                    ChangeKind::Scale,
                    "deploy/api 3 → 5 replicas · apps".to_owned()
                ),
                (
                    ChangeKind::Rollout,
                    "sts/db pg:15 → pg:16 · data".to_owned()
                ),
            ]
        );
        observed.observe(&second, now());
        assert_eq!(observed.changes(&cluster()).len(), 2);
    }
}
