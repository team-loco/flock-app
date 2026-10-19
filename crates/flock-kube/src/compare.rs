use std::sync::Arc;

use flock_core::changes::PatchLine;
use flock_core::compare::{Observed, SpecDiff, build};
use flock_core::{ClusterId, Comparison, ResourceRef, WorkloadKind};
use futures::{StreamExt, stream};
use jiff::Timestamp;
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use k8s_openapi::api::autoscaling::v2::HorizontalPodAutoscaler;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::Api;
use kube::api::ListParams;
use serde_json::Value;

use crate::manager::ClusterHandle;

pub const SPEC_PATH: &str = "spec.template";
const CPU: &str = "cpu";
const ROLLING_UPDATE: &str = "RollingUpdate";
const SEPARATOR: &str = " · ";
const CLUSTER_LIMIT: usize = 8;

pub struct Probe {
    pub observed: Observed,
    pub template: Value,
}

fn int_or_string(value: Option<&IntOrString>) -> Option<String> {
    match value? {
        IntOrString::Int(number) => Some(number.to_string()),
        IntOrString::String(text) => Some(text.clone()),
    }
}

pub fn rolling_label(
    kind: Option<&str>,
    surge: Option<&IntOrString>,
    unavailable: Option<&IntOrString>,
) -> String {
    let kind = kind.unwrap_or(ROLLING_UPDATE);
    let limits: Vec<String> = [int_or_string(surge), int_or_string(unavailable)]
        .into_iter()
        .flatten()
        .collect();
    if limits.is_empty() {
        kind.to_owned()
    } else {
        format!("{kind} {}", limits.join(" / "))
    }
}

fn find<K: kube::Resource + Clone>(items: Vec<Arc<K>>, target: &ResourceRef) -> Option<Arc<K>> {
    items.into_iter().find(|item| {
        item.meta().name.as_deref() == Some(target.name.as_str())
            && item.meta().namespace == target.namespace
    })
}

pub fn spec_of(handle: &ClusterHandle, target: &ResourceRef) -> Option<(Value, String)> {
    let kind = WorkloadKind::from_kind(&target.kind)?;
    match kind {
        WorkloadKind::Deployment => {
            let item = find(handle.objects::<Deployment>(), target)?;
            let spec = item.spec.as_ref()?;
            let strategy = spec.strategy.as_ref();
            let rolling = strategy.and_then(|s| s.rolling_update.as_ref());
            let label = rolling_label(
                strategy.and_then(|s| s.type_.as_deref()),
                rolling.and_then(|r| r.max_surge.as_ref()),
                rolling.and_then(|r| r.max_unavailable.as_ref()),
            );
            Some((serde_json::to_value(&spec.template).ok()?, label))
        }
        WorkloadKind::StatefulSet => {
            let item = find(handle.objects::<StatefulSet>(), target)?;
            let spec = item.spec.as_ref()?;
            let strategy = spec.update_strategy.as_ref();
            let rolling = strategy.and_then(|s| s.rolling_update.as_ref());
            let label = rolling_label(
                strategy.and_then(|s| s.type_.as_deref()),
                None,
                rolling.and_then(|r| r.max_unavailable.as_ref()),
            );
            Some((serde_json::to_value(&spec.template).ok()?, label))
        }
        WorkloadKind::DaemonSet => {
            let item = find(handle.objects::<DaemonSet>(), target)?;
            let spec = item.spec.as_ref()?;
            let strategy = spec.update_strategy.as_ref();
            let rolling = strategy.and_then(|s| s.rolling_update.as_ref());
            let label = rolling_label(
                strategy.and_then(|s| s.type_.as_deref()),
                rolling.and_then(|r| r.max_surge.as_ref()),
                rolling.and_then(|r| r.max_unavailable.as_ref()),
            );
            Some((serde_json::to_value(&spec.template).ok()?, label))
        }
    }
}

pub fn autoscaler_label(hpa: &HorizontalPodAutoscaler) -> String {
    let spec = &hpa.spec;
    let min = spec.min_replicas.unwrap_or(1);
    let range = format!("{min}–{}", spec.max_replicas);
    let cpu = spec
        .metrics
        .iter()
        .flatten()
        .filter_map(|metric| metric.resource.as_ref())
        .find(|resource| resource.name == CPU)
        .and_then(|resource| resource.target.average_utilization);
    match cpu {
        Some(percent) => format!("{range} @ {percent}% {CPU}"),
        None => range,
    }
}

