use std::collections::{BTreeMap, BTreeSet};

use crate::crd::{Crd, CrdKey, CustomObjects, Definition};
use crate::helm::{HelmKey, HelmRelease, HelmReleaseDetail};
use crate::home::{ClusterMetrics, problems_badge};
use crate::{
    CLUSTER_NAV, ChangesLog, ClusterCatalog, ClusterDetail, ClusterId, Comparison, Counts,
    FLOCK_NAV, FlockHome, FluxObject, Forward, GitOps, NavSection, Page, ResourceKind, ResourceRef,
    ResourceSummary, Status, WatchState,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterSummary {
    pub id: ClusterId,
    pub name: String,
    pub status: Status,
    pub state: String,
    pub distro: String,
    pub region: String,
    pub version: Option<String>,
    pub nodes: Option<u32>,
    pub problems: Counts,
    pub nav: BTreeMap<Page, Counts>,
    pub metrics: Option<ClusterMetrics>,
}

impl ClusterSummary {
    pub fn is_connected(&self) -> bool {
        self.status != Status::Offline
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub clusters: Vec<ClusterSummary>,
    pub flock_nav: BTreeMap<Page, Counts>,
    pub home: FlockHome,
    pub resources: Vec<ResourceSummary>,
    pub changes: ChangesLog,
    pub details: BTreeMap<ClusterId, ClusterDetail>,
    pub pages: BTreeMap<ClusterId, BTreeMap<Page, WatchState>>,
    pub gitops: GitOps,
    pub flux: Vec<FluxObject>,
    pub flux_events: Vec<crate::events::ClusterEvent>,
    #[cfg(feature = "loco")]
    pub loco: BTreeMap<ClusterId, crate::loco::LocoCluster>,
    pub comparisons: Vec<Comparison>,
    pub catalog: ClusterCatalog,
    pub pods: Vec<crate::pods::Pod>,
    pub forwards: Vec<Forward>,
    pub crds: BTreeMap<ClusterId, Vec<Crd>>,
    pub custom: Vec<CustomObjects>,
    pub helm: Vec<HelmRelease>,
    pub helm_details: Vec<HelmReleaseDetail>,
}

impl Snapshot {
    pub fn cluster(&self, id: &ClusterId) -> Option<&ClusterSummary> {
        self.clusters.iter().find(|c| &c.id == id)
    }

    pub fn nav_sections(&self, cluster: Option<&ClusterId>) -> Vec<&'static NavSection> {
        match cluster {
            None => FLOCK_NAV.iter().collect(),
            Some(id) => CLUSTER_NAV
                .iter()
                .filter(|section| self.shows_section(section, id))
                .collect(),
        }
    }

    #[cfg(feature = "loco")]
    fn shows_section(&self, section: &NavSection, id: &ClusterId) -> bool {
        !section.is_loco() || self.loco.contains_key(id)
    }

    #[cfg(not(feature = "loco"))]
    #[allow(clippy::unused_self)]
    fn shows_section(&self, _: &NavSection, _: &ClusterId) -> bool {
        true
    }

    pub fn crds(&self, id: &ClusterId) -> &[Crd] {
        self.crds.get(id).map_or(&[], Vec::as_slice)
    }

    pub fn crd(&self, id: &ClusterId, key: &CrdKey) -> Option<&Crd> {
        self.crds(id).iter().find(|crd| &crd.key == key)
    }

    pub fn definitions(&self, id: &ClusterId) -> Vec<Definition> {
        let mut definitions: Vec<Definition> = self
            .crds(id)
            .iter()
            .map(|crd| Definition::of(crd, self.known_objects(id, &crd.key)))
            .collect();
        definitions.sort_by(|a, b| {
            a.key
                .kind
                .cmp(&b.key.kind)
                .then_with(|| a.key.group.cmp(&b.key.group))
        });
        definitions
    }

    fn known_objects(&self, id: &ClusterId, key: &CrdKey) -> Option<usize> {
        let Some(kind) = key.flux_kind() else {
            return self
                .custom_objects(id, key)
                .map(|objects| objects.rows.len());
        };
        let objects = self
            .flux
            .iter()
            .filter(|object| &object.key.cluster == id && object.key.kind == kind);
        Some(objects.count())
    }

    pub fn custom_objects(&self, id: &ClusterId, key: &CrdKey) -> Option<&CustomObjects> {
        self.custom
            .iter()
            .find(|objects| &objects.cluster == id && &objects.key == key)
    }

    pub fn helm_releases(&self, id: &ClusterId) -> Vec<&HelmRelease> {
        self.helm
            .iter()
            .filter(|release| &release.key.cluster == id)
            .collect()
    }

    pub fn helm_release(&self, key: &HelmKey) -> Option<&HelmRelease> {
        self.helm.iter().find(|release| &release.key == key)
    }

    pub fn helm_detail(&self, key: &HelmKey) -> Option<&HelmReleaseDetail> {
        self.helm_details.iter().find(|detail| &detail.key == key)
    }

    pub fn detail(&self, id: &ClusterId) -> Option<&ClusterDetail> {
        self.details.get(id)
    }

    pub fn page_state(&self, id: &ClusterId, page: Page) -> Option<&WatchState> {
        self.pages.get(id)?.get(&page)
    }

    pub fn page_rows(&self, id: &ClusterId, page: Page) -> Option<usize> {
        let detail = self.detail(id);
        match page {
            Page::Nodes => detail.map(|detail| detail.nodes.len()),
            Page::Namespaces => detail.map(|detail| detail.namespaces.len()),
            Page::Events => detail.map(|detail| detail.events.len()),
            Page::Pods => Some(self.pods.iter().filter(|pod| &pod.cluster == id).count()),
            Page::Kustomizations => Some(self.flux_rows(id, page)),
            Page::HelmReleases => Some(self.flux_rows(id, page)),
            Page::Sources => Some(self.flux_rows(id, page)),
            other => {
                let kind = ResourceKind::for_page(other)?;
                detail.map(|detail| detail.resources(kind).len())
            }
        }
    }

    fn flux_rows(&self, id: &ClusterId, page: Page) -> usize {
        self.flux
            .iter()
            .filter(|object| &object.key.cluster == id && object.key.kind.page() == page)
            .count()
    }

    pub fn comparison(&self, resource: &ResourceRef) -> Option<&Comparison> {
        self.comparisons.iter().find(|c| c.matches(resource))
    }

    pub fn yaml(&self, resource: &ResourceRef) -> Option<&str> {
        let kind = ResourceKind::ALL
            .into_iter()
            .find(|kind| kind.name() == resource.kind)?;
        self.detail(&resource.cluster)?
            .resources(kind)
            .iter()
            .find(|row| {
                Some(row.namespace.as_str()) == resource.namespace.as_deref()
                    && row.name == resource.name
            })
            .map(|row| row.yaml.as_str())
            .filter(|yaml| !yaml.is_empty())
    }

    pub fn connected(&self) -> usize {
        self.clusters.iter().filter(|c| c.is_connected()).count()
    }

    pub fn problems(&self) -> Counts {
        self.clusters.iter().map(|c| c.problems).sum()
    }

    pub fn status(&self) -> Status {
        Status::worst(self.clusters.iter().map(|c| c.status))
    }

    pub fn keep_loaded(&mut self, previous: Self) {
        let Self { details, pods, .. } = previous;
        for (id, detail) in details {
            if self.connected_cluster(&id) && !self.details.contains_key(&id) {
                self.details.insert(id, detail);
            }
        }
        let unbuilt: BTreeSet<ClusterId> = self
            .clusters
            .iter()
            .filter(|cluster| cluster.is_connected())
            .filter(|cluster| cluster.nav.get(&Page::Pods).and_then(|c| c.total) > Some(0))
            .filter(|cluster| !self.pods.iter().any(|pod| pod.cluster == cluster.id))
            .map(|cluster| cluster.id.clone())
            .collect();
        let kept = pods
            .into_iter()
            .filter(|pod| unbuilt.contains(&pod.cluster));
        self.pods.extend(kept);
    }

    fn connected_cluster(&self, id: &ClusterId) -> bool {
        self.cluster(id).is_some_and(ClusterSummary::is_connected)
    }

    pub fn remove_cluster(&mut self, id: &ClusterId) {
        self.clusters.retain(|c| &c.id != id);
        self.details.remove(id);
        self.pages.remove(id);
        self.home.problems.retain(|p| &p.target.cluster != id);
        self.resources.retain(|r| &r.resource.cluster != id);
        self.flux.retain(|o| &o.key.cluster != id);
        #[cfg(feature = "loco")]
        self.loco.remove(id);
        self.catalog.clusters.retain(|c| &c.cluster != id);
        self.pods.retain(|p| &p.cluster != id);
        self.forwards.retain(|f| &f.target.cluster != id);
        self.crds.remove(id);
        self.custom.retain(|objects| &objects.cluster != id);
        self.helm.retain(|release| &release.key.cluster != id);
        self.helm_details.retain(|detail| &detail.key.cluster != id);
        let badge = problems_badge(&self.home.problems);
        self.flock_nav.insert(Page::Problems, badge);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn cluster(name: &str, status: Status, problems: Counts) -> ClusterSummary {
        ClusterSummary {
            id: ClusterId::new(name),
            name: name.into(),
            status,
            state: String::new(),
            distro: String::new(),
            region: String::new(),
            version: None,
            nodes: None,
            problems,
            nav: BTreeMap::new(),
            metrics: None,
        }
    }

    pub(crate) fn sample() -> Snapshot {
        Snapshot {
            clusters: vec![
                cluster("a", Status::Err, Counts::default().with_err(2).with_warn(3)),
                cluster("b", Status::Ok, Counts::default().with_sus(1)),
                cluster("c", Status::Offline, Counts::default()),
            ],
            flock_nav: BTreeMap::new(),
            home: FlockHome::default(),
            resources: Vec::new(),
            changes: ChangesLog::default(),
            details: BTreeMap::new(),
            pages: BTreeMap::new(),
            gitops: GitOps::default(),
            flux: Vec::new(),
            flux_events: Vec::new(),
            #[cfg(feature = "loco")]
            loco: BTreeMap::new(),
            comparisons: Vec::new(),
            catalog: ClusterCatalog::default(),
            pods: Vec::new(),
            forwards: Vec::new(),
            crds: BTreeMap::new(),
            custom: Vec::new(),
            helm: Vec::new(),
            helm_details: Vec::new(),
        }
    }

    #[test]
    fn rollups_sum_problems_and_take_worst_status() {
        let snapshot = sample();
        assert_eq!(snapshot.connected(), 2);
        assert_eq!(
            snapshot.problems(),
            Counts::default().with_err(2).with_warn(3).with_sus(1)
        );
        assert_eq!(snapshot.status(), Status::Err);
    }

    fn problem(cluster: &str, status: Status) -> crate::home::Problem {
        crate::home::Problem {
            status,
            target: ResourceRef {
                cluster: ClusterId::new(cluster),
                kind: "Pod".into(),
                namespace: Some("ns".into()),
                name: "p".into(),
            },
            message: String::new(),
            age: std::time::Duration::ZERO,
        }
    }

    #[test]
    fn removing_a_cluster_drops_its_problems_and_badge_counts() {
        let mut snapshot = sample();
        snapshot.home.problems = vec![
            problem("a", Status::Err),
            problem("a", Status::Warn),
            problem("b", Status::Warn),
        ];
        snapshot
            .flock_nav
            .insert(Page::Problems, problems_badge(&snapshot.home.problems));
        snapshot
            .details
            .insert(ClusterId::new("a"), ClusterDetail::default());
        snapshot.remove_cluster(&ClusterId::new("a"));
        assert!(snapshot.cluster(&ClusterId::new("a")).is_none());
        assert!(snapshot.detail(&ClusterId::new("a")).is_none());
        assert_eq!(snapshot.home.problems, vec![problem("b", Status::Warn)]);
        assert_eq!(
            snapshot.flock_nav.get(&Page::Problems),
            Some(&Counts::default().with_warn(1))
        );
    }

    #[test]
    fn page_rows_tell_an_empty_list_from_a_missing_detail() {
        let mut snapshot = sample();
        let id = ClusterId::new("a");
        assert_eq!(snapshot.page_rows(&id, Page::Deployments), None);
        assert_eq!(snapshot.page_rows(&id, Page::HelmReleases), Some(0));
        assert_eq!(snapshot.page_rows(&id, Page::ClusterOverview), None);
        snapshot
            .details
            .insert(id.clone(), ClusterDetail::default());
        assert_eq!(snapshot.page_rows(&id, Page::Deployments), Some(0));
        let states = BTreeMap::from([(Page::Deployments, WatchState::Synced)]);
        snapshot.pages.insert(id.clone(), states);
        assert_eq!(
            snapshot.page_state(&id, Page::Deployments),
            Some(&WatchState::Synced)
        );
        snapshot.remove_cluster(&id);
        assert_eq!(snapshot.page_state(&id, Page::Deployments), None);
    }

    fn pod_on(cluster: &str, name: &str) -> crate::pods::Pod {
        crate::pods::Pod {
            cluster: ClusterId::new(cluster),
            name: name.into(),
            ..crate::pods::Pod::default()
        }
    }

    #[test]
    fn data_a_snapshot_did_not_rebuild_stays_until_fresh_data_replaces_it() {
        let mut previous = sample();
        for id in ["a", "b", "c"] {
            previous
                .details
                .insert(ClusterId::new(id), ClusterDetail::default());
        }
        previous.pods = vec![pod_on("a", "old-a"), pod_on("b", "old-b")];
        let mut next = sample();
        for cluster in &mut next.clusters {
            cluster.nav.insert(Page::Pods, Counts::total(1));
        }
        let mut rebuilt = ClusterDetail::default();
        let row = crate::resources::tests::resource("web", "podinfo", Status::Ok);
        rebuilt
            .resources
            .insert(ResourceKind::Deployment, vec![row]);
        next.details.insert(ClusterId::new("b"), rebuilt.clone());
        next.pods = vec![pod_on("b", "new-b")];
        next.keep_loaded(previous);
        assert_eq!(
            next.detail(&ClusterId::new("a")),
            Some(&ClusterDetail::default())
        );
        assert_eq!(next.detail(&ClusterId::new("b")), Some(&rebuilt));
        assert_eq!(next.detail(&ClusterId::new("c")), None);
        let names: Vec<&str> = next.pods.iter().map(|pod| pod.name.as_str()).collect();
        assert_eq!(names, ["new-b", "old-a"]);
    }

    #[test]
    fn a_cluster_with_no_pods_drops_the_rows_it_had() {
        let mut previous = sample();
        previous.pods = vec![pod_on("a", "gone")];
        let mut next = sample();
        next.keep_loaded(previous);
        assert_eq!(next.pods, Vec::new());
    }

    #[test]
    fn yaml_comes_from_the_inspected_row() {
        let mut snapshot = sample();
        let mut row = crate::resources::tests::resource("payments", "checkout-api", Status::Ok);
        row.yaml = "kind: Deployment".into();
        let mut detail = ClusterDetail::default();
        detail.resources.insert(ResourceKind::Deployment, vec![row]);
        snapshot.details.insert(ClusterId::new("a"), detail);
        let target = ResourceRef {
            cluster: ClusterId::new("a"),
            kind: "Deployment".into(),
            namespace: Some("payments".into()),
            name: "checkout-api".into(),
        };
        assert_eq!(snapshot.yaml(&target), Some("kind: Deployment"));
        let other = ResourceRef {
            name: "cart".into(),
            ..target.clone()
        };
        assert_eq!(snapshot.yaml(&other), None);
        let pod = ResourceRef {
            kind: "Pod".into(),
            ..target
        };
        assert_eq!(snapshot.yaml(&pod), None);
    }

    #[test]
    fn definitions_sort_by_kind_and_count_only_known_objects() {
        let mut snapshot = sample();
        let id = ClusterId::new("a");
        let crd = |group: &str, kind: &str, plural: &str| Crd {
            key: CrdKey::new(group, kind),
            plural: plural.into(),
            version: "v1".into(),
            namespaced: true,
            columns: Vec::new(),
        };
        let issuer = crd("cert-manager.io", "Issuer", "issuers");
        let certificate = crd("cert-manager.io", "Certificate", "certificates");
        let kustomization = crd(
            "kustomize.toolkit.fluxcd.io",
            "Kustomization",
            "kustomizations",
        );
        snapshot
            .crds
            .insert(id.clone(), vec![kustomization, issuer, certificate.clone()]);
        snapshot.custom.push(CustomObjects {
            cluster: id.clone(),
            key: certificate.key,
            rows: Vec::new(),
        });
        snapshot.flux.push(crate::flux::tests::object(
            "a",
            crate::flux::Phase::Ready,
            "r1",
        ));
        snapshot.flux.push(crate::flux::tests::object(
            "b",
            crate::flux::Phase::Ready,
            "r1",
        ));
        let rows: Vec<(String, Option<usize>)> = snapshot
            .definitions(&id)
            .into_iter()
            .map(|definition| (definition.key.kind, definition.objects))
            .collect();
        assert_eq!(
            rows,
            [
                ("Certificate".to_owned(), Some(0)),
                ("Issuer".to_owned(), None),
                ("Kustomization".to_owned(), Some(1)),
            ]
        );
        assert_eq!(snapshot.definitions(&ClusterId::new("b")), Vec::new());
    }

    #[test]
    fn empty_flock_is_ok() {
        assert_eq!(Snapshot::default().status(), Status::Ok);
    }

    fn labels(sections: &[&NavSection]) -> Vec<Option<&'static str>> {
        sections.iter().map(|section| section.label).collect()
    }

    #[test]
    fn flock_home_shows_the_flock_nav() {
        let snapshot = sample();
        assert_eq!(
            snapshot.nav_sections(None),
            FLOCK_NAV.iter().collect::<Vec<_>>()
        );
    }

    #[cfg(not(feature = "loco"))]
    #[test]
    fn clusters_show_the_whole_cluster_nav() {
        let snapshot = sample();
        let sections = snapshot.nav_sections(Some(&ClusterId::new("a")));
        assert_eq!(sections, CLUSTER_NAV.iter().collect::<Vec<_>>());
        assert!(!labels(&sections).contains(&Some("Loco")));
    }

    #[cfg(feature = "loco")]
    #[test]
    fn loco_section_needs_loco_on_the_cluster() {
        let mut snapshot = sample();
        snapshot
            .loco
            .insert(ClusterId::new("a"), crate::loco::LocoCluster::default());
        let with = labels(&snapshot.nav_sections(Some(&ClusterId::new("a"))));
        let without = labels(&snapshot.nav_sections(Some(&ClusterId::new("b"))));
        assert!(with.contains(&Some("Loco")));
        assert!(!without.contains(&Some("Loco")));
        assert_eq!(with.len(), without.len() + 1);
    }
}
