use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;

use flock_core::Status;
use flock_core::filter::highlights;
use flock_core::units::Amount;
use gpui::{
    Animation, AnimationExt, AnyElement, App, AppContext as _, ClickEvent, Context, CursorStyle,
    Div, ElementId, Entity, EventEmitter, FocusHandle, Focusable, HighlightStyle, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, ListSizingBehavior, MouseButton, MouseDownEvent,
    MouseMoveEvent, ParentElement, Pixels, Point, Render, ScrollStrategy, SharedString,
    StatefulInteractiveElement, Styled, Task, TextRun, UniformListScrollHandle, Window, div,
    ease_in_out, prelude::FluentBuilder, uniform_list,
};

use super::button::IconButton;
use super::list_keys::{Nav, TypeAhead, find, typed};
use super::menu::{ActionMenu, MenuItem};
use super::selectable::{SelectableText, is_text_drag};
use super::status::StatusGlyph;
use super::text::{AmountText, Highlighted};
use super::tooltip::Tooltip;
use crate::prefs::{self, ColumnWidths, FLOCK_SCOPE};
use crate::theme::{
    ActiveTheme, COLUMN_DIVIDER, COLUMN_HANDLE, COLUMN_MIN, FLASH, FONT_MONO, FONT_SANS,
    GLYPH_SLOT, Palette, ROW_TABLE, ROW_TABLE_MIN, RULE, SPACE_4, SPACE_6, SPACE_8, TEXT_AXIS,
    TEXT_BODY, TEXT_MONO, TEXT_SMALL,
};

const SORT_ASCENDING: &str = "↑";
const SORT_DESCENDING: &str = "↓";
const MORE: &str = "⋯";
const MORE_TOOLTIP: &str = "Actions";
const FLASH_STRENGTH: f32 = 0.35;
const TRAILING: Pixels = SPACE_8;
const DOUBLE_CLICK: usize = 2;
const COLUMN_HANDLE_GROUP: &str = "table-column-handle";
pub const TABLE_CONTEXT: &str = "FlockTable";

pub type CellRenderer<R> = Rc<dyn Fn(&R, &Palette) -> AnyElement>;
pub type Comparator<R> = Rc<dyn Fn(&R, &R) -> Ordering>;
pub type RowActions<R> = Rc<dyn Fn(&R, &[&R]) -> Vec<MenuItem>>;
type RowStatus<R> = Rc<dyn Fn(&R) -> Option<Status>>;
type RowKey<R> = Rc<dyn Fn(&R) -> SharedString>;
type SpanRow<R> = Rc<dyn Fn(&R, &Palette) -> Option<AnyElement>>;
type TextFn<R> = Rc<dyn Fn(&R) -> SharedString>;
type ColorFn<R> = Rc<dyn Fn(&R, &Palette) -> Hsla>;
type GlyphFn<R> = Rc<dyn Fn(&R) -> Status>;

struct TextCell<R> {
    text: TextFn<R>,
    color: Option<ColorFn<R>>,
    glyph: Option<GlyphFn<R>>,
}

impl<R> TextCell<R> {
    fn render(
        &self,
        row: &R,
        palette: &Palette,
        words: &[String],
        selectable: bool,
        selected: bool,
    ) -> AnyElement {
        let text = (self.text)(row);
        let ranges = highlights(words, &text);
        let content = if selectable {
            let style = HighlightStyle {
                color: Some(palette.acc),
                ..HighlightStyle::default()
            };
            SelectableText::new(text)
                .on_selected(selected)
                .highlights(ranges.into_iter().map(|range| (range, style)))
                .into_any_element()
        } else if ranges.is_empty() {
            text.into_any_element()
        } else {
            Highlighted::new(text, ranges).into_any_element()
        };
        let label = div()
            .truncate()
            .when_some(self.color.as_ref(), |label, color| {
                label.text_color(color(row, palette))
            })
            .child(content);
        match &self.glyph {
            Some(glyph) => div()
                .flex()
                .items_center()
                .gap(SPACE_4)
                .min_w_0()
                .child(StatusGlyph::new(glyph(row)))
                .child(label)
                .into_any_element(),
            None => label.into_any_element(),
        }
    }
}

pub type HighlightedCellRenderer<R> = Rc<dyn Fn(&R, &Palette, &[String]) -> AnyElement>;

