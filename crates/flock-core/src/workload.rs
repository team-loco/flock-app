use std::time::Duration;

use crate::condition_line::ConditionLine;
use crate::diagnosis::Condition;
use crate::{ResourceRef, Status, Uid, time, units};

const CRASH_LOOP: &str = "CrashLoopBackOff";
pub const PROGRESS_DEADLINE: &str = "ProgressDeadlineExceeded";
const NONE: &str = "—";
const FIELD_SEPARATOR: &str = " · ";
const LIMIT_SEPARATOR: &str = " / ";
const NUMBER_WORDS: [&str; 10] = [
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkloadKind {
    Deployment,
    StatefulSet,
    DaemonSet,
}

impl WorkloadKind {
    pub const fn kind(self) -> &'static str {
        match self {
            Self::Deployment => "Deployment",
            Self::StatefulSet => "StatefulSet",
            Self::DaemonSet => "DaemonSet",
        }
    }

    pub fn from_kind(kind: &str) -> Option<Self> {
        match kind {
            "Deployment" => Some(Self::Deployment),
            "StatefulSet" => Some(Self::StatefulSet),
            "DaemonSet" => Some(Self::DaemonSet),
            _ => None,
        }
    }

    pub const fn scalable(self) -> bool {
        match self {
            Self::Deployment => true,
            Self::StatefulSet => true,
            Self::DaemonSet => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub label: &'static str,
    pub value: String,
    pub status: Option<Status>,
}

impl Field {
    fn new(label: &'static str, value: String) -> Self {
        Self {
            label,
            value,
            status: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pending {
    Restarting,
    Scaling { replicas: u32 },
    RollingBack { revision: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workload {
    pub target: ResourceRef,
    pub uid: Uid,
    pub kind: WorkloadKind,
    pub desired: u32,
    pub conditions: Vec<Condition>,
    pub pending: Option<Pending>,
    pub managed_by: Option<ResourceRef>,
    pub pods: Vec<Pod>,
    pub revisions: Vec<Revision>,
    pub history_pending: bool,
    pub containers: Vec<ContainerSpec>,
    pub events: Vec<Event>,
}

impl Workload {
    pub fn ready(&self) -> u32 {
        let ready = self.pods.iter().filter(|pod| pod.is_ready()).count();
        u32::try_from(ready).unwrap_or(u32::MAX)
    }

    pub fn stalled(&self) -> bool {
        self.conditions
            .iter()
            .any(|condition| condition.reason == PROGRESS_DEADLINE)
    }

    pub fn condition_line(&self) -> Option<ConditionLine> {
        if self.pending.is_some() {
            return None;
        }
        ConditionLine::pick(&self.conditions)
    }

    pub fn ready_label(&self) -> String {
        format!("{}/{} ready", self.ready(), self.desired)
    }

    pub fn current_revision(&self) -> Option<&Revision> {
        self.revisions.iter().find(|revision| revision.current)
    }

    pub fn rollback_target(&self) -> Option<&Revision> {
        let current = self.current_revision()?.number;
        self.revisions
            .iter()
            .filter(|revision| revision.number < current)
            .max_by_key(|revision| revision.number)
    }

    pub fn image(&self) -> Option<&str> {
        let revision = self
            .current_revision()
            .map(|revision| revision.image.as_str());
        let container = self.containers.first().map(|c| c.image.as_str());
        revision.or(container).map(short_image)
    }

    pub fn status(&self) -> Status {
        if self.pending.is_some() {
            return Status::Progress;
        }
        if self.ready() >= self.desired {
            return Status::Ok;
        }
        let failing = self.pods.iter().any(|pod| pod.status == Status::Err);
        match (failing, self.stalled()) {
            (true, _) => Status::Err,
            (false, true) => Status::Warn,
            (false, false) => Status::Progress,
        }
    }

    pub fn fields(&self) -> Vec<Field> {
        let status = self.status();
        let ready = Field {
            status: (status != Status::Ok).then_some(status),
            ..Field::new("Ready", format!("{}/{}", self.ready(), self.desired))
        };
        let image = self.image().map_or_else(|| NONE.to_owned(), str::to_owned);
        let revision = self.current_revision().map_or_else(
            || NONE.to_owned(),
            |revision| {
                let age = time::age(revision.age);
                format!("{}{FIELD_SEPARATOR}{age} ago", revision.number)
            },
        );
        let container = self.containers.first();
        let pair = |request: Option<&String>, limit: Option<&String>| {
            let request = request.map_or(NONE, String::as_str);
            let limit = limit.map_or(NONE, String::as_str);
            format!("{request}{LIMIT_SEPARATOR}{limit}")
        };
        let cpu = pair(
            container.and_then(|c| c.requests.cpu.as_ref()),
            container.and_then(|c| c.limits.cpu.as_ref()),
        );
        let memory = pair(
            container.and_then(|c| c.requests.memory.as_ref()),
            container.and_then(|c| c.limits.memory.as_ref()),
        );
        let mut fields = vec![
            ready,
            Field::new("Image", image),
            Field::new("Revision", revision),
            Field::new("Replicas", self.desired.to_string()),
            Field::new("CPU request / limit", cpu),
            Field::new("Memory request / limit", memory),
        ];
        if let Some(owner) = &self.managed_by {
            fields.push(Field::new("Managed by", owner.name.clone()));
        }
        fields
    }

    pub fn sentence(&self) -> String {
        if let Some(pending) = &self.pending {
            return pending_sentence(pending);
        }
        let revision = self.current_revision().map(|r| r.number);
        let ready = self.ready();
        if ready >= self.desired {
            return ready_sentence(self.desired, revision);
        }
        let failing: Vec<&Pod> = self
            .pods
            .iter()
            .filter(|pod| pod.status == Status::Err)
            .collect();
        let head = if failing.is_empty() {
            let waiting = self.desired - ready;
            format!(
                "{} of {} pods {} not ready yet",
                capitalized(waiting),
                self.desired,
                verb(waiting)
            )
        } else {
            let count = u32::try_from(failing.len()).unwrap_or(u32::MAX);
            format!(
                "{} {} {} {}",
                capitalized(count),
                plural(count, "pod", "pods"),
                verb(count),
                problem(&failing)
            )
        };
        let tail = match (self.stalled(), revision) {
            (true, Some(number)) => format!(", so the rollout to revision {number} is stuck."),
            (false, Some(number)) if failing.is_empty() => {
                format!("; the rollout to revision {number} is in progress.")
            }
            _ => ".".into(),
        };
        format!("{head}{tail}")
    }
}

fn pending_sentence(pending: &Pending) -> String {
    match pending {
        Pending::Restarting => "Restarting the rollout: new pods replace the old ones.".into(),
        Pending::Scaling { replicas } => format!(
            "Scaling to {replicas} {}.",
            plural(*replicas, "replica", "replicas")
        ),
        Pending::RollingBack { revision } => format!("Rolling back to revision {revision}."),
    }
}

fn ready_sentence(desired: u32, revision: Option<u64>) -> String {
    let on = revision
        .map(|n| format!(" on revision {n}"))
        .unwrap_or_default();
    match desired {
        0 => "Scaled to zero; no pods are running.".into(),
        1 => format!("The pod is ready{on}."),
        n => format!("All {} pods are ready{on}.", number_word(n)),
    }
}

fn problem(failing: &[&Pod]) -> String {
    let phase = failing
        .first()
        .map(|pod| pod.phase.as_str())
        .unwrap_or_default();
    let shared = failing.iter().all(|pod| pod.phase == phase);
    match (shared, phase) {
        (true, CRASH_LOOP) => "crash-looping".into(),
        (true, phase) if !phase.is_empty() => format!("failing ({phase})"),
        _ => "failing".into(),
    }
}

fn verb(count: u32) -> &'static str {
    plural(count, "is", "are")
}

fn plural(count: u32, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}

pub fn number_word(n: u32) -> String {
    usize::try_from(n)
        .ok()
        .and_then(|ix| NUMBER_WORDS.get(ix))
        .map_or_else(|| n.to_string(), |word| (*word).to_owned())
}

fn capitalized(n: u32) -> String {
    let word = number_word(n);
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

pub fn short_image(image: &str) -> &str {
    image.rsplit('/').next().unwrap_or(image)
}

pub fn short_pod_name(name: &str) -> &str {
    name.rsplit('-').next().unwrap_or(name)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pod {
    pub name: String,
    pub phase: String,
    pub status: Status,
    pub node: String,
    pub cpu_millis: Option<u32>,
    pub memory_bytes: Option<u64>,
    pub age: Duration,
    pub containers: Vec<ContainerStatus>,
}

impl Pod {
    pub fn ready_containers(&self) -> usize {
        self.containers.iter().filter(|c| c.ready).count()
    }

    pub fn is_ready(&self) -> bool {
        !self.containers.is_empty() && self.ready_containers() == self.containers.len()
    }

    pub fn ready_label(&self) -> String {
        format!("{}/{}", self.ready_containers(), self.containers.len())
    }

    pub fn restarts(&self) -> u32 {
        self.containers.iter().map(|c| c.restarts).sum()
    }

    pub fn container(&self, name: &str) -> Option<&ContainerStatus> {
        self.containers.iter().find(|c| c.name == name)
    }

    pub fn cpu_label(&self) -> String {
        self.cpu_millis
            .map_or_else(|| NONE.to_owned(), units::cpu_millis)
    }

    pub fn memory_label(&self) -> String {
        self.memory_bytes
            .map_or_else(|| NONE.to_owned(), units::memory_bytes)
    }

    pub fn age_label(&self) -> String {
        time::age(self.age)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerStatus {
    pub name: String,
    pub ready: bool,
    pub restarts: u32,
    pub last_exit: Option<Exit>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exit {
    pub code: i32,
    pub reason: String,
    pub ago: Duration,
}

impl Exit {
    pub fn label(&self) -> String {
        format!(
            "{} ({}) · {} ago",
            self.code,
            self.reason,
            time::age(self.ago)
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revision {
    pub number: u64,
    pub owner: String,
    pub image: String,
    pub ready: u32,
    pub replicas: u32,
    pub age: Duration,
    pub commit: Option<Commit>,
    pub current: bool,
}

impl Revision {
    pub fn pods_label(&self) -> String {
        if self.replicas == 0 {
            "0".into()
        } else {
            format!("{}/{}", self.ready, self.replicas)
        }
    }

    pub fn status(&self) -> Option<Status> {
        match (self.replicas, self.ready) {
            (0, _) => None,
            (replicas, ready) if ready >= replicas => Some(Status::Ok),
            _ => Some(Status::Err),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub subject: String,
}

impl Commit {
    pub fn label(&self, workload: &str) -> String {
        let scope = format!("{workload}: ");
        let subject = self.subject.strip_prefix(&scope).unwrap_or(&self.subject);
        format!("{} {subject}", self.sha)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerSpec {
    pub name: String,
    pub image: String,
    pub ports: Vec<Port>,
    pub requests: Resources,
    pub limits: Resources,
    pub readiness: Option<Probe>,
    pub env: Vec<EnvVar>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Port {
    pub port: u16,
    pub protocol: String,
    pub name: Option<String>,
}

impl Port {
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
    pub fn label(&self) -> String {
        let parts: Vec<String> = [("CPU", &self.cpu), ("memory", &self.memory)]
            .into_iter()
            .filter_map(|(name, value)| value.as_ref().map(|v| format!("{name} {v}")))
            .collect();
        if parts.is_empty() {
            "none".into()
        } else {
            parts.join(" · ")
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeAction {
    HttpGet { port: u16, path: String },
    TcpSocket { port: u16 },
    Exec { command: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    pub action: ProbeAction,
    pub period: Duration,
}

impl Probe {
    pub fn label(&self) -> String {
        let action = match &self.action {
            ProbeAction::HttpGet { port, path } => format!("GET :{port}{path}"),
            ProbeAction::TcpSocket { port } => format!("TCP :{port}"),
            ProbeAction::Exec { command } => format!("exec {command}"),
        };
        format!("{action} · {}", time::age(self.period))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnvSource {
    Value(String),
    SecretKey { name: String, key: String },
    ConfigMapKey { name: String, key: String },
    Field(String),
}

impl EnvSource {
    pub fn label(&self) -> String {
        match self {
            Self::Value(value) => format!("= {value}"),
            Self::SecretKey { name, key } => format!("← secret {name} / {key}"),
            Self::ConfigMapKey { name, key } => format!("← configmap {name} / {key}"),
            Self::Field(path) => format!("← field {path}"),
        }
    }

    fn object(&self) -> Option<String> {
        match self {
            Self::Value(_) => None,
            Self::SecretKey { name, .. } => Some(format!("secret/{name}")),
            Self::ConfigMapKey { name, .. } => Some(format!("configmap/{name}")),
            Self::Field(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnvResolution {
    Literal,
    Resolved,
    MissingKey { optional: bool, keys: Vec<String> },
    MissingObject { optional: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvVar {
    pub name: String,
    pub source: EnvSource,
    pub resolution: EnvResolution,
}

impl EnvVar {
    pub fn status(&self) -> Option<Status> {
        match self.resolution {
            EnvResolution::Literal => None,
            EnvResolution::Resolved => Some(Status::Ok),
            EnvResolution::MissingKey { .. } => Some(Status::Err),
            EnvResolution::MissingObject { .. } => Some(Status::Err),
        }
    }

    pub fn note(&self) -> Option<String> {
        let object = self.source.object().unwrap_or_default();
        match &self.resolution {
            EnvResolution::Literal => None,
            EnvResolution::Resolved => None,
            EnvResolution::MissingKey { optional, keys } => Some(format!(
                "Key not found, {} {object} has: {}.",
                consequence(*optional),
                keys.join(", ")
            )),
            EnvResolution::MissingObject { optional } => Some(format!(
                "{object} does not exist, {}",
                consequence(*optional)
            )),
        }
    }
}

fn consequence(optional: bool) -> &'static str {
    if optional {
        "so the var is empty (optional: true)."
    } else {
        "so the container cannot start."
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub age: Duration,
    pub status: Status,
    pub reason: String,
    pub message: String,
    pub count: u32,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ClusterId;

    const HOUR: Duration = Duration::from_secs(3600);

    fn container(name: &str, ready: bool, restarts: u32) -> ContainerStatus {
        ContainerStatus {
            name: name.into(),
            ready,
            restarts,
            last_exit: None,
        }
    }

    fn pod(name: &str, phase: &str, status: Status, ready: bool) -> Pod {
        Pod {
            name: name.into(),
            phase: phase.into(),
            status,
            node: "node".into(),
            cpu_millis: None,
            memory_bytes: None,
            age: HOUR,
            containers: vec![container("api", ready, 0), container("envoy", true, 0)],
        }
    }

    fn revision(number: u64, current: bool) -> Revision {
        Revision {
            number,
            owner: format!("rs-{number}"),
            image: format!("ghcr.io/acme/checkout-api:2.{number}"),
            ready: 0,
            replicas: 0,
            age: HOUR,
            commit: None,
            current,
        }
    }

    pub(crate) fn workload(pods: Vec<Pod>, stalled: bool) -> Workload {
        let stuck = Condition::new("Progressing", "False").reason(PROGRESS_DEADLINE);
        let conditions = if stalled { vec![stuck] } else { Vec::new() };
        Workload {
            uid: Uid::default(),
            target: ResourceRef {
                cluster: ClusterId::new("prod-us-east"),
                kind: "Deployment".into(),
                namespace: Some("payments".into()),
                name: "checkout-api".into(),
            },
            kind: WorkloadKind::Deployment,
            desired: 3,
            conditions,
            pending: None,
            managed_by: None,
            pods,
            revisions: vec![revision(37, true), revision(36, false), revision(35, false)],
            history_pending: false,
            containers: Vec::new(),
            events: Vec::new(),
        }
    }

    pub(crate) fn running(name: &str) -> Pod {
        pod(name, "Running", Status::Ok, true)
    }

    pub(crate) fn crashing(name: &str) -> Pod {
        pod(name, CRASH_LOOP, Status::Err, false)
    }

    #[test]
    fn compact_fields_name_readiness_image_revision_and_resources() {
        let mut w = workload(vec![crashing("a"), running("b"), running("c")], true);
        w.containers.push(ContainerSpec {
            name: "api".into(),
            image: "checkout-api".into(),
            ports: Vec::new(),
            requests: Resources {
                cpu: Some("500m".into()),
                memory: Some("512Mi".into()),
            },
            limits: Resources {
                cpu: Some("1".into()),
                memory: None,
            },
            readiness: None,
            env: Vec::new(),
        });
        w.managed_by = Some(ResourceRef {
            kind: "Kustomization".into(),
            name: "apps-payments".into(),
            ..w.target.clone()
        });
        let fields = w.fields();
        let shown: Vec<(&str, &str)> = fields
            .iter()
            .map(|field| (field.label, field.value.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                ("Ready", "2/3"),
                ("Image", "checkout-api:2.37"),
                ("Revision", "37 · 1h ago"),
                ("Replicas", "3"),
                ("CPU request / limit", "500m / 1"),
                ("Memory request / limit", "512Mi / —"),
                ("Managed by", "apps-payments"),
            ]
        );
        assert_eq!(fields.first().and_then(|f| f.status), Some(Status::Err));
        let healthy = workload(vec![running("a"), running("b"), running("c")], false);
        assert_eq!(healthy.fields().first().and_then(|f| f.status), None);
        assert_eq!(healthy.fields().len(), 6);
    }

    #[test]
    fn one_crash_looping_pod_blocks_the_rollout() {
        let w = workload(vec![crashing("a"), running("b"), running("c")], true);
        assert_eq!(w.ready_label(), "2/3 ready");
        assert_eq!(w.status(), Status::Err);
        assert_eq!(
            w.sentence(),
            "One pod is crash-looping, so the rollout to revision 37 is stuck."
        );
    }

    #[test]
    fn several_failing_pods_with_mixed_phases() {
        let mut oom = crashing("b");
        oom.phase = "OOMKilled".into();
        let w = workload(vec![crashing("a"), oom, running("c")], false);
        assert_eq!(w.sentence(), "Two pods are failing.");
    }

    #[test]
    fn unready_pods_without_failures_are_progressing() {
        let starting = pod("a", "Running", Status::Progress, false);
        let w = workload(vec![starting, running("b"), running("c")], false);
        assert_eq!(w.status(), Status::Progress);
        assert_eq!(
            w.sentence(),
            "One of 3 pods is not ready yet; the rollout to revision 37 is in progress."
        );
    }

    #[test]
    fn healthy_workload_reports_its_revision() {
        let w = workload(vec![running("a"), running("b"), running("c")], false);
        assert_eq!(w.status(), Status::Ok);
        assert_eq!(w.sentence(), "All three pods are ready on revision 37.");
        let mut single = workload(vec![running("a")], false);
        single.desired = 1;
        assert_eq!(single.sentence(), "The pod is ready on revision 37.");
        let mut idle = workload(Vec::new(), false);
        idle.desired = 0;
        assert_eq!(idle.sentence(), "Scaled to zero; no pods are running.");
    }

    #[test]
    fn pending_action_wins() {
        let mut w = workload(vec![crashing("a")], true);
        w.pending = Some(Pending::RollingBack { revision: 36 });
        assert_eq!(w.status(), Status::Progress);
        assert_eq!(w.sentence(), "Rolling back to revision 36.");
        w.pending = Some(Pending::Scaling { replicas: 1 });
        assert_eq!(w.sentence(), "Scaling to 1 replica.");
    }

    #[test]
    fn rollback_target_is_the_newest_older_revision() {
        let w = workload(Vec::new(), false);
        assert_eq!(w.rollback_target().map(|r| r.number), Some(36));
        assert_eq!(w.image(), Some("checkout-api:2.37"));
    }

    #[test]
    fn pod_rollups() {
        let mut p = crashing("checkout-api-7f9c6d-x2lqp");
        p.containers[0].restarts = 14;
        assert_eq!(p.ready_label(), "1/2");
        assert_eq!(p.restarts(), 14);
        assert!(!p.is_ready());
        assert_eq!(short_pod_name(&p.name), "x2lqp");
    }

    #[test]
    fn spec_labels_match_the_inspector() {
        let port = Port {
            port: 8080,
            protocol: "TCP".into(),
            name: Some("http".into()),
        };
        assert_eq!(port.label(), "8080/TCP http");
        let requests = Resources {
            cpu: Some("500m".into()),
            memory: Some("512Mi".into()),
        };
        assert_eq!(requests.label(), "CPU 500m · memory 512Mi");
        assert_eq!(Resources::default().label(), "none");
        let probe = Probe {
            action: ProbeAction::HttpGet {
                port: 8080,
                path: "/healthz".into(),
            },
            period: Duration::from_secs(5),
        };
        assert_eq!(probe.label(), "GET :8080/healthz · 5s");
        let exit = Exit {
            code: 1,
            reason: "Error".into(),
            ago: Duration::from_secs(120),
        };
        assert_eq!(exit.label(), "1 (Error) · 2m ago");
    }

    #[test]
    fn missing_secret_key_explains_itself() {
        let var = EnvVar {
            name: "STRIPE_WEBHOOK_SECRET".into(),
            source: EnvSource::SecretKey {
                name: "stripe".into(),
                key: "webhook_secret".into(),
            },
            resolution: EnvResolution::MissingKey {
                optional: true,
                keys: vec!["api-key".into(), "webhook-secret".into()],
            },
        };
        assert_eq!(var.status(), Some(Status::Err));
        assert_eq!(var.source.label(), "← secret stripe / webhook_secret");
        assert_eq!(
            var.note().as_deref(),
            Some(
                "Key not found, so the var is empty (optional: true). secret/stripe has: api-key, webhook-secret."
            )
        );
        let gone = EnvVar {
            resolution: EnvResolution::MissingObject { optional: false },
            ..var
        };
        assert_eq!(
            gone.note().as_deref(),
            Some("secret/stripe does not exist, so the container cannot start.")
        );
    }

    #[test]
    fn literal_env_has_no_status() {
        let var = EnvVar {
            name: "LOG_LEVEL".into(),
            source: EnvSource::Value("info".into()),
            resolution: EnvResolution::Literal,
        };
        assert_eq!(var.status(), None);
        assert_eq!(var.source.label(), "= info");
        assert_eq!(var.note(), None);
    }

    #[test]
    fn kinds_round_trip_and_map_to_pages() {
        for kind in [
            WorkloadKind::Deployment,
            WorkloadKind::StatefulSet,
            WorkloadKind::DaemonSet,
        ] {
            assert_eq!(WorkloadKind::from_kind(kind.kind()), Some(kind));
        }
        assert_eq!(WorkloadKind::from_kind("Job"), None);
        assert!(!WorkloadKind::DaemonSet.scalable());
    }

    #[test]
    fn commit_labels_drop_the_workload_scope() {
        let commit = Commit {
            sha: "a41e07c".into(),
            subject: "checkout-api: bump to 2.14.0".into(),
        };
        assert_eq!(commit.label("checkout-api"), "a41e07c bump to 2.14.0");
        assert_eq!(
            commit.label("payments-worker"),
            "a41e07c checkout-api: bump to 2.14.0"
        );
    }

    #[test]
    fn revision_pod_counts() {
        let mut current = revision(37, true);
        current.ready = 2;
        current.replicas = 3;
        assert_eq!(current.pods_label(), "2/3");
        assert_eq!(current.status(), Some(Status::Err));
        assert_eq!(revision(36, false).pods_label(), "0");
        assert_eq!(revision(36, false).status(), None);
    }

    #[test]
    fn pods_without_metrics_show_a_dash() {
        let mut p = pod("a", "Running", Status::Ok, true);
        assert_eq!(
            (p.cpu_label().as_str(), p.memory_label().as_str()),
            ("—", "—")
        );
        p.cpu_millis = Some(120);
        assert_ne!(p.cpu_label(), "—");
    }

    #[test]
    fn the_image_comes_from_the_spec_until_the_history_arrives() {
        let mut w = workload(Vec::new(), false);
        w.revisions.clear();
        assert_eq!(w.image(), None);
        w.containers.push(ContainerSpec {
            name: "api".into(),
            image: "ghcr.io/acme/checkout-api:2.14.0".into(),
            ports: Vec::new(),
            requests: Resources::default(),
            limits: Resources::default(),
            readiness: None,
            env: Vec::new(),
        });
        assert_eq!(w.image(), Some("checkout-api:2.14.0"));
    }

    #[test]
    fn the_status_line_is_the_condition_unless_an_action_is_pending() {
        let mut w = workload(vec![crashing("a")], true);
        assert!(w.stalled());
        assert_eq!(
            w.condition_line().map(|line| line.compact()).as_deref(),
            Some("Progressing=False · ProgressDeadlineExceeded")
        );
        w.pending = Some(Pending::Restarting);
        assert_eq!(w.condition_line(), None);
        assert!(!workload(Vec::new(), false).stalled());
    }
}
