mod add_cluster;
mod changes;
mod compare;
mod crds;
mod derive;
mod detail;
mod edit;
mod flux;
mod flux_events;
mod gitops;
mod helm;
mod home;
mod lists;
#[cfg(feature = "loco")]
mod loco;
mod multi_logs;
mod namespace;
mod pod_streams;
mod pods;
mod related;
mod resources;
mod settings;
mod sim;
mod simulate;
mod workload;

use std::collections::BTreeMap;
use std::sync::mpsc;
use std::thread;

use flock_core::home::problems_badge;
use flock_core::{
    Backend, ClusterId, ClusterSummary, Command, Counts, Page, Receiver, Sender, Snapshot, Status,
    Update,
};

const PROD_US_EAST: &str = "prod-us-east";
const OCTET_BASE: usize = 4;

pub struct DemoBackend;

impl Backend for DemoBackend {
    fn start(self: Box<Self>, updates: Sender<Update>, commands: Receiver<Command>) {
        let spawned = thread::Builder::new()
            .name("flock-demo".into())
            .spawn(move || run(&updates, &commands));
        if let Err(error) = spawned {
            tracing::error!(%error, "demo backend thread failed to start");
        }
    }
}

enum Input {
    Command(Command),
    Settle(simulate::Settle),
    SettleFlux(flux::Settle),
}

fn run(updates: &Sender<Update>, commands: &Receiver<Command>) {
    let mut state = snapshot();
    if updates
        .send_blocking(Update::Snapshot(Box::new(state.clone())))
        .is_err()
    {
        return;
    }
    let mut adding = add_cluster::AddClusters::default();
    let mut flux = flux::Flux::default();
    let (input_tx, input_rx) = mpsc::channel();
    let forward = input_tx.clone();
    let commands = commands.clone();
    let forwarder = thread::Builder::new()
        .name("flock-demo-commands".into())
        .spawn(move || {
            while let Ok(command) = commands.recv_blocking() {
                if forward.send(Input::Command(command)).is_err() {
                    break;
                }
            }
        });
    if let Err(error) = forwarder {
        tracing::error!(%error, "demo command forwarder failed to start");
        return;
    }
    let workloads = workload::Workloads::new(&state);
    while let Ok(input) = input_rx.recv() {
        let changed = match input {
            Input::Command(command) => {
                let command = match command {
                    Command::ForWindow(_, command) => *command,
                    Command::WindowClosed(_) => continue,
                    command => command,
                };
                if adding.handle(&command, &mut state, updates) {
                    continue;
                }
                tracing::debug!(?command, "demo backend received command");
                if edit::handle(&command, &state) {
                    continue;
                }
                if pod_streams::handle(&command, &state, &workloads) {
                    continue;
                }
                if multi_logs::handle(&command, &state) {
                    continue;
                }
                workloads.handle(&command);
                let related = related::handle(&command, &mut state);
                if let Command::Flux(key, _, action) = &command {
                    let outcome = flux.apply(&mut state, key, *action);
                    if let Some(pending) = outcome.settle {
                        schedule(&input_tx, Input::SettleFlux(pending));
                    }
                    outcome.changed
                } else {
                    let forwards_changed = simulate::forward(&mut state, &command);
                    let immediate = sim::apply_now(&mut state, &command) || forwards_changed;
                    let pending = simulate::apply(&mut state, &command);
                    let changed = immediate || pending.is_some() || related;
                    if let Some(pending) = pending {
                        schedule(&input_tx, Input::Settle(pending));
                    }
                    changed
                }
            }
            Input::Settle(pending) => {
                if let Some(next) = simulate::settle(&mut state, &pending) {
                    schedule(&input_tx, Input::Settle(next));
                }
                true
            }
            Input::SettleFlux(pending) => {
                flux::settle(&mut state, &pending);
                true
            }
        };
        if changed
            && updates
                .send_blocking(Update::Snapshot(Box::new(state.clone())))
                .is_err()
        {
            return;
        }
    }
}

