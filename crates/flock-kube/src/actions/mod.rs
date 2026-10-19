pub mod drain;
pub mod exec;
pub mod failure;
pub mod forward;
pub mod logs;
pub mod multi_logs;
pub mod mutate;
pub mod pending;
pub mod rollout;

use std::sync::{Arc, Mutex};

use flock_core::action::Notice;
use flock_core::{
    ClusterId, Command, ExecRequest, Forward, LogEvent, LogRequest, MultiLogRequest, Receiver,
    ResourceRef, Seen, Sender, Snapshot, Status, Update,
};
use pending::Mutation;

use crate::error::{Error, Result};
use crate::manager::{ClusterHandle, lock};
use failure::action_failure;

struct Active {
    forward: Forward,
    handle: ClusterHandle,
    id: u64,
}

impl Active {
    fn stop(&self) {
        self.handle.stop_port_forward(self.id);
    }

    fn alive(&self) -> bool {
        self.handle
            .port_forwards()
            .iter()
            .any(|info| info.id == self.id)
    }
}

#[derive(Default)]
struct Registry {
    forwards: Vec<Active>,
    pending: Vec<(u64, Mutation)>,
    next: u64,
}

#[derive(Clone)]
pub struct Actions {
    registry: Arc<Mutex<Registry>>,
    updates: Sender<Update>,
    changed: Sender<()>,
}

pub fn cluster_of(command: &Command) -> Option<&ClusterId> {
    match command {
        Command::StreamLogs(request) => Some(&request.pod.cluster),
        Command::StreamPodLogs(request) => Some(request.target.cluster()),
        Command::Exec(request) => Some(&request.target.pod.cluster),
        Command::PortForward { service, .. } => Some(&service.cluster),
        Command::Cordon(seen) => Some(&seen.target.cluster),
        Command::Uncordon(seen) => Some(&seen.target.cluster),
        Command::Drain(seen) => Some(&seen.target.cluster),
        Command::Delete(seen) => Some(&seen.target.cluster),
        Command::RestartRollout(seen) => Some(&seen.target.cluster),
        Command::Scale { target, .. } => Some(&target.target.cluster),
        Command::Rollback { target, .. } => Some(&target.target.cluster),
        Command::Flux(key, ..) => Some(&key.cluster),
        _ => None,
    }
}

fn failed_title(verb: &str, target: &ResourceRef) -> String {
    let kind = flock_core::resource::short_kind(&target.kind);
    format!("{verb} {kind}/{} failed", target.name)
}

fn failed_detail(target: &ResourceRef, error: &Error) -> String {
    format!("{} · {}", target.cluster, action_failure(error).line())
}

pub(crate) fn failed_notice(verb: &str, target: &ResourceRef, error: &Error) -> Notice {
    Notice::failed(failed_title(verb, target), failed_detail(target, error))
}

impl Actions {
    pub fn new(updates: Sender<Update>) -> (Self, Receiver<()>) {
        let (changed, signals) = async_channel::unbounded();
        let actions = Self {
            registry: Arc::default(),
            updates,
            changed,
        };
        (actions, signals)
    }

    fn notify(&self, notice: Notice) {
        if self.updates.try_send(Update::Notice(notice)).is_err() {
            tracing::warn!("UI stopped accepting notices");
        }
    }

    fn touch(&self) {
        self.changed.try_send(()).ok();
    }

    pub fn run(&self, command: &Command, handle: Option<ClusterHandle>) {
        match command {
            Command::StopPortForward { local } => self.stop_forward(*local),
            Command::StreamLogs(request) => Self::stream_logs(request, handle),
            Command::StreamPodLogs(request) => Self::stream_pod_logs(request, handle),
            Command::Exec(request) => Self::exec(request, handle),
            Command::PortForward {
                service,
                local,
                remote,
            } => {
                let forward = Forward {
                    target: service.clone(),
                    local: *local,
                    remote: *remote,
                };
                self.start_forward(forward, handle);
            }
            _ => {
                if let Some(mutation) = Mutation::from_command(command) {
                    self.mutate(mutation, handle);
                }
            }
        }
    }

    pub fn copy_spec(&self, mutation: Mutation, handle: Option<ClusterHandle>) {
        self.mutate(mutation, handle);
    }

    fn mutate(&self, mutation: Mutation, handle: Option<ClusterHandle>) {
        let id = {
            let mut registry = lock(&self.registry);
            registry.next += 1;
            let id = registry.next;
            registry.pending.push((id, mutation.clone()));
            id
        };
        self.touch();
        let this = self.clone();
        tokio::spawn(async move {
            let outcome = match handle {
                Some(handle) => {
                    let performed = handle.until_closed(perform(&handle, &mutation)).await;
                    performed.unwrap_or(Err(Error::NotConnected))
                }
                None => Err(Error::NotConnected),
            };
            lock(&this.registry)
                .pending
                .retain(|(pending, _)| *pending != id);
            this.touch();
            let target = mutation.target();
            match outcome {
                Ok(blocked) if blocked.is_empty() => {
                    if let Some(notice) = mutation.done() {
                        this.notify(notice);
                    }
                }
                Ok(blocked) => this.notify(Notice {
                    status: Status::Warn,
                    title: format!("Drain {} incomplete", target.name),
                    detail: Some(format!(
                        "{} · {}",
                        target.cluster,
                        drain::blocked_line(&blocked)
                    )),
                }),
                Err(error) => {
                    this.notify(failed_notice(mutation.verb(), &target, &error));
                }
            }
        });
    }

    fn stream_logs(request: &LogRequest, handle: Option<ClusterHandle>) {
        if let Some(handle) = handle {
            tokio::spawn(logs::stream(handle, request.clone()));
            return;
        }
        let line = logs::marker(&Error::NotConnected.to_string());
        request.sink.send_blocking(line);
    }

