use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use flock_core::crd::{Crd, CrdKey, CustomObjects, PrinterColumn};
use flock_core::diagnosis::{self, Condition};
use flock_core::flux::FluxKind;
use flock_core::resources::Resource;
use flock_core::{ClusterId, Status, Uid};

use crate::detail::{days, hours, minutes};

pub const CERT_MANAGER: &str = "cert-manager.io";
pub const CERTIFICATE: &str = "Certificate";
const GATEWAY_API: &str = "gateway.networking.k8s.io";
const MONITORING: &str = "monitoring.coreos.com";
const NOTIFICATION: &str = "notification.toolkit.fluxcd.io";
const HELM_RELEASE_VERSION: &str = "v2";
const ALERT_VERSION: &str = "v1beta3";
const READY: &str = "Ready";
const ACCEPTED: &str = "Accepted";
const PROGRAMMED: &str = "Programmed";
const TRUE: &str = "True";
const FALSE: &str = "False";

fn column(name: &str, json_path: &str) -> PrinterColumn {
    PrinterColumn {
        name: name.into(),
        json_path: json_path.into(),
        date: false,
    }
}

fn ready_column(name: &str, kind: &str) -> PrinterColumn {
    column(
        name,
        &format!(".status.conditions[?(@.type==\"{kind}\")].status"),
    )
}

fn crd(
    group: &str,
    kind: &str,
    plural: &str,
    namespaced: bool,
    columns: Vec<PrinterColumn>,
) -> Crd {
    Crd {
        key: CrdKey::new(group, kind),
        plural: plural.into(),
        version: "v1".into(),
        namespaced,
        columns,
    }
}

fn flux(kind: FluxKind, plural: &str) -> Crd {
    crd(kind.group(), kind.label(), plural, true, Vec::new())
}

fn versioned(crd: Crd, version: &str) -> Crd {
    Crd {
        version: version.into(),
        ..crd
    }
}

fn flux_definitions() -> Vec<Crd> {
    vec![
        flux(FluxKind::Kustomization, "kustomizations"),
        versioned(
            flux(FluxKind::HelmRelease, "helmreleases"),
            HELM_RELEASE_VERSION,
        ),
        flux(FluxKind::GitRepository, "gitrepositories"),
        flux(FluxKind::HelmRepository, "helmrepositories"),
        flux(FluxKind::OciRepository, "ocirepositories"),
        crd(
            FluxKind::GitRepository.group(),
            "HelmChart",
            "helmcharts",
            true,
            Vec::new(),
        ),
        versioned(
            crd(NOTIFICATION, "Alert", "alerts", true, Vec::new()),
            ALERT_VERSION,
        ),
    ]
}

#[cfg(feature = "loco")]
fn loco_definitions() -> Vec<Crd> {
    let group = flock_core::loco::GROUP;
    vec![
        crd(group, "Application", "applications", true, Vec::new()),
        crd(group, "Build", "builds", true, Vec::new()),
    ]
}

#[cfg(not(feature = "loco"))]
const fn loco_definitions() -> Vec<Crd> {
    Vec::new()
}

pub fn definitions() -> Vec<Crd> {
    let mut definitions = third_party_definitions();
    definitions.extend(flux_definitions());
    definitions.extend(loco_definitions());
    definitions
}

fn third_party_definitions() -> Vec<Crd> {
    vec![
        crd(
            CERT_MANAGER,
            CERTIFICATE,
            "certificates",
            true,
            vec![
                ready_column(READY, READY),
                column("Secret", ".spec.secretName"),
            ],
        ),
        crd(
            CERT_MANAGER,
            "ClusterIssuer",
            "clusterissuers",
            false,
            vec![ready_column(READY, READY)],
        ),
        crd(
            CERT_MANAGER,
            "Issuer",
            "issuers",
            true,
            vec![ready_column(READY, READY)],
        ),
        crd(
            GATEWAY_API,
            "Gateway",
            "gateways",
            true,
            vec![
                column("Class", ".spec.gatewayClassName"),
                column("Address", ".status.addresses[*].value"),
                ready_column(PROGRAMMED, PROGRAMMED),
            ],
        ),
        crd(
            GATEWAY_API,
            "GatewayClass",
            "gatewayclasses",
            false,
            vec![
                column("Controller", ".spec.controllerName"),
                ready_column(ACCEPTED, ACCEPTED),
            ],
        ),
        crd(
            GATEWAY_API,
            "HTTPRoute",
            "httproutes",
            true,
            vec![column("Hostnames", ".spec.hostnames")],
        ),
        crd(
            MONITORING,
            "PrometheusRule",
            "prometheusrules",
            true,
            Vec::new(),
        ),
        crd(
            MONITORING,
            "ServiceMonitor",
            "servicemonitors",
            true,
            Vec::new(),
        ),
    ]
}

struct Seed {
    namespace: &'static str,
    name: &'static str,
    cells: &'static [&'static str],
    conditions: Vec<Condition>,
    age: Duration,
    spec: &'static str,
}

fn ready(kind: &str) -> Condition {
    Condition::new(kind, TRUE).reason(kind).age(Some(days(3)))
}

fn yaml(key: &CrdKey, seed: &Seed) -> String {
    let namespace = if seed.namespace.is_empty() {
        String::new()
    } else {
        format!("  namespace: {}\n", seed.namespace)
    };
    let mut conditions = String::new();
    for c in &seed.conditions {
        let _ = write!(
            conditions,
            "  - type: {}\n    status: \"{}\"\n    reason: {}\n",
            c.kind, c.value, c.reason
        );
    }
    format!(
        "apiVersion: {}/v1\nkind: {}\nmetadata:\n  name: {}\n{namespace}spec:\n{}status:\n  conditions:\n{conditions}",
        key.group, key.kind, seed.name, seed.spec
    )
}

