use std::collections::{BTreeMap, BTreeSet};

use flock_core::flux::{Drift, DriftField, DriftState, DriftedObject};
use flock_flux::InventoryEntry;
use futures::StreamExt;
use jiff::Timestamp;
use kube::api::{ApiResource, DynamicObject, ListParams};
use kube::core::GroupVersionKind;
use kube::{Api, Client};
use serde_json::{Map, Value};

use super::convert::elapsed;

pub const KUSTOMIZE_MANAGER: &str = "kustomize-controller";
const APPLY: &str = "Apply";
const STATUS_SUBRESOURCE: &str = "status";
const NAME_LABEL: &str = "kustomize.toolkit.fluxcd.io/name";
const NAMESPACE_LABEL: &str = "kustomize.toolkit.fluxcd.io/namespace";
const SECRET: &str = "Secret";
const HIDDEN: &str = "(hidden)";
const ABSENT: &str = "(removed)";
const ELLIPSIS: char = '…';
const VALUE_MAX: usize = 120;
const CONCURRENCY: usize = 8;
const FIELD_PREFIX: &str = "f:";
const KEY_PREFIX: &str = "k:";
const VALUE_PREFIX: &str = "v:";
const SELF_KEY: &str = ".";

#[derive(Clone, Debug, PartialEq)]
enum Segment {
    Field(String),
    Key(Map<String, Value>),
    Item(Value),
}

fn segment(key: &str) -> Option<Segment> {
    if let Some(field) = key.strip_prefix(FIELD_PREFIX) {
        return Some(Segment::Field(field.to_owned()));
    }
    if let Some(json) = key.strip_prefix(KEY_PREFIX) {
        return serde_json::from_str(json).ok().map(Segment::Key);
    }
    let json = key.strip_prefix(VALUE_PREFIX)?;
    serde_json::from_str(json).ok().map(Segment::Item)
}

fn leaves(node: &Value, prefix: &mut Vec<Segment>, out: &mut Vec<Vec<Segment>>) {
    let children: Vec<(&String, &Value)> = node
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| key.as_str() != SELF_KEY)
        .collect();
    if children.is_empty() {
        if !prefix.is_empty() {
            out.push(prefix.clone());
        }
        return;
    }
    for (key, child) in children {
        let Some(step) = segment(key) else {
            continue;
        };
        prefix.push(step);
        leaves(child, prefix, out);
        prefix.pop();
    }
}

fn render_path(path: &[Segment]) -> String {
    let mut text = String::new();
    for step in path {
        match step {
            Segment::Field(name) => {
                if !text.is_empty() {
                    text.push('.');
                }
                text.push_str(name);
            }
            Segment::Key(keys) => {
                let pairs: Vec<String> = keys
                    .iter()
                    .map(|(key, value)| format!("{key}={}", plain(value)))
                    .collect();
                text.push('[');
                text.push_str(&pairs.join(","));
                text.push(']');
            }
            Segment::Item(value) => {
                text.push('[');
                text.push_str(&plain(value));
                text.push(']');
            }
        }
    }
    text
}

fn plain(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn lookup<'a>(live: &'a Value, path: &[Segment]) -> Option<&'a Value> {
    path.iter().try_fold(live, |node, step| match step {
        Segment::Field(name) => node.get(name),
        Segment::Key(keys) => node
            .as_array()?
            .iter()
            .find(|item| keys.iter().all(|(key, value)| item.get(key) == Some(value))),
        Segment::Item(wanted) => node.as_array()?.iter().find(|item| *item == wanted),
    })
}

fn shown(value: Option<&Value>, hidden: bool) -> String {
    let Some(value) = value else {
        return ABSENT.to_owned();
    };
    if hidden {
        return HIDDEN.to_owned();
    }
    let text = plain(value);
    if text.chars().count() <= VALUE_MAX {
        return text;
    }
    let mut cut: String = text.chars().take(VALUE_MAX).collect();
    cut.push(ELLIPSIS);
    cut
}

fn time_of(entry: &Value) -> Option<Timestamp> {
    entry.get("time")?.as_str()?.parse().ok()
}

