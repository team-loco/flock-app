use std::collections::BTreeMap;

use crate::Status;
use crate::nodes::Node;
use crate::units::{self, CPU_UNIT};

const PRESSURE_WARN: f32 = 0.9;
const PRESSURE_ERR: f32 = 0.95;
const VERSION_PREFIX: char = 'v';
const VERSION_SEPARATOR: char = '.';
const NO_FIT: &str = "no room";
const WHOLE_FROM: f32 = 10.;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fit {
    pub cpu: f32,
    pub memory: f32,
}

impl Fit {
    fn of(node: &Node) -> Self {
        Self {
            cpu: free(node.cpu.allocatable, node.cpu_commitment.requests),
            memory: free(node.memory.allocatable, node.memory_commitment.requests),
        }
    }

    fn room(node: &Node) -> f32 {
        let cpu = 1. - node.cpu_commitment.requests;
        let memory = 1. - node.memory_commitment.requests;
        cpu.min(memory)
    }

    pub fn label(&self) -> String {
        let cpu = units::figure(self.cpu);
        format!("{cpu} {CPU_UNIT} · {} Gi", compact(self.memory))
    }
}

fn compact(value: f32) -> String {
    if value >= WHOLE_FROM {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

fn free(allocatable: f32, requested: f32) -> f32 {
    (allocatable * (1. - requested)).max(0.)
}

pub fn pressure(fraction: f32) -> Option<Status> {
    if fraction >= PRESSURE_ERR {
        Some(Status::Err)
    } else if fraction >= PRESSURE_WARN {
        Some(Status::Warn)
    } else {
        None
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodePool {
    pub name: String,
    pub instance_type: String,
    pub nodes: usize,
    pub cpu: f32,
    pub memory: f32,
    pub fit: Option<Fit>,
}

impl NodePool {
    pub fn of(nodes: &[Node]) -> Vec<Self> {
        let mut groups: BTreeMap<&str, Vec<&Node>> = BTreeMap::new();
        for node in nodes {
            groups.entry(node.pool()).or_default().push(node);
        }
        let mut pools: Vec<Self> = groups
            .into_iter()
            .map(|(name, members)| Self::group(name, &members))
            .collect();
        pools.sort_by(|a, b| b.nodes.cmp(&a.nodes).then_with(|| a.name.cmp(&b.name)));
        pools
    }

    fn group(name: &str, members: &[&Node]) -> Self {
        let requested = |pick: fn(&Node) -> (f32, f32)| {
            let (used, total) = members.iter().fold((0., 0.), |(used, total), node| {
                let (allocatable, requests) = pick(node);
                (used + allocatable * requests, total + allocatable)
            });
            if total > 0. { used / total } else { 0. }
        };
        let fit = members
            .iter()
            .filter(|node| node.schedulable && node.is_ready())
            .max_by(|a, b| Fit::room(a).total_cmp(&Fit::room(b)))
            .map(|node| Fit::of(node));
        Self {
            name: name.to_owned(),
            instance_type: instance_label(members),
            nodes: members.len(),
            cpu: requested(|n| (n.cpu.allocatable, n.cpu_commitment.requests)),
            memory: requested(|n| (n.memory.allocatable, n.memory_commitment.requests)),
            fit,
        }
    }

    pub fn cpu_status(&self) -> Option<Status> {
        pressure(self.cpu)
    }

    pub fn memory_status(&self) -> Option<Status> {
        pressure(self.memory)
    }

    pub fn fit_status(&self) -> Option<Status> {
        if self.fit.is_none() {
            return Some(Status::Err);
        }
        self.cpu_status().max(self.memory_status())
    }

    pub fn fit_label(&self) -> String {
        self.fit
            .map_or_else(|| NO_FIT.to_owned(), |fit| fit.label())
    }
}

fn instance_label(members: &[&Node]) -> String {
    let mut types: BTreeMap<&str, usize> = BTreeMap::new();
    for node in members {
        *types.entry(node.instance_type.as_str()).or_default() += 1;
    }
    let others = types.len().saturating_sub(1);
    let common = types
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(name, _)| name.to_owned())
        .unwrap_or_default();
    if others == 0 {
        common
    } else {
        format!("{common} +{others}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Version {
    pub(crate) major: u32,
    pub(crate) minor: u32,
    patch: Option<u32>,
}

impl Version {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_start_matches(VERSION_PREFIX);
        let mut parts = text.split(VERSION_SEPARATOR);
        let major = parts.next()?.parse().ok()?;
        let minor = leading_number(parts.next()?)?;
        let patch = parts.next().and_then(leading_number);
        Some(Self {
            major,
            minor,
            patch,
        })
    }

    pub(crate) fn minor_label(self) -> String {
        format!("{VERSION_PREFIX}{}.{}", self.major, self.minor)
    }

    fn same_minor(self, other: Self) -> bool {
        (self.major, self.minor) == (other.major, other.minor)
    }
}

fn leading_number(text: &str) -> Option<u32> {
    let end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    text.get(..end)?.parse().ok()
}

pub fn kubelet_skewed(kubelet: &str, control_plane: &str) -> bool {
    let versions = Version::parse(kubelet).zip(Version::parse(control_plane));
    versions.is_some_and(|(kubelet, server)| !kubelet.same_minor(server))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::Usage;
    use crate::nodes::tests::node;
    use crate::overview::Commitment;

    fn pooled(name: &str, pool: &str, instance: &str, requests: (f32, f32)) -> Node {
        let mut node = node(name, instance, true, true);
        node.labels = vec![format!("karpenter.sh/nodepool={pool}")];
        node.cpu = Usage {
            used: 0.,
            allocatable: 8.,
        };
        node.memory = Usage {
            used: 0.,
            allocatable: 32.,
        };
        node.cpu_commitment = Commitment {
            requests: requests.0,
            limits: 0.,
        };
        node.memory_commitment = Commitment {
            requests: requests.1,
            limits: 0.,
        };
        node
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn pools_group_nodes_and_sum_requests_against_allocatable() {
        let nodes = [
            pooled("a", "general", "m7g.4xlarge", (0.5, 0.25)),
            pooled("b", "general", "m7g.4xlarge", (1., 0.75)),
            pooled("c", "general", "m7g.2xlarge", (0.25, 0.5)),
            pooled("d", "gpu", "g5.2xlarge", (0.96, 0.6)),
        ];
        let pools = NodePool::of(&nodes);
        let names: Vec<&str> = pools.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["general", "gpu"]);
        let general = &pools[0];
        assert_eq!(general.nodes, 3);
        assert_eq!(general.instance_type, "m7g.4xlarge +1");
        assert!(close(general.cpu, 0.5833));
        assert!(close(general.memory, 0.5));
        assert_eq!(general.cpu_status(), None);
        let fit = general.fit.unwrap_or_default();
        assert!(close(fit.cpu, 6.));
        assert!(close(fit.memory, 16.));
        assert_eq!(fit.label(), "6.00 cores · 16 Gi");
        let gpu = &pools[1];
        assert_eq!(gpu.instance_type, "g5.2xlarge");
        assert_eq!(gpu.cpu_status(), Some(Status::Err));
        assert_eq!(gpu.fit_status(), Some(Status::Err));
    }

    #[test]
    fn largest_fit_skips_cordoned_and_not_ready_nodes() {
        let mut cordoned = pooled("a", "web", "m6i", (0., 0.));
        cordoned.schedulable = false;
        let mut down = node("b", "m6i", true, false);
        down.labels = vec!["karpenter.sh/nodepool=web".into()];
        let busy = pooled("c", "web", "m6i", (0.9, 0.95));
        let pools = NodePool::of(&[cordoned.clone(), down, busy]);
        let fit = pools[0].fit.unwrap_or_default();
        assert!(close(fit.cpu, 0.8));
        assert_eq!(pools[0].fit_status(), None);
        assert_eq!(pools[0].memory_status(), None);
        let none = NodePool::of(&[cordoned]);
        assert_eq!(none[0].fit, None);
        assert_eq!(none[0].fit_label(), "no room");
        assert_eq!(none[0].fit_status(), Some(Status::Err));
    }

    #[test]
    fn pressure_warns_at_ninety_and_fails_at_ninety_five_percent() {
        assert_eq!(pressure(0.89), None);
        assert_eq!(pressure(0.9), Some(Status::Warn));
        assert_eq!(pressure(0.95), Some(Status::Err));
    }

    #[test]
    fn a_kubelet_is_skewed_only_on_another_minor() {
        assert!(kubelet_skewed("v1.30.4-eks-a737599", "v1.31.2"));
        assert!(!kubelet_skewed("v1.31.0", "v1.31.2-eks-7f9249a"));
        assert!(!kubelet_skewed("", "v1.31.2"));
    }
}
