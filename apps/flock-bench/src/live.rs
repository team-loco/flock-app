use std::cell::Cell;
use std::collections::BTreeSet;
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use flock::{Preset, Screen, WINDOW_HEIGHT, WINDOW_WIDTH, bind_keys, root_view};
use flock_core::flux::Phase;
use flock_core::flux_graph::FluxGraph;
use flock_core::{
    ClusterId, Command, LogEvent, LogLevel, LogLine, LogSource, Receiver, Sender, Sink, Snapshot,
    SourceId, Stamp, Uid, Update,
};
use flock_ui::kit::LogView;
use flock_ui::theme::Appearance;
use flock_ui::{Root, Shell, Store};
use gpui::{
    AnyWindowHandle, App, AppContext as _, BenchAppContext, Entity, Pixels, PlatformInput, Point,
    ScrollDelta, ScrollWheelEvent, SharedString, Window, point, px, size,
};

pub const SESSION_TIMEOUT: Duration = Duration::from_secs(60);
pub const LOG_RATES: [usize; 3] = [100, 1_000, 10_000];
pub const LOG_LINES: usize = 300;
pub const LOG_TICK: Duration = Duration::from_millis(20);
pub const LOG_PODS: usize = 6;
pub const CLUSTER_COUNTS: [usize; 3] = [3, 10, 30];
pub const FLUX_NODES: [usize; 3] = [50, 500, 2_000];
pub const FLUX_PAN_NODES: [usize; 3] = [50, 200, 500];
pub const TERMINAL_BYTES: [usize; 3] = [256 * 1024, 1024 * 1024, 4 * 1024 * 1024];
pub const TERMINAL_CHUNK: usize = 32 * 1024;
pub const STATUS_QUERY: &[u8] = b"\x1b[5n";
pub const STATUS_OK: &[u8] = b"\x1b[0n";

const MILLIS_PER_SECOND: u64 = 1_000;
const NANOS_PER_MILLI: i128 = 1_000_000;
const SECONDS_PER_MINUTE: u64 = 60;
const MINUTES_PER_HOUR: u64 = 60;
const HOURS_PER_DAY: u64 = 24;
const LINE_STEP_MS: u64 = 3;
const LATENCY_SPREAD_MS: u64 = 97;
const LATENCY_FLOOR_MS: u64 = 3;
const SENTINEL: &str = "flock-bench end of burst";
const LEVEL_CYCLE: [LogLevel; 8] = [
    LogLevel::Info,
    LogLevel::Info,
    LogLevel::Debug,
    LogLevel::Info,
    LogLevel::Warn,
    LogLevel::Info,
    LogLevel::Info,
    LogLevel::Error,
];
const ROUTES: [&str; 5] = [
    "/api/v1/orders",
    "/api/v1/cart/items",
    "/healthz",
    "/api/v1/payments/authorize",
    "/api/v1/catalog/search",
];
const POD_HASH: &str = "7d9f8b6c5";
const CONTAINER: &str = "app";
const CLONE_TAG: &str = "-clone-";
const FLUX_FANOUT: usize = 4;
const HALF: f32 = 0.5;
const PAN_TURNS: u64 = 20;
const PAN_STEP: f32 = 96.;

pub struct Load(pub usize);

