use std::sync::Arc;

use flock_core::crd::CrdKey;
use flock_core::edit::{Draft, EditEvent, EditRequest, EditStep};
use flock_core::settings::NamespaceSelection;
use flock_core::{
    Backend, ClusterId, Command, ContextRef, NewCluster, Page, Preferences, Receiver, ResourceRef,
    Sender, Snapshot, Update,
};
use flock_kube::LiveBackend;
use flock_kube::discovery::{DiscoverySources, KubeContext, discover};
use flock_kube::manager::{ClusterManager, ConnectionStatus};
use flock_kube::settings::KubeSettings;
use flock_kube::shell_env::ShellEnv;
use flock_kube::store::ClusterStore;

#[path = "common/kubeconfig.rs"]
mod kubeconfig;

use kubeconfig::kind_only_home;

const E2E_VAR: &str = "FLOCK_E2E";
const FLUX_CONTEXT: &str = "kind-fleet-a";
const LOCO_CONTEXT: &str = "kind-flock-loco";
const SUPPORT_DIR: &str = "support";
const HELM_GROUP: &str = "helm.toolkit.fluxcd.io";
const HELM_RELEASE: &str = "HelmRelease";
const GATEWAY_GROUP: &str = "gateway.networking.k8s.io";
const GATEWAY_CLASS: &str = "GatewayClass";
const READY_COLUMN: &str = "Ready";
const WAIT: std::time::Duration = std::time::Duration::from_secs(300);
const POLL: std::time::Duration = std::time::Duration::from_millis(500);

async fn eventually<T, F, Fut>(what: &str, observe: F, done: impl Fn(&T) -> bool) -> T
where
    F: Fn() -> Fut,
    Fut: Future<Output = T>,
{
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let observed = observe().await;
        if done(&observed) {
            return observed;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what} never happened"
        );
        tokio::time::sleep(POLL).await;
    }
}

fn enabled() -> bool {
    std::env::var(E2E_VAR).is_ok_and(|value| value == "1")
}

fn context(name: &str) -> Option<KubeContext> {
    let sources = DiscoverySources {
        kubeconfig_env: std::env::var_os("KUBECONFIG"),
        home: None,
        folders: Vec::new(),
    };
    discover(&sources)
        .contexts
        .into_iter()
        .find(|context| context.id.context == name)
}

struct Running {
    _home: tempfile::TempDir,
    commands: Sender<Command>,
    updates: Receiver<Update>,
}

fn start(context: &KubeContext) -> Running {
    let home = kind_only_home(std::slice::from_ref(context));
    let sources = DiscoverySources {
        kubeconfig_env: None,
        home: Some(home.path().to_path_buf()),
        folders: Vec::new(),
    };
    let mut store = ClusterStore::empty(&home.path().join(SUPPORT_DIR));
    let added = NewCluster {
        context: ContextRef {
            file: home.path().join(".kube").join("config"),
            context: context.id.context.clone(),
        },
        name: context.id.context.clone(),
    };
    assert!(store.add(&[added]).is_ok());
    let backend = LiveBackend::new(KubeSettings::default(), sources, store);
    let (updates_tx, updates) = async_channel::unbounded();
    let (commands, commands_rx) = async_channel::unbounded();
    Box::new(backend).start(updates_tx, commands_rx);
    Running {
        _home: home,
        commands,
        updates,
    }
}

impl Running {
    fn send(&self, command: Command) {
        assert!(self.commands.send_blocking(command).is_ok());
    }

    fn until(&self, what: &str, done: impl Fn(&Snapshot) -> bool) -> Snapshot {
        let deadline = std::time::Instant::now() + WAIT;
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "{what} never happened"
            );
            let Ok(Update::Snapshot(snapshot)) = self.updates.recv_blocking() else {
                continue;
            };
            if done(&snapshot) {
                return *snapshot;
            }
        }
    }
}

fn all_namespaces(cluster: &ClusterId, key: &CrdKey) -> Command {
    let mut preferences = Preferences::default();
    preferences.select_namespace(&key.qualified(), NamespaceSelection::All);
    Command::SetPreferences(cluster.clone(), preferences)
}

