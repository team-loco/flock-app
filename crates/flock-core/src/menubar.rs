use crate::{ClusterId, ClusterSummary, Counts, Snapshot, Status};

const PAUSE: &str = "Pause notifications";
const RESUME: &str = "Resume notifications";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterItem {
    pub id: ClusterId,
    pub name: String,
    pub status: Status,
    pub meta: String,
    pub meta_status: Status,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuBar {
    pub badge: String,
    pub dimmed: bool,
    pub clusters: Vec<ClusterItem>,
    pub paused: bool,
}

impl MenuBar {
    pub fn new(snapshot: &Snapshot, paused: bool) -> Self {
        let clusters = &snapshot.clusters;
        let counts = clusters
            .iter()
            .map(cluster_counts)
            .fold(Counts::default(), |sum, c| sum + c);
        let badge = if counts.err > 0 {
            format!("{}{}", Status::Err.glyph(), counts.err)
        } else if counts.warn > 0 {
            format!("{}{}", Status::Warn.glyph(), counts.warn)
        } else {
            String::new()
        };
        Self {
            badge,
            dimmed: !clusters.iter().any(ClusterSummary::is_connected),
            clusters: clusters.iter().map(item).collect(),
            paused,
        }
    }

    pub const fn pause_title(&self) -> &'static str {
        if self.paused { RESUME } else { PAUSE }
    }
}

fn cluster_counts(cluster: &ClusterSummary) -> Counts {
    let problems = cluster.problems;
    let err = match cluster.status {
        Status::Err => problems.err.max(1),
        _ => problems.err,
    };
    let warn = match cluster.status {
        Status::Warn => problems.warn.max(1),
        _ => problems.warn,
    };
    Counts::default().with_err(err).with_warn(warn)
}

fn item(cluster: &ClusterSummary) -> ClusterItem {
    let badges = cluster.problems.badges();
    let (meta, meta_status) = if !badges.is_empty() {
        let parts: Vec<String> = badges
            .iter()
            .map(|(status, n)| format!("{}{n}", status.glyph()))
            .collect();
        (parts.join(" "), cluster.problems.status())
    } else if cluster.status == Status::Ok {
        (String::new(), Status::Ok)
    } else {
        (cluster.state.to_lowercase(), cluster.status)
    };
    ClusterItem {
        id: cluster.id.clone(),
        name: cluster.name.clone(),
        status: cluster.status,
        meta,
        meta_status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn cluster(name: &str, status: Status, state: &str, problems: Counts) -> ClusterSummary {
        ClusterSummary {
            id: ClusterId::new(name),
            name: name.into(),
            status,
            state: state.into(),
            distro: String::new(),
            region: String::new(),
            version: None,
            nodes: None,
            problems,
            nav: BTreeMap::default(),
            metrics: None,
        }
    }

    fn design_fleet() -> Snapshot {
        let none = Counts::default();
        Snapshot {
            clusters: vec![
                cluster(
                    "prod-us-east",
                    Status::Err,
                    "Errors",
                    none.with_err(2).with_warn(3),
                ),
                cluster("prod-eu-west", Status::Ok, "Healthy", none),
                cluster("staging", Status::Warn, "Warnings", none.with_warn(2)),
                cluster("qa-gke", Status::Err, "Auth expired", none),
                cluster("edge-k3s", Status::Offline, "Offline", none),
            ],
            ..Snapshot::default()
        }
    }

    fn metas(menu: &MenuBar) -> Vec<(&str, &str, Status)> {
        menu.clusters
            .iter()
            .map(|c| (c.name.as_str(), c.meta.as_str(), c.meta_status))
            .collect()
    }

    #[test]
    fn clusters_list_in_order_with_problem_counts_or_their_state() {
        let menu = MenuBar::new(&design_fleet(), false);
        assert_eq!(
            metas(&menu),
            vec![
                ("prod-us-east", "✕2 ▲3", Status::Err),
                ("prod-eu-west", "", Status::Ok),
                ("staging", "▲2", Status::Warn),
                ("qa-gke", "auth expired", Status::Err),
                ("edge-k3s", "offline", Status::Offline),
            ]
        );
        assert_eq!(menu.clusters[4].status, Status::Offline);
        assert_eq!(menu.clusters[0].id, ClusterId::new("prod-us-east"));
    }

    #[test]
    fn badge_counts_errors_first_and_a_failing_cluster_counts_once() {
        let mut snapshot = design_fleet();
        assert_eq!(MenuBar::new(&snapshot, false).badge, "✕3");
        snapshot.clusters.retain(|c| c.status != Status::Err);
        assert_eq!(MenuBar::new(&snapshot, false).badge, "▲2");
        snapshot.clusters.retain(|c| c.status != Status::Warn);
        let calm = MenuBar::new(&snapshot, false);
        assert_eq!(calm.badge, "");
        assert!(!calm.dimmed);
    }

    #[test]
    fn icon_dims_when_no_cluster_is_reachable() {
        let mut snapshot = design_fleet();
        snapshot.clusters.retain(|c| c.status == Status::Offline);
        let menu = MenuBar::new(&snapshot, false);
        assert!(menu.dimmed);
        assert_eq!(menu.badge, "");
        assert!(MenuBar::new(&Snapshot::default(), false).dimmed);
    }

    #[test]
    fn pause_item_toggles_to_resume() {
        let snapshot = Snapshot::default();
        assert_eq!(MenuBar::new(&snapshot, false).pause_title(), PAUSE);
        assert_eq!(MenuBar::new(&snapshot, true).pause_title(), RESUME);
    }
}
