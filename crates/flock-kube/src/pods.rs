use std::collections::HashMap;
use std::sync::Arc;

use flock_core::pods::{
    Container, ContainerPort, ContainerState, Pod, PodEvent, PodPhase, Qos, Usage, Volume,
};
use flock_core::{ClusterId, ResourceRef, Status, Uid};
use jiff::Timestamp;
use k8s_openapi::api::core::v1::{self as core, ContainerStatus};

use crate::clock::age;
use crate::conditions::conditions;
use crate::detail;
use crate::metrics::PodUsage;
use crate::problems::last_exit;
use crate::snapshot::{FAILING_WAIT_REASONS, PodHealth, pod_health};

const POD_KIND: &str = "Pod";
const CRASH_LOOP: &str = "CrashLoopBackOff";
const RUNNING: &str = "Running";
const NOT_READY: &str = "Running · not ready";
const PENDING: &str = "Pending";
const SCHEDULED_CONDITION: &str = "PodScheduled";
const CONDITION_FALSE: &str = "False";
const DEFAULT_PROTOCOL: &str = "TCP";
const BYTES_PER_MIB: f64 = 1_048_576.;

type PodLookup<'a, T> = HashMap<&'a str, HashMap<&'a str, T>>;

pub fn phase(text: Option<&str>) -> PodPhase {
    match text {
        Some("Pending") => PodPhase::Pending,
        Some("Running") => PodPhase::Running,
        Some("Succeeded") => PodPhase::Succeeded,
        Some("Failed") => PodPhase::Failed,
        _ => PodPhase::Unknown,
    }
}

fn qos(text: Option<&str>) -> Qos {
    match text {
        Some("Guaranteed") => Qos::Guaranteed,
        Some("BestEffort") => Qos::BestEffort,
        _ => Qos::Burstable,
    }
}

pub(crate) fn health_status(health: PodHealth) -> Status {
    match health {
        PodHealth::Healthy => Status::Ok,
        PodHealth::Warning => Status::Warn,
        PodHealth::Failing => Status::Err,
    }
}

pub(crate) fn unschedulable(pod: &core::Pod) -> Option<String> {
    pod.status
        .as_ref()?
        .conditions
        .iter()
        .flatten()
        .find(|c| c.type_ == SCHEDULED_CONDITION && c.status == CONDITION_FALSE)
        .map(|c| c.message.clone().unwrap_or_default())
}

pub(crate) fn container(
    spec: &core::Container,
    status: Option<&ContainerStatus>,
    waiting_note: Option<&str>,
) -> Container {
    let ports = spec
        .ports
        .iter()
        .flatten()
        .filter_map(|p| {
            Some(ContainerPort {
                port: u16::try_from(p.container_port).ok()?,
                protocol: p
                    .protocol
                    .clone()
                    .unwrap_or_else(|| DEFAULT_PROTOCOL.into()),
                name: p.name.clone(),
            })
        })
        .collect();
    let state = status.and_then(|s| s.state.as_ref());
    let ready = status.is_some_and(|s| s.ready);
    let restarts = status
        .and_then(|s| u32::try_from(s.restart_count).ok())
        .unwrap_or_default();
    let last_code = status
        .and_then(|s| s.last_state.as_ref())
        .and_then(|l| l.terminated.as_ref())
        .map(|t| format!("exit {}", t.exit_code));
    let last = status.and_then(last_exit);
    let running = state.is_some_and(|s| s.running.is_some());
    let (kind, health, reason, note, message) = if running {
        let health = if ready { Status::Ok } else { Status::Warn };
        let message = last.map(|last| format!("last {last}")).unwrap_or_default();
        (
            ContainerState::Running,
            health,
            RUNNING.to_owned(),
            None,
            message,
        )
    } else if let Some(terminated) = state.and_then(|s| s.terminated.as_ref()) {
        let health = if terminated.exit_code == 0 {
            Status::Ok
        } else {
            Status::Err
        };
        let reason = terminated.reason.clone().unwrap_or_default();
        let note = Some(format!("exit {}", terminated.exit_code));
        let message = terminated
            .message
            .clone()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| format!("exit {}", terminated.exit_code));
        (ContainerState::Terminated, health, reason, note, message)
    } else if let Some(waiting) = state.and_then(|s| s.waiting.as_ref()) {
        let reason = waiting.reason.clone().unwrap_or_default();
        let failing = FAILING_WAIT_REASONS.contains(&reason.as_str());
        let health = if failing { Status::Err } else { Status::Warn };
        let waiting_message = waiting.message.clone().filter(|m| !m.is_empty());
        let (note, message) = if reason == CRASH_LOOP {
            let message = last.map(|last| format!("last {last}")).unwrap_or_default();
            (last_code, message)
        } else {
            (waiting_message.clone(), waiting_message.unwrap_or_default())
        };
        (ContainerState::Waiting, health, reason, note, message)
    } else {
        let note = waiting_note.map(str::to_owned);
        (
            ContainerState::Waiting,
            Status::Warn,
            PENDING.to_owned(),
            note,
            String::new(),
        )
    };
    Container {
        name: spec.name.clone(),
        image: spec.image.clone().unwrap_or_default(),
        state: kind,
        status: health,
        reason,
        ready,
        restarts,
        ports,
        note,
        message,
    }
}

