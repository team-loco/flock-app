use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use flock_core::filter::TextFilter;
use flock_core::logpage::{
    self, Bucket, GroupKey, LevelBand, LineFacts, MatchCursor, RUNNING, SourceCount, SourceFacts,
    SourceRow, TimeRange, TimeWindow, VOLUME_BUCKETS, VOLUME_TICKS,
};
use flock_core::multilog::{ColorSlots, MergedLog, Placement};
use flock_core::tabs::pod_suffix;
use flock_core::{
    ClusterId, Command, LogEvent, LogLevel, LogSource, LogTarget, MultiLogRequest, ResourceRef,
    Sink, Snapshot, SourceId,
};
use gpui::{
    AnyElement, AppContext, Context, Entity, EventEmitter, FontWeight, Hsla, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, Window, div, prelude::FluentBuilder,
    px, svg,
};

use crate::assets::{CHEVRON_DOWN, CHEVRON_UP, GROUP, PLUS};
use crate::highlight::{SOURCE_COLORS, source_colors};
use crate::kit::{
    ActionMenu, Button, FilterBar, FilterEvent, IconButton, LogLine, LogView, MenuItem,
    PanelSection, SourceDetail, drain, pane,
};
use crate::screens::pods::{log_file_name, save_log};
use crate::screens::workload::kit_line;
use crate::theme::{
    ActiveTheme, FONT_MONO, GLYPH_SLOT, ICON, LOG_SOURCE_CLUSTERS, LOG_SOURCE_DETAIL,
    LOG_SOURCE_OFF, LOG_SOURCE_SWATCH, LOG_SOURCE_SWATCH_RING, LOG_SOURCES_PANEL, LOGS_QUERY_MAX,
    LOGS_QUERY_MIN, Palette, RADIUS, RADIUS_SMALL, RULE, SEGMENT, SPACE_1, SPACE_2, SPACE_3,
    SPACE_4, SPACE_5, SPACE_8, TEXT_AXIS, TEXT_BODY, TEXT_GLYPH, TEXT_LABEL, TEXT_SMALL,
    TEXT_SUBHEADING, VOLUME_BAR_GAP, VOLUME_HEIGHT, VOLUME_TICKS_GAP,
};

