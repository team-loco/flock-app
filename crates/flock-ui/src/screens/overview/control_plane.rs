use flock_core::control_plane::{ControlPlane, ETCD_CHECK, Probe, SlowWebhook, latency};
use flock_core::overview::{PodPhases, join_clauses};
use flock_core::{Page, Status};
use gpui::{
    AnyElement, Context, Div, Hsla, IntoElement, ParentElement, SharedString, Styled, Window, div,
    prelude::FluentBuilder,
};

use super::{OverviewData, OverviewScreen, mono, row_link, section, section_head};
use crate::theme::{
    FONT_STATUS, OVERVIEW_FACT_CHEVRON, OVERVIEW_FACT_GLYPH, Palette, RULE, SPACE_2, SPACE_4,
    TEXT_BODY, TEXT_CONTROL, TEXT_GLYPH, TEXT_LABEL, TEXT_MONO,
};

const TITLE: &str = "Control plane";
const UNKNOWN: &str = "—";
const CHEVRON: &str = "›";
const API_SERVER: &str = "API server";
const API_SERVER_ID: &str = "overview-control-api-server";
const SCHEDULER_ID: &str = "overview-control-scheduler";
const SAMPLING: &str = "Sampling /metrics; rates follow the next sample";
const WAITING_HEALTH: &str = "Checking /readyz and /livez";
const NOT_EXPOSED: &str = "not exposed";
const NEEDS_METRICS: &str = "latency needs /metrics";

enum Action {
    Drill,
    Page(Page),
}

struct Row {
    id: &'static str,
    key: String,
    sub: Option<String>,
    value: String,
    status: Status,
    value_color: Option<Hsla>,
    action: Option<Action>,
}

impl Row {
    fn plain(id: &'static str, key: impl Into<String>, status: Status) -> Self {
        Self {
            id,
            key: key.into(),
            sub: None,
            value: UNKNOWN.to_owned(),
            status,
            value_color: None,
            action: None,
        }
    }
}

fn failing_checks(plane: &ControlPlane) -> Option<String> {
    let health = plane.health.ready()?;
    let failing: Vec<String> = health.failing().into_iter().map(str::to_owned).collect();
    (!failing.is_empty()).then(|| format!("readyz/livez failing: {}", join_clauses(&failing)))
}

fn api_server(plane: &ControlPlane, palette: &Palette) -> Row {
    let failing = failing_checks(plane);
    let mut row = Row::plain(API_SERVER_ID, API_SERVER, Status::Offline);
    match &plane.traffic {
        Probe::Ready(traffic) => {
            row.status = traffic.status();
            row.sub = Some(traffic.summary());
            row.value = format!("{} 5xx", traffic.error_share());
            row.value_color = (row.status == Status::Ok).then_some(palette.fg2);
            row.action = Some(Action::Drill);
        }
        Probe::Waiting => row.sub = Some(SAMPLING.to_owned()),
        Probe::Unavailable(reason) => row.sub = Some(reason.clone()),
    }
    if let Some(failing) = failing {
        row.status = Status::Err;
        row.sub = Some(failing);
        row.value_color = None;
    }
    if row.sub.is_none() && plane.health == Probe::Waiting {
        row.sub = Some(WAITING_HEALTH.to_owned());
    }
    row
}

fn webhooks(plane: &ControlPlane) -> Row {
    let id = "overview-control-webhooks";
    let webhooks = match &plane.webhooks {
        Probe::Ready(webhooks) => webhooks,
        Probe::Waiting => return Row::plain(id, "Admission webhooks", Status::Offline),
        Probe::Unavailable(reason) => {
            return Row {
                sub: Some(reason.clone()),
                ..Row::plain(id, "Admission webhooks", Status::Offline)
            };
        }
    };
    let key = format!("Admission webhooks · {}", webhooks.len());
    if plane.traffic.ready().is_none() {
        let sub = (!webhooks.is_empty()).then(|| NEEDS_METRICS.to_owned());
        return Row {
            sub,
            ..Row::plain(id, key, Status::Offline)
        };
    }
    let slow = plane.slow_webhooks();
    let sub = slow.first().map(SlowWebhook::detail).or_else(|| {
        plane
            .slowest_webhook()
            .map(|(name, p99)| format!("{name} p99 {}", latency(p99)))
    });
    let status = if slow.is_empty() {
        Status::Ok
    } else {
        Status::Warn
    };
    Row {
        sub,
        value: format!("{} slow", slow.len()),
        ..Row::plain(id, key, status)
    }
}

