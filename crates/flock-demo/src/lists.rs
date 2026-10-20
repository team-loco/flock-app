use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use flock_core::diagnosis::from_conditions;
use flock_core::resources::{Resource, ResourceKind};
use flock_core::time::age;
use flock_core::workload::PROGRESS_DEADLINE;
use flock_core::{Condition, Diagnosis, Status, Uid};

use crate::detail::{days, hours, minutes};

const STORAGE_CLASS: &str = "gp3";
const INGRESS_CLASS: &str = "nginx";
const INGRESS_ADDRESS: &str =
    "k8s-ingressn-ingressn-5c1d0e7a1f-2f8e6b1c94d7a3e0.elb.us-east-1.amazonaws.com";
const SERVICE_SUBNET: &str = "172.20";
const SUBNET_OCTET: usize = 256;
const SERVICE_OFFSET: usize = 10;
const JOB_ID_BASE: u64 = 28_771_020;
const JOB_ID_STRIDE: u64 = 37;
const HTTPS_PORT: u16 = 443;
const HTTP_PORT: u16 = 80;
const API_SERVER_PORT: u16 = 443;
const NODE_EXPORTER_PORT: u16 = 9100;
const PROMETHEUS_PORT: u16 = 9090;
const WEBHOOK_PORT: u16 = 443;
const CLUSTER_AGE_DAYS: u64 = 214;
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0100_0000_01b3;
const UID_GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
const TRUE: &str = "True";
const FALSE: &str = "False";
const DEADLINE_MESSAGE: &str = "Job was active longer than specified deadline";

struct Deploy {
    ns: &'static str,
    name: &'static str,
    image: &'static str,
    replicas: u32,
    ready: u32,
    status: Status,
    port: u16,
    age_days: u64,
}

const fn deploy(
    ns: &'static str,
    name: &'static str,
    image: &'static str,
    replicas: u32,
    port: u16,
) -> Deploy {
    Deploy {
        ns,
        name,
        image,
        replicas,
        ready: replicas,
        status: Status::Ok,
        port,
        age_days: CLUSTER_AGE_DAYS,
    }
}

const DEPLOYMENTS: [Deploy; 48] = [
    Deploy {
        ready: 2,
        status: Status::Err,
        ..deploy(
            "payments",
            "checkout-api",
            "ghcr.io/acme/checkout-api:2.14.0",
            3,
            8080,
        )
    },
    deploy(
        "payments",
        "payments-worker",
        "ghcr.io/acme/payments-worker:1.9.3",
        2,
        8080,
    ),
    deploy(
        "payments",
        "payments-gateway",
        "ghcr.io/acme/payments-gateway:3.4.0",
        3,
        8080,
    ),
    deploy(
        "payments",
        "fraud-scorer",
        "ghcr.io/acme/fraud-scorer:0.12.1",
        2,
        8080,
    ),
    deploy(
        "orders",
        "orders-api",
        "ghcr.io/acme/orders-api:4.8.2",
        4,
        8080,
    ),
    deploy(
        "orders",
        "orders-worker",
        "ghcr.io/acme/orders-api:4.8.2",
        3,
        8080,
    ),
    deploy(
        "orders",
        "cart-service",
        "ghcr.io/acme/cart-service:2.2.0",
        3,
        8080,
    ),
    deploy(
        "inventory",
        "inventory-api",
        "ghcr.io/acme/inventory-api:1.21.0",
        3,
        8080,
    ),
    deploy(
        "inventory",
        "stock-sync",
        "ghcr.io/acme/inventory-api:1.21.0",
        1,
        8080,
    ),
    deploy(
        "inventory",
        "warehouse-adapter",
        "ghcr.io/acme/warehouse-adapter:0.9.4",
        2,
        8080,
    ),
    deploy(
        "web",
        "web-frontend",
        "ghcr.io/acme/web-frontend:5.2.1",
        6,
        3000,
    ),
    deploy("web", "web-bff", "ghcr.io/acme/web-bff:5.2.1", 4, 8080),
    deploy(
        "web",
        "assets-purger",
        "ghcr.io/acme/assets-purger:1.0.3",
        1,
        8080,
    ),
    deploy(
        "search",
        "search-api",
        "ghcr.io/acme/search-api:0.18.0",
        3,
        8080,
    ),
    deploy(
        "search",
        "query-suggest",
        "ghcr.io/acme/query-suggest:0.7.2",
        2,
        8080,
    ),
    Deploy {
        age_days: 97,
        ..deploy(
            "ml",
            "model-server",
            "ghcr.io/acme/model-server:2.3.0",
            2,
            8501,
        )
    },
    Deploy {
        age_days: 97,
        ..deploy(
            "ml",
            "feature-store-api",
            "ghcr.io/acme/feature-store-api:1.4.1",
            2,
            8080,
        )
    },
    deploy("auth", "auth-api", "ghcr.io/acme/auth-api:6.1.0", 3, 8080),
    deploy(
        "auth",
        "session-service",
        "ghcr.io/acme/auth-api:6.1.0",
        2,
        8080,
    ),
    deploy(
        "notifications",
        "notify-api",
        "ghcr.io/acme/notify-api:2.0.5",
        2,
        8080,
    ),
    deploy(
        "notifications",
        "email-sender",
        "ghcr.io/acme/notify-api:2.0.5",
        2,
        8080,
    ),
    deploy(
        "notifications",
        "push-gateway",
        "ghcr.io/acme/push-gateway:1.3.0",
        2,
        8080,
    ),
    deploy(
        "analytics",
        "events-collector",
        "ghcr.io/acme/events-collector:3.9.0",
        4,
        8080,
    ),
    deploy(
        "analytics",
        "dashboards-api",
        "ghcr.io/acme/dashboards-api:3.9.0",
        2,
        8080,
    ),
    Deploy {
        age_days: 180,
        ..deploy(
            "data-pipeline",
            "ingest-router",
            "ghcr.io/acme/ingest-router:1.15.2",
            3,
            8080,
        )
    },
    Deploy {
        age_days: 180,
        ..deploy(
            "data-pipeline",
            "transform-worker",
            "ghcr.io/acme/transform-worker:1.15.2",
            6,
            8080,
        )
    },
    Deploy {
        age_days: 97,
        ..deploy(
            "feature-flags",
            "flagd",
            "ghcr.io/open-feature/flagd:v0.11.1",
            2,
            8013,
        )
    },
    Deploy {
        ready: 0,
        status: Status::Warn,
        ..deploy("monitoring", "grafana", "grafana/grafana:11.2.0", 1, 3000)
    },
    deploy(
        "monitoring",
        "kube-state-metrics",
        "registry.k8s.io/kube-state-metrics/kube-state-metrics:v2.13.0",
        1,
        8080,
    ),
    deploy(
        "monitoring",
        "prometheus-operator",
        "quay.io/prometheus-operator/prometheus-operator:v0.76.1",
        1,
        8080,
    ),
    deploy(
        "logging",
        "loki-gateway",
        "nginxinc/nginx-unprivileged:1.27-alpine",
        2,
        8080,
    ),
    deploy(
        "logging",
        "vector-aggregator",
        "timberio/vector:0.41.1-distroless-libc",
        2,
        6000,
    ),
    deploy(
        "kube-system",
        "coredns",
        "602401143452.dkr.ecr.us-east-1.amazonaws.com/eks/coredns:v1.11.3-eksbuild.1",
        2,
        53,
    ),
    deploy(
        "kube-system",
        "metrics-server",
        "registry.k8s.io/metrics-server/metrics-server:v0.7.2",
        1,
        443,
    ),
    deploy(
        "kube-system",
        "aws-load-balancer-controller",
        "public.ecr.aws/eks/aws-load-balancer-controller:v2.8.3",
        2,
        9443,
    ),
    deploy(
        "kube-system",
        "cluster-autoscaler",
        "registry.k8s.io/autoscaling/cluster-autoscaler:v1.31.0",
        1,
        8085,
    ),
    deploy(
        "kube-system",
        "ebs-csi-controller",
        "public.ecr.aws/ebs-csi-driver/aws-ebs-csi-driver:v1.34.0",
        2,
        9808,
    ),
    deploy(
        "flux-system",
        "source-controller",
        "ghcr.io/fluxcd/source-controller:v1.4.1",
        1,
        9090,
    ),
    deploy(
        "flux-system",
        "kustomize-controller",
        "ghcr.io/fluxcd/kustomize-controller:v1.4.0",
        1,
        8080,
    ),
    deploy(
        "flux-system",
        "helm-controller",
        "ghcr.io/fluxcd/helm-controller:v1.1.0",
        1,
        8080,
    ),
    deploy(
        "flux-system",
        "notification-controller",
        "ghcr.io/fluxcd/notification-controller:v1.4.0",
        1,
        9292,
    ),
    deploy(
        "cert-manager",
        "cert-manager",
        "quay.io/jetstack/cert-manager-controller:v1.15.3",
        1,
        9402,
    ),
    deploy(
        "cert-manager",
        "cert-manager-webhook",
        "quay.io/jetstack/cert-manager-webhook:v1.15.3",
        1,
        10250,
    ),
    deploy(
        "cert-manager",
        "cert-manager-cainjector",
        "quay.io/jetstack/cert-manager-cainjector:v1.15.3",
        1,
        9402,
    ),
    deploy(
        "ingress-nginx",
        "ingress-nginx-controller",
        "registry.k8s.io/ingress-nginx/controller:v1.11.2",
        3,
        80,
    ),
    Deploy {
        age_days: 61,
        ..deploy(
            "envoy-gateway-system",
            "envoy-gateway",
            "docker.io/envoyproxy/gateway:v1.1.1",
            1,
            18000,
        )
    },
    Deploy {
        age_days: 61,
        ..deploy(
            "loco-system",
            "loco-controller",
            "ghcr.io/loco/loco-controller:0.31.0",
            1,
            8080,
        )
    },
    Deploy {
        age_days: 61,
        ..deploy(
            "loco-system",
            "loco-build-controller",
            "ghcr.io/loco/loco-build-controller:0.31.0",
            1,
            8080,
        )
    },
];

