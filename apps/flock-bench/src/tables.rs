use std::fmt;
use std::time::Duration;

use flock::Screen;
use flock_core::cluster::Namespace;
use flock_core::crd::{Crd, CustomObjects};
use flock_core::events::{ClusterEvent, Event};
use flock_core::flux::FluxObject;
use flock_core::helm::HelmRelease;
use flock_core::nodes::Node;
use flock_core::pods::Pod;
use flock_core::resources::{EVENT_LIST, POD_LIST, Resource, ResourceKind};
use flock_core::settings::NamespaceSelection;
use flock_core::{ClusterId, Page, Snapshot, Uid};

const ROW_COUNTS: [usize; 3] = [1_000, 5_000, 20_000];
const QUERY_ROW: usize = 42;
const TOUCH_LABEL: &str = "flock-bench/generation";
pub const GROUPED_TABLE: &str = "pods";
pub const GROUPED_BY: &str = "namespace";

#[derive(Clone, Copy)]
pub struct Table {
    pub name: &'static str,
    pub screen: Screen,
    pub page: Page,
    scale: fn(&mut Snapshot, &ClusterId, Page, usize),
    touch: fn(&mut Snapshot, &ClusterId, Page, u64) -> bool,
    count: fn(&mut Snapshot, &ClusterId, Page) -> usize,
}

pub const TABLES: [Table; 11] = [
    Table::new::<Pod>("pods", Screen::Pods, Page::Pods),
    Table::new::<Resource>("deployments", Screen::Resources, Page::Deployments),
    Table::new::<Event>("events", Screen::Events, Page::Events),
    Table::new::<HelmRelease>("helm", Screen::HelmReleases, Page::Releases),
    Table::new::<FluxObject>(
        "kustomizations",
        Screen::Kustomizations,
        Page::Kustomizations,
    ),
    Table::new::<FluxObject>(
        "flux-helmreleases",
        Screen::Kustomizations,
        Page::HelmReleases,
    ),
    Table::new::<FluxObject>("sources", Screen::Kustomizations, Page::Sources),
    Table::new::<CustomObjects>("custom", Screen::CustomResources, Page::CustomResources),
    Table::new::<Node>("nodes", Screen::Nodes, Page::Nodes),
    Table::new::<Namespace>("namespaces", Screen::Namespaces, Page::Namespaces),
    Table::new::<ClusterEvent>("flux-events", Screen::FluxEvents, Page::FluxEvents),
];

impl Table {
    const fn new<R: Rows>(name: &'static str, screen: Screen, page: Page) -> Self {
        Self {
            name,
            screen,
            page,
            scale: scale::<R>,
            touch: touch::<R>,
            count: count::<R>,
        }
    }

    pub fn snapshot(&self, cluster: &ClusterId, rows: usize) -> Snapshot {
        let mut snapshot = flock_demo::snapshot();
        (self.scale)(&mut snapshot, cluster, self.page, rows);
        show_all_namespaces(&mut snapshot, cluster);
        snapshot
    }

    pub fn touch(&self, snapshot: &mut Snapshot, cluster: &ClusterId, generation: u64) -> bool {
        (self.touch)(snapshot, cluster, self.page, generation)
    }

    pub fn count(&self, snapshot: &Snapshot, cluster: &ClusterId) -> usize {
        let mut copy = snapshot.clone();
        (self.count)(&mut copy, cluster, self.page)
    }
}

pub struct Workload {
    pub table: Table,
    pub rows: usize,
}

impl fmt::Display for Workload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.table.name, self.rows)
    }
}

pub fn workloads() -> impl Iterator<Item = Workload> {
    TABLES.into_iter().flat_map(|table| {
        ROW_COUNTS
            .into_iter()
            .map(move |rows| Workload { table, rows })
    })
}

pub fn grouped_workloads() -> impl Iterator<Item = Workload> {
    workloads().filter(|workload| workload.table.name == GROUPED_TABLE)
}

pub fn query() -> String {
    number(QUERY_ROW)
}

fn number(ix: usize) -> String {
    format!("{ix:05}")
}

fn numbered(name: &str, ix: usize) -> String {
    format!("{name}-{}", number(ix))
}

