use std::collections::BTreeMap;
use std::time::Duration;

use flock_core::loco::{
    AgentLogLine, AgentSync, AppPhase, AppRoute, Application, Applications, Build, BuildLogLine,
    BuildPhase, Builds, Component, ControllerObject, Degraded, HealthCheck, Listener, LocoCluster,
    Placement, Platform, Resources, Rollout, Route, Routes, Workspace, WorkspaceApps, short_digest,
};
use flock_core::{ClusterId, Counts, Page, ResourceRef, Status, Uid};
use flock_loco::applications::{
    ApplicationModel, ObjectRelationship, RelatedObject, application_model, related_object,
};
use flock_loco::builds::{BuildModel, build_model};
use flock_loco::generated::{
    ApplicationStatusPhase, BuildStatusPhase, applications::Application as LocoApplication,
    builds::Build as LocoBuild,
};
use flock_loco::platform::{
    Health, WorkloadHealth, component_catalog, component_health, daemonset_health,
    deployment_health, statefulset_health,
};
use flock_loco::routes::{
    Gateway, HttpRoute, ListenerModel, RouteModel, RouteProblem, gateway_listeners, rank_routes,
    route_model,
};
use flock_loco::{
    APPLICATION_NAME, APPLICATION_NAMESPACE, BUILD_ID, BUILD_NAMESPACE, WORKSPACE_PREFIX,
};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::{ContainerStatus, Pod, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

pub const LOCO_NAMESPACE: &str = "loco-system";
pub const GATEWAY_NAME: &str = flock_loco::routes::GATEWAY_NAME;
pub const AGENT_DEPLOYMENT: &str = "loco-core-agent";
pub const FETCH_CONTAINER: &str = "fetch";
pub const BUILD_CONTAINER: &str = "build";
pub const PUSH_CONTAINER: &str = "push";
pub const BUILD_CONTAINERS: [&str; 3] = [FETCH_CONTAINER, BUILD_CONTAINER, PUSH_CONTAINER];
const ENVOY_PROXY: &str = "envoy-proxy";
const DEGRADED: &str = "Degraded";
const CONDITION_TRUE: &str = "True";
const MISSING: &str = "—";
const SHARED_POLICY: &str = "workspace isolation";
const NOT_FOUND: &str = "not found";
const ACCEPTED: &str = "Accepted";
const NO_READY_ENDPOINTS: &str = "no ready endpoints";
const BACKEND_NOT_FOUND: &str = "backend Service not found";
const NOT_ACCEPTED: &str = "not accepted by the gateway yet";
const REJECTED: &str = "rejected by the gateway";
const STARTED_FORMAT: &str = "%H:%M";
const HTTPS: &str = "HTTPS";
const WILDCARD: &str = "*.";
const TLS_SEPARATOR: &str = ", ";
const ERROR_MARKERS: [&str; 2] = ["ERROR", "error:"];
const FAILING_WAIT_REASONS: [&str; 4] = [
    "ImagePullBackOff",
    "ErrImagePull",
    "CreateContainerConfigError",
    "CrashLoopBackOff",
];

#[derive(Debug, Default)]
pub struct Inputs {
    pub cluster: ClusterId,
    pub applications: Vec<LocoApplication>,
    pub builds: Vec<LocoBuild>,
    pub gateways: Vec<Gateway>,
    pub routes: Vec<HttpRoute>,
    pub endpoint_slices: Vec<EndpointSlice>,
    pub services: Vec<Service>,
    pub deployments: Vec<Deployment>,
    pub daemonsets: Vec<DaemonSet>,
    pub statefulsets: Vec<StatefulSet>,
    pub jobs: Vec<Job>,
    pub pods: Vec<Pod>,
    pub secrets: Vec<ObjectMeta>,
    pub service_accounts: Vec<ObjectMeta>,
    pub network_policies: Vec<ObjectMeta>,
    pub workloads_watched: bool,
    pub agent: Option<WorkloadHealth>,
    pub builds_enabled: Option<bool>,
    pub agent_log: Vec<String>,
    pub container_logs: BTreeMap<(String, String), Vec<String>>,
}

pub fn loco_cluster(inputs: &Inputs, now: Timestamp, zone: &TimeZone) -> LocoCluster {
    let gateway = gateway(inputs);
    let gateway_namespace = gateway.map_or(LOCO_NAMESPACE, |gateway| {
        gateway
            .metadata
            .namespace
            .as_deref()
            .unwrap_or(LOCO_NAMESPACE)
    });
    let listeners = gateway
        .and_then(|gateway| gateway_listeners(gateway).ok())
        .unwrap_or_default();
    let route_models = route_models(inputs, gateway_namespace);
    let platform = platform(inputs, zone);
    let envoy_ready = platform
        .components
        .iter()
        .find(|component| component.name == ENVOY_PROXY)
        .and_then(|component| component.ready);
    LocoCluster {
        applications: applications(inputs, &route_models, &listeners, now),
        builds: builds(inputs, now, zone),
        routes: routes(inputs, gateway, &listeners, &route_models, envoy_ready),
        platform,
    }
}

fn gateway(inputs: &Inputs) -> Option<&Gateway> {
    let named = |gateway: &&Gateway| gateway.metadata.name.as_deref() == Some(GATEWAY_NAME);
    let in_loco =
        |gateway: &&Gateway| gateway.metadata.namespace.as_deref() == Some(LOCO_NAMESPACE);
    inputs
        .gateways
        .iter()
        .filter(named)
        .find(in_loco)
        .or_else(|| inputs.gateways.iter().find(named))
}

fn route_models<'a>(
    inputs: &'a Inputs,
    gateway_namespace: &str,
) -> Vec<(RouteModel, &'a HttpRoute)> {
    inputs
        .routes
        .iter()
        .filter_map(|route| {
            let model = route_model(
                route,
                gateway_namespace,
                &inputs.services,
                &inputs.endpoint_slices,
            );
            match model {
                Ok(model) => Some((model, route)),
                Err(error) => {
                    tracing::debug!(%error, "skipping an unreadable HTTPRoute");
                    None
                }
            }
        })
        .collect()
}

