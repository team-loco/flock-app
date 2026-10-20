mod changes;
pub(crate) mod convert;
mod drift;
pub(crate) mod focus;
mod git;
mod grid;
mod health;
mod requests;
mod watch;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use flock_core::{
    ClusterId, Command, Counts, FluxKey, FluxKind, Page, ResourceRef, Snapshot, Status,
};
use flock_flux::ObjectKind;
use jiff::Timestamp;

use crate::flux::{FluxHealth, WATCHED};
use crate::manager::{ClusterHandle, ConnectionStatus, lock};
use changes::{Consumer, Repo, changes_log, repo_key, wanted_revision};
use convert::{Inputs, Parsed, core_status, flux_object, parse, summary};
use focus::Focus;
use git::{Mirrors, RepoKey};
use grid::ClusterFluxView;
use health::Workloads;
use requests::Requests;
use watch::{ClusterFlux, Failures, FluxInfo, KINDS};

const CHANGE_BUFFER: usize = 1;

pub struct GitOpsState {
    clusters: BTreeMap<ClusterId, ClusterFlux>,
    focus: BTreeMap<u64, Focus>,
    mirrors: Mirrors,
    requests: Requests,
    changed_tx: async_channel::Sender<()>,
    changed_rx: async_channel::Receiver<()>,
}

struct ClusterData {
    id: ClusterId,
    online: bool,
    objects: Vec<Parsed>,
    info: FluxInfo,
}

impl GitOpsState {
    pub fn new(git_timeout: Duration) -> Self {
        let (changed_tx, changed_rx) = async_channel::bounded(CHANGE_BUFFER);
        Self {
            clusters: BTreeMap::new(),
            focus: BTreeMap::new(),
            mirrors: Mirrors::start(git_timeout, changed_tx.clone()),
            requests: Requests::default(),
            changed_tx,
            changed_rx,
        }
    }

    pub fn changed(&self) -> async_channel::Receiver<()> {
        self.changed_rx.clone()
    }

