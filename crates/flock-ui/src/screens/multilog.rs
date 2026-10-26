use std::collections::BTreeMap;

use flock_core::multilog::{ColorSlots, MergedLog, Placement};
use flock_core::tabs::pod_suffix;
use flock_core::{Command, LogEvent, LogSource, LogTarget, MultiLogRequest, Sink, SourceId};
use gpui::{
    AnyElement, AppContext, Context, Entity, EventEmitter, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Task,
    Window, div, prelude::FluentBuilder,
};

use crate::highlight::{SOURCE_COLORS, source_colors};
use crate::kit::{
    ActionMenu, FILTER_PLACEHOLDER, InputEvent, LogLine, LogView, MenuItem, MetaLine, PageHeader,
    TextField, TextFieldState, drain, find_matches,
};
use crate::screens::workload::kit_line;
use crate::theme::{
    ActiveTheme, CONTROL_SMALL, FONT_MONO, LEGEND_CHIP_SWATCH, RADIUS, RADIUS_SMALL, RULE, SPACE_2,
    SPACE_3, SPACE_4, SPACE_8, TEXT_SMALL,
};

const PAUSE: &str = "Pause";
const META_SEPARATOR: &str = " · ";
const CONTAINER_SEPARATOR: &str = "/";
const ENDED: &str = "ended";
const LEFT: &str = "left";
const FOLLOWING: &str = "new pods join automatically";
const WAITING: &str = "Waiting for running containers…";
const PODS: &str = "pods";
const POD: &str = "pod";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceState {
    Live,
    Ended,
    Left,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegendEntry {
    pub source: LogSource,
    pub label: SharedString,
    pub slot: usize,
    pub state: SourceState,
    pub hidden: bool,
}

pub struct MultiLogs {
    id: SharedString,
    target: LogTarget,
    merged: MergedLog<LogLine>,
    legend: BTreeMap<SourceId, LegendEntry>,
    slots: ColorSlots,
    logs: Entity<LogView>,
    filter: Entity<TextFieldState>,
    options: Entity<ActionMenu>,
    query: String,
    paused: bool,
    pending: usize,
    failure: Option<String>,
    stream: Option<Task<()>>,
    _filter: Subscription,
}

impl EventEmitter<Command> for MultiLogs {}

fn pod_key(source: &LogSource) -> String {
    format!("{}/{}", source.namespace, source.pod)
}

impl MultiLogs {
    pub fn new(
        id: impl Into<SharedString>,
        target: LogTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let id: SharedString = id.into();
        let logs = cx.new(|_| LogView::new(id.clone()));
        let filter = cx.new(|cx| TextFieldState::new(window, cx).placeholder(FILTER_PLACEHOLDER));
        let options = cx.new(|cx| ActionMenu::new(format!("{id}-options"), cx));
        let subscription = cx.subscribe(&filter, |this, filter, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                let query = filter.read(cx).value().to_string();
                this.set_query(query, cx);
            }
        });
        Self {
            id,
            target,
            merged: MergedLog::default(),
            legend: BTreeMap::new(),
            slots: ColorSlots::new(SOURCE_COLORS),
            logs,
            filter,
            options,
            query: String::new(),
            paused: false,
            pending: 0,
            failure: None,
            stream: None,
            _filter: subscription,
        }
    }

    pub fn target(&self) -> &LogTarget {
        &self.target
    }

    pub fn logs(&self) -> &Entity<LogView> {
        &self.logs
    }

    pub fn legend(&self) -> Vec<&LegendEntry> {
        self.legend.values().collect()
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    pub fn watch(&mut self, cx: &mut Context<Self>) {
        let (sink, events) = Sink::channel();
        self.merged.clear();
        self.legend.clear();
        self.slots = ColorSlots::new(SOURCE_COLORS);
        self.failure = None;
        self.logs.update(cx, |logs, cx| {
            logs.clear(cx);
            logs.set_follow(true, cx);
        });
        cx.emit(Command::StreamPodLogs(MultiLogRequest {
            target: self.target.clone(),
            sink,
        }));
        self.stream = Some(cx.spawn(async move |this, cx| {
            drain(events, this, cx, Self::apply).await;
        }));
    }

    pub fn apply(&mut self, events: Vec<LogEvent>, cx: &mut Context<Self>) {
        let mut rebuild = false;
        let mut appended = Vec::new();
        for event in events {
            match event {
                LogEvent::Joined(id, source) => {
                    let slot = self.slots.assign(&pod_key(&source));
                    let entry = LegendEntry {
                        label: SharedString::default(),
                        source,
                        slot,
                        state: SourceState::Live,
                        hidden: false,
                    };
                    self.legend.insert(id, entry);
                    rebuild |= self.relabel();
                }
                LogEvent::Line(id, at, line) => {
                    let line = kit_line(line);
                    let visible = self.visible(id, &line);
                    let projected = visible.then(|| self.project(id, &line));
                    let before = self.merged.len();
                    match self.merged.insert(at, id, line) {
                        Placement::End => appended.extend(projected),
                        Placement::At(ix) => {
                            let mirrored = !rebuild && !self.paused && self.all_shown();
                            match projected.filter(|_| mirrored) {
                                Some(line) => {
                                    let earlier = std::mem::take(&mut appended);
                                    rebuild = !self.insert_shown(before, ix, earlier, line, cx);
                                }
                                None => rebuild |= visible,
                            }
                        }
                        Placement::Dropped => {}
                    }
                }
                LogEvent::Ended(id) => self.mark(id, SourceState::Ended),
                LogEvent::Left(id) => {
                    self.mark(id, SourceState::Left);
                    self.release(id);
                }
                LogEvent::Failed(reason) => self.failure = Some(reason),
            }
        }
        if self.paused {
            self.pending += appended.len();
            cx.notify();
            return;
        }
        if rebuild {
            self.rebuild(cx);
        } else if !appended.is_empty() {
            self.logs.update(cx, |logs, cx| logs.extend(appended, cx));
        }
        cx.notify();
    }

    fn all_shown(&self) -> bool {
        self.query.is_empty() && self.legend.values().all(|entry| !entry.hidden)
    }

    fn insert_shown(
        &mut self,
        before: usize,
        ix: usize,
        earlier: Vec<LogLine>,
        line: LogLine,
        cx: &mut Context<Self>,
    ) -> bool {
        self.logs.update(cx, |logs, cx| {
            logs.extend(earlier, cx);
            if logs.len() != before {
                return false;
            }
            logs.insert(ix, line, cx);
            true
        })
    }

    fn mark(&mut self, id: SourceId, state: SourceState) {
        if let Some(entry) = self.legend.get_mut(&id) {
            entry.state = state;
        }
    }

    fn release(&mut self, id: SourceId) {
        let Some(key) = self.legend.get(&id).map(|entry| pod_key(&entry.source)) else {
            return;
        };
        let live = self
            .legend
            .values()
            .any(|entry| entry.state != SourceState::Left && pod_key(&entry.source) == key);
        if !live {
            self.slots.release(&key);
        }
    }

    fn relabel(&mut self) -> bool {
        let mut containers: BTreeMap<String, usize> = BTreeMap::new();
        for entry in self.legend.values() {
            *containers.entry(pod_key(&entry.source)).or_default() += 1;
        }
        let mut changed = false;
        for entry in self.legend.values_mut() {
            let pod = pod_suffix(&entry.source.pod);
            let several = containers
                .get(&pod_key(&entry.source))
                .is_some_and(|count| *count > 1);
            let label: SharedString = if several {
                format!("{pod}{CONTAINER_SEPARATOR}{}", entry.source.container).into()
            } else {
                pod.to_owned().into()
            };
            if entry.label != label {
                changed |= !entry.label.is_empty();
                entry.label = label;
            }
        }
        changed
    }

    fn visible(&self, id: SourceId, line: &LogLine) -> bool {
        let shown = self.legend.get(&id).is_none_or(|entry| !entry.hidden);
        shown && (self.query.is_empty() || !find_matches(&line.message, &self.query).is_empty())
    }

    fn project(&self, id: SourceId, line: &LogLine) -> LogLine {
        match self.legend.get(&id) {
            Some(entry) => line.clone().with_source(entry.label.clone(), entry.slot),
            None => line.clone(),
        }
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let lines: Vec<LogLine> = self
            .merged
            .iter()
            .filter(|entry| self.visible(entry.source, &entry.item))
            .map(|entry| self.project(entry.source, &entry.item))
            .collect();
        self.pending = 0;
        self.logs.update(cx, |logs, cx| logs.set_lines(lines, cx));
    }

    pub fn toggle(&mut self, id: SourceId, cx: &mut Context<Self>) {
        if let Some(entry) = self.legend.get_mut(&id) {
            entry.hidden = !entry.hidden;
        }
        self.rebuild(cx);
        cx.notify();
    }

    pub fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.query = query;
        let highlight = self.query.clone();
        self.logs
            .update(cx, |logs, cx| logs.set_highlight(highlight, cx));
        self.rebuild(cx);
        cx.notify();
    }

    pub fn set_paused(&mut self, paused: bool, cx: &mut Context<Self>) {
        self.paused = paused;
        if !paused {
            self.rebuild(cx);
        }
        cx.notify();
    }

    pub fn tools(&self, extra: Vec<MenuItem>, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let paused = self.paused;
        let pause = if paused && self.pending > 0 {
            format!("{PAUSE} · {}", self.pending)
        } else {
            PAUSE.to_owned()
        };
        let mut items = LogView::option_items(&self.logs, false, cx);
        let screen = cx.weak_entity();
        items.extend(extra);
        items.push(
            MenuItem::new(pause, move |_, cx| {
                screen
                    .update(cx, |this, cx| this.set_paused(!paused, cx))
                    .ok();
            })
            .checked(paused),
        );
        self.options
            .update(cx, |menu, cx| menu.set_items(items, cx));
        div()
            .flex()
            .items_center()
            .gap(SPACE_3)
            .child(TextField::search(&self.filter))
            .child(self.options.clone())
    }

    pub fn summary(&self) -> String {
        let pods: std::collections::BTreeSet<String> = self
            .legend
            .values()
            .filter(|entry| entry.state != SourceState::Left)
            .map(|entry| pod_key(&entry.source))
            .collect();
        let noun = if pods.len() == 1 { POD } else { PODS };
        let count = format!("{} {noun}", pods.len());
        let mut facts: Vec<String> = Vec::new();
        facts.extend(self.target.namespace().map(str::to_owned));
        facts.push(self.target.cluster().to_string());
        facts.push(count);
        if self.target.follows_new_pods() {
            facts.push(FOLLOWING.to_owned());
        }
        facts.join(META_SEPARATOR)
    }

    fn chip(
        &self,
        id: SourceId,
        entry: &LegendEntry,
        color: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = *cx.palette();
        let shown = !entry.hidden;
        let note = match entry.state {
            SourceState::Live => None,
            SourceState::Ended => Some(ENDED),
            SourceState::Left => Some(LEFT),
        };
        let selector = format!("{}-source-{}", self.id, id.0);
        let element_id = SharedString::from(selector.clone());
        div()
            .id(element_id)
            .debug_selector(move || selector)
            .flex()
            .flex_none()
            .items_center()
            .gap(SPACE_2)
            .h(CONTROL_SMALL)
            .px(SPACE_3)
            .rounded(RADIUS)
            .border(RULE)
            .cursor_pointer()
            .font_family(FONT_MONO)
            .text_size(TEXT_SMALL)
            .when(shown, |chip| {
                chip.border_color(palette.selline)
                    .bg(palette.sel)
                    .text_color(palette.selfg)
            })
            .when(!shown, |chip| {
                chip.border_color(palette.line2).text_color(palette.mute)
            })
            .child(
                div()
                    .size(LEGEND_CHIP_SWATCH)
                    .flex_none()
                    .rounded(RADIUS_SMALL)
                    .when(shown, |swatch| swatch.bg(color))
                    .when(!shown, |swatch| swatch.border(RULE).border_color(color)),
            )
            .child(entry.label.clone())
            .children(note.map(|note| div().text_color(palette.mute).child(note)))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(id, cx)))
            .into_any_element()
    }
}

