use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use flock_core::cluster::Namespace;
use flock_core::events::Event;
use flock_core::home::{ClusterMetrics, Problem, problem_counts};
use flock_core::nodes::Node;
use flock_core::overview::{Overview, PodPhases};
use flock_core::pod_tops::PodTops;
use flock_core::pods::{Pod, PodEvent, nav_counts};
use flock_core::resources::{Resource, ResourceKind};
use flock_core::{
    ClusterDetail, ClusterId, ClusterSummary, Counts, FluxObject, Page, ResourceSummary, Status,
};

const KIND_DISTRO: &str = "kind";
const LOCAL_REGION: &str = "local";
const ZONES: [char; 3] = ['a', 'b', 'c'];
const ZONE_LABEL: &str = "topology.kubernetes.io/zone=";
const KIND_CONTROL_PLANE: &str = "control-plane";
const KIND_WORKER: &str = "worker";
const KIND_SUBNET: &str = "172.18.0";
const KIND_FIRST_HOST: usize = 2;
const SUBNET_BASE: usize = 11;
const SUBNET_STRIDE: usize = 10;
const HOST_BASE: usize = 20;
const HOST_STRIDE: usize = 17;
const HOST_RANGE: usize = 230;
const PERCENT: f32 = 100.;
const CHECKOUT: &str = "checkout-api";
pub const CHECKOUT_REPLICA_SET: &str = "checkout-api-7f9c6d";
const CHECKOUT_IMAGE: &str = "ghcr.io/acme/checkout-api";
const CHECKOUT_PORT: u16 = 8080;
const DEPLOYMENT: &str = "Deployment";
const POD: &str = "Pod";
const CLUSTER_KIND: &str = "Cluster";
const SUFFIX_ALPHABET: &[u8] = b"bcdfghjklmnpqrstvwxz2456789";
const SUFFIX_LEN: usize = 5;
const SUFFIX_STEP: usize = 7;
const VERSION_SEPARATOR: &str = " · ";
const VERSION_LABEL: &str = "version";
const POD_HOST_BASE: usize = 100;
const POD_HOST_RANGE: usize = 150;

pub struct Template<'a> {
    pub detail: &'a ClusterDetail,
    pub pods: &'a [Pod],
}

pub struct Derived {
    pub detail: ClusterDetail,
    pub pods: Vec<Pod>,
}

pub struct Checkout {
    pub replicas: u32,
    pub version: Option<String>,
}

pub fn checkout(resources: &[ResourceSummary], cluster: &ClusterId) -> Option<Checkout> {
    let summary = resources.iter().find(|r| {
        &r.resource.cluster == cluster
            && r.resource.kind == DEPLOYMENT
            && r.resource.name == CHECKOUT
    })?;
    let (ready, version) = match summary.detail.split_once(VERSION_SEPARATOR) {
        Some((ready, version)) => (ready, Some(version.to_owned())),
        None => (summary.detail.as_str(), None),
    };
    let replicas = ready.split_once('/')?.1.trim().parse().ok()?;
    Some(Checkout { replicas, version })
}

pub fn cluster(
    summary: &ClusterSummary,
    octet: usize,
    problems: &[Problem],
    checkout: Option<&Checkout>,
    template: &Template<'_>,
) -> Option<Derived> {
    let metrics = summary.metrics.as_ref()?;
    let problems: Vec<&Problem> = problems
        .iter()
        .filter(|p| p.target.cluster == summary.id)
        .collect();
    let share = share(metrics, template.detail);
    let nodes = nodes(summary, metrics, octet, &template.detail.nodes);
    let detail = ClusterDetail {
        overview: overview(metrics, &template.detail.overview),
        namespaces: namespaces(&template.detail.namespaces, &problems, share),
        events: Vec::new(),
        resources: resources(&template.detail.resources, share, checkout),
        nodes,
        related: Vec::new(),
        namespace_details: Vec::new(),
        objects: Vec::new(),
    };
    let pods = pods(
        summary,
        &detail.nodes,
        &problems,
        checkout,
        metrics.pods.total.unwrap_or_default() as usize,
        template.pods,
    );
    let present: BTreeSet<&str> = detail
        .resources
        .values()
        .flatten()
        .map(|r| r.name.as_str())
        .chain(pods.iter().map(|p| p.name.as_str()))
        .collect();
    let events = events(&problems, &template.detail.events, &present);
    let tops = PodTops::of(&pods);
    let mut overview = Overview {
        tops,
        ..detail.overview
    };
    for change in &mut overview.changes {
        change.target.cluster = summary.id.clone();
    }
    for target in overview
        .expiring
        .iter_mut()
        .filter_map(|item| item.target.as_mut())
    {
        target.cluster = summary.id.clone();
    }
    Some(Derived {
        detail: ClusterDetail {
            events,
            overview,
            ..detail
        },
        pods,
    })
}

