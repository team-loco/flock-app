use std::collections::BTreeMap;
use std::time::Duration;

use flock_core::pods::{
    Container, ContainerPort, ContainerState, Pod, PodEvent, PodPhase, Probe, ProbeKind, Qos,
    Resources, Termination, Usage, Volume,
};
use flock_core::related::{REPLICA_SET, TEMPLATE_HASH};
use flock_core::{ClusterId, Condition, ResourceRef, Status, Uid};

mod fill;

const CLUSTER: &str = "prod-us-east";
const SERVICE_ACCOUNT: &str = "default";
const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;
const TCP: &str = "TCP";
const UDP: &str = "UDP";
const RUNNING: &str = "Running";
const WAITING: &str = "Waiting";
const CRASH_LOOP: &str = "CrashLoopBackOff";
const NOT_READY: &str = "Running · not ready";
const CONTAINERS_NOT_READY: &str = "ContainersNotReady";
const UNSCHEDULABLE: &str = "Unschedulable";
const TRUE: &str = "True";
const FALSE: &str = "False";
const SCHEDULING_MESSAGE: &str = "0/14 nodes are available: 14 Insufficient nvidia.com/gpu.";
const API_ACCESS: (&str, &str) = ("kube-api-access", "projected · token, ca.crt, namespace");

const NODE_12_87: &str = "ip-10-4-12-87.ec2.internal";
const NODE_14_201: &str = "ip-10-4-14-201.ec2.internal";
const NODE_22_63: &str = "ip-10-4-22-63.ec2.internal";
const NODE_30_5: &str = "ip-10-4-30-5.ec2.internal";

const CHECKOUT_IMAGE: &str = "ghcr.io/acme/checkout-api:2.14.0";
const ENVOY_IMAGE: &str = "envoyproxy/envoy:v1.31.2";

