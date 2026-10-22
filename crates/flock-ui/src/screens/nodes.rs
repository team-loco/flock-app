use flock_core::emptiness::Noun;
use flock_core::events::Event;
use flock_core::filter::label_values;
use flock_core::node_pools::{kubelet_skewed, pressure};
use flock_core::nodes::{Node, NodeFilter, NodePod, NodeSummary, Usage, pool_counts, request_line};
use flock_core::overview::{Commitment, percent};
use flock_core::pods::{POD_KIND, PodEvent};
use flock_core::time::age;
use flock_core::units::{Amount, CPU_UNIT, MEMORY_UNIT, amount, figure};
use flock_core::{
    ClusterDetail, ClusterId, ClusterSummary, Command, Page, ResourceRef, Seen, Status,
};
use gpui::{
    AnyElement, AppContext, Context, Entity, EventEmitter, Hsla, InteractiveElement, IntoElement,
    ParentElement, Pixels, Render, SharedString, Styled, WeakEntity, Window, div, px,
};

use super::{NODE_KIND, ScreenEvent};
use crate::kit::{
    ActionMenu, CONDITIONS, Column, ConditionTable, ConfirmDialog, FilterBar, FilterEvent,
    Inspector, KeyValue, LabelChips, Link, MenuItem, Meter, MiniMeter, PanelSection, Presence,
    Select, SelectEvent, Table, TableEvent, chip_label, confirm, slide_in, slide_out,
};
use crate::screens::{ListShape, edit_item, empty_list, new_tab_item};
use crate::theme::{
    ActiveTheme, FONT_MONO, INSPECTOR_NODE, Palette, SEARCH_WIDTH, SPACE_3, SPACE_4, SPACE_5,
    SPACE_6, TEXT_BODY, TEXT_LABEL, TEXT_MONO, TEXT_SMALL,
};

const VERSION_SEPARATOR: char = '-';
const ALL_POOLS: &str = "All pools";
const NAME_WEIGHT: f32 = 1.6;
const NAME_MIN: Pixels = px(150.);
const POOL_WEIGHT: f32 = 1.;
const POOL_MIN: Pixels = px(90.);
const STATUS_WIDTH: Pixels = px(176.);
const PODS_WIDTH: Pixels = px(56.);
const REQUEST_WEIGHT: f32 = 0.8;
const REQUEST_MIN: Pixels = px(70.);
const REQUEST_FIGURE: Pixels = px(32.);
const VERSION_WIDTH: Pixels = px(64.);
const AGE_WIDTH: Pixels = px(40.);
const INSPECTOR_LABEL: Pixels = px(72.);
const USAGE_LABEL: Pixels = px(64.);
const POD_FIGURE: Pixels = px(56.);
const INSPECTOR_ID: &str = "node-inspector";
const ACTIONS_ID: &str = "node-actions";
const FOOTER_NOTE: &str = "requests / allocatable";
const USED: &str = "used";
const BY_REQUEST: &str = "by CPU request";
const CPU_HEADING: &str = "CPU";
const MEMORY_HEADING: &str = "Mem";

pub struct NodesScreen {
    cluster: Option<ClusterId>,
    cluster_name: String,
    control_plane: Option<String>,
    shown: usize,
    summary: NodeSummary,
    nodes: Vec<Node>,
    events: Vec<Event>,
    confirm_destructive: bool,
    selected: Option<String>,
    pool: Option<String>,
    table: Entity<Table<NodeRow>>,
    filter: Entity<FilterBar>,
    pools: Entity<Select>,
    actions: Entity<ActionMenu>,
    presence: Presence<Node>,
}

#[derive(Clone, Debug, PartialEq)]
struct NodeRow {
    node: Node,
    skewed: bool,
}

fn node_ref(cluster: &ClusterId, name: &str) -> ResourceRef {
    ResourceRef {
        cluster: cluster.clone(),
        kind: NODE_KIND.to_owned(),
        namespace: None,
        name: name.to_owned(),
    }
}

fn presence(screen: &mut NodesScreen) -> &mut Presence<Node> {
    &mut screen.presence
}

impl EventEmitter<ScreenEvent> for NodesScreen {}

