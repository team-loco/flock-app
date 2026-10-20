mod data;
mod exec;
#[cfg(feature = "loco")]
mod loco;
mod logs;
mod sim;

use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use flock_core::{
    ClusterId, Command, ExecRequest, LogRequest, ResourceRef, Sink, Snapshot, Status, Workload,
};

use sim::{Action, Names};

use data::{checkout_api, healthy_checkout};

const ACTION_DURATION: Duration = Duration::from_secs(2);
const LOG_TICK: Duration = Duration::from_millis(700);

#[derive(Default)]
struct Shared {
    workloads: Vec<Workload>,
    watchers: Vec<(ResourceRef, Sink<Workload>)>,
    names: Names,
}

impl Shared {
    fn publish(&mut self, target: &ResourceRef) {
        let Some(workload) = self.workloads.iter().find(|w| &w.target == target) else {
            return;
        };
        self.watchers
            .retain(|(watched, sink)| watched != target || sink.send_blocking(workload.clone()));
    }

    fn pod_status(&self, pod: &ResourceRef) -> Option<Status> {
        self.workloads
            .iter()
            .filter(|w| w.target.cluster == pod.cluster && w.target.namespace == pod.namespace)
            .flat_map(|w| w.pods.iter())
            .find(|p| p.name == pod.name)
            .map(|p| p.status)
    }
}

#[derive(Clone)]
pub struct Workloads {
    shared: Arc<Mutex<Shared>>,
}

