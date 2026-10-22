use std::collections::BTreeMap;

use futures::future::join_all;
use k8s_openapi::api::authentication::v1::SelfSubjectReview;
use k8s_openapi::api::authorization::v1::{
    ResourceAttributes, SelfSubjectAccessReview, SelfSubjectAccessReviewSpec,
};
use k8s_openapi::api::core::v1::{Namespace, Node, Pod, Service, ServicePort};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::APIGroup;
use k8s_openapi::kube_aggregator::pkg::apis::apiregistration::v1::APIService;
use kube::api::{DynamicObject, ListParams, PostParams};
use kube::core::{ApiResource, GroupVersionKind};
use kube::{Api, Client};

use crate::error::Result;

pub const FLUX_KUSTOMIZE_GROUP: &str = "kustomize.toolkit.fluxcd.io";
pub const FLUX_HELM_GROUP: &str = "helm.toolkit.fluxcd.io";
pub const FLUX_SOURCE_GROUP: &str = "source.toolkit.fluxcd.io";
pub const FLUX_NAMESPACE: &str = "flux-system";
pub const LOCO_GROUP: &str = "infra.loco.io";
pub const METRICS_APISERVICE: &str = "v1beta1.metrics.k8s.io";
const VERSION_LABEL: &str = "app.kubernetes.io/version";
const AVAILABLE_CONDITION: &str = "Available";
const CONDITION_TRUE: &str = "True";
const COUNT_PAGE: u32 = 1;
const PROMETHEUS_NAMESPACES: [&str; 6] = [
    "monitoring",
    "prometheus",
    "observability",
    "kube-prometheus-stack",
    "kube-system",
    "default",
];
const PROMETHEUS_NAME: &str = "prometheus";
const PROMETHEUS_EXCLUDED: [&str; 8] = [
    "operator",
    "alertmanager",
    "node-exporter",
    "pushgateway",
    "adapter",
    "kube-state",
    "blackbox",
    "config-reloader",
];
const PROMETHEUS_PORT: i32 = 9090;
const PROMETHEUS_PORT_NAMES: [&str; 3] = ["web", "http-web", "http"];
const HTTPS_SCHEME: &str = "https";
const HTTP_SCHEME: &str = "http";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrometheusTarget {
    pub namespace: String,
    pub service: String,
    pub port: u16,
    pub scheme: String,
}

