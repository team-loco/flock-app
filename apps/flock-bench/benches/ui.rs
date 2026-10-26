use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use criterion::Criterion;
use flock::{Preset, Screen, WINDOW_HEIGHT, WINDOW_WIDTH, bind_keys, root_view};
use flock_bench::{Workload, age_one_pod, scaled_snapshot, viewed_cluster, workloads};
use flock_core::{ClusterId, Command, Receiver, Sender, Snapshot, Update};
use flock_ui::theme::Appearance;
use flock_ui::{Root, Shell, Store};
use gpui::{
    AnyWindowHandle, App, AppContext as _, BenchAppContext, Entity, Keystroke, PlatformInput,
    ScrollDelta, ScrollWheelEvent, Window, point, px, size,
};

const SAMPLES: usize = 10;
const WARM_UP: Duration = Duration::from_secs(1);
const MEASUREMENT: Duration = Duration::from_secs(3);
const SESSION_TIMEOUT: Duration = Duration::from_secs(60);
const SCROLL_TURNS: u64 = 60;
const SCROLL_STEP: f32 = 96.;
const TABLE_X: f32 = 720.;
const TABLE_Y: f32 = 480.;
const QUERY: &str = "checkout";

struct Flock {
    window: AnyWindowHandle,
    shell: Entity<Shell>,
    updates: Sender<Update>,
    commands: Receiver<Command>,
}

impl Flock {
    fn open(snapshot: Snapshot, cx: &mut BenchAppContext) -> Self {
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
            let view = root_view(Screen::Pods, &Preset::default(), store, window, cx);
            let shell = view.clone().downcast::<Shell>().ok();
            window.replace_root(cx, |window, cx| Root::new(view, window, cx));
            shell
        });
        let Some(shell) = shell else {
            panic!("the Pods screen is not a shell");
        };
        cx.run_until(|| cx.read(|app| shell.read(app).settled()).then_some(()));
        cx.settle();
        let flock = Self {
            window,
            shell,
            updates,
            commands,
        };
        flock.assert_pods_page(cx);
        flock
    }

    fn assert_pods_page(&self, cx: &BenchAppContext) {
        let location = cx.read(|app| self.shell.read(app).state().location().clone());
        assert_eq!(location.page, flock_core::Page::Pods);
        assert_eq!(location.cluster, Some(cluster()));
    }

    fn drain_commands(&self) {
        while self.commands.try_recv().is_ok() {}
    }

    fn matches(&self, cx: &App) -> Option<(String, (usize, usize))> {
        let bar = self.shell.read(cx).filter_bar(cx)?;
        let bar = bar.read(cx);
        Some((bar.text(cx), bar.matches()?))
    }
}

fn draws(window: &Window) -> u64 {
    window
        .frame_duration_snapshot()
        .draw_duration_histogram
        .len()
}

fn cluster() -> ClusterId {
    let Some(cluster) = viewed_cluster() else {
        panic!("the Pods screen opens without a cluster");
    };
    cluster
}

fn viewed_pods(snapshot: &Snapshot) -> usize {
    let cluster = cluster();
    snapshot
        .pods
        .iter()
        .filter(|pod| pod.cluster == cluster)
        .count()
}

struct Burst {
    next: Option<Snapshot>,
    pod: usize,
    age: Duration,
    total: usize,
    updates: Sender<Update>,
    shell: Entity<Shell>,
    stop: Arc<AtomicBool>,
}

#[gpui::bench(inputs = workloads(), input_name = "pods", group = "Snapshot apply")]
fn snapshot_apply(workload: &Workload, cx: &mut BenchAppContext) {
    let base = scaled_snapshot(&cluster(), workload.pods);
    assert_eq!(viewed_pods(&base), workload.pods);
    let total = base.pods.len();
    let flock = Flock::open(base.clone(), cx);
    let generation = Cell::new(0);
    let applied = Rc::new(Cell::new(0_u64));
    cx.bench_renderer_session(
        SESSION_TIMEOUT,
        |_| {
            flock.drain_commands();
            let mut next = base.clone();
            let Some(aged) = age_one_pod(&mut next, generation.get()) else {
                panic!("no pod to age");
            };
            generation.set(generation.get() + 1);
            let stop = Arc::new(AtomicBool::new(false));
            let burst = Burst {
                next: Some(next),
                pod: aged.pod,
                age: aged.age,
                total,
                updates: flock.updates.clone(),
                shell: flock.shell.clone(),
                stop: stop.clone(),
            };
            (burst, flock.window, stop)
        },
        |burst, _, _, cx| {
            if let Some(next) = burst.next.take() {
                let sent = burst.updates.try_send(Update::Snapshot(Arc::new(next)));
                assert!(sent.is_ok(), "the store stopped listening");
                return;
            }
            let pods = &burst.shell.read(cx).state().snapshot.pods;
            let landed = pods.get(burst.pod).is_some_and(|pod| pod.age == burst.age);
            if landed {
                assert_eq!(pods.len(), burst.total);
                applied.set(applied.get() + 1);
                burst.stop.store(true, Ordering::Release);
            }
        },
    );
    assert_eq!(applied.get(), generation.get());
}

