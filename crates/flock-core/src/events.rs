use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::time::Duration;

use crate::filter::{FilterSpec, Labels, Searchable};
use crate::{ClusterId, Command, Counts, FluxKey, FluxKind, ResourceRef, Status};

pub const FLUX_EVENT_KINDS: [&str; 6] = [
    "Kustomization",
    "HelmRelease",
    "GitRepository",
    "HelmRepository",
    "OCIRepository",
    "HelmChart",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub labels: BTreeMap<String, String>,
    pub details: EventDetails,
    pub status: Status,
    pub reason: String,
    pub object: String,
    pub namespace: Option<String>,
    pub message: String,
    pub age: Duration,
    pub count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventDetails {
    pub uid: String,
    pub kind: String,
    pub object_uid: Option<String>,
    pub object_namespace: Option<String>,
    pub event_type: String,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    pub first_age: Option<Duration>,
    pub source_component: String,
    pub source_host: String,
}

impl Event {
    pub fn target(&self, cluster: &ClusterId) -> Option<ResourceRef> {
        let (kind, name) = self.object.split_once('/')?;
        if name.is_empty() || kind.is_empty() {
            return None;
        }
        let kind = if self.details.kind.is_empty() {
            crate::resource::KINDS
                .iter()
                .find(|info| info.kind.eq_ignore_ascii_case(kind))
                .map_or(kind, |info| info.kind)
        } else {
            &self.details.kind
        };
        let namespace = match kind {
            "Node" => None,
            "Namespace" => None,
            _ => self
                .details
                .object_namespace
                .clone()
                .or_else(|| self.namespace.clone()),
        };
        Some(ResourceRef {
            cluster: cluster.clone(),
            kind: kind.into(),
            namespace,
            name: name.into(),
        })
    }

    pub fn is_about(&self, kind: &str, namespace: Option<&str>, name: &str) -> bool {
        let Some((object_kind, object_name)) = self.object.split_once('/') else {
            return false;
        };
        let event_kind = if self.details.kind.is_empty() {
            object_kind
        } else {
            &self.details.kind
        };
        let event_namespace = self
            .details
            .object_namespace
            .as_deref()
            .or(self.namespace.as_deref());
        let same_namespace = namespace.is_none_or(|ns| event_namespace == Some(ns));
        object_name == name && event_kind.eq_ignore_ascii_case(kind) && same_namespace
    }

    pub fn same_object(&self, other: &Self) -> bool {
        match (&self.details.object_uid, &other.details.object_uid) {
            (Some(left), Some(right)) => left == right,
            _ => self.object == other.object && self.namespace == other.namespace,
        }
    }

    pub fn key(&self) -> String {
        if !self.details.uid.is_empty() {
            return self.details.uid.clone();
        }
        format!(
            "{:?}/{}/{}/{}",
            self.namespace, self.object, self.reason, self.message
        )
    }

    pub fn is_warning(&self) -> bool {
        self.status != Status::Ok
    }
}

impl Searchable for Event {
    const FILTER: FilterSpec<Self> = FilterSpec {
        fields: &[
            ("reason", |e, (), out| out.push(e.reason.as_str().into())),
            ("object", |e, (), out| out.push(e.object.as_str().into())),
            ("message", |e, (), out| out.push(e.message.as_str().into())),
            ("namespace", |e, (), out| {
                out.push(e.namespace.as_deref().unwrap_or_default().into());
            }),
        ],
        facets: &[("reason", |e, (), out| out.push(e.reason.as_str().into()))],
        namespace: true,
        labels: true,
    };

    fn namespace(&self) -> Option<&str> {
        self.namespace.as_deref()
    }

    fn labels(&self) -> Option<Cow<'_, Labels>> {
        Some(Cow::Borrowed(&self.labels))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EventFilter {
    #[default]
    All,
    Warnings,
    Normal,
}

impl EventFilter {
    pub const ALL: [Self; 3] = [Self::All, Self::Warnings, Self::Normal];

    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Warnings => "Warnings",
            Self::Normal => "Normal",
        }
    }

    pub fn keeps(self, event: &Event) -> bool {
        match self {
            Self::All => true,
            Self::Warnings => event.is_warning(),
            Self::Normal => !event.is_warning(),
        }
    }
}

