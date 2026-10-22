use std::time::Duration;

use flock_core::nodes::{Node, state_label};
use flock_core::{
    ClusterDetail, ClusterId, Command, Counts, Forward, Page, ResourceKind, ResourceRef, Seen,
    Snapshot, Status,
};

use crate::sim;

pub const SETTLE: Duration = Duration::from_secs(2);
const DAEMON_PODS: u32 = 6;
const UNSCHEDULABLE_TAINT: &str = "node.kubernetes.io/unschedulable:NoSchedule";
const NODE_KIND: &str = "Node";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Settle {
    Cordoned(ResourceRef),
    Uncordoned(ResourceRef),
    Drained(ResourceRef),
    Deleted(ResourceRef),
    #[cfg(feature = "loco")]
    Loco(crate::loco::Settle),
    Connected(ClusterId),
    RolledOut(ResourceRef),
    PodReplaced(ResourceRef),
    PodStarted(ResourceRef),
}

pub fn apply(snapshot: &mut Snapshot, command: &Command) -> Option<Settle> {
    match command {
        Command::Cordon(Seen { target, .. }) => {
            mark_node(snapshot, target, "Cordoning")?;
            Some(Settle::Cordoned(target.clone()))
        }
        Command::Uncordon(Seen { target, .. }) => {
            mark_node(snapshot, target, "Uncordoning")?;
            Some(Settle::Uncordoned(target.clone()))
        }
        Command::Drain(Seen { target, .. }) => {
            mark_node(snapshot, target, "Draining")?;
            Some(Settle::Drained(target.clone()))
        }
        Command::Delete(Seen { target, .. })
            if crate::pods::terminate(&mut snapshot.pods, target) =>
        {
            recount_pods(snapshot, &target.cluster);
            Some(Settle::PodReplaced(target.clone()))
        }
        Command::Delete(Seen { target, .. }) => {
            let resource = find_resource(snapshot, target)?;
            resource.status = Status::Progress;
            Some(Settle::Deleted(target.clone()))
        }
        #[cfg(feature = "loco")]
        Command::Loco(command) => crate::loco::apply(snapshot, command).map(Settle::Loco),
        Command::Connect(cluster) => {
            sim::reconnect(snapshot, cluster, sim::CONNECTING)?;
            Some(Settle::Connected(cluster.clone()))
        }
        Command::Reauthenticate(cluster) => {
            sim::reconnect(snapshot, cluster, sim::REAUTHENTICATING)?;
            Some(Settle::Connected(cluster.clone()))
        }
        Command::CopySpec { from, to, .. } => {
            sim::copy_spec(snapshot, from, &to.target)?;
            Some(Settle::RolledOut(to.target.clone()))
        }
        _ => None,
    }
}

pub fn forward(snapshot: &mut Snapshot, command: &Command) -> bool {
    match command {
        Command::PortForward {
            service,
            local,
            remote,
        } => {
            snapshot.forwards.retain(|forward| forward.local != *local);
            snapshot.forwards.push(Forward {
                target: service.clone(),
                local: *local,
                remote: *remote,
            });
            true
        }
        Command::StopPortForward { local } => {
            let before = snapshot.forwards.len();
            snapshot.forwards.retain(|forward| forward.local != *local);
            snapshot.forwards.len() != before
        }
        _ => false,
    }
}

pub fn settle(snapshot: &mut Snapshot, settle: &Settle) -> Option<Settle> {
    match settle {
        Settle::Cordoned(target) => {
            if let Some(node) = find_node(snapshot, target) {
                set_schedulable(node, false);
            }
        }
        Settle::Uncordoned(target) => {
            if let Some(node) = find_node(snapshot, target) {
                set_schedulable(node, true);
            }
        }
        Settle::Drained(target) => {
            if let Some(node) = find_node(snapshot, target) {
                set_schedulable(node, false);
                node.pods = node.pods.min(DAEMON_PODS);
            }
        }
        Settle::Deleted(target) => remove_resource(snapshot, target),
        #[cfg(feature = "loco")]
        Settle::Loco(pending) => crate::loco::settle(snapshot, pending),
        Settle::Connected(cluster) => sim::connected(snapshot, cluster),
        Settle::RolledOut(target) => sim::rolled_out(snapshot, target),
        Settle::PodReplaced(target) => {
            let next = crate::pods::replace(&mut snapshot.pods, target).map(Settle::PodStarted);
            recount_pods(snapshot, &target.cluster);
            return next;
        }
        Settle::PodStarted(target) => {
            crate::pods::start(&mut snapshot.pods, target);
            recount_pods(snapshot, &target.cluster);
        }
    }
    if let Settle::Cordoned(target) | Settle::Uncordoned(target) | Settle::Drained(target) = settle
    {
        recount_nodes(snapshot, target);
    }
    None
}

