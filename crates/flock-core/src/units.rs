const MILLIS_PER_CORE: f64 = 1000.;
const MICROS_PER_CORE: f64 = 1_000_000.;
const NANOS_PER_CORE: f64 = 1_000_000_000.;
const SMALLEST_FIGURE: f32 = 0.01;
const BELOW_SMALLEST: &str = "<0.01";
const BYTES_PER_MIB: u64 = 1024 * 1024;
const BETWEEN: &str = " / ";
const WHOLE_CORE: f64 = 1.;
const CPU_SUFFIXES: [(char, f64); 3] = [
    ('m', MILLIS_PER_CORE),
    ('u', MICROS_PER_CORE),
    ('n', NANOS_PER_CORE),
];

pub const CPU_UNIT: &str = "cores";
pub const MEMORY_UNIT: &str = "GiB";
pub const POD_MEMORY_UNIT: &str = "MiB";
pub const NOT_MEASURED: &str = "—";
pub const WAITING_FOR_METRICS: &str = "Waiting for metrics";

pub fn figure(value: f32) -> String {
    let value = value.max(0.);
    if value > 0. && value < SMALLEST_FIGURE {
        return BELOW_SMALLEST.to_owned();
    }
    format!("{value:.2}")
}

#[allow(clippy::cast_possible_truncation)]
pub fn millicores(millis: u32) -> String {
    let cores = f64::from(millis) / MILLIS_PER_CORE;
    figure(cores as f32)
}

pub fn mebibytes(bytes: u64) -> String {
    (bytes / BYTES_PER_MIB).to_string()
}

pub fn against(used: Option<f32>, requested: f32) -> String {
    let used_nothing = used.is_none_or(|used| used <= 0.);
    if requested <= 0. && used_nothing {
        return NOT_MEASURED.to_owned();
    }
    let used = used.map_or_else(|| NOT_MEASURED.to_owned(), figure);
    let requested = if requested > 0. {
        figure(requested)
    } else {
        NOT_MEASURED.to_owned()
    };
    format!("{used}{BETWEEN}{requested}")
}

pub fn amount(used: f32, total: f32, unit: &str) -> String {
    format!("{}{BETWEEN}{} {unit}", figure(used), figure(total))
}

#[allow(clippy::cast_possible_truncation)]
pub fn cpu_quantity(text: &str) -> Option<f32> {
    let text = text.trim();
    let (number, per_core) = CPU_SUFFIXES
        .iter()
        .find_map(|&(suffix, per_core)| text.strip_suffix(suffix).map(|n| (n, per_core)))
        .unwrap_or((text, WHOLE_CORE));
    let value: f64 = number.parse().ok()?;
    (value.is_finite() && value >= 0.).then_some((value / per_core) as f32)
}

pub fn written_cpu(text: &str) -> String {
    cpu_quantity(text).map_or_else(|| text.to_owned(), figure)
}

pub fn times(count: u32) -> String {
    match count {
        1 => "once".into(),
        _ => format!("{count} times"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn figures_keep_two_decimals() {
        assert_eq!(figure(0.2), "0.20");
        assert_eq!(figure(1.), "1.00");
        assert_eq!(figure(0.028), "0.03");
        assert_eq!(figure(68.4), "68.40");
        assert_eq!(figure(0.), "0.00");
        assert_eq!(figure(-1.), "0.00");
    }

    #[test]
    fn figures_below_a_hundredth_read_as_less_than() {
        assert_eq!(figure(0.004), "<0.01");
        assert_eq!(figure(0.0099), "<0.01");
        assert_eq!(figure(0.01), "0.01");
    }

    #[test]
    fn millicores_read_in_cores() {
        assert_eq!(millicores(3), "<0.01");
        assert_eq!(millicores(28), "0.03");
        assert_eq!(millicores(350), "0.35");
        assert_eq!(millicores(1500), "1.50");
        assert_eq!(millicores(0), "0.00");
    }

    #[test]
    fn pod_memory_reads_in_whole_mebibytes() {
        assert_eq!(mebibytes(38 * BYTES_PER_MIB), "38");
        assert_eq!(mebibytes(2048 * BYTES_PER_MIB + 5), "2048");
    }

    #[test]
    fn used_against_requested_shares_one_unit() {
        assert_eq!(against(Some(0.003), 0.001), "<0.01 / <0.01");
        assert_eq!(against(Some(0.2), 1.), "0.20 / 1.00");
        assert_eq!(against(Some(0.028), 0.35), "0.03 / 0.35");
        assert_eq!(against(None, 0.2), "— / 0.20");
    }

    #[test]
    fn nothing_requested_reads_as_a_dash() {
        assert_eq!(against(Some(0.), 0.), "—");
        assert_eq!(against(None, 0.), "—");
        assert_eq!(against(Some(0.2), 0.), "0.20 / —");
    }

    #[test]
    fn cpu_quantities_parse_to_cores() {
        assert_eq!(cpu_quantity("100m"), Some(0.1));
        assert_eq!(cpu_quantity("2"), Some(2.));
        assert_eq!(cpu_quantity("0.5"), Some(0.5));
        assert_eq!(cpu_quantity("250000u"), Some(0.25));
        assert_eq!(cpu_quantity("500000000n"), Some(0.5));
        assert_eq!(cpu_quantity("lots"), None);
        assert_eq!(cpu_quantity(""), None);
    }

    #[test]
    fn written_cpu_reads_in_cores_or_as_written() {
        assert_eq!(written_cpu("100m"), "0.10");
        assert_eq!(written_cpu("1"), "1.00");
        assert_eq!(written_cpu("lots"), "lots");
    }

    #[test]
    fn repeat_counts_read_as_words() {
        assert_eq!(times(1), "once");
        assert_eq!(times(18), "18 times");
    }

    #[test]
    fn amounts_keep_two_decimals_on_both_sides() {
        assert_eq!(amount(1., 7.8, MEMORY_UNIT), "1.00 / 7.80 GiB");
        assert_eq!(amount(68.4, 112., CPU_UNIT), "68.40 / 112.00 cores");
        assert_eq!(amount(0., 0., MEMORY_UNIT), "0.00 / 0.00 GiB");
    }
}