const ID: &str = "logs";
const TITLE: &str = "Logs";
const QUERY_PLACEHOLDER: &str = "Filter by @level, @pod… or search text";
const VOLUME: &str = "Volume";
const SOURCES: &str = "Sources";
const EXPORT: &str = "Export";
const ADD_SOURCE: &str = "Add source";
const PREVIOUS_MATCH: &str = "Previous match";
const NEXT_MATCH: &str = "Next match";
const WAITING: &str = "Waiting for running containers…";
const NO_MORE_PODS: &str = "No other pods in these namespaces";
const OPEN: &str = "▾";
const SHUT: &str = "▸";
const DOT: &str = " · ";
const SLASH: &str = "/";
const WORKLOAD_PREFIX: &str = "deploy/";
const ENDED: &str = "ended";
const LEFT: &str = "left";
const EXPORT_NAME: &str = "logs";
const WRAP: &str = "wrap";
const TIME: &str = "time";
const RAW: &str = "raw";
const ADD_LIMIT: usize = 20;
const SOURCE_INDENT: f32 = 14.;
const DEFAULT_GROUP: [GroupKey; 1] = [GroupKey::Status];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceState {
    Live,
    Ended,
    Left,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ViewOption {
    Wrap,
    Time,
    Raw,
}

impl ViewOption {
    const ALL: [Self; 3] = [Self::Wrap, Self::Time, Self::Raw];

    fn label(self) -> &'static str {
        match self {
            Self::Wrap => WRAP,
            Self::Time => TIME,
            Self::Raw => RAW,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageSource {
    pub source: LogSource,
    pub cluster: ClusterId,
    pub workload: String,
    pub slot: usize,
    pub state: SourceState,
    pub on: bool,
    pub lines: usize,
    pub status: String,
    pub node: String,
    pub restarts: u32,
}

impl PageSource {
    pub fn label(&self) -> String {
        let pod = pod_suffix(&self.source.pod);
        format!("{pod}{SLASH}{}", self.source.container)
    }

    fn facts(&self, level: Option<LogLevel>) -> LineFacts<'_> {
        LineFacts {
            level,
            cluster: self.cluster.as_str(),
            namespace: &self.source.namespace,
            workload: &self.workload,
            pod: &self.source.pod,
            container: &self.source.container,
        }
    }

    fn group_facts(&self) -> SourceFacts {
        SourceFacts {
            status: self.status.clone(),
            workload: self.workload.clone(),
            cluster: self.cluster.to_string(),
            node: self.node.clone(),
            on: self.on,
        }
    }
}

#[derive(Clone, Debug)]
struct Stored {
    line: LogLine,
    level: Option<LogLevel>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Volume {
    buckets: Vec<Bucket>,
    ticks: Vec<String>,
    summary: String,
    ranges: Vec<(TimeRange, usize)>,
}

pub struct LogsPage {
    pub(crate) navigation: Option<Entity<crate::kit::HeaderNavigation>>,
    target: LogTarget,
    clusters: Vec<ClusterId>,
    extra: Vec<LogTarget>,
    merged: MergedLog<Stored>,
    sources: BTreeMap<SourceId, PageSource>,
    ids: HashMap<(usize, SourceId), SourceId>,
    slots: ColorSlots,
    levels: BTreeMap<LevelBand, usize>,
    snapshot: Option<Arc<Snapshot>>,
    logs: Entity<LogView>,
    query: Entity<FilterBar>,
    menu: Entity<ActionMenu>,
    filter: TextFilter,
    keys: Vec<GroupKey>,
    closed: BTreeSet<String>,
    range: TimeRange,
    cursor: MatchCursor,
    volume: Volume,
    failure: Option<String>,
    compact: bool,
    streams: Vec<Task<()>>,
    _query: Subscription,
}

impl EventEmitter<Command> for LogsPage {}

fn source_key(cluster: &ClusterId, source: &LogSource) -> String {
    format!(
        "{cluster}/{}/{}/{}",
        source.namespace, source.pod, source.container
    )
}

impl LogsPage {
    pub fn new(
        target: LogTarget,
        clusters: Vec<ClusterId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let logs = cx.new(|_| LogView::new(format!("{ID}-lines")).parsed().marked());
        let query = cx.new(|cx| {
            let mut bar = FilterBar::new(format!("{ID}-query"), window, cx).inline();
            bar.set_placeholder(QUERY_PLACEHOLDER);
            bar
        });
        let menu = cx.new(|cx| ActionMenu::context(format!("{ID}-menu"), cx));
        let subscription = cx.subscribe(&query, |this, _, event: &FilterEvent, cx| match event {
            FilterEvent::Changed => this.query_changed(cx),
        });
        Self {
            navigation: None,
            target,
            clusters,
            extra: Vec::new(),
            merged: MergedLog::default(),
            sources: BTreeMap::new(),
            ids: HashMap::new(),
            slots: ColorSlots::new(SOURCE_COLORS),
            levels: BTreeMap::new(),
            snapshot: None,
            logs,
            query,
            menu,
            filter: TextFilter::default(),
            keys: DEFAULT_GROUP.to_vec(),
            closed: BTreeSet::new(),
            range: TimeRange::Live,
            cursor: MatchCursor::default(),
            volume: Volume::default(),
            failure: None,
            compact: false,
            streams: Vec::new(),
            _query: subscription,
        }
    }

    pub fn shows(&self, target: &LogTarget, clusters: &[ClusterId]) -> bool {
        &self.target == target && self.clusters == clusters
    }

    pub fn target(&self) -> &LogTarget {
        &self.target
    }

    pub fn logs(&self) -> &Entity<LogView> {
        &self.logs
    }

    pub fn sources(&self) -> Vec<&PageSource> {
        self.sources.values().collect()
    }

    pub fn match_position(&self) -> String {
        self.cursor.position()
    }

    pub fn current_match(&self) -> Option<usize> {
        self.cursor.line()
    }

    pub fn set_compact(&mut self, compact: bool, cx: &mut Context<Self>) {
        if self.compact != compact {
            self.compact = compact;
            cx.notify();
        }
    }

    fn several_clusters(&self) -> bool {
        self.clusters.len() > 1
    }

    pub fn watch(&mut self, cx: &mut Context<Self>) {
        self.merged.clear();
        self.sources.clear();
        self.ids.clear();
        self.levels.clear();
        self.slots = ColorSlots::new(SOURCE_COLORS);
        self.failure = None;
        self.streams.clear();
        self.logs.update(cx, |logs, cx| {
            logs.clear(cx);
            logs.set_follow(true, cx);
        });
        let width = if self.several_clusters() {
            LOG_SOURCE_CLUSTERS
        } else {
            LOG_SOURCE_DETAIL
        };
        self.logs
            .update(cx, |logs, cx| logs.set_source_width(width, cx));
        let targets: Vec<LogTarget> = self
            .target
            .across(&self.clusters)
            .into_iter()
            .chain(self.extra.clone())
            .collect();
        for target in targets {
            self.stream(target, cx);
        }
        self.refresh_volume(cx);
    }

    fn stream(&mut self, target: LogTarget, cx: &mut Context<Self>) {
        let stream = self.streams.len();
        let (sink, events) = Sink::channel();
        let cluster = target.cluster().clone();
        cx.emit(Command::StreamPodLogs(MultiLogRequest { target, sink }));
        self.streams.push(cx.spawn(async move |this, cx| {
            drain(events, this, cx, move |page: &mut Self, batch, cx| {
                page.apply(stream, &cluster, batch, cx);
            })
            .await;
        }));
    }

    pub fn set_snapshot(&mut self, snapshot: &Arc<Snapshot>, cx: &mut Context<Self>) {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|known| Arc::ptr_eq(known, snapshot))
        {
            return;
        }
        self.snapshot = Some(snapshot.clone());
        let ids: Vec<SourceId> = self.sources.keys().copied().collect();
        for id in ids {
            self.describe(id);
        }
        cx.notify();
    }

    fn describe(&mut self, id: SourceId) {
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        let workload_target = match &self.target {
            LogTarget::Workload(target) => Some(target.name.clone()),
            LogTarget::Selector { .. } => None,
            LogTarget::Pods { .. } => None,
        };
        let Some(entry) = self.sources.get_mut(&id) else {
            return;
        };
        let pod = snapshot.pods.iter().find(|pod| {
            pod.cluster == entry.cluster
                && pod.namespace == entry.source.namespace
                && pod.name == entry.source.pod
        });
        let Some(pod) = pod else {
            return;
        };
        let owner = pod
            .owner
            .as_ref()
            .map(|owner| (owner.kind.as_str(), owner.name.as_str()));
        entry.workload =
            workload_target.unwrap_or_else(|| logpage::workload_name(owner, &pod.name).to_owned());
        entry.node = pod.node.clone().unwrap_or_default();
        if let Some(container) = pod
            .containers
            .iter()
            .find(|c| c.name == entry.source.container)
        {
            entry.status = logpage::source_status(container);
            entry.restarts = container.restarts;
        }
    }

    pub fn apply(
        &mut self,
        stream: usize,
        cluster: &ClusterId,
        events: Vec<LogEvent>,
        cx: &mut Context<Self>,
    ) {
        let mut rebuild = false;
        let mut appended = Vec::new();
        for event in events {
            match event {
                LogEvent::Joined(remote, source) => {
                    let id = SourceId(u32::try_from(self.ids.len()).unwrap_or(u32::MAX));
                    self.ids.insert((stream, remote), id);
                    let slot = self.slots.assign(&source_key(cluster, &source));
                    let workload = match &self.target {
                        LogTarget::Workload(target) => target.name.clone(),
                        LogTarget::Selector { .. } => source.pod.clone(),
                        LogTarget::Pods { .. } => source.pod.clone(),
                    };
                    self.sources.insert(
                        id,
                        PageSource {
                            source,
                            cluster: cluster.clone(),
                            workload,
                            slot,
                            state: SourceState::Live,
                            on: true,
                            lines: 0,
                            status: RUNNING.to_owned(),
                            node: String::new(),
                            restarts: 0,
                        },
                    );
                    self.describe(id);
                }
                LogEvent::Line(remote, at, line) => {
                    let Some(id) = self.ids.get(&(stream, remote)).copied() else {
                        continue;
                    };
                    let level = logpage::detect_level(line.level, &line.message);
                    let stored = stored_line(line, level);
                    *self.levels.entry(LevelBand::of(level)).or_default() += 1;
                    if let Some(entry) = self.sources.get_mut(&id) {
                        entry.lines += 1;
                    }
                    let shown = self.passes(id, level);
                    let projected = shown.then(|| self.project(id, &stored));
                    match self.merged.insert(at, id, stored) {
                        Placement::End => appended.extend(projected),
                        Placement::At(_) => rebuild |= shown,
                        Placement::Dropped => {}
                    }
                }
                LogEvent::Ended(remote) => self.mark(stream, remote, SourceState::Ended),
                LogEvent::Left(remote) => self.mark(stream, remote, SourceState::Left),
                LogEvent::Failed(reason) => self.failure = Some(reason),
            }
        }
        rebuild |= self.range != TimeRange::Live && !appended.is_empty();
        if rebuild {
            self.rebuild(cx);
        } else if !appended.is_empty() {
            self.append(appended, cx);
        }
        self.refresh_volume(cx);
        cx.notify();
    }

    fn mark(&mut self, stream: usize, remote: SourceId, state: SourceState) {
        let id = self.ids.get(&(stream, remote)).copied();
        if let Some(entry) = id.and_then(|id| self.sources.get_mut(&id)) {
            entry.state = state;
            if state == SourceState::Left {
                let key = source_key(&entry.cluster, &entry.source);
                self.slots.release(&key);
            }
        }
    }

    fn passes(&self, id: SourceId, level: Option<LogLevel>) -> bool {
        self.sources.get(&id).is_some_and(|source| {
            source.on && logpage::line_matches(&self.filter, &source.facts(level))
        })
    }

    fn window(&self) -> Option<TimeWindow> {
        let oldest = self.merged.iter().next()?.key.0;
        let newest = self.merged.iter().next_back()?.key.0;
        Some(self.range.window(oldest, newest))
    }

    fn project(&self, id: SourceId, stored: &Stored) -> LogLine {
        let Some(source) = self.sources.get(&id) else {
            return stored.line.clone();
        };
        let detail = SourceDetail {
            workload: source.workload.clone().into(),
            place: source.label().into(),
            cluster: self
                .several_clusters()
                .then(|| source.cluster.to_string().into()),
        };
        stored.line.clone().with_detail(detail, source.slot)
    }

    fn append(&mut self, lines: Vec<LogLine>, cx: &mut Context<Self>) {
        let added = lines.len();
        let before = self.logs.read(cx).len();
        self.logs.update(cx, |logs, cx| logs.extend(lines, cx));
        let after = self.logs.read(cx).len();
        if after < before + added {
            self.rematch(cx);
            return;
        }
        let hits = self.logs.read(cx).match_lines_from(before);
        let first = self.cursor.is_empty() && !hits.is_empty();
        self.cursor.extend(hits);
        if first {
            self.mark_current(cx);
        }
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let window = self.window().filter(|_| self.range != TimeRange::Live);
        let lines: Vec<LogLine> = self
            .merged
            .iter()
            .filter(|entry| window.is_none_or(|window| window.contains(entry.key.0)))
            .filter(|entry| self.passes(entry.source, entry.item.level))
            .map(|entry| self.project(entry.source, &entry.item))
            .collect();
        self.logs.update(cx, |logs, cx| logs.set_lines(lines, cx));
        self.rematch(cx);
    }

    fn rematch(&mut self, cx: &mut Context<Self>) {
        let hits = self.logs.read(cx).match_lines_from(0);
        let near = self.cursor.line();
        self.cursor.keep_near(hits, near);
        self.mark_current(cx);
    }

    fn mark_current(&mut self, cx: &mut Context<Self>) {
        let current = self.cursor.line();
        self.logs
            .update(cx, |logs, cx| logs.set_current(current, cx));
    }

    fn reveal_current(&mut self, cx: &mut Context<Self>) {
        self.mark_current(cx);
        if let Some(line) = self.cursor.line() {
            self.logs.update(cx, |logs, cx| logs.reveal(line, cx));
        }
    }

    fn refresh_volume(&mut self, cx: &mut Context<Self>) {
        let filtered: Vec<(i128, LevelBand)> = self
            .merged
            .iter()
            .filter(|entry| self.passes(entry.source, entry.item.level))
            .map(|entry| (entry.key.0, LevelBand::of(entry.item.level)))
            .collect();
        let stamps: Vec<i128> = filtered.iter().map(|(at, _)| *at).collect();
        let window = self.window();
        let buckets = window.map_or_else(Vec::new, |window| {
            logpage::histogram(filtered.iter().copied(), window, VOLUME_BUCKETS)
        });
        let ticks = window.map_or_else(Vec::new, |window| logpage::ticks(window, VOLUME_TICKS));
        let shown = self.logs.read(cx).len();
        self.volume = Volume {
            buckets,
            ticks,
            summary: logpage::volume_summary(shown, self.merged.len(), window),
            ranges: logpage::range_counts(&stamps),
        };
        let counts = self.sources.values().map(|source| SourceCount {
            facts: source.facts(None),
            lines: source.lines,
        });
        let facets = logpage::facets(counts, &self.levels, self.several_clusters());
        self.query.update(cx, |bar, cx| bar.set_facets(facets, cx));
    }

    fn query_changed(&mut self, cx: &mut Context<Self>) {
        self.filter = self.query.read(cx).query().clone();
        let phrase = logpage::search_phrase(&self.filter);
        self.logs
            .update(cx, |logs, cx| logs.set_highlight(phrase, cx));
        self.cursor = MatchCursor::default();
        self.rebuild(cx);
        self.reveal_current(cx);
        self.refresh_volume(cx);
        cx.notify();
    }

    pub fn step_match(&mut self, forward: bool, cx: &mut Context<Self>) {
        if forward {
            self.cursor.next();
        } else {
            self.cursor.previous();
        }
        self.reveal_current(cx);
        cx.notify();
    }

    pub fn toggle_source(&mut self, id: SourceId, cx: &mut Context<Self>) {
        if let Some(entry) = self.sources.get_mut(&id) {
            entry.on = !entry.on;
        }
        self.rebuild(cx);
        self.refresh_volume(cx);
        cx.notify();
    }

    fn set_range(&mut self, range: TimeRange, cx: &mut Context<Self>) {
        self.range = range;
        self.rebuild(cx);
        self.refresh_volume(cx);
        if range == TimeRange::Live {
            self.logs.update(cx, |logs, cx| logs.set_follow(true, cx));
        }
        cx.notify();
    }

    fn toggle_view(&mut self, option: ViewOption, cx: &mut Context<Self>) {
        self.logs.update(cx, |logs, cx| match option {
            ViewOption::Wrap => logs.set_wrap(!logs.wrap(), cx),
            ViewOption::Time => logs.set_timestamps(!logs.timestamps(), cx),
            ViewOption::Raw => logs.set_raw(!logs.raw(), cx),
        });
        if option == ViewOption::Raw {
            self.rematch(cx);
        }
        cx.notify();
    }

    fn view_on(&self, option: ViewOption, cx: &gpui::App) -> bool {
        let logs = self.logs.read(cx);
        match option {
            ViewOption::Wrap => logs.wrap(),
            ViewOption::Time => logs.timestamps(),
            ViewOption::Raw => logs.raw(),
        }
    }

    fn toggle_group(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.closed.remove(&path) {
            self.closed.insert(path);
        }
        cx.notify();
    }

    fn pick_key(&mut self, key: GroupKey, cx: &mut Context<Self>) {
        self.keys = logpage::toggle_key(&self.keys, key);
        self.closed.clear();
        cx.notify();
    }

    fn export(&self, cx: &mut Context<Self>) {
        let text = self.logs.read(cx).export();
        let name = log_file_name(&self.target.label(), EXPORT_NAME);
        save_log(&name, text, cx);
    }

    fn show_group_menu(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let page = cx.weak_entity();
        let items = GroupKey::ALL
            .iter()
            .map(|key| {
                let key = *key;
                let page = page.clone();
                MenuItem::new(key.label(), move |_, cx| {
                    page.update(cx, |page, cx| page.pick_key(key, cx)).ok();
                })
                .checked(self.keys.contains(&key))
            })
            .collect();
        self.menu.update(cx, |menu, cx| {
            menu.show_at(event.position, items, window, cx);
        });
    }

    fn addable(&self) -> Vec<ResourceRef> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        let namespaces: BTreeSet<(&ClusterId, &str)> = self
            .sources
            .values()
            .map(|source| (&source.cluster, source.source.namespace.as_str()))
            .collect();
        let shown: BTreeSet<(&ClusterId, &str)> = self
            .sources
            .values()
            .map(|source| (&source.cluster, source.source.pod.as_str()))
            .collect();
        snapshot
            .pods
            .iter()
            .filter(|pod| namespaces.contains(&(&pod.cluster, pod.namespace.as_str())))
            .filter(|pod| !shown.contains(&(&pod.cluster, pod.name.as_str())))
            .take(ADD_LIMIT)
            .map(flock_core::pods::Pod::resource)
            .collect()
    }

    fn show_add_menu(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let page = cx.weak_entity();
        let pods = self.addable();
        let items = if pods.is_empty() {
            vec![MenuItem::new(NO_MORE_PODS, |_, _| {}).disabled(true)]
        } else {
            pods.into_iter()
                .map(|pod| {
                    let page = page.clone();
                    let label =
                        format!("{}/{}", pod.namespace.clone().unwrap_or_default(), pod.name);
                    MenuItem::new(label, move |_, cx| {
                        let pod = pod.clone();
                        page.update(cx, |page, cx| page.add_pod(&pod, cx)).ok();
                    })
                })
                .collect()
        };
        self.menu.update(cx, |menu, cx| {
            menu.show_at(event.position, items, window, cx);
        });
    }

    fn add_pod(&mut self, pod: &ResourceRef, cx: &mut Context<Self>) {
        let Some(target) = LogTarget::pods(std::slice::from_ref(pod)) else {
            return;
        };
        self.extra.push(target.clone());
        self.stream(target, cx);
        cx.notify();
    }
}

fn stored_line(line: flock_core::LogLine, level: Option<LogLevel>) -> Stored {
    let short = line
        .time
        .as_deref()
        .map(|time| logpage::short_time(time).to_owned());
    let mut kit = kit_line(flock_core::LogLine {
        time: short,
        level,
        message: line.message,
    })
    .with_fields();
    if let Some(parsed) = kit.parsed.as_mut() {
        parsed.time = parsed
            .time
            .take()
            .map(|time| logpage::short_time(&time).to_owned().into());
    }
    Stored { line: kit, level }
}

impl LogsPage {
    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let palette = *cx.palette();
        let searching = !self.filter.words.is_empty();
        let steps = searching.then(|| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(SPACE_1)
                .child(
                    div()
                        .debug_selector(|| format!("{ID}-match-position"))
                        .font_family(FONT_MONO)
                        .text_size(TEXT_LABEL)
                        .text_color(palette.fg2)
                        .whitespace_nowrap()
                        .child(self.cursor.position()),
                )
                .child(
                    IconButton::svg(format!("{ID}-previous-match"), CHEVRON_UP, PREVIOUS_MATCH)
                        .on_click(cx.listener(|this, _, _, cx| this.step_match(false, cx))),
                )
                .child(
                    IconButton::svg(format!("{ID}-next-match"), CHEVRON_DOWN, NEXT_MATCH)
                        .on_click(cx.listener(|this, _, _, cx| this.step_match(true, cx))),
                )
        });
        let ranges: Vec<AnyElement> = self
            .volume
            .ranges
            .iter()
            .enumerate()
            .map(|(ix, (range, count))| {
                let range = *range;
                let on = range == self.range;
                segment(format!("{ID}-range-{ix}"), on, ix, &palette)
                    .child(range.label())
                    .child(
                        div()
                            .text_color(if on { palette.selfg } else { palette.mute })
                            .child(count.to_string()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.set_range(range, cx)))
                    .into_any_element()
            })
            .collect();
        let views: Vec<AnyElement> = ViewOption::ALL
            .iter()
            .enumerate()
            .map(|(ix, option)| {
                let option = *option;
                let on = self.view_on(option, cx);
                segment(format!("{ID}-view-{ix}"), on, ix, &palette)
                    .child(option.label())
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_view(option, cx)))
                    .into_any_element()
            })
            .collect();
        div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap(SPACE_5)
            .px(SPACE_8)
            .py(SPACE_5)
            .border_b(RULE)
            .border_color(palette.line)
            .children(self.navigation.clone())
            .child(
                div()
                    .text_size(TEXT_SUBHEADING)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(TITLE),
            )
            .child(
                div()
                    .w(LOGS_QUERY_MAX)
                    .min_w(LOGS_QUERY_MIN)
                    .child(self.query.clone()),
            )
            .children(steps)
            .child(div().flex_1())
            .child(group(&palette).children(ranges))
            .child(group(&palette).children(views))
            .child(
                Button::new(format!("{ID}-export"), EXPORT)
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| this.export(cx))),
            )
    }

    fn volume(&self, palette: &Palette) -> PanelSection {
        let max = self
            .volume
            .buckets
            .iter()
            .map(Bucket::total)
            .max()
            .unwrap_or_default()
            .max(1);
        let scale = |count: usize| VOLUME_HEIGHT * logpage::share(count, max);
        let colors = band_colors(palette);
        let bars = self.volume.buckets.iter().map(|bucket| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .justify_end()
                .children(
                    LevelBand::ALL
                        .iter()
                        .zip(colors)
                        .filter_map(|(band, color)| {
                            let count = bucket.count(*band);
                            (count > 0).then(|| div().h(scale(count)).bg(color))
                        }),
                )
        });
        let ticks = self
            .volume
            .ticks
            .iter()
            .map(|tick| div().child(tick.clone()));
        PanelSection::new(ID, VOLUME)
            .count(self.volume.summary.clone())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(VOLUME_TICKS_GAP)
                    .child(
                        div()
                            .debug_selector(|| format!("{ID}-histogram"))
                            .h(VOLUME_HEIGHT)
                            .flex()
                            .items_end()
                            .gap(VOLUME_BAR_GAP)
                            .children(bars),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .font_family(FONT_MONO)
                            .text_size(TEXT_AXIS)
                            .text_color(palette.mute)
                            .children(ticks),
                    ),
            )
    }

    fn sources_panel(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let palette = *cx.palette();
        let colors = source_colors(&palette);
        let ids: Vec<SourceId> = self.sources.keys().copied().collect();
        let facts: Vec<SourceFacts> = self.sources.values().map(PageSource::group_facts).collect();
        let rows = logpage::group_sources(&facts, &self.keys, &self.closed);
        let several_keys = self.keys.len() > 1;
        let elements: Vec<AnyElement> = rows
            .into_iter()
            .enumerate()
            .map(|(n, row)| match row {
                head @ SourceRow::Head { .. } => Self::head_row(n, head, several_keys, cx),
                SourceRow::Source { ix, depth } => {
                    let id = ids.get(ix).copied();
                    let source = id.and_then(|id| self.sources.get(&id).map(|s| (id, s)));
                    match source {
                        Some((id, source)) => {
                            let color = colors
                                .get(source.slot % SOURCE_COLORS)
                                .copied()
                                .unwrap_or(palette.fg2);
                            self.source_row(id, source, depth, color, cx)
                        }
                        None => div().into_any_element(),
                    }
                }
            })
            .collect();
        let on = self.sources.values().filter(|source| source.on).count();
        let waiting = (self.sources.is_empty()).then(|| {
            let (text, color) = match &self.failure {
                Some(reason) => (reason.clone(), palette.err),
                None => (WAITING.to_owned(), palette.mute),
            };
            div().text_size(TEXT_SMALL).text_color(color).child(text)
        });
        let picker = self.group_picker(cx);
        let add = Self::add_source(cx);
        div()
            .id(SharedString::from(format!("{ID}-sources")))
            .flex_none()
            .w(LOG_SOURCES_PANEL)
            .h_full()
            .overflow_y_scroll()
            .border_r(RULE)
            .border_color(palette.line)
            .bg(palette.panel)
            .text_size(TEXT_BODY)
            .child(
                PanelSection::new(ID, SOURCES)
                    .count(format!("{on}/{}", self.sources.len()))
                    .trailing(picker)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(SPACE_1)
                            .children(elements)
                            .children(waiting),
                    )
                    .child(add),
            )
            .child(self.menu.clone())
    }

    fn group_picker(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let palette = *cx.palette();
        div()
            .id(SharedString::from(format!("{ID}-group")))
            .debug_selector(|| format!("{ID}-group"))
            .flex()
            .items_center()
            .gap(SPACE_2)
            .h(SEGMENT)
            .px(SPACE_3)
            .rounded(RADIUS)
            .border(RULE)
            .border_color(palette.line2)
            .cursor_pointer()
            .font_family(FONT_MONO)
            .text_size(TEXT_SMALL)
            .hover(move |style| style.bg(palette.hover))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.show_group_menu(event, window, cx);
                }),
            )
            .child(svg().path(GROUP).size(GLYPH_SLOT).text_color(palette.mute))
            .child(logpage::group_label(&self.keys))
            .child(
                svg()
                    .path(CHEVRON_DOWN)
                    .size(GLYPH_SLOT)
                    .text_color(palette.mute),
            )
    }

    fn add_source(cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let palette = *cx.palette();
        div()
            .id(SharedString::from(format!("{ID}-add-source")))
            .debug_selector(|| format!("{ID}-add-source"))
            .flex()
            .items_center()
            .gap(SPACE_3)
            .py(SPACE_2)
            .cursor_pointer()
            .text_size(TEXT_BODY)
            .text_color(palette.acc)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.show_add_menu(event, window, cx);
                }),
            )
            .child(svg().path(PLUS).size(ICON).text_color(palette.acc))
            .child(ADD_SOURCE)
    }

    fn head_row(n: usize, row: SourceRow, show_key: bool, cx: &mut Context<Self>) -> AnyElement {
        let SourceRow::Head {
            key,
            name,
            path,
            depth,
            open,
            on,
            total,
        } = row
        else {
            return div().into_any_element();
        };
        let palette = *cx.palette();
        let failing = key == GroupKey::Status && name != RUNNING;
        let label = match key {
            GroupKey::Workload => format!("{WORKLOAD_PREFIX}{name}"),
            GroupKey::Status => name,
            GroupKey::Cluster => name,
            GroupKey::Node => name,
        };
        let selector = format!("{ID}-group-row-{n}");
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .flex()
            .items_center()
            .gap(SPACE_3)
            .h(SEGMENT)
            .pl(indent(depth))
            .pr(SPACE_2)
            .rounded(RADIUS)
            .cursor_pointer()
            .font_family(FONT_MONO)
            .text_size(TEXT_SMALL)
            .hover(move |style| style.bg(palette.hover))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_group(path.clone(), cx)))
            .child(
                div()
                    .w(GLYPH_SLOT)
                    .flex_none()
                    .text_size(TEXT_GLYPH)
                    .text_color(palette.mute)
                    .child(if open { OPEN } else { SHUT }),
            )
            .when(show_key, |row| {
                row.child(
                    div()
                        .text_size(TEXT_AXIS)
                        .text_color(palette.mute)
                        .child(key.word()),
                )
            })
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .when(depth == 0, |name| name.font_weight(FontWeight::MEDIUM))
                    .text_color(if failing { palette.err } else { palette.fg })
                    .child(label),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex_none()
                    .text_size(TEXT_LABEL)
                    .text_color(palette.mute)
                    .child(format!("{on}/{total}")),
            )
            .into_any_element()
    }

    fn source_row(
        &self,
        id: SourceId,
        source: &PageSource,
        depth: usize,
        color: Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = *cx.palette();
        let by_workload = self.keys.contains(&GroupKey::Workload);
        let by_cluster = self.keys.contains(&GroupKey::Cluster);
        let by_status = self.keys.contains(&GroupKey::Status);
        let pod = pod_suffix(&source.source.pod).to_owned();
        let failing = source.status != RUNNING;
        let note = match source.state {
            SourceState::Live => None,
            SourceState::Ended => Some(ENDED),
            SourceState::Left => Some(LEFT),
        };
        let meta = logpage::source_meta(source.restarts, &source.status, by_status);
        let meta = match (meta, note) {
            (Some(meta), Some(note)) => Some(format!("{meta}{DOT}{note}")),
            (Some(meta), None) => Some(meta),
            (None, Some(note)) => Some(note.to_owned()),
            (None, None) => None,
        };
        let cluster = (self.several_clusters() && !by_cluster).then(|| source.cluster.to_string());
        let selector = format!("{ID}-source-{}", id.0);
        let swatch = div()
            .size(LOG_SOURCE_SWATCH)
            .flex_none()
            .rounded(RADIUS_SMALL)
            .when(source.on, |swatch| swatch.bg(color))
            .when(!source.on, |swatch| {
                swatch.border(LOG_SOURCE_SWATCH_RING).border_color(color)
            });
        let name = div()
            .flex()
            .min_w_0()
            .whitespace_nowrap()
            .when(!by_workload, |name| {
                name.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(palette.mute)
                        .child(format!("{}{DOT}", source.workload)),
                )
            })
            .child(
                div()
                    .flex_none()
                    .child(format!("{pod}{DOT}{}", source.source.container)),
            );
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .flex()
            .items_center()
            .gap(SPACE_3)
            .py(SPACE_1)
            .pl(indent(depth))
            .pr(SPACE_2)
            .rounded(RADIUS)
            .cursor_pointer()
            .font_family(FONT_MONO)
            .text_size(TEXT_SMALL)
            .when(!source.on, |row| row.opacity(LOG_SOURCE_OFF))
            .hover(move |style| style.bg(palette.hover))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_source(id, cx)))
            .child(swatch)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(name)
                    .children(cluster.map(|cluster| {
                        div()
                            .truncate()
                            .text_size(TEXT_AXIS)
                            .text_color(palette.mute)
                            .child(cluster)
                    }))
                    .children(meta.map(|meta| {
                        div()
                            .truncate()
                            .text_size(TEXT_AXIS)
                            .text_color(if failing { palette.err } else { palette.mute })
                            .child(meta)
                    })),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(TEXT_LABEL)
                    .text_color(palette.mute)
                    .child(source.lines.to_string()),
            )
            .into_any_element()
    }
}

