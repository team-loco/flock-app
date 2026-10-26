use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::{Duration, Instant};

use flock_core::compare::{Field, SpecDiff};
use flock_core::flux::{FluxAction, FluxKey, FluxKind};
use flock_core::{
    Backend, ClusterId, Command, ContextRef, ExecEvent, ExecRequest, ExecTarget, NewCluster,
    NodeShellRequest, Notice, Receiver, ResourceRef, Seen, Sender, Sink, Snapshot, Source,
    TermSize, Uid, Update,
};
use flock_kube::LiveBackend;
use flock_kube::discovery::{DiscoverySources, KubeContext, discover};
use flock_kube::settings::KubeSettings;
use flock_kube::store::ClusterStore;
use futures::FutureExt;
use k8s_openapi::api::apps::v1::{ControllerRevision, Deployment, ReplicaSet, StatefulSet};
use k8s_openapi::api::core::v1::{ConfigMap, Node, Pod, PodTemplateSpec, Service};
use kube::api::{ApiResource, DynamicObject, GroupVersionKind};
use kube::api::{DeleteParams, ListParams, Patch, PatchParams, PostParams};
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::{Api, Client, Config, ResourceExt};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

mod common;

use common::kubeconfig::kind_only_home;
use common::scratch::ScratchNamespace;
use common::{WAIT, eventually};

const E2E_VAR: &str = "FLOCK_E2E";
const SUPPORT_DIR: &str = "support";
const CONTEXT: &str = "kind-fleet-a";
const PODINFO_NAMESPACE: &str = "apps";
const PODINFO: &str = "podinfo";
const PODINFO_SELECTOR: &str = "app.kubernetes.io/name=podinfo";
const PODINFO_CONTAINER: &str = "podinfo";
const PODINFO_PORT: u16 = 9898;
const DEBUG_POD: &str = "flock-actions-e2e";
const DEBUG_IMAGE: &str = "busybox:1.37";
const NODE_SHELL_NAMESPACE: &str = "default";
const STALE_SHELL_AGE_SECONDS: i64 = 3_600;
const ECHO_MARKER: &str = "flock-e2e-exec";
const OTHER_CONTEXT: &str = "kind-fleet-b";
const SCRATCH_DEPLOYMENT: &str = "flock-compare-e2e";
const REVISION_ANNOTATION: &str = "deployment.kubernetes.io/revision";
const SCRATCH_ROLLBACK: &str = "flock-restart-rollback-e2e";
const SCRATCH_SCALE: &str = "flock-scale-e2e";
const SCALED_UP: u32 = 2;
const SCRATCH_STATEFUL_SET: &str = "flock-rollback-e2e";
const NEWER_IMAGE: &str = "busybox:1.36";
const CTRL_C: u8 = 0x03;
const FOREGROUND_SETTLE: Duration = Duration::from_secs(1);
const SHELL_OUTPUT_TIMEOUT: Duration = Duration::from_secs(20);
const SIZE_PROBE_ATTEMPTS: usize = 20;
const SIZE_PROBE_WAIT: Duration = Duration::from_millis(500);
const SHELL_SIZE: TermSize = TermSize { cols: 97, rows: 23 };

fn enabled() -> bool {
    std::env::var(E2E_VAR).is_ok_and(|value| value == "1")
}

fn kind_context(name: &str) -> Option<KubeContext> {
    let sources = DiscoverySources {
        kubeconfig_env: std::env::var_os("KUBECONFIG"),
        home: None,
        folders: Vec::new(),
    };
    let discovery = discover(&sources);
    discovery
        .contexts
        .into_iter()
        .find(|context| context.id.context == name)
}

struct Harness {
    scratch: ScratchNamespace,
    home: tempfile::TempDir,
    commands: Sender<Command>,
    updates: Receiver<Update>,
    client: Client,
}

async fn client_in(kubeconfig: &std::path::Path, name: &str) -> Client {
    let options = KubeConfigOptions {
        context: Some(name.to_owned()),
        ..KubeConfigOptions::default()
    };
    let Ok(kubeconfig) = Kubeconfig::read_from(kubeconfig) else {
        panic!("kind kubeconfig");
    };
    let Ok(config) = Config::from_custom_kubeconfig(kubeconfig, &options).await else {
        panic!("{name} client config");
    };
    let Ok(client) = Client::try_from(config) else {
        panic!("{name} client");
    };
    client
}

fn cluster() -> ClusterId {
    ClusterId::new(CONTEXT)
}

fn resource(kind: &str, namespace: Option<&str>, name: &str) -> ResourceRef {
    ResourceRef {
        cluster: cluster(),
        kind: kind.into(),
        namespace: namespace.map(Into::into),
        name: name.into(),
    }
}

impl Harness {
    async fn start() -> Option<Self> {
        Self::start_with(&[CONTEXT]).await
    }

    async fn start_with(names: &[&str]) -> Option<Self> {
        if !enabled() {
            return None;
        }
        let contexts: Vec<KubeContext> = names
            .iter()
            .map(|name| {
                let Some(context) = kind_context(name) else {
                    panic!("{E2E_VAR}=1 needs KUBECONFIG with {name}");
                };
                context
            })
            .collect();
        let home = kind_only_home(&contexts);
        let file = home.path().join(".kube").join("config");
        let mut clients = Vec::new();
        for name in names {
            clients.push((*name, client_in(&file, name).await));
        }
        let clusters: Vec<(&str, &Client)> = clients.iter().map(|(name, c)| (*name, c)).collect();
        let scratch = ScratchNamespace::create(&clusters).await;
        let client = client_in(&file, CONTEXT).await;
        let sources = DiscoverySources {
            kubeconfig_env: None,
            home: Some(home.path().to_path_buf()),
            folders: Vec::new(),
        };
        let mut store = ClusterStore::empty(&home.path().join(SUPPORT_DIR));
        let added: Vec<NewCluster> = names
            .iter()
            .map(|name| NewCluster {
                context: ContextRef {
                    file: home.path().join(".kube").join("config"),
                    context: (*name).to_owned(),
                },
                name: (*name).to_owned(),
            })
            .collect();
        assert!(store.add(&added).is_ok());
        let backend = LiveBackend::new(KubeSettings::default(), sources, store);
        let (updates_tx, updates) = async_channel::unbounded();
        let (commands, commands_rx) = async_channel::unbounded();
        Box::new(backend).start(updates_tx, commands_rx);
        let harness = Self {
            scratch,
            home,
            commands,
            updates,
            client,
        };
        harness
            .snapshot_until(|snapshot| {
                snapshot
                    .cluster(&cluster())
                    .is_some_and(|summary| summary.nodes.is_some())
            })
            .await;
        Some(harness)
    }