pub fn health_status(health: Health) -> Status {
    match health {
        Health::Healthy => Status::Ok,
        Health::Progressing => Status::Progress,
        Health::Unhealthy => Status::Err,
        Health::Suspended => Status::Suspended,
        Health::Unknown => Status::Offline,
    }
}

fn app_phase(phase: &ApplicationStatusPhase) -> AppPhase {
    match phase {
        ApplicationStatusPhase::Idle => AppPhase::Idle,
        ApplicationStatusPhase::Deploying => AppPhase::Deploying,
        ApplicationStatusPhase::Ready => AppPhase::Ready,
        ApplicationStatusPhase::Failed => AppPhase::Failed,
        ApplicationStatusPhase::Unknown => AppPhase::Unknown,
    }
}

fn build_phase(phase: &BuildStatusPhase) -> BuildPhase {
    match phase {
        BuildStatusPhase::Pending => BuildPhase::Pending,
        BuildStatusPhase::Running => BuildPhase::Running,
        BuildStatusPhase::Succeeded => BuildPhase::Succeeded,
        BuildStatusPhase::Failed => BuildPhase::Failed,
        BuildStatusPhase::Canceled => BuildPhase::Canceled,
        BuildStatusPhase::Unknown => BuildPhase::Unknown,
    }
}

fn elapsed_since(then: &str, now: Timestamp) -> Option<Duration> {
    let then: Timestamp = then.parse().ok()?;
    let seconds = now.as_second().saturating_sub(then.as_second());
    u64::try_from(seconds).ok().map(Duration::from_secs)
}

fn reference(cluster: &ClusterId, kind: &str, namespace: &str, name: &str) -> ResourceRef {
    ResourceRef {
        cluster: cluster.clone(),
        kind: kind.to_owned(),
        namespace: Some(namespace.to_owned()),
        name: name.to_owned(),
    }
}

fn uid_of(metadata: &ObjectMeta) -> Uid {
    Uid::new(metadata.uid.clone().unwrap_or_default())
}

fn annotation<'a>(metadata: &'a ObjectMeta, key: &str) -> Option<&'a str> {
    metadata.annotations.as_ref()?.get(key).map(String::as_str)
}

fn label<'a>(metadata: &'a ObjectMeta, key: &str) -> Option<&'a str> {
    metadata.labels.as_ref()?.get(key).map(String::as_str)
}

fn applications(
    inputs: &Inputs,
    routes: &[(RouteModel, &HttpRoute)],
    listeners: &[ListenerModel],
    now: Timestamp,
) -> Applications {
    let mut groups: BTreeMap<String, Vec<Application>> = BTreeMap::new();
    for resource in &inputs.applications {
        let model = match application_model(resource) {
            Ok(model) => model,
            Err(error) => {
                tracing::debug!(%error, "skipping an unreadable Application");
                continue;
            }
        };
        let application = application(inputs, resource, &model, routes, listeners, now);
        groups
            .entry(model.workspace_namespace.clone())
            .or_default()
            .push(application);
    }
    let total = groups.values().map(Vec::len).sum();
    let workspaces = groups
        .into_iter()
        .map(|(namespace, mut applications)| {
            applications.sort_by(|left, right| left.name.cmp(&right.name));
            WorkspaceApps {
                workspace: Workspace {
                    namespace,
                    name: None,
                },
                applications,
            }
        })
        .collect();
    Applications { total, workspaces }
}

fn application(
    inputs: &Inputs,
    resource: &LocoApplication,
    model: &ApplicationModel,
    routes: &[(RouteModel, &HttpRoute)],
    listeners: &[ListenerModel],
    now: Timestamp,
) -> Application {
    let status = resource.status.as_ref();
    let degraded = status
        .and_then(|status| status.conditions.as_ref())
        .and_then(|conditions| {
            conditions
                .iter()
                .find(|condition| condition.type_ == DEGRADED && condition.status == CONDITION_TRUE)
        })
        .map(|condition| {
            let seconds = now
                .as_second()
                .saturating_sub(condition.last_transition_time.0.as_second());
            Degraded {
                reason: condition.reason.clone(),
                message: condition.message.clone(),
                since: u64::try_from(seconds).ok().map(Duration::from_secs),
            }
        });
    let rollout = model.deployment_id.as_ref().map(|deployment_id| Rollout {
        deployment_id: deployment_id.clone(),
        age: status
            .and_then(|status| status.started_at.as_deref())
            .and_then(|started| elapsed_since(started, now)),
        build: None,
    });
    let resources = model.resources.as_ref().map(|resources| Resources {
        cpu: resources.cpu.clone(),
        memory: resources.memory.clone(),
        min_replicas: resources.replicas.min,
        max_replicas: resources.replicas.max,
        cpu_target: resources
            .scalers
            .as_ref()
            .filter(|scalers| scalers.enabled.unwrap_or(true))
            .and_then(|scalers| scalers.cpu_target),
    });
    let health_check = model.health_check.as_ref().map(|check| HealthCheck {
        path: check.path.clone(),
        interval_secs: check.interval,
        timeout_secs: check.timeout,
        fail_threshold: check.fail_threshold,
    });
    let (objects, route) = controller_objects(inputs, resource, model, routes, listeners);
    let workload_uid = inputs
        .deployments
        .iter()
        .find(|deployment| {
            deployment.metadata.namespace.as_deref() == Some(model.workspace_namespace.as_str())
                && deployment.metadata.name.as_deref() == Some(model.workload_name.as_str())
        })
        .map(|deployment| uid_of(&deployment.metadata))
        .unwrap_or_default();
    let labels = resource.metadata.labels.clone().unwrap_or_default();
    Application {
        labels,
        resource: reference(
            &inputs.cluster,
            &model.application.kind,
            &model.application.namespace,
            &model.application.name,
        ),
        workload_uid,
        name: model.application.name.clone(),
        environment: model.environment.clone().unwrap_or_default(),
        region: resource.spec.region.clone(),
        phase: app_phase(&model.phase),
        ready_replicas: model.ready_replicas,
        desired_replicas: model.desired_replicas,
        placement: Placement {
            desired: model.placement.desired_revision,
            applied: model.placement.applied_revision,
            issue: None,
        },
        hostname: model.hostname.clone(),
        degraded,
        image: model.image.clone(),
        rollout,
        resources,
        health_check,
        objects,
        route,
    }
}

