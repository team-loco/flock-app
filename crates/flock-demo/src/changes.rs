use std::time::Duration;

use flock_core::changes::{
    ChangesLog, Commit, FilePatch, Lane, LineChange, PatchLine, Rendering, Rollout, Target,
    TargetKind, ViewKind,
};
use flock_core::{ClusterId, Status};

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;
const FETCHED_MINUTES: u64 = 4;
const APPLIED_HEALTHY: &str = "applied · healthy";

const STAGING: &str = "staging";
const US_EAST: &str = "prod-us-east";
const EU_WEST: &str = "prod-eu-west";
const DEV: &str = "dev-kind";
const CLUSTERS: [&str; 4] = [STAGING, US_EAST, EU_WEST, DEV];

pub fn log() -> ChangesLog {
    ChangesLog {
        repository: "fleet-infra".into(),
        branch: "main".into(),
        fetched: Duration::from_secs(FETCHED_MINUTES * MINUTE),
        lanes: vec![
            lane(STAGING, "stg", "staging"),
            lane(US_EAST, "use1", "us-east"),
            lane(EU_WEST, "euw1", "eu-west"),
            lane(DEV, "dev", "dev"),
        ],
        commits: vec![
            checkout_bump(),
            prometheus_bump(),
            web_replicas(),
            retry_budget(),
            payments_pdb(),
        ],
        unavailable: None,
        pending: false,
    }
}

fn lane(cluster: &str, short: &str, label: &str) -> Lane {
    Lane {
        cluster: ClusterId::new(cluster),
        short: short.into(),
        label: label.into(),
    }
}

fn ks(name: &str) -> Target {
    Target {
        kind: TargetKind::Kustomization,
        name: name.into(),
    }
}

fn rollout(status: Status, cluster: &str, object: &str, note: &str) -> Rollout {
    Rollout {
        cluster: ClusterId::new(cluster),
        status,
        object: object.into(),
        note: note.into(),
    }
}

fn healthy_everywhere(object: &str) -> Vec<Rollout> {
    CLUSTERS
        .iter()
        .map(|cluster| rollout(Status::Ok, cluster, object, APPLIED_HEALTHY))
        .collect()
}

fn line(change: LineChange, number: u32, text: &str) -> PatchLine {
    PatchLine {
        change,
        number,
        text: text.into(),
        note: None,
    }
}

fn ctx(number: u32, text: &str) -> PatchLine {
    line(LineChange::Context, number, text)
}

fn add(number: u32, text: &str) -> PatchLine {
    line(LineChange::Added, number, text)
}

fn del(number: u32, text: &str) -> PatchLine {
    line(LineChange::Removed, number, text)
}

fn file(path: &str, lines: Vec<PatchLine>) -> FilePatch {
    FilePatch {
        path: path.into(),
        lines,
    }
}

fn added_file(path: &str, lines: &[&str]) -> FilePatch {
    let numbered = (1..).zip(lines).map(|(n, text)| add(n, text)).collect();
    file(path, numbered)
}

fn source(files: Vec<FilePatch>) -> Rendering {
    Rendering {
        kind: ViewKind::Source,
        truncated: false,
        files,
    }
}

fn rendered(cluster: &str, files: Vec<FilePatch>) -> Rendering {
    Rendering {
        kind: ViewKind::Rendered(ClusterId::new(cluster)),
        truncated: false,
        files,
    }
}

