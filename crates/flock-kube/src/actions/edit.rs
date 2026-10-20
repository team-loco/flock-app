use flock_core::Uid;
use flock_core::edit::{
    Conflict, Draft, EditEvent, EditRequest, EditStep, Live, Rebased, diff_lines,
};
use flock_flux::redact::{self, CHANGED, MASK};
use kube::api::{Patch, PatchParams};
use serde_json::{Map, Value};

use super::failure::action_failure;
use crate::error::{Error, Result};
use crate::manager::ClusterHandle;
use crate::ops::{FIELD_MANAGER, ObjectRef};

const HTTP_CONFLICT: u16 = 409;
const METADATA: &str = "metadata";
const STATUS: &str = "status";
const ANNOTATIONS: &str = "annotations";
const UID: &str = "uid";
const RESOURCE_VERSION: &str = "resourceVersion";
const UID_POINTER: &str = "/metadata/uid";
const RESOURCE_VERSION_POINTER: &str = "/metadata/resourceVersion";
const LAST_APPLIED: &str = "kubectl.kubernetes.io/last-applied-configuration";
const SERVER_METADATA: [&str; 8] = [
    "managedFields",
    "resourceVersion",
    "uid",
    "creationTimestamp",
    "generation",
    "selfLink",
    "deletionTimestamp",
    "deletionGracePeriodSeconds",
];
const WHOLE_OBJECT: &str = "the object";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    DryRun,
    Apply,
}

pub fn strip(object: &Value) -> Value {
    let mut value = object.clone();
    let Some(fields) = value.as_object_mut() else {
        return value;
    };
    fields.retain(|key, _| key != STATUS);
    if let Some(metadata) = fields.get_mut(METADATA).and_then(Value::as_object_mut) {
        metadata.retain(|key, _| !SERVER_METADATA.contains(&key.as_str()));
        let annotations = metadata.get_mut(ANNOTATIONS).and_then(Value::as_object_mut);
        let emptied = annotations.is_some_and(|annotations| {
            annotations.retain(|key, _| key != LAST_APPLIED);
            annotations.is_empty()
        });
        if emptied {
            metadata.retain(|key, _| key != ANNOTATIONS);
        }
    }
    value
}

pub fn shown(object: &Value, revealed: bool) -> Value {
    let mut value = strip(object);
    if !revealed {
        redact::object(&mut value);
    }
    value
}

fn shown_against(object: &Value, previous: &Value, revealed: bool) -> Value {
    let mut value = strip(object);
    if !revealed {
        redact::object_against(&mut value, &strip(previous));
    }
    value
}

fn text(value: &Value) -> Result<String> {
    Ok(serde_norway::to_string(value)?)
}

fn parse(text: &str) -> Result<Value> {
    Ok(serde_norway::from_str(text)?)
}

fn field<'a>(object: &'a Value, pointer: &str, name: &'static str) -> Result<&'a str> {
    object
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or(Error::MissingField(name))
}

