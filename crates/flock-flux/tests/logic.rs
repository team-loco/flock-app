use std::{error::Error, io::Write};

use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Compression, write::GzEncoder};
use flock_flux::{
    GitOpsObject, InventoryEntry, ObjectError, ObjectKind, Status, actions, failing_condition,
    generated, helm_storage, matrix, redact, summarize,
};
use k8s_openapi::api::core::v1::Secret;
use serde::Deserialize;
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn Error>>;
const KUSTOMIZATION: &str = include_str!("fixtures/kustomization.json");
const HELMRELEASE: &str = include_str!("fixtures/helmrelease.json");
const SOURCE: &str = include_str!("fixtures/source.json");
const SECRET_ONE: &str = include_str!("fixtures/helm-secret-v1.json");
const SECRET_TWO: &str = include_str!("fixtures/helm-secret-v2.json");

fn object(fixture: &str) -> Result<GitOpsObject, Box<dyn Error>> {
    let value = serde_json::from_str(fixture)?;
    Ok(GitOpsObject::from_json(&value)?)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SummaryCase {
    name: String,
    object: Value,
    expected_status: Status,
    expected_sentence: String,
}

#[test]
fn summaries_use_flux_reasons_and_precedence() -> TestResult {
    let cases: Vec<SummaryCase> = serde_json::from_str(include_str!("fixtures/summaries.json"))?;
    for case in cases {
        let object = GitOpsObject::from_json(&case.object)?;
        let summary = summarize(&object, None);
        assert_eq!(summary.status, case.expected_status, "{}", case.name);
        assert_eq!(summary.sentence, case.expected_sentence, "{}", case.name);
    }
    Ok(())
}

fn retry_cycle() -> Result<[GitOpsObject; 3], Box<dyn Error>> {
    let mut value: Value = serde_json::from_str(KUSTOMIZATION)?;
    let failed = json!([
        {"type": "Reconciling", "status": "True", "reason": "ProgressingWithRetry",
         "message": "Fetching manifests", "observedGeneration": 2, "lastTransitionTime": "2026-10-14T18:27:43Z"},
        {"type": "Ready", "status": "False", "reason": "ArtifactFailed",
         "message": "kustomization path not found: no such file or directory",
         "observedGeneration": 2, "lastTransitionTime": "2026-10-14T18:27:43Z"}
    ]);
    let retrying = json!([
        {"type": "Reconciling", "status": "True", "reason": "Progressing",
         "message": "Fetching manifests", "observedGeneration": 2, "lastTransitionTime": "2026-10-14T18:27:43Z"},
        {"type": "Ready", "status": "Unknown", "reason": "Progressing",
         "message": "Reconciliation in progress", "observedGeneration": 2, "lastTransitionTime": "2026-10-14T18:27:43Z"}
    ]);
    let succeeded = json!([
        {"type": "Ready", "status": "True", "reason": "ReconciliationSucceeded",
         "message": "Applied revision main@sha1:a41e07c", "observedGeneration": 2, "lastTransitionTime": "2026-10-14T18:27:43Z"}
    ]);
    let mut at = |conditions: Value| -> Result<GitOpsObject, Box<dyn Error>> {
        value["status"]["conditions"] = conditions;
        Ok(GitOpsObject::from_json(&value)?)
    };
    Ok([at(failed)?, at(retrying)?, at(succeeded)?])
}

#[test]
fn a_retry_after_a_failure_stays_failed_until_it_succeeds() -> TestResult {
    let [failed, mut retrying, mut succeeded] = retry_cycle()?;
    let mut fresh = retrying.clone();
    assert_eq!(summarize(&failed, None).status, Status::Err);
    assert_eq!(summarize(&retrying, None).status, Status::Progress);
    let mut remembered = failed.clone().carry_failure(None);
    remembered = retrying.carry_failure(remembered);
    let summary = summarize(&retrying, None);
    assert_eq!(summary.status, Status::Err);
    assert_eq!(
        summary.sentence,
        "The build path does not exist in the source artifact."
    );
    remembered = succeeded.carry_failure(remembered);
    assert_eq!(remembered, None);
    assert_eq!(summarize(&succeeded, None).status, Status::Ok);
    fresh.carry_failure(remembered);
    assert_eq!(summarize(&fresh, None).status, Status::Progress);
    Ok(())
}

#[test]
fn failing_condition_names_ready_released_or_stalled() -> TestResult {
    let [failed, retrying, _] = retry_cycle()?;
    let reason = |object: &GitOpsObject| failing_condition(object).map(|c| c.reason.clone());
    assert_eq!(reason(&failed).as_deref(), Some("ArtifactFailed"));
    let mut carried = retrying.clone();
    carried.carry_failure(failing_condition(&failed).cloned());
    assert_eq!(reason(&carried).as_deref(), Some("ArtifactFailed"));
    let mut release = object(HELMRELEASE)?;
    release.conditions = serde_json::from_value(json!([
        {"type": "Ready", "status": "False", "reason": "UpgradeFailed", "message": "Helm upgrade failed"},
        {"type": "Released", "status": "False", "reason": "UpgradeFailed",
         "message": "Helm upgrade failed for release monitoring/kube-prometheus-stack: timed out waiting for the condition"}
    ]))?;
    let released = failing_condition(&release).map(|c| c.message.clone());
    assert!(released.is_some_and(|m| m.contains("timed out waiting")));
    release.conditions.push(serde_json::from_value(json!(
        {"type": "Stalled", "status": "True", "reason": "RetriesExceeded", "message": "Failed to upgrade after 3 attempts"}
    ))?);
    assert_eq!(reason(&release).as_deref(), Some("RetriesExceeded"));
    release.suspended = true;
    assert!(failing_condition(&release).is_none());
    let ready = object(KUSTOMIZATION)?;
    assert!(failing_condition(&ready).is_none());
    let source = object(SOURCE)?;
    assert_eq!(reason(&source).as_deref(), Some("GitOperationFailed"));
    Ok(())
}

#[test]
fn a_remembered_failure_yields_to_a_newer_generation() -> TestResult {
    let [failed, mut retrying, _] = retry_cycle()?;
    let remembered = failed.clone().carry_failure(None);
    retrying.generation = Some(3);
    retrying.carry_failure(remembered);
    assert_eq!(summarize(&retrying, None).status, Status::Progress);
    Ok(())
}

#[test]
fn condition_messages_drop_url_credentials() -> TestResult {
    let mut value: Value = serde_json::from_str(SOURCE)?;
    let condition = value
        .pointer_mut("/status/conditions/0/message")
        .ok_or("missing condition message")?;
    *condition = json!("clone https://user:planted-token@git.example.com/repo.git failed");
    let source = GitOpsObject::from_json(&value)?;
    let messages: Vec<&str> = source
        .conditions
        .iter()
        .map(|c| c.message.as_str())
        .collect();
    assert!(
        messages.contains(&"clone https://git.example.com/repo.git failed"),
        "{messages:?}"
    );
    Ok(())
}

#[test]
fn summary_preserves_failure_and_remediation_details() -> TestResult {
    let object = object(HELMRELEASE)?;
    let summary = summarize(&object, None);
    assert_eq!(
        summary.detail.as_deref(),
        Some("context deadline exceeded Rolled back to 61.9.0")
    );
    Ok(())
}

#[test]
fn source_failures_only_affect_the_referenced_object() -> TestResult {
    let object = object(KUSTOMIZATION)?;
    let mut source = self::object(SOURCE)?;
    let summary = summarize(&object, Some(&source));
    assert_eq!(summary.status, Status::Warn);
    assert_eq!(summary.sentence, "Source fleet-infra is not ready.");
    assert_eq!(summary.detail.as_deref(), Some("authentication required"));
    source.identity.name = "unrelated".into();
    assert_eq!(summarize(&object, Some(&source)).status, Status::Ok);
    Ok(())
}

#[test]
fn typed_kustomization_normalizes_inventory_and_dependencies() -> TestResult {
    let typed: generated::Kustomization = serde_json::from_str(KUSTOMIZATION)?;
    let object = GitOpsObject::try_from(&typed)?;
    assert_eq!(object.identity.kind, ObjectKind::Kustomization);
    assert_eq!(object.identity.name, "apps-payments");
    assert_eq!(object.revision(), Some("main@sha1:a41e07c"));
    assert_eq!(object.interval.as_deref(), Some("5m"));
    assert_eq!(
        object.last_handled_reconcile_at.as_deref(),
        Some("2026-10-12T18:11:00Z")
    );
    assert_eq!(object.generation, Some(2));
    assert_eq!(object.observed_generation, Some(2));
    assert_eq!(object.inventory.len(), 2);
    let deployment = object.inventory.first().ok_or("missing deployment")?;
    assert_eq!(deployment.namespace, "payments");
    assert_eq!(deployment.kind, "Deployment");
    let namespace = object.inventory.last().ok_or("missing namespace")?;
    assert_eq!(namespace.namespace, "");
    assert_eq!(namespace.group, "");
    let dependency = object.depends_on.first().ok_or("missing dependency")?;
    assert_eq!(dependency.namespace, "flux-system");
    assert_eq!(dependency.kind, "Kustomization");
    assert!(dependency.ready_expr.is_some());
    Ok(())
}

#[test]
fn helm_history_uses_deployed_revision_and_keeps_attempted_revision() -> TestResult {
    let typed: generated::HelmRelease = serde_json::from_str(HELMRELEASE)?;
    let object = GitOpsObject::try_from(&typed)?;
    assert_eq!(object.revision(), Some("61.9.0"));
    assert_eq!(object.last_attempted_revision.as_deref(), Some("62.7.0"));
    assert_eq!(object.history.len(), 2);
    assert_eq!(object.inventory.len(), 1);
    let source = object.source_ref.ok_or("missing chart reference")?;
    assert_eq!(source.kind, "HelmChart");
    assert_eq!(source.name, "flux-system-monitoring");
    Ok(())
}

#[test]
fn helm_chart_references_resolve_namespaces() -> TestResult {
    let mut value: Value = serde_json::from_str(HELMRELEASE)?;
    value["spec"]["chartRef"] =
        json!({"kind":"OCIRepository","name":"monitoring","namespace":"sources"});
    let object = GitOpsObject::from_json(&value)?;
    let source = object.source_ref.ok_or("missing OCI reference")?;
    assert_eq!(source.kind, "OCIRepository");
    assert_eq!(source.namespace, "sources");
    value["spec"]
        .as_object_mut()
        .ok_or("invalid spec")?
        .remove("chartRef");
    value["status"]
        .as_object_mut()
        .ok_or("invalid status")?
        .remove("helmChart");
    let object = GitOpsObject::from_json(&value)?;
    let source = object.source_ref.ok_or("missing repository")?;
    assert_eq!(source.kind, "HelmRepository");
    assert_eq!(source.namespace, "flux-system");
    Ok(())
}

#[test]
fn all_generated_sources_normalize_artifact_revisions() -> TestResult {
    for (kind, spec) in [
        (
            ObjectKind::GitRepository,
            json!({"interval":"1m","url":"https://github.com/acme/infra"}),
        ),
        (
            ObjectKind::HelmRepository,
            json!({"url":"https://example.com/charts"}),
        ),
        (
            ObjectKind::HelmChart,
            json!({"interval":"1m","chart":"payments","sourceRef":{"kind":"HelmRepository","name":"repo"}}),
        ),
        (
            ObjectKind::OCIRepository,
            json!({"interval":"1m","url":"oci://example.com/charts"}),
        ),
    ] {
        let value = json!({"apiVersion":kind.api_version(),"kind":kind,"metadata":{"name":"source","namespace":"flux-system"},"spec":spec,"status":{"artifact":{"revision":"revision-one","path":"artifact.tar.gz","url":"http://source/artifact","digest":"sha256:abc","lastUpdateTime":"2026-10-12T18:11:00Z"}}});
        let object = GitOpsObject::from_json(&value)?;
        assert_eq!(object.revision(), Some("revision-one"));
    }
    Ok(())
}

#[test]
fn invalid_external_fields_are_rejected_and_unknown_fields_are_tolerated() -> TestResult {
    let original: Value = serde_json::from_str(KUSTOMIZATION)?;
    for (pointer, invalid) in [
        ("/metadata/name", json!("")),
        ("/metadata/namespace", json!("")),
        ("/spec/suspend", json!("yes")),
        ("/spec/sourceRef/name", json!("")),
        ("/status/inventory/entries/0/id", json!("bad")),
    ] {
        let mut value = original.clone();
        if let Some(field) = value.pointer_mut(pointer) {
            *field = invalid;
        } else if pointer == "/spec/suspend" {
            value["spec"]["suspend"] = invalid;
        }
        assert!(GitOpsObject::from_json(&value).is_err(), "{pointer}");
    }
    let mut value = original.clone();
    value["apiVersion"] = json!("kustomize.toolkit.fluxcd.io/v9");
    assert!(matches!(
        GitOpsObject::from_json(&value),
        Err(ObjectError::UnsupportedVersion(_))
    ));
    let mut value = original;
    value["spec"]["futureField"] = json!("ignored");
    assert!(GitOpsObject::from_json(&value).is_ok());
    assert!(InventoryEntry::parse("ns_name_group_kind_extra", "v1").is_err());
    assert!(InventoryEntry::parse("ns__group_kind", "v1").is_err());
    assert!(InventoryEntry::parse("ns_name_group_kind", "").is_err());
    Ok(())
}

#[test]
fn stale_failure_and_drift_conditions_do_not_override_current_readiness() -> TestResult {
    let mut object = object(KUSTOMIZATION)?;
    let mut stale = object.ready().ok_or("missing ready")?.clone();
    stale.type_ = "Stalled".into();
    stale.observed_generation = Some(1);
    object.conditions.push(stale.clone());
    stale.type_ = "Drifted".into();
    object.conditions.push(stale);
    assert_eq!(summarize(&object, None).status, Status::Ok);
    Ok(())
}

#[test]
fn patches_share_tokens_and_touch_only_intended_fields() -> TestResult {
    let at = "2026-10-12T18:11:00Z".parse()?;
    let token = "2026-10-12T18:11:00+00:00";
    assert_eq!(
        actions::reconcile(at),
        json!({"metadata":{"annotations":{"reconcile.fluxcd.io/requestedAt":token}}})
    );
    assert_eq!(
        actions::force(at),
        json!({"metadata":{"annotations":{"reconcile.fluxcd.io/requestedAt":token,"reconcile.fluxcd.io/forceAt":token}}})
    );
    assert_eq!(
        actions::reset(at),
        json!({"metadata":{"annotations":{"reconcile.fluxcd.io/requestedAt":token,"reconcile.fluxcd.io/resetAt":token}}})
    );
    assert_eq!(actions::suspend(), json!({"spec":{"suspend":true}}));
    assert_eq!(actions::resume(), json!({"spec":{"suspend":false}}));
    Ok(())
}

#[test]
fn helm_storage_decodes_both_base64_layers_and_all_release_fields() -> TestResult {
    let release = helm_storage::decode_secret_json(SECRET_TWO)?;
    assert_eq!(release.name, "payments.vault");
    assert_eq!(release.namespace, "payments");
    assert_eq!(release.version, 2);
    assert_eq!(release.status, "deployed");
    assert_eq!(release.chart_name, "payments");
    assert_eq!(release.chart_version, "1.2.0");
    assert_eq!(release.app_version.as_deref(), Some("2.14.0"));
    assert!(release.first_deployed.is_some());
    assert!(release.last_deployed.is_some());
    assert_eq!(release.description, "Upgrade complete");
    assert!(release.manifest.contains("value: after"));
    Ok(())
}

#[test]
fn helm_history_is_ordered_and_diff_contains_every_change() -> TestResult {
    let one: Secret = serde_json::from_str(SECRET_ONE)?;
    let two: Secret = serde_json::from_str(SECRET_TWO)?;
    let history = helm_storage::revision_history([&two, &one])?;
    let older = history.first().ok_or("missing older revision")?;
    let newer = history.last().ok_or("missing newer revision")?;
    assert_eq!(older.version, 1);
    assert_eq!(newer.version, 2);
    let diff = helm_storage::manifest_diff(older, newer);
    assert!(diff.contains("--- payments.vault/v1"));
    assert!(diff.contains("+++ payments.vault/v2"));
    assert!(diff.contains("-  value: before\n+  value: after\n"));
    assert_eq!(helm_storage::manifest_diff(newer, newer), "");
    assert!(helm_storage::revision_history([&one, &one]).is_err());
    Ok(())
}

#[test]
fn helm_history_diffs_never_show_secret_values() -> TestResult {
    let one: Secret = serde_json::from_str(SECRET_ONE)?;
    let two: Secret = serde_json::from_str(SECRET_TWO)?;
    let mut history = helm_storage::revision_history([&one, &two])?;
    let secret = |password: &str| {
        format!(
            "---\n# Source: payments/templates/secret.yaml\napiVersion: v1\nkind: Secret\nmetadata:\n  name: db\n  namespace: payments\ndata:\n  password: {password}\nstringData:\n  api-key: fixed-key\n"
        )
    };
    let [older, newer] = history.as_mut_slice() else {
        return Err("expected two revisions".into());
    };
    older.manifest.push_str(&secret("b2xkLXBhc3N3b3Jk"));
    newer.manifest.push_str(&secret("bmV3LXBhc3N3b3Jk"));
    let diff = helm_storage::manifest_diff(older, newer);
    for value in ["b2xkLXBhc3N3b3Jk", "bmV3LXBhc3N3b3Jk", "fixed-key"] {
        assert!(!diff.contains(value), "{diff}");
    }
    assert!(
        diff.contains("-  value: before\n+  value: after\n"),
        "{diff}"
    );
    assert!(
        diff.contains(&format!(
            "-  password: {}\n+  password: {}\n",
            redact::MASK,
            redact::CHANGED
        )),
        "{diff}"
    );
    Ok(())
}

fn encoded_secret(bytes: &[u8]) -> Result<Secret, Box<dyn Error>> {
    let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
    gzip.write_all(bytes)?;
    let compressed = gzip.finish()?;
    let base64_text = STANDARD.encode(compressed);
    let mut secret: Secret = serde_json::from_str(SECRET_ONE)?;
    let data = secret.data.as_mut().ok_or("missing data")?;
    let release = data.get_mut("release").ok_or("missing release")?;
    release.0 = base64_text.into_bytes();
    Ok(secret)
}

#[test]
fn helm_storage_rejects_corrupt_and_mismatched_external_data() -> TestResult {
    let original: Secret = serde_json::from_str(SECRET_ONE)?;
    let mut secret = original.clone();
    secret.type_ = Some("Opaque".into());
    assert!(matches!(
        helm_storage::decode_secret(&secret),
        Err(helm_storage::StorageError::InvalidSecret)
    ));
    secret = original.clone();
    secret.data = None;
    assert!(matches!(
        helm_storage::decode_secret(&secret),
        Err(helm_storage::StorageError::MissingData)
    ));
    secret = original.clone();
    secret.metadata.name = Some("sh.helm.release.v1.payments.vault.v2".into());
    assert!(matches!(
        helm_storage::decode_secret(&secret),
        Err(helm_storage::StorageError::MetadataMismatch)
    ));
    secret = original;
    let data = secret.data.as_mut().ok_or("missing data")?;
    data.get_mut("release").ok_or("missing release")?.0 = b"%%%".to_vec();
    assert!(matches!(
        helm_storage::decode_secret(&secret),
        Err(helm_storage::StorageError::Base64(_))
    ));
    let data = secret.data.as_mut().ok_or("missing data")?;
    data.get_mut("release").ok_or("missing release")?.0 = STANDARD.encode(b"not gzip").into_bytes();
    assert!(matches!(
        helm_storage::decode_secret(&secret),
        Err(helm_storage::StorageError::Compression(_))
    ));
    let invalid_json = encoded_secret(b"not json")?;
    assert!(matches!(
        helm_storage::decode_secret(&invalid_json),
        Err(helm_storage::StorageError::Json(_))
    ));
    Ok(())
}

#[test]
fn matrix_matches_kind_namespace_and_name_preserving_cluster_order() -> TestResult {
    let object = object(KUSTOMIZATION)?;
    let mut changed = object.clone();
    changed.last_applied_revision = Some("main@sha1:older".into());
    changed.suspended = true;
    let clusters = [
        matrix::ClusterObjects {
            cluster: "prod".into(),
            online: true,
            objects: vec![object],
        },
        matrix::ClusterObjects {
            cluster: "staging".into(),
            online: true,
            objects: vec![changed],
        },
        matrix::ClusterObjects {
            cluster: "dev".into(),
            online: true,
            objects: vec![],
        },
    ];
    let rows = matrix::rows(&clusters)?;
    let row = rows.first().ok_or("missing row")?;
    assert!(row.differs);
    assert_eq!(
        row.cells
            .iter()
            .map(|cell| cell.cluster.as_str())
            .collect::<Vec<_>>(),
        ["prod", "staging", "dev"]
    );
    let staging = row
        .cells
        .iter()
        .find(|cell| cell.cluster == "staging")
        .ok_or("missing staging")?;
    assert!(staging.differs);
    assert_eq!(staging.summary.status, Status::Suspended);
    let dev = row.cells.last().ok_or("missing dev")?;
    assert!(!dev.present);
    assert!(!dev.differs);
    assert_eq!(
        dev.summary.sentence,
        "The object is not deployed on this cluster."
    );
    Ok(())
}

#[test]
fn matrix_excludes_stale_offline_revisions_and_rejects_duplicates() -> TestResult {
    let object = object(KUSTOMIZATION)?;
    let mut stale = object.clone();
    stale.last_applied_revision = Some("old".into());
    let prod = matrix::ClusterObjects {
        cluster: "prod".into(),
        online: true,
        objects: vec![object.clone()],
    };
    let offline = matrix::ClusterObjects {
        cluster: "offline".into(),
        online: false,
        objects: vec![stale],
    };
    let rows = matrix::rows(&[prod.clone(), offline])?;
    let row = rows.first().ok_or("missing row")?;
    assert!(!row.differs);
    assert_eq!(
        row.cells.last().ok_or("missing offline")?.summary.status,
        Status::Offline
    );
    assert!(matches!(
        matrix::rows(&[prod.clone(), prod]),
        Err(matrix::MatrixError::DuplicateCluster(_))
    ));
    let duplicate = matrix::ClusterObjects {
        cluster: "prod".into(),
        online: true,
        objects: vec![object.clone(), object.clone()],
    };
    assert!(matches!(
        matrix::rows(&[duplicate]),
        Err(matrix::MatrixError::DuplicateObject(_))
    ));
    let mut other_namespace = object.clone();
    other_namespace.identity.namespace = "other".into();
    let cluster = matrix::ClusterObjects {
        cluster: "prod".into(),
        online: true,
        objects: vec![object, other_namespace],
    };
    assert_eq!(matrix::rows(&[cluster])?.len(), 2);
    Ok(())
}

#[test]
fn helm_values_split_user_supplied_from_computed_and_redact_secrets() -> TestResult {
    let stored = json!({
        "name": "payments.vault", "namespace": "payments", "version": 1,
        "info": { "status": "deployed" },
        "chart": {
            "metadata": { "name": "payments", "version": "1.2.0" },
            "values": {
                "replicas": 1,
                "image": { "repository": "ghcr.io/acme/payments", "tag": "1.0.0" },
                "database": { "host": "db", "password": "chart-default" },
                "debug": true
            }
        },
        "config": {
            "image": { "tag": "1.2.0" },
            "database": { "password": "planted-password" },
            "apiToken": "planted-token",
            "repo": "https://deploy:planted-url-secret@git.example.com/acme",
            "debug": null
        },
        "manifest": ""
    });
    let secret = encoded_secret(&serde_json::to_vec(&stored)?)?;
    let release = helm_storage::decode_secret(&secret)?;
    let computed = helm_storage::computed_values(&release);
    assert_eq!(
        computed,
        json!({
            "replicas": 1,
            "image": { "repository": "ghcr.io/acme/payments", "tag": "1.2.0" },
            "database": { "host": "db", "password": "planted-password" },
            "apiToken": "planted-token",
            "repo": "https://deploy:planted-url-secret@git.example.com/acme"
        })
    );
    for mut values in [release.config.clone(), computed] {
        redact::values(&mut values);
        let text = values.to_string();
        for planted in ["planted-password", "planted-token", "planted-url-secret"] {
            assert!(!text.contains(planted), "{text}");
        }
        assert_eq!(values.pointer("/image/tag"), Some(&json!("1.2.0")));
        assert_eq!(values.pointer("/apiToken"), Some(&json!(redact::MASK)));
    }
    let bare = helm_storage::decode_secret_json(SECRET_ONE)?;
    assert!(bare.chart_values.is_object());
    Ok(())
}
