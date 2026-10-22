use std::sync::Arc;
use std::time::Duration;

use flock_core::home::{Problem, Revision};
use flock_core::{ClusterId, Counts, ResourceRef, Status};
use flock_flux::ObjectKind;
use jiff::Timestamp;
use k8s_openapi::api::core::v1::Event;

use crate::problems::target_events;
use crate::resources::CustomKind;

const ROOT_NAME: &str = "flux-system";
const SHORT_SHA: usize = 7;
pub const WATCHED: [ObjectKind; 2] = [ObjectKind::Kustomization, ObjectKind::HelmRelease];

pub fn custom_kind(kind: ObjectKind) -> CustomKind {
    let (group, version) = kind.api_version().split_once('/').unwrap_or_default();
    CustomKind::new(group, version, kind.as_str())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FluxHealth {
    pub kind: ObjectKind,
    pub namespace: String,
    pub name: String,
    pub status: Status,
    pub reason: String,
    pub message: String,
    pub condition: String,
    pub suspended: bool,
    pub revision: Option<String>,
    pub since: Duration,
}

pub fn counts(objects: &[FluxHealth]) -> Counts {
    let total = u32::try_from(objects.len()).unwrap_or(u32::MAX);
    let tally = |status: Status| {
        let matching = objects.iter().filter(|o| o.status == status).count();
        u32::try_from(matching).unwrap_or(u32::MAX)
    };
    Counts::total(total)
        .with_err(tally(Status::Err))
        .with_warn(tally(Status::Warn))
        .with_sus(tally(Status::Suspended))
}

pub fn problems(
    cluster: &ClusterId,
    objects: &[FluxHealth],
    events: &[Arc<Event>],
    now: Timestamp,
) -> Vec<Problem> {
    objects
        .iter()
        .filter(|object| {
            matches!(
                object.status,
                Status::Err | Status::Warn | Status::Suspended
            )
        })
        .map(|object| {
            let target = ResourceRef {
                cluster: cluster.clone(),
                kind: object.kind.as_str().to_owned(),
                namespace: Some(object.namespace.clone()),
                name: object.name.clone(),
            };
            let events = target_events(&target, events, now);
            Problem {
                status: object.status,
                target,
                reason: object.reason.clone(),
                message: object.message.clone(),
                condition: object.condition.clone(),
                age: object.since,
                events,
            }
        })
        .collect()
}

pub fn short_revision(revision: &str) -> String {
    match revision.rsplit_once(':') {
        Some((_, hash)) => hash.chars().take(SHORT_SHA).collect(),
        None => revision
            .rsplit_once('@')
            .map_or(revision, |(_, tail)| tail)
            .to_owned(),
    }
}

pub fn revision(objects: &[FluxHealth]) -> Revision {
    let mut kustomizations: Vec<&FluxHealth> = objects
        .iter()
        .filter(|o| o.kind == ObjectKind::Kustomization && o.revision.is_some())
        .collect();
    kustomizations.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    let root = kustomizations
        .iter()
        .find(|o| o.name == ROOT_NAME && o.namespace == ROOT_NAME)
        .or_else(|| kustomizations.first());
    match root {
        Some(object) => Revision {
            sha: object
                .revision
                .as_deref()
                .map(short_revision)
                .unwrap_or_default(),
            held: object.suspended,
        },
        None => Revision {
            sha: String::new(),
            held: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn health(
        kind: ObjectKind,
        namespace: &str,
        name: &str,
        status: Status,
        revision: Option<&str>,
    ) -> FluxHealth {
        FluxHealth {
            kind,
            namespace: namespace.into(),
            name: name.into(),
            status,
            reason: format!("{name} reason"),
            message: format!("{name} message"),
            condition: "Ready=False".into(),
            suspended: status == Status::Suspended,
            revision: revision.map(str::to_owned),
            since: Duration::from_mins(4),
        }
    }

    fn objects() -> Vec<FluxHealth> {
        vec![
            health(
                ObjectKind::Kustomization,
                "flux-system",
                "broken-kustomization",
                Status::Err,
                None,
            ),
            health(
                ObjectKind::HelmRelease,
                "search",
                "search-indexer",
                Status::Suspended,
                None,
            ),
            health(
                ObjectKind::Kustomization,
                "flux-system",
                "apps",
                Status::Ok,
                Some("master@sha1:b6b680fe507be26c0a5e8c8a1b6c3a0e8d9f0a11"),
            ),
        ]
    }

    #[test]
    fn failing_and_suspended_objects_become_problems() {
        let problems = problems(
            &ClusterId::new("kind-fleet-a"),
            &objects(),
            &[],
            Timestamp::UNIX_EPOCH,
        );
        let lines: Vec<(Status, String, &str, &str, &str)> = problems
            .iter()
            .map(|p| {
                (
                    p.status,
                    p.target.to_string(),
                    p.target.kind.as_str(),
                    p.reason.as_str(),
                    p.message.as_str(),
                )
            })
            .collect();
        assert_eq!(
            lines,
            [
                (
                    Status::Err,
                    "flux-system/broken-kustomization".into(),
                    "Kustomization",
                    "broken-kustomization reason",
                    "broken-kustomization message"
                ),
                (
                    Status::Suspended,
                    "search/search-indexer".into(),
                    "HelmRelease",
                    "search-indexer reason",
                    "search-indexer message"
                ),
            ]
        );
    }

    #[test]
    fn counts_and_revision_summarize_the_cluster() {
        let objects = objects();
        let counts = counts(&objects);
        assert_eq!(counts, Counts::total(3).with_err(1).with_sus(1));
        let revision = revision(&objects);
        assert_eq!(revision.sha, "b6b680f");
        assert!(!revision.held);
    }

    #[test]
    fn short_revisions() {
        assert_eq!(short_revision("main@sha1:a41e07c99"), "a41e07c");
        assert_eq!(short_revision("6.5.4"), "6.5.4");
        assert_eq!(short_revision("v1@6.5.4"), "6.5.4");
        assert_eq!(
            custom_kind(ObjectKind::HelmRelease),
            CustomKind::new("helm.toolkit.fluxcd.io", "v2", "HelmRelease")
        );
    }
}
