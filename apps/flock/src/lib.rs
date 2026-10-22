use clap::ValueEnum;
use flock_core::crd::CrdKey;
use flock_core::helm::HelmKey;
use flock_core::history::Location;
use flock_core::tabs::{SavedPane, SavedWindow};
use flock_core::{
    AddClusterStep, ClusterId, Command, FluxKey, FluxKind, LogTarget, Page, ResourceKind,
    ResourceRef,
};
use flock_ui::actions::{CheckForUpdates, NewWindow, OpenSettings, Quit};
use flock_ui::theme::Appearance;
use flock_ui::{Gallery, GalleryPage, Shell, Store};
use gpui::{AnyView, App, AppContext as _, Entity, Menu, MenuItem, Window};

pub const WINDOW_WIDTH: f32 = 1440.;
pub const WINDOW_HEIGHT: f32 = 900.;
const SAMPLE_CLUSTER: &str = "prod-us-east";
const SAMPLE_QUERY: &str = "checkout";
const SAMPLE_RELEASE_CLUSTER: &str = "staging";
const SAMPLE_NAMESPACE: &str = "payments";
const SAMPLE_DEPLOYMENT: &str = "checkout-api";
const DEPLOYMENT: &str = "Deployment";
const POD: &str = "Pod";
const NAMESPACE: &str = "Namespace";
const SAMPLE_CONFIG: &str = "checkout-api-config";
const JOB: &str = "Job";
const NODE: &str = "Node";
const SAMPLE_RUNNING_JOB_NAMESPACE: &str = "ml";
const SAMPLE_RUNNING_JOB: &str = "ml-batch-5c7f";
const SAMPLE_NODE: &str = "ip-10-4-12-87";
const SAMPLE_OTHER_CLUSTER: &str = "prod-eu-west";
const SAMPLE_POD: &str = "checkout-api-7f9c6d-x2lqp";
const SAMPLE_CONTAINER: &str = "api";
const SAMPLE_KUSTOMIZATION: &str = "apps-payments";
const SAMPLE_JOB_NAMESPACE: &str = "data-pipeline";
const SAMPLE_JOB: &str = "backfill-orders-28770412";
const SAMPLE_CRD_GROUP: &str = "cert-manager.io";
const SAMPLE_CRD_KIND: &str = "Certificate";
const SAMPLE_CRD_NAMESPACE: &str = "web";
const SAMPLE_CRD_OBJECT: &str = "web-frontend-tls";
const SAMPLE_HELM_NAMESPACE: &str = "monitoring";
const SAMPLE_HELM_RELEASE: &str = "kube-prometheus-stack";
const FLUX_NAMESPACE: &str = "flux-system";

pub const APP_ID: &str = "dev.flock.app";
pub const APP_NAME: &str = "Flock";
pub const CHECK_FOR_UPDATES_ITEM: &str = "Check for Updates…";
const SETTINGS_ITEM: &str = "Settings…";
const NEW_WINDOW_ITEM: &str = "New Window";
const QUIT_ITEM: &str = "Quit Flock";
const BUNDLE_EXTENSION: &str = "app";

pub fn menus() -> Vec<Menu> {
    vec![Menu::new(APP_NAME).items([
        MenuItem::action(CHECK_FOR_UPDATES_ITEM, CheckForUpdates),
        MenuItem::separator(),
        MenuItem::action(SETTINGS_ITEM, OpenSettings),
        MenuItem::separator(),
        MenuItem::action(NEW_WINDOW_ITEM, NewWindow),
        MenuItem::separator(),
        MenuItem::action(QUIT_ITEM, Quit),
    ])]
}

pub fn bundled(executable: &std::path::Path) -> bool {
    executable
        .ancestors()
        .any(|dir| dir.extension().is_some_and(|ext| ext == BUNDLE_EXTENSION))
}

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys(flock_ui::keymap::key_bindings());
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Object {
    pub namespace: String,
    pub name: String,
}

