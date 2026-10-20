use std::collections::BTreeMap;
use std::sync::Arc;

use flock_core::home::Problem;
use flock_core::overview::NetworkFacts;
use flock_core::{ClusterId, ResourceRef, Status};
use jiff::Timestamp;
use k8s_openapi::api::core::v1::{PersistentVolumeClaim, Service};
use k8s_openapi::api::discovery::v1::{Endpoint, EndpointSlice};

const SERVICE_NAME_LABEL: &str = "kubernetes.io/service-name";
const EXTERNAL_NAME: &str = "ExternalName";
const LOAD_BALANCER: &str = "LoadBalancer";
const PENDING: &str = "Pending";
const SERVICE_KIND: &str = "Service";
const NO_ENDPOINTS: &str = "no ready endpoints";

pub fn facts(
    services: &[Arc<Service>],
    slices: Option<&[Arc<EndpointSlice>]>,
    claims: &[Arc<PersistentVolumeClaim>],
) -> NetworkFacts {
    NetworkFacts {
        services_without_endpoints: slices.map(|slices| without_endpoints(services, slices)),
        load_balancers_pending: services.iter().filter(|s| waits_for_address(s)).count(),
        pvcs_pending: claims.iter().filter(|c| is_pending(c)).count(),
    }
}

#[derive(Clone, Copy, Default)]
struct Tally {
    ready: usize,
    total: usize,
}

fn tallies(slices: &[Arc<EndpointSlice>]) -> BTreeMap<(&str, &str), Tally> {
    let mut tallies: BTreeMap<(&str, &str), Tally> = BTreeMap::new();
    for slice in slices {
        let namespace = slice.metadata.namespace.as_deref();
        let service = slice
            .metadata
            .labels
            .as_ref()
            .and_then(|labels| labels.get(SERVICE_NAME_LABEL));
        let (Some(namespace), Some(service)) = (namespace, service) else {
            continue;
        };
        let tally = tallies.entry((namespace, service.as_str())).or_default();
        tally.total += slice.endpoints.iter().flatten().count();
        tally.ready += slice
            .endpoints
            .iter()
            .flatten()
            .filter(|e| is_ready(e))
            .count();
    }
    tallies
}

fn service_key(service: &Service) -> (&str, &str) {
    let namespace = service.metadata.namespace.as_deref().unwrap_or_default();
    let name = service.metadata.name.as_deref().unwrap_or_default();
    (namespace, name)
}

fn without_endpoints(services: &[Arc<Service>], slices: &[Arc<EndpointSlice>]) -> usize {
    let tallies = tallies(slices);
    services
        .iter()
        .filter(|service| selects_pods(service))
        .filter(|service| {
            tallies
                .get(&service_key(service))
                .is_none_or(|tally| tally.ready == 0)
        })
        .count()
}

pub fn unready_services(
    cluster: &ClusterId,
    services: &[Arc<Service>],
    slices: Option<&[Arc<EndpointSlice>]>,
    now: Timestamp,
) -> Vec<Problem> {
    let Some(slices) = slices else {
        return Vec::new();
    };
    let tallies = tallies(slices);
    services
        .iter()
        .filter(|service| selects_pods(service))
        .filter_map(|service| {
            let tally = tallies
                .get(&service_key(service))
                .copied()
                .unwrap_or_default();
            if tally.total > 0 && tally.ready == tally.total {
                return None;
            }
            let (namespace, name) = service_key(service);
            let message = if tally.total == 0 {
                NO_ENDPOINTS.to_owned()
            } else {
                format!(
                    "{} of {} endpoints not ready",
                    tally.total - tally.ready,
                    tally.total
                )
            };
            Some(Problem {
                status: Status::Warn,
                target: ResourceRef {
                    cluster: cluster.clone(),
                    kind: SERVICE_KIND.to_owned(),
                    namespace: Some(namespace.to_owned()),
                    name: name.to_owned(),
                },
                message,
                age: crate::clock::age(&service.metadata, now),
            })
        })
        .collect()
}

fn is_ready(endpoint: &Endpoint) -> bool {
    let ready = endpoint.conditions.as_ref().and_then(|c| c.ready);
    ready != Some(false) && !endpoint.addresses.is_empty()
}

fn selects_pods(service: &Service) -> bool {
    let Some(spec) = service.spec.as_ref() else {
        return false;
    };
    let external = spec.type_.as_deref() == Some(EXTERNAL_NAME);
    let selected = spec.selector.as_ref().is_some_and(|s| !s.is_empty());
    selected && !external
}

