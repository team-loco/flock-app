use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use flock_core::flux::HelmRevision;
use flock_core::helm::{HelmKey, HelmRelease, HelmReleaseDetail};
use flock_core::{ClusterId, FluxKey, FluxKind};
use flock_flux::helm_storage::{self, Release};
use flock_flux::{ObjectKind, redact};
use jiff::Timestamp;
use k8s_openapi::api::core::v1::Secret;
use kube::api::DynamicObject;
use serde_json::Value;

use crate::clock::between;
use crate::gitops::convert::release_status;
use crate::gitops::focus::{assemble, storage};
use crate::manager::lock;

pub const FIELD_SELECTOR: &str = "type=helm.sh/release.v1";

type Slot = (String, String);
type Cache = HashMap<(ClusterId, String), (String, Arc<Release>)>;

#[derive(Default)]
pub struct Decoded {
    cache: Mutex<Cache>,
}

fn secret_key(secret: &Secret) -> Option<(String, String)> {
    let meta = &secret.metadata;
    let id = meta.uid.clone().or_else(|| meta.name.clone())?;
    Some((id, meta.resource_version.clone().unwrap_or_default()))
}

impl Decoded {
    pub fn releases(&self, cluster: &ClusterId, secrets: &[Arc<Secret>]) -> Vec<Arc<Release>> {
        let mut cache = lock(&self.cache);
        let mut decoded = Vec::new();
        let mut seen = Vec::new();
        for secret in secrets {
            let Some((id, version)) = secret_key(secret) else {
                continue;
            };
            let slot = (cluster.clone(), id);
            let cached = cache
                .get(&slot)
                .filter(|(known, _)| *known == version)
                .map(|(_, release)| release.clone());
            let release = match cached {
                Some(release) => release,
                None => match helm_storage::decode_secret(secret) {
                    Ok(release) => {
                        let release = Arc::new(release);
                        cache.insert(slot.clone(), (version, release.clone()));
                        release
                    }
                    Err(error) => {
                        let name = secret.metadata.name.as_deref().unwrap_or_default();
                        tracing::debug!(%error, secret = name, "Helm storage secret skipped");
                        continue;
                    }
                },
            };
            seen.push(slot);
            decoded.push(release);
        }
        cache.retain(|slot, _| &slot.0 != cluster || seen.contains(slot));
        decoded
    }

    pub fn forget(&self, keep: &[ClusterId]) {
        lock(&self.cache).retain(|(cluster, _), _| keep.contains(cluster));
    }
}

pub fn owners(cluster: &ClusterId, objects: &[Arc<DynamicObject>]) -> BTreeMap<Slot, FluxKey> {
    objects
        .iter()
        .filter_map(|object| {
            let value = serde_json::to_value(object.as_ref()).ok()?;
            let located = storage(&value)?;
            let key = FluxKey::new(
                cluster.as_str(),
                FluxKind::HelmRelease,
                object.metadata.namespace.clone()?,
                object.metadata.name.clone()?,
            );
            Some((located, key))
        })
        .collect()
}

fn updated(release: &Release) -> Option<Timestamp> {
    let at = release.last_deployed.or(release.first_deployed)?;
    Timestamp::from_second(at.timestamp()).ok()
}

pub fn summaries(
    cluster: &ClusterId,
    releases: &[Arc<Release>],
    owners: &BTreeMap<Slot, FluxKey>,
    now: Timestamp,
) -> Vec<HelmRelease> {
    let mut latest: BTreeMap<Slot, &Release> = BTreeMap::new();
    for release in releases {
        let slot = (release.namespace.clone(), release.name.clone());
        let newer = latest
            .get(&slot)
            .is_none_or(|known| known.version < release.version);
        if newer {
            latest.insert(slot, release);
        }
    }
    latest
        .into_iter()
        .map(|(slot, release)| HelmRelease {
            key: HelmKey::new(cluster.clone(), &release.namespace, &release.name),
            chart: release.chart_name.clone(),
            chart_version: release.chart_version.clone(),
            app_version: release.app_version.clone().unwrap_or_default(),
            revision: u32::try_from(release.version).unwrap_or_default(),
            status: release_status(&release.status),
            state: release.status.clone(),
            updated: updated(release).map(|at| between(at, now)),
            flux: owners.get(&slot).cloned(),
        })
        .collect()
}

fn values_text(mut values: Value) -> String {
    redact::values(&mut values);
    let empty = values.as_object().is_some_and(serde_json::Map::is_empty);
    if empty {
        return String::new();
    }
    serde_norway::to_string(&values).unwrap_or_default()
}

