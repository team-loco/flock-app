pub mod live;
pub mod tables;

use std::time::Duration;

use flock::{Preset, Screen};
use flock_core::{ClusterId, Command};

pub const SAMPLES: usize = 10;
pub const WARM_UP: Duration = Duration::from_secs(1);
pub const MEASUREMENT: Duration = Duration::from_secs(3);

pub fn viewed_cluster() -> Option<ClusterId> {
    Screen::Pods
        .commands(&Preset::default())
        .into_iter()
        .find_map(|command| match command {
            Command::SelectCluster(cluster) => cluster,
            _ => None,
        })
}
