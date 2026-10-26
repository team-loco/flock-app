use std::ops::Range;

use flock_core::TermSize;
use gpui::{
    AnyElement, App, ClipboardItem, Context, CursorStyle, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, Keystroke,
    MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, StyleRefinement, Styled, UniformListScrollHandle, ViewElement,
    Window, canvas, div, font, prelude::FluentBuilder, uniform_list,
};

use crate::theme::{
    ActiveTheme, FONT_MONO, LOG_LINE, Palette, SPACE_3, SPACE_6, TEXT_MONO, terminal_color,
};

gpui::actions!(terminal, [SendTab, SendBackTab]);

pub const TERMINAL_CONTEXT: &str = "Terminal";
const DELETE: u8 = 0x7f;
const ESCAPE: u8 = 0x1b;
const CTRL_OFFSET: u8 = 0x60;
const CSI: u8 = b'[';
const SS3: u8 = b'O';
const NEWLINE: &[u8] = b"\r\n";
const NOTE_START: &str = "\x1b[0;2m";
const NOTE_END: &str = "\x1b[0m\r\n";
const PASTE_START: &str = "\x1b[200~";
const PASTE_END: &str = "\x1b[201~";
const BLANK: &str = " ";
const TAB: &str = "tab";
const WIDE_CELLS: u16 = 2;

const STATUS_REPORT: char = 'n';
const DEVICE_ATTRIBUTES: char = 'c';
const CURSOR_REPORT: u16 = 6;
const DEVICE_STATUS: u16 = 5;
const DEVICE_OK: &str = "\x1b[0n";
const DEVICE_ID: &str = "\x1b[?1;2c";

const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;
const SCROLLBACK_ROWS: usize = 10_000;
const REPLAY_LIMIT: usize = 4 * 1024 * 1024;

#[derive(Default)]
struct Replies(Vec<u8>);

impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        first: Option<u8>,
        _: Option<u8>,
        params: &[&[u16]],
        action: char,
    ) {
        let param = params.first().and_then(|p| p.first()).copied();
        let reply = match (first, action, param) {
            (None, STATUS_REPORT, Some(CURSOR_REPORT)) => {
                let (row, col) = screen.cursor_position();
                format!("\x1b[{};{}R", row + 1, col + 1)
            }
            (None, STATUS_REPORT, Some(DEVICE_STATUS)) => DEVICE_OK.to_owned(),
            (None, DEVICE_ATTRIBUTES, None | Some(0)) => DEVICE_ID.to_owned(),
            _ => return,
        };
        self.0.extend_from_slice(reply.as_bytes());
    }
}

pub struct TerminalBuffer {
    parser: vt100::Parser<Replies>,
    replay: Vec<u8>,
    open: bool,
}

fn parser(rows: u16, cols: u16) -> vt100::Parser<Replies> {
    vt100::Parser::new_with_callbacks(rows, cols, SCROLLBACK_ROWS, Replies::default())
}

impl Default for TerminalBuffer {
    fn default() -> Self {
        Self {
            parser: parser(DEFAULT_ROWS, DEFAULT_COLS),
            replay: Vec::new(),
            open: false,
        }
    }
}

