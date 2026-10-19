use std::collections::BTreeMap;

use flock_core::pods::Pod;
use flock_core::related::{
    self, CLAIM, CONFIG_MAP, Fact, Key, NAMESPACE, NODE, PERSISTENT_VOLUME, POD, REPLICA_SET,
    SECRET, SERVICE, TEMPLATE_HASH,
};
use flock_core::resources::{Resource, ResourceKind};
use flock_core::{ClusterDetail, ClusterId, Command, ResourceRef, Snapshot};

const APP_LABEL: &str = "app.kubernetes.io/name";
const CRON_JOB: &str = "CronJob";
const VOLUME_COLUMN: usize = 3;
const SUFFIX: char = '-';
const CONFIG_SUFFIX: &str = "-config";
const SECRETS_SUFFIX: &str = "-secrets";
const CREDENTIALS_SUFFIX: &str = "-credentials";
const TLS_SUFFIX: &str = "-tls";
const SOURCES: [(&str, &str); 3] = [
    ("configmap ", CONFIG_MAP),
    ("secret ", SECRET),
    ("pvc ", CLAIM),
];
const KEPT: usize = 16;
const DOMAIN: char = '.';

fn app(name: &str) -> BTreeMap<String, String> {
    BTreeMap::from([(APP_LABEL.to_owned(), name.to_owned())])
}

fn workload_of(pod: &Pod) -> Option<(String, Option<String>)> {
    let owner = pod.owner.as_ref()?;
    if owner.kind != REPLICA_SET {
        return Some((owner.name.clone(), None));
    }
    let (deployment, hash) = owner.name.rsplit_once(SUFFIX)?;
    Some((deployment.to_owned(), Some(hash.to_owned())))
}

fn pod_fact(pod: &Pod) -> Fact {
    let mut labels = pod.labels.clone();
    if let Some((workload, hash)) = workload_of(pod) {
        labels.entry(APP_LABEL.to_owned()).or_insert(workload);
        if let Some(hash) = hash {
            labels.entry(TEMPLATE_HASH.to_owned()).or_insert(hash);
        }
    }
    let volumes = pod.volumes.iter().filter_map(|volume| {
        SOURCES.iter().find_map(|(prefix, kind)| {
            let name = volume.source.strip_prefix(prefix)?;
            Some(Key::new(*kind, name))
        })
    });
    let node = pod
        .node
        .iter()
        .filter_map(|node| node.split(DOMAIN).next())
        .map(|node| Key::new(NODE, node));
    Fact {
        status: pod.status,
        note: pod.reason.clone(),
        age: pod.age,
        labels,
        owner: pod
            .owner
            .as_ref()
            .map(|owner| Key::new(owner.kind.clone(), owner.name.clone())),
        uses: volumes.chain(node).collect(),
        ..Fact::new(POD, Some(&pod.namespace), &pod.name)
    }
}

struct Rows<'a>(&'a ClusterDetail);

impl Rows<'_> {
    fn has(&self, kind: ResourceKind, namespace: &str, name: &str) -> bool {
        self.0
            .resources(kind)
            .iter()
            .any(|row| row.namespace == namespace && row.name == name)
    }

    fn named(&self, kind: ResourceKind, row: &Resource, suffix: &str) -> Option<Key> {
        let name = format!("{}{suffix}", row.name);
        self.has(kind, &row.namespace, &name)
            .then(|| Key::new(kind.name(), name))
    }

    fn cron_of(&self, job: &Resource) -> Option<Key> {
        let (cron, _) = job.name.rsplit_once(SUFFIX)?;
        self.has(ResourceKind::CronJob, &job.namespace, cron)
            .then(|| Key::new(CRON_JOB, cron))
    }

    fn fact(&self, kind: ResourceKind, row: &Resource) -> Fact {
        let base = Fact {
            status: row.status,
            note: kind.status_cell(row).unwrap_or_default(),
            age: row.age,
            labels: row.labels.clone(),
            ..Fact::new(kind.name(), Some(&row.namespace), &row.name)
        };
        match kind {
            ResourceKind::Deployment => self.workload(base, row),
            ResourceKind::StatefulSet => self.workload(base, row),
            ResourceKind::DaemonSet => self.workload(base, row),
            ResourceKind::Job => Fact {
                owner: self.cron_of(row),
                ..base
            },
            ResourceKind::CronJob => base,
            ResourceKind::Service => Fact {
                selector: app(&row.name),
                ..base
            },
            ResourceKind::Ingress => {
                let service = Key::new(SERVICE, row.name.clone());
                let tls = self.named(ResourceKind::Secret, row, TLS_SUFFIX);
                Fact {
                    uses: std::iter::once(service).chain(tls).collect(),
                    ..base
                }
            }
            ResourceKind::ConfigMap => base,
            ResourceKind::Secret => base,
            ResourceKind::PersistentVolumeClaim => {
                let volume = row
                    .cells
                    .get(VOLUME_COLUMN)
                    .map(|name| Key::new(PERSISTENT_VOLUME, name.clone()));
                Fact {
                    uses: volume.into_iter().collect(),
                    ..base
                }
            }
        }
    }

    fn workload(&self, base: Fact, row: &Resource) -> Fact {
        let uses = [
            self.named(ResourceKind::ConfigMap, row, CONFIG_SUFFIX),
            self.named(ResourceKind::Secret, row, SECRETS_SUFFIX),
            self.named(ResourceKind::Secret, row, CREDENTIALS_SUFFIX),
        ];
        Fact {
            template: app(&row.name),
            uses: uses.into_iter().flatten().collect(),
            ..base
        }
    }
}