impl NodesScreen {
    pub fn inspecting(&self) -> bool {
        self.selected.is_some()
    }

    pub fn close_inspector(&mut self, cx: &mut Context<Self>) -> bool {
        if self.selected.is_none() {
            return false;
        }
        self.close(cx);
        true
    }

    pub fn list_focus(&self, cx: &gpui::App) -> gpui::FocusHandle {
        gpui::Focusable::focus_handle(&self.table, cx)
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let table = cx.new(|cx| {
            Table::new("nodes", columns(), cx)
                .row_key(|r: &NodeRow| r.node.name.clone().into())
                .row_status(|r: &NodeRow| (r.node.status != Status::Ok).then_some(r.node.status))
        });
        cx.subscribe(&table, |this, table, event: &TableEvent, cx| {
            let TableEvent::Selected(ix) = event else {
                return;
            };
            let name = table.read(cx).row(*ix).map(|r| r.node.name.clone());
            this.select(name, cx);
        })
        .detach();
        let pools = cx.new(|cx| {
            Select::new("node-pool", vec![ALL_POOLS.into()], cx)
                .mono()
                .width(SEARCH_WIDTH)
        });
        cx.subscribe(&pools, |this, select, _: &SelectEvent, cx| {
            let select = select.read(cx);
            this.pool = (select.selected_index() > 0)
                .then(|| select.selected_label().map(ToString::to_string))
                .flatten();
            this.refresh(cx);
        })
        .detach();
        let filter = cx.new(|cx| {
            FilterBar::new("node-filter", Node::FILTER.placeholder(), window, cx)
                .scope(pools.clone())
        });
        cx.subscribe(&filter, |this, _, FilterEvent::Changed, cx| {
            this.refresh(cx);
        })
        .detach();
        let actions = cx.new(|cx| ActionMenu::new(ACTIONS_ID, cx));
        Self {
            cluster: None,
            cluster_name: String::new(),
            control_plane: None,
            shown: 0,
            summary: NodeSummary::default(),
            nodes: Vec::new(),
            events: Vec::new(),
            confirm_destructive: true,
            selected: None,
            pool: None,
            table,
            filter,
            pools,
            actions,
            presence: Presence::default(),
        }
    }

    pub fn set(
        &mut self,
        cluster: &ClusterSummary,
        detail: &ClusterDetail,
        confirm_destructive: bool,
        cx: &mut Context<Self>,
    ) {
        self.selected = kept_selection(self.cluster.as_ref(), &cluster.id, self.selected.take());
        self.cluster = Some(cluster.id.clone());
        self.cluster_name.clone_from(&cluster.name);
        let changed = self.confirm_destructive != confirm_destructive
            || self.control_plane != cluster.version
            || self.nodes != detail.nodes;
        self.confirm_destructive = confirm_destructive;
        self.control_plane.clone_from(&cluster.version);
        if self.events != detail.events {
            self.events.clone_from(&detail.events);
            cx.notify();
        }
        if !changed {
            return;
        }
        self.nodes.clone_from(&detail.nodes);
        self.summary = detail.node_summary();
        if self
            .selected
            .as_ref()
            .is_some_and(|name| !self.nodes.iter().any(|n| &n.name == name))
        {
            self.selected = None;
        }
        self.set_pools(cx);
        self.refresh(cx);
    }

    pub fn filter_bar(&self) -> &Entity<FilterBar> {
        &self.filter
    }

