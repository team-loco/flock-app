use flock_core::emptiness::Noun;
use flock_core::filter::{StatusFilter, label_values};
use flock_core::nodes::{Node, NodeSummary, Usage};
use flock_core::overview::{Capacity, Commitment, percent};
use flock_core::time::age;
use flock_core::units::{CPU_UNIT, MEMORY_UNIT};
use flock_core::{
    ClusterDetail, ClusterId, ClusterSummary, Command, Counts, Page, ResourceRef, Seen, Status,
};
use gpui::{
    AnyElement, AppContext, Context, Entity, EventEmitter, Hsla, IntoElement, ParentElement,
    Pixels, Render, SharedString, Styled, WeakEntity, Window, div, px,
};

use super::{NODE_KIND, ScreenEvent};
use crate::kit::{
    Button, CapacityMeter, Column, ConfirmDialog, FilterBar, FilterEvent, IconButton, Inspector,
    KeyValue, MenuItem, MetaLine, Meter, MiniMeter, Presence, SectionLabel, StatusGlyph,
    StatusText, Table, TableEvent, chip_label, confirm, slide_in, slide_out,
};
use crate::screens::{ListShape, empty_list, new_tab_item};
use crate::theme::{
    ActiveTheme, FONT_MONO, INSPECTOR, Palette, SPACE_2, SPACE_3, SPACE_4, SPACE_5, TEXT_MONO,
    TEXT_SMALL,
};

const NONE: &str = "none";
const VERSION_SEPARATOR: char = '-';
const NAME_WEIGHT: f32 = 1.4;
const NAME_MIN: Pixels = px(150.);
const STATUS_WEIGHT: f32 = 0.8;
const STATUS_MIN: Pixels = px(150.);
const INSTANCE_WEIGHT: f32 = 0.5;
const INSTANCE_WIDTH: Pixels = px(96.);
const ZONE_WIDTH: Pixels = px(80.);
const USAGE_WIDTH: Pixels = px(84.);
const PODS_WIDTH: Pixels = px(60.);
const VERSION_WIDTH: Pixels = px(64.);
const AGE_WIDTH: Pixels = px(40.);
const INSPECTOR_LABEL: Pixels = px(84.);
const CONDITION_LABEL: Pixels = px(112.);
const INSPECTOR_ID: &str = "node-inspector";

pub struct NodesScreen {
    cluster: Option<ClusterId>,
    cluster_name: String,
    shown: usize,
    summary: NodeSummary,
    nodes: Vec<Node>,
    confirm_destructive: bool,
    selected: Option<String>,
    table: Entity<Table<Node>>,
    filter: Entity<FilterBar>,
    presence: Presence<Node>,
}

fn presence(screen: &mut NodesScreen) -> &mut Presence<Node> {
    &mut screen.presence
}

impl EventEmitter<ScreenEvent> for NodesScreen {}

