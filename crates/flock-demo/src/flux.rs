use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use flock_core::diagnosis::from_conditions;
use flock_core::flux::{
    Drift, DriftField, DriftState, DriftedObject, FluxDetail, HelmDetail, HelmRevision,
    HistoryEntry, ManagedObject, Phase, Revision, Sentence, SourceDetail, Span, SpecRow,
};
use flock_core::{Condition, FluxAction, FluxKey, FluxKind, FluxObject, Snapshot, Status, Uid};

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;

const FLUX_SYSTEM: &str = "flux-system";
const MONITORING: &str = "monitoring";
const FLEET_INFRA: &str = "fleet-infra";
const PROMETHEUS_COMMUNITY: &str = "prometheus-community";
const KUBE_PROMETHEUS_STACK: &str = "kube-prometheus-stack";
const APPS_PAYMENTS: &str = "apps-payments";
const INFRA_CONTROLLERS: &str = "infra-controllers";
const INFRA_CONFIGS: &str = "infra-configs";
const EAST_APPS: [&str; 3] = ["apps-web", "apps-search", "apps-ml"];
const RETRY_LEAD: &str = "Retrying in ";
const RETRY_INTERVAL: &str = "2m";

const PROD_US_EAST: &str = "prod-us-east";
const PROD_EU_WEST: &str = "prod-eu-west";
const STAGING: &str = "staging";
const DEV_KIND: &str = "dev-kind";
const FLUX_CLUSTERS: [&str; 4] = [PROD_US_EAST, PROD_EU_WEST, STAGING, DEV_KIND];

const HEAD: &str = "a41e07c";
const HEAD_FULL: &str = "a41e07c3f91d";
const MONITORING_BUMP: &str = "3be91f2";
const MONITORING_BUMP_FULL: &str = "3be91f2a0c77";
const PREVIOUS: &str = "7c2d9e0";
const PREVIOUS_FULL: &str = "7c2d9e0b51e4";
const PROMETHEUS_OLD: &str = "61.9.0";
const PROMETHEUS_NEW: &str = "62.7.0";
const HEALTH_TIMEOUT: &str = "5m";
const READY: &str = "Ready";
const RELEASED: &str = "Released";
const RECONCILING: &str = "Reconciling";
const TRUE: &str = "True";
const FALSE: &str = "False";
const UNKNOWN: &str = "Unknown";
const SUCCEEDED: &str = "ReconciliationSucceeded";
const PROGRESSING: &str = "Progressing";
const HEALTH_CHECK_FAILED: &str = "HealthCheckFailed";
const UPGRADE_FAILED: &str = "UpgradeFailed";
const PAYMENTS_HEALTH_MESSAGE: &str = "health check failed after 5m0s: timeout waiting for: [Deployment/payments/checkout-api status: 'InProgress']";
const PROMETHEUS_UPGRADE_MESSAGE: &str = "Helm upgrade failed for release monitoring/kube-prometheus-stack with chart kube-prometheus-stack@62.7.0: context deadline exceeded";

const fn ago(secs: u64) -> Duration {
    Duration::from_secs(secs)
}

fn key(cluster: &str, kind: FluxKind, namespace: &str, name: &str) -> FluxKey {
    FluxKey::new(cluster, kind, namespace, name)
}

fn fleet_infra(cluster: &str) -> FluxKey {
    key(cluster, FluxKind::GitRepository, FLUX_SYSTEM, FLEET_INFRA)
}

fn prometheus_repo(cluster: &str) -> FluxKey {
    key(
        cluster,
        FluxKind::HelmRepository,
        FLUX_SYSTEM,
        PROMETHEUS_COMMUNITY,
    )
}

fn entry(age: u64, status: Status, revision: &str, message: &str, count: u32) -> HistoryEntry {
    HistoryEntry {
        age: Some(ago(age)),
        status,
        revision: revision.into(),
        message: message.into(),
        count,
    }
}

fn managed(
    status: Status,
    kind: &str,
    namespace: Option<&str>,
    name: &str,
    detail: &str,
    changed: bool,
) -> ManagedObject {
    ManagedObject {
        status,
        kind: kind.into(),
        namespace: namespace.map(Into::into),
        name: name.into(),
        detail: detail.into(),
        changed,
    }
}

fn condition_yaml(phase: Phase, sentence: &Sentence) -> String {
    let ready = if phase.status() == Status::Ok {
        "\"True\""
    } else {
        "\"False\""
    };
    format!(
        "status:\n  conditions:\n  - type: Ready\n    status: {ready}\n    reason: {}\n    message: \"{}\"\n",
        phase.label(),
        sentence.plain()
    )
}

fn object_yaml(key: &FluxKey, api: &str, spec: &str, phase: Phase, sentence: &Sentence) -> String {
    format!(
        "apiVersion: {api}\nkind: {}\nmetadata:\n  name: {}\n  namespace: {}\nspec:\n{spec}{}",
        key.kind.label(),
        key.name,
        key.namespace,
        condition_yaml(phase, sentence)
    )
}