impl fmt::Display for Load {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub fn loads(sizes: [usize; 3]) -> impl Iterator<Item = Load> {
    sizes.into_iter().map(Load)
}

pub struct Session {
    pub window: AnyWindowHandle,
    pub shell: Entity<Shell>,
    pub updates: Sender<Update>,
    commands: Receiver<Command>,
}

impl Session {
    pub fn open(screen: Screen, snapshot: Snapshot, cx: &mut BenchAppContext) -> Self {
        let initialised = cx.update(|cx| {
            let result = flock_ui::init(Some(Appearance::Dark), cx);
            bind_keys(cx);
            cx.set_reduce_motion(true);
            result
        });
        if let Err(error) = initialised {
            panic!("initialising Flock: {error}");
        }
        let (updates, update_rx) = async_channel::unbounded();
        let (command_tx, commands) = async_channel::unbounded();
        let first = Update::Snapshot(Arc::new(snapshot));
        assert!(updates.try_send(first).is_ok(), "the first snapshot");
        let mut bench_window = cx.add_empty_window();
        let window = bench_window.window_handle();
        let shell = bench_window.update(|window, cx| {
            window.resize(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)));
            let store = cx.new(|cx| Store::new(update_rx, command_tx, cx));
            let view = root_view(screen, &Preset::default(), store, window, cx);
            let shell = view.clone().downcast::<Shell>().ok();
            window.replace_root(cx, |window, cx| Root::new(view, window, cx));
            shell
        });
        let Some(shell) = shell else {
            panic!("{screen:?} is not a shell");
        };
        let session = Self {
            window,
            shell,
            updates,
            commands,
        };
        session.settle(cx);
        session
    }

    fn settle(&self, cx: &mut BenchAppContext) {
        cx.run_until(|| cx.read(|app| self.shell.read(app).settled()).then_some(()));
        cx.settle();
    }

    pub fn dispatch(&self, command: Command, cx: &mut BenchAppContext) {
        cx.update(|app| {
            self.shell
                .update(app, |shell, cx| shell.dispatch(command, cx));
        });
        self.settle(cx);
    }

    pub fn command<T>(&self, pick: impl Fn(Command) -> Option<T>) -> Option<T> {
        while let Ok(command) = self.commands.try_recv() {
            let command = match command {
                Command::ForWindow(_, inner) => *inner,
                other => other,
            };
            if let Some(found) = pick(command) {
                return Some(found);
            }
        }
        None
    }

    pub fn drain_commands(&self) {
        while self.commands.try_recv().is_ok() {}
    }

    pub fn log_view(&self, cx: &App) -> Option<Entity<LogView>> {
        self.shell.read(cx).log_view(cx)
    }

    pub fn last_log(&self, cx: &App) -> Option<SharedString> {
        let view = self.log_view(cx)?;
        let view = view.read(cx);
        let last = view.len().checked_sub(1)?;
        view.line(last).map(|line| line.message.clone())
    }

    pub fn wait_for_logs(&self, lines: usize, cx: &mut BenchAppContext) {
        cx.run_until(|| {
            let shown = cx.read(|app| self.log_view(app).map(|view| view.read(app).len()));
            (shown == Some(lines)).then_some(())
        });
        cx.settle();
    }
}

pub fn draws(window: &Window) -> u64 {
    window
        .frame_duration_snapshot()
        .draw_duration_histogram
        .len()
}

pub fn pace<T: Send + 'static>(
    sink: Sink<T>,
    chunks: Vec<Vec<T>>,
    tick: Duration,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let started = Instant::now();
        for (ix, chunk) in chunks.into_iter().enumerate() {
            let due = tick.saturating_mul(u32::try_from(ix).unwrap_or(u32::MAX));
            if let Some(wait) = due.checked_sub(started.elapsed()) {
                thread::sleep(wait);
            }
            for item in chunk {
                if !sink.send_blocking(item) {
                    return;
                }
            }
        }
    })
}

struct Stream<T> {
    chunks: Option<Vec<Vec<T>>>,
    feeder: Option<JoinHandle<()>>,
    generation: u64,
    stop: Arc<AtomicBool>,
}

pub fn bench_stream<T: Send + 'static>(
    session: &Session,
    sink: &Sink<T>,
    tick: Duration,
    mut chunks: impl FnMut(u64) -> Vec<Vec<T>>,
    done: impl Fn(&App, u64) -> bool,
    cx: &mut BenchAppContext,
) -> u64 {
    let generation = Cell::new(0);
    let finished = Rc::new(Cell::new(0_u64));
    cx.bench_renderer_session(
        SESSION_TIMEOUT,
        |_| {
            session.drain_commands();
            let stop = Arc::new(AtomicBool::new(false));
            let stream = Stream {
                chunks: Some(chunks(generation.get())),
                feeder: None,
                generation: generation.get(),
                stop: stop.clone(),
            };
            generation.set(generation.get() + 1);
            (stream, session.window, stop)
        },
        |stream, _, _, cx| {
            if let Some(chunks) = stream.chunks.take() {
                stream.feeder = Some(pace(sink.clone(), chunks, tick));
                return;
            }
            if done(cx, stream.generation) {
                stream.feeder.take();
                finished.set(finished.get() + 1);
                stream.stop.store(true, Ordering::Release);
            }
        },
    );
    assert_eq!(finished.get(), generation.get());
    finished.get()
}