enum Cell<R> {
    Custom(CellRenderer<R>),
    Highlighted(HighlightedCellRenderer<R>),
    Text(TextCell<R>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Width {
    Fixed(Pixels),
    Flex { weight: f32, min: Pixels },
    Auto { min: Pixels, max: Pixels },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

pub struct Column<R> {
    title: SharedString,
    width: Width,
    align: Align,
    mono: bool,
    sort: Option<Comparator<R>>,
    cell: Cell<R>,
    fit: Option<TextFn<R>>,
    fit_extra: Pixels,
    highlight: bool,
    selectable: bool,
    tooltip: Option<TextFn<R>>,
}

impl<R: 'static> Column<R> {
    fn build(title: impl Into<SharedString>, cell: Cell<R>) -> Self {
        Self {
            title: title.into(),
            width: Width::Flex {
                weight: 1.,
                min: Pixels::ZERO,
            },
            align: Align::Start,
            mono: false,
            sort: None,
            cell,
            fit: None,
            fit_extra: Pixels::ZERO,
            highlight: false,
            selectable: false,
            tooltip: None,
        }
    }

    pub fn new(
        title: impl Into<SharedString>,
        cell: impl Fn(&R, &Palette) -> AnyElement + 'static,
    ) -> Self {
        Self::build(title, Cell::Custom(Rc::new(cell)))
    }

    pub fn highlighted(
        title: impl Into<SharedString>,
        cell: impl Fn(&R, &Palette, &[String]) -> AnyElement + 'static,
    ) -> Self {
        Self::build(title, Cell::Highlighted(Rc::new(cell)))
    }

    fn text_cell(
        title: impl Into<SharedString>,
        text: TextFn<R>,
        color: Option<ColorFn<R>>,
        glyph: Option<GlyphFn<R>>,
    ) -> Self {
        let fit_extra = if glyph.is_some() {
            GLYPH_SLOT + SPACE_4
        } else {
            Pixels::ZERO
        };
        let cell = Cell::Text(TextCell {
            text: text.clone(),
            color,
            glyph,
        });
        Self {
            fit: Some(text),
            fit_extra,
            ..Self::build(title, cell)
        }
    }

    pub fn amount(title: impl Into<SharedString>, amount: impl Fn(&R) -> Amount + 'static) -> Self {
        Self::new(title, move |row, palette| {
            AmountText::new(amount(row))
                .color(palette.fg2)
                .into_any_element()
        })
    }

    pub fn text(
        title: impl Into<SharedString>,
        text: impl Fn(&R) -> SharedString + 'static,
    ) -> Self {
        Self::text_cell(title, Rc::new(text), None, None)
    }

    pub fn colored_text(
        title: impl Into<SharedString>,
        text: impl Fn(&R) -> SharedString + 'static,
        color: impl Fn(&R, &Palette) -> Hsla + 'static,
    ) -> Self {
        Self::text_cell(title, Rc::new(text), Some(Rc::new(color)), None)
    }

    pub fn status_text(
        title: impl Into<SharedString>,
        status: impl Fn(&R) -> Status + 'static,
        text: impl Fn(&R) -> SharedString + 'static,
    ) -> Self {
        Self::text_cell(title, Rc::new(text), None, Some(Rc::new(status)))
    }

    pub fn highlight(mut self) -> Self {
        self.highlight = true;
        self
    }

    pub fn selectable(mut self) -> Self {
        self.selectable = true;
        self
    }

    pub fn fit_by(mut self, text: impl Fn(&R) -> SharedString + 'static, extra: Pixels) -> Self {
        self.fit = Some(Rc::new(text));
        self.fit_extra = extra;
        self
    }

    pub fn fit_status_by(self, text: impl Fn(&R) -> SharedString + 'static) -> Self {
        self.fit_by(text, GLYPH_SLOT + SPACE_4)
    }

    pub fn tooltip(mut self, text: impl Fn(&R) -> SharedString + 'static) -> Self {
        self.tooltip = Some(Rc::new(text));
        self
    }

    fn render_cell(
        &self,
        row: &R,
        palette: &Palette,
        words: &[String],
        selected: bool,
    ) -> AnyElement {
        let content = match &self.cell {
            Cell::Custom(cell) => cell(row, palette),
            Cell::Highlighted(cell) => cell(row, palette, words),
            Cell::Text(cell) if self.highlight => {
                cell.render(row, palette, words, self.selectable, selected)
            }
            Cell::Text(cell) => cell.render(row, palette, &[], self.selectable, selected),
        };
        let tooltip = self.tooltip.as_ref().map(|text| text(row));
        match tooltip.filter(|tooltip| !tooltip.is_empty()) {
            Some(tooltip) => justify(div().flex().items_center(), self.align)
                .id(self.title.clone())
                .w_full()
                .min_w_0()
                .tooltip(Tooltip::text(tooltip))
                .child(content)
                .into_any_element(),
            None => content,
        }
    }

    fn min(&self) -> Pixels {
        match self.width {
            Width::Fixed(_) => COLUMN_MIN,
            Width::Flex { min, .. } => min.max(COLUMN_MIN),
            Width::Auto { min, .. } => min.max(COLUMN_MIN),
        }
    }

    pub fn fixed(mut self, width: Pixels) -> Self {
        self.width = Width::Fixed(width);
        self
    }

    pub fn flex(mut self, weight: f32, min: Pixels) -> Self {
        self.width = Width::Flex { weight, min };
        self
    }

    pub fn auto(mut self, min: Pixels, max: Pixels) -> Self {
        self.width = Width::Auto { min, max };
        self
    }

    pub fn end(mut self) -> Self {
        self.align = Align::End;
        self
    }

    pub fn center(mut self) -> Self {
        self.align = Align::Center;
        self
    }

    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    pub fn sortable(mut self, compare: impl Fn(&R, &R) -> Ordering + 'static) -> Self {
        self.sort = Some(Rc::new(compare));
        self
    }

    pub fn sort_by_key<K: Ord>(self, key: impl Fn(&R) -> K + 'static) -> Self {
        self.sortable(move |a, b| key(a).cmp(&key(b)))
    }
}

fn cell_frame(width: Width, align: Align) -> Div {
    justify(sized(cell_body().h_full(), width), align)
}

fn cell_body() -> Div {
    div()
        .flex()
        .items_center()
        .overflow_hidden()
        .whitespace_nowrap()
}

fn sized(cell: Div, width: Width) -> Div {
    match width {
        Width::Fixed(width) => cell.w(width).flex_none(),
        Width::Flex { weight, min } => cell.flex_grow(weight).flex_basis(Pixels::ZERO).min_w(min),
        Width::Auto { min, .. } => cell.w(min).flex_none(),
    }
}

fn justify(cell: Div, align: Align) -> Div {
    match align {
        Align::Start => cell,
        Align::Center => cell.justify_center(),
        Align::End => cell.justify_end(),
    }
}

pub fn min_row_width(widths: impl Iterator<Item = Width>, gap: Pixels, padding: Pixels) -> Pixels {
    let mut total = padding;
    for (ix, width) in widths.enumerate() {
        if ix > 0 {
            total += gap;
        }
        total += match width {
            Width::Fixed(width) => width,
            Width::Flex { min, .. } | Width::Auto { min, .. } => min,
        };
    }
    total
}

pub fn auto_width(content: Pixels, min: Pixels, max: Pixels) -> Pixels {
    content.ceil().max(min).min(max.max(min))
}

pub fn resized(start: Pixels, delta: Pixels, min: Pixels) -> Pixels {
    (start + delta).max(min)
}

fn text_width(window: &Window, text: &str, family: &'static str, size: Pixels) -> Pixels {
    if text.is_empty() {
        return Pixels::ZERO;
    }
    let run = TextRun {
        len: text.len(),
        font: gpui::font(family),
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .layout_line(text, size, &[run], None)
        .width
}

pub fn sort_order<R>(
    rows: &[R],
    compare: &dyn Fn(&R, &R) -> Ordering,
    direction: SortDirection,
) -> Vec<usize> {
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by(|&a, &b| {
        let ordering = compare(&rows[a], &rows[b]);
        match direction {
            SortDirection::Ascending => ordering,
            SortDirection::Descending => ordering.reverse(),
        }
    });
    order
}

pub fn next_sort(
    current: Option<(usize, SortDirection)>,
    column: usize,
) -> Option<(usize, SortDirection)> {
    match current {
        Some((col, SortDirection::Ascending)) if col == column => {
            Some((column, SortDirection::Descending))
        }
        Some((col, SortDirection::Descending)) if col == column => None,
        Some(_) => Some((column, SortDirection::Ascending)),
        None => Some((column, SortDirection::Ascending)),
    }
}

pub fn changed_statuses(
    before: &HashMap<SharedString, Status>,
    after: &[(SharedString, Status)],
) -> Vec<SharedString> {
    after
        .iter()
        .filter(|(key, status)| before.get(key).is_some_and(|old| old != status))
        .map(|(key, _)| key.clone())
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableEvent {
    Selected(usize),
    Opened(usize),
    Marked,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Drag {
    column: usize,
    origin: Pixels,
    start: Pixels,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Header {
    Hidden,
    Sans,
    Mono,
}

pub struct Table<R: 'static> {
    id: SharedString,
    columns: Rc<Vec<Column<R>>>,
    rows: Rc<Vec<R>>,
    order: Vec<usize>,
    sort: Option<(usize, SortDirection)>,
    selected: Option<usize>,
    marked: Option<Vec<usize>>,
    row_height: Pixels,
    column_gap: Pixels,
    inset: Pixels,
    span: Option<SpanRow<R>>,
    click_opens: bool,
    fit: bool,
    rules: bool,
    header: Header,
    status: Option<RowStatus<R>>,
    key: Option<RowKey<R>>,
    name: Option<RowKey<R>>,
    flashes: HashMap<SharedString, u64>,
    flash_generation: u64,
    flash_timer: Option<Task<()>>,
    empty: SharedString,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    scope: SharedString,
    widths: HashMap<SharedString, Pixels>,
    auto: Option<HashMap<SharedString, Pixels>>,
    measured: Rc<RefCell<Vec<Pixels>>>,
    drag: Option<Drag>,
    words: Vec<String>,
    hovered: Option<usize>,
    actions: Option<RowActions<R>>,
    menu: Entity<ActionMenu>,
    type_ahead: TypeAhead,
}

impl<R: 'static> EventEmitter<TableEvent> for Table<R> {}

impl<R: 'static> Table<R> {
    pub fn new(
        id: impl Into<SharedString>,
        columns: Vec<Column<R>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let id: SharedString = id.into();
        let menu = cx.new(|cx| ActionMenu::context(format!("{id}-row-menu"), cx));
        let widths = stored_widths(FLOCK_SCOPE, &id, cx);
        Self {
            id,
            columns: Rc::new(columns),
            rows: Rc::new(Vec::new()),
            order: Vec::new(),
            sort: None,
            selected: None,
            marked: None,
            row_height: ROW_TABLE,
            column_gap: SPACE_6,
            inset: SPACE_8,
            span: None,
            click_opens: false,
            fit: false,
            rules: true,
            header: Header::Sans,
            status: None,
            key: None,
            name: None,
            flashes: HashMap::new(),
            flash_generation: 0,
            flash_timer: None,
            empty: SharedString::default(),
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle().tab_stop(true),
            type_ahead: TypeAhead::default(),
            scope: FLOCK_SCOPE.into(),
            widths,
            auto: None,
            measured: Rc::default(),
            drag: None,
            words: Vec::new(),
            hovered: None,
            actions: None,
            menu,
        }
    }

    pub fn row_actions(mut self, actions: impl Fn(&R, &[&R]) -> Vec<MenuItem> + 'static) -> Self {
        self.actions = Some(Rc::new(actions));
        self
    }

    pub fn set_row_actions(
        &mut self,
        actions: impl Fn(&R, &[&R]) -> Vec<MenuItem> + 'static,
        cx: &mut Context<Self>,
    ) {
        self.actions = Some(Rc::new(actions));
        cx.notify();
    }

    pub fn set_scope(&mut self, scope: impl Into<SharedString>, cx: &mut Context<Self>) {
        let scope = scope.into();
        if scope == self.scope {
            return;
        }
        self.widths = stored_widths(&scope, &self.id, cx);
        self.scope = scope;
        cx.notify();
    }

    pub fn set_highlight(&mut self, words: Vec<String>, cx: &mut Context<Self>) {
        if self.words != words {
            self.words = words;
            cx.notify();
        }
    }

    pub fn hovered_row(&self) -> Option<&R> {
        self.hovered.and_then(|ix| self.rows.get(ix))
    }

    pub fn column_width(&self, title: &str) -> Option<Pixels> {
        self.widths.get(title).copied()
    }

    fn width_of(&self, column: &Column<R>) -> Width {
        if let Some(width) = self.widths.get(&column.title) {
            return Width::Fixed(*width);
        }
        match column.width {
            Width::Auto { min, .. } => {
                let resolved = self.auto.as_ref().and_then(|auto| auto.get(&column.title));
                Width::Fixed(resolved.copied().unwrap_or(min))
            }
            width @ (Width::Fixed(_) | Width::Flex { .. }) => width,
        }
    }

    fn resolve_auto(&mut self, window: &Window) {
        if self.auto.is_some() {
            return;
        }
        let mut resolved = HashMap::new();
        for column in self.columns.iter() {
            let Width::Auto { min, max } = column.width else {
                continue;
            };
            let content = self.content_width(column, window).unwrap_or(Pixels::ZERO);
            resolved.insert(column.title.clone(), auto_width(content, min, max));
        }
        self.auto = Some(resolved);
    }

    fn content_width(&self, column: &Column<R>, window: &Window) -> Option<Pixels> {
        let text = column.fit.as_ref()?;
        let (family, size) = if column.mono {
            (FONT_MONO, TEXT_MONO)
        } else {
            (FONT_SANS, TEXT_BODY)
        };
        let widest = self
            .rows
            .iter()
            .map(|row| text_width(window, &text(row), family, size))
            .fold(Pixels::ZERO, Pixels::max);
        let (header_family, header_size) = match self.header {
            Header::Mono => (FONT_MONO, TEXT_AXIS),
            Header::Sans => (FONT_SANS, TEXT_SMALL),
            Header::Hidden => (FONT_SANS, TEXT_SMALL),
        };
        let heading = text_width(window, &column.title, header_family, header_size)
            + text_width(window, SORT_ASCENDING, header_family, header_size);
        Some((widest + column.fit_extra).max(heading))
    }

    fn start_resize(
        &mut self,
        column: usize,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        if event.click_count >= DOUBLE_CLICK {
            self.drag = None;
            self.fit_column(column, window, cx);
            return;
        }
        let measured = self.measured.borrow().get(column).copied();
        let Some(start) = measured.or_else(|| {
            self.columns.get(column).map(|c| match self.width_of(c) {
                Width::Fixed(width) => width,
                Width::Flex { min, .. } | Width::Auto { min, .. } => min,
            })
        }) else {
            return;
        };
        self.drag = Some(Drag {
            column,
            origin: event.position.x,
            start,
        });
    }

    fn drag_to(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(drag) = self.drag else {
            return;
        };
        if !event.dragging() {
            self.finish_resize(cx);
            return;
        }
        let Some(column) = self.columns.get(drag.column) else {
            return;
        };
        let width = resized(drag.start, event.position.x - drag.origin, column.min());
        let title = column.title.clone();
        if self.widths.get(&title) != Some(&width) {
            self.widths.insert(title, width);
            cx.notify();
        }
    }

    fn finish_resize(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            self.save_widths(cx);
        }
    }

    fn fit_column(&mut self, ix: usize, window: &Window, cx: &mut Context<Self>) {
        let Some(column) = self.columns.get(ix) else {
            return;
        };
        let title = column.title.clone();
        match self.content_width(column, window) {
            Some(content) => {
                let width = content.max(column.min());
                self.widths.insert(title, width.ceil());
            }
            None => {
                self.widths.remove(&title);
            }
        }
        self.save_widths(cx);
        cx.notify();
    }

    fn save_widths(&self, cx: &mut App) {
        let widths: ColumnWidths = self
            .widths
            .iter()
            .map(|(title, width)| (title.to_string(), f32::from(*width)))
            .collect();
        prefs::set_column_widths(&self.scope, &self.id, widths, cx);
    }

    fn open_menu(
        &mut self,
        ix: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = match (self.rows.get(ix), &self.actions) {
            (Some(row), Some(actions)) => actions(row, &self.marked_rows()),
            _ => Vec::new(),
        };
        if items.is_empty() {
            return;
        }
        self.menu
            .update(cx, |menu, cx| menu.show_at(position, items, window, cx));
    }

    fn hover_row(&mut self, ix: usize, hovered: bool, cx: &mut Context<Self>) {
        let next = if hovered {
            Some(ix)
        } else if self.hovered == Some(ix) {
            None
        } else {
            self.hovered
        };
        if next != self.hovered {
            self.hovered = next;
            cx.notify();
        }
    }

    pub fn rows(mut self, rows: Vec<R>) -> Self {
        self.rows = Rc::new(rows);
        self.auto = None;
        self.resort();
        self
    }

    pub fn row_height(mut self, height: Pixels) -> Self {
        self.row_height = height;
        self
    }

    pub fn column_gap(mut self, gap: Pixels) -> Self {
        self.column_gap = gap;
        self
    }

    pub fn inset(mut self, inset: Pixels) -> Self {
        self.inset = inset;
        self
    }

    pub fn span_rows(
        mut self,
        span: impl Fn(&R, &Palette) -> Option<AnyElement> + 'static,
    ) -> Self {
        self.span = Some(Rc::new(span));
        self
    }

    pub fn click_opens(mut self) -> Self {
        self.click_opens = true;
        self
    }

    pub fn fit_content(mut self) -> Self {
        self.fit = true;
        self
    }

    pub fn unruled(mut self) -> Self {
        self.rules = false;
        self
    }

    pub fn headerless(mut self) -> Self {
        self.header = Header::Hidden;
        self
    }

    pub fn mono_header(mut self) -> Self {
        self.header = Header::Mono;
        self
    }

    pub fn empty(mut self, text: impl Into<SharedString>) -> Self {
        self.empty = text.into();
        self
    }

    pub fn set_empty(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        let text = text.into();
        if self.empty != text {
            self.empty = text;
            cx.notify();
        }
    }

    pub fn row_status(mut self, status: impl Fn(&R) -> Option<Status> + 'static) -> Self {
        self.status = Some(Rc::new(status));
        self
    }

    pub fn multi_select(mut self) -> Self {
        self.marked = Some(Vec::new());
        self
    }

    pub fn marked_rows(&self) -> Vec<&R> {
        self.marked
            .iter()
            .flatten()
            .filter_map(|&ix| self.rows.get(ix))
            .collect()
    }

    fn is_marked(&self, ix: usize) -> bool {
        self.marked
            .as_ref()
            .is_some_and(|marked| marked.contains(&ix))
    }

    fn toggle_mark(&mut self, row: usize) {
        if self.key.is_none() || row >= self.rows.len() {
            return;
        }
        let current = self
            .selected
            .filter(|&selected| selected != row && selected < self.rows.len());
        let Some(marked) = self.marked.as_mut() else {
            return;
        };
        if marked.is_empty() {
            marked.extend(current);
        }
        match marked.iter().position(|&marked| marked == row) {
            Some(at) => {
                marked.remove(at);
            }
            None => marked.push(row),
        }
        self.order_marks();
    }

    fn order_marks(&mut self) {
        let order = &self.order;
        if let Some(marked) = self.marked.as_mut() {
            marked.sort_by_key(|&ix| order.iter().position(|&shown| shown == ix));
        }
    }

    fn marked_keys(&self) -> Vec<SharedString> {
        let Some(key) = &self.key else {
            return Vec::new();
        };
        self.marked_rows().into_iter().map(|row| key(row)).collect()
    }

    pub fn row_key(mut self, key: impl Fn(&R) -> SharedString + 'static) -> Self {
        self.key = Some(Rc::new(key));
        self
    }

    pub fn type_ahead(mut self, name: impl Fn(&R) -> SharedString + 'static) -> Self {
        self.name = Some(Rc::new(name));
        self
    }

    pub fn sorted_by(mut self, column: usize, direction: SortDirection) -> Self {
        self.sort = Some((column, direction));
        self.resort();
        self
    }

    pub fn min_width(&self) -> Pixels {
        let widths = self.columns.iter().map(|c| self.width_of(c));
        min_row_width(widths, self.column_gap, self.inset + TRAILING)
    }

    pub fn row(&self, index: usize) -> Option<&R> {
        self.rows.get(index)
    }

    pub fn selected_row(&self) -> Option<&R> {
        self.selected.and_then(|ix| self.rows.get(ix))
    }

    pub fn select(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        self.selected = index.filter(|&ix| ix < self.rows.len());
        if let Some(position) = self.position_of(self.selected) {
            self.scroll.scroll_to_item(position, ScrollStrategy::Center);
        }
        cx.notify();
    }

    pub fn set_columns(&mut self, columns: Vec<Column<R>>, cx: &mut Context<Self>) {
        self.columns = Rc::new(columns);
        self.auto = None;
        self.sort = None;
        self.resort();
        cx.notify();
    }

    pub fn set_rows(&mut self, rows: Vec<R>, cx: &mut Context<Self>) {
        let selected_key = self.selected_key();
        let marked_keys = self.marked_keys();
        if self.record_flashes(&rows) {
            let latest = self.flash_generation;
            self.flash_timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(FLASH).await;
                this.update(cx, |table, cx| {
                    table.flashes.retain(|_, generation| *generation > latest);
                    cx.notify();
                })
                .ok();
            }));
        }
        self.rows = Rc::new(rows);
        self.auto = None;
        self.hovered = self.hovered.filter(|&ix| ix < self.rows.len());
        self.selected = match (selected_key, &self.key) {
            (Some(wanted), Some(key)) => self.rows.iter().position(|row| key(row) == wanted),
            _ => self.selected.filter(|&ix| ix < self.rows.len()),
        };
        if let Some(marked) = self.marked.as_mut() {
            *marked = match (&self.key, marked_keys.is_empty()) {
                (Some(key), false) => self
                    .rows
                    .iter()
                    .enumerate()
                    .filter(|(_, row)| marked_keys.contains(&key(row)))
                    .map(|(ix, _)| ix)
                    .collect(),
                _ => Vec::new(),
            };
        }
        self.resort();
        cx.notify();
    }

