use std::collections::BTreeMap;

use crate::Status;
use crate::nodes::Node;
use crate::overview::plural;

const PRESSURE_WARN: f32 = 0.9;
const PRESSURE_ERR: f32 = 0.95;
const VERSION_PREFIX: char = 'v';
const VERSION_SEPARATOR: char = '.';
const LIST_SEPARATOR: &str = ", ";
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
        format!("{} cpu · {} Gi", compact(self.cpu), compact(self.memory))
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

    fn label(self) -> String {
        match self.patch {
            Some(patch) => format!("{}.{patch}", self.minor_label()),
            None => self.minor_label(),
        }
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KubeletSkew {
    pub control_plane: String,
    pub nodes: usize,
    pub versions: Vec<String>,
    pub pools: Vec<String>,
}

impl KubeletSkew {
    pub fn of(nodes: &[Node], control_plane: &str) -> Option<Self> {
        let server = Version::parse(control_plane)?;
        let skewed: Vec<(&Node, Version)> = nodes
            .iter()
            .filter_map(|node| Some((node, Version::parse(&node.version)?)))
            .filter(|(_, version)| !version.same_minor(server))
            .collect();
        let mut versions: Vec<Version> = skewed.iter().map(|(_, version)| *version).collect();
        versions.sort_unstable();
        versions.dedup();
        let mut pools: Vec<String> = skewed
            .iter()
            .map(|(node, _)| node.pool().to_owned())
            .collect();
        pools.sort_unstable();
        pools.dedup();
        Some(Self {
            control_plane: server.minor_label(),
            nodes: skewed.len(),
            versions: versions.into_iter().map(Version::label).collect(),
            pools,
        })
    }

    pub fn status(&self) -> Status {
        if self.nodes == 0 {
            Status::Ok
        } else {
            Status::Warn
        }
    }

    pub fn detail(&self) -> String {
        if self.nodes == 0 {
            return format!("all kubelets on {}", self.control_plane);
        }
        let nodes = plural(self.nodes, "node", "nodes");
        let pools = if self.pools.len() == 1 {
            "pool"
        } else {
            "pools"
        };
        format!(
            "{nodes} on {} · {pools} {} · control plane {}",
            self.versions.join(LIST_SEPARATOR),
            self.pools.join(LIST_SEPARATOR),
            self.control_plane
        )
    }
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
        assert_eq!(fit.label(), "6.0 cpu · 16 Gi");
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

    fn versioned(name: &str, pool: &str, version: &str) -> Node {
        let mut node = pooled(name, pool, "m6i", (0., 0.));
        node.version = version.into();
        node
    }

    #[test]
    fn kubelet_skew_counts_nodes_off_the_control_plane_minor() {
        let nodes = [
            versioned("a", "general", "v1.31.2-eks-7f9249a"),
            versioned("b", "memory", "v1.30.4-eks-1a2b3c4"),
            versioned("c", "memory", "v1.30.4-eks-1a2b3c4"),
            versioned("d", "gpu", "v1.29.10"),
            versioned("e", "gpu", "unknown"),
        ];
        let skew = KubeletSkew::of(&nodes, "v1.31.2-eks-abcdef0");
        let skew = skew.unwrap_or_default();
        assert_eq!(skew.control_plane, "v1.31");
        assert_eq!(skew.nodes, 3);
        assert_eq!(skew.versions, ["v1.29.10", "v1.30.4"]);
        assert_eq!(skew.pools, ["gpu", "memory"]);
        assert_eq!(skew.status(), Status::Warn);
        assert_eq!(
            skew.detail(),
            "3 nodes on v1.29.10, v1.30.4 · pools gpu, memory · control plane v1.31"
        );
    }

    #[test]
    fn matching_kubelets_and_unknown_servers() {
        let nodes = [versioned("a", "general", "v1.31.0")];
        let skew = KubeletSkew::of(&nodes, "v1.31.2").unwrap_or_default();
        assert_eq!(skew.nodes, 0);
        assert_eq!(skew.status(), Status::Ok);
        assert_eq!(skew.detail(), "all kubelets on v1.31");
        let one = KubeletSkew::of(&[versioned("b", "memory-x86", "v1.30.4")], "v1.31.2")
            .unwrap_or_default();
        assert_eq!(
            one.detail(),
            "1 node on v1.30.4 · pool memory-x86 · control plane v1.31"
        );
        assert_eq!(KubeletSkew::of(&nodes, ""), None);
        assert_eq!(KubeletSkew::of(&nodes, "v1"), None);
    }
}
