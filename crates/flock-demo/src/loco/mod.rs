mod platform;
mod routes;

use std::collections::BTreeMap;
use std::time::Duration;

use flock_core::loco::{
    AppPhase, AppRoute, Application, Applications, Build, BuildLogLine, BuildPhase, Builds,
    ControllerObject, Degraded, HealthCheck, LocoCluster, LocoCommand, LogTone, Placement,
    PlacementIssue, Resources, Rollout, Workspace, WorkspaceApps,
};
use flock_core::{ClusterId, ResourceRef, Snapshot, Status, Uid};

const CLUSTER: &str = "prod-us-east";
const APP_NAMESPACE: &str = "loco-system";
const BUILD_NAMESPACE: &str = "loco-builds";
const BUILD_POOL: &str = "builds";
const BUILD_RETENTION: Duration = Duration::from_secs(24 * HOUR);
const BUILD_NODE: &str = "ip-10-4-40-7";
const APPLICATIONS_TOTAL: usize = 214;
const REGION: &str = "us-east";
const REGISTRY: &str = "loco-registry:5000/acme";
const APP_DIGEST: &str = "sha256:9e1cca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b980778544d0";
const AUTH_DIGEST: &str = "sha256:51ab3e23e8160039594a33894f6564e1b1348bbd7a0088d42c4acb73eeae0f2e";
const HOST_SUFFIX: &str = ".onloco.app";
const STAGING: &str = "staging";
const PRODUCTION: &str = "production";
const GATEWAY: &str = "eg";
const LISTENER: &str = "https-apps-0";
const APP_PORT: u16 = 8080;
const ENV_SECRET_REVISION: u32 = 6;
const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;
const CANCEL_REQUESTED: &str = "cancel requested from Flock";
const CANCELED_LINE: &str = "canceled";

const PAYMENTS: (&str, &str) = ("ws-0192c3e7", "acme / payments");
const WEB: (&str, &str) = ("ws-0192c3f1", "acme / web");
const CORE: (&str, &str) = ("ws-0192d0aa", "beta / core");

pub fn clusters() -> BTreeMap<ClusterId, LocoCluster> {
    let cluster = LocoCluster {
        platform: platform::platform(),
        applications: applications(),
        builds: builds(),
        routes: routes::routes(),
    };
    BTreeMap::from([(ClusterId::new(CLUSTER), cluster)])
}

fn workspace((namespace, name): (&str, &str)) -> Workspace {
    Workspace {
        namespace: namespace.into(),
        name: Some(name.into()),
    }
}

struct AppSeed {
    id: &'static str,
    name: &'static str,
    environment: &'static str,
    phase: AppPhase,
    ready: i32,
    desired: i32,
    applied_revision: i64,
    desired_revision: i64,
}

const fn seed(
    id: &'static str,
    name: &'static str,
    environment: &'static str,
    phase: AppPhase,
    (ready, desired): (i32, i32),
    (applied_revision, desired_revision): (i64, i64),
) -> AppSeed {
    AppSeed {
        id,
        name,
        environment,
        phase,
        ready,
        desired,
        applied_revision,
        desired_revision,
    }
}

fn applications() -> Applications {
    let group = |ws: (&str, &str), seeds: Vec<AppSeed>| WorkspaceApps {
        workspace: workspace(ws),
        applications: seeds
            .into_iter()
            .map(|seed| application(&seed, ws.0))
            .collect(),
    };
    Applications {
        total: APPLICATIONS_TOTAL,
        workspaces: vec![
            group(
                PAYMENTS,
                vec![
                    seed(
                        "checkout",
                        "checkout",
                        PRODUCTION,
                        AppPhase::Failed,
                        (0, 2),
                        (14, 14),
                    ),
                    seed(
                        "checkout-worker",
                        "checkout-worker",
                        PRODUCTION,
                        AppPhase::Ready,
                        (2, 2),
                        (9, 9),
                    ),
                    seed(
                        "auth",
                        "auth",
                        PRODUCTION,
                        AppPhase::Ready,
                        (3, 3),
                        (31, 31),
                    ),
                ],
            ),
            group(
                WEB,
                vec![
                    seed(
                        "frontend",
                        "frontend",
                        PRODUCTION,
                        AppPhase::Ready,
                        (4, 4),
                        (22, 22),
                    ),
                    seed(
                        "frontend-stg",
                        "frontend",
                        STAGING,
                        AppPhase::Deploying,
                        (1, 2),
                        (7, 7),
                    ),
                    seed(
                        "docs",
                        "docs",
                        PRODUCTION,
                        AppPhase::Deploying,
                        (1, 1),
                        (3, 3),
                    ),
                ],
            ),
            group(
                CORE,
                vec![
                    seed("api", "api", PRODUCTION, AppPhase::Ready, (2, 2), (21, 22)),
                    seed("jobs", "jobs", PRODUCTION, AppPhase::Ready, (1, 1), (5, 5)),
                ],
            ),
        ],
    }
}