pub fn filter_counts<'a>(events: impl IntoIterator<Item = &'a Event>) -> Vec<(EventFilter, usize)> {
    let matching: Vec<&Event> = events.into_iter().collect();
    EventFilter::ALL
        .iter()
        .map(|filter| (*filter, matching.iter().filter(|e| filter.keeps(e)).count()))
        .collect()
}

fn warnings_order(a: &Event, b: &Event) -> Ordering {
    b.is_warning()
        .cmp(&a.is_warning())
        .then(b.status.cmp(&a.status))
        .then(a.age.cmp(&b.age))
}

pub fn warnings_first(events: &[Event]) -> Vec<Event> {
    let mut sorted = events.to_vec();
    sorted.sort_by(warnings_order);
    sorted
}

pub fn counts(events: &[Event]) -> Counts {
    let flagged = Counts::of(events.iter().map(|e| e.status));
    Counts {
        total: None,
        ..flagged
    }
}

pub fn in_namespace(
    events: &[Event],
    candidates: impl IntoIterator<Item = usize>,
    namespace: Option<&str>,
) -> Vec<usize> {
    candidates
        .into_iter()
        .filter(|&ix| {
            events.get(ix).is_some_and(|event| {
                namespace.is_none_or(|namespace| event.namespace.as_deref() == Some(namespace))
            })
        })
        .collect()
}

pub fn filter_events(events: &[Event], matched: &[usize], filter: EventFilter) -> Vec<usize> {
    let mut kept: Vec<(usize, &Event)> = matched
        .iter()
        .filter_map(|&ix| events.get(ix).map(|event| (ix, event)))
        .filter(|(_, event)| filter.keeps(event))
        .collect();
    kept.sort_by(|(_, a), (_, b)| warnings_order(a, b));
    kept.into_iter().map(|(ix, _)| ix).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterEvent {
    pub cluster: ClusterId,
    pub cluster_name: String,
    pub event: Event,
}

impl ClusterEvent {
    pub fn kind(&self) -> &str {
        &self.event.details.kind
    }

    pub fn name(&self) -> &str {
        self.event
            .object
            .split_once('/')
            .map_or(self.event.object.as_str(), |(_, name)| name)
    }

    pub fn key(&self) -> String {
        format!("{}/{}", self.cluster.as_str(), self.event.key())
    }

    pub fn open(&self) -> Option<Command> {
        let target = self.event.target(&self.cluster)?;
        let kind = FluxKind::from_label(&target.kind)?;
        let namespace = target.namespace?;
        let key = FluxKey::new(self.cluster.as_str(), kind, namespace, target.name);
        Some(Command::OpenFluxObject(key))
    }
}

impl Searchable for ClusterEvent {
    const FILTER: FilterSpec<Self> = FilterSpec {
        fields: &[
            ("cluster", |e, (), out| {
                out.push(e.cluster_name.as_str().into());
            }),
            ("kind", |e, (), out| out.push(e.kind().into())),
            ("reason", |e, (), out| {
                out.push(e.event.reason.as_str().into());
            }),
            ("name", |e, (), out| out.push(e.name().into())),
            ("message", |e, (), out| {
                out.push(e.event.message.as_str().into());
            }),
            ("namespace", |e, (), out| {
                out.push(e.event.namespace.as_deref().unwrap_or_default().into());
            }),
        ],
        facets: &[
            ("cluster", |e, (), out| {
                out.push(e.cluster_name.as_str().into());
            }),
            ("kind", |e, (), out| out.push(e.kind().into())),
            ("reason", |e, (), out| {
                out.push(e.event.reason.as_str().into());
            }),
        ],
        namespace: true,
        labels: true,
    };

    fn namespace(&self) -> Option<&str> {
        self.event.namespace.as_deref()
    }

    fn labels(&self) -> Option<Cow<'_, Labels>> {
        Some(Cow::Borrowed(&self.event.labels))
    }
}

fn newest_order(a: &ClusterEvent, b: &ClusterEvent) -> Ordering {
    a.event
        .age
        .cmp(&b.event.age)
        .then_with(|| a.cluster.cmp(&b.cluster))
        .then_with(|| a.event.key().cmp(&b.event.key()))
}

