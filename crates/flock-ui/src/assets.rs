use std::borrow::Cow;

use gpui::{App, AssetSource, SharedString};

pub const CHEVRON_LEFT: &str = "icons/chevron-left.svg";
pub const CHEVRON_RIGHT: &str = "icons/chevron-right.svg";
pub const CHEVRON_DOWN: &str = "icons/chevron-down.svg";
pub const CHEVRON_UP: &str = "icons/chevron-up.svg";
pub const CHECK: &str = "icons/check.svg";
pub const DOWNLOAD: &str = "icons/download.svg";
pub const PLUS: &str = "icons/plus.svg";
pub const GEAR: &str = "icons/gear.svg";
pub const SPLIT: &str = "icons/split.svg";
pub const REFRESH: &str = "icons/refresh.svg";

const ICONS: &[(&str, &[u8])] = &[
    (
        CHEVRON_LEFT,
        include_bytes!("../assets/icons/chevron-left.svg"),
    ),
    (
        CHEVRON_RIGHT,
        include_bytes!("../assets/icons/chevron-right.svg"),
    ),
    (
        CHEVRON_DOWN,
        include_bytes!("../assets/icons/chevron-down.svg"),
    ),
    (CHEVRON_UP, include_bytes!("../assets/icons/chevron-up.svg")),
    (CHECK, include_bytes!("../assets/icons/check.svg")),
    (DOWNLOAD, include_bytes!("../assets/icons/download.svg")),
    (PLUS, include_bytes!("../assets/icons/plus.svg")),
    (GEAR, include_bytes!("../assets/icons/gear.svg")),
    (SPLIT, include_bytes!("../assets/icons/split.svg")),
    (REFRESH, include_bytes!("../assets/icons/refresh.svg")),
];

pub(crate) const PLEX_SANS_SEMIBOLD: &[u8] =
    include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf");

const FLOCK_STATUS: &[u8] = include_bytes!("../assets/fonts/FlockStatus.ttf");
const FLOCK_STATUS_MEDIUM: &[u8] = include_bytes!("../assets/fonts/FlockStatus-Medium.ttf");

const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf"),
    include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf"),
    PLEX_SANS_SEMIBOLD,
    FLOCK_STATUS,
    FLOCK_STATUS_MEDIUM,
    include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf"),
];

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("registering bundled fonts: {0}")]
    Fonts(String),
}

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let found = ICONS.iter().find(|(name, _)| *name == path);
        Ok(found.map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let names = ICONS.iter().map(|(name, _)| *name);
        Ok(names
            .filter(|name| name.starts_with(path))
            .map(SharedString::new_static)
            .collect())
    }
}

pub fn register_fonts(cx: &App) -> Result<(), AssetError> {
    let fonts = FONTS.iter().map(|bytes| Cow::Borrowed(*bytes)).collect();
    cx.text_system()
        .add_fonts(fonts)
        .map_err(|error| AssetError::Fonts(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::kit::STATUSES;

    const ICON_DROP_PERCENT: i16 = 14;
    const LOADER_PROBE: char = 'm';

    #[test]
    fn flock_status_draws_each_status_symbol_in_a_square_em_box()
    -> Result<(), ttf_parser::FaceParsingError> {
        let plex =
            ttf_parser::Face::parse(include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf"), 0)?;
        let em = plex.units_per_em();
        let em_box = i16::try_from(em).unwrap_or(i16::MAX);
        let bottom = -em_box * ICON_DROP_PERCENT / 100;
        for bytes in [FLOCK_STATUS, FLOCK_STATUS_MEDIUM] {
            let face = ttf_parser::Face::parse(bytes, 0)?;
            assert_eq!(face.units_per_em(), plex.units_per_em());
            assert_eq!(face.ascender(), plex.ascender());
            assert_eq!(face.descender(), plex.descender());
            assert_eq!(face.x_height(), plex.x_height());
            assert_eq!(face.capital_height(), plex.capital_height());
            let family = face.names().into_iter().filter_map(|name| {
                (name.name_id == ttf_parser::name_id::FAMILY)
                    .then(|| name.to_string())
                    .flatten()
            });
            assert!(
                family
                    .into_iter()
                    .any(|name| name == crate::theme::FONT_STATUS)
            );
            let characters: Vec<_> = STATUSES
                .iter()
                .flat_map(|status| status.glyph().chars())
                .collect();
            assert_eq!(usize::from(face.number_of_glyphs()), characters.len() + 2);
            let probe = face.glyph_index(LOADER_PROBE);
            assert!(probe.is_some(), "gpui skips fonts without {LOADER_PROBE}");
            assert!(
                probe
                    .and_then(|glyph| face.glyph_bounding_box(glyph))
                    .is_none()
            );
            for character in characters {
                let glyph = face.glyph_index(character);
                assert!(glyph.is_some(), "missing {character}");
                let advance = glyph.and_then(|glyph| face.glyph_hor_advance(glyph));
                assert_eq!(advance, Some(em));
                let bounds = glyph.and_then(|glyph| face.glyph_bounding_box(glyph));
                let bearing = glyph.and_then(|glyph| face.glyph_hor_side_bearing(glyph));
                assert_eq!(
                    bearing,
                    bounds.map(|bounds| bounds.x_min),
                    "{character} bearing"
                );
                assert!(bounds.is_some_and(|bounds| bounds.x_min >= 0
                    && bounds.x_max <= em_box
                    && bounds.y_min >= bottom
                    && bounds.y_max <= bottom + em_box));
            }
            assert!(face.glyph_index('M').is_none());
        }
        Ok(())
    }

    #[test]
    fn serves_bundled_icons_only() {
        let left = Assets.load(CHEVRON_LEFT).ok().flatten();
        assert!(left.is_some_and(|bytes| bytes.starts_with(b"<svg")));
        assert!(Assets.load("icons/missing.svg").ok().flatten().is_none());
    }
}
