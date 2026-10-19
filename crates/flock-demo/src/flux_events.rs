use std::collections::BTreeMap;
use std::time::Duration;

use flock_core::events::{ClusterEvent, Event, EventDetails};
use flock_core::{ClusterId, Status};

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const FLUX_SYSTEM: &str = "flux-system";
const NORMAL: &str = "Normal";
const WARNING: &str = "Warning";
const KUSTOMIZE: &str = "kustomize-controller";
const HELM: &str = "helm-controller";
const SOURCE: &str = "source-controller";

struct Seed {
    cluster: &'static str,
    kind: &'static str,
    namespace: &'static str,
    name: &'static str,
    status: Status,
    reason: &'static str,
    message: &'static str,
    age: u64,
    count: u32,
    component: &'static str,
}

const SEEDS: [Seed; 9] = [
    Seed {
        cluster: "prod-us-east",
        kind: "Kustomization",
        namespace: FLUX_SYSTEM,
        name: "apps-payments",
        status: Status::Err,
        reason: "HealthCheckFailed",
        message: "health check failed after 5m0s: timeout waiting for: [Deployment/payments/checkout-api status: 'InProgress']",
        age: 2 * MINUTE,
        count: 18,
        component: KUSTOMIZE,
    },
    Seed {
        cluster: "prod-us-east",
        kind: "GitRepository",
        namespace: FLUX_SYSTEM,
        name: "fleet-infra",
        status: Status::Ok,
        reason: "NewArtifact",
        message: "stored artifact for commit 'payments: raise checkout-api memory limit'",
        age: 4 * MINUTE,
        count: 1,
        component: SOURCE,
    },
    Seed {
        cluster: "staging",
        kind: "HelmRelease",
        namespace: "monitoring",
        name: "kube-prometheus-stack",
        status: Status::Warn,
        reason: "UpgradeFailed",
        message: "Helm upgrade failed for release monitoring/kube-prometheus-stack with chart kube-prometheus-stack@62.7.0: context deadline exceeded",
        age: 6 * MINUTE,
        count: 4,
        component: HELM,
    },
    Seed {
        cluster: "prod-eu-west",
        kind: "Kustomization",
        namespace: FLUX_SYSTEM,
        name: "apps-payments",
        status: Status::Ok,
        reason: "ReconciliationSucceeded",
        message: "Reconciliation finished in 2.4s, next run in 10m0s",
        age: 9 * MINUTE,
        count: 1,
        component: KUSTOMIZE,
    },
    Seed {
        cluster: "prod-us-east",
        kind: "Kustomization",
        namespace: FLUX_SYSTEM,
        name: "infra-configs",
        status: Status::Ok,
        reason: "ReconciliationSucceeded",
        message: "Reconciliation finished in 1.1s, next run in 10m0s",
        age: 11 * MINUTE,
        count: 3,
        component: KUSTOMIZE,
    },
    Seed {
        cluster: "dev-kind",
        kind: "HelmRepository",
        namespace: FLUX_SYSTEM,
        name: "prometheus-community",
        status: Status::Ok,
        reason: "NewArtifact",
        message: "stored fetched index of size 3.1MB from 'https://prometheus-community.github.io/helm-charts'",
        age: 25 * MINUTE,
        count: 1,
        component: SOURCE,
    },
    Seed {
        cluster: "staging",
        kind: "HelmChart",
        namespace: FLUX_SYSTEM,
        name: "monitoring-kube-prometheus-stack",
        status: Status::Ok,
        reason: "ChartPullSucceeded",
        message: "pulled 'kube-prometheus-stack' chart with version '62.7.0'",
        age: 31 * MINUTE,
        count: 1,
        component: SOURCE,
    },
    Seed {
        cluster: "prod-eu-west",
        kind: "Kustomization",
        namespace: FLUX_SYSTEM,
        name: "apps-search",
        status: Status::Ok,
        reason: "Progressing",
        message: "Deployment/search/search-api configured",
        age: HOUR,
        count: 1,
        component: KUSTOMIZE,
    },
    Seed {
        cluster: "prod-us-east",
        kind: "HelmRelease",
        namespace: "monitoring",
        name: "kube-prometheus-stack",
        status: Status::Ok,
        reason: "UpgradeSucceeded",
        message: "Helm upgrade succeeded for release monitoring/kube-prometheus-stack.v9 with chart kube-prometheus-stack@62.7.0",
        age: 2 * HOUR,
        count: 1,
        component: HELM,
    },
];

pub fn events() -> Vec<ClusterEvent> {
    SEEDS
        .iter()
        .enumerate()
        .map(|(ix, seed)| {
            let age = Duration::from_secs(seed.age);
            let event_type = if seed.status == Status::Ok {
                NORMAL
            } else {
                WARNING
            };
            let details = EventDetails {
                uid: format!("flux-event-{ix}"),
                kind: seed.kind.into(),
                object_uid: None,
                object_namespace: Some(seed.namespace.into()),
                event_type: event_type.into(),
                first_seen: None,
                last_seen: None,
                first_age: Some(age),
                source_component: seed.component.into(),
                source_host: String::new(),
            };
            ClusterEvent {
                cluster: ClusterId::new(seed.cluster),
                cluster_name: seed.cluster.into(),
                event: Event {
                    labels: BTreeMap::new(),
                    details,
                    status: seed.status,
                    reason: seed.reason.into(),
                    object: format!("{}/{}", seed.kind.to_lowercase(), seed.name),
                    namespace: Some(seed.namespace.into()),
                    message: seed.message.into(),
                    age,
                    count: seed.count,
                },
            }
        })
        .collect()
}
