use std::time::Duration;

use crate::cluster_changes::ClusterChange;
use crate::control_plane::ControlPlane;
use crate::expiry::{Expiring, SupportEnd};
use crate::home::Problem;
use crate::pod_tops::PodTops;
use crate::resources::{Resource, ResourceKind};
use crate::{Page, Status, units};

pub const PERCENT: f32 = 100.;
const THOUSANDS_GROUP: usize = 3;
pub const NAMED_LIMIT: usize = 3;
const HOUR: Duration = Duration::from_hours(1);
const SIX_HOURS: Duration = Duration::from_hours(6);
const DAY: Duration = Duration::from_hours(24);
const WEEK: Duration = Duration::from_hours(7 * 24);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Capacity {
    pub used: f32,
    pub total: f32,
    pub unit: String,
    pub requests: f32,
    pub limits: f32,
}

impl Capacity {
    pub fn fraction(&self) -> f32 {
        if self.total > 0. {
            self.used / self.total
        } else {
            0.
        }
    }

    pub fn amount(&self) -> String {
        units::amount(self.used, self.total, &self.unit)
    }
}

pub fn percent(fraction: f32) -> String {
    format!("{:.0}%", fraction * PERCENT)
}

pub fn thousands(value: u32) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (ix, digit) in digits.chars().enumerate() {
        let remaining = digits.len() - ix;
        if ix > 0 && remaining.is_multiple_of(THOUSANDS_GROUP) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PodPhases {
    pub running: u32,
    pub pending: u32,
    pub failing: u32,
    pub unschedulable: u32,
}

impl PodPhases {
    pub const fn total(&self) -> u32 {
        self.running + self.pending + self.failing
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TimeRange {
    #[default]
    Hour,
    SixHours,
    Day,
    Week,
}

impl TimeRange {
    pub const ALL: [Self; 4] = [Self::Hour, Self::SixHours, Self::Day, Self::Week];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Hour => "1h",
            Self::SixHours => "6h",
            Self::Day => "24h",
            Self::Week => "7d",
        }
    }

    pub const fn duration(self) -> Duration {
        match self {
            Self::Hour => HOUR,
            Self::SixHours => SIX_HOURS,
            Self::Day => DAY,
            Self::Week => WEEK,
        }
    }

    pub const fn axis(self) -> [&'static str; 5] {
        match self {
            Self::Hour => ["−60m", "−45m", "−30m", "−15m", "now"],
            Self::SixHours => ["−6h", "−4.5h", "−3h", "−1.5h", "now"],
            Self::Day => ["−24h", "−18h", "−12h", "−6h", "now"],
            Self::Week => ["−7d", "−5.25d", "−3.5d", "−1.75d", "now"],
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Utilization {
    pub range: TimeRange,
    pub cpu: Vec<f32>,
    pub memory: Vec<f32>,
    pub cpu_positions: Vec<f32>,
    pub memory_positions: Vec<f32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Commitment {
    pub requests: f32,
    pub limits: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MetricsServer {
    #[default]
    Waiting,
    Answering,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UtilizationSource {
    Prometheus,
    Local {
        since: Option<String>,
        last: Option<String>,
        metrics_server: MetricsServer,
    },
    Error(String),
}

impl Default for UtilizationSource {
    fn default() -> Self {
        Self::Local {
            since: None,
            last: None,
            metrics_server: MetricsServer::default(),
        }
    }
}

impl UtilizationSource {
    pub fn note(&self) -> Option<String> {
        match self {
            Self::Local {
                since: Some(since), ..
            } => Some(format!("Recorded by Flock while it's open, since {since}")),
            Self::Local { since: None, .. } => None,
            Self::Prometheus => None,
            Self::Error(_) => None,
        }
    }

    pub fn error(&self) -> Option<&str> {
        match self {
            Self::Error(reason) => Some(reason),
            Self::Prometheus => None,
            Self::Local { .. } => None,
        }
    }

    pub fn empty_sentence(range: TimeRange) -> String {
        format!("No usage recorded in the last {}", range.label())
    }

    pub fn empty_reason(&self) -> String {
        match self {
            Self::Local {
                last,
                metrics_server,
                ..
            } => {
                let recording = match metrics_server {
                    MetricsServer::Waiting => {
                        "Flock records usage only while it's open and is waiting for metrics-server."
                    }
                    MetricsServer::Answering => "Flock records usage only while it's open.",
                    MetricsServer::Unavailable => {
                        "metrics-server isn't answering, so Flock can't record usage."
                    }
                };
                let last = last.as_ref().map_or_else(
                    || "No samples yet.".to_owned(),
                    |at| format!("Last sample: {at}."),
                );
                format!("This cluster has no Prometheus. {recording} {last}")
            }
            Self::Prometheus => "Prometheus returned no samples for this window.".into(),
            Self::Error(reason) => format!("Prometheus failed: {reason}"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overview {
    pub metrics_source: String,
    pub utilization_source: UtilizationSource,
    pub cpu: Commitment,
    pub memory: Commitment,
    pub pods: PodPhases,
    pub utilization: Vec<Utilization>,
    pub tops: PodTops,
    pub network: NetworkFacts,
    pub expiring: Vec<Expiring>,
    pub support: Option<SupportEnd>,
    pub unready_services: Vec<Problem>,
    pub changes: Vec<ClusterChange>,
    pub control_plane: ControlPlane,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetworkFacts {
    pub services_without_endpoints: Option<usize>,
    pub load_balancers_pending: usize,
    pub pvcs_pending: usize,
}

impl Overview {
    pub fn utilization(&self, range: TimeRange) -> Option<&Utilization> {
        self.utilization.iter().find(|u| u.range == range)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkloadHealth {
    pub page: Page,
    pub ok: usize,
    pub bad: usize,
    pub ok_label: String,
    pub bad_label: Option<String>,
    pub bad_status: Status,
}

impl WorkloadHealth {
    pub fn of(kind: ResourceKind, rows: &[Resource]) -> Self {
        let count = |wanted: Status| rows.iter().filter(|r| r.status == wanted).count();
        let total = rows.len();
        let ready = count(Status::Ok);
        let (ok, bad, ok_word, bad_word, bad_status) = match kind {
            ResourceKind::Deployment => (
                ready,
                total - ready,
                "available",
                "unavailable",
                Status::Err,
            ),
            ResourceKind::StatefulSet => (ready, total - ready, "ready", "not ready", Status::Err),
            ResourceKind::DaemonSet => (ready, total - ready, "ready", "not ready", Status::Err),
            ResourceKind::Job => (ready, count(Status::Err), "complete", "failed", Status::Err),
            ResourceKind::CronJob => {
                let suspended = count(Status::Suspended);
                (
                    total - suspended,
                    suspended,
                    "scheduled",
                    "suspended",
                    Status::Suspended,
                )
            }
            ResourceKind::Service => (total, 0, "", "", Status::Err),
            ResourceKind::Ingress => (total, 0, "", "", Status::Err),
            ResourceKind::ConfigMap => (total, 0, "", "", Status::Err),
            ResourceKind::Secret => (total, 0, "", "", Status::Err),
            ResourceKind::PersistentVolumeClaim => (total, 0, "", "", Status::Err),
        };
        let figure = |n: usize| thousands(u32::try_from(n).unwrap_or(u32::MAX));
        let ok_label = format!("{} {ok_word}", figure(ok)).trim_end().to_owned();
        Self {
            page: kind.page(),
            ok,
            bad,
            ok_label,
            bad_label: (bad > 0).then(|| format!("{} {bad_word}", figure(bad))),
            bad_status,
        }
    }
}

pub fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        format!("{count} {one}")
    } else {
        format!("{count} {many}")
    }
}

pub fn join_clauses(clauses: &[String]) -> String {
    match clauses {
        [] => String::new(),
        [only] => only.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

pub fn named(names: &[&str]) -> String {
    if names.len() <= NAMED_LIMIT {
        let all: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
        return join_clauses(&all);
    }
    let mut shown: Vec<String> = names
        .iter()
        .take(NAMED_LIMIT)
        .map(|name| (*name).to_owned())
        .collect();
    shown.push(format!("{} more", names.len() - NAMED_LIMIT));
    join_clauses(&shown)
}

pub const fn verb(count: usize, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::tests::resource;

    fn local(last: Option<&str>, metrics_server: MetricsServer) -> UtilizationSource {
        UtilizationSource::Local {
            since: Some("Oct 16, 08:49".into()),
            last: last.map(str::to_owned),
            metrics_server,
        }
    }

    #[test]
    fn history_messages_say_why_the_window_is_empty() {
        let reopened = local(Some("Oct 19, 12:28"), MetricsServer::Waiting);
        assert_eq!(
            reopened.note().as_deref(),
            Some("Recorded by Flock while it's open, since Oct 16, 08:49")
        );
        assert_eq!(
            UtilizationSource::empty_sentence(TimeRange::Hour),
            "No usage recorded in the last 1h"
        );
        assert_eq!(
            reopened.empty_reason(),
            "This cluster has no Prometheus. Flock records usage only while it's open and is waiting for metrics-server. Last sample: Oct 19, 12:28."
        );
        assert_eq!(
            local(Some("Oct 19, 12:28"), MetricsServer::Answering).empty_reason(),
            "This cluster has no Prometheus. Flock records usage only while it's open. Last sample: Oct 19, 12:28."
        );
        assert_eq!(
            local(None, MetricsServer::Unavailable).empty_reason(),
            "This cluster has no Prometheus. metrics-server isn't answering, so Flock can't record usage. No samples yet."
        );
        let fresh = UtilizationSource::default();
        assert_eq!(fresh.note(), None);
        assert_eq!(
            fresh.empty_reason(),
            "This cluster has no Prometheus. Flock records usage only while it's open and is waiting for metrics-server. No samples yet."
        );
        let failed = UtilizationSource::Error("forbidden".into());
        assert_eq!(failed.error(), Some("forbidden"));
        assert_eq!(failed.note(), None);
        assert_eq!(UtilizationSource::Prometheus.note(), None);
        assert_eq!(reopened.error(), None);
    }

    #[test]
    fn capacity_formats_amount_and_fraction() {
        let cpu = Capacity {
            used: 68.4,
            total: 112.,
            unit: "cores".into(),
            requests: 0.74,
            limits: 1.31,
        };
        assert_eq!(cpu.amount(), "68.4 / 112.0 cores");
        assert_eq!(percent(cpu.fraction()), "61%");
        assert_eq!(percent(cpu.limits), "131%");
        assert!(Capacity::default().fraction().abs() < f32::EPSILON);
    }

    #[test]
    fn thousands_groups_digits() {
        assert_eq!(thousands(7), "7");
        assert_eq!(thousands(412), "412");
        assert_eq!(thousands(1540), "1,540");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn workload_rows_read_like_the_design_per_kind() {
        let deployments = [
            resource("a", "1", Status::Ok),
            resource("a", "2", Status::Warn),
            resource("a", "3", Status::Err),
        ];
        let health = WorkloadHealth::of(ResourceKind::Deployment, &deployments);
        assert_eq!(health.ok_label, "1 available");
        assert_eq!(health.bad_label.as_deref(), Some("2 unavailable"));
        assert_eq!((health.ok, health.bad), (1, 2));
        let jobs = [
            resource("a", "1", Status::Ok),
            resource("a", "2", Status::Progress),
            resource("a", "3", Status::Err),
        ];
        let health = WorkloadHealth::of(ResourceKind::Job, &jobs);
        assert_eq!(health.ok_label, "1 complete");
        assert_eq!(health.bad_label.as_deref(), Some("1 failed"));
        assert_eq!(health.page, Page::Jobs);
        let crons = [
            resource("a", "1", Status::Ok),
            resource("a", "2", Status::Suspended),
        ];
        let health = WorkloadHealth::of(ResourceKind::CronJob, &crons);
        assert_eq!(health.ok_label, "1 scheduled");
        assert_eq!(health.bad_label.as_deref(), Some("1 suspended"));
        assert_eq!(health.bad_status, Status::Suspended);
        let daemons = [resource("a", "1", Status::Ok)];
        let health = WorkloadHealth::of(ResourceKind::DaemonSet, &daemons);
        assert_eq!(health.ok_label, "1 ready");
        assert_eq!(health.bad_label, None);
    }

    #[test]
    fn clauses_join_with_commas_and_and() {
        let clauses = |items: &[&str]| items.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(join_clauses(&[]), "");
        assert_eq!(join_clauses(&clauses(&["a"])), "a");
        assert_eq!(join_clauses(&clauses(&["a", "b"])), "a and b");
        assert_eq!(join_clauses(&clauses(&["a", "b", "c"])), "a, b and c");
        assert_eq!(plural(1, "node", "nodes"), "1 node");
        assert_eq!(plural(5, "pod", "pods"), "5 pods");
    }

    #[test]
    fn named_lists_up_to_the_limit_then_counts_the_rest() {
        assert_eq!(named(&[]), "");
        assert_eq!(named(&["a"]), "a");
        assert_eq!(named(&["a", "b", "c"]), "a, b and c");
        assert_eq!(named(&["a", "b", "c", "d", "e"]), "a, b, c and 2 more");
        assert_eq!(verb(1, "is", "are"), "is");
        assert_eq!(verb(2, "is", "are"), "are");
    }

    #[test]
    fn every_range_has_five_axis_labels_ending_now() {
        for range in TimeRange::ALL {
            assert_eq!(range.axis()[4], "now");
        }
    }
}
