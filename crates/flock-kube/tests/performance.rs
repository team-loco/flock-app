use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

use flock_core::{Backend, ClusterId, Command, NewCluster, Page, ResourceKind};
use flock_kube::assemble::{self, Extras, Selection};
use flock_kube::discovery::{DiscoverySources, KubeContext};
use flock_kube::live::SNAPSHOT_INTERVAL;
use flock_kube::settings::KubeSettings;
use flock_kube::shell_env::ShellEnv;
use flock_kube::store::ClusterStore;
use flock_kube::{detail::PodIndex, lists, pods, snapshot};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Patch, PatchParams};
use serde_json::json;

#[path = "performance_support/scale.rs"]
mod scale;
#[path = "performance_support/synthetic.rs"]
mod synthetic;

use scale::{LIVE_CONFIGMAPS, LIVE_PODS, SCALE_NAMESPACE, WAIT};

const SAMPLES: usize = 40;
const REBUILD_BUDGET: Duration = Duration::from_millis(20);

fn measure<T>(name: &str, mut build: impl FnMut() -> T) -> Duration {
    drop(black_box(build()));
    let mut timings = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        let result = black_box(build());
        timings.push(start.elapsed());
        drop(result);
    }
    timings.sort_unstable();
    let median = timings[SAMPLES / 2];
    println!(
        "{name}: median={median:?}, max={:?}, samples={SAMPLES}",
        timings.last()
    );
    median
}

#[test]
#[ignore = "run in release mode with --ignored --nocapture --test-threads=1"]
fn synthetic_snapshot_builders() {
    let raw = synthetic::pods();
    let cluster = ClusterId::new("scale");
    let now = Timestamp::now();
    let zone = TimeZone::UTC;
    let clock = pods::Clock { now, zone: &zone };
    let pods = measure("10k pod rows", || {
        pods::pods(&cluster, &raw, &[], &[], &clock)
    });
    let overview = measure("10k pod overview", || {
        (PodIndex::build(&raw, now), snapshot::pod_counts(&raw))
    });
    let combined = measure("10k selected cluster pods + overview", || {
        let rows = pods::pods(&cluster, &raw, &[], &[], &clock);
        let index = PodIndex::build(&raw, now);
        let counts = snapshot::pod_counts(&raw);
        (rows, index, counts)
    });
    let maps = synthetic::configmaps();
    let configmaps = measure("5k configmap rows", || {
        let mut rows: Vec<_> = maps.iter().map(|map| lists::config_map(map, now)).collect();
        rows.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
        rows
    });
    assert!(pods < REBUILD_BUDGET, "pod rows: {pods:?}");
    assert!(overview < REBUILD_BUDGET, "overview: {overview:?}");
    assert!(combined < REBUILD_BUDGET, "selected rebuild: {combined:?}");
    assert!(
        configmaps < REBUILD_BUDGET,
        "configmap rows: {configmaps:?}"
    );
}

const UPDATE_MEDIAN_BUDGET: Duration = Duration::from_millis(300);
const UPDATE_MAX_BUDGET: Duration = Duration::from_secs(1);
const LIVE_SAMPLES: usize = 12;
const PROBE_SPACING: Duration = Duration::from_secs(1);
const PROBE_POD: &str = "scale-pod-0000";
const PROBE_LABEL: &str = "flock-scale-probe";
const STORM_LABEL: &str = "flock-scale-storm";
const STORM_DURATION: Duration = Duration::from_secs(10);
const STORM_WORKERS: usize = 8;
const STORM_FIRST_POD: usize = 1;
const STORM_PODS: usize = LIVE_PODS - STORM_FIRST_POD;
const SECONDS_PER_MINUTE: u64 = 60;
const PERCENT: f64 = 100.;
const EVENT_LIMIT: usize = 200;

fn resident_kib() -> u64 {
    let pid = std::process::id().to_string();
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output();
    let Ok(output) = output else {
        panic!("ps failed")
    };
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    let Ok(rss) = text.trim().parse() else {
        panic!("invalid ps RSS")
    };
    rss
}