    fn set_pools(&mut self, cx: &mut Context<Self>) {
        let counts = pool_counts(&self.nodes);
        if self
            .pool
            .as_ref()
            .is_some_and(|pool| !counts.iter().any(|(name, _)| name == pool))
        {
            self.pool = None;
        }
        let options: Vec<SharedString> = std::iter::once(ALL_POOLS.into())
            .chain(counts.iter().map(|(name, _)| name.clone().into()))
            .collect();
        let numbers: Vec<usize> = std::iter::once(self.nodes.len())
            .chain(counts.iter().map(|(_, count)| *count))
            .collect();
        let selected = self
            .pool
            .as_ref()
            .and_then(|pool| counts.iter().position(|(name, _)| name == pool))
            .map_or(0, |ix| ix + 1);
        self.pools.update(cx, |select, cx| {
            select.set_options(options, cx);
            select.set_counts(numbers, cx);
            select.set_selected(selected, cx);
        });
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let node_labels: Vec<_> = self.nodes.iter().map(Node::label_map).collect();
        let labels = label_values(&node_labels);
        self.filter.update(cx, |bar, cx| bar.set_labels(labels, cx));
        let (query, chip) = {
            let bar = self.filter.read(cx);
            (bar.query().clone(), bar.chip())
        };
        let pool = self.pool.as_deref();
        let base: Vec<&Node> = self
            .nodes
            .iter()
            .filter(|n| pool.is_none_or(|pool| n.pool() == pool))
            .filter(|n| n.matches(&query))
            .collect();
        let chips = NodeFilter::counts(&base)
            .into_iter()
            .map(|(filter, n)| chip_label(filter.label(), n))
            .collect();
        let chip = NodeFilter::at(chip);
        let control_plane = self.control_plane.as_deref();
        let rows: Vec<NodeRow> = base
            .into_iter()
            .filter(|n| chip.keeps(n))
            .map(|node| NodeRow {
                node: node.clone(),
                skewed: control_plane.is_some_and(|server| kubelet_skewed(&node.version, server)),
            })
            .collect();
        let shown = rows.len();
        self.shown = shown;
        let total = self.nodes.len();
        self.filter.update(cx, |bar, cx| {
            bar.set_chips(chips, cx);
            bar.set_matches(shown, total, cx);
        });
        let scope = self.cluster.as_ref().map(|c| c.as_str().to_owned());
        let words = query.words;
        let actions = self.row_actions(cx);
        self.table.update(cx, |table, cx| {
            table.set_row_actions(actions, cx);
            if let Some(scope) = scope {
                table.set_scope(scope, cx);
            }
            table.set_highlight(words, cx);
            table.set_rows(rows, cx);
        });
        self.select_in_table(cx);
        self.set_inspector_actions(cx);
        cx.notify();
    }

    pub fn reveal(&mut self, cluster: ClusterId, name: String, cx: &mut Context<Self>) {
        if self.cluster.as_ref() == Some(&cluster) && self.selected.as_ref() == Some(&name) {
            return;
        }
        self.cluster = Some(cluster);
        self.selected = Some(name);
        self.select_in_table(cx);
        self.set_inspector_actions(cx);
        cx.notify();
    }

    fn select_in_table(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.selected.as_ref() else {
            return;
        };
        let name = name.clone();
        self.table.update(cx, |table, cx| {
            let position = (0..)
                .map_while(|ix| table.row(ix))
                .position(|r| r.node.name == name);
            if position.is_some() {
                table.select(position, cx);
            }
        });
    }

    pub fn header_note(&self) -> String {
        format!("{} / {} ready", self.summary.ready, self.summary.total)
    }

