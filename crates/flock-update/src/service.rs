use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::SystemTime;

use flock_core::{Receiver, Sender, Trigger, UpdaterRequest, UpdaterState, UpdaterStatus};
use semver::Version;

use crate::install::{self, Target};
use crate::release::{Release, newest};
use crate::signature;
use crate::source::Source;
use crate::stage::{Expected, Layout};
use crate::{Error, Result};

const THREAD: &str = "flock-update";

pub type Relaunch = Box<dyn Fn(&Path) -> Result<()> + Send>;

#[derive(Clone, Debug)]
pub struct Config {
    pub current: Version,
    pub identifier: String,
    pub bundle_name: String,
    pub target: Target,
    pub public_key: String,
    pub max_download: u64,
    pub max_text: u64,
}

struct Staged {
    version: Version,
    path: PathBuf,
}

pub struct Updater<S> {
    source: S,
    config: Config,
    relaunch: Relaunch,
    status: UpdaterStatus,
    staged: Option<Staged>,
}

impl<S: Source> Updater<S> {
    pub fn new(source: S, config: Config, relaunch: Relaunch) -> Self {
        if let Target::Bundle(bundle) = &config.target {
            let cleaned = Layout::beside(bundle).and_then(|layout| layout.clean());
            if let Err(error) = cleaned {
                tracing::warn!(%error, "removing a previous update");
            }
        }
        Self {
            source,
            config,
            relaunch,
            status: UpdaterStatus::default(),
            staged: None,
        }
    }

    pub fn status(&self) -> &UpdaterStatus {
        &self.status
    }

    pub fn handle(&mut self, request: &UpdaterRequest, emit: &mut dyn FnMut(&UpdaterStatus)) {
        match *request {
            UpdaterRequest::Check {
                trigger,
                prereleases,
            } => self.check(trigger, prereleases, emit),
            UpdaterRequest::Install => self.install(emit),
        }
    }

    fn set(&mut self, state: UpdaterState, emit: &mut dyn FnMut(&UpdaterStatus)) {
        self.status.state = state;
        emit(&self.status);
    }

    fn fail(&mut self, error: &Error, emit: &mut dyn FnMut(&UpdaterStatus)) {
        tracing::warn!(%error, "update");
        let reason = error.to_string();
        self.set(UpdaterState::Failed { reason }, emit);
    }

    fn check(&mut self, trigger: Trigger, prereleases: bool, emit: &mut dyn FnMut(&UpdaterStatus)) {
        self.status.trigger = trigger;
        if self.staged.is_some() {
            emit(&self.status);
            return;
        }
        self.set(UpdaterState::Checking, emit);
        let found = self
            .source
            .releases(prereleases)
            .map(|releases| newest(releases, &self.config.current, prereleases));
        match found {
            Err(error) => self.fail(&error, emit),
            Ok(None) => {
                self.status.checked = Some(SystemTime::now());
                self.set(UpdaterState::UpToDate, emit);
            }
            Ok(Some(release)) => {
                self.status.checked = Some(SystemTime::now());
                self.offer(&release, emit);
            }
        }
    }

    fn offer(&mut self, release: &Release, emit: &mut dyn FnMut(&UpdaterStatus)) {
        let version = release.version.to_string();
        let Target::Bundle(bundle) = self.config.target.clone() else {
            let page = release.page.clone();
            self.set(UpdaterState::Available { version, page }, emit);
            return;
        };
        let downloading = UpdaterState::Downloading {
            version: version.clone(),
        };
        self.set(downloading, emit);
        match self.download(release, &bundle) {
            Ok(path) => {
                self.staged = Some(Staged {
                    version: release.version.clone(),
                    path,
                });
                self.set(UpdaterState::Ready { version }, emit);
            }
            Err(error) => self.fail(&error, emit),
        }
    }

    fn text(&self, url: &str) -> Result<String> {
        let bytes = self.source.fetch(url, self.config.max_text)?;
        String::from_utf8(bytes).map_err(|_| Error::SignatureFormat)
    }

    fn download(&self, release: &Release, bundle: &Path) -> Result<PathBuf> {
        let artifacts = release.artifacts()?;
        if artifacts.zip.size > self.config.max_download {
            return Err(Error::TooLarge(self.config.max_download));
        }
        let signed = self.text(&artifacts.signature.url)?;
        let checksum = self.text(&artifacts.checksum.url)?;
        let zip = self
            .source
            .fetch(&artifacts.zip.url, self.config.max_download)?;
        signature::verify(&zip, &signed, &self.config.public_key)?;
        signature::verify_checksum(&zip, &checksum)?;
        let expected = Expected {
            bundle_name: &self.config.bundle_name,
            identifier: &self.config.identifier,
            version: &release.version,
            current: &self.config.current,
        };
        Layout::beside(bundle)?.stage(&zip, expected)
    }

