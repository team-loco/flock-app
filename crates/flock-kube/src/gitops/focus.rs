use std::sync::{Arc, Mutex};
use std::time::Duration;

use flock_core::flux::{Drift, HelmRevision, HistoryEntry};
use flock_core::{FluxKey, FluxKind, Status};
use flock_flux::helm_storage::{Release, manifest_diff, revision_history};
use flock_flux::revision::short;
use flock_flux::{ObjectKind, redact};
use jiff::Timestamp;
use k8s_openapi::api::core::v1::{Event, Secret};
use kube::Api;
use kube::api::ListParams;
use serde_json::Value;

use super::convert::{elapsed, release_status};
use crate::flux::custom_kind;
use crate::manager::{ClusterHandle, Subscription, lock};
use crate::resources::WatchKind;
use crate::task::TaskGuard;

const REFRESH: Duration = Duration::from_secs(10);
const DRIFT_EVERY: u32 = 3;
const HISTORY_MAX: usize = 20;
const WARNING: &str = "Warning";
const REVISION_ANNOTATION_SUFFIX: &str = "/revision";
const HELM_OWNER: &str = "owner=helm";
const WORKLOAD_KINDS: [WatchKind; 3] = [
    WatchKind::Deployments,
    WatchKind::StatefulSets,
    WatchKind::DaemonSets,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredEvent {
    pub at: Option<Timestamp>,
    pub status: Status,
    pub revision: String,
    pub message: String,
    pub count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredRelease {
    pub revision: HelmRevision,
    pub deployed: Option<Timestamp>,
}

#[derive(Debug, Default)]
pub struct FocusDetail {
    events: Option<Vec<StoredEvent>>,
    releases: Option<Vec<StoredRelease>>,
    drift: Option<Drift>,
    seen: Vec<(String, String)>,
}

impl FocusDetail {
    pub fn history(&self, now: Timestamp) -> Option<Vec<HistoryEntry>> {
        let events = self.events.as_ref()?;
        let entries = events
            .iter()
            .map(|event| HistoryEntry {
                age: event.at.map(|then| elapsed(then, now)),
                status: event.status,
                revision: event.revision.clone(),
                message: event.message.clone(),
                count: event.count,
            })
            .collect();
        Some(entries)
    }

    pub fn drift(&self) -> Option<Drift> {
        self.drift.clone()
    }

    pub fn releases(&self, now: Timestamp) -> Option<Vec<HelmRevision>> {
        let releases = self.releases.as_ref()?;
        let revisions = releases
            .iter()
            .map(|stored| HelmRevision {
                age: stored
                    .deployed
                    .map(|then| elapsed(then, now))
                    .unwrap_or_default(),
                ..stored.revision.clone()
            })
            .collect();
        Some(revisions)
    }
}

pub struct Focus {
    pub key: FluxKey,
    pub detail: Arc<Mutex<FocusDetail>>,
    _workloads: Vec<Subscription>,
    _task: TaskGuard,
}

impl Focus {
    pub fn start(key: FluxKey, handle: &ClusterHandle, changed: async_channel::Sender<()>) -> Self {
        let detail = Arc::new(Mutex::new(FocusDetail::default()));
        let workloads = WORKLOAD_KINDS
            .iter()
            .map(|kind| handle.subscribe(kind.clone()))
            .collect();
        let task = TaskGuard::spawn(follow(key.clone(), handle.clone(), detail.clone(), changed));
        Self {
            key,
            detail,
            _workloads: workloads,
            _task: task,
        }
    }
}

async fn follow(
    key: FluxKey,
    handle: ClusterHandle,
    detail: Arc<Mutex<FocusDetail>>,
    changed: async_channel::Sender<()>,
) {
    let mut round: u32 = 0;
    loop {
        let drift_due = round.is_multiple_of(DRIFT_EVERY) || lock(&detail).drift.is_none();
        round = round.wrapping_add(1);
        if let Ok(client) = handle.client() {
            let history = async {
                if key.kind != FluxKind::Kustomization {
                    let events = events(client.clone(), &key).await;
                    lock(&detail).events = events;
                }
            };
            let releases = async {
                if key.kind == FluxKind::HelmRelease {
                    load_releases(client.clone(), &handle, &key, &detail).await;
                }
            };
            let drift = async {
                if key.kind == FluxKind::Kustomization && drift_due {
                    let found = drift(client.clone(), &handle, &key).await;
                    if found.is_some() {
                        lock(&detail).drift = found;
                    }
                }
            };
            tokio::join!(history, releases, drift);
            if changed.try_send(()).is_err() {
                tracing::trace!("GitOps change already pending");
            }
        }
        tokio::time::sleep(REFRESH).await;
    }
}

async fn drift(client: kube::Client, handle: &ClusterHandle, key: &FluxKey) -> Option<Drift> {
    let objects = handle.custom_objects(&custom_kind(ObjectKind::Kustomization));
    let object = objects.iter().find(|object| {
        object.metadata.namespace.as_deref() == Some(key.namespace.as_str())
            && object.metadata.name.as_deref() == Some(key.name.as_str())
    })?;
    let parsed = super::convert::parse(object, ObjectKind::Kustomization)?;
    let inventory = &parsed.object.inventory;
    let checked = super::drift::check(
        client,
        inventory,
        &key.namespace,
        &key.name,
        Timestamp::now(),
    );
    Some(checked.await)
}

async fn events(client: kube::Client, key: &FluxKey) -> Option<Vec<StoredEvent>> {
    let api: Api<Event> = Api::namespaced(client, &key.namespace);
    let selector = format!(
        "involvedObject.kind={},involvedObject.name={}",
        key.kind.label(),
        key.name
    );
    match api.list(&ListParams::default().fields(&selector)).await {
        Ok(list) => Some(stored_events(&list.items)),
        Err(error) => {
            tracing::debug!(%error, "Flux events unreadable");
            None
        }
    }
}

pub fn stored_events(events: &[Event]) -> Vec<StoredEvent> {
    let mut stored: Vec<StoredEvent> = events
        .iter()
        .map(|event| {
            let at = event
                .last_timestamp
                .as_ref()
                .map(|time| time.0)
                .or_else(|| event.event_time.as_ref().map(|time| time.0))
                .or_else(|| {
                    event
                        .metadata
                        .creation_timestamp
                        .as_ref()
                        .map(|time| time.0)
                });
            let revision = event
                .metadata
                .annotations
                .iter()
                .flatten()
                .find(|(name, _)| name.ends_with(REVISION_ANNOTATION_SUFFIX))
                .map(|(_, value)| short(value))
                .unwrap_or_default();
            let status = if event.type_.as_deref() == Some(WARNING) {
                Status::Err
            } else {
                Status::Ok
            };
            StoredEvent {
                at,
                status,
                revision,
                message: redact::credentials(
                    event
                        .message
                        .as_deref()
                        .and_then(|message| message.lines().next())
                        .unwrap_or_default(),
                ),
                count: event
                    .count
                    .and_then(|count| u32::try_from(count).ok())
                    .unwrap_or(1),
            }
        })
        .collect();
    stored.sort_by_key(|event| std::cmp::Reverse(event.at));
    stored.truncate(HISTORY_MAX);
    stored
}

pub fn storage(release: &Value) -> Option<(String, String)> {
    let namespace = release.pointer("/metadata/namespace")?.as_str()?;
    let name = release.pointer("/metadata/name")?.as_str()?;
    let text = |pointer: &str| release.pointer(pointer).and_then(Value::as_str);
    let target = text("/spec/targetNamespace");
    let storage = text("/status/storageNamespace")
        .or_else(|| text("/spec/storageNamespace"))
        .or(target)
        .unwrap_or(namespace);
    let default_name = match target {
        Some(target) => format!("{target}-{name}"),
        None => name.to_owned(),
    };
    let release_name = text("/status/history/0/name")
        .or_else(|| text("/spec/releaseName"))
        .map_or(default_name, str::to_owned);
    Some((storage.to_owned(), release_name))
}

async fn load_releases(
    client: kube::Client,
    handle: &ClusterHandle,
    key: &FluxKey,
    detail: &Arc<Mutex<FocusDetail>>,
) {
    let objects = handle.custom_objects(&custom_kind(ObjectKind::HelmRelease));
    let release = objects.iter().find(|object| {
        object.metadata.namespace.as_deref() == Some(key.namespace.as_str())
            && object.metadata.name.as_deref() == Some(key.name.as_str())
    });
    let located = release
        .and_then(|object| serde_json::to_value(object.as_ref()).ok())
        .and_then(|value| storage(&value));
    let Some((namespace, name)) = located else {
        return;
    };
    let api: Api<Secret> = Api::namespaced(client, &namespace);
    let selector = format!("{HELM_OWNER},name={name}");
    let secrets = match api.list(&ListParams::default().labels(&selector)).await {
        Ok(list) => list.items,
        Err(error) => {
            tracing::debug!(%error, "Helm storage unreadable");
            return;
        }
    };
    let mut seen: Vec<(String, String)> = secrets
        .iter()
        .map(|secret| {
            let name = secret.metadata.name.clone().unwrap_or_default();
            let version = secret.metadata.resource_version.clone().unwrap_or_default();
            (name, version)
        })
        .collect();
    seen.sort();
    if lock(detail).seen == seen {
        return;
    }
    let releases = match revision_history(&secrets) {
        Ok(history) => assemble(&history),
        Err(error) => {
            tracing::warn!(%error, release = %name, "Helm history could not be decoded");
            return;
        }
    };
    let mut detail = lock(detail);
    detail.releases = Some(releases);
    detail.seen = seen;
}

pub fn assemble(history: &[Release]) -> Vec<StoredRelease> {
    let mut stored: Vec<StoredRelease> = history
        .iter()
        .enumerate()
        .filter_map(|(ix, release)| {
            let revision = u32::try_from(release.version).ok()?;
            let previous = ix.checked_sub(1).and_then(|before| history.get(before));
            let diff = match previous {
                Some(older) if older.manifest != release.manifest => manifest_diff(older, release),
                Some(_) => String::new(),
                None => String::new(),
            };
            let deployed = release
                .last_deployed
                .or(release.first_deployed)
                .and_then(|at| Timestamp::from_second(at.timestamp()).ok());
            Some(StoredRelease {
                revision: HelmRevision {
                    revision,
                    status: release_status(&release.status),
                    state: release.status.clone(),
                    chart_version: release.chart_version.clone(),
                    app_version: release.app_version.clone().unwrap_or_default(),
                    description: release.description.clone(),
                    age: Duration::ZERO,
                    diff,
                },
                deployed,
            })
        })
        .collect();
    stored.reverse();
    stored
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn release(version: i64, status: &str, manifest: &str) -> Release {
        Release {
            name: "checkout-api".to_owned(),
            namespace: "payments".to_owned(),
            version,
            status: status.to_owned(),
            chart_name: "podinfo".to_owned(),
            chart_version: "6.15.0".to_owned(),
            app_version: Some("6.15.0".to_owned()),
            first_deployed: None,
            last_deployed: None,
            description: format!("rev {version}"),
            manifest: manifest.to_owned(),
            config: json!({}),
            chart_values: json!({}),
        }
    }

    #[test]
    fn helm_history_is_newest_first_with_diffs_against_the_previous_revision() {
        let history = [
            release(1, "superseded", "kind: Service\n"),
            release(2, "deployed", "kind: Service\nport: 80\n"),
            release(3, "failed", "kind: Service\nport: 80\n"),
        ];
        let stored = assemble(&history);
        let shape: Vec<(u32, Status, bool)> = stored
            .iter()
            .map(|s| {
                let r = &s.revision;
                (r.revision, r.status, r.diff.is_empty())
            })
            .collect();
        assert_eq!(
            shape,
            [
                (3, Status::Err, true),
                (2, Status::Ok, false),
                (1, Status::Ok, true),
            ]
        );
        assert!(stored[1].revision.diff.contains("+port: 80"));
        assert!(stored[1].revision.diff.contains("checkout-api/v1"));
    }

    #[test]
    fn storage_defaults_follow_flux() {
        let release = json!({
            "metadata": { "name": "podinfo", "namespace": "flux-system" },
            "spec": { "targetNamespace": "apps" }
        });
        assert_eq!(
            storage(&release),
            Some(("apps".to_owned(), "apps-podinfo".to_owned()))
        );
        let recorded = json!({
            "metadata": { "name": "podinfo", "namespace": "apps" },
            "spec": {},
            "status": { "storageNamespace": "apps", "history": [{ "name": "podinfo" }] }
        });
        assert_eq!(
            storage(&recorded),
            Some(("apps".to_owned(), "podinfo".to_owned()))
        );
    }

    #[test]
    fn events_become_history_newest_first() {
        let parsed: Result<Vec<Event>, _> = serde_json::from_value(json!([
            {
                "metadata": { "name": "a", "annotations": { "helm.toolkit.fluxcd.io/revision": "6.15.0" } },
                "involvedObject": {},
                "type": "Warning", "count": 3,
                "message": "Helm upgrade failed\n\nLast Helm logs",
                "lastTimestamp": "2026-10-13T08:30:00Z"
            },
            {
                "metadata": { "name": "b" },
                "involvedObject": {},
                "type": "Normal",
                "message": "Helm install succeeded",
                "lastTimestamp": "2026-10-13T09:30:00Z"
            }
        ]));
        let Ok(events) = parsed else {
            panic!("event fixtures");
        };
        let stored = stored_events(&events);
        let shape: Vec<(Status, &str, &str, u32)> = stored
            .iter()
            .map(|e| (e.status, e.revision.as_str(), e.message.as_str(), e.count))
            .collect();
        assert_eq!(
            shape,
            [
                (Status::Ok, "", "Helm install succeeded", 1),
                (Status::Err, "6.15.0", "Helm upgrade failed", 3),
            ]
        );
    }
}
