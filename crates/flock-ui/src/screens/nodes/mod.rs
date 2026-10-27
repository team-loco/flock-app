use std::rc::Rc;
use std::sync::Arc;

use flock_core::emptiness::Noun;
use flock_core::events::Event;
use flock_core::filter::label_values;
use flock_core::node_pools::{kubelet_skewed, pressure};
use flock_core::nodes::{Node, NodeFilter, NodePod, NodeSummary, Usage, pool_counts, request_line};
use flock_core::overview::{Commitment, percent};
use flock_core::pods::{POD_KIND, PodEvent};
use flock_core::time::age;
use flock_core::units::{CPU_UNIT, MEMORY_UNIT, NOT_MEASURED, amount};
use flock_core::{ClusterId, ClusterSummary, Command, Page, ResourceRef, Seen, Snapshot, Status};
use gpui::{
    AnyElement, AppContext, Context, Entity, EventEmitter, Hsla, InteractiveElement, IntoElement,
    ParentElement, Pixels, Render, SharedString, Styled, WeakEntity, Window, div, px,
};

mod allocation;
mod inspector;
mod shell;

use super::{NODE_KIND, ScreenEvent};
use crate::kit::{
    ActionMenu, Column, ConfirmDialog, DockState, FilterBar, FilterEvent, MenuItem, Meter,
    MiniMeter, Presence, Select, SelectEvent, SnapshotRow, SnapshotRows, Table, TableEvent,
    Terminal, chip_label, confirm, pane, slide_in, slide_out,
};
use crate::screens::{ListShape, edit_item, empty_list, new_tab_item};
use crate::theme::{
    ActiveTheme, FONT_MONO, INSPECTOR_NODE, Palette, SEARCH_WIDTH, SPACE_3, SPACE_4, SPACE_5,
    SPACE_6, TEXT_BODY, TEXT_LABEL, TEXT_MONO,
};
use allocation::Allocation;
use inspector::{NodeContent, NodeInspector};
use shell::{NodeShell, TERMINAL_ID, ask_shell};

const VERSION_SEPARATOR: char = '-';
const ALL_POOLS: &str = "All pools";
const NAME_WEIGHT: f32 = 1.6;
const NAME_MIN: Pixels = px(100.);
const POOL_WEIGHT: f32 = 1.;
const POOL_MIN: Pixels = px(70.);
const STATUS_WIDTH: Pixels = px(150.);
const PODS_WIDTH: Pixels = px(56.);
const REQUEST_WEIGHT: f32 = 0.8;
const REQUEST_MIN: Pixels = px(112.);
const REQUEST_FIGURE: Pixels = px(32.);
const VERSION_WIDTH: Pixels = px(64.);
const AGE_WIDTH: Pixels = px(40.);
const INSPECTOR_LABEL: Pixels = px(72.);
const DISK_LABEL: &str = "Disk";
const USAGE_LABEL: Pixels = px(64.);
const POD_FIGURE: Pixels = px(56.);
const INSPECTOR_ID: &str = "node-inspector";
const SHELL_ID: &str = "node-shell";
const SHELL_ITEM: &str = "Shell…";
const ALL_PODS_ID: &str = "node-inspector-all-pods";
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
    nodes: Rc<SnapshotRows<Node>>,
    confirm_destructive: bool,
    selected: Option<String>,
    pool: Option<String>,
    table: Entity<Table<NodeRow>>,
    filter: Entity<FilterBar>,
    pools: Entity<Select>,
    actions: Entity<ActionMenu>,
    inspector: Entity<NodeInspector>,
    presence: Presence<Node>,
    shell: Option<NodeShell>,
    terminal: Entity<Terminal>,
    dock: Entity<DockState>,
    allocation: Allocation,
}