fn waits_for_address(service: &Service) -> bool {
    let balanced = service.spec.as_ref().and_then(|s| s.type_.as_deref()) == Some(LOAD_BALANCER);
    let assigned = service
        .status
        .as_ref()
        .and_then(|s| s.load_balancer.as_ref())
        .and_then(|lb| lb.ingress.as_ref())
        .is_some_and(|ingress| !ingress.is_empty());
    balanced && !assigned
}

fn is_pending(claim: &PersistentVolumeClaim) -> bool {
    claim.status.as_ref().and_then(|s| s.phase.as_deref()) == Some(PENDING)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::problems::tests::parse;

    fn service(name: &str, spec: &serde_json::Value, status: &serde_json::Value) -> Arc<Service> {
        parse(json!({
            "metadata": {"name": name, "namespace": "shop"},
            "spec": spec,
            "status": status,
        }))
    }

    fn slice(service: &str, endpoints: &serde_json::Value) -> Arc<EndpointSlice> {
        parse(json!({
            "metadata": {
                "name": format!("{service}-abc"),
                "namespace": "shop",
                "labels": {"kubernetes.io/service-name": service},
            },
            "addressType": "IPv4",
            "endpoints": endpoints,
        }))
    }

    fn claim(phase: &str) -> Arc<PersistentVolumeClaim> {
        parse(json!({"metadata": {"name": "data"}, "status": {"phase": phase}}))
    }

    #[test]
    fn services_without_ready_endpoints_are_counted() {
        let selector = json!({"selector": {"app": "x"}});
        let services = [
            service("served", &selector, &json!({})),
            service("not-ready", &selector, &json!({})),
            service("no-slice", &selector, &json!({})),
            service("unset-ready", &selector, &json!({})),
            service("manual", &json!({}), &json!({})),
            service(
                "external",
                &json!({"type": "ExternalName", "selector": {"app": "x"}}),
                &json!({}),
            ),
        ];
        let slices = [
            slice(
                "served",
                &json!([{"addresses": ["10.0.0.1"], "conditions": {"ready": true}}]),
            ),
            slice(
                "not-ready",
                &json!([{"addresses": ["10.0.0.2"], "conditions": {"ready": false}}]),
            ),
            slice("unset-ready", &json!([{"addresses": ["10.0.0.3"]}])),
        ];
        let facts = facts(&services, Some(&slices), &[]);
        assert_eq!(facts.services_without_endpoints, Some(2));
    }

    #[test]
    fn partly_ready_and_endpointless_services_are_listed_with_their_counts() {
        let selector = json!({"selector": {"app": "x"}});
        let services = [
            service("whole", &selector, &json!({})),
            service("partial", &selector, &json!({})),
            service("empty", &selector, &json!({})),
            service("manual", &json!({}), &json!({})),
        ];
        let ready = json!({"addresses": ["10.0.0.1"], "conditions": {"ready": true}});
        let down = json!({"addresses": ["10.0.0.2"], "conditions": {"ready": false}});
        let slices = [
            slice("whole", &json!([ready])),
            slice("partial", &json!([ready, down, down])),
        ];
        let cluster = ClusterId::new("a");
        let now = Timestamp::UNIX_EPOCH;
        let problems = unready_services(&cluster, &services, Some(&slices), now);
        let found: Vec<(&str, &str)> = problems
            .iter()
            .map(|p| (p.target.name.as_str(), p.message.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                ("partial", "2 of 3 endpoints not ready"),
                ("empty", NO_ENDPOINTS)
            ]
        );
        let unknown = unready_services(&cluster, &services, None, now);
        assert_eq!(unknown.len(), 0);
    }

    #[test]
    fn unreadable_endpoint_slices_leave_the_count_unknown() {
        let services = [service("a", &json!({"selector": {"app": "a"}}), &json!({}))];
        assert_eq!(facts(&services, None, &[]).services_without_endpoints, None);
    }

    #[test]
    fn load_balancers_without_an_address_and_pending_claims_are_counted() {
        let services = [
            service("waiting", &json!({"type": "LoadBalancer"}), &json!({})),
            service(
                "empty",
                &json!({"type": "LoadBalancer"}),
                &json!({"loadBalancer": {"ingress": []}}),
            ),
            service(
                "assigned",
                &json!({"type": "LoadBalancer"}),
                &json!({"loadBalancer": {"ingress": [{"ip": "203.0.113.7"}]}}),
            ),
            service("internal", &json!({"type": "ClusterIP"}), &json!({})),
        ];
        let claims = [claim("Pending"), claim("Bound"), claim("Lost")];
        let facts = facts(&services, Some(&[]), &claims);
        assert_eq!(facts.load_balancers_pending, 2);
        assert_eq!(facts.pvcs_pending, 1);
    }
}