fn volume(volume: &core::Volume) -> Volume {
    let source = if let Some(map) = &volume.config_map {
        format!("configmap {}", map.name)
    } else if let Some(secret) = &volume.secret {
        format!(
            "secret {}",
            secret.secret_name.as_deref().unwrap_or_default()
        )
    } else if let Some(claim) = &volume.persistent_volume_claim {
        format!("pvc {}", claim.claim_name)
    } else if let Some(path) = &volume.host_path {
        format!("hostPath {}", path.path)
    } else if volume.empty_dir.is_some() {
        "emptyDir".to_owned()
    } else if volume.projected.is_some() {
        "projected".to_owned()
    } else {
        "other".to_owned()
    };
    Volume {
        name: volume.name.clone(),
        source,
    }
}

pub(crate) fn reason(pod: &core::Pod, containers: &[Container], health: PodHealth) -> String {
    let status = pod.status.as_ref();
    if let Some(reason) = status.and_then(|s| s.reason.clone()) {
        return reason;
    }
    let failing = containers
        .iter()
        .find(|c| c.status == Status::Err && c.state == ContainerState::Waiting);
    if let Some(container) = failing {
        return container.reason.clone();
    }
    let phase = status
        .and_then(|s| s.phase.clone())
        .unwrap_or_else(|| PENDING.to_owned());
    if phase == RUNNING && health == PodHealth::Warning {
        return NOT_READY.to_owned();
    }
    phase
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn whole(value: f64) -> u32 {
    value.round().clamp(0., f64::from(u32::MAX)) as u32
}

pub fn pod(
    cluster: &ClusterId,
    pod: &core::Pod,
    usage: Option<&Usage>,
    events: Vec<PodEvent>,
    now: Timestamp,
) -> Pod {
    let namespace = pod.metadata.namespace.clone().unwrap_or_default();
    let status = pod.status.as_ref();
    let statuses: HashMap<&str, &ContainerStatus> = status
        .and_then(|s| s.container_statuses.as_ref())
        .into_iter()
        .flatten()
        .map(|s| (s.name.as_str(), s))
        .collect();
    let scheduling = unschedulable(pod);
    let containers: Vec<Container> = pod
        .spec
        .iter()
        .flat_map(|s| s.containers.iter())
        .map(|c| {
            container(
                c,
                statuses.get(c.name.as_str()).copied(),
                scheduling.as_deref(),
            )
        })
        .collect();
    let health = pod_health(pod);
    let owner = pod
        .metadata
        .owner_references
        .iter()
        .flatten()
        .find(|o| o.controller == Some(true))
        .map(|o| ResourceRef {
            cluster: cluster.clone(),
            kind: o.kind.clone(),
            namespace: Some(namespace.clone()),
            name: o.name.clone(),
        });
    let conditions = conditions(status.and_then(|s| s.conditions.as_ref()), now);
    let volumes = pod
        .spec
        .iter()
        .flat_map(|s| s.volumes.iter().flatten())
        .map(volume)
        .collect();
    Pod {
        cluster: cluster.clone(),
        name: pod.metadata.name.clone().unwrap_or_default(),
        uid: Uid::new(pod.metadata.uid.clone().unwrap_or_default()),
        phase: phase(status.and_then(|s| s.phase.as_deref())),
        status: health_status(health),
        reason: reason(pod, &containers, health),
        node: pod.spec.as_ref().and_then(|s| s.node_name.clone()),
        ip: status.and_then(|s| s.pod_ip.clone()),
        owner,
        qos: qos(status.and_then(|s| s.qos_class.as_deref())),
        age: age(&pod.metadata, now),
        labels: pod.metadata.labels.clone().unwrap_or_default(),
        containers,
        usage: usage.copied(),
        conditions,
        volumes,
        events,
        namespace,
    }
}

fn pod_events(events: &[Arc<core::Event>], now: Timestamp) -> PodLookup<'_, Vec<PodEvent>> {
    let mut by_pod: PodLookup<'_, Vec<PodEvent>> = HashMap::new();
    for event in events {
        let involved = &event.involved_object;
        if involved.kind.as_deref() != Some(POD_KIND) {
            continue;
        }
        let namespace = involved.namespace.as_deref().unwrap_or_default();
        let name = involved.name.as_deref().unwrap_or_default();
        let row = detail::event(event, now);
        by_pod
            .entry(namespace)
            .or_default()
            .entry(name)
            .or_default()
            .push(PodEvent {
                status: row.status,
                reason: row.reason,
                message: row.message,
                count: row.count,
                age: row.age,
            });
    }
    for rows in by_pod.values_mut().flat_map(HashMap::values_mut) {
        rows.sort_by_key(|e| e.age);
    }
    by_pod
}

