use std::time::Duration;

use crate::filter::{FilterSpec, TextFilter};
use crate::pods::PodEvent;
use crate::{ClusterId, Counts, ResourceRef, Status};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterMetrics {
    pub pods: Counts,
    pub cpu_percent: Option<u8>,
    pub memory_percent: Option<u8>,
    pub flux: Counts,
    pub revision: Revision,
}

impl ClusterMetrics {
    pub fn flux_ready(&self) -> u32 {
        let total = self.flux.total.unwrap_or_default();
        let not_ready = self.flux.err + self.flux.warn + self.flux.sus;
        total.saturating_sub(not_ready)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revision {
    pub sha: String,
    pub held: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub status: Status,
    pub target: ResourceRef,
    pub reason: String,
    pub message: String,
    pub condition: String,
    pub age: Duration,
    pub events: Vec<PodEvent>,
}

impl Problem {
    pub const FILTER: FilterSpec<Self> = FilterSpec {
        fields: &[
            ("reason", |p, (), out| out.push(p.reason.as_str().into())),
            ("object", |p, (), out| {
                out.push(p.target.kind.as_str().into());
                out.push(p.target.to_string().into());
            }),
        ],
        facets: &[
            ("reason", |p, (), out| out.push(p.reason.as_str().into())),
            ("kind", |p, (), out| out.push(p.target.kind.as_str().into())),
        ],
        namespace: true,
        labels: false,
    };

    pub fn matches(&self, query: &TextFilter) -> bool {
        Self::FILTER.matches(self, query, self.target.namespace.as_deref(), &[])
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Severity {
    #[default]
    All,
    Errors,
    Warnings,
    Suspended,
}

impl Severity {
    pub const ALL: [Self; 4] = [Self::All, Self::Errors, Self::Warnings, Self::Suspended];
    pub const OVERVIEW: [Self; 3] = [Self::All, Self::Errors, Self::Warnings];

    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Errors => "Errors",
            Self::Warnings => "Warnings",
            Self::Suspended => "Suspended",
        }
    }

    pub fn keeps(self, status: Status) -> bool {
        match self {
            Self::All => true,
            Self::Errors => status == Status::Err,
            Self::Warnings => status == Status::Warn,
            Self::Suspended => status == Status::Suspended,
        }
    }
}

pub fn severity_counts(problems: &[Problem], query: &TextFilter) -> Vec<(Severity, usize)> {
    let matching: Vec<&Problem> = problems.iter().filter(|p| p.matches(query)).collect();
    Severity::ALL
        .into_iter()
        .map(|severity| {
            let count = matching.iter().filter(|p| severity.keeps(p.status)).count();
            (severity, count)
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProblemGroup<'a> {
    pub cluster: &'a ClusterId,
    pub problems: Vec<&'a Problem>,
}

impl ProblemGroup<'_> {
    pub fn counts(&self) -> Counts {
        problem_counts(self.problems.iter().copied())
    }
}

pub fn problem_counts<'a>(problems: impl IntoIterator<Item = &'a Problem>) -> Counts {
    problems
        .into_iter()
        .fold(Counts::default(), |counts, problem| match problem.status {
            Status::Err => counts.with_err(counts.err + 1),
            Status::Warn => counts.with_warn(counts.warn + 1),
            Status::Suspended => counts.with_sus(counts.sus + 1),
            Status::Ok => counts,
            Status::Progress => counts,
            Status::Offline => counts,
        })
}

pub fn problems_badge(problems: &[Problem]) -> Counts {
    let counts = problem_counts(problems);
    Counts::default()
        .with_err(counts.err)
        .with_warn(counts.warn)
}

pub fn group_problems<'a>(
    problems: &'a [Problem],
    severity: Severity,
    query: &TextFilter,
) -> Vec<ProblemGroup<'a>> {
    let mut groups: Vec<ProblemGroup<'a>> = Vec::new();
    let kept = problems
        .iter()
        .filter(|p| severity.keeps(p.status) && p.matches(query));
    for problem in kept {
        let cluster = &problem.target.cluster;
        match groups.iter_mut().find(|g| g.cluster == cluster) {
            Some(group) => group.problems.push(problem),
            None => groups.push(ProblemGroup {
                cluster,
                problems: vec![problem],
            }),
        }
    }
    groups
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FlockHome {
    pub problems: Vec<Problem>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem(cluster: &str, status: Status) -> Problem {
        Problem {
            status,
            target: ResourceRef {
                cluster: ClusterId::new(cluster),
                kind: "Pod".into(),
                namespace: Some("ns".into()),
                name: "p".into(),
            },
            reason: "BackOff".into(),
            message: String::new(),
            condition: String::new(),
            age: Duration::ZERO,
            events: Vec::new(),
        }
    }

    fn all() -> TextFilter {
        TextFilter::default()
    }

    fn sample() -> Vec<Problem> {
        vec![
            problem("a", Status::Err),
            problem("b", Status::Suspended),
            problem("a", Status::Warn),
            problem("c", Status::Err),
        ]
    }

    fn shape(groups: &[ProblemGroup<'_>]) -> Vec<(String, usize)> {
        groups
            .iter()
            .map(|g| (g.cluster.to_string(), g.problems.len()))
            .collect()
    }

    #[test]
    fn groups_keep_first_appearance_order() {
        let problems = sample();
        let groups = group_problems(&problems, Severity::All, &all());
        assert_eq!(
            shape(&groups),
            [("a".into(), 2), ("b".into(), 1), ("c".into(), 1)]
        );
        assert_eq!(
            groups[0].counts(),
            Counts::default().with_err(1).with_warn(1)
        );
    }

    #[test]
    fn errors_filter_drops_groups_without_errors() {
        let problems = sample();
        let groups = group_problems(&problems, Severity::Errors, &all());
        assert_eq!(shape(&groups), [("a".into(), 1), ("c".into(), 1)]);
    }

    #[test]
    fn warnings_and_suspended_filters_keep_only_their_status() {
        let problems = sample();
        let warnings = group_problems(&problems, Severity::Warnings, &all());
        assert_eq!(shape(&warnings), [("a".into(), 1)]);
        let suspended = group_problems(&problems, Severity::Suspended, &all());
        assert_eq!(shape(&suspended), [("b".into(), 1)]);
        assert_eq!(suspended[0].counts(), Counts::default().with_sus(1));
    }

    #[test]
    fn search_matches_reason_object_and_namespace_and_narrows_the_counts() {
        let mut problems = sample();
        problems[1].reason = "Suspended".into();
        problems[1].target.kind = "Kustomization".into();
        problems[1].target.namespace = Some("flux-system".into());
        problems[1].target.name = "apps".into();
        let counts = |text: &str| -> Vec<usize> {
            severity_counts(&problems, &TextFilter::parse(text))
                .into_iter()
                .map(|(_, n)| n)
                .collect()
        };
        assert_eq!(counts(""), [4, 2, 1, 1]);
        assert_eq!(counts("suspend"), [1, 0, 0, 1]);
        assert_eq!(counts("flux-system/apps"), [1, 0, 0, 1]);
        assert_eq!(counts("kustomization"), [1, 0, 0, 1]);
        assert_eq!(counts("backoff"), [3, 2, 1, 0]);
        let found = group_problems(&problems, Severity::All, &TextFilter::parse("apps"));
        assert_eq!(shape(&found), [("b".into(), 1)]);
    }

    #[test]
    fn counts_tally_each_severity() {
        let counts = problem_counts(&sample());
        assert_eq!(
            counts,
            Counts::default().with_err(2).with_warn(1).with_sus(1)
        );
    }

    #[test]
    fn badge_counts_errors_and_warnings_only() {
        assert_eq!(
            problems_badge(&sample()),
            Counts::default().with_err(2).with_warn(1)
        );
    }

    #[test]
    fn flux_ready_subtracts_flagged_objects() {
        let metrics = ClusterMetrics {
            pods: Counts::total(1),
            cpu_percent: None,
            memory_percent: None,
            flux: Counts::total(21).with_err(1),
            revision: Revision {
                sha: "a41e07c".into(),
                held: false,
            },
        };
        assert_eq!(metrics.flux_ready(), 20);
    }
}
