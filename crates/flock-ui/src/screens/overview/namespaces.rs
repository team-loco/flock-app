use flock_core::cluster::{Namespace, NamespaceSort};
use flock_core::overview::{percent, thousands};
use flock_core::{Page, ResourceRef, Status};
use gpui::{
    AnyElement, Context, Div, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, relative,
};

use super::{OverviewData, OverviewScreen, mono, problem_link, row_link, section, section_head};
use crate::kit::Link;
use crate::theme::{
    FONT_MONO, OVERVIEW_COUNT_WIDTH, OVERVIEW_NAMESPACE_MIN, OVERVIEW_NAMESPACE_ROW,
    OVERVIEW_RESTARTS_WIDTH, OVERVIEW_USAGE_BAR, OVERVIEW_USAGE_MIN, OVERVIEW_USAGE_WEIGHT,
    Palette, ROW_TABLE_MIN, SPACE_2, SPACE_3, SPACE_4, SPACE_7, TEXT_LABEL, TEXT_MONO,
};

const TOP_NAMESPACES: usize = 6;
const NAMESPACE_KIND: &str = "Namespace";
const RESTARTS_HIGH: u32 = 10;
const FULL: f32 = 1.;
const DESCENDING: &str = " ↓";
const ASCENDING: &str = " ↑";
const FOOTNOTE: &str = "CPU in cores (m below 1), memory in GiB (Mi below 1) · used / requested · bar fills at 100% of requests";

#[derive(Clone, Copy)]
enum Cell {
    Name,
    Count,
    Usage,
    Restarts,
}

fn cell(kind: Cell, content: Div) -> Div {
    match kind {
        Cell::Name => content.flex_1().min_w(OVERVIEW_NAMESPACE_MIN),
        Cell::Count => content
            .flex_none()
            .w(OVERVIEW_COUNT_WIDTH)
            .flex()
            .justify_end(),
        Cell::Usage => content
            .flex_grow(OVERVIEW_USAGE_WEIGHT)
            .flex_basis(OVERVIEW_USAGE_MIN)
            .min_w(OVERVIEW_USAGE_MIN),
        Cell::Restarts => content
            .flex_none()
            .w(OVERVIEW_RESTARTS_WIDTH)
            .flex()
            .justify_end(),
    }
}

const fn column(sort: NamespaceSort) -> Cell {
    match sort {
        NamespaceSort::Name => Cell::Name,
        NamespaceSort::Pods => Cell::Count,
        NamespaceSort::Failing => Cell::Count,
        NamespaceSort::Cpu => Cell::Usage,
        NamespaceSort::Memory => Cell::Usage,
        NamespaceSort::Restarts => Cell::Restarts,
    }
}

fn grid() -> Div {
    div().flex().items_center().gap(SPACE_7)
}

fn headers(current: NamespaceSort, palette: &Palette, cx: &mut Context<OverviewScreen>) -> Div {
    let buttons = NamespaceSort::ALL
        .into_iter()
        .enumerate()
        .map(|(ix, sort)| {
            let selected = sort == current;
            let arrow = match (selected, sort.ascending()) {
                (true, true) => ASCENDING,
                (true, false) => DESCENDING,
                (false, _) => "",
            };
            let color = if selected { palette.fg } else { palette.mute };
            let id = SharedString::from(format!("overview-ns-sort-{ix}"));
            let selector = id.to_string();
            let button = div()
                .debug_selector(|| selector)
                .id(id)
                .cursor_pointer()
                .text_color(color)
                .whitespace_nowrap()
                .child(format!("{}{arrow}", sort.label()))
                .on_click(cx.listener(move |screen, _, _, cx| screen.sort_by(sort, cx)));
            cell(column(sort), div().child(button))
        });
    grid()
        .h(ROW_TABLE_MIN)
        .text_size(TEXT_LABEL)
        .children(buttons)
}

fn share_color(share: Option<f32>, palette: &Palette) -> Hsla {
    match share {
        Some(share) if share > FULL => palette.warn,
        Some(_) => palette.acc,
        None => palette.mute,
    }
}

