use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use flock_core::control_plane::{
    ApiService, Check, ControlPlane, FailedRequests, Health, Probe, Traffic, Webhook,
    is_server_error,
};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use k8s_openapi::api::admissionregistration::v1::{
    MutatingWebhookConfiguration, ValidatingWebhookConfiguration,
};
use k8s_openapi::kube_aggregator::pkg::apis::apiregistration::v1::APIService;
use kube::Api;
use kube::api::ListParams;

use crate::error::{Error, Result};
use crate::failure::classify;
use crate::manager::ClusterHandle;

const REQUESTS: &str = "apiserver_request_total";
const LATENCY: &str = "apiserver_request_duration_seconds_bucket";
const ETCD_LATENCY: &str = "etcd_request_duration_seconds_bucket";
const WEBHOOK_LATENCY: &str = "apiserver_admission_webhook_admission_duration_seconds_bucket";
const LONG_RUNNING_VERBS: [&str; 2] = ["WATCH", "CONNECT"];
const CODE: &str = "code";
const VERB: &str = "verb";
const RESOURCE: &str = "resource";
const GROUP: &str = "group";
const SUBRESOURCE: &str = "subresource";
const NAME: &str = "name";
const LE: &str = "le";
const PASSED: &str = "[+]";
const FAILED: &str = "[-]";
const PASSED_SUFFIX: &str = " ok";
const FAILED_SEPARATOR: &str = " failed";
const READYZ: &str = "/readyz?verbose";
const LIVEZ: &str = "/livez?verbose";
const METRICS: &str = "/metrics";
const AVAILABLE: &str = "Available";
const CONDITION_TRUE: &str = "True";
const UNKNOWN_REASON: &str = "Unknown";
const DEFAULT_WEBHOOK_TIMEOUT: Duration = Duration::from_secs(10);
const QUANTILE: f64 = 0.99;
const MINUTE: Duration = Duration::from_mins(1);
const CLOCK_FORMAT: &str = "%H:%M";
pub const WINDOW: Duration = Duration::from_hours(1);
const FAILURE_LIMIT: usize = 50;
const GAP_SAMPLES: u32 = 3;
const SLOT_EDGE: Duration = Duration::from_nanos(1);

type Buckets = BTreeMap<String, f64>;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct RequestKey {
    code: String,
    verb: String,
    resource: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scrape {
    requests: BTreeMap<RequestKey, f64>,
    latency: Buckets,
    etcd: Buckets,
    webhooks: BTreeMap<String, Buckets>,
}

struct Sample<'a> {
    name: &'a str,
    labels: Vec<(&'a str, String)>,
    value: f64,
}

impl Sample<'_> {
    fn label(&self, key: &str) -> &str {
        self.labels
            .iter()
            .find(|(name, _)| *name == key)
            .map_or("", |(_, value)| value.as_str())
    }
}

fn family(line: &str) -> Option<&str> {
    let end = line.find(['{', ' '])?;
    line.get(..end)
}

fn parse_labels(text: &str) -> Option<(Vec<(&str, String)>, &str)> {
    let mut labels = Vec::new();
    let mut rest = text;
    loop {
        rest = rest.trim_start_matches([',', ' ']);
        if let Some(after) = rest.strip_prefix('}') {
            return Some((labels, after));
        }
        let eq = rest.find('=')?;
        let key = rest.get(..eq)?.trim();
        let quoted = rest.get(eq + 1..)?.strip_prefix('"')?;
        let mut value = String::new();
        let mut chars = quoted.char_indices();
        let end = loop {
            let (ix, c) = chars.next()?;
            match c {
                '"' => break ix,
                '\\' => match chars.next()?.1 {
                    'n' => value.push('\n'),
                    other => value.push(other),
                },
                other => value.push(other),
            }
        };
        labels.push((key, value));
        rest = quoted.get(end + 1..)?;
    }
}