const APP_NAMESPACES: [&str; 13] = [
    "payments",
    "orders",
    "inventory",
    "web",
    "search",
    "ml",
    "auth",
    "notifications",
    "analytics",
    "data-pipeline",
    "feature-flags",
    "monitoring",
    "logging",
];

const PULL_SECRET_NAMESPACES: [&str; 9] = [
    "payments",
    "orders",
    "inventory",
    "web",
    "search",
    "ml",
    "auth",
    "notifications",
    "analytics",
];

const NAMESPACES: [&str; 23] = [
    "analytics",
    "auth",
    "cert-manager",
    "data-pipeline",
    "default",
    "envoy-gateway-system",
    "feature-flags",
    "flux-system",
    "ingress-nginx",
    "inventory",
    "kube-node-lease",
    "kube-public",
    "kube-system",
    "logging",
    "loco-builds",
    "loco-system",
    "ml",
    "monitoring",
    "notifications",
    "orders",
    "payments",
    "search",
    "web",
];

struct Stateful {
    ns: &'static str,
    name: &'static str,
    image: &'static str,
    replicas: u32,
    ready: u32,
    status: Status,
    port: u16,
    volume: Option<(&'static str, &'static str)>,
}

const fn stateful(
    ns: &'static str,
    name: &'static str,
    image: &'static str,
    replicas: u32,
    port: u16,
    volume: Option<(&'static str, &'static str)>,
) -> Stateful {
    Stateful {
        ns,
        name,
        image,
        replicas,
        ready: replicas,
        status: Status::Ok,
        port,
        volume,
    }
}