    fn select(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        self.selected = name;
        let target = self.selected.as_ref().zip(self.cluster.as_ref());
        let command = target.map_or(Command::CloseInspector, |(name, cluster)| {
            Command::InspectResource(node_ref(cluster, name))
        });
        self.set_inspector_actions(cx);
        cx.emit(ScreenEvent::Command(command));
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| table.select(None, cx));
        self.select(None, cx);
    }

    fn selected_node(&self) -> Option<&Node> {
        let name = self.selected.as_ref()?;
        self.nodes.iter().find(|n| &n.name == name)
    }

    fn skewed(&self, node: &Node) -> bool {
        self.control_plane
            .as_deref()
            .is_some_and(|server| kubelet_skewed(&node.version, server))
    }

    fn set_inspector_actions(&mut self, cx: &mut Context<Self>) {
        let screen = cx.weak_entity();
        let items = self
            .selected_node()
            .zip(self.cluster.as_ref())
            .map(|(node, cluster)| {
                let actions = NodeActions::of(node, cluster, self.confirm_destructive, &screen);
                vec![actions.toggle, actions.drain, actions.edit]
            })
            .unwrap_or_default();
        self.actions
            .update(cx, |menu, cx| menu.set_items(items, cx));
    }

    fn row_actions(&self, cx: &mut Context<Self>) -> impl Fn(&NodeRow) -> Vec<MenuItem> + 'static {
        let cluster = self.cluster.clone();
        let confirm_destructive = self.confirm_destructive;
        let screen = cx.weak_entity();
        move |row: &NodeRow| {
            let Some(cluster) = cluster.clone() else {
                return Vec::new();
            };
            let node = &row.node;
            let target = node_target(node, &cluster);
            let open_screen = screen.clone();
            let open = new_tab_item(Command::OpenResource(target.target), move |command, cx| {
                let _ = open_screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)));
            });
            let actions = NodeActions::of(node, &cluster, confirm_destructive, &screen);
            vec![open, actions.edit, actions.toggle, actions.drain]
        }
    }

    fn inspector(&self, node: &Node, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let skewed = self.skewed(node);
        let kubelet_color = if skewed { palette.warn } else { palette.fg };
        let kubelet = div().text_color(kubelet_color).child(node.version.clone());
        let details = KeyValue::new()
            .label_width(INSPECTOR_LABEL)
            .text("pool", node.pool().to_owned())
            .text("instance", node.instance_type.clone())
            .text("zone", node.zone.clone())
            .text("internal IP", node.internal_ip.clone())
            .row("kubelet", kubelet)
            .text("OS image", node.os_image.clone())
            .text("runtime", node.runtime.clone())
            .text("age", age(node.age));
        let events: Vec<PodEvent> =
            flock_core::diagnosis::about(&self.events, NODE_KIND, None, &node.name)
                .into_iter()
                .map(PodEvent::from)
                .collect();
        let not_ok = node.conditions_not_ok();
        let condition_color = if not_ok > 0 {
            palette.warn
        } else {
            palette.mute
        };
        let conditions = PanelSection::new(INSPECTOR_ID, CONDITIONS)
            .count(node.conditions.len().to_string())
            .count_color(condition_color)
            .closed()
            .child(ConditionTable::new(&node.conditions));
        let taints = PanelSection::new(INSPECTOR_ID, "Taints")
            .count(node.taints.len().to_string())
            .child(LabelChips::new(node.taints.clone()));
        let pods = PanelSection::new(INSPECTOR_ID, "Pods")
            .count(pods_count(node))
            .child(self.top_pods(node, palette, cx));
        Inspector::new(INSPECTOR_ID, node.name.clone())
            .mono()
            .width(INSPECTOR_NODE)
            .status(node.status, node.state.clone())
            .issues(node.issue())
            .actions(self.actions.clone())
            .panel(PanelSection::new(INSPECTOR_ID, "Usage").child(node_usage(node, palette)))
            .panel(PanelSection::new(INSPECTOR_ID, "Details").child(details))
            .panel(taints)
            .panel(PanelSection::labels(INSPECTOR_ID, node.labels.clone()).closed())
            .panel(conditions)
            .panel(pods)
            .panel(PanelSection::events(INSPECTOR_ID, events))
            .into_any_element()
    }

    fn top_pods(&self, node: &Node, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let heading = pod_line(
            div().child(BY_REQUEST).into_any_element(),
            CPU_HEADING,
            MEMORY_HEADING,
        )
        .font_family(crate::theme::FONT_SANS)
        .text_size(TEXT_LABEL)
        .text_color(palette.mute);
        let cluster = self.cluster.clone();
        let screen = cx.weak_entity();
        let rows = node.top_pods.iter().enumerate().map(|(ix, pod)| {
            let target = cluster.as_ref().map(|cluster| pod_ref(cluster, pod));
            let screen = screen.clone();
            let name = Link::new(
                SharedString::from(format!("node-pod-{ix}")),
                pod.name.clone(),
            )
            .mono()
            .color(palette.fg)
            .on_click(move |_, _, cx| {
                if let Some(target) = target.clone() {
                    let command = ScreenEvent::Command(Command::OpenResource(target));
                    let _ = screen.update(cx, |_, cx| cx.emit(command));
                }
            });
            let label = div()
                .flex()
                .gap(SPACE_3)
                .min_w_0()
                .overflow_hidden()
                .child(name)
                .child(
                    div()
                        .truncate()
                        .text_color(palette.mute)
                        .child(pod.namespace.clone()),
                )
                .into_any_element();
            pod_line(
                label,
                figure(pod.cpu),
                Amount::of(figure(pod.memory), MEMORY_UNIT).labelled(),
            )
            .text_color(palette.fg2)
            .into_any_element()
        });
        div()
            .flex()
            .flex_col()
            .gap(SPACE_3)
            .font_family(FONT_MONO)
            .text_size(TEXT_SMALL)
            .child(heading)
            .children(rows)
            .into_any_element()
    }
}

