use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use flock_core::home::Problem;
use flock_core::{ClusterId, Diagnosis, ResourceRef, Status};
use jiff::Timestamp;
use k8s_openapi::api::core::v1::{ContainerStatus, Node, Pod, PodCondition};

use crate::clock::{age, since};
use crate::manager::ConnectionStatus;
use crate::snapshot::{FAILING_WAIT_REASONS, node_ready, schedulable};

const POD_KIND: &str = "Pod";
const NODE_KIND: &str = "Node";
const CLUSTER_KIND: &str = "Cluster";
const REPLICA_SET: &str = "ReplicaSet";
const DEPLOYMENT: &str = "Deployment";
const TEMPLATE_HASH_LABEL: &str = "pod-template-hash";
const CRASH_LOOP: &str = "CrashLoopBackOff";
const READY_CONDITION: &str = "Ready";
const SCHEDULED_CONDITION: &str = "PodScheduled";
const UNSCHEDULABLE: &str = "Unschedulable";
const CONDITION_TRUE: &str = "True";
const CONDITION_FALSE: &str = "False";
const PHASE_RUNNING: &str = "Running";
const PHASE_SUCCEEDED: &str = "Succeeded";
const PHASE_FAILED: &str = "Failed";
const NOT_READY_GRACE: Duration = Duration::from_secs(120);
const ELLIPSIS: char = '…';
const CORDONED: &str = "Cordoned (SchedulingDisabled)";
const NOT_READY: &str = "NotReady";
const SINGLE: i32 = 1;
const UNSCHEDULABLE_TAINT: &str = "node.kubernetes.io/unschedulable";

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OwnerKey {
    pub namespace: String,
    pub kind: String,
    pub name: String,
}

pub fn owner_of(pod: &Pod) -> OwnerKey {
    let namespace = pod.metadata.namespace.clone().unwrap_or_default();
    let controller = pod
        .metadata
        .owner_references
        .iter()
        .flatten()
        .find(|owner| owner.controller == Some(true));
    let Some(owner) = controller else {
        return OwnerKey {
            namespace,
            kind: POD_KIND.to_owned(),
            name: pod.metadata.name.clone().unwrap_or_default(),
        };
    };
    let hash = pod
        .metadata
        .labels
        .as_ref()
        .and_then(|labels| labels.get(TEMPLATE_HASH_LABEL));
    let deployment = (owner.kind == REPLICA_SET)
        .then_some(hash)
        .flatten()
        .and_then(|hash| owner.name.strip_suffix(hash.as_str()))
        .and_then(|prefix| prefix.strip_suffix('-'));
    match deployment {
        Some(name) => OwnerKey {
            namespace,
            kind: DEPLOYMENT.to_owned(),
            name: name.to_owned(),
        },
        None => OwnerKey {
            namespace,
            kind: owner.kind.clone(),
            name: owner.name.clone(),
        },
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Issue {
    Waiting {
        reason: String,
        message: String,
        restarts: i32,
        exit: Option<i32>,
        last: Option<String>,
    },
    Unschedulable {
        message: String,
    },
    NotReady {
        container: Option<String>,
    },
}

impl Issue {
    pub fn status(&self) -> Status {
        match self {
            Self::Waiting { .. } => Status::Err,
            Self::Unschedulable { .. } => Status::Warn,
            Self::NotReady { .. } => Status::Warn,
        }
    }

    pub fn diagnosis(&self) -> Diagnosis {
        match self {
            Self::Waiting {
                reason,
                restarts,
                last,
                ..
            } if reason == CRASH_LOOP => {
                let noun = if *restarts == SINGLE {
                    "restart"
                } else {
                    "restarts"
                };
                let message = match last {
                    Some(last) => format!("{restarts} {noun}, last {last}"),
                    None => format!("{restarts} {noun}"),
                };
                Diagnosis::new(reason.clone(), message)
            }
            Self::Waiting {
                reason, message, ..
            } => Diagnosis::new(reason.clone(), message.clone()),
            Self::Unschedulable { message } => Diagnosis::new(UNSCHEDULABLE, message.clone()),
            Self::NotReady {
                container: Some(container),
            } => Diagnosis::new(NOT_READY, format!("{container} is running but not ready")),
            Self::NotReady { container: None } => {
                Diagnosis::new(NOT_READY, "running but not ready")
            }
        }
    }

    pub fn sentence(&self) -> String {
        match self {
            Self::Waiting {
                reason,
                restarts,
                exit,
                ..
            } if reason == CRASH_LOOP => {
                let noun = if *restarts == 1 {
                    "restart"
                } else {
                    "restarts"
                };
                match exit {
                    Some(code) => format!("{reason}, {restarts} {noun}, exit {code}"),
                    None => format!("{reason}, {restarts} {noun}"),
                }
            }
            Self::Waiting {
                reason, message, ..
            } => clause(reason, message),
            Self::Unschedulable { message } => clause("FailedScheduling", message),
            Self::NotReady {
                container: Some(container),
            } => format!("{container} is running but not ready"),
            Self::NotReady { container: None } => "running but not ready".to_owned(),
        }
    }
}

fn clause(reason: &str, message: &str) -> String {
    let message = message.trim();
    if message.is_empty() {
        reason.to_owned()
    } else {
        format!("{reason}: {message}")
    }
}

pub(crate) fn last_exit(container: &ContainerStatus) -> Option<String> {
    let terminated = container.last_state.as_ref()?.terminated.as_ref()?;
    let code = terminated.exit_code;
    let reason = terminated
        .reason
        .as_deref()
        .filter(|reason| !reason.is_empty());
    Some(match reason {
        Some(reason) => format!("exit {reason} ({code})"),
        None => format!("exit {code}"),
    })
}

fn condition<'a>(pod: &'a Pod, kind: &str) -> Option<&'a PodCondition> {
    pod.status
        .as_ref()?
        .conditions
        .iter()
        .flatten()
        .find(|condition| condition.type_ == kind)
}