fn parse_sample(line: &str) -> Option<Sample<'_>> {
    let name = family(line)?;
    let rest = line.get(name.len()..)?;
    let (labels, rest) = match rest.strip_prefix('{') {
        Some(inner) => parse_labels(inner)?,
        None => (Vec::new(), rest),
    };
    let value = rest.split_whitespace().next()?.parse().ok()?;
    Some(Sample {
        name,
        labels,
        value,
    })
}

fn resource_of(sample: &Sample<'_>) -> String {
    let resource = sample.label(RESOURCE);
    let subresource = sample.label(SUBRESOURCE);
    if resource.is_empty() {
        return subresource.to_owned();
    }
    let group = sample.label(GROUP);
    let mut text = if group.is_empty() {
        resource.to_owned()
    } else {
        format!("{resource}.{group}")
    };
    if !subresource.is_empty() {
        text.push('/');
        text.push_str(subresource);
    }
    text
}

fn add_bucket(buckets: &mut Buckets, sample: &Sample<'_>) {
    *buckets.entry(sample.label(LE).to_owned()).or_default() += sample.value;
}

pub fn scrape(body: &str) -> Scrape {
    let mut scrape = Scrape::default();
    let wanted = [REQUESTS, LATENCY, ETCD_LATENCY, WEBHOOK_LATENCY];
    for line in body.lines() {
        if !family(line).is_some_and(|name| wanted.contains(&name)) {
            continue;
        }
        let Some(sample) = parse_sample(line) else {
            continue;
        };
        match sample.name {
            REQUESTS => {
                let key = RequestKey {
                    code: sample.label(CODE).to_owned(),
                    verb: sample.label(VERB).to_owned(),
                    resource: resource_of(&sample),
                };
                *scrape.requests.entry(key).or_default() += sample.value;
            }
            LATENCY => {
                if !LONG_RUNNING_VERBS.contains(&sample.label(VERB)) {
                    add_bucket(&mut scrape.latency, &sample);
                }
            }
            ETCD_LATENCY => add_bucket(&mut scrape.etcd, &sample),
            WEBHOOK_LATENCY => {
                let name = sample.label(NAME).to_owned();
                add_bucket(scrape.webhooks.entry(name).or_default(), &sample);
            }
            _ => {}
        }
    }
    scrape
}

pub fn checks(body: &str) -> Vec<Check> {
    body.lines()
        .filter_map(|line| {
            if let Some(passed) = line.strip_prefix(PASSED) {
                let name = passed.strip_suffix(PASSED_SUFFIX).unwrap_or(passed);
                return Some(Check {
                    name: name.to_owned(),
                    passed: true,
                });
            }
            let failed = line.strip_prefix(FAILED)?;
            let name = failed
                .split_once(FAILED_SEPARATOR)
                .map_or(failed, |(name, _)| name);
            Some(Check {
                name: name.to_owned(),
                passed: false,
            })
        })
        .collect()
}

fn increase<K: Ord + Clone>(
    total: &mut BTreeMap<K, f64>,
    before: &BTreeMap<K, f64>,
    after: &BTreeMap<K, f64>,
) {
    for (key, now) in after {
        let then = before.get(key).copied().unwrap_or_default();
        let delta = if *now >= then { now - then } else { *now };
        *total.entry(key.clone()).or_default() += delta;
    }
}

