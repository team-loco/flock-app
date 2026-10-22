mod api_drill;
mod changes;
mod charts;
mod control_plane;
mod facts;
mod header;
mod namespaces;
mod pods;
mod pools;
mod summary;
mod workloads;

use std::rc::Rc;

use flock_core::cluster::{Namespace, NamespaceSort};
use flock_core::cluster_changes::{ClusterChange, newest_first};
use flock_core::control_plane::ControlPlane;
use flock_core::expiry::{Expiring, Horizon, SupportEnd};
use flock_core::node_pools::{KubeletSkew, NodePool};
use flock_core::nodes::NodeSummary;
use flock_core::overview::{
    Capacity, NetworkFacts, PodPhases, TimeRange, Utilization, UtilizationSource, WorkloadHealth,
};
use flock_core::pod_tops::PodTops;
use flock_core::{
    ClusterDetail, ClusterId, ClusterSummary, Command, Counts, Page, ResourceRef, Status,
};
use gpui::{
    AnyElement, App, AppContext, Context, Div, ElementId, Entity, EventEmitter, FocusHandle,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, div, relative,
};

use super::ScreenEvent;
use crate::kit::{
    ActionMenu, Activate, Link, MenuItem, SectionLabel, Segmented, problem_link, status_symbols,
};
use crate::theme::{
    ActiveTheme, FONT_MONO, Palette, RULE, SPACE_3, SPACE_4, SPACE_5, SPACE_7, SPACE_8,
    TEXT_CONTROL, TEXT_SMALL,
};

const CHANGES_BAND_WEIGHTS: [f32; 2] = [3., 2.];
const ACTIONS: &str = "overview-actions";
const SCROLL: &str = "overview-scroll";
const ESCAPE: &str = "escape";
const CLUSTER_SETTINGS: &str = "Cluster settings";
const CLUSTER_SETTINGS_DESCRIPTION: &str =
    "Change this cluster's default namespace, metrics source and labels.";

#[derive(Clone, Debug, PartialEq)]
struct OverviewData {
    cluster: ClusterId,
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
    expiring: Vec<Expiring>,
    support: Option<SupportEnd>,
    utilization: Vec<Utilization>,
    workloads: Vec<WorkloadHealth>,
    tops: PodTops,
    namespaces: Vec<Namespace>,
    changes: Vec<ClusterChange>,
    control_plane: ControlPlane,
}

impl OverviewData {
    fn new(cluster: &ClusterSummary, detail: &ClusterDetail) -> Self {
        let mut changes = detail.overview.changes.clone();
        newest_first(&mut changes);
        Self {
            cluster: cluster.id.clone(),
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
            expiring: detail.overview.expiring.clone(),
            support: detail.overview.support.clone(),
            utilization: detail.overview.utilization.clone(),
            workloads: detail.workload_health(),
            tops: detail.overview.tops.clone(),
            namespaces: detail.namespaces.clone(),
            changes,
            control_plane: detail.overview.control_plane.clone(),
        }
    }
}

pub struct OverviewScreen {
    data: Option<OverviewData>,
    range: TimeRange,
    horizon: Horizon,
    sort: NamespaceSort,
    drill: bool,
    code: Option<String>,
    focus: FocusHandle,
    menu: Entity<ActionMenu>,
}

impl EventEmitter<ScreenEvent> for OverviewScreen {}

impl OverviewScreen {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let menu = cx.new(|cx| ActionMenu::new(ACTIONS, cx));
        Self {
            data: None,
            range: TimeRange::default(),
            horizon: Horizon::default(),
            sort: NamespaceSort::default(),
            drill: false,
            code: None,
            focus: cx.focus_handle(),
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
        self.show(data, cx);
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
            self.drill = false;
            self.code = None;
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
        div()
            .flex()
            .items_center()
            .gap(SPACE_4)
            .child(ranges)
            .child(this.menu.clone())
    }

    fn select_range(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(range) = TimeRange::ALL.get(ix) {
            self.range = *range;
            cx.notify();
        }
    }

    pub fn inspecting(&self) -> bool {
        self.drill
    }

    pub fn close_inspector(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.drill;
        self.close_drill(cx);
        open
    }

