use std::time::Instant;

use flock_core::history::Location;
use flock_core::watch::{NOT_STARTED, WATCH_SYNC_TIMEOUT};
use flock_core::{ClusterId, Command, Page, Status, WatchState};
use gpui::{AnyElement, Context, IntoElement, ParentElement, Task, div};

use super::Shell;
use crate::kit::{Button, ConnectionProgress, ErrorState, Skeleton};

const RETRY: &str = "Retry";

#[derive(Default)]
pub(super) struct LoadClock {
    key: Option<(ClusterId, Page)>,
    since: Option<Instant>,
    wake: Option<Task<()>>,
}

impl Shell {
    fn tracked(location: &Location) -> Option<(ClusterId, Page)> {
        let page = location.page;
        let cluster = location.cluster.clone()?;
        let tracked = !page.is_flock()
            && page != Page::Compare
            && location.resource.is_none()
            && location.flux_object.is_none();
        tracked.then_some((cluster, page))
    }

    fn reported(&self, cluster: &ClusterId, page: Page) -> WatchState {
        let snapshot = &self.state.snapshot;
        if let Some(state) = snapshot.page_state(cluster, page) {
            return state.clone();
        }
        let Some(summary) = snapshot.cluster(cluster) else {
            return WatchState::pending(NOT_STARTED);
        };
        if summary.nodes.is_none() && summary.status != Status::Progress {
            return WatchState::Failed(format!("{}: {}", summary.name, summary.state));
        }
        if snapshot.detail(cluster).is_some() {
            WatchState::Synced
        } else {
            WatchState::pending(NOT_STARTED)
        }
    }

    pub(super) fn track_load(&mut self, cx: &mut Context<Self>) {
        for ix in 0..self.panes.len() {
            self.track_pane_load(ix, cx);
        }
    }

    fn track_pane_load(&mut self, ix: usize, cx: &mut Context<Self>) {
        let tracked = Self::tracked(self.location_at(ix));
        let pending = tracked.as_ref().is_some_and(|(cluster, page)| {
            matches!(self.reported(cluster, *page), WatchState::Pending(_))
        });
        let load = &mut self.screens_mut(ix).load;
        if load.key != tracked {
            *load = LoadClock {
                key: tracked,
                ..LoadClock::default()
            };
        }
        if !pending {
            load.since = None;
            load.wake = None;
            return;
        }
        if load.since.is_some() {
            return;
        }
        load.since = Some(Instant::now());
        load.wake = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(WATCH_SYNC_TIMEOUT).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        }));
    }

    pub(super) fn page_state(&self, ix: usize, location: &Location) -> Option<WatchState> {
        let (cluster, page) = Self::tracked(location)?;
        let reported = self.reported(&cluster, page);
        let waited = self.screens(ix).load.since.map(|since| since.elapsed());
        Some(reported.expire(waited.unwrap_or_default()))
    }

    pub(super) fn load_view(
        &self,
        ix: usize,
        location: &Location,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let state = self.page_state(ix, location)?;
        let cluster = self.cluster_of(location)?;
        let id = cluster.id.clone();
        let page = location.page;
        let element = match state {
            WatchState::Synced => return None,
            WatchState::Refreshing(_) => return None,
            WatchState::Failed(reason) => {
                let retry = Button::new("page-retry", RETRY)
                    .on_click(cx.listener(move |this, _, _, cx| this.retry(ix, id.clone(), cx)));
                ErrorState::raw(&reason, retry).into_any_element()
            }
            WatchState::Pending(step) => {
                let progress = (cluster.status == Status::Progress)
                    .then(|| ConnectionProgress::new(cluster.name.clone(), step));
                let skeleton = if page == Page::ClusterOverview {
                    Skeleton::blocks()
                } else {
                    Skeleton::rows()
                };
                div().children(progress).child(skeleton).into_any_element()
            }
        };
        Some(element)
    }

    fn retry(&mut self, ix: usize, cluster: ClusterId, cx: &mut Context<Self>) {
        if let Some(screens) = self.panes.get_mut(ix) {
            screens.load = LoadClock::default();
        }
        self.dispatch(Command::Connect(cluster), cx);
    }
}
