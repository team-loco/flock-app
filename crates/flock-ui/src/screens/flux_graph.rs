use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use flock_core::flux_graph::{CrossCluster, EdgeKind, FluxGraph, GraphEdge, GraphNode, Step};
use flock_core::overview::plural;
use flock_core::{ClusterId, ClusterSummary, Command, FluxKey, Snapshot};
use gpui::{
    AnyElement, App, Bounds, Context, EventEmitter, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Path, PathBuilder,
    PathVertex, Pixels, Point, Render, ScrollHandle, SharedString, Size,
    StatefulInteractiveElement, Styled, Window, canvas, div, point, prelude::FluentBuilder, size,
};

use super::ScreenEvent;
use crate::kit::scroll_viewport;
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
const CHUNK_VERTICES: usize = 192;

pub struct FluxGraphScreen {
    cluster: Option<ClusterId>,
    cluster_name: String,
    snapshot: Arc<Snapshot>,
    graph: FluxGraph,
    cards: Vec<Rc<Card>>,
    scroll: ScrollHandle,
    edge_paths: Rc<RefCell<Option<EdgePaths>>>,
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
            snapshot: Arc::default(),
            graph: FluxGraph::default(),
            cards: Vec::new(),
            scroll: ScrollHandle::new(),
            edge_paths: Rc::default(),
            selected: None,
            focus: cx.focus_handle(),
        }
    }

    pub fn set(
        &mut self,
        cluster: &ClusterSummary,
        snapshot: &Arc<Snapshot>,
        cx: &mut Context<Self>,
    ) {
        let moved = self.cluster.as_ref() != Some(&cluster.id);
        let unchanged = !moved
            && self.snapshot.flux == snapshot.flux
            && self.snapshot.clusters == snapshot.clusters;
        self.snapshot = Arc::clone(snapshot);
        if unchanged {
            return;
        }
        if moved {
            self.selected = None;
        }
        self.cluster = Some(cluster.id.clone());
        self.cluster_name.clone_from(&cluster.name);
        self.graph = FluxGraph::build(&cluster.id, &snapshot.flux);
        self.cards = self.graph.nodes.iter().map(Card::new).collect();
        self.restroke();
        cx.notify();
    }

    pub fn header_note(&self) -> String {
        let present: Vec<&GraphNode> = self.graph.nodes.iter().filter(|n| n.present).collect();
        plural(present.len(), "object", "objects")
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
        let present = self.snapshot.flux.iter().any(|object| &object.key == key);
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
        card: &Rc<Card>,
        palette: &Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let selected = self.selected.as_ref() == Some(&node.key);
        let origin = node_origin(node);
        let open = Rc::clone(card);
        let select = Rc::clone(card);
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
                this.open(&open.key, cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, window, cx| {
                    let focus = this.focus.clone();
                    window.focus(&focus, cx);
                    let ix = this.graph.find(&select.key);
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
                            .child(card.name.clone()),
                    ),
            )
            .child(
                div()
                    .pl(SPACE_6)
                    .truncate()
                    .text_size(TEXT_SMALL)
                    .text_color(palette.mute)
                    .child(card.detail.clone()),
            )
            .into_any_element()
    }

    fn restroke(&self) {
        let previous = self.edge_paths.take();
        let next = EdgePaths::new(&self.graph, self.selected_ix(), previous);
        self.edge_paths.replace(Some(next));
    }

    fn edges(&self, palette: &Palette) -> AnyElement {
        let selected = self.selected_ix();
        let stale = self
            .edge_paths
            .borrow()
            .as_ref()
            .is_none_or(|cached| cached.selected != selected);
        if stale {
            self.restroke();
        }
        let cache = Rc::clone(&self.edge_paths);
        let (active, idle) = (palette.acc, palette.edge);
        canvas(
            |_, _, _| {},
            move |bounds: Bounds<Pixels>, (), window, _| {
                let mut cache = cache.borrow_mut();
                let Some(cache) = cache.as_mut() else {
                    return;
                };
                let visible = window.content_mask().bounds;
                for edge in &mut cache.edges {
                    if !in_view(edge.extent, bounds.origin, visible) {
                        continue;
                    }
                    let color = if edge.active { active } else { idle };
                    let Some(tessellation) = edge.tessellated(bounds.origin) else {
                        continue;
                    };
                    window.paint_path(tessellation.visible(bounds.origin, visible), color);
                }
            },
        )
        .absolute()
        .size_full()
        .into_any_element()
    }

    fn legend(palette: &Palette) -> impl IntoElement {
        let swatch = |dashed: bool| {
            let piece = || div().w(GRAPH_LEGEND_SWATCH).h(RULE).bg(palette.edge);
            if dashed {
                div().flex().gap(SPACE_2).child(piece()).child(piece())
            } else {
                div().child(
                    div()
                        .w(GRAPH_LEGEND_SWATCH * 2. + SPACE_2)
                        .h(RULE)
                        .bg(palette.edge),
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

fn node_extent(node: &GraphNode) -> Bounds<Pixels> {
    Bounds::new(node_origin(node), size(GRAPH_NODE_WIDTH, GRAPH_NODE_HEIGHT))
}

fn in_view(extent: Bounds<Pixels>, by: Point<Pixels>, visible: Bounds<Pixels>) -> bool {
    let placed = Bounds {
        origin: extent.origin + by,
        size: extent.size,
    };
    visible.intersects(&placed)
}

fn viewport(offset: Point<Pixels>, content: Size<Pixels>, window: Size<Pixels>) -> Bounds<Pixels> {
    let scrolled = point(-offset.x, -offset.y);
    let room = content - window;
    let furthest = point(room.width, room.height).max(&Point::default());
    let origin = scrolled.max(&Point::default()).min(&furthest);
    Bounds::new(origin, window)
}

struct Card {
    key: FluxKey,
    name: SharedString,
    detail: SharedString,
}

impl Card {
    fn new(node: &GraphNode) -> Rc<Self> {
        let kind = if node.present {
            node.key.kind.label().to_owned()
        } else {
            format!("{} · {MISSING}", node.key.kind.label())
        };
        let detail = format!("{kind} · {}", node.key.namespace);
        Rc::new(Self {
            key: node.key.clone(),
            name: node.key.name.clone().into(),
            detail: detail.into(),
        })
    }
}

struct EdgePaths {
    selected: Option<usize>,
    edges: Vec<CachedEdge>,
}

impl EdgePaths {
    fn new(graph: &FluxGraph, selected: Option<usize>, previous: Option<Self>) -> Self {
        let mut previous = previous.map(|cache| cache.edges).unwrap_or_default();
        let edges = graph
            .edges
            .iter()
            .enumerate()
            .filter_map(|(at, edge)| {
                let route = route(graph, edge)?;
                let active = selected.is_some_and(|ix| graph.touches(edge, ix));
                let width = if active {
                    GRAPH_EDGE_ACTIVE
                } else {
                    GRAPH_EDGE
                };
                let mut cached = CachedEdge {
                    extent: extent(&route, width),
                    route,
                    kind: edge.kind,
                    width,
                    active,
                    stroke: Stroke::Pending,
                };
                if let Some(old) = previous.get_mut(at).filter(|old| old.draws_like(&cached)) {
                    cached.stroke = std::mem::replace(&mut old.stroke, Stroke::Pending);
                }
                Some(cached)
            })
            .collect();
        Self { selected, edges }
    }
}

struct CachedEdge {
    route: Route,
    kind: EdgeKind,
    width: Pixels,
    active: bool,
    extent: Bounds<Pixels>,
    stroke: Stroke,
}

impl CachedEdge {
    fn draws_like(&self, other: &Self) -> bool {
        self.route == other.route && self.kind == other.kind && self.width == other.width
    }

    fn tessellated(&mut self, origin: Point<Pixels>) -> Option<&Tessellation> {
        if matches!(self.stroke, Stroke::Pending) {
            self.stroke = match stroke(&self.route, self.kind, self.width, origin) {
                Some(path) => Stroke::Built(Box::new(Tessellation::new(path, origin))),
                None => Stroke::Failed,
            };
        }
        match &self.stroke {
            Stroke::Built(tessellation) => Some(tessellation),
            Stroke::Pending => None,
            Stroke::Failed => None,
        }
    }
}

enum Stroke {
    Pending,
    Failed,
    Built(Box<Tessellation>),
}

struct Tessellation {
    anchor: Point<Pixels>,
    outline: Path<Pixels>,
    vertices: Vec<PathVertex<Pixels>>,
    chunks: Vec<Bounds<Pixels>>,
}

impl Tessellation {
    fn new(mut outline: Path<Pixels>, anchor: Point<Pixels>) -> Self {
        let vertices = std::mem::take(&mut outline.vertices);
        let chunks = vertices
            .chunks(CHUNK_VERTICES)
            .filter_map(|run| {
                let first = run.first()?.xy_position;
                let (low, high) = run.iter().fold((first, first), |(low, high), vertex| {
                    (low.min(&vertex.xy_position), high.max(&vertex.xy_position))
                });
                Some(Bounds::from_corners(low, high))
            })
            .collect();
        Self {
            anchor,
            outline,
            vertices,
            chunks,
        }
    }

    fn visible(&self, origin: Point<Pixels>, visible: Bounds<Pixels>) -> Path<Pixels> {
        let delta = origin - self.anchor;
        let mut piece = self.outline.clone();
        piece.bounds.origin += delta;
        let runs = self.vertices.chunks(CHUNK_VERTICES).zip(&self.chunks);
        for (run, extent) in runs {
            if !in_view(*extent, delta, visible) {
                continue;
            }
            piece.vertices.extend(run.iter().map(|vertex| {
                let mut moved = vertex.clone();
                moved.xy_position += delta;
                moved
            }));
        }
        piece
    }
}

#[derive(PartialEq)]
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

fn extent(route: &Route, width: Pixels) -> Bounds<Pixels> {
    let points = route
        .passes
        .iter()
        .flat_map(|(enter, leave)| [*enter, *leave])
        .chain([route.start, route.end]);
    let (low, high) = points.fold((route.start, route.start), |(low, high), at| {
        (low.min(&at), high.max(&at))
    });
    let pad = point(width, width);
    Bounds::from_corners(low - pad, high + pad)
}

fn stroke(
    route: &Route,
    kind: EdgeKind,
    width: Pixels,
    origin: Point<Pixels>,
) -> Option<Path<Pixels>> {
    let mut path = PathBuilder::stroke(width);
    if kind == EdgeKind::Source {
        path = path.dash_array(&[CHART_DASH, CHART_DASH]);
    }
    let mut at = origin + route.start;
    path.move_to(at);
    for (enter, leave) in &route.passes {
        let enter = origin + *enter;
        curve(&mut path, at, enter);
        at = origin + *leave;
        path.line_to(at);
    }
    curve(&mut path, at, origin + route.end);
    path.build().ok()
}

fn curve(path: &mut PathBuilder, from: Point<Pixels>, to: Point<Pixels>) {
    let bend = (to.x - from.x) * HALF;
    path.cubic_bezier_to(to, point(from.x + bend, from.y), point(to.x - bend, to.y));
}

#[allow(clippy::cast_precision_loss)]
const fn position(value: usize) -> f32 {
    value as f32
}

fn canvas_size(graph: &FluxGraph) -> Size<Pixels> {
    let layers = position(graph.layers.len());
    let rows = position(graph.rows);
    let width = SPACE_8 * 2. + GRAPH_NODE_WIDTH * layers + GRAPH_COLUMN_GAP * (layers - 1.).max(0.);
    let height = SPACE_8 * 2. + GRAPH_NODE_HEIGHT * rows + GRAPH_ROW_GAP * (rows - 1.).max(0.);
    size(width, height)
}

impl Render for FluxGraphScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        if self.graph.nodes.is_empty() {
            let sentence = format!("{NOTHING} {}.", self.cluster_name);
            return div()
                .size_full()
                .child(EmptyState::new(sentence))
                .into_any_element();
        }
        let content = canvas_size(&self.graph);
        let visible = viewport(self.scroll.offset(), content, window.viewport_size());
        let nodes: Vec<AnyElement> = self
            .graph
            .nodes
            .iter()
            .zip(&self.cards)
            .enumerate()
            .filter(|(_, (node, _))| in_view(node_extent(node), Point::default(), visible))
            .map(|(ix, (node, card))| self.node(ix, node, card, &palette, cx))
            .collect();
        let across = self
            .selected
            .as_ref()
            .and_then(|key| self.snapshot.flux_across(key))
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
                scroll_viewport(
                    CANVAS_ID,
                    div()
                        .relative()
                        .w(content.width)
                        .h(content.height)
                        .child(self.edges(&palette))
                        .children(nodes),
                )
                .both_axes()
                .track_scroll(&self.scroll)
                .flex_1()
                .min_h_0(),
            )
            .children(across)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use gpui::{px, size};

    use super::*;

    const DELTA: f32 = 40.;
    const FAR: f32 = 100_000.;
    const PASSES: usize = 6;
    const WINDOW_SHARE: f32 = 0.25;

    fn long_route() -> Route {
        let column = GRAPH_NODE_WIDTH + GRAPH_COLUMN_GAP;
        let passes = (1..=PASSES)
            .map(|step| {
                let x = column * position(step);
                let y = GRAPH_NODE_HEIGHT * position(step * step);
                (point(x, y), point(x + GRAPH_NODE_WIDTH, y))
            })
            .collect();
        Route {
            start: point(GRAPH_NODE_WIDTH, SPACE_8),
            passes,
            end: point(
                column * position(PASSES + 1),
                GRAPH_NODE_HEIGHT * position(PASSES * PASSES),
            ),
        }
    }

    fn tessellated(kind: EdgeKind) -> (Path<Pixels>, Tessellation) {
        let Some(path) = stroke(&long_route(), kind, GRAPH_EDGE, Point::default()) else {
            panic!("the sample edge does not tessellate");
        };
        let tessellation = Tessellation::new(path.clone(), Point::default());
        (path, tessellation)
    }

    fn everywhere() -> Bounds<Pixels> {
        Bounds::new(point(px(-FAR), px(-FAR)), size(px(FAR * 2.), px(FAR * 2.)))
    }

    fn positions(path: &Path<Pixels>) -> Vec<Point<Pixels>> {
        path.vertices
            .iter()
            .map(|vertex| vertex.xy_position)
            .collect()
    }

    #[test]
    fn a_fully_visible_edge_paints_exactly_the_tessellated_path() {
        let (path, tessellation) = tessellated(EdgeKind::Source);
        assert!(tessellation.chunks.len() > 1);
        let painted = tessellation.visible(Point::default(), everywhere());
        assert_eq!(painted.bounds, path.bounds);
        assert_eq!(positions(&painted), positions(&path));
    }

    #[test]
    fn panning_translates_the_cached_vertices_and_bounds() {
        let (path, tessellation) = tessellated(EdgeKind::DependsOn);
        let by = point(px(DELTA), px(-DELTA));
        let painted = tessellation.visible(by, everywhere());
        let expected: Vec<Point<Pixels>> = positions(&path).iter().map(|at| *at + by).collect();
        assert_eq!(painted.bounds.origin, path.bounds.origin + by);
        assert_eq!(painted.bounds.size, path.bounds.size);
        assert_eq!(positions(&painted), expected);
    }

    #[test]
    fn only_chunks_in_view_are_painted() {
        let (path, tessellation) = tessellated(EdgeKind::Source);
        let start = long_route().start;
        let corner = Bounds::new(start, size(GRAPH_NODE_WIDTH, GRAPH_NODE_HEIGHT));
        let painted = tessellation.visible(Point::default(), corner);
        assert!(!painted.vertices.is_empty());
        assert!(painted.vertices.len() < path.vertices.len());
        assert_eq!(painted.bounds, path.bounds);
        let away = Bounds::new(point(px(-FAR), px(-FAR)), size(px(DELTA), px(DELTA)));
        assert!(
            tessellation
                .visible(Point::default(), away)
                .vertices
                .is_empty()
        );
    }

    fn window() -> Size<Pixels> {
        size(px(FAR * WINDOW_SHARE), px(FAR * WINDOW_SHARE))
    }

    fn content() -> Size<Pixels> {
        size(px(FAR), px(FAR))
    }

    #[test]
    fn the_viewport_follows_the_scroll_offset() {
        let offset = point(px(-DELTA), px(-DELTA * 2.));
        let visible = viewport(offset, content(), window());
        assert_eq!(visible.origin, point(px(DELTA), px(DELTA * 2.)));
        assert_eq!(visible.size, window());
    }

    #[test]
    fn overscroll_keeps_the_viewport_inside_the_content() {
        let past = point(px(-FAR * 2.), px(DELTA));
        let visible = viewport(past, content(), window());
        let room = content() - window();
        assert_eq!(visible.origin, point(room.width, px(0.)));
    }

    #[test]
    fn content_smaller_than_the_window_shows_from_the_top_left() {
        let small = size(px(DELTA), px(DELTA));
        let visible = viewport(point(px(-DELTA), px(-DELTA)), small, window());
        assert_eq!(visible.origin, Point::default());
    }

    #[test]
    fn only_nodes_in_the_viewport_are_built() {
        let column = GRAPH_NODE_WIDTH + GRAPH_COLUMN_GAP;
        let scrolled = point(-column * position(PASSES), px(0.));
        let narrow = size(column, GRAPH_NODE_HEIGHT);
        let visible = viewport(scrolled, content(), narrow);
        let at = |layer: usize| Bounds::new(slot_origin(layer, 0), narrow);
        assert!(!in_view(at(0), Point::default(), visible));
        assert!(in_view(at(PASSES), Point::default(), visible));
        assert!(!in_view(at(PASSES + 2), Point::default(), visible));
    }

    #[test]
    fn an_edge_extent_covers_its_stroke() {
        let route = long_route();
        let (path, _) = tessellated(EdgeKind::DependsOn);
        let bounds = extent(&route, GRAPH_EDGE);
        assert!(bounds.contains(&path.bounds.origin));
        assert!(bounds.contains(&path.bounds.bottom_right()));
    }
}
