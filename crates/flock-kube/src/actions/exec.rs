use std::time::Duration;

use flock_core::{ExecEvent, ExecRequest, Sink};
use k8s_openapi::api::core::v1::Pod;
use kube::Api;
use kube::api::{Patch, PatchParams};
use serde_json::{Value, json};

use super::failure::action_failure;
use crate::error::{Error, Result};
use crate::manager::ClusterHandle;
use crate::ops::{self, ExecExit, ExecSession, FIELD_MANAGER};

pub const SHELL_FALLBACK: &str =
    "export TERM=xterm-256color; [ -x /bin/bash ] && exec /bin/bash -i || exec /bin/sh -i";
const SHELL: &str = "/bin/sh";
const SHELL_FLAG: &str = "-c";
const DEBUG_POLL: Duration = Duration::from_millis(500);
const DEBUG_START_TIMEOUT: Duration = Duration::from_secs(90);
const DEBUG_PREFIX: &str = "debugger-";
const DEBUG_SUFFIX_LEN: usize = 5;
const SUFFIX_ALPHABET: &[u8] = b"bcdfghjklmnpqrstvwxz2456789";
const PROMPT_NUDGE: &[u8] = b"\r";
const CRLF: &str = "\r\n";
const EXEC_FAILURE: &str = "exec";
const MISSING_EXECUTABLE: [&str; 2] = ["executable file not found", "no such file or directory"];

pub fn missing_shell(message: &str) -> bool {
    let message = message.to_lowercase();
    message.contains(EXEC_FAILURE)
        && MISSING_EXECUTABLE
            .iter()
            .any(|missing| message.contains(missing))
}

pub fn shell_command() -> Vec<String> {
    vec![SHELL.into(), SHELL_FLAG.into(), SHELL_FALLBACK.into()]
}

pub fn debug_name(seed: u128) -> String {
    let base = SUFFIX_ALPHABET.len() as u128;
    let mut rest = seed;
    let mut suffix = String::with_capacity(DEBUG_SUFFIX_LEN);
    for _ in 0..DEBUG_SUFFIX_LEN {
        let index = usize::try_from(rest % base).unwrap_or_default();
        let letter = SUFFIX_ALPHABET.get(index).copied().unwrap_or(b'x');
        suffix.push(char::from(letter));
        rest /= base;
    }
    format!("{DEBUG_PREFIX}{suffix}")
}

pub fn debug_patch(name: &str, image: &str, target: &str) -> Value {
    json!({
        "spec": {
            "ephemeralContainers": [{
                "name": name,
                "image": image,
                "targetContainerName": target,
                "stdin": true,
                "tty": true,
                "terminationMessagePolicy": "File"
            }]
        }
    })
}

pub enum DebugState {
    Waiting(Option<String>),
    Running,
    Exited(String),
}

pub fn debug_state(pod: &Pod, name: &str) -> DebugState {
    let state = pod
        .status
        .as_ref()
        .and_then(|status| status.ephemeral_container_statuses.as_ref())
        .and_then(|statuses| statuses.iter().find(|status| status.name == name))
        .and_then(|status| status.state.as_ref());
    let Some(state) = state else {
        return DebugState::Waiting(None);
    };
    if state.running.is_some() {
        return DebugState::Running;
    }
    if let Some(terminated) = &state.terminated {
        let reason = terminated
            .message
            .clone()
            .or_else(|| terminated.reason.clone())
            .unwrap_or_else(|| format!("exit {}", terminated.exit_code));
        return DebugState::Exited(reason);
    }
    let waiting = state.waiting.as_ref().and_then(|waiting| {
        let reason = waiting.reason.clone()?;
        Some(match &waiting.message {
            Some(message) => format!("{reason}: {message}"),
            None => reason,
        })
    });
    DebugState::Waiting(waiting)
}

fn exit_code(exit: &ExecExit) -> Option<i32> {
    match exit {
        ExecExit::Success => Some(0),
        ExecExit::Code(code) => Some(*code),
        ExecExit::Failed(_) => None,
    }
}

fn namespace(request: &ExecRequest) -> Result<String> {
    request
        .target
        .pod
        .namespace
        .clone()
        .ok_or(Error::MissingField("metadata.namespace"))
}

