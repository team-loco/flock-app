use std::borrow::Cow;
use std::collections::BTreeMap;

use crate::filter::{Searchable, TextFilter};
use crate::nav::FLOCK_NAV;
use crate::query::{Facets, NAMESPACE_FACET};
use crate::resources::{Resource, ResourceKind};
use crate::scope::Check;
use crate::{ClusterId, Counts, Page, Snapshot, Status};

pub const STATUS_FACET: &str = "status";
pub const CLUSTER_FACET: &str = "cluster";
pub const IMAGE_FACET: &str = "image";
pub const EXTRA_FACETS: [&str; 3] = [STATUS_FACET, CLUSTER_FACET, IMAGE_FACET];
const READY_COLUMN: &str = "Ready";
const IMAGE_COLUMN: &str = "Image";
const TAG_SEPARATOR: char = ':';
const PATH_SEPARATOR: char = '/';

pub fn badges(snapshot: &Snapshot) -> BTreeMap<Page, Counts> {
    let pages = FLOCK_NAV.iter().flat_map(|section| section.pages.iter());
    pages
        .filter_map(|&page| Some((page, page.across()?)))
        .map(|(page, listed)| (page, flagged(badge(snapshot, listed))))
        .collect()
}

fn badge(snapshot: &Snapshot, listed: Page) -> Counts {
    if listed == Page::Releases {
        return Counts::of(snapshot.helm.iter().map(|release| release.status));
    }
    snapshot
        .clusters
        .iter()
        .filter_map(|cluster| cluster.nav.get(&listed).copied())
        .sum()
}

const fn flagged(counts: Counts) -> Counts {
    Counts {
        total: None,
        ..counts
    }
}

pub const fn status_value(status: Status) -> &'static str {
    match status {
        Status::Err => "Failing",
        Status::Warn => "Degraded",
        Status::Ok => "Healthy",
        Status::Progress => "Progressing",
        Status::Suspended => "Suspended",
        Status::Offline => "Offline",
    }
}

pub fn image_name(image: &str) -> &str {
    split_tag(image).0
}

pub fn image_tag(image: &str) -> Option<&str> {
    split_tag(image).1
}

fn split_tag(image: &str) -> (&str, Option<&str>) {
    let path = image.rfind(PATH_SEPARATOR).map_or(0, |at| at + 1);
    match image[path..].rfind(TAG_SEPARATOR) {
        Some(at) => {
            let split = path + at;
            (&image[..split], Some(&image[split + 1..]))
        }
        None => (image, None),
    }
}

pub fn cell<'a>(kind: ResourceKind, row: &'a Resource, column: &str) -> Option<&'a str> {
    let ix = kind.columns().iter().position(|title| *title == column)?;
    row.cells.get(ix).map(String::as_str)
}

