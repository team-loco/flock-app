use std::collections::BTreeMap;
use std::time::Duration;

use super::{Workspace, short_digest};
use crate::filter::{FilterSpec, TextFilter};
use crate::{ResourceRef, Seen, Status, Uid};

const MISSING: &str = "—";
const BUILDING: &str = "building…";
const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 3600;
const RETENTION_HOURS_BELOW: u64 = 48;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildPhase {
    Pending,
    Running,
    Succeeded,
    Failed,
    Canceled,
    Unknown,
}

impl BuildPhase {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Running => "Running",
            Self::Succeeded => "Succeeded",
            Self::Failed => "Failed",
            Self::Canceled => "Canceled",
            Self::Unknown => "Unknown",
        }
    }

    pub const fn status(self) -> Status {
        match self {
            Self::Pending => Status::Progress,
            Self::Running => Status::Progress,
            Self::Succeeded => Status::Ok,
            Self::Failed => Status::Err,
            Self::Canceled => Status::Offline,
            Self::Unknown => Status::Offline,
        }
    }

    pub const fn is_active(self) -> bool {
        matches!(self, Self::Pending | Self::Running)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogTone {
    Plain,
    Dim,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildLogLine {
    pub tone: LogTone,
    pub text: String,
}

impl BuildLogLine {
    pub fn new(tone: LogTone, text: impl Into<String>) -> Self {
        Self {
            tone,
            text: text.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Build {
    pub labels: BTreeMap<String, String>,
    pub resource: ResourceRef,
    pub job_uid: Uid,
    pub id: String,
    pub service: String,
    pub workspace: Workspace,
    pub phase: BuildPhase,
    pub started: Option<String>,
    pub took: Option<Duration>,
    pub image_digest: Option<String>,
    pub cache_hit_percent: Option<u8>,
    pub job: Option<String>,
    pub node: Option<String>,
    pub fetch: Status,
    pub buildkit: Status,
    pub log: Vec<BuildLogLine>,
}

impl Build {
    pub fn seen(&self) -> Seen {
        Seen::new(self.resource.clone(), self.job_uid.clone())
    }

    pub const FILTER: FilterSpec<Self> = FilterSpec {
        fields: &[
            ("build", |b, (), out| out.push(b.id.as_str().into())),
            ("service", |b, (), out| out.push(b.service.as_str().into())),
            ("workspace", |b, (), out| {
                out.push(b.workspace.label().into());
            }),
            ("namespace", |b, (), out| {
                out.push(b.workspace.namespace.as_str().into());
            }),
        ],
        namespace: true,
        labels: true,
    };

    pub fn matches(&self, query: &TextFilter) -> bool {
        Self::FILTER.matches(
            self,
            query,
            Some(&self.workspace.namespace),
            &[&self.labels],
        )
    }

    pub fn took_label(&self) -> String {
        self.took
            .map_or_else(|| MISSING.to_owned(), minutes_seconds)
    }

    pub fn image_label(&self) -> String {
        if self.phase.is_active() {
            return BUILDING.to_owned();
        }
        let Some(digest) = &self.image_digest else {
            return MISSING.to_owned();
        };
        let short = short_digest(digest);
        match self.cache_hit_percent {
            Some(percent) => format!("{short} · cache {percent}%"),
            None => short,
        }
    }

    pub fn job_label(&self) -> String {
        let job = self
            .job
            .as_ref()
            .map_or_else(|| MISSING.to_owned(), |job| format!("job/{job}"));
        match &self.node {
            Some(node) => format!("{job} · node {node}"),
            None => job,
        }
    }
}

fn minutes_seconds(took: Duration) -> String {
    let secs = took.as_secs();
    format!(
        "{}m{:02}s",
        secs / SECONDS_PER_MINUTE,
        secs % SECONDS_PER_MINUTE
    )
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Builds {
    pub namespace: String,
    pub pool: Option<String>,
    pub retention: Option<Duration>,
    pub builds: Vec<Build>,
}

impl Builds {
    pub fn subtitle(&self) -> String {
        let mut parts = vec![self.namespace.clone()];
        parts.extend(self.pool.as_ref().map(|pool| format!("pool={pool}")));
        parts.extend(
            self.retention
                .map(|kept| format!("kept {}", retention(kept))),
        );
        parts.join(" · ")
    }

    pub fn find_mut(&mut self, resource: &ResourceRef) -> Option<&mut Build> {
        self.builds
            .iter_mut()
            .find(|build| &build.resource == resource)
    }
}

fn retention(kept: Duration) -> String {
    let hours = kept.as_secs() / SECONDS_PER_HOUR;
    if (1..RETENTION_HOURS_BELOW).contains(&hours) {
        format!("{hours}h")
    } else {
        crate::time::age(kept)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClusterId;

    fn build(phase: BuildPhase, digest: Option<&str>, cache: Option<u8>) -> Build {
        Build {
            labels: BTreeMap::new(),
            resource: ResourceRef {
                cluster: ClusterId::new("prod"),
                kind: "Build".into(),
                namespace: Some("loco-builds".into()),
                name: "build-1".into(),
            },
            id: "1".into(),
            service: "web".into(),
            workspace: Workspace {
                namespace: "ws-1".into(),
                name: None,
            },
            phase,
            started: None,
            took: Some(Duration::from_secs(108)),
            image_digest: digest.map(Into::into),
            cache_hit_percent: cache,
            job_uid: Uid::default(),
            job: None,
            node: None,
            fetch: Status::Ok,
            buildkit: Status::Ok,
            log: Vec::new(),
        }
    }

    #[test]
    fn list_label_tokens_match_metadata() {
        let mut build = build(BuildPhase::Running, None, None);
        build.labels.insert("team".into(), "checkout".into());
        assert!(build.matches(&TextFilter::parse("label:team=checkout")));
        assert!(!build.matches(&TextFilter::parse("l:team=platform")));
    }

    #[test]
    fn builds_match_service_and_workspace() {
        let web = build(BuildPhase::Failed, None, None);
        assert!(web.matches(&TextFilter::parse("WEB")));
        assert!(web.matches(&TextFilter::parse("ns:ws-1")));
        assert!(!web.matches(&TextFilter::parse("api")));
    }

    #[test]
    fn durations_read_as_minutes_and_padded_seconds() {
        assert_eq!(build(BuildPhase::Failed, None, None).took_label(), "1m48s");
        assert_eq!(minutes_seconds(Duration::from_secs(42)), "0m42s");
        assert_eq!(minutes_seconds(Duration::from_secs(131)), "2m11s");
    }

    #[test]
    fn image_column_shows_progress_digest_and_cache() {
        let digest = Some("sha256:9e1c0000000044d0");
        assert_eq!(
            build(BuildPhase::Running, digest, None).image_label(),
            "building…"
        );
        assert_eq!(
            build(BuildPhase::Succeeded, digest, Some(81)).image_label(),
            "sha256:9e1c…44d0 · cache 81%"
        );
        assert_eq!(
            build(BuildPhase::Succeeded, digest, None).image_label(),
            "sha256:9e1c…44d0"
        );
        assert_eq!(build(BuildPhase::Failed, None, None).image_label(), "—");
    }

    #[test]
    fn job_label_names_the_node_when_known() {
        let mut b = build(BuildPhase::Running, None, None);
        assert_eq!(b.job_label(), "—");
        b.job = Some("build-0192f3a1".into());
        b.node = Some("ip-10-4-40-7".into());
        assert_eq!(b.job_label(), "job/build-0192f3a1 · node ip-10-4-40-7");
    }

    #[test]
    fn subtitle_lists_namespace_pool_and_retention() {
        let builds = Builds {
            namespace: "loco-builds".into(),
            pool: Some("builds".into()),
            retention: Some(Duration::from_hours(24)),
            builds: Vec::new(),
        };
        assert_eq!(builds.subtitle(), "loco-builds · pool=builds · kept 24h");
        let bare = Builds {
            pool: None,
            retention: None,
            ..builds
        };
        assert_eq!(bare.subtitle(), "loco-builds");
    }
}
