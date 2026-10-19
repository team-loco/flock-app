use std::cmp::Reverse;
use std::ops::Range;

use crate::filter::{LabelMatch, label_matches, label_token, namespace_token};
use crate::fuzzy::fuzzy;
use crate::resource::{kind_info, kind_matches, kind_rank, short_kind};
use crate::{
    ClusterId, ClusterSummary, Command, FLOCK_NAV, LogTarget, Page, ResourceRef, ResourceSummary,
    Snapshot, Status,
};

const MAX_RESOURCES: usize = 50;
const PRIVILEGED_PORTS: u16 = 1024;
const LOCAL_PORT_OFFSET: u16 = 8000;
const CLUSTER_PREFIX: &str = "@";
const KIND_PREFIX: &str = "k:";
pub const COMMAND_PREFIX: &str = ">";
const FLOCK: &str = "Flock";
const CLUSTER_KIND: &str = "cluster";
const PAGE_KIND: &str = "page";
pub const LOGS_KEY: &str = "⌘L";
pub const COMPARE_KEY: &str = "⌘⇧D";
pub const OWNER_KEY: &str = "⌘⇧G";
pub const STOP_FORWARD: &str = "Stop port-forward";
pub const CHECK_FOR_UPDATES: &str = "Flock: Check for updates";

const GLOBAL_COMMANDS: &[(&str, &str, Command)] = &[
    ("Toggle sidebar", "⌘B", Command::ToggleSidebar),
    ("Open settings", "⌘,", Command::SelectPage(Page::Settings)),
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    pub cluster: Option<String>,
    pub namespace: Option<String>,
    pub kind: Option<String>,
    pub label: Option<LabelMatch>,
    pub commands_only: bool,
}

impl Query {
    pub fn parse(input: &str) -> Self {
        let trimmed = input.trim_start();
        let (commands_only, rest) = match trimmed.strip_prefix(COMMAND_PREFIX) {
            Some(rest) => (true, rest),
            None => (false, trimmed),
        };
        let mut query = Self {
            commands_only,
            ..Self::default()
        };
        let mut words = Vec::new();
        for token in rest.split_whitespace() {
            if let Some(cluster) = token.strip_prefix(CLUSTER_PREFIX) {
                query.cluster = Some(cluster.to_lowercase());
            } else if let Some(namespace) = namespace_token(token) {
                query.namespace = Some(namespace);
            } else if let Some(kind) = token.strip_prefix(KIND_PREFIX) {
                query.kind = Some(kind.to_owned());
            } else if let Some(label) = label_token(token) {
                query.label = Some(label);
            } else {
                words.push(token);
            }
        }
        query.text = words.join(" ");
        query
    }

    fn filters_resources(&self) -> bool {
        self.namespace.is_some() || self.kind.is_some() || self.label.is_some()
    }

    fn in_cluster(&self, name: &str) -> bool {
        self.cluster
            .as_ref()
            .is_none_or(|wanted| name.to_lowercase().contains(wanted))
    }

