use flock_core::action::Notice;
use flock_core::compare::short_names;
use flock_core::flux::{FluxAction, FluxKey, Phase};
use flock_core::{ClusterId, Command, ResourceKind, ResourceRef, Seen, Snapshot, Status, Uid};
use serde_json::Value;

const POD: &str = "Pod";
const APPLYING: &str = "Applying";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mutation {
    Cordon(Seen),
    Uncordon(Seen),
    Drain(Seen),
    Delete(Seen),
    Restart(Seen),
    Scale(Seen, u32),
    Rollback(Seen, u64),
    Flux(FluxKey, Uid, FluxAction),
    CopySpec {
        target: ResourceRef,
        uid: Uid,
        from: ClusterId,
        path: String,
        template: Value,
    },
}

impl Mutation {
    pub fn from_command(command: &Command) -> Option<Self> {
        let mutation = match command {
            Command::Cordon(target) => Self::Cordon(target.clone()),
            Command::Uncordon(target) => Self::Uncordon(target.clone()),
            Command::Drain(target) => Self::Drain(target.clone()),
            Command::Delete(target) => Self::Delete(target.clone()),
            Command::RestartRollout(target) => Self::Restart(target.clone()),
            Command::Scale { target, replicas } => Self::Scale(target.clone(), *replicas),
            Command::Rollback { target, revision } => Self::Rollback(target.clone(), *revision),
            Command::Flux(key, uid, action) => Self::Flux(key.clone(), uid.clone(), *action),
            _ => return None,
        };
        Some(mutation)
    }

    pub fn target(&self) -> ResourceRef {
        match self {
            Self::Cordon(seen) => seen.target.clone(),
            Self::Uncordon(seen) => seen.target.clone(),
            Self::Drain(seen) => seen.target.clone(),
            Self::Delete(seen) => seen.target.clone(),
            Self::Restart(seen) => seen.target.clone(),
            Self::Scale(seen, _) => seen.target.clone(),
            Self::Rollback(seen, _) => seen.target.clone(),
            Self::CopySpec { target, .. } => target.clone(),
            Self::Flux(key, ..) => ResourceRef {
                cluster: key.cluster.clone(),
                kind: key.kind.label().to_owned(),
                namespace: Some(key.namespace.clone()),
                name: key.name.clone(),
            },
        }
    }

    pub fn verb(&self) -> &'static str {
        match self {
            Self::Cordon(_) => "Cordon",
            Self::Uncordon(_) => "Uncordon",
            Self::Drain(_) => "Drain",
            Self::Delete(_) => "Delete",
            Self::Restart(_) => "Restart",
            Self::Scale(..) => "Scale",
            Self::Rollback(..) => "Roll back",
            Self::CopySpec { .. } => "Apply",
            Self::Flux(.., FluxAction::Reconcile) => "Reconcile",
            Self::Flux(.., FluxAction::ReconcileWithSource) => "Reconcile",
            Self::Flux(.., FluxAction::Force) => "Force",
            Self::Flux(.., FluxAction::Suspend) => "Suspend",
            Self::Flux(.., FluxAction::Resume) => "Resume",
        }
    }

    pub fn done(&self) -> Option<Notice> {
        let Self::CopySpec {
            target, from, path, ..
        } = self
        else {
            return None;
        };
        let (base, other) = short_names(target.cluster.as_str(), from.as_str());
        Some(Notice {
            status: Status::Ok,
            title: format!("Applied {other} {path} to {base}"),
            detail: Some(format!("{} · {target}", target.cluster)),
        })
    }

    fn node_state(&self) -> Option<&'static str> {
        match self {
            Self::Cordon(_) => Some("Cordoning"),
            Self::Uncordon(_) => Some("Uncordoning"),
            Self::Drain(_) => Some("Draining"),
            _ => None,
        }
    }
}

fn mark_node(snapshot: &mut Snapshot, target: &ResourceRef, state: &str) {
    let node = snapshot
        .details
        .get_mut(&target.cluster)
        .and_then(|detail| {
            detail
                .nodes
                .iter_mut()
                .find(|node| node.name == target.name)
        });
    if let Some(node) = node {
        node.status = Status::Progress;
        state.clone_into(&mut node.state);
    }
}

fn mark_resource(snapshot: &mut Snapshot, target: &ResourceRef) {
    let namespace = target.namespace.as_deref().unwrap_or_default();
    let kind = ResourceKind::ALL
        .into_iter()
        .find(|kind| kind.name() == target.kind);
    let rows = kind.and_then(|kind| {
        snapshot
            .details
            .get_mut(&target.cluster)
            .and_then(|detail| detail.resources.get_mut(&kind))
    });
    let row = rows.and_then(|rows| {
        rows.iter_mut()
            .find(|row| row.namespace == namespace && row.name == target.name)
    });
    if let Some(row) = row {
        row.status = Status::Progress;
    }
    let summary = snapshot
        .resources
        .iter_mut()
        .find(|summary| &summary.resource == target);
    if let Some(summary) = summary {
        summary.status = Some(Status::Progress);
    }
    if target.kind != POD {
        return;
    }
    let pod = snapshot.pods.iter_mut().find(|pod| {
        pod.cluster == target.cluster && pod.namespace == namespace && pod.name == target.name
    });
    if let Some(pod) = pod {
        pod.status = Status::Progress;
    }
}

fn mark_flux(snapshot: &mut Snapshot, key: &FluxKey) {
    if let Some(object) = snapshot.flux.iter_mut().find(|object| &object.key == key) {
        object.phase = Phase::Reconciling;
    }
}

