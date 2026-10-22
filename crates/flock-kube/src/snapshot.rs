use std::collections::BTreeMap;
use std::sync::Arc;

use flock_core::home::ClusterMetrics;
use flock_core::{ClusterId, ClusterSummary, Counts, Page, Status};
use k8s_openapi::api::core::v1::{Node, Pod};

use crate::manager::{ClusterHandle, ConnectionStatus};
use crate::provider::region;
use crate::resources::WatchKind;

const POD_FAILED: &str = "Failed";
const POD_SUCCEEDED: &str = "Succeeded";
const POD_PENDING: &str = "Pending";
const READY_CONDITION: &str = "Ready";
const CONDITION_TRUE: &str = "True";
pub const FAILING_WAIT_REASONS: [&str; 7] = [
    "CrashLoopBackOff",
    "ImagePullBackOff",
    "ErrImagePull",
    "CreateContainerConfigError",
    "CreateContainerError",
    "InvalidImageName",
    "RunContainerError",
];
const STATE_CONNECTING: &str = "Connecting";
const STATE_HEALTHY: &str = "Healthy";
const STATE_WARNINGS: &str = "Warnings";
const STATE_ERRORS: &str = "Errors";
const STATE_DEGRADED: &str = "Degraded";
const STATE_AUTH_EXPIRED: &str = "Auth expired";
const STATE_OFFLINE: &str = "Offline";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PodHealth {
    Healthy,
    Warning,
    Failing,
}

pub fn pod_health(pod: &Pod) -> PodHealth {
    let Some(status) = pod.status.as_ref() else {
        return PodHealth::Warning;
    };
    let phase = status.phase.as_deref().unwrap_or_default();
    if phase == POD_SUCCEEDED {
        return PodHealth::Healthy;
    }
    if phase == POD_FAILED {
        return PodHealth::Failing;
    }
    let waiting_failure = status
        .container_statuses
        .iter()
        .flatten()
        .chain(status.init_container_statuses.iter().flatten())
        .filter_map(|container| {
            container
                .state
                .as_ref()?
                .waiting
                .as_ref()?
                .reason
                .as_deref()
        })
        .any(|reason| FAILING_WAIT_REASONS.contains(&reason));
    if waiting_failure {
        return PodHealth::Failing;
    }
    let ready =
        status.conditions.iter().flatten().any(|condition| {
            condition.type_ == READY_CONDITION && condition.status == CONDITION_TRUE
        });
    if phase == POD_PENDING || !ready {
        return PodHealth::Warning;
    }
    PodHealth::Healthy
}

pub(crate) fn count(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

pub fn is_counted(pod: &Pod) -> bool {
    let phase = pod.status.as_ref().and_then(|s| s.phase.as_deref());
    phase != Some(POD_SUCCEEDED)
}

pub fn pod_counts(pods: &[Arc<Pod>]) -> Counts {
    let counted = || pods.iter().filter(|pod| is_counted(pod));
    let mut counts = Counts::total(count(counted().count()));
    for pod in counted() {
        match pod_health(pod) {
            PodHealth::Failing => counts.err += 1,
            PodHealth::Warning => counts.warn += 1,
            PodHealth::Healthy => {}
        }
    }
    counts
}

pub fn node_ready(node: &Node) -> bool {
    node.status
        .as_ref()
        .and_then(|status| status.conditions.as_ref())
        .into_iter()
        .flatten()
        .any(|condition| condition.type_ == READY_CONDITION && condition.status == CONDITION_TRUE)
}

pub fn schedulable(node: &Node) -> bool {
    !node
        .spec
        .as_ref()
        .and_then(|spec| spec.unschedulable)
        .unwrap_or_default()
}

pub fn node_counts(nodes: &[Arc<Node>]) -> Counts {
    let not_ready = nodes.iter().filter(|node| !node_ready(node)).count();
    Counts::total(count(nodes.len())).with_warn(count(not_ready))
}

pub fn cluster_state(status: &ConnectionStatus, problems: Counts) -> (Status, &'static str) {
    match status {
        ConnectionStatus::Connecting => (Status::Progress, STATE_CONNECTING),
        ConnectionStatus::Connected => match problems.status() {
            Status::Err => (Status::Err, STATE_ERRORS),
            Status::Warn => (Status::Warn, STATE_WARNINGS),
            _ => (Status::Ok, STATE_HEALTHY),
        },
        ConnectionStatus::Degraded(_) => (Status::Warn, STATE_DEGRADED),
        ConnectionStatus::AuthExpired(_) => (Status::Err, STATE_AUTH_EXPIRED),
        ConnectionStatus::Offline(_) => (Status::Offline, STATE_OFFLINE),
    }
}

pub fn page_kind(page: Page) -> Option<WatchKind> {
    match page {
        Page::Nodes => Some(WatchKind::Nodes),
        Page::Namespaces => Some(WatchKind::Namespaces),
        Page::Events => Some(WatchKind::Events),
        Page::Pods => Some(WatchKind::Pods),
        Page::Deployments => Some(WatchKind::Deployments),
        Page::StatefulSets => Some(WatchKind::StatefulSets),
        Page::DaemonSets => Some(WatchKind::DaemonSets),
        Page::Jobs => Some(WatchKind::Jobs),
        Page::CronJobs => Some(WatchKind::CronJobs),
        Page::Services => Some(WatchKind::Services),
        Page::Ingresses => Some(WatchKind::Ingresses),
        Page::ConfigMaps => Some(WatchKind::ConfigMaps),
        Page::Secrets => Some(WatchKind::Secrets),
        Page::Pvcs => Some(WatchKind::PersistentVolumeClaims),
        _ => None,
    }
}

pub const BACKGROUND_KINDS: [WatchKind; 16] = [
    WatchKind::Nodes,
    WatchKind::Namespaces,
    WatchKind::Events,
    WatchKind::Pods,
    WatchKind::Deployments,
    WatchKind::StatefulSets,
    WatchKind::DaemonSets,
    WatchKind::Jobs,
    WatchKind::CronJobs,
    WatchKind::Services,
    WatchKind::EndpointSlices,
    WatchKind::Ingresses,
    WatchKind::ConfigMaps,
    WatchKind::Secrets,
    WatchKind::PersistentVolumeClaims,
    WatchKind::CustomResourceDefinitions,
];

pub const OVERVIEW_KINDS: [WatchKind; 7] = [
    WatchKind::Namespaces,
    WatchKind::Events,
    WatchKind::Deployments,
    WatchKind::StatefulSets,
    WatchKind::DaemonSets,
    WatchKind::Jobs,
    WatchKind::CronJobs,
];

pub fn page_kinds(page: Page) -> Vec<WatchKind> {
    match page {
        Page::ClusterOverview => OVERVIEW_KINDS.to_vec(),
        Page::Pods => vec![WatchKind::Pods, WatchKind::Events],
        other => page_kind(other).into_iter().collect(),
    }
}

pub struct Facts<'a> {
    pub problems: Counts,
    pub metrics: Option<ClusterMetrics>,
    pub nodes: &'a [Arc<Node>],
    pub nav: BTreeMap<Page, Counts>,
}

