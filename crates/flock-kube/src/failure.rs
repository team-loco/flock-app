use std::error::Error as StdError;
use std::fmt;
use std::io::ErrorKind;

use flock_flux::redact;
use kube::client::AuthError;

use crate::discovery::KubeContext;
use crate::error::Error;
use crate::provider::{AuthKind, Provider};

const CONTEXT_CHANGED: &str = "The kubeconfig changed since Flock read it; Flock reads it again.";
const HTTP_UNAUTHORIZED: u16 = 401;
const HTTP_FORBIDDEN: u16 = 403;
const HTTP_NOT_FOUND: u16 = 404;
const MAX_DETAIL_CHARS: usize = 160;
const EXPIRED_MARKERS: [&str; 12] = [
    "expiredtoken",
    "expired",
    "sso session",
    "sso login",
    "invalid_grant",
    "reauth",
    "re-auth",
    "login again",
    "log in again",
    "refresh token",
    "gcloud auth login",
    "az login",
];
const UNREACHABLE_MARKERS: [&str; 8] = [
    "connection refused",
    "connect error",
    "tcp connect",
    "dns error",
    "failed to lookup address",
    "network is unreachable",
    "no route to host",
    "timed out",
];
const TLS_MARKERS: [&str; 4] = ["certificate", "tls", "handshake", "unknownissuer"];
const AWS_PROFILE_FLAG: &str = "--profile";
const EXEC_PARSE_FAILED: &str =
    "the credential plugin printed output that is not an ExecCredential";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Unreachable,
    Tls,
    ExpiredCredentials,
    MissingPlugin,
    AuthFailed,
    Unauthorized,
    Forbidden,
    NotFound,
    Timeout,
    Config,
    Other,
}

impl FailureKind {
    pub fn is_auth(self) -> bool {
        matches!(
            self,
            Self::ExpiredCredentials | Self::MissingPlugin | Self::AuthFailed | Self::Unauthorized
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailureKind,
    pub detail: String,
    pub message: String,
    pub remedy: Option<String>,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}

fn chain_text(err: &(dyn StdError + 'static)) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(inner) = source {
        parts.push(inner.to_string());
        source = inner.source();
    }
    parts.join(": ")
}

fn find_in_chain<'a, T: StdError + 'static>(err: &'a (dyn StdError + 'static)) -> Option<&'a T> {
    let mut current: Option<&(dyn StdError + 'static)> = Some(err);
    while let Some(candidate) = current {
        if let Some(found) = candidate.downcast_ref::<T>() {
            return Some(found);
        }
        current = candidate.source();
    }
    None
}

fn kube_error(err: &Error) -> Option<&kube::Error> {
    match err {
        Error::Kube(inner) => Some(inner),
        _ => find_in_chain::<kube::Error>(err),
    }
}

fn find_auth_error(err: &Error) -> Option<&AuthError> {
    if let Some(kube::Error::Auth(auth)) = kube_error(err) {
        return Some(auth);
    }
    find_in_chain::<AuthError>(err)
}

fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    line.chars().take(MAX_DETAIL_CHARS).collect()
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    let lower = haystack.to_ascii_lowercase();
    needles.iter().any(|needle| lower.contains(needle))
}

fn exec_label(auth: &AuthKind) -> String {
    auth.exec()
        .map_or_else(|| auth.to_string(), crate::provider::ExecCommand::summary)
}

fn login_command(context: &KubeContext) -> Option<String> {
    let exec = context.auth.exec();
    match context.provider {
        Provider::Eks => {
            let profile = exec.and_then(|exec| exec.flag_value(AWS_PROFILE_FLAG));
            Some(profile.map_or_else(
                || "aws sso login".to_owned(),
                |profile| format!("aws sso login --profile {profile}"),
            ))
        }
        Provider::Gke => Some("gcloud auth login".to_owned()),
        Provider::Aks => Some("az login".to_owned()),
        Provider::Doks => Some("doctl auth init".to_owned()),
        _ => None,
    }
}

fn install_command(program: &str) -> Option<String> {
    match program {
        "gke-gcloud-auth-plugin" => {
            Some("gcloud components install gke-gcloud-auth-plugin".to_owned())
        }
        "aws" => Some("brew install awscli".to_owned()),
        "kubelogin" => Some("brew install Azure/kubelogin/kubelogin".to_owned()),
        "doctl" => Some("brew install doctl".to_owned()),
        "aws-iam-authenticator" => Some("brew install aws-iam-authenticator".to_owned()),
        _ => None,
    }
}

fn auth_message(auth: &AuthError, context: &KubeContext) -> String {
    let label = exec_label(&context.auth);
    match auth {
        AuthError::AuthExecRun { status, out, .. } => {
            let stderr = redact::credentials(&String::from_utf8_lossy(&out.stderr));
            format!("{label} failed with {status}: {}", stderr.trim())
        }
        AuthError::AuthExecParse(_) => format!("{label}: {EXEC_PARSE_FAILED}"),
        _ => redact::credentials(&chain_text(auth)),
    }
}

