use flock_core::flux::HelmRevision;
use flock_core::helm::{HelmKey, HelmRelease, HelmReleaseDetail};
use flock_core::{ClusterId, FluxKey, FluxKind, Status};

use crate::detail::{days, hours, minutes};

const MONITORING: &str = "monitoring";
const PROMETHEUS: &str = "kube-prometheus-stack";
const DEPLOYED: &str = "deployed";
const SUPERSEDED: &str = "superseded";
const FAILED: &str = "failed";
const PROMETHEUS_VALUES: &str = "grafana:\n  adminPassword: ••••••\n  ingress:\n    enabled: true\n    hosts:\n    - grafana.acme.dev\nprometheus:\n  prometheusSpec:\n    retention: 15d\n";
const PROMETHEUS_COMPUTED: &str = "alertmanager:\n  enabled: true\ngrafana:\n  adminPassword: ••••••\n  enabled: true\n  ingress:\n    enabled: true\n    hosts:\n    - grafana.acme.dev\nprometheus:\n  prometheusSpec:\n    replicas: 1\n    retention: 15d\n    scrapeInterval: 30s\n";
const PROMETHEUS_MANIFEST: &str = "---\n# Source: kube-prometheus-stack/charts/grafana/templates/secret.yaml\napiVersion: v1\nkind: Secret\nmetadata:\n  name: kube-prometheus-stack-grafana\n  namespace: monitoring\ndata:\n  admin-password: ••••••\n---\n# Source: kube-prometheus-stack/charts/grafana/templates/service.yaml\napiVersion: v1\nkind: Service\nmetadata:\n  name: kube-prometheus-stack-grafana\n  namespace: monitoring\nspec:\n  ports:\n  - name: http-web\n    port: 80\n    targetPort: 3000\n";
const NGINX_VALUES: &str = "controller:\n  replicaCount: 2\n  service:\n    type: LoadBalancer\n";
const NGINX_COMPUTED: &str = "controller:\n  image:\n    tag: v1.11.2\n  replicaCount: 2\n  service:\n    type: LoadBalancer\ndefaultBackend:\n  enabled: false\n";
const NGINX_MANIFEST: &str = "---\n# Source: ingress-nginx/templates/controller-service.yaml\napiVersion: v1\nkind: Service\nmetadata:\n  name: ingress-nginx-controller\n  namespace: ingress-nginx\nspec:\n  type: LoadBalancer\n";
const AIRFLOW_VALUES: &str =
    "executor: KubernetesExecutor\nwebserver:\n  defaultUser:\n    password: ••••••\n";
const AIRFLOW_MANIFEST: &str = "---\n# Source: airflow/templates/webserver/webserver-deployment.yaml\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: airflow-webserver\n  namespace: data-pipeline\n";

struct Seed {
    namespace: &'static str,
    name: &'static str,
    chart: &'static str,
    app_version: &'static str,
    flux: bool,
    history: Vec<HelmRevision>,
    values: (&'static str, &'static str),
    manifest: &'static str,
}

fn revision(
    number: u32,
    state: &str,
    chart_version: &str,
    description: &str,
    age: std::time::Duration,
) -> HelmRevision {
    HelmRevision {
        revision: number,
        status: if state == FAILED {
            Status::Err
        } else {
            Status::Ok
        },
        state: state.into(),
        chart_version: chart_version.into(),
        app_version: String::new(),
        description: description.into(),
        age,
        diff: String::new(),
    }
}

fn seeds() -> Vec<Seed> {
    vec![
        Seed {
            namespace: MONITORING,
            name: PROMETHEUS,
            chart: PROMETHEUS,
            app_version: "v0.75.2",
            flux: true,
            history: vec![
                revision(14, DEPLOYED, "61.9.0", "Upgrade complete", days(5)),
                revision(13, SUPERSEDED, "61.3.2", "Upgrade complete", days(23)),
                revision(12, SUPERSEDED, "61.3.2", "Upgrade complete", days(41)),
            ],
            values: (PROMETHEUS_VALUES, PROMETHEUS_COMPUTED),
            manifest: PROMETHEUS_MANIFEST,
        },
        Seed {
            namespace: "ingress-nginx",
            name: "ingress-nginx",
            chart: "ingress-nginx",
            app_version: "1.11.2",
            flux: false,
            history: vec![
                revision(3, DEPLOYED, "4.11.2", "Upgrade complete", days(12)),
                revision(2, SUPERSEDED, "4.11.1", "Upgrade complete", days(60)),
                revision(1, SUPERSEDED, "4.10.0", "Install complete", days(214)),
            ],
            values: (NGINX_VALUES, NGINX_COMPUTED),
            manifest: NGINX_MANIFEST,
        },
        Seed {
            namespace: "data-pipeline",
            name: "airflow",
            chart: "airflow",
            app_version: "2.9.3",
            flux: false,
            history: vec![
                revision(
                    5,
                    FAILED,
                    "1.15.0",
                    "Upgrade \"airflow\" failed: context deadline exceeded",
                    minutes(40),
                ),
                revision(4, DEPLOYED, "1.14.0", "Upgrade complete", hours(30)),
            ],
            values: (AIRFLOW_VALUES, AIRFLOW_VALUES),
            manifest: AIRFLOW_MANIFEST,
        },
    ]
}

fn key(cluster: &ClusterId, seed: &Seed) -> HelmKey {
    HelmKey::new(cluster.clone(), seed.namespace, seed.name)
}

pub fn releases(cluster: &ClusterId) -> Vec<HelmRelease> {
    seeds()
        .iter()
        .filter_map(|seed| {
            let latest = seed.history.first()?;
            let flux = seed.flux.then(|| {
                FluxKey::new(
                    cluster.as_str(),
                    FluxKind::HelmRelease,
                    seed.namespace,
                    seed.name,
                )
            });
            Some(HelmRelease {
                key: key(cluster, seed),
                chart: seed.chart.into(),
                chart_version: latest.chart_version.clone(),
                app_version: seed.app_version.into(),
                revision: latest.revision,
                status: latest.status,
                state: latest.state.clone(),
                updated: Some(latest.age),
                flux,
            })
        })
        .collect()
}

pub fn details(cluster: &ClusterId) -> Vec<HelmReleaseDetail> {
    seeds()
        .into_iter()
        .map(|seed| HelmReleaseDetail {
            key: key(cluster, &seed),
            user_values: seed.values.0.into(),
            computed_values: seed.values.1.into(),
            manifest: seed.manifest.into(),
            history: seed.history,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_demo_release_has_a_detail_and_one_has_failed() {
        let cluster = ClusterId::new("prod-us-east");
        let releases = releases(&cluster);
        let details = details(&cluster);
        assert_eq!(releases.len(), details.len());
        for release in &releases {
            assert!(details.iter().any(|d| d.key == release.key));
        }
        let failed = releases.iter().filter(|r| r.status == Status::Err).count();
        assert_eq!(failed, 1);
        assert_eq!(releases.iter().filter(|r| r.flux.is_some()).count(), 1);
    }
}
