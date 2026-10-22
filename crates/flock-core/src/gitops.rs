use std::collections::BTreeMap;
use std::time::Duration;

use crate::filter::{FilterSpec, TextFilter};
use crate::{ClusterId, ResourceRef, Status};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FluxKind {
    Kustomization,
    HelmRelease,
}

impl FluxKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Kustomization => "Kustomization",
            Self::HelmRelease => "HelmRelease",
        }
    }

    pub const fn plural(self) -> &'static str {
        match self {
            Self::Kustomization => "Kustomizations",
            Self::HelmRelease => "HelmReleases",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncCell {
    pub status: Option<Status>,
    pub revision: String,
    pub note: String,
    pub behind: bool,
}

impl SyncCell {
    pub fn not_deployed() -> Self {
        Self {
            status: None,
            revision: String::new(),
            note: "not deployed".into(),
            behind: false,
        }
    }

    pub fn is_deployed(&self) -> bool {
        self.status.is_some()
    }

    pub fn out_of_sync(&self) -> bool {
        self.behind || self.status.is_some_and(|status| status != Status::Ok)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncRow {
    pub label_sets: Vec<BTreeMap<String, String>>,
    pub kind: FluxKind,
    pub namespace: String,
    pub name: String,
    pub detail: String,
    pub cells: Vec<SyncCell>,
}

impl SyncRow {
    pub fn out_of_sync(&self) -> bool {
        self.cells.iter().any(SyncCell::out_of_sync)
    }

    pub const FILTER: FilterSpec<Self> = FilterSpec {
        fields: &[
            ("name", |r, (), out| out.push(r.name.as_str().into())),
            ("namespace", |r, (), out| {
                out.push(r.namespace.as_str().into());
            }),
        ],
        namespace: true,
        labels: true,
    };

    pub fn matches(&self, query: &TextFilter) -> bool {
        let label_sets: Vec<&BTreeMap<String, String>> = self.label_sets.iter().collect();
        Self::FILTER.matches(self, query, Some(&self.namespace), &label_sets)
    }

    pub fn target(&self, cluster: &ClusterId) -> ResourceRef {
        ResourceRef {
            cluster: cluster.clone(),
            kind: self.kind.as_str().into(),
            namespace: Some(self.namespace.clone()),
            name: self.name.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncGroup {
    pub kind: FluxKind,
    pub total: u32,
    pub rows: Vec<SyncRow>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SyncFilter {
    #[default]
    All,
    Kustomizations,
    HelmReleases,
    OutOfSync,
}

impl SyncFilter {
    pub const ALL: [Self; 4] = [
        Self::All,
        Self::Kustomizations,
        Self::HelmReleases,
        Self::OutOfSync,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Kustomizations => "Kustomizations",
            Self::HelmReleases => "HelmReleases",
            Self::OutOfSync => "Out of sync",
        }
    }

    fn keeps_kind(self, kind: FluxKind) -> bool {
        match self {
            Self::All => true,
            Self::Kustomizations => kind == FluxKind::Kustomization,
            Self::HelmReleases => kind == FluxKind::HelmRelease,
            Self::OutOfSync => true,
        }
    }

    fn keeps_row(self, row: &SyncRow) -> bool {
        self != Self::OutOfSync || row.out_of_sync()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisibleGroup<'a> {
    pub kind: FluxKind,
    pub total: u32,
    pub rows: Vec<&'a SyncRow>,
}

impl VisibleGroup<'_> {
    pub fn count_label(&self) -> String {
        format!("{} of {}", self.rows.len(), self.total)
    }
}

pub fn visible_groups<'a>(
    groups: &'a [SyncGroup],
    filter: SyncFilter,
    query: &TextFilter,
) -> Vec<VisibleGroup<'a>> {
    groups
        .iter()
        .filter(|group| filter.keeps_kind(group.kind))
        .map(|group| VisibleGroup {
            kind: group.kind,
            total: group.total,
            rows: group
                .rows
                .iter()
                .filter(|r| filter.keeps_row(r) && r.matches(query))
                .collect(),
        })
        .filter(|group| !group.rows.is_empty())
        .collect()
}

pub fn filter_counts(groups: &[SyncGroup], query: &TextFilter) -> Vec<(SyncFilter, usize)> {
    SyncFilter::ALL
        .iter()
        .map(|filter| {
            let rows = visible_groups(groups, *filter, query)
                .iter()
                .map(|group| group.rows.len())
                .sum();
            (*filter, rows)
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub kind: String,
    pub namespace: String,
    pub name: String,
    pub url: String,
    pub reference: Option<String>,
    pub revision: String,
    pub fetched: Duration,
    pub ready: u32,
    pub clusters: u32,
}

impl Source {
    pub const FILTER: FilterSpec<Self> = FilterSpec {
        fields: &[
            ("name", |s, (), out| out.push(s.name.as_str().into())),
            ("namespace", |s, (), out| {
                out.push(s.namespace.as_str().into());
            }),
            ("url", |s, (), out| out.push(s.url.as_str().into())),
        ],
        namespace: true,
        labels: false,
    };

    pub fn matches(&self, query: &TextFilter) -> bool {
        Self::FILTER.matches(self, query, Some(&self.namespace), &[])
    }

    pub fn status(&self) -> Status {
        if self.ready >= self.clusters {
            Status::Ok
        } else {
            Status::Warn
        }
    }

    pub fn ready_label(&self) -> String {
        format!("{} {}/{}", self.status().glyph(), self.ready, self.clusters)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Controller {
    pub name: String,
    pub status: Option<Status>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FluxInstall {
    pub cluster: ClusterId,
    pub version: String,
    pub upgrade: Option<String>,
    pub controllers: Vec<Controller>,
}

impl FluxInstall {
    pub fn status(&self) -> Status {
        if self.upgrade.is_some() {
            Status::Warn
        } else {
            Status::Ok
        }
    }

    pub fn note(&self) -> String {
        match &self.upgrade {
            Some(version) => format!("{} {version} available", Status::Warn.glyph()),
            None => "healthy".into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitOps {
    pub clusters: Vec<ClusterId>,
    pub sources: Vec<Source>,
    pub groups: Vec<SyncGroup>,
    pub installs: Vec<FluxInstall>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(status: Status, behind: bool) -> SyncCell {
        SyncCell {
            status: Some(status),
            revision: "a41e07c".into(),
            note: "ready".into(),
            behind,
        }
    }

    fn row(kind: FluxKind, name: &str, cells: Vec<SyncCell>) -> SyncRow {
        SyncRow {
            label_sets: Vec::new(),
            kind,
            namespace: "flux-system".into(),
            name: name.into(),
            detail: String::new(),
            cells,
        }
    }

    fn groups() -> Vec<SyncGroup> {
        vec![
            SyncGroup {
                kind: FluxKind::Kustomization,
                total: 12,
                rows: vec![
                    row(
                        FluxKind::Kustomization,
                        "in-sync",
                        vec![cell(Status::Ok, false), SyncCell::not_deployed()],
                    ),
                    row(
                        FluxKind::Kustomization,
                        "failing",
                        vec![cell(Status::Ok, false), cell(Status::Err, false)],
                    ),
                ],
            },
            SyncGroup {
                kind: FluxKind::HelmRelease,
                total: 9,
                rows: vec![row(
                    FluxKind::HelmRelease,
                    "behind",
                    vec![cell(Status::Ok, true), cell(Status::Ok, false)],
                )],
            },
        ]
    }

    fn shape<'a>(groups: &[VisibleGroup<'a>]) -> Vec<(FluxKind, Vec<&'a str>)> {
        groups
            .iter()
            .map(|g| (g.kind, g.rows.iter().map(|r| r.name.as_str()).collect()))
            .collect()
    }

    #[test]
    fn grid_label_tokens_match_a_complete_label_set_from_one_cluster() {
        let mut row = row(FluxKind::Kustomization, "apps", Vec::new());
        row.label_sets = vec![
            BTreeMap::from([
                ("env".into(), "prod".into()),
                ("team".into(), "platform".into()),
            ]),
            BTreeMap::from([
                ("env".into(), "stage".into()),
                ("team".into(), "checkout".into()),
            ]),
        ];
        assert!(row.matches(&TextFilter::parse("label:env=stage l:team=checkout")));
        assert!(!row.matches(&TextFilter::parse("label:env=prod l:team=checkout")));
    }

    #[test]
    fn out_of_sync_means_not_ready_or_behind() {
        assert!(!cell(Status::Ok, false).out_of_sync());
        assert!(cell(Status::Ok, true).out_of_sync());
        assert!(cell(Status::Progress, false).out_of_sync());
        assert!(cell(Status::Suspended, false).out_of_sync());
        assert!(!SyncCell::not_deployed().out_of_sync());
    }

    #[test]
    fn all_keeps_every_group_and_row() {
        let groups = groups();
        let visible = visible_groups(&groups, SyncFilter::All, &TextFilter::default());
        assert_eq!(
            shape(&visible),
            [
                (FluxKind::Kustomization, vec!["in-sync", "failing"]),
                (FluxKind::HelmRelease, vec!["behind"]),
            ]
        );
        assert_eq!(visible[0].count_label(), "2 of 12");
    }

    #[test]
    fn text_filters_narrow_rows_and_counts() {
        let groups = groups();
        let query = TextFilter::parse("fail");
        let visible = visible_groups(&groups, SyncFilter::All, &query);
        assert_eq!(
            shape(&visible),
            [(FluxKind::Kustomization, vec!["failing"])]
        );
        let counts: Vec<usize> = filter_counts(&groups, &query)
            .into_iter()
            .map(|(_, n)| n)
            .collect();
        assert_eq!(counts, [1, 1, 0, 1]);
    }

    #[test]
    fn kind_filters_keep_one_group() {
        let groups = groups();
        let helm = visible_groups(&groups, SyncFilter::HelmReleases, &TextFilter::default());
        assert_eq!(shape(&helm), [(FluxKind::HelmRelease, vec!["behind"])]);
        let ks = visible_groups(&groups, SyncFilter::Kustomizations, &TextFilter::default());
        assert_eq!(ks.len(), 1);
        assert_eq!(ks[0].kind, FluxKind::Kustomization);
    }

    #[test]
    fn out_of_sync_drops_clean_rows_and_empty_groups() {
        let groups = groups();
        let visible = visible_groups(&groups, SyncFilter::OutOfSync, &TextFilter::default());
        assert_eq!(
            shape(&visible),
            [
                (FluxKind::Kustomization, vec!["failing"]),
                (FluxKind::HelmRelease, vec!["behind"]),
            ]
        );
        assert_eq!(visible[0].count_label(), "1 of 12");
        let clean = vec![SyncGroup {
            kind: FluxKind::HelmRelease,
            total: 1,
            rows: vec![row(
                FluxKind::HelmRelease,
                "ok",
                vec![cell(Status::Ok, false)],
            )],
        }];
        assert_eq!(
            visible_groups(&clean, SyncFilter::OutOfSync, &TextFilter::default()).len(),
            0
        );
        let counts: Vec<usize> = filter_counts(&groups, &TextFilter::default())
            .into_iter()
            .map(|(_, n)| n)
            .collect();
        assert_eq!(
            counts.first(),
            Some(&groups.iter().map(|g| g.rows.len()).sum())
        );
    }

    #[test]
    fn row_target_points_at_the_object_on_one_cluster() {
        let row = row(FluxKind::HelmRelease, "cert-manager", Vec::new());
        let target = row.target(&ClusterId::new("staging"));
        assert_eq!(target.cluster, ClusterId::new("staging"));
        assert_eq!(target.kind, "HelmRelease");
        assert_eq!(target.to_string(), "flux-system/cert-manager");
    }

    #[test]
    fn source_ready_label_warns_when_a_cluster_lags() {
        let mut source = Source {
            kind: "GitRepository".into(),
            namespace: "flux-system".into(),
            name: "fleet-infra".into(),
            url: String::new(),
            reference: None,
            revision: String::new(),
            fetched: Duration::ZERO,
            ready: 4,
            clusters: 4,
        };
        assert_eq!(source.ready_label(), "● 4/4");
        source.ready = 3;
        assert_eq!(source.status(), Status::Warn);
        assert_eq!(source.ready_label(), "▲ 3/4");
    }

    #[test]
    fn install_note_offers_the_upgrade() {
        let mut install = FluxInstall {
            cluster: ClusterId::new("dev-kind"),
            version: "v2.3.0".into(),
            upgrade: Some("v2.4.0".into()),
            controllers: Vec::new(),
        };
        assert_eq!(install.note(), "▲ v2.4.0 available");
        assert_eq!(install.status(), Status::Warn);
        install.upgrade = None;
        assert_eq!(install.note(), "healthy");
        assert_eq!(install.status(), Status::Ok);
    }
}
