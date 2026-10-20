use std::cmp::Ordering;
use std::time::Duration;

use crate::pods::{Pod, PodPhase};
use crate::{ResourceRef, Status, time};

pub const TOP_PODS: usize = 5;
const RESTARTS_HIGH: u32 = 10;
const RESTARTS_SOME: u32 = 1;
const PENDING_LONG: Duration = Duration::from_mins(30);
const FAILED_SCHEDULING: &str = "FailedScheduling";
const SEPARATOR: &str = " · ";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestartingPod {
    pub target: ResourceRef,
    pub container: String,
    pub reason: Option<String>,
    pub exit_code: Option<i32>,
    pub memory_limit: Option<String>,
    pub restarts: u32,
}

impl RestartingPod {
    pub fn of(pod: &Pod) -> Option<Self> {
        let container = pod
            .containers
            .iter()
            .filter(|c| c.restarts > 0)
            .max_by_key(|c| c.restarts)?;
        let last = container.last_termination.as_ref();
        Some(Self {
            target: pod.resource(),
            container: container.name.clone(),
            reason: last.map(|t| t.reason.clone()),
            exit_code: last.map(|t| t.exit_code),
            memory_limit: container.limits.memory.clone(),
            restarts: container.restarts,
        })
    }

    pub fn detail(&self) -> String {
        let reason = self.reason.clone().filter(|reason| !reason.is_empty());
        let exit = self.exit_code.map(|code| format!("exit {code}"));
        let limit = self
            .memory_limit
            .as_ref()
            .map(|limit| format!("limit {limit}"));
        std::iter::once(self.container.clone())
            .chain(reason)
            .chain(exit)
            .chain(limit)
            .collect::<Vec<_>>()
            .join(SEPARATOR)
    }

