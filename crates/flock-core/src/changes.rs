use std::time::Duration;

use crate::filter::{FilterSpec, TextFilter};
use crate::overview::plural;
use crate::time::age;
use crate::{ClusterId, Status};

const SHORT_SHA: usize = 7;
const ADDED: &str = "+";
const REMOVED: &str = "−";
const SEPARATOR: &str = " · ";
const UNAVAILABLE: &str = "source unavailable";
const NO_GIT_SOURCE: &str = "No Kustomization on a connected cluster builds from a git source.";
const HISTORY_FAILED: &str = "Could not read the git history";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangesLog {
    pub repository: String,
    pub branch: String,
    pub fetched: Duration,
    pub lanes: Vec<Lane>,
    pub commits: Vec<Commit>,
    pub unavailable: Option<String>,
    pub pending: bool,
}

impl ChangesLog {
    pub fn source_label(&self) -> String {
        format!("{}/{}", self.repository, self.branch)
    }

    pub fn fetched_label(&self) -> String {
        match &self.unavailable {
            Some(reason) => format!("{UNAVAILABLE}{SEPARATOR}{reason}"),
            None => format!("{} fetched {} ago", self.repository, age(self.fetched)),
        }
    }

    pub fn empty_reason(&self) -> String {
        match &self.unavailable {
            Some(reason) => format!("{HISTORY_FAILED}{SEPARATOR}{reason}"),
            None => NO_GIT_SOURCE.to_owned(),
        }
    }

    pub fn recent(&self, count: usize) -> &[Commit] {
        self.commits.get(..count).unwrap_or(&self.commits)
    }

    pub fn commit(&self, sha: &str) -> Option<&Commit> {
        if sha.is_empty() {
            return None;
        }
        self.commits.iter().find(|c| c.sha.starts_with(sha))
    }

