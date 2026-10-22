use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::GitOpsObject;

pub const REQUESTED_AT: &str = "reconcile.fluxcd.io/requestedAt";
pub const FORCE_AT: &str = "reconcile.fluxcd.io/forceAt";
pub const RESET_AT: &str = "reconcile.fluxcd.io/resetAt";
pub const FORCE: &str = "force";

pub fn reconcile(at: DateTime<Utc>) -> Value {
    let token = at.to_rfc3339();
    json!({"metadata": {"annotations": {REQUESTED_AT: token}}})
}

pub fn force_upgrade(at: DateTime<Utc>) -> Value {
    let token = at.to_rfc3339();
    json!({"metadata": {"annotations": {REQUESTED_AT: token, FORCE_AT: token}}})
}

pub fn force_apply(at: DateTime<Utc>) -> Value {
    let token = at.to_rfc3339();
    json!({"metadata": {"annotations": {REQUESTED_AT: token}}, "spec": {FORCE: true}})
}

pub fn force_setting(object: &GitOpsObject) -> Option<bool> {
    object.spec.get(FORCE).and_then(Value::as_bool)
}

pub fn restore_force(original: Option<bool>) -> Value {
    json!({"spec": {FORCE: original}})
}

pub fn handled(object: &GitOpsObject, at: DateTime<Utc>, generation: i64) -> bool {
    let token = at.to_rfc3339();
    let requested = object.last_handled_reconcile_at.as_deref() == Some(token.as_str());
    let observed = object
        .observed_generation
        .is_some_and(|observed| observed >= generation);
    requested && observed
}

pub fn reset(at: DateTime<Utc>) -> Value {
    let token = at.to_rfc3339();
    json!({"metadata": {"annotations": {REQUESTED_AT: token, RESET_AT: token}}})
}

pub fn suspend() -> Value {
    json!({"spec": {"suspend": true}})
}
pub fn resume() -> Value {
    json!({"spec": {"suspend": false}})
}