struct Step<C> {
    next: Option<Arc<Snapshot>>,
    sent: Arc<Snapshot>,
    check: C,
    stop: Arc<AtomicBool>,
}

pub fn bench_snapshots<C>(
    session: &Session,
    mut next: impl FnMut(u64) -> (Snapshot, C),
    landed: impl Fn(&Snapshot, &C) -> bool,
    cx: &mut BenchAppContext,
) -> u64 {
    let generation = Cell::new(0);
    let applied = Rc::new(Cell::new(0_u64));
    cx.bench_renderer_session(
        SESSION_TIMEOUT,
        |_| {
            session.drain_commands();
            let stop = Arc::new(AtomicBool::new(false));
            let (snapshot, check) = next(generation.get());
            let sent = Arc::new(snapshot);
            let pending = Step {
                next: Some(sent.clone()),
                sent,
                check,
                stop: stop.clone(),
            };
            generation.set(generation.get() + 1);
            (pending, session.window, stop)
        },
        |step, _, _, cx| {
            if let Some(next) = step.next.take() {
                let sent = session.updates.try_send(Update::Snapshot(next));
                assert!(sent.is_ok(), "the store stopped listening");
                return;
            }
            let shown = &session.shell.read(cx).state().snapshot;
            if Arc::ptr_eq(shown, &step.sent) && landed(shown, &step.check) {
                applied.set(applied.get() + 1);
                step.stop.store(true, Ordering::Release);
            }
        },
    );
    assert_eq!(applied.get(), generation.get());
    applied.get()
}

struct Wheel {
    dispatched: u64,
    stop: Arc<AtomicBool>,
}

struct Finished {
    dispatched: u64,
    draws_at_last_step: u64,
}

pub fn bench_pan(session: &Session, cx: &mut BenchAppContext) -> u64 {
    bench_scroll(session, pane_center(), PAN_TURNS, pan_step, cx)
}

pub fn bench_scroll(
    session: &Session,
    position: Point<Pixels>,
    turns: u64,
    delta: impl Fn(u64) -> Point<Pixels>,
    cx: &mut BenchAppContext,
) -> u64 {
    let sessions = Rc::new(Cell::new(0_u64));
    let finished = Rc::new(Cell::new(None));
    cx.bench_renderer_session(
        SESSION_TIMEOUT,
        |_| {
            session.drain_commands();
            let stop = Arc::new(AtomicBool::new(false));
            let wheel = Wheel {
                dispatched: 0,
                stop: stop.clone(),
            };
            (wheel, session.window, stop)
        },
        |wheel, turn, window, cx| {
            let event = ScrollWheelEvent {
                position,
                delta: ScrollDelta::Pixels(delta(turn)),
                ..ScrollWheelEvent::default()
            };
            window.dispatch_event(PlatformInput::ScrollWheel(event), cx);
            wheel.dispatched += 1;
            if turn + 1 == turns {
                finished.set(Some(Finished {
                    dispatched: wheel.dispatched,
                    draws_at_last_step: draws(window),
                }));
                sessions.set(sessions.get() + 1);
                wheel.stop.store(true, Ordering::Release);
            }
        },
    );
    let Some(finished) = finished.take() else {
        panic!("the last wheel step never ran");
    };
    let Ok(drawn) = cx.update_window(session.window, |_, window, _| draws(window)) else {
        panic!("the bench window closed before the scroll finished");
    };
    assert_eq!(finished.dispatched, turns, "wheel steps were dropped");
    assert!(
        drawn > finished.draws_at_last_step,
        "no frame drew after the last wheel step"
    );
    sessions.get()
}

fn pane_center() -> Point<Pixels> {
    point(px(WINDOW_WIDTH * HALF), px(WINDOW_HEIGHT * HALF))
}

