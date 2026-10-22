use std::collections::BTreeMap;
use std::time::Duration;

use flock_core::Condition;
use flock_core::cluster::Namespace;
use flock_core::cluster_changes::{ChangeKind, ClusterChange};
use flock_core::control_plane::{
    ApiService, Check, ControlPlane, ETCD_CHECK, FailedRequests, Health, Probe, Traffic, Webhook,
};
use flock_core::events::Event;
use flock_core::expiry::{Expiring, SupportEnd};
use flock_core::home::Problem;
use flock_core::nodes::{
    CONDITION_TRUE, Node, NodePod, READY_CONDITION, TOP_PODS, Usage, state_label,
};
use flock_core::overview::{Commitment, NetworkFacts, Overview, PodPhases, TimeRange, Utilization};
use flock_core::pod_tops::PodTops;
use flock_core::pods::{Pod, PodFilter, failing};
use flock_core::{ClusterDetail, ClusterId, Counts, ResourceRef, Status, Uid};

use crate::lists;

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;

const NODE_CPU: f32 = 8.;
const GENERAL_MEMORY: f32 = 36.;
const GPU_MEMORY: f32 = 40.;
const POD_CAPACITY: u32 = 110;
const MILLIS_PER_CORE: f32 = 1000.;
const MIB_PER_GIB: f32 = 1024.;
const KUBELET: &str = "v1.31.2-eks-7f9249a";
const GPU_KUBELET: &str = "v1.30.4-eks-a737599";
const CPU_LIMIT_RATIO: f32 = 1.4;
const MEMORY_LIMIT_RATIO: f32 = 1.2;
const OS_IMAGE: &str = "Amazon Linux 2023.5.20240916";
const EVENT_FIRST_AGE: Duration = Duration::from_secs(3600);
const BAR_SEED: u64 = 11;
const BAR_MULTIPLIER: u64 = 9301;
const BAR_INCREMENT: u64 = 49297;
const BAR_MODULUS: u64 = 233_280;
const BAR_MINUTES: u64 = 60;
const INCIDENT_MINUTES: std::ops::RangeInclusive<u64> = 14..=22;
const ROLLOUT_MINUTES: std::ops::RangeInclusive<u64> = 33..=35;
const EVENT_NOW: jiff::Timestamp = jiff::Timestamp::constant(1_791_873_360, 0);

const DEMO_CLUSTER: &str = "prod-us-east";
const RUNTIME: &str = "containerd://1.7.20";
const GENERAL: &str = "m6i.4xlarge";
const GPU: &str = "g5.2xlarge";
const UNSCHEDULABLE_TAINT: &str = "node.kubernetes.io/unschedulable:NoSchedule";
const GPU_TAINT: &str = "nvidia.com/gpu=present:NoSchedule";

pub fn minutes(n: u64) -> Duration {
    Duration::from_secs(n * MINUTE)
}

pub fn hours(n: u64) -> Duration {
    Duration::from_secs(n * HOUR)
}

pub fn days(n: u64) -> Duration {
    Duration::from_secs(n * DAY)
}

pub fn prod_us_east(pods: &[Pod]) -> ClusterDetail {
    let resources = lists::prod_us_east();
    ClusterDetail {
        overview: overview(pods),
        nodes: nodes(pods),
        namespaces: namespaces(),
        events: events(),
        resources,
        related: Vec::new(),
        namespace_details: Vec::new(),
        objects: Vec::new(),
    }
}

const UNSCHEDULABLE_PODS: u32 = 6;

fn pod_phases(pods: &[Pod]) -> PodPhases {
    let tally = |keep: &dyn Fn(&Pod) -> bool| {
        let matching = pods.iter().filter(|pod| keep(pod)).count();
        u32::try_from(matching).unwrap_or(u32::MAX)
    };
    PodPhases {
        running: tally(&|pod| PodFilter::Running.keeps(pod) && !failing(pod)),
        pending: tally(&|pod| PodFilter::Pending.keeps(pod)),
        failing: tally(&failing),
        unschedulable: UNSCHEDULABLE_PODS,
    }
}

fn overview(pods: &[Pod]) -> Overview {
    Overview {
        utilization_source: flock_core::overview::UtilizationSource::Prometheus,
        cpu: Commitment {
            requests: 0.74,
            limits: 1.31,
        },
        memory: Commitment {
            requests: 0.66,
            limits: 0.92,
        },
        pods: pod_phases(pods),
        utilization: utilization(),
        tops: PodTops::default(),
        network: NetworkFacts {
            services_without_endpoints: Some(2),
            load_balancers_pending: 1,
            pvcs_pending: 0,
        },
        expiring: expiring(),
        support: Some(SupportEnd {
            label: "EKS v1.31 extended support ends".into(),
            date: jiff::civil::date(2026, 11, 26),
            days: 37,
        }),
        unready_services: unready_services(),
        changes: changes(),
        control_plane: control_plane(),
    }
}

