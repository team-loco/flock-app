use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use crate::crd::{CrdKey, display_kind};
use crate::events::Event;
use crate::resource::kind_info;
use crate::{ClusterId, Counts, Page, ResourceKind, ResourceRef, Status};

pub const LIMIT: usize = 12;
pub const POD: &str = "Pod";
pub const REPLICA_SET: &str = "ReplicaSet";
pub const DEPLOYMENT: &str = "Deployment";
pub const SERVICE: &str = "Service";
pub const NAMESPACE: &str = "Namespace";
pub const NODE: &str = "Node";
pub const JOB: &str = "Job";
pub const PERSISTENT_VOLUME: &str = "PersistentVolume";
pub const AUTOSCALER: &str = "HorizontalPodAutoscaler";
pub const HTTP_ROUTE: &str = "HTTPRoute";
pub const GATEWAY_API: &str = "gateway.networking.k8s.io";
pub const CONFIG_MAP: &str = "ConfigMap";
pub const SECRET: &str = "Secret";
pub const CLAIM: &str = "PersistentVolumeClaim";
pub const TEMPLATE_HASH: &str = "pod-template-hash";
pub const CONTROLLED_BY: &str = "Controlled by";
pub const NOT_FOUND: &str = "not found";
const EVENTS: &str = "Events";
const EVENT_SEPARATOR: &str = " · ";
const HASH_SEPARATOR: char = '-';
const PLURAL_SUFFIX: &str = "s";
const CLUSTER_SCOPED: [&str; 3] = [NODE, PERSISTENT_VOLUME, NAMESPACE];
const ORDER: [&str; 17] = [
    "CronJob",
    DEPLOYMENT,
    "StatefulSet",
    "DaemonSet",
    REPLICA_SET,
    JOB,
    POD,
    SERVICE,
    "Ingress",
    HTTP_ROUTE,
    AUTOSCALER,
    CONFIG_MAP,
    SECRET,
    CLAIM,
    PERSISTENT_VOLUME,
    NODE,
    NAMESPACE,
];
const PLURALS: [(&str, &str); 4] = [
    (REPLICA_SET, "ReplicaSets"),
    (AUTOSCALER, "Autoscalers"),
    (PERSISTENT_VOLUME, "PersistentVolumes"),
    (HTTP_ROUTE, "HTTPRoutes"),
];
pub const DETAIL_KINDS: [&str; 10] = [
    JOB,
    "CronJob",
    SERVICE,
    "Ingress",
    CONFIG_MAP,
    SECRET,
    CLAIM,
    PERSISTENT_VOLUME,
    AUTOSCALER,
    NAMESPACE,
];
pub const NAMESPACE_PAGES: [Page; 11] = [
    Page::Deployments,
    Page::StatefulSets,
    Page::DaemonSets,
    Page::Jobs,
    Page::CronJobs,
    Page::Pods,
    Page::Services,
    Page::Ingresses,
    Page::ConfigMaps,
    Page::Secrets,
    Page::Pvcs,
];

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key {
    pub kind: String,
    pub name: String,
}

