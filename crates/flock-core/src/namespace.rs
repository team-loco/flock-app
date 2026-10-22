use std::cmp::Reverse;
use std::collections::BTreeMap;

use crate::cluster::{ClusterDetail, Namespace};
use crate::events::Event;
use crate::home::Problem;
use crate::issue::{Issue, named};
use crate::overview::{WorkloadHealth, plural};
use crate::related::{Group, Related};
use crate::resources::ResourceKind;
use crate::time::age;
use crate::units::{self, Amount, CPU_UNIT, MEMORY_UNIT};
use crate::watch::WatchState;
use crate::{Counts, Page, ResourceRef, Status};

pub const RECENT_EVENTS: usize = 4;
const SEPARATOR: &str = " · ";
const RATIO: &str = "/";
const QUOTA_SCOPE: char = '.';
const CPU_RESOURCE: &str = "cpu";
const FAILING: &str = "failing";
const DEGRADED: &str = "degraded";
const SUSPENDED: &str = "suspended";
const READY: &str = "ready";
const RUNNING: &str = "running";
const PENDING: &str = "pending";
const UNREADY_ENDPOINTS: &str = "endpoints not ready";
const REQUESTED: &str = "requested";
const OBJECT_ONE: &str = "object";
const OBJECT_MANY: &str = "objects";
const CREATED: &str = "created";
const AGO: &str = "ago";
const EVENTS: &str = "Events";
const WORKLOADS: [ResourceKind; 5] = [
    ResourceKind::Deployment,
    ResourceKind::StatefulSet,
    ResourceKind::DaemonSet,
    ResourceKind::Job,
    ResourceKind::CronJob,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tally {
    pub label: String,
    pub status: Status,
    pub count: usize,
}

pub fn tally<'a>(pods: impl IntoIterator<Item = (Status, &'a str)>) -> Vec<Tally> {
    let mut seen: BTreeMap<&str, (Status, usize)> = BTreeMap::new();
    for (status, label) in pods {
        let entry = seen.entry(label).or_insert((status, 0));
        entry.0 = entry.0.max(status);
        entry.1 += 1;
    }
    let mut tallies: Vec<Tally> = seen
        .into_iter()
        .map(|(label, (status, count))| Tally {
            label: label.to_owned(),
            status,
            count,
        })
        .collect();
    tallies.sort_by_key(|t| (t.status, Reverse(t.count)));
    tallies
}

