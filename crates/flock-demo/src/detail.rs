use std::collections::BTreeMap;
use std::time::Duration;

use flock_core::Condition;
use flock_core::cluster::Namespace;
use flock_core::events::Event;
use flock_core::nodes::{CONDITION_TRUE, Node, READY_CONDITION, Usage};
use flock_core::overview::{Commitment, NetworkFacts, Overview, PodPhases, TimeRange, Utilization};
use flock_core::pod_tops::PodTops;
use flock_core::{ClusterDetail, Counts, Status, Uid};

use crate::lists;

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;

const NODE_CPU: f32 = 8.;
const GENERAL_MEMORY: f32 = 36.;
const GPU_MEMORY: f32 = 40.;
const POD_CAPACITY: u32 = 110;
const KUBELET: &str = "v1.31.2-eks-7f9249a";
const GPU_KUBELET: &str = "v1.30.4-eks-a737599";
const GPU_COMMITMENT: (Commitment, Commitment) = (
    Commitment {
        requests: 0.96,
        limits: 1.,
    },
    Commitment {
        requests: 0.71,
        limits: 0.9,
    },
);
const OS_IMAGE: &str = "Amazon Linux 2023.5.20240916";
const EVENT_FIRST_AGE: Duration = Duration::from_secs(3600);
const EVENT_NOW: jiff::Timestamp = jiff::Timestamp::constant(1_791_873_360, 0);

const RUNTIME: &str = "containerd://1.7.20";
const GENERAL: &str = "m6i.4xlarge";
const GPU: &str = "g5.2xlarge";
const UNSCHEDULABLE_TAINT: &str = "node.kubernetes.io/unschedulable:NoSchedule";
const GPU_TAINT: &str = "nvidia.com/gpu=present:NoSchedule";
pub const SCHEDULING_DISABLED: &str = "SchedulingDisabled";
pub const READY: &str = "Ready";

pub fn minutes(n: u64) -> Duration {
    Duration::from_secs(n * MINUTE)
}

pub fn hours(n: u64) -> Duration {
    Duration::from_secs(n * HOUR)
}

pub fn days(n: u64) -> Duration {
    Duration::from_secs(n * DAY)
}

pub fn prod_us_east() -> ClusterDetail {
    let resources = lists::prod_us_east();
    ClusterDetail {
        overview: overview(),
        nodes: nodes(),
        namespaces: namespaces(),
        events: events(),
        resources,
        related: Vec::new(),
        namespace_details: Vec::new(),
    }
}

fn overview() -> Overview {
    Overview {
        utilization_source: flock_core::overview::UtilizationSource::Prometheus,
        metrics_source: "monitoring/prometheus-k8s".into(),
        cpu: Commitment {
            requests: 0.74,
            limits: 1.31,
        },
        memory: Commitment {
            requests: 0.66,
            limits: 0.92,
        },
        pods: PodPhases {
            running: 398,
            pending: 9,
            failing: 5,
        },
        utilization: utilization(),
        tops: PodTops::default(),
        network: NetworkFacts {
            services_without_endpoints: Some(2),
            load_balancers_pending: 1,
            pvcs_pending: 0,
        },
    }
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
    pods: u32,
    age_days: u64,
}