const STATEFULSETS: [Stateful; 12] = [
    stateful(
        "payments",
        "payments-db",
        "postgres:16.4",
        1,
        5432,
        Some(("data", "50Gi")),
    ),
    Stateful {
        ready: 0,
        status: Status::Warn,
        ..stateful(
            "search",
            "search-indexer",
            "ghcr.io/acme/search-indexer:0.18.0",
            1,
            9200,
            Some(("data", "200Gi")),
        )
    },
    stateful(
        "monitoring",
        "prometheus-k8s",
        "quay.io/prometheus/prometheus:v2.54.1",
        1,
        9090,
        Some(("prometheus-k8s-db", "100Gi")),
    ),
    stateful(
        "monitoring",
        "alertmanager-main",
        "quay.io/prometheus/alertmanager:v0.27.0",
        3,
        9093,
        Some(("alertmanager-main-db", "10Gi")),
    ),
    stateful(
        "logging",
        "loki",
        "grafana/loki:3.1.1",
        3,
        3100,
        Some(("storage", "50Gi")),
    ),
    stateful(
        "data-pipeline",
        "kafka",
        "bitnami/kafka:3.8.0",
        3,
        9092,
        Some(("data", "500Gi")),
    ),
    stateful(
        "data-pipeline",
        "zookeeper",
        "bitnami/zookeeper:3.9.2",
        3,
        2181,
        Some(("data", "10Gi")),
    ),
    stateful(
        "orders",
        "orders-db",
        "postgres:16.4",
        1,
        5432,
        Some(("data", "50Gi")),
    ),
    stateful("inventory", "redis", "redis:7.4.0", 1, 6379, None),
    stateful(
        "analytics",
        "clickhouse",
        "clickhouse/clickhouse-server:24.8",
        2,
        8123,
        Some(("data", "300Gi")),
    ),
    stateful(
        "auth",
        "auth-db",
        "postgres:16.4",
        1,
        5432,
        Some(("data", "20Gi")),
    ),
    stateful(
        "notifications",
        "rabbitmq",
        "rabbitmq:3.13-management",
        3,
        5672,
        None,
    ),
];

struct Daemon {
    ns: &'static str,
    name: &'static str,
    image: &'static str,
    desired: u32,
    selector: &'static str,
}

const ALL_NODES: u32 = 14;
const GPU_NODES: u32 = 2;
const LINUX: &str = "kubernetes.io/os=linux";
const GPU_GROUP: &str = "eks.amazonaws.com/nodegroup=gpu";
const NO_SELECTOR: &str = "—";

const DAEMONSETS: [Daemon; 9] = [
    Daemon {
        ns: "kube-system",
        name: "aws-node",
        image: "602401143452.dkr.ecr.us-east-1.amazonaws.com/amazon-k8s-cni:v1.18.3",
        desired: ALL_NODES,
        selector: NO_SELECTOR,
    },
    Daemon {
        ns: "kube-system",
        name: "kube-proxy",
        image: "602401143452.dkr.ecr.us-east-1.amazonaws.com/eks/kube-proxy:v1.31.0-minimal-eksbuild.2",
        desired: ALL_NODES,
        selector: NO_SELECTOR,
    },
    Daemon {
        ns: "kube-system",
        name: "ebs-csi-node",
        image: "public.ecr.aws/ebs-csi-driver/aws-ebs-csi-driver:v1.34.0",
        desired: ALL_NODES,
        selector: LINUX,
    },
    Daemon {
        ns: "kube-system",
        name: "efs-csi-node",
        image: "public.ecr.aws/efs-csi-driver/amazon/aws-efs-csi-driver:v2.0.7",
        desired: ALL_NODES,
        selector: LINUX,
    },
    Daemon {
        ns: "kube-system",
        name: "eks-pod-identity-agent",
        image: "602401143452.dkr.ecr.us-east-1.amazonaws.com/eks/eks-pod-identity-agent:0.1.10",
        desired: ALL_NODES,
        selector: NO_SELECTOR,
    },
    Daemon {
        ns: "kube-system",
        name: "nvidia-device-plugin",
        image: "nvcr.io/nvidia/k8s-device-plugin:v0.16.2",
        desired: GPU_NODES,
        selector: GPU_GROUP,
    },
    Daemon {
        ns: "monitoring",
        name: "node-exporter",
        image: "quay.io/prometheus/node-exporter:v1.8.2",
        desired: ALL_NODES,
        selector: LINUX,
    },
    Daemon {
        ns: "monitoring",
        name: "dcgm-exporter",
        image: "nvcr.io/nvidia/k8s/dcgm-exporter:3.3.7-3.5.0-ubuntu22.04",
        desired: GPU_NODES,
        selector: GPU_GROUP,
    },
    Daemon {
        ns: "logging",
        name: "vector",
        image: "timberio/vector:0.41.1-distroless-libc",
        desired: ALL_NODES,
        selector: LINUX,
    },
];

struct Cron {
    ns: &'static str,
    name: &'static str,
    schedule: &'static str,
    image: &'static str,
    last: Duration,
    interval: Duration,
    took: Duration,
}

