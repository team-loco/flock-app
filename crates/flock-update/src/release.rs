use semver::Version;
use serde::Deserialize;

use crate::signature::{CHECKSUM_EXTENSION, SIGNATURE_EXTENSION};
use crate::version::{self, is_prerelease};
use crate::{Error, Result};

const ASSET_PREFIX: &str = "Flock";
const ARCH: &str = "arm64";
const ZIP_EXTENSION: &str = "zip";
const LOCO_VARIANT: &str = "loco";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    Plain,
    Loco,
}

impl Variant {
    #[cfg(not(feature = "loco"))]
    pub const CURRENT: Self = Self::Plain;
    #[cfg(feature = "loco")]
    pub const CURRENT: Self = Self::Loco;
}

#[derive(Debug, Deserialize)]
struct RawRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    #[serde(rename = "browser_download_url")]
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub page: String,
    pub draft: bool,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artifacts {
    pub zip: Asset,
    pub checksum: Asset,
    pub signature: Asset,
}

impl RawRelease {
    fn release(self) -> Option<Release> {
        let version = version::parse(&self.tag_name).ok()?;
        Some(Release {
            version,
            page: self.html_url,
            draft: self.draft,
            prerelease: self.prerelease,
            assets: self.assets,
        })
    }
}

pub fn parse_latest(json: &str) -> Result<Vec<Release>> {
    let raw: RawRelease = serde_json::from_str(json).map_err(|error| Error::json(&error))?;
    Ok(raw.release().into_iter().collect())
}

pub fn parse_list(json: &str) -> Result<Vec<Release>> {
    let raw: Vec<RawRelease> = serde_json::from_str(json).map_err(|error| Error::json(&error))?;
    Ok(raw.into_iter().filter_map(RawRelease::release).collect())
}

pub fn newest(releases: Vec<Release>, current: &Version, prereleases: bool) -> Option<Release> {
    releases
        .into_iter()
        .filter(|release| !release.draft)
        .filter(|release| prereleases || !(release.prerelease || is_prerelease(&release.version)))
        .filter(|release| release.version > *current)
        .max_by(|a, b| a.version.cmp(&b.version))
}

pub fn zip_name(version: &Version, variant: Variant) -> String {
    match variant {
        Variant::Plain => format!("{ASSET_PREFIX}-{version}-{ARCH}.{ZIP_EXTENSION}"),
        Variant::Loco => format!("{ASSET_PREFIX}-{version}-{LOCO_VARIANT}-{ARCH}.{ZIP_EXTENSION}"),
    }
}

impl Release {
    fn asset(&self, name: &str) -> Result<Asset> {
        self.assets
            .iter()
            .find(|asset| asset.name == name)
            .cloned()
            .ok_or_else(|| Error::MissingAsset {
                version: self.version.to_string(),
                name: name.to_owned(),
            })
    }

    pub fn artifacts(&self) -> Result<Artifacts> {
        self.artifacts_for(Variant::CURRENT)
    }

