use std::sync::Arc;

use flock_core::crd::{Crd, CrdKey, PrinterColumn};
use flock_core::diagnosis::{self, Condition};
use flock_core::related::{Fact, Related};
use flock_core::resources::Resource;
use flock_core::time::age as age_label;
use flock_core::{ResourceRef, Status, Uid};
use jiff::Timestamp;
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::api::DynamicObject;
use kube::core::ApiResource;
use serde_json::Value;

use crate::clock::{age, between};
use crate::error::{Error, Result};
use crate::jsonpath::Path;
use crate::manager::ClusterHandle;
use crate::ops::ObjectRef;
use crate::resources::CustomKind;

const NAMESPACED: &str = "Namespaced";
const CREATION_PATH: &str = ".metadata.creationTimestamp";
const DATE_TYPE: &str = "date";
const MISSING: &str = "";
const CONDITIONS_POINTER: &str = "/status/conditions";
const TYPE: &str = "type";
const STATUS: &str = "status";
const REASON: &str = "reason";
const MESSAGE: &str = "message";
const TRANSITION: &str = "lastTransitionTime";
const VERSION_SEPARATOR: char = '/';

pub fn definition(definition: &CustomResourceDefinition) -> Option<Crd> {
    let spec = &definition.spec;
    let storage = spec.versions.iter().find(|version| version.storage)?;
    let mut columns: Vec<PrinterColumn> = Vec::new();
    let shown = storage
        .additional_printer_columns
        .iter()
        .flatten()
        .filter(|column| column.priority.unwrap_or_default() == 0)
        .filter(|column| column.json_path.trim() != CREATION_PATH);
    for column in shown {
        if columns.iter().any(|known| known.name == column.name) {
            continue;
        }
        columns.push(PrinterColumn {
            name: column.name.clone(),
            json_path: column.json_path.clone(),
            date: column.type_ == DATE_TYPE,
        });
    }
    Some(Crd {
        key: CrdKey::new(spec.group.clone(), spec.names.kind.clone()),
        plural: spec.names.plural.clone(),
        version: storage.name.clone(),
        namespaced: spec.scope == NAMESPACED,
        columns,
    })
}

pub fn definitions(handle: &ClusterHandle) -> Vec<Crd> {
    let mut crds: Vec<Crd> = handle
        .objects::<CustomResourceDefinition>()
        .iter()
        .filter_map(|crd| definition(crd))
        .collect();
    crds.sort_by(|a, b| a.key.cmp(&b.key));
    crds
}

pub fn find(handle: &ClusterHandle, key: &CrdKey) -> Option<Crd> {
    handle
        .objects::<CustomResourceDefinition>()
        .iter()
        .filter(|crd| crd.spec.group == key.group && crd.spec.names.kind == key.kind)
        .find_map(|crd| definition(crd))
}

pub fn custom_kind(crd: &Crd) -> CustomKind {
    CustomKind::new(&crd.key.group, &crd.version, &crd.key.kind)
}

pub fn api_version(crd: &Crd) -> String {
    format!("{}{VERSION_SEPARATOR}{}", crd.key.group, crd.version)
}

pub fn api_resource(crd: &Crd) -> ApiResource {
    ApiResource {
        group: crd.key.group.clone(),
        version: crd.version.clone(),
        api_version: api_version(crd),
        kind: crd.key.kind.clone(),
        plural: crd.plural.clone(),
    }
}

fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => MISSING.to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::Array(_) => value.to_string(),
        Value::Object(_) => value.to_string(),
    }
}

pub fn cell(column: &PrinterColumn, path: Option<&Path>, object: &Value, now: Timestamp) -> String {
    let Some(found) = path.and_then(|path| path.first(object)) else {
        return MISSING.to_owned();
    };
    if column.date {
        let parsed = found.as_str().and_then(|at| at.parse::<Timestamp>().ok());
        if let Some(at) = parsed {
            return age_label(between(at, now));
        }
    }
    text(found)
}

pub fn conditions(object: &Value, now: Timestamp) -> Vec<Condition> {
    let items = object.pointer(CONDITIONS_POINTER).and_then(Value::as_array);
    items
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let field = |name: &str| item.get(name).and_then(Value::as_str).unwrap_or_default();
            let kind = field(TYPE);
            if kind.is_empty() {
                return None;
            }
            let changed = field(TRANSITION).parse::<Timestamp>().ok();
            let condition = Condition::new(kind, field(STATUS))
                .reason(field(REASON))
                .message(field(MESSAGE))
                .age(changed.map(|at| between(at, now)));
            Some(condition)
        })
        .collect()
}

pub struct Columns(Vec<Option<Path>>);

