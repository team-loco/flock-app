use std::net::SocketAddr;
use std::sync::atomic::Ordering;

use flock_core::{TermSize, Uid};
use flock_flux::redact;
use futures::{AsyncBufReadExt, SinkExt, StreamExt};
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Status;
use kube::api::{
    Api, AttachParams, AttachedProcess, DeleteParams, DynamicObject, LogParams, Patch, PatchParams,
    Preconditions, PropagationPolicy, TerminalSize,
};
use kube::core::{ApiResource, GroupVersionKind};
use kube::{Client, Resource};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;

use crate::error::{Error, Result};
use crate::manager::{ClusterHandle, lock};
use crate::resources::CustomKind;
use crate::task::TaskGuard;

pub const FIELD_MANAGER: &str = "flock";
pub const RESTARTED_AT_ANNOTATION: &str = "kubectl.kubernetes.io/restartedAt";
const LOG_CHANNEL_CAPACITY: usize = 1024;
const EXEC_CHANNEL_CAPACITY: usize = 256;
const EXEC_READ_BUFFER: usize = 8192;
const LOOPBACK: &str = "127.0.0.1";
const EXIT_SUCCESS: &str = "Success";
const EXIT_CODE_CAUSE: &str = "ExitCode";
const METADATA: &str = "metadata";
const UID: &str = "uid";
const UID_POINTER: &str = "/metadata/uid";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectRef {
    pub api_version: String,
    pub kind: String,
    pub namespace: Option<String>,
    pub name: String,
}

impl ObjectRef {
    pub fn gvk(&self) -> GroupVersionKind {
        let (group, version) = self
            .api_version
            .split_once('/')
            .unwrap_or(("", self.api_version.as_str()));
        GroupVersionKind::gvk(group, version, &self.kind)
    }

