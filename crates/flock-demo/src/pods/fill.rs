use std::collections::BTreeMap;
use std::time::Duration;

use flock_core::pods::{Container, ContainerState, Pod, PodPhase, Qos, Usage};
use flock_core::{ClusterId, Status, Uid};

use super::{CLUSTER, CRASH_LOOP, DAY, HOUR, RUNNING, conditions, owner, running, suffix, volumes};

const NODE_DOMAIN: &str = ".ec2.internal";
const STATEFUL_SET: &str = "StatefulSet";
const DAEMON_SET: &str = "DaemonSet";
const REPLICA_SET: &str = "ReplicaSet";
const JOB: &str = "Job";
const PENDING: &str = "Pending";
const WAITING: &str = "Waiting";
const CPU_BASE: u32 = 12;
const CPU_SPREAD: u32 = 380;
const CPU_STEP: u32 = 37;
const MEMORY_BASE: u32 = 48;
const MEMORY_SPREAD: u32 = 640;
const MEMORY_STEP: u32 = 53;
const SEED_STEP: u64 = 104_729;
const CRASH_RESTARTS: u32 = 9;

#[derive(Clone, Copy)]
enum Health {
    Running,
    Pending(&'static str),
    Crashing(&'static str),
}

#[derive(Clone, Copy)]
struct Group {
    namespace: &'static str,
    kind: &'static str,
    owner: &'static str,
    container: &'static str,
    image: &'static str,
    count: u32,
    age: u64,
    health: Health,
}

const fn group(
    namespace: &'static str,
    kind: &'static str,
    owner: &'static str,
    container: &'static str,
    image: &'static str,
    count: u32,
    age: u64,
) -> Group {
    Group {
        namespace,
        kind,
        owner,
        container,
        image,
        count,
        age,
        health: Health::Running,
    }
}

const fn with(health: Health, group: Group) -> Group {
    Group { health, ..group }
}

const GROUPS: [Group; 36] = [
    group(
        "analytics",
        REPLICA_SET,
        "analytics-api-6d5f8b",
        "api",
        "ghcr.io/acme/analytics-api:3.4.1",
        24,
        9 * DAY,
    ),
    group(
        "analytics",
        REPLICA_SET,
        "event-collector-7b9c4d",
        "collector",
        "ghcr.io/acme/event-collector:1.12.0",
        30,
        9 * DAY,
    ),
    with(
        Health::Pending("0/14 nodes are available: 14 Insufficient cpu."),
        group(
            "analytics",
            JOB,
            "spark-executor-4f8d",
            "executor",
            "ghcr.io/acme/spark:3.5.2",
            3,
            12 * 60,
        ),
    ),
    group(
        "auth",
        REPLICA_SET,
        "auth-server-5c8d7f",
        "server",
        "ghcr.io/acme/auth-server:4.2.0",
        20,
        6 * DAY,
    ),
    group(
        "cert-manager",
        REPLICA_SET,
        "cert-manager-6f7b9c",
        "controller",
        "quay.io/jetstack/cert-manager-controller:v1.15.3",
        3,
        34 * DAY,
    ),
    group(
        "data-pipeline",
        REPLICA_SET,
        "etl-worker-8c6d5f",
        "worker",
        "ghcr.io/acme/etl-worker:2.7.0",
        40,
        2 * DAY,
    ),
    with(
        Health::Crashing("OOMKilled"),
        group(
            "data-pipeline",
            REPLICA_SET,
            "etl-scheduler-5c6d7f",
            "scheduler",
            "ghcr.io/acme/etl-scheduler:2.7.0",
            1,
            2 * DAY,
        ),
    ),
    group(
        "data-pipeline",
        STATEFUL_SET,
        "kafka",
        "kafka",
        "bitnami/kafka:3.8.0",
        3,
        34 * DAY,
    ),
    group(
        "envoy-gateway-system",
        REPLICA_SET,
        "envoy-gateway-5b7d9c",
        "envoy-gateway",
        "envoyproxy/gateway:v1.1.2",
        8,
        12 * DAY,
    ),
    group(
        "feature-flags",
        REPLICA_SET,
        "flagd-6c9f7d",
        "flagd",
        "ghcr.io/open-feature/flagd:v0.11.2",
        4,
        12 * DAY,
    ),
    group(
        "flux-system",
        REPLICA_SET,
        "source-controller-7d8f9c",
        "manager",
        "ghcr.io/fluxcd/source-controller:v1.4.1",
        6,
        23 * DAY,
    ),
    group(
        "ingress-nginx",
        REPLICA_SET,
        "ingress-nginx-controller-6b8c7d",
        "controller",
        "registry.k8s.io/ingress-nginx/controller:v1.11.2",
        6,
        23 * DAY,
    ),
    group(
        "inventory",
        REPLICA_SET,
        "inventory-api-5f9d8c",
        "api",
        "ghcr.io/acme/inventory-api:1.30.2",
        30,
        4 * DAY,
    ),
    group(
        "kube-system",
        DAEMON_SET,
        "kube-proxy",
        "kube-proxy",
        "registry.k8s.io/kube-proxy:v1.31.2",
        14,
        34 * DAY,
    ),
    group(
        "kube-system",
        DAEMON_SET,
        "ebs-csi-node",
        "ebs-plugin",
        "public.ecr.aws/ebs-csi-driver/aws-ebs-csi-driver:v1.34.0",
        14,
        34 * DAY,
    ),
    group(
        "kube-system",
        DAEMON_SET,
        "aws-node",
        "aws-node",
        "amazon-k8s-cni:v1.18.3",
        13,
        34 * DAY,
    ),
    group(
        "kube-system",
        DAEMON_SET,
        "node-local-dns",
        "node-cache",
        "registry.k8s.io/dns/k8s-dns-node-cache:1.23.1",
        14,
        34 * DAY,
    ),
    group(
        "kube-system",
        REPLICA_SET,
        "coredns-5d78c9869d",
        "coredns",
        "registry.k8s.io/coredns/coredns:v1.11.3",
        1,
        34 * DAY,
    ),
    group(
        "kube-system",
        REPLICA_SET,
        "cluster-autoscaler-7c9d8b",
        "cluster-autoscaler",
        "registry.k8s.io/autoscaling/cluster-autoscaler:v1.31.0",
        2,
        34 * DAY,
    ),
    group(
        "kube-system",
        REPLICA_SET,
        "metrics-server-6d94bc",
        "metrics-server",
        "registry.k8s.io/metrics-server/metrics-server:v0.7.2",
        2,
        34 * DAY,
    ),
    group(
        "logging",
        DAEMON_SET,
        "fluent-bit",
        "fluent-bit",
        "cr.fluentbit.io/fluent/fluent-bit:3.1.7",
        14,
        21 * DAY,
    ),
    group(
        "loco-system",
        REPLICA_SET,
        "loco-controller-6f8d7c",
        "controller",
        "ghcr.io/loco/controller:0.9.4",
        3,
        5 * DAY,
    ),
    group(
        "loco-system",
        REPLICA_SET,
        "loco-agent-5d7c9b",
        "agent",
        "ghcr.io/loco/agent:0.9.4",
        3,
        5 * DAY,
    ),
    group(
        "ml",
        REPLICA_SET,
        "model-server-7f6c8d",
        "server",
        "ghcr.io/acme/model-server:0.8.0",
        4,
        3 * DAY,
    ),
    with(
        Health::Pending("0/14 nodes are available: 14 Insufficient nvidia.com/gpu."),
        group(
            "ml",
            JOB,
            "ml-batch-5c7f",
            "trainer",
            "ghcr.io/acme/trainer:cuda12",
            4,
            6 * 60,
        ),
    ),
    group(
        "monitoring",
        DAEMON_SET,
        "node-exporter",
        "node-exporter",
        "quay.io/prometheus/node-exporter:v1.8.2",
        14,
        34 * DAY,
    ),
    group(
        "monitoring",
        REPLICA_SET,
        "kube-state-metrics-6b9f8c",
        "kube-state-metrics",
        "registry.k8s.io/kube-state-metrics/kube-state-metrics:v2.13.0",
        2,
        34 * DAY,
    ),
    group(
        "monitoring",
        STATEFUL_SET,
        "alertmanager-main",
        "alertmanager",
        "quay.io/prometheus/alertmanager:v0.27.0",
        3,
        34 * DAY,
    ),
    group(
        "notifications",
        REPLICA_SET,
        "notifier-6d8c7b",
        "notifier",
        "ghcr.io/acme/notifier:2.3.0",
        24,
        7 * DAY,
    ),
    with(
        Health::Crashing("exit 2"),
        group(
            "notifications",
            REPLICA_SET,
            "mailer-6b7c8d",
            "mailer",
            "ghcr.io/acme/mailer:1.4.0",
            1,
            7 * DAY,
        ),
    ),
    group(
        "orders",
        REPLICA_SET,
        "orders-api-7c8d9f",
        "api",
        "ghcr.io/acme/orders-api:5.0.3",
        36,
        DAY,
    ),
    group(
        "orders",
        REPLICA_SET,
        "orders-worker-6f7c8d",
        "worker",
        "ghcr.io/acme/orders-worker:5.0.3",
        12,
        DAY,
    ),
    with(
        Health::Crashing("exit 1"),
        group(
            "orders",
            REPLICA_SET,
            "orders-sync-7d9c6b",
            "sync",
            "ghcr.io/acme/orders-sync:5.0.3",
            2,
            DAY,
        ),
    ),
    group(
        "payments",
        REPLICA_SET,
        "payments-gateway-5d6c7f",
        "gateway",
        "ghcr.io/acme/payments-gateway:3.1.0",
        20,
        9 * DAY,
    ),
    group(
        "search",
        REPLICA_SET,
        "search-api-6c7d8f",
        "api",
        "ghcr.io/acme/search-api:0.18.0",
        13,
        6 * DAY,
    ),
    group(
        "web",
        REPLICA_SET,
        "web-frontend-84d6f",
        "web",
        "ghcr.io/acme/web-frontend:5.2.1",
        6,
        HOUR,
    ),
];

pub fn pods(nodes: &[String]) -> Vec<Pod> {
    let mut index: u32 = 0;
    let mut out = Vec::new();
    for group in &GROUPS {
        for replica in 0..group.count {
            let node = nodes
                .get(usize::try_from(index).unwrap_or_default() % nodes.len().max(1))
                .cloned();
            out.push(filler(group, replica, index, node));
            index += 1;
        }
    }
    out
}

fn name(group: &Group, replica: u32, index: u32) -> String {
    if group.kind == STATEFUL_SET {
        return format!("{}-{replica}", group.owner);
    }
    let seed = (u64::from(index) + 1).wrapping_mul(SEED_STEP);
    format!("{}-{}", group.owner, suffix(seed))
}

fn container(group: &Group) -> Container {
    let base = running(group.container, group.image, 0, Vec::new());
    match group.health {
        Health::Running => base,
        Health::Pending(note) => Container {
            state: ContainerState::Waiting,
            status: Status::Warn,
            reason: WAITING.into(),
            ready: false,
            note: Some(note.into()),
            message: note.into(),
            ..base
        },
        Health::Crashing(note) => Container {
            state: ContainerState::Waiting,
            status: Status::Err,
            reason: CRASH_LOOP.into(),
            ready: false,
            restarts: CRASH_RESTARTS,
            note: Some(note.into()),
            message: format!("last {note}"),
            ..base
        },
    }
}

fn filler(group: &Group, replica: u32, index: u32, node: Option<String>) -> Pod {
    let (phase, status, reason) = match group.health {
        Health::Running => (PodPhase::Running, Status::Ok, RUNNING),
        Health::Pending(_) => (PodPhase::Pending, Status::Warn, PENDING),
        Health::Crashing(_) => (PodPhase::Running, Status::Err, CRASH_LOOP),
    };
    let scheduled = !matches!(group.health, Health::Pending(_));
    let node = node
        .filter(|_| scheduled)
        .map(|node| format!("{node}{NODE_DOMAIN}"));
    let usage = scheduled.then(|| Usage {
        cpu_millis: CPU_BASE + index.wrapping_mul(CPU_STEP) % CPU_SPREAD,
        memory_mib: MEMORY_BASE + index.wrapping_mul(MEMORY_STEP) % MEMORY_SPREAD,
    });
    let app = group
        .owner
        .rsplit_once('-')
        .map_or(group.owner, |(stem, _)| stem);
    let app = if group.kind == REPLICA_SET || group.kind == JOB {
        app
    } else {
        group.owner
    };
    let mut pod = Pod {
        uid: Uid::default(),
        cluster: ClusterId::new(CLUSTER),
        namespace: group.namespace.into(),
        name: name(group, replica, index),
        phase,
        status,
        reason: reason.into(),
        node,
        ip: None,
        owner: Some(owner(group.kind, group.namespace, group.owner)),
        qos: Qos::Burstable,
        age: Duration::from_secs(group.age),
        labels: BTreeMap::from([("app".to_owned(), app.to_owned())]),
        containers: vec![container(group)],
        usage,
        conditions: Vec::new(),
        volumes: volumes(&[]),
        events: Vec::new(),
    };
    pod.conditions = conditions(&pod);
    pod
}
