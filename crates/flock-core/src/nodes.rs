use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use crate::diagnosis::{self, Condition, Diagnosis};
use crate::filter::{FilterSpec, TextFilter};
use crate::overview::{Commitment, plural};
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
    pub cpu_commitment: Commitment,
    pub memory_commitment: Commitment,
    pub pods: u32,
    pub pod_capacity: u32,
    pub age: Duration,
    pub conditions: Vec<Condition>,
    pub taints: Vec<String>,
    pub labels: Vec<String>,
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

    pub fn is_ready(&self) -> bool {
        self.has_condition(READY_CONDITION)
    }

    pub fn has_disk_pressure(&self) -> bool {
        self.has_condition(DISK_PRESSURE_CONDITION)
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
const DISK_PRESSURE_CONDITION: &str = "DiskPressure";
const POOL_LABELS: [&str; 5] = [
    "eks.amazonaws.com/nodegroup",
    "cloud.google.com/gke-nodepool",
    "kubernetes.azure.com/agentpool",
    "agentpool",
    "karpenter.sh/nodepool",
];
const SCHEDULING_DISABLED: &str = "SchedulingDisabled";
const CORDONED_MESSAGE: &str = "Cordoned: no new pods are scheduled here.";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NodeSummary {
    pub total: usize,
    pub ready: usize,
    pub cordoned: usize,
    pub not_ready: usize,
    pub disk_pressure: usize,
    pub pools: usize,
    pub instance_types: Vec<(String, usize)>,
    pub pod_capacity: u32,
}

impl NodeSummary {
    pub fn of(nodes: &[Node]) -> Self {
        let mut instance_types: Vec<(String, usize)> = Vec::new();
        for node in nodes {
            match instance_types
                .iter_mut()
                .find(|(name, _)| *name == node.instance_type)
            {
                Some((_, count)) => *count += 1,
                None => instance_types.push((node.instance_type.clone(), 1)),
            }
        }
        let cordoned = nodes.iter().filter(|n| !n.schedulable).count();
        let not_ready = nodes.iter().filter(|n| !n.is_ready()).count();
        let ready = nodes
            .iter()
            .filter(|n| n.schedulable && n.is_ready())
            .count();
        let pools: BTreeSet<&str> = nodes.iter().map(Node::pool).collect();
        Self {
            total: nodes.len(),
            ready,
            cordoned,
            not_ready,
            disk_pressure: nodes.iter().filter(|n| n.has_disk_pressure()).count(),
            pools: pools.len(),
            instance_types,
            pod_capacity: nodes.iter().map(|n| n.pod_capacity).sum(),
        }
    }

    pub fn meta_line(&self) -> String {
        format!(
            "{} · {} ready · {} cordoned · {}",
            plural(self.total, "node", "nodes"),
            self.ready,
            self.cordoned,
            self.instance_label()
        )
    }

    pub fn instance_label(&self) -> String {
        let parts: Vec<String> = self
            .instance_types
            .iter()
            .map(|(name, count)| format!("{count} × {name}"))
            .collect();
        parts.join(" · ")
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
            cpu_commitment: Commitment::default(),
            memory_commitment: Commitment::default(),
            pods: 0,
            pod_capacity: 110,
            age: Duration::ZERO,
            conditions: vec![Condition::new(READY_CONDITION, value)],
            taints: Vec::new(),
            labels: Vec::new(),
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
    fn summary_counts_ready_cordoned_and_groups_instance_types() {
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
        assert_eq!(summary.instance_label(), "3 × m6i.4xlarge · 1 × g5.2xlarge");
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
    fn meta_line_counts_one_node_in_the_singular() {
        let one = NodeSummary::of(&[node("a", "m6i.large", true, true)]);
        assert_eq!(
            one.meta_line(),
            "1 node · 1 ready · 0 cordoned · 1 × m6i.large"
        );
        let two = NodeSummary::of(&[
            node("a", "m6i.large", true, true),
            node("b", "m6i.large", false, true),
        ]);
        assert_eq!(
            two.meta_line(),
            "2 nodes · 1 ready · 1 cordoned · 2 × m6i.large"
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