pub fn parse_object(text: &str) -> Result<Object, String> {
    match text.split_once('/') {
        Some((namespace, name)) if !namespace.is_empty() && !name.is_empty() => Ok(Object {
            namespace: namespace.to_owned(),
            name: name.to_owned(),
        }),
        _ => Err(format!("expected NAMESPACE/NAME, got {text}")),
    }
}

#[derive(Clone, Debug, Default)]
pub struct Preset {
    pub cluster: Option<ClusterId>,
    pub object: Option<Object>,
}

impl Preset {
    fn cluster_or(&self, sample: &str) -> ClusterId {
        self.cluster
            .clone()
            .unwrap_or_else(|| ClusterId::new(sample))
    }

    fn object_or(&self, namespace: &str, name: &str) -> Object {
        self.object.clone().unwrap_or_else(|| Object {
            namespace: namespace.to_owned(),
            name: name.to_owned(),
        })
    }

    fn flux(&self, sample_cluster: &str, kind: FluxKind, namespace: &str, name: &str) -> FluxKey {
        let object = self.object_or(namespace, name);
        FluxKey::new(
            self.cluster_or(sample_cluster).as_str(),
            kind,
            object.namespace,
            object.name,
        )
    }

    fn object_ref(&self, kind: &str, namespace: &str, name: &str) -> ResourceRef {
        let object = self.object_or(namespace, name);
        ResourceRef {
            cluster: self.cluster_or(SAMPLE_CLUSTER),
            kind: kind.into(),
            namespace: Some(object.namespace),
            name: object.name,
        }
    }

    fn cluster_object(&self, kind: &str, name: &str) -> ResourceRef {
        let name = self
            .object
            .as_ref()
            .map_or_else(|| name.to_owned(), |object| object.name.clone());
        ResourceRef {
            cluster: self.cluster_or(SAMPLE_CLUSTER),
            kind: kind.into(),
            namespace: None,
            name,
        }
    }

