pub mod add_cluster;
pub mod app_settings;
pub mod changes;
pub mod command;
pub mod compare;
mod event_inspector;
mod events;
pub mod flock;
mod flux_list;
pub(crate) mod flux_object;
pub mod gitops;
pub mod logs;
pub mod multilog;
mod namespaces;
mod nodes;
mod overview;
pub mod pods;
mod resources;
pub mod settings;
pub mod workload;

use flock_core::emptiness::{Emptiness, Noun};
use flock_core::{ClusterSummary, Command, Counts, Page, ResourceKind, ResourceRef, Snapshot};
use gpui::{
    AnyElement, App, AppContext, Context, Entity, FocusHandle, IntoElement, Subscription, Window,
};

use crate::kit::{EmptyState, FilterBar, Link, MenuItem, ShowAll};

pub use events::EventsScreen;
pub use flux_list::FluxListScreen;
pub use namespaces::NamespacesScreen;
pub use nodes::NodesScreen;
pub use overview::OverviewScreen;
pub use resources::ResourcesScreen;

pub(crate) struct ListShape<'a> {
    pub shown: usize,
    pub total: usize,
    pub namespace: Option<(&'a str, usize)>,
    pub noun: Noun,
    pub cluster: &'a str,
}

pub(crate) fn empty_list(
    shape: &ListShape,
    filter: &Entity<FilterBar>,
    show_all: Option<ShowAll>,
    cx: &App,
) -> Option<EmptyState> {
    if shape.shown > 0 {
        return None;
    }
    let text = filter.read(cx).text(cx);
    let emptiness = Emptiness::classify(shape.total, shape.namespace, &text);
    Some(EmptyState::for_list(
        &emptiness,
        shape.noun,
        shape.cluster,
        filter,
        show_all,
    ))
}

pub const NODE_KIND: &str = "Node";
pub const OPEN_FULL_VIEW: &str = "Open full view →";

pub type FullView = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

pub fn full_view_link(id: &'static str, open: &FullView) -> Link {
    let open = open.clone();
    Link::new(id, OPEN_FULL_VIEW).on_click(move |_, window, cx| open(window, cx))
}
pub const OPEN_IN_NEW_TAB: &str = "Open in new tab";
const NEW_TAB_DESCRIPTION: &str = "Keep this page and open it in a tab beside it.";