#[allow(clippy::too_many_lines)]
fn crons() -> [Cron; 14] {
    [
        Cron {
            ns: "payments",
            name: "settlement-report",
            schedule: "0 2 * * *",
            image: "ghcr.io/acme/payments-worker:1.9.3",
            last: hours(19),
            interval: hours(24),
            took: minutes(4),
        },
        Cron {
            ns: "payments",
            name: "reconcile-ledger",
            schedule: "*/30 * * * *",
            image: "ghcr.io/acme/payments-worker:1.9.3",
            last: minutes(14),
            interval: minutes(30),
            took: Duration::from_secs(48),
        },
        Cron {
            ns: "orders",
            name: "abandoned-carts",
            schedule: "0 * * * *",
            image: "ghcr.io/acme/orders-api:4.8.2",
            last: minutes(14),
            interval: hours(1),
            took: Duration::from_secs(31),
        },
        Cron {
            ns: "inventory",
            name: "stock-snapshot",
            schedule: "15 */6 * * *",
            image: "ghcr.io/acme/inventory-api:1.21.0",
            last: minutes(179),
            interval: hours(6),
            took: minutes(2),
        },
        Cron {
            ns: "web",
            name: "sitemap-builder",
            schedule: "0 4 * * *",
            image: "ghcr.io/acme/web-bff:5.2.1",
            last: hours(17),
            interval: hours(24),
            took: minutes(1),
        },
        Cron {
            ns: "search",
            name: "reindex-delta",
            schedule: "*/15 * * * *",
            image: "ghcr.io/acme/search-indexer:0.18.0",
            last: minutes(14),
            interval: minutes(15),
            took: Duration::from_secs(95),
        },
        Cron {
            ns: "ml",
            name: "retrain-ranker",
            schedule: "0 3 * * 0",
            image: "ghcr.io/acme/trainer:cuda12",
            last: days(3),
            interval: days(7),
            took: minutes(52),
        },
        Cron {
            ns: "auth",
            name: "rotate-signing-keys",
            schedule: "0 0 1 * *",
            image: "ghcr.io/acme/auth-api:6.1.0",
            last: days(12),
            interval: days(30),
            took: Duration::from_secs(12),
        },
        Cron {
            ns: "notifications",
            name: "digest-mailer",
            schedule: "0 8 * * *",
            image: "ghcr.io/acme/notify-api:2.0.5",
            last: hours(13),
            interval: hours(24),
            took: minutes(6),
        },
        Cron {
            ns: "analytics",
            name: "rollup-hourly",
            schedule: "5 * * * *",
            image: "ghcr.io/acme/events-collector:3.9.0",
            last: minutes(9),
            interval: hours(1),
            took: minutes(3),
        },
        Cron {
            ns: "data-pipeline",
            name: "compact-topics",
            schedule: "0 1 * * *",
            image: "bitnami/kafka:3.8.0",
            last: hours(20),
            interval: hours(24),
            took: minutes(11),
        },
        Cron {
            ns: "monitoring",
            name: "grafana-backup",
            schedule: "30 2 * * *",
            image: "ghcr.io/acme/grafana-backup:0.4.0",
            last: hours(18),
            interval: hours(24),
            took: Duration::from_secs(40),
        },
        Cron {
            ns: "logging",
            name: "loki-retention",
            schedule: "0 5 * * *",
            image: "grafana/loki:3.1.1",
            last: hours(16),
            interval: hours(24),
            took: minutes(7),
        },
        Cron {
            ns: "flux-system",
            name: "image-scan",
            schedule: "0 */4 * * *",
            image: "ghcr.io/acme/image-scan:1.2.0",
            last: minutes(74),
            interval: hours(4),
            took: minutes(2),
        },
    ]
}

struct Ingress {
    ns: &'static str,
    name: &'static str,
    host: &'static str,
}

const INGRESSES: [Ingress; 11] = [
    Ingress {
        ns: "web",
        name: "web-frontend",
        host: "www.acme.com",
    },
    Ingress {
        ns: "web",
        name: "web-bff",
        host: "api.acme.com",
    },
    Ingress {
        ns: "payments",
        name: "checkout-api",
        host: "checkout.acme.com",
    },
    Ingress {
        ns: "payments",
        name: "payments-gateway",
        host: "pay.acme.com",
    },
    Ingress {
        ns: "auth",
        name: "auth-api",
        host: "auth.acme.com",
    },
    Ingress {
        ns: "orders",
        name: "orders-api",
        host: "orders.acme.com",
    },
    Ingress {
        ns: "monitoring",
        name: "grafana",
        host: "grafana.internal.acme.com",
    },
    Ingress {
        ns: "search",
        name: "search-api",
        host: "search.acme.com",
    },
    Ingress {
        ns: "notifications",
        name: "push-gateway",
        host: "push.acme.com",
    },
    Ingress {
        ns: "analytics",
        name: "dashboards-api",
        host: "insights.acme.com",
    },
    Ingress {
        ns: "feature-flags",
        name: "flagd",
        host: "flags.internal.acme.com",
    },
];

const EXTRA_CONFIGMAPS: [(&str, &str, &[&str]); 17] = [
    ("kube-system", "coredns", &["Corefile"]),
    ("kube-system", "aws-auth", &["mapRoles", "mapUsers"]),
    ("kube-system", "kube-proxy", &["kubeconfig"]),
    ("kube-system", "kube-proxy-config", &["config"]),
    (
        "kube-system",
        "amazon-vpc-cni",
        &["enable-network-policy-controller", "enable-windows-ipam"],
    ),
    ("kube-system", "cluster-autoscaler-status", &["status"]),
    (
        "monitoring",
        "prometheus-k8s-rulefiles-0",
        &["monitoring-alerts.yaml", "kube-rules.yaml"],
    ),
    ("monitoring", "alertmanager-templates", &["slack.tmpl"]),
    ("logging", "loki-runtime", &["runtime-config.yaml"]),
    ("logging", "vector-config", &["vector.yaml"]),
    (
        "ingress-nginx",
        "ingress-nginx-controller",
        &["allow-snippet-annotations", "use-forwarded-headers"],
    ),
    ("data-pipeline", "kafka-config", &["server.properties"]),
    ("data-pipeline", "zookeeper-config", &["zoo.cfg"]),
    ("search", "search-indexer-config", &["indexer.yaml"]),
    ("payments", "payments-db-init", &["init.sql"]),
    ("feature-flags", "flag-definitions", &["flags.json"]),
    (
        "kube-system",
        "extension-apiserver-authentication",
        &["client-ca-file"],
    ),
];

const EXTRA_SECRETS: [(&str, &str, &str, &[&str]); 7] = [
    (
        "flux-system",
        "flux-system",
        OPAQUE,
        &["identity", "identity.pub", "known_hosts"],
    ),
    ("flux-system", "sops-age", OPAQUE, &["age.agekey"]),
    (
        "cert-manager",
        "cert-manager-webhook-ca",
        TLS,
        &["ca.crt", "tls.crt", "tls.key"],
    ),
    (
        "cert-manager",
        "letsencrypt-prod-account-key",
        OPAQUE,
        &["tls.key"],
    ),
    (
        "ingress-nginx",
        "ingress-nginx-admission",
        OPAQUE,
        &["ca", "cert", "key"],
    ),
    ("loco-system", "loco-agent-token", OPAQUE, &["token"]),
    (
        "loco-system",
        "loco-registry-credentials",
        DOCKER_CONFIG,
        &[".dockerconfigjson"],
    ),
];

