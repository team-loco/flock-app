use std::hint::black_box;
use std::time::{Duration, Instant};

use flock_core::{Backend, ClusterId, Command, NewCluster, Page, ResourceKind, Snapshot, Update};
use flock_kube::LiveBackend;
use flock_kube::detail::PodIndex;
use flock_kube::settings::KubeSettings;
use flock_kube::shell_env::ShellEnv;
use flock_kube::store::ClusterStore;
use flock_kube::{lists, pods};
use jiff::Timestamp;
use jiff::tz::TimeZone;

#[global_allocator]
static ALLOCATOR: dhat::Alloc = dhat::Alloc;

#[path = "performance_support/scale.rs"]
mod scale;
#[path = "performance_support/synthetic.rs"]
mod synthetic;

const MAX_INDEX_ALLOCATIONS: u64 = 16;
const MAX_POD_ALLOCATIONS: u64 = 140_016;
const MAX_CONFIGMAP_ALLOCATIONS: u64 = 20_016;

const WATCHER_HEAP_BUDGET: usize = 32 * 1024 * 1024;
const CANCEL_WAIT: Duration = Duration::from_millis(100);
const BACKGROUND_SETTLE: Duration = Duration::from_secs(30);
const BACKGROUND_GRACE: Duration = Duration::from_secs(3);
const BACKGROUND_HEAP_BUDGET: usize = 64 * 1024 * 1024;
const WATCH_METRIC: &str = "apiserver_longrunning_requests{";
const WATCH_VERB: &str = "verb=\"WATCH\"";
const SUPPORT_DIR: &str = "support";
const MIB: f64 = 1024. * 1024.;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires FLOCK_E2E=1 and e2e:scale; run alone with --test-threads=1"]
async fn kind_scale_watcher_heap() {
    let (_directory, _sources, context) = scale::context();
    let profiler = dhat::Profiler::builder().testing().build();
    let settings = KubeSettings::default();
    let (manager, handle, subscriptions) = scale::populated_stores(&context, &settings).await;
    let populated = dhat::HeapStats::get().curr_bytes;
    drop(subscriptions);
    tokio::time::sleep(CANCEL_WAIT).await;
    let emptied = dhat::HeapStats::get().curr_bytes;
    let retained = populated.saturating_sub(emptied);
    println!(
        "watcher stores + watch tasks released heap={retained} bytes, populated process heap={populated} bytes"
    );
    assert!(retained > 0, "watcher memory was not released");
    assert!(
        retained < WATCHER_HEAP_BUDGET,
        "watcher heap exceeds budget: {retained}"
    );
    manager.disconnect(&context.id);
    drop(handle);
    drop(manager);
    drop(profiler);
}

#[test]
#[ignore = "run alone in release mode with --test-threads=1"]
fn synthetic_snapshot_allocations() {
    let raw = synthetic::pods();
    let maps = synthetic::configmaps();
    let cluster = ClusterId::new("scale");
    let now = Timestamp::now();
    let zone = TimeZone::UTC;
    let clock = pods::Clock { now, zone: &zone };
    let profiler = dhat::Profiler::builder().testing().build();
    let rows = pods::pods(&cluster, &raw, &[], &[], &clock);
    let pod_stats = dhat::HeapStats::get();
    let index = PodIndex::build(&raw, now);
    let overview_stats = dhat::HeapStats::get();
    println!(
        "10k pod rows allocated {} blocks, {} bytes; overview added {} blocks, {} bytes",
        pod_stats.total_blocks,
        pod_stats.total_bytes,
        overview_stats.total_blocks - pod_stats.total_blocks,
        overview_stats.total_bytes - pod_stats.total_bytes
    );
    assert!(
        pod_stats.total_blocks <= MAX_POD_ALLOCATIONS,
        "pod rows allocated {} blocks",
        pod_stats.total_blocks
    );
    let index_allocations = overview_stats.total_blocks - pod_stats.total_blocks;
    assert!(
        index_allocations <= MAX_INDEX_ALLOCATIONS,
        "overview allocated {index_allocations} blocks"
    );
    let configmaps: Vec<_> = maps.iter().map(|map| lists::config_map(map, now)).collect();
    let configmap_stats = dhat::HeapStats::get();
    let allocations = configmap_stats.total_blocks - overview_stats.total_blocks;
    println!("5k configmap rows allocated {allocations} blocks");
    assert!(
        allocations <= MAX_CONFIGMAP_ALLOCATIONS,
        "configmap rows allocated {allocations} blocks"
    );
    black_box((&rows, &index, &configmaps));
    drop(configmaps);
    drop(index);
    drop(rows);
    drop(profiler);
}

async fn watch_count(client: &kube::Client) -> f64 {
    let Ok(request) = http::Request::get("/metrics").body(Vec::new()) else {
        panic!("metrics request")
    };
    let Ok(text) = client.request_text(request).await else {
        panic!("API server metrics unreadable")
    };
    text.lines()
        .filter(|line| line.starts_with(WATCH_METRIC) && line.contains(WATCH_VERB))
        .filter_map(|line| line.rsplit_once(' '))
        .filter_map(|(_, value)| value.parse::<f64>().ok())
        .sum()
}