fn row(key: &CrdKey, seed: Seed) -> Resource {
    let status = Status::worst(seed.conditions.iter().map(Condition::status));
    let diagnosis =
        diagnosis::from_conditions(&seed.conditions).filter(|_| diagnosis::needs_diagnosis(status));
    Resource {
        yaml: yaml(key, &seed),
        namespace: seed.namespace.into(),
        name: seed.name.into(),
        uid: Uid::new(format!("{}-{}", key.kind.to_lowercase(), seed.name)),
        status,
        cells: seed.cells.iter().map(|cell| (*cell).to_owned()).collect(),
        age: seed.age,
        labels: BTreeMap::new(),
        conditions: seed.conditions,
        diagnosis,
    }
}

fn certificates() -> Vec<Seed> {
    vec![
        Seed {
            namespace: "payments",
            name: "checkout-tls",
            cells: &[TRUE, "checkout-tls"],
            conditions: vec![ready(READY)],
            age: days(41),
            spec: "  secretName: checkout-tls\n  dnsNames:\n  - checkout.acme.dev\n  issuerRef:\n    kind: ClusterIssuer\n    name: letsencrypt\n",
        },
        Seed {
            namespace: "web",
            name: "web-frontend-tls",
            cells: &[FALSE, "web-frontend-tls"],
            conditions: vec![
                Condition::new(READY, FALSE)
                    .reason("Failed")
                    .message("The certificate request has failed to complete and will be retried: ACME order is invalid")
                    .age(Some(minutes(12))),
                Condition::new("Issuing", FALSE)
                    .reason("Failed")
                    .message("The certificate request has failed to complete and will be retried")
                    .age(Some(minutes(12))),
            ],
            age: days(9),
            spec: "  secretName: web-frontend-tls\n  dnsNames:\n  - www.acme.dev\n  issuerRef:\n    kind: ClusterIssuer\n    name: letsencrypt\n",
        },
        Seed {
            namespace: "monitoring",
            name: "grafana-tls",
            cells: &[TRUE, "grafana-tls"],
            conditions: vec![ready(READY)],
            age: days(120),
            spec: "  secretName: grafana-tls\n  dnsNames:\n  - grafana.acme.dev\n  issuerRef:\n    kind: ClusterIssuer\n    name: letsencrypt\n",
        },
        Seed {
            namespace: "payments",
            name: "payments-db-tls",
            cells: &["Unknown", "payments-db-tls"],
            conditions: vec![
                Condition::new(READY, "Unknown")
                    .reason("Pending")
                    .message("Issuing certificate as Secret does not exist")
                    .age(Some(minutes(3))),
            ],
            age: minutes(3),
            spec: "  secretName: payments-db-tls\n  issuerRef:\n    kind: Issuer\n    name: payments-ca\n",
        },
    ]
}

fn issuers() -> Vec<Seed> {
    vec![Seed {
        namespace: "",
        name: "letsencrypt",
        cells: &[TRUE],
        conditions: vec![ready(READY)],
        age: days(214),
        spec: "  acme:\n    server: https://acme-v02.api.letsencrypt.org/directory\n",
    }]
}

fn gateways() -> Vec<Seed> {
    vec![Seed {
        namespace: "edge",
        name: "public",
        cells: &["eg", "203.0.113.24", TRUE],
        conditions: vec![ready(ACCEPTED), ready(PROGRAMMED)],
        age: days(60),
        spec: "  gatewayClassName: eg\n  listeners:\n  - name: https\n    port: 443\n    protocol: HTTPS\n",
    }]
}

fn routes() -> Vec<Seed> {
    vec![
        Seed {
            namespace: "web",
            name: "web-frontend",
            cells: &["[\"www.acme.dev\"]"],
            conditions: Vec::new(),
            age: days(30),
            spec: "  hostnames:\n  - www.acme.dev\n",
        },
        Seed {
            namespace: "payments",
            name: "checkout-api",
            cells: &["[\"checkout.acme.dev\"]"],
            conditions: Vec::new(),
            age: hours(20),
            spec: "  hostnames:\n  - checkout.acme.dev\n",
        },
    ]
}

fn seeds(key: &CrdKey) -> Vec<Seed> {
    match (key.group.as_str(), key.kind.as_str()) {
        (CERT_MANAGER, CERTIFICATE) => certificates(),
        (CERT_MANAGER, "ClusterIssuer") => issuers(),
        (GATEWAY_API, "Gateway") => gateways(),
        (GATEWAY_API, "HTTPRoute") => routes(),
        _ => Vec::new(),
    }
}

pub fn objects(cluster: &ClusterId) -> Vec<CustomObjects> {
    definitions()
        .into_iter()
        .filter(|crd| crd.key.page().is_none())
        .map(|crd| CustomObjects {
            cluster: cluster.clone(),
            rows: seeds(&crd.key)
                .into_iter()
                .map(|seed| row(&crd.key, seed))
                .collect(),
            key: crd.key,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_demo_row_has_one_cell_per_printer_column() {
        let cluster = ClusterId::new("prod-us-east");
        let definitions = definitions();
        for objects in objects(&cluster) {
            let Some(crd) = definitions.iter().find(|crd| crd.key == objects.key) else {
                panic!("{:?} has no definition", objects.key);
            };
            for row in &objects.rows {
                assert_eq!(row.cells.len(), crd.columns.len(), "{}", row.name);
                assert_eq!(row.namespace.is_empty(), !crd.namespaced, "{}", row.name);
            }
        }
        let failing = objects(&cluster)
            .into_iter()
            .flat_map(|objects| objects.rows)
            .filter(|row| row.status == Status::Err)
            .count();
        assert_eq!(failing, 1);
    }
}
