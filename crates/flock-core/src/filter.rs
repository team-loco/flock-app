use std::borrow::Cow;
use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;
use std::ops::Range;
use std::rc::Rc;

use crate::Status;
use crate::query::{Chip, Facets, LABEL_FACET, NAMESPACE_FACET, NODE_FACET, Term, is_facet_token};

pub const NAMESPACE_PREFIX: &str = "ns:";
pub const LABEL_PREFIXES: [&str; 2] = ["label:", "l:"];
const LABEL_SEPARATOR: char = '=';

pub type LabelMatch = (String, Option<String>);
pub type Labels = BTreeMap<String, String>;

pub fn namespace_token(token: &str) -> Option<String> {
    token
        .strip_prefix(NAMESPACE_PREFIX)
        .map(str::to_ascii_lowercase)
}

pub fn node_query(node: &str) -> String {
    Term::include(NODE_FACET, node).to_string()
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

const SEARCH_SEPARATOR: &str = "\n";

pub type FieldValues<'a> = Vec<Cow<'a, str>>;
pub type FieldFn<T, C = ()> = for<'a> fn(&'a T, &'a C, &mut FieldValues<'a>);

pub struct FilterSpec<T: 'static, C: 'static = ()> {
    pub fields: &'static [(&'static str, FieldFn<T, C>)],
    pub facets: &'static [(&'static str, FieldFn<T, C>)],
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

    pub fn unlabeled_facets<'a>(
        &self,
        rows: impl IntoIterator<Item = (&'a T, Option<&'a str>)>,
    ) -> Facets {
        let mut facets = self.facets();
        for (row, namespace) in rows {
            self.count_facets(row, &(), namespace, &[], &mut facets);
        }
        facets
    }

    fn search_text(&self, row: &T) -> String {
        let mut values = FieldValues::new();
        for (_, field) in self.fields {
            field(row, &(), &mut values);
        }
        let mut text = values.join(SEARCH_SEPARATOR);
        text.make_ascii_lowercase();
        text
    }
}

pub trait Searchable: Sized + 'static {
    const FILTER: FilterSpec<Self>;

    fn namespace(&self) -> Option<&str>;

    fn labels(&self) -> Option<Cow<'_, Labels>>;

    fn matches(&self, query: &TextFilter) -> bool {
        let labels = self.labels();
        let label_sets: Vec<&Labels> = labels.iter().map(AsRef::as_ref).collect();
        Self::FILTER.matches(self, query, self.namespace(), &label_sets)
    }
}

pub fn facets<'a, T: Searchable>(rows: impl IntoIterator<Item = &'a T>) -> Facets {
    let mut facets = T::FILTER.facets();
    for row in rows {
        let labels = row.labels();
        let label_sets: Vec<&Labels> = labels.iter().map(AsRef::as_ref).collect();
        T::FILTER.count_facets(row, &(), row.namespace(), &label_sets, &mut facets);
    }
    facets
}

fn matches_search<T: Searchable>(row: &T, query: &TextFilter, search_text: &str) -> bool {
    let labels = if query.has_facet(LABEL_FACET) {
        row.labels()
    } else {
        None
    };
    let label_sets: Vec<&Labels> = labels.iter().map(AsRef::as_ref).collect();
    T::FILTER.scope_matches(row, &(), query, row.namespace(), &label_sets)
        && query.matches_search_text(search_text)
}

struct Matched {
    query: TextFilter,
    rows: Rc<[usize]>,
}

pub struct Search<T> {
    texts: OnceCell<Vec<String>>,
    matched: RefCell<Option<Matched>>,
    rows: PhantomData<fn(&T)>,
}

impl<T> Default for Search<T> {
    fn default() -> Self {
        Self {
            texts: OnceCell::new(),
            matched: RefCell::new(None),
            rows: PhantomData,
        }
    }
}