fn age(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

fn port(port: u16, protocol: &str, name: &str) -> ContainerPort {
    ContainerPort {
        port,
        protocol: protocol.into(),
        name: Some(name.into()),
    }
}

fn running(name: &str, image: &str, restarts: u32, ports: Vec<ContainerPort>) -> Container {
    Container {
        name: name.into(),
        image: image.into(),
        state: ContainerState::Running,
        status: Status::Ok,
        reason: RUNNING.into(),
        ready: true,
        restarts,
        ports,
        note: None,
        message: String::new(),
        ..Container::default()
    }
}

fn waiting(name: &str, image: &str, note: &str) -> Container {
    Container {
        name: name.into(),
        image: image.into(),
        state: ContainerState::Waiting,
        status: Status::Warn,
        reason: WAITING.into(),
        ready: false,
        restarts: 0,
        ports: Vec::new(),
        note: Some(note.into()),
        message: note.into(),
        ..Container::default()
    }
}

fn envoy() -> Container {
    running("envoy", ENVOY_IMAGE, 0, vec![port(9901, TCP, "admin")])
}

fn resources(cpu: &str, memory: &str) -> Resources {
    Resources {
        cpu: Some(cpu.into()),
        memory: Some(memory.into()),
    }
}

fn probe(kind: ProbeKind, target: &str) -> Probe {
    Probe {
        kind,
        target: target.into(),
    }
}

fn checkout_api() -> Container {
    Container {
        requests: resources("250m", "256Mi"),
        limits: resources("1", "512Mi"),
        probes: vec![
            probe(ProbeKind::Liveness, ":8080/healthz"),
            probe(ProbeKind::Readiness, ":8080/readyz"),
        ],
        since: Some(age(3 * HOUR)),
        ..running("api", CHECKOUT_IMAGE, 0, vec![port(8080, TCP, "http")])
    }
}

fn owner(kind: &str, namespace: &str, name: &str) -> ResourceRef {
    ResourceRef {
        cluster: ClusterId::new(CLUSTER),
        kind: kind.into(),
        namespace: Some(namespace.into()),
        name: name.into(),
    }
}

fn event(status: Status, reason: &str, message: &str, count: u32, seconds: u64) -> PodEvent {
    PodEvent {
        status,
        reason: reason.into(),
        message: message.into(),
        count,
        age: age(seconds),
    }
}

fn volumes(extra: &[(&str, &str)]) -> Vec<Volume> {
    extra
        .iter()
        .chain(std::iter::once(&API_ACCESS))
        .map(|(name, source)| Volume {
            name: (*name).into(),
            source: (*source).into(),
        })
        .collect()
}

pub fn conditions(pod: &Pod) -> Vec<Condition> {
    let since = Some(pod.age);
    if pod.node.is_none() {
        let scheduled = Condition::new("PodScheduled", FALSE)
            .reason(UNSCHEDULABLE)
            .message(SCHEDULING_MESSAGE)
            .age(since);
        return vec![scheduled];
    }
    let ready = pod.containers.iter().all(|c| c.ready);
    let value = if ready { TRUE } else { FALSE };
    let unready: Vec<&str> = pod
        .containers
        .iter()
        .filter(|c| !c.ready)
        .map(|c| c.name.as_str())
        .collect();
    let (reason, message) = if ready {
        (String::new(), String::new())
    } else {
        let message = format!("containers with unready status: [{}]", unready.join(" "));
        (CONTAINERS_NOT_READY.to_owned(), message)
    };
    let readiness = |kind: &str| {
        Condition::new(kind, value)
            .reason(reason.clone())
            .message(message.clone())
            .age(since)
    };
    vec![
        Condition::new("PodScheduled", TRUE).age(since),
        Condition::new("Initialized", TRUE).age(since),
        readiness("ContainersReady"),
        readiness("Ready"),
    ]
}

struct Seed {
    namespace: &'static str,
    name: &'static str,
    phase: PodPhase,
    status: Status,
    reason: &'static str,
    age: u64,
    node: Option<&'static str>,
    ip: Option<&'static str>,
    owner: (&'static str, &'static str),
    qos: Qos,
    labels: &'static [(&'static str, &'static str)],
    containers: Vec<Container>,
    usage: Option<(u32, u32)>,
    volumes: &'static [(&'static str, &'static str)],
    events: Vec<PodEvent>,
}

fn template_hash(kind: &str, owner: &str) -> Option<(String, String)> {
    let (_, hash) = owner.rsplit_once('-').filter(|_| kind == REPLICA_SET)?;
    Some((TEMPLATE_HASH.to_owned(), hash.to_owned()))
}

fn pod(seed: Seed) -> Pod {
    let hash = template_hash(seed.owner.0, seed.owner.1);
    let labels: BTreeMap<String, String> = seed
        .labels
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .chain(hash)
        .collect();
    let mut pod = Pod {
        uid: Uid::default(),
        cluster: ClusterId::new(CLUSTER),
        namespace: seed.namespace.into(),
        name: seed.name.into(),
        phase: seed.phase,
        status: seed.status,
        reason: seed.reason.into(),
        node: seed.node.map(Into::into),
        ip: seed.ip.map(Into::into),
        owner: Some(owner(seed.owner.0, seed.namespace, seed.owner.1)),
        service_account: Some(SERVICE_ACCOUNT.into()),
        qos: seed.qos,
        age: age(seed.age),
        labels,
        containers: seed.containers,
        usage: seed.usage.map(|(cpu_millis, memory_mib)| Usage {
            cpu_millis,
            memory_mib,
        }),
        conditions: Vec::new(),
        volumes: volumes(seed.volumes),
        events: seed.events,
    };
    pod.conditions = conditions(&pod);
    pod
}

const CHECKOUT_LABELS: &[(&str, &str)] = &[("app", "checkout-api"), ("version", "2.14.0")];
const CHECKOUT_VOLUMES: &[(&str, &str)] = &[
    ("config", "configmap checkout-api-config"),
    ("secrets", "secret checkout-api-secrets"),
];

fn healthy_checkout(
    name: &'static str,
    node: &'static str,
    ip: &'static str,
    usage: (u32, u32),
) -> Seed {
    Seed {
        namespace: "payments",
        name,
        phase: PodPhase::Running,
        status: Status::Ok,
        reason: RUNNING,
        age: 3 * HOUR,
        node: Some(node),
        ip: Some(ip),
        owner: ("ReplicaSet", "checkout-api-7f9c6d"),
        qos: Qos::Burstable,
        labels: CHECKOUT_LABELS,
        containers: vec![checkout_api(), envoy()],
        usage: Some(usage),
        volumes: CHECKOUT_VOLUMES,
        events: Vec::new(),
    }
}

fn crashing_checkout() -> Seed {
    let api = Container {
        state: ContainerState::Waiting,
        status: Status::Err,
        reason: CRASH_LOOP.into(),
        ready: false,
        restarts: 14,
        note: Some("exit 1".into()),
        message: "last exit Error (1)".into(),
        since: None,
        last_termination: Some(Termination {
            reason: "Error".into(),
            exit_code: 1,
            started: Some("14:08".into()),
            finished: Some("14:09".into()),
            ago: Some(age(2 * MINUTE)),
            ran: Some(age(MINUTE + 12)),
        }),
        ..checkout_api()
    };
    Seed {
        labels: &[
            ("app", "checkout-api"),
            ("pod-template-hash", "7f9c6d"),
            ("version", "2.14.0"),
        ],
        status: Status::Err,
        reason: CRASH_LOOP,
        containers: vec![api, envoy()],
        events: vec![
            event(
                Status::Err,
                "BackOff",
                "Back-off restarting failed container api in pod checkout-api-7f9c6d-x2lqp",
                14,
                2 * MINUTE,
            ),
            event(
                Status::Ok,
                "Pulled",
                "Container image \"ghcr.io/acme/checkout-api:2.14.0\" already present on machine",
                15,
                3 * HOUR,
            ),
            event(
                Status::Ok,
                "Scheduled",
                "Successfully assigned payments/checkout-api-7f9c6d-x2lqp to ip-10-4-12-87.ec2.internal",
                1,
                3 * HOUR,
            ),
        ],
        ..healthy_checkout(
            "checkout-api-7f9c6d-x2lqp",
            NODE_12_87,
            "10.4.13.22",
            (4, 38),
        )
    }
}

fn web(name: &'static str, node: &'static str, ip: &'static str, usage: (u32, u32)) -> Seed {
    Seed {
        namespace: "web",
        name,
        phase: PodPhase::Running,
        status: Status::Ok,
        reason: RUNNING,
        age: 21 * MINUTE,
        node: Some(node),
        ip: Some(ip),
        owner: ("ReplicaSet", "web-frontend-84d6f"),
        qos: Qos::Burstable,
        labels: &[("app", "web-frontend")],
        containers: vec![running(
            "web",
            "ghcr.io/acme/web-frontend:5.2.1",
            0,
            vec![port(3000, TCP, "http")],
        )],
        usage: Some(usage),
        volumes: &[],
        events: Vec::new(),
    }
}

fn platform_seeds() -> Vec<Seed> {
    vec![
        Seed {
            namespace: "monitoring",
            name: "prometheus-k8s-0",
            phase: PodPhase::Running,
            status: Status::Ok,
            reason: RUNNING,
            age: 34 * DAY,
            node: Some(NODE_30_5),
            ip: Some("10.4.31.90"),
            owner: ("StatefulSet", "prometheus-k8s"),
            qos: Qos::Burstable,
            labels: &[("app.kubernetes.io/name", "prometheus")],
            containers: vec![running(
                "prometheus",
                "quay.io/prometheus/prometheus:v2.54.1",
                0,
                vec![port(9090, TCP, "web")],
            )],
            usage: Some((450, 3277)),
            volumes: &[(
                "prometheus-k8s-db",
                "pvc prometheus-k8s-db-prometheus-k8s-0",
            )],
            events: Vec::new(),
        },
        Seed {
            namespace: "kube-system",
            name: "coredns-5d78c9869d-8hjvq",
            phase: PodPhase::Running,
            status: Status::Ok,
            reason: RUNNING,
            age: 34 * DAY,
            node: Some(NODE_12_87),
            ip: Some("10.4.13.2"),
            owner: ("ReplicaSet", "coredns-5d78c9869d"),
            qos: Qos::Burstable,
            labels: &[("k8s-app", "kube-dns")],
            containers: vec![running(
                "coredns",
                "registry.k8s.io/coredns/coredns:v1.11.3",
                0,
                vec![port(53, UDP, "dns")],
            )],
            usage: Some((3, 22)),
            volumes: &[("config-volume", "configmap coredns")],
            events: Vec::new(),
        },
        Seed {
            namespace: "kube-system",
            name: "aws-node-7kx2p",
            phase: PodPhase::Running,
            status: Status::Ok,
            reason: RUNNING,
            age: 34 * DAY,
            node: Some(NODE_12_87),
            ip: Some("10.4.12.87"),
            owner: ("DaemonSet", "aws-node"),
            qos: Qos::Burstable,
            labels: &[("k8s-app", "aws-node")],
            containers: vec![running("aws-node", "amazon-k8s-cni:v1.18.3", 0, Vec::new())],
            usage: Some((5, 48)),
            volumes: &[("cni-bin-dir", "hostPath /opt/cni/bin")],
            events: Vec::new(),
        },
    ]
}

fn workload_seeds() -> Vec<Seed> {
    vec![
        Seed {
            namespace: "payments",
            name: "payments-worker-5d8b-7hx2c",
            phase: PodPhase::Running,
            status: Status::Ok,
            reason: RUNNING,
            age: 2 * DAY,
            node: Some(NODE_12_87),
            ip: Some("10.4.13.57"),
            owner: ("ReplicaSet", "payments-worker-5d8b"),
            qos: Qos::Guaranteed,
            labels: &[("app", "payments-worker")],
            containers: vec![running(
                "worker",
                "ghcr.io/acme/payments-worker:1.9.3",
                1,
                Vec::new(),
            )],
            usage: Some((96, 120)),
            volumes: &[],
            events: Vec::new(),
        },
        Seed {
            namespace: "payments",
            name: "payments-db-0",
            phase: PodPhase::Running,
            status: Status::Ok,
            reason: RUNNING,
            age: 34 * DAY,
            node: Some(NODE_30_5),
            ip: Some("10.4.31.18"),
            owner: ("StatefulSet", "payments-db"),
            qos: Qos::Guaranteed,
            labels: &[("app", "payments-db"), ("role", "primary")],
            containers: vec![running(
                "postgres",
                "postgres:16.4",
                0,
                vec![port(5432, TCP, "postgres")],
            )],
            usage: Some((310, 1434)),
            volumes: &[("data", "pvc data-payments-db-0")],
            events: Vec::new(),
        },
        web(
            "web-frontend-84d6f-2xk9p",
            NODE_22_63,
            "10.4.23.77",
            (41, 156),
        ),
        web(
            "web-frontend-84d6f-c7v3n",
            NODE_14_201,
            "10.4.15.62",
            (38, 149),
        ),
    ]
}

fn problem_seeds() -> Vec<Seed> {
    let indexer = Container {
        status: Status::Warn,
        reason: NOT_READY.into(),
        ready: false,
        restarts: 2,
        note: Some("readiness 503".into()),
        message: "Readiness probe failed: HTTP probe failed with statuscode: 503".into(),
        ..running(
            "indexer",
            "ghcr.io/acme/search-indexer:0.18.0",
            0,
            Vec::new(),
        )
    };
    vec![
        Seed {
            namespace: "search",
            name: "search-indexer-0",
            phase: PodPhase::Running,
            status: Status::Warn,
            reason: NOT_READY,
            age: 6 * DAY,
            node: Some(NODE_30_5),
            ip: Some("10.4.31.44"),
            owner: ("StatefulSet", "search-indexer"),
            qos: Qos::Burstable,
            labels: &[("app", "search-indexer")],
            containers: vec![indexer],
            usage: Some((640, 2150)),
            volumes: &[("data", "pvc data-search-indexer-0")],
            events: vec![event(
                Status::Warn,
                "Unhealthy",
                "Readiness probe failed: HTTP probe failed with statuscode: 503",
                212,
                40,
            )],
        },
        Seed {
            namespace: "ml",
            name: "ml-batch-5c7f-9kq2d",
            phase: PodPhase::Pending,
            status: Status::Warn,
            reason: "Pending",
            age: 6 * MINUTE,
            node: None,
            ip: None,
            owner: ("Job", "ml-batch-5c7f"),
            qos: Qos::Guaranteed,
            labels: &[("job-name", "ml-batch-5c7f")],
            containers: vec![waiting(
                "trainer",
                "ghcr.io/acme/trainer:cuda12",
                "unschedulable · nvidia.com/gpu: 1",
            )],
            usage: None,
            volumes: &[("scratch", "emptyDir")],
            events: vec![event(
                Status::Warn,
                "FailedScheduling",
                "0/14 nodes are available: 14 Insufficient nvidia.com/gpu.",
                7,
                MINUTE,
            )],
        },
        Seed {
            namespace: "monitoring",
            name: "grafana-6b8d9-tw4mn",
            phase: PodPhase::Pending,
            status: Status::Warn,
            reason: "ContainerCreating",
            age: 18 * MINUTE,
            node: Some(NODE_14_201),
            ip: None,
            owner: ("ReplicaSet", "grafana-6b8d9"),
            qos: Qos::BestEffort,
            labels: &[("app.kubernetes.io/name", "grafana")],
            containers: vec![waiting(
                "grafana",
                "grafana/grafana:11.2.0",
                "FailedMount · dashboards",
            )],
            usage: None,
            volumes: &[
                ("dashboards", "configmap grafana-dashboards"),
                ("storage", "emptyDir"),
            ],
            events: vec![event(
                Status::Warn,
                "FailedMount",
                "MountVolume.SetUp failed for volume \"dashboards\" : configmap \"grafana-dashboards\" not found",
                9,
                2 * MINUTE,
            )],
        },
    ]
}

pub fn pods() -> Vec<Pod> {
    let checkout = vec![
        crashing_checkout(),
        healthy_checkout(
            "checkout-api-7f9c6d-m8vtn",
            NODE_14_201,
            "10.4.15.9",
            (212, 301),
        ),
        healthy_checkout(
            "checkout-api-7f9c6d-q4zhk",
            NODE_22_63,
            "10.4.23.140",
            (187, 288),
        ),
    ];
    let nodes: Vec<String> = crate::detail::nodes()
        .into_iter()
        .filter(|node| node.schedulable)
        .map(|node| node.name)
        .collect();
    checkout
        .into_iter()
        .chain(workload_seeds())
        .chain(problem_seeds())
        .chain(platform_seeds())
        .map(pod)
        .chain(fill::pods(&nodes))
        .collect()
}

const TERMINATING: &str = "Terminating";
const CREATING: &str = "ContainerCreating";
const SCHEDULED: &str = "Scheduled";
const STATEFUL_SET: &str = "StatefulSet";
const REPLACED_BY: &[&str] = &["ReplicaSet", STATEFUL_SET, "DaemonSet"];
const SUFFIX_ALPHABET: &[u8] = b"bcdfghjklmnpqrstvwxz2456789";
const SUFFIX_LEN: usize = 5;
const SUFFIX_SEED: u64 = 48_271;
const SUFFIX_STEP: u64 = 7_919;

fn find<'a>(pods: &'a mut [Pod], target: &ResourceRef) -> Option<&'a mut Pod> {
    pods.iter_mut().find(|pod| pod.is(target))
}