fn errors_per_minute() -> Vec<u64> {
    let mut seed = BAR_SEED;
    (0..BAR_MINUTES)
        .map(|minute| {
            seed = (seed * BAR_MULTIPLIER + BAR_INCREMENT) % BAR_MODULUS;
            let jitter = seed * 3 / BAR_MODULUS;
            let mut count = 1 + jitter;
            if INCIDENT_MINUTES.contains(&minute) {
                count += 9 + seed * 10 / BAR_MODULUS;
            }
            if ROLLOUT_MINUTES.contains(&minute) {
                count += 6;
            }
            count
        })
        .collect()
}

fn failed(code: &str, verb: &str, resource: &str, count: u64) -> FailedRequests {
    FailedRequests {
        code: code.into(),
        verb: verb.into(),
        resource: resource.into(),
        count,
    }
}

fn webhook(name: &str, configuration: &str, timeout: u64) -> Webhook {
    Webhook {
        name: name.into(),
        configuration: configuration.into(),
        timeout: Duration::from_secs(timeout),
    }
}

fn control_plane() -> ControlPlane {
    let checks = ["ping", "log", ETCD_CHECK, "informer-sync", "shutdown"]
        .into_iter()
        .map(|name| Check {
            name: name.into(),
            passed: true,
        })
        .collect();
    let traffic = Traffic {
        since: "09:58".into(),
        until: "10:57".into(),
        window: Duration::from_hours(1),
        requests: 1_071_382,
        errors: 214,
        p99: Some(Duration::from_millis(182)),
        etcd_p99: Some(Duration::from_millis(24)),
        errors_per_minute: errors_per_minute(),
        failures: vec![
            failed("503", "GET", "pods.metrics.k8s.io", 148),
            failed("504", "LIST", "pods", 41),
            failed("503", "GET", "nodes.metrics.k8s.io", 10),
            failed("500", "CREATE", "pods", 9),
            failed("500", "PATCH", "deployments.apps", 6),
        ],
        webhook_p99: vec![
            (
                "validate.kyverno.svc-fail".into(),
                Duration::from_millis(2800),
            ),
            ("mutate.kyverno.svc-fail".into(), Duration::from_millis(410)),
            ("webhook.cert-manager.io".into(), Duration::from_millis(12)),
        ],
    };
    let kyverno = "kyverno-resource-validating";
    let webhooks = vec![
        webhook("validate.kyverno.svc-fail", kyverno, 10),
        webhook("mutate.kyverno.svc-fail", "kyverno-resource-mutating", 10),
        webhook("webhook.cert-manager.io", "cert-manager-webhook", 30),
        webhook("vpod.kb.io", "aws-load-balancer-webhook", 10),
        webhook("mpod.kb.io", "aws-load-balancer-webhook", 10),
        webhook("validation.gatekeeper.sh", "gatekeeper-validating", 3),
    ];
    ControlPlane {
        health: Probe::Ready(Health { checks }),
        traffic: Probe::Ready(traffic),
        webhooks: Probe::Ready(webhooks),
        unavailable_services: vec![ApiService {
            name: "v1beta1.metrics.k8s.io".into(),
            reason: "FailedDiscoveryCheck".into(),
        }],
    }
}

fn expiring() -> Vec<Expiring> {
    [
        (
            "ingress",
            "wildcard-onloco",
            9,
            "Ready=False Failed · ACME order failed: 429 rateLimited",
        ),
        (
            "kyverno",
            "webhook-tls",
            12,
            "Ready=False DoesNotExist · Issuer \"kyverno-ca\" not found",
        ),
        ("monitoring", "grafana-tls", 23, "Ready=True · Issuing=True"),
        ("payments", "internal-api", 41, "Ready=True"),
        ("platform", "registry-tls", 67, "Ready=True"),
        ("observability", "otel-collector", 84, "Ready=True"),
    ]
    .into_iter()
    .map(|(namespace, name, days, detail)| Expiring {
        label: format!("Certificate {namespace}/{name}"),
        detail: detail.into(),
        days: Some(days),
        ready: detail.starts_with("Ready=True"),
        target: Some(ResourceRef {
            cluster: ClusterId::new(DEMO_CLUSTER),
            kind: "Certificate.cert-manager.io".into(),
            namespace: Some(namespace.into()),
            name: name.into(),
        }),
    })
    .collect()
}

fn unready_services() -> Vec<Problem> {
    let service = Problem {
        status: Status::Warn,
        target: ResourceRef {
            cluster: ClusterId::new(DEMO_CLUSTER),
            kind: "Service".into(),
            namespace: Some("payments".into()),
            name: "checkout-api".into(),
        },
        reason: "EndpointsNotReady".into(),
        message: "1 of 3 endpoints not ready".into(),
        condition: String::new(),
        age: Duration::from_secs(3 * HOUR),
        events: Vec::new(),
    };
    vec![service]
}

