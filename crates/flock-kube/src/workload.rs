use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use flock_core::pods::PodEvent;
use flock_core::stream::Sink;
use flock_core::workload::{
    Commit, ContainerSpec, ContainerStatus, EnvResolution, EnvSource, EnvVar, Exit, Pod, Port,
    Probe, ProbeAction, Resources, Revision,
};
use flock_core::{Condition, ResourceRef, Uid, Workload, WorkloadKind};
use futures::{StreamExt, stream};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use k8s_openapi::api::apps::v1::{
    ControllerRevision, DaemonSet, Deployment, ReplicaSet, StatefulSet,
};
use k8s_openapi::api::core::v1::{self as core, ConfigMap, PodTemplateSpec, Secret};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::ListParams;
use kube::{Api, Client};

use crate::clock::{age, since};
use crate::conditions::conditions;
use crate::detail;
use crate::manager::ClusterHandle;
use crate::metrics::PodUsage;
use crate::problems::{OwnerKey, owner_of};
use crate::resources::WatchKind;

const CPU: &str = "cpu";
const MEMORY: &str = "memory";
const REVISION_ANNOTATION: &str = "deployment.kubernetes.io/revision";
const CHANGE_CAUSE: &str = "kubernetes.io/change-cause";
const REVISION_HASH_LABEL: &str = "controller-revision-hash";
const TEMPLATE_IMAGE: &str = "/spec/template/spec/containers/0/image";
const DEFAULT_REPLICAS: i32 = 1;
const DEFAULT_PERIOD_SECS: i32 = 10;
const DEFAULT_PROTOCOL: &str = "TCP";
const SHA_MIN: usize = 7;
const KEY_LOOKUP_LIMIT: usize = 8;
const SHA_MAX: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RefKind {
    ConfigMap,
    Secret,
}

pub type Keys = HashMap<(RefKind, String), Option<BTreeSet<String>>>;

fn owner_key(target: &ResourceRef) -> OwnerKey {
    OwnerKey {
        namespace: target.namespace.clone().unwrap_or_default(),
        kind: target.kind.clone(),
        name: target.name.clone(),
    }
}

pub fn managed_by(target: &ResourceRef, meta: &ObjectMeta) -> Option<ResourceRef> {
    let labels = meta.labels.as_ref()?;
    flock_core::flux::managed_by(&target.cluster, labels)
}

fn quantity(map: Option<&BTreeMap<String, Quantity>>, key: &str) -> Option<String> {
    map?.get(key).map(|q| q.0.clone())
}

fn resources(map: Option<&BTreeMap<String, Quantity>>) -> Resources {
    Resources {
        cpu: quantity(map, CPU),
        memory: quantity(map, MEMORY),
    }
}

fn int_port(port: &IntOrString) -> Option<u16> {
    match port {
        IntOrString::Int(number) => u16::try_from(*number).ok(),
        IntOrString::String(_) => None,
    }
}

fn probe(probe: Option<&core::Probe>) -> Option<Probe> {
    let probe = probe?;
    let action = if let Some(http) = &probe.http_get {
        ProbeAction::HttpGet {
            port: int_port(&http.port)?,
            path: http.path.clone().unwrap_or_default(),
        }
    } else if let Some(tcp) = &probe.tcp_socket {
        ProbeAction::TcpSocket {
            port: int_port(&tcp.port)?,
        }
    } else {
        let command = probe.exec.as_ref()?.command.clone().unwrap_or_default();
        ProbeAction::Exec {
            command: command.join(" "),
        }
    };
    let period = probe.period_seconds.unwrap_or(DEFAULT_PERIOD_SECS);
    Some(Probe {
        action,
        period: Duration::from_secs(u64::try_from(period).unwrap_or_default()),
    })
}

