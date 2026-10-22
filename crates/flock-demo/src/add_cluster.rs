use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use flock_core::{
    CheckLine, CheckStatus, ClusterId, ClusterSummary, Command, ContextChoice, ContextRef, Counts,
    KubeconfigFile, NewCluster, Page, Sender, Snapshot, Status, Update, Verdict,
};

const CHECK_STEP: Duration = Duration::from_millis(160);
const DEFAULT_FILE: &str = "~/.kube/config";
const SANDBOX_FILE: &str = "~/.kube/eks/sandbox.yaml";
const ANALYTICS: &str = "gke_acme-data_us-central1_analytics";
const MINIKUBE: &str = "minikube";
const SANDBOX: &str = "arn:aws:eks:us-west-2:[ACCOUNT]:cluster/sandbox";
const ANALYTICS_NODES: u32 = 9;
const ANALYTICS_PODS: u32 = 212;

struct Seed {
    file: &'static str,
    context: &'static str,
    server: &'static str,
    auth: &'static str,
    selected: bool,
}

const SEEDS: [Seed; 3] = [
    Seed {
        file: DEFAULT_FILE,
        context: ANALYTICS,
        server: "https://34.118.[IP]",
        auth: "gke-gcloud-auth-plugin",
        selected: true,
    },
    Seed {
        file: DEFAULT_FILE,
        context: MINIKUBE,
        server: "https://127.0.0.1:52011",
        auth: "client certificate",
        selected: false,
    },
    Seed {
        file: SANDBOX_FILE,
        context: SANDBOX,
        server: "https://[ID].eks.amazonaws.com",
        auth: "aws eks get-token",
        selected: true,
    },
];

#[derive(Default)]
pub struct AddClusters {
    added: BTreeMap<ClusterId, ContextRef>,
    checked: BTreeSet<ContextRef>,
}

impl AddClusters {
    pub fn handle(
        &mut self,
        command: &Command,
        flock: &mut Snapshot,
        updates: &Sender<Update>,
    ) -> bool {
        match command {
            Command::OpenAddCluster(_) => {
                let files = self.files();
                send(updates, Update::Contexts(files));
            }
            Command::CheckContexts(ids) => {
                let runs = ids
                    .iter()
                    .map(|id| (id.clone(), !self.checked.insert(id.clone())))
                    .collect();
                simulate(updates, runs);
            }
            Command::AddClusters(new) => {
                for cluster in new {
                    self.added
                        .insert(ClusterId::new(&cluster.name), cluster.context.clone());
                    flock.clusters.push(summary(cluster));
                }
                send(updates, Update::Snapshot(Box::new(flock.clone())));
            }
            Command::RemoveCluster(id) => {
                self.added.remove(id);
                flock.remove_cluster(id);
                send(updates, Update::Snapshot(Box::new(flock.clone())));
            }
            _ => return false,
        }
        true
    }

    pub fn files(&self) -> Vec<KubeconfigFile> {
        let mut files: Vec<KubeconfigFile> = Vec::new();
        let available = SEEDS
            .iter()
            .filter(|seed| !self.added.values().any(|id| id == &context_ref(seed)));
        for seed in available {
            let choice = ContextChoice {
                id: context_ref(seed),
                server: Some(seed.server.to_owned()),
                auth: seed.auth.to_owned(),
                selected: seed.selected,
            };
            match files.iter_mut().find(|file| file.label == seed.file) {
                Some(file) => file.contexts.push(choice),
                None => files.push(KubeconfigFile {
                    label: seed.file.to_owned(),
                    contexts: vec![choice],
                }),
            }
        }
        files
    }
}

fn send(updates: &Sender<Update>, update: Update) {
    if updates.send_blocking(update).is_err() {
        tracing::debug!("demo update dropped: UI closed");
    }
}

fn context_ref(seed: &Seed) -> ContextRef {
    ContextRef {
        file: PathBuf::from(seed.file),
        context: seed.context.to_owned(),
    }
}

fn line(status: CheckStatus, detail: &str, millis: u64) -> CheckLine {
    CheckLine {
        status,
        detail: detail.to_owned(),
        raw: None,
        remedy: None,
        duration: Duration::from_millis(millis),
    }
}

fn raw_line(status: CheckStatus, raw: &str, detail: &str, millis: u64) -> CheckLine {
    CheckLine {
        raw: Some(raw.to_owned()),
        ..line(status, detail, millis)
    }
}

pub fn script(context: &ContextRef, retried: bool) -> (Vec<CheckLine>, Verdict) {
    match context.context.as_str() {
        ANALYTICS => (
            vec![
                line(CheckStatus::Ok, "Reachable in 142 ms", 142),
                line(
                    CheckStatus::Ok,
                    "Kubernetes v1.30.4 · 9 nodes · 212 pods",
                    310,
                ),
                line(
                    CheckStatus::Ok,
                    "Flux v2.4.0 · 14 Kustomizations, 6 HelmReleases",
                    420,
                ),
                line(
                    CheckStatus::Ok,
                    "Metrics from monitoring/prometheus-k8s",
                    260,
                ),
                raw_line(
                    CheckStatus::Warn,
                    "secrets is forbidden: cannot list resource \"secrets\"",
                    "env values won’t resolve",
                    180,
                ),
            ],
            Verdict::Ready,
        ),
        SANDBOX if retried => (
            vec![
                line(CheckStatus::Ok, "Reachable in 88 ms", 88),
                line(
                    CheckStatus::Ok,
                    "Kubernetes v1.29.8 · 4 nodes · 61 pods",
                    240,
                ),
            ],
            Verdict::Ready,
        ),
        SANDBOX => {
            let mut expired =
                raw_line(CheckStatus::Err, "aws eks get-token: ExpiredToken", "", 640);
            expired.remedy = Some("aws sso login --profile sandbox".to_owned());
            (
                vec![line(CheckStatus::Ok, "Reachable in 88 ms", 88), expired],
                Verdict::Unauthorized,
            )
        }
        _ => {
            let mut refused = raw_line(
                CheckStatus::Err,
                "tcp connect error: Connection refused (os error 61)",
                "",
                3,
            );
            refused.remedy = Some("minikube start".to_owned());
            (vec![refused], Verdict::CannotReach)
        }
    }
}