pub fn pod_ready(pod: &Pod) -> bool {
    condition(pod, READY_CONDITION).is_some_and(|c| c.status == CONDITION_TRUE)
}

pub fn is_active(pod: &Pod) -> bool {
    let phase = pod.status.as_ref().and_then(|s| s.phase.as_deref());
    pod.metadata.deletion_timestamp.is_none()
        && phase != Some(PHASE_SUCCEEDED)
        && phase != Some(PHASE_FAILED)
}

fn waiting_issue(pod: &Pod) -> Option<Issue> {
    let status = pod.status.as_ref()?;
    status
        .init_container_statuses
        .iter()
        .flatten()
        .chain(status.container_statuses.iter().flatten())
        .find_map(|container| {
            let waiting = container.state.as_ref()?.waiting.as_ref()?;
            let reason = waiting.reason.as_deref()?;
            if !FAILING_WAIT_REASONS.contains(&reason) {
                return None;
            }
            let exit = container
                .last_state
                .as_ref()
                .and_then(|last| last.terminated.as_ref())
                .map(|terminated| terminated.exit_code);
            Some(Issue::Waiting {
                reason: reason.to_owned(),
                message: waiting.message.clone().unwrap_or_default(),
                restarts: container.restart_count,
                exit,
                last: last_exit(container),
            })
        })
}

fn unready_container(pod: &Pod) -> Option<String> {
    pod.status
        .as_ref()?
        .container_statuses
        .iter()
        .flatten()
        .find(|container| !container.ready)
        .map(|container| container.name.clone())
}

pub fn pod_issue(pod: &Pod, now: Timestamp) -> Option<(Issue, Duration)> {
    if !is_active(pod) {
        return None;
    }
    let ready = condition(pod, READY_CONDITION);
    let not_ready_for = since(ready.and_then(|c| c.last_transition_time.as_ref()), now);
    if let Some(issue) = waiting_issue(pod) {
        let age = if ready.is_some() {
            not_ready_for
        } else {
            age(&pod.metadata, now)
        };
        return Some((issue, age));
    }
    let scheduled = condition(pod, SCHEDULED_CONDITION);
    if let Some(scheduled) = scheduled
        .filter(|c| c.status == CONDITION_FALSE && c.reason.as_deref() == Some(UNSCHEDULABLE))
    {
        let issue = Issue::Unschedulable {
            message: scheduled.message.clone().unwrap_or_default(),
        };
        return Some((issue, since(scheduled.last_transition_time.as_ref(), now)));
    }
    let running = pod.status.as_ref().and_then(|s| s.phase.as_deref()) == Some(PHASE_RUNNING);
    let unready = ready.is_some_and(|c| c.status == CONDITION_FALSE);
    if running && unready && not_ready_for >= NOT_READY_GRACE {
        let issue = Issue::NotReady {
            container: unready_container(pod),
        };
        return Some((issue, not_ready_for));
    }
    None
}

struct Flagged {
    pod: String,
    issue: Issue,
    age: Duration,
}

#[derive(Default)]
struct Group {
    total: u32,
    ready: u32,
    flagged: Vec<Flagged>,
}