fn resolve(keys: &Keys, kind: RefKind, name: &str, key: &str, optional: bool) -> EnvResolution {
    match keys.get(&(kind, name.to_owned())) {
        Some(Some(present)) if present.contains(key) => EnvResolution::Resolved,
        Some(Some(present)) => EnvResolution::MissingKey {
            optional,
            keys: present.iter().cloned().collect(),
        },
        Some(None) => EnvResolution::MissingObject { optional },
        None => EnvResolution::Resolved,
    }
}

fn env_var(var: &core::EnvVar, keys: &Keys) -> EnvVar {
    let from = var.value_from.as_ref();
    let (source, resolution) = if let Some(secret) = from.and_then(|f| f.secret_key_ref.as_ref()) {
        let optional = secret.optional.unwrap_or_default();
        let resolution = resolve(keys, RefKind::Secret, &secret.name, &secret.key, optional);
        let source = EnvSource::SecretKey {
            name: secret.name.clone(),
            key: secret.key.clone(),
        };
        (source, resolution)
    } else if let Some(map) = from.and_then(|f| f.config_map_key_ref.as_ref()) {
        let optional = map.optional.unwrap_or_default();
        let resolution = resolve(keys, RefKind::ConfigMap, &map.name, &map.key, optional);
        let source = EnvSource::ConfigMapKey {
            name: map.name.clone(),
            key: map.key.clone(),
        };
        (source, resolution)
    } else if let Some(field) = from.and_then(|f| f.field_ref.as_ref()) {
        (
            EnvSource::Field(field.field_path.clone()),
            EnvResolution::Literal,
        )
    } else {
        let value = var.value.clone().unwrap_or_default();
        (EnvSource::Value(value), EnvResolution::Literal)
    };
    EnvVar {
        name: var.name.clone(),
        source,
        resolution,
    }
}

pub fn env_refs(template: Option<&PodTemplateSpec>) -> BTreeSet<(RefKind, String)> {
    template
        .and_then(|t| t.spec.as_ref())
        .into_iter()
        .flat_map(|spec| spec.containers.iter())
        .flat_map(|c| c.env.iter().flatten())
        .filter_map(|var| {
            let from = var.value_from.as_ref()?;
            if let Some(secret) = &from.secret_key_ref {
                return Some((RefKind::Secret, secret.name.clone()));
            }
            from.config_map_key_ref
                .as_ref()
                .map(|map| (RefKind::ConfigMap, map.name.clone()))
        })
        .collect()
}

