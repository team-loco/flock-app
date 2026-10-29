use flock_core::history::Location;
use flock_core::overview::plural;
use flock_core::tabs::TabId;
use flock_core::watch::WatchState;
use flock_core::{ClusterId, Command, Page, ResourceRef};
use gpui::{
    AnyElement, AppContext, Context, Entity, FontWeight, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Styled, Subscription, Window, div,
};

use flock_core::AddClusterStep;

use super::Shell;
use crate::assets::GEAR;
use crate::kit::{Button, EmptyState, IconButton, Skeleton, scroll_viewport, status_symbols};
use crate::screens::ScreenEvent;
use crate::screens::add_cluster::empty_flock;
use crate::screens::detail::DetailScreen;
use crate::screens::edit::EditScreen;
use crate::screens::logs::LogsScreen;
use crate::screens::logs_page::LogsPage;
use crate::screens::namespace::NamespaceScreen;
use crate::screens::workload::WorkloadScreen;
use crate::theme::{
    ActiveTheme, FONT_MONO, PAGE_HEADER, Palette, RULE, SPACE_2, SPACE_5, SPACE_7, SPACE_8,
    TEXT_LABEL, TEXT_SMALL, TEXT_TITLE,
};

const UNAVAILABLE_REASON: &str =
    "It was removed from Flock or its kubeconfig context is gone, so this tab has nothing to show.";