pub fn recount_pods(snapshot: &mut Snapshot, cluster: &ClusterId) {
    let counts = flock_core::pods::nav_counts(&snapshot.pods, cluster);
    let pods = snapshot.pods.iter().filter(|pod| &pod.cluster == cluster);
    let tops = flock_core::pod_tops::PodTops::of(pods);
    if let Some(detail) = snapshot.details.get_mut(cluster) {
        detail.overview.tops = tops;
    }
    let summary = snapshot.clusters.iter_mut().find(|c| &c.id == cluster);
    if let Some(summary) = summary {
        summary.nav.insert(Page::Pods, counts);
    }
}

fn recount_nodes(snapshot: &mut Snapshot, target: &ResourceRef) {
    let Some(summary) = snapshot
        .detail(&target.cluster)
        .map(ClusterDetail::node_summary)
    else {
        return;
    };
    let flagged = u32::try_from(summary.cordoned + summary.not_ready).unwrap_or(u32::MAX);
    let total = u32::try_from(summary.total).unwrap_or(u32::MAX);
    let cluster = snapshot
        .clusters
        .iter_mut()
        .find(|cluster| cluster.id == target.cluster);
    if let Some(cluster) = cluster {
        cluster
            .nav
            .insert(Page::Nodes, Counts::total(total).with_warn(flagged));
    }
}

fn detail<'a>(snapshot: &'a mut Snapshot, target: &ResourceRef) -> Option<&'a mut ClusterDetail> {
    snapshot.details.get_mut(&target.cluster)
}

fn find_node<'a>(snapshot: &'a mut Snapshot, target: &ResourceRef) -> Option<&'a mut Node> {
    if target.kind != NODE_KIND {
        return None;
    }
    detail(snapshot, target)?
        .nodes
        .iter_mut()
        .find(|node| node.name == target.name)
}

fn mark_node(snapshot: &mut Snapshot, target: &ResourceRef, state: &str) -> Option<()> {
    let node = find_node(snapshot, target)?;
    node.status = Status::Progress;
    node.state = state.into();
    Some(())
}

fn set_schedulable(node: &mut Node, schedulable: bool) {
    node.schedulable = schedulable;
    node.taints.retain(|taint| taint != UNSCHEDULABLE_TAINT);
    node.state = state_label(true, schedulable);
    if schedulable {
        node.status = Status::Ok;
    } else {
        node.status = Status::Warn;
        node.taints.push(UNSCHEDULABLE_TAINT.into());
    }
}

fn kind_of(target: &ResourceRef) -> Option<ResourceKind> {
    ResourceKind::ALL
        .into_iter()
        .find(|kind| kind.name() == target.kind)
}

fn find_resource<'a>(
    snapshot: &'a mut Snapshot,
    target: &ResourceRef,
) -> Option<&'a mut flock_core::resources::Resource> {
    let kind = kind_of(target)?;
    let namespace = target.namespace.as_deref()?;
    detail(snapshot, target)?
        .resources
        .get_mut(&kind)?
        .iter_mut()
        .find(|row| row.namespace == namespace && row.name == target.name)
}

