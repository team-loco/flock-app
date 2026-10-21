use std::collections::BTreeMap;
use std::sync::Arc;

use flock_core::diagnosis::{self, claim, from_conditions};
use flock_core::resources::{Resource, ResourceKind};
use flock_core::time::age as age_label;
use flock_core::{Condition, Counts, Diagnosis, Status, Uid};
use jiff::Timestamp;
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use k8s_openapi::api::batch::v1::{CronJob, Job};
use k8s_openapi::api::core::v1::{
    ConfigMap, PersistentVolumeClaim, PodTemplateSpec, Secret, Service,
};
use k8s_openapi::api::networking::v1::Ingress;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::PartialObjectMeta;

use crate::clock::{age, between};
use crate::conditions::conditions;
use crate::detail::PodIndex;
use crate::manager::ClusterHandle;
use crate::problems::OwnerKey;
use crate::snapshot::count;

const MISSING: &str = "—";
const LIST_SEPARATOR: &str = ",";
const PROGRESS_DEADLINE: &str = "ProgressDeadlineExceeded";
const COMPLETE: &str = "Complete";
const FAILED: &str = "Failed";
const RUNNING: &str = "Running";
const SUSPENDED: &str = "Suspended";
const CONDITION_TRUE: &str = "True";
const BOUND: &str = "Bound";
const PENDING: &str = "Pending";
const STORAGE: &str = "storage";
const DEFAULT_REPLICAS: i32 = 1;
const DEFAULT_COMPLETIONS: i32 = 1;

pub(crate) fn row(
    meta: &ObjectMeta,
    status: Status,
    cells: Vec<String>,
    now: Timestamp,
) -> Resource {
    let labels = meta.labels.clone().unwrap_or_default();
    Resource {
        namespace: meta.namespace.clone().unwrap_or_default(),
        name: meta.name.clone().unwrap_or_default(),
        uid: Uid::new(meta.uid.clone().unwrap_or_default()),
        status,
        cells,
        age: age(meta, now),
        yaml: String::new(),
        labels,
        conditions: Vec::new(),
        diagnosis: None,
    }
}

fn explained(
    mut resource: Resource,
    conditions: Vec<Condition>,
    diagnosis: Option<Diagnosis>,
) -> Resource {
    resource.diagnosis = diagnosis.filter(|_| diagnosis::needs_diagnosis(resource.status));
    resource.conditions = conditions;
    resource
}

fn workload_diagnosis(
    pods: &PodIndex,
    meta: &ObjectMeta,
    kind: ResourceKind,
    conditions: &[Condition],
) -> Option<Diagnosis> {
    pods.diagnosis(&owner(meta, kind))
        .or_else(|| from_conditions(conditions))
}

fn owner(meta: &ObjectMeta, kind: ResourceKind) -> OwnerKey {
    OwnerKey {
        namespace: meta.namespace.clone().unwrap_or_default(),
        kind: kind.name().to_owned(),
        name: meta.name.clone().unwrap_or_default(),
    }
}

fn image(template: Option<&PodTemplateSpec>) -> String {
    template
        .and_then(|t| t.spec.as_ref())
        .and_then(|s| s.containers.first())
        .and_then(|c| c.image.clone())
        .unwrap_or_default()
}

fn rollout(ready: i32, desired: i32, issue: Option<Status>) -> Status {
    let base = if ready >= desired {
        Status::Ok
    } else {
        Status::Progress
    };
    issue.map_or(base, |issue| issue.max(base))
}

fn or_missing(value: String) -> String {
    if value.is_empty() {
        MISSING.to_owned()
    } else {
        value
    }
}

fn deployment_status(item: &Deployment, pods: &PodIndex) -> Status {
    let status = item.status.as_ref();
    let desired = item
        .spec
        .as_ref()
        .and_then(|s| s.replicas)
        .unwrap_or(DEFAULT_REPLICAS);
    let ready = status.and_then(|s| s.ready_replicas).unwrap_or_default();
    let stalled = status
        .and_then(|s| s.conditions.as_ref())
        .into_iter()
        .flatten()
        .any(|c| c.reason.as_deref() == Some(PROGRESS_DEADLINE));
    if stalled {
        return Status::Err;
    }
    let issue = pods.issue(&owner(&item.metadata, ResourceKind::Deployment));
    rollout(ready, desired, issue)
}

