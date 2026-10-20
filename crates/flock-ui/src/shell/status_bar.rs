use std::time::{Duration, Instant};

use flock_core::action::forwards_label;
use flock_core::gitops::flux_ready;
use flock_core::overview::plural;
use flock_core::palette::{COMMAND_PREFIX, STOP_FORWARD};
use flock_core::time::age;
use flock_core::{Command, Page, Status};
use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, div, svg,
};

use super::Shell;
use crate::assets::REFRESH;
use crate::kit::{Tooltip, status_symbols};
use crate::theme::{
    ActiveTheme, FONT_MONO, ICON, Palette, RADIUS_SMALL, RULE, SPACE_2, SPACE_3, SPACE_5, SPACE_6,
    SPACE_8, STATUS_BAR, STATUS_BUTTON, TEXT_SMALL,
};

const FLUX: &str = "flux";
const VERSION_TOOLTIP: &str = "Kubernetes version";
const REFRESH_TOOLTIP: &str = "Refresh this view";
pub const REFRESH_SELECTOR: &str = "status-refresh";
const REFRESH_KEY: &str = "⇧⌘R";
pub(super) const SYNC_TICK: Duration = Duration::from_secs(1);

fn synced_label(elapsed: Duration) -> String {
    format!("synced {} ago", age(elapsed))
}

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
            .children(self.kubernetes_version())
            .child(div().flex_1())
            .child(self.refresh_button(&palette, cx))
            .children(crate::updater::status_item(cx))
            .children(self.forwards(cx))
            .child(self.switcher.clone())
            .into_any_element()
    }

    pub(super) fn refresh_view(&mut self, cx: &mut Context<Self>) {
        let scope = self.state.location().cluster.clone();
        self.synced_at = Instant::now();
        self.dispatch(Command::Refresh(scope), cx);
    }

    fn kubernetes_version(&self) -> Option<AnyElement> {
        let version = self.selected_cluster()?.version.clone()?;
        let element = div()
            .id("status-version")
            .tooltip(Tooltip::text(VERSION_TOOLTIP))
            .child(version)
            .into_any_element();
        Some(element)
    }

    fn refresh_button(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let hover = palette.hover;
        let label = synced_label(self.synced_at.elapsed());
        div()
            .id(REFRESH_SELECTOR)
            .debug_selector(|| REFRESH_SELECTOR.to_owned())
            .flex()
            .items_center()
            .gap(SPACE_3)
            .h(STATUS_BUTTON)
            .px(SPACE_2)
            .rounded(RADIUS_SMALL)
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .tooltip(Tooltip::text(format!("{REFRESH_TOOLTIP}  {REFRESH_KEY}")))
            .on_click(cx.listener(|this, _, _, cx| this.refresh_view(cx)))
            .child(svg().path(REFRESH).size(ICON).text_color(palette.mute))
            .child(label)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_synced_label_counts_up_in_the_largest_whole_unit() {
        let cases = [
            (0, "synced 0s ago"),
            (4, "synced 4s ago"),
            (59, "synced 59s ago"),
            (60, "synced 1m ago"),
            (3600, "synced 1h ago"),
        ];
        for (secs, expected) in cases {
            assert_eq!(synced_label(Duration::from_secs(secs)), expected);
        }
    }
}