fn change(
    (age, at): (Duration, &str),
    kind: ChangeKind,
    status: Option<Status>,
    (object, namespace, name): (&str, Option<&str>, &str),
    (what, by): (&str, &str),
) -> ClusterChange {
    ClusterChange {
        age,
        at: at.into(),
        kind,
        status,
        target: ResourceRef {
            cluster: ClusterId::new(DEMO_CLUSTER),
            kind: object.into(),
            namespace: namespace.map(Into::into),
            name: name.into(),
        },
        what: what.into(),
        by: by.into(),
    }
}

fn changes() -> Vec<ClusterChange> {
    vec![
        change(
            (minutes(3), "10:31"),
            ChangeKind::Rollout,
            None,
            ("Deployment", Some("payments"), "checkout-api"),
            (
                "deploy/checkout-api rev 52 · payments",
                "flux · apps-payments",
            ),
        ),
        change(
            (minutes(22), "10:12"),
            ChangeKind::Scale,
            None,
            ("Deployment", Some("payments"), "payments-worker"),
            ("deploy/payments-worker 2 → 4 replicas · payments", ""),
        ),
        change(
            (minutes(34), "10:00"),
            ChangeKind::Helm,
            Some(Status::Err),
            ("HelmRelease", Some("monitoring"), "kube-prometheus-stack"),
            (
                "monitoring/kube-prometheus-stack 62.6.0 → 62.7.0 · failed",
                "helm-controller",
            ),
        ),
        change(
            (minutes(53), "09:41"),
            ChangeKind::Flux,
            None,
            ("Kustomization", Some("flux-system"), "platform-registry"),
            (
                "ks/platform-registry → main@a91c3f2",
                "kustomize-controller",
            ),
        ),
        change(
            (minutes(57), "09:37"),
            ChangeKind::Node,
            Some(Status::Warn),
            ("Node", None, "ip-10-4-20-19"),
            ("ip-10-4-20-19 cordoned", "node-controller"),
        ),
        change(
            (minutes(58), "09:36"),
            ChangeKind::Namespace,
            None,
            ("Namespace", None, "search"),
            ("search created", ""),
        ),
        change(
            (hours(3), "07:34"),
            ChangeKind::Node,
            None,
            ("Node", None, "ip-10-4-14-201"),
            ("ip-10-4-14-201 joined · m6i.4xlarge", ""),
        ),
        change(
            (days(2), "Oct 18"),
            ChangeKind::Crd,
            None,
            (
                "CustomResourceDefinition",
                None,
                "certificates.cert-manager.io",
            ),
            ("certificates.cert-manager.io created", "helm"),
        ),
    ]
}

fn utilization() -> Vec<Utilization> {
    let series = [
        (
            TimeRange::Hour,
            vec![
                38.9, 42.2, 45.6, 43.9, 48.9, 52.2, 50.0, 55.6, 57.8, 54.4, 58.9, 61.1, 60.0,
            ],
            vec![
                52.2, 52.8, 53.3, 53.3, 54.4, 55.0, 55.6, 55.6, 56.7, 56.7, 57.2, 57.8, 57.8,
            ],
        ),
        (
            TimeRange::SixHours,
            vec![
                44.0, 41.0, 39.0, 42.0, 46.0, 43.0, 40.0, 38.9, 45.6, 52.2, 55.6, 58.9, 60.0,
            ],
            vec![
                55.0, 54.6, 54.0, 53.2, 52.8, 52.5, 52.2, 52.2, 53.3, 55.0, 56.7, 57.2, 57.8,
            ],
        ),
        (
            TimeRange::Day,
            vec![
                52.0, 48.0, 41.0, 33.0, 29.0, 27.0, 31.0, 40.0, 49.0, 55.0, 58.0, 57.0, 60.0,
            ],
            vec![
                58.0, 57.0, 55.0, 53.0, 51.0, 50.0, 50.0, 51.0, 53.0, 55.0, 57.0, 57.5, 57.8,
            ],
        ),
        (
            TimeRange::Week,
            vec![
                55.0, 47.0, 58.0, 46.0, 57.0, 45.0, 59.0, 44.0, 56.0, 43.0, 54.0, 50.0, 60.0,
            ],
            vec![
                54.0, 55.0, 55.5, 56.0, 55.0, 54.5, 56.0, 57.0, 56.0, 55.5, 57.0, 57.5, 57.8,
            ],
        ),
    ];
    series
        .into_iter()
        .map(|(range, cpu, memory)| Utilization {
            range,
            cpu,
            memory,
            ..Utilization::default()
        })
        .collect()
}

struct NodeSeed {
    name: &'static str,
    zone: &'static str,
    gpu: bool,
    cpu: f32,
    memory: f32,
    age_days: u64,
    requests: (f32, f32),
}