impl Columns {
    pub fn of(crd: &Crd) -> Self {
        Self(
            crd.columns
                .iter()
                .map(|column| Path::parse(&column.json_path))
                .collect(),
        )
    }
}

pub fn row(crd: &Crd, columns: &Columns, object: &DynamicObject, now: Timestamp) -> Resource {
    let value = serde_json::to_value(object).unwrap_or(Value::Null);
    let cells = crd
        .columns
        .iter()
        .zip(&columns.0)
        .map(|(column, path)| cell(column, path.as_ref(), &value, now))
        .collect();
    let meta = &object.metadata;
    let conditions = conditions(&value, now);
    let status = Status::worst(conditions.iter().map(Condition::status));
    let diagnosis =
        diagnosis::from_conditions(&conditions).filter(|_| diagnosis::needs_diagnosis(status));
    Resource {
        namespace: meta.namespace.clone().unwrap_or_default(),
        name: meta.name.clone().unwrap_or_default(),
        uid: Uid::new(meta.uid.clone().unwrap_or_default()),
        status,
        cells,
        age: age(meta, now),
        yaml: String::new(),
        labels: meta.labels.clone().unwrap_or_default(),
        conditions,
        diagnosis,
    }
}

pub fn rows(handle: &ClusterHandle, crd: &Crd, now: Timestamp) -> Vec<Resource> {
    let columns = Columns::of(crd);
    let mut rows: Vec<Resource> = handle
        .custom_objects(&custom_kind(crd))
        .iter()
        .map(|object| row(crd, &columns, object, now))
        .collect();
    rows.sort_by(|a, b| {
        a.namespace
            .cmp(&b.namespace)
            .then_with(|| a.name.cmp(&b.name))
    });
    rows
}

fn same(object: &DynamicObject, target: &ResourceRef) -> bool {
    object.metadata.name.as_deref() == Some(target.name.as_str())
        && object.metadata.namespace.as_deref() == target.namespace.as_deref()
}

pub fn object(handle: &ClusterHandle, target: &ResourceRef) -> Option<Arc<DynamicObject>> {
    let key = CrdKey::parse(&target.kind)?;
    let crd = find(handle, &key)?;
    handle
        .custom_objects(&custom_kind(&crd))
        .into_iter()
        .find(|object| same(object, target))
}

pub fn cached_version(handle: &ClusterHandle, target: &ResourceRef) -> Option<String> {
    object(handle, target)?.metadata.resource_version.clone()
}

pub fn object_ref(handle: &ClusterHandle, target: &ResourceRef) -> Result<ObjectRef> {
    let Some(key) = CrdKey::parse(&target.kind) else {
        return crate::actions::mutate::object_ref(target);
    };
    let crd = find(handle, &key).ok_or_else(|| Error::UnknownKind {
        api_version: key.group.clone(),
        kind: key.kind.clone(),
    })?;
    Ok(ObjectRef {
        api_version: api_version(&crd),
        kind: key.kind,
        namespace: target.namespace.clone(),
        name: target.name.clone(),
    })
}

