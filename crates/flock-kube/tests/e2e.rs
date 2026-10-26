use flock_core::overview::TimeRange;
use flock_core::resources::Resource;
use flock_core::{Backend, ClusterId, Command, ContextRef, NewCluster, Page, ResourceKind, Update};
use flock_kube::LiveBackend;
use flock_kube::checks::{CheckKind, CheckStatus, Verdict, run_checks};
use flock_kube::discovery::{DiscoverySources, KubeContext, discover};
use flock_kube::manager::{ClusterEvent, ClusterHandle, ClusterManager, ConnectionStatus};
use flock_kube::ops::{ExecExit, ExecRequest, LogRequest, PatchBody, object_ref};
use flock_kube::provider::Provider;
use flock_kube::resources::WatchKind;
use flock_kube::settings::KubeSettings;
use flock_kube::shell_env::ShellEnv;
use flock_kube::store::ClusterStore;
use k8s_openapi::api::core::v1::{ConfigMap, Pod};
use kube::api::{DeleteParams, PartialObjectMeta, PostParams, PropagationPolicy};
use kube::{Api, ResourceExt};
use serde_json::json;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

mod common;

use common::kubeconfig::kind_only_home;
use common::scratch::ScratchNamespace;
use common::{WAIT, eventually};

const E2E_VAR: &str = "FLOCK_E2E";
const CONTEXT: &str = "kind-fleet-a";
const POD: &str = "flock-kube-e2e";
const CONFIGMAP: &str = "flock-kube-e2e";
const IMAGE: &str = "busybox:1.37";
const HTTP_PORT: u16 = 8080;
const GREETING: &str = "hello from flock";
const SUPPORT_DIR: &str = "support";

fn enabled() -> bool {
    std::env::var(E2E_VAR).is_ok_and(|value| value == "1")
}

fn kind_context() -> Option<KubeContext> {
    let sources = DiscoverySources {
        kubeconfig_env: std::env::var_os("KUBECONFIG"),
        home: None,
        folders: Vec::new(),
    };
    let discovery = discover(&sources);
    discovery
        .contexts
        .into_iter()
        .find(|context| context.id.context == CONTEXT)
}

async fn connected(manager: &ClusterManager, context: KubeContext) -> ClusterHandle {
    let handle = manager.connect(context);
    eventually(
        "cluster connected",
        || async { handle.status() },
        |status| *status == ConnectionStatus::Connected,
    )
    .await;
    handle
}

fn test_pod() -> Pod {
    let script = format!(
        "echo {GREETING}; while true; do printf 'HTTP/1.1 200 OK\\r\\nContent-Length: 2\\r\\n\\r\\nok' | nc -l -p {HTTP_PORT}; done"
    );
    let Ok(pod) = serde_json::from_value(json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": { "name": POD },
        "spec": {
            "terminationGracePeriodSeconds": 0,
            "containers": [{
                "name": "main",
                "image": IMAGE,
                "command": ["sh", "-c", script],
                "ports": [{ "containerPort": HTTP_PORT }]
            }]
        }
    })) else {
        panic!("pod fixture");
    };
    pod
}

