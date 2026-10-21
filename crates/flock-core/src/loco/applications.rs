use std::collections::BTreeMap;
use std::time::Duration;

use super::Workspace;
use crate::filter::{FilterSpec, TextFilter};
use crate::issue::{Issue, object_line};
use crate::time::age;
use crate::{ClusterId, ResourceRef, Seen, Status, Uid, units};

const MISSING: &str = "—";
const SEPARATOR: &str = " · ";
const APPLICATION: &str = "Application";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppPhase {
    Idle,
    Deploying,
    Ready,
    Failed,
    Unknown,
}

impl AppPhase {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Deploying => "Deploying",
            Self::Ready => "Ready",
            Self::Failed => "Failed",
            Self::Unknown => "Unknown",
        }
    }

    pub const fn status(self) -> Status {
        match self {
            Self::Idle => Status::Suspended,
            Self::Deploying => Status::Progress,
            Self::Ready => Status::Ok,
            Self::Failed => Status::Err,
            Self::Unknown => Status::Offline,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementIssue {
    pub status: Status,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Placement {
    pub desired: Option<i64>,
    pub applied: Option<i64>,
    pub issue: Option<PlacementIssue>,
}

impl Placement {
    pub fn out_of_sync(&self) -> bool {
        self.desired.is_some() && self.desired != self.applied
    }

    pub fn short(&self) -> String {
        match (self.applied, self.desired) {
            (Some(applied), Some(desired)) if applied != desired => {
                format!("r{applied} → r{desired}")
            }
            (None, Some(desired)) => format!("{MISSING} → r{desired}"),
            (_, Some(desired)) => format!("r{desired}"),
            (Some(applied), None) => format!("r{applied}"),
            (None, None) => MISSING.to_owned(),
        }
    }

    pub fn detail(&self) -> String {
        match (self.applied, self.desired) {
            (Some(applied), Some(desired)) if applied != desired => {
                format!("rev {desired} desired · rev {applied} applied by agent")
            }
            (None, Some(desired)) => format!("rev {desired} desired · not applied yet"),
            (_, Some(desired)) => format!("rev {desired} desired · rev {desired} applied by agent"),
            (Some(applied), None) => format!("rev {applied} applied by agent"),
            (None, None) => MISSING.to_owned(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Degraded {
    pub reason: String,
    pub message: String,
    pub since: Option<Duration>,
}

impl Degraded {
    pub fn issue(&self, status: Status, application: &str) -> Issue {
        let object = object_line([
            Some(format!("{APPLICATION} {application}")),
            self.since.map(|since| format!("since {}", age(since))),
        ]);
        Issue::new(status, self.reason.clone(), self.message.clone(), object)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rollout {
    pub deployment_id: String,
    pub age: Option<Duration>,
    pub build: Option<String>,
}

impl Rollout {
    pub fn summary(&self) -> String {
        let mut parts = vec![self.deployment_id.clone()];
        parts.extend(self.age.map(|elapsed| format!("{} ago", age(elapsed))));
        parts.extend(self.build.as_ref().map(|build| format!("build {build}")));
        parts.join(SEPARATOR)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resources {
    pub cpu: String,
    pub memory: String,
    pub min_replicas: i32,
    pub max_replicas: i32,
    pub cpu_target: Option<i32>,
}

impl Resources {
    pub fn summary(&self) -> String {
        let replicas = if self.min_replicas == self.max_replicas {
            format!(
                "{} {}",
                self.min_replicas,
                if self.min_replicas == 1 {
                    "replica"
                } else {
                    "replicas"
                }
            )
        } else {
            format!("{}–{} replicas", self.min_replicas, self.max_replicas)
        };
        let mut parts = vec![
            format!("CPU {}", units::written_cpu(&self.cpu)),
            format!("memory {}", self.memory),
            replicas,
        ];
        parts.extend(
            self.cpu_target
                .map(|target| format!("scales at {target}% CPU")),
        );
        parts.join(SEPARATOR)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthCheck {
    pub path: String,
    pub interval_secs: i32,
    pub timeout_secs: i32,
    pub fail_threshold: i32,
}

impl HealthCheck {
    pub fn summary(&self) -> String {
        format!(
            "GET {} · every {}s · timeout {}s · fail after {}",
            self.path, self.interval_secs, self.timeout_secs, self.fail_threshold
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControllerObject {
    pub status: Status,
    pub kind: String,
    pub namespace: String,
    pub name: Option<String>,
    pub detail: String,
}

impl ControllerObject {
    pub fn reference(&self, cluster: &ClusterId) -> Option<ResourceRef> {
        let name = self.name.clone()?;
        Some(ResourceRef {
            cluster: cluster.clone(),
            kind: self.kind.clone(),
            namespace: Some(self.namespace.clone()),
            name,
        })
    }

    pub fn label(&self) -> String {
        let object = match &self.name {
            Some(name) => format!("{}/{name}", self.namespace),
            None => self.namespace.clone(),
        };
        if self.detail.is_empty() {
            object
        } else {
            format!("{object}{SEPARATOR}{}", self.detail)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppRoute {
    pub host: String,
    pub gateway: String,
    pub listener: String,
    pub service: String,
    pub port: u16,
}

impl AppRoute {
    pub fn summary(&self) -> String {
        format!(
            "{} → Gateway {} · {} → {}:{}",
            self.host, self.gateway, self.listener, self.service, self.port
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Application {
    pub labels: BTreeMap<String, String>,
    pub resource: ResourceRef,
    pub workload_uid: Uid,
    pub name: String,
    pub environment: String,
    pub region: Option<String>,
    pub phase: AppPhase,
    pub ready_replicas: Option<i32>,
    pub desired_replicas: Option<i32>,
    pub placement: Placement,
    pub hostname: Option<String>,
    pub degraded: Option<Degraded>,
    pub image: Option<String>,
    pub rollout: Option<Rollout>,
    pub resources: Option<Resources>,
    pub health_check: Option<HealthCheck>,
    pub objects: Vec<ControllerObject>,
    pub route: Option<AppRoute>,
}

impl Application {
    pub fn seen(&self) -> Seen {
        Seen::new(self.resource.clone(), self.workload_uid.clone())
    }

    pub fn replicas(&self) -> String {
        match (self.ready_replicas, self.desired_replicas) {
            (Some(ready), Some(desired)) => format!("{ready}/{desired}"),
            _ => MISSING.to_owned(),
        }
    }

    pub fn needs_attention(&self) -> bool {
        self.phase != AppPhase::Ready || self.placement.out_of_sync()
    }

    pub fn deployment(&self) -> Option<ResourceRef> {
        self.objects
            .iter()
            .find(|object| object.kind == "Deployment")
            .and_then(|object| object.reference(&self.resource.cluster))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceApps {
    pub workspace: Workspace,
    pub applications: Vec<Application>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AppFilter {
    #[default]
    All,
    NotReady,
}

impl AppFilter {
    pub const ALL: [Self; 2] = [Self::All, Self::NotReady];

    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::NotReady => "Not ready",
        }
    }

    pub fn keeps(self, application: &Application) -> bool {
        match self {
            Self::All => true,
            Self::NotReady => application.needs_attention(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applications {
    pub total: usize,
    pub workspaces: Vec<WorkspaceApps>,
}

impl Application {
    pub const FILTER: FilterSpec<Self, Workspace> = FilterSpec {
        fields: &[
            ("name", |a, _, out| out.push(a.name.as_str().into())),
            ("environment", |a, _, out| {
                out.push(a.environment.as_str().into());
            }),
            ("workspace", |_, w, out| out.push(w.label().into())),
            ("namespace", |_, w, out| {
                out.push(w.namespace.as_str().into());
            }),
            ("image", |a, _, out| {
                out.push(a.image.as_deref().unwrap_or_default().into());
            }),
            ("hostname", |a, _, out| {
                out.push(a.hostname.as_deref().unwrap_or_default().into());
            }),
        ],
        namespace: true,
        labels: true,
    };

    pub fn matches(&self, workspace: &Workspace, query: &TextFilter) -> bool {
        Self::FILTER.matches_in(
            self,
            workspace,
            query,
            Some(&workspace.namespace),
            &[&self.labels],
        )
    }
}

impl Applications {
    pub fn filter_counts(&self, query: &TextFilter) -> Vec<(AppFilter, usize)> {
        AppFilter::ALL
            .iter()
            .map(|filter| {
                let apps = self
                    .filtered(*filter, query)
                    .iter()
                    .map(|group| group.applications.len())
                    .sum();
                (*filter, apps)
            })
            .collect()
    }

    pub fn filtered(&self, filter: AppFilter, query: &TextFilter) -> Vec<WorkspaceApps> {
        self.workspaces
            .iter()
            .filter_map(|group| {
                let applications: Vec<Application> = group
                    .applications
                    .iter()
                    .filter(|application| {
                        filter.keeps(application) && application.matches(&group.workspace, query)
                    })
                    .cloned()
                    .collect();
                (!applications.is_empty()).then(|| WorkspaceApps {
                    workspace: group.workspace.clone(),
                    applications,
                })
            })
            .collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Workspace, &Application)> {
        self.workspaces.iter().flat_map(|group| {
            group
                .applications
                .iter()
                .map(move |application| (&group.workspace, application))
        })
    }

    pub fn find(&self, resource: &ResourceRef) -> Option<(&Workspace, &Application)> {
        self.iter()
            .find(|(_, application)| &application.resource == resource)
    }

    pub fn find_mut(&mut self, resource: &ResourceRef) -> Option<&mut Application> {
        self.workspaces
            .iter_mut()
            .flat_map(|group| group.applications.iter_mut())
            .find(|application| &application.resource == resource)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn application(name: &str, phase: AppPhase, placement: Placement) -> Application {
        Application {
            labels: BTreeMap::new(),
            resource: ResourceRef {
                cluster: ClusterId::new("prod"),
                kind: "Application".into(),
                namespace: Some("loco-system".into()),
                name: name.into(),
            },
            name: name.into(),
            environment: "production".into(),
            region: None,
            phase,
            ready_replicas: Some(1),
            desired_replicas: Some(2),
            placement,
            hostname: None,
            degraded: None,
            image: None,
            rollout: None,
            resources: None,
            health_check: None,
            workload_uid: Uid::default(),
            objects: Vec::new(),
            route: None,
        }
    }

    fn rev(applied: i64, desired: i64) -> Placement {
        Placement {
            desired: Some(desired),
            applied: Some(applied),
            issue: None,
        }
    }

    #[test]
    fn list_label_tokens_match_metadata() {
        let mut application = application("web", AppPhase::Ready, rev(1, 1));
        application.labels.insert("team".into(), "checkout".into());
        let workspace = Workspace {
            namespace: "ws-1".into(),
            name: None,
        };
        assert!(application.matches(&workspace, &TextFilter::parse("label:team=checkout")));
        assert!(!application.matches(&workspace, &TextFilter::parse("l:team=platform")));
    }

    #[test]
    fn placement_reads_applied_then_desired() {
        assert_eq!(rev(14, 14).short(), "r14");
        assert_eq!(rev(21, 22).short(), "r21 → r22");
        assert_eq!(rev(9, 9).detail(), "rev 9 desired · rev 9 applied by agent");
        assert_eq!(
            rev(21, 22).detail(),
            "rev 22 desired · rev 21 applied by agent"
        );
        let pending = Placement {
            desired: Some(3),
            applied: None,
            issue: None,
        };
        assert_eq!(pending.short(), "— → r3");
        assert!(pending.out_of_sync());
        assert!(!Placement::default().out_of_sync());
        assert_eq!(Placement::default().short(), "—");
    }

    #[test]
    fn summaries_match_the_design_wording() {
        let resources = Resources {
            cpu: "500m".into(),
            memory: "512Mi".into(),
            min_replicas: 2,
            max_replicas: 6,
            cpu_target: Some(70),
        };
        assert_eq!(
            resources.summary(),
            "CPU 0.50 · memory 512Mi · 2–6 replicas · scales at 70% CPU"
        );
        let check = HealthCheck {
            path: "/healthz".into(),
            interval_secs: 10,
            timeout_secs: 3,
            fail_threshold: 3,
        };
        assert_eq!(
            check.summary(),
            "GET /healthz · every 10s · timeout 3s · fail after 3"
        );
        let rollout = Rollout {
            deployment_id: "0192f4b2".into(),
            age: Some(Duration::from_secs(360)),
            build: Some("0192f3a1".into()),
        };
        assert_eq!(rollout.summary(), "0192f4b2 · 6m ago · build 0192f3a1");
        let degraded = Degraded {
            reason: "CreateContainerConfigError".into(),
            message: String::new(),
            since: Some(Duration::from_secs(360)),
        };
        let issue = degraded.issue(Status::Err, "checkout");
        assert_eq!(issue.reason, "CreateContainerConfigError");
        assert_eq!(issue.object, "Application checkout · since 6m");
    }

    #[test]
    fn controller_objects_label_shared_ones_by_namespace() {
        let policy = ControllerObject {
            status: Status::Ok,
            kind: "NetworkPolicy".into(),
            namespace: "ws-1".into(),
            name: None,
            detail: "workspace isolation".into(),
        };
        assert_eq!(policy.label(), "ws-1 · workspace isolation");
        assert_eq!(policy.reference(&ClusterId::new("c")), None);
        let deployment = ControllerObject {
            name: Some("checkout".into()),
            kind: "Deployment".into(),
            detail: String::new(),
            ..policy
        };
        assert_eq!(deployment.label(), "ws-1/checkout");
        let mut app = application("checkout", AppPhase::Failed, Placement::default());
        app.objects.push(deployment);
        let reference = app.deployment().map(|r| r.to_string());
        assert_eq!(reference.as_deref(), Some("ws-1/checkout"));
    }

    #[test]
    fn not_ready_keeps_unready_or_out_of_sync_apps_and_drops_empty_groups() {
        let workspace = |namespace: &str| Workspace {
            namespace: namespace.into(),
            name: None,
        };
        let applications = Applications {
            total: 3,
            workspaces: vec![
                WorkspaceApps {
                    workspace: workspace("ws-a"),
                    applications: vec![
                        application("ok", AppPhase::Ready, rev(1, 1)),
                        application("drift", AppPhase::Ready, rev(1, 2)),
                    ],
                },
                WorkspaceApps {
                    workspace: workspace("ws-b"),
                    applications: vec![application("fine", AppPhase::Ready, rev(4, 4))],
                },
            ],
        };
        let everything = TextFilter::default();
        assert_eq!(applications.filtered(AppFilter::All, &everything).len(), 2);
        assert_eq!(
            applications
                .filtered(AppFilter::All, &TextFilter::parse("drift"))
                .len(),
            1
        );
        let counts: Vec<usize> = applications
            .filter_counts(&everything)
            .into_iter()
            .map(|(_, n)| n)
            .collect();
        assert_eq!(counts.get(1), Some(&1));
        let not_ready = applications.filtered(AppFilter::NotReady, &everything);
        let names: Vec<_> = not_ready
            .iter()
            .flat_map(|g| g.applications.iter().map(|a| a.name.as_str()))
            .collect();
        assert_eq!(names, ["drift"]);
        let drift = &not_ready[0].applications[0].resource;
        assert_eq!(
            applications.find(drift).map(|(w, _)| w.label()),
            Some("ws-a")
        );
    }
}
