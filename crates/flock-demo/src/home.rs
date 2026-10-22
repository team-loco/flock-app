use std::time::Duration;

use flock_core::home::{ClusterMetrics, Problem, Revision};
use flock_core::pods::PodEvent;
use flock_core::{ClusterId, Counts, FlockHome, ResourceRef, Status};

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;
const HEAD: &str = "a41e07c";
const HELD_AT: &str = "7c2d9e0";
const ADDED_CPU: u8 = 27;
const ADDED_MEMORY: u8 = 38;
const ADDED_KUSTOMIZATIONS: u32 = 8;

struct MetricsSeed {
    pods: Option<Counts>,
    cpu: u8,
    memory: u8,
    flux: Counts,
    revision: &'static str,
    held: bool,
}

pub fn metrics(cluster: &str) -> Option<ClusterMetrics> {
    let seed = match cluster {
        "prod-us-east" => MetricsSeed {
            pods: None,
            cpu: 61,
            memory: 61,
            flux: Counts::total(21).with_err(1),
            revision: HEAD,
            held: false,
        },
        "prod-eu-west" => MetricsSeed {
            pods: Some(Counts::total(356)),
            cpu: 48,
            memory: 55,
            flux: Counts::total(21).with_sus(1),
            revision: HELD_AT,
            held: true,
        },
        "staging" => MetricsSeed {
            pods: Some(Counts::total(188).with_warn(2)),
            cpu: 35,
            memory: 40,
            flux: Counts::total(21).with_warn(1),
            revision: HEAD,
            held: false,
        },
        "dev-kind" => MetricsSeed {
            pods: Some(Counts::total(64)),
            cpu: 12,
            memory: 22,
            flux: Counts::total(12),
            revision: HEAD,
            held: false,
        },
        _ => return None,
    };
    Some(ClusterMetrics {
        pods: seed.pods.unwrap_or_default(),
        cpu_percent: Some(seed.cpu),
        memory_percent: Some(seed.memory),
        flux: seed.flux,
        revision: Revision {
            sha: seed.revision.into(),
            held: seed.held,
        },
    })
}

pub fn added_metrics(pods: u32) -> ClusterMetrics {
    ClusterMetrics {
        pods: Counts::total(pods),
        cpu_percent: Some(ADDED_CPU),
        memory_percent: Some(ADDED_MEMORY),
        flux: Counts::total(ADDED_KUSTOMIZATIONS),
        revision: Revision {
            sha: HEAD.into(),
            held: false,
        },
    }
}

pub fn flock_home() -> FlockHome {
    FlockHome {
        problems: problems(),
    }
}

fn target(cluster: &str, kind: &str, object: &str) -> ResourceRef {
    let (namespace, name) = match object.split_once('/') {
        Some((namespace, name)) => (Some(namespace.into()), name.into()),
        None => (None, object.into()),
    };
    ResourceRef {
        cluster: ClusterId::new(cluster),
        kind: kind.into(),
        namespace,
        name,
    }
}

fn problem(
    cluster: &str,
    status: Status,
    (kind, object): (&str, &str),
    (reason, message, condition): (&str, &str, &str),
    age_secs: u64,
) -> Problem {
    Problem {
        status,
        target: target(cluster, kind, object),
        reason: reason.into(),
        message: message.into(),
        condition: condition.into(),
        age: Duration::from_secs(age_secs),
        events: Vec::new(),
    }
}

fn event(status: Status, reason: &str, message: &str, age_secs: u64) -> PodEvent {
    PodEvent {
        status,
        reason: reason.into(),
        message: message.into(),
        count: 1,
        age: Duration::from_secs(age_secs),
    }
}

fn payments_kustomization() -> Problem {
    let health_check = "Health check failed after 5m0s: timeout waiting for: [Deployment/payments/checkout-api status: 'InProgress']";
    let mut kustomization = problem(
        "prod-us-east",
        Status::Err,
        ("Kustomization", "flux-system/apps-payments"),
        ("HealthCheckFailed", health_check, "Ready=False"),
        4 * MINUTE,
    );
    kustomization.events = vec![
        event(Status::Err, "HealthCheckFailed", health_check, 4 * MINUTE),
        event(
            Status::Ok,
            "ReconciliationSucceeded",
            "Reconciliation finished in 1.2s, next run in 10m0s",
            3 * HOUR,
        ),
    ];
    kustomization
}

fn checkout_deployment() -> Problem {
    let mut deployment = problem(
        "prod-us-east",
        Status::Err,
        ("Deployment", "payments/checkout-api"),
        (
            "ProgressDeadlineExceeded",
            "ReplicaSet \"checkout-api-7f9c6d\" has timed out progressing.",
            "Progressing=False",
        ),
        3 * HOUR,
    );
    deployment.events = vec![
        event(
            Status::Err,
            "BackOff",
            "Back-off restarting failed container api in pod checkout-api-7f9c6d-x2lqp",
            2 * MINUTE,
        ),
        event(
            Status::Ok,
            "ScalingReplicaSet",
            "Scaled up replica set checkout-api-7f9c6d to 3 from 2",
            3 * HOUR,
        ),
    ];
    deployment
}

fn problems() -> Vec<Problem> {
    vec![
        payments_kustomization(),
        checkout_deployment(),
        problem(
            "prod-us-east",
            Status::Warn,
            ("Pod", "ml/ml-batch-5c7f-9kq2d"),
            (
                "Unschedulable",
                "0/14 nodes are available: 4 Insufficient nvidia.com/gpu, 10 node(s) didn't match Pod's node affinity/selector.",
                "PodScheduled=False",
            ),
            6 * MINUTE,
        ),
        problem(
            "prod-us-east",
            Status::Warn,
            ("Pod", "monitoring/grafana-6b8d9-tw4mn"),
            (
                "FailedMount",
                "MountVolume.SetUp failed for volume \"dashboards\" : configmap \"dashboards\" not found",
                "ContainersReady=False",
            ),
            18 * MINUTE,
        ),
        problem(
            "prod-us-east",
            Status::Warn,
            ("Node", "ip-10-4-31-12"),
            ("SchedulingDisabled", "spec.unschedulable: true", ""),
            42 * MINUTE,
        ),
        problem(
            "prod-eu-west",
            Status::Suspended,
            ("Kustomization", "flux-system/apps-payments"),
            ("Suspended", "spec.suspend: true", ""),
            2 * DAY,
        ),
        problem(
            "prod-eu-west",
            Status::Warn,
            ("HorizontalPodAutoscaler", "web/web-frontend"),
            (
                "TooManyReplicas",
                "the desired replica count is more than the maximum replica count",
                "ScalingLimited=True",
            ),
            25 * MINUTE,
        ),
        problem(
            "staging",
            Status::Warn,
            ("HelmRelease", "monitoring/kube-prometheus-stack"),
            (
                "UpgradeFailed",
                "Helm upgrade failed for release monitoring/kube-prometheus-stack with chart kube-prometheus-stack@62.7.0: context deadline exceeded",
                "Ready=False",
            ),
            9 * MINUTE,
        ),
        problem(
            "staging",
            Status::Warn,
            ("Pod", "search/search-indexer-0"),
            (
                "Unhealthy",
                "Readiness probe failed: HTTP probe failed with statuscode: 503",
                "Ready=False",
            ),
            11 * MINUTE,
        ),
        problem(
            "qa-gke",
            Status::Err,
            ("Cluster", "qa-gke"),
            (
                "Unauthorized",
                "getting credentials: exec: executable gke-gcloud-auth-plugin failed with exit code 1",
                "connection",
            ),
            DAY,
        ),
    ]
}
