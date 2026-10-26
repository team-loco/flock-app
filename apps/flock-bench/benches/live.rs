use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use criterion::Criterion;
use flock::Screen;
use flock_bench::live::{
    CLUSTER_COUNTS, FLUX_NODES, FLUX_PAN_NODES, LOG_LINES, LOG_PODS, LOG_RATES, LOG_TICK, Load,
    STATUS_OK, Session, TERMINAL_BYTES, bench_pan, bench_snapshots, bench_stream, bump_running,
    cat_output, fleet, flip_phase, flux_clone, flux_graph, loads, log_sources, multi_burst,
    multi_prefill, pod_burst, prefill_lines, running, sentinel, viewed_cluster,
};
use flock_bench::{MEASUREMENT, SAMPLES, WARM_UP};
use flock_core::multilog::MERGED_LINES;
use flock_core::{Command, ExecEvent, LogTarget, Page, ResourceRef};
use flock_ui::kit::LOG_CAPACITY;
use gpui::{AppContext as _, BenchAppContext};

const POD_KIND: &str = "Pod";
const SHELL_NOTE: &str = "flock-bench shell";

fn sentinel_shown(session: &Session, cx: &gpui::App, generation: u64) -> bool {
    let expected = sentinel(generation);
    session
        .last_log(cx)
        .is_some_and(|last| last.as_ref() == expected)
}

fn assert_capped(session: &Session, cx: &BenchAppContext, capacity: usize) {
    let shown = cx.read(|app| session.log_view(app).map(|view| view.read(app).len()));
    assert_eq!(
        shown,
        Some(capacity),
        "the log pane keeps at most {capacity} lines"
    );
}

#[gpui::bench(inputs = loads(LOG_RATES), input_name = "lines_per_s", group = "Pod log follow")]
fn pod_log_follow(rate: &Load, cx: &mut BenchAppContext) {
    let cluster = viewed_cluster();
    let session = Session::open(Screen::Pods, flock_demo::snapshot(), cx);
    let pod = cx.read(|app| {
        let snapshot = &session.shell.read(app).state().snapshot;
        snapshot
            .pods
            .iter()
            .find(|pod| pod.cluster == cluster && !pod.containers.is_empty())
            .cloned()
    });
    let Some(pod) = pod else {
        panic!("no pod with containers on {cluster}");
    };
    let container = pod.containers[0].name.clone();
    let target = ResourceRef {
        cluster: cluster.clone(),
        kind: POD_KIND.into(),
        namespace: Some(pod.namespace.clone()),
        name: pod.name.clone(),
    };
    session.dispatch(
        Command::OpenLogs {
            pod: target,
            container,
        },
        cx,
    );
    let location = cx.read(|app| session.shell.read(app).state().location().clone());
    assert_eq!(location.page, Page::Logs);
    let request = session.command(|command| match command {
        Command::StreamLogs(request) => Some(request),
        _ => None,
    });
    let Some(request) = request else {
        panic!("the logs pane did not ask for a stream");
    };
    for line in prefill_lines(LOG_CAPACITY) {
        assert!(
            request.sink.send_blocking(line),
            "the logs pane stopped listening"
        );
    }
    session.wait_for_logs(LOG_CAPACITY, cx);
    bench_stream(
        &session,
        &request.sink,
        LOG_TICK,
        |generation| pod_burst(LOG_CAPACITY, rate.0, LOG_LINES, generation),
        |app, generation| sentinel_shown(&session, app, generation),
        cx,
    );
    assert_capped(&session, cx, LOG_CAPACITY);
}

#[gpui::bench(inputs = loads(LOG_RATES), input_name = "lines_per_s", group = "Multi-pod log follow")]
fn multi_log_follow(rate: &Load, cx: &mut BenchAppContext) {
    let session = Session::open(Screen::Logs, flock_demo::snapshot(), cx);
    let request = session.command(|command| match command {
        Command::StreamPodLogs(request) => Some(request),
        _ => None,
    });
    let Some(request) = request else {
        panic!("the multi-pod logs pane did not ask for a stream");
    };
    let workload = match &request.target {
        LogTarget::Workload(workload) => workload.name.clone(),
        other => panic!("unexpected log target {other:?}"),
    };
    let joined = log_sources(&workload, LOG_PODS);
    let prefill = multi_prefill(MERGED_LINES, LOG_PODS);
    for event in joined.into_iter().chain(prefill) {
        assert!(
            request.sink.send_blocking(event),
            "the pane stopped listening"
        );
    }
    session.wait_for_logs(MERGED_LINES, cx);
    bench_stream(
        &session,
        &request.sink,
        LOG_TICK,
        |generation| multi_burst(MERGED_LINES, rate.0, LOG_LINES, LOG_PODS, generation),
        |app, generation| sentinel_shown(&session, app, generation),
        cx,
    );
    assert_capped(&session, cx, MERGED_LINES);
}

