use std::collections::{BTreeMap, BTreeSet};

use crate::filter::TextFilter;
use crate::logparse;
use crate::pods::{Container, ContainerState};
use crate::query::{Facets, NAMESPACE_FACET};
use crate::{LogLevel, Status};

pub const LEVEL_FACET: &str = "level";
pub const CLUSTER_FACET: &str = "cluster";
pub const WORKLOAD_FACET: &str = "workload";
pub const POD_FACET: &str = "pod";
pub const CONTAINER_FACET: &str = "container";
pub const VOLUME_BUCKETS: usize = 60;
pub const VOLUME_TICKS: usize = 5;
pub const RUNNING: &str = "Running";

const NANOS_PER_SECOND: i128 = 1_000_000_000;
const SECONDS_PER_MINUTE: i128 = 60;
const SECONDS_PER_HOUR: i128 = 3600;
const SECONDS_PER_DAY: i128 = 86_400;
const SECONDS_TICKS_BELOW: i128 = 5 * SECONDS_PER_MINUTE * NANOS_PER_SECOND;
const TIME_MARK: char = 'T';
const ZULU: char = 'Z';
const OFFSET_MARKS: [char; 2] = ['+', '-'];
const MILLIS_END: usize = 12;
const NAME_SEPARATOR: char = '-';
const REPLICA_SET: &str = "ReplicaSet";
const JOB: &str = "Job";
const WAITING: &str = "Waiting";
const TERMINATED: &str = "Terminated";
const META_SEPARATOR: &str = " · ";

pub fn level_word(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Debug => "DEBUG",
        LogLevel::Info => "INFO",
        LogLevel::Warn => "WARN",
        LogLevel::Error => "ERROR",
        LogLevel::Fatal => "FATAL",
    }
}

pub fn detect_level(stated: Option<LogLevel>, message: &str) -> Option<LogLevel> {
    let parsed = logparse::parse(message).and_then(|parsed| parsed.level);
    let leading = || {
        message
            .split_whitespace()
            .next()
            .map(|word| word.trim_matches(|c: char| !c.is_ascii_alphabetic()))
            .and_then(LogLevel::from_word)
    };
    parsed.or(stated).or_else(leading)
}

pub fn short_time(stamp: &str) -> &str {
    let Some((_, time)) = stamp.split_once(TIME_MARK) else {
        return stamp;
    };
    let time = time.trim_end_matches(ZULU);
    let zone = time
        .char_indices()
        .skip(1)
        .find(|(_, c)| OFFSET_MARKS.contains(c))
        .map_or(time.len(), |(at, _)| at);
    let time = time.get(..zone).unwrap_or(time);
    time.get(..MILLIS_END).unwrap_or(time)
}

pub fn workload_name<'a>(owner: Option<(&str, &'a str)>, pod: &'a str) -> &'a str {
    match owner {
        Some((REPLICA_SET, name)) => name
            .rsplit_once(NAME_SEPARATOR)
            .map_or(name, |(head, _)| head),
        Some((JOB, name)) => name,
        Some((_, name)) => name,
        None => pod,
    }
}

pub fn source_status(container: &Container) -> String {
    let healthy = container.state == ContainerState::Running && container.status != Status::Err;
    if healthy {
        return RUNNING.to_owned();
    }
    if !container.reason.is_empty() {
        return container.reason.clone();
    }
    match container.state {
        ContainerState::Running => RUNNING.to_owned(),
        ContainerState::Waiting => WAITING.to_owned(),
        ContainerState::Terminated => TERMINATED.to_owned(),
    }
}

