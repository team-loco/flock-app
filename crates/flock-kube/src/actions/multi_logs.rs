use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use flock_core::multilog::PodFacts;
use flock_core::{LogEvent, LogSource, LogTarget, MultiLogRequest, Sink, SourceId, Stamp};
use jiff::tz::TimeZone;
use k8s_openapi::api::core::v1::Pod;
use kube::ResourceExt;
use tokio::sync::Semaphore;

use super::failure::action_failure;
use super::logs::{TAIL_LINES, exit_marker, marker, parse_stamped};
use crate::error::{Error, Result};
use crate::manager::ClusterHandle;
use crate::ops;
use crate::problems::owner_of;
use crate::resources::WatchKind;
use crate::task::TaskGuard;

pub const MAX_STREAMS: usize = 24;
const MEMBERSHIP_POLL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Started {
    pub restarts: i32,
}

pub fn matching<'a>(target: &LogTarget, pods: &'a [Arc<Pod>]) -> Vec<&'a Pod> {
    let empty = BTreeMap::new();
    pods.iter()
        .map(AsRef::as_ref)
        .filter(|pod| {
            let owner = owner_of(pod);
            let facts = PodFacts {
                namespace: pod.metadata.namespace.as_deref().unwrap_or_default(),
                name: pod.metadata.name.as_deref().unwrap_or_default(),
                owner_kind: Some(owner.kind.as_str()),
                owner_name: Some(owner.name.as_str()),
                labels: pod.metadata.labels.as_ref().unwrap_or(&empty),
            };
            target.matches(&facts)
        })
        .collect()
}

pub fn started(pods: &[&Pod]) -> BTreeMap<LogSource, Started> {
    let mut sources = BTreeMap::new();
    for pod in pods {
        let namespace = pod.namespace().unwrap_or_default();
        let name = pod.name_any();
        let statuses = pod
            .status
            .as_ref()
            .and_then(|status| status.container_statuses.as_ref())
            .into_iter()
            .flatten();
        for status in statuses {
            let state = status.state.as_ref();
            let running = state.is_some_and(|s| s.running.is_some() || s.terminated.is_some());
            if !running {
                continue;
            }
            let source = LogSource {
                namespace: namespace.clone(),
                pod: name.clone(),
                container: status.name.clone(),
            };
            sources.insert(
                source,
                Started {
                    restarts: status.restart_count,
                },
            );
        }
    }
    sources
}

pub fn present(pods: &[&Pod]) -> BTreeSet<(String, String)> {
    pods.iter()
        .map(|pod| (pod.namespace().unwrap_or_default(), pod.name_any()))
        .collect()
}

struct Running {
    restarts: i32,
    task: TaskGuard,
}

struct Fanout {
    handle: ClusterHandle,
    sink: Sink<LogEvent>,
    limit: Arc<Semaphore>,
    ids: BTreeMap<LogSource, SourceId>,
    running: BTreeMap<SourceId, Running>,
    next: u32,
}

impl Fanout {
    fn new(handle: ClusterHandle, sink: Sink<LogEvent>) -> Self {
        Self {
            handle,
            sink,
            limit: Arc::new(Semaphore::new(MAX_STREAMS)),
            ids: BTreeMap::new(),
            running: BTreeMap::new(),
            next: 0,
        }
    }

    fn send(&self, event: LogEvent) -> bool {
        self.sink.send_blocking(event)
    }

    fn sync(&mut self, target: &LogTarget) -> bool {
        let pods = self.handle.objects::<Pod>();
        let matched = matching(target, &pods);
        let here = present(&matched);
        let wanted = started(&matched);
        let gone: Vec<(LogSource, SourceId)> = self
            .ids
            .iter()
            .filter(|(source, _)| !here.contains(&(source.namespace.clone(), source.pod.clone())))
            .map(|(source, id)| (source.clone(), *id))
            .collect();
        for (source, id) in gone {
            self.ids.remove(&source);
            self.running.remove(&id);
            if !self.send(LogEvent::Left(id)) {
                return false;
            }
        }
        for (source, started) in wanted {
            let Some(id) = self.join(&source) else {
                return false;
            };
            let current = self.running.get(&id);
            let keep = current.is_some_and(|running| {
                !running.task.is_finished() || running.restarts == started.restarts
            });
            if keep {
                continue;
            }
            let task = self.pump(id, source);
            self.running.insert(
                id,
                Running {
                    restarts: started.restarts,
                    task,
                },
            );
        }
        true
    }

