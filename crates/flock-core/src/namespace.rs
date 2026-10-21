use std::cmp::Reverse;
use std::collections::BTreeMap;

use crate::cluster::{ClusterDetail, Namespace};
use crate::events::Event;
use crate::home::Problem;
use crate::overview::plural;
use crate::related::{Group, Related};
use crate::time::age;
use crate::units::{self, Metric};
use crate::watch::WatchState;
use crate::{Counts, Page, ResourceRef, Status};

pub const RECENT_EVENTS: usize = 4;
const SEPARATOR: &str = " · ";
const RATIO: &str = "/";
const FAILING: &str = "failing";
const DEGRADED: &str = "degraded";
const SUSPENDED: &str = "suspended";
const READY: &str = "ready";
const ALL_READY: &str = "all ready";
const REQUESTED: &str = "requested";
const WARNING_ONE: &str = "warning";
const WARNING_MANY: &str = "warnings";
const POD_ONE: &str = "pod";
const POD_MANY: &str = "pods";
const OBJECT_ONE: &str = "object";
const OBJECT_MANY: &str = "objects";
const CREATED: &str = "created";
const AGO: &str = "ago";
const EVENTS: &str = "Events";
const PODS_FACT: &str = "pods";
const FACT_PAGES: [(Page, &str); 2] = [
    (Page::Deployments, "deployments"),
    (Page::Services, "services"),
];
const READY_PAGES: [Page; 6] = [
    Page::Deployments,
    Page::StatefulSets,
    Page::DaemonSets,
    Page::Jobs,
    Page::CronJobs,
    Page::Pods,
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

    pub fn cpu_label(&self) -> String {
        self.label(Metric::Cpu)
    }

    pub fn memory_label(&self) -> String {
        self.label(Metric::Memory)
    }

    fn label(&self, metric: Metric) -> String {
        let values: Vec<Option<f32>> = std::iter::once(Some(self.requested))
            .chain(self.quota.map(Some))
            .collect();
        let amounts = units::amounts(metric, &values).labelled();
        format!("{amounts} {REQUESTED}")
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
        format!("{} {}{RATIO}{}", self.resource, self.used, self.hard)
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
pub struct HeaderFact {
    pub label: &'static str,
    pub total: usize,
    pub parts: Vec<(Status, String)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NamespacePage {
    pub target: Option<ResourceRef>,
    pub namespace: Option<Namespace>,
    pub attention: Vec<Problem>,
    pub groups: Vec<Group>,
    pub events: Vec<Event>,
    pub detail: Option<NamespaceDetail>,
    pub managed_by: Option<ResourceRef>,
}

fn counted(counts: Counts, total: usize, ready: bool) -> Vec<(Status, String)> {
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
    if parts.is_empty() && ready && total > 0 {
        parts.push((Status::Ok, ALL_READY.to_owned()));
    }
    parts
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
        let unready = detail.map(|d| d.overview.unready_services.as_slice());
        let attention = problems
            .iter()
            .chain(unready.unwrap_or_default())
            .filter(|p| {
                p.target.cluster == target.cluster && p.target.namespace.as_deref() == Some(name)
            })
            .cloned()
            .collect();
        let managed_by = namespace
            .as_ref()
            .and_then(|n| crate::flux::managed_by(&target.cluster, &n.labels));
        Self {
            target: Some(target.clone()),
            namespace,
            attention,
            groups,
            events,
            detail: detail.and_then(|d| d.namespace_detail(name)).cloned(),
            managed_by,
        }
    }

    pub fn badge(&self) -> Option<(Status, String)> {
        let counts = Counts::of(self.attention.iter().map(|p| p.status));
        if counts.err > 0 {
            return Some((Status::Err, format!("{} {FAILING}", counts.err)));
        }
        (counts.warn > 0).then(|| {
            let count = counts.warn as usize;
            (Status::Warn, plural(count, WARNING_ONE, WARNING_MANY))
        })
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

    pub fn facts(&self) -> Vec<HeaderFact> {
        let pods = self.pods();
        let pod_parts = self.pod_tally().map_or_else(
            || {
                self.group(Page::Pods)
                    .map(|g| counted(g.counts, g.total, false))
                    .unwrap_or_default()
            },
            tally_parts,
        );
        let mut facts = Vec::new();
        if pods > 0 {
            facts.push(HeaderFact {
                label: PODS_FACT,
                total: pods,
                parts: pod_parts,
            });
        }
        for (page, label) in FACT_PAGES {
            let Some(group) = self.group(page).filter(|g| g.total > 0) else {
                continue;
            };
            facts.push(HeaderFact {
                label,
                total: group.total,
                parts: counted(group.counts, group.total, false),
            });
        }
        facts
    }

    pub fn meta(&self) -> String {
        let mut parts = vec![plural(self.pods(), POD_ONE, POD_MANY)];
        if let Some(namespace) = &self.namespace {
            parts.push(format!("{CREATED} {} {AGO}", age(namespace.age)));
        }
        parts.join(SEPARATOR)
    }

    pub fn summary(&self, group: &Group) -> (Status, String) {
        let pods = self
            .pod_tally()
            .filter(|_| group.title == Page::Pods.label());
        let parts = pods.map_or_else(
            || {
                let ready = READY_PAGES.iter().any(|page| page.label() == group.title);
                counted(group.counts, group.total, ready)
            },
            tally_parts,
        );
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
            message: "2/3 ready".into(),
            age: Duration::from_hours(3),
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
    fn only_this_namespaces_problems_need_attention_and_errors_set_the_badge() {
        let page = page();
        assert_eq!(page.attention.len(), 2);
        assert_eq!(page.badge(), Some((Status::Err, "1 failing".into())));
        let quiet = NamespacePage {
            attention: vec![problem(NS, Status::Warn), problem(NS, Status::Warn)],
            ..NamespacePage::default()
        };
        assert_eq!(quiet.badge(), Some((Status::Warn, "2 warnings".into())));
        assert_eq!(NamespacePage::default().badge(), None);
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
    }

    #[test]
    fn header_facts_read_pods_by_reason_then_deployments_and_services() {
        let page = page();
        let facts: Vec<(&str, usize, Vec<String>)> = page
            .facts()
            .into_iter()
            .map(|f| (f.label, f.total, f.parts.into_iter().map(|p| p.1).collect()))
            .collect();
        assert_eq!(
            facts,
            [
                (
                    "pods",
                    3,
                    vec!["2 Running".to_owned(), "1 CrashLoopBackOff".to_owned()]
                ),
                (
                    "deployments",
                    2,
                    vec!["1 ready".to_owned(), "1 failing".to_owned()]
                ),
                ("services", 1, Vec::new()),
            ]
        );
        assert_eq!(page.meta(), "3 pods · created 214d ago");
        assert_eq!(
            page.managed_by.map(|owner| (owner.kind, owner.name)),
            Some(("Kustomization".to_owned(), "apps-payments".to_owned()))
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
                "1 ready · 1 failing",
                "all ready",
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
        assert_eq!(page.cpu().cpu_label(), "1.2 / 4.0 cores requested");
        assert_eq!(page.memory().memory_label(), "2.5 GiB requested");
    }

    #[test]
    fn usage_reads_requested_against_the_quota() {
        let cpu = Requested {
            requested: 1.2,
            quota: Some(4.),
        };
        assert_eq!(cpu.cpu_label(), "1.2 / 4.0 cores requested");
        assert_eq!(cpu.share(), Some(0.3));
        let memory = Requested {
            requested: 2.6,
            quota: None,
        };
        assert_eq!(memory.memory_label(), "2.6 GiB requested");
        assert_eq!(memory.share(), None);
        let small = Requested {
            requested: 0.12,
            quota: Some(0.5),
        };
        assert_eq!(small.cpu_label(), "120m / 500m requested");
        let little = Requested {
            requested: 0.0625,
            quota: None,
        };
        assert_eq!(little.memory_label(), "64Mi requested");
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
        let denied: Listing<Quota> = Listing {
            items: Vec::new(),
            state: WatchState::Failed("forbidden".into()),
        };
        assert_eq!(denied.unavailable(), Some("forbidden"));
        assert!(!denied.loading());
        assert!(Listing::<Quota>::default().loading());
    }
}
