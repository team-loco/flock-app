use std::collections::BTreeMap;
use std::time::Duration;

use crate::diagnosis::{self, Condition, Diagnosis};
use crate::events::Event;
use crate::filter::{StatusFilter, TextFilter};
use crate::{Page, Status, Uid};

const READY_COLUMN: &str = "Ready";
const STATUS_COLUMN: &str = "Status";
const RATIO: char = '/';

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResourceKind {
    Deployment,
    StatefulSet,
    DaemonSet,
    Job,
    CronJob,
    Service,
    Ingress,
    ConfigMap,
    Secret,
    PersistentVolumeClaim,
}

impl ResourceKind {
    pub const ALL: [Self; 10] = [
        Self::Deployment,
        Self::StatefulSet,
        Self::DaemonSet,
        Self::Job,
        Self::CronJob,
        Self::Service,
        Self::Ingress,
        Self::ConfigMap,
        Self::Secret,
        Self::PersistentVolumeClaim,
    ];

    pub const fn page(self) -> Page {
        match self {
            Self::Deployment => Page::Deployments,
            Self::StatefulSet => Page::StatefulSets,
            Self::DaemonSet => Page::DaemonSets,
            Self::Job => Page::Jobs,
            Self::CronJob => Page::CronJobs,
            Self::Service => Page::Services,
            Self::Ingress => Page::Ingresses,
            Self::ConfigMap => Page::ConfigMaps,
            Self::Secret => Page::Secrets,
            Self::PersistentVolumeClaim => Page::Pvcs,
        }
    }

    pub fn for_page(page: Page) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.page() == page)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Deployment => "Deployment",
            Self::StatefulSet => "StatefulSet",
            Self::DaemonSet => "DaemonSet",
            Self::Job => "Job",
            Self::CronJob => "CronJob",
            Self::Service => "Service",
            Self::Ingress => "Ingress",
            Self::ConfigMap => "ConfigMap",
            Self::Secret => "Secret",
            Self::PersistentVolumeClaim => "PersistentVolumeClaim",
        }
    }

    pub const fn columns(self) -> &'static [&'static str] {
        match self {
            Self::Deployment => &["Ready", "Up-to-date", "Available", "Image"],
            Self::StatefulSet => &["Ready", "Service", "Image"],
            Self::DaemonSet => &["Desired", "Ready", "Node selector", "Image"],
            Self::Job => &["Completions", "Duration", "Status"],
            Self::CronJob => &["Schedule", "Suspend", "Active", "Last schedule"],
            Self::Service => &["Type", "Cluster IP", "Ports"],
            Self::Ingress => &["Class", "Hosts", "Address"],
            Self::ConfigMap => &["Keys"],
            Self::Secret => &["Type", "Keys"],
            Self::PersistentVolumeClaim => &["Status", "Capacity", "Storage class", "Volume"],
        }
    }

    pub fn status_cell(self, row: &Resource) -> Option<String> {
        let columns = self.columns();
        let ready = columns.iter().position(|c| *c == READY_COLUMN);
        let ready = ready
            .and_then(|ix| row.cells.get(ix))
            .filter(|cell| cell.contains(RATIO))
            .map(|cell| format!("{cell} ready"));
        let status = || {
            let ix = columns.iter().position(|c| *c == STATUS_COLUMN)?;
            row.cells.get(ix).filter(|cell| !cell.is_empty()).cloned()
        };
        ready.or_else(status)
    }

    pub const fn is_workload(self) -> bool {
        matches!(
            self,
            Self::Deployment | Self::StatefulSet | Self::DaemonSet | Self::Job | Self::CronJob
        )
    }
}

pub const POD_LIST: &str = "Pod";
pub const EVENT_LIST: &str = "Event";

pub fn namespace_choices(
    known: impl IntoIterator<Item = String>,
    chosen: Option<&str>,
) -> Vec<String> {
    let mut names: Vec<String> = known.into_iter().chain(chosen.map(str::to_owned)).collect();
    names.sort();
    names.dedup();
    names
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    pub namespace: String,
    pub name: String,
    pub uid: Uid,
    pub status: Status,
    pub cells: Vec<String>,
    pub age: Duration,
    pub yaml: String,
    pub labels: BTreeMap<String, String>,
    pub conditions: Vec<Condition>,
    pub diagnosis: Option<Diagnosis>,
}

impl Resource {
    pub fn events<'a>(&self, kind: ResourceKind, events: &'a [Event]) -> Vec<&'a Event> {
        diagnosis::about(events, kind.name(), Some(&self.namespace), &self.name)
    }

    pub fn matches(&self, namespace: Option<&str>, query: &TextFilter) -> bool {
        let in_namespace = namespace.is_none_or(|ns| ns == self.namespace);
        let fields = [self.name.as_str(), self.namespace.as_str()];
        in_namespace && query.matches_labeled(Some(&self.namespace), &self.labels, &fields)
    }
}

pub fn filter_resources(
    rows: &[Resource],
    namespace: Option<&str>,
    query: &TextFilter,
) -> Vec<Resource> {
    rows.iter()
        .filter(|row| row.matches(namespace, query))
        .cloned()
        .collect()
}

