use std::collections::BTreeMap;

use crate::{Page, ResourceRef, Status};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceSummary {
    pub resource: ResourceRef,
    pub status: Option<Status>,
    pub detail: String,
    pub labels: BTreeMap<String, String>,
    pub ports: Vec<u16>,
    pub owner: Option<ResourceRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KindInfo {
    pub kind: &'static str,
    pub short: &'static str,
    pub page: Option<Page>,
    pub has_pods: bool,
}

const fn info(kind: &'static str, short: &'static str, page: Option<Page>) -> KindInfo {
    KindInfo {
        kind,
        short,
        page,
        has_pods: false,
    }
}

const fn workload(kind: &'static str, short: &'static str, page: Page) -> KindInfo {
    KindInfo {
        kind,
        short,
        page: Some(page),
        has_pods: true,
    }
}

pub const KINDS: &[KindInfo] = &[
    workload("Deployment", "deploy", Page::Deployments),
    workload("StatefulSet", "sts", Page::StatefulSets),
    workload("DaemonSet", "ds", Page::DaemonSets),
    info("Pod", "pod", Some(Page::Pods)),
    info("Job", "job", Some(Page::Jobs)),
    info("CronJob", "cj", Some(Page::CronJobs)),
    info("HorizontalPodAutoscaler", "hpa", None),
    info("Service", "svc", Some(Page::Services)),
    info("Ingress", "ing", Some(Page::Ingresses)),
    info("ConfigMap", "cm", Some(Page::ConfigMaps)),
    info("Secret", "secret", Some(Page::Secrets)),
    info("PersistentVolumeClaim", "pvc", Some(Page::Pvcs)),
    info("Kustomization", "ks", Some(Page::Kustomizations)),
    info("HelmRelease", "hr", Some(Page::HelmReleases)),
    info("GitRepository", "gitrepo", Some(Page::Sources)),
    info("HelmRepository", "helmrepo", Some(Page::Sources)),
    info("OCIRepository", "ocirepo", Some(Page::Sources)),
    info("Node", "node", Some(Page::Nodes)),
    info("Namespace", "ns", Some(Page::Namespaces)),
];

pub fn kind_info(kind: &str) -> Option<&'static KindInfo> {
    KINDS.iter().find(|info| info.kind == kind)
}

pub fn kind_rank(kind: &str) -> usize {
    KINDS
        .iter()
        .position(|info| info.kind == kind)
        .unwrap_or(KINDS.len())
}

pub fn short_kind(kind: &str) -> String {
    let kind = crate::crd::display_kind(kind);
    kind_info(kind).map_or_else(|| kind.to_lowercase(), |info| info.short.to_owned())
}

pub fn page_for_kind(kind: &str) -> Page {
    if crate::crd::CrdKey::parse(kind).is_some() {
        return Page::CustomResources;
    }
    kind_info(kind)
        .and_then(|info| info.page)
        .unwrap_or(Page::ClusterOverview)
}

pub fn kind_matches(kind: &str, wanted: &str) -> bool {
    let wanted = wanted.to_lowercase();
    let full = kind.to_lowercase();
    full.starts_with(&wanted) || short_kind(kind) == wanted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_kinds_follow_kubectl() {
        assert_eq!(short_kind("Deployment"), "deploy");
        assert_eq!(short_kind("Service"), "svc");
        assert_eq!(short_kind("Widget"), "widget");
    }

    #[test]
    fn kinds_route_to_their_list_page() {
        assert_eq!(page_for_kind("Pod"), Page::Pods);
        assert_eq!(page_for_kind("OCIRepository"), Page::Sources);
        assert_eq!(
            page_for_kind("HorizontalPodAutoscaler"),
            Page::ClusterOverview
        );
        assert_eq!(page_for_kind("Widget"), Page::ClusterOverview);
        assert_eq!(
            page_for_kind("Certificate.cert-manager.io"),
            Page::CustomResources
        );
        assert_eq!(short_kind("Certificate.cert-manager.io"), "certificate");
    }

    #[test]
    fn kind_filter_accepts_short_full_and_prefix() {
        assert!(kind_matches("Deployment", "deploy"));
        assert!(kind_matches("Deployment", "Deployment"));
        assert!(kind_matches("Service", "svc"));
        assert!(kind_matches("Service", "serv"));
        assert!(!kind_matches("Secret", "svc"));
    }

    #[test]
    fn owners_rank_before_what_they_own() {
        assert!(kind_rank("Deployment") < kind_rank("Pod"));
        assert!(kind_rank("Service") < kind_rank("Secret"));
        assert_eq!(kind_rank("Widget"), KINDS.len());
    }
}
