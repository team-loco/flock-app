use std::collections::BTreeMap;
use std::time::Duration;

use flock_core::flux::{HistoryEntry, Phase, Sentence};
use flock_core::{FluxAction, FluxKey, FluxObject, Status};
use jiff::Timestamp;

use super::convert::elapsed;

const WAIT_MAX: Duration = Duration::from_mins(5);
const REQUESTED: &str = "reconcile requested";
const WAITING: &str = "Reconciliation requested; waiting for the controller to pick it up.";

#[derive(Debug, Default)]
pub struct Requests(BTreeMap<FluxKey, Timestamp>);

pub fn awaiting(requested: Timestamp, handled: Option<&str>, now: Timestamp) -> bool {
    let fresh = elapsed(requested, now) < WAIT_MAX;
    let handled: Option<Timestamp> = handled.and_then(|token| token.parse().ok());
    fresh && handled.is_none_or(|handled| handled < requested)
}

impl Requests {
    pub fn record(&mut self, key: &FluxKey, action: FluxAction, now: Timestamp) {
        self.0.retain(|_, at| elapsed(*at, now) < WAIT_MAX);
        match action {
            FluxAction::Reconcile => self.0.insert(key.clone(), now),
            FluxAction::ReconcileWithSource => self.0.insert(key.clone(), now),
            FluxAction::Force => self.0.insert(key.clone(), now),
            FluxAction::Suspend => self.0.remove(key),
            FluxAction::Resume => self.0.remove(key),
        };
    }

    pub fn apply(&self, object: &mut FluxObject, handled: Option<&str>, now: Timestamp) {
        let Some(requested) = self.0.get(&object.key) else {
            return;
        };
        if object.phase.is_suspended() || !awaiting(*requested, handled, now) {
            return;
        }
        let revision = object
            .applied
            .as_ref()
            .map(|r| r.id.clone())
            .unwrap_or_default();
        object.phase = Phase::Reconciling;
        object.sentence = Sentence::text(WAITING);
        object.history.insert(
            0,
            HistoryEntry {
                age: None,
                status: Status::Progress,
                revision,
                message: REQUESTED.to_owned(),
                count: 1,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use flock_core::Uid;
    use flock_core::flux::FluxDetail;

    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap_or(Timestamp::UNIX_EPOCH)
    }

    fn object(phase: Phase) -> FluxObject {
        FluxObject {
            labels: BTreeMap::new(),
            uid: Uid::default(),
            key: FluxKey::new(
                "a",
                flock_core::FluxKind::Kustomization,
                "flux-system",
                "apps",
            ),
            phase,
            sentence: Sentence::default(),
            applied: None,
            last_healthy: None,
            source: None,
            depends_on: Vec::new(),
            inventory: Vec::new(),
            history: Vec::new(),
            spec: Vec::new(),
            yaml: String::new(),
            detail: FluxDetail::Kustomization,
            conditions: Vec::new(),
            diagnosis: None,
        }
    }

    #[test]
    fn requests_wait_until_the_controller_handles_a_newer_token() {
        let requested = at("2026-10-13T10:00:00Z");
        let now = at("2026-10-13T10:00:05Z");
        assert!(awaiting(requested, None, now));
        assert!(awaiting(requested, Some("2026-10-13T09:00:00Z"), now));
        assert!(!awaiting(requested, Some("2026-10-13T10:00:00.5Z"), now));
        assert!(!awaiting(requested, None, at("2026-10-13T10:06:00Z")));
    }

    #[test]
    fn pending_reconcile_shows_progress_until_handled() {
        let mut requests = Requests::default();
        let key = object(Phase::Ready).key;
        let now = at("2026-10-13T10:00:00Z");
        requests.record(&key, FluxAction::Reconcile, now);
        let mut running = object(Phase::Ready);
        requests.apply(&mut running, Some("2026-10-13T09:00:00Z"), now);
        assert_eq!(running.phase, Phase::Reconciling);
        assert_eq!(running.history[0].message, REQUESTED);
        let mut done = object(Phase::Failing);
        requests.apply(&mut done, Some("2026-10-13T10:00:01Z"), now);
        assert_eq!(done.phase, Phase::Failing);
        requests.record(&key, FluxAction::Suspend, now);
        let mut after = object(Phase::Ready);
        requests.apply(&mut after, None, now);
        assert_eq!(after.phase, Phase::Ready);
    }
}
