mod changes;
mod charts;
mod facts;
mod header;
mod namespaces;
mod pods;
mod pools;
mod summary;
mod workloads;

use flock_core::cluster::{Namespace, NamespaceSort};
use flock_core::cluster_changes::{ClusterChange, newest_first};
use flock_core::node_pools::{KubeletSkew, NodePool};
use flock_core::nodes::NodeSummary;
use flock_core::overview::{
    Capacity, NetworkFacts, PodPhases, TimeRange, Utilization, UtilizationSource, WorkloadHealth,
    plural,
};
use flock_core::pod_tops::PodTops;
use flock_core::{
    ClusterDetail, ClusterId, ClusterSummary, Command, Counts, Page, ResourceRef, Status,
};
use gpui::{
    AnyElement, App, AppContext, Context, Div, ElementId, Entity, EventEmitter, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, Stateful, StatefulInteractiveElement, Styled,
    Window, div,
};

use super::ScreenEvent;
use crate::kit::{ActionMenu, Button, Link, MenuItem, SectionLabel, Segmented};
use crate::theme::{
    ActiveTheme, FONT_MONO, Palette, RULE, SPACE_3, SPACE_4, SPACE_5, SPACE_7, SPACE_8,
    TEXT_CONTROL, TEXT_SMALL,
};

const LIVE: &str = "Live";
const LIVE_TOOLTIP: &str = "Follow new data; turn off to hold the current view";
const SEPARATOR: &str = " · ";
const ACTIONS: &str = "overview-actions";
const SCROLL: &str = "overview-scroll";
const CLUSTER_SETTINGS: &str = "Cluster settings";
const CLUSTER_SETTINGS_DESCRIPTION: &str =
    "Change this cluster's default namespace, metrics source and labels.";

#[derive(Clone, Debug, PartialEq)]
struct OverviewData {
    cluster: ClusterId,
    meta: String,
    problems: Counts,
    flux: Option<header::FluxLine>,
    source: UtilizationSource,
    cpu: Capacity,
    memory: Capacity,
    pods: PodPhases,
    nodes: NodeSummary,
    node_health: Counts,
    pools: Vec<NodePool>,
    skew: Option<KubeletSkew>,
    network: NetworkFacts,
    utilization: Vec<Utilization>,
    workloads: Vec<WorkloadHealth>,
    tops: PodTops,
    namespaces: Vec<Namespace>,
    changes: Vec<ClusterChange>,
}

impl OverviewData {
    fn new(cluster: &ClusterSummary, detail: &ClusterDetail) -> Self {
        let namespaces = plural(detail.namespaces.len(), "namespace", "namespaces");
        let meta = [
            cluster.distro.clone(),
            cluster.region.clone(),
            namespaces,
            detail.overview.metrics_source.clone(),
        ];
        let meta: Vec<String> = meta.into_iter().filter(|part| !part.is_empty()).collect();
        let mut changes = detail.overview.changes.clone();
        newest_first(&mut changes);
        Self {
            cluster: cluster.id.clone(),
            meta: meta.join(SEPARATOR),
            problems: cluster.problems,
            flux: header::FluxLine::of(cluster),
            source: detail.overview.utilization_source.clone(),
            cpu: detail.cpu(),
            memory: detail.memory(),
            pods: detail.overview.pods,
            nodes: detail.node_summary(),
            node_health: Counts::of(detail.nodes.iter().map(|n| n.status)),
            pools: NodePool::of(&detail.nodes),
            skew: cluster
                .version
                .as_deref()
                .and_then(|version| KubeletSkew::of(&detail.nodes, version)),
            network: detail.overview.network,
            utilization: detail.overview.utilization.clone(),
            workloads: detail.workload_health(),
            tops: detail.overview.tops.clone(),
            namespaces: detail.namespaces.clone(),
            changes,
        }
    }
}

pub struct OverviewScreen {
    data: Option<OverviewData>,
    paused: Option<OverviewData>,
    live: bool,
    range: TimeRange,
    sort: NamespaceSort,
    menu: Entity<ActionMenu>,
}

