use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use flock_core::cluster::Namespace;
use flock_core::events::{Event, warnings_first};
use flock_core::nodes;
use flock_core::overview::{Commitment, PodPhases};
use flock_core::{Counts, Diagnosis, Status, Uid};
use jiff::Timestamp;
use k8s_openapi::api::core::v1::{self as core, Node, Pod};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;

use crate::clock::{age, between};
use crate::conditions::conditions;
use crate::metrics::{NodeUsage, PodUsage, Usage, cpu_millicores, parse_quantity};
use crate::problems::{OwnerKey, is_active, owner_of, pod_issue};
use crate::snapshot::{PodHealth, node_ready, pod_health, schedulable};
use crate::usage::{Allocatable, allocatable, cores, gib, narrow};

const CPU: &str = "cpu";
const MEMORY: &str = "memory";
const PODS: &str = "pods";
const PHASE_PENDING: &str = "Pending";
const PHASE_SUCCEEDED: &str = "Succeeded";
const STATE_READY: &str = "Ready";
const STATE_NOT_READY: &str = "NotReady";
const STATE_CORDONED: &str = "SchedulingDisabled";
const INSTANCE_TYPE_LABELS: [&str; 2] = [
    "node.kubernetes.io/instance-type",
    "beta.kubernetes.io/instance-type",
];
const ZONE_LABELS: [&str; 2] = [
    "topology.kubernetes.io/zone",
    "failure-domain.beta.kubernetes.io/zone",
];
const INTERNAL_IP: &str = "InternalIP";
const WARNING: &str = "Warning";
const ERROR_REASONS: [&str; 4] = [
    "BackOff",
    "Failed",
    "ProgressDeadlineExceeded",
    "FailedCreate",
];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Totals {
    cpu: f64,
    memory: f64,
}