fn related(
    resource: &LocoApplication,
    kind: &str,
    metadata: &ObjectMeta,
    namespace: &str,
) -> Option<RelatedObject> {
    if metadata.namespace.as_deref() != Some(namespace) {
        return None;
    }
    related_object(resource, kind, metadata).ok().flatten()
}

fn owned_by_application(found: &RelatedObject) -> bool {
    found.relationship == ObjectRelationship::Application
}

fn controller_objects(
    inputs: &Inputs,
    resource: &LocoApplication,
    model: &ApplicationModel,
    routes: &[(RouteModel, &HttpRoute)],
    listeners: &[ListenerModel],
) -> (Vec<ControllerObject>, Option<AppRoute>) {
    let namespace = model.workspace_namespace.as_str();
    let object = |status, related: RelatedObject, detail: String| ControllerObject {
        status,
        kind: related.reference.kind,
        namespace: related.reference.namespace,
        name: Some(related.reference.name),
        detail,
    };
    let mut objects = Vec::new();
    for deployment in &inputs.deployments {
        let Some(found) = related(resource, "Deployment", &deployment.metadata, namespace)
            .filter(owned_by_application)
        else {
            continue;
        };
        let (status, detail) = deployment_health(deployment).map_or_else(
            |_| (Status::Offline, String::new()),
            |health| {
                let detail = match (health.ready, health.desired) {
                    (Some(ready), Some(desired)) => format!("{ready}/{desired} ready"),
                    _ => String::new(),
                };
                (health_status(health.health), detail)
            },
        );
        objects.push(object(status, found, detail));
    }
    for service in &inputs.services {
        let Some(found) =
            related(resource, "Service", &service.metadata, namespace).filter(owned_by_application)
        else {
            continue;
        };
        let port = service
            .spec
            .as_ref()
            .and_then(|spec| spec.ports.as_ref())
            .and_then(|ports| ports.first())
            .map(|port| format!(":{}", port.port))
            .unwrap_or_default();
        objects.push(object(Status::Ok, found, port));
    }
    let mut app_route = None;
    for (route, raw) in routes {
        let Some(found) =
            related(resource, "HTTPRoute", &raw.metadata, namespace).filter(owned_by_application)
        else {
            continue;
        };
        let (status, detail) = route_status(route);
        let detail = if status == Status::Ok {
            ACCEPTED.to_owned()
        } else {
            detail
        };
        app_route = app_route.or_else(|| application_route(route, listeners));
        objects.push(object(status, found, detail));
    }
    let owned = [
        ("Secret", &inputs.secrets),
        ("ServiceAccount", &inputs.service_accounts),
    ];
    for (kind, metadata) in owned {
        for meta in metadata {
            let Some(found) = related(resource, kind, meta, namespace).filter(owned_by_application)
            else {
                continue;
            };
            objects.push(object(Status::Ok, found, String::new()));
        }
    }
    let policies = inputs
        .network_policies
        .iter()
        .filter_map(|meta| related(resource, "NetworkPolicy", meta, namespace));
    let (owned_policies, shared_policies): (Vec<_>, Vec<_>) =
        policies.partition(owned_by_application);
    for found in owned_policies {
        objects.push(object(Status::Ok, found, String::new()));
    }
    if let Some(shared) = shared_policies.into_iter().next() {
        let mut policy = object(Status::Ok, shared, SHARED_POLICY.to_owned());
        policy.name = None;
        objects.push(policy);
    }
    (objects, app_route)
}

fn host_matches(pattern: &str, host: &str) -> bool {
    match pattern.strip_prefix(WILDCARD) {
        Some(suffix) => host
            .strip_suffix(suffix)
            .is_some_and(|prefix| prefix.ends_with('.') && prefix.len() > 1),
        None => pattern == host,
    }
}

fn application_route(route: &RouteModel, listeners: &[ListenerModel]) -> Option<AppRoute> {
    let host = route.hostnames.first()?;
    let backend = route.backends.first()?;
    let matching: Vec<&ListenerModel> = listeners
        .iter()
        .filter(|listener| {
            listener
                .hostname
                .as_deref()
                .is_none_or(|pattern| host_matches(pattern, host))
        })
        .collect();
    let listener = matching
        .iter()
        .find(|listener| listener.protocol == HTTPS)
        .or_else(|| matching.first())
        .map_or_else(|| MISSING.to_owned(), |listener| listener.name.clone());
    Some(AppRoute {
        host: host.clone(),
        gateway: GATEWAY_NAME.to_owned(),
        listener,
        service: format!("{}/{}", backend.service.namespace, backend.service.name),
        port: backend
            .port
            .and_then(|port| u16::try_from(port).ok())
            .unwrap_or_default(),
    })
}