impl<T: Searchable> Search<T> {
    pub fn matching(&self, rows: &[T], query: &TextFilter) -> Rc<[usize]> {
        let previous = self.matched.borrow_mut().take();
        let candidates: Box<dyn Iterator<Item = usize>> = match &previous {
            Some(previous) if query.narrows(&previous.query) => {
                Box::new(previous.rows.iter().copied())
            }
            _ => Box::new(0..rows.len()),
        };
        let texts = (!query.words.is_empty()).then(|| self.texts(rows));
        let kept: Rc<[usize]> = candidates
            .filter(|&ix| {
                let text = texts
                    .and_then(|texts| texts.get(ix))
                    .map_or("", String::as_str);
                rows.get(ix)
                    .is_some_and(|row| matches_search(row, query, text))
            })
            .collect();
        let matched = Matched {
            query: query.clone(),
            rows: Rc::clone(&kept),
        };
        self.matched.replace(Some(matched));
        kept
    }

    fn texts(&self, rows: &[T]) -> &[String] {
        self.texts
            .get_or_init(|| rows.iter().map(|row| T::FILTER.search_text(row)).collect())
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

    pub fn facets(&self) -> Facets {
        let mut facets = Facets::default();
        if self.namespace {
            facets.declare(NAMESPACE_FACET);
        }
        for (name, _) in self.facets {
            facets.declare(name);
        }
        if self.labels {
            facets.declare(LABEL_FACET);
        }
        facets
    }

    pub fn group_facets(&self) -> Vec<&'static str> {
        let declared = self.facets.iter().map(|(name, _)| *name);
        let namespace = self.namespace.then_some(NAMESPACE_FACET);
        let mut names: Vec<&'static str> = Vec::new();
        for name in namespace.into_iter().chain(declared) {
            if name != LABEL_FACET && !names.contains(&name) {
                names.push(name);
            }
        }
        names
    }

    pub fn group_value<'a>(
        &self,
        row: &'a T,
        context: &'a C,
        namespace: Option<&'a str>,
        facet: &str,
    ) -> Option<Cow<'a, str>> {
        if facet == NAMESPACE_FACET {
            return namespace.filter(|_| self.namespace).map(Cow::Borrowed);
        }
        let mut values = FieldValues::new();
        for (_, field) in self.facets.iter().filter(|(name, _)| *name == facet) {
            field(row, context, &mut values);
        }
        values.into_iter().find(|value| !value.is_empty())
    }

    pub fn count_facets(
        &self,
        row: &T,
        context: &C,
        namespace: Option<&str>,
        label_sets: &[&BTreeMap<String, String>],
        facets: &mut Facets,
    ) {
        if let Some(namespace) = namespace.filter(|_| self.namespace) {
            facets.count(NAMESPACE_FACET, namespace);
        }
        for (name, field) in self.facets {
            let mut values = FieldValues::new();
            field(row, context, &mut values);
            values.sort();
            values.dedup();
            for value in &values {
                facets.count(name, value);
            }
        }
        if self.labels {
            let labels: BTreeSet<String> = label_sets
                .iter()
                .flat_map(|labels| labels.iter())
                .map(|(key, value)| format!("{key}{LABEL_SEPARATOR}{value}"))
                .collect();
            for label in &labels {
                facets.count(LABEL_FACET, label);
            }
        }
    }

    fn scope_matches(
        &self,
        row: &T,
        context: &C,
        query: &TextFilter,
        namespace: Option<&str>,
        label_sets: &[&BTreeMap<String, String>],
    ) -> bool {
        let matches_with = |labels: &[&BTreeMap<String, String>]| {
            query.terms_match(|term| self.has_value(row, context, namespace, labels, term))
        };
        if label_sets.len() > 1 && query.has_facet(LABEL_FACET) {
            return label_sets
                .iter()
                .any(|labels| matches_with(std::slice::from_ref(labels)));
        }
        matches_with(label_sets)
    }

    fn has_value(
        &self,
        row: &T,
        context: &C,
        namespace: Option<&str>,
        label_sets: &[&BTreeMap<String, String>],
        term: &Term,
    ) -> bool {
        if term.facet == NAMESPACE_FACET {
            return self.namespace
                && namespace.is_some_and(|namespace| term.matches_value(namespace));
        }
        if let Some(label) = term.label() {
            return self.labels
                && label_sets
                    .iter()
                    .any(|labels| label_matches(&label, labels));
        }
        let mut values = FieldValues::new();
        for (_, field) in self.facets.iter().filter(|(name, _)| *name == term.facet) {
            field(row, context, &mut values);
        }
        values.iter().any(|value| term.matches_value(value))
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
    pub terms: Vec<Term>,
}