    fn selected_key(&self) -> Option<SharedString> {
        let key = self.key.as_ref()?;
        self.selected_row().map(|row| key(row))
    }

    fn record_flashes(&mut self, next: &[R]) -> bool {
        let (Some(key), Some(status)) = (&self.key, &self.status) else {
            return false;
        };
        let before: HashMap<SharedString, Status> = self
            .rows
            .iter()
            .filter_map(|row| status(row).map(|s| (key(row), s)))
            .collect();
        let after: Vec<(SharedString, Status)> = next
            .iter()
            .filter_map(|row| status(row).map(|s| (key(row), s)))
            .collect();
        let mut flashed = false;
        for changed in changed_statuses(&before, &after) {
            self.flash_generation += 1;
            self.flashes.insert(changed, self.flash_generation);
            flashed = true;
        }
        flashed
    }

    fn resort(&mut self) {
        let comparator = self.sort.and_then(|(col, dir)| {
            self.columns
                .get(col)
                .and_then(|c| c.sort.clone())
                .map(|c| (c, dir))
        });
        self.order = match comparator {
            Some((compare, direction)) => sort_order(&self.rows, compare.as_ref(), direction),
            None => (0..self.rows.len()).collect(),
        };
        self.order_marks();
    }

    fn position_of(&self, row: Option<usize>) -> Option<usize> {
        let row = row?;
        self.order.iter().position(|&ix| ix == row)
    }