impl EventEmitter<ScreenEvent> for OverviewScreen {}

impl OverviewScreen {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let menu = cx.new(|cx| ActionMenu::new(ACTIONS, cx));
        Self {
            data: None,
            paused: None,
            live: true,
            range: TimeRange::default(),
            sort: NamespaceSort::default(),
            menu,
        }
    }

    pub fn set(
        &mut self,
        cluster: &ClusterSummary,
        detail: &ClusterDetail,
        cx: &mut Context<Self>,
    ) {
        let data = OverviewData::new(cluster, detail);
        if self.live || self.data.is_none() {
            self.show(data, cx);
        } else {
            self.paused = Some(data);
        }
    }

    fn show(&mut self, data: OverviewData, cx: &mut Context<Self>) {
        if self.data.as_ref() == Some(&data) {
            return;
        }
        if self
            .data
            .as_ref()
            .is_none_or(|shown| shown.cluster != data.cluster)
        {
            self.set_actions(data.cluster.clone(), cx);
        }
        self.data = Some(data);
        cx.notify();
    }

    fn set_actions(&self, cluster: ClusterId, cx: &mut Context<Self>) {
        let screen = cx.entity().downgrade();
        let settings = MenuItem::new(CLUSTER_SETTINGS, move |_, cx| {
            let command = Command::ShowClusterSettings(cluster.clone());
            let sent = screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)));
            if sent.is_err() {
                tracing::debug!("overview closed before opening cluster settings");
            }
        })
        .description(CLUSTER_SETTINGS_DESCRIPTION);
        self.menu
            .update(cx, |menu, cx| menu.set_items(vec![settings], cx));
    }

    fn toggle_live(&mut self, cx: &mut Context<Self>) {
        self.live = !self.live;
        if let Some(data) = self.paused.take().filter(|_| self.live) {
            self.show(data, cx);
        }
        cx.notify();
    }

    pub fn header_controls(screen: &Entity<Self>, cx: &App) -> impl IntoElement {
        let this = screen.read(cx);
        let selected = TimeRange::ALL
            .iter()
            .position(|r| *r == this.range)
            .unwrap_or_default();
        let ranges_screen = screen.clone();
        let ranges = TimeRange::ALL
            .iter()
            .fold(Segmented::new("range").small().mono(), |seg, r| {
                seg.option(r.label())
            })
            .selected(selected)
            .on_select(move |ix, _, cx| {
                ranges_screen.update(cx, |screen, cx| screen.select_range(*ix, cx));
            });
        let live_screen = screen.clone();
        let live = Button::new("overview-live", LIVE)
            .small()
            .selected(this.live)
            .tooltip(LIVE_TOOLTIP)
            .on_click(move |_, _, cx| {
                live_screen.update(cx, OverviewScreen::toggle_live);
            });
        div()
            .flex()
            .items_center()
            .gap(SPACE_4)
            .child(ranges)
            .child(live)
            .child(this.menu.clone())
    }

    fn select_range(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(range) = TimeRange::ALL.get(ix) {
            self.range = *range;
            cx.notify();
        }
    }

    fn sort_by(&mut self, sort: NamespaceSort, cx: &mut Context<Self>) {
        self.sort = sort;
        cx.notify();
    }

    fn navigate(page: Page, cx: &mut Context<Self>) -> impl Fn(&mut Window, &mut App) + 'static {
        let this = cx.entity().downgrade();
        move |_, cx| {
            this.update(cx, |_, cx| cx.emit(ScreenEvent::Navigate(page)))
                .ok();
        }
    }

    fn open(
        target: ResourceRef,
        cx: &mut Context<Self>,
    ) -> impl Fn(&mut Window, &mut App) + 'static {
        let this = cx.entity().downgrade();
        move |_, cx| {
            let command = Command::OpenResource(target.clone());
            this.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)))
                .ok();
        }
    }
}