const NODES: [NodeSeed; 14] = [
    NodeSeed {
        name: "ip-10-4-11-34",
        zone: "us-east-1a",
        gpu: false,
        cpu: 5.2,
        memory: 23.,
        age_days: 34,
        requests: (0.74, 0.66),
    },
    NodeSeed {
        name: "ip-10-4-12-87",
        zone: "us-east-1a",
        gpu: false,
        cpu: 6.1,
        memory: 26.,
        age_days: 34,
        requests: (0.94, 0.81),
    },
    NodeSeed {
        name: "ip-10-4-13-150",
        zone: "us-east-1a",
        gpu: false,
        cpu: 4.8,
        memory: 21.,
        age_days: 34,
        requests: (0.7, 0.62),
    },
    NodeSeed {
        name: "ip-10-4-14-201",
        zone: "us-east-1a",
        gpu: false,
        cpu: 5.9,
        memory: 25.,
        age_days: 34,
        requests: (0.86, 0.7),
    },
    NodeSeed {
        name: "ip-10-4-20-19",
        zone: "us-east-1b",
        gpu: false,
        cpu: 4.4,
        memory: 20.,
        age_days: 34,
        requests: (0.68, 0.6),
    },
    NodeSeed {
        name: "ip-10-4-21-212",
        zone: "us-east-1b",
        gpu: false,
        cpu: 5.0,
        memory: 22.,
        age_days: 34,
        requests: (0.72, 0.64),
    },
    NodeSeed {
        name: "ip-10-4-22-63",
        zone: "us-east-1b",
        gpu: false,
        cpu: 5.6,
        memory: 24.,
        age_days: 34,
        requests: (0.83, 0.66),
    },
    NodeSeed {
        name: "ip-10-4-23-8",
        zone: "us-east-1b",
        gpu: false,
        cpu: 4.9,
        memory: 21.,
        age_days: 12,
        requests: (0.66, 0.58),
    },
    NodeSeed {
        name: "ip-10-4-30-5",
        zone: "us-east-1c",
        gpu: false,
        cpu: 6.3,
        memory: 28.,
        age_days: 34,
        requests: (0.97, 0.77),
    },
    NodeSeed {
        name: "ip-10-4-31-12",
        zone: "us-east-1c",
        gpu: false,
        cpu: 1.2,
        memory: 8.,
        age_days: 34,
        requests: (0.22, 0.18),
    },
    NodeSeed {
        name: "ip-10-4-32-77",
        zone: "us-east-1c",
        gpu: false,
        cpu: 5.4,
        memory: 23.,
        age_days: 34,
        requests: (0.75, 0.67),
    },
    NodeSeed {
        name: "ip-10-4-33-140",
        zone: "us-east-1c",
        gpu: false,
        cpu: 4.7,
        memory: 21.,
        age_days: 12,
        requests: (0.69, 0.61),
    },
    NodeSeed {
        name: "ip-10-4-40-7",
        zone: "us-east-1a",
        gpu: true,
        cpu: 4.5,
        memory: 25.,
        age_days: 9,
        requests: (0.96, 0.71),
    },
    NodeSeed {
        name: "ip-10-4-41-66",
        zone: "us-east-1b",
        gpu: true,
        cpu: 4.4,
        memory: 25.,
        age_days: 9,
        requests: (0.96, 0.71),
    },
];

const NODE_DISK: f32 = 50.;
const DISK_BASE: f32 = 12.;
const DISK_PER_POD: f32 = 0.85;
const DISK_UNAVAILABLE: &str = "ip-10-4-41-66";
pub(crate) const CORDONED: &str = "ip-10-4-31-12";

pub fn schedulable_node_names() -> Vec<String> {
    NODES
        .iter()
        .filter(|seed| seed.name != CORDONED)
        .map(|seed| seed.name.to_owned())
        .collect()
}

pub fn nodes(pods: &[Pod]) -> Vec<Node> {
    NODES.iter().map(|seed| node(seed, pods)).collect()
}

fn node_pods<'a>(pods: &'a [Pod], name: &str) -> Vec<&'a Pod> {
    pods.iter()
        .filter(|pod| pod.node.as_deref() == Some(name))
        .collect()
}

#[allow(clippy::cast_precision_loss)]
fn scaled(value: u32, per_unit: f32) -> f32 {
    value as f32 / per_unit
}

fn top_pods(on_node: &[&Pod]) -> Vec<NodePod> {
    let mut top: Vec<NodePod> = on_node
        .iter()
        .map(|pod| {
            let usage = pod.usage.unwrap_or_default();
            NodePod {
                name: pod.name.clone(),
                namespace: pod.namespace.clone(),
                cpu: scaled(usage.cpu_millis, MILLIS_PER_CORE),
                memory: scaled(usage.memory_mib, MIB_PER_GIB),
            }
        })
        .collect();
    top.sort_by(|a, b| {
        b.cpu
            .total_cmp(&a.cpu)
            .then(b.memory.total_cmp(&a.memory))
            .then_with(|| a.name.cmp(&b.name))
    });
    top.truncate(TOP_PODS);
    top
}