pub fn objects() -> Vec<FluxObject> {
    let mut objects = vec![apps_payments_east()];
    objects.extend(peer_payments());
    for cluster in FLUX_CLUSTERS {
        objects.push(infra(
            cluster,
            INFRA_CONTROLLERS,
            "./infrastructure/controllers",
        ));
        objects.push(infra(cluster, INFRA_CONFIGS, "./infrastructure/configs"));
        objects.push(git_source(cluster));
        objects.push(helm_source(cluster));
    }
    for name in EAST_APPS {
        let path = format!(
            "./apps/{}/overlays/{PROD_US_EAST}",
            name.trim_start_matches("apps-")
        );
        objects.push(infra(PROD_US_EAST, name, &path));
    }
    objects.push(prometheus_staging());
    for cluster in [PROD_US_EAST, PROD_EU_WEST, DEV_KIND] {
        objects.push(prometheus_ready(cluster));
    }
    objects
}

fn payments_inventory() -> Vec<ManagedObject> {
    let ns = Some("payments");
    vec![
        managed(
            Status::Err,
            "Deployment",
            ns,
            "checkout-api",
            "2/3 ready",
            true,
        ),
        managed(
            Status::Ok,
            "Deployment",
            ns,
            "payments-worker",
            "2/2 ready",
            false,
        ),
        managed(
            Status::Ok,
            "StatefulSet",
            ns,
            "payments-db",
            "1/1 ready",
            false,
        ),
        managed(
            Status::Ok,
            "Service",
            ns,
            "checkout-api",
            "ClusterIP",
            false,
        ),
        managed(
            Status::Ok,
            "ConfigMap",
            ns,
            "checkout-api-config-8d7k2",
            "",
            true,
        ),
        managed(
            Status::Ok,
            "ExternalSecret",
            ns,
            "stripe",
            "synced 4m ago",
            false,
        ),
        managed(
            Status::Ok,
            "HorizontalPodAutoscaler",
            ns,
            "checkout-api",
            "3 of 3–12",
            false,
        ),
        managed(
            Status::Ok,
            "PodDisruptionBudget",
            ns,
            "checkout-api",
            "",
            false,
        ),
        managed(Status::Ok, "ServiceAccount", ns, "checkout-api", "", false),
        managed(Status::Ok, "NetworkPolicy", ns, "default-deny", "", false),
        managed(Status::Ok, "Namespace", None, "payments", "", false),
    ]
}

fn kustomization_spec(cluster: &str, path: &str) -> (Vec<SpecRow>, String) {
    let rows = vec![
        SpecRow::new("source", format!("{FLEET_INFRA} · main")),
        SpecRow::new("path", path.trim_start_matches("./")),
        SpecRow::new("every", format!("10m · timeout {HEALTH_TIMEOUT} · prune")),
    ];
    let yaml = format!(
        "  interval: 10m\n  timeout: {HEALTH_TIMEOUT}\n  prune: true\n  path: {path}\n  sourceRef:\n    kind: GitRepository\n    name: {FLEET_INFRA}\n  postBuild:\n    substitute:\n      cluster: {cluster}\n"
    );
    (rows, yaml)
}

fn settled_conditions(phase: Phase, sentence: &Sentence) -> Vec<Condition> {
    let ready = Condition::new(READY, TRUE)
        .reason(SUCCEEDED)
        .message(sentence.plain())
        .age(Some(ago(3 * HOUR)));
    let waiting = Condition::new(READY, UNKNOWN)
        .reason(PROGRESSING)
        .message("Reconciliation in progress")
        .age(Some(ago(MINUTE)));
    let reconciling = Condition::new(RECONCILING, TRUE)
        .reason(PROGRESSING)
        .age(Some(ago(MINUTE)));
    match phase {
        Phase::Ready => vec![ready],
        Phase::Suspended { .. } => vec![ready],
        Phase::Reconciling => vec![reconciling, waiting],
        Phase::Retrying => vec![reconciling, waiting],
        Phase::Failing => vec![waiting],
    }
}

fn explain(object: &mut FluxObject, conditions: Vec<Condition>) {
    object.diagnosis = from_conditions(&conditions);
    object.conditions = conditions;
}

fn payments_path(cluster: &str) -> String {
    format!("./apps/payments/overlays/{cluster}")
}

fn kustomization(
    cluster: &str,
    name: &str,
    path: &str,
    phase: Phase,
    sentence: Sentence,
) -> FluxObject {
    let key = key(cluster, FluxKind::Kustomization, FLUX_SYSTEM, name);
    let (spec, spec_yaml) = kustomization_spec(cluster, path);
    let yaml = object_yaml(
        &key,
        "kustomize.toolkit.fluxcd.io/v1",
        &spec_yaml,
        phase,
        &sentence,
    );
    let conditions = settled_conditions(phase, &sentence);
    FluxObject {
        conditions,
        diagnosis: None,
        labels: BTreeMap::new(),
        uid: Uid::default(),
        source: Some(fleet_infra(cluster)),
        key,
        phase,
        sentence,
        applied: Some(Revision::new(HEAD, ago(3 * HOUR))),
        last_healthy: Some(Revision::new(HEAD, ago(3 * HOUR))),
        depends_on: Vec::new(),
        inventory: Vec::new(),
        history: Vec::new(),
        spec,
        yaml,
        detail: FluxDetail::Kustomization(None),
    }
}