pub fn detail(
    key: &HelmKey,
    releases: &[Arc<Release>],
    now: Timestamp,
) -> Option<HelmReleaseDetail> {
    let mut history: Vec<Release> = releases
        .iter()
        .filter(|release| release.namespace == key.namespace && release.name == key.name)
        .map(|release| release.as_ref().clone())
        .collect();
    history.sort_by_key(|release| release.version);
    let current = history.last()?.clone();
    let revisions: Vec<HelmRevision> = assemble(&history)
        .into_iter()
        .map(|stored| HelmRevision {
            age: stored
                .deployed
                .map(|at| between(at, now))
                .unwrap_or_default(),
            ..stored.revision
        })
        .collect();
    Some(HelmReleaseDetail {
        key: key.clone(),
        history: revisions,
        user_values: values_text(current.config.clone()),
        computed_values: values_text(helm_storage::computed_values(&current)),
        manifest: redact::manifest(&current.manifest),
    })
}

pub fn flux_kind() -> crate::resources::CustomKind {
    crate::flux::custom_kind(ObjectKind::HelmRelease)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn now() -> Timestamp {
        "2026-10-19T12:00:00Z".parse().unwrap_or_default()
    }

    fn release(name: &str, version: i64, status: &str, config: Value) -> Arc<Release> {
        let deployed: chrono::DateTime<chrono::Utc> =
            "2026-10-19T11:00:00Z".parse().unwrap_or_default();
        Arc::new(Release {
            name: name.into(),
            namespace: "payments".into(),
            version,
            status: status.into(),
            chart_name: "podinfo".into(),
            chart_version: format!("6.{version}.0"),
            app_version: Some("6.0".into()),
            first_deployed: Some(deployed),
            last_deployed: Some(deployed),
            description: format!("Upgrade {version}"),
            manifest: format!(
                "---\napiVersion: v1\nkind: Secret\nmetadata:\n  name: db\ndata:\n  password: c2VjcmV0LXt7dmVyc2lvbn19\n---\nkind: ConfigMap\nmetadata:\n  name: v{version}\n"
            ),
            config,
            chart_values: json!({ "replicas": 1, "auth": { "password": "chart-pass" } }),
        })
    }

    fn cluster() -> ClusterId {
        ClusterId::new("fleet-a")
    }

    #[test]
    fn summaries_keep_the_latest_revision_and_mark_flux_owners() {
        let releases = [
            release("checkout-api", 1, "superseded", json!({})),
            release("checkout-api", 2, "failed", json!({})),
            release("hand-installed", 1, "deployed", json!({})),
        ];
        let key = FluxKey::new("fleet-a", FluxKind::HelmRelease, "flux-system", "checkout");
        let owners = BTreeMap::from([(("payments".into(), "checkout-api".into()), key.clone())]);
        let rows = summaries(&cluster(), &releases, &owners, now());
        let shape: Vec<(&str, u32, flock_core::Status, Option<&FluxKey>)> = rows
            .iter()
            .map(|r| (r.key.name.as_str(), r.revision, r.status, r.flux.as_ref()))
            .collect();
        assert_eq!(
            shape,
            [
                ("checkout-api", 2, flock_core::Status::Err, Some(&key)),
                ("hand-installed", 1, flock_core::Status::Ok, None),
            ]
        );
        assert_eq!(rows[0].chart_label(), "podinfo-6.2.0");
        assert_eq!(rows[0].updated, Some(std::time::Duration::from_secs(3600)));
    }

    #[test]
    fn details_hold_every_revision_and_redacted_values_and_manifest() {
        let releases = [
            release("checkout-api", 1, "superseded", json!({ "replicas": 2 })),
            release(
                "checkout-api",
                2,
                "deployed",
                json!({ "replicas": 3, "auth": { "password": "planted-user-pass" } }),
            ),
            release("other", 1, "deployed", json!({})),
        ];
        let key = HelmKey::new(cluster(), "payments", "checkout-api");
        let Some(detail) = detail(&key, &releases, now()) else {
            panic!("detail");
        };
        let revisions: Vec<u32> = detail.history.iter().map(|r| r.revision).collect();
        assert_eq!(revisions, [2, 1]);
        assert!(detail.user_values.contains("replicas: 3"));
        assert!(!detail.user_values.contains("planted-user-pass"));
        assert!(detail.computed_values.contains("replicas: 3"));
        assert!(!detail.computed_values.contains("chart-pass"));
        assert!(!detail.manifest.contains("c2VjcmV0"));
        assert!(detail.manifest.contains("name: v2"));
        let missing = HelmKey::new(cluster(), "payments", "gone");
        assert_eq!(super::detail(&missing, &releases, now()), None);
    }

    #[test]
    fn flux_owners_resolve_to_their_storage_namespace_and_release_name() {
        let object: DynamicObject = serde_json::from_value(json!({
            "apiVersion": "helm.toolkit.fluxcd.io/v2", "kind": "HelmRelease",
            "metadata": { "name": "podinfo", "namespace": "flux-system" },
            "spec": { "targetNamespace": "apps" }
        }))
        .unwrap_or_else(|_| panic!("fixture"));
        let found = owners(&cluster(), &[Arc::new(object)]);
        let key = found.get(&("apps".to_owned(), "apps-podinfo".to_owned()));
        assert_eq!(key.map(|key| key.name.as_str()), Some("podinfo"));
    }
}