fn short_pod(name: &str) -> String {
    let tail = name.rsplit('-').next().unwrap_or(name);
    format!("{ELLIPSIS}{tail}")
}

pub fn pod_problems(cluster: &ClusterId, pods: &[Arc<Pod>], now: Timestamp) -> Vec<Problem> {
    let mut groups: HashMap<OwnerKey, Group> = HashMap::new();
    for pod in pods.iter().filter(|pod| is_active(pod)) {
        let group = groups.entry(owner_of(pod)).or_default();
        group.total += 1;
        if pod_ready(pod) {
            group.ready += 1;
        }
        if let Some((issue, age)) = pod_issue(pod, now) {
            group.flagged.push(Flagged {
                pod: pod.metadata.name.clone().unwrap_or_default(),
                issue,
                age,
            });
        }
    }
    groups
        .into_iter()
        .filter_map(|(owner, group)| group_problem(cluster, owner, &group))
        .collect()
}

fn group_problem(cluster: &ClusterId, owner: OwnerKey, group: &Group) -> Option<Problem> {
    let worst = group.flagged.iter().max_by(|a, b| {
        a.issue
            .status()
            .cmp(&b.issue.status())
            .then(a.age.cmp(&b.age))
    })?;
    let mut message = if owner.kind == POD_KIND {
        worst.issue.sentence()
    } else {
        format!(
            "{}/{} ready · pod {} {}",
            group.ready,
            group.total,
            short_pod(&worst.pod),
            worst.issue.sentence()
        )
    };
    let others = group.flagged.len() - 1;
    if others > 0 {
        let _ = write!(message, " (+{others} more)");
    }
    Some(Problem {
        status: worst.issue.status(),
        target: ResourceRef {
            cluster: cluster.clone(),
            kind: owner.kind,
            namespace: Some(owner.namespace),
            name: owner.name,
        },
        message,
        age: worst.age,
    })
}

fn node_ref(cluster: &ClusterId, node: &Node) -> ResourceRef {
    ResourceRef {
        cluster: cluster.clone(),
        kind: NODE_KIND.to_owned(),
        namespace: None,
        name: node.metadata.name.clone().unwrap_or_default(),
    }
}

pub fn node_problems(cluster: &ClusterId, nodes: &[Arc<Node>], now: Timestamp) -> Vec<Problem> {
    let mut problems = Vec::new();
    for node in nodes {
        let status = node.status.as_ref();
        if !node_ready(node) {
            let ready = status
                .and_then(|s| s.conditions.as_ref())
                .into_iter()
                .flatten()
                .find(|c| c.type_ == READY_CONDITION);
            let message = ready.and_then(|c| c.message.as_deref()).unwrap_or_default();
            problems.push(Problem {
                status: Status::Err,
                target: node_ref(cluster, node),
                message: clause(NOT_READY, message),
                age: since(ready.and_then(|c| c.last_transition_time.as_ref()), now),
            });
        }
        if !schedulable(node) {
            let added = node
                .spec
                .as_ref()
                .and_then(|spec| spec.taints.as_ref())
                .into_iter()
                .flatten()
                .find(|taint| taint.key == UNSCHEDULABLE_TAINT)
                .and_then(|taint| taint.time_added.as_ref());
            problems.push(Problem {
                status: Status::Warn,
                target: node_ref(cluster, node),
                message: CORDONED.to_owned(),
                age: since(added, now),
            });
        }
    }
    problems
}

pub fn connection_problem(cluster: &ClusterId, status: &ConnectionStatus) -> Option<Problem> {
    let ConnectionStatus::AuthExpired(failure) = status else {
        return None;
    };
    let message = match &failure.remedy {
        Some(remedy) => format!("{} · {remedy}", failure.message),
        None => failure.message.clone(),
    };
    Some(Problem {
        status: Status::Err,
        target: ResourceRef {
            cluster: cluster.clone(),
            kind: CLUSTER_KIND.to_owned(),
            namespace: None,
            name: cluster.to_string(),
        },
        message,
        age: Duration::ZERO,
    })
}