fn classify_auth(auth: &AuthError, context: &KubeContext, message: String) -> Failure {
    let label = exec_label(&context.auth);
    match auth {
        AuthError::AuthExecStart(io) if io.kind() == ErrorKind::NotFound => {
            let program = context
                .auth
                .exec()
                .map(|exec| exec.program().to_owned())
                .unwrap_or_default();
            Failure {
                kind: FailureKind::MissingPlugin,
                detail: format!("{program} isn’t installed or isn’t on PATH."),
                remedy: install_command(&program),
                message,
            }
        }
        AuthError::AuthExecRun { status, out, .. } => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let output = if stderr.trim().is_empty() {
                status.to_string()
            } else {
                redact::credentials(&stderr)
            };
            let kind = if contains_any(&output, &EXPIRED_MARKERS) {
                FailureKind::ExpiredCredentials
            } else {
                FailureKind::AuthFailed
            };
            Failure {
                kind,
                detail: format!("{label}: {}", first_line(&output)),
                remedy: login_command(context),
                message,
            }
        }
        AuthError::AuthExecParse(_) => Failure {
            kind: FailureKind::AuthFailed,
            detail: format!("{label}: {EXEC_PARSE_FAILED}"),
            remedy: login_command(context),
            message,
        },
        _ => Failure {
            kind: FailureKind::AuthFailed,
            detail: format!("{label}: {}", first_line(&auth.to_string())),
            remedy: login_command(context),
            message,
        },
    }
}

fn classify_status(status: &kube::core::Status, context: &KubeContext, message: String) -> Failure {
    let detail = first_line(&status.message);
    match status.code {
        HTTP_UNAUTHORIZED => {
            let expired = matches!(context.auth, AuthKind::Exec(_) | AuthKind::Oidc);
            let kind = if expired {
                FailureKind::ExpiredCredentials
            } else {
                FailureKind::Unauthorized
            };
            Failure {
                kind,
                detail,
                remedy: login_command(context),
                message,
            }
        }
        HTTP_FORBIDDEN => Failure {
            kind: FailureKind::Forbidden,
            detail,
            remedy: None,
            message,
        },
        HTTP_NOT_FOUND => Failure {
            kind: FailureKind::NotFound,
            detail,
            remedy: None,
            message,
        },
        _ => Failure {
            kind: FailureKind::Other,
            detail,
            remedy: None,
            message,
        },
    }
}