fn schedule(input: &mpsc::Sender<Input>, next: Input) {
    let input = input.clone();
    let timer = thread::Builder::new()
        .name("flock-demo-settle".into())
        .spawn(move || {
            thread::sleep(simulate::SETTLE);
            let _ = input.send(next);
        });
    if let Err(error) = timer {
        tracing::error!(%error, "demo settle timer failed to start");
    }
}

pub fn snapshot() -> Snapshot {
    let prod_pods = pods::pods();
    let home = home::flock_home();
    let flock_nav = BTreeMap::from([
        (Page::Problems, problems_badge(&home.problems)),
        (Page::GitOps, Counts::default().with_err(1).with_sus(1)),
    ]);
    let mut snapshot = Snapshot {
        clusters: vec![
            prod_us_east(),
            cluster(ClusterSeed {
                name: "prod-eu-west",
                status: Status::Ok,
                state: "Healthy",
                distro: "EKS",
                region: "eu-west-1",
                version: Some("v1.31.2"),
                nodes: Some(11),
                problems: Counts::default().with_sus(1).with_warn(1),
            }),
            staging(),
            cluster(ClusterSeed {
                name: "dev-kind",
                status: Status::Ok,
                state: "Healthy",
                distro: "kind",
                region: "local",
                version: Some("v1.31.0"),
                nodes: Some(3),
                problems: Counts::default(),
            }),
            cluster(ClusterSeed {
                name: "qa-gke",
                status: Status::Err,
                state: "Auth expired",
                distro: "GKE",
                region: "us-central1",
                version: None,
                nodes: None,
                problems: Counts::default().with_err(1),
            }),
            cluster(ClusterSeed {
                name: "edge-k3s",
                status: Status::Offline,
                state: "Offline 3h",
                distro: "k3s",
                region: "edge",
                version: Some("v1.29.8"),
                nodes: None,
                problems: Counts::default(),
            }),
        ],
        flock_nav,
        home,
        resources: resources::resources(),
        changes: changes::log(),
        details: BTreeMap::from([(
            ClusterId::new(PROD_US_EAST),
            detail::prod_us_east(&prod_pods),
        )]),
        pages: BTreeMap::new(),
        gitops: gitops::gitops(),
        flux: flux::objects(),
        flux_events: flux_events::events(),
        #[cfg(feature = "loco")]
        loco: loco::clusters(),
        comparisons: compare::comparisons(),
        catalog: settings::catalog(),
        pods: prod_pods,
        forwards: Vec::new(),
        crds: BTreeMap::from([(ClusterId::new(PROD_US_EAST), crds::definitions())]),
        custom: crds::objects(&ClusterId::new(PROD_US_EAST)),
        helm: helm::releases(&ClusterId::new(PROD_US_EAST)),
        helm_details: helm::details(&ClusterId::new(PROD_US_EAST)),
    };
    simulate::recount_pods(&mut snapshot, &ClusterId::new(PROD_US_EAST));
    derive_clusters(&mut snapshot);
    snapshot
}

fn derive_clusters(snapshot: &mut Snapshot) {
    let prod = ClusterId::new(PROD_US_EAST);
    let Some(template_detail) = snapshot.details.get(&prod).cloned() else {
        return;
    };
    let template_pods: Vec<flock_core::pods::Pod> = snapshot
        .pods
        .iter()
        .filter(|p| p.cluster == prod)
        .cloned()
        .collect();
    let template = derive::Template {
        detail: &template_detail,
        pods: &template_pods,
    };
    for (ix, summary) in snapshot.clusters.iter_mut().enumerate() {
        if summary.id == prod {
            continue;
        }
        let checkout = derive::checkout(&snapshot.resources, &summary.id);
        let derived = derive::cluster(
            summary,
            OCTET_BASE + ix,
            &snapshot.home.problems,
            checkout.as_ref(),
            &template,
        );
        let Some(derived) = derived else {
            continue;
        };
        summary.nav = derive::nav(summary, &derived.detail, &derived.pods, &snapshot.flux);
        snapshot.pods.extend(derived.pods);
        snapshot.details.insert(summary.id.clone(), derived.detail);
    }
}

