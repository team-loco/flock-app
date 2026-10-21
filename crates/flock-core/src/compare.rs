use crate::changes::{LineChange, PatchLine};
use crate::tabs::short_name;
use crate::workload::{ContainerSpec, EnvResolution, EnvSource, EnvVar, Resources, short_image};
use crate::{ClusterId, ResourceRef, Seen, Status, Uid, Workload, units};

const MISSING: &str = "—";
const READY: &str = "Ready";
const IMAGE: &str = "Image";
const REPLICAS: &str = "Replicas · HPA";
const CPU: &str = "CPU request / limit";
const MEMORY: &str = "Memory request / limit";
const STRATEGY: &str = "Strategy";
const LAST_ROLLOUT: &str = "Last rollout";
const HEALTHY: &str = "Healthy";
const DEGRADED: &str = "Degraded";
const STALLED: &str = "Stalled";
const ROLLING_OUT: &str = "Rolling out";
const UNKNOWN: &str = "Unknown";
const NOT_FOUND: &str = "not found";
const OPTIONAL: &str = "optional";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observed {
    pub cluster: ClusterId,
    pub workload: Workload,
    pub replicas: String,
    pub strategy: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comparison {
    pub kind: String,
    pub namespace: Option<String>,
    pub name: String,
    pub clusters: Vec<ComparedCluster>,
    pub fields: Vec<Field>,
    pub spec: Option<SpecDiff>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComparedCluster {
    pub cluster: ClusterId,
    pub uid: Uid,
    pub status: Status,
    pub health: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub label: String,
    pub cells: Vec<Cell>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub text: String,
    pub flag: Option<Status>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecDiff {
    pub path: String,
    pub base: ClusterId,
    pub other: ClusterId,
    pub lines: Vec<PatchLine>,
}

impl Cell {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            flag: None,
        }
    }

    pub fn flagged(text: impl Into<String>, flag: Status) -> Self {
        Self {
            text: text.into(),
            flag: Some(flag),
        }
    }
}

impl Field {
    pub fn differs(&self) -> bool {
        let first = self.cells.first().map(|c| c.text.as_str());
        self.cells.iter().any(|c| Some(c.text.as_str()) != first)
    }
}

impl SpecDiff {
    pub fn has_changes(&self) -> bool {
        self.lines.iter().any(|l| l.change != LineChange::Context)
    }

    pub fn apply_label(&self) -> String {
        let (base, other) = short_names(self.base.as_str(), self.other.as_str());
        format!("Apply {other} value to {base}…")
    }
}

impl Comparison {
    pub fn matches(&self, resource: &ResourceRef) -> bool {
        self.kind == resource.kind
            && self.namespace == resource.namespace
            && self.name == resource.name
    }

    pub fn seen_on(&self, cluster: &ClusterId) -> Seen {
        let uid = self
            .clusters
            .iter()
            .find(|compared| &compared.cluster == cluster)
            .map(|compared| compared.uid.clone())
            .unwrap_or_default();
        Seen::new(self.on(cluster), uid)
    }

    pub fn on(&self, cluster: &ClusterId) -> ResourceRef {
        ResourceRef {
            cluster: cluster.clone(),
            kind: self.kind.clone(),
            namespace: self.namespace.clone(),
            name: self.name.clone(),
        }
    }

    pub fn visible(&self, differences_only: bool) -> Vec<&Field> {
        self.fields
            .iter()
            .filter(|f| !differences_only || f.differs())
            .collect()
    }

    pub fn differing(&self) -> usize {
        self.fields.iter().filter(|f| f.differs()).count()
    }

    pub fn hidden_note(&self, differences_only: bool) -> String {
        let hidden = self.fields.len() - self.visible(differences_only).len();
        match hidden {
            0 => "Showing all fields".into(),
            1 => "1 identical field hidden".into(),
            n => format!("{n} identical fields hidden"),
        }
    }
}

pub fn build(target: &ResourceRef, observed: &[Observed], spec: Option<SpecDiff>) -> Comparison {
    let mut ordered: Vec<&Observed> = observed.iter().collect();
    ordered.sort_by_key(|o| o.cluster != target.cluster);
    Comparison {
        kind: target.kind.clone(),
        namespace: target.namespace.clone(),
        name: target.name.clone(),
        clusters: ordered
            .iter()
            .map(|o| compared_cluster(&o.workload, &o.cluster))
            .collect(),
        fields: fields(&ordered),
        spec,
    }
}

fn compared_cluster(workload: &Workload, cluster: &ClusterId) -> ComparedCluster {
    let status = workload.status();
    let word = match status {
        Status::Ok => HEALTHY,
        Status::Err => DEGRADED,
        Status::Warn => STALLED,
        Status::Progress => ROLLING_OUT,
        Status::Suspended => UNKNOWN,
        Status::Offline => UNKNOWN,
    };
    let health = match status {
        Status::Ok => word.to_owned(),
        _ => format!("{word} · {}", workload.ready_label()),
    };
    ComparedCluster {
        cluster: cluster.clone(),
        uid: workload.uid.clone(),
        status,
        health,
    }
}

fn first_container(workload: &Workload) -> Option<&ContainerSpec> {
    workload.containers.first()
}

fn container_text(
    resources: impl Fn(&ContainerSpec) -> Option<String>,
    workload: &Workload,
) -> String {
    first_container(workload)
        .and_then(resources)
        .unwrap_or_else(|| MISSING.to_owned())
}

fn pair(
    request: &Resources,
    limit: &Resources,
    pick: fn(&Resources) -> Option<&String>,
    read: fn(&str) -> String,
) -> String {
    let side =
        |resources: &Resources| pick(resources).map_or_else(|| MISSING.to_owned(), |v| read(v));
    format!("{} / {}", side(request), side(limit))
}

fn env_cell(var: Option<&EnvVar>) -> Cell {
    let Some(var) = var else {
        return Cell::new(MISSING);
    };
    let source = match &var.source {
        EnvSource::SecretKey { name, key } => format!("secret {name} · key {key}"),
        EnvSource::ConfigMapKey { name, key } => format!("configmap {name} · key {key}"),
        other => other.label(),
    };
    let text = match &var.resolution {
        EnvResolution::MissingKey { optional, .. } => missing(&source, *optional),
        EnvResolution::MissingObject { optional } => missing(&source, *optional),
        EnvResolution::Literal => source,
        EnvResolution::Resolved => source,
    };
    match var.status() {
        Some(Status::Err) => Cell::flagged(text, Status::Err),
        _ => Cell::new(text),
    }
}

fn missing(source: &str, optional: bool) -> String {
    if optional {
        format!("{source} ({NOT_FOUND}, {OPTIONAL})")
    } else {
        format!("{source} ({NOT_FOUND})")
    }
}

fn referenced(var: &EnvVar) -> bool {
    matches!(
        var.source,
        EnvSource::SecretKey { .. } | EnvSource::ConfigMapKey { .. }
    )
}

fn env_names(observed: &[&Observed]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let vars = observed
        .iter()
        .flat_map(|o| o.workload.containers.iter())
        .flat_map(|c| c.env.iter())
        .filter(|var| referenced(var));
    for var in vars {
        if !names.contains(&var.name) {
            names.push(var.name.clone());
        }
    }
    names
}

fn env_var<'a>(workload: &'a Workload, name: &str) -> Option<&'a EnvVar> {
    workload
        .containers
        .iter()
        .flat_map(|c| c.env.iter())
        .find(|var| var.name == name)
}