pub fn classify(err: &Error, context: &KubeContext) -> Failure {
    let message = chain_text(err);
    if let Error::Timeout(after) = err {
        return Failure {
            kind: FailureKind::Timeout,
            detail: format!("No answer within {} s.", after.as_secs()),
            remedy: None,
            message,
        };
    }
    if let Error::ContextChanged(_) = err {
        return Failure {
            kind: FailureKind::Config,
            detail: CONTEXT_CHANGED.to_owned(),
            remedy: None,
            message,
        };
    }
    if let Error::Kubeconfig { .. } = err {
        return Failure {
            kind: FailureKind::Config,
            detail: first_line(&message),
            remedy: None,
            message,
        };
    }
    if let Some(auth) = find_auth_error(err) {
        let message = auth_message(auth, context);
        return classify_auth(auth, context, message);
    }
    if let Some(kube::Error::Api(status)) = kube_error(err) {
        return classify_status(status, context, message);
    }
    if contains_any(&message, &TLS_MARKERS) {
        return Failure {
            kind: FailureKind::Tls,
            detail: format!("TLS failed: {}", first_line(&message)),
            remedy: None,
            message,
        };
    }
    if contains_any(&message, &UNREACHABLE_MARKERS) {
        return Failure {
            kind: FailureKind::Unreachable,
            detail: format!(
                "Can’t reach {}.",
                context.server.as_deref().unwrap_or("the API server")
            ),
            remedy: None,
            message,
        };
    }
    Failure {
        kind: FailureKind::Other,
        detail: first_line(&message),
        remedy: None,
        message,
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;
    use std::path::PathBuf;
    use std::process::{ExitStatus, Output};

    use super::*;
    use crate::discovery::ContextRef;
    use crate::provider::ExecCommand;

    const EXIT_FAILURE_RAW: i32 = 256;

    fn context(provider: Provider, command: &str, args: &[&str]) -> KubeContext {
        KubeContext {
            trust: crate::discovery::Fingerprint::default(),
            credentials: crate::discovery::Fingerprint::default(),
            id: ContextRef {
                file: PathBuf::from("/k/config"),
                context: "sandbox".to_owned(),
            },
            cluster: "sandbox".to_owned(),
            user: None,
            namespace: None,
            server: Some("https://x.eks.amazonaws.com".to_owned()),
            provider,
            auth: AuthKind::Exec(ExecCommand {
                command: command.to_owned(),
                args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            }),
            is_current: false,
        }
    }

    fn exec_failed(stderr: &str) -> Error {
        let auth = AuthError::AuthExecRun {
            cmd: "aws".to_owned(),
            status: ExitStatus::from_raw(EXIT_FAILURE_RAW),
            out: Output {
                status: ExitStatus::from_raw(EXIT_FAILURE_RAW),
                stdout: Vec::new(),
                stderr: stderr.as_bytes().to_vec(),
            },
        };
        Error::Kube(kube::Error::Auth(auth))
    }

    #[test]
    fn a_changed_context_is_a_config_failure_not_an_auth_one() {
        let ctx = context(Provider::Generic, "kubectl", &[]);
        let failure = classify(&Error::ContextChanged("lab".into()), &ctx);
        assert_eq!(failure.kind, FailureKind::Config);
        assert_eq!(failure.detail, CONTEXT_CHANGED);
        assert!(!failure.kind.is_auth());
    }

    #[test]
    fn expired_sso_token() {
        let ctx = context(
            Provider::Eks,
            "aws",
            &[
                "eks",
                "get-token",
                "--cluster-name",
                "sandbox",
                "--profile",
                "sandbox",
            ],
        );
        let failure = classify(
            &exec_failed(
                "\nAn error occurred (ExpiredTokenException) when calling the GetCallerIdentity operation: The security token included in the request is expired\n",
            ),
            &ctx,
        );
        assert_eq!(failure.kind, FailureKind::ExpiredCredentials);
        assert_eq!(
            failure.detail,
            "aws eks get-token: An error occurred (ExpiredTokenException) when calling the \
             GetCallerIdentity operation: The security token included in the request is expired"
        );
        assert_eq!(
            failure.remedy.as_deref(),
            Some("aws sso login --profile sandbox")
        );
        assert!(failure.kind.is_auth());
    }

    #[test]
    fn exec_plugin_output_never_reaches_the_failure() {
        let ctx = context(Provider::Eks, "aws", &["eks", "get-token"]);
        let auth = AuthError::AuthExecRun {
            cmd: "aws".to_owned(),
            status: ExitStatus::from_raw(EXIT_FAILURE_RAW),
            out: Output {
                status: ExitStatus::from_raw(EXIT_FAILURE_RAW),
                stdout: br#"{"status":{"token":"planted-exec-token"}}"#.to_vec(),
                stderr: Vec::new(),
            },
        };
        let failure = classify(&Error::Kube(kube::Error::Auth(auth)), &ctx);
        assert_eq!(failure.kind, FailureKind::AuthFailed);
        assert!(
            !failure.message.contains("planted-exec-token"),
            "{}",
            failure.message
        );
        assert!(
            !failure.detail.contains("planted-exec-token"),
            "{}",
            failure.detail
        );
    }

    #[test]
    fn missing_plugin_binary() {
        let ctx = context(Provider::Gke, "gke-gcloud-auth-plugin", &[]);
        let not_found = std::io::Error::new(ErrorKind::NotFound, "No such file or directory");
        let err = Error::Kube(kube::Error::Auth(AuthError::AuthExecStart(not_found)));
        let failure = classify(&err, &ctx);
        assert_eq!(failure.kind, FailureKind::MissingPlugin);
        assert_eq!(
            failure.remedy.as_deref(),
            Some("gcloud components install gke-gcloud-auth-plugin")
        );
    }

    #[test]
    fn other_exec_failure_keeps_first_line() {
        let ctx = context(Provider::Doks, "doctl", &["kubernetes", "cluster"]);
        let failure = classify(&exec_failed("Error: unable to find cluster\nmore"), &ctx);
        assert_eq!(failure.kind, FailureKind::AuthFailed);
        assert_eq!(
            failure.detail,
            "doctl kubernetes cluster: Error: unable to find cluster"
        );
    }

    #[test]
    fn api_statuses() {
        let ctx = context(Provider::Gke, "gke-gcloud-auth-plugin", &[]);
        let status = |code: u16, message: &str| {
            Error::Kube(kube::Error::Api(Box::new(kube::core::Status {
                code,
                message: message.to_owned(),
                ..kube::core::Status::default()
            })))
        };
        let unauthorized = classify(&status(401, "Unauthorized"), &ctx);
        assert_eq!(unauthorized.kind, FailureKind::ExpiredCredentials);
        assert_eq!(unauthorized.detail, "Unauthorized");
        assert_eq!(unauthorized.remedy.as_deref(), Some("gcloud auth login"));
        let forbidden = classify(
            &status(403, "pods is forbidden: User \"x\" cannot list"),
            &ctx,
        );
        assert_eq!(forbidden.kind, FailureKind::Forbidden);
        assert!(!forbidden.kind.is_auth());
    }

    #[test]
    fn timeout_and_unreachable() {
        let ctx = context(Provider::Generic, "x", &[]);
        let timeout = classify(&Error::Timeout(std::time::Duration::from_secs(10)), &ctx);
        assert_eq!(timeout.kind, FailureKind::Timeout);
        let refused = Error::Io(std::io::Error::new(
            ErrorKind::ConnectionRefused,
            "tcp connect error: Connection refused (os error 61)",
        ));
        assert_eq!(classify(&refused, &ctx).kind, FailureKind::Unreachable);
    }
}
