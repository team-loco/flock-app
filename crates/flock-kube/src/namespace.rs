use std::collections::BTreeMap;
use std::sync::Arc;

use flock_core::namespace::{Listing, NamespaceDetail, Policy, Quota, QuotaItem, tally};
use flock_core::related::NAMESPACE;
use flock_core::{ResourceRef, WatchState};
use jiff::Timestamp;
use k8s_openapi::api::core::v1::{Pod, ResourceQuota};
use k8s_openapi::api::networking::v1::{
    NetworkPolicy, NetworkPolicyPeer, NetworkPolicyPort, NetworkPolicySpec,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;

use crate::manager::ClusterHandle;
use crate::metrics::{cpu_millicores, parse_quantity};
use crate::related::pod;
use crate::resources::WatchKind;
use crate::usage::{cores, gib};

pub const WATCHES: [WatchKind; 2] = [WatchKind::ResourceQuotas, WatchKind::NetworkPolicies];
const CPU_QUOTAS: [&str; 2] = ["requests.cpu", "cpu"];
const MEMORY_QUOTAS: [&str; 2] = ["requests.memory", "memory"];
const NO_USE: &str = "0";
const INGRESS: &str = "Ingress";
const EGRESS: &str = "Egress";
const NAME_LABELS: [&str; 3] = ["app.kubernetes.io/name", "app", "k8s-app"];
const ALL_PODS: &str = "";
const SAME_NAMESPACE: &str = "same namespace";
const ALL_NAMESPACES: &str = "all namespaces";
const ANYWHERE: &str = "anywhere";
const NAMESPACE_PREFIX: &str = "ns ";
const DENY_ALL: &str = "deny all ingress + egress";
const DENY_INGRESS: &str = "deny all ingress";
const DENY_EGRESS: &str = "deny all egress";
const DENY_INGRESS_TO: &str = "deny ingress to";
const DENY_EGRESS_FROM: &str = "deny egress from";
const INGRESS_FROM: &str = "ingress from";
const EGRESS_TO: &str = "egress to";
const ARROW: &str = " → ";
const RULE_SEPARATOR: &str = "; ";
const LIST_SEPARATOR: &str = ", ";
const PORT_SEPARATOR: &str = ",";
const PEER_SEPARATOR: char = '/';
const PORT_PREFIX: char = ':';
const RANGE: char = '-';
const LABEL_JOIN: &str = ",";

pub fn details(
    handle: &ClusterHandle,
    targets: &[&ResourceRef],
    now: Timestamp,
) -> Vec<NamespaceDetail> {
    let mut built: Vec<NamespaceDetail> = Vec::new();
    for target in targets.iter().filter(|t| t.kind == NAMESPACE) {
        if built.iter().any(|detail| detail.name == target.name) {
            continue;
        }
        let objects = Objects {
            pods: &handle.objects::<Pod>(),
            quotas: &handle.objects::<ResourceQuota>(),
            quota_state: handle.watch_state(&WatchKind::ResourceQuotas),
            policies: &handle.objects::<NetworkPolicy>(),
            policy_state: handle.watch_state(&WatchKind::NetworkPolicies),
        };
        built.push(detail(&target.name, &objects, now));
    }
    built
}

pub struct Objects<'a> {
    pub pods: &'a [Arc<Pod>],
    pub quotas: &'a [Arc<ResourceQuota>],
    pub quota_state: WatchState,
    pub policies: &'a [Arc<NetworkPolicy>],
    pub policy_state: WatchState,
}

fn in_namespace(namespace: Option<&str>, name: &str) -> bool {
    namespace == Some(name)
}

