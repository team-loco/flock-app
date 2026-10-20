use std::ops::Range;

use crate::Status;
use crate::diagnosis::Condition;

pub const MESSAGE_LIMIT: usize = 200;
const SEPARATOR: &str = " · ";
const SPACE: &str = " ";
const ELLIPSIS: char = '…';
const SUSPENDED_HEAD: &str = "spec.suspend=true";
const LAST_APPLIED: &str = "last applied";
const PROBLEM_ORDER: [&str; 6] = [
    "Stalled",
    "ReplicaFailure",
    "Released",
    "Progressing",
    "Ready",
    "Available",
];
const PRIMARY: [&str; 2] = ["Ready", "Available"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConditionLine {
    pub head: String,
    pub status: Status,
    pub reason: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineText {
    pub text: String,
    pub head: Range<usize>,
    pub separator: Option<Range<usize>>,
}

impl ConditionLine {
    pub fn pick(conditions: &[Condition]) -> Option<Self> {
        let problem = conditions
            .iter()
            .filter_map(|condition| {
                let rank = problem_rank(condition.status())?;
                Some((rank, problem_order(&condition.kind), condition))
            })
            .min_by_key(|(rank, order, _)| (*rank, *order))
            .map(|(_, _, condition)| condition);
        let primary = || {
            PRIMARY
                .iter()
                .find_map(|kind| conditions.iter().find(|c| c.kind == *kind))
                .or_else(|| conditions.first())
        };
        problem.or_else(primary).map(Self::from_condition)
    }

    pub fn suspended(applied: Option<&str>) -> Self {
        let message = applied
            .map(|revision| format!("{LAST_APPLIED} {revision}"))
            .unwrap_or_default();
        Self {
            head: SUSPENDED_HEAD.into(),
            status: Status::Suspended,
            reason: String::new(),
            message,
        }
    }

    fn from_condition(condition: &Condition) -> Self {
        Self {
            head: format!("{}={}", condition.kind, condition.value),
            status: condition.status(),
            reason: condition.reason.clone(),
            message: one_line(&condition.message),
        }
    }

    pub fn full(&self) -> LineText {
        let mut text = self.head.clone();
        let head = 0..text.len();
        if !self.reason.is_empty() {
            text.push_str(SPACE);
            text.push_str(&self.reason);
        }
        let separator = (!self.message.is_empty()).then(|| {
            let start = text.len();
            text.push_str(SEPARATOR);
            text.push_str(&self.message);
            start..start + SEPARATOR.len()
        });
        LineText {
            text,
            head,
            separator,
        }
    }

    pub fn compact(&self) -> String {
        let detail = if self.reason.is_empty() {
            &self.message
        } else {
            &self.reason
        };
        if detail.is_empty() {
            self.head.clone()
        } else {
            format!("{}{SEPARATOR}{detail}", self.head)
        }
    }
}

const fn problem_rank(status: Status) -> Option<u8> {
    match status {
        Status::Progress => Some(0),
        Status::Err => Some(1),
        Status::Warn => Some(2),
        Status::Ok => None,
        Status::Suspended => None,
        Status::Offline => None,
    }
}

fn problem_order(kind: &str) -> usize {
    PROBLEM_ORDER
        .iter()
        .position(|known| *known == kind)
        .unwrap_or(PROBLEM_ORDER.len())
}

fn one_line(message: &str) -> String {
    let words: Vec<&str> = message.split_whitespace().collect();
    let joined = words.join(SPACE);
    if joined.chars().count() <= MESSAGE_LIMIT {
        return joined;
    }
    let kept: String = joined.chars().take(MESSAGE_LIMIT).collect();
    let mut cut = kept.trim_end().to_owned();
    cut.push(ELLIPSIS);
    cut
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
    }

    #[test]
    fn a_failing_condition_wins_over_healthy_ones() {
        let conditions = [available("True", "MinimumReplicasAvailable"), stuck()];
        let line = ConditionLine::pick(&conditions);
        assert_eq!(
            line.map(|line| line.full().text).as_deref(),
            Some(
                "Progressing=False ProgressDeadlineExceeded · ReplicaSet \"checkout-api-7f9c6d\" has timed out progressing."
            )
        );
    }

    #[test]
    fn progressing_explains_a_stuck_rollout_better_than_available() {
        let conditions = [available("False", "MinimumReplicasUnavailable"), stuck()];
        let line = ConditionLine::pick(&conditions);
        assert_eq!(
            line.map(|line| line.compact()).as_deref(),
            Some("Progressing=False · ProgressDeadlineExceeded")
        );
    }

    #[test]
    fn stalled_comes_before_ready_and_released_before_ready() {
        let ready = Condition::new("Ready", "False").reason("ReconciliationFailed");
        let stalled = Condition::new("Stalled", "True").reason("RetriesExceeded");
        let released = Condition::new("Released", "False").reason("UpgradeFailed");
        let flux = ConditionLine::pick(&[ready.clone(), stalled]).map(|l| l.head);
        let helm = ConditionLine::pick(&[ready, released]).map(|l| l.head);
        assert_eq!(flux.as_deref(), Some("Stalled=True"));
        assert_eq!(helm.as_deref(), Some("Released=False"));
    }

    #[test]
    fn reconciling_hides_the_unknown_ready_condition() {
        let waiting = Condition::new("Ready", "Unknown")
            .reason("Progressing")
            .message("Reconciliation in progress");
        let reconciling = Condition::new("Reconciling", "True")
            .reason("Progressing")
            .message("Reconciliation in progress");
        let line = ConditionLine::pick(&[waiting, reconciling]);
        assert_eq!(line.as_ref().map(|l| l.status), Some(Status::Progress));
        assert_eq!(
            line.map(|line| line.full().text).as_deref(),
            Some("Reconciling=True Progressing · Reconciliation in progress")
        );
    }

    #[test]
    fn healthy_objects_show_ready_or_available() {
        let progressing = Condition::new("Progressing", "True").reason("NewReplicaSetAvailable");
        let deployment = [progressing, available("True", "MinimumReplicasAvailable")];
        let line = ConditionLine::pick(&deployment);
        assert_eq!(
            line.map(|line| line.compact()).as_deref(),
            Some("Available=True · MinimumReplicasAvailable")
        );
        let job = [Condition::new("Complete", "True")];
        let line = ConditionLine::pick(&job).map(|line| line.head);
        assert_eq!(line.as_deref(), Some("Complete=True"));
        assert_eq!(ConditionLine::pick(&[]), None);
    }

    #[test]
    fn full_text_marks_the_head_and_separator() {
        let line = ConditionLine::pick(&[stuck()]);
        let full = line.map(|line| line.full());
        let head = full.as_ref().map(|full| &full.text[full.head.clone()]);
        let separator = full
            .as_ref()
            .and_then(|full| full.separator.clone().map(|range| &full.text[range]));
        assert_eq!(head, Some("Progressing=False"));
        assert_eq!(separator, Some(SEPARATOR));
        let bare = ConditionLine::pick(&[Condition::new("Ready", "True")]).map(|l| l.full());
        assert_eq!(bare.as_ref().map(|b| b.text.as_str()), Some("Ready=True"));
        assert_eq!(bare.and_then(|b| b.separator), None);
    }

    #[test]
    fn messages_become_one_line_and_long_ones_are_cut() {
        let wrapped =
            Condition::new("Ready", "False").message("health check\n  failed\tafter 5m0s");
        let line = ConditionLine::pick(&[wrapped]).map(|line| line.message);
        assert_eq!(line.as_deref(), Some("health check failed after 5m0s"));
        let long = "word ".repeat(MESSAGE_LIMIT);
        let line = ConditionLine::pick(&[Condition::new("Ready", "False").message(long)])
            .map(|line| line.message)
            .unwrap_or_default();
        assert_eq!(line.chars().count(), MESSAGE_LIMIT);
        assert!(line.ends_with("word…"), "{line}");
    }

    #[test]
    fn suspended_objects_name_the_spec_field_and_last_revision() {
        let line = ConditionLine::suspended(Some("7c2d9e0"));
        assert_eq!(line.status, Status::Suspended);
        assert_eq!(line.compact(), "spec.suspend=true · last applied 7c2d9e0");
        assert_eq!(line.full().text, "spec.suspend=true · last applied 7c2d9e0");
        assert_eq!(
            ConditionLine::suspended(None).compact(),
            "spec.suspend=true"
        );
    }
}
