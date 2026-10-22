use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flock_core::changes::{FilePatch, LineChange, PatchLine};
use flock_flux::redact;
use jiff::Timestamp;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::watch;
use tokio::task::{Id, JoinSet};
use tokio::time::Instant;

use crate::manager::lock;
use crate::store::{APP_ID, CACHES};
use crate::task::TaskGuard;

const GIT_CACHE: &str = "git";
const FETCH_INTERVAL: Duration = Duration::from_mins(5);
const COMMIT_LIMIT: usize = 30;
const MAX_FILES: usize = 50;
const MAX_FILE_LINES: usize = 400;
const MAX_LINE_BYTES: usize = 1_000;
const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_STDERR_BYTES: usize = 8 * 1024;
const FETCH_LIMIT: usize = 4;
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
const RECORD: char = '\u{1e}';
const FIELD: char = '\u{1f}';
const LOG_FORMAT: &str = "--format=%x1e%H%x1f%an%x1f%at%x1f%s";
const GIT: &str = "git";
const NO_HOME: &str = "no home directory for the git cache";
const STDERR_FALLBACK: &str = "git exited unsuccessfully";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RepoKey {
    pub url: String,
    pub branch: String,
}

impl RepoKey {
    pub fn name(&self) -> String {
        repo_name(&self.url)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitCommit {
    pub sha: String,
    pub author: String,
    pub time: Timestamp,
    pub subject: String,
    pub files: Vec<FilePatch>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MirrorState {
    Fetching,
    Ready {
        commits: Vec<GitCommit>,
        fetched: Timestamp,
    },
    Unavailable(String),
}

type Wanted = BTreeMap<RepoKey, BTreeSet<String>>;
type States = Arc<Mutex<BTreeMap<RepoKey, MirrorState>>>;

pub struct Mirrors {
    states: States,
    wanted: watch::Sender<Wanted>,
    _task: TaskGuard,
}

impl Mirrors {
    pub fn start(timeout: Duration, changed: async_channel::Sender<()>) -> Self {
        let states = Arc::new(Mutex::new(BTreeMap::new()));
        let (wanted, receiver) = watch::channel(Wanted::new());
        let cache = std::env::home_dir().map(|home| home.join(CACHES).join(APP_ID).join(GIT_CACHE));
        let fetch = move |key: RepoKey| {
            let cache = cache.clone();
            async move {
                match cache {
                    Some(cache) => sync(&cache, &key, timeout).await,
                    None => MirrorState::Unavailable(NO_HOME.to_owned()),
                }
            }
        };
        let task = TaskGuard::spawn(mirror(fetch, receiver, states.clone(), changed));
        Self {
            states,
            wanted,
            _task: task,
        }
    }

    pub fn want(&self, wanted: Wanted) {
        self.wanted.send_if_modified(|current| {
            let differs = *current != wanted;
            if differs {
                *current = wanted;
            }
            differs
        });
    }

    pub fn states(&self) -> BTreeMap<RepoKey, MirrorState> {
        lock(&self.states).clone()
    }
}

struct Attempt {
    at: Instant,
    revisions: BTreeSet<String>,
}

type Finished = (RepoKey, BTreeSet<String>, MirrorState);

async fn mirror<F, Fut>(
    fetch: F,
    mut wanted: watch::Receiver<Wanted>,
    states: States,
    changed: async_channel::Sender<()>,
) where
    F: Fn(RepoKey) -> Fut + Send,
    Fut: Future<Output = MirrorState> + Send + 'static,
{
    let mut attempts: BTreeMap<RepoKey, Attempt> = BTreeMap::new();
    let mut running: JoinSet<Finished> = JoinSet::new();
    let mut in_flight: HashMap<Id, RepoKey> = HashMap::new();
    loop {
        let current = wanted.borrow_and_update().clone();
        lock(&states).retain(|key, _| current.contains_key(key));
        attempts.retain(|key, _| current.contains_key(key));
        for (key, revisions) in &current {
            if in_flight.len() >= FETCH_LIMIT {
                break;
            }
            if in_flight.values().any(|running| running == key) {
                continue;
            }
            let state = lock(&states).get(key).cloned();
            let attempt = attempts.get(key);
            let expired = attempt.is_none_or(|attempt| attempt.at.elapsed() >= FETCH_INTERVAL);
            let attempted = attempt.map(|attempt| &attempt.revisions);
            if !needs_fetch(state.as_ref(), revisions, attempted, expired) {
                continue;
            }
            if state.is_none() {
                lock(&states).insert(key.clone(), MirrorState::Fetching);
                notify(&changed);
            }
            let job = fetch(key.clone());
            let finished = (key.clone(), revisions.clone());
            let handle = running.spawn(async move {
                let (key, revisions) = finished;
                (key, revisions, job.await)
            });
            in_flight.insert(handle.id(), key.clone());
        }
        tokio::select! {
            result = wanted.changed() => {
                if result.is_err() {
                    return;
                }
            }
            Some(joined) = running.join_next_with_id() => {
                match joined {
                    Ok((id, (key, revisions, next))) => {
                        in_flight.remove(&id);
                        if wanted.borrow().contains_key(&key) {
                            attempts.insert(key.clone(), Attempt { at: Instant::now(), revisions });
                            lock(&states).insert(key, next);
                            notify(&changed);
                        }
                    }
                    Err(error) => {
                        if let Some(key) = in_flight.remove(&error.id()) {
                            tracing::warn!(url = %redact::credentials(&key.url), %error, "git mirror fetch stopped");
                        }
                    }
                }
            }
            () = tokio::time::sleep(FETCH_INTERVAL) => {}
        }
    }
}

fn notify(changed: &async_channel::Sender<()>) {
    if changed.try_send(()).is_err() {
        tracing::trace!("GitOps change already pending");
    }
}

pub fn needs_fetch(
    current: Option<&MirrorState>,
    revisions: &BTreeSet<String>,
    attempted: Option<&BTreeSet<String>>,
    expired: bool,
) -> bool {
    match current {
        None => true,
        Some(MirrorState::Fetching) => true,
        Some(MirrorState::Unavailable(_)) => expired,
        Some(MirrorState::Ready { commits, .. }) => {
            let known = |sha: &String| commits.iter().any(|commit| &commit.sha == sha);
            let tried = |sha: &String| attempted.is_some_and(|tried| tried.contains(sha));
            expired || revisions.iter().any(|sha| !known(sha) && !tried(sha))
        }
    }
}

async fn sync(cache: &Path, key: &RepoKey, timeout: Duration) -> MirrorState {
    let dir = mirror_dir(cache, &key.url);
    match fetch_log(&dir, key, timeout).await {
        Ok(commits) => MirrorState::Ready {
            commits,
            fetched: Timestamp::now(),
        },
        Err(reason) => {
            tracing::info!(url = %redact::credentials(&key.url), %reason, "git mirror unavailable");
            MirrorState::Unavailable(reason)
        }
    }
}

async fn fetch_log(dir: &Path, key: &RepoKey, timeout: Duration) -> Result<Vec<GitCommit>, String> {
    if !dir.exists() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|error| error.to_string())?;
        git(dir, &["init", "--bare", "--quiet"], timeout).await?;
    }
    let refspec = format!("+refs/heads/{0}:refs/heads/{0}", key.branch);
    git(
        dir,
        &["fetch", "--quiet", "--no-tags", &key.url, &refspec],
        timeout,
    )
    .await?;
    let reference = format!("refs/heads/{}", key.branch);
    let limit = format!("-n{COMMIT_LIMIT}");
    let log = git(
        dir,
        &[
            "log",
            "-p",
            "--first-parent",
            "--diff-merges=first-parent",
            "--no-color",
            "--no-ext-diff",
            LOG_FORMAT,
            &limit,
            &reference,
            "--",
        ],
        timeout,
    )
    .await?;
    if log.truncated {
        tracing::info!(url = %redact::credentials(&key.url), limit = MAX_OUTPUT_BYTES, "git log output truncated");
    }
    Ok(parse_log(&log.text))
}

struct Captured {
    text: String,
    truncated: bool,
}

async fn read_capped<R: AsyncRead + Unpin>(reader: R, cap: usize) -> (Vec<u8>, bool, R) {
    let limit = u64::try_from(cap).unwrap_or(u64::MAX).saturating_add(1);
    let mut limited = reader.take(limit);
    let mut bytes = Vec::new();
    if let Err(error) = limited.read_to_end(&mut bytes).await {
        tracing::debug!(%error, "git output unreadable");
    }
    let truncated = bytes.len() > cap;
    bytes.truncate(cap);
    (bytes, truncated, limited.into_inner())
}

async fn read_stderr<R: AsyncRead + Unpin>(reader: R) -> Vec<u8> {
    let (bytes, truncated, mut rest) = read_capped(reader, MAX_STDERR_BYTES).await;
    if truncated && let Err(error) = tokio::io::copy(&mut rest, &mut tokio::io::sink()).await {
        tracing::debug!(%error, "git stderr unreadable");
    }
    bytes
}

async fn capture(mut child: Child, timeout: Duration, what: &str) -> Result<Captured, String> {
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Err(format!("git {what} has no output pipes"));
    };
    let run = async {
        let out = async {
            let (bytes, truncated, _) = read_capped(stdout, MAX_OUTPUT_BYTES).await;
            if truncated && let Err(error) = child.start_kill() {
                tracing::debug!(%error, "git could not be stopped");
            }
            (bytes, truncated)
        };
        let ((stdout, truncated), stderr) = tokio::join!(out, read_stderr(stderr));
        let status = child.wait().await;
        (stdout, truncated, stderr, status)
    };
    let (stdout, truncated, stderr, status) = tokio::time::timeout(timeout, run)
        .await
        .map_err(|_| format!("git {what} timed out after {}s", timeout.as_secs()))?;
    let status = status.map_err(|error| format!("git could not run: {error}"))?;
    if status.success() || truncated {
        return Ok(Captured {
            text: String::from_utf8_lossy(&stdout).into_owned(),
            truncated,
        });
    }
    Err(redact::credentials(&first_line(&String::from_utf8_lossy(
        &stderr,
    ))))
}

