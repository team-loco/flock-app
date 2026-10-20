use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use flock_core::overview::{TimeRange, Utilization};
use jiff::{SignedDuration, Timestamp};
use k8s_openapi::api::core::v1::Node;
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;

use crate::error::Result;
use crate::manager::ClusterHandle;
use crate::metrics::{NodeUsage, RangeQuery, Series, Usage, cpu_millicores, parse_quantity};
use crate::probe::PrometheusTarget;
use serde::{Deserialize, Serialize};

const PERCENT: f64 = 100.;
const MILLI_PER_CORE: f64 = 1e3;
const BYTES_PER_GIB: f64 = 1_073_741_824.;
const CPU: &str = "cpu";
const MEMORY: &str = "memory";
const CHART_POINTS: u32 = 13;
const HOUR_SECS: u64 = 3600;
pub const HISTORY: SignedDuration = SignedDuration::from_hours(24 * 7);
const FULL_RESOLUTION: SignedDuration = SignedDuration::from_hours(1);
const OLDER_BUCKET: SignedDuration = SignedDuration::from_mins(2);
pub const MAX_SAMPLES: usize = 10_000;
const CPU_QUERY: &str = "100 * (1 - avg(rate(node_cpu_seconds_total{mode=\"idle\"}[5m])))";
const MEMORY_QUERY: &str =
    "100 * (1 - sum(node_memory_MemAvailable_bytes) / sum(node_memory_MemTotal_bytes))";

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Allocatable {
    pub cpu_millicores: f64,
    pub memory_bytes: f64,
}

fn quantity<'a>(
    map: Option<&'a std::collections::BTreeMap<String, Quantity>>,
    key: &str,
) -> Option<&'a str> {
    map?.get(key).map(|q| q.0.as_str())
}

pub fn allocatable(node: &Node) -> Allocatable {
    let map = node.status.as_ref().and_then(|s| s.allocatable.as_ref());
    Allocatable {
        cpu_millicores: quantity(map, CPU)
            .and_then(cpu_millicores)
            .unwrap_or_default(),
        memory_bytes: quantity(map, MEMORY)
            .and_then(parse_quantity)
            .unwrap_or_default(),
    }
}

pub fn cores(millicores: f64) -> f32 {
    narrow(millicores / MILLI_PER_CORE)
}

pub fn gib(bytes: f64) -> f32 {
    narrow(bytes / BYTES_PER_GIB)
}

#[allow(clippy::cast_possible_truncation)]
pub fn narrow(value: f64) -> f32 {
    value as f32
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub cpu: f32,
    pub memory: f32,
}

