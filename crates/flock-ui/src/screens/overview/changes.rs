use flock_core::Command;
use flock_core::cluster_changes::{ChangeKind, ClusterChange, within};
use flock_core::overview::TimeRange;
use gpui::{AnyElement, Context, Hsla, IntoElement, ParentElement, SharedString, Styled, div};

use super::{OverviewData, OverviewScreen, ScreenEvent, mono, row_link, section, section_head};
use crate::kit::Link;
use crate::theme::{
    OVERVIEW_CHANGE_BY, OVERVIEW_CHANGE_KIND, OVERVIEW_CHANGE_ROW, OVERVIEW_CHANGE_TIME, Palette,
    SPACE_5, TEXT_CONTROL, TEXT_LABEL, TEXT_MONO, TEXT_SMALL,
};

const TITLE: &str = "Recent changes";
const ALL_ID: &str = "overview-changes-all";
const ROW_ID: &str = "overview-change";
const LIMIT: usize = 6;

fn kind_color(change: &ClusterChange, palette: &Palette) -> Hsla {
    if let Some(status) = change.status {
        return palette.status(status);
    }
    match change.kind {
        ChangeKind::Rollout => palette.acc,
        ChangeKind::Helm => palette.acc,
        ChangeKind::Flux => palette.acc,
        ChangeKind::Scale => palette.mute,
        ChangeKind::Node => palette.mute,
        ChangeKind::Crd => palette.mute,
        ChangeKind::Namespace => palette.mute,
    }
}

fn row(
    ix: usize,
    change: &ClusterChange,
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> AnyElement {
    let time = mono(change.at.clone(), palette.mute)
        .flex_none()
        .w(OVERVIEW_CHANGE_TIME)
        .text_size(TEXT_SMALL);
    let kind = mono(change.kind.label(), kind_color(change, palette))
        .flex_none()
        .w(OVERVIEW_CHANGE_KIND)
        .text_size(TEXT_LABEL);
    let what = mono(change.what.clone(), palette.fg)
        .flex_1()
        .min_w_0()
        .text_size(TEXT_MONO)
        .truncate();
    let by = div()
        .flex_none()
        .w(OVERVIEW_CHANGE_BY)
        .flex()
        .justify_end()
        .text_size(TEXT_SMALL)
        .text_color(palette.mute)
        .child(div().truncate().child(change.by.clone()));
    let activate = OverviewScreen::open(change.target.clone(), cx);
    row_link(
        SharedString::from(format!("{ROW_ID}-{ix}")),
        activate,
        palette,
    )
    .flex()
    .items_center()
    .gap(SPACE_5)
    .h(OVERVIEW_CHANGE_ROW)
    .child(time)
    .child(kind)
    .child(what)
    .child(by)
    .into_any_element()
}

pub(super) fn render(
    data: &OverviewData,
    range: TimeRange,
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> Vec<AnyElement> {
    let recent: Vec<&ClusterChange> = within(&data.changes, range).collect();
    let meta = format!("last {} · {}", range.label(), recent.len());
    let cluster = data.cluster.clone();
    let all =
        Link::new(ALL_ID, crate::screens::ALL_LINK).on_click(cx.listener(move |_, _, _, cx| {
            let command = Command::OpenClusterChanges(cluster.clone());
            cx.emit(ScreenEvent::Command(command));
        }));
    let head = section_head(TITLE, Some(meta), Some(all), palette);
    let rows: Vec<AnyElement> = recent
        .into_iter()
        .take(LIMIT)
        .enumerate()
        .map(|(ix, change)| row(ix, change, palette, cx))
        .collect();
    let empty = rows.is_empty().then(|| {
        div()
            .text_size(TEXT_CONTROL)
            .text_color(palette.mute)
            .child(format!("No changes in the last {}.", range.label()))
    });
    let section = section(palette).child(head).children(rows).children(empty);
    vec![section.into_any_element()]
}
