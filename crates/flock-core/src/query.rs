use std::collections::BTreeMap;
use std::fmt;

pub const NAMESPACE_FACET: &str = "namespace";
pub const LABEL_FACET: &str = "label";
pub const NODE_FACET: &str = "node";
pub const TEXT_KEY: &str = "name:";
pub const FACET_LIST: &str = "facets";

const FACET_MARK: char = '@';
const EXCLUDE_MARK: char = '-';
const FACET_SEPARATOR: char = ':';
const LABEL_SEPARATOR: char = '=';
const TEXT_WILDCARD: char = '*';
const SUGGESTED_VALUES: usize = 4;
const MATCHED_VALUES: usize = 6;
const LEGACY_FACETS: [(&str, &str); 5] = [
    ("ns", NAMESPACE_FACET),
    (NAMESPACE_FACET, NAMESPACE_FACET),
    ("l", LABEL_FACET),
    (LABEL_FACET, LABEL_FACET),
    (NODE_FACET, NODE_FACET),
];

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Term {
    pub facet: String,
    pub value: String,
    pub exclude: bool,
}

impl Term {
    pub fn include(facet: &str, value: &str) -> Self {
        Self {
            facet: facet.to_owned(),
            value: value.to_owned(),
            exclude: false,
        }
    }

    pub fn exclude(facet: &str, value: &str) -> Self {
        Self {
            exclude: true,
            ..Self::include(facet, value)
        }
    }

    pub fn parse(token: &str) -> Option<Self> {
        let (exclude, marked, rest) = split_marks(token);
        let (key, value) = rest.split_once(FACET_SEPARATOR)?;
        let facet = facet_name(key, marked)?;
        let label_ok = facet != LABEL_FACET || valid_label(value);
        (!value.is_empty() && label_ok).then(|| Self {
            facet,
            value: value.to_owned(),
            exclude,
        })
    }

    pub fn key(&self) -> String {
        let exclude = if self.exclude { "-" } else { "" };
        format!("{exclude}{FACET_MARK}{}{FACET_SEPARATOR}", self.facet)
    }

    pub fn same_value(&self, other: &Self) -> bool {
        self.facet == other.facet && self.value == other.value
    }

    pub fn matches_value(&self, actual: &str) -> bool {
        self.value.eq_ignore_ascii_case(actual)
    }

    pub fn group(&self) -> (&str, Option<&str>) {
        let label_key = (self.facet == LABEL_FACET).then(|| {
            self.value
                .split_once(LABEL_SEPARATOR)
                .map_or(self.value.as_str(), |(key, _)| key)
        });
        (&self.facet, label_key)
    }

    pub fn label(&self) -> Option<(String, Option<String>)> {
        if self.facet != LABEL_FACET {
            return None;
        }
        Some(match self.value.split_once(LABEL_SEPARATOR) {
            Some((key, value)) => (key.to_owned(), Some(value.to_owned())),
            None => (self.value.clone(), None),
        })
    }
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.key(), self.value)
    }
}

fn split_marks(token: &str) -> (bool, bool, &str) {
    let excluded = token
        .strip_prefix(EXCLUDE_MARK)
        .filter(|rest| rest.starts_with(FACET_MARK));
    let (exclude, rest) = excluded.map_or((false, token), |rest| (true, rest));
    let (marked, rest) = rest
        .strip_prefix(FACET_MARK)
        .map_or((false, rest), |rest| (true, rest));
    (exclude, marked, rest)
}

fn facet_name(key: &str, marked: bool) -> Option<String> {
    let key = key.to_ascii_lowercase();
    let legacy = LEGACY_FACETS
        .iter()
        .find(|(alias, _)| *alias == key)
        .map(|(_, facet)| (*facet).to_owned());
    match legacy {
        Some(facet) => Some(facet),
        None if marked && valid_facet(&key) => Some(key),
        None => None,
    }
}

fn valid_facet(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn valid_label(value: &str) -> bool {
    match value.split_once(LABEL_SEPARATOR) {
        Some((key, value)) => !key.is_empty() && !value.is_empty(),
        None => !value.is_empty(),
    }
}

pub fn is_facet_token(token: &str) -> bool {
    let (exclude, marked, rest) = split_marks(token);
    if exclude || marked {
        return true;
    }
    rest.split_once(FACET_SEPARATOR)
        .is_some_and(|(key, _)| facet_name(key, false).is_some())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chip {
    Text(String),
    Term(Term),
}

impl Chip {
    pub fn parse(token: &str) -> Self {
        Term::parse(token).map_or_else(|| Self::Text(token.to_owned()), Self::Term)
    }
}

impl fmt::Display for Chip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => f.write_str(text),
            Self::Term(term) => term.fmt(f),
        }
    }
}