    pub fn from_object(object: &DynamicObject) -> Result<Self> {
        let types = object
            .types
            .as_ref()
            .ok_or(Error::MissingField("apiVersion/kind"))?;
        let name = object
            .metadata
            .name
            .clone()
            .ok_or(Error::MissingField("metadata.name"))?;
        Ok(Self {
            api_version: types.api_version.clone(),
            kind: types.kind.clone(),
            namespace: object.metadata.namespace.clone(),
            name,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatchBody {
    Merge(Value),
    Json(Value),
    Strategic(Value),
}

impl PatchBody {
    pub fn guarded(self, uid: &Uid) -> Self {
        match self {
            Self::Merge(value) => Self::Merge(with_uid(value, uid)),
            Self::Strategic(value) => Self::Strategic(with_uid(value, uid)),
            Self::Json(value) => Self::Json(with_uid_test(&value, uid)),
        }
    }
}

pub(crate) fn with_uid(mut value: Value, uid: &Uid) -> Value {
    if let Some(object) = value.as_object_mut() {
        let metadata = object.entry(METADATA).or_insert_with(|| json!({}));
        if let Some(metadata) = metadata.as_object_mut() {
            metadata.insert(UID.to_owned(), json!(uid.as_str()));
        }
    }
    value
}

pub(crate) fn with_uid_test(value: &Value, uid: &Uid) -> Value {
    let test = json!({ "op": "test", "path": UID_POINTER, "value": uid.as_str() });
    let operations = value.as_array().into_iter().flatten().cloned();
    Value::Array(std::iter::once(test).chain(operations).collect())
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogRequest {
    pub namespace: String,
    pub pod: String,
    pub container: Option<String>,
    pub follow: bool,
    pub tail_lines: Option<i64>,
    pub timestamps: bool,
    pub since_seconds: Option<i64>,
    pub previous: bool,
}

impl LogRequest {
    fn params(&self) -> LogParams {
        LogParams {
            container: self.container.clone(),
            follow: self.follow,
            tail_lines: self.tail_lines,
            timestamps: self.timestamps,
            since_seconds: self.since_seconds,
            previous: self.previous,
            ..LogParams::default()
        }
    }
}

pub struct LogStream {
    pub lines: async_channel::Receiver<Result<String>>,
    _task: TaskGuard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecRequest {
    pub namespace: String,
    pub pod: String,
    pub container: Option<String>,
    pub command: Vec<String>,
    pub tty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecExit {
    Success,
    Code(i32),
    Failed(String),
}

pub struct ExecSession {
    pub stdin: async_channel::Sender<Vec<u8>>,
    pub output: async_channel::Receiver<Vec<u8>>,
    pub resize: async_channel::Sender<TermSize>,
    pub exit: async_channel::Receiver<ExecExit>,
    _task: TaskGuard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardInfo {
    pub id: u64,
    pub namespace: String,
    pub pod: String,
    pub remote_port: u16,
    pub local_addr: SocketAddr,
}

pub(crate) struct ActiveForward {
    info: ForwardInfo,
    task: TaskGuard,
}

fn split_api_version(api_version: &str) -> (&str, &str) {
    api_version.split_once('/').unwrap_or(("", api_version))
}

fn exit_of(status: Option<Status>) -> ExecExit {
    let Some(status) = status else {
        return ExecExit::Success;
    };
    if status.status.as_deref() == Some(EXIT_SUCCESS) {
        return ExecExit::Success;
    }
    let code = status
        .details
        .as_ref()
        .and_then(|details| details.causes.as_ref())
        .into_iter()
        .flatten()
        .find(|cause| cause.reason.as_deref() == Some(EXIT_CODE_CAUSE))
        .and_then(|cause| cause.message.as_deref())
        .and_then(|message| message.parse::<i32>().ok());
    match code {
        Some(code) => ExecExit::Code(code),
        None => ExecExit::Failed(status.message.unwrap_or_default()),
    }
}

pub fn restart_patch(now: jiff::Timestamp) -> Value {
    json!({
        "spec": {
            "template": {
                "metadata": {
                    "annotations": { RESTARTED_AT_ANNOTATION: now.to_string() }
                }
            }
        }
    })
}

pub fn parse_manifest(yaml: &str) -> Result<DynamicObject> {
    let object: DynamicObject = serde_norway::from_str(yaml)?;
    if object.types.is_none() {
        return Err(Error::MissingField("apiVersion/kind"));
    }
    if object.metadata.name.is_none() {
        return Err(Error::MissingField("metadata.name"));
    }
    Ok(object)
}

async fn pump_forward(mut local: TcpStream, pods: Api<Pod>, pod: String, port: u16) -> Result<()> {
    let mut forwarder = pods.portforward(&pod, &[port]).await?;
    let mut upstream = forwarder
        .take_stream(port)
        .ok_or(Error::MissingField("port-forward stream"))?;
    tokio::io::copy_bidirectional(&mut local, &mut upstream).await?;
    drop(upstream);
    forwarder.abort();
    Ok(())
}

impl ClusterHandle {
    pub async fn resolve(&self, api_version: &str, kind: &str) -> Result<ApiResource> {
        let key = (api_version.to_owned(), kind.to_owned());
        if let Some(found) = lock(&self.shared.resources).get(&key) {
            return Ok(found.clone());
        }
        let client = self.client()?;
        let (group, version) = split_api_version(api_version);
        let gvk = GroupVersionKind::gvk(group, version, kind);
        let (resource, _) = kube::discovery::pinned_kind(&client, &gvk)
            .await
            .map_err(|_| Error::UnknownKind {
                api_version: api_version.to_owned(),
                kind: kind.to_owned(),
            })?;
        lock(&self.shared.resources).insert(key, resource.clone());
        Ok(resource)
    }

    pub(crate) async fn resolve_custom(&self, kind: &CustomKind) -> Result<ApiResource> {
        let api_version = kind.gvk().api_version();
        self.resolve(&api_version, &kind.kind).await
    }

    pub(crate) async fn dynamic_api(
        &self,
        target: &ObjectRef,
    ) -> Result<(Client, Api<DynamicObject>)> {
        let client = self.client()?;
        let resource = self.resolve(&target.api_version, &target.kind).await?;
        let api = match &target.namespace {
            Some(namespace) => Api::namespaced_with(client.clone(), namespace, &resource),
            None => Api::all_with(client.clone(), &resource),
        };
        Ok((client, api))
    }

    pub async fn get(&self, target: &ObjectRef) -> Result<DynamicObject> {
        let (_, api) = self.dynamic_api(target).await?;
        Ok(api.get(&target.name).await?)
    }

    pub async fn get_yaml(&self, target: &ObjectRef) -> Result<String> {
        let object = self.get(target).await?;
        let mut value = serde_json::to_value(&object)?;
        redact::object(&mut value);
        Ok(serde_norway::to_string(&value)?)
    }

    pub async fn apply_yaml(&self, yaml: &str, force: bool) -> Result<DynamicObject> {
        let object = parse_manifest(yaml)?;
        let target = ObjectRef::from_object(&object)?;
        let (_, api) = self.dynamic_api(&target).await?;
        let params = PatchParams::apply(FIELD_MANAGER);
        let params = if force { params.force() } else { params };
        Ok(api
            .patch(&target.name, &params, &Patch::Apply(&object))
            .await?)
    }

    pub async fn patch(&self, target: &ObjectRef, body: PatchBody) -> Result<DynamicObject> {
        let (_, api) = self.dynamic_api(target).await?;
        let params = PatchParams {
            field_manager: Some(FIELD_MANAGER.to_owned()),
            ..PatchParams::default()
        };
        let patched = match body {
            PatchBody::Merge(value) => {
                api.patch(&target.name, &params, &Patch::Merge(value))
                    .await?
            }
            PatchBody::Strategic(value) => {
                api.patch(&target.name, &params, &Patch::Strategic(value))
                    .await?
            }
            PatchBody::Json(value) => {
                let operations: json_patch::Patch = serde_json::from_value(value)?;
                api.patch(&target.name, &params, &Patch::Json::<()>(operations))
                    .await?
            }
        };
        Ok(patched)
    }

    pub async fn scale(&self, target: &ObjectRef, replicas: i32, uid: &Uid) -> Result<()> {
        let (_, api) = self.dynamic_api(target).await?;
        let params = PatchParams {
            field_manager: Some(FIELD_MANAGER.to_owned()),
            ..PatchParams::default()
        };
        let body = with_uid(json!({ "spec": { "replicas": replicas } }), uid);
        api.patch_scale(&target.name, &params, &Patch::Merge(&body))
            .await?;
        Ok(())
    }

    pub async fn delete(
        &self,
        target: &ObjectRef,
        propagation: PropagationPolicy,
        uid: Option<&Uid>,
    ) -> Result<()> {
        let (_, api) = self.dynamic_api(target).await?;
        let preconditions = uid.map(|uid| Preconditions {
            uid: Some(uid.as_str().to_owned()),
            resource_version: None,
        });
        let params = DeleteParams {
            propagation_policy: Some(propagation),
            preconditions,
            ..DeleteParams::default()
        };
        api.delete(&target.name, &params).await?;
        Ok(())
    }

    pub async fn logs(&self, request: &LogRequest) -> Result<LogStream> {
        let client = self.client()?;
        let pods: Api<Pod> = Api::namespaced(client, &request.namespace);
        let params = request.params();
        let reader = pods.log_stream(&request.pod, &params).await?;
        let (sender, lines) = async_channel::bounded(LOG_CHANNEL_CAPACITY);
        let _guard = self.shared.runtime.enter();
        let task = TaskGuard::spawn(async move {
            let mut stream = std::pin::pin!(reader.lines());
            while let Some(line) = stream.next().await {
                let item = line.map_err(Error::Io);
                if sender.send(item).await.is_err() {
                    return;
                }
            }
        });
        Ok(LogStream { lines, _task: task })
    }

    pub async fn exec(&self, request: &ExecRequest) -> Result<ExecSession> {
        let client = self.client()?;
        let pods: Api<Pod> = Api::namespaced(client, &request.namespace);
        let mut params = AttachParams::default()
            .stdin(true)
            .stdout(true)
            .stderr(!request.tty)
            .tty(request.tty);
        if let Some(container) = &request.container {
            params = params.container(container.clone());
        }
        let process = pods
            .exec(&request.pod, request.command.clone(), &params)
            .await?;
        self.session(process)
    }

    pub async fn attach(&self, namespace: &str, pod: &str, container: &str) -> Result<ExecSession> {
        let client = self.client()?;
        let pods: Api<Pod> = Api::namespaced(client, namespace);
        let params = AttachParams::interactive_tty().container(container);
        let process = pods.attach(pod, &params).await?;
        self.session(process)
    }

    fn session(&self, mut process: AttachedProcess) -> Result<ExecSession> {
        let mut writer = process.stdin().ok_or(Error::Exec("no stdin".to_owned()))?;
        let mut reader = process
            .stdout()
            .ok_or(Error::Exec("no stdout".to_owned()))?;
        let mut errors = process.stderr();
        let mut resizer = process.terminal_size();
        let status = process.take_status();
        let (stdin, stdin_rx) = async_channel::bounded::<Vec<u8>>(EXEC_CHANNEL_CAPACITY);
        let (output_tx, output) = async_channel::bounded(EXEC_CHANNEL_CAPACITY);
        let (resize, resize_rx) = async_channel::bounded::<TermSize>(EXEC_CHANNEL_CAPACITY);
        let (exit_tx, exit) = async_channel::bounded(1);
        let _guard = self.shared.runtime.enter();
        let task = TaskGuard::spawn(async move {
            let input = async {
                while let Ok(bytes) = stdin_rx.recv().await {
                    if writer.write_all(&bytes).await.is_err() || writer.flush().await.is_err() {
                        return;
                    }
                }
                let _ = writer.shutdown().await;
            };
            let terminal_sizes = async {
                while let Ok(size) = resize_rx.recv().await {
                    let Some(sender) = resizer.as_mut() else {
                        continue;
                    };
                    let terminal = TerminalSize {
                        width: size.cols,
                        height: size.rows,
                    };
                    if sender.send(terminal).await.is_err() {
                        return;
                    }
                }
            };
            let stdout_tx = output_tx.clone();
            let out = async move {
                let mut buffer = vec![0_u8; EXEC_READ_BUFFER];
                while let Ok(read) = reader.read(&mut buffer).await {
                    let Some(chunk) = buffer.get(..read).filter(|chunk| !chunk.is_empty()) else {
                        return;
                    };
                    if stdout_tx.send(chunk.to_vec()).await.is_err() {
                        return;
                    }
                }
            };
            let err = async move {
                let Some(stderr) = errors.as_mut() else {
                    return;
                };
                let mut buffer = vec![0_u8; EXEC_READ_BUFFER];
                while let Ok(read) = stderr.read(&mut buffer).await {
                    let Some(chunk) = buffer.get(..read).filter(|chunk| !chunk.is_empty()) else {
                        return;
                    };
                    if output_tx.send(chunk.to_vec()).await.is_err() {
                        return;
                    }
                }
            };
            let finished = async move {
                let status = async move {
                    match status {
                        Some(status) => exit_of(status.await),
                        None => ExecExit::Success,
                    }
                };
                let (outcome, (), ()) = futures::join!(status, out, err);
                let _ = exit_tx.send(outcome).await;
            };
            tokio::select! {
                () = finished => {}
                () = async { futures::join!(input, terminal_sizes); std::future::pending::<()>().await } => {}
            }
            drop(process);
        });
        Ok(ExecSession {
            stdin,
            output,
            resize,
            exit,
            _task: task,
        })
    }

    pub async fn port_forward(
        &self,
        namespace: &str,
        pod: &str,
        remote_port: u16,
        local_port: u16,
    ) -> Result<ForwardInfo> {
        let client = self.client()?;
        let listener = TcpListener::bind((LOOPBACK, local_port)).await?;
        let local_addr = listener.local_addr()?;
        let id = self.shared.next_forward.fetch_add(1, Ordering::Relaxed);
        let info = ForwardInfo {
            id,
            namespace: namespace.to_owned(),
            pod: pod.to_owned(),
            remote_port,
            local_addr,
        };
        let pods: Api<Pod> = Api::namespaced(client, namespace);
        let pod_name = pod.to_owned();
        let _guard = self.shared.runtime.enter();
        let task = TaskGuard::spawn(async move {
            let mut connections = JoinSet::new();
            while let Ok((socket, _)) = listener.accept().await {
                while connections.try_join_next().is_some() {}
                let pods = pods.clone();
                let pod_name = pod_name.clone();
                connections.spawn(async move {
                    if let Err(err) = pump_forward(socket, pods, pod_name, remote_port).await {
                        tracing::warn!(%err, "port-forward connection failed");
                    }
                });
            }
        });
        let active = ActiveForward {
            info: info.clone(),
            task,
        };
        lock(&self.shared.forwards).insert(id, active);
        Ok(info)
    }

    pub fn port_forwards(&self) -> Vec<ForwardInfo> {
        lock(&self.shared.forwards)
            .values()
            .filter(|active| !active.task.is_finished())
            .map(|active| active.info.clone())
            .collect()
    }

    pub fn stop_port_forward(&self, id: u64) -> bool {
        lock(&self.shared.forwards).remove(&id).is_some()
    }
}

pub fn object_ref<K: Resource<DynamicType = ()>>(namespace: Option<&str>, name: &str) -> ObjectRef {
    ObjectRef {
        api_version: K::api_version(&()).into_owned(),
        kind: K::kind(&()).into_owned(),
        namespace: namespace.map(str::to_owned),
        name: name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use k8s_openapi::api::apps::v1::Deployment;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::{StatusCause, StatusDetails};

    use super::*;

    #[test]
    fn restart_patch_sets_annotation() {
        let Ok(now) = "2026-10-12T10:00:00Z".parse::<jiff::Timestamp>() else {
            panic!("timestamp");
        };
        let patch = restart_patch(now);
        assert_eq!(
            patch["spec"]["template"]["metadata"]["annotations"][RESTARTED_AT_ANNOTATION],
            json!("2026-10-12T10:00:00Z")
        );
    }

    #[test]
    fn guarded_patches_pin_the_uid() {
        let uid = Uid::new("u1");
        let merge = PatchBody::Merge(json!({"spec": {"suspend": true}})).guarded(&uid);
        assert_eq!(
            merge,
            PatchBody::Merge(json!({"metadata": {"uid": "u1"}, "spec": {"suspend": true}}))
        );
        let json_patch = PatchBody::Json(json!([{"op": "replace", "path": "/spec/x", "value": 1}]))
            .guarded(&uid);
        assert_eq!(
            json_patch,
            PatchBody::Json(json!([
                {"op": "test", "path": "/metadata/uid", "value": "u1"},
                {"op": "replace", "path": "/spec/x", "value": 1}
            ]))
        );
    }

    #[test]
    fn parses_manifest_and_ref() {
        let yaml = "apiVersion: kustomize.toolkit.fluxcd.io/v1\nkind: Kustomization\nmetadata:\n  name: apps\n  namespace: flux-system\nspec:\n  suspend: true\n";
        let Ok(object) = parse_manifest(yaml) else {
            panic!("manifest parses");
        };
        let Ok(target) = ObjectRef::from_object(&object) else {
            panic!("ref");
        };
        assert_eq!(target.namespace.as_deref(), Some("flux-system"));
        let gvk = target.gvk();
        assert_eq!(
            (gvk.group.as_str(), gvk.version.as_str()),
            ("kustomize.toolkit.fluxcd.io", "v1")
        );
        assert!(matches!(
            parse_manifest("kind: Pod\nmetadata: {}\n"),
            Err(Error::Yaml(_) | Error::MissingField(_))
        ));
    }

    #[test]
    fn core_refs_have_empty_group() {
        let target = object_ref::<Pod>(Some("default"), "web");
        assert_eq!(target.api_version, "v1");
        assert_eq!(target.gvk().group, "");
        let deployment = object_ref::<Deployment>(Some("apps"), "api");
        assert_eq!(deployment.api_version, "apps/v1");
    }

    #[test]
    fn previous_log_requests_ask_for_the_terminated_instance() {
        let request = LogRequest {
            namespace: "web".into(),
            pod: "podinfo-1".into(),
            container: Some("podinfo".into()),
            tail_lines: Some(500),
            timestamps: true,
            previous: true,
            ..LogRequest::default()
        };
        let params = request.params();
        assert!(params.previous);
        assert!(!params.follow);
        assert_eq!(params.container.as_deref(), Some("podinfo"));
        assert_eq!(params.tail_lines, Some(500));
        assert!(!LogRequest::default().params().previous);
    }

    #[test]
    fn exit_codes() {
        assert_eq!(exit_of(None), ExecExit::Success);
        let success = Status {
            status: Some("Success".to_owned()),
            ..Status::default()
        };
        assert_eq!(exit_of(Some(success)), ExecExit::Success);
        let failed = Status {
            status: Some("Failure".to_owned()),
            reason: Some("NonZeroExitCode".to_owned()),
            message: Some("command terminated with non-zero exit code".to_owned()),
            details: Some(StatusDetails {
                causes: Some(vec![StatusCause {
                    reason: Some("ExitCode".to_owned()),
                    message: Some("3".to_owned()),
                    field: None,
                }]),
                ..StatusDetails::default()
            }),
            ..Status::default()
        };
        assert_eq!(exit_of(Some(failed)), ExecExit::Code(3));
    }
}
