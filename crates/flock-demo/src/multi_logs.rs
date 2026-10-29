use std::thread;
use std::time::Duration;

use flock_core::multilog::PodFacts;
use flock_core::pods::{ContainerState, Pod};
use flock_core::{
    Command, LogEvent, LogLevel, LogLine, LogSource, MultiLogRequest, Snapshot, SourceId, Stamp,
    Status,
};

const TICK: Duration = Duration::from_millis(900);
const START_MS: u64 = (21 * 3600 + 14 * 60) * 1000;
const STEP_MS: u64 = 640;
const POD_OFFSET_MS: u64 = 211;
const INITIAL_LINES: u64 = 90;
const CRASH_EVERY: u64 = 15;
const SIDECAR: &str = "envoy";
const NANOS_PER_MS: i128 = 1_000_000;
const MS_PER_SECOND: u64 = 1000;
const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 3600;
const REPLICA_SET: &str = "ReplicaSet";
const DEPLOYMENT: &str = "Deployment";
const NAME_SEPARATOR: char = '-';

const MESSAGES: [(LogLevel, &str); 9] = [
    (
        LogLevel::Info,
        r#"level=info msg="POST /v1/charge" route=/v1/charge status=200 dur=182ms"#,
    ),
    (
        LogLevel::Info,
        r#"{"level":"info","msg":"charge.create","amount":4200,"currency":"usd"}"#,
    ),
    (LogLevel::Debug, "stripe client: GET /v1/balance dur=91ms"),
    (LogLevel::Info, "GET /healthz 200 0.3ms"),
    (
        LogLevel::Warn,
        r#"level=warn msg="charge.create retry 1/3" status=502 upstream=checkout-api.payments.svc:8080"#,
    ),
    (
        LogLevel::Info,
        r#"level=info msg="POST /v1/charge" route=/v1/charge status=200 dur=204ms"#,
    ),
    (
        LogLevel::Error,
        r#"{"level":"error","msg":"POST /v1/charge","status":500,"err":"stripe: webhook secret not configured"}"#,
    ),
    (LogLevel::Debug, "cache hit sku:44718 ttl=287s"),
    (
        LogLevel::Warn,
        "slow query 412ms: SELECT … FROM carts WHERE id = $1",
    ),
];

const SIDECAR_MESSAGES: [(LogLevel, &str); 4] = [
    (
        LogLevel::Info,
        "GET /healthz 200 0.4ms upstream=127.0.0.1:8080",
    ),
    (
        LogLevel::Info,
        "POST /v1/charge 200 upstream=127.0.0.1:8080",
    ),
    (
        LogLevel::Warn,
        "upstream request timeout route=/v1/charge status=504",
    ),
    (
        LogLevel::Info,
        "GET /v1/cart/9318 200 upstream=127.0.0.1:8080",
    ),
];

const CRASH: [(LogLevel, &str); 4] = [
    (
        LogLevel::Info,
        "starting checkout-api v2.14.0 (commit 9f3c1ab)",
    ),
    (
        LogLevel::Warn,
        "env STRIPE_WEBHOOK_SECRET is empty; falling back to /etc/checkout/stripe.env",
    ),
    (
        LogLevel::Error,
        "open /etc/checkout/stripe.env: no such file or directory",
    ),
    (LogLevel::Error, "stripe webhook secret not set; exiting"),
];

pub fn handle(command: &Command, snapshot: &Snapshot) -> bool {
    let Command::StreamPodLogs(request) = command else {
        return false;
    };
    let sources = sources(snapshot, request);
    let request = request.clone();
    let spawned = thread::Builder::new()
        .name("flock-demo-multi-logs".into())
        .spawn(move || stream(&request, &sources));
    if let Err(error) = spawned {
        tracing::error!(%error, "demo multi-log worker failed to start");
    }
    true
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Source {
    source: LogSource,
    crashing: bool,
}

fn owner(pod: &Pod) -> (Option<&str>, Option<&str>) {
    let Some(owner) = &pod.owner else {
        return (None, None);
    };
    if owner.kind == REPLICA_SET
        && let Some((deployment, _)) = owner.name.rsplit_once(NAME_SEPARATOR)
    {
        return (Some(DEPLOYMENT), Some(deployment));
    }
    (Some(owner.kind.as_str()), Some(owner.name.as_str()))
}

fn sources(snapshot: &Snapshot, request: &MultiLogRequest) -> Vec<Source> {
    let cluster = request.target.cluster();
    snapshot
        .pods
        .iter()
        .filter(|pod| &pod.cluster == cluster)
        .filter(|pod| {
            let (owner_kind, owner_name) = owner(pod);
            let facts = PodFacts {
                namespace: &pod.namespace,
                name: &pod.name,
                owner_kind,
                owner_name,
                labels: &pod.labels,
            };
            request.target.matches(&facts)
        })
        .flat_map(|pod| {
            pod.containers
                .iter()
                .filter(|container| {
                    container.state != ContainerState::Waiting || container.restarts > 0
                })
                .map(|container| Source {
                    source: LogSource {
                        namespace: pod.namespace.clone(),
                        pod: pod.name.clone(),
                        container: container.name.clone(),
                    },
                    crashing: container.status == Status::Err,
                })
        })
        .collect()
}

fn clock(ms: u64) -> String {
    let seconds = ms / MS_PER_SECOND;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        seconds / SECONDS_PER_HOUR,
        seconds % SECONDS_PER_HOUR / SECONDS_PER_MINUTE,
        seconds % SECONDS_PER_MINUTE,
        ms % MS_PER_SECOND
    )
}

