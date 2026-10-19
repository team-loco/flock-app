use std::time::Duration;

use flock_core::{LogLevel, LogLine, LogRequest};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use k8s_openapi::api::core::v1::{ContainerState, Pod};
use kube::Api;

use super::failure::action_failure;
use crate::error::{Error, Result};
use crate::manager::ClusterHandle;
use crate::ops;

pub(super) const TAIL_LINES: i64 = 500;
const CLOSE_POLL: Duration = Duration::from_millis(500);
const TIME_FORMAT: &str = "%H:%M:%S%.3f";
const LEVEL_KEYS: [&str; 6] = [
    "\"level\":\"",
    "\"lvl\":\"",
    "\"severity\":\"",
    "level=",
    "lvl=",
    "severity=",
];
const KLOG_DIGITS: usize = 4;

fn level_word(word: &str) -> Option<LogLevel> {
    match word {
        "trace" => Some(LogLevel::Debug),
        "debug" => Some(LogLevel::Debug),
        "dbg" => Some(LogLevel::Debug),
        "info" => Some(LogLevel::Info),
        "inf" => Some(LogLevel::Info),
        "notice" => Some(LogLevel::Info),
        "warn" => Some(LogLevel::Warn),
        "warning" => Some(LogLevel::Warn),
        "wrn" => Some(LogLevel::Warn),
        "error" => Some(LogLevel::Error),
        "err" => Some(LogLevel::Error),
        "fatal" => Some(LogLevel::Fatal),
        "panic" => Some(LogLevel::Fatal),
        "critical" => Some(LogLevel::Fatal),
        "crit" => Some(LogLevel::Fatal),
        _ => None,
    }
}

fn klog_level(token: &str) -> Option<LogLevel> {
    let mut chars = token.chars();
    let level = match chars.next()? {
        'I' => LogLevel::Info,
        'W' => LogLevel::Warn,
        'E' => LogLevel::Error,
        'F' => LogLevel::Fatal,
        _ => return None,
    };
    let digits = chars.take(KLOG_DIGITS).filter(char::is_ascii_digit).count();
    (digits == KLOG_DIGITS).then_some(level)
}

pub fn detect_level(message: &str) -> Option<LogLevel> {
    let lower = message.to_ascii_lowercase();
    for key in LEVEL_KEYS {
        if let Some(start) = lower.find(key) {
            let rest = lower.get(start + key.len()..).unwrap_or_default();
            let word: String = rest
                .trim_start_matches('"')
                .chars()
                .take_while(char::is_ascii_alphabetic)
                .collect();
            if let Some(level) = level_word(&word) {
                return Some(level);
            }
        }
    }
    let first = message.split_whitespace().next()?;
    if let Some(level) = klog_level(first) {
        return Some(level);
    }
    let word = first
        .trim_matches(|c: char| !c.is_ascii_alphabetic())
        .to_ascii_lowercase();
    level_word(&word)
}

pub fn parse_stamped(raw: &str, zone: &TimeZone) -> (Option<Timestamp>, LogLine) {
    let stamped = raw
        .split_once(' ')
        .and_then(|(stamp, rest)| stamp.parse::<Timestamp>().ok().map(|at| (at, rest)));
    let (at, message) = match stamped {
        Some((at, rest)) => (Some(at), rest),
        None => (None, raw),
    };
    let time = at.map(|at| at.to_zoned(zone.clone()).strftime(TIME_FORMAT).to_string());
    let line = LogLine {
        time,
        level: detect_level(message),
        message: message.to_owned(),
    };
    (at, line)
}

pub fn parse_line(raw: &str, zone: &TimeZone) -> LogLine {
    parse_stamped(raw, zone).1
}

pub fn marker(text: &str) -> LogLine {
    LogLine::marker(format!("── {text} ──"))
}

pub fn exit_marker(pod: &Pod, container: &str) -> Option<LogLine> {
    let status = pod
        .status
        .as_ref()?
        .container_statuses
        .as_ref()?
        .iter()
        .find(|status| status.name == container)?;
    let state: &ContainerState = status.state.as_ref()?;
    let terminated = state.terminated.as_ref()?;
    let reason = terminated
        .reason
        .as_deref()
        .map(|reason| format!(" ({reason})"))
        .unwrap_or_default();
    let text = format!(
        "container exited {}{reason} · restart {}",
        terminated.exit_code, status.restart_count
    );
    Some(marker(&text))
}

fn ops_request(request: &LogRequest) -> Result<ops::LogRequest> {
    let namespace = request
        .pod
        .namespace
        .clone()
        .ok_or(Error::MissingField("metadata.namespace"))?;
    Ok(ops::LogRequest {
        namespace,
        pod: request.pod.name.clone(),
        container: Some(request.container.clone()),
        follow: true,
        tail_lines: Some(TAIL_LINES),
        timestamps: true,
        since_seconds: None,
    })
}