fn text<'a>(entry: &'a Value, field: &str) -> &'a str {
    entry.get(field).and_then(Value::as_str).unwrap_or_default()
}

pub fn changed_fields(live: &Value, now: Timestamp) -> Vec<DriftField> {
    let entries: Vec<&Value> = live
        .pointer("/metadata/managedFields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    let applied = entries
        .iter()
        .filter(|entry| text(entry, "manager") == KUSTOMIZE_MANAGER)
        .filter(|entry| text(entry, "operation") == APPLY)
        .filter_map(|entry| time_of(entry))
        .max();
    let Some(applied) = applied else {
        return Vec::new();
    };
    let hidden = text(live, "kind") == SECRET;
    let mut fields = Vec::new();
    for entry in entries {
        let manager = text(entry, "manager");
        let subresource = text(entry, "subresource");
        let Some(at) = time_of(entry) else {
            continue;
        };
        if manager == KUSTOMIZE_MANAGER || subresource == STATUS_SUBRESOURCE || at <= applied {
            continue;
        }
        let owner = if subresource.is_empty() {
            manager.to_owned()
        } else {
            format!("{manager} ({subresource})")
        };
        let mut paths = Vec::new();
        if let Some(owned) = entry.get("fieldsV1") {
            leaves(owned, &mut Vec::new(), &mut paths);
        }
        for path in paths {
            fields.push(DriftField {
                path: render_path(&path),
                value: shown(lookup(live, &path), hidden),
                manager: owner.clone(),
                age: Some(elapsed(at, now)),
            });
        }
    }
    fields.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.manager.cmp(&b.manager)));
    fields
}

type Identity = (String, String, String, String);

fn identity(entry: &InventoryEntry) -> Identity {
    (
        entry.group.clone(),
        entry.kind.clone(),
        entry.namespace.clone(),
        entry.name.clone(),
    )
}

fn namespace_of(namespace: &str) -> Option<String> {
    (!namespace.is_empty()).then(|| namespace.to_owned())
}

fn drifted(entry: &InventoryEntry, state: DriftState, fields: Vec<DriftField>) -> DriftedObject {
    DriftedObject {
        state,
        kind: entry.kind.clone(),
        namespace: namespace_of(&entry.namespace),
        name: entry.name.clone(),
        fields,
    }
}

pub fn extras(
    inventory: &[InventoryEntry],
    labelled: &[(String, String, String, String)],
) -> Vec<DriftedObject> {
    let known: BTreeSet<Identity> = inventory.iter().map(identity).collect();
    labelled
        .iter()
        .filter(|found| !known.contains(*found))
        .map(|(_, kind, namespace, name)| DriftedObject {
            state: DriftState::Extra,
            kind: kind.clone(),
            namespace: namespace_of(namespace),
            name: name.clone(),
            fields: Vec::new(),
        })
        .collect()
}

pub fn order(objects: &mut [DriftedObject]) {
    objects.sort_by(|a, b| a.state.cmp(&b.state).then_with(|| a.path().cmp(&b.path())));
}

fn gvk(entry: &InventoryEntry) -> GroupVersionKind {
    GroupVersionKind::gvk(&entry.group, &entry.version, &entry.kind)
}

fn api(client: &Client, resource: &ApiResource, namespace: &str) -> Api<DynamicObject> {
    if namespace.is_empty() {
        Api::all_with(client.clone(), resource)
    } else {
        Api::namespaced_with(client.clone(), namespace, resource)
    }
}

enum Probe {
    Found(DriftedObject),
    Clean,
    Unreadable(String),
}

async fn probe(
    client: Client,
    resource: Option<ApiResource>,
    entry: InventoryEntry,
    now: Timestamp,
) -> Probe {
    let label = format!("{}/{}", entry.kind, entry.name);
    let Some(resource) = resource else {
        return Probe::Unreadable(label);
    };
    match api(&client, &resource, &entry.namespace)
        .get_opt(&entry.name)
        .await
    {
        Ok(None) => Probe::Found(drifted(&entry, DriftState::Missing, Vec::new())),
        Ok(Some(object)) => {
            let live = serde_json::to_value(&object).unwrap_or_default();
            let fields = changed_fields(&live, now);
            if fields.is_empty() {
                Probe::Clean
            } else {
                Probe::Found(drifted(&entry, DriftState::Changed, fields))
            }
        }
        Err(error) => {
            tracing::debug!(%error, object = %label, "drift probe failed");
            Probe::Unreadable(label)
        }
    }
}

