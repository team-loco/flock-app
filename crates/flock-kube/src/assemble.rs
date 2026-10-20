use std::collections::BTreeMap;
use std::sync::Arc;

use flock_core::cluster_changes::ClusterChange;
use flock_core::home::{ClusterMetrics, Problem, problem_counts};
use flock_core::overview::{MetricsServer, Overview, TimeRange, Utilization, UtilizationSource};
use flock_core::{
    ClusterDetail, ClusterId, ClusterSummary, Counts, Page, ResourceKind, ResourceRef,
};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Event, Namespace, Node, PersistentVolumeClaim, Pod, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;

use crate::cluster_changes::{self, Change, Observed};
use crate::detail::{self, PodIndex, capacity};
use crate::flux::{self, FluxHealth};
use crate::lists::{self, watched_lists};
use crate::manager::{ClusterHandle, ConnectionStatus};
use crate::metrics::{NodeUsage, PodUsage};
use crate::network;
use crate::pod_tops::pod_tops;
use crate::probe::PrometheusTarget;
use crate::problems::{connection_problem, node_problems, pod_problems};
use crate::related;
use crate::resources::WatchKind;
use crate::snapshot::{Facts, count, node_counts, page_kind, pod_counts, summarize};
use crate::usage::{UsageRing, sample};

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
    pub polled: bool,
    pub prometheus: Option<Prometheus>,
    pub observed: Observed,
}

impl Extras {
    pub fn record(&mut self, nodes: &[Arc<Node>], usage: Option<Vec<NodeUsage>>) {
        if let Some(sample) = usage.as_deref().and_then(|usage| sample(nodes, usage)) {
            self.ring.push(Timestamp::now(), sample);
        }
        self.node_usage = usage;
        self.polled = true;
    }

    fn metrics_server(&self) -> MetricsServer {
        match (&self.node_usage, self.polled) {
            (Some(_), _) => MetricsServer::Answering,
            (None, true) => MetricsServer::Unavailable,
            (None, false) => MetricsServer::Waiting,
        }
    }
}