struct Scroll {
    first_draw: u64,
    stop: Arc<AtomicBool>,
}

#[gpui::bench(inputs = workloads(), input_name = "pods", group = "Pods table scroll")]
fn pods_scroll(workload: &Workload, cx: &mut BenchAppContext) {
    let flock = Flock::open(scaled_snapshot(&cluster(), workload.pods), cx);
    let shown = cx.read(|app| flock.matches(app));
    assert_eq!(shown, Some((String::new(), (workload.pods, workload.pods))));
    let sessions = Rc::new(Cell::new(0_u64));
    cx.bench_renderer_session(
        SESSION_TIMEOUT,
        |cx| {
            flock.drain_commands();
            let first_draw = cx
                .update_window(flock.window, |_, window, _| draws(window))
                .unwrap_or_default();
            let stop = Arc::new(AtomicBool::new(false));
            let scroll = Scroll {
                first_draw,
                stop: stop.clone(),
            };
            (scroll, flock.window, stop)
        },
        |scroll, turn, window, cx| {
            let step = if turn < SCROLL_TURNS / 2 {
                -SCROLL_STEP
            } else {
                SCROLL_STEP
            };
            let wheel = ScrollWheelEvent {
                position: point(px(TABLE_X), px(TABLE_Y)),
                delta: ScrollDelta::Pixels(point(px(0.), px(step))),
                ..ScrollWheelEvent::default()
            };
            window.dispatch_event(PlatformInput::ScrollWheel(wheel), cx);
            if turn + 1 == SCROLL_TURNS {
                let drawn = draws(window) - scroll.first_draw;
                assert!(drawn >= turn, "{drawn} frames for {turn} scroll steps");
                sessions.set(sessions.get() + 1);
                scroll.stop.store(true, Ordering::Release);
            }
        },
    );
    assert!(sessions.get() > 0);
}

struct Typing {
    stop: Arc<AtomicBool>,
}

#[gpui::bench(inputs = workloads(), input_name = "pods", group = "Pods filter typing")]
fn pods_filter_typing(workload: &Workload, cx: &mut BenchAppContext) {
    let flock = Flock::open(scaled_snapshot(&cluster(), workload.pods), cx);
    let typed = Rc::new(Cell::new(0_u64));
    let keys: Vec<Keystroke> = QUERY
        .chars()
        .filter_map(|key| Keystroke::parse(&key.to_string()).ok())
        .collect();
    assert_eq!(keys.len(), QUERY.len());
    cx.bench_renderer_session(
        SESSION_TIMEOUT,
        |cx| {
            flock.drain_commands();
            let cleared = cx.update_window(flock.window, |_, window, cx| {
                let bar = flock.shell.read(cx).filter_bar(cx)?;
                bar.update(cx, |bar, cx| {
                    bar.clear(window, cx);
                    bar.focus(window, cx);
                });
                Some(())
            });
            assert_eq!(cleared.ok().flatten(), Some(()), "the Pods filter bar");
            cx.run_until_idle();
            let stop = Arc::new(AtomicBool::new(false));
            let typing = Typing { stop: stop.clone() };
            (typing, flock.window, stop)
        },
        |typing, turn, window, cx| {
            let key = usize::try_from(turn).ok().and_then(|ix| keys.get(ix));
            if let Some(key) = key {
                window.dispatch_keystroke(key.clone(), cx);
                return;
            }
            let Some((text, (shown, total))) = flock.matches(cx) else {
                return;
            };
            if text == QUERY && shown > 0 && shown < total {
                assert_eq!(total, workload.pods);
                typed.set(typed.get() + 1);
                typing.stop.store(true, Ordering::Release);
            }
        },
    );
    assert!(typed.get() > 0);
}

gpui::bench_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(SAMPLES)
        .warm_up_time(WARM_UP)
        .measurement_time(MEASUREMENT);
    targets = snapshot_apply, pods_scroll, pods_filter_typing
}
gpui::bench_main!(benches);