impl NodesScreen {
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
                .row_key(|n: &Node| n.name.clone().into())
                .row_status(|n: &Node| (n.status != Status::Ok).then_some(n.status))
        });
        cx.subscribe(&table, |this, table, event: &TableEvent, cx| {
            let TableEvent::Selected(ix) = event else {
                return;
            };
            let name = table.read(cx).row(*ix).map(|n| n.name.clone());
            this.select(name, cx);
        })
        .detach();
        let filter = cx.new(|cx| FilterBar::new("node-filter", window, cx));
        cx.subscribe(&filter, |this, _, FilterEvent::Changed, cx| {
            this.refresh(cx);
        })
        .detach();
        Self {
            cluster: None,
            cluster_name: String::new(),
            shown: 0,
            summary: NodeSummary::default(),
            nodes: Vec::new(),
            confirm_destructive: true,
            selected: None,
            table,
            filter,
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
        if self.confirm_destructive != confirm_destructive {
            self.confirm_destructive = confirm_destructive;
            self.refresh(cx);
        }
        if self.nodes == detail.nodes {
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
        self.refresh(cx);
    }

    pub fn filter_bar(&self) -> &Entity<FilterBar> {
        &self.filter
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let node_labels: Vec<_> = self.nodes.iter().map(Node::label_map).collect();
        let labels = label_values(&node_labels);
        self.filter.update(cx, |bar, cx| bar.set_labels(labels, cx));
        let (query, chip) = {
            let bar = self.filter.read(cx);
            (bar.query().clone(), bar.chip())
        };
        let base: Vec<&Node> = self.nodes.iter().filter(|n| n.matches(&query)).collect();
        let chips = StatusFilter::counts(base.iter().map(|n| n.status))
            .into_iter()
            .map(|(filter, n)| chip_label(filter.label(), n))
            .collect();
        let chip = StatusFilter::at(chip);
        let rows: Vec<Node> = base
            .into_iter()
            .filter(|n| chip.keeps(n.status))
            .cloned()
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
        cx.notify();
    }

    pub fn reveal(&mut self, cluster: ClusterId, name: String, cx: &mut Context<Self>) {
        self.cluster = Some(cluster);
        self.selected = Some(name);
        self.select_in_table(cx);
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
                .position(|n| n.name == name);
            if position.is_some() {
                table.select(position, cx);
            }
        });
    }

    pub fn header_note(&self) -> (Counts, String) {
        let counts = Counts::of(self.nodes.iter().map(|n| n.status));
        let ready = format!("{} / {} ready", self.summary.ready, self.summary.total);
        (counts, ready)
    }

    fn select(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        self.selected = name;
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

    fn target(&self, node: &Node) -> Option<Seen> {
        let cluster = self.cluster.as_ref()?;
        Some(node_target(node, cluster))
    }

    fn row_actions(&self, cx: &mut Context<Self>) -> impl Fn(&Node) -> Vec<MenuItem> + 'static {
        let cluster = self.cluster.clone();
        let confirm_destructive = self.confirm_destructive;
        let screen = cx.weak_entity();
        move |node: &Node| {
            let Some(cluster) = cluster.clone() else {
                return Vec::new();
            };
            node_items(node, &cluster, confirm_destructive, screen.clone())
        }
    }

    fn inspector(&self, node: &Node, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let target = self.target(node);
        let primary = target.map(|target| {
            let (label, command) = if node.schedulable {
                ("Cordon", Command::Cordon(target))
            } else {
                ("Uncordon", Command::Uncordon(target))
            };
            Button::new("node-primary", label)
                .primary()
                .disabled(node.status == Status::Progress)
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(ScreenEvent::Command(command.clone()));
                }))
        });
        let close = IconButton::glyph("close-node", "×", "Close")
            .on_click(cx.listener(|this, _, _, cx| this.close(cx)));
        let facts = KeyValue::new()
            .label_width(INSPECTOR_LABEL)
            .text("instance", node.instance_type.clone())
            .text("zone", node.zone.clone())
            .text("internal IP", node.internal_ip.clone())
            .text("kubelet", node.version.clone())
            .text("OS image", node.os_image.clone())
            .text("runtime", node.runtime.clone())
            .text("age", age(node.age));
        let capacity = node_capacity(node, palette);
        let conditions = node.conditions.iter().map(|c| {
            let status = if c.healthy { Status::Ok } else { Status::Err };
            div()
                .flex()
                .items_center()
                .gap(SPACE_4)
                .child(StatusGlyph::new(status))
                .child(
                    div()
                        .w(CONDITION_LABEL)
                        .flex_none()
                        .truncate()
                        .child(c.kind.clone()),
                )
                .child(
                    div()
                        .font_family(FONT_MONO)
                        .text_size(TEXT_SMALL)
                        .text_color(palette.fg2)
                        .flex_none()
                        .child(c.value.clone()),
                )
                .child(
                    div()
                        .truncate()
                        .text_color(palette.mute)
                        .child(c.message.clone()),
                )
        });
        Inspector::new(INSPECTOR_ID, node.name.clone())
            .mono()
            .status(node.status, node.state.clone())
            .actions(div().flex().gap(SPACE_3).children(primary).child(close))
            .section(None, facts.into_any_element())
            .section(Some("Capacity"), capacity.into_any_element())
            .section(
                Some("Conditions"),
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_3)
                    .children(conditions)
                    .into_any_element(),
            )
            .section(Some("Taints"), label_list(&node.taints, palette))
            .section(Some("Labels"), label_list(&node.labels, palette))
            .into_any_element()
    }
}

