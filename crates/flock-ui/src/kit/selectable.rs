use std::cell::{Cell, RefCell};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AnyElement, App, BorderStyle, Bounds, ClickEvent, Corners, CursorStyle, Edges, Element,
    ElementId, Global, GlobalElementId, HighlightStyle, Hitbox, HitboxBehavior, Hsla,
    InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseUpEvent,
    PaintQuad, ParentElement, Pixels, Point, RenderOnce, SharedString, Styled, StyledText,
    Subscription, TextLayout, Window, WindowId, div, px, transparent_black,
};
use gpui_base::{TextSelection, TextSelectionHandle, TextSelectionRegistration, TextSelectionRun};

use crate::theme::{ActiveTheme, FONT_MONO};

gpui::actions!(flock, [SelectAllText]);

const DEFAULT_ID: &str = "selectable";
const LINE_BREAK: &str = "\n";
const WORD_BREAK: char = ' ';
const NO_SEPARATOR: &str = "";
const DRAG_SLOP: Pixels = px(3.);

type LinkHandler = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Default)]
struct Shared {
    text: RefCell<String>,
    selected: RefCell<String>,
    all: Cell<bool>,
    pressed_link: Cell<Option<usize>>,
}

impl Shared {
    fn copy_text(&self) -> String {
        if self.all.get() {
            self.text.borrow().clone()
        } else {
            self.selected.borrow().clone()
        }
    }
}

#[derive(Clone)]
struct Retained {
    handle: TextSelectionHandle,
    shared: Rc<Shared>,
    _refresh: Rc<Subscription>,
}

#[derive(Default)]
struct DocumentOrder {
    next: u64,
    reset_pending: bool,
}

impl Global for DocumentOrder {}

#[derive(Default)]
struct Focused(Option<(WindowId, Retained)>);

impl Global for Focused {}

pub(crate) fn init(cx: &mut App) {
    cx.on_action(select_all);
}

fn select_all(_: &SelectAllText, cx: &mut App) {
    let Some((window, retained)) = cx.try_global::<Focused>().and_then(|f| f.0.clone()) else {
        return;
    };
    TextSelection::clear_for_window(window, cx);
    retained.handle.set_local_selection(true, cx);
    retained.shared.all.set(true);
    cx.refresh_windows();
}

fn next_order(window: &Window, cx: &mut App) -> u64 {
    let order = cx.default_global::<DocumentOrder>();
    order.next = order.next.wrapping_add(1);
    let next = order.next;
    if !order.reset_pending {
        order.reset_pending = true;
        window.defer(cx, |_, cx| {
            let order = cx.default_global::<DocumentOrder>();
            order.next = 0;
            order.reset_pending = false;
        });
    }
    next
}

pub fn moved(from: Point<Pixels>, to: Point<Pixels>) -> bool {
    let delta = to - from;
    delta.x.abs() > DRAG_SLOP || delta.y.abs() > DRAG_SLOP
}

pub fn is_text_drag(event: &ClickEvent, window: &mut Window, cx: &mut App) -> bool {
    let ClickEvent::Mouse(click) = event else {
        return false;
    };
    moved(click.down.position, click.up.position) && TextSelection::has_selection(window, cx)
}