fn payments_dependencies(cluster: &str) -> Vec<FluxKey> {
    [INFRA_CONTROLLERS, INFRA_CONFIGS]
        .map(|name| key(cluster, FluxKind::Kustomization, FLUX_SYSTEM, name))
        .into()
}

fn apps_payments_east() -> FluxObject {
    let sentence = Sentence(vec![
        Span::Object("checkout-api".into()),
        Span::Text(format!(
            " did not become ready within the {HEALTH_TIMEOUT} health check. Retrying in 1m48s."
        )),
    ]);
    let mut object = kustomization(
        PROD_US_EAST,
        APPS_PAYMENTS,
        &payments_path(PROD_US_EAST),
        Phase::Failing,
        sentence,
    );
    let failed = Condition::new(READY, FALSE)
        .reason(HEALTH_CHECK_FAILED)
        .message(PAYMENTS_HEALTH_MESSAGE)
        .age(Some(ago(4 * MINUTE)));
    explain(&mut object, vec![failed]);
    object.last_healthy = Some(Revision::new(PREVIOUS, ago(DAY)));
    object.depends_on = payments_dependencies(PROD_US_EAST);
    object.inventory = payments_inventory();
    object.detail = FluxDetail::Kustomization(Some(payments_drift()));
    object.history = vec![
        entry(
            4 * MINUTE,
            Status::Err,
            HEAD,
            "health check timed out: checkout-api",
            18,
        ),
        entry(
            3 * HOUR,
            Status::Ok,
            HEAD,
            "applied · checkout-api, checkout-api-config configured",
            1,
        ),
        entry(DAY, Status::Ok, PREVIOUS, "applied · no changes", 142),
    ];
    object
}

fn healthy_payments(cluster: &str, healthy_after: &str) -> FluxObject {
    let sentence = Sentence::text(format!(
        "Applied {HEAD} 3h ago; every object was healthy after {healthy_after}."
    ));
    let mut object = kustomization(
        cluster,
        APPS_PAYMENTS,
        &payments_path(cluster),
        Phase::Ready,
        sentence,
    );
    object.depends_on = payments_dependencies(cluster);
    object.inventory = payments_inventory()
        .into_iter()
        .map(|item| ManagedObject {
            status: Status::Ok,
            detail: item.detail.replace("2/3", "3/3"),
            ..item
        })
        .collect();
    object.history = vec![
        entry(
            3 * HOUR,
            Status::Ok,
            HEAD,
            "applied · checkout-api, checkout-api-config configured",
            1,
        ),
        entry(DAY, Status::Ok, PREVIOUS, "applied · no changes", 142),
    ];
    object
}

fn peer_payments() -> Vec<FluxObject> {
    let staging = healthy_payments(STAGING, "48s");
    let dev = healthy_payments(DEV_KIND, "21s");
    let phase = Phase::Suspended {
        since: Some(ago(2 * DAY)),
    };
    let sentence = Sentence::text(
        "Suspended for the change freeze, so Flux applies nothing here. 2 commits behind fleet-infra/main.",
    );
    let mut west = kustomization(
        PROD_EU_WEST,
        APPS_PAYMENTS,
        &payments_path(PROD_EU_WEST),
        phase,
        sentence,
    );
    west.applied = Some(Revision::new(PREVIOUS, ago(DAY)));
    west.last_healthy = Some(Revision::new(PREVIOUS, ago(DAY)));
    west.depends_on = payments_dependencies(PROD_EU_WEST);
    west.inventory = payments_inventory()
        .into_iter()
        .map(|item| ManagedObject {
            status: Status::Ok,
            detail: item.detail.replace("2/3", "3/3"),
            changed: false,
            ..item
        })
        .collect();
    west.history = vec![
        entry(
            2 * DAY,
            Status::Suspended,
            PREVIOUS,
            "suspended · change freeze",
            1,
        ),
        entry(DAY, Status::Ok, PREVIOUS, "applied · no changes", 96),
    ];
    vec![staging, dev, west]
}

fn infra(cluster: &str, name: &str, path: &str) -> FluxObject {
    let sentence = Sentence::text(format!(
        "Applied {HEAD} 3h ago; everything it manages is ready."
    ));
    let mut object = kustomization(cluster, name, path, Phase::Ready, sentence);
    if name == INFRA_CONFIGS {
        object.depends_on = vec![key(
            cluster,
            FluxKind::Kustomization,
            FLUX_SYSTEM,
            INFRA_CONTROLLERS,
        )];
    }
    object.history = vec![entry(
        3 * HOUR,
        Status::Ok,
        HEAD,
        "applied · no changes",
        211,
    )];
    object
}

