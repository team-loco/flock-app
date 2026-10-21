use std::collections::BTreeMap;
use std::time::Duration;

use flock_core::diagnosis::needs_diagnosis;
use flock_core::flux::{
    FluxDetail, HelmDetail, HelmRevision, HistoryEntry, ManagedObject, Phase, Revision,
    SourceDetail, SpecRow,
};
use flock_core::{ClusterId, Condition, Diagnosis, FluxKey, FluxKind, FluxObject, Status, Uid};
use flock_flux::revision::{self, short};
use flock_flux::{
    GitOpsObject, InventoryEntry, ObjectKind, Summary, failing_condition, redact, summarize,
};
use jiff::Timestamp;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ManagedFieldsEntry;
use kube::api::DynamicObject;
use serde_json::Value;

use crate::flux::FluxHealth;
use crate::resources::owns_suspend;
const API_VERSION: &str = "apiVersion";
const KIND: &str = "kind";
const READY: &str = "Ready";
const SEPARATOR: &str = " · ";
const VALUES_MAX: usize = 40;
const DEFAULT_BRANCH: &str = "master";
const ANY_VERSION: &str = "*";
const LATEST_VERSION: &str = "latest version";
const SUCCEEDED_SUFFIX: &str = "Succeeded";
const UID_POINTER: &str = "/metadata/uid";
const MANAGED_FIELDS: &str = "/metadata/managedFields";
const LABELS_POINTER: &str = "/metadata/labels";

#[derive(Clone, Debug)]
pub struct Parsed {
    pub object: GitOpsObject,
    pub raw: Value,
}

impl Parsed {
    pub fn labels(&self) -> BTreeMap<String, String> {
        self.raw
            .pointer(LABELS_POINTER)
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.to_owned())))
            .collect()
    }

    pub fn kind(&self) -> ObjectKind {
        self.object.identity.kind
    }

    pub fn is(&self, kind: &str, namespace: &str, name: &str) -> bool {
        let identity = &self.object.identity;
        identity.kind.as_str() == kind && identity.namespace == namespace && identity.name == name
    }

    pub fn uid(&self) -> Option<&str> {
        self.raw.pointer(UID_POINTER).and_then(Value::as_str)
    }

    fn status(&self, pointer: &str) -> Option<&Value> {
        self.raw.pointer(&format!("/status{pointer}"))
    }

    fn spec_str(&self, pointer: &str) -> Option<&str> {
        self.object.spec.pointer(pointer).and_then(Value::as_str)
    }

    fn changed(&self, kind: &str) -> Option<Timestamp> {
        self.raw
            .pointer("/status/conditions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|condition| condition.get("type").and_then(Value::as_str) == Some(kind))
            .and_then(|condition| timestamp(condition.get("lastTransitionTime")))
    }

    pub(super) fn ready_since(&self) -> Option<Timestamp> {
        self.changed(READY)
    }

    fn conditions(&self, now: Timestamp) -> Vec<Condition> {
        self.object
            .conditions
            .iter()
            .map(|condition| {
                let age = self
                    .changed(&condition.type_)
                    .map(|then| elapsed(then, now));
                Condition::new(condition.type_.clone(), condition.status.clone())
                    .reason(condition.reason.clone())
                    .message(condition.message.clone())
                    .age(age)
            })
            .collect()
    }

    fn suspended_at(&self) -> Option<Timestamp> {
        self.raw
            .pointer(MANAGED_FIELDS)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|field| {
                serde_json::from_value::<ManagedFieldsEntry>((*field).clone())
                    .is_ok_and(|entry| owns_suspend(&entry))
            })
            .filter_map(|field| timestamp(field.get("time")))
            .max()
    }
}

pub fn parse(object: &DynamicObject, kind: ObjectKind) -> Option<Parsed> {
    let mut raw = serde_json::to_value(object).ok()?;
    let fields = raw.as_object_mut()?;
    fields
        .entry(API_VERSION)
        .or_insert_with(|| Value::from(kind.api_version()));
    fields
        .entry(KIND)
        .or_insert_with(|| Value::from(kind.as_str()));
    match GitOpsObject::from_json(&raw) {
        Ok(object) => Some(Parsed { object, raw }),
        Err(error) => {
            tracing::debug!(%error, kind = kind.as_str(), "skipping unreadable Flux object");
            None
        }
    }
}

pub fn timestamp(value: Option<&Value>) -> Option<Timestamp> {
    value?.as_str()?.parse().ok()
}