pub fn summarize(
    id: ClusterId,
    name: String,
    handle: &ClusterHandle,
    facts: Facts<'_>,
) -> ClusterSummary {
    let Facts {
        problems,
        metrics,
        nodes,
        nav,
    } = facts;
    let context = handle.context();
    let info = handle.info();
    let connection = handle.status();
    let (status, state) = cluster_state(&connection, problems);
    let synced_nodes = !nodes.is_empty();
    ClusterSummary {
        id,
        name,
        status,
        state: state.to_owned(),
        distro: info.provider.label().to_owned(),
        region: region(
            info.provider,
            &context.id.context,
            context.server.as_deref(),
        ),
        version: info.version,
        nodes: synced_nodes.then(|| count(nodes.len())),
        problems,
        nav,
        metrics,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn every_cluster_page_reads_only_background_stores() {
        let pages = flock_core::CLUSTER_NAV
            .iter()
            .flat_map(|section| section.pages.iter().copied());
        for page in pages {
            for kind in page_kinds(page) {
                assert!(BACKGROUND_KINDS.contains(&kind), "{page:?} needs {kind:?}");
            }
        }
    }
    use crate::failure::{Failure, FailureKind};

    fn pod(status: &serde_json::Value) -> Arc<Pod> {
        let parsed = serde_json::from_value(json!({
            "metadata": { "name": "p", "namespace": "n" },
            "status": status
        }));
        let Ok(pod) = parsed else {
            panic!("pod fixture");
        };
        Arc::new(pod)
    }

    fn ready() -> serde_json::Value {
        json!({ "phase": "Running", "conditions": [{ "type": "Ready", "status": "True" }] })
    }

    #[test]
    fn classifies_pods() {
        let crash = pod(&json!({
            "phase": "Running",
            "containerStatuses": [{
                "name": "c", "image": "i", "imageID": "", "ready": false, "restartCount": 5,
                "state": { "waiting": { "reason": "CrashLoopBackOff" } }
            }]
        }));
        let pending = pod(&json!({ "phase": "Pending" }));
        let done = pod(&json!({ "phase": "Succeeded" }));
        let failed = pod(&json!({ "phase": "Failed" }));
        assert_eq!(pod_health(&crash), PodHealth::Failing);
        assert_eq!(pod_health(&pending), PodHealth::Warning);
        assert_eq!(pod_health(&done), PodHealth::Healthy);
        assert_eq!(pod_health(&pod(&ready())), PodHealth::Healthy);
        let counts = pod_counts(&[crash, pending, done, failed, pod(&ready())]);
        assert_eq!(counts, Counts::total(4).with_err(2).with_warn(1));
    }

    #[test]
    fn connection_states_match_design_words() {
        let failure = Failure {
            kind: FailureKind::ExpiredCredentials,
            detail: String::new(),
            message: String::new(),
            remedy: None,
        };
        assert_eq!(
            cluster_state(
                &ConnectionStatus::AuthExpired(failure.clone()),
                Counts::default()
            ),
            (Status::Err, "Auth expired")
        );
        assert_eq!(
            cluster_state(&ConnectionStatus::Offline(failure), Counts::default()),
            (Status::Offline, "Offline")
        );
        assert_eq!(
            cluster_state(&ConnectionStatus::Connected, Counts::default()),
            (Status::Ok, "Healthy")
        );
        assert_eq!(
            cluster_state(&ConnectionStatus::Connected, Counts::default().with_warn(2)),
            (Status::Warn, "Warnings")
        );
        assert_eq!(
            cluster_state(&ConnectionStatus::Connecting, Counts::default()),
            (Status::Progress, "Connecting")
        );
    }

    #[test]
    fn pages_round_trip_to_kinds() {
        assert_eq!(
            page_kind(Page::Pvcs),
            Some(WatchKind::PersistentVolumeClaims)
        );
        assert_eq!(page_kind(Page::GitOps), None);
        assert_eq!(
            page_kinds(Page::ClusterOverview).len(),
            OVERVIEW_KINDS.len()
        );
        assert_eq!(page_kinds(Page::Secrets), [WatchKind::Secrets]);
        assert_eq!(page_kinds(Page::Changes), []);
    }
}
