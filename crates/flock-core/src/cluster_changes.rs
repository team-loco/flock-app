use std::time::Duration;

use crate::overview::TimeRange;
use crate::{ResourceRef, Status};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChangeKind {
    Rollout,
    Scale,
    Helm,
    Flux,
    Node,
    Crd,
    Namespace,
}

impl ChangeKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Rollout => "rollout",
            Self::Scale => "scale",
            Self::Helm => "helm",
            Self::Flux => "flux",
            Self::Node => "node",
            Self::Crd => "crd",
            Self::Namespace => "ns",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterChange {
    pub age: Duration,
    pub at: String,
    pub kind: ChangeKind,
    pub status: Option<Status>,
    pub target: ResourceRef,
    pub what: String,
    pub by: String,
}

pub fn newest_first(changes: &mut [ClusterChange]) {
    changes.sort_by(|a, b| {
        let order = (a.age, a.kind, &a.target.name, &a.what);
        order.cmp(&(b.age, b.kind, &b.target.name, &b.what))
    });
}

pub fn within(changes: &[ClusterChange], range: TimeRange) -> impl Iterator<Item = &ClusterChange> {
    let window = range.duration();
    changes.iter().filter(move |change| change.age <= window)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClusterId;

    const MINUTE: Duration = Duration::from_secs(60);

    fn change(kind: ChangeKind, name: &str, age: Duration) -> ClusterChange {
        ClusterChange {
            age,
            at: String::new(),
            kind,
            status: None,
            target: ResourceRef {
                cluster: ClusterId::new("prod"),
                kind: "Deployment".into(),
                namespace: Some("apps".into()),
                name: name.into(),
            },
            what: name.into(),
            by: String::new(),
        }
    }

    #[test]
    fn newest_change_comes_first_and_ties_are_stable() {
        let mut changes = vec![
            change(ChangeKind::Node, "old", MINUTE * 90),
            change(ChangeKind::Flux, "b", MINUTE),
            change(ChangeKind::Rollout, "a", MINUTE),
            change(ChangeKind::Rollout, "fresh", Duration::ZERO),
        ];
        newest_first(&mut changes);
        let names: Vec<&str> = changes.iter().map(|c| c.target.name.as_str()).collect();
        assert_eq!(names, ["fresh", "a", "b", "old"]);
    }

    #[test]
    fn range_keeps_only_changes_inside_the_window() {
        let changes = vec![
            change(ChangeKind::Rollout, "recent", MINUTE * 10),
            change(ChangeKind::Rollout, "edge", MINUTE * 60),
            change(ChangeKind::Rollout, "older", MINUTE * 61),
        ];
        let hour: Vec<&str> = within(&changes, TimeRange::Hour)
            .map(|c| c.target.name.as_str())
            .collect();
        assert_eq!(hour, ["recent", "edge"]);
        assert_eq!(within(&changes, TimeRange::Day).count(), 3);
    }
}