    pub fn sync<'a>(
        &mut self,
        clusters: impl IntoIterator<Item = (&'a ClusterId, &'a ClusterHandle)>,
    ) {
        let wanted: BTreeMap<&ClusterId, &ClusterHandle> = clusters.into_iter().collect();
        self.clusters.retain(|id, flux| {
            wanted
                .get(id)
                .is_some_and(|handle| Arc::ptr_eq(&handle.shared, &flux.handle.shared))
        });
        for (id, handle) in wanted {
            if !self.clusters.contains_key(id) {
                let flux = ClusterFlux::start(handle.clone(), self.changed_tx.clone());
                self.clusters.insert(id.clone(), flux);
            }
        }
        self.focus
            .retain(|_, focus| self.clusters.contains_key(&focus.key.cluster));
    }

    pub fn close_window(&mut self, window: u64) {
        self.focus.remove(&window);
    }

    pub fn command(&mut self, window: u64, command: &Command) {
        match command {
            Command::OpenFluxObject(key) => self.open(window, key),
            Command::SelectCluster(_) => {
                self.focus.remove(&window);
            }
            Command::SelectPage(_) => {
                self.focus.remove(&window);
            }
            Command::OpenResource(resource) => match flux_key(resource) {
                Some(key) => self.open(window, &key),
                None => {
                    self.focus.remove(&window);
                }
            },
            Command::OpenCommit(_) => {
                self.focus.remove(&window);
            }
            Command::Flux(key, _, action) => self.requests.record(key, *action, Timestamp::now()),
            _ => {}
        }
    }

    fn open(&mut self, window: u64, key: &FluxKey) {
        if self
            .focus
            .get(&window)
            .is_some_and(|focus| &focus.key == key)
        {
            return;
        }
        self.focus.remove(&window);
        if let Some(flux) = self.clusters.get(&key.cluster) {
            let focus = Focus::start(key.clone(), &flux.handle, self.changed_tx.clone());
            self.focus.insert(window, focus);
        }
    }

    pub fn page_state(&self, cluster: &ClusterId, page: Page) -> Option<flock_core::WatchState> {
        self.clusters.get(cluster)?.page_state(page)
    }

    pub fn health(&self, cluster: &ClusterId, now: Timestamp) -> Option<Vec<FluxHealth>> {
        let flux = self.clusters.get(cluster)?;
        if !flux.info().detected() {
            return None;
        }
        Some(cluster_health(&parsed_objects(flux), now))
    }

    fn data(&self) -> Vec<ClusterData> {
        self.clusters
            .iter()
            .map(|(id, flux)| {
                let objects = parsed_objects(flux);
                let online = !matches!(
                    flux.handle.status(),
                    ConnectionStatus::Offline(_) | ConnectionStatus::AuthExpired(_)
                );
                ClusterData {
                    id: id.clone(),
                    online,
                    objects,
                    info: flux.info(),
                }
            })
            .collect()
    }

    pub fn fill(&self, snapshot: &mut Snapshot) {
        let now = Timestamp::now();
        let data = self.data();
        for cluster in &data {
            if !cluster.info.detected() {
                continue;
            }
            let Some(handle) = self.clusters.get(&cluster.id).map(|flux| &flux.handle) else {
                continue;
            };
            let workloads = Workloads::of(handle);
            let health = |entry: &flock_flux::InventoryEntry| workloads.health(entry);
            for parsed in &cluster.objects {
                let focused = self
                    .focus
                    .values()
                    .find(|focus| convert::key(&cluster.id, parsed).as_ref() == Some(&focus.key));
                let detail = focused.map(|focus| lock(&focus.detail));
                let history = detail.as_ref().and_then(|d| d.history(now));
                let releases = detail.as_ref().and_then(|d| d.releases(now));
                let drift = detail.as_ref().and_then(|d| d.drift());
                drop(detail);
                let inputs = Inputs {
                    cluster: &cluster.id,
                    objects: &cluster.objects,
                    now,
                    health: &health,
                    history,
                    releases,
                    drift,
                };
                if let Some(mut object) = flux_object(parsed, inputs) {
                    let handled = parsed.object.last_handled_reconcile_at.as_deref();
                    self.requests.apply(&mut object, handled, now);
                    snapshot.flux.push(object);
                }
            }
            if let Some(summary) = snapshot.clusters.iter_mut().find(|c| c.id == cluster.id) {
                summary.nav.extend(nav_counts(&cluster.objects));
            }
        }
        let views: Vec<ClusterFluxView<'_>> = data
            .iter()
            .map(|cluster| ClusterFluxView {
                id: &cluster.id,
                online: cluster.online,
                objects: &cluster.objects,
                info: &cluster.info,
            })
            .collect();
        snapshot.gitops = grid::gitops(&views, now);
        snapshot.changes = self.changes(&data, now);
    }

    fn changes(&self, data: &[ClusterData], now: Timestamp) -> flock_core::ChangesLog {
        let mut consumers: BTreeMap<RepoKey, Vec<Consumer<'_>>> = BTreeMap::new();
        let mut wanted: BTreeMap<RepoKey, BTreeSet<String>> = BTreeMap::new();
        for cluster in data {
            let kustomizations = cluster
                .objects
                .iter()
                .filter(|parsed| parsed.kind() == ObjectKind::Kustomization);
            for kustomization in kustomizations {
                let Some(source) = convert::source(kustomization, &cluster.objects) else {
                    continue;
                };
                let Some(key) = repo_key(source) else {
                    continue;
                };
                let revisions = wanted.entry(key.clone()).or_default();
                revisions.extend(wanted_revision(source));
                consumers.entry(key).or_default().push(Consumer {
                    cluster: &cluster.id,
                    kustomization,
                    source,
                });
            }
        }
        self.mirrors.want(wanted);
        let states = self.mirrors.states();
        let repos: Vec<Repo<'_>> = consumers
            .into_iter()
            .map(|(key, consumers)| Repo {
                state: states.get(&key).cloned(),
                key,
                consumers,
            })
            .collect();
        let clusters: Vec<ClusterId> = data.iter().map(|cluster| cluster.id.clone()).collect();
        changes_log(&repos, &clusters, now)
    }
}