fn git_source(cluster: &str) -> FluxObject {
    let key = fleet_infra(cluster);
    let phase = Phase::Ready;
    let sentence = Sentence(vec![
        Span::Text("Fetched main at ".into()),
        Span::Revision(HEAD.into()),
        Span::Text(" 4m ago. The next fetch is due in 1m.".into()),
    ]);
    let url = "ssh://git@github.com/acme/fleet-infra";
    let spec = vec![
        SpecRow::new("url", url),
        SpecRow::new("ref", "branch main"),
        SpecRow::new("every", "1m · timeout 60s"),
        SpecRow::new("secret", "fleet-infra-deploy-key"),
    ];
    let spec_yaml = format!(
        "  interval: 1m\n  timeout: 60s\n  url: {url}\n  ref:\n    branch: main\n  secretRef:\n    name: fleet-infra-deploy-key\n"
    );
    let yaml = object_yaml(
        &key,
        "source.toolkit.fluxcd.io/v1",
        &spec_yaml,
        phase,
        &sentence,
    );
    let conditions = settled_conditions(phase, &sentence);
    FluxObject {
        conditions,
        diagnosis: None,
        labels: BTreeMap::new(),
        uid: Uid::default(),
        key,
        phase,
        sentence,
        applied: Some(Revision::new(HEAD, ago(3 * HOUR))),
        last_healthy: None,
        source: None,
        depends_on: Vec::new(),
        inventory: Vec::new(),
        history: vec![
            entry(
                3 * HOUR,
                Status::Ok,
                HEAD,
                &format!("fetched main@sha1:{HEAD_FULL}"),
                1,
            ),
            entry(
                5 * HOUR,
                Status::Ok,
                MONITORING_BUMP,
                &format!("fetched main@sha1:{MONITORING_BUMP_FULL}"),
                1,
            ),
            entry(
                DAY,
                Status::Ok,
                PREVIOUS,
                &format!("fetched main@sha1:{PREVIOUS_FULL}"),
                1,
            ),
        ],
        spec,
        yaml,
        detail: FluxDetail::Source(SourceDetail {
            url: url.into(),
            reference: "main".into(),
            fetched: ago(4 * MINUTE),
        }),
    }
}

fn helm_source(cluster: &str) -> FluxObject {
    let key = prometheus_repo(cluster);
    let phase = Phase::Ready;
    let sentence = Sentence::text("Fetched the chart index 12m ago. The next fetch is due in 18m.");
    let url = "https://prometheus-community.github.io/helm-charts";
    let index = "index 2026-10-12";
    let spec = vec![
        SpecRow::new("url", url),
        SpecRow::new("every", "30m · timeout 60s"),
    ];
    let spec_yaml = format!("  interval: 30m\n  timeout: 60s\n  url: {url}\n");
    let yaml = object_yaml(
        &key,
        "source.toolkit.fluxcd.io/v1",
        &spec_yaml,
        phase,
        &sentence,
    );
    let conditions = settled_conditions(phase, &sentence);
    FluxObject {
        conditions,
        diagnosis: None,
        labels: BTreeMap::new(),
        uid: Uid::default(),
        key,
        phase,
        sentence,
        applied: Some(Revision::new(index, ago(12 * MINUTE))),
        last_healthy: None,
        source: None,
        depends_on: Vec::new(),
        inventory: Vec::new(),
        history: vec![entry(
            12 * MINUTE,
            Status::Ok,
            index,
            "fetched index.yaml",
            48,
        )],
        spec,
        yaml,
        detail: FluxDetail::Source(SourceDetail {
            url: url.into(),
            reference: "—".into(),
            fetched: ago(12 * MINUTE),
        }),
    }
}

fn prometheus_inventory(prometheus: (Status, &str), changed: bool) -> Vec<ManagedObject> {
    let ns = Some(MONITORING);
    let (status, ready) = prometheus;
    vec![
        managed(
            status,
            "StatefulSet",
            ns,
            "prometheus-kube-prometheus-stack-prometheus",
            ready,
            changed,
        ),
        managed(
            Status::Ok,
            "Deployment",
            ns,
            "kube-prometheus-stack-operator",
            "1/1 ready",
            changed,
        ),
        managed(
            Status::Ok,
            "StatefulSet",
            ns,
            "alertmanager-kube-prometheus-stack-alertmanager",
            "1/1 ready",
            false,
        ),
        managed(
            Status::Ok,
            "Deployment",
            ns,
            "kube-prometheus-stack-kube-state-metrics",
            "1/1 ready",
            false,
        ),
        managed(
            Status::Ok,
            "DaemonSet",
            ns,
            "kube-prometheus-stack-prometheus-node-exporter",
            "6/6 ready",
            false,
        ),
        managed(
            Status::Ok,
            "Service",
            ns,
            "kube-prometheus-stack-prometheus",
            "ClusterIP",
            false,
        ),
        managed(
            Status::Ok,
            "ServiceMonitor",
            ns,
            "kube-prometheus-stack-apiserver",
            "",
            false,
        ),
        managed(
            Status::Ok,
            "PrometheusRule",
            ns,
            "kube-prometheus-stack-k8s.rules",
            "",
            changed,
        ),
        managed(
            Status::Ok,
            "CustomResourceDefinition",
            None,
            "prometheuses.monitoring.coreos.com",
            "",
            changed,
        ),
    ]
}

