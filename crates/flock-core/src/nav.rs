use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Page {
    Overview,
    Problems,
    GitOps,
    Changes,
    FluxEvents,
    FlockDeployments,
    FlockPods,
    FlockNodes,
    FlockReleases,
    ClusterSettings,
    Settings,
    ClusterOverview,
    Nodes,
    Namespaces,
    Events,
    ClusterProblems,
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
            Self::FlockDeployments => "Deployments",
            Self::FlockPods => "Pods",
            Self::FlockNodes => "Nodes",
            Self::FlockReleases => "Helm releases",
            Self::ClusterSettings => "Clusters",
            Self::Settings => "Settings",
            Self::ClusterOverview => "Overview",
            Self::Nodes => "Nodes",
            Self::Namespaces => "Namespaces",
            Self::Events => "Events",
            Self::ClusterProblems => "Problems",
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
            Self::FlockDeployments => Self::Deployments.singular(),
            Self::FlockPods => Self::Pods.singular(),
            Self::FlockNodes => Self::Nodes.singular(),
            Self::FlockReleases => Self::Releases.singular(),
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
                | Self::FlockDeployments
                | Self::FlockPods
                | Self::FlockNodes
                | Self::FlockReleases
                | Self::ClusterSettings
                | Self::Settings
        )
    }

    pub const fn across(self) -> Option<Self> {
        match self {
            Self::FlockDeployments => Some(Self::Deployments),
            Self::FlockPods => Some(Self::Pods),
            Self::FlockNodes => Some(Self::Nodes),
            Self::FlockReleases => Some(Self::Releases),
            _ => None,
        }
    }

    pub fn section(self) -> Option<&'static str> {
        CLUSTER_NAV
            .iter()
            .find(|section| section.pages.contains(&self))
            .and_then(|section| section.label)
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

pub const FLOCK_NAV: &[NavSection] = &[
    NavSection {
        label: None,
        pages: &[
            Page::Overview,
            Page::Problems,
            Page::GitOps,
            Page::Changes,
            Page::FluxEvents,
        ],
    },
    NavSection {
        label: Some("Resources"),
        pages: &[
            Page::FlockDeployments,
            Page::FlockPods,
            Page::FlockNodes,
            Page::FlockReleases,
        ],
    },
];

pub const CLUSTER_NAV: &[NavSection] = &[
    NavSection {
        label: None,
        pages: &[
            Page::ClusterOverview,
            Page::Nodes,
            Page::Namespaces,
            Page::Events,
            Page::ClusterProblems,
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
    NavSection {
        label: Some("Extensions"),
        pages: &[Page::CustomResources],
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
    fn flock_resource_pages_list_a_cluster_page_across_clusters() {
        let resources = FLOCK_NAV.iter().find(|s| s.label == Some("Resources"));
        let pages = resources.map(|s| s.pages).unwrap_or_default();
        let across: Vec<Option<Page>> = pages.iter().map(|p| p.across()).collect();
        assert_eq!(
            across,
            [
                Some(Page::Deployments),
                Some(Page::Pods),
                Some(Page::Nodes),
                Some(Page::Releases)
            ]
        );
        assert_eq!(Page::FlockReleases.label(), "Helm releases");
        assert_eq!(Page::FlockPods.singular(), "Pod");
        assert_eq!(Page::Deployments.across(), None);
    }

    #[test]
    fn cluster_overview_title_differs_from_its_nav_label() {
        assert_eq!(Page::ClusterOverview.label(), "Overview");
        assert_eq!(Page::ClusterOverview.title(), "Cluster overview");
        assert_eq!(Page::Nodes.title(), "Nodes");
    }

    #[test]
    fn pages_name_their_sidebar_section() {
        assert_eq!(Page::Sources.section(), Some("Flux"));
        assert_eq!(Page::CustomResources.section(), Some("Extensions"));
        assert_eq!(Page::Nodes.section(), None);
        assert_eq!(Page::ClusterProblems.section(), None);
        assert_eq!(Page::Overview.section(), None);
    }

    #[test]
    fn cluster_problems_follow_events() {
        let basics = CLUSTER_NAV[0].pages;
        let events = basics.iter().position(|page| *page == Page::Events);
        assert_eq!(
            basics.get(events.map_or(0, |ix| ix + 1)),
            Some(&Page::ClusterProblems)
        );
        assert_eq!(Page::ClusterProblems.label(), Page::Problems.label());
        assert!(!Page::ClusterProblems.is_flock());
    }

    #[cfg(feature = "loco")]
    #[test]
    fn loco_section_follows_cluster_basics() {
        assert_eq!(CLUSTER_NAV[1].label, Some("Loco"));
    }
}