const OPAQUE: &str = "Opaque";
const TLS: &str = "kubernetes.io/tls";
const DOCKER_CONFIG: &str = "kubernetes.io/dockerconfigjson";
const TLS_KEYS: &[&str] = &["tls.crt", "tls.key"];
const PULL_KEYS: &[&str] = &[".dockerconfigjson"];
const CHECKOUT_KEYS: &[&str] = &["STRIPE_API_KEY", "DATABASE_URL"];
const APP_KEYS: &[&str] = &["DATABASE_URL", "API_TOKEN"];
const CREDENTIAL_KEYS: &[&str] = &["username", "password"];
const CONFIG_KEYS: &[&str] = &["app.yaml", "logging.yaml"];
const CHECKOUT_CONFIG: &str = "  app.yaml: |\n    server:\n      port: 8080\n      host: 0.0.0.0\n      readTimeout: 30s\n      writeTimeout: 30s\n    telemetry:\n      endpoint: https://otel-collector.monitoring.svc.cluster.local:4318/v1/traces?service=checkout-api&environment=production&region=us-east-1\n      enabled: true\n      service: checkout-api\n      region: us-east-1\n      environment: production\n      sampling: 1.0\n    database:\n      host: payments-db.payments.svc.cluster.local\n      port: 5432\n      name: checkout\n      sslMode: require\n      maxConnections: 20\n      connectTimeout: 10s\n    retries:\n      attempts: 3\n      initialDelay: 100ms\n      maxDelay: 5s\n    health:\n      path: /healthz\n      interval: 10s";
const ROOT_CA_KEYS: &[&str] = &["ca.crt"];
const CHECKOUT: &str = "checkout-api";

pub(crate) fn deployment_conditions(available: bool, progressing: Condition) -> Vec<Condition> {
    let available = if available {
        Condition::new("Available", TRUE)
            .reason("MinimumReplicasAvailable")
            .message("Deployment has minimum availability.")
    } else {
        Condition::new("Available", FALSE)
            .reason("MinimumReplicasUnavailable")
            .message("Deployment does not have minimum availability.")
    };
    vec![
        available.age(Some(hours(3))),
        progressing.age(Some(hours(3))),
    ]
}

pub(crate) fn rolled_out(name: &str) -> Condition {
    Condition::new("Progressing", TRUE)
        .reason("NewReplicaSetAvailable")
        .message(format!(
            "ReplicaSet \"{name}\" has successfully progressed."
        ))
}

pub(crate) fn timed_out(replica_set: &str) -> Condition {
    Condition::new("Progressing", FALSE)
        .reason(PROGRESS_DEADLINE)
        .message(format!(
            "ReplicaSet \"{replica_set}\" has timed out progressing."
        ))
}

struct Trouble {
    kind: ResourceKind,
    name: &'static str,
    conditions: Vec<Condition>,
    diagnosis: Option<Diagnosis>,
}

fn troubles() -> Vec<Trouble> {
    let stalled = timed_out("checkout-api-7f9c6d");
    let deadline = Condition::new("Failed", TRUE)
        .reason("DeadlineExceeded")
        .message(DEADLINE_MESSAGE)
        .age(Some(minutes(108)));
    let target = Condition::new("FailureTarget", TRUE)
        .reason("DeadlineExceeded")
        .message(DEADLINE_MESSAGE)
        .age(Some(minutes(108)));
    let unready = Condition::new("Ready", FALSE)
        .reason("PodNotReady")
        .message("1 of 1 replicas are not ready")
        .age(Some(minutes(40)));
    vec![
        Trouble {
            kind: ResourceKind::Deployment,
            name: CHECKOUT,
            conditions: deployment_conditions(true, stalled),
            diagnosis: Some(Diagnosis::new(
                "CrashLoopBackOff",
                "14 restarts, last exit Error (1)",
            )),
        },
        Trouble {
            kind: ResourceKind::Deployment,
            name: "grafana",
            conditions: deployment_conditions(false, rolled_out("grafana-6b8d9")),
            diagnosis: Some(Diagnosis::new(
                "FailedMount",
                "MountVolume.SetUp failed for volume \"dashboards\" : configmap \"grafana-dashboards\" not found",
            )),
        },
        Trouble {
            kind: ResourceKind::StatefulSet,
            name: "search-indexer",
            conditions: vec![unready],
            diagnosis: Some(Diagnosis::new(
                "NotReady",
                "Readiness probe failed: HTTP probe failed with statuscode: 503",
            )),
        },
        Trouble {
            kind: ResourceKind::Job,
            name: "backfill-orders-28770412",
            conditions: vec![target, deadline],
            diagnosis: None,
        },
    ]
}

fn explain(kind: ResourceKind, mut rows: Vec<Resource>) -> Vec<Resource> {
    for row in &mut rows {
        if kind == ResourceKind::Deployment && row.status == Status::Ok {
            let replica_set = format!("{}-{}", row.name, crate::derive::suffix(row.name.len()));
            row.conditions = deployment_conditions(true, rolled_out(&replica_set));
        }
    }
    for trouble in troubles().into_iter().filter(|t| t.kind == kind) {
        if let Some(row) = rows.iter_mut().find(|row| row.name == trouble.name) {
            row.diagnosis = trouble
                .diagnosis
                .or_else(|| from_conditions(&trouble.conditions));
            row.conditions = trouble.conditions;
        }
    }
    rows
}

pub fn prod_us_east() -> BTreeMap<ResourceKind, Vec<Resource>> {
    let lists = [
        (ResourceKind::Deployment, deployments()),
        (ResourceKind::StatefulSet, statefulsets()),
        (ResourceKind::DaemonSet, daemonsets()),
        (ResourceKind::Job, jobs()),
        (ResourceKind::CronJob, cronjobs()),
        (ResourceKind::Service, services()),
        (ResourceKind::Ingress, ingresses()),
        (ResourceKind::ConfigMap, configmaps()),
        (ResourceKind::Secret, secrets()),
        (ResourceKind::PersistentVolumeClaim, pvcs()),
    ];
    lists
        .into_iter()
        .map(|(kind, rows)| (kind, explain(kind, rows)))
        .collect()
}

fn metadata(api: &str, kind: ResourceKind, ns: &str, name: &str) -> String {
    format!(
        "apiVersion: {api}\nkind: {}\nmetadata:\n  name: {name}\n  namespace: {ns}\n  labels:\n    app.kubernetes.io/name: {name}\n",
        kind.name()
    )
}