fn prometheus_values() -> Vec<SpecRow> {
    vec![
        SpecRow::new("prometheus.replicas", "2"),
        SpecRow::new("prometheus.retention", "15d"),
        SpecRow::new("alertmanager.enabled", "true"),
        SpecRow::new("grafana.enabled", "false"),
        SpecRow::new("valuesFrom", "monitoring-values"),
    ]
}

fn prometheus_spec(cluster: &str, version: &str) -> (Vec<SpecRow>, String) {
    let rows = vec![
        SpecRow::new("chart", format!("{KUBE_PROMETHEUS_STACK} · {version}")),
        SpecRow::new("source", PROMETHEUS_COMMUNITY),
        SpecRow::new("every", "30m · timeout 10m"),
        SpecRow::new("upgrade", "retries 3 · remediate rollback"),
    ];
    let yaml = format!(
        "  interval: 30m\n  timeout: 10m\n  chart:\n    spec:\n      chart: {KUBE_PROMETHEUS_STACK}\n      version: {version}\n      sourceRef:\n        kind: HelmRepository\n        name: {PROMETHEUS_COMMUNITY}\n        namespace: {FLUX_SYSTEM}\n  upgrade:\n    remediation:\n      retries: 3\n      strategy: rollback\n  valuesFrom:\n  - kind: ConfigMap\n    name: monitoring-values\n  values:\n    cluster: {cluster}\n    prometheus:\n      prometheusSpec:\n        replicas: 2\n        retention: 15d\n    grafana:\n      enabled: false\n"
    );
    (rows, yaml)
}

const UPGRADE_DIFF: &str = "--- a/values.yaml\n+++ b/values.yaml\n@@ -1,10 +1,10 @@\n chart: kube-prometheus-stack\n-version: 61.9.0\n-appVersion: v0.75.2\n+version: 62.7.0\n+appVersion: v0.76.1\n prometheus:\n   prometheusSpec:\n     replicas: 2\n     retention: 15d\n-    scrapeInterval: 30s\n+    scrapeInterval: 15s\n grafana:\n   enabled: false\n";

const PREVIOUS_UPGRADE_DIFF: &str = "--- a/values.yaml\n+++ b/values.yaml\n@@ -1,6 +1,6 @@\n chart: kube-prometheus-stack\n-version: 61.3.2\n-appVersion: v0.75.0\n+version: 61.9.0\n+appVersion: v0.75.2\n prometheus:\n   prometheusSpec:\n     replicas: 2\n";

fn release(
    revision: u32,
    status: Status,
    state: &str,
    chart: (&str, &str),
    description: &str,
    age: u64,
    diff: &str,
) -> HelmRevision {
    let (chart_version, app_version) = chart;
    HelmRevision {
        revision,
        status,
        state: state.into(),
        chart_version: chart_version.into(),
        app_version: app_version.into(),
        description: description.into(),
        age: ago(age),
        diff: diff.into(),
    }
}

fn helm_release(cluster: &str, version: &str, phase: Phase, sentence: Sentence) -> FluxObject {
    let key = key(
        cluster,
        FluxKind::HelmRelease,
        MONITORING,
        KUBE_PROMETHEUS_STACK,
    );
    let (spec, spec_yaml) = prometheus_spec(cluster, version);
    let yaml = object_yaml(
        &key,
        "helm.toolkit.fluxcd.io/v2",
        &spec_yaml,
        phase,
        &sentence,
    );
    let conditions = settled_conditions(phase, &sentence);
    FluxObject {
        conditions,
        diagnosis: None,
        labels: BTreeMap::new(),
        uid: Uid::default(),
        key,
        phase,
        sentence,
        applied: Some(Revision::new(version, ago(5 * DAY))),
        last_healthy: Some(Revision::new(PROMETHEUS_OLD, ago(5 * DAY))),
        source: Some(prometheus_repo(cluster)),
        depends_on: Vec::new(),
        inventory: prometheus_inventory((Status::Ok, "2/2 ready"), false),
        history: Vec::new(),
        spec,
        yaml,
        detail: FluxDetail::HelmRelease(HelmDetail {
            chart: KUBE_PROMETHEUS_STACK.into(),
            version: version.into(),
            values: prometheus_values(),
            releases: vec![
                release(
                    14,
                    Status::Ok,
                    "deployed",
                    (PROMETHEUS_OLD, "v0.75.2"),
                    "Upgrade complete",
                    5 * DAY,
                    PREVIOUS_UPGRADE_DIFF,
                ),
                release(
                    13,
                    Status::Ok,
                    "superseded",
                    ("61.3.2", "v0.75.0"),
                    "Upgrade complete",
                    23 * DAY,
                    "",
                ),
            ],
        }),
    }
}