pub fn terminate(pods: &mut [Pod], target: &ResourceRef) -> bool {
    let Some(pod) = find(pods, target) else {
        return false;
    };
    pod.status = Status::Progress;
    pod.reason = TERMINATING.into();
    true
}

pub fn replace(pods: &mut Vec<Pod>, target: &ResourceRef) -> Option<ResourceRef> {
    let at = pods.iter().position(|pod| pod.is(target))?;
    let old = pods.remove(at);
    let replaced = old
        .owner
        .as_ref()
        .is_some_and(|owner| REPLACED_BY.contains(&owner.kind.as_str()));
    if !replaced {
        return None;
    }
    let name = replacement_name(&old);
    let fresh = creating(old, name);
    let next = fresh.resource();
    pods.insert(at, fresh);
    Some(next)
}

pub fn start(pods: &mut [Pod], target: &ResourceRef) {
    let Some(pod) = find(pods, target) else {
        return;
    };
    pod.phase = PodPhase::Running;
    pod.status = Status::Ok;
    pod.reason = RUNNING.into();
    for container in &mut pod.containers {
        container.state = ContainerState::Running;
        container.status = Status::Ok;
        container.reason = RUNNING.into();
        container.ready = true;
    }
    pod.conditions = conditions(pod);
}

fn replacement_name(old: &Pod) -> String {
    let stateful = old
        .owner
        .as_ref()
        .is_some_and(|owner| owner.kind == STATEFUL_SET);
    if stateful {
        return old.name.clone();
    }
    let seed = old.name.bytes().fold(SUFFIX_SEED, |acc, b| {
        acc.wrapping_mul(SUFFIX_STEP).wrapping_add(u64::from(b))
    });
    let suffix = suffix(seed);
    match old.name.rsplit_once('-') {
        Some((stem, _)) => format!("{stem}-{suffix}"),
        None => format!("{}-{suffix}", old.name),
    }
}