pub fn related(
    handle: &ClusterHandle,
    scope: &crate::related::Scope<'_>,
    target: &ResourceRef,
) -> Option<Related> {
    let key = CrdKey::parse(&target.kind)?;
    let found = object(handle, target)?;
    let crd = find(handle, &key)?;
    let columns = Columns::of(&crd);
    let row = row(&crd, &columns, &found, scope.now);
    let mut facts = crate::related::facts(handle, scope, target.namespace.as_deref());
    let known = facts.iter().any(|fact| {
        fact.kind == target.kind && fact.name == target.name && fact.namespace == target.namespace
    });
    if known {
        return Some(flock_core::related::related(scope.cluster, &facts, target));
    }
    facts.push(Fact {
        status: row.status,
        age: row.age,
        labels: row.labels.clone(),
        owner: crate::related::owner(&found.metadata),
        ..Fact::new(&key.kind, target.namespace.as_deref(), &target.name)
    });
    let bare = ResourceRef {
        kind: key.kind,
        ..target.clone()
    };
    let mut related = flock_core::related::related(scope.cluster, &facts, &bare);
    related.target = target.clone();
    Some(related)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parse<T: serde::de::DeserializeOwned>(value: Value) -> T {
        let Ok(parsed) = serde_json::from_value(value) else {
            panic!("fixture");
        };
        parsed
    }

    fn now() -> Timestamp {
        "2026-10-19T12:00:00Z".parse().unwrap_or_default()
    }

    fn gateway_crd() -> CustomResourceDefinition {
        parse(json!({
            "metadata": { "name": "gateways.gateway.networking.k8s.io" },
            "spec": {
                "group": "gateway.networking.k8s.io",
                "names": { "kind": "Gateway", "plural": "gateways" },
                "scope": "Namespaced",
                "versions": [
                    { "name": "v1beta1", "served": true, "storage": false },
                    {
                        "name": "v1", "served": true, "storage": true,
                        "schema": { "openAPIV3Schema": { "type": "object" } },
                        "additionalPrinterColumns": [
                            { "name": "Class", "type": "string", "jsonPath": ".spec.gatewayClassName" },
                            { "name": "Address", "type": "string", "jsonPath": ".status.addresses[*].value" },
                            { "name": "Programmed", "type": "string", "jsonPath": ".status.conditions[?(@.type==\"Programmed\")].status" },
                            { "name": "Age", "type": "date", "jsonPath": ".metadata.creationTimestamp" },
                            { "name": "Listeners", "type": "integer", "priority": 1, "jsonPath": ".spec.listeners" },
                            { "name": "Class", "type": "string", "jsonPath": ".spec.gatewayClassName" },
                            { "name": "Resolved", "type": "date", "jsonPath": ".status.resolvedAt" }
                        ]
                    }
                ]
            }
        }))
    }

    #[test]
    fn definitions_read_the_storage_version_and_default_printer_columns() {
        let Some(crd) = definition(&gateway_crd()) else {
            panic!("definition");
        };
        assert_eq!(crd.key, CrdKey::new("gateway.networking.k8s.io", "Gateway"));
        assert_eq!(crd.version, "v1");
        assert!(crd.namespaced);
        let names: Vec<(&str, bool)> = crd
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c.date))
            .collect();
        assert_eq!(
            names,
            [
                ("Class", false),
                ("Address", false),
                ("Programmed", false),
                ("Resolved", true)
            ]
        );
        let resource = api_resource(&crd);
        assert_eq!(resource.api_version, "gateway.networking.k8s.io/v1");
        assert_eq!(resource.plural, "gateways");
        let mut cluster = gateway_crd();
        cluster.spec.scope = "Cluster".into();
        cluster
            .spec
            .versions
            .iter_mut()
            .for_each(|v| v.storage = false);
        assert_eq!(definition(&cluster), None);
    }

    #[test]
    fn rows_fill_printer_columns_status_and_whats_wrong_from_conditions() {
        let Some(crd) = definition(&gateway_crd()) else {
            panic!("definition");
        };
        let object: DynamicObject = parse(json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
            "metadata": {
                "name": "edge", "namespace": "loco-system", "uid": "u1",
                "creationTimestamp": "2026-10-19T10:00:00Z", "labels": { "app": "edge" }
            },
            "spec": { "gatewayClassName": "eg" },
            "status": {
                "addresses": [{ "value": "172.18.0.5" }, { "value": "172.18.0.6" }],
                "resolvedAt": "2026-10-19T11:58:00Z",
                "conditions": [
                    { "type": "Accepted", "status": "True", "reason": "Accepted" },
                    { "type": "Programmed", "status": "False", "reason": "AddressNotAssigned",
                      "message": "No addresses have been assigned", "lastTransitionTime": "2026-10-19T11:00:00Z" }
                ]
            }
        }));
        let row = row(&crd, &Columns::of(&crd), &object, now());
        assert_eq!(row.cells, ["eg", "172.18.0.5", "False", "2m"]);
        assert_eq!(row.status, Status::Err);
        assert_eq!(row.namespace, "loco-system");
        assert_eq!(row.age, std::time::Duration::from_secs(7200));
        assert_eq!(row.conditions.len(), 2);
        assert_eq!(
            row.diagnosis,
            Some(flock_core::Diagnosis::new(
                "AddressNotAssigned",
                "No addresses have been assigned"
            ))
        );
        let bare: DynamicObject = parse(json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
            "metadata": { "name": "plain" }
        }));
        let plain = super::row(&crd, &Columns::of(&crd), &bare, now());
        assert_eq!(plain.cells, ["", "", "", ""]);
        assert_eq!(plain.status, Status::Ok);
        assert_eq!(plain.diagnosis, None);
        assert_eq!(plain.namespace, "");
    }

    #[test]
    fn cells_format_numbers_flags_lists_and_unparsable_dates() {
        let value = json!({ "n": 3, "b": true, "l": ["a"], "d": "soon" });
        let column = |path: &str, date: bool| PrinterColumn {
            name: String::new(),
            json_path: path.into(),
            date,
        };
        let cell_at = |path: &str, date: bool| {
            let parsed = Path::parse(path);
            cell(&column(path, date), parsed.as_ref(), &value, now())
        };
        assert_eq!(cell_at(".n", false), "3");
        assert_eq!(cell_at(".b", false), "true");
        assert_eq!(cell_at(".l", false), "[\"a\"]");
        assert_eq!(cell_at(".d", true), "soon");
        assert_eq!(cell_at("bad path", false), "");
    }
}