fn checkout_bump() -> Commit {
    let deployment = file(
        "apps/payments/base/deployment.yaml",
        vec![
            ctx(21, "      containers:"),
            ctx(22, "      - name: api"),
            del(23, "        image: ghcr.io/acme/checkout-api:2.13.4"),
            add(23, "        image: ghcr.io/acme/checkout-api:2.14.0"),
            ctx(24, "        env:"),
            add(25, "        - name: STRIPE_WEBHOOK_SECRET"),
            add(26, "          valueFrom:"),
            add(27, "            secretKeyRef:"),
            add(28, "              name: stripe"),
            add(29, "              key: webhook-secret"),
            add(30, "              optional: true"),
            ctx(31, "        - name: LOG_LEVEL"),
        ],
    );
    let configmap = file(
        "apps/payments/base/configmap.yaml",
        vec![
            ctx(8, "  files:"),
            ctx(9, "    - app.yaml"),
            del(10, "    - stripe.env"),
            del(11, "      # legacy: read by checkout-api < 2.14"),
            ctx(12, "generatorOptions:"),
        ],
    );
    Commit {
        sha: "a41e07c3f91d".into(),
        message: "checkout-api: bump to 2.14.0, read webhook secret from env".into(),
        author: "@mkhan".into(),
        age: Duration::from_secs(3 * HOUR),
        when: "today 18:11".into(),
        target: ks("apps-payments"),
        rollout: vec![
            rollout(
                Status::Ok,
                STAGING,
                "apps-payments",
                "applied 3h ago · healthy after 48s",
            ),
            rollout(
                Status::Err,
                US_EAST,
                "apps-payments",
                "applied 3h ago · health check failing (checkout-api 2/3)",
            ),
            rollout(
                Status::Suspended,
                EU_WEST,
                "apps-payments",
                "not applied · Kustomization suspended (change freeze)",
            ),
            rollout(
                Status::Ok,
                DEV,
                "apps-payments",
                "applied 3h ago · healthy after 21s",
            ),
        ],
        views: vec![
            source(vec![deployment, configmap]),
            rendered(US_EAST, vec![checkout_rendered(US_EAST)]),
            rendered(STAGING, vec![checkout_rendered(STAGING)]),
        ],
    }
}

fn checkout_rendered(cluster: &str) -> FilePatch {
    let key = if cluster == US_EAST {
        PatchLine {
            note: Some(
                "set by overlays/prod-us-east/patches/stripe.yaml:9 · unchanged 214d".into(),
            ),
            ..add(48, "              key: webhook_secret")
        }
    } else {
        add(48, "              key: webhook-secret")
    };
    file(
        &format!("Deployment/payments/checkout-api · rendered for {cluster}"),
        vec![
            ctx(41, "      - name: api"),
            del(42, "        image: ghcr.io/acme/checkout-api:2.13.4"),
            add(42, "        image: ghcr.io/acme/checkout-api:2.14.0"),
            ctx(43, "        env:"),
            add(44, "        - name: STRIPE_WEBHOOK_SECRET"),
            add(45, "          valueFrom:"),
            add(46, "            secretKeyRef:"),
            add(47, "              name: stripe"),
            key,
            add(49, "              optional: true"),
            ctx(50, "        - name: LOG_LEVEL"),
        ],
    )
}

fn prometheus_bump() -> Commit {
    Commit {
        sha: "3be91f2a0c77".into(),
        message: "monitoring: kube-prometheus-stack 62.7.0 on staging".into(),
        author: "@renovate[bot]".into(),
        age: Duration::from_secs(5 * HOUR),
        when: "today 16:02".into(),
        target: Target {
            kind: TargetKind::HelmRelease,
            name: "kube-prometheus-stack".into(),
        },
        rollout: vec![rollout(
            Status::Warn,
            STAGING,
            "infra-monitoring",
            "HelmRelease upgrade retrying (2/3): context deadline exceeded",
        )],
        views: vec![source(vec![file(
            "infrastructure/monitoring/staging/release.yaml",
            vec![
                ctx(12, "      chart: kube-prometheus-stack"),
                del(13, "      version: 61.9.0"),
                add(13, "      version: 62.7.0"),
            ],
        )])],
    }
}

fn web_replicas() -> Commit {
    Commit {
        sha: "7c2d9e0b51e4".into(),
        message: "web-frontend: raise min replicas to 6".into(),
        author: "@jlee".into(),
        age: Duration::from_secs(DAY),
        when: "yesterday 11:40".into(),
        target: ks("apps-web"),
        rollout: healthy_everywhere("apps-web"),
        views: vec![source(vec![file(
            "apps/web/base/hpa.yaml",
            vec![
                ctx(9, "spec:"),
                del(10, "  minReplicas: 3"),
                add(10, "  minReplicas: 6"),
            ],
        )])],
    }
}

fn retry_budget() -> Commit {
    Commit {
        sha: "d81f3c04a9b2".into(),
        message: "checkout-api: retry budget for stripe calls".into(),
        author: "@mkhan".into(),
        age: Duration::from_secs(9 * DAY),
        when: "Oct 3".into(),
        target: ks("apps-payments"),
        rollout: healthy_everywhere("apps-payments"),
        views: vec![source(vec![file(
            "apps/payments/base/deployment.yaml",
            vec![
                ctx(24, "        env:"),
                add(25, "        - name: STRIPE_MAX_RETRIES"),
                add(26, "          value: \"3\""),
                add(27, "        - name: STRIPE_RETRY_BUDGET_MS"),
                add(28, "          value: \"2000\""),
                ctx(29, "        - name: LOG_LEVEL"),
            ],
        )])],
    }
}