pub fn deployment(item: &Deployment, pods: &PodIndex, now: Timestamp) -> Resource {
    let spec = item.spec.as_ref();
    let status = item.status.as_ref();
    let desired = spec.and_then(|s| s.replicas).unwrap_or(DEFAULT_REPLICAS);
    let ready = status.and_then(|s| s.ready_replicas).unwrap_or_default();
    let updated = status.and_then(|s| s.updated_replicas).unwrap_or_default();
    let available = status
        .and_then(|s| s.available_replicas)
        .unwrap_or_default();
    let health = deployment_status(item, pods);
    let cells = vec![
        format!("{ready}/{desired}"),
        updated.to_string(),
        available.to_string(),
        image(spec.map(|s| &s.template)),
    ];
    let conditions = conditions(status.and_then(|s| s.conditions.as_ref()), now);
    let kind = ResourceKind::Deployment;
    let diagnosis = workload_diagnosis(pods, &item.metadata, kind, &conditions);
    explained(
        row(&item.metadata, health, cells, now),
        conditions,
        diagnosis,
    )
}

fn stateful_set_replicas(item: &StatefulSet) -> (i32, i32) {
    let desired = item
        .spec
        .as_ref()
        .and_then(|s| s.replicas)
        .unwrap_or(DEFAULT_REPLICAS);
    let ready = item
        .status
        .as_ref()
        .and_then(|s| s.ready_replicas)
        .unwrap_or_default();
    (ready, desired)
}

fn stateful_set_status(item: &StatefulSet, pods: &PodIndex) -> Status {
    let (ready, desired) = stateful_set_replicas(item);
    let issue = pods.issue(&owner(&item.metadata, ResourceKind::StatefulSet));
    rollout(ready, desired, issue)
}

pub fn stateful_set(item: &StatefulSet, pods: &PodIndex, now: Timestamp) -> Resource {
    let spec = item.spec.as_ref();
    let (ready, desired) = stateful_set_replicas(item);
    let service = spec
        .and_then(|s| s.service_name.clone())
        .unwrap_or_default();
    let cells = vec![
        format!("{ready}/{desired}"),
        or_missing(service),
        image(spec.map(|s| &s.template)),
    ];
    let status = item.status.as_ref();
    let conditions = conditions(status.and_then(|s| s.conditions.as_ref()), now);
    let kind = ResourceKind::StatefulSet;
    let diagnosis = workload_diagnosis(pods, &item.metadata, kind, &conditions);
    let health = stateful_set_status(item, pods);
    explained(
        row(&item.metadata, health, cells, now),
        conditions,
        diagnosis,
    )
}

fn daemon_set_pods(item: &DaemonSet) -> (i32, i32) {
    let status = item.status.as_ref();
    let desired = status
        .map(|s| s.desired_number_scheduled)
        .unwrap_or_default();
    let ready = status.map(|s| s.number_ready).unwrap_or_default();
    (ready, desired)
}

fn daemon_set_status(item: &DaemonSet, pods: &PodIndex) -> Status {
    let (ready, desired) = daemon_set_pods(item);
    let issue = pods.issue(&owner(&item.metadata, ResourceKind::DaemonSet));
    rollout(ready, desired, issue)
}

pub fn daemon_set(item: &DaemonSet, pods: &PodIndex, now: Timestamp) -> Resource {
    let spec = item.spec.as_ref();
    let (ready, desired) = daemon_set_pods(item);
    let selector = spec
        .and_then(|s| s.template.spec.as_ref())
        .and_then(|s| s.node_selector.as_ref())
        .map(|labels| {
            let pairs: Vec<String> = labels.iter().map(|(k, v)| format!("{k}={v}")).collect();
            pairs.join(LIST_SEPARATOR)
        })
        .unwrap_or_default();
    let cells = vec![
        desired.to_string(),
        ready.to_string(),
        or_missing(selector),
        image(spec.map(|s| &s.template)),
    ];
    let status = item.status.as_ref();
    let conditions = conditions(status.and_then(|s| s.conditions.as_ref()), now);
    let kind = ResourceKind::DaemonSet;
    let diagnosis = workload_diagnosis(pods, &item.metadata, kind, &conditions);
    let health = daemon_set_status(item, pods);
    explained(
        row(&item.metadata, health, cells, now),
        conditions,
        diagnosis,
    )
}