pub fn elapsed(then: Timestamp, now: Timestamp) -> Duration {
    Duration::try_from(now.duration_since(then)).unwrap_or_default()
}

pub const fn core_kind(kind: ObjectKind) -> Option<FluxKind> {
    match kind {
        ObjectKind::Kustomization => Some(FluxKind::Kustomization),
        ObjectKind::HelmRelease => Some(FluxKind::HelmRelease),
        ObjectKind::GitRepository => Some(FluxKind::GitRepository),
        ObjectKind::HelmRepository => Some(FluxKind::HelmRepository),
        ObjectKind::OCIRepository => Some(FluxKind::OciRepository),
        ObjectKind::HelmChart => None,
    }
}

pub const fn core_status(status: flock_flux::Status) -> Status {
    match status {
        flock_flux::Status::Ok => Status::Ok,
        flock_flux::Status::Warn => Status::Warn,
        flock_flux::Status::Err => Status::Err,
        flock_flux::Status::Progress => Status::Progress,
        flock_flux::Status::Suspended => Status::Suspended,
        flock_flux::Status::Offline => Status::Offline,
    }
}

pub fn key(cluster: &ClusterId, parsed: &Parsed) -> Option<FluxKey> {
    let identity = &parsed.object.identity;
    let kind = core_kind(identity.kind)?;
    Some(FluxKey {
        cluster: cluster.clone(),
        kind,
        namespace: identity.namespace.clone(),
        name: identity.name.clone(),
    })
}

pub fn source<'a>(parsed: &Parsed, objects: &'a [Parsed]) -> Option<&'a Parsed> {
    let reference = parsed.object.source_ref.as_ref()?;
    objects
        .iter()
        .find(|candidate| candidate.is(&reference.kind, &reference.namespace, &reference.name))
}

pub fn summary(parsed: &Parsed, objects: &[Parsed]) -> Summary {
    summarize(&parsed.object, source(parsed, objects).map(|s| &s.object))
}

pub fn health(parsed: &Parsed, objects: &[Parsed], now: Timestamp) -> FluxHealth {
    let summary = summary(parsed, objects);
    let identity = &parsed.object.identity;
    FluxHealth {
        kind: identity.kind,
        namespace: identity.namespace.clone(),
        name: identity.name.clone(),
        status: core_status(summary.status),
        sentence: summary.sentence,
        suspended: parsed.object.suspended,
        revision: parsed.object.revision().map(str::to_owned),
        since: parsed
            .suspended_at()
            .filter(|_| parsed.object.suspended)
            .or_else(|| parsed.ready_since())
            .map(|then| elapsed(then, now))
            .unwrap_or_default(),
    }
}

pub struct Inputs<'a> {
    pub cluster: &'a ClusterId,
    pub objects: &'a [Parsed],
    pub now: Timestamp,
    pub health: &'a dyn Fn(&InventoryEntry) -> Option<(Status, String)>,
    pub history: Option<Vec<HistoryEntry>>,
    pub releases: Option<Vec<HelmRevision>>,
    pub drift: Option<flock_core::flux::Drift>,
}

fn diagnosis(parsed: &Parsed, summary: &Summary, phase: &Phase) -> Option<Diagnosis> {
    if !needs_diagnosis(phase.status()) {
        return None;
    }
    let failing = failing_condition(&parsed.object).map(|condition| {
        let core = Condition::new(condition.type_.clone(), condition.status.clone())
            .reason(condition.reason.clone());
        Diagnosis::new(core.label(), condition.message.clone())
    });
    failing.or_else(|| {
        let reason = parsed
            .object
            .condition(READY)
            .map(|condition| condition.reason.clone())
            .unwrap_or_default();
        let message = summary
            .detail
            .clone()
            .unwrap_or_else(|| summary.sentence.clone());
        Some(Diagnosis::new(reason, message))
    })
}