fn suffix(seed: u64) -> String {
    let base = SUFFIX_ALPHABET.len() as u64;
    let mut value = seed;
    let mut out = String::with_capacity(SUFFIX_LEN);
    for _ in 0..SUFFIX_LEN {
        let digit = usize::try_from(value % base).unwrap_or_default();
        let letter = SUFFIX_ALPHABET.get(digit).copied().unwrap_or(b'x');
        out.push(char::from(letter));
        value /= base;
    }
    out
}

fn creating(old: Pod, name: String) -> Pod {
    let scheduled = PodEvent {
        status: Status::Ok,
        reason: SCHEDULED.into(),
        message: format!(
            "Successfully assigned {}/{name} to {}",
            old.namespace,
            old.node.clone().unwrap_or_default()
        ),
        count: 1,
        age: Duration::ZERO,
    };
    let containers = old
        .containers
        .into_iter()
        .map(|container| Container {
            state: ContainerState::Waiting,
            status: Status::Progress,
            reason: CREATING.into(),
            ready: false,
            restarts: 0,
            note: None,
            ..container
        })
        .collect();
    let mut pod = Pod {
        name,
        phase: PodPhase::Pending,
        status: Status::Progress,
        reason: CREATING.into(),
        age: Duration::ZERO,
        containers,
        events: vec![scheduled],
        ..old
    };
    pod.conditions = conditions(&pod);
    pod
}

