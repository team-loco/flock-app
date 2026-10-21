use flock_core::overview::{plural, thousands};
use flock_core::{Page, ResourceRef};
use gpui::{AnyElement, Context, Hsla, IntoElement, ParentElement, SharedString, Styled, div};

use super::{
    OverviewData, OverviewScreen, head_row, meta_text, mono, problem_link, row_link, section,
    status_color,
};
use crate::kit::Link;
use crate::theme::{Palette, SPACE_2, SPACE_5, TEXT_CONTROL, TEXT_LABEL, TEXT_MONO};

const ALL: &str = "All →";

struct Row {
    target: ResourceRef,
    detail: String,
    value: String,
    color: Hsla,
}

struct Top {
    id: &'static str,
    title: &'static str,
    meta: String,
    problems: u32,
    columns: (&'static str, &'static str),
    empty: &'static str,
    rows: Vec<Row>,
}

fn row(
    id: &'static str,
    ix: usize,
    row: Row,
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> AnyElement {
    let namespace = row.target.namespace.clone().unwrap_or_default();
    let name = div()
        .flex()
        .gap(SPACE_5)
        .min_w_0()
        .text_size(TEXT_MONO)
        .child(mono(row.target.name.clone(), palette.fg).truncate())
        .child(mono(namespace, palette.mute));
    let detail = mono(row.detail, palette.mute)
        .text_size(TEXT_LABEL)
        .truncate();
    let activate = OverviewScreen::open(row.target, cx);
    row_link(SharedString::from(format!("{id}-{ix}")), activate, palette)
        .flex()
        .items_center()
        .gap(SPACE_5)
        .py(SPACE_2)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(name)
                .child(detail),
        )
        .child(mono(row.value, row.color).text_size(TEXT_MONO))
        .into_any_element()
}

fn top(top: Top, palette: &Palette, cx: &mut Context<OverviewScreen>) -> AnyElement {
    let all = Link::new(SharedString::from(format!("{}-all", top.id)), ALL)
        .on_click(cx.listener(|_, _, _, cx| cx.emit(super::ScreenEvent::Navigate(Page::Pods))));
    let meta = meta_text(top.meta, palette);
    let meta = if top.problems > 0 {
        let activate = OverviewScreen::attention(Page::Pods, None, cx);
        problem_link(format!("{}-meta", top.id), meta, activate).into_any_element()
    } else {
        meta.into_any_element()
    };
    let head = head_row(top.title, Some(meta), Some(all));
    let columns = div()
        .flex()
        .pb(SPACE_2)
        .text_size(TEXT_LABEL)
        .text_color(palette.mute)
        .child(div().flex_1().child(top.columns.0))
        .child(top.columns.1);
    let id = top.id;
    let rows: Vec<AnyElement> = top
        .rows
        .into_iter()
        .enumerate()
        .map(|(ix, r)| row(id, ix, r, palette, cx))
        .collect();
    let empty = rows.is_empty().then(|| {
        div()
            .text_size(TEXT_CONTROL)
            .text_color(palette.mute)
            .child(top.empty)
    });
    section(palette)
        .child(head)
        .child(columns)
        .children(rows)
        .children(empty)
        .into_any_element()
}

pub(super) fn render(
    data: &OverviewData,
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> Vec<AnyElement> {
    let restarts = data
        .tops
        .restarts
        .iter()
        .map(|pod| Row {
            target: pod.target.clone(),
            detail: pod.detail(),
            value: pod.restarts.to_string(),
            color: status_color(pod.status(), palette.fg2, palette),
        })
        .collect();
    let pending = data
        .tops
        .pending
        .iter()
        .map(|pod| Row {
            target: pod.target.clone(),
            detail: pod.message.clone(),
            value: pod.duration(),
            color: status_color(pod.status(), palette.fg2, palette),
        })
        .collect();
    let restarted = plural(data.tops.restarted, "pod restarted", "pods restarted");
    let tops = [
        Top {
            id: "overview-restarts",
            title: "Pods · most container restarts",
            meta: restarted,
            problems: 0,
            columns: ("Pod · container · last termination", "Restarts"),
            empty: "No container has restarted.",
            rows: restarts,
        },
        Top {
            id: "overview-pending",
            title: "Pods · Pending longest",
            meta: format!("{} Pending", thousands(data.pods.pending)),
            problems: data.pods.pending,
            columns: ("Pod · last FailedScheduling message", "Pending for"),
            empty: "No pods are pending.",
            rows: pending,
        },
    ];
    tops.into_iter().map(|t| top(t, palette, cx)).collect()
}
