use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::{generated, redact};

const READY: &str = "Ready";
const FALSE: &str = "False";
const UNKNOWN: &str = "Unknown";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum ObjectKind {
    Kustomization,
    HelmRelease,
    GitRepository,
    HelmRepository,
    HelmChart,
    OCIRepository,
}

impl ObjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Kustomization => "Kustomization",
            Self::HelmRelease => "HelmRelease",
            Self::GitRepository => "GitRepository",
            Self::HelmRepository => "HelmRepository",
            Self::HelmChart => "HelmChart",
            Self::OCIRepository => "OCIRepository",
        }
    }

    pub fn api_version(self) -> &'static str {
        match self {
            Self::Kustomization => "kustomize.toolkit.fluxcd.io/v1",
            Self::HelmRelease => "helm.toolkit.fluxcd.io/v2",
            Self::GitRepository => "source.toolkit.fluxcd.io/v1",
            Self::HelmRepository => "source.toolkit.fluxcd.io/v1",
            Self::HelmChart => "source.toolkit.fluxcd.io/v1",
            Self::OCIRepository => "source.toolkit.fluxcd.io/v1",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ObjectIdentity {
    pub kind: ObjectKind,
    pub namespace: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ResourceRef {
    pub kind: String,
    pub namespace: String,
    pub name: String,
    pub ready_expr: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Condition {
    #[serde(rename = "type")]
    pub type_: String,
    pub status: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub message: String,
    #[serde(default, rename = "observedGeneration")]
    pub observed_generation: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct InventoryEntry {
    pub namespace: String,
    pub name: String,
    pub group: String,
    pub kind: String,
    pub version: String,
}

impl InventoryEntry {
    pub fn parse(id: &str, version: &str) -> Result<Self, ObjectError> {
        let parts: Vec<_> = id.split('_').collect();
        let [namespace, name, group, kind] = parts.as_slice() else {
            return Err(ObjectError::InvalidInventory(id.to_owned()));
        };
        if name.is_empty() || kind.is_empty() || version.is_empty() {
            return Err(ObjectError::InvalidInventory(id.to_owned()));
        }
        Ok(Self {
            namespace: (*namespace).to_owned(),
            name: (*name).to_owned(),
            group: (*group).to_owned(),
            kind: (*kind).to_owned(),
            version: version.to_owned(),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseSnapshot {
    pub name: String,
    pub namespace: String,
    pub version: i64,
    pub status: String,
    pub chart_name: String,
    pub chart_version: String,
    pub app_version: Option<String>,
    pub first_deployed: String,
    pub last_deployed: String,
    #[serde(default)]
    pub action: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct GitOpsObject {
    pub identity: ObjectIdentity,
    pub conditions: Vec<Condition>,
    pub suspended: bool,
    pub source_ref: Option<ResourceRef>,
    pub last_applied_revision: Option<String>,
    pub last_attempted_revision: Option<String>,
    pub artifact_revision: Option<String>,
    pub generation: Option<i64>,
    pub observed_generation: Option<i64>,
    pub inventory: Vec<InventoryEntry>,
    pub history: Vec<ReleaseSnapshot>,
    pub depends_on: Vec<ResourceRef>,
    pub interval: Option<String>,
    pub last_handled_reconcile_at: Option<String>,
    pub install_failures: i64,
    pub upgrade_failures: i64,
    pub spec: Value,
    pub last_failure: Option<Condition>,
}

#[derive(Debug, Error)]
pub enum ObjectError {
    #[error("invalid Flux object JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("missing or invalid field {0}")]
    InvalidField(&'static str),
    #[error("unsupported Flux API version {0}")]
    UnsupportedVersion(String),
    #[error("invalid inventory ID {0}")]
    InvalidInventory(String),
}

fn string(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_owned)
}

fn reference(value: &Value, namespace: &str, kind: &str) -> Result<ResourceRef, ObjectError> {
    let name = string(value, "name").filter(|name| !name.is_empty());
    let name = name.ok_or(ObjectError::InvalidField("reference.name"))?;
    let namespace = string(value, "namespace").unwrap_or_else(|| namespace.to_owned());
    let kind = string(value, "kind").unwrap_or_else(|| kind.to_owned());
    Ok(ResourceRef {
        kind,
        namespace,
        name,
        ready_expr: string(value, "readyExpr"),
    })
}

fn validate(kind: ObjectKind, value: &Value) -> Result<(), ObjectError> {
    match kind {
        ObjectKind::Kustomization => {
            serde_json::from_value::<generated::Kustomization>(value.clone())?;
        }
        ObjectKind::HelmRelease => {
            serde_json::from_value::<generated::HelmRelease>(value.clone())?;
        }
        ObjectKind::GitRepository => {
            serde_json::from_value::<generated::GitRepository>(value.clone())?;
        }
        ObjectKind::HelmRepository => {
            serde_json::from_value::<generated::HelmRepository>(value.clone())?;
        }
        ObjectKind::HelmChart => {
            serde_json::from_value::<generated::HelmChart>(value.clone())?;
        }
        ObjectKind::OCIRepository => {
            serde_json::from_value::<generated::OciRepository>(value.clone())?;
        }
    }
    Ok(())
}

fn source_reference(
    kind: ObjectKind,
    spec: &Value,
    status: &Value,
    namespace: &str,
) -> Result<Option<ResourceRef>, ObjectError> {
    let source = match kind {
        ObjectKind::HelmRelease => {
            let chart = status
                .get("helmChart")
                .and_then(Value::as_str)
                .and_then(|chart| chart.split_once('/'));
            if let Some(source) = spec.get("chartRef") {
                Some(reference(source, namespace, "HelmChart")?)
            } else if let Some((chart_namespace, chart_name)) = chart {
                Some(ResourceRef {
                    kind: "HelmChart".into(),
                    namespace: chart_namespace.into(),
                    name: chart_name.into(),
                    ready_expr: None,
                })
            } else {
                spec.pointer("/chart/spec/sourceRef")
                    .map(|source| reference(source, namespace, "HelmRepository"))
                    .transpose()?
            }
        }
        _ => spec
            .get("sourceRef")
            .map(|source| reference(source, namespace, "GitRepository"))
            .transpose()?,
    };
    Ok(source)
}

fn inventory(status: &Value) -> Result<Vec<InventoryEntry>, ObjectError> {
    let entries = status
        .pointer("/inventory/entries")
        .cloned()
        .map(serde_json::from_value::<Vec<Value>>)
        .transpose()?
        .unwrap_or_default();
    let inventory = entries
        .iter()
        .map(|entry| {
            let id = entry
                .get("id")
                .and_then(Value::as_str)
                .ok_or(ObjectError::InvalidField("inventory.id"))?;
            let version = entry
                .get("v")
                .and_then(Value::as_str)
                .ok_or(ObjectError::InvalidField("inventory.v"))?;
            InventoryEntry::parse(id, version)
        })
        .collect::<Result<_, _>>()?;
    Ok(inventory)
}

fn release_history(
    kind: ObjectKind,
    status: &Value,
) -> Result<(Vec<ReleaseSnapshot>, Option<String>), ObjectError> {
    let history: Vec<ReleaseSnapshot> = if kind == ObjectKind::HelmRelease {
        status
            .get("history")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let last_applied_revision = if kind == ObjectKind::HelmRelease {
        history
            .iter()
            .filter(|snapshot| snapshot.status == "deployed")
            .max_by_key(|snapshot| snapshot.version)
            .map(|snapshot| snapshot.chart_version.clone())
    } else {
        string(status, "lastAppliedRevision")
    };
    Ok((history, last_applied_revision))
}

impl GitOpsObject {
    pub fn from_json(value: &Value) -> Result<Self, ObjectError> {
        let kind: ObjectKind = serde_json::from_value(
            value
                .get("kind")
                .cloned()
                .ok_or(ObjectError::InvalidField("kind"))?,
        )?;
        let api_version =
            string(value, "apiVersion").ok_or(ObjectError::InvalidField("apiVersion"))?;
        if api_version != kind.api_version() {
            return Err(ObjectError::UnsupportedVersion(api_version));
        }
        validate(kind, value)?;
        let metadata = value.get("metadata").unwrap_or(&Value::Null);
        let name = string(metadata, "name")
            .filter(|name| !name.is_empty())
            .ok_or(ObjectError::InvalidField("metadata.name"))?;
        let namespace = string(metadata, "namespace")
            .filter(|namespace| !namespace.is_empty())
            .ok_or(ObjectError::InvalidField("metadata.namespace"))?;
        let spec = value
            .get("spec")
            .filter(|spec| spec.is_object())
            .cloned()
            .ok_or(ObjectError::InvalidField("spec"))?;
        let status = value.get("status").unwrap_or(&Value::Null);
        let conditions: Vec<Condition> = status
            .get("conditions")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default();
        let conditions = conditions
            .into_iter()
            .map(|condition| Condition {
                message: redact::credentials(&condition.message),
                ..condition
            })
            .collect();
        let source_ref = source_reference(kind, &spec, status, &namespace)?;
        let dependencies = spec
            .get("dependsOn")
            .cloned()
            .map(serde_json::from_value::<Vec<Value>>)
            .transpose()?
            .unwrap_or_default();
        let kind_name = string(value, "kind").ok_or(ObjectError::InvalidField("kind"))?;
        let depends_on = dependencies
            .iter()
            .map(|dependency| reference(dependency, &namespace, &kind_name))
            .collect::<Result<_, _>>()?;
        let inventory = inventory(status)?;
        let (history, last_applied_revision) = release_history(kind, status)?;
        let identity = ObjectIdentity {
            kind,
            namespace,
            name,
        };
        Ok(Self {
            identity,
            conditions,
            suspended: spec
                .get("suspend")
                .and_then(Value::as_bool)
                .unwrap_or_default(),
            source_ref,
            last_applied_revision,
            last_attempted_revision: string(status, "lastAttemptedRevision"),
            artifact_revision: status
                .pointer("/artifact/revision")
                .and_then(Value::as_str)
                .map(str::to_owned),
            generation: metadata.get("generation").and_then(Value::as_i64),
            observed_generation: status.get("observedGeneration").and_then(Value::as_i64),
            inventory,
            history,
            depends_on,
            interval: string(&spec, "interval"),
            last_handled_reconcile_at: string(status, "lastHandledReconcileAt"),
            install_failures: status
                .get("installFailures")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
            upgrade_failures: status
                .get("upgradeFailures")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
            spec,
            last_failure: None,
        })
    }

    pub fn condition(&self, type_: &str) -> Option<&Condition> {
        self.conditions
            .iter()
            .find(|condition| condition.type_ == type_)
    }

    pub fn ready(&self) -> Option<&Condition> {
        let live = self.condition(READY);
        let in_progress = live.is_some_and(|condition| condition.status == UNKNOWN);
        match &self.last_failure {
            Some(failure) if in_progress => Some(failure),
            _ => live,
        }
    }

    pub fn carry_failure(&mut self, remembered: Option<Condition>) -> Option<Condition> {
        let live = self.condition(READY)?;
        if live.status == FALSE {
            return Some(live.clone());
        }
        if live.status != UNKNOWN {
            return None;
        }
        self.last_failure.clone_from(&remembered);
        remembered
    }

    pub fn revision(&self) -> Option<&str> {
        self.last_applied_revision
            .as_deref()
            .or(self.artifact_revision.as_deref())
    }

    pub fn current(&self, condition: &Condition) -> bool {
        let observed = condition.observed_generation.or(self.observed_generation);
        match (self.generation, observed) {
            (Some(generation), Some(observed)) => observed >= generation,
            _ => true,
        }
    }
}

macro_rules! convert {
    ($type:ty) => {
        impl TryFrom<&$type> for GitOpsObject {
            type Error = ObjectError;
            fn try_from(object: &$type) -> Result<Self, Self::Error> {
                let value = serde_json::to_value(object)?;
                Self::from_json(&value)
            }
        }
    };
}

convert!(generated::Kustomization);
convert!(generated::HelmRelease);
convert!(generated::GitRepository);
convert!(generated::HelmRepository);
convert!(generated::HelmChart);
convert!(generated::OciRepository);