pub fn newest_first(events: &mut [ClusterEvent]) {
    events.sort_by(newest_order);
}

pub fn filter_cluster_events(
    events: &[ClusterEvent],
    matched: &[usize],
    filter: EventFilter,
) -> Vec<usize> {
    let mut kept: Vec<(usize, &ClusterEvent)> = matched
        .iter()
        .filter_map(|&ix| events.get(ix).map(|event| (ix, event)))
        .filter(|(_, event)| filter.keeps(&event.event))
        .collect();
    kept.sort_by(|(_, a), (_, b)| newest_order(a, b));
    kept.into_iter().map(|(ix, _)| ix).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::{Search, TextFilter};

    const MINUTE: u64 = 60;

    fn event(status: Status, reason: &str, minutes: u64) -> Event {
        Event {
            labels: BTreeMap::new(),
            details: EventDetails::default(),
            status,
            reason: reason.into(),
            object: format!("pod/{}", reason.to_lowercase()),
            namespace: Some("payments".into()),
            message: format!("{reason} happened"),
            age: Duration::from_secs(minutes * MINUTE),
            count: 1,
        }
    }

    fn reasons(events: &[Event]) -> Vec<&str> {
        events.iter().map(|e| e.reason.as_str()).collect()
    }

    fn filtered<'a>(events: &'a [Event], filter: EventFilter, query: &str) -> Vec<&'a str> {
        let matched = Search::default().matching(events, &TextFilter::parse(query));
        filter_events(events, &matched, filter)
            .into_iter()
            .map(|ix| events[ix].reason.as_str())
            .collect()
    }

    #[test]
    fn list_label_tokens_match_metadata() {
        let mut event = event(Status::Ok, "Created", 1);
        event.labels.insert("team".into(), "checkout".into());
        assert!(event.matches(&TextFilter::parse("label:team=checkout")));
        assert!(!event.matches(&TextFilter::parse("l:team=platform")));
    }

    #[test]
    fn namespace_filter_keeps_all_explicit_and_empty_namespaces_empty() {
        let mut rows = vec![event(Status::Warn, "BackOff", 1)];
        let mut other = rows[0].clone();
        other.namespace = Some("default".into());
        rows.push(other);
        assert_eq!(in_namespace(&rows, 0..rows.len(), None), [0, 1]);
        assert_eq!(in_namespace(&rows, 0..rows.len(), Some("payments")), [0]);
        assert_eq!(in_namespace(&rows, [1], Some("payments")), [0_usize; 0]);
        assert_eq!(
            in_namespace(&rows, 0..rows.len(), Some("missing")),
            [0_usize; 0]
        );
    }

    #[test]
    fn object_links_use_canonical_kinds_and_preserve_namespace() {
        let mut row = event(Status::Warn, "BackOff", 1);
        let cluster = ClusterId::new("dev");
        let target = row.target(&cluster).unwrap_or_else(|| panic!("target"));
        assert_eq!(target.kind, "Pod");
        assert_eq!(target.namespace.as_deref(), Some("payments"));
        assert_eq!(target.name, "backoff");
        row.object = "node/worker".into();
        assert_eq!(
            row.target(&cluster).and_then(|target| target.namespace),
            None
        );
        row.object = "pod/".into();
        assert!(row.target(&cluster).is_none());
    }

    #[test]
    fn object_identity_distinguishes_namespaces_and_recreated_objects() {
        let row = event(Status::Warn, "BackOff", 1);
        let mut other = row.clone();
        other.reason = "Pulled".into();
        assert!(row.same_object(&other));
        other.namespace = Some("default".into());
        assert!(!row.same_object(&other));
        let mut row = row;
        row.details.object_uid = Some("old".into());
        other = row.clone();
        other.details.object_uid = Some("new".into());
        assert!(!row.same_object(&other));
    }

    #[test]
    fn stable_uid_keeps_selection_when_message_and_count_change() {
        let mut row = event(Status::Warn, "BackOff", 1);
        row.details.uid = "event-uid".into();
        let mut updated = row.clone();
        updated.count += 1;
        updated.message = "retrying".into();
        assert_eq!(row.key(), updated.key());
    }

    #[test]
    fn warnings_come_first_errors_before_warnings_then_newest() {
        let events = [
            event(Status::Ok, "Scheduled", 1),
            event(Status::Warn, "Unhealthy", 11),
            event(Status::Err, "BackOff", 2),
            event(Status::Warn, "FailedMount", 6),
        ];
        assert_eq!(
            reasons(&warnings_first(&events)),
            ["BackOff", "FailedMount", "Unhealthy", "Scheduled"]
        );
    }

    #[test]
    fn counts_split_errors_from_warnings() {
        let events = [
            event(Status::Ok, "Scheduled", 1),
            event(Status::Err, "BackOff", 2),
            event(Status::Warn, "FailedMount", 6),
            event(Status::Warn, "Unhealthy", 11),
        ];
        assert_eq!(counts(&events), Counts::default().with_err(1).with_warn(2));
    }

    #[test]
    fn filter_by_type_and_text() {
        let events = [
            event(Status::Ok, "Scheduled", 1),
            event(Status::Err, "BackOff", 2),
            event(Status::Warn, "FailedMount", 6),
        ];
        assert_eq!(
            filtered(&events, EventFilter::Warnings, ""),
            ["BackOff", "FailedMount"]
        );
        assert_eq!(filtered(&events, EventFilter::Normal, ""), ["Scheduled"]);
        assert_eq!(
            filtered(&events, EventFilter::All, "mount"),
            ["FailedMount"]
        );
        assert_eq!(filtered(&events, EventFilter::Normal, "backoff"), [""; 0]);
        assert_eq!(filter_events(&events, &[2, 0], EventFilter::All), [2, 0]);
    }

    fn flux_event(cluster: &str, kind: &str, reason: &str, minutes: u64) -> ClusterEvent {
        let mut row = event(Status::Ok, reason, minutes);
        row.object = format!("{}/apps", kind.to_lowercase());
        row.namespace = Some("flux-system".into());
        row.details.kind = kind.into();
        row.details.uid = format!("{cluster}-{reason}");
        ClusterEvent {
            cluster: ClusterId::new(cluster),
            cluster_name: cluster.into(),
            event: row,
        }
    }

    #[test]
    fn flux_events_read_newest_first_and_filter_by_cluster_kind_and_warning() {
        let mut failing = flux_event("west", "HelmRelease", "UpgradeFailed", 2);
        failing.event.status = Status::Err;
        let rows = vec![
            flux_event("east", "Kustomization", "ReconciliationSucceeded", 9),
            failing,
            flux_event("east", "GitRepository", "NewArtifact", 1),
        ];
        let search = Search::default();
        let flux = |filter: EventFilter, query: &str| {
            let matched = search.matching(&rows, &TextFilter::parse(query));
            filter_cluster_events(&rows, &matched, filter)
        };
        let all = flux(EventFilter::All, "");
        let reasons: Vec<&str> = all
            .iter()
            .map(|&ix| rows[ix].event.reason.as_str())
            .collect();
        assert_eq!(
            reasons,
            ["NewArtifact", "UpgradeFailed", "ReconciliationSucceeded"]
        );
        assert_eq!(flux(EventFilter::All, "west").len(), 1);
        assert_eq!(flux(EventFilter::All, "kustomization").len(), 1);
        assert_eq!(flux(EventFilter::Warnings, "").len(), 1);
        assert_eq!(flux(EventFilter::All, "ns:flux-system ").len(), 3);
        let counts = filter_counts(rows.iter().map(|row| &row.event));
        assert_eq!(counts[1], (EventFilter::Warnings, 1));
    }

    #[test]
    fn flux_event_rows_open_their_flux_object() {
        let row = flux_event("east", "Kustomization", "ReconciliationSucceeded", 1);
        let key = FluxKey::new("east", FluxKind::Kustomization, "flux-system", "apps");
        assert_eq!(row.open(), Some(Command::OpenFluxObject(key)));
        let chart = flux_event("east", "HelmChart", "ChartPullSucceeded", 1);
        assert_eq!(chart.open(), None);
        assert_eq!(chart.name(), "apps");
    }
}