pub fn image(kind: ResourceKind, row: &Resource) -> Option<&str> {
    cell(kind, row, IMAGE_COLUMN).filter(|image| !image.is_empty())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sibling {
    pub cluster: ClusterId,
    pub name: String,
    pub status: Status,
    pub ready: String,
    pub tag: Option<String>,
}

pub fn siblings(
    snapshot: &Snapshot,
    kind: ResourceKind,
    at: &ClusterId,
    namespace: &str,
    name: &str,
) -> Vec<Sibling> {
    let find = |cluster: &ClusterId| {
        snapshot
            .detail(cluster)?
            .resources(kind)
            .iter()
            .find(|row| row.namespace == namespace && row.name == name)
    };
    let own_tag = find(at)
        .and_then(|row| image(kind, row))
        .and_then(image_tag);
    snapshot
        .clusters
        .iter()
        .filter(|cluster| &cluster.id != at)
        .filter_map(|cluster| {
            let row = find(&cluster.id)?;
            let tag = image(kind, row).and_then(image_tag);
            Some(Sibling {
                cluster: cluster.id.clone(),
                name: cluster.name.clone(),
                status: row.status,
                ready: cell(kind, row, READY_COLUMN).unwrap_or_default().to_owned(),
                tag: tag.filter(|tag| Some(*tag) != own_tag).map(str::to_owned),
            })
        })
        .collect()
}

pub const fn header_check(marked: usize, shown: usize) -> Check {
    if marked == 0 {
        Check::Off
    } else if marked >= shown {
        Check::On
    } else {
        Check::Mixed
    }
}

pub const fn selects_all(marked: usize) -> bool {
    marked == 0
}

pub type Extras<'a> = [(&'static str, Option<Cow<'a, str>>)];

pub fn split_query(query: &TextFilter, facets: &[&str]) -> (TextFilter, TextFilter) {
    let (extra, rest) = query
        .terms
        .iter()
        .cloned()
        .partition(|term| facets.contains(&term.facet.as_str()));
    let kept = TextFilter {
        words: query.words.clone(),
        terms: rest,
    };
    let extra = TextFilter {
        words: Vec::new(),
        terms: extra,
    };
    (kept, extra)
}

pub fn keeps(extra: &TextFilter, values: &Extras<'_>) -> bool {
    extra.terms_match(|term| {
        values
            .iter()
            .filter(|(facet, _)| *facet == term.facet)
            .filter_map(|(_, value)| value.as_deref())
            .any(|value| term.matches_value(value))
    })
}

pub fn facets<'a, T: Searchable>(
    rows: impl IntoIterator<Item = (&'a T, Vec<(&'static str, Option<Cow<'a, str>>)>)>,
) -> Facets {
    let mut facets = Facets::default();
    for facet in [STATUS_FACET, NAMESPACE_FACET, CLUSTER_FACET, IMAGE_FACET] {
        facets.declare(facet);
    }
    for (row, extras) in rows {
        for (facet, value) in &extras {
            if let Some(value) = value {
                facets.count(facet, value);
            }
        }
        let labels = row.labels();
        let label_sets: Vec<&crate::filter::Labels> = labels.iter().map(AsRef::as_ref).collect();
        T::FILTER.count_facets(row, &(), row.namespace(), &label_sets, &mut facets);
    }
    facets
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Uid;
    use crate::flock::tests::sample;
    use crate::helm::{HelmKey, HelmRelease};
    use crate::query::Term;

    fn id(name: &str) -> ClusterId {
        ClusterId::new(name)
    }

    fn deployment(name: &str, status: Status, ready: &str, image: &str) -> Resource {
        Resource {
            namespace: "payments".into(),
            name: name.into(),
            uid: Uid::new(String::new()),
            status,
            cells: vec![ready.into(), ready.into(), ready.into(), image.into()],
            age: std::time::Duration::ZERO,
            yaml: String::new(),
            labels: BTreeMap::new(),
            conditions: Vec::new(),
            diagnosis: None,
        }
    }

    fn with_deployments(rows: &[(&str, Resource)]) -> Snapshot {
        let mut snapshot = sample();
        for (cluster, row) in rows {
            let detail = snapshot.details.entry(id(cluster)).or_default();
            detail
                .resources
                .entry(ResourceKind::Deployment)
                .or_default()
                .push(row.clone());
        }
        snapshot
    }

    #[test]
    fn images_split_into_name_and_tag_past_a_registry_port() {
        assert_eq!(image_tag("checkout-api:2.14.0"), Some("2.14.0"));
        assert_eq!(image_name("checkout-api:2.14.0"), "checkout-api");
        assert_eq!(image_tag("registry:5000/team/api"), None);
        assert_eq!(
            image_name("registry:5000/team/api:1.2"),
            "registry:5000/team/api"
        );
        assert_eq!(image_tag("registry:5000/team/api:1.2"), Some("1.2"));
    }

    #[test]
    fn other_clusters_list_the_same_object_with_a_tag_only_when_it_differs() {
        let snapshot = with_deployments(&[
            ("a", deployment("api", Status::Err, "2/3", "api:2.14.0")),
            ("b", deployment("api", Status::Ok, "3/3", "api:2.13.4")),
            ("c", deployment("api", Status::Warn, "0/1", "api:2.14.0")),
            ("c", deployment("web", Status::Ok, "1/1", "web:1")),
        ]);
        let found = siblings(
            &snapshot,
            ResourceKind::Deployment,
            &id("a"),
            "payments",
            "api",
        );
        let summary: Vec<(&str, Status, &str, Option<&str>)> = found
            .iter()
            .map(|s| {
                (
                    s.name.as_str(),
                    s.status,
                    s.ready.as_str(),
                    s.tag.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("b", Status::Ok, "3/3", Some("2.13.4")),
                ("c", Status::Warn, "0/1", None)
            ]
        );
        let none = siblings(
            &snapshot,
            ResourceKind::Deployment,
            &id("c"),
            "payments",
            "web",
        );
        assert_eq!(none, []);
    }

    #[test]
    fn other_clusters_only_look_at_the_clusters_in_the_view() {
        let snapshot = with_deployments(&[
            ("a", deployment("api", Status::Ok, "1/1", "api:1")),
            ("b", deployment("api", Status::Ok, "1/1", "api:1")),
        ]);
        let view = snapshot.restricted(&[id("a"), id("c")]);
        let found = siblings(&view, ResourceKind::Deployment, &id("a"), "payments", "api");
        assert_eq!(found, []);
    }

    #[test]
    fn the_header_checkbox_is_off_mixed_or_on_and_clicking_it_selects_all_only_from_none() {
        assert_eq!(header_check(0, 4), Check::Off);
        assert_eq!(header_check(2, 4), Check::Mixed);
        assert_eq!(header_check(4, 4), Check::On);
        assert_eq!(header_check(0, 0), Check::Off);
        assert!(selects_all(0));
        assert!(!selects_all(2));
    }

    #[test]
    fn cluster_status_and_image_terms_are_matched_outside_the_row_spec() {
        let query = TextFilter {
            words: vec!["api".into()],
            terms: vec![
                Term::include(CLUSTER_FACET, "prod-eu-west"),
                Term::include(NAMESPACE_FACET, "payments"),
                Term::exclude(STATUS_FACET, "Failing"),
            ],
        };
        let (kept, extra) = split_query(&query, &EXTRA_FACETS);
        assert_eq!(kept.words, ["api"]);
        assert_eq!(kept.terms, [Term::include(NAMESPACE_FACET, "payments")]);
        assert_eq!(extra.terms.len(), 2);
        let row = |cluster: &'static str, status: Status| {
            vec![
                (CLUSTER_FACET, Some(Cow::Borrowed(cluster))),
                (STATUS_FACET, Some(Cow::Borrowed(status_value(status)))),
            ]
        };
        assert!(keeps(&extra, &row("prod-eu-west", Status::Ok)));
        assert!(!keeps(&extra, &row("prod-eu-west", Status::Err)));
        assert!(!keeps(&extra, &row("prod-us-east", Status::Ok)));
        assert!(keeps(&TextFilter::default(), &row("x", Status::Err)));
    }

    #[test]
    fn facets_lead_with_status_namespace_cluster_and_image_and_count_rows() {
        let rows = [
            deployment("api", Status::Err, "0/1", "api:1"),
            deployment("web", Status::Ok, "1/1", "web:2"),
        ];
        let extras = |row: &Resource, cluster: &'static str| {
            vec![
                (STATUS_FACET, Some(Cow::Borrowed(status_value(row.status)))),
                (CLUSTER_FACET, Some(Cow::Borrowed(cluster))),
                (
                    IMAGE_FACET,
                    image(ResourceKind::Deployment, row)
                        .map(image_name)
                        .map(|name| Cow::Owned(name.to_owned())),
                ),
            ]
        };
        let found = facets(
            rows.iter()
                .zip(["a", "b"])
                .map(|(row, c)| (row, extras(row, c))),
        );
        let names: Vec<&str> = found.names().map(|(name, _)| name).collect();
        assert_eq!(
            names,
            [STATUS_FACET, NAMESPACE_FACET, CLUSTER_FACET, IMAGE_FACET]
        );
        assert_eq!(found.values(CLUSTER_FACET), [("a", 1), ("b", 1)]);
        assert_eq!(found.values(STATUS_FACET), [("Failing", 1), ("Healthy", 1)]);
        assert_eq!(found.values(NAMESPACE_FACET), [("payments", 2)]);
    }

    #[test]
    fn badges_sum_problems_across_the_clusters_in_view_without_totals() {
        let mut snapshot = sample();
        for (cluster, counts) in [
            ("a", Counts::total(10).with_err(1)),
            ("b", Counts::total(5).with_err(2).with_warn(1)),
        ] {
            if let Some(summary) = snapshot.clusters.iter_mut().find(|c| c.id == id(cluster)) {
                summary.nav.insert(Page::Deployments, counts);
            }
        }
        snapshot.helm.push(HelmRelease {
            key: HelmKey::new(id("a"), "monitoring", "grafana"),
            chart: String::new(),
            chart_version: String::new(),
            app_version: String::new(),
            revision: 1,
            status: Status::Warn,
            state: String::new(),
            updated: None,
            flux: None,
        });
        let all = badges(&snapshot);
        let expected = Counts::default().with_err(3).with_warn(1);
        assert_eq!(all.get(&Page::FlockDeployments), Some(&expected));
        let warn = Counts::default().with_warn(1);
        assert_eq!(all.get(&Page::FlockReleases), Some(&warn));
        let view = snapshot.restricted(&[id("a")]);
        let one = badges(&view);
        let err = Counts::default().with_err(1);
        assert_eq!(one.get(&Page::FlockDeployments), Some(&err));
        assert_eq!(one.get(&Page::FlockPods), Some(&Counts::default()));
    }
}
