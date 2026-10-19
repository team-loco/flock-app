use std::time::{Duration, Instant};

use flock::Screen;
use flock_core::{
    Backend, ClusterId, Command, ContextRef, NewCluster, Page, Snapshot, Update, WatchState,
};
use flock_kube::LiveBackend;
use flock_kube::discovery::{DiscoverySources, discover};
use flock_kube::settings::KubeSettings;
use flock_kube::store::ClusterStore;
use flock_shots::Ui;

#[path = "../../../crates/flock-kube/tests/common/kubeconfig.rs"]
mod kubeconfig;

const E2E_VAR: &str = "FLOCK_E2E";
const CLUSTER: &str = "kind-fleet-a";
const CLUSTERS: [&str; 3] = ["kind-fleet-a", "kind-fleet-b", "kind-flock-loco"];
const WINDOW: u64 = 7;
const WAIT: Duration = Duration::from_secs(120);
const DATA_DIR: &str = "data";

fn start() -> (
    tempfile::TempDir,
    async_channel::Receiver<Update>,
    async_channel::Sender<Command>,
) {
    let sources = DiscoverySources {
        kubeconfig_env: std::env::var_os("KUBECONFIG"),
        home: None,
        folders: Vec::new(),
    };
    let discovery = discover(&sources);
    let contexts: Vec<_> = CLUSTERS
        .iter()
        .map(|name| {
            let Some(context) = discovery.contexts.iter().find(|c| c.id.context == *name) else {
                panic!("{E2E_VAR}=1 needs KUBECONFIG with {name}");
            };
            context.clone()
        })
        .collect();
    let home = kubeconfig::kind_only_home(&contexts);
    let file = home.path().join(".kube").join("config");
    let mut store = ClusterStore::empty(&home.path().join(DATA_DIR));
    let added: Vec<NewCluster> = CLUSTERS
        .iter()
        .map(|name| NewCluster {
            context: ContextRef {
                file: file.clone(),
                context: (*name).to_owned(),
            },
            name: (*name).to_owned(),
        })
        .collect();
    assert!(store.add(&added).is_ok());
    let sources = DiscoverySources {
        kubeconfig_env: None,
        home: Some(home.path().to_path_buf()),
        folders: Vec::new(),
    };
    let backend = LiveBackend::new(KubeSettings::default(), sources, store);
    let (updates_tx, updates) = async_channel::unbounded();
    let (commands, commands_rx) = async_channel::unbounded();
    Box::new(backend).start(updates_tx, commands_rx);
    (home, updates, commands)
}

fn loaded(snapshot: &Snapshot, id: &ClusterId) -> bool {
    let synced = snapshot.page_state(id, Page::Deployments) == Some(&WatchState::Synced);
    synced
        && snapshot
            .page_rows(id, Page::Deployments)
            .is_some_and(|rows| rows > 0)
}

#[test]
fn live_deployments_in_an_empty_default_namespace_keep_the_filter_bar() -> anyhow::Result<()> {
    if std::env::var(E2E_VAR).as_deref() != Ok("1") {
        return Ok(());
    }
    let (_home, updates, commands) = start();
    let id = ClusterId::new(CLUSTER);
    let steps = [
        Command::SelectCluster(Some(id.clone())),
        Command::SelectPage(Page::Deployments),
    ];
    for step in steps {
        let command = Command::ForWindow(WINDOW, Box::new(step));
        commands.send_blocking(command)?;
    }
    let deadline = Instant::now() + WAIT;
    let snapshot = loop {
        anyhow::ensure!(
            Instant::now() < deadline,
            "{CLUSTER} Deployments never loaded"
        );
        if let Ok(Update::Snapshot(snapshot)) = updates.recv_blocking()
            && loaded(&snapshot, &id)
        {
            break snapshot;
        }
    };
    let in_default = snapshot
        .detail(&id)
        .map(|detail| {
            detail
                .resources(flock_core::ResourceKind::Deployment)
                .iter()
                .filter(|row| row.namespace == "default")
                .count()
        })
        .unwrap_or_default();
    anyhow::ensure!(in_default == 0, "the scenario has Deployments in default");
    let (mut ui, feed) = Ui::scripted(Screen::Flock)?;
    feed.try_send(Update::Snapshot(snapshot))?;
    ui.settle()?;
    ui.press("cmd-shift-c")?;
    ui.type_text(CLUSTER)?;
    ui.press("enter")?;
    ui.press("cmd-k")?;
    ui.type_text("deployments")?;
    ui.press("enter")?;
    assert_eq!(ui.state().location().page, Page::Deployments);
    assert!(ui.bounds("resource-filter").is_some());
    assert!(ui.bounds("resource-namespace").is_some());
    assert!(ui.bounds("empty-state-namespace").is_some());
    assert!(ui.bounds("empty-show-all-namespaces").is_some());
    Ok(())
}