async fn visible(
    updates: &async_channel::Receiver<flock_core::Update>,
    predicate: impl Fn(&flock_core::Snapshot) -> bool,
) -> flock_core::Snapshot {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let received = tokio::time::timeout_at(deadline, updates.recv()).await;
        let Ok(Ok(update)) = received else {
            panic!("backend did not publish the expected snapshot")
        };
        if let flock_core::Update::Snapshot(snapshot) = update
            && predicate(&snapshot)
        {
            return Arc::unwrap_or_clone(snapshot);
        }
    }
}

fn cpu_time() -> Duration {
    let pid = std::process::id().to_string();
    let output = std::process::Command::new("ps")
        .args(["-o", "time=", "-p", &pid])
        .output();
    let Ok(output) = output else {
        panic!("ps failed")
    };
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    let Some((minutes, seconds)) = text.trim().split_once(':') else {
        panic!("invalid ps CPU time: {text}")
    };
    let (Ok(minutes), Ok(seconds)) = (minutes.parse::<u64>(), seconds.parse::<f64>()) else {
        panic!("invalid ps CPU time: {text}")
    };
    Duration::from_secs(minutes * SECONDS_PER_MINUTE) + Duration::from_secs_f64(seconds)
}

fn load_average() -> String {
    let output = std::process::Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output();
    let Ok(output) = output else {
        panic!("sysctl failed")
    };
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

struct Feed {
    updates: async_channel::Receiver<flock_core::Update>,
    cluster: ClusterId,
    state: flock_core::AppState,
    snapshots: usize,
}

impl Feed {
    fn new(updates: async_channel::Receiver<flock_core::Update>, cluster: ClusterId) -> Self {
        Self {
            updates,
            cluster,
            state: flock_core::AppState::default(),
            snapshots: 0,
        }
    }

    async fn until_labelled(&mut self, marker: &str) {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let received = tokio::time::timeout_at(deadline, self.updates.recv()).await;
            let Ok(Ok(update)) = received else {
                panic!("backend did not publish the probe label")
            };
            let flock_core::Update::Snapshot(snapshot) = update else {
                continue;
            };
            self.snapshots += 1;
            let shown = snapshot.pods.iter().any(|row| {
                row.name == PROBE_POD
                    && row.namespace == SCALE_NAMESPACE
                    && row.labels.get(PROBE_LABEL).map(String::as_str) == Some(marker)
            });
            self.state.update(flock_core::Update::Snapshot(snapshot));
            if shown {
                self.state
                    .apply(&Command::SelectCluster(Some(self.cluster.clone())));
                self.state.apply(&Command::SelectPage(Page::Pods));
                black_box(&self.state);
                return;
            }
        }
    }
}

#[derive(Default)]
struct Probes {
    total: Vec<Duration>,
    after_ack: Vec<Duration>,
}

impl Probes {
    async fn probe(&mut self, api: &kube::Api<Pod>, feed: &mut Feed, marker: &str) {
        let patch = json!({"metadata": {"labels": {PROBE_LABEL: marker}}});
        let start = Instant::now();
        let patched = api
            .patch(PROBE_POD, &PatchParams::default(), &Patch::Merge(&patch))
            .await;
        assert!(patched.is_ok(), "probe patch failed: {patched:?}");
        let acknowledged = Instant::now();
        feed.until_labelled(marker).await;
        self.total.push(start.elapsed());
        self.after_ack.push(acknowledged.elapsed());
    }

    fn report(self, label: &str) -> (Spread, Spread) {
        let samples = self.total.len();
        let total = spread(self.total);
        let after_ack = spread(self.after_ack);
        println!(
            "API patch to UI AppState ({label}): median={:?}, max={:?}; after API acknowledgement: median={:?}, max={:?}; samples={samples}, load={}",
            total.median,
            total.max,
            after_ack.median,
            after_ack.max,
            load_average()
        );
        (total, after_ack)
    }
}

struct Spread {
    median: Duration,
    max: Duration,
}

