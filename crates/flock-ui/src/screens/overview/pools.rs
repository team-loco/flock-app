use flock_core::node_pools::NodePool;
use flock_core::overview::percent;
use flock_core::{Page, Status};
use gpui::{
    AnyElement, Context, Div, IntoElement, ParentElement, SharedString, Styled, div, relative,
};

use super::{OverviewData, OverviewScreen, mono, row_link, section, section_head, status_color};
use crate::theme::{
    OVERVIEW_POOL_FIT, OVERVIEW_POOL_METER, OVERVIEW_POOL_NODES, OVERVIEW_POOL_ROW,
    OVERVIEW_USAGE_BAR, Palette, SPACE_1, SPACE_2, SPACE_5, TEXT_AXIS, TEXT_LABEL, TEXT_MONO,
};

const TITLE: &str = "Node pools · requested / allocatable";
const FULL: f32 = 1.;

#[derive(Clone, Copy)]
enum Cell {
    Pool,
    Nodes,
    Meter,
    Fit,
}

fn cell(kind: Cell, content: Div) -> Div {
    match kind {
        Cell::Pool => content.flex_1().min_w_0(),
        Cell::Nodes => content
            .flex_none()
            .w(OVERVIEW_POOL_NODES)
            .flex()
            .justify_end(),
        Cell::Meter => content.flex_none().w(OVERVIEW_POOL_METER),
        Cell::Fit => content
            .flex_none()
            .w(OVERVIEW_POOL_FIT)
            .flex()
            .justify_end(),
    }
}

fn grid() -> Div {
    div().flex().items_center().gap(SPACE_5)
}

fn headers(palette: &Palette) -> Div {
    let labels = [
        (Cell::Pool, "Pool"),
        (Cell::Nodes, "Nodes"),
        (Cell::Meter, "CPU"),
        (Cell::Meter, "Memory"),
        (Cell::Fit, "Largest fit"),
    ];
    grid()
        .pb(SPACE_2)
        .text_size(TEXT_LABEL)
        .text_color(palette.mute)
        .children(labels.map(|(kind, label)| cell(kind, div().child(label))))
}

fn meter(fraction: f32, status: Option<Status>, palette: &Palette) -> Div {
    let color = status_color(status, palette.fg2, palette);
    let filled = fraction.clamp(0., FULL);
    let bar = div()
        .h(OVERVIEW_USAGE_BAR)
        .w_full()
        .bg(palette.line2)
        .child(div().h_full().w(relative(filled)).bg(color));
    div()
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .child(mono(percent(fraction), color).text_size(TEXT_LABEL))
        .child(bar)
}

fn row(
    ix: usize,
    pool: &NodePool,
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> AnyElement {
    let name = div()
        .flex()
        .flex_col()
        .child(mono(pool.name.clone(), palette.fg).truncate())
        .child(
            mono(pool.instance_type.clone(), palette.mute)
                .text_size(TEXT_AXIS)
                .truncate(),
        );
    let fit_color = status_color(pool.fit_status(), palette.fg2, palette);
    let cells = [
        cell(Cell::Pool, name),
        cell(Cell::Nodes, mono(pool.nodes.to_string(), palette.fg)),
        cell(Cell::Meter, meter(pool.cpu, pool.cpu_status(), palette)),
        cell(
            Cell::Meter,
            meter(pool.memory, pool.memory_status(), palette),
        ),
        cell(
            Cell::Fit,
            mono(pool.fit_label(), fit_color).text_size(TEXT_LABEL),
        ),
    ];
    let activate = OverviewScreen::navigate(Page::Nodes, cx);
    row_link(
        SharedString::from(format!("overview-pool-{ix}")),
        activate,
        palette,
    )
    .flex()
    .items_center()
    .gap(SPACE_5)
    .min_h(OVERVIEW_POOL_ROW)
    .text_size(TEXT_MONO)
    .children(cells)
    .into_any_element()
}

pub(super) fn render(
    data: &OverviewData,
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> Vec<AnyElement> {
    if data.pools.is_empty() {
        return Vec::new();
    }
    let rows: Vec<AnyElement> = data
        .pools
        .iter()
        .enumerate()
        .map(|(ix, pool)| row(ix, pool, palette, cx))
        .collect();
    let section = section(palette)
        .child(section_head(TITLE, None, None, palette))
        .child(headers(palette))
        .children(rows);
    vec![section.into_any_element()]
}
