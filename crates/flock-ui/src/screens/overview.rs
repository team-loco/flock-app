use std::rc::Rc;

use flock_core::events::Event;
use flock_core::events::EventFilter;
use flock_core::nodes::NodeSummary;
use flock_core::overview::{
    Capacity, PodPhases, TimeRange, Utilization, UtilizationSource, WorkloadHealth, percent,
    plural, thousands,
};
use flock_core::time::age;
use flock_core::{ClusterDetail, ClusterId, ClusterSummary, Command, Page, ResourceRef, Status};
use gpui::{
    AnyElement, App, AppContext, Context, ElementId, Entity, EventEmitter, FontWeight, Hsla,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, WeakEntity, Window, div, px,
};

use super::ScreenEvent;
use super::event_inspector::{EventInspector, slot};
use crate::kit::{
    ActionMenu, Button, CapacityMeter, ClickableSection, Column, EmptyState, ErrorState, LineChart,
    Link, MenuItem, MetaLine, Presence, SectionHeader, SectionLabel, Segmented, Series, StackedBar,
    StatusText, Table, TableEvent, Tooltip,
};
use crate::theme::{
    ActiveTheme, FONT_MONO, METER, Palette, RADIUS_SMALL, RULE, SPACE_1, SPACE_3, SPACE_4, SPACE_5,
    SPACE_7, SPACE_8, TEXT_BODY, TEXT_CONTROL, TEXT_FIGURE, TEXT_MONO, TEXT_SMALL,
};

const RECENT_WARNINGS: usize = 5;
const LIVE: &str = "Live";
const LIVE_TOOLTIP: &str = "Follow new data; turn off to hold the current view";
const CHART_MAX: f32 = 100.;
const Y_LABELS: [&str; 5] = ["100%", "75%", "50%", "25%", "0"];
const SEPARATOR: &str = " · ";
const NO_NAMESPACE: &str = "—";
const OBJECT_LINK: &str = "warning-object";
const ALL_EVENTS: &str = "All events →";
const UTILIZATION: &str = "Utilization";
const RETRY: &str = "Retry";
const SET_METRICS_SOURCE: &str = "Set metrics source";
const ACTIONS: &str = "overview-actions";
const CLUSTER_SETTINGS: &str = "Cluster settings";
const CLUSTER_SETTINGS_DESCRIPTION: &str =
    "Change this cluster's default namespace, metrics source and labels.";
const CHART_WEIGHT: f32 = 2.;
const REASON_WIDTH: Pixels = px(150.);
const OBJECT_WEIGHT: f32 = 1.2;
const OBJECT_MIN: Pixels = px(230.);
const NAMESPACE_WIDTH: Pixels = px(110.);
const MESSAGE_WEIGHT: f32 = 2.;
const MESSAGE_MIN: Pixels = px(260.);
const AGE_WIDTH: Pixels = px(56.);
const COUNT_WIDTH: Pixels = px(56.);

#[derive(Clone, Debug, PartialEq)]
struct OverviewData {
    meta: String,
    cluster: ClusterId,
    source: UtilizationSource,
    cpu: Capacity,
    memory: Capacity,
    pods: PodPhases,
    nodes: NodeSummary,
    node_cells: Vec<(Status, String)>,
    utilization: Vec<Utilization>,
    workloads: Vec<WorkloadHealth>,
    warnings: usize,
}

impl OverviewData {
    fn new(cluster: &ClusterSummary, detail: &ClusterDetail) -> Self {
        let version = cluster.version.clone().unwrap_or_default();
        let namespaces = plural(detail.namespaces.len(), "namespace", "namespaces");
        let metrics = format!("metrics {}", detail.overview.metrics_source);
        let meta = [
            cluster.distro.clone(),
            version,
            cluster.region.clone(),
            namespaces,
            metrics,
        ];
        let node_cells = detail
            .nodes
            .iter()
            .map(|n| (n.status, format!("{}{SEPARATOR}{}", n.name, n.state)))
            .collect();
        Self {
            meta: meta.join(SEPARATOR),
            cluster: cluster.id.clone(),
            source: detail.overview.utilization_source.clone(),
            cpu: detail.cpu(),
            memory: detail.memory(),
            pods: detail.overview.pods,
            nodes: detail.node_summary(),
            node_cells,
            utilization: detail.overview.utilization.clone(),
            workloads: detail.workload_health(),
            warnings: detail.warnings(),
        }
    }
}