    fn toggle_sort(&mut self, column: usize, cx: &mut Context<Self>) {
        self.sort = next_sort(self.sort, column);
        self.resort();
        cx.notify();
    }

    fn click_row(
        &mut self,
        row: usize,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.is_middle_click() {
            cx.emit(TableEvent::Opened(row));
        }
        if !event.standard_click() {
            return;
        }
        if is_text_drag(event, window, cx) {
            return;
        }
        window.focus(&self.focus, cx);
        if self.marked.is_some() && event.modifiers().shift {
            self.toggle_mark(row);
            self.selected = Some(row);
            cx.emit(TableEvent::Selected(row));
            cx.emit(TableEvent::Marked);
            cx.notify();
            return;
        }
        if let Some(marked) = self.marked.as_mut().filter(|marked| !marked.is_empty()) {
            marked.clear();
            cx.emit(TableEvent::Marked);
        }
        self.selected = Some(row);
        cx.emit(TableEvent::Selected(row));
        let new_tab = event.modifiers().platform;
        if self.click_opens || new_tab || event.click_count() > 1 {
            cx.emit(TableEvent::Opened(row));
        }
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.key == "enter" && !keystroke.modifiers.modified() {
            if let Some(row) = self.selected {
                cx.emit(TableEvent::Opened(row));
            }
            cx.stop_propagation();
            return;
        }
        let position = self.position_of(self.selected);
        let target = match (Nav::parse(keystroke), typed(keystroke)) {
            (Some(nav), _) => nav.target(position, self.order.len()),
            (None, Some(text)) => self.jump_to_name(text),
            (None, None) => return,
        };
        cx.stop_propagation();
        let Some(row) = target.and_then(|p| self.order.get(p).copied()) else {
            return;
        };
        self.selected = Some(row);
        if let Some(position) = self.position_of(Some(row)) {
            self.scroll
                .scroll_to_item(position, ScrollStrategy::Nearest);
        }
        cx.emit(TableEvent::Selected(row));
        cx.notify();
    }

