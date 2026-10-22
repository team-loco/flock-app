use flock_core::changes::{LineChange, PatchLine};
use flock_core::compare::{Cell, ComparedCluster, Comparison, Field, SpecDiff};
use flock_core::{ClusterId, Status, Uid};

const PROD_US: &str = "prod-us-east";
const PROD_EU: &str = "prod-eu-west";
const STAGING: &str = "staging";
const SPEC_PATH: &str = "spec.template";
const READY: &str = "Ready";
const LAST_ROLLOUT: &str = "Last rollout";
const SPEC_FIELDS: [&str; 2] = ["Image", "STRIPE_WEBHOOK_SECRET"];
const ROLLING_OUT: &str = "Rolling out · rev 38";
const HEALTHY: &str = "Healthy";
const ROLLED_OUT: &str = "0s ago · rev 38 · flock";
const ALL_READY: &str = "3 / 3";

fn compared(name: &str, status: Status, health: &str) -> ComparedCluster {
    ComparedCluster {
        cluster: ClusterId::new(name),
        uid: Uid::default(),
        status,
        health: health.into(),
    }
}

fn field(label: &str, cells: [Cell; 3]) -> Field {
    Field {
        label: label.into(),
        cells: cells.into(),
    }
}

fn plain(text: &str) -> Cell {
    Cell::new(text)
}

fn err(text: &str) -> Cell {
    Cell::flagged(text, Status::Err)
}

fn warn(text: &str) -> Cell {
    Cell::flagged(text, Status::Warn)
}

fn line(change: LineChange, number: u32, text: &str) -> PatchLine {
    PatchLine {
        change,
        number,
        text: text.into(),
        note: None,
    }
}

pub fn comparisons() -> Vec<Comparison> {
    vec![checkout_api()]
}

fn checkout_api() -> Comparison {
    let same = |text: &str| [plain(text), plain(text), plain(text)];
    Comparison {
        kind: "Deployment".into(),
        namespace: Some("payments".into()),
        name: "checkout-api".into(),
        clusters: vec![
            compared(PROD_US, Status::Err, "Degraded · 1 pod crashlooping"),
            compared(PROD_EU, Status::Ok, HEALTHY),
            compared(STAGING, Status::Ok, HEALTHY),
        ],
        fields: vec![
            field(READY, [err("2 / 3"), plain("3 / 3"), plain("2 / 2")]),
            field(
                "Image",
                [
                    plain("checkout-api:2.14.0"),
                    warn("checkout-api:2.13.4"),
                    plain("checkout-api:2.14.0"),
                ],
            ),
            field(
                "STRIPE_WEBHOOK_SECRET",
                [
                    err("secret stripe · key webhook_secret (not found, optional)"),
                    plain("secret stripe · key webhook-secret"),
                    plain("secret stripe · key webhook-secret"),
                ],
            ),
            field(
                "Replicas · HPA",
                [
                    plain("3 · 3–12 @ 70% cpu"),
                    plain("3 · 3–12 @ 70% cpu"),
                    warn("2 · 1–4 @ 80% cpu"),
                ],
            ),
            field(
                "CPU request / limit",
                [
                    plain("0.50 / 1.00 cores"),
                    plain("0.50 / 1.00 cores"),
                    warn("0.25 / 0.50 cores"),
                ],
            ),
            field("Memory request / limit", same("512Mi / 1Gi")),
            field("Strategy", same("RollingUpdate 25% / 25%")),
            field(
                LAST_ROLLOUT,
                [
                    plain("3h ago · rev 37 · argocd"),
                    plain("2d ago · rev 35 · argocd"),
                    plain("41m ago · rev 52 · ci-bot"),
                ],
            ),
        ],
        spec: Some(SpecDiff {
            path: SPEC_PATH.into(),
            base: ClusterId::new(PROD_US),
            other: ClusterId::new(PROD_EU),
            lines: spec_lines(),
        }),
    }
}

