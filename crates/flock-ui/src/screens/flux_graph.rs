use flock_core::flux_graph::{CrossCluster, EdgeKind, FluxGraph, GraphEdge, GraphNode, Step};
use flock_core::overview::plural;
use flock_core::{ClusterId, ClusterSummary, Command, Counts, FluxKey, Snapshot};
use gpui::{
    AnyElement, App, Bounds, Context, EventEmitter, FocusHandle, Focusable, FontWeight, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, PathBuilder, Pixels,
    Point, Render, SharedString, StatefulInteractiveElement, Styled, Window, canvas, div, point,
    prelude::FluentBuilder,
};

use super::ScreenEvent;
use crate::kit::{EmptyState, Matrix, MatrixCell, SectionLabel, StatusGlyph, StatusText};
use crate::theme::{
    ActiveTheme, CHART_DASH, FIELD_LABEL, FONT_MONO, GRAPH_COLUMN_GAP, GRAPH_COMPARE_COLUMN,
    GRAPH_EDGE, GRAPH_EDGE_ACTIVE, GRAPH_LEGEND_SWATCH, GRAPH_NODE_HEIGHT, GRAPH_NODE_WIDTH,
    GRAPH_ROW_GAP, Palette, RADIUS, RULE, SPACE_1, SPACE_2, SPACE_3, SPACE_4, SPACE_6, SPACE_7,
    SPACE_8, TEXT_CONTROL, TEXT_MONO, TEXT_SMALL,
};

const NODE_SELECTOR: &str = "flux-graph-node";
const CANVAS_ID: &str = "flux-graph";
const COMPARE_ID: &str = "flux-graph-compare";
const COMPARE_LABEL: &str = "Across clusters";
const DEPENDS_ON: &str = "depends on";
const SOURCE: &str = "source";
const MISSING: &str = "not found";
const HINT: &str =
    "Click an object to open it. Arrow keys select one to compare across clusters; Enter opens it.";
const ONLY_HERE: &str = "Only on this cluster.";
const NOTHING: &str = "No Flux objects on";
const HALF: f32 = 0.5;

pub struct FluxGraphScreen {
    cluster: Option<ClusterId>,
    cluster_name: String,
    scope: Snapshot,
    graph: FluxGraph,
    selected: Option<FluxKey>,
    focus: FocusHandle,
}

impl EventEmitter<ScreenEvent> for FluxGraphScreen {}