pub fn flux_object(parsed: &Parsed, inputs: Inputs<'_>) -> Option<FluxObject> {
    let key = key(inputs.cluster, parsed)?;
    let summary = summary(parsed, inputs.objects);
    let now = inputs.now;
    let phase = phase(parsed, &summary, now);
    let applied = applied(parsed, now);
    let last_healthy = last_healthy(parsed, now);
    let source = source_key(parsed, inputs.objects, inputs.cluster);
    let depends_on = parsed
        .object
        .depends_on
        .iter()
        .filter_map(|reference| {
            let kind = FluxKind::from_label(&reference.kind)?;
            Some(FluxKey::new(
                inputs.cluster.as_str(),
                kind,
                reference.namespace.clone(),
                reference.name.clone(),
            ))
        })
        .collect();
    let inventory = parsed
        .object
        .inventory
        .iter()
        .map(|entry| managed(entry, inputs.health))
        .collect();
    let history = inputs
        .history
        .unwrap_or_else(|| reconcile_history(parsed, now));
    let detail = detail(parsed, now, inputs.releases, inputs.drift);
    let uid = parsed.uid().map(Uid::new).unwrap_or_default();
    let labels = parsed.labels();
    let diagnosis = diagnosis(parsed, &summary, &phase);
    let conditions = parsed.conditions(now);
    Some(FluxObject {
        labels,
        key,
        uid,
        phase,
        applied,
        last_healthy,
        source,
        depends_on,
        inventory,
        history,
        spec: spec_rows(parsed, inputs.objects),
        yaml: yaml(&parsed.raw),
        detail,
        conditions,
        diagnosis,
    })
}

fn phase(parsed: &Parsed, summary: &Summary, now: Timestamp) -> Phase {
    match summary.status {
        flock_flux::Status::Ok => Phase::Ready,
        flock_flux::Status::Progress => Phase::Reconciling,
        flock_flux::Status::Offline => Phase::Reconciling,
        flock_flux::Status::Warn => Phase::Retrying,
        flock_flux::Status::Err => Phase::Failing,
        flock_flux::Status::Suspended => Phase::Suspended {
            since: parsed.suspended_at().map(|then| elapsed(then, now)),
        },
    }
}

fn is_source(kind: ObjectKind) -> bool {
    matches!(
        kind,
        ObjectKind::GitRepository
            | ObjectKind::HelmRepository
            | ObjectKind::OCIRepository
            | ObjectKind::HelmChart
    )
}

fn applied_revision(parsed: &Parsed) -> Option<&str> {
    let object = &parsed.object;
    if is_source(parsed.kind()) {
        return object.artifact_revision.as_deref();
    }
    object
        .last_attempted_revision
        .as_deref()
        .or(object.last_applied_revision.as_deref())
}

pub(super) fn reconciled_at(parsed: &Parsed, revision: &str) -> Option<Timestamp> {
    match parsed.kind() {
        ObjectKind::HelmRelease => parsed
            .object
            .history
            .iter()
            .find(|snapshot| snapshot.chart_version == revision)
            .and_then(|snapshot| snapshot.last_deployed.parse().ok()),
        ObjectKind::Kustomization => parsed
            .status("/history")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|entry| {
                entry.pointer("/metadata/revision").and_then(Value::as_str) == Some(revision)
            })
            .and_then(|entry| timestamp(entry.get("firstReconciled"))),
        ObjectKind::GitRepository => None,
        ObjectKind::HelmRepository => None,
        ObjectKind::HelmChart => None,
        ObjectKind::OCIRepository => None,
    }
}

fn applied(parsed: &Parsed, now: Timestamp) -> Option<Revision> {
    let revision = applied_revision(parsed)?;
    let at = if is_source(parsed.kind()) {
        timestamp(parsed.status("/artifact/lastUpdateTime"))
    } else {
        reconciled_at(parsed, revision).or_else(|| parsed.ready_since())
    };
    let age = at.map(|then| elapsed(then, now)).unwrap_or_default();
    Some(Revision::new(short(revision), age))
}

fn last_healthy(parsed: &Parsed, now: Timestamp) -> Option<Revision> {
    if is_source(parsed.kind()) {
        return None;
    }
    let revision = parsed.object.last_applied_revision.as_deref()?;
    let age = reconciled_at(parsed, revision)
        .map(|then| elapsed(then, now))
        .unwrap_or_default();
    Some(Revision::new(short(revision), age))
}

fn source_key(parsed: &Parsed, objects: &[Parsed], cluster: &ClusterId) -> Option<FluxKey> {
    let mut found = source(parsed, objects)?;
    if found.kind() == ObjectKind::HelmChart {
        found = source(found, objects)?;
    }
    key(cluster, found)
}

pub fn git_reference(parsed: &Parsed) -> String {
    let spec = |key: &str| parsed.spec_str(&format!("/ref/{key}")).map(str::to_owned);
    spec("branch")
        .or_else(|| spec("tag"))
        .or_else(|| spec("semver"))
        .or_else(|| spec("name"))
        .or_else(|| spec("commit"))
        .or_else(|| {
            let artifact = parsed.object.artifact_revision.as_deref()?;
            revision::git(artifact).map(|parsed| parsed.reference.to_owned())
        })
        .unwrap_or_else(|| DEFAULT_BRANCH.to_owned())
}

