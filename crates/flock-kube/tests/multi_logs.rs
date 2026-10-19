use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use flock_core::{
    ClusterId, LogEvent, LogSource, LogTarget, MultiLogRequest, ResourceRef, Sink, SourceId,
};
use flock_kube::actions::multi_logs;
use flock_kube::discovery::{DiscoverySources, KubeContext, discover};
use flock_kube::manager::{ClusterHandle, ClusterManager, ConnectionStatus};
use flock_kube::settings::KubeSettings;
use flock_kube::shell_env::ShellEnv;
use k8s_openapi::api::apps::v1::Deployment;
use kube::Api;
use kube::api::{DeleteParams, Patch, PatchParams, PostParams};
use serde_json::json;

mod common;

use common::kubeconfig::kind_only_home;
use common::{WAIT, eventually};

const E2E_VAR: &str = "FLOCK_E2E";
const CONTEXT: &str = "kind-fleet-a";
const NAMESPACE: &str = "default";
const DEPLOYMENT: &str = "flock-multilog-e2e";
const IMAGE: &str = "busybox:1.37";
const REPLICAS: i32 = 2;
const SCALED: i32 = 3;

fn enabled() -> bool {
    std::env::var(E2E_VAR).is_ok_and(|value| value == "1")
}

fn kind_context(sources: &DiscoverySources) -> Option<KubeContext> {
    discover(sources)
        .contexts
        .into_iter()
        .find(|context| context.id.context == CONTEXT)
}

fn deployment() -> Deployment {
    let script = "i=0; while true; do echo \"tick $(hostname) $i\"; i=$((i+1)); sleep 1; done";
    let Ok(deployment) = serde_json::from_value(json!({
        "apiVersion": "apps/v1",
        "kind": "Deployment",
        "metadata": { "name": DEPLOYMENT, "namespace": NAMESPACE },
        "spec": {
            "replicas": REPLICAS,
            "selector": { "matchLabels": { "app": DEPLOYMENT } },
            "template": {
                "metadata": { "labels": { "app": DEPLOYMENT } },
                "spec": {
                    "terminationGracePeriodSeconds": 0,
                    "containers": [{
                        "name": "ticker",
                        "image": IMAGE,
                        "command": ["sh", "-c", script]
                    }]
                }
            }
        }
    })) else {
        panic!("deployment fixture");
    };
    deployment
}

#[derive(Debug, Default)]
struct Seen {
    sources: BTreeMap<SourceId, LogSource>,
    lines: BTreeMap<SourceId, usize>,
    left: BTreeSet<SourceId>,
}

impl Seen {
    fn record(&mut self, event: LogEvent) {
        match event {
            LogEvent::Joined(id, source) => {
                self.sources.insert(id, source);
            }
            LogEvent::Line(id, Some(_), line) if line.message.starts_with("tick ") => {
                *self.lines.entry(id).or_default() += 1;
            }
            LogEvent::Line(..) => {}
            LogEvent::Ended(_) => {}
            LogEvent::Left(id) => {
                self.left.insert(id);
            }
            LogEvent::Failed(reason) => panic!("stream failed: {reason}"),
        }
    }

    fn pods_with_lines(&self) -> BTreeSet<String> {
        self.lines
            .keys()
            .filter(|id| !self.left.contains(id))
            .filter_map(|id| self.sources.get(id))
            .map(|source| source.pod.clone())
            .collect()
    }
}

async fn connected(manager: &ClusterManager, context: KubeContext) -> ClusterHandle {
    let handle = manager.connect(context);
    eventually(
        "cluster connected",
        || async { handle.status() },
        |status| *status == ConnectionStatus::Connected,
    )
    .await;
    handle
}

async fn scale(api: &Api<Deployment>, replicas: i32) {
    let patch = json!({ "spec": { "replicas": replicas } });
    let params = PatchParams::default();
    let patched = api.patch(DEPLOYMENT, &params, &Patch::Merge(&patch)).await;
    assert!(patched.is_ok(), "{patched:?}");
}

async fn until(
    what: &str,
    events: &async_channel::Receiver<LogEvent>,
    seen: &mut Seen,
    done: impl Fn(&Seen) -> bool,
) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !done(seen) {
        let received = tokio::time::timeout_at(deadline, events.recv()).await;
        let Ok(Ok(event)) = received else {
            panic!("{what} not seen within {WAIT:?}; seen: {seen:#?}");
        };
        seen.record(event);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn multi_pod_logs_follow_a_deployment_as_it_scales() {
    if !enabled() {
        return;
    }
    let user = DiscoverySources {
        kubeconfig_env: std::env::var_os("KUBECONFIG"),
        home: None,
        folders: Vec::new(),
    };
    let Some(user_context) = kind_context(&user) else {
        panic!("{CONTEXT} context required");
    };
    let home = kind_only_home(std::slice::from_ref(&user_context));
    let isolated = DiscoverySources {
        kubeconfig_env: None,
        home: Some(home.path().to_path_buf()),
        folders: Vec::new(),
    };
    let Some(context) = kind_context(&isolated) else {
        panic!("{CONTEXT} in the temporary kubeconfig");
    };
    let env = Arc::new(ShellEnv::from_process(String::new()));
    let runtime = tokio::runtime::Handle::current();
    let (manager, _events) = ClusterManager::new(runtime, KubeSettings::default(), env);
    let handle = connected(&manager, context).await;
    let Ok(client) = handle.client() else {
        panic!("client after connect");
    };
    let api: Api<Deployment> = Api::namespaced(client, NAMESPACE);
    let _ = api.delete(DEPLOYMENT, &DeleteParams::default()).await;
    eventually(
        "previous deployment gone",
        || async { api.get_opt(DEPLOYMENT).await.ok().flatten() },
        Option::is_none,
    )
    .await;
    let created = api.create(&PostParams::default(), &deployment()).await;
    assert!(created.is_ok(), "{created:?}");

    let (sink, events) = Sink::channel();
    let target = LogTarget::Workload(ResourceRef {
        cluster: ClusterId::new(CONTEXT),
        kind: "Deployment".into(),
        namespace: Some(NAMESPACE.into()),
        name: DEPLOYMENT.into(),
    });
    let request = MultiLogRequest { target, sink };
    let streaming = tokio::spawn(multi_logs::stream(handle.clone(), request));
    let mut seen = Seen::default();
    let replicas = usize::try_from(REPLICAS).unwrap_or_default();
    until("lines from both replicas", &events, &mut seen, |seen| {
        seen.pods_with_lines().len() >= replicas
    })
    .await;
    let before = seen.pods_with_lines();

    scale(&api, SCALED).await;
    let scaled = usize::try_from(SCALED).unwrap_or_default();
    until("lines from the new pod", &events, &mut seen, |seen| {
        seen.pods_with_lines().len() >= scaled
    })
    .await;
    let joined: Vec<String> = seen
        .pods_with_lines()
        .difference(&before)
        .cloned()
        .collect();
    assert_eq!(joined.len(), 1, "{seen:#?}");

    scale(&api, REPLICAS).await;
    until(
        "a pod leaving after scale-down",
        &events,
        &mut seen,
        |seen| !seen.left.is_empty(),
    )
    .await;

    drop(events);
    let _ = tokio::time::timeout(WAIT, streaming).await;
    let deleted = api.delete(DEPLOYMENT, &DeleteParams::default()).await;
    assert!(deleted.is_ok(), "{deleted:?}");
}
