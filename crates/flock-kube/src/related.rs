use std::collections::BTreeMap;
use std::sync::Arc;

use flock_core::events::Event as CoreEvent;
use flock_core::nodes::Node as CoreNode;
use flock_core::related::{
    self, CLAIM, CONFIG_MAP, Fact, Key, NAMESPACE, NODE, PERSISTENT_VOLUME, POD, Related, SECRET,
    SERVICE,
};
use flock_core::resources::{Resource, ResourceKind};
use flock_core::{ClusterId, ResourceRef};
use jiff::Timestamp;
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use k8s_openapi::api::batch::v1::{CronJob, Job};
use k8s_openapi::api::core::v1::{
    ConfigMap, PersistentVolumeClaim, Pod, PodSpec, PodTemplateSpec, Secret, Service,
};
use k8s_openapi::api::networking::v1::{Ingress, IngressBackend};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::PartialObjectMeta;

use crate::clock::age;
use crate::detail::PodIndex;
use crate::lists;
use crate::manager::ClusterHandle;
use crate::pods::health_status;
use crate::snapshot::{PodHealth, pod_health};

pub struct Scope<'a> {
    pub cluster: &'a ClusterId,
    pub nodes: &'a [CoreNode],
    pub events: &'a [CoreEvent],
    pub index: &'a PodIndex,
    pub now: Timestamp,
}

pub(crate) fn owner(meta: &ObjectMeta) -> Option<Key> {
    meta.owner_references
        .iter()
        .flatten()
        .find(|reference| reference.controller == Some(true))
        .map(|reference| Key::new(reference.kind.clone(), reference.name.clone()))
}

fn push(uses: &mut Vec<Key>, kind: &str, name: &str) {
    let key = Key::new(kind, name);
    if !name.is_empty() && !uses.contains(&key) {
        uses.push(key);
    }
}

pub fn spec_uses(spec: &PodSpec) -> Vec<Key> {
    let mut uses = Vec::new();
    for volume in spec.volumes.iter().flatten() {
        if let Some(map) = &volume.config_map {
            push(&mut uses, CONFIG_MAP, &map.name);
        }
        if let Some(name) = volume
            .secret
            .as_ref()
            .and_then(|s| s.secret_name.as_deref())
        {
            push(&mut uses, SECRET, name);
        }
        if let Some(claim) = &volume.persistent_volume_claim {
            push(&mut uses, CLAIM, &claim.claim_name);
        }
        let sources = volume.projected.as_ref().and_then(|p| p.sources.as_ref());
        for source in sources.into_iter().flatten() {
            if let Some(map) = &source.config_map {
                push(&mut uses, CONFIG_MAP, &map.name);
            }
            if let Some(secret) = &source.secret {
                push(&mut uses, SECRET, &secret.name);
            }
        }
    }
    let containers = spec
        .init_containers
        .iter()
        .flatten()
        .chain(spec.containers.iter());
    for container in containers {
        for env in container.env.iter().flatten() {
            let source = env.value_from.as_ref();
            if let Some(map) = source.and_then(|s| s.config_map_key_ref.as_ref()) {
                push(&mut uses, CONFIG_MAP, &map.name);
            }
            if let Some(secret) = source.and_then(|s| s.secret_key_ref.as_ref()) {
                push(&mut uses, SECRET, &secret.name);
            }
        }
        for from in container.env_from.iter().flatten() {
            if let Some(map) = &from.config_map_ref {
                push(&mut uses, CONFIG_MAP, &map.name);
            }
            if let Some(secret) = &from.secret_ref {
                push(&mut uses, SECRET, &secret.name);
            }
        }
    }
    for pull in spec.image_pull_secrets.iter().flatten() {
        push(&mut uses, SECRET, &pull.name);
    }
    uses
}

fn template_facts(template: Option<&PodTemplateSpec>) -> (BTreeMap<String, String>, Vec<Key>) {
    let labels = template
        .and_then(|t| t.metadata.as_ref())
        .and_then(|m| m.labels.clone())
        .unwrap_or_default();
    let uses = template
        .and_then(|t| t.spec.as_ref())
        .map(spec_uses)
        .unwrap_or_default();
    (labels, uses)
}