impl Render for MultiLogs {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let colors = source_colors(&palette);
        let entries: Vec<(SourceId, LegendEntry)> = self
            .legend
            .iter()
            .map(|(id, entry)| (*id, entry.clone()))
            .collect();
        let chips: Vec<AnyElement> = entries
            .iter()
            .map(|(id, entry)| {
                let color = colors
                    .get(entry.slot % SOURCE_COLORS)
                    .copied()
                    .unwrap_or(palette.fg2);
                self.chip(*id, entry, color, cx)
            })
            .collect();
        let note = match (&self.failure, chips.is_empty()) {
            (Some(reason), _) => Some((reason.clone(), palette.err)),
            (None, true) => Some((WAITING.to_owned(), palette.mute)),
            (None, false) => None,
        };
        let legend = div()
            .debug_selector({
                let id = self.id.clone();
                move || format!("{id}-legend")
            })
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap(SPACE_3)
            .px(SPACE_8)
            .py(SPACE_3)
            .border_b(RULE)
            .border_color(palette.line)
            .children(chips)
            .children(
                note.map(|(text, color)| div().text_size(TEXT_SMALL).text_color(color).child(text)),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette.editor)
            .child(legend)
            .child(div().flex_1().min_h_0().child(LogView::pane(&self.logs)))
    }
}