fn job_outcome(item: &Job) -> (Status, &'static str) {
    let suspended = item
        .spec
        .as_ref()
        .and_then(|s| s.suspend)
        .unwrap_or_default();
    let condition = |kind: &str| {
        item.status
            .as_ref()
            .and_then(|s| s.conditions.as_ref())
            .into_iter()
            .flatten()
            .any(|c| c.type_ == kind && c.status == CONDITION_TRUE)
    };
    if condition(COMPLETE) {
        (Status::Ok, COMPLETE)
    } else if condition(FAILED) {
        (Status::Err, FAILED)
    } else if suspended {
        (Status::Suspended, SUSPENDED)
    } else {
        (Status::Progress, RUNNING)
    }
}

fn job_status(item: &Job, pods: &PodIndex) -> Status {
    let (outcome, _) = job_outcome(item);
    let issue = pods.issue(&owner(&item.metadata, ResourceKind::Job));
    match (outcome, issue) {
        (Status::Progress, Some(issue)) => issue.max(outcome),
        _ => outcome,
    }
}

pub fn job(item: &Job, pods: &PodIndex, now: Timestamp) -> Resource {
    let status = item.status.as_ref();
    let completions = item
        .spec
        .as_ref()
        .and_then(|s| s.completions)
        .unwrap_or(DEFAULT_COMPLETIONS);
    let succeeded = status.and_then(|s| s.succeeded).unwrap_or_default();
    let started = status.and_then(|s| s.start_time.as_ref()).map(|t| t.0);
    let finished = status
        .and_then(|s| s.completion_time.as_ref())
        .map_or(now, |t| t.0);
    let duration = started.map_or_else(
        || MISSING.to_owned(),
        |started| age_label(between(started, finished)),
    );
    let (_, label) = job_outcome(item);
    let cells = vec![
        format!("{succeeded}/{completions}"),
        duration,
        label.to_owned(),
    ];
    let conditions = conditions(status.and_then(|s| s.conditions.as_ref()), now);
    let owner = owner(&item.metadata, ResourceKind::Job);
    let diagnosis = from_conditions(&conditions).or_else(|| pods.diagnosis(&owner));
    let health = job_status(item, pods);
    explained(
        row(&item.metadata, health, cells, now),
        conditions,
        diagnosis,
    )
}

fn cron_job_status(item: &CronJob) -> Status {
    if item.spec.suspend.unwrap_or_default() {
        Status::Suspended
    } else {
        Status::Ok
    }
}

pub fn cron_job(item: &CronJob, now: Timestamp) -> Resource {
    let spec = &item.spec;
    let suspended = spec.suspend.unwrap_or_default();
    let status = item.status.as_ref();
    let active = status.and_then(|s| s.active.as_ref()).map_or(0, Vec::len);
    let last = status
        .and_then(|s| s.last_schedule_time.as_ref())
        .map_or_else(|| MISSING.to_owned(), |t| age_label(between(t.0, now)));
    let health = cron_job_status(item);
    let suspend = if suspended { "True" } else { "False" };
    let cells = vec![
        spec.schedule.clone(),
        suspend.to_owned(),
        active.to_string(),
        last,
    ];
    row(&item.metadata, health, cells, now)
}

pub fn service(item: &Service, now: Timestamp) -> Resource {
    let spec = item.spec.as_ref();
    let ports: Vec<String> = spec
        .and_then(|s| s.ports.as_ref())
        .into_iter()
        .flatten()
        .map(|p| format!("{}/{}", p.port, p.protocol.as_deref().unwrap_or("TCP")))
        .collect();
    let cells = vec![
        spec.and_then(|s| s.type_.clone()).unwrap_or_default(),
        or_missing(spec.and_then(|s| s.cluster_ip.clone()).unwrap_or_default()),
        or_missing(ports.join(LIST_SEPARATOR)),
    ];
    row(&item.metadata, Status::Ok, cells, now)
}