async fn wait_running(handle: &ClusterHandle) {
    eventually(
        "pod running",
        || async {
            let pods = handle.objects::<Pod>();
            pods.iter().find(|pod| pod.name_any() == POD).cloned()
        },
        |pod| {
            let phase = pod
                .as_ref()
                .and_then(|pod| pod.status.as_ref())
                .and_then(|status| status.phase.as_deref());
            phase == Some("Running")
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn checks_report_ready_for_kind() {
    if !enabled() {
        return;
    }
    let Some(context) = kind_context() else {
        return;
    };
    let env = ShellEnv::from_process(String::new());
    let report = run_checks(&context, &env, &KubeSettings::default()).await;
    assert_eq!(report.verdict, Verdict::Ready, "{report:#?}");
    assert_eq!(report.facts.provider, Provider::Kind);
    let flux = report
        .checks
        .iter()
        .find(|check| check.kind == CheckKind::Flux);
    assert_eq!(flux.map(|c| c.status), Some(CheckStatus::Ok), "{report:#?}");
    assert!(report.facts.flux.is_some());
    assert!(report.facts.version.is_some());
}

async fn recreate_pod(handle: &ClusterHandle, namespace: &str) {
    let Ok(client) = handle.client() else {
        panic!("client after connect");
    };
    let pod_api: Api<Pod> = Api::namespaced(client, namespace);
    let _ = pod_api.delete(POD, &DeleteParams::default()).await;
    eventually(
        "old pod deleted",
        || async { pod_api.get_opt(POD).await.ok().flatten() },
        Option::is_none,
    )
    .await;
    let created = pod_api.create(&PostParams::default(), &test_pod()).await;
    assert!(created.is_ok(), "{created:?}");
    wait_running(handle).await;
}

async fn check_logs(handle: &ClusterHandle, namespace: &str) {
    let request = LogRequest {
        namespace: namespace.to_owned(),
        pod: POD.to_owned(),
        tail_lines: Some(10),
        ..LogRequest::default()
    };
    let Ok(logs) = handle.logs(&request).await else {
        panic!("logs");
    };
    let first = tokio::time::timeout(WAIT, logs.lines.recv()).await;
    assert!(
        matches!(first, Ok(Ok(Ok(ref line))) if line == GREETING),
        "{first:?}"
    );
}

async fn check_exec(handle: &ClusterHandle, namespace: &str) {
    let exec = ExecRequest {
        namespace: namespace.to_owned(),
        pod: POD.to_owned(),
        container: None,
        command: vec!["echo".to_owned(), "flock-exec".to_owned()],
        tty: false,
    };
    let Ok(session) = handle.exec(&exec).await else {
        panic!("exec");
    };
    let mut output = Vec::new();
    while let Ok(Ok(chunk)) = tokio::time::timeout(WAIT, session.output.recv()).await {
        output.extend(chunk);
    }
    assert_eq!(String::from_utf8_lossy(&output).trim(), "flock-exec");
    let exit = tokio::time::timeout(WAIT, session.exit.recv()).await;
    assert!(matches!(exit, Ok(Ok(ExecExit::Success))), "{exit:?}");
}

async fn check_port_forward(handle: &ClusterHandle, namespace: &str) {
    let Ok(forward) = handle.port_forward(namespace, POD, HTTP_PORT, 0).await else {
        panic!("port-forward");
    };
    assert_eq!(handle.port_forwards(), vec![forward.clone()]);
    let Ok(mut socket) = tokio::net::TcpStream::connect(forward.local_addr).await else {
        panic!("connect to forward");
    };
    assert!(socket.write_all(b"GET / HTTP/1.0\r\n\r\n").await.is_ok());
    let mut response = String::new();
    let read = tokio::time::timeout(WAIT, socket.read_to_string(&mut response)).await;
    assert!(read.is_ok(), "forward read timed out");
    assert!(response.ends_with("ok"), "{response}");
    assert!(handle.stop_port_forward(forward.id));
    assert_eq!(handle.port_forwards(), Vec::new());
}

async fn check_patch_yaml_apply(handle: &ClusterHandle, namespace: &str) {
    let target = object_ref::<Pod>(Some(namespace), POD);
    let patch = PatchBody::Merge(json!({ "metadata": { "labels": { "flock-e2e": "patched" } } }));
    let Ok(patched) = handle.patch(&target, patch).await else {
        panic!("patch");
    };
    assert_eq!(
        patched.labels().get("flock-e2e").map(String::as_str),
        Some("patched")
    );
    let Ok(yaml) = handle.get_yaml(&target).await else {
        panic!("yaml");
    };
    assert!(yaml.contains("flock-e2e: patched"));
    assert!(!yaml.contains("managedFields"));

    let manifest = format!(
        "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: {CONFIGMAP}\n  namespace: {namespace}\ndata:\n  key: value\n"
    );
    let applied = handle.apply_yaml(&manifest, false).await;
    assert!(applied.is_ok(), "{applied:?}");
    let configmap = object_ref::<ConfigMap>(Some(namespace), CONFIGMAP);
    assert!(
        handle
            .delete(&configmap, PropagationPolicy::Background, None)
            .await
            .is_ok()
    );
    assert!(
        handle
            .delete(&target, PropagationPolicy::Background, None)
            .await
            .is_ok()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn watch_logs_exec_forward_patch() {
    if !enabled() {
        return;
    }
    let Some(context) = kind_context() else {
        return;
    };
    let env = Arc::new(ShellEnv::from_process(String::new()));
    let runtime = tokio::runtime::Handle::current();
    let (manager, events) = ClusterManager::new(runtime, KubeSettings::default(), env);
    let handle = connected(&manager, context).await;
    let Ok(client) = handle.client() else {
        panic!("client after connect");
    };
    let scratch = ScratchNamespace::create(&[(CONTEXT, &client)]).await;
    let namespace = scratch.name();
    let pods = handle.subscribe(WatchKind::Pods);
    let configmaps = handle.subscribe(WatchKind::ConfigMaps);
    recreate_pod(&handle, namespace).await;

    let mut saw_pods_changed = false;
    while let Ok(event) = events.try_recv() {
        if let ClusterEvent::Changed { kinds, .. } = event {
            saw_pods_changed |= kinds.contains(&WatchKind::Pods);
        }
    }
    assert!(saw_pods_changed);
    assert_ne!(handle.objects::<PartialObjectMeta<ConfigMap>>().len(), 0);

    check_logs(&handle, namespace).await;
    check_exec(&handle, namespace).await;
    check_port_forward(&handle, namespace).await;
    check_patch_yaml_apply(&handle, namespace).await;

    drop(pods);
    drop(configmaps);
    assert_eq!(handle.watched_kinds(), Vec::new());
}

#[test]
fn live_backend_publishes_kind_cluster() {
    if !enabled() {
        return;
    }
    let Some(context) = kind_context() else {
        return;
    };
    let home = kind_only_home(std::slice::from_ref(&context));
    let sources = DiscoverySources {
        kubeconfig_env: None,
        home: Some(home.path().to_path_buf()),
        folders: Vec::new(),
    };
    let mut store = ClusterStore::empty(&home.path().join(SUPPORT_DIR));
    let added = NewCluster {
        context: ContextRef {
            file: home.path().join(".kube").join("config"),
            context: CONTEXT.to_owned(),
        },
        name: CONTEXT.to_owned(),
    };
    assert!(store.add(&[added]).is_ok());
    let backend = LiveBackend::new(KubeSettings::default(), sources, store);
    let (updates_tx, updates) = async_channel::unbounded();
    let (commands, commands_rx) = async_channel::unbounded();
    Box::new(backend).start(updates_tx, commands_rx);
    let mut everywhere = flock_core::Preferences::default();
    everywhere.select_namespace(
        ResourceKind::Deployment.name(),
        flock_core::settings::NamespaceSelection::All,
    );
    let all_namespaces = Command::SetPreferences(ClusterId::new(CONTEXT), everywhere);
    assert!(commands.send_blocking(all_namespaces).is_ok());
    let select = Command::SelectCluster(Some(ClusterId::new(CONTEXT)));
    assert!(commands.send_blocking(select).is_ok());
    let page = Command::SelectPage(Page::Deployments);
    assert!(commands.send_blocking(page).is_ok());
    let deadline = std::time::Instant::now() + WAIT;
    let mut last = None;
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "no healthy snapshot; last cluster summary: {last:#?}"
        );
        let Ok(Update::Snapshot(snapshot)) = updates.recv_blocking() else {
            panic!("backend stopped");
        };
        let Some(cluster) = snapshot.cluster(&ClusterId::new(CONTEXT)) else {
            continue;
        };
        last = Some(cluster.clone());
        let deployments = cluster.nav.get(&Page::Deployments).and_then(|c| c.total);
        if cluster.nodes == Some(1) && deployments.is_some_and(|n| n > 0) {
            assert_eq!(cluster.distro, "kind");
            assert_eq!(cluster.region, "local");
            assert!(cluster.version.is_some());
            assert_eq!(snapshot.clusters.len(), 1);
            return;
        }
    }
}

const COMPARED_NAMESPACES: [&str; 4] = ["kube-system", "flux-system", "payments", "web"];

fn listed(snapshot: &flock_core::Snapshot, kind: ResourceKind) -> Option<Vec<Resource>> {
    let rows = snapshot.detail(&ClusterId::new(CONTEXT))?.resources(kind);
    (!rows.is_empty()).then(|| rows.to_vec())
}

fn names_in(rows: &[Resource], namespace: &str) -> Vec<String> {
    let mut names: Vec<String> = rows
        .iter()
        .filter(|row| row.namespace == namespace)
        .map(|row| row.name.clone())
        .collect();
    names.sort();
    names
}

async fn namespaced_names<K>(client: &kube::Client, namespace: &str) -> Vec<String>
where
    K: kube::Resource<Scope = k8s_openapi::NamespaceResourceScope, DynamicType = ()>
        + Clone
        + serde::de::DeserializeOwned
        + std::fmt::Debug,
{
    let api: Api<K> = Api::namespaced(client.clone(), namespace);
    let Ok(list) = api.list(&kube::api::ListParams::default()).await else {
        panic!("listing {namespace} failed");
    };
    let mut names: Vec<String> = list.items.iter().map(ResourceExt::name_any).collect();
    names.sort();
    names
}

#[tokio::test(flavor = "multi_thread")]
async fn filtering_the_cluster_wide_store_matches_a_namespaced_list() {
    if !enabled() {
        return;
    }
    let Some(context) = kind_context() else {
        return;
    };
    let env = ShellEnv::from_process(String::new());
    let settings = KubeSettings::default();
    let Ok(client) = flock_kube::client::connect(&context, &env, settings.connect_timeout).await
    else {
        panic!("client");
    };
    let home = kind_only_home(std::slice::from_ref(&context));
    let sources = DiscoverySources {
        kubeconfig_env: None,
        home: Some(home.path().to_path_buf()),
        folders: Vec::new(),
    };
    let mut store = ClusterStore::empty(&home.path().join(SUPPORT_DIR));
    let added = NewCluster {
        context: ContextRef {
            file: home.path().join(".kube").join("config"),
            context: CONTEXT.to_owned(),
        },
        name: CONTEXT.to_owned(),
    };
    assert!(store.add(&[added]).is_ok());
    let backend = LiveBackend::new(settings, sources, store);
    let (updates_tx, updates) = async_channel::unbounded();
    let (commands, commands_rx) = async_channel::unbounded();
    Box::new(backend).start(updates_tx, commands_rx);
    let cluster = ClusterId::new(CONTEXT);
    for command in [
        Command::SelectCluster(Some(cluster.clone())),
        Command::SelectPage(Page::Deployments),
    ] {
        assert!(commands.send(command).await.is_ok());
    }
    let deadline = tokio::time::Instant::now() + WAIT;
    let (deployments, services) = loop {
        let received = tokio::time::timeout_at(deadline, updates.recv()).await;
        let Ok(Ok(update)) = received else {
            panic!("no snapshot with deployments and services");
        };
        let Update::Snapshot(snapshot) = update else {
            continue;
        };
        let deployments = listed(&snapshot, ResourceKind::Deployment);
        let services = listed(&snapshot, ResourceKind::Service);
        if let (Some(deployments), Some(services)) = (deployments, services) {
            break (deployments, services);
        }
    };
    for namespace in COMPARED_NAMESPACES {
        let api = namespaced_names::<k8s_openapi::api::apps::v1::Deployment>(&client, namespace);
        assert_eq!(names_in(&deployments, namespace), api.await, "{namespace}");
        let api = namespaced_names::<k8s_openapi::api::core::v1::Service>(&client, namespace);
        assert_eq!(names_in(&services, namespace), api.await, "{namespace}");
    }
}

#[test]
fn live_utilization_has_hour_samples_and_six_hour_local_history() {
    if !enabled() {
        return;
    }
    let Some(context) = kind_context() else {
        panic!("kind-fleet-a context required");
    };
    let home = kind_only_home(std::slice::from_ref(&context));
    let sources = DiscoverySources {
        kubeconfig_env: None,
        home: Some(home.path().to_path_buf()),
        folders: Vec::new(),
    };
    let mut store = ClusterStore::empty(&home.path().join(SUPPORT_DIR));
    let added = NewCluster {
        context: ContextRef {
            file: home.path().join(".kube/config"),
            context: CONTEXT.to_owned(),
        },
        name: CONTEXT.to_owned(),
    };
    assert!(store.add(&[added]).is_ok());
    let backend = LiveBackend::new(KubeSettings::default(), sources, store);
    let (updates_tx, updates) = async_channel::unbounded();
    let (commands, commands_rx) = async_channel::unbounded();
    Box::new(backend).start(updates_tx, commands_rx);
    assert!(
        commands
            .send_blocking(Command::SelectCluster(Some(ClusterId::new(CONTEXT))))
            .is_ok()
    );
    assert!(
        commands
            .send_blocking(Command::SelectPage(Page::ClusterOverview))
            .is_ok()
    );
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "no local metrics history"
        );
        let Ok(Update::Snapshot(snapshot)) = updates.recv_blocking() else {
            continue;
        };
        let Some(detail) = snapshot.detail(&ClusterId::new(CONTEXT)) else {
            continue;
        };
        let hour = detail.overview.utilization(TimeRange::Hour);
        if hour.is_none_or(|hour| hour.cpu.is_empty()) {
            continue;
        }
        assert_eq!(snapshot.clusters.len(), 1);
        assert!(
            detail
                .overview
                .utilization(TimeRange::SixHours)
                .is_some_and(|range| !range.cpu.is_empty())
        );
        assert!(
            detail
                .overview
                .utilization_source
                .note()
                .is_some_and(|note| note.starts_with("Recorded by Flock while it's open, since "))
        );
        break;
    }
    assert!(
        commands
            .send_blocking(Command::Disconnect(ClusterId::new(CONTEXT)))
            .is_ok()
    );
    let deadline = std::time::Instant::now() + WAIT;
    let directory = flock_kube::metrics_history::cache_dir(home.path());
    let path = flock_kube::metrics_history::path(&directory, &ClusterId::new(CONTEXT));
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "history not persisted on disconnect"
        );
        let Ok(Update::Snapshot(snapshot)) = updates.recv_blocking() else {
            continue;
        };
        if snapshot
            .cluster(&ClusterId::new(CONTEXT))
            .is_some_and(|cluster| cluster.state == "Disconnected")
        {
            assert!(
                flock_kube::metrics_history::load(&path, jiff::Timestamp::now())
                    .latest()
                    .is_some()
            );
            break;
        }
    }
}