#[test]
fn live_helm_releases_list_inspect_and_stop_watching_when_the_page_closes() {
    if !enabled() {
        return;
    }
    let Some(context) = context(FLUX_CONTEXT) else {
        return;
    };
    let running = start(&context);
    let cluster = ClusterId::new(FLUX_CONTEXT);
    let key = CrdKey::new(HELM_GROUP, HELM_RELEASE);
    running.send(all_namespaces(&cluster, &key));
    running.send(Command::SelectCluster(Some(cluster.clone())));
    let listed = running.until("the CRD list", |snapshot| {
        snapshot.crd(&cluster, &key).is_some()
    });
    let Some(crd) = listed.crd(&cluster, &key).cloned() else {
        panic!("no HelmRelease CRD");
    };
    assert!(crd.namespaced);
    assert_eq!(crd.version, "v2");
    let ready = crd.columns.iter().position(|c| c.name == READY_COLUMN);
    assert!(ready.is_some(), "{:?}", crd.columns);
    running.send(Command::OpenCrd {
        cluster: cluster.clone(),
        crd: key.clone(),
    });
    let rows = running.until("HelmRelease rows", |snapshot| {
        snapshot
            .custom_objects(&cluster, &key)
            .is_some_and(|objects| !objects.rows.is_empty())
    });
    let states = rows.page_state(&cluster, Page::CustomResources).cloned();
    assert!(
        states
            .as_ref()
            .is_some_and(flock_core::WatchState::is_loaded),
        "{states:?}"
    );
    let Some(first) = rows
        .custom_objects(&cluster, &key)
        .and_then(|objects| objects.rows.first().cloned())
    else {
        panic!("no rows");
    };
    let filled = ready.and_then(|ix| first.cells.get(ix)).cloned();
    assert!(
        filled
            .as_deref()
            .is_some_and(|cell| ["True", "False", "Unknown"].contains(&cell)),
        "{filled:?}"
    );
    let target = key.object(
        cluster.clone(),
        Some(first.namespace.clone()),
        first.name.clone(),
    );
    running.send(Command::InspectResource(target.clone()));
    let inspected = running.until("YAML and Related for the inspected release", |snapshot| {
        let yaml = snapshot
            .custom_objects(&cluster, &key)
            .and_then(|objects| objects.rows.iter().find(|row| row.name == first.name))
            .is_some_and(|row| row.yaml.contains("kind: HelmRelease"));
        let related = snapshot
            .detail(&cluster)
            .is_some_and(|detail| detail.related(&target).is_some());
        yaml && related
    });
    assert!(inspected.custom_objects(&cluster, &key).is_some());
    running.send(Command::SelectPage(Page::Pods));
    running.until("the object watch to stop", |snapshot| {
        snapshot.custom.is_empty()
    });
}