impl Render for NodesScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let meta = self.summary.meta_line();
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
        let current = self.selected_node().cloned();
        let shown = current.as_ref().map(|node| (node, node.name.as_str()));
        let motion = self.presence.track(shown, presence, cx);
        let inspector = match current {
            Some(node) => {
                let panel = self.inspector(&node, &palette, cx);
                Some(slide_in(panel, INSPECTOR_ID, motion, INSPECTOR))
            }
            None => self.presence.leaving().cloned().map(|node| {
                let panel = self.inspector(&node, &palette, cx);
                slide_out(panel, INSPECTOR_ID, INSPECTOR)
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
                    .child(MetaLine::new(meta))
                    .child(self.filter.clone())
                    .child(div().flex_1().min_h_0().child(body)),
            )
            .children(inspector)
    }
}

fn node_target(node: &Node, cluster: &ClusterId) -> Seen {
    let target = ResourceRef {
        cluster: cluster.clone(),
        kind: NODE_KIND.into(),
        namespace: None,
        name: node.name.clone(),
    };
    Seen::new(target, node.uid.clone())
}

fn node_items(
    node: &Node,
    cluster: &ClusterId,
    confirm_destructive: bool,
    screen: WeakEntity<NodesScreen>,
) -> Vec<MenuItem> {
    let target = node_target(node, cluster);
    let open_screen = screen.clone();
    let open = new_tab_item(
        Command::OpenResource(target.target.clone()),
        move |command, cx| {
            let _ = open_screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)));
        },
    );
    let (label, description, command) = if node.schedulable {
        (
            "Cordon",
            "Stop scheduling new pods here; running pods stay.",
            Command::Cordon(target.clone()),
        )
    } else {
        (
            "Uncordon",
            "Let the scheduler place new pods here again.",
            Command::Uncordon(target.clone()),
        )
    };
    let toggle_screen = screen.clone();
    let toggle = MenuItem::new(label, move |_, cx| {
        let command = command.clone();
        let _ = toggle_screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)));
    })
    .disabled(node.status == Status::Progress)
    .description(description);
    let detail = format!("{} · {}", target.target.cluster, target.target.name);
    vec![
        open,
        toggle,
        MenuItem::new("Drain…", move |window, cx| {
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
        .destructive()
        .description("Cordon, then evict every pod except DaemonSet pods."),
    ]
}

fn label_list(items: &[String], palette: &Palette) -> AnyElement {
    let lines: Vec<AnyElement> = if items.is_empty() {
        vec![
            div()
                .text_color(palette.mute)
                .child(NONE)
                .into_any_element(),
        ]
    } else {
        items
            .iter()
            .map(|item| div().truncate().child(item.clone()).into_any_element())
            .collect()
    };
    div()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .font_family(FONT_MONO)
        .text_size(TEXT_SMALL)
        .text_color(palette.fg2)
        .children(lines)
        .into_any_element()
}

fn count(value: u32) -> f32 {
    f32::from(u16::try_from(value).unwrap_or(u16::MAX))
}

fn short_version(version: &str) -> SharedString {
    let short = version.split(VERSION_SEPARATOR).next().unwrap_or(version);
    short.to_owned().into()
}

fn node_capacity(node: &Node, palette: &Palette) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(SPACE_5)
        .child(capacity_row(
            "CPU",
            node.cpu,
            node.cpu_commitment,
            CPU_UNIT,
            palette.acc,
        ))
        .child(capacity_row(
            "Memory",
            node.memory,
            node.memory_commitment,
            MEMORY_UNIT,
            palette.ai,
        ))
        .child(usage_row(
            "Pods",
            Usage {
                used: count(node.pods),
                allocatable: count(node.pod_capacity),
            },
            palette,
        ))
}