fn pan_step(turn: u64) -> Point<Pixels> {
    let step = if turn < PAN_TURNS / 2 {
        -PAN_STEP
    } else {
        PAN_STEP
    };
    point(px(step), px(step))
}

pub fn viewed_cluster() -> ClusterId {
    let Some(cluster) = crate::viewed_cluster() else {
        panic!("the sample screens open without a cluster");
    };
    cluster
}

pub fn sentinel(generation: u64) -> String {
    format!("{SENTINEL} {generation}")
}

pub fn is_sentinel(message: &str, generation: u64) -> bool {
    let shown = message
        .strip_prefix(SENTINEL)
        .and_then(|rest| rest.strip_prefix(' '))
        .and_then(|number| number.parse::<u64>().ok());
    shown == Some(generation)
}

fn clock(seq: u64) -> String {
    let ms = seq * LINE_STEP_MS;
    let seconds = ms / MILLIS_PER_SECOND;
    let minutes = seconds / SECONDS_PER_MINUTE;
    let hours = minutes / MINUTES_PER_HOUR;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        hours % HOURS_PER_DAY,
        minutes % MINUTES_PER_HOUR,
        seconds % SECONDS_PER_MINUTE,
        ms % MILLIS_PER_SECOND
    )
}

fn stamp(seq: u64) -> Stamp {
    Stamp(i128::from(seq * LINE_STEP_MS) * NANOS_PER_MILLI)
}

pub fn log_line(seq: u64) -> LogLine {
    let ix = usize::try_from(seq).unwrap_or_default();
    let level = LEVEL_CYCLE[ix % LEVEL_CYCLE.len()];
    let route = ROUTES[ix % ROUTES.len()];
    let latency = seq % LATENCY_SPREAD_MS + LATENCY_FLOOR_MS;
    let message = format!(
        "{{\"msg\":\"request handled\",\"method\":\"GET\",\"path\":\"{route}\",\"status\":200,\"latency_ms\":{latency},\"trace_id\":\"{seq:032x}\",\"seq\":{seq}}}"
    );
    LogLine::new(&clock(seq), level, message)
}

fn sentinel_line(seq: u64, generation: u64) -> LogLine {
    LogLine::new(&clock(seq), LogLevel::Info, sentinel(generation))
}

fn lines_per_tick(rate: usize, tick: Duration) -> usize {
    let per_tick = rate.saturating_mul(usize::try_from(tick.as_millis()).unwrap_or(usize::MAX));
    (per_tick / usize::try_from(MILLIS_PER_SECOND).unwrap_or(1)).max(1)
}

fn first_seq(prefill: usize, lines: usize, generation: u64) -> u64 {
    let prefill = u64::try_from(prefill).unwrap_or(u64::MAX);
    let lines = u64::try_from(lines).unwrap_or(u64::MAX);
    prefill + generation * (lines + 1)
}

pub fn prefill_lines(count: usize) -> Vec<LogLine> {
    (0..u64::try_from(count).unwrap_or_default())
        .map(log_line)
        .collect()
}

pub fn pod_burst(prefill: usize, rate: usize, lines: usize, generation: u64) -> Vec<Vec<LogLine>> {
    let start = first_seq(prefill, lines, generation);
    let end = start + u64::try_from(lines).unwrap_or_default();
    let all: Vec<LogLine> = (start..end).map(log_line).collect();
    let mut chunks: Vec<Vec<LogLine>> = all
        .chunks(lines_per_tick(rate, LOG_TICK))
        .map(<[LogLine]>::to_vec)
        .collect();
    chunks.push(vec![sentinel_line(end, generation)]);
    chunks
}

pub fn log_sources(workload: &str, pods: usize) -> Vec<LogEvent> {
    (0..pods)
        .map(|ix| {
            let source = LogSource {
                namespace: String::new(),
                pod: format!("{workload}-{POD_HASH}-{ix:05}"),
                container: CONTAINER.into(),
            };
            LogEvent::Joined(source_id(ix), source)
        })
        .collect()
}

fn source_id(ix: usize) -> SourceId {
    SourceId(u32::try_from(ix).unwrap_or(u32::MAX))
}