fn route_status(route: &RouteModel) -> (Status, String) {
    let detail = |fallback: &str| {
        if route.messages.is_empty() {
            fallback.to_owned()
        } else {
            route.messages.join(" · ")
        }
    };
    match route.problem {
        RouteProblem::BackendNotFound => (Status::Err, detail(BACKEND_NOT_FOUND)),
        RouteProblem::NoReadyEndpoints => (Status::Warn, NO_READY_ENDPOINTS.to_owned()),
        RouteProblem::Rejected => (Status::Err, detail(REJECTED)),
        RouteProblem::Unknown => (Status::Offline, detail(NOT_ACCEPTED)),
        RouteProblem::None => (Status::Ok, String::new()),
    }
}

fn routes(
    inputs: &Inputs,
    gateway: Option<&Gateway>,
    listeners: &[ListenerModel],
    models: &[(RouteModel, &HttpRoute)],
    envoy_ready: Option<(u32, u32)>,
) -> Routes {
    let gateway_label = gateway.map_or_else(
        || format!("{LOCO_NAMESPACE}/{GATEWAY_NAME}"),
        |gateway| {
            format!(
                "{}/{}",
                gateway
                    .metadata
                    .namespace
                    .as_deref()
                    .unwrap_or(LOCO_NAMESPACE),
                gateway.metadata.name.as_deref().unwrap_or(GATEWAY_NAME)
            )
        },
    );
    let listeners = listeners
        .iter()
        .map(|listener| Listener {
            status: if listener.problems.is_empty() {
                Status::Ok
            } else {
                Status::Warn
            },
            name: listener.name.clone(),
            hostname: listener.hostname.clone(),
            port: u16::try_from(listener.port).unwrap_or_default(),
            http3: false,
            routes: listener
                .attached_routes
                .and_then(|routes| u32::try_from(routes).ok()),
            tls: listener
                .certificates
                .iter()
                .map(|certificate| certificate.name.as_str())
                .collect::<Vec<_>>()
                .join(TLS_SEPARATOR),
        })
        .collect();
    let mut ranked: Vec<RouteModel> = models.iter().map(|(model, _)| model.clone()).collect();
    rank_routes(&mut ranked);
    let routes: Vec<Route> = ranked
        .iter()
        .filter_map(|model| {
            let (_, raw) = models
                .iter()
                .find(|(found, _)| found.route == model.route)?;
            Some(route(&inputs.cluster, model, &raw.metadata))
        })
        .collect();
    Routes {
        gateway: gateway_label,
        envoy_ready,
        http3: false,
        listeners,
        total: routes.len(),
        routes,
    }
}

fn route(cluster: &ClusterId, model: &RouteModel, metadata: &ObjectMeta) -> Route {
    let (status, detail) = route_status(model);
    let backend = model.backends.first().map_or_else(
        || MISSING.to_owned(),
        |backend| match backend.port {
            Some(port) => format!("{}:{port}", backend.service.name),
            None => backend.service.name.clone(),
        },
    );
    let counted: Vec<(usize, usize)> = model
        .backends
        .iter()
        .filter_map(|backend| backend.ready_endpoints.zip(backend.total_endpoints))
        .collect();
    let ready = (!counted.is_empty()).then(|| {
        let ready: usize = counted.iter().map(|(ready, _)| ready).sum();
        let total: usize = counted.iter().map(|(_, total)| total).sum();
        (
            u32::try_from(ready).unwrap_or(u32::MAX),
            u32::try_from(total).unwrap_or(u32::MAX),
        )
    });
    let application = annotation(metadata, APPLICATION_NAME)
        .zip(annotation(metadata, APPLICATION_NAMESPACE))
        .map(|(name, namespace)| reference(cluster, "Application", namespace, name));
    let labels = metadata.labels.clone().unwrap_or_default();
    Route {
        labels,
        status,
        name: model.route.name.clone(),
        host: model.hostnames.first().cloned(),
        backend,
        ready,
        detail,
        application,
    }
}

fn builds(inputs: &Inputs, now: Timestamp, zone: &TimeZone) -> Builds {
    let chrono_now = chrono::DateTime::from_timestamp(now.as_second(), 0).unwrap_or_default();
    let jobs: Vec<Job> = inputs
        .jobs
        .iter()
        .filter(|job| job.metadata.namespace.as_deref() == Some(BUILD_NAMESPACE))
        .cloned()
        .collect();
    let pods: Vec<Pod> = inputs
        .pods
        .iter()
        .filter(|pod| pod.metadata.namespace.as_deref() == Some(BUILD_NAMESPACE))
        .cloned()
        .collect();
    let mut ordered: Vec<&LocoBuild> = inputs.builds.iter().collect();
    ordered.sort_by(|left, right| {
        let created = |build: &LocoBuild| {
            build
                .metadata
                .creation_timestamp
                .as_ref()
                .map(|time| time.0)
        };
        created(right)
            .cmp(&created(left))
            .then_with(|| left.metadata.name.cmp(&right.metadata.name))
    });
    let builds = ordered
        .into_iter()
        .filter_map(|build| match build_model(build, &jobs, &pods, chrono_now) {
            Ok(model) => Some(build_row(inputs, build, &model, &jobs, &pods, zone)),
            Err(error) => {
                tracing::debug!(%error, "skipping an unreadable Build");
                None
            }
        })
        .collect();
    let retention = jobs
        .iter()
        .find_map(|job| job.spec.as_ref()?.ttl_seconds_after_finished)
        .and_then(|seconds| u64::try_from(seconds).ok())
        .map(Duration::from_secs);
    Builds {
        namespace: BUILD_NAMESPACE.to_owned(),
        pool: None,
        retention,
        builds,
    }
}

