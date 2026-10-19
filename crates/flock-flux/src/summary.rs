use serde::{Deserialize, Serialize};

use crate::object::{Condition, GitOpsObject};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warn,
    Err,
    Progress,
    Suspended,
    Offline,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Summary {
    pub status: Status,
    pub sentence: String,
    pub detail: Option<String>,
}

fn result(status: Status, sentence: impl Into<String>, condition: Option<&Condition>) -> Summary {
    let detail = condition
        .filter(|condition| !condition.message.is_empty())
        .map(|condition| condition.message.clone());
    Summary {
        status,
        sentence: sentence.into(),
        detail,
    }
}

fn active<'a>(object: &'a GitOpsObject, type_: &str) -> Option<&'a Condition> {
    object
        .condition(type_)
        .filter(|condition| condition.status == "True" && object.current(condition))
}

fn release_failure(object: &GitOpsObject, condition: &Condition, action: &str) -> Summary {
    let failures = if action == "install" {
        object.install_failures
    } else {
        object.upgrade_failures
    };
    let strategy_path = format!("/{action}/strategy/name");
    let retrying = object
        .spec
        .pointer(&strategy_path)
        .and_then(serde_json::Value::as_str)
        == Some("RetryOnFailure");
    let retries_path = format!("/{action}/remediation/retries");
    let retries = object
        .spec
        .pointer(&retries_path)
        .and_then(serde_json::Value::as_i64)
        .unwrap_or_default();
    let exhausted = !retrying && retries >= 0 && failures > retries;
    let sentence = if exhausted {
        format!("Helm {action} failed and retries are exhausted.")
    } else {
        format!("Helm {action} failed and will be retried.")
    };
    let status = if exhausted { Status::Err } else { Status::Warn };
    let mut summary = result(status, sentence, Some(condition));
    if let Some(remediation) = active(object, "Remediated") {
        let detail = summary.detail.get_or_insert_with(String::new);
        if !detail.is_empty() {
            detail.push(' ');
        }
        detail.push_str(&remediation.message);
    }
    summary
}

const MISSING_PATH_SENTENCE: &str = "The build path does not exist in the source artifact.";
const MISSING_PATH_MARKERS: [&str; 2] = ["no such file or directory", "path not found"];

fn missing_path(condition: &Condition) -> bool {
    MISSING_PATH_MARKERS
        .iter()
        .any(|marker| condition.message.contains(marker))
}

fn failure(object: &GitOpsObject, condition: &Condition) -> Summary {
    match condition.reason.as_str() {
        "InstallFailed" => release_failure(object, condition, "install"),
        "UpgradeFailed" => release_failure(object, condition, "upgrade"),
        "DependencyNotReady" => result(Status::Warn, "A dependency is not ready.", Some(condition)),
        "ArtifactFailed" | "BuildFailed" if missing_path(condition) => {
            result(Status::Err, MISSING_PATH_SENTENCE, Some(condition))
        }
        "ArtifactFailed" => result(
            Status::Err,
            "The source artifact could not be loaded.",
            Some(condition),
        ),
        "BuildFailed" => result(
            Status::Err,
            "The source could not be built.",
            Some(condition),
        ),
        "HealthCheckFailed" => result(
            Status::Err,
            "Managed resources failed their health checks.",
            Some(condition),
        ),
        "SourceNotReady" => result(Status::Warn, "The source is not ready.", Some(condition)),
        "Progressing" => result(
            Status::Progress,
            "Reconciliation is in progress.",
            Some(condition),
        ),
        _ => result(Status::Err, "Reconciliation failed.", Some(condition)),
    }
}

const RELEASED: &str = "Released";
const STALLED: &str = "Stalled";
const FALSE: &str = "False";

pub fn failing_condition(object: &GitOpsObject) -> Option<&Condition> {
    if object.suspended {
        return None;
    }
    let ready = object.ready().filter(|condition| condition.status == FALSE);
    let released = object
        .condition(RELEASED)
        .filter(|condition| condition.status == FALSE);
    active(object, STALLED).or(released).or(ready)
}

pub fn summarize(object: &GitOpsObject, source: Option<&GitOpsObject>) -> Summary {
    if object.suspended {
        return result(Status::Suspended, "Reconciliation is suspended.", None);
    }
    let ready = object.ready().filter(|condition| object.current(condition));
    let stale = object.ready().is_some() && ready.is_none();
    if stale {
        return result(
            Status::Progress,
            "The latest specification is waiting to be reconciled.",
            None,
        );
    }
    if let Some(stalled) = active(object, "Stalled") {
        let sentence = if stalled.reason == "RetriesExceeded" {
            "Helm release retries are exhausted."
        } else {
            "Reconciliation has stalled and needs intervention."
        };
        return result(Status::Err, sentence, Some(stalled));
    }
    if let Some(source) = source.filter(|source| {
        object.source_ref.as_ref().is_some_and(|reference| {
            reference.name == source.identity.name
                && reference.namespace == source.identity.namespace
                && reference.kind == source.identity.kind.as_str()
        })
    }) {
        let source_summary = summarize(source, None);
        if source_summary.status != Status::Ok {
            return Summary {
                status: Status::Warn,
                sentence: format!("Source {} is not ready.", source.identity.name),
                detail: source_summary.detail.or(Some(source_summary.sentence)),
            };
        }
    }
    if let Some(condition) = ready.filter(|condition| condition.status == "False") {
        return failure(object, condition);
    }
    if let Some(drift) = active(object, "Drifted") {
        return result(
            Status::Warn,
            "Deployed resources differ from the desired release.",
            Some(drift),
        );
    }
    if let Some(reconciling) = active(object, "Reconciling") {
        return result(
            Status::Progress,
            "Reconciliation is in progress.",
            Some(reconciling),
        );
    }
    match ready {
        Some(condition) if condition.status == "True" => {
            result(Status::Ok, "The object is ready.", Some(condition))
        }
        Some(condition) => result(Status::Offline, "Readiness is unknown.", Some(condition)),
        None => result(
            Status::Progress,
            "The object is waiting for its first reconciliation.",
            None,
        ),
    }
}
