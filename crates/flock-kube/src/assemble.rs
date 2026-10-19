use std::collections::BTreeMap;
use std::sync::Arc;

use flock_core::home::{ClusterMetrics, Problem, problem_counts};
use flock_core::overview::{Overview, TimeRange, Utilization, UtilizationSource};
use flock_core::{
    ClusterDetail, ClusterId, ClusterSummary, Counts, Page, ResourceKind, ResourceRef,
};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use k8s_openapi::api::core::v1::{Event, Namespace, Node, Pod};

use crate::detail::{self, PodIndex, capacity};
use crate::flux::{self, FluxHealth};
use crate::lists::{self, watched_lists};
use crate::manager::{ClusterHandle, ConnectionStatus};
use crate::metrics::{NodeUsage, PodUsage};
use crate::probe::PrometheusTarget;
use crate::problems::{connection_problem, node_problems, pod_problems};
use crate::resources::WatchKind;
use crate::snapshot::{Facts, count, node_counts, page_kind, pod_counts, summarize};
use crate::usage::{UsageRing, sample};

const PERCENT: f32 = 100.;
const METRICS_SERVER: &str = "metrics-server";
const NO_METRICS: &str = "unavailable";
const PROMETHEUS: &str = "Prometheus";
const SINCE_FORMAT: &str = "%b %-d, %H:%M";

fn since_label(at: Timestamp, zone: &TimeZone) -> String {
    at.to_zoned(zone.clone()).strftime(SINCE_FORMAT).to_string()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Prometheus {
    pub target: Option<PrometheusTarget>,
    pub utilization: Vec<Utilization>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Extras {
    pub node_usage: Option<Vec<NodeUsage>>,
    pub pod_usage: Option<Vec<PodUsage>>,
    pub ring: UsageRing,
    pub prometheus: Option<Prometheus>,
}

impl Extras {
    pub fn record(&mut self, nodes: &[Arc<Node>], usage: Option<Vec<NodeUsage>>) {
        if let Some(sample) = usage.as_deref().and_then(|usage| sample(nodes, usage)) {
            self.ring.push(Timestamp::now(), sample);
        }
        self.node_usage = usage;
    }
}

pub struct Selection<'a> {
    pub pod_rows: bool,
    pub event_limit: usize,
    pub yaml: &'a [&'a (ResourceRef, String)],
}

pub struct Built {
    pub summary: ClusterSummary,
    pub pods: Vec<flock_core::pods::Pod>,
    pub problems: Vec<Problem>,
    pub flux: Counts,
    pub detail: Option<ClusterDetail>,
}

fn reachable(status: &ConnectionStatus) -> bool {
    matches!(
        status,
        ConnectionStatus::Connected | ConnectionStatus::Degraded(_)
    )
}

pub fn cluster(
    id: &ClusterId,
    handle: &ClusterHandle,
    extras: &Extras,
    flux: Option<&[FluxHealth]>,
    selection: Option<&Selection<'_>>,
    now: Timestamp,
) -> Built {
    let connection = handle.status();
    let pods = handle.objects::<Pod>();
    let nodes = handle.objects::<Node>();
    let objects = flux.unwrap_or_default();
    let mut problems = pod_problems(id, &pods, now);
    problems.extend(node_problems(id, &nodes, now));
    problems.extend(flux::problems(id, objects));
    problems.extend(connection_problem(id, &connection));
    let flux_counts = flux.map(flux::counts).unwrap_or_default();
    let percents = extras.ring.latest().map(|s| s.percents());
    let pod_tally = pod_counts(&pods);
    let metrics = reachable(&connection).then(|| ClusterMetrics {
        pods: pod_tally,
        cpu_percent: percents.map(|(cpu, _)| cpu),
        memory_percent: percents.map(|(_, memory)| memory),
        flux: flux_counts,
        revision: flux::revision(objects),
    });
    let index = PodIndex::build(&pods, now);
    let events = handle.objects::<Event>();
    let facts = Facts {
        problems: problem_counts(&problems),
        metrics,
        nodes: &nodes,
        nav: nav(handle, &nodes, pod_tally, &events, &index),
    };
    let summary = summarize(id.clone(), id.to_string(), handle, facts);
    let pod_rows = if selection.is_some_and(|selection| selection.pod_rows) {
        crate::pods::pods(
            id,
            &pods,
            &events,
            extras.pod_usage.as_deref().unwrap_or_default(),
            now,
        )
    } else {
        Vec::new()
    };
    let detail = selection.map(|selection| {
        let parts = Parts {
            nodes: &nodes,
            events: &events,
            index: &index,
        };
        cluster_detail(handle, extras, &parts, selection, now)
    });
    Built {
        summary,
        pods: pod_rows,
        problems,
        flux: Counts {
            total: None,
            ..flux_counts
        },
        detail,
    }
}

