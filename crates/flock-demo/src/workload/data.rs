use std::time::Duration;

use flock_core::workload::{
    Commit, ContainerSpec, ContainerStatus, EnvResolution, EnvSource, EnvVar, Event, Exit, Pod,
    Port, Probe, ProbeAction, Resources, Revision,
};
use flock_core::{ClusterId, Condition, ResourceRef, Status, Uid, Workload, WorkloadKind};

use crate::lists::{deployment_conditions, rolled_out, timed_out};

pub const CLUSTER: &str = "prod-us-east";
pub const NAMESPACE: &str = "payments";
pub const NAME: &str = "checkout-api";
pub const API: &str = "api";
pub const ENVOY: &str = "envoy";
pub const BROKEN_IMAGE: &str = "ghcr.io/acme/checkout-api:2.14.0";
const BROKEN_REPLICA_SET: &str = "checkout-api-7f9c6d";

const SECOND: u64 = 1;
const MINUTE: u64 = 60 * SECOND;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;
const MIB: u64 = 1024 * 1024;
const POD_AGE: u64 = 3 * HOUR;
const CRASH_RESTARTS: u32 = 14;
const LAST_EXIT_AGO: u64 = 2 * MINUTE;
const API_PORT: u16 = 8080;
const ENVOY_ADMIN_PORT: u16 = 9901;
const ENVOY_PORT: u16 = 8443;
const PROBE_PERIOD: u64 = 5 * SECOND;
const DESIRED: u32 = 3;
const CURRENT_READY: u32 = 2;
const HEALTHY_CPU: u32 = 198;
const HEALTHY_MEMORY_MIB: u64 = 294;

pub fn target() -> ResourceRef {
    ResourceRef {
        cluster: ClusterId::new(CLUSTER),
        kind: WorkloadKind::Deployment.kind().into(),
        namespace: Some(NAMESPACE.into()),
        name: NAME.into(),
    }
}

pub fn checkout_api() -> Workload {
    Workload {
        uid: Uid::default(),
        target: target(),
        kind: WorkloadKind::Deployment,
        desired: DESIRED,
        conditions: deployment_conditions(true, timed_out(BROKEN_REPLICA_SET)),
        pending: None,
        managed_by: Some(ResourceRef {
            cluster: ClusterId::new(CLUSTER),
            kind: "Kustomization".into(),
            namespace: Some("flux-system".into()),
            name: "apps-payments".into(),
        }),
        pods: pods(),
        revisions: revisions(),
        history_pending: false,
        containers: vec![api_container(), envoy_container()],
        events: events(),
    }
}

pub fn healthy_checkout(
    cluster: &ClusterId,
    pods: &[(String, String)],
    version: Option<&str>,
) -> Workload {
    let mut workload = checkout_api();
    let replicas = u32::try_from(pods.len()).unwrap_or(u32::MAX);
    let current = version.map(|version| format!(":{version}"));
    let current_ix = workload
        .revisions
        .iter()
        .position(|r| {
            current
                .as_ref()
                .is_none_or(|tag| r.image.ends_with(tag.as_str()))
        })
        .unwrap_or_default();
    workload.revisions.drain(..current_ix);
    for (ix, revision) in workload.revisions.iter_mut().enumerate() {
        revision.current = ix == 0;
        revision.ready = if revision.current { replicas } else { 0 };
        revision.replicas = revision.ready;
    }
    let image = workload.revisions.first().map(|r| r.image.clone());
    workload.target.cluster = cluster.clone();
    if let Some(owner) = workload.managed_by.as_mut() {
        owner.cluster = cluster.clone();
    }
    workload.desired = replicas;
    workload.conditions = healthy_conditions(&workload);
    workload.pods = pods
        .iter()
        .map(|(name, node)| {
            running_pod(
                name.clone(),
                node,
                HEALTHY_CPU,
                HEALTHY_MEMORY_MIB,
                secs(POD_AGE),
            )
        })
        .collect();
    for container in &mut workload.containers {
        if container.name == API {
            if let Some(image) = &image {
                container.image.clone_from(image);
            }
            for var in &mut container.env {
                var.resolution = match var.resolution {
                    EnvResolution::MissingKey { .. } => EnvResolution::Resolved,
                    ref other => other.clone(),
                };
            }
        }
    }
    workload.events = Vec::new();
    workload
}

