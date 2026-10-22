use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use crate::diagnosis::{self, Condition, Diagnosis};
use crate::filter::{FilterSpec, TextFilter};
use crate::issue::{Issue, named};
use crate::overview::{Commitment, percent};
use crate::related::NODE;
use crate::units::figure;
use crate::{Status, Uid};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    pub used: f32,
    pub allocatable: f32,
}

impl Usage {
    pub fn fraction(&self) -> f32 {
        if self.allocatable > 0. {
            self.used / self.allocatable
        } else {
            0.
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NodePod {
    pub name: String,
    pub namespace: String,
    pub cpu: f32,
    pub memory: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub name: String,
    pub uid: Uid,
    pub status: Status,
    pub state: String,
    pub schedulable: bool,
    pub instance_type: String,
    pub zone: String,
    pub version: String,
    pub internal_ip: String,
    pub os_image: String,
    pub runtime: String,
    pub cpu: Usage,
    pub memory: Usage,
    pub disk: Option<Usage>,
    pub cpu_commitment: Commitment,
    pub memory_commitment: Commitment,
    pub pods: u32,
    pub pod_capacity: u32,
    pub age: Duration,
    pub conditions: Vec<Condition>,
    pub taints: Vec<String>,
    pub labels: Vec<String>,
    pub top_pods: Vec<NodePod>,
}

impl Node {
    pub const FILTER: FilterSpec<Self> = FilterSpec {
        fields: &[
            ("name", |n, (), out| out.push(n.name.as_str().into())),
            ("status", |n, (), out| out.push(n.state.as_str().into())),
            ("instance type", |n, (), out| {
                out.push(n.instance_type.as_str().into());
            }),
            ("zone", |n, (), out| out.push(n.zone.as_str().into())),
            ("version", |n, (), out| out.push(n.version.as_str().into())),
            ("IP", |n, (), out| out.push(n.internal_ip.as_str().into())),
            ("label", |n, (), out| {
                out.extend(n.labels.iter().map(|l| l.as_str().into()));
            }),
        ],
        namespace: false,
        labels: true,
    };

    pub fn matches(&self, query: &TextFilter) -> bool {
        let labels = self.label_map();
        Self::FILTER.matches(self, query, None, &[&labels])
    }

    pub fn label_map(&self) -> BTreeMap<String, String> {
        self.labels
            .iter()
            .filter_map(|label| label.split_once('='))
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect()
    }

    pub fn diagnosis(&self) -> Option<Diagnosis> {
        if !diagnosis::needs_diagnosis(self.status) {
            return None;
        }
        diagnosis::node(&self.conditions).or_else(|| {
            (!self.schedulable).then(|| Diagnosis::new(SCHEDULING_DISABLED, CORDONED_MESSAGE))
        })
    }

    pub fn issue(&self) -> Option<Issue> {
        let diagnosis = self.diagnosis()?;
        let object = named(NODE, &self.name, None);
        Some(Issue::from_diagnosis(self.status, &diagnosis, object))
    }

    pub fn is_ready(&self) -> bool {
        self.has_condition(READY_CONDITION)
    }

    pub fn has_disk_pressure(&self) -> bool {
        self.has_condition(DISK_PRESSURE_CONDITION)
    }

    pub fn has_pressure(&self) -> bool {
        PRESSURE_CONDITIONS
            .iter()
            .any(|kind| self.has_condition(kind))
    }

    pub fn conditions_not_ok(&self) -> usize {
        self.conditions
            .iter()
            .filter(|c| {
                let healthy = if c.kind == READY_CONDITION {
                    CONDITION_TRUE
                } else {
                    CONDITION_FALSE
                };
                c.value != healthy
            })
            .count()
    }

    fn has_condition(&self, kind: &str) -> bool {
        self.conditions
            .iter()
            .any(|c| c.kind == kind && c.value == CONDITION_TRUE)
    }

    pub fn label(&self, key: &str) -> Option<&str> {
        self.labels.iter().find_map(|label| {
            let (name, value) = label.split_once('=')?;
            (name == key).then_some(value)
        })
    }

    pub fn pool(&self) -> &str {
        POOL_LABELS
            .iter()
            .find_map(|key| self.label(key))
            .filter(|pool| !pool.is_empty())
            .unwrap_or(&self.instance_type)
    }
}

pub const READY_CONDITION: &str = "Ready";
pub const CONDITION_TRUE: &str = "True";
pub const TOP_PODS: usize = 5;
const CONDITION_FALSE: &str = "False";
const DISK_PRESSURE_CONDITION: &str = "DiskPressure";
const PRESSURE_CONDITIONS: [&str; 3] = ["MemoryPressure", DISK_PRESSURE_CONDITION, "PIDPressure"];
const READY_STATE: &str = "Ready";
const NOT_READY_STATE: &str = "NotReady";
const STATE_SEPARATOR: char = ',';
const POOL_LABELS: [&str; 5] = [
    "eks.amazonaws.com/nodegroup",
    "cloud.google.com/gke-nodepool",
    "kubernetes.azure.com/agentpool",
    "agentpool",
    "karpenter.sh/nodepool",
];
const SCHEDULING_DISABLED: &str = "SchedulingDisabled";
const CORDONED_MESSAGE: &str = "Cordoned: no new pods are scheduled here.";

pub fn state_label(ready: bool, schedulable: bool) -> String {
    let readiness = if ready { READY_STATE } else { NOT_READY_STATE };
    if schedulable {
        readiness.to_owned()
    } else {
        format!("{readiness}{STATE_SEPARATOR}{SCHEDULING_DISABLED}")
    }
}

pub fn request_line(allocatable: f32, commitment: Commitment) -> String {
    format!(
        "requested {} ({}) · limits {} ({})",
        figure(commitment.requests * allocatable),
        percent(commitment.requests),
        figure(commitment.limits * allocatable),
        percent(commitment.limits)
    )
}

pub fn pool_counts(nodes: &[Node]) -> Vec<(String, usize)> {
    let mut pools: BTreeMap<&str, usize> = BTreeMap::new();
    for node in nodes {
        *pools.entry(node.pool()).or_default() += 1;
    }
    pools
        .into_iter()
        .map(|(pool, count)| (pool.to_owned(), count))
        .collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NodeFilter {
    #[default]
    All,
    Ready,
    NotReady,
    Cordoned,
    Pressure,
}

impl NodeFilter {
    pub const ALL: [Self; 5] = [
        Self::All,
        Self::Ready,
        Self::NotReady,
        Self::Cordoned,
        Self::Pressure,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Ready => READY_STATE,
            Self::NotReady => NOT_READY_STATE,
            Self::Cordoned => "Cordoned",
            Self::Pressure => "Pressure",
        }
    }

    pub fn at(ix: usize) -> Self {
        Self::ALL.get(ix).copied().unwrap_or_default()
    }

    pub fn keeps(self, node: &Node) -> bool {
        match self {
            Self::All => true,
            Self::Ready => node.is_ready() && node.schedulable,
            Self::NotReady => !node.is_ready(),
            Self::Cordoned => !node.schedulable,
            Self::Pressure => node.has_pressure(),
        }
    }

    pub fn counts(nodes: &[&Node]) -> Vec<(Self, usize)> {
        Self::ALL
            .iter()
            .map(|filter| {
                let count = nodes.iter().filter(|n| filter.keeps(n)).count();
                (*filter, count)
            })
            .collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NodeSummary {
    pub total: usize,
    pub ready: usize,
    pub cordoned: usize,
    pub not_ready: usize,
    pub disk_pressure: usize,
    pub pools: usize,
    pub pod_capacity: u32,
}

impl NodeSummary {
    pub fn of(nodes: &[Node]) -> Self {
        let count = |filter: NodeFilter| nodes.iter().filter(|n| filter.keeps(n)).count();
        let pools: BTreeSet<&str> = nodes.iter().map(Node::pool).collect();
        Self {
            total: nodes.len(),
            ready: count(NodeFilter::Ready),
            cordoned: count(NodeFilter::Cordoned),
            not_ready: count(NodeFilter::NotReady),
            disk_pressure: nodes.iter().filter(|n| n.has_disk_pressure()).count(),
            pools: pools.len(),
            pod_capacity: nodes.iter().map(|n| n.pod_capacity).sum(),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn node(name: &str, instance: &str, schedulable: bool, ready: bool) -> Node {
        let value = if ready { CONDITION_TRUE } else { "False" };
        Node {
            uid: Uid::default(),
            name: name.into(),
            status: Status::Ok,
            state: String::new(),
            schedulable,
            instance_type: instance.into(),
            zone: String::new(),
            version: String::new(),
            internal_ip: String::new(),
            os_image: String::new(),
            runtime: String::new(),
            cpu: Usage::default(),
            memory: Usage::default(),
            disk: None,
            cpu_commitment: Commitment::default(),
            memory_commitment: Commitment::default(),
            pods: 0,
            pod_capacity: 110,
            age: Duration::ZERO,
            conditions: vec![Condition::new(READY_CONDITION, value)],
            taints: Vec::new(),
            labels: Vec::new(),
            top_pods: Vec::new(),
        }
    }

    #[test]
    fn node_label_tokens_use_metadata_keys_and_values() {
        let mut node = node("worker", "standard", true, true);
        node.labels = vec![
            "topology.kubernetes.io/zone=us-west".into(),
            "unstructured".into(),
        ];
        assert!(node.matches(&TextFilter::parse(
            "label:topology.kubernetes.io/zone=us-west"
        )));
        assert!(node.matches(&TextFilter::parse("l:topology.kubernetes.io/zone")));
        assert!(!node.matches(&TextFilter::parse("l:topology.kubernetes.io/zone=us-east")));
        assert_eq!(node.label_map().len(), 1);
    }

    #[test]
    fn unhealthy_nodes_explain_themselves() {
        let mut down = node("a", "m6i", true, false);
        down.status = Status::Err;
        down.conditions[0].message = "Kubelet stopped posting node status.".into();
        assert_eq!(
            down.diagnosis(),
            Some(Diagnosis::new(
                "NotReady",
                "Kubelet stopped posting node status."
            ))
        );
        let mut cordoned = node("b", "m6i", false, true);
        cordoned.status = Status::Warn;
        assert_eq!(
            cordoned.diagnosis().map(|d| d.reason),
            Some("SchedulingDisabled".into())
        );
        assert_eq!(node("c", "m6i", true, true).diagnosis(), None);
    }

    #[test]
    fn node_filter_matches_name_instance_and_zone() {
        let n = node("ip-10-4-31-12", "m6i.xlarge", true, true);
        assert!(n.matches(&TextFilter::parse("31-12")));
        assert!(n.matches(&TextFilter::parse("M6I")));
        assert!(!n.matches(&TextFilter::parse("c7g")));
        assert!(!n.matches(&TextFilter::parse("ns:default")));
    }

    #[test]
    fn summary_counts_ready_cordoned_and_pools() {
        let nodes = [
            node("a", "m6i.4xlarge", true, true),
            node("b", "m6i.4xlarge", false, true),
            node("c", "g5.2xlarge", true, true),
            node("d", "m6i.4xlarge", true, false),
        ];
        let summary = NodeSummary::of(&nodes);
        assert_eq!(summary.total, 4);
        assert_eq!(summary.ready, 2);
        assert_eq!(summary.cordoned, 1);
        assert_eq!(summary.not_ready, 1);
        assert_eq!(summary.pod_capacity, 440);
        assert_eq!(summary.pools, 2);
        assert_eq!(summary.disk_pressure, 0);
    }

    #[test]
    fn pools_come_from_provider_labels_before_the_instance_type() {
        let mut eks = node("a", "m7g.4xlarge", true, true);
        eks.labels = vec!["eks.amazonaws.com/nodegroup=general-arm".into()];
        let mut gke = node("b", "e2-standard-8", true, true);
        gke.labels = vec!["cloud.google.com/gke-nodepool=default-pool".into()];
        let mut aks = node("c", "Standard_D4s_v5", true, true);
        aks.labels = vec!["kubernetes.azure.com/agentpool=nodepool1".into()];
        let mut karpenter = node("d", "c7g.2xlarge", true, true);
        karpenter.labels = vec!["karpenter.sh/nodepool=builds".into()];
        let mut blank = node("e", "g5.2xlarge", true, true);
        blank.labels = vec!["eks.amazonaws.com/nodegroup=".into()];
        let pools: Vec<&str> = [&eks, &gke, &aks, &karpenter, &blank]
            .map(Node::pool)
            .into();
        assert_eq!(
            pools,
            [
                "general-arm",
                "default-pool",
                "nodepool1",
                "builds",
                "g5.2xlarge"
            ]
        );
    }

    #[test]
    fn summary_counts_disk_pressure() {
        let mut full = node("a", "m6i", true, true);
        full.conditions
            .push(Condition::new(DISK_PRESSURE_CONDITION, CONDITION_TRUE));
        let summary = NodeSummary::of(&[full, node("b", "m6i", true, true)]);
        assert_eq!(summary.disk_pressure, 1);
        assert_eq!(summary.pools, 1);
    }

    #[test]
    fn pool_counts_group_nodes_by_pool_in_name_order() {
        let mut gpu = node("a", "g5.2xlarge", true, true);
        gpu.labels = vec!["eks.amazonaws.com/nodegroup=gpu".into()];
        let mut general = node("b", "m6i", true, true);
        general.labels = vec!["eks.amazonaws.com/nodegroup=general".into()];
        let counts = pool_counts(&[gpu, general.clone(), general]);
        assert_eq!(counts, [("general".into(), 2), ("gpu".into(), 1)]);
    }

    #[test]
    fn state_reads_like_kubectl() {
        assert_eq!(state_label(true, true), "Ready");
        assert_eq!(state_label(false, true), "NotReady");
        assert_eq!(state_label(true, false), "Ready,SchedulingDisabled");
        assert_eq!(state_label(false, false), "NotReady,SchedulingDisabled");
    }

    #[test]
    fn filters_count_ready_not_ready_cordoned_and_pressure() {
        let mut pressured = node("a", "m6i", false, true);
        pressured
            .conditions
            .push(Condition::new("MemoryPressure", CONDITION_TRUE));
        let nodes = [
            pressured,
            node("b", "m6i", true, true),
            node("c", "m6i", true, false),
        ];
        let refs: Vec<&Node> = nodes.iter().collect();
        let counts: Vec<(&str, usize)> = NodeFilter::counts(&refs)
            .into_iter()
            .map(|(filter, n)| (filter.label(), n))
            .collect();
        assert_eq!(
            counts,
            [
                ("All", 3),
                ("Ready", 1),
                ("NotReady", 1),
                ("Cordoned", 1),
                ("Pressure", 1)
            ]
        );
        assert_eq!(NodeFilter::at(4), NodeFilter::Pressure);
        assert_eq!(NodeFilter::at(9), NodeFilter::All);
    }

    #[test]
    fn conditions_not_ok_counts_false_ready_and_true_pressure() {
        let mut n = node("a", "m6i", true, true);
        n.conditions
            .push(Condition::new(DISK_PRESSURE_CONDITION, CONDITION_FALSE));
        assert_eq!(n.conditions_not_ok(), 0);
        n.conditions
            .push(Condition::new("PIDPressure", CONDITION_TRUE));
        assert_eq!(n.conditions_not_ok(), 1);
        assert_eq!(node("b", "m6i", true, false).conditions_not_ok(), 1);
    }

    #[test]
    fn request_line_shows_amounts_and_shares_of_allocatable() {
        let commitment = Commitment {
            requests: 0.89,
            limits: 1.38,
        };
        assert_eq!(
            request_line(15.89, commitment),
            "requested 14.14 (89%) · limits 21.93 (138%)"
        );
    }

    #[test]
    fn usage_fraction_guards_zero_allocatable() {
        assert!(
            (Usage {
                used: 2.,
                allocatable: 8.
            }
            .fraction()
                - 0.25)
                .abs()
                < f32::EPSILON
        );
        assert!(Usage::default().fraction().abs() < f32::EPSILON);
    }
}