fn multi_event(seq: u64, pods: usize) -> (usize, LogEvent) {
    let pod = usize::try_from(seq).unwrap_or_default() % pods.max(1);
    let event = LogEvent::Line(source_id(pod), Some(stamp(seq)), log_line(seq));
    (pod, event)
}

pub fn multi_prefill(count: usize, pods: usize) -> Vec<LogEvent> {
    (0..u64::try_from(count).unwrap_or_default())
        .map(|seq| multi_event(seq, pods).1)
        .collect()
}

pub fn multi_burst(
    prefill: usize,
    rate: usize,
    lines: usize,
    pods: usize,
    generation: u64,
) -> Vec<Vec<LogEvent>> {
    let start = first_seq(prefill, lines, generation);
    let end = start + u64::try_from(lines).unwrap_or_default();
    let events: Vec<(usize, LogEvent)> = (start..end).map(|seq| multi_event(seq, pods)).collect();
    let mut chunks: Vec<Vec<LogEvent>> = events
        .chunks(lines_per_tick(rate, LOG_TICK))
        .map(|tick| {
            let mut tick = tick.to_vec();
            tick.sort_by_key(|(pod, _)| *pod);
            tick.into_iter().map(|(_, event)| event).collect()
        })
        .collect();
    let last = LogEvent::Line(
        source_id(0),
        Some(stamp(end)),
        sentinel_line(end, generation),
    );
    chunks.push(vec![last]);
    chunks
}

pub fn cat_output(bytes: usize) -> Vec<Vec<u8>> {
    let mut output = Vec::with_capacity(bytes + STATUS_QUERY.len());
    let mut seq = 0_u64;
    while output.len() < bytes {
        let line = log_line(seq);
        let text = format!("{} {}\r\n", line.time.unwrap_or_default(), line.message);
        output.extend_from_slice(text.as_bytes());
        seq += 1;
    }
    output.truncate(bytes);
    output.extend_from_slice(STATUS_QUERY);
    output.chunks(TERMINAL_CHUNK).map(<[u8]>::to_vec).collect()
}

pub fn fleet(cluster: &ClusterId, clusters: usize) -> Snapshot {
    let mut snapshot = flock_demo::snapshot();
    let demo: Vec<ClusterId> = snapshot.clusters.iter().map(|c| c.id.clone()).collect();
    let kept: BTreeSet<ClusterId> = std::iter::once(cluster.clone())
        .chain(demo.iter().filter(|id| *id != cluster).cloned())
        .take(clusters)
        .collect();
    retain_clusters(&mut snapshot, &kept);
    let templates: Vec<ClusterId> = demo
        .iter()
        .filter(|id| snapshot.details.contains_key(*id))
        .cloned()
        .collect();
    let missing = clusters.saturating_sub(snapshot.clusters.len());
    for ix in 0..missing {
        let template = &templates[ix % templates.len()];
        let id = ClusterId::new(format!("{template}{CLONE_TAG}{ix:02}"));
        clone_cluster(&mut snapshot, template, &id);
    }
    snapshot
}

fn retain_clusters(snapshot: &mut Snapshot, kept: &BTreeSet<ClusterId>) {
    snapshot.clusters.retain(|c| kept.contains(&c.id));
    snapshot.details.retain(|id, _| kept.contains(id));
    snapshot.pages.retain(|id, _| kept.contains(id));
    snapshot.pods.retain(|pod| kept.contains(&pod.cluster));
    snapshot
        .catalog
        .clusters
        .retain(|settings| kept.contains(&settings.cluster));
}

fn clone_cluster(snapshot: &mut Snapshot, template: &ClusterId, id: &ClusterId) {
    if let Some(mut summary) = snapshot.cluster(template).cloned() {
        summary.id = id.clone();
        summary.name = id.to_string();
        snapshot.clusters.push(summary);
    }
    if let Some(detail) = snapshot.details.get(template).cloned() {
        snapshot.details.insert(id.clone(), detail);
    }
    if let Some(pages) = snapshot.pages.get(template).cloned() {
        snapshot.pages.insert(id.clone(), pages);
    }
    let settings = snapshot
        .catalog
        .clusters
        .iter()
        .find(|settings| &settings.cluster == template)
        .cloned();
    if let Some(mut settings) = settings {
        settings.cluster = id.clone();
        snapshot.catalog.clusters.push(settings);
    }
    let pods: Vec<_> = snapshot
        .pods
        .iter()
        .filter(|pod| &pod.cluster == template)
        .map(|pod| {
            let mut pod = pod.clone();
            pod.cluster = id.clone();
            pod
        })
        .collect();
    snapshot.pods.extend(pods);
}