impl Workloads {
    pub fn new(snapshot: &Snapshot) -> Self {
        let sample = data::target().cluster;
        let peers: Vec<Workload> = snapshot
            .clusters
            .iter()
            .filter(|c| c.id != sample)
            .filter_map(|c| peer_checkout(snapshot, &c.id))
            .collect();
        let shared = Shared {
            workloads: sample_workloads().into_iter().chain(peers).collect(),
            ..Shared::default()
        };
        Self {
            shared: Arc::new(Mutex::new(shared)),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn knows_pod(&self, pod: &ResourceRef) -> bool {
        self.lock().pod_status(pod).is_some()
    }

    pub fn handle(&self, command: &Command) {
        match command {
            Command::WatchWorkload { target, sink } => self.watch(target, sink),
            Command::StreamLogs(request) => self.follow_logs(request),
            Command::Exec(request) => spawn_exec(request.clone()),
            Command::RestartRollout(seen) => self.act(&seen.target, Action::Restart),
            Command::Scale { target, replicas } => {
                self.act(&target.target, Action::Scale(*replicas));
            }
            Command::Rollback { target, revision } => {
                self.act(&target.target, Action::Rollback(*revision));
            }
            _ => {}
        }
    }

    fn watch(&self, target: &ResourceRef, sink: &Sink<Workload>) {
        let mut shared = self.lock();
        if !shared.workloads.iter().any(|w| &w.target == target) {
            return;
        }
        shared.watchers.push((target.clone(), sink.clone()));
        shared.publish(target);
    }

    fn act(&self, target: &ResourceRef, action: Action) {
        {
            let mut shared = self.lock();
            let Some(workload) = shared.workloads.iter_mut().find(|w| &w.target == target) else {
                return;
            };
            sim::begin(workload, action);
            shared.publish(target);
        }
        let this = self.clone();
        let target = target.clone();
        spawn("flock-demo-action", move || {
            thread::sleep(ACTION_DURATION);
            let mut shared = this.lock();
            let Shared {
                workloads, names, ..
            } = &mut *shared;
            if let Some(workload) = workloads.iter_mut().find(|w| w.target == target) {
                sim::finish(workload, action, names);
            }
            shared.publish(&target);
        });
    }

    fn follow_logs(&self, request: &LogRequest) {
        let status = self.lock().pod_status(&request.pod).unwrap_or(Status::Ok);
        let script = logs::script(status, &request.container, request.instance);
        let request = request.clone();
        spawn("flock-demo-logs", move || {
            for line in script.initial {
                if !request.sink.send_blocking(line) {
                    return;
                }
            }
            if !script.streams {
                return;
            }
            let mut ticker = logs::Ticker::new(&request.container);
            loop {
                thread::sleep(LOG_TICK);
                if !request.sink.send_blocking(ticker.next_line()) {
                    return;
                }
            }
        });
    }
}

fn sample_workloads() -> Vec<Workload> {
    let workloads = vec![checkout_api()];
    #[cfg(feature = "loco")]
    let workloads = [workloads, vec![loco::checkout()]].concat();
    workloads
}

fn peer_checkout(snapshot: &Snapshot, cluster: &ClusterId) -> Option<Workload> {
    let checkout = crate::derive::checkout(&snapshot.resources, cluster)?;
    let pods: Vec<(String, String)> = snapshot
        .pods
        .iter()
        .filter(|p| &p.cluster == cluster && p.namespace == data::NAMESPACE)
        .filter(|p| p.name.starts_with(crate::derive::CHECKOUT_REPLICA_SET))
        .map(|p| (p.name.clone(), p.node.clone().unwrap_or_default()))
        .collect();
    if pods.is_empty() {
        return None;
    }
    Some(healthy_checkout(
        cluster,
        &pods,
        checkout.version.as_deref(),
    ))
}

fn spawn_exec(request: ExecRequest) {
    spawn("flock-demo-exec", move || exec::run(&request));
}

fn spawn(name: &str, work: impl FnOnce() + Send + 'static) {
    if let Err(error) = thread::Builder::new().name(name.into()).spawn(work) {
        tracing::error!(%error, "demo worker failed to start");
    }
}

#[cfg(test)]
mod tests {
    use flock_core::{ExecEvent, ExecTarget, LogLine, Seen, Source, Uid};

    use super::data::target as sample_target;
    use super::*;

    fn pod(name: &str) -> ResourceRef {
        ResourceRef {
            kind: "Pod".into(),
            name: name.into(),
            ..sample_target()
        }
    }

    #[test]
    fn watching_sends_the_current_state_then_action_progress() {
        let workloads = Workloads::new(&crate::snapshot());
        let (sink, rx) = Sink::channel();
        workloads.handle(&Command::WatchWorkload {
            target: sample_target(),
            sink,
        });
        assert_eq!(rx.recv_blocking().ok(), Some(checkout_api()));
        workloads.handle(&Command::Rollback {
            target: Seen::new(sample_target(), Uid::default()),
            revision: 36,
        });
        let started = rx.recv_blocking().ok();
        assert_eq!(started.map(|w| w.status()), Some(Status::Progress));
        let done = rx.recv_blocking().ok();
        assert_eq!(done.map(|w| w.status()), Some(Status::Ok));
    }

    #[cfg(feature = "loco")]
    #[test]
    fn the_loco_checkout_deployment_has_a_workload_to_open() {
        let workloads = Workloads::new(&crate::snapshot());
        let (sink, rx) = Sink::channel();
        workloads.handle(&Command::WatchWorkload {
            target: loco::target(),
            sink,
        });
        let workload = rx.recv_blocking().ok();
        assert_eq!(workload.map(|w| w.target), Some(loco::target()));
    }

    #[test]
    fn peer_clusters_serve_a_healthy_checkout_matching_their_pods() {
        let snapshot = crate::snapshot();
        let workloads = Workloads::new(&snapshot);
        let (sink, rx) = Sink::channel();
        let target = ResourceRef {
            cluster: ClusterId::new("prod-eu-west"),
            ..sample_target()
        };
        workloads.handle(&Command::WatchWorkload {
            target: target.clone(),
            sink,
        });
        let workload = rx.recv_blocking().ok();
        assert_eq!(workload.as_ref().map(Workload::status), Some(Status::Ok));
        assert_eq!(workload.as_ref().map(|w| w.desired), Some(3));
        assert_eq!(
            workload.as_ref().and_then(Workload::image),
            Some("checkout-api:2.13.4")
        );
        let pod = workload.and_then(|w| w.pods.first().map(|p| p.name.clone()));
        let listed = snapshot
            .pods
            .iter()
            .any(|p| p.cluster == target.cluster && Some(&p.name) == pod.as_ref());
        assert!(listed);
    }

    #[test]
    fn watching_an_unknown_workload_closes_the_sink() {
        let workloads = Workloads::new(&crate::snapshot());
        let (sink, rx) = Sink::channel();
        let target = ResourceRef {
            name: "missing".into(),
            ..sample_target()
        };
        workloads.handle(&Command::WatchWorkload { target, sink });
        assert!(rx.recv_blocking().is_err());
    }

    #[test]
    fn crashing_pod_logs_explain_waiting() {
        let workloads = Workloads::new(&crate::snapshot());
        let (sink, rx) = Sink::channel();
        workloads.handle(&Command::StreamLogs(LogRequest {
            pod: pod("checkout-api-7f9c6d-x2lqp"),
            container: data::API.into(),
            instance: flock_core::LogInstance::Current,
            sink,
        }));
        let lines: Vec<LogLine> = std::iter::from_fn(|| rx.recv_blocking().ok()).collect();
        assert_eq!(lines, logs::crash_waiting());
    }

    #[test]
    fn healthy_pod_logs_keep_streaming_until_dropped() {
        let workloads = Workloads::new(&crate::snapshot());
        let (sink, rx) = Sink::channel();
        workloads.handle(&Command::StreamLogs(LogRequest {
            pod: pod("checkout-api-7f9c6d-m8vtn"),
            container: data::API.into(),
            instance: flock_core::LogInstance::Current,
            sink,
        }));
        let initial = logs::healthy_api().len();
        let received: Vec<LogLine> = (0..=initial)
            .filter_map(|_| rx.recv_blocking().ok())
            .collect();
        assert_eq!(received.len(), initial + 1);
    }

    #[test]
    fn exec_session_replays_history_and_answers_input() {
        let (output, events) = Sink::channel();
        let (input, source) = Source::channel();
        Workloads::new(&crate::snapshot()).handle(&Command::Exec(ExecRequest {
            target: ExecTarget {
                pod: pod("checkout-api-7f9c6d-x2lqp"),
                container: data::API.into(),
                debug_image: Some("busybox:1.36".into()),
            },
            input: source,
            resize: Source::channel().1,
            output,
        }));
        assert!(matches!(
            events.recv_blocking(),
            Ok(ExecEvent::Started { .. })
        ));
        assert!(matches!(events.recv_blocking(), Ok(ExecEvent::Output(_))));
        assert!(input.send_blocking(b"exit\r".to_vec()).is_ok());
        let rest: Vec<ExecEvent> = std::iter::from_fn(|| events.recv_blocking().ok()).collect();
        assert_eq!(rest.last(), Some(&ExecEvent::Exited { code: Some(0) }));
    }
}