#[derive(Clone, Copy)]
struct ClusterSeed {
    name: &'static str,
    status: Status,
    state: &'static str,
    distro: &'static str,
    region: &'static str,
    version: Option<&'static str>,
    nodes: Option<u32>,
    problems: Counts,
}

fn cluster(seed: ClusterSeed) -> ClusterSummary {
    let nav = seed
        .nodes
        .map(|n| BTreeMap::from([(Page::Nodes, Counts::total(n))]))
        .unwrap_or_default();
    ClusterSummary {
        id: ClusterId::new(seed.name),
        name: seed.name.into(),
        status: seed.status,
        state: seed.state.into(),
        distro: seed.distro.into(),
        region: seed.region.into(),
        version: seed.version.map(Into::into),
        nodes: seed.nodes,
        problems: seed.problems,
        nav,
        metrics: home::metrics(seed.name),
    }
}

fn staging() -> ClusterSummary {
    let mut summary = cluster(ClusterSeed {
        name: "staging",
        status: Status::Warn,
        state: "Warnings",
        distro: "GKE",
        region: "us-central1",
        version: Some("v1.30.5"),
        nodes: Some(6),
        problems: Counts::default().with_warn(2),
    });
    summary.nav = BTreeMap::from([
        (Page::Nodes, Counts::total(6)),
        (Page::Namespaces, Counts::total(14)),
        (Page::Events, Counts::default().with_warn(3)),
        (Page::Pods, Counts::total(187).with_warn(1)),
        (Page::Deployments, Counts::total(31)),
        (Page::StatefulSets, Counts::total(7)),
        (Page::DaemonSets, Counts::total(9)),
        (Page::Jobs, Counts::total(12)),
        (Page::CronJobs, Counts::total(5)),
        (Page::Kustomizations, Counts::total(3)),
        (Page::HelmReleases, Counts::total(9).with_warn(1)),
        (Page::Sources, Counts::total(4)),
        (Page::Services, Counts::total(38)),
        (Page::Ingresses, Counts::total(6)),
        (Page::ConfigMaps, Counts::total(54)),
        (Page::Secrets, Counts::total(40)),
        (Page::Pvcs, Counts::total(11)),
    ]);
    summary
}