    pub fn visible(&self, query: &TextFilter, cluster: Option<&ClusterId>) -> Vec<&Commit> {
        self.commits
            .iter()
            .filter(|commit| commit.matches(query))
            .filter(|commit| cluster.is_none_or(|id| commit.rollout_on(id).is_some()))
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lane {
    pub cluster: ClusterId,
    pub short: String,
    pub label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetKind {
    Kustomization,
    HelmRelease,
}

impl TargetKind {
    pub const fn short(self) -> &'static str {
        match self {
            Self::Kustomization => "ks",
            Self::HelmRelease => "hr",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub kind: TargetKind,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rollout {
    pub cluster: ClusterId,
    pub status: Status,
    pub object: String,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewKind {
    Source,
    Rendered(ClusterId),
}

impl ViewKind {
    pub fn label(&self) -> String {
        match self {
            Self::Source => "Source".into(),
            Self::Rendered(cluster) => format!("Rendered{SEPARATOR}{cluster}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rendering {
    pub kind: ViewKind,
    pub note: String,
    pub files: Vec<FilePatch>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineChange {
    Context,
    Added,
    Removed,
    Hunk,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchLine {
    pub change: LineChange,
    pub number: u32,
    pub text: String,
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePatch {
    pub path: String,
    pub lines: Vec<PatchLine>,
}

impl FilePatch {
    pub fn added(&self) -> usize {
        self.count(LineChange::Added)
    }

    pub fn removed(&self) -> usize {
        self.count(LineChange::Removed)
    }

    fn count(&self, change: LineChange) -> usize {
        self.lines.iter().filter(|l| l.change == change).count()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub message: String,
    pub author: String,
    pub age: Duration,
    pub when: String,
    pub target: Target,
    pub rollout: Vec<Rollout>,
    pub views: Vec<Rendering>,
}

impl Commit {
    pub fn short_sha(&self) -> &str {
        self.sha.get(..SHORT_SHA).unwrap_or(&self.sha)
    }

    pub fn age_label(&self) -> String {
        age(self.age)
    }

    pub fn rollout_on(&self, cluster: &ClusterId) -> Option<&Rollout> {
        self.rollout.iter().find(|r| &r.cluster == cluster)
    }

    pub fn source(&self) -> Option<&Rendering> {
        self.views.iter().find(|v| v.kind == ViewKind::Source)
    }

    pub fn stat(&self) -> String {
        let files = self
            .source()
            .map(|s| s.files.as_slice())
            .unwrap_or_default();
        let added = files.iter().map(FilePatch::added).sum();
        let removed = files.iter().map(FilePatch::removed).sum();
        let noun = if files.len() == 1 { "file" } else { "files" };
        let counts = change_counts(added, removed);
        let summary = format!("{} {noun}", files.len());
        if counts.is_empty() {
            summary
        } else {
            format!("{summary}{SEPARATOR}{counts}")
        }
    }

    pub fn target_sentence(&self) -> String {
        let scope = match self.rollout.as_slice() {
            [only] => only.cluster.to_string(),
            all => plural(all.len(), "cluster", "clusters"),
        };
        format!(
            "affects {}/{} on {scope}",
            self.target.kind.short(),
            self.target.name
        )
    }

    pub fn default_view(&self) -> ViewKind {
        let worst = self.rollout.iter().max_by_key(|r| r.status);
        let rendered = worst
            .filter(|r| r.status != Status::Ok)
            .map(|r| ViewKind::Rendered(r.cluster.clone()))
            .filter(|kind| self.views.iter().any(|v| &v.kind == kind));
        rendered.unwrap_or(ViewKind::Source)
    }

    pub fn view(&self, chosen: Option<&ViewKind>) -> Option<&Rendering> {
        let fallback = self.default_view();
        let wanted = chosen.unwrap_or(&fallback);
        let find = |kind: &ViewKind| self.views.iter().find(|v| &v.kind == kind);
        find(wanted)
            .or_else(|| find(&fallback))
            .or_else(|| self.views.first())
    }

    pub const FILTER: FilterSpec<Self> = FilterSpec {
        fields: &[
            ("commit", |c, (), out| out.push(c.sha.as_str().into())),
            ("message", |c, (), out| out.push(c.message.as_str().into())),
            ("author", |c, (), out| out.push(c.author.as_str().into())),
            ("target", |c, (), out| {
                out.push(c.target.name.as_str().into());
            }),
            ("file", |c, (), out| {
                let files = c.source().map(|s| s.files.as_slice()).unwrap_or_default();
                out.extend(files.iter().map(|f| f.path.as_str().into()));
            }),
        ],
        namespace: false,
        labels: false,
    };

    pub fn matches(&self, query: &TextFilter) -> bool {
        Self::FILTER.matches(self, query, None, &[])
    }
}

pub const fn outcome_label(status: Status) -> &'static str {
    match status {
        Status::Ok => "applied",
        Status::Err => "failing",
        Status::Suspended => "held",
        Status::Warn => "retrying",
        Status::Progress => "pending",
        Status::Offline => "not yet",
    }
}

pub fn change_counts(added: usize, removed: usize) -> String {
    let parts = [
        (added > 0).then(|| format!("{ADDED}{added}")),
        (removed > 0).then(|| format!("{REMOVED}{removed}")),
    ];
    parts.into_iter().flatten().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_log_names_its_reason() {
        let mut log = ChangesLog::default();
        assert_eq!(log.empty_reason(), NO_GIT_SOURCE);
        log.unavailable = Some("podinfo: timed out".into());
        assert_eq!(
            log.empty_reason(),
            "Could not read the git history · podinfo: timed out"
        );
    }

    fn line(change: LineChange, number: u32) -> PatchLine {
        PatchLine {
            change,
            number,
            text: String::new(),
            note: None,
        }
    }

    fn file(path: &str, added: u32, removed: u32) -> FilePatch {
        let adds = (0..added).map(|n| line(LineChange::Added, n));
        let dels = (0..removed).map(|n| line(LineChange::Removed, n));
        FilePatch {
            path: path.into(),
            lines: adds.chain(dels).collect(),
        }
    }

    fn rollout(cluster: &str, status: Status) -> Rollout {
        Rollout {
            cluster: ClusterId::new(cluster),
            status,
            object: "apps-payments".into(),
            note: String::new(),
        }
    }

    fn view(kind: ViewKind, files: Vec<FilePatch>) -> Rendering {
        Rendering {
            kind,
            note: String::new(),
            files,
        }
    }

    fn commit(sha: &str, rollout: Vec<Rollout>, views: Vec<Rendering>) -> Commit {
        Commit {
            sha: sha.into(),
            message: String::new(),
            author: String::new(),
            age: Duration::ZERO,
            when: String::new(),
            target: Target {
                kind: TargetKind::Kustomization,
                name: "apps-payments".into(),
            },
            rollout,
            views,
        }
    }

    fn east() -> ViewKind {
        ViewKind::Rendered(ClusterId::new("prod-us-east"))
    }

    fn payments() -> Commit {
        commit(
            "a41e07c3f91d",
            vec![
                rollout("staging", Status::Ok),
                rollout("prod-us-east", Status::Err),
                rollout("prod-eu-west", Status::Suspended),
            ],
            vec![
                view(
                    ViewKind::Source,
                    vec![
                        file("apps/payments/base/deployment.yaml", 7, 1),
                        file("apps/payments/base/configmap.yaml", 0, 2),
                    ],
                ),
                view(east(), vec![file("Deployment/payments/checkout-api", 7, 1)]),
            ],
        )
    }

    fn monitoring() -> Commit {
        commit(
            "3be91f2a0c77",
            vec![rollout("staging", Status::Warn)],
            vec![view(
                ViewKind::Source,
                vec![file("infrastructure/monitoring/staging/release.yaml", 1, 1)],
            )],
        )
    }

    #[test]
    fn stat_sums_source_files_and_omits_zero_counts() {
        assert_eq!(payments().stat(), "2 files · +7 −3");
        assert_eq!(monitoring().stat(), "1 file · +1 −1");
        let added_only = commit(
            "x",
            Vec::new(),
            vec![view(ViewKind::Source, vec![file("a", 4, 0)])],
        );
        assert_eq!(added_only.stat(), "1 file · +4");
        assert_eq!(change_counts(0, 2), "−2");
        assert_eq!(change_counts(0, 0), "");
    }

    #[test]
    fn short_sha_takes_seven_characters() {
        assert_eq!(payments().short_sha(), "a41e07c");
        assert_eq!(commit("abc", Vec::new(), Vec::new()).short_sha(), "abc");
    }

    #[test]
    fn target_names_one_cluster_or_counts_many() {
        assert_eq!(
            payments().target_sentence(),
            "affects ks/apps-payments on 3 clusters"
        );
        let mut single = monitoring();
        single.target = Target {
            kind: TargetKind::HelmRelease,
            name: "kube-prometheus-stack".into(),
        };
        assert_eq!(
            single.target_sentence(),
            "affects hr/kube-prometheus-stack on staging"
        );
    }

    #[test]
    fn default_view_renders_for_the_worst_cluster_when_available() {
        assert_eq!(payments().default_view(), east());
        assert_eq!(monitoring().default_view(), ViewKind::Source);
    }

    #[test]
    fn chosen_view_falls_back_when_the_commit_lacks_it() {
        let payments = payments();
        let source = payments.view(Some(&ViewKind::Source));
        assert_eq!(source.map(|v| &v.kind), Some(&ViewKind::Source));
        assert_eq!(payments.view(None).map(|v| &v.kind), Some(&east()));
        let monitoring = monitoring();
        let fallback = monitoring.view(Some(&east()));
        assert_eq!(fallback.map(|v| &v.kind), Some(&ViewKind::Source));
        assert_eq!(commit("x", Vec::new(), Vec::new()).view(None), None);
    }

    #[test]
    fn visible_filters_by_path_text_and_cluster() {
        let log = ChangesLog {
            commits: vec![payments(), monitoring()],
            ..ChangesLog::default()
        };
        let shas = |commits: Vec<&Commit>| -> Vec<String> {
            commits.iter().map(|c| c.short_sha().to_owned()).collect()
        };
        let visible = |text: &str, cluster: Option<&ClusterId>| {
            shas(log.visible(&TextFilter::parse(text), cluster))
        };
        assert_eq!(visible("", None), ["a41e07c", "3be91f2"]);
        assert_eq!(visible(" apps/payments ", None), ["a41e07c"]);
        assert_eq!(visible("monitoring", None), ["3be91f2"]);
        assert_eq!(visible("3be91", None), ["3be91f2"]);
        assert_eq!(visible("nothing-here", None), Vec::<String>::new());
        let east = ClusterId::new("prod-us-east");
        assert_eq!(visible("", Some(&east)), ["a41e07c"]);
    }

    #[test]
    fn outcomes_read_as_one_word_per_status() {
        assert_eq!(outcome_label(Status::Ok), "applied");
        assert_eq!(outcome_label(Status::Suspended), "held");
        assert_eq!(outcome_label(Status::Offline), "not yet");
        let log = ChangesLog {
            commits: vec![payments(), monitoring()],
            ..ChangesLog::default()
        };
        assert_eq!(log.recent(1).len(), 1);
        assert_eq!(log.recent(5).len(), 2);
    }

    #[test]
    fn commits_resolve_by_abbreviated_sha() {
        let log = ChangesLog {
            commits: vec![payments(), monitoring()],
            ..ChangesLog::default()
        };
        let found = |sha: &str| log.commit(sha).map(|c| c.short_sha().to_owned());
        assert_eq!(found("3be91f2"), Some("3be91f2".into()));
        assert_eq!(found("a41e07c3f91d"), Some("a41e07c".into()));
        assert_eq!(found(""), None);
        assert_eq!(found("ffff"), None);
    }

    #[test]
    fn labels_read_like_the_design() {
        let log = ChangesLog {
            repository: "fleet-infra".into(),
            branch: "main".into(),
            fetched: Duration::from_mins(4),
            ..ChangesLog::default()
        };
        assert_eq!(log.source_label(), "fleet-infra/main");
        assert_eq!(log.fetched_label(), "fleet-infra fetched 4m ago");
        assert_eq!(east().label(), "Rendered · prod-us-east");
        let failed = ChangesLog {
            unavailable: Some("fleet-infra: Permission denied (publickey).".into()),
            ..log
        };
        assert_eq!(
            failed.fetched_label(),
            "source unavailable · fleet-infra: Permission denied (publickey)."
        );
        assert_eq!(ViewKind::Source.label(), "Source");
    }
}
