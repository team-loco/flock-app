use std::collections::BTreeMap;
use std::time::Duration;

use crate::events::{self, Event, recent_warnings};
use crate::filter::TextFilter;
use crate::nodes::{Node, NodeSummary};
use crate::overview::{Capacity, Overview, WorkloadHealth};
use crate::related::Related;
use crate::resources::{Resource, ResourceKind};
use crate::units::{self, CPU_UNIT, MEMORY_UNIT, NOT_MEASURED, WAITING_FOR_METRICS};
use crate::{Counts, Page, ResourceRef, Status};

const CPU_NOTE: &str = "CPU in use by the pods in this namespace";
const MEMORY_NOTE: &str = "Memory in use by the pods in this namespace";
pub const ACTIVE_PHASE: &str = "Active";

#[derive(Clone, Debug, PartialEq)]
pub struct Namespace {
    pub name: String,
    pub labels: BTreeMap<String, String>,
    pub status: Status,
    pub phase: String,
    pub pods: u32,
    pub problems: Counts,
    pub cpu: Option<f32>,
    pub memory: Option<f32>,
    pub age: Duration,
}

impl Namespace {
    pub fn matches(&self, query: &TextFilter) -> bool {
        query.matches_labeled(
            Some(&self.name),
            &self.labels,
            &[self.name.as_str(), self.phase.as_str()],
        )
    }

    pub fn cpu_label(&self) -> String {
        self.cpu
            .map_or_else(|| NOT_MEASURED.into(), units::cpu_cores)
    }

    pub fn memory_label(&self) -> String {
        self.memory
            .map_or_else(|| NOT_MEASURED.into(), units::memory_gib)
    }