fn spread(mut timings: Vec<Duration>) -> Spread {
    timings.sort_unstable();
    let median = timings.get(timings.len() / 2).copied().unwrap_or_default();
    let max = timings.last().copied().unwrap_or_default();
    Spread { median, max }
}

async fn isolated_latency(api: &kube::Api<Pod>, feed: &mut Feed) {
    let mut probes = Probes::default();
    for sample in 0..LIVE_SAMPLES {
        tokio::time::sleep(PROBE_SPACING).await;
        let marker = format!("isolated-{sample}");
        probes.probe(api, feed, &marker).await;
    }
    let (total, after_ack) = probes.report("isolated");
    assert!(
        total.median < UPDATE_MEDIAN_BUDGET,
        "median UI update latency {:?}",
        total.median
    );
    assert!(
        after_ack.max < UPDATE_MAX_BUDGET,
        "max UI update latency after API acknowledgement {:?}",
        after_ack.max
    );
}

async fn churn(api: kube::Api<Pod>, worker: usize, until: Instant) -> usize {
    let mut patches = 0;
    while Instant::now() < until {
        let index = STORM_FIRST_POD + (worker + patches * STORM_WORKERS) % STORM_PODS;
        let name = format!("scale-pod-{index:04}");
        let patch = json!({"metadata": {"labels": {STORM_LABEL: patches.to_string()}}});
        let result = api
            .patch(&name, &PatchParams::default(), &Patch::Merge(&patch))
            .await;
        assert!(result.is_ok(), "storm patch failed: {result:?}");
        patches += 1;
    }
    patches
}