fn show_all_namespaces(snapshot: &mut Snapshot, cluster: &ClusterId) {
    let crds: Vec<String> = snapshot
        .crds(cluster)
        .iter()
        .map(|crd: &Crd| crd.key.qualified())
        .collect();
    let builtin = [POD_LIST, EVENT_LIST]
        .into_iter()
        .chain(ResourceKind::ALL.into_iter().map(ResourceKind::name))
        .map(str::to_owned);
    let lists: Vec<String> = builtin.chain(crds).collect();
    let settings = snapshot
        .catalog
        .clusters
        .iter_mut()
        .filter(|settings| &settings.cluster == cluster);
    for settings in settings {
        for list in &lists {
            settings
                .preferences
                .select_namespace(list, NamespaceSelection::All);
        }
    }
}

trait Rows {
    type Row: Clone;
    fn groups<'a>(
        snapshot: &'a mut Snapshot,
        cluster: &ClusterId,
        page: Page,
    ) -> Vec<&'a mut Vec<Self::Row>>;
    fn viewed(row: &Self::Row, cluster: &ClusterId, page: Page) -> bool;
    fn number(row: &mut Self::Row, ix: usize);
    fn touch(row: &mut Self::Row, generation: u64);
}

fn scale<R: Rows>(snapshot: &mut Snapshot, cluster: &ClusterId, page: Page, rows: usize) {
    for group in R::groups(snapshot, cluster, page) {
        let (templates, others): (Vec<R::Row>, Vec<R::Row>) = std::mem::take(group)
            .into_iter()
            .partition(|row| R::viewed(row, cluster, page));
        let scaled = templates
            .iter()
            .cycle()
            .take(rows)
            .enumerate()
            .map(|(ix, template)| {
                let mut row = template.clone();
                R::number(&mut row, ix);
                row
            });
        *group = scaled.chain(others).collect();
    }
}

fn touch<R: Rows>(
    snapshot: &mut Snapshot,
    cluster: &ClusterId,
    page: Page,
    generation: u64,
) -> bool {
    let Some(group) = R::groups(snapshot, cluster, page).into_iter().next() else {
        return false;
    };
    let viewed = group
        .iter()
        .filter(|row| R::viewed(row, cluster, page))
        .count();
    let Some(ix) = u64::try_from(viewed)
        .ok()
        .and_then(|viewed| generation.checked_rem(viewed))
        .and_then(|ix| usize::try_from(ix).ok())
    else {
        return false;
    };
    let row = group
        .iter_mut()
        .filter(|row| R::viewed(row, cluster, page))
        .nth(ix);
    let Some(row) = row else {
        return false;
    };
    R::touch(row, generation);
    true
}

fn count<R: Rows>(snapshot: &mut Snapshot, cluster: &ClusterId, page: Page) -> usize {
    R::groups(snapshot, cluster, page)
        .into_iter()
        .next()
        .map_or(0, |group| {
            group
                .iter()
                .filter(|row| R::viewed(row, cluster, page))
                .count()
        })
}

fn older(age: &mut Duration, generation: u64) {
    *age += Duration::from_secs(generation + 1);
}

fn detail<'a>(
    snapshot: &'a mut Snapshot,
    cluster: &ClusterId,
) -> Option<&'a mut flock_core::ClusterDetail> {
    snapshot.details.get_mut(cluster)
}

impl Rows for Pod {
    type Row = Self;

    fn groups<'a>(snapshot: &'a mut Snapshot, _: &ClusterId, _: Page) -> Vec<&'a mut Vec<Self>> {
        vec![&mut snapshot.pods]
    }

    fn viewed(row: &Self, cluster: &ClusterId, _: Page) -> bool {
        &row.cluster == cluster
    }

    fn number(row: &mut Self, ix: usize) {
        row.name = numbered(&row.name, ix);
        row.uid = Uid::new(row.name.clone());
    }

    fn touch(row: &mut Self, generation: u64) {
        older(&mut row.age, generation);
    }
}

impl Rows for Resource {
    type Row = Self;