pub fn joined(texts: &[SharedString], ranges: &[Option<Range<usize>>], separator: &str) -> String {
    let first = ranges.iter().position(Option::is_some);
    let last = ranges.iter().rposition(Option::is_some);
    let (Some(first), Some(last)) = (first, last) else {
        return String::new();
    };
    (first..=last)
        .map(|ix| {
            let text = texts.get(ix).map_or("", SharedString::as_ref);
            ranges
                .get(ix)
                .cloned()
                .flatten()
                .and_then(|range| text.get(range))
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(separator)
}

pub fn quads(
    start: Point<Pixels>,
    end: Point<Pixels>,
    bounds: Bounds<Pixels>,
    line_height: Pixels,
) -> Vec<Bounds<Pixels>> {
    if start.y == end.y {
        return vec![Bounds::from_corners(
            start,
            Point::new(end.x, end.y + line_height),
        )];
    }
    let mut quads = vec![Bounds::from_corners(
        start,
        Point::new(bounds.right(), start.y + line_height),
    )];
    if end.y > start.y + line_height {
        quads.push(Bounds::from_corners(
            Point::new(bounds.left(), start.y + line_height),
            Point::new(bounds.right(), end.y),
        ));
    }
    quads.push(Bounds::from_corners(
        Point::new(bounds.left(), end.y),
        Point::new(end.x, end.y + line_height),
    ));
    quads
}

fn range_quads(layout: &TextLayout, range: Range<usize>) -> Vec<Bounds<Pixels>> {
    match (
        layout.position_for_index(range.start),
        layout.position_for_index(range.end),
    ) {
        (Some(start), Some(end)) => quads(start, end, layout.bounds(), layout.line_height()),
        _ => Vec::new(),
    }
}

fn row_bounds(text: Bounds<Pixels>, block: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds::from_corners(
        Point::new(block.left(), text.top()),
        Point::new(block.right(), text.bottom()),
    )
}

fn paint_highlight(bounds: Bounds<Pixels>, color: Hsla, window: &mut Window) {
    window.paint_quad(PaintQuad {
        bounds,
        background: color.into(),
        corner_radii: Corners::default(),
        border_widths: Edges::default(),
        border_color: transparent_black(),
        border_style: BorderStyle::default(),
    });
}

fn default_id(text: &str) -> ElementId {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    ElementId::NamedInteger(DEFAULT_ID.into(), hasher.finish())
}

pub struct Selectable {
    id: ElementId,
    child: AnyElement,
    runs: Vec<(SharedString, TextLayout)>,
    links: Vec<(Range<usize>, LinkHandler)>,
    rows: bool,
    on_selected: bool,
    separator: &'static str,
}

impl Selectable {
    pub fn new(id: impl Into<ElementId>, child: impl IntoElement) -> Self {
        Self {
            id: id.into(),
            child: child.into_any_element(),
            runs: Vec::new(),
            links: Vec::new(),
            rows: false,
            on_selected: false,
            separator: LINE_BREAK,
        }
    }

    pub fn separator(mut self, separator: &'static str) -> Self {
        self.separator = separator;
        self
    }

    pub fn run(mut self, text: impl Into<SharedString>, layout: TextLayout) -> Self {
        self.runs.push((text.into(), layout));
        self
    }

    pub fn whole_rows(mut self) -> Self {
        self.rows = true;
        self
    }

    pub fn on_selected(mut self, on_selected: bool) -> Self {
        self.on_selected = on_selected;
        self
    }

    fn link(mut self, range: Range<usize>, handler: LinkHandler) -> Self {
        self.links.push((range, handler));
        self
    }

    fn retained(global_id: &GlobalElementId, window: &mut Window, cx: &mut App) -> Retained {
        window.with_element_state(global_id, |retained: Option<Retained>, window| {
            let retained = retained.unwrap_or_else(|| {
                let handle = TextSelectionHandle::new("", cx);
                let shared = Rc::new(Shared::default());
                let copying = shared.clone();
                handle.copy_with(move |_| copying.copy_text(), cx);
                let clearing = shared.clone();
                handle.clear_with(move |_| clearing.all.set(false), cx);
                let refresh = handle.refresh_window_on_change(window, cx);
                Retained {
                    handle,
                    shared,
                    _refresh: Rc::new(refresh),
                }
            });
            (retained.clone(), retained)
        })
    }
}

pub struct SelectablePrepaint {
    retained: Option<Retained>,
    hitbox: Hitbox,
    links: Vec<(usize, Hitbox)>,
}

impl IntoElement for Selectable {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Selectable {
    type RequestLayoutState = ();
    type PrepaintState = SelectablePrepaint;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (): &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.child.prepaint(window, cx);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let retained = global_id.map(|id| Self::retained(id, window, cx));
        if let Some(retained) = &retained {
            let text_bounds = self
                .runs
                .iter()
                .map(|(_, layout)| {
                    let text = layout.bounds();
                    if self.rows {
                        row_bounds(text, bounds)
                    } else {
                        text
                    }
                })
                .collect();
            let order = next_order(window, cx);
            let registration = TextSelectionRegistration::new(hitbox.clone(), bounds)
                .with_document_order(order)
                .with_text_bounds(text_bounds)
                .with_rendered_element(&retained.handle, window, cx);
            retained.handle.register(registration, window, cx);
        }
        let links = match self.runs.first() {
            Some((_, layout)) => self
                .links
                .iter()
                .enumerate()
                .flat_map(|(ix, (range, _))| {
                    range_quads(layout, range.clone())
                        .into_iter()
                        .map(move |quad| (ix, quad))
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|(ix, quad)| (ix, window.insert_hitbox(quad, HitboxBehavior::Normal)))
                .collect(),
            None => Vec::new(),
        };
        SelectablePrepaint {
            retained,
            hitbox,
            links,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        (): &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(retained) = prepaint.retained.clone() {
            self.paint_selection(&retained, window, cx);
            self.listen(&retained, prepaint, window);
        }
        window.set_cursor_style(CursorStyle::IBeam, &prepaint.hitbox);
        for (_, hitbox) in &prepaint.links {
            window.set_cursor_style(CursorStyle::PointingHand, hitbox);
        }
        self.child.paint(window, cx);
    }
}

impl Selectable {
    fn paint_selection(&self, retained: &Retained, window: &mut Window, cx: &mut App) {
        let runs: Vec<TextSelectionRun> = self
            .runs
            .iter()
            .zip(0_u64..)
            .map(|((text, layout), order)| {
                TextSelectionRun::new(text.clone(), layout.clone(), layout.bounds())
                    .with_document_order(order)
            })
            .collect();
        let projection = retained.handle.update_runs(&runs, cx);
        let texts: Vec<SharedString> = self.runs.iter().map(|(text, _)| text.clone()).collect();
        let all = retained.shared.all.get();
        let ranges: Vec<Option<Range<usize>>> = if all {
            texts.iter().map(|text| Some(0..text.len())).collect()
        } else {
            projection.ranges().to_vec()
        };
        *retained.shared.text.borrow_mut() = texts
            .iter()
            .map(SharedString::as_ref)
            .collect::<Vec<_>>()
            .join(self.separator);
        *retained.shared.selected.borrow_mut() = joined(&texts, &ranges, self.separator);
        let palette = cx.palette();
        let color = if self.on_selected {
            palette.textsel_on_sel
        } else {
            palette.sel
        };
        for ((_, layout), range) in self.runs.iter().zip(ranges) {
            let Some(range) = range.filter(|range| !range.is_empty()) else {
                continue;
            };
            for quad in range_quads(layout, range) {
                paint_highlight(quad, color, window);
            }
        }
    }

    fn listen(&self, retained: &Retained, prepaint: &SelectablePrepaint, window: &mut Window) {
        let hitbox = prepaint.hitbox.clone();
        let focus = retained.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if !phase.capture() || event.button != MouseButton::Left {
                return;
            }
            let window_id = window.window_handle().window_id();
            let focused = cx.default_global::<Focused>();
            if hitbox.is_hovered(window) {
                focused.0 = Some((window_id, focus.clone()));
            } else if focused
                .0
                .as_ref()
                .is_some_and(|(_, current)| current.handle.entity_id() == focus.handle.entity_id())
            {
                focused.0 = None;
            }
        });
        for (ix, hitbox) in prepaint.links.iter().cloned() {
            let Some(handler) = self.links.get(ix).map(|(_, handler)| handler.clone()) else {
                continue;
            };
            let pressing = retained.shared.clone();
            let down = hitbox.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, _| {
                if phase.bubble() && event.button == MouseButton::Left && down.is_hovered(window) {
                    pressing.pressed_link.set(Some(ix));
                }
            });
            let releasing = retained.shared.clone();
            window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                if !phase.bubble() || event.button != MouseButton::Left {
                    return;
                }
                let pressed = releasing.pressed_link.take() == Some(ix);
                if pressed
                    && hitbox.is_hovered(window)
                    && TextSelection::selected_text(window, cx).is_empty()
                {
                    handler(window, cx);
                }
            });
        }
    }
}