fn reference_row(parsed: &Parsed) -> Option<String> {
    ["branch", "tag", "semver", "name", "commit", "digest"]
        .iter()
        .find_map(|key| {
            parsed
                .spec_str(&format!("/ref/{key}"))
                .map(|value| format!("{key} {value}"))
        })
}

fn every(parsed: &Parsed) -> Option<String> {
    let interval = parsed.spec_str("/interval")?;
    let timeout = parsed.spec_str("/timeout").map(|t| format!("timeout {t}"));
    let prune = (parsed.object.spec.get("prune").and_then(Value::as_bool) == Some(true))
        .then(|| "prunes deleted objects".to_owned());
    let parts: Vec<String> = std::iter::once(interval.to_owned())
        .chain(timeout)
        .chain(prune)
        .collect();
    Some(parts.join(SEPARATOR))
}

fn spec_rows(parsed: &Parsed, objects: &[Parsed]) -> Vec<SpecRow> {
    let mut rows = Vec::new();
    let mut push = |label: &str, value: Option<String>| {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            rows.push(SpecRow::new(label, value));
        }
    };
    match parsed.kind() {
        ObjectKind::Kustomization => {
            let source = source(parsed, objects);
            let name = parsed.object.source_ref.as_ref().map(|r| r.name.clone());
            let reference = source
                .filter(|s| s.kind() == ObjectKind::GitRepository)
                .map(git_reference);
            let label = match (name, reference) {
                (Some(name), Some(reference)) => Some(format!("{name}{SEPARATOR}{reference}")),
                (name, _) => name,
            };
            push("source", label);
            let path = parsed
                .spec_str("/path")
                .map(|p| p.trim_start_matches("./").to_owned());
            push("path", path);
            push("interval", every(parsed));
        }
        ObjectKind::HelmRelease => {
            push("chart", Some(chart_row(parsed)));
            let source = parsed
                .object
                .spec
                .pointer("/chart/spec/sourceRef/name")
                .or_else(|| parsed.object.spec.pointer("/chartRef/name"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            push("source", source);
            push("interval", every(parsed));
            push("upgrade", remediation(parsed));
        }
        ObjectKind::GitRepository => {
            push("url", parsed.spec_str("/url").map(redact::credentials));
            push("reference", reference_row(parsed));
            push("interval", every(parsed));
            push(
                "secret",
                parsed.spec_str("/secretRef/name").map(str::to_owned),
            );
        }
        ObjectKind::HelmRepository => {
            push("url", parsed.spec_str("/url").map(redact::credentials));
            push("interval", every(parsed));
        }
        ObjectKind::HelmChart => {
            push("chart", parsed.spec_str("/chart").map(str::to_owned));
            push("interval", every(parsed));
        }
        ObjectKind::OCIRepository => {
            push("url", parsed.spec_str("/url").map(redact::credentials));
            push("reference", reference_row(parsed));
            push("interval", every(parsed));
        }
    }
    rows
}

fn remediation(parsed: &Parsed) -> Option<String> {
    let spec = &parsed.object.spec;
    let retries = spec
        .pointer("/upgrade/remediation/retries")
        .and_then(Value::as_i64)
        .map(|n| format!("retries {n}"));
    let strategy = spec
        .pointer("/upgrade/remediation/strategy")
        .and_then(Value::as_str)
        .map(|s| format!("remediate {s}"));
    let parts: Vec<String> = retries.into_iter().chain(strategy).collect();
    (!parts.is_empty()).then(|| parts.join(SEPARATOR))
}

fn chart_row(parsed: &Parsed) -> String {
    let (chart, version) = chart(parsed);
    let resolved = parsed.object.last_attempted_revision.is_some();
    let version = match (resolved, version.as_str()) {
        (_, "") => String::new(),
        (true, resolved) => resolved.to_owned(),
        (false, ANY_VERSION) => LATEST_VERSION.to_owned(),
        (false, constraint) => format!("version matching {constraint}"),
    };
    [chart, version]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(SEPARATOR)
}

fn chart(parsed: &Parsed) -> (String, String) {
    let chart = parsed
        .spec_str("/chart/spec/chart")
        .or_else(|| parsed.spec_str("/chartRef/name"))
        .unwrap_or_default()
        .to_owned();
    let version = parsed
        .object
        .last_attempted_revision
        .clone()
        .or_else(|| parsed.spec_str("/chart/spec/version").map(str::to_owned))
        .unwrap_or_default();
    (chart, version)
}

fn detail(
    parsed: &Parsed,
    now: Timestamp,
    releases: Option<Vec<HelmRevision>>,
    drift: Option<flock_core::flux::Drift>,
) -> FluxDetail {
    match parsed.kind() {
        ObjectKind::Kustomization => FluxDetail::Kustomization(drift),
        ObjectKind::HelmRelease => {
            let (chart, version) = chart(parsed);
            let releases = releases.unwrap_or_else(|| status_releases(parsed, now));
            FluxDetail::HelmRelease(HelmDetail {
                chart,
                version,
                values: values(&parsed.object.spec),
                releases,
            })
        }
        ObjectKind::GitRepository => source_detail(parsed, git_reference(parsed), now),
        ObjectKind::OCIRepository => {
            let reference = reference_row(parsed).unwrap_or_default();
            source_detail(parsed, reference, now)
        }
        ObjectKind::HelmRepository => source_detail(parsed, String::new(), now),
        ObjectKind::HelmChart => source_detail(parsed, String::new(), now),
    }
}

fn source_detail(parsed: &Parsed, reference: String, now: Timestamp) -> FluxDetail {
    let fetched = timestamp(parsed.status("/artifact/lastUpdateTime"))
        .map(|then| elapsed(then, now))
        .unwrap_or_default();
    FluxDetail::Source(SourceDetail {
        url: redact::credentials(parsed.spec_str("/url").unwrap_or_default()),
        reference,
        fetched,
    })
}

pub fn release_status(state: &str) -> Status {
    match state {
        "deployed" => Status::Ok,
        "superseded" => Status::Ok,
        "failed" => Status::Err,
        "pending-install" => Status::Progress,
        "pending-upgrade" => Status::Progress,
        "pending-rollback" => Status::Progress,
        _ => Status::Offline,
    }
}

fn status_releases(parsed: &Parsed, now: Timestamp) -> Vec<HelmRevision> {
    let mut releases: Vec<HelmRevision> = parsed
        .object
        .history
        .iter()
        .filter_map(|snapshot| {
            let revision = u32::try_from(snapshot.version).ok()?;
            let deployed: Option<Timestamp> = snapshot.last_deployed.parse().ok();
            Some(HelmRevision {
                revision,
                status: release_status(&snapshot.status),
                state: snapshot.status.clone(),
                chart_version: snapshot.chart_version.clone(),
                app_version: snapshot.app_version.clone().unwrap_or_default(),
                description: String::new(),
                age: deployed.map(|then| elapsed(then, now)).unwrap_or_default(),
                diff: String::new(),
            })
        })
        .collect();
    releases.sort_by_key(|release| std::cmp::Reverse(release.revision));
    releases
}

pub fn values(spec: &Value) -> Vec<SpecRow> {
    let mut rows = Vec::new();
    if let Some(values) = spec.get("values") {
        flatten(String::new(), values, &mut rows);
    }
    rows.truncate(VALUES_MAX);
    let sources: Vec<&str> = spec
        .get("valuesFrom")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|source| source.get("name").and_then(Value::as_str))
        .collect();
    if !sources.is_empty() {
        rows.push(SpecRow::new("valuesFrom", sources.join(", ")));
    }
    rows
}