    fn namespace(&self) -> &str {
        self.scratch.name()
    }

    fn send(&self, command: Command) {
        assert!(self.commands.try_send(command).is_ok());
    }

    async fn snapshot_until(&self, done: impl Fn(&Snapshot) -> bool) -> Snapshot {
        let deadline = Instant::now() + WAIT;
        let mut last = None;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let update = tokio::time::timeout(left, self.updates.recv()).await;
            let Ok(Ok(update)) = update else {
                panic!("snapshot condition never met; last cluster summary: {last:#?}");
            };
            if let Update::Snapshot(snapshot) = update {
                if done(&snapshot) {
                    return Arc::unwrap_or_clone(snapshot);
                }
                last = snapshot.cluster(&cluster()).cloned();
            }
        }
    }

    async fn first_pod(&self, namespace: &str, selector: &str) -> String {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), namespace);
        let Ok(listed) = pods.list(&ListParams::default().labels(selector)).await else {
            panic!("list pods");
        };
        let Some(pod) = listed.items.first() else {
            panic!("no pod for {selector}");
        };
        pod.name_any()
    }
}

async fn exec_events(events: &Receiver<ExecEvent>) -> (Vec<ExecEvent>, String) {
    let mut seen = Vec::new();
    let mut output = String::new();
    while let Ok(Ok(event)) = tokio::time::timeout(WAIT, events.recv()).await {
        if let ExecEvent::Output(bytes) = &event {
            output.push_str(&String::from_utf8_lossy(bytes));
        }
        let exited = matches!(event, ExecEvent::Exited { .. });
        seen.push(event);
        if exited {
            break;
        }
    }
    (seen, output)
}

struct Shell {
    input: Sender<Vec<u8>>,
    resize: Sender<TermSize>,
    events: Receiver<ExecEvent>,
}

fn run_shell(harness: &Harness, target: ExecTarget) -> Shell {
    let (input, source) = Source::channel();
    let (resize, sizes) = Source::channel();
    let (output, events) = Sink::channel();
    harness.send(Command::Exec(ExecRequest {
        target,
        input: source,
        resize: sizes,
        output,
    }));
    Shell {
        input,
        resize,
        events,
    }
}

async fn shell_output_until(events: &Receiver<ExecEvent>, marker: &str) -> String {
    let deadline = Instant::now() + SHELL_OUTPUT_TIMEOUT;
    let mut output = String::new();
    while !output.contains(marker) {
        let left = deadline.saturating_duration_since(Instant::now());
        let event = tokio::time::timeout(left, events.recv()).await;
        match event {
            Ok(Ok(ExecEvent::Output(bytes))) => output.push_str(&String::from_utf8_lossy(&bytes)),
            Ok(Ok(ExecEvent::Started { .. })) => {}
            other => panic!("shell did not print {marker:?}: {output:?}; {other:?}"),
        }
    }
    output
}

async fn await_tty_size(
    input: &Sender<Vec<u8>>,
    events: &Receiver<ExecEvent>,
    size: &str,
) -> Vec<ExecEvent> {
    let mut seen = Vec::new();
    for _ in 0..SIZE_PROBE_ATTEMPTS {
        assert!(input.send(b"stty size\r".to_vec()).await.is_ok());
        let mut output = String::new();
        while let Ok(Ok(event)) = tokio::time::timeout(SIZE_PROBE_WAIT, events.recv()).await {
            if let ExecEvent::Output(bytes) = &event {
                output.push_str(&String::from_utf8_lossy(bytes));
            }
            seen.push(event);
            if output.contains(size) {
                return seen;
            }
        }
    }
    panic!("the shell never reported {size:?}: {seen:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_resizes_colours_edits_completes_and_cancels() {
    let Some(harness) = Harness::start().await else {
        return;
    };
    let pod = harness.first_pod(PODINFO_NAMESPACE, PODINFO_SELECTOR).await;
    let target = ExecTarget {
        pod: resource("Pod", Some(PODINFO_NAMESPACE), &pod),
        container: PODINFO_CONTAINER.into(),
        debug_image: None,
    };
    let shell = run_shell(&harness, target);
    shell_output_until(&shell.events, "$ ").await;
    assert!(shell.resize.send(SHELL_SIZE).await.is_ok());
    assert!(shell.input.send(b"stty size\r".to_vec()).await.is_ok());
    let size = format!("{} {}", SHELL_SIZE.rows, SHELL_SIZE.cols);
    shell_output_until(&shell.events, &size).await;
    assert!(
        shell
            .input
            .send(b"printf '\\033[31mFLOCK-%s\\033[0m\\n' oops\x7f\x7f\x7f\x7fOK\r".to_vec())
            .await
            .is_ok()
    );
    let colored = shell_output_until(&shell.events, "FLOCK-OK").await;
    assert!(colored.contains("\x1b[31mFLOCK-OK\x1b[0m"), "{colored:?}");
    assert!(
        shell
            .input
            .send(b"printf '%s\\n' ac\x1b[Db\x1b[C\r".to_vec())
            .await
            .is_ok()
    );
    shell_output_until(&shell.events, "abc\r\n").await;
    assert!(shell.input.send(b"ech\t".to_vec()).await.is_ok());
    shell_output_until(&shell.events, "echo ").await;
    assert!(shell.input.send(b"TAB-OK\r".to_vec()).await.is_ok());
    shell_output_until(&shell.events, "\nTAB-OK\r\n").await;
    assert!(shell.input.send(b"sleep 30\r".to_vec()).await.is_ok());
    shell_output_until(&shell.events, "sleep 30\r\n").await;
    tokio::time::sleep(FOREGROUND_SETTLE).await;
    assert!(shell.input.send(vec![CTRL_C]).await.is_ok());
    shell_output_until(&shell.events, "$ ").await;
    assert!(
        shell
            .input
            .send(b"echo CTRL-$((20 + 2))\r".to_vec())
            .await
            .is_ok()
    );
    shell_output_until(&shell.events, "CTRL-22\r\n").await;
    drop(shell.events);
    eventually(
        "exec input closed after receiver dropped",
        || async { shell.input.is_closed() },
        |closed| *closed,
    )
    .await;
    assert!(shell.resize.is_closed());
}

#[tokio::test(flavor = "multi_thread")]
async fn exec_runs_a_shell_with_a_tty() {
    let Some(harness) = Harness::start().await else {
        return;
    };
    let pod = harness.first_pod(PODINFO_NAMESPACE, PODINFO_SELECTOR).await;
    let target = ExecTarget {
        pod: resource("Pod", Some(PODINFO_NAMESPACE), &pod),
        container: PODINFO_CONTAINER.into(),
        debug_image: None,
    };
    let Shell {
        input,
        resize,
        events,
    } = run_shell(&harness, target);
    assert!(resize.send(SHELL_SIZE).await.is_ok());
    let size = format!("{} {}", SHELL_SIZE.rows, SHELL_SIZE.cols);
    let mut seen = await_tty_size(&input, &events, &size).await;
    let script = format!("echo {ECHO_MARKER}-$((40 + 2)) $(stty size)\rexit\r");
    assert!(input.send(script.into_bytes()).await.is_ok());
    let (rest, output) = exec_events(&events).await;
    seen.extend(rest);
    assert!(
        matches!(seen.first(), Some(ExecEvent::Started { .. })),
        "{seen:?}"
    );
    let size = format!("{ECHO_MARKER}-42 {} {}", SHELL_SIZE.rows, SHELL_SIZE.cols);
    assert!(output.contains(&size), "{output}");
    assert_eq!(seen.last(), Some(&ExecEvent::Exited { code: Some(0) }));
}

fn debug_pod() -> Pod {
    let Ok(pod) = serde_json::from_value(json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": { "name": DEBUG_POD },
        "spec": {
            "terminationGracePeriodSeconds": 0,
            "containers": [{
                "name": "main",
                "image": DEBUG_IMAGE,
                "command": ["sleep", "3600"]
            }]
        }
    })) else {
        panic!("pod fixture");
    };
    pod
}