    fn artifacts_for(&self, variant: Variant) -> Result<Artifacts> {
        let zip = zip_name(&self.version, variant);
        Ok(Artifacts {
            zip: self.asset(&zip)?,
            checksum: self.asset(&format!("{zip}.{CHECKSUM_EXTENSION}"))?,
            signature: self.asset(&format!("{zip}.{SIGNATURE_EXTENSION}"))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LATEST: &str = include_str!("fixtures/latest.json");
    const LIST: &str = include_str!("fixtures/releases.json");

    fn current() -> Version {
        Version::new(0, 1, 0)
    }

    fn latest() -> Release {
        let releases = parse_latest(LATEST).unwrap_or_default();
        let [release] = releases.as_slice() else {
            panic!("expected one release, got {releases:?}");
        };
        release.clone()
    }

    fn zip_url(release: &Release, variant: Variant) -> String {
        release.artifacts_for(variant).map_or_else(
            |error| panic!("artifacts: {error}"),
            |artifacts| artifacts.zip.url,
        )
    }

    #[test]
    fn each_variant_picks_only_its_own_zip() {
        let release = latest();
        assert!(zip_url(&release, Variant::Plain).ends_with("/Flock-0.2.0-arm64.zip"));
        assert!(zip_url(&release, Variant::Loco).ends_with("/Flock-0.2.0-loco-arm64.zip"));
    }

    #[test]
    fn the_running_build_picks_its_own_variant() {
        let release = latest();
        let current = release
            .artifacts()
            .unwrap_or_else(|error| panic!("artifacts: {error}"));
        assert_eq!(current.zip.url, zip_url(&release, Variant::CURRENT));
        assert_eq!(Variant::CURRENT == Variant::Loco, cfg!(feature = "loco"));
    }

    #[test]
    fn a_release_without_the_loco_zip_has_no_loco_artifacts() {
        let mut release = latest();
        release
            .assets
            .retain(|asset| !asset.name.contains("-loco-"));
        assert!(release.artifacts_for(Variant::Plain).is_ok());
        assert!(matches!(
            release.artifacts_for(Variant::Loco),
            Err(Error::MissingAsset { .. })
        ));
    }

    #[test]
    fn a_release_without_the_plain_zip_has_no_plain_artifacts() {
        let mut release = latest();
        release.assets.retain(|asset| asset.name.contains("-loco-"));
        assert!(release.artifacts_for(Variant::Loco).is_ok());
        assert!(matches!(
            release.artifacts_for(Variant::Plain),
            Err(Error::MissingAsset { .. })
        ));
    }

    #[test]
    fn the_latest_release_parses_with_its_three_artifacts() {
        let releases = parse_latest(LATEST).unwrap_or_default();
        let [release] = releases.as_slice() else {
            panic!("expected one release, got {releases:?}");
        };
        assert_eq!(release.version, Version::new(0, 2, 0));
        assert_eq!(
            release.page,
            "https://github.com/team-loco/flock-app/releases/tag/v0.2.0"
        );
        let artifacts = release
            .artifacts_for(Variant::Plain)
            .unwrap_or_else(|error| panic!("artifacts: {error}"));
        assert_eq!(artifacts.zip.name, "Flock-0.2.0-arm64.zip");
        assert_eq!(artifacts.zip.size, 41_234_567);
        assert!(
            artifacts
                .checksum
                .url
                .ends_with("/Flock-0.2.0-arm64.zip.sha256")
        );
        assert!(
            artifacts
                .signature
                .url
                .ends_with("/Flock-0.2.0-arm64.zip.sig")
        );
    }

    #[test]
    fn a_release_without_a_signature_has_no_artifacts() {
        let releases = parse_list(LIST).unwrap_or_default();
        let stable = releases
            .iter()
            .find(|release| release.version == Version::new(0, 2, 0));
        let missing = stable.map(|release| release.artifacts_for(Variant::Plain));
        assert!(matches!(missing, Some(Err(Error::MissingAsset { .. }))));
    }

    #[test]
    fn the_list_skips_tags_that_are_not_versions() {
        let releases = parse_list(LIST).unwrap_or_default();
        let tags: Vec<String> = releases.iter().map(|r| r.version.to_string()).collect();
        assert_eq!(tags, ["0.4.0", "0.3.0-beta.1", "0.2.0", "0.1.0"]);
    }

    #[test]
    fn drafts_are_never_offered_and_prereleases_only_on_request() {
        let releases = parse_list(LIST).unwrap_or_default();
        let stable = newest(releases.clone(), &current(), false);
        assert_eq!(stable.map(|r| r.version), Some(Version::new(0, 2, 0)));
        let beta = newest(releases, &current(), true);
        assert_eq!(
            beta.map(|r| r.version.to_string()),
            Some("0.3.0-beta.1".into())
        );
    }

    #[test]
    fn nothing_newer_than_the_running_version_means_up_to_date() {
        let releases = parse_latest(LATEST).unwrap_or_default();
        assert_eq!(
            newest(releases.clone(), &Version::new(0, 2, 0), false),
            None
        );
        assert_eq!(newest(releases, &Version::new(0, 3, 0), true), None);
    }

    #[test]
    fn malformed_json_is_an_error() {
        assert!(matches!(
            parse_latest("{\"tag_name\": 3}"),
            Err(Error::Json(_))
        ));
        assert!(matches!(parse_list("<html>"), Err(Error::Json(_))));
    }
}