    fn stream_pod_logs(request: &MultiLogRequest, handle: Option<ClusterHandle>) {
        if let Some(handle) = handle {
            tokio::spawn(multi_logs::stream(handle, request.clone()));
            return;
        }
        let reason = action_failure(&Error::NotConnected).reason;
        request.sink.send_blocking(LogEvent::Failed(reason));
    }

    fn exec(request: &ExecRequest, handle: Option<ClusterHandle>) {
        if let Some(handle) = handle {
            tokio::spawn(exec::run(handle, request.clone()));
            return;
        }
        exec::fail(&request.output, &Error::NotConnected.to_string());
    }

    fn start_forward(&self, forward: Forward, handle: Option<ClusterHandle>) {
        let this = self.clone();
        tokio::spawn(async move {
            let started = match handle {
                Some(handle) => this.forward(forward.clone(), handle).await,
                None => Err(Error::NotConnected),
            };
            if let Err(error) = started {
                let title = failed_title("Port-forward", &forward.target);
                this.notify(Notice::failed(
                    title,
                    failed_detail(&forward.target, &error),
                ));
            }
        });
    }

    async fn forward(&self, forward: Forward, handle: ClusterHandle) -> Result<()> {
        let info = forward::start(&handle, &forward.target, forward.local, forward.remote).await?;
        let active = Active {
            forward: Forward {
                local: info.local_addr.port(),
                ..forward
            },
            handle,
            id: info.id,
        };
        lock(&self.registry).forwards.push(active);
        self.touch();
        Ok(())
    }

    fn stop_forward(&self, local: u16) {
        let mut registry = lock(&self.registry);
        registry.forwards.retain(|active| {
            let keep = active.forward.local != local;
            if !keep {
                active.stop();
            }
            keep
        });
        drop(registry);
        self.touch();
    }

    pub fn overlay(&self, snapshot: &mut Snapshot) {
        let mut registry = lock(&self.registry);
        registry.forwards.retain(|active| {
            let keep = !active.handle.is_closed() && active.alive();
            if !keep {
                active.stop();
            }
            keep
        });
        snapshot.forwards = registry
            .forwards
            .iter()
            .map(|active| active.forward.clone())
            .collect();
        for (_, mutation) in &registry.pending {
            pending::mark(snapshot, mutation);
        }
    }
}

async fn perform(handle: &ClusterHandle, mutation: &Mutation) -> Result<Vec<drain::Blocked>> {
    match mutation {
        Mutation::Cordon(seen) => mutate::cordon(handle, seen, true).await?,
        Mutation::Uncordon(seen) => mutate::cordon(handle, seen, false).await?,
        Mutation::Drain(seen) => {
            mutate::cordon(handle, seen, true).await?;
            return drain::drain(handle, &seen.target.name).await;
        }
        Mutation::Delete(seen) => mutate::delete(handle, seen).await?,
        Mutation::Restart(seen) => mutate::restart(handle, seen).await?,
        Mutation::Scale(seen, replicas) => mutate::scale(handle, seen, *replicas).await?,
        Mutation::Rollback(seen, revision) => {
            let target = &seen.target;
            let namespace = target
                .namespace
                .as_deref()
                .ok_or(Error::MissingField("metadata.namespace"))?;
            if target.kind == rollout::DEPLOYMENT {
                rollout::rollback(handle, namespace, &target.name, &seen.uid, *revision).await?;
            } else {
                rollout::rollback_controller(
                    handle,
                    &target.kind,
                    namespace,
                    &target.name,
                    &seen.uid,
                    *revision,
                )
                .await?;
            }
        }
        Mutation::Flux(key, uid, action) => mutate::flux(handle, key, uid, *action).await?,
        Mutation::CopySpec {
            target,
            uid,
            template,
            ..
        } => {
            let seen = Seen::new(target.clone(), uid.clone());
            mutate::replace_template(handle, &seen, template).await?;
        }
    }
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use flock_core::ClusterSummary;

    use super::*;

    #[tokio::test]
    async fn a_disconnected_cluster_keeps_no_forward_even_while_its_row_is_listed() {
        let (client, _) = crate::fake::client(std::time::Duration::ZERO, |_, _| None);
        let (manager, handle) = crate::fake::handle(&client);
        let Ok(info) = handle.port_forward("apps", "web-1", 80, 0).await else {
            panic!("forward starts");
        };
        let (updates, _) = async_channel::unbounded();
        let (actions, _) = Actions::new(updates);
        let cluster = ClusterId::new("prod");
        let target = ResourceRef {
            cluster: cluster.clone(),
            kind: "Service".into(),
            namespace: Some("apps".into()),
            name: "web".into(),
        };
        let forward = Forward {
            target,
            local: info.local_addr.port(),
            remote: 80,
        };
        lock(&actions.registry).forwards.push(Active {
            forward,
            handle: handle.clone(),
            id: info.id,
        });
        let mut snapshot = Snapshot {
            clusters: vec![ClusterSummary {
                id: cluster,
                name: "prod".into(),
                status: Status::Offline,
                state: "Disconnected".into(),
                distro: String::new(),
                region: String::new(),
                version: None,
                nodes: None,
                problems: flock_core::Counts::default(),
                nav: std::collections::BTreeMap::new(),
                metrics: None,
            }],
            ..Snapshot::default()
        };
        actions.overlay(&mut snapshot);
        assert_eq!(snapshot.forwards.len(), 1);

        manager.disconnect(handle.id());
        actions.overlay(&mut snapshot);

        assert_eq!(snapshot.forwards, Vec::new());
        assert!(lock(&actions.registry).forwards.is_empty());
    }
}