pub fn extract_terms(input: &str) -> (Vec<Term>, String) {
    let mut terms = Vec::new();
    let mut text = String::new();
    for part in input.split_inclusive(char::is_whitespace) {
        let complete = part.ends_with(char::is_whitespace);
        match Term::parse(part.trim_end()).filter(|_| complete) {
            Some(term) => terms.push(term),
            None => text.push_str(part),
        }
    }
    if text.trim().is_empty() {
        text.clear();
    }
    (terms, text)
}

pub fn add_term(chips: &mut Vec<Chip>, term: Term) {
    chips.retain(|chip| !matches!(chip, Chip::Term(old) if old.same_value(&term)));
    chips.push(Chip::Term(term));
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facets(Vec<(String, BTreeMap<String, usize>)>);

impl Facets {
    pub fn declare(&mut self, facet: &str) {
        if !self.0.iter().any(|(name, _)| name == facet) {
            self.0.push((facet.to_owned(), BTreeMap::new()));
        }
    }

    pub fn count(&mut self, facet: &str, value: &str) {
        if value.is_empty() {
            return;
        }
        self.declare(facet);
        let Some((_, values)) = self.0.iter_mut().find(|(name, _)| name == facet) else {
            return;
        };
        match values.get_mut(value) {
            Some(count) => *count += 1,
            None => {
                values.insert(value.to_owned(), 1);
            }
        }
    }

    pub fn add(&mut self, facet: &str, value: &str, count: usize) {
        if value.is_empty() || count == 0 {
            return;
        }
        self.declare(facet);
        let Some((_, values)) = self.0.iter_mut().find(|(name, _)| name == facet) else {
            return;
        };
        *values.entry(value.to_owned()).or_default() += count;
    }

    pub fn names(&self) -> impl Iterator<Item = (&str, usize)> {
        self.0
            .iter()
            .filter(|(_, values)| !values.is_empty())
            .map(|(name, values)| (name.as_str(), values.len()))
    }

    pub fn values(&self, facet: &str) -> Vec<(&str, usize)> {
        let mut values: Vec<(&str, usize)> = self
            .0
            .iter()
            .filter(|(name, _)| name == facet)
            .flat_map(|(_, values)| values.iter().map(|(value, n)| (value.as_str(), *n)))
            .collect();
        values.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        values
    }

    fn has(&self, facet: &str) -> bool {
        self.names().any(|(name, _)| name == facet)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Suggestion {
    Text(String),
    Facet {
        facet: String,
        values: usize,
        exclude: bool,
    },
    Value {
        term: Term,
        count: usize,
    },
}

impl Suggestion {
    pub fn header(&self) -> Option<String> {
        match self {
            Self::Text(_) => None,
            Self::Facet { .. } => Some(FACET_LIST.to_owned()),
            Self::Value { term, .. } => Some(format!("{FACET_MARK}{}", term.facet)),
        }
    }

    pub fn key(&self) -> String {
        match self {
            Self::Text(_) => TEXT_KEY.to_owned(),
            Self::Facet { facet, exclude, .. } => Term {
                facet: facet.clone(),
                value: String::new(),
                exclude: *exclude,
            }
            .key(),
            Self::Value { term, .. } => term.key(),
        }
    }

    pub fn value(&self) -> String {
        match self {
            Self::Text(text) => format!("{TEXT_WILDCARD}{text}{TEXT_WILDCARD}"),
            Self::Facet { .. } => String::new(),
            Self::Value { term, .. } => term.value.clone(),
        }
    }

    pub fn count(&self) -> Option<usize> {
        match self {
            Self::Text(_) => None,
            Self::Facet { values, .. } => Some(*values),
            Self::Value { count, .. } => Some(*count),
        }
    }
}

pub fn last_token(input: &str) -> &str {
    input.rsplit(char::is_whitespace).next().unwrap_or(input)
}

pub fn suggest(facets: &Facets, chosen: &[Chip], input: &str) -> Vec<Suggestion> {
    let token = last_token(input);
    if is_facet_token(token) {
        return facet_suggestions(facets, token);
    }
    free_text(facets, chosen, input)
}

fn facet_suggestions(facets: &Facets, token: &str) -> Vec<Suggestion> {
    let (exclude, marked, rest) = split_marks(token);
    let typed_facet = rest
        .split_once(FACET_SEPARATOR)
        .and_then(|(key, part)| Some((facet_name(key, marked)?, part)))
        .filter(|(facet, _)| facets.has(facet));
    match typed_facet {
        Some((facet, part)) => facet_values(facets, &facet, part, exclude),
        None if marked => facet_list(facets, rest, exclude),
        None => Vec::new(),
    }
}

fn contains(value: &str, part: &str) -> bool {
    value.to_ascii_lowercase().contains(part)
}

fn facet_values(facets: &Facets, facet: &str, part: &str, exclude: bool) -> Vec<Suggestion> {
    let part = part.to_ascii_lowercase();
    facets
        .values(facet)
        .into_iter()
        .filter(|(value, _)| contains(value, &part))
        .map(|(value, count)| Suggestion::Value {
            term: Term {
                exclude,
                ..Term::include(facet, value)
            },
            count,
        })
        .collect()
}

fn facet_list(facets: &Facets, part: &str, exclude: bool) -> Vec<Suggestion> {
    if part.contains(FACET_SEPARATOR) {
        return Vec::new();
    }
    let part = part.to_ascii_lowercase();
    facets
        .names()
        .filter(|(name, _)| name.contains(&part))
        .map(|(name, values)| Suggestion::Facet {
            facet: name.to_owned(),
            values,
            exclude,
        })
        .collect()
}

fn free_text(facets: &Facets, chosen: &[Chip], input: &str) -> Vec<Suggestion> {
    let part = input.to_ascii_lowercase();
    let limit = if part.is_empty() {
        SUGGESTED_VALUES
    } else {
        MATCHED_VALUES
    };
    let picked = |facet: &str, value: &str| {
        chosen.iter().any(
            |chip| matches!(chip, Chip::Term(term) if term.facet == facet && term.value == value),
        )
    };
    let text = (!part.is_empty()).then(|| Suggestion::Text(input.to_owned()));
    let values = facets.names().flat_map(|(facet, _)| {
        facets
            .values(facet)
            .into_iter()
            .filter(|(value, _)| contains(value, &part) && !picked(facet, value))
            .take(limit)
            .map(|(value, count)| Suggestion::Value {
                term: Term::include(facet, value),
                count,
            })
    });
    text.into_iter().chain(values).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facets() -> Facets {
        let mut facets = Facets::default();
        facets.declare(NAMESPACE_FACET);
        for (namespace, n) in [
            ("payments", 4),
            ("web", 2),
            ("search", 2),
            ("monitoring", 2),
        ] {
            for _ in 0..n {
                facets.count(NAMESPACE_FACET, namespace);
            }
        }
        facets.count("status", "Healthy");
        facets.count("status", "Failing");
        facets.count("status", "Healthy");
        facets.count("status", "");
        facets.declare(LABEL_FACET);
        facets
    }

    #[test]
    fn terms_parse_marked_facets_exclusions_and_legacy_tokens() {
        assert_eq!(
            Term::parse("@namespace:payments"),
            Some(Term::include(NAMESPACE_FACET, "payments"))
        );
        assert_eq!(
            Term::parse("-@Status:Failing"),
            Some(Term::exclude("status", "Failing"))
        );
        assert_eq!(
            Term::parse("ns:web"),
            Some(Term::include(NAMESPACE_FACET, "web"))
        );
        assert_eq!(
            Term::parse("l:app=cart"),
            Some(Term::include(LABEL_FACET, "app=cart"))
        );
        assert_eq!(
            Term::parse("label:tier"),
            Some(Term::include(LABEL_FACET, "tier"))
        );
        assert_eq!(
            Term::parse("node:ip-10-4"),
            Some(Term::include(NODE_FACET, "ip-10-4"))
        );
        for incomplete in [
            "@namespace:",
            "@ns",
            "status:Failing",
            "-ns:web",
            "l:app=",
            "label:=cart",
            "@:x",
            "checkout",
        ] {
            assert_eq!(Term::parse(incomplete), None, "{incomplete}");
        }
        assert!(is_facet_token("@names"));
        assert!(is_facet_token("-@"));
        assert!(is_facet_token("ns:"));
        assert!(!is_facet_token("image:tag"));
        assert!(!is_facet_token("-api"));
    }

    #[test]
    fn chips_print_back_to_tokens_that_parse_to_the_same_chip() {
        for token in ["@namespace:payments", "-@status:Failing", "checkout"] {
            assert_eq!(Chip::parse(token).to_string(), token);
        }
        assert_eq!(Chip::parse("ns:web").to_string(), "@namespace:web");
        let chip = Chip::parse("-@label:app=web");
        assert_eq!(Chip::parse(&chip.to_string()), chip);
    }

    #[test]
    fn complete_terms_followed_by_space_leave_the_text() {
        assert_eq!(
            extract_terms("checkout @namespace:payments api "),
            (
                vec![Term::include(NAMESPACE_FACET, "payments")],
                "checkout api ".into()
            )
        );
        assert_eq!(
            extract_terms("ns:web  "),
            (vec![Term::include(NAMESPACE_FACET, "web")], String::new())
        );
        assert_eq!(
            extract_terms("@namespace:payments"),
            (Vec::new(), "@namespace:payments".into())
        );
    }

    #[test]
    fn adding_a_value_replaces_its_opposite() {
        let mut chips = vec![Chip::Text("api".into())];
        add_term(&mut chips, Term::include(NAMESPACE_FACET, "web"));
        add_term(&mut chips, Term::exclude(NAMESPACE_FACET, "web"));
        assert_eq!(
            chips,
            [
                Chip::Text("api".into()),
                Chip::Term(Term::exclude(NAMESPACE_FACET, "web"))
            ]
        );
    }

    #[test]
    fn facets_count_values_by_frequency_and_skip_empty_ones() {
        let facets = facets();
        assert_eq!(
            facets.names().collect::<Vec<_>>(),
            [(NAMESPACE_FACET, 4), ("status", 2)]
        );
        assert_eq!(
            facets.values(NAMESPACE_FACET),
            [
                ("payments", 4),
                ("monitoring", 2),
                ("search", 2),
                ("web", 2)
            ]
        );
        assert_eq!(facets.values("status"), [("Healthy", 2), ("Failing", 1)]);
        assert_eq!(facets.values("missing"), []);
    }

    fn rows(suggestions: &[Suggestion]) -> Vec<String> {
        suggestions
            .iter()
            .map(|s| format!("{}{}", s.key(), s.value()))
            .collect()
    }

    #[test]
    fn empty_input_suggests_four_unchosen_values_per_facet() {
        let chosen = [Chip::Term(Term::include(NAMESPACE_FACET, "web"))];
        let suggestions = suggest(&facets(), &chosen, "");
        assert_eq!(
            rows(&suggestions),
            [
                "@namespace:payments",
                "@namespace:monitoring",
                "@namespace:search",
                "@status:Healthy",
                "@status:Failing"
            ]
        );
        assert_eq!(suggestions[0].header().as_deref(), Some("@namespace"));
        assert_eq!(suggestions[0].count(), Some(4));
    }

    #[test]
    fn typed_text_leads_with_a_name_match_then_matching_values() {
        let suggestions = suggest(&facets(), &[], "IN");
        assert_eq!(
            rows(&suggestions),
            ["name:*IN*", "@namespace:monitoring", "@status:Failing"]
        );
        assert_eq!(suggestions[0].header(), None);
        assert_eq!(suggestions[0].count(), None);
    }

    #[test]
    fn a_mark_lists_facets_and_a_facet_lists_its_values() {
        let listed = suggest(&facets(), &[], "@");
        assert_eq!(rows(&listed), ["@namespace:", "@status:"]);
        assert_eq!(listed[0].count(), Some(4));
        assert_eq!(listed[0].header().as_deref(), Some(FACET_LIST));
        assert_eq!(rows(&suggest(&facets(), &[], "-@st")), ["-@status:"]);
        assert_eq!(
            rows(&suggest(&facets(), &[], "@namespace:")),
            [
                "@namespace:payments",
                "@namespace:monitoring",
                "@namespace:search",
                "@namespace:web"
            ]
        );
        assert_eq!(
            rows(&suggest(&facets(), &[], "-@namespace:EB")),
            ["-@namespace:web"]
        );
        assert_eq!(
            rows(&suggest(&facets(), &[], "ns:pay")),
            ["@namespace:payments"]
        );
        assert_eq!(suggest(&facets(), &[], "@missing:x"), []);
        assert_eq!(suggest(&facets(), &[], "@label:"), []);
        assert_eq!(
            rows(&suggest(&facets(), &[], "checkout @status:f")),
            ["@status:Failing"]
        );
        assert_eq!(last_token("checkout @status:f"), "@status:f");
        assert_eq!(last_token("checkout "), "");
    }
}
