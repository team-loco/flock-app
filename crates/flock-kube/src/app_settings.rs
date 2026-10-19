use std::path::{Path, PathBuf};

use flock_core::AppSettings;
use jiff::Timestamp;

use crate::error::Result;
use crate::store::{load, write};

const SETTINGS_FILE: &str = "settings.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsStore {
    path: PathBuf,
    settings: AppSettings,
    set_aside: Option<PathBuf>,
}

impl SettingsStore {
    pub fn open(dir: &Path) -> Result<Self> {
        Self::open_at(dir, Timestamp::now())
    }

    pub fn open_at(dir: &Path, now: Timestamp) -> Result<Self> {
        let path = dir.join(SETTINGS_FILE);
        let loaded = load::<AppSettings>(&path, now)?;
        Ok(Self {
            path,
            settings: loaded.value.unwrap_or_default(),
            set_aside: loaded.set_aside,
        })
    }

    pub fn settings(&self) -> &AppSettings {
        &self.settings
    }

    pub fn set_aside(&self) -> Option<&Path> {
        self.set_aside.as_deref()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn save(&mut self, settings: &AppSettings) -> Result<()> {
        write(&self.path, settings)?;
        self.settings = settings.clone();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use flock_core::app_settings::{Highlight, PollInterval, Refresh, Rgb, ThemeChoice};

    use super::*;

    fn now() -> Timestamp {
        match "2026-10-13T06:36:00Z".parse() {
            Ok(now) => now,
            Err(error) => panic!("timestamp: {error}"),
        }
    }

    #[test]
    fn a_missing_file_reads_as_the_defaults() {
        let Ok(dir) = tempfile::tempdir() else {
            panic!("tempdir");
        };
        let Ok(store) = SettingsStore::open_at(dir.path(), now()) else {
            panic!("open");
        };
        assert_eq!(store.settings(), &AppSettings::default());
        assert_eq!(store.set_aside(), None);
    }

    #[test]
    fn saved_settings_survive_a_reopen() {
        let Ok(dir) = tempfile::tempdir() else {
            panic!("tempdir");
        };
        let nested = dir.path().join("dev.flock.app");
        let Ok(mut store) = SettingsStore::open_at(&nested, now()) else {
            panic!("open");
        };
        let mut settings = AppSettings {
            show_in_menu_bar: false,
            theme: ThemeChoice::Light,
            primary: Highlight::Custom(Rgb {
                r: 0xd9,
                g: 0x46,
                b: 0x8a,
            }),
            secondary: Highlight::Graphite,
            refresh: Refresh {
                metrics: PollInterval::Seconds60,
                ..Refresh::default()
            },
            ..AppSettings::default()
        };
        settings
            .inspector_widths
            .insert("node-inspector".to_owned(), 640);
        settings
            .inspector_widths
            .insert("resource-inspector".to_owned(), 420);
        assert!(store.save(&settings).is_ok());
        let Ok(reopened) = SettingsStore::open_at(&nested, now()) else {
            panic!("reopen");
        };
        assert_eq!(reopened.settings(), &settings);
    }

    #[test]
    fn a_corrupt_file_is_set_aside_and_defaults_apply() {
        let Ok(dir) = tempfile::tempdir() else {
            panic!("tempdir");
        };
        assert!(fs::write(dir.path().join(SETTINGS_FILE), "{\"theme\": 3").is_ok());
        let Ok(mut store) = SettingsStore::open_at(dir.path(), now()) else {
            panic!("open");
        };
        let kept = dir.path().join("settings.json.corrupt-20261013T063600Z");
        assert_eq!(store.set_aside(), Some(kept.as_path()));
        assert_eq!(store.settings(), &AppSettings::default());
        assert!(store.save(&AppSettings::default()).is_ok());
        assert_eq!(
            fs::read_to_string(&kept).ok().as_deref(),
            Some("{\"theme\": 3")
        );
    }
}
