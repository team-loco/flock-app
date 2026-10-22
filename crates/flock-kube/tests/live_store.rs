use std::time::{Duration, Instant};

use flock_core::{Backend, ClusterId, Command, NewCluster, Update};
use flock_kube::LiveBackend;
use flock_kube::discovery::DiscoverySources;
use flock_kube::settings::KubeSettings;
use flock_kube::store::ClusterStore;

const CONTEXT: &str = "unreachable-test";
const NAME: &str = "local";
const WAIT: Duration = Duration::from_secs(60);
const SUPPORT_DIR: &str = "support";
const KUBECONFIG: &str = "apiVersion: v1
kind: Config
current-context: unreachable-test
clusters:
- name: unreachable-test
  cluster:
    server: https://127.0.0.1:1
contexts:
- name: unreachable-test
  context:
    cluster: unreachable-test
    user: unreachable-test
users:
- name: unreachable-test
  user:
    token: abc
";

fn next(updates: &async_channel::Receiver<Update>, deadline: Instant) -> Update {
    assert!(Instant::now() < deadline, "backend went quiet");
    let Ok(update) = updates.recv_blocking() else {
        panic!("backend stopped");
    };
    update
}

#[test]
fn connects_only_to_added_clusters_and_remembers_them() {
    let Ok(home) = tempfile::tempdir() else {
        panic!("tempdir");
    };
    let kube = home.path().join(".kube");
    assert!(std::fs::create_dir_all(&kube).is_ok());
    assert!(std::fs::write(kube.join("config"), KUBECONFIG).is_ok());
    let sources = DiscoverySources {
        kubeconfig_env: None,
        home: Some(home.path().to_path_buf()),
        folders: Vec::new(),
    };
    let support = home.path().join(SUPPORT_DIR);
    let store = ClusterStore::empty(&support);
    let settings = KubeSettings {
        shell_env_timeout: Duration::from_millis(1),
        ..KubeSettings::default()
    };
    let backend = LiveBackend::new(settings, sources, store);
    let (updates_tx, updates) = async_channel::unbounded();
    let (commands, commands_rx) = async_channel::unbounded();
    Box::new(backend).start(updates_tx, commands_rx);
    let deadline = Instant::now() + WAIT;

    let Update::Snapshot(first) = next(&updates, deadline) else {
        panic!("expected the first snapshot");
    };
    assert_eq!(first.clusters.len(), 0);

    let open = Command::OpenAddCluster(flock_core::AddClusterStep::Pick);
    assert!(commands.send_blocking(open).is_ok());
    let context = loop {
        if let Update::Contexts(files) = next(&updates, deadline) {
            let ids: Vec<_> = files.iter().flat_map(|f| &f.contexts).collect();
            assert_eq!(ids.len(), 1);
            let Some(choice) = ids.first() else {
                panic!("one context");
            };
            assert_eq!(choice.id.context, CONTEXT);
            assert!(!choice.selected);
            break choice.id.clone();
        }
    };

    let add = Command::AddClusters(vec![NewCluster {
        context,
        name: NAME.to_owned(),
    }]);
    assert!(commands.send_blocking(add).is_ok());
    loop {
        if let Update::Snapshot(snapshot) = next(&updates, deadline)
            && snapshot.cluster(&ClusterId::new(NAME)).is_some()
        {
            break;
        }
    }
    let Ok(saved) = ClusterStore::open(&support) else {
        panic!("store readable");
    };
    assert_eq!(saved.clusters().len(), 1);

    let remove = Command::RemoveCluster(ClusterId::new(NAME));
    assert!(commands.send_blocking(remove).is_ok());
    loop {
        if let Update::Snapshot(snapshot) = next(&updates, deadline)
            && snapshot.clusters.is_empty()
        {
            break;
        }
    }
    let Ok(saved) = ClusterStore::open(&support) else {
        panic!("store readable");
    };
    assert_eq!(saved.clusters().len(), 0);
}

const PASTED_NAME: &str = "lab";

fn start(
    home: &std::path::Path,
) -> (
    async_channel::Sender<Command>,
    async_channel::Receiver<Update>,
) {
    let sources = DiscoverySources {
        kubeconfig_env: None,
        home: Some(home.to_path_buf()),
        folders: Vec::new(),
    };
    let store = ClusterStore::empty(&home.join(SUPPORT_DIR));
    let settings = KubeSettings {
        shell_env_timeout: Duration::from_millis(1),
        ..KubeSettings::default()
    };
    let backend = LiveBackend::new(settings, sources, store);
    let (updates_tx, updates) = async_channel::unbounded();
    let (commands, commands_rx) = async_channel::unbounded();
    Box::new(backend).start(updates_tx, commands_rx);
    (commands, updates)
}

fn contexts_named(
    updates: &async_channel::Receiver<Update>,
    deadline: Instant,
    context: &str,
) -> Update {
    loop {
        let update = next(updates, deadline);
        let found = match &update {
            Update::Contexts(files) => files
                .iter()
                .flat_map(|file| &file.contexts)
                .any(|choice| choice.id.context == context),
            Update::KubeconfigRejected(_) => true,
            _ => false,
        };
        if found {
            return update;
        }
    }
}

#[test]
fn pasted_kubeconfig_is_saved_and_discovered() {
    let Ok(home) = tempfile::tempdir() else {
        panic!("tempdir");
    };
    let (commands, updates) = start(home.path());
    let deadline = Instant::now() + WAIT;

    let bad = Command::SaveKubeconfig {
        name: PASTED_NAME.to_owned(),
        yaml: "foo: bar".to_owned(),
    };
    assert!(commands.send_blocking(bad).is_ok());
    assert!(matches!(
        contexts_named(&updates, deadline, CONTEXT),
        Update::KubeconfigRejected(_)
    ));

    let good = Command::SaveKubeconfig {
        name: PASTED_NAME.to_owned(),
        yaml: KUBECONFIG.to_owned(),
    };
    assert!(commands.send_blocking(good).is_ok());
    let Update::Contexts(files) = contexts_named(&updates, deadline, CONTEXT) else {
        panic!("expected the discovered context");
    };
    let saved = home
        .path()
        .join(SUPPORT_DIR)
        .join("kubeconfigs")
        .join("lab.yaml");
    let files_ids: Vec<_> = files
        .iter()
        .flat_map(|f| &f.contexts)
        .map(|c| &c.id.file)
        .collect();
    assert_eq!(files_ids, vec![&saved]);
}