impl TerminalBuffer {
    fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        self.replay.extend_from_slice(bytes);
        if self.replay.len() > REPLAY_LIMIT {
            let excess = self.replay.len() - REPLAY_LIMIT / 2;
            self.replay.drain(..excess);
        }
    }

    pub fn note(&mut self, text: &str) {
        if self.parser.screen().cursor_position().1 != 0 {
            self.process(NEWLINE);
        }
        self.process(format!("{NOTE_START}{text}{NOTE_END}").as_bytes());
    }

    pub fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.process(bytes);
        std::mem::take(&mut self.parser.callbacks_mut().0)
    }

    fn resize(&mut self, size: TermSize) {
        let mut reflowed = parser(size.rows.max(1), size.cols.max(1));
        reflowed.process(&self.replay);
        reflowed.callbacks_mut().0.clear();
        self.parser = reflowed;
    }

    fn history(&mut self) -> usize {
        let screen = self.parser.screen_mut();
        screen.set_scrollback(usize::MAX);
        let rows = screen.scrollback();
        screen.set_scrollback(0);
        rows
    }

    fn line_count(&mut self) -> usize {
        self.history() + usize::from(self.parser.screen().size().0)
    }

    fn show_line(&mut self, line: usize, history: usize) -> u16 {
        self.parser
            .screen_mut()
            .set_scrollback(history.saturating_sub(line));
        u16::try_from(line.saturating_sub(history)).unwrap_or(u16::MAX)
    }

    pub fn export(&mut self) -> String {
        let mut lines = self.lines();
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.join("\n")
    }

    fn lines(&mut self) -> Vec<String> {
        let history = self.history();
        let mut lines = Vec::new();
        for line in 0..self.line_count() {
            let row = self.show_line(line, history);
            let screen = self.parser.screen();
            let text = screen
                .rows(0, screen.size().1)
                .nth(usize::from(row))
                .unwrap_or_default();
            lines.push(text);
        }
        self.parser.screen_mut().set_scrollback(0);
        lines
    }

    fn runs(&mut self, line: usize, history: usize) -> Vec<Run> {
        let row = self.show_line(line, history);
        let screen = self.parser.screen();
        let (cursor_row, cursor_col) = screen.cursor_position();
        let cursor =
            (self.open && !screen.hide_cursor() && line == history + usize::from(cursor_row))
                .then_some(cursor_col);
        let runs = row_runs(screen, row, cursor);
        self.parser.screen_mut().set_scrollback(0);
        runs
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Intensity {
    Normal,
    Bold,
    Dim,
}

impl Intensity {
    fn of(cell: &vt100::Cell) -> Self {
        if cell.bold() {
            Self::Bold
        } else if cell.dim() {
            Self::Dim
        } else {
            Self::Normal
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CellStyle {
    fg: vt100::Color,
    bg: vt100::Color,
    intensity: Intensity,
    italic: bool,
    underline: bool,
    inverse: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Run {
    text: String,
    cells: u16,
    wide: bool,
    style: CellStyle,
}

fn row_runs(screen: &vt100::Screen, row: u16, cursor: Option<u16>) -> Vec<Run> {
    let significant = |col: u16| {
        cursor == Some(col)
            || screen.cell(row, col).is_some_and(|cell| {
                !cell.contents().trim().is_empty()
                    || cell.bgcolor() != vt100::Color::Default
                    || cell.inverse()
                    || cell.underline()
            })
    };
    let width = (0..screen.size().1)
        .rev()
        .find(|col| significant(*col))
        .map_or(0, |col| col + 1);
    let mut runs: Vec<Run> = Vec::new();
    for col in 0..width {
        let Some(cell) = screen.cell(row, col) else {
            break;
        };
        if cell.is_wide_continuation() {
            continue;
        }
        let style = CellStyle {
            fg: cell.fgcolor(),
            bg: cell.bgcolor(),
            intensity: Intensity::of(cell),
            italic: cell.italic(),
            underline: cell.underline(),
            inverse: cell.inverse() != (cursor == Some(col)),
        };
        let text = if cell.has_contents() {
            cell.contents()
        } else {
            BLANK
        };
        let wide = cell.is_wide();
        match runs.last_mut() {
            Some(run) if run.style == style && !run.wide && !wide => {
                run.text.push_str(text);
                run.cells += 1;
            }
            _ => runs.push(Run {
                text: text.to_owned(),
                cells: if wide { WIDE_CELLS } else { 1 },
                wide,
                style,
            }),
        }
    }
    runs
}

fn run_colors(style: CellStyle, palette: &Palette) -> (gpui::Hsla, gpui::Hsla) {
    let fg = if style.intensity == Intensity::Dim && style.fg == vt100::Color::Default {
        palette.mute
    } else {
        terminal_color(style.fg, palette.fg, palette)
    };
    let bg = terminal_color(style.bg, palette.editor, palette);
    if style.inverse { (bg, fg) } else { (fg, bg) }
}

fn control_byte(key: &str) -> Option<u8> {
    match key {
        "space" => Some(0),
        "[" => Some(ESCAPE),
        _ => key
            .as_bytes()
            .first()
            .copied()
            .filter(|b| key.len() == 1 && b.is_ascii_lowercase())
            .map(|b| b - CTRL_OFFSET),
    }
}

fn named_key(key: &str, shift: bool) -> Option<&'static [u8]> {
    match key {
        "enter" => Some(b"\r"),
        "backspace" => Some(&[DELETE]),
        TAB if shift => Some(b"\x1b[Z"),
        TAB => Some(b"\t"),
        "escape" => Some(&[ESCAPE]),
        "space" => Some(b" "),
        "up" => Some(b"\x1b[A"),
        "down" => Some(b"\x1b[B"),
        "right" => Some(b"\x1b[C"),
        "left" => Some(b"\x1b[D"),
        "home" => Some(b"\x1b[H"),
        "end" => Some(b"\x1b[F"),
        "insert" => Some(b"\x1b[2~"),
        "delete" => Some(b"\x1b[3~"),
        "pageup" => Some(b"\x1b[5~"),
        "pagedown" => Some(b"\x1b[6~"),
        _ => None,
    }
}

fn is_cursor_key(key: &str) -> bool {
    matches!(key, "up" | "down" | "left" | "right" | "home" | "end")
}

fn input_bytes(keystroke: &Keystroke, application_cursor: bool) -> Option<Vec<u8>> {
    let modifiers = keystroke.modifiers;
    if modifiers.platform {
        return None;
    }
    let key = keystroke.key.as_str();
    let mut bytes = if modifiers.control {
        vec![control_byte(key)?]
    } else if let Some(named) = named_key(key, modifiers.shift) {
        let mut bytes = named.to_vec();
        if application_cursor && is_cursor_key(key) && bytes.get(1) == Some(&CSI) {
            bytes[1] = SS3;
        }
        bytes
    } else if let Some(text) = &keystroke.key_char {
        return Some(text.as_bytes().to_vec());
    } else if modifiers.alt && key.chars().count() == 1 {
        key.as_bytes().to_vec()
    } else {
        return None;
    };
    if modifiers.alt {
        bytes.insert(0, ESCAPE);
    }
    Some(bytes)
}

fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        format!("{PASTE_START}{text}{PASTE_END}").into_bytes()
    } else {
        text.as_bytes().to_vec()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    Input(Vec<u8>),
    Resize(TermSize),
}

pub fn grid(width: Pixels, height: Pixels, cell: Pixels) -> TermSize {
    let usable_width = f32::from(width) - 2. * f32::from(SPACE_6);
    let usable_height = f32::from(height) - 2. * f32::from(SPACE_3);
    let cols = (usable_width / f32::from(cell)).floor();
    let rows = (usable_height / f32::from(LOG_LINE)).floor();
    TermSize {
        cols: cells(cols),
        rows: cells(rows),
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "terminal columns stay far below f32 precision"
)]
fn cell_count(count: usize) -> f32 {
    count as f32
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to the u16 range first"
)]
fn cells(count: f32) -> u16 {
    if count.is_nan() {
        return 1;
    }
    count.clamp(1., f32::from(u16::MAX)) as u16
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct GridPoint {
    pub line: usize,
    pub col: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSelection {
    anchor: GridPoint,
    head: GridPoint,
}

impl GridSelection {
    pub fn new(anchor: GridPoint, head: GridPoint) -> Self {
        Self { anchor, head }
    }

    fn ordered(self) -> (GridPoint, GridPoint) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.head
    }

    pub fn columns(self, line: usize, width: usize) -> Option<Range<usize>> {
        let (start, end) = self.ordered();
        if self.is_empty() || line < start.line || line > end.line {
            return None;
        }
        let from = if line == start.line { start.col } else { 0 };
        let to = if line == end.line { end.col } else { width };
        (from < to).then_some(from..to)
    }

    pub fn text(self, lines: &[String]) -> String {
        let (start, end) = self.ordered();
        lines
            .iter()
            .enumerate()
            .take(end.line + 1)
            .skip(start.line)
            .map(|(line, text)| {
                let width = text.chars().count();
                let columns = self.columns(line, width).unwrap_or_default();
                let picked: String = text
                    .chars()
                    .skip(columns.start)
                    .take(columns.len())
                    .collect();
                picked.trim_end().to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub fn column_at(x: Pixels, left: Pixels, advance: Pixels) -> usize {
    let offset = (f32::from(x - left - SPACE_6) / f32::from(advance)).round();
    if offset.is_nan() || offset <= 0. {
        return 0;
    }
    cells(offset).into()
}

pub struct Terminal {
    id: SharedString,
    buffer: TerminalBuffer,
    size: Option<TermSize>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    left: Pixels,
    advance: Pixels,
    selection: Option<GridSelection>,
    selecting: bool,
}

impl EventEmitter<TerminalEvent> for Terminal {}

impl Terminal {
    pub fn pane(view: &Entity<Self>) -> ViewElement<Entity<Self>> {
        view.clone().cached(StyleRefinement::default().size_full())
    }

    pub fn new(id: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            id: id.into(),
            buffer: TerminalBuffer::default(),
            size: None,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            left: Pixels::ZERO,
            advance: TEXT_MONO,
            selection: None,
            selecting: false,
        }
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.buffer = TerminalBuffer::default();
        if let Some(size) = self.size {
            self.buffer.resize(size);
        }
        cx.notify();
    }

    pub fn copy(&mut self, cx: &mut Context<Self>) {
        let text = match self.selection.filter(|selection| !selection.is_empty()) {
            Some(selection) => selection.text(&self.buffer.lines()),
            None => self.buffer.export(),
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    pub fn select(&mut self, selection: Option<GridSelection>, cx: &mut Context<Self>) {
        self.selection = selection;
        cx.notify();
    }

    fn point(&self, line: usize, x: Pixels) -> GridPoint {
        GridPoint {
            line,
            col: column_at(x, self.left, self.advance),
        }
    }

    fn press(&mut self, line: usize, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let at = self.point(line, event.position.x);
        let anchor = match self.selection {
            Some(selection) if event.modifiers.shift => selection.anchor,
            _ => at,
        };
        self.selection = Some(GridSelection::new(anchor, at));
        self.selecting = true;
        cx.notify();
    }

    fn drag(&mut self, line: usize, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let head = self.point(line, event.position.x);
        if let Some(selection) = self.selection.as_mut() {
            selection.head = head;
            cx.notify();
        }
    }

    pub fn export(&mut self) -> String {
        self.buffer.export()
    }

    pub fn note(&mut self, text: impl AsRef<str>, cx: &mut Context<Self>) {
        self.buffer.note(text.as_ref());
        self.after_change(cx);
    }

    pub fn feed(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        let replies = self.buffer.feed(bytes);
        if self.buffer.is_open() && !replies.is_empty() {
            cx.emit(TerminalEvent::Input(replies));
        }
        self.after_change(cx);
    }

    pub fn line(&mut self, text: impl AsRef<str>, cx: &mut Context<Self>) {
        self.feed(text.as_ref().as_bytes(), cx);
        self.feed(NEWLINE, cx);
    }

    pub fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.buffer.set_open(open);
        cx.notify();
    }

    pub fn is_open(&self) -> bool {
        self.buffer.is_open()
    }

    pub fn size(&self) -> Option<TermSize> {
        self.size
    }

    fn measured(&mut self, size: TermSize, left: Pixels, cx: &mut Context<Self>) {
        self.left = left;
        if self.size != Some(size) {
            self.size = Some(size);
            self.buffer.resize(size);
            cx.notify();
            cx.emit(TerminalEvent::Resize(size));
        }
    }

    fn after_change(&mut self, cx: &mut Context<Self>) {
        if self.scroll.is_scrolled_to_end() != Some(false) {
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let bracketed = self.buffer.parser.screen().bracketed_paste();
        cx.emit(TerminalEvent::Input(paste_bytes(&text, bracketed)));
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let platform = keystroke.modifiers.platform;
        if platform && keystroke.key == "c" {
            self.copy(cx);
            cx.stop_propagation();
            return;
        }
        if !self.buffer.is_open() {
            return;
        }
        if platform && keystroke.key == "v" {
            self.paste(cx);
            cx.stop_propagation();
            return;
        }
        let application_cursor = self.buffer.parser.screen().application_cursor();
        if let Some(bytes) = input_bytes(keystroke, application_cursor) {
            self.selection = None;
            cx.emit(TerminalEvent::Input(bytes));
            cx.stop_propagation();
        }
    }

    fn send_tab(&mut self, shift: bool, cx: &mut Context<Self>) {
        if let Some(bytes) = named_key(TAB, shift).filter(|_| self.buffer.is_open()) {
            cx.emit(TerminalEvent::Input(bytes.to_vec()));
        }
    }

    fn render_lines(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let palette = *cx.palette();
        let font_id = window.text_system().resolve_font(&font(FONT_MONO));
        let advance = window
            .text_system()
            .em_advance(font_id, TEXT_MONO)
            .unwrap_or(TEXT_MONO);
        self.advance = advance;
        let history = self.buffer.history();
        let width = self.size.map_or(DEFAULT_COLS, |size| size.cols).into();
        range
            .map(|line| {
                let highlight = self
                    .selection
                    .and_then(|selection| selection.columns(line, width))
                    .map(|columns| {
                        div()
                            .absolute()
                            .top_0()
                            .left(SPACE_6 + advance * cell_count(columns.start))
                            .w(advance * cell_count(columns.len()))
                            .h(LOG_LINE)
                            .bg(palette.sel)
                    });
                let runs = self.buffer.runs(line, history).into_iter().map(|run| {
                    let (fg, bg) = run_colors(run.style, &palette);
                    div()
                        .flex_none()
                        .w(advance * f32::from(run.cells))
                        .h(LOG_LINE)
                        .when(bg != palette.editor, |cell| cell.bg(bg))
                        .text_color(fg)
                        .when(run.style.intensity == Intensity::Bold, |style| {
                            style.font_weight(FontWeight::BOLD)
                        })
                        .when(run.style.italic, Styled::italic)
                        .when(run.style.underline, Styled::underline)
                        .child(run.text)
                });
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .h(LOG_LINE)
                    .px(SPACE_6)
                    .whitespace_nowrap()
                    .cursor(CursorStyle::IBeam)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event, _, cx| this.press(line, event, cx)),
                    )
                    .on_mouse_move(
                        cx.listener(move |this, event, _, cx| this.drag(line, event, cx)),
                    )
                    .children(highlight)
                    .children(runs)
                    .into_any_element()
            })
            .collect()
    }
}

impl Focusable for Terminal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Terminal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let font_id = window.text_system().resolve_font(&font(FONT_MONO));
        let cell = window.text_system().em_advance(font_id, TEXT_MONO).ok();
        let this = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                let Some(cell) = cell else {
                    return;
                };
                let size = grid(bounds.size.width, bounds.size.height, cell);
                let left = bounds.origin.x;
                this.update(cx, |terminal, cx| terminal.measured(size, left, cx))
                    .ok();
            },
            |_, (), _, _| {},
        )
        .absolute()
        .size_full();
        let list = uniform_list(
            ElementId::Name(self.id.clone()),
            self.buffer.line_count(),
            cx.processor(Self::render_lines),
        )
        .track_scroll(&self.scroll)
        .flex_1()
        .min_h_0();
        div()
            .id(ElementId::NamedInteger(self.id.clone(), 0))
            .key_context(TERMINAL_CONTEXT)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .on_action(cx.listener(|this, _: &SendTab, _, cx| this.send_tab(false, cx)))
            .on_action(cx.listener(|this, _: &SendBackTab, _, cx| this.send_tab(true, cx)))
            .on_click(cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.selecting = false),
            )
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .py(SPACE_3)
            .bg(palette.editor)
            .font_family(FONT_MONO)
            .text_size(TEXT_MONO)
            .child(measure)
            .child(list)
    }
}

#[cfg(test)]
mod tests {
    use gpui::px;

    use super::*;

    fn texts(buffer: &mut TerminalBuffer) -> Vec<String> {
        buffer.export().lines().map(str::to_owned).collect()
    }

    fn key(source: &str) -> Keystroke {
        Keystroke::parse(source).map_or_else(
            |_| panic!("{source} is not a keystroke"),
            Keystroke::with_simulated_ime,
        )
    }

    fn bytes(source: &str, application_cursor: bool) -> Option<Vec<u8>> {
        input_bytes(&key(source), application_cursor)
    }

    #[test]
    fn grid_fits_whole_cells_inside_the_padding() {
        let width = SPACE_6 * 2. + px(7.5) * 80.;
        let height = SPACE_3 * 2. + LOG_LINE * 24.;
        assert_eq!(
            grid(width + px(5.), height + px(3.), px(7.5)),
            TermSize { cols: 80, rows: 24 }
        );
        assert_eq!(grid(px(0.), px(0.), px(7.5)), TermSize { cols: 1, rows: 1 });
        assert_eq!(grid(width, height, px(0.)).rows, 24);
    }

    #[test]
    fn printable_named_control_and_alt_keys_encode_as_tty_bytes() {
        assert_eq!(bytes("a", false), Some(b"a".to_vec()));
        assert_eq!(bytes("shift-a", false), Some(b"A".to_vec()));
        assert_eq!(bytes("enter", false), Some(b"\r".to_vec()));
        assert_eq!(bytes("backspace", false), Some(vec![DELETE]));
        assert_eq!(bytes("tab", false), Some(b"\t".to_vec()));
        assert_eq!(bytes("shift-tab", false), Some(b"\x1b[Z".to_vec()));
        assert_eq!(bytes("space", false), Some(b" ".to_vec()));
        assert_eq!(bytes("escape", false), Some(vec![ESCAPE]));
        assert_eq!(bytes("delete", false), Some(b"\x1b[3~".to_vec()));
        assert_eq!(bytes("ctrl-c", false), Some(vec![3]));
        assert_eq!(bytes("ctrl-d", false), Some(vec![4]));
        assert_eq!(bytes("ctrl-space", false), Some(vec![0]));
        assert_eq!(bytes("ctrl-[", false), Some(vec![ESCAPE]));
        assert_eq!(bytes("alt-b", false), Some(b"\x1bb".to_vec()));
        assert_eq!(bytes("cmd-k", false), None);
    }

    #[test]
    fn arrows_follow_the_application_cursor_mode() {
        assert_eq!(bytes("up", false), Some(b"\x1b[A".to_vec()));
        assert_eq!(bytes("left", false), Some(b"\x1b[D".to_vec()));
        assert_eq!(bytes("up", true), Some(b"\x1bOA".to_vec()));
        assert_eq!(bytes("home", true), Some(b"\x1bOH".to_vec()));
        assert_eq!(bytes("pageup", true), Some(b"\x1b[5~".to_vec()));
    }

    #[test]
    fn paste_is_bracketed_only_when_the_program_asks() {
        assert_eq!(paste_bytes("one\ntwo", true), b"\x1b[200~one\ntwo\x1b[201~");
        assert_eq!(paste_bytes("one\ntwo", false), b"one\ntwo");
    }

    #[test]
    fn carriage_returns_backspace_and_line_edits_redraw_in_place() {
        let mut buffer = TerminalBuffer::default();
        buffer.feed(b"/ # ls\r\napp.yaml\r\n/ # ");
        assert_eq!(texts(&mut buffer), ["/ # ls", "app.yaml", "/ # "]);
        buffer.feed(b"lx\x08 \x08s");
        buffer.feed(b"\r\nold prompt\rnew\x1b[K");
        assert_eq!(texts(&mut buffer), ["/ # ls", "app.yaml", "/ # ls", "new"]);
    }

    #[test]
    fn escape_sequences_style_cells_instead_of_printing() {
        let mut buffer = TerminalBuffer::default();
        buffer.feed(b"\x1b[1;31mred\x1b[0m \x1b]0;title\x07done");
        assert_eq!(texts(&mut buffer), ["red done"]);
        let runs = buffer.runs(0, 0);
        let texts: Vec<&str> = runs.iter().map(|run| run.text.as_str()).collect();
        assert_eq!(texts, ["red", " done"]);
        assert_eq!(runs[0].style.fg, vt100::Color::Idx(1));
        assert_eq!(runs[0].style.intensity, Intensity::Bold);
        assert_eq!(runs[1].style.fg, vt100::Color::Default);
    }

    #[test]
    fn the_cursor_cell_renders_inverted_while_open() {
        let mut buffer = TerminalBuffer::default();
        buffer.feed(b"$ ");
        assert!(buffer.runs(0, 0).iter().all(|run| !run.style.inverse));
        buffer.set_open(true);
        let runs = buffer.runs(0, 0);
        let cursor: Vec<(&str, bool)> = runs
            .iter()
            .map(|run| (run.text.as_str(), run.style.inverse))
            .collect();
        assert_eq!(cursor, [("$ ", false), (" ", true)]);
        buffer.feed(b"\x1b[?25l");
        assert!(buffer.runs(0, 0).iter().all(|run| !run.style.inverse));
    }

    #[test]
    fn utf8_split_across_chunks_and_wide_characters_stay_whole() {
        let mut buffer = TerminalBuffer::default();
        let bullet = "•".as_bytes();
        buffer.feed(&bullet[..1]);
        buffer.feed(&bullet[1..]);
        buffer.feed("界x".as_bytes());
        assert_eq!(texts(&mut buffer), ["•界x"]);
        let runs = buffer.runs(0, 0);
        let cells: Vec<(&str, u16)> = runs
            .iter()
            .map(|run| (run.text.as_str(), run.cells))
            .collect();
        assert_eq!(cells, [("•", 1), ("界", 2), ("x", 1)]);
    }

    #[test]
    fn notes_are_muted_lines_in_order_with_the_output() {
        let mut buffer = TerminalBuffer::default();
        buffer.note("$ kubectl exec");
        buffer.feed(b"/ # exit");
        buffer.note("Session ended.");
        assert_eq!(
            texts(&mut buffer),
            ["$ kubectl exec", "/ # exit", "Session ended."]
        );
        let note = buffer.runs(2, 0);
        assert!(note.iter().all(|run| run.style.intensity == Intensity::Dim));
        let palette = Palette::dark();
        assert_eq!(run_colors(note[0].style, &palette).0, palette.mute);
    }

    #[test]
    fn alternate_screen_programs_restore_the_shell_on_exit() {
        let mut buffer = TerminalBuffer::default();
        buffer.feed(b"red\r\nshell\x1b[2DXY");
        assert_eq!(texts(&mut buffer), ["red", "sheXY"]);
        buffer.feed(b"\x1b[?1049h\x1b[2J\x1b[Htool");
        assert!(buffer.parser.screen().alternate_screen());
        assert_eq!(texts(&mut buffer), ["tool"]);
        buffer.feed(b"\x1b[?1049l");
        assert_eq!(texts(&mut buffer), ["red", "sheXY"]);
    }

    #[test]
    fn scrollback_keeps_lines_that_left_the_screen_and_resize_reflows_the_grid() {
        let mut buffer = TerminalBuffer::default();
        buffer.resize(TermSize { rows: 2, cols: 10 });
        buffer.feed(b"one\r\ntwo\r\nthree");
        assert_eq!(buffer.history(), 1);
        assert_eq!(buffer.line_count(), 3);
        assert_eq!(buffer.export(), "one\ntwo\nthree");
        let first: Vec<String> = buffer.runs(0, 1).into_iter().map(|run| run.text).collect();
        assert_eq!(first, ["one"]);
        assert_eq!(buffer.parser.screen().scrollback(), 0);
        buffer.resize(TermSize { rows: 3, cols: 20 });
        assert_eq!(buffer.parser.screen().size(), (3, 20));
        assert!(buffer.export().contains("three"));
    }

    #[test]
    fn long_lines_wrap_again_when_the_terminal_narrows_or_widens() {
        let mut buffer = TerminalBuffer::default();
        buffer.resize(TermSize { rows: 5, cols: 30 });
        buffer.note("note");
        buffer.feed(&[b'A'; 50]);
        buffer.feed(b"\r\n$ \x1b[6n");
        buffer.resize(TermSize { rows: 5, cols: 20 });
        assert_eq!(
            texts(&mut buffer),
            [
                "note",
                &"A".repeat(20),
                &"A".repeat(20),
                &"A".repeat(10),
                "$ "
            ]
        );
        assert_eq!(buffer.parser.callbacks_mut().0, Vec::<u8>::new());
        buffer.resize(TermSize { rows: 5, cols: 60 });
        assert_eq!(texts(&mut buffer), ["note", &"A".repeat(50), "$ "]);
    }

    #[test]
    fn cursor_position_and_device_queries_are_answered() {
        let mut buffer = TerminalBuffer::default();
        assert_eq!(buffer.feed(b"~ $ \x1b[6n"), b"\x1b[1;5R");
        assert_eq!(buffer.feed(b"\x1b[5n\x1b[c"), b"\x1b[0n\x1b[?1;2c");
        assert_eq!(buffer.feed(b"\x1b[31mplain"), b"");
        assert_eq!(texts(&mut buffer), ["~ $ plain"]);
    }

    #[test]
    fn busybox_ash_redraws_from_a_kind_pod_render_as_the_shell_drew_them() {
        let mut buffer = TerminalBuffer::default();
        buffer.feed(b"~ $ \x1b[6n");
        buffer.feed(b"ech");
        buffer.feed(b"\r~ $ \x1b[Jech\r~ $ echo \x1b[J");
        assert_eq!(texts(&mut buffer), ["~ $ echo "]);
        buffer.feed(b"TAB-OK\r\nTAB-OK\r\n~ $ ");
        buffer.feed(b"printf '%s\\n' ac\x08bc\x08c\r\nabc\r\n~ $ ");
        buffer.feed(b"sleep 30\r\n^C\r\n~ $ ");
        assert_eq!(
            texts(&mut buffer),
            [
                "~ $ echo TAB-OK",
                "TAB-OK",
                "~ $ printf '%s\\n' abc",
                "abc",
                "~ $ sleep 30",
                "^C",
                "~ $ "
            ]
        );
    }

    fn at(line: usize, col: usize) -> GridPoint {
        GridPoint { line, col }
    }

    #[test]
    fn a_selection_spans_from_the_anchor_to_the_head_in_either_direction() {
        let lines: Vec<String> = ["$ kubectl get pods", "", "NAME   READY", "api-0  1/1"]
            .iter()
            .map(|line| (*line).to_owned())
            .collect();
        let down = GridSelection::new(at(0, 2), at(2, 4));
        assert_eq!(down.text(&lines), "kubectl get pods\n\nNAME");
        let up = GridSelection::new(at(2, 4), at(0, 2));
        assert_eq!(up.text(&lines), down.text(&lines));
        assert_eq!(down.columns(0, 80), Some(2..80));
        assert_eq!(down.columns(1, 80), Some(0..80));
        assert_eq!(down.columns(2, 80), Some(0..4));
        assert_eq!(down.columns(3, 80), None);
        let click = GridSelection::new(at(1, 3), at(1, 3));
        assert!(click.is_empty());
        assert_eq!(click.columns(1, 80), None);
    }

    #[test]
    fn columns_round_to_the_nearest_cell_boundary() {
        let left = px(100.);
        let advance = px(8.);
        assert_eq!(column_at(left, left, advance), 0);
        assert_eq!(column_at(left + SPACE_6 + px(11.), left, advance), 1);
        assert_eq!(column_at(left + SPACE_6 + px(13.), left, advance), 2);
    }

    #[gpui::test]
    fn copy_takes_the_selection_when_there_is_one(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext as _;

        let terminal = cx.new(|cx| Terminal::new("select-terminal", cx));
        terminal.update(cx, |terminal, cx| {
            terminal.feed(b"first line\r\nsecond line", cx);
            terminal.select(Some(GridSelection::new(at(0, 6), at(1, 6))), cx);
            terminal.copy(cx);
        });
        let text = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
        assert_eq!(text.as_deref(), Some("line\nsecond"));
    }

    #[gpui::test]
    fn copy_exports_text_without_ansi_sequences(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext as _;

        let terminal = cx.new(|cx| Terminal::new("copy-terminal", cx));
        terminal.update(cx, |terminal, cx| {
            terminal.feed(b"\x1b[31mred\x1b[0m\r\nplain", cx);
            terminal.copy(cx);
        });
        let text = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
        assert_eq!(text.as_deref(), Some("red\nplain"));
    }
}