pub fn source_meta(restarts: u32, status: &str, grouped_by_status: bool) -> Option<String> {
    let running = status == RUNNING;
    if grouped_by_status && running {
        return None;
    }
    let noun = if restarts == 1 { "restart" } else { "restarts" };
    let restarts = format!("{restarts} {noun}");
    if grouped_by_status || running {
        Some(restarts)
    } else {
        Some(format!("{restarts}{META_SEPARATOR}{status}"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineFacts<'a> {
    pub level: Option<LogLevel>,
    pub cluster: &'a str,
    pub namespace: &'a str,
    pub workload: &'a str,
    pub pod: &'a str,
    pub container: &'a str,
}

impl LineFacts<'_> {
    fn value(&self, facet: &str) -> Option<&str> {
        match facet {
            LEVEL_FACET => self.level.map(level_word),
            CLUSTER_FACET => Some(self.cluster),
            NAMESPACE_FACET => Some(self.namespace),
            WORKLOAD_FACET => Some(self.workload),
            POD_FACET => Some(self.pod),
            CONTAINER_FACET => Some(self.container),
            _ => None,
        }
    }
}

pub fn line_matches(filter: &TextFilter, facts: &LineFacts<'_>) -> bool {
    filter.terms_match(|term| {
        facts
            .value(&term.facet)
            .is_some_and(|value| term.matches_value(value))
    })
}

pub fn search_phrase(filter: &TextFilter) -> String {
    filter.words.join(" ")
}

pub fn contains_phrase(text: &str, phrase: &str) -> bool {
    !phrase.is_empty() && text.to_lowercase().contains(&phrase.to_lowercase())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceCount<'a> {
    pub facts: LineFacts<'a>,
    pub lines: usize,
}

pub fn facets<'a>(
    sources: impl IntoIterator<Item = SourceCount<'a>>,
    levels: &BTreeMap<LevelBand, usize>,
    several_clusters: bool,
) -> Facets {
    let mut facets = Facets::default();
    for band in LevelBand::ALL {
        let count = levels.get(&band).copied().unwrap_or_default();
        facets.add(LEVEL_FACET, band.word(), count);
    }
    for source in sources {
        let line = source.facts;
        if several_clusters {
            facets.add(CLUSTER_FACET, line.cluster, source.lines);
        }
        facets.add(NAMESPACE_FACET, line.namespace, source.lines);
        facets.add(WORKLOAD_FACET, line.workload, source.lines);
        facets.add(POD_FACET, line.pod, source.lines);
        facets.add(CONTAINER_FACET, line.container, source.lines);
    }
    facets
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LevelBand {
    Error,
    Warn,
    Info,
    Debug,
}

impl LevelBand {
    pub const ALL: [Self; 4] = [Self::Error, Self::Warn, Self::Info, Self::Debug];

    pub fn of(level: Option<LogLevel>) -> Self {
        match level {
            Some(LogLevel::Fatal) => Self::Error,
            Some(LogLevel::Error) => Self::Error,
            Some(LogLevel::Warn) => Self::Warn,
            Some(LogLevel::Info) => Self::Info,
            Some(LogLevel::Debug) => Self::Debug,
            None => Self::Info,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bucket {
    pub error: usize,
    pub warn: usize,
    pub info: usize,
    pub debug: usize,
}

impl Bucket {
    pub fn total(&self) -> usize {
        self.error + self.warn + self.info + self.debug
    }

    pub fn count(&self, band: LevelBand) -> usize {
        match band {
            LevelBand::Error => self.error,
            LevelBand::Warn => self.warn,
            LevelBand::Info => self.info,
            LevelBand::Debug => self.debug,
        }
    }

    fn add(&mut self, band: LevelBand) {
        match band {
            LevelBand::Error => self.error += 1,
            LevelBand::Warn => self.warn += 1,
            LevelBand::Info => self.info += 1,
            LevelBand::Debug => self.debug += 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeWindow {
    pub start: i128,
    pub end: i128,
}

impl TimeWindow {
    pub fn contains(&self, at: i128) -> bool {
        self.start <= at && at <= self.end
    }

    pub fn span(&self) -> i128 {
        self.end - self.start
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TimeRange {
    #[default]
    Live,
    Minutes15,
    Hour,
    Hours6,
}

impl TimeRange {
    pub const ALL: [Self; 4] = [Self::Live, Self::Minutes15, Self::Hour, Self::Hours6];

    pub fn label(self) -> &'static str {
        match self {
            Self::Live => "Live",
            Self::Minutes15 => "15m",
            Self::Hour => "1h",
            Self::Hours6 => "6h",
        }
    }

    fn seconds(self) -> Option<i128> {
        match self {
            Self::Live => None,
            Self::Minutes15 => Some(15 * SECONDS_PER_MINUTE),
            Self::Hour => Some(SECONDS_PER_HOUR),
            Self::Hours6 => Some(6 * SECONDS_PER_HOUR),
        }
    }

    pub fn window(self, oldest: i128, newest: i128) -> TimeWindow {
        let start = self
            .seconds()
            .map_or(oldest, |seconds| newest - seconds * NANOS_PER_SECOND);
        TimeWindow { start, end: newest }
    }
}

pub fn range_counts(stamps: &[i128]) -> Vec<(TimeRange, usize)> {
    let (Some(oldest), Some(newest)) = (stamps.iter().min(), stamps.iter().max()) else {
        return TimeRange::ALL.iter().map(|range| (*range, 0)).collect();
    };
    TimeRange::ALL
        .iter()
        .map(|range| {
            let window = range.window(*oldest, *newest);
            let count = stamps.iter().filter(|at| window.contains(**at)).count();
            (*range, count)
        })
        .collect()
}

pub fn histogram(
    lines: impl IntoIterator<Item = (i128, LevelBand)>,
    window: TimeWindow,
    buckets: usize,
) -> Vec<Bucket> {
    let mut out = vec![Bucket::default(); buckets];
    let Some(last) = buckets.checked_sub(1) else {
        return out;
    };
    let span = window.span().max(1);
    let width = i128::try_from(buckets).unwrap_or(1);
    for (at, band) in lines {
        if !window.contains(at) {
            continue;
        }
        let ix = (at - window.start) * width / span;
        let ix = usize::try_from(ix).unwrap_or(last).min(last);
        if let Some(bucket) = out.get_mut(ix) {
            bucket.add(band);
        }
    }
    out
}

#[allow(clippy::cast_precision_loss)]
pub fn share(count: usize, max: usize) -> f32 {
    if max == 0 {
        return 0.;
    }
    count.min(max) as f32 / max as f32
}

pub fn clock(at: i128, seconds: bool) -> String {
    let total = (at / NANOS_PER_SECOND).rem_euclid(SECONDS_PER_DAY);
    let hours = total / SECONDS_PER_HOUR;
    let minutes = total % SECONDS_PER_HOUR / SECONDS_PER_MINUTE;
    if seconds {
        let secs = total % SECONDS_PER_MINUTE;
        format!("{hours:02}:{minutes:02}:{secs:02}")
    } else {
        format!("{hours:02}:{minutes:02}")
    }
}

pub fn ticks(window: TimeWindow, count: usize) -> Vec<String> {
    let seconds = window.span() < SECONDS_TICKS_BELOW;
    let Some(steps) = count.checked_sub(1).filter(|steps| *steps > 0) else {
        return vec![clock(window.start, seconds)];
    };
    let steps = i128::try_from(steps).unwrap_or(1);
    (0..=steps)
        .map(|step| clock(window.start + window.span() * step / steps, seconds))
        .collect()
}

pub fn volume_summary(shown: usize, total: usize, window: Option<TimeWindow>) -> String {
    let lines = format!("{shown} of {total} lines");
    match window {
        Some(window) => {
            let seconds = window.span() < SECONDS_TICKS_BELOW;
            let start = clock(window.start, seconds);
            let end = clock(window.end, seconds);
            format!("{lines} · {start}–{end}")
        }
        None => lines,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GroupKey {
    Status,
    Workload,
    Cluster,
    Node,
}

impl GroupKey {
    pub const ALL: [Self; 4] = [Self::Status, Self::Workload, Self::Cluster, Self::Node];

    pub fn label(self) -> &'static str {
        match self {
            Self::Status => "Status",
            Self::Workload => "Workload",
            Self::Cluster => "Cluster",
            Self::Node => "Node",
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Workload => "workload",
            Self::Cluster => "cluster",
            Self::Node => "node",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceFacts {
    pub status: String,
    pub workload: String,
    pub cluster: String,
    pub node: String,
    pub on: bool,
}

impl SourceFacts {
    fn value(&self, key: GroupKey) -> &str {
        match key {
            GroupKey::Status => &self.status,
            GroupKey::Workload => &self.workload,
            GroupKey::Cluster => &self.cluster,
            GroupKey::Node => &self.node,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceRow {
    Head {
        key: GroupKey,
        name: String,
        path: String,
        depth: usize,
        open: bool,
        on: usize,
        total: usize,
    },
    Source {
        ix: usize,
        depth: usize,
    },
}

pub fn group_sources(
    sources: &[SourceFacts],
    keys: &[GroupKey],
    closed: &BTreeSet<String>,
) -> Vec<SourceRow> {
    let all: Vec<usize> = (0..sources.len()).collect();
    let mut rows = Vec::new();
    build_groups(sources, keys, closed, &all, 0, "", &mut rows);
    rows
}

fn build_groups(
    sources: &[SourceFacts],
    keys: &[GroupKey],
    closed: &BTreeSet<String>,
    members: &[usize],
    depth: usize,
    path: &str,
    rows: &mut Vec<SourceRow>,
) {
    let Some(key) = keys.get(depth).copied() else {
        rows.extend(
            members
                .iter()
                .map(|ix| SourceRow::Source { ix: *ix, depth }),
        );
        return;
    };
    let mut groups: BTreeMap<(bool, &str), Vec<usize>> = BTreeMap::new();
    for ix in members {
        let Some(source) = sources.get(*ix) else {
            continue;
        };
        let value = source.value(key);
        let healthy = key == GroupKey::Status && value == RUNNING;
        groups.entry((healthy, value)).or_default().push(*ix);
    }
    for ((_, name), group) in groups {
        let path = format!("{path}/{}={name}", key.word());
        let open = !closed.contains(&path);
        let on = group
            .iter()
            .filter(|ix| sources.get(**ix).is_some_and(|source| source.on))
            .count();
        rows.push(SourceRow::Head {
            key,
            name: name.to_owned(),
            path: path.clone(),
            depth,
            open,
            on,
            total: group.len(),
        });
        if open {
            build_groups(sources, keys, closed, &group, depth + 1, &path, rows);
        }
    }
}

pub fn toggle_key(keys: &[GroupKey], key: GroupKey) -> Vec<GroupKey> {
    if keys.contains(&key) {
        keys.iter()
            .copied()
            .filter(|chosen| *chosen != key)
            .collect()
    } else {
        keys.iter().copied().chain([key]).collect()
    }
}

pub fn group_label(keys: &[GroupKey]) -> String {
    if keys.is_empty() {
        return "none".to_owned();
    }
    keys.iter()
        .map(|key| key.word())
        .collect::<Vec<_>>()
        .join(" › ")
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MatchCursor {
    lines: Vec<usize>,
    current: usize,
}

impl MatchCursor {
    pub fn new(lines: Vec<usize>) -> Self {
        Self { lines, current: 0 }
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn line(&self) -> Option<usize> {
        self.lines.get(self.current).copied()
    }

    pub fn position(&self) -> String {
        if self.lines.is_empty() {
            return "0/0".to_owned();
        }
        format!("{}/{}", self.current + 1, self.lines.len())
    }

    pub fn next(&mut self) {
        let last = self.lines.len().saturating_sub(1);
        self.current = (self.current + 1).min(last);
    }

    pub fn previous(&mut self) {
        self.current = self.current.saturating_sub(1);
    }

    pub fn extend(&mut self, lines: impl IntoIterator<Item = usize>) {
        self.lines.extend(lines);
    }

    pub fn keep_near(&mut self, lines: Vec<usize>, line: Option<usize>) {
        let current = line.map_or(0, |line| lines.partition_point(|hit| *hit < line));
        self.current = current.min(lines.len().saturating_sub(1));
        self.lines = lines;
    }
}

#[cfg(test)]
mod tests {
    use crate::query::Term;

    use super::*;

    const SECOND: i128 = NANOS_PER_SECOND;

    fn facts(
        level: Option<LogLevel>,
        cluster: &'static str,
        pod: &'static str,
    ) -> LineFacts<'static> {
        LineFacts {
            level,
            cluster,
            namespace: "payments",
            workload: "checkout-api",
            pod,
            container: "api",
        }
    }

    #[test]
    fn levels_come_from_parsed_fields_before_the_stated_level_and_leading_word() {
        let json = r#"{"level":"error","msg":"boom"}"#;
        assert_eq!(
            detect_level(Some(LogLevel::Info), json),
            Some(LogLevel::Error)
        );
        assert_eq!(
            detect_level(Some(LogLevel::Warn), "plain text"),
            Some(LogLevel::Warn)
        );
        assert_eq!(
            detect_level(None, "[WARN] disk almost full"),
            Some(LogLevel::Warn)
        );
        assert_eq!(detect_level(None, "GET /healthz 200"), None);
    }

    #[test]
    fn source_meta_names_restarts_and_a_failing_state() {
        assert_eq!(
            source_meta(14, "CrashLoopBackOff", false).as_deref(),
            Some("14 restarts · CrashLoopBackOff")
        );
        assert_eq!(source_meta(1, RUNNING, false).as_deref(), Some("1 restart"));
        assert_eq!(
            source_meta(14, "CrashLoopBackOff", true).as_deref(),
            Some("14 restarts")
        );
        assert_eq!(source_meta(0, RUNNING, true), None);
        let crashing = Container {
            state: ContainerState::Waiting,
            status: Status::Err,
            reason: "CrashLoopBackOff".into(),
            ..Container::default()
        };
        assert_eq!(source_status(&crashing), "CrashLoopBackOff");
        let running = Container {
            state: ContainerState::Running,
            status: Status::Ok,
            ..Container::default()
        };
        assert_eq!(source_status(&running), RUNNING);
    }

    #[test]
    fn timestamps_shorten_to_the_time_of_day() {
        assert_eq!(short_time("2026-10-20T21:12:20.231456Z"), "21:12:20.231");
        assert_eq!(short_time("2026-10-20T21:12:20+02:00"), "21:12:20");
        assert_eq!(short_time("21:12:20.231"), "21:12:20.231");
    }

    #[test]
    fn workloads_are_named_from_the_pod_owner() {
        let rs = Some((REPLICA_SET, "checkout-api-7f9c6d"));
        assert_eq!(
            workload_name(rs, "checkout-api-7f9c6d-x2lqp"),
            "checkout-api"
        );
        assert_eq!(workload_name(Some(("StatefulSet", "db")), "db-0"), "db");
        assert_eq!(workload_name(None, "debug"), "debug");
    }

    #[test]
    fn facet_chips_filter_lines_and_free_text_does_not() {
        let line = facts(Some(LogLevel::Error), "prod-us-east", "x2lqp");
        let error = TextFilter {
            terms: vec![Term::include(LEVEL_FACET, "error")],
            ..TextFilter::default()
        };
        assert!(line_matches(&error, &line));
        let warn_or_info = TextFilter {
            terms: vec![
                Term::include(LEVEL_FACET, "WARN"),
                Term::include(LEVEL_FACET, "INFO"),
            ],
            ..TextFilter::default()
        };
        assert!(!line_matches(&warn_or_info, &line));
        let not_east = TextFilter {
            terms: vec![Term::exclude(CLUSTER_FACET, "prod-us-east")],
            ..TextFilter::default()
        };
        assert!(!line_matches(&not_east, &line));
        let text = TextFilter::parse("stripe secret");
        assert!(line_matches(&text, &line));
        assert_eq!(search_phrase(&text), "stripe secret");
        assert!(contains_phrase("Stripe Secret not set", "stripe secret"));
        assert!(!contains_phrase("anything", ""));
    }

    #[test]
    fn facets_count_lines_per_value_and_cluster_only_when_several() {
        let sources = [
            SourceCount {
                facts: facts(None, "a", "x2lqp"),
                lines: 3,
            },
            SourceCount {
                facts: facts(None, "a", "m8vtn"),
                lines: 2,
            },
        ];
        let levels = BTreeMap::from([(LevelBand::Error, 4), (LevelBand::Info, 1)]);
        let one = facets(sources, &levels, false);
        assert_eq!(one.values(POD_FACET), [("x2lqp", 3), ("m8vtn", 2)]);
        assert_eq!(one.values(LEVEL_FACET), [("ERROR", 4), ("INFO", 1)]);
        assert_eq!(one.values(CLUSTER_FACET), []);
        let several = facets(sources, &levels, true);
        assert_eq!(several.values(CLUSTER_FACET), [("a", 5)]);
    }

    #[test]
    fn ranges_end_at_the_newest_line_and_count_what_they_hold() {
        let newest = 7200 * SECOND;
        let stamps = [0, 5000 * SECOND, 6900 * SECOND, newest];
        let counts = range_counts(&stamps);
        assert_eq!(
            counts,
            [
                (TimeRange::Live, 4),
                (TimeRange::Minutes15, 2),
                (TimeRange::Hour, 3),
                (TimeRange::Hours6, 4),
            ]
        );
        assert_eq!(
            TimeRange::Live.window(0, newest),
            TimeWindow {
                start: 0,
                end: newest
            }
        );
        assert_eq!(range_counts(&[]).first(), Some(&(TimeRange::Live, 0)));
    }

    #[test]
    fn the_histogram_buckets_lines_by_time_and_stacks_them_by_level() {
        let window = TimeWindow {
            start: 0,
            end: 10 * SECOND,
        };
        let lines = [
            (0, LevelBand::Info),
            (SECOND, LevelBand::Error),
            (9 * SECOND, LevelBand::Warn),
            (10 * SECOND, LevelBand::Debug),
            (11 * SECOND, LevelBand::Error),
        ];
        let buckets = histogram(lines, window, 5);
        assert_eq!(buckets.len(), 5);
        assert_eq!(buckets.first().map(Bucket::total), Some(2));
        assert_eq!(buckets.first().map(|b| b.count(LevelBand::Error)), Some(1));
        assert_eq!(buckets.last().map(|b| (b.warn, b.debug)), Some((1, 1)));
        assert_eq!(buckets.iter().map(Bucket::total).sum::<usize>(), 4);
        assert_eq!(histogram(lines, window, 0), []);
    }

    #[test]
    fn bar_shares_are_fractions_of_the_tallest_bucket() {
        assert!((share(1, 4) - 0.25).abs() < f32::EPSILON);
        assert!((share(9, 4) - 1.).abs() < f32::EPSILON);
        assert!(share(3, 0).abs() < f32::EPSILON);
    }

    #[test]
    fn ticks_and_summary_name_the_window_in_minutes_or_seconds() {
        let hour = TimeWindow {
            start: (20 * 3600 + 40 * 60) * SECOND,
            end: (21 * 3600 + 40 * 60) * SECOND,
        };
        assert_eq!(
            ticks(hour, 5),
            ["20:40", "20:55", "21:10", "21:25", "21:40"]
        );
        assert_eq!(
            volume_summary(29, 1284, Some(hour)),
            "29 of 1284 lines · 20:40–21:40"
        );
        let short = TimeWindow {
            start: 76_440 * SECOND,
            end: 76_460 * SECOND,
        };
        assert_eq!(ticks(short, 2), ["21:14:00", "21:14:20"]);
        assert_eq!(volume_summary(0, 0, None), "0 of 0 lines");
    }

    fn source(status: &str, workload: &str, on: bool) -> SourceFacts {
        SourceFacts {
            status: status.to_owned(),
            workload: workload.to_owned(),
            cluster: "a".to_owned(),
            node: "n1".to_owned(),
            on,
        }
    }

    #[test]
    fn sources_group_by_nested_keys_with_running_last_and_closed_groups_folded() {
        let sources = [
            source(RUNNING, "checkout-api", true),
            source("CrashLoopBackOff", "checkout-api", true),
            source(RUNNING, "payments-worker", false),
        ];
        let keys = [GroupKey::Status, GroupKey::Workload];
        let rows = group_sources(&sources, &keys, &BTreeSet::new());
        let heads: Vec<(String, usize, usize, usize)> = rows
            .iter()
            .filter_map(|row| match row {
                SourceRow::Head {
                    name,
                    depth,
                    on,
                    total,
                    ..
                } => Some((name.clone(), *depth, *on, *total)),
                SourceRow::Source { .. } => None,
            })
            .collect();
        assert_eq!(
            heads,
            [
                ("CrashLoopBackOff".to_owned(), 0, 1, 1),
                ("checkout-api".to_owned(), 1, 1, 1),
                (RUNNING.to_owned(), 0, 1, 2),
                ("checkout-api".to_owned(), 1, 1, 1),
                ("payments-worker".to_owned(), 1, 0, 1),
            ]
        );
        assert!(rows.contains(&SourceRow::Source { ix: 1, depth: 2 }));
        let closed = BTreeSet::from(["/status=Running".to_owned()]);
        let folded = group_sources(&sources, &keys, &closed);
        assert_eq!(folded.len(), 4);
        let flat = group_sources(&sources, &[], &BTreeSet::new());
        assert_eq!(flat.len(), 3);
        assert_eq!(toggle_key(&keys, GroupKey::Status), [GroupKey::Workload]);
        assert_eq!(
            toggle_key(&[GroupKey::Status], GroupKey::Node),
            [GroupKey::Status, GroupKey::Node]
        );
        assert_eq!(group_label(&keys), "status › workload");
        assert_eq!(group_label(&[]), "none");
    }

    #[test]
    fn the_match_cursor_steps_within_bounds_and_stays_near_after_a_rebuild() {
        let mut cursor = MatchCursor::new(vec![2, 5, 9]);
        assert_eq!(cursor.position(), "1/3");
        cursor.previous();
        assert_eq!(cursor.line(), Some(2));
        cursor.next();
        cursor.next();
        cursor.next();
        assert_eq!(
            (cursor.line(), cursor.position()),
            (Some(9), "3/3".to_owned())
        );
        cursor.keep_near(vec![1, 4, 6, 8], Some(5));
        assert_eq!(cursor.line(), Some(6));
        cursor.keep_near(Vec::new(), Some(5));
        assert_eq!((cursor.line(), cursor.position()), (None, "0/0".to_owned()));
        cursor.extend([3]);
        assert_eq!(cursor.line(), Some(3));
    }
}