pub struct Selection<'a> {
    pub pod_rows: bool,
    pub event_limit: usize,
    pub yaml: &'a [&'a (ResourceRef, String)],
    pub inspected: &'a [&'a ResourceRef],
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
            &crate::pods::Clock {
                now,
                zone: &TimeZone::system(),
            },
        )
    } else {
        Vec::new()
    };
    let detail = selection.map(|selection| {
        let parts = Parts {
            pods: &pods,
            nodes: &nodes,
            events: &events,
            index: &index,
        };
        cluster_detail(id, handle, extras, &parts, selection, now)
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

fn object_changes(
    id: &ClusterId,
    handle: &ClusterHandle,
    extras: &Extras,
    parts: &Parts<'_>,
    now: Timestamp,
) -> Vec<ClusterChange> {
    let deployments = handle.objects::<Deployment>();
    let mut changes: Vec<Change> = cluster_changes::rollouts(id, &deployments);
    changes.extend(cluster_changes::nodes(id, parts.nodes, parts.events));
    changes.extend(cluster_changes::namespaces(
        id,
        &handle.objects::<Namespace>(),
    ));
    changes.extend(cluster_changes::definitions(
        id,
        &handle.objects::<CustomResourceDefinition>(),
    ));
    changes.extend(extras.observed.changes(id));
    let zone = TimeZone::system();
    let clock = crate::pods::Clock { now, zone: &zone };
    changes
        .into_iter()
        .filter_map(|change| change.stamp(&clock))
        .collect()
}

struct Parts<'a> {
    pods: &'a [Arc<Pod>],
    nodes: &'a [Arc<Node>],
    events: &'a [Arc<Event>],
    index: &'a PodIndex,
}

fn cluster_detail(
    id: &ClusterId,
    handle: &ClusterHandle,
    extras: &Extras,
    parts: &Parts<'_>,
    selection: &Selection<'_>,
    now: Timestamp,
) -> ClusterDetail {
    let Parts {
        pods,
        nodes,
        events,
        index,
    } = *parts;
    let (cpu, memory) = index.commitments(capacity(nodes));
    let usage = extras.node_usage.as_deref().unwrap_or_default();
    let (metrics_source, utilization_source, utilization) = extras.prometheus.as_ref().map_or_else(
        || local_history(extras, now, &TimeZone::system()),
        prometheus_history,
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
    let slices = handle
        .watch_state(&WatchKind::EndpointSlices)
        .is_loaded()
        .then(|| handle.objects::<EndpointSlice>());
    let services = handle.objects::<Service>();
    let unready_services = network::unready_services(id, &services, slices.as_deref(), now);
    let network = network::facts(
        &services,
        slices.as_deref(),
        &handle.objects::<PersistentVolumeClaim>(),
    );
    let nodes = detail::nodes(nodes, index, usage, now);
    let scope = related::Scope {
        cluster: id,
        nodes: &nodes,
        events: &events,
        index,
        now,
    };
    let related = related::related(handle, &scope, selection.inspected);
    ClusterDetail {
        overview: Overview {
            metrics_source,
            utilization_source,
            cpu,
            memory,
            pods: index.phases,
            utilization,
            tops: pod_tops(id, pods, now),
            network,
            unready_services,
            changes: object_changes(id, handle, extras, parts, now),
        },
        nodes,
        namespaces: detail::namespaces(
            &handle.objects::<Namespace>(),
            index,
            extras.pod_usage.as_deref(),
            now,
        ),
        events,
        resources,
        related,
        namespace_details: crate::namespace::details(handle, selection.inspected, now),
    }
}

type History = (String, UtilizationSource, Vec<Utilization>);

fn prometheus_history(prometheus: &Prometheus) -> History {
    let ranges = prometheus.utilization.clone();
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

fn local_history(extras: &Extras, now: Timestamp, zone: &TimeZone) -> History {
    let ranges = TimeRange::ALL
        .into_iter()
        .map(|range| extras.ring.utilization(range, now))
        .collect();
    let label = if extras.node_usage.is_some() {
        METRICS_SERVER
    } else {
        NO_METRICS
    };
    let source = UtilizationSource::Local {
        since: extras.ring.since().map(|at| since_label(at, zone)),
        last: extras.ring.last_at().map(|at| since_label(at, zone)),
        metrics_server: extras.metrics_server(),
    };
    (label.to_owned(), source, ranges)
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

    #[test]
    fn a_reopened_app_explains_the_empty_hour_from_its_last_sample() -> crate::Result<()> {
        let parse = |at: &str| {
            at.parse::<Timestamp>()
                .map_err(|_| crate::Error::MetricsHistory("timestamp"))
        };
        let mut extras = Extras::default();
        let sample = crate::usage::Sample {
            cpu: 10.,
            memory: 20.,
        };
        extras.ring.push(parse("2026-10-16T08:49:00Z")?, sample);
        extras.ring.push(parse("2026-10-19T12:28:00Z")?, sample);
        let now = parse("2026-10-20T08:45:00Z")?;
        let (label, source, ranges) = local_history(&extras, now, &TimeZone::UTC);
        assert_eq!(label, NO_METRICS);
        assert!(ranges.iter().any(|range| range.cpu.is_empty()));
        assert_eq!(
            source.empty_reason(),
            "This cluster has no Prometheus. Flock records usage only while it's open and is waiting for metrics-server. Last sample: Oct 19, 12:28."
        );
        extras.record(&[], None);
        let (_, source, _) = local_history(&extras, now, &TimeZone::UTC);
        assert!(
            source
                .empty_reason()
                .contains("metrics-server isn't answering")
        );
        extras.record(&[], Some(Vec::new()));
        let (label, source, _) = local_history(&extras, now, &TimeZone::UTC);
        assert_eq!(label, METRICS_SERVER);
        assert!(matches!(
            source,
            UtilizationSource::Local {
                metrics_server: MetricsServer::Answering,
                ..
            }
        ));
        Ok(())
    }
}