fn against_base(label: &str, texts: Vec<String>) -> Field {
    let base = texts.first().cloned();
    let cells = texts
        .into_iter()
        .map(|text| {
            if Some(&text) == base.as_ref() {
                Cell::new(text)
            } else {
                Cell::flagged(text, Status::Warn)
            }
        })
        .collect();
    Field {
        label: label.to_owned(),
        cells,
    }
}

fn ready_cell(workload: &Workload) -> Cell {
    let text = format!("{} / {}", workload.ready(), workload.desired);
    match workload.status() {
        Status::Err => Cell::flagged(text, Status::Err),
        Status::Warn => Cell::flagged(text, Status::Warn),
        _ => Cell::new(text),
    }
}

fn last_rollout(workload: &Workload) -> String {
    workload.current_revision().map_or_else(
        || MISSING.to_owned(),
        |revision| {
            format!(
                "{} ago · rev {}",
                crate::time::age(revision.age),
                revision.number
            )
        },
    )
}

fn fields(observed: &[&Observed]) -> Vec<Field> {
    let each = |f: &dyn Fn(&Observed) -> String| observed.iter().map(|o| f(o)).collect::<Vec<_>>();
    let mut fields = vec![Field {
        label: READY.to_owned(),
        cells: observed.iter().map(|o| ready_cell(&o.workload)).collect(),
    }];
    fields.push(against_base(
        IMAGE,
        each(&|o| container_text(|c| Some(short_image(&c.image).to_owned()), &o.workload)),
    ));
    for name in env_names(observed) {
        let cells: Vec<Cell> = observed
            .iter()
            .map(|o| env_cell(env_var(&o.workload, &name)))
            .collect();
        let base = cells.first().map(|c| c.text.clone());
        let cells = cells
            .into_iter()
            .map(|cell| match cell.flag {
                None if Some(&cell.text) != base.as_ref() => Cell::flagged(cell.text, Status::Warn),
                _ => cell,
            })
            .collect();
        fields.push(Field { label: name, cells });
    }
    fields.push(against_base(REPLICAS, each(&|o| o.replicas.clone())));
    fields.push(against_base(
        CPU,
        each(&|o| {
            container_text(
                |c| {
                    Some(pair(
                        &c.requests,
                        &c.limits,
                        |r| r.cpu.as_ref(),
                        units::written_cpu,
                    ))
                },
                &o.workload,
            )
        }),
    ));
    fields.push(against_base(
        MEMORY,
        each(&|o| {
            container_text(
                |c| {
                    Some(pair(
                        &c.requests,
                        &c.limits,
                        |r| r.memory.as_ref(),
                        str::to_owned,
                    ))
                },
                &o.workload,
            )
        }),
    ));
    fields.push(against_base(STRATEGY, each(&|o| o.strategy.clone())));
    fields.push(Field {
        label: LAST_ROLLOUT.to_owned(),
        cells: observed
            .iter()
            .map(|o| Cell::new(last_rollout(&o.workload)))
            .collect(),
    });
    fields
}

