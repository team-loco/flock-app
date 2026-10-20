use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::condition_line::ConditionLine;
use crate::filter::{FilterSpec, TextFilter};

use crate::{
    ClusterId, Condition, Diagnosis, Page, ResourceRef, Snapshot, Status, Uid, time, units,
};

const FILTER_HIDES_ALL: &str = "Nothing matches this filter.";
const NOTHING_APPLIED_RECONCILING: &str =
    "Nothing applied yet: the first reconcile is still running.";
const NOTHING_APPLIED_FAILING: &str = "Nothing applied: the last reconcile failed.";
const NOTHING_APPLIED_RETRYING: &str =
    "Nothing applied: the last reconcile failed and is being retried.";
const NOTHING_APPLIED_SUSPENDED: &str = "Nothing applied while suspended.";
const NOTHING_MANAGED: &str = "This object manages nothing.";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum FluxKind {
    Kustomization,
    HelmRelease,
    GitRepository,
    HelmRepository,
    OciRepository,
}

const KUSTOMIZE_GROUP: &str = "kustomize.toolkit.fluxcd.io";
const HELM_GROUP: &str = "helm.toolkit.fluxcd.io";
const SOURCE_GROUP: &str = "source.toolkit.fluxcd.io";

impl FluxKind {
    pub const ALL: [Self; 5] = [
        Self::Kustomization,
        Self::HelmRelease,
        Self::GitRepository,
        Self::HelmRepository,
        Self::OciRepository,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Kustomization => "Kustomization",
            Self::HelmRelease => "HelmRelease",
            Self::GitRepository => "GitRepository",
            Self::HelmRepository => "HelmRepository",
            Self::OciRepository => "OCIRepository",
        }
    }

    pub const fn page(self) -> Page {
        match self {
            Self::Kustomization => Page::Kustomizations,
            Self::HelmRelease => Page::HelmReleases,
            Self::GitRepository => Page::Sources,
            Self::HelmRepository => Page::Sources,
            Self::OciRepository => Page::Sources,
        }
    }

    pub const fn group(self) -> &'static str {
        match self {
            Self::Kustomization => KUSTOMIZE_GROUP,
            Self::HelmRelease => HELM_GROUP,
            Self::GitRepository => SOURCE_GROUP,
            Self::HelmRepository => SOURCE_GROUP,
            Self::OciRepository => SOURCE_GROUP,
        }
    }

    pub const fn is_source(self) -> bool {
        matches!(
            self,
            Self::GitRepository | Self::HelmRepository | Self::OciRepository
        )
    }

    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.label() == label)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FluxKey {
    pub cluster: ClusterId,
    pub kind: FluxKind,
    pub namespace: String,
    pub name: String,
}

impl FluxKey {
    pub fn new(
        cluster: &str,
        kind: FluxKind,
        namespace: impl Into<String>,
        name: impl Into<String>,
    ) -> Self {
        Self {
            cluster: ClusterId::new(cluster),
            kind,
            namespace: namespace.into(),
            name: name.into(),
        }
    }

    pub fn on(&self, cluster: &ClusterId) -> Self {
        Self {
            cluster: cluster.clone(),
            ..self.clone()
        }
    }

    pub fn resource(&self) -> ResourceRef {
        ResourceRef {
            cluster: self.cluster.clone(),
            kind: self.kind.label().to_owned(),
            namespace: Some(self.namespace.clone()),
            name: self.name.clone(),
        }
    }