async fn pump(handle: &ClusterHandle, request: &LogRequest) -> Result<()> {
    let params = ops_request(request)?;
    let stream = handle.logs(&params).await?;
    let zone = TimeZone::system();
    let mut poll = tokio::time::interval(CLOSE_POLL);
    loop {
        tokio::select! {
            line = stream.lines.recv() => {
                let Ok(line) = line else { break };
                let line = parse_line(&line?, &zone);
                if !request.sink.send_blocking(line) {
                    return Ok(());
                }
            }
            _ = poll.tick() => {
                if request.sink.is_closed() {
                    return Ok(());
                }
            }
        }
    }
    let pods: Api<Pod> = Api::namespaced(handle.client()?, &params.namespace);
    let pod = pods.get(&params.pod).await?;
    if let Some(line) = exit_marker(&pod, &request.container) {
        request.sink.send_blocking(line);
    }
    Ok(())
}

pub async fn stream(handle: ClusterHandle, request: LogRequest) {
    let pumped = handle.until_closed(pump(&handle, &request)).await;
    let outcome = pumped.unwrap_or(Err(Error::NotConnected));
    if let Err(error) = outcome {
        let failure = action_failure(&error);
        request.sink.send_blocking(marker(&failure.reason));
    }
}

#[cfg(test)]
mod tests {
    use k8s_openapi::api::core::v1::{ContainerStateTerminated, ContainerStatus, PodStatus};

    use super::*;

    #[test]
    fn timestamps_become_local_clock_times() {
        let line = parse_line(
            "2026-10-13T21:12:20.114523891Z {\"level\":\"error\",\"msg\":\"boom\"}",
            &TimeZone::UTC,
        );
        assert_eq!(line.time.as_deref(), Some("21:12:20.114"));
        assert_eq!(line.level, Some(LogLevel::Error));
        assert_eq!(line.message, "{\"level\":\"error\",\"msg\":\"boom\"}");
    }

    #[test]
    fn lines_without_a_timestamp_are_kept_whole() {
        let line = parse_line("plain text here", &TimeZone::UTC);
        assert_eq!(line.time, None);
        assert_eq!(line.message, "plain text here");
        assert_eq!(line.level, None);
    }

    #[test]
    fn levels_come_from_common_formats() {
        assert_eq!(
            detect_level("ts=1 level=warn msg=slow"),
            Some(LogLevel::Warn)
        );
        assert_eq!(
            detect_level("I1013 21:12:20.114 main.go:12] started"),
            Some(LogLevel::Info)
        );
        assert_eq!(detect_level("[ERROR] open failed"), Some(LogLevel::Error));
        assert_eq!(detect_level("FATAL: config invalid"), Some(LogLevel::Fatal));
        assert_eq!(
            detect_level("{\"severity\":\"DEBUG\"}"),
            Some(LogLevel::Debug)
        );
        assert_eq!(detect_level("GET /healthz 200"), None);
        assert_eq!(detect_level("Install done"), None);
    }

    fn pod(last: Option<ContainerStateTerminated>) -> Pod {
        Pod {
            status: Some(PodStatus {
                container_statuses: Some(vec![ContainerStatus {
                    name: "api".into(),
                    restart_count: 14,
                    state: last.map(|terminated| ContainerState {
                        terminated: Some(terminated),
                        ..ContainerState::default()
                    }),
                    ..ContainerStatus::default()
                }]),
                ..PodStatus::default()
            }),
            ..Pod::default()
        }
    }

    #[test]
    fn terminated_logs_end_with_how_the_container_exited() {
        let crashed = pod(Some(ContainerStateTerminated {
            exit_code: 1,
            reason: Some("Error".into()),
            ..ContainerStateTerminated::default()
        }));
        assert_eq!(
            exit_marker(&crashed, "api"),
            Some(LogLine::marker(
                "── container exited 1 (Error) · restart 14 ──"
            ))
        );
        assert_eq!(exit_marker(&crashed, "sidecar"), None);
        assert_eq!(exit_marker(&pod(None), "api"), None);
    }

    #[test]
    fn requests_follow_the_current_container() {
        let mut request = LogRequest {
            pod: flock_core::ResourceRef {
                cluster: flock_core::ClusterId::new("kind-fleet-a"),
                kind: "Pod".into(),
                namespace: Some("payments".into()),
                name: "checkout-api-1".into(),
            },
            container: "api".into(),
            sink: flock_core::Sink::channel().0,
        };
        let live = ops_request(&request).ok();
        assert_eq!(
            live.as_ref().map(|r| (r.follow, r.timestamps)),
            Some((true, true))
        );
        assert_eq!(live.and_then(|r| r.container).as_deref(), Some("api"));
        request.pod.namespace = None;
        assert!(ops_request(&request).is_err());
    }
}
