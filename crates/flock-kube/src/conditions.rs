use flock_core::Condition;
use jiff::Timestamp;
use k8s_openapi::api::apps::v1::{DaemonSetCondition, DeploymentCondition, StatefulSetCondition};
use k8s_openapi::api::batch::v1::JobCondition;
use k8s_openapi::api::core::v1::{NodeCondition, PersistentVolumeClaimCondition, PodCondition};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;

use crate::clock::between;

pub trait KubeCondition {
    fn core(&self, now: Timestamp) -> Condition;
}

fn build(
    kind: &str,
    value: &str,
    reason: Option<&String>,
    message: Option<&String>,
    changed: Option<&Time>,
    now: Timestamp,
) -> Condition {
    let age = changed.map(|time| between(time.0, now));
    Condition::new(kind, value)
        .reason(reason.cloned().unwrap_or_default())
        .message(message.cloned().unwrap_or_default())
        .age(age)
}

macro_rules! kube_condition {
    ($type:ty) => {
        impl KubeCondition for $type {
            fn core(&self, now: Timestamp) -> Condition {
                build(
                    &self.type_,
                    &self.status,
                    self.reason.as_ref(),
                    self.message.as_ref(),
                    self.last_transition_time.as_ref(),
                    now,
                )
            }
        }
    };
}

kube_condition!(DeploymentCondition);
kube_condition!(StatefulSetCondition);
kube_condition!(DaemonSetCondition);
kube_condition!(JobCondition);
kube_condition!(PodCondition);
kube_condition!(NodeCondition);
kube_condition!(PersistentVolumeClaimCondition);

pub fn conditions<C: KubeCondition>(items: Option<&Vec<C>>, now: Timestamp) -> Vec<Condition> {
    items
        .into_iter()
        .flatten()
        .map(|item| item.core(now))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn kube_conditions_keep_reason_message_and_age() {
        let now = Timestamp::from_second(1_000).unwrap_or_default();
        let then = Timestamp::from_second(400).unwrap_or_default();
        let job = JobCondition {
            type_: "Failed".into(),
            status: "True".into(),
            reason: Some("DeadlineExceeded".into()),
            message: Some(" Job was active longer than specified deadline ".into()),
            last_transition_time: Some(Time(then)),
            ..JobCondition::default()
        };
        let rows = conditions(Some(&vec![job]), now);
        assert_eq!(
            rows,
            [Condition::new("Failed", "True")
                .reason("DeadlineExceeded")
                .message("Job was active longer than specified deadline")
                .age(Some(Duration::from_secs(600)))]
        );
        assert_eq!(
            conditions::<JobCondition>(None, now),
            Vec::<Condition>::new()
        );
    }
}