pub fn bump_running(snapshot: &mut Snapshot, cluster: &ClusterId, generation: u64) -> Option<u32> {
    let detail = snapshot.details.get_mut(cluster)?;
    let bumped = u32::try_from(generation % u64::from(u32::MAX)).ok()? + 1;
    detail.overview.pods.running += bumped;
    Some(detail.overview.pods.running)
}

pub fn running(snapshot: &Snapshot, cluster: &ClusterId) -> Option<u32> {
    snapshot
        .details
        .get(cluster)
        .map(|detail| detail.overview.pods.running)
}

pub fn flux_graph(cluster: &ClusterId, nodes: usize) -> Snapshot {
    let mut snapshot = flock_demo::snapshot();
    let templates: Vec<_> = snapshot
        .flux
        .iter()
        .filter(|object| &object.key.cluster == cluster && object.source.is_some())
        .cloned()
        .collect();
    let base = FluxGraph::build(cluster, &snapshot.flux).nodes.len();
    let mut clones: Vec<flock_core::FluxKey> = Vec::new();
    for ix in 0..nodes.saturating_sub(base) {
        let mut object = templates[ix % templates.len()].clone();
        object.key.name = format!("{}{CLONE_TAG}{ix:04}", object.key.name);
        object.uid = Uid::new(format!("{cluster}/{}", object.key.name));
        if let Some(parent) = (ix / FLUX_FANOUT)
            .checked_sub(1)
            .and_then(|p| clones.get(p))
        {
            object.depends_on = vec![parent.clone()];
        }
        clones.push(object.key.clone());
        snapshot.flux.push(object);
    }
    snapshot
}

pub fn flux_clone(snapshot: &Snapshot, cluster: &ClusterId, generation: u64) -> Option<usize> {
    let clones: Vec<usize> = snapshot
        .flux
        .iter()
        .enumerate()
        .filter(|(_, object)| &object.key.cluster == cluster && object.key.name.contains(CLONE_TAG))
        .map(|(ix, _)| ix)
        .collect();
    let count = u64::try_from(clones.len()).ok()?;
    let pick = usize::try_from(generation.checked_rem(count)?).ok()?;
    clones.get(pick).copied()
}