fn flatten(prefix: String, value: &Value, rows: &mut Vec<SpecRow>) {
    let join = |key: &str| {
        if prefix.is_empty() {
            key.to_owned()
        } else {
            format!("{prefix}.{key}")
        }
    };
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                flatten(join(key), child, rows);
            }
        }
        Value::Array(items) => {
            for (ix, child) in items.iter().enumerate() {
                flatten(format!("{prefix}[{ix}]"), child, rows);
            }
        }
        Value::String(text) => rows.push(SpecRow::new(prefix, text.clone())),
        Value::Null => rows.push(SpecRow::new(prefix, "null")),
        Value::Bool(flag) => rows.push(SpecRow::new(prefix, flag.to_string())),
        Value::Number(number) => rows.push(SpecRow::new(prefix, number.to_string())),
    }
}

fn managed(
    entry: &InventoryEntry,
    health: &dyn Fn(&InventoryEntry) -> Option<(Status, String)>,
) -> ManagedObject {
    let (status, detail) = health(entry).unwrap_or((Status::Ok, String::new()));
    ManagedObject {
        status,
        kind: entry.kind.clone(),
        namespace: (!entry.namespace.is_empty()).then(|| entry.namespace.clone()),
        name: entry.name.clone(),
        detail,
        changed: false,
    }
}