fn quantile(buckets: &Buckets) -> Option<Duration> {
    let mut sorted: Vec<(f64, f64)> = buckets
        .iter()
        .filter_map(|(le, count)| Some((le.parse::<f64>().ok()?, *count)))
        .collect();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total = sorted.last()?.1;
    if total <= 0. {
        return None;
    }
    let rank = QUANTILE * total;
    let (mut lower, mut below) = (0., 0.);
    for (le, count) in sorted {
        if count >= rank {
            if le.is_infinite() {
                return Duration::try_from_secs_f64(lower).ok();
            }
            let width = count - below;
            let share = if width > 0. {
                (rank - below) / width
            } else {
                1.
            };
            return Duration::try_from_secs_f64(lower + (le - lower) * share).ok();
        }
        lower = le;
        below = count;
    }
    None
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn whole(value: f64) -> u64 {
    value.round().max(0.) as u64
}

fn clock(at: Timestamp, zone: &TimeZone) -> String {
    at.to_zoned(zone.clone()).strftime(CLOCK_FORMAT).to_string()
}

fn elapsed(from: Timestamp, to: Timestamp) -> Duration {
    Duration::try_from(to.duration_since(from)).unwrap_or_default()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct History {
    samples: VecDeque<(Timestamp, Scrape)>,
}

impl History {
    pub fn push(&mut self, at: Timestamp, scrape: Scrape, max_gap: Duration) {
        let stale = self
            .samples
            .back()
            .is_some_and(|(last, _)| elapsed(*last, at) > max_gap);
        if stale {
            self.samples.clear();
        }
        self.samples.push_back((at, scrape));
        while self
            .samples
            .front()
            .is_some_and(|(first, _)| elapsed(*first, at) > WINDOW)
        {
            self.samples.pop_front();
        }
    }

    pub fn traffic(&self, zone: &TimeZone) -> Option<Traffic> {
        let (first, _) = self.samples.front()?;
        let (last, _) = self.samples.back()?;
        if self.samples.len() < 2 {
            return None;
        }
        let window = elapsed(*first, *last);
        let minutes = window.as_secs().div_ceil(MINUTE.as_secs()).max(1);
        let mut per_minute = vec![0.; usize::try_from(minutes).unwrap_or_default()];
        let mut requests = BTreeMap::new();
        let mut latency = Buckets::new();
        let mut etcd = Buckets::new();
        let mut webhooks: BTreeMap<String, Buckets> = BTreeMap::new();
        for (before, after) in self.samples.iter().zip(self.samples.iter().skip(1)) {
            let mut step = BTreeMap::new();
            increase(&mut step, &before.1.requests, &after.1.requests);
            let errors: f64 = step
                .iter()
                .filter(|(key, _)| is_server_error(&key.code))
                .map(|(_, count)| count)
                .sum();
            let minute =
                elapsed(*first, after.0).saturating_sub(SLOT_EDGE).as_secs() / MINUTE.as_secs();
            let slot = usize::try_from(minute).unwrap_or_default();
            if let Some(bar) = per_minute.get_mut(slot) {
                *bar += errors;
            }
            for (key, count) in step {
                *requests.entry(key).or_default() += count;
            }
            increase(&mut latency, &before.1.latency, &after.1.latency);
            increase(&mut etcd, &before.1.etcd, &after.1.etcd);
            for (name, buckets) in &after.1.webhooks {
                let empty = Buckets::new();
                let then = before.1.webhooks.get(name).unwrap_or(&empty);
                increase(webhooks.entry(name.clone()).or_default(), then, buckets);
            }
        }
        Some(summarize(
            (*first, *last, window),
            &requests,
            per_minute,
            (&latency, &etcd, &webhooks),
            zone,
        ))
    }
}

fn summarize(
    (first, last, window): (Timestamp, Timestamp, Duration),
    requests: &BTreeMap<RequestKey, f64>,
    per_minute: Vec<f64>,
    (latency, etcd, webhooks): (&Buckets, &Buckets, &BTreeMap<String, Buckets>),
    zone: &TimeZone,
) -> Traffic {
    let total: f64 = requests.values().sum();
    let mut failures: Vec<FailedRequests> = requests
        .iter()
        .filter(|(key, _)| is_server_error(&key.code))
        .map(|(key, count)| FailedRequests {
            code: key.code.clone(),
            verb: key.verb.clone(),
            resource: key.resource.clone(),
            count: whole(*count),
        })
        .filter(|failure| failure.count > 0)
        .collect();
    failures.sort_by_key(|failure| std::cmp::Reverse(failure.count));
    failures.truncate(FAILURE_LIMIT);
    let errors = failures.iter().map(|failure| failure.count).sum();
    let webhook_p99 = webhooks
        .iter()
        .filter_map(|(name, buckets)| Some((name.clone(), quantile(buckets)?)))
        .collect();
    Traffic {
        since: clock(first, zone),
        until: clock(last, zone),
        window,
        requests: whole(total),
        errors,
        p99: quantile(latency),
        etcd_p99: quantile(etcd),
        errors_per_minute: per_minute.into_iter().map(whole).collect(),
        failures,
        webhook_p99,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sampled {
    pub health: Probe<Health>,
    pub metrics: std::result::Result<Scrape, String>,
    pub webhooks: Probe<Vec<Webhook>>,
    pub unavailable_services: Vec<ApiService>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    history: History,
    health: Probe<Health>,
    metrics_error: Option<String>,
    webhooks: Probe<Vec<Webhook>>,
    unavailable_services: Vec<ApiService>,
}

impl State {
    pub fn record(&mut self, sampled: Sampled, at: Timestamp, interval: Duration) {
        self.health = sampled.health;
        self.webhooks = sampled.webhooks;
        self.unavailable_services = sampled.unavailable_services;
        match sampled.metrics {
            Ok(scrape) => {
                self.metrics_error = None;
                self.history.push(at, scrape, interval * GAP_SAMPLES);
            }
            Err(reason) => {
                self.metrics_error = Some(reason);
                self.history = History::default();
            }
        }
    }

    pub fn view(&self, zone: &TimeZone) -> ControlPlane {
        let traffic = match (&self.metrics_error, self.history.traffic(zone)) {
            (Some(reason), _) => Probe::Unavailable(reason.clone()),
            (None, Some(traffic)) => Probe::Ready(traffic),
            (None, None) => Probe::Waiting,
        };
        ControlPlane {
            health: self.health.clone(),
            traffic,
            webhooks: self.webhooks.clone(),
            unavailable_services: self.unavailable_services.clone(),
        }
    }
}

fn reason(error: &Error, handle: &ClusterHandle, path: &str) -> String {
    let failure = classify(error, handle.context());
    format!("{path}: {}", failure.detail)
}

fn server_error_body(error: &Error) -> Option<&str> {
    match error {
        Error::Kube(kube::Error::Api(status)) if status.code >= 500 => Some(&status.message),
        _ => None,
    }
}

fn timeout_of(seconds: Option<i32>) -> Duration {
    seconds
        .and_then(|s| u64::try_from(s).ok())
        .map_or(DEFAULT_WEBHOOK_TIMEOUT, Duration::from_secs)
}

fn unavailable(service: &APIService) -> Option<ApiService> {
    let conditions = service.status.as_ref()?.conditions.as_ref()?;
    let available = conditions.iter().find(|c| c.type_ == AVAILABLE)?;
    if available.status == CONDITION_TRUE {
        return None;
    }
    Some(ApiService {
        name: service.metadata.name.clone().unwrap_or_default(),
        reason: available
            .reason
            .clone()
            .unwrap_or_else(|| UNKNOWN_REASON.to_owned()),
    })
}

impl ClusterHandle {
    async fn get_text(&self, path: &str) -> Result<String> {
        let client = self.client()?;
        let request = http::Request::get(path).body(Vec::new())?;
        Ok(client.request_text(request).await?)
    }

    async fn health_checks(&self, path: &str) -> std::result::Result<Vec<Check>, String> {
        match self.get_text(path).await {
            Ok(body) => Ok(checks(&body)),
            Err(error) => server_error_body(&error)
                .map(checks)
                .filter(|checks| !checks.is_empty())
                .ok_or_else(|| reason(&error, self, path)),
        }
    }

    async fn webhooks(&self) -> Result<Vec<Webhook>> {
        let client = self.client()?;
        let validating: Api<ValidatingWebhookConfiguration> = Api::all(client.clone());
        let mutating: Api<MutatingWebhookConfiguration> = Api::all(client);
        let params = ListParams::default();
        let (validating, mutating) = tokio::join!(validating.list(&params), mutating.list(&params));
        let mut webhooks = Vec::new();
        for config in validating?.items {
            let configuration = config.metadata.name.unwrap_or_default();
            webhooks.extend(config.webhooks.into_iter().flatten().map(|w| Webhook {
                name: w.name,
                configuration: configuration.clone(),
                timeout: timeout_of(w.timeout_seconds),
            }));
        }
        for config in mutating?.items {
            let configuration = config.metadata.name.unwrap_or_default();
            webhooks.extend(config.webhooks.into_iter().flatten().map(|w| Webhook {
                name: w.name,
                configuration: configuration.clone(),
                timeout: timeout_of(w.timeout_seconds),
            }));
        }
        Ok(webhooks)
    }

    async fn unavailable_services(&self) -> Result<Vec<ApiService>> {
        let api: Api<APIService> = Api::all(self.client()?);
        let list = api.list(&ListParams::default()).await?;
        Ok(list.items.iter().filter_map(unavailable).collect())
    }

    pub async fn control_plane(&self) -> Sampled {
        let (ready, live, metrics, webhooks, services) = tokio::join!(
            self.health_checks(READYZ),
            self.health_checks(LIVEZ),
            self.get_text(METRICS),
            self.webhooks(),
            self.unavailable_services(),
        );
        let health = match (ready, live) {
            (Ok(mut ready), Ok(live)) => {
                ready.extend(live);
                Probe::Ready(Health { checks: ready })
            }
            (Ok(checks), Err(_)) => Probe::Ready(Health { checks }),
            (Err(_), Ok(checks)) => Probe::Ready(Health { checks }),
            (Err(reason), Err(_)) => Probe::Unavailable(reason),
        };
        let metrics = metrics
            .map(|body| scrape(&body))
            .map_err(|error| reason(&error, self, METRICS));
        let webhooks = match webhooks {
            Ok(webhooks) => Probe::Ready(webhooks),
            Err(error) => Probe::Unavailable(reason(&error, self, "webhook configurations")),
        };
        Sampled {
            health,
            metrics,
            webhooks,
            unavailable_services: services.unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = r#"# HELP apiserver_request_total [STABLE] Counter of apiserver requests.
# TYPE apiserver_request_total counter
apiserver_request_total{code="200",component="apiserver",dry_run="",group="",resource="pods",scope="namespace",subresource="",verb="LIST",version="v1"} 120
apiserver_request_total{code="503",component="apiserver",dry_run="",group="metrics.k8s.io",resource="pods",scope="namespace",subresource="",verb="GET",version="v1beta1"} 4
apiserver_request_total{code="500",component="",dry_run="",group="",resource="",scope="",subresource="/readyz",verb="GET",version=""} 2
apiserver_request_total{code="500",component="apiserver",dry_run="",group="",resource="nodes",scope="resource",subresource="status",verb="PATCH",version="v1"} 1
apiserver_request_total_extra{code="500"} 999
apiserver_request_duration_seconds_bucket{verb="GET",le="0.1"} 90
apiserver_request_duration_seconds_bucket{verb="GET",le="1"} 100
apiserver_request_duration_seconds_bucket{verb="GET",le="+Inf"} 100
apiserver_request_duration_seconds_bucket{verb="WATCH",le="0.1"} 0
apiserver_request_duration_seconds_bucket{verb="WATCH",le="+Inf"} 50
etcd_request_duration_seconds_bucket{operation="get",resource="pods",le="0.005"} 10
etcd_request_duration_seconds_bucket{operation="get",resource="pods",le="+Inf"} 10
apiserver_admission_webhook_admission_duration_seconds_bucket{name="validate.kyverno.svc-fail",operation="CREATE",rejected="false",type="validating",le="2.5"} 0
apiserver_admission_webhook_admission_duration_seconds_bucket{name="validate.kyverno.svc-fail",operation="CREATE",rejected="false",type="validating",le="+Inf"} 3
apiserver_storage_size_bytes{storage_cluster_id="etcd-0"} 1.0739712e+07
"#;

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_second(1_791_873_360 + secs).unwrap_or_default()
    }

    fn key(code: &str, verb: &str, resource: &str) -> RequestKey {
        RequestKey {
            code: code.into(),
            verb: verb.into(),
            resource: resource.into(),
        }
    }

    fn requests(pairs: &[(RequestKey, f64)]) -> Scrape {
        Scrape {
            requests: pairs.iter().cloned().collect(),
            ..Scrape::default()
        }
    }

    fn buckets(pairs: &[(&str, f64)]) -> Buckets {
        pairs.iter().map(|(le, n)| ((*le).to_owned(), *n)).collect()
    }

    #[test]
    fn scrape_keeps_only_the_families_it_reads() {
        let scrape = scrape(BODY);
        assert_eq!(scrape.requests.len(), 4);
        assert_eq!(
            scrape
                .requests
                .get(&key("503", "GET", "pods.metrics.k8s.io")),
            Some(&4.)
        );
        assert_eq!(
            scrape.requests.get(&key("500", "GET", "/readyz")),
            Some(&2.)
        );
        assert_eq!(
            scrape.requests.get(&key("500", "PATCH", "nodes/status")),
            Some(&1.)
        );
        assert_eq!(
            scrape.latency,
            buckets(&[("0.1", 90.), ("1", 100.), ("+Inf", 100.)])
        );
        assert_eq!(scrape.etcd.get("+Inf"), Some(&10.));
        assert_eq!(
            scrape
                .webhooks
                .get("validate.kyverno.svc-fail")
                .and_then(|b| b.get("+Inf")),
            Some(&3.)
        );
    }

    #[test]
    fn label_values_unescape() {
        let line = r#"x_total{path="a\"b\\c",empty="",nl="1\n2"} 7 1700000000"#;
        let sample = parse_sample(line);
        let sample = sample.as_ref();
        assert_eq!(sample.map(|s| s.label("path")), Some("a\"b\\c"));
        assert_eq!(sample.map(|s| s.label("nl")), Some("1\n2"));
        assert_eq!(sample.map(|s| s.label("empty")), Some(""));
        assert_eq!(sample.map(|s| s.value), Some(7.));
        assert!(parse_sample("broken{a=\"x 1").is_none());
        assert_eq!(parse_sample("up 1").map(|s| s.value), Some(1.));
    }

    #[test]
    fn quantile_interpolates_inside_the_bucket() {
        let p99 = quantile(&buckets(&[("0.1", 90.), ("1", 100.), ("+Inf", 100.)]));
        let p99 = p99.map_or(0., |p99| p99.as_secs_f64());
        assert!((p99 - 0.91).abs() < 1e-9);
        let overflow = quantile(&buckets(&[("2.5", 0.), ("+Inf", 3.)]));
        assert_eq!(overflow, Some(Duration::from_millis(2500)));
        assert_eq!(quantile(&buckets(&[("+Inf", 0.)])), None);
        assert_eq!(quantile(&Buckets::new()), None);
    }

    #[test]
    fn health_lines_parse_into_checks() {
        let body = "[+]ping ok\n[-]etcd failed: reason withheld\n[+]poststarthook/rbac ok\nreadyz check failed\n";
        let checks = checks(body);
        let names: Vec<(&str, bool)> = checks.iter().map(|c| (c.name.as_str(), c.passed)).collect();
        assert_eq!(
            names,
            vec![
                ("ping", true),
                ("etcd", false),
                ("poststarthook/rbac", true)
            ]
        );
    }

    #[test]
    fn traffic_rates_come_from_deltas_between_samples() {
        let ok = key("200", "GET", "pods");
        let unavailable = key("503", "GET", "pods.metrics.k8s.io");
        let fresh = key("500", "CREATE", "pods");
        let gap = Duration::from_mins(5);
        let mut history = History::default();
        history.push(
            at(0),
            requests(&[(ok.clone(), 1000.), (unavailable.clone(), 50.)]),
            gap,
        );
        assert_eq!(history.traffic(&TimeZone::UTC), None);
        history.push(
            at(30),
            requests(&[(ok.clone(), 1100.), (unavailable.clone(), 54.)]),
            gap,
        );
        history.push(
            at(90),
            requests(&[(ok.clone(), 40.), (unavailable, 60.), (fresh, 2.)]),
            gap,
        );
        let traffic = history.traffic(&TimeZone::UTC);
        let traffic = traffic.as_ref();
        assert_eq!(traffic.map(|t| t.window), Some(Duration::from_secs(90)));
        assert_eq!(traffic.map(|t| t.requests), Some(100 + 4 + 40 + 6 + 2));
        assert_eq!(traffic.map(|t| t.errors), Some(12));
        assert_eq!(
            traffic.map(|t| t.errors_per_minute.clone()),
            Some(vec![4, 8])
        );
        assert_eq!(
            traffic.map(|t| (t.since.as_str(), t.until.as_str())),
            Some(("06:36", "06:37"))
        );
        let top = traffic.and_then(|t| t.failures.first());
        assert_eq!(top.map(|f| (f.code.as_str(), f.count)), Some(("503", 10)));
    }

    #[test]
    fn a_long_gap_starts_a_new_window() {
        let ok = key("200", "GET", "pods");
        let gap = Duration::from_secs(90);
        let mut history = History::default();
        history.push(at(0), requests(&[(ok.clone(), 1.)]), gap);
        history.push(at(30), requests(&[(ok.clone(), 2.)]), gap);
        history.push(at(600), requests(&[(ok.clone(), 9.)]), gap);
        assert_eq!(history.traffic(&TimeZone::UTC), None);
        history.push(at(630), requests(&[(ok, 10.)]), gap);
        let window = history.traffic(&TimeZone::UTC).map(|t| t.window);
        assert_eq!(window, Some(Duration::from_secs(30)));
    }

    #[test]
    fn samples_older_than_the_window_drop_off() {
        let ok = key("200", "GET", "pods");
        let step = Duration::from_mins(20);
        let mut history = History::default();
        for (ix, secs) in [0, 1200, 2400, 3600, 4800].into_iter().enumerate() {
            let count = f64::from(u32::try_from(ix).unwrap_or_default());
            history.push(at(secs), requests(&[(ok.clone(), count)]), step);
        }
        let window = history.traffic(&TimeZone::UTC).map(|t| t.window);
        assert_eq!(window, Some(WINDOW));
    }

    #[test]
    fn state_reports_forbidden_metrics_and_keeps_health() {
        let mut state = State::default();
        let health = Probe::Ready(Health {
            checks: vec![Check {
                name: "ping".into(),
                passed: true,
            }],
        });
        let sampled = Sampled {
            health: health.clone(),
            metrics: Err("/metrics: forbidden".into()),
            webhooks: Probe::Unavailable("forbidden".into()),
            unavailable_services: Vec::new(),
        };
        state.record(sampled, at(0), Duration::from_secs(30));
        let view = state.view(&TimeZone::UTC);
        assert_eq!(
            view.traffic,
            Probe::Unavailable("/metrics: forbidden".into())
        );
        assert_eq!(view.health, health);
    }
}