    fn groups<'a>(
        snapshot: &'a mut Snapshot,
        cluster: &ClusterId,
        page: Page,
    ) -> Vec<&'a mut Vec<Self>> {
        let kind = ResourceKind::for_page(page);
        let rows = detail(snapshot, cluster)
            .zip(kind)
            .and_then(|(detail, kind)| detail.resources.get_mut(&kind));
        rows.into_iter().collect()
    }

    fn viewed(_: &Self, _: &ClusterId, _: Page) -> bool {
        true
    }

    fn number(row: &mut Self, ix: usize) {
        row.name = numbered(&row.name, ix);
        row.uid = Uid::new(row.name.clone());
    }

    fn touch(row: &mut Self, generation: u64) {
        older(&mut row.age, generation);
    }
}

impl Rows for CustomObjects {
    type Row = Resource;

    fn groups<'a>(
        snapshot: &'a mut Snapshot,
        cluster: &ClusterId,
        _: Page,
    ) -> Vec<&'a mut Vec<Resource>> {
        snapshot
            .custom
            .iter_mut()
            .filter(|objects| &objects.cluster == cluster)
            .map(|objects| &mut objects.rows)
            .collect()
    }

    fn viewed(_: &Resource, _: &ClusterId, _: Page) -> bool {
        true
    }

    fn number(row: &mut Resource, ix: usize) {
        <Resource as Rows>::number(row, ix);
    }

    fn touch(row: &mut Resource, generation: u64) {
        <Resource as Rows>::touch(row, generation);
    }
}

impl Rows for Event {
    type Row = Self;

    fn groups<'a>(
        snapshot: &'a mut Snapshot,
        cluster: &ClusterId,
        _: Page,
    ) -> Vec<&'a mut Vec<Self>> {
        detail(snapshot, cluster)
            .map(|detail| &mut detail.events)
            .into_iter()
            .collect()
    }

    fn viewed(_: &Self, _: &ClusterId, _: Page) -> bool {
        true
    }

    fn number(row: &mut Self, ix: usize) {
        row.object = numbered(&row.object, ix);
        row.details.uid = numbered(&row.details.uid, ix);
    }

    fn touch(row: &mut Self, generation: u64) {
        older(&mut row.age, generation);
    }
}

impl Rows for ClusterEvent {
    type Row = Self;

    fn groups<'a>(snapshot: &'a mut Snapshot, _: &ClusterId, _: Page) -> Vec<&'a mut Vec<Self>> {
        vec![&mut snapshot.flux_events]
    }

    fn viewed(_: &Self, _: &ClusterId, _: Page) -> bool {
        true
    }

    fn number(row: &mut Self, ix: usize) {
        <Event as Rows>::number(&mut row.event, ix);
    }

    fn touch(row: &mut Self, generation: u64) {
        <Event as Rows>::touch(&mut row.event, generation);
    }
}

impl Rows for HelmRelease {
    type Row = Self;

    fn groups<'a>(snapshot: &'a mut Snapshot, _: &ClusterId, _: Page) -> Vec<&'a mut Vec<Self>> {
        vec![&mut snapshot.helm]
    }

    fn viewed(row: &Self, cluster: &ClusterId, _: Page) -> bool {
        &row.key.cluster == cluster
    }

    fn number(row: &mut Self, ix: usize) {
        row.key.name = numbered(&row.key.name, ix);
    }

    fn touch(row: &mut Self, generation: u64) {
        row.revision += 1;
        if let Some(updated) = row.updated.as_mut() {
            older(updated, generation);
        }
    }
}

impl Rows for FluxObject {
    type Row = Self;

    fn groups<'a>(snapshot: &'a mut Snapshot, _: &ClusterId, _: Page) -> Vec<&'a mut Vec<Self>> {
        vec![&mut snapshot.flux]
    }

    fn viewed(row: &Self, cluster: &ClusterId, page: Page) -> bool {
        &row.key.cluster == cluster && row.key.kind.page() == page
    }

    fn number(row: &mut Self, ix: usize) {
        row.key.name = numbered(&row.key.name, ix);
        row.uid = Uid::new(row.key.name.clone());
    }

    fn touch(row: &mut Self, generation: u64) {
        row.labels
            .insert(TOUCH_LABEL.into(), generation.to_string());
    }
}

impl Rows for Node {
    type Row = Self;