    pub fn same_object(&self, other: &Self) -> bool {
        self.kind == other.kind && self.namespace == other.namespace && self.name == other.name
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FluxAction {
    Reconcile,
    ReconcileWithSource,
    Force,
    Suspend,
    Resume,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Ready,
    Reconciling,
    Retrying,
    Failing,
    Suspended { since: Option<Duration> },
}

impl Phase {
    pub const fn status(self) -> Status {
        match self {
            Self::Ready => Status::Ok,
            Self::Reconciling => Status::Progress,
            Self::Retrying => Status::Warn,
            Self::Failing => Status::Err,
            Self::Suspended { .. } => Status::Suspended,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Reconciling => "Reconciling",
            Self::Retrying => "Retrying",
            Self::Failing => "Failing",
            Self::Suspended { .. } => "Suspended",
        }
    }

    pub fn note(self) -> String {
        match self {
            Self::Suspended { since: Some(since) } => format!("suspended {}", time::age(since)),
            Self::Suspended { since: None } => self.label().to_lowercase(),
            Self::Ready => self.label().to_lowercase(),
            Self::Reconciling => self.label().to_lowercase(),
            Self::Retrying => self.label().to_lowercase(),
            Self::Failing => self.label().to_lowercase(),
        }
    }

    pub const fn is_suspended(self) -> bool {
        matches!(self, Self::Suspended { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revision {
    pub id: String,
    pub age: Duration,
}

impl Revision {
    pub fn new(id: impl Into<String>, age: Duration) -> Self {
        Self { id: id.into(), age }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Span {
    Text(String),
    Object(String),
    Revision(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sentence(pub Vec<Span>);

impl Sentence {
    pub fn text(text: impl Into<String>) -> Self {
        Self(vec![Span::Text(text.into())])
    }

    pub fn plain(&self) -> String {
        self.0
            .iter()
            .map(|span| match span {
                Span::Text(text) => text.as_str(),
                Span::Object(name) => name.as_str(),
                Span::Revision(id) => id.as_str(),
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedObject {
    pub status: Status,
    pub kind: String,
    pub namespace: Option<String>,
    pub name: String,
    pub detail: String,
    pub changed: bool,
}

impl ManagedObject {
    pub fn path(&self) -> String {
        match &self.namespace {
            Some(namespace) => format!("{namespace}/{}", self.name),
            None => self.name.clone(),
        }
    }

    pub fn resource(&self, cluster: &ClusterId) -> ResourceRef {
        ResourceRef {
            cluster: cluster.clone(),
            kind: self.kind.clone(),
            namespace: self.namespace.clone(),
            name: self.name.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InventoryFilter {
    #[default]
    All,
    Unhealthy,
    Changed,
}

impl InventoryFilter {
    pub const ALL: [Self; 3] = [Self::All, Self::Unhealthy, Self::Changed];

    pub fn matches(self, object: &ManagedObject) -> bool {
        match self {
            Self::All => true,
            Self::Unhealthy => object.status != Status::Ok,
            Self::Changed => object.changed,
        }
    }

    pub fn label(self, applied: Option<&Revision>) -> String {
        match (self, applied) {
            (Self::All, _) => "All".into(),
            (Self::Unhealthy, _) => "Unhealthy".into(),
            (Self::Changed, Some(revision)) => format!("Changed in {}", revision.id),
            (Self::Changed, None) => "Changed".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    pub age: Option<Duration>,
    pub status: Status,
    pub revision: String,
    pub message: String,
    pub count: u32,
}

impl HistoryEntry {
    pub fn age_label(&self) -> String {
        self.age.map_or_else(|| "now".into(), time::age)
    }

    pub fn count_label(&self) -> Option<String> {
        (self.count > 1).then(|| units::times(self.count))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecRow {
    pub label: String,
    pub value: String,
}

impl SpecRow {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelmRevision {
    pub revision: u32,
    pub status: Status,
    pub state: String,
    pub chart_version: String,
    pub app_version: String,
    pub description: String,
    pub age: Duration,
    pub diff: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelmDetail {
    pub chart: String,
    pub version: String,
    pub values: Vec<SpecRow>,
    pub releases: Vec<HelmRevision>,
}

impl HelmDetail {
    pub fn deployed(&self) -> Option<&HelmRevision> {
        self.releases.iter().find(|r| r.status == Status::Ok)
    }

    pub fn first_change(&self) -> Option<u32> {
        let changed = self.releases.iter().find(|r| !r.diff.is_empty());
        changed.or(self.releases.first()).map(|r| r.revision)
    }

    pub fn previous(&self, revision: u32) -> Option<&HelmRevision> {
        self.releases.iter().find(|r| r.revision < revision)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceDetail {
    pub url: String,
    pub reference: String,
    pub fetched: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DriftState {
    Missing,
    Extra,
    Changed,
}

impl DriftState {
    pub const fn status(self) -> Status {
        match self {
            Self::Missing => Status::Err,
            Self::Extra => Status::Warn,
            Self::Changed => Status::Warn,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Missing => "missing from the cluster",
            Self::Extra => "labelled for this Kustomization but not in its inventory",
            Self::Changed => "changed since kustomize-controller last applied it",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriftField {
    pub path: String,
    pub value: String,
    pub manager: String,
    pub age: Option<Duration>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriftedObject {
    pub state: DriftState,
    pub kind: String,
    pub namespace: Option<String>,
    pub name: String,
    pub fields: Vec<DriftField>,
}

impl DriftedObject {
    pub fn resource(&self, cluster: &ClusterId) -> ResourceRef {
        ResourceRef {
            cluster: cluster.clone(),
            kind: self.kind.clone(),
            namespace: self.namespace.clone(),
            name: self.name.clone(),
        }
    }

    pub fn path(&self) -> String {
        match &self.namespace {
            Some(namespace) => format!("{}/{namespace}/{}", self.kind, self.name),
            None => format!("{}/{}", self.kind, self.name),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Drift {
    pub checked: usize,
    pub objects: Vec<DriftedObject>,
    pub unreadable: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FluxDetail {
    Kustomization(Option<Drift>),
    HelmRelease(HelmDetail),
    Source(SourceDetail),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FluxObject {
    pub key: FluxKey,
    pub uid: Uid,
    pub labels: BTreeMap<String, String>,
    pub phase: Phase,
    pub sentence: Sentence,
    pub applied: Option<Revision>,
    pub last_healthy: Option<Revision>,
    pub source: Option<FluxKey>,
    pub depends_on: Vec<FluxKey>,
    pub inventory: Vec<ManagedObject>,
    pub history: Vec<HistoryEntry>,
    pub spec: Vec<SpecRow>,
    pub yaml: String,
    pub detail: FluxDetail,
    pub conditions: Vec<Condition>,
    pub diagnosis: Option<Diagnosis>,
}

impl FluxObject {
    pub const FILTER: FilterSpec<Self> = FilterSpec {
        fields: &[
            ("name", |o, (), out| out.push(o.key.name.as_str().into())),
            ("kind", |o, (), out| out.push(o.key.kind.label().into())),
        ],
        namespace: true,
        labels: true,
    };

    pub fn matches(&self, query: &TextFilter) -> bool {
        Self::FILTER.matches(self, query, Some(&self.key.namespace), &[&self.labels])
    }

    pub fn status(&self) -> Status {
        self.phase.status()
    }

    pub fn condition_line(&self) -> Option<ConditionLine> {
        if self.phase.is_suspended() {
            let applied = self.applied.as_ref().map(|revision| revision.id.as_str());
            return Some(ConditionLine::suspended(applied));
        }
        ConditionLine::pick(&self.conditions)
    }

    pub fn current_diagnosis(&self) -> Option<&Diagnosis> {
        self.diagnosis
            .as_ref()
            .filter(|_| crate::diagnosis::needs_diagnosis(self.status()))
    }

    pub fn inventory(&self, filter: InventoryFilter) -> Vec<ManagedObject> {
        self.inventory
            .iter()
            .filter(|object| filter.matches(object))
            .cloned()
            .collect()
    }

    pub fn inventory_empty(&self) -> &'static str {
        if !self.inventory.is_empty() {
            return FILTER_HIDES_ALL;
        }
        match self.phase {
            Phase::Reconciling => NOTHING_APPLIED_RECONCILING,
            Phase::Failing => NOTHING_APPLIED_FAILING,
            Phase::Retrying => NOTHING_APPLIED_RETRYING,
            Phase::Suspended { .. } => NOTHING_APPLIED_SUSPENDED,
            Phase::Ready => NOTHING_MANAGED,
        }
    }

    pub fn shown_last_healthy(&self) -> Option<&Revision> {
        let applied = self.applied.as_ref().map(|r| r.id.as_str());
        self.last_healthy
            .as_ref()
            .filter(|healthy| self.status() != Status::Ok && Some(healthy.id.as_str()) != applied)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerVerdict {
    HealthyElsewhere,
    HealthyAtAnotherRevision,
    FailingEverywhere,
}

impl PeerVerdict {
    pub const fn sentence(self) -> &'static str {
        match self {
            Self::HealthyElsewhere => "Healthy elsewhere at the same revision.",
            Self::HealthyAtAnotherRevision => "Healthy elsewhere, at a different revision.",
            Self::FailingEverywhere => "Failing on every cluster at this revision.",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dependency {
    pub key: FluxKey,
    pub status: Status,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FluxPage {
    pub object: FluxObject,
    pub peers: Vec<FluxObject>,
    pub dependencies: Vec<Dependency>,
    pub dependents: Vec<FluxObject>,
    pub verdict: Option<PeerVerdict>,
}

impl Snapshot {
    pub fn flux_page(&self, key: &FluxKey) -> Option<FluxPage> {
        let object = self.flux_object(key)?.clone();
        let dependencies = object
            .depends_on
            .iter()
            .map(|dependency| Dependency {
                key: dependency.clone(),
                status: self
                    .flux_object(dependency)
                    .map_or(Status::Offline, FluxObject::status),
            })
            .collect();
        Some(FluxPage {
            peers: self.flux_peers(key).into_iter().cloned().collect(),
            dependents: self.flux_dependents(key).into_iter().cloned().collect(),
            verdict: self.peer_verdict(key),
            dependencies,
            object,
        })
    }

    pub fn flux_object(&self, key: &FluxKey) -> Option<&FluxObject> {
        self.flux.iter().find(|object| &object.key == key)
    }

    pub fn flux_peers(&self, key: &FluxKey) -> Vec<&FluxObject> {
        let applied = self
            .flux_object(key)
            .and_then(|object| object.applied.as_ref())
            .map(|revision| revision.id.as_str());
        let cluster_rank = |object: &FluxObject| {
            self.clusters
                .iter()
                .position(|cluster| cluster.id == object.key.cluster)
        };
        let mut peers: Vec<&FluxObject> = self
            .flux
            .iter()
            .filter(|object| object.key.same_object(key) && object.key.cluster != key.cluster)
            .collect();
        peers.sort_by_key(|object| {
            let revision = object.applied.as_ref().map(|r| r.id.as_str());
            (revision != applied, cluster_rank(object))
        });
        peers
    }

    pub fn flux_dependents(&self, source: &FluxKey) -> Vec<&FluxObject> {
        self.flux
            .iter()
            .filter(|object| object.source.as_ref() == Some(source))
            .collect()
    }

    pub fn peer_verdict(&self, key: &FluxKey) -> Option<PeerVerdict> {
        let object = self.flux_object(key)?;
        if object.status() == Status::Ok {
            return None;
        }
        let applied = object.applied.as_ref()?;
        let peers = self.flux_peers(key);
        let same_revision: Vec<_> = peers
            .into_iter()
            .filter(|peer| peer.applied.as_ref().map(|r| &r.id) == Some(&applied.id))
            .collect();
        if same_revision.is_empty() {
            let healthy_elsewhere = self
                .flux_peers(key)
                .iter()
                .any(|peer| peer.status() == Status::Ok);
            return healthy_elsewhere.then_some(PeerVerdict::HealthyAtAnotherRevision);
        }
        if same_revision.iter().any(|peer| peer.status() == Status::Ok) {
            Some(PeerVerdict::HealthyElsewhere)
        } else {
            Some(PeerVerdict::FailingEverywhere)
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::flock::tests::cluster;
    use crate::{Counts, Snapshot};

    const MINUTE: u64 = 60;
    const DAY: u64 = 86_400;

    pub(crate) fn object(cluster: &str, phase: Phase, revision: &str) -> FluxObject {
        FluxObject {
            labels: BTreeMap::new(),
            uid: Uid::default(),
            key: FluxKey::new(cluster, FluxKind::Kustomization, "flux-system", "apps"),
            phase,
            sentence: Sentence::default(),
            applied: Some(Revision::new(revision, Duration::from_secs(MINUTE))),
            last_healthy: None,
            source: None,
            depends_on: Vec::new(),
            inventory: Vec::new(),
            history: Vec::new(),
            spec: Vec::new(),
            yaml: String::new(),
            detail: FluxDetail::Kustomization(None),
            conditions: Vec::new(),
            diagnosis: None,
        }
    }

    #[test]
    fn a_suspended_object_names_its_spec_field_instead_of_a_condition() {
        let mut held = object("west", Phase::Suspended { since: None }, "7c2d9e0");
        held.conditions = vec![Condition::new("Ready", "True").reason("ReconciliationSucceeded")];
        let line = held.condition_line().map(|line| line.compact());
        assert_eq!(
            line.as_deref(),
            Some("spec.suspend=true · last applied 7c2d9e0")
        );
        held.phase = Phase::Ready;
        let line = held.condition_line().map(|line| line.compact());
        assert_eq!(
            line.as_deref(),
            Some("Ready=True · ReconciliationSucceeded")
        );
    }

    #[test]
    fn a_reconciling_object_hides_its_last_failure() {
        let mut failing = object("east", Phase::Failing, "abc");
        failing.diagnosis = Some(Diagnosis::new("HealthCheckFailed", "timeout"));
        assert_eq!(
            failing.current_diagnosis().map(|d| d.reason.as_str()),
            Some("HealthCheckFailed")
        );
        failing.phase = Phase::Reconciling;
        assert_eq!(failing.current_diagnosis(), None);
    }

    fn flock(objects: Vec<FluxObject>) -> Snapshot {
        Snapshot {
            clusters: ["east", "west", "staging", "dev"]
                .map(|name| cluster(name, Status::Ok, Counts::default()))
                .into(),
            flux: objects,
            ..Snapshot::default()
        }
    }

    fn managed(status: Status, changed: bool) -> ManagedObject {
        ManagedObject {
            status,
            kind: "Deployment".into(),
            namespace: Some("payments".into()),
            name: "checkout-api".into(),
            detail: String::new(),
            changed,
        }
    }

    #[test]
    fn kinds_route_to_their_nav_page() {
        assert_eq!(FluxKind::Kustomization.page(), Page::Kustomizations);
        assert_eq!(FluxKind::HelmRelease.page(), Page::HelmReleases);
        assert_eq!(FluxKind::OciRepository.page(), Page::Sources);
        assert!(FluxKind::GitRepository.is_source());
        assert!(!FluxKind::HelmRelease.is_source());
        assert_eq!(
            FluxKind::from_label("OCIRepository"),
            Some(FluxKind::OciRepository)
        );
        assert_eq!(FluxKind::from_label("Deployment"), None);
    }

    #[test]
    fn phases_map_to_status_and_notes() {
        let suspended = Phase::Suspended {
            since: Some(Duration::from_secs(2 * DAY)),
        };
        assert_eq!(suspended.status(), Status::Suspended);
        assert_eq!(suspended.note(), "suspended 2d");
        let unknown = Phase::Suspended { since: None };
        assert_eq!(unknown.note(), "suspended");
        assert_eq!(Phase::Failing.status(), Status::Err);
        assert_eq!(Phase::Ready.note(), "ready");
        assert_eq!(Phase::Retrying.status(), Status::Warn);
    }

    #[test]
    fn inventory_filters_by_health_and_change() {
        let mut ks = object("east", Phase::Failing, "a41e07c");
        ks.inventory = vec![
            managed(Status::Err, true),
            managed(Status::Ok, false),
            managed(Status::Ok, true),
        ];
        assert_eq!(ks.inventory(InventoryFilter::All).len(), 3);
        assert_eq!(ks.inventory(InventoryFilter::Unhealthy).len(), 1);
        assert_eq!(ks.inventory(InventoryFilter::Changed).len(), 2);
        let applied = ks.applied.as_ref();
        assert_eq!(
            InventoryFilter::Changed.label(applied),
            "Changed in a41e07c"
        );
    }

    #[test]
    fn an_empty_inventory_names_the_object_not_the_filter() {
        let mut ks = object("east", Phase::Failing, "a41e07c");
        assert_eq!(ks.inventory_empty(), NOTHING_APPLIED_FAILING);
        ks.phase = Phase::Suspended {
            since: Some(Duration::from_secs(DAY)),
        };
        assert_eq!(ks.inventory_empty(), NOTHING_APPLIED_SUSPENDED);
        ks.phase = Phase::Ready;
        assert_eq!(ks.inventory_empty(), NOTHING_MANAGED);
        ks.inventory = vec![managed(Status::Ok, false)];
        assert_eq!(ks.inventory_empty(), FILTER_HIDES_ALL);
    }

    #[test]
    fn managed_objects_resolve_to_resources() {
        let resource = managed(Status::Ok, false).resource(&ClusterId::new("east"));
        assert_eq!(resource.kind, "Deployment");
        assert_eq!(resource.to_string(), "payments/checkout-api");
        assert_eq!(managed(Status::Ok, false).path(), "payments/checkout-api");
    }

    #[test]
    fn history_labels_now_and_repeat_counts() {
        let entry = HistoryEntry {
            age: None,
            status: Status::Progress,
            revision: "a41e07c".into(),
            message: "reconcile requested".into(),
            count: 1,
        };
        assert_eq!(entry.age_label(), "now");
        assert_eq!(entry.count_label(), None);
        let repeated = HistoryEntry {
            age: Some(Duration::from_secs(4 * MINUTE)),
            count: 18,
            ..entry
        };
        assert_eq!(repeated.age_label(), "4m");
        assert_eq!(repeated.count_label().as_deref(), Some("18 times"));
    }

    #[test]
    fn last_healthy_shows_only_when_it_differs_and_object_is_unhealthy() {
        let mut ks = object("east", Phase::Failing, "a41e07c");
        ks.last_healthy = Some(Revision::new("7c2d9e0", Duration::from_secs(DAY)));
        assert_eq!(
            ks.shown_last_healthy().map(|r| r.id.as_str()),
            Some("7c2d9e0")
        );
        ks.phase = Phase::Ready;
        assert_eq!(ks.shown_last_healthy(), None);
    }

    #[test]
    fn peers_list_same_revision_first_in_cluster_order() {
        let snapshot = flock(vec![
            object("east", Phase::Failing, "a41e07c"),
            object(
                "west",
                Phase::Suspended {
                    since: Some(Duration::from_secs(2 * DAY)),
                },
                "7c2d9e0",
            ),
            object("staging", Phase::Ready, "a41e07c"),
            object("dev", Phase::Ready, "a41e07c"),
        ]);
        let key = FluxKey::new("east", FluxKind::Kustomization, "flux-system", "apps");
        let peers: Vec<_> = snapshot
            .flux_peers(&key)
            .into_iter()
            .map(|peer| peer.key.cluster.as_str())
            .collect();
        assert_eq!(peers, ["staging", "dev", "west"]);
        assert_eq!(
            snapshot.peer_verdict(&key),
            Some(PeerVerdict::HealthyElsewhere)
        );
    }

    #[test]
    fn verdicts_need_an_unhealthy_object_and_same_revision_peers() {
        let failing_everywhere = flock(vec![
            object("east", Phase::Failing, "a41e07c"),
            object("west", Phase::Failing, "a41e07c"),
        ]);
        let east = FluxKey::new("east", FluxKind::Kustomization, "flux-system", "apps");
        assert_eq!(
            failing_everywhere.peer_verdict(&east),
            Some(PeerVerdict::FailingEverywhere)
        );
        let healthy = flock(vec![
            object("east", Phase::Ready, "a41e07c"),
            object("west", Phase::Ready, "a41e07c"),
        ]);
        assert_eq!(healthy.peer_verdict(&east), None);
        let alone = flock(vec![
            object("east", Phase::Failing, "a41e07c"),
            object("west", Phase::Ready, "7c2d9e0"),
        ]);
        assert_eq!(
            alone.peer_verdict(&east),
            Some(PeerVerdict::HealthyAtAnotherRevision)
        );
        let nobody_healthy = flock(vec![
            object("east", Phase::Failing, "a41e07c"),
            object("west", Phase::Failing, "7c2d9e0"),
        ]);
        assert_eq!(nobody_healthy.peer_verdict(&east), None);
    }

    #[test]
    fn dependents_are_objects_using_the_source() {
        let source = FluxKey::new("east", FluxKind::GitRepository, "flux-system", "infra");
        let mut user = object("east", Phase::Ready, "a41e07c");
        user.source = Some(source.clone());
        let other = object("west", Phase::Ready, "a41e07c");
        let snapshot = flock(vec![user, other]);
        let dependents = snapshot.flux_dependents(&source);
        assert_eq!(dependents.len(), 1);
        assert_eq!(dependents[0].key.cluster.as_str(), "east");
    }

    #[test]
    fn page_resolves_dependency_status_and_marks_missing_ones_offline() {
        let mut ks = object("east", Phase::Failing, "a41e07c");
        let present = FluxKey::new("east", FluxKind::Kustomization, "flux-system", "infra");
        let missing = FluxKey::new("east", FluxKind::Kustomization, "flux-system", "gone");
        ks.depends_on = vec![present.clone(), missing];
        let mut infra = object("east", Phase::Ready, "a41e07c");
        infra.key = present;
        let key = ks.key.clone();
        let snapshot = flock(vec![ks, infra]);
        let page = snapshot.flux_page(&key);
        let statuses: Vec<_> = page
            .iter()
            .flat_map(|page| page.dependencies.iter().map(|d| d.status))
            .collect();
        assert_eq!(statuses, [Status::Ok, Status::Offline]);
    }

    #[test]
    fn deployed_release_is_the_ok_one() {
        let release = |revision, status| HelmRevision {
            revision,
            status,
            state: String::new(),
            chart_version: String::new(),
            app_version: String::new(),
            description: String::new(),
            age: Duration::ZERO,
            diff: String::new(),
        };
        let detail = HelmDetail {
            chart: "kube-prometheus-stack".into(),
            version: "62.7.0".into(),
            values: Vec::new(),
            releases: vec![release(9, Status::Warn), release(8, Status::Ok)],
        };
        assert_eq!(detail.deployed().map(|r| r.revision), Some(8));
        assert_eq!(detail.first_change(), Some(9));
        assert_eq!(detail.previous(9).map(|r| r.revision), Some(8));
        assert_eq!(detail.previous(8), None);
    }

    #[test]
    fn first_change_skips_retries_without_a_diff() {
        let release = |revision, diff: &str| HelmRevision {
            revision,
            status: Status::Warn,
            state: String::new(),
            chart_version: String::new(),
            app_version: String::new(),
            description: String::new(),
            age: Duration::ZERO,
            diff: diff.into(),
        };
        let detail = HelmDetail {
            chart: String::new(),
            version: String::new(),
            values: Vec::new(),
            releases: vec![release(16, ""), release(15, "-a\n+b\n"), release(14, "")],
        };
        assert_eq!(detail.first_change(), Some(15));
    }

    #[test]
    fn sentence_plain_text_joins_spans() {
        let sentence = Sentence(vec![
            Span::Object("checkout-api".into()),
            Span::Text(" did not become ready.".into()),
        ]);
        assert_eq!(sentence.plain(), "checkout-api did not become ready.");
    }
}