fn build_row(
    inputs: &Inputs,
    build: &LocoBuild,
    model: &BuildModel,
    jobs: &[Job],
    pods: &[Pod],
    zone: &TimeZone,
) -> Build {
    let job_uid = jobs
        .iter()
        .find(|job| {
            job.metadata.namespace == Some(model.build.namespace.clone())
                && job.metadata.name.as_deref() == Some(model.build.name.as_str())
        })
        .map(|job| uid_of(&job.metadata))
        .unwrap_or_default();
    let phase = build_phase(&model.phase);
    let pod = model.pods.last().and_then(|reference| {
        pods.iter()
            .find(|pod| pod.metadata.name.as_deref() == Some(reference.name.as_str()))
    });
    let started = build
        .status
        .as_ref()
        .and_then(|status| status.started_at.as_deref())
        .and_then(|started| started.parse::<Timestamp>().ok())
        .map(|started| {
            started
                .to_zoned(zone.clone())
                .strftime(STARTED_FORMAT)
                .to_string()
        });
    let container = |name: &str| container_status(pod, name, phase);
    let labels = build.metadata.labels.clone().unwrap_or_default();
    Build {
        labels,
        resource: reference(
            &inputs.cluster,
            &model.build.kind,
            &model.build.namespace,
            &model.build.name,
        ),
        job_uid,
        id: build.spec.build_id.clone(),
        service: format!("{}{}", flock_loco::RESOURCE_PREFIX, model.resource_id),
        workspace: Workspace {
            namespace: format!("{WORKSPACE_PREFIX}{}", model.workspace_id),
            name: None,
        },
        phase,
        started,
        took: model.duration.and_then(|duration| duration.to_std().ok()),
        image_digest: model.image_digest.clone(),
        cache_hit_percent: None,
        job: model.jobs.first().map(|job| job.name.clone()),
        node: pod
            .and_then(|pod| pod.spec.as_ref())
            .and_then(|spec| spec.node_name.clone()),
        fetch: container(FETCH_CONTAINER),
        buildkit: container(BUILD_CONTAINER),
        log: pod.map(|pod| build_log(inputs, pod)).unwrap_or_default(),
    }
}

fn container_status(pod: Option<&Pod>, name: &str, phase: BuildPhase) -> Status {
    let statuses = pod.and_then(|pod| pod.status.as_ref()).map(|status| {
        status
            .init_container_statuses
            .iter()
            .flatten()
            .chain(status.container_statuses.iter().flatten())
            .collect::<Vec<&ContainerStatus>>()
    });
    let found = statuses
        .as_ref()
        .and_then(|statuses| statuses.iter().find(|status| status.name == name));
    let Some(found) = found else {
        return match phase {
            BuildPhase::Succeeded => Status::Ok,
            BuildPhase::Pending => Status::Progress,
            BuildPhase::Running => Status::Progress,
            BuildPhase::Failed => Status::Offline,
            BuildPhase::Canceled => Status::Offline,
            BuildPhase::Unknown => Status::Offline,
        };
    };
    let state = found.state.as_ref();
    if let Some(terminated) = state.and_then(|state| state.terminated.as_ref()) {
        return if terminated.exit_code == 0 {
            Status::Ok
        } else {
            Status::Err
        };
    }
    let failing = state
        .and_then(|state| state.waiting.as_ref())
        .and_then(|waiting| waiting.reason.as_deref())
        .is_some_and(|reason| FAILING_WAIT_REASONS.contains(&reason));
    if failing {
        return Status::Err;
    }
    if phase.is_active() {
        Status::Progress
    } else {
        Status::Offline
    }
}

fn build_log(inputs: &Inputs, pod: &Pod) -> Vec<BuildLogLine> {
    let Some(name) = pod.metadata.name.as_deref() else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    for container in BUILD_CONTAINERS {
        let key = (name.to_owned(), container.to_owned());
        let Some(text) = inputs.container_logs.get(&key) else {
            continue;
        };
        lines.extend(
            text.iter()
                .map(|line| BuildLogLine::new(build_tone(container, line), line.clone())),
        );
    }
    lines
}

pub fn build_tone(container: &str, line: &str) -> flock_core::loco::LogTone {
    use flock_core::loco::LogTone;
    if ERROR_MARKERS.iter().any(|marker| line.contains(marker)) {
        LogTone::Error
    } else if container == FETCH_CONTAINER {
        LogTone::Dim
    } else {
        LogTone::Plain
    }
}

pub fn build_pod(pod: &Pod) -> bool {
    pod.metadata.namespace.as_deref() == Some(BUILD_NAMESPACE)
        && label(&pod.metadata, BUILD_ID).is_some()
}

fn image_version(image: &str) -> Option<String> {
    if let Some((_, digest)) = image.split_once('@') {
        return Some(short_digest(digest));
    }
    let name_start = image.rfind('/').map_or(0, |slash| slash + 1);
    let (_, tag) = image.get(name_start..)?.rsplit_once(':')?;
    Some(tag.to_owned())
}