pub fn replicas_label(desired: u32, autoscaler: Option<&str>) -> String {
    match autoscaler {
        Some(range) => format!("{desired}{SEPARATOR}{range}"),
        None => desired.to_string(),
    }
}

async fn autoscaler(handle: &ClusterHandle, target: &ResourceRef) -> Option<String> {
    let client = handle.client().ok()?;
    let namespace = target.namespace.as_deref()?;
    let api: Api<HorizontalPodAutoscaler> = Api::namespaced(client, namespace);
    let listed = api.list(&ListParams::default()).await.ok()?;
    listed
        .items
        .iter()
        .find(|hpa| {
            let reference = &hpa.spec.scale_target_ref;
            reference.kind == target.kind && reference.name == target.name
        })
        .map(autoscaler_label)
}

pub async fn observe(
    handle: &ClusterHandle,
    cluster: &ClusterId,
    target: &ResourceRef,
    now: Timestamp,
) -> Option<Probe> {
    let on = ResourceRef {
        cluster: cluster.clone(),
        ..target.clone()
    };
    let (template, strategy) = spec_of(handle, &on)?;
    let (workload, range) = tokio::join!(
        crate::workload::build(handle, &on, now),
        autoscaler(handle, &on)
    );
    let workload = workload?;
    let replicas = replicas_label(workload.desired, range.as_deref());
    Some(Probe {
        observed: Observed {
            cluster: cluster.clone(),
            workload,
            replicas,
            strategy,
        },
        template,
    })
}

fn yaml(template: &Value) -> String {
    serde_norway::to_string(template).unwrap_or_default()
}

pub fn spec_lines(base: &Value, other: &Value) -> Vec<PatchLine> {
    flock_core::edit::diff_lines(&yaml(base), &yaml(other))
}

pub fn pick_other<'a>(
    base: &ClusterId,
    chosen: Option<&ClusterId>,
    probes: &'a [Probe],
) -> Option<&'a Probe> {
    let template = probes
        .iter()
        .find(|probe| &probe.observed.cluster == base)
        .map(|probe| &probe.template);
    let others = || {
        probes
            .iter()
            .filter(|probe| &probe.observed.cluster != base)
    };
    let picked = chosen.and_then(|chosen| others().find(|p| &p.observed.cluster == chosen));
    picked
        .or_else(|| others().find(|p| Some(&p.template) != template))
        .or_else(|| others().next())
}

pub fn comparison(
    target: &ResourceRef,
    chosen: Option<&ClusterId>,
    probes: &[Probe],
) -> Option<Comparison> {
    let base = probes
        .iter()
        .find(|probe| probe.observed.cluster == target.cluster)?;
    let spec = pick_other(&target.cluster, chosen, probes).map(|other| SpecDiff {
        path: SPEC_PATH.to_owned(),
        base: target.cluster.clone(),
        other: other.observed.cluster.clone(),
        lines: spec_lines(&base.template, &other.template),
    });
    let observed: Vec<Observed> = probes.iter().map(|p| p.observed.clone()).collect();
    Some(build(target, &observed, spec))
}

pub async fn compare(
    clusters: &[(ClusterId, ClusterHandle)],
    target: &ResourceRef,
    chosen: Option<&ClusterId>,
    now: Timestamp,
) -> Option<Comparison> {
    let observations: Vec<_> = clusters
        .iter()
        .map(|(id, handle)| observe(handle, id, target, now))
        .collect();
    let probes: Vec<Probe> = stream::iter(observations)
        .buffered(CLUSTER_LIMIT)
        .filter_map(std::future::ready)
        .collect()
        .await;
    comparison(target, chosen, &probes)
}

#[cfg(test)]
mod tests {
    use flock_core::{Uid, Workload};
    use serde_json::json;

    use flock_core::changes::LineChange;

    use super::*;