#[derive(IntoElement)]
pub struct SelectableText {
    id: Option<ElementId>,
    text: SharedString,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    mono: Vec<Range<usize>>,
    links: Vec<(Range<usize>, LinkHandler)>,
    on_selected: bool,
}

impl SelectableText {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            id: None,
            text: text.into(),
            highlights: Vec::new(),
            mono: Vec::new(),
            links: Vec::new(),
            on_selected: false,
        }
    }

    pub fn on_selected(mut self, on_selected: bool) -> Self {
        self.on_selected = on_selected;
        self
    }

    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn highlights(
        mut self,
        highlights: impl IntoIterator<Item = (Range<usize>, HighlightStyle)>,
    ) -> Self {
        self.highlights.extend(highlights);
        self
    }

    pub fn mono(mut self, range: Range<usize>) -> Self {
        self.mono.push(range);
        self
    }

    pub fn link(
        mut self,
        range: Range<usize>,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.links.push((range, Rc::new(handler)));
        self
    }
}

impl RenderOnce for SelectableText {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let id = self.id.unwrap_or_else(|| default_id(&self.text));
        let fonts = self
            .mono
            .into_iter()
            .map(|range| (range, SharedString::from(FONT_MONO)));
        let styled = StyledText::new(self.text.clone())
            .with_highlights(self.highlights)
            .with_font_family_overrides(fonts);
        let layout = styled.layout().clone();
        let selectable = Selectable::new(id, styled)
            .run(self.text, layout)
            .on_selected(self.on_selected);
        self.links
            .into_iter()
            .fold(selectable, |selectable, (range, handler)| {
                selectable.link(range, handler)
            })
    }
}