async fn recreate(pods: &Api<Pod>) {
    let _ = pods.delete(DEBUG_POD, &DeleteParams::default()).await;
    eventually(
        "old debug pod deleted",
        || async { pods.get_opt(DEBUG_POD).await.ok().flatten() },
        Option::is_none,
    )
    .await;
    let created = pods.create(&PostParams::default(), &debug_pod()).await;
    assert!(created.is_ok(), "{created:?}");
    eventually(
        "debug pod running",
        || async { pods.get(DEBUG_POD).await.ok().and_then(|pod| pod.status) },
        |status| status.as_ref().and_then(|status| status.phase.as_deref()) == Some("Running"),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn debug_shell_attaches_an_ephemeral_container() {
    let Some(harness) = Harness::start().await else {
        return;
    };
    let pods: Api<Pod> = Api::namespaced(harness.client.clone(), harness.namespace());
    recreate(&pods).await;
    let target = ExecTarget {
        pod: resource("Pod", Some(harness.namespace()), DEBUG_POD),
        container: "main".into(),
        debug_image: Some(DEBUG_IMAGE.into()),
    };
    let Shell { input, events, .. } = run_shell(&harness, target);
    let Ok(Ok(started)) = tokio::time::timeout(WAIT, events.recv()).await else {
        panic!("debug session never started");
    };
    assert!(
        matches!(&started, ExecEvent::Started { note } if note.contains("Debug container: debugger-")),
        "{started:?}"
    );
    let script = format!("echo {ECHO_MARKER}\rexit\r");
    assert!(input.send(script.into_bytes()).await.is_ok());
    let (seen, output) = exec_events(&events).await;
    let _ = pods.delete(DEBUG_POD, &DeleteParams::default()).await;
    assert!(output.contains(ECHO_MARKER), "{output}");
    assert!(
        matches!(seen.last(), Some(ExecEvent::Exited { .. })),
        "{seen:?}"
    );
}

async fn http_get(port: u16) -> String {
    let Ok(mut socket) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await else {
        panic!("connect to forward");
    };
    let request = "GET / HTTP/1.0\r\nHost: localhost\r\n\r\n";
    assert!(socket.write_all(request.as_bytes()).await.is_ok());
    let mut response = String::new();
    let read = tokio::time::timeout(WAIT, socket.read_to_string(&mut response)).await;
    assert!(read.is_ok(), "forward read timed out");
    response
}

async fn assert_forward_torn_down(mut idle: tokio::net::TcpStream) {
    let mut buffer = [0_u8; 1];
    let read = tokio::time::timeout(WAIT, idle.read(&mut buffer)).await;
    let Ok(closed) = read else {
        panic!("open connection survived the forward being stopped");
    };
    assert!(!matches!(closed, Ok(read) if read > 0), "{closed:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn port_forward_a_service_then_stop_it() {
    let Some(harness) = Harness::start().await else {
        return;
    };
    let service = resource("Service", Some(PODINFO_NAMESPACE), PODINFO);
    harness.send(Command::PortForward {
        service: service.clone(),
        local: 0,
        remote: PODINFO_PORT,
    });
    let snapshot = harness
        .snapshot_until(|snapshot| !snapshot.forwards.is_empty())
        .await;
    let Some(forward) = snapshot.forwards.first().cloned() else {
        panic!("forward listed");
    };
    assert_eq!(forward.target, service);
    assert_ne!(forward.local, 0);
    let response = http_get(forward.local).await;
    assert!(response.starts_with("HTTP/1.0 200"), "{response}");
    assert!(response.contains("\"hostname\""), "{response}");
    let Ok(idle) = tokio::net::TcpStream::connect(("127.0.0.1", forward.local)).await else {
        panic!("connect to forward");
    };
    harness.send(Command::StopPortForward {
        local: forward.local,
    });
    harness
        .snapshot_until(|snapshot| snapshot.forwards.is_empty())
        .await;
    assert_forward_torn_down(idle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn disconnect_closes_the_clusters_forwards() {
    let Some(harness) = Harness::start().await else {
        return;
    };
    let service = resource("Service", Some(PODINFO_NAMESPACE), PODINFO);
    harness.send(Command::PortForward {
        service,
        local: 0,
        remote: PODINFO_PORT,
    });
    let snapshot = harness
        .snapshot_until(|snapshot| !snapshot.forwards.is_empty())
        .await;
    let Some(forward) = snapshot.forwards.first().cloned() else {
        panic!("forward listed");
    };
    assert!(http_get(forward.local).await.starts_with("HTTP/1.0 200"));
    let Ok(idle) = tokio::net::TcpStream::connect(("127.0.0.1", forward.local)).await else {
        panic!("connect to forward");
    };
    harness.send(Command::Disconnect(cluster()));
    harness
        .snapshot_until(|snapshot| snapshot.forwards.is_empty())
        .await;
    assert_forward_torn_down(idle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn port_forward_failures_become_notices() {
    let Some(harness) = Harness::start().await else {
        return;
    };
    harness.send(Command::PortForward {
        service: resource("Service", Some(PODINFO_NAMESPACE), "no-such-service"),
        local: 0,
        remote: PODINFO_PORT,
    });
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok(Ok(update)) = tokio::time::timeout(left, harness.updates.recv()).await else {
            panic!("no notice");
        };
        if let Update::Notice(notice) = update {
            assert_eq!(notice.title, "Port-forward svc/no-such-service failed");
            let detail = notice.detail.unwrap_or_default();
            assert!(detail.starts_with("kind-fleet-a · Not found:"), "{detail}");
            return;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn port_forward_a_pod_port_from_the_pods_page() {
    let Some(harness) = Harness::start().await else {
        return;
    };
    let pod = harness.first_pod(PODINFO_NAMESPACE, PODINFO_SELECTOR).await;
    let target = resource("Pod", Some(PODINFO_NAMESPACE), &pod);
    harness.send(Command::PortForward {
        service: target.clone(),
        local: 0,
        remote: PODINFO_PORT,
    });
    let snapshot = harness
        .snapshot_until(|snapshot| !snapshot.forwards.is_empty())
        .await;
    let Some(forward) = snapshot.forwards.first().cloned() else {
        panic!("forward listed");
    };
    assert_eq!(forward.target, target);
    let response = http_get(forward.local).await;
    assert!(response.starts_with("HTTP/1.0 200"), "{response}");
    harness.send(Command::StopPortForward {
        local: forward.local,
    });
    harness
        .snapshot_until(|snapshot| snapshot.forwards.is_empty())
        .await;
}

const WEB_NAMESPACE: &str = "web";
const FLUX_NAMESPACE: &str = "flux-system";
const KUSTOMIZATION: &str = "apps";
const NODE: &str = "fleet-a-control-plane";
const SCRATCH_CONFIGMAP: &str = "flock-actions-e2e";
const SCRATCH_REPLACED: &str = "flock-replaced-e2e";
const REQUESTED_AT: &str = "reconcile.fluxcd.io/requestedAt";
const APPS_V1: &str = "apps/v1";
const CORE_V1: &str = "v1";
const KUSTOMIZE_V1: &str = "kustomize.toolkit.fluxcd.io/v1";
const STALE_UID: &str = "00000000-0000-0000-0000-000000000000";
const SCRATCH_KUSTOMIZATION: &str = "flock-force-e2e";
const PODINFO_SOURCE: &str = "podinfo";
const PINNED_IP: &str = "10.96.201.10";
const CHANGED_IP: &str = "10.96.201.11";
const IMMUTABLE: &str = "may not change once set";
const REPLACED: &str = "was replaced after you confirmed";

async fn replicas(deployments: &Api<Deployment>) -> Option<i32> {
    let deployment = deployments.get(PODINFO).await.ok()?;
    deployment.spec?.replicas
}

async fn template(deployments: &Api<Deployment>) -> Option<PodTemplateSpec> {
    template_of(deployments, PODINFO).await
}

async fn template_of(deployments: &Api<Deployment>, name: &str) -> Option<PodTemplateSpec> {
    let deployment = deployments.get(name).await.ok()?;
    Some(deployment.spec?.template)
}

async fn seen_by(client: &Client, api_version: &str, target: ResourceRef) -> Seen {
    let (group, version) = api_version.split_once('/').unwrap_or(("", api_version));
    let gvk = GroupVersionKind::gvk(group, version, &target.kind);
    let resource = ApiResource::from_gvk(&gvk);
    let api: Api<DynamicObject> = match &target.namespace {
        Some(namespace) => Api::namespaced_with(client.clone(), namespace, &resource),
        None => Api::all_with(client.clone(), &resource),
    };
    let Ok(object) = api.get(&target.name).await else {
        panic!("{} {} exists", target.kind, target.name);
    };
    let uid = object.metadata.uid.unwrap_or_default();
    Seen::new(target, Uid::new(uid))
}

fn stale(target: ResourceRef) -> Seen {
    Seen::new(target, Uid::new(STALE_UID))
}

async fn next_notice(harness: &Harness) -> Notice {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok(Ok(update)) = tokio::time::timeout(left, harness.updates.recv()).await else {
            panic!("no notice");
        };
        if let Update::Notice(notice) = update {
            return notice;
        }
    }
}

fn replaced(notice: &Notice, title: &str) {
    assert_eq!(notice.title, title);
    let detail = notice.detail.clone().unwrap_or_default();
    assert!(detail.contains(REPLACED), "{detail}");
}

fn no_notices(harness: &Harness) {
    while let Ok(update) = harness.updates.try_recv() {
        if let Update::Notice(notice) = update {
            panic!("unexpected notice: {notice:?}");
        }
    }
}

async fn restoring<T>(body: impl Future<Output = T>, restore: impl Future<Output = ()>) -> T {
    let outcome = AssertUnwindSafe(body).catch_unwind().await;
    restore.await;
    match outcome {
        Ok(value) => value,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

async fn scratch_replicas(deployments: &Api<Deployment>) -> Option<i32> {
    let deployment = deployments.get(SCRATCH_SCALE).await.ok()?;
    deployment.spec?.replicas
}

#[tokio::test(flavor = "multi_thread")]
async fn scale_up_then_back_down() {
    let Some(harness) = Harness::start().await else {
        return;
    };
    let namespace = harness.namespace().to_owned();
    let deployments: Api<Deployment> = Api::namespaced(harness.client.clone(), &namespace);
    let scratch = scratch_deployment(SCRATCH_SCALE, DEBUG_IMAGE);
    let created = deployments.create(&PostParams::default(), &scratch).await;
    assert!(created.is_ok(), "scratch deployment: {created:?}");
    let target = resource("Deployment", Some(&namespace), SCRATCH_SCALE);
    let seen = seen_by(&harness.client, APPS_V1, target).await;
    harness.send(Command::Scale {
        target: seen.clone(),
        replicas: SCALED_UP,
    });
    eventually(
        "scale up",
        || scratch_replicas(&deployments),
        |observed| *observed == i32::try_from(SCALED_UP).ok(),
    )
    .await;
    harness.send(Command::Scale {
        target: seen,
        replicas: 0,
    });
    eventually(
        "scale down",
        || scratch_replicas(&deployments),
        |observed| *observed == Some(0),
    )
    .await;
    no_notices(&harness);
}

async fn restart_and_rollback(harness: &Harness, deployments: &Api<Deployment>) {
    let scratch = scratch_deployment(SCRATCH_ROLLBACK, DEBUG_IMAGE);
    let created = deployments.create(&PostParams::default(), &scratch).await;
    let Ok(created) = created else {
        panic!("scratch deployment: {created:?}");
    };
    let Some(original) = template_of(deployments, SCRATCH_ROLLBACK).await else {
        panic!("scratch template");
    };
    let sets: Api<ReplicaSet> = Api::namespaced(harness.client.clone(), harness.namespace());
    eventually(
        "initial replica set revision",
        || async { sets.list(&ListParams::default()).await.ok() },
        |listed| {
            listed.as_ref().is_some_and(|listed| {
                listed.items.iter().any(|set| {
                    set.metadata.owner_references.iter().flatten().any(|owner| {
                        owner.controller == Some(true)
                            && Some(&owner.uid) == created.metadata.uid.as_ref()
                    }) && set
                        .metadata
                        .annotations
                        .as_ref()
                        .is_some_and(|annotations| annotations.contains_key(REVISION_ANNOTATION))
                })
            })
        },
    )
    .await;
    let target = resource("Deployment", Some(harness.namespace()), SCRATCH_ROLLBACK);
    let target = seen_by(&harness.client, APPS_V1, target).await;
    harness.send(Command::RestartRollout(target.clone()));
    eventually(
        "restart annotation",
        || template_of(deployments, SCRATCH_ROLLBACK),
        |observed| observed.as_ref() != Some(&original) && observed.is_some(),
    )
    .await;
    harness.send(Command::Rollback {
        target,
        revision: 0,
    });
    eventually(
        "rollback",
        || template_of(deployments, SCRATCH_ROLLBACK),
        |observed| observed.as_ref() == Some(&original),
    )
    .await;
    no_notices(harness);
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_then_roll_back_to_the_original_template() {
    let Some(harness) = Harness::start().await else {
        return;
    };
    let deployments: Api<Deployment> = Api::namespaced(harness.client.clone(), harness.namespace());
    let _ = deployments
        .delete(SCRATCH_ROLLBACK, &DeleteParams::default())
        .await;
    eventually(
        "old rollback deployment gone",
        || async { deployments.get_opt(SCRATCH_ROLLBACK).await.ok().flatten() },
        Option::is_none,
    )
    .await;
    let outcome = AssertUnwindSafe(restart_and_rollback(&harness, &deployments))
        .catch_unwind()
        .await;
    let deleted = deployments
        .delete(SCRATCH_ROLLBACK, &DeleteParams::default())
        .await;
    assert!(deleted.is_ok() || outcome.is_err(), "{deleted:?}");
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

fn scratch_stateful_set(image: &str) -> StatefulSet {
    let Ok(set) = serde_json::from_value(json!({
        "metadata": { "name": SCRATCH_STATEFUL_SET },
        "spec": {
            "replicas": 0,
            "serviceName": SCRATCH_STATEFUL_SET,
            "selector": { "matchLabels": { "app": SCRATCH_STATEFUL_SET } },
            "template": {
                "metadata": { "labels": { "app": SCRATCH_STATEFUL_SET } },
                "spec": { "containers": [{ "name": "main", "image": image, "command": ["sleep", "3600"] }] }
            }
        }
    })) else {
        panic!("statefulset fixture");
    };
    set
}

async fn stateful_image(sets: &Api<StatefulSet>) -> Option<String> {
    let set = sets.get(SCRATCH_STATEFUL_SET).await.ok()?;
    set.spec?.template.spec?.containers.first()?.image.clone()
}

async fn revisions_of(client: &Client, namespace: &str) -> usize {
    let history: Api<ControllerRevision> = Api::namespaced(client.clone(), namespace);
    let listed = history.list(&ListParams::default()).await;
    listed.map_or(0, |list| {
        list.items
            .iter()
            .filter(|revision| {
                revision
                    .metadata
                    .name
                    .as_deref()
                    .is_some_and(|name| name.starts_with(SCRATCH_STATEFUL_SET))
            })
            .count()
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn stateful_set_rolls_back_through_controller_revisions() {
    let Some(harness) = Harness::start().await else {
        return;
    };
    let sets: Api<StatefulSet> = Api::namespaced(harness.client.clone(), harness.namespace());
    let _ = sets
        .delete(SCRATCH_STATEFUL_SET, &DeleteParams::default())
        .await;
    eventually(
        "old statefulset gone",
        || async { sets.get_opt(SCRATCH_STATEFUL_SET).await.ok().flatten() },
        Option::is_none,
    )
    .await;
    let created = sets
        .create(&PostParams::default(), &scratch_stateful_set(DEBUG_IMAGE))
        .await;
    assert!(created.is_ok(), "{created:?}");
    eventually(
        "first revision",
        || revisions_of(&harness.client, harness.namespace()),
        |count| *count == 1,
    )
    .await;
    let changed = json!({ "spec": { "template": { "spec": { "containers": [{ "name": "main", "image": NEWER_IMAGE }] } } } });
    let patched = sets
        .patch(
            SCRATCH_STATEFUL_SET,
            &PatchParams::default(),
            &Patch::Strategic(changed),
        )
        .await;
    assert!(patched.is_ok(), "{patched:?}");
    eventually(
        "second revision",
        || revisions_of(&harness.client, harness.namespace()),
        |count| *count == 2,
    )
    .await;
    let target = resource(
        "StatefulSet",
        Some(harness.namespace()),
        SCRATCH_STATEFUL_SET,
    );
    let target = seen_by(&harness.client, APPS_V1, target).await;
    harness.send(Command::Rollback {
        target,
        revision: 0,
    });
    eventually(
        "rolled back",
        || stateful_image(&sets),
        |image| image.as_deref() == Some(DEBUG_IMAGE),
    )
    .await;
    no_notices(&harness);
    let deleted = sets
        .delete(SCRATCH_STATEFUL_SET, &DeleteParams::default())
        .await;
    assert!(deleted.is_ok(), "{deleted:?}");
}

fn scratch_deployment(name: &str, image: &str) -> Deployment {
    let Ok(deployment) = serde_json::from_value(json!({
        "metadata": { "name": name },
        "spec": {
            "replicas": 0,
            "selector": { "matchLabels": { "app": name } },
            "template": {
                "metadata": { "labels": { "app": name } },
                "spec": { "containers": [{ "name": "main", "image": image, "command": ["sleep", "3600"] }] }
            }
        }
    })) else {
        panic!("deployment fixture");
    };
    deployment
}

async fn deployment_image(deployments: &Api<Deployment>) -> Option<String> {
    let deployment = deployments.get(SCRATCH_DEPLOYMENT).await.ok()?;
    deployment
        .spec?
        .template
        .spec?
        .containers
        .first()?
        .image
        .clone()
}

async fn recreate_scratch(deployments: &Api<Deployment>, image: &str) {
    let _ = deployments
        .delete(SCRATCH_DEPLOYMENT, &DeleteParams::default())
        .await;
    eventually(
        "old scratch deployment gone",
        || async { deployments.get_opt(SCRATCH_DEPLOYMENT).await.ok().flatten() },
        Option::is_none,
    )
    .await;
    let scratch = scratch_deployment(SCRATCH_DEPLOYMENT, image);
    let created = deployments.create(&PostParams::default(), &scratch).await;
    assert!(created.is_ok(), "{created:?}");
}

async fn compare_and_apply(harness: &Harness, ours: &Api<Deployment>, theirs: &Api<Deployment>) {
    recreate_scratch(ours, DEBUG_IMAGE).await;
    recreate_scratch(theirs, NEWER_IMAGE).await;
    let target = resource("Deployment", Some(harness.namespace()), SCRATCH_DEPLOYMENT);
    harness.send(Command::Compare(target.clone()));
    let snapshot = harness
        .snapshot_until(|snapshot| {
            snapshot.comparison(&target).is_some_and(|comparison| {
                comparison.clusters.len() == 2
                    && comparison.spec.as_ref().is_some_and(SpecDiff::has_changes)
            })
        })
        .await;
    let Some(comparison) = snapshot.comparison(&target) else {
        panic!("comparison");
    };
    let columns: Vec<&str> = comparison
        .clusters
        .iter()
        .map(|c| c.cluster.as_str())
        .collect();
    assert_eq!(columns, [CONTEXT, OTHER_CONTEXT]);
    let image = comparison.fields.iter().find(|f| f.label == "Image");
    assert!(image.is_some_and(Field::differs), "{comparison:#?}");
    harness.send(Command::CopySpec {
        from: ClusterId::new(OTHER_CONTEXT),
        to: comparison.seen_on(&cluster()),
        path: "spec.template".into(),
    });
    eventually(
        "template applied",
        || deployment_image(ours),
        |image| image.as_deref() == Some(NEWER_IMAGE),
    )
    .await;
    harness
        .snapshot_until(|snapshot| {
            snapshot
                .comparison(&target)
                .and_then(|comparison| comparison.spec.as_ref())
                .is_some_and(|spec| !spec.has_changes())
        })
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_diffs_two_clusters_and_applies_the_other_template() {
    let Some(harness) = Harness::start_with(&[CONTEXT, OTHER_CONTEXT]).await else {
        return;
    };
    let ours: Api<Deployment> = Api::namespaced(harness.client.clone(), harness.namespace());
    let file = harness.home.path().join(".kube").join("config");
    let other_client = client_in(&file, OTHER_CONTEXT).await;
    let theirs: Api<Deployment> = Api::namespaced(other_client, harness.namespace());
    let outcome = AssertUnwindSafe(compare_and_apply(&harness, &ours, &theirs))
        .catch_unwind()
        .await;
    for deployments in [&ours, &theirs] {
        let deleted = deployments
            .delete(SCRATCH_DEPLOYMENT, &DeleteParams::default())
            .await;
        assert!(deleted.is_ok() || outcome.is_err(), "{deleted:?}");
    }
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

async fn cordon_and_uncordon(harness: &Harness, nodes: &Api<Node>) {
    let unschedulable = || async {
        nodes
            .get(NODE)
            .await
            .ok()
            .and_then(|node| node.spec?.unschedulable)
            .unwrap_or(false)
    };
    let target = seen_by(&harness.client, CORE_V1, resource("Node", None, NODE)).await;
    harness.send(Command::Cordon(target.clone()));
    eventually("cordon", unschedulable, |observed| *observed).await;
    harness.send(Command::Uncordon(target));
    eventually("uncordon", unschedulable, |observed| !*observed).await;
    no_notices(harness);
}

async fn uncordon_node(nodes: &Api<Node>) {
    let patch = json!({ "spec": { "unschedulable": false } });
    let patched = nodes
        .patch(NODE, &PatchParams::default(), &Patch::Merge(&patch))
        .await;
    assert!(patched.is_ok(), "restore node schedulable: {patched:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn cordon_then_uncordon_the_node() {
    let Some(harness) = Harness::start().await else {
        return;
    };
    let nodes: Api<Node> = Api::all(harness.client.clone());
    restoring(cordon_and_uncordon(&harness, &nodes), uncordon_node(&nodes)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_waits_until_the_object_is_gone() {
    let Some(harness) = Harness::start().await else {
        return;
    };
    let configmaps: Api<ConfigMap> = Api::namespaced(harness.client.clone(), harness.namespace());
    let Ok(scratch) = serde_json::from_value::<ConfigMap>(json!({
        "metadata": { "name": SCRATCH_CONFIGMAP },
        "data": { "key": "value" }
    })) else {
        panic!("configmap fixture");
    };
    let _ = configmaps.create(&PostParams::default(), &scratch).await;
    let target = resource("ConfigMap", Some(harness.namespace()), SCRATCH_CONFIGMAP);
    let target = seen_by(&harness.client, CORE_V1, target).await;
    harness.send(Command::Delete(target));
    eventually(
        "delete",
        || async { configmaps.get_opt(SCRATCH_CONFIGMAP).await.ok().flatten() },
        Option::is_none,
    )
    .await;
    no_notices(&harness);
}

#[tokio::test(flavor = "multi_thread")]
async fn flux_reconcile_requests_a_sync() {
    let Some(harness) = Harness::start().await else {
        return;
    };
    let key = FluxKey::new(
        CONTEXT,
        FluxKind::Kustomization,
        FLUX_NAMESPACE,
        KUSTOMIZATION,
    );
    let kustomizations = kustomizations(&harness.client, FLUX_NAMESPACE);
    let requested = || async {
        kustomizations
            .get(KUSTOMIZATION)
            .await
            .ok()
            .and_then(|object| object.annotations().get(REQUESTED_AT).cloned())
    };
    let before = requested().await;
    let target = resource("Kustomization", Some(FLUX_NAMESPACE), KUSTOMIZATION);
    let seen = seen_by(&harness.client, KUSTOMIZE_V1, target).await;
    harness.send(Command::Flux(
        key,
        seen.uid,
        FluxAction::ReconcileWithSource,
    ));
    eventually("reconcile request", requested, |now| {
        now.is_some() && *now != before
    })
    .await;
    no_notices(&harness);
}

#[tokio::test(flavor = "multi_thread")]
async fn unsupported_actions_explain_themselves() {
    let Some(harness) = Harness::start().await else {
        return;
    };
    harness.send(Command::Scale {
        target: stale(resource("DaemonSet", Some("kube-system"), "kube-proxy")),
        replicas: 2,
    });
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok(Ok(update)) = tokio::time::timeout(left, harness.updates.recv()).await else {
            panic!("no notice");
        };
        if let Update::Notice(notice) = update {
            assert_eq!(notice.title, "Scale ds/kube-proxy failed");
            assert_eq!(
                notice.detail.as_deref(),
                Some("kind-fleet-a · Scale is not supported for DaemonSet")
            );
            return;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_delete_confirmed_for_a_replaced_object_leaves_the_new_one() {
    let Some(harness) = Harness::start().await else {
        return;
    };
    let configmaps: Api<ConfigMap> = Api::namespaced(harness.client.clone(), harness.namespace());
    let Ok(scratch) = serde_json::from_value::<ConfigMap>(json!({
        "metadata": { "name": SCRATCH_REPLACED },
        "data": { "key": "value" }
    })) else {
        panic!("configmap fixture");
    };
    let _ = configmaps
        .delete(SCRATCH_REPLACED, &DeleteParams::default())
        .await;
    let created = configmaps.create(&PostParams::default(), &scratch).await;
    assert!(created.is_ok(), "{created:?}");
    let target = resource("ConfigMap", Some(harness.namespace()), SCRATCH_REPLACED);
    let seen = seen_by(&harness.client, CORE_V1, target).await;
    let deleted = configmaps
        .delete(SCRATCH_REPLACED, &DeleteParams::default())
        .await;
    assert!(deleted.is_ok(), "{deleted:?}");
    let recreated = configmaps.create(&PostParams::default(), &scratch).await;
    let Ok(recreated) = recreated else {
        panic!("recreate: {recreated:?}");
    };
    harness.send(Command::Delete(seen));
    replaced(
        &next_notice(&harness).await,
        "Delete cm/flock-replaced-e2e failed",
    );
    let survivor = configmaps.get_opt(SCRATCH_REPLACED).await.ok().flatten();
    assert_eq!(
        survivor.and_then(|survivor| survivor.metadata.uid),
        recreated.metadata.uid
    );
    let _ = configmaps
        .delete(SCRATCH_REPLACED, &DeleteParams::default())
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn writes_confirmed_for_another_uid_change_nothing() {
    let Some(harness) = Harness::start().await else {
        return;
    };
    let deployments: Api<Deployment> = Api::namespaced(harness.client.clone(), WEB_NAMESPACE);
    let before = (replicas(&deployments).await, template(&deployments).await);
    let target = resource("Deployment", Some(WEB_NAMESPACE), PODINFO);
    harness.send(Command::Scale {
        target: stale(target.clone()),
        replicas: 7,
    });
    replaced(&next_notice(&harness).await, "Scale deploy/podinfo failed");
    harness.send(Command::RestartRollout(stale(target.clone())));
    replaced(
        &next_notice(&harness).await,
        "Restart deploy/podinfo failed",
    );
    harness.send(Command::Rollback {
        target: stale(target),
        revision: 0,
    });
    replaced(
        &next_notice(&harness).await,
        "Roll back deploy/podinfo failed",
    );
    let key = FluxKey::new(
        CONTEXT,
        FluxKind::Kustomization,
        FLUX_NAMESPACE,
        KUSTOMIZATION,
    );
    harness.send(Command::Flux(key, Uid::new(STALE_UID), FluxAction::Suspend));
    replaced(&next_notice(&harness).await, "Suspend ks/apps failed");
    let node = stale(resource("Node", None, NODE));
    harness.send(Command::Cordon(node));
    replaced(
        &next_notice(&harness).await,
        "Cordon node/fleet-a-control-plane failed",
    );
    let after = (replicas(&deployments).await, template(&deployments).await);
    assert_eq!(after, before);
}

fn kustomizations(client: &Client, namespace: &str) -> Api<DynamicObject> {
    let kustomization = ApiResource::from_gvk(&GroupVersionKind::gvk(
        "kustomize.toolkit.fluxcd.io",
        "v1",
        "Kustomization",
    ));
    Api::namespaced_with(client.clone(), namespace, &kustomization)
}

fn pinned_cluster_ip(ip: &str) -> serde_json::Value {
    let patch = format!("- op: add\n  path: /spec/clusterIP\n  value: {ip}\n");
    json!([{ "target": { "kind": "Service", "name": PODINFO }, "patch": patch }])
}

fn ready_of(object: &DynamicObject) -> Option<(String, String)> {
    let conditions = object.data.pointer("/status/conditions")?.as_array()?;
    let ready = conditions
        .iter()
        .find(|condition| condition["type"] == "Ready")?;
    let status = ready["status"].as_str()?.to_owned();
    let message = ready["message"].as_str().unwrap_or_default().to_owned();
    Some((status, message))
}

async fn service_identity(services: &Api<Service>) -> Option<(String, String)> {
    let service = services.get_opt(PODINFO).await.ok().flatten()?;
    let uid = service.metadata.uid?;
    let ip = service.spec?.cluster_ip?;
    Some((uid, ip))
}

async fn force_apply_body(harness: &Harness, kustomizations: &Api<DynamicObject>) {
    let namespace = harness.namespace();
    let services: Api<Service> = Api::namespaced(harness.client.clone(), namespace);
    let pinned = eventually(
        "pinned service",
        || service_identity(&services),
        |identity| identity.as_ref().is_some_and(|(_, ip)| ip == PINNED_IP),
    )
    .await;
    let changed = json!({ "spec": { "patches": pinned_cluster_ip(CHANGED_IP) } });
    let patched = kustomizations
        .patch(
            SCRATCH_KUSTOMIZATION,
            &PatchParams::default(),
            &Patch::Merge(&changed),
        )
        .await;
    let Ok(patched) = patched else {
        panic!("change cluster IP: {patched:?}");
    };
    let Some(changed_generation) = patched.metadata.generation else {
        panic!("kustomization has no generation");
    };
    eventually(
        "plain apply rejected",
        || async { kustomizations.get(SCRATCH_KUSTOMIZATION).await.ok() },
        |object| {
            object
                .as_ref()
                .and_then(ready_of)
                .is_some_and(|(status, message)| status == "False" && message.contains(IMMUTABLE))
        },
    )
    .await;
    let target = resource("Kustomization", Some(namespace), SCRATCH_KUSTOMIZATION);
    let seen = seen_by(&harness.client, KUSTOMIZE_V1, target).await;
    let key = FluxKey::new(
        CONTEXT,
        FluxKind::Kustomization,
        namespace,
        SCRATCH_KUSTOMIZATION,
    );
    harness.send(Command::Flux(key, seen.uid, FluxAction::Force));
    eventually(
        "service recreated",
        || service_identity(&services),
        |identity| {
            identity.as_ref().is_some_and(|(uid, ip)| {
                ip == CHANGED_IP && pinned.as_ref().is_some_and(|(old, _)| old != uid)
            })
        },
    )
    .await;
    let restored_generation = changed_generation + 2;
    eventually(
        "spec.force restored",
        || async { kustomizations.get(SCRATCH_KUSTOMIZATION).await.ok() },
        |object| {
            object.as_ref().is_some_and(|object| {
                object.metadata.generation == Some(restored_generation)
                    && object.data.pointer("/spec/force") == Some(&json!(false))
                    && ready_of(object).is_some_and(|(status, _)| status == "True")
            })
        },
    )
    .await;
    no_notices(harness);
}

#[tokio::test(flavor = "multi_thread")]
async fn force_apply_recreates_an_immutable_change_then_restores_spec_force() {
    let Some(harness) = Harness::start().await else {
        return;
    };
    let namespace = harness.namespace().to_owned();
    let kustomizations = kustomizations(&harness.client, &namespace);
    let fixture = json!({
        "apiVersion": KUSTOMIZE_V1,
        "kind": "Kustomization",
        "metadata": { "name": SCRATCH_KUSTOMIZATION },
        "spec": {
            "interval": "60m",
            "path": "./kustomize",
            "prune": true,
            "wait": false,
            "timeout": "1m",
            "force": false,
            "targetNamespace": namespace,
            "sourceRef": {
                "kind": "GitRepository",
                "name": PODINFO_SOURCE,
                "namespace": FLUX_NAMESPACE
            },
            "patches": pinned_cluster_ip(PINNED_IP)
        }
    });
    let Ok(object) = serde_json::from_value::<DynamicObject>(fixture) else {
        panic!("kustomization fixture");
    };
    let created = kustomizations.create(&PostParams::default(), &object).await;
    assert!(created.is_ok(), "{created:?}");
    restoring(force_apply_body(&harness, &kustomizations), async {
        let _ = kustomizations
            .delete(SCRATCH_KUSTOMIZATION, &DeleteParams::default())
            .await;
    })
    .await;
}

async fn foreign_shell_pod(pods: &Api<Pod>, name: &str, heartbeat: i64) {
    let body = json!({
        "metadata": {
            "name": name,
            "labels": {"flock.dev/node-shell": "foreign-run"},
            "annotations": {"flock.dev/heartbeat": heartbeat.to_string()},
        },
        "spec": {
            "terminationGracePeriodSeconds": 0,
            "containers": [{"name": "shell", "image": DEBUG_IMAGE, "command": ["sleep", "3600"]}],
        },
    });
    let Ok(pod) = serde_json::from_value::<Pod>(body) else {
        panic!("foreign pod");
    };
    let created = pods.create(&PostParams::default(), &pod).await;
    assert!(created.is_ok(), "{created:?}");
}

fn unix_now() -> i64 {
    let elapsed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    elapsed.map_or(0, |elapsed| {
        i64::try_from(elapsed.as_secs()).unwrap_or_default()
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn node_shell_runs_on_the_node_and_deletes_its_pod() {
    let Some(harness) = Harness::start().await else {
        return;
    };
    let nodes: Api<Node> = Api::all(harness.client.clone());
    let Ok(listed) = nodes.list(&ListParams::default()).await else {
        panic!("list nodes");
    };
    let Some(node) = listed.items.first().map(ResourceExt::name_any) else {
        panic!("no node");
    };
    let pods: Api<Pod> = Api::namespaced(harness.client.clone(), NODE_SHELL_NAMESPACE);
    let fresh = "node-shell-foreign-fresh";
    let stale = "node-shell-foreign-stale";
    foreign_shell_pod(&pods, fresh, unix_now()).await;
    foreign_shell_pod(&pods, stale, unix_now() - STALE_SHELL_AGE_SECONDS).await;
    let (input, source) = Source::channel();
    let (_resize, sizes) = Source::channel();
    let (output, events) = Sink::channel();
    harness.send(Command::NodeShell(NodeShellRequest {
        node: resource("Node", None, &node),
        image: DEBUG_IMAGE.into(),
        input: source,
        resize: sizes,
        output,
    }));
    let Ok(Ok(started)) = tokio::time::timeout(WAIT, events.recv()).await else {
        panic!("node shell never started");
    };
    let ExecEvent::Started { note } = &started else {
        panic!("{started:?}");
    };
    let Some(pod) = note.rsplit('/').next().map(str::to_owned) else {
        panic!("{note}");
    };
    assert!(pods.get_opt(&pod).await.ok().flatten().is_some(), "{pod}");
    eventually(
        "stale foreign shell deleted",
        || async { pods.get_opt(stale).await.ok().flatten() },
        Option::is_none,
    )
    .await;
    assert!(pods.get_opt(fresh).await.ok().flatten().is_some());
    assert!(input.send(b"hostname\r".to_vec()).await.is_ok());
    let output = shell_output_until(&events, &format!("{node}\r\n")).await;
    assert!(output.contains(&node), "{output}");
    assert!(input.send(b"exit\r".to_vec()).await.is_ok());
    let (seen, _) = exec_events(&events).await;
    assert!(
        matches!(seen.last(), Some(ExecEvent::Exited { .. })),
        "{seen:?}"
    );
    eventually(
        "node shell pod deleted",
        || async { pods.get_opt(&pod).await.ok().flatten() },
        Option::is_none,
    )
    .await;
    let removed = pods.delete(fresh, &DeleteParams::default()).await;
    assert!(removed.is_ok(), "{removed:?}");
}