fn platform(inputs: &Inputs, zone: &TimeZone) -> Platform {
    let mut workloads: Vec<WorkloadHealth> = Vec::new();
    workloads.extend(
        inputs
            .deployments
            .iter()
            .filter_map(|d| deployment_health(d).ok()),
    );
    workloads.extend(
        inputs
            .daemonsets
            .iter()
            .filter_map(|d| daemonset_health(d).ok()),
    );
    workloads.extend(
        inputs
            .statefulsets
            .iter()
            .filter_map(|s| statefulset_health(s).ok()),
    );
    workloads.extend(
        inputs
            .pods
            .iter()
            .filter_map(|pod| flock_loco::platform::pod_health(pod).ok()),
    );
    let agent_watched = workloads.iter().any(|workload| {
        workload.reference.namespace == LOCO_NAMESPACE
            && workload.reference.name == AGENT_DEPLOYMENT
            && workload.reference.kind == "Deployment"
    });
    if !agent_watched {
        workloads.extend(inputs.agent.clone());
    }
    let catalog = component_catalog();
    let components: Vec<Component> = catalog
        .iter()
        .map(|component| component_health(component, &workloads))
        .filter(|health| inputs.workloads_watched || !health.workloads.is_empty())
        .map(|health| {
            let found = !health.workloads.is_empty();
            let ready = found.then(|| {
                let sum = |pick: fn(&WorkloadHealth) -> Option<i32>| {
                    let total: i32 = health.workloads.iter().filter_map(pick).sum();
                    u32::try_from(total).unwrap_or_default()
                };
                (sum(|w| w.ready), sum(|w| w.desired))
            });
            Component {
                status: health_status(health.health),
                name: health.component.name.clone(),
                namespace: health.component.namespace.clone(),
                ready,
                version: health
                    .workloads
                    .iter()
                    .filter_map(|workload| workload.images.first())
                    .find_map(|image| image_version(image)),
                detail: if found {
                    String::new()
                } else {
                    NOT_FOUND.to_owned()
                },
            }
        })
        .collect();
    let agent = workloads.iter().find(|workload| {
        workload.reference.namespace == LOCO_NAMESPACE
            && workload.reference.name == AGENT_DEPLOYMENT
    });
    let ready = agent.and_then(|agent| {
        let ready = u32::try_from(agent.ready?).ok()?;
        let desired = u32::try_from(agent.desired?).ok()?;
        Some((ready, desired))
    });
    let images = agent
        .and_then(|agent| agent.images.first())
        .and_then(|image| image_version(image));
    Platform {
        namespace: LOCO_NAMESPACE.to_owned(),
        images,
        agent: AgentSync {
            ready,
            builds_enabled: inputs.builds_enabled,
            ..AgentSync::default()
        },
        components,
        log: agent_log(&inputs.agent_log, zone),
    }
}

fn agent_log(lines: &[String], zone: &TimeZone) -> Vec<AgentLogLine> {
    lines
        .iter()
        .rev()
        .map(|line| super::logs::agent_line(line, zone))
        .collect()
}

