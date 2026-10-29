use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use gpui::{App, Global};
use serde::{Deserialize, Serialize};

const FILE: &str = "view.json";
const TEMP_SUFFIX: &str = "json.tmp";
pub const FLOCK_SCOPE: &str = "flock";

#[derive(Debug, thiserror::Error)]
pub enum PrefsError {
    #[error("view preferences: {0}")]
    Io(#[from] std::io::Error),
    #[error("view preferences are not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

pub type ColumnWidths = BTreeMap<String, f32>;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScopePrefs {
    pub columns: BTreeMap<String, ColumnWidths>,
    pub groups: BTreeMap<String, Grouping>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Grouping {
    pub by: Option<String>,
    pub collapsed: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrefsFile {
    pub scopes: BTreeMap<String, ScopePrefs>,
}

#[derive(Debug, Default)]
pub struct ViewPrefs {
    path: Option<PathBuf>,
    file: PrefsFile,
}

impl Global for ViewPrefs {}

impl ViewPrefs {
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn empty_at(dir: &Path) -> Self {
        Self {
            path: Some(dir.join(FILE)),
            file: PrefsFile::default(),
        }
    }

    pub fn open(dir: &Path) -> Result<Self, PrefsError> {
        let path = dir.join(FILE);
        let file = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == ErrorKind::NotFound => PrefsFile::default(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path: Some(path),
            file,
        })
    }

    pub fn column_widths(&self, scope: &str, table: &str) -> ColumnWidths {
        self.file
            .scopes
            .get(scope)
            .and_then(|prefs| prefs.columns.get(table))
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_column_widths(
        &mut self,
        scope: &str,
        table: &str,
        widths: ColumnWidths,
    ) -> Result<(), PrefsError> {
        let prefs = self.file.scopes.entry(scope.to_owned()).or_default();
        if prefs.columns.get(table) == Some(&widths) {
            return Ok(());
        }
        prefs.columns.insert(table.to_owned(), widths);
        self.save()
    }

    pub fn grouping(&self, scope: &str, table: &str) -> Grouping {
        self.saved_grouping(scope, table).unwrap_or_default()
    }

    pub fn saved_grouping(&self, scope: &str, table: &str) -> Option<Grouping> {
        self.file
            .scopes
            .get(scope)
            .and_then(|prefs| prefs.groups.get(table))
            .cloned()
    }

    pub fn set_grouping(
        &mut self,
        scope: &str,
        table: &str,
        grouping: Grouping,
    ) -> Result<(), PrefsError> {
        let prefs = self.file.scopes.entry(scope.to_owned()).or_default();
        if prefs.groups.get(table) == Some(&grouping) {
            return Ok(());
        }
        prefs.groups.insert(table.to_owned(), grouping);
        self.save()
    }

    fn save(&self) -> Result<(), PrefsError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_vec_pretty(&self.file)?;
        let temp = path.with_extension(TEMP_SUFFIX);
        fs::write(&temp, json)?;
        fs::rename(&temp, path)?;
        Ok(())
    }
}

pub fn init(dir: Option<&Path>, cx: &mut App) {
    let prefs = match dir {
        Some(dir) => ViewPrefs::open(dir).unwrap_or_else(|error| {
            tracing::warn!(%error, "starting with default view preferences");
            ViewPrefs::empty_at(dir)
        }),
        None => ViewPrefs::in_memory(),
    };
    cx.set_global(prefs);
}

pub fn column_widths(scope: &str, table: &str, cx: &App) -> ColumnWidths {
    cx.try_global::<ViewPrefs>()
        .map(|prefs| prefs.column_widths(scope, table))
        .unwrap_or_default()
}

pub fn set_column_widths(scope: &str, table: &str, widths: ColumnWidths, cx: &mut App) {
    let result = cx
        .default_global::<ViewPrefs>()
        .set_column_widths(scope, table, widths);
    if let Err(error) = result {
        tracing::warn!(%error, "could not save column widths");
    }
}

pub fn saved_grouping(scope: &str, table: &str, cx: &App) -> Option<Grouping> {
    cx.try_global::<ViewPrefs>()
        .and_then(|prefs| prefs.saved_grouping(scope, table))
}

pub fn set_grouping(scope: &str, table: &str, grouping: Grouping, cx: &mut App) {
    let result = cx
        .default_global::<ViewPrefs>()
        .set_grouping(scope, table, grouping);
    if let Err(error) = result {
        tracing::warn!(%error, "could not save the table grouping");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_widths_survive_a_reopen() -> Result<(), PrefsError> {
        let dir = tempfile::tempdir()?;
        let mut prefs = ViewPrefs::open(dir.path())?;
        let widths = ColumnWidths::from([("Name".to_owned(), 320.)]);
        prefs.set_column_widths("prod", "pods", widths.clone())?;
        let reopened = ViewPrefs::open(dir.path())?;
        assert_eq!(reopened.column_widths("prod", "pods"), widths);
        assert!(reopened.column_widths("staging", "pods").is_empty());
        assert!(reopened.column_widths("prod", "nodes").is_empty());
        Ok(())
    }

    #[test]
    fn groupings_survive_a_reopen_per_scope_and_table() -> Result<(), PrefsError> {
        let dir = tempfile::tempdir()?;
        let mut prefs = ViewPrefs::open(dir.path())?;
        let grouping = Grouping {
            by: Some("namespace".to_owned()),
            collapsed: BTreeSet::from(["kube-system".to_owned()]),
        };
        prefs.set_grouping("prod", "pods", grouping.clone())?;
        let reopened = ViewPrefs::open(dir.path())?;
        assert_eq!(reopened.grouping("prod", "pods"), grouping);
        assert_eq!(reopened.grouping("staging", "pods"), Grouping::default());
        assert_eq!(reopened.grouping("prod", "nodes"), Grouping::default());
        Ok(())
    }

    #[test]
    fn files_written_before_grouping_still_load() -> Result<(), PrefsError> {
        let dir = tempfile::tempdir()?;
        let old = br#"{"scopes":{"prod":{"columns":{"pods":{"Name":320.0}}}}}"#;
        fs::write(dir.path().join(FILE), old)?;
        let prefs = ViewPrefs::open(dir.path())?;
        assert_eq!(prefs.grouping("prod", "pods"), Grouping::default());
        assert_eq!(prefs.column_widths("prod", "pods").len(), 1);
        Ok(())
    }

    #[test]
    fn a_missing_file_starts_empty_and_a_broken_one_is_an_error() -> Result<(), PrefsError> {
        let dir = tempfile::tempdir()?;
        assert_eq!(ViewPrefs::open(dir.path())?.file, PrefsFile::default());
        fs::write(dir.path().join(FILE), b"{ not json")?;
        assert!(matches!(
            ViewPrefs::open(dir.path()),
            Err(PrefsError::Json(_))
        ));
        Ok(())
    }

    #[test]
    fn in_memory_prefs_never_touch_the_disk() -> Result<(), PrefsError> {
        let mut prefs = ViewPrefs::in_memory();
        let widths = ColumnWidths::from([("Age".to_owned(), 60.)]);
        prefs.set_column_widths(FLOCK_SCOPE, "flock-clusters", widths.clone())?;
        assert_eq!(prefs.column_widths(FLOCK_SCOPE, "flock-clusters"), widths);
        Ok(())
    }
}