    fn toggle_drill(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.drill = !self.drill;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn close_drill(&mut self, cx: &mut Context<Self>) {
        self.drill = false;
        cx.notify();
    }

    fn filter_code(&mut self, code: Option<String>, cx: &mut Context<Self>) {
        self.code = code;
        cx.notify();
    }

    fn select_horizon(&mut self, horizon: Horizon, cx: &mut Context<Self>) {
        self.horizon = horizon;
        cx.notify();
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

    fn attention(page: Page, namespace: Option<String>, cx: &mut Context<Self>) -> Activate {
        let this = cx.entity().downgrade();
        Rc::new(move |_, cx| {
            let namespace = namespace.clone();
            let event = ScreenEvent::NeedsAttention { page, namespace };
            this.update(cx, |_, cx| cx.emit(event)).ok();
        })
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
        let facts_band: Vec<AnyElement> =
            std::iter::once(control_plane::render(&data, self.drill, &palette, cx))
                .chain(facts::render(&data, self.horizon, &palette, cx))
                .collect();
        let sections = [
            Some(header::render(&data, &palette, cx)),
            Some(summary::render(&data, &palette, cx)),
            Some(charts::render(&data, self.range, &palette, cx)),
            weighted_band(changes_band, &CHANGES_BAND_WEIGHTS, &palette),
            band(pods::render(&data, &palette, cx), &palette),
            Some(namespaces::render(&data, self.sort, &palette, cx)),
            band(facts_band, &palette),
            Some(workloads::render(&data, &palette, cx)),
        ];
        let content = div()
            .flex()
            .flex_col()
            .tab_group()
            .pb(SPACE_8)
            .children(sections.into_iter().flatten());
        let scroll = div()
            .debug_selector(|| SCROLL.to_owned())
            .id(SCROLL)
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .child(content);
        let drill = self
            .drill
            .then(|| api_drill::render(&data.control_plane, self.code.as_deref(), &palette, cx));
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == ESCAPE && this.drill {
                    this.close_drill(cx);
                    cx.stop_propagation();
                }
            }))
            .flex()
            .size_full()
            .child(scroll)
            .children(drill)
            .into_any_element()
    }
}

fn band_shares(count: usize, weights: &[f32]) -> Vec<f32> {
    let weight = |ix: usize| weights.get(ix).copied().unwrap_or(1.);
    let total: f32 = (0..count).map(weight).sum();
    (0..count).map(|ix| weight(ix) / total).collect()
}

fn band(sections: Vec<AnyElement>, palette: &Palette) -> Option<AnyElement> {
    weighted_band(sections, &[], palette)
}

fn weighted_band(
    sections: Vec<AnyElement>,
    weights: &[f32],
    palette: &Palette,
) -> Option<AnyElement> {
    if sections.is_empty() {
        return None;
    }
    let last = sections.len() - 1;
    let shares = band_shares(sections.len(), weights);
    let cells = sections
        .into_iter()
        .zip(shares)
        .enumerate()
        .map(|(ix, (section, share))| {
            let cell = div()
                .flex_none()
                .w(relative(share))
                .min_w_0()
                .flex()
                .flex_col()
                .child(section);
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

fn meta_text(meta: String, palette: &Palette) -> Div {
    div()
        .font_family(FONT_MONO)
        .text_size(TEXT_SMALL)
        .text_color(palette.mute)
        .truncate()
        .child(meta)
}

fn section_head(label: &str, meta: Option<String>, link: Option<Link>, palette: &Palette) -> Div {
    let meta = meta.map(|meta| meta_text(meta, palette).into_any_element());
    head_row(label, meta, link)
}

fn head_row(label: &str, meta: Option<AnyElement>, link: Option<Link>) -> Div {
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
        .child(status_symbols(text))
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
    fn band_shares_follow_weights_and_default_to_equal() {
        assert_eq!(band_shares(2, &CHANGES_BAND_WEIGHTS), vec![0.6, 0.4]);
        assert_eq!(band_shares(2, &[]), vec![0.5, 0.5]);
        assert_eq!(band_shares(1, &[]), vec![1.]);
    }

    #[test]
    fn flux_line_is_absent_without_flux() {
        let data = OverviewData::new(&summary(), &ClusterDetail::default());
        assert_eq!(data.flux, None);
    }
}