pub fn detail(name: &str, objects: &Objects<'_>, now: Timestamp) -> NamespaceDetail {
    let pods: Vec<&Arc<Pod>> = objects
        .pods
        .iter()
        .filter(|p| in_namespace(p.metadata.namespace.as_deref(), name))
        .collect();
    let facts: Vec<_> = pods.iter().map(|p| pod(p, now)).collect();
    let pod_tally = tally(facts.iter().map(|f| (f.status, f.note.as_str())));
    let quotas: Vec<&Arc<ResourceQuota>> = objects
        .quotas
        .iter()
        .filter(|q| in_namespace(q.metadata.namespace.as_deref(), name))
        .collect();
    let cpu_quota = smallest(&quotas, &CPU_QUOTAS, cpu_millicores).map(cores);
    let memory_quota = smallest(&quotas, &MEMORY_QUOTAS, parse_quantity).map(gib);
    let mut policies: Vec<Policy> = objects
        .policies
        .iter()
        .filter(|p| in_namespace(p.metadata.namespace.as_deref(), name))
        .map(|p| policy(p))
        .collect();
    policies.sort_by(|a, b| a.name.cmp(&b.name));
    let mut quota_rows: Vec<Quota> = quotas.iter().map(|q| quota(q)).collect();
    quota_rows.sort_by(|a, b| a.name.cmp(&b.name));
    NamespaceDetail {
        name: name.to_owned(),
        pods: pod_tally,
        cpu_quota,
        memory_quota,
        quotas: Listing {
            items: quota_rows,
            state: objects.quota_state.clone(),
        },
        policies: Listing {
            items: policies,
            state: objects.policy_state.clone(),
        },
    }
}

fn hard(quota: &ResourceQuota) -> Option<&BTreeMap<String, Quantity>> {
    let status = quota.status.as_ref().and_then(|s| s.hard.as_ref());
    status.or_else(|| quota.spec.as_ref().and_then(|s| s.hard.as_ref()))
}

fn smallest(
    quotas: &[&Arc<ResourceQuota>],
    keys: &[&str],
    parse: fn(&str) -> Option<f64>,
) -> Option<f64> {
    quotas
        .iter()
        .filter_map(|q| hard(q))
        .filter_map(|hard| keys.iter().find_map(|key| hard.get(*key)))
        .filter_map(|quantity| parse(&quantity.0))
        .min_by(f64::total_cmp)
}

fn quota(quota: &ResourceQuota) -> Quota {
    let used = quota.status.as_ref().and_then(|s| s.used.as_ref());
    let items = hard(quota)
        .into_iter()
        .flatten()
        .map(|(resource, limit)| QuotaItem {
            resource: resource.clone(),
            used: used
                .and_then(|used| used.get(resource))
                .map_or_else(|| NO_USE.to_owned(), |q| q.0.clone()),
            hard: limit.0.clone(),
        })
        .collect();
    Quota {
        name: quota.metadata.name.clone().unwrap_or_default(),
        items,
    }
}

fn policy(policy: &NetworkPolicy) -> Policy {
    let summary = policy.spec.as_ref().map(summary).unwrap_or_default();
    Policy {
        name: policy.metadata.name.clone().unwrap_or_default(),
        summary,
    }
}

fn selector(selector: Option<&LabelSelector>) -> String {
    let Some(selector) = selector else {
        return ALL_PODS.to_owned();
    };
    let labels = selector.match_labels.as_ref();
    let named = labels.and_then(|labels| NAME_LABELS.iter().find_map(|key| labels.get(*key)));
    if let Some(name) = named {
        return name.clone();
    }
    let pairs = labels
        .into_iter()
        .flatten()
        .map(|(key, value)| format!("{key}={value}"));
    let expressions = selector
        .match_expressions
        .iter()
        .flatten()
        .map(|e| format!("{} {}", e.key, e.operator));
    let parts: Vec<String> = pairs.chain(expressions).collect();
    parts.join(LABEL_JOIN)
}

fn peer(peer: &NetworkPolicyPeer) -> String {
    if let Some(block) = &peer.ip_block {
        return block.cidr.clone();
    }
    let pods = peer
        .pod_selector
        .as_ref()
        .map(|s| selector(Some(s)))
        .filter(|label| !label.is_empty());
    let Some(namespaces) = peer.namespace_selector.as_ref() else {
        return pods.unwrap_or_else(|| SAME_NAMESPACE.to_owned());
    };
    let namespaces = selector(Some(namespaces));
    let namespaces = if namespaces.is_empty() {
        ALL_NAMESPACES.to_owned()
    } else {
        format!("{NAMESPACE_PREFIX}{namespaces}")
    };
    match pods {
        Some(pods) => format!("{namespaces}{PEER_SEPARATOR}{pods}"),
        None => namespaces,
    }
}