fn indent(depth: usize) -> gpui::Pixels {
    let depth = u16::try_from(depth).unwrap_or(u16::MAX);
    SPACE_2 + px(SOURCE_INDENT * f32::from(depth))
}

fn band_colors(palette: &Palette) -> [Hsla; 4] {
    [palette.err, palette.warn, palette.acc, palette.sep]
}

fn group(palette: &Palette) -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .rounded(RADIUS)
        .border(RULE)
        .border_color(palette.line2)
        .overflow_hidden()
        .font_family(FONT_MONO)
        .text_size(TEXT_SMALL)
}

fn segment(id: String, on: bool, ix: usize, palette: &Palette) -> gpui::Stateful<gpui::Div> {
    let hover = palette.hover;
    let selector = id.clone();
    div()
        .id(SharedString::from(id))
        .debug_selector(move || selector)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .h(SEGMENT)
        .px(SPACE_4)
        .cursor_pointer()
        .whitespace_nowrap()
        .when(ix > 0, |segment| {
            segment.border_l(RULE).border_color(palette.line2)
        })
        .when(on, |segment| {
            segment.bg(palette.sel).text_color(palette.selfg)
        })
        .when(!on, |segment| {
            segment
                .text_color(palette.fg2)
                .hover(move |style| style.bg(hover))
        })
}