#[derive(Clone)]
struct NodeRow {
    node: SnapshotRow<Node>,
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
            this.refresh_rows(cx);
        })
        .detach();
        let filter = cx.new(|cx| FilterBar::new("node-filter", window, cx).scope(pools.clone()));
        cx.subscribe(&filter, |this, _, FilterEvent::Changed, cx| {
            this.refresh_rows(cx);
        })
        .detach();
        let actions = cx.new(|cx| ActionMenu::new(ACTIONS_ID, cx));
        let screen = cx.weak_entity();
        let inspector = cx.new(|_| NodeInspector::new(screen, actions.clone()));
        let terminal = cx.new(|cx| Terminal::new(TERMINAL_ID, cx));
        let dock = cx.new(|_| DockState::collapsed());
        cx.observe(&dock, |this, dock, cx| {
            if !dock.read(cx).is_open() {
                this.close_shell(cx);
            }
        })
        .detach();
        Self {
            cluster: None,
            cluster_name: String::new(),
            control_plane: None,
            shown: 0,
            summary: NodeSummary::default(),
            nodes: SnapshotRows::empty(),
            confirm_destructive: true,
            selected: None,
            pool: None,
            table,
            filter,
            pools,
            actions,
            inspector,
            presence: Presence::default(),
            shell: None,
            terminal,
            dock,
            allocation: Allocation::default(),
        }
    }

    pub fn set(
        &mut self,
        cluster: &ClusterSummary,
        snapshot: &Arc<Snapshot>,
        confirm_destructive: bool,
        cx: &mut Context<Self>,
    ) {
        self.selected = kept_selection(self.cluster.as_ref(), &cluster.id, self.selected.take());
        let switched = self.cluster.as_ref() != Some(&cluster.id);
        self.cluster = Some(cluster.id.clone());
        self.cluster_name.clone_from(&cluster.name);
        let changed = switched
            || self.confirm_destructive != confirm_destructive
            || self.control_plane != cluster.version
            || !self.nodes.from(snapshot);
        self.confirm_destructive = confirm_destructive;
        self.control_plane.clone_from(&cluster.version);
        if !changed {
            return;
        }
        let id = cluster.id.clone();
        self.nodes = SnapshotRows::new(snapshot, move |snapshot| {
            snapshot
                .detail(&id)
                .map_or(&[][..], |detail| detail.nodes.as_slice())
        });
        self.summary = NodeSummary::of(self.nodes.items());
        if self
            .selected
            .as_ref()
            .is_some_and(|name| !self.nodes.items().iter().any(|n| &n.name == name))
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
        let counts = pool_counts(self.nodes.items());
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
        let numbers: Vec<usize> = std::iter::once(self.nodes.items().len())
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
        let nodes = self.nodes.items();
        let node_labels: Vec<_> = nodes.iter().map(Node::label_map).collect();
        let labels = label_values(&node_labels);
        self.filter.update(cx, |bar, cx| bar.set_labels(labels, cx));
        self.refresh_rows(cx);
    }

    fn refresh_rows(&mut self, cx: &mut Context<Self>) {
        let nodes = self.nodes.items();
        let (query, chip) = {
            let bar = self.filter.read(cx);
            (bar.query().clone(), bar.chip())
        };
        let pool = self.pool.as_deref();
        let base: Vec<(usize, &Node)> = self
            .nodes
            .matched_items(&query)
            .into_iter()
            .filter(|(_, n)| pool.is_none_or(|pool| n.pool() == pool))
            .collect();
        let base_nodes: Vec<&Node> = base.iter().map(|(_, node)| *node).collect();
        let chips = NodeFilter::counts(&base_nodes)
            .into_iter()
            .map(|(filter, n)| chip_label(filter.label(), n))
            .collect();
        let chip = NodeFilter::at(chip);
        let control_plane = self.control_plane.as_deref();
        let kept: Vec<usize> = base
            .into_iter()
            .filter(|(_, n)| chip.keeps(n))
            .map(|(ix, _)| ix)
            .collect();
        let rows: Vec<NodeRow> = self
            .nodes
            .rows(kept)
            .into_iter()
            .map(|node| {
                let skewed =
                    control_plane.is_some_and(|server| kubelet_skewed(&node.version, server));
                NodeRow { node, skewed }
            })
            .collect();
        let shown = rows.len();
        self.shown = shown;
        let total = nodes.len();
        self.filter.update(cx, |bar, cx| {
            bar.set_chips(chips, cx);
            bar.set_matches(shown, total, cx);
        });
        let scope = self.cluster.as_ref().map(|c| c.as_str().to_owned());
        let words = query.words;
        let actions = self.row_actions(cx);
        self.table.update(cx, |table, cx| {
            table.set_row_actions(actions);
            if let Some(scope) = scope {
                table.set_scope(scope, cx);
            }
            table.set_highlight(words, cx);
            table.set_rows(rows, cx);
        });
        self.select_in_table(cx);
        self.set_inspector_actions(cx);
        self.sync_inspector(cx);
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
        self.sync_inspector(cx);
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
        if self.selected != name {
            self.allocation.forget_focus();
        }
        self.selected = name;
        let target = self.selected.as_ref().zip(self.cluster.as_ref());
        let command = target.map_or(Command::CloseInspector, |(name, cluster)| {
            Command::InspectResource(node_ref(cluster, name))
        });
        self.set_inspector_actions(cx);
        self.sync_inspector(cx);
        cx.emit(ScreenEvent::Command(command));
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| table.select(None, cx));
        self.select(None, cx);
    }

    fn selected_node(&self) -> Option<&Node> {
        let name = self.selected.as_ref()?;
        self.nodes.items().iter().find(|n| &n.name == name)
    }

    fn events(&self) -> &[Event] {
        let detail = self
            .cluster
            .as_ref()
            .and_then(|cluster| self.nodes.snapshot().detail(cluster));
        detail.map_or(&[][..], |detail| detail.events.as_slice())
    }

    fn sync_inspector(&mut self, cx: &mut Context<Self>) {
        let Some(node) = self.selected_node() else {
            return;
        };
        let skewed = self
            .control_plane
            .as_deref()
            .is_some_and(|server| kubelet_skewed(&node.version, server));
        let events = flock_core::diagnosis::about(self.events(), NODE_KIND, None, &node.name)
            .into_iter()
            .map(PodEvent::from)
            .collect();
        let content = NodeContent {
            node: node.clone(),
            cluster: self.cluster.clone(),
            skewed,
            events,
        };
        self.inspector
            .update(cx, |inspector, cx| inspector.shown.show(content, cx));
    }

    fn set_inspector_actions(&mut self, cx: &mut Context<Self>) {
        let screen = cx.weak_entity();
        let items = self
            .selected_node()
            .zip(self.cluster.as_ref())
            .map(|(node, cluster)| {
                let actions = NodeActions::of(node, cluster, self.confirm_destructive, &screen);
                vec![actions.toggle, actions.drain, actions.edit, actions.shell]
            })
            .unwrap_or_default();
        self.actions
            .update(cx, |menu, cx| menu.set_items(items, cx));
    }

    fn row_actions(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&NodeRow, &[&NodeRow]) -> Vec<MenuItem> + 'static {
        let cluster = self.cluster.clone();
        let confirm_destructive = self.confirm_destructive;
        let screen = cx.weak_entity();
        move |row: &NodeRow, _: &[&NodeRow]| {
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
}