fn share(metrics: &ClusterMetrics, template: &ClusterDetail) -> f32 {
    let template_pods = template.overview.pods.total();
    if template_pods == 0 {
        return 0.;
    }
    ratio(metrics.pods.total.unwrap_or_default(), template_pods)
}

#[allow(clippy::cast_precision_loss)]
fn ratio(part: u32, whole: u32) -> f32 {
    part as f32 / whole as f32
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn scaled(len: usize, share: f32) -> usize {
    ((len as f32 * share).ceil() as usize).min(len)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn scaled_count(value: u32, share: f32) -> u32 {
    (value as f32 * share).round() as u32
}

fn nodes(
    summary: &ClusterSummary,
    metrics: &ClusterMetrics,
    octet: usize,
    template: &[Node],
) -> Vec<Node> {
    let general: Vec<&Node> = template
        .iter()
        .filter(|n| n.schedulable && n.taints.is_empty())
        .collect();
    let count = summary.nodes.unwrap_or_default() as usize;
    let pods = metrics.pods.total.unwrap_or_default();
    let cpu = percent(metrics.cpu_percent);
    let memory = percent(metrics.memory_percent);
    (0..count)
        .filter_map(|ix| {
            let base = general.get(ix % general.len().max(1))?;
            let (name, internal_ip) = node_name(summary, octet, ix);
            let zone = zone(&summary.region, ix);
            let mut node = (*base).clone();
            node.name = name;
            node.internal_ip = internal_ip;
            node.version = summary.version.clone().unwrap_or_default();
            node.cpu.used = node.cpu.allocatable * cpu;
            node.memory.used = node.memory.allocatable * memory;
            node.pods = spread(pods, count, ix);
            node.labels = node
                .labels
                .iter()
                .map(|label| {
                    if label.starts_with(ZONE_LABEL) {
                        format!("{ZONE_LABEL}{zone}")
                    } else {
                        label.clone()
                    }
                })
                .collect();
            node.zone = zone;
            Some(node)
        })
        .collect()
}

fn percent(value: Option<u8>) -> f32 {
    f32::from(value.unwrap_or_default()) / PERCENT
}

fn spread(total: u32, count: usize, ix: usize) -> u32 {
    let count = u32::try_from(count).unwrap_or(u32::MAX).max(1);
    let ix = u32::try_from(ix).unwrap_or(u32::MAX);
    total / count + u32::from(ix < total % count)
}

fn node_name(summary: &ClusterSummary, octet: usize, ix: usize) -> (String, String) {
    if summary.distro == KIND_DISTRO {
        let name = match ix {
            0 => format!("{}-{KIND_CONTROL_PLANE}", summary.name),
            1 => format!("{}-{KIND_WORKER}", summary.name),
            n => format!("{}-{KIND_WORKER}{n}", summary.name),
        };
        return (name, format!("{KIND_SUBNET}.{}", ix + KIND_FIRST_HOST));
    }
    let subnet = SUBNET_BASE + SUBNET_STRIDE * (ix % ZONES.len()) + ix / ZONES.len();
    let host = HOST_BASE + (HOST_STRIDE * ix) % HOST_RANGE;
    (
        format!("ip-10-{octet}-{subnet}-{host}"),
        format!("10.{octet}.{subnet}.{host}"),
    )
}

fn zone(region: &str, ix: usize) -> String {
    if region == LOCAL_REGION {
        return region.to_owned();
    }
    let letter = ZONES[ix % ZONES.len()];
    format!("{region}{letter}")
}

fn overview(metrics: &ClusterMetrics, template: &Overview) -> Overview {
    let total = metrics.pods.total.unwrap_or_default();
    let failing = metrics.pods.err;
    let cpu = f32::from(metrics.cpu_percent.unwrap_or_default());
    let memory = f32::from(metrics.memory_percent.unwrap_or_default());
    let mut overview = template.clone();
    overview.pods = PodPhases {
        running: total.saturating_sub(failing),
        pending: 0,
        failing,
        unschedulable: 0,
    };
    let cpu_scale = template
        .utilization
        .first()
        .and_then(|u| u.cpu.last())
        .map_or(1., |last| cpu / last);
    for series in &mut overview.utilization {
        let memory_scale = series.memory.last().map_or(1., |last| memory / last);
        let series_cpu_scale = series.cpu.last().map_or(1., |last| cpu / last);
        series.cpu.iter_mut().for_each(|v| *v *= series_cpu_scale);
        series.memory.iter_mut().for_each(|v| *v *= memory_scale);
    }
    overview.cpu.requests *= cpu_scale;
    overview.cpu.limits *= cpu_scale;
    overview
}

fn namespaces(template: &[Namespace], problems: &[&Problem], share: f32) -> Vec<Namespace> {
    template
        .iter()
        .map(|ns| {
            let flagged = problems
                .iter()
                .copied()
                .filter(|p| p.target.namespace.as_deref() == Some(ns.name.as_str()));
            let problems = problem_counts(flagged);
            Namespace {
                status: problems.status(),
                pods: scaled_count(ns.pods, share),
                problems,
                cpu: ns.cpu.map(|cpu| cpu * share),
                memory: ns.memory.map(|memory| memory * share),
                cpu_requests: ns.cpu_requests * share,
                memory_requests: ns.memory_requests * share,
                restarts: scaled_count(ns.restarts, share),
                ..ns.clone()
            }
        })
        .filter(|ns| ns.pods > 0)
        .collect()
}

fn reason(message: &str) -> String {
    let head = message.split([':', '·', '(']).next().unwrap_or(message);
    head.trim().to_owned()
}

fn events(problems: &[&Problem], template: &[Event], present: &BTreeSet<&str>) -> Vec<Event> {
    let flagged = problems
        .iter()
        .filter(|p| p.target.kind != CLUSTER_KIND)
        .map(|p| Event {
            labels: BTreeMap::new(),
            details: crate::detail::event_details(
                &format!("{}/{}", p.target.kind.to_lowercase(), p.target.name),
                p.status,
                p.age,
            ),
            status: p.status,
            reason: reason(&p.message),
            object: format!("{}/{}", p.target.kind.to_lowercase(), p.target.name),
            namespace: p.target.namespace.clone(),
            message: p.message.clone(),
            age: p.age,
            count: 1,
        });
    let normal = template
        .iter()
        .filter(|e| !e.is_warning())
        .filter(|e| {
            let name = e
                .object
                .split_once('/')
                .map_or(e.object.as_str(), |(_, n)| n);
            present.contains(name)
        })
        .cloned();
    flagged.chain(normal).collect()
}

fn resources(
    template: &BTreeMap<ResourceKind, Vec<Resource>>,
    share: f32,
    checkout: Option<&Checkout>,
) -> BTreeMap<ResourceKind, Vec<Resource>> {
    template
        .iter()
        .map(|(kind, rows)| {
            let healthy: Vec<&Resource> = rows
                .iter()
                .filter(|r| r.status == Status::Ok && r.name != CHECKOUT)
                .collect();
            let kept = healthy.into_iter().take(scaled(rows.len(), share)).cloned();
            let lead = (*kind == ResourceKind::Deployment)
                .then(|| checkout.and_then(|c| checkout_row(rows, c)))
                .flatten();
            (*kind, lead.into_iter().chain(kept).collect())
        })
        .collect()
}

fn checkout_image(checkout: &Checkout, template: &str) -> String {
    match &checkout.version {
        Some(version) => format!("{CHECKOUT_IMAGE}:{version}"),
        None => template.to_owned(),
    }
}

fn checkout_row(template: &[Resource], checkout: &Checkout) -> Option<Resource> {
    let base = template.iter().find(|r| r.name == CHECKOUT)?;
    let template_image = base.cells.last().cloned().unwrap_or_default();
    let image = checkout_image(checkout, &template_image);
    let replicas = checkout.replicas;
    let mut yaml = format!(
        "apiVersion: apps/v1\nkind: {DEPLOYMENT}\nmetadata:\n  name: {CHECKOUT}\n  namespace: {}\n  labels:\n    app.kubernetes.io/name: {CHECKOUT}\n",
        base.namespace
    );
    let _ = write!(
        yaml,
        "spec:\n  replicas: {replicas}\n  template:\n    spec:\n      containers:\n        - name: {CHECKOUT}\n          image: {image}\n          ports:\n            - containerPort: {CHECKOUT_PORT}\nstatus:\n  replicas: {replicas}\n  readyReplicas: {replicas}\n  availableReplicas: {replicas}\n"
    );
    Some(Resource {
        status: Status::Ok,
        cells: vec![
            format!("{replicas}/{replicas}"),
            replicas.to_string(),
            replicas.to_string(),
            image,
        ],
        yaml,
        conditions: Vec::new(),
        diagnosis: None,
        ..base.clone()
    })
}

pub fn suffix(seed: usize) -> String {
    (0..SUFFIX_LEN)
        .map(|ix| {
            let at = (seed * SUFFIX_STEP + ix * (seed + SUFFIX_STEP)) % SUFFIX_ALPHABET.len();
            char::from(SUFFIX_ALPHABET[at])
        })
        .collect()
}

fn pods(
    summary: &ClusterSummary,
    nodes: &[Node],
    problems: &[&Problem],
    checkout: Option<&Checkout>,
    total: usize,
    template: &[Pod],
) -> Vec<Pod> {
    let healthy: Vec<&Pod> = template.iter().filter(|p| p.status == Status::Ok).collect();
    let checkout_base = healthy
        .iter()
        .copied()
        .find(|p| p.name.starts_with(CHECKOUT));
    let checkout_pods = checkout
        .zip(checkout_base)
        .map(|(checkout, base)| {
            (0..checkout.replicas as usize)
                .map(|ix| checkout_pod(base, checkout, ix))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let flagged: Vec<Pod> = problems
        .iter()
        .filter(|p| p.target.kind == POD)
        .filter_map(|p| {
            problem_pod(
                p,
                healthy
                    .iter()
                    .copied()
                    .find(|h| !h.name.starts_with(CHECKOUT)),
            )
        })
        .collect();
    let listed = checkout_pods.len() + flagged.len();
    let others = healthy
        .iter()
        .filter(|p| !p.name.starts_with(CHECKOUT))
        .take(total.saturating_sub(listed))
        .map(|p| (*p).clone());
    checkout_pods
        .into_iter()
        .chain(others)
        .chain(flagged)
        .enumerate()
        .map(|(ix, mut pod)| {
            let node = nodes.get(ix % nodes.len().max(1));
            pod.cluster = summary.id.clone();
            pod.node = node.map(|n| n.name.clone());
            pod.ip = node.map(|n| pod_ip(&n.internal_ip, ix));
            pod
        })
        .collect()
}

fn pod_ip(node_ip: &str, ix: usize) -> String {
    let subnet = node_ip
        .rsplit_once('.')
        .map_or(node_ip, |(subnet, _)| subnet);
    format!("{subnet}.{}", POD_HOST_BASE + ix % POD_HOST_RANGE)
}

fn checkout_pod(base: &Pod, checkout: &Checkout, ix: usize) -> Pod {
    let mut pod = base.clone();
    pod.name = format!("{CHECKOUT_REPLICA_SET}-{}", suffix(ix));
    if let Some(version) = &checkout.version {
        pod.labels.insert(VERSION_LABEL.into(), version.clone());
        let image = format!("{CHECKOUT_IMAGE}:{version}");
        let api = pod
            .containers
            .iter_mut()
            .find(|c| c.image.starts_with(CHECKOUT_IMAGE));
        if let Some(api) = api {
            api.image = image;
        }
    }
    pod
}

fn problem_pod(problem: &Problem, base: Option<&Pod>) -> Option<Pod> {
    let mut pod = base?.clone();
    let reason = reason(&problem.message);
    pod.namespace = problem.target.namespace.clone().unwrap_or_default();
    pod.name.clone_from(&problem.target.name);
    pod.status = problem.status;
    pod.reason.clone_from(&reason);
    pod.owner = None;
    pod.labels = BTreeMap::new();
    if let Some(container) = pod.containers.first_mut() {
        container.status = problem.status;
        container.ready = false;
        container.reason.clone_from(&reason);
    }
    pod.events = vec![PodEvent {
        status: problem.status,
        reason,
        message: problem.message.clone(),
        count: 1,
        age: problem.age,
    }];
    pod.conditions = crate::pods::conditions(&pod);
    Some(pod)
}

pub fn nav(
    summary: &ClusterSummary,
    detail: &ClusterDetail,
    pods: &[Pod],
    flux: &[FluxObject],
) -> BTreeMap<Page, Counts> {
    let mut nav = BTreeMap::from([
        (Page::Nodes, tally(detail.nodes.iter().map(|n| n.status))),
        (
            Page::Namespaces,
            Counts::total(count(detail.namespaces.len())),
        ),
        (
            Page::Events,
            Counts::default().with_warn(count(detail.warnings())),
        ),
    ]);
    nav.insert(Page::Pods, nav_counts(pods, &summary.id));
    for kind in ResourceKind::ALL {
        let rows = detail.resources(kind);
        nav.insert(kind.page(), tally(rows.iter().map(|r| r.status)));
    }
    let objects: Vec<&FluxObject> = flux
        .iter()
        .filter(|o| o.key.cluster == summary.id)
        .collect();
    for page in [Page::Kustomizations, Page::HelmReleases, Page::Sources] {
        let statuses = objects
            .iter()
            .filter(|o| o.key.kind.page() == page)
            .map(|o| o.status());
        nav.insert(page, tally(statuses));
    }
    nav
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn tally(statuses: impl Iterator<Item = Status>) -> Counts {
    let statuses: Vec<Status> = statuses.collect();
    let of = |status: Status| count(statuses.iter().filter(|s| **s == status).count());
    Counts::total(count(statuses.len()))
        .with_err(of(Status::Err))
        .with_warn(of(Status::Warn))
        .with_sus(of(Status::Suspended))
}