#[derive(Clone)]
struct WarningRow {
    event: Event,
    target: Option<ResourceRef>,
}

struct Warnings {
    rows: Vec<WarningRow>,
    events: Vec<Event>,
}

impl Warnings {
    fn new(cluster: &ClusterSummary, detail: &ClusterDetail) -> Self {
        let rows = detail
            .recent_warnings(RECENT_WARNINGS)
            .into_iter()
            .map(|event| WarningRow {
                target: event.target(&cluster.id),
                event,
            })
            .collect();
        Self {
            rows,
            events: detail.events.clone(),
        }
    }
}

pub struct OverviewScreen {
    data: Option<OverviewData>,
    paused: Option<(OverviewData, Warnings)>,
    live: bool,
    range: TimeRange,
    events: Vec<Event>,
    selected: Option<String>,
    presence: Presence<Event>,
    warnings: Entity<Table<WarningRow>>,
    menu: Entity<ActionMenu>,
}

fn presence_of(screen: &mut OverviewScreen) -> &mut Presence<Event> {
    &mut screen.presence
}

impl EventEmitter<ScreenEvent> for OverviewScreen {}

impl OverviewScreen {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let screen = cx.weak_entity();
        let warnings = cx.new(|cx| {
            Table::new("overview-warnings", warning_columns(screen), cx)
                .empty("No warnings.")
                .fit_content()
                .click_opens()
                .row_key(|row: &WarningRow| row.event.key().into())
        });
        cx.subscribe(&warnings, |this, table, event: &TableEvent, cx| {
            if let TableEvent::Opened(ix) = event {
                this.selected = table.read(cx).row(*ix).map(|row| row.event.key());
                cx.notify();
            }
        })
        .detach();
        let menu = cx.new(|cx| ActionMenu::new(ACTIONS, cx));
        Self {
            data: None,
            paused: None,
            live: true,
            range: TimeRange::default(),
            events: Vec::new(),
            selected: None,
            presence: Presence::default(),
            warnings,
            menu,
        }
    }

    pub fn close_inspector(&mut self, cx: &mut Context<Self>) -> bool {
        if self.selected.is_none() {
            return false;
        }
        self.close(cx);
        true
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.warnings.update(cx, |table, cx| table.select(None, cx));
        cx.notify();
    }

    fn inspector(&self, event: &Event, cx: &mut Context<Self>) -> AnyElement {
        let target = self
            .data
            .as_ref()
            .and_then(|data| event.target(&data.cluster));
        let close_screen = cx.weak_entity();
        let open_screen = cx.weak_entity();
        EventInspector {
            event,
            events: &self.events,
            target,
            close: Rc::new(move |_, cx| {
                let _ = close_screen.update(cx, OverviewScreen::close);
            }),
            open: Rc::new(move |target, _, cx| {
                let command = Command::OpenResource(target);
                let _ = open_screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)));
            }),
        }
        .into_element()
    }

    pub fn set(
        &mut self,
        cluster: &ClusterSummary,
        detail: &ClusterDetail,
        cx: &mut Context<Self>,
    ) {
        let data = OverviewData::new(cluster, detail);
        if self.live || self.data.is_none() {
            if self.data.as_ref() != Some(&data) {
                self.show(data, Warnings::new(cluster, detail), cx);
            }
        } else {
            self.paused = Some((data, Warnings::new(cluster, detail)));
        }
    }

    fn show(&mut self, data: OverviewData, warnings: Warnings, cx: &mut Context<Self>) {
        if self.data.as_ref() == Some(&data) {
            return;
        }
        let cluster = data.cluster.clone();
        if self
            .data
            .as_ref()
            .is_some_and(|shown| shown.cluster != cluster)
        {
            self.selected = None;
        }
        if self
            .data
            .as_ref()
            .is_none_or(|shown| shown.cluster != cluster)
        {
            self.set_actions(cluster, cx);
        }
        self.data = Some(data);
        self.events = warnings.events;
        self.warnings
            .update(cx, |table, cx| table.set_rows(warnings.rows, cx));
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
        if let Some((data, warnings)) = self.paused.take().filter(|_| self.live) {
            self.show(data, warnings, cx);
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

    fn capacity(data: &OverviewData, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let cpu = capacity_cell("CPU", &data.cpu, palette.acc, palette);
        let memory = capacity_cell("Memory", &data.memory, palette.ai, palette);
        let pods = data.pods;
        let pod_bar = StackedBar::new(vec![
            (Status::Ok, count_weight(pods.running)),
            (Status::Warn, count_weight(pods.pending)),
            (Status::Err, count_weight(pods.failing)),
        ]);
        let pods_cell = Cell {
            label: "Pods",
            detail: format!("capacity {}", thousands(data.nodes.pod_capacity)),
            figure: thousands(pods.total()),
            bar: pod_bar.into_any_element(),
            notes: vec![
                counted(pods.running, "running", palette.fg),
                counted(pods.pending, "pending", palette.warn),
                counted(pods.failing, "failing", palette.err),
            ],
        };
        let nodes_cell = Cell {
            label: "Nodes",
            detail: data.nodes.compact_instance_label(),
            figure: data.nodes.total.to_string(),
            bar: Self::node_strip(data, palette, cx),
            notes: vec![
                counted_usize(data.nodes.ready, "ready", palette.fg),
                counted_usize(data.nodes.cordoned, "cordoned", palette.warn),
            ],
        };
        let pods_cell = Self::cell_section("overview-pods", Page::Pods, pods_cell, palette, cx);
        let nodes_cell = Self::cell_section("overview-nodes", Page::Nodes, nodes_cell, palette, cx);
        let cells = [cpu, memory, pods_cell, nodes_cell];
        let last = cells.len() - 1;
        let row = cells.into_iter().enumerate().map(|(ix, cell)| {
            let column = div().flex().flex_col().flex_1().min_w_0().child(cell);
            if ix < last {
                column.border_r(RULE).border_color(palette.line)
            } else {
                column
            }
        });
        div()
            .flex()
            .border_b(RULE)
            .border_color(palette.line)
            .children(row)
            .into_any_element()
    }

    fn cell_section(
        id: &'static str,
        page: Page,
        cell: Cell,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let detail = detail_text(cell.detail, palette);
        ClickableSection::new(id)
            .title(SectionLabel::new(cell.label))
            .trailing(detail)
            .child(cell_figure(cell.figure))
            .child(cell.bar)
            .children(cell_notes(cell.notes, palette))
            .on_activate(Self::navigate(page, cx))
            .into_any_element()
    }

    fn navigate(page: Page, cx: &mut Context<Self>) -> impl Fn(&mut Window, &mut App) + 'static {
        let this = cx.entity().downgrade();
        move |_, cx| {
            this.update(cx, |_, cx| cx.emit(ScreenEvent::Navigate(page)))
                .ok();
        }
    }

    fn node_strip(data: &OverviewData, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let segments: Vec<AnyElement> = data
            .node_cells
            .iter()
            .enumerate()
            .map(|(ix, (status, label))| {
                let color = palette.status(*status);
                let label: SharedString = label.clone().into();
                div()
                    .id(ElementId::NamedInteger("node-cell".into(), ix as u64))
                    .flex_1()
                    .h(METER)
                    .rounded(RADIUS_SMALL)
                    .bg(color)
                    .cursor_pointer()
                    .tooltip(Tooltip::text(label))
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(ScreenEvent::Navigate(Page::Nodes));
                    }))
                    .into_any_element()
            })
            .collect();
        div()
            .flex()
            .gap(SPACE_1)
            .children(segments)
            .into_any_element()
    }

    fn utilization(
        &self,
        data: &OverviewData,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut chart = LineChart::new(CHART_MAX)
            .y_labels(Y_LABELS.iter().map(|l| SharedString::from(*l)).collect())
            .x_labels(
                self.range
                    .axis()
                    .iter()
                    .map(|l| SharedString::from(*l))
                    .collect(),
            )
            .title(SectionLabel::new(UTILIZATION));
        let available = data
            .utilization
            .iter()
            .find(|u| u.range == self.range)
            .filter(|u| !u.cpu.is_empty() || !u.memory.is_empty());
        let mut note = None;
        if let Some(reason) = data.source.error() {
            let retry = Button::new("metrics-retry", RETRY).on_click(cx.listener(|_, _, _, cx| {
                cx.emit(ScreenEvent::Navigate(Page::ClusterOverview));
            }));
            chart = chart.message(ErrorState::new(reason, retry));
        } else if let Some(u) = available {
            chart = chart
                .series(
                    Series::new("CPU", palette.acc, u.cpu.clone())
                        .positions(u.cpu_positions.clone())
                        .area(palette.accbg),
                )
                .series(
                    Series::new("Memory", palette.ai, u.memory.clone())
                        .positions(u.memory_positions.clone()),
                )
                .threshold(u.cpu_requests, "CPU requests", palette.warn);
            note = data.source.note();
        } else {
            let cluster = data.cluster.clone();
            let settings = Button::new("metrics-settings", SET_METRICS_SOURCE)
                .primary()
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(ScreenEvent::Command(Command::ShowClusterSettings(
                        cluster.clone(),
                    )));
                }));
            let empty = EmptyState::new(UtilizationSource::empty_sentence(self.range))
                .selector("metrics-empty")
                .detail(data.source.empty_reason())
                .action(settings);
            chart = chart.message(empty);
        }
        let note = note.map(|note| {
            div()
                .debug_selector(|| "metrics-note".to_owned())
                .text_size(TEXT_CONTROL)
                .text_color(palette.mute)
                .child(note)
        });
        div()
            .debug_selector(|| "utilization-chart".to_owned())
            .flex()
            .flex_col()
            .gap(SPACE_4)
            .child(chart)
            .children(note)
            .into_any_element()
    }

    fn workloads(data: &OverviewData, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<AnyElement> = data
            .workloads
            .iter()
            .map(|w| {
                let page = w.page;
                let id = format!("overview-{}", page.label().to_lowercase());
                let summary = div()
                    .font_family(FONT_MONO)
                    .text_size(TEXT_MONO)
                    .text_color(palette.mute)
                    .child(w.summary.clone());
                let bar = StackedBar::new(vec![
                    (Status::Ok, count_weight_usize(w.ok)),
                    (Status::Warn, count_weight_usize(w.mid)),
                    (Status::Err, count_weight_usize(w.bad)),
                ])
                .thin();
                ClickableSection::new(SharedString::from(id))
                    .title(div().text_size(TEXT_BODY).child(page.label()))
                    .trailing(summary)
                    .padding(SPACE_8, SPACE_4)
                    .gap(SPACE_3)
                    .child(bar)
                    .on_activate(Self::navigate(page, cx))
                    .into_any_element()
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(SPACE_8)
                    .pt(SPACE_7)
                    .pb(SPACE_4)
                    .child(SectionLabel::new("Workloads")),
            )
            .children(rows)
            .pb(SPACE_4)
            .into_any_element()
    }
}

