use std::collections::BTreeSet;

use flock_core::changes::{
    ChangesLog, Commit, Lane, Rendering, Rollout, Target, TargetKind, ViewKind,
};
use flock_core::tabs::short_name;
use flock_core::{ClusterId, Status};
use flock_flux::ObjectKind;
use flock_flux::revision;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::Value;

use super::convert::{Parsed, elapsed};
use super::git::{GitCommit, MirrorState, RepoKey};

const DEFAULT_BRANCH: &str = "master";
const REPO_SEPARATOR: &str = ", ";
const TODAY: &str = "today";
const YESTERDAY: &str = "yesterday";
const CLOCK_FORMAT: &str = "%H:%M";
const DATE_FORMAT: &str = "%b %-d";

pub struct Consumer<'a> {
    pub cluster: &'a ClusterId,
    pub kustomization: &'a Parsed,
    pub source: &'a Parsed,
}

pub struct Repo<'a> {
    pub key: RepoKey,
    pub state: Option<MirrorState>,
    pub consumers: Vec<Consumer<'a>>,
}

pub fn repo_key(source: &Parsed) -> Option<RepoKey> {
    if source.kind() != ObjectKind::GitRepository {
        return None;
    }
    let spec = &source.object.spec;
    let url = spec.get("url").and_then(Value::as_str)?.to_owned();
    let branch = match spec.get("ref") {
        None => DEFAULT_BRANCH.to_owned(),
        Some(reference) => {
            let only_branch = reference
                .as_object()
                .is_some_and(|fields| fields.keys().all(|key| key == "branch"));
            if !only_branch {
                return None;
            }
            reference.get("branch").and_then(Value::as_str)?.to_owned()
        }
    };
    Some(RepoKey { url, branch })
}

pub fn wanted_revision(source: &Parsed) -> Option<String> {
    let artifact = source.object.artifact_revision.as_deref()?;
    revision::git(artifact).map(|parsed| parsed.sha.to_owned())
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub applied: Option<String>,
    pub attempted: Option<String>,
    pub fetched: Option<String>,
    pub suspended: bool,
    pub failure: Option<String>,
}

impl Progress {
    pub fn of(consumer: &Consumer<'_>) -> Self {
        let object = &consumer.kustomization.object;
        let sha = |revision: Option<&str>| {
            revision
                .and_then(revision::git)
                .map(|parsed| parsed.sha.to_owned())
        };
        let failure = object
            .ready()
            .filter(|condition| condition.status == "False")
            .map(|condition| {
                condition
                    .message
                    .lines()
                    .next()
                    .unwrap_or(condition.reason.as_str())
                    .to_owned()
            });
        Self {
            applied: sha(object.last_applied_revision.as_deref()),
            attempted: sha(object.last_attempted_revision.as_deref()),
            fetched: sha(consumer.source.object.artifact_revision.as_deref()),
            suspended: object.suspended,
            failure,
        }
    }
}

fn position(commits: &[&str], sha: Option<&str>) -> Option<usize> {
    let sha = sha?;
    let found = commits
        .iter()
        .position(|commit| commit.starts_with(sha) || sha.starts_with(commit));
    Some(found.unwrap_or(commits.len()))
}

pub fn outcome(commits: &[&str], index: usize, progress: &Progress) -> (Status, String) {
    let applied = position(commits, progress.applied.as_deref());
    let attempted = position(commits, progress.attempted.as_deref());
    let fetched = position(commits, progress.fetched.as_deref());
    let reached = |at: Option<usize>| at.is_some_and(|at| index >= at);
    if reached(applied) {
        let newest = applied == Some(index);
        return match (&progress.failure, newest) {
            (Some(failure), true) if attempted == applied => {
                (Status::Err, format!("applied · {failure}"))
            }
            (None, true) => (Status::Ok, "applied · healthy".to_owned()),
            (_, _) => (Status::Ok, "applied".to_owned()),
        };
    }
    if progress.suspended {
        return (
            Status::Suspended,
            "not applied · Kustomization suspended".to_owned(),
        );
    }
    if reached(attempted) {
        return match &progress.failure {
            Some(failure) => (Status::Err, format!("failed · {failure}")),
            None => (Status::Progress, "applying".to_owned()),
        };
    }
    if reached(fetched) {
        return (Status::Progress, "fetched · waiting to apply".to_owned());
    }
    (Status::Offline, "not yet reached".to_owned())
}