pub fn explain(resources: &mut BTreeMap<ResourceKind, Vec<Resource>>, events: &[Event]) {
    for (kind, rows) in resources.iter_mut() {
        for row in rows.iter_mut() {
            if !diagnosis::needs_diagnosis(row.status) {
                row.diagnosis = None;
                continue;
            }
            let about = row.events(*kind, events);
            row.diagnosis = diagnosis::with_events(row.diagnosis.take(), &about);
        }
    }
}

pub fn filter_counts(rows: &[Resource]) -> Vec<(StatusFilter, usize)> {
    StatusFilter::counts(rows.iter().map(|row| row.status))
}

pub fn namespaces_of(rows: &[Resource]) -> Vec<String> {
    let mut names: Vec<String> = rows.iter().map(|row| row.namespace.clone()).collect();
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn resource(namespace: &str, name: &str, status: Status) -> Resource {
        Resource {
            uid: Uid::default(),
            namespace: namespace.into(),
            name: name.into(),
            status,
            cells: Vec::new(),
            age: Duration::ZERO,
            yaml: String::new(),
            labels: BTreeMap::new(),
            conditions: Vec::new(),
            diagnosis: None,
        }
    }

    #[test]
    fn explain_drops_healthy_reasons_and_fills_messages_from_events() {
        use crate::events::EventDetails;
        let mut pending = resource("db", "data-0", Status::Warn);
        pending.diagnosis = Some(Diagnosis::new("Pending", ""));
        let mut calm = resource("db", "data-1", Status::Ok);
        calm.diagnosis = Some(Diagnosis::new("Stale", ""));
        let event = Event {
            labels: BTreeMap::new(),
            details: EventDetails {
                kind: "PersistentVolumeClaim".into(),
                ..EventDetails::default()
            },
            status: Status::Warn,
            reason: "ProvisioningFailed".into(),
            object: "persistentvolumeclaim/data-0".into(),
            namespace: Some("db".into()),
            message: "storageclass \"fast\" not found".into(),
            age: Duration::ZERO,
            count: 1,
        };
        let mut lists =
            BTreeMap::from([(ResourceKind::PersistentVolumeClaim, vec![pending, calm])]);
        explain(&mut lists, &[event]);
        let rows = &lists[&ResourceKind::PersistentVolumeClaim];
        assert_eq!(
            rows[0].diagnosis,
            Some(Diagnosis::new(
                "Pending",
                "ProvisioningFailed: storageclass \"fast\" not found"
            ))
        );
        assert_eq!(rows[1].diagnosis, None);
    }

    #[test]
    fn label_filters_match_resource_metadata() {
        let mut row = resource("payments", "checkout", Status::Ok);
        row.labels.insert("app".into(), "cart".into());
        assert!(row.matches(None, &TextFilter::parse("checkout ns:pay label:app=cart")));
        assert!(row.matches(None, &TextFilter::parse("l:app")));
        assert!(!row.matches(None, &TextFilter::parse("l:app=web")));
        assert!(!row.matches(Some("web"), &TextFilter::parse("l:app")));
    }

    #[test]
    fn every_kind_maps_to_its_own_page() {
        for kind in ResourceKind::ALL {
            assert_eq!(ResourceKind::for_page(kind.page()), Some(kind));
            assert_ne!(kind.columns().len(), 0);
        }
        assert_eq!(ResourceKind::for_page(Page::Nodes), None);
    }

    #[test]
    fn filter_matches_namespace_and_text() {
        let rows = vec![
            resource("payments", "checkout-api", Status::Err),
            resource("payments", "payments-worker", Status::Ok),
            resource("web", "web-frontend", Status::Ok),
        ];
        let names = |found: Vec<Resource>| found.into_iter().map(|r| r.name).collect::<Vec<_>>();
        assert_eq!(
            names(filter_resources(
                &rows,
                Some("payments"),
                &TextFilter::default()
            )),
            ["checkout-api", "payments-worker"]
        );
        assert_eq!(
            names(filter_resources(&rows, None, &TextFilter::parse(" WEB "))),
            ["web-frontend"]
        );
        assert_eq!(
            names(filter_resources(&rows, None, &TextFilter::parse("pay"))),
            ["checkout-api", "payments-worker"]
        );
        assert_eq!(
            names(filter_resources(
                &rows,
                Some("web"),
                &TextFilter::parse("checkout")
            )),
            [""; 0]
        );
        assert_eq!(
            names(filter_resources(&rows, None, &TextFilter::parse("ns:web"))),
            ["web-frontend"]
        );
        let counts: Vec<usize> = filter_counts(&rows).into_iter().map(|(_, n)| n).collect();
        assert_eq!(counts, [3, 1]);
    }

    #[test]
    fn namespace_choices_include_the_pick_even_before_it_loads() {
        let known = ["web".to_owned(), "payments".to_owned(), "web".to_owned()];
        assert_eq!(
            namespace_choices(known.clone(), Some("loco-system")),
            ["loco-system", "payments", "web"]
        );
        assert_eq!(namespace_choices(known, None), ["payments", "web"]);
    }

    #[test]
    fn namespaces_are_sorted_and_unique() {
        let rows = vec![
            resource("web", "a", Status::Ok),
            resource("payments", "b", Status::Ok),
            resource("web", "c", Status::Ok),
        ];
        assert_eq!(namespaces_of(&rows), ["payments", "web"]);
    }
}
