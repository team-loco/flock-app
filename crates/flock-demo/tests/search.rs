use flock_core::Snapshot;
use flock_core::filter::{
    FieldValues, LABEL_PREFIXES, NAMESPACE_PREFIX, Search, Searchable, TextFilter,
};
use flock_core::query::Term;
use flock_core::resources::ResourceKind;

const SAMPLED_ROWS: usize = 4;
const PREFIX_CHARS: usize = 3;
const MISSING: &str = "zz-no-such-row";

fn queries<T: Searchable>(rows: &[T]) -> Vec<String> {
    let mut queries = vec![String::new(), MISSING.to_owned()];
    for row in rows.iter().take(SAMPLED_ROWS) {
        for (name, field) in T::FILTER.facets {
            let mut facet = FieldValues::new();
            field(row, &(), &mut facet);
            for value in &facet {
                queries.push(Term::include(name, value).to_string());
                queries.push(Term::exclude(name, value).to_string());
            }
        }
        let mut values = FieldValues::new();
        for (_, field) in T::FILTER.fields {
            field(row, &(), &mut values);
        }
        for value in &values {
            let short: String = value.chars().take(PREFIX_CHARS).collect();
            queries.push(value.to_string());
            queries.push(value.to_uppercase());
            queries.push(format!("{short} {MISSING}"));
            queries.push(short);
        }
        if let [first, second, ..] = values.as_slice() {
            queries.push(format!("{first} {second}"));
        }
        if let Some(namespace) = row.namespace() {
            let short: String = namespace.chars().take(PREFIX_CHARS).collect();
            queries.push(format!("{NAMESPACE_PREFIX}{short}"));
            queries.push(format!("{NAMESPACE_PREFIX}{namespace} {short}"));
        }
        for (key, value) in row.labels().iter().flat_map(|labels| labels.iter()) {
            let [label, short] = LABEL_PREFIXES;
            queries.push(format!("{label}{key}={value}"));
            queries.push(format!("{short}{key} {value}"));
        }
    }
    queries
}

fn assert_search_matches_the_spec<T: Searchable>(name: &str, rows: &[T]) {
    assert!(!rows.is_empty(), "{name} has no demo rows");
    let search = Search::default();
    for query in queries(rows) {
        let typed = query.char_indices().map(|(ix, _)| ix).chain([query.len()]);
        for end in typed {
            let prefix = query.get(..end).unwrap_or_default();
            let filter = TextFilter::parse(prefix);
            let expected: Vec<usize> = rows
                .iter()
                .enumerate()
                .filter(|(_, row)| row.matches(&filter))
                .map(|(ix, _)| ix)
                .collect();
            let found = search.matching(rows, &filter);
            assert_eq!(*found, *expected, "{name}: {prefix:?}");
        }
    }
}

#[test]
fn cached_search_agrees_with_the_filter_spec_on_every_row_type() {
    let snapshot: Snapshot = flock_demo::snapshot();
    assert_search_matches_the_spec("pods", &snapshot.pods);
    assert_search_matches_the_spec("flux", &snapshot.flux);
    assert_search_matches_the_spec("helm", &snapshot.helm);
    assert_search_matches_the_spec("flux events", &snapshot.flux_events);
    for detail in snapshot.details.values() {
        assert_search_matches_the_spec("nodes", &detail.nodes);
        assert_search_matches_the_spec("namespaces", &detail.namespaces);
        assert_search_matches_the_spec("events", &detail.events);
        for kind in ResourceKind::ALL {
            assert_search_matches_the_spec(kind.name(), detail.resources(kind));
        }
    }
    let custom = snapshot
        .custom
        .iter()
        .find(|objects| !objects.rows.is_empty())
        .map_or(&[][..], |objects| objects.rows.as_slice());
    assert_search_matches_the_spec("custom", custom);
}
