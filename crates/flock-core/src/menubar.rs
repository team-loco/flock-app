use crate::overview::plural;
use crate::{ClusterId, ClusterSummary, Snapshot};

const PAUSE: &str = "Pause notifications";
const RESUME: &str = "Resume notifications";
const BRAND: &str = "Flock";
const LABEL_ONE: &str = "cluster connected";
const LABEL_MANY: &str = "clusters connected";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterItem {
    pub id: ClusterId,
    pub name: String,
    pub available: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuBar {
    pub connected: usize,
    pub clusters: Vec<ClusterItem>,
    pub paused: bool,
}

impl MenuBar {
    pub fn new(snapshot: &Snapshot, paused: bool) -> Self {
        Self {
            connected: snapshot.connected(),
            clusters: snapshot.clusters.iter().map(item).collect(),
            paused,
        }
    }

    pub fn badge(&self) -> String {
        self.connected.to_string()
    }

    pub const fn dimmed(&self) -> bool {
        self.connected == 0
    }

    pub fn accessibility_label(&self) -> String {
        let connected = plural(self.connected, LABEL_ONE, LABEL_MANY);
        format!("{BRAND}, {connected}")
    }

    pub const fn pause_title(&self) -> &'static str {
        if self.paused { RESUME } else { PAUSE }
    }
}

fn item(cluster: &ClusterSummary) -> ClusterItem {
    ClusterItem {
        id: cluster.id.clone(),
        name: cluster.name.clone(),
        available: cluster.is_connected(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flock::tests::reachable;
    use crate::{Counts, Status};
    use std::collections::BTreeMap;

    fn cluster(name: &str, status: Status, problems: Counts) -> ClusterSummary {
        ClusterSummary {
            id: ClusterId::new(name),
            name: name.into(),
            status,
            state: String::new(),
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
                reachable("prod-us-east", Status::Err, none.with_err(2).with_warn(3)),
                reachable("prod-eu-west", Status::Ok, none),
                reachable("staging", Status::Warn, none.with_warn(2)),
                cluster("edge-k3s", Status::Offline, none),
            ],
            ..Snapshot::default()
        }
    }

    #[test]
    fn rows_carry_only_the_name_and_whether_it_is_connected() {
        let menu = MenuBar::new(&design_fleet(), false);
        let rows: Vec<(&str, bool)> = menu
            .clusters
            .iter()
            .map(|c| (c.name.as_str(), c.available))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("prod-us-east", true),
                ("prod-eu-west", true),
                ("staging", true),
                ("edge-k3s", false),
            ]
        );
        assert_eq!(menu.clusters[0].id, ClusterId::new("prod-us-east"));
    }

    #[test]
    fn badge_counts_connected_clusters_whatever_their_health() {
        let menu = MenuBar::new(&design_fleet(), false);
        assert_eq!(menu.badge(), "3");
        assert!(!menu.dimmed());
        assert_eq!(menu.accessibility_label(), "Flock, 3 clusters connected");
    }

    #[test]
    fn badge_reads_zero_and_dims_when_nothing_is_connected() {
        let mut snapshot = design_fleet();
        snapshot.clusters.retain(|c| c.status == Status::Offline);
        let menu = MenuBar::new(&snapshot, false);
        assert_eq!(menu.badge(), "0");
        assert!(menu.dimmed());
        assert_eq!(menu.accessibility_label(), "Flock, 0 clusters connected");
        assert!(MenuBar::new(&Snapshot::default(), false).dimmed());
    }

    #[test]
    fn auth_expired_cluster_is_not_counted_and_its_row_is_dimmed() {
        let mut snapshot = design_fleet();
        snapshot.clusters.push(cluster(
            "qa-gke",
            Status::Err,
            Counts::default().with_err(1),
        ));
        let menu = MenuBar::new(&snapshot, false);
        assert_eq!(menu.badge(), "3");
        let expired = menu.clusters.iter().find(|c| c.name == "qa-gke");
        assert_eq!(expired.map(|c| c.available), Some(false));
    }

    #[test]
    fn one_connected_cluster_reads_singular() {
        let mut snapshot = design_fleet();
        snapshot.clusters.truncate(1);
        let menu = MenuBar::new(&snapshot, false);
        assert_eq!(menu.accessibility_label(), "Flock, 1 cluster connected");
    }

    #[test]
    fn pause_item_toggles_to_resume() {
        let snapshot = Snapshot::default();
        assert_eq!(MenuBar::new(&snapshot, false).pause_title(), PAUSE);
        assert_eq!(MenuBar::new(&snapshot, true).pause_title(), RESUME);
    }
}