pub fn nav_counts(loco: &LocoCluster) -> Vec<(Page, Counts)> {
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let applications = &loco.applications;
    let failed = applications
        .iter()
        .filter(|(_, app)| app.phase == AppPhase::Failed)
        .count();
    let drifted = applications
        .iter()
        .filter(|(_, app)| app.phase != AppPhase::Failed && app.placement.out_of_sync())
        .count();
    let failed_builds = loco
        .builds
        .builds
        .iter()
        .filter(|build| build.phase == BuildPhase::Failed)
        .count();
    let unhealthy = loco
        .platform
        .components
        .iter()
        .filter(|component| component.status != Status::Ok)
        .count();
    vec![
        (
            Page::LocoPlatform,
            Counts::default().with_warn(count(unhealthy)),
        ),
        (
            Page::LocoApplications,
            Counts::total(count(applications.total))
                .with_err(count(failed))
                .with_warn(count(drifted)),
        ),
        (
            Page::LocoBuilds,
            Counts::default().with_err(count(failed_builds)),
        ),
        (
            Page::LocoRoutes,
            Counts::total(count(loco.routes.total)).with_warn(count(loco.routes.problems())),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use flock_core::loco::LogTone;
    use serde::de::DeserializeOwned;

    fn parse<T: DeserializeOwned>(json: &str) -> Vec<T> {
        match serde_json::from_str(json) {
            Ok(items) => items,
            Err(error) => panic!("fixture: {error}"),
        }
    }

    fn now() -> Timestamp {
        match "2026-10-12T21:10:00Z".parse() {
            Ok(now) => now,
            Err(error) => panic!("timestamp: {error}"),
        }
    }

    fn inputs() -> Inputs {
        Inputs {
            cluster: ClusterId::new("kind-fleet-b"),
            applications: parse(include_str!("fixtures/applications.json")),
            builds: parse(include_str!("fixtures/builds.json")),
            gateways: parse(include_str!("fixtures/gateways.json")),
            routes: parse(include_str!("fixtures/routes.json")),
            endpoint_slices: parse(include_str!("fixtures/endpoint_slices.json")),
            services: parse(include_str!("fixtures/services.json")),
            deployments: parse(include_str!("fixtures/deployments.json")),
            daemonsets: Vec::new(),
            statefulsets: Vec::new(),
            jobs: parse(include_str!("fixtures/jobs.json")),
            pods: parse(include_str!("fixtures/pods.json")),
            secrets: Vec::new(),
            service_accounts: Vec::new(),
            network_policies: Vec::new(),
            workloads_watched: true,
            agent: None,
            builds_enabled: Some(true),
            agent_log: vec![
                r#"{"time":"2026-10-12T21:09:01.5Z","level":"INFO","msg":"placement applied","resource":"resource-web","revision":14}"#.to_owned(),
                r#"{"time":"2026-10-12T21:09:30Z","level":"ERROR","msg":"apply failed","err":"invalid"}"#.to_owned(),
            ],
            container_logs: BTreeMap::from([
                (
                    ("build-b1-x7k2p".to_owned(), BUILD_CONTAINER.to_owned()),
                    vec!["#1 load build definition".to_owned(), "ERROR: failed to solve".to_owned()],
                ),
                (
                    ("build-b1-x7k2p".to_owned(), FETCH_CONTAINER.to_owned()),
                    vec!["fetched source".to_owned()],
                ),
            ]),
        }
    }

    fn mapped() -> LocoCluster {
        loco_cluster(&inputs(), now(), &TimeZone::UTC)
    }

    #[test]
    fn applications_group_by_workspace_with_raw_names() {
        let loco = mapped();
        let applications = &loco.applications;
        assert_eq!(applications.total, 2);
        let groups: Vec<(&str, Vec<&str>)> = applications
            .workspaces
            .iter()
            .map(|group| {
                let names = group.applications.iter().map(|a| a.name.as_str()).collect();
                (group.workspace.label(), names)
            })
            .collect();
        assert_eq!(
            groups,
            [
                ("ws-0192c3e7", vec!["resource-api"]),
                ("ws-0192c3f1", vec!["resource-web"]),
            ]
        );
    }

    #[test]
    fn a_failing_application_keeps_the_controller_message() {
        let loco = mapped();
        let Some((_, api)) = loco
            .applications
            .iter()
            .find(|(_, a)| a.name == "resource-api")
        else {
            panic!("resource-api missing");
        };
        assert_eq!(api.phase, AppPhase::Failed);
        assert_eq!(api.replicas(), "0/2");
        assert_eq!(api.placement.short(), "r13 → r14");
        assert_eq!(api.placement.issue, None);
        let Some(degraded) = &api.degraded else {
            panic!("degraded missing");
        };
        let issue = degraded.issue(api.phase.status(), &api.name);
        assert_eq!(issue.reason, "CreateContainerConfigError");
        assert_eq!(
            issue.object,
            format!("Application {} · since 10m", api.name)
        );
        assert_eq!(
            api.resources.as_ref().map(Resources::summary).as_deref(),
            Some("CPU 0.50 · memory 512Mi · 2–6 replicas · scales at 70% CPU")
        );
        assert_eq!(
            api.rollout.as_ref().map(Rollout::summary).as_deref(),
            Some("0192f4b2 · 6m ago")
        );
    }

    #[test]
    fn controller_objects_and_route_come_from_the_workspace() {
        let loco = mapped();
        let Some((_, web)) = loco
            .applications
            .iter()
            .find(|(_, a)| a.name == "resource-web")
        else {
            panic!("resource-web missing");
        };
        let objects: Vec<(Status, String)> = web
            .objects
            .iter()
            .map(|object| (object.status, format!("{} {}", object.kind, object.label())))
            .collect();
        assert_eq!(
            objects,
            [
                (
                    Status::Ok,
                    "Deployment ws-0192c3f1/resource-web · 2/2 ready".to_owned()
                ),
                (
                    Status::Ok,
                    "Service ws-0192c3f1/resource-web · :80".to_owned()
                ),
                (
                    Status::Ok,
                    "HTTPRoute ws-0192c3f1/resource-web-route · Accepted".to_owned()
                ),
            ]
        );
        assert_eq!(
            web.deployment().map(|d| d.to_string()).as_deref(),
            Some("ws-0192c3f1/resource-web")
        );
        assert_eq!(
            web.route.as_ref().map(AppRoute::summary).as_deref(),
            Some("web.onloco.app → Gateway eg · https-apps-0 → ws-0192c3f1/resource-web:80")
        );
    }

    fn meta(name: &str, labels: &[(&str, &str)], annotations: &[(&str, &str)]) -> ObjectMeta {
        let pairs = |items: &[(&str, &str)]| {
            items
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect()
        };
        ObjectMeta {
            name: Some(name.to_owned()),
            namespace: Some("ws-0192c3f1".to_owned()),
            labels: Some(pairs(labels)),
            annotations: Some(pairs(annotations)),
            ..ObjectMeta::default()
        }
    }

    #[test]
    fn secrets_service_accounts_and_shared_policies_join_the_owned_objects() {
        let managed = ("app.kubernetes.io/managed-by", "loco-controller");
        let owner = [
            ("loco.io/application-name", "resource-web"),
            ("loco.io/application-namespace", "loco-system"),
        ];
        let mut inputs = inputs();
        inputs.secrets = vec![
            meta(
                "resource-web-env",
                &[managed, ("app", "resource-web")],
                &owner,
            ),
            meta("unrelated", &[], &[]),
        ];
        inputs.service_accounts = vec![meta(
            "resource-web",
            &[managed, ("app", "resource-web")],
            &owner,
        )];
        inputs.network_policies = vec![
            meta(
                "isolation",
                &[managed, ("loco.io/workspace-id", "0192c3f1")],
                &[],
            ),
            meta(
                "ingress",
                &[managed, ("loco.io/workspace-id", "0192c3f1")],
                &[],
            ),
            meta(
                "other",
                &[managed, ("loco.io/workspace-id", "elsewhere")],
                &[],
            ),
        ];
        let loco = loco_cluster(&inputs, now(), &TimeZone::UTC);
        let Some((_, web)) = loco
            .applications
            .iter()
            .find(|(_, a)| a.name == "resource-web")
        else {
            panic!("resource-web missing");
        };
        let rows: Vec<String> = web
            .objects
            .iter()
            .skip(3)
            .map(|object| format!("{} {}", object.kind, object.label()))
            .collect();
        assert_eq!(
            rows,
            [
                "Secret ws-0192c3f1/resource-web-env",
                "ServiceAccount ws-0192c3f1/resource-web",
                "NetworkPolicy ws-0192c3f1 · workspace isolation",
            ]
        );
    }

    #[test]
    fn routes_rank_problems_first_and_link_applications() {
        let loco = mapped();
        let routes = &loco.routes;
        assert_eq!(routes.gateway, "loco-system/eg");
        let rows: Vec<(Status, &str, String, String)> = routes
            .routes
            .iter()
            .map(|r| {
                (
                    r.status,
                    r.name.as_str(),
                    r.backend.clone(),
                    r.ready_label(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                (
                    Status::Warn,
                    "resource-api-route",
                    "resource-api:80".to_owned(),
                    "0/2".to_owned()
                ),
                (
                    Status::Ok,
                    "resource-web-route",
                    "resource-web:80".to_owned(),
                    "2/2".to_owned()
                ),
            ]
        );
        assert_eq!(
            routes.routes.first().and_then(|r| r.application.clone()),
            Some(reference(
                &ClusterId::new("kind-fleet-b"),
                "Application",
                "loco-system",
                "resource-api"
            ))
        );
        let listeners: Vec<(&str, String, String)> = routes
            .listeners
            .iter()
            .map(|l| (l.name.as_str(), l.port_label(), l.routes_label()))
            .collect();
        assert_eq!(
            listeners,
            [
                ("https-apps-0", "443".to_owned(), "2".to_owned()),
                ("http-apps-0", "80".to_owned(), "—".to_owned()),
            ]
        );
        assert_eq!(
            routes.listeners.first().map(|l| l.tls.as_str()),
            Some("loco-core-tls")
        );
    }

    #[test]
    fn builds_list_newest_first_with_pod_state_and_log() {
        let loco = mapped();
        let builds = &loco.builds;
        assert_eq!(builds.subtitle(), "loco-builds · kept 24h");
        let rows: Vec<(&str, BuildPhase, Option<&str>, String, String)> = builds
            .builds
            .iter()
            .map(|b| {
                (
                    b.id.as_str(),
                    b.phase,
                    b.started.as_deref(),
                    b.took_label(),
                    b.image_label(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                (
                    "b1",
                    BuildPhase::Running,
                    Some("21:07"),
                    "3m00s".to_owned(),
                    "building…".to_owned()
                ),
                (
                    "b0",
                    BuildPhase::Succeeded,
                    Some("20:31"),
                    "2m11s".to_owned(),
                    "sha256:9e1c…44d0".to_owned()
                ),
            ]
        );
        let Some(running) = builds.builds.first() else {
            panic!("no builds");
        };
        assert_eq!(running.service, "resource-web");
        assert_eq!(running.workspace.label(), "ws-0192c3f1");
        assert_eq!(running.job_label(), "job/build-b1 · node fleet-b-worker");
        assert_eq!(
            (running.fetch, running.buildkit),
            (Status::Ok, Status::Progress)
        );
        let log: Vec<(LogTone, &str)> = running
            .log
            .iter()
            .map(|line| (line.tone, line.text.as_str()))
            .collect();
        assert_eq!(
            log,
            [
                (LogTone::Dim, "fetched source"),
                (LogTone::Plain, "#1 load build definition"),
                (LogTone::Error, "ERROR: failed to solve"),
            ]
        );
    }

    #[test]
    fn platform_reports_components_and_never_invents_agent_sync() {
        let loco = mapped();
        let platform = &loco.platform;
        assert_eq!(platform.subtitle(), "loco-system · images sha-8c1f2e7");
        assert_eq!(platform.agent.stream(), None);
        assert_eq!(platform.agent.last_heartbeat, None);
        assert_eq!(platform.agent.reconnects, None);
        assert_eq!(platform.agent.kubernetes_healthy, None);
        assert_eq!(platform.agent.ready, Some((1, 1)));
        assert_eq!(platform.agent.builds_enabled, Some(true));
        let components: Vec<(Status, &str, String, Option<&str>, &str)> = platform
            .components
            .iter()
            .take(3)
            .map(|c| {
                (
                    c.status,
                    c.name.as_str(),
                    c.ready_label(),
                    c.version.as_deref(),
                    c.detail.as_str(),
                )
            })
            .collect();
        assert_eq!(
            components,
            [
                (
                    Status::Ok,
                    "agent",
                    "1/1".to_owned(),
                    Some("sha-8c1f2e7"),
                    ""
                ),
                (
                    Status::Err,
                    "loco-controller",
                    "0/1".to_owned(),
                    Some("sha-8c1f2e7"),
                    ""
                ),
                (
                    Status::Offline,
                    "loco-build-controller",
                    "—".to_owned(),
                    None,
                    "not found"
                ),
            ]
        );
        let log: Vec<(&str, &str)> = platform
            .log
            .iter()
            .map(|line| (line.time.as_str(), line.message.as_str()))
            .collect();
        assert_eq!(
            log,
            [
                ("21:09:30", "apply failed  err=invalid"),
                (
                    "21:09:01",
                    "placement applied  resource=resource-web  revision=14"
                ),
            ]
        );
    }

    #[test]
    fn unwatched_workloads_only_show_the_polled_agent() {
        let mut inputs = inputs();
        let agent = inputs
            .deployments
            .iter()
            .find(|d| d.metadata.name.as_deref() == Some(AGENT_DEPLOYMENT))
            .and_then(|d| deployment_health(d).ok());
        inputs.deployments.clear();
        inputs.workloads_watched = false;
        inputs.agent = agent;
        let loco = loco_cluster(&inputs, now(), &TimeZone::UTC);
        let names: Vec<&str> = loco
            .platform
            .components
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, ["agent"]);
        assert_eq!(loco.platform.agent.ready, Some((1, 1)));
    }

    #[test]
    fn nav_counts_flag_failures_and_route_problems() {
        let counts = nav_counts(&mapped());
        let find = |page| counts.iter().find(|(p, _)| *p == page).map(|(_, c)| *c);
        assert_eq!(
            find(Page::LocoApplications),
            Some(Counts::total(2).with_err(1))
        );
        assert_eq!(find(Page::LocoRoutes), Some(Counts::total(2).with_warn(1)));
        assert_eq!(find(Page::LocoBuilds), Some(Counts::default()));
    }

    #[test]
    fn wildcard_listeners_match_one_label_deep() {
        assert!(host_matches("*.onloco.app", "web.onloco.app"));
        assert!(!host_matches("*.onloco.app", "onloco.app"));
        assert!(host_matches("api.loco.build", "api.loco.build"));
        assert_eq!(
            image_version("ghcr.io/loco/agent:sha-1"),
            Some("sha-1".to_owned())
        );
        assert_eq!(image_version("localhost:5000/agent"), None);
    }
}