fn words(reason: &str) -> String {
    let mut out = String::new();
    for (ix, c) in reason.chars().enumerate() {
        if c.is_uppercase() && ix > 0 {
            out.push(' ');
        }
        out.extend(c.to_lowercase());
    }
    out
}

pub fn note(summary: &Summary, object: &GitOpsObject) -> String {
    match summary.status {
        flock_flux::Status::Ok => "ready".to_owned(),
        flock_flux::Status::Suspended => "suspended".to_owned(),
        flock_flux::Status::Warn => reason_note(summary, object),
        flock_flux::Status::Err => reason_note(summary, object),
        flock_flux::Status::Progress => reason_note(summary, object),
        flock_flux::Status::Offline => reason_note(summary, object),
    }
}

fn reason_note(summary: &Summary, object: &GitOpsObject) -> String {
    let reason = object
        .ready()
        .map(|condition| condition.reason.as_str())
        .filter(|reason| !reason.is_empty());
    match reason {
        Some(reason) => words(reason),
        None => summary.sentence.trim_end_matches('.').to_lowercase(),
    }
}

fn reconcile_history(parsed: &Parsed, now: Timestamp) -> Vec<HistoryEntry> {
    if parsed.kind() != ObjectKind::Kustomization {
        return Vec::new();
    }
    let mut history: Vec<HistoryEntry> = parsed
        .status("/history")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|entry| history_entry(entry, now))
        .collect();
    let failing = parsed
        .object
        .ready()
        .filter(|condition| condition.status == "False");
    if let Some(condition) = failing {
        let revision = parsed
            .object
            .last_attempted_revision
            .as_deref()
            .map(short)
            .unwrap_or_default();
        history.insert(
            0,
            HistoryEntry {
                age: Some(
                    parsed
                        .ready_since()
                        .map(|then| elapsed(then, now))
                        .unwrap_or_default(),
                ),
                status: Status::Err,
                revision,
                message: condition
                    .message
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
                count: 1,
            },
        );
    }
    history
}

fn history_entry(entry: &Value, now: Timestamp) -> HistoryEntry {
    let outcome = entry
        .get("lastReconciledStatus")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let ok = outcome.ends_with(SUCCEEDED_SUFFIX);
    let message = if ok {
        "applied".to_owned()
    } else {
        words(outcome)
    };
    let revision = entry
        .pointer("/metadata/revision")
        .and_then(Value::as_str)
        .map(short)
        .unwrap_or_default();
    HistoryEntry {
        age: timestamp(entry.get("lastReconciled")).map(|then| elapsed(then, now)),
        status: if ok { Status::Ok } else { Status::Err },
        revision,
        message,
        count: entry
            .get("totalReconciliations")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(1),
    }
}

pub fn yaml(raw: &Value) -> String {
    let mut value = raw.clone();
    redact::object(&mut value);
    serde_norway::to_string(&value).unwrap_or_default()
}

#[cfg(test)]
pub(super) mod tests {
    use serde_json::json;

    use super::*;

    pub(in crate::gitops) fn parsed(value: &Value) -> Parsed {
        let object = match GitOpsObject::from_json(value) {
            Ok(object) => object,
            Err(error) => panic!("flux fixture: {error}"),
        };
        Parsed {
            object,
            raw: value.clone(),
        }
    }

    pub(in crate::gitops) fn kustomization(name: &str, ready: &str, revision: &str) -> Value {
        json!({
            "apiVersion": "kustomize.toolkit.fluxcd.io/v1",
            "kind": "Kustomization",
            "metadata": { "name": name, "namespace": "flux-system", "generation": 1, "labels": { "team": "platform" } },
            "spec": {
                "interval": "10m", "prune": true, "path": "./kustomize",
                "sourceRef": { "kind": "GitRepository", "name": "podinfo" }
            },
            "status": {
                "observedGeneration": 1,
                "conditions": [{
                    "type": "Ready", "status": ready, "reason": "HealthCheckFailed",
                    "message": "timeout waiting for Deployment/web/podinfo\nmore",
                    "lastTransitionTime": "2026-10-13T09:00:00Z"
                }],
                "lastAppliedRevision": format!("master@sha1:{revision}"),
                "lastAttemptedRevision": format!("master@sha1:{revision}"),
                "history": [{
                    "digest": "sha256:9aa9",
                    "metadata": { "revision": format!("master@sha1:{revision}") },
                    "firstReconciled": "2026-10-13T08:00:00Z",
                    "lastReconciled": "2026-10-13T09:50:00Z",
                    "lastReconciledDuration": "288ms",
                    "lastReconciledStatus": "ReconciliationSucceeded",
                    "totalReconciliations": 72
                }],
                "inventory": { "entries": [
                    { "id": "web_podinfo_apps_Deployment", "v": "v1" },
                    { "id": "_web__Namespace", "v": "v1" }
                ]}
            }
        })
    }