pub fn ingress(item: &Ingress, now: Timestamp) -> Resource {
    let spec = item.spec.as_ref();
    let hosts: Vec<String> = spec
        .and_then(|s| s.rules.as_ref())
        .into_iter()
        .flatten()
        .filter_map(|r| r.host.clone())
        .collect();
    let addresses: Vec<String> = item
        .status
        .as_ref()
        .and_then(|s| s.load_balancer.as_ref())
        .and_then(|lb| lb.ingress.as_ref())
        .into_iter()
        .flatten()
        .filter_map(|i| i.hostname.clone().or_else(|| i.ip.clone()))
        .collect();
    let cells = vec![
        or_missing(
            spec.and_then(|s| s.ingress_class_name.clone())
                .unwrap_or_default(),
        ),
        or_missing(hosts.join(LIST_SEPARATOR)),
        or_missing(addresses.join(LIST_SEPARATOR)),
    ];
    row(&item.metadata, Status::Ok, cells, now)
}

pub fn config_map(item: &PartialObjectMeta<ConfigMap>, now: Timestamp) -> Resource {
    row(&item.metadata, Status::Ok, vec![MISSING.to_owned()], now)
}

pub fn secret(item: &PartialObjectMeta<Secret>, now: Timestamp) -> Resource {
    let cells = vec![MISSING.to_owned(), MISSING.to_owned()];
    row(&item.metadata, Status::Ok, cells, now)
}

fn pvc_status(item: &PersistentVolumeClaim) -> Status {
    let phase = item.status.as_ref().and_then(|s| s.phase.as_deref());
    match phase.unwrap_or_default() {
        BOUND => Status::Ok,
        PENDING => Status::Warn,
        _ => Status::Err,
    }
}

pub fn pvc(item: &PersistentVolumeClaim, now: Timestamp) -> Resource {
    let status = item.status.as_ref();
    let spec = item.spec.as_ref();
    let phase = status.and_then(|s| s.phase.clone()).unwrap_or_default();
    let health = pvc_status(item);
    let capacity = status
        .and_then(|s| s.capacity.as_ref())
        .and_then(|c| c.get(STORAGE))
        .map(|q| q.0.clone())
        .unwrap_or_default();
    let cells = vec![
        or_missing(phase),
        or_missing(capacity),
        or_missing(
            spec.and_then(|s| s.storage_class_name.clone())
                .unwrap_or_default(),
        ),
        or_missing(spec.and_then(|s| s.volume_name.clone()).unwrap_or_default()),
    ];
    let conditions = conditions(status.and_then(|s| s.conditions.as_ref()), now);
    let phase = status.and_then(|s| s.phase.as_deref()).unwrap_or_default();
    let diagnosis = claim(phase, &conditions);
    explained(
        row(&item.metadata, health, cells, now),
        conditions,
        diagnosis,
    )
}

fn sorted(mut rows: Vec<Resource>) -> Vec<Resource> {
    rows.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    rows
}

fn map<K, F>(items: &[Arc<K>], build: F) -> Vec<Resource>
where
    F: Fn(&K) -> Resource,
{
    sorted(items.iter().map(|item| build(item)).collect())
}

pub fn rows(
    handle: &ClusterHandle,
    kind: ResourceKind,
    pods: &PodIndex,
    now: Timestamp,
) -> Vec<Resource> {
    match kind {
        ResourceKind::Deployment => map(&handle.objects::<Deployment>(), |i| {
            deployment(i, pods, now)
        }),
        ResourceKind::StatefulSet => map(&handle.objects::<StatefulSet>(), |i| {
            stateful_set(i, pods, now)
        }),
        ResourceKind::DaemonSet => {
            map(&handle.objects::<DaemonSet>(), |i| daemon_set(i, pods, now))
        }
        ResourceKind::Job => map(&handle.objects::<Job>(), |i| job(i, pods, now)),
        ResourceKind::CronJob => map(&handle.objects::<CronJob>(), |i| cron_job(i, now)),
        ResourceKind::Service => map(&handle.objects::<Service>(), |i| service(i, now)),
        ResourceKind::Ingress => map(&handle.objects::<Ingress>(), |i| ingress(i, now)),
        ResourceKind::ConfigMap => map(&handle.objects::<PartialObjectMeta<ConfigMap>>(), |i| {
            config_map(i, now)
        }),
        ResourceKind::Secret => map(&handle.objects::<PartialObjectMeta<Secret>>(), |i| {
            secret(i, now)
        }),
        ResourceKind::PersistentVolumeClaim => {
            map(&handle.objects::<PersistentVolumeClaim>(), |i| pvc(i, now))
        }
    }
}