pub fn pods(
    cluster: &ClusterId,
    pods: &[Arc<core::Pod>],
    events: &[Arc<core::Event>],
    usage: &[PodUsage],
    now: Timestamp,
) -> Vec<Pod> {
    let mut used: PodLookup<'_, Usage> = HashMap::new();
    for entry in usage {
        let usage = Usage {
            cpu_millis: whole(entry.usage.cpu_millicores),
            memory_mib: whole(entry.usage.memory_bytes / BYTES_PER_MIB),
        };
        used.entry(&entry.namespace)
            .or_default()
            .insert(&entry.name, usage);
    }
    let mut events = pod_events(events, now);
    let mut rows: Vec<Pod> = pods
        .iter()
        .map(|p| {
            let namespace = p.metadata.namespace.as_deref().unwrap_or_default();
            let name = p.metadata.name.as_deref().unwrap_or_default();
            let own = events
                .get_mut(namespace)
                .and_then(|rows| rows.remove(name))
                .unwrap_or_default();
            let usage = used.get(namespace).and_then(|rows| rows.get(name));
            pod(cluster, p, usage, own, now)
        })
        .collect();
    rows.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    rows
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::{Value, json};

    use flock_core::Counts;
    use flock_core::pods::{PodFilter, PodQuery, nav_counts};

    use super::*;
    use crate::metrics;
    use crate::problems::tests::{at, crash_status, now, parse, ready_status, replica_pod};

    fn cluster() -> ClusterId {
        ClusterId::new("kind-fleet-a")
    }

    fn with_spec(status: &Value) -> Arc<core::Pod> {
        parse(json!({
            "metadata": {
                "name": "checkout-api-7f9c-x2lqp", "namespace": "payments", "creationTimestamp": at(180),
                "labels": { "app": "checkout-api" },
                "ownerReferences": [{
                    "apiVersion": "apps/v1", "kind": "ReplicaSet", "uid": "u",
                    "name": "checkout-api-7f9c", "controller": true
                }]
            },
            "spec": {
                "nodeName": "fleet-a-control-plane",
                "containers": [{ "name": "api", "image": "ghcr.io/acme/checkout-api:2.14.0",
                    "ports": [{ "containerPort": 8080, "name": "http" }] }],
                "volumes": [
                    { "name": "config", "configMap": { "name": "checkout-config" } },
                    { "name": "data", "persistentVolumeClaim": { "claimName": "checkout-data" } }
                ]
            },
            "status": status
        }))
    }

    #[test]
    fn crashing_pod_maps_containers_owner_volumes_and_events() {
        let mut status = crash_status();
        status["podIP"] = json!("10.244.0.12");
        status["qosClass"] = json!("Guaranteed");
        status["containerStatuses"][0]["lastState"] =
            json!({ "terminated": { "exitCode": 137, "reason": "OOMKilled" } });
        let raw = with_spec(&status);
        let event: Arc<core::Event> = parse(json!({
            "metadata": { "name": "e", "namespace": "payments" },
            "involvedObject": { "kind": "Pod", "name": "checkout-api-7f9c-x2lqp", "namespace": "payments" },
            "reason": "BackOff", "type": "Warning", "message": "Back-off restarting failed container api",
            "lastTimestamp": at(2), "count": 14
        }));
        let usage = [PodUsage {
            namespace: "payments".into(),
            name: "checkout-api-7f9c-x2lqp".into(),
            usage: metrics::Usage {
                cpu_millicores: 4.2,
                memory_bytes: 38. * BYTES_PER_MIB,
            },
        }];
        let rows = pods(&cluster(), &[raw], &[event], &usage, now());
        let pod = &rows[0];
        assert_eq!(pod.status, Status::Err);
        assert_eq!(pod.reason, "CrashLoopBackOff");
        assert_eq!(pod.phase, PodPhase::Running);
        assert_eq!(pod.qos, Qos::Guaranteed);
        assert_eq!(pod.ip.as_deref(), Some("10.244.0.12"));
        assert_eq!(pod.age, Duration::from_mins(180));
        assert_eq!(
            pod.owner.as_ref().map(|o| o.kind.as_str()),
            Some("ReplicaSet")
        );
        let api = &pod.containers[0];
        assert_eq!(
            (api.state, api.status, api.restarts, api.note.as_deref()),
            (ContainerState::Waiting, Status::Err, 14, Some("exit 137"))
        );
        assert_eq!(api.ports[0].label(), "8080/TCP http");
        assert_eq!(api.message, "last exit OOMKilled (137)");
        let sources: Vec<&str> = pod.volumes.iter().map(|v| v.source.as_str()).collect();
        assert_eq!(sources, ["configmap checkout-config", "pvc checkout-data"]);
        assert_eq!(pod.events.len(), 1);
        assert_eq!(
            (pod.events[0].status, pod.events[0].count),
            (Status::Err, 14)
        );
        assert_eq!(
            pod.usage.map(|u| (u.cpu_millis, u.memory_mib)),
            Some((4, 38))
        );
        assert_eq!(pod.ready_label(), "0/1");
    }

    #[test]
    fn unscheduled_pod_explains_its_wait() {
        let raw: Arc<core::Pod> = parse(json!({
            "metadata": { "name": "unschedulable", "namespace": "apps" },
            "spec": { "containers": [{ "name": "app", "image": "busybox" }] },
            "status": { "phase": "Pending", "conditions": [{
                "type": "PodScheduled", "status": "False", "reason": "Unschedulable",
                "message": "0/1 nodes are available"
            }] }
        }));
        let row = pod(&cluster(), &raw, None, Vec::new(), now());
        assert_eq!((row.status, row.reason.as_str()), (Status::Warn, "Pending"));
        assert_eq!(
            row.containers[0].note.as_deref(),
            Some("0/1 nodes are available")
        );
        assert_eq!(row.conditions[0].reason, "Unschedulable");
        assert_eq!(
            row.diagnosis(),
            Some(flock_core::Diagnosis::new(
                "Unschedulable",
                "0/1 nodes are available"
            ))
        );
    }

    #[test]
    fn sidebar_counts_agree_with_the_pod_list() {
        let raw = [
            replica_pod("a-1-x", "a", "1", &crash_status()),
            replica_pod("a-1-y", "a", "1", &ready_status()),
            replica_pod("a-1-z", "a", "1", &json!({ "phase": "Pending" })),
        ];
        let rows = pods(&cluster(), &raw, &[], &[], now());
        let failing = PodQuery {
            filter: PodFilter::Failing,
            ..PodQuery::default()
        };
        assert_eq!(nav_counts(&rows, &cluster()), Counts::total(3).with_err(1));
        assert_eq!(failing.apply(&rows, &cluster()).len(), 1);
    }
}