impl Render for OverviewScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let Some(data) = self.data.clone() else {
            return div().into_any_element();
        };
        let changes_band: Vec<AnyElement> = changes::render(&data, self.range, &palette, cx)
            .into_iter()
            .chain(pools::render(&data, &palette, cx))
            .collect();
        let sections = [
            Some(header::render(&data, &palette, cx)),
            Some(summary::render(&data, &palette, cx)),
            band(changes_band, &palette),
            band(pods::render(&data, &palette, cx), &palette),
            Some(namespaces::render(&data, self.sort, &palette, cx)),
            band(facts::render(&data, &palette, cx), &palette),
            Some(workloads::render(&data, &palette, cx)),
            Some(charts::render(&data, self.range, &palette, cx)),
        ];
        let content = div()
            .flex()
            .flex_col()
            .tab_group()
            .pb(SPACE_8)
            .children(sections.into_iter().flatten());
        div()
            .debug_selector(|| SCROLL.to_owned())
            .id(SCROLL)
            .size_full()
            .overflow_y_scroll()
            .child(content)
            .into_any_element()
    }
}

fn band(sections: Vec<AnyElement>, palette: &Palette) -> Option<AnyElement> {
    if sections.is_empty() {
        return None;
    }
    let last = sections.len() - 1;
    let cells = sections.into_iter().enumerate().map(|(ix, section)| {
        let cell = div().flex_1().min_w_0().flex().flex_col().child(section);
        if ix < last {
            cell.border_r(RULE).border_color(palette.line)
        } else {
            cell
        }
    });
    let row = div()
        .flex()
        .border_b(RULE)
        .border_color(palette.line)
        .children(cells);
    Some(row.into_any_element())
}

fn section(palette: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .px(SPACE_8)
        .pt(SPACE_7)
        .pb(SPACE_5)
        .text_color(palette.fg)
}

fn section_head(label: &str, meta: Option<String>, link: Option<Link>, palette: &Palette) -> Div {
    let meta = meta.map(|meta| {
        div()
            .font_family(FONT_MONO)
            .text_size(TEXT_SMALL)
            .text_color(palette.mute)
            .truncate()
            .child(meta)
    });
    let link = link.map(|link| div().text_size(TEXT_CONTROL).child(link));
    div()
        .flex()
        .items_center()
        .gap(SPACE_4)
        .mb(SPACE_3)
        .child(SectionLabel::new(label.to_owned()))
        .children(meta)
        .child(div().flex_1())
        .children(link)
}

fn row_link(
    id: impl Into<ElementId>,
    activate: impl Fn(&mut Window, &mut App) + 'static,
    palette: &Palette,
) -> Stateful<Div> {
    let id = id.into();
    let selector = id.to_string();
    let hover = palette.hover;
    div()
        .debug_selector(|| selector)
        .id(id)
        .cursor_pointer()
        .border_t(RULE)
        .border_color(palette.line)
        .hover(move |style| style.bg(hover))
        .on_click(move |_, window, cx| activate(window, cx))
}

fn mono(text: impl Into<SharedString>, color: gpui::Hsla) -> Div {
    div()
        .font_family(FONT_MONO)
        .text_color(color)
        .whitespace_nowrap()
        .child(text.into())
}

fn status_color(status: Option<Status>, otherwise: gpui::Hsla, palette: &Palette) -> gpui::Hsla {
    status.map_or(otherwise, |status| palette.status(status))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn summary() -> ClusterSummary {
        ClusterSummary {
            id: ClusterId::new("prod-us-east"),
            name: "prod-us-east".into(),
            status: Status::Err,
            state: "Degraded".into(),
            distro: "EKS".into(),
            region: "us-east-1".into(),
            version: Some("v1.31.2".into()),
            nodes: Some(14),
            problems: Counts::default(),
            nav: BTreeMap::new(),
            metrics: None,
        }
    }

    #[test]
    fn meta_line_reads_like_the_design() {
        let mut detail = ClusterDetail::default();
        detail.overview.metrics_source = "monitoring/prometheus-k8s".into();
        let data = OverviewData::new(&summary(), &detail);
        assert_eq!(
            data.meta,
            "EKS · us-east-1 · 0 namespaces · monitoring/prometheus-k8s"
        );
        assert_eq!(data.flux, None);
    }
}