fn remove_resource(snapshot: &mut Snapshot, target: &ResourceRef) {
    let Some(kind) = kind_of(target) else {
        return;
    };
    let Some(namespace) = target.namespace.as_deref() else {
        return;
    };
    let Some(rows) = detail(snapshot, target).and_then(|d| d.resources.get_mut(&kind)) else {
        return;
    };
    let before = rows.len();
    rows.retain(|row| !(row.namespace == namespace && row.name == target.name));
    if rows.len() == before {
        return;
    }
    let summary = snapshot
        .clusters
        .iter_mut()
        .find(|cluster| cluster.id == target.cluster);
    if let Some(total) = summary
        .and_then(|cluster| cluster.nav.get_mut(&kind.page()))
        .and_then(|counts| counts.total.as_mut())
    {
        *total = total.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flock_core::{ClusterId, Uid};

    const CLUSTER: &str = "prod-us-east";

    fn node_ref(name: &str) -> ResourceRef {
        ResourceRef {
            cluster: ClusterId::new(CLUSTER),
            kind: NODE_KIND.into(),
            namespace: None,
            name: name.into(),
        }
    }

    fn seen(target: ResourceRef) -> Seen {
        Seen::new(target, Uid::default())
    }

    fn node(snapshot: &Snapshot, name: &str) -> Node {
        let detail = snapshot.detail(&ClusterId::new(CLUSTER));
        let found = detail.and_then(|d| d.nodes.iter().find(|n| n.name == name).cloned());
        found.unwrap_or_else(|| panic!("node {name} missing"))
    }

    #[test]
    fn cordon_goes_through_progress_then_disables_scheduling() {
        let mut snapshot = crate::snapshot();
        let target = node_ref("ip-10-4-12-87");
        let pending = apply(&mut snapshot, &Command::Cordon(seen(target.clone())));
        assert_eq!(pending, Some(Settle::Cordoned(target.clone())));
        assert_eq!(node(&snapshot, "ip-10-4-12-87").status, Status::Progress);
        settle(&mut snapshot, &Settle::Cordoned(target));
        let cordoned = node(&snapshot, "ip-10-4-12-87");
        assert!(!cordoned.schedulable);
        assert_eq!(cordoned.status, Status::Warn);
        assert_eq!(cordoned.state, "Ready,SchedulingDisabled");
        assert!(cordoned.taints.iter().any(|t| t == UNSCHEDULABLE_TAINT));
    }

    #[test]
    fn uncordon_restores_ready() {
        let mut snapshot = crate::snapshot();
        let target = node_ref("ip-10-4-31-12");
        let pending = apply(&mut snapshot, &Command::Uncordon(seen(target.clone())));
        let Some(pending) = pending else {
            panic!("uncordon should settle");
        };
        settle(&mut snapshot, &pending);
        let node = node(&snapshot, "ip-10-4-31-12");
        assert!(node.schedulable);
        assert_eq!(node.status, Status::Ok);
        let nav = snapshot
            .cluster(&ClusterId::new(CLUSTER))
            .and_then(|c| c.nav.get(&Page::Nodes).copied());
        assert_eq!(nav, Some(Counts::total(14)));
        assert!(!node.taints.iter().any(|t| t == UNSCHEDULABLE_TAINT));
    }

    #[test]
    fn drain_evicts_all_but_daemon_pods() {
        let mut snapshot = crate::snapshot();
        let target = node_ref("ip-10-4-30-5");
        apply(&mut snapshot, &Command::Drain(seen(target.clone())));
        settle(&mut snapshot, &Settle::Drained(target));
        let node = node(&snapshot, "ip-10-4-30-5");
        assert_eq!(node.pods, DAEMON_PODS);
        assert!(!node.schedulable);
    }

    #[test]
    fn delete_marks_progress_then_removes_and_updates_nav() {
        let mut snapshot = crate::snapshot();
        let id = ClusterId::new(CLUSTER);
        let target = ResourceRef {
            cluster: id.clone(),
            kind: "ConfigMap".into(),
            namespace: Some("payments".into()),
            name: "checkout-api-config".into(),
        };
        let pending = apply(&mut snapshot, &Command::Delete(seen(target.clone())));
        assert_eq!(pending, Some(Settle::Deleted(target.clone())));
        settle(&mut snapshot, &Settle::Deleted(target));
        let rows = snapshot
            .detail(&id)
            .map(|d| d.resources(ResourceKind::ConfigMap).len());
        assert_eq!(rows, Some(87));
        let nav = snapshot
            .cluster(&id)
            .and_then(|c| c.nav.get(&Page::ConfigMaps))
            .and_then(|c| c.total);
        assert_eq!(nav, Some(87));
    }

    #[test]
    fn port_forwards_are_listed_until_stopped() {
        let mut snapshot = crate::snapshot();
        let service = ResourceRef {
            cluster: ClusterId::new(CLUSTER),
            kind: "Service".into(),
            namespace: Some("payments".into()),
            name: "checkout-api".into(),
        };
        let start = Command::PortForward {
            service,
            local: 8080,
            remote: 80,
        };
        assert!(forward(&mut snapshot, &start));
        assert!(forward(&mut snapshot, &start));
        assert_eq!(snapshot.forwards.len(), 1);
        assert!(forward(
            &mut snapshot,
            &Command::StopPortForward { local: 8080 }
        ));
        assert_eq!(snapshot.forwards, Vec::new());
        assert!(!forward(
            &mut snapshot,
            &Command::StopPortForward { local: 8080 }
        ));
    }

    #[test]
    fn unknown_targets_are_ignored() {
        let mut snapshot = crate::snapshot();
        let before = snapshot.clone();
        assert_eq!(
            apply(&mut snapshot, &Command::Cordon(seen(node_ref("nope")))),
            None
        );
        assert_eq!(apply(&mut snapshot, &Command::ToggleSidebar), None);
        assert_eq!(snapshot, before);
    }
}