fn application(seed: &AppSeed, namespace: &str) -> Application {
    let failing = seed.phase == AppPhase::Failed;
    let host = if seed.environment == STAGING {
        format!("{}-{STAGING}{HOST_SUFFIX}", seed.name)
    } else {
        format!("{}{HOST_SUFFIX}", seed.name)
    };
    let rollout = if failing {
        Rollout {
            deployment_id: "0192f4b2".into(),
            age: Some(Duration::from_secs(6 * MINUTE)),
            build: Some("0192f3a1".into()),
        }
    } else {
        Rollout {
            deployment_id: "0192e8c0".into(),
            age: Some(Duration::from_secs(2 * DAY)),
            build: None,
        }
    };
    let degraded = failing.then(|| Degraded {
        reason: "CreateContainerConfigError".into(),
        message: "container has runAsNonRoot and image will run as root (pod: \"checkout-6c9d7f-2xk4p_ws-0192c3e7\", container: app)".into(),
        since: Some(Duration::from_secs(6 * MINUTE)),
    });
    let replicas = format!("{}/{}", seed.ready, seed.desired);
    let deployment_status = if failing { Status::Err } else { Status::Ok };
    Application {
        labels: BTreeMap::new(),
        resource: app_ref(seed.id),
        name: seed.name.into(),
        environment: seed.environment.into(),
        region: Some(REGION.into()),
        phase: seed.phase,
        ready_replicas: Some(seed.ready),
        desired_replicas: Some(seed.desired),
        placement: Placement {
            desired: Some(seed.desired_revision),
            applied: Some(seed.applied_revision),
            issue: placement_issue(seed.id),
        },
        hostname: Some(host.clone()),
        degraded,
        image: Some(format!("{REGISTRY}/{}@{APP_DIGEST}", seed.name)),
        rollout: Some(rollout),
        resources: Some(Resources {
            cpu: "500m".into(),
            memory: "512Mi".into(),
            min_replicas: 2,
            max_replicas: 6,
            cpu_target: Some(70),
        }),
        health_check: Some(HealthCheck {
            path: "/healthz".into(),
            interval_secs: 10,
            timeout_secs: 3,
            fail_threshold: 3,
        }),
        workload_uid: Uid::default(),
        objects: controller_objects(namespace, seed.name, &replicas, deployment_status),
        route: Some(AppRoute {
            host,
            gateway: GATEWAY.into(),
            listener: LISTENER.into(),
            service: format!("{namespace}/{}", seed.name),
            port: APP_PORT,
        }),
    }
}

fn placement_issue(id: &str) -> Option<PlacementIssue> {
    let (status, message) = match id {
        "api" => (
            Status::Err,
            "apply rejected: spec.serviceSpec.resources.replicas: max must be ≥ min · retrying in 30s",
        ),
        "frontend-stg" => (
            Status::Progress,
            "env secret pending (revision 4 not staged yet)",
        ),
        "docs" => (Status::Progress, "applying"),
        _ => return None,
    };
    Some(PlacementIssue {
        status,
        message: message.into(),
    })
}

pub(crate) fn app_ref(id: &str) -> ResourceRef {
    ResourceRef {
        cluster: ClusterId::new(CLUSTER),
        kind: "Application".into(),
        namespace: Some(APP_NAMESPACE.into()),
        name: id.into(),
    }
}

