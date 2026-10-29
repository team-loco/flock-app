use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;

use flock_core::Status;
use flock_core::filter::highlights;
use flock_core::grouping::{Group, Grouped, Item, group};
use flock_core::units::Amount;
use gpui::{
    Animation, AnimationExt, AnyElement, App, AppContext as _, ClickEvent, Context, CursorStyle,
    Div, ElementId, Entity, EventEmitter, FocusHandle, Focusable, HighlightStyle, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, ListSizingBehavior, MouseButton, MouseDownEvent,
    MouseMoveEvent, ParentElement, Pixels, Point, Render, ScrollStrategy, SharedString,
    StatefulInteractiveElement, StyleRefinement, Styled, Task, TextRun, UniformListScrollHandle,
    ViewElement, Window, div, ease_in_out, prelude::FluentBuilder, uniform_list,
};

use super::button::IconButton;
use super::group::{GROUP_PREFIX, Grouping, Header as GroupHeader, UNGROUPED, option_label};
use super::list_keys::{Nav, TypeAhead, find, typed};
use super::menu::{ActionMenu, MenuItem};
use super::selectable::{SelectableText, is_text_drag};
use super::status::StatusGlyph;
use super::text::{AmountText, Highlighted};
use super::tooltip::Tooltip;
use crate::assets::GROUP;
use crate::prefs::{self, ColumnWidths, FLOCK_SCOPE};
use crate::theme::{
    ActiveTheme, COLUMN_DIVIDER, COLUMN_HANDLE, COLUMN_MIN, FLASH, FONT_MONO, FONT_SANS,
    GLYPH_SLOT, GROUP_MENU_WIDTH, Palette, ROW_TABLE, ROW_TABLE_MIN, RULE, SPACE_4, SPACE_6,
    SPACE_8, TEXT_AXIS, TEXT_BODY, TEXT_MONO, TEXT_SMALL,
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
    after: &HashMap<SharedString, Status>,
) -> Vec<SharedString> {
    after
        .iter()
        .filter(|&(key, status)| before.get(key).is_some_and(|old| old != status))
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
    statuses: Option<HashMap<SharedString, Status>>,
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
    grouping: Option<Grouping<R>>,
    group_by: Option<usize>,
    collapsed: BTreeSet<String>,
    groups: Vec<Group>,
    items: Vec<Item>,
    header_focus: Option<String>,
    group_menu: Entity<ActionMenu>,
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
        let group_menu = cx.new(|cx| {
            ActionMenu::new(format!("{id}-group"), cx)
                .prefixed(GROUP, GROUP_PREFIX, UNGROUPED)
                .width(GROUP_MENU_WIDTH)
        });
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
            statuses: None,
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
            grouping: None,
            group_by: None,
            collapsed: BTreeSet::new(),
            groups: Vec::new(),
            items: Vec::new(),
            header_focus: None,
            group_menu,
        }
    }

    pub fn grouped(mut self, grouping: Grouping<R>, cx: &mut Context<Self>) -> Self {
        self.grouping = Some(grouping);
        self.load_grouping(cx);
        self.resort();
        self
    }

    pub fn group_menu(&self) -> &Entity<ActionMenu> {
        &self.group_menu
    }

    pub fn group_by(&mut self, facet: Option<&str>, cx: &mut Context<Self>) {
        let option = facet.and_then(|facet| self.option_of(facet));
        if option == self.group_by {
            return;
        }
        self.group_by = option;
        self.collapsed.clear();
        self.header_focus = None;
        self.save_grouping(cx);
        self.sync_group_menu(cx);
        self.regroup();
        cx.notify();
    }

    pub fn toggle_group(&mut self, value: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(value) {
            self.collapsed.insert(value.to_owned());
        }
        self.header_focus = Some(value.to_owned());
        self.save_grouping(cx);
        self.regroup();
        cx.notify();
    }

    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    fn option_of(&self, facet: &str) -> Option<usize> {
        let options = &self.grouping.as_ref()?.options;
        options.iter().position(|option| option.facet == facet)
    }

    fn group_facet(&self) -> Option<&SharedString> {
        let options = &self.grouping.as_ref()?.options;
        options.get(self.group_by?).map(|option| &option.facet)
    }

    fn load_grouping(&mut self, cx: &mut Context<Self>) {
        if self.grouping.is_none() {
            return;
        }
        let saved = prefs::grouping(&self.scope, &self.id, cx);
        self.group_by = saved.by.as_deref().and_then(|facet| self.option_of(facet));
        self.collapsed = saved.collapsed;
        self.header_focus = None;
        self.sync_group_menu(cx);
    }

    fn save_grouping(&self, cx: &mut App) {
        let saved = prefs::Grouping {
            by: self.group_facet().map(ToString::to_string),
            collapsed: self.collapsed.clone(),
        };
        prefs::set_grouping(&self.scope, &self.id, saved, cx);
    }

    fn sync_group_menu(&self, cx: &mut Context<Self>) {
        let Some(grouping) = &self.grouping else {
            return;
        };
        let table = cx.weak_entity();
        let choices = std::iter::once(None).chain(
            grouping
                .options
                .iter()
                .map(|option| Some(option.facet.clone())),
        );
        let items: Vec<MenuItem> = choices
            .enumerate()
            .map(|(ix, facet)| {
                let label = facet
                    .as_deref()
                    .map_or_else(|| UNGROUPED.into(), option_label);
                let checked = facet.as_ref() == self.group_facet();
                let table = table.clone();
                MenuItem::new(label, move |_, cx| {
                    let facet = facet.clone();
                    let table = table.clone();
                    cx.defer(move |cx| {
                        let _ = table.update(cx, |table, cx| table.group_by(facet.as_deref(), cx));
                    });
                })
                .checked(checked)
                .shortcut((ix + 1).to_string())
            })
            .collect();
        let label = self
            .group_facet()
            .map_or_else(|| UNGROUPED.into(), |facet| option_label(facet));
        self.group_menu.update(cx, |menu, cx| {
            menu.set_items(items, cx);
            menu.set_label(label, cx);
        });
    }

    fn regroup(&mut self) {
        let grouped = match (&self.grouping, self.group_by) {
            (Some(grouping), Some(option)) => grouping.options.get(option).map(|option| {
                let rows = &self.rows;
                let entries = self.order.iter().filter_map(|&ix| {
                    let row = rows.get(ix)?;
                    Some((ix, (option.key)(row), (grouping.status)(row)))
                });
                group(entries, &self.collapsed)
            }),
            (Some(_), None) => None,
            (None, _) => None,
        };
        let Grouped { groups, items } = grouped.unwrap_or_else(|| Grouped {
            groups: Vec::new(),
            items: self.order.iter().map(|&ix| Item::Row(ix)).collect(),
        });
        self.groups = groups;
        self.items = items;
        let focus = self.header_focus.take();
        self.header_focus = focus.filter(|value| self.groups.iter().any(|g| &g.value == value));
    }

    pub fn row_actions(mut self, actions: impl Fn(&R, &[&R]) -> Vec<MenuItem> + 'static) -> Self {
        self.actions = Some(Rc::new(actions));
        self
    }

    pub fn set_row_actions(&mut self, actions: impl Fn(&R, &[&R]) -> Vec<MenuItem> + 'static) {
        self.actions = Some(Rc::new(actions));
    }

    pub fn set_scope(&mut self, scope: impl Into<SharedString>, cx: &mut Context<Self>) {
        let scope = scope.into();
        if scope == self.scope {
            return;
        }
        self.widths = stored_widths(&scope, &self.id, cx);
        self.scope = scope;
        self.load_grouping(cx);
        self.regroup();
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
        self.statuses = None;
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

    pub fn pane(view: &Entity<Self>, cx: &App) -> ViewElement<Entity<Self>> {
        let table = view.read(cx);
        if !table.fit {
            return super::pane(view);
        }
        let style = StyleRefinement::default().w_full().h(table.fit_height());
        view.clone().cached(style)
    }

    fn fit_height(&self) -> Pixels {
        let header = match self.header {
            Header::Hidden => Pixels::ZERO,
            Header::Sans => ROW_TABLE_MIN,
            Header::Mono => ROW_TABLE_MIN,
        };
        let rows = u16::try_from(self.items.len().max(1)).unwrap_or(u16::MAX);
        header + self.row_height * f32::from(rows)
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
        let index = index.filter(|&ix| ix < self.rows.len());
        if index == self.selected {
            return;
        }
        self.selected = index;
        self.header_focus = None;
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

    fn statuses_of(&self, rows: &[R]) -> Option<HashMap<SharedString, Status>> {
        let (Some(key), Some(status)) = (&self.key, &self.status) else {
            return None;
        };
        let statuses = rows
            .iter()
            .filter_map(|row| status(row).map(|s| (key(row), s)))
            .collect();
        Some(statuses)
    }

    fn record_flashes(&mut self, next: &[R]) -> bool {
        let Some(after) = self.statuses_of(next) else {
            return false;
        };
        let before = match self.statuses.take() {
            Some(before) => before,
            None => self.statuses_of(&self.rows).unwrap_or_default(),
        };
        let mut flashed = false;
        for changed in changed_statuses(&before, &after) {
            self.flash_generation += 1;
            self.flashes.insert(changed, self.flash_generation);
            flashed = true;
        }
        self.statuses = Some(after);
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
        self.regroup();
    }

    fn position_of(&self, row: Option<usize>) -> Option<usize> {
        let row = row?;
        self.items.iter().position(|&item| item == Item::Row(row))
    }

    fn focused_group(&self) -> Option<usize> {
        let value = self.header_focus.as_ref()?;
        self.groups.iter().position(|group| &group.value == value)
    }

    fn cursor(&self) -> Option<usize> {
        match self.focused_group() {
            Some(group) => self
                .items
                .iter()
                .position(|&item| item == Item::Group(group)),
            None => self.position_of(self.selected),
        }
    }

    fn shows_selection(&self, ix: usize) -> bool {
        (self.selected == Some(ix) && self.header_focus.is_none()) || self.is_marked(ix)
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
        self.header_focus = None;
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
            let focused = self.focused_group().and_then(|g| self.groups.get(g));
            match (focused.map(|group| group.value.clone()), self.selected) {
                (Some(value), _) => self.toggle_group(&value, cx),
                (None, Some(row)) => cx.emit(TableEvent::Opened(row)),
                (None, None) => {}
            }
            cx.stop_propagation();
            return;
        }
        let target = match (Nav::parse(keystroke), typed(keystroke)) {
            (Some(nav), _) => nav.target(self.cursor(), self.items.len()),
            (None, Some(text)) => self.jump_to_name(text),
            (None, None) => return,
        };
        cx.stop_propagation();
        let Some(position) = target else {
            return;
        };
        match self.items.get(position).copied() {
            Some(Item::Row(row)) => {
                self.selected = Some(row);
                self.header_focus = None;
                cx.emit(TableEvent::Selected(row));
            }
            Some(Item::Group(group)) => {
                self.header_focus = self.groups.get(group).map(|g| g.value.clone());
            }
            None => return,
        }
        self.scroll
            .scroll_to_item(position, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn jump_to_name(&mut self, text: &str) -> Option<usize> {
        let key = self.name.clone().or_else(|| self.key.clone())?;
        let query = self.type_ahead.push(text, Instant::now()).to_owned();
        let rows: Vec<(usize, SharedString)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(position, item)| match item {
                Item::Row(ix) => self.rows.get(*ix).map(|row| (position, key(row))),
                Item::Group(_) => None,
            })
            .collect();
        let found = find(&query, rows.iter().map(|(_, name)| name.as_ref()))?;
        rows.get(found).map(|(position, _)| *position)
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
            .filter_map(|position| match *self.items.get(position)? {
                Item::Row(ix) => {
                    let row = rows.get(ix)?;
                    Some(self.render_row(ix, row, &columns, &palette, cx))
                }
                Item::Group(group) => self.render_group(group, &palette, cx),
            })
            .collect()
    }

    fn render_group(
        &self,
        ix: usize,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let group = self.groups.get(ix)?;
        let facet = self.group_facet()?;
        let header = GroupHeader {
            id: format!("{}-group-{ix}", self.id).into(),
            facet,
            group,
            focused: self.header_focus.as_ref() == Some(&group.value),
            height: self.row_height,
            inset: self.inset,
            trailing: TRAILING,
        };
        let value = group.value.clone();
        let element = header
            .render(palette)
            .on_click(cx.listener(move |this, _, window, cx| {
                window.focus(&this.focus, cx);
                this.toggle_group(&value, cx);
            }));
        Some(element.into_any_element())
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
        let selected = self.shows_selection(ix);
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
                self.items.len(),
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

    use super::super::SnapshotRow;

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
        let after = HashMap::from([
            (SharedString::from("a"), Status::Err),
            (SharedString::from("b"), Status::Warn),
            (SharedString::from("c"), Status::Err),
        ]);
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

    #[gpui::test]
    fn the_table_notifies_only_when_what_it_shows_changes(cx: &mut gpui::TestAppContext) {
        let table = cx.new(|cx| Table::new("quiet", Vec::new(), cx).rows(vec!["a", "b"]));
        let notified = Rc::new(std::cell::Cell::new(0));
        let count = Rc::clone(&notified);
        let _observer = cx.update(|cx| cx.observe(&table, move |_, _| count.set(count.get() + 1)));
        table.update(cx, |table, cx| table.select(None, cx));
        table.update(cx, |table, cx| table.set_highlight(Vec::new(), cx));
        table.update(cx, |table, cx| table.hover_row(0, false, cx));
        table.update(cx, |table, _| table.set_row_actions(|_, _| Vec::new()));
        assert_eq!(notified.get(), 0);
        table.update(cx, |table, cx| table.select(Some(1), cx));
        table.update(cx, |table, cx| table.select(Some(1), cx));
        assert_eq!(notified.get(), 1);
        table.update(cx, |table, cx| table.hover_row(0, true, cx));
        assert_eq!(notified.get(), 2);
    }

    #[gpui::test]
    fn a_fitted_table_is_as_tall_as_its_header_and_rows(cx: &mut gpui::TestAppContext) {
        let fitted = cx.new(|cx| {
            Table::new("fitted", Vec::new(), cx)
                .fit_content()
                .rows(vec!["a", "b", "c"])
        });
        let height = fitted.read_with(cx, |table, _| table.fit_height());
        assert_eq!(height, ROW_TABLE_MIN + ROW_TABLE * 3.);
        let empty = cx.new(|cx| {
            Table::<&str>::new("empty", Vec::new(), cx)
                .fit_content()
                .headerless()
        });
        let height = empty.read_with(cx, |table, _| table.fit_height());
        assert_eq!(height, ROW_TABLE);
    }

    type Member = (&'static str, &'static str, Status);

    fn team_table(cx: &mut gpui::TestAppContext) -> Entity<Table<Member>> {
        cx.new(|cx| {
            let columns = vec![
                Column::text("Name", |row: &Member| row.0.into()).sort_by_key(|row: &Member| row.0),
            ];
            let grouping = Grouping::new(|row: &Member| row.2).by("team", |row: &Member| {
                Some(std::borrow::Cow::Borrowed(row.1))
            });
            Table::new("teams", columns, cx)
                .row_key(|row: &Member| row.0.into())
                .grouped(grouping, cx)
                .rows(vec![
                    ("b", "web", Status::Ok),
                    ("a", "pay", Status::Err),
                    ("c", "web", Status::Warn),
                    ("d", "pay", Status::Ok),
                ])
        })
    }

    fn shown(table: &Table<Member>) -> Vec<String> {
        table
            .items
            .iter()
            .map(|item| match *item {
                Item::Group(g) => table
                    .groups
                    .get(g)
                    .map(|group| format!("[{}]", group.value))
                    .unwrap_or_default(),
                Item::Row(ix) => table
                    .rows
                    .get(ix)
                    .map(|row| row.0.to_owned())
                    .unwrap_or_default(),
            })
            .collect()
    }

    #[gpui::test]
    fn grouping_puts_headers_before_their_sorted_rows_and_hides_collapsed_groups(
        cx: &mut gpui::TestAppContext,
    ) {
        let table = team_table(cx);
        table.update(cx, |table, cx| {
            assert_eq!(shown(table), ["b", "a", "c", "d"]);
            table.group_by(Some("team"), cx);
            assert_eq!(shown(table), ["[pay]", "a", "d", "[web]", "b", "c"]);
            table.toggle_sort(0, cx);
            table.toggle_sort(0, cx);
            assert_eq!(shown(table), ["[pay]", "d", "a", "[web]", "c", "b"]);
            table.toggle_group("pay", cx);
            assert_eq!(shown(table), ["[pay]", "[web]", "c", "b"]);
            assert_eq!(table.fit_height(), ROW_TABLE_MIN + ROW_TABLE * 4.);
            table.group_by(None, cx);
            assert_eq!(shown(table), ["d", "c", "b", "a"]);
        });
    }

    #[gpui::test]
    fn grouping_and_collapsed_groups_are_remembered_per_scope(cx: &mut gpui::TestAppContext) {
        let table = team_table(cx);
        table.update(cx, |table, cx| {
            table.set_scope("prod", cx);
            table.group_by(Some("team"), cx);
            table.toggle_group("web", cx);
            table.set_scope("staging", cx);
            assert_eq!(shown(table), ["b", "a", "c", "d"]);
            table.set_scope("prod", cx);
            assert_eq!(shown(table), ["[pay]", "a", "d", "[web]"]);
        });
        let reopened = team_table(cx);
        reopened.update(cx, |table, cx| {
            table.set_scope("prod", cx);
            assert_eq!(shown(table), ["[pay]", "a", "d", "[web]"]);
        });
    }

    fn press(table: &Entity<Table<Member>>, key: &str, cx: &mut gpui::TestAppContext) {
        let keystroke =
            gpui::Keystroke::parse(key).unwrap_or_else(|error| panic!("{key}: {error}"));
        let event = KeyDownEvent {
            keystroke,
            is_held: false,
            prefer_character_input: false,
        };
        table.update(cx, |table, cx| table.key_down(&event, cx));
    }

    #[gpui::test]
    fn arrow_keys_land_on_headers_and_enter_toggles_them(cx: &mut gpui::TestAppContext) {
        let table = team_table(cx);
        let events = Rc::new(RefCell::new(Vec::new()));
        let seen = events.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&table, move |_, event: &TableEvent, _| {
                seen.borrow_mut().push(*event);
            })
        });
        table.update(cx, |table, cx| table.group_by(Some("team"), cx));
        press(&table, "down", cx);
        assert_eq!(
            table
                .read_with(cx, |t, _| t.header_focus.clone())
                .as_deref(),
            Some("pay")
        );
        press(&table, "enter", cx);
        assert_eq!(
            table.read_with(cx, |t, _| shown(t)),
            ["[pay]", "[web]", "b", "c"]
        );
        press(&table, "down", cx);
        press(&table, "down", cx);
        assert_eq!(
            table.read_with(cx, |t, _| t.selected_row().map(|r| r.0)),
            Some("b")
        );
        assert_eq!(table.read_with(cx, |t, _| t.header_focus.clone()), None);
        press(&table, "enter", cx);
        assert_eq!(
            *events.borrow(),
            [TableEvent::Selected(0), TableEvent::Opened(0)]
        );
        press(&table, "up", cx);
        press(&table, "enter", cx);
        assert_eq!(table.read_with(cx, |t, _| shown(t)), ["[pay]", "[web]"]);
        assert_eq!(events.borrow().len(), 2);
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

    fn snapshot_table(
        rows: Vec<SnapshotRow<flock_core::events::Event>>,
        cx: &mut gpui::TestAppContext,
    ) -> Entity<Table<SnapshotRow<flock_core::events::Event>>> {
        type Row = SnapshotRow<flock_core::events::Event>;
        cx.new(|cx| {
            let columns = vec![
                Column::text("Reason", |row: &Row| row.reason.clone().into())
                    .sort_by_key(|row: &Row| row.reason.clone()),
            ];
            Table::new("snapshot-rows", columns, cx)
                .row_key(|row: &Row| row.reason.clone().into())
                .row_status(|row: &Row| (row.status != Status::Ok).then_some(row.status))
                .multi_select()
                .rows(rows)
        })
    }

    fn reasons(rows: &[&SnapshotRow<flock_core::events::Event>]) -> Vec<String> {
        rows.iter().map(|row| row.reason.clone()).collect()
    }

    #[gpui::test]
    fn snapshot_rows_keep_selection_marks_sort_and_flashes_across_snapshots(
        cx: &mut gpui::TestAppContext,
    ) {
        use super::super::snapshot_row::tests::{event, events};
        let cluster = flock_core::ClusterId::new("a");
        let before = events(
            &cluster,
            vec![
                event("b", Status::Warn),
                event("a", Status::Ok),
                event("c", Status::Ok),
            ],
        );
        let table = snapshot_table(before.all(), cx);
        table.update(cx, |table, cx| {
            table.select(Some(0), cx);
            table.toggle_mark(2);
            table.toggle_sort(0, cx);
            assert_eq!(reasons(&table.marked_rows()), ["b", "c"]);
        });
        let after = events(
            &cluster,
            vec![
                event("c", Status::Ok),
                event("d", Status::Ok),
                event("b", Status::Err),
                event("a", Status::Ok),
            ],
        );
        table.update(cx, |table, cx| {
            table.set_rows(after.all(), cx);
            let selected = table.selected_row().map(|row| row.reason.clone());
            assert_eq!(selected.as_deref(), Some("b"));
            assert_eq!(reasons(&table.marked_rows()), ["b", "c"]);
            let sorted: Vec<String> = table
                .order
                .iter()
                .filter_map(|&ix| table.row(ix))
                .map(|row| row.reason.clone())
                .collect();
            assert_eq!(sorted, ["a", "b", "c", "d"]);
            assert!(table.flashes.contains_key("b"));
            assert_eq!(table.flashes.len(), 1);
        });
    }

    #[gpui::test]
    fn updating_rows_derives_each_status_key_once(cx: &mut gpui::TestAppContext) {
        let keys = Rc::new(std::cell::Cell::new(0_usize));
        let counted = keys.clone();
        let rows = |status| {
            (0..100)
                .map(|ix| (ix, status))
                .collect::<Vec<(usize, Status)>>()
        };
        let table = cx.new(|cx| {
            Table::new("keys", Vec::new(), cx)
                .row_key(move |row: &(usize, Status)| {
                    counted.set(counted.get() + 1);
                    row.0.to_string().into()
                })
                .row_status(|row| Some(row.1))
                .rows(rows(Status::Ok))
        });
        table.update(cx, |table, cx| table.set_rows(rows(Status::Warn), cx));
        keys.set(0);
        table.update(cx, |table, cx| table.set_rows(rows(Status::Err), cx));
        assert_eq!(keys.get(), 100);
        assert_eq!(table.read_with(cx, |table, _| table.flashes.len()), 100);
    }

    #[gpui::test]
    fn updating_rows_without_selection_marks_or_status_derives_no_keys(
        cx: &mut gpui::TestAppContext,
    ) {
        let keys = Rc::new(std::cell::Cell::new(0_usize));
        let counted = keys.clone();
        let table = cx.new(|cx| {
            Table::new("keys", Vec::new(), cx)
                .row_key(move |row: &usize| {
                    counted.set(counted.get() + 1);
                    row.to_string().into()
                })
                .multi_select()
                .rows((0..100).collect())
        });
        table.update(cx, |table, cx| table.set_rows((0..100).rev().collect(), cx));
        assert_eq!(keys.get(), 0);
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
