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
const INITIAL_LINES: u64 = 12;
const NANOS_PER_MS: i128 = 1_000_000;
const MS_PER_SECOND: u64 = 1000;
const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 3600;
const REPLICA_SET: &str = "ReplicaSet";
const DEPLOYMENT: &str = "Deployment";
const NAME_SEPARATOR: char = '-';

const MESSAGES: [(LogLevel, &str); 8] = [
    (LogLevel::Info, "GET /healthz 200 0.3ms"),
    (LogLevel::Info, "POST /v1/cart/9312/items 201 18.2ms"),
    (LogLevel::Info, "GET /v1/cart/9312 200 6.9ms"),
    (LogLevel::Debug, "cache hit sku:44718 ttl=287s"),
    (LogLevel::Info, "POST /v1/checkout 200 96.1ms"),
    (
        LogLevel::Warn,
        "slow query 412ms: SELECT … FROM carts WHERE id = $1",
    ),
    (LogLevel::Info, "GET /v1/cart/9318 200 5.2ms"),
    (LogLevel::Info, "GET /healthz 200 0.4ms"),
];

const CRASH: (LogLevel, &str) = (
    LogLevel::Error,
    "open /etc/checkout/stripe.env: no such file or directory",
);

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

fn line(ix: usize, n: u64, crashing: bool) -> (Option<Stamp>, LogLine) {
    let offset = u64::try_from(ix).unwrap_or_default() * POD_OFFSET_MS;
    let ms = START_MS + n * STEP_MS + offset;
    let message_ix = usize::try_from(n).unwrap_or_default() + ix;
    let (level, message) = if crashing {
        CRASH
    } else {
        MESSAGES
            .get(message_ix % MESSAGES.len())
            .copied()
            .unwrap_or(CRASH)
    };
    let stamp = Stamp(i128::from(ms) * NANOS_PER_MS);
    (Some(stamp), LogLine::new(&clock(ms), level, message))
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
                if source.crashing && n > 0 {
                    return true;
                }
                let (stamp, line) = line(ix, n, source.crashing);
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

    #[test]
    fn demo_lines_from_different_pods_interleave_in_time() {
        let (a, _) = line(0, 1, false);
        let (b, _) = line(1, 1, false);
        let (next, _) = line(0, 2, false);
        assert!(a < b && b < next);
    }
}
