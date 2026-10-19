use flock_core::history::Location;
use flock_core::overview::plural;
use flock_core::watch::WatchState;
use flock_core::{Command, Counts, Page};
use gpui::{
    AnyElement, App, AppContext, Context, Entity, FontWeight, InteractiveElement, IntoElement,
    MouseButton, ParentElement, StatefulInteractiveElement, Styled, Window, div,
};

use flock_core::AddClusterStep;

use super::Shell;
use crate::assets::GEAR;
use crate::kit::{Button, EmptyState, IconButton, Skeleton, status_symbols};
use crate::screens::add_cluster::empty_flock;
use crate::screens::logs::LogsScreen;
use crate::screens::multilog::MultiLogsScreen;
use crate::screens::workload::WorkloadScreen;
use crate::theme::{
    ActiveTheme, FONT_MONO, PAGE_HEADER, Palette, RULE, SPACE_2, SPACE_4, SPACE_5, SPACE_7,
    SPACE_8, TEXT_LABEL, TEXT_SMALL, TEXT_TITLE,
};

const LIVE: &str = "live";
const SEPARATOR: &str = "· ";
const UNAVAILABLE_REASON: &str =
    "It was removed from Flock or its kubeconfig context is gone, so this tab has nothing to show.";
const SHOW_ALL_CLUSTERS: &str = "Show all clusters";
const OVERVIEW_SETTINGS: &str = "overview-settings";
const CLUSTER_SETTINGS: &str = "Cluster settings";