    fn jump_to_name(&mut self, text: &str) -> Option<usize> {
        let key = self.name.clone().or_else(|| self.key.clone())?;
        let query = self.type_ahead.push(text, Instant::now()).to_owned();
        let names: Vec<SharedString> = self
            .order
            .iter()
            .filter_map(|&ix| self.rows.get(ix))
            .map(|row| key(row))
            .collect();
        find(&query, names.iter().map(SharedString::as_ref))
    }

    fn render_header(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let acc = palette.acc;
        let cells: Vec<AnyElement> = self
            .columns
            .iter()
            .enumerate()
            .map(|(ix, column)| {
                let arrow = match self.sort {
                    Some((col, SortDirection::Ascending)) if col == ix => Some(SORT_ASCENDING),
                    Some((col, SortDirection::Descending)) if col == ix => Some(SORT_DESCENDING),
                    Some(_) => None,
                    None => None,
                };
                let dragging = self.drag.is_some_and(|drag| drag.column == ix);
                let divider = div()
                    .w(RULE)
                    .h(COLUMN_DIVIDER)
                    .bg(if dragging { palette.acc } else { palette.sep })
                    .group_hover(COLUMN_HANDLE_GROUP, |style| style.bg(acc));
                let handle = div()
                    .debug_selector(|| format!("{}-resize-{ix}", self.id))
                    .group(COLUMN_HANDLE_GROUP)
                    .absolute()
                    .top_0()
                    .right(-(self.column_gap + COLUMN_HANDLE) / 2.)
                    .h_full()
                    .w(COLUMN_HANDLE)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor(CursorStyle::ResizeLeftRight)
                    .child(divider)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.start_resize(ix, event, window, cx);
                        }),
                    );
                let frame = justify(cell_body().size_full(), column.align)
                    .id(ix)
                    .child(column.title.clone())
                    .children(arrow);
                let frame = if column.sort.is_some() {
                    let fg = palette.fg;
                    frame
                        .cursor_pointer()
                        .hover(move |style| style.text_color(fg))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_sort(ix, cx)))
                } else {
                    frame
                };
                sized(div().relative().flex().h_full(), self.width_of(column))
                    .child(frame)
                    .child(handle)
                    .into_any_element()
            })
            .collect();
        let measured = self.measured.clone();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(self.column_gap)
            .h(ROW_TABLE_MIN)
            .pl(self.inset)
            .pr(TRAILING)
            .border_b(RULE)
            .border_color(palette.line)
            .text_size(TEXT_SMALL)
            .text_color(palette.mute)
            .when(self.header == Header::Mono, |header| {
                header.font_family(FONT_MONO).text_size(TEXT_AXIS)
            })
            .on_children_prepainted(move |bounds, _, _| {
                *measured.borrow_mut() = bounds.iter().map(|b| b.size.width).collect();
            })
            .children(cells)
            .into_any_element()
    }

    fn render_rows(
        &mut self,
        range: Range<usize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let palette = *cx.palette();
        let columns = self.columns.clone();
        let rows = self.rows.clone();
        range
            .filter_map(|position| {
                let ix = *self.order.get(position)?;
                let row = rows.get(ix)?;
                Some(self.render_row(ix, row, &columns, &palette, cx))
            })
            .collect()
    }

    fn render_row(
        &self,
        ix: usize,
        row: &R,
        columns: &[Column<R>],
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(content) = self.span.as_ref().and_then(|span| span(row, palette)) {
            return div()
                .flex()
                .items_center()
                .w_full()
                .h(self.row_height)
                .border_b(RULE)
                .border_color(palette.line)
                .child(content)
                .into_any_element();
        }
        let selected = self.selected == Some(ix) || self.is_marked(ix);
        let status = self.status.as_ref().and_then(|status| status(row));
        let tint = status.and_then(|status| palette.tint(status));
        let hover = palette.hover;
        let cells: Vec<Div> = columns
            .iter()
            .map(|column| {
                cell_frame(self.width_of(column), column.align)
                    .when(column.mono, |cell| {
                        cell.font_family(FONT_MONO).text_size(TEXT_MONO)
                    })
                    .child(column.render_cell(row, palette, &self.words, selected))
            })
            .collect();
        let more = (self.hovered == Some(ix))
            .then(|| self.row_more(ix, row, cx))
            .flatten();
        let element = div()
            .debug_selector(|| format!("{}-row-{ix}", self.id))
            .id(ix)
            .relative()
            .w_full()
            .flex()
            .items_center()
            .gap(self.column_gap)
            .h(self.row_height)
            .pl(self.inset)
            .pr(TRAILING)
            .when(self.rules, |row| {
                row.border_b(RULE).border_color(palette.line)
            })
            .text_size(TEXT_BODY)
            .text_color(palette.fg)
            .cursor_pointer()
            .when_some(tint, Styled::bg)
            .when(selected, |row| row.bg(palette.sel))
            .when(!selected, |row| row.hover(move |style| style.bg(hover)))
            .on_click(cx.listener(move |this, event, window, cx| {
                this.click_row(ix, event, window, cx);
            }))
            .on_aux_click(cx.listener(move |this, event, window, cx| {
                this.click_row(ix, event, window, cx);
            }))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.hover_row(ix, *hovered, cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.open_menu(ix, event.position, window, cx);
                }),
            )
            .children(cells)
            .children(more);
        let flash = self
            .key
            .as_ref()
            .filter(|_| !self.flashes.is_empty())
            .and_then(|key| self.flashes.get(&key(row)).copied())
            .zip(status);
        match flash {
            Some((generation, status)) if !selected => {
                let from = palette.status(status).opacity(FLASH_STRENGTH);
                let to = tint.unwrap_or(palette.editor);
                let animation = Animation::new(FLASH).with_easing(ease_in_out);
                let key = ElementId::NamedInteger(self.id.clone(), generation);
                element
                    .with_animation(key, animation, move |row, delta| {
                        row.bg(mix(from, to, delta))
                    })
                    .into_any_element()
            }
            Some(_) => element.into_any_element(),
            None => element.into_any_element(),
        }
    }
}

