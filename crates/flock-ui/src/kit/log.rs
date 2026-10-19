use std::collections::VecDeque;
use std::ops::Range;

use gpui::{
    AnyElement, Context, ElementId, HighlightStyle, Hsla, InteractiveElement, IntoElement,
    ListAlignment, ListState, ParentElement, Pixels, Render, ScrollStrategy, SharedString, Styled,
    UniformListScrollHandle, Window, div, list, prelude::FluentBuilder, px, uniform_list,
};

use super::selectable::SelectableText;
use crate::highlight::{SOURCE_COLORS, source_colors};
use crate::theme::{
    ActiveTheme, FONT_MONO, LOG_LEVEL, LOG_LINE, LOG_SOURCE, LOG_TIMESTAMP, Palette, SPACE_3,
    SPACE_6, TEXT_MONO, TEXT_UI,
};

pub const LOG_CAPACITY: usize = 10_000;
pub const LOG_FILTER_PLACEHOLDER: &str = "Filter log lines";
const NO_LINES: &str = "No log lines yet.";
const ESC: char = '\u{1b}';
const BEL: char = '\u{7}';
const CSI_FINAL: std::ops::RangeInclusive<char> = '\u{40}'..='\u{7e}';
const TIMESTAMP_DATE_DASH: usize = 4;
const TIMESTAMP_MIN_LEN: usize = 20;
const WRAP_OVERDRAW: Pixels = px(200.);
const LINE_ID: &str = "log-line";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

impl Level {
    fn parse(word: &str) -> Option<Self> {
        match word.trim_matches(|c: char| !c.is_ascii_alphabetic()) {
            "DEBUG" => Some(Self::Debug),
            "debug" => Some(Self::Debug),
            "INFO" => Some(Self::Info),
            "info" => Some(Self::Info),
            "WARN" => Some(Self::Warn),
            "WARNING" => Some(Self::Warn),
            "warn" => Some(Self::Warn),
            "ERROR" => Some(Self::Error),
            "error" => Some(Self::Error),
            "FATAL" => Some(Self::Fatal),
            "fatal" => Some(Self::Fatal),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
            Self::Fatal => "FATAL",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineSource {
    pub label: SharedString,
    pub slot: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    pub timestamp: Option<SharedString>,
    pub level: Option<Level>,
    pub message: SharedString,
    pub source: Option<LineSource>,
}

impl LogLine {
    pub fn new(
        timestamp: Option<&str>,
        level: Option<Level>,
        message: impl Into<SharedString>,
    ) -> Self {
        Self {
            timestamp: timestamp.map(|t| SharedString::from(t.to_owned())),
            level,
            message: message.into(),
            source: None,
        }
    }

    pub fn with_source(mut self, label: impl Into<SharedString>, slot: usize) -> Self {
        self.source = Some(LineSource {
            label: label.into(),
            slot,
        });
        self
    }

    pub fn text(&self) -> String {
        let level = self.level.map(Level::label);
        let source = self.source.as_ref().map(|source| source.label.as_ref());
        let parts: Vec<&str> = self
            .timestamp
            .as_deref()
            .into_iter()
            .chain(source)
            .chain(level)
            .chain(std::iter::once(self.message.as_ref()))
            .collect();
        parts.join(" ")
    }

    pub fn parse(raw: &str) -> Self {
        let clean = strip_ansi(raw);
        let (timestamp, rest) = split_timestamp(&clean);
        let first_word = rest.split_whitespace().next().unwrap_or_default();
        let level = Level::parse(first_word);
        let message = match level {
            Some(_) => {
                let trimmed = rest.trim_start();
                trimmed
                    .strip_prefix(first_word)
                    .unwrap_or(trimmed)
                    .trim_start()
            }
            None => rest,
        };
        Self {
            timestamp: timestamp.map(|t| SharedString::from(t.to_owned())),
            level,
            message: SharedString::from(message.to_owned()),
            source: None,
        }
    }
}

fn split_timestamp(line: &str) -> (Option<&str>, &str) {
    let Some((first, rest)) = line.split_once(' ') else {
        return (None, line);
    };
    let looks_like_rfc3339 = first.len() >= TIMESTAMP_MIN_LEN
        && first.as_bytes().get(TIMESTAMP_DATE_DASH) == Some(&b'-')
        && first.contains('T')
        && first.ends_with('Z');
    if looks_like_rfc3339 {
        (Some(first), rest)
    } else {
        (None, line)
    }
}

pub fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != ESC {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if CSI_FINAL.contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == BEL {
                        break;
                    }
                    if c == ESC && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            Some(_) => {}
            None => {}
        }
    }
    out
}

#[derive(Debug)]
pub struct LogBuffer {
    lines: VecDeque<LogLine>,
    capacity: usize,
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self::with_capacity(LOG_CAPACITY)
    }
}

