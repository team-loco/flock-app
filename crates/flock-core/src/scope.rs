use crate::overview::plural;
use crate::switcher::ALL_CLUSTERS;
use crate::{ClusterId, ClusterSummary};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    All,
    One(ClusterId),
    Several(Vec<ClusterId>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Off,
    Mixed,
    On,
}

impl Scope {
    pub fn of(selected: &[ClusterId], all: &[ClusterId]) -> Self {
        let kept: Vec<ClusterId> = all
            .iter()
            .filter(|id| selected.contains(id))
            .cloned()
            .collect();
        match kept.as_slice() {
            [] => Self::All,
            _ if kept.len() == all.len() => Self::All,
            [only] => Self::One(only.clone()),
            _ => Self::Several(kept),
        }
    }

    pub const fn cluster(&self) -> Option<&ClusterId> {
        match self {
            Self::One(id) => Some(id),
            Self::All => None,
            Self::Several(_) => None,
        }
    }

    pub fn restriction(&self) -> Option<&[ClusterId]> {
        match self {
            Self::Several(ids) => Some(ids),
            Self::All => None,
            Self::One(_) => None,
        }
    }

    pub fn contains(&self, id: &ClusterId) -> bool {
        match self {
            Self::All => true,
            Self::One(only) => only == id,
            Self::Several(ids) => ids.contains(id),
        }
    }

    pub fn selected(&self, all: &[ClusterId]) -> Vec<ClusterId> {
        match self {
            Self::All => all.to_vec(),
            Self::One(id) => vec![id.clone()],
            Self::Several(ids) => {
                let present: Vec<ClusterId> =
                    all.iter().filter(|id| ids.contains(id)).cloned().collect();
                if present.is_empty() {
                    all.to_vec()
                } else {
                    present
                }
            }
        }
    }

    pub fn toggle(&self, id: &ClusterId, all: &[ClusterId]) -> Self {
        let mut selected = self.selected(all);
        if let Some(at) = selected.iter().position(|chosen| chosen == id) {
            if selected.len() == 1 {
                return self.clone();
            }
            selected.remove(at);
        } else if all.contains(id) {
            selected.push(id.clone());
        } else {
            return self.clone();
        }
        Self::of(&selected, all)
    }

    pub fn toggle_all(&self, all: &[ClusterId]) -> Self {
        match (self, all.first()) {
            (Self::All, Some(first)) => Self::One(first.clone()),
            (Self::All, None) => Self::All,
            (Self::One(_), _) => Self::All,
            (Self::Several(_), _) => Self::All,
        }
    }

    pub const fn all_check(&self) -> Check {
        match self {
            Self::All => Check::On,
            Self::One(_) => Check::Mixed,
            Self::Several(_) => Check::Mixed,
        }
    }

    pub fn check(&self, id: &ClusterId) -> Check {
        if self.contains(id) {
            Check::On
        } else {
            Check::Off
        }
    }

    pub fn tally(&self, all: &[ClusterId]) -> String {
        format!("{} of {}", self.selected(all).len(), all.len())
    }

    pub fn name(&self, clusters: &[ClusterSummary]) -> String {
        match self {
            Self::All => ALL_CLUSTERS.to_owned(),
            Self::One(id) => name_of(id, clusters),
            Self::Several(ids) => plural(ids.len(), "cluster", "clusters"),
        }
    }

    pub fn trigger(&self, clusters: &[ClusterSummary]) -> (String, Option<String>) {
        let all = ids(clusters);
        let selected = self.selected(&all);
        match (self, selected.first()) {
            (Self::All, _) => (ALL_CLUSTERS.to_owned(), None),
            (Self::One(id), _) => (name_of(id, clusters), None),
            (Self::Several(_), None) => (ALL_CLUSTERS.to_owned(), None),
            (Self::Several(_), Some(first)) => {
                let more = (selected.len() > 1).then(|| format!("+{}", selected.len() - 1));
                (name_of(first, clusters), more)
            }
        }
    }
}

pub fn ids(clusters: &[ClusterSummary]) -> Vec<ClusterId> {
    clusters.iter().map(|cluster| cluster.id.clone()).collect()
}

