use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use flock_core::settings::{MetricsSource, NamespaceSelection};
use flock_core::{ClusterId, NewCluster, Preferences};
use jiff::Timestamp;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::discovery::{ContextRef, Fingerprint, KubeContext};
use crate::error::Result;

pub(crate) const APP_SUPPORT: &str = "Library/Application Support";
pub(crate) const CACHES: &str = "Library/Caches";
pub(crate) const APP_ID: &str = "dev.flock.app";
const CLUSTERS_FILE: &str = "clusters.json";
const TEMP_SUFFIX: &str = "json.tmp";
const CORRUPT_SUFFIX: &str = "corrupt";
const CORRUPT_STAMP: &str = "%Y%m%dT%H%M%SZ";
const KUBECONFIGS_DIR: &str = "kubeconfigs";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedCluster {
    pub file: PathBuf,
    pub context: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default)]
    pub preferences: SavedPreferences,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<Pin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    pub server: Option<String>,
    pub trust: Fingerprint,
}

impl Pin {
    pub fn of(context: &KubeContext) -> Self {
        Self {
            server: context.server.clone(),
            trust: context.trust.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedPreferences {
    pub namespaces: BTreeMap<String, SavedNamespace>,
    pub default_namespace: Option<String>,
    pub metrics: SavedMetrics,
    pub confirm_destructive: bool,
    pub notify: bool,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SavedMetrics {
    #[default]
    Auto,
    MetricsServer,
    Prometheus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SavedNamespace {
    All,
    Named(String),
}

impl From<&NamespaceSelection> for SavedNamespace {
    fn from(selection: &NamespaceSelection) -> Self {
        match selection {
            NamespaceSelection::All => Self::All,
            NamespaceSelection::Named(namespace) => Self::Named(namespace.clone()),
        }
    }
}

impl From<&SavedNamespace> for NamespaceSelection {
    fn from(saved: &SavedNamespace) -> Self {
        match saved {
            SavedNamespace::All => Self::All,
            SavedNamespace::Named(namespace) => Self::Named(namespace.clone()),
        }
    }
}

impl Default for SavedPreferences {
    fn default() -> Self {
        Self::from(&Preferences::default())
    }
}

impl From<&Preferences> for SavedPreferences {
    fn from(preferences: &Preferences) -> Self {
        let metrics = match preferences.metrics {
            MetricsSource::Auto => SavedMetrics::Auto,
            MetricsSource::MetricsServer => SavedMetrics::MetricsServer,
            MetricsSource::Prometheus => SavedMetrics::Prometheus,
        };
        Self {
            namespaces: preferences
                .namespaces
                .iter()
                .map(|(list, selection)| (list.clone(), SavedNamespace::from(selection)))
                .collect(),
            default_namespace: preferences.default_namespace.clone(),
            metrics,
            confirm_destructive: preferences.confirm_destructive,
            notify: preferences.notify,
            labels: preferences.labels.clone(),
        }
    }
}

impl From<&SavedPreferences> for Preferences {
    fn from(saved: &SavedPreferences) -> Self {
        let metrics = match saved.metrics {
            SavedMetrics::Auto => MetricsSource::Auto,
            SavedMetrics::MetricsServer => MetricsSource::MetricsServer,
            SavedMetrics::Prometheus => MetricsSource::Prometheus,
        };
        Self {
            namespaces: saved
                .namespaces
                .iter()
                .map(|(list, saved)| (list.clone(), NamespaceSelection::from(saved)))
                .collect(),
            default_namespace: saved.default_namespace.clone(),
            metrics,
            confirm_destructive: saved.confirm_destructive,
            notify: saved.notify,
            labels: saved.labels.clone(),
        }
    }
}

impl SavedCluster {
    pub fn context_ref(&self) -> ContextRef {
        ContextRef {
            file: self.file.clone(),
            context: self.context.clone(),
        }
    }

    pub fn id(&self) -> ClusterId {
        ClusterId::new(&self.name)
    }

    pub fn label(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct StoreFile {
    clusters: Vec<SavedCluster>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterStore {
    path: PathBuf,
    clusters: Vec<SavedCluster>,
    set_aside: Option<PathBuf>,
}

impl ClusterStore {
    pub fn default_dir(home: &Path) -> PathBuf {
        home.join(APP_SUPPORT).join(APP_ID)
    }

    pub fn open(dir: &Path) -> Result<Self> {
        Self::open_at(dir, Timestamp::now())
    }

    pub fn open_at(dir: &Path, now: Timestamp) -> Result<Self> {
        let loaded = load::<StoreFile>(&dir.join(CLUSTERS_FILE), now)?;
        let file = loaded.value.unwrap_or_default();
        Ok(Self {
            clusters: file.clusters,
            set_aside: loaded.set_aside,
            ..Self::empty(dir)
        })
    }

    pub fn empty(dir: &Path) -> Self {
        Self {
            path: dir.join(CLUSTERS_FILE),
            clusters: Vec::new(),
            set_aside: None,
        }
    }

    pub fn set_aside(&self) -> Option<&Path> {
        self.set_aside.as_deref()
    }

    pub fn kubeconfigs_dir(&self) -> PathBuf {
        let dir = self.path.parent().unwrap_or(Path::new(""));
        dir.join(KUBECONFIGS_DIR)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn clusters(&self) -> &[SavedCluster] {
        &self.clusters
    }

    pub fn contains(&self, context: &ContextRef) -> bool {
        self.clusters.iter().any(|c| &c.context_ref() == context)
    }

    pub fn add(&mut self, added: &[NewCluster]) -> Result<()> {
        for cluster in added {
            let taken = self.contains(&cluster.context)
                || self.clusters.iter().any(|c| c.name == cluster.name);
            if taken {
                continue;
            }
            self.clusters.push(SavedCluster {
                file: cluster.context.file.clone(),
                context: cluster.context.context.clone(),
                name: cluster.name.clone(),
                display_name: None,
                preferences: SavedPreferences::default(),
                pin: None,
            });
        }
        self.save()
    }

    pub fn remove(&mut self, id: &ClusterId) -> Result<Option<SavedCluster>> {
        let Some(ix) = self.clusters.iter().position(|c| &c.id() == id) else {
            return Ok(None);
        };
        let removed = self.clusters.remove(ix);
        self.save()?;
        Ok(Some(removed))
    }

    pub fn get(&self, id: &ClusterId) -> Option<&SavedCluster> {
        self.clusters.iter().find(|c| &c.id() == id)
    }

    pub fn set_preferences(&mut self, id: &ClusterId, preferences: &Preferences) -> Result<bool> {
        let Some(saved) = self.clusters.iter_mut().find(|c| &c.id() == id) else {
            return Ok(false);
        };
        saved.preferences = SavedPreferences::from(preferences);
        self.save()?;
        Ok(true)
    }

    pub fn rename(&mut self, id: &ClusterId, input: &str) -> Result<bool> {
        let Some(saved) = self.clusters.iter_mut().find(|c| &c.id() == id) else {
            return Ok(false);
        };
        let display_name = flock_core::rename::display_name(input);
        if saved.display_name == display_name {
            return Ok(false);
        }
        saved.display_name = display_name;
        self.save()?;
        Ok(true)
    }

    pub fn rebind(&mut self, id: &ClusterId, to: &ContextRef) -> Result<bool> {
        if self.contains(to) {
            return Ok(false);
        }
        let Some(saved) = self.clusters.iter_mut().find(|c| &c.id() == id) else {
            return Ok(false);
        };
        saved.file.clone_from(&to.file);
        saved.context.clone_from(&to.context);
        saved.pin = None;
        self.save()?;
        Ok(true)
    }

    pub fn pin(&mut self, id: &ClusterId, pin: Pin) -> Result<bool> {
        let Some(saved) = self.clusters.iter_mut().find(|c| &c.id() == id) else {
            return Ok(false);
        };
        if saved.pin.as_ref() == Some(&pin) {
            return Ok(false);
        }
        saved.pin = Some(pin);
        self.save()?;
        Ok(true)
    }

    fn save(&self) -> Result<()> {
        let body = StoreFile {
            clusters: self.clusters.clone(),
        };
        write(&self.path, &body)
    }
}

pub(crate) struct Loaded<T> {
    pub value: Option<T>,
    pub set_aside: Option<PathBuf>,
}

pub(crate) fn load<T: DeserializeOwned>(path: &Path, now: Timestamp) -> Result<Loaded<T>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(Loaded {
                value: None,
                set_aside: None,
            });
        }
        Err(error) => return Err(error.into()),
    };
    match serde_json::from_slice::<T>(&bytes) {
        Ok(value) => Ok(Loaded {
            value: Some(value),
            set_aside: None,
        }),
        Err(error) => {
            let aside = corrupt_path(path, now);
            fs::rename(path, &aside)?;
            tracing::warn!(%error, file = %path.display(), kept = %aside.display(), "unreadable; set aside");
            Ok(Loaded {
                value: None,
                set_aside: Some(aside),
            })
        }
    }
}

pub(crate) fn write<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(value)?;
    let temp = path.with_extension(TEMP_SUFFIX);
    fs::write(&temp, json)?;
    fs::rename(&temp, path)?;
    Ok(())
}

fn corrupt_path(path: &Path, now: Timestamp) -> PathBuf {
    let stamp = now.strftime(CORRUPT_STAMP);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!("{name}.{CORRUPT_SUFFIX}-{stamp}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Timestamp {
        match "2026-10-13T06:36:00Z".parse() {
            Ok(now) => now,
            Err(error) => panic!("timestamp: {error}"),
        }
    }

    fn new_cluster(context: &str, name: &str) -> NewCluster {
        NewCluster {
            context: ContextRef {
                file: PathBuf::from("/h/.kube/config"),
                context: context.to_owned(),
            },
            name: name.to_owned(),
        }
    }

    #[test]
    fn renaming_sets_a_display_name_and_blank_input_resets_it() {
        let Ok(dir) = tempfile::tempdir() else {
            panic!("tempdir");
        };
        let mut store = ClusterStore::empty(dir.path());
        let id = ClusterId::new("fleet-a");
        assert!(store.add(&[new_cluster("kind-fleet-a", "fleet-a")]).is_ok());
        assert_eq!(store.rename(&id, " Prod A ").ok(), Some(true));
        assert_eq!(store.get(&id).map(SavedCluster::label), Some("Prod A"));
        assert_eq!(store.rename(&id, "Prod A").ok(), Some(false));
        assert_eq!(store.rename(&id, "  ").ok(), Some(true));
        assert_eq!(store.get(&id).map(SavedCluster::label), Some("fleet-a"));
        assert_eq!(store.rename(&ClusterId::new("gone"), "x").ok(), Some(false));
    }

    #[test]
    fn rebinding_keeps_the_entry_and_refuses_contexts_already_added() {
        let Ok(dir) = tempfile::tempdir() else {
            panic!("tempdir");
        };
        let mut store = ClusterStore::empty(dir.path());
        let id = ClusterId::new("fleet-a");
        let added = [
            new_cluster("kind-fleet-a", "fleet-a"),
            new_cluster("kind-fleet-b", "fleet-b"),
        ];
        assert!(store.add(&added).is_ok());
        assert_eq!(store.rename(&id, "Prod A").ok(), Some(true));
        let pin = Pin {
            server: None,
            trust: Fingerprint::default(),
        };
        assert_eq!(store.pin(&id, pin).ok(), Some(true));
        let taken = new_cluster("kind-fleet-b", "x").context;
        assert_eq!(store.rebind(&id, &taken).ok(), Some(false));
        let free = new_cluster("kind-other", "x").context;
        assert_eq!(store.rebind(&id, &free).ok(), Some(true));
        let Some(saved) = store.get(&id) else {
            panic!("entry kept");
        };
        assert_eq!(saved.context_ref(), free);
        assert_eq!(saved.label(), "Prod A");
        assert_eq!(saved.pin, None);
    }

    #[test]
    fn missing_file_is_an_empty_flock() {
        let Ok(dir) = tempfile::tempdir() else {
            panic!("tempdir");
        };
        let Ok(store) = ClusterStore::open_at(dir.path(), now()) else {
            panic!("open");
        };
        assert_eq!(store.clusters(), &[]);
    }

    #[test]
    fn added_clusters_survive_a_reopen_and_removal_forgets_them() {
        let Ok(dir) = tempfile::tempdir() else {
            panic!("tempdir");
        };
        let nested = dir.path().join("dev.flock.app");
        let mut store = ClusterStore::empty(&nested);
        let added = [
            new_cluster("kind-fleet-a", "fleet-a"),
            new_cluster("kind-fleet-b", "fleet-b"),
            new_cluster("kind-fleet-a", "again"),
            new_cluster("other", "fleet-b"),
        ];
        assert!(store.add(&added).is_ok());
        let Ok(reopened) = ClusterStore::open_at(&nested, now()) else {
            panic!("reopen");
        };
        let names: Vec<&str> = reopened
            .clusters()
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, ["fleet-a", "fleet-b"]);
        assert!(reopened.contains(&new_cluster("kind-fleet-b", "x").context));

        let mut store = reopened;
        let removed = store.remove(&ClusterId::new("fleet-a"));
        assert_eq!(
            removed.ok().flatten().map(|c| c.context),
            Some("kind-fleet-a".to_owned())
        );
        let Ok(reopened) = ClusterStore::open_at(&nested, now()) else {
            panic!("reopen");
        };
        assert_eq!(reopened.clusters().len(), 1);
        assert!(matches!(store.remove(&ClusterId::new("nope")), Ok(None)));
    }

    #[test]
    fn preferences_survive_a_reopen() {
        let Ok(dir) = tempfile::tempdir() else {
            panic!("tempdir");
        };
        let mut store = ClusterStore::empty(dir.path());
        assert!(store.add(&[new_cluster("kind-fleet-a", "fleet-a")]).is_ok());
        let preferences = Preferences {
            namespaces: BTreeMap::from([("Event".to_owned(), NamespaceSelection::All)]),
            default_namespace: Some("payments".to_owned()),
            metrics: MetricsSource::Prometheus,
            confirm_destructive: false,
            notify: false,
            labels: vec!["env=dev".to_owned()],
        };
        let id = ClusterId::new("fleet-a");
        assert!(matches!(store.set_preferences(&id, &preferences), Ok(true)));
        assert!(matches!(
            store.set_preferences(&ClusterId::new("nope"), &preferences),
            Ok(false)
        ));
        let Ok(reopened) = ClusterStore::open_at(dir.path(), now()) else {
            panic!("reopen");
        };
        let saved = reopened.get(&id).map(|c| Preferences::from(&c.preferences));
        assert_eq!(saved, Some(preferences.clone()));
        let mut reopened = reopened;
        let mut named = preferences;
        named.select_namespace("Pod", NamespaceSelection::Named("web".into()));
        assert!(reopened.set_preferences(&id, &named).is_ok());
        let reopened = ClusterStore::open_at(dir.path(), now())
            .unwrap_or_else(|error| panic!("reopen: {error}"));
        assert_eq!(
            reopened
                .get(&id)
                .map(|cluster| Preferences::from(&cluster.preferences)),
            Some(named)
        );
    }

    #[test]
    fn files_without_preferences_get_defaults() {
        let Ok(dir) = tempfile::tempdir() else {
            panic!("tempdir");
        };
        let old = r#"{"clusters":[{"file":"/h/.kube/config","context":"a","name":"a"}]}"#;
        assert!(fs::write(dir.path().join(CLUSTERS_FILE), old).is_ok());
        let Ok(store) = ClusterStore::open_at(dir.path(), now()) else {
            panic!("open");
        };
        let saved = store
            .get(&ClusterId::new("a"))
            .map(|c| Preferences::from(&c.preferences));
        assert_eq!(saved, Some(Preferences::default()));
    }

    #[test]
    fn corrupt_file_is_set_aside_before_anything_is_written() {
        let Ok(dir) = tempfile::tempdir() else {
            panic!("tempdir");
        };
        assert!(fs::write(dir.path().join(CLUSTERS_FILE), "{not json").is_ok());
        let Ok(mut store) = ClusterStore::open_at(dir.path(), now()) else {
            panic!("open");
        };
        let kept = dir.path().join("clusters.json.corrupt-20261013T063600Z");
        assert_eq!(store.set_aside(), Some(kept.as_path()));
        assert_eq!(store.clusters(), &[]);
        assert!(store.add(&[new_cluster("kind-fleet-a", "fleet-a")]).is_ok());
        assert_eq!(fs::read_to_string(&kept).ok().as_deref(), Some("{not json"));
        let Ok(reopened) = ClusterStore::open_at(dir.path(), now()) else {
            panic!("reopen");
        };
        assert_eq!(reopened.clusters().len(), 1);
        assert_eq!(reopened.set_aside(), None);
    }
}