fn peers(peers: Option<&Vec<NetworkPolicyPeer>>) -> String {
    let named: Vec<String> = peers.into_iter().flatten().map(peer).collect();
    if named.is_empty() {
        return ANYWHERE.to_owned();
    }
    named.join(LIST_SEPARATOR)
}

fn ports(ports: Option<&Vec<NetworkPolicyPort>>) -> String {
    let listed: Vec<String> = ports
        .into_iter()
        .flatten()
        .filter_map(|port| {
            let number = match port.port.as_ref()? {
                IntOrString::Int(number) => number.to_string(),
                IntOrString::String(name) => name.clone(),
            };
            Some(match port.end_port {
                Some(end) => format!("{number}{RANGE}{end}"),
                None => number,
            })
        })
        .collect();
    if listed.is_empty() {
        return String::new();
    }
    format!("{PORT_PREFIX}{}", listed.join(PORT_SEPARATOR))
}

fn directed(subject: &str, peer: &str, ports: &str, inbound: bool) -> String {
    match (subject.is_empty(), inbound) {
        (true, true) => format!("{INGRESS_FROM} {peer}{ports}"),
        (true, false) => format!("{EGRESS_TO} {peer}{ports}"),
        (false, true) => format!("{peer}{ARROW}{subject}{ports}"),
        (false, false) => format!("{subject}{ARROW}{peer}{ports}"),
    }
}

fn denied(subject: &str, inbound: bool) -> String {
    match (subject.is_empty(), inbound) {
        (true, true) => DENY_INGRESS.to_owned(),
        (true, false) => DENY_EGRESS.to_owned(),
        (false, true) => format!("{DENY_INGRESS_TO} {subject}"),
        (false, false) => format!("{DENY_EGRESS_FROM} {subject}"),
    }
}

