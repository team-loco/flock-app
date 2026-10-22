use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::time::Duration;

use crate::events::{self, Event};
use crate::filter::{FilterSpec, TextFilter};
use crate::namespace::NamespaceDetail;
use crate::nodes::{Node, NodeSummary};
use crate::overview::{Capacity, Overview, WorkloadHealth};
use crate::related::Related;
use crate::resources::{Resource, ResourceKind};
use crate::units::{self, Amount, CPU_UNIT, MEMORY_UNIT, WAITING_FOR_METRICS};
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
    pub cpu_requests: f32,
    pub memory_requests: f32,
    pub restarts: u32,
    pub age: Duration,
}

impl Namespace {
    pub const FILTER: FilterSpec<Self> = FilterSpec {
        fields: &[
            ("name", |n, (), out| out.push(n.name.as_str().into())),
            ("phase", |n, (), out| out.push(n.phase.as_str().into())),
        ],
        namespace: false,
        labels: true,
    };

    pub fn matches(&self, query: &TextFilter) -> bool {
        let name_ok = query.matches_namespace(Some(&self.name));
        let rest = TextFilter {
            namespace: None,
            ..query.clone()
        };
        name_ok && Self::FILTER.matches(self, &rest, Some(&self.name), &[&self.labels])
    }

    pub fn cpu_label(&self) -> Amount {
        self.cpu.map_or_else(Amount::missing, |cpu| {
            Amount::of(units::figure(cpu), CPU_UNIT)
        })
    }