pub fn flip_phase(snapshot: &mut Snapshot, ix: usize) -> Option<Phase> {
    let object = snapshot.flux.get_mut(ix)?;
    object.phase = if object.phase == Phase::Reconciling {
        Phase::Ready
    } else {
        Phase::Reconciling
    };
    Some(object.phase)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cluster() -> ClusterId {
        viewed_cluster()
    }

    #[test]
    fn a_pod_burst_paces_its_lines_and_ends_with_the_generation_sentinel() {
        let chunks = pod_burst(10, 1_000, 100, 2);
        let lines: Vec<&LogLine> = chunks.iter().flatten().collect();
        let per_tick = lines_per_tick(1_000, LOG_TICK);
        assert_eq!(per_tick, 20);
        assert_eq!(chunks.len(), 100 / per_tick + 1);
        assert_eq!(lines.len(), 101);
        assert_eq!(
            lines.first().map(|l| l.message.contains("\"seq\":212")),
            Some(true)
        );
        assert_eq!(lines.last().map(|l| l.message.clone()), Some(sentinel(2)));
        assert!(is_sentinel(&sentinel(2), 2));
        assert!(!is_sentinel(&sentinel(2), 1));
        assert!(!is_sentinel(&sentinel(12), 1));
        assert!(!is_sentinel("2", 2));
        assert_eq!(lines_per_tick(1, LOG_TICK), 1);
    }

    #[test]
    fn a_multi_burst_groups_each_tick_by_pod_and_ends_on_the_newest_stamp() {
        let chunks = multi_burst(0, 10_000, 400, LOG_PODS, 0);
        let stamps = |chunk: &[LogEvent]| -> Vec<(u32, Stamp)> {
            chunk
                .iter()
                .filter_map(|event| match event {
                    LogEvent::Line(id, Some(at), _) => Some((id.0, *at)),
                    _ => None,
                })
                .collect()
        };
        let first = stamps(&chunks[0]);
        assert_eq!(first.len(), 200);
        assert!(first.windows(2).all(|pair| pair[0].0 <= pair[1].0));
        assert!(first.windows(2).any(|pair| pair[0].1 > pair[1].1));
        let Some(newest) = chunks
            .iter()
            .flat_map(|c| stamps(c))
            .map(|(_, at)| at)
            .max()
        else {
            panic!("no stamps");
        };
        let last = chunks.last().map(|chunk| stamps(chunk));
        assert_eq!(last, Some(vec![(0, newest)]));
        assert_eq!(log_sources("checkout", LOG_PODS).len(), LOG_PODS);
    }

    #[test]
    fn cat_output_is_the_requested_size_then_a_status_query() {
        let chunks = cat_output(100_000);
        let bytes: Vec<u8> = chunks.concat();
        assert_eq!(bytes.len(), 100_000 + STATUS_QUERY.len());
        assert!(bytes.ends_with(STATUS_QUERY));
        assert!(chunks.iter().all(|chunk| chunk.len() <= TERMINAL_CHUNK));
    }

    #[test]
    fn the_fleet_holds_exactly_the_requested_unique_clusters_with_details() {
        let cluster = cluster();
        for count in CLUSTER_COUNTS {
            let snapshot = fleet(&cluster, count);
            let ids: BTreeSet<&ClusterId> = snapshot.clusters.iter().map(|c| &c.id).collect();
            assert_eq!(snapshot.clusters.len(), count);
            assert_eq!(ids.len(), count);
            assert!(ids.contains(&cluster));
            assert!(snapshot.details.keys().all(|id| ids.contains(id)));
            assert!(snapshot.pods.iter().all(|pod| ids.contains(&pod.cluster)));
            let pods: BTreeSet<(&ClusterId, &str, &str)> = snapshot
                .pods
                .iter()
                .map(|pod| (&pod.cluster, pod.namespace.as_str(), pod.name.as_str()))
                .collect();
            assert_eq!(pods.len(), snapshot.pods.len());
        }
    }

    #[test]
    fn bumping_changes_only_the_viewed_clusters_running_pods() {
        let cluster = cluster();
        let base = fleet(&cluster, 3);
        let mut next = base.clone();
        let bumped = bump_running(&mut next, &cluster, 4);
        assert_eq!(bumped, running(&next, &cluster));
        assert_eq!(bumped, running(&base, &cluster).map(|r| r + 5));
        next.details.remove(&cluster);
        let mut others = base;
        others.details.remove(&cluster);
        assert_eq!(next, others);
    }

    #[test]
    fn the_flux_graph_has_exactly_the_requested_nodes() {
        let cluster = cluster();
        for nodes in FLUX_NODES {
            let snapshot = flux_graph(&cluster, nodes);
            let graph = FluxGraph::build(&cluster, &snapshot.flux);
            assert_eq!(graph.nodes.len(), nodes);
            assert!(graph.layers.len() > 2);
        }
    }

    #[test]
    fn flipping_a_clone_toggles_its_phase_between_generations() {
        let cluster = cluster();
        let mut snapshot = flux_graph(&cluster, 50);
        let Some(first) = flux_clone(&snapshot, &cluster, 0) else {
            panic!("no clone");
        };
        let second = flux_clone(&snapshot, &cluster, 1);
        assert_ne!(Some(first), second);
        let before = snapshot.flux[first].phase;
        let flipped = flip_phase(&mut snapshot, first);
        assert_ne!(flipped, Some(before));
        assert!(snapshot.flux[first].key.name.contains(CLONE_TAG));
        assert_ne!(flip_phase(&mut snapshot, first), flipped);
    }
}