    pub const fn status(&self) -> Option<Status> {
        if self.restarts > RESTARTS_HIGH {
            Some(Status::Err)
        } else if self.restarts > RESTARTS_SOME {
            Some(Status::Warn)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingPod {
    pub target: ResourceRef,
    pub message: String,
    pub pending: Duration,
}

impl PendingPod {
    pub fn of(pod: &Pod) -> Option<Self> {
        if pod.phase != PodPhase::Pending {
            return None;
        }
        let scheduling = pod
            .events
            .iter()
            .filter(|event| event.reason == FAILED_SCHEDULING)
            .min_by_key(|event| event.age)
            .map(|event| event.message.clone());
        Some(Self {
            target: pod.resource(),
            message: scheduling.unwrap_or_else(|| pod.reason.clone()),
            pending: pod.age,
        })
    }

    pub fn duration(&self) -> String {
        time::span(self.pending)
    }

    pub fn status(&self) -> Option<Status> {
        (self.pending >= PENDING_LONG).then_some(Status::Warn)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PodTops {
    pub restarts: Vec<RestartingPod>,
    pub restarted: usize,
    pub pending: Vec<PendingPod>,
}

fn by_name(a: &ResourceRef, b: &ResourceRef) -> Ordering {
    a.namespace
        .cmp(&b.namespace)
        .then_with(|| a.name.cmp(&b.name))
}

impl PodTops {
    pub fn new(mut restarts: Vec<RestartingPod>, mut pending: Vec<PendingPod>) -> Self {
        let restarted = restarts.len();
        restarts.sort_by(|a, b| {
            b.restarts
                .cmp(&a.restarts)
                .then_with(|| by_name(&a.target, &b.target))
        });
        restarts.truncate(TOP_PODS);
        pending.sort_by(|a, b| {
            b.pending
                .cmp(&a.pending)
                .then_with(|| by_name(&a.target, &b.target))
        });
        pending.truncate(TOP_PODS);
        Self {
            restarts,
            restarted,
            pending,
        }
    }

    pub fn of<'a>(pods: impl IntoIterator<Item = &'a Pod> + Clone) -> Self {
        let restarts = pods
            .clone()
            .into_iter()
            .filter_map(RestartingPod::of)
            .collect();
        let pending = pods.into_iter().filter_map(PendingPod::of).collect();
        Self::new(restarts, pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClusterId;
    use crate::pods::{Container, PodEvent, Resources, Termination};

    fn pod(name: &str, restarts: &[u32]) -> Pod {
        let containers = restarts
            .iter()
            .enumerate()
            .map(|(ix, restarts)| Container {
                name: format!("c{ix}"),
                restarts: *restarts,
                ..Container::default()
            })
            .collect();
        Pod {
            cluster: ClusterId::new("prod"),
            namespace: "search".into(),
            name: name.into(),
            phase: PodPhase::Running,
            containers,
            ..Pod::default()
        }
    }

    fn pending(name: &str, minutes: u64) -> Pod {
        Pod {
            phase: PodPhase::Pending,
            reason: "ContainerCreating".into(),
            age: Duration::from_secs(minutes * 60),
            ..pod(name, &[0])
        }
    }

    #[test]
    fn restarts_keep_the_top_five_by_count_and_count_every_restarted_pod() {
        let pods = [
            pod("a", &[3]),
            pod("b", &[0]),
            pod("c", &[41, 2]),
            pod("d", &[9]),
            pod("e", &[1]),
            pod("f", &[14]),
            pod("g", &[9]),
        ];
        let tops = PodTops::of(&pods);
        let names: Vec<_> = tops
            .restarts
            .iter()
            .map(|r| r.target.name.as_str())
            .collect();
        assert_eq!(names, ["c", "f", "d", "g", "a"]);
        assert_eq!(tops.restarted, 6);
        assert_eq!(tops.restarts[0].container, "c0");
        assert_eq!(tops.restarts[0].status(), Some(Status::Err));
        assert_eq!(tops.restarts[2].status(), Some(Status::Warn));
        assert_eq!(RestartingPod::of(&pods[4]).and_then(|r| r.status()), None);
    }

    #[test]
    fn restart_detail_names_the_container_and_its_last_termination() {
        let mut crashing = pod("indexer-0", &[41]);
        if let Some(container) = crashing.containers.first_mut() {
            container.name = "indexer".into();
            container.limits = Resources {
                cpu: None,
                memory: Some("4Gi".into()),
            };
            container.last_termination = Some(Termination {
                reason: "OOMKilled".into(),
                exit_code: 137,
                ..Termination::default()
            });
        }
        let row = RestartingPod::of(&crashing);
        assert_eq!(
            row.map(|r| r.detail()).as_deref(),
            Some("indexer · OOMKilled · exit 137 · limit 4Gi")
        );
        let bare = RestartingPod::of(&pod("api", &[2]));
        assert_eq!(bare.map(|r| r.detail()).as_deref(), Some("c0"));
    }

    #[test]
    fn pending_pods_sort_longest_first_with_the_latest_scheduling_message() {
        let mut gpu = pending("trainer", 192);
        gpu.events = vec![
            PodEvent {
                status: Status::Warn,
                reason: FAILED_SCHEDULING.into(),
                message: "0/64 nodes are available: old".into(),
                count: 1,
                age: Duration::from_secs(600),
            },
            PodEvent {
                status: Status::Warn,
                reason: FAILED_SCHEDULING.into(),
                message: "0/64 nodes are available: 4 Insufficient nvidia.com/gpu".into(),
                count: 3,
                age: Duration::from_secs(60),
            },
        ];
        let pods = [
            pending("etl", 19),
            gpu,
            pod("running", &[0]),
            pending("reports", 22),
        ];
        let tops = PodTops::of(&pods);
        let names: Vec<_> = tops
            .pending
            .iter()
            .map(|p| p.target.name.as_str())
            .collect();
        assert_eq!(names, ["trainer", "reports", "etl"]);
        assert_eq!(
            tops.pending[0].message,
            "0/64 nodes are available: 4 Insufficient nvidia.com/gpu"
        );
        assert_eq!(tops.pending[0].duration(), "3h 12m");
        assert_eq!(tops.pending[0].status(), Some(Status::Warn));
        assert_eq!(tops.pending[1].message, "ContainerCreating");
        assert_eq!(tops.pending[1].status(), None);
    }
}