pub fn new_tab_item(open: Command, emit: impl Fn(Command, &mut App) + 'static) -> MenuItem {
    let command = Command::NewTab(Box::new(open));
    MenuItem::new(OPEN_IN_NEW_TAB, move |_, cx| emit(command.clone(), cx))
        .description(NEW_TAB_DESCRIPTION)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenEvent {
    Command(Command),
    Navigate(Page),
    ShowEvents(flock_core::events::EventFilter),
}

pub struct ClusterPages {
    overview: Entity<OverviewScreen>,
    nodes: Entity<NodesScreen>,
    namespaces: Entity<NamespacesScreen>,
    events: Entity<EventsScreen>,
    resources: Entity<ResourcesScreen>,
    flux: Entity<FluxListScreen>,
    _subscriptions: Vec<Subscription>,
}

impl ClusterPages {
    pub fn new<V: 'static>(
        window: &mut Window,
        cx: &mut Context<V>,
        on_event: impl Fn(&mut V, &ScreenEvent, &mut Context<V>) + Clone + 'static,
    ) -> Self {
        let overview = cx.new(OverviewScreen::new);
        let nodes = cx.new(|cx| NodesScreen::new(window, cx));
        let namespaces = cx.new(|cx| NamespacesScreen::new(window, cx));
        let events = cx.new(|cx| EventsScreen::new(window, cx));
        let resources = cx.new(|cx| ResourcesScreen::new(window, cx));
        let flux = cx.new(|cx| FluxListScreen::new(window, cx));
        let subscriptions = vec![
            subscribe(&flux, on_event.clone(), cx),
            subscribe(&overview, on_event.clone(), cx),
            subscribe(&nodes, on_event.clone(), cx),
            subscribe(&namespaces, on_event.clone(), cx),
            subscribe(&events, on_event.clone(), cx),
            subscribe(&resources, on_event, cx),
        ];
        Self {
            overview,
            nodes,
            namespaces,
            events,
            resources,
            flux,
            _subscriptions: subscriptions,
        }
    }

    pub fn sync(
        &self,
        page: Page,
        cluster: Option<&ClusterSummary>,
        snapshot: &Snapshot,
        confirm_destructive: bool,
        preferences: &flock_core::Preferences,
        cx: &mut App,
    ) {
        if let Some(cluster) = cluster
            && is_flux_list(page)
        {
            self.flux.update(cx, |screen, cx| {
                screen.set(cluster, page, &snapshot.flux, cx);
            });
            return;
        }
        let detail = cluster.and_then(|cluster| snapshot.detail(&cluster.id));
        let (Some(cluster), Some(detail)) = (cluster, detail) else {
            return;
        };
        match page {
            Page::ClusterOverview => self
                .overview
                .update(cx, |screen, cx| screen.set(cluster, detail, cx)),
            Page::Nodes => self.nodes.update(cx, |screen, cx| {
                screen.set(cluster, detail, confirm_destructive, cx);
            }),
            Page::Namespaces => self
                .namespaces
                .update(cx, |screen, cx| screen.set(cluster, detail, cx)),
            Page::Events => self.events.update(cx, |screen, cx| {
                screen.set(cluster, detail, preferences, cx);
            }),
            other => {
                if let Some(kind) = ResourceKind::for_page(other) {
                    self.resources.update(cx, |screen, cx| {
                        screen.set(cluster, kind, detail, confirm_destructive, preferences, cx);
                    });
                }
            }
        }
    }

    pub fn reveal(&self, target: &ResourceRef, cx: &mut App) {
        if target.kind == NODE_KIND {
            let name = target.name.clone();
            let cluster = target.cluster.clone();
            self.nodes
                .update(cx, |screen, cx| screen.reveal(cluster, name, cx));
            return;
        }
        let kind = ResourceKind::ALL
            .into_iter()
            .find(|kind| kind.name() == target.kind);
        if let (Some(kind), Some(namespace)) = (kind, target.namespace.clone()) {
            let name = target.name.clone();
            self.resources
                .update(cx, |screen, cx| screen.reveal(kind, namespace, name, cx));
        }
    }

    pub fn render(&self, page: Page, has_detail: bool) -> Option<AnyElement> {
        if is_flux_list(page) {
            return Some(self.flux.clone().into_any_element());
        }
        if !has_detail {
            return None;
        }
        let element = match page {
            Page::ClusterOverview => self.overview.clone().into_any_element(),
            Page::Nodes => self.nodes.clone().into_any_element(),
            Page::Namespaces => self.namespaces.clone().into_any_element(),
            Page::Events => self.events.clone().into_any_element(),
            other => {
                ResourceKind::for_page(other)?;
                self.resources.clone().into_any_element()
            }
        };
        Some(element)
    }

    pub fn filter_bar(&self, page: Page, cx: &App) -> Option<Entity<FilterBar>> {
        let bar = match page {
            Page::Nodes => self.nodes.read(cx).filter_bar(),
            Page::Namespaces => self.namespaces.read(cx).filter_bar(),
            Page::Events => self.events.read(cx).filter_bar(),
            other if is_flux_list(other) => self.flux.read(cx).filter_bar(),
            other => {
                ResourceKind::for_page(other)?;
                self.resources.read(cx).filter_bar()
            }
        };
        Some(bar.clone())
    }

    pub fn close_inspector(&self, page: Page, cx: &mut App) -> bool {
        match page {
            Page::Nodes => self.nodes.update(cx, NodesScreen::close_inspector),
            Page::Events => self.events.update(cx, EventsScreen::close_inspector),
            Page::ClusterOverview => self.overview.update(cx, OverviewScreen::close_inspector),
            other if ResourceKind::for_page(other).is_some() => {
                self.resources.update(cx, ResourcesScreen::close_inspector)
            }
            _ => false,
        }
    }

    pub fn show_event_filter(&self, filter: flock_core::events::EventFilter, cx: &mut App) {
        self.events
            .update(cx, |screen, cx| screen.show_filter(filter, cx));
    }

    pub fn list_focus(&self, page: Page, cx: &mut App) -> Option<FocusHandle> {
        match page {
            Page::Nodes => Some(self.nodes.read(cx).list_focus(cx)),
            Page::Namespaces => Some(self.namespaces.read(cx).list_focus(cx)),
            Page::Events => Some(self.events.read(cx).list_focus(cx)),
            other if is_flux_list(other) => Some(self.flux.read(cx).list_focus(cx)),
            other => {
                ResourceKind::for_page(other)?;
                Some(self.resources.update(cx, ResourcesScreen::list_focus))
            }
        }
    }

    pub fn header_controls(&self, page: Page, cx: &App) -> Option<AnyElement> {
        (page == Page::ClusterOverview)
            .then(|| OverviewScreen::header_controls(&self.overview, cx).into_any_element())
    }

    pub fn header_note(&self, page: Page, has_detail: bool, cx: &App) -> Option<(Counts, String)> {
        if is_flux_list(page) {
            return Some(self.flux.read(cx).header_note());
        }
        if !has_detail {
            return None;
        }
        match page {
            Page::Nodes => Some(self.nodes.read(cx).header_note()),
            Page::Namespaces => Some(self.namespaces.read(cx).header_note()),
            Page::Events => Some(self.events.read(cx).header_note()),
            other => {
                ResourceKind::for_page(other)?;
                Some(self.resources.read(cx).header_note())
            }
        }
    }
}

pub fn is_flux_list(page: Page) -> bool {
    matches!(
        page,
        Page::Kustomizations | Page::HelmReleases | Page::Sources
    )
}

fn subscribe<V: 'static, S: gpui::EventEmitter<ScreenEvent>>(
    screen: &Entity<S>,
    on_event: impl Fn(&mut V, &ScreenEvent, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> Subscription {
    cx.subscribe(screen, move |view, _, event, cx| on_event(view, event, cx))
}

pub(crate) fn init(_: &mut App) {}
#[cfg(feature = "loco")]
pub mod loco;