fn simulate(updates: &Sender<Update>, runs: Vec<(ContextRef, bool)>) {
    for (id, retried) in runs {
        let updates = updates.clone();
        let spawned = thread::Builder::new()
            .name("flock-demo-checks".into())
            .spawn(move || {
                let (lines, verdict) = script(&id, retried);
                for line in lines {
                    thread::sleep(CHECK_STEP);
                    send(&updates, Update::Check(id.clone(), line));
                }
                send(&updates, Update::Checked(id, verdict));
            });
        if let Err(error) = spawned {
            tracing::error!(%error, "demo checks failed to start");
        }
    }
}

fn summary(cluster: &NewCluster) -> ClusterSummary {
    let analytics = cluster.context.context == ANALYTICS;
    let nav = BTreeMap::from([
        (Page::Nodes, Counts::total(ANALYTICS_NODES)),
        (Page::Pods, Counts::total(ANALYTICS_PODS)),
    ]);
    ClusterSummary {
        id: ClusterId::new(&cluster.name),
        name: cluster.name.clone(),
        status: Status::Ok,
        state: "Healthy".into(),
        distro: if analytics { "GKE" } else { "Kubernetes" }.into(),
        region: if analytics { "us-central1" } else { "" }.into(),
        version: analytics.then(|| "v1.30.4".into()),
        nodes: analytics.then_some(ANALYTICS_NODES),
        problems: Counts::default(),
        nav: if analytics { nav } else { BTreeMap::new() },
        metrics: analytics.then(|| crate::home::added_metrics(ANALYTICS_PODS)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analytics() -> NewCluster {
        NewCluster {
            context: ContextRef {
                file: PathBuf::from(DEFAULT_FILE),
                context: ANALYTICS.to_owned(),
            },
            name: "analytics".to_owned(),
        }
    }

    #[test]
    fn picker_groups_design_contexts_by_file() {
        let files = AddClusters::default().files();
        let labels: Vec<&str> = files.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(labels, [DEFAULT_FILE, SANDBOX_FILE]);
        let selected: Vec<bool> = files
            .iter()
            .flat_map(|f| &f.contexts)
            .map(|c| c.selected)
            .collect();
        assert_eq!(selected, [true, false, true]);
        let meta = files
            .first()
            .and_then(|f| f.contexts.first())
            .map(ContextChoice::meta);
        assert_eq!(
            meta.as_deref(),
            Some("https://34.118.[IP] · gke-gcloud-auth-plugin")
        );
    }

    #[test]
    fn scripts_match_the_design_results() {
        let (lines, verdict) = script(&analytics().context, false);
        assert_eq!(verdict, Verdict::Ready);
        assert_eq!(lines.len(), 5);
        assert_eq!(lines.last().map(|l| l.status), Some(CheckStatus::Warn));
        let sandbox = ContextRef {
            file: PathBuf::from(SANDBOX_FILE),
            context: SANDBOX.to_owned(),
        };
        let (lines, verdict) = script(&sandbox, false);
        assert_eq!(verdict, Verdict::Unauthorized);
        let remedy = lines.last().and_then(|l| l.remedy.clone());
        assert_eq!(remedy.as_deref(), Some("aws sso login --profile sandbox"));
        let (_, verdict) = script(&sandbox, true);
        assert_eq!(verdict, Verdict::Ready);
    }

    #[test]
    fn checks_stream_one_line_at_a_time_then_finish() {
        let (tx, rx) = async_channel::unbounded();
        let id = analytics().context;
        simulate(&tx, vec![(id.clone(), false)]);
        let received: Vec<Update> = (0..6).filter_map(|_| rx.recv_blocking().ok()).collect();
        assert_eq!(received.len(), 6);
        assert!(matches!(received.first(), Some(Update::Check(ctx, _)) if ctx == &id));
        assert_eq!(received.last(), Some(&Update::Checked(id, Verdict::Ready)));
    }

    #[test]
    fn added_clusters_leave_the_picker_until_removed() {
        let (tx, rx) = async_channel::unbounded();
        let mut adding = AddClusters::default();
        let mut flock = Snapshot::default();
        let add = Command::AddClusters(vec![analytics()]);
        assert!(adding.handle(&add, &mut flock, &tx));
        assert_eq!(flock.clusters.len(), 1);
        let pods = flock
            .clusters
            .first()
            .and_then(|c| c.metrics.as_ref())
            .and_then(|m| m.pods.total);
        assert_eq!(pods, Some(ANALYTICS_PODS));
        assert!(matches!(rx.recv_blocking(), Ok(Update::Snapshot(s)) if s.clusters.len() == 1));
        let remaining: usize = adding.files().iter().map(|f| f.contexts.len()).sum();
        assert_eq!(remaining, 2);
        let remove = Command::RemoveCluster(ClusterId::new("analytics"));
        assert!(adding.handle(&remove, &mut flock, &tx));
        assert_eq!(flock.clusters.len(), 0);
        let remaining: usize = adding.files().iter().map(|f| f.contexts.len()).sum();
        assert_eq!(remaining, 3);
        assert!(!adding.handle(&Command::ToggleSidebar, &mut flock, &tx));
    }
}
