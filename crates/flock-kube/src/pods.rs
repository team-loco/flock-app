use std::collections::HashMap;
use std::sync::Arc;

use std::collections::BTreeMap;

use flock_core::pods::{
    Container, ContainerPort, ContainerState, Pod, PodEvent, PodPhase, Probe, ProbeKind, Qos,
    Resources, Termination, Usage, Volume,
};
use flock_core::time::{self, Date, LocalTime};
use flock_core::{ClusterId, ResourceRef, Status, Uid};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use k8s_openapi::api::core::v1::{self as core, ContainerStateTerminated, ContainerStatus};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;

use crate::clock::{age, between, since};
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
const CPU: &str = "cpu";
const MEMORY: &str = "memory";
const EXEC_PROBE: &str = "exec";

pub struct Clock<'a> {
    pub now: Timestamp,
    pub zone: &'a TimeZone,
}

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

fn resources(quantities: Option<&BTreeMap<String, Quantity>>) -> Resources {
    let get = |key: &str| quantities.and_then(|q| q.get(key)).map(|q| q.0.clone());
    Resources {
        cpu: get(CPU),
        memory: get(MEMORY),
    }
}

fn port_label(port: &IntOrString) -> String {
    match port {
        IntOrString::Int(number) => format!(":{number}"),
        IntOrString::String(name) => format!(":{name}"),
    }
}

fn probe(kind: ProbeKind, probe: Option<&core::Probe>) -> Option<Probe> {
    let probe = probe?;
    let target = if let Some(http) = &probe.http_get {
        let path = http.path.as_deref().unwrap_or_default();
        format!("{}{path}", port_label(&http.port))
    } else if let Some(tcp) = &probe.tcp_socket {
        format!("tcp {}", port_label(&tcp.port))
    } else if let Some(grpc) = &probe.grpc {
        format!("grpc :{}", grpc.port)
    } else {
        EXEC_PROBE.to_owned()
    };
    Some(Probe { kind, target })
}