fn prod_us_east() -> ClusterSummary {
    let mut summary = cluster(ClusterSeed {
        name: PROD_US_EAST,
        status: Status::Err,
        state: "Degraded",
        distro: "EKS",
        region: "us-east-1",
        version: Some("v1.31.2"),
        nodes: Some(14),
        problems: Counts::default().with_err(2).with_warn(3),
    });
    summary.nav = BTreeMap::from([
        (Page::Nodes, Counts::total(14).with_warn(1)),
        (Page::Namespaces, Counts::total(23)),
        #[cfg(feature = "loco")]
        (Page::LocoPlatform, Counts::default().with_warn(1)),
        #[cfg(feature = "loco")]
        (Page::LocoApplications, Counts::total(214).with_err(1)),
        #[cfg(feature = "loco")]
        (Page::LocoBuilds, Counts::default().with_err(1)),
        #[cfg(feature = "loco")]
        (Page::LocoRoutes, Counts::total(44).with_warn(2)),
        (Page::Kustomizations, Counts::total(12).with_err(1)),
        (Page::HelmReleases, Counts::total(9)),
        (Page::Sources, Counts::total(4)),
    ]);
    summary.nav.extend(detail::prod_us_east(&[]).nav());
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pod_nav_counts_come_from_the_pod_rows() {
        use flock_core::pods::{PodFilter, failing, filter_counts};
        let snapshot = snapshot();
        let cluster = ClusterId::new(PROD_US_EAST);
        let nav = snapshot
            .clusters
            .iter()
            .find(|c| c.id == cluster)
            .and_then(|c| c.nav.get(&Page::Pods).copied());
        assert_eq!(nav, Some(Counts::total(412).with_err(5)));
        let rows: Vec<&flock_core::pods::Pod> = snapshot
            .pods
            .iter()
            .filter(|p| p.cluster == cluster)
            .collect();
        let counts = filter_counts(&rows);
        let phases = snapshot.details.get(&cluster).map(|d| d.overview.pods);
        let phases = phases.map(|p| (p.running, p.pending, p.failing));
        let crashing = rows.iter().filter(|pod| failing(pod)).count();
        let healthy_running = rows
            .iter()
            .filter(|pod| PodFilter::Running.keeps(pod) && !failing(pod))
            .count();
        assert_eq!(counts[3], (PodFilter::Pending, 9));
        assert_eq!((healthy_running, crashing), (398, 5));
        assert_eq!(phases, Some((398, 9, 5)));
    }

    #[test]
    fn prod_us_east_nav_counts_come_from_its_detail() {
        let summary = prod_us_east();
        assert_eq!(
            summary.nav.get(&Page::Deployments),
            Some(&Counts::total(48).with_err(1).with_warn(1))
        );
        assert_eq!(
            summary.nav.get(&Page::Events),
            Some(&Counts::default().with_err(2).with_warn(5))
        );
        assert_eq!(summary.nav.get(&Page::ConfigMaps), Some(&Counts::total(88)));
    }

    #[test]
    fn sample_matches_design_rollups() {
        let snapshot = snapshot();
        assert_eq!(snapshot.clusters.len(), 6);
        assert_eq!(snapshot.connected(), 4);
        let problems = snapshot.problems();
        assert_eq!((problems.err, problems.warn, problems.sus), (3, 6, 1));
    }

    #[test]
    fn problem_list_agrees_with_cluster_rollups() {
        let snapshot = snapshot();
        for cluster in &snapshot.clusters {
            let listed = snapshot
                .home
                .problems
                .iter()
                .filter(|p| p.target.cluster == cluster.id);
            let counts = flock_core::home::problem_counts(listed);
            assert_eq!(
                (counts.err, counts.warn, counts.sus),
                (
                    cluster.problems.err,
                    cluster.problems.warn,
                    cluster.problems.sus
                ),
                "{}",
                cluster.name
            );
        }
    }

    #[test]
    fn problems_badge_matches_design() {
        let badge = snapshot().flock_nav.get(&Page::Problems).copied();
        assert_eq!(badge, Some(Counts::default().with_err(3).with_warn(6)));
    }

    #[test]
    fn removing_a_cluster_drops_its_problems_from_the_flock() {
        let (tx, _rx) = async_channel::unbounded();
        let mut flock = snapshot();
        let mut adding = add_cluster::AddClusters::default();
        let removed = ClusterId::new(PROD_US_EAST);
        assert!(adding.handle(&Command::RemoveCluster(removed.clone()), &mut flock, &tx));
        assert!(
            flock
                .home
                .problems
                .iter()
                .all(|p| p.target.cluster != removed)
        );
        let badge = flock.flock_nav.get(&Page::Problems).copied();
        assert_eq!(badge, Some(Counts::default().with_err(1).with_warn(3)));
    }

    #[test]
    fn prod_us_east_keeps_the_design_data() {
        let snapshot = snapshot();
        let prod = ClusterId::new(PROD_US_EAST);
        let mut expected = detail::prod_us_east(&pods::pods());
        expected.overview.tops = flock_core::pod_tops::PodTops::of(&pods::pods());
        assert_eq!(snapshot.detail(&prod), Some(&expected));
        let mut summary = snapshot.cluster(&prod).cloned();
        if let Some(summary) = summary.as_mut() {
            summary.nav.remove(&Page::Pods);
        }
        assert_eq!(summary, Some(prod_us_east()));
        let prod_pods: Vec<_> = snapshot
            .pods
            .iter()
            .filter(|p| p.cluster == prod)
            .cloned()
            .collect();
        assert_eq!(prod_pods, pods::pods());
    }

    #[test]
    fn reachable_clusters_have_pages_derived_from_their_summary() {
        let snapshot = snapshot();
        for cluster in snapshot.clusters.iter().filter(|c| c.metrics.is_some()) {
            let detail = snapshot.detail(&cluster.id);
            let Some((detail, metrics)) = detail.zip(cluster.metrics.as_ref()) else {
                panic!("{} has no detail", cluster.name);
            };
            assert_eq!(
                u32::try_from(detail.nodes.len()).ok(),
                cluster.nodes,
                "{}",
                cluster.name
            );
            assert_eq!(
                Some(detail.overview.pods.total()),
                metrics.pods.total,
                "{}",
                cluster.name
            );
            let listed_pods = flock_core::pods::nav_counts(&snapshot.pods, &cluster.id);
            assert_eq!(listed_pods.total, metrics.pods.total, "{}", cluster.name);
            assert_eq!(
                cluster.nav.get(&Page::Pods),
                Some(&listed_pods),
                "{}",
                cluster.name
            );
            let deployments =
                u32::try_from(detail.resources(flock_core::ResourceKind::Deployment).len()).ok();
            let listed = cluster.nav.get(&Page::Deployments).and_then(|c| c.total);
            assert_eq!(listed, deployments, "{}", cluster.name);
            let pods = snapshot.pods.iter().filter(|p| p.cluster == cluster.id);
            assert!(pods.count() > 0, "{}", cluster.name);
        }
    }

    #[test]
    fn derived_cluster_lists_its_problems_and_checkout() {
        let snapshot = snapshot();
        let staging = ClusterId::new("staging");
        let detail = snapshot.detail(&staging);
        let warnings = detail.map(flock_core::ClusterDetail::warnings);
        assert_eq!(warnings, Some(2));
        let flagged = snapshot
            .pods
            .iter()
            .find(|p| p.cluster == staging && p.name == "search-indexer-0")
            .map(|p| p.status);
        assert_eq!(flagged, Some(Status::Warn));
        let checkout = detail
            .and_then(|d| d.resources(flock_core::ResourceKind::Deployment).first())
            .map(|r| (r.name.as_str(), r.cells.first().map(String::as_str)));
        assert_eq!(checkout, Some(("checkout-api", Some("2/2"))));
    }

    #[test]
    fn unreachable_clusters_have_no_pages() {
        let snapshot = snapshot();
        for name in ["qa-gke", "edge-k3s"] {
            let id = ClusterId::new(name);
            assert!(snapshot.detail(&id).is_none(), "{name}");
            assert!(snapshot.pods.iter().all(|p| p.cluster != id), "{name}");
        }
    }

    #[test]
    fn unreachable_clusters_have_no_metrics() {
        let snapshot = snapshot();
        let without: Vec<&str> = snapshot
            .clusters
            .iter()
            .filter(|c| c.metrics.is_none())
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(without, ["qa-gke", "edge-k3s"]);
    }

    #[test]
    fn backend_sends_snapshot_then_drains_commands() {
        let (update_tx, update_rx) = async_channel::unbounded();
        let (command_tx, command_rx) = async_channel::unbounded();
        Box::new(DemoBackend).start(update_tx, command_rx);
        let first = update_rx.recv_blocking();
        assert_eq!(first, Ok(Update::Snapshot(Box::new(snapshot()))));
        assert!(command_tx.send_blocking(Command::ToggleSidebar).is_ok());
    }
}
