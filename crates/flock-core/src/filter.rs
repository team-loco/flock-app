use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use crate::Status;

pub const NAMESPACE_PREFIX: &str = "ns:";
pub const LABEL_PREFIXES: [&str; 2] = ["label:", "l:"];
pub const NODE_PREFIX: &str = "node:";
const LABEL_SEPARATOR: char = '=';
const TOKEN_KEYWORDS: [(&str, &str); 4] = [
    (NAMESPACE_PREFIX, NAMESPACE_PREFIX),
    ("namespace", NAMESPACE_PREFIX),
    (LABEL_PREFIXES[0], LABEL_PREFIXES[0]),
    (LABEL_PREFIXES[1], LABEL_PREFIXES[1]),
];

pub type LabelMatch = (String, Option<String>);

pub fn namespace_token(token: &str) -> Option<String> {
    token
        .strip_prefix(NAMESPACE_PREFIX)
        .map(str::to_ascii_lowercase)
}

pub fn node_token(token: &str) -> Option<String> {
    token.strip_prefix(NODE_PREFIX).map(str::to_ascii_lowercase)
}

pub fn node_query(node: &str) -> String {
    format!("{NODE_PREFIX}{node}")
}

pub fn label_token(token: &str) -> Option<LabelMatch> {
    let label = LABEL_PREFIXES
        .iter()
        .find_map(|prefix| token.strip_prefix(prefix))?;
    Some(match label.split_once(LABEL_SEPARATOR) {
        Some((key, value)) => (key.to_owned(), Some(value.to_owned())),
        None => (label.to_owned(), None),
    })
}

pub fn label_matches(wanted: &LabelMatch, labels: &BTreeMap<String, String>) -> bool {
    let (key, value) = wanted;
    match (labels.get(key), value) {
        (Some(actual), Some(value)) => actual == value,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

pub type LabelValues = BTreeMap<String, BTreeSet<String>>;

pub fn label_values<'a>(
    sets: impl IntoIterator<Item = &'a BTreeMap<String, String>>,
) -> LabelValues {
    let mut values = LabelValues::new();
    for (key, value) in sets.into_iter().flatten() {
        values.entry(key.clone()).or_default().insert(value.clone());
    }
    values
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FilterCandidates {
    pub namespaces: BTreeSet<String>,
    pub labels: LabelValues,
}

impl FilterCandidates {
    pub fn suggest(&self, input: &str) -> Vec<String> {
        if input.ends_with(char::is_whitespace) {
            return Vec::new();
        }
        let Some(token) = input.split_whitespace().last() else {
            return Vec::new();
        };
        if let Some(part) = token.strip_prefix(NAMESPACE_PREFIX) {
            return self
                .namespaces
                .iter()
                .filter(|name| name.starts_with(part))
                .map(|name| format!("{NAMESPACE_PREFIX}{name}"))
                .collect();
        }
        if let Some((prefix, part)) = LABEL_PREFIXES
            .iter()
            .find_map(|prefix| Some((*prefix, token.strip_prefix(prefix)?)))
        {
            return self.suggest_label(prefix, part);
        }
        let prefixes: BTreeSet<&str> = TOKEN_KEYWORDS
            .iter()
            .filter(|(keyword, _)| keyword.starts_with(token))
            .map(|(_, prefix)| *prefix)
            .collect();
        prefixes.into_iter().map(str::to_owned).collect()
    }

    fn suggest_label(&self, prefix: &str, part: &str) -> Vec<String> {
        match part.split_once(LABEL_SEPARATOR) {
            Some((key, part)) => self
                .labels
                .get(key)
                .into_iter()
                .flatten()
                .filter(|value| value.starts_with(part))
                .map(|value| format!("{prefix}{key}{LABEL_SEPARATOR}{value}"))
                .collect(),
            None => self
                .labels
                .keys()
                .filter(|key| key.starts_with(part))
                .map(|key| format!("{prefix}{key}{LABEL_SEPARATOR}"))
                .collect(),
        }
    }
}

pub fn extract_tokens(input: &str) -> (Vec<String>, String) {
    let mut tokens = Vec::new();
    let mut text = String::new();
    for part in input.split_inclusive(char::is_whitespace) {
        let token = part.trim_end();
        if part.ends_with(char::is_whitespace) && applied_token(token) {
            tokens.push(token.to_owned());
        } else {
            text.push_str(part);
        }
    }
    if text.trim().is_empty() {
        text.clear();
    }
    (tokens, text)
}

pub fn applied_token(token: &str) -> bool {
    if let Some(namespace) = namespace_token(token) {
        return !namespace.is_empty();
    }
    if let Some(node) = node_token(token) {
        return !node.is_empty();
    }
    label_token(token)
        .is_some_and(|(key, value)| !key.is_empty() && value.is_none_or(|value| !value.is_empty()))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatusFilter {
    #[default]
    All,
    Attention,
}

impl StatusFilter {
    pub const ALL: [Self; 2] = [Self::All, Self::Attention];

    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Attention => "Needs attention",
        }
    }

    pub fn at(ix: usize) -> Self {
        Self::ALL.get(ix).copied().unwrap_or_default()
    }

    pub fn keeps(self, status: Status) -> bool {
        match self {
            Self::All => true,
            Self::Attention => status != Status::Ok,
        }
    }

    pub fn counts(statuses: impl IntoIterator<Item = Status>) -> Vec<(Self, usize)> {
        let statuses: Vec<Status> = statuses.into_iter().collect();
        Self::ALL
            .iter()
            .map(|filter| {
                let count = statuses.iter().filter(|s| filter.keeps(**s)).count();
                (*filter, count)
            })
            .collect()
    }
}

const NODE_NAME: &str = "node";
const SEARCH_SEPARATOR: &str = "\n";

pub type FieldValues<'a> = Vec<Cow<'a, str>>;
pub type FieldFn<T, C = ()> = for<'a> fn(&'a T, &'a C, &mut FieldValues<'a>);