impl PrometheusTarget {
    pub fn label(&self) -> String {
        format!("{}/{}", self.namespace, self.service)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApiGroups(BTreeMap<String, String>);

impl ApiGroups {
    pub fn preferred_version(&self, group: &str) -> Option<&str> {
        self.0.get(group).map(String::as_str)
    }

    pub fn has(&self, group: &str) -> bool {
        self.0.contains_key(group)
    }
}

fn preferred(group: &APIGroup) -> Option<String> {
    group
        .preferred_version
        .as_ref()
        .map(|version| version.version.clone())
        .or_else(|| {
            group
                .versions
                .first()
                .map(|version| version.version.clone())
        })
}

pub async fn api_groups(client: &Client) -> Result<ApiGroups> {
    let list = client.list_api_groups().await?;
    let groups = list
        .groups
        .iter()
        .filter_map(|group| preferred(group).map(|version| (group.name.clone(), version)))
        .collect();
    Ok(ApiGroups(groups))
}

pub async fn whoami(client: &Client) -> Result<Option<String>> {
    let api: Api<SelfSubjectReview> = Api::all(client.clone());
    let review = api
        .create(&PostParams::default(), &SelfSubjectReview::default())
        .await?;
    Ok(review
        .status
        .and_then(|status| status.user_info)
        .and_then(|info| info.username))
}

fn count_of(items: usize, remaining: Option<i64>) -> u64 {
    let listed = u64::try_from(items).unwrap_or(u64::MAX);
    let rest = remaining
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or(0);
    listed.saturating_add(rest)
}

pub async fn count<K>(api: &Api<K>) -> Result<u64>
where
    K: kube::Resource + Clone + serde::de::DeserializeOwned + std::fmt::Debug,
{
    let params = ListParams::default().limit(COUNT_PAGE);
    let list = api.list_metadata(&params).await?;
    Ok(count_of(
        list.items.len(),
        list.metadata.remaining_item_count,
    ))
}

pub async fn count_pods(client: &Client) -> Result<u64> {
    count(&Api::<Pod>::all(client.clone())).await
}

pub struct NodeSample {
    pub count: u64,
    pub provider_id: Option<String>,
}

pub async fn sample_nodes(client: &Client) -> Result<NodeSample> {
    let api: Api<Node> = Api::all(client.clone());
    let list = api.list(&ListParams::default().limit(COUNT_PAGE)).await?;
    let provider_id = list
        .items
        .first()
        .and_then(|node| node.spec.as_ref())
        .and_then(|spec| spec.provider_id.clone());
    Ok(NodeSample {
        count: count_of(list.items.len(), list.metadata.remaining_item_count),
        provider_id,
    })
}

pub fn dynamic_api(
    client: &Client,
    group: &str,
    version: &str,
    kind: &str,
    plural: &str,
) -> Api<DynamicObject> {
    let gvk = GroupVersionKind::gvk(group, version, kind);
    let resource = ApiResource::from_gvk_with_plural(&gvk, plural);
    Api::all_with(client.clone(), &resource)
}

pub async fn flux_version(client: &Client) -> Result<Option<String>> {
    let api: Api<Namespace> = Api::all(client.clone());
    let namespace = api.get_opt(FLUX_NAMESPACE).await?;
    Ok(namespace
        .and_then(|ns| ns.metadata.labels)
        .and_then(|labels| labels.get(VERSION_LABEL).cloned()))
}

pub async fn metrics_server_available(client: &Client) -> Result<bool> {
    let api: Api<APIService> = Api::all(client.clone());
    let service = api.get_opt(METRICS_APISERVICE).await?;
    let available = service
        .and_then(|service| service.status)
        .and_then(|status| status.conditions)
        .unwrap_or_default()
        .iter()
        .any(|condition| {
            condition.type_ == AVAILABLE_CONDITION && condition.status == CONDITION_TRUE
        });
    Ok(available)
}

fn is_prometheus_name(name: &str) -> bool {
    name.contains(PROMETHEUS_NAME)
        && !PROMETHEUS_EXCLUDED
            .iter()
            .any(|excluded| name.contains(excluded))
}

fn prometheus_port(ports: &[ServicePort]) -> Option<&ServicePort> {
    ports
        .iter()
        .find(|port| port.port == PROMETHEUS_PORT)
        .or_else(|| {
            ports.iter().find(|port| {
                port.name
                    .as_deref()
                    .is_some_and(|name| PROMETHEUS_PORT_NAMES.contains(&name))
            })
        })
}

pub fn prometheus_target(service: &Service) -> Option<PrometheusTarget> {
    let name = service.metadata.name.as_deref()?;
    if !is_prometheus_name(name) {
        return None;
    }
    let namespace = service.metadata.namespace.clone()?;
    let ports = service.spec.as_ref()?.ports.as_deref()?;
    let port = prometheus_port(ports)?;
    let number = u16::try_from(port.port).ok()?;
    let secure = port
        .name
        .as_deref()
        .is_some_and(|name| name.contains(HTTPS_SCHEME));
    let scheme = if secure { HTTPS_SCHEME } else { HTTP_SCHEME };
    Some(PrometheusTarget {
        namespace,
        service: name.to_owned(),
        port: number,
        scheme: scheme.to_owned(),
    })
}

async fn prometheus_in(client: &Client, namespace: &str) -> Option<PrometheusTarget> {
    let api: Api<Service> = Api::namespaced(client.clone(), namespace);
    let services = api.list(&ListParams::default()).await.ok()?;
    let mut found: Vec<PrometheusTarget> = services
        .items
        .iter()
        .filter_map(prometheus_target)
        .collect();
    found.sort_by_key(|target| target.service.len());
    found.into_iter().next()
}

pub async fn find_prometheus(client: &Client) -> Option<PrometheusTarget> {
    let lookups = PROMETHEUS_NAMESPACES.map(|namespace| prometheus_in(client, namespace));
    join_all(lookups).await.into_iter().flatten().next()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessRequest {
    pub group: &'static str,
    pub resource: &'static str,
    pub verb: &'static str,
}

pub async fn can(client: &Client, request: &AccessRequest) -> Result<bool> {
    let api: Api<SelfSubjectAccessReview> = Api::all(client.clone());
    let attributes = ResourceAttributes {
        group: Some(request.group.to_owned()),
        resource: Some(request.resource.to_owned()),
        verb: Some(request.verb.to_owned()),
        ..ResourceAttributes::default()
    };
    let review = SelfSubjectAccessReview {
        spec: SelfSubjectAccessReviewSpec {
            resource_attributes: Some(attributes),
            ..SelfSubjectAccessReviewSpec::default()
        },
        ..SelfSubjectAccessReview::default()
    };
    let answer = api.create(&PostParams::default(), &review).await?;
    Ok(answer.status.is_some_and(|status| status.allowed))
}

#[cfg(test)]
mod tests {
    use k8s_openapi::api::core::v1::ServiceSpec;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    use super::*;

    fn service(name: &str, ports: &[(&str, i32)]) -> Service {
        Service {
            metadata: ObjectMeta {
                name: Some(name.to_owned()),
                namespace: Some("monitoring".to_owned()),
                ..ObjectMeta::default()
            },
            spec: Some(ServiceSpec {
                ports: Some(
                    ports
                        .iter()
                        .map(|(name, port)| ServicePort {
                            name: Some((*name).to_owned()),
                            port: *port,
                            ..ServicePort::default()
                        })
                        .collect(),
                ),
                ..ServiceSpec::default()
            }),
            status: None,
        }
    }

    #[test]
    fn picks_prometheus_services() {
        let target = prometheus_target(&service(
            "prometheus-k8s",
            &[("web", 9090), ("reloader", 8080)],
        ));
        assert_eq!(
            target,
            Some(PrometheusTarget {
                namespace: "monitoring".to_owned(),
                service: "prometheus-k8s".to_owned(),
                port: 9090,
                scheme: "http".to_owned(),
            })
        );
        assert_eq!(
            target.map(|t| t.label()),
            Some("monitoring/prometheus-k8s".to_owned())
        );
        assert_eq!(
            prometheus_target(&service("kps-prometheus", &[("http-web", 80)])).map(|t| t.port),
            Some(80)
        );
        assert_eq!(
            prometheus_target(&service("prometheus-operator", &[("http", 8080)])),
            None
        );
        assert_eq!(
            prometheus_target(&service("prometheus-node-exporter", &[("m", 9100)])),
            None
        );
        assert_eq!(
            prometheus_target(&service("grafana", &[("http", 9090)])),
            None
        );
    }

    #[test]
    fn counts_include_remaining_items() {
        assert_eq!(count_of(1, Some(211)), 212);
        assert_eq!(count_of(0, None), 0);
        assert_eq!(count_of(1, Some(-3)), 1);
    }
}