impl Render for LogsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        if self.compact {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .bg(palette.editor)
                .child(crate::kit::PageTitle::new(TITLE).leading(self.navigation.clone()))
                .child(div().flex_1().min_h_0().child(pane(&self.logs)))
                .into_any_element();
        }
        let header = self.header(cx);
        let volume = div()
            .flex_none()
            .debug_selector(|| format!("{ID}-volume-section"))
            .child(self.volume(&palette));
        let sources = self.sources_panel(cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette.editor)
            .child(header)
            .child(volume)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(sources)
                    .child(div().flex_1().min_w_0().h_full().child(pane(&self.logs))),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use flock_core::multilog::Stamp;
    use gpui::{App, TestAppContext};

    use super::*;

    const SECOND: i128 = 1_000_000_000;

    fn source(pod: &str, container: &str) -> LogSource {
        LogSource {
            namespace: "payments".into(),
            pod: pod.into(),
            container: container.into(),
        }
    }

    fn line(id: u32, at: i128, level: LogLevel, message: &str) -> LogEvent {
        let line = flock_core::LogLine::new("2026-10-20T21:12:20.231Z", level, message);
        LogEvent::Line(SourceId(id), Some(Stamp(at * SECOND)), line)
    }

    fn shown(page: &LogsPage, cx: &App) -> Vec<String> {
        let logs = page.logs.read(cx);
        (0..logs.len())
            .filter_map(|ix| logs.line(ix).map(|line| line.message.to_string()))
            .collect()
    }

    fn open(cx: &mut TestAppContext) -> gpui::WindowHandle<LogsPage> {
        cx.update(|cx| {
            crate::init(Some(crate::theme::Appearance::Dark), cx)
                .unwrap_or_else(|error| panic!("UI init: {error}"));
        });
        let cluster = ClusterId::new("a");
        let target = LogTarget::Pods {
            cluster: cluster.clone(),
            pods: BTreeSet::new(),
        };
        let window = cx.add_window(|window, cx| LogsPage::new(target, vec![cluster], window, cx));
        let events = vec![
            LogEvent::Joined(SourceId(0), source("checkout-7f9c6d-x2lqp", "api")),
            LogEvent::Joined(SourceId(1), source("checkout-7f9c6d-m8vtn", "api")),
            line(0, 1, LogLevel::Info, "POST /v1/charge 200"),
            line(1, 2, LogLevel::Error, "stripe webhook secret not set"),
            line(0, 3, LogLevel::Warn, "stripe retry 1/3"),
            line(
                1,
                4,
                LogLevel::Info,
                r#"{"level":"error","msg":"stripe down"}"#,
            ),
        ];
        let _ = window.update(cx, |page, _, cx| {
            page.apply(0, &ClusterId::new("a"), events, cx);
        });
        window
    }

    #[gpui::test]
    fn sources_toggle_their_lines_and_levels_come_from_parsed_fields(cx: &mut TestAppContext) {
        let window = open(cx);
        let updated = window.update(cx, |page, _, cx| {
            assert_eq!(shown(page, cx).len(), 4);
            assert_eq!(page.levels.get(&LevelBand::Error), Some(&2));
            page.toggle_source(SourceId(1), cx);
            assert_eq!(shown(page, cx), ["POST /v1/charge 200", "stripe retry 1/3"]);
            assert_eq!(page.volume.summary, "2 of 4 lines · 00:00:01–00:00:04");
        });
        assert!(updated.is_ok());
    }

    #[gpui::test]
    fn search_text_steps_through_matching_lines(cx: &mut TestAppContext) {
        let window = open(cx);
        let updated = window.update(cx, |page, _, cx| {
            page.filter = TextFilter::parse("stripe");
            let phrase = logpage::search_phrase(&page.filter);
            page.logs
                .update(cx, |logs, cx| logs.set_highlight(phrase, cx));
            page.rebuild(cx);
            assert_eq!(page.match_position(), "1/3");
            assert_eq!(page.current_match(), Some(1));
            page.step_match(true, cx);
            page.step_match(true, cx);
            page.step_match(true, cx);
            assert_eq!(page.match_position(), "3/3");
            page.step_match(false, cx);
            assert_eq!(page.current_match(), Some(2));
            assert_eq!(page.logs.read(cx).current(), Some(2));
            page.filter = TextFilter::parse("@level:ERROR stripe");
            page.rebuild(cx);
            assert_eq!(shown(page, cx).len(), 2);
        });
        assert!(updated.is_ok());
    }
}
