use jiff::civil::{Date, date};

use crate::node_pools::Version;
use crate::{ResourceRef, Status};

pub const SOON_DAYS: i64 = 14;
pub const SUPPORT_SOURCE: &str = "https://endoflife.date/kubernetes";
const SUPPORTED_MAJOR: u32 = 1;
const UPSTREAM: &str = "Kubernetes";
const EKS: &str = "EKS";
const GKE: &str = "GKE";
const AKS: &str = "AKS";
const STANDARD: &str = "standard support";
const EXTENDED: &str = "extended support";
const SUPPORT: &str = "support";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Horizon {
    Week,
    TwoWeeks,
    #[default]
    Month,
    Quarter,
}

impl Horizon {
    pub const ALL: [Self; 4] = [Self::Week, Self::TwoWeeks, Self::Month, Self::Quarter];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Week => "7d",
            Self::TwoWeeks => "14d",
            Self::Month => "30d",
            Self::Quarter => "90d",
        }
    }

    pub const fn days(self) -> i64 {
        match self {
            Self::Week => 7,
            Self::TwoWeeks => 14,
            Self::Month => 30,
            Self::Quarter => 90,
        }
    }

    pub const fn covers(self, days: i64) -> bool {
        days <= self.days()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expiring {
    pub label: String,
    pub detail: String,
    pub days: Option<i64>,
    pub ready: bool,
    pub target: Option<ResourceRef>,
}

impl Expiring {
    pub fn shown(&self, horizon: Horizon) -> bool {
        !self.ready || self.days.is_some_and(|days| horizon.covers(days))
    }

    pub fn status(&self) -> Status {
        expiry_status(self.days, self.ready)
    }
}

pub fn expiry_status(days: Option<i64>, ready: bool) -> Status {
    match days {
        Some(days) if days < 0 => Status::Err,
        Some(_) if !ready => Status::Warn,
        Some(days) if days <= SOON_DAYS => Status::Warn,
        Some(_) => Status::Offline,
        None if ready => Status::Offline,
        None => Status::Warn,
    }
}

pub fn soonest_first(items: &mut [Expiring]) {
    items.sort_by(|a, b| {
        let key = |item: &Expiring| (item.ready, item.days.unwrap_or(i64::MAX));
        key(a).cmp(&key(b)).then_with(|| a.label.cmp(&b.label))
    });
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SupportTrack {
    #[default]
    Upstream,
    Eks,
    Gke,
    Aks,
}

impl SupportTrack {
    const fn name(self) -> &'static str {
        match self {
            Self::Upstream => UPSTREAM,
            Self::Eks => EKS,
            Self::Gke => GKE,
            Self::Aks => AKS,
        }
    }

    fn windows(self) -> &'static [Window] {
        match self {
            Self::Upstream => &UPSTREAM_WINDOWS,
            Self::Eks => &EKS_WINDOWS,
            Self::Gke => &GKE_WINDOWS,
            Self::Aks => &AKS_WINDOWS,
        }
    }

    fn phases(self, window: &Window) -> Vec<(&'static str, Date)> {
        match (self, window.extended) {
            (Self::Upstream, _) => vec![(SUPPORT, window.standard)],
            (_, Some(extended)) => vec![(STANDARD, window.standard), (EXTENDED, extended)],
            (_, None) => vec![(STANDARD, window.standard)],
        }
    }
}

struct Window {
    minor: u32,
    standard: Date,
    extended: Option<Date>,
}

const fn upstream(minor: u32, end: Date) -> Window {
    Window {
        minor,
        standard: end,
        extended: None,
    }
}

const fn vendor(minor: u32, standard: Date, extended: Date) -> Window {
    Window {
        minor,
        standard,
        extended: Some(extended),
    }
}

