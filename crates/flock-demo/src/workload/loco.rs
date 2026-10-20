use std::time::Duration;

use flock_core::workload::{
    ContainerSpec, ContainerStatus, EnvResolution, EnvSource, EnvVar, Event, Pod, Port, Probe,
    ProbeAction, Resources, Revision,
};
use flock_core::{ClusterId, ResourceRef, Status, Uid, Workload, WorkloadKind};

use crate::lists::{deployment_conditions, timed_out};

pub const CLUSTER: &str = "prod-us-east";
pub const NAMESPACE: &str = "ws-0192c3e7";
pub const NAME: &str = "checkout";

const IMAGE: &str = "loco-registry:5000/acme/checkout@sha256:9e1cca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b980778544d0";
const POD_PREFIX: &str = "checkout-6d9f8c7b4";
const NODE: &str = "ip-10-4-12-87";
const PHASE: &str = "CreateContainerConfigError";
const REASON: &str = "Failed";
const ERROR_MESSAGE: &str = "container has runAsNonRoot and image will run as root";
const APP_PORT: u16 = 8080;
const PROBE_PERIOD_SECS: u64 = 10;
const DESIRED: u32 = 2;
const REVISION: u64 = 1;
const AGE_SECS: u64 = 6 * 60;
const EVENT_COUNT: u32 = 24;
const POD_SUFFIXES: [&str; 2] = ["k2m7x", "r9t4w"];
const ENV_SECRET: &str = "checkout-env";

pub fn target() -> ResourceRef {
    ResourceRef {
        cluster: ClusterId::new(CLUSTER),
        kind: WorkloadKind::Deployment.kind().into(),
        namespace: Some(NAMESPACE.into()),
        name: NAME.into(),
    }
}

pub fn checkout() -> Workload {
    let age = Duration::from_secs(AGE_SECS);
    Workload {
        uid: Uid::default(),
        target: target(),
        kind: WorkloadKind::Deployment,
        desired: DESIRED,
        conditions: deployment_conditions(false, timed_out(POD_PREFIX)),
        pending: None,
        managed_by: None,
        pods: POD_SUFFIXES
            .iter()
            .map(|suffix| pod(&format!("{POD_PREFIX}-{suffix}"), age))
            .collect(),
        revisions: vec![Revision {
            number: REVISION,
            owner: POD_PREFIX.into(),
            image: IMAGE.into(),
            ready: 0,
            replicas: DESIRED,
            age,
            commit: None,
            current: true,
        }],
        history_pending: false,
        containers: vec![container()],
        events: vec![Event {
            age,
            status: Status::Err,
            reason: REASON.into(),
            message: ERROR_MESSAGE.into(),
            count: EVENT_COUNT,
        }],
    }
}

fn pod(name: &str, age: Duration) -> Pod {
    Pod {
        name: name.into(),
        phase: PHASE.into(),
        status: Status::Err,
        node: NODE.into(),
        cpu_millis: None,
        memory_bytes: None,
        age,
        containers: vec![ContainerStatus {
            name: NAME.into(),
            ready: false,
            restarts: 0,
            last_exit: None,
        }],
    }
}

fn container() -> ContainerSpec {
    ContainerSpec {
        name: NAME.into(),
        image: IMAGE.into(),
        ports: vec![Port {
            port: APP_PORT,
            protocol: "TCP".into(),
            name: Some("http".into()),
        }],
        requests: Resources {
            cpu: Some("500m".into()),
            memory: Some("512Mi".into()),
        },
        limits: Resources {
            cpu: Some("500m".into()),
            memory: Some("512Mi".into()),
        },
        readiness: Some(Probe {
            action: ProbeAction::HttpGet {
                port: APP_PORT,
                path: "/healthz".into(),
            },
            period: Duration::from_secs(PROBE_PERIOD_SECS),
        }),
        env: vec![EnvVar {
            name: "LOCO_ENV".into(),
            source: EnvSource::SecretKey {
                name: ENV_SECRET.into(),
                key: "env".into(),
            },
            resolution: EnvResolution::Resolved,
        }],
    }
}