#[cfg(test)]
mod tests {
    use flock_core::pods::{PodFilter, filter_counts};

    use super::*;

    const DESIGN_ROWS: usize = 13;

    #[test]
    fn names_are_unique() {
        let pods = pods();
        let names: std::collections::BTreeSet<(&str, &str)> = pods
            .iter()
            .map(|p| (p.namespace.as_str(), p.name.as_str()))
            .collect();
        assert_eq!(names.len(), pods.len());
    }

    #[test]
    fn dataset_matches_the_design_rows() {
        let pods = pods();
        let names: Vec<&str> = pods
            .iter()
            .take(DESIGN_ROWS)
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "checkout-api-7f9c6d-x2lqp",
                "checkout-api-7f9c6d-m8vtn",
                "checkout-api-7f9c6d-q4zhk",
                "payments-worker-5d8b-7hx2c",
                "payments-db-0",
                "web-frontend-84d6f-2xk9p",
                "web-frontend-84d6f-c7v3n",
                "search-indexer-0",
                "ml-batch-5c7f-9kq2d",
                "grafana-6b8d9-tw4mn",
                "prometheus-k8s-0",
                "coredns-5d78c9869d-8hjvq",
                "aws-node-7kx2p",
            ]
        );
        let restarts: Vec<u32> = pods.iter().take(DESIGN_ROWS).map(Pod::restarts).collect();
        assert_eq!(restarts, [14, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn filters_count_the_sample() {
        let pods = pods();
        let refs: Vec<&Pod> = pods.iter().collect();
        let counts = filter_counts(&refs);
        assert_eq!(
            counts,
            [
                (PodFilter::All, 412),
                (PodFilter::Attention, 15),
                (PodFilter::Running, 403),
                (PodFilter::Pending, 9),
                (PodFilter::Failed, 0),
                (PodFilter::Succeeded, 0),
            ]
        );
    }

    #[test]
    fn conditions_follow_scheduling_and_readiness() {
        let pods = pods();
        let ready = |name: &str| {
            pods.iter().find(|p| p.name == name).map(|p| {
                p.conditions
                    .iter()
                    .map(Condition::is_healthy)
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(ready("payments-db-0"), Some(vec![true; 4]));
        assert_eq!(
            ready("checkout-api-7f9c6d-x2lqp"),
            Some(vec![true, true, false, false])
        );
        assert_eq!(ready("ml-batch-5c7f-9kq2d"), Some(vec![false]));
    }

    fn by_name(pods: &[Pod], name: &str) -> Option<Pod> {
        pods.iter().find(|p| p.name == name).cloned()
    }

    #[test]
    fn deleting_a_replicaset_pod_terminates_then_replaces_it() {
        let mut all = pods();
        let target = by_name(&all, "checkout-api-7f9c6d-x2lqp").map(|p| p.resource());
        let Some(target) = target else {
            panic!("sample pod missing");
        };
        assert!(terminate(&mut all, &target));
        assert_eq!(
            by_name(&all, "checkout-api-7f9c6d-x2lqp").map(|p| p.status),
            Some(Status::Progress)
        );
        let Some(fresh) = replace(&mut all, &target) else {
            panic!("expected a replacement");
        };
        assert!(by_name(&all, "checkout-api-7f9c6d-x2lqp").is_none());
        assert!(fresh.name.starts_with("checkout-api-7f9c6d-"));
        assert_ne!(fresh.name, target.name);
        let created = by_name(&all, &fresh.name).unwrap_or_default();
        assert_eq!((created.status, created.restarts()), (Status::Progress, 0));
        start(&mut all, &fresh);
        let started = by_name(&all, &fresh.name).unwrap_or_default();
        assert_eq!(
            (started.status, started.ready_label().as_str()),
            (Status::Ok, "2/2")
        );
    }

    #[test]
    fn statefulset_pods_keep_their_name_and_job_pods_are_not_replaced() {
        let mut all = pods();
        let db = by_name(&all, "payments-db-0")
            .unwrap_or_default()
            .resource();
        assert_eq!(
            replace(&mut all, &db).map(|r| r.name),
            Some("payments-db-0".to_owned())
        );
        let job = by_name(&all, "ml-batch-5c7f-9kq2d")
            .unwrap_or_default()
            .resource();
        assert_eq!(replace(&mut all, &job), None);
        assert!(by_name(&all, "ml-batch-5c7f-9kq2d").is_none());
    }
}