impl<R: 'static> Table<R> {
    fn row_more(&self, ix: usize, row: &R, cx: &mut Context<Self>) -> Option<AnyElement> {
        let actions = self.actions.as_ref()?;
        if actions(row, &self.marked_rows()).is_empty() {
            return None;
        }
        let id = format!("{}-row-{ix}-more", self.id);
        let button = IconButton::glyph(ElementId::Name(id.clone().into()), MORE, MORE_TOOLTIP)
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.open_menu(ix, event.position(), window, cx);
            }));
        Some(
            div()
                .debug_selector(|| id)
                .absolute()
                .top_0()
                .right(SPACE_4)
                .h_full()
                .flex()
                .items_center()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(button)
                .into_any_element(),
        )
    }
}

fn stored_widths(scope: &str, table: &str, cx: &App) -> HashMap<SharedString, Pixels> {
    prefs::column_widths(scope, table, cx)
        .into_iter()
        .filter(|(_, width)| width.is_finite())
        .map(|(title, width)| (SharedString::from(title), gpui::px(width).max(COLUMN_MIN)))
        .collect()
}

fn mix(from: Hsla, to: Hsla, delta: f32) -> Hsla {
    let from = from.to_rgb();
    let to = to.to_rgb();
    let lerp = |a: f32, b: f32| a + (b - a) * delta;
    gpui::Rgba {
        r: lerp(from.r, to.r),
        g: lerp(from.g, to.g),
        b: lerp(from.b, to.b),
        a: lerp(from.a, to.a),
    }
    .into()
}

