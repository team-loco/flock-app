use std::collections::BTreeSet;
use std::sync::Arc;

use flock_core::overview::NetworkFacts;
use k8s_openapi::api::core::v1::{PersistentVolumeClaim, Service};
use k8s_openapi::api::discovery::v1::{Endpoint, EndpointSlice};

const SERVICE_NAME_LABEL: &str = "kubernetes.io/service-name";
const EXTERNAL_NAME: &str = "ExternalName";
const LOAD_BALANCER: &str = "LoadBalancer";
const PENDING: &str = "Pending";

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

fn without_endpoints(services: &[Arc<Service>], slices: &[Arc<EndpointSlice>]) -> usize {
    let served: BTreeSet<(&str, &str)> = slices
        .iter()
        .filter(|slice| slice.endpoints.iter().flatten().any(is_ready))
        .filter_map(|slice| {
            let namespace = slice.metadata.namespace.as_deref()?;
            let service = slice.metadata.labels.as_ref()?.get(SERVICE_NAME_LABEL)?;
            Some((namespace, service.as_str()))
        })
        .collect();
    services
        .iter()
        .filter(|service| selects_pods(service))
        .filter(|service| {
            let namespace = service.metadata.namespace.as_deref().unwrap_or_default();
            let name = service.metadata.name.as_deref().unwrap_or_default();
            !served.contains(&(namespace, name))
        })
        .count()
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