fn row(
    ns: &str,
    name: &str,
    status: Status,
    cells: Vec<String>,
    age: Duration,
    body: &str,
) -> Resource {
    Resource {
        uid: Uid::default(),
        namespace: ns.into(),
        name: name.into(),
        status,
        cells,
        age,
        yaml: body.to_owned(),
        labels: BTreeMap::from([("app.kubernetes.io/name".into(), name.into())]),
        conditions: Vec::new(),
        diagnosis: None,
    }
}

fn deployments() -> Vec<Resource> {
    DEPLOYMENTS
        .iter()
        .map(|d| {
            let kind = ResourceKind::Deployment;
            let mut yaml = metadata("apps/v1", kind, d.ns, d.name);
            let _ = write!(
                yaml,
                "spec:\n  replicas: {r}\n  selector:\n    matchLabels:\n      app.kubernetes.io/name: {n}\n  template:\n    spec:\n      containers:\n        - name: {n}\n          image: {i}\n          ports:\n            - containerPort: {p}\nstatus:\n  replicas: {r}\n  updatedReplicas: {r}\n  readyReplicas: {ready}\n  availableReplicas: {ready}\n",
                r = d.replicas,
                n = d.name,
                i = d.image,
                p = d.port,
                ready = d.ready,
            );
            let cells = vec![
                format!("{}/{}", d.ready, d.replicas),
                d.replicas.to_string(),
                d.ready.to_string(),
                d.image.into(),
            ];
            row(d.ns, d.name, d.status, cells, days(d.age_days), &yaml)
        })
        .collect()
}

fn statefulsets() -> Vec<Resource> {
    STATEFULSETS
        .iter()
        .map(|s| {
            let kind = ResourceKind::StatefulSet;
            let mut yaml = metadata("apps/v1", kind, s.ns, s.name);
            let _ = write!(
                yaml,
                "spec:\n  replicas: {r}\n  serviceName: {n}\n  template:\n    spec:\n      containers:\n        - name: {n}\n          image: {i}\n          ports:\n            - containerPort: {p}\n",
                r = s.replicas,
                n = s.name,
                i = s.image,
                p = s.port,
            );
            if let Some((volume, size)) = s.volume {
                let _ = write!(
                    yaml,
                    "  volumeClaimTemplates:\n    - metadata:\n        name: {volume}\n      spec:\n        storageClassName: {STORAGE_CLASS}\n        resources:\n          requests:\n            storage: {size}\n"
                );
            }
            let _ = write!(
                yaml,
                "status:\n  replicas: {}\n  readyReplicas: {}\n",
                s.replicas, s.ready
            );
            let cells = vec![
                format!("{}/{}", s.ready, s.replicas),
                s.name.into(),
                s.image.into(),
            ];
            row(s.ns, s.name, s.status, cells, days(CLUSTER_AGE_DAYS), &yaml)
        })
        .collect()
}

fn daemonsets() -> Vec<Resource> {
    DAEMONSETS
        .iter()
        .map(|d| {
            let kind = ResourceKind::DaemonSet;
            let mut yaml = metadata("apps/v1", kind, d.ns, d.name);
            let selector = if d.selector == NO_SELECTOR {
                String::new()
            } else {
                let (key, value) = d.selector.split_once('=').unwrap_or((d.selector, ""));
                format!("      nodeSelector:\n        {key}: {value}\n")
            };
            let _ = write!(
                yaml,
                "spec:\n  template:\n    spec:\n{selector}      containers:\n        - name: {n}\n          image: {i}\nstatus:\n  desiredNumberScheduled: {d}\n  numberReady: {d}\n",
                n = d.name,
                i = d.image,
                d = d.desired,
            );
            let cells = vec![
                d.desired.to_string(),
                d.desired.to_string(),
                d.selector.into(),
                d.image.into(),
            ];
            row(d.ns, d.name, Status::Ok, cells, days(CLUSTER_AGE_DAYS), &yaml)
        })
        .collect()
}

fn job(
    ns: &str,
    name: &str,
    image: &str,
    outcome: JobOutcome,
    took: Duration,
    started: Duration,
) -> Resource {
    let kind = ResourceKind::Job;
    let (status, label, succeeded, failed) = match outcome {
        JobOutcome::Complete => (Status::Ok, "Complete", 1, 0),
        JobOutcome::Running => (Status::Progress, "Running", 0, 0),
        JobOutcome::Failed => (Status::Err, "Failed", 0, 1),
    };
    let mut yaml = metadata("batch/v1", kind, ns, name);
    let _ = write!(
        yaml,
        "spec:\n  completions: 1\n  backoffLimit: 6\n  template:\n    spec:\n      restartPolicy: Never\n      containers:\n        - name: main\n          image: {image}\nstatus:\n  succeeded: {succeeded}\n  failed: {failed}\n"
    );
    if matches!(outcome, JobOutcome::Failed) {
        yaml.push_str("  conditions:\n    - type: FailureTarget\n      status: \"True\"\n      reason: DeadlineExceeded\n      message: Job was active longer than specified deadline\n    - type: Failed\n      status: \"True\"\n      reason: DeadlineExceeded\n      message: Job was active longer than specified deadline\n");
    }
    let cells = vec![format!("{succeeded}/1"), age(took), label.into()];
    row(ns, name, status, cells, started, &yaml)
}

#[derive(Clone, Copy)]
enum JobOutcome {
    Complete,
    Running,
    Failed,
}

fn jobs() -> Vec<Resource> {
    let mut rows = Vec::new();
    for (ix, cron) in crons().iter().enumerate() {
        for run in 0..2u32 {
            let id = JOB_ID_BASE + ix as u64 * JOB_ID_STRIDE + u64::from(run);
            let name = format!("{}-{id}", cron.name);
            let started = cron.last + cron.interval * run;
            rows.push(job(
                cron.ns,
                &name,
                cron.image,
                JobOutcome::Complete,
                cron.took,
                started,
            ));
        }
    }
    rows.push(job(
        "cert-manager",
        "cert-manager-startupapicheck",
        "quay.io/jetstack/cert-manager-startupapicheck:v1.15.3",
        JobOutcome::Complete,
        Duration::from_secs(38),
        days(CLUSTER_AGE_DAYS),
    ));
    rows.push(job(
        "ml",
        "ml-batch-5c7f",
        "ghcr.io/acme/trainer:cuda12",
        JobOutcome::Running,
        minutes(6),
        minutes(6),
    ));
    rows.push(job(
        "data-pipeline",
        "backfill-orders-28770412",
        "ghcr.io/acme/transform-worker:1.15.2",
        JobOutcome::Failed,
        minutes(12),
        hours(2),
    ));
    rows
}

