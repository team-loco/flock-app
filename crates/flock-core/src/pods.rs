use std::collections::BTreeMap;

use std::time::Duration;

use crate::diagnosis::{self, Condition, Diagnosis};
use crate::filter::{FilterSpec, StatusFilter, TextFilter};
use crate::related::{DEPLOYMENT, REPLICA_SET, deployment_name};
use crate::resource::short_kind;
use crate::{ClusterId, Counts, ResourceRef, Status, Uid, time, units};

pub const POD_KIND: &str = "Pod";
const RESTARTS_ERR: u32 = 5;
const SHORT_NAME_MIN: usize = 4;
const SEPARATOR: &str = " · ";
const ARROW: &str = " → ";
pub const OWNER_ARROW: &str = " ← ";
const FAILING_GLYPH: &str = "✕";
const LINK_ARROW: &str = "→";
const RUNNING_LABEL: &str = "Running";
const WAITING_LABEL: &str = "Waiting";
const TERMINATED_LABEL: &str = "Terminated";

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
pub struct Resources {
    pub cpu: Option<String>,
    pub memory: Option<String>,
}

impl Resources {
    pub fn label(&self) -> Option<String> {
        let cpu = self.cpu.as_ref().map(|cpu| format!("cpu {cpu}"));
        let memory = self.memory.as_ref().map(|memory| format!("mem {memory}"));
        let parts: Vec<String> = cpu.into_iter().chain(memory).collect();
        (!parts.is_empty()).then(|| parts.join(SEPARATOR))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeKind {
    Liveness,
    Readiness,
    Startup,
}

impl ProbeKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Liveness => "live",
            Self::Readiness => "ready",
            Self::Startup => "start",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    pub kind: ProbeKind,
    pub target: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Termination {
    pub reason: String,
    pub exit_code: i32,
    pub started: Option<String>,
    pub finished: Option<String>,
    pub ago: Option<Duration>,
    pub ran: Option<Duration>,
}

impl Termination {
    pub fn detail(&self) -> String {
        let exit = format!("exit {}", self.exit_code);
        let ago = self.ago.map(|ago| format!("{} ago", time::age(ago)));
        std::iter::once(exit)
            .chain(ago)
            .collect::<Vec<_>>()
            .join(SEPARATOR)
    }

    pub fn run(&self) -> Option<String> {
        let (started, finished) = (self.started.as_ref()?, self.finished.as_ref()?);
        let window = format!("{started}{ARROW}{finished}");
        let took = self.ran.map(time::span);
        Some(
            std::iter::once(window)
                .chain(took)
                .collect::<Vec<_>>()
                .join(SEPARATOR),
        )
    }

    pub fn span(&self) -> Option<String> {
        self.run().map(|run| format!("ran {run}"))
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
    pub requests: Resources,
    pub limits: Resources,
    pub probes: Vec<Probe>,
    pub since: Option<Duration>,
    pub terminated: Option<Termination>,
    pub last_termination: Option<Termination>,
}

impl Container {
    pub fn state_label(&self) -> String {
        match (self.state, &self.terminated) {
            (ContainerState::Running, _) => match self.since {
                Some(since) => format!("{RUNNING_LABEL}{SEPARATOR}{}", time::age(since)),
                None => RUNNING_LABEL.to_owned(),
            },
            (ContainerState::Terminated, Some(terminated)) => {
                let reason = Some(terminated.reason.clone()).filter(|r| !r.is_empty());
                let exit = format!("exit {}", terminated.exit_code);
                std::iter::once(TERMINATED_LABEL.to_owned())
                    .chain(reason)
                    .chain(std::iter::once(exit))
                    .collect::<Vec<_>>()
                    .join(SEPARATOR)
            }
            (ContainerState::Terminated, None) => TERMINATED_LABEL.to_owned(),
            (ContainerState::Waiting, _) if self.reason.is_empty() => WAITING_LABEL.to_owned(),
            (ContainerState::Waiting, _) => format!("{WAITING_LABEL}{SEPARATOR}{}", self.reason),
        }
    }

    pub fn restarts_label(&self) -> String {
        match self.restarts {
            1 => "1 restart".to_owned(),
            n => format!("{n} restarts"),
        }
    }

    pub fn probes_label(&self) -> Option<String> {
        let probes: Vec<String> = self
            .probes
            .iter()
            .map(|probe| format!("{} {}", probe.kind.label(), probe.target))
            .collect();
        (!probes.is_empty()).then(|| probes.join(SEPARATOR))
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
    pub service_account: Option<String>,
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
        let (ready, total) = self.ready_count();
        format!("{ready}/{total}")
    }

    pub fn ready_count(&self) -> (usize, usize) {
        let ready = self.containers.iter().filter(|c| c.ready).count();
        (ready, self.containers.len())
    }

    pub fn all_ready(&self) -> bool {
        let (ready, total) = self.ready_count();
        ready == total
    }

    pub fn owner_chain(&self) -> Vec<ResourceRef> {
        let Some(owner) = &self.owner else {
            return Vec::new();
        };
        let deployment = (owner.kind == REPLICA_SET)
            .then(|| deployment_name(&owner.name, &self.labels))
            .flatten()
            .map(|name| ResourceRef {
                kind: DEPLOYMENT.to_owned(),
                name: name.to_owned(),
                ..owner.clone()
            });
        deployment.into_iter().chain([owner.clone()]).collect()
    }

    pub fn owner_label(&self) -> Option<String> {
        self.owner_chain().first().map(owner_segment)
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
}

pub fn owner_segment(owner: &ResourceRef) -> String {
    format!("{}/{}", short_kind(&owner.kind), owner.name)
}

pub fn owner_chain_label(chain: &[ResourceRef]) -> String {
    chain
        .iter()
        .map(owner_segment)
        .collect::<Vec<_>>()
        .join(OWNER_ARROW)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailingElsewhere {
    pub count: usize,
    pub namespace: Option<String>,
}

impl FailingElsewhere {
    pub fn label(&self) -> String {
        let count = self.count;
        format!("{FAILING_GLYPH} {count} failing in other namespaces {LINK_ARROW}")
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PodFilter {
    #[default]
    All,
    Attention,
    Running,
    Pending,
    Failed,
    Succeeded,
}

impl PodFilter {
    pub const ALL: [Self; 6] = [
        Self::All,
        Self::Attention,
        Self::Running,
        Self::Pending,
        Self::Failed,
        Self::Succeeded,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Attention => StatusFilter::Attention.label(),
            Self::Running => "Running",
            Self::Pending => "Pending",
            Self::Failed => "Failed",
            Self::Succeeded => "Succeeded",
        }
    }

    pub fn keeps(self, pod: &Pod) -> bool {
        match self {
            Self::All => true,
            Self::Attention => StatusFilter::Attention.keeps(pod.status),
            Self::Running => pod.phase == PodPhase::Running,
            Self::Pending => pod.phase == PodPhase::Pending,
            Self::Failed => pod.phase == PodPhase::Failed,
            Self::Succeeded => pod.phase == PodPhase::Succeeded,
        }
    }
}

pub fn failing(pod: &Pod) -> bool {
    pod.status == Status::Err
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
    let tally = |keep: &dyn Fn(&Pod) -> bool| {
        let matching = rows.iter().filter(|pod| keep(pod)).count();
        u32::try_from(matching).unwrap_or(u32::MAX)
    };
    Counts::total(tally(&|_| true)).with_err(tally(&failing))
}

impl Pod {
    pub const FILTER: FilterSpec<Self> = FilterSpec {
        fields: &[
            ("name", |p, (), out| out.push(p.name.as_str().into())),
            ("namespace", |p, (), out| {
                out.push(p.namespace.as_str().into());
            }),
            ("node", |p, (), out| {
                out.push(p.node.as_deref().unwrap_or_default().into());
            }),
            ("label", |p, (), out| {
                out.extend(p.labels.iter().map(|(k, v)| format!("{k}={v}").into()));
            }),
        ],
        namespace: true,
        labels: true,
    };
}

pub fn matches_query(pod: &Pod, query: &TextFilter) -> bool {
    Pod::FILTER.matches(pod, query, Some(&pod.namespace), &[&pod.labels])
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

    pub fn failing_elsewhere(&self, pods: &[Pod], cluster: &ClusterId) -> Option<FailingElsewhere> {
        let chosen = self.namespace.as_ref()?;
        let elsewhere: Vec<&str> = pods
            .iter()
            .filter(|pod| &pod.cluster == cluster && &pod.namespace != chosen && failing(pod))
            .map(|pod| pod.namespace.as_str())
            .collect();
        let first = *elsewhere.first()?;
        let namespace = elsewhere
            .iter()
            .all(|namespace| *namespace == first)
            .then(|| first.to_owned());
        Some(FailingElsewhere {
            count: elsewhere.len(),
            namespace,
        })
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
            ..Container::default()
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
    fn filters_split_pods_by_phase() {
        let mut done = healthy("migrate-29031-x8f2k", "web");
        done.phase = PodPhase::Succeeded;
        let mut evicted = healthy("evicted", "web");
        evicted.phase = PodPhase::Failed;
        let mut lost = healthy("lost", "web");
        lost.phase = PodPhase::Unknown;
        let pods = [
            crashing(),
            healthy("a", "web"),
            pending(),
            done,
            evicted,
            lost,
        ];
        let refs: Vec<&Pod> = pods.iter().collect();
        let counts: Vec<(&str, usize)> = filter_counts(&refs)
            .into_iter()
            .map(|(filter, n)| (filter.label(), n))
            .collect();
        assert_eq!(
            counts,
            [
                ("All", 6),
                ("Needs attention", 2),
                ("Running", 2),
                ("Pending", 1),
                ("Failed", 1),
                ("Succeeded", 1)
            ]
        );
    }

    #[test]
    fn owners_name_the_deployment_behind_a_replica_set() {
        let owner = |kind: &str, name: &str| ResourceRef {
            cluster: ClusterId::new("prod"),
            kind: kind.into(),
            namespace: Some("web".into()),
            name: name.into(),
        };
        let mut pod = healthy("podinfo-cbf5d9d89-9clpk", "web");
        assert_eq!(pod.owner_label(), None);
        pod.owner = Some(owner("ReplicaSet", "podinfo-cbf5d9d89"));
        pod.labels = BTreeMap::from([("pod-template-hash".into(), "cbf5d9d89".into())]);
        assert_eq!(pod.owner_label().as_deref(), Some("deploy/podinfo"));
        pod.labels.clear();
        assert_eq!(pod.owner_label().as_deref(), Some("rs/podinfo-cbf5d9d89"));
        pod.owner = Some(owner("StatefulSet", "redis"));
        assert_eq!(pod.owner_label().as_deref(), Some("sts/redis"));
        pod.owner = Some(owner("Job", "migrate-29031"));
        assert_eq!(pod.owner_label().as_deref(), Some("job/migrate-29031"));
    }

    #[test]
    fn owner_chains_lead_from_the_deployment_to_the_replica_set() {
        let mut pod = healthy("podinfo-cbf5d9d89-9clpk", "web");
        assert_eq!(pod.owner_chain(), Vec::<ResourceRef>::new());
        let set = ResourceRef {
            cluster: ClusterId::new("prod"),
            kind: "ReplicaSet".into(),
            namespace: Some("web".into()),
            name: "podinfo-cbf5d9d89".into(),
        };
        pod.owner = Some(set.clone());
        assert_eq!(
            owner_chain_label(&pod.owner_chain()),
            "rs/podinfo-cbf5d9d89"
        );
        pod.labels = BTreeMap::from([("pod-template-hash".into(), "cbf5d9d89".into())]);
        let chain = pod.owner_chain();
        assert_eq!(
            owner_chain_label(&chain),
            "deploy/podinfo ← rs/podinfo-cbf5d9d89"
        );
        let deployment = ResourceRef {
            kind: "Deployment".into(),
            name: "podinfo".into(),
            ..set.clone()
        };
        assert_eq!(chain, [deployment, set]);
    }

    #[test]
    fn failing_pods_in_other_namespaces_point_at_where_they_are() {
        let cluster = ClusterId::new("prod");
        let mut pods = vec![crashing(), healthy("web-1", "web"), pending()];
        let mut query = PodQuery::default();
        assert_eq!(query.failing_elsewhere(&pods, &cluster), None);
        query.namespace = Some("payments".into());
        assert_eq!(query.failing_elsewhere(&pods, &cluster), None);
        query.namespace = Some("web".into());
        let one = query.failing_elsewhere(&pods, &cluster);
        assert_eq!(
            one,
            Some(FailingElsewhere {
                count: 1,
                namespace: Some("payments".into()),
            })
        );
        assert_eq!(
            one.map(|elsewhere| elsewhere.label()).as_deref(),
            Some("✕ 1 failing in other namespaces →")
        );
        let mut ml = pending();
        ml.status = Status::Err;
        pods.push(ml);
        let spread = query.failing_elsewhere(&pods, &cluster);
        assert_eq!(
            spread,
            Some(FailingElsewhere {
                count: 2,
                namespace: None,
            })
        );
        let elsewhere_cluster = ClusterId::new("staging");
        assert_eq!(query.failing_elsewhere(&pods, &elsewhere_cluster), None);
    }

    #[test]
    fn container_cards_read_like_the_design() {
        let mut web = container("podinfod", ContainerState::Running, Status::Ok, 1);
        web.since = Some(Duration::from_hours(15));
        web.requests = Resources {
            cpu: Some("1m".into()),
            memory: Some("16Mi".into()),
        };
        web.limits = Resources {
            cpu: Some("2".into()),
            memory: None,
        };
        web.probes = vec![
            Probe {
                kind: ProbeKind::Liveness,
                target: ":9898/healthz".into(),
            },
            Probe {
                kind: ProbeKind::Readiness,
                target: ":9898/readyz".into(),
            },
        ];
        assert_eq!(web.state_label(), "Running · 15h");
        assert_eq!(web.restarts_label(), "1 restart");
        assert_eq!(web.requests.label().as_deref(), Some("cpu 1m · mem 16Mi"));
        assert_eq!(web.limits.label().as_deref(), Some("cpu 2"));
        assert_eq!(Resources::default().label(), None);
        assert_eq!(
            web.probes_label().as_deref(),
            Some("live :9898/healthz · ready :9898/readyz")
        );
        let mut done = container("migrate", ContainerState::Terminated, Status::Ok, 0);
        done.terminated = Some(Termination {
            reason: "Completed".into(),
            ..Termination::default()
        });
        assert_eq!(done.state_label(), "Terminated · Completed · exit 0");
        assert_eq!(done.restarts_label(), "0 restarts");
        assert_eq!(done.probes_label(), None);
        let mut crash = container("api", ContainerState::Waiting, Status::Err, 14);
        crash.reason = "CrashLoopBackOff".into();
        assert_eq!(crash.state_label(), "Waiting · CrashLoopBackOff");
    }

    #[test]
    fn terminations_say_how_and_when_the_container_stopped() {
        let last = Termination {
            reason: "Error".into(),
            exit_code: 1,
            started: Some("2026-10-19 16:31".into()),
            finished: Some("2026-10-20 02:37".into()),
            ago: Some(Duration::from_hours(15)),
            ran: Some(Duration::from_mins(6 * 60 + 6)),
        };
        assert_eq!(last.detail(), "exit 1 · 15h ago");
        assert_eq!(
            last.span().as_deref(),
            Some("ran 2026-10-19 16:31 → 2026-10-20 02:37 · 6h 6m")
        );
        let unknown = Termination {
            exit_code: 137,
            ..Termination::default()
        };
        assert_eq!(
            last.run().as_deref(),
            Some("2026-10-19 16:31 → 2026-10-20 02:37 · 6h 6m")
        );
        assert_eq!(unknown.detail(), "exit 137");
        assert_eq!(unknown.span(), None);
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
        let pending = PodQuery {
            filter: PodFilter::Pending,
            ..PodQuery::default()
        };
        assert_eq!(pending.apply(&pods, &prod).len(), 1);
        assert_eq!(pending.base(&pods, &prod).len(), 3);
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
