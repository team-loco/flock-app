use std::time::{Duration, SystemTime};

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;
const YEAR: u64 = 365 * DAY;

pub fn age(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    match secs {
        s if s < MINUTE => format!("{s}s"),
        s if s < HOUR => format!("{}m", s / MINUTE),
        s if s < DAY => format!("{}h", s / HOUR),
        s if s < YEAR => format!("{}d", s / DAY),
        s => format!("{}y", s / YEAR),
    }
}

pub fn span(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    let (major, major_unit, minor, minor_unit) = match secs {
        s if s < MINUTE => return format!("{s}s"),
        s if s < HOUR => (s / MINUTE, "m", s % MINUTE, "s"),
        s if s < DAY => (s / HOUR, "h", s % HOUR / MINUTE, "m"),
        s => (s / DAY, "d", s % DAY / HOUR, "h"),
    };
    match minor {
        0 => format!("{major}{major_unit}"),
        _ => format!("{major}{major_unit} {minor}{minor_unit}"),
    }
}

pub fn age_since(then: SystemTime, now: SystemTime) -> String {
    let elapsed = now.duration_since(then).unwrap_or_default();
    age(elapsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_largest_whole_unit() {
        let cases = [
            (0, "0s"),
            (4, "4s"),
            (59, "59s"),
            (60, "1m"),
            (150, "2m"),
            (HOUR - 1, "59m"),
            (3 * HOUR + 59 * MINUTE, "3h"),
            (DAY, "1d"),
            (214 * DAY, "214d"),
            (YEAR - 1, "364d"),
            (2 * YEAR + DAY, "2y"),
        ];
        for (secs, expected) in cases {
            assert_eq!(age(Duration::from_secs(secs)), expected, "{secs}s");
        }
    }

    #[test]
    fn spans_show_the_two_largest_units() {
        let cases = [
            (27, "27s"),
            (3 * MINUTE + 12, "3m 12s"),
            (6 * HOUR + 6 * MINUTE + 40, "6h 6m"),
            (2 * HOUR, "2h"),
            (2 * DAY + 4 * HOUR, "2d 4h"),
        ];
        for (secs, expected) in cases {
            assert_eq!(span(Duration::from_secs(secs)), expected, "{secs}s");
        }
    }

    #[test]
    fn future_timestamps_read_as_zero() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(DAY);
        let later = now + Duration::from_secs(90);
        assert_eq!(age_since(later, now), "0s");
        assert_eq!(age_since(now - Duration::from_secs(90), now), "1m");
    }
}