impl Render for NodesScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let shape = ListShape {
            shown: self.shown,
            total: self.nodes.len(),
            namespace: None,
            noun: Noun::of(Page::Nodes),
            cluster: &self.cluster_name,
        };
        let body: AnyElement = match empty_list(&shape, &self.filter, None, cx) {
            Some(empty) => empty.into_any_element(),
            None => self.table.clone().into_any_element(),
        };
        let footer = div()
            .debug_selector(|| "node-footer".to_owned())
            .px(SPACE_6)
            .py(SPACE_4)
            .font_family(FONT_MONO)
            .text_size(TEXT_LABEL)
            .text_color(palette.mute)
            .child(footer_line(self.shown, self.nodes.len()));
        let current = self.selected_node().cloned();
        let shown = current.as_ref().map(|node| (node, node.name.as_str()));
        let motion = self.presence.track(shown, presence, cx);
        let inspector = match current {
            Some(node) => {
                let panel = self.inspector(&node, &palette, cx);
                Some(slide_in(panel, INSPECTOR_ID, motion, INSPECTOR_NODE))
            }
            None => self.presence.leaving().cloned().map(|node| {
                let panel = self.inspector(&node, &palette, cx);
                slide_out(panel, INSPECTOR_ID, INSPECTOR_NODE)
            }),
        };
        div()
            .size_full()
            .flex()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(self.filter.clone())
                    .child(div().flex_1().min_h_0().child(body))
                    .child(footer),
            )
            .children(inspector)
    }
}

fn footer_line(shown: usize, total: usize) -> String {
    format!("{shown} of {total} nodes · {FOOTER_NOTE}")
}

fn pods_count(node: &Node) -> String {
    let listed = node.top_pods.len();
    if listed == 0 {
        node.pods.to_string()
    } else {
        format!("{} · top {listed}", node.pods)
    }
}

fn pod_ref(cluster: &ClusterId, pod: &NodePod) -> ResourceRef {
    ResourceRef {
        cluster: cluster.clone(),
        kind: POD_KIND.to_owned(),
        namespace: Some(pod.namespace.clone()),
        name: pod.name.clone(),
    }
}

fn pod_line(
    label: AnyElement,
    cpu: impl Into<SharedString>,
    memory: impl Into<SharedString>,
) -> gpui::Div {
    let figure = |text: SharedString| {
        div()
            .flex_none()
            .w(POD_FIGURE)
            .flex()
            .justify_end()
            .child(text)
    };
    div()
        .flex()
        .items_center()
        .gap(SPACE_5)
        .child(div().flex_1().min_w_0().child(label))
        .child(figure(cpu.into()))
        .child(figure(memory.into()))
}

fn node_target(node: &Node, cluster: &ClusterId) -> Seen {
    Seen::new(node_ref(cluster, &node.name), node.uid.clone())
}

struct NodeActions {
    toggle: MenuItem,
    drain: MenuItem,
    edit: MenuItem,
}