impl Key {
    pub fn new(kind: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            name: name.into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fact {
    pub kind: String,
    pub namespace: Option<String>,
    pub name: String,
    pub status: Status,
    pub note: String,
    pub age: Duration,
    pub labels: BTreeMap<String, String>,
    pub owner: Option<Key>,
    pub selector: BTreeMap<String, String>,
    pub template: BTreeMap<String, String>,
    pub uses: Vec<Key>,
}

impl Fact {
    pub fn new(kind: &str, namespace: Option<&str>, name: &str) -> Self {
        Self {
            kind: kind.to_owned(),
            namespace: namespace.map(str::to_owned),
            name: name.to_owned(),
            ..Self::default()
        }
    }

    fn key(&self) -> Key {
        Key::new(self.kind.clone(), self.name.clone())
    }

    fn is(&self, key: &Key, namespace: Option<&str>) -> bool {
        self.kind == key.kind && self.name == key.name && self.namespace.as_deref() == namespace
    }

    fn refers_to(&self, target: &ResourceRef) -> bool {
        self.kind == target.kind
            && self.name == target.name
            && self.namespace.as_deref() == target.namespace.as_deref()
    }

    fn references(&self, key: &Key, namespace: Option<&str>) -> bool {
        let reachable = namespace.is_none() || self.namespace.as_deref() == namespace;
        reachable && self.uses.contains(key)
    }

    fn reference(&self, cluster: &ClusterId) -> ResourceRef {
        ResourceRef {
            cluster: cluster.clone(),
            kind: self.kind.clone(),
            namespace: self.namespace.clone(),
            name: self.name.clone(),
        }
    }

    fn item(&self, cluster: &ClusterId) -> Item {
        Item {
            target: self.reference(cluster),
            label: self.name.clone(),
            status: Some(self.status),
            note: self.note.clone(),
            age: Some(self.age),
            linked: opens(&self.kind),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub target: ResourceRef,
    pub label: String,
    pub status: Option<Status>,
    pub note: String,
    pub age: Option<Duration>,
    pub linked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListLink {
    pub page: Page,
    pub namespace: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub title: String,
    pub items: Vec<Item>,
    pub total: usize,
    pub counts: Counts,
    pub list: Option<ListLink>,
}

impl Group {
    fn new(title: String, mut items: Vec<Item>, list: Option<ListLink>) -> Self {
        let total = items.len();
        let counts = Counts::of(items.iter().filter_map(|item| item.status));
        items.truncate(LIMIT);
        Self {
            title,
            items,
            total,
            counts,
            list,
        }
    }

    pub fn hidden(&self) -> usize {
        self.total.saturating_sub(self.items.len())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Related {
    pub target: ResourceRef,
    pub groups: Vec<Group>,
}

pub fn has_page(kind: &str) -> bool {
    kind_info(kind).is_some_and(|info| info.page.is_some())
}

fn opens(kind: &str) -> bool {
    has_page(kind) || DETAIL_KINDS.contains(&kind) || CrdKey::parse(kind).is_some()
}

pub fn plural(kind: &str) -> String {
    let kind = display_kind(kind);
    let fixed = PLURALS
        .iter()
        .find(|(singular, _)| *singular == kind)
        .map(|(_, plural)| (*plural).to_owned());
    let page = || {
        kind_info(kind)
            .and_then(|info| info.page)
            .filter(|page| page_kind(*page) == Some(kind) || page.singular() == kind)
            .map(|page| page.label().to_owned())
    };
    fixed
        .or_else(page)
        .unwrap_or_else(|| format!("{kind}{PLURAL_SUFFIX}"))
}

pub fn page_kind(page: Page) -> Option<&'static str> {
    match page {
        Page::Pods => Some(POD),
        other => ResourceKind::for_page(other).map(ResourceKind::name),
    }
}

fn is_cluster_scoped(kind: &str) -> bool {
    CLUSTER_SCOPED.contains(&kind)
}

pub fn deployment_name<'a>(
    replica_set: &'a str,
    labels: &BTreeMap<String, String>,
) -> Option<&'a str> {
    let hash = labels.get(TEMPLATE_HASH)?;
    replica_set
        .strip_suffix(hash.as_str())?
        .strip_suffix(HASH_SEPARATOR)
}

fn deployment_of(replica_set: &str, labels: &BTreeMap<String, String>) -> Option<Key> {
    deployment_name(replica_set, labels).map(|name| Key::new(DEPLOYMENT, name))
}

fn with_replica_sets(facts: &[Fact]) -> Vec<Fact> {
    let known: BTreeSet<(Option<&str>, &str)> = facts
        .iter()
        .filter(|fact| fact.kind == REPLICA_SET)
        .map(|fact| (fact.namespace.as_deref(), fact.name.as_str()))
        .collect();
    let mut made: BTreeMap<(Option<String>, String), (Fact, Vec<Status>)> = BTreeMap::new();
    for pod in facts.iter().filter(|fact| fact.kind == POD) {
        let Some(owner) = pod.owner.as_ref().filter(|owner| owner.kind == REPLICA_SET) else {
            continue;
        };
        if known.contains(&(pod.namespace.as_deref(), owner.name.as_str())) {
            continue;
        }
        let slot = (pod.namespace.clone(), owner.name.clone());
        let (fact, statuses) = made.entry(slot).or_insert_with(|| {
            let fact = Fact {
                owner: deployment_of(&owner.name, &pod.labels),
                ..Fact::new(REPLICA_SET, pod.namespace.as_deref(), &owner.name)
            };
            (fact, Vec::new())
        });
        fact.age = fact.age.max(pod.age);
        statuses.push(pod.status);
    }
    let synthesized = made.into_values().map(|(mut fact, statuses)| {
        fact.note = crate::overview::plural(statuses.len(), "pod", "pods");
        fact.status = Status::worst(statuses);
        fact
    });
    facts.iter().cloned().chain(synthesized).collect()
}

struct Graph<'a> {
    cluster: &'a ClusterId,
    facts: &'a [Fact],
}

impl<'a> Graph<'a> {
    fn find(&self, key: &Key, namespace: Option<&str>) -> Option<&'a Fact> {
        let namespace = if is_cluster_scoped(&key.kind) {
            None
        } else {
            namespace
        };
        self.facts.iter().find(|fact| fact.is(key, namespace))
    }

    fn missing(&self, key: &Key, namespace: Option<&str>) -> Item {
        let namespace = if is_cluster_scoped(&key.kind) {
            None
        } else {
            namespace.map(str::to_owned)
        };
        let tracked = has_page(&key.kind);
        Item {
            target: ResourceRef {
                cluster: self.cluster.clone(),
                kind: key.kind.clone(),
                namespace,
                name: key.name.clone(),
            },
            label: key.name.clone(),
            status: tracked.then_some(Status::Warn),
            note: if tracked {
                NOT_FOUND.to_owned()
            } else {
                String::new()
            },
            age: None,
            linked: false,
        }
    }

    fn ancestors(&self, fact: &'a Fact) -> Vec<Result<&'a Fact, Key>> {
        let mut chain = Vec::new();
        let mut seen = BTreeSet::from([fact.key()]);
        let mut next = fact.owner.clone();
        let namespace = fact.namespace.as_deref();
        while let Some(owner) = next.take() {
            if !seen.insert(owner.clone()) {
                break;
            }
            match self.find(&owner, namespace) {
                Some(found) => {
                    next.clone_from(&found.owner);
                    chain.push(Ok(found));
                }
                None => chain.push(Err(owner)),
            }
        }
        chain
    }

    fn descendants(&self, fact: &'a Fact) -> Vec<&'a Fact> {
        let mut found: Vec<&Fact> = Vec::new();
        let mut seen = BTreeSet::from([fact.key()]);
        let mut frontier = vec![fact.key()];
        while !frontier.is_empty() {
            let children: Vec<&Fact> = self
                .facts
                .iter()
                .filter(|child| child.namespace == fact.namespace)
                .filter(|child| child.owner.as_ref().is_some_and(|o| frontier.contains(o)))
                .filter(|child| seen.insert(child.key()))
                .collect();
            frontier = children.iter().map(|child| child.key()).collect();
            found.extend(children);
        }
        found
    }

    fn in_namespace(&self, namespace: Option<&'a str>) -> impl Iterator<Item = &'a Fact> {
        self.facts
            .iter()
            .filter(move |fact| fact.namespace.as_deref() == namespace)
    }

    fn selected_pods(&self, fact: &'a Fact) -> Vec<&'a Fact> {
        if fact.selector.is_empty() {
            return Vec::new();
        }
        self.in_namespace(fact.namespace.as_deref())
            .filter(|pod| pod.kind == POD && selects(&fact.selector, &pod.labels))
            .collect()
    }

    fn selecting_services(&self, fact: &'a Fact) -> Vec<&'a Fact> {
        let labels = if fact.kind == POD {
            &fact.labels
        } else {
            &fact.template
        };
        if labels.is_empty() {
            return Vec::new();
        }
        self.in_namespace(fact.namespace.as_deref())
            .filter(|service| service.kind == SERVICE && !service.selector.is_empty())
            .filter(|service| selects(&service.selector, labels))
            .collect()
    }

    fn users(&self, key: &Key, namespace: Option<&str>) -> Vec<&'a Fact> {
        self.facts
            .iter()
            .filter(|fact| fact.references(key, namespace))
            .collect()
    }

    fn top_users(&self, fact: &'a Fact) -> Vec<&'a Fact> {
        let namespace = if is_cluster_scoped(&fact.kind) {
            None
        } else {
            fact.namespace.as_deref()
        };
        let users = self.users(&fact.key(), namespace);
        let keys: BTreeSet<(Option<&str>, Key)> = users
            .iter()
            .map(|user| (user.namespace.as_deref(), user.key()))
            .collect();
        users
            .into_iter()
            .filter(|user| {
                !self
                    .ancestors(user)
                    .into_iter()
                    .flatten()
                    .any(|ancestor| keys.contains(&(ancestor.namespace.as_deref(), ancestor.key())))
            })
            .collect()
    }

    fn routes(&self, services: &[&'a Fact]) -> Vec<&'a Fact> {
        services
            .iter()
            .flat_map(|service| self.users(&service.key(), service.namespace.as_deref()))
            .collect()
    }

    fn used(&self, fact: &'a Fact) -> Vec<Item> {
        fact.uses
            .iter()
            .map(|key| {
                self.find(key, fact.namespace.as_deref()).map_or_else(
                    || self.missing(key, fact.namespace.as_deref()),
                    |found| found.item(self.cluster),
                )
            })
            .collect()
    }

    fn owner_items(&self, fact: &'a Fact) -> Vec<Item> {
        self.ancestors(fact)
            .into_iter()
            .map(|ancestor| match ancestor {
                Ok(found) => found.item(self.cluster),
                Err(key) => self.missing(&key, fact.namespace.as_deref()),
            })
            .collect()
    }
}

fn selects(selector: &BTreeMap<String, String>, labels: &BTreeMap<String, String>) -> bool {
    selector
        .iter()
        .all(|(key, value)| labels.get(key) == Some(value))
}

#[derive(Default)]
struct Kinds(BTreeMap<(usize, String), Vec<Item>>);

impl Kinds {
    fn add(&mut self, items: impl IntoIterator<Item = Item>) {
        for item in items {
            let slot = (rank(&item.target.kind), item.target.kind.clone());
            let bucket = self.0.entry(slot).or_default();
            if !bucket.iter().any(|known| known.target == item.target) {
                bucket.push(item);
            }
        }
    }

    fn groups(self) -> impl Iterator<Item = Group> {
        self.0.into_iter().map(|((_, kind), mut items)| {
            sort(&kind, &mut items);
            Group::new(plural(&kind), items, None)
        })
    }
}

fn rank(kind: &str) -> usize {
    let kind = display_kind(kind);
    ORDER
        .iter()
        .position(|known| *known == kind)
        .unwrap_or(ORDER.len())
}

fn sort(kind: &str, items: &mut [Item]) {
    if kind == JOB {
        items.sort_by_key(|item| item.age);
    } else {
        items.sort_by(|a, b| {
            Reverse(a.status)
                .cmp(&Reverse(b.status))
                .then_with(|| a.label.cmp(&b.label))
        });
    }
}

pub fn related(cluster: &ClusterId, facts: &[Fact], target: &ResourceRef) -> Related {
    let facts = with_replica_sets(facts);
    let graph = Graph {
        cluster,
        facts: &facts,
    };
    let groups = facts
        .iter()
        .find(|fact| fact.refers_to(target))
        .map(|fact| groups_of(&graph, fact))
        .unwrap_or_default();
    Related {
        target: target.clone(),
        groups,
    }
}

fn groups_of<'a>(graph: &Graph<'a>, fact: &'a Fact) -> Vec<Group> {
    let mut groups = Vec::new();
    let owners = graph.owner_items(fact);
    if !owners.is_empty() {
        groups.push(Group::new(CONTROLLED_BY.to_owned(), owners, None));
    }
    let cluster = graph.cluster;
    let mut kinds = Kinds::default();
    kinds.add(graph.descendants(fact).into_iter().map(|f| f.item(cluster)));
    kinds.add(
        graph
            .selected_pods(fact)
            .into_iter()
            .map(|f| f.item(cluster)),
    );
    let services = graph.selecting_services(fact);
    kinds.add(services.iter().map(|f| f.item(cluster)));
    kinds.add(graph.routes(&services).into_iter().map(|f| f.item(cluster)));
    kinds.add(graph.top_users(fact).into_iter().map(|f| f.item(cluster)));
    kinds.add(graph.used(fact));
    groups.extend(kinds.groups());
    groups
}

pub fn contents(cluster: &ClusterId, facts: &[Fact], events: &[Event], namespace: &str) -> Related {
    let target = ResourceRef {
        cluster: cluster.clone(),
        kind: NAMESPACE.to_owned(),
        namespace: None,
        name: namespace.to_owned(),
    };
    let link = |page: Page| {
        Some(ListLink {
            page,
            namespace: namespace.to_owned(),
        })
    };
    let facts = with_replica_sets(facts);
    let group = |kind: &str| {
        let mut items: Vec<Item> = facts
            .iter()
            .filter(|fact| fact.kind == kind && fact.namespace.as_deref() == Some(namespace))
            .map(|fact| fact.item(cluster))
            .collect();
        sort(kind, &mut items);
        items
    };
    let mut groups: Vec<Group> = Vec::new();
    for page in NAMESPACE_PAGES {
        let Some(kind) = page_kind(page) else {
            continue;
        };
        groups.push(Group::new(page.label().to_owned(), group(kind), link(page)));
        if kind == POD {
            groups.push(Group::new(plural(REPLICA_SET), group(REPLICA_SET), None));
        }
    }
    let events: Vec<Item> = events
        .iter()
        .filter(|event| event.namespace.as_deref() == Some(namespace))
        .filter_map(|event| {
            let target = event.target(cluster)?;
            let linked = has_page(&target.kind);
            Some(Item {
                label: format!("{}{EVENT_SEPARATOR}{}", event.reason, event.object),
                status: Some(event.status),
                note: event.message.clone(),
                age: Some(event.age),
                linked,
                target,
            })
        })
        .collect();
    groups.push(Group::new(EVENTS.to_owned(), events, link(Page::Events)));
    Related { target, groups }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NS: &str = "shop";

    fn cluster() -> ClusterId {
        ClusterId::new("a")
    }

    fn fact(kind: &str, name: &str) -> Fact {
        Fact::new(kind, Some(NS), name)
    }

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn target(kind: &str, namespace: Option<&str>, name: &str) -> ResourceRef {
        ResourceRef {
            cluster: cluster(),
            kind: kind.into(),
            namespace: namespace.map(str::to_owned),
            name: name.into(),
        }
    }

    fn pod(name: &str, replica_set: &str, hash: &str, node: &str) -> Fact {
        Fact {
            labels: labels(&[("app", "cart"), (TEMPLATE_HASH, hash)]),
            owner: Some(Key::new(REPLICA_SET, replica_set)),
            uses: vec![
                Key::new(NODE, node),
                Key::new(CONFIG_MAP, "cart-config"),
                Key::new(SECRET, "cart-env"),
            ],
            ..fact(POD, name)
        }
    }

    fn shop() -> Vec<Fact> {
        let deployment = Fact {
            template: labels(&[("app", "cart")]),
            uses: vec![
                Key::new(CONFIG_MAP, "cart-config"),
                Key::new(SECRET, "cart-env"),
                Key::new(SECRET, "cart-tls"),
            ],
            ..fact(DEPLOYMENT, "cart")
        };
        let crashing = Fact {
            status: Status::Err,
            ..pod("cart-7f9c-b", "cart-7f9c", "7f9c", "node-b")
        };
        let service = Fact {
            selector: labels(&[("app", "cart")]),
            ..fact(SERVICE, "cart")
        };
        let other = Fact {
            selector: labels(&[("app", "web")]),
            ..fact(SERVICE, "web")
        };
        let ingress = Fact {
            uses: vec![Key::new(SERVICE, "cart")],
            ..fact("Ingress", "shop")
        };
        let autoscaler = Fact {
            uses: vec![Key::new(DEPLOYMENT, "cart")],
            ..fact(AUTOSCALER, "cart")
        };
        let bare = Fact {
            uses: vec![
                Key::new(CONFIG_MAP, "cart-config"),
                Key::new(NODE, "node-a"),
            ],
            ..fact(POD, "debug")
        };
        vec![
            deployment,
            pod("cart-7f9c-a", "cart-7f9c", "7f9c", "node-a"),
            crashing,
            service,
            other,
            ingress,
            autoscaler,
            bare,
            fact(CONFIG_MAP, "cart-config"),
            fact(SECRET, "cart-env"),
            Fact::new(NODE, None, "node-a"),
            Fact::new(NODE, None, "node-b"),
        ]
    }

    fn titles(related: &Related) -> Vec<&str> {
        related.groups.iter().map(|g| g.title.as_str()).collect()
    }

    fn names<'a>(related: &'a Related, title: &str) -> Vec<&'a str> {
        related
            .groups
            .iter()
            .find(|g| g.title == title)
            .map(|g| g.items.iter().map(|i| i.label.as_str()).collect())
            .unwrap_or_default()
    }

    fn item<'a>(related: &'a Related, title: &str, label: &str) -> Option<&'a Item> {
        related
            .groups
            .iter()
            .find(|g| g.title == title)?
            .items
            .iter()
            .find(|i| i.label == label)
    }

    #[test]
    fn a_deployment_lists_replica_sets_pods_services_routes_autoscaler_and_config() {
        let related = related(&cluster(), &shop(), &target(DEPLOYMENT, Some(NS), "cart"));
        assert_eq!(
            titles(&related),
            [
                "ReplicaSets",
                "Pods",
                "Services",
                "Ingresses",
                "Autoscalers",
                "ConfigMaps",
                "Secrets"
            ]
        );
        assert_eq!(names(&related, "ReplicaSets"), ["cart-7f9c"]);
        assert_eq!(names(&related, "Pods"), ["cart-7f9c-b", "cart-7f9c-a"]);
        assert_eq!(names(&related, "Services"), ["cart"]);
        assert_eq!(names(&related, "Ingresses"), ["shop"]);
        assert_eq!(names(&related, "Autoscalers"), ["cart"]);
        assert_eq!(names(&related, "ConfigMaps"), ["cart-config"]);
        assert_eq!(names(&related, "Secrets"), ["cart-tls", "cart-env"]);
        let missing = item(&related, "Secrets", "cart-tls");
        assert_eq!(
            missing.map(|i| (i.status, i.note.as_str(), i.linked)),
            Some((Some(Status::Warn), NOT_FOUND, false))
        );
        let replica_set = item(&related, "ReplicaSets", "cart-7f9c");
        assert_eq!(
            replica_set.map(|i| (i.status, i.note.as_str(), i.linked)),
            Some((Some(Status::Err), "2 pods", false))
        );
        assert!(item(&related, "Pods", "cart-7f9c-a").is_some_and(|i| i.linked));
    }

    #[test]
    fn a_pod_names_its_owner_chain_node_services_and_references() {
        let related = related(&cluster(), &shop(), &target(POD, Some(NS), "cart-7f9c-a"));
        assert_eq!(
            titles(&related),
            [
                CONTROLLED_BY,
                "Services",
                "Ingresses",
                "ConfigMaps",
                "Secrets",
                "Nodes"
            ]
        );
        assert_eq!(names(&related, CONTROLLED_BY), ["cart-7f9c", "cart"]);
        assert_eq!(names(&related, "Nodes"), ["node-a"]);
        assert_eq!(names(&related, "Services"), ["cart"]);
        assert_eq!(names(&related, "Ingresses"), ["shop"]);
        assert_eq!(
            item(&related, "Nodes", "node-a").map(|i| i.target.namespace.clone()),
            Some(None)
        );
    }

    #[test]
    fn a_service_lists_the_pods_it_selects_and_the_routes_to_it() {
        let service = related(&cluster(), &shop(), &target(SERVICE, Some(NS), "cart"));
        assert_eq!(titles(&service), ["Pods", "Ingresses"]);
        assert_eq!(names(&service, "Pods"), ["cart-7f9c-b", "cart-7f9c-a"]);
        let unmatched = related(&cluster(), &shop(), &target(SERVICE, Some(NS), "web"));
        assert_eq!(unmatched.groups, []);
    }

    #[test]
    fn a_node_lists_every_pod_scheduled_on_it() {
        let related = related(&cluster(), &shop(), &target(NODE, None, "node-a"));
        assert_eq!(titles(&related), ["Pods"]);
        assert_eq!(names(&related, "Pods"), ["cart-7f9c-a", "debug"]);
    }

    #[test]
    fn config_consumers_are_the_topmost_objects_that_reference_it() {
        let related = related(
            &cluster(),
            &shop(),
            &target(CONFIG_MAP, Some(NS), "cart-config"),
        );
        assert_eq!(titles(&related), ["Deployments", "Pods"]);
        assert_eq!(names(&related, "Deployments"), ["cart"]);
        assert_eq!(names(&related, "Pods"), ["debug"]);
    }

    #[test]
    fn a_claim_links_its_consumers_and_names_its_volume() {
        let claim = Fact {
            uses: vec![Key::new(PERSISTENT_VOLUME, "pv-1")],
            ..fact(CLAIM, "data")
        };
        let consumer = Fact {
            owner: Some(Key::new("StatefulSet", "db")),
            uses: vec![Key::new(CLAIM, "data")],
            ..fact(POD, "db-0")
        };
        let facts = vec![claim, consumer, fact("StatefulSet", "db")];
        let claim = related(&cluster(), &facts, &target(CLAIM, Some(NS), "data"));
        assert_eq!(titles(&claim), ["Pods", "PersistentVolumes"]);
        let volume = item(&claim, "PersistentVolumes", "pv-1");
        assert_eq!(
            volume.map(|i| (i.status, i.linked, i.target.namespace.clone())),
            Some((None, false, None))
        );
        let pod = related(&cluster(), &facts, &target(POD, Some(NS), "db-0"));
        assert_eq!(names(&pod, CONTROLLED_BY), ["db"]);
        assert_eq!(names(&pod, "PVCs"), ["data"]);
    }

    #[test]
    fn a_bound_volume_links_both_ways_and_routes_and_autoscalers_open() {
        let claim = Fact {
            uses: vec![Key::new(PERSISTENT_VOLUME, "pv-1")],
            ..fact(CLAIM, "data")
        };
        let route_kind = CrdKey::new(GATEWAY_API, HTTP_ROUTE).qualified();
        let route = Fact {
            uses: vec![Key::new(SERVICE, "cart")],
            ..fact(&route_kind, "web")
        };
        let mut facts = shop();
        facts.extend([claim, route, Fact::new(PERSISTENT_VOLUME, None, "pv-1")]);
        let claim = related(&cluster(), &facts, &target(CLAIM, Some(NS), "data"));
        let volume = item(&claim, "PersistentVolumes", "pv-1");
        assert_eq!(
            volume.map(|i| (i.linked, i.target.namespace.clone())),
            Some((true, None))
        );
        let volume = related(&cluster(), &facts, &target(PERSISTENT_VOLUME, None, "pv-1"));
        assert_eq!(titles(&volume), ["PVCs"]);
        assert!(item(&volume, "PVCs", "data").is_some_and(|i| i.linked));
        let service = related(&cluster(), &facts, &target(SERVICE, Some(NS), "cart"));
        assert_eq!(titles(&service), ["Pods", "Ingresses", "HTTPRoutes"]);
        let deployment = related(&cluster(), &facts, &target(DEPLOYMENT, Some(NS), "cart"));
        assert!(item(&deployment, "HTTPRoutes", "web").is_some_and(|i| i.linked));
        assert!(item(&deployment, "Autoscalers", "cart").is_some_and(|i| i.linked));
        let route = related(&cluster(), &facts, &target(&route_kind, Some(NS), "web"));
        assert_eq!(names(&route, "Services"), ["cart"]);
    }

    #[test]
    fn a_cron_job_lists_its_job_history_newest_first() {
        let job = |name: &str, minutes: u64, status: Status| Fact {
            owner: Some(Key::new("CronJob", "nightly")),
            age: Duration::from_secs(minutes * 60),
            status,
            ..fact(JOB, name)
        };
        let pod = Fact {
            owner: Some(Key::new(JOB, "nightly-2")),
            ..fact(POD, "nightly-2-x")
        };
        let facts = vec![
            fact("CronJob", "nightly"),
            job("nightly-1", 120, Status::Ok),
            job("nightly-3", 5, Status::Ok),
            job("nightly-2", 60, Status::Err),
            pod,
        ];
        let cron = related(&cluster(), &facts, &target("CronJob", Some(NS), "nightly"));
        assert_eq!(
            names(&cron, "Jobs"),
            ["nightly-3", "nightly-2", "nightly-1"]
        );
        assert_eq!(names(&cron, "Pods"), ["nightly-2-x"]);
        let job = related(&cluster(), &facts, &target(JOB, Some(NS), "nightly-2"));
        assert_eq!(titles(&job), [CONTROLLED_BY, "Pods"]);
        assert_eq!(names(&job, CONTROLLED_BY), ["nightly"]);
    }

    #[test]
    fn unknown_targets_and_owner_cycles_end_quietly() {
        let looped = vec![
            Fact {
                owner: Some(Key::new(JOB, "b")),
                ..fact(JOB, "a")
            },
            Fact {
                owner: Some(Key::new(JOB, "a")),
                ..fact(JOB, "b")
            },
        ];
        let related_a = related(&cluster(), &looped, &target(JOB, Some(NS), "a"));
        assert_eq!(names(&related_a, CONTROLLED_BY), ["b"]);
        assert_eq!(names(&related_a, "Jobs"), ["b"]);
        let gone = related(&cluster(), &looped, &target(JOB, Some(NS), "zzz"));
        assert_eq!(gone.groups, []);
    }

    #[test]
    fn groups_keep_the_worst_first_and_count_what_they_hide() {
        let mut facts: Vec<Fact> = (0..LIMIT + 3)
            .map(|ix| Fact {
                uses: vec![Key::new(NODE, "big")],
                status: if ix == LIMIT + 2 {
                    Status::Err
                } else {
                    Status::Ok
                },
                ..fact(POD, &format!("p{ix:02}"))
            })
            .collect();
        facts.push(Fact::new(NODE, None, "big"));
        let related = related(&cluster(), &facts, &target(NODE, None, "big"));
        let Some(group) = related.groups.first() else {
            panic!("pods group");
        };
        assert_eq!(group.items.len(), LIMIT);
        assert_eq!(group.total, LIMIT + 3);
        assert_eq!(group.hidden(), 3);
        assert_eq!(group.counts.err, 1);
        assert_eq!(group.items.first().map(|i| i.label.as_str()), Some("p14"));
    }

    fn event(namespace: &str, reason: &str, object: &str) -> Event {
        Event {
            labels: BTreeMap::new(),
            details: crate::events::EventDetails::default(),
            status: Status::Warn,
            reason: reason.into(),
            object: object.into(),
            namespace: Some(namespace.into()),
            message: "Back-off restarting failed container".into(),
            age: Duration::ZERO,
            count: 1,
        }
    }

    #[test]
    fn a_namespace_lists_workloads_then_pods_and_replica_sets_then_the_rest_with_list_links() {
        let mut facts = shop();
        facts.push(Fact::new(DEPLOYMENT, Some("web"), "web"));
        let events = vec![
            event(NS, "BackOff", "pod/cart-7f9c-b"),
            event("web", "Pulled", "pod/web-1"),
        ];
        let related = contents(&cluster(), &facts, &events, NS);
        assert_eq!(related.target, target(NAMESPACE, None, NS));
        assert_eq!(
            titles(&related),
            [
                "Deployments",
                "StatefulSets",
                "DaemonSets",
                "Jobs",
                "CronJobs",
                "Pods",
                "ReplicaSets",
                "Services",
                "Ingresses",
                "ConfigMaps",
                "Secrets",
                "PVCs",
                "Events"
            ]
        );
        let total = |title: &str| {
            related
                .groups
                .iter()
                .find(|g| g.title == title)
                .map(|g| g.total)
        };
        assert_eq!(total("Pods"), Some(3));
        assert_eq!(total("Deployments"), Some(1));
        assert_eq!(total("PVCs"), Some(0));
        assert_eq!(total("ReplicaSets"), Some(1));
        assert_eq!(names(&related, "Pods").first(), Some(&"cart-7f9c-b"));
        let events = related.groups.last();
        assert_eq!(
            events.and_then(|g| g.list.clone()),
            Some(ListLink {
                page: Page::Events,
                namespace: NS.into()
            })
        );
        let backoff = events.and_then(|g| g.items.first());
        assert_eq!(
            backoff.map(|i| (i.label.as_str(), i.target.name.as_str(), i.linked)),
            Some(("BackOff · pod/cart-7f9c-b", "cart-7f9c-b", true))
        );
        assert_eq!(
            related
                .groups
                .first()
                .and_then(|g| g.list.as_ref())
                .map(|l| l.page),
            Some(Page::Deployments)
        );
    }

    #[test]
    fn plurals_follow_the_sidebar_labels() {
        assert_eq!(plural(POD), "Pods");
        assert_eq!(plural(CLAIM), "PVCs");
        assert_eq!(plural(REPLICA_SET), "ReplicaSets");
        assert_eq!(plural(NODE), "Nodes");
        assert_eq!(plural("Widget"), "Widgets");
    }
}
