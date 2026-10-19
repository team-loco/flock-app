use std::collections::BTreeMap;

use crate::filter::TextFilter;
use std::time::Duration;

use crate::diagnosis::{self, Condition, Diagnosis};
use crate::{ClusterId, Counts, ResourceRef, Status, Uid, units};

pub const POD_KIND: &str = "Pod";
const RESTARTS_ERR: u32 = 5;
const SHORT_NAME_MIN: usize = 4;
const SEPARATOR: &str = " · ";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PodPhase {
    #[default]
    Pending,
    Running,
    Succeeded,
    Failed,
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Qos {
    Guaranteed,
    #[default]
    Burstable,
    BestEffort,
}

impl Qos {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Guaranteed => "Guaranteed",
            Self::Burstable => "Burstable",
            Self::BestEffort => "BestEffort",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ContainerState {
    Running,
    #[default]
    Waiting,
    Terminated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerPort {
    pub port: u16,
    pub protocol: String,
    pub name: Option<String>,
}

impl ContainerPort {
    pub fn label(&self) -> String {
        match &self.name {
            Some(name) => format!("{}/{} {name}", self.port, self.protocol),
            None => format!("{}/{}", self.port, self.protocol),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Container {
    pub name: String,
    pub image: String,
    pub state: ContainerState,
    pub status: Status,
    pub reason: String,
    pub ready: bool,
    pub restarts: u32,
    pub ports: Vec<ContainerPort>,
    pub note: Option<String>,
    pub message: String,
}

impl Container {
    fn started(&self) -> bool {
        self.state != ContainerState::Waiting || self.restarts > 0
    }

    pub fn meta(&self) -> String {
        if !self.started() {
            return self.note.clone().unwrap_or_default();
        }
        let restarts = format!("restarts {}", self.restarts);
        let ports = self.ports.iter().map(|p| format!(":{}", p.port));
        std::iter::once(restarts)
            .chain(self.note.clone())
            .chain(ports)
            .collect::<Vec<_>>()
            .join(SEPARATOR)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub cpu_millis: u32,
    pub memory_mib: u32,
}

impl Usage {
    pub fn cpu_label(self) -> String {
        units::cpu_millis(self.cpu_millis)
    }

    pub fn memory_label(self) -> String {
        units::memory_mib(self.memory_mib)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    pub name: String,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PodEvent {
    pub status: Status,
    pub reason: String,
    pub message: String,
    pub count: u32,
    pub age: Duration,
}

impl From<&crate::events::Event> for PodEvent {
    fn from(event: &crate::events::Event) -> Self {
        Self {
            status: event.status,
            reason: event.reason.clone(),
            message: event.message.clone(),
            count: event.count,
            age: event.age,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pod {
    pub cluster: ClusterId,
    pub namespace: String,
    pub name: String,
    pub uid: Uid,
    pub phase: PodPhase,
    pub status: Status,
    pub reason: String,
    pub node: Option<String>,
    pub ip: Option<String>,
    pub owner: Option<ResourceRef>,
    pub qos: Qos,
    pub age: Duration,
    pub labels: BTreeMap<String, String>,
    pub containers: Vec<Container>,
    pub usage: Option<Usage>,
    pub conditions: Vec<Condition>,
    pub volumes: Vec<Volume>,
    pub events: Vec<PodEvent>,
}

impl Pod {
    pub fn resource(&self) -> ResourceRef {
        ResourceRef {
            cluster: self.cluster.clone(),
            kind: POD_KIND.to_owned(),
            namespace: Some(self.namespace.clone()),
            name: self.name.clone(),
        }
    }

    pub fn is(&self, resource: &ResourceRef) -> bool {
        resource.kind == POD_KIND
            && resource.cluster == self.cluster
            && resource.name == self.name
            && resource.namespace.as_deref() == Some(self.namespace.as_str())
    }

    pub fn ready_label(&self) -> String {
        let ready = self.containers.iter().filter(|c| c.ready).count();
        format!("{ready}/{}", self.containers.len())
    }

    pub fn restarts(&self) -> u32 {
        self.containers.iter().map(|c| c.restarts).sum()
    }

    pub fn restart_status(&self) -> Option<Status> {
        match self.restarts() {
            0 => None,
            n if n > RESTARTS_ERR => Some(Status::Err),
            _ => Some(Status::Warn),
        }
    }

    pub fn short_name(&self) -> &str {
        match self.name.rsplit_once('-') {
            Some((_, suffix)) if suffix.len() >= SHORT_NAME_MIN => suffix,
            Some(_) => &self.name,
            None => &self.name,
        }
    }

    pub fn node_short(&self) -> Option<&str> {
        let node = self.node.as_deref()?;
        Some(node.split_once('.').map_or(node, |(host, _)| host))
    }

    pub fn ports(&self) -> impl Iterator<Item = (&Container, &ContainerPort)> {
        self.containers
            .iter()
            .flat_map(|c| c.ports.iter().map(move |p| (c, p)))
    }

    pub fn diagnosis(&self) -> Option<Diagnosis> {
        if !diagnosis::needs_diagnosis(self.status) {
            return None;
        }
        let failing = |status: Status| {
            self.containers
                .iter()
                .find(|c| c.status == status)
                .map(|c| Diagnosis::new(c.reason.clone(), c.message.clone()))
        };
        failing(Status::Err)
            .or_else(|| diagnosis::from_conditions(&self.conditions))
            .or_else(|| failing(Status::Warn))
            .or_else(|| Some(Diagnosis::new(self.reason.clone(), String::new())))
    }

    pub fn sentence(&self) -> String {
        if self.status == Status::Ok {
            let node = self.node_short().unwrap_or_default();
            return format!("Running and ready on {node}.");
        }
        let worst = self
            .containers
            .iter()
            .filter(|c| c.status != Status::Ok)
            .max_by_key(|c| c.status);
        match (self.status, worst) {
            (Status::Progress, _) => format!("{}.", self.reason),
            (_, Some(container)) => container_sentence(container),
            (_, None) => format!("{}.", self.reason),
        }
    }
}

fn container_sentence(container: &Container) -> String {
    let name = &container.name;
    let note = container.note.as_deref().unwrap_or(&container.reason);
    match container.state {
        ContainerState::Running => format!("{name} is running but not ready ({note})."),
        ContainerState::Waiting if container.restarts > 0 => format!(
            "{name} keeps crashing ({note}) and has restarted {} times.",
            container.restarts
        ),
        ContainerState::Waiting => format!("{name} is waiting to start: {note}."),
        ContainerState::Terminated => format!("{name} has terminated ({note})."),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PodFilter {
    #[default]
    All,
    Running,
    Pending,
    Failing,
}

impl PodFilter {
    pub const ALL: [Self; 4] = [Self::All, Self::Running, Self::Pending, Self::Failing];

    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Running => "Running",
            Self::Pending => "Pending",
            Self::Failing => "Failing",
        }
    }

    pub fn keeps(self, pod: &Pod) -> bool {
        match self {
            Self::All => true,
            Self::Running => pod.phase == PodPhase::Running && pod.status != Status::Err,
            Self::Pending => pod.phase == PodPhase::Pending,
            Self::Failing => pod.status == Status::Err,
        }
    }
}

pub fn filter_counts(pods: &[&Pod]) -> Vec<(PodFilter, usize)> {
    PodFilter::ALL
        .iter()
        .map(|filter| {
            let count = pods.iter().filter(|pod| filter.keeps(pod)).count();
            (*filter, count)
        })
        .collect()
}

pub fn nav_counts(pods: &[Pod], cluster: &ClusterId) -> Counts {
    let rows: Vec<&Pod> = pods.iter().filter(|pod| &pod.cluster == cluster).collect();
    let tally = |filter: PodFilter| {
        let matching = rows.iter().filter(|pod| filter.keeps(pod)).count();
        u32::try_from(matching).unwrap_or(u32::MAX)
    };
    Counts::total(tally(PodFilter::All)).with_err(tally(PodFilter::Failing))
}

pub fn matches_query(pod: &Pod, query: &TextFilter) -> bool {
    if query.is_empty() {
        return true;
    }
    let pairs: Vec<String> = pod
        .labels
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    let mut fields: Vec<&str> = vec![
        pod.name.as_str(),
        pod.namespace.as_str(),
        pod.node.as_deref().unwrap_or_default(),
    ];
    fields.extend(pairs.iter().map(String::as_str));
    query.matches_labeled(Some(&pod.namespace), &pod.labels, &fields)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PodQuery {
    pub text: TextFilter,
    pub namespace: Option<String>,
    pub filter: PodFilter,
}

impl PodQuery {
    pub fn base<'a>(&self, pods: &'a [Pod], cluster: &ClusterId) -> Vec<&'a Pod> {
        pods.iter()
            .filter(|pod| &pod.cluster == cluster)
            .filter(|pod| {
                self.namespace
                    .as_ref()
                    .is_none_or(|namespace| &pod.namespace == namespace)
            })
            .filter(|pod| matches_query(pod, &self.text))
            .collect()
    }

    pub fn apply<'a>(&self, pods: &'a [Pod], cluster: &ClusterId) -> Vec<&'a Pod> {
        self.base(pods, cluster)
            .into_iter()
            .filter(|pod| self.filter.keeps(pod))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(port: u16) -> ContainerPort {
        ContainerPort {
            port,
            protocol: "TCP".into(),
            name: None,
        }
    }

    fn container(name: &str, state: ContainerState, status: Status, restarts: u32) -> Container {
        Container {
            name: name.into(),
            image: format!("{name}:1"),
            state,
            status,
            reason: String::new(),
            ready: status == Status::Ok,
            restarts,
            ports: Vec::new(),
            note: None,
            message: String::new(),
        }
    }

    fn crashing() -> Pod {
        let mut api = container("api", ContainerState::Waiting, Status::Err, 14);
        api.note = Some("exit 1".into());
        api.reason = "CrashLoopBackOff".into();
        api.ports = vec![port(8080)];
        let mut envoy = container("envoy", ContainerState::Running, Status::Ok, 0);
        envoy.ports = vec![port(9901)];
        Pod {
            cluster: ClusterId::new("prod"),
            namespace: "payments".into(),
            name: "checkout-api-7f9c6d-x2lqp".into(),
            phase: PodPhase::Running,
            status: Status::Err,
            reason: "CrashLoopBackOff".into(),
            node: Some("ip-10-4-12-87.ec2.internal".into()),
            labels: BTreeMap::from([("app".into(), "checkout-api".into())]),
            containers: vec![api, envoy],
            ..Pod::default()
        }
    }

    fn healthy(name: &str, namespace: &str) -> Pod {
        Pod {
            cluster: ClusterId::new("prod"),
            namespace: namespace.into(),
            name: name.into(),
            phase: PodPhase::Running,
            status: Status::Ok,
            reason: "Running".into(),
            node: Some("ip-10-4-30-5.ec2.internal".into()),
            containers: vec![container("web", ContainerState::Running, Status::Ok, 0)],
            ..Pod::default()
        }
    }

    fn pending() -> Pod {
        let mut trainer = container("trainer", ContainerState::Waiting, Status::Warn, 0);
        trainer.note = Some("unschedulable · nvidia.com/gpu: 1".into());
        Pod {
            cluster: ClusterId::new("prod"),
            namespace: "ml".into(),
            name: "ml-batch-5c7f-9kq2d".into(),
            phase: PodPhase::Pending,
            status: Status::Warn,
            reason: "Pending".into(),
            containers: vec![trainer],
            ..Pod::default()
        }
    }

    #[test]
    fn diagnosis_prefers_failing_containers_then_conditions() {
        let mut pod = crashing();
        pod.containers[0].message = "OOMKilled, exit 137".into();
        assert_eq!(
            pod.diagnosis(),
            Some(Diagnosis::new("CrashLoopBackOff", "OOMKilled, exit 137"))
        );
        let mut waiting = pending();
        waiting.containers[0].reason = "Pending".into();
        waiting.conditions = vec![
            Condition::new("PodScheduled", "False")
                .reason("Unschedulable")
                .message("0/3 nodes are available: insufficient nvidia.com/gpu."),
        ];
        assert_eq!(
            waiting.diagnosis(),
            Some(Diagnosis::new(
                "Unschedulable",
                "0/3 nodes are available: insufficient nvidia.com/gpu."
            ))
        );
        waiting.conditions.clear();
        assert_eq!(
            waiting.diagnosis().map(|d| d.reason),
            Some("Pending".into())
        );
        assert_eq!(healthy("web", "web").diagnosis(), None);
    }

    #[test]
    fn container_meta_matches_the_design() {
        let pod = crashing();
        let metas: Vec<String> = pod.containers.iter().map(Container::meta).collect();
        assert_eq!(
            metas,
            ["restarts 14 · exit 1 · :8080", "restarts 0 · :9901"]
        );
        assert_eq!(
            pending().containers[0].meta(),
            "unschedulable · nvidia.com/gpu: 1"
        );
    }

    #[test]
    fn ready_restarts_and_names_derive_from_containers() {
        let pod = crashing();
        assert_eq!(pod.ready_label(), "1/2");
        assert_eq!(pod.restarts(), 14);
        assert_eq!(pod.restart_status(), Some(Status::Err));
        assert_eq!(pod.short_name(), "x2lqp");
        assert_eq!(pod.node_short(), Some("ip-10-4-12-87"));
        assert_eq!(
            healthy("payments-db-0", "payments").short_name(),
            "payments-db-0"
        );
        assert_eq!(healthy("a", "b").restart_status(), None);
    }

    #[test]
    fn a_few_restarts_warn() {
        let mut pod = healthy("worker", "payments");
        pod.containers[0].restarts = 2;
        assert_eq!(pod.restart_status(), Some(Status::Warn));
    }

    #[test]
    fn sentences_say_what_is_wrong_first() {
        assert_eq!(
            crashing().sentence(),
            "api keeps crashing (exit 1) and has restarted 14 times."
        );
        assert_eq!(
            pending().sentence(),
            "trainer is waiting to start: unschedulable · nvidia.com/gpu: 1."
        );
        assert_eq!(
            healthy("db-0", "payments").sentence(),
            "Running and ready on ip-10-4-30-5."
        );
        let mut terminating = healthy("db-0", "payments");
        terminating.status = Status::Progress;
        terminating.reason = "Terminating".into();
        assert_eq!(terminating.sentence(), "Terminating.");
    }

    #[test]
    fn not_ready_containers_explain_the_probe() {
        let mut pod = healthy("search-indexer-0", "search");
        pod.status = Status::Warn;
        pod.containers[0].status = Status::Warn;
        pod.containers[0].note = Some("readiness 503".into());
        assert_eq!(
            pod.sentence(),
            "web is running but not ready (readiness 503)."
        );
    }

    #[test]
    fn filters_split_running_pending_and_failing() {
        let pods = [crashing(), healthy("a", "web"), pending()];
        let refs: Vec<&Pod> = pods.iter().collect();
        let counts: Vec<usize> = filter_counts(&refs).into_iter().map(|(_, n)| n).collect();
        assert_eq!(counts, [3, 1, 1, 1]);
    }

    #[test]
    fn nav_counts_are_the_all_and_failing_filters_of_one_cluster() {
        let mut other = crashing();
        other.cluster = ClusterId::new("staging");
        let pods = [crashing(), healthy("a", "web"), pending(), other];
        let prod = ClusterId::new("prod");
        assert_eq!(nav_counts(&pods, &prod), Counts::total(3).with_err(1));
    }

    #[test]
    fn query_matches_name_namespace_node_and_labels() {
        let pod = crashing();
        assert!(matches_query(&pod, &TextFilter::parse("")));
        assert!(matches_query(&pod, &TextFilter::parse("X2LQP")));
        assert!(matches_query(&pod, &TextFilter::parse("payments")));
        assert!(matches_query(&pod, &TextFilter::parse("10-4-12-87")));
        assert!(matches_query(&pod, &TextFilter::parse("app=checkout")));
        assert!(!matches_query(&pod, &TextFilter::parse("grafana")));
        assert!(matches_query(
            &pod,
            &TextFilter::parse("l:app=checkout-api ns:pay")
        ));
        assert!(!matches_query(&pod, &TextFilter::parse("l:app=web")));
    }

    #[test]
    fn query_scopes_to_cluster_namespace_and_filter() {
        let mut other = healthy("elsewhere", "web");
        other.cluster = ClusterId::new("staging");
        let pods = vec![crashing(), healthy("a", "web"), pending(), other];
        let prod = ClusterId::new("prod");
        let query = PodQuery {
            namespace: Some("web".into()),
            ..PodQuery::default()
        };
        let names: Vec<&str> = query
            .apply(&pods, &prod)
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(names, ["a"]);
        let failing = PodQuery {
            filter: PodFilter::Failing,
            ..PodQuery::default()
        };
        assert_eq!(failing.apply(&pods, &prod).len(), 1);
        assert_eq!(failing.base(&pods, &prod).len(), 3);
    }

    #[test]
    fn resource_refs_round_trip() {
        let pod = crashing();
        let resource = pod.resource();
        assert!(pod.is(&resource));
        assert_eq!(resource.to_string(), "payments/checkout-api-7f9c6d-x2lqp");
        let mut other = resource;
        other.kind = "Deployment".into();
        assert!(!pod.is(&other));
    }

    #[test]
    fn usage_labels_switch_units() {
        let small = Usage {
            cpu_millis: 212,
            memory_mib: 301,
        };
        assert_eq!(
            (small.cpu_label(), small.memory_label()),
            ("212m".into(), "301 MiB".into())
        );
        let large = Usage {
            cpu_millis: 1500,
            memory_mib: 2048,
        };
        assert_eq!(
            (large.cpu_label(), large.memory_label()),
            ("1.5 cores".into(), "2.0 GiB".into())
        );
    }

    #[test]
    fn port_labels_include_the_name() {
        let mut named = port(8080);
        named.name = Some("http".into());
        assert_eq!(named.label(), "8080/TCP http");
        assert_eq!(port(53).label(), "53/TCP");
    }
}