impl TextFilter {
    pub fn parse(input: &str) -> Self {
        let mut filter = Self::default();
        for token in input.split_whitespace() {
            if let Some(term) = Term::parse(token) {
                filter.terms.push(term);
            } else if !is_facet_token(token) {
                filter.words.push(token.to_ascii_lowercase());
            }
        }
        filter
    }

    pub fn from_chips(chips: &[Chip], typed: &str) -> Self {
        let mut filter = Self::default();
        for chip in chips {
            match chip {
                Chip::Text(text) => filter
                    .words
                    .extend(text.split_whitespace().map(str::to_ascii_lowercase)),
                Chip::Term(term) => filter.terms.push(term.clone()),
            }
        }
        filter.words.extend(Self::parse(typed).words);
        filter
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty() && self.terms.is_empty()
    }

    pub fn has_facet(&self, facet: &str) -> bool {
        self.terms.iter().any(|term| term.facet == facet)
    }

    pub fn terms_match(&self, has: impl Fn(&Term) -> bool) -> bool {
        let (negative, positive): (Vec<&Term>, Vec<&Term>) =
            self.terms.iter().partition(|term| term.exclude);
        let removed = negative.iter().any(|term| has(term));
        let kept = positive.iter().all(|wanted| {
            positive
                .iter()
                .filter(|term| term.group() == wanted.group())
                .any(|term| has(term))
        });
        !removed && kept
    }