pub(crate) fn node_load(pods: &[Pod], name: &str) -> (u32, Vec<NodePod>) {
    let on_node = node_pods(pods, name);
    let count = u32::try_from(on_node.len()).unwrap_or(u32::MAX);
    (count, top_pods(&on_node))
}

fn node(seed: &NodeSeed, pods: &[Pod]) -> Node {
    let (pod_count, top_pods) = node_load(pods, seed.name);
    let schedulable = seed.name != CORDONED;
    let (instance, memory, group, kubelet) = if seed.gpu {
        (GPU, GPU_MEMORY, "gpu", GPU_KUBELET)
    } else {
        (GENERAL, GENERAL_MEMORY, "general", KUBELET)
    };
    let (cpu_request, memory_request) = seed.requests;
    let cpu_commitment = Commitment {
        requests: cpu_request,
        limits: cpu_request * CPU_LIMIT_RATIO,
    };
    let memory_commitment = Commitment {
        requests: memory_request,
        limits: memory_request * MEMORY_LIMIT_RATIO,
    };
    let mut taints = Vec::new();
    if seed.gpu {
        taints.push(GPU_TAINT.to_owned());
    }
    let status = if schedulable {
        Status::Ok
    } else {
        taints.push(UNSCHEDULABLE_TAINT.to_owned());
        Status::Warn
    };
    let disk = (seed.name != DISK_UNAVAILABLE).then(|| Usage {
        used: DISK_BASE + f32::from(u16::try_from(pod_count).unwrap_or_default()) * DISK_PER_POD,
        allocatable: NODE_DISK,
    });
    let internal_ip = seed.name.trim_start_matches("ip-").replace('-', ".");
    Node {
        uid: Uid::default(),
        name: seed.name.into(),
        status,
        state: state_label(true, schedulable),
        schedulable,
        instance_type: instance.into(),
        zone: seed.zone.into(),
        version: kubelet.into(),
        internal_ip,
        os_image: OS_IMAGE.into(),
        runtime: RUNTIME.into(),
        cpu: Usage {
            used: seed.cpu,
            allocatable: NODE_CPU,
        },
        memory: Usage {
            used: seed.memory,
            allocatable: memory,
        },
        disk,
        cpu_commitment,
        memory_commitment,
        pods: pod_count,
        pod_capacity: POD_CAPACITY,
        age: days(seed.age_days),
        conditions: conditions(),
        taints,
        labels: vec![
            format!("node.kubernetes.io/instance-type={instance}"),
            format!("topology.kubernetes.io/zone={}", seed.zone),
            format!("eks.amazonaws.com/nodegroup={group}"),
        ],
        top_pods,
    }
}

fn conditions() -> Vec<Condition> {
    let calm = |kind: &str, message: &str| Condition::new(kind, "False").message(message);
    vec![
        Condition::new(READY_CONDITION, CONDITION_TRUE)
            .reason("KubeletReady")
            .message("kubelet is posting ready status"),
        calm("MemoryPressure", "kubelet has sufficient memory available"),
        calm("DiskPressure", "kubelet has no disk pressure"),
        calm("PIDPressure", "kubelet has sufficient PID available"),
    ]
}

struct NamespaceSeed {
    name: &'static str,
    pods: u32,
    problems: Counts,
    cpu: f32,
    memory: f32,
    requests: (f32, f32),
    restarts: u32,
    age_days: u64,
}

const NONE: Counts = Counts {
    err: 0,
    warn: 0,
    sus: 0,
    total: None,
};