    fn install(&mut self, emit: &mut dyn FnMut(&UpdaterStatus)) {
        let Target::Bundle(bundle) = self.config.target.clone() else {
            return;
        };
        let Some(staged) = self.staged.take() else {
            return;
        };
        self.status.trigger = Trigger::Manual;
        let version = staged.version.to_string();
        self.set(UpdaterState::Installing { version }, emit);
        let installed = Layout::beside(&bundle)
            .and_then(|layout| install::swap(&layout, &staged.path))
            .and_then(|()| (self.relaunch)(&bundle));
        match installed {
            Ok(()) => self.set(UpdaterState::Relaunching, emit),
            Err(error) => self.fail(&error, emit),
        }
    }
}

pub fn spawn<S: Source>(
    mut updater: Updater<S>,
    requests: Receiver<UpdaterRequest>,
    statuses: Sender<UpdaterStatus>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name(THREAD.to_owned())
        .spawn(move || {
            while let Ok(request) = requests.recv_blocking() {
                let mut emit = |status: &UpdaterStatus| {
                    let _ = statuses.send_blocking(status.clone());
                };
                updater.handle(&request, &mut emit);
            }
        })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::release::{Asset, Variant, zip_name};
    use crate::signature::{Keypair, checksum_line, generate, sign};
    use crate::testing::{BUNDLE, ID, bundle_version, fake_bundle, temp, zip};

    const MAX_DOWNLOAD: u64 = 1 << 26;
    const MAX_TEXT: u64 = 1 << 12;
    const PAGE: &str = "https://github.com/team-loco/flock-app/releases/tag/v0.2.0";

    struct Fake {
        releases: Option<Vec<Release>>,
        files: BTreeMap<String, Vec<u8>>,
    }

    impl Source for Fake {
        fn releases(&self, _: bool) -> Result<Vec<Release>> {
            self.releases
                .clone()
                .ok_or_else(|| Error::Network("offline".into()))
        }

        fn fetch(&self, url: &str, limit: u64) -> Result<Vec<u8>> {
            let bytes = self.files.get(url).cloned().ok_or(Error::Http(404))?;
            if bytes.len() as u64 > limit {
                return Err(Error::TooLarge(limit));
            }
            Ok(bytes)
        }
    }

    fn keys() -> Keypair {
        generate().unwrap_or_else(|error| panic!("keygen: {error}"))
    }

    fn asset(name: &str, size: usize) -> Asset {
        Asset {
            name: name.to_owned(),
            size: size as u64,
            url: format!("https://example.test/{name}"),
        }
    }

    fn update_zip() -> String {
        zip_name(&Version::new(0, 2, 0), Variant::CURRENT)
    }

    fn release_of(
        zip: &[u8],
        signature: &str,
        checksum: &str,
    ) -> (Release, BTreeMap<String, Vec<u8>>) {
        let name = update_zip();
        let files = [
            (name.clone(), zip.to_vec()),
            (format!("{name}.sig"), signature.as_bytes().to_vec()),
            (format!("{name}.sha256"), checksum.as_bytes().to_vec()),
        ];
        let assets = files
            .iter()
            .map(|(name, bytes)| asset(name, bytes.len()))
            .collect();
        let release = Release {
            version: Version::new(0, 2, 0),
            page: PAGE.to_owned(),
            draft: false,
            prerelease: false,
            assets,
        };
        let files = files
            .into_iter()
            .map(|(name, bytes)| (format!("https://example.test/{name}"), bytes))
            .collect();
        (release, files)
    }

    fn config(target: Target, public_key: &str) -> Config {
        Config {
            current: Version::new(0, 1, 0),
            identifier: ID.to_owned(),
            bundle_name: BUNDLE.to_owned(),
            target,
            public_key: public_key.to_owned(),
            max_download: MAX_DOWNLOAD,
            max_text: MAX_TEXT,
        }
    }

    struct Run {
        states: Vec<UpdaterState>,
        launched: Arc<Mutex<Vec<PathBuf>>>,
    }

    fn run(source: Fake, config: Config, requests: &[UpdaterRequest]) -> Run {
        let launched = Arc::new(Mutex::new(Vec::new()));
        let record = launched.clone();
        let relaunch: Relaunch = Box::new(move |bundle| {
            if let Ok(mut launched) = record.lock() {
                launched.push(bundle.to_path_buf());
            }
            Ok(())
        });
        let mut updater = Updater::new(source, config, relaunch);
        let mut states = Vec::new();
        for request in requests {
            updater.handle(request, &mut |status| states.push(status.state.clone()));
        }
        Run { states, launched }
    }

    fn manual_check() -> UpdaterRequest {
        UpdaterRequest::Check {
            trigger: Trigger::Manual,
            prereleases: false,
        }
    }

    fn signed_update(dir: &Path, keys: &Keypair) -> Fake {
        let update = fake_bundle(&dir.join("build"), ID, "0.2.0");
        let bytes = zip(&update);
        let signature = sign(&bytes, &keys.private).unwrap_or_default();
        let checksum = checksum_line(&bytes, &update_zip());
        let (release, files) = release_of(&bytes, &signature, &checksum);
        Fake {
            releases: Some(vec![release]),
            files,
        }
    }

    #[test]
    fn a_newer_signed_release_downloads_stages_and_installs_on_restart() {
        let dir = temp();
        let keys = keys();
        let running = fake_bundle(&dir.path().join("Applications"), ID, "0.1.0");
        let source = signed_update(dir.path(), &keys);
        let config = config(Target::Bundle(running.clone()), &keys.public);
        let run = run(source, config, &[manual_check(), UpdaterRequest::Install]);
        let version = "0.2.0".to_owned();
        assert_eq!(
            run.states,
            [
                UpdaterState::Checking,
                UpdaterState::Downloading {
                    version: version.clone()
                },
                UpdaterState::Ready {
                    version: version.clone()
                },
                UpdaterState::Installing { version },
                UpdaterState::Relaunching,
            ]
        );
        assert_eq!(bundle_version(&running), "0.2.0");
        let launched = run.launched.lock().map(|l| l.clone()).unwrap_or_default();
        assert_eq!(launched, [running]);
    }

    #[test]
    fn a_tampered_zip_never_touches_the_disk() {
        let dir = temp();
        let keys = keys();
        let running = fake_bundle(&dir.path().join("Applications"), ID, "0.1.0");
        let mut source = signed_update(dir.path(), &keys);
        for (url, bytes) in &mut source.files {
            if url.rsplit('.').next() == Some("zip") {
                bytes.push(0);
            }
        }
        let config = config(Target::Bundle(running.clone()), &keys.public);
        let run = run(source, config, &[manual_check(), UpdaterRequest::Install]);
        let last = run.states.last().cloned();
        assert_eq!(
            last,
            Some(UpdaterState::Failed {
                reason: "the signature is invalid".into()
            })
        );
        assert_eq!(bundle_version(&running), "0.1.0");
        assert!(!dir.path().join("Applications/.flock-update").exists());
        assert!(run.launched.lock().is_ok_and(|l| l.is_empty()));
    }

    #[test]
    fn a_zip_signed_with_another_key_is_refused() {
        let dir = temp();
        let signer = keys();
        let trusted = keys();
        let running = fake_bundle(&dir.path().join("Applications"), ID, "0.1.0");
        let source = signed_update(dir.path(), &signer);
        let config = config(Target::Bundle(running.clone()), &trusted.public);
        let run = run(source, config, &[manual_check()]);
        assert!(matches!(
            run.states.last(),
            Some(UpdaterState::Failed { reason }) if reason.contains("signature")
        ));
        assert_eq!(bundle_version(&running), "0.1.0");
    }

    #[test]
    fn a_copy_that_cannot_replace_itself_links_to_the_release() {
        let dir = temp();
        let keys = keys();
        let source = signed_update(dir.path(), &keys);
        let run = run(
            source,
            config(Target::Manual, &keys.public),
            &[manual_check()],
        );
        assert_eq!(
            run.states,
            [
                UpdaterState::Checking,
                UpdaterState::Available {
                    version: "0.2.0".into(),
                    page: PAGE.into()
                }
            ]
        );
    }

    #[test]
    fn no_newer_release_is_up_to_date_and_records_the_check() {
        let keys = keys();
        let source = Fake {
            releases: Some(Vec::new()),
            files: BTreeMap::new(),
        };
        let mut updater = Updater::new(
            source,
            config(Target::Manual, &keys.public),
            Box::new(|_| Ok(())),
        );
        updater.handle(&manual_check(), &mut |_| {});
        assert_eq!(updater.status().state, UpdaterState::UpToDate);
        assert_eq!(updater.status().trigger, Trigger::Manual);
        assert!(updater.status().checked.is_some());
    }

    #[test]
    fn an_unreachable_source_fails_with_a_reason() {
        let keys = keys();
        let source = Fake {
            releases: None,
            files: BTreeMap::new(),
        };
        let run = run(
            source,
            config(Target::Manual, &keys.public),
            &[manual_check()],
        );
        assert!(matches!(
            run.states.last(),
            Some(UpdaterState::Failed { reason }) if reason.starts_with("could not reach GitHub")
        ));
    }
}
