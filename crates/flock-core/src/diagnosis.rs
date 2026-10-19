use std::time::Duration;

use crate::Status;
use crate::events::Event;

pub const CONDITION_TRUE: &str = "True";
pub const CONDITION_FALSE: &str = "False";
pub const READY: &str = "Ready";
const NOT_PREFIX: &str = "Not";
const NOT_READY: &str = "NotReady";
const PENDING: &str = "Pending";
const LOST: &str = "Lost";
const LOST_MESSAGE: &str = "The volume bound to this claim no longer exists.";
const NEGATIVE: [&str; 9] = [
    "Failed",
    "FailureTarget",
    "Stalled",
    "ReplicaFailure",
    "MemoryPressure",
    "DiskPressure",
    "PIDPressure",
    "NetworkUnavailable",
    "DisruptionTarget",
];
const IN_PROGRESS: [&str; 3] = ["Reconciling", "Resizing", "FileSystemResizePending"];
const SUSPENDED: &str = "Suspended";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Condition {
    pub kind: String,
    pub value: String,
    pub reason: String,
    pub message: String,
    pub age: Option<Duration>,
}

impl Condition {
    pub fn new(kind: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            value: value.into(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = reason.into();
        self
    }

    #[must_use]
    pub fn message(mut self, message: impl Into<String>) -> Self {
        message.into().trim().clone_into(&mut self.message);
        self
    }

    #[must_use]
    pub fn age(mut self, age: Option<Duration>) -> Self {
        self.age = age;
        self
    }

    fn negative(&self) -> bool {
        NEGATIVE.contains(&self.kind.as_str())
    }

    pub fn is_true(&self) -> bool {
        self.value == CONDITION_TRUE
    }

    pub fn status(&self) -> Status {
        let set = self.is_true();
        let unset = self.value == CONDITION_FALSE;
        if IN_PROGRESS.contains(&self.kind.as_str()) {
            return if set { Status::Progress } else { Status::Ok };
        }
        if self.kind == SUSPENDED {
            return if set { Status::Suspended } else { Status::Ok };
        }
        let (good, bad) = if self.negative() {
            (unset, set)
        } else {
            (set, unset)
        };
        if good {
            Status::Ok
        } else if bad {
            Status::Err
        } else {
            Status::Warn
        }
    }

    pub fn is_healthy(&self) -> bool {
        self.status() == Status::Ok
    }

    pub fn label(&self) -> String {
        if !self.reason.is_empty() {
            self.reason.clone()
        } else if self.negative() {
            self.kind.clone()
        } else {
            format!("{NOT_PREFIX}{}", self.kind)
        }
    }

    fn diagnosis(&self) -> Diagnosis {
        Diagnosis::new(self.label(), self.message.clone())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diagnosis {
    pub reason: String,
    pub message: String,
}

impl Diagnosis {
    pub fn new(reason: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            message: message.into().trim().to_owned(),
        }
    }

    pub fn text(&self) -> String {
        if self.message.is_empty() {
            self.reason.clone()
        } else if self.reason.is_empty() {
            self.message.clone()
        } else {
            format!("{}: {}", self.reason, self.message)
        }
    }
}

pub fn needs_diagnosis(status: Status) -> bool {
    matches!(status, Status::Err | Status::Warn)
}

fn worst<'a>(conditions: impl IntoIterator<Item = &'a Condition>) -> Option<&'a Condition> {
    conditions
        .into_iter()
        .filter(|condition| needs_diagnosis(condition.status()))
        .fold(None, |picked: Option<&Condition>, next| match picked {
            Some(current) if current.status() >= next.status() => Some(current),
            _ => Some(next),
        })
}

pub fn from_conditions(conditions: &[Condition]) -> Option<Diagnosis> {
    worst(conditions).map(Condition::diagnosis)
}

pub fn node(conditions: &[Condition]) -> Option<Diagnosis> {
    let ready = conditions.iter().find(|c| c.kind == READY);
    if let Some(ready) = ready.filter(|ready| !ready.is_true()) {
        return Some(Diagnosis::new(NOT_READY, ready.message.clone()));
    }
    let pressure = worst(conditions.iter().filter(|c| c.negative()))?;
    Some(Diagnosis::new(
        pressure.kind.clone(),
        pressure.message.clone(),
    ))
}

pub fn claim(phase: &str, conditions: &[Condition]) -> Option<Diagnosis> {
    match phase {
        PENDING => {
            let message = worst(conditions).map(|c| c.message.clone());
            Some(Diagnosis::new(PENDING, message.unwrap_or_default()))
        }
        LOST => Some(Diagnosis::new(LOST, LOST_MESSAGE)),
        _ => from_conditions(conditions),
    }
}

pub fn about<'a>(
    events: &'a [Event],
    kind: &str,
    namespace: Option<&str>,
    name: &str,
) -> Vec<&'a Event> {
    let mut found: Vec<&Event> = events
        .iter()
        .filter(|event| event.is_about(kind, namespace, name))
        .collect();
    found.sort_by_key(|event| event.age);
    found
}