const UPSTREAM_WINDOWS: [Window; 10] = [
    upstream(28, date(2024, 10, 28)),
    upstream(29, date(2025, 2, 28)),
    upstream(30, date(2025, 7, 15)),
    upstream(31, date(2025, 11, 11)),
    upstream(32, date(2026, 2, 28)),
    upstream(33, date(2026, 6, 28)),
    upstream(34, date(2026, 10, 27)),
    upstream(35, date(2027, 2, 28)),
    upstream(36, date(2027, 6, 28)),
    upstream(37, date(2027, 10, 28)),
];

const EKS_WINDOWS: [Window; 10] = [
    vendor(28, date(2024, 11, 26), date(2025, 11, 26)),
    vendor(29, date(2025, 3, 23), date(2026, 3, 23)),
    vendor(30, date(2025, 7, 23), date(2026, 7, 23)),
    vendor(31, date(2025, 11, 26), date(2026, 11, 26)),
    vendor(32, date(2026, 3, 23), date(2027, 3, 23)),
    vendor(33, date(2026, 7, 29), date(2027, 7, 29)),
    vendor(34, date(2026, 12, 2), date(2027, 12, 2)),
    vendor(35, date(2027, 3, 27), date(2028, 3, 27)),
    vendor(36, date(2027, 8, 2), date(2028, 8, 2)),
    vendor(37, date(2027, 12, 1), date(2028, 12, 1)),
];

const GKE_WINDOWS: [Window; 9] = [
    vendor(28, date(2024, 12, 31), date(2025, 2, 4)),
    vendor(29, date(2025, 2, 28), date(2025, 4, 12)),
    vendor(30, date(2025, 7, 31), date(2025, 9, 30)),
    vendor(31, date(2025, 11, 30), date(2026, 1, 16)),
    vendor(32, date(2026, 2, 28), date(2026, 4, 11)),
    vendor(33, date(2026, 6, 30), date(2026, 8, 3)),
    vendor(34, date(2026, 8, 30), date(2026, 10, 1)),
    vendor(35, date(2027, 2, 28), date(2027, 4, 11)),
    vendor(36, date(2027, 6, 30), date(2027, 8, 9)),
];

