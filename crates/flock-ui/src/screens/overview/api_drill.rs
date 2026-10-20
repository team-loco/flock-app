use flock_core::Status;
use flock_core::control_plane::{ControlPlane, Probe, Traffic, grouped, latency};
use gpui::{
    AnyElement, Context, Div, InteractiveElement, IntoElement, ParentElement, Styled, div,
    prelude::FluentBuilder, relative,
};

use super::{OverviewScreen, mono};
use crate::kit::{IconButton, Inspector, KeyValue, SectionLabel, Segmented};
use crate::theme::{
    FONT_MONO, FONT_STATUS, OVERVIEW_DRILL_BAR_GAP, OVERVIEW_DRILL_BARS, OVERVIEW_DRILL_CODE,
    OVERVIEW_DRILL_LABEL, Palette, RULE, SPACE_2, SPACE_4, TEXT_GLYPH, TEXT_LABEL, TEXT_MONO,
    TEXT_SMALL,
};

pub(super) const INSPECTOR_ID: &str = "overview-api-drill";
const TITLE: &str = "API server · 5xx";
const CLOSE_ID: &str = "overview-api-drill-close";
const CODES_ID: &str = "overview-api-drill-codes";
const REQUEST_ID: &str = "overview-api-drill-request";
const ALL: &str = "All";
const TOP_FAILURES: usize = 5;
const NO_FAILURES: &str = "No 5xx responses in this window.";

fn window(traffic: &Traffic) -> String {
    format!(
        "{}–{} · {} sampled by Flock",
        traffic.since,
        traffic.until,
        flock_core::time::span(traffic.window)
    )
}

fn facts(traffic: &Traffic, palette: &Palette) -> AnyElement {
    let errors = format!("{} · {}", grouped(traffic.errors), traffic.error_share());
    let p99 = traffic.p99.map_or_else(|| "—".to_owned(), latency);
    KeyValue::new()
        .label_width(OVERVIEW_DRILL_LABEL)
        .text("window", window(traffic))
        .text("requests", grouped(traffic.requests))
        .row("5xx", mono(errors, palette.err).text_size(TEXT_MONO))
        .text("p99", p99)
        .into_any_element()
}

#[allow(clippy::cast_precision_loss)]
fn bars(traffic: &Traffic, palette: &Palette) -> AnyElement {
    let peak = traffic
        .errors_per_minute
        .iter()
        .copied()
        .max()
        .unwrap_or(0)
        .max(1) as f32;
    let err = palette.err;
    let columns = traffic.errors_per_minute.iter().map(|count| {
        let share = *count as f32 / peak;
        div().flex_1().h(relative(share)).bg(err)
    });
    let chart = div()
        .flex()
        .items_end()
        .gap(OVERVIEW_DRILL_BAR_GAP)
        .h(OVERVIEW_DRILL_BARS)
        .children(columns);
    let axis = div()
        .flex()
        .justify_between()
        .pt(SPACE_2)
        .border_t(RULE)
        .border_color(palette.line2)
        .font_family(FONT_MONO)
        .text_size(TEXT_SMALL)
        .text_color(palette.mute)
        .child(traffic.since.clone())
        .child(traffic.until.clone());
    div()
        .flex()
        .flex_col()
        .child(chart)
        .child(axis)
        .into_any_element()
}

fn line(ix: usize, code: &str, title: String, value: String, palette: &Palette) -> Div {
    let selector = format!("{REQUEST_ID}-{ix}");
    div()
        .debug_selector(|| selector)
        .flex()
        .items_baseline()
        .gap(SPACE_4)
        .py(SPACE_2)
        .border_t(RULE)
        .border_color(palette.line)
        .child(
            mono(code.to_owned(), palette.err)
                .flex_none()
                .w(OVERVIEW_DRILL_CODE)
                .text_size(TEXT_MONO),
        )
        .child(
            mono(title, palette.fg)
                .flex_1()
                .min_w_0()
                .text_size(TEXT_MONO)
                .truncate(),
        )
        .child(mono(value, palette.fg).text_size(TEXT_MONO))
}

