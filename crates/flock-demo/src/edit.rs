use flock_core::edit::{EditEvent, EditStep, Live, diff_lines};
use flock_core::{Command, ResourceRef, Snapshot, Uid};

const STATUS: &str = "status:";
const DEMO_UID: &str = "demo-uid";
const FIRST_VERSION: u64 = 1;

pub fn handle(command: &Command, snapshot: &Snapshot) -> bool {
    let Command::Edit(request) = command else {
        return false;
    };
    let event = match &request.step {
        EditStep::Load { reveal } => EditEvent::Loaded(Live {
            text: manifest(snapshot, &request.target),
            uid: Uid::new(DEMO_UID),
            resource_version: FIRST_VERSION.to_string(),
            revealed: *reveal,
        }),
        EditStep::Review(draft) => EditEvent::Reviewed(diff_lines(&draft.base.text, &draft.text)),
        EditStep::Apply(draft) => {
            let version = draft
                .base
                .resource_version
                .parse::<u64>()
                .unwrap_or(FIRST_VERSION);
            EditEvent::Applied(Live {
                text: draft.text.clone(),
                resource_version: (version + 1).to_string(),
                ..draft.base.clone()
            })
        }
    };
    request.sink.send_blocking(event);
    true
}

fn manifest(snapshot: &Snapshot, target: &ResourceRef) -> String {
    let flux = snapshot
        .flux
        .iter()
        .find(|object| object.key.resource() == *target)
        .map(|object| object.yaml.as_str());
    let listed = snapshot
        .yaml(target)
        .or(flux)
        .filter(|yaml| !yaml.is_empty());
    match listed {
        Some(yaml) => without_status(yaml),
        None => minimal(target),
    }
}

fn without_status(yaml: &str) -> String {
    let mut kept = String::new();
    let mut skipping = false;
    for line in yaml.split_inclusive('\n') {
        let top_level = !line.starts_with(' ') && !line.trim().is_empty();
        if top_level {
            skipping = line.starts_with(STATUS);
        }
        if !skipping {
            kept.push_str(line);
        }
    }
    kept
}

fn minimal(target: &ResourceRef) -> String {
    let api_version = api_version(&target.kind);
    let namespace = target
        .namespace
        .as_ref()
        .map(|namespace| format!("  namespace: {namespace}\n"))
        .unwrap_or_default();
    format!(
        "apiVersion: {api_version}\nkind: {}\nmetadata:\n  name: {}\n{namespace}",
        target.kind, target.name
    )
}

fn api_version(kind: &str) -> &'static str {
    match kind {
        "Deployment" => "apps/v1",
        "StatefulSet" => "apps/v1",
        "DaemonSet" => "apps/v1",
        "Job" => "batch/v1",
        "CronJob" => "batch/v1",
        "Ingress" => "networking.k8s.io/v1",
        _ => "v1",
    }
}

#[cfg(test)]
mod tests {
    use flock_core::edit::{Draft, EditRequest};
    use flock_core::{ClusterId, Sink};

    use super::*;

    #[test]
    fn the_demo_loads_reviews_and_applies_an_edit() {
        let snapshot = crate::snapshot();
        let target = ResourceRef {
            cluster: ClusterId::new("prod-us-east"),
            kind: "ConfigMap".into(),
            namespace: Some("payments".into()),
            name: "checkout-api-config".into(),
        };
        let (sink, events) = Sink::channel();
        let load = Command::Edit(Box::new(EditRequest {
            target: target.clone(),
            step: EditStep::Load { reveal: false },
            sink: sink.clone(),
        }));
        assert!(handle(&load, &snapshot));
        let Ok(EditEvent::Loaded(live)) = events.recv_blocking() else {
            panic!("loaded");
        };
        assert!(live.text.contains("name: checkout-api-config"));
        assert!(!live.text.contains(STATUS));
        let draft = Draft {
            text: live.text.replace("port: 8080", "port: 9090"),
            base: live,
        };
        let review = Command::Edit(Box::new(EditRequest {
            target,
            step: EditStep::Review(draft),
            sink,
        }));
        assert!(handle(&review, &snapshot));
        let Ok(EditEvent::Reviewed(lines)) = events.recv_blocking() else {
            panic!("reviewed");
        };
        assert!(lines.iter().any(|line| line.text.contains("port: 9090")));
    }

    #[test]
    fn status_blocks_are_dropped() {
        let yaml = "kind: Deployment\nspec:\n  replicas: 2\nstatus:\n  replicas: 2\n";
        assert_eq!(
            without_status(yaml),
            "kind: Deployment\nspec:\n  replicas: 2\n"
        );
    }
}
