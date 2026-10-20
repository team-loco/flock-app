use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Page {
    Overview,
    Problems,
    GitOps,
    Changes,
    FluxEvents,
    ClusterSettings,
    Settings,
    ClusterOverview,
    Nodes,
    Namespaces,
    Events,
    #[cfg(feature = "loco")]
    LocoPlatform,
    #[cfg(feature = "loco")]
    LocoApplications,
    #[cfg(feature = "loco")]
    LocoBuilds,
    #[cfg(feature = "loco")]
    LocoRoutes,
    Pods,
    Deployments,
    StatefulSets,
    DaemonSets,
    Jobs,
    CronJobs,
    Kustomizations,
    HelmReleases,
    Sources,
    FluxGraph,
    Releases,
    Services,
    Ingresses,
    ConfigMaps,
    Secrets,
    Pvcs,
    CustomResources,
    Compare,
    EditYaml,
    Logs,
}

impl Page {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Problems => "Problems",
            Self::GitOps => "GitOps",
            Self::Changes => "Changes",
            Self::FluxEvents => "Flux events",
            Self::ClusterSettings => "Clusters",
            Self::Settings => "Settings",
            Self::ClusterOverview => "Overview",
            Self::Nodes => "Nodes",
            Self::Namespaces => "Namespaces",
            Self::Events => "Events",
            #[cfg(feature = "loco")]
            Self::LocoPlatform => "Platform",
            #[cfg(feature = "loco")]
            Self::LocoApplications => "Applications",
            #[cfg(feature = "loco")]
            Self::LocoBuilds => "Builds",
            #[cfg(feature = "loco")]
            Self::LocoRoutes => "Routes",
            Self::Pods => "Pods",
            Self::Deployments => "Deployments",
            Self::StatefulSets => "StatefulSets",
            Self::DaemonSets => "DaemonSets",
            Self::Jobs => "Jobs",
            Self::CronJobs => "CronJobs",
            Self::Kustomizations => "Kustomizations",
            Self::HelmReleases => "HelmReleases",
            Self::Sources => "Sources",
            Self::FluxGraph => "Graph",
            Self::Releases => "Releases",
            Self::Services => "Services",
            Self::Ingresses => "Ingresses",
            Self::ConfigMaps => "ConfigMaps",
            Self::Secrets => "Secrets",
            Self::Pvcs => "PVCs",
            Self::CustomResources => "Custom resources",
            Self::Compare => "Compare",
            Self::EditYaml => "Edit YAML",
            Self::Logs => "Logs",
        }
    }

    pub const fn singular(self) -> &'static str {
        match self {
            Self::Nodes => "Node",
            Self::Namespaces => "Namespace",
            Self::Events => "Event",
            #[cfg(feature = "loco")]
            Self::LocoApplications => "Application",
            #[cfg(feature = "loco")]
            Self::LocoBuilds => "Build",
            #[cfg(feature = "loco")]
            Self::LocoRoutes => "Route",
            Self::Pods => "Pod",
            Self::Deployments => "Deployment",
            Self::StatefulSets => "StatefulSet",
            Self::DaemonSets => "DaemonSet",
            Self::Jobs => "Job",
            Self::CronJobs => "CronJob",
            Self::Kustomizations => "Kustomization",
            Self::HelmReleases => "HelmRelease",
            Self::Sources => "Source",
            Self::Releases => "Release",
            Self::Services => "Service",
            Self::Ingresses => "Ingress",
            Self::ConfigMaps => "ConfigMap",
            Self::Secrets => "Secret",
            Self::Pvcs => "PVC",
            Self::CustomResources => "Custom resource",
            other => other.label(),
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::ClusterOverview => "Cluster overview",
            Self::FluxGraph => "Flux graph",
            Self::Releases => "Helm releases",
            other => other.label(),
        }
    }

    pub const fn is_flock(self) -> bool {
        matches!(
            self,
            Self::Overview
                | Self::Problems
                | Self::GitOps
                | Self::Changes
                | Self::FluxEvents
                | Self::ClusterSettings
                | Self::Settings
        )
    }

    #[cfg(feature = "loco")]
    pub const fn is_loco(self) -> bool {
        matches!(
            self,
            Self::LocoPlatform | Self::LocoApplications | Self::LocoBuilds | Self::LocoRoutes
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavSection {
    pub label: Option<&'static str>,
    pub pages: &'static [Page],
}

impl NavSection {
    #[cfg(feature = "loco")]
    pub fn is_loco(&self) -> bool {
        self.pages.iter().copied().any(Page::is_loco)
    }
}

pub const FLOCK_NAV: &[NavSection] = &[NavSection {
    label: None,
    pages: &[
        Page::Overview,
        Page::Problems,
        Page::GitOps,
        Page::Changes,
        Page::FluxEvents,
    ],
}];

pub const CLUSTER_NAV: &[NavSection] = &[
    NavSection {
        label: None,
        pages: &[
            Page::ClusterOverview,
            Page::Nodes,
            Page::Namespaces,
            Page::Events,
        ],
    },
    #[cfg(feature = "loco")]
    NavSection {
        label: Some("Loco"),
        pages: &[
            Page::LocoPlatform,
            Page::LocoApplications,
            Page::LocoBuilds,
            Page::LocoRoutes,
        ],
    },
    NavSection {
        label: Some("Workloads"),
        pages: &[
            Page::Pods,
            Page::Deployments,
            Page::StatefulSets,
            Page::DaemonSets,
            Page::Jobs,
            Page::CronJobs,
        ],
    },
    NavSection {
        label: Some("Flux"),
        pages: &[
            Page::Kustomizations,
            Page::HelmReleases,
            Page::Sources,
            Page::FluxGraph,
        ],
    },
    NavSection {
        label: Some("Helm"),
        pages: &[Page::Releases],
    },
    NavSection {
        label: Some("Network"),
        pages: &[Page::Services, Page::Ingresses],
    },
    NavSection {
        label: Some("Config"),
        pages: &[Page::ConfigMaps, Page::Secrets],
    },
    NavSection {
        label: Some("Storage"),
        pages: &[Page::Pvcs],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flock_nav_holds_only_flock_pages() {
        let flock_pages = FLOCK_NAV.iter().flat_map(|s| s.pages.iter());
        assert!(flock_pages.copied().all(Page::is_flock));
        let cluster_pages = CLUSTER_NAV.iter().flat_map(|s| s.pages.iter());
        assert!(!cluster_pages.copied().any(Page::is_flock));
    }

    #[test]
    fn cluster_overview_title_differs_from_its_nav_label() {
        assert_eq!(Page::ClusterOverview.label(), "Overview");
        assert_eq!(Page::ClusterOverview.title(), "Cluster overview");
        assert_eq!(Page::Nodes.title(), "Nodes");
    }

    #[cfg(feature = "loco")]
    #[test]
    fn loco_section_follows_cluster_basics() {
        assert_eq!(CLUSTER_NAV[1].label, Some("Loco"));
    }
}
