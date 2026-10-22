use flock_core::overview::{MetricsServer, Overview, UtilizationSource};
use flock_core::{Command, Page, Receiver, Sender, Snapshot, Status, Update, WatchState};

const LISTING: &str = "Listing Deployment";
const CONNECTING_STEP: &str = "Checking the API server and nodes";
const FORBIDDEN: &str = "deployments.apps is forbidden: User \"dev\" cannot list resource \"deployments\" in API group \"apps\" at the cluster scope";
const CONNECTING: &str = "Connecting";
const HISTORY_START: &str = "Oct 14, 16:00";
const LAST_SAMPLE: &str = "Oct 19, 12:28";
const RECENT_START: f32 = 0.95;
const PROMETHEUS_DOWN: &str = "Prometheus: service unavailable (503)";
const PAGES: [Page; 2] = [Page::ClusterOverview, Page::Deployments];

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum StateFixture {
    Loading,
    Connecting,
    Empty,
    Error,
    MetricsLocal,
    MetricsEmpty,
    MetricsError,
}

fn snapshot(fixture: StateFixture) -> Snapshot {
    let mut snapshot = flock_demo::snapshot();
    let state = match fixture {
        StateFixture::Loading => WatchState::pending(LISTING),
        StateFixture::Connecting => WatchState::pending(CONNECTING_STEP),
        StateFixture::Empty => WatchState::Synced,
        StateFixture::Error => WatchState::Failed(FORBIDDEN.to_owned()),
        StateFixture::MetricsLocal => WatchState::Synced,
        StateFixture::MetricsEmpty => WatchState::Synced,
        StateFixture::MetricsError => WatchState::Synced,
    };
    for cluster in &mut snapshot.clusters {
        if fixture == StateFixture::Connecting {
            cluster.status = Status::Progress;
            CONNECTING.clone_into(&mut cluster.state);
            cluster.nodes = None;
        }
        let pages = PAGES
            .into_iter()
            .map(|page| (page, state.clone()))
            .collect();
        snapshot.pages.insert(cluster.id.clone(), pages);
    }
    if fixture == StateFixture::Empty {
        for detail in snapshot.details.values_mut() {
            detail.resources.clear();
        }
    }
    for detail in snapshot.details.values_mut() {
        metrics(fixture, &mut detail.overview);
    }
    snapshot
}

fn metrics(fixture: StateFixture, overview: &mut Overview) {
    let local = UtilizationSource::Local {
        since: Some(HISTORY_START.to_owned()),
        last: Some(LAST_SAMPLE.to_owned()),
        metrics_server: MetricsServer::Answering,
    };
    match fixture {
        StateFixture::MetricsLocal => {
            overview.utilization_source = local;
            for range in &mut overview.utilization {
                let steps = range.cpu.len().saturating_sub(1).max(1);
                range.cpu_positions = (0..range.cpu.len())
                    .map(|ix| RECENT_START + (1. - RECENT_START) * share(ix, steps))
                    .collect();
                range.memory_positions = range.cpu_positions.clone();
            }
        }
        StateFixture::MetricsEmpty => {
            overview.utilization.clear();
            overview.utilization_source = local;
        }
        StateFixture::MetricsError => {
            overview.utilization.clear();
            overview.utilization_source = UtilizationSource::Error(PROMETHEUS_DOWN.to_owned());
        }
        StateFixture::Loading => {}
        StateFixture::Connecting => {}
        StateFixture::Empty => {}
        StateFixture::Error => {}
    }
}

fn share(ix: usize, steps: usize) -> f32 {
    let as_f32 = |n: usize| f32::from(u16::try_from(n).unwrap_or(u16::MAX));
    as_f32(ix) / as_f32(steps)
}

pub(crate) fn backend(fixture: StateFixture) -> (Receiver<Update>, Sender<Command>) {
    let (updates, receiver) = async_channel::unbounded();
    let (sender, commands) = async_channel::unbounded();
    std::thread::spawn(move || {
        let first = Update::Snapshot(Box::new(snapshot(fixture)));
        if updates.send_blocking(first).is_err() {
            return;
        }
        while let Ok(command) = commands.recv_blocking() {
            let command = match command {
                Command::ForWindow(_, command) => *command,
                command => command,
            };
            if let Command::Connect(_) = command {
                let loaded = Update::Snapshot(Box::new(flock_demo::snapshot()));
                if updates.send_blocking(loaded).is_err() {
                    return;
                }
            }
        }
    });
    (receiver, sender)
}
