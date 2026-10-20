mod applications;
mod builds;
mod platform;
mod routes;

pub use applications::{
    AppFilter, AppPhase, AppRoute, Application, Applications, ControllerObject, Degraded,
    HealthCheck, Placement, PlacementIssue, Resources, Rollout, WorkspaceApps,
};
pub use builds::{Build, BuildLogLine, BuildPhase, Builds, LogTone};
pub use platform::{
    AgentLevel, AgentLogLine, AgentSync, BuildQueue, Component, PlacementSummary, Platform,
    Reconnects, uptime,
};
pub use routes::{Listener, Route, RouteFilter, Routes};

use crate::Seen;

pub const GROUP: &str = "infra.loco.io";
const DIGEST_PREFIX: &str = "sha256:";
const DIGEST_KEEP: usize = 4;
const ELLIPSIS: char = '…';

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocoCluster {
    pub platform: Platform,
    pub applications: Applications,
    pub builds: Builds,
    pub routes: Routes,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocoCommand {
    RestartApplication(Seen),
    CancelBuild(Seen),
}

impl LocoCommand {
    pub fn seen(&self) -> &Seen {
        match self {
            Self::RestartApplication(application) => application,
            Self::CancelBuild(build) => build,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    pub namespace: String,
    pub name: Option<String>,
}

impl Workspace {
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.namespace)
    }
}

pub fn short_digest(text: &str) -> String {
    let Some(at) = text.find(DIGEST_PREFIX) else {
        return text.to_owned();
    };
    let hex_start = at + DIGEST_PREFIX.len();
    let hex_len = text[hex_start..]
        .find(|c: char| !c.is_ascii_hexdigit())
        .unwrap_or(text.len() - hex_start);
    if hex_len <= DIGEST_KEEP * 2 {
        return text.to_owned();
    }
    let hex_end = hex_start + hex_len;
    let head = &text[hex_start..hex_start + DIGEST_KEEP];
    let tail = &text[hex_end - DIGEST_KEEP..hex_end];
    format!(
        "{}{head}{ELLIPSIS}{tail}{}",
        &text[..hex_start],
        &text[hex_end..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digests_keep_four_characters_each_side() {
        let image = "loco-registry:5000/acme/checkout@sha256:9e1c0a7b55d2e8f1c3a944d0";
        assert_eq!(
            short_digest(image),
            "loco-registry:5000/acme/checkout@sha256:9e1c…44d0"
        );
        assert_eq!(short_digest("sha256:51ab00000f2e"), "sha256:51ab…0f2e");
        assert_eq!(short_digest("sha256:abcd1234"), "sha256:abcd1234");
        assert_eq!(short_digest("ghcr.io/x:1.0"), "ghcr.io/x:1.0");
    }

    #[test]
    fn workspaces_fall_back_to_their_namespace() {
        let raw = Workspace {
            namespace: "ws-0192c3e7".into(),
            name: None,
        };
        assert_eq!(raw.label(), "ws-0192c3e7");
        let named = Workspace {
            name: Some("acme / payments".into()),
            ..raw
        };
        assert_eq!(named.label(), "acme / payments");
    }
}