fn tally_parts(tallies: &[Tally]) -> Vec<(Status, String)> {
    tallies
        .iter()
        .map(|t| (t.status, format!("{} {}", t.count, t.label)))
        .collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Requested {
    pub requested: f32,
    pub quota: Option<f32>,
}

impl Requested {
    pub fn share(&self) -> Option<f32> {
        let quota = self.quota.filter(|quota| *quota > 0.)?;
        Some((self.requested / quota).clamp(0., 1.))
    }

    pub fn cpu_label(&self) -> Amount {
        self.label(CPU_UNIT)
    }

    pub fn memory_label(&self) -> Amount {
        self.label(MEMORY_UNIT)
    }

    fn label(&self, unit: &str) -> Amount {
        let unit = format!("{unit} {REQUESTED}");
        match self.quota {
            Some(quota) => units::amount(self.requested, quota, &unit),
            None => Amount::of(units::figure(self.requested), &unit),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuotaItem {
    pub resource: String,
    pub used: String,
    pub hard: String,
}

impl QuotaItem {
    pub fn label(&self) -> String {
        let amounts = if self.is_cpu() {
            units::written_cpus(Some(&self.used), Some(&self.hard), RATIO)
        } else {
            format!("{}{RATIO}{}", self.used, self.hard)
        };
        format!("{} {amounts}", self.resource)
    }

    fn is_cpu(&self) -> bool {
        self.resource.rsplit(QUOTA_SCOPE).next() == Some(CPU_RESOURCE)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quota {
    pub name: String,
    pub items: Vec<QuotaItem>,
}

impl Quota {
    pub fn line(&self) -> String {
        std::iter::once(self.name.clone())
            .chain(self.items.iter().map(QuotaItem::label))
            .collect::<Vec<_>>()
            .join(SEPARATOR)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub name: String,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listing<T> {
    pub items: Vec<T>,
    pub state: WatchState,
}

impl<T> Default for Listing<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            state: WatchState::pending(crate::watch::NOT_STARTED),
        }
    }
}

impl<T> Listing<T> {
    pub fn unavailable(&self) -> Option<&str> {
        match &self.state {
            WatchState::Failed(reason) => Some(reason),
            WatchState::Pending(_) => None,
            WatchState::Synced => None,
            WatchState::Refreshing(_) => None,
        }
    }

    pub fn loading(&self) -> bool {
        matches!(self.state, WatchState::Pending(_))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NamespaceDetail {
    pub name: String,
    pub pods: Vec<Tally>,
    pub cpu_quota: Option<f32>,
    pub memory_quota: Option<f32>,
    pub quotas: Listing<Quota>,
    pub policies: Listing<Policy>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TilePart {
    pub status: Status,
    pub count: usize,
    pub label: String,
}

impl TilePart {
    fn new(status: Status, count: usize, word: &str) -> Self {
        Self {
            status,
            count,
            label: format!("{count} {word}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tile {
    pub page: Page,
    pub total: usize,
    pub parts: Vec<TilePart>,
}

impl Tile {
    fn new(page: Page, total: usize, ok: TilePart, flagged: Vec<TilePart>) -> Self {
        let flagged = flagged.into_iter().filter(|part| part.count > 0);
        Self {
            page,
            total,
            parts: std::iter::once(ok).chain(flagged).collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamespaceIssue {
    pub target: ResourceRef,
    pub issue: Issue,
}

impl NamespaceIssue {
    fn of(problem: &Problem, detail: Option<&ClusterDetail>) -> Self {
        let target = &problem.target;
        let diagnosis = detail.and_then(|detail| {
            let kind = ResourceKind::ALL
                .into_iter()
                .find(|kind| kind.name() == target.kind)?;
            detail
                .resources
                .get(&kind)?
                .iter()
                .find(|row| {
                    target.namespace.as_deref() == Some(row.namespace.as_str())
                        && row.name == target.name
                })?
                .diagnosis
                .as_ref()
        });
        let object = named(&target.kind, &target.name, Some(problem.age));
        let issue = match diagnosis {
            Some(diagnosis) => Issue::from_diagnosis(problem.status, diagnosis, object),
            None => Issue::new(
                problem.status,
                problem.reason.clone(),
                problem.message.clone(),
                object,
            ),
        };
        Self {
            target: target.clone(),
            issue,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NamespacePage {
    pub target: Option<ResourceRef>,
    pub namespace: Option<Namespace>,
    pub attention: Vec<Problem>,
    pub issues: Vec<NamespaceIssue>,
    pub groups: Vec<Group>,
    pub events: Vec<Event>,
    pub detail: Option<NamespaceDetail>,
    pub managed_by: Option<ResourceRef>,
    pub unready_services: usize,
}

fn counted(counts: Counts, total: usize) -> Vec<(Status, String)> {
    let flagged = counts.flagged() as usize;
    let mut parts = Vec::new();
    if flagged > 0 {
        let ok = total.saturating_sub(flagged);
        if ok > 0 {
            parts.push((Status::Ok, format!("{ok} {READY}")));
        }
    }
    let labelled = [
        (Status::Err, counts.err, FAILING),
        (Status::Warn, counts.warn, DEGRADED),
        (Status::Suspended, counts.sus, SUSPENDED),
    ];
    for (status, count, label) in labelled {
        if count > 0 {
            parts.push((status, format!("{count} {label}")));
        }
    }
    parts
}

fn health(kind: ResourceKind, group: Option<&Group>) -> WorkloadHealth {
    let total = group.map_or(0, |g| g.total);
    let counts = group.map(|g| g.counts).unwrap_or_default();
    let flagged = counts.flagged() as usize;
    let count = |status: Status| match status {
        Status::Ok => total.saturating_sub(flagged),
        Status::Err => counts.err as usize,
        Status::Warn => counts.warn as usize,
        Status::Suspended => counts.sus as usize,
        _ => 0,
    };
    WorkloadHealth::counted(kind, total, count)
}

fn health_parts(health: &WorkloadHealth) -> Vec<(Status, String)> {
    let ok = (health.ok > 0 || health.bad == 0).then(|| (Status::Ok, health.ok_label.clone()));
    let bad = health
        .bad_label
        .clone()
        .map(|label| (health.bad_status, label));
    ok.into_iter().chain(bad).collect()
}

impl NamespacePage {
    pub fn build(
        target: &ResourceRef,
        detail: Option<&ClusterDetail>,
        problems: &[Problem],
    ) -> Self {
        let name = target.name.as_str();
        let namespace = detail
            .and_then(|d| d.namespaces.iter().find(|n| n.name == name))
            .cloned();
        let groups = detail
            .and_then(|d| d.related(target))
            .map(|related: &Related| {
                related
                    .groups
                    .iter()
                    .filter(|g| g.title != EVENTS)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let mut events: Vec<Event> = detail
            .map(|d| {
                d.events
                    .iter()
                    .filter(|e| e.namespace.as_deref() == Some(name))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        events.sort_by_key(|e| e.age);
        let here = |p: &&Problem| {
            p.target.cluster == target.cluster && p.target.namespace.as_deref() == Some(name)
        };
        let unready = detail
            .map(|d| d.overview.unready_services.as_slice())
            .unwrap_or_default();
        let attention: Vec<Problem> = problems
            .iter()
            .chain(unready)
            .filter(here)
            .cloned()
            .collect();
        let unready_services = unready.iter().filter(here).count();
        let issues = attention
            .iter()
            .map(|problem| NamespaceIssue::of(problem, detail))
            .collect();
        let managed_by = namespace
            .as_ref()
            .and_then(|n| crate::flux::managed_by(&target.cluster, &n.labels));
        Self {
            target: Some(target.clone()),
            namespace,
            attention,
            issues,
            groups,
            events,
            detail: detail.and_then(|d| d.namespace_detail(name)).cloned(),
            managed_by,
            unready_services,
        }
    }

    fn group(&self, page: Page) -> Option<&Group> {
        self.groups.iter().find(|g| g.title == page.label())
    }

    fn pod_tally(&self) -> Option<&[Tally]> {
        self.detail
            .as_ref()
            .map(|d| d.pods.as_slice())
            .filter(|pods| !pods.is_empty())
    }

    pub fn pods(&self) -> usize {
        self.pod_tally().map_or_else(
            || {
                self.group(Page::Pods).map_or_else(
                    || self.namespace.as_ref().map_or(0, |n| n.pods as usize),
                    |g| g.total,
                )
            },
            |pods| pods.iter().map(|t| t.count).sum(),
        )
    }

    fn pod_tile(&self) -> Tile {
        let (running, pending, failing) = self.pod_tally().map_or_else(
            || {
                let group = self.group(Page::Pods);
                let counts = group.map(|g| g.counts).unwrap_or_default();
                let total = group.map_or(0, |g| g.total);
                let running = total.saturating_sub(counts.flagged() as usize);
                (running, counts.warn as usize, counts.err as usize)
            },
            |pods| {
                let count = |wanted: Status| {
                    pods.iter()
                        .filter(|t| t.status == wanted)
                        .map(|t| t.count)
                        .sum::<usize>()
                };
                (count(Status::Ok), count(Status::Warn), count(Status::Err))
            },
        );
        let ok = TilePart::new(Status::Ok, running, RUNNING);
        let flagged = vec![
            TilePart::new(Status::Warn, pending, PENDING),
            TilePart::new(Status::Err, failing, FAILING),
        ];
        Tile::new(Page::Pods, self.pods(), ok, flagged)
    }

    fn workload_tile(&self, kind: ResourceKind) -> Tile {
        let group = self.group(kind.page());
        let total = group.map_or(0, |g| g.total);
        let health = health(kind, group);
        let ok = TilePart {
            status: Status::Ok,
            count: health.ok,
            label: health.ok_label.clone(),
        };
        let bad = health.bad_label.clone().map(|label| TilePart {
            status: health.bad_status,
            count: health.bad,
            label,
        });
        Tile::new(health.page, total, ok, bad.into_iter().collect())
    }

    fn service_tile(&self) -> Tile {
        let total = self.group(Page::Services).map_or(0, |g| g.total);
        let unready = self.unready_services.min(total);
        let ok = TilePart::new(Status::Ok, total - unready, READY);
        let flagged = vec![TilePart::new(Status::Warn, unready, UNREADY_ENDPOINTS)];
        Tile::new(Page::Services, total, ok, flagged)
    }

    pub fn tiles(&self) -> Vec<Tile> {
        vec![
            self.pod_tile(),
            self.workload_tile(ResourceKind::Deployment),
            self.workload_tile(ResourceKind::StatefulSet),
            self.service_tile(),
        ]
    }

    pub fn meta(&self) -> Option<String> {
        self.namespace
            .as_ref()
            .map(|namespace| format!("{CREATED} {} {AGO}", age(namespace.age)))
    }

    pub fn summary(&self, group: &Group) -> (Status, String) {
        let pods = self
            .pod_tally()
            .filter(|_| group.title == Page::Pods.label());
        let workload = WORKLOADS
            .into_iter()
            .find(|kind| kind.page().label() == group.title);
        let parts = match (pods, workload) {
            (Some(pods), _) => tally_parts(pods),
            (None, Some(kind)) => health_parts(&health(kind, Some(group))),
            (None, None) => counted(group.counts, group.total),
        };
        let text: Vec<String> = parts.iter().map(|(_, text)| text.clone()).collect();
        (group.counts.status(), text.join(SEPARATOR))
    }

    pub fn contents(&self, filter: &str) -> Vec<Group> {
        let needle = filter.trim().to_lowercase();
        self.groups
            .iter()
            .filter(|g| g.total > 0)
            .filter_map(|g| {
                if needle.is_empty() {
                    return Some(g.clone());
                }
                let items: Vec<_> = g
                    .items
                    .iter()
                    .filter(|item| item.label.to_lowercase().contains(&needle))
                    .cloned()
                    .collect();
                (!items.is_empty()).then(|| Group {
                    total: items.len(),
                    items,
                    ..g.clone()
                })
            })
            .collect()
    }

    pub fn empty(&self) -> Vec<&str> {
        self.groups
            .iter()
            .filter(|g| g.total == 0)
            .map(|g| g.title.as_str())
            .collect()
    }

    pub fn objects(&self) -> String {
        let total = self.groups.iter().map(|g| g.total).sum();
        plural(total, OBJECT_ONE, OBJECT_MANY)
    }

    fn requested(&self, requested: impl Fn(&Namespace) -> f32, quota: Option<f32>) -> Requested {
        Requested {
            requested: self.namespace.as_ref().map(requested).unwrap_or_default(),
            quota,
        }
    }

    pub fn cpu(&self) -> Requested {
        let quota = self.detail.as_ref().and_then(|d| d.cpu_quota);
        self.requested(|n| n.cpu_requests, quota)
    }

    pub fn memory(&self) -> Requested {
        let quota = self.detail.as_ref().and_then(|d| d.memory_quota);
        self.requested(|n| n.memory_requests, quota)
    }

    pub fn recent_events(&self) -> &[Event] {
        let shown = self.events.len().min(RECENT_EVENTS);
        &self.events[..shown]
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::ClusterId;
    use crate::related::{Fact, contents};

    const NS: &str = "payments";

    fn cluster() -> ClusterId {
        ClusterId::new("prod")
    }

    fn target() -> ResourceRef {
        ResourceRef {
            cluster: cluster(),
            kind: "Namespace".into(),
            namespace: None,
            name: NS.into(),
        }
    }

    fn fact(kind: &str, name: &str, status: Status, note: &str) -> Fact {
        Fact {
            status,
            note: note.into(),
            ..Fact::new(kind, Some(NS), name)
        }
    }

    fn event(reason: &str, minutes: u64) -> Event {
        Event {
            labels: BTreeMap::new(),
            details: crate::events::EventDetails::default(),
            status: Status::Ok,
            reason: reason.into(),
            object: "pod/x".into(),
            namespace: Some(NS.into()),
            message: String::new(),
            age: Duration::from_mins(minutes),
            count: 1,
        }
    }

    fn problem(namespace: &str, status: Status) -> Problem {
        Problem {
            status,
            target: ResourceRef {
                cluster: cluster(),
                kind: "Deployment".into(),
                namespace: Some(namespace.into()),
                name: "checkout-api".into(),
            },
            reason: "ProgressDeadlineExceeded".into(),
            message: "ReplicaSet \"checkout-api-7f9c6d\" has timed out progressing.".into(),
            condition: String::new(),
            age: Duration::from_hours(3),
            events: Vec::new(),
        }
    }

    fn detail() -> ClusterDetail {
        let facts = vec![
            fact("Deployment", "checkout-api", Status::Err, "2/3 ready"),
            fact("Deployment", "payments-worker", Status::Ok, "2/2 ready"),
            fact("StatefulSet", "payments-db", Status::Ok, "1/1 ready"),
            fact("Pod", "checkout-api-x2lqp", Status::Err, "CrashLoopBackOff"),
            fact("Pod", "checkout-api-m8vtn", Status::Ok, "Running"),
            fact("Pod", "payments-db-0", Status::Ok, "Running"),
            fact("Service", "checkout-api", Status::Ok, "ClusterIP"),
            fact("Secret", "stripe", Status::Ok, "2 keys"),
        ];
        let namespace = Namespace {
            name: NS.into(),
            labels: BTreeMap::from([(
                "kustomize.toolkit.fluxcd.io/name".to_owned(),
                "apps-payments".to_owned(),
            )]),
            status: Status::Err,
            phase: crate::cluster::ACTIVE_PHASE.into(),
            pods: 3,
            problems: Counts::default(),
            cpu: None,
            memory: None,
            cpu_requests: 1.2,
            memory_requests: 2.5,
            restarts: 0,
            age: Duration::from_hours(24 * 214),
        };
        let pods = tally([
            (Status::Err, "CrashLoopBackOff"),
            (Status::Ok, "Running"),
            (Status::Ok, "Running"),
        ]);
        ClusterDetail {
            namespaces: vec![namespace],
            events: vec![
                event("Pulled", 90),
                event("BackOff", 2),
                event("Synced", 4),
                event("Scaled", 30),
                event("Created", 60),
            ],
            related: vec![contents(&cluster(), &facts, &[], NS)],
            namespace_details: vec![NamespaceDetail {
                name: NS.into(),
                pods,
                cpu_quota: Some(4.),
                ..NamespaceDetail::default()
            }],
            ..ClusterDetail::default()
        }
    }

    fn page() -> NamespacePage {
        let problems = [
            problem(NS, Status::Err),
            problem("orders", Status::Err),
            problem(NS, Status::Warn),
        ];
        NamespacePage::build(&target(), Some(&detail()), &problems)
    }

    #[test]
    fn pods_tally_by_reason_with_healthy_first_then_by_count() {
        let pods = tally([
            (Status::Err, "CrashLoopBackOff"),
            (Status::Ok, "Running"),
            (Status::Warn, "Pending"),
            (Status::Ok, "Running"),
        ]);
        let labels: Vec<(&str, usize)> = pods.iter().map(|t| (t.label.as_str(), t.count)).collect();
        assert_eq!(
            labels,
            [("Running", 2), ("Pending", 1), ("CrashLoopBackOff", 1)]
        );
    }

    #[test]
    fn only_this_namespaces_problems_need_attention() {
        assert_eq!(page().attention.len(), 2);
    }

    #[test]
    fn issues_use_the_resource_diagnosis_and_fall_back_to_the_problem_message() {
        let mut detail = detail();
        let row = crate::resources::Resource {
            namespace: NS.into(),
            name: "checkout-api".into(),
            uid: crate::Uid::default(),
            status: Status::Err,
            cells: Vec::new(),
            age: Duration::ZERO,
            yaml: String::new(),
            labels: BTreeMap::new(),
            conditions: Vec::new(),
            diagnosis: Some(crate::Diagnosis::new("CrashLoopBackOff", "back-off 5m0s")),
        };
        detail.resources.insert(ResourceKind::Deployment, vec![row]);
        let mut service = problem(NS, Status::Warn);
        service.target.kind = "Service".into();
        service.reason = "EndpointsNotReady".into();
        service.message = "1 of 3 endpoints not ready".into();
        let problems = [problem(NS, Status::Err), service];
        let page = NamespacePage::build(&target(), Some(&detail), &problems);
        let issues: Vec<(&str, &str, &str)> = page
            .issues
            .iter()
            .map(|i| {
                (
                    i.issue.reason.as_str(),
                    i.issue.message.as_str(),
                    i.issue.object.as_str(),
                )
            })
            .collect();
        assert_eq!(
            issues,
            [
                (
                    "CrashLoopBackOff",
                    "back-off 5m0s",
                    "Deployment checkout-api · 3h"
                ),
                (
                    "EndpointsNotReady",
                    "1 of 3 endpoints not ready",
                    "Service checkout-api · 3h"
                ),
            ]
        );
    }

    #[test]
    fn unready_services_of_this_namespace_join_what_needs_attention() {
        let mut service = problem(NS, Status::Warn);
        service.target.kind = "Service".into();
        let mut elsewhere = problem("orders", Status::Warn);
        elsewhere.target.kind = "Service".into();
        let mut detail = detail();
        detail.overview.unready_services = vec![service.clone(), elsewhere];
        let page = NamespacePage::build(&target(), Some(&detail), &[]);
        assert_eq!(page.attention, [service]);
        assert_eq!(page.unready_services, 1);
        let services = page.tiles().pop().map(|tile| labels(&tile));
        assert_eq!(
            services,
            Some(vec![
                (Status::Ok, "0 ready".to_owned()),
                (Status::Warn, "1 endpoints not ready".to_owned())
            ])
        );
    }

    type TileRow = (Page, usize, Vec<(Status, String)>);

    fn rows(page: &NamespacePage) -> Vec<TileRow> {
        page.tiles()
            .iter()
            .map(|tile| (tile.page, tile.total, labels(tile)))
            .collect()
    }

    fn labels(tile: &Tile) -> Vec<(Status, String)> {
        tile.parts
            .iter()
            .map(|part| (part.status, part.label.clone()))
            .collect()
    }

    #[test]
    fn tiles_count_pods_deployments_stateful_sets_and_services_in_their_own_terms() {
        let page = page();
        let tiles = rows(&page);
        assert_eq!(
            tiles,
            [
                (
                    Page::Pods,
                    3,
                    vec![
                        (Status::Ok, "2 running".to_owned()),
                        (Status::Err, "1 failing".to_owned())
                    ]
                ),
                (
                    Page::Deployments,
                    2,
                    vec![
                        (Status::Ok, "1 available".to_owned()),
                        (Status::Err, "1 unavailable".to_owned())
                    ]
                ),
                (
                    Page::StatefulSets,
                    1,
                    vec![(Status::Ok, "1 ready".to_owned())]
                ),
                (Page::Services, 1, vec![(Status::Ok, "1 ready".to_owned())]),
            ]
        );
        assert_eq!(page.meta().as_deref(), Some("created 214d ago"));
        assert_eq!(
            page.managed_by.map(|owner| (owner.kind, owner.name)),
            Some(("Kustomization".to_owned(), "apps-payments".to_owned()))
        );
    }

    #[test]
    fn an_empty_namespace_still_shows_every_tile_at_zero() {
        let page = NamespacePage::default();
        let tiles = rows(&page);
        assert_eq!(
            tiles,
            [
                (Page::Pods, 0, vec![(Status::Ok, "0 running".to_owned())]),
                (
                    Page::Deployments,
                    0,
                    vec![(Status::Ok, "0 available".to_owned())]
                ),
                (
                    Page::StatefulSets,
                    0,
                    vec![(Status::Ok, "0 ready".to_owned())]
                ),
                (Page::Services, 0, vec![(Status::Ok, "0 ready".to_owned())]),
            ]
        );
    }

    #[test]
    fn contents_skip_empty_kinds_and_summarise_each_group() {
        let page = page();
        let shown = page.contents("");
        let titles: Vec<&str> = shown.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(
            titles,
            ["Deployments", "StatefulSets", "Pods", "Services", "Secrets"]
        );
        let summaries: Vec<String> = shown.iter().map(|g| page.summary(g).1).collect();
        assert_eq!(
            summaries,
            [
                "1 available · 1 unavailable",
                "1 ready",
                "2 Running · 1 CrashLoopBackOff",
                "",
                ""
            ]
        );
        assert_eq!(page.summary(&shown[0]).0, Status::Err);
        assert_eq!(page.objects(), "8 objects");
        assert!(page.empty().contains(&"DaemonSets"));
        assert!(page.empty().contains(&"ReplicaSets"));
    }

    #[test]
    fn the_filter_keeps_matching_rows_and_drops_groups_without_any() {
        let page = page();
        let shown = page.contents("Checkout");
        let rows: Vec<(&str, usize)> = shown.iter().map(|g| (g.title.as_str(), g.total)).collect();
        assert_eq!(rows, [("Deployments", 1), ("Pods", 2), ("Services", 1)]);
    }

    #[test]
    fn recent_events_are_the_newest_few() {
        let page = page();
        let reasons: Vec<&str> = page
            .recent_events()
            .iter()
            .map(|e| e.reason.as_str())
            .collect();
        assert_eq!(reasons, ["BackOff", "Synced", "Scaled", "Created"]);
        assert_eq!(page.events.len(), 5);
    }

    #[test]
    fn usage_pairs_the_namespace_requests_with_its_quota() {
        let page = page();
        assert_eq!(
            page.cpu().cpu_label().labelled(),
            "1.20 / 4.00 cores requested"
        );
        assert_eq!(
            page.memory().memory_label().labelled(),
            "2.50 GiB requested"
        );
    }

    #[test]
    fn usage_reads_requested_against_the_quota() {
        let cpu = Requested {
            requested: 1.2,
            quota: Some(4.),
        };
        assert_eq!(cpu.cpu_label().labelled(), "1.20 / 4.00 cores requested");
        assert_eq!(cpu.share(), Some(0.3));
        let memory = Requested {
            requested: 2.6,
            quota: None,
        };
        assert_eq!(memory.memory_label().labelled(), "2.60 GiB requested");
        assert_eq!(memory.share(), None);
        let small = Requested {
            requested: 0.12,
            quota: Some(0.5),
        };
        assert_eq!(small.cpu_label().labelled(), "0.12 / 0.50 cores requested");
        let little = Requested {
            requested: 0.0625,
            quota: None,
        };
        assert_eq!(little.memory_label().labelled(), "0.06 GiB requested");
        let over = Requested {
            requested: 5.,
            quota: Some(4.),
        };
        assert_eq!(over.share(), Some(1.));
    }

    #[test]
    fn quotas_read_as_one_line_and_forbidden_listings_say_why() {
        let quota = Quota {
            name: "payments-quota".into(),
            items: vec![QuotaItem {
                resource: "pods".into(),
                used: "9".into(),
                hard: "30".into(),
            }],
        };
        assert_eq!(quota.line(), "payments-quota · pods 9/30");
        let cpu = |resource: &str| QuotaItem {
            resource: resource.into(),
            used: "1200m".into(),
            hard: "4".into(),
        };
        assert_eq!(cpu("requests.cpu").label(), "requests.cpu 1.20/4.00 cores");
        assert_eq!(cpu("cpu").label(), "cpu 1.20/4.00 cores");
        assert_eq!(cpu("requests.memory").label(), "requests.memory 1200m/4");
        let denied: Listing<Quota> = Listing {
            items: Vec::new(),
            state: WatchState::Failed("forbidden".into()),
        };
        assert_eq!(denied.unavailable(), Some("forbidden"));
        assert!(!denied.loading());
        assert!(Listing::<Quota>::default().loading());
    }
}