const SHOW_ALL_CLUSTERS: &str = "Show all clusters";
const OVERVIEW_SETTINGS: &str = "overview-settings";
const PAGE_CONTENT: &str = "page-content";
const PAGE_HEADER_SELECTOR: &str = "page-header";
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
        if let Some(screen) = self.edit_screen(ix, &location, window, cx) {
            return fill(screen);
        }
        if let Some(screen) = self.workload_screen(ix, &location, window, cx) {
            return fill(screen);
        }
        if let Some(screen) = self.detail_page(ix, &location, window, cx) {
            return screen;
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
        if location.helm.is_some() {
            return screens.helm_release.clone().into_any_element();
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
                .child(scroll_viewport(PAGE_CONTENT, view).flex_1().min_h_0())
                .into_any_element()
        } else if location.page == Page::Pods && self.has_detail(&location) {
            div()
                .flex_1()
                .min_h_0()
                .child(self.screens(ix).pods.clone())
                .into_any_element()
        } else {
            scroll_viewport(PAGE_CONTENT, self.content(ix, &location, &palette, cx))
                .flex_1()
                .min_h_0()
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

    fn detail_page(
        &mut self,
        ix: usize,
        location: &Location,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if let Some(screen) = self.namespace_screen(ix, location, window, cx) {
            self.screens_mut(ix).detail = None;
            return Some(fill(screen));
        }
        self.detail_screen(ix, location, cx).map(fill)
    }

    fn namespace_screen(
        &mut self,
        ix: usize,
        location: &Location,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<NamespaceScreen>> {
        let pane = self.screens(ix).pane;
        let target = location
            .detail()
            .filter(|target| target.kind == flock_core::related::NAMESPACE)
            .cloned();
        let Some(target) = target else {
            self.screens_mut(ix).namespace = None;
            return None;
        };
        let current = self
            .screens(ix)
            .namespace
            .as_ref()
            .filter(|(screen, _)| screen.read(cx).target() == &target)
            .map(|(screen, _)| screen.clone());
        if let Some(screen) = current {
            return Some(screen);
        }
        let screen = cx.new(|cx| NamespaceScreen::new(target, window, cx));
        let navigation = self.screens(ix).navigation.clone();
        let subscription = cx.subscribe(&screen, move |shell, _, command: &Command, cx| {
            shell.pane_command(pane, command.clone(), cx);
        });
        let events = cx.subscribe_in(
            &screen,
            window,
            move |shell, _, event: &ScreenEvent, window, cx| {
                shell.pane_screen_event(pane, event, window, cx);
            },
        );
        screen.update(cx, |screen, _| screen.navigation = Some(navigation));
        self.sync_namespace(&screen, location, cx);
        self.screens_mut(ix).namespace =
            Some((screen.clone(), Subscription::join(subscription, events)));
        Some(screen)
    }

    fn detail_screen(
        &mut self,
        ix: usize,
        location: &Location,
        cx: &mut Context<Self>,
    ) -> Option<Entity<DetailScreen>> {
        let pane = self.screens(ix).pane;
        let Some(target) = location.detail().cloned() else {
            self.screens_mut(ix).detail = None;
            return None;
        };
        let current = self
            .screens(ix)
            .detail
            .as_ref()
            .filter(|(screen, _)| screen.read(cx).target() == &target)
            .map(|(screen, _)| screen.clone());
        if let Some(screen) = current {
            return Some(screen);
        }
        let screen = cx.new(|cx| DetailScreen::new(target, cx));
        let navigation = self.screens(ix).navigation.clone();
        let subscription = cx.subscribe(&screen, move |shell, _, command: &Command, cx| {
            shell.pane_command(pane, command.clone(), cx);
        });
        screen.update(cx, |screen, _| screen.navigation = Some(navigation));
        self.sync_detail(&screen, location, cx);
        self.screens_mut(ix).detail = Some((screen.clone(), subscription));
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
    ) -> Option<Entity<LogsPage>> {
        let pane = self.screens(ix).pane;
        let target = location
            .logs
            .clone()
            .filter(|_| location.page == Page::Logs);
        let Some(target) = target else {
            self.screens_mut(ix).multi_logs = None;
            return None;
        };
        let all: Vec<ClusterId> = self
            .state
            .snapshot
            .clusters
            .iter()
            .map(|cluster| cluster.id.clone())
            .collect();
        let clusters = location.scope().selected(&all);
        let compact = self.panes.len() > 1;
        let snapshot = self.state.snapshot.clone();
        let current = self
            .screens(ix)
            .multi_logs
            .as_ref()
            .filter(|(screen, _)| screen.read(cx).shows(&target, &clusters))
            .map(|(screen, _)| screen.clone());
        if let Some(screen) = current {
            screen.update(cx, |screen, cx| {
                screen.set_compact(compact, cx);
                screen.set_snapshot(&snapshot, cx);
            });
            return Some(screen);
        }
        let screen = cx.new(|cx| LogsPage::new(target, clusters, window, cx));
        let navigation = self.screens(ix).navigation.clone();
        screen.update(cx, |screen, _| screen.navigation = Some(navigation));
        let subscription = cx.subscribe(&screen, move |shell, _, command: &Command, cx| {
            shell.pane_command(pane, command.clone(), cx);
        });
        screen.update(cx, |screen, cx| {
            screen.set_compact(compact, cx);
            screen.set_snapshot(&snapshot, cx);
            screen.watch(cx);
        });
        self.screens_mut(ix).multi_logs = Some((screen.clone(), subscription));
        Some(screen)
    }

    fn edit_screen(
        &mut self,
        ix: usize,
        location: &Location,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<EditScreen>> {
        let pane = self.screens(ix).pane;
        let tabs = self.state.panes().panes()[ix].tabs();
        let editing: Vec<(TabId, ResourceRef)> = tabs
            .iter()
            .filter_map(|tab| {
                tab.location()
                    .editing()
                    .map(|target| (tab.id(), target.clone()))
            })
            .collect();
        let tab = self.state.panes().panes()[ix].active().id();
        self.screens_mut(ix).edits.retain(|(id, screen, _)| {
            editing
                .iter()
                .any(|(open, target)| open == id && screen.read(cx).target() == target)
        });
        let target = location.editing().cloned()?;
        let confirm_apply = self
            .state
            .snapshot
            .catalog
            .get(&target.cluster)
            .is_none_or(|settings| settings.preferences.confirm_destructive);
        let current = self
            .screens(ix)
            .edits
            .iter()
            .find(|(id, _, _)| *id == tab)
            .map(|(_, screen, _)| screen.clone());
        if let Some(screen) = current {
            screen.update(cx, |screen, _| screen.set_confirm_apply(confirm_apply));
            return Some(screen);
        }
        let screen = cx.new(|cx| EditScreen::new(target, window, cx));
        let navigation = self.screens(ix).navigation.clone();
        let subscription = cx.subscribe(&screen, move |shell, _, command: &Command, cx| {
            shell.pane_command(pane, command.clone(), cx);
        });
        screen.update(cx, |screen, cx| {
            screen.navigation = Some(navigation);
            screen.set_confirm_apply(confirm_apply);
            screen.load(cx);
        });
        self.screens_mut(ix)
            .edits
            .push((tab, screen.clone(), subscription));
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
        let action = Button::new("tab-show-flock", SHOW_ALL_CLUSTERS).on_click(
            cx.listener(|this, _, _, cx| this.retarget(&flock_core::scope::Scope::All, cx)),
        );
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
        let status = cluster.and(note).map(|text| {
            div()
                .debug_selector(|| "page-note".to_owned())
                .font_family(FONT_MONO)
                .text_size(TEXT_SMALL)
                .text_color(palette.mute)
                .child(status_symbols(text))
        });
        let controls = cluster.and_then(|_| screens.pages.header_controls(location.page, cx));
        div()
            .debug_selector(|| PAGE_HEADER_SELECTOR.to_owned())
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
                    .child(page_title(location)),
            )
            .children(location.crd.as_ref().map(|crd| {
                div()
                    .debug_selector(|| "page-crd-group".to_owned())
                    .font_family(FONT_MONO)
                    .text_size(TEXT_SMALL)
                    .text_color(palette.mute)
                    .child(crd.group.clone())
            }))
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
            return empty_flock(add).into_any_element();
        }
        let screens = self.screens(ix);
        match location.page {
            Page::Overview => screens.flock.clone().into_any_element(),
            Page::Problems => screens.problems.clone().into_any_element(),
            Page::GitOps => screens.gitops.clone().into_any_element(),
            Page::FluxEvents => screens.flux_events.clone().into_any_element(),
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

fn page_title(location: &Location) -> String {
    location
        .crd
        .as_ref()
        .map_or_else(|| location.page.title().to_owned(), |crd| crd.kind.clone())
}

fn fill(screen: impl IntoElement) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .h_full()
        .child(screen)
        .into_any_element()
}