    pub fn selector_labels(&self) -> Option<Vec<LabelMatch>> {
        let labels: Vec<LabelMatch> = self
            .terms
            .iter()
            .filter(|term| !term.exclude)
            .filter_map(Term::label)
            .collect();
        let keys: BTreeSet<&str> = labels.iter().map(|(key, _)| key.as_str()).collect();
        let excluded = self
            .terms
            .iter()
            .any(|term| term.exclude && term.facet == LABEL_FACET);
        (!labels.is_empty() && !excluded && keys.len() == labels.len()).then_some(labels)
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
        self.terms == previous.terms
            && previous
                .words
                .iter()
                .all(|old| self.words.iter().any(|new| new.contains(old.as_str())))
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
    fn parses_facet_chips_legacy_tokens_and_words() {
        let filter = TextFilter::parse("Checkout ns:Pay label:app=cart -@status:Failing @ns: api");
        assert_eq!(filter.words, ["checkout", "api"]);
        assert_eq!(
            filter.terms,
            [
                Term::include(NAMESPACE_FACET, "Pay"),
                Term::include(LABEL_FACET, "app=cart"),
                Term::exclude("status", "Failing"),
            ]
        );
        assert!(TextFilter::parse("   ").is_empty());
        assert!(TextFilter::parse("@namespace").is_empty());
    }

    #[test]
    fn chips_and_typed_words_make_one_query_without_half_typed_terms() {
        let chips = [
            Chip::Text("Check Out".into()),
            Chip::parse("@namespace:web"),
        ];
        let filter = TextFilter::from_chips(&chips, "api @namespace:pay");
        assert_eq!(filter.words, ["check", "out", "api"]);
        assert_eq!(filter.terms, [Term::include(NAMESPACE_FACET, "web")]);
    }

    #[test]
    fn every_word_must_hit_some_field() {
        let filter = TextFilter::parse("check API");
        assert!(filter.matches_text(&["checkout-api", "payments"]));
        assert!(filter.matches_text(&["checkout", "api-gw"]));
        assert!(!filter.matches_text(&["checkout", "payments"]));
        assert!(TextFilter::default().matches_text(&[]));
    }

    #[test]
    fn includes_of_one_facet_or_different_facets_and_excludes_remove() {
        let row = [("namespace", "web"), ("status", "Failing")];
        let matches = |query: &str| {
            TextFilter::parse(query).terms_match(|term| {
                row.iter()
                    .any(|(facet, value)| term.facet == *facet && term.matches_value(value))
            })
        };
        assert!(matches(""));
        assert!(matches("@namespace:payments @namespace:WEB"));
        assert!(!matches("@namespace:payments"));
        assert!(matches("@namespace:web @status:failing"));
        assert!(!matches("@namespace:web @status:Healthy"));
        assert!(!matches("@namespace:web -@status:Failing"));
        assert!(matches("-@status:Healthy -@node:a"));
        assert!(!matches("@node:a"));
    }

    #[test]
    fn label_terms_need_labels_and_keep_their_case() {
        let filter = TextFilter::parse("l:App=Cart");
        let matches = |labels: &BTreeMap<String, String>| {
            filter.terms_match(|term| term.label().is_some_and(|l| label_matches(&l, labels)))
        };
        assert!(matches(&labels(&[("App", "Cart")])));
        assert!(!matches(&labels(&[("App", "cart")])));
        let pod = labels(&[("app", "web"), ("tier", "db")]);
        let on_pod = |query: &str| {
            TextFilter::parse(query)
                .terms_match(|term| term.label().is_some_and(|l| label_matches(&l, &pod)))
        };
        assert!(on_pod("@label:app=api @label:app=web"));
        assert!(!on_pod("@label:app=web @label:tier=cache"));
        assert!(on_pod("@label:app=web @label:tier=db"));
        let key_only = TextFilter::parse("label:app");
        assert!(key_only.terms_match(|term| {
            term.label()
                .is_some_and(|l| label_matches(&l, &labels(&[("app", "web")])))
        }));
    }

    #[test]
    fn only_unambiguous_label_includes_become_a_selector() {
        let selector = |query: &str| TextFilter::parse(query).selector_labels();
        assert_eq!(
            selector("@label:app=web l:canary @namespace:x"),
            Some(vec![
                ("app".to_owned(), Some("web".to_owned())),
                ("canary".to_owned(), None)
            ])
        );
        assert_eq!(selector("@namespace:x"), None);
        assert_eq!(selector("@label:app=web @label:app=api"), None);
        assert_eq!(selector("@label:app=web -@label:tier=db"), None);
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

    impl Searchable for Toy {
        const FILTER: FilterSpec<Self> = FilterSpec {
            fields: &[
                ("title", |t, (), out| out.push(t.title.as_str().into())),
                ("owner", |t, (), out| out.push(t.owner.as_str().into())),
            ],
            facets: &[("owner", |t, (), out| out.push(t.owner.as_str().into()))],
            namespace: false,
            labels: true,
        };

        fn namespace(&self) -> Option<&str> {
            Some(&self.namespace)
        }

        fn labels(&self) -> Option<Cow<'_, Labels>> {
            Some(Cow::Borrowed(&self.labels))
        }
    }

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
        toy().matches(&TextFilter::parse(query))
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
    fn node_presets_are_node_facet_chips_and_need_a_node_facet() {
        assert_eq!(node_query("worker-2"), "@node:worker-2");
        assert_eq!(
            TextFilter::parse(&node_query("worker-2")).terms,
            [Term::include(NODE_FACET, "worker-2")]
        );
        assert!(!toy_matches("node:alpha"));
        assert!(toy_matches("-@node:alpha"));
    }

    #[test]
    fn spec_honours_its_namespace_label_and_declared_facets() {
        assert!(!toy_matches("ns:delta"));
        assert!(toy_matches("label:team=core"));
        assert!(!toy_matches("label:team=other"));
        assert!(toy_matches("@owner:BETA"));
        assert!(!toy_matches("@owner:bet"));
        assert!(!toy_matches("-@owner:beta"));
        assert!(!toy_matches("@title:alpha"));
    }

    #[test]
    fn facets_count_declared_values_and_labels_in_spec_order() {
        let rows = [
            toy(),
            Toy {
                owner: "gamma".into(),
                labels: labels(&[("team", "core"), ("tier", "db")]),
                ..toy()
            },
        ];
        let facets = facets(&rows);
        assert_eq!(
            facets.names().collect::<Vec<_>>(),
            [("owner", 2), (LABEL_FACET, 2)]
        );
        assert_eq!(
            facets.values(LABEL_FACET),
            [("team=core", 2), ("tier=db", 1)]
        );
    }

    #[test]
    fn groups_offer_namespace_and_declared_facets_but_not_labels() {
        let spec = FilterSpec::<Toy> {
            namespace: true,
            ..Toy::FILTER
        };
        assert_eq!(spec.group_facets(), [NAMESPACE_FACET, "owner"]);
        assert_eq!(Toy::FILTER.group_facets(), ["owner"]);
        let row = toy();
        let value = |spec: &FilterSpec<Toy>, facet| {
            spec.group_value(&row, &(), row.namespace(), facet)
                .map(Cow::into_owned)
        };
        assert_eq!(value(&spec, NAMESPACE_FACET).as_deref(), Some("delta"));
        assert_eq!(value(&Toy::FILTER, NAMESPACE_FACET), None);
        assert_eq!(value(&spec, "owner").as_deref(), Some("beta"));
        assert_eq!(value(&spec, LABEL_FACET), None);
    }

    #[test]
    fn search_matches_cached_text_and_narrows_typed_queries() {
        let rows = [
            Toy {
                title: "Checkout-API".into(),
                ..toy()
            },
            Toy {
                title: "cart".into(),
                labels: labels(&[("team", "web")]),
                ..toy()
            },
            toy(),
        ];
        let search = Search::default();
        let matching = |query: &str| search.matching(&rows, &TextFilter::parse(query)).to_vec();
        assert_eq!(matching(""), [0, 1, 2]);
        assert_eq!(matching("c"), [0, 1]);
        assert_eq!(matching("ch"), [0]);
        assert_eq!(matching("CHECKOUT-api"), [0]);
        assert_eq!(matching("c"), [0, 1]);
        assert_eq!(matching("api\nbeta"), [0]);
        assert_eq!(matching("beta"), [0, 1, 2]);
        assert_eq!(matching("tbeta"), Vec::<usize>::new());
        assert_eq!(matching("label:team=web"), [1]);
        assert_eq!(matching("gamma"), Vec::<usize>::new());
        assert_eq!(matching("ns:delta"), Vec::<usize>::new());
        assert_eq!(matching("a"), [0, 1, 2]);
    }

    #[test]
    fn a_query_narrows_one_whose_words_it_extends() {
        let narrows =
            |new: &str, old: &str| TextFilter::parse(new).narrows(&TextFilter::parse(old));
        assert!(narrows("checkout", "check"));
        assert!(narrows("check", ""));
        assert!(narrows("check out", "check"));
        assert!(narrows("xcheckx", "check"));
        assert!(narrows("check @namespace:pay", "check ns:pay"));
        assert!(!narrows("che", "check"));
        assert!(!narrows("", "check"));
        assert!(!narrows("check ns:pay", "check"));
        assert!(!narrows("check ns:payments", "check ns:pay"));
        assert!(!narrows("check -@node:a", "check"));
        assert!(!narrows("check l:app=web", "check l:app"));
    }
}