async fn start_debug(
    handle: &ClusterHandle,
    request: &ExecRequest,
    image: &str,
) -> Result<(ExecSession, String)> {
    let namespace = namespace(request)?;
    let pod = &request.target.pod.name;
    let pods: Api<Pod> = Api::namespaced(handle.client()?, &namespace);
    let seed = jiff::Timestamp::now().as_nanosecond().unsigned_abs();
    let name = debug_name(seed);
    let patch = debug_patch(&name, image, &request.target.container);
    let params = PatchParams {
        field_manager: Some(FIELD_MANAGER.to_owned()),
        ..PatchParams::default()
    };
    pods.patch_ephemeral_containers(pod, &params, &Patch::Strategic(patch))
        .await?;
    let deadline = tokio::time::Instant::now() + DEBUG_START_TIMEOUT;
    let mut last = None;
    loop {
        match debug_state(&pods.get(pod).await?, &name) {
            DebugState::Running => break,
            DebugState::Exited(reason) => return Err(Error::DebugContainer { name, reason }),
            DebugState::Waiting(reason) => last = reason.or(last),
        }
        if tokio::time::Instant::now() >= deadline {
            let reason = last.unwrap_or_else(|| Error::Timeout(DEBUG_START_TIMEOUT).to_string());
            return Err(Error::DebugContainer { name, reason });
        }
        tokio::time::sleep(DEBUG_POLL).await;
    }
    let session = handle.attach(&namespace, pod, &name).await?;
    session.stdin.send(PROMPT_NUDGE.to_vec()).await.ok();
    Ok((session, name))
}

async fn start(handle: &ClusterHandle, request: &ExecRequest) -> Result<(ExecSession, String)> {
    if let Some(image) = &request.target.debug_image {
        let (session, name) = start_debug(handle, request, image).await?;
        let note = format!(
            "Targeting container \"{}\". Debug container: {name}",
            request.target.container
        );
        return Ok((session, note));
    }
    let exec = ops::ExecRequest {
        namespace: namespace(request)?,
        pod: request.target.pod.name.clone(),
        container: Some(request.target.container.clone()),
        command: shell_command(),
        tty: true,
    };
    let session = handle.exec(&exec).await?;
    let note = format!("Connected to container \"{}\".", request.target.container);
    Ok((session, note))
}

async fn pump(session: &ExecSession, request: &ExecRequest) -> Option<ExecExit> {
    let mut resizing = true;
    loop {
        tokio::select! {
            chunk = session.output.recv() => {
                let Ok(chunk) = chunk else { break };
                if !request.output.send_blocking(ExecEvent::Output(chunk)) {
                    return None;
                }
            }
            bytes = request.input.recv() => {
                let bytes = bytes?;
                if session.stdin.send(bytes).await.is_err() {
                    return None;
                }
            }
            size = request.resize.recv(), if resizing => {
                let sent = match size {
                    Some(size) => session.resize.send(size).await.is_ok(),
                    None => false,
                };
                resizing = sent;
            }
            () = request.output.closed() => return None,
        }
    }
    tokio::select! {
        exit = session.exit.recv() => exit.ok(),
        () = request.output.closed() => None,
    }
}

fn no_shell(output: &Sink<ExecEvent>) {
    output.send_blocking(ExecEvent::NoShell);
    output.send_blocking(ExecEvent::Exited { code: None });
}

pub fn fail(output: &Sink<ExecEvent>, reason: &str) {
    let text = format!("{reason}{CRLF}");
    output.send_blocking(ExecEvent::Output(text.into_bytes()));
    output.send_blocking(ExecEvent::Exited { code: None });
}

pub async fn run(handle: ClusterHandle, request: ExecRequest) {
    if handle
        .until_closed(session(&handle, &request))
        .await
        .is_none()
    {
        fail(&request.output, &Error::NotConnected.to_string());
    }
}

async fn session(handle: &ClusterHandle, request: &ExecRequest) {
    let output = request.output.clone();
    let started = tokio::select! {
        result = start(handle, request) => result,
        () = output.closed() => return,
    };
    let shell = !request.target.is_debug();
    let (session, note) = match started {
        Ok(started) => started,
        Err(error) => {
            let line = action_failure(&error).line();
            if shell && missing_shell(&format!("{error} {line}")) {
                no_shell(&output);
            } else {
                fail(&output, &line);
            }
            return;
        }
    };
    if !output.send_blocking(ExecEvent::Started { note }) {
        return;
    }
    let Some(exit) = pump(&session, request).await else {
        return;
    };
    if let ExecExit::Failed(message) = &exit
        && shell
        && missing_shell(message)
    {
        no_shell(&output);
        return;
    }
    if let ExecExit::Failed(message) = &exit {
        let text = format!("{message}{CRLF}");
        output.send_blocking(ExecEvent::Output(text.into_bytes()));
    }
    output.send_blocking(ExecEvent::Exited {
        code: exit_code(&exit),
    });
}