fn probes(spec: &core::Container) -> Vec<Probe> {
    [
        probe(ProbeKind::Liveness, spec.liveness_probe.as_ref()),
        probe(ProbeKind::Readiness, spec.readiness_probe.as_ref()),
        probe(ProbeKind::Startup, spec.startup_probe.as_ref()),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn date(day: jiff::civil::Date) -> Date {
    Date {
        year: day.year(),
        month: day.month(),
        day: day.day(),
    }
}

fn stamp(at: Timestamp, clock: &Clock<'_>) -> String {
    let local = at.to_zoned(clock.zone.clone());
    let today = clock.now.to_zoned(clock.zone.clone()).date();
    let moment = LocalTime {
        date: date(local.date()),
        hour: local.hour(),
        minute: local.minute(),
    };
    time::stamp(moment, date(today))
}

fn termination(terminated: &ContainerStateTerminated, clock: &Clock<'_>) -> Termination {
    let started = terminated.started_at.as_ref().map(|t| t.0);
    let finished = terminated.finished_at.as_ref().map(|t| t.0);
    let ran = started
        .zip(finished)
        .map(|(started, finished)| between(started, finished));
    Termination {
        reason: terminated.reason.clone().unwrap_or_default(),
        exit_code: terminated.exit_code,
        started: started.map(|at| stamp(at, clock)),
        finished: finished.map(|at| stamp(at, clock)),
        ago: finished.map(|at| between(at, clock.now)),
        ran,
    }
}

fn ports(spec: &core::Container) -> Vec<ContainerPort> {
    spec.ports
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
        .collect()
}

pub(crate) fn container(
    spec: &core::Container,
    status: Option<&ContainerStatus>,
    waiting_note: Option<&str>,
    clock: &Clock<'_>,
) -> Container {
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
    let running_since = state
        .and_then(|s| s.running.as_ref())
        .map(|r| since(r.started_at.as_ref(), clock.now));
    let terminated = state
        .and_then(|s| s.terminated.as_ref())
        .map(|t| termination(t, clock));
    let last_termination = status
        .and_then(|s| s.last_state.as_ref())
        .and_then(|l| l.terminated.as_ref())
        .map(|t| termination(t, clock));
    let since_state = running_since.or_else(|| terminated.as_ref().and_then(|t| t.ago));
    let running = running_since.is_some();
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
        ports: ports(spec),
        note,
        message,
        requests: resources(spec.resources.as_ref().and_then(|r| r.requests.as_ref())),
        limits: resources(spec.resources.as_ref().and_then(|r| r.limits.as_ref())),
        probes: probes(spec),
        since: since_state,
        terminated,
        last_termination,
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
    clock: &Clock<'_>,
) -> Pod {
    let now = clock.now;
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
                clock,
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
        service_account: pod
            .spec
            .as_ref()
            .and_then(|s| s.service_account_name.clone()),
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
    clock: &Clock<'_>,
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
    let mut events = pod_events(events, clock.now);
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
            pod(cluster, p, usage, own, clock)
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

    static UTC: TimeZone = TimeZone::UTC;

    fn clock() -> Clock<'static> {
        Clock {
            now: now(),
            zone: &UTC,
        }
    }

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
                "serviceAccountName": "checkout",
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
        let rows = pods(&cluster(), &[raw], &[event], &usage, &clock());
        let pod = &rows[0];
        assert_eq!(pod.status, Status::Err);
        assert_eq!(pod.reason, "CrashLoopBackOff");
        assert_eq!(pod.phase, PodPhase::Running);
        assert_eq!(pod.qos, Qos::Guaranteed);
        assert_eq!(pod.service_account.as_deref(), Some("checkout"));
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
    fn containers_carry_resources_probes_and_terminations() {
        let raw: Arc<core::Pod> = parse(json!({
            "metadata": { "name": "podinfo-cbf5d9d89-9clpk", "namespace": "web" },
            "spec": { "containers": [
                { "name": "podinfod", "image": "ghcr.io/stefanprodan/podinfo:6.15.0",
                  "resources": { "requests": { "cpu": "1m", "memory": "16Mi" }, "limits": { "cpu": "2" } },
                  "livenessProbe": { "httpGet": { "path": "/healthz", "port": 9898 } },
                  "readinessProbe": { "tcpSocket": { "port": "http" } },
                  "startupProbe": { "exec": { "command": ["true"] } } },
                { "name": "migrate", "image": "migrate:1" }
            ] },
            "status": { "phase": "Running", "containerStatuses": [
                { "name": "podinfod", "image": "i", "imageID": "", "ready": true, "restartCount": 1,
                  "state": { "running": { "startedAt": at(900) } },
                  "lastState": { "terminated": { "exitCode": 1, "reason": "Error",
                      "startedAt": at(1266), "finishedAt": at(900) } } },
                { "name": "migrate", "image": "i", "imageID": "", "ready": false, "restartCount": 0,
                  "state": { "terminated": { "exitCode": 0, "reason": "Completed",
                      "startedAt": at(30), "finishedAt": at(29) } } }
            ] }
        }));
        let row = pod(&cluster(), &raw, None, Vec::new(), &clock());
        let web = &row.containers[0];
        assert_eq!(web.state_label(), "Running · 15h");
        assert_eq!(web.requests.label().as_deref(), Some("cpu 1m · mem 16Mi"));
        assert_eq!(web.limits.label().as_deref(), Some("cpu 2"));
        assert_eq!(
            web.probes_label().as_deref(),
            Some("live :9898/healthz · ready tcp :http · start exec")
        );
        let last = web.last_termination.clone().unwrap_or_default();
        assert_eq!(
            (last.reason.as_str(), last.detail()),
            ("Error", "exit 1 · 15h ago".into())
        );
        assert_eq!(last.ran, Some(Duration::from_mins(366)));
        assert_eq!(
            last.span().as_deref(),
            Some("ran 2027-01-14 10:54 → 2027-01-14 17:00 · 6h 6m")
        );
        let migrate = &row.containers[1];
        assert_eq!(migrate.state_label(), "Terminated · Completed · exit 0");
        assert_eq!(migrate.since, Some(Duration::from_mins(29)));
        let run = migrate.terminated.as_ref().and_then(Termination::run);
        assert_eq!(run.as_deref(), Some("07:30 → 07:31 · 1m"));
        assert_eq!(migrate.requests.label(), None);
        assert_eq!(migrate.last_termination, None);
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
        let row = pod(&cluster(), &raw, None, Vec::new(), &clock());
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
        let rows = pods(&cluster(), &raw, &[], &[], &clock());
        let pending = PodQuery {
            filter: PodFilter::Pending,
            ..PodQuery::default()
        };
        assert_eq!(nav_counts(&rows, &cluster()), Counts::total(3).with_err(1));
        assert_eq!(pending.apply(&rows, &cluster()).len(), 1);
    }
}
