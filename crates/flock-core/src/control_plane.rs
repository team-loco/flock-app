use std::collections::BTreeMap;
use std::time::Duration;

use crate::Status;
use crate::issue::{Issue, object_line};
use crate::overview::thousands;

const MILLIS_PER_SECOND: f64 = 1e3;
const THOUSAND: f64 = 1e3;
const COMPACT_FROM: f64 = 1e4;
const PERCENT: f64 = 100.;
const MILLION: f64 = 1e6;
const BILLION: f64 = 1e9;
const WARN_ERROR_RATIO: f64 = 0.01;
const SLOW_WEBHOOK_SHARE: f64 = 0.25;
const SERVER_ERROR_CLASS: char = '5';
const API_SERVICE: &str = "APIService";
const UNAVAILABLE: &str = "Available=False";
pub const ETCD_CHECK: &str = "etcd";

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Probe<T> {
    #[default]
    Waiting,
    Unavailable(String),
    Ready(T),
}

impl<T> Probe<T> {
    pub const fn ready(&self) -> Option<&T> {
        match self {
            Self::Ready(value) => Some(value),
            Self::Waiting => None,
            Self::Unavailable(_) => None,
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Unavailable(reason) => Some(reason),
            Self::Waiting => None,
            Self::Ready(_) => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ControlPlane {
    pub health: Probe<Health>,
    pub traffic: Probe<Traffic>,
    pub webhooks: Probe<Vec<Webhook>>,
    pub unavailable_services: Vec<ApiService>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Health {
    pub checks: Vec<Check>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub passed: bool,
}

impl Health {
    pub fn failing(&self) -> Vec<&str> {
        let mut failing: Vec<&str> = self
            .checks
            .iter()
            .filter(|check| !check.passed)
            .map(|check| check.name.as_str())
            .collect();
        failing.dedup();
        failing
    }

    pub fn passes(&self, name: &str) -> Option<bool> {
        let mut matching = self.checks.iter().filter(|check| check.name == name);
        let first = matching.next()?;
        Some(first.passed && matching.all(|check| check.passed))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Traffic {
    pub since: String,
    pub until: String,
    pub window: Duration,
    pub requests: u64,
    pub errors: u64,
    pub p99: Option<Duration>,
    pub etcd_p99: Option<Duration>,
    pub errors_per_minute: Vec<u64>,
    pub failures: Vec<FailedRequests>,
    pub webhook_p99: Vec<(String, Duration)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailedRequests {
    pub code: String,
    pub verb: String,
    pub resource: String,
    pub count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Webhook {
    pub name: String,
    pub configuration: String,
    pub timeout: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiService {
    pub name: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlowWebhook {
    pub name: String,
    pub configuration: String,
    pub p99: Duration,
    pub timeout: Duration,
}

impl SlowWebhook {
    pub fn detail(&self) -> String {
        format!("{} {}", self.name, self.timing())
    }

    pub fn timing(&self) -> String {
        format!(
            "p99 {} (timeout {})",
            latency(self.p99),
            latency(self.timeout)
        )
    }
}

pub fn is_server_error(code: &str) -> bool {
    code.len() == 3 && code.starts_with(SERVER_ERROR_CLASS)
}

impl Traffic {
    #[allow(clippy::cast_precision_loss)]
    pub fn error_ratio(&self) -> f64 {
        if self.requests == 0 {
            return 0.;
        }
        self.errors as f64 / self.requests as f64
    }

    pub fn status(&self) -> Status {
        if self.error_ratio() >= WARN_ERROR_RATIO {
            Status::Warn
        } else {
            Status::Ok
        }
    }

    pub fn error_share(&self) -> String {
        format!("{:.2}%", self.error_ratio() * PERCENT)
    }

    pub fn summary(&self) -> String {
        let counts = format!(
            "{} of {} requests",
            compact(self.errors),
            compact(self.requests)
        );
        match self.p99 {
            Some(p99) => format!("p99 {} · {counts}", latency(p99)),
            None => counts,
        }
    }

    pub fn codes(&self) -> Vec<(String, u64)> {
        let mut totals: BTreeMap<&str, u64> = BTreeMap::new();
        for failure in &self.failures {
            *totals.entry(failure.code.as_str()).or_default() += failure.count;
        }
        let mut codes: Vec<(String, u64)> = totals
            .into_iter()
            .map(|(code, count)| (code.to_owned(), count))
            .collect();
        codes.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        codes
    }

    pub fn failures_with(&self, code: Option<&str>) -> Vec<&FailedRequests> {
        self.failures
            .iter()
            .filter(|failure| code.is_none_or(|code| failure.code == code))
            .collect()
    }
}

impl ControlPlane {
    pub fn issues(&self) -> Vec<Issue> {
        let services = self.unavailable_services.iter().map(|service| {
            let object = object_line([
                Some(format!("{API_SERVICE} {}", service.name)),
                Some(UNAVAILABLE.to_owned()),
            ]);
            Issue::new(Status::Err, service.reason.clone(), String::new(), object)
        });
        let webhooks = self.slow_webhooks().into_iter().map(|webhook| {
            let timing = webhook.timing();
            Issue::new(Status::Warn, webhook.name, timing, webhook.configuration)
        });
        services.chain(webhooks).collect()
    }

    pub fn slow_webhooks(&self) -> Vec<SlowWebhook> {
        let (Some(traffic), Some(webhooks)) = (self.traffic.ready(), self.webhooks.ready()) else {
            return Vec::new();
        };
        let mut slow: Vec<SlowWebhook> = webhooks
            .iter()
            .filter_map(|webhook| {
                let p99 = traffic
                    .webhook_p99
                    .iter()
                    .find(|(name, _)| *name == webhook.name)
                    .map(|(_, p99)| *p99)?;
                let limit = webhook.timeout.mul_f64(SLOW_WEBHOOK_SHARE);
                (p99 >= limit).then(|| SlowWebhook {
                    name: webhook.name.clone(),
                    configuration: webhook.configuration.clone(),
                    p99,
                    timeout: webhook.timeout,
                })
            })
            .collect();
        slow.sort_by_key(|webhook| std::cmp::Reverse(webhook.p99));
        slow
    }

    pub fn slowest_webhook(&self) -> Option<(String, Duration)> {
        let traffic = self.traffic.ready()?;
        traffic
            .webhook_p99
            .iter()
            .max_by_key(|(_, p99)| *p99)
            .cloned()
    }
}

pub fn latency(duration: Duration) -> String {
    let millis = duration.as_secs_f64() * MILLIS_PER_SECOND;
    if millis < MILLIS_PER_SECOND {
        format!("{millis:.0} ms")
    } else {
        format!("{:.1} s", duration.as_secs_f64())
    }
}

#[allow(clippy::cast_precision_loss)]
pub fn compact(count: u64) -> String {
    let value = count as f64;
    if value >= BILLION {
        format!("{:.2}B", value / BILLION)
    } else if value >= MILLION {
        format!("{:.2}M", value / MILLION)
    } else if value >= COMPACT_FROM {
        format!("{:.0}k", value / THOUSAND)
    } else {
        grouped(count)
    }
}

pub fn grouped(count: u64) -> String {
    thousands(u32::try_from(count).unwrap_or(u32::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn traffic() -> Traffic {
        Traffic {
            requests: 1_071_382,
            errors: 214,
            p99: Some(Duration::from_millis(182)),
            failures: vec![
                failed("503", "GET", "pods.metrics.k8s.io", 148),
                failed("504", "LIST", "pods", 41),
                failed("503", "GET", "nodes.metrics.k8s.io", 10),
                failed("500", "CREATE", "pods", 15),
            ],
            webhook_p99: vec![
                (
                    "validate.kyverno.svc-fail".into(),
                    Duration::from_millis(2800),
                ),
                ("fast.example.com".into(), Duration::from_millis(40)),
            ],
            ..Traffic::default()
        }
    }

    fn failed(code: &str, verb: &str, resource: &str, count: u64) -> FailedRequests {
        FailedRequests {
            code: code.into(),
            verb: verb.into(),
            resource: resource.into(),
            count,
        }
    }

    fn webhook(name: &str, timeout: u64) -> Webhook {
        Webhook {
            name: name.into(),
            configuration: "kyverno-resource-validating".into(),
            timeout: Duration::from_secs(timeout),
        }
    }

    #[test]
    fn api_server_row_reads_like_the_design() {
        let traffic = traffic();
        assert_eq!(traffic.summary(), "p99 182 ms · 214 of 1.07M requests");
        assert_eq!(traffic.error_share(), "0.02%");
        assert_eq!(traffic.status(), Status::Ok);
        let failing = Traffic {
            errors: 20_000,
            ..traffic
        };
        assert_eq!(failing.status(), Status::Warn);
        assert_eq!(Traffic::default().error_share(), "0.00%");
    }

    #[test]
    fn codes_total_their_requests_busiest_first() {
        let traffic = traffic();
        assert_eq!(
            traffic.codes(),
            vec![("503".into(), 158), ("504".into(), 41), ("500".into(), 15)]
        );
        assert_eq!(traffic.failures_with(Some("503")).len(), 2);
        assert_eq!(traffic.failures_with(None).len(), 4);
    }

    #[test]
    fn webhooks_are_slow_past_a_quarter_of_their_timeout() {
        let plane = ControlPlane {
            traffic: Probe::Ready(traffic()),
            webhooks: Probe::Ready(vec![
                webhook("validate.kyverno.svc-fail", 10),
                webhook("fast.example.com", 10),
                webhook("unused.example.com", 1),
            ]),
            ..ControlPlane::default()
        };
        let slow = plane.slow_webhooks();
        assert_eq!(slow.len(), 1);
        assert_eq!(
            slow[0].detail(),
            "validate.kyverno.svc-fail p99 2.8 s (timeout 10.0 s)"
        );
        assert_eq!(
            plane.slowest_webhook().map(|(name, _)| name),
            Some("validate.kyverno.svc-fail".into())
        );
        assert_eq!(ControlPlane::default().slow_webhooks(), Vec::new());
        let plane = ControlPlane {
            unavailable_services: vec![ApiService {
                name: "v1beta1.metrics.k8s.io".into(),
                reason: "FailedDiscoveryCheck".into(),
            }],
            ..plane
        };
        let issues = plane.issues();
        let reasons: Vec<&str> = issues.iter().map(|i| i.reason.as_str()).collect();
        assert_eq!(
            reasons,
            ["FailedDiscoveryCheck", "validate.kyverno.svc-fail"]
        );
        assert_eq!(
            issues[0].object,
            "APIService v1beta1.metrics.k8s.io · Available=False"
        );
        assert_eq!(issues[1].message, "p99 2.8 s (timeout 10.0 s)");
        assert_eq!(ControlPlane::default().issues(), Vec::new());
    }

    #[test]
    fn health_reports_failing_checks_once() {
        let check = |name: &str, passed| Check {
            name: name.into(),
            passed,
        };
        let health = Health {
            checks: vec![
                check("ping", true),
                check(ETCD_CHECK, false),
                check(ETCD_CHECK, false),
                check("log", true),
            ],
        };
        assert_eq!(health.failing(), vec![ETCD_CHECK]);
        assert_eq!(health.passes(ETCD_CHECK), Some(false));
        assert_eq!(health.passes("ping"), Some(true));
        assert_eq!(health.passes("missing"), None);
    }

    #[test]
    fn numbers_format_compactly() {
        assert_eq!(compact(214), "214");
        assert_eq!(compact(9_999), "9,999");
        assert_eq!(compact(48_200), "48k");
        assert_eq!(compact(1_071_382), "1.07M");
        assert_eq!(compact(2_500_000_000), "2.50B");
        assert_eq!(latency(Duration::from_millis(182)), "182 ms");
        assert_eq!(latency(Duration::from_millis(2800)), "2.8 s");
        assert!(is_server_error("503"));
        assert!(!is_server_error("404"));
        assert!(!is_server_error("5"));
    }
}