fn name_of(id: &ClusterId, clusters: &[ClusterSummary]) -> String {
    clusters
        .iter()
        .find(|cluster| &cluster.id == id)
        .map_or_else(|| id.to_string(), |cluster| cluster.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flock::tests::sample;

    fn id(name: &str) -> ClusterId {
        ClusterId::new(name)
    }

    fn all() -> Vec<ClusterId> {
        vec![id("a"), id("b"), id("c")]
    }

    #[test]
    fn a_selection_normalises_to_all_one_or_several_in_cluster_order() {
        assert_eq!(Scope::of(&[], &all()), Scope::All);
        assert_eq!(Scope::of(&[id("c"), id("a"), id("b")], &all()), Scope::All);
        assert_eq!(Scope::of(&[id("b")], &all()), Scope::One(id("b")));
        assert_eq!(
            Scope::of(&[id("c"), id("a"), id("gone")], &all()),
            Scope::Several(vec![id("a"), id("c")])
        );
    }

    #[test]
    fn toggling_adds_or_removes_one_cluster() {
        let two = Scope::All.toggle(&id("b"), &all());
        assert_eq!(two, Scope::Several(vec![id("a"), id("c")]));
        let one = two.toggle(&id("a"), &all());
        assert_eq!(one, Scope::One(id("c")));
        assert_eq!(
            one.toggle(&id("b"), &all()),
            Scope::Several(vec![id("b"), id("c")])
        );
        assert_eq!(two.toggle(&id("b"), &all()), Scope::All);
    }

    #[test]
    fn the_last_selected_cluster_cannot_be_deselected() {
        let one = Scope::One(id("b"));
        assert_eq!(one.toggle(&id("b"), &all()), one);
        let only = [id("a")];
        assert_eq!(Scope::All.toggle(&id("a"), &only), Scope::All);
    }

    #[test]
    fn toggling_an_unknown_cluster_changes_nothing() {
        let one = Scope::One(id("b"));
        assert_eq!(one.toggle(&id("gone"), &all()), one);
    }

    #[test]
    fn all_clusters_selects_all_or_when_all_are_selected_only_the_first() {
        assert_eq!(Scope::All.toggle_all(&all()), Scope::One(id("a")));
        assert_eq!(Scope::One(id("b")).toggle_all(&all()), Scope::All);
        let several = Scope::Several(vec![id("a"), id("b")]);
        assert_eq!(several.toggle_all(&all()), Scope::All);
        assert_eq!(Scope::All.toggle_all(&[]), Scope::All);
    }

    #[test]
    fn the_all_row_is_checked_for_all_and_mixed_otherwise() {
        assert_eq!(Scope::All.all_check(), Check::On);
        assert_eq!(Scope::One(id("a")).all_check(), Check::Mixed);
        let several = Scope::Several(vec![id("a"), id("b")]);
        assert_eq!(several.all_check(), Check::Mixed);
        assert_eq!(several.check(&id("b")), Check::On);
        assert_eq!(several.check(&id("c")), Check::Off);
        assert_eq!(Scope::All.check(&id("c")), Check::On);
        assert_eq!(several.tally(&all()), "2 of 3");
        assert_eq!(Scope::All.tally(&all()), "3 of 3");
    }

    #[test]
    fn clusters_that_left_drop_out_of_the_selection() {
        let several = Scope::Several(vec![id("b"), id("gone")]);
        assert_eq!(several.selected(&all()), [id("b")]);
        let left = Scope::Several(vec![id("gone"), id("away")]);
        assert_eq!(left.selected(&all()), all());
    }

    #[test]
    fn the_scope_is_named_for_headers_and_the_status_bar() {
        let clusters = sample().clusters;
        assert_eq!(Scope::All.name(&clusters), "All clusters");
        assert_eq!(Scope::One(id("b")).name(&clusters), "b");
        let several = Scope::Several(vec![id("a"), id("c")]);
        assert_eq!(several.name(&clusters), "2 clusters");
        assert_eq!(Scope::All.trigger(&clusters), ("All clusters".into(), None));
        assert_eq!(Scope::One(id("b")).trigger(&clusters), ("b".into(), None));
        assert_eq!(
            several.trigger(&clusters),
            ("a".into(), Some("+1".to_owned()))
        );
    }
}
