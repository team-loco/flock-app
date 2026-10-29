use std::collections::BTreeMap;
use std::time::Duration;

use crate::filter::FilterSpec;
use crate::flux::FluxObject;
use crate::home::Problem;
use crate::overview::plural;
use crate::pods::Pod;
use crate::time::age;
use crate::{ClusterId, Counts, ResourceRef, Status};

pub const WINDOW: Duration = Duration::from_hours(6);
pub const BUCKETS: usize = 36;
pub const MARKER_LEAD: Duration = Duration::from_mins(30);
pub const MAX_MARKERS: usize = 3;
const TOP_VALUES: usize = 2;
const SEPARATOR: &str = " · ";
const POD: &str = "Pod";
const POD_OWNERS: [&str; 6] = [
    POD,
    "Deployment",
    "StatefulSet",
    "DaemonSet",
    "Job",
    "ReplicaSet",
];
const QUOTE: char = '"';
const NAME: &str = "‹name›";
const POD_SLOT: &str = "‹pod›";
const NAMESPACE_SLOT: &str = "‹namespace›";
const CONTAINER_SLOT: &str = "‹container›";
const TEAM: &str = "‹team›";
const APP: &str = "‹app›";
const TAG: &str = "‹tag›";
const DIGEST: &str = "@sha256:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occurrence {
    pub status: Status,
    pub target: ResourceRef,
    pub workload: Option<String>,
    pub node: Option<String>,
    pub image: Option<String>,
    pub memory_limit: Option<String>,
    pub age: Duration,
}

impl Occurrence {
    pub const FILTER: FilterSpec<Self> = FilterSpec {
        fields: &[
            ("name", |o, (), out| out.push(o.target.name.as_str().into())),
            ("workload", |o, (), out| {
                if let Some(workload) = &o.workload {
                    out.push(workload.as_str().into());
                }
            }),
            ("node", |o, (), out| {
                if let Some(node) = &o.node {
                    out.push(node.as_str().into());
                }
            }),
            ("image", |o, (), out| {
                if let Some(image) = &o.image {
                    out.push(image.as_str().into());
                }
            }),
        ],
        facets: &[
            ("workload", |o, (), out| {
                out.extend(o.workload.as_deref().map(Into::into));
            }),
            ("node", |o, (), out| {
                out.extend(o.node.as_deref().map(Into::into));
            }),
            ("image", |o, (), out| {
                out.extend(o.image.as_deref().map(Into::into));
            }),
        ],
        namespace: true,
        labels: false,
    };

    pub fn is_pod(&self) -> bool {
        self.target.kind == POD
    }

    pub fn object_label(&self) -> String {
        if self.is_pod() {
            return self.target.name.clone();
        }
        format!("{}/{}", self.target.kind, self.target.name)
    }

    pub fn tag(&self) -> Option<&str> {
        let image = self.image.as_deref()?;
        let (_, name) = image.rsplit_once('/').unwrap_or(("", image));
        name.rsplit_once(':').map(|(_, tag)| tag)
    }

    pub fn registry(&self) -> Option<&str> {
        self.image.as_deref().and_then(registry)
    }

