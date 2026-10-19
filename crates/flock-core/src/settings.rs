use std::collections::BTreeMap;

use crate::{ClusterId, Status};

const DEFAULT_NAMESPACE: &str = "default";
const EXEC: &str = "exec";
const SEPARATOR: &str = " · ";
const AUTO: &str = "auto";
const METRICS_SERVER: &str = "metrics-server";
const PROMETHEUS: &str = "prometheus";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClusterCatalog {
    pub sources: usize,
    pub clusters: Vec<ClusterSettings>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterSettings {
    pub cluster: ClusterId,
    pub connection: Connection,
    pub auth_expired: bool,
    pub context_change: Option<ContextChange>,
    pub prometheus: Option<String>,
    pub preferences: Preferences,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextChange {
    pub pinned: String,
    pub current: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connection {
    pub context: String,
    pub server: String,
    pub auth: Auth,
    pub kubeconfig: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Auth {
    Exec(String),
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preferences {
    pub namespaces: BTreeMap<String, NamespaceSelection>,
    pub default_namespace: Option<String>,
    pub metrics: MetricsSource,
    pub confirm_destructive: bool,
    pub notify: bool,
    pub labels: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MetricsSource {
    #[default]
    Auto,
    MetricsServer,
    Prometheus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClusterAction {
    Open,
    Reauthenticate,
    Connect,
    ConfirmContext,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            namespaces: BTreeMap::new(),
            default_namespace: None,
            metrics: MetricsSource::default(),
            confirm_destructive: true,
            notify: true,
            labels: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamespaceSelection {
    All,
    Named(String),
}

impl NamespaceSelection {
    pub fn of(namespace: Option<&str>) -> Self {
        namespace.map_or(Self::All, |namespace| Self::Named(namespace.to_owned()))
    }

    pub fn namespace(&self) -> Option<&str> {
        match self {
            Self::All => None,
            Self::Named(namespace) => Some(namespace),
        }
    }
}

impl Preferences {
    pub fn namespace_selection(&self, list: &str) -> NamespaceSelection {
        self.namespaces.get(list).cloned().unwrap_or_else(|| {
            let namespace = self.default_namespace.as_deref().and_then(parse_namespace);
            NamespaceSelection::Named(namespace.unwrap_or_else(|| DEFAULT_NAMESPACE.into()))
        })
    }

    pub fn select_namespace(&mut self, list: &str, selection: NamespaceSelection) {
        self.namespaces.insert(list.to_owned(), selection);
    }
}

impl Auth {
    pub fn label(&self) -> String {
        match self {
            Self::Exec(command) => format!("{EXEC}{SEPARATOR}{command}"),
            Self::Other(label) => label.clone(),
        }
    }

    pub fn program(&self) -> Option<&str> {
        match self {
            Self::Exec(command) => command.split_whitespace().next(),
            Self::Other(_) => None,
        }
    }
}

impl MetricsSource {
    pub const ALL: [Self; 3] = [Self::Auto, Self::MetricsServer, Self::Prometheus];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => AUTO,
            Self::MetricsServer => METRICS_SERVER,
            Self::Prometheus => PROMETHEUS,
        }
    }
}

impl ClusterAction {
    pub fn for_cluster(status: Status, auth_expired: bool, context_changed: bool) -> Self {
        if context_changed {
            Self::ConfirmContext
        } else if auth_expired {
            Self::Reauthenticate
        } else if status == Status::Offline {
            Self::Connect
        } else {
            Self::Open
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Open => "Open cluster",
            Self::Reauthenticate => "Re-authenticate",
            Self::Connect => "Connect",
            Self::ConfirmContext => "Trust new server",
        }
    }
}

impl ClusterSettings {
    pub fn context_sentence(&self) -> Option<String> {
        let change = self.context_change.as_ref()?;
        let sentence = if change.pinned == change.current {
            "This context's certificate authority changed. Flock stopped using it until you trust the new one.".to_owned()
        } else {
            format!(
                "This context now points at {} instead of {}. Flock stopped using it until you trust the new server.",
                change.current, change.pinned
            )
        };
        Some(sentence)
    }

    pub fn auth_sentence(&self) -> Option<String> {
        if !self.auth_expired {
            return None;
        }
        let sentence = match self.connection.auth.program() {
            Some(program) => format!(
                "Credentials from {program} expired. Re-run your login and Flock reconnects on its own."
            ),
            None => {
                "Credentials expired. Update the kubeconfig and Flock reconnects on its own.".into()
            }
        };
        Some(sentence)
    }
}

impl ClusterCatalog {
    pub fn get(&self, cluster: &ClusterId) -> Option<&ClusterSettings> {
        self.clusters.iter().find(|c| &c.cluster == cluster)
    }

    pub fn get_mut(&mut self, cluster: &ClusterId) -> Option<&mut ClusterSettings> {
        self.clusters.iter_mut().find(|c| &c.cluster == cluster)
    }

    pub fn sentence(&self) -> String {
        let sources = match self.sources {
            1 => "1 kubeconfig source".to_owned(),
            n => format!("{n} kubeconfig sources"),
        };
        format!(
            "Discovered from {sources}. Files are watched; new contexts appear here automatically."
        )
    }
}

pub fn parse_labels(text: &str) -> Vec<String> {
    let mut labels: Vec<String> = Vec::new();
    let words = text
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|w| !w.is_empty());
    for word in words {
        if !labels.iter().any(|l| l == word) {
            labels.push(word.to_owned());
        }
    }
    labels
}

pub fn format_labels(labels: &[String]) -> String {
    labels.join(" ")
}

pub fn parse_namespace(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(auth: Auth, auth_expired: bool) -> ClusterSettings {
        ClusterSettings {
            cluster: ClusterId::new("qa-gke"),
            connection: Connection {
                context: "gke_qa".into(),
                server: "https://[IP]".into(),
                auth,
                kubeconfig: "~/.kube/config".into(),
            },
            auth_expired,
            context_change: None,
            prometheus: None,
            preferences: Preferences::default(),
        }
    }

    #[test]
    fn namespace_memory_is_per_list_over_the_preferred_default() {
        let mut preferences = Preferences::default();
        assert_eq!(
            preferences.namespace_selection("Event").namespace(),
            Some("default")
        );
        preferences.default_namespace = Some("payments".into());
        assert_eq!(
            preferences.namespace_selection("Event").namespace(),
            Some("payments")
        );
        preferences.select_namespace("Event", NamespaceSelection::All);
        assert_eq!(preferences.namespace_selection("Event").namespace(), None);
        preferences.select_namespace("Pod", NamespaceSelection::of(Some("loco-system")));
        assert_eq!(
            preferences.namespace_selection("Pod").namespace(),
            Some("loco-system")
        );
        assert_eq!(
            preferences.namespace_selection("Deployment").namespace(),
            Some("payments")
        );
    }

    #[test]
    fn auth_labels_follow_the_design() {
        let exec = Auth::Exec("aws eks get-token".into());
        assert_eq!(exec.label(), "exec · aws eks get-token");
        assert_eq!(exec.program(), Some("aws"));
        let certificate = Auth::Other("client certificate".into());
        assert_eq!(certificate.label(), "client certificate");
        assert_eq!(certificate.program(), None);
    }

    #[test]
    fn expired_auth_names_the_plugin() {
        let expired = settings(Auth::Exec("gke-gcloud-auth-plugin".into()), true);
        assert_eq!(
            expired.auth_sentence().as_deref(),
            Some(
                "Credentials from gke-gcloud-auth-plugin expired. Re-run your login and Flock reconnects on its own."
            )
        );
        let fine = settings(Auth::Other("token".into()), false);
        assert_eq!(fine.auth_sentence(), None);
        assert!(
            settings(Auth::Other("token".into()), true)
                .auth_sentence()
                .is_some()
        );
    }

    #[test]
    fn primary_action_depends_on_connection() {
        assert_eq!(
            ClusterAction::for_cluster(Status::Err, true, false),
            ClusterAction::Reauthenticate
        );
        assert_eq!(
            ClusterAction::for_cluster(Status::Offline, false, false),
            ClusterAction::Connect
        );
        assert_eq!(
            ClusterAction::for_cluster(Status::Warn, false, false),
            ClusterAction::Open
        );
        assert_eq!(
            ClusterAction::for_cluster(Status::Offline, true, true),
            ClusterAction::ConfirmContext
        );
    }

    #[test]
    fn a_changed_context_says_what_moved() {
        let mut changed = settings(Auth::Other("token".into()), false);
        assert_eq!(changed.context_sentence(), None);
        changed.context_change = Some(ContextChange {
            pinned: "https://a:6443".into(),
            current: "https://b:6443".into(),
        });
        assert_eq!(
            changed.context_sentence().as_deref(),
            Some(
                "This context now points at https://b:6443 instead of https://a:6443. Flock stopped using it until you trust the new server."
            )
        );
        changed.context_change = Some(ContextChange {
            pinned: "https://a:6443".into(),
            current: "https://a:6443".into(),
        });
        assert!(
            changed
                .context_sentence()
                .is_some_and(|sentence| sentence.contains("certificate authority"))
        );
    }

    #[test]
    fn metrics_sources_are_labelled_by_their_kubectl_names() {
        let labels: Vec<&str> = MetricsSource::ALL
            .into_iter()
            .map(MetricsSource::label)
            .collect();
        assert_eq!(labels, ["auto", "metrics-server", "prometheus"]);
    }

    #[test]
    fn labels_parse_from_spaces_or_commas_without_duplicates() {
        assert_eq!(
            parse_labels(" env=prod, region=us-east-1  env=prod local"),
            ["env=prod", "region=us-east-1", "local"]
        );
        assert_eq!(parse_labels("  "), Vec::<String>::new());
        let labels = parse_labels("env=dev local");
        assert_eq!(format_labels(&labels), "env=dev local");
    }

    #[test]
    fn blank_namespace_means_none() {
        assert_eq!(parse_namespace("  "), None);
        assert_eq!(parse_namespace(" payments "), Some("payments".into()));
    }

    #[test]
    fn catalog_sentence_counts_sources() {
        let catalog = ClusterCatalog {
            sources: 3,
            clusters: Vec::new(),
        };
        assert_eq!(
            catalog.sentence(),
            "Discovered from 3 kubeconfig sources. Files are watched; new contexts appear here automatically."
        );
    }
}