async fn git(dir: &Path, args: &[&str], timeout: Duration) -> Result<Captured, String> {
    let mut command = Command::new(GIT);
    command
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        command.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    let child = command
        .spawn()
        .map_err(|error| format!("git could not run: {error}"))?;
    capture(child, timeout, args.first().copied().unwrap_or_default()).await
}

pub fn first_line(stderr: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map_or_else(|| STDERR_FALLBACK.to_owned(), str::to_owned)
}

pub fn repo_name(url: &str) -> String {
    let clean = redact::credentials(url);
    let path = clean.split(['?', '#']).next().unwrap_or_default();
    let trimmed = path.trim_end_matches('/');
    let last = trimmed.rsplit(['/', ':']).next().unwrap_or(trimmed);
    last.strip_suffix(".git").unwrap_or(last).to_owned()
}

fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(FNV_OFFSET, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
    })
}

pub fn mirror_dir(cache: &Path, url: &str) -> PathBuf {
    let safe: String = repo_name(url)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    cache.join(format!("{safe}-{:016x}.git", fnv1a(url)))
}

pub fn parse_log(text: &str) -> Vec<GitCommit> {
    let records: Vec<&str> = text
        .split(RECORD)
        .filter(|record| !record.trim().is_empty())
        .collect();
    records
        .into_iter()
        .filter_map(|record| {
            let (header, patch) = record.split_once('\n').unwrap_or((record, ""));
            let mut fields = header.split(FIELD);
            let sha = fields.next()?.trim().to_owned();
            let author = fields.next()?.to_owned();
            let seconds: i64 = fields.next()?.parse().ok()?;
            let subject = fields.next().unwrap_or_default().to_owned();
            let time = Timestamp::from_second(seconds).ok()?;
            let valid = !sha.is_empty() && sha.chars().all(|c| c.is_ascii_hexdigit());
            let files = parse_patch(patch);
            valid.then_some(GitCommit {
                sha,
                author,
                time,
                subject,
                files,
            })
        })
        .collect()
}