fn row_fact(kind: ResourceKind, row: &Resource, meta: &ObjectMeta) -> Fact {
    Fact {
        status: row.status,
        note: kind.status_cell(row).unwrap_or_default(),
        age: row.age,
        labels: row.labels.clone(),
        owner: owner(meta),
        ..Fact::new(kind.name(), Some(&row.namespace), &row.name)
    }
}

fn workload_fact(
    kind: ResourceKind,
    row: &Resource,
    meta: &ObjectMeta,
    template: Option<&PodTemplateSpec>,
) -> Fact {
    let (template, uses) = template_facts(template);
    Fact {
        template,
        uses,
        ..row_fact(kind, row, meta)
    }
}

pub fn pod(item: &Pod, now: Timestamp) -> Fact {
    let meta = &item.metadata;
    let health = pod_health(item);
    let status = item.status.as_ref();
    let waiting = status
        .and_then(|s| s.container_statuses.as_ref())
        .into_iter()
        .flatten()
        .find_map(|c| c.state.as_ref()?.waiting.as_ref()?.reason.clone());
    let phase = status.and_then(|s| s.phase.clone()).unwrap_or_default();
    let note = if health == PodHealth::Healthy {
        phase
    } else {
        waiting.unwrap_or(phase)
    };
    let spec = item.spec.as_ref();
    let mut uses = spec.map(spec_uses).unwrap_or_default();
    if let Some(node) = spec.and_then(|s| s.node_name.as_deref()) {
        push(&mut uses, NODE, node);
    }
    Fact {
        status: health_status(health),
        note,
        age: age(meta, now),
        labels: meta.labels.clone().unwrap_or_default(),
        owner: owner(meta),
        uses,
        ..Fact::new(
            POD,
            meta.namespace.as_deref(),
            meta.name.as_deref().unwrap_or_default(),
        )
    }
}

fn backend_service(backend: &IngressBackend) -> Option<&str> {
    backend
        .service
        .as_ref()
        .map(|service| service.name.as_str())
}

pub fn ingress_uses(item: &Ingress) -> Vec<Key> {
    let mut uses = Vec::new();
    let Some(spec) = item.spec.as_ref() else {
        return uses;
    };
    let default = spec.default_backend.as_ref().and_then(backend_service);
    let paths = spec
        .rules
        .iter()
        .flatten()
        .filter_map(|rule| rule.http.as_ref())
        .flat_map(|http| http.paths.iter())
        .filter_map(|path| backend_service(&path.backend));
    for service in default.into_iter().chain(paths) {
        push(&mut uses, SERVICE, service);
    }
    let secrets = spec
        .tls
        .iter()
        .flatten()
        .filter_map(|tls| tls.secret_name.as_deref());
    for secret in secrets {
        push(&mut uses, SECRET, secret);
    }
    uses
}

fn in_scope(meta: &ObjectMeta, namespace: Option<&str>) -> bool {
    namespace.is_none_or(|namespace| meta.namespace.as_deref() == Some(namespace))
}

fn collect<K>(
    items: &[Arc<K>],
    namespace: Option<&str>,
    meta: impl Fn(&K) -> &ObjectMeta,
    build: impl Fn(&K) -> Fact,
) -> Vec<Fact> {
    items
        .iter()
        .filter(|item| in_scope(meta(item), namespace))
        .map(|item| build(item))
        .collect()
}

pub fn facts(handle: &ClusterHandle, scope: &Scope<'_>, namespace: Option<&str>) -> Vec<Fact> {
    let now = scope.now;
    let mut facts = collect(
        &handle.objects::<Pod>(),
        namespace,
        |i| &i.metadata,
        |i| pod(i, now),
    );
    facts.extend(workload_facts(handle, scope, namespace));
    facts.extend(other_facts(handle, scope, namespace));
    facts.extend(scope.nodes.iter().map(|node| Fact {
        status: node.status,
        note: node.state.clone(),
        age: node.age,
        ..Fact::new(NODE, None, &node.name)
    }));
    facts
}