const EDIT_CONFIGMAP: &str = "flock-edit-e2e";

fn edit_target(namespace: &str) -> flock_core::ResourceRef {
    flock_core::ResourceRef {
        cluster: ClusterId::new(CONTEXT),
        kind: "ConfigMap".into(),
        namespace: Some(namespace.into()),
        name: EDIT_CONFIGMAP.into(),
    }
}

async fn edit(
    handle: &ClusterHandle,
    namespace: &str,
    step: flock_core::edit::EditStep,
) -> flock_core::edit::EditEvent {
    let (sink, events) = flock_core::Sink::channel();
    let request = flock_core::edit::EditRequest {
        target: edit_target(namespace),
        step,
        sink,
    };
    flock_kube::actions::edit::run(handle.clone(), request).await;
    let Ok(event) = events.recv().await else {
        panic!("edit sent no event");
    };
    event
}

async fn loaded(handle: &ClusterHandle, namespace: &str) -> flock_core::edit::Live {
    let event = edit(
        handle,
        namespace,
        flock_core::edit::EditStep::Load { reveal: false },
    )
    .await;
    let flock_core::edit::EditEvent::Loaded(live) = event else {
        panic!("{event:?}");
    };
    live
}

async fn remove_edit_configmap(configmaps: &Api<ConfigMap>) {
    let _ = configmaps
        .delete(EDIT_CONFIGMAP, &DeleteParams::default())
        .await;
}