fn mark_comparison(snapshot: &mut Snapshot, target: &ResourceRef, from: &ClusterId, path: &str) {
    let column = snapshot
        .comparisons
        .iter_mut()
        .filter(|comparison| comparison.matches(target))
        .flat_map(|comparison| comparison.clusters.iter_mut())
        .find(|compared| compared.cluster == target.cluster);
    if let Some(compared) = column {
        compared.status = Status::Progress;
        compared.health = format!("{APPLYING} {from} {path}");
    }
}

pub fn mark(snapshot: &mut Snapshot, mutation: &Mutation) {
    if let Mutation::CopySpec {
        target, from, path, ..
    } = mutation
    {
        mark_comparison(snapshot, target, from, path);
    }
    if let Some(state) = mutation.node_state() {
        mark_node(snapshot, &mutation.target(), state);
        return;
    }
    if let Mutation::Flux(key, ..) = mutation {
        mark_flux(snapshot, key);
        return;
    }
    mark_resource(snapshot, &mutation.target());
}

#[cfg(test)]
mod tests {
    use flock_core::ClusterId;
    use flock_core::nodes::{Node, Usage};
    use flock_core::resources::Resource;

    use super::*;

    fn target(kind: &str, namespace: Option<&str>, name: &str) -> ResourceRef {
        ResourceRef {
            cluster: ClusterId::new("a"),
            kind: kind.into(),
            namespace: namespace.map(Into::into),
            name: name.into(),
        }
    }

    fn snapshot() -> Snapshot {
        let mut snapshot = Snapshot::default();
        let detail = snapshot.details.entry(ClusterId::new("a")).or_default();
        detail.nodes.push(Node {
            cpu_commitment: flock_core::overview::Commitment::default(),
            memory_commitment: flock_core::overview::Commitment::default(),
            uid: Uid::default(),
            name: "n1".into(),
            status: Status::Ok,
            state: "Ready".into(),
            schedulable: true,
            instance_type: String::new(),
            zone: String::new(),
            version: String::new(),
            internal_ip: String::new(),
            os_image: String::new(),
            runtime: String::new(),
            cpu: Usage::default(),
            memory: Usage::default(),
            disk: None,
            pods: 0,
            pod_capacity: 0,
            age: std::time::Duration::ZERO,
            conditions: Vec::new(),
            taints: Vec::new(),
            labels: Vec::new(),
            top_pods: Vec::new(),
        });
        detail.resources.insert(
            ResourceKind::Deployment,
            vec![Resource {
                uid: Uid::default(),
                namespace: "web".into(),
                name: "podinfo".into(),
                status: Status::Ok,
                cells: Vec::new(),
                age: std::time::Duration::ZERO,
                yaml: String::new(),
                labels: std::collections::BTreeMap::new(),
                conditions: Vec::new(),
                diagnosis: None,
            }],
        );
        snapshot
    }

    fn seen(target: ResourceRef) -> Seen {
        Seen::new(target, Uid::new("u1"))
    }

    #[test]
    fn commands_become_mutations_with_targets() {
        let node = target("Node", None, "n1");
        let mutation = Mutation::from_command(&Command::Drain(seen(node.clone())));
        assert_eq!(mutation, Some(Mutation::Drain(seen(node.clone()))));
        assert_eq!(
            mutation.map(|m| (m.target(), m.verb())),
            Some((node, "Drain"))
        );
        assert_eq!(Mutation::from_command(&Command::ToggleSidebar), None);
    }

    #[test]
    fn pending_node_actions_show_progress_and_their_verb() {
        let mut snapshot = snapshot();
        mark(
            &mut snapshot,
            &Mutation::Cordon(seen(target("Node", None, "n1"))),
        );
        let node = snapshot
            .detail(&ClusterId::new("a"))
            .and_then(|detail| detail.nodes.first().cloned());
        assert_eq!(
            node.map(|n| (n.status, n.state)),
            Some((Status::Progress, "Cordoning".to_owned()))
        );
    }

    #[test]
    fn pending_workload_actions_mark_the_row() {
        let mut snapshot = snapshot();
        let deployment = target("Deployment", Some("web"), "podinfo");
        mark(&mut snapshot, &Mutation::Scale(seen(deployment), 3));
        let status = snapshot
            .detail(&ClusterId::new("a"))
            .map(|detail| detail.resources(ResourceKind::Deployment)[0].status);
        assert_eq!(status, Some(Status::Progress));
    }

    #[test]
    fn copying_a_spec_marks_the_compare_column_and_reports_success() {
        use flock_core::compare::{ComparedCluster, Comparison};
        let target = target("Deployment", Some("payments"), "checkout-api");
        let mut snapshot = Snapshot::default();
        snapshot.comparisons.push(Comparison {
            kind: "Deployment".into(),
            namespace: Some("payments".into()),
            name: "checkout-api".into(),
            clusters: vec![ComparedCluster {
                cluster: ClusterId::new("a"),
                uid: Uid::new("u1"),
                status: Status::Err,
                health: "Degraded".into(),
            }],
            fields: Vec::new(),
            spec: None,
        });
        let mutation = Mutation::CopySpec {
            target: target.clone(),
            uid: Uid::new("u1"),
            from: ClusterId::new("b"),
            path: "spec.template".into(),
            template: Value::Null,
        };
        assert_eq!(
            Mutation::from_command(&Command::CopySpec {
                from: ClusterId::new("b"),
                to: seen(target),
                path: "spec.template".into(),
            }),
            None
        );
        mark(&mut snapshot, &mutation);
        let column = &snapshot.comparisons[0].clusters[0];
        assert_eq!(column.status, Status::Progress);
        assert_eq!(column.health, "Applying b spec.template");
        assert_eq!(
            mutation.done().map(|notice| notice.title),
            Some("Applied b spec.template to a".to_owned())
        );
        assert_eq!(mutation.verb(), "Apply");
    }
}