fn workload_facts(handle: &ClusterHandle, scope: &Scope<'_>, namespace: Option<&str>) -> Vec<Fact> {
    let now = scope.now;
    let index = scope.index;
    let mut facts = collect(
        &handle.objects::<Deployment>(),
        namespace,
        |i| &i.metadata,
        |i| {
            let row = lists::deployment(i, index, now);
            let template = i.spec.as_ref().map(|s| &s.template);
            workload_fact(ResourceKind::Deployment, &row, &i.metadata, template)
        },
    );
    facts.extend(collect(
        &handle.objects::<StatefulSet>(),
        namespace,
        |i| &i.metadata,
        |i| {
            let row = lists::stateful_set(i, index, now);
            let template = i.spec.as_ref().map(|s| &s.template);
            workload_fact(ResourceKind::StatefulSet, &row, &i.metadata, template)
        },
    ));
    facts.extend(collect(
        &handle.objects::<DaemonSet>(),
        namespace,
        |i| &i.metadata,
        |i| {
            let row = lists::daemon_set(i, index, now);
            let template = i.spec.as_ref().map(|s| &s.template);
            workload_fact(ResourceKind::DaemonSet, &row, &i.metadata, template)
        },
    ));
    facts.extend(collect(
        &handle.objects::<Job>(),
        namespace,
        |i| &i.metadata,
        |i| {
            let row = lists::job(i, index, now);
            let template = i.spec.as_ref().map(|s| &s.template);
            workload_fact(ResourceKind::Job, &row, &i.metadata, template)
        },
    ));
    facts.extend(collect(
        &handle.objects::<CronJob>(),
        namespace,
        |i| &i.metadata,
        |i| {
            let row = lists::cron_job(i, now);
            let template = i.spec.job_template.spec.as_ref().map(|s| &s.template);
            workload_fact(ResourceKind::CronJob, &row, &i.metadata, template)
        },
    ));
    facts
}

fn other_facts(handle: &ClusterHandle, scope: &Scope<'_>, namespace: Option<&str>) -> Vec<Fact> {
    let now = scope.now;
    let mut facts = collect(
        &handle.objects::<Service>(),
        namespace,
        |i| &i.metadata,
        |i| {
            let row = lists::service(i, now);
            let selector = i
                .spec
                .as_ref()
                .and_then(|s| s.selector.clone())
                .unwrap_or_default();
            Fact {
                selector,
                ..row_fact(ResourceKind::Service, &row, &i.metadata)
            }
        },
    );
    facts.extend(collect(
        &handle.objects::<Ingress>(),
        namespace,
        |i| &i.metadata,
        |i| {
            let row = lists::ingress(i, now);
            Fact {
                uses: ingress_uses(i),
                ..row_fact(ResourceKind::Ingress, &row, &i.metadata)
            }
        },
    ));
    facts.extend(collect(
        &handle.objects::<PartialObjectMeta<ConfigMap>>(),
        namespace,
        |i| &i.metadata,
        |i| {
            row_fact(
                ResourceKind::ConfigMap,
                &lists::config_map(i, now),
                &i.metadata,
            )
        },
    ));
    facts.extend(collect(
        &handle.objects::<PartialObjectMeta<Secret>>(),
        namespace,
        |i| &i.metadata,
        |i| row_fact(ResourceKind::Secret, &lists::secret(i, now), &i.metadata),
    ));
    facts.extend(collect(
        &handle.objects::<PersistentVolumeClaim>(),
        namespace,
        |i| &i.metadata,
        |i| {
            let row = lists::pvc(i, now);
            let mut uses = Vec::new();
            if let Some(volume) = i.spec.as_ref().and_then(|s| s.volume_name.as_deref()) {
                push(&mut uses, PERSISTENT_VOLUME, volume);
            }
            Fact {
                uses,
                ..row_fact(ResourceKind::PersistentVolumeClaim, &row, &i.metadata)
            }
        },
    ));
    facts
}