    fn deployment(&self) -> ResourceRef {
        let object = self.object_or(SAMPLE_NAMESPACE, SAMPLE_DEPLOYMENT);
        ResourceRef {
            cluster: self.cluster_or(SAMPLE_CLUSTER),
            kind: DEPLOYMENT.into(),
            namespace: Some(object.namespace),
            name: object.name,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Screen {
    Flock,
    FleetProblems,
    Cluster,
    Collapsed,
    Switcher,
    Command,
    Changes,
    Overview,
    Nodes,
    Namespaces,
    Namespace,
    Events,
    Problems,
    Resources,
    ResourceInspector,
    Jobs,
    JobInspector,
    Kustomizations,
    FluxGraph,
    FluxEvents,
    CustomResources,
    CustomResourceDefinitions,
    CustomResourceInspector,
    HelmReleases,
    HelmRelease,
    Job,
    Node,
    Gitops,
    Compare,
    EditYaml,
    ClusterSettings,
    Settings,
    Kit,
    KitOverlays,
    KitPanes,
    AddCluster,
    AddClusterCheck,
    Workload,
    Split,
    Pods,
    PodsShell,
    NodesShell,
    Logs,
    Kustomization,
    Helmrelease,
    Source,
    #[cfg(feature = "loco")]
    LocoPlatform,
    #[cfg(feature = "loco")]
    LocoApps,
    #[cfg(feature = "loco")]
    LocoBuilds,
    #[cfg(feature = "loco")]
    LocoRoutes,
}

impl Screen {
    pub fn commands(self, preset: &Preset) -> Vec<Command> {
        let cluster = preset.cluster_or(SAMPLE_CLUSTER);
        let sample = Command::SelectCluster(Some(cluster.clone()));
        match self {
            Self::Flock => vec![Command::SelectPage(Page::Overview)],
            Self::FleetProblems => vec![Command::SelectPage(Page::Problems)],
            Self::Cluster => vec![sample],
            Self::Collapsed => vec![sample, Command::ToggleSidebar],
            Self::Switcher => vec![sample, Command::OpenResource(preset.deployment())],
            Self::Command => vec![Command::SelectPage(Page::Overview)],
            Self::Changes => vec![Command::SelectPage(Page::Changes)],
            Self::Overview => vec![sample, Command::SelectPage(Page::ClusterOverview)],
            Self::Nodes => vec![sample, Command::SelectPage(Page::Nodes)],
            Self::Namespaces => vec![sample, Command::SelectPage(Page::Namespaces)],
            Self::Events => vec![sample, Command::SelectPage(Page::Events)],
            Self::Problems => vec![sample, Command::SelectPage(Page::ClusterProblems)],
            Self::Resources => vec![sample, Command::SelectPage(Page::Deployments)],
            Self::Namespace => {
                let target = preset.cluster_object(NAMESPACE, SAMPLE_NAMESPACE);
                vec![Command::OpenResource(target)]
            }
            Self::ResourceInspector => {
                let target = preset.object_ref("ConfigMap", SAMPLE_NAMESPACE, SAMPLE_CONFIG);
                let page = Command::SelectPage(Page::ConfigMaps);
                vec![sample, page, Command::InspectResource(target)]
            }
            Self::Job => {
                let target =
                    preset.object_ref(JOB, SAMPLE_RUNNING_JOB_NAMESPACE, SAMPLE_RUNNING_JOB);
                vec![Command::OpenResource(target)]
            }
            Self::Jobs => vec![sample, Command::SelectPage(Page::Jobs)],
            Self::JobInspector => {
                let job = ResourceKind::Job.name();
                let target = preset.object_ref(job, SAMPLE_JOB_NAMESPACE, SAMPLE_JOB);
                let page = Command::SelectPage(Page::Jobs);
                vec![sample, page, Command::InspectResource(target)]
            }
            Self::Kustomizations => vec![sample, Command::SelectPage(Page::Kustomizations)],
            Self::FluxGraph => vec![sample, Command::SelectPage(Page::FluxGraph)],
            Self::FluxEvents => vec![Command::SelectPage(Page::FluxEvents)],
            Self::CustomResources => vec![sample, open_certificates(&cluster)],
            Self::CustomResourceDefinitions => {
                vec![sample, Command::SelectPage(Page::CustomResources)]
            }
            Self::HelmReleases => vec![sample, Command::SelectPage(Page::Releases)],
            Self::HelmRelease => vec![sample, open_helm_release(&cluster)],
            Self::CustomResourceInspector => {
                let open = open_certificates(&cluster);
                vec![sample, open, inspect_certificate(&cluster)]
            }
            Self::Node => {
                let target = preset.cluster_object(NODE, SAMPLE_NODE);
                vec![sample, Command::OpenResource(target)]
            }
            Self::Gitops => vec![Command::SelectPage(Page::GitOps)],
            Self::Compare => vec![Command::Compare(preset.deployment())],
            Self::EditYaml => {
                let target = preset.object_ref("ConfigMap", SAMPLE_NAMESPACE, SAMPLE_CONFIG);
                vec![Command::EditYaml(target)]
            }
            Self::ClusterSettings => vec![Command::ShowClusterSettings(cluster)],
            Self::Settings => vec![Command::SelectPage(Page::Settings)],
            Self::Kit => Vec::new(),
            Self::KitOverlays => Vec::new(),
            Self::KitPanes => Vec::new(),
            Self::AddCluster => vec![Command::OpenAddCluster(AddClusterStep::Pick)],
            Self::AddClusterCheck => vec![Command::OpenAddCluster(AddClusterStep::Check)],
            Self::Workload => vec![sample, Command::OpenResource(preset.deployment())],
            Self::Split => vec![sample, Command::OpenResource(preset.deployment())],
            Self::Pods => vec![sample, Command::SelectPage(Page::Pods)],
            Self::PodsShell => vec![sample, Command::SelectPage(Page::Pods)],
            Self::NodesShell => inspect_node(preset, sample),
            Self::Logs => vec![sample, follow_logs(preset)],
            Self::Kustomization => flux_screen(preset.flux(
                SAMPLE_CLUSTER,
                FluxKind::Kustomization,
                "flux-system",
                "apps-payments",
            )),
            Self::Helmrelease => flux_screen(preset.flux(
                SAMPLE_RELEASE_CLUSTER,
                FluxKind::HelmRelease,
                "monitoring",
                "kube-prometheus-stack",
            )),
            Self::Source => flux_screen(preset.flux(
                SAMPLE_CLUSTER,
                FluxKind::GitRepository,
                "flux-system",
                "fleet-infra",
            )),
            #[cfg(feature = "loco")]
            Self::LocoPlatform => vec![sample, Command::SelectPage(Page::LocoPlatform)],
            #[cfg(feature = "loco")]
            Self::LocoApps => vec![sample, Command::SelectPage(Page::LocoApplications)],
            #[cfg(feature = "loco")]
            Self::LocoBuilds => vec![sample, Command::SelectPage(Page::LocoBuilds)],
            #[cfg(feature = "loco")]
            Self::LocoRoutes => vec![sample, Command::SelectPage(Page::LocoRoutes)],
        }
    }
}

fn follow_logs(preset: &Preset) -> Command {
    Command::FollowLogs(LogTarget::Workload(preset.deployment()))
}

fn open_certificates(cluster: &ClusterId) -> Command {
    Command::OpenCrd {
        cluster: cluster.clone(),
        crd: CrdKey::new(SAMPLE_CRD_GROUP, SAMPLE_CRD_KIND),
    }
}

fn open_helm_release(cluster: &ClusterId) -> Command {
    let key = HelmKey::new(cluster.clone(), SAMPLE_HELM_NAMESPACE, SAMPLE_HELM_RELEASE);
    Command::OpenHelmRelease(key)
}

fn inspect_certificate(cluster: &ClusterId) -> Command {
    let certificates = CrdKey::new(SAMPLE_CRD_GROUP, SAMPLE_CRD_KIND);
    let namespace = Some(SAMPLE_CRD_NAMESPACE.into());
    let target = certificates.object(cluster.clone(), namespace, SAMPLE_CRD_OBJECT.into());
    Command::InspectResource(target)
}

impl Screen {
    pub fn tabs(self, preset: &Preset) -> Option<SavedWindow> {
        let cluster = preset.cluster_or(SAMPLE_CLUSTER);
        let deployment = preset.deployment();
        let workload = Location {
            resource: Some(deployment.clone()),
            ..Location::page_on(cluster.clone(), Page::Deployments)
        };
        let other = ClusterId::new(SAMPLE_OTHER_CLUSTER);
        let elsewhere = Location {
            resource: Some(ResourceRef {
                cluster: other.clone(),
                ..deployment
            }),
            ..Location::page_on(other.clone(), Page::Deployments)
        };
        let other_key = FluxKey::new(
            other.as_str(),
            FluxKind::Kustomization,
            FLUX_NAMESPACE,
            SAMPLE_KUSTOMIZATION,
        );
        let other_kustomization = Location {
            flux_object: Some(other_key),
            ..Location::page_on(other, Page::Kustomizations)
        };
        let key = FluxKey::new(
            cluster.as_str(),
            FluxKind::Kustomization,
            FLUX_NAMESPACE,
            SAMPLE_KUSTOMIZATION,
        );
        let kustomization = Location {
            flux_object: Some(key),
            ..Location::page_on(cluster.clone(), Page::Kustomizations)
        };
        let logs = Location {
            resource: Some(ResourceRef {
                cluster: cluster.clone(),
                kind: POD.into(),
                namespace: Some(SAMPLE_NAMESPACE.into()),
                name: SAMPLE_POD.into(),
            }),
            container: Some(SAMPLE_CONTAINER.into()),
            ..Location::page_on(cluster, Page::Logs)
        };
        let (tabs, active) = match self {
            Self::Flock => (
                vec![
                    Location::flock(Page::Overview),
                    Location::flock(Page::GitOps),
                    workload,
                ],
                0,
            ),
            Self::Workload => (vec![workload, kustomization, elsewhere], 0),
            Self::Switcher => (vec![workload, kustomization, elsewhere], 0),
            Self::Kustomization => (vec![workload, kustomization, logs], 1),
            Self::Split => {
                let left = SavedPane {
                    tabs: vec![workload, logs, kustomization],
                    active: 0,
                };
                let right = SavedPane {
                    tabs: vec![elsewhere, other_kustomization],
                    active: 0,
                };
                return Some(SavedWindow {
                    panes: vec![left, right],
                    ..SavedWindow::default()
                });
            }
            _ => return None,
        };
        Some(SavedWindow {
            panes: vec![SavedPane { tabs, active }],
            focused: 0,
            ..SavedWindow::default()
        })
    }
}

fn flux_screen(key: FluxKey) -> Vec<Command> {
    vec![Command::OpenFluxObject(key)]
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum AppearanceArg {
    Dark,
    Light,
}

impl From<AppearanceArg> for Appearance {
    fn from(arg: AppearanceArg) -> Self {
        match arg {
            AppearanceArg::Dark => Self::Dark,
            AppearanceArg::Light => Self::Light,
        }
    }
}

pub fn root_view(
    screen: Screen,
    preset: &Preset,
    store: Entity<Store>,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    match screen {
        Screen::Kit => cx
            .new(|cx| Gallery::new(GalleryPage::Primitives, window, cx))
            .into(),
        Screen::KitOverlays => cx
            .new(|cx| Gallery::new(GalleryPage::Overlays, window, cx))
            .into(),
        Screen::KitPanes => cx
            .new(|cx| Gallery::new(GalleryPage::Panes, window, cx))
            .into(),
        Screen::Flock => shell_view(store, screen, preset, window, cx),
        Screen::FleetProblems => shell_view(store, screen, preset, window, cx),
        Screen::Cluster => shell_view(store, screen, preset, window, cx),
        Screen::Collapsed => shell_view(store, screen, preset, window, cx),
        Screen::Switcher => shell_view(store, screen, preset, window, cx),
        Screen::Command => shell_view(store, screen, preset, window, cx),
        Screen::Changes => shell_view(store, screen, preset, window, cx),
        Screen::Overview => shell_view(store, screen, preset, window, cx),
        Screen::Nodes => shell_view(store, screen, preset, window, cx),
        Screen::Namespaces => shell_view(store, screen, preset, window, cx),
        Screen::Events => shell_view(store, screen, preset, window, cx),
        Screen::Problems => shell_view(store, screen, preset, window, cx),
        Screen::Resources => shell_view(store, screen, preset, window, cx),
        Screen::ResourceInspector => shell_view(store, screen, preset, window, cx),
        Screen::Jobs => shell_view(store, screen, preset, window, cx),
        Screen::JobInspector => shell_view(store, screen, preset, window, cx),
        Screen::Kustomizations => shell_view(store, screen, preset, window, cx),
        Screen::FluxGraph => shell_view(store, screen, preset, window, cx),
        Screen::FluxEvents => shell_view(store, screen, preset, window, cx),
        Screen::CustomResources => shell_view(store, screen, preset, window, cx),
        Screen::CustomResourceDefinitions => shell_view(store, screen, preset, window, cx),
        Screen::CustomResourceInspector => shell_view(store, screen, preset, window, cx),
        Screen::HelmReleases => shell_view(store, screen, preset, window, cx),
        Screen::HelmRelease => shell_view(store, screen, preset, window, cx),
        Screen::Namespace => shell_view(store, screen, preset, window, cx),
        Screen::Job => shell_view(store, screen, preset, window, cx),
        Screen::Node => shell_view(store, screen, preset, window, cx),
        Screen::AddCluster => shell_view(store, screen, preset, window, cx),
        Screen::AddClusterCheck => shell_view(store, screen, preset, window, cx),
        Screen::Gitops => shell_view(store, screen, preset, window, cx),
        Screen::Workload => shell_view(store, screen, preset, window, cx),
        Screen::Split => shell_view(store, screen, preset, window, cx),
        Screen::Pods => shell_view(store, screen, preset, window, cx),
        Screen::PodsShell => shell_view(store, screen, preset, window, cx),
        Screen::NodesShell => shell_view(store, screen, preset, window, cx),
        Screen::Logs => shell_view(store, screen, preset, window, cx),
        Screen::Kustomization => shell_view(store, screen, preset, window, cx),
        Screen::Helmrelease => shell_view(store, screen, preset, window, cx),
        Screen::Source => shell_view(store, screen, preset, window, cx),
        #[cfg(feature = "loco")]
        Screen::LocoPlatform => shell_view(store, screen, preset, window, cx),
        #[cfg(feature = "loco")]
        Screen::LocoApps => shell_view(store, screen, preset, window, cx),
        #[cfg(feature = "loco")]
        Screen::LocoBuilds => shell_view(store, screen, preset, window, cx),
        Screen::Compare => shell_view(store, screen, preset, window, cx),
        Screen::EditYaml => shell_view(store, screen, preset, window, cx),
        Screen::ClusterSettings => shell_view(store, screen, preset, window, cx),
        Screen::Settings => shell_view(store, screen, preset, window, cx),
        #[cfg(feature = "loco")]
        Screen::LocoRoutes => shell_view(store, screen, preset, window, cx),
    }
}

fn inspect_node(preset: &Preset, sample: Command) -> Vec<Command> {
    let target = preset.cluster_object(NODE, SAMPLE_NODE);
    let page = Command::SelectPage(Page::Nodes);
    vec![sample, page, Command::InspectResource(target)]
}

fn shell_view(
    store: Entity<Store>,
    screen: Screen,
    preset: &Preset,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    let initial = screen.commands(preset);
    let shell = cx.new(|cx| Shell::new(store, initial, window, cx));
    if let Screen::Command = screen {
        shell.update(cx, |shell, cx| shell.open_palette(SAMPLE_QUERY, window, cx));
    }
    if let Screen::Switcher = screen {
        shell.update(cx, |shell, cx| shell.open_switcher(window, cx));
    }
    if let Screen::NodesShell = screen {
        let node = preset.cluster_object(NODE, SAMPLE_NODE);
        shell.update(cx, |shell, cx| shell.open_node_shell(node, window, cx));
    }
    if let Screen::PodsShell = screen {
        shell.update(cx, |shell, cx| shell.show_pod_terminal(window, cx));
    }
    shell.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_executables_inside_an_app_bundle_count_as_bundled() {
        use std::path::Path;
        assert!(bundled(Path::new(
            "/Applications/Flock.app/Contents/MacOS/flock"
        )));
        assert!(!bundled(Path::new("/Users/me/flock/target/debug/flock")));
    }

    #[test]
    fn objects_parse_as_namespace_and_name() {
        assert_eq!(
            parse_object("payments/checkout-api"),
            Ok(Object {
                namespace: "payments".into(),
                name: "checkout-api".into(),
            })
        );
        assert!(parse_object("checkout-api").is_err());
        assert!(parse_object("/x").is_err());
    }

    #[test]
    fn presets_name_the_live_cluster_and_object() {
        let preset = Preset {
            cluster: Some(ClusterId::new("kind-fleet-a")),
            object: parse_object("apps/podinfo").ok(),
        };
        let key = FluxKey::new("kind-fleet-a", FluxKind::HelmRelease, "apps", "podinfo");
        assert_eq!(
            Screen::Helmrelease.commands(&preset),
            [Command::OpenFluxObject(key)]
        );
        let workload = Screen::Workload.commands(&preset);
        assert_eq!(
            workload.last(),
            Some(&Command::OpenResource(ResourceRef {
                cluster: ClusterId::new("kind-fleet-a"),
                kind: DEPLOYMENT.into(),
                namespace: Some("apps".into()),
                name: "podinfo".into(),
            }))
        );
    }

    #[test]
    fn presets_default_to_the_sample_data() {
        let preset = Preset::default();
        let release = Screen::Helmrelease.commands(&preset);
        let Some(Command::OpenFluxObject(key)) = release.first() else {
            panic!("expected a Flux object");
        };
        assert_eq!(key.cluster, ClusterId::new(SAMPLE_RELEASE_CLUSTER));
        assert_eq!(
            Screen::Nodes.commands(&preset).first(),
            Some(&Command::SelectCluster(Some(ClusterId::new(
                SAMPLE_CLUSTER
            ))))
        );
    }
}
