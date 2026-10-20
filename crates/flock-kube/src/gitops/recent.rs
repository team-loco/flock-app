use flock_core::cluster_changes::ChangeKind;
use flock_core::{ClusterId, ResourceRef, Status};
use flock_flux::ObjectKind;
use flock_flux::revision::{git, short};
use jiff::Timestamp;

use super::convert::{Parsed, reconciled_at};
use crate::cluster_changes::Change;

const KUSTOMIZE_CONTROLLER: &str = "kustomize-controller";
const HELM_CONTROLLER: &str = "helm-controller";
const FAILED: &str = "failed";
const ROLLBACK: &str = "rollback";
const REFERENCE_SEPARATOR: char = '/';

fn target(cluster: &ClusterId, parsed: &Parsed) -> ResourceRef {
    let identity = &parsed.object.identity;
    ResourceRef {
        cluster: cluster.clone(),
        kind: identity.kind.as_str().to_owned(),
        namespace: Some(identity.namespace.clone()),
        name: identity.name.clone(),
    }
}

fn revision_label(revision: &str) -> String {
    let sha = short(revision);
    git(revision)
        .and_then(|parsed| parsed.reference.rsplit(REFERENCE_SEPARATOR).next())
        .filter(|reference| !reference.is_empty())
        .map_or_else(|| sha.clone(), |reference| format!("{reference}@{sha}"))
}

fn applied(cluster: &ClusterId, parsed: &Parsed) -> Option<Change> {
    let revision = parsed.object.last_applied_revision.as_deref()?;
    let at = reconciled_at(parsed, revision).or_else(|| parsed.ready_since())?;
    let name = &parsed.object.identity.name;
    Some(Change {
        at,
        kind: ChangeKind::Flux,
        status: None,
        target: target(cluster, parsed),
        what: format!("ks/{name} → {}", revision_label(revision)),
        by: KUSTOMIZE_CONTROLLER.to_owned(),
    })
}

fn releases(cluster: &ClusterId, parsed: &Parsed) -> Vec<Change> {
    let identity = &parsed.object.identity;
    let mut history: Vec<_> = parsed.object.history.iter().collect();
    history.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.version));
    let previous = history.iter().skip(1).map(Some).chain([None]);
    history
        .iter()
        .zip(previous)
        .filter_map(|(snapshot, before)| {
            let at: Timestamp = snapshot.last_deployed.parse().ok()?;
            let rollback = snapshot.action.as_deref() == Some(ROLLBACK);
            let version = match before {
                _ if rollback => format!(
                    "{ROLLBACK} to {} · rev {}",
                    snapshot.chart_version, snapshot.version
                ),
                Some(before) if before.chart_version != snapshot.chart_version => {
                    format!("{} → {}", before.chart_version, snapshot.chart_version)
                }
                _ => format!("{} · rev {}", snapshot.chart_version, snapshot.version),
            };
            let failed = snapshot.status == FAILED;
            let outcome = if failed {
                format!(" · {FAILED}")
            } else {
                String::new()
            };
            Some(Change {
                at,
                kind: ChangeKind::Helm,
                status: failed.then_some(Status::Err),
                target: target(cluster, parsed),
                what: format!(
                    "{}/{} {version}{outcome}",
                    identity.namespace, identity.name
                ),
                by: HELM_CONTROLLER.to_owned(),
            })
        })
        .collect()
}

pub(super) fn changes(cluster: &ClusterId, objects: &[Parsed]) -> Vec<Change> {
    objects
        .iter()
        .flat_map(|parsed| match parsed.kind() {
            ObjectKind::Kustomization => applied(cluster, parsed).into_iter().collect(),
            ObjectKind::HelmRelease => releases(cluster, parsed),
            ObjectKind::GitRepository => Vec::new(),
            ObjectKind::HelmRepository => Vec::new(),
            ObjectKind::HelmChart => Vec::new(),
            ObjectKind::OCIRepository => Vec::new(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::gitops::convert::tests::{kustomization, parsed};

    fn cluster() -> ClusterId {
        ClusterId::new("fleet-a")
    }

    fn snapshot(version: i64, chart: &str, status: &str, deployed: &str) -> serde_json::Value {
        json!({
            "name": "kube-prometheus-stack", "namespace": "monitoring", "version": version,
            "status": status, "chartName": "kube-prometheus-stack", "chartVersion": chart,
            "firstDeployed": "2026-10-01T00:00:00Z", "lastDeployed": deployed,
            "configDigest": "sha256:c0", "digest": "sha256:d0"
        })
    }

    #[test]
    fn kustomization_reports_its_applied_revision_at_first_reconcile() {
        let ks = parsed(&kustomization("platform", "True", "a91c3f2aa8f1"));
        let changes = changes(&cluster(), &[ks]);
        let [change] = changes.as_slice() else {
            panic!("{}", changes.len());
        };
        assert_eq!(change.what, "ks/platform → master@a91c3f2");
        assert_eq!(change.by, KUSTOMIZE_CONTROLLER);
        assert_eq!(change.at.to_string(), "2026-10-13T08:00:00Z");
    }

    #[test]
    fn helm_history_shows_upgrades_rollbacks_and_failures_newest_first() {
        let mut rolled_back = snapshot(4, "62.6.0", "deployed", "2026-10-13T10:05:00Z");
        rolled_back["action"] = json!(ROLLBACK);
        let release = parsed(&json!({
            "apiVersion": "helm.toolkit.fluxcd.io/v2",
            "kind": "HelmRelease",
            "metadata": { "name": "kube-prometheus-stack", "namespace": "monitoring" },
            "spec": { "interval": "10m", "chart": { "spec": { "chart": "kube-prometheus-stack", "sourceRef": { "kind": "HelmRepository", "name": "prometheus" } } } },
            "status": { "history": [
                rolled_back,
                snapshot(2, "62.6.0", "superseded", "2026-10-12T09:00:00Z"),
                snapshot(3, "62.7.0", "failed", "2026-10-13T10:00:00Z"),
                snapshot(1, "62.6.0", "superseded", "2026-10-10T09:00:00Z")
            ]}
        }));
        let rows: Vec<(String, Option<Status>)> = changes(&cluster(), &[release])
            .into_iter()
            .map(|c| (c.what, c.status))
            .collect();
        assert_eq!(
            rows,
            [
                (
                    "monitoring/kube-prometheus-stack rollback to 62.6.0 · rev 4".to_owned(),
                    None
                ),
                (
                    "monitoring/kube-prometheus-stack 62.6.0 → 62.7.0 · failed".to_owned(),
                    Some(Status::Err)
                ),
                (
                    "monitoring/kube-prometheus-stack 62.6.0 · rev 2".to_owned(),
                    None
                ),
                (
                    "monitoring/kube-prometheus-stack 62.6.0 · rev 1".to_owned(),
                    None
                ),
            ]
        );
    }
}