fn prometheus_ready(cluster: &str) -> FluxObject {
    let sentence = Sentence::text(format!(
        "Revision 14 ({PROMETHEUS_OLD}) is deployed and every object it manages is ready."
    ));
    helm_release(cluster, PROMETHEUS_OLD, Phase::Ready, sentence)
}

fn prometheus_staging() -> FluxObject {
    let sentence = Sentence(vec![
        Span::Text(format!(
            "The upgrade to {PROMETHEUS_NEW} timed out waiting for "
        )),
        Span::Object("prometheus-kube-prometheus-stack-prometheus".into()),
        Span::Text(format!(
            ": context deadline exceeded. Retry 2 of 3 runs in 4m; revision 14 ({PROMETHEUS_OLD}) is still serving."
        )),
    ]);
    let mut object = helm_release(STAGING, PROMETHEUS_NEW, Phase::Retrying, sentence);
    let changed = Some(ago(9 * MINUTE));
    let failed = |kind: &str| {
        Condition::new(kind, FALSE)
            .reason(UPGRADE_FAILED)
            .message(PROMETHEUS_UPGRADE_MESSAGE)
            .age(changed)
    };
    let reconciling = Condition::new(RECONCILING, TRUE)
        .reason("ProgressingWithRetry")
        .message("Running 'upgrade' action with timeout of 5m0s")
        .age(changed);
    explain(
        &mut object,
        vec![failed(READY), failed(RELEASED), reconciling],
    );
    object.applied = Some(Revision::new(PROMETHEUS_NEW, ago(9 * MINUTE)));
    object.inventory = prometheus_inventory((Status::Warn, "1/2 ready"), true);
    if let FluxDetail::HelmRelease(detail) = &mut object.detail {
        let failed = |revision, age, diff: &str| {
            release(
                revision,
                Status::Warn,
                "failed",
                (PROMETHEUS_NEW, "v0.76.1"),
                "Upgrade \"kube-prometheus-stack\" failed: context deadline exceeded",
                age,
                diff,
            )
        };
        let retried = failed(16, 9 * MINUTE, "");
        let first = failed(15, 5 * HOUR, UPGRADE_DIFF);
        detail.releases.insert(0, first);
        detail.releases.insert(0, retried);
    }
    object
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settle(Box<FluxObject>);

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub changed: bool,
    pub settle: Option<Settle>,
}

#[derive(Debug, Default)]
pub struct Flux {
    before_suspend: HashMap<FluxKey, FluxObject>,
}

impl Flux {
    pub fn apply(&mut self, snapshot: &mut Snapshot, key: &FluxKey, action: FluxAction) -> Outcome {
        match action {
            FluxAction::Reconcile => reconcile(snapshot, key, action),
            FluxAction::ReconcileWithSource => reconcile(snapshot, key, action),
            FluxAction::Force => reconcile(snapshot, key, action),
            FluxAction::Suspend => self.suspend(snapshot, key),
            FluxAction::Resume => self.resume(snapshot, key),
        }
    }

    fn suspend(&mut self, snapshot: &mut Snapshot, key: &FluxKey) -> Outcome {
        let Some(before) = snapshot.flux_object(key).cloned() else {
            return Outcome::default();
        };
        let Some(next) = suspend(&before) else {
            return Outcome::default();
        };
        self.before_suspend.insert(key.clone(), before);
        replace(snapshot, next);
        Outcome {
            changed: true,
            settle: None,
        }
    }

    fn resume(&mut self, snapshot: &mut Snapshot, key: &FluxKey) -> Outcome {
        let Some(current) = snapshot.flux_object(key).cloned() else {
            return Outcome::default();
        };
        if !current.phase.is_suspended() {
            return Outcome::default();
        }
        let next = self
            .before_suspend
            .remove(key)
            .unwrap_or_else(|| resumed(&current));
        replace(snapshot, next);
        let reconciled = reconcile(snapshot, key, FluxAction::Reconcile);
        Outcome {
            changed: true,
            settle: reconciled.settle,
        }
    }
}

pub fn settle(snapshot: &mut Snapshot, settle: &Settle) {
    let Settle(before) = settle;
    let still_running = snapshot
        .flux_object(&before.key)
        .is_some_and(|object| object.phase == Phase::Reconciling);
    if still_running {
        replace(snapshot, finish(before));
    }
}

fn replace(snapshot: &mut Snapshot, next: FluxObject) {
    let slot = snapshot.flux.iter_mut().find(|o| o.key == next.key);
    if let Some(slot) = slot {
        *slot = next;
    }
}

