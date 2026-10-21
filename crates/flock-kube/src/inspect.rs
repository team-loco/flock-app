use flock_core::ResourceRef;
use flock_core::related::{AUTOSCALER, PERSISTENT_VOLUME};
use flock_core::resources::ResourceKind;
use flock_flux::{ObjectKind, redact};
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use k8s_openapi::api::autoscaling::v2::HorizontalPodAutoscaler;
use k8s_openapi::api::batch::v1::{CronJob, Job};
use k8s_openapi::api::core::v1::{
    ConfigMap, Namespace, Node, PersistentVolume, PersistentVolumeClaim, Pod, Secret, Service,
};
use k8s_openapi::api::networking::v1::Ingress;
use kube::Resource;
use kube::api::PartialObjectMeta;
use serde_json::Value;

use crate::error::Result;
use crate::flux::custom_kind;
use crate::manager::ClusterHandle;
use crate::ops::ObjectRef;
use crate::resources::Watched;

const POD_KIND: &str = "Pod";
const NODE_KIND: &str = "Node";
const NAMESPACE_KIND: &str = "Namespace";
const CORE_KINDS: [(&str, &str); 5] = [
    (POD_KIND, "v1"),
    (NODE_KIND, "v1"),
    (NAMESPACE_KIND, "v1"),
    (PERSISTENT_VOLUME, "v1"),
    (AUTOSCALER, "autoscaling/v2"),
];

fn resource_api_version(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Deployment => "apps/v1",
        ResourceKind::StatefulSet => "apps/v1",
        ResourceKind::DaemonSet => "apps/v1",
        ResourceKind::Job => "batch/v1",
        ResourceKind::CronJob => "batch/v1",
        ResourceKind::Service => "v1",
        ResourceKind::Ingress => "networking.k8s.io/v1",
        ResourceKind::ConfigMap => "v1",
        ResourceKind::Secret => "v1",
        ResourceKind::PersistentVolumeClaim => "v1",
    }
}

const FLUX_KINDS: [ObjectKind; 6] = [
    ObjectKind::Kustomization,
    ObjectKind::HelmRelease,
    ObjectKind::GitRepository,
    ObjectKind::HelmRepository,
    ObjectKind::HelmChart,
    ObjectKind::OCIRepository,
];

pub fn api_version(kind: &str) -> Option<&'static str> {
    let builtin = ResourceKind::ALL
        .into_iter()
        .find(|k| k.name() == kind)
        .map(resource_api_version);
    let core = || {
        CORE_KINDS
            .iter()
            .find(|(name, _)| *name == kind)
            .map(|(_, version)| *version)
    };
    let flux = || {
        FLUX_KINDS
            .into_iter()
            .find(|k| k.as_str() == kind)
            .map(ObjectKind::api_version)
    };
    builtin.or_else(core).or_else(flux)
}

pub fn object_ref(target: &ResourceRef) -> Option<ObjectRef> {
    Some(ObjectRef {
        api_version: api_version(&target.kind)?.to_owned(),
        kind: target.kind.clone(),
        namespace: target.namespace.clone(),
        name: target.name.clone(),
    })
}

pub fn render(object: &mut Value) -> Result<String> {
    redact::object(object);
    Ok(serde_norway::to_string(object)?)
}

pub struct Inspected {
    pub version: Option<String>,
    pub yaml: String,
}

pub async fn yaml(handle: &ClusterHandle, target: &ResourceRef) -> Result<Inspected> {
    let reference = match object_ref(target) {
        Some(reference) => reference,
        None => crate::crd::object_ref(handle, target)?,
    };
    let object = handle.get(&reference).await?;
    let version = object.metadata.resource_version.clone();
    let mut value = serde_json::to_value(&object)?;
    let yaml = render(&mut value)?;
    Ok(Inspected { version, yaml })
}

fn same_object<K: Resource>(object: &K, target: &ResourceRef) -> bool {
    let meta = object.meta();
    meta.name.as_deref() == Some(target.name.as_str())
        && meta.namespace.as_deref() == target.namespace.as_deref()
}

