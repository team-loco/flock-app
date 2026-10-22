use std::time::Duration;

use crate::Status;
use crate::diagnosis::{Condition, Diagnosis, READY, needs_diagnosis};
use crate::time::age;

const SEPARATOR: &str = " · ";
const PROBLEM_ORDER: [&str; 6] = [
    "Stalled",
    "ReplicaFailure",
    "Released",
    "Progressing",
    "Ready",
    "Available",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub status: Status,
    pub reason: String,
    pub message: String,
    pub object: String,
}

impl Issue {
    pub fn new(
        status: Status,
        reason: impl Into<String>,
        message: impl Into<String>,
        object: impl Into<String>,
    ) -> Self {
        Self {
            status,
            reason: reason.into(),
            message: message.into().trim().to_owned(),
            object: object.into(),
        }
    }

    pub fn from_diagnosis(
        status: Status,
        diagnosis: &Diagnosis,
        object: impl Into<String>,
    ) -> Self {
        Self::new(
            status,
            diagnosis.reason.clone(),
            diagnosis.message.clone(),
            object,
        )
    }

    pub fn from_condition(kind: &str, name: &str, condition: &Condition) -> Self {
        let object = object_line([
            Some(format!("{kind} {name}")),
            Some(format!("{}={}", condition.kind, condition.value)),
            condition.age.map(age),
        ]);
        Self::new(
            condition.status(),
            condition.label(),
            condition.message.clone(),
            object,
        )
    }
}

pub fn object_line<const N: usize>(parts: [Option<String>; N]) -> String {
    let parts: Vec<String> = parts.into_iter().flatten().collect();
    parts.join(SEPARATOR)
}

pub fn named(kind: &str, name: &str, since: Option<Duration>) -> String {
    object_line([Some(format!("{kind} {name}")), since.map(age)])
}

pub fn failing_condition(conditions: &[Condition]) -> Option<&Condition> {
    conditions
        .iter()
        .filter(|condition| needs_diagnosis(condition.status()))
        .min_by_key(|condition| (rank(condition.status()), order(&condition.kind)))
}

pub fn headline_condition(conditions: &[Condition]) -> Option<&Condition> {
    failing_condition(conditions)
        .or_else(|| conditions.iter().find(|condition| condition.kind == READY))
        .or_else(|| conditions.first())
}

fn rank(status: Status) -> u8 {
    u8::from(status != Status::Err)
}

fn order(kind: &str) -> usize {
    PROBLEM_ORDER
        .iter()
        .position(|known| *known == kind)
        .unwrap_or(PROBLEM_ORDER.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn available(value: &str, reason: &str) -> Condition {
        Condition::new("Available", value).reason(reason)
    }

    fn stuck() -> Condition {
        Condition::new("Progressing", "False")
            .reason("ProgressDeadlineExceeded")
            .message("ReplicaSet \"checkout-api-7f9c6d\" has timed out progressing.")
            .age(Some(Duration::from_hours(3)))
    }

    #[test]
    fn the_headline_is_the_failing_condition_then_ready_then_the_first() {
        let reconciling = Condition::new("Reconciling", "True").reason("Progressing");
        let ready = Condition::new("Ready", "True").reason("ReconciliationSucceeded");
        let failed = Condition::new("Ready", "False").reason("HealthCheckFailed");
        let healthy = [reconciling.clone(), ready];
        let failing = [reconciling.clone(), failed];
        let only = [reconciling];
        let pick =
            |conditions: &[Condition]| headline_condition(conditions).map(|c| c.reason.clone());
        assert_eq!(pick(&healthy).as_deref(), Some("ReconciliationSucceeded"));
        assert_eq!(pick(&failing).as_deref(), Some("HealthCheckFailed"));
        assert_eq!(pick(&only).as_deref(), Some("Progressing"));
        assert_eq!(pick(&[]), None);
    }

    #[test]
    fn progressing_explains_a_stuck_rollout_better_than_available() {
        let conditions = [available("False", "MinimumReplicasUnavailable"), stuck()];
        let picked = failing_condition(&conditions).map(|c| c.reason.as_str());
        assert_eq!(picked, Some("ProgressDeadlineExceeded"));
    }

    #[test]
    fn stalled_comes_before_ready_and_released_before_ready() {
        let ready = Condition::new("Ready", "False").reason("ReconciliationFailed");
        let stalled = Condition::new("Stalled", "True").reason("RetriesExceeded");
        let released = Condition::new("Released", "False").reason("UpgradeFailed");
        let flux = failing_condition(&[ready.clone(), stalled]).map(|c| c.kind.clone());
        let helm = failing_condition(&[ready, released]).map(|c| c.kind.clone());
        assert_eq!(flux.as_deref(), Some("Stalled"));
        assert_eq!(helm.as_deref(), Some("Released"));
    }

    #[test]
    fn errors_win_over_warnings_and_healthy_or_progressing_objects_have_none() {
        let unknown = Condition::new("Ready", "Unknown").reason("Progressing");
        let failed = Condition::new("Available", "False").reason("MinimumReplicasUnavailable");
        let picked = failing_condition(&[unknown.clone(), failed]).map(|c| c.kind.clone());
        assert_eq!(picked.as_deref(), Some("Available"));
        let reconciling = Condition::new("Reconciling", "True");
        let healthy = available("True", "MinimumReplicasAvailable");
        assert_eq!(failing_condition(&[reconciling, healthy]), None);
        assert_eq!(failing_condition(&[]), None);
        assert!(failing_condition(&[unknown]).is_some());
    }

    #[test]
    fn a_condition_issue_names_the_object_condition_and_age() {
        let issue = Issue::from_condition("Deployment", "checkout-api", &stuck());
        assert_eq!(issue.status, Status::Err);
        assert_eq!(issue.reason, "ProgressDeadlineExceeded");
        assert_eq!(
            issue.message,
            "ReplicaSet \"checkout-api-7f9c6d\" has timed out progressing."
        );
        assert_eq!(
            issue.object,
            "Deployment checkout-api · Progressing=False · 3h"
        );
    }

    #[test]
    fn object_lines_skip_missing_parts() {
        assert_eq!(named("Pod", "web-0", None), "Pod web-0");
        assert_eq!(
            named("Service", "web", Some(Duration::from_secs(120))),
            "Service web · 2m"
        );
    }
}