    pub fn memory_label(&self) -> Amount {
        self.memory.map_or_else(Amount::missing, |memory| {
            Amount::of(units::figure(memory), MEMORY_UNIT)
        })
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

    pub fn cpu_share(&self) -> Option<f32> {
        share(self.cpu, self.cpu_requests)
    }

    pub fn memory_share(&self) -> Option<f32> {
        share(self.memory, self.memory_requests)
    }

    pub fn cpu_against_requests(&self) -> Amount {
        Amount::of(units::against(self.cpu, self.cpu_requests), CPU_UNIT)
    }

    pub fn memory_against_requests(&self) -> Amount {
        Amount::of(
            units::against(self.memory, self.memory_requests),
            MEMORY_UNIT,
        )
    }
}

fn share(used: Option<f32>, requested: f32) -> Option<f32> {
    used.filter(|_| requested > 0.).map(|used| used / requested)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NamespaceSort {
    Name,
    Pods,
    Failing,
    #[default]
    Cpu,
    Memory,
    Restarts,
}

impl NamespaceSort {
    pub const ALL: [Self; 6] = [
        Self::Name,
        Self::Pods,
        Self::Failing,
        Self::Cpu,
        Self::Memory,
        Self::Restarts,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Name => "Namespace",
            Self::Pods => "Pods",
            Self::Failing => "Failing",
            Self::Cpu => "CPU used / requested",
            Self::Memory => "Memory used / requested",
            Self::Restarts => "Restarts",
        }
    }

    pub const fn ascending(self) -> bool {
        matches!(self, Self::Name)
    }

    fn compare(self, a: &Namespace, b: &Namespace) -> Ordering {
        let shares = |a: Option<f32>, b: Option<f32>| {
            let rank = |share: Option<f32>| share.filter(|s| s.is_finite()).unwrap_or(f32::MIN);
            rank(b).total_cmp(&rank(a))
        };
        let ordering = match self {
            Self::Name => Ordering::Equal,
            Self::Pods => b.pods.cmp(&a.pods),
            Self::Failing => b.problems.err.cmp(&a.problems.err),
            Self::Cpu => shares(a.cpu_share(), b.cpu_share()),
            Self::Memory => shares(a.memory_share(), b.memory_share()),
            Self::Restarts => b.restarts.cmp(&a.restarts),
        };
        ordering.then_with(|| a.name.cmp(&b.name))
    }

    pub fn top(self, namespaces: &[Namespace], limit: usize) -> Vec<Namespace> {
        let mut sorted: Vec<&Namespace> = namespaces.iter().collect();
        sorted.sort_by(|a, b| self.compare(a, b));
        sorted.into_iter().take(limit).cloned().collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailObject {
    pub target: ResourceRef,
    pub row: Resource,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClusterDetail {
    pub overview: Overview,
    pub nodes: Vec<Node>,
    pub namespaces: Vec<Namespace>,
    pub events: Vec<Event>,
    pub resources: BTreeMap<ResourceKind, Vec<Resource>>,
    pub related: Vec<Related>,
    pub namespace_details: Vec<NamespaceDetail>,
    pub objects: Vec<DetailObject>,
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

    pub fn namespace_detail(&self, name: &str) -> Option<&NamespaceDetail> {
        self.namespace_details
            .iter()
            .find(|detail| detail.name == name)
    }

    pub fn object(&self, target: &ResourceRef) -> Option<&Resource> {
        self.objects
            .iter()
            .find(|object| &object.target == target)
            .map(|object| &object.row)
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
        assert_eq!(ns.cpu_label().labelled(), "—");
        assert_eq!(ns.memory_label().labelled(), "—");
        assert_eq!(ns.cpu_note(), "Waiting for metrics");
        ns.cpu = Some(0.026);
        ns.memory = Some(0.25);
        assert_eq!(ns.cpu_label().labelled(), "0.03 cores");
        assert_eq!(ns.memory_label().labelled(), "0.25 GiB");
        assert_eq!(
            ns.memory_note(),
            "Memory in use by the pods in this namespace"
        );
    }

    #[test]
    fn namespace_label_tokens_match_its_metadata() {
        let mut namespace = namespace("payments", Counts::default());
        namespace.labels.insert("team".into(), "checkout".into());
        assert!(namespace.matches(&TextFilter::parse("pay label:team=checkout")));
        assert!(!namespace.matches(&TextFilter::parse("label:team=platform")));
    }

    #[test]
    fn namespace_filter_matches_its_name() {
        let ns = namespace("payments", Counts::default());
        assert!(ns.matches(&TextFilter::parse("pay")));
        assert!(!ns.matches(&TextFilter::parse("orders")));
        assert!(ns.matches(&TextFilter::parse("ns:pay")));
        assert!(!ns.matches(&TextFilter::parse("ns:orders")));
        assert!(ns.matches(&TextFilter::parse("ns:pay active")));
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
            cpu_requests: 0.,
            memory_requests: 0.,
            restarts: 0,
            age: Duration::ZERO,
        }
    }

    fn loaded(name: &str, pods: u32, cpu: (f32, f32), restarts: u32) -> Namespace {
        Namespace {
            pods,
            cpu: Some(cpu.0),
            cpu_requests: cpu.1,
            restarts,
            ..namespace(name, Counts::default().with_err(pods / 10))
        }
    }

    #[test]
    fn namespaces_sort_by_each_column_and_keep_the_top_rows() {
        let rows = [
            loaded("web", 410, (30.1, 125.0), 0),
            loaded("search", 312, (61.2, 43.1), 96),
            loaded("ml", 188, (88.4, 74.9), 18),
            loaded("idle", 3, (0.0, 0.0), 0),
        ];
        let names = |sort: NamespaceSort, limit: usize| -> Vec<String> {
            sort.top(&rows, limit).into_iter().map(|n| n.name).collect()
        };
        assert_eq!(names(NamespaceSort::default(), 3), ["search", "ml", "web"]);
        assert_eq!(names(NamespaceSort::Name, 2), ["idle", "ml"]);
        assert_eq!(names(NamespaceSort::Pods, 1), ["web"]);
        assert_eq!(names(NamespaceSort::Failing, 2), ["web", "search"]);
        assert_eq!(names(NamespaceSort::Restarts, 2), ["search", "ml"]);
        assert_eq!(
            names(NamespaceSort::Memory, 4),
            ["idle", "ml", "search", "web"]
        );
        assert!(NamespaceSort::Name.ascending());
        assert!(!NamespaceSort::Cpu.ascending());
    }

    #[test]
    fn namespace_usage_reads_against_its_requests() {
        let search = loaded("search", 312, (61.2, 43.1), 96);
        assert_eq!(
            search.cpu_against_requests().labelled(),
            "61.20 / 43.10 cores"
        );
        assert_eq!(search.cpu_share().map(|s| (s * 100.).round()), Some(142.));
        let unmeasured = namespace("new", Counts::default());
        assert_eq!(unmeasured.memory_against_requests(), Amount::missing());
        assert_eq!(unmeasured.memory_share(), None);
    }

    #[test]
    fn small_namespace_usage_reads_in_cores() {
        let small = loaded("loco", 2, (0.004, 0.002), 0);
        assert_eq!(small.cpu_against_requests().labelled(), "0.00 / 0.00 cores");
        let busy = loaded("web", 2, (0.028, 0.35), 0);
        assert_eq!(busy.cpu_against_requests().labelled(), "0.03 / 0.35 cores");
        assert_eq!(small.cpu_share().map(|s| (s * 100.).round()), Some(200.));
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
        assert_eq!(detail.cpu().amount().labelled(), "4.00 / 16.00 cores");
        assert_eq!(detail.memory().amount().labelled(), "0.00 / 0.00 GiB");
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
        assert_eq!(health[0].ok_label, "1 available");
        assert_eq!(health[1].ok_label, "0 ready");
    }
}