pub fn live(object: &Value, revealed: bool) -> Result<Live> {
    let uid = field(object, UID_POINTER, "metadata.uid")?;
    let resource_version = field(object, RESOURCE_VERSION_POINTER, "metadata.resourceVersion")?;
    Ok(Live {
        text: text(&shown(object, revealed))?,
        uid: Uid::new(uid),
        resource_version: resource_version.to_owned(),
        revealed,
    })
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

fn index(path: &str, ix: usize) -> String {
    format!("{path}[{ix}]")
}

fn named(path: &str) -> String {
    if path.is_empty() {
        WHOLE_OBJECT.to_owned()
    } else {
        path.to_owned()
    }
}

fn masked(text: &str) -> bool {
    text == MASK || text == CHANGED
}

fn placeholder_free(value: &Value, path: &str) -> Result<()> {
    match value {
        Value::String(text) if text.contains(MASK) => Err(Error::Redacted(named(path))),
        Value::Object(fields) => fields
            .iter()
            .try_for_each(|(key, child)| placeholder_free(child, &join(path, key))),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .try_for_each(|(ix, child)| placeholder_free(child, &index(path, ix))),
        Value::String(_) => Ok(()),
        Value::Null => Ok(()),
        Value::Bool(_) => Ok(()),
        Value::Number(_) => Ok(()),
    }
}

pub fn restore(
    edited: &Value,
    shown: Option<&Value>,
    actual: Option<&Value>,
    path: &str,
) -> Result<Value> {
    if let Some(actual) = actual.filter(|_| shown == Some(edited)) {
        return Ok(actual.clone());
    }
    match (edited, shown, actual) {
        (Value::Object(edited), Some(Value::Object(shown)), Some(Value::Object(actual))) => {
            let mut fields = Map::new();
            for (key, child) in edited {
                let restored = restore(child, shown.get(key), actual.get(key), &join(path, key))?;
                fields.insert(key.clone(), restored);
            }
            Ok(Value::Object(fields))
        }
        (Value::Array(edited), Some(Value::Array(shown)), Some(Value::Array(actual))) => edited
            .iter()
            .enumerate()
            .map(|(ix, child)| restore(child, shown.get(ix), actual.get(ix), &index(path, ix)))
            .collect::<Result<Vec<Value>>>()
            .map(Value::Array),
        (Value::String(_), Some(Value::String(shown)), Some(Value::String(actual)))
            if shown != actual && !masked(shown) =>
        {
            Err(Error::Redacted(named(path)))
        }
        _ => {
            placeholder_free(edited, path)?;
            Ok(edited.clone())
        }
    }
}

fn pin(value: &mut Value, base: &Live) {
    let Some(fields) = value.as_object_mut() else {
        return;
    };
    let metadata = fields
        .entry(METADATA)
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(metadata) = metadata.as_object_mut() {
        metadata.insert(UID.to_owned(), Value::from(base.uid.as_str()));
        metadata.insert(
            RESOURCE_VERSION.to_owned(),
            Value::from(base.resource_version.as_str()),
        );
    }
}

pub fn payload(draft: &Draft, current: &Value) -> Result<Value> {
    let edited = parse(&draft.text)?;
    let seen = shown(current, draft.base.revealed);
    let actual = strip(current);
    let mut value = restore(&edited, Some(&seen), Some(&actual), "")?;
    pin(&mut value, &draft.base);
    Ok(value)
}

pub fn rebase(base: &Value, mine: &Value, theirs: &Value) -> (Value, Vec<String>) {
    let mut contested = Vec::new();
    let merged = merge(Some(base), Some(mine), Some(theirs), "", &mut contested);
    (merged.unwrap_or(Value::Null), contested)
}

fn merge(
    base: Option<&Value>,
    mine: Option<&Value>,
    theirs: Option<&Value>,
    path: &str,
    contested: &mut Vec<String>,
) -> Option<Value> {
    if mine == base {
        return theirs.cloned();
    }
    if theirs == base || mine == theirs {
        return mine.cloned();
    }
    if let (Some(Value::Object(mine)), Some(Value::Object(theirs))) = (mine, theirs) {
        let base = base.and_then(Value::as_object);
        let keys = theirs
            .keys()
            .chain(mine.keys().filter(|key| !theirs.contains_key(*key)));
        let mut fields = Map::new();
        for key in keys {
            let before = base.and_then(|base| base.get(key));
            let child = join(path, key);
            if let Some(value) = merge(before, mine.get(key), theirs.get(key), &child, contested) {
                fields.insert(key.clone(), value);
            }
        }
        return Some(Value::Object(fields));
    }
    contested.push(named(path));
    mine.cloned()
}

fn same_object(current: &Value, base: &Live) -> bool {
    let uid = current.pointer(UID_POINTER).and_then(Value::as_str);
    let version = current
        .pointer(RESOURCE_VERSION_POINTER)
        .and_then(Value::as_str);
    uid == Some(base.uid.as_str()) && version == Some(base.resource_version.as_str())
}

fn current_base(current: &Value, base: &Live) -> Result<Option<Live>> {
    if same_object(current, base) {
        return Ok(Some(base.clone()));
    }
    let now = live(current, base.revealed)?;
    let unchanged = parse(&base.text)? == shown(current, base.revealed);
    Ok((now.uid == base.uid && unchanged).then_some(now))
}

fn is_conflict(error: &Error) -> bool {
    matches!(error, Error::Kube(kube::Error::Api(status)) if status.code == HTTP_CONFLICT)
}

pub fn conflict(draft: &Draft, current: &Value) -> Result<EditEvent> {
    let revealed = draft.base.revealed;
    let live = live(current, revealed)?;
    let base = parse(&draft.base.text)?;
    let mine = parse(&draft.text)?;
    let theirs = shown(current, revealed);
    let (merged, contested) = rebase(&base, &mine, &theirs);
    let changed = diff_lines(&draft.base.text, &live.text);
    let rebased = Rebased {
        text: text(&merged)?,
        contested,
    };
    Ok(EditEvent::Conflict(Box::new(Conflict {
        live,
        changed,
        rebased,
    })))
}

async fn current(handle: &ClusterHandle, target: &ObjectRef) -> Result<Value> {
    let object = handle.get(target).await?;
    Ok(serde_json::to_value(&object)?)
}

async fn send(
    handle: &ClusterHandle,
    target: &ObjectRef,
    body: &Value,
    mode: Mode,
) -> Result<Value> {
    let (_, api) = handle.dynamic_api(target).await?;
    let params = PatchParams::apply(FIELD_MANAGER).force();
    let params = match mode {
        Mode::DryRun => params.dry_run(),
        Mode::Apply => params,
    };
    let object = api
        .patch(&target.name, &params, &Patch::Apply(body))
        .await?;
    Ok(serde_json::to_value(&object)?)
}

async fn submit(
    handle: &ClusterHandle,
    target: &ObjectRef,
    draft: &Draft,
    mode: Mode,
) -> Result<EditEvent> {
    let before = current(handle, target).await?;
    let Some(base) = current_base(&before, &draft.base)? else {
        return conflict(draft, &before);
    };
    let draft = &Draft {
        text: draft.text.clone(),
        base,
    };
    let body = payload(draft, &before)?;
    let result = match send(handle, target, &body, mode).await {
        Ok(result) => result,
        Err(error) if is_conflict(&error) => {
            let now = current(handle, target).await?;
            return conflict(draft, &now);
        }
        Err(error) => return Err(error),
    };
    let revealed = draft.base.revealed;
    match mode {
        Mode::DryRun => {
            let old = text(&shown(&before, revealed))?;
            let new = text(&shown_against(&result, &before, revealed))?;
            Ok(EditEvent::Reviewed(diff_lines(&old, &new)))
        }
        Mode::Apply => Ok(EditEvent::Applied(live(&result, revealed)?)),
    }
}

async fn step(handle: &ClusterHandle, request: &EditRequest) -> Result<EditEvent> {
    let target = crate::crd::object_ref(handle, &request.target)?;
    match &request.step {
        EditStep::Load { reveal } => {
            let object = current(handle, &target).await?;
            Ok(EditEvent::Loaded(live(&object, *reveal)?))
        }
        EditStep::Review(draft) => submit(handle, &target, draft, Mode::DryRun).await,
        EditStep::Apply(draft) => submit(handle, &target, draft, Mode::Apply).await,
    }
}

pub async fn run(handle: ClusterHandle, request: EditRequest) {
    let outcome = handle.until_closed(step(&handle, &request)).await;
    let event = match outcome.unwrap_or(Err(Error::NotConnected)) {
        Ok(event) => event,
        Err(error) => EditEvent::Failed(action_failure(&error).line()),
    };
    request.sink.send_blocking(event);
}

pub fn fail(request: &EditRequest) {
    request
        .sink
        .send_blocking(EditEvent::Failed(Error::NotConnected.to_string()));
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use flock_core::{ClusterId, ResourceRef, Sink};
    use http::{Method, StatusCode};
    use k8s_openapi::api::core::v1::{ConfigMap, Secret};
    use kube::core::ApiResource;
    use serde_json::json;

    use super::*;
    use crate::manager::lock;

    const PLANTED: &str = "cGxhbnRlZC1wYXNzd29yZA==";
    const SECRET_PATH: &str = "/api/v1/namespaces/apps/secrets/db";
    const CONFIGMAP_PATH: &str = "/api/v1/namespaces/apps/configmaps/settings";

    fn secret(version: &str) -> Value {
        json!({
            "apiVersion": "v1",
            "kind": "Secret",
            "metadata": {
                "name": "db",
                "namespace": "apps",
                "uid": "u1",
                "resourceVersion": version,
                "creationTimestamp": "2026-10-01T00:00:00Z",
                "labels": { "app": "db" },
                "annotations": { LAST_APPLIED: format!("{{\"kind\":\"Secret\",\"data\":{{\"password\":\"{PLANTED}\"}}}}") },
                "managedFields": [{ "manager": "kubectl" }],
            },
            "type": "Opaque",
            "data": { "password": PLANTED, "user": "YWRtaW4=" },
        })
    }

    fn configmap(version: &str, level: &str, replicas: u32) -> Value {
        json!({
            "apiVersion": "v1",
            "kind": "ConfigMap",
            "metadata": {
                "name": "settings",
                "namespace": "apps",
                "uid": "u1",
                "resourceVersion": version,
                "generation": 3,
            },
            "data": { "level": level, "replicas": replicas.to_string(), "url": "https://bot:pw@git.example.com/repo" },
        })
    }

    fn draft_from(object: &Value, edit: impl Fn(&str) -> String) -> Draft {
        let Ok(base) = live(object, false) else {
            panic!("live object");
        };
        let text = edit(&base.text);
        Draft { base, text }
    }

    fn secret_ref() -> ResourceRef {
        ResourceRef {
            cluster: ClusterId::new("kind-fleet-a"),
            kind: "Secret".into(),
            namespace: Some("apps".into()),
            name: "db".into(),
        }
    }

    #[test]
    fn strip_drops_server_fields_and_keeps_what_the_user_owns() {
        let stripped = strip(&secret("7"));
        let metadata = &stripped["metadata"];
        for field in SERVER_METADATA {
            assert!(metadata.get(field).is_none(), "{field} survived");
        }
        assert!(metadata.get(ANNOTATIONS).is_none());
        assert_eq!(metadata["labels"]["app"], json!("db"));
        assert_eq!(stripped["type"], json!("Opaque"));
        let mut with_status = configmap("1", "info", 1);
        with_status["status"] = json!({ "phase": "Active" });
        with_status["metadata"]["annotations"] = json!({ "team": "core", LAST_APPLIED: "{}" });
        let stripped = strip(&with_status);
        assert!(stripped.get(STATUS).is_none());
        assert_eq!(
            stripped["metadata"]["annotations"],
            json!({ "team": "core" })
        );
    }

    #[test]
    fn the_editable_text_never_holds_a_secret_value_unless_revealed() {
        let Ok(hidden) = live(&secret("7"), false) else {
            panic!("live");
        };
        assert!(!hidden.text.contains(PLANTED), "{}", hidden.text);
        assert!(hidden.text.contains(MASK));
        assert_eq!(hidden.uid, Uid::new("u1"));
        assert_eq!(hidden.resource_version, "7");
        let Ok(shown) = live(&secret("7"), true) else {
            panic!("live");
        };
        assert!(shown.text.contains(PLANTED));
        assert!(!shown.text.contains("managedFields"));
    }

    #[test]
    fn applying_an_untouched_redacted_secret_sends_the_real_values() {
        let object = secret("7");
        let draft = draft_from(&object, |text| text.replace("app: db", "app: database"));
        let Ok(body) = payload(&draft, &object) else {
            panic!("payload");
        };
        let sent = body.to_string();
        assert!(!sent.contains(MASK), "{sent}");
        assert_eq!(body["data"]["password"], json!(PLANTED));
        assert_eq!(body["metadata"]["labels"]["app"], json!("database"));
        assert_eq!(body["metadata"]["uid"], json!("u1"));
        assert_eq!(body["metadata"]["resourceVersion"], json!("7"));
        assert!(body["metadata"].get("managedFields").is_none());
        assert!(body["metadata"].get(ANNOTATIONS).is_none());
    }

    #[test]
    fn a_new_secret_value_replaces_its_mask_and_the_rest_are_restored() {
        let object = secret("7");
        let draft = draft_from(&object, |text| {
            text.replacen(&format!("password: {MASK}"), "password: bmV3", 1)
        });
        let Ok(body) = payload(&draft, &object) else {
            panic!("payload");
        };
        assert_eq!(body["data"]["password"], json!("bmV3"));
        assert_eq!(body["data"]["user"], json!("YWRtaW4="));
    }

    #[test]
    fn placeholders_are_never_written_back() {
        let object = secret("7");
        let typed = draft_from(&object, |text| {
            format!("{text}stringData:\n  token: \"{MASK}\"\n")
        });
        assert!(matches!(
            payload(&typed, &object),
            Err(Error::Redacted(path)) if path == "stringData.token"
        ));
        let config = configmap("1", "info", 1);
        let partial = draft_from(&config, |text| {
            text.replace("git.example.com/repo", "git.example.com/other")
        });
        assert!(matches!(
            payload(&partial, &config),
            Err(Error::Redacted(path)) if path == "data.url"
        ));
        let untouched = draft_from(&config, |text| text.replace("level: info", "level: debug"));
        let Ok(body) = payload(&untouched, &config) else {
            panic!("payload");
        };
        assert_eq!(
            body["data"]["url"],
            json!("https://bot:pw@git.example.com/repo")
        );
    }

    #[test]
    fn rebase_keeps_both_sides_and_names_what_both_changed() {
        let base = json!({ "data": { "level": "info", "replicas": "1", "mode": "a" } });
        let mine = json!({ "data": { "level": "debug", "replicas": "1", "mode": "b" } });
        let theirs =
            json!({ "data": { "level": "info", "replicas": "3", "mode": "c", "new": "x" } });
        let (merged, contested) = rebase(&base, &mine, &theirs);
        assert_eq!(
            merged,
            json!({ "data": { "level": "debug", "replicas": "3", "mode": "b", "new": "x" } })
        );
        assert_eq!(contested, ["data.mode"]);
        let removed = json!({ "data": { "level": "info", "replicas": "1" } });
        let (merged, contested) = rebase(&base, &removed, &theirs);
        assert_eq!(merged["data"].get("mode"), None);
        assert_eq!(contested, ["data.mode"]);
    }

    #[test]
    fn a_conflict_offers_the_edit_rebased_onto_the_new_object() {
        let opened = configmap("1", "info", 1);
        let draft = draft_from(&opened, |text| text.replace("level: info", "level: debug"));
        let now = configmap("2", "info", 3);
        let Ok(EditEvent::Conflict(conflict)) = conflict(&draft, &now) else {
            panic!("conflict");
        };
        assert_eq!(conflict.live.resource_version, "2");
        assert_eq!(conflict.rebased.contested, Vec::<String>::new());
        assert!(conflict.rebased.text.contains("level: debug"));
        assert!(
            conflict.rebased.text.contains("replicas: '3'"),
            "{}",
            conflict.rebased.text
        );
        let changed: Vec<&str> = conflict
            .changed
            .iter()
            .filter(|line| line.change != flock_core::changes::LineChange::Context)
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(changed, ["  replicas: '1'", "  replicas: '3'"]);
    }

    fn fake(
        respond: impl Fn(&Method, &str, &Value) -> Option<(StatusCode, String)> + Send + Sync + 'static,
    ) -> (
        crate::manager::ClusterManager,
        ClusterHandle,
        Arc<crate::fake::Traffic>,
    ) {
        let (client, traffic) =
            crate::fake::client_reading_bodies(std::time::Duration::ZERO, respond);
        let (manager, handle) = crate::fake::handle(&client);
        let mut resources = lock(&handle.shared.resources);
        resources.insert(
            ("v1".to_owned(), "Secret".to_owned()),
            ApiResource::erase::<Secret>(&()),
        );
        resources.insert(
            ("v1".to_owned(), "ConfigMap".to_owned()),
            ApiResource::erase::<ConfigMap>(&()),
        );
        drop(resources);
        (manager, handle, traffic)
    }

    async fn outcome(handle: &ClusterHandle, target: ResourceRef, step: EditStep) -> EditEvent {
        let (sink, events) = Sink::channel();
        run(handle.clone(), EditRequest { target, step, sink }).await;
        let Ok(event) = events.recv().await else {
            panic!("no event");
        };
        event
    }

    #[tokio::test]
    async fn apply_sends_preconditions_and_real_values_on_the_wire() {
        let (_manager, handle, traffic) = fake(|method, _, body| {
            if method == Method::GET {
                return Some((StatusCode::OK, secret("7").to_string()));
            }
            let mut applied = body.clone();
            applied["metadata"]["resourceVersion"] = json!("8");
            Some((StatusCode::OK, applied.to_string()))
        });
        let draft = draft_from(&secret("7"), |text| {
            text.replace("app: db", "app: database")
        });
        let event = outcome(&handle, secret_ref(), EditStep::Apply(draft)).await;
        let EditEvent::Applied(applied) = event else {
            panic!("{event:?}");
        };
        assert_eq!(applied.resource_version, "8");
        assert!(!applied.text.contains(PLANTED));
        let sent = traffic.bodies(&Method::PATCH, SECRET_PATH);
        assert_eq!(sent.len(), 1);
        let body = sent[0].to_string();
        assert!(!body.contains(MASK), "{body}");
        assert!(body.contains(PLANTED), "{body}");
        assert_eq!(sent[0]["metadata"]["uid"], json!("u1"));
        assert_eq!(sent[0]["metadata"]["resourceVersion"], json!("7"));
        let requests = traffic.requests();
        assert!(
            requests.iter().any(|(method, path)| method == Method::PATCH
                && path.contains("fieldManager=flock")
                && path.contains("force=true")),
            "{requests:?}"
        );
    }

    #[tokio::test]
    async fn review_is_a_dry_run_and_diffs_live_against_its_result() {
        let (_manager, handle, traffic) = fake(|method, _, body| {
            if method == Method::GET {
                return Some((StatusCode::OK, configmap("1", "info", 1).to_string()));
            }
            Some((StatusCode::OK, body.to_string()))
        });
        let draft = draft_from(&configmap("1", "info", 1), |text| {
            text.replace("level: info", "level: debug")
        });
        let target = ResourceRef {
            kind: "ConfigMap".into(),
            name: "settings".into(),
            ..secret_ref()
        };
        let event = outcome(&handle, target, EditStep::Review(draft)).await;
        let EditEvent::Reviewed(lines) = event else {
            panic!("{event:?}");
        };
        let changed: Vec<&str> = lines
            .iter()
            .filter(|line| line.change != flock_core::changes::LineChange::Context)
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(changed, ["  level: info", "  level: debug"]);
        let requests = traffic.requests();
        let patch = requests.iter().find(|(method, _)| method == Method::PATCH);
        assert!(
            patch.is_some_and(
                |(_, path)| path.starts_with(CONFIGMAP_PATH) && path.contains("dryRun=All")
            ),
            "{requests:?}"
        );
    }

    #[tokio::test]
    async fn a_stale_edit_is_never_sent_and_reports_the_conflict() {
        let (_manager, handle, traffic) = fake(|method, _, _| {
            let mut theirs = secret("9");
            theirs["metadata"]["labels"]["tier"] = json!("backend");
            (method == Method::GET).then(|| (StatusCode::OK, theirs.to_string()))
        });
        let draft = draft_from(&secret("7"), |text| {
            text.replace("app: db", "app: database")
        });
        let event = outcome(&handle, secret_ref(), EditStep::Apply(draft)).await;
        let EditEvent::Conflict(conflict) = event else {
            panic!("{event:?}");
        };
        assert_eq!(conflict.live.resource_version, "9");
        assert!(conflict.rebased.text.contains("app: database"));
        assert!(!conflict.rebased.text.contains(PLANTED));
        assert_eq!(
            traffic.bodies(&Method::PATCH, SECRET_PATH),
            Vec::<Value>::new()
        );
    }

    #[tokio::test]
    async fn a_new_version_with_the_same_editable_text_is_not_a_conflict() {
        let (_manager, handle, traffic) = fake(|method, _, body| {
            if method == Method::GET {
                return Some((StatusCode::OK, secret("9").to_string()));
            }
            let mut applied = body.clone();
            applied["metadata"]["resourceVersion"] = json!("10");
            Some((StatusCode::OK, applied.to_string()))
        });
        let draft = draft_from(&secret("7"), |text| {
            text.replace("app: db", "app: database")
        });
        let event = outcome(&handle, secret_ref(), EditStep::Apply(draft)).await;
        let EditEvent::Applied(applied) = event else {
            panic!("{event:?}");
        };
        assert_eq!(applied.resource_version, "10");
        let sent = traffic.bodies(&Method::PATCH, SECRET_PATH);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["metadata"]["resourceVersion"], json!("9"));
    }

    #[tokio::test]
    async fn a_conflict_from_the_server_reports_the_object_it_has_now() {
        let version = Arc::new(Mutex::new("7"));
        let seen = version.clone();
        let (_manager, handle, _) = fake(move |method, _, _| {
            if method == Method::GET {
                return Some((StatusCode::OK, secret(*lock(&seen)).to_string()));
            }
            *lock(&seen) = "8";
            Some((
                StatusCode::CONFLICT,
                json!({"kind":"Status","apiVersion":"v1","status":"Failure","reason":"Conflict","code":409,"message":"the object has been modified"}).to_string(),
            ))
        });
        let draft = draft_from(&secret("7"), |text| {
            text.replace("app: db", "app: database")
        });
        let event = outcome(&handle, secret_ref(), EditStep::Apply(draft)).await;
        let EditEvent::Conflict(conflict) = event else {
            panic!("{event:?}");
        };
        assert_eq!(conflict.live.resource_version, "8");
        assert_eq!(*lock(&version), "8");
    }
}