pub fn order(problems: &mut [Problem]) {
    let mut worst: HashMap<ClusterId, Status> = HashMap::new();
    for problem in problems.iter() {
        let entry = worst.entry(problem.target.cluster.clone()).or_default();
        *entry = (*entry).max(problem.status);
    }
    let rank = |problem: &Problem| {
        worst
            .get(&problem.target.cluster)
            .copied()
            .unwrap_or_default()
    };
    problems.sort_by(|a, b| {
        rank(b)
            .cmp(&rank(a))
            .then_with(|| a.target.cluster.cmp(&b.target.cluster))
            .then(b.status.cmp(&a.status))
            .then(a.age.cmp(&b.age))
            .then_with(|| a.target.cmp(&b.target))
    });
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::failure::{Failure, FailureKind};

    pub(crate) const NOW: i64 = 1_800_000_000;
    const MINUTE: i64 = 60;

    pub(crate) fn now() -> Timestamp {
        Timestamp::from_second(NOW).unwrap_or_default()
    }

    pub(crate) fn at(minutes_ago: i64) -> String {
        Timestamp::from_second(NOW - minutes_ago * MINUTE)
            .unwrap_or_default()
            .to_string()
    }

    pub(crate) fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Arc<T> {
        match serde_json::from_value(value) {
            Ok(object) => Arc::new(object),
            Err(error) => panic!("fixture: {error}"),
        }
    }

    pub(crate) fn replica_pod(
        name: &str,
        deployment: &str,
        hash: &str,
        status: &Value,
    ) -> Arc<Pod> {
        parse(json!({
            "metadata": {
                "name": name,
                "namespace": "payments",
                "creationTimestamp": at(200),
                "labels": { "pod-template-hash": hash },
                "ownerReferences": [{
                    "apiVersion": "apps/v1", "kind": "ReplicaSet", "uid": "u",
                    "name": format!("{deployment}-{hash}"), "controller": true
                }]
            },
            "spec": { "containers": [{ "name": "api", "image": "i" }] },
            "status": status
        }))
    }

    pub(crate) fn crash_status() -> Value {
        json!({
            "phase": "Running",
            "conditions": [{ "type": "Ready", "status": "False", "lastTransitionTime": at(180) }],
            "containerStatuses": [{
                "name": "api", "image": "i", "imageID": "", "ready": false, "restartCount": 14,
                "state": { "waiting": { "reason": "CrashLoopBackOff", "message": "back-off" } },
                "lastState": { "terminated": { "exitCode": 1 } }
            }]
        })
    }

    pub(crate) fn ready_status() -> Value {
        json!({
            "phase": "Running",
            "conditions": [{ "type": "Ready", "status": "True" }],
            "containerStatuses": [{
                "name": "api", "image": "i", "imageID": "", "ready": true, "restartCount": 0
            }]
        })
    }

    fn cluster() -> ClusterId {
        ClusterId::new("kind-fleet-a")
    }

    #[test]
    fn replica_set_pods_belong_to_their_deployment() {
        let pod = replica_pod(
            "checkout-api-7f9c6d-x2lqp",
            "checkout-api",
            "7f9c6d",
            &ready_status(),
        );
        let owner = owner_of(&pod);
        assert_eq!(
            (owner.kind.as_str(), owner.name.as_str()),
            ("Deployment", "checkout-api")
        );
    }

    #[test]
    fn crashloop_rolls_up_to_the_deployment() {
        let pods = [
            replica_pod(
                "checkout-api-7f9c6d-x2lqp",
                "checkout-api",
                "7f9c6d",
                &crash_status(),
            ),
            replica_pod(
                "checkout-api-7f9c6d-a1b2c",
                "checkout-api",
                "7f9c6d",
                &ready_status(),
            ),
            replica_pod(
                "checkout-api-7f9c6d-d3e4f",
                "checkout-api",
                "7f9c6d",
                &ready_status(),
            ),
        ];
        let problems = pod_problems(&cluster(), &pods, now());
        assert_eq!(problems.len(), 1);
        let problem = &problems[0];
        assert_eq!(problem.status, Status::Err);
        assert_eq!(problem.target.to_string(), "payments/checkout-api");
        assert_eq!(problem.target.kind, "Deployment");
        assert_eq!(
            problem.message,
            "2/3 ready · pod …x2lqp CrashLoopBackOff, 14 restarts, exit 1"
        );
        assert_eq!(problem.age, Duration::from_hours(3));
    }

    #[test]
    fn missing_secret_key_reads_the_kubelet_message() {
        let status = json!({
            "phase": "Pending",
            "containerStatuses": [{
                "name": "webhook", "image": "i", "imageID": "", "ready": false, "restartCount": 0,
                "state": { "waiting": {
                    "reason": "CreateContainerConfigError",
                    "message": "couldn't find key webhook_secret in Secret payments/stripe"
                } }
            }]
        });
        let pods = [replica_pod(
            "stripe-webhook-68bc-b2l6q",
            "stripe-webhook",
            "68bc",
            &status,
        )];
        let problems = pod_problems(&cluster(), &pods, now());
        assert_eq!(
            problems[0].message,
            "0/1 ready · pod …b2l6q CreateContainerConfigError: couldn't find key webhook_secret in Secret payments/stripe"
        );
        assert_eq!(problems[0].age, Duration::from_mins(200));
    }

    #[test]
    fn unschedulable_bare_pod_is_a_pod_warning() {
        let pod: Arc<Pod> = parse(json!({
            "metadata": { "name": "unschedulable", "namespace": "apps" },
            "status": {
                "phase": "Pending",
                "conditions": [{
                    "type": "PodScheduled", "status": "False", "reason": "Unschedulable",
                    "message": "0/1 nodes are available: 1 node(s) didn't match Pod's node affinity/selector.",
                    "lastTransitionTime": at(6)
                }]
            }
        }));
        let problems = pod_problems(&cluster(), &[pod], now());
        let problem = &problems[0];
        assert_eq!(problem.status, Status::Warn);
        assert_eq!(problem.target.kind, "Pod");
        assert_eq!(
            problem.message,
            "FailedScheduling: 0/1 nodes are available: 1 node(s) didn't match Pod's node affinity/selector."
        );
        assert_eq!(problem.age, Duration::from_mins(6));
    }

    #[test]
    fn healthy_completed_and_briefly_unready_pods_are_quiet() {
        let starting = json!({
            "phase": "Running",
            "conditions": [{ "type": "Ready", "status": "False", "lastTransitionTime": at(0) }]
        });
        let done = json!({ "phase": "Succeeded" });
        let pods = [
            replica_pod("web-1-a", "web", "1", &ready_status()),
            replica_pod("web-1-b", "web", "1", &starting),
            replica_pod("web-1-c", "web", "1", &done),
        ];
        assert_eq!(pod_problems(&cluster(), &pods, now()), Vec::new());
    }

    #[test]
    fn nodes_report_not_ready_and_cordoned() {
        let node: Arc<Node> = parse(json!({
            "metadata": { "name": "worker" },
            "spec": {
                "unschedulable": true,
                "taints": [{ "key": "node.kubernetes.io/unschedulable", "effect": "NoSchedule", "timeAdded": at(42) }]
            },
            "status": { "conditions": [{
                "type": "Ready", "status": "Unknown", "message": "Kubelet stopped posting node status.",
                "lastTransitionTime": at(3)
            }] }
        }));
        let problems = node_problems(&cluster(), &[node], now());
        let summary: Vec<(Status, &str, Duration)> = problems
            .iter()
            .map(|p| (p.status, p.message.as_str(), p.age))
            .collect();
        assert_eq!(
            summary,
            [
                (
                    Status::Err,
                    "NotReady: Kubelet stopped posting node status.",
                    Duration::from_secs(180)
                ),
                (
                    Status::Warn,
                    "Cordoned (SchedulingDisabled)",
                    Duration::from_mins(42)
                ),
            ]
        );
    }

    #[test]
    fn expired_credentials_become_a_cluster_problem() {
        let failure = Failure {
            kind: FailureKind::ExpiredCredentials,
            detail: String::new(),
            message: "Credentials expired".into(),
            remedy: Some("run gcloud auth login".into()),
        };
        let problem = connection_problem(&cluster(), &ConnectionStatus::AuthExpired(failure));
        assert_eq!(
            problem.map(|p| p.message),
            Some("Credentials expired · run gcloud auth login".into())
        );
        assert_eq!(
            connection_problem(&cluster(), &ConnectionStatus::Connected),
            None
        );
    }

    #[test]
    fn order_puts_the_worst_cluster_first_then_errors_then_newest() {
        let problem = |cluster: &str, status: Status, minutes: u64| Problem {
            status,
            target: ResourceRef {
                cluster: ClusterId::new(cluster),
                kind: POD_KIND.into(),
                namespace: None,
                name: format!("{cluster}-{minutes}"),
            },
            message: String::new(),
            age: Duration::from_secs(minutes * 60),
        };
        let mut problems = vec![
            problem("a", Status::Warn, 5),
            problem("b", Status::Warn, 9),
            problem("b", Status::Err, 30),
            problem("b", Status::Warn, 1),
        ];
        order(&mut problems);
        let names: Vec<&str> = problems.iter().map(|p| p.target.name.as_str()).collect();
        assert_eq!(names, ["b-30", "b-1", "b-9", "a-5"]);
    }
}