fn parsed_objects(flux: &ClusterFlux) -> Vec<Parsed> {
    let mut objects: Vec<Parsed> = KINDS
        .iter()
        .flat_map(|kind| {
            flux.objects(*kind)
                .into_iter()
                .filter_map(move |object| parse(&object, *kind))
        })
        .collect();
    carry_failures(&mut lock(&flux.failures), &mut objects);
    objects
}

fn carry_failures(failures: &mut Failures, objects: &mut [Parsed]) {
    let mut next = Failures::new();
    for parsed in objects {
        let Some(uid) = parsed.uid().map(str::to_owned) else {
            continue;
        };
        let remembered = failures.remove(&uid);
        if let Some(failure) = parsed.object.carry_failure(remembered) {
            next.insert(uid, failure);
        }
    }
    *failures = next;
}

fn cluster_health(objects: &[Parsed], now: Timestamp) -> Vec<FluxHealth> {
    objects
        .iter()
        .filter(|parsed| WATCHED.contains(&parsed.kind()))
        .map(|parsed| convert::health(parsed, objects, now))
        .collect()
}

fn flux_key(resource: &ResourceRef) -> Option<FluxKey> {
    let kind = FluxKind::from_label(&resource.kind)?;
    let namespace = resource.namespace.clone()?;
    Some(FluxKey {
        cluster: resource.cluster.clone(),
        kind,
        namespace,
        name: resource.name.clone(),
    })
}