const NODES: [NodeSeed; 14] = [
    NodeSeed {
        name: "ip-10-4-11-34",
        zone: "us-east-1a",
        gpu: false,
        cpu: 5.2,
        memory: 23.,
        pods: 34,
        age_days: 34,
    },
    NodeSeed {
        name: "ip-10-4-12-87",
        zone: "us-east-1a",
        gpu: false,
        cpu: 6.1,
        memory: 26.,
        pods: 38,
        age_days: 34,
    },
    NodeSeed {
        name: "ip-10-4-13-150",
        zone: "us-east-1a",
        gpu: false,
        cpu: 4.8,
        memory: 21.,
        pods: 31,
        age_days: 34,
    },
    NodeSeed {
        name: "ip-10-4-14-201",
        zone: "us-east-1a",
        gpu: false,
        cpu: 5.9,
        memory: 25.,
        pods: 36,
        age_days: 34,
    },
    NodeSeed {
        name: "ip-10-4-20-19",
        zone: "us-east-1b",
        gpu: false,
        cpu: 4.4,
        memory: 20.,
        pods: 30,
        age_days: 34,
    },
    NodeSeed {
        name: "ip-10-4-21-212",
        zone: "us-east-1b",
        gpu: false,
        cpu: 5.0,
        memory: 22.,
        pods: 33,
        age_days: 34,
    },
    NodeSeed {
        name: "ip-10-4-22-63",
        zone: "us-east-1b",
        gpu: false,
        cpu: 5.6,
        memory: 24.,
        pods: 35,
        age_days: 34,
    },
    NodeSeed {
        name: "ip-10-4-23-8",
        zone: "us-east-1b",
        gpu: false,
        cpu: 4.9,
        memory: 21.,
        pods: 30,
        age_days: 12,
    },
    NodeSeed {
        name: "ip-10-4-30-5",
        zone: "us-east-1c",
        gpu: false,
        cpu: 6.3,
        memory: 28.,
        pods: 37,
        age_days: 34,
    },
    NodeSeed {
        name: "ip-10-4-31-12",
        zone: "us-east-1c",
        gpu: false,
        cpu: 1.2,
        memory: 8.,
        pods: 6,
        age_days: 34,
    },
    NodeSeed {
        name: "ip-10-4-32-77",
        zone: "us-east-1c",
        gpu: false,
        cpu: 5.4,
        memory: 23.,
        pods: 32,
        age_days: 34,
    },
    NodeSeed {
        name: "ip-10-4-33-140",
        zone: "us-east-1c",
        gpu: false,
        cpu: 4.7,
        memory: 21.,
        pods: 30,
        age_days: 12,
    },
    NodeSeed {
        name: "ip-10-4-40-7",
        zone: "us-east-1a",
        gpu: true,
        cpu: 4.5,
        memory: 25.,
        pods: 20,
        age_days: 9,
    },
    NodeSeed {
        name: "ip-10-4-41-66",
        zone: "us-east-1b",
        gpu: true,
        cpu: 4.4,
        memory: 25.,
        pods: 20,
        age_days: 9,
    },
];

const CORDONED: &str = "ip-10-4-31-12";

pub fn nodes() -> Vec<Node> {
    NODES.iter().map(node).collect()
}

fn node(seed: &NodeSeed) -> Node {
    let schedulable = seed.name != CORDONED;
    let (instance, memory, group, kubelet) = if seed.gpu {
        (GPU, GPU_MEMORY, "gpu", GPU_KUBELET)
    } else {
        (GENERAL, GENERAL_MEMORY, "general", KUBELET)
    };
    let (cpu_commitment, memory_commitment) = if seed.gpu {
        GPU_COMMITMENT
    } else {
        (overview().cpu, overview().memory)
    };
    let mut taints = Vec::new();
    if seed.gpu {
        taints.push(GPU_TAINT.to_owned());
    }
    let (status, state) = if schedulable {
        (Status::Ok, READY)
    } else {
        taints.push(UNSCHEDULABLE_TAINT.to_owned());
        (Status::Warn, SCHEDULING_DISABLED)
    };
    let internal_ip = seed.name.trim_start_matches("ip-").replace('-', ".");
    Node {
        uid: Uid::default(),
        name: seed.name.into(),
        status,
        state: state.into(),
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
        cpu_commitment,
        memory_commitment,
        pods: seed.pods,
        pod_capacity: POD_CAPACITY,
        age: days(seed.age_days),
        conditions: conditions(),
        taints,
        labels: vec![
            format!("node.kubernetes.io/instance-type={instance}"),
            format!("topology.kubernetes.io/zone={}", seed.zone),
            format!("eks.amazonaws.com/nodegroup={group}"),
        ],
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
    fn prod_us_east_matches_design_overview() {
        let detail = prod_us_east();
        assert_eq!(detail.cpu().amount(), "68.4 / 112.0 cores");
        assert_eq!(percent(detail.cpu().fraction()), "61%");
        assert_eq!(detail.memory().amount(), "312.0 / 512.0 GiB");
        assert_eq!(percent(detail.memory().fraction()), "61%");
        let nodes = detail.node_summary();
        assert_eq!((nodes.total, nodes.ready, nodes.cordoned), (14, 13, 1));
        assert_eq!(nodes.pod_capacity, 1540);
        assert_eq!(detail.overview.pods.total(), 412);
        let node_pods: u32 = detail.nodes.iter().map(|n| n.pods).sum();
        assert_eq!(node_pods, 412);
        let namespace_pods: u32 = detail.namespaces.iter().map(|n| n.pods).sum();
        assert_eq!(namespace_pods, 412);
        assert_eq!(detail.namespaces.len(), 23);
        assert_eq!(detail.warnings(), 7);
        assert_eq!(detail.nodes[9].name, CORDONED);
    }
}