fn tally<K>(items: &[Arc<K>], status: impl Fn(&K) -> Status) -> Counts {
    Counts::of(items.iter().map(|item| status(item)))
}

pub fn counts(handle: &ClusterHandle, kind: ResourceKind, pods: &PodIndex) -> Counts {
    match kind {
        ResourceKind::Deployment => tally(&handle.objects::<Deployment>(), |i| {
            deployment_status(i, pods)
        }),
        ResourceKind::StatefulSet => tally(&handle.objects::<StatefulSet>(), |i| {
            stateful_set_status(i, pods)
        }),
        ResourceKind::DaemonSet => tally(&handle.objects::<DaemonSet>(), |i| {
            daemon_set_status(i, pods)
        }),
        ResourceKind::Job => tally(&handle.objects::<Job>(), |i| job_status(i, pods)),
        ResourceKind::CronJob => tally(&handle.objects::<CronJob>(), cron_job_status),
        ResourceKind::Service => Counts::total(count(handle.objects::<Service>().len())),
        ResourceKind::Ingress => Counts::total(count(handle.objects::<Ingress>().len())),
        ResourceKind::ConfigMap => {
            let maps = handle.objects::<PartialObjectMeta<ConfigMap>>();
            Counts::total(count(maps.len()))
        }
        ResourceKind::Secret => {
            let secrets = handle.objects::<PartialObjectMeta<Secret>>();
            Counts::total(count(secrets.len()))
        }
        ResourceKind::PersistentVolumeClaim => {
            tally(&handle.objects::<PersistentVolumeClaim>(), pvc_status)
        }
    }
}

