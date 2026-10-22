use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::Status;
use crate::overview::plural;

const GKE_PREFIX: &str = "gke_";
const GKE_SEPARATOR: char = '_';
const EKS_PREFIX: &str = "arn:aws:eks:";
const EKS_CLUSTER: &str = ":cluster/";
const HOME: &str = "~";
const META_SEPARATOR: &str = " · ";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContextRef {
    pub file: PathBuf,
    pub context: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextChoice {
    pub id: ContextRef,
    pub server: Option<String>,
    pub auth: String,
    pub selected: bool,
}

impl ContextChoice {
    pub fn meta(&self) -> String {
        let parts = [self.server.as_deref(), Some(self.auth.as_str())];
        let present: Vec<&str> = parts.into_iter().flatten().collect();
        present.join(META_SEPARATOR)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KubeconfigFile {
    pub label: String,
    pub contexts: Vec<ContextChoice>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewCluster {
    pub context: ContextRef,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckStatus {
    Ok,
    Warn,
    Err,
    Info,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckLine {
    pub status: CheckStatus,
    pub detail: String,
    pub raw: Option<String>,
    pub remedy: Option<String>,
    pub duration: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Checking,
    Ready,
    CannotReach,
    Unauthorized,
}

impl Verdict {
    pub const fn status(self) -> Status {
        match self {
            Self::Checking => Status::Progress,
            Self::Ready => Status::Ok,
            Self::CannotReach => Status::Err,
            Self::Unauthorized => Status::Err,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Checking => "Checking…",
            Self::Ready => "Ready",
            Self::CannotReach => "Can’t reach",
            Self::Unauthorized => "Unauthorized",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AddClusterStep {
    #[default]
    Pick,
    Check,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub context: ContextRef,
    pub name: String,
    pub checks: Vec<CheckLine>,
    pub verdict: Verdict,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameProblem {
    Empty,
    Duplicate(String),
}

impl NameProblem {
    pub fn sentence(&self) -> String {
        match self {
            Self::Empty => "Every cluster needs a name.".to_owned(),
            Self::Duplicate(name) => format!("Another cluster is already named {name}."),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddClusterFlow {
    pub step: AddClusterStep,
    pub files: Option<Vec<KubeconfigFile>>,
    pub selected: BTreeSet<ContextRef>,
    pub names: BTreeMap<ContextRef, String>,
    pub candidates: Vec<Candidate>,
    existing: BTreeSet<String>,
    connect_on_load: bool,
}

impl AddClusterFlow {
    pub fn new(step: AddClusterStep, existing: impl IntoIterator<Item = String>) -> Self {
        Self {
            existing: existing.into_iter().collect(),
            connect_on_load: step == AddClusterStep::Check,
            ..Self::default()
        }
    }

    pub fn load(&mut self, files: Vec<KubeconfigFile>) -> Option<Vec<ContextRef>> {
        for choice in files.iter().flat_map(|file| &file.contexts) {
            self.names
                .entry(choice.id.clone())
                .or_insert_with(|| suggest_name(&choice.id.context));
            if choice.selected {
                self.selected.insert(choice.id.clone());
            }
        }
        self.files = Some(files);
        let auto = std::mem::take(&mut self.connect_on_load);
        auto.then(|| self.connect()).flatten()
    }

    pub fn choices(&self) -> impl Iterator<Item = &ContextChoice> {
        self.files.iter().flatten().flat_map(|file| &file.contexts)
    }

    pub fn is_selected(&self, id: &ContextRef) -> bool {
        self.selected.contains(id)
    }

    pub fn toggle(&mut self, id: &ContextRef) {
        if !self.selected.remove(id) {
            self.selected.insert(id.clone());
        }
    }

    pub fn rename(&mut self, id: &ContextRef, name: &str) {
        self.names.insert(id.clone(), name.trim().to_owned());
    }

    pub fn name(&self, id: &ContextRef) -> &str {
        self.names.get(id).map_or("", String::as_str)
    }

    fn selection(&self) -> Vec<&ContextRef> {
        self.choices()
            .map(|choice| &choice.id)
            .filter(|id| self.selected.contains(*id))
            .collect()
    }

    pub fn selected_count(&self) -> usize {
        self.selection().len()
    }

    pub fn connect_label(&self) -> String {
        format!("Connect {}", self.selected_count())
    }

    pub fn name_problem(&self) -> Option<NameProblem> {
        let mut seen = self.existing.clone();
        for id in self.selection() {
            let name = self.name(id);
            if name.is_empty() {
                return Some(NameProblem::Empty);
            }
            if !seen.insert(name.to_owned()) {
                return Some(NameProblem::Duplicate(name.to_owned()));
            }
        }
        None
    }

    pub fn can_connect(&self) -> bool {
        self.selected_count() > 0 && self.name_problem().is_none()
    }

    pub fn connect(&mut self) -> Option<Vec<ContextRef>> {
        if !self.can_connect() {
            return None;
        }
        let ids: Vec<ContextRef> = self.selection().into_iter().cloned().collect();
        self.candidates = ids
            .iter()
            .map(|id| Candidate {
                context: id.clone(),
                name: self.name(id).to_owned(),
                checks: Vec::new(),
                verdict: Verdict::Checking,
            })
            .collect();
        self.step = AddClusterStep::Check;
        Some(ids)
    }

    pub fn back(&mut self) {
        self.step = AddClusterStep::Pick;
        self.candidates.clear();
    }

    pub fn retry(&mut self, id: &ContextRef) -> bool {
        let Some(candidate) = self.candidate_mut(id) else {
            return false;
        };
        candidate.checks.clear();
        candidate.verdict = Verdict::Checking;
        true
    }

    fn candidate_mut(&mut self, id: &ContextRef) -> Option<&mut Candidate> {
        self.candidates.iter_mut().find(|c| &c.context == id)
    }

    pub fn record(&mut self, id: &ContextRef, line: CheckLine) {
        if let Some(candidate) = self.candidate_mut(id) {
            candidate.checks.push(line);
        }
    }

    pub fn finish(&mut self, id: &ContextRef, verdict: Verdict) {
        if let Some(candidate) = self.candidate_mut(id) {
            candidate.verdict = verdict;
        }
    }

    pub fn title(&self) -> String {
        match self.step {
            AddClusterStep::Pick => "Add clusters".to_owned(),
            AddClusterStep::Check => {
                let count = self.candidates.len();
                let noun = if count == 1 { "cluster" } else { "clusters" };
                format!("Connecting {count} {noun}")
            }
        }
    }

    pub fn ready(&self) -> Vec<NewCluster> {
        self.candidates
            .iter()
            .filter(|c| c.verdict == Verdict::Ready)
            .map(|c| NewCluster {
                context: c.context.clone(),
                name: c.name.clone(),
            })
            .collect()
    }

    pub fn add_label(&self) -> String {
        match self.ready().as_slice() {
            [] => "Add".to_owned(),
            [one] => format!("Add {}", one.name),
            many => format!("Add {}", plural(many.len(), "cluster", "clusters")),
        }
    }

    pub fn ready_note(&self) -> String {
        let ready = self.ready().len();
        format!("{ready} of {} ready", self.candidates.len())
    }
}

pub fn suggest_name(context: &str) -> String {
    if let Some(rest) = context.strip_prefix(GKE_PREFIX)
        && let Some(name) = rest.rsplit(GKE_SEPARATOR).next()
        && !name.is_empty()
    {
        return name.to_owned();
    }
    if context.starts_with(EKS_PREFIX)
        && let Some((_, name)) = context.rsplit_once(EKS_CLUSTER)
        && !name.is_empty()
    {
        return name.to_owned();
    }
    context.to_owned()
}

pub fn home_relative(path: &Path, home: Option<&Path>) -> String {
    let relative = home.and_then(|home| path.strip_prefix(home).ok());
    match relative {
        Some(rest) => Path::new(HOME).join(rest).display().to_string(),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(context: &str) -> ContextRef {
        ContextRef {
            file: PathBuf::from("/h/.kube/config"),
            context: context.to_owned(),
        }
    }

    fn choice(context: &str, selected: bool) -> ContextChoice {
        ContextChoice {
            id: id(context),
            server: Some("https://127.0.0.1:6443".to_owned()),
            auth: "client certificate".to_owned(),
            selected,
        }
    }

    fn flow(step: AddClusterStep, existing: &[&str]) -> (AddClusterFlow, Option<Vec<ContextRef>>) {
        let mut flow = AddClusterFlow::new(step, existing.iter().map(|s| (*s).to_owned()));
        let files = vec![KubeconfigFile {
            label: "~/.kube/config".to_owned(),
            contexts: vec![
                choice("gke_acme-data_us-central1_analytics", true),
                choice("minikube", false),
                choice("arn:aws:eks:us-west-2:1:cluster/sandbox", true),
            ],
        }];
        let started = flow.load(files);
        (flow, started)
    }

    fn line(status: CheckStatus) -> CheckLine {
        CheckLine {
            status,
            detail: "Reachable in 88 ms".to_owned(),
            raw: None,
            remedy: None,
            duration: Duration::ZERO,
        }
    }

    #[test]
    fn names_follow_provider_conventions() {
        assert_eq!(
            suggest_name("gke_acme-data_us-central1_analytics"),
            "analytics"
        );
        assert_eq!(
            suggest_name("arn:aws:eks:us-west-2:123:cluster/sandbox"),
            "sandbox"
        );
        assert_eq!(suggest_name("kind-fleet-a"), "kind-fleet-a");
        assert_eq!(suggest_name("gke_"), "gke_");
    }

    #[test]
    fn paths_under_home_use_a_tilde() {
        let home = Path::new("/Users/me");
        assert_eq!(
            home_relative(Path::new("/Users/me/.kube/eks/sandbox.yaml"), Some(home)),
            "~/.kube/eks/sandbox.yaml"
        );
        assert_eq!(
            home_relative(Path::new("/etc/k.yaml"), Some(home)),
            "/etc/k.yaml"
        );
    }

    #[test]
    fn meta_joins_server_and_auth() {
        let mut c = choice("x", false);
        assert_eq!(c.meta(), "https://127.0.0.1:6443 · client certificate");
        c.server = None;
        assert_eq!(c.meta(), "client certificate");
    }

    #[test]
    fn loading_applies_backend_selection_and_names() {
        let (mut flow, started) = flow(AddClusterStep::Pick, &[]);
        assert_eq!(started, None);
        assert_eq!(flow.connect_label(), "Connect 2");
        assert_eq!(flow.name(&id("minikube")), "minikube");
        flow.toggle(&id("minikube"));
        flow.toggle(&id("gke_acme-data_us-central1_analytics"));
        assert_eq!(flow.connect_label(), "Connect 2");
        assert!(flow.is_selected(&id("minikube")));
    }

    #[test]
    fn opening_at_check_connects_once_contexts_arrive() {
        let (flow, started) = flow(AddClusterStep::Check, &[]);
        assert_eq!(started.map(|ids| ids.len()), Some(2));
        assert_eq!(flow.step, AddClusterStep::Check);
        assert_eq!(flow.title(), "Connecting 2 clusters");
    }

    #[test]
    fn names_must_be_present_and_unique() {
        let (mut flow, _) = flow(AddClusterStep::Pick, &["analytics"]);
        assert_eq!(
            flow.name_problem(),
            Some(NameProblem::Duplicate("analytics".to_owned()))
        );
        assert!(flow.connect().is_none());
        flow.rename(&id("gke_acme-data_us-central1_analytics"), "  ");
        assert_eq!(flow.name_problem(), Some(NameProblem::Empty));
        flow.rename(&id("gke_acme-data_us-central1_analytics"), " data ");
        assert_eq!(flow.name_problem(), None);
        assert_eq!(
            flow.name(&id("gke_acme-data_us-central1_analytics")),
            "data"
        );
    }

    #[test]
    fn nothing_selected_cannot_connect() {
        let (mut flow, _) = flow(AddClusterStep::Pick, &[]);
        flow.selected.clear();
        assert!(!flow.can_connect());
        assert_eq!(flow.connect_label(), "Connect 0");
    }

    #[test]
    fn check_results_drive_the_footer() {
        let (mut flow, _) = flow(AddClusterStep::Pick, &[]);
        assert!(flow.connect().is_some());
        let analytics = id("gke_acme-data_us-central1_analytics");
        let sandbox = id("arn:aws:eks:us-west-2:1:cluster/sandbox");
        assert_eq!(flow.add_label(), "Add");
        assert_eq!(flow.ready_note(), "0 of 2 ready");
        flow.record(&analytics, line(CheckStatus::Ok));
        flow.finish(&analytics, Verdict::Ready);
        flow.record(&sandbox, line(CheckStatus::Ok));
        flow.finish(&sandbox, Verdict::Unauthorized);
        assert_eq!(flow.add_label(), "Add analytics");
        assert_eq!(flow.ready_note(), "1 of 2 ready");
        assert_eq!(
            flow.ready(),
            vec![NewCluster {
                context: analytics,
                name: "analytics".to_owned()
            }]
        );
        assert!(flow.retry(&sandbox));
        assert_eq!(flow.candidates.get(1).map(|c| c.checks.len()), Some(0));
        assert_eq!(flow.ready_note(), "1 of 2 ready");
    }

    #[test]
    fn the_footer_counts_ready_clusters() {
        let (mut flow, _) = flow(AddClusterStep::Pick, &[]);
        flow.toggle(&id("minikube"));
        assert!(flow.connect().is_some());
        flow.finish(&id("minikube"), Verdict::CannotReach);
        flow.finish(
            &id("arn:aws:eks:us-west-2:1:cluster/sandbox"),
            Verdict::Unauthorized,
        );
        flow.finish(&id("gke_acme-data_us-central1_analytics"), Verdict::Ready);
        assert_eq!(flow.ready_note(), "1 of 3 ready");
        assert_eq!(flow.add_label(), "Add analytics");
        flow.finish(&id("minikube"), Verdict::Ready);
        assert_eq!(flow.ready_note(), "2 of 3 ready");
        assert_eq!(flow.add_label(), "Add 2 clusters");
    }

    #[test]
    fn back_returns_to_the_picker_with_the_selection() {
        let (mut flow, _) = flow(AddClusterStep::Pick, &[]);
        assert!(flow.connect().is_some());
        flow.back();
        assert_eq!(flow.step, AddClusterStep::Pick);
        assert_eq!(flow.candidates.len(), 0);
        assert_eq!(flow.selected_count(), 2);
        assert_eq!(flow.title(), "Add clusters");
    }

    #[test]
    fn verdicts_map_to_status() {
        assert_eq!(Verdict::Checking.status(), Status::Progress);
        assert_eq!(Verdict::Ready.status(), Status::Ok);
        assert_eq!(Verdict::Unauthorized.status(), Status::Err);
        assert_eq!(Verdict::Ready.label(), "Ready");
        assert_eq!(Verdict::Unauthorized.label(), "Unauthorized");
    }
}