pub fn facts(snapshot: &Snapshot, cluster: &ClusterId) -> Vec<Fact> {
    let pods = snapshot
        .pods
        .iter()
        .filter(|pod| &pod.cluster == cluster)
        .map(pod_fact);
    let Some(detail) = snapshot.detail(cluster) else {
        return pods.collect();
    };
    let rows = Rows(detail);
    let resources = ResourceKind::ALL.into_iter().flat_map(|kind| {
        let rows = &rows;
        detail
            .resources(kind)
            .iter()
            .map(move |row| rows.fact(kind, row))
    });
    let nodes = detail.nodes.iter().map(|node| Fact {
        status: node.status,
        note: node.state.clone(),
        age: node.age,
        ..Fact::new(NODE, None, &node.name)
    });
    pods.chain(resources).chain(nodes).collect()
}

fn relate(snapshot: &Snapshot, target: &ResourceRef) -> Option<related::Related> {
    let facts = facts(snapshot, &target.cluster);
    if target.kind == NAMESPACE {
        let events = &snapshot.detail(&target.cluster)?.events;
        return Some(related::contents(
            &target.cluster,
            &facts,
            events,
            &target.name,
        ));
    }
    Some(related::related(&target.cluster, &facts, target))
}

pub fn handle(command: &Command, snapshot: &mut Snapshot) -> bool {
    let target = match command {
        Command::OpenResource(target) => target,
        Command::InspectResource(target) => target,
        _ => return false,
    };
    let Some(found) = relate(snapshot, target) else {
        return false;
    };
    let Some(detail) = snapshot.details.get_mut(&target.cluster) else {
        return false;
    };
    detail.related.retain(|known| known.target != found.target);
    detail.related.insert(0, found);
    detail.related.truncate(KEPT);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkout() -> ResourceRef {
        ResourceRef {
            cluster: ClusterId::new("prod-us-east"),
            kind: "Deployment".into(),
            namespace: Some("payments".into()),
            name: "checkout-api".into(),
        }
    }

    fn titles(snapshot: &Snapshot, target: &ResourceRef) -> Vec<String> {
        snapshot
            .detail(&target.cluster)
            .and_then(|detail| detail.related(target))
            .map(|related| related.groups.iter().map(|g| g.title.clone()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn opening_the_sample_deployment_relates_its_pods_service_and_config() {
        let mut snapshot = crate::snapshot();
        let target = checkout();
        assert!(handle(
            &Command::OpenResource(target.clone()),
            &mut snapshot
        ));
        assert_eq!(
            titles(&snapshot, &target),
            [
                "ReplicaSets",
                "Pods",
                "Services",
                "Ingresses",
                "ConfigMaps",
                "Secrets"
            ]
        );
        assert!(!handle(&Command::ToggleSidebar, &mut snapshot));
    }

    #[test]
    fn opening_a_namespace_lists_its_contents() {
        let mut snapshot = crate::snapshot();
        let target = ResourceRef {
            cluster: ClusterId::new("prod-us-east"),
            kind: NAMESPACE.into(),
            namespace: None,
            name: "payments".into(),
        };
        assert!(handle(
            &Command::OpenResource(target.clone()),
            &mut snapshot
        ));
        let related = snapshot
            .detail(&target.cluster)
            .and_then(|detail| detail.related(&target));
        let deployments = related
            .and_then(|related| related.groups.iter().find(|g| g.title == "Deployments"))
            .map(|group| group.total);
        assert!(deployments.is_some_and(|total| total > 0));
    }
}