pub struct FilterSpec<T: 'static, C: 'static = ()> {
    pub fields: &'static [(&'static str, FieldFn<T, C>)],
    pub namespace: bool,
    pub labels: bool,
}

impl<T> FilterSpec<T> {
    pub fn matches(
        &self,
        row: &T,
        query: &TextFilter,
        namespace: Option<&str>,
        label_sets: &[&BTreeMap<String, String>],
    ) -> bool {
        self.matches_in(row, &(), query, namespace, label_sets)
    }

    pub fn search_text(&self, row: &T) -> String {
        let mut values = FieldValues::new();
        for (_, field) in self.fields {
            field(row, &(), &mut values);
        }
        let mut text = values.join(SEARCH_SEPARATOR);
        text.make_ascii_lowercase();
        text
    }

    pub fn matches_search(
        &self,
        row: &T,
        query: &TextFilter,
        namespace: Option<&str>,
        label_sets: &[&BTreeMap<String, String>],
        search_text: &str,
    ) -> bool {
        self.scope_matches(row, &(), query, namespace, label_sets)
            && query.matches_search_text(search_text)
    }
}

impl<T, C> FilterSpec<T, C> {
    pub fn matches_in(
        &self,
        row: &T,
        context: &C,
        query: &TextFilter,
        namespace: Option<&str>,
        label_sets: &[&BTreeMap<String, String>],
    ) -> bool {
        self.scope_matches(row, context, query, namespace, label_sets)
            && self.text_matches(row, context, query)
    }

    fn scope_matches(
        &self,
        row: &T,
        context: &C,
        query: &TextFilter,
        namespace: Option<&str>,
        label_sets: &[&BTreeMap<String, String>],
    ) -> bool {
        let namespace_ok = if self.namespace {
            query.matches_namespace(namespace)
        } else {
            query.namespace.is_none()
        };
        let labels_ok = query.labels.is_empty()
            || (self.labels && label_sets.iter().any(|labels| query.matches_labels(labels)));
        namespace_ok && labels_ok && self.node_matches(row, context, query)
    }

    fn node_matches(&self, row: &T, context: &C, query: &TextFilter) -> bool {
        let Some(wanted) = &query.node else {
            return true;
        };
        let mut values = FieldValues::new();
        for (_, field) in self.fields.iter().filter(|(name, _)| *name == NODE_NAME) {
            field(row, context, &mut values);
        }
        values.iter().any(|node| node.eq_ignore_ascii_case(wanted))
    }