const AKS_WINDOWS: [Window; 9] = [
    vendor(28, date(2025, 1, 31), date(2026, 2, 28)),
    vendor(29, date(2025, 3, 31), date(2026, 4, 30)),
    vendor(30, date(2025, 8, 22), date(2026, 7, 31)),
    vendor(31, date(2025, 11, 1), date(2026, 11, 30)),
    vendor(32, date(2026, 3, 31), date(2027, 3, 31)),
    vendor(33, date(2026, 7, 31), date(2027, 7, 31)),
    vendor(34, date(2026, 11, 30), date(2027, 11, 30)),
    vendor(35, date(2027, 3, 31), date(2028, 3, 31)),
    vendor(36, date(2027, 6, 30), date(2028, 6, 30)),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupportEnd {
    pub label: String,
    pub date: Date,
    pub days: i64,
}

impl SupportEnd {
    pub fn of(track: SupportTrack, version: &str, today: Date) -> Option<Self> {
        let version = Version::parse(version).filter(|v| v.major == SUPPORTED_MAJOR)?;
        let window = track
            .windows()
            .iter()
            .find(|window| window.minor == version.minor)?;
        let phases = track.phases(window);
        let (phase, end) = phases
            .iter()
            .copied()
            .find(|(_, end)| *end >= today)
            .or_else(|| phases.last().copied())?;
        let days = today.until(end).ok()?.get_days().into();
        let verb = if end >= today { "ends" } else { "ended" };
        let minor = version.minor_label();
        let label = format!("{} {minor} {phase} {verb}", track.name());
        Some(Self {
            label,
            date: end,
            days,
        })
    }

    pub fn shown(&self, horizon: Horizon) -> bool {
        horizon.covers(self.days)
    }

    pub fn status(&self) -> Status {
        if self.days < 0 {
            Status::Err
        } else {
            Status::Warn
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cert(label: &str, days: Option<i64>, ready: bool) -> Expiring {
        Expiring {
            label: label.into(),
            detail: String::new(),
            days,
            ready,
            target: None,
        }
    }

    #[test]
    fn horizons_cover_their_days() {
        assert!(Horizon::Week.covers(7));
        assert!(!Horizon::Week.covers(8));
        assert!(Horizon::Quarter.covers(-3));
        assert_eq!(Horizon::default(), Horizon::Month);
        let labels: Vec<&str> = Horizon::ALL.iter().map(|h| h.label()).collect();
        assert_eq!(labels, ["7d", "14d", "30d", "90d"]);
    }

    #[test]
    fn not_ready_certificates_show_at_any_horizon() {
        assert!(!cert("a", Some(41), true).shown(Horizon::Month));
        assert!(cert("a", Some(23), true).shown(Horizon::Month));
        assert!(cert("a", Some(41), false).shown(Horizon::Week));
        assert!(cert("a", None, false).shown(Horizon::Week));
        assert!(!cert("a", None, true).shown(Horizon::Quarter));
    }

    #[test]
    fn status_follows_the_design_thresholds() {
        assert_eq!(cert("a", Some(9), true).status(), Status::Warn);
        assert_eq!(cert("a", Some(14), true).status(), Status::Warn);
        assert_eq!(cert("a", Some(23), true).status(), Status::Offline);
        assert_eq!(cert("a", Some(23), false).status(), Status::Warn);
        assert_eq!(cert("a", Some(-1), true).status(), Status::Err);
        assert_eq!(cert("a", None, false).status(), Status::Warn);
    }

    #[test]
    fn soonest_first_puts_not_ready_then_days() {
        let mut items = vec![
            cert("late", Some(60), true),
            cert("soon", Some(3), true),
            cert("broken", Some(40), false),
        ];
        soonest_first(&mut items);
        let order: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(order, ["broken", "soon", "late"]);
    }

    #[test]
    fn eks_moves_from_standard_to_extended_support() {
        let today = date(2026, 10, 20);
        let end = SupportEnd::of(SupportTrack::Eks, "v1.31.2-eks-1234", today);
        assert_eq!(
            end,
            Some(SupportEnd {
                label: "EKS v1.31 extended support ends".into(),
                date: date(2026, 11, 26),
                days: 37,
            })
        );
        let standard = SupportEnd::of(SupportTrack::Eks, "v1.34.1", today);
        assert_eq!(
            standard.map(|end| (end.label, end.days)),
            Some(("EKS v1.34 standard support ends".into(), 43))
        );
    }

    #[test]
    fn upstream_end_of_life_and_past_dates() {
        let today = date(2026, 10, 20);
        let upstream = SupportEnd::of(SupportTrack::Upstream, "v1.34.0", today);
        assert_eq!(
            upstream.as_ref().map(|end| (end.label.as_str(), end.days)),
            Some(("Kubernetes v1.34 support ends", 7))
        );
        assert_eq!(upstream.map(|end| end.status()), Some(Status::Warn));
        let ended = SupportEnd::of(SupportTrack::Gke, "1.31.4-gke.100", today);
        assert_eq!(
            ended.as_ref().map(|end| (end.label.as_str(), end.days)),
            Some(("GKE v1.31 extended support ended", -277))
        );
        assert_eq!(ended.map(|end| end.status()), Some(Status::Err));
    }

    #[test]
    fn unknown_versions_have_no_support_end() {
        let today = date(2026, 10, 20);
        assert_eq!(
            SupportEnd::of(SupportTrack::Upstream, "v1.12.0", today),
            None
        );
        assert_eq!(SupportEnd::of(SupportTrack::Aks, "v1.37.0", today), None);
        assert_eq!(
            SupportEnd::of(SupportTrack::Upstream, "garbage", today),
            None
        );
        assert_eq!(
            SupportEnd::of(SupportTrack::Upstream, "v2.0.0", today),
            None
        );
    }
}