    fn join(&mut self, source: &LogSource) -> Option<SourceId> {
        if let Some(id) = self.ids.get(source) {
            return Some(*id);
        }
        let id = SourceId(self.next);
        self.next += 1;
        self.ids.insert(source.clone(), id);
        self.send(LogEvent::Joined(id, source.clone()))
            .then_some(id)
    }

    fn pump(&self, id: SourceId, source: LogSource) -> TaskGuard {
        let handle = self.handle.clone();
        let sink = self.sink.clone();
        let limit = self.limit.clone();
        TaskGuard::spawn(async move {
            let Ok(_permit) = limit.acquire_owned().await else {
                return;
            };
            let outcome = follow(&handle, id, &source, &sink).await;
            if let Err(error) = outcome {
                let reason = action_failure(&error).reason;
                sink.send_blocking(LogEvent::Line(id, None, marker(&reason)));
            }
            sink.send_blocking(LogEvent::Ended(id));
        })
    }
}

async fn follow(
    handle: &ClusterHandle,
    id: SourceId,
    source: &LogSource,
    sink: &Sink<LogEvent>,
) -> Result<()> {
    let params = ops::LogRequest {
        namespace: source.namespace.clone(),
        pod: source.pod.clone(),
        container: Some(source.container.clone()),
        follow: true,
        tail_lines: Some(TAIL_LINES),
        timestamps: true,
        since_seconds: None,
    };
    let stream = handle.logs(&params).await?;
    let zone = TimeZone::system();
    while let Ok(raw) = stream.lines.recv().await {
        let (at, line) = parse_stamped(&raw?, &zone);
        let stamp = at.map(|at| Stamp(at.as_nanosecond()));
        if !sink.send_blocking(LogEvent::Line(id, stamp, line)) {
            return Ok(());
        }
    }
    let pods = handle.objects::<Pod>();
    let pod = pods.iter().find(|pod| {
        pod.name_any() == source.pod && pod.namespace().as_deref() == Some(&source.namespace)
    });
    if let Some(line) = pod.and_then(|pod| exit_marker(pod, &source.container)) {
        sink.send_blocking(LogEvent::Line(id, None, line));
    }
    Ok(())
}

async fn run(handle: &ClusterHandle, request: &MultiLogRequest) {
    let _pods = handle.subscribe(WatchKind::Pods);
    let mut fanout = Fanout::new(handle.clone(), request.sink.clone());
    let mut tick = tokio::time::interval(MEMBERSHIP_POLL);
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            () = request.sink.closed() => return,
        }
        if !fanout.sync(&request.target) {
            return;
        }
    }
}

pub async fn stream(handle: ClusterHandle, request: MultiLogRequest) {
    if handle.until_closed(run(&handle, &request)).await.is_none() {
        let reason = action_failure(&Error::NotConnected).reason;
        request.sink.send_blocking(LogEvent::Failed(reason));
    }
}

#[cfg(test)]
mod tests {
    use flock_core::{ClusterId, ResourceRef};
    use serde_json::json;

    use super::*;