fn capacity_row(
    label: &'static str,
    usage: Usage,
    commitment: Commitment,
    unit: &str,
    color: Hsla,
) -> impl IntoElement {
    let capacity = Capacity {
        used: usage.used,
        total: usage.allocatable,
        unit: unit.into(),
        requests: commitment.requests,
        limits: commitment.limits,
    };
    let id = format!("node-capacity-{label}");
    div()
        .flex()
        .flex_col()
        .gap(SPACE_4)
        .child(SectionLabel::new(label))
        .child(CapacityMeter::new(id, capacity, color))
}

fn usage_row(label: &'static str, usage: Usage, palette: &Palette) -> impl IntoElement {
    let amount = format!("{} / {}", usage.used, usage.allocatable);
    div()
        .flex()
        .flex_col()
        .gap(SPACE_3)
        .child(
            div()
                .flex()
                .child(SectionLabel::new(label))
                .child(div().flex_1())
                .child(
                    div()
                        .font_family(FONT_MONO)
                        .text_size(TEXT_MONO)
                        .text_color(palette.mute)
                        .child(format!("{amount} · {}", percent(usage.fraction()))),
                ),
        )
        .child(Meter::new(usage.fraction()))
}

fn usage_cell(usage: Usage, color: Hsla) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(SPACE_4)
        .child(MiniMeter::new(usage.fraction()).color(color))
        .child(percent(usage.fraction()))
        .into_any_element()
}

fn columns() -> Vec<Column<Node>> {
    vec![
        Column::status_text(
            "Name",
            |n: &Node| n.status,
            |n: &Node| n.name.clone().into(),
        )
        .highlight()
        .mono()
        .flex(NAME_WEIGHT, NAME_MIN)
        .sort_by_key(|n: &Node| n.name.clone()),
        Column::new("Status", |n: &Node, _| {
            StatusText::new(n.status, n.state.clone()).into_any_element()
        })
        .flex(STATUS_WEIGHT, STATUS_MIN)
        .sort_by_key(|n: &Node| std::cmp::Reverse(n.status)),
        Column::colored_text(
            "Instance",
            |n: &Node| n.instance_type.clone().into(),
            |_, p| p.fg2,
        )
        .mono()
        .flex(INSTANCE_WEIGHT, INSTANCE_WIDTH)
        .sort_by_key(|n: &Node| n.instance_type.clone()),
        Column::colored_text("Zone", |n: &Node| n.zone.clone().into(), |_, p| p.fg2)
            .mono()
            .fixed(ZONE_WIDTH)
            .sort_by_key(|n: &Node| n.zone.clone()),
        Column::new("CPU", |n: &Node, p| usage_cell(n.cpu, p.acc))
            .mono()
            .fixed(USAGE_WIDTH)
            .sortable(|a: &Node, b: &Node| a.cpu.fraction().total_cmp(&b.cpu.fraction())),
        Column::new("Memory", |n: &Node, p| usage_cell(n.memory, p.ai))
            .mono()
            .fixed(USAGE_WIDTH)
            .sortable(|a: &Node, b: &Node| a.memory.fraction().total_cmp(&b.memory.fraction())),
        Column::text("Pods", |n: &Node| {
            format!("{}/{}", n.pods, n.pod_capacity).into()
        })
        .mono()
        .end()
        .fixed(PODS_WIDTH)
        .sort_by_key(|n: &Node| n.pods),
        Column::colored_text(
            "Version",
            |n: &Node| short_version(&n.version),
            |_, p| p.fg2,
        )
        .mono()
        .fixed(VERSION_WIDTH),
        Column::colored_text("Age", |n: &Node| age(n.age).into(), |_, p| p.mute)
            .mono()
            .fixed(AGE_WIDTH)
            .sort_by_key(|n: &Node| n.age),
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
}