pub fn containers(template: Option<&PodTemplateSpec>, keys: &Keys) -> Vec<ContainerSpec> {
    template
        .and_then(|t| t.spec.as_ref())
        .into_iter()
        .flat_map(|spec| spec.containers.iter())
        .map(|c| {
            let declared = c.resources.as_ref();
            ContainerSpec {
                name: c.name.clone(),
                image: c.image.clone().unwrap_or_default(),
                ports: c
                    .ports
                    .iter()
                    .flatten()
                    .filter_map(|p| {
                        Some(Port {
                            port: u16::try_from(p.container_port).ok()?,
                            protocol: p
                                .protocol
                                .clone()
                                .unwrap_or_else(|| DEFAULT_PROTOCOL.into()),
                            name: p.name.clone(),
                        })
                    })
                    .collect(),
                requests: resources(declared.and_then(|r| r.requests.as_ref())),
                limits: resources(declared.and_then(|r| r.limits.as_ref())),
                readiness: probe(c.readiness_probe.as_ref()),
                env: c.env.iter().flatten().map(|v| env_var(v, keys)).collect(),
            }
        })
        .collect()
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn whole(value: f64) -> u64 {
    value.round().max(0.) as u64
}

pub fn pods(
    target: &ResourceRef,
    raw: &[&core::Pod],
    usage: &[PodUsage],
    now: Timestamp,
) -> Vec<Pod> {
    let zone = TimeZone::system();
    let clock = crate::pods::Clock { now, zone: &zone };
    let used: HashMap<&str, &PodUsage> = usage.iter().map(|u| (u.name.as_str(), u)).collect();
    let mut rows: Vec<Pod> = raw
        .iter()
        .map(|pod| {
            let name = pod.metadata.name.clone().unwrap_or_default();
            let view = crate::pods::pod(&target.cluster, pod, None, Vec::new(), &clock);
            let usage = used.get(name.as_str());
            let containers = pod
                .status
                .as_ref()
                .and_then(|s| s.container_statuses.as_ref())
                .into_iter()
                .flatten()
                .map(|c| ContainerStatus {
                    name: c.name.clone(),
                    ready: c.ready,
                    restarts: u32::try_from(c.restart_count).unwrap_or_default(),
                    last_exit: c
                        .last_state
                        .as_ref()
                        .and_then(|l| l.terminated.as_ref())
                        .map(|t| Exit {
                            code: t.exit_code,
                            reason: t.reason.clone().unwrap_or_default(),
                            ago: since(t.finished_at.as_ref(), now),
                        }),
                })
                .collect();
            Pod {
                phase: view.reason,
                status: view.status,
                node: view.node.unwrap_or_default(),
                cpu_millis: usage
                    .map(|u| u32::try_from(whole(u.usage.cpu_millicores)).unwrap_or(u32::MAX)),
                memory_bytes: usage.map(|u| whole(u.usage.memory_bytes)),
                age: age(&pod.metadata, now),
                containers,
                name,
            }
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows
}

fn commit(cause: Option<&String>) -> Option<Commit> {
    let cause = cause?.trim();
    let (head, rest) = cause.split_once(' ').unwrap_or((cause, ""));
    let hex =
        (SHA_MIN..=SHA_MAX).contains(&head.len()) && head.chars().all(|c| c.is_ascii_hexdigit());
    Some(if hex {
        Commit {
            sha: head.to_owned(),
            subject: rest.trim().to_owned(),
        }
    } else {
        Commit {
            sha: String::new(),
            subject: cause.to_owned(),
        }
    })
}

fn annotation<'a>(meta: &'a ObjectMeta, key: &str) -> Option<&'a String> {
    meta.annotations.as_ref()?.get(key)
}

fn owned_by(meta: &ObjectMeta, kind: &str, name: &str) -> bool {
    meta.owner_references
        .iter()
        .flatten()
        .any(|o| o.kind == kind && o.name == name)
}

pub fn replica_set_revisions(
    deployment: &Deployment,
    sets: &[Arc<ReplicaSet>],
    now: Timestamp,
) -> Vec<Revision> {
    let name = deployment.metadata.name.as_deref().unwrap_or_default();
    let current = annotation(&deployment.metadata, REVISION_ANNOTATION);
    let mut rows: Vec<Revision> = sets
        .iter()
        .filter(|rs| owned_by(&rs.metadata, WorkloadKind::Deployment.kind(), name))
        .filter_map(|rs| {
            let number = annotation(&rs.metadata, REVISION_ANNOTATION)?
                .parse()
                .ok()?;
            let status = rs.status.as_ref();
            let template = rs.spec.as_ref().and_then(|s| s.template.as_ref());
            Some(Revision {
                number,
                owner: rs.metadata.name.clone().unwrap_or_default(),
                image: first_image(template),
                ready: status
                    .and_then(|s| s.ready_replicas)
                    .and_then(|n| u32::try_from(n).ok())
                    .unwrap_or_default(),
                replicas: status
                    .and_then(|s| u32::try_from(s.replicas).ok())
                    .unwrap_or_default(),
                age: age(&rs.metadata, now),
                commit: commit(annotation(&rs.metadata, CHANGE_CAUSE)),
                current: current
                    .and_then(|c| c.parse::<u64>().ok())
                    .is_some_and(|c| c == number),
            })
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.number));
    rows
}

fn first_image(template: Option<&PodTemplateSpec>) -> String {
    template
        .and_then(|t| t.spec.as_ref())
        .and_then(|s| s.containers.first())
        .and_then(|c| c.image.clone())
        .unwrap_or_default()
}

pub fn controller_revisions(
    kind: WorkloadKind,
    name: &str,
    current: Option<&str>,
    revisions: &[ControllerRevision],
    pods: &[&core::Pod],
    now: Timestamp,
) -> Vec<Revision> {
    let mut owned: Vec<&ControllerRevision> = revisions
        .iter()
        .filter(|r| owned_by(&r.metadata, kind.kind(), name))
        .collect();
    owned.sort_by_key(|r| std::cmp::Reverse(r.revision));
    let newest = owned.first().and_then(|r| r.metadata.name.clone());
    let current = current.map(str::to_owned).or(newest);
    owned
        .into_iter()
        .map(|r| {
            let revision_name = r.metadata.name.clone().unwrap_or_default();
            let hash = revision_name
                .strip_prefix(&format!("{name}-"))
                .unwrap_or(&revision_name)
                .to_owned();
            let matching: Vec<&&core::Pod> = pods
                .iter()
                .filter(|p| {
                    let label = p
                        .metadata
                        .labels
                        .as_ref()
                        .and_then(|l| l.get(REVISION_HASH_LABEL));
                    label.is_some_and(|l| *l == revision_name || *l == hash)
                })
                .collect();
            let ready = matching
                .iter()
                .filter(|p| crate::problems::pod_ready(p))
                .count();
            Revision {
                number: u64::try_from(r.revision).unwrap_or_default(),
                image: r
                    .data
                    .as_ref()
                    .and_then(|d| d.0.pointer(TEMPLATE_IMAGE))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                ready: u32::try_from(ready).unwrap_or(u32::MAX),
                replicas: u32::try_from(matching.len()).unwrap_or(u32::MAX),
                age: age(&r.metadata, now),
                commit: commit(annotation(&r.metadata, CHANGE_CAUSE)),
                current: current.as_deref() == Some(revision_name.as_str()),
                owner: revision_name,
            }
        })
        .collect()
}

pub fn events(
    target: &ResourceRef,
    owners: &BTreeSet<String>,
    pods: &BTreeSet<String>,
    raw: &[Arc<core::Event>],
    now: Timestamp,
) -> Vec<PodEvent> {
    let namespace = target.namespace.as_deref();
    let mut rows: Vec<PodEvent> = raw
        .iter()
        .filter(|e| e.involved_object.namespace.as_deref() == namespace)
        .filter(|e| {
            let name = e.involved_object.name.as_deref().unwrap_or_default();
            let kind = e.involved_object.kind.as_deref().unwrap_or_default();
            (kind == target.kind && name == target.name)
                || (kind == "ReplicaSet" && owners.contains(name))
                || (kind == "Pod" && pods.contains(name))
        })
        .map(|e| {
            let row = detail::event(e, now);
            PodEvent {
                age: row.age,
                status: row.status,
                reason: row.reason,
                message: row.message,
                count: row.count,
            }
        })
        .collect();
    rows.sort_by_key(|e| e.age);
    rows
}

fn names<T: kube::Resource>(items: &[&T]) -> BTreeSet<String> {
    items.iter().filter_map(|i| i.meta().name.clone()).collect()
}

pub fn watch_kinds(kind: WorkloadKind) -> Vec<WatchKind> {
    let own = match kind {
        WorkloadKind::Deployment => WatchKind::Deployments,
        WorkloadKind::StatefulSet => WatchKind::StatefulSets,
        WorkloadKind::DaemonSet => WatchKind::DaemonSets,
    };
    let mut kinds = vec![own, WatchKind::Events];
    if kind == WorkloadKind::Deployment {
        kinds.push(WatchKind::ReplicaSets);
    }
    kinds
}

async fn secret_keys(
    client: Client,
    namespace: &str,
    name: &str,
) -> Option<Option<BTreeSet<String>>> {
    let api: Api<Secret> = Api::namespaced(client, namespace);
    let secret = api.get_opt(name).await.ok()?;
    Some(secret.map(|s| {
        let data = s.data.unwrap_or_default().into_keys();
        let strings = s.string_data.unwrap_or_default().into_keys();
        data.chain(strings).collect()
    }))
}

async fn config_map_keys(
    client: Client,
    namespace: &str,
    name: &str,
) -> Option<Option<BTreeSet<String>>> {
    let api: Api<ConfigMap> = Api::namespaced(client, namespace);
    let map = api.get_opt(name).await.ok()?;
    Some(map.map(|m| {
        let data = m.data.unwrap_or_default().into_keys();
        let binary = m.binary_data.unwrap_or_default().into_keys();
        data.chain(binary).collect()
    }))
}

async fn keys(handle: &ClusterHandle, namespace: &str, refs: BTreeSet<(RefKind, String)>) -> Keys {
    let Ok(client) = handle.client() else {
        return Keys::new();
    };
    let lookups = refs.into_iter().map(|(kind, name)| {
        let client = client.clone();
        async move {
            let present = match kind {
                RefKind::Secret => secret_keys(client, namespace, &name).await,
                RefKind::ConfigMap => config_map_keys(client, namespace, &name).await,
            };
            present.map(|present| ((kind, name), present))
        }
    });
    stream::iter(lookups)
        .buffer_unordered(KEY_LOOKUP_LIMIT)
        .filter_map(std::future::ready)
        .collect()
        .await
}

struct Spec {
    meta: ObjectMeta,
    template: Option<PodTemplateSpec>,
    desired: u32,
    conditions: Vec<Condition>,
    current_revision: Option<String>,
}

fn find<K: kube::Resource + Clone>(items: Vec<Arc<K>>, target: &ResourceRef) -> Option<Arc<K>> {
    items.into_iter().find(|i| {
        i.meta().name.as_deref() == Some(target.name.as_str())
            && i.meta().namespace == target.namespace
    })
}

fn count(value: i32) -> u32 {
    u32::try_from(value).unwrap_or_default()
}

fn spec(
    handle: &ClusterHandle,
    kind: WorkloadKind,
    target: &ResourceRef,
    now: Timestamp,
) -> Option<Spec> {
    match kind {
        WorkloadKind::Deployment => {
            let item = find(handle.objects::<Deployment>(), target)?;
            let spec = item.spec.as_ref();
            let status = item.status.as_ref();
            Some(Spec {
                meta: item.metadata.clone(),
                template: spec.map(|s| s.template.clone()),
                desired: count(spec.and_then(|s| s.replicas).unwrap_or(DEFAULT_REPLICAS)),
                conditions: conditions(status.and_then(|s| s.conditions.as_ref()), now),
                current_revision: None,
            })
        }
        WorkloadKind::StatefulSet => {
            let item = find(handle.objects::<StatefulSet>(), target)?;
            let spec = item.spec.as_ref();
            let status = item.status.as_ref();
            Some(Spec {
                meta: item.metadata.clone(),
                template: spec.map(|s| s.template.clone()),
                desired: count(spec.and_then(|s| s.replicas).unwrap_or(DEFAULT_REPLICAS)),
                conditions: conditions(status.and_then(|s| s.conditions.as_ref()), now),
                current_revision: status.and_then(|s| s.update_revision.clone()),
            })
        }
        WorkloadKind::DaemonSet => {
            let item = find(handle.objects::<DaemonSet>(), target)?;
            let status = item.status.as_ref();
            Some(Spec {
                meta: item.metadata.clone(),
                template: item.spec.as_ref().map(|s| s.template.clone()),
                desired: count(
                    status
                        .map(|s| s.desired_number_scheduled)
                        .unwrap_or_default(),
                ),
                conditions: conditions(status.and_then(|s| s.conditions.as_ref()), now),
                current_revision: None,
            })
        }
    }
}

pub async fn build(
    handle: &ClusterHandle,
    target: &ResourceRef,
    now: Timestamp,
) -> Option<Workload> {
    let kind = WorkloadKind::from_kind(&target.kind)?;
    let namespace = target.namespace.clone()?;
    let spec = spec(handle, kind, target, now)?;
    let all_pods = handle.objects::<core::Pod>();
    let owner = owner_key(target);
    let raw: Vec<&core::Pod> = all_pods
        .iter()
        .map(AsRef::as_ref)
        .filter(|p| p.metadata.namespace.as_deref() == Some(namespace.as_str()))
        .filter(|p| owner_of(p) == owner)
        .collect();
    let history = async {
        match kind {
            WorkloadKind::Deployment => find(handle.objects::<Deployment>(), target)
                .map(|deployment| {
                    replica_set_revisions(&deployment, &handle.objects::<ReplicaSet>(), now)
                })
                .unwrap_or_default(),
            WorkloadKind::StatefulSet => {
                let current = spec.current_revision.as_deref();
                controller_history(handle, kind, target, current, &raw, now).await
            }
            WorkloadKind::DaemonSet => {
                controller_history(handle, kind, target, None, &raw, now).await
            }
        }
    };
    let (revisions, keys, usage) = tokio::join!(
        history,
        keys(handle, &namespace, env_refs(spec.template.as_ref())),
        handle.pod_usage(Some(&namespace)),
    );
    let usage = usage.unwrap_or_default();
    let history_pending = kind == WorkloadKind::Deployment
        && !handle.watch_state(&WatchKind::ReplicaSets).is_loaded();
    let owners: BTreeSet<String> = revisions.iter().map(|r| r.owner.clone()).collect();
    let pod_names = names(&raw);
    let events = events(
        target,
        &owners,
        &pod_names,
        &handle.objects::<core::Event>(),
        now,
    );
    Some(Workload {
        target: target.clone(),
        uid: Uid::new(spec.meta.uid.clone().unwrap_or_default()),
        kind,
        desired: spec.desired,
        conditions: spec.conditions,
        pending: None,
        managed_by: managed_by(target, &spec.meta),
        pods: pods(target, &raw, &usage, now),
        revisions,
        history_pending,
        containers: containers(spec.template.as_ref(), &keys),
        events,
    })
}

async fn controller_history(
    handle: &ClusterHandle,
    kind: WorkloadKind,
    target: &ResourceRef,
    current: Option<&str>,
    pods: &[&core::Pod],
    now: Timestamp,
) -> Vec<Revision> {
    let Ok(client) = handle.client() else {
        return Vec::new();
    };
    let namespace = target.namespace.clone().unwrap_or_default();
    let api: Api<ControllerRevision> = Api::namespaced(client, &namespace);
    let listed = api.list(&ListParams::default()).await;
    let items = listed.map(|l| l.items).unwrap_or_default();
    controller_revisions(kind, &target.name, current, &items, pods, now)
}

pub async fn watch(
    handle: ClusterHandle,
    target: ResourceRef,
    sink: Sink<Workload>,
    refresh: Duration,
) {
    let Some(kind) = WorkloadKind::from_kind(&target.kind) else {
        return;
    };
    let _subscriptions: Vec<_> = watch_kinds(kind)
        .into_iter()
        .map(|kind| handle.subscribe(kind))
        .collect();
    let mut last: Option<Workload> = None;
    while !sink.is_closed() {
        let built = build(&handle, &target, Timestamp::now()).await;
        if let Some(workload) = built
            && last.as_ref() != Some(&workload)
        {
            if !sink.send_blocking(workload.clone()) {
                return;
            }
            last = Some(workload);
        }
        tokio::time::sleep(refresh).await;
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::problems::tests::{at, crash_status, now, parse, ready_status, replica_pod};

    const ROUND_TRIP: Duration = Duration::from_secs(1);
    const SECRET: &str =
        r#"{"apiVersion":"v1","kind":"Secret","metadata":{"name":"s"},"data":{"token":"eA=="}}"#;

    #[tokio::test(start_paused = true)]
    async fn env_sources_are_looked_up_together_up_to_the_limit() {
        let (client, traffic) = crate::fake::client(ROUND_TRIP, |_, path| {
            path.contains("/secrets/")
                .then(|| (http::StatusCode::OK, SECRET.to_owned()))
        });
        let (_manager, handle) = crate::fake::handle(&client);
        let refs: BTreeSet<(RefKind, String)> = (0..KEY_LOOKUP_LIMIT)
            .map(|ix| (RefKind::Secret, format!("secret-{ix}")))
            .chain(std::iter::once((RefKind::ConfigMap, "missing".to_owned())))
            .collect();
        let started = tokio::time::Instant::now();
        let found = keys(&handle, "apps", refs).await;
        assert!(
            started.elapsed() <= ROUND_TRIP * 2,
            "{:?}",
            started.elapsed()
        );
        assert!(traffic.peak() >= KEY_LOOKUP_LIMIT);
        assert_eq!(found.len(), KEY_LOOKUP_LIMIT + 1);
        assert_eq!(
            found.get(&(RefKind::Secret, "secret-0".to_owned())),
            Some(&Some(BTreeSet::from(["token".to_owned()])))
        );
        assert_eq!(
            found.get(&(RefKind::ConfigMap, "missing".to_owned())),
            Some(&None)
        );
    }

    fn target(kind: &str, name: &str) -> ResourceRef {
        ResourceRef {
            cluster: flock_core::ClusterId::new("fleet-a"),
            kind: kind.into(),
            namespace: Some("payments".into()),
            name: name.into(),
        }
    }

    fn template() -> PodTemplateSpec {
        match serde_json::from_value(json!({
            "spec": { "containers": [{
                "name": "webhook", "image": "ghcr.io/stefanprodan/podinfo:latest",
                "ports": [{ "containerPort": 9898, "name": "http" }],
                "resources": { "requests": { "cpu": "100m", "memory": "64Mi" } },
                "readinessProbe": { "httpGet": { "path": "/readyz", "port": 9898 }, "periodSeconds": 5 },
                "env": [
                    { "name": "MODE", "value": "live" },
                    { "name": "STRIPE_WEBHOOK_SECRET", "valueFrom": { "secretKeyRef": { "name": "stripe", "key": "webhook_secret" } } },
                    { "name": "FLAGS", "valueFrom": { "configMapKeyRef": { "name": "flags", "key": "all", "optional": true } } }
                ]
            }] }
        })) {
            Ok(template) => template,
            Err(error) => panic!("fixture: {error}"),
        }
    }

    #[test]
    fn env_reports_the_missing_secret_key() {
        let mut keys = Keys::new();
        keys.insert(
            (RefKind::Secret, "stripe".into()),
            Some(BTreeSet::from(["webhook-secret".to_owned()])),
        );
        keys.insert((RefKind::ConfigMap, "flags".into()), None);
        let template = template();
        assert_eq!(
            env_refs(Some(&template)),
            BTreeSet::from([
                (RefKind::ConfigMap, "flags".to_owned()),
                (RefKind::Secret, "stripe".to_owned())
            ])
        );
        let specs = containers(Some(&template), &keys);
        let spec = &specs[0];
        assert_eq!(spec.requests.label(), "CPU 100m · memory 64Mi");
        assert_eq!(
            spec.readiness.as_ref().map(Probe::label).as_deref(),
            Some("GET :9898/readyz · 5s")
        );
        let resolutions: Vec<&EnvResolution> = spec.env.iter().map(|v| &v.resolution).collect();
        assert_eq!(
            resolutions,
            [
                &EnvResolution::Literal,
                &EnvResolution::MissingKey {
                    optional: false,
                    keys: vec!["webhook-secret".into()]
                },
                &EnvResolution::MissingObject { optional: true },
            ]
        );
        assert_eq!(
            spec.env[1].note().as_deref(),
            Some("key webhook_secret not in secret · optional: false · has webhook-secret")
        );
    }

    #[test]
    fn deployment_revisions_come_from_its_replica_sets() {
        let deployment: Arc<Deployment> = parse(json!({
            "metadata": {
                "name": "checkout-api", "namespace": "payments",
                "annotations": { "deployment.kubernetes.io/revision": "2" },
                "labels": { "helm.toolkit.fluxcd.io/name": "checkout-api", "helm.toolkit.fluxcd.io/namespace": "payments" }
            },
            "spec": { "selector": {}, "template": {} }
        }));
        let set = |hash: &str,
                   revision: &str,
                   replicas: i32,
                   cause: Option<&str>|
         -> Arc<ReplicaSet> {
            let mut annotations = json!({ "deployment.kubernetes.io/revision": revision });
            if let Some(cause) = cause {
                annotations["kubernetes.io/change-cause"] = json!(cause);
            }
            parse(json!({
                "metadata": {
                    "name": format!("checkout-api-{hash}"), "namespace": "payments", "creationTimestamp": at(60),
                    "annotations": annotations,
                    "ownerReferences": [{ "apiVersion": "apps/v1", "kind": "Deployment", "name": "checkout-api", "uid": "u", "controller": true }]
                },
                "spec": { "selector": {}, "template": { "spec": { "containers": [{ "name": "api", "image": format!("podinfo:{revision}") }] } } },
                "status": { "replicas": replicas, "readyReplicas": 0 }
            }))
        };
        let sets = [
            set("aaa", "1", 0, None),
            set("bbb", "2", 1, Some("a41e07c bump to 2.14.0")),
        ];
        let revisions = replica_set_revisions(&deployment, &sets, now());
        let summary: Vec<(u64, bool, String)> = revisions
            .iter()
            .map(|r| (r.number, r.current, r.pods_label()))
            .collect();
        assert_eq!(summary, [(2, true, "0/1".into()), (1, false, "0".into())]);
        assert_eq!(
            revisions[0].commit,
            Some(Commit {
                sha: "a41e07c".into(),
                subject: "bump to 2.14.0".into()
            })
        );
        let target = target("Deployment", "checkout-api");
        assert_eq!(
            managed_by(&target, &deployment.metadata).map(|r| r.kind),
            Some("HelmRelease".into())
        );
    }

    #[test]
    fn workload_pods_and_events_follow_its_replicas() {
        let crash = replica_pod(
            "checkout-api-7f9c-x2lqp",
            "checkout-api",
            "7f9c",
            &crash_status(),
        );
        let ok = replica_pod(
            "checkout-api-7f9c-m8vtn",
            "checkout-api",
            "7f9c",
            &ready_status(),
        );
        let raw = [crash.as_ref(), ok.as_ref()];
        let target = target("Deployment", "checkout-api");
        let rows = pods(&target, &raw, &[], now());
        let phases: Vec<&str> = rows.iter().map(|p| p.phase.as_str()).collect();
        assert_eq!(phases, ["Running", "CrashLoopBackOff"]);
        let exit = rows[1].containers[0].last_exit.as_ref().map(|e| e.code);
        assert_eq!(exit, Some(1));
        let event = |kind: &str, name: &str, minutes: i64| -> Arc<core::Event> {
            parse(json!({
                "metadata": { "name": format!("{name}.1"), "namespace": "payments" },
                "involvedObject": { "kind": kind, "name": name, "namespace": "payments" },
                "reason": "BackOff", "type": "Warning", "lastTimestamp": at(minutes)
            }))
        };
        let raw_events = [
            event("Pod", "checkout-api-7f9c-x2lqp", 2),
            event("Deployment", "checkout-api", 30),
            event("Pod", "someone-else", 1),
        ];
        let rows = events(&target, &BTreeSet::new(), &names(&raw), &raw_events, now());
        let ages: Vec<Duration> = rows.iter().map(|e| e.age).collect();
        assert_eq!(ages, [Duration::from_mins(2), Duration::from_mins(30)]);
    }
}
