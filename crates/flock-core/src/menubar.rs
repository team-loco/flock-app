use crate::{ResourceRef, Snapshot, Status};

const PROBLEM_LIMIT: usize = 10;
const REASON_LIMIT: usize = 160;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProblemItem {
    pub title: String,
    pub target: ResourceRef,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuBar {
    pub label: String,
    pub problems: Vec<ProblemItem>,
}

impl MenuBar {
    pub fn from_snapshot(snapshot: &Snapshot) -> Self {
        let problems = &snapshot.home.problems;
        let counts = crate::home::problems_badge(problems);
        let statuses = snapshot.clusters.iter().map(|cluster| cluster.status);
        let statuses = statuses.chain(problems.iter().map(|problem| problem.status));
        let worst = Status::worst(statuses);
        let mut labels = Vec::new();
        if counts.err > 0 {
            labels.push(format!("{} {}", Status::Err.glyph(), counts.err));
        }
        if counts.warn > 0 {
            labels.push(format!("{} {}", Status::Warn.glyph(), counts.warn));
        }
        if worst > counts.status() || labels.is_empty() {
            labels.insert(0, worst.glyph().to_owned());
        }
        let mut sorted: Vec<_> = problems.iter().collect();
        sorted.sort_by_key(|problem| std::cmp::Reverse(problem.status));
        let problems = sorted
            .into_iter()
            .take(PROBLEM_LIMIT)
            .map(|problem| {
                let target = &problem.target;
                let cluster = snapshot
                    .cluster(&target.cluster)
                    .map_or_else(|| target.cluster.as_str(), |cluster| cluster.name.as_str());
                let reason = problem
                    .message
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                let mut shortened: String = reason.chars().take(REASON_LIMIT).collect();
                if reason.chars().count() > REASON_LIMIT {
                    shortened.push('…');
                }
                let namespace = target
                    .namespace
                    .as_ref()
                    .map(|ns| format!(" ({ns})"))
                    .unwrap_or_default();
                ProblemItem {
                    title: format!(
                        "{cluster} · {}/{}{namespace} · {shortened}",
                        target.kind, target.name
                    ),
                    target: target.clone(),
                }
            })
            .collect();
        Self {
            label: labels.join(" "),
            problems,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClusterId, home::Problem};
    use std::{collections::BTreeMap, time::Duration};

    fn problem(status: Status, name: &str) -> Problem {
        Problem {
            status,
            target: ResourceRef {
                cluster: ClusterId::new("production"),
                kind: "Pod".into(),
                namespace: Some("default".into()),
                name: name.into(),
            },
            message: "Back-off\n restarting\tcontainer".into(),
            age: Duration::ZERO,
        }
    }

    #[test]
    fn healthy_snapshot_has_a_calm_label_and_no_problems() {
        let menu = MenuBar::from_snapshot(&Snapshot::default());
        assert_eq!(menu.label, "●");
        assert_eq!(menu.problems, Vec::<ProblemItem>::new());
    }

    #[test]
    fn counts_cover_all_problems_while_menu_is_bounded_and_errors_lead() {
        let mut snapshot = Snapshot::default();
        snapshot.home.problems = vec![problem(Status::Warn, "warning"); PROBLEM_LIMIT];
        let error = problem(Status::Err, "broken");
        snapshot.home.problems.push(error.clone());
        let menu = MenuBar::from_snapshot(&snapshot);
        assert_eq!(menu.label, format!("✕ 1 ▲ {PROBLEM_LIMIT}"));
        assert_eq!(menu.problems.len(), PROBLEM_LIMIT);
        assert_eq!(menu.problems[0].target, error.target);
        assert_eq!(
            menu.problems[0].title,
            "production · Pod/broken (default) · Back-off restarting container"
        );
        snapshot.home.problems.clear();
        assert_eq!(MenuBar::from_snapshot(&snapshot).label, "●");
    }

    #[test]
    fn cluster_health_and_display_names_come_from_the_same_snapshot() {
        let mut snapshot = Snapshot::default();
        snapshot.clusters.push(crate::ClusterSummary {
            id: ClusterId::new("production"),
            name: "Production Europe".into(),
            status: Status::Offline,
            state: "offline".into(),
            distro: String::new(),
            region: String::new(),
            version: None,
            nodes: None,
            problems: crate::Counts::default(),
            nav: BTreeMap::default(),
            metrics: None,
        });
        assert_eq!(MenuBar::from_snapshot(&snapshot).label, "○");
        snapshot
            .home
            .problems
            .push(problem(Status::Warn, "warning"));
        let menu = MenuBar::from_snapshot(&snapshot);
        assert_eq!(menu.label, "○ ▲ 1");
        assert!(
            menu.problems[0]
                .title
                .starts_with("Production Europe · Pod/warning")
        );
    }

    #[test]
    fn non_error_states_and_long_unicode_reasons_are_preserved_safely() {
        let mut snapshot = Snapshot::default();
        let mut offline = problem(Status::Offline, "offline");
        offline.message = "界".repeat(REASON_LIMIT + 1);
        snapshot.home.problems = vec![offline];
        let menu = MenuBar::from_snapshot(&snapshot);
        assert_eq!(menu.label, "○");
        assert!(
            menu.problems[0]
                .title
                .ends_with(&format!("{}…", "界".repeat(REASON_LIMIT)))
        );
    }
}