impl NodeActions {
    fn of(
        node: &Node,
        cluster: &ClusterId,
        confirm_destructive: bool,
        screen: &WeakEntity<NodesScreen>,
    ) -> Self {
        let target = node_target(node, cluster);
        let (label, command) = if node.schedulable {
            ("Cordon", Command::Cordon(target.clone()))
        } else {
            ("Uncordon", Command::Uncordon(target.clone()))
        };
        let toggle_screen = screen.clone();
        let toggle = MenuItem::new(label, move |_, cx| {
            let command = command.clone();
            let _ = toggle_screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)));
        })
        .disabled(node.status == Status::Progress);
        let edit_screen = screen.clone();
        let edit = edit_item(target.target.clone(), move |command, cx| {
            let _ = edit_screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)));
        });
        let detail = format!("{} · {}", target.target.cluster, target.target.name);
        let screen = screen.clone();
        let drain = MenuItem::new("Drain…", move |window, cx| {
            let screen = screen.clone();
            let target = target.clone();
            if !confirm_destructive {
                let command = Command::Drain(target);
                let _ = screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)));
                return;
            }
            let dialog = ConfirmDialog::new(
                "Drain node",
                "The node is cordoned and every pod except DaemonSet pods is evicted. Pods without a controller are not recreated.",
                "Drain node",
                move |_, cx| {
                    let command = Command::Drain(target.clone());
                    let _ = screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)));
                },
            )
            .detail(detail.clone());
            confirm(dialog, window, cx);
        })
        .destructive();
        Self {
            toggle,
            drain,
            edit,
        }
    }
}

fn count(value: u32) -> f32 {
    f32::from(u16::try_from(value).unwrap_or(u16::MAX))
}

fn short_version(version: &str) -> SharedString {
    let short = version.split(VERSION_SEPARATOR).next().unwrap_or(version);
    short.to_owned().into()
}

fn pressure_color(fraction: f32, palette: &Palette) -> Option<Hsla> {
    pressure(fraction).map(|status| palette.status(status))
}

fn node_usage(node: &Node, palette: &Palette) -> AnyElement {
    let pods = Usage {
        used: count(node.pods),
        allocatable: count(node.pod_capacity),
    };
    div()
        .flex()
        .flex_col()
        .gap(SPACE_6)
        .child(usage_row(
            "CPU",
            amount(node.cpu.used, node.cpu.allocatable, CPU_UNIT).labelled(),
            node.cpu,
            Some(node.cpu_commitment),
            palette,
        ))
        .child(usage_row(
            "Memory",
            amount(node.memory.used, node.memory.allocatable, MEMORY_UNIT).labelled(),
            node.memory,
            Some(node.memory_commitment),
            palette,
        ))
        .child(usage_row(
            "Pods",
            format!("{} / {}", node.pods, node.pod_capacity),
            pods,
            None,
            palette,
        ))
        .into_any_element()
}

fn usage_row(
    label: &'static str,
    figures: String,
    usage: Usage,
    commitment: Option<Commitment>,
    palette: &Palette,
) -> impl IntoElement {
    let fraction = usage.fraction();
    let warning = pressure_color(fraction, palette);
    let share = match commitment {
        Some(_) => format!("{} {USED}", percent(fraction)),
        None => percent(fraction),
    };
    let meter = Meter::new(fraction).color(warning.unwrap_or(palette.acc));
    let meter = match commitment {
        Some(commitment) => meter.request(commitment.requests),
        None => meter,
    };
    let sub = commitment.map(|commitment| {
        div()
            .font_family(FONT_MONO)
            .text_size(TEXT_LABEL)
            .text_color(palette.mute)
            .child(request_line(usage.allocatable, commitment))
    });
    div()
        .debug_selector(move || format!("node-usage-{label}"))
        .flex()
        .flex_col()
        .gap(SPACE_3)
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_4)
                .font_family(FONT_MONO)
                .text_size(TEXT_MONO)
                .child(
                    div()
                        .flex_none()
                        .w(USAGE_LABEL)
                        .font_family(crate::theme::FONT_SANS)
                        .text_size(TEXT_BODY)
                        .text_color(palette.mute)
                        .child(label),
                )
                .child(figures)
                .child(div().flex_1())
                .child(
                    div()
                        .text_color(warning.unwrap_or(palette.fg2))
                        .child(share),
                ),
        )
        .child(div().ml(USAGE_LABEL + SPACE_4).child(meter))
        .children(sub.map(|sub| div().ml(USAGE_LABEL + SPACE_4).child(sub)))
}