    pub(in crate::gitops) fn git_repository() -> Value {
        json!({
            "apiVersion": "source.toolkit.fluxcd.io/v1",
            "kind": "GitRepository",
            "metadata": { "name": "podinfo", "namespace": "flux-system" },
            "spec": {
                "interval": "1m", "timeout": "60s",
                "url": "https://github.com/stefanprodan/podinfo",
                "ref": { "branch": "master" }
            },
            "status": {
                "conditions": [{
                    "type": "Ready", "status": "True", "reason": "Succeeded", "message": "",
                    "lastTransitionTime": "2026-10-13T09:56:00Z"
                }],
                "artifact": {
                    "digest": "sha256:903f", "path": "p", "url": "u",
                    "revision": "master@sha1:3e0ff8ae123b710bc91de1315cba0f996a8896c2",
                    "lastUpdateTime": "2026-10-13T09:56:00Z"
                }
            }
        })
    }

    fn now() -> Timestamp {
        "2026-10-13T10:00:00Z".parse().unwrap_or_default()
    }

    fn convert(value: &Value, objects: &[Parsed]) -> Option<FluxObject> {
        let cluster = ClusterId::new("fleet-a");
        let healthy = |entry: &InventoryEntry| {
            (entry.kind == "Deployment").then(|| (Status::Warn, "0/1 ready".to_owned()))
        };
        flux_object(
            &parsed(value),
            Inputs {
                cluster: &cluster,
                objects,
                now: now(),
                health: &healthy,
                history: None,
                releases: None,
                drift: None,
            },
        )
    }

    #[test]
    fn chart_rows_read_in_words_without_dangling_separators() {
        let release = |spec: Value, revision: Option<&str>| {
            let mut status = json!({});
            if let Some(revision) = revision {
                status = json!({"lastAttemptedRevision": revision});
            }
            let value = json!({
                "apiVersion": "helm.toolkit.fluxcd.io/v2",
                "kind": "HelmRelease",
                "metadata": {"name": "podinfo", "namespace": "flux-system"},
                "spec": spec,
                "status": status,
            });
            chart_row(&parsed(&value))
        };
        let source_ref = || json!({"kind": "HelmRepository", "name": "repo"});
        let chart = |version: Option<&str>| match version {
            Some(v) => {
                json!({"interval": "1m", "chart": {"spec": {"chart": "podinfo", "version": v, "sourceRef": source_ref()}}})
            }
            None => {
                json!({"interval": "1m", "chart": {"spec": {"chart": "podinfo", "sourceRef": source_ref()}}})
            }
        };
        assert_eq!(release(chart(Some("*")), None), "podinfo · latest version");
        assert_eq!(
            release(chart(Some("^6.0")), None),
            "podinfo · version matching ^6.0"
        );
        assert_eq!(release(chart(Some("*")), Some("6.5.1")), "podinfo · 6.5.1");
        assert_eq!(release(chart(None), None), "podinfo");
    }

    #[test]
    fn suspended_objects_age_from_whoever_last_wrote_spec_suspend() {
        let mut value = kustomization("apps", "True", "3e0ff8ae123b");
        value["spec"]["suspend"] = json!(true);
        let unknown = convert(&value, &[]).map(|object| object.phase);
        assert_eq!(unknown, Some(Phase::Suspended { since: None }));
        value["metadata"]["managedFields"] = json!([
            {"manager": "kubectl", "operation": "Update", "time": "2026-10-13T09:59:00Z",
             "fieldsType": "FieldsV1", "fieldsV1": {"f:spec": {"f:interval": {}}}},
            {"manager": "flux", "operation": "Update", "time": "2026-10-11T10:00:00Z",
             "fieldsType": "FieldsV1", "fieldsV1": {"f:spec": {"f:suspend": {}}}}
        ]);
        let two_days = Some(Duration::from_hours(48));
        let known = convert(&value, &[]).map(|object| object.phase);
        assert_eq!(known, Some(Phase::Suspended { since: two_days }));
        let health = health(&parsed(&value), &[], now());
        assert_eq!(Some(health.since), two_days);
    }