fn controller_objects(
    namespace: &str,
    name: &str,
    replicas: &str,
    deployment: Status,
) -> Vec<ControllerObject> {
    let object = |status, kind: &str, name: Option<String>, detail: String| ControllerObject {
        status,
        kind: kind.into(),
        namespace: namespace.into(),
        name,
        detail,
    };
    let named = Some(name.to_owned());
    vec![
        object(
            deployment,
            "Deployment",
            named.clone(),
            format!("{replicas} ready"),
        ),
        object(Status::Ok, "Service", named.clone(), format!(":{APP_PORT}")),
        object(Status::Ok, "HTTPRoute", named.clone(), "Accepted".into()),
        object(
            Status::Ok,
            "Secret",
            Some(format!("{name}-env")),
            format!("revision {ENV_SECRET_REVISION}"),
        ),
        object(Status::Ok, "ServiceAccount", named, String::new()),
        object(
            Status::Ok,
            "NetworkPolicy",
            None,
            "workspace isolation + gateway ingress".into(),
        ),
    ]
}

struct BuildSeed {
    id: &'static str,
    service: &'static str,
    workspace: (&'static str, &'static str),
    phase: BuildPhase,
    started: &'static str,
    took_secs: u64,
    image: Option<(&'static str, u8)>,
    buildkit: Status,
}

fn builds() -> Builds {
    let seeds = [
        BuildSeed {
            id: "0192f3a1",
            service: "frontend",
            workspace: WEB,
            phase: BuildPhase::Failed,
            started: "21:13",
            took_secs: 108,
            image: None,
            buildkit: Status::Err,
        },
        BuildSeed {
            id: "0192f3c7",
            service: "docs",
            workspace: WEB,
            phase: BuildPhase::Running,
            started: "21:14",
            took_secs: 42,
            image: None,
            buildkit: Status::Progress,
        },
        BuildSeed {
            id: "0192f39c",
            service: "checkout",
            workspace: PAYMENTS,
            phase: BuildPhase::Succeeded,
            started: "21:07",
            took_secs: 131,
            image: Some((APP_DIGEST, 81)),
            buildkit: Status::Ok,
        },
        BuildSeed {
            id: "0192f388",
            service: "auth",
            workspace: PAYMENTS,
            phase: BuildPhase::Succeeded,
            started: "20:52",
            took_secs: 58,
            image: Some((AUTH_DIGEST, 96)),
            buildkit: Status::Ok,
        },
        BuildSeed {
            id: "0192f301",
            service: "api",
            workspace: CORE,
            phase: BuildPhase::Canceled,
            started: "20:31",
            took_secs: 12,
            image: None,
            buildkit: Status::Offline,
        },
    ];
    Builds {
        namespace: BUILD_NAMESPACE.into(),
        pool: Some(BUILD_POOL.into()),
        retention: Some(BUILD_RETENTION),
        builds: seeds.iter().map(build).collect(),
    }
}

fn build(seed: &BuildSeed) -> Build {
    let name = format!("build-{}", seed.id);
    let took = Duration::from_secs(seed.took_secs);
    let mut build = Build {
        labels: BTreeMap::new(),
        resource: ResourceRef {
            cluster: ClusterId::new(CLUSTER),
            kind: "Build".into(),
            namespace: Some(BUILD_NAMESPACE.into()),
            name: name.clone(),
        },
        id: seed.id.into(),
        service: seed.service.into(),
        workspace: workspace(seed.workspace),
        phase: seed.phase,
        started: Some(seed.started.into()),
        took: Some(took),
        image_digest: seed.image.map(|(digest, _)| digest.into()),
        cache_hit_percent: seed.image.map(|(_, cache)| cache),
        job_uid: Uid::default(),
        job: Some(name),
        node: Some(BUILD_NODE.into()),
        fetch: Status::Ok,
        buildkit: seed.buildkit,
        log: Vec::new(),
    };
    build.log = build_log(&build);
    build
}

fn build_log(build: &Build) -> Vec<BuildLogLine> {
    let plain = |text: &str| BuildLogLine::new(LogTone::Plain, text);
    let dim = |text: &str| BuildLogLine::new(LogTone::Dim, text);
    let error = |text: &str| BuildLogLine::new(LogTone::Error, text);
    match build.phase {
        BuildPhase::Failed => vec![
            dim("fetch: source 0192f3a1.tar.gz (2.4 MiB) ok"),
            plain("#1 [internal] load build definition from Dockerfile"),
            plain("#2 [internal] load metadata for docker.io/oven/bun:1.2-alpine"),
            plain("#3 importing cache manifest from loco-registry:5000/acme/frontend:cache"),
            dim("#4 [1/5] FROM docker.io/oven/bun:1.2-alpine  CACHED"),
            dim("#5 [2/5] COPY package.json bun.lock ./  CACHED"),
            plain("#6 [3/5] RUN bun install --frozen-lockfile"),
            error("#6 1.204 error: lockfile had changes, but lockfile is frozen"),
            error(
                "#6 ERROR: process \"/bin/sh -c bun install --frozen-lockfile\" did not complete successfully: exit code: 1",
            ),
            error("ERROR: failed to solve: exit code: 1"),
        ],
        BuildPhase::Running => vec![
            dim("fetch: source 0192f3c7.tar.gz (880 KiB) ok"),
            plain("#1 [internal] load build definition from Dockerfile"),
            plain("#4 [2/4] RUN npm ci"),
            plain("#4 38.2 added 412 packages in 37s"),
        ],
        BuildPhase::Canceled => vec![
            dim("fetch: source 0192f301.tar.gz ok"),
            dim("canceled by loco builds cancel"),
        ],
        BuildPhase::Succeeded => vec![
            dim("fetch: source ok"),
            plain("#9 exporting to image"),
            plain(&format!(
                "#9 pushing layers to {REGISTRY}/{}",
                build.service
            )),
            plain(&format!("#9 DONE {}", build.took_label())),
        ],
        BuildPhase::Pending => Vec::new(),
        BuildPhase::Unknown => Vec::new(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Settle {
    Restarted(ResourceRef, AppPhase),
    Canceled(ResourceRef),
}

pub fn apply(snapshot: &mut Snapshot, command: &LocoCommand) -> Option<Settle> {
    match command {
        LocoCommand::RestartApplication(app) => {
            let application = find_app(snapshot, &app.target)?;
            let previous = std::mem::replace(&mut application.phase, AppPhase::Deploying);
            Some(Settle::Restarted(app.target.clone(), previous))
        }
        LocoCommand::CancelBuild(build) => {
            let found =
                find_build(snapshot, &build.target).filter(|found| found.phase.is_active())?;
            found
                .log
                .push(BuildLogLine::new(LogTone::Dim, CANCEL_REQUESTED));
            Some(Settle::Canceled(build.target.clone()))
        }
    }
}

pub fn settle(snapshot: &mut Snapshot, settle: &Settle) {
    match settle {
        Settle::Restarted(app, phase) => {
            if let Some(application) = find_app(snapshot, app) {
                application.phase = *phase;
            }
        }
        Settle::Canceled(build) => {
            if let Some(found) = find_build(snapshot, build) {
                found.phase = BuildPhase::Canceled;
                found.buildkit = Status::Offline;
                found
                    .log
                    .push(BuildLogLine::new(LogTone::Dim, CANCELED_LINE));
            }
        }
    }
}

fn find_app<'a>(snapshot: &'a mut Snapshot, app: &ResourceRef) -> Option<&'a mut Application> {
    snapshot
        .loco
        .get_mut(&app.cluster)?
        .applications
        .find_mut(app)
}

fn find_build<'a>(snapshot: &'a mut Snapshot, build: &ResourceRef) -> Option<&'a mut Build> {
    snapshot
        .loco
        .get_mut(&build.cluster)?
        .builds
        .find_mut(build)
}