#[gpui::bench(inputs = loads(TERMINAL_BYTES), input_name = "bytes", group = "Terminal output burst")]
fn terminal_burst(bytes: &Load, cx: &mut BenchAppContext) {
    let session = Session::open(Screen::PodsShell, flock_demo::snapshot(), cx);
    let opened = cx.update_window(session.window, |_, window, cx| {
        session
            .shell
            .update(cx, |shell, cx| shell.show_pod_terminal(window, cx));
    });
    assert!(opened.is_ok(), "the session window closed");
    cx.settle();
    let request = session.command(|command| match command {
        Command::Exec(request) => Some(request),
        _ => None,
    });
    let Some(request) = request else {
        panic!("the pod terminal did not start a shell");
    };
    let started = ExecEvent::Started {
        note: SHELL_NOTE.into(),
    };
    assert!(
        request.output.send_blocking(started),
        "the terminal stopped listening"
    );
    let replies = Arc::new(AtomicU64::new(0));
    let counted = replies.clone();
    let input = request.input.clone();
    thread::spawn(move || {
        while let Some(bytes) = input.recv_blocking() {
            let answered = bytes.windows(STATUS_OK.len()).any(|w| w == STATUS_OK);
            if answered {
                counted.fetch_add(1, Ordering::AcqRel);
            }
        }
    });
    let output = cat_output(bytes.0);
    let bursts = bench_stream(
        &session,
        &request.output,
        Duration::ZERO,
        |_| {
            output
                .iter()
                .map(|chunk| vec![ExecEvent::Output(chunk.clone())])
                .collect()
        },
        |_, generation| replies.load(Ordering::Acquire) > generation,
        cx,
    );
    assert_eq!(replies.load(Ordering::Acquire), bursts);
}

#[gpui::bench(inputs = loads(CLUSTER_COUNTS), input_name = "clusters", group = "Overview snapshot apply")]
fn overview_apply(clusters: &Load, cx: &mut BenchAppContext) {
    let cluster = viewed_cluster();
    let base = fleet(&cluster, clusters.0);
    assert_eq!(base.clusters.len(), clusters.0);
    let session = Session::open(Screen::Overview, base.clone(), cx);
    let location = cx.read(|app| session.shell.read(app).state().location().clone());
    assert_eq!(location.page, Page::ClusterOverview);
    bench_snapshots(
        &session,
        |generation| {
            let mut next = base.clone();
            let shown = bump_running(&mut next, &cluster, generation);
            assert!(shown.is_some(), "the viewed cluster has no detail");
            (next, shown)
        },
        |snapshot, shown| running(snapshot, &cluster) == *shown,
        cx,
    );
}

#[gpui::bench(inputs = loads(FLUX_NODES), input_name = "nodes", group = "Flux graph apply")]
fn flux_graph_apply(nodes: &Load, cx: &mut BenchAppContext) {
    let cluster = viewed_cluster();
    let base = flux_graph(&cluster, nodes.0);
    let session = Session::open(Screen::FluxGraph, base.clone(), cx);
    let location = cx.read(|app| session.shell.read(app).state().location().clone());
    assert_eq!(location.page, Page::FluxGraph);
    bench_snapshots(
        &session,
        |generation| {
            let mut next = base.clone();
            let Some(ix) = flux_clone(&next, &cluster, generation) else {
                panic!("no generated flux object");
            };
            let phase = flip_phase(&mut next, ix);
            (next, (ix, phase))
        },
        |snapshot, (ix, phase)| snapshot.flux.get(*ix).map(|object| object.phase) == *phase,
        cx,
    );
}

#[gpui::bench(inputs = loads(FLUX_PAN_NODES), input_name = "nodes", group = "Flux graph pan")]
fn flux_graph_pan(nodes: &Load, cx: &mut BenchAppContext) {
    let cluster = viewed_cluster();
    let session = Session::open(Screen::FluxGraph, flux_graph(&cluster, nodes.0), cx);
    let location = cx.read(|app| session.shell.read(app).state().location().clone());
    assert_eq!(location.page, Page::FluxGraph);
    assert!(bench_pan(&session, cx) > 0);
}

gpui::bench_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(SAMPLES)
        .warm_up_time(WARM_UP)
        .measurement_time(MEASUREMENT);
    targets = pod_log_follow, multi_log_follow, terminal_burst, overview_apply, flux_graph_apply, flux_graph_pan
}
gpui::bench_main!(benches);
