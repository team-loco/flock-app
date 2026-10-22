use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::Duration;

use flock_core::{ExecEvent, NodeShellRequest};
use jiff::Timestamp;
use k8s_openapi::api::core::v1::{
    Container, HostPathVolumeSource, Pod, PodSpec, SecurityContext, Toleration, Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{DeleteParams, ListParams, Patch, PatchParams, PostParams};
use kube::{Api, ResourceExt};

use super::exec::{
    self, DEBUG_POLL, DEBUG_START_TIMEOUT, DebugState, PROMPT_NUDGE, Streams, container_state,
    exit_code, seed, suffix,
};
use super::failure::action_failure;
use crate::clock;
use crate::error::{Error, Result};
use crate::manager::ClusterHandle;
use crate::ops::{ExecExit, ExecSession, FIELD_MANAGER};

pub const NAMESPACE: &str = "default";
pub const SHELL_LABEL: &str = "flock.dev/node-shell";
pub const HEARTBEAT_ANNOTATION: &str = "flock.dev/heartbeat";
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
pub const STALE_TIMEOUT: Duration = Duration::from_secs(180);
const NAME_PREFIX: &str = "node-shell-";
const CONTAINER: &str = "shell";
const HOST_VOLUME: &str = "host-root";
const HOST_ROOT: &str = "/";
const HOST_MOUNT: &str = "/host";
const NEVER: &str = "Never";
const TOLERATE_ALL: &str = "Exists";
const FAILED_PHASE: &str = "Failed";
const SHELL: [&str; 10] = [
    "nsenter", "-t", "1", "-m", "-u", "-i", "-n", "-p", "--", "sh",
];
const FATAL_WAITING: [&str; 3] = ["ErrImagePull", "ImagePullBackOff", "InvalidImageName"];

static RUN: LazyLock<String> = LazyLock::new(|| suffix(seed()));

pub fn pod_name(node: &str, seed: u128) -> String {
    format!("{NAME_PREFIX}{node}-{}", suffix(seed))
}

pub fn heartbeat_value(now: Timestamp) -> String {
    now.as_second().to_string()
}

pub fn is_stale(pod: &Pod, now: Timestamp) -> bool {
    let beat = pod
        .annotations()
        .get(HEARTBEAT_ANNOTATION)
        .and_then(|value| value.parse::<i64>().ok())
        .and_then(|seconds| Timestamp::from_second(seconds).ok());
    let silent = match beat {
        Some(beat) => clock::between(beat, now),
        None => clock::age(&pod.metadata, now),
    };
    silent > STALE_TIMEOUT
}

pub fn shell_pod(name: &str, node: &str, image: &str, run: &str, now: Timestamp) -> Pod {
    let annotations = BTreeMap::from([(HEARTBEAT_ANNOTATION.to_owned(), heartbeat_value(now))]);
    let labels = BTreeMap::from([(SHELL_LABEL.to_owned(), run.to_owned())]);
    let command = SHELL.iter().map(|part| (*part).to_owned()).collect();
    let container = Container {
        name: CONTAINER.to_owned(),
        image: Some(image.to_owned()),
        command: Some(command),
        stdin: Some(true),
        tty: Some(true),
        security_context: Some(SecurityContext {
            privileged: Some(true),
            ..SecurityContext::default()
        }),
        volume_mounts: Some(vec![VolumeMount {
            name: HOST_VOLUME.to_owned(),
            mount_path: HOST_MOUNT.to_owned(),
            ..VolumeMount::default()
        }]),
        ..Container::default()
    };
    let host = Volume {
        name: HOST_VOLUME.to_owned(),
        host_path: Some(HostPathVolumeSource {
            path: HOST_ROOT.to_owned(),
            ..HostPathVolumeSource::default()
        }),
        ..Volume::default()
    };
    let tolerate_all = Toleration {
        operator: Some(TOLERATE_ALL.to_owned()),
        ..Toleration::default()
    };
    Pod {
        metadata: ObjectMeta {
            name: Some(name.to_owned()),
            labels: Some(labels),
            annotations: Some(annotations),
            ..ObjectMeta::default()
        },
        spec: Some(PodSpec {
            node_name: Some(node.to_owned()),
            host_pid: Some(true),
            host_network: Some(true),
            host_ipc: Some(true),
            restart_policy: Some(NEVER.to_owned()),
            tolerations: Some(vec![tolerate_all]),
            containers: vec![container],
            volumes: Some(vec![host]),
            ..PodSpec::default()
        }),
        ..Pod::default()
    }
}

pub fn stale_selector(run: &str) -> String {
    format!("{SHELL_LABEL},{SHELL_LABEL}!={run}")
}

pub fn shell_state(pod: &Pod) -> DebugState {
    let status = pod.status.as_ref();
    if status.and_then(|status| status.phase.as_deref()) == Some(FAILED_PHASE) {
        let reason = status
            .and_then(|status| status.message.clone().or_else(|| status.reason.clone()))
            .unwrap_or_else(|| FAILED_PHASE.to_owned());
        return DebugState::Exited(reason);
    }
    let statuses = status.and_then(|status| status.container_statuses.as_deref());
    match container_state(statuses, CONTAINER) {
        DebugState::Waiting(Some(reason))
            if FATAL_WAITING.iter().any(|fatal| reason.starts_with(fatal)) =>
        {
            DebugState::Exited(reason)
        }
        state => state,
    }
}

async fn remove_stale(pods: &Api<Pod>) {
    let selector = stale_selector(&RUN);
    let listed = match pods.list(&ListParams::default().labels(&selector)).await {
        Ok(listed) => listed,
        Err(error) => {
            tracing::warn!(%error, "listing stale node shells failed");
            return;
        }
    };
    let now = Timestamp::now();
    for pod in listed.items.iter().filter(|pod| is_stale(pod, now)) {
        remove(pods, &pod.name_any()).await;
    }
}

async fn remove(pods: &Api<Pod>, name: &str) {
    let params = DeleteParams {
        grace_period_seconds: Some(0),
        ..DeleteParams::default()
    };
    if let Err(error) = pods.delete(name, &params).await {
        tracing::debug!(%error, name, "deleting node shell pod failed");
    }
}

async fn beat(pods: &Api<Pod>, name: &str) {
    loop {
        tokio::time::sleep(HEARTBEAT_INTERVAL).await;
        let value = heartbeat_value(Timestamp::now());
        let patch = serde_json::json!({"metadata": {"annotations": {HEARTBEAT_ANNOTATION: value}}});
        let params = PatchParams::default();
        if let Err(error) = pods.patch(name, &params, &Patch::Merge(&patch)).await {
            tracing::debug!(%error, name, "node shell heartbeat failed");
        }
    }
}

async fn wait_running(pods: &Api<Pod>, name: &str) -> Result<()> {
    let deadline = tokio::time::Instant::now() + DEBUG_START_TIMEOUT;
    let mut last = None;
    loop {
        match shell_state(&pods.get(name).await?) {
            DebugState::Running => return Ok(()),
            DebugState::Exited(reason) => return Err(Error::NodeShell(reason)),
            DebugState::Waiting(reason) => last = reason.or(last),
        }
        if tokio::time::Instant::now() >= deadline {
            let reason = last.unwrap_or_else(|| Error::Timeout(DEBUG_START_TIMEOUT).to_string());
            return Err(Error::NodeShell(reason));
        }
        tokio::time::sleep(DEBUG_POLL).await;
    }
}

async fn start(
    handle: &ClusterHandle,
    pods: &Api<Pod>,
    name: &str,
    request: &NodeShellRequest,
) -> Result<ExecSession> {
    remove_stale(pods).await;
    let pod = shell_pod(
        name,
        &request.node.name,
        &request.image,
        &RUN,
        Timestamp::now(),
    );
    let params = PostParams {
        field_manager: Some(FIELD_MANAGER.to_owned()),
        ..PostParams::default()
    };
    pods.create(&params, &pod).await?;
    wait_running(pods, name).await?;
    let session = handle.attach(NAMESPACE, name, CONTAINER).await?;
    session.stdin.send(PROMPT_NUDGE.to_vec()).await.ok();
    Ok(session)
}

async fn session(handle: &ClusterHandle, pods: &Api<Pod>, name: &str, request: &NodeShellRequest) {
    let output = &request.output;
    let started = tokio::select! {
        result = start(handle, pods, name, request) => result,
        () = output.closed() => return,
    };
    let session = match started {
        Ok(session) => session,
        Err(error) => {
            exec::fail(output, &action_failure(&error).reason);
            return;
        }
    };
    let note = format!("Pod: {NAMESPACE}/{name}");
    if !output.send_blocking(ExecEvent::Started { note }) {
        return;
    }
    let streams = Streams {
        input: &request.input,
        resize: &request.resize,
        output,
    };
    let Some(exit) = exec::pump(&session, streams).await else {
        return;
    };
    if let ExecExit::Failed(message) = &exit {
        exec::fail(output, message);
        return;
    }
    output.send_blocking(ExecEvent::Exited {
        code: exit_code(&exit),
    });
}

pub async fn run(handle: ClusterHandle, request: NodeShellRequest) {
    let client = match handle.client() {
        Ok(client) => client,
        Err(error) => {
            exec::fail(&request.output, &error.to_string());
            return;
        }
    };
    let pods: Api<Pod> = Api::namespaced(client, NAMESPACE);
    let name = pod_name(&request.node.name, seed());
    let finished = tokio::select! {
        finished = handle.until_closed(session(&handle, &pods, &name, &request)) => finished,
        () = beat(&pods, &name) => None,
    };
    if finished.is_none() {
        exec::fail(&request.output, &Error::NotConnected.to_string());
    }
    remove(&pods, &name).await;
}

#[cfg(test)]
mod tests {
    use k8s_openapi::api::core::v1::{
        ContainerState, ContainerStateRunning, ContainerStateWaiting, ContainerStatus, PodStatus,
    };
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;

    use super::*;

    fn spec(pod: &Pod) -> &PodSpec {
        let Some(spec) = pod.spec.as_ref() else {
            panic!("pod has no spec");
        };
        spec
    }

    fn with_container(state: ContainerState) -> Pod {
        Pod {
            status: Some(PodStatus {
                container_statuses: Some(vec![ContainerStatus {
                    name: CONTAINER.into(),
                    state: Some(state),
                    ..ContainerStatus::default()
                }]),
                ..PodStatus::default()
            }),
            ..Pod::default()
        }
    }

    #[test]
    fn the_pod_is_pinned_to_the_node_and_shares_its_namespaces() {
        let pod = shell_pod(
            "node-shell-n1-abcde",
            "n1",
            "busybox:1.36",
            "run01",
            at(1_000),
        );
        let spec = spec(&pod);
        assert_eq!(spec.node_name.as_deref(), Some("n1"));
        assert_eq!(spec.host_pid, Some(true));
        assert_eq!(spec.host_network, Some(true));
        assert_eq!(spec.host_ipc, Some(true));
        assert_eq!(spec.restart_policy.as_deref(), Some("Never"));
        assert_eq!(pod.metadata.name.as_deref(), Some("node-shell-n1-abcde"));
        assert_eq!(
            pod.labels().get(SHELL_LABEL).map(String::as_str),
            Some("run01")
        );
    }

    #[test]
    fn the_pod_tolerates_every_taint() {
        let pod = shell_pod("p", "n1", "busybox:1.36", "run01", at(1_000));
        let Some(tolerations) = spec(&pod).tolerations.as_ref() else {
            panic!("no tolerations");
        };
        assert_eq!(tolerations.len(), 1);
        assert_eq!(tolerations[0].operator.as_deref(), Some("Exists"));
        assert_eq!(tolerations[0].key, None);
        assert_eq!(tolerations[0].effect, None);
    }

    #[test]
    fn the_container_is_privileged_and_enters_the_host() {
        let pod = shell_pod("p", "n1", "busybox:1.36", "run01", at(1_000));
        let spec = spec(&pod);
        let Some(container) = spec.containers.first() else {
            panic!("no container");
        };
        assert_eq!(container.image.as_deref(), Some("busybox:1.36"));
        assert_eq!(
            container
                .security_context
                .as_ref()
                .and_then(|context| context.privileged),
            Some(true)
        );
        assert_eq!(container.stdin, Some(true));
        assert_eq!(container.tty, Some(true));
        assert_eq!(
            container
                .command
                .as_deref()
                .map(|command| command.join(" ")),
            Some("nsenter -t 1 -m -u -i -n -p -- sh".to_owned())
        );
        let mount = container
            .volume_mounts
            .as_ref()
            .and_then(|mounts| mounts.first());
        assert_eq!(mount.map(|mount| mount.mount_path.as_str()), Some("/host"));
        let host = spec
            .volumes
            .as_ref()
            .and_then(|volumes| volumes.first())
            .and_then(|volume| volume.host_path.as_ref());
        assert_eq!(host.map(|host| host.path.as_str()), Some("/"));
    }

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_second(seconds).unwrap_or_default()
    }

    fn beating(seconds: &str) -> Pod {
        let mut pod = Pod::default();
        pod.metadata.annotations = Some(BTreeMap::from([(
            HEARTBEAT_ANNOTATION.to_owned(),
            seconds.to_owned(),
        )]));
        pod
    }

    fn created(seconds: i64) -> Pod {
        let mut pod = Pod::default();
        pod.metadata.creation_timestamp = Some(Time(at(seconds)));
        pod
    }

    #[test]
    fn a_fresh_heartbeat_is_alive_and_an_old_one_is_stale() {
        let now = at(10_000);
        let limit = i64::try_from(STALE_TIMEOUT.as_secs()).unwrap_or_default();
        assert!(!is_stale(&beating("9990"), now));
        assert!(!is_stale(&beating(&(10_000 - limit).to_string()), now));
        assert!(is_stale(&beating(&(10_000 - limit - 1).to_string()), now));
    }

    #[test]
    fn without_a_heartbeat_the_creation_time_decides() {
        let now = at(10_000);
        let limit = i64::try_from(STALE_TIMEOUT.as_secs()).unwrap_or_default();
        assert!(!is_stale(&created(9_990), now));
        assert!(is_stale(&created(10_000 - limit - 1), now));
        assert!(!is_stale(&Pod::default(), now));
        assert!(is_stale(
            &{
                let mut pod = created(0);
                pod.metadata.annotations = Some(BTreeMap::from([(
                    HEARTBEAT_ANNOTATION.to_owned(),
                    "garbage".to_owned(),
                )]));
                pod
            },
            now
        ));
    }

    #[test]
    fn new_pods_start_with_a_heartbeat() {
        let pod = shell_pod("n", "n1", "img", "run01", at(1_234));
        assert_eq!(
            pod.annotations()
                .get(HEARTBEAT_ANNOTATION)
                .map(String::as_str),
            Some("1234")
        );
    }

    #[test]
    fn names_carry_the_node_and_stale_pods_are_other_runs() {
        let name = pod_name("kind-control-plane", 7);
        assert!(name.starts_with("node-shell-kind-control-plane-"));
        assert_ne!(pod_name("n1", 1), pod_name("n1", 2));
        assert_eq!(
            stale_selector("run01"),
            "flock.dev/node-shell,flock.dev/node-shell!=run01"
        );
    }

    #[test]
    fn image_pull_failures_end_the_wait_and_running_attaches() {
        let pulling = with_container(ContainerState {
            waiting: Some(ContainerStateWaiting {
                reason: Some("ErrImagePull".into()),
                message: Some("failed to pull \"nope:0\"".into()),
            }),
            ..ContainerState::default()
        });
        assert!(matches!(
            shell_state(&pulling),
            DebugState::Exited(reason) if reason == "ErrImagePull: failed to pull \"nope:0\""
        ));
        let creating = with_container(ContainerState {
            waiting: Some(ContainerStateWaiting {
                reason: Some("ContainerCreating".into()),
                message: None,
            }),
            ..ContainerState::default()
        });
        assert!(matches!(
            shell_state(&creating),
            DebugState::Waiting(Some(reason)) if reason == "ContainerCreating"
        ));
        let running = with_container(ContainerState {
            running: Some(ContainerStateRunning::default()),
            ..ContainerState::default()
        });
        assert!(matches!(shell_state(&running), DebugState::Running));
        assert!(matches!(
            shell_state(&Pod::default()),
            DebugState::Waiting(None)
        ));
    }

    #[test]
    fn failed_pods_report_their_message() {
        let pod = Pod {
            status: Some(PodStatus {
                phase: Some("Failed".into()),
                message: Some("Pod was rejected: node(s) didn't match".into()),
                ..PodStatus::default()
            }),
            ..Pod::default()
        };
        assert!(matches!(
            shell_state(&pod),
            DebugState::Exited(reason) if reason == "Pod was rejected: node(s) didn't match"
        ));
    }
}