#[cfg(test)]
mod tests {
    use k8s_openapi::api::core::v1::{
        ContainerState, ContainerStateRunning, ContainerStateTerminated, ContainerStateWaiting,
        ContainerStatus, PodStatus,
    };

    use super::*;

    fn pod_with(name: &str, state: ContainerState) -> Pod {
        Pod {
            status: Some(PodStatus {
                ephemeral_container_statuses: Some(vec![ContainerStatus {
                    name: name.into(),
                    state: Some(state),
                    ..ContainerStatus::default()
                }]),
                ..PodStatus::default()
            }),
            ..Pod::default()
        }
    }

    #[test]
    fn shell_prefers_bash_and_falls_back_to_sh() {
        assert_eq!(
            shell_command(),
            [
                "/bin/sh",
                "-c",
                "export TERM=xterm-256color; [ -x /bin/bash ] && exec /bin/bash -i || exec /bin/sh -i"
            ]
        );
    }

    #[test]
    fn missing_shells_are_told_apart_from_other_exec_failures() {
        let containerd = "error executing command in container: failed to exec in container: failed to start exec \"390235f8\": OCI runtime exec failed: exec failed: unable to start container process: exec: \"/bin/sh\": stat /bin/sh: no such file or directory: unknown";
        let docker = "OCI runtime exec failed: exec failed: container_linux.go:380: starting container process caused: exec: \"/bin/sh\": executable file not found in $PATH: unknown";
        assert!(missing_shell(containerd));
        assert!(missing_shell(docker));
        assert!(!missing_shell(
            "command terminated with non-zero exit code: 127"
        ));
        assert!(!missing_shell("pods \"api\" not found"));
        assert!(!missing_shell("open /data/x: no such file or directory"));
    }

    #[test]
    fn debug_names_look_like_kubectl_debug() {
        let name = debug_name(123_456_789);
        assert!(name.starts_with(DEBUG_PREFIX));
        assert_eq!(name.len(), DEBUG_PREFIX.len() + DEBUG_SUFFIX_LEN);
        assert_ne!(debug_name(1), debug_name(2));
    }

    #[test]
    fn debug_patch_targets_the_container() {
        let patch = debug_patch("debugger-abcde", "busybox:1.36", "api");
        let container = &patch["spec"]["ephemeralContainers"][0];
        assert_eq!(container["targetContainerName"], json!("api"));
        assert_eq!(container["tty"], json!(true));
        assert_eq!(container["image"], json!("busybox:1.36"));
    }

    #[test]
    fn debug_state_follows_the_ephemeral_status() {
        let name = "debugger-abcde";
        assert!(matches!(
            debug_state(&Pod::default(), name),
            DebugState::Waiting(None)
        ));
        let pulling = pod_with(
            name,
            ContainerState {
                waiting: Some(ContainerStateWaiting {
                    reason: Some("ErrImagePull".into()),
                    message: Some("not found".into()),
                }),
                ..ContainerState::default()
            },
        );
        assert!(matches!(
            debug_state(&pulling, name),
            DebugState::Waiting(Some(reason)) if reason == "ErrImagePull: not found"
        ));
        let running = pod_with(
            name,
            ContainerState {
                running: Some(ContainerStateRunning::default()),
                ..ContainerState::default()
            },
        );
        assert!(matches!(debug_state(&running, name), DebugState::Running));
        let exited = pod_with(
            name,
            ContainerState {
                terminated: Some(ContainerStateTerminated {
                    exit_code: 2,
                    ..ContainerStateTerminated::default()
                }),
                ..ContainerState::default()
            },
        );
        assert!(matches!(
            debug_state(&exited, name),
            DebugState::Exited(reason) if reason == "exit 2"
        ));
    }

    #[test]
    fn exit_codes_map_to_the_terminal() {
        assert_eq!(exit_code(&ExecExit::Success), Some(0));
        assert_eq!(exit_code(&ExecExit::Code(127)), Some(127));
        assert_eq!(exit_code(&ExecExit::Failed("x".into())), None);
    }
}