    #[test]
    fn failing_flux_objects_carry_their_reason_and_condition_ages() {
        let objects = [parsed(&git_repository())];
        let failing = convert(&kustomization("apps", "False", "abc"), &objects);
        let Some(failing) = failing else {
            panic!("converted");
        };
        assert_eq!(
            failing.diagnosis,
            Some(Diagnosis::new(
                "HealthCheckFailed",
                "timeout waiting for Deployment/web/podinfo\nmore"
            ))
        );
        assert_eq!(failing.conditions.len(), 1);
        assert_eq!(failing.conditions[0].age, Some(Duration::from_hours(1)));
        let ready = convert(&kustomization("apps", "True", "abc"), &objects);
        assert_eq!(ready.and_then(|object| object.diagnosis), None);
    }

    #[test]
    fn flux_rows_retain_metadata_labels_for_filtering() {
        let value = kustomization("apps", "True", "abc");
        let row = convert(&value, &[]);
        let team = row.as_ref().and_then(|row| row.labels.get("team"));
        assert_eq!(team.map(String::as_str), Some("platform"));
    }

    #[test]
    fn healthy_kustomization_converts_like_the_design() {
        let objects = [parsed(&git_repository())];
        let object = convert(&kustomization("apps", "True", "3e0ff8ae123b"), &objects);
        let Some(object) = object else {
            panic!("converted");
        };
        assert_eq!(object.phase, Phase::Ready);
        assert_eq!(
            object.source,
            Some(FluxKey::new(
                "fleet-a",
                FluxKind::GitRepository,
                "flux-system",
                "podinfo"
            ))
        );
        let spec: Vec<(&str, &str)> = object
            .spec
            .iter()
            .map(|row| (row.label.as_str(), row.value.as_str()))
            .collect();
        assert_eq!(
            spec,
            [
                ("source", "podinfo · master"),
                ("path", "kustomize"),
                ("interval", "10m · prunes deleted objects"),
            ]
        );
        let inventory: Vec<(Status, String, Option<String>)> = object
            .inventory
            .iter()
            .map(|item| (item.status, item.path(), item.namespace.clone()))
            .collect();
        assert_eq!(
            inventory,
            [
                (
                    Status::Warn,
                    "web/podinfo".to_owned(),
                    Some("web".to_owned())
                ),
                (Status::Ok, "web".to_owned(), None),
            ]
        );
        assert_eq!(object.history.len(), 1);
        assert_eq!(object.history[0].count_label().as_deref(), Some("72 times"));
        assert_eq!(object.history[0].message, "applied");
        assert!(!object.yaml.contains("managedFields"));
    }

    #[test]
    fn failing_kustomization_leads_history_with_the_failure() {
        let object = convert(&kustomization("apps", "False", "3e0ff8ae123b"), &[]);
        let Some(object) = object else {
            panic!("converted");
        };
        assert_eq!(object.phase, Phase::Failing);
        assert_eq!(object.history[0].status, Status::Err);
        assert_eq!(object.history[0].age_label(), "1h");
    }

    #[test]
    fn sources_report_their_artifact() {
        let object = convert(&git_repository(), &[]);
        let Some(object) = object else {
            panic!("converted");
        };
        assert_eq!(
            object.applied.as_ref().map(|r| r.id.as_str()),
            Some("3e0ff8a")
        );
        assert_eq!(
            object.detail,
            FluxDetail::Source(SourceDetail {
                url: "https://github.com/stefanprodan/podinfo".to_owned(),
                reference: "master".to_owned(),
                fetched: Duration::from_mins(4),
            })
        );
    }

    #[test]
    fn values_flatten_to_dotted_rows() {
        let spec = json!({
            "values": { "replicaCount": 2, "image": { "tag": "6.15.0" }, "args": ["--x"] },
            "valuesFrom": [{ "kind": "ConfigMap", "name": "extra" }]
        });
        let mut rows: Vec<(String, String)> = values(&spec)
            .into_iter()
            .map(|row| (row.label, row.value))
            .collect();
        rows.sort();
        assert_eq!(
            rows,
            [
                ("args[0]".to_owned(), "--x".to_owned()),
                ("image.tag".to_owned(), "6.15.0".to_owned()),
                ("replicaCount".to_owned(), "2".to_owned()),
                ("valuesFrom".to_owned(), "extra".to_owned()),
            ]
        );
    }

    #[test]
    fn notes_turn_reasons_into_words() {
        let failing = parsed(&kustomization("apps", "False", "abc1234"));
        let summary = summary(&failing, &[]);
        assert_eq!(note(&summary, &failing.object), "health check failed");
    }
}