fn usage(amount: String, share: Option<f32>, palette: &Palette) -> Div {
    let color = share_color(share, palette);
    let label = share.map_or_else(String::new, percent);
    let filled = share.map_or(0., |share| share.clamp(0., FULL));
    let figures = div()
        .flex()
        .gap(SPACE_4)
        .child(mono(amount, palette.fg2))
        .child(div().flex_1())
        .child(mono(label, color));
    let bar = div()
        .h(OVERVIEW_USAGE_BAR)
        .w_full()
        .bg(palette.line2)
        .child(div().h_full().w(relative(filled)).bg(color));
    div()
        .flex()
        .flex_col()
        .gap(SPACE_3)
        .child(figures)
        .child(bar)
}

fn restarts_color(restarts: u32, palette: &Palette) -> Hsla {
    match restarts {
        0 => palette.mute,
        n if n > RESTARTS_HIGH => palette.err,
        _ => palette.fg2,
    }
}

fn failing_cell(
    ix: usize,
    namespace: &Namespace,
    target: &ResourceRef,
    figure: Div,
    cx: &mut Context<OverviewScreen>,
) -> Div {
    if namespace.problems.err == 0 {
        return cell(Cell::Count, figure);
    }
    let open = OverviewScreen::open(target.clone(), cx);
    let activate: super::Activate = std::rc::Rc::new(open);
    let link = problem_link(format!("overview-ns-{ix}-failing"), figure, activate);
    cell(Cell::Count, div().child(link))
}

fn row(
    ix: usize,
    namespace: &Namespace,
    cluster: &flock_core::ClusterId,
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> AnyElement {
    let failing = namespace.problems.err;
    let (failing_label, failing_color) = if failing > 0 {
        (format!("{} {failing}", Status::Err.glyph()), palette.err)
    } else {
        ("0".to_owned(), palette.mute)
    };
    let target = ResourceRef {
        cluster: cluster.clone(),
        kind: NAMESPACE_KIND.to_owned(),
        namespace: None,
        name: namespace.name.clone(),
    };
    let cells = [
        cell(
            Cell::Name,
            mono(namespace.name.clone(), palette.fg).truncate(),
        ),
        cell(Cell::Count, mono(thousands(namespace.pods), palette.fg2)),
        failing_cell(
            ix,
            namespace,
            &target,
            mono(failing_label, failing_color),
            cx,
        ),
        cell(
            Cell::Usage,
            usage(
                namespace.cpu_against_requests(),
                namespace.cpu_share(),
                palette,
            ),
        ),
        cell(
            Cell::Usage,
            usage(
                namespace.memory_against_requests(),
                namespace.memory_share(),
                palette,
            ),
        ),
        cell(
            Cell::Restarts,
            mono(
                namespace.restarts.to_string(),
                restarts_color(namespace.restarts, palette),
            ),
        ),
    ];
    let activate = OverviewScreen::open(target, cx);
    row_link(
        SharedString::from(format!("overview-ns-{ix}")),
        activate,
        palette,
    )
    .flex()
    .items_center()
    .gap(SPACE_7)
    .min_h(OVERVIEW_NAMESPACE_ROW)
    .py(SPACE_2)
    .font_family(FONT_MONO)
    .text_size(TEXT_MONO)
    .children(cells)
    .into_any_element()
}

pub(super) fn render(
    data: &OverviewData,
    sort: NamespaceSort,
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> AnyElement {
    let total = data.namespaces.len();
    let shown = sort.top(&data.namespaces, TOP_NAMESPACES);
    let meta = format!(
        "top {} of {total} · sorted by {}",
        shown.len(),
        sort.label().to_lowercase()
    );
    let all = Link::new("overview-ns-all", format!("All {total} →")).on_click(
        cx.listener(|_, _, _, cx| cx.emit(super::ScreenEvent::Navigate(Page::Namespaces))),
    );
    let rows: Vec<AnyElement> = shown
        .into_iter()
        .enumerate()
        .map(|(ix, ns)| row(ix, &ns, &data.cluster, palette, cx))
        .collect();
    let footnote = mono(FOOTNOTE, palette.mute)
        .text_size(TEXT_LABEL)
        .pt(SPACE_3);
    section(palette)
        .border_b(crate::theme::RULE)
        .border_color(palette.line)
        .child(section_head("Namespaces", Some(meta), Some(all), palette))
        .child(headers(sort, palette, cx))
        .children(rows)
        .child(footnote)
        .into_any_element()
}