fn requests(
    traffic: &Traffic,
    code: Option<&str>,
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> AnyElement {
    let codes = traffic.codes();
    let code = code.filter(|code| codes.iter().any(|(known, _)| known == code));
    let total: u64 = codes.iter().map(|(_, count)| count).sum();
    let selected = code
        .and_then(|code| codes.iter().position(|(c, _)| c == code))
        .map_or(0, |ix| ix + 1);
    let options: Vec<String> = std::iter::once(format!("{ALL} {total}"))
        .chain(codes.iter().map(|(code, count)| format!("{code} {count}")))
        .collect();
    let screen = cx.entity().downgrade();
    let picks: Vec<Option<String>> = std::iter::once(None)
        .chain(codes.iter().map(|(code, _)| Some(code.clone())))
        .collect();
    let segmented = options
        .into_iter()
        .fold(Segmented::new(CODES_ID).small().mono(), |seg, option| {
            seg.option(option)
        })
        .selected(selected)
        .on_select(move |ix, _, cx| {
            let pick = picks.get(*ix).cloned().flatten();
            screen
                .update(cx, |screen, cx| screen.filter_code(pick, cx))
                .ok();
        });
    let header = div()
        .flex()
        .items_center()
        .gap(SPACE_4)
        .child(div().flex_1().child(SectionLabel::new("Requests")))
        .when(total > 0, |header| header.child(segmented));
    let rows: Vec<Div> = traffic
        .failures_with(code)
        .into_iter()
        .take(TOP_FAILURES)
        .enumerate()
        .map(|(ix, failure)| {
            let title = format!("{} {}", failure.verb, failure.resource);
            line(ix, &failure.code, title, grouped(failure.count), palette)
        })
        .collect();
    let empty = rows
        .is_empty()
        .then(|| div().text_color(palette.mute).child(NO_FAILURES));
    div()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .child(header)
        .child(div().flex().flex_col().children(rows).children(empty))
        .into_any_element()
}

fn related(plane: &ControlPlane, palette: &Palette) -> Option<AnyElement> {
    let services = plane.unavailable_services.iter().map(|service| {
        (
            Status::Err,
            format!("APIService {}", service.name),
            format!("Available=False {}", service.reason),
        )
    });
    let webhooks = plane.slow_webhooks().into_iter().map(|webhook| {
        let detail = webhook.detail();
        (Status::Warn, webhook.configuration, detail)
    });
    let rows: Vec<Div> = services
        .chain(webhooks)
        .map(|(status, name, cond)| {
            div()
                .flex()
                .items_baseline()
                .gap(SPACE_4)
                .py(SPACE_2)
                .border_t(RULE)
                .border_color(palette.line)
                .child(
                    div()
                        .flex_none()
                        .font_family(FONT_STATUS)
                        .text_size(TEXT_GLYPH)
                        .text_color(palette.status(status))
                        .child(status.glyph()),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(mono(name, palette.fg).text_size(TEXT_MONO).truncate())
                        .child(mono(cond, palette.mute).text_size(TEXT_LABEL).truncate()),
                )
        })
        .collect();
    (!rows.is_empty()).then(|| div().flex().flex_col().children(rows).into_any_element())
}

pub(super) fn render(
    plane: &ControlPlane,
    code: Option<&str>,
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> AnyElement {
    let screen = cx.entity().downgrade();
    let close = IconButton::glyph(CLOSE_ID, "×", "Close").on_click(move |_, _, cx| {
        screen.update(cx, OverviewScreen::close_drill).ok();
    });
    let inspector = Inspector::new(INSPECTOR_ID, TITLE).actions(close);
    let traffic = match &plane.traffic {
        Probe::Ready(traffic) => traffic,
        Probe::Waiting => {
            return inspector
                .section(
                    None,
                    div().child("Waiting for a second sample of /metrics."),
                )
                .into_any_element();
        }
        Probe::Unavailable(reason) => {
            return inspector
                .section(None, div().child(reason.clone()))
                .into_any_element();
        }
    };
    let inspector = inspector
        .section(None, facts(traffic, palette))
        .section(Some("5xx per minute"), bars(traffic, palette))
        .section(None, requests(traffic, code, palette, cx));
    match related(plane, palette) {
        Some(related) => inspector.section(Some("Related objects"), related),
        None => inspector,
    }
    .into_any_element()
}