async fn labelled(
    client: &Client,
    resource: &ApiResource,
    group: &str,
    selector: &str,
) -> Option<Vec<Identity>> {
    let api: Api<DynamicObject> = Api::all_with(client.clone(), resource);
    let listed = api.list(&ListParams::default().labels(selector)).await;
    match listed {
        Ok(list) => Some(
            list.items
                .into_iter()
                .map(|object| {
                    (
                        group.to_owned(),
                        resource.kind.clone(),
                        object.metadata.namespace.unwrap_or_default(),
                        object.metadata.name.unwrap_or_default(),
                    )
                })
                .collect(),
        ),
        Err(error) => {
            tracing::debug!(%error, kind = %resource.kind, "drift extras unreadable");
            None
        }
    }
}

pub async fn check(
    client: Client,
    inventory: &[InventoryEntry],
    namespace: &str,
    name: &str,
    now: Timestamp,
) -> Drift {
    let kinds: BTreeMap<(String, String, String), GroupVersionKind> = inventory
        .iter()
        .map(|entry| {
            let key = (
                entry.group.clone(),
                entry.version.clone(),
                entry.kind.clone(),
            );
            (key, gvk(entry))
        })
        .collect();
    let resolved = futures::stream::iter(kinds)
        .map(|(key, gvk)| {
            let client = client.clone();
            async move {
                let resource = kube::discovery::pinned_kind(&client, &gvk).await;
                (key, resource.ok().map(|(resource, _)| resource))
            }
        })
        .buffer_unordered(CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
    let resources: BTreeMap<(String, String, String), Option<ApiResource>> =
        resolved.into_iter().collect();
    let jobs: Vec<(Option<ApiResource>, InventoryEntry)> = inventory
        .iter()
        .map(|entry| {
            let key = (
                entry.group.clone(),
                entry.version.clone(),
                entry.kind.clone(),
            );
            let resource = resources.get(&key).cloned().flatten();
            (resource, entry.clone())
        })
        .collect();
    let probes = futures::stream::iter(jobs)
        .map(|(resource, entry)| probe(client.clone(), resource, entry, now))
        .buffer_unordered(CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
    let mut drift = Drift {
        checked: inventory.len(),
        ..Drift::default()
    };
    for found in probes {
        match found {
            Probe::Found(object) => drift.objects.push(object),
            Probe::Clean => {}
            Probe::Unreadable(label) => drift.unreadable.push(label),
        }
    }
    let selector = format!("{NAME_LABEL}={name},{NAMESPACE_LABEL}={namespace}");
    let mut found = Vec::new();
    for ((group, _, _), resource) in &resources {
        let Some(resource) = resource else {
            continue;
        };
        if let Some(rows) = labelled(&client, resource, group, &selector).await {
            found.extend(rows);
        }
    }
    found.sort();
    found.dedup();
    drift.objects.extend(extras(inventory, &found));
    order(&mut drift.objects);
    drift.unreadable.sort();
    drift
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn now() -> Timestamp {
        "2026-10-19T12:00:00Z"
            .parse()
            .unwrap_or(Timestamp::UNIX_EPOCH)
    }

    fn deployment(entries: &Value) -> Value {
        json!({
            "kind": "Deployment",
            "metadata": { "name": "podinfo", "managedFields": entries },
            "spec": {
                "replicas": 3,
                "template": { "spec": { "containers": [
                    { "name": "podinfo", "image": "podinfo:6.5", "env": [{ "name": "DEBUG", "value": "1" }] }
                ] } }
            },
            "status": { "readyReplicas": 3 }
        })
    }

    fn apply_at(time: &str) -> Value {
        json!({
            "manager": KUSTOMIZE_MANAGER, "operation": "Apply", "time": time,
            "fieldsV1": { "f:spec": { "f:replicas": {} } }
        })
    }

    #[test]
    fn fields_other_managers_wrote_after_the_last_apply_are_drift() {
        let live = deployment(&json!([
            apply_at("2026-10-19T11:00:00Z"),
            {
                "manager": "kubectl-edit", "operation": "Update", "time": "2026-10-19T11:58:00Z",
                "fieldsV1": { "f:spec": { "f:template": { "f:spec": { "f:containers": {
                    "k:{\"name\":\"podinfo\"}": {
                        ".": {},
                        "f:image": {},
                        "f:env": { "k:{\"name\":\"DEBUG\"}": { ".": {}, "f:name": {}, "f:value": {} } }
                    }
                } } } } }
            },
            {
                "manager": "kube-controller-manager", "operation": "Update", "subresource": "status",
                "time": "2026-10-19T11:59:00Z", "fieldsV1": { "f:status": { "f:readyReplicas": {} } }
            },
            {
                "manager": "kubectl", "operation": "Update", "subresource": "scale",
                "time": "2026-10-19T11:59:30Z", "fieldsV1": { "f:spec": { "f:replicas": {} } }
            },
            {
                "manager": "helm", "operation": "Update", "time": "2026-10-19T10:00:00Z",
                "fieldsV1": { "f:metadata": { "f:labels": { "f:old": {} } } }
            }
        ]));
        let fields = changed_fields(&live, now());
        let rows: Vec<(&str, &str, &str)> = fields
            .iter()
            .map(|f| (f.path.as_str(), f.value.as_str(), f.manager.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                ("spec.replicas", "3", "kubectl (scale)"),
                (
                    "spec.template.spec.containers[name=podinfo].env[name=DEBUG].name",
                    "DEBUG",
                    "kubectl-edit"
                ),
                (
                    "spec.template.spec.containers[name=podinfo].env[name=DEBUG].value",
                    "1",
                    "kubectl-edit"
                ),
                (
                    "spec.template.spec.containers[name=podinfo].image",
                    "podinfo:6.5",
                    "kubectl-edit"
                ),
            ]
        );
        assert_eq!(fields[0].age, Some(std::time::Duration::from_secs(30)));
    }

    #[test]
    fn objects_flux_never_applied_report_no_field_drift() {
        let live = deployment(&json!([{
            "manager": "kubectl-edit", "operation": "Update", "time": "2026-10-19T11:58:00Z",
            "fieldsV1": { "f:spec": { "f:replicas": {} } }
        }]));
        assert_eq!(changed_fields(&live, now()), Vec::new());
    }

    #[test]
    fn secret_values_stay_hidden() {
        let live = json!({
            "kind": "Secret",
            "metadata": { "name": "creds", "managedFields": [
                apply_at("2026-10-19T11:00:00Z"),
                { "manager": "kubectl-edit", "operation": "Update", "time": "2026-10-19T11:30:00Z",
                  "fieldsV1": { "f:data": { "f:password": {} } } }
            ] },
            "data": { "password": "c2VjcmV0" }
        });
        let fields = changed_fields(&live, now());
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].value, HIDDEN);
    }

    fn entry(kind: &str, namespace: &str, name: &str) -> InventoryEntry {
        InventoryEntry {
            namespace: namespace.into(),
            name: name.into(),
            group: String::new(),
            kind: kind.into(),
            version: "v1".into(),
        }
    }

    #[test]
    fn labelled_objects_outside_the_inventory_are_extra() {
        let inventory = [entry("Service", "web", "podinfo")];
        let labelled = [
            (
                String::new(),
                "Service".into(),
                "web".into(),
                "podinfo".into(),
            ),
            (
                String::new(),
                "Service".into(),
                "web".into(),
                "leftover".into(),
            ),
        ];
        let mut found = extras(&inventory, &labelled);
        found.push(drifted(
            &entry("ConfigMap", "web", "a"),
            DriftState::Missing,
            Vec::new(),
        ));
        order(&mut found);
        let paths: Vec<(DriftState, String)> = found.iter().map(|o| (o.state, o.path())).collect();
        assert_eq!(
            paths,
            [
                (DriftState::Missing, "ConfigMap/web/a".to_owned()),
                (DriftState::Extra, "Service/web/leftover".to_owned()),
            ]
        );
    }
}
