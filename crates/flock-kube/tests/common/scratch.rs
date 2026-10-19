use std::time::{SystemTime, UNIX_EPOCH};

use k8s_openapi::api::core::v1::{Namespace, ServiceAccount};
use kube::api::PostParams;
use kube::{Api, Client};
use serde_json::json;

use super::eventually;

const PREFIX: &str = "flock-e2e-";
const KUBECTL: &str = "kubectl";
const SERVICE_ACCOUNT: &str = "default";

pub struct ScratchNamespace {
    name: String,
    contexts: Vec<String>,
}

fn unique_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!("{PREFIX}{}-{nanos:x}", std::process::id())
}

impl ScratchNamespace {
    pub async fn create(clusters: &[(&str, &Client)]) -> Self {
        let scratch = Self {
            name: unique_name(),
            contexts: clusters
                .iter()
                .map(|(context, _)| (*context).to_owned())
                .collect(),
        };
        let Ok(namespace) =
            serde_json::from_value::<Namespace>(json!({ "metadata": { "name": scratch.name } }))
        else {
            panic!("namespace fixture");
        };
        for (_, client) in clusters {
            let namespaces: Api<Namespace> = Api::all((*client).clone());
            let created = namespaces.create(&PostParams::default(), &namespace).await;
            assert!(created.is_ok(), "{created:?}");
            let accounts: Api<ServiceAccount> = Api::namespaced((*client).clone(), &scratch.name);
            eventually(
                "scratch namespace service account",
                || async { accounts.get_opt(SERVICE_ACCOUNT).await.ok().flatten() },
                Option::is_some,
            )
            .await;
        }
        scratch
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for ScratchNamespace {
    fn drop(&mut self) {
        for context in &self.contexts {
            let deleted = std::process::Command::new(KUBECTL)
                .args(["--context", context, "delete", "namespace", &self.name])
                .args(["--wait=false", "--ignore-not-found"])
                .status();
            if !deleted.is_ok_and(|status| status.success()) {
                eprintln!("could not delete namespace {} on {context}", self.name);
            }
        }
    }
}