impl Render for OverviewScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let Some(data) = self.data.clone() else {
            return div().into_any_element();
        };
        let all_events =
            Link::new("all-events", ALL_EVENTS).on_click(cx.listener(|_, _, _, cx| {
                cx.emit(ScreenEvent::ShowEvents(EventFilter::Warnings));
            }));
        let current = self
            .selected
            .as_ref()
            .and_then(|key| self.events.iter().find(|event| &event.key() == key))
            .cloned();
        let mut presence = std::mem::take(&mut self.presence);
        let inspector = slot(
            current,
            &mut presence,
            presence_of,
            |event, cx| self.inspector(event, cx),
            cx,
        );
        self.presence = presence;
        let content = div()
            .id("overview-scroll")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .child(self.sections(&data, &palette, all_events, cx));
        div()
            .size_full()
            .flex()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && this.selected.is_some() {
                    this.close(cx);
                    cx.stop_propagation();
                }
            }))
            .child(content)
            .children(inspector)
            .into_any_element()
    }
}

impl OverviewScreen {
    fn sections(
        &self,
        data: &OverviewData,
        palette: &Palette,
        all_events: Link,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let middle = div()
            .flex()
            .border_b(RULE)
            .border_color(palette.line)
            .child(
                div()
                    .flex_grow(CHART_WEIGHT)
                    .flex_basis(Pixels::ZERO)
                    .min_w_0()
                    .px(SPACE_8)
                    .py(SPACE_7)
                    .border_r(RULE)
                    .border_color(palette.line)
                    .child(self.utilization(data, palette, cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Self::workloads(data, palette, cx)),
            );
        div()
            .flex()
            .flex_col()
            .tab_group()
            .pb(SPACE_8)
            .child(MetaLine::new(data.meta.clone()))
            .child(Self::capacity(data, palette, cx))
            .child(middle)
            .child(
                SectionHeader::new("Warnings")
                    .annotation(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(TEXT_SMALL)
                            .text_color(warnings_color(data.warnings, palette))
                            .child(data.warnings.to_string()),
                    )
                    .end(div().text_size(TEXT_CONTROL).child(all_events)),
            )
            .child(
                div()
                    .border_t(RULE)
                    .border_color(palette.line)
                    .child(self.warnings.clone()),
            )
            .into_any_element()
    }
}

fn warning_columns(screen: WeakEntity<OverviewScreen>) -> Vec<Column<WarningRow>> {
    vec![
        Column::new("Reason", |row: &WarningRow, _| {
            StatusText::new(row.event.status, row.event.reason.clone()).into_any_element()
        })
        .fixed(REASON_WIDTH),
        Column::new("Object", move |row: &WarningRow, _| {
            object_cell(row, &screen)
        })
        .flex(OBJECT_WEIGHT, OBJECT_MIN),
        Column::colored_text(
            "Namespace",
            |row: &WarningRow| {
                row.event
                    .namespace
                    .clone()
                    .unwrap_or_else(|| NO_NAMESPACE.into())
                    .into()
            },
            |_, palette| palette.fg2,
        )
        .fixed(NAMESPACE_WIDTH),
        Column::colored_text(
            "Message",
            |row: &WarningRow| row.event.message.clone().into(),
            |_, palette| palette.fg2,
        )
        .selectable()
        .tooltip(|row: &WarningRow| row.event.message.clone().into())
        .flex(MESSAGE_WEIGHT, MESSAGE_MIN),
        Column::colored_text(
            "Age",
            |row: &WarningRow| age(row.event.age).into(),
            |_, palette| palette.mute,
        )
        .mono()
        .fixed(AGE_WIDTH),
        Column::colored_text(
            "Count",
            |row: &WarningRow| row.event.count.to_string().into(),
            |_, palette| palette.mute,
        )
        .mono()
        .end()
        .fixed(COUNT_WIDTH),
    ]
}

fn object_cell(row: &WarningRow, screen: &WeakEntity<OverviewScreen>) -> AnyElement {
    let object = row.event.object.clone();
    let Some(target) = row.target.clone() else {
        return div()
            .truncate()
            .font_family(FONT_MONO)
            .child(object)
            .into_any_element();
    };
    let screen = screen.clone();
    let id = format!("{OBJECT_LINK}-{object}");
    let link = Link::new(SharedString::from(id), object)
        .mono()
        .on_click(move |_, _, cx| {
            let command = Command::OpenResource(target.clone());
            let _ = screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)));
        });
    div()
        .min_w_0()
        .overflow_hidden()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(link)
        .into_any_element()
}