const NAMESPACES: [NamespaceSeed; 23] = [
    NamespaceSeed {
        name: "analytics",
        pods: 30,
        problems: NONE,
        cpu: 4.1,
        memory: 22.5,
        requests: (9.5, 40.0),
        restarts: 1,
        age_days: 214,
    },
    NamespaceSeed {
        name: "auth",
        pods: 12,
        problems: NONE,
        cpu: 1.2,
        memory: 6.4,
        requests: (2.0, 8.0),
        restarts: 0,
        age_days: 214,
    },
    NamespaceSeed {
        name: "cert-manager",
        pods: 3,
        problems: NONE,
        cpu: 0.1,
        memory: 0.4,
        requests: (0.2, 0.5),
        restarts: 0,
        age_days: 214,
    },
    NamespaceSeed {
        name: "data-pipeline",
        pods: 30,
        problems: NONE,
        cpu: 9.8,
        memory: 61.0,
        requests: (14.0, 72.0),
        restarts: 2,
        age_days: 180,
    },
    NamespaceSeed {
        name: "default",
        pods: 0,
        problems: NONE,
        cpu: 0.,
        memory: 0.,
        requests: (0.0, 0.0),
        restarts: 0,
        age_days: 214,
    },
    NamespaceSeed {
        name: "envoy-gateway-system",
        pods: 8,
        problems: NONE,
        cpu: 0.9,
        memory: 2.1,
        requests: (1.5, 3.0),
        restarts: 0,
        age_days: 61,
    },
    NamespaceSeed {
        name: "feature-flags",
        pods: 4,
        problems: NONE,
        cpu: 0.2,
        memory: 0.6,
        requests: (0.5, 1.0),
        restarts: 0,
        age_days: 97,
    },
    NamespaceSeed {
        name: "flux-system",
        pods: 6,
        problems: Counts { err: 1, ..NONE },
        cpu: 0.4,
        memory: 1.3,
        requests: (0.8, 2.0),
        restarts: 0,
        age_days: 214,
    },
    NamespaceSeed {
        name: "ingress-nginx",
        pods: 6,
        problems: NONE,
        cpu: 1.1,
        memory: 1.8,
        requests: (1.5, 2.0),
        restarts: 1,
        age_days: 214,
    },
    NamespaceSeed {
        name: "inventory",
        pods: 20,
        problems: NONE,
        cpu: 3.2,
        memory: 14.0,
        requests: (4.0, 16.0),
        restarts: 0,
        age_days: 214,
    },
    NamespaceSeed {
        name: "kube-node-lease",
        pods: 0,
        problems: NONE,
        cpu: 0.,
        memory: 0.,
        requests: (0.0, 0.0),
        restarts: 0,
        age_days: 214,
    },
    NamespaceSeed {
        name: "kube-public",
        pods: 0,
        problems: NONE,
        cpu: 0.,
        memory: 0.,
        requests: (0.0, 0.0),
        restarts: 0,
        age_days: 214,
    },
    NamespaceSeed {
        name: "kube-system",
        pods: 80,
        problems: NONE,
        cpu: 5.6,
        memory: 18.2,
        requests: (6.0, 16.0),
        restarts: 3,
        age_days: 214,
    },
    NamespaceSeed {
        name: "logging",
        pods: 18,
        problems: NONE,
        cpu: 2.7,
        memory: 19.5,
        requests: (3.0, 24.0),
        restarts: 0,
        age_days: 214,
    },
    NamespaceSeed {
        name: "loco-builds",
        pods: 2,
        problems: NONE,
        cpu: 1.6,
        memory: 3.0,
        requests: (2.0, 4.0),
        restarts: 0,
        age_days: 61,
    },
    NamespaceSeed {
        name: "loco-system",
        pods: 9,
        problems: NONE,
        cpu: 0.6,
        memory: 1.9,
        requests: (1.0, 2.0),
        restarts: 0,
        age_days: 61,
    },
    NamespaceSeed {
        name: "ml",
        pods: 9,
        problems: Counts { warn: 1, ..NONE },
        cpu: 8.2,
        memory: 46.0,
        requests: (7.0, 52.0),
        restarts: 9,
        age_days: 97,
    },
    NamespaceSeed {
        name: "monitoring",
        pods: 41,
        problems: Counts { warn: 1, ..NONE },
        cpu: 6.4,
        memory: 38.7,
        requests: (6.0, 36.0),
        restarts: 2,
        age_days: 214,
    },
    NamespaceSeed {
        name: "notifications",
        pods: 16,
        problems: NONE,
        cpu: 1.5,
        memory: 7.2,
        requests: (2.0, 8.0),
        restarts: 0,
        age_days: 214,
    },
    NamespaceSeed {
        name: "orders",
        pods: 34,
        problems: NONE,
        cpu: 5.3,
        memory: 21.0,
        requests: (6.0, 24.0),
        restarts: 1,
        age_days: 214,
    },
    NamespaceSeed {
        name: "payments",
        pods: 22,
        problems: Counts { err: 1, ..NONE },
        cpu: 3.8,
        memory: 15.6,
        requests: (9.5, 20.0),
        restarts: 15,
        age_days: 214,
    },
    NamespaceSeed {
        name: "search",
        pods: 14,
        problems: Counts { warn: 1, ..NONE },
        cpu: 4.6,
        memory: 30.2,
        requests: (3.2, 28.0),
        restarts: 96,
        age_days: 214,
    },
    NamespaceSeed {
        name: "web",
        pods: 48,
        problems: NONE,
        cpu: 4.9,
        memory: 20.1,
        requests: (12.0, 32.0),
        restarts: 0,
        age_days: 214,
    },
];