async fn server_refuses_a_stale_apply(
    configmaps: &Api<ConfigMap>,
    namespace: &str,
    base: &flock_core::edit::Live,
) {
    let mut outdated = json!({
        "apiVersion": "v1",
        "kind": "ConfigMap",
        "metadata": { "name": EDIT_CONFIGMAP, "namespace": namespace, "resourceVersion": base.resource_version },
        "data": { "level": "stale" }
    });
    outdated["metadata"]["uid"] = json!(base.uid.as_str());
    let params = kube::api::PatchParams::apply("flock").force();
    let refused = configmaps
        .patch(EDIT_CONFIGMAP, &params, &kube::api::Patch::Apply(&outdated))
        .await;
    assert!(
        matches!(&refused, Err(kube::Error::Api(status)) if status.code == 409),
        "{refused:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn edit_yaml_reviews_applies_and_rebases_a_concurrent_edit() {
    use flock_core::edit::{Draft, EditEvent, EditStep};

    if !enabled() {
        return;
    }
    let Some(context) = kind_context() else {
        return;
    };
    let env = Arc::new(ShellEnv::from_process(String::new()));
    let runtime = tokio::runtime::Handle::current();
    let (manager, _events) = ClusterManager::new(runtime, KubeSettings::default(), env);
    let handle = connected(&manager, context).await;
    let Ok(client) = handle.client() else {
        panic!("client");
    };
    let scratch = ScratchNamespace::create(&[(CONTEXT, &client)]).await;
    let namespace = scratch.name();
    let configmaps: Api<ConfigMap> = Api::namespaced(client, namespace);
    remove_edit_configmap(&configmaps).await;
    let Ok(scratch) = serde_json::from_value::<ConfigMap>(json!({
        "metadata": { "name": EDIT_CONFIGMAP },
        "data": { "level": "info" }
    })) else {
        panic!("configmap fixture");
    };
    assert!(
        configmaps
            .create(&PostParams::default(), &scratch)
            .await
            .is_ok()
    );

    let base = loaded(&handle, namespace).await;
    assert!(!base.text.contains("managedFields"), "{}", base.text);
    let draft = Draft {
        text: base.text.replace("level: info", "level: debug"),
        base: base.clone(),
    };
    let reviewed = edit(&handle, namespace, EditStep::Review(draft.clone())).await;
    let EditEvent::Reviewed(lines) = reviewed else {
        panic!("{reviewed:?}");
    };
    assert!(lines.iter().any(|line| line.text.contains("level: debug")));
    let untouched = configmaps.get(EDIT_CONFIGMAP).await;
    let level = untouched
        .ok()
        .and_then(|cm| cm.data)
        .and_then(|data| data.get("level").cloned());
    assert_eq!(level.as_deref(), Some("info"));

    let applied = edit(&handle, namespace, EditStep::Apply(draft)).await;
    let EditEvent::Applied(after) = applied else {
        panic!("{applied:?}");
    };
    assert_ne!(after.resource_version, base.resource_version);
    let Ok(stored) = configmaps.get(EDIT_CONFIGMAP).await else {
        panic!("configmap");
    };
    let level = stored.data.as_ref().and_then(|data| data.get("level"));
    assert_eq!(level.map(String::as_str), Some("debug"));

    let stale = Draft {
        text: after.text.replace("level: debug", "level: warn"),
        base: after.clone(),
    };
    let concurrent = PatchBody::Merge(json!({ "data": { "owner": "someone-else" } }));
    let target = object_ref::<ConfigMap>(Some(namespace), EDIT_CONFIGMAP);
    assert!(handle.patch(&target, concurrent).await.is_ok());
    let conflicted = edit(&handle, namespace, EditStep::Apply(stale)).await;
    let EditEvent::Conflict(conflict) = conflicted else {
        panic!("{conflicted:?}");
    };
    assert!(
        conflict
            .changed
            .iter()
            .any(|line| line.text.contains("owner: someone-else")),
        "{:?}",
        conflict.changed
    );
    assert_eq!(conflict.rebased.contested, Vec::<String>::new());
    let rebased = Draft {
        text: conflict.rebased.text.clone(),
        base: conflict.live.clone(),
    };
    let reapplied = edit(&handle, namespace, EditStep::Apply(rebased)).await;
    assert!(matches!(reapplied, EditEvent::Applied(_)), "{reapplied:?}");
    let Ok(stored) = configmaps.get(EDIT_CONFIGMAP).await else {
        panic!("configmap");
    };
    let data = stored.data.unwrap_or_default();
    assert_eq!(data.get("level").map(String::as_str), Some("warn"));
    assert_eq!(data.get("owner").map(String::as_str), Some("someone-else"));

    server_refuses_a_stale_apply(&configmaps, namespace, &base).await;

    remove_edit_configmap(&configmaps).await;
}