fn line(ix: usize, n: u64, source: &Source) -> (Option<Stamp>, LogLine) {
    let offset = u64::try_from(ix).unwrap_or_default() * POD_OFFSET_MS;
    let ms = START_MS + n * STEP_MS + offset;
    let message_ix = usize::try_from(n).unwrap_or_default() + ix;
    let messages: &[(LogLevel, &str)] = if source.crashing {
        &CRASH
    } else if source.source.container == SIDECAR {
        &SIDECAR_MESSAGES
    } else {
        &MESSAGES
    };
    let crash_step = usize::try_from(n % CRASH_EVERY).unwrap_or_default();
    let pick = if source.crashing {
        crash_step
    } else {
        message_ix % messages.len()
    };
    let (level, message) = messages.get(pick).copied().unwrap_or(CRASH[0]);
    let stamp = Stamp(i128::from(ms) * NANOS_PER_MS);
    (Some(stamp), LogLine::new(&clock(ms), level, message))
}

fn speaks(source: &Source, n: u64) -> bool {
    !source.crashing || n % CRASH_EVERY < u64::try_from(CRASH.len()).unwrap_or_default()
}

fn stream(request: &MultiLogRequest, sources: &[Source]) {
    let ids: Vec<SourceId> = (0..sources.len())
        .map(|ix| SourceId(u32::try_from(ix).unwrap_or_default()))
        .collect();
    for (id, source) in ids.iter().zip(sources) {
        if !request
            .sink
            .send_blocking(LogEvent::Joined(*id, source.source.clone()))
        {
            return;
        }
    }
    let send = |n: u64| {
        ids.iter()
            .zip(sources)
            .enumerate()
            .all(|(ix, (id, source))| {
                if !speaks(source, n) {
                    return true;
                }
                let (stamp, line) = line(ix, n, source);
                request.sink.send_blocking(LogEvent::Line(*id, stamp, line))
            })
    };
    for n in 0..INITIAL_LINES {
        if !send(n) {
            return;
        }
    }
    for n in INITIAL_LINES.. {
        thread::sleep(TICK);
        if !send(n) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use flock_core::{LogTarget, ResourceRef, Sink};

    use super::*;

    #[test]
    fn a_workload_target_streams_its_replica_sets_pods() {
        let snapshot = crate::snapshot();
        let (sink, events) = Sink::channel();
        let request = MultiLogRequest {
            target: LogTarget::Workload(ResourceRef {
                cluster: flock_core::ClusterId::new(crate::PROD_US_EAST),
                kind: DEPLOYMENT.into(),
                namespace: Some("payments".into()),
                name: "checkout-api".into(),
            }),
            sink,
        };
        let pods: Vec<String> = sources(&snapshot, &request)
            .into_iter()
            .map(|source| source.source.pod)
            .collect();
        assert!(pods.len() >= 2, "{pods:?}");
        assert!(pods.iter().all(|pod| pod.starts_with("checkout-api-")));
        drop(events);
    }

    fn demo_source(container: &str, crashing: bool) -> Source {
        Source {
            source: LogSource {
                namespace: "payments".into(),
                pod: "checkout-api-7f9c6d-x2lqp".into(),
                container: container.into(),
            },
            crashing,
        }
    }

    #[test]
    fn demo_lines_from_different_pods_interleave_in_time() {
        let api = demo_source("api", false);
        let (a, _) = line(0, 1, &api);
        let (b, _) = line(1, 1, &api);
        let (next, _) = line(0, 2, &api);
        assert!(a < b && b < next);
    }

    #[test]
    fn demo_sources_speak_every_level_and_crashing_ones_loop() {
        let api = demo_source("api", false);
        let levels: std::collections::BTreeSet<String> = (0..9)
            .filter_map(|n| line(0, n, &api).1.level)
            .map(|level| format!("{level:?}"))
            .collect();
        assert_eq!(levels.len(), 4, "{levels:?}");
        let crashing = demo_source("api", true);
        let spoken: Vec<u64> = (0..CRASH_EVERY * 2)
            .filter(|n| speaks(&crashing, *n))
            .collect();
        assert_eq!(spoken, [0, 1, 2, 3, 15, 16, 17, 18]);
        assert_eq!(line(0, 3, &crashing).1.level, Some(LogLevel::Error));
    }
}