fn namespaces() -> Vec<Namespace> {
    NAMESPACES
        .iter()
        .map(|seed| Namespace {
            labels: BTreeMap::new(),
            name: seed.name.into(),
            status: seed.problems.status(),
            phase: "Active".into(),
            pods: seed.pods,
            problems: seed.problems,
            cpu: Some(seed.cpu),
            memory: Some(seed.memory),
            cpu_requests: seed.requests.0,
            memory_requests: seed.requests.1,
            restarts: seed.restarts,
            age: days(seed.age_days),
        })
        .collect()
}

struct EventSeed {
    status: Status,
    reason: &'static str,
    object: &'static str,
    namespace: Option<&'static str>,
    message: &'static str,
    age: Duration,
    count: u32,
}

#[allow(clippy::too_many_lines)]
fn events() -> Vec<Event> {
    let seeds = [
        EventSeed {
            status: Status::Err,
            reason: "BackOff",
            object: "pod/checkout-api-7f9c6d-x2lqp",
            namespace: Some("payments"),
            message: "Back-off restarting failed container api",
            age: minutes(2),
            count: 14,
        },
        EventSeed {
            status: Status::Warn,
            reason: "FailedScheduling",
            object: "pod/ml-batch-5c7f-9kq2d",
            namespace: Some("ml"),
            message: "0/14 nodes are available: 14 Insufficient nvidia.com/gpu",
            age: minutes(6),
            count: 22,
        },
        EventSeed {
            status: Status::Warn,
            reason: "Unhealthy",
            object: "pod/search-indexer-0",
            namespace: Some("search"),
            message: "Readiness probe failed: HTTP probe failed with statuscode: 503",
            age: minutes(11),
            count: 5,
        },
        EventSeed {
            status: Status::Warn,
            reason: "FailedMount",
            object: "pod/grafana-6b8d9-tw4mn",
            namespace: Some("monitoring"),
            message: "MountVolume.SetUp failed for volume \"dashboards\": configmap not found",
            age: minutes(18),
            count: 3,
        },
        EventSeed {
            status: Status::Warn,
            reason: "NodeNotSchedulable",
            object: "node/ip-10-4-31-12",
            namespace: None,
            message: "Node ip-10-4-31-12 status is now: NodeNotSchedulable",
            age: minutes(42),
            count: 1,
        },
        EventSeed {
            status: Status::Warn,
            reason: "FailedGetResourceMetric",
            object: "horizontalpodautoscaler/web-frontend",
            namespace: Some("web"),
            message: "failed to get cpu utilization: unable to get metrics for resource cpu: no metrics returned from resource metrics API",
            age: hours(1),
            count: 4,
        },
        EventSeed {
            status: Status::Err,
            reason: "ProgressDeadlineExceeded",
            object: "deployment/checkout-api",
            namespace: Some("payments"),
            message: "ReplicaSet \"checkout-api-7f9c6d\" has timed out progressing",
            age: hours(3),
            count: 1,
        },
        EventSeed {
            status: Status::Ok,
            reason: "SuccessfulCreate",
            object: "job/backfill-orders-28770412",
            namespace: Some("data-pipeline"),
            message: "Created pod: backfill-orders-28770412-q7x2m",
            age: hours(2),
            count: 1,
        },
        EventSeed {
            status: Status::Ok,
            reason: "ReconciliationSucceeded",
            object: "kustomization/infra-controllers",
            namespace: Some("flux-system"),
            message: "Reconciliation finished in 1.2s, next run in 10m0s",
            age: minutes(3),
            count: 18,
        },
        EventSeed {
            status: Status::Ok,
            reason: "SuccessfulCreate",
            object: "job/ml-batch-5c7f",
            namespace: Some("ml"),
            message: "Created pod: ml-batch-5c7f-9kq2d",
            age: minutes(6),
            count: 1,
        },
        EventSeed {
            status: Status::Ok,
            reason: "Scheduled",
            object: "pod/web-frontend-84d6f-2xk9p",
            namespace: Some("web"),
            message: "Successfully assigned web/web-frontend-84d6f-2xk9p to ip-10-4-22-63",
            age: minutes(21),
            count: 1,
        },
        EventSeed {
            status: Status::Ok,
            reason: "Pulled",
            object: "pod/web-frontend-84d6f-2xk9p",
            namespace: Some("web"),
            message: "Successfully pulled image \"ghcr.io/acme/web-frontend:5.2.1\" in 2.1s",
            age: minutes(21),
            count: 1,
        },
        EventSeed {
            status: Status::Ok,
            reason: "ScalingReplicaSet",
            object: "deployment/checkout-api",
            namespace: Some("payments"),
            message: "Scaled up replica set checkout-api-7f9c6d to 3 from 2",
            age: hours(3),
            count: 1,
        },
        EventSeed {
            status: Status::Ok,
            reason: "ScalingReplicaSet",
            object: "deployment/checkout-api",
            namespace: Some("payments"),
            message: "Scaled down replica set checkout-api-5b8f2a to 0 from 1",
            age: hours(3),
            count: 1,
        },
    ];
    seeds
        .into_iter()
        .map(|seed| Event {
            labels: BTreeMap::new(),
            details: event_details(seed.object, seed.status, seed.age),
            status: seed.status,
            reason: seed.reason.into(),
            object: seed.object.into(),
            namespace: seed.namespace.map(Into::into),
            message: seed.message.into(),
            age: seed.age,
            count: seed.count,
        })
        .collect()
}