fn cronjobs() -> Vec<Resource> {
    crons()
        .iter()
        .map(|c| {
            let kind = ResourceKind::CronJob;
            let mut yaml = metadata("batch/v1", kind, c.ns, c.name);
            let _ = write!(
                yaml,
                "spec:\n  schedule: \"{}\"\n  suspend: false\n  concurrencyPolicy: Forbid\n  jobTemplate:\n    spec:\n      template:\n        spec:\n          containers:\n            - name: main\n              image: {}\nstatus:\n  lastScheduleTime: {} ago\n",
                c.schedule,
                c.image,
                age(c.last),
            );
            let cells = vec![
                c.schedule.into(),
                "False".into(),
                "0".into(),
                age(c.last),
            ];
            row(c.ns, c.name, Status::Ok, cells, days(CLUSTER_AGE_DAYS), &yaml)
        })
        .collect()
}

fn service_ip(ix: usize) -> String {
    let n = ix + SERVICE_OFFSET;
    format!("{SERVICE_SUBNET}.{}.{}", n / SUBNET_OCTET, n % SUBNET_OCTET)
}

fn service(
    ns: &str,
    name: &str,
    kind_label: &str,
    cluster_ip: &str,
    port: u16,
    age: Duration,
) -> Resource {
    let kind = ResourceKind::Service;
    let mut yaml = metadata("v1", kind, ns, name);
    let _ = write!(
        yaml,
        "spec:\n  type: {kind_label}\n  clusterIP: {cluster_ip}\n  selector:\n    app.kubernetes.io/name: {name}\n  ports:\n    - port: {port}\n      protocol: TCP\n"
    );
    let cells = vec![kind_label.into(), cluster_ip.into(), format!("{port}/TCP")];
    row(ns, name, Status::Ok, cells, age, &yaml)
}

fn services() -> Vec<Resource> {
    let mut rows = vec![service(
        "default",
        "kubernetes",
        "ClusterIP",
        &service_ip(0),
        API_SERVER_PORT,
        days(CLUSTER_AGE_DAYS),
    )];
    for (ix, d) in DEPLOYMENTS.iter().enumerate() {
        let load_balancer = d.name == "ingress-nginx-controller";
        let (type_label, port) = if load_balancer {
            ("LoadBalancer", HTTP_PORT)
        } else {
            ("ClusterIP", d.port)
        };
        rows.push(service(
            d.ns,
            d.name,
            type_label,
            &service_ip(ix + 1),
            port,
            days(d.age_days),
        ));
    }
    for s in &STATEFULSETS {
        rows.push(service(
            s.ns,
            s.name,
            "ClusterIP",
            "None",
            s.port,
            days(CLUSTER_AGE_DAYS),
        ));
    }
    let extras = [
        ("monitoring", "node-exporter", "None", NODE_EXPORTER_PORT),
        ("monitoring", "prometheus-operated", "None", PROMETHEUS_PORT),
    ];
    for (ns, name, ip, port) in extras {
        rows.push(service(
            ns,
            name,
            "ClusterIP",
            ip,
            port,
            days(CLUSTER_AGE_DAYS),
        ));
    }
    let admission_ip = service_ip(DEPLOYMENTS.len() + 1);
    rows.push(service(
        "ingress-nginx",
        "ingress-nginx-controller-admission",
        "ClusterIP",
        &admission_ip,
        WEBHOOK_PORT,
        days(CLUSTER_AGE_DAYS),
    ));
    rows
}

fn ingresses() -> Vec<Resource> {
    INGRESSES
        .iter()
        .map(|i| {
            let kind = ResourceKind::Ingress;
            let mut yaml = metadata("networking.k8s.io/v1", kind, i.ns, i.name);
            let _ = write!(
                yaml,
                "spec:\n  ingressClassName: {INGRESS_CLASS}\n  tls:\n    - hosts:\n        - {host}\n      secretName: {name}-tls\n  rules:\n    - host: {host}\n      http:\n        paths:\n          - path: /\n            pathType: Prefix\n            backend:\n              service:\n                name: {name}\n                port:\n                  number: {HTTPS_PORT}\nstatus:\n  loadBalancer:\n    ingress:\n      - hostname: {INGRESS_ADDRESS}\n",
                host = i.host,
                name = i.name,
            );
            let cells = vec![INGRESS_CLASS.into(), i.host.into(), INGRESS_ADDRESS.into()];
            row(i.ns, i.name, Status::Ok, cells, days(CLUSTER_AGE_DAYS), &yaml)
        })
        .collect()
}

fn keyed(
    kind: ResourceKind,
    ns: &str,
    name: &str,
    kind_type: Option<&str>,
    keys: &[&str],
) -> Resource {
    let mut yaml = metadata("v1", kind, ns, name);
    if let Some(kind_type) = kind_type {
        let _ = writeln!(yaml, "type: {kind_type}");
    }
    yaml.push_str("data:\n");
    for key in keys {
        let _ = writeln!(yaml, "  {key}: ••••••");
    }
    let mut cells = Vec::new();
    if let Some(kind_type) = kind_type {
        cells.push(kind_type.to_owned());
    }
    cells.push(keys.len().to_string());
    row(ns, name, Status::Ok, cells, days(CLUSTER_AGE_DAYS), &yaml)
}

