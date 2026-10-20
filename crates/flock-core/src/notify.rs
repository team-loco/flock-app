use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::app_settings::Notifications;
use crate::home::Problem;
use crate::{ClusterId, ResourceRef, Snapshot, Status};

pub const COOLDOWN: Duration = Duration::from_mins(30);
pub const CONNECT_GRACE: Duration = Duration::from_secs(30);
const SEPARATOR: &str = " · ";
const TAG_PREFIX: &str = "flock-problem";

#[derive(Debug, Default)]
pub struct Notifier {
    clusters: HashMap<ClusterId, Instant>,
    last_seen: HashMap<ResourceRef, Instant>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alert {
    pub tag: String,
    pub title: String,
    pub body: String,
    pub target: ResourceRef,
}

impl Notifier {
    pub fn observe(
        &mut self,
        snapshot: &Snapshot,
        settings: Notifications,
        now: Instant,
    ) -> Vec<Alert> {
        for cluster in snapshot.clusters.iter().filter(|c| c.is_connected()) {
            self.clusters.entry(cluster.id.clone()).or_insert(now);
        }
        let mut alerts = Vec::new();
        for problem in &snapshot.home.problems {
            if !wanted(problem.status, settings) {
                continue;
            }
            let previous = self.last_seen.insert(problem.target.clone(), now);
            let fresh = previous.is_none_or(|seen| now.duration_since(seen) >= COOLDOWN);
            if fresh
                && settings.enabled
                && !settings.paused
                && self.settled(&problem.target.cluster, now)
            {
                let muted = snapshot
                    .catalog
                    .get(&problem.target.cluster)
                    .is_some_and(|c| !c.preferences.notify);
                if !muted {
                    alerts.push(alert(problem));
                }
            }
        }
        self.last_seen
            .retain(|_, seen| now.duration_since(*seen) < COOLDOWN);
        alerts
    }