pub struct MultiLogsScreen {
    pub(crate) navigation: Option<Entity<crate::kit::HeaderNavigation>>,
    view: Entity<MultiLogs>,
    compact: bool,
    _events: Subscription,
}

impl EventEmitter<Command> for MultiLogsScreen {}

impl MultiLogsScreen {
    pub fn new(target: LogTarget, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let view = cx.new(|cx| MultiLogs::new("tab-pod-logs", target, window, cx));
        let events = cx.subscribe(&view, |_, _, command: &Command, cx| {
            cx.emit(command.clone());
        });
        cx.observe(&view, |_, _, cx| cx.notify()).detach();
        Self {
            navigation: None,
            view,
            compact: false,
            _events: events,
        }
    }

    pub fn shows(&self, target: &LogTarget, cx: &gpui::App) -> bool {
        self.view.read(cx).target() == target
    }

    pub fn view(&self) -> &Entity<MultiLogs> {
        &self.view
    }

    pub fn set_compact(&mut self, compact: bool, cx: &mut Context<Self>) {
        if self.compact != compact {
            self.compact = compact;
            cx.notify();
        }
    }

    pub fn watch(&mut self, cx: &mut Context<Self>) {
        self.view.update(cx, MultiLogs::watch);
    }
}

impl Render for MultiLogsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let title = self.view.read(cx).target().label();
        if self.compact {
            let logs = self.view.read(cx).logs().clone();
            return div()
                .size_full()
                .flex()
                .flex_col()
                .bg(palette.editor)
                .child(crate::kit::PageTitle::new(title).leading(self.navigation.clone()))
                .child(div().flex_1().min_h_0().child(LogView::pane(&logs)));
        }
        let summary = self.view.read(cx).summary();
        let header = PageHeader::new(title)
            .leading(self.navigation.clone())
            .mono()
            .meta(MetaLine::new(summary));
        let tools = self
            .view
            .update(cx, |view, cx| view.tools(Vec::new(), cx).into_any_element());
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette.editor)
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .px(SPACE_8)
                    .py(SPACE_4)
                    .border_b(RULE)
                    .border_color(palette.line)
                    .child(tools),
            )
            .child(div().flex_1().min_h_0().child(self.view.clone()))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use flock_core::multilog::{MERGED_LINES, Stamp};
    use flock_core::{ClusterId, LogLevel};
    use gpui::{App, TestAppContext};

    use super::*;

    const TIME: &str = "2026-10-25T00:00:00Z";
    const QUERY: &str = "1";

    fn source(pod: &str) -> LogSource {
        LogSource {
            namespace: "web".into(),
            pod: pod.into(),
            container: "api".into(),
        }
    }

    fn line(id: u32, at: i128) -> LogEvent {
        let line = flock_core::LogLine::new(TIME, LogLevel::Info, at.to_string());
        LogEvent::Line(SourceId(id), Some(Stamp(at)), line)
    }

    fn shown(view: &MultiLogs, cx: &App) -> Vec<String> {
        let logs = view.logs.read(cx);
        (0..logs.len())
            .filter_map(|ix| logs.line(ix).map(|line| line.message.to_string()))
            .collect()
    }

    fn merged(view: &MultiLogs) -> Vec<String> {
        view.merged
            .iter()
            .filter(|entry| view.visible(entry.source, &entry.item))
            .map(|entry| entry.item.message.to_string())
            .collect()
    }

    fn open(cx: &mut TestAppContext) -> gpui::WindowHandle<MultiLogs> {
        cx.update(|cx| {
            crate::init(Some(crate::theme::Appearance::Dark), cx)
                .unwrap_or_else(|error| panic!("UI init: {error}"));
        });
        let target = LogTarget::Pods {
            cluster: ClusterId::new("a"),
            pods: BTreeSet::new(),
        };
        let window = cx.add_window(|window, cx| MultiLogs::new("test-logs", target, window, cx));
        let joined = vec![
            LogEvent::Joined(SourceId(0), source("web-a")),
            LogEvent::Joined(SourceId(1), source("web-b")),
        ];
        let _ = window.update(cx, |view, _, cx| view.apply(joined, cx));
        window
    }

    #[gpui::test]
    fn late_lines_land_in_time_order(cx: &mut TestAppContext) {
        let window = open(cx);
        let updated = window.update(cx, |view, _, cx| {
            view.apply(vec![line(0, 1), line(0, 3), line(0, 5)], cx);
            view.apply(vec![line(1, 2), line(0, 6), line(1, 4)], cx);
            assert_eq!(shown(view, cx), ["1", "2", "3", "4", "5", "6"]);
            assert_eq!(shown(view, cx), merged(view));
        });
        assert!(updated.is_ok());
    }

    #[gpui::test]
    fn late_lines_match_the_merged_order_when_filtered_or_full(cx: &mut TestAppContext) {
        let window = open(cx);
        let updated = window.update(cx, |view, _, cx| {
            let early: Vec<LogEvent> = (0..MERGED_LINES as i128)
                .map(|at| line(0, at * 2))
                .collect();
            view.apply(early, cx);
            view.apply(vec![line(1, 101), line(1, 7)], cx);
            assert_eq!(view.logs.read(cx).len(), MERGED_LINES);
            assert_eq!(shown(view, cx), merged(view));
            view.set_query(QUERY.to_owned(), cx);
            view.apply(vec![line(1, 1001), line(1, 15)], cx);
            assert_eq!(shown(view, cx), merged(view));
        });
        assert!(updated.is_ok());
    }

    #[test]
    fn pods_share_a_key_across_their_containers() {
        let source = |container: &str| LogSource {
            namespace: "web".into(),
            pod: "checkout-7f9c6d-x2lqp".into(),
            container: container.into(),
        };
        assert_eq!(pod_key(&source("api")), pod_key(&source("envoy")));
    }
}