pub fn healthy_conditions(workload: &Workload) -> Vec<Condition> {
    let owner = workload
        .current_revision()
        .map_or(NAME, |revision| revision.owner.as_str());
    deployment_conditions(true, rolled_out(owner))
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn containers(api_ready: bool, api_restarts: u32, last_exit: Option<Exit>) -> Vec<ContainerStatus> {
    vec![
        ContainerStatus {
            name: API.into(),
            ready: api_ready,
            restarts: api_restarts,
            last_exit,
        },
        ContainerStatus {
            name: ENVOY.into(),
            ready: true,
            restarts: 0,
            last_exit: None,
        },
    ]
}

pub fn crash_exit() -> Exit {
    Exit {
        code: 1,
        reason: "Error".into(),
        ago: secs(LAST_EXIT_AGO),
    }
}

pub fn crashing_pod(name: String, node: &str, restarts: u32, age: Duration) -> Pod {
    Pod {
        name,
        phase: "CrashLoopBackOff".into(),
        status: Status::Err,
        node: node.into(),
        cpu_millis: Some(4),
        memory_bytes: Some(38 * MIB),
        age,
        containers: containers(false, restarts, Some(crash_exit())),
    }
}

pub fn running_pod(name: String, node: &str, cpu: u32, mem_mib: u64, age: Duration) -> Pod {
    Pod {
        name,
        phase: "Running".into(),
        status: Status::Ok,
        node: node.into(),
        cpu_millis: Some(cpu),
        memory_bytes: Some(mem_mib * MIB),
        age,
        containers: containers(true, 0, None),
    }
}

fn pods() -> Vec<Pod> {
    let age = secs(POD_AGE);
    vec![
        crashing_pod(
            "checkout-api-7f9c6d-x2lqp".into(),
            "ip-10-4-12-87",
            CRASH_RESTARTS,
            age,
        ),
        running_pod(
            "checkout-api-7f9c6d-m8vtn".into(),
            "ip-10-4-14-201",
            212,
            301,
            age,
        ),
        running_pod(
            "checkout-api-7f9c6d-q4zhk".into(),
            "ip-10-4-22-63",
            187,
            288,
            age,
        ),
    ]
}

fn revision(
    number: u64,
    owner: &str,
    image: &str,
    age: u64,
    commit: (&str, &str),
    current: bool,
) -> Revision {
    let (ready, replicas) = if current {
        (CURRENT_READY, DESIRED)
    } else {
        (0, 0)
    };
    Revision {
        number,
        owner: owner.into(),
        image: image.into(),
        ready,
        replicas,
        age: secs(age),
        commit: Some(Commit {
            sha: commit.0.into(),
            subject: commit.1.into(),
        }),
        current,
    }
}

fn revisions() -> Vec<Revision> {
    vec![
        revision(
            37,
            BROKEN_REPLICA_SET,
            BROKEN_IMAGE,
            3 * HOUR,
            (
                "a41e07c",
                "checkout-api: bump to 2.14.0, read webhook secret from env",
            ),
            true,
        ),
        revision(
            36,
            "checkout-api-5b8f2a",
            "ghcr.io/acme/checkout-api:2.13.4",
            9 * DAY,
            ("d81f3c0", "checkout-api: retry budget for stripe calls"),
            false,
        ),
        revision(
            35,
            "checkout-api-66c1e9",
            "ghcr.io/acme/checkout-api:2.13.3",
            23 * DAY,
            ("e19a4b8", "payments: add PDB and HPA"),
            false,
        ),
    ]
}

fn env(name: &str, source: EnvSource, resolution: EnvResolution) -> EnvVar {
    EnvVar {
        name: name.into(),
        source,
        resolution,
    }
}

fn secret(name: &str, key: &str) -> EnvSource {
    EnvSource::SecretKey {
        name: name.into(),
        key: key.into(),
    }
}

fn http_probe(port: u16, path: &str) -> Probe {
    Probe {
        action: ProbeAction::HttpGet {
            port,
            path: path.into(),
        },
        period: secs(PROBE_PERIOD),
    }
}

fn port(port: u16, name: &str) -> Port {
    Port {
        port,
        protocol: "TCP".into(),
        name: Some(name.into()),
    }
}

fn resources(cpu: &str, memory: &str) -> Resources {
    Resources {
        cpu: Some(cpu.into()),
        memory: Some(memory.into()),
    }
}

fn api_container() -> ContainerSpec {
    ContainerSpec {
        name: API.into(),
        image: BROKEN_IMAGE.into(),
        ports: vec![port(API_PORT, "http")],
        requests: resources("500m", "512Mi"),
        limits: resources("1", "1Gi"),
        readiness: Some(http_probe(API_PORT, "/healthz")),
        env: vec![
            env(
                "DATABASE_URL",
                secret("payments-db", "url"),
                EnvResolution::Resolved,
            ),
            env(
                "STRIPE_API_KEY",
                secret("stripe", "api-key"),
                EnvResolution::Resolved,
            ),
            env(
                "STRIPE_WEBHOOK_SECRET",
                secret("stripe", "webhook_secret"),
                EnvResolution::MissingKey {
                    optional: true,
                    keys: vec!["api-key".into(), "webhook-secret".into()],
                },
            ),
            env(
                "LOG_LEVEL",
                EnvSource::Value("info".into()),
                EnvResolution::Literal,
            ),
        ],
    }
}

fn envoy_container() -> ContainerSpec {
    ContainerSpec {
        name: ENVOY.into(),
        image: "envoyproxy/envoy:v1.31.2".into(),
        ports: vec![port(ENVOY_PORT, "https"), port(ENVOY_ADMIN_PORT, "admin")],
        requests: resources("100m", "128Mi"),
        limits: resources("500m", "256Mi"),
        readiness: Some(http_probe(ENVOY_ADMIN_PORT, "/ready")),
        env: vec![
            env(
                "ENVOY_LOG_LEVEL",
                EnvSource::Value("warn".into()),
                EnvResolution::Literal,
            ),
            env(
                "POD_NAME",
                EnvSource::Field("metadata.name".into()),
                EnvResolution::Resolved,
            ),
        ],
    }
}

fn event(age: u64, status: Status, reason: &str, message: &str, count: u32) -> Event {
    Event {
        age: secs(age),
        status,
        reason: reason.into(),
        message: message.into(),
        count,
    }
}

fn events() -> Vec<Event> {
    let pods_age = POD_AGE;
    vec![
        event(
            LAST_EXIT_AGO,
            Status::Err,
            "BackOff",
            "Back-off restarting failed container api in pod checkout-api-7f9c6d-x2lqp",
            CRASH_RESTARTS,
        ),
        event(
            pods_age,
            Status::Err,
            "ProgressDeadlineExceeded",
            "ReplicaSet \"checkout-api-7f9c6d\" has timed out progressing",
            1,
        ),
        event(
            pods_age,
            Status::Ok,
            "ScalingReplicaSet",
            "Scaled down replica set checkout-api-5b8f2a to 0 from 1",
            1,
        ),
        event(
            pods_age,
            Status::Ok,
            "ScalingReplicaSet",
            "Scaled up replica set checkout-api-7f9c6d to 3 from 2",
            1,
        ),
        event(
            pods_age,
            Status::Ok,
            "ScalingReplicaSet",
            "Scaled up replica set checkout-api-7f9c6d to 1",
            1,
        ),
        event(
            pods_age,
            Status::Ok,
            "SuccessfulCreate",
            "Created pod: checkout-api-7f9c6d-q4zhk",
            1,
        ),
        event(
            pods_age,
            Status::Ok,
            "SuccessfulCreate",
            "Created pod: checkout-api-7f9c6d-m8vtn",
            1,
        ),
    ]
}
