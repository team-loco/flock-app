use flock_core::action::forwards_label;
use flock_core::gitops::flux_ready;
use flock_core::overview::plural;
use flock_core::palette::{COMMAND_PREFIX, STOP_FORWARD};
use flock_core::{Command, Page, Status};
use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, div,
};

use super::Shell;
use crate::kit::status_symbols;
use crate::theme::{
    ActiveTheme, FONT_MONO, RULE, SPACE_5, SPACE_6, SPACE_8, STATUS_BAR, TEXT_SMALL,
};

const FLUX: &str = "flux";
#[cfg(feature = "loco")]
const LOCO_SYNC_UNKNOWN: &str = "sync not observable in-cluster";

impl Shell {
    pub(super) fn status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = *cx.palette();
        let snapshot = &self.state.snapshot;
        let problems = snapshot.problems();
        let left = match self.state.location().page {
            Page::Changes if self.selected_cluster().is_none() => snapshot.changes.fetched_label(),
            Page::GitOps if self.selected_cluster().is_none() => {
                let flux = flux_ready(&snapshot.clusters);
                format!("{FLUX} {}/{} ready", flux.ready, flux.total)
            }
            _ => format!(
                "{} of {} connected",
                snapshot.connected(),
                plural(snapshot.clusters.len(), "cluster", "clusters")
            ),
        };
        div()
            .flex()
            .items_center()
            .flex_none()
            .gap(SPACE_8)
            .h(STATUS_BAR)
            .pl(SPACE_6)
            .border_t(RULE)
            .border_color(palette.line)
            .bg(palette.chrome)
            .font_family(FONT_MONO)
            .text_size(TEXT_SMALL)
            .text_color(palette.mute)
            .whitespace_nowrap()
            .overflow_hidden()
            .child(
                div()
                    .id("status-problems")
                    .flex()
                    .gap(SPACE_5)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dispatch(Command::SelectPage(Page::Overview), cx);
                    }))
                    .child(div().text_color(palette.err).child(status_symbols(format!(
                        "{} {}",
                        Status::Err.glyph(),
                        problems.err
                    ))))
                    .child(div().text_color(palette.warn).child(status_symbols(format!(
                        "{} {}",
                        Status::Warn.glyph(),
                        problems.warn
                    )))),
            )
            .child(div().min_w_0().truncate().child(left))
            .child(div().flex_1())
            .children(crate::updater::status_item(cx))
            .children(self.forwards(cx))
            .children(self.loco_agent(&palette))
            .child(self.switcher.clone())
            .into_any_element()
    }

    fn forwards(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let label = forwards_label(self.state.snapshot.forwards.len())?;
        let element = div()
            .id("status-forwards")
            .cursor_pointer()
            .on_click(cx.listener(|this, _, window, cx| {
                let query = format!("{COMMAND_PREFIX}{STOP_FORWARD}");
                this.open_palette(&query, window, cx);
            }))
            .child(label)
            .into_any_element();
        Some(element)
    }

    #[cfg(feature = "loco")]
    fn loco_agent(&self, palette: &crate::theme::Palette) -> Option<AnyElement> {
        use crate::theme::SPACE_3;
        let id = self.state.location().cluster.as_ref()?;
        let agent = &self.state.snapshot.loco.get(id)?.platform.agent;
        let (status, label) = match agent.last_heartbeat {
            Some(heartbeat) => (
                Status::Ok,
                format!(
                    "loco agent · synced {} ago",
                    flock_core::time::age(heartbeat)
                ),
            ),
            None => match agent.ready.zip(agent.readiness()) {
                Some(((ready, desired), status)) => (
                    status,
                    format!("loco agent {ready}/{desired} ready · {LOCO_SYNC_UNKNOWN}"),
                ),
                None => (Status::Offline, format!("loco agent · {LOCO_SYNC_UNKNOWN}")),
            },
        };
        let element = div()
            .flex()
            .items_center()
            .gap(SPACE_3)
            .child(
                div()
                    .text_color(palette.status(status))
                    .child(status_symbols(status.glyph())),
            )
            .child(label)
            .into_any_element();
        Some(element)
    }

    #[cfg(not(feature = "loco"))]
    #[allow(clippy::unused_self)]
    fn loco_agent(&self, _: &crate::theme::Palette) -> Option<AnyElement> {
        None
    }
}