impl Sample {
    pub fn percents(&self) -> (u8, u8) {
        (whole_percent(self.cpu), whole_percent(self.memory))
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn whole_percent(value: f32) -> u8 {
    value.round().clamp(0., f32::from(u8::MAX)) as u8
}

pub fn sample(nodes: &[Arc<Node>], usage: &[NodeUsage]) -> Option<Sample> {
    let used: HashMap<&str, &Usage> = usage.iter().map(|u| (u.name.as_str(), &u.usage)).collect();
    let mut total = Allocatable::default();
    let mut busy = Allocatable::default();
    for node in nodes {
        let name = node.metadata.name.as_deref().unwrap_or_default();
        let Some(usage) = used.get(name) else {
            continue;
        };
        let capacity = allocatable(node);
        total.cpu_millicores += capacity.cpu_millicores;
        total.memory_bytes += capacity.memory_bytes;
        busy.cpu_millicores += usage.cpu_millicores;
        busy.memory_bytes += usage.memory_bytes;
    }
    if total.cpu_millicores <= 0. || total.memory_bytes <= 0. {
        return None;
    }
    Some(Sample {
        cpu: narrow(PERCENT * busy.cpu_millicores / total.cpu_millicores),
        memory: narrow(PERCENT * busy.memory_bytes / total.memory_bytes),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimedSample {
    pub at: Timestamp,
    pub sample: Sample,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UsageRing {
    samples: VecDeque<TimedSample>,
}

impl UsageRing {
    pub fn push(&mut self, at: Timestamp, sample: Sample) {
        if !sample.cpu.is_finite() || !sample.memory.is_finite() {
            return;
        }
        if self.samples.back().is_some_and(|last| last.at >= at) {
            return;
        }
        self.samples.push_back(TimedSample { at, sample });
        self.prune(at);
    }

    pub fn prune(&mut self, now: Timestamp) {
        let oldest = now.saturating_sub(HISTORY).unwrap_or(Timestamp::MIN);
        let recent = now
            .saturating_sub(FULL_RESOLUTION)
            .unwrap_or(Timestamp::MIN);
        let mut bucket = None;
        self.samples.retain(|sample| {
            if sample.at < oldest || sample.at > now {
                return false;
            }
            if sample.at >= recent {
                return true;
            }
            let next = sample.at.as_second().div_euclid(OLDER_BUCKET.as_secs());
            if bucket == Some(next) {
                return false;
            }
            bucket = Some(next);
            true
        });
        while self.samples.len() > MAX_SAMPLES {
            self.samples.pop_front();
        }
    }

    pub fn valid(&self) -> bool {
        let finite = self
            .samples
            .iter()
            .all(|s| s.sample.cpu.is_finite() && s.sample.memory.is_finite());
        let ordered = self
            .samples
            .iter()
            .zip(self.samples.iter().skip(1))
            .all(|(a, b)| a.at < b.at);
        self.samples.len() <= MAX_SAMPLES && finite && ordered
    }

    pub fn since(&self) -> Option<Timestamp> {
        self.samples.front().map(|s| s.at)
    }

    pub fn latest(&self) -> Option<Sample> {
        self.samples.back().map(|s| s.sample)
    }

    pub fn last_at(&self) -> Option<Timestamp> {
        self.samples.back().map(|s| s.at)
    }

    #[allow(clippy::cast_precision_loss)]
    pub fn utilization(&self, range: TimeRange, now: Timestamp) -> Utilization {
        let query = range_query("", range, now);
        let points: Vec<_> = self
            .samples
            .iter()
            .filter(|s| s.at >= query.start && s.at <= now)
            .collect();
        let positions: Vec<_> = points
            .iter()
            .map(|s| position(s.at.as_second() as f64, &query))
            .collect();
        Utilization {
            range,
            cpu: points.iter().map(|s| s.sample.cpu).collect(),
            memory: points.iter().map(|s| s.sample.memory).collect(),
            cpu_positions: positions.clone(),
            memory_positions: positions,
        }
    }
}

#[allow(clippy::cast_precision_loss)]
fn position(at: f64, query: &RangeQuery) -> f32 {
    let start = query.start.as_second() as f64;
    let span = (query.end.as_second() - query.start.as_second()) as f64;
    narrow((at - start) / span)
}

pub fn window(range: TimeRange) -> Duration {
    let hours = match range {
        TimeRange::Hour => 1,
        TimeRange::SixHours => 6,
        TimeRange::Day => 24,
        TimeRange::Week => 24 * 7,
    };
    Duration::from_secs(hours * HOUR_SECS)
}

pub fn range_query(query: &str, range: TimeRange, now: Timestamp) -> RangeQuery {
    let span = window(range);
    let step = span.as_secs() / u64::from(CHART_POINTS - 1);
    let back = SignedDuration::try_from(span).unwrap_or_default();
    RangeQuery {
        query: query.to_owned(),
        start: now.checked_sub(back).unwrap_or(now),
        end: now,
        step_seconds: u32::try_from(step).unwrap_or(u32::MAX),
    }
}

pub fn values(series: &[Series]) -> Vec<f32> {
    series
        .first()
        .map(|s| s.points.iter().map(|(_, v)| narrow(*v)).collect())
        .unwrap_or_default()
}

fn positions(series: &[Series], query: &RangeQuery) -> Vec<f32> {
    series
        .first()
        .map(|s| {
            s.points
                .iter()
                .map(|(at, _)| position(*at, query))
                .collect()
        })
        .unwrap_or_default()
}

async fn utilization(
    handle: &ClusterHandle,
    target: &PrometheusTarget,
    range: TimeRange,
    now: Timestamp,
) -> Result<Utilization> {
    let cpu_query = range_query(CPU_QUERY, range, now);
    let memory_query = range_query(MEMORY_QUERY, range, now);
    let (cpu, memory) = futures::try_join!(
        handle.prometheus_range(target, &cpu_query),
        handle.prometheus_range(target, &memory_query),
    )?;
    Ok(Utilization {
        range,
        cpu: values(&cpu),
        memory: values(&memory),
        cpu_positions: positions(&cpu, &cpu_query),
        memory_positions: positions(&memory, &memory_query),
    })
}

pub async fn prometheus_utilization(
    handle: &ClusterHandle,
    target: &PrometheusTarget,
    now: Timestamp,
) -> Result<Vec<Utilization>> {
    let ranges = TimeRange::ALL.map(|range| utilization(handle, target, range, now));
    futures::future::try_join_all(ranges).await
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::problems::tests::parse;

    const ROUND_TRIP: Duration = Duration::from_secs(1);
    const EMPTY_MATRIX: &str = r#"{"status":"success","data":{"resultType":"matrix","result":[]}}"#;

    #[tokio::test(start_paused = true)]
    async fn every_range_query_runs_at_once() {
        let (client, traffic) = crate::fake::client(ROUND_TRIP, |_, path| {
            path.contains("/proxy/")
                .then(|| (http::StatusCode::OK, EMPTY_MATRIX.to_owned()))
        });
        let (_manager, handle) = crate::fake::handle(&client);
        let target = PrometheusTarget {
            namespace: "monitoring".to_owned(),
            service: "prometheus".to_owned(),
            port: 9090,
            scheme: "http".to_owned(),
        };
        let started = tokio::time::Instant::now();
        let ranges = prometheus_utilization(&handle, &target, Timestamp::UNIX_EPOCH).await;
        assert!(started.elapsed() < ROUND_TRIP * 2);
        assert_eq!(ranges.map(|r| r.len()).ok(), Some(TimeRange::ALL.len()));
        assert!(traffic.peak() >= TimeRange::ALL.len() * 2);
    }

    #[tokio::test(start_paused = true)]
    async fn prometheus_errors_preserve_the_reason() {
        let (client, _) = crate::fake::client(ROUND_TRIP, |_, path| {
            path.contains("/proxy/").then(|| {
                (
                    http::StatusCode::OK,
                    r#"{"status":"error","error":"permission denied"}"#.to_owned(),
                )
            })
        });
        let (_manager, handle) = crate::fake::handle(&client);
        let target = PrometheusTarget {
            namespace: "monitoring".to_owned(),
            service: "prometheus".to_owned(),
            port: 9090,
            scheme: "http".to_owned(),
        };
        let result = prometheus_utilization(&handle, &target, Timestamp::UNIX_EPOCH).await;
        assert!(
            matches!(result, Err(crate::Error::Prometheus(reason)) if reason == "permission denied")
        );
    }

    fn node(name: &str, cpu: &str, memory: &str) -> Arc<Node> {
        parse(json!({
            "metadata": { "name": name },
            "status": { "allocatable": { "cpu": cpu, "memory": memory } }
        }))
    }

    fn used(name: &str, millicores: f64, gib: f64) -> NodeUsage {
        NodeUsage {
            name: name.into(),
            usage: Usage {
                cpu_millicores: millicores,
                memory_bytes: gib * BYTES_PER_GIB,
            },
        }
    }

    #[test]
    fn sample_divides_usage_by_allocatable_of_reporting_nodes() {
        let nodes = [
            node("a", "4", "8Gi"),
            node("b", "4000m", "8Gi"),
            node("c", "4", "8Gi"),
        ];
        let usage = [used("a", 1000., 2.), used("b", 3000., 6.)];
        let Some(found) = sample(&nodes, &usage) else {
            panic!("sample");
        };
        assert_eq!(found.percents(), (50, 50));
        assert_eq!(sample(&nodes, &[]), None);
    }

    fn timestamp(seconds: i64) -> Timestamp {
        Timestamp::from_second(seconds).unwrap_or_default()
    }

    fn steady(ring: &mut UsageRing, seconds: impl Iterator<Item = i64>) {
        for at in seconds {
            ring.push(
                timestamp(at),
                Sample {
                    cpu: 10.,
                    memory: 20.,
                },
            );
        }
    }

    #[test]
    fn ring_keeps_ordered_finite_samples_and_places_them_in_each_window() {
        let mut ring = UsageRing::default();
        steady(&mut ring, 1..=4);
        let stale = Sample {
            cpu: 9.,
            memory: 9.,
        };
        ring.push(timestamp(3), stale);
        let broken = Sample {
            cpu: f32::NAN,
            memory: 9.,
        };
        ring.push(timestamp(5), broken);
        assert_eq!(ring.samples.len(), 4);
        assert_eq!(ring.latest().map(|s| s.cpu), Some(10.));
        assert_eq!(ring.since(), Some(timestamp(1)));
        for range in TimeRange::ALL {
            let found = ring.utilization(range, timestamp(4));
            assert_eq!(found.cpu.len(), 4);
            assert!(found.cpu_positions.first().is_some_and(|at| *at > 0.99));
            assert_eq!(found.cpu_positions.last(), Some(&1.));
        }
        let later = ring.utilization(TimeRange::Hour, timestamp(7200));
        assert_eq!(later.cpu, Vec::<f32>::new());
        assert_eq!(
            ring.utilization(TimeRange::SixHours, timestamp(7200))
                .cpu
                .len(),
            4
        );
    }

    #[test]
    fn history_from_earlier_sessions_leaves_the_last_hour_empty_and_plots_the_rest() {
        const DAY: i64 = 86_400;
        const SESSION: i64 = 3 * 3600;
        let mut ring = UsageRing::default();
        steady(&mut ring, (0..=SESSION).step_by(15));
        steady(&mut ring, (3 * DAY..=3 * DAY + SESSION).step_by(15));
        let reopened = timestamp(3 * DAY + SESSION + 20 * 3600);
        ring.prune(reopened);
        assert_eq!(ring.since(), Some(timestamp(0)));
        assert_eq!(ring.last_at(), Some(timestamp(3 * DAY + SESSION)));
        let hour = ring.utilization(TimeRange::Hour, reopened);
        assert_eq!(hour.cpu, Vec::<f32>::new());
        let six = ring.utilization(TimeRange::SixHours, reopened);
        assert_eq!(six.cpu, Vec::<f32>::new());
        let day = ring.utilization(TimeRange::Day, reopened);
        assert_ne!(day.cpu, Vec::<f32>::new());
        assert!(day.cpu_positions.iter().all(|at| (0. ..0.2).contains(at)));
        let week = ring.utilization(TimeRange::Week, reopened);
        assert!(week.cpu.len() > day.cpu.len());
        ring.push(
            reopened,
            Sample {
                cpu: 3.,
                memory: 4.,
            },
        );
        let hour = ring.utilization(TimeRange::Hour, reopened);
        assert_eq!(hour.cpu, [3.]);
        assert_eq!(hour.cpu_positions, [1.]);
    }

    #[test]
    fn samples_older_than_an_hour_keep_one_point_per_two_minutes() {
        let mut ring = UsageRing::default();
        steady(&mut ring, (0..=7200).step_by(5));
        let old: Vec<_> = ring
            .samples
            .iter()
            .filter(|s| s.at < timestamp(3600))
            .collect();
        assert_eq!(old.len(), 30);
        assert!(
            old.iter()
                .zip(old.iter().skip(1))
                .all(|(a, b)| b.at.duration_since(a.at) >= OLDER_BUCKET)
        );
        let hour = ring.utilization(TimeRange::Hour, timestamp(7200));
        assert_eq!(hour.cpu.len(), 721);
    }

    #[test]
    fn seven_days_stay_under_the_sample_bound_and_older_points_drop() {
        let mut ring = UsageRing::default();
        steady(
            &mut ring,
            (0..=601_200)
                .step_by(120)
                .chain((601_201..=604_800).step_by(1)),
        );
        assert!(ring.samples.len() <= MAX_SAMPLES);
        assert_eq!(ring.since(), Some(timestamp(0)));
        assert!(ring.valid());
        let week = ring.utilization(TimeRange::Week, timestamp(604_800));
        assert!(week.cpu_positions.first().is_some_and(|at| *at == 0.));
        ring.prune(timestamp(608_400));
        assert!(ring.since().is_some_and(|since| since >= timestamp(3600)));
    }

    #[test]
    fn range_queries_span_the_window_in_twelve_steps() {
        let now = Timestamp::from_second(1_800_000_000).unwrap_or_default();
        let query = range_query(CPU_QUERY, TimeRange::Day, now);
        assert_eq!(query.end.as_second() - query.start.as_second(), 86_400);
        assert_eq!(query.step_seconds, 7_200);
    }
}