fn request_cell(fraction: f32, palette: &Palette) -> AnyElement {
    let warning = pressure_color(fraction, palette);
    div()
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .gap(SPACE_3)
        .child(
            MiniMeter::new(fraction)
                .fill()
                .color(warning.unwrap_or(palette.acc)),
        )
        .child(
            div()
                .flex_none()
                .w(REQUEST_FIGURE)
                .flex()
                .justify_end()
                .text_color(warning.unwrap_or(palette.fg2))
                .child(percent(fraction)),
        )
        .into_any_element()
}

fn columns() -> Vec<Column<NodeRow>> {
    vec![
        Column::text("Name", |r: &NodeRow| r.node.name.clone().into())
            .highlight()
            .mono()
            .flex(NAME_WEIGHT, NAME_MIN)
            .sort_by_key(|r: &NodeRow| r.node.name.clone()),
        Column::colored_text(
            "Pool",
            |r: &NodeRow| r.node.pool().to_owned().into(),
            |_, p| p.fg2,
        )
        .mono()
        .flex(POOL_WEIGHT, POOL_MIN)
        .sort_by_key(|r: &NodeRow| r.node.pool().to_owned()),
        Column::colored_text(
            "Status",
            |r: &NodeRow| r.node.state.clone().into(),
            |r: &NodeRow, p| p.status(r.node.status),
        )
        .fixed(STATUS_WIDTH)
        .sort_by_key(|r: &NodeRow| std::cmp::Reverse(r.node.status)),
        Column::colored_text(
            "Pods",
            |r: &NodeRow| format!("{}/{}", r.node.pods, r.node.pod_capacity).into(),
            |_, p| p.fg2,
        )
        .mono()
        .end()
        .fixed(PODS_WIDTH)
        .sort_by_key(|r: &NodeRow| r.node.pods),
        Column::new("CPU req", |r: &NodeRow, p| {
            request_cell(r.node.cpu_commitment.requests, p)
        })
        .mono()
        .flex(REQUEST_WEIGHT, REQUEST_MIN)
        .sortable(|a: &NodeRow, b: &NodeRow| {
            a.node
                .cpu_commitment
                .requests
                .total_cmp(&b.node.cpu_commitment.requests)
        }),
        Column::new("Mem req", |r: &NodeRow, p| {
            request_cell(r.node.memory_commitment.requests, p)
        })
        .mono()
        .flex(REQUEST_WEIGHT, REQUEST_MIN)
        .sortable(|a: &NodeRow, b: &NodeRow| {
            a.node
                .memory_commitment
                .requests
                .total_cmp(&b.node.memory_commitment.requests)
        }),
        Column::colored_text(
            "Kubelet",
            |r: &NodeRow| short_version(&r.node.version),
            |r: &NodeRow, p| if r.skewed { p.warn } else { p.fg2 },
        )
        .mono()
        .fixed(VERSION_WIDTH)
        .sort_by_key(|r: &NodeRow| r.node.version.clone()),
        Column::colored_text("Age", |r: &NodeRow| age(r.node.age).into(), |_, p| p.mute)
            .mono()
            .end()
            .fixed(AGE_WIDTH)
            .sort_by_key(|r: &NodeRow| r.node.age),
    ]
}

fn kept_selection(
    shown: Option<&ClusterId>,
    next: &ClusterId,
    selected: Option<String>,
) -> Option<String> {
    selected.filter(|_| shown == Some(next))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_survives_data_for_the_cluster_it_was_revealed_on() {
        let prod = ClusterId::new("prod-us-east");
        let other = ClusterId::new("staging");
        let node = Some("ip-10-4-31-12".to_owned());
        assert_eq!(kept_selection(Some(&prod), &prod, node.clone()), node);
        assert_eq!(kept_selection(Some(&other), &prod, node.clone()), None);
        assert_eq!(kept_selection(None, &prod, node), None);
    }

    #[test]
    fn version_drops_the_build_suffix() {
        assert_eq!(short_version("v1.31.2-eks-7f9249a"), "v1.31.2");
        assert_eq!(short_version("v1.29.8"), "v1.29.8");
    }

    #[test]
    fn footer_counts_shown_nodes_against_all() {
        assert_eq!(footer_line(9, 64), "9 of 64 nodes · requests / allocatable");
    }
}
