use std::fmt;
use std::time::Duration;

use flock::{Preset, Screen};
use flock_core::pods::Pod;
use flock_core::resources::POD_LIST;
use flock_core::settings::NamespaceSelection;
use flock_core::{ClusterId, Command, Snapshot, Uid};

const POD_COUNTS: [usize; 3] = [1_000, 5_000, 20_000];

pub struct Workload {
    pub pods: usize,
}

impl fmt::Display for Workload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.pods)
    }
}

pub fn workloads() -> impl Iterator<Item = Workload> {
    POD_COUNTS.into_iter().map(|pods| Workload { pods })
}

pub fn viewed_cluster() -> Option<ClusterId> {
    Screen::Pods
        .commands(&Preset::default())
        .into_iter()
        .find_map(|command| match command {
            Command::SelectCluster(cluster) => cluster,
            _ => None,
        })
}

pub fn scaled_snapshot(cluster: &ClusterId, pods: usize) -> Snapshot {
    let mut snapshot = flock_demo::snapshot();
    let (templates, others): (Vec<Pod>, Vec<Pod>) = std::mem::take(&mut snapshot.pods)
        .into_iter()
        .partition(|pod| &pod.cluster == cluster);
    let scaled = templates
        .iter()
        .cycle()
        .take(pods)
        .enumerate()
        .map(|(ix, template)| numbered(template, ix));
    snapshot.pods = scaled.chain(others).collect();
    let settings = snapshot
        .catalog
        .clusters
        .iter_mut()
        .filter(|settings| &settings.cluster == cluster);
    for settings in settings {
        settings
            .preferences
            .select_namespace(POD_LIST, NamespaceSelection::All);
    }
    snapshot
}

fn numbered(template: &Pod, ix: usize) -> Pod {
    let mut pod = template.clone();
    pod.name = format!("{}-{ix:05}", template.name);
    pod.uid = Uid::new(pod.name.clone());
    pod
}

pub struct Aged {
    pub pod: usize,
    pub age: Duration,
}

pub fn age_one_pod(snapshot: &mut Snapshot, generation: u64) -> Option<Aged> {
    let count = u64::try_from(snapshot.pods.len()).ok()?;
    let ix = usize::try_from(generation.checked_rem(count)?).ok()?;
    let pod = snapshot.pods.get_mut(ix)?;
    pod.age += Duration::from_secs(generation + 1);
    Some(Aged {
        pod: ix,
        age: pod.age,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    const PODS: usize = 2_500;

    fn cluster() -> ClusterId {
        let Some(cluster) = viewed_cluster() else {
            panic!("the Pods screen opens without a cluster");
        };
        cluster
    }

    #[test]
    fn the_viewed_cluster_shows_exactly_the_requested_unique_pods_in_all_namespaces() {
        let cluster = cluster();
        let demo = flock_demo::snapshot();
        let snapshot = scaled_snapshot(&cluster, PODS);
        let viewed: Vec<&Pod> = snapshot
            .pods
            .iter()
            .filter(|pod| pod.cluster == cluster)
            .collect();
        let names: BTreeSet<&str> = viewed.iter().map(|pod| pod.name.as_str()).collect();
        let others = |snapshot: &Snapshot| {
            snapshot
                .pods
                .iter()
                .filter(|pod| pod.cluster != cluster)
                .count()
        };
        assert_eq!(viewed.len(), PODS);
        assert_eq!(names.len(), PODS);
        assert_eq!(others(&snapshot), others(&demo));
        let all = snapshot
            .catalog
            .clusters
            .iter()
            .filter(|settings| settings.cluster == cluster)
            .map(|settings| settings.preferences.namespace_selection(POD_LIST))
            .collect::<Vec<_>>();
        assert_eq!(all, vec![NamespaceSelection::All]);
    }

    #[test]
    fn ageing_touches_one_pod_and_moves_on_each_generation() {
        let mut snapshot = scaled_snapshot(&cluster(), PODS);
        let before = snapshot.clone();
        let Some(first) = age_one_pod(&mut snapshot, 0) else {
            panic!("no pod to age");
        };
        let Some(second) = age_one_pod(&mut snapshot, 1) else {
            panic!("no pod to age");
        };
        let changed = snapshot
            .pods
            .iter()
            .zip(&before.pods)
            .filter(|(after, before)| after != before)
            .count();
        assert_ne!(first.pod, second.pod);
        assert_eq!(changed, 2);
        assert_eq!(snapshot.pods[second.pod].age, second.age);
        assert!(age_one_pod(&mut Snapshot::default(), 0).is_none());
    }
}