fn configmaps() -> Vec<Resource> {
    let kind = ResourceKind::ConfigMap;
    let roots = NAMESPACES
        .iter()
        .map(|ns| keyed(kind, ns, "kube-root-ca.crt", None, ROOT_CA_KEYS));
    let apps = DEPLOYMENTS.iter().map(|d| {
        let mut resource = keyed(kind, d.ns, &format!("{}-config", d.name), None, CONFIG_KEYS);
        if d.name == CHECKOUT {
            resource.yaml = resource.yaml.replace("  app.yaml: ••••••", CHECKOUT_CONFIG);
        }
        resource
    });
    let extras = EXTRA_CONFIGMAPS
        .iter()
        .map(|(ns, name, keys)| keyed(kind, ns, name, None, keys));
    roots.chain(apps).chain(extras).collect()
}

fn secrets() -> Vec<Resource> {
    let kind = ResourceKind::Secret;
    let apps = DEPLOYMENTS
        .iter()
        .filter(|d| APP_NAMESPACES.contains(&d.ns))
        .map(|d| {
            let keys = if d.name == CHECKOUT {
                CHECKOUT_KEYS
            } else {
                APP_KEYS
            };
            keyed(
                kind,
                d.ns,
                &format!("{}-secrets", d.name),
                Some(OPAQUE),
                keys,
            )
        });
    let tls = INGRESSES
        .iter()
        .map(|i| keyed(kind, i.ns, &format!("{}-tls", i.name), Some(TLS), TLS_KEYS));
    let credentials = STATEFULSETS.iter().map(|s| {
        keyed(
            kind,
            s.ns,
            &format!("{}-credentials", s.name),
            Some(OPAQUE),
            CREDENTIAL_KEYS,
        )
    });
    let extras = EXTRA_SECRETS
        .iter()
        .map(|(ns, name, kind_type, keys)| keyed(kind, ns, name, Some(kind_type), keys));
    let pulls = PULL_SECRET_NAMESPACES
        .iter()
        .map(|ns| keyed(kind, ns, "ghcr-pull", Some(DOCKER_CONFIG), PULL_KEYS));
    apps.chain(tls)
        .chain(credentials)
        .chain(extras)
        .chain(pulls)
        .collect()
}

fn fake_uid(seed: &str) -> String {
    let mut hash = FNV_OFFSET;
    let mut hex = String::new();
    let total: usize = UID_GROUPS.iter().sum();
    while hex.len() < total {
        for byte in seed.bytes().chain(hex.clone().into_bytes()) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        let _ = write!(hex, "{hash:016x}");
    }
    let mut groups = Vec::new();
    let mut start = 0;
    for len in UID_GROUPS {
        groups.push(&hex[start..start + len]);
        start += len;
    }
    groups.join("-")
}

fn pvcs() -> Vec<Resource> {
    let kind = ResourceKind::PersistentVolumeClaim;
    let mut rows = Vec::new();
    for s in &STATEFULSETS {
        let Some((volume, size)) = s.volume else {
            continue;
        };
        for replica in 0..s.replicas {
            let name = format!("{volume}-{}-{replica}", s.name);
            let volume_name = format!("pvc-{}", fake_uid(&format!("{}/{name}", s.ns)));
            let mut yaml = metadata("v1", kind, s.ns, &name);
            let _ = write!(
                yaml,
                "spec:\n  accessModes:\n    - ReadWriteOnce\n  storageClassName: {STORAGE_CLASS}\n  volumeName: {volume_name}\n  resources:\n    requests:\n      storage: {size}\nstatus:\n  phase: Bound\n  capacity:\n    storage: {size}\n"
            );
            let cells = vec![
                "Bound".into(),
                size.into(),
                STORAGE_CLASS.into(),
                volume_name,
            ];
            rows.push(row(
                s.ns,
                &name,
                Status::Ok,
                cells,
                days(CLUSTER_AGE_DAYS),
                &yaml,
            ));
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_match_the_navigation() {
        let resources = prod_us_east();
        let count = |kind| resources.get(&kind).map_or(0, Vec::len);
        let expected = [
            (ResourceKind::Deployment, 48),
            (ResourceKind::StatefulSet, 12),
            (ResourceKind::DaemonSet, 9),
            (ResourceKind::Job, 31),
            (ResourceKind::CronJob, 14),
            (ResourceKind::Service, 64),
            (ResourceKind::Ingress, 11),
            (ResourceKind::ConfigMap, 88),
            (ResourceKind::Secret, 71),
            (ResourceKind::PersistentVolumeClaim, 19),
        ];
        for (kind, n) in expected {
            assert_eq!(count(kind), n, "{kind:?}");
        }
    }

    #[test]
    fn every_unhealthy_row_says_why() {
        for (kind, rows) in prod_us_east() {
            for row in rows {
                let flagged = matches!(row.status, Status::Err | Status::Warn);
                assert_eq!(row.diagnosis.is_some(), flagged, "{kind:?} {}", row.name);
            }
        }
        let jobs = prod_us_east()
            .remove(&ResourceKind::Job)
            .unwrap_or_default();
        let failed = jobs
            .iter()
            .find(|job| job.name == "backfill-orders-28770412");
        assert_eq!(
            failed.and_then(|job| job.diagnosis.clone()),
            Some(Diagnosis::new("DeadlineExceeded", DEADLINE_MESSAGE))
        );
    }

    #[test]
    fn cells_line_up_with_kind_columns() {
        for (kind, rows) in prod_us_east() {
            for row in rows {
                assert_eq!(
                    row.cells.len(),
                    kind.columns().len(),
                    "{kind:?} {}",
                    row.name
                );
                assert!(row.yaml.contains(&format!("name: {}", row.name)));
            }
        }
    }

    #[test]
    fn names_are_unique_per_namespace() {
        for (kind, rows) in prod_us_east() {
            let mut keys: Vec<(String, String)> = rows
                .iter()
                .map(|r| (r.namespace.clone(), r.name.clone()))
                .collect();
            let before = keys.len();
            keys.sort();
            keys.dedup();
            assert_eq!(keys.len(), before, "{kind:?}");
        }
    }

    #[test]
    fn fake_uids_are_stable_and_shaped() {
        let uid = fake_uid("payments/data-payments-db-0");
        assert_eq!(uid, fake_uid("payments/data-payments-db-0"));
        assert_ne!(uid, fake_uid("orders/data-orders-db-0"));
        let lengths: Vec<usize> = uid.split('-').map(str::len).collect();
        assert_eq!(lengths, UID_GROUPS);
    }
}