fn spec_lines() -> Vec<PatchLine> {
    use LineChange::{Added, Context, Removed};
    vec![
        line(Context, 31, "      containers:"),
        line(Context, 32, "      - name: api"),
        line(
            Removed,
            33,
            "        image: ghcr.io/acme/checkout-api:2.14.0",
        ),
        line(Added, 33, "        image: ghcr.io/acme/checkout-api:2.13.4"),
        line(Context, 34, "        env:"),
        line(Context, 35, "        - name: STRIPE_WEBHOOK_SECRET"),
        line(Context, 36, "          valueFrom:"),
        line(Context, 37, "            secretKeyRef:"),
        line(Context, 38, "              name: stripe"),
        line(Removed, 39, "              key: webhook_secret"),
        line(Added, 39, "              key: webhook-secret"),
        line(Context, 40, "              optional: true"),
    ]
}

fn column(comparison: &Comparison, cluster: &ClusterId) -> Option<usize> {
    comparison
        .clusters
        .iter()
        .position(|c| &c.cluster == cluster)
}

fn set_cell(comparison: &mut Comparison, label: &str, column: usize, cell: Cell) {
    let target = comparison
        .fields
        .iter_mut()
        .find(|f| f.label == label)
        .and_then(|f| f.cells.get_mut(column));
    if let Some(slot) = target {
        *slot = cell;
    }
}

pub fn copy_spec(comparison: &mut Comparison, from: &ClusterId, to: &ClusterId) -> bool {
    let (Some(source), Some(target)) = (column(comparison, from), column(comparison, to)) else {
        return false;
    };
    for label in SPEC_FIELDS {
        let copied = comparison
            .fields
            .iter()
            .find(|f| f.label == label)
            .and_then(|f| f.cells.get(source))
            .map(|c| Cell::new(c.text.clone()));
        if let Some(cell) = copied {
            set_cell(comparison, label, target, cell);
        }
    }
    set_cell(comparison, LAST_ROLLOUT, target, Cell::new(ROLLED_OUT));
    if let Some(spec) = comparison.spec.as_mut() {
        spec.lines.retain(|l| l.change != LineChange::Removed);
        for line in &mut spec.lines {
            line.change = LineChange::Context;
        }
    }
    if let Some(cluster) = comparison.clusters.get_mut(target) {
        cluster.status = Status::Progress;
        cluster.health = ROLLING_OUT.into();
    }
    true
}

pub fn finish_rollout(comparison: &mut Comparison, cluster: &ClusterId) {
    let Some(target) = column(comparison, cluster) else {
        return;
    };
    set_cell(comparison, READY, target, Cell::new(ALL_READY));
    if let Some(compared) = comparison.clusters.get_mut(target) {
        compared.status = Status::Ok;
        compared.health = HEALTHY.into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkout_api_reproduces_the_design() {
        let comparison = checkout_api();
        assert_eq!(comparison.differing(), 6);
        assert_eq!(comparison.hidden_note(true), "2 identical fields hidden");
        let spec = comparison.spec.as_ref().map(SpecDiff::apply_label);
        assert_eq!(spec.as_deref(), Some("Apply eu-west value to us-east…"));
    }

    #[test]
    fn copying_the_spec_rolls_out_then_heals() {
        let mut comparison = checkout_api();
        let us = ClusterId::new(PROD_US);
        assert!(copy_spec(&mut comparison, &ClusterId::new(PROD_EU), &us));
        let image = comparison.fields.iter().find(|f| f.label == "Image");
        let images: Vec<&str> = image
            .map(|f| f.cells.iter().map(|c| c.text.as_str()).collect())
            .unwrap_or_default();
        assert_eq!(
            images,
            [
                "checkout-api:2.13.4",
                "checkout-api:2.13.4",
                "checkout-api:2.14.0"
            ]
        );
        assert!(!comparison.spec.as_ref().is_some_and(SpecDiff::has_changes));
        assert_eq!(comparison.clusters[0].status, Status::Progress);
        finish_rollout(&mut comparison, &us);
        assert_eq!(comparison.clusters[0].status, Status::Ok);
        assert_eq!(comparison.fields[0].cells[0], Cell::new(ALL_READY));
    }

    #[test]
    fn copying_between_unknown_clusters_changes_nothing() {
        let mut comparison = checkout_api();
        let ghost = ClusterId::new("ghost");
        assert!(!copy_spec(
            &mut comparison,
            &ghost,
            &ClusterId::new(PROD_US)
        ));
        assert_eq!(comparison, checkout_api());
    }
}