struct Cursor {
    old: u32,
    new: u32,
}

fn hunk_start(line: &str) -> Option<Cursor> {
    let body = line.strip_prefix("@@ -")?;
    let (old, rest) = body.split_once(" +")?;
    let new = rest.split_once(" @@").map_or(rest, |(new, _)| new);
    let start = |range: &str| range.split(',').next()?.parse::<u32>().ok();
    Some(Cursor {
        old: start(old)?,
        new: start(new)?,
    })
}

fn clip(text: &str) -> String {
    if text.len() <= MAX_LINE_BYTES {
        return text.to_owned();
    }
    let end = text.floor_char_boundary(MAX_LINE_BYTES);
    text.get(..end).unwrap_or_default().to_owned()
}

pub fn parse_patch(text: &str) -> Vec<FilePatch> {
    let mut files: Vec<FilePatch> = Vec::new();
    let mut cursor: Option<Cursor> = None;
    let mut skipping = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            cursor = None;
            if files.len() >= MAX_FILES {
                skipping = true;
                continue;
            }
            let path = rest
                .split_once(" b/")
                .map_or(rest, |(_, new)| new)
                .to_owned();
            files.push(FilePatch {
                path,
                lines: Vec::new(),
            });
            continue;
        }
        if skipping {
            continue;
        }
        let (change, text, number) = if let Some(start) = hunk_start(line) {
            let number = start.new;
            cursor = Some(start);
            (LineChange::Hunk, line, number)
        } else if let Some(at) = cursor.as_mut()
            && let Some(text) = line.strip_prefix('+')
        {
            at.new += 1;
            (LineChange::Added, text, at.new - 1)
        } else if let Some(at) = cursor.as_mut()
            && let Some(text) = line.strip_prefix('-')
        {
            at.old += 1;
            (LineChange::Removed, text, at.old - 1)
        } else if let Some(at) = cursor.as_mut()
            && let Some(text) = line.strip_prefix(' ')
        {
            at.old += 1;
            at.new += 1;
            (LineChange::Context, text, at.new - 1)
        } else {
            continue;
        };
        let Some(file) = files.last_mut() else {
            continue;
        };
        if file.lines.len() >= MAX_FILE_LINES {
            continue;
        }
        let text = clip(text);
        file.lines.push(PatchLine {
            change,
            number,
            text,
            note: None,
        });
    }
    files
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tracing_subscriber::fmt::MakeWriter;

    use super::*;

    const PLANTED_USER: &str = "planted-user-token";
    const PLANTED_QUERY: &str = "planted-query-token";
    const FETCH_TIMEOUT: Duration = Duration::from_secs(20);

    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            lock(&self.0).extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Captured {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[tokio::test]
    async fn failed_fetches_never_log_or_report_url_credentials() {
        let Ok(cache) = tempfile::tempdir() else {
            panic!("tempdir");
        };
        let key = RepoKey {
            url: format!(
                "https://x-access-token:{PLANTED_USER}@127.0.0.1:9/acme/repo.git?access_token={PLANTED_QUERY}"
            ),
            branch: "main".to_owned(),
        };
        let captured = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(captured.clone())
            .with_ansi(false)
            .finish();
        let state = {
            let _guard = tracing::subscriber::set_default(subscriber);
            sync(cache.path(), &key, FETCH_TIMEOUT).await
        };
        let MirrorState::Unavailable(reason) = state else {
            panic!("fetch from a closed port succeeded");
        };
        let logged = String::from_utf8_lossy(&lock(&captured.0)).into_owned();
        assert!(logged.contains("git mirror unavailable"), "{logged}");
        for output in [&logged, &reason, &key.name()] {
            assert!(!output.contains(PLANTED_USER), "{output}");
            assert!(!output.contains(PLANTED_QUERY), "{output}");
        }
    }

    const LOG: &str = "\u{1e}3e0ff8ae123b\u{1f}Stefan Prodan\u{1f}1791870000\u{1f}Release v6.15.0\n\ndiff --git a/kustomize/deployment.yaml b/kustomize/deployment.yaml\nindex 1..2 100644\n--- a/kustomize/deployment.yaml\n+++ b/kustomize/deployment.yaml\n@@ -21,3 +21,4 @@ spec:\n       containers:\n-        image: podinfo:6.14.0\n+        image: podinfo:6.15.0\n+        args: []\n       ports:\n\u{1e}aaaa111\u{1f}Bot\u{1f}1791860000\u{1f}docs: typo\n\ndiff --git a/README.md b/README.md\n--- a/README.md\n+++ b/README.md\n@@ -1 +1 @@\n-colour\n+color\n";

    #[test]
    fn log_parses_commits_and_numbered_patch_lines() {
        let commits = parse_log(LOG);
        assert_eq!(commits.len(), 2);
        let release = &commits[0];
        assert_eq!(release.sha, "3e0ff8ae123b");
        assert_eq!(release.author, "Stefan Prodan");
        assert_eq!(release.subject, "Release v6.15.0");
        assert_eq!(release.time.as_second(), 1_791_870_000);
        let file = &release.files[0];
        assert_eq!(file.path, "kustomize/deployment.yaml");
        let lines: Vec<(LineChange, u32, &str)> = file
            .lines
            .iter()
            .map(|l| (l.change, l.number, l.text.as_str()))
            .collect();
        assert_eq!(
            lines,
            [
                (LineChange::Hunk, 21, "@@ -21,3 +21,4 @@ spec:"),
                (LineChange::Context, 21, "      containers:"),
                (LineChange::Removed, 22, "        image: podinfo:6.14.0"),
                (LineChange::Added, 22, "        image: podinfo:6.15.0"),
                (LineChange::Added, 23, "        args: []"),
                (LineChange::Context, 24, "      ports:"),
            ]
        );
        assert_eq!(commits[1].files[0].added(), 1);
        assert_eq!(commits[1].files[0].removed(), 1);
    }

    #[test]
    fn every_hunk_keeps_its_header_so_skipped_lines_show() {
        let patch = "diff --git a/w.yml b/w.yml\n@@ -26,3 +26,3 @@ jobs:\n       a\n-      b\n+      c\n@@ -48,2 +48,3 @@ jobs:\n       d\n+      e\n";
        let files = parse_patch(patch);
        let lines: Vec<(LineChange, u32, &str)> = files
            .iter()
            .flat_map(|file| &file.lines)
            .map(|l| (l.change, l.number, l.text.as_str()))
            .collect();
        assert_eq!(
            lines,
            [
                (LineChange::Hunk, 26, "@@ -26,3 +26,3 @@ jobs:"),
                (LineChange::Context, 26, "      a"),
                (LineChange::Removed, 27, "      b"),
                (LineChange::Added, 27, "      c"),
                (LineChange::Hunk, 48, "@@ -48,2 +48,3 @@ jobs:"),
                (LineChange::Context, 48, "      d"),
                (LineChange::Added, 49, "      e"),
            ]
        );
        assert_eq!(files.first().map(FilePatch::added), Some(2));
        assert_eq!(files.first().map(FilePatch::removed), Some(1));
    }

    #[test]
    fn mirror_dirs_are_stable_safe_and_distinct() {
        let cache = Path::new("/c");
        let https = mirror_dir(cache, "https://github.com/stefanprodan/podinfo");
        let ssh = mirror_dir(cache, "ssh://git@github.com/stefanprodan/podinfo.git");
        assert_eq!(
            https,
            mirror_dir(cache, "https://github.com/stefanprodan/podinfo")
        );
        assert_ne!(https, ssh);
        let name = https
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        assert!(name.starts_with("podinfo-"), "{name}");
        assert_eq!(https.extension().and_then(|e| e.to_str()), Some("git"));
        assert_eq!(https.parent(), Some(cache));
        let odd = mirror_dir(cache, "git@host:team/../we ird.git");
        let odd_name = odd.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        assert!(odd_name.starts_with("we_ird-"), "{odd_name}");
    }

    #[test]
    fn repo_names_drop_suffixes() {
        assert_eq!(
            repo_name("https://github.com/acme/fleet-infra.git/"),
            "fleet-infra"
        );
        assert_eq!(
            repo_name("git@github.com:acme/fleet-infra.git"),
            "fleet-infra"
        );
    }

    #[test]
    fn fetch_is_needed_for_new_stale_or_unknown_revisions() {
        let commit = GitCommit {
            sha: "abc".to_owned(),
            author: String::new(),
            time: Timestamp::UNIX_EPOCH,
            subject: String::new(),
            files: Vec::new(),
        };
        let ready = MirrorState::Ready {
            commits: vec![commit],
            fetched: Timestamp::UNIX_EPOCH,
        };
        let known = BTreeSet::from(["abc".to_owned()]);
        let unknown = BTreeSet::from(["def".to_owned()]);
        assert!(needs_fetch(None, &known, None, false));
        assert!(!needs_fetch(Some(&ready), &known, Some(&known), false));
        assert!(needs_fetch(Some(&ready), &unknown, Some(&known), false));
        assert!(needs_fetch(Some(&ready), &known, Some(&known), true));
        let failed = MirrorState::Unavailable("auth".to_owned());
        assert!(!needs_fetch(Some(&failed), &unknown, Some(&known), false));
        assert!(needs_fetch(Some(&failed), &unknown, Some(&known), true));
    }

    #[test]
    fn an_unresolved_revision_waits_for_the_interval() {
        let ready = MirrorState::Ready {
            commits: Vec::new(),
            fetched: Timestamp::UNIX_EPOCH,
        };
        let pinned = BTreeSet::from(["old".to_owned()]);
        let advanced = BTreeSet::from(["old".to_owned(), "new".to_owned()]);
        assert!(!needs_fetch(Some(&ready), &pinned, Some(&pinned), false));
        assert!(needs_fetch(Some(&ready), &advanced, Some(&pinned), false));
        assert!(needs_fetch(Some(&ready), &pinned, Some(&pinned), true));
    }

    fn key(url: &str) -> RepoKey {
        RepoKey {
            url: url.to_owned(),
            branch: "main".to_owned(),
        }
    }

    fn ready() -> MirrorState {
        MirrorState::Ready {
            commits: Vec::new(),
            fetched: Timestamp::UNIX_EPOCH,
        }
    }

    struct Fake {
        states: States,
        wanted: watch::Sender<Wanted>,
        calls: Arc<Mutex<Vec<RepoKey>>>,
        peak: Arc<std::sync::atomic::AtomicUsize>,
        _task: TaskGuard,
    }

    fn fake(delay: fn(&RepoKey) -> Duration) -> Fake {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let states: States = Arc::new(Mutex::new(BTreeMap::new()));
        let (wanted, receiver) = watch::channel(Wanted::new());
        let (changed, _) = async_channel::unbounded();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let fetch = {
            let calls = calls.clone();
            let peak = peak.clone();
            move |key: RepoKey| {
                lock(&calls).push(key.clone());
                let active = active.clone();
                let peak = peak.clone();
                let wait = delay(&key);
                async move {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(wait).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    ready()
                }
            }
        };
        let task = TaskGuard::spawn(mirror(fetch, receiver, states.clone(), changed));
        Fake {
            states,
            wanted,
            calls,
            peak,
            _task: task,
        }
    }

    const SLOW: &str = "https://example.test/slow";
    const FAST: &str = "https://example.test/fast";
    const FAST_FETCH: Duration = Duration::from_secs(1);
    const SLOW_FETCH: Duration = Duration::from_secs(100);

    fn by_name(key: &RepoKey) -> Duration {
        if key.url == SLOW {
            SLOW_FETCH
        } else {
            FAST_FETCH
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_repo_does_not_hold_back_a_fast_one() {
        let mirrors = fake(by_name);
        let wanted = Wanted::from([(key(SLOW), BTreeSet::new()), (key(FAST), BTreeSet::new())]);
        mirrors.wanted.send_replace(wanted);
        tokio::time::sleep(FAST_FETCH * 2).await;
        let states = lock(&mirrors.states).clone();
        assert_eq!(states.get(&key(FAST)), Some(&ready()));
        assert_eq!(states.get(&key(SLOW)), Some(&MirrorState::Fetching));
        tokio::time::sleep(SLOW_FETCH).await;
        assert_eq!(lock(&mirrors.states).get(&key(SLOW)), Some(&ready()));
    }

    #[tokio::test(start_paused = true)]
    async fn fetches_run_together_up_to_the_limit() {
        let mirrors = fake(|_| FAST_FETCH);
        let repos = FETCH_LIMIT + 2;
        let wanted: Wanted = (0..repos)
            .map(|ix| (key(&format!("https://example.test/{ix}")), BTreeSet::new()))
            .collect();
        mirrors.wanted.send_replace(wanted);
        tokio::time::sleep(FAST_FETCH * 3).await;
        let states = lock(&mirrors.states).clone();
        assert_eq!(states.len(), repos);
        assert!(states.values().all(|state| *state == ready()));
        assert_eq!(
            mirrors.peak.load(std::sync::atomic::Ordering::SeqCst),
            FETCH_LIMIT
        );
    }

    #[tokio::test(start_paused = true)]
    async fn other_repos_changing_do_not_refetch_an_unresolved_revision() {
        let mirrors = fake(|_| FAST_FETCH);
        let pinned = (key(SLOW), BTreeSet::from(["old".to_owned()]));
        for revision in ["a", "b", "c"] {
            let advancing = (key(FAST), BTreeSet::from([revision.to_owned()]));
            mirrors
                .wanted
                .send_replace(Wanted::from([pinned.clone(), advancing]));
            tokio::time::sleep(FAST_FETCH * 2).await;
        }
        let calls = lock(&mirrors.calls).clone();
        let pinned_fetches = calls.iter().filter(|called| **called == key(SLOW)).count();
        let advancing_fetches = calls.iter().filter(|called| **called == key(FAST)).count();
        assert_eq!(pinned_fetches, 1);
        assert_eq!(advancing_fetches, 3);
    }

    #[test]
    fn patches_stop_at_file_line_and_byte_budgets() {
        let many = (0..=MAX_FILES)
            .map(|ix| format!("diff --git a/f{ix} b/f{ix}\n@@ -1 +1 @@\n+x\n"))
            .collect::<Vec<_>>()
            .concat();
        let files = parse_patch(&many);
        assert_eq!(files.len(), MAX_FILES);

        let mut long = String::from("diff --git a/big b/big\n@@ -1 +1 @@\n");
        for _ in 0..=MAX_FILE_LINES {
            long.push_str("+line\n");
        }
        let files = parse_patch(&long);
        assert_eq!(files.first().map(|f| f.lines.len()), Some(MAX_FILE_LINES));

        let wide = format!(
            "diff --git a/w b/w\n@@ -1 +1 @@\n+{}\n",
            "é".repeat(MAX_LINE_BYTES)
        );
        let files = parse_patch(&wide);
        let text = files
            .first()
            .and_then(|f| f.lines.first())
            .map(|l| l.text.clone())
            .unwrap_or_default();
        assert!(text.len() <= MAX_LINE_BYTES && !text.is_empty());
    }

    #[tokio::test]
    async fn endless_output_is_cut_at_the_budget() {
        let Ok(child) = Command::new("yes")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
        else {
            panic!("spawn yes");
        };
        let captured = capture(child, Duration::from_secs(30), "yes").await;
        let Ok(captured) = captured else {
            panic!("capture failed");
        };
        assert!(captured.truncated);
        assert_eq!(captured.text.len(), MAX_OUTPUT_BYTES);
    }

    #[tokio::test]
    async fn noisy_stderr_is_bounded() {
        let Ok(child) = Command::new("sh")
            .args(["-c", "head -c 1000000 /dev/zero | tr '\\0' x >&2; exit 1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
        else {
            panic!("spawn sh");
        };
        let captured = capture(child, Duration::from_secs(30), "sh").await;
        let Err(reason) = captured else {
            panic!("failure expected");
        };
        assert_eq!(reason.len(), MAX_STDERR_BYTES);
    }

    #[test]
    fn stderr_reduces_to_its_first_line() {
        assert_eq!(
            first_line("\nfatal: could not read Username\nmore"),
            "fatal: could not read Username"
        );
        assert_eq!(first_line(""), STDERR_FALLBACK);
    }

    #[test]
    fn repo_names_ignore_credentials_and_queries() {
        assert_eq!(
            repo_name("https://user:secret@host/acme/repo.git?token=secret"),
            "repo"
        );
        assert_eq!(repo_name("https://user:secret@host"), "host");
    }
}