    fn owner_kind(&self) -> Option<&str> {
        let workload = self.workload.as_deref()?;
        workload.split_once('/').map(|(kind, _)| kind)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trait {
    pub name: &'static str,
    pub value: String,
    pub shared: usize,
    pub of: usize,
}

impl Trait {
    pub fn share_label(&self) -> String {
        format!("{}/{}", self.shared, self.of)
    }

    #[allow(clippy::cast_precision_loss)]
    pub fn fraction(&self) -> f32 {
        if self.of == 0 {
            return 0.;
        }
        self.shared as f32 / self.of as f32
    }

    pub fn is_universal(&self) -> bool {
        self.shared == self.of
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Marker {
    pub age: Duration,
    pub what: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cause {
    pub status: Status,
    pub reason: String,
    pub template: String,
    pub occurrences: Vec<Occurrence>,
}

impl Cause {
    pub fn key(&self) -> (&str, &str) {
        (&self.reason, &self.template)
    }

    pub fn count(&self) -> usize {
        self.occurrences.len()
    }

    pub fn oldest(&self) -> Duration {
        self.occurrences
            .iter()
            .map(|o| o.age)
            .max()
            .unwrap_or_default()
    }

    pub fn newest(&self) -> Option<&Occurrence> {
        self.occurrences.first()
    }

    pub fn is_pods(&self) -> bool {
        self.occurrences.iter().all(Occurrence::is_pod)
    }

    fn distinct<'a>(&'a self, value: impl Fn(&'a Occurrence) -> Option<&'a str>) -> usize {
        let mut values: Vec<&str> = self.occurrences.iter().filter_map(value).collect();
        values.sort_unstable();
        values.dedup();
        values.len()
    }

    pub fn namespaces(&self) -> usize {
        self.distinct(|o| o.target.namespace.as_deref())
    }

    pub fn scope(&self) -> String {
        let objects = if self.is_pods() {
            plural(self.count(), "pod", "pods")
        } else {
            plural(self.count(), "object", "objects")
        };
        let workloads = self.distinct(|o| o.workload.as_deref());
        let nodes = self.distinct(|o| o.node.as_deref());
        let parts = [
            Some(objects),
            (workloads > 0).then(|| plural(workloads, "workload", "workloads")),
            (self.namespaces() > 0).then(|| plural(self.namespaces(), "namespace", "namespaces")),
            (nodes > 0).then(|| plural(nodes, "node", "nodes")),
        ];
        parts
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(SEPARATOR)
    }

    pub fn meta(&self) -> String {
        let since = age(self.oldest());
        match self.namespaces() {
            0 => since,
            n => format!("{n} ns{SEPARATOR}{since}"),
        }
    }

    pub fn started(&self) -> String {
        let first = format!("first seen {} ago", age(self.oldest()));
        match self.newest() {
            Some(newest) => format!("{first}{SEPARATOR}last seen {} ago", age(newest.age)),
            None => first,
        }
    }

    pub fn histogram(&self) -> [u32; BUCKETS] {
        let mut bars = [0; BUCKETS];
        let width = WINDOW / u32::try_from(BUCKETS).unwrap_or(u32::MAX);
        for occurrence in self.occurrences.iter().filter(|o| o.age < WINDOW) {
            let back = usize::try_from(occurrence.age.as_secs() / width.as_secs().max(1))
                .unwrap_or(usize::MAX);
            let index = BUCKETS.saturating_sub(back.saturating_add(1));
            if let Some(bar) = bars.get_mut(index) {
                *bar += 1;
            }
        }
        bars
    }

    pub fn traits(&self) -> Vec<Trait> {
        let traits = [
            self.shared("registry", spread, |o| o.registry().map(str::to_owned)),
            self.shared("namespace", spread, |o| o.target.namespace.clone()),
            self.shared("node", distinct, |o| o.node.clone()),
            self.shared("owner", spread, |o| o.owner_kind().map(str::to_owned)),
            self.shared("limits.memory", spread, |o| o.memory_limit.clone()),
            self.shared("image", spread, |o| o.image.as_deref().map(image_name)),
        ];
        let mut traits: Vec<Trait> = traits.into_iter().flatten().collect();
        traits.sort_by_key(|t| std::cmp::Reverse(t.shared));
        traits
    }

    fn shared(
        &self,
        name: &'static str,
        summary: fn(&[(String, usize)]) -> String,
        value: impl Fn(&Occurrence) -> Option<String>,
    ) -> Option<Trait> {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for found in self.occurrences.iter().filter_map(value) {
            *counts.entry(found).or_default() += 1;
        }
        let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let (_, top) = ranked.first()?;
        let shared = *top;
        let value = match ranked.as_slice() {
            [(only, _)] => only.clone(),
            many => summary(many),
        };
        Some(Trait {
            name,
            value,
            shared,
            of: self.count(),
        })
    }

    pub fn markers(&self, flux: &[FluxObject], cluster: &ClusterId) -> Vec<Marker> {
        let start = self.oldest();
        let earliest = start.saturating_add(MARKER_LEAD).min(WINDOW);
        let mut markers: Vec<Marker> = flux
            .iter()
            .filter(|object| &object.key.cluster == cluster)
            .filter_map(|object| {
                let applied = object.applied.as_ref()?;
                let near = applied.age >= start && applied.age <= earliest;
                near.then(|| Marker {
                    age: applied.age,
                    what: format!(
                        "{}/{} applied {}",
                        object.key.kind.label(),
                        object.key.name,
                        applied.id
                    ),
                })
            })
            .collect();
        markers.sort_by_key(|m| std::cmp::Reverse(m.age));
        markers.truncate(MAX_MARKERS);
        markers
    }
}

fn spread(ranked: &[(String, usize)]) -> String {
    let shown = ranked
        .iter()
        .take(TOP_VALUES)
        .map(|(value, n)| format!("{value} {n}"));
    let rest = ranked.len().saturating_sub(TOP_VALUES);
    let more = (rest > 0).then(|| format!("+{rest}"));
    shown.chain(more).collect::<Vec<_>>().join(SEPARATOR)
}

fn distinct(ranked: &[(String, usize)]) -> String {
    format!("{} distinct", ranked.len())
}

fn image_name(image: &str) -> String {
    let path = registry(image).map_or(image, |host| image.get(host.len() + 1..).unwrap_or(image));
    path.rsplit_once('/')
        .map_or(path, |(_, name)| name)
        .to_owned()
}

fn registry(image: &str) -> Option<&str> {
    let (host, _) = image.split_once('/')?;
    let is_host = host.contains('.') || host.contains(':') || host == "localhost";
    is_host.then_some(host)
}

pub fn counts(causes: &[Cause]) -> Counts {
    causes
        .iter()
        .flat_map(|cause| &cause.occurrences)
        .fold(Counts::default(), |counts, o| match o.status {
            Status::Err => counts.with_err(counts.err + 1),
            Status::Warn => counts.with_warn(counts.warn + 1),
            Status::Suspended => counts.with_sus(counts.sus + 1),
            Status::Ok => counts,
            Status::Progress => counts,
            Status::Offline => counts,
        })
}

pub fn causes(cluster: &ClusterId, pods: &[Pod], problems: &[Problem]) -> Vec<Cause> {
    let pods: Vec<&Pod> = pods.iter().filter(|pod| &pod.cluster == cluster).collect();
    let from_pods = !pods.is_empty();
    let mut grouped: Vec<Cause> = Vec::new();
    for pod in pods {
        if let Some((reason, template, occurrence)) = pod_occurrence(pod) {
            add(&mut grouped, reason, template, occurrence);
        }
    }
    let others = problems
        .iter()
        .filter(|p| &p.target.cluster == cluster)
        .filter(|p| !(from_pods && POD_OWNERS.contains(&p.target.kind.as_str())));
    for problem in others {
        let (reason, template, occurrence) = problem_occurrence(problem);
        add(&mut grouped, reason, template, occurrence);
    }
    for cause in &mut grouped {
        cause.occurrences.sort_by_key(|o| o.age);
    }
    grouped.sort_by(|a, b| {
        b.status
            .cmp(&a.status)
            .then_with(|| b.count().cmp(&a.count()))
            .then_with(|| a.reason.cmp(&b.reason))
    });
    grouped
}

fn add(causes: &mut Vec<Cause>, reason: String, template: String, occurrence: Occurrence) {
    let found = causes
        .iter_mut()
        .find(|cause| cause.key() == (reason.as_str(), template.as_str()));
    match found {
        Some(cause) => {
            cause.status = cause.status.max(occurrence.status);
            cause.occurrences.push(occurrence);
        }
        None => causes.push(Cause {
            status: occurrence.status,
            reason,
            template,
            occurrences: vec![occurrence],
        }),
    }
}

fn pod_occurrence(pod: &Pod) -> Option<(String, String, Occurrence)> {
    let diagnosis = pod.diagnosis()?;
    let failing = pod
        .containers
        .iter()
        .filter(|c| c.status != Status::Ok)
        .max_by_key(|c| c.status)
        .or_else(|| pod.containers.first());
    let mut names = vec![
        (pod.name.as_str(), POD_SLOT),
        (pod.namespace.as_str(), NAMESPACE_SLOT),
    ];
    if let Some(container) = failing {
        names.push((container.name.as_str(), CONTAINER_SLOT));
    }
    let template = template(&diagnosis.message, &names);
    let unhealthy = pod
        .conditions
        .iter()
        .filter(|c| !c.is_healthy())
        .filter_map(|c| c.age)
        .max();
    let occurrence = Occurrence {
        status: pod.status,
        target: pod.resource(),
        workload: pod.owner_label(),
        node: pod.node_short().map(str::to_owned),
        image: failing.map(|c| c.image.clone()).filter(|i| !i.is_empty()),
        memory_limit: failing.and_then(|c| c.limits.memory.clone()),
        age: unhealthy.unwrap_or(pod.age),
    };
    Some((diagnosis.reason, template, occurrence))
}

fn problem_occurrence(problem: &Problem) -> (String, String, Occurrence) {
    let reason = if problem.reason.is_empty() {
        problem.target.kind.as_str()
    } else {
        problem.reason.as_str()
    };
    let message = problem.message.trim();
    let mut names = vec![(problem.target.name.as_str(), NAME)];
    if let Some(namespace) = problem.target.namespace.as_deref() {
        names.push((namespace, NAMESPACE_SLOT));
    }
    let node = (problem.target.kind == "Node").then(|| problem.target.name.clone());
    let occurrence = Occurrence {
        status: problem.status,
        target: problem.target.clone(),
        workload: None,
        node,
        image: None,
        memory_limit: None,
        age: problem.age,
    };
    (reason.to_owned(), template(message, &names), occurrence)
}

pub fn template(message: &str, names: &[(&str, &str)]) -> String {
    let quoted = quoted_slots(message);
    let mut names: Vec<&(&str, &str)> = names.iter().filter(|(name, _)| !name.is_empty()).collect();
    names.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
    names
        .into_iter()
        .fold(quoted, |text, (name, slot)| replace_word(&text, name, slot))
}

fn quoted_slots(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut parts = message.split(QUOTE);
    if let Some(first) = parts.next() {
        out.push_str(first);
    }
    let mut inside = true;
    let mut rest: Vec<&str> = parts.collect();
    let closed = rest.len().is_multiple_of(2);
    let last = if closed { None } else { rest.pop() };
    for part in rest {
        out.push(QUOTE);
        if inside {
            out.push_str(&quoted_slot(part));
        } else {
            out.push_str(part);
        }
        inside = !inside;
    }
    if let Some(last) = last {
        out.push(QUOTE);
        out.push_str(last);
    }
    out
}

fn quoted_slot(value: &str) -> String {
    if value.contains(char::is_whitespace) || value.is_empty() {
        return value.to_owned();
    }
    if value.contains('/') || value.contains(':') {
        return image_slot(value);
    }
    NAME.to_owned()
}

fn image_slot(image: &str) -> String {
    let host = registry(image);
    let path = host.map_or(image, |host| image.get(host.len() + 1..).unwrap_or(image));
    let untagged = match path.split_once(DIGEST) {
        Some((name, _)) => name,
        None => path.rsplit_once(':').map_or(path, |(name, _)| name),
    };
    let tagged = untagged.len() < path.len();
    let segments = untagged.split('/').count();
    let mut slots: Vec<&str> = std::iter::repeat_n(TEAM, segments.saturating_sub(1)).collect();
    slots.push(APP);
    let mut out = host.map(|h| format!("{h}/")).unwrap_or_default();
    out.push_str(&slots.join("/"));
    if tagged {
        out.push(':');
        out.push_str(TAG);
    }
    out
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | '.')
}

fn replace_word(text: &str, word: &str, slot: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(word) {
        let before = rest.get(..at).unwrap_or_default();
        let after = rest.get(at + word.len()..).unwrap_or_default();
        let starts = before.chars().next_back().is_none_or(|c| !is_word(c));
        let ends = after.chars().next().is_none_or(|c| {
            !is_word(c) || (c == '.' && after.chars().nth(1).is_none_or(|n| !is_word(n)))
        });
        out.push_str(before);
        if starts && ends {
            out.push_str(slot);
        } else {
            out.push_str(word);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnosis::Condition;
    use crate::flux::{FluxKey, FluxKind, Revision};
    use crate::pods::{Container, ContainerState, Resources};

    const MINUTE: u64 = 60;

    fn cluster() -> ClusterId {
        ClusterId::new("prod")
    }

    fn container(reason: &str, message: &str, image: &str) -> Container {
        Container {
            name: "api".into(),
            image: image.into(),
            state: ContainerState::Waiting,
            status: Status::Err,
            reason: reason.into(),
            message: message.into(),
            limits: Resources {
                cpu: None,
                memory: Some("512Mi".into()),
            },
            ..Container::default()
        }
    }

    fn pod(namespace: &str, name: &str, container: Container, minutes: u64) -> Pod {
        Pod {
            cluster: cluster(),
            namespace: namespace.into(),
            name: name.into(),
            status: Status::Err,
            reason: container.reason.clone(),
            node: Some(format!("node-{namespace}.internal")),
            age: Duration::from_secs(minutes * MINUTE),
            containers: vec![container],
            ..Pod::default()
        }
    }

    fn pull(namespace: &str, app: &str, minutes: u64) -> Pod {
        let image = format!("registry.acme.io/{namespace}/{app}:1.{minutes}");
        let message = format!("Back-off pulling image \"{image}\"");
        let failing = container("ImagePullBackOff", &message, &image);
        pod(namespace, &format!("{app}-x{minutes}"), failing, minutes)
    }

    #[test]
    fn templates_replace_images_quoted_names_and_the_objects_own_names() {
        assert_eq!(
            template(
                "Failed to pull image \"registry.acme.io/payments/checkout:2.1\": 401 Unauthorized",
                &[]
            ),
            "Failed to pull image \"registry.acme.io/‹team›/‹app›:‹tag›\": 401 Unauthorized"
        );
        assert_eq!(
            template("Back-off pulling image \"nginx:bad\"", &[]),
            "Back-off pulling image \"‹app›:‹tag›\""
        );
        assert_eq!(
            template("secret \"stripe\" not found", &[]),
            "secret \"‹name›\" not found"
        );
        assert_eq!(
            template(
                "couldn't find key token in Secret payments/stripe for web",
                &[("payments", NAMESPACE_SLOT), ("web", POD_SLOT)]
            ),
            "couldn't find key token in Secret ‹namespace›/stripe for ‹pod›"
        );
        assert_eq!(
            template("webhook web-1 web.", &[("web", POD_SLOT)]),
            "webhook web-1 ‹pod›."
        );
        assert_eq!(template("unclosed \"quote", &[]), "unclosed \"quote");
    }

    #[test]
    fn pods_with_the_same_reason_and_template_share_one_cause() {
        let crash = container(
            "CrashLoopBackOff",
            "last OOMKilled · exit 137",
            "ghcr.io/acme/indexer:0.19",
        );
        let pods = vec![
            pull("payments", "checkout", 2),
            pull("web", "frontend", 5),
            pod("search", "indexer-a", crash.clone(), 1),
            pod("search", "indexer-b", crash, 3),
            Pod {
                cluster: ClusterId::new("other"),
                ..pull("web", "frontend", 9)
            },
            Pod {
                status: Status::Ok,
                ..Pod::default()
            },
        ];
        let causes = causes(&cluster(), &pods, &[]);
        let summary: Vec<(&str, &str, usize)> = causes
            .iter()
            .map(|c| (c.reason.as_str(), c.template.as_str(), c.count()))
            .collect();
        assert_eq!(
            summary,
            [
                ("CrashLoopBackOff", "last OOMKilled · exit 137", 2),
                (
                    "ImagePullBackOff",
                    "Back-off pulling image \"registry.acme.io/‹team›/‹app›:‹tag›\"",
                    2
                ),
            ]
        );
        let names: Vec<&str> = causes[1]
            .occurrences
            .iter()
            .map(|o| o.target.name.as_str())
            .collect();
        assert_eq!(names, ["checkout-x2", "frontend-x5"]);
        assert_eq!(causes[1].scope(), "2 pods · 2 namespaces · 2 nodes");
        assert_eq!(causes[1].meta(), "2 ns · 5m");
        assert_eq!(causes[1].occurrences[0].tag(), Some("1.2"));
    }

    #[test]
    fn problems_pods_already_explain_are_not_counted_twice() {
        let problem = |kind: &str, name: &str, reason: &str, message: &str| Problem {
            status: Status::Warn,
            target: ResourceRef {
                cluster: cluster(),
                kind: kind.into(),
                namespace: None,
                name: name.into(),
            },
            reason: reason.into(),
            message: message.into(),
            condition: String::new(),
            age: Duration::from_secs(MINUTE),
            events: Vec::new(),
        };
        let problems = vec![
            problem(
                "Deployment",
                "checkout",
                "ImagePullBackOff",
                "Back-off pulling image",
            ),
            problem(
                "Node",
                "ip-10-4-31-12",
                "NodeStatusUnknown",
                "Kubelet stopped posting node status.",
            ),
            problem(
                "Node",
                "ip-10-4-31-13",
                "NodeStatusUnknown",
                "Kubelet stopped posting node status.",
            ),
            problem("Node", "ip-10-4-31-14", "", "spec.unschedulable: true"),
            problem(
                "Kustomization",
                "apps",
                "HealthCheckFailed",
                "Health check failed for apps",
            ),
        ];
        let pods = vec![pull("payments", "checkout", 2)];
        let causes = causes(&cluster(), &pods, &problems);
        let summary: Vec<(&str, &str, usize)> = causes
            .iter()
            .map(|c| (c.reason.as_str(), c.template.as_str(), c.count()))
            .collect();
        assert_eq!(
            summary,
            [
                (
                    "ImagePullBackOff",
                    "Back-off pulling image \"registry.acme.io/‹team›/‹app›:‹tag›\"",
                    1
                ),
                (
                    "NodeStatusUnknown",
                    "Kubelet stopped posting node status.",
                    2
                ),
                ("HealthCheckFailed", "Health check failed for ‹name›", 1),
                ("Node", "spec.unschedulable: true", 1),
            ]
        );
        assert_eq!(causes[1].scope(), "2 objects · 2 nodes");
        assert_eq!(
            causes[1].occurrences[0].object_label(),
            "Node/ip-10-4-31-12"
        );
        let without_pods = super::causes(&cluster(), &[], &problems);
        assert_eq!(without_pods.len(), 4);
    }

    #[test]
    fn unhealthy_conditions_date_an_occurrence_before_the_pod_age() {
        let mut stuck = pull("web", "frontend", 90);
        stuck.conditions = vec![
            Condition::new("PodScheduled", "True").age(Some(Duration::from_secs(90 * MINUTE))),
            Condition::new("Ready", "False").age(Some(Duration::from_secs(20 * MINUTE))),
        ];
        let causes = causes(&cluster(), &[stuck], &[]);
        assert_eq!(causes[0].oldest(), Duration::from_secs(20 * MINUTE));
    }

    #[test]
    fn histogram_buckets_six_hours_newest_last() {
        let pods = vec![
            pull("web", "a", 0),
            pull("web", "b", 9),
            pull("web", "c", 10),
            pull("web", "d", 359),
            pull("web", "e", 400),
        ];
        let cause = causes(&cluster(), &pods, &[]).remove(0);
        let bars = cause.histogram();
        assert_eq!(bars[BUCKETS - 1], 2);
        assert_eq!(bars[BUCKETS - 2], 1);
        assert_eq!(bars[0], 1);
        assert_eq!(bars.iter().sum::<u32>(), 4);
        assert_eq!(cause.started(), "first seen 6h ago · last seen 0s ago");
    }

    #[test]
    fn traits_rank_what_most_occurrences_share() {
        let pods = vec![
            pull("payments", "checkout", 1),
            pull("payments", "cart", 2),
            pull("web", "frontend", 3),
        ];
        let cause = causes(&cluster(), &pods, &[]).remove(0);
        let traits: Vec<(&str, String, String)> = cause
            .traits()
            .into_iter()
            .map(|t| (t.name, t.value.clone(), t.share_label()))
            .collect();
        assert_eq!(
            traits,
            [
                ("registry", "registry.acme.io".to_owned(), "3/3".to_owned()),
                ("limits.memory", "512Mi".to_owned(), "3/3".to_owned()),
                (
                    "namespace",
                    "payments 2 · web 1".to_owned(),
                    "2/3".to_owned()
                ),
                ("node", "2 distinct".to_owned(), "2/3".to_owned()),
                (
                    "image",
                    "cart:1.2 1 · checkout:1.1 1 · +1".to_owned(),
                    "1/3".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn markers_are_flux_applies_shortly_before_a_cause_started() {
        let pods = vec![pull("web", "a", 60), pull("web", "b", 10)];
        let cause = causes(&cluster(), &pods, &[]).remove(0);
        let applied = |name: &str, minutes: u64, cluster: &str| {
            let mut object = crate::flux::tests::object(cluster, crate::flux::Phase::Ready, "r1");
            object.key = FluxKey::new(cluster, FluxKind::Kustomization, "flux-system", name);
            object.applied = Some(Revision::new(
                "main@a91c3f2",
                Duration::from_secs(minutes * MINUTE),
            ));
            object
        };
        let flux = vec![
            applied("registry", 70, "prod"),
            applied("too-early", 120, "prod"),
            applied("after", 30, "prod"),
            applied("elsewhere", 70, "other"),
        ];
        let markers = cause.markers(&flux, &cluster());
        assert_eq!(
            markers,
            [Marker {
                age: Duration::from_secs(70 * MINUTE),
                what: "Kustomization/registry applied main@a91c3f2".into(),
            }]
        );
    }
}