    fn pod(name: &str, app: &str, running: bool, restarts: i32) -> Arc<Pod> {
        let state = if running {
            json!({ "running": { "startedAt": "2026-10-14T10:00:00Z" } })
        } else {
            json!({ "waiting": { "reason": "ContainerCreating" } })
        };
        let value = json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {
                "name": name,
                "namespace": "web",
                "labels": { "app": app, "pod-template-hash": "7f9c6d" },
                "ownerReferences": [{
                    "apiVersion": "apps/v1",
                    "kind": "ReplicaSet",
                    "name": format!("{app}-7f9c6d"),
                    "uid": "u",
                    "controller": true
                }]
            },
            "status": {
                "containerStatuses": [
                    { "name": "api", "ready": running, "restartCount": restarts, "image": "i", "imageID": "", "state": state },
                    { "name": "envoy", "ready": true, "restartCount": 0, "image": "i", "imageID": "", "state": { "running": {} } }
                ]
            }
        });
        let Ok(pod) = serde_json::from_value(value) else {
            panic!("pod fixture");
        };
        Arc::new(pod)
    }

    fn checkout() -> LogTarget {
        LogTarget::Workload(ResourceRef {
            cluster: ClusterId::new("kind-fleet-a"),
            kind: "Deployment".into(),
            namespace: Some("web".into()),
            name: "checkout".into(),
        })
    }

    #[test]
    fn a_workload_target_streams_every_started_container_of_its_pods() {
        let pods = vec![
            pod("checkout-7f9c6d-a", "checkout", true, 0),
            pod("checkout-7f9c6d-b", "checkout", false, 0),
            pod("cart-7f9c6d-c", "cart", true, 0),
        ];
        let matched = matching(&checkout(), &pods);
        assert_eq!(
            present(&matched),
            BTreeSet::from([
                ("web".to_owned(), "checkout-7f9c6d-a".to_owned()),
                ("web".to_owned(), "checkout-7f9c6d-b".to_owned()),
            ])
        );
        let sources: Vec<(String, String)> = started(&matched)
            .into_keys()
            .map(|source| (source.pod, source.container))
            .collect();
        assert_eq!(
            sources,
            [
                ("checkout-7f9c6d-a".to_owned(), "api".to_owned()),
                ("checkout-7f9c6d-a".to_owned(), "envoy".to_owned()),
                ("checkout-7f9c6d-b".to_owned(), "envoy".to_owned()),
            ]
        );
    }

    #[test]
    fn a_job_target_streams_the_pods_the_job_owns() {
        let value = json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {
                "name": "migrate-42-x7k2p",
                "namespace": "web",
                "ownerReferences": [{
                    "apiVersion": "batch/v1",
                    "kind": "Job",
                    "name": "migrate-42",
                    "uid": "u",
                    "controller": true
                }]
            }
        });
        let Ok(job_pod) = serde_json::from_value::<Pod>(value) else {
            panic!("pod fixture");
        };
        let pods = vec![
            Arc::new(job_pod),
            pod("checkout-7f9c6d-a", "checkout", true, 0),
        ];
        let target = LogTarget::Workload(ResourceRef {
            cluster: ClusterId::new("kind-fleet-a"),
            kind: "Job".into(),
            namespace: Some("web".into()),
            name: "migrate-42".into(),
        });
        let names: Vec<String> = matching(&target, &pods)
            .into_iter()
            .map(ResourceExt::name_any)
            .collect();
        assert_eq!(names, ["migrate-42-x7k2p"]);
    }

    #[test]
    fn restarts_are_reported_per_container() {
        let pods = vec![pod("checkout-7f9c6d-a", "checkout", true, 3)];
        let matched = matching(&checkout(), &pods);
        let restarts: Vec<i32> = started(&matched)
            .into_values()
            .map(|started| started.restarts)
            .collect();
        assert_eq!(restarts, [3, 0]);
    }

    #[test]
    fn a_selector_target_matches_labels_in_its_namespace() {
        let Ok(selector) = flock_core::multilog::LabelSelector::parse("app=cart") else {
            panic!("selector parses");
        };
        let target = LogTarget::Selector {
            cluster: ClusterId::new("kind-fleet-a"),
            namespace: Some("web".into()),
            selector,
        };
        let pods = vec![
            pod("checkout-7f9c6d-a", "checkout", true, 0),
            pod("cart-7f9c6d-c", "cart", true, 0),
        ];
        let names: Vec<String> = matching(&target, &pods)
            .into_iter()
            .map(ResourceExt::name_any)
            .collect();
        assert_eq!(names, ["cart-7f9c6d-c"]);
    }
}