pub fn summary(spec: &NetworkPolicySpec) -> String {
    let declared = spec.policy_types.as_ref();
    let has = |kind: &str| declared.is_some_and(|types| types.iter().any(|t| t == kind));
    let ingress_rules = spec.ingress.as_deref().unwrap_or_default();
    let egress_rules = spec.egress.as_deref().unwrap_or_default();
    let ingress = declared.is_none() || has(INGRESS);
    let egress = has(EGRESS) || (declared.is_none() && !egress_rules.is_empty());
    let subject = selector(spec.pod_selector.as_ref());
    if subject.is_empty()
        && ingress
        && egress
        && ingress_rules.is_empty()
        && egress_rules.is_empty()
    {
        return DENY_ALL.to_owned();
    }
    let mut parts = Vec::new();
    if ingress {
        if ingress_rules.is_empty() {
            parts.push(denied(&subject, true));
        }
        for rule in ingress_rules {
            let from = peers(rule.from.as_ref());
            parts.push(directed(&subject, &from, &ports(rule.ports.as_ref()), true));
        }
    }
    if egress {
        if egress_rules.is_empty() {
            parts.push(denied(&subject, false));
        }
        for rule in egress_rules {
            let to = peers(rule.to.as_ref());
            parts.push(directed(&subject, &to, &ports(rule.ports.as_ref()), false));
        }
    }
    parts.join(RULE_SEPARATOR)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::problems::tests::{now, parse};

    fn spec(value: Value) -> NetworkPolicySpec {
        let parsed: Arc<NetworkPolicySpec> = parse(value);
        (*parsed).clone()
    }

    #[test]
    fn an_empty_selector_with_both_types_and_no_rules_denies_everything() {
        let policy = spec(json!({ "podSelector": {}, "policyTypes": ["Ingress", "Egress"] }));
        assert_eq!(summary(&policy), "deny all ingress + egress");
        let ingress_only = spec(json!({ "podSelector": {} }));
        assert_eq!(summary(&ingress_only), "deny all ingress");
    }

    #[test]
    fn rules_name_their_peers_ports_and_the_pods_they_cover() {
        let gateway = spec(json!({
            "podSelector": {},
            "ingress": [{ "from": [{ "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": "envoy" } } }] }]
        }));
        assert_eq!(
            summary(&gateway),
            "ingress from ns kubernetes.io/metadata.name=envoy"
        );
        let database = spec(json!({
            "podSelector": { "matchLabels": { "app": "payments-db" } },
            "ingress": [{
                "from": [{ "podSelector": { "matchLabels": { "app": "checkout-api" } } }],
                "ports": [{ "port": 5432 }]
            }]
        }));
        assert_eq!(summary(&database), "checkout-api → payments-db:5432");
        let egress = spec(json!({
            "podSelector": { "matchLabels": { "app": "worker" } },
            "policyTypes": ["Egress"],
            "egress": [
                { "to": [{ "ipBlock": { "cidr": "10.0.0.0/8" } }], "ports": [{ "port": 443, "endPort": 445 }] },
                {}
            ]
        }));
        assert_eq!(
            summary(&egress),
            "worker → 10.0.0.0/8:443-445; worker → anywhere"
        );
    }

    #[test]
    fn a_namespace_detail_tallies_pods_and_reads_quotas_and_policies() {
        let running: Arc<Pod> = parse(json!({
            "metadata": { "name": "api-1", "namespace": "payments" },
            "spec": { "containers": [{ "name": "api", "resources": { "requests": { "cpu": "500m", "memory": "1Gi" } } }] },
            "status": { "phase": "Running", "conditions": [{ "type": "Ready", "status": "True" }] }
        }));
        let crashing: Arc<Pod> = parse(json!({
            "metadata": { "name": "api-2", "namespace": "payments" },
            "spec": { "containers": [{ "name": "api", "resources": { "requests": { "cpu": "700m", "memory": "1Gi" } } }] },
            "status": { "phase": "Running", "containerStatuses": [{
                "name": "api", "image": "api", "imageID": "", "ready": false, "restartCount": 14,
                "state": { "waiting": { "reason": "CrashLoopBackOff" } }
            }] }
        }));
        let elsewhere: Arc<Pod> = parse(json!({
            "metadata": { "name": "web-1", "namespace": "web" },
            "spec": { "containers": [{ "name": "web", "resources": { "requests": { "cpu": "4" } } }] },
            "status": { "phase": "Running" }
        }));
        let quota: Arc<ResourceQuota> = parse(json!({
            "metadata": { "name": "payments-quota", "namespace": "payments" },
            "spec": { "hard": { "pods": "30", "requests.cpu": "4", "requests.memory": "8Gi" } },
            "status": {
                "hard": { "pods": "30", "requests.cpu": "4", "requests.memory": "8Gi" },
                "used": { "pods": "2", "requests.cpu": "1200m" }
            }
        }));
        let deny: Arc<NetworkPolicy> = parse(json!({
            "metadata": { "name": "default-deny", "namespace": "payments" },
            "spec": { "podSelector": {}, "policyTypes": ["Ingress", "Egress"] }
        }));
        let objects = Objects {
            pods: &[running, crashing, elsewhere],
            quotas: &[quota],
            quota_state: WatchState::Synced,
            policies: &[deny],
            policy_state: WatchState::Failed("forbidden".into()),
        };
        let detail = detail("payments", &objects, now());
        let pods: Vec<(&str, usize)> = detail
            .pods
            .iter()
            .map(|t| (t.label.as_str(), t.count))
            .collect();
        assert_eq!(pods, [("Running", 1), ("CrashLoopBackOff", 1)]);
        assert_eq!(detail.cpu_quota, Some(4.));
        assert_eq!(detail.memory_quota, Some(8.));
        let lines: Vec<String> = detail.quotas.items.iter().map(Quota::line).collect();
        assert_eq!(
            lines,
            ["payments-quota · pods 2/30 · requests.cpu 1200m/4 · requests.memory 0/8Gi"]
        );
        assert_eq!(
            detail.policies.items[0].summary,
            "deny all ingress + egress"
        );
        assert_eq!(detail.policies.unavailable(), Some("forbidden"));
    }
}