fn scheduler(pods: PodPhases) -> Row {
    let status = if pods.pending == 0 {
        Status::Ok
    } else {
        Status::Warn
    };
    let sub = (pods.unschedulable > 0).then(|| format!("{} unschedulable", pods.unschedulable));
    Row {
        sub,
        value: format!("{} pending", pods.pending),
        action: Some(Action::Page(Page::Pods)),
        ..Row::plain(SCHEDULER_ID, "Scheduler queue", status)
    }
}

fn etcd(plane: &ControlPlane) -> Row {
    let check = plane
        .health
        .ready()
        .and_then(|health| health.passes(ETCD_CHECK));
    let p99 = plane.traffic.ready().and_then(|traffic| traffic.etcd_p99);
    let (status, value) = match check {
        Some(true) => (Status::Ok, "healthy"),
        Some(false) => (Status::Err, "failing"),
        None => (Status::Offline, NOT_EXPOSED),
    };
    Row {
        sub: p99.map(|p99| format!("API server → etcd p99 {}", latency(p99))),
        value: value.to_owned(),
        ..Row::plain("overview-control-etcd", "etcd", status)
    }
}

fn colors(row: &Row, palette: &Palette) -> (Hsla, Hsla) {
    match row.status {
        Status::Offline => (palette.mute, palette.mute),
        Status::Ok => (palette.ok, row.value_color.unwrap_or(palette.mute)),
        status => (palette.status(status), palette.status(status)),
    }
}

fn body(row: Row, clickable: bool, palette: &Palette) -> Div {
    let (glyph_color, value_color) = colors(&row, palette);
    let sub = row
        .sub
        .map(|sub| mono(sub, palette.mute).text_size(TEXT_LABEL).truncate());
    let key = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .child(div().child(row.key))
        .children(sub);
    div()
        .flex()
        .items_baseline()
        .gap(SPACE_4)
        .py(SPACE_2)
        .text_size(TEXT_BODY)
        .child(
            div()
                .flex_none()
                .w(OVERVIEW_FACT_GLYPH)
                .font_family(FONT_STATUS)
                .text_size(TEXT_GLYPH)
                .text_color(glyph_color)
                .child(row.status.glyph()),
        )
        .child(key)
        .child(mono(row.value, value_color).text_size(TEXT_MONO))
        .child(
            div()
                .flex_none()
                .w(OVERVIEW_FACT_CHEVRON)
                .text_size(TEXT_CONTROL)
                .text_color(palette.sep)
                .when(clickable, |chevron| chevron.child(CHEVRON)),
        )
}

