use std::convert::Infallible;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use http::{Method, Request, Response, StatusCode};
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::ConfigMap;
use kube::Client;
use kube::client::Body;
use kube::core::ApiResource;
use serde_json::Value;
use tower_service::Service;

use crate::discovery::{ContextRef, KubeContext};
use crate::manager::{ClusterEvent, ClusterHandle, ClusterManager, lock};
use crate::provider::{AuthKind, Provider};
use crate::settings::KubeSettings;
use crate::shell_env::ShellEnv;

const NAMESPACE: &str = "default";
const MISSING_KUBECONFIG: &str = "/missing-fake-kubeconfig";
const NOT_FOUND: &str =
    r#"{"kind":"Status","apiVersion":"v1","status":"Failure","reason":"NotFound","code":404}"#;

pub type Reply = (StatusCode, String);
type Respond = dyn Fn(&Method, &str, &Value) -> Option<Reply> + Send + Sync;

#[derive(Default)]
pub struct Traffic {
    active: AtomicUsize,
    peak: AtomicUsize,
    requests: Mutex<Vec<(Method, String)>>,
    bodies: Mutex<Vec<(Method, String, Value)>>,
}

impl Traffic {
    pub fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }

    pub fn requests(&self) -> Vec<(Method, String)> {
        lock(&self.requests).clone()
    }

    pub fn bodies(&self, method: &Method, path: &str) -> Vec<Value> {
        lock(&self.bodies)
            .iter()
            .filter(|(sent, at, _)| sent == method && at.starts_with(path))
            .map(|(_, _, body)| body.clone())
            .collect()
    }
}

#[derive(Clone)]
struct Fake {
    delay: Duration,
    respond: Arc<Respond>,
    traffic: Arc<Traffic>,
}

impl Service<Request<Body>> for Fake {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response<Body>, Infallible>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        let method = request.method().clone();
        let path = request.uri().to_string();
        lock(&self.traffic.requests).push((method.clone(), path.clone()));
        let delay = self.delay;
        let traffic = self.traffic.clone();
        let respond = self.respond.clone();
        Box::pin(async move {
            let bytes = request
                .into_body()
                .collect_bytes()
                .await
                .unwrap_or_default();
            let sent = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            let (status, body) = respond(&method, &path, &sent)
                .unwrap_or_else(|| (StatusCode::NOT_FOUND, NOT_FOUND.to_owned()));
            lock(&traffic.bodies).push((method, path, sent));
            let now = traffic.active.fetch_add(1, Ordering::SeqCst) + 1;
            traffic.peak.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(delay).await;
            traffic.active.fetch_sub(1, Ordering::SeqCst);
            let mut response = Response::new(Body::from(body.into_bytes()));
            *response.status_mut() = status;
            response.headers_mut().insert(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("application/json"),
            );
            Ok(response)
        })
    }
}

pub fn client(
    delay: Duration,
    respond: impl Fn(&Method, &str) -> Option<Reply> + Send + Sync + 'static,
) -> (Client, Arc<Traffic>) {
    client_reading_bodies(delay, move |method, path, _| respond(method, path))
}

pub fn client_reading_bodies(
    delay: Duration,
    respond: impl Fn(&Method, &str, &Value) -> Option<Reply> + Send + Sync + 'static,
) -> (Client, Arc<Traffic>) {
    let traffic = Arc::new(Traffic::default());
    let fake = Fake {
        delay,
        respond: Arc::new(respond),
        traffic: traffic.clone(),
    };
    (Client::new(fake, NAMESPACE), traffic)
}

pub fn handle(client: &Client) -> (ClusterManager, ClusterHandle) {
    let (manager, handle, _) = supervised(client);
    (manager, handle)
}