    pub fn cpu_note(&self) -> &'static str {
        self.cpu.map_or(WAITING_FOR_METRICS, |_| CPU_NOTE)
    }

    pub fn memory_note(&self) -> &'static str {
        self.memory.map_or(WAITING_FOR_METRICS, |_| MEMORY_NOTE)
    }

    pub fn phase_note(&self) -> Option<&str> {
        (!self.phase.is_empty() && self.phase != ACTIVE_PHASE).then_some(self.phase.as_str())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClusterDetail {
    pub overview: Overview,
    pub nodes: Vec<Node>,
    pub namespaces: Vec<Namespace>,
    pub events: Vec<Event>,
    pub resources: BTreeMap<ResourceKind, Vec<Resource>>,
    pub related: Vec<Related>,
}

impl ClusterDetail {
    pub fn node_summary(&self) -> NodeSummary {
        NodeSummary::of(&self.nodes)
    }

    pub fn cpu(&self) -> Capacity {
        Capacity {
            used: self.nodes.iter().map(|n| n.cpu.used).sum(),
            total: self.nodes.iter().map(|n| n.cpu.allocatable).sum(),
            unit: CPU_UNIT.into(),
            requests: self.overview.cpu.requests,
            limits: self.overview.cpu.limits,
        }
    }

    pub fn memory(&self) -> Capacity {
        Capacity {
            used: self.nodes.iter().map(|n| n.memory.used).sum(),
            total: self.nodes.iter().map(|n| n.memory.allocatable).sum(),
            unit: MEMORY_UNIT.into(),
            requests: self.overview.memory.requests,
            limits: self.overview.memory.limits,
        }
    }

    pub fn related(&self, target: &ResourceRef) -> Option<&Related> {
        self.related
            .iter()
            .find(|related| &related.target == target)
    }

    pub fn resources(&self, kind: ResourceKind) -> &[Resource] {
        self.resources.get(&kind).map_or(&[], Vec::as_slice)
    }

    pub fn workload_health(&self) -> Vec<WorkloadHealth> {
        ResourceKind::ALL
            .into_iter()
            .filter(|kind| kind.is_workload())
            .map(|kind| WorkloadHealth::of(kind, self.resources(kind)))
            .collect()
    }

    pub fn nav(&self) -> Vec<(Page, Counts)> {
        let resources = self
            .resources
            .iter()
            .map(|(kind, rows)| (kind.page(), Counts::of(rows.iter().map(|r| r.status))));
        std::iter::once((Page::Events, events::counts(&self.events)))
            .chain(resources)
            .collect()
    }

    pub fn warnings(&self) -> usize {
        self.events.iter().filter(|e| e.is_warning()).count()
    }

    pub fn recent_warnings(&self, limit: usize) -> Vec<Event> {
        recent_warnings(&self.events, limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::tests::node;
    use crate::overview::PodPhases;
    use crate::resources::tests::resource;

    fn detail(pods: PodPhases, nodes: Vec<Node>) -> ClusterDetail {
        ClusterDetail {
            overview: Overview {
                pods,
                ..Overview::default()
            },
            nodes,
            ..ClusterDetail::default()
        }
    }

    #[test]
    fn namespace_usage_reads_a_dash_until_metrics_arrive() {
        let mut ns = namespace("payments", Counts::default());
        assert_eq!(ns.cpu_label(), "—");
        assert_eq!(ns.memory_label(), "—");
        assert_eq!(ns.cpu_note(), "Waiting for metrics");
        ns.cpu = Some(0.026);
        ns.memory = Some(0.25);
        assert_eq!(ns.cpu_label(), "26m");
        assert_eq!(ns.memory_label(), "256 MiB");
        assert_eq!(
            ns.memory_note(),
            "Memory in use by the pods in this namespace"
        );
    }

    #[test]
    fn namespace_label_tokens_match_its_metadata() {
        let mut namespace = namespace("payments", Counts::default());
        namespace.labels.insert("team".into(), "checkout".into());
        assert!(namespace.matches(&TextFilter::parse("ns:pay label:team=checkout")));
        assert!(!namespace.matches(&TextFilter::parse("label:team=platform")));
    }

    #[test]
    fn namespace_filter_matches_its_name() {
        let ns = namespace("payments", Counts::default());
        assert!(ns.matches(&TextFilter::parse("pay")));
        assert!(ns.matches(&TextFilter::parse("ns:pay")));
        assert!(!ns.matches(&TextFilter::parse("orders")));
    }

    fn namespace(name: &str, problems: Counts) -> Namespace {
        Namespace {
            labels: BTreeMap::new(),
            name: name.into(),
            status: problems.status(),
            phase: ACTIVE_PHASE.into(),
            pods: 0,
            problems,
            cpu: None,
            memory: None,
            age: Duration::ZERO,
        }
    }

    #[test]
    fn phase_note_hides_active() {
        let mut ns = namespace("old", Counts::default());
        assert_eq!(ns.phase_note(), None);
        ns.phase = "Terminating".into();
        assert_eq!(ns.phase_note(), Some("Terminating"));
    }

    #[test]
    fn nav_counts_events_and_every_resource_kind() {
        let mut detail = ClusterDetail::default();
        detail.resources.insert(
            ResourceKind::Deployment,
            vec![
                resource("a", "x", Status::Err),
                resource("a", "y", Status::Warn),
                resource("a", "z", Status::Ok),
            ],
        );
        let nav = detail.nav();
        assert_eq!(
            nav,
            [
                (Page::Events, Counts::default()),
                (Page::Deployments, Counts::total(3).with_err(1).with_warn(1)),
            ]
        );
    }

    #[test]
    fn capacity_sums_node_usage() {
        let mut a = node("a", "m6i", true, true);
        a.cpu = crate::nodes::Usage {
            used: 1.5,
            allocatable: 8.,
        };
        let mut b = node("b", "m6i", true, true);
        b.cpu = crate::nodes::Usage {
            used: 2.5,
            allocatable: 8.,
        };
        let detail = detail(PodPhases::default(), vec![a, b]);
        assert_eq!(detail.cpu().amount(), "4.0 / 16.0 cores");
        assert_eq!(detail.memory().amount(), "0.0 / 0.0 GiB");
    }

    #[test]
    fn workload_health_covers_workload_kinds_in_nav_order() {
        let mut detail = ClusterDetail::default();
        detail.resources.insert(
            ResourceKind::Deployment,
            vec![resource("a", "x", Status::Ok)],
        );
        detail
            .resources
            .insert(ResourceKind::Service, vec![resource("a", "s", Status::Ok)]);
        let health = detail.workload_health();
        let pages: Vec<_> = health.iter().map(|h| h.page.label()).collect();
        assert_eq!(
            pages,
            [
                "Deployments",
                "StatefulSets",
                "DaemonSets",
                "Jobs",
                "CronJobs"
            ]
        );
        assert_eq!(health[0].summary, "1 / 1 ready");
        assert_eq!(health[1].summary, "0 / 0 ready");
    }
}