fn render_row(
    mut row: Row,
    drilled: bool,
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> AnyElement {
    let id = row.id;
    let action = row.action.take();
    let content = body(row, action.is_some(), palette);
    match action {
        Some(Action::Drill) => {
            let screen = cx.entity().downgrade();
            let activate = move |window: &mut Window, cx: &mut gpui::App| {
                screen
                    .update(cx, |screen, cx| screen.toggle_drill(window, cx))
                    .ok();
            };
            row_link(SharedString::from(id), activate, palette)
                .when(drilled, |link| link.bg(palette.sel))
                .child(content)
                .into_any_element()
        }
        Some(Action::Page(page)) => {
            let activate = OverviewScreen::navigate(page, cx);
            row_link(SharedString::from(id), activate, palette)
                .child(content)
                .into_any_element()
        }
        None => div()
            .border_t(RULE)
            .border_color(palette.line)
            .child(content)
            .into_any_element(),
    }
}

fn meta(plane: &ControlPlane) -> Option<String> {
    let traffic = plane.traffic.ready()?;
    Some(format!("sampled since {}", traffic.since))
}

pub(super) fn render(
    data: &OverviewData,
    drilled: bool,
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> AnyElement {
    let plane = &data.control_plane;
    let rows = [
        api_server(plane, palette),
        webhooks(plane),
        scheduler(data.pods),
        etcd(plane),
    ];
    let rows: Vec<AnyElement> = rows
        .into_iter()
        .map(|row| render_row(row, drilled, palette, cx))
        .collect();
    section(palette)
        .child(section_head(TITLE, meta(plane), None, palette))
        .children(rows)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use flock_core::control_plane::{Check, Health, Traffic, Webhook};

    use super::*;

    fn traffic() -> Traffic {
        Traffic {
            since: "09:58".into(),
            requests: 1_071_382,
            errors: 214,
            p99: Some(Duration::from_millis(182)),
            etcd_p99: Some(Duration::from_millis(24)),
            ..Traffic::default()
        }
    }

    fn health(etcd: bool) -> Probe<Health> {
        Probe::Ready(Health {
            checks: vec![Check {
                name: ETCD_CHECK.into(),
                passed: etcd,
            }],
        })
    }

    #[test]
    fn api_server_row_reads_the_sampled_traffic() {
        let plane = ControlPlane {
            health: health(true),
            traffic: Probe::Ready(traffic()),
            ..ControlPlane::default()
        };
        let row = api_server(&plane, &Palette::dark());
        assert_eq!(row.status, Status::Ok);
        assert_eq!(
            row.sub.as_deref(),
            Some("p99 182 ms · 214 of 1.07M requests")
        );
        assert_eq!(row.value, "0.02% 5xx");
        assert!(matches!(row.action, Some(Action::Drill)));
        assert_eq!(meta(&plane).as_deref(), Some("sampled since 09:58"));
        let etcd = etcd(&plane);
        assert_eq!(etcd.value, "healthy");
        assert_eq!(etcd.sub.as_deref(), Some("API server → etcd p99 24 ms"));
    }

    #[test]
    fn forbidden_metrics_show_the_reason_without_a_drill() {
        let reason = "/metrics: forbidden: User \"dev\" cannot get path \"/metrics\"";
        let plane = ControlPlane {
            health: health(false),
            traffic: Probe::Unavailable(reason.into()),
            webhooks: Probe::Ready(vec![Webhook {
                name: "w".into(),
                configuration: "c".into(),
                timeout: Duration::from_secs(10),
            }]),
            ..ControlPlane::default()
        };
        let row = api_server(&plane, &Palette::dark());
        assert_eq!(row.status, Status::Err);
        assert_eq!(row.sub.as_deref(), Some("readyz/livez failing: etcd"));
        assert!(row.action.is_none());
        let unhealthy = ControlPlane {
            health: Probe::default(),
            ..plane.clone()
        };
        assert_eq!(
            api_server(&unhealthy, &Palette::dark()).sub.as_deref(),
            Some(reason)
        );
        let webhooks = webhooks(&plane);
        assert_eq!(webhooks.key, "Admission webhooks · 1");
        assert_eq!(webhooks.sub.as_deref(), Some(NEEDS_METRICS));
        assert_eq!(webhooks.value, UNKNOWN);
        assert_eq!(etcd(&plane).value, "failing");
        assert_eq!(etcd(&ControlPlane::default()).value, NOT_EXPOSED);
    }

    #[test]
    fn scheduler_queue_counts_pending_and_unschedulable_pods() {
        let pods = PodPhases {
            pending: 82,
            unschedulable: 64,
            ..PodPhases::default()
        };
        let row = scheduler(pods);
        assert_eq!(row.value, "82 pending");
        assert_eq!(row.sub.as_deref(), Some("64 unschedulable"));
        assert_eq!(row.status, Status::Warn);
        assert_eq!(scheduler(PodPhases::default()).status, Status::Ok);
    }
}