    fn keeps(&self, summary: &ResourceSummary) -> bool {
        let resource = &summary.resource;
        let namespace_ok = self.namespace.as_ref().is_none_or(|wanted| {
            resource
                .namespace
                .as_ref()
                .is_some_and(|ns| ns.to_lowercase().contains(wanted))
        });
        let kind_ok = self
            .kind
            .as_ref()
            .is_none_or(|wanted| kind_matches(&resource.kind, wanted));
        let label_ok = self
            .label
            .as_ref()
            .is_none_or(|wanted| label_matches(wanted, &summary.labels));
        namespace_ok && kind_ok && label_ok
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Section {
    Clusters,
    Pages,
    Resources,
    Commands,
}

impl Section {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Clusters => "Clusters",
            Self::Pages => "Pages",
            Self::Resources => "Resources",
            Self::Commands => "Commands",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub section: Section,
    pub kind: String,
    pub name: String,
    pub highlights: Vec<Range<usize>>,
    pub namespace: String,
    pub cluster: String,
    pub status: Option<Status>,
    pub detail: String,
    pub shortcut: Option<&'static str>,
    pub command: Command,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Results {
    pub hits: Vec<Hit>,
    pub clusters: usize,
}

impl Results {
    pub fn summary(&self) -> String {
        let matches = plural(self.hits.len(), "match", "matches");
        let clusters = plural(self.clusters, "cluster", "clusters");
        format!("{matches} · {clusters}")
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    let noun = if n == 1 { one } else { many };
    format!("{n} {noun}")
}

pub fn search(snapshot: &Snapshot, current: Option<&ClusterId>, input: &str) -> Results {
    let query = Query::parse(input);
    let clusters: Vec<&ClusterSummary> = snapshot
        .clusters
        .iter()
        .filter(|c| query.in_cluster(&c.name))
        .collect();
    if query.commands_only {
        return Results {
            hits: command_mode_hits(snapshot, &query),
            clusters: clusters.len(),
        };
    }
    let mut hits = Vec::new();
    let browsing = !query.filters_resources();
    let unscoped = query.cluster.is_none();
    if browsing {
        hits.extend(cluster_hits(&clusters, &query.text));
    }
    if browsing && unscoped {
        hits.extend(page_hits(snapshot, current, &query.text));
    }
    let resources = resource_hits(snapshot, &query);
    let contextual = contextual_commands(snapshot, &resources);
    hits.extend(resources.into_iter().map(|(hit, _)| hit));
    hits.extend(contextual);
    if browsing && unscoped {
        hits.extend(global_commands(snapshot, &query.text));
    }
    Results {
        hits,
        clusters: clusters.len(),
    }
}

fn matched(text: &str, query: &str) -> Option<(i64, Vec<Range<usize>>)> {
    if query.trim().is_empty() {
        return Some((0, Vec::new()));
    }
    fuzzy(query, text).map(|m| (m.score, m.ranges))
}

fn cluster_hits(clusters: &[&ClusterSummary], text: &str) -> Vec<Hit> {
    let mut scored: Vec<(i64, Hit)> = clusters
        .iter()
        .filter_map(|cluster| {
            let (score, highlights) = matched(&cluster.name, text)?;
            let hit = Hit {
                section: Section::Clusters,
                kind: CLUSTER_KIND.into(),
                name: cluster.name.clone(),
                highlights,
                namespace: String::new(),
                cluster: format!("{} · {}", cluster.distro, cluster.region),
                status: Some(cluster.status),
                detail: cluster.state.clone(),
                shortcut: None,
                command: Command::SelectCluster(Some(cluster.id.clone())),
            };
            Some((score, hit))
        })
        .collect();
    scored.sort_by_key(|(score, _)| Reverse(*score));
    scored.into_iter().map(|(_, hit)| hit).collect()
}

fn page_hits(snapshot: &Snapshot, current: Option<&ClusterId>, text: &str) -> Vec<Hit> {
    let flock_pages = FLOCK_NAV
        .iter()
        .flat_map(|section| section.pages.iter())
        .map(|page| (*page, FLOCK.to_owned()));
    let cluster_name = current
        .and_then(|id| snapshot.cluster(id))
        .map(|cluster| cluster.name.clone());
    let cluster_sections = current.map(|id| snapshot.nav_sections(Some(id)));
    let cluster_pages =
        cluster_name
            .zip(cluster_sections)
            .into_iter()
            .flat_map(|(name, sections)| {
                sections
                    .into_iter()
                    .flat_map(|section| section.pages.iter())
                    .map(move |page| (*page, name.clone()))
            });
    let mut scored: Vec<(i64, Hit)> = flock_pages
        .chain(cluster_pages)
        .filter_map(|(page, scope)| {
            let (score, highlights) = matched(page.label(), text)?;
            let hit = Hit {
                section: Section::Pages,
                kind: PAGE_KIND.into(),
                name: page.label().into(),
                highlights,
                namespace: String::new(),
                cluster: scope,
                status: None,
                detail: String::new(),
                shortcut: None,
                command: Command::SelectPage(page),
            };
            Some((score, hit))
        })
        .collect();
    if text.trim().is_empty() {
        return scored.into_iter().map(|(_, hit)| hit).collect();
    }
    scored.sort_by_key(|(score, _)| Reverse(*score));
    scored.into_iter().map(|(_, hit)| hit).collect()
}

fn resource_hits<'a>(snapshot: &'a Snapshot, query: &Query) -> Vec<(Hit, &'a ResourceSummary)> {
    if query.text.trim().is_empty() && !query.filters_resources() && query.cluster.is_none() {
        return Vec::new();
    }
    ranked_resources(snapshot, query, &query.text)
        .into_iter()
        .take(MAX_RESOURCES)
        .map(|(highlights, summary)| (resource_hit(snapshot, summary, highlights), summary))
        .collect()
}

fn ranked_resources<'a>(
    snapshot: &'a Snapshot,
    query: &Query,
    text: &str,
) -> Vec<(Vec<Range<usize>>, &'a ResourceSummary)> {
    let cluster_order = |id: &ClusterId| {
        snapshot
            .clusters
            .iter()
            .position(|c| &c.id == id)
            .unwrap_or(snapshot.clusters.len())
    };
    let mut scored: Vec<_> = snapshot
        .resources
        .iter()
        .filter(|summary| {
            let cluster_name = snapshot
                .cluster(&summary.resource.cluster)
                .map_or(summary.resource.cluster.as_str(), |c| c.name.as_str());
            query.in_cluster(cluster_name) && query.keeps(summary)
        })
        .filter_map(|summary| {
            let (score, highlights) = matched(&summary.resource.name, text)?;
            Some((score, highlights, summary))
        })
        .collect();
    scored.sort_by_key(|(score, _, summary)| {
        let resource = &summary.resource;
        (
            Reverse(*score),
            Reverse(summary.status),
            kind_rank(&resource.kind),
            cluster_order(&resource.cluster),
            resource.name.clone(),
        )
    });
    scored
        .into_iter()
        .map(|(_, highlights, summary)| (highlights, summary))
        .collect()
}

fn resource_hit(
    snapshot: &Snapshot,
    summary: &ResourceSummary,
    highlights: Vec<Range<usize>>,
) -> Hit {
    let resource = &summary.resource;
    let cluster = snapshot
        .cluster(&resource.cluster)
        .map_or_else(|| resource.cluster.to_string(), |c| c.name.clone());
    Hit {
        section: Section::Resources,
        kind: short_kind(&resource.kind),
        name: resource.name.clone(),
        highlights,
        namespace: resource.namespace.clone().unwrap_or_default(),
        cluster,
        status: summary.status,
        detail: summary.detail.clone(),
        shortcut: None,
        command: Command::OpenResource(resource.clone()),
    }
}

fn command_hit(label: String, shortcut: Option<&'static str>, command: Command) -> Hit {
    Hit {
        section: Section::Commands,
        kind: COMMAND_PREFIX.into(),
        name: label,
        highlights: Vec::new(),
        namespace: String::new(),
        cluster: String::new(),
        status: None,
        detail: String::new(),
        shortcut,
        command,
    }
}

fn is_workload(summary: &ResourceSummary) -> bool {
    kind_info(&summary.resource.kind).is_some_and(|info| info.has_pods)
}

fn contextual_commands(snapshot: &Snapshot, resources: &[(Hit, &ResourceSummary)]) -> Vec<Hit> {
    let workload = resources.iter().find(|(_, summary)| is_workload(summary));
    let mut commands = workload
        .map(|(hit, summary)| workload_commands(snapshot, hit, summary))
        .unwrap_or_default();
    let service = resources
        .iter()
        .find_map(|(hit, summary)| service_command(hit, summary));
    commands.extend(service);
    commands
}

pub fn resource_commands(snapshot: &Snapshot, resource: &ResourceRef) -> Vec<Hit> {
    snapshot
        .resources
        .iter()
        .find(|s| &s.resource == resource)
        .map(|summary| summary_commands(snapshot, summary))
        .unwrap_or_default()
}

fn summary_commands(snapshot: &Snapshot, summary: &ResourceSummary) -> Vec<Hit> {
    let hit = resource_hit(snapshot, summary, Vec::new());
    let mut commands = if is_workload(summary) {
        workload_commands(snapshot, &hit, summary)
    } else {
        Vec::new()
    };
    commands.extend(service_command(&hit, summary));
    commands
}

fn workload_commands(snapshot: &Snapshot, hit: &Hit, summary: &ResourceSummary) -> Vec<Hit> {
    let resource = &summary.resource;
    let target = format!("{}/{}", hit.kind, resource.name);
    let mut commands = vec![command_hit(
        format!("Logs: follow all {} pods on {}", resource.name, hit.cluster),
        Some(LOGS_KEY),
        Command::FollowLogs(LogTarget::Workload(resource.clone())),
    )];
    if elsewhere(snapshot, resource) {
        commands.push(command_hit(
            format!("Compare {target} across clusters"),
            Some(COMPARE_KEY),
            Command::Compare(resource.clone()),
        ));
    }
    if let Some(owner) = &summary.owner {
        commands.push(command_hit(
            format!("Show Flux owner of {target}"),
            Some(OWNER_KEY),
            Command::OpenResource(owner.clone()),
        ));
    }
    commands
}

fn service_command(hit: &Hit, summary: &ResourceSummary) -> Option<Hit> {
    let is_service = kind_info(&summary.resource.kind).is_some_and(|info| info.short == "svc");
    let remote = summary.ports.first().copied().filter(|_| is_service)?;
    let local = local_port(remote);
    let resource = &summary.resource;
    Some(command_hit(
        format!(
            "Port-forward {}/{} {local} → {remote}",
            hit.kind, resource.name
        ),
        None,
        Command::PortForward {
            service: resource.clone(),
            local,
            remote,
        },
    ))
}

fn command_mode_hits(snapshot: &Snapshot, query: &Query) -> Vec<Hit> {
    let words: Vec<&str> = query.text.split_whitespace().collect();
    let mut targets: Vec<&ResourceSummary> = Vec::new();
    let named = words
        .iter()
        .flat_map(|word| ranked_resources(snapshot, query, word));
    for (_, summary) in named {
        if !targets.iter().any(|t| t.resource == summary.resource) {
            targets.push(summary);
        }
    }
    if targets.is_empty() && !words.is_empty() {
        let all = ranked_resources(snapshot, query, "");
        targets = all.into_iter().map(|(_, summary)| summary).collect();
    }
    let contextual = targets
        .into_iter()
        .flat_map(|summary| summary_commands(snapshot, summary));
    let global = global_commands(snapshot, "");
    let updates = command_hit(CHECK_FOR_UPDATES.to_owned(), None, Command::CheckForUpdates);
    let mut scored: Vec<(i64, Hit)> = Vec::new();
    let hits = global.into_iter().chain([updates]).chain(contextual);
    for mut hit in hits {
        if scored.iter().any(|(_, kept)| kept.name == hit.name) {
            continue;
        }
        let Some((score, highlights)) = matched_words(&hit.name, &words) else {
            continue;
        };
        hit.highlights = highlights;
        scored.push((score, hit));
    }
    scored.sort_by_key(|(score, _)| Reverse(*score));
    scored
        .into_iter()
        .take(MAX_RESOURCES)
        .map(|(_, hit)| hit)
        .collect()
}

fn starts_word(text: &str, at: usize) -> bool {
    text.get(..at)
        .and_then(|before| before.chars().next_back())
        .is_none_or(|c| !c.is_alphanumeric())
}

fn matched_words(text: &str, words: &[&str]) -> Option<(i64, Vec<Range<usize>>)> {
    let mut total = 0;
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for word in words {
        let (score, found) = matched(text, word)?;
        let start = found.first().map_or(0, |range| range.start);
        if !starts_word(text, start) {
            return None;
        }
        total += score;
        ranges.extend(found);
    }
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            Some(_) => merged.push(range),
            None => merged.push(range),
        }
    }
    Some((total, merged))
}

