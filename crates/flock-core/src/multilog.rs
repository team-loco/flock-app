use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::filter::LabelMatch;
use crate::stream::{LogLine, Sink};
use crate::{ClusterId, ResourceRef};

pub const MERGED_LINES: usize = 10_000;
const REQUIREMENT_SEPARATOR: char = ',';
const NOT_EQUALS: &str = "!=";
const DOUBLE_EQUALS: &str = "==";
const EQUALS: char = '=';
const ABSENT: char = '!';
const SET_OPEN: char = '(';
const PODS_LABEL: &str = "pods";

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SelectorError {
    #[error("the label selector is empty")]
    Empty,
    #[error("unsupported label selector requirement \"{0}\"")]
    Unsupported(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Requirement {
    Equals(String, String),
    NotEquals(String, String),
    Exists(String),
    Absent(String),
}

impl Requirement {
    fn parse(text: &str) -> Result<Self, SelectorError> {
        let unsupported = || SelectorError::Unsupported(text.to_owned());
        if text.contains(SET_OPEN) {
            return Err(unsupported());
        }
        let pair = |key: &str, value: &str| {
            let key = key.trim();
            (!key.is_empty()).then(|| (key.to_owned(), value.trim().to_owned()))
        };
        if let Some((key, value)) = text.split_once(NOT_EQUALS) {
            let (key, value) = pair(key, value).ok_or_else(unsupported)?;
            return Ok(Self::NotEquals(key, value));
        }
        if let Some((key, value)) = text.split_once(DOUBLE_EQUALS) {
            let (key, value) = pair(key, value).ok_or_else(unsupported)?;
            return Ok(Self::Equals(key, value));
        }
        if let Some((key, value)) = text.split_once(EQUALS) {
            let (key, value) = pair(key, value).ok_or_else(unsupported)?;
            return Ok(Self::Equals(key, value));
        }
        if let Some(key) = text.strip_prefix(ABSENT) {
            let key = key.trim();
            if key.is_empty() {
                return Err(unsupported());
            }
            return Ok(Self::Absent(key.to_owned()));
        }
        if text.contains(char::is_whitespace) {
            return Err(unsupported());
        }
        Ok(Self::Exists(text.to_owned()))
    }

    fn matches(&self, labels: &BTreeMap<String, String>) -> bool {
        match self {
            Self::Equals(key, value) => labels.get(key) == Some(value),
            Self::NotEquals(key, value) => labels.get(key) != Some(value),
            Self::Exists(key) => labels.contains_key(key),
            Self::Absent(key) => !labels.contains_key(key),
        }
    }
}

impl fmt::Display for Requirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Equals(key, value) => write!(f, "{key}{EQUALS}{value}"),
            Self::NotEquals(key, value) => write!(f, "{key}{NOT_EQUALS}{value}"),
            Self::Exists(key) => f.write_str(key),
            Self::Absent(key) => write!(f, "{ABSENT}{key}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LabelSelector(Vec<Requirement>);

impl LabelSelector {
    pub fn parse(text: &str) -> Result<Self, SelectorError> {
        let requirements = text
            .split(REQUIREMENT_SEPARATOR)
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(Requirement::parse)
            .collect::<Result<Vec<_>, _>>()?;
        if requirements.is_empty() {
            return Err(SelectorError::Empty);
        }
        Ok(Self(requirements))
    }

    pub fn from_matches(matches: &[LabelMatch]) -> Option<Self> {
        let requirements: Vec<Requirement> = matches
            .iter()
            .map(|(key, value)| match value {
                Some(value) => Requirement::Equals(key.clone(), value.clone()),
                None => Requirement::Exists(key.clone()),
            })
            .collect();
        (!requirements.is_empty()).then_some(Self(requirements))
    }

    pub fn matches(&self, labels: &BTreeMap<String, String>) -> bool {
        self.0.iter().all(|requirement| requirement.matches(labels))
    }
}

impl fmt::Display for LabelSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self.0.iter().map(ToString::to_string).collect();
        f.write_str(&parts.join(&REQUIREMENT_SEPARATOR.to_string()))
    }
}

impl Serialize for LabelSelector {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for LabelSelector {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PodFacts<'a> {
    pub namespace: &'a str,
    pub name: &'a str,
    pub owner_kind: Option<&'a str>,
    pub owner_name: Option<&'a str>,
    pub labels: &'a BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogTarget {
    Workload(ResourceRef),
    Selector {
        cluster: ClusterId,
        namespace: Option<String>,
        selector: LabelSelector,
    },
    Pods {
        cluster: ClusterId,
        pods: BTreeSet<(String, String)>,
    },
}

impl LogTarget {
    pub fn pods(refs: &[ResourceRef]) -> Option<Self> {
        let cluster = refs.first()?.cluster.clone();
        let mut pods = BTreeSet::new();
        for pod in refs {
            let namespace = pod.namespace.clone()?;
            if pod.cluster != cluster {
                return None;
            }
            pods.insert((namespace, pod.name.clone()));
        }
        Some(Self::Pods { cluster, pods })
    }

    pub fn cluster(&self) -> &ClusterId {
        match self {
            Self::Workload(target) => &target.cluster,
            Self::Selector { cluster, .. } => cluster,
            Self::Pods { cluster, .. } => cluster,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Workload(target) => target.name.clone(),
            Self::Selector { selector, .. } => selector.to_string(),
            Self::Pods { pods, .. } => match pods.iter().next() {
                Some((_, name)) if pods.len() == 1 => name.clone(),
                Some(_) => format!("{} {PODS_LABEL}", pods.len()),
                None => PODS_LABEL.to_owned(),
            },
        }
    }

    pub fn namespace(&self) -> Option<&str> {
        match self {
            Self::Workload(target) => target.namespace.as_deref(),
            Self::Selector { namespace, .. } => namespace.as_deref(),
            Self::Pods { pods, .. } => {
                let mut namespaces = pods.iter().map(|(namespace, _)| namespace.as_str());
                let first = namespaces.next()?;
                namespaces.all(|other| other == first).then_some(first)
            }
        }
    }

    pub fn on(&self, cluster: &ClusterId) -> Self {
        match self {
            Self::Workload(target) => Self::Workload(ResourceRef {
                cluster: cluster.clone(),
                ..target.clone()
            }),
            Self::Selector {
                namespace,
                selector,
                ..
            } => Self::Selector {
                cluster: cluster.clone(),
                namespace: namespace.clone(),
                selector: selector.clone(),
            },
            Self::Pods { pods, .. } => Self::Pods {
                cluster: cluster.clone(),
                pods: pods.clone(),
            },
        }
    }

    pub fn follows_new_pods(&self) -> bool {
        match self {
            Self::Workload(_) => true,
            Self::Selector { .. } => true,
            Self::Pods { .. } => false,
        }
    }

    pub fn matches(&self, pod: &PodFacts<'_>) -> bool {
        match self {
            Self::Workload(target) => {
                target.namespace.as_deref() == Some(pod.namespace)
                    && pod.owner_kind == Some(target.kind.as_str())
                    && pod.owner_name == Some(target.name.as_str())
            }
            Self::Selector {
                namespace,
                selector,
                ..
            } => {
                namespace.as_deref().is_none_or(|ns| ns == pod.namespace)
                    && selector.matches(pod.labels)
            }
            Self::Pods { pods, .. } => pods
                .iter()
                .any(|(namespace, name)| namespace == pod.namespace && name == pod.name),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LogSource {
    pub namespace: String,
    pub pod: String,
    pub container: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Stamp(pub i128);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogEvent {
    Joined(SourceId, LogSource),
    Line(SourceId, Option<Stamp>, LogLine),
    Ended(SourceId),
    Left(SourceId),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiLogRequest {
    pub target: LogTarget,
    pub sink: Sink<LogEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry<T> {
    pub key: Stamp,
    pub source: SourceId,
    pub item: T,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    End,
    At(usize),
    Dropped,
}

#[derive(Clone, Debug)]
pub struct MergedLog<T> {
    entries: VecDeque<Entry<T>>,
    capacity: usize,
    last: HashMap<SourceId, Stamp>,
}

impl<T> Default for MergedLog<T> {
    fn default() -> Self {
        Self::with_capacity(MERGED_LINES)
    }
}

impl<T> MergedLog<T> {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity,
            last: HashMap::new(),
        }
    }

    pub fn insert(&mut self, at: Option<Stamp>, source: SourceId, item: T) -> Placement {
        if self.capacity == 0 {
            return Placement::Dropped;
        }
        let carried = self.last.get(&source).copied();
        let newest = self.entries.back().map(|entry| entry.key);
        let key = at.or(carried).or(newest).unwrap_or(Stamp(i128::MIN));
        if let Some(at) = at {
            self.last.insert(source, at);
        }
        let mut ix = self.entries.partition_point(|entry| entry.key <= key);
        if self.entries.len() == self.capacity {
            if ix == 0 {
                return Placement::Dropped;
            }
            self.entries.pop_front();
            ix -= 1;
        }
        self.entries.insert(ix, Entry { key, source, item });
        if ix + 1 == self.entries.len() {
            Placement::End
        } else {
            Placement::At(ix)
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Entry<T>> + ExactSizeIterator {
        self.entries.iter()
    }

    pub fn get(&self, ix: usize) -> Option<&Entry<T>> {
        self.entries.get(ix)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.last.clear();
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColorSlots {
    count: usize,
    assigned: BTreeMap<String, usize>,
    active: BTreeSet<String>,
}

impl ColorSlots {
    pub fn new(count: usize) -> Self {
        Self {
            count,
            ..Self::default()
        }
    }

    pub fn assign(&mut self, key: &str) -> usize {
        self.active.insert(key.to_owned());
        if let Some(slot) = self.assigned.get(key) {
            return *slot;
        }
        let used: BTreeSet<usize> = self
            .active
            .iter()
            .filter_map(|active| self.assigned.get(active).copied())
            .collect();
        let free = (0..self.count).find(|slot| !used.contains(slot));
        let slot = free.unwrap_or_else(|| self.assigned.len().checked_rem(self.count).unwrap_or(0));
        self.assigned.insert(key.to_owned(), slot);
        slot
    }

    pub fn release(&mut self, key: &str) {
        self.active.remove(key);
    }

    pub fn slot(&self, key: &str) -> Option<usize> {
        self.assigned.get(key).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn deployment(name: &str) -> ResourceRef {
        ResourceRef {
            cluster: ClusterId::new("kind-fleet-a"),
            kind: "Deployment".into(),
            namespace: Some("web".into()),
            name: name.into(),
        }
    }

    fn facts<'a>(
        namespace: &'a str,
        name: &'a str,
        owner: Option<(&'a str, &'a str)>,
        labels: &'a BTreeMap<String, String>,
    ) -> PodFacts<'a> {
        PodFacts {
            namespace,
            name,
            owner_kind: owner.map(|(kind, _)| kind),
            owner_name: owner.map(|(_, name)| name),
            labels,
        }
    }

    fn merged(capacity: usize, lines: &[(u32, Option<i128>, &'static str)]) -> Vec<&'static str> {
        let mut log = MergedLog::with_capacity(capacity);
        for (source, at, text) in lines {
            log.insert(at.map(Stamp), SourceId(*source), *text);
        }
        log.iter().map(|entry| entry.item).collect()
    }

    #[test]
    fn lines_from_several_pods_merge_in_timestamp_order() {
        let order = merged(
            10,
            &[
                (0, Some(10), "a1"),
                (0, Some(30), "a2"),
                (0, Some(50), "a3"),
                (1, Some(20), "b1"),
                (1, Some(40), "b2"),
                (1, Some(60), "b3"),
            ],
        );
        assert_eq!(order, ["a1", "b1", "a2", "b2", "a3", "b3"]);
    }

    #[test]
    fn equal_timestamps_keep_arrival_order() {
        let order = merged(
            10,
            &[
                (0, Some(5), "first"),
                (1, Some(5), "second"),
                (0, Some(5), "third"),
            ],
        );
        assert_eq!(order, ["first", "second", "third"]);
    }

    #[test]
    fn lines_without_a_timestamp_follow_the_last_line_of_their_pod() {
        let order = merged(
            10,
            &[
                (0, Some(10), "a1"),
                (1, Some(20), "b1"),
                (0, None, "a1 continued"),
                (1, Some(30), "b2"),
            ],
        );
        assert_eq!(order, ["a1", "a1 continued", "b1", "b2"]);
    }

    #[test]
    fn placement_says_whether_a_line_landed_at_the_end() {
        let mut log = MergedLog::with_capacity(10);
        assert_eq!(
            log.insert(Some(Stamp(10)), SourceId(0), "a"),
            Placement::End
        );
        assert_eq!(
            log.insert(Some(Stamp(20)), SourceId(0), "b"),
            Placement::End
        );
        assert_eq!(
            log.insert(Some(Stamp(15)), SourceId(1), "c"),
            Placement::At(1)
        );
        assert_eq!(log.insert(None, SourceId(2), "marker"), Placement::End);
    }

    #[test]
    fn the_merged_buffer_keeps_only_the_newest_lines() {
        let mut log = MergedLog::with_capacity(3);
        for at in 0..5 {
            log.insert(Some(Stamp(at * 10)), SourceId(0), at);
        }
        let kept: Vec<i128> = log.iter().map(|entry| entry.item).collect();
        assert_eq!(kept, [2, 3, 4]);
        assert_eq!(
            log.insert(Some(Stamp(1)), SourceId(1), 99),
            Placement::Dropped
        );
        assert_eq!(log.len(), 3);
        assert_eq!(
            log.insert(Some(Stamp(25)), SourceId(1), 25),
            Placement::At(0)
        );
        let kept: Vec<i128> = log.iter().map(|entry| entry.item).collect();
        assert_eq!(kept, [25, 3, 4]);
    }

    #[test]
    fn the_default_buffer_is_bounded_by_the_constant() {
        let mut log = MergedLog::default();
        for at in 0..=MERGED_LINES {
            log.insert(
                Some(Stamp(i128::try_from(at).unwrap_or_default())),
                SourceId(0),
                (),
            );
        }
        assert_eq!(log.len(), MERGED_LINES);
    }

    #[test]
    fn colours_stay_with_a_pod_and_free_slots_are_reused() {
        let mut slots = ColorSlots::new(3);
        assert_eq!(slots.assign("a"), 0);
        assert_eq!(slots.assign("b"), 1);
        assert_eq!(slots.assign("a"), 0);
        assert_eq!(slots.assign("c"), 2);
        slots.release("b");
        assert_eq!(slots.assign("d"), 1);
        assert_eq!(slots.slot("b"), Some(1));
        assert_eq!(slots.assign("e"), 1);
        assert_eq!(slots.slot("a"), Some(0));
    }

    #[test]
    fn colour_slots_never_exceed_the_palette() {
        let mut slots = ColorSlots::new(2);
        let assigned: Vec<usize> = ["a", "b", "c", "d"]
            .iter()
            .map(|pod| slots.assign(pod))
            .collect();
        assert!(assigned.iter().all(|slot| *slot < 2), "{assigned:?}");
        assert_eq!(ColorSlots::new(0).assign("a"), 0);
    }

    #[test]
    fn selectors_parse_equality_existence_and_absence() {
        let Ok(selector) = LabelSelector::parse("app=web, tier!=db,canary,!legacy") else {
            panic!("selector parses");
        };
        assert_eq!(selector.to_string(), "app=web,tier!=db,canary,!legacy");
        assert!(selector.matches(&labels(&[("app", "web"), ("canary", "1")])));
        assert!(!selector.matches(&labels(&[("app", "web")])));
        assert!(!selector.matches(&labels(&[("app", "web"), ("canary", ""), ("tier", "db")])));
        assert!(!selector.matches(&labels(&[("app", "web"), ("canary", ""), ("legacy", "")])));
        let Ok(double) = LabelSelector::parse("app==web") else {
            panic!("double equals parses");
        };
        assert!(double.matches(&labels(&[("app", "web")])));
        assert_eq!(LabelSelector::parse(" , "), Err(SelectorError::Empty));
        assert!(matches!(
            LabelSelector::parse("env in (a,b)"),
            Err(SelectorError::Unsupported(_))
        ));
        assert!(matches!(
            LabelSelector::parse("=web"),
            Err(SelectorError::Unsupported(_))
        ));
    }

    #[test]
    fn filter_label_tokens_become_a_selector() {
        let matches = vec![
            ("app".to_owned(), Some("web".to_owned())),
            ("canary".to_owned(), None),
        ];
        let selector = LabelSelector::from_matches(&matches).map(|s| s.to_string());
        assert_eq!(selector.as_deref(), Some("app=web,canary"));
        assert_eq!(LabelSelector::from_matches(&[]), None);
    }

    #[test]
    fn a_job_target_matches_the_pods_the_job_owns() {
        let target = LogTarget::Workload(ResourceRef {
            kind: "Job".into(),
            name: "migrate-42".into(),
            ..deployment("x")
        });
        let none = BTreeMap::new();
        assert!(target.matches(&facts(
            "web",
            "migrate-42-abcde",
            Some(("Job", "migrate-42")),
            &none
        )));
        assert!(!target.matches(&facts(
            "web",
            "migrate-41-abcde",
            Some(("Job", "migrate-41")),
            &none
        )));
        assert!(!target.matches(&facts(
            "web",
            "migrate-42",
            Some(("Deployment", "migrate-42")),
            &none
        )));
    }

    #[test]
    fn a_workload_target_matches_only_the_pods_it_owns() {
        let target = LogTarget::Workload(deployment("checkout"));
        let none = BTreeMap::new();
        let owned = facts("web", "checkout-1", Some(("Deployment", "checkout")), &none);
        let other = facts("web", "cart-1", Some(("Deployment", "cart")), &none);
        let elsewhere = facts("api", "checkout-1", Some(("Deployment", "checkout")), &none);
        assert!(target.matches(&owned));
        assert!(!target.matches(&other));
        assert!(!target.matches(&elsewhere));
        assert!(target.follows_new_pods());
        assert_eq!(target.label(), "checkout");
    }

    #[test]
    fn selector_and_pod_targets_match_by_labels_and_names() {
        let Ok(selector) = LabelSelector::parse("app=web") else {
            panic!("selector parses");
        };
        let target = LogTarget::Selector {
            cluster: ClusterId::new("a"),
            namespace: Some("web".into()),
            selector,
        };
        let web = labels(&[("app", "web")]);
        assert!(target.matches(&facts("web", "x", None, &web)));
        assert!(!target.matches(&facts("api", "x", None, &web)));
        assert_eq!(target.label(), "app=web");

        let pod = |name: &str| ResourceRef {
            cluster: ClusterId::new("a"),
            kind: "Pod".into(),
            namespace: Some("web".into()),
            name: name.into(),
        };
        let Some(pods) = LogTarget::pods(&[pod("x"), pod("y")]) else {
            panic!("pods target");
        };
        let none = BTreeMap::new();
        assert!(pods.matches(&facts("web", "y", None, &none)));
        assert!(!pods.matches(&facts("web", "z", None, &none)));
        assert!(!pods.follows_new_pods());
        assert_eq!(pods.label(), "2 pods");
        assert_eq!(pods.namespace(), Some("web"));
        let other = ResourceRef {
            cluster: ClusterId::new("b"),
            ..pod("z")
        };
        assert_eq!(LogTarget::pods(&[pod("x"), other]), None);
        assert_eq!(LogTarget::pods(&[]), None);
    }

    #[test]
    fn targets_survive_a_round_trip_through_json() {
        let Ok(selector) = LabelSelector::parse("app=web,!legacy") else {
            panic!("selector parses");
        };
        let targets = [
            LogTarget::Workload(deployment("checkout")),
            LogTarget::Selector {
                cluster: ClusterId::new("a"),
                namespace: None,
                selector,
            },
        ];
        for target in targets {
            let Ok(json) = serde_json::to_string(&target) else {
                panic!("serializes");
            };
            let back: Result<LogTarget, _> = serde_json::from_str(&json);
            assert_eq!(back.ok(), Some(target), "{json}");
        }
    }
}