async fn storm(api: &kube::Api<Pod>, feed: &mut Feed) {
    let start = Instant::now();
    let until = start + STORM_DURATION;
    let cpu_before = cpu_time();
    let snapshots_before = feed.snapshots;
    let workers: Vec<_> = (0..STORM_WORKERS)
        .map(|worker| tokio::spawn(churn(api.clone(), worker, until)))
        .collect();
    let mut probes = Probes::default();
    while Instant::now() < until {
        let marker = format!("storm-{}", probes.total.len());
        probes.probe(api, feed, &marker).await;
    }
    let mut patches = 0;
    for worker in workers {
        let Ok(done) = worker.await else {
            panic!("storm worker failed")
        };
        patches += done;
    }
    let elapsed = start.elapsed();
    let cpu = cpu_time().saturating_sub(cpu_before);
    let snapshots = feed.snapshots - snapshots_before;
    let seconds = elapsed.as_secs_f64();
    let rate = f64::from(u32::try_from(snapshots).unwrap_or(u32::MAX)) / seconds;
    let patch_rate = f64::from(u32::try_from(patches).unwrap_or(u32::MAX)) / seconds;
    println!(
        "storm: {patch_rate:.0} pod patches/s, {rate:.1} snapshots/s, process CPU {:.0}%, load={}",
        cpu.as_secs_f64() / seconds * PERCENT,
        load_average()
    );
    let (_, after_ack) = probes.report("storm");
    let periods = elapsed.as_millis() / SNAPSHOT_INTERVAL.as_millis();
    let allowed = usize::try_from(periods).unwrap_or(usize::MAX) + 1;
    assert!(
        snapshots <= allowed,
        "{snapshots} snapshots in {elapsed:?} exceed one per {SNAPSHOT_INTERVAL:?}"
    );
    assert!(
        after_ack.max < UPDATE_MAX_BUDGET,
        "max UI update latency under storm after API acknowledgement {:?}",
        after_ack.max
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires FLOCK_E2E=1 and mise run e2e:scale; run alone in release mode"]
async fn kind_scale_snapshot_and_update_latency() {
    let (directory, sources, context) = scale::context();
    let settings = KubeSettings::default();
    let cluster = ClusterId::new("scale-b");
    benchmark_stores(&context, &settings, &cluster).await;
    benchmark_updates(&directory, sources, context, settings, cluster).await;
}

async fn benchmark_stores(context: &KubeContext, settings: &KubeSettings, cluster: &ClusterId) {
    let rss_before = resident_kib();
    let (manager, handle, subscriptions) = scale::populated_stores(context, settings).await;
    let rss_stores = resident_kib();
    println!(
        "watcher population RSS delta={} KiB (includes client/runtime, not exact heap size)",
        rss_stores.saturating_sub(rss_before)
    );
    let extras = Extras::default();
    for page in [
        Page::ClusterOverview,
        Page::Pods,
        Page::ConfigMaps,
        Page::Services,
    ] {
        let selection = Selection {
            pod_rows: page == Page::Pods,
            event_limit: EVENT_LIMIT,
            yaml: &[],
            inspected: &[],
        };
        let name = format!("kind scale {page:?} selected rebuild");
        measure(&name, || {
            assemble::cluster(
                cluster,
                &handle,
                &extras,
                None,
                Some(&selection),
                Timestamp::now(),
            )
        });
    }
    drop(subscriptions);
    manager.disconnect(&context.id);
    drop(handle);
    drop(manager);
}

async fn benchmark_updates(
    directory: &tempfile::TempDir,
    sources: DiscoverySources,
    context: KubeContext,
    settings: KubeSettings,
    cluster: ClusterId,
) {
    let support = directory.path().join("support");
    let store = ClusterStore::empty(&support);
    let backend = flock_kube::LiveBackend::new(settings, sources, store);
    let (updates_tx, updates) = async_channel::unbounded();
    let (commands, commands_rx) = async_channel::unbounded();
    Box::new(backend).start(updates_tx, commands_rx);
    let add = Command::AddClusters(vec![NewCluster {
        context: context.id.clone(),
        name: cluster.to_string(),
    }]);
    assert!(commands.send(add).await.is_ok());
    visible(&updates, |s| s.cluster(&cluster).is_some()).await;
    assert!(
        commands
            .send(Command::SelectCluster(Some(cluster.clone())))
            .await
            .is_ok()
    );
    assert!(
        commands
            .send(Command::SelectPage(Page::ConfigMaps))
            .await
            .is_ok()
    );
    let configmaps = visible(&updates, |s| {
        s.detail(&cluster)
            .is_some_and(|d| d.resources(ResourceKind::ConfigMap).len() >= LIVE_CONFIGMAPS)
    })
    .await;
    assert!(
        configmaps.pods.is_empty(),
        "ConfigMaps should not build hidden pod details"
    );
    let Some(summary) = configmaps.cluster(&cluster) else {
        panic!("cluster missing")
    };
    assert!(summary.nav.get(&Page::Pods).is_some_and(|counts| {
        counts
            .total
            .is_some_and(|total| usize::try_from(total).is_ok_and(|total| total >= LIVE_PODS))
    }));
    let env = ShellEnv::from_process(String::new());
    let connected =
        flock_kube::client::connect(&context, &env, KubeSettings::default().connect_timeout).await;
    let Ok(client) = connected else {
        panic!("client failed")
    };
    assert!(commands.send(Command::SelectPage(Page::Pods)).await.is_ok());
    visible(&updates, |s| {
        s.pods
            .iter()
            .filter(|p| p.namespace == SCALE_NAMESPACE)
            .count()
            == LIVE_PODS
    })
    .await;
    let api: kube::Api<Pod> = kube::Api::namespaced(client, SCALE_NAMESPACE);
    let mut feed = Feed::new(updates, cluster);
    let warmed = api.get(PROBE_POD).await;
    assert!(
        warmed.is_ok(),
        "could not warm the API connection: {warmed:?}"
    );
    isolated_latency(&api, &mut feed).await;
    storm(&api, &mut feed).await;
    let cleared = json!({"metadata": {"labels": {PROBE_LABEL: null}}});
    let cleanup = api
        .patch(PROBE_POD, &PatchParams::default(), &Patch::Merge(&cleared))
        .await;
    assert!(cleanup.is_ok(), "probe label cleanup failed: {cleanup:?}");
    drop(commands);
}