fn elsewhere(snapshot: &Snapshot, resource: &ResourceRef) -> bool {
    snapshot.resources.iter().any(|other| {
        let other = &other.resource;
        other.cluster != resource.cluster
            && other.kind == resource.kind
            && other.namespace == resource.namespace
            && other.name == resource.name
    })
}

pub fn local_port(remote: u16) -> u16 {
    if remote < PRIVILEGED_PORTS {
        remote + LOCAL_PORT_OFFSET
    } else {
        remote
    }
}

fn global_commands(snapshot: &Snapshot, text: &str) -> Vec<Hit> {
    let fixed = GLOBAL_COMMANDS
        .iter()
        .map(|(label, key, command)| ((*label).to_owned(), Some(*key), command.clone()));
    let stops = snapshot.forwards.iter().map(|forward| {
        let label = format!("{STOP_FORWARD} {}", forward.label());
        let command = Command::StopPortForward {
            local: forward.local,
        };
        (label, None, command)
    });
    let mut scored: Vec<(i64, Hit)> = fixed
        .chain(stops)
        .filter_map(|(label, key, command)| {
            let (score, highlights) = matched(&label, text)?;
            let mut hit = command_hit(label, key, command);
            hit.highlights = highlights;
            Some((score, hit))
        })
        .collect();
    scored.sort_by_key(|(score, _)| Reverse(*score));
    scored.into_iter().map(|(_, hit)| hit).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::flock::tests::cluster;
    use crate::{Counts, Page};

    fn resource(cluster: &str, kind: &str, namespace: &str, name: &str) -> ResourceRef {
        ResourceRef {
            cluster: ClusterId::new(cluster),
            kind: kind.into(),
            namespace: Some(namespace.into()),
            name: name.into(),
        }
    }

    fn summary(resource: ResourceRef, status: Option<Status>) -> ResourceSummary {
        ResourceSummary {
            resource,
            status,
            detail: String::new(),
            labels: BTreeMap::new(),
            ports: Vec::new(),
            owner: None,
        }
    }

    fn snapshot() -> Snapshot {
        let owner = resource("a", "Kustomization", "flux-system", "apps");
        let mut deploy_a = summary(
            resource("a", "Deployment", "shop", "cart"),
            Some(Status::Err),
        );
        deploy_a.owner = Some(owner.clone());
        deploy_a.labels.insert("app".into(), "cart".into());
        let mut svc = summary(resource("a", "Service", "shop", "cart"), None);
        svc.ports = vec![80];
        Snapshot {
            clusters: vec![
                cluster("a", Status::Err, Counts::default()),
                cluster("b", Status::Ok, Counts::default()),
            ],
            resources: vec![
                summary(
                    resource("b", "Deployment", "shop", "cart"),
                    Some(Status::Ok),
                ),
                svc,
                summary(resource("a", "Pod", "shop", "cart-7f9c"), Some(Status::Err)),
                deploy_a,
                summary(owner, Some(Status::Err)),
            ],
            ..Snapshot::default()
        }
    }

    fn names(results: &Results) -> Vec<(Section, String, String)> {
        results
            .hits
            .iter()
            .map(|hit| {
                (
                    hit.section,
                    hit.kind.clone(),
                    format!("{} {}", hit.cluster, hit.name),
                )
            })
            .collect()
    }

    #[test]
    fn parses_scopes_out_of_the_query() {
        let query = Query::parse("@Prod ns:pay cart k:deploy l:app=cart");
        assert_eq!(query.text, "cart");
        assert_eq!(query.cluster.as_deref(), Some("prod"));
        assert_eq!(query.namespace.as_deref(), Some("pay"));
        assert_eq!(query.kind.as_deref(), Some("deploy"));
        assert_eq!(query.label, Some(("app".into(), Some("cart".into()))));
        assert!(!query.commands_only);
        let commands = Query::parse(">toggle res");
        assert!(commands.commands_only);
        assert_eq!(commands.text, "toggle res");
    }

    #[test]
    fn resources_rank_problems_then_owners_then_cluster_order() {
        let results = search(&snapshot(), None, "cart");
        let resources: Vec<_> = names(&results)
            .into_iter()
            .filter(|(section, ..)| *section == Section::Resources)
            .map(|(_, kind, name)| format!("{kind} {name}"))
            .collect();
        assert_eq!(
            resources,
            [
                "deploy a cart",
                "pod a cart-7f9c",
                "deploy b cart",
                "svc a cart"
            ]
        );
    }

    #[test]
    fn workload_and_service_hits_suggest_commands() {
        let results = search(&snapshot(), None, "cart");
        let commands: Vec<_> = results
            .hits
            .iter()
            .filter(|hit| hit.section == Section::Commands)
            .map(|hit| (hit.name.as_str(), hit.command.clone()))
            .collect();
        let deploy = resource("a", "Deployment", "shop", "cart");
        assert_eq!(
            commands,
            [
                (
                    "Logs: follow all cart pods on a",
                    Command::FollowLogs(LogTarget::Workload(deploy.clone()))
                ),
                (
                    "Compare deploy/cart across clusters",
                    Command::Compare(deploy)
                ),
                (
                    "Show Flux owner of deploy/cart",
                    Command::OpenResource(resource("a", "Kustomization", "flux-system", "apps"))
                ),
                (
                    "Port-forward svc/cart 8080 → 80",
                    Command::PortForward {
                        service: resource("a", "Service", "shop", "cart"),
                        local: 8080,
                        remote: 80
                    }
                ),
            ]
        );
    }

    fn command_names(input: &str) -> Vec<String> {
        search(&snapshot(), None, input)
            .hits
            .into_iter()
            .map(|hit| hit.name)
            .collect()
    }

    #[test]
    fn command_mode_finds_resource_commands_by_verb() {
        assert_eq!(
            command_names(">compare"),
            ["Compare deploy/cart across clusters"]
        );
        assert_eq!(
            command_names(">cart compare"),
            ["Compare deploy/cart across clusters"]
        );
        assert_eq!(
            command_names(">owner cart"),
            ["Show Flux owner of deploy/cart"]
        );
        assert_eq!(
            command_names(">"),
            ["Toggle sidebar", "Open settings", CHECK_FOR_UPDATES]
        );
    }

    #[test]
    fn command_mode_highlights_every_word() {
        let results = search(&snapshot(), None, ">cart compare");
        let highlights = results.hits.first().map(|hit| hit.highlights.clone());
        assert_eq!(highlights, Some(vec![0..7, 15..19]));
    }

    #[test]
    fn resource_commands_follow_the_open_resource() {
        let snapshot = snapshot();
        let deploy = resource("a", "Deployment", "shop", "cart");
        let keys: Vec<_> = resource_commands(&snapshot, &deploy)
            .into_iter()
            .map(|hit| hit.shortcut)
            .collect();
        assert_eq!(keys, [Some(LOGS_KEY), Some(COMPARE_KEY), Some(OWNER_KEY)]);
        let missing = resource("a", "Deployment", "shop", "gone");
        assert_eq!(resource_commands(&snapshot, &missing), Vec::new());
    }

    #[test]
    fn active_forwards_can_be_stopped_from_commands() {
        let mut snapshot = snapshot();
        snapshot.forwards = vec![crate::Forward {
            target: resource("a", "Service", "shop", "cart"),
            local: 8080,
            remote: 80,
        }];
        let results = search(&snapshot, None, ">stop port");
        let stops: Vec<_> = results
            .hits
            .iter()
            .map(|hit| (hit.name.as_str(), hit.command.clone()))
            .collect();
        assert_eq!(
            stops,
            [(
                "Stop port-forward localhost:8080 → svc/cart:80",
                Command::StopPortForward { local: 8080 }
            )]
        );
    }

    #[test]
    fn scopes_filter_resources() {
        let snapshot = snapshot();
        let in_b = search(&snapshot, None, "@b cart");
        assert_eq!(in_b.clusters, 1);
        let scoped = search(&snapshot, None, "@b");
        let sections: Vec<_> = scoped.hits.iter().map(|hit| hit.section).collect();
        assert_eq!(
            sections,
            [
                Section::Clusters,
                Section::Resources,
                Section::Commands,
                Section::Commands
            ]
        );
        assert!(
            in_b.hits
                .iter()
                .all(|hit| hit.section != Section::Resources || hit.cluster == "b")
        );
        let pods = search(&snapshot, None, "k:pod");
        let kinds: Vec<_> = pods.hits.iter().map(|hit| hit.kind.as_str()).collect();
        assert_eq!(kinds, ["pod"]);
        let labelled = search(&snapshot, None, "l:app=cart");
        assert_eq!(labelled.hits.len(), 4);
        let flux = search(&snapshot, None, "ns:flux");
        assert_eq!(flux.hits.first().map(|hit| hit.name.as_str()), Some("apps"));
    }

    #[test]
    fn empty_query_browses_clusters_pages_and_commands() {
        let snapshot = snapshot();
        let results = search(&snapshot, None, "");
        let sections: Vec<_> = results.hits.iter().map(|hit| hit.section).collect();
        assert!(!sections.contains(&Section::Resources));
        assert_eq!(sections.first(), Some(&Section::Clusters));
        let flock_pages = FLOCK_NAV.iter().map(|s| s.pages.len()).sum::<usize>();
        let pages = sections.iter().filter(|s| **s == Section::Pages).count();
        assert_eq!(pages, flock_pages);
        let in_cluster = search(&snapshot, Some(&ClusterId::new("a")), "pods");
        let pods = in_cluster.hits.first().map(|hit| hit.command.clone());
        assert_eq!(pods, Some(Command::SelectPage(Page::Pods)));
    }

    #[cfg(feature = "loco")]
    #[test]
    fn loco_pages_are_listed_only_for_clusters_running_loco() {
        let mut snapshot = snapshot();
        let a = ClusterId::new("a");
        let page = |snapshot: &Snapshot| {
            search(snapshot, Some(&a), "routes")
                .hits
                .iter()
                .any(|hit| hit.command == Command::SelectPage(Page::LocoRoutes))
        };
        assert!(!page(&snapshot));
        snapshot
            .loco
            .insert(a.clone(), crate::loco::LocoCluster::default());
        assert!(page(&snapshot));
    }

    #[test]
    fn command_scope_only_lists_commands() {
        let results = search(&snapshot(), None, ">side");
        let names: Vec<_> = results.hits.iter().map(|hit| hit.name.as_str()).collect();
        assert_eq!(names, ["Toggle sidebar"]);
    }

    #[test]
    fn command_mode_offers_the_update_check() {
        let results = search(&snapshot(), None, ">check updates");
        let hit = results.hits.first();
        assert_eq!(hit.map(|hit| hit.name.as_str()), Some(CHECK_FOR_UPDATES));
        assert_eq!(hit.map(|hit| &hit.command), Some(&Command::CheckForUpdates));
    }

    #[test]
    fn summary_counts_matches_and_clusters() {
        let results = search(&snapshot(), None, "cart");
        assert_eq!(results.summary(), "8 matches · 2 clusters");
        let one = Results {
            hits: results.hits.into_iter().take(1).collect(),
            clusters: 1,
        };
        assert_eq!(one.summary(), "1 match · 1 cluster");
    }

    #[test]
    fn privileged_ports_forward_to_a_high_local_port() {
        assert_eq!(local_port(80), 8080);
        assert_eq!(local_port(8443), 8443);
    }
}