fn nav_counts(objects: &[Parsed]) -> [(Page, Counts); 3] {
    let tally = |kinds: &[ObjectKind]| {
        let statuses: Vec<Status> = objects
            .iter()
            .filter(|parsed| kinds.contains(&parsed.kind()))
            .map(|parsed| core_status(summary(parsed, objects).status))
            .collect();
        let count = |status: Status| {
            let n = statuses.iter().filter(|s| **s == status).count();
            u32::try_from(n).unwrap_or(u32::MAX)
        };
        Counts::total(u32::try_from(statuses.len()).unwrap_or(u32::MAX))
            .with_err(count(Status::Err))
            .with_warn(count(Status::Warn))
            .with_sus(count(Status::Suspended))
    };
    [
        (
            FluxKind::Kustomization.page(),
            tally(&[ObjectKind::Kustomization]),
        ),
        (
            FluxKind::HelmRelease.page(),
            tally(&[ObjectKind::HelmRelease]),
        ),
        (
            FluxKind::GitRepository.page(),
            tally(&[
                ObjectKind::GitRepository,
                ObjectKind::HelmRepository,
                ObjectKind::OCIRepository,
            ]),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use convert::tests::{git_repository, kustomization, parsed};

    const FIRST_WINDOW: u64 = 1;
    const SECOND_WINDOW: u64 = 2;

    #[tokio::test]
    async fn flux_detail_watches_follow_each_window_until_it_closes() {
        let settings = crate::settings::KubeSettings::default();
        let env = Arc::new(crate::shell_env::ShellEnv {
            vars: BTreeMap::new(),
            path: Vec::new(),
            origin: crate::shell_env::EnvOrigin::Fallback {
                reason: "test".into(),
            },
        });
        let runtime = tokio::runtime::Handle::current();
        let (manager, _) = crate::manager::ClusterManager::new(runtime, settings.clone(), env);
        let context = crate::discovery::KubeContext {
            trust: crate::discovery::Fingerprint::default(),
            credentials: crate::discovery::Fingerprint::default(),
            id: flock_core::ContextRef {
                file: "/missing-c77-test-kubeconfig".into(),
                context: "test".into(),
            },
            cluster: "test".into(),
            user: None,
            namespace: None,
            server: None,
            provider: crate::provider::Provider::Generic,
            auth: crate::provider::AuthKind::None,
            is_current: false,
        };
        let handle = manager.connect(context);
        let cluster = ClusterId::new("test");
        let mut gitops = GitOpsState::new(settings.git_timeout);
        gitops.sync(std::iter::once((&cluster, &handle)));
        let first = FluxKey::new("test", FluxKind::HelmRelease, "apps", "first");
        let second = FluxKey::new("test", FluxKind::Kustomization, "apps", "second");
        gitops.command(FIRST_WINDOW, &Command::OpenFluxObject(first.clone()));
        gitops.command(SECOND_WINDOW, &Command::OpenFluxObject(second));
        assert_eq!(gitops.focus.len(), 2);
        gitops.command(SECOND_WINDOW, &Command::SelectPage(Page::GitOps));
        assert_eq!(gitops.focus.len(), 1);
        assert!(
            gitops
                .focus
                .get(&FIRST_WINDOW)
                .is_some_and(|focus| focus.key == first)
        );
        gitops.close_window(FIRST_WINDOW);
        assert!(gitops.focus.is_empty());
    }

    #[test]
    fn flux_resources_map_to_flux_keys() {
        let resource = |kind: &str| ResourceRef {
            cluster: ClusterId::new("fleet-a"),
            kind: kind.to_owned(),
            namespace: Some("payments".to_owned()),
            name: "checkout-api".to_owned(),
        };
        assert_eq!(
            flux_key(&resource("HelmRelease")),
            Some(FluxKey::new(
                "fleet-a",
                FluxKind::HelmRelease,
                "payments",
                "checkout-api"
            ))
        );
        assert_eq!(flux_key(&resource("Deployment")), None);
    }

    #[test]
    fn health_reads_the_source_like_the_object_page() {
        let mut failing = git_repository();
        failing["status"]["conditions"][0]["status"] = "False".into();
        failing["status"]["conditions"][0]["reason"] = "GitOperationFailed".into();
        let objects = [
            parsed(&kustomization("apps", "True", "3e0ff8ae123b")),
            parsed(&failing),
        ];
        let health = cluster_health(&objects, Timestamp::UNIX_EPOCH);
        let summarized: Vec<(&str, Status, &str)> = health
            .iter()
            .map(|h| (h.name.as_str(), h.status, h.sentence.as_str()))
            .collect();
        let page = summary(&objects[0], &objects);
        assert_eq!(
            summarized,
            [("apps", core_status(page.status), page.sentence.as_str())]
        );
        assert_eq!(health[0].status, Status::Warn);
    }

    #[test]
    fn nav_counts_tally_flux_pages() {
        let objects = [
            parsed(&kustomization("apps", "True", "3e0ff8ae123b")),
            parsed(&kustomization("broken", "False", "3e0ff8ae123b")),
            parsed(&git_repository()),
        ];
        let counts = nav_counts(&objects);
        assert_eq!(
            counts,
            [
                (Page::Kustomizations, Counts::total(2).with_err(1)),
                (Page::HelmReleases, Counts::total(0)),
                (Page::Sources, Counts::total(1)),
            ]
        );
    }

    #[test]
    fn a_failing_kustomization_stays_failing_through_each_retry() {
        let at = |ready: &str, reason: &str| {
            let mut value = kustomization("broken", ready, "3e0ff8ae123b");
            value["metadata"]["uid"] = serde_json::json!("uid-broken");
            value["status"]["conditions"][0]["reason"] = serde_json::json!(reason);
            vec![parsed(&value), parsed(&git_repository())]
        };
        let failing = Counts::total(1).with_err(1);
        let mut failures = Failures::new();
        for (ready, reason, expected) in [
            ("False", "ArtifactFailed", failing),
            ("Unknown", "Progressing", failing),
            ("False", "ArtifactFailed", failing),
            ("Unknown", "Progressing", failing),
            ("True", "ReconciliationSucceeded", Counts::total(1)),
            ("Unknown", "Progressing", Counts::total(1)),
        ] {
            let mut objects = at(ready, reason);
            carry_failures(&mut failures, &mut objects);
            let [(_, kustomizations), ..] = nav_counts(&objects);
            assert_eq!(kustomizations, expected, "{ready} {reason}");
        }
    }
}