impl LogBuffer {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            capacity,
        }
    }

    pub fn push(&mut self, line: LogLine) {
        if self.capacity == 0 {
            return;
        }
        if self.lines.len() == self.capacity {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn get(&self, ix: usize) -> Option<&LogLine> {
        self.lines.get(ix)
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }
}

pub fn find_matches(text: &str, query: &str) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let lower_text = text.to_lowercase();
    let lower_query = query.to_lowercase();
    if lower_text.len() != text.len() {
        return text
            .match_indices(query)
            .map(|(at, m)| at..at + m.len())
            .collect();
    }
    lower_text
        .match_indices(&lower_query)
        .map(|(at, m)| at..at + m.len())
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Levels {
    Column,
    Tint,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct LineColors {
    level: Hsla,
    message: Hsla,
    row: Option<Hsla>,
}

fn line_colors(level: Option<Level>, levels: Levels, palette: &Palette) -> LineColors {
    let (level_color, row) = match level {
        Some(Level::Error) => (palette.err, Some(palette.errbg)),
        Some(Level::Fatal) => (palette.err, Some(palette.errbg)),
        Some(Level::Warn) => (palette.warn, None),
        Some(Level::Debug) => (palette.mute, None),
        Some(Level::Info) => (palette.acc, None),
        None => (palette.mute, None),
    };
    let message = match level {
        Some(_) if levels == Levels::Tint => level_color,
        Some(_) => palette.fg,
        None => palette.fg,
    };
    LineColors {
        level: level_color,
        message,
        row,
    }
}

pub struct LogView {
    id: SharedString,
    buffer: LogBuffer,
    follow: bool,
    timestamps: bool,
    timestamp_width: Pixels,
    levels: Levels,
    inset: Pixels,
    query: SharedString,
    scroll: UniformListScrollHandle,
    wrap: bool,
    wrapped: ListState,
    sources: Option<(Palette, [Hsla; SOURCE_COLORS])>,
}

impl LogView {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            buffer: LogBuffer::default(),
            follow: true,
            timestamps: true,
            timestamp_width: LOG_TIMESTAMP,
            levels: Levels::Column,
            inset: SPACE_6,
            query: SharedString::default(),
            scroll: UniformListScrollHandle::new(),
            wrap: false,
            wrapped: ListState::new(0, ListAlignment::Top, WRAP_OVERDRAW),
            sources: None,
        }
    }

    pub fn set_lines(&mut self, lines: impl IntoIterator<Item = LogLine>, cx: &mut Context<Self>) {
        self.buffer.clear();
        self.extend(lines, cx);
    }

    pub fn set_highlight(&mut self, query: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.query = query.into();
        cx.notify();
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn line(&self, ix: usize) -> Option<&LogLine> {
        self.buffer.get(ix)
    }

    fn source_colors(&mut self, palette: &Palette) -> [Hsla; SOURCE_COLORS] {
        match &self.sources {
            Some((cached, colors)) if cached == palette => *colors,
            _ => {
                let colors = source_colors(palette);
                self.sources = Some((*palette, colors));
                colors
            }
        }
    }

    pub fn wrap(&self) -> bool {
        self.wrap
    }

    pub fn set_wrap(&mut self, wrap: bool, cx: &mut Context<Self>) {
        self.wrap = wrap;
        self.sync_wrapped();
        cx.notify();
    }

    fn sync_wrapped(&mut self) {
        if !self.wrap {
            return;
        }
        self.wrapped.reset(self.buffer.len());
        if self.follow {
            self.wrapped.scroll_to_end();
        }
    }

    pub fn plain(mut self) -> Self {
        self.levels = Levels::Tint;
        self.timestamps = false;
        self
    }

    pub fn timestamp_width(mut self, width: Pixels) -> Self {
        self.timestamp_width = width;
        self
    }

    pub fn inset(mut self, inset: Pixels) -> Self {
        self.inset = inset;
        self
    }

    pub fn push(&mut self, line: LogLine, cx: &mut Context<Self>) {
        self.buffer.push(line);
        self.after_append(cx);
    }

    pub fn extend(&mut self, lines: impl IntoIterator<Item = LogLine>, cx: &mut Context<Self>) {
        for line in lines {
            self.buffer.push(line);
        }
        self.after_append(cx);
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.buffer.clear();
        self.sync_wrapped();
        cx.notify();
    }

    pub fn follow(&self) -> bool {
        self.follow
    }

    pub fn set_follow(&mut self, follow: bool, cx: &mut Context<Self>) {
        self.follow = follow;
        if follow {
            self.scroll.scroll_to_bottom();
            self.sync_wrapped();
        }
        cx.notify();
    }

    pub fn timestamps(&self) -> bool {
        self.timestamps
    }

    pub fn export(&self) -> String {
        let mut out = String::new();
        for line in &self.buffer.lines {
            out.push_str(&line.text());
            out.push('\n');
        }
        out
    }

    pub fn set_timestamps(&mut self, show: bool, cx: &mut Context<Self>) {
        self.timestamps = show;
        cx.notify();
    }

    pub fn set_query(&mut self, query: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.query = query.into();
        if let Some(first) = self.first_match() {
            self.follow = false;
            self.scroll.scroll_to_item(first, ScrollStrategy::Center);
        }
        cx.notify();
    }

    fn first_match(&self) -> Option<usize> {
        if self.query.is_empty() {
            return None;
        }
        (0..self.buffer.len()).find(|&ix| {
            self.buffer
                .get(ix)
                .is_some_and(|line| !find_matches(&line.message, &self.query).is_empty())
        })
    }

    fn after_append(&mut self, cx: &mut Context<Self>) {
        if self.follow {
            self.scroll.scroll_to_bottom();
        }
        self.sync_wrapped();
        cx.notify();
    }

    fn render_lines(
        &mut self,
        range: Range<usize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let palette = *cx.palette();
        let sources = self.source_colors(&palette);
        range
            .filter_map(|ix| {
                self.buffer
                    .get(ix)
                    .map(|line| self.render_line(ix, line, &palette, &sources))
            })
            .collect()
    }

    fn render_line(
        &self,
        ix: usize,
        line: &LogLine,
        palette: &Palette,
        sources: &[Hsla; SOURCE_COLORS],
    ) -> AnyElement {
        let colors = line_colors(line.level, self.levels, palette);
        let source = line.source.as_ref().map(|source| {
            let color = sources
                .get(source.slot % SOURCE_COLORS)
                .copied()
                .unwrap_or(palette.fg2);
            div()
                .w(LOG_SOURCE)
                .flex_none()
                .truncate()
                .text_color(color)
                .child(source.label.clone())
        });
        let highlight = HighlightStyle {
            background_color: Some(palette.accbg),
            color: Some(palette.fg),
            ..HighlightStyle::default()
        };
        let matches = find_matches(&line.message, &self.query);
        let id = ElementId::NamedInteger(LINE_ID.into(), ix as u64);
        let message = SelectableText::new(line.message.clone())
            .id(id)
            .highlights(matches.into_iter().map(|range| (range, highlight)));
        div()
            .flex()
            .gap(SPACE_6)
            .px(self.inset)
            .when(self.wrap, |row| row.min_h(LOG_LINE).line_height(LOG_LINE))
            .when(!self.wrap, |row| {
                row.h(LOG_LINE).w_full().whitespace_nowrap()
            })
            .when_some(colors.row, Styled::bg)
            .when(self.timestamps, |row| {
                row.child(
                    div()
                        .flex_none()
                        .min_w(self.timestamp_width)
                        .text_color(palette.mute)
                        .children(line.timestamp.clone()),
                )
            })
            .children(source)
            .when(self.levels == Levels::Column, |row| {
                row.child(
                    div()
                        .w(LOG_LEVEL)
                        .flex_none()
                        .text_color(colors.level)
                        .children(line.level.map(Level::label)),
                )
            })
            .child(
                div()
                    .when(self.wrap, |text| text.flex_1().min_w_0())
                    .text_color(colors.message)
                    .child(message),
            )
            .into_any_element()
    }
}

impl Render for LogView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let lines = if self.wrap {
            list(
                self.wrapped.clone(),
                cx.processor(|this, ix: usize, _, cx| {
                    let palette = *cx.palette();
                    let sources = this.source_colors(&palette);
                    this.buffer.get(ix).map_or_else(
                        || div().into_any_element(),
                        |line| this.render_line(ix, line, &palette, &sources),
                    )
                }),
            )
            .flex_1()
            .min_h_0()
            .py(SPACE_3)
            .into_any_element()
        } else {
            uniform_list(
                ElementId::Name(self.id.clone()),
                self.buffer.len(),
                cx.processor(Self::render_lines),
            )
            .track_scroll(&self.scroll)
            .flex_1()
            .min_h_0()
            .py(SPACE_3)
            .into_any_element()
        };
        let selector = self.id.to_string();
        div()
            .id(ElementId::NamedInteger(self.id.clone(), 0))
            .debug_selector(|| selector)
            .size_full()
            .flex()
            .flex_col()
            .bg(palette.editor)
            .font_family(FONT_MONO)
            .text_size(TEXT_MONO)
            .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                let at_end = if this.wrap {
                    this.wrapped.is_scrolled_to_end()
                } else {
                    this.scroll.is_scrolled_to_end()
                };
                if this.follow && at_end == Some(false) {
                    this.follow = false;
                    cx.notify();
                }
            }))
            .when(self.buffer.is_empty(), |view| {
                view.child(
                    div()
                        .px(SPACE_6)
                        .py(SPACE_3)
                        .font_family(crate::theme::FONT_SANS)
                        .text_size(TEXT_UI)
                        .text_color(palette.mute)
                        .child(NO_LINES),
                )
            })
            .child(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_export_as_plain_text() {
        let full = LogLine::new(Some("21:12:20.231"), Some(Level::Fatal), "exit 1");
        assert_eq!(full.text(), "21:12:20.231 FATAL exit 1");
        let tagged = full.clone().with_source("x2lqp", 3);
        assert_eq!(tagged.text(), "21:12:20.231 x2lqp FATAL exit 1");
        assert_eq!(
            LogLine::new(None, None, "── exited ──").text(),
            "── exited ──"
        );
    }

    #[test]
    fn strips_colour_cursor_and_title_sequences() {
        let raw = "\u{1b}[31mERROR\u{1b}[0m boom \u{1b}[2K\u{1b}]0;title\u{7}done \u{1b}]8;;x\u{1b}\\link";
        assert_eq!(strip_ansi(raw), "ERROR boom done link");
        assert_eq!(strip_ansi("plain ✕ text"), "plain ✕ text");
        assert_eq!(strip_ansi("trailing \u{1b}"), "trailing ");
    }

    #[test]
    fn buffer_drops_oldest_lines_past_capacity() {
        let mut buffer = LogBuffer::with_capacity(3);
        for n in 0..5 {
            buffer.push(LogLine::new(None, None, n.to_string()));
        }
        assert_eq!(buffer.len(), 3);
        let first = buffer.get(0).map(|line| line.message.to_string());
        assert_eq!(first.as_deref(), Some("2"));
    }

    #[test]
    fn default_buffer_is_bounded_by_the_constant() {
        let mut buffer = LogBuffer::default();
        for _ in 0..=LOG_CAPACITY {
            buffer.push(LogLine::new(None, None, "x"));
        }
        assert_eq!(buffer.len(), LOG_CAPACITY);
    }

    #[test]
    fn parses_kubectl_timestamps_and_levels() {
        let line = LogLine::parse(
            "2026-10-12T21:12:20.231Z \u{1b}[31mERROR\u{1b}[0m open stripe.env: no such file",
        );
        assert_eq!(line.timestamp.as_deref(), Some("2026-10-12T21:12:20.231Z"));
        assert_eq!(line.level, Some(Level::Error));
        assert_eq!(line.message.as_ref(), "open stripe.env: no such file");

        let bare = LogLine::parse("GET /healthz 200 0.3ms");
        assert_eq!(bare.timestamp, None);
        assert_eq!(bare.level, None);
        assert_eq!(bare.message.as_ref(), "GET /healthz 200 0.3ms");
    }

    #[test]
    fn plain_lines_take_the_level_colour() {
        let palette = Palette::dark();
        let leveled = line_colors(Some(Level::Error), Levels::Column, &palette);
        assert_eq!(leveled.message, palette.fg);
        assert_eq!(leveled.row, Some(palette.errbg));
        let plain = line_colors(Some(Level::Error), Levels::Tint, &palette);
        assert_eq!(plain.message, palette.err);
        assert_eq!(plain.row, Some(palette.errbg));
        let dim = line_colors(Some(Level::Debug), Levels::Tint, &palette);
        assert_eq!(dim.message, palette.mute);
        assert_eq!(
            line_colors(None, Levels::Tint, &palette).message,
            palette.fg
        );
    }

    #[test]
    fn finds_matches_case_insensitively() {
        assert_eq!(find_matches("Stripe stripe", "STRIPE"), [0..6, 7..13]);
        assert_eq!(find_matches("anything", "").len(), 0);
    }
}