fn reconcile(snapshot: &mut Snapshot, key: &FluxKey, action: FluxAction) -> Outcome {
    let Some(before) = snapshot.flux_object(key).cloned() else {
        return Outcome::default();
    };
    let Some(running) = begin(&before, action) else {
        return Outcome::default();
    };
    replace(snapshot, running);
    Outcome {
        changed: true,
        settle: Some(Settle(Box::new(before))),
    }
}

pub fn begin(object: &FluxObject, action: FluxAction) -> Option<FluxObject> {
    if object.phase.is_suspended() {
        return None;
    }
    let message = match action {
        FluxAction::Reconcile => "reconcile requested",
        FluxAction::ReconcileWithSource => "source fetched · reconcile requested",
        FluxAction::Force => "forced apply requested",
        FluxAction::Suspend => return None,
        FluxAction::Resume => return None,
    };
    let revision = object
        .applied
        .as_ref()
        .map(|r| r.id.clone())
        .unwrap_or_default();
    let mut next = object.clone();
    next.phase = Phase::Reconciling;
    next.sentence = reconciling_sentence(object, &revision);
    next.history.insert(
        0,
        HistoryEntry {
            age: None,
            status: Status::Progress,
            revision,
            message: message.into(),
            count: 1,
        },
    );
    Some(next)
}

fn reconciling_sentence(object: &FluxObject, revision: &str) -> Sentence {
    if object.key.kind.is_source() {
        return Sentence::text(format!(
            "Fetching the latest revision; {revision} is current."
        ));
    }
    let waiting = object
        .inventory
        .iter()
        .find(|item| item.status != Status::Ok)
        .map(|item| item.name.clone());
    match waiting {
        Some(name) => Sentence(vec![
            Span::Text(format!(
                "Applying {revision}, then waiting up to {HEALTH_TIMEOUT} for "
            )),
            Span::Object(name),
            Span::Text(" to become ready.".into()),
        ]),
        None => Sentence::text(format!(
            "Applying {revision}, then waiting up to {HEALTH_TIMEOUT} for its objects to become ready."
        )),
    }
}

pub fn finish(before: &FluxObject) -> FluxObject {
    let mut next = before.clone();
    let outcome_ok = before.status() == Status::Ok;
    let revision = before
        .applied
        .as_ref()
        .map(|r| r.id.clone())
        .unwrap_or_default();
    next.sentence = restart_countdown(&before.sentence);
    let top = next.history.first_mut();
    match top {
        Some(top) if !outcome_ok || top.message == NO_CHANGES => {
            top.count += 1;
            top.age = Some(Duration::ZERO);
        }
        Some(_) => next.history.insert(0, no_changes(revision)),
        None => next.history.push(no_changes(revision)),
    }
    next
}

fn restart_countdown(sentence: &Sentence) -> Sentence {
    let spans = sentence.0.iter().map(|span| match span {
        Span::Text(text) => match text.find(RETRY_LEAD) {
            Some(at) => Span::Text(format!("{}{RETRY_LEAD}{RETRY_INTERVAL}.", &text[..at])),
            None => span.clone(),
        },
        Span::Object(_) => span.clone(),
        Span::Revision(_) => span.clone(),
    });
    Sentence(spans.collect())
}

const NO_CHANGES: &str = "applied · no changes";

fn no_changes(revision: String) -> HistoryEntry {
    HistoryEntry {
        age: Some(Duration::ZERO),
        status: Status::Ok,
        revision,
        message: NO_CHANGES.into(),
        count: 1,
    }
}

pub fn suspend(object: &FluxObject) -> Option<FluxObject> {
    if object.phase.is_suspended() {
        return None;
    }
    let mut next = object.clone();
    next.phase = Phase::Suspended {
        since: Some(Duration::ZERO),
    };
    next.sentence = Sentence::text("Suspended, so Flux applies nothing here until it is resumed.");
    Some(next)
}

fn resumed(object: &FluxObject) -> FluxObject {
    let mut next = object.clone();
    next.phase = Phase::Ready;
    next.sentence = Sentence::text("Resumed. Flux applies the latest revision on its next run.");
    next
}