impl Totals {
    fn add(&mut self, other: Self) {
        self.cpu += other.cpu;
        self.memory += other.memory;
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct NamespaceFacts {
    pods: u32,
    problems: Counts,
    requests: Totals,
    restarts: u32,
}

impl NamespaceFacts {
    fn record(&mut self, health: PodHealth, requests: Totals, restarts: u32) {
        self.pods += 1;
        self.requests.add(requests);
        self.restarts += restarts;
        match health {
            PodHealth::Failing => self.problems.err += 1,
            PodHealth::Warning => self.problems.warn += 1,
            PodHealth::Healthy => {}
        }
    }
}

#[derive(Debug, Default)]
pub struct PodIndex {
    pub issues: HashMap<OwnerKey, (Status, Diagnosis)>,
    pub phases: PodPhases,
    namespaces: HashMap<String, NamespaceFacts>,
    per_node: HashMap<String, u32>,
    node_resources: HashMap<String, (Totals, Totals)>,
    requests: Totals,
    limits: Totals,
}

fn resources(map: Option<&BTreeMap<String, Quantity>>) -> Totals {
    let get = |key: &str| map.and_then(|m| m.get(key)).map(|q| q.0.as_str());
    Totals {
        cpu: get(CPU).and_then(cpu_millicores).unwrap_or_default(),
        memory: get(MEMORY).and_then(parse_quantity).unwrap_or_default(),
    }
}

fn restart_count(pod: &Pod) -> u32 {
    let statuses = pod
        .status
        .as_ref()
        .and_then(|s| s.container_statuses.as_ref());
    statuses
        .into_iter()
        .flatten()
        .map(|c| u32::try_from(c.restart_count).unwrap_or_default())
        .sum()
}

fn pod_resources(pod: &Pod) -> (Totals, Totals) {
    let mut requests = Totals::default();
    let mut limits = Totals::default();
    for container in pod.spec.iter().flat_map(|spec| spec.containers.iter()) {
        let declared = container.resources.as_ref();
        requests.add(resources(declared.and_then(|r| r.requests.as_ref())));
        limits.add(resources(declared.and_then(|r| r.limits.as_ref())));
    }
    (requests, limits)
}

impl PodIndex {
    pub fn build(pods: &[Arc<Pod>], now: Timestamp) -> Self {
        let mut index = Self::default();
        for pod in pods {
            let phase = pod.status.as_ref().and_then(|s| s.phase.as_deref());
            let health = pod_health(pod);
            match health {
                PodHealth::Failing => index.phases.failing += 1,
                PodHealth::Healthy if phase == Some(PHASE_SUCCEEDED) => {}
                PodHealth::Healthy => index.phases.running += 1,
                PodHealth::Warning if phase == Some(PHASE_PENDING) => index.phases.pending += 1,
                PodHealth::Warning => index.phases.running += 1,
            }
            if phase == Some(PHASE_PENDING) && crate::pods::unschedulable(pod).is_some() {
                index.phases.unschedulable += 1;
            }
            if !is_active(pod) {
                continue;
            }
            let (requests, limits) = pod_resources(pod);
            let restarts = restart_count(pod);
            let namespace = pod.metadata.namespace.as_deref().unwrap_or_default();
            if let Some(facts) = index.namespaces.get_mut(namespace) {
                facts.record(health, requests, restarts);
            } else {
                let mut facts = NamespaceFacts::default();
                facts.record(health, requests, restarts);
                index.namespaces.insert(namespace.to_owned(), facts);
            }
            if let Some(node) = pod.spec.as_ref().and_then(|s| s.node_name.as_deref()) {
                if let Some(count) = index.per_node.get_mut(node) {
                    *count += 1;
                } else {
                    index.per_node.insert(node.to_owned(), 1);
                }
            }
            if let Some(name) = pod.spec.as_ref().and_then(|s| s.node_name.as_ref()) {
                let totals = index.node_resources.entry(name.clone()).or_default();
                totals.0.add(requests);
                totals.1.add(limits);
            }
            index.requests.add(requests);
            index.limits.add(limits);
            if let Some((issue, _)) = pod_issue(pod, now) {
                let status = issue.status();
                let entry = index
                    .issues
                    .entry(owner_of(pod))
                    .or_insert_with(|| (status, issue.diagnosis()));
                if status > entry.0 {
                    *entry = (status, issue.diagnosis());
                }
            }
        }
        index
    }

    pub fn commitments(&self, capacity: Allocatable) -> (Commitment, Commitment) {
        commitments(self.requests, self.limits, capacity)
    }

    pub fn issue(&self, owner: &OwnerKey) -> Option<Status> {
        self.issues.get(owner).map(|(status, _)| *status)
    }

    pub fn diagnosis(&self, owner: &OwnerKey) -> Option<Diagnosis> {
        self.issues
            .get(owner)
            .map(|(_, diagnosis)| diagnosis.clone())
    }
}

fn commitments(
    requests: Totals,
    limits: Totals,
    capacity: Allocatable,
) -> (Commitment, Commitment) {
    let share = |used: f64, total: f64| {
        if total > 0. { narrow(used / total) } else { 0. }
    };
    let cpu = Commitment {
        requests: share(requests.cpu, capacity.cpu_millicores),
        limits: share(limits.cpu, capacity.cpu_millicores),
    };
    let memory = Commitment {
        requests: share(requests.memory, capacity.memory_bytes),
        limits: share(limits.memory, capacity.memory_bytes),
    };
    (cpu, memory)
}

pub fn capacity(nodes: &[Arc<Node>]) -> Allocatable {
    nodes
        .iter()
        .map(|node| allocatable(node))
        .fold(Allocatable::default(), |total, next| Allocatable {
            cpu_millicores: total.cpu_millicores + next.cpu_millicores,
            memory_bytes: total.memory_bytes + next.memory_bytes,
        })
}

fn first_label(labels: Option<&BTreeMap<String, String>>, keys: &[&str]) -> Option<String> {
    let labels = labels?;
    keys.iter().find_map(|key| labels.get(*key).cloned())
}

fn taint_label(taint: &core::Taint) -> String {
    match taint.value.as_deref().filter(|v| !v.is_empty()) {
        Some(value) => format!("{}={value}:{}", taint.key, taint.effect),
        None => format!("{}:{}", taint.key, taint.effect),
    }
}

fn pod_capacity(node: &Node) -> u32 {
    node.status
        .as_ref()
        .and_then(|s| s.allocatable.as_ref())
        .and_then(|m| m.get(PODS))
        .and_then(|q| q.0.parse().ok())
        .unwrap_or_default()
}

pub fn node(node: &Node, pods: u32, usage: Option<&Usage>, now: Timestamp) -> nodes::Node {
    let labels = node.metadata.labels.as_ref();
    let status = node.status.as_ref();
    let info = status.and_then(|s| s.node_info.as_ref());
    let ready = node_ready(node);
    let can_schedule = schedulable(node);
    let (health, state) = match (ready, can_schedule) {
        (false, _) => (Status::Err, STATE_NOT_READY),
        (true, false) => (Status::Warn, STATE_CORDONED),
        (true, true) => (Status::Ok, STATE_READY),
    };
    let platform = info.map(|i| format!("{}/{}", i.operating_system, i.architecture));
    let capacity = allocatable(node);
    let conditions = conditions(status.and_then(|s| s.conditions.as_ref()), now);
    let taints = node
        .spec
        .as_ref()
        .and_then(|s| s.taints.as_ref())
        .into_iter()
        .flatten()
        .map(taint_label)
        .collect();
    let internal_ip = status
        .and_then(|s| s.addresses.as_ref())
        .into_iter()
        .flatten()
        .find(|a| a.type_ == INTERNAL_IP)
        .map(|a| a.address.clone())
        .unwrap_or_default();
    nodes::Node {
        name: node.metadata.name.clone().unwrap_or_default(),
        uid: Uid::new(node.metadata.uid.clone().unwrap_or_default()),
        status: health,
        state: state.to_owned(),
        schedulable: can_schedule,
        instance_type: first_label(labels, &INSTANCE_TYPE_LABELS)
            .or(platform)
            .unwrap_or_default(),
        zone: first_label(labels, &ZONE_LABELS).unwrap_or_default(),
        version: info.map(|i| i.kubelet_version.clone()).unwrap_or_default(),
        internal_ip,
        os_image: info.map(|i| i.os_image.clone()).unwrap_or_default(),
        runtime: info
            .map(|i| i.container_runtime_version.clone())
            .unwrap_or_default(),
        cpu: nodes::Usage {
            used: usage.map_or(0., |u| cores(u.cpu_millicores)),
            allocatable: cores(capacity.cpu_millicores),
        },
        memory: nodes::Usage {
            used: usage.map_or(0., |u| gib(u.memory_bytes)),
            allocatable: gib(capacity.memory_bytes),
        },
        cpu_commitment: Commitment::default(),
        memory_commitment: Commitment::default(),
        pods,
        pod_capacity: pod_capacity(node),
        age: age(&node.metadata, now),
        conditions,
        taints,
        labels: labels
            .into_iter()
            .flatten()
            .map(|(k, v)| format!("{k}={v}"))
            .collect(),
    }
}

pub fn nodes(
    nodes: &[Arc<Node>],
    index: &PodIndex,
    usage: &[NodeUsage],
    now: Timestamp,
) -> Vec<nodes::Node> {
    let used: HashMap<&str, &Usage> = usage.iter().map(|u| (u.name.as_str(), &u.usage)).collect();
    let mut rows: Vec<nodes::Node> = nodes
        .iter()
        .map(|n| {
            let name = n.metadata.name.as_deref().unwrap_or_default();
            let pods = index.per_node.get(name).copied().unwrap_or_default();
            let mut row = node(n, pods, used.get(name).copied(), now);
            let (requests, limits) = index.node_resources.get(name).copied().unwrap_or_default();
            let (cpu, memory) = commitments(requests, limits, allocatable(n));
            row.cpu_commitment = cpu;
            row.memory_commitment = memory;
            row
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows
}

pub fn namespaces(
    namespaces: &[Arc<core::Namespace>],
    index: &PodIndex,
    usage: Option<&[PodUsage]>,
    now: Timestamp,
) -> Vec<Namespace> {
    let mut used: HashMap<&str, Totals> = HashMap::new();
    for pod in usage.unwrap_or_default() {
        used.entry(pod.namespace.as_str()).or_default().add(Totals {
            cpu: pod.usage.cpu_millicores,
            memory: pod.usage.memory_bytes,
        });
    }
    let mut rows: Vec<Namespace> = namespaces
        .iter()
        .map(|ns| {
            let name = ns.metadata.name.clone().unwrap_or_default();
            let facts = index.namespaces.get(&name).cloned().unwrap_or_default();
            let totals = used.get(name.as_str()).copied().unwrap_or_default();
            let labels = ns.metadata.labels.clone().unwrap_or_default();
            Namespace {
                labels,
                status: facts.problems.status(),
                phase: ns
                    .status
                    .as_ref()
                    .and_then(|s| s.phase.clone())
                    .unwrap_or_default(),
                pods: facts.pods,
                problems: facts.problems,
                cpu: usage.map(|_| cores(totals.cpu)),
                memory: usage.map(|_| gib(totals.memory)),
                cpu_requests: cores(facts.requests.cpu),
                memory_requests: gib(facts.requests.memory),
                restarts: facts.restarts,
                age: age(&ns.metadata, now),
                name,
            }
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows
}

pub(crate) fn last_seen(event: &core::Event) -> Option<Timestamp> {
    event
        .series
        .as_ref()
        .and_then(|s| s.last_observed_time.as_ref())
        .map(|t| t.0)
        .or_else(|| event.last_timestamp.as_ref().map(|t| t.0))
        .or_else(|| event.event_time.as_ref().map(|t| t.0))
        .or_else(|| event.first_timestamp.as_ref().map(|t| t.0))
        .or_else(|| event.metadata.creation_timestamp.as_ref().map(|t| t.0))
}

pub fn event_status(event: &core::Event) -> Status {
    let reason = event.reason.as_deref().unwrap_or_default();
    if event.type_.as_deref() != Some(WARNING) {
        Status::Ok
    } else if ERROR_REASONS.contains(&reason) {
        Status::Err
    } else {
        Status::Warn
    }
}

const FLUX_GROUP: &str = "toolkit.fluxcd.io/";

pub fn event_counts(events: &[Arc<core::Event>]) -> Counts {
    let flagged = Counts::of(events.iter().map(|event| event_status(event)));
    Counts {
        total: None,
        ..flagged
    }
}

pub fn event(event: &core::Event, now: Timestamp) -> Event {
    let reason = event.reason.clone().unwrap_or_default();
    let status = event_status(event);
    let involved = &event.involved_object;
    let object = format!(
        "{}/{}",
        involved.kind.as_deref().unwrap_or_default().to_lowercase(),
        involved.name.as_deref().unwrap_or_default()
    );
    let count = event
        .series
        .as_ref()
        .and_then(|s| s.count)
        .or(event.count)
        .and_then(|c| u32::try_from(c).ok())
        .unwrap_or(1);
    let first_seen = event
        .first_timestamp
        .as_ref()
        .map(|t| t.0)
        .or_else(|| event.event_time.as_ref().map(|t| t.0))
        .or_else(|| event.metadata.creation_timestamp.as_ref().map(|t| t.0));
    let source = event.source.as_ref();
    let details = flock_core::events::EventDetails {
        uid: event.metadata.uid.clone().unwrap_or_default(),
        kind: involved.kind.clone().unwrap_or_default(),
        object_uid: involved.uid.clone(),
        object_namespace: involved.namespace.clone(),
        event_type: event.type_.clone().unwrap_or_else(|| "Unknown".into()),
        first_seen: first_seen.map(|t| t.to_string()),
        last_seen: last_seen(event).map(|t| t.to_string()),
        first_age: first_seen.map(|t| between(t, now)),
        source_component: source
            .and_then(|s| s.component.clone())
            .or_else(|| event.reporting_component.clone())
            .unwrap_or_default(),
        source_host: source
            .and_then(|s| s.host.clone())
            .or_else(|| event.reporting_instance.clone())
            .unwrap_or_default(),
    };
    let labels = event.metadata.labels.clone().unwrap_or_default();
    Event {
        labels,
        details,
        status,
        reason,
        object,
        namespace: event
            .metadata
            .namespace
            .clone()
            .or_else(|| involved.namespace.clone())
            .filter(|ns| !ns.is_empty()),
        message: event
            .message
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_owned(),
        age: last_seen(event)
            .map(|t| between(t, now))
            .unwrap_or_default(),
        count,
    }
}

pub fn flux_events(events: &[Arc<core::Event>], now: Timestamp) -> Vec<Event> {
    events
        .iter()
        .filter(|event| is_flux(event))
        .map(|e| event(e, now))
        .collect()
}

fn is_flux(event: &core::Event) -> bool {
    let involved = &event.involved_object;
    let kind = involved.kind.as_deref().unwrap_or_default();
    let group = involved.api_version.as_deref().unwrap_or_default();
    flock_core::events::FLUX_EVENT_KINDS.contains(&kind) && group.contains(FLUX_GROUP)
}

pub fn events(events: &[Arc<core::Event>], limit: usize, now: Timestamp) -> Vec<Event> {
    let mut rows: Vec<Event> = events.iter().map(|e| event(e, now)).collect();
    rows.sort_by_key(|e| e.age);
    rows.truncate(limit);
    warnings_first(&rows)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::problems::tests::{at, crash_status, now, parse, ready_status, replica_pod};

    fn pod_with_resources(name: &str, node: &str, cpu: &str, memory: &str) -> Arc<Pod> {
        parse(json!({
            "metadata": { "name": name, "namespace": "web" },
            "spec": {
                "nodeName": node,
                "containers": [{
                    "name": "c", "image": "i",
                    "resources": {
                        "requests": { "cpu": cpu, "memory": memory },
                        "limits": { "cpu": "2", "memory": memory }
                    }
                }]
            },
            "status": { "phase": "Running", "conditions": [{ "type": "Ready", "status": "True" }] }
        }))
    }

    fn worker(name: &str) -> Arc<Node> {
        parse(json!({
            "metadata": {
                "name": name,
                "creationTimestamp": at(60),
                "labels": { "topology.kubernetes.io/zone": "us-east-1a", "kubernetes.io/os": "linux" }
            },
            "spec": { "taints": [{ "key": "gpu", "value": "present", "effect": "NoSchedule" }] },
            "status": {
                "allocatable": { "cpu": "4", "memory": "8Gi", "pods": "110" },
                "addresses": [{ "type": "InternalIP", "address": "10.0.0.7" }],
                "nodeInfo": {
                    "architecture": "arm64", "operatingSystem": "linux", "osImage": "Debian",
                    "kubeletVersion": "v1.37.0", "containerRuntimeVersion": "containerd://2.1",
                    "bootID": "", "kernelVersion": "", "kubeProxyVersion": "", "machineID": "", "systemUUID": ""
                },
                "conditions": [
                    { "type": "Ready", "status": "True", "message": "kubelet is posting ready status" },
                    { "type": "MemoryPressure", "status": "False" }
                ]
            }
        }))
    }

    fn involved(kind: &str, api_version: &str, name: &str) -> Arc<core::Event> {
        parse(json!({
            "metadata": { "name": name, "namespace": "flux-system", "uid": name },
            "involvedObject": {
                "kind": kind, "apiVersion": api_version, "name": name, "namespace": "flux-system"
            },
            "reason": "ReconciliationSucceeded",
            "type": "Normal",
            "lastTimestamp": at(60)
        }))
    }

    #[test]
    fn flux_events_keep_only_events_about_flux_objects() {
        let events = [
            involved("Kustomization", "kustomize.toolkit.fluxcd.io/v1", "apps"),
            involved("HelmChart", "source.toolkit.fluxcd.io/v1", "chart"),
            involved("Pod", "v1", "web-0"),
            involved("Kustomization", "kustomize.config.k8s.io/v1beta1", "other"),
        ];
        let rows = flux_events(&events, now());
        let objects: Vec<&str> = rows.iter().map(|row| row.object.as_str()).collect();
        assert_eq!(objects, ["kustomization/apps", "helmchart/chart"]);
        assert_eq!(rows[0].details.kind, "Kustomization");
    }

    #[test]
    fn node_commitments_use_only_active_pods_assigned_to_that_node() {
        let mut finished = (*pod_with_resources("done", "w1", "4", "8Gi")).clone();
        finished.status = Some(core::PodStatus {
            phase: Some("Succeeded".into()),
            ..Default::default()
        });
        let pods = [
            pod_with_resources("a", "w1", "500m", "1Gi"),
            pod_with_resources("b", "w2", "1500m", "3Gi"),
            Arc::new(finished),
        ];
        let index = PodIndex::build(&pods, now());
        let workers = [worker("w1"), worker("w2"), worker("w3")];
        let rows = nodes(&workers, &index, &[], now());
        assert_eq!(
            rows[0].cpu_commitment,
            Commitment {
                requests: 0.125,
                limits: 0.5
            }
        );
        assert_eq!(
            rows[0].memory_commitment,
            Commitment {
                requests: 0.125,
                limits: 0.125
            }
        );
        assert_eq!(
            rows[1].cpu_commitment,
            Commitment {
                requests: 0.375,
                limits: 0.5
            }
        );
        assert_eq!(rows[2].cpu_commitment, Commitment::default());
    }

    #[test]
    fn overview_sums_requests_limits_and_phases() {
        let pending: Arc<Pod> = parse(json!({
            "metadata": { "name": "p", "namespace": "apps" },
            "status": { "phase": "Pending" }
        }));
        let unschedulable: Arc<Pod> = parse(json!({
            "metadata": { "name": "u", "namespace": "apps" },
            "status": {
                "phase": "Pending",
                "conditions": [{ "type": "PodScheduled", "status": "False", "reason": "Unschedulable" }]
            }
        }));
        let pods = [
            pod_with_resources("a", "w1", "500m", "1Gi"),
            pod_with_resources("b", "w1", "1500m", "3Gi"),
            replica_pod("api-1-x", "api", "1", &crash_status()),
            pending,
            unschedulable,
        ];
        let index = PodIndex::build(&pods, now());
        assert_eq!(
            index.phases,
            PodPhases {
                running: 2,
                pending: 2,
                failing: 1,
                unschedulable: 1,
            }
        );
        let nodes = [worker("w1")];
        let (cpu, memory) = index.commitments(capacity(&nodes));
        assert!((cpu.requests - 0.5).abs() < 1e-6);
        assert!((cpu.limits - 1.0).abs() < 1e-6);
        assert!((memory.requests - 0.5).abs() < 1e-6);
        let owner = OwnerKey {
            namespace: "payments".into(),
            kind: "Deployment".into(),
            name: "api".into(),
        };
        assert_eq!(index.issue(&owner), Some(Status::Err));
        assert_eq!(
            index.diagnosis(&owner),
            Some(Diagnosis::new(
                "CrashLoopBackOff",
                "14 restarts, last exit 1"
            ))
        );
    }

    #[test]
    fn node_rows_carry_usage_labels_and_pod_counts() {
        let pods = [pod_with_resources("a", "w1", "1", "1Gi")];
        let index = PodIndex::build(&pods, now());
        let usage = [NodeUsage {
            name: "w1".into(),
            usage: Usage {
                cpu_millicores: 1000.,
                memory_bytes: 2. * 1_073_741_824.,
            },
        }];
        let rows = nodes(&[worker("w1")], &index, &usage, now());
        let row = &rows[0];
        assert_eq!((row.status, row.state.as_str()), (Status::Ok, "Ready"));
        assert_eq!(row.instance_type, "linux/arm64");
        assert_eq!(row.zone, "us-east-1a");
        assert_eq!(row.internal_ip, "10.0.0.7");
        assert_eq!(row.pods, 1);
        assert_eq!(row.pod_capacity, 110);
        assert_eq!((row.cpu.used, row.cpu.allocatable), (1., 4.));
        assert_eq!((row.memory.used, row.memory.allocatable), (2., 8.));
        assert_eq!(row.taints, ["gpu=present:NoSchedule"]);
        assert_eq!(row.age, Duration::from_secs(3600));
        assert!(row.conditions.iter().all(flock_core::Condition::is_healthy));
        assert!(row.is_ready());
    }

    #[test]
    fn namespaces_count_pods_problems_and_usage() {
        let namespace: Arc<core::Namespace> = parse(json!({
            "metadata": { "name": "payments", "creationTimestamp": at(1440) },
            "status": { "phase": "Active" }
        }));
        let pods = [
            replica_pod("api-1-x", "api", "1", &crash_status()),
            replica_pod("api-1-y", "api", "1", &ready_status()),
        ];
        let index = PodIndex::build(&pods, now());
        let usage = [PodUsage {
            namespace: "payments".into(),
            name: "api-1-y".into(),
            usage: Usage {
                cpu_millicores: 250.,
                memory_bytes: 536_870_912.,
            },
        }];
        let rows = namespaces(
            std::slice::from_ref(&namespace),
            &index,
            Some(&usage),
            now(),
        );
        let row = &rows[0];
        assert_eq!(row.pods, 2);
        assert_eq!(row.problems, Counts::default().with_err(1));
        assert_eq!(row.status, Status::Err);
        assert_eq!((row.cpu, row.memory), (Some(0.25), Some(0.5)));
        assert_eq!(row.phase, "Active");
        let before_first_poll = namespaces(&[namespace], &index, None, now());
        assert_eq!(
            (before_first_poll[0].cpu, before_first_poll[0].memory),
            (None, None)
        );
    }

    #[test]
    fn event_inspector_preserves_identity_timestamps_and_reporting_source() {
        let source: core::Event = parse::<core::Event>(json!({
            "metadata": { "name": "backoff", "namespace": "payments", "uid": "event-uid", "creationTimestamp": at(60) },
            "involvedObject": { "kind": "Pod", "name": "checkout", "namespace": "payments", "uid": "pod-uid" },
            "reason": "BackOff", "type": "Warning", "message": "Full message without truncation",
            "firstTimestamp": at(30), "lastTimestamp": at(5),
            "series": { "count": 9, "lastObservedTime": at(1) },
            "reportingComponent": "kubelet", "reportingInstance": "worker-a"
        })).as_ref().clone();
        let row = event(&source, now());
        assert_eq!(row.details.uid, "event-uid");
        assert_eq!(row.details.object_uid.as_deref(), Some("pod-uid"));
        assert_eq!(row.details.kind, "Pod");
        assert_eq!(row.details.event_type, "Warning");
        assert_eq!(row.details.source_component, "kubelet");
        assert_eq!(row.details.source_host, "worker-a");
        assert_eq!(row.details.first_age, Some(Duration::from_mins(30)));
        assert_eq!(row.details.first_seen.as_deref(), Some(at(30).as_str()));
        assert_eq!(row.details.last_seen.as_deref(), Some(at(1).as_str()));
        assert_eq!(row.age, Duration::from_secs(60));
        assert_eq!(row.count, 9);
        assert_eq!(row.message, "Full message without truncation");
    }

    #[test]
    fn event_unknown_fields_remain_unknown_and_node_events_keep_event_namespace() {
        let source = parse::<core::Event>(json!({
            "metadata": { "name": "node-event", "namespace": "default" },
            "involvedObject": { "kind": "Node", "name": "worker" }
        }));
        let row = event(&source, now());
        assert_eq!(row.namespace.as_deref(), Some("default"));
        assert_eq!(row.details.first_seen, None);
        assert_eq!(row.details.last_seen, None);
        assert_eq!(row.details.event_type, "Unknown");
    }

    #[test]
    fn events_put_errors_and_warnings_first_then_newest() {
        let event = |reason: &str, kind: &str, minutes: i64| -> Arc<core::Event> {
            parse(json!({
                "metadata": { "name": format!("e-{reason}"), "namespace": "payments" },
                "involvedObject": { "kind": "Pod", "name": "checkout-api-x", "namespace": "payments" },
                "reason": reason, "type": kind, "message": format!("{reason} happened "),
                "lastTimestamp": at(minutes), "count": 3
            }))
        };
        let rows = events(
            &[
                event("Scheduled", "Normal", 1),
                event("Unhealthy", "Warning", 11),
                event("BackOff", "Warning", 2),
                event("Pulled", "Normal", 30),
            ],
            3,
            now(),
        );
        let reasons: Vec<(&str, Status)> =
            rows.iter().map(|e| (e.reason.as_str(), e.status)).collect();
        assert_eq!(
            reasons,
            [
                ("BackOff", Status::Err),
                ("Unhealthy", Status::Warn),
                ("Scheduled", Status::Ok)
            ]
        );
        let first = &rows[0];
        assert_eq!(first.object, "pod/checkout-api-x");
        assert_eq!(first.message, "BackOff happened");
        assert_eq!((first.count, first.age), (3, Duration::from_secs(120)));
    }
}
