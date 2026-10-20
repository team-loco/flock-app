use std::time::Duration;

use flock_core::app_settings::Refresh;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KubeSettings {
    pub shell_env_timeout: Duration,
    pub connect_timeout: Duration,
    pub check_timeout: Duration,
    pub health_interval: Duration,
    pub backoff_initial: Duration,
    pub backoff_max: Duration,
    pub discovery_debounce: Duration,
    pub git_timeout: Duration,
    pub metrics_interval: Duration,
    pub prometheus_interval: Duration,
    pub control_plane_interval: Duration,
    pub snapshot_interval: Duration,
    pub event_limit: usize,
    pub workload_refresh: Duration,
}

impl KubeSettings {
    pub fn with_refresh(refresh: &Refresh) -> Self {
        let mut settings = Self::default();
        settings.apply_refresh(refresh);
        settings
    }

    pub fn apply_refresh(&mut self, refresh: &Refresh) {
        self.metrics_interval = refresh.metrics.duration();
        self.prometheus_interval = refresh.prometheus.duration();
        self.snapshot_interval = refresh.snapshots.duration();
    }
}

impl Default for KubeSettings {
    fn default() -> Self {
        let refresh = Refresh::default();
        Self {
            shell_env_timeout: Duration::from_secs(5),
            connect_timeout: Duration::from_secs(10),
            check_timeout: Duration::from_secs(15),
            health_interval: Duration::from_secs(30),
            backoff_initial: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            discovery_debounce: Duration::from_millis(300),
            git_timeout: Duration::from_secs(120),
            metrics_interval: refresh.metrics.duration(),
            prometheus_interval: refresh.prometheus.duration(),
            control_plane_interval: Duration::from_secs(30),
            snapshot_interval: refresh.snapshots.duration(),
            event_limit: 1000,
            workload_refresh: Duration::from_secs(2),
        }
    }
}

#[cfg(test)]
mod tests {
    use flock_core::app_settings::{PollInterval, SnapshotCap};

    use super::*;

    #[test]
    fn refresh_presets_set_the_polling_and_snapshot_intervals() {
        let refresh = Refresh {
            metrics: PollInterval::Seconds5,
            prometheus: PollInterval::Seconds30,
            snapshots: SnapshotCap::Seconds1,
        };
        let settings = KubeSettings::with_refresh(&refresh);
        assert_eq!(settings.metrics_interval, Duration::from_secs(5));
        assert_eq!(settings.prometheus_interval, Duration::from_secs(30));
        assert_eq!(settings.snapshot_interval, Duration::from_secs(1));
        assert_eq!(
            KubeSettings::with_refresh(&Refresh::default()),
            KubeSettings::default()
        );
    }
}