#[test]
fn live_cluster_scoped_definitions_list_without_namespaces() {
    if !enabled() {
        return;
    }
    let Some(context) = context(LOCO_CONTEXT) else {
        return;
    };
    let running = start(&context);
    let cluster = ClusterId::new(LOCO_CONTEXT);
    let key = CrdKey::new(GATEWAY_GROUP, GATEWAY_CLASS);
    running.send(Command::OpenCrd {
        cluster: cluster.clone(),
        crd: key.clone(),
    });
    let rows = running.until("GatewayClass rows", |snapshot| {
        snapshot
            .custom_objects(&cluster, &key)
            .is_some_and(|objects| !objects.rows.is_empty())
    });
    assert!(rows.crd(&cluster, &key).is_some_and(|crd| !crd.namespaced));
    let objects = rows.custom_objects(&cluster, &key).map(|o| o.rows.clone());
    assert!(
        objects
            .unwrap_or_default()
            .iter()
            .all(|row| row.namespace.is_empty())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn live_edit_yaml_loads_and_dry_runs_a_custom_object() {
    if !enabled() {
        return;
    }
    let Some(context) = context(FLUX_CONTEXT) else {
        return;
    };
    let env = Arc::new(ShellEnv::from_process(String::new()));
    let runtime = tokio::runtime::Handle::current();
    let (manager, _events) = ClusterManager::new(runtime, KubeSettings::default(), env);
    let handle = manager.connect(context);
    let _definitions =
        handle.subscribe(flock_kube::resources::WatchKind::CustomResourceDefinitions);
    let key = CrdKey::new(HELM_GROUP, HELM_RELEASE);
    eventually(
        "the HelmRelease definition",
        || async { flock_kube::crd::find(&handle, &key) },
        Option::is_some,
    )
    .await;
    assert_eq!(handle.status(), ConnectionStatus::Connected);
    let Some(crd) = flock_kube::crd::find(&handle, &key) else {
        panic!("definition");
    };
    let _objects = handle.subscribe_resource(
        flock_kube::crd::custom_kind(&crd),
        flock_kube::crd::api_resource(&crd),
    );
    let rows = eventually(
        "HelmRelease objects",
        || async { flock_kube::crd::rows(&handle, &crd, jiff::Timestamp::now()) },
        |rows| !rows.is_empty(),
    )
    .await;
    let Some(first) = rows.first() else {
        panic!("rows");
    };
    let target: ResourceRef = key.object(
        ClusterId::new(FLUX_CONTEXT),
        Some(first.namespace.clone()),
        first.name.clone(),
    );
    let send = |step: EditStep| {
        let (sink, events) = flock_core::Sink::channel();
        let request = EditRequest {
            target: target.clone(),
            step,
            sink,
        };
        let handle = handle.clone();
        async move {
            flock_kube::actions::edit::run(handle, request).await;
            events.recv().await.ok()
        }
    };
    let Some(EditEvent::Loaded(live)) = send(EditStep::Load { reveal: false }).await else {
        panic!("custom object did not load for editing");
    };
    assert!(live.text.contains("kind: HelmRelease"), "{}", live.text);
    let draft = Draft {
        text: live.text.clone(),
        base: live,
    };
    let reviewed = send(EditStep::Review(draft)).await;
    assert!(
        matches!(reviewed, Some(EditEvent::Reviewed(_))),
        "{reviewed:?}"
    );
}

#[test]
fn live_helm_releases_come_from_storage_with_history_values_and_flux_owners() {
    if !enabled() {
        return;
    }
    let Some(context) = context(FLUX_CONTEXT) else {
        return;
    };
    let running = start(&context);
    let cluster = ClusterId::new(FLUX_CONTEXT);
    running.send(Command::SelectCluster(Some(cluster.clone())));
    running.send(Command::SelectPage(Page::Releases));
    let listed = running.until("Helm releases owned by Flux", |snapshot| {
        snapshot
            .helm_releases(&cluster)
            .iter()
            .any(|release| release.flux.is_some() && release.revision > 1)
    });
    let Some(release) = listed
        .helm_releases(&cluster)
        .into_iter()
        .filter(|release| release.flux.is_some())
        .max_by_key(|release| release.revision)
        .cloned()
    else {
        panic!("no release");
    };
    assert_ne!(release.chart, "");
    assert_ne!(release.chart_version, "");
    assert!(release.updated.is_some());
    running.send(Command::OpenHelmRelease(release.key.clone()));
    let opened = running.until("the release history", |snapshot| {
        snapshot
            .helm_detail(&release.key)
            .is_some_and(|detail| detail.history.len() > 1)
    });
    let Some(detail) = opened.helm_detail(&release.key) else {
        panic!("detail");
    };
    let shown = opened
        .helm_releases(&cluster)
        .into_iter()
        .find(|shown| shown.key == release.key)
        .map(|shown| shown.revision);
    assert_eq!(detail.history.first().map(|r| r.revision), shown);
    assert!(detail.manifest.contains("kind:"), "{}", detail.manifest);
    assert_ne!(detail.computed_values, "");
    assert!(
        listed
            .page_state(&cluster, Page::Releases)
            .is_some_and(flock_core::WatchState::is_loaded)
    );
    running.send(Command::SelectPage(Page::Pods));
    running.until("the releases kept after leaving the page", |snapshot| {
        snapshot.helm_details.is_empty() && !snapshot.helm_releases(&cluster).is_empty()
    });
}

#[test]
fn live_releases_flux_does_not_manage_are_listed_and_open() {
    if !enabled() {
        return;
    }
    let Some(context) = context(LOCO_CONTEXT) else {
        return;
    };
    let running = start(&context);
    let cluster = ClusterId::new(LOCO_CONTEXT);
    running.send(Command::SelectCluster(Some(cluster.clone())));
    running.send(Command::SelectPage(Page::Releases));
    let listed = running.until("Flux and non-Flux releases", |snapshot| {
        let releases = snapshot.helm_releases(&cluster);
        releases.iter().any(|release| release.flux.is_none())
            && releases.iter().any(|release| release.flux.is_some())
    });
    let Some(release) = listed
        .helm_releases(&cluster)
        .into_iter()
        .find(|release| release.flux.is_none())
        .cloned()
    else {
        panic!("no release outside Flux");
    };
    assert_ne!(release.chart, "");
    running.send(Command::OpenHelmRelease(release.key.clone()));
    let opened = running.until("the non-Flux release page", |snapshot| {
        snapshot.helm_detail(&release.key).is_some()
    });
    let Some(detail) = opened.helm_detail(&release.key) else {
        panic!("detail");
    };
    let shown = opened
        .helm_releases(&cluster)
        .into_iter()
        .find(|shown| shown.key == release.key)
        .map(|shown| shown.revision);
    assert_eq!(detail.history.first().map(|r| r.revision), shown);
    assert!(detail.manifest.contains("kind:"), "{}", detail.manifest);
}