pub(crate) fn event_details(
    object: &str,
    status: Status,
    age: Duration,
) -> flock_core::events::EventDetails {
    let short_kind = object
        .split_once('/')
        .map(|(kind, _)| kind)
        .unwrap_or_default();
    let kind = flock_core::resource::KINDS
        .iter()
        .find(|info| info.kind.eq_ignore_ascii_case(short_kind))
        .map(|info| info.kind.to_owned())
        .unwrap_or_default();
    let first_age = EVENT_FIRST_AGE.max(age);
    let first_seen = EVENT_NOW
        .checked_sub(first_age)
        .ok()
        .map(|time| time.to_string());
    let last_seen = EVENT_NOW.checked_sub(age).ok().map(|time| time.to_string());
    flock_core::events::EventDetails {
        kind,
        event_type: if status == Status::Ok {
            "Normal".into()
        } else {
            "Warning".into()
        },
        first_seen,
        last_seen,
        first_age: Some(first_age),
        source_component: "kubelet".into(),
        source_host: "ip-10-0-11-82.ec2.internal".into(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flock_core::overview::percent;

    #[test]
    fn event_detail_times_agree_with_relative_ages() {
        let details = event_details("pod/checkout", Status::Err, minutes(2));
        assert_eq!(details.kind, "Pod");
        assert_eq!(details.event_type, "Warning");
        assert_eq!(details.first_seen.as_deref(), Some("2026-10-13T05:36:00Z"));
        assert_eq!(details.last_seen.as_deref(), Some("2026-10-13T06:34:00Z"));
        let older = event_details("deployment/checkout", Status::Ok, hours(2));
        assert_eq!(older.first_age, Some(hours(2)));
        assert_eq!(older.first_seen, older.last_seen);
    }

    #[test]
    fn the_cordoned_node_still_runs_a_few_demo_pods() {
        let pods = crate::pods::pods();
        let nodes = nodes(&pods);
        let cordoned = nodes.iter().find(|node| node.name == CORDONED);
        assert!(cordoned.is_some_and(|node| node.pods > 0 && !node.schedulable));
        assert!(cordoned.is_some_and(|node| !node.top_pods.is_empty()));
    }

    #[test]
    fn the_overview_pod_phases_are_counted_from_the_demo_pods() {
        let pods = crate::pods::pods();
        let phases = pod_phases(&pods);
        assert_eq!(phases.total() as usize, pods.len());
        assert_eq!(
            phases.failing as usize,
            pods.iter().filter(|p| failing(p)).count()
        );
    }

    #[test]
    fn every_node_pod_count_is_the_number_of_demo_pods_on_it() {
        let pods = crate::pods::pods();
        let nodes = nodes(&pods);
        assert!(nodes.iter().any(|node| node.pods > 0));
        for node in &nodes {
            let on_node = pods
                .iter()
                .filter(|pod| pod.node.as_deref() == Some(node.name.as_str()))
                .count();
            assert_eq!(node.pods as usize, on_node, "{}", node.name);
            assert!(node.top_pods.len() <= TOP_PODS);
            for top in &node.top_pods {
                assert!(
                    pods.iter()
                        .any(|pod| pod.node.as_deref() == Some(node.name.as_str())
                            && pod.name == top.name
                            && pod.namespace == top.namespace),
                    "{} on {}",
                    top.name,
                    node.name
                );
            }
        }
    }

    #[test]
    fn prod_us_east_matches_design_overview() {
        let detail = prod_us_east(&crate::pods::pods());
        assert_eq!(detail.cpu().amount().labelled(), "68.40 / 112.00 cores");
        assert_eq!(percent(detail.cpu().fraction()), "61%");
        assert_eq!(detail.memory().amount().labelled(), "312.00 / 512.00 GiB");
        assert_eq!(percent(detail.memory().fraction()), "61%");
        let nodes = detail.node_summary();
        assert_eq!((nodes.total, nodes.ready, nodes.cordoned), (14, 13, 1));
        assert_eq!(nodes.pod_capacity, 1540);
        assert_eq!(detail.overview.pods.total(), 412);
        let namespace_pods: u32 = detail.namespaces.iter().map(|n| n.pods).sum();
        assert_eq!(namespace_pods, 412);
        assert_eq!(detail.namespaces.len(), 23);
        assert_eq!(detail.warnings(), 7);
        assert_eq!(detail.nodes[9].name, CORDONED);
    }
}