fn counted(snapshot: &Snapshot, cluster: &ClusterId, page: Page) -> Option<u32> {
    snapshot.cluster(cluster)?.nav.get(&page)?.total
}

fn populated(snapshot: &Snapshot, cluster: &ClusterId) -> bool {
    let at_least = |page, wanted: usize| {
        counted(snapshot, cluster, page)
            .is_some_and(|total| usize::try_from(total).is_ok_and(|total| total >= wanted))
    };
    at_least(Page::Pods, scale::LIVE_PODS)
        && at_least(Page::ConfigMaps, scale::LIVE_CONFIGMAPS)
        && at_least(Page::Services, scale::LIVE_SERVICES)
        && at_least(Page::Deployments, scale::LIVE_DEPLOYMENTS)
}

async fn next_snapshot(
    updates: &async_channel::Receiver<Update>,
    until: tokio::time::Instant,
) -> Option<Snapshot> {
    loop {
        let received = tokio::time::timeout_at(until, updates.recv()).await;
        match received {
            Ok(Ok(Update::Snapshot(snapshot))) => return Some(*snapshot),
            Ok(Ok(_)) => {}
            Ok(Err(_)) => return None,
            Err(_) => return None,
        }
    }
}

fn label(elapsed: Option<Duration>) -> String {
    elapsed.map_or_else(|| "not shown".to_owned(), |elapsed| format!("{elapsed:?}"))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires FLOCK_E2E=1 and e2e:scale; run alone with --test-threads=1"]
async fn kind_scale_background_heap_and_watches() {
    let (directory, sources, context) = scale::context();
    let env = ShellEnv::from_process(String::new());
    let settings = KubeSettings::default();
    let connected = flock_kube::client::connect(&context, &env, settings.connect_timeout).await;
    let Ok(client) = connected else {
        panic!("client failed")
    };
    let watches_before = watch_count(&client).await;
    let profiler = dhat::Profiler::builder().testing().build();
    let heap_before = dhat::HeapStats::get().curr_bytes;
    let store = ClusterStore::empty(&directory.path().join(SUPPORT_DIR));
    let backend = LiveBackend::new(settings, sources, store);
    let (updates_tx, updates) = async_channel::unbounded();
    let (commands, commands_rx) = async_channel::unbounded();
    Box::new(backend).start(updates_tx, commands_rx);
    let cluster = ClusterId::new("scale-b");
    let started = Instant::now();
    let add = Command::AddClusters(vec![NewCluster {
        context: context.id.clone(),
        name: cluster.to_string(),
    }]);
    assert!(commands.send(add).await.is_ok());
    let deadline = tokio::time::Instant::now() + BACKGROUND_SETTLE;
    let mut first_pods = None;
    let mut first_deployments = None;
    while let Some(snapshot) = next_snapshot(&updates, deadline).await {
        if first_pods.is_none() && counted(&snapshot, &cluster, Page::Pods).is_some() {
            first_pods = Some(started.elapsed());
        }
        if first_deployments.is_none() && counted(&snapshot, &cluster, Page::Deployments).is_some()
        {
            first_deployments = Some(started.elapsed());
        }
        if populated(&snapshot, &cluster) {
            break;
        }
    }
    tokio::time::sleep(BACKGROUND_GRACE).await;
    while updates.try_recv().is_ok() {}
    let idle_heap = dhat::HeapStats::get()
        .curr_bytes
        .saturating_sub(heap_before);
    let idle_watches = watch_count(&client).await - watches_before;
    for page in [Page::ClusterOverview, Page::ConfigMaps] {
        let step = match page {
            Page::ClusterOverview => Command::SelectCluster(Some(cluster.clone())),
            other => Command::SelectPage(other),
        };
        assert!(commands.send(step).await.is_ok());
    }
    let deadline = tokio::time::Instant::now() + scale::WAIT;
    while let Some(snapshot) = next_snapshot(&updates, deadline).await {
        let listed = snapshot
            .detail(&cluster)
            .is_some_and(|detail| !detail.resources(ResourceKind::ConfigMap).is_empty());
        if listed {
            break;
        }
    }
    tokio::time::sleep(BACKGROUND_GRACE).await;
    while updates.try_recv().is_ok() {}
    let page_heap = dhat::HeapStats::get()
        .curr_bytes
        .saturating_sub(heap_before);
    let page_watches = watch_count(&client).await - watches_before;
    let mib = |bytes: usize| f64::from(u32::try_from(bytes).unwrap_or(u32::MAX)) / MIB;
    println!(
        "background: first Pods count {}, first Deployments count {}; after connect heap={:.1} MiB watches={idle_watches}; on ConfigMaps heap={:.1} MiB watches={page_watches}",
        label(first_pods),
        label(first_deployments),
        mib(idle_heap),
        mib(page_heap),
    );
    assert!(
        page_heap < BACKGROUND_HEAP_BUDGET,
        "heap with one scale cluster exceeds budget: {page_heap}"
    );
    drop(commands);
    drop(profiler);
}