impl Render for NodesScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let shape = ListShape {
            shown: self.shown,
            total: self.nodes.items().len(),
            namespace: None,
            noun: Noun::of(Page::Nodes),
            cluster: &self.cluster_name,
        };
        let (body, listed): (AnyElement, bool) = match empty_list(&shape, &self.filter, None, cx) {
            Some(empty) => (empty.into_any_element(), false),
            None => (Table::pane(&self.table, cx).into_any_element(), true),
        };
        let footer = div()
            .debug_selector(|| "node-footer".to_owned())
            .px(SPACE_6)
            .py(SPACE_4)
            .font_family(FONT_MONO)
            .text_size(TEXT_LABEL)
            .text_color(palette.mute)
            .child(footer_line(self.shown, self.nodes.items().len()));
        let current = self.selected_node().cloned();
        let allocation = current
            .as_ref()
            .filter(|_| listed)
            .map(|node| self.allocation.render(node, cx));
        let shown = current.as_ref().map(|node| (node, node.name.as_str()));
        let motion = self.presence.track(shown, presence, cx);
        let inspector = match current {
            Some(_) => Some(slide_in(
                pane(&self.inspector),
                INSPECTOR_ID,
                motion,
                INSPECTOR_NODE,
            )),
            None => self
                .presence
                .leaving()
                .map(|_| slide_out(pane(&self.inspector), INSPECTOR_ID, INSPECTOR_NODE)),
        };
        let dock = self.dock_view(cx);
        let main = div()
            .flex_1()
            .min_h_0()
            .flex()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(self.filter.clone())
                    .child(div().flex_1().min_h_0().child(body))
                    .child(footer)
                    .children(allocation),
            )
            .children(inspector);
        div().size_full().flex().flex_col().child(main).child(dock)
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
    shell: MenuItem,
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
        let shell_screen = screen.clone();
        let shell_target = target.target.clone();
        let shell = MenuItem::new(SHELL_ITEM, move |window, cx| {
            ask_shell(shell_target.clone(), shell_screen.clone(), window, cx);
        });
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
            shell,
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
            Some(node.cpu),
            Some(node.cpu_commitment),
            palette,
        ))
        .child(usage_row(
            "Memory",
            amount(node.memory.used, node.memory.allocatable, MEMORY_UNIT).labelled(),
            Some(node.memory),
            Some(node.memory_commitment),
            palette,
        ))
        .child(disk_row(node.disk, palette))
        .child(usage_row(
            "Pods",
            format!("{} / {}", node.pods, node.pod_capacity),
            Some(pods),
            None,
            palette,
        ))
        .into_any_element()
}

fn disk_row(disk: Option<Usage>, palette: &Palette) -> impl IntoElement {
    let figures = disk.map_or_else(
        || NOT_MEASURED.to_owned(),
        |disk| amount(disk.used, disk.allocatable, MEMORY_UNIT).labelled(),
    );
    usage_row(DISK_LABEL, figures, disk, None, palette)
}

fn usage_row(
    label: &'static str,
    figures: String,
    usage: Option<Usage>,
    commitment: Option<Commitment>,
    palette: &Palette,
) -> impl IntoElement {
    let fraction = usage.map_or(0., |usage| usage.fraction());
    let warning = pressure_color(fraction, palette);
    let share = match (usage, commitment) {
        (None, _) => NOT_MEASURED.to_owned(),
        (Some(_), Some(_)) => format!("{} {USED}", percent(fraction)),
        (Some(_), None) => percent(fraction),
    };
    let meter = Meter::new(fraction).color(warning.unwrap_or(palette.acc));
    let meter = match commitment {
        Some(commitment) => meter.request(commitment.requests),
        None => meter,
    };
    let sub = commitment.zip(usage).map(|(commitment, usage)| {
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
        Column::new("CPU requested", |r: &NodeRow, p| {
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
        Column::new("Memory requested", |r: &NodeRow, p| {
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