pub fn lanes(clusters: &[ClusterId]) -> Vec<Lane> {
    let names: Vec<&str> = clusters.iter().map(ClusterId::as_str).collect();
    clusters
        .iter()
        .map(|cluster| Lane {
            cluster: cluster.clone(),
            short: short_name(cluster.as_str(), &names),
            label: cluster.to_string(),
        })
        .collect()
}

pub fn when(time: Timestamp, now: Timestamp) -> String {
    let zone = TimeZone::system();
    let at = time.to_zoned(zone.clone());
    let today = now.to_zoned(zone).date();
    let clock = at.strftime(CLOCK_FORMAT).to_string();
    if at.date() == today {
        return format!("{TODAY} {clock}");
    }
    if today
        .yesterday()
        .is_ok_and(|yesterday| yesterday == at.date())
    {
        return format!("{YESTERDAY} {clock}");
    }
    at.strftime(DATE_FORMAT).to_string()
}

fn kustomization_path(consumer: &Consumer<'_>) -> String {
    consumer
        .kustomization
        .object
        .spec
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_start_matches("./")
        .trim_end_matches('/')
        .to_owned()
}

fn target<'a>(commit: &GitCommit, consumers: &'a [Consumer<'a>]) -> Option<&'a Consumer<'a>> {
    let touched = consumers.iter().find(|consumer| {
        let path = kustomization_path(consumer);
        commit
            .files
            .iter()
            .any(|file| path.is_empty() || file.path.starts_with(&path))
    });
    touched.or_else(|| {
        consumers
            .iter()
            .min_by_key(|consumer| &consumer.kustomization.object.identity.name)
    })
}

fn commit(
    commit: &GitCommit,
    index: usize,
    shas: &[&str],
    repo: &Repo<'_>,
    lanes: &[ClusterId],
    now: Timestamp,
) -> Option<Commit> {
    let target = target(commit, &repo.consumers)?;
    let target_name = &target.kustomization.object.identity.name;
    let rollout = lanes
        .iter()
        .filter_map(|cluster| {
            let on_cluster: Vec<&Consumer<'_>> = repo
                .consumers
                .iter()
                .filter(|consumer| consumer.cluster == cluster)
                .collect();
            let chosen = on_cluster
                .iter()
                .find(|consumer| &consumer.kustomization.object.identity.name == target_name)
                .or_else(|| on_cluster.first())?;
            let (status, note) = outcome(shas, index, &Progress::of(chosen));
            Some(Rollout {
                cluster: cluster.clone(),
                status,
                object: chosen.kustomization.object.identity.name.clone(),
                note,
            })
        })
        .collect();
    Some(Commit {
        sha: commit.sha.clone(),
        message: commit.subject.clone(),
        author: commit.author.clone(),
        age: elapsed(commit.time, now),
        when: when(commit.time, now),
        target: Target {
            kind: TargetKind::Kustomization,
            name: target_name.clone(),
        },
        rollout,
        views: vec![Rendering {
            kind: ViewKind::Source,
            truncated: commit.truncated,
            files: commit.files.clone(),
        }],
    })
}