impl<R: 'static> Focusable for Table<R> {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl<R: 'static> Render for Table<R> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.resolve_auto(window);
        let palette = *cx.palette();
        let header = (self.header != Header::Hidden).then(|| self.render_header(&palette, cx));
        let body = if self.rows.is_empty() {
            div()
                .flex()
                .items_center()
                .h(self.row_height)
                .px(SPACE_8)
                .border_b(RULE)
                .border_color(palette.line)
                .text_size(TEXT_BODY)
                .text_color(palette.mute)
                .child(self.empty.clone())
                .into_any_element()
        } else {
            let list = uniform_list(
                ElementId::Name(self.id.clone()),
                self.order.len(),
                cx.processor(Self::render_rows),
            )
            .track_scroll(&self.scroll);
            if self.fit {
                list.with_sizing_behavior(ListSizingBehavior::Infer)
                    .w_full()
                    .into_any_element()
            } else {
                list.flex_1().min_h_0().into_any_element()
            }
        };
        let content = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w(self.min_width())
            .when(!self.fit, Styled::h_full)
            .children(header)
            .child(body);
        div()
            .id(ElementId::NamedInteger(self.id.clone(), 0))
            .key_context(TABLE_CONTEXT)
            .relative()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.key_down(event, cx)))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                this.drag_to(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.finish_resize(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.finish_resize(cx)),
            )
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered && this.hovered.take().is_some() {
                    cx.notify();
                }
            }))
            .overflow_x_scroll()
            .restrict_scroll_to_axis()
            .when(!self.fit, Styled::size_full)
            .when(self.fit, Styled::w_full)
            .child(content)
            .child(self.menu.clone())
    }
}

#[cfg(test)]
mod tests {
    use gpui::px;

    use super::*;