impl Shell {
    pub(super) fn page(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tab = self.state.panes().panes()[ix].active();
        self.screens(ix)
            .navigation
            .clone()
            .update(cx, |navigation, cx| {
                navigation.set_history(ix, tab, &self.state.snapshot, window, cx);
            });
        let location = self.location_at(ix).clone();
        if self.loaded && !self.state.available(&location) {
            return self.unavailable(ix, &location, cx);
        }
        #[cfg(feature = "loco")]
        if crate::screens::loco::owns(location.page) {
            return self.screens(ix).loco.clone().into_any_element();
        }
        if let Some(screen) = self.multi_logs_screen(ix, &location, window, cx) {
            self.screens_mut(ix).logs = None;
            return fill(screen);
        }
        if let Some(screen) = self.logs_screen(ix, &location, window, cx) {
            return fill(screen);
        }
        if let Some(screen) = self.workload_screen(ix, &location, window, cx) {
            return fill(screen);
        }
        let palette = *cx.palette();
        let screens = self.screens(ix);
        let full_page: Option<AnyElement> = match location.page {
            Page::Changes => Some(screens.changes.clone().into_any_element()),
            Page::Compare => Some(screens.compare.clone().into_any_element()),
            Page::ClusterSettings => Some(screens.settings.clone().into_any_element()),
            Page::Settings => Some(screens.app_settings.clone().into_any_element()),
            _ => None,
        };
        if let Some(screen) = full_page {
            return div()
                .flex_1()
                .min_w_0()
                .h_full()
                .bg(palette.editor)
                .child(screen)
                .into_any_element();
        }
        if location.flux_object.is_some() {
            return screens.flux_object.clone().into_any_element();
        }
        let loading = self.load_view(ix, &location, cx);
        let body = if let Some(view) = loading {
            let bar = self.filter_bar_at(ix, &location, cx);
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .children(bar)
                .child(
                    div()
                        .id("page-content")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(view),
                )
                .into_any_element()
        } else if location.page == Page::Pods && self.has_detail(&location) {
            div()
                .flex_1()
                .min_h_0()
                .child(self.screens(ix).pods.clone())
                .into_any_element()
        } else {
            div()
                .id("page-content")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(self.content(ix, &location, &palette, cx))
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(palette.editor)
            .child(self.page_header(ix, &location, &palette, cx))
            .child(body)
            .into_any_element()
    }

    fn workload_screen(
        &mut self,
        ix: usize,
        location: &Location,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<WorkloadScreen>> {
        let pane = self.screens(ix).pane;
        let Some(target) = location.workload().cloned() else {
            self.screens_mut(ix).workload = None;
            return None;
        };
        let compact = self.full_view(ix, cx);
        let current = self
            .screens(ix)
            .workload
            .as_ref()
            .filter(|(screen, _)| screen.read(cx).target() == &target)
            .map(|(screen, _)| screen.clone());
        if let Some(screen) = current {
            screen.update(cx, |screen, cx| screen.set_compact(compact, cx));
            return Some(screen);
        }
        let screen = cx.new(|cx| WorkloadScreen::new(target, window, cx));
        let navigation = self.screens(ix).navigation.clone();
        screen.update(cx, |screen, _| screen.navigation = Some(navigation));
        let subscription = cx.subscribe(&screen, move |shell, _, command: &Command, cx| {
            shell.pane_command(pane, command.clone(), cx);
        });
        screen.update(cx, |screen, cx| {
            screen.set_compact(compact, cx);
            screen.watch(cx);
        });
        self.sync_workload_yaml(&screen, cx);
        self.screens_mut(ix).workload = Some((screen.clone(), subscription));
        Some(screen)
    }

    fn logs_screen(
        &mut self,
        ix: usize,
        location: &Location,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<LogsScreen>> {
        let pane = self.screens(ix).pane;
        let target = location
            .resource
            .clone()
            .zip(location.container.clone())
            .filter(|_| location.page == Page::Logs);
        let Some((pod, container)) = target else {
            self.screens_mut(ix).logs = None;
            return None;
        };
        let compact = self.panes.len() > 1;
        let current = self
            .screens(ix)
            .logs
            .as_ref()
            .filter(|(screen, _)| screen.read(cx).shows(&pod, &container))
            .map(|(screen, _)| screen.clone());
        if let Some(screen) = current {
            screen.update(cx, |screen, cx| screen.set_compact(compact, cx));
            return Some(screen);
        }
        let screen = cx.new(|cx| LogsScreen::new(pod, container, window, cx));
        let navigation = self.screens(ix).navigation.clone();
        screen.update(cx, |screen, _| screen.navigation = Some(navigation));
        let subscription = cx.subscribe(&screen, move |shell, _, command: &Command, cx| {
            shell.pane_command(pane, command.clone(), cx);
        });
        screen.update(cx, |screen, cx| {
            screen.set_compact(compact, cx);
            screen.watch(cx);
        });
        self.screens_mut(ix).logs = Some((screen.clone(), subscription));
        Some(screen)
    }

    fn multi_logs_screen(
        &mut self,
        ix: usize,
        location: &Location,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<MultiLogsScreen>> {
        let pane = self.screens(ix).pane;
        let target = location
            .logs
            .clone()
            .filter(|_| location.page == Page::Logs);
        let Some(target) = target else {
            self.screens_mut(ix).multi_logs = None;
            return None;
        };
        let compact = self.panes.len() > 1;
        let current = self
            .screens(ix)
            .multi_logs
            .as_ref()
            .filter(|(screen, _)| screen.read(cx).shows(&target, cx))
            .map(|(screen, _)| screen.clone());
        if let Some(screen) = current {
            screen.update(cx, |screen, cx| screen.set_compact(compact, cx));
            return Some(screen);
        }
        let screen = cx.new(|cx| MultiLogsScreen::new(target, window, cx));
        let navigation = self.screens(ix).navigation.clone();
        screen.update(cx, |screen, _| screen.navigation = Some(navigation));
        let subscription = cx.subscribe(&screen, move |shell, _, command: &Command, cx| {
            shell.pane_command(pane, command.clone(), cx);
        });
        screen.update(cx, |screen, cx| {
            screen.set_compact(compact, cx);
            screen.watch(cx);
        });
        self.screens_mut(ix).multi_logs = Some((screen.clone(), subscription));
        Some(screen)
    }

    fn unavailable(&self, ix: usize, location: &Location, cx: &mut Context<Self>) -> AnyElement {
        let palette = *cx.palette();
        let name = location
            .cluster
            .as_ref()
            .map(|id| flock_core::tabs::cluster_name(id, &self.state.snapshot))
            .unwrap_or_default();
        let sentence = format!("{name} is not available. {UNAVAILABLE_REASON}");
        let action = Button::new("tab-show-flock", SHOW_ALL_CLUSTERS)
            .on_click(cx.listener(|this, _, _, cx| this.retarget(None, cx)));
        div()
            .debug_selector(|| "cluster-unavailable".to_owned())
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(palette.editor)
            .child(
                crate::kit::PageTitle::new(location.page.title())
                    .leading(Some(self.screens(ix).navigation.clone())),
            )
            .child(EmptyState::new(sentence).action(action))
            .into_any_element()
    }

    fn page_header(
        &self,
        ix: usize,
        location: &Location,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let settings = self
            .cluster_of(location)
            .filter(|_| location.page == Page::ClusterOverview)
            .map(|cluster| {
                let id = cluster.id.clone();
                IconButton::svg(OVERVIEW_SETTINGS, GEAR, CLUSTER_SETTINGS).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.focus_pane(ix, cx);
                        this.dispatch(Command::ShowClusterSettings(id.clone()), cx);
                    },
                ))
            });
        let screens = self.screens(ix);
        let has_detail = self.has_detail(location);
        let loading = matches!(self.page_state(ix, location), Some(WatchState::Pending(_)));
        let note = if loading {
            None
        } else if location.page == Page::Pods {
            Some(screens.pods.read(cx).header_note())
        } else {
            screens.pages.header_note(location.page, has_detail, cx)
        };
        let cluster = self.cluster_of(location);
        let noted = note.is_some();
        let status: Option<Vec<(String, gpui::Hsla)>> = match (cluster, note) {
            (Some(_), Some((counts, text))) => Some(note_parts(&counts, text, palette)),
            (Some(cluster), None) => {
                let text = format!("{} {}", cluster.status.glyph(), cluster.state);
                Some(vec![(text, palette.status(cluster.status))])
            }
            (None, _) if location.page == Page::GitOps => None,
            (None, _) => Some(vec![(LIVE.to_owned(), palette.mute)]),
        };
        let status = status.map(|parts| {
            div()
                .debug_selector(move || {
                    if noted {
                        "page-note"
                    } else {
                        "page-cluster-state"
                    }
                    .to_owned()
                })
                .flex()
                .gap(SPACE_4)
                .font_family(FONT_MONO)
                .text_size(TEXT_SMALL)
                .children(
                    parts
                        .into_iter()
                        .map(|(text, color)| div().text_color(color).child(status_symbols(text))),
                )
        });
        let controls = cluster.and_then(|_| screens.pages.header_controls(location.page, cx));
        div()
            .flex()
            .items_center()
            .flex_none()
            .gap(SPACE_5)
            .h(PAGE_HEADER)
            .px(SPACE_8)
            .border_b(RULE)
            .border_color(palette.line)
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .child(screens.navigation.clone())
            .child(
                div()
                    .text_size(TEXT_TITLE)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(location.page.title()),
            )
            .children(status)
            .children(settings)
            .child(div().flex_1())
            .children(controls)
    }

    fn content(
        &self,
        ix: usize,
        location: &Location,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let empty = self.loaded
            && self.state.snapshot.clusters.is_empty()
            && location.page == Page::Overview;
        if empty {
            let add = cx.listener(|this, _, window, cx| {
                this.open_add_cluster(AddClusterStep::Pick, window, cx);
            });
            return empty_flock(palette, add).into_any_element();
        }
        let screens = self.screens(ix);
        match location.page {
            Page::Overview => screens.flock.clone().into_any_element(),
            Page::Problems => screens.flock.clone().into_any_element(),
            Page::GitOps => screens.gitops.clone().into_any_element(),
            page => screens
                .pages
                .render(page, self.has_detail(location))
                .unwrap_or_else(|| self.placeholder(location, palette)),
        }
    }

    pub(super) fn has_detail(&self, location: &Location) -> bool {
        self.cluster_of(location)
            .is_some_and(|c| self.state.snapshot.detail(&c.id).is_some())
    }

    fn placeholder(&self, location: &Location, palette: &Palette) -> AnyElement {
        if self.cluster_of(location).is_some() {
            return Skeleton::rows().into_any_element();
        }
        let scope = plural(self.state.snapshot.clusters.len(), "cluster", "clusters");
        div()
            .flex()
            .flex_col()
            .gap(SPACE_2)
            .px(SPACE_8)
            .pt(SPACE_7)
            .child(
                div()
                    .text_size(TEXT_LABEL)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.mute)
                    .child(location.page.label().to_uppercase()),
            )
            .child(div().text_color(palette.fg2).child(scope))
            .into_any_element()
    }
}

fn fill(screen: impl IntoElement) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .h_full()
        .child(screen)
        .into_any_element()
}

fn note_parts(counts: &Counts, text: String, palette: &Palette) -> Vec<(String, gpui::Hsla)> {
    let mut parts: Vec<(String, gpui::Hsla)> = counts
        .badges()
        .into_iter()
        .map(|(status, n)| (format!("{} {n}", status.glyph()), palette.status(status)))
        .collect();
    let text = if parts.is_empty() {
        text
    } else {
        format!("{SEPARATOR}{text}")
    };
    parts.push((text, palette.mute));
    parts
}
