use std::time::Duration;

use flock_core::workload::{Event, Pending, Pod, Revision};
use flock_core::{Status, Workload};

use super::data::{BROKEN_IMAGE, crashing_pod, healthy_conditions, running_pod};

const NODES: [&str; 3] = ["ip-10-4-12-87", "ip-10-4-14-201", "ip-10-4-22-63"];
const HEALTHY_CPU: u32 = 196;
const HEALTHY_MEM_MIB: u64 = 294;
const SUFFIXES: [&str; 12] = [
    "h2k9d", "p7xqm", "w4rtc", "b9vnz", "c3jfl", "t6mzs", "k8dwe", "r5gqa", "n2vbx", "f9tlk",
    "d4wsy", "j7hpc",
];
const HASHES: [&str; 6] = ["8d4e1b", "c27f90", "4a9e3d", "e6b1c8", "71fa2c", "b3d58e"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Restart,
    Scale(u32),
    Rollback(u64),
}

#[derive(Debug, Default)]
pub struct Names {
    pods: usize,
    sets: usize,
}

impl Names {
    fn pod(&mut self, owner: &str) -> String {
        let suffix = SUFFIXES
            .get(self.pods % SUFFIXES.len())
            .copied()
            .unwrap_or_default();
        self.pods += 1;
        format!("{owner}-{suffix}")
    }

    fn replica_set(&mut self, workload: &str) -> String {
        let hash = HASHES
            .get(self.sets % HASHES.len())
            .copied()
            .unwrap_or_default();
        self.sets += 1;
        format!("{workload}-{hash}")
    }
}

pub fn begin(workload: &mut Workload, action: Action) {
    workload.pending = Some(match action {
        Action::Restart => Pending::Restarting,
        Action::Scale(replicas) => Pending::Scaling { replicas },
        Action::Rollback(revision) => Pending::RollingBack { revision },
    });
}

pub fn finish(workload: &mut Workload, action: Action, names: &mut Names) {
    workload.pending = None;
    match action {
        Action::Restart => restart(workload, names),
        Action::Scale(replicas) => scale(workload, replicas, names),
        Action::Rollback(revision) => rollback(workload, revision, names),
    }
    let ready = workload.ready();
    if let Some(current) = workload.revisions.iter_mut().find(|r| r.current) {
        current.ready = ready;
        current.replicas = workload.desired;
    }
    if ready >= workload.desired {
        workload.conditions = healthy_conditions(workload);
    }
}

fn next_number(workload: &Workload) -> u64 {
    workload
        .revisions
        .iter()
        .map(|r| r.number)
        .max()
        .unwrap_or_default()
        + 1
}

fn retire_current(workload: &mut Workload) {
    for revision in &mut workload.revisions {
        if revision.current {
            revision.current = false;
            revision.ready = 0;
            revision.replicas = 0;
        }
    }
}

fn new_pod(image: &str, owner: &str, ix: usize, first_of_rollout: bool, names: &mut Names) -> Pod {
    let name = names.pod(owner);
    let node = NODES.get(ix % NODES.len()).copied().unwrap_or_default();
    if image == BROKEN_IMAGE && first_of_rollout {
        crashing_pod(name, node, 1, Duration::ZERO)
    } else {
        running_pod(name, node, HEALTHY_CPU, HEALTHY_MEM_MIB, Duration::ZERO)
    }
}

fn replace_pods(workload: &mut Workload, image: &str, owner: &str, names: &mut Names) {
    let count = usize::try_from(workload.desired).unwrap_or_default();
    workload.pods = (0..count)
        .map(|ix| new_pod(image, owner, ix, ix == 0, names))
        .collect();
}

fn restart(workload: &mut Workload, names: &mut Names) {
    let Some(current) = workload.current_revision().cloned() else {
        return;
    };
    let owner = names.replica_set(&workload.target.name);
    let number = next_number(workload);
    retire_current(workload);
    replace_pods(workload, &current.image, &owner, names);
    let event_message = format!("Scaled up replica set {owner} to {}", workload.desired);
    workload.revisions.insert(
        0,
        Revision {
            number,
            owner,
            age: Duration::ZERO,
            current: true,
            ..current
        },
    );
    record(workload, Status::Ok, "ScalingReplicaSet", event_message);
}