fn version_of<K>(handle: &ClusterHandle, target: &ResourceRef) -> Option<String>
where
    K: Watched,
    K::DynamicType: Eq + std::hash::Hash + Clone,
{
    handle
        .objects::<K>()
        .into_iter()
        .find(|object| same_object(object.as_ref(), target))
        .and_then(|object| object.meta().resource_version.clone())
}

pub fn cached_version(handle: &ClusterHandle, target: &ResourceRef) -> Option<String> {
    if flock_core::crd::CrdKey::parse(&target.kind).is_some() {
        return crate::crd::cached_version(handle, target);
    }
    let flux = FLUX_KINDS
        .into_iter()
        .find(|kind| kind.as_str() == target.kind);
    if let Some(kind) = flux {
        return handle
            .custom_objects(&custom_kind(kind))
            .into_iter()
            .find(|object| same_object(object.as_ref(), target))
            .and_then(|object| object.metadata.resource_version.clone());
    }
    let builtin = ResourceKind::ALL
        .into_iter()
        .find(|kind| kind.name() == target.kind);
    match builtin {
        Some(ResourceKind::Deployment) => version_of::<Deployment>(handle, target),
        Some(ResourceKind::StatefulSet) => version_of::<StatefulSet>(handle, target),
        Some(ResourceKind::DaemonSet) => version_of::<DaemonSet>(handle, target),
        Some(ResourceKind::Job) => version_of::<Job>(handle, target),
        Some(ResourceKind::CronJob) => version_of::<CronJob>(handle, target),
        Some(ResourceKind::Service) => version_of::<Service>(handle, target),
        Some(ResourceKind::Ingress) => version_of::<Ingress>(handle, target),
        Some(ResourceKind::ConfigMap) => version_of::<PartialObjectMeta<ConfigMap>>(handle, target),
        Some(ResourceKind::Secret) => version_of::<PartialObjectMeta<Secret>>(handle, target),
        Some(ResourceKind::PersistentVolumeClaim) => {
            version_of::<PersistentVolumeClaim>(handle, target)
        }
        None => match target.kind.as_str() {
            POD_KIND => version_of::<Pod>(handle, target),
            NODE_KIND => version_of::<Node>(handle, target),
            NAMESPACE_KIND => version_of::<Namespace>(handle, target),
            PERSISTENT_VOLUME => version_of::<PersistentVolume>(handle, target),
            AUTOSCALER => version_of::<HorizontalPodAutoscaler>(handle, target),
            _ => None,
        },
    }
}

pub fn stale(fetched: Option<&str>, cached: Option<&str>) -> bool {
    cached.is_some() && cached != fetched
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn secrets_render_without_their_values() {
        let applied = json!({
            "apiVersion": "v1",
            "kind": "Secret",
            "metadata": { "name": "stripe" },
            "stringData": { "webhook-secret": "planted-password" }
        });
        let mut secret = json!({
            "apiVersion": "v1",
            "kind": "Secret",
            "metadata": {
                "name": "stripe",
                "managedFields": [{ "manager": "kubectl" }],
                "annotations": {
                    "kubectl.kubernetes.io/last-applied-configuration": applied.to_string()
                }
            },
            "type": "Opaque",
            "data": { "webhook-secret": "c2VjcmV0" }
        });
        let Ok(text) = render(&mut secret) else {
            panic!("render");
        };
        assert!(
            text.contains(&format!("webhook-secret: {}", redact::MASK)),
            "{text}"
        );
        assert!(!text.contains("c2VjcmV0"));
        assert!(!text.contains("planted-password"), "{text}");
        assert!(!text.contains("managedFields"));
    }

    #[test]
    fn yaml_refreshes_only_when_the_watch_sees_a_new_version() {
        assert!(stale(Some("41"), Some("42")));
        assert!(stale(None, Some("42")));
        assert!(!stale(Some("42"), Some("42")));
        assert!(!stale(Some("42"), None));
    }

    #[test]
    fn kinds_resolve_their_api_versions() {
        assert_eq!(api_version("Deployment"), Some("apps/v1"));
        assert_eq!(api_version("Node"), Some("v1"));
        assert_eq!(
            api_version("HelmRelease"),
            Some("helm.toolkit.fluxcd.io/v2")
        );
        assert_eq!(api_version("Widget"), None);
    }
}