    fn text_matches(&self, row: &T, context: &C, query: &TextFilter) -> bool {
        let mut values = FieldValues::new();
        for (_, field) in self.fields {
            field(row, context, &mut values);
        }
        let fields: Vec<&str> = values.iter().map(AsRef::as_ref).collect();
        query.matches_text(&fields)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextFilter {
    pub words: Vec<String>,
    pub namespace: Option<String>,
    pub node: Option<String>,
    pub labels: Vec<LabelMatch>,
}

impl TextFilter {
    pub fn parse(input: &str) -> Self {
        let mut filter = Self::default();
        for token in input.split_whitespace() {
            if let Some(namespace) = namespace_token(token) {
                filter.namespace = Some(namespace);
            } else if let Some(node) = node_token(token) {
                filter.node = Some(node);
            } else if let Some(label) = label_token(token) {
                filter.labels.push(label);
            } else {
                filter.words.push(token.to_ascii_lowercase());
            }
        }
        filter
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
            && self.namespace.is_none()
            && self.node.is_none()
            && self.labels.is_empty()
    }

    pub fn matches_text(&self, fields: &[&str]) -> bool {
        self.words.iter().all(|word| {
            fields
                .iter()
                .any(|field| field.to_ascii_lowercase().contains(word.as_str()))
        })
    }

    pub fn matches_search_text(&self, search_text: &str) -> bool {
        self.words
            .iter()
            .all(|word| search_text.contains(word.as_str()))
    }

    pub fn narrows(&self, previous: &Self) -> bool {
        self.namespace == previous.namespace
            && self.node == previous.node
            && self.labels == previous.labels
            && previous
                .words
                .iter()
                .all(|old| self.words.iter().any(|new| new.contains(old.as_str())))
    }

    pub fn matches_namespace(&self, namespace: Option<&str>) -> bool {
        self.namespace.as_ref().is_none_or(|wanted| {
            namespace.is_some_and(|ns| ns.to_ascii_lowercase().contains(wanted.as_str()))
        })
    }

    pub fn matches_labels(&self, labels: &BTreeMap<String, String>) -> bool {
        self.labels
            .iter()
            .all(|wanted| label_matches(wanted, labels))
    }

    pub fn matches(&self, namespace: Option<&str>, fields: &[&str]) -> bool {
        self.labels.is_empty()
            && self.node.is_none()
            && self.matches_namespace(namespace)
            && self.matches_text(fields)
    }

    pub fn matches_labeled(
        &self,
        namespace: Option<&str>,
        labels: &BTreeMap<String, String>,
        fields: &[&str],
    ) -> bool {
        self.node.is_none()
            && self.matches_namespace(namespace)
            && self.matches_labels(labels)
            && self.matches_text(fields)
    }

    pub fn highlights(&self, text: &str) -> Vec<Range<usize>> {
        highlights(&self.words, text)
    }
}

pub fn highlights(words: &[String], text: &str) -> Vec<Range<usize>> {
    let haystack = text.to_ascii_lowercase();
    let mut ranges: Vec<Range<usize>> = words
        .iter()
        .filter(|word| !word.is_empty())
        .flat_map(|word| {
            haystack
                .match_indices(word.as_str())
                .map(|(start, hit)| start..start + hit.len())
                .collect::<Vec<_>>()
        })
        .collect();
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
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

    #[test]
    fn extracting_complete_tokens_keeps_free_text_and_its_word_boundaries() {
        assert_eq!(
            extract_tokens("checkout ns:payments api "),
            (vec!["ns:payments".into()], "checkout api ".into())
        );
        assert_eq!(
            extract_tokens("ns:payments  "),
            (vec!["ns:payments".into()], String::new())
        );
        assert_eq!(
            extract_tokens("label:app=cart checkout"),
            (vec!["label:app=cart".into()], "checkout".into())
        );
        assert_eq!(
            extract_tokens("checkout  l:app="),
            (Vec::new(), "checkout  l:app=".into())
        );
        assert_eq!(
            extract_tokens("ns:payments"),
            (Vec::new(), "ns:payments".into())
        );
    }

    #[test]
    fn completion_suggests_prefixes_namespaces_and_only_current_label_values() {
        let sets = [
            labels(&[("app", "cart"), ("tier", "api")]),
            labels(&[("app", "web")]),
        ];
        let none: Vec<String> = Vec::new();
        let candidates = FilterCandidates {
            namespaces: BTreeSet::from(["payments".into(), "platform".into()]),
            labels: label_values(&sets),
        };
        assert_eq!(candidates.suggest("n"), ["ns:"]);
        assert_eq!(candidates.suggest("na"), ["ns:"]);
        assert_eq!(candidates.suggest("l"), ["l:", "label:"]);
        assert_eq!(candidates.suggest("nginx"), none);
        assert_eq!(candidates.suggest("ns:"), ["ns:payments", "ns:platform"]);
        assert_eq!(candidates.suggest("checkout ns:pay"), ["ns:payments"]);
        assert_eq!(candidates.suggest("l:"), ["l:app=", "l:tier="]);
        assert_eq!(candidates.suggest("label:a"), ["label:app="]);
        assert_eq!(
            candidates.suggest("label:app="),
            ["label:app=cart", "label:app=web"]
        );
        assert_eq!(candidates.suggest("l:app=c"), ["l:app=cart"]);
        assert_eq!(candidates.suggest("label:unknown="), none);
        assert_eq!(candidates.suggest("checkout"), none);
        assert_eq!(candidates.suggest("ns:payments "), none);
        assert_eq!(candidates.suggest(""), none);
    }

    #[test]
    fn chips_require_complete_tokens_and_preserve_label_case() {
        for token in ["ns:payments", "label:App=Cart", "l:tier"] {
            assert!(applied_token(token), "{token}");
        }
        assert!(applied_token("node:ip-10-4-12-87"));
        for token in [
            "ns:",
            "node:",
            "label:",
            "l:app=",
            "checkout",
            "label:=cart",
        ] {
            assert!(!applied_token(token), "{token}");
        }
        let filter = TextFilter::parse("checkout ns:payments label:App=Cart");
        assert!(filter.matches_labeled(
            Some("payments"),
            &labels(&[("App", "Cart")]),
            &["checkout"]
        ));
        assert!(!filter.matches_labeled(
            Some("payments"),
            &labels(&[("App", "cart")]),
            &["checkout"]
        ));
    }

    #[test]
    fn parses_namespace_and_label_tokens_like_the_palette() {
        let filter = TextFilter::parse("Checkout ns:Pay label:app=cart l:tier api");
        assert_eq!(filter.words, ["checkout", "api"]);
        assert_eq!(filter.namespace.as_deref(), Some("pay"));
        assert_eq!(
            filter.labels,
            [
                ("app".to_owned(), Some("cart".to_owned())),
                ("tier".to_owned(), None)
            ]
        );
        assert!(TextFilter::parse("   ").is_empty());
    }

    #[test]
    fn every_word_must_hit_some_field() {
        let filter = TextFilter::parse("check API");
        assert!(filter.matches(None, &["checkout-api", "payments"]));
        assert!(filter.matches(None, &["checkout", "api-gw"]));
        assert!(!filter.matches(None, &["checkout", "payments"]));
        assert!(TextFilter::default().matches(None, &[]));
    }

    #[test]
    fn namespace_tokens_match_by_substring_and_need_a_namespace() {
        let filter = TextFilter::parse("ns:pay");
        assert!(filter.matches(Some("payments"), &[]));
        assert!(!filter.matches(Some("orders"), &[]));
        assert!(!filter.matches(None, &[]));
    }

    #[test]
    fn label_tokens_need_labels() {
        let filter = TextFilter::parse("l:app=cart");
        assert!(filter.matches_labeled(None, &labels(&[("app", "cart")]), &[]));
        assert!(!filter.matches_labeled(None, &labels(&[("app", "web")]), &[]));
        assert!(!filter.matches(None, &["app=cart"]));
        let key_only = TextFilter::parse("label:app");
        assert!(key_only.matches_labeled(None, &labels(&[("app", "web")]), &[]));
    }

    #[test]
    fn status_chips_count_everything_and_what_needs_attention() {
        let statuses = [Status::Ok, Status::Err, Status::Warn, Status::Ok];
        assert_eq!(
            StatusFilter::counts(statuses.iter().copied()),
            [(StatusFilter::All, 4), (StatusFilter::Attention, 2)]
        );
        assert_eq!(StatusFilter::at(9), StatusFilter::All);
    }

    #[test]
    fn highlights_merge_overlapping_hits() {
        let filter = TextFilter::parse("api pi CHECK");
        assert_eq!(
            filter.highlights("checkout-api-api"),
            vec![0..5, 9..12, 13..16]
        );
        assert_eq!(
            TextFilter::default().highlights("checkout"),
            Vec::<Range<usize>>::new()
        );
    }

    struct Toy {
        title: String,
        owner: String,
        hidden: String,
        namespace: String,
        labels: BTreeMap<String, String>,
    }

    const TOY: FilterSpec<Toy> = FilterSpec {
        fields: &[
            ("title", |t, (), out| out.push(t.title.as_str().into())),
            ("owner", |t, (), out| out.push(t.owner.as_str().into())),
        ],
        namespace: false,
        labels: true,
    };

    fn toy() -> Toy {
        Toy {
            title: "alpha".into(),
            owner: "beta".into(),
            hidden: "gamma".into(),
            namespace: "delta".into(),
            labels: labels(&[("team", "core")]),
        }
    }

    fn toy_matches(query: &str) -> bool {
        let row = toy();
        let query = TextFilter::parse(query);
        TOY.matches(&row, &query, Some(&row.namespace), &[&row.labels])
    }

    #[test]
    fn spec_matches_only_the_fields_it_names() {
        assert!(toy_matches("alpha"));
        assert!(toy_matches("beta"));
        assert!(!toy_matches("gamma"));
        assert!(!toy_matches("delta"));
        assert_eq!(toy().hidden, "gamma");
    }

    #[test]
    fn node_tokens_parse_lowercased_and_need_a_node_field() {
        let filter = TextFilter::parse("api node:Worker-2");
        assert_eq!(filter.words, ["api"]);
        assert_eq!(filter.node.as_deref(), Some("worker-2"));
        assert!(!filter.is_empty());
        assert!(TextFilter::parse(&node_query("worker")).node.is_some());
        assert!(!TextFilter::parse("node:worker").matches(None, &["worker"]));
        assert!(!toy_matches("node:alpha"));
    }

    #[test]
    fn spec_honours_its_namespace_and_label_flags() {
        assert!(!toy_matches("ns:delta"));
        assert!(toy_matches("label:team=core"));
        assert!(!toy_matches("label:team=other"));
    }

    #[test]
    fn search_text_matches_like_the_fields_it_joins() {
        let row = Toy {
            title: "Checkout-API".into(),
            ..toy()
        };
        let text = TOY.search_text(&row);
        let queries = [
            "",
            "check",
            "API",
            "checkout-api",
            "beta",
            "api beta",
            "tbeta",
            "api\nbeta",
            "gamma",
            "label:team=core",
            "label:team=web",
            "ns:delta",
            "node:beta",
        ];
        for query in queries {
            let query = TextFilter::parse(query);
            let namespace = Some(row.namespace.as_str());
            let labels = [&row.labels];
            assert_eq!(
                TOY.matches_search(&row, &query, namespace, &labels, &text),
                TOY.matches(&row, &query, namespace, &labels),
                "{query:?}"
            );
        }
    }

    #[test]
    fn a_query_narrows_one_whose_words_it_extends() {
        let narrows =
            |new: &str, old: &str| TextFilter::parse(new).narrows(&TextFilter::parse(old));
        assert!(narrows("checkout", "check"));
        assert!(narrows("check", ""));
        assert!(narrows("check out", "check"));
        assert!(narrows("xcheckx", "check"));
        assert!(narrows("check ns:pay", "check ns:pay"));
        assert!(!narrows("chec", "check"));
        assert!(!narrows("", "check"));
        assert!(!narrows("check ns:pay", "check"));
        assert!(!narrows("check ns:payments", "check ns:pay"));
        assert!(!narrows("check node:a", "check"));
        assert!(!narrows("check l:app=web", "check l:app"));
    }
}