impl Focusable for FluxGraphScreen {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl FluxGraphScreen {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            cluster: None,
            cluster_name: String::new(),
            scope: Snapshot::default(),
            graph: FluxGraph::default(),
            selected: None,
            focus: cx.focus_handle(),
        }
    }

    pub fn set(&mut self, cluster: &ClusterSummary, snapshot: &Snapshot, cx: &mut Context<Self>) {
        let moved = self.cluster.as_ref() != Some(&cluster.id);
        if !moved && self.scope.flux == snapshot.flux && self.scope.clusters == snapshot.clusters {
            return;
        }
        if moved {
            self.selected = None;
        }
        self.cluster = Some(cluster.id.clone());
        self.cluster_name.clone_from(&cluster.name);
        self.scope = Snapshot {
            clusters: snapshot.clusters.clone(),
            flux: snapshot.flux.clone(),
            ..Snapshot::default()
        };
        self.graph = FluxGraph::build(&cluster.id, &snapshot.flux);
        cx.notify();
    }

    pub fn header_note(&self) -> (Counts, String) {
        let present: Vec<&GraphNode> = self.graph.nodes.iter().filter(|n| n.present).collect();
        let counts = Counts::of(present.iter().map(|node| node.status));
        (counts, plural(present.len(), "object", "objects"))
    }

    fn selected_ix(&self) -> Option<usize> {
        self.selected.as_ref().and_then(|key| self.graph.find(key))
    }

    fn select(&mut self, ix: Option<usize>, cx: &mut Context<Self>) {
        self.selected = ix
            .and_then(|ix| self.graph.nodes.get(ix))
            .map(|node| node.key.clone());
        cx.notify();
    }

    fn open(&self, key: &FluxKey, cx: &mut Context<Self>) {
        let present = self.scope.flux.iter().any(|object| &object.key == key);
        if present {
            cx.emit(ScreenEvent::Command(Command::OpenFluxObject(key.clone())));
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.modified() {
            return;
        }
        let step = match event.keystroke.key.as_str() {
            "up" => Some(Step::Up),
            "down" => Some(Step::Down),
            "left" => Some(Step::Left),
            "right" => Some(Step::Right),
            _ => None,
        };
        if let Some(step) = step {
            let next = self.graph.step(self.selected_ix(), step);
            self.select(next, cx);
            cx.stop_propagation();
            return;
        }
        match event.keystroke.key.as_str() {
            "enter" => {
                if let Some(key) = self.selected.clone() {
                    self.open(&key, cx);
                    cx.stop_propagation();
                }
            }
            "escape" if self.selected.is_some() => {
                self.select(None, cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    fn node(
        &self,
        ix: usize,
        node: &GraphNode,
        palette: &Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let selected = self.selected.as_ref() == Some(&node.key);
        let origin = node_origin(node);
        let key = node.key.clone();
        let select_key = node.key.clone();
        let kind = if node.present {
            node.key.kind.label().to_owned()
        } else {
            format!("{} · {MISSING}", node.key.kind.label())
        };
        let detail = format!("{kind} · {}", node.key.namespace);
        let name_color = if node.present {
            palette.fg
        } else {
            palette.mute
        };
        div()
            .id((NODE_SELECTOR, ix))
            .debug_selector(move || format!("{NODE_SELECTOR}-{ix}"))
            .absolute()
            .left(origin.x)
            .top(origin.y)
            .w(GRAPH_NODE_WIDTH)
            .h(GRAPH_NODE_HEIGHT)
            .flex()
            .flex_col()
            .justify_center()
            .gap(SPACE_1)
            .px(SPACE_4)
            .rounded(RADIUS)
            .border(RULE)
            .border_color(if selected {
                palette.selline
            } else {
                palette.line2
            })
            .bg(if selected { palette.sel } else { palette.panel })
            .when(node.present, |this| {
                this.cursor_pointer().hover(|style| style.bg(palette.hover))
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                let focus = this.focus.clone();
                window.focus(&focus, cx);
                this.open(&key, cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, window, cx| {
                    let focus = this.focus.clone();
                    window.focus(&focus, cx);
                    let ix = this.graph.find(&select_key);
                    this.select(ix, cx);
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_3)
                    .min_w_0()
                    .child(StatusGlyph::new(node.status))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(FONT_MONO)
                            .text_size(TEXT_MONO)
                            .text_color(if selected { palette.selfg } else { name_color })
                            .child(node.key.name.clone()),
                    ),
            )
            .child(
                div()
                    .pl(SPACE_6)
                    .truncate()
                    .text_size(TEXT_SMALL)
                    .text_color(palette.mute)
                    .child(detail),
            )
            .into_any_element()
    }

    fn edges(&self, palette: &Palette) -> AnyElement {
        let selected = self.selected_ix();
        let lines: Vec<(Route, EdgeKind, Hsla, Pixels)> = self
            .graph
            .edges
            .iter()
            .filter_map(|edge| {
                let route = route(&self.graph, edge)?;
                let active = selected.is_some_and(|ix| self.graph.touches(edge, ix));
                let (color, width) = if active {
                    (palette.acc, GRAPH_EDGE_ACTIVE)
                } else {
                    (palette.line2, GRAPH_EDGE)
                };
                Some((route, edge.kind, color, width))
            })
            .collect();
        canvas(
            |_, _, _| {},
            move |bounds: Bounds<Pixels>, (), window, _| {
                for (route, kind, color, width) in &lines {
                    let mut path = PathBuilder::stroke(*width);
                    if *kind == EdgeKind::Source {
                        path = path.dash_array(&[CHART_DASH, CHART_DASH]);
                    }
                    let mut at = bounds.origin + route.start;
                    path.move_to(at);
                    for (enter, leave) in &route.passes {
                        let enter = bounds.origin + *enter;
                        curve(&mut path, at, enter);
                        at = bounds.origin + *leave;
                        path.line_to(at);
                    }
                    curve(&mut path, at, bounds.origin + route.end);
                    if let Ok(built) = path.build() {
                        window.paint_path(built, *color);
                    }
                }
            },
        )
        .absolute()
        .size_full()
        .into_any_element()
    }

    fn legend(palette: &Palette) -> impl IntoElement {
        let swatch = |dashed: bool| {
            let piece = || div().w(GRAPH_LEGEND_SWATCH).h(RULE).bg(palette.mute);
            if dashed {
                div().flex().gap(SPACE_2).child(piece()).child(piece())
            } else {
                div().child(
                    div()
                        .w(GRAPH_LEGEND_SWATCH * 2. + SPACE_2)
                        .h(RULE)
                        .bg(palette.mute),
                )
            }
        };
        let item = |dashed: bool, label: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(SPACE_3)
                .child(swatch(dashed))
                .child(label)
        };
        div()
            .flex()
            .items_center()
            .gap(SPACE_7)
            .px(SPACE_8)
            .py(SPACE_4)
            .border_b(RULE)
            .border_color(palette.line)
            .text_size(TEXT_CONTROL)
            .text_color(palette.mute)
            .child(item(false, DEPENDS_ON))
            .child(item(true, SOURCE))
            .child(div().flex_1())
            .child(div().truncate().child(HINT))
    }

    fn compare(across: &CrossCluster, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        let title = format!(
            "{} {}/{}",
            across.key.kind.label(),
            across.key.namespace,
            across.key.name
        );
        let columns: Vec<SharedString> = across
            .clusters
            .iter()
            .map(|(_, name)| SharedString::from(name.clone()))
            .collect();
        let mut matrix =
            Matrix::new(COMPARE_ID, "", columns).min_widths(FIELD_LABEL, GRAPH_COMPARE_COLUMN);
        for row in &across.rows {
            let cells = row
                .cells
                .iter()
                .zip(&across.clusters)
                .map(|(cell, (cluster, _))| {
                    let content: AnyElement = match cell.status {
                        Some(status) => {
                            StatusText::new(status, cell.text.clone()).into_any_element()
                        }
                        None => div()
                            .truncate()
                            .font_family(FONT_MONO)
                            .text_size(TEXT_MONO)
                            .text_color(if cell.differs {
                                palette.warn
                            } else {
                                palette.fg2
                            })
                            .child(cell.text.clone())
                            .into_any_element(),
                    };
                    let key = across.key.on(cluster);
                    MatrixCell::new(content).on_click(cx.listener(move |this, _, _, cx| {
                        this.open(&key, cx);
                    }))
                })
                .collect();
            matrix = matrix.row(div().text_color(palette.fg2).child(row.label), cells);
        }
        let only_here = (across.clusters.len() < 2).then(|| {
            div()
                .px(SPACE_8)
                .pb(SPACE_4)
                .text_color(palette.mute)
                .child(ONLY_HERE)
        });
        div()
            .debug_selector(|| COMPARE_ID.to_owned())
            .flex_none()
            .flex()
            .flex_col()
            .border_t(RULE)
            .border_color(palette.line)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_4)
                    .px(SPACE_8)
                    .py(SPACE_6)
                    .child(SectionLabel::new(COMPARE_LABEL))
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(TEXT_MONO)
                            .text_color(palette.fg2)
                            .font_weight(FontWeight::NORMAL)
                            .child(title),
                    ),
            )
            .children(only_here)
            .child(matrix)
            .into_any_element()
    }
}

fn node_origin(node: &GraphNode) -> Point<Pixels> {
    slot_origin(node.layer, node.row)
}

struct Route {
    start: Point<Pixels>,
    passes: Vec<(Point<Pixels>, Point<Pixels>)>,
    end: Point<Pixels>,
}

fn slot_origin(layer: usize, row: usize) -> Point<Pixels> {
    let column = GRAPH_NODE_WIDTH + GRAPH_COLUMN_GAP;
    let height = GRAPH_NODE_HEIGHT + GRAPH_ROW_GAP;
    point(
        SPACE_8 + column * position(layer),
        SPACE_8 + height * position(row),
    )
}

fn route(graph: &FluxGraph, edge: &GraphEdge) -> Option<Route> {
    let from = graph.nodes.get(edge.from)?;
    let to = graph.nodes.get(edge.to)?;
    let middle = GRAPH_NODE_HEIGHT * HALF;
    let start = node_origin(from);
    let end = node_origin(to);
    let passes = edge
        .via
        .iter()
        .enumerate()
        .map(|(step, row)| {
            let slot = slot_origin(from.layer + step + 1, *row);
            let y = slot.y + middle;
            (point(slot.x, y), point(slot.x + GRAPH_NODE_WIDTH, y))
        })
        .collect();
    Some(Route {
        start: point(start.x + GRAPH_NODE_WIDTH, start.y + middle),
        passes,
        end: point(end.x, end.y + middle),
    })
}

fn curve(path: &mut PathBuilder, from: Point<Pixels>, to: Point<Pixels>) {
    let bend = (to.x - from.x) * HALF;
    path.cubic_bezier_to(to, point(from.x + bend, from.y), point(to.x - bend, to.y));
}

#[allow(clippy::cast_precision_loss)]
const fn position(value: usize) -> f32 {
    value as f32
}

fn canvas_size(graph: &FluxGraph) -> (Pixels, Pixels) {
    let layers = position(graph.layers.len());
    let rows = position(graph.rows);
    let width = SPACE_8 * 2. + GRAPH_NODE_WIDTH * layers + GRAPH_COLUMN_GAP * (layers - 1.).max(0.);
    let height = SPACE_8 * 2. + GRAPH_NODE_HEIGHT * rows + GRAPH_ROW_GAP * (rows - 1.).max(0.);
    (width, height)
}

impl Render for FluxGraphScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        if self.graph.nodes.is_empty() {
            let sentence = format!("{NOTHING} {}.", self.cluster_name);
            return div()
                .size_full()
                .child(EmptyState::new(sentence))
                .into_any_element();
        }
        let (width, height) = canvas_size(&self.graph);
        let nodes: Vec<AnyElement> = self
            .graph
            .nodes
            .iter()
            .enumerate()
            .map(|(ix, node)| self.node(ix, node, &palette, cx))
            .collect();
        let across = self
            .selected
            .as_ref()
            .and_then(|key| self.scope.flux_across(key))
            .map(|across| Self::compare(&across, &palette, cx));
        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                this.key_down(event, cx);
            }))
            .child(Self::legend(&palette))
            .child(
                div()
                    .id(CANVAS_ID)
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .child(
                        div()
                            .relative()
                            .w(width)
                            .h(height)
                            .child(self.edges(&palette))
                            .children(nodes),
                    ),
            )
            .children(across)
            .into_any_element()
    }
}
