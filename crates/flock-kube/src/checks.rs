use std::time::{Duration, Instant};

use futures::future::{BoxFuture, FutureExt, join_all};
use futures::stream::{FuturesOrdered, StreamExt};
use kube::Client;
use tokio::net::TcpStream;

use crate::client::connect;
use crate::discovery::{ContextRef, KubeContext};
use crate::error::{Error, Result};
use crate::failure::{Failure, FailureKind, classify};
use crate::probe::{
    self, AccessRequest, ApiGroups, FLUX_HELM_GROUP, FLUX_KUSTOMIZE_GROUP, PrometheusTarget,
};
use crate::provider::{Provider, refine_provider};
use crate::settings::KubeSettings;
use crate::shell_env::ShellEnv;

const HTTPS_DEFAULT_PORT: u16 = 443;
const HTTP_DEFAULT_PORT: u16 = 80;
const HTTP_SCHEME: &str = "http";
const KUSTOMIZATION_KIND: &str = "Kustomization";
const KUSTOMIZATION_PLURAL: &str = "kustomizations";
const HELM_RELEASE_KIND: &str = "HelmRelease";
const HELM_RELEASE_PLURAL: &str = "helmreleases";
const SECRETS: &str = "secrets";
const PODS: &str = "pods";
const NAMESPACES: &str = "namespaces";
const NOTHING_DENIED: &str = "Can read everything Flock shows.";
const ALL_DENIED: &str = "every page will be empty";
const CORE_DENIED: &str = "most pages will be empty";
const SECRETS_DENIED: &str = "env values won’t resolve";
const SOME_DENIED: &str = "those pages will be empty";
const CORE_ACCESS: [AccessRequest; 6] = [
    AccessRequest {
        group: "",
        resource: PODS,
        verb: "list",
    },
    AccessRequest {
        group: "",
        resource: NAMESPACES,
        verb: "list",
    },
    AccessRequest {
        group: "",
        resource: "nodes",
        verb: "list",
    },
    AccessRequest {
        group: "",
        resource: "events",
        verb: "list",
    },
    AccessRequest {
        group: "apps",
        resource: "deployments",
        verb: "list",
    },
    AccessRequest {
        group: "",
        resource: SECRETS,
        verb: "list",
    },
];
const FLUX_ACCESS: [AccessRequest; 2] = [
    AccessRequest {
        group: FLUX_KUSTOMIZE_GROUP,
        resource: KUSTOMIZATION_PLURAL,
        verb: "list",
    },
    AccessRequest {
        group: FLUX_HELM_GROUP,
        resource: HELM_RELEASE_PLURAL,
        verb: "list",
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckKind {
    Reach,
    Authenticate,
    Version,
    Permissions,
    Flux,
    Metrics,
    #[cfg(feature = "loco")]
    Loco,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Ok,
    Warn,
    Err,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub kind: CheckKind,
    pub status: CheckStatus,
    pub detail: String,
    pub raw: Option<String>,
    pub remedy: Option<String>,
    pub duration: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Ready,
    CannotReach,
    Unauthorized,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FluxInfo {
    pub version: Option<String>,
    pub kustomizations: Option<u64>,
    pub helm_releases: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterFacts {
    pub provider: Provider,
    pub version: Option<String>,
    pub user: Option<String>,
    pub nodes: Option<u64>,
    pub pods: Option<u64>,
    pub flux: Option<FluxInfo>,
    pub metrics_server: bool,
    pub prometheus: Option<PrometheusTarget>,
    #[cfg(feature = "loco")]
    pub loco: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    pub context: ContextRef,
    pub verdict: Verdict,
    pub checks: Vec<CheckResult>,
    pub facts: ClusterFacts,
    pub failure: Option<Failure>,
}

fn host_port(server: &str) -> Option<(String, u16)> {
    let uri: http::Uri = server.parse().ok()?;
    let host = uri.host()?.trim_matches(['[', ']']).to_owned();
    let default_port = if uri.scheme_str() == Some(HTTP_SCHEME) {
        HTTP_DEFAULT_PORT
    } else {
        HTTPS_DEFAULT_PORT
    };
    Some((host, uri.port_u16().unwrap_or(default_port)))
}

async fn reach(server: &str, timeout: Duration) -> Result<Duration> {
    let (host, port) =
        host_port(server).ok_or(Error::MissingField("valid server URL in kubeconfig"))?;
    let started = Instant::now();
    tokio::time::timeout(timeout, TcpStream::connect((host.as_str(), port)))
        .await
        .map_err(|_| Error::Timeout(timeout))??;
    Ok(started.elapsed())
}

async fn timed<T, F>(timeout: Duration, future: F) -> (Result<T>, Duration)
where
    F: Future<Output = Result<T>>,
{
    let started = Instant::now();
    let outcome = tokio::time::timeout(timeout, future)
        .await
        .unwrap_or(Err(Error::Timeout(timeout)));
    (outcome, started.elapsed())
}

fn result(kind: CheckKind, status: CheckStatus, detail: String, duration: Duration) -> CheckResult {
    CheckResult {
        kind,
        status,
        detail,
        raw: None,
        remedy: None,
        duration,
    }
}

fn failed(kind: CheckKind, failure: &Failure, duration: Duration) -> CheckResult {
    CheckResult {
        kind,
        status: CheckStatus::Err,
        detail: String::new(),
        raw: Some(failure.detail.clone()),
        remedy: failure.remedy.clone(),
        duration,
    }
}

fn plural(count: u64, singular: &str, many: &str) -> String {
    if count == 1 {
        format!("{count} {singular}")
    } else {
        format!("{count} {many}")
    }
}

pub fn version_detail(version: &str, nodes: Option<u64>, pods: Option<u64>) -> String {
    let parts = [
        Some(format!("Kubernetes {version}")),
        nodes.map(|count| plural(count, "node", "nodes")),
        pods.map(|count| plural(count, "pod", "pods")),
    ];
    parts.into_iter().flatten().collect::<Vec<_>>().join(" · ")
}

pub fn flux_detail(flux: &FluxInfo) -> String {
    let version = flux
        .version
        .as_deref()
        .map_or_else(|| "Flux".to_owned(), |version| format!("Flux {version}"));
    let counts: Vec<String> = [
        flux.kustomizations
            .map(|count| plural(count, KUSTOMIZATION_KIND, "Kustomizations")),
        flux.helm_releases
            .map(|count| plural(count, HELM_RELEASE_KIND, "HelmReleases")),
    ]
    .into_iter()
    .flatten()
    .collect();
    if counts.is_empty() {
        version
    } else {
        format!("{version} · {}", counts.join(", "))
    }
}

pub fn forbidden(request: &AccessRequest) -> String {
    let denied = format!(
        "{} is forbidden: cannot {} resource \"{}\"",
        request.resource, request.verb, request.resource
    );
    if request.group.is_empty() {
        denied
    } else {
        format!("{denied} in API group \"{}\"", request.group)
    }
}

pub fn permissions_line(
    denied: &[&AccessRequest],
    checked: usize,
) -> (CheckStatus, String, Option<String>) {
    if denied.is_empty() {
        return (CheckStatus::Ok, NOTHING_DENIED.to_owned(), None);
    }
    let raw = denied
        .iter()
        .map(|request| forbidden(request))
        .collect::<Vec<_>>()
        .join("\n");
    let resources: Vec<&str> = denied.iter().map(|request| request.resource).collect();
    let (status, consequence) = if denied.len() == checked {
        (CheckStatus::Err, ALL_DENIED)
    } else if resources.contains(&PODS) && resources.contains(&NAMESPACES) {
        (CheckStatus::Err, CORE_DENIED)
    } else if resources == [SECRETS] {
        (CheckStatus::Warn, SECRETS_DENIED)
    } else {
        (CheckStatus::Warn, SOME_DENIED)
    };
    (status, consequence.to_owned(), Some(raw))
}

pub fn metrics_detail(
    metrics_server: bool,
    prometheus: Option<&PrometheusTarget>,
) -> (CheckStatus, String) {
    match (prometheus, metrics_server) {
        (Some(target), true) => (
            CheckStatus::Ok,
            format!("Metrics from {} and metrics-server", target.label()),
        ),
        (Some(target), false) => (CheckStatus::Ok, format!("Metrics from {}", target.label())),
        (None, true) => (CheckStatus::Ok, "Metrics from metrics-server".to_owned()),
        (None, false) => (
            CheckStatus::Warn,
            "No metrics-server or Prometheus found, so usage charts stay empty.".to_owned(),
        ),
    }
}

struct Session<'a> {
    context: &'a KubeContext,
    settings: &'a KubeSettings,
    progress: Option<&'a async_channel::Sender<CheckResult>>,
    checks: Vec<CheckResult>,
    facts: ClusterFacts,
}

impl Session<'_> {
    fn record(&mut self, check: CheckResult) {
        if let Some(progress) = self.progress
            && progress.try_send(check.clone()).is_err()
        {
            tracing::debug!("check progress listener went away");
        }
        self.checks.push(check);
    }

    fn finish(self, verdict: Verdict, failure: Option<Failure>) -> CheckReport {
        CheckReport {
            context: self.context.id.clone(),
            verdict,
            checks: self.checks,
            facts: self.facts,
            failure,
        }
    }

    async fn reach(&mut self) -> Option<Failure> {
        let Some(server) = self.context.server.clone() else {
            let failure = classify(
                &Error::MissingField("server URL in kubeconfig"),
                self.context,
            );
            self.record(failed(CheckKind::Reach, &failure, Duration::ZERO));
            return Some(failure);
        };
        let (outcome, took) = timed(
            self.settings.connect_timeout,
            reach(&server, self.settings.connect_timeout),
        )
        .await;
        match outcome {
            Ok(latency) => {
                let detail = format!("Reachable in {} ms", latency.as_millis());
                self.record(result(CheckKind::Reach, CheckStatus::Ok, detail, took));
                None
            }
            Err(err) => {
                let mut failure = classify(&err, self.context);
                if failure.kind == FailureKind::Other {
                    failure.kind = FailureKind::Unreachable;
                }
                self.record(failed(CheckKind::Reach, &failure, took));
                Some(failure)
            }
        }
    }

    async fn authenticate(&mut self, env: &ShellEnv) -> Result<Client, Failure> {
        let started = Instant::now();
        let connected = connect(self.context, env, self.settings.check_timeout).await;
        let client = match connected {
            Ok(client) => client,
            Err(err) => {
                let failure = classify(&err, self.context);
                self.record(failed(CheckKind::Authenticate, &failure, started.elapsed()));
                return Err(failure);
            }
        };
        let (who, _) = timed(self.settings.check_timeout, probe::whoami(&client)).await;
        let took = started.elapsed();
        match who {
            Ok(user) => {
                let detail = user.as_deref().map_or_else(
                    || format!("Authenticated with {}", self.context.auth),
                    |user| format!("Signed in as {user}"),
                );
                self.facts.user = user;
                self.record(result(
                    CheckKind::Authenticate,
                    CheckStatus::Ok,
                    detail,
                    took,
                ));
                Ok(client)
            }
            Err(err) => {
                let failure = classify(&err, self.context);
                if matches!(failure.kind, FailureKind::Forbidden | FailureKind::NotFound) {
                    let detail = format!("Authenticated with {}", self.context.auth);
                    self.record(result(
                        CheckKind::Authenticate,
                        CheckStatus::Ok,
                        detail,
                        took,
                    ));
                    return Ok(client);
                }
                self.record(failed(CheckKind::Authenticate, &failure, took));
                Err(failure)
            }
        }
    }

    fn learn(&mut self, learned: Learned) {
        match learned {
            Learned::Nothing => {}
            Learned::Version {
                provider,
                version,
                nodes,
                pods,
            } => {
                self.facts.provider = provider;
                self.facts.version = Some(version);
                self.facts.nodes = nodes;
                self.facts.pods = pods;
            }
            Learned::Flux(info) => self.facts.flux = Some(info),
            Learned::Metrics { server, prometheus } => {
                self.facts.metrics_server = server;
                self.facts.prometheus = prometheus;
            }
            #[cfg(feature = "loco")]
            Learned::Loco(found) => self.facts.loco = found,
        }
    }

    async fn survey(&mut self, client: &Client) {
        let context = self.context;
        let settings = self.settings;
        let groups = timed(settings.check_timeout, probe::api_groups(client))
            .map(|(groups, _)| groups.unwrap_or_default())
            .shared();
        let mut steps: FuturesOrdered<BoxFuture<'_, Step>> = FuturesOrdered::new();
        steps.push_back(version(client, context, settings).boxed());
        let flux_groups = groups.clone();
        steps.push_back(async move { flux(client, &flux_groups.await, settings).await }.boxed());
        steps.push_back(metrics(client, settings).boxed());
        let permission_groups = groups.clone();
        steps.push_back(
            async move { permissions(client, context, &permission_groups.await, settings).await }
                .boxed(),
        );
        #[cfg(feature = "loco")]
        steps.push_back(async move { loco(&groups.await) }.boxed());
        while let Some((check, learned)) = steps.next().await {
            self.learn(learned);
            self.record(check);
        }
    }
}

type Step = (CheckResult, Learned);

enum Learned {
    Nothing,
    Version {
        provider: Provider,
        version: String,
        nodes: Option<u64>,
        pods: Option<u64>,
    },
    Flux(FluxInfo),
    Metrics {
        server: bool,
        prometheus: Option<PrometheusTarget>,
    },
    #[cfg(feature = "loco")]
    Loco(bool),
}

async fn version(client: &Client, context: &KubeContext, settings: &KubeSettings) -> Step {
    let timeout = settings.check_timeout;
    let ((info, took), (nodes, _), (pods, _)) = tokio::join!(
        timed(timeout, async { Ok(client.apiserver_version().await?) }),
        timed(timeout, probe::sample_nodes(client)),
        timed(timeout, probe::count_pods(client)),
    );
    let info = match info {
        Ok(info) => info,
        Err(err) => {
            let failure = classify(&err, context);
            return (failed(CheckKind::Version, &failure, took), Learned::Nothing);
        }
    };
    let nodes = nodes.ok();
    let node_count = nodes.as_ref().map(|sample| sample.count);
    let provider_id = nodes
        .as_ref()
        .and_then(|sample| sample.provider_id.as_deref());
    let provider = refine_provider(context.provider, Some(&info.git_version), provider_id);
    let pods = pods.ok();
    let detail = version_detail(&info.git_version, node_count, pods);
    let check = result(CheckKind::Version, CheckStatus::Ok, detail, took);
    let learned = Learned::Version {
        provider,
        version: info.git_version,
        nodes: node_count,
        pods,
    };
    (check, learned)
}

async fn permissions(
    client: &Client,
    context: &KubeContext,
    groups: &ApiGroups,
    settings: &KubeSettings,
) -> Step {
    let started = Instant::now();
    let flux = groups.has(FLUX_KUSTOMIZE_GROUP);
    let requests: Vec<&AccessRequest> = CORE_ACCESS
        .iter()
        .chain(FLUX_ACCESS.iter().filter(|_| flux))
        .collect();
    let timeout = settings.check_timeout;
    let answers = join_all(
        requests
            .iter()
            .map(|request| timed(timeout, probe::can(client, request))),
    )
    .await;
    let mut denied = Vec::new();
    for (request, (answer, _)) in requests.iter().zip(answers) {
        match answer {
            Ok(true) => {}
            Ok(false) => denied.push(*request),
            Err(err) => {
                let failure = classify(&err, context);
                let check = failed(CheckKind::Permissions, &failure, started.elapsed());
                return (check, Learned::Nothing);
            }
        }
    }
    let (status, detail, raw) = permissions_line(&denied, requests.len());
    let check = CheckResult {
        raw,
        ..result(CheckKind::Permissions, status, detail, started.elapsed())
    };
    (check, Learned::Nothing)
}

async fn count_releases(client: &Client, groups: &ApiGroups, timeout: Duration) -> Option<u64> {
    let helm_version = groups.preferred_version(FLUX_HELM_GROUP)?;
    let releases = probe::dynamic_api(
        client,
        FLUX_HELM_GROUP,
        helm_version,
        HELM_RELEASE_KIND,
        HELM_RELEASE_PLURAL,
    );
    timed(timeout, probe::count(&releases)).await.0.ok()
}

async fn flux(client: &Client, groups: &ApiGroups, settings: &KubeSettings) -> Step {
    let started = Instant::now();
    let Some(kustomize_version) = groups.preferred_version(FLUX_KUSTOMIZE_GROUP) else {
        let detail = "Flux not installed".to_owned();
        let check = result(
            CheckKind::Flux,
            CheckStatus::Info,
            detail,
            started.elapsed(),
        );
        return (check, Learned::Nothing);
    };
    let timeout = settings.check_timeout;
    let kustomizations = probe::dynamic_api(
        client,
        FLUX_KUSTOMIZE_GROUP,
        kustomize_version,
        KUSTOMIZATION_KIND,
        KUSTOMIZATION_PLURAL,
    );
    let ((version, _), (kustomization_count, _), helm_count) = tokio::join!(
        timed(timeout, probe::flux_version(client)),
        timed(timeout, probe::count(&kustomizations)),
        count_releases(client, groups, timeout),
    );
    let info = FluxInfo {
        version: version.ok().flatten(),
        kustomizations: kustomization_count.ok(),
        helm_releases: helm_count,
    };
    let detail = flux_detail(&info);
    let check = result(CheckKind::Flux, CheckStatus::Ok, detail, started.elapsed());
    (check, Learned::Flux(info))
}

async fn metrics(client: &Client, settings: &KubeSettings) -> Step {
    let started = Instant::now();
    let timeout = settings.check_timeout;
    let ((metrics_server, _), (prometheus, _)) = tokio::join!(
        timed(timeout, probe::metrics_server_available(client)),
        timed(timeout, async { Ok(probe::find_prometheus(client).await) }),
    );
    let server = metrics_server.unwrap_or(false);
    let prometheus = prometheus.ok().flatten();
    let (status, detail) = metrics_detail(server, prometheus.as_ref());
    let check = result(CheckKind::Metrics, status, detail, started.elapsed());
    (check, Learned::Metrics { server, prometheus })
}

#[cfg(feature = "loco")]
fn loco(groups: &ApiGroups) -> Step {
    let found = groups.has(probe::LOCO_GROUP);
    let (status, detail) = if found {
        (CheckStatus::Ok, "Loco detected".to_owned())
    } else {
        (CheckStatus::Info, "Loco not installed".to_owned())
    };
    let check = result(CheckKind::Loco, status, detail, Duration::ZERO);
    (check, Learned::Loco(found))
}

pub async fn run_checks(
    context: &KubeContext,
    env: &ShellEnv,
    settings: &KubeSettings,
) -> CheckReport {
    run_session(context, env, settings, None).await
}

pub async fn run_checks_streaming(
    context: &KubeContext,
    env: &ShellEnv,
    settings: &KubeSettings,
    progress: &async_channel::Sender<CheckResult>,
) -> CheckReport {
    run_session(context, env, settings, Some(progress)).await
}

async fn run_session(
    context: &KubeContext,
    env: &ShellEnv,
    settings: &KubeSettings,
    progress: Option<&async_channel::Sender<CheckResult>>,
) -> CheckReport {
    let mut session = Session {
        context,
        settings,
        progress,
        checks: Vec::new(),
        facts: ClusterFacts {
            provider: context.provider,
            version: None,
            user: None,
            nodes: None,
            pods: None,
            flux: None,
            metrics_server: false,
            prometheus: None,
            #[cfg(feature = "loco")]
            loco: false,
        },
    };
    if let Some(failure) = session.reach().await {
        return session.finish(Verdict::CannotReach, Some(failure));
    }
    let client = match session.authenticate(env).await {
        Ok(client) => client,
        Err(failure) => {
            let verdict = if failure.kind == FailureKind::Unreachable {
                Verdict::CannotReach
            } else {
                Verdict::Unauthorized
            };
            return session.finish(verdict, Some(failure));
        }
    };
    session.survey(&client).await;
    session.finish(Verdict::Ready, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_matches_design() {
        assert_eq!(
            version_detail("v1.30.4", Some(9), Some(212)),
            "Kubernetes v1.30.4 · 9 nodes · 212 pods"
        );
        assert_eq!(
            version_detail("v1.31.0", Some(1), None),
            "Kubernetes v1.31.0 · 1 node"
        );
    }

    #[test]
    fn flux_line_matches_design() {
        let info = FluxInfo {
            version: Some("v2.4.0".to_owned()),
            kustomizations: Some(14),
            helm_releases: Some(6),
        };
        assert_eq!(
            flux_detail(&info),
            "Flux v2.4.0 · 14 Kustomizations, 6 HelmReleases"
        );
        assert_eq!(flux_detail(&FluxInfo::default()), "Flux");
    }

    #[test]
    fn permission_lines_show_the_forbidden_message_and_what_breaks() {
        let [pods, namespaces, nodes, _, _, secrets] = &CORE_ACCESS;
        let [kustomizations, _] = &FLUX_ACCESS;
        assert_eq!(
            permissions_line(&[], 6),
            (CheckStatus::Ok, NOTHING_DENIED.to_owned(), None)
        );
        assert_eq!(
            permissions_line(&[secrets], 6),
            (
                CheckStatus::Warn,
                "env values won’t resolve".to_owned(),
                Some("secrets is forbidden: cannot list resource \"secrets\"".to_owned())
            )
        );
        assert_eq!(
            permissions_line(&[nodes, kustomizations], 8),
            (
                CheckStatus::Warn,
                SOME_DENIED.to_owned(),
                Some(
                    "nodes is forbidden: cannot list resource \"nodes\"\n\
                     kustomizations is forbidden: cannot list resource \"kustomizations\" \
                     in API group \"kustomize.toolkit.fluxcd.io\""
                        .to_owned()
                )
            )
        );
        assert_eq!(
            permissions_line(&[pods, namespaces], 6).1,
            CORE_DENIED.to_owned()
        );
        assert_eq!(permissions_line(&[pods, nodes], 2).0, CheckStatus::Err);
    }

    #[test]
    fn metrics_lines() {
        let target = PrometheusTarget {
            namespace: "monitoring".to_owned(),
            service: "prometheus-k8s".to_owned(),
            port: 9090,
            scheme: "http".to_owned(),
        };
        assert_eq!(
            metrics_detail(false, Some(&target)).1,
            "Metrics from monitoring/prometheus-k8s"
        );
        assert_eq!(metrics_detail(false, None).0, CheckStatus::Warn);
    }

    #[test]
    fn server_host_and_port() {
        assert_eq!(
            host_port("https://127.0.0.1:52814"),
            Some(("127.0.0.1".to_owned(), 52814))
        );
        assert_eq!(
            host_port("https://x.eks.amazonaws.com"),
            Some(("x.eks.amazonaws.com".to_owned(), 443))
        );
        assert_eq!(host_port("http://[::1]"), Some(("::1".to_owned(), 80)));
    }

    #[tokio::test]
    async fn streaming_reports_each_check_as_it_finishes() {
        let Ok(listener) = std::net::TcpListener::bind("127.0.0.1:0") else {
            panic!("bind");
        };
        let Ok(addr) = listener.local_addr() else {
            panic!("addr");
        };
        drop(listener);
        let context = KubeContext {
            trust: crate::discovery::Fingerprint::default(),
            credentials: crate::discovery::Fingerprint::default(),
            id: ContextRef {
                file: "/k".into(),
                context: "gone".to_owned(),
            },
            cluster: "gone".to_owned(),
            user: None,
            namespace: None,
            server: Some(format!("https://{addr}")),
            provider: Provider::Generic,
            auth: crate::provider::AuthKind::Token,
            is_current: false,
        };
        let env = ShellEnv::from_process(String::new());
        let (tx, rx) = async_channel::unbounded();
        let report = run_checks_streaming(&context, &env, &KubeSettings::default(), &tx).await;
        drop(tx);
        let mut streamed = Vec::new();
        while let Ok(check) = rx.recv().await {
            streamed.push(check);
        }
        assert_eq!(streamed, report.checks);
    }

    const ROUND_TRIP: Duration = Duration::from_secs(1);

    fn session<'a>(context: &'a KubeContext, settings: &'a KubeSettings) -> Session<'a> {
        Session {
            context,
            settings,
            progress: None,
            checks: Vec::new(),
            facts: ClusterFacts {
                provider: context.provider,
                version: None,
                user: None,
                nodes: None,
                pods: None,
                flux: None,
                metrics_server: false,
                prometheus: None,
                #[cfg(feature = "loco")]
                loco: false,
            },
        }
    }

    #[tokio::test(start_paused = true)]
    async fn independent_checks_overlap_and_report_in_order() {
        let context = KubeContext {
            trust: crate::discovery::Fingerprint::default(),
            credentials: crate::discovery::Fingerprint::default(),
            id: ContextRef {
                file: "/k".into(),
                context: "fake".to_owned(),
            },
            cluster: "fake".to_owned(),
            user: None,
            namespace: None,
            server: None,
            provider: Provider::Generic,
            auth: crate::provider::AuthKind::Token,
            is_current: false,
        };
        let settings = KubeSettings::default();
        let (client, traffic) = crate::fake::client(ROUND_TRIP, |_, _| None);
        let mut session = session(&context, &settings);
        let started = tokio::time::Instant::now();
        session.survey(&client).await;
        let took = started.elapsed();
        assert!(took <= ROUND_TRIP * 2, "{took:?}");
        assert!(traffic.peak() > CORE_ACCESS.len(), "{}", traffic.peak());
        let kinds: Vec<CheckKind> = session.checks.iter().map(|check| check.kind).collect();
        let core = [
            CheckKind::Version,
            CheckKind::Flux,
            CheckKind::Metrics,
            CheckKind::Permissions,
        ];
        #[cfg(not(feature = "loco"))]
        let expected = core.to_vec();
        #[cfg(feature = "loco")]
        let expected = [core.as_slice(), &[CheckKind::Loco]].concat();
        assert_eq!(kinds, expected);
    }

    #[tokio::test]
    async fn unreachable_server_stops_after_reach() {
        let Ok(listener) = std::net::TcpListener::bind("127.0.0.1:0") else {
            panic!("bind");
        };
        let Ok(addr) = listener.local_addr() else {
            panic!("addr");
        };
        drop(listener);
        let context = KubeContext {
            trust: crate::discovery::Fingerprint::default(),
            credentials: crate::discovery::Fingerprint::default(),
            id: ContextRef {
                file: "/k".into(),
                context: "gone".to_owned(),
            },
            cluster: "gone".to_owned(),
            user: None,
            namespace: None,
            server: Some(format!("https://{addr}")),
            provider: Provider::Generic,
            auth: crate::provider::AuthKind::Token,
            is_current: false,
        };
        let env = ShellEnv::from_process(String::new());
        let report = run_checks(&context, &env, &KubeSettings::default()).await;
        assert_eq!(report.verdict, Verdict::CannotReach);
        assert_eq!(report.checks.len(), 1);
        assert_eq!(
            report.checks.first().map(|c| c.status),
            Some(CheckStatus::Err)
        );
        assert_eq!(
            report.failure.map(|f| f.kind),
            Some(FailureKind::Unreachable)
        );
    }
}
