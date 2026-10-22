use flock_core::overview::{Capacity, PERCENT, TimeRange, Utilization, UtilizationSource, percent};
use flock_core::{Command, Page};
use gpui::{
    AnyElement, Context, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    Styled, div,
};

use super::{OverviewData, OverviewScreen, ScreenEvent, band, mono, section};
use crate::kit::{Button, EmptyState, ErrorState, LineChart, SectionLabel, Series};
use crate::theme::{OVERVIEW_CHART_HEIGHT, Palette, SPACE_4, SPACE_8, TEXT_CONTROL, TEXT_SMALL};

const CHART_MAX: f32 = 100.;
const Y_LABELS: [&str; 3] = ["100%", "50%", "0"];
const UTILIZATION: &str = "Utilization";
const USED: &str = "used";
const RETRY: &str = "Retry";
const SET_METRICS_SOURCE: &str = "Set metrics source";

fn labels(labels: &[&'static str]) -> Vec<SharedString> {
    labels.iter().map(|l| SharedString::from(*l)).collect()
}

struct Plot<'a> {
    title: &'static str,
    capacity: &'a Capacity,
    values: Vec<f32>,
    positions: Vec<f32>,
    color: Hsla,
}

fn chart(plot: Plot<'_>, range: TimeRange, palette: &Palette) -> AnyElement {
    let now = plot.values.last().map(|v| percent(v / PERCENT));
    let requests = plot.capacity.requests * PERCENT;
    let threshold = format!("requests {}", percent(plot.capacity.requests));
    let line = LineChart::new(CHART_MAX)
        .height(OVERVIEW_CHART_HEIGHT)
        .y_labels(labels(&Y_LABELS))
        .x_labels(labels(&range.axis()))
        .series(Series::new(USED, plot.color, plot.values).positions(plot.positions))
        .threshold(requests, threshold, palette.fg2);
    let now = now.map(|now| mono(now, palette.fg2).text_size(TEXT_SMALL));
    let head = div()
        .flex()
        .items_center()
        .gap(SPACE_4)
        .child(SectionLabel::new(plot.title))
        .children(now)
        .child(div().flex_1())
        .child(mono(plot.capacity.amount().labelled(), palette.mute).text_size(TEXT_SMALL));
    section(palette)
        .gap(SPACE_4)
        .child(head)
        .child(line)
        .into_any_element()
}

fn message(
    data: &OverviewData,
    range: TimeRange,
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> AnyElement {
    let chart = LineChart::new(CHART_MAX).title(SectionLabel::new(UTILIZATION));
    let chart = if let Some(reason) = data.source.error() {
        let retry = Button::new("metrics-retry", RETRY).on_click(cx.listener(|_, _, _, cx| {
            cx.emit(ScreenEvent::Navigate(Page::ClusterOverview));
        }));
        chart.message(ErrorState::raw(reason, retry))
    } else {
        let cluster = data.cluster.clone();
        let settings = Button::new("metrics-settings", SET_METRICS_SOURCE)
            .primary()
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(ScreenEvent::Command(Command::ShowClusterSettings(
                    cluster.clone(),
                )));
            }));
        let empty = EmptyState::new(UtilizationSource::empty_sentence(range))
            .selector("metrics-empty")
            .centered()
            .detail(data.source.empty_reason())
            .action(settings);
        chart.message(empty)
    };
    section(palette).child(chart).into_any_element()
}

fn available(data: &OverviewData, range: TimeRange) -> Option<&Utilization> {
    if data.source.error().is_some() {
        return None;
    }
    data.utilization
        .iter()
        .find(|u| u.range == range)
        .filter(|u| !u.cpu.is_empty() || !u.memory.is_empty())
}

pub(super) fn render(
    data: &OverviewData,
    range: TimeRange,
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> AnyElement {
    let (charts, note) = match available(data, range) {
        Some(u) => {
            let cpu = Plot {
                title: "CPU",
                capacity: &data.cpu,
                values: u.cpu.clone(),
                positions: u.cpu_positions.clone(),
                color: palette.acc,
            };
            let memory = Plot {
                title: "Memory",
                capacity: &data.memory,
                values: u.memory.clone(),
                positions: u.memory_positions.clone(),
                color: palette.ai,
            };
            let charts = vec![chart(cpu, range, palette), chart(memory, range, palette)];
            (charts, data.source.note())
        }
        None => (vec![message(data, range, palette, cx)], None),
    };
    let note = note.map(|note| {
        div()
            .debug_selector(|| "metrics-note".to_owned())
            .px(SPACE_8)
            .pt(SPACE_4)
            .text_size(TEXT_CONTROL)
            .text_color(palette.mute)
            .child(note)
    });
    div()
        .debug_selector(|| "utilization-chart".to_owned())
        .flex()
        .flex_col()
        .children(band(charts, palette))
        .children(note)
        .into_any_element()
}
