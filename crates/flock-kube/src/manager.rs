use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use flock_core::WatchState;
use flock_core::watch::NOT_STARTED;
use k8s_openapi::apimachinery::pkg::version::Info;
use kube::Client;
use kube::api::DynamicObject;
use kube::core::ApiResource;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use crate::backoff::Backoff;
use crate::client::connect;
use crate::discovery::{ContextRef, KubeContext};
use crate::error::{Error, Result};
use crate::failure::{Failure, FailureKind, classify};
use crate::probe;
use crate::provider::{Provider, refine_provider};
use crate::resources::{
    CustomKind, RunningWatch, Signal, SignalEvent, WatchKind, Watched, custom_snapshot, snapshot,
    spawn_builtin, spawn_custom,
};
use crate::settings::KubeSettings;
use crate::shell_env::ShellEnv;
use crate::task::TaskGuard;
use crate::throttle::Throttle;

pub const COALESCE_INTERVAL: Duration = Duration::from_millis(250);
pub const STEP_CREDENTIALS: &str = "Reading kubeconfig and credentials";
pub const STEP_API: &str = "Checking the API server and nodes";
pub const STEP_IDENTITY: &str = "Checking the signed-in identity";
const OFFLINE: &str = "cluster offline";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindFailure {
    pub kind: WatchKind,
    pub failure: Failure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connecting,
    Connected,
    Degraded(Vec<KindFailure>),
    AuthExpired(Failure),
    Offline(Failure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterInfo {
    pub provider: Provider,
    pub version: Option<String>,
    pub user: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClusterEvent {
    Status {
        id: ContextRef,
        status: ConnectionStatus,
    },
    Changed {
        id: ContextRef,
        kinds: BTreeSet<WatchKind>,
    },
}

struct WatchEntry {
    refs: usize,
    resource: Option<ApiResource>,
    running: Option<RunningWatch>,
    state: WatchState,
    loaded: bool,
}

pub(crate) struct Shared {
    pub(crate) context: KubeContext,
    pub(crate) runtime: Handle,
    pub(crate) env: Arc<ShellEnv>,
    pub(crate) settings: KubeSettings,
    status: Mutex<ConnectionStatus>,
    step: Mutex<&'static str>,
    info: Mutex<ClusterInfo>,
    client: Mutex<Option<Client>>,
    watches: Mutex<HashMap<WatchKind, WatchEntry>>,
    signals: mpsc::UnboundedSender<Signal>,
    pub(crate) resources: Mutex<HashMap<(String, String), ApiResource>>,
    pub(crate) forwards: Mutex<BTreeMap<u64, crate::ops::ActiveForward>>,
    pub(crate) next_forward: std::sync::atomic::AtomicU64,
    closed: watch::Sender<bool>,
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Shared {
    #[cfg(feature = "loco")]
    pub(crate) fn current_client(&self) -> Option<Client> {
        lock(&self.client).clone()
    }

    fn start(&self, kind: &WatchKind, entry: &mut WatchEntry, client: &Client) {
        entry.state = WatchState::pending(format!("Listing {}", kind.name()));
        entry.loaded = false;
        let signals = self.signals.clone();
        entry.running = match (kind, &entry.resource) {
            (WatchKind::Custom(custom), Some(resource)) => Some(spawn_custom(
                &self.runtime,
                client.clone(),
                custom,
                resource,
                signals,
            )),
            (WatchKind::Custom(_), None) => None,
            (builtin, _) => spawn_builtin(
                builtin,
                self.settings.event_limit,
                &self.runtime,
                client.clone(),
                signals,
            ),
        };
    }

    pub(crate) fn set_client(&self, client: Option<&Client>) {
        if self.is_closed() && client.is_some() {
            return;
        }
        *lock(&self.client) = client.cloned();
        let mut watches = lock(&self.watches);
        for (kind, entry) in watches.iter_mut() {
            entry.running = None;
            if let Some(client) = client {
                self.start(kind, entry, client);
            }
        }
    }

    fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    fn closing(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut closed = self.closed.subscribe();
        async move {
            closed.wait_for(|closed| *closed).await.ok();
        }
    }

    fn close(&self) {
        self.closed.send_replace(true);
        self.set_client(None);
        lock(&self.forwards).clear();
        lock(&self.resources).clear();
    }

    fn subscribe(&self, kind: &WatchKind, resource: Option<ApiResource>) {
        let client = lock(&self.client).clone();
        let mut watches = lock(&self.watches);
        let entry = watches.entry(kind.clone()).or_insert_with(|| WatchEntry {
            refs: 0,
            resource,
            running: None,
            state: WatchState::pending(NOT_STARTED),
            loaded: false,
        });
        entry.refs += 1;
        if entry.running.is_none()
            && let Some(client) = client
        {
            self.start(kind, entry, &client);
        }
    }

    fn synced(&self, kind: &WatchKind) {
        if let Some(entry) = lock(&self.watches).get_mut(kind) {
            entry.state = WatchState::Synced;
            entry.loaded = true;
        }
    }

    fn failed(&self, kind: &WatchKind, reason: &str) {
        if let Some(entry) = lock(&self.watches).get_mut(kind) {
            entry.state = if entry.loaded {
                WatchState::Refreshing(reason.to_owned())
            } else {
                WatchState::Failed(reason.to_owned())
            };
        }
    }

    fn set_step(&self, step: &'static str) {
        *lock(&self.step) = step;
    }

    fn unsubscribe(&self, kind: &WatchKind) {
        let mut watches = lock(&self.watches);
        let remove = watches.get_mut(kind).is_some_and(|entry| {
            entry.refs = entry.refs.saturating_sub(1);
            entry.refs == 0
        });
        if remove {
            watches.remove(kind);
            let dropped = Signal {
                kind: kind.clone(),
                event: SignalEvent::Dropped,
            };
            self.signals.send(dropped).ok();
        }
    }

    fn watching(&self, kind: &WatchKind) -> bool {
        lock(&self.watches).contains_key(kind)
    }
}

pub struct Subscription {
    shared: Weak<Shared>,
    kind: WatchKind,
}

impl Subscription {
    pub fn kind(&self) -> &WatchKind {
        &self.kind
    }

    pub fn belongs_to(&self, handle: &ClusterHandle) -> bool {
        std::ptr::eq(self.shared.as_ptr(), Arc::as_ptr(&handle.shared))
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.upgrade() {
            shared.unsubscribe(&self.kind);
        }
    }
}

#[derive(Clone)]
pub struct ClusterHandle {
    pub(crate) shared: Arc<Shared>,
    _supervisor: Arc<TaskGuard>,
}

impl ClusterHandle {
    pub fn close(&self) {
        self.shared.close();
    }

    pub fn is_closed(&self) -> bool {
        self.shared.is_closed()
    }

    pub async fn until_closed<F: Future>(&self, work: F) -> Option<F::Output> {
        tokio::select! {
            output = work => Some(output),
            () = self.shared.closing() => None,
        }
    }

    pub fn id(&self) -> &ContextRef {
        &self.shared.context.id
    }

    pub fn context(&self) -> &KubeContext {
        &self.shared.context
    }

    pub fn status(&self) -> ConnectionStatus {
        lock(&self.shared.status).clone()
    }

    pub fn info(&self) -> ClusterInfo {
        lock(&self.shared.info).clone()
    }

    pub fn client(&self) -> Result<Client> {
        lock(&self.shared.client).clone().ok_or(Error::NotConnected)
    }

    pub fn subscribe(&self, kind: WatchKind) -> Subscription {
        self.shared.subscribe(&kind, None);
        Subscription {
            shared: Arc::downgrade(&self.shared),
            kind,
        }
    }

    pub async fn subscribe_custom(&self, kind: CustomKind) -> Result<Subscription> {
        let resource = self.resolve_custom(&kind).await?;
        let kind = WatchKind::Custom(kind);
        self.shared.subscribe(&kind, Some(resource));
        Ok(Subscription {
            shared: Arc::downgrade(&self.shared),
            kind,
        })
    }

    pub fn subscribe_resource(&self, kind: CustomKind, resource: ApiResource) -> Subscription {
        let kind = WatchKind::Custom(kind);
        self.shared.subscribe(&kind, Some(resource));
        Subscription {
            shared: Arc::downgrade(&self.shared),
            kind,
        }
    }

    pub fn objects<K: Watched>(&self) -> Vec<Arc<K>>
    where
        K::DynamicType: Eq + std::hash::Hash + Clone,
    {
        let watches = lock(&self.shared.watches);
        watches
            .get(&K::KIND)
            .and_then(|entry| entry.running.as_ref())
            .map(snapshot::<K>)
            .unwrap_or_default()
    }

    pub fn custom_objects(&self, kind: &CustomKind) -> Vec<Arc<DynamicObject>> {
        let watches = lock(&self.shared.watches);
        watches
            .get(&WatchKind::Custom(kind.clone()))
            .and_then(|entry| entry.running.as_ref())
            .map(custom_snapshot)
            .unwrap_or_default()
    }

    pub fn watch_state(&self, kind: &WatchKind) -> WatchState {
        match self.status() {
            ConnectionStatus::Connecting => WatchState::pending(*lock(&self.shared.step)),
            ConnectionStatus::Offline(failure) => {
                WatchState::Failed(format!("{OFFLINE}: {}", failure.detail))
            }
            ConnectionStatus::AuthExpired(failure) => WatchState::Failed(failure.detail),
            ConnectionStatus::Connected => self.entry_state(kind),
            ConnectionStatus::Degraded(_) => self.entry_state(kind),
        }
    }

    fn entry_state(&self, kind: &WatchKind) -> WatchState {
        lock(&self.shared.watches).get(kind).map_or_else(
            || WatchState::pending(NOT_STARTED),
            |entry| entry.state.clone(),
        )
    }

    pub fn watched_kinds(&self) -> Vec<WatchKind> {
        let kinds: BTreeSet<WatchKind> = lock(&self.shared.watches).keys().cloned().collect();
        kinds.into_iter().collect()
    }
}

#[derive(Default)]
struct Health {
    probe: Option<Failure>,
    kinds: BTreeMap<WatchKind, Failure>,
    connected_once: bool,
}

impl Health {
    fn record(&mut self, kind: WatchKind, failure: Failure) {
        let page_scoped =
            matches!(kind, WatchKind::Custom(_)) && failure.kind == FailureKind::Forbidden;
        if !page_scoped {
            self.kinds.insert(kind, failure);
        }
    }

    fn status(&self) -> ConnectionStatus {
        if let Some(failure) = &self.probe {
            if failure.kind.is_auth() {
                return ConnectionStatus::AuthExpired(failure.clone());
            }
            return ConnectionStatus::Offline(failure.clone());
        }
        if !self.connected_once {
            return ConnectionStatus::Connecting;
        }
        if self.kinds.is_empty() {
            return ConnectionStatus::Connected;
        }
        let failures = self
            .kinds
            .iter()
            .map(|(kind, failure)| KindFailure {
                kind: kind.clone(),
                failure: failure.clone(),
            })
            .collect();
        ConnectionStatus::Degraded(failures)
    }
}

async fn establish(shared: &Shared) -> Result<Client> {
    shared.set_step(STEP_CREDENTIALS);
    let client = connect(
        &shared.context,
        &shared.env,
        shared.settings.connect_timeout,
    )
    .await?;
    shared.set_step(STEP_API);
    let (version, provider_id) = identify(&client, shared.settings.check_timeout).await?;
    let provider = refine_provider(
        shared.context.provider,
        Some(&version.git_version),
        provider_id.as_deref(),
    );
    let mut info = lock(&shared.info);
    info.provider = provider;
    info.version = Some(version.git_version);
    Ok(client)
}

async fn identify(client: &Client, timeout: Duration) -> Result<(Info, Option<String>)> {
    let (version, nodes) = tokio::join!(
        tokio::time::timeout(timeout, client.apiserver_version()),
        tokio::time::timeout(timeout, probe::sample_nodes(client)),
    );
    let version = version.map_err(|_| Error::Timeout(timeout))??;
    let provider_id = nodes
        .ok()
        .and_then(Result::ok)
        .and_then(|sample| sample.provider_id);
    Ok((version, provider_id))
}

async fn health_check(shared: &Shared) -> Result<(), Failure> {
    let existing = lock(&shared.client).clone();
    let client = if let Some(client) = existing {
        client
    } else {
        let client = establish(shared)
            .await
            .map_err(|err| classify(&err, &shared.context))?;
        shared.set_client(Some(&client));
        client
    };
    shared.set_step(STEP_IDENTITY);
    let timeout = shared.settings.check_timeout;
    let who = tokio::time::timeout(timeout, probe::whoami(&client))
        .await
        .unwrap_or(Err(Error::Timeout(timeout)));
    match who {
        Ok(user) => {
            lock(&shared.info).user = user;
            Ok(())
        }
        Err(err) => {
            let failure = classify(&err, &shared.context);
            match failure.kind {
                FailureKind::Forbidden => Ok(()),
                FailureKind::NotFound => Ok(()),
                _ => Err(failure),
            }
        }
    }
}

async fn supervise(
    shared: Arc<Shared>,
    mut signals: mpsc::UnboundedReceiver<Signal>,
    events: async_channel::Sender<ClusterEvent>,
) {
    let id = shared.context.id.clone();
    let mut backoff = Backoff::new(shared.settings.backoff_initial, shared.settings.backoff_max);
    let mut health = Health::default();
    let mut published = None;
    let mut dirty = BTreeSet::new();
    let mut flush = Throttle::new(COALESCE_INTERVAL);
    let mut next_probe = Instant::now();
    let closing = shared.closing();
    tokio::pin!(closing);
    loop {
        tokio::select! {
            () = &mut closing => return,
            () = tokio::time::sleep_until(next_probe) => {
                match health_check(&shared).await {
                    Ok(()) => {
                        health.probe = None;
                        health.connected_once = true;
                        backoff.reset();
                        next_probe = Instant::now() + shared.settings.health_interval;
                    }
                    Err(failure) => {
                        if failure.kind.is_auth() {
                            shared.set_client(None);
                        }
                        health.probe = Some(failure);
                        next_probe = Instant::now() + backoff.next_delay();
                    }
                }
            }
            received = signals.recv() => {
                let Some(signal) = received else { return };
                match signal.event {
                    SignalEvent::Changed => {
                        shared.synced(&signal.kind);
                        health.kinds.remove(&signal.kind);
                        dirty.insert(signal.kind);
                    }
                    SignalEvent::Failed(err) => {
                        if shared.watching(&signal.kind) {
                            let failure = classify(&err, &shared.context);
                            shared.failed(&signal.kind, &failure.detail);
                            dirty.insert(signal.kind.clone());
                            health.record(signal.kind, failure);
                        }
                    }
                    SignalEvent::Dropped => {
                        if !shared.watching(&signal.kind) {
                            health.kinds.remove(&signal.kind);
                        }
                    }
                }
            }
            () = flush.ready(), if !dirty.is_empty() => {
                flush.fired();
                let kinds = std::mem::take(&mut dirty);
                let event = ClusterEvent::Changed { id: id.clone(), kinds };
                if events.send(event).await.is_err() {
                    return;
                }
            }
        }
        let status = health.status();
        if published.as_ref() != Some(&status) {
            *lock(&shared.status) = status.clone();
            published = Some(status.clone());
            let event = ClusterEvent::Status {
                id: id.clone(),
                status,
            };
            if events.send(event).await.is_err() {
                return;
            }
        }
    }
}

pub struct ClusterManager {
    runtime: Handle,
    settings: KubeSettings,
    env: Arc<ShellEnv>,
    events: async_channel::Sender<ClusterEvent>,
    clusters: Mutex<HashMap<ContextRef, ClusterHandle>>,
}

impl ClusterManager {
    pub fn new(
        runtime: Handle,
        settings: KubeSettings,
        env: Arc<ShellEnv>,
    ) -> (Self, async_channel::Receiver<ClusterEvent>) {
        let (events, receiver) = async_channel::unbounded();
        let manager = Self {
            runtime,
            settings,
            env,
            events,
            clusters: Mutex::new(HashMap::new()),
        };
        (manager, receiver)
    }

    pub fn connect(&self, context: KubeContext) -> ClusterHandle {
        let mut clusters = lock(&self.clusters);
        if let Some(existing) = clusters.get(&context.id) {
            if existing.shared.context == context {
                return existing.clone();
            }
            existing.close();
        }
        let (signals_tx, signals_rx) = mpsc::unbounded_channel();
        let info = ClusterInfo {
            provider: context.provider,
            version: None,
            user: None,
        };
        let shared = Arc::new(Shared {
            context: context.clone(),
            runtime: self.runtime.clone(),
            env: self.env.clone(),
            settings: self.settings.clone(),
            status: Mutex::new(ConnectionStatus::Connecting),
            step: Mutex::new(STEP_CREDENTIALS),
            info: Mutex::new(info),
            client: Mutex::new(None),
            watches: Mutex::new(HashMap::new()),
            signals: signals_tx,
            resources: Mutex::new(HashMap::new()),
            forwards: Mutex::new(BTreeMap::new()),
            next_forward: std::sync::atomic::AtomicU64::new(1),
            closed: watch::Sender::new(false),
        });
        let _guard = self.runtime.enter();
        let supervisor =
            TaskGuard::spawn(supervise(shared.clone(), signals_rx, self.events.clone()));
        let handle = ClusterHandle {
            shared,
            _supervisor: Arc::new(supervisor),
        };
        clusters.insert(context.id, handle.clone());
        handle
    }

    pub fn disconnect(&self, id: &ContextRef) -> bool {
        let removed = lock(&self.clusters).remove(id);
        removed.inspect(ClusterHandle::close).is_some()
    }

    pub fn get(&self, id: &ContextRef) -> Option<ClusterHandle> {
        lock(&self.clusters).get(id).cloned()
    }

    pub fn connected(&self) -> Vec<ContextRef> {
        lock(&self.clusters).keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(kind: FailureKind) -> Failure {
        Failure {
            kind,
            detail: String::new(),
            message: String::new(),
            remedy: None,
        }
    }

    #[tokio::test]
    async fn a_failure_after_the_first_sync_keeps_the_kind_loaded_until_the_store_restarts() {
        let (client, _) = crate::fake::client(Duration::ZERO, |_, _| None);
        let (_manager, handle) = crate::fake::handle(&client);
        let _jobs = handle.subscribe(WatchKind::Jobs);
        let _secrets = handle.subscribe(WatchKind::Secrets);
        *lock(&handle.shared.status) = ConnectionStatus::Connected;
        handle.shared.failed(&WatchKind::Secrets, "forbidden");
        assert_eq!(
            handle.watch_state(&WatchKind::Secrets),
            WatchState::Failed("forbidden".into())
        );
        handle.shared.synced(&WatchKind::Jobs);
        handle.shared.failed(&WatchKind::Jobs, "watch dropped");
        assert_eq!(
            handle.watch_state(&WatchKind::Jobs),
            WatchState::Refreshing("watch dropped".into())
        );
        handle.shared.synced(&WatchKind::Jobs);
        assert_eq!(handle.watch_state(&WatchKind::Jobs), WatchState::Synced);
        handle.shared.set_client(Some(&client));
        handle.shared.failed(&WatchKind::Jobs, "relist failed");
        assert_eq!(
            handle.watch_state(&WatchKind::Jobs),
            WatchState::Failed("relist failed".into())
        );
    }

    #[test]
    fn status_transitions() {
        let mut health = Health::default();
        assert_eq!(health.status(), ConnectionStatus::Connecting);
        health.probe = Some(failure(FailureKind::Unreachable));
        assert!(matches!(health.status(), ConnectionStatus::Offline(_)));
        health.probe = Some(failure(FailureKind::ExpiredCredentials));
        assert!(matches!(health.status(), ConnectionStatus::AuthExpired(_)));
        health.probe = None;
        health.connected_once = true;
        assert_eq!(health.status(), ConnectionStatus::Connected);
        health
            .kinds
            .insert(WatchKind::Secrets, failure(FailureKind::Forbidden));
        assert!(matches!(health.status(), ConnectionStatus::Degraded(ref f) if f.len() == 1));
    }

    #[test]
    fn forbidden_custom_watch_does_not_degrade() {
        let mut health = Health {
            connected_once: true,
            ..Health::default()
        };
        let kind = WatchKind::Custom(CustomKind::new("example.com", "v1", "Widget"));
        health.record(kind.clone(), failure(FailureKind::Forbidden));
        assert_eq!(health.status(), ConnectionStatus::Connected);
        health.record(kind, failure(FailureKind::Other));
        assert!(matches!(health.status(), ConnectionStatus::Degraded(_)));
        health.record(WatchKind::Secrets, failure(FailureKind::Forbidden));
        assert!(matches!(health.status(), ConnectionStatus::Degraded(ref f) if f.len() == 2));
    }

    const ROUND_TRIP: Duration = Duration::from_secs(1);
    const STATUS_DEADLINE: Duration = Duration::from_secs(5);
    const VERSION: &str = r#"{"major":"1","minor":"36","gitVersion":"v1.36.0","gitCommit":"","gitTreeState":"","buildDate":"","goVersion":"","compiler":"","platform":""}"#;
    const NODES: &str = r#"{"kind":"NodeList","apiVersion":"v1","metadata":{},"items":[{"metadata":{"name":"n"},"spec":{"providerID":"kind://docker/n"}}]}"#;

    #[tokio::test(start_paused = true)]
    async fn version_and_node_sample_are_fetched_together() {
        let (client, traffic) = crate::fake::client(ROUND_TRIP, |_, path| {
            let body = if path.starts_with("/version") {
                VERSION
            } else {
                NODES
            };
            Some((http::StatusCode::OK, body.to_owned()))
        });
        let started = Instant::now();
        let identified = identify(&client, Duration::from_secs(30)).await;
        assert!(started.elapsed() < ROUND_TRIP * 2);
        assert_eq!(traffic.peak(), 2);
        let Ok((version, provider_id)) = identified else {
            panic!("identify failed");
        };
        assert_eq!(version.git_version, "v1.36.0");
        assert_eq!(provider_id.as_deref(), Some("kind://docker/n"));
    }

    const RELEASE_ATTEMPTS: usize = 200;
    const RELEASE_POLL: Duration = Duration::from_millis(10);
    const LOOPBACK: &str = "127.0.0.1";

    async fn port_released(port: u16) -> bool {
        for _ in 0..RELEASE_ATTEMPTS {
            if tokio::net::TcpListener::bind((LOOPBACK, port))
                .await
                .is_ok()
            {
                return true;
            }
            tokio::time::sleep(RELEASE_POLL).await;
        }
        false
    }

    #[tokio::test]
    async fn disconnect_tears_down_everything_the_cluster_owns() {
        let (client, _) = crate::fake::client(Duration::ZERO, |_, _| None);
        let (manager, handle) = crate::fake::handle(&client);
        let retained = handle.clone();
        let Ok(forward) = handle.port_forward("apps", "web-1", 80, 0).await else {
            panic!("forward starts");
        };
        let port = forward.local_addr.port();
        let _pods = handle.subscribe(WatchKind::Pods);
        let watching = lock(&handle.shared.watches)
            .values()
            .any(|entry| entry.running.is_some());
        assert!(watching);
        let stream = tokio::spawn({
            let handle = handle.clone();
            async move { handle.until_closed(std::future::pending::<()>()).await }
        });
        drop(handle);

        assert!(manager.disconnect(retained.id()));

        assert!(retained.is_closed());
        assert!(matches!(retained.client(), Err(Error::NotConnected)));
        assert_eq!(retained.port_forwards(), Vec::new());
        let watching = lock(&retained.shared.watches)
            .values()
            .any(|entry| entry.running.is_some());
        assert!(!watching);
        assert!(matches!(stream.await, Ok(None)));
        assert!(port_released(port).await);
        for _ in 0..RELEASE_ATTEMPTS {
            if retained.shared.signals.is_closed() {
                break;
            }
            tokio::time::sleep(RELEASE_POLL).await;
        }
        assert!(retained.shared.signals.is_closed());
        retained.shared.set_client(Some(&client));
        assert!(matches!(retained.client(), Err(Error::NotConnected)));
    }

    const WATCH_ROUND_TRIP: Duration = Duration::from_millis(20);
    const SYNC_PATIENCE: Duration = Duration::from_secs(10);
    const EMPTY_DEPLOYMENTS: &str = r#"{"kind":"DeploymentList","apiVersion":"apps/v1","metadata":{"resourceVersion":"1"},"items":[]}"#;
    const FORBIDDEN_SECRETS: &str = r#"{"kind":"Status","apiVersion":"v1","status":"Failure","message":"secrets is forbidden: User cannot list resource secrets","reason":"Forbidden","code":403}"#;

    async fn settled(handle: &ClusterHandle, kind: &WatchKind) -> WatchState {
        let deadline = Instant::now() + SYNC_PATIENCE;
        loop {
            let state = handle.watch_state(kind);
            if !matches!(state, WatchState::Pending(_)) || Instant::now() > deadline {
                return state;
            }
            tokio::time::sleep(WATCH_ROUND_TRIP).await;
        }
    }

    #[tokio::test]
    async fn watch_states_tell_an_empty_list_from_a_forbidden_one_and_an_unstarted_one() {
        let (client, _) = crate::fake::client(WATCH_ROUND_TRIP, |_, path| {
            if path.contains("watch=true") {
                return Some((http::StatusCode::OK, String::new()));
            }
            if path.starts_with("/apis/apps/v1/deployments") {
                return Some((http::StatusCode::OK, EMPTY_DEPLOYMENTS.to_owned()));
            }
            if path.starts_with("/api/v1/secrets") {
                return Some((http::StatusCode::FORBIDDEN, FORBIDDEN_SECRETS.to_owned()));
            }
            None
        });
        let (_manager, handle, _events) = crate::fake::supervised(&client);
        let _deployments = handle.subscribe(WatchKind::Deployments);
        let _secrets = handle.subscribe(WatchKind::Secrets);
        assert_eq!(
            settled(&handle, &WatchKind::Deployments).await,
            WatchState::Synced
        );
        let secrets = settled(&handle, &WatchKind::Secrets).await;
        assert!(
            matches!(&secrets, WatchState::Failed(reason) if reason.contains("forbidden")),
            "{secrets:?}"
        );
        assert_eq!(
            handle.watch_state(&WatchKind::Jobs),
            WatchState::pending(NOT_STARTED)
        );
    }

    async fn status_matching(
        events: &async_channel::Receiver<ClusterEvent>,
        wanted: impl Fn(&ConnectionStatus) -> bool,
    ) -> bool {
        let deadline = Instant::now() + STATUS_DEADLINE;
        while Instant::now() < deadline {
            let next = tokio::time::timeout(STATUS_DEADLINE, events.recv()).await;
            if let Ok(Ok(ClusterEvent::Status { status, .. })) = next
                && wanted(&status)
            {
                return true;
            }
        }
        false
    }

    #[tokio::test]
    async fn dropping_a_failed_watch_returns_the_cluster_to_connected() {
        let (client, _) = crate::fake::client(WATCH_ROUND_TRIP, |_, path| {
            if path.starts_with("/api/v1/secrets") {
                return Some((http::StatusCode::FORBIDDEN, FORBIDDEN_SECRETS.to_owned()));
            }
            None
        });
        let (_manager, handle, events) = crate::fake::supervised(&client);
        let secrets = handle.subscribe(WatchKind::Secrets);
        let degraded = status_matching(&events, |status| {
            matches!(status, ConnectionStatus::Degraded(_))
        })
        .await;
        assert!(degraded);
        drop(secrets);
        let connected =
            status_matching(&events, |status| *status == ConnectionStatus::Connected).await;
        assert!(connected);
        assert_eq!(handle.status(), ConnectionStatus::Connected);
    }

    #[test]
    fn an_unreachable_cluster_fails_every_watch_with_its_reason() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        let Ok(runtime) = runtime else {
            panic!("runtime");
        };
        runtime.block_on(async {
            let (client, _) = crate::fake::client(Duration::ZERO, |_, _| None);
            let (_manager, handle) = crate::fake::handle(&client);
            assert_eq!(
                handle.watch_state(&WatchKind::Pods),
                WatchState::pending(STEP_CREDENTIALS)
            );
            let mut offline = failure(FailureKind::Unreachable);
            offline.detail = "Connection refused.".into();
            *lock(&handle.shared.status) = ConnectionStatus::Offline(offline);
            assert_eq!(
                handle.watch_state(&WatchKind::Pods),
                WatchState::Failed("cluster offline: Connection refused.".into())
            );
        });
    }

    const HANG_PATIENCE: Duration = Duration::from_secs(60);
    const FAST_ENOUGH: Duration = Duration::from_secs(10);

    fn kubeconfig(dir: &std::path::Path, name: &str, server: &str) -> KubeContext {
        let file = dir.join(format!("{name}.yaml"));
        let text = format!(
            "apiVersion: v1\nkind: Config\nclusters:\n- name: {name}\n  cluster:\n    server: {server}\ncontexts:\n- name: {name}\n  context:\n    cluster: {name}\n    user: {name}\nusers:\n- name: {name}\n  user:\n    token: abc\n"
        );
        assert!(std::fs::write(&file, text).is_ok());
        let Ok(parsed) = kube::config::Kubeconfig::read_from(&file) else {
            panic!("kubeconfig");
        };
        let found = crate::discovery::contexts_of(&file, &parsed)
            .into_iter()
            .next();
        let Some(context) = found else {
            panic!("context");
        };
        context
    }

    #[tokio::test]
    async fn a_hanging_cluster_does_not_delay_another() {
        let Ok(dir) = tempfile::tempdir() else {
            panic!("tempdir");
        };
        let Ok(hanging) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
            panic!("bind");
        };
        let Ok(address) = hanging.local_addr() else {
            panic!("addr");
        };
        let settings = KubeSettings {
            connect_timeout: HANG_PATIENCE,
            check_timeout: HANG_PATIENCE,
            ..KubeSettings::default()
        };
        let env = Arc::new(ShellEnv::from_process(String::new()));
        let (manager, events) = ClusterManager::new(Handle::current(), settings, env);
        let stuck = kubeconfig(dir.path(), "stuck", &format!("https://{address}"));
        let refused = kubeconfig(dir.path(), "refused", "https://127.0.0.1:1");
        let stuck_handle = manager.connect(stuck);
        let refused_handle = manager.connect(refused.clone());
        let offline = tokio::time::timeout(FAST_ENOUGH, async {
            while let Ok(event) = events.recv().await {
                if let ClusterEvent::Status {
                    id,
                    status: ConnectionStatus::Offline(_),
                } = event
                    && id == refused.id
                {
                    return true;
                }
            }
            false
        })
        .await;
        assert_eq!(offline, Ok(true));
        assert!(matches!(
            refused_handle.status(),
            ConnectionStatus::Offline(_)
        ));
        assert_eq!(stuck_handle.status(), ConnectionStatus::Connecting);
        drop(hanging);
    }
}