fn payments_drift() -> Drift {
    let changed = DriftedObject {
        state: DriftState::Changed,
        kind: "Deployment".into(),
        namespace: Some("payments".into()),
        name: "checkout-api".into(),
        fields: vec![
            DriftField {
                path: "spec.replicas".into(),
                value: "5".into(),
                manager: "kubectl (scale)".into(),
                age: Some(ago(42 * MINUTE)),
            },
            DriftField {
                path: "spec.template.spec.containers[name=api].env[name=LOG_LEVEL].value".into(),
                value: "debug".into(),
                manager: "kubectl-edit".into(),
                age: Some(ago(3 * HOUR)),
            },
        ],
    };
    let extra = DriftedObject {
        state: DriftState::Extra,
        kind: "ConfigMap".into(),
        namespace: Some("payments".into()),
        name: "checkout-api-flags-old".into(),
        fields: Vec::new(),
    };
    Drift {
        checked: 11,
        objects: vec![changed, extra],
        unreadable: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find<'a>(objects: &'a [FluxObject], key: &FluxKey) -> Option<&'a FluxObject> {
        objects.iter().find(|object| &object.key == key)
    }

    fn payments_east() -> FluxObject {
        apps_payments_east()
    }

    #[test]
    fn design_kustomization_is_failing_on_prod_us_east() {
        let object = payments_east();
        assert_eq!(object.phase, Phase::Failing);
        assert_eq!(object.inventory.len(), 11);
        assert_eq!(
            object.sentence.plain(),
            "checkout-api did not become ready within the 5m health check. Retrying in 1m48s."
        );
        let counts: Vec<_> = object
            .history
            .iter()
            .map(HistoryEntry::count_label)
            .collect();
        assert_eq!(
            counts,
            [Some("18 times".into()), None, Some("142 times".into())]
        );
    }

    #[test]
    fn every_object_has_unique_key_and_resolvable_links() {
        let objects = objects();
        for (ix, object) in objects.iter().enumerate() {
            let duplicates = objects[ix + 1..]
                .iter()
                .filter(|o| o.key == object.key)
                .count();
            assert_eq!(duplicates, 0, "{:?}", object.key);
            let links = object.source.iter().chain(object.depends_on.iter());
            for link in links {
                assert!(find(&objects, link).is_some(), "{link:?}");
            }
        }
    }

    #[test]
    fn reconcile_shows_progress_then_records_the_outcome() {
        let before = payments_east();
        let running = begin(&before, FluxAction::Reconcile);
        let running = running.as_ref();
        assert_eq!(running.map(|o| o.phase), Some(Phase::Reconciling));
        assert_eq!(
            running.map(|o| o.sentence.plain()).as_deref(),
            Some("Applying a41e07c, then waiting up to 5m for checkout-api to become ready.")
        );
        assert_eq!(
            running.map(|o| o.history[0].age_label()).as_deref(),
            Some("now")
        );
        let done = finish(&before);
        assert_eq!(done.phase, Phase::Failing);
        assert_eq!(done.history.len(), before.history.len());
        assert_eq!(done.history[0].count, 19);
        assert_eq!(done.history[0].age, Some(Duration::ZERO));
        assert_eq!(
            done.sentence.plain(),
            "checkout-api did not become ready within the 5m health check. Retrying in 2m."
        );
    }

    #[test]
    fn healthy_reconcile_adds_then_bumps_no_changes() {
        let before = healthy_payments(STAGING, "48s");
        let once = finish(&before);
        assert_eq!(once.history.len(), before.history.len() + 1);
        assert_eq!(once.history[0].message, NO_CHANGES);
        let twice = finish(&once);
        assert_eq!(twice.history.len(), once.history.len());
        assert_eq!(twice.history[0].count, 2);
    }

    #[test]
    fn suspended_objects_ignore_reconcile_and_suspend() {
        let west = &peer_payments()[2];
        assert!(begin(west, FluxAction::Reconcile).is_none());
        assert!(suspend(west).is_none());
        let suspended = suspend(&payments_east());
        assert_eq!(
            suspended.map(|o| o.phase),
            Some(Phase::Suspended {
                since: Some(Duration::ZERO)
            })
        );
    }

    #[test]
    fn suspend_then_resume_restores_and_reconciles() {
        let mut snapshot = Snapshot {
            flux: vec![payments_east()],
            ..Snapshot::default()
        };
        let mut flux = Flux::default();
        let key = payments_east().key;
        let suspended = flux.apply(&mut snapshot, &key, FluxAction::Suspend);
        assert!(suspended.changed);
        assert!(snapshot.flux[0].phase.is_suspended());
        let resumed = flux.apply(&mut snapshot, &key, FluxAction::Resume);
        assert_eq!(snapshot.flux[0].phase, Phase::Reconciling);
        let pending = resumed.settle;
        assert!(pending.is_some());
        if let Some(pending) = pending {
            settle(&mut snapshot, &pending);
        }
        assert_eq!(snapshot.flux[0].phase, Phase::Failing);
        assert_eq!(snapshot.flux[0].history[0].count, 19);
    }

    #[test]
    fn staging_release_is_retrying_its_upgrade() {
        let objects = objects();
        let hr = key(
            STAGING,
            FluxKind::HelmRelease,
            MONITORING,
            KUBE_PROMETHEUS_STACK,
        );
        let object = find(&objects, &hr);
        assert_eq!(object.map(FluxObject::status), Some(Status::Warn));
        let Some(FluxDetail::HelmRelease(detail)) = object.map(|o| &o.detail) else {
            panic!("staging kube-prometheus-stack is a HelmRelease");
        };
        let revisions: Vec<_> = detail.releases.iter().map(|r| r.revision).collect();
        assert_eq!(revisions, [16, 15, 14, 13]);
        assert_eq!(
            detail.deployed().map(|r| r.chart_version.as_str()),
            Some("61.9.0")
        );
    }
}