pub fn supervised(
    client: &Client,
) -> (
    ClusterManager,
    ClusterHandle,
    async_channel::Receiver<ClusterEvent>,
) {
    let settings = KubeSettings::default();
    let env = Arc::new(ShellEnv::from_process(String::new()));
    let (manager, events) = ClusterManager::new(tokio::runtime::Handle::current(), settings, env);
    let context = KubeContext {
        trust: crate::discovery::Fingerprint::default(),
        credentials: crate::discovery::Fingerprint::default(),
        id: ContextRef {
            file: MISSING_KUBECONFIG.into(),
            context: NAMESPACE.to_owned(),
        },
        cluster: NAMESPACE.to_owned(),
        user: None,
        namespace: None,
        server: None,
        provider: Provider::Generic,
        auth: AuthKind::None,
        is_current: false,
    };
    let handle = manager.connect(context);
    handle.shared.set_client(Some(client));
    (manager, handle, events)
}

pub const SEEN: &str = "seen-uid";
pub const RECREATED: &str = "recreated-uid";
const CONFLICT: &str = r#"{"kind":"Status","apiVersion":"v1","status":"Failure","reason":"Conflict","code":409,"message":"Precondition failed: UID mismatch"}"#;
const ETCD_TIMEOUT: &str = r#"{"kind":"Status","apiVersion":"v1","status":"Failure","message":"etcdserver: request timed out","code":500}"#;
const UID_POINTERS: [&str; 3] = ["/preconditions/uid", "/metadata/uid", "/0/value"];

fn object(kind: &str, uid: &str) -> String {
    serde_json::json!({
        "apiVersion": crate::actions::mutate::api_version(kind),
        "kind": kind,
        "metadata": { "name": "podinfo", "namespace": "apps", "uid": uid }
    })
    .to_string()
}

pub fn sent_uid(body: &Value, pointer: &str) -> Option<String> {
    body.pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

pub fn recreated(
    kind: &'static str,
) -> (
    ClusterManager,
    ClusterHandle,
    Arc<Traffic>,
    Arc<Mutex<bool>>,
) {
    let changed = Arc::new(Mutex::new(false));
    let state = changed.clone();
    let (client, traffic) = client_reading_bodies(Duration::ZERO, move |method, _, body| {
        let current = object(kind, RECREATED);
        if method == Method::GET {
            return Some((StatusCode::OK, current));
        }
        let wanted = UID_POINTERS
            .iter()
            .find_map(|pointer| sent_uid(body, pointer));
        if wanted.as_deref() != Some(RECREATED) {
            return Some((StatusCode::CONFLICT, CONFLICT.to_owned()));
        }
        *lock(&state) = true;
        Some((StatusCode::OK, current))
    });
    let (manager, handle) = registered(&client);
    (manager, handle, traffic, changed)
}

pub fn overloaded(
    kind: &'static str,
    timeouts: usize,
) -> (ClusterManager, ClusterHandle, Arc<Traffic>) {
    let writes = AtomicUsize::new(0);
    let (client, traffic) = client(Duration::ZERO, move |method, _| {
        let current = object(kind, SEEN);
        if method != Method::PATCH {
            return Some((StatusCode::OK, current));
        }
        if writes.fetch_add(1, Ordering::SeqCst) < timeouts {
            return Some((StatusCode::INTERNAL_SERVER_ERROR, ETCD_TIMEOUT.to_owned()));
        }
        Some((StatusCode::OK, current))
    });
    let (manager, handle) = registered(&client);
    (manager, handle, traffic)
}

fn registered(client: &Client) -> (ClusterManager, ClusterHandle) {
    let (manager, handle) = handle(client);
    let mut resources = lock(&handle.shared.resources);
    for (api_version, name, resource) in [
        ("v1", "ConfigMap", ApiResource::erase::<ConfigMap>(&())),
        (
            "apps/v1",
            "Deployment",
            ApiResource::erase::<Deployment>(&()),
        ),
        ("batch/v1", "Job", ApiResource::erase::<Job>(&())),
    ] {
        resources.insert((api_version.to_owned(), name.to_owned()), resource);
    }
    drop(resources);
    (manager, handle)
}