fn warnings_color(warnings: usize, palette: &Palette) -> Hsla {
    if warnings > 0 {
        palette.warn
    } else {
        palette.mute
    }
}

fn count_weight(count: u32) -> f32 {
    f32::from(u16::try_from(count).unwrap_or(u16::MAX))
}

fn count_weight_usize(count: usize) -> f32 {
    f32::from(u16::try_from(count).unwrap_or(u16::MAX))
}

fn counted(value: u32, label: &'static str, color: Hsla) -> AnyElement {
    div()
        .flex()
        .gap(SPACE_3)
        .child(
            div()
                .text_color(color)
                .font_weight(FontWeight::MEDIUM)
                .child(thousands(value)),
        )
        .child(label)
        .into_any_element()
}

fn counted_usize(value: usize, label: &'static str, color: Hsla) -> AnyElement {
    let value = u32::try_from(value).unwrap_or(u32::MAX);
    counted(value, label, color)
}

struct Cell {
    label: &'static str,
    detail: String,
    figure: String,
    bar: AnyElement,
    notes: Vec<AnyElement>,
}

fn capacity_cell(
    label: &'static str,
    capacity: &Capacity,
    color: Hsla,
    palette: &Palette,
) -> AnyElement {
    let id = format!("overview-capacity-{label}");
    let meter = CapacityMeter::new(id, capacity.clone(), color);
    let cell = Cell {
        label,
        detail: capacity.amount(),
        figure: percent(capacity.fraction()),
        bar: meter.into_any_element(),
        notes: Vec::new(),
    };
    static_cell(cell, palette)
}

