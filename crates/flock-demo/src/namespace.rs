use flock_core::namespace::{Listing, NamespaceDetail, Policy, Quota, QuotaItem, tally};
use flock_core::{ClusterId, Snapshot, WatchState};

const PAYMENTS: &str = "payments";
const PAYMENTS_CPU_QUOTA: f32 = 16.;
const PAYMENTS_MEMORY_QUOTA: f32 = 32.;

fn quotas(namespace: &str) -> Vec<Quota> {
    if namespace != PAYMENTS {
        return Vec::new();
    }
    vec![Quota {
        name: "payments-quota".into(),
        items: vec![QuotaItem {
            resource: "pods".into(),
            used: "9".into(),
            hard: "30".into(),
        }],
    }]
}

fn policies(namespace: &str) -> Vec<Policy> {
    if namespace != PAYMENTS {
        return Vec::new();
    }
    [
        ("default-deny", "deny all ingress + egress"),
        ("allow-gateway", "ingress from ns envoy"),
        ("allow-db", "checkout-api → payments-db:5432"),
    ]
    .into_iter()
    .map(|(name, summary)| Policy {
        name: name.into(),
        summary: summary.into(),
    })
    .collect()
}

pub fn detail(snapshot: &Snapshot, cluster: &ClusterId, namespace: &str) -> NamespaceDetail {
    let pods = tally(
        snapshot
            .pods
            .iter()
            .filter(|pod| &pod.cluster == cluster && pod.namespace == namespace)
            .map(|pod| (pod.status, pod.reason.as_str())),
    );
    let payments = namespace == PAYMENTS;
    NamespaceDetail {
        name: namespace.to_owned(),
        pods,
        cpu_quota: payments.then_some(PAYMENTS_CPU_QUOTA),
        memory_quota: payments.then_some(PAYMENTS_MEMORY_QUOTA),
        quotas: Listing {
            items: quotas(namespace),
            state: WatchState::Synced,
        },
        policies: Listing {
            items: policies(namespace),
            state: WatchState::Synced,
        },
    }
}
