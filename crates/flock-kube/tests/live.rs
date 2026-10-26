use std::sync::Arc;
use std::time::Duration;

use flock_core::{Backend, NewCluster, Snapshot, Update};
use flock_kube::LiveBackend;
use flock_kube::discovery::{DiscoverySources, discover};
use flock_kube::settings::KubeSettings;
use flock_kube::store::ClusterStore;

#[path = "common/kubeconfig.rs"]
mod kubeconfig;

const E2E_VAR: &str = "FLOCK_E2E";
const CLUSTERS: [(&str, &str); 2] = [("fleet-a", "kind-fleet-a"), ("fleet-b", "kind-fleet-b")];
const SHARED_PROBLEMS: [&str; 3] = ["unschedulable", "broken-kustomization", "stripe-webhook"];
const FLOCK_A_PROBLEMS: [&str; 1] = ["checkout-api"];
const WAIT: Duration = Duration::from_secs(180);
const DATA_DIR: &str = "data";

fn missing_problems(snapshot: &Snapshot) -> Vec<String> {
    let mut missing = Vec::new();
    for (name, _) in CLUSTERS {
        let expected = SHARED_PROBLEMS.iter().chain(if name == "fleet-a" {
            FLOCK_A_PROBLEMS.iter()
        } else {
            [].iter()
        });
        for problem in expected {
            let found = snapshot.home.problems.iter().any(|entry| {
                entry.target.cluster.as_str() == name && entry.target.name.contains(problem)
            });
            if !found {
                missing.push(format!("{name}/{problem}"));
            }
        }
    }
    missing
}

#[tokio::test]
async fn live_backend_connects_both_kind_clusters_and_shows_the_scenarios() -> Result<(), String> {
    if std::env::var(E2E_VAR).as_deref() != Ok("1") {
        return Ok(());
    }
    let sources = DiscoverySources {
        kubeconfig_env: std::env::var_os("KUBECONFIG"),
        home: None,
        folders: Vec::new(),
    };
    let discovery = discover(&sources);
    let contexts = CLUSTERS
        .iter()
        .map(|(_, name)| {
            discovery
                .contexts
                .iter()
                .find(|context| context.id.context == *name)
                .cloned()
                .ok_or_else(|| format!("KUBECONFIG must define {name}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let home = kubeconfig::kind_only_home(&contexts);
    let file = home.path().join(".kube").join("config");
    let mut store = ClusterStore::empty(&home.path().join(DATA_DIR));
    let added = CLUSTERS
        .iter()
        .map(|(name, context)| NewCluster {
            context: flock_core::ContextRef {
                file: file.clone(),
                context: (*context).to_owned(),
            },
            name: (*name).to_owned(),
        })
        .collect::<Vec<_>>();
    store.add(&added).map_err(|error| error.to_string())?;
    let sources = DiscoverySources {
        kubeconfig_env: None,
        home: Some(home.path().to_path_buf()),
        folders: Vec::new(),
    };
    let backend = LiveBackend::new(KubeSettings::default(), sources, store);
    let (updates_tx, updates) = async_channel::unbounded();
    let (_commands, commands_rx) = async_channel::unbounded();
    Box::new(backend).start(updates_tx, commands_rx);
    let deadline = tokio::time::Instant::now() + WAIT;
    let mut last = Snapshot::default();
    loop {
        let received = tokio::time::timeout_at(deadline, updates.recv()).await;
        let update = match received {
            Ok(update) => update.map_err(|error| {
                format!("backend stopped before the scenarios appeared: {error}")
            })?,
            Err(_) => {
                return Err(format!(
                    "missing {:?}, last snapshot {last:#?}",
                    missing_problems(&last)
                ));
            }
        };
        if let Update::Snapshot(snapshot) = update {
            let connected = snapshot
                .clusters
                .iter()
                .filter(|c| c.is_connected())
                .count();
            if connected == CLUSTERS.len() && missing_problems(&snapshot).is_empty() {
                return Ok(());
            }
            last = Arc::unwrap_or_clone(snapshot);
        }
    }
}