    fn settled(&self, cluster: &ClusterId, now: Instant) -> bool {
        self.clusters
            .get(cluster)
            .is_some_and(|first| now.duration_since(*first) >= CONNECT_GRACE)
    }
}

fn wanted(status: Status, settings: Notifications) -> bool {
    match status {
        Status::Err => true,
        Status::Warn => settings.warnings,
        Status::Ok => false,
        Status::Progress => false,
        Status::Suspended => false,
        Status::Offline => false,
    }
}

fn alert(problem: &Problem) -> Alert {
    let target = &problem.target;
    let namespace = target.namespace.as_deref().unwrap_or_default();
    Alert {
        tag: format!(
            "{TAG_PREFIX}/{}/{}/{namespace}/{}",
            target.cluster, target.kind, target.name
        ),
        title: format!("{}{SEPARATOR}{}", target.name, target.cluster),
        body: problem.message.clone(),
        target: target.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Preferences;
    use crate::flock::tests::sample;
    use crate::settings::{Auth, ClusterSettings, Connection};

    fn problem(name: &str, status: Status) -> Problem {
        Problem {
            status,
            target: ResourceRef {
                cluster: ClusterId::new("a"),
                kind: "Pod".into(),
                namespace: Some("shop".into()),
                name: name.into(),
            },
            message: "CrashLoopBackOff".into(),
            age: Duration::ZERO,
        }
    }

    fn with(problems: Vec<Problem>) -> Snapshot {
        let mut snapshot = sample();
        snapshot.home.problems = problems;
        snapshot
    }

    const NONE: [&str; 0] = [];

    fn names(alerts: &[Alert]) -> Vec<&str> {
        alerts.iter().map(|a| a.target.name.as_str()).collect()
    }

    fn settled(notifier: &mut Notifier, start: Instant) -> Instant {
        notifier.observe(&with(Vec::new()), Notifications::default(), start);
        start + CONNECT_GRACE
    }

    #[test]
    fn problems_present_while_a_cluster_connects_are_not_announced() {
        let mut notifier = Notifier::default();
        let start = Instant::now();
        let crashing = with(vec![problem("cart", Status::Err)]);
        let alerts = notifier.observe(&crashing, Notifications::default(), start);
        assert_eq!(names(&alerts), NONE);
        let later = start + CONNECT_GRACE;
        assert_eq!(
            names(&notifier.observe(&crashing, Notifications::default(), later)),
            NONE
        );
    }

    #[test]
    fn a_new_error_is_announced_once_and_a_flapping_pod_stays_quiet() {
        let mut notifier = Notifier::default();
        let now = settled(&mut notifier, Instant::now());
        let crashing = with(vec![problem("cart", Status::Err)]);
        let alerts = notifier.observe(&crashing, Notifications::default(), now);
        assert_eq!(names(&alerts), ["cart"]);
        assert_eq!(alerts[0].title, "cart · a");
        assert_eq!(alerts[0].tag, "flock-problem/a/Pod/shop/cart");
        let calm = with(Vec::new());
        let step = Duration::from_secs(60);
        assert_eq!(
            names(&notifier.observe(&calm, Notifications::default(), now + step)),
            NONE
        );
        let again = notifier.observe(&crashing, Notifications::default(), now + step * 2);
        assert_eq!(names(&again), NONE);
        let much_later = now + step * 2 + COOLDOWN;
        assert_eq!(
            names(&notifier.observe(&calm, Notifications::default(), much_later)),
            NONE
        );
        let returned = notifier.observe(&crashing, Notifications::default(), much_later + COOLDOWN);
        assert_eq!(names(&returned), ["cart"]);
    }

    #[test]
    fn warnings_need_the_option_and_disabled_notifications_stay_silent() {
        let mut notifier = Notifier::default();
        let now = settled(&mut notifier, Instant::now());
        let warning = with(vec![problem("db", Status::Warn)]);
        assert_eq!(
            names(&notifier.observe(&warning, Notifications::default(), now)),
            NONE
        );
        let with_warnings = Notifications {
            warnings: true,
            ..Notifications::default()
        };
        let shown = notifier.observe(&warning, with_warnings, now);
        assert_eq!(names(&shown), ["db"]);
        let off = Notifications {
            enabled: false,
            warnings: true,
            paused: false,
        };
        let crashing = with(vec![problem("cart", Status::Err)]);
        assert_eq!(names(&notifier.observe(&crashing, off, now)), NONE);
        assert_eq!(
            names(&notifier.observe(&crashing, Notifications::default(), now)),
            NONE
        );
    }

    #[test]
    fn paused_notifications_stay_silent_and_do_not_replay_on_resume() {
        let mut notifier = Notifier::default();
        let now = settled(&mut notifier, Instant::now());
        let paused = Notifications {
            paused: true,
            ..Notifications::default()
        };
        let crashing = with(vec![problem("cart", Status::Err)]);
        assert_eq!(names(&notifier.observe(&crashing, paused, now)), NONE);
        assert_eq!(
            names(&notifier.observe(&crashing, Notifications::default(), now)),
            NONE
        );
        let fresh = with(vec![
            problem("cart", Status::Err),
            problem("db", Status::Err),
        ]);
        assert_eq!(
            names(&notifier.observe(&fresh, Notifications::default(), now)),
            ["db"]
        );
    }

    #[test]
    fn muted_clusters_are_skipped() {
        let mut notifier = Notifier::default();
        let now = settled(&mut notifier, Instant::now());
        let mut crashing = with(vec![problem("cart", Status::Err)]);
        crashing.catalog.clusters.push(ClusterSettings {
            context_change: None,
            cluster: ClusterId::new("a"),
            connection: Connection {
                context: "a".into(),
                server: String::new(),
                auth: Auth::Other("token".into()),
                kubeconfig: String::new(),
            },
            auth_expired: false,
            prometheus: None,
            preferences: Preferences {
                notify: false,
                ..Preferences::default()
            },
        });
        assert_eq!(
            names(&notifier.observe(&crashing, Notifications::default(), now)),
            NONE
        );
    }
}