pub fn watched_lists(
    handle: &ClusterHandle,
    pods: &PodIndex,
    now: Timestamp,
) -> BTreeMap<ResourceKind, Vec<Resource>> {
    let watched = handle.watched_kinds();
    ResourceKind::ALL
        .into_iter()
        .filter(|kind| {
            crate::snapshot::page_kind(kind.page()).is_some_and(|w| watched.contains(&w))
        })
        .map(|kind| (kind, rows(handle, kind, pods, now)))
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::problems::tests::{at, crash_status, now, parse, replica_pod};

    #[test]
    fn deployment_cells_and_status_follow_its_pods() {
        let item: Arc<Deployment> = parse(json!({
            "metadata": { "name": "checkout-api", "namespace": "payments", "creationTimestamp": at(60), "labels": { "app": "checkout" } },
            "spec": {
                "replicas": 3,
                "selector": {},
                "template": { "spec": { "containers": [{ "name": "api", "image": "ghcr.io/acme/checkout-api:2.14.0" }] } }
            },
            "status": { "readyReplicas": 2, "updatedReplicas": 3, "availableReplicas": 2 }
        }));
        let quiet = PodIndex::build(&[], now());
        let rolling = deployment(&item, &quiet, now());
        assert_eq!(
            rolling.cells,
            ["2/3", "3", "2", "ghcr.io/acme/checkout-api:2.14.0"]
        );
        assert_eq!(rolling.status, Status::Progress);
        assert_eq!(
            rolling.labels.get("app").map(String::as_str),
            Some("checkout")
        );
        let crashing = PodIndex::build(
            &[replica_pod(
                "checkout-api-7f9c-x2lqp",
                "checkout-api",
                "7f9c",
                &crash_status(),
            )],
            now(),
        );
        let crashed = deployment(&item, &crashing, now());
        assert_eq!(crashed.status, Status::Err);
        assert_eq!(
            crashed.diagnosis.map(|d| d.reason),
            Some("CrashLoopBackOff".into())
        );
        assert_eq!(rolling.diagnosis, None);
    }

    #[test]
    fn jobs_report_completions_duration_and_outcome() {
        let item: Arc<Job> = parse(json!({
            "metadata": { "name": "backfill", "namespace": "data" },
            "spec": { "template": {} },
            "status": {
                "succeeded": 0, "startTime": at(14), "completionTime": null,
                "conditions": [{ "type": "Failed", "status": "True" }]
            }
        }));
        let quiet = PodIndex::build(&[], now());
        let row = job(&item, &quiet, now());
        assert_eq!(row.cells, ["0/1", "14m", "Failed"]);
        assert_eq!(row.status, Status::Err);
        assert_eq!(row.diagnosis.map(|d| d.reason), Some("Failed".into()));
    }

    #[test]
    fn failed_jobs_explain_the_deadline_and_running_jobs_their_pods() {
        let item: Arc<Job> = parse(json!({
            "metadata": { "name": "backfill", "namespace": "data" },
            "spec": { "template": {}, "activeDeadlineSeconds": 600 },
            "status": {
                "startTime": at(14),
                "conditions": [
                    { "type": "FailureTarget", "status": "True", "reason": "DeadlineExceeded", "message": "Job was active longer than specified deadline" },
                    { "type": "Failed", "status": "True", "reason": "DeadlineExceeded", "message": "Job was active longer than specified deadline", "lastTransitionTime": at(4) }
                ]
            }
        }));
        let quiet = PodIndex::build(&[], now());
        let row = job(&item, &quiet, now());
        assert_eq!(
            row.diagnosis,
            Some(Diagnosis::new(
                "DeadlineExceeded",
                "Job was active longer than specified deadline"
            ))
        );
        assert_eq!(row.conditions.len(), 2);
        assert_eq!(
            row.conditions[1].age,
            Some(std::time::Duration::from_secs(240))
        );
        let running: Arc<Job> = parse(json!({
            "metadata": { "name": "import", "namespace": "payments" },
            "spec": { "template": {} },
            "status": { "active": 1 }
        }));
        let mut pod = (*replica_pod("import-x", "import", "1", &crash_status())).clone();
        if let Some(owner) = pod
            .metadata
            .owner_references
            .as_mut()
            .and_then(|o| o.first_mut())
        {
            owner.kind = "Job".into();
            owner.name = "import".into();
        }
        let pod_index = PodIndex::build(&[Arc::new(pod)], now());
        let row = job(&running, &pod_index, now());
        assert_eq!(row.status, Status::Err);
        assert_eq!(
            row.diagnosis.map(|d| d.reason),
            Some("CrashLoopBackOff".into())
        );
        assert_eq!(job(&running, &quiet, now()).diagnosis, None);
    }

    #[test]
    fn suspended_cron_jobs_and_unbound_claims_are_flagged() {
        let cron: Arc<CronJob> = parse(json!({
            "metadata": { "name": "nightly", "namespace": "ops" },
            "spec": { "schedule": "0 3 * * *", "suspend": true, "jobTemplate": {} },
            "status": { "lastScheduleTime": at(120) }
        }));
        let row = cron_job(&cron, now());
        assert_eq!(row.cells, ["0 3 * * *", "True", "0", "2h"]);
        assert_eq!(row.status, Status::Suspended);
        let claim: Arc<PersistentVolumeClaim> = parse(json!({
            "metadata": { "name": "data-0", "namespace": "db" },
            "spec": { "storageClassName": "standard" },
            "status": { "phase": "Pending" }
        }));
        let row = pvc(&claim, now());
        assert_eq!(row.cells, ["Pending", "—", "standard", "—"]);
        assert_eq!(row.status, Status::Warn);
        assert_eq!(row.diagnosis, Some(Diagnosis::new("Pending", "")));
    }

    #[test]
    fn services_list_type_ip_and_ports() {
        let item: Arc<Service> = parse(json!({
            "metadata": { "name": "web", "namespace": "web" },
            "spec": { "type": "ClusterIP", "clusterIP": "10.96.0.12", "ports": [{ "port": 80 }, { "port": 443, "protocol": "TCP" }] }
        }));
        assert_eq!(
            service(&item, now()).cells,
            ["ClusterIP", "10.96.0.12", "80/TCP,443/TCP"]
        );
    }
}