pub fn short_names(a: &str, b: &str) -> (String, String) {
    let names = [a, b];
    (short_name(a, &names), short_name(b, &names))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(label: &str, values: &[&str]) -> Field {
        Field {
            label: label.into(),
            cells: values.iter().map(|v| Cell::new(*v)).collect(),
        }
    }

    fn compared(name: &str, status: Status, health: &str) -> ComparedCluster {
        ComparedCluster {
            cluster: ClusterId::new(name),
            uid: Uid::default(),
            status,
            health: health.into(),
        }
    }

    fn sample() -> Comparison {
        Comparison {
            kind: "Deployment".into(),
            namespace: Some("payments".into()),
            name: "checkout-api".into(),
            clusters: vec![
                compared("a", Status::Err, "Degraded · 1 pod crashlooping"),
                compared("b", Status::Ok, "Healthy"),
            ],
            fields: vec![
                field("Ready", &["2 / 3", "3 / 3"]),
                field("Memory", &["512Mi", "512Mi"]),
                field("Strategy", &["RollingUpdate", "RollingUpdate"]),
            ],
            spec: None,
        }
    }

    #[test]
    fn a_field_differs_when_any_cell_does() {
        assert!(field("x", &["1", "1", "2"]).differs());
        assert!(!field("x", &["1", "1"]).differs());
        assert!(!field("x", &[]).differs());
    }

    #[test]
    fn differences_only_hides_identical_fields() {
        let comparison = sample();
        let labels: Vec<&str> = comparison
            .visible(true)
            .iter()
            .map(|f| f.label.as_str())
            .collect();
        assert_eq!(labels, ["Ready"]);
        assert_eq!(comparison.visible(false).len(), 3);
        assert_eq!(comparison.hidden_note(true), "2 identical fields hidden");
        assert_eq!(comparison.hidden_note(false), "Showing all fields");
    }

    #[test]
    fn comparisons_match_resources_on_any_cluster() {
        let comparison = sample();
        let elsewhere = comparison.on(&ClusterId::new("staging"));
        assert!(comparison.matches(&elsewhere));
        let other = ResourceRef {
            name: "cart".into(),
            ..elsewhere
        };
        assert!(!comparison.matches(&other));
    }

    #[test]
    fn short_names_drop_the_shared_prefix() {
        let pair = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        assert_eq!(
            short_names("prod-us-east", "prod-eu-west"),
            pair("us-east", "eu-west")
        );
        assert_eq!(
            short_names("staging", "prod-us-east"),
            pair("staging", "prod-us-east")
        );
        assert_eq!(
            short_names("fleet-a", "fleet-b"),
            pair("fleet-a", "fleet-b")
        );
        assert_eq!(
            short_names("prod-us-east", "prod-us-east"),
            pair("prod-us-east", "prod-us-east")
        );
    }

    fn spec(image: &str, key: &str, resolution: EnvResolution) -> ContainerSpec {
        ContainerSpec {
            name: "api".into(),
            image: format!("ghcr.io/acme/checkout-api:{image}"),
            ports: Vec::new(),
            requests: Resources {
                cpu: Some("500m".into()),
                memory: None,
            },
            limits: Resources {
                cpu: Some("1".into()),
                memory: None,
            },
            readiness: None,
            env: vec![EnvVar {
                name: "STRIPE_WEBHOOK_SECRET".into(),
                source: EnvSource::SecretKey {
                    name: "stripe".into(),
                    key: key.into(),
                },
                resolution,
            }],
        }
    }

    fn observed(cluster: &str, healthy: bool, container: ContainerSpec) -> Observed {
        use crate::workload::tests::{crashing, running, workload};
        let pods = if healthy {
            vec![running("a"), running("b"), running("c")]
        } else {
            vec![crashing("a"), running("b"), running("c")]
        };
        let mut workload = workload(pods, false);
        workload.containers = vec![container];
        Observed {
            cluster: ClusterId::new(cluster),
            workload,
            replicas: "3".into(),
            strategy: "RollingUpdate 25% / 25%".into(),
        }
    }

    fn texts(field: &Field) -> Vec<(&str, Option<Status>)> {
        field
            .cells
            .iter()
            .map(|c| (c.text.as_str(), c.flag))
            .collect()
    }

    #[test]
    fn live_workloads_build_the_design_rows_with_the_base_first() {
        let missing = EnvResolution::MissingKey {
            optional: true,
            keys: vec!["webhook-secret".into()],
        };
        let target = ResourceRef {
            cluster: ClusterId::new("kind-fleet-a"),
            kind: "Deployment".into(),
            namespace: Some("payments".into()),
            name: "checkout-api".into(),
        };
        let observed = [
            observed(
                "kind-fleet-b",
                true,
                spec("2.13.4", "webhook-secret", EnvResolution::Resolved),
            ),
            observed(
                "kind-fleet-a",
                false,
                spec("2.14.0", "webhook_secret", missing),
            ),
        ];
        let comparison = build(&target, &observed, None);
        let clusters: Vec<(&str, Status, &str)> = comparison
            .clusters
            .iter()
            .map(|c| (c.cluster.as_str(), c.status, c.health.as_str()))
            .collect();
        assert_eq!(
            clusters,
            [
                ("kind-fleet-a", Status::Err, "Degraded · 2/3 ready"),
                ("kind-fleet-b", Status::Ok, "Healthy"),
            ]
        );
        let labels: Vec<&str> = comparison.fields.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                READY,
                IMAGE,
                "STRIPE_WEBHOOK_SECRET",
                REPLICAS,
                CPU,
                MEMORY,
                STRATEGY,
                LAST_ROLLOUT
            ]
        );
        assert_eq!(
            texts(&comparison.fields[0]),
            [("2 / 3", Some(Status::Err)), ("3 / 3", None)]
        );
        assert_eq!(
            texts(&comparison.fields[1]),
            [
                ("checkout-api:2.14.0", None),
                ("checkout-api:2.13.4", Some(Status::Warn))
            ]
        );
        assert_eq!(
            texts(&comparison.fields[2]),
            [
                (
                    "secret stripe · key webhook_secret (not found, optional)",
                    Some(Status::Err)
                ),
                ("secret stripe · key webhook-secret", Some(Status::Warn)),
            ]
        );
        assert_eq!(
            texts(&comparison.fields[4]),
            [("0.50 / 1.00", None), ("0.50 / 1.00", None)]
        );
        assert_eq!(
            texts(&comparison.fields[5]),
            [("— / —", None), ("— / —", None)]
        );
        assert_eq!(comparison.differing(), 3);
        assert_eq!(comparison.fields[7].cells[0].text, "1h ago · rev 37");
    }

    #[test]
    fn apply_label_moves_the_other_value_onto_the_base() {
        let diff = SpecDiff {
            path: "spec.template".into(),
            base: ClusterId::new("prod-us-east"),
            other: ClusterId::new("prod-eu-west"),
            lines: vec![PatchLine {
                change: LineChange::Context,
                number: 1,
                text: "containers:".into(),
                note: None,
            }],
        };
        assert_eq!(diff.apply_label(), "Apply eu-west value to us-east…");
        assert!(!diff.has_changes());
    }
}
