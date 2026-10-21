use crate::error::Error;

const HTTP_FORBIDDEN: u16 = 403;
const HTTP_NOT_FOUND: u16 = 404;
const HTTP_CONFLICT: u16 = 409;
const HTTP_INVALID: u16 = 422;
const HTTP_TOO_MANY_REQUESTS: u16 = 429;
const HTTP_SERVER_ERRORS: std::ops::Range<u16> = 500..600;
const MAX_REASON_CHARS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionFailureKind {
    Forbidden,
    NotFound,
    Conflict,
    Invalid,
    Blocked,
    Other,
}

impl ActionFailureKind {
    fn label(self) -> Option<&'static str> {
        match self {
            Self::Forbidden => Some("Forbidden"),
            Self::NotFound => Some("Not found"),
            Self::Conflict => Some("Conflict"),
            Self::Invalid => Some("Invalid"),
            Self::Blocked => Some("Blocked"),
            Self::Other => None,
        }
    }

    fn of_code(code: u16) -> Self {
        match code {
            HTTP_FORBIDDEN => Self::Forbidden,
            HTTP_NOT_FOUND => Self::NotFound,
            HTTP_CONFLICT => Self::Conflict,
            HTTP_INVALID => Self::Invalid,
            HTTP_TOO_MANY_REQUESTS => Self::Blocked,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionFailure {
    pub kind: ActionFailureKind,
    pub reason: String,
}

impl ActionFailure {
    pub fn line(&self) -> String {
        match self.kind.label() {
            Some(label) => format!("{label}: {}", self.reason),
            None => self.reason.clone(),
        }
    }
}

fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    line.chars().take(MAX_REASON_CHARS).collect()
}

pub fn is_transient(error: &Error) -> bool {
    matches!(error, Error::Kube(kube::Error::Api(status)) if HTTP_SERVER_ERRORS.contains(&status.code))
}

pub fn action_failure(error: &Error) -> ActionFailure {
    if let Error::Kube(kube::Error::Api(status)) = error {
        let message = if status.message.is_empty() {
            status.reason.as_str()
        } else {
            status.message.as_str()
        };
        return ActionFailure {
            kind: ActionFailureKind::of_code(status.code),
            reason: first_line(message),
        };
    }
    ActionFailure {
        kind: ActionFailureKind::Other,
        reason: first_line(&error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use kube::core::Status;

    use super::*;

    fn api(code: u16, message: &str, reason: &str) -> Error {
        let status = Status::failure(message, reason).with_code(code);
        Error::Kube(kube::Error::Api(status.boxed()))
    }

    #[test]
    fn only_server_errors_are_transient() {
        let timeout = api(500, "etcdserver: request timed out", "InternalError");
        assert!(is_transient(&timeout));
        let unavailable = api(
            503,
            "the server is currently unable to handle the request",
            "",
        );
        assert!(is_transient(&unavailable));
        let conflict = api(
            HTTP_CONFLICT,
            "Precondition failed: UID mismatch",
            "Conflict",
        );
        assert!(!is_transient(&conflict));
        let throttled = api(
            HTTP_TOO_MANY_REQUESTS,
            "disruption budget",
            "TooManyRequests",
        );
        assert!(!is_transient(&throttled));
        assert!(!is_transient(&Error::NotConnected));
    }

    #[test]
    fn api_errors_are_classified_by_status_code() {
        let forbidden = action_failure(&api(
            HTTP_FORBIDDEN,
            "nodes \"n1\" is forbidden: User \"dev\" cannot patch resource \"nodes\"",
            "Forbidden",
        ));
        assert_eq!(forbidden.kind, ActionFailureKind::Forbidden);
        assert_eq!(
            forbidden.line(),
            "Forbidden: nodes \"n1\" is forbidden: User \"dev\" cannot patch resource \"nodes\""
        );
        let missing = action_failure(&api(HTTP_NOT_FOUND, "pods \"x\" not found", "NotFound"));
        assert_eq!(missing.kind, ActionFailureKind::NotFound);
        let conflict = action_failure(&api(HTTP_CONFLICT, "", "Conflict"));
        assert_eq!(conflict.line(), "Conflict: Conflict");
        let blocked = action_failure(&api(
            HTTP_TOO_MANY_REQUESTS,
            "Cannot evict pod as it would violate the pod's disruption budget.",
            "TooManyRequests",
        ));
        assert_eq!(blocked.kind, ActionFailureKind::Blocked);
    }

    #[test]
    fn other_errors_keep_their_first_line() {
        let failure = action_failure(&Error::NoReadyPod("podinfo".into()));
        assert_eq!(failure.kind, ActionFailureKind::Other);
        assert_eq!(failure.line(), "no ready pod backs svc/podinfo");
        let multi = action_failure(&Error::Exec("timed out\n  retrying".into()));
        assert_eq!(multi.reason, "exec: timed out");
    }
}
