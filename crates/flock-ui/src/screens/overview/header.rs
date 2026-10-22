use flock_core::{ClusterSummary, Page, Status};
use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, div,
};

use super::{OverviewData, OverviewScreen, mono};
use crate::theme::{FONT_MONO, Palette, RULE, SPACE_5, SPACE_6, SPACE_8, TEXT_MONO};

const FLUX: &str = "Flux";
const ARROW: &str = "→";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FluxLine {
    ready: u32,
    total: u32,
    suspended: u32,
    revision: String,
}

impl FluxLine {
    pub(super) fn of(cluster: &ClusterSummary) -> Option<Self> {
        let metrics = cluster.metrics.as_ref()?;
        let total = metrics.flux.total.filter(|total| *total > 0)?;
        Some(Self {
            ready: metrics.flux_ready(),
            total,
            suspended: metrics.flux.sus,
            revision: metrics.revision.sha.clone(),
        })
    }
}

fn problems(
    data: &OverviewData,
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> Option<AnyElement> {
    let counts = data.problems;
    if counts.err == 0 && counts.warn == 0 {
        return None;
    }
    let part = |status: Status, count: u32| {
        (count > 0).then(|| {
            mono(
                format!("{} {count}", status.glyph()),
                palette.status(status),
            )
        })
    };
    let activate = OverviewScreen::navigate(Page::ClusterProblems, cx);
    let link = div()
        .debug_selector(|| "overview-problems".to_owned())
        .id("overview-problems")
        .flex()
        .gap(SPACE_5)
        .cursor_pointer()
        .hover(Styled::underline)
        .on_click(move |_, window, cx| activate(window, cx))
        .children(part(Status::Err, counts.err))
        .children(part(Status::Warn, counts.warn));
    Some(link.into_any_element())
}

fn flux(line: &FluxLine, palette: &Palette, cx: &mut Context<OverviewScreen>) -> AnyElement {
    let activate = OverviewScreen::navigate(Page::Kustomizations, cx);
    let suspended = (line.suspended > 0).then(|| {
        mono(
            format!(
                "· {} {} suspended",
                Status::Suspended.glyph(),
                line.suspended
            ),
            palette.sus,
        )
    });
    let revision = (!line.revision.is_empty()).then(|| {
        div()
            .flex()
            .gap(SPACE_5)
            .child(mono("· applied", palette.fg2))
            .child(mono(line.revision.clone(), palette.acc))
    });
    div()
        .debug_selector(|| "overview-flux".to_owned())
        .id("overview-flux")
        .flex()
        .gap(SPACE_5)
        .cursor_pointer()
        .hover(Styled::underline)
        .on_click(move |_, window, cx| activate(window, cx))
        .child(mono(FLUX, palette.fg2))
        .child(mono(
            format!("{}/{} ready", line.ready, line.total),
            palette.fg,
        ))
        .children(suspended)
        .children(revision)
        .child(mono(ARROW, palette.fg2))
        .into_any_element()
}

pub(super) fn render(
    data: &OverviewData,
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> AnyElement {
    let first = div()
        .flex()
        .items_center()
        .gap(SPACE_5)
        .min_w_0()
        .children(problems(data, palette, cx));
    let flux = data.flux.as_ref().map(|line| flux(line, palette, cx));
    div()
        .debug_selector(|| "meta-line".to_owned())
        .flex()
        .flex_col()
        .gap(SPACE_5)
        .px(SPACE_8)
        .py(SPACE_6)
        .border_b(RULE)
        .border_color(palette.line)
        .font_family(FONT_MONO)
        .text_size(TEXT_MONO)
        .child(first)
        .children(flux)
        .into_any_element()
}