fn static_cell(cell: Cell, palette: &Palette) -> AnyElement {
    let header = div()
        .flex()
        .items_center()
        .gap(SPACE_5)
        .child(SectionLabel::new(cell.label))
        .child(div().flex_1())
        .child(detail_text(cell.detail, palette));
    div()
        .flex()
        .flex_col()
        .gap(SPACE_5)
        .px(SPACE_8)
        .py(SPACE_7)
        .child(header)
        .child(cell_figure(cell.figure))
        .child(cell.bar)
        .children(cell_notes(cell.notes, palette))
        .into_any_element()
}

fn detail_text(detail: String, palette: &Palette) -> AnyElement {
    div()
        .font_family(FONT_MONO)
        .text_size(TEXT_MONO)
        .text_color(palette.mute)
        .truncate()
        .child(detail)
        .into_any_element()
}

fn cell_figure(figure: String) -> AnyElement {
    div()
        .text_size(TEXT_FIGURE)
        .font_weight(FontWeight::SEMIBOLD)
        .child(figure)
        .into_any_element()
}

fn cell_notes(notes: Vec<AnyElement>, palette: &Palette) -> Option<AnyElement> {
    if notes.is_empty() {
        return None;
    }
    let row = div()
        .flex()
        .gap(SPACE_7)
        .text_size(TEXT_CONTROL)
        .text_color(palette.mute)
        .children(notes);
    Some(row.into_any_element())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flock_core::ClusterId;
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
            problems: flock_core::Counts::default(),
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
            "EKS · v1.31.2 · us-east-1 · 0 namespaces · metrics monitoring/prometheus-k8s"
        );
    }
}
