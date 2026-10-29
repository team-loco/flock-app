pub mod action;
pub mod add_cluster;
pub mod allocation;
pub mod app_settings;
pub mod changes;
pub mod cluster;
pub mod cluster_changes;
pub mod compare;
pub mod control_plane;
pub mod crd;
pub mod diagnosis;
pub mod edit;
pub mod emptiness;
pub mod events;
pub mod expiry;
pub mod filter;
pub mod flock;
pub mod flux;
pub mod flux_graph;
pub mod fuzzy;
pub mod gitops;
pub mod grouping;
pub mod helm;
pub mod history;
pub mod home;
pub mod id;
pub mod issue;
#[cfg(feature = "loco")]
pub mod loco;
pub mod logpage;
pub mod logparse;
pub mod menubar;
pub mod multilog;
pub mod namespace;
pub mod nav;
pub mod node_pools;
pub mod nodes;
pub mod notify;
pub mod overview;
pub mod palette;
pub mod pod_tops;
pub mod pods;
pub mod problem_groups;
pub mod query;
pub mod related;
pub mod rename;
pub mod resource;
pub mod resources;
pub mod scope;
pub mod settings;
pub mod state;
pub mod status;
pub mod stream;
pub mod switcher;
pub mod tabs;
pub mod time;
pub mod units;
pub mod updater;
pub mod watch;
pub mod workload;

pub use action::{Forward, Notice};
pub use add_cluster::{
    AddClusterFlow, AddClusterStep, Candidate, CheckLine, CheckStatus, ContextChoice, ContextRef,
    KubeconfigFile, NewCluster, Verdict,
};
pub use app_settings::AppSettings;
pub use async_channel::{Receiver, Sender};
pub use changes::ChangesLog;
pub use cluster::ClusterDetail;
pub use compare::Comparison;
pub use diagnosis::{Condition, Diagnosis};
pub use flock::{ClusterSummary, Connection, Snapshot};
pub use flux::{FluxAction, FluxKey, FluxKind, FluxObject, FluxPage};
pub use gitops::GitOps;
pub use home::FlockHome;
pub use id::{ClusterId, ResourceRef, Seen, Uid};
pub use multilog::{LogEvent, LogSource, LogTarget, MultiLogRequest, SourceId, Stamp};
pub use nav::{CLUSTER_NAV, FLOCK_NAV, NavSection, Page};
pub use resource::ResourceSummary;
pub use resources::ResourceKind;
pub use settings::{ClusterCatalog, Preferences};
pub use state::{AppState, Command, Update};
pub use status::{Counts, Status};
pub use stream::{
    ExecEvent, ExecRequest, ExecTarget, LogInstance, LogLevel, LogLine, LogRequest,
    NodeShellRequest, Sink, Source, TermSize, node_shell_command_line,
};
pub use updater::{Trigger, UpdaterRequest, UpdaterState, UpdaterStatus};
pub use watch::WatchState;
pub use workload::{Workload, WorkloadKind};

pub trait Backend: Send + 'static {
    fn start(self: Box<Self>, updates: Sender<Update>, commands: Receiver<Command>);
}