fn scale(workload: &mut Workload, replicas: u32, names: &mut Names) {
    let Some(current) = workload.current_revision().cloned() else {
        return;
    };
    let wanted = usize::try_from(replicas).unwrap_or_default();
    let direction = if replicas >= workload.desired {
        "up"
    } else {
        "down"
    };
    workload.desired = replicas;
    workload.pods.sort_by_key(|pod| !pod.is_ready());
    workload.pods.truncate(wanted);
    let missing = wanted.saturating_sub(workload.pods.len());
    let start = workload.pods.len();
    for ix in start..start + missing {
        let pod = new_pod(&current.image, &current.owner, ix, false, names);
        workload.pods.push(pod);
    }
    let message = format!(
        "Scaled {direction} replica set {} to {replicas}",
        current.owner
    );
    record(workload, Status::Ok, "ScalingReplicaSet", message);
}

fn rollback(workload: &mut Workload, revision: u64, names: &mut Names) {
    let number = next_number(workload);
    let Some(position) = workload.revisions.iter().position(|r| r.number == revision) else {
        return;
    };
    retire_current(workload);
    let mut target = workload.revisions.remove(position);
    target.number = number;
    target.current = true;
    target.age = Duration::ZERO;
    replace_pods(workload, &target.image, &target.owner, names);
    workload.revisions.insert(0, target);
    let message = format!(
        "Rolled back deployment \"{}\" to revision {revision}",
        workload.target.name
    );
    record(workload, Status::Ok, "DeploymentRollback", message);
}

fn record(workload: &mut Workload, status: Status, reason: &str, message: String) {
    workload.events.insert(
        0,
        Event {
            age: Duration::ZERO,
            status,
            reason: reason.into(),
            message,
            count: 1,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workload::data::checkout_api;

    fn run(action: Action) -> Workload {
        let mut workload = checkout_api();
        begin(&mut workload, action);
        assert_eq!(workload.status(), Status::Progress);
        finish(&mut workload, action, &mut Names::default());
        assert_eq!(workload.pending, None);
        workload
    }

    #[test]
    fn rollback_reuses_the_old_replica_set_as_a_new_revision() {
        let workload = run(Action::Rollback(36));
        let numbers: Vec<u64> = workload.revisions.iter().map(|r| r.number).collect();
        assert_eq!(numbers, [38, 37, 35]);
        let current = workload.current_revision().cloned();
        assert_eq!(
            current.as_ref().map(|r| r.owner.as_str()),
            Some("checkout-api-5b8f2a")
        );
        assert_eq!(current.map(|r| r.pods_label()).as_deref(), Some("3/3"));
        assert_eq!(workload.status(), Status::Ok);
        assert!(!workload.stalled());
        assert_eq!(
            workload
                .condition_line()
                .map(|line| line.compact())
                .as_deref(),
            Some("Available=True · MinimumReplicasAvailable")
        );
        assert_eq!(
            workload.sentence(),
            "All three pods are ready on revision 38."
        );
        assert_eq!(
            workload.events.first().map(|e| e.reason.as_str()),
            Some("DeploymentRollback")
        );
    }

    #[test]
    fn restart_keeps_the_broken_image_crashing() {
        let workload = run(Action::Restart);
        assert_eq!(workload.revisions.len(), 4);
        assert_eq!(workload.current_revision().map(|r| r.number), Some(38));
        assert_eq!(workload.ready(), 2);
        assert_eq!(workload.status(), Status::Err);
        assert!(
            workload
                .pods
                .iter()
                .all(|pod| pod.name.starts_with("checkout-api-8d4e1b-"))
        );
    }

    #[test]
    fn scaling_down_drops_unready_pods_first() {
        let workload = run(Action::Scale(2));
        assert_eq!(workload.desired, 2);
        assert!(workload.pods.iter().all(Pod::is_ready));
        assert_eq!(workload.status(), Status::Ok);
        assert_eq!(
            workload
                .current_revision()
                .map(Revision::pods_label)
                .as_deref(),
            Some("2/2")
        );
    }

    #[test]
    fn scaling_up_adds_running_pods() {
        let workload = run(Action::Scale(5));
        assert_eq!(workload.pods.len(), 5);
        assert_eq!(workload.ready(), 4);
        assert_eq!(workload.ready_label(), "4/5 ready");
    }

    #[test]
    fn names_never_repeat_within_a_cycle() {
        let mut names = Names::default();
        let first = names.pod("rs");
        let second = names.pod("rs");
        assert_ne!(first, second);
    }
}