    #[test]
    fn sorts_stably_in_both_directions() {
        let rows = [("b", 2), ("a", 1), ("c", 2), ("d", 0)];
        let by_number = |a: &(&str, i32), b: &(&str, i32)| a.1.cmp(&b.1);
        assert_eq!(
            sort_order(&rows, &by_number, SortDirection::Ascending),
            [3, 1, 0, 2]
        );
        assert_eq!(
            sort_order(&rows, &by_number, SortDirection::Descending),
            [0, 2, 1, 3]
        );
    }

    #[test]
    fn min_width_adds_columns_gaps_and_padding() {
        let widths = [
            Width::Flex {
                weight: 1.3,
                min: px(140.),
            },
            Width::Fixed(px(128.)),
            Width::Fixed(px(76.)),
        ];
        assert_eq!(
            min_row_width(widths.into_iter(), px(14.), px(32.)),
            px(140. + 128. + 76. + 2. * 14. + 32.)
        );
        assert_eq!(min_row_width(std::iter::empty(), px(14.), px(32.)), px(32.));
    }

    #[test]
    fn auto_width_follows_content_within_bounds() {
        assert_eq!(auto_width(px(10.), px(40.), px(120.)), px(40.));
        assert_eq!(auto_width(px(75.2), px(40.), px(120.)), px(76.));
        assert_eq!(auto_width(px(300.), px(40.), px(120.)), px(120.));
        assert_eq!(auto_width(px(300.), px(40.), px(20.)), px(40.));
    }

    #[test]
    fn min_width_counts_auto_columns_at_their_minimum() {
        let widths = [
            Width::Auto {
                min: px(40.),
                max: px(200.),
            },
            Width::Fixed(px(60.)),
        ];
        assert_eq!(
            min_row_width(widths.into_iter(), px(10.), px(0.)),
            px(40. + 60. + 10.)
        );
    }

    #[test]
    fn resizing_never_goes_below_the_minimum() {
        assert_eq!(resized(px(120.), px(40.), px(32.)), px(160.));
        assert_eq!(resized(px(120.), px(-200.), px(32.)), px(32.));
    }

    #[test]
    fn header_clicks_cycle_ascending_descending_off() {
        let first = next_sort(None, 2);
        assert_eq!(first, Some((2, SortDirection::Ascending)));
        let second = next_sort(first, 2);
        assert_eq!(second, Some((2, SortDirection::Descending)));
        assert_eq!(next_sort(second, 2), None);
        assert_eq!(next_sort(second, 0), Some((0, SortDirection::Ascending)));
    }

    #[test]
    fn flags_only_rows_whose_status_changed() {
        let before = HashMap::from([
            (SharedString::from("a"), Status::Ok),
            (SharedString::from("b"), Status::Warn),
        ]);
        let after = [
            (SharedString::from("a"), Status::Err),
            (SharedString::from("b"), Status::Warn),
            (SharedString::from("c"), Status::Err),
        ];
        assert_eq!(changed_statuses(&before, &after), [SharedString::from("a")]);
    }

    #[gpui::test]
    fn a_status_flash_ends_after_it_has_played(cx: &mut gpui::TestAppContext) {
        let table = cx.new(|cx| {
            Table::new("flash", Vec::new(), cx)
                .row_key(|row: &(&str, Status)| row.0.into())
                .row_status(|row| Some(row.1))
                .rows(vec![("a", Status::Ok)])
        });
        table.update(cx, |table, cx| table.set_rows(vec![("a", Status::Err)], cx));
        assert!(table.read_with(cx, |table, _| table.flashes.contains_key("a")));
        cx.executor().advance_clock(FLASH);
        cx.run_until_parked();
        assert!(table.read_with(cx, |table, _| table.flashes.is_empty()));
    }

    fn marked_names(table: &Table<&'static str>) -> Vec<&'static str> {
        table.marked_rows().into_iter().copied().collect()
    }

    #[gpui::test]
    fn marks_follow_their_rows_through_updates_and_sorting(cx: &mut gpui::TestAppContext) {
        let table = cx.new(|cx| {
            let columns =
                vec![Column::text("Name", |row: &&str| (*row).into()).sort_by_key(|row| *row)];
            Table::new("marks", columns, cx)
                .row_key(|row: &&str| (*row).into())
                .multi_select()
                .rows(vec!["b", "a", "c"])
        });
        table.update(cx, |table, cx| {
            table.select(Some(2), cx);
            table.toggle_mark(0);
            assert_eq!(marked_names(table), ["b", "c"]);
            table.toggle_mark(1);
            table.toggle_mark(2);
            assert_eq!(marked_names(table), ["b", "a"]);
            table.toggle_sort(0, cx);
            assert_eq!(marked_names(table), ["a", "b"]);
            table.set_rows(vec!["d", "b", "c"], cx);
            assert_eq!(marked_names(table), ["b"]);
            assert!(table.is_marked(1));
            assert!(!table.is_marked(0));
        });
    }

    #[gpui::test]
    fn reading_marks_derives_no_row_keys(cx: &mut gpui::TestAppContext) {
        let keys = Rc::new(std::cell::Cell::new(0_usize));
        let counted = keys.clone();
        let rows: Vec<usize> = (0..100).collect();
        let table = cx.new(|cx| {
            Table::new("marks", Vec::new(), cx)
                .row_key(move |row: &usize| {
                    counted.set(counted.get() + 1);
                    row.to_string().into()
                })
                .multi_select()
                .rows(rows)
        });
        table.update(cx, |table, _| {
            table.toggle_mark(7);
            keys.set(0);
            assert_eq!(table.marked_rows(), [&7]);
            assert!((0..100).filter(|&ix| table.is_marked(ix)).eq([7]));
        });
        assert_eq!(keys.get(), 0);
    }
}