pub fn changes_log(repos: &[Repo<'_>], clusters: &[ClusterId], now: Timestamp) -> ChangesLog {
    let consuming: BTreeSet<&ClusterId> = repos
        .iter()
        .flat_map(|repo| repo.consumers.iter().map(|consumer| consumer.cluster))
        .collect();
    let consumed: Vec<ClusterId> = clusters
        .iter()
        .filter(|cluster| consuming.contains(cluster))
        .cloned()
        .collect();
    let mut commits: Vec<Commit> = Vec::new();
    let mut fetched: Option<Timestamp> = None;
    let mut unavailable: Vec<String> = Vec::new();
    let mut pending = false;
    for repo in repos {
        match &repo.state {
            Some(MirrorState::Ready {
                commits: history,
                fetched: at,
            }) => {
                fetched = Some(fetched.map_or(*at, |seen| seen.min(*at)));
                let shas: Vec<&str> = history.iter().map(|c| c.sha.as_str()).collect();
                let built = history
                    .iter()
                    .enumerate()
                    .filter_map(|(ix, c)| commit(c, ix, &shas, repo, &consumed, now));
                commits.extend(built);
            }
            Some(MirrorState::Unavailable(reason)) => {
                unavailable.push(format!("{}: {reason}", repo.key.name()));
            }
            Some(MirrorState::Fetching) => pending = true,
            None => pending = true,
        }
    }
    commits.sort_by_key(|commit| commit.age);
    let names: Vec<String> = repos.iter().map(|repo| repo.key.name()).collect();
    let branches: BTreeSet<&str> = repos.iter().map(|repo| repo.key.branch.as_str()).collect();
    ChangesLog {
        repository: names.join(REPO_SEPARATOR),
        branch: branches
            .into_iter()
            .collect::<Vec<_>>()
            .join(REPO_SEPARATOR),
        fetched: fetched.map(|at| elapsed(at, now)).unwrap_or_default(),
        lanes: lanes(&consumed),
        commits,
        unavailable: (!unavailable.is_empty()).then(|| unavailable.join(REPO_SEPARATOR)),
        pending,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitops::convert::tests::{git_repository, kustomization, parsed};

    const SHAS: [&str; 3] = ["ccc3333", "bbb2222", "aaa1111"];

    fn progress(applied: &str, attempted: &str, fetched: &str) -> Progress {
        let some = |sha: &str| (!sha.is_empty()).then(|| sha.to_owned());
        Progress {
            applied: some(applied),
            attempted: some(attempted),
            fetched: some(fetched),
            suspended: false,
            failure: None,
        }
    }

    fn outcomes(progress: &Progress) -> Vec<(Status, String)> {
        (0..SHAS.len())
            .map(|ix| outcome(&SHAS, ix, progress))
            .collect()
    }

    #[test]
    fn applied_commits_and_older_are_applied() {
        let healthy = progress("bbb2222", "bbb2222", "bbb2222");
        assert_eq!(
            outcomes(&healthy),
            [
                (Status::Offline, "not yet reached".to_owned()),
                (Status::Ok, "applied · healthy".to_owned()),
                (Status::Ok, "applied".to_owned()),
            ]
        );
    }

    #[test]
    fn fetched_but_unapplied_commits_are_pending() {
        let waiting = progress("aaa1111", "aaa1111", "ccc3333");
        let statuses: Vec<Status> = outcomes(&waiting).into_iter().map(|(s, _)| s).collect();
        assert_eq!(statuses, [Status::Progress, Status::Progress, Status::Ok]);
    }

    #[test]
    fn failed_attempts_carry_the_object_reason() {
        let mut failing = progress("aaa1111", "ccc3333", "ccc3333");
        failing.failure = Some("kustomization path not found".to_owned());
        assert_eq!(
            outcomes(&failing)[0],
            (
                Status::Err,
                "failed · kustomization path not found".to_owned()
            )
        );
        let mut unhealthy = progress("ccc3333", "ccc3333", "ccc3333");
        unhealthy.failure = Some("health check failed".to_owned());
        assert_eq!(
            outcomes(&unhealthy)[0],
            (Status::Err, "applied · health check failed".to_owned())
        );
        assert_eq!(outcomes(&unhealthy)[1].0, Status::Ok);
    }

    #[test]
    fn suspended_objects_hold_newer_commits() {
        let mut held = progress("aaa1111", "aaa1111", "ccc3333");
        held.suspended = true;
        let statuses: Vec<Status> = outcomes(&held).into_iter().map(|(s, _)| s).collect();
        assert_eq!(statuses, [Status::Suspended, Status::Suspended, Status::Ok]);
    }

    #[test]
    fn revisions_outside_the_window_count_as_older() {
        let old = progress("0000000", "0000000", "ccc3333");
        let statuses: Vec<Status> = outcomes(&old).into_iter().map(|(s, _)| s).collect();
        assert_eq!(statuses, [Status::Progress; 3]);
        let nothing = progress("", "", "");
        assert_eq!(outcomes(&nothing)[2].0, Status::Offline);
    }

    #[test]
    fn repo_keys_need_a_branch_reference() {
        let source = parsed(&git_repository());
        assert_eq!(
            repo_key(&source),
            Some(RepoKey {
                url: "https://github.com/stefanprodan/podinfo".to_owned(),
                branch: "master".to_owned(),
            })
        );
        let mut tagged = git_repository();
        tagged["spec"]["ref"] = serde_json::json!({ "tag": "6.15.0" });
        assert_eq!(repo_key(&parsed(&tagged)), None);
        assert_eq!(
            wanted_revision(&source).as_deref(),
            Some("3e0ff8ae123b710bc91de1315cba0f996a8896c2")
        );
    }

    #[test]
    fn lanes_keep_whole_names_unless_an_environment_prefix_is_shared() {
        let headers = |names: &[&str]| -> Vec<String> {
            let clusters: Vec<ClusterId> = names.iter().map(|n| ClusterId::new(*n)).collect();
            lanes(&clusters)
                .into_iter()
                .map(|lane| lane.short)
                .collect()
        };
        assert_eq!(headers(&["fleet-a", "fleet-b"]), ["fleet-a", "fleet-b"]);
        assert_eq!(
            headers(&["prod-us-east", "prod-eu-west", "dev"]),
            ["us-east", "eu-west", "dev"]
        );
        let clusters = [ClusterId::new("prod-us-east")];
        assert_eq!(
            lanes(&clusters).first().map(|lane| lane.label.as_str()),
            Some("prod-us-east")
        );
    }

    #[test]
    fn log_maps_each_commit_onto_every_consuming_cluster() {
        let a = ClusterId::new("fleet-a");
        let b = ClusterId::new("fleet-b");
        let source = parsed(&git_repository());
        let applied = parsed(&kustomization(
            "apps",
            "True",
            "3e0ff8ae123b710bc91de1315cba0f996a8896c2",
        ));
        let behind = parsed(&kustomization("apps", "True", "1111111"));
        let now: Timestamp = "2026-10-13T10:00:00Z"
            .parse()
            .unwrap_or(Timestamp::UNIX_EPOCH);
        let history = vec![
            GitCommit {
                sha: "3e0ff8ae123b710bc91de1315cba0f996a8896c2".to_owned(),
                author: "Stefan".to_owned(),
                time: "2026-10-13T07:00:00Z"
                    .parse()
                    .unwrap_or(Timestamp::UNIX_EPOCH),
                subject: "Release v6.15.0".to_owned(),
                files: Vec::new(),
                truncated: false,
            },
            GitCommit {
                sha: "1111111aaaa".to_owned(),
                author: "Bot".to_owned(),
                time: "2026-10-12T07:00:00Z"
                    .parse()
                    .unwrap_or(Timestamp::UNIX_EPOCH),
                subject: "older".to_owned(),
                files: Vec::new(),
                truncated: false,
            },
        ];
        let repo = Repo {
            key: RepoKey {
                url: "https://github.com/stefanprodan/podinfo".to_owned(),
                branch: "master".to_owned(),
            },
            state: Some(MirrorState::Ready {
                commits: history,
                fetched: "2026-10-13T09:58:00Z"
                    .parse()
                    .unwrap_or(Timestamp::UNIX_EPOCH),
            }),
            consumers: vec![
                Consumer {
                    cluster: &a,
                    kustomization: &applied,
                    source: &source,
                },
                Consumer {
                    cluster: &b,
                    kustomization: &behind,
                    source: &source,
                },
            ],
        };
        let log = changes_log(&[repo], &[a.clone(), b.clone()], now);
        assert_eq!(log.source_label(), "podinfo/master");
        assert_eq!(log.fetched_label(), "podinfo fetched 2m ago");
        assert_eq!(log.commits.len(), 2);
        let head = &log.commits[0];
        assert_eq!(head.target_sentence(), "affects ks/apps on 2 clusters");
        let statuses: Vec<Status> = head.rollout.iter().map(|r| r.status).collect();
        assert_eq!(statuses, [Status::Ok, Status::Progress]);
        assert_eq!(
            log.commits[1].rollout_on(&b).map(|r| r.status),
            Some(Status::Ok)
        );
    }

    #[test]
    fn unreachable_sources_explain_themselves() {
        let repo = Repo {
            key: RepoKey {
                url: "git@github.com:acme/private.git".to_owned(),
                branch: "main".to_owned(),
            },
            state: Some(MirrorState::Unavailable(
                "Permission denied (publickey).".to_owned(),
            )),
            consumers: Vec::new(),
        };
        let log = changes_log(&[repo], &[], Timestamp::UNIX_EPOCH);
        assert_eq!(
            log.unavailable.as_deref(),
            Some("private: Permission denied (publickey).")
        );
        assert_eq!(log.commits, Vec::new());
        assert!(!log.pending);
    }

    #[test]
    fn history_is_pending_while_a_source_is_still_being_fetched() {
        let repo = |state| Repo {
            key: RepoKey {
                url: "https://github.com/stefanprodan/podinfo".to_owned(),
                branch: "master".to_owned(),
            },
            state,
            consumers: Vec::new(),
        };
        let fetching = changes_log(
            &[repo(Some(MirrorState::Fetching))],
            &[],
            Timestamp::UNIX_EPOCH,
        );
        assert!(fetching.pending);
        let unstarted = changes_log(&[repo(None)], &[], Timestamp::UNIX_EPOCH);
        assert!(unstarted.pending);
        assert!(!changes_log(&[], &[], Timestamp::UNIX_EPOCH).pending);
    }
}