fn payments_pdb() -> Commit {
    let pdb = added_file(
        "apps/payments/base/pdb.yaml",
        &[
            "apiVersion: policy/v1",
            "kind: PodDisruptionBudget",
            "metadata:",
            "  name: checkout-api",
            "  namespace: payments",
            "spec:",
            "  minAvailable: 2",
            "  unhealthyPodEvictionPolicy: AlwaysAllow",
            "  selector:",
            "    matchLabels:",
            "      app: checkout-api",
        ],
    );
    let hpa = added_file(
        "apps/payments/base/hpa.yaml",
        &[
            "apiVersion: autoscaling/v2",
            "kind: HorizontalPodAutoscaler",
            "metadata:",
            "  name: checkout-api",
            "  namespace: payments",
            "spec:",
            "  scaleTargetRef:",
            "    apiVersion: apps/v1",
            "    kind: Deployment",
            "    name: checkout-api",
            "  minReplicas: 3",
            "  maxReplicas: 12",
            "  metrics:",
            "    - type: Resource",
            "      resource:",
            "        name: cpu",
            "        target:",
            "          type: Utilization",
            "          averageUtilization: 70",
            "    - type: Resource",
            "      resource:",
            "        name: memory",
            "        target:",
            "          type: Utilization",
            "          averageUtilization: 80",
        ],
    );
    let kustomization = file(
        "apps/payments/base/kustomization.yaml",
        vec![
            ctx(3, "resources:"),
            ctx(4, "  - deployment.yaml"),
            ctx(5, "  - service.yaml"),
            add(6, "  - pdb.yaml"),
            add(7, "  - hpa.yaml"),
        ],
    );
    Commit {
        sha: "e19a4b8c2f60".into(),
        message: "payments: add PDB and HPA".into(),
        author: "@ops-team".into(),
        age: Duration::from_secs(23 * DAY),
        when: "Sep 19".into(),
        target: ks("apps-payments"),
        rollout: healthy_everywhere("apps-payments"),
        views: vec![source(vec![pdb, hpa, kustomization])],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lanes(commit: &Commit, log: &ChangesLog) -> Vec<Option<Status>> {
        log.lanes
            .iter()
            .map(|lane| commit.rollout_on(&lane.cluster).map(|r| r.status))
            .collect()
    }

    #[test]
    fn commits_match_the_design_list() {
        let log = log();
        let rows: Vec<_> = log
            .commits
            .iter()
            .map(|c| (c.short_sha(), c.author.as_str(), c.age_label(), c.stat()))
            .collect();
        assert_eq!(
            rows,
            [
                ("a41e07c", "@mkhan", "3h".into(), "2 files · +7 −3".into()),
                (
                    "3be91f2",
                    "@renovate[bot]",
                    "5h".into(),
                    "1 file · +1 −1".into()
                ),
                ("7c2d9e0", "@jlee", "1d".into(), "1 file · +1 −1".into()),
                ("d81f3c0", "@mkhan", "9d".into(), "1 file · +4".into()),
                ("e19a4b8", "@ops-team", "23d".into(), "3 files · +38".into()),
            ]
        );
        assert_eq!(log.fetched_label(), "fleet-infra fetched 4m ago");
    }

    #[test]
    fn lanes_show_the_design_outcomes() {
        let log = log();
        let bump = lanes(&log.commits[0], &log);
        assert_eq!(
            bump,
            [
                Some(Status::Ok),
                Some(Status::Err),
                Some(Status::Suspended),
                Some(Status::Ok)
            ]
        );
        let prometheus = lanes(&log.commits[1], &log);
        assert_eq!(prometheus, [Some(Status::Warn), None, None, None]);
        assert_eq!(
            log.commits[1].target_sentence(),
            "affects hr/kube-prometheus-stack on staging"
        );
    }

    #[test]
    fn checkout_bump_opens_on_the_failing_cluster() {
        let commit = &log().commits[0];
        let east = ViewKind::Rendered(ClusterId::new(US_EAST));
        assert_eq!(commit.default_view(), east);
        let view = commit.view(None);
        let noted = view
            .into_iter()
            .flat_map(|v| &v.files)
            .flat_map(|f| &f.lines)
            .filter(|l| l.note.is_some())
            .count();
        assert_eq!(noted, 1);
    }
}
