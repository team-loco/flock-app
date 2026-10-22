use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use flock_core::home::Problem;
use flock_core::pods::PodEvent;
use flock_core::{ClusterId, Diagnosis, ResourceRef, Status};
use jiff::Timestamp;
use k8s_openapi::api::core::v1::{ContainerStatus, Event, Node, Pod, PodCondition};

use crate::clock::{age, since};
use crate::manager::ConnectionStatus;
use crate::snapshot::{FAILING_WAIT_REASONS, node_ready, schedulable};
use crate::workload;

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
const FAILED_SCHEDULING: &str = "FailedScheduling";
const SCHEDULING_DISABLED: &str = "SchedulingDisabled";
const CORDONED_SPEC: &str = "spec.unschedulable: true";
const UNAUTHORIZED: &str = "Unauthorized";
const CONNECTION: &str = "connection";
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
            Some(Issue::Waiting {
                reason: reason.to_owned(),
                message: waiting.message.clone().unwrap_or_default(),
                restarts: container.restart_count,
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
    issue: Issue,
    age: Duration,
    raw: Diagnosis,
    condition: String,
}

#[derive(Default)]
struct Group {
    flagged: Vec<Flagged>,
    replica_sets: BTreeSet<String>,
    pods: BTreeSet<String>,
}

fn condition_line(pod: &Pod, kind: &str) -> String {
    condition(pod, kind)
        .map(|c| format!("{}={}", c.type_, c.status))
        .unwrap_or_default()
}

fn raw(pod: &Pod, issue: &Issue) -> (Diagnosis, String) {
    match issue {
        Issue::Waiting {
            reason, message, ..
        } => {
            let raw = Diagnosis::new(reason.clone(), message.clone());
            (raw, condition_line(pod, READY_CONDITION))
        }
        Issue::Unschedulable { message } => {
            let raw = Diagnosis::new(FAILED_SCHEDULING, message.clone());
            (raw, condition_line(pod, SCHEDULED_CONDITION))
        }
        Issue::NotReady { .. } => {
            let ready = condition(pod, READY_CONDITION);
            let reason = ready
                .and_then(|c| c.reason.clone())
                .unwrap_or_else(|| NOT_READY.to_owned());
            let message = ready.and_then(|c| c.message.clone()).unwrap_or_default();
            (
                Diagnosis::new(reason, message),
                condition_line(pod, READY_CONDITION),
            )
        }
    }
}

fn replica_set(pod: &Pod) -> Option<String> {
    pod.metadata
        .owner_references
        .iter()
        .flatten()
        .find(|owner| owner.controller == Some(true) && owner.kind == REPLICA_SET)
        .map(|owner| owner.name.clone())
}

pub fn pod_problems(
    cluster: &ClusterId,
    pods: &[Arc<Pod>],
    events: &[Arc<Event>],
    now: Timestamp,
) -> Vec<Problem> {
    let mut groups: HashMap<OwnerKey, Group> = HashMap::new();
    for pod in pods.iter().filter(|pod| is_active(pod)) {
        let Some((issue, age)) = pod_issue(pod, now) else {
            continue;
        };
        let group = groups.entry(owner_of(pod)).or_default();
        group.pods.extend(pod.metadata.name.clone());
        group.replica_sets.extend(replica_set(pod));
        let (raw, condition) = raw(pod, &issue);
        group.flagged.push(Flagged {
            issue,
            age,
            raw,
            condition,
        });
    }
    groups
        .into_iter()
        .filter_map(|(owner, group)| group_problem(cluster, owner, &group, events, now))
        .collect()
}

fn group_problem(
    cluster: &ClusterId,
    owner: OwnerKey,
    group: &Group,
    events: &[Arc<Event>],
    now: Timestamp,
) -> Option<Problem> {
    let worst = group.flagged.iter().max_by(|a, b| {
        a.issue
            .status()
            .cmp(&b.issue.status())
            .then(a.age.cmp(&b.age))
    })?;
    let target = ResourceRef {
        cluster: cluster.clone(),
        kind: owner.kind,
        namespace: Some(owner.namespace),
        name: owner.name,
    };
    let events = workload::events(&target, &group.replica_sets, &group.pods, events, now);
    Some(Problem {
        status: worst.issue.status(),
        target,
        reason: worst.raw.reason.clone(),
        message: worst.raw.message.clone(),
        condition: worst.condition.clone(),
        age: worst.age,
        events,
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

pub fn target_events(target: &ResourceRef, events: &[Arc<Event>], now: Timestamp) -> Vec<PodEvent> {
    let none = BTreeSet::new();
    workload::events(target, &none, &none, events, now)
}

pub fn node_problems(
    cluster: &ClusterId,
    nodes: &[Arc<Node>],
    events: &[Arc<Event>],
    now: Timestamp,
) -> Vec<Problem> {
    let mut problems = Vec::new();
    for node in nodes {
        let target = node_ref(cluster, node);
        let status = node.status.as_ref();
        if !node_ready(node) {
            let ready = status
                .and_then(|s| s.conditions.as_ref())
                .into_iter()
                .flatten()
                .find(|c| c.type_ == READY_CONDITION);
            let reason = ready
                .and_then(|c| c.reason.clone())
                .unwrap_or_else(|| NOT_READY.to_owned());
            let message = ready.and_then(|c| c.message.clone()).unwrap_or_default();
            let condition = ready
                .map(|c| format!("{}={}", c.type_, c.status))
                .unwrap_or_default();
            problems.push(Problem {
                status: Status::Err,
                target: target.clone(),
                reason,
                message,
                condition,
                age: since(ready.and_then(|c| c.last_transition_time.as_ref()), now),
                events: target_events(&target, events, now),
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
                target: target.clone(),
                reason: SCHEDULING_DISABLED.to_owned(),
                message: CORDONED_SPEC.to_owned(),
                condition: String::new(),
                age: since(added, now),
                events: target_events(&target, events, now),
            });
        }
    }
    problems
}

pub fn connection_problem(cluster: &ClusterId, status: &ConnectionStatus) -> Option<Problem> {
    let ConnectionStatus::AuthExpired(failure) = status else {
        return None;
    };
    Some(Problem {
        status: Status::Err,
        target: ResourceRef {
            cluster: cluster.clone(),
            kind: CLUSTER_KIND.to_owned(),
            namespace: None,
            name: cluster.to_string(),
        },
        reason: UNAUTHORIZED.to_owned(),
        message: failure.message.clone(),
        condition: CONNECTION.to_owned(),
        age: Duration::ZERO,
        events: Vec::new(),
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
        let event = |kind: &str, name: &str, minutes: i64| -> Arc<Event> {
            parse(json!({
                "metadata": { "name": format!("{name}.1"), "namespace": "payments" },
                "involvedObject": { "kind": kind, "name": name, "namespace": "payments" },
                "reason": "BackOff", "type": "Warning", "lastTimestamp": at(minutes)
            }))
        };
        let events = [
            event("Pod", "checkout-api-7f9c6d-x2lqp", 2),
            event("ReplicaSet", "checkout-api-7f9c6d", 30),
            event("Pod", "checkout-api-7f9c6d-a1b2c", 1),
            event("Pod", "someone-else", 1),
        ];
        let problems = pod_problems(&cluster(), &pods, &events, now());
        assert_eq!(problems.len(), 1);
        let problem = &problems[0];
        assert_eq!(problem.status, Status::Err);
        assert_eq!(problem.target.to_string(), "payments/checkout-api");
        assert_eq!(problem.target.kind, "Deployment");
        assert_eq!(
            (
                problem.reason.as_str(),
                problem.message.as_str(),
                problem.condition.as_str()
            ),
            ("CrashLoopBackOff", "back-off", "Ready=False")
        );
        assert_eq!(problem.age, Duration::from_hours(3));
        let ages: Vec<Duration> = problem.events.iter().map(|e| e.age).collect();
        assert_eq!(ages, [Duration::from_mins(2), Duration::from_mins(30)]);
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
        let problems = pod_problems(&cluster(), &pods, &[], now());
        assert_eq!(problems[0].reason, "CreateContainerConfigError");
        assert_eq!(
            problems[0].message,
            "couldn't find key webhook_secret in Secret payments/stripe"
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
        let problems = pod_problems(&cluster(), &[pod], &[], now());
        let problem = &problems[0];
        assert_eq!(problem.status, Status::Warn);
        assert_eq!(problem.target.kind, "Pod");
        assert_eq!(
            (problem.reason.as_str(), problem.condition.as_str()),
            ("FailedScheduling", "PodScheduled=False")
        );
        assert_eq!(
            problem.message,
            "0/1 nodes are available: 1 node(s) didn't match Pod's node affinity/selector."
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
        assert_eq!(pod_problems(&cluster(), &pods, &[], now()), Vec::new());
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
                "type": "Ready", "status": "Unknown", "reason": "NodeStatusUnknown",
                "message": "Kubelet stopped posting node status.",
                "lastTransitionTime": at(3)
            }] }
        }));
        let problems = node_problems(&cluster(), &[node], &[], now());
        let summary: Vec<(Status, &str, &str, &str, Duration)> = problems
            .iter()
            .map(|p| {
                (
                    p.status,
                    p.reason.as_str(),
                    p.message.as_str(),
                    p.condition.as_str(),
                    p.age,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (
                    Status::Err,
                    "NodeStatusUnknown",
                    "Kubelet stopped posting node status.",
                    "Ready=Unknown",
                    Duration::from_secs(180)
                ),
                (
                    Status::Warn,
                    "SchedulingDisabled",
                    "spec.unschedulable: true",
                    "",
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
            problem.map(|p| (p.reason, p.message, p.condition)),
            Some((
                "Unauthorized".into(),
                "Credentials expired".into(),
                "connection".into()
            ))
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
            reason: String::new(),
            message: String::new(),
            condition: String::new(),
            age: Duration::from_secs(minutes * 60),
            events: Vec::new(),
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