pub fn related(
    handle: &ClusterHandle,
    scope: &Scope<'_>,
    targets: &[&ResourceRef],
) -> Vec<Related> {
    let mut built: Vec<Related> = Vec::new();
    for target in targets {
        if built.iter().any(|related| &related.target == *target) {
            continue;
        }
        if target.kind == NAMESPACE {
            let facts = facts(handle, scope, Some(&target.name));
            let contents = related::contents(scope.cluster, &facts, scope.events, &target.name);
            built.push(contents);
            continue;
        }
        if flock_core::crd::CrdKey::parse(&target.kind).is_some() {
            built.extend(crate::crd::related(handle, scope, target));
            continue;
        }
        let facts = facts(handle, scope, target.namespace.as_deref());
        built.push(related::related(scope.cluster, &facts, target));
    }
    built
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::problems::tests::{now, parse};

    #[test]
    fn pod_specs_reference_their_volumes_env_and_pull_secrets() {
        let pod: Arc<Pod> = parse(json!({
            "metadata": {
                "name": "cart-7f9c-a", "namespace": "shop",
                "labels": { "app": "cart", "pod-template-hash": "7f9c" },
                "ownerReferences": [{ "apiVersion": "apps/v1", "kind": "ReplicaSet", "name": "cart-7f9c", "uid": "u", "controller": true }]
            },
            "spec": {
                "nodeName": "node-a",
                "imagePullSecrets": [{ "name": "registry" }],
                "volumes": [
                    { "name": "config", "configMap": { "name": "cart-config" } },
                    { "name": "certs", "secret": { "secretName": "cart-tls" } },
                    { "name": "data", "persistentVolumeClaim": { "claimName": "cart-data" } },
                    { "name": "bundle", "projected": { "sources": [{ "configMap": { "name": "ca-bundle" } }, { "secret": { "name": "cart-tls" } }] } }
                ],
                "containers": [{
                    "name": "api",
                    "env": [
                        { "name": "TOKEN", "valueFrom": { "secretKeyRef": { "name": "cart-env", "key": "token" } } },
                        { "name": "MODE", "valueFrom": { "configMapKeyRef": { "name": "cart-config", "key": "mode" } } }
                    ],
                    "envFrom": [{ "configMapRef": { "name": "shared" } }, { "secretRef": { "name": "cart-env" } }]
                }]
            },
            "status": { "phase": "Running" }
        }));
        let fact = super::pod(&pod, now());
        let uses: Vec<(&str, &str)> = fact
            .uses
            .iter()
            .map(|key| (key.kind.as_str(), key.name.as_str()))
            .collect();
        assert_eq!(
            uses,
            [
                (CONFIG_MAP, "cart-config"),
                (SECRET, "cart-tls"),
                (CLAIM, "cart-data"),
                (CONFIG_MAP, "ca-bundle"),
                (SECRET, "cart-env"),
                (CONFIG_MAP, "shared"),
                (SECRET, "registry"),
                (NODE, "node-a"),
            ]
        );
        assert_eq!(fact.owner, Some(Key::new("ReplicaSet", "cart-7f9c")));
        assert_eq!(fact.namespace.as_deref(), Some("shop"));
        assert_eq!(fact.labels.get("app").map(String::as_str), Some("cart"));
    }

    #[test]
    fn ingresses_reference_their_backend_services_and_tls_secrets() {
        let ingress: Arc<Ingress> = parse(json!({
            "metadata": { "name": "shop", "namespace": "shop" },
            "spec": {
                "defaultBackend": { "service": { "name": "fallback", "port": { "number": 80 } } },
                "tls": [{ "hosts": ["shop.example"], "secretName": "shop-tls" }],
                "rules": [{ "host": "shop.example", "http": { "paths": [
                    { "path": "/", "pathType": "Prefix", "backend": { "service": { "name": "cart", "port": { "number": 80 } } } },
                    { "path": "/api", "pathType": "Prefix", "backend": { "service": { "name": "cart", "port": { "number": 80 } } } }
                ] } }]
            }
        }));
        assert_eq!(
            ingress_uses(&ingress),
            [
                Key::new(SERVICE, "fallback"),
                Key::new(SERVICE, "cart"),
                Key::new(SECRET, "shop-tls"),
            ]
        );
    }
}
