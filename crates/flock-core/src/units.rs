const MILLIS_PER_CORE: u32 = 1000;
const KIB: u64 = 1024;
const MIB: u64 = KIB * KIB;
const GIB: u64 = MIB * KIB;
const BYTES_PER_MIB: u64 = MIB;
const MILLIS_PER_CORE_F32: f32 = 1000.;
const GIB_F32: f32 = 1024. * 1024. * 1024.;
const MIB_PER_GIB_F32: f32 = 1024.;
const WHOLE_UNIT: f32 = 1.;
const MILLICORES: &str = "m";
const MEBIBYTES: &str = "Mi";
const BETWEEN: &str = " / ";

pub const CPU_UNIT: &str = "cores";
pub const MEMORY_UNIT: &str = "GiB";
pub const NOT_MEASURED: &str = "—";
pub const WAITING_FOR_METRICS: &str = "Waiting for metrics";

pub fn fixed(value: f32) -> String {
    format!("{value:.1}")
}

pub fn cpu_millis(millis: u32) -> String {
    if millis < MILLIS_PER_CORE {
        return format!("{millis}m");
    }
    let cores = f64::from(millis) / f64::from(MILLIS_PER_CORE);
    format!("{cores:.1} {CPU_UNIT}")
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn cpu_cores(cores: f32) -> String {
    let millis = (cores.max(0.) * MILLIS_PER_CORE_F32).round() as u32;
    cpu_millis(millis)
}

#[allow(clippy::cast_precision_loss)]
pub fn memory_bytes(bytes: u64) -> String {
    if bytes >= GIB {
        format!("{:.1} {MEMORY_UNIT}", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{} MiB", bytes / MIB)
    } else {
        format!("{} KiB", bytes / KIB)
    }
}

pub fn memory_mib(mib: u32) -> String {
    memory_bytes(u64::from(mib) * BYTES_PER_MIB)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn memory_gib(gib: f32) -> String {
    memory_bytes((gib.max(0.) * GIB_F32) as u64)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    Cpu,
    Memory,
}

impl Metric {
    const fn unit(self) -> &'static str {
        match self {
            Self::Cpu => CPU_UNIT,
            Self::Memory => MEMORY_UNIT,
        }
    }

    const fn small_unit(self) -> &'static str {
        match self {
            Self::Cpu => MILLICORES,
            Self::Memory => MEBIBYTES,
        }
    }

    const fn small_per_unit(self) -> f32 {
        match self {
            Self::Cpu => MILLIS_PER_CORE_F32,
            Self::Memory => MIB_PER_GIB_F32,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Amounts {
    pub figures: String,
    pub unit: Option<&'static str>,
}

impl Amounts {
    pub fn labelled(&self) -> String {
        match self.unit {
            Some(unit) => format!("{} {unit}", self.figures),
            None => self.figures.clone(),
        }
    }
}

pub fn amounts(metric: Metric, values: &[Option<f32>]) -> Amounts {
    let largest = values
        .iter()
        .flatten()
        .fold(0_f32, |most, &value| most.max(value));
    let small = largest < WHOLE_UNIT;
    let figures: Vec<String> = values
        .iter()
        .map(|value| match value {
            None => NOT_MEASURED.to_owned(),
            Some(value) if small => {
                let scaled = value.max(0.) * metric.small_per_unit();
                format!("{scaled:.0}{}", metric.small_unit())
            }
            Some(value) => fixed(*value),
        })
        .collect();
    Amounts {
        figures: figures.join(BETWEEN),
        unit: (!small).then_some(metric.unit()),
    }
}

pub fn times(count: u32) -> String {
    match count {
        1 => "once".into(),
        _ => format!("{count} times"),
    }
}

pub fn amount(used: f32, total: f32, unit: &str) -> String {
    format!("{} / {} {unit}", fixed(used), fixed(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_reads_in_millicores_below_a_core_and_cores_above() {
        assert_eq!(cpu_millis(4), "4m");
        assert_eq!(cpu_millis(212), "212m");
        assert_eq!(cpu_millis(1000), "1.0 cores");
        assert_eq!(cpu_millis(2500), "2.5 cores");
        assert_eq!(cpu_cores(0.026), "26m");
        assert_eq!(cpu_cores(0.), "0m");
        assert_eq!(cpu_cores(1.25), "1.2 cores");
    }

    #[test]
    fn memory_spells_its_binary_units() {
        assert_eq!(memory_bytes(512 * KIB), "512 KiB");
        assert_eq!(memory_bytes(38 * MIB), "38 MiB");
        assert_eq!(memory_bytes(301 * MIB + 5), "301 MiB");
        assert_eq!(memory_bytes(GIB), "1.0 GiB");
        assert_eq!(memory_bytes(GIB + GIB / 2), "1.5 GiB");
        assert_eq!(memory_mib(768), "768 MiB");
        assert_eq!(memory_mib(2048), "2.0 GiB");
        assert_eq!(memory_gib(0.25), "256 MiB");
    }

    #[test]
    fn small_amounts_read_in_millicores_and_mebibytes() {
        let cpu = amounts(Metric::Cpu, &[Some(0.12), Some(0.066)]);
        assert_eq!(cpu.figures, "120m / 66m");
        assert_eq!(cpu.labelled(), "120m / 66m");
        let memory = amounts(Metric::Memory, &[Some(0.25), Some(0.0625)]);
        assert_eq!(memory.figures, "256Mi / 64Mi");
        assert_eq!(amounts(Metric::Cpu, &[Some(0.004)]).figures, "4m");
    }

    #[test]
    fn amounts_from_one_unit_read_in_cores_and_gibibytes() {
        let cpu = amounts(Metric::Cpu, &[Some(1.25), Some(0.5)]);
        assert_eq!(cpu.figures, "1.2 / 0.5");
        assert_eq!(cpu.labelled(), "1.2 / 0.5 cores");
        let memory = amounts(Metric::Memory, &[Some(2.6), Some(8.)]);
        assert_eq!(memory.labelled(), "2.6 / 8.0 GiB");
    }

    #[test]
    fn unmeasured_amounts_read_as_a_dash() {
        let small = amounts(Metric::Cpu, &[None, Some(0.2)]);
        assert_eq!(small.figures, "— / 200m");
        let large = amounts(Metric::Memory, &[None, Some(3.)]);
        assert_eq!(large.figures, "— / 3.0");
    }

    #[test]
    fn repeat_counts_read_as_words() {
        assert_eq!(times(1), "once");
        assert_eq!(times(18), "18 times");
    }

    #[test]
    fn fixed_keeps_one_decimal() {
        assert_eq!(fixed(61.), "61.0");
        assert_eq!(fixed(0.), "0.0");
        assert_eq!(fixed(22.46), "22.5");
    }

    #[test]
    fn amounts_keep_one_decimal_on_both_sides() {
        assert_eq!(amount(1., 7.8, MEMORY_UNIT), "1.0 / 7.8 GiB");
        assert_eq!(amount(68.4, 112., CPU_UNIT), "68.4 / 112.0 cores");
        assert_eq!(amount(0., 0., MEMORY_UNIT), "0.0 / 0.0 GiB");
    }
}
