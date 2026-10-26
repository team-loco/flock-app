use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use criterion::Criterion;
use flock_bench::live::{SESSION_TIMEOUT, Session, bench_scroll, bench_snapshots, viewed_cluster};
use flock_bench::tables::{Table, Workload, query, workloads};
use flock_bench::{MEASUREMENT, SAMPLES, WARM_UP};
use flock_core::{Command, Page, Snapshot};
use gpui::{App, AppContext as _, BenchAppContext, Keystroke, point, px};

const SCROLL_TURNS: u64 = 60;
const SCROLL_STEP: f32 = 96.;
const TABLE_X: f32 = 720.;
const TABLE_Y: f32 = 480.;

fn open(table: &Table, snapshot: Snapshot, cx: &mut BenchAppContext) -> Session {
    let session = Session::open(table.screen, snapshot, cx);
    if page(&session, cx) != table.page {
        session.dispatch(Command::SelectPage(table.page), cx);
    }
    assert_eq!(page(&session, cx), table.page, "{}", table.name);
    session
}

fn page(session: &Session, cx: &BenchAppContext) -> Page {
    cx.read(|app| session.shell.read(app).state().location().page)
}

fn matches(session: &Session, cx: &App) -> Option<(String, (usize, usize))> {
    let bar = session.shell.read(cx).filter_bar(cx)?;
    let bar = bar.read(cx);
    Some((bar.text(cx), bar.matches()?))
}

#[gpui::bench(inputs = workloads(), input_name = "rows", group = "Snapshot apply")]
fn snapshot_apply(workload: &Workload, cx: &mut BenchAppContext) {
    let table = &workload.table;
    let cluster = viewed_cluster();
    let base = table.snapshot(&cluster, workload.rows);
    assert_eq!(table.count(&base, &cluster), workload.rows);
    let session = open(table, base.clone(), cx);
    let applied = bench_snapshots(
        &session,
        |generation| {
            let mut next = base.clone();
            assert!(
                table.touch(&mut next, &cluster, generation),
                "no row to touch"
            );
            (next, ())
        },
        |_, ()| true,
        cx,
    );
    assert!(applied > 0);
}

#[gpui::bench(inputs = workloads(), input_name = "rows", group = "Table scroll")]
fn table_scroll(workload: &Workload, cx: &mut BenchAppContext) {
    let table = &workload.table;
    let session = open(table, table.snapshot(&viewed_cluster(), workload.rows), cx);
    let shown = cx.read(|app| matches(&session, app));
    let all = Some((String::new(), (workload.rows, workload.rows)));
    assert_eq!(shown, all, "{}", table.name);
    let position = point(px(TABLE_X), px(TABLE_Y));
    let step = |turn: u64| {
        let step = if turn < SCROLL_TURNS / 2 {
            -SCROLL_STEP
        } else {
            SCROLL_STEP
        };
        point(px(0.), px(step))
    };
    assert!(bench_scroll(&session, position, SCROLL_TURNS, step, cx) > 0);
}

struct Typing {
    stop: Arc<AtomicBool>,
}

#[gpui::bench(inputs = workloads(), input_name = "rows", group = "Filter typing")]
fn filter_typing(workload: &Workload, cx: &mut BenchAppContext) {
    let table = &workload.table;
    let session = open(table, table.snapshot(&viewed_cluster(), workload.rows), cx);
    let typed = Rc::new(Cell::new(0_u64));
    let query = query();
    let keys: Vec<Keystroke> = query
        .chars()
        .filter_map(|key| Keystroke::parse(&key.to_string()).ok())
        .collect();
    assert_eq!(keys.len(), query.len());
    cx.bench_renderer_session(
        SESSION_TIMEOUT,
        |cx| {
            session.drain_commands();
            let cleared = cx.update_window(session.window, |_, window, cx| {
                let bar = session.shell.read(cx).filter_bar(cx)?;
                bar.update(cx, |bar, cx| {
                    bar.clear(window, cx);
                    bar.focus(window, cx);
                });
                Some(())
            });
            assert_eq!(
                cleared.ok().flatten(),
                Some(()),
                "the {} filter bar",
                table.name
            );
            cx.run_until_idle();
            let stop = Arc::new(AtomicBool::new(false));
            let typing = Typing { stop: stop.clone() };
            (typing, session.window, stop)
        },
        |typing, turn, window, cx| {
            let key = usize::try_from(turn).ok().and_then(|ix| keys.get(ix));
            if let Some(key) = key {
                window.dispatch_keystroke(key.clone(), cx);
                return;
            }
            let Some((text, (shown, total))) = matches(&session, cx) else {
                return;
            };
            if text == query && shown > 0 && shown < total {
                assert_eq!(total, workload.rows);
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
    targets = snapshot_apply, table_scroll, filter_typing
}
gpui::bench_main!(benches);