fn nav(
    handle: &ClusterHandle,
    nodes: &[Arc<Node>],
    pods: Counts,
    events: &[Arc<Event>],
    index: &PodIndex,
) -> BTreeMap<Page, Counts> {
    let synced = |kind: &WatchKind| handle.watch_state(kind).is_loaded();
    let mut nav = BTreeMap::new();
    if synced(&WatchKind::Nodes) {
        nav.insert(Page::Nodes, node_counts(nodes));
    }
    if synced(&WatchKind::Pods) {
        nav.insert(Page::Pods, Counts { warn: 0, ..pods });
    }
    if synced(&WatchKind::Namespaces) {
        let namespaces = handle.objects::<Namespace>().len();
        nav.insert(Page::Namespaces, Counts::total(count(namespaces)));
    }
    if synced(&WatchKind::Events) {
        nav.insert(Page::Events, detail::event_counts(events));
    }
    for kind in ResourceKind::ALL {
        let page = kind.page();
        if page_kind(page).is_some_and(|watched| synced(&watched)) {
            nav.insert(page, lists::counts(handle, kind, index));
        }
    }
    nav
}

struct Parts<'a> {
    nodes: &'a [Arc<Node>],
    events: &'a [Arc<Event>],
    index: &'a PodIndex,
}

fn cluster_detail(
    handle: &ClusterHandle,
    extras: &Extras,
    parts: &Parts<'_>,
    selection: &Selection<'_>,
    now: Timestamp,
) -> ClusterDetail {
    let Parts {
        nodes,
        events,
        index,
    } = *parts;
    let (cpu, memory) = index.commitments(capacity(nodes));
    let usage = extras.node_usage.as_deref().unwrap_or_default();
    let requests = cpu.requests * PERCENT;
    let (metrics_source, utilization_source, utilization) = extras.prometheus.as_ref().map_or_else(
        || local_history(extras, now, requests),
        |prometheus| prometheus_history(prometheus, requests),
    );
    let mut resources = watched_lists(handle, index, now);
    let events = detail::events(events, selection.event_limit, now);
    flock_core::resources::explain(&mut resources, &events);
    for (target, yaml) in selection.yaml.iter().copied() {
        let row = resources
            .iter_mut()
            .filter(|(kind, _)| kind.name() == target.kind)
            .flat_map(|(_, rows)| rows.iter_mut())
            .find(|row| {
                Some(&row.namespace) == target.namespace.as_ref() && row.name == target.name
            });
        if let Some(row) = row {
            row.yaml.clone_from(yaml);
        }
    }
    ClusterDetail {
        overview: Overview {
            metrics_source,
            utilization_source,
            cpu,
            memory,
            pods: index.phases,
            utilization,
        },
        nodes: detail::nodes(nodes, index, usage, now),
        namespaces: detail::namespaces(
            &handle.objects::<Namespace>(),
            index,
            extras.pod_usage.as_deref(),
            now,
        ),
        events,
        resources,
    }
}

type History = (String, UtilizationSource, Vec<Utilization>);

fn prometheus_history(prometheus: &Prometheus, requests: f32) -> History {
    let ranges = prometheus
        .utilization
        .iter()
        .map(|range| Utilization {
            cpu_requests: requests,
            ..range.clone()
        })
        .collect();
    let source = prometheus
        .error
        .as_ref()
        .map_or(UtilizationSource::Prometheus, |reason| {
            UtilizationSource::Error(reason.clone())
        });
    let label = prometheus
        .target
        .as_ref()
        .map_or_else(|| PROMETHEUS.to_owned(), PrometheusTarget::label);
    (label, source, ranges)
}

fn local_history(extras: &Extras, now: Timestamp, requests: f32) -> History {
    let ranges = TimeRange::ALL
        .into_iter()
        .map(|range| extras.ring.utilization(range, now, requests))
        .collect();
    let label = if extras.node_usage.is_some() {
        METRICS_SERVER
    } else {
        NO_METRICS
    };
    let since = extras
        .ring
        .since()
        .map(|since| since_label(since, &TimeZone::system()));
    (label.to_owned(), UtilizationSource::Local { since }, ranges)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_start_reads_as_a_short_local_time() -> crate::Result<()> {
        let at: Timestamp = "2026-10-14T09:05:00Z"
            .parse()
            .map_err(|_| crate::Error::MetricsHistory("timestamp"))?;
        assert_eq!(since_label(at, &TimeZone::UTC), "Oct 14, 09:05");
        Ok(())
    }
}