    fn groups<'a>(
        snapshot: &'a mut Snapshot,
        cluster: &ClusterId,
        _: Page,
    ) -> Vec<&'a mut Vec<Self>> {
        detail(snapshot, cluster)
            .map(|detail| &mut detail.nodes)
            .into_iter()
            .collect()
    }

    fn viewed(_: &Self, _: &ClusterId, _: Page) -> bool {
        true
    }

    fn number(row: &mut Self, ix: usize) {
        row.name = numbered(&row.name, ix);
        row.uid = Uid::new(row.name.clone());
    }

    fn touch(row: &mut Self, generation: u64) {
        older(&mut row.age, generation);
    }
}

impl Rows for Namespace {
    type Row = Self;

    fn groups<'a>(
        snapshot: &'a mut Snapshot,
        cluster: &ClusterId,
        _: Page,
    ) -> Vec<&'a mut Vec<Self>> {
        detail(snapshot, cluster)
            .map(|detail| &mut detail.namespaces)
            .into_iter()
            .collect()
    }

    fn viewed(_: &Self, _: &ClusterId, _: Page) -> bool {
        true
    }

    fn number(row: &mut Self, ix: usize) {
        row.name = numbered(&row.name, ix);
    }

    fn touch(row: &mut Self, generation: u64) {
        older(&mut row.age, generation);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROWS: usize = 2_500;

    #[test]
    fn every_table_has_demo_rows_to_repeat() {
        let cluster = crate::live::viewed_cluster();
        let demo = flock_demo::snapshot();
        let empty: Vec<&str> = TABLES
            .iter()
            .filter(|table| table.count(&demo, &cluster) == 0)
            .map(|table| table.name)
            .collect();
        assert_eq!(empty, Vec::<&str>::new());
    }

    #[test]
    fn scaling_shows_exactly_the_requested_rows_and_keeps_the_rest() {
        let cluster = crate::live::viewed_cluster();
        for table in TABLES {
            let snapshot = table.snapshot(&cluster, ROWS);
            assert_eq!(table.count(&snapshot, &cluster), ROWS, "{}", table.name);
        }
        let pods = TABLES[0].snapshot(&cluster, ROWS);
        let demo = flock_demo::snapshot();
        let others = |snapshot: &Snapshot| {
            snapshot
                .pods
                .iter()
                .filter(|pod| pod.cluster != cluster)
                .count()
        };
        assert_eq!(others(&pods), others(&demo));
    }

    #[test]
    fn scaled_rows_have_unique_names() {
        let cluster = crate::live::viewed_cluster();
        let snapshot = TABLES[0].snapshot(&cluster, ROWS);
        let names: std::collections::BTreeSet<&str> = snapshot
            .pods
            .iter()
            .filter(|pod| pod.cluster == cluster)
            .map(|pod| pod.name.as_str())
            .collect();
        assert_eq!(names.len(), ROWS);
    }

    #[test]
    fn every_list_shows_all_namespaces() {
        let cluster = crate::live::viewed_cluster();
        let snapshot = TABLES[0].snapshot(&cluster, ROWS);
        let lists = [POD_LIST, EVENT_LIST, ResourceKind::Deployment.name()];
        let selections: Vec<NamespaceSelection> = snapshot
            .catalog
            .clusters
            .iter()
            .filter(|settings| settings.cluster == cluster)
            .flat_map(|settings| lists.map(|list| settings.preferences.namespace_selection(list)))
            .collect();
        assert_eq!(selections, vec![NamespaceSelection::All; lists.len()]);
    }

    #[test]
    fn touching_changes_one_viewed_row_and_moves_on_each_generation() {
        let cluster = crate::live::viewed_cluster();
        for table in TABLES {
            let mut snapshot = table.snapshot(&cluster, ROWS);
            let before = snapshot.clone();
            assert!(table.touch(&mut snapshot, &cluster, 0), "{}", table.name);
            let once = snapshot.clone();
            assert!(table.touch(&mut snapshot, &cluster, 1), "{}", table.name);
            assert_ne!(before, once, "{}", table.name);
            assert_ne!(once, snapshot, "{}", table.name);
            assert_eq!(table.count(&snapshot, &cluster), ROWS, "{}", table.name);
        }
        assert!(!TABLES[0].touch(&mut Snapshot::default(), &cluster, 0));
    }

    #[test]
    fn the_query_matches_one_numbered_row() {
        let names: Vec<String> = (0..ROWS).map(|ix| numbered("row", ix)).collect();
        let hits = names.iter().filter(|name| name.contains(&query())).count();
        assert_eq!(hits, 1);
    }
}