pub fn words(text: &str) -> Option<Vec<&str>> {
    (!text.contains(LINE_BREAK)).then(|| text.split_inclusive(WORD_BREAK).collect())
}

#[derive(IntoElement)]
pub struct SelectableWords {
    text: SharedString,
}

impl SelectableWords {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

impl RenderOnce for SelectableWords {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let Some(pieces) = words(&self.text) else {
            return SelectableText::new(self.text).into_any_element();
        };
        let pieces: Vec<(SharedString, StyledText)> = pieces
            .into_iter()
            .map(|piece| {
                let text = SharedString::from(piece.to_owned());
                (text.clone(), StyledText::new(text))
            })
            .collect();
        let runs: Vec<(SharedString, TextLayout)> = pieces
            .iter()
            .map(|(text, styled)| (text.clone(), styled.layout().clone()))
            .collect();
        let styled = pieces.into_iter().map(|(_, styled)| styled);
        let block = div()
            .flex()
            .flex_wrap()
            .min_w_0()
            .children(styled.map(|text| div().max_w_full().child(text)));
        runs.into_iter()
            .fold(
                Selectable::new(default_id(&self.text), block).separator(NO_SEPARATOR),
                |selectable, (text, layout)| selectable.run(text, layout),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use gpui::{point, size};

    use super::*;

    fn texts(lines: &[&str]) -> Vec<SharedString> {
        lines.iter().map(|line| SharedString::from(*line)).collect()
    }

    #[test]
    fn copied_lines_keep_blank_lines_between_the_ends() {
        let lines = texts(&["spec:", "", "  replicas: 3", "  paused: false"]);
        let ranges = [Some(2..5), None, Some(0..13), None];
        assert_eq!(joined(&lines, &ranges, LINE_BREAK), "ec:\n\n  replicas: 3");
    }

    #[test]
    fn nothing_selected_copies_nothing() {
        let lines = texts(&["a", "b"]);
        assert_eq!(joined(&lines, &[None, None], LINE_BREAK), "");
        assert_eq!(joined(&lines, &[None, Some(0..1)], LINE_BREAK), "b");
    }

    #[test]
    fn ranges_off_character_boundaries_copy_nothing_for_that_line() {
        let lines = texts(&["→ x", "y"]);
        assert_eq!(joined(&lines, &[Some(1..3), Some(0..1)], LINE_BREAK), "\ny");
    }

    #[test]
    fn only_a_press_that_travels_counts_as_a_drag() {
        let down = point(px(100.), px(40.));
        assert!(!moved(down, down));
        assert!(!moved(down, down + point(px(2.), px(-2.))));
        assert!(moved(down, down + point(px(40.), px(0.))));
        assert!(moved(down, down + point(px(0.), px(-8.))));
    }

    #[test]
    fn one_line_selection_is_one_quad() {
        let bounds = Bounds::new(point(px(10.), px(20.)), size(px(100.), px(60.)));
        let quads = quads(
            point(px(20.), px(20.)),
            point(px(50.), px(20.)),
            bounds,
            px(20.),
        );
        assert_eq!(
            quads,
            vec![Bounds::from_corners(
                point(px(20.), px(20.)),
                point(px(50.), px(40.))
            )]
        );
    }

    #[test]
    fn wrapped_selection_fills_the_middle_rows() {
        let bounds = Bounds::new(point(px(10.), px(20.)), size(px(100.), px(100.)));
        let quads = quads(
            point(px(40.), px(20.)),
            point(px(30.), px(80.)),
            bounds,
            px(20.),
        );
        assert_eq!(
            quads,
            vec![
                Bounds::from_corners(point(px(40.), px(20.)), point(px(110.), px(40.))),
                Bounds::from_corners(point(px(10.), px(40.)), point(px(110.), px(80.))),
                Bounds::from_corners(point(px(10.), px(80.)), point(px(30.), px(100.))),
            ]
        );
    }

    #[test]
    fn words_keep_their_spaces_so_joining_them_gives_the_text_back() {
        let text = "couldn't find key  webhook_secret in Secret payments/stripe";
        let pieces = words(text).unwrap_or_default();
        assert_eq!(pieces.concat(), text);
        assert_eq!(pieces.last(), Some(&"payments/stripe"));
        assert!(pieces.contains(&" "));
        assert_eq!(words("first\nsecond"), None);
    }

    #[test]
    fn default_ids_follow_the_text() {
        assert_eq!(default_id("checkout-api"), default_id("checkout-api"));
        assert_ne!(default_id("checkout-api"), default_id("checkout-web"));
    }
}
