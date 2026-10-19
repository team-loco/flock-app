use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

const HEX_PREFIX: &str = "#";
const HEX_DIGITS: usize = 6;
const HEX_RADIX: u32 = 16;
const RED_SHIFT: u32 = 16;
const GREEN_SHIFT: u32 = 8;
const CHANNEL: u32 = 0xFF;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub show_in_menu_bar: bool,
    pub inspector_widths: BTreeMap<String, u16>,
    pub theme: ThemeChoice,
    pub primary: Highlight,
    pub secondary: Highlight,
    pub refresh: Refresh,
    pub notifications: Notifications,
    pub updates: Updates,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            show_in_menu_bar: true,
            inspector_widths: BTreeMap::default(),
            theme: ThemeChoice::default(),
            primary: Highlight::default(),
            secondary: Highlight::default(),
            refresh: Refresh::default(),
            notifications: Notifications::default(),
            updates: Updates::default(),
        }
    }
}

impl AppSettings {
    pub fn keeps_running_in_menu_bar(&self) -> bool {
        cfg!(target_os = "macos") && self.show_in_menu_bar
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub const fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Highlight {
    #[default]
    Blue,
    Purple,
    Teal,
    Green,
    Orange,
    Pink,
    Graphite,
    Custom(Rgb),
}

impl Highlight {
    pub const PRESETS: [Self; 7] = [
        Self::Blue,
        Self::Purple,
        Self::Teal,
        Self::Green,
        Self::Orange,
        Self::Pink,
        Self::Graphite,
    ];

    pub fn label(self) -> String {
        match self {
            Self::Blue => "Blue".to_owned(),
            Self::Purple => "Purple".to_owned(),
            Self::Teal => "Teal".to_owned(),
            Self::Green => "Green".to_owned(),
            Self::Orange => "Orange".to_owned(),
            Self::Pink => "Pink".to_owned(),
            Self::Graphite => "Graphite".to_owned(),
            Self::Custom(rgb) => rgb.to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn to_u32(self) -> u32 {
        ((self.r as u32) << RED_SHIFT) | ((self.g as u32) << GREEN_SHIFT) | self.b as u32
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("expected a colour like #3a7bd5")]
pub struct BadHex;

impl FromStr for Rgb {
    type Err = BadHex;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let trimmed = input.trim();
        let digits = trimmed.strip_prefix(HEX_PREFIX).unwrap_or(trimmed);
        let valid = digits.len() == HEX_DIGITS && digits.chars().all(|c| c.is_ascii_hexdigit());
        if !valid {
            return Err(BadHex);
        }
        let value = u32::from_str_radix(digits, HEX_RADIX).map_err(|_| BadHex)?;
        let channel = |shift: u32| u8::try_from((value >> shift) & CHANNEL).map_err(|_| BadHex);
        Ok(Self {
            r: channel(RED_SHIFT)?,
            g: channel(GREEN_SHIFT)?,
            b: channel(0)?,
        })
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{HEX_PREFIX}{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

impl Serialize for Rgb {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Refresh {
    pub metrics: PollInterval,
    pub prometheus: PollInterval,
    pub snapshots: SnapshotCap,
}

impl Default for Refresh {
    fn default() -> Self {
        Self {
            metrics: PollInterval::Seconds15,
            prometheus: PollInterval::Seconds60,
            snapshots: DEFAULT_SNAPSHOTS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PollInterval {
    #[serde(rename = "5s")]
    Seconds5,
    #[serde(rename = "15s")]
    Seconds15,
    #[serde(rename = "30s")]
    Seconds30,
    #[serde(rename = "60s")]
    Seconds60,
}

impl PollInterval {
    pub const ALL: [Self; 4] = [
        Self::Seconds5,
        Self::Seconds15,
        Self::Seconds30,
        Self::Seconds60,
    ];

    pub const fn duration(self) -> Duration {
        match self {
            Self::Seconds5 => Duration::from_secs(5),
            Self::Seconds15 => Duration::from_secs(15),
            Self::Seconds30 => Duration::from_secs(30),
            Self::Seconds60 => Duration::from_secs(60),
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Seconds5 => "5s",
            Self::Seconds15 => "15s",
            Self::Seconds30 => "30s",
            Self::Seconds60 => "60s",
        }
    }
}

pub const DEFAULT_SNAPSHOTS: SnapshotCap = SnapshotCap::Millis100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnapshotCap {
    #[serde(rename = "100ms")]
    Millis100,
    #[serde(rename = "250ms")]
    Millis250,
    #[serde(rename = "500ms")]
    Millis500,
    #[serde(rename = "1s")]
    Seconds1,
}

impl SnapshotCap {
    pub const ALL: [Self; 4] = [
        Self::Millis100,
        Self::Millis250,
        Self::Millis500,
        Self::Seconds1,
    ];

    pub const fn duration(self) -> Duration {
        match self {
            Self::Millis100 => Duration::from_millis(100),
            Self::Millis250 => Duration::from_millis(250),
            Self::Millis500 => Duration::from_millis(500),
            Self::Seconds1 => Duration::from_secs(1),
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Millis100 => "100ms",
            Self::Millis250 => "250ms",
            Self::Millis500 => "500ms",
            Self::Seconds1 => "1s",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notifications {
    pub enabled: bool,
    pub warnings: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            enabled: true,
            warnings: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Updates {
    pub automatic: bool,
    pub interval: CheckInterval,
    pub prereleases: bool,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            automatic: true,
            interval: CheckInterval::Hours6,
            prereleases: false,
        }
    }
}

const HOUR: u64 = 60 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckInterval {
    #[serde(rename = "1h")]
    Hours1,
    #[serde(rename = "6h")]
    Hours6,
    #[serde(rename = "24h")]
    Hours24,
}

impl CheckInterval {
    pub const ALL: [Self; 3] = [Self::Hours1, Self::Hours6, Self::Hours24];

    pub const fn duration(self) -> Duration {
        match self {
            Self::Hours1 => Duration::from_secs(HOUR),
            Self::Hours6 => Duration::from_secs(6 * HOUR),
            Self::Hours24 => Duration::from_secs(24 * HOUR),
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Hours1 => "1h",
            Self::Hours6 => "6h",
            Self::Hours24 => "24h",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_label_their_durations() {
        for interval in PollInterval::ALL {
            let seconds = interval.duration().as_secs();
            assert_eq!(interval.label(), format!("{seconds}s"));
        }
        assert_eq!(
            SnapshotCap::Millis500.duration(),
            Duration::from_millis(500)
        );
        assert_eq!(SnapshotCap::Seconds1.label(), "1s");
        for interval in CheckInterval::ALL {
            let hours = interval.duration().as_secs() / HOUR;
            assert_eq!(interval.label(), format!("{hours}h"));
        }
    }

    #[test]
    fn settings_round_trip_through_json() {
        let settings = AppSettings {
            show_in_menu_bar: false,
            theme: ThemeChoice::Dark,
            primary: Highlight::Custom(Rgb {
                r: 0x3a,
                g: 0x7b,
                b: 0xd5,
            }),
            secondary: Highlight::Teal,
            refresh: Refresh {
                metrics: PollInterval::Seconds5,
                prometheus: PollInterval::Seconds30,
                snapshots: SnapshotCap::Seconds1,
            },
            notifications: Notifications {
                enabled: false,
                warnings: true,
            },
            inspector_widths: BTreeMap::from([("resource-inspector".to_owned(), 640)]),
            updates: Updates {
                automatic: false,
                interval: CheckInterval::Hours24,
                prereleases: true,
            },
        };
        let json = serde_json::to_string(&settings).unwrap_or_default();
        assert!(json.contains("\"theme\":\"dark\""), "{json}");
        assert!(json.contains("\"metrics\":\"5s\""), "{json}");
        assert!(
            json.contains("\"primary\":{\"custom\":\"#3a7bd5\"}"),
            "{json}"
        );
        assert!(json.contains("\"secondary\":\"teal\""), "{json}");
        assert!(json.contains("\"interval\":\"24h\""), "{json}");
        let read: Result<AppSettings, _> = serde_json::from_str(&json);
        assert_eq!(read.ok(), Some(settings));
    }

    #[test]
    fn hex_colours_parse_with_or_without_the_hash() {
        let teal = Rgb {
            r: 0x14,
            g: 0x8a,
            b: 0x99,
        };
        assert_eq!("#148A99".parse(), Ok(teal));
        assert_eq!(" 148a99 ".parse(), Ok(teal));
        assert_eq!(teal.to_string(), "#148a99");
        assert_eq!(teal.to_u32(), 0x0014_8a99);
        for bad in ["", "#12345", "#1234567", "#12345g", "blue"] {
            assert_eq!(bad.parse::<Rgb>(), Err(BadHex), "{bad}");
        }
        let read: Result<AppSettings, _> = serde_json::from_str(r#"{"primary":{"custom":"nope"}}"#);
        assert!(read.is_err());
    }

    #[test]
    fn missing_fields_take_their_defaults() {
        let read: Result<AppSettings, _> = serde_json::from_str(r#"{"refresh":{"metrics":"30s"}}"#);
        let expected = AppSettings {
            refresh: Refresh {
                metrics: PollInterval::Seconds30,
                ..Refresh::default()
            },
            ..AppSettings::default()
        };
        assert_eq!(read.ok(), Some(expected));
        let updates = AppSettings::default().updates;
        assert!(AppSettings::default().show_in_menu_bar);
        assert!(updates.automatic);
        assert!(!updates.prereleases);
    }

    #[test]
    fn only_macos_with_the_menu_bar_enabled_keeps_running_without_windows() {
        let mut settings = AppSettings::default();
        assert_eq!(
            settings.keeps_running_in_menu_bar(),
            cfg!(target_os = "macos")
        );
        settings.show_in_menu_bar = false;
        assert!(!settings.keeps_running_in_menu_bar());
    }
}
