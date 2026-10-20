use std::sync::Arc;

use flock_core::pod_tops::{PendingPod, PodTops, RestartingPod};
use flock_core::{ClusterId, ResourceRef};
use jiff::Timestamp;
use k8s_openapi::api::core::v1::{ContainerStatus, Pod};

use crate::clock::age;
use crate::pods::unschedulable;

const POD_KIND: &str = "Pod";
const PHASE_PENDING: &str = "Pending";
const MEMORY: &str = "memory";

fn target(cluster: &ClusterId, pod: &Pod) -> ResourceRef {
    ResourceRef {
        cluster: cluster.clone(),
        kind: POD_KIND.to_owned(),
        namespace: pod.metadata.namespace.clone(),
        name: pod.metadata.name.clone().unwrap_or_default(),
    }
}

fn memory_limit(pod: &Pod, container: &str) -> Option<String> {
    pod.spec
        .as_ref()?
        .containers
        .iter()
        .find(|c| c.name == container)?
        .resources
        .as_ref()?
        .limits
        .as_ref()?
        .get(MEMORY)
        .map(|q| q.0.clone())
}

fn restarting(cluster: &ClusterId, pod: &Pod) -> Option<RestartingPod> {
    let status: &ContainerStatus = pod
        .status
        .as_ref()?
        .container_statuses
        .iter()
        .flatten()
        .filter(|c| c.restart_count > 0)
        .max_by_key(|c| c.restart_count)?;
    let last = status
        .last_state
        .as_ref()
        .and_then(|state| state.terminated.as_ref());
    Some(RestartingPod {
        target: target(cluster, pod),
        container: status.name.clone(),
        reason: last.and_then(|t| t.reason.clone()),
        exit_code: last.map(|t| t.exit_code),
        memory_limit: memory_limit(pod, &status.name),
        restarts: u32::try_from(status.restart_count).unwrap_or_default(),
    })
}

fn waiting_reason(pod: &Pod) -> Option<String> {
    pod.status
        .as_ref()?
        .container_statuses
        .iter()
        .flatten()
        .find_map(|c| c.state.as_ref()?.waiting.as_ref()?.reason.clone())
}

fn pending(cluster: &ClusterId, pod: &Pod, now: Timestamp) -> Option<PendingPod> {
    let phase = pod.status.as_ref().and_then(|s| s.phase.as_deref());
    if phase != Some(PHASE_PENDING) {
        return None;
    }
    let message = unschedulable(pod)
        .filter(|message| !message.is_empty())
        .or_else(|| waiting_reason(pod))
        .unwrap_or_else(|| PHASE_PENDING.to_owned());
    Some(PendingPod {
        target: target(cluster, pod),
        message,
        pending: age(&pod.metadata, now),
    })
}

pub fn pod_tops(cluster: &ClusterId, pods: &[Arc<Pod>], now: Timestamp) -> PodTops {
    let restarts = pods.iter().filter_map(|p| restarting(cluster, p)).collect();
    let pending = pods
        .iter()
        .filter_map(|p| pending(cluster, p, now))
        .collect();
    PodTops::new(restarts, pending)
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::core::v1::{
        Container, ContainerState, ContainerStateTerminated, ContainerStateWaiting, PodCondition,
        PodSpec, PodStatus, ResourceRequirements,
    };
    use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, Time};
    use std::collections::BTreeMap;

    fn at(second: i64) -> crate::Result<Timestamp> {
        Timestamp::from_second(second).map_err(|_| crate::Error::MetricsHistory("timestamp"))
    }

    fn pod(name: &str, phase: &str) -> Pod {
        Pod {
            metadata: ObjectMeta {
                name: Some(name.into()),
                namespace: Some("search".into()),
                creation_timestamp: Some(Time(Timestamp::UNIX_EPOCH)),
                ..ObjectMeta::default()
            },
            spec: Some(PodSpec::default()),
            status: Some(PodStatus {
                phase: Some(phase.into()),
                ..PodStatus::default()
            }),
        }
    }

    fn oom(name: &str, restarts: i32) -> Pod {
        let mut pod = pod(name, "Running");
        let limits = BTreeMap::from([(MEMORY.to_owned(), Quantity("4Gi".into()))]);
        pod.spec = Some(PodSpec {
            containers: vec![Container {
                name: "indexer".into(),
                resources: Some(ResourceRequirements {
                    limits: Some(limits),
                    ..ResourceRequirements::default()
                }),
                ..Container::default()
            }],
            ..PodSpec::default()
        });
        if let Some(status) = pod.status.as_mut() {
            status.container_statuses = Some(vec![ContainerStatus {
                name: "indexer".into(),
                restart_count: restarts,
                last_state: Some(ContainerState {
                    terminated: Some(ContainerStateTerminated {
                        reason: Some("OOMKilled".into()),
                        exit_code: 137,
                        ..ContainerStateTerminated::default()
                    }),
                    ..ContainerState::default()
                }),
                ..ContainerStatus::default()
            }]);
        }
        pod
    }

    #[test]
    fn restarting_pods_carry_their_last_termination_and_memory_limit() {
        let cluster = ClusterId::new("prod");
        let pods = [
            Arc::new(oom("a", 3)),
            Arc::new(oom("b", 41)),
            Arc::new(pod("c", "Running")),
        ];
        let tops = pod_tops(&cluster, &pods, Timestamp::UNIX_EPOCH);
        assert_eq!(tops.restarted, 2);
        let first = tops.restarts.first();
        assert_eq!(first.map(|r| r.target.name.as_str()), Some("b"));
        assert_eq!(
            first.map(RestartingPod::detail).as_deref(),
            Some("indexer · OOMKilled · exit 137 · limit 4Gi")
        );
    }

    #[test]
    fn pending_pods_explain_themselves_from_scheduling_or_waiting() -> crate::Result<()> {
        let cluster = ClusterId::new("prod");
        let mut unschedulable = pod("trainer", PHASE_PENDING);
        if let Some(status) = unschedulable.status.as_mut() {
            status.conditions = Some(vec![PodCondition {
                type_: "PodScheduled".into(),
                status: "False".into(),
                message: Some("0/3 nodes are available: 3 Insufficient cpu.".into()),
                ..PodCondition::default()
            }]);
        }
        let mut creating = pod("web", PHASE_PENDING);
        creating.metadata.creation_timestamp = Some(Time(at(600)?));
        if let Some(status) = creating.status.as_mut() {
            status.container_statuses = Some(vec![ContainerStatus {
                state: Some(ContainerState {
                    waiting: Some(ContainerStateWaiting {
                        reason: Some("ContainerCreating".into()),
                        ..ContainerStateWaiting::default()
                    }),
                    ..ContainerState::default()
                }),
                ..ContainerStatus::default()
            }]);
        }
        let pods = [Arc::new(creating), Arc::new(unschedulable)];
        let now = at(3600)?;
        let tops = pod_tops(&cluster, &pods, now);
        let messages: Vec<_> = tops.pending.iter().map(|p| p.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "0/3 nodes are available: 3 Insufficient cpu.",
                "ContainerCreating"
            ]
        );
        assert_eq!(
            tops.pending.first().map(PendingPod::duration).as_deref(),
            Some("1h")
        );
        Ok(())
    }
}