pub fn with_events(diagnosis: Option<Diagnosis>, events: &[&Event]) -> Option<Diagnosis> {
    let warning = events.iter().find(|event| event.is_warning());
    match (diagnosis, warning) {
        (Some(diagnosis), Some(event)) if diagnosis.message.is_empty() => Some(Diagnosis {
            message: Diagnosis::new(event.reason.clone(), event.message.clone()).text(),
            ..diagnosis
        }),
        (diagnosis, _) => diagnosis,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::events::EventDetails;

    fn condition(kind: &str, value: &str, reason: &str, message: &str) -> Condition {
        Condition::new(kind, value).reason(reason).message(message)
    }

    fn event(status: Status, object: &str, reason: &str, minutes: u64) -> Event {
        Event {
            labels: BTreeMap::new(),
            details: EventDetails::default(),
            status,
            reason: reason.into(),
            object: object.into(),
            namespace: Some("db".into()),
            message: format!("{reason} message"),
            age: Duration::from_secs(minutes * 60),
            count: 1,
        }
    }

    #[test]
    fn polarity_follows_the_condition_type() {
        assert_eq!(Condition::new("Ready", "True").status(), Status::Ok);
        assert_eq!(Condition::new("Ready", "False").status(), Status::Err);
        assert_eq!(Condition::new("Ready", "Unknown").status(), Status::Warn);
        assert_eq!(Condition::new("Failed", "True").status(), Status::Err);
        assert_eq!(
            Condition::new("MemoryPressure", "False").status(),
            Status::Ok
        );
        assert_eq!(
            Condition::new("Reconciling", "True").status(),
            Status::Progress
        );
        assert_eq!(Condition::new("Ready", "False").label(), "NotReady");
        assert_eq!(
            Condition::new("DiskPressure", "True").label(),
            "DiskPressure"
        );
    }

    #[test]
    fn failed_jobs_report_the_deadline_or_backoff_reason() {
        let conditions = [
            condition("Suspended", "False", "", ""),
            condition(
                "Failed",
                "True",
                "DeadlineExceeded",
                "Job was active longer than specified deadline",
            ),
        ];
        assert_eq!(
            from_conditions(&conditions),
            Some(Diagnosis::new(
                "DeadlineExceeded",
                "Job was active longer than specified deadline"
            ))
        );
        let backoff = [condition(
            "Failed",
            "True",
            "BackoffLimitExceeded",
            "Job has reached the specified backoff limit",
        )];
        assert_eq!(
            from_conditions(&backoff).map(|d| d.reason),
            Some("BackoffLimitExceeded".into())
        );
        let complete = [condition("Complete", "True", "", "")];
        assert_eq!(from_conditions(&complete), None);
    }

    #[test]
    fn the_first_of_the_worst_conditions_wins() {
        let conditions = [
            condition("Available", "Unknown", "Waiting", ""),
            condition(
                "Progressing",
                "False",
                "ProgressDeadlineExceeded",
                "timed out",
            ),
            condition("ReplicaFailure", "True", "FailedCreate", "quota"),
        ];
        assert_eq!(
            from_conditions(&conditions),
            Some(Diagnosis::new("ProgressDeadlineExceeded", "timed out"))
        );
    }

    #[test]
    fn nodes_say_not_ready_before_pressure() {
        let pressured = [
            condition("Ready", "True", "KubeletReady", ""),
            condition(
                "MemoryPressure",
                "True",
                "KubeletHasInsufficientMemory",
                "low",
            ),
        ];
        assert_eq!(
            node(&pressured),
            Some(Diagnosis::new("MemoryPressure", "low"))
        );
        let down = [
            condition(
                "Ready",
                "Unknown",
                "NodeStatusUnknown",
                "Kubelet stopped posting node status.",
            ),
            condition("MemoryPressure", "Unknown", "", ""),
        ];
        assert_eq!(
            node(&down),
            Some(Diagnosis::new(
                "NotReady",
                "Kubelet stopped posting node status."
            ))
        );
        assert_eq!(node(&[condition("Ready", "True", "", "")]), None);
    }

    #[test]
    fn pending_claims_borrow_the_newest_warning_event() {
        let events = [
            event(
                Status::Ok,
                "persistentvolumeclaim/data-0",
                "ExternalProvisioning",
                1,
            ),
            event(
                Status::Warn,
                "persistentvolumeclaim/data-0",
                "ProvisioningFailed",
                2,
            ),
            event(Status::Warn, "persistentvolumeclaim/data-1", "Other", 0),
        ];
        let mine = about(&events, "PersistentVolumeClaim", Some("db"), "data-0");
        assert_eq!(mine.len(), 2);
        let diagnosis = with_events(claim("Pending", &[]), &mine);
        assert_eq!(
            diagnosis,
            Some(Diagnosis::new(
                "Pending",
                "ProvisioningFailed: ProvisioningFailed message"
            ))
        );
        assert_eq!(claim("Bound", &[]), None);
        assert_eq!(claim("Lost", &[]).map(|d| d.reason), Some("Lost".into()));
        let explained = Some(Diagnosis::new("Failed", "kept"));
        assert_eq!(with_events(explained.clone(), &mine), explained);
    }

    #[test]
    fn text_joins_reason_and_message() {
        assert_eq!(Diagnosis::new("OOMKilled", "").text(), "OOMKilled");
        assert_eq!(Diagnosis::new("", "gone").text(), "gone");
        assert_eq!(Diagnosis::new("A", " b ").text(), "A: b");
    }
}