    fn probe(cluster: &str, image: &str) -> Probe {
        let target = ResourceRef {
            cluster: ClusterId::new(cluster),
            kind: "Deployment".into(),
            namespace: Some("payments".into()),
            name: "checkout-api".into(),
        };
        Probe {
            observed: Observed {
                cluster: ClusterId::new(cluster),
                workload: Workload {
                    uid: Uid::default(),
                    target,
                    kind: WorkloadKind::Deployment,
                    desired: 1,
                    stalled: false,
                    pending: None,
                    managed_by: None,
                    pods: Vec::new(),
                    revisions: Vec::new(),
                    history_pending: false,
                    containers: Vec::new(),
                    events: Vec::new(),
                },
                replicas: "1".into(),
                strategy: "RollingUpdate".into(),
            },
            template: json!({ "spec": { "containers": [{ "name": "api", "image": image, "args": ["--port", "9898"] }] } }),
        }
    }

    #[test]
    fn strategies_read_like_the_design() {
        let quarter = IntOrString::String("25%".into());
        assert_eq!(
            rolling_label(Some("RollingUpdate"), Some(&quarter), Some(&quarter)),
            "RollingUpdate 25% / 25%"
        );
        assert_eq!(rolling_label(Some("OnDelete"), None, None), "OnDelete");
        assert_eq!(
            rolling_label(None, None, Some(&IntOrString::Int(1))),
            "RollingUpdate 1"
        );
    }

    #[test]
    fn autoscalers_show_range_and_cpu_target() {
        let parsed = serde_json::from_value::<HorizontalPodAutoscaler>(json!({
            "metadata": { "name": "checkout-api" },
            "spec": {
                "minReplicas": 3, "maxReplicas": 12,
                "scaleTargetRef": { "apiVersion": "apps/v1", "kind": "Deployment", "name": "checkout-api" },
                "metrics": [{ "type": "Resource", "resource": { "name": "cpu", "target": { "type": "Utilization", "averageUtilization": 70 } } }]
            }
        }));
        let Ok(hpa) = parsed else {
            panic!("hpa fixture");
        };
        let label = autoscaler_label(&hpa);
        assert_eq!(label, "3–12 @ 70% cpu");
        assert_eq!(replicas_label(3, Some(&label)), "3 · 3–12 @ 70% cpu");
        assert_eq!(replicas_label(2, None), "2");
    }

    #[test]
    fn template_diff_marks_the_changed_lines() {
        let lines = spec_lines(
            &probe("a", "api:2.14.0").template,
            &probe("b", "api:2.13.4").template,
        );
        let changed: Vec<(LineChange, &str)> = lines
            .iter()
            .filter(|line| line.change != LineChange::Context)
            .map(|line| (line.change, line.text.trim()))
            .collect();
        assert_eq!(
            changed,
            [
                (LineChange::Removed, "image: api:2.14.0"),
                (LineChange::Added, "image: api:2.13.4"),
            ]
        );
        assert!(lines.iter().any(|line| line.change == LineChange::Context));
    }

    #[test]
    fn the_other_side_is_the_chosen_cluster_or_the_first_that_differs() {
        let probes = [
            probe("a", "api:1"),
            probe("b", "api:1"),
            probe("c", "api:2"),
        ];
        let base = ClusterId::new("a");
        let other = |chosen: Option<&str>| {
            let chosen = chosen.map(ClusterId::new);
            pick_other(&base, chosen.as_ref(), &probes).map(|p| p.observed.cluster.to_string())
        };
        assert_eq!(other(None).as_deref(), Some("c"));
        assert_eq!(other(Some("b")).as_deref(), Some("b"));
        assert_eq!(other(Some("a")).as_deref(), Some("c"));
        assert_eq!(other(Some("ghost")).as_deref(), Some("c"));
        let target = probes[0].observed.workload.target.clone();
        let comparison = comparison(&target, None, &probes);
        assert_eq!(comparison.as_ref().map(|c| c.clusters.len()), Some(3));
        let spec = comparison.and_then(|c| c.spec);
        assert!(spec.as_ref().is_some_and(SpecDiff::has_changes));
        assert_eq!(spec.map(|s| s.other), Some(ClusterId::new("c")));
        let missing = ResourceRef {
            cluster: ClusterId::new("z"),
            ..target
        };
        assert_eq!(super::comparison(&missing, None, &probes), None);
    }
}