#[cfg(test)]
mod tests {
    use flock_core::Seen;

    use super::*;

    fn snapshot() -> Snapshot {
        Snapshot {
            loco: clusters(),
            ..Snapshot::default()
        }
    }

    fn cluster(snapshot: &Snapshot) -> Option<&LocoCluster> {
        snapshot.loco.get(&ClusterId::new(CLUSTER))
    }

    #[test]
    fn applications_reproduce_the_design() {
        let snapshot = snapshot();
        let Some(loco) = cluster(&snapshot) else {
            panic!("prod-us-east has Loco");
        };
        let rows: Vec<String> = loco
            .applications
            .iter()
            .map(|(ws, a)| {
                format!(
                    "{} {} {} {} {} {}",
                    ws.label(),
                    a.name,
                    a.environment,
                    a.phase.label(),
                    a.replicas(),
                    a.placement.short()
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                "acme / payments checkout production Failed 0/2 r14",
                "acme / payments checkout-worker production Ready 2/2 r9",
                "acme / payments auth production Ready 3/3 r31",
                "acme / web frontend production Ready 4/4 r22",
                "acme / web frontend staging Deploying 1/2 r7",
                "acme / web docs production Deploying 1/1 r3",
                "beta / core api production Ready 2/2 r21 → r22",
                "beta / core jobs production Ready 1/1 r5",
            ]
        );
        let (_, checkout) = loco.applications.iter().next().unzip();
        let image = checkout
            .and_then(|a| a.image.as_deref())
            .map(flock_core::loco::short_digest);
        assert_eq!(
            image.as_deref(),
            Some("loco-registry:5000/acme/checkout@sha256:9e1c…44d0")
        );
        let route = checkout
            .and_then(|a| a.route.as_ref())
            .map(AppRoute::summary);
        assert_eq!(
            route.as_deref(),
            Some("checkout.onloco.app → Gateway eg · https-apps-0 → ws-0192c3e7/checkout:8080")
        );
    }

    #[test]
    fn builds_reproduce_the_design() {
        let snapshot = snapshot();
        let Some(loco) = cluster(&snapshot) else {
            panic!("prod-us-east has Loco");
        };
        let rows: Vec<String> = loco
            .builds
            .builds
            .iter()
            .map(|b| {
                format!(
                    "{} {} {} {} {} {} {}",
                    b.id,
                    b.service,
                    b.workspace.label(),
                    b.phase.label(),
                    b.started.as_deref().unwrap_or_default(),
                    b.took_label(),
                    b.image_label()
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                "0192f3a1 frontend acme / web Failed 21:13 1m48s —",
                "0192f3c7 docs acme / web Running 21:14 0m42s building…",
                "0192f39c checkout acme / payments Succeeded 21:07 2m11s sha256:9e1c…44d0 · cache 81%",
                "0192f388 auth acme / payments Succeeded 20:52 0m58s sha256:51ab…0f2e · cache 96%",
                "0192f301 api beta / core Canceled 20:31 0m12s —",
            ]
        );
        let last = loco.builds.builds.get(2).and_then(|b| b.log.last());
        assert_eq!(last.map(|l| l.text.as_str()), Some("#9 DONE 2m11s"));
        assert_eq!(
            loco.builds.subtitle(),
            "loco-builds · pool=builds · kept 24h"
        );
    }

    #[test]
    fn platform_reproduces_the_design() {
        let snapshot = snapshot();
        let Some(loco) = cluster(&snapshot) else {
            panic!("prod-us-east has Loco");
        };
        let summary = loco.applications.placement_summary();
        assert_eq!(summary.at_desired, 211);
        assert_eq!(summary.badges, [(Status::Progress, 2), (Status::Err, 1)]);
        let placements: Vec<String> = loco
            .applications
            .unsettled()
            .iter()
            .map(|(ws, app)| {
                let message = app.placement.issue.as_ref().map(|i| i.message.as_str());
                format!(
                    "{} {} {}",
                    app.name,
                    ws.label(),
                    message.unwrap_or_default()
                )
            })
            .collect();
        assert_eq!(
            placements,
            [
                "api beta / core apply rejected: spec.serviceSpec.resources.replicas: max must be ≥ min · retrying in 30s",
                "frontend acme / web env secret pending (revision 4 not staged yet)",
                "docs acme / web applying",
            ]
        );
        let queue = loco.builds.queue();
        assert_eq!((queue.running, queue.queued), (1, 0));
        assert_eq!(loco.platform.components.len(), 10);
        assert_eq!(loco.platform.log.len(), 6);
        assert_eq!(loco.platform.subtitle(), "loco-system · images sha-8c1f2e7");
    }

    #[test]
    fn routes_reproduce_the_design() {
        let snapshot = snapshot();
        let Some(loco) = cluster(&snapshot) else {
            panic!("prod-us-east has Loco");
        };
        let routes = &loco.routes;
        assert_eq!(
            routes.subtitle(),
            "Gateway loco-system/eg · Envoy 3/3 · HTTP/3 on"
        );
        assert_eq!((routes.total, routes.problems()), (44, 2));
        let problems: Vec<String> = routes
            .ranked(
                flock_core::loco::RouteFilter::Problems,
                &flock_core::filter::TextFilter::default(),
            )
            .iter()
            .map(|r| format!("{} {} {}", r.host_label(), r.ready_label(), r.detail))
            .collect();
        assert_eq!(
            problems,
            [
                "checkout.onloco.app 0/2 Accepted · no ready endpoints (app failing) → 503",
                "old-landing.onloco.app — ResolvedRefs=False · BackendNotFound",
            ]
        );
        let linked = routes
            .routes
            .iter()
            .filter_map(|r| r.application.as_ref())
            .all(|app| loco.applications.find(app).is_some());
        assert!(linked);
    }

    #[test]
    fn restart_deploys_then_returns_to_the_previous_phase() {
        let mut snapshot = snapshot();
        let app = ResourceRef {
            cluster: ClusterId::new(CLUSTER),
            kind: "Application".into(),
            namespace: Some(APP_NAMESPACE.into()),
            name: "auth".into(),
        };
        let phase = |s: &Snapshot| {
            cluster(s)
                .and_then(|c| c.applications.find(&app))
                .map(|(_, a)| a.phase)
        };
        let command = LocoCommand::RestartApplication(Seen::new(app.clone(), Uid::default()));
        let pending = apply(&mut snapshot, &command);
        assert_eq!(
            pending,
            Some(Settle::Restarted(app.clone(), AppPhase::Ready))
        );
        assert_eq!(phase(&snapshot), Some(AppPhase::Deploying));
        if let Some(pending) = pending {
            settle(&mut snapshot, &pending);
        }
        assert_eq!(phase(&snapshot), Some(AppPhase::Ready));
        let ghost = LocoCommand::RestartApplication(Seen::new(
            ResourceRef {
                name: "ghost".into(),
                ..app.clone()
            },
            Uid::default(),
        ));
        assert_eq!(apply(&mut snapshot, &ghost), None);
    }

    #[test]
    fn cancel_stops_only_active_builds() {
        let mut snapshot = snapshot();
        let reference = |id: &str| ResourceRef {
            cluster: ClusterId::new(CLUSTER),
            kind: "Build".into(),
            namespace: Some(BUILD_NAMESPACE.into()),
            name: format!("build-{id}"),
        };
        let failed = LocoCommand::CancelBuild(Seen::new(reference("0192f3a1"), Uid::default()));
        assert_eq!(apply(&mut snapshot, &failed), None);
        let running = LocoCommand::CancelBuild(Seen::new(reference("0192f3c7"), Uid::default()));
        let pending = apply(&mut snapshot, &running);
        assert_eq!(pending, Some(Settle::Canceled(reference("0192f3c7"))));
        if let Some(pending) = pending {
            settle(&mut snapshot, &pending);
        }
        let phases: Vec<_> = cluster(&snapshot)
            .map(|c| c.builds.builds.iter().map(|b| b.phase).take(2).collect())
            .unwrap_or_default();
        assert_eq!(phases, [BuildPhase::Failed, BuildPhase::Canceled]);
        let build = cluster(&snapshot).and_then(|c| c.builds.builds.get(1));
        assert_eq!(build.map(|b| b.buildkit), Some(Status::Offline));
        let tail: Vec<_> = build
            .map(|b| {
                b.log
                    .iter()
                    .rev()
                    .take(2)
                    .map(|l| l.text.as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(tail, [CANCELED_LINE, CANCEL_REQUESTED]);
    }
}
