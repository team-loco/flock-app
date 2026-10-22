use std::collections::BTreeMap;
use std::rc::Rc;

use crate::kit::{SelectableText, Tooltip};
use flock_core::diagnosis::true_label;
use flock_core::emptiness::Noun;
use flock_core::filter::label_values;
use flock_core::multilog::LabelSelector;
use flock_core::pods::{
    Container, ContainerState, FailingElsewhere, OWNER_ARROW, Pod, PodFilter, PodPhase, PodQuery,
    Resources, Termination, Usage, filter_counts, nav_counts, owner_segment, restarts_label,
};
use flock_core::related::Related;
use flock_core::resources::{NamespaceCount, POD_LIST, namespace_counts};
use flock_core::settings::NamespaceSelection;
use flock_core::units::Amount;
use flock_core::{
    ClusterId, Command, Condition, LogInstance, LogTarget, Page, Preferences, ResourceRef, Seen,
    Snapshot, Status,
};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, WeakEntity, Window, div, prelude::FluentBuilder, px,
};

mod streams;

pub(crate) use streams::exit_note;

use streams::{LogSession, ShellSession, TERMINAL_ID};

use crate::kit::{
    Button, CONDITIONS, Column, ConditionTable, ConfirmDialog, DockState, Entrance, ErrorBlocks,
    FILTER_PLACEHOLDER, FilterBar, FilterEvent, InputEvent, InspectorPane, KeyValue, Link, LogView,
    MenuItem, PanelHeader, PanelSection, Select, SelectEvent, ShowAll, StatusText, Table,
    TableEvent, Terminal, TextFieldState, chip_label, confirm, swap_in,
};
use crate::screens::related::{OnPick, Pick, RELATED, inspector_section};
use crate::screens::{
    ALL_NAMESPACES, ListShape, chosen_namespace, edit_item, empty_list, new_tab_item,
    set_namespace_options,
};
use crate::theme::{
    ActiveTheme, FONT_MONO, INSPECTOR_POD, Palette, SPACE_2, SPACE_3, SPACE_4, SPACE_5, SPACE_6,
    TEXT_BODY, TEXT_MONO, TEXT_SMALL,
};

const INSPECTOR_ID: &str = "pod-inspector";
const REQUESTS: &str = "requests";
const LIMITS: &str = "limits";
const FAILING_ELSEWHERE: &str = "pod-failing-elsewhere";
const OWNER_LINK: &str = "pod-owner";
const NAME_MIN: Pixels = px(138.);
const NAME_WEIGHT: f32 = 2.;
const OWNER_MIN: Pixels = px(76.);
const OWNER_WEIGHT: f32 = 1.;
const READY: Pixels = px(48.);
const RESTARTS: Pixels = px(64.);
const STATUS_MIN: Pixels = px(92.);
const STATUS_MAX: Pixels = px(220.);
const NODE_MIN: Pixels = px(92.);
const NODE_WEIGHT: f32 = 1.;
const CPU: Pixels = px(84.);
const MEMORY: Pixels = px(68.);
const AGE: Pixels = px(44.);
const NAMESPACE_SELECT: Pixels = px(180.);
const CONTAINER_SELECT: Pixels = px(160.);
const INSPECTOR_LABEL: Pixels = px(96.);
const CONTAINER_LABEL: Pixels = px(80.);
const SEPARATOR: &str = " · ";

const MISSING: &str = "—";
const NO_RESOURCES: &str = "none";
const DETAILS: &str = "Details";
const CONTAINERS: &str = "Containers";
const VOLUMES: &str = "Volumes";
const LOGS_BEFORE_RESTART: &str = "Logs before restart →";
const LOGS_FOR: &str = "Logs for";
const SELECTED_PODS: &str = "selected pods";
const SHELL: &str = "Shell";
const CONTAINER_HEAD: &str = "pod-container-head";
const CONTAINER_STATE: &str = "pod-container-state";
const NO_RUNNING_CONTAINER: &str =
    "No container is running, so there is nothing to open a shell in";
const MARKED_LOGS_DESCRIPTION: &str = "Merge the output of every ⇧-clicked pod in one tab.";
const SELECTOR_LOGS_DESCRIPTION: &str =
    "Follows every pod that matches the label filter, including new ones";
const NO_SELECTION: &str = "Select a pod to see its containers, conditions and events.";
pub struct PodsScreen {
    pods: Vec<Pod>,
    cluster: Option<ClusterId>,
    names: Vec<(ClusterId, String)>,
    cluster_namespaces: BTreeMap<ClusterId, Vec<String>>,
    shown: usize,
    query: PodQuery,
    selected: Option<ResourceRef>,
    panel_open: bool,
    related: Vec<Related>,
    inspected: Option<ResourceRef>,
    container: Option<String>,
    instance: LogInstance,
    elsewhere: Entity<ElsewhereLink>,
    log: Option<LogSession>,
    shell: Option<ShellSession>,
    table: Entity<Table<Pod>>,
    filter: Entity<FilterBar>,
    namespace: Entity<Select>,
    preferences: Preferences,
    containers: Entity<Select>,
    search: Entity<TextFieldState>,
    logs: Entity<LogView>,
    terminal: Entity<Terminal>,
    dock: Entity<DockState>,
    entrance: Entrance,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Command> for PodsScreen {}

fn entrance(screen: &mut PodsScreen) -> &mut Entrance {
    &mut screen.entrance
}

impl PodsScreen {
    pub fn list_focus(&self, cx: &gpui::App) -> gpui::FocusHandle {
        gpui::Focusable::focus_handle(&self.table, cx)
    }

    pub fn dock(&self) -> &Entity<DockState> {
        &self.dock
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let table = cx.new(|cx| {
            Table::new("pods", columns(), cx)
                .row_key(|pod: &Pod| format!("{}/{}", pod.namespace, pod.name).into())
                .multi_select()
                .click_opens()
                .column_gap(SPACE_6)
        });
        let search = cx.new(|cx| TextFieldState::new(window, cx).placeholder(FILTER_PLACEHOLDER));
        let namespace = cx.new(|cx| {
            Select::new("pod-namespace", vec![ALL_NAMESPACES.into()], cx)
                .mono()
                .width(NAMESPACE_SELECT)
        });
        let (filter, elsewhere) = filter_bar(&namespace, window, cx);
        let containers = cx.new(|cx| {
            Select::new("pod-container", Vec::new(), cx)
                .mono()
                .width(CONTAINER_SELECT)
        });
        let logs = cx.new(|_| LogView::new("pod-logs").parsed());
        let terminal = cx.new(|cx| Terminal::new(TERMINAL_ID, cx));
        let dock = cx.new(|_| DockState::new());
        let subscriptions = vec![
            cx.subscribe(&table, |this, table, event: &TableEvent, cx| {
                this.table_event(&table, event, cx);
            }),
            cx.subscribe(&filter, |this, bar, FilterEvent::Changed, cx| {
                let (text, chip) = {
                    let bar = bar.read(cx);
                    (bar.query().clone(), bar.chip())
                };
                this.query.text = text;
                this.query.filter = PodFilter::ALL.get(chip).copied().unwrap_or_default();
                this.refresh(cx);
            }),
            cx.subscribe(&search, |this, state, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let text = state.read(cx).value();
                    this.logs.update(cx, |logs, cx| logs.set_query(text, cx));
                }
            }),
            cx.subscribe(&namespace, |this, select, SelectEvent::Changed(_), cx| {
                let chosen = chosen_namespace(select.read(cx));
                this.choose_namespace(chosen, cx);
            }),
            cx.subscribe(&containers, |this, select, SelectEvent::Changed(ix), cx| {
                let name = select.read(cx).selected_label().map(ToString::to_string);
                let name = name.or_else(|| {
                    this.selected_pod()
                        .and_then(|pod| pod.containers.get(*ix))
                        .map(|c| c.name.clone())
                });
                this.pick_container(name);
                this.sync_streams(cx);
            }),
            cx.observe(&dock, |this, _, cx| this.sync_streams(cx)),
        ];
        Self {
            pods: Vec::new(),
            cluster: None,
            names: Vec::new(),
            cluster_namespaces: BTreeMap::new(),
            shown: 0,
            query: PodQuery::default(),
            selected: None,
            panel_open: true,
            related: Vec::new(),
            inspected: None,
            container: None,
            instance: LogInstance::Current,
            elsewhere,
            log: None,
            shell: None,
            table,
            filter,
            namespace,
            preferences: Preferences::default(),
            containers,
            search,
            logs,
            terminal,
            dock,
            entrance: Entrance::default(),
            _subscriptions: subscriptions,
        }
    }

    pub fn set_snapshot(&mut self, snapshot: &Snapshot, cx: &mut Context<Self>) {
        self.pods.clone_from(&snapshot.pods);
        self.related = snapshot
            .details
            .values()
            .flat_map(|detail| detail.related.iter().cloned())
            .collect();
        self.names = snapshot
            .clusters
            .iter()
            .map(|cluster| (cluster.id.clone(), cluster.name.clone()))
            .collect();
        self.cluster_namespaces = snapshot
            .details
            .iter()
            .map(|(id, detail)| {
                let names = detail.namespaces.iter().map(|ns| ns.name.clone());
                (id.clone(), names.collect())
            })
            .collect();
        self.refresh(cx);
    }

    pub fn set_context(
        &mut self,
        cluster: Option<&ClusterId>,
        resource: Option<&ResourceRef>,
        preferences: &Preferences,
        cx: &mut Context<Self>,
    ) {
        if self.cluster.as_ref() != cluster {
            self.cluster = cluster.cloned();
            self.selected = None;
            self.panel_open = true;
        }
        self.preferences = preferences.clone();
        self.query.namespace = preferences
            .namespace_selection(POD_LIST)
            .namespace()
            .map(str::to_owned);
        let wanted = resource.filter(|r| self.pods.iter().any(|pod| pod.is(r)));
        if let Some(pod) = wanted {
            let elsewhere = pod
                .namespace
                .as_ref()
                .filter(|ns| {
                    self.query
                        .namespace
                        .as_ref()
                        .is_some_and(|chosen| chosen != *ns)
                })
                .cloned();
            if let Some(namespace) = elsewhere {
                self.choose_namespace(Some(namespace), cx);
            }
            self.selected = Some(pod.clone());
            self.panel_open = true;
            self.pick_container(None);
        }
        self.refresh(cx);
    }

    pub fn sync_inspection(
        &mut self,
        on_page: bool,
        opened: Option<&ResourceRef>,
        cx: &mut Context<Self>,
    ) {
        if !on_page {
            self.inspected = None;
            return;
        }
        if self.selected == self.inspected || !self.panel_open {
            return;
        }
        self.inspected.clone_from(&self.selected);
        if let Some(picked) = self.selected.clone()
            && opened != Some(&picked)
        {
            cx.emit(Command::InspectResource(picked));
        }
    }

    fn choose_namespace(&mut self, choice: Option<String>, cx: &mut Context<Self>) {
        let selection = NamespaceSelection::of(choice.as_deref());
        self.query.namespace = choice;
        self.refresh(cx);
        let Some(cluster) = self.cluster.clone() else {
            return;
        };
        self.preferences.select_namespace(POD_LIST, selection);
        cx.emit(Command::SetPreferences(cluster, self.preferences.clone()));
    }

    pub fn filter_bar(&self) -> &Entity<FilterBar> {
        &self.filter
    }

    pub fn header_note(&self) -> String {
        let total = self
            .cluster
            .as_ref()
            .and_then(|cluster| nav_counts(&self.pods, cluster).total)
            .unwrap_or_default();
        total.to_string()
    }

    fn selector_logs(&self) -> Option<(LogTarget, String)> {
        let cluster = self.cluster.clone()?;
        let selector = LabelSelector::from_matches(&self.query.text.labels)?;
        let label = selector.to_string();
        let target = LogTarget::Selector {
            cluster,
            namespace: self.query.namespace.clone(),
            selector,
        };
        Some((target, label))
    }

    fn cluster_pods(&self) -> Vec<&Pod> {
        match &self.cluster {
            Some(cluster) => self.query.base(&self.pods, cluster),
            None => Vec::new(),
        }
    }

    fn namespace_counts(&self) -> Vec<NamespaceCount> {
        let known = self
            .cluster
            .as_ref()
            .and_then(|cluster| self.cluster_namespaces.get(cluster))
            .into_iter()
            .flatten()
            .map(String::as_str);
        let rows = self
            .pods
            .iter()
            .filter(|pod| self.cluster.as_ref() == Some(&pod.cluster))
            .map(|pod| pod.namespace.as_str());
        namespace_counts(known, rows, self.query.namespace.as_deref())
    }

    fn selected_pod(&self) -> Option<&Pod> {
        let selected = self.selected.as_ref()?;
        self.pods.iter().find(|pod| pod.is(selected))
    }

    fn selected_container(&self) -> Option<&Container> {
        let pod = self.selected_pod()?;
        match &self.container {
            Some(name) => pod.containers.iter().find(|c| &c.name == name),
            None => pod.containers.first(),
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let namespace = self.query.namespace.as_ref();
        let labels = label_values(
            self.pods
                .iter()
                .filter(|pod| self.cluster.as_ref() == Some(&pod.cluster))
                .filter(|pod| namespace.is_none_or(|namespace| &pod.namespace == namespace))
                .map(|pod| &pod.labels),
        );
        self.filter.update(cx, |bar, cx| bar.set_labels(labels, cx));
        let rows: Vec<Pod> = match &self.cluster {
            Some(cluster) => self
                .query
                .apply(&self.pods, cluster)
                .into_iter()
                .cloned()
                .collect(),
            None => Vec::new(),
        };
        let counts = self.namespace_counts();
        let total = self.cluster_total();
        let chosen = self.query.namespace.as_deref();
        set_namespace_options(&self.namespace, &counts, total, chosen, cx);
        let chips = filter_counts(&self.cluster_pods())
            .into_iter()
            .map(|(filter, n)| chip_label(filter.label(), n))
            .collect();
        let elsewhere = self
            .cluster
            .as_ref()
            .and_then(|cluster| self.query.failing_elsewhere(&self.pods, cluster));
        self.elsewhere.update(cx, |link, cx| {
            if link.elsewhere != elsewhere {
                link.elsewhere = elsewhere;
                cx.notify();
            }
        });
        let shown = rows.len();
        self.shown = shown;
        let total = self.cluster_total();
        self.filter.update(cx, |bar, cx| {
            bar.set_chips(chips, cx);
            bar.set_matches(shown, total, cx);
        });
        let still_there = self
            .selected
            .as_ref()
            .is_some_and(|selected| rows.iter().any(|pod| pod.is(selected)));
        if !still_there {
            self.selected = rows.first().map(Pod::resource);
            self.pick_container(None);
        }
        let position = self
            .selected
            .as_ref()
            .and_then(|selected| rows.iter().position(|pod| pod.is(selected)));
        let scope = self.cluster.as_ref().map(|c| c.as_str().to_owned());
        let words = self.query.text.words.clone();
        let selector = self.selector_logs();
        let screen = cx.weak_entity();
        self.table.update(cx, |table, cx| {
            if let Some(scope) = scope {
                table.set_scope(scope, cx);
            }
            table.set_highlight(words, cx);
            table.set_row_actions(
                move |pod: &Pod, marked: &[&Pod]| {
                    menu_items(pod, &screen, logs_target(marked, selector.as_ref()))
                },
                cx,
            );
            table.set_rows(rows, cx);
            table.select(position, cx);
        });
        self.sync_selection(cx);
    }

    fn go_elsewhere(&mut self, cx: &mut Context<Self>) {
        let target = self.elsewhere.read(cx).elsewhere.clone();
        if let Some(target) = target {
            self.choose_namespace(target.namespace, cx);
        }
    }

    fn show_all_namespaces(&mut self, cx: &mut Context<Self>) {
        self.namespace
            .update(cx, |select, cx| select.set_selected(0, cx));
        self.choose_namespace(None, cx);
    }

    fn cluster_name(&self) -> &str {
        let named = self
            .names
            .iter()
            .find(|(id, _)| Some(id) == self.cluster.as_ref());
        named.map_or("", |(_, name)| name.as_str())
    }

    fn cluster_total(&self) -> usize {
        self.cluster.as_ref().map_or(0, |cluster| {
            self.pods
                .iter()
                .filter(|pod| &pod.cluster == cluster)
                .count()
        })
    }

    fn select(&mut self, pod: Option<ResourceRef>, cx: &mut Context<Self>) {
        if self.selected == pod {
            return;
        }
        self.selected = pod;
        self.inspected.clone_from(&self.selected);
        if let Some(pod) = self.selected.clone().filter(|_| self.panel_open) {
            cx.emit(Command::InspectResource(pod));
        }
        self.pick_container(None);
        self.instance = LogInstance::Current;
        self.sync_selection(cx);
    }

    fn table_event(
        &mut self,
        table: &Entity<Table<Pod>>,
        event: &TableEvent,
        cx: &mut Context<Self>,
    ) {
        let ix = match *event {
            TableEvent::Selected(ix) => ix,
            TableEvent::Opened(ix) => ix,
            TableEvent::Marked => {
                cx.notify();
                return;
            }
        };
        let pod = table.read(cx).row(ix).map(Pod::resource);
        if let Some(pod) = pod {
            self.select(Some(pod), cx);
        }
        if matches!(event, TableEvent::Opened(_)) {
            self.open_panel(cx);
        }
    }

    pub fn inspecting(&self) -> bool {
        self.panel_open && self.selected.is_some()
    }

    pub fn close_inspector(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.inspecting() {
            return false;
        }
        self.panel_open = false;
        cx.emit(Command::CloseInspector);
        cx.notify();
        true
    }

    fn open_panel(&mut self, cx: &mut Context<Self>) {
        if self.panel_open {
            return;
        }
        self.panel_open = true;
        self.inspected.clone_from(&self.selected);
        if let Some(pod) = self.selected.clone() {
            cx.emit(Command::InspectResource(pod));
        }
        cx.notify();
    }

    fn pick_container(&mut self, name: Option<String>) {
        self.container = name;
    }

    fn sync_selection(&mut self, cx: &mut Context<Self>) {
        let names: Vec<SharedString> = self
            .selected_pod()
            .map(|pod| {
                pod.containers
                    .iter()
                    .map(|c| SharedString::from(c.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let current = self.selected_container().map(|c| c.name.clone());
        let index = current
            .as_ref()
            .and_then(|name| names.iter().position(|n| n.as_ref() == name))
            .unwrap_or_default();
        self.container = current;
        self.containers.update(cx, |select, cx| {
            select.set_options(names, cx);
            select.set_selected(index, cx);
        });
        self.sync_streams(cx);
        cx.notify();
    }

    fn inspector(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let palette = *cx.palette();
        let selection = self
            .selected_pod()
            .map(|pod| format!("{}/{}", pod.namespace, pod.name))
            .unwrap_or_default();
        let motion = self.entrance.track(Some(&selection), entrance, cx);
        let related = self.selected.as_ref().and_then(|target| {
            self.related
                .iter()
                .find(|related| &related.target == target)
        });
        let screen = cx.weak_entity();
        let pick: OnPick = Rc::new(move |pick, _, cx| {
            if let Pick::Object(target) = pick {
                let _ = screen.update(cx, |_, cx| cx.emit(Command::OpenResource(target)));
            }
        });
        let related =
            PanelSection::new(INSPECTOR_ID, RELATED).child(inspector_section(related, &pick, cx));
        let (header, body) = match self.selected_pod() {
            Some(pod) => (
                Some(Self::pod_header(pod, cx)),
                Self::pod_details(pod, &palette, cx)
                    .child(related)
                    .into_any_element(),
            ),
            None => (
                None,
                div()
                    .p(SPACE_6)
                    .text_color(palette.mute)
                    .child(NO_SELECTION)
                    .into_any_element(),
            ),
        };
        let panel = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(palette.panel)
            .text_size(TEXT_BODY)
            .children(header)
            .child(
                div()
                    .id(gpui::ElementId::Name(
                        format!("{INSPECTOR_ID}-{selection}").into(),
                    ))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            );
        div()
            .size_full()
            .bg(palette.panel)
            .child(swap_in(panel, INSPECTOR_ID, motion))
            .into_any_element()
    }

    fn pod_header(pod: &Pod, cx: &mut Context<Self>) -> AnyElement {
        let running = pod
            .containers
            .iter()
            .any(|container| container.state == ContainerState::Running);
        let shell = Button::new("pod-shell", SHELL)
            .primary()
            .small()
            .disabled(!running)
            .when(!running, |shell| shell.tooltip(NO_RUNNING_CONTAINER))
            .on_click(cx.listener(|this, _, window, cx| this.open_terminal(window, cx)));
        PanelHeader::new(INSPECTOR_ID, pod.name.clone())
            .mono()
            .status(pod.status, pod.reason.clone())
            .action(shell)
            .into_any_element()
    }

    fn pod_details(pod: &Pod, palette: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let labels: Vec<String> = pod
            .labels
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        div()
            .flex()
            .flex_col()
            .child(ErrorBlocks::new(pod.issue().into_iter().collect()))
            .child(PanelSection::new(INSPECTOR_ID, DETAILS).child(Self::facts(pod, palette, cx)))
            .child(PanelSection::labels(INSPECTOR_ID, labels))
            .child(Self::containers_section(pod, palette, cx))
            .child(conditions(pod, palette))
            .child(PanelSection::events(INSPECTOR_ID, pod.events.clone()).closed())
            .child(volumes(pod))
    }

    fn facts(pod: &Pod, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = Self::owner_chain(pod, palette, cx);
        let created = format!("{} ago", flock_core::time::age(pod.age));
        let ready = div()
            .text_color(palette.status(pod.status))
            .child(format!("{} containers", pod.ready_label()));
        KeyValue::new()
            .label_width(INSPECTOR_LABEL)
            .text("Namespace", pod.namespace.clone())
            .text("Node", or_missing(pod.node.clone()))
            .text("Pod IP", or_missing(pod.ip.clone()))
            .row("Ready", ready)
            .row("Controlled by", owner)
            .text("QoS", pod.qos.label())
            .text("Service account", or_missing(pod.service_account.clone()))
            .text("Created", created)
    }

    fn owner_chain(pod: &Pod, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let chain = pod.owner_chain();
        if chain.is_empty() {
            return missing(palette);
        }
        let mut parts: Vec<AnyElement> = Vec::new();
        for (ix, target) in chain.into_iter().enumerate() {
            if ix > 0 {
                let arrow = div().text_color(palette.mute).child(OWNER_ARROW);
                parts.push(arrow.into_any_element());
            }
            let id = gpui::ElementId::NamedInteger(OWNER_LINK.into(), ix as u64);
            let link = Link::new(id, owner_segment(&target))
                .mono()
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(Command::OpenResource(target.clone()));
                }));
            parts.push(link.into_any_element());
        }
        div()
            .min_w_0()
            .flex()
            .flex_wrap()
            .font_family(FONT_MONO)
            .children(parts)
            .into_any_element()
    }

    fn containers_section(pod: &Pod, palette: &Palette, cx: &mut Context<Self>) -> PanelSection {
        let rows: Vec<AnyElement> = pod
            .containers
            .iter()
            .enumerate()
            .map(|(ix, container)| Self::container_row(pod, ix, container, palette, cx))
            .collect();
        let restarted = pod
            .containers
            .iter()
            .any(|container| container.last_termination.is_some());
        let count = pod.containers.len().to_string();
        let section = PanelSection::new(INSPECTOR_ID, CONTAINERS);
        let section = if restarted {
            section
                .count(format!(
                    "{count}{SEPARATOR}{}",
                    restarts_label(pod.restarts())
                ))
                .count_color(palette.warn)
        } else {
            section.count(count)
        };
        section.child(div().flex().flex_col().gap(SPACE_6).children(rows))
    }

    fn container_row(
        pod: &Pod,
        ix: usize,
        container: &Container,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ports: Vec<AnyElement> = container
            .ports
            .iter()
            .enumerate()
            .map(|(port_ix, port)| {
                let target = pod.resource();
                let remote = port.port;
                let id =
                    gpui::ElementId::NamedInteger(format!("forward-{ix}").into(), port_ix as u64);
                div()
                    .flex()
                    .gap(SPACE_4)
                    .child(div().text_color(palette.fg).child(port.label()))
                    .child(
                        Link::new(id, "Forward").on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(Command::PortForward {
                                service: target.clone(),
                                local: remote,
                                remote,
                            });
                        })),
                    )
                    .into_any_element()
            })
            .collect();
        let state_color = match (container.state, container.status) {
            (ContainerState::Terminated, Status::Ok) => palette.mute,
            (_, status) => palette.status(status),
        };
        let fields = container_fields(container, ports, palette);
        let name = container.name.clone();
        let before_restart = container.last_termination.as_ref().map(|_| {
            let link = Link::new(
                gpui::ElementId::NamedInteger("pod-logs-before-restart".into(), ix as u64),
                LOGS_BEFORE_RESTART,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.show_previous_logs(name.clone(), cx);
            }));
            div().pl(CONTAINER_LABEL + SPACE_5).child(link)
        });
        div()
            .flex()
            .flex_col()
            .gap(SPACE_3)
            .child(
                div()
                    .debug_selector(|| CONTAINER_HEAD.into())
                    .flex()
                    .flex_wrap()
                    .items_baseline()
                    .gap(SPACE_4)
                    .font_family(FONT_MONO)
                    .text_size(TEXT_MONO)
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(container.name.clone()),
                    )
                    .child(
                        div()
                            .debug_selector(|| CONTAINER_STATE.into())
                            .text_color(state_color)
                            .child(container.state_label()),
                    ),
            )
            .child(fields)
            .children(before_restart)
            .into_any_element()
    }
}

fn container_fields(container: &Container, ports: Vec<AnyElement>, palette: &Palette) -> KeyValue {
    let resources =
        |side: &str, resources: &Resources| match (resources.label(), resources.written()) {
            (Some(label), Some(written)) => div()
                .id(SharedString::from(format!(
                    "container-{side}-{}",
                    container.name
                )))
                .tooltip(Tooltip::text(written))
                .child(value(label, palette.fg))
                .into_any_element(),
            _ => value(NO_RESOURCES, palette.mute),
        };
    let restarts_color = if container.restarts > 0 {
        palette.warn
    } else {
        palette.mute
    };
    let probes = container
        .probes_label()
        .map(|probes| value(probes, palette.fg2));
    let run = container
        .terminated
        .as_ref()
        .and_then(Termination::run)
        .map(|run| value(run, palette.fg));
    let mut fields = KeyValue::new().label_width(CONTAINER_LABEL).row(
        "restarts",
        value(container.restarts.to_string(), restarts_color),
    );
    if let Some(last) = &container.last_termination {
        fields = fields.row("last exit", value(last.detail(), palette.warn));
        if let Some(run) = last.run() {
            fields = fields.row("last run", value(run, palette.fg2));
        }
    }
    let fields = fields
        .row("image", value(container.image.clone(), palette.fg))
        .row(REQUESTS, resources(REQUESTS, &container.requests))
        .row(LIMITS, resources(LIMITS, &container.limits));
    let fields = match probes {
        Some(probes) => fields.row("probes", probes),
        None => fields,
    };
    let fields = match run {
        Some(run) => fields.row("ran", run),
        None => fields,
    };
    if ports.is_empty() {
        return fields;
    }
    let ports = div().flex().flex_col().gap(SPACE_2).children(ports);
    fields.row("ports", ports)
}

fn value(text: impl Into<SharedString>, color: gpui::Hsla) -> AnyElement {
    div()
        .min_w_0()
        .text_color(color)
        .child(SelectableText::new(text))
        .into_any_element()
}

fn conditions(pod: &Pod, palette: &Palette) -> PanelSection {
    let all_true = pod.conditions.iter().all(Condition::is_true);
    let color = if all_true { palette.ok } else { palette.mute };
    PanelSection::new(INSPECTOR_ID, CONDITIONS)
        .count(true_label(&pod.conditions))
        .count_color(color)
        .closed()
        .child(ConditionTable::new(&pod.conditions))
}

fn missing(palette: &Palette) -> AnyElement {
    div()
        .text_color(palette.mute)
        .child(MISSING)
        .into_any_element()
}

fn or_missing(value: Option<String>) -> SharedString {
    value.map_or_else(|| MISSING.into(), SharedString::from)
}

fn volumes(pod: &Pod) -> PanelSection {
    let rows = pod.volumes.iter().fold(
        KeyValue::new().label_width(INSPECTOR_LABEL),
        |grid, volume| grid.text(volume.name.clone(), volume.source.clone()),
    );
    PanelSection::new(INSPECTOR_ID, VOLUMES)
        .count(pod.volumes.len().to_string())
        .child(rows)
}

fn logs_target(
    marked: &[&Pod],
    selector: Option<&(LogTarget, String)>,
) -> Option<(LogTarget, String)> {
    if marked.len() > 1 {
        let pods: Vec<ResourceRef> = marked.iter().map(|pod| pod.resource()).collect();
        let label = format!("{} {SELECTED_PODS}", marked.len());
        return LogTarget::pods(&pods).map(|target| (target, label));
    }
    selector.cloned()
}

fn open_logs(target: LogTarget, cx: &mut Context<PodsScreen>) {
    cx.emit(Command::NewTab(Box::new(Command::FollowLogs(target))));
}

fn menu_items(
    pod: &Pod,
    screen: &WeakEntity<PodsScreen>,
    logs: Option<(LogTarget, String)>,
) -> Vec<MenuItem> {
    let screen = screen.clone();
    let logs_screen = screen.clone();
    let logs_pod = pod.resource();
    let open_screen = screen.clone();
    let open = new_tab_item(Command::OpenResource(pod.resource()), move |command, cx| {
        let _ = open_screen.update(cx, |_, cx| cx.emit(command));
    });
    let edit_screen = screen.clone();
    let edit = edit_item(pod.resource(), move |command, cx| {
        let _ = edit_screen.update(cx, |_, cx| cx.emit(command));
    });
    let mut items = vec![
        open,
        edit,
        MenuItem::new("Logs", move |_, cx| {
            let pod = logs_pod.clone();
            let _ = logs_screen.update(cx, |this, cx| {
                this.select(Some(pod), cx);
                this.refresh(cx);
                this.show_logs(cx);
            });
        })
        .description("Follow this pod's output in the dock."),
    ];
    if let Some((target, label)) = logs {
        let description = if matches!(target, LogTarget::Selector { .. }) {
            SELECTOR_LOGS_DESCRIPTION
        } else {
            MARKED_LOGS_DESCRIPTION
        };
        let selected_screen = screen.clone();
        items.push(
            MenuItem::new(format!("{LOGS_FOR} {label}"), move |_, cx| {
                let target = target.clone();
                let _ = selected_screen.update(cx, |_, cx| open_logs(target, cx));
            })
            .description(description),
        );
    }
    for (container, port) in pod.ports() {
        let target = pod.resource();
        let forward_screen = screen.clone();
        let remote = port.port;
        items.push(
            MenuItem::new(format!("Port-forward :{remote}"), move |_, cx| {
                let command = Command::PortForward {
                    service: target.clone(),
                    local: remote,
                    remote,
                };
                let _ = forward_screen.update(cx, |_, cx| cx.emit(command));
            })
            .description(format!(
                "localhost:{remote} → {} {}",
                container.name,
                port.label()
            )),
        );
    }
    let target = pod.resource();
    let seen = Seen::new(target.clone(), pod.uid.clone());
    let replaced = pod.owner.is_some();
    let consequence = if replaced {
        "Its controller creates a replacement within seconds; logs from its containers are lost."
    } else {
        "Nothing recreates it; logs from its containers are lost."
    };
    let detail = format!("{} · {}", target.cluster, target);
    items.push(
        MenuItem::new("Delete pod…", move |window, cx| {
            let seen = seen.clone();
            let screen = screen.clone();
            let dialog = ConfirmDialog::new(
                "Delete pod",
                format!("The pod is deleted now. {consequence}"),
                "Delete pod",
                move |_, cx| {
                    let command = Command::Delete(seen.clone());
                    let _ = screen.update(cx, |_, cx| cx.emit(command));
                },
            )
            .detail(detail.clone());
            confirm(dialog, window, cx);
        })
        .destructive()
        .description(consequence),
    );
    items
}

fn text(value: impl Into<SharedString>, color: gpui::Hsla) -> AnyElement {
    div()
        .truncate()
        .text_color(color)
        .child(value.into())
        .into_any_element()
}

fn ready_color(pod: &Pod, palette: &Palette) -> gpui::Hsla {
    if pod.phase == PodPhase::Succeeded {
        palette.mute
    } else if pod.all_ready() {
        palette.fg2
    } else {
        palette.warn
    }
}

fn usage(pod: &Pod, label: fn(Usage) -> Amount) -> Amount {
    pod.usage.map_or_else(Amount::missing, label)
}

fn columns() -> Vec<Column<Pod>> {
    vec![
        Column::text("Name", |p: &Pod| p.name.clone().into())
            .highlight()
            .mono()
            .flex(NAME_WEIGHT, NAME_MIN)
            .sort_by_key(|p: &Pod| p.name.clone()),
        Column::colored_text(
            "Owner",
            |p: &Pod| p.owner_label().unwrap_or_else(|| MISSING.into()).into(),
            |_, palette| palette.fg2,
        )
        .mono()
        .flex(OWNER_WEIGHT, OWNER_MIN)
        .sort_by_key(Pod::owner_label),
        Column::new("Ready", |p: &Pod, palette| {
            text(p.ready_label(), ready_color(p, palette))
        })
        .mono()
        .end()
        .fixed(READY)
        .sort_by_key(|p: &Pod| p.all_ready()),
        Column::new("Restarts", |p: &Pod, palette| {
            let color = p
                .restart_status()
                .map_or(palette.mute, |status| palette.status(status));
            text(p.restarts().to_string(), color)
        })
        .mono()
        .end()
        .fixed(RESTARTS)
        .sort_by_key(|p: &Pod| std::cmp::Reverse(p.restarts())),
        Column::new("Status", |p: &Pod, _| {
            StatusText::new(p.status, p.reason.clone()).into_any_element()
        })
        .fit_status_by(|p: &Pod| p.reason.clone().into())
        .auto(STATUS_MIN, STATUS_MAX)
        .sort_by_key(|p: &Pod| std::cmp::Reverse(p.status)),
        Column::colored_text(
            "Node",
            |p: &Pod| p.node_short().unwrap_or(MISSING).to_owned().into(),
            |_, palette| palette.fg2,
        )
        .highlight()
        .mono()
        .flex(NODE_WEIGHT, NODE_MIN)
        .sort_by_key(|p: &Pod| p.node.clone()),
        Column::amount("CPU", |p: &Pod| usage(p, Usage::cpu_label))
            .mono()
            .end()
            .fixed(CPU)
            .sort_by_key(|p: &Pod| std::cmp::Reverse(p.usage.map(|u| u.cpu_millis))),
        Column::amount("Memory", |p: &Pod| usage(p, Usage::memory_label))
            .mono()
            .end()
            .fixed(MEMORY)
            .sort_by_key(|p: &Pod| std::cmp::Reverse(p.usage.map(|u| u.memory_mib))),
        Column::new("Age", |p: &Pod, palette| {
            text(flock_core::time::age(p.age), palette.mute)
        })
        .mono()
        .end()
        .fixed(AGE)
        .sort_by_key(|p: &Pod| p.age),
    ]
}

fn filter_bar(
    namespace: &Entity<Select>,
    window: &mut Window,
    cx: &mut Context<PodsScreen>,
) -> (Entity<FilterBar>, Entity<ElsewhereLink>) {
    let screen = cx.weak_entity();
    let elsewhere = cx.new(|_| ElsewhereLink {
        elsewhere: None,
        screen,
    });
    let filter = cx.new(|cx| {
        FilterBar::new("pod-filter", window, cx)
            .scope(namespace.clone())
            .aside(elsewhere.clone())
    });
    (filter, elsewhere)
}

struct ElsewhereLink {
    elsewhere: Option<FailingElsewhere>,
    screen: WeakEntity<PodsScreen>,
}

impl Render for ElsewhereLink {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let link = self.elsewhere.as_ref().map(|elsewhere| {
            let screen = self.screen.clone();
            Link::new(FAILING_ELSEWHERE, elsewhere.label())
                .mono()
                .color(palette.err)
                .on_click(move |_, _, cx| {
                    let _ = screen.update(cx, PodsScreen::go_elsewhere);
                })
        });
        div()
            .min_w_0()
            .truncate()
            .text_size(TEXT_SMALL)
            .children(link)
    }
}

impl Render for PodsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let inspector = self.panel_open.then(|| self.inspector(cx));
        let dock = self.dock_view(cx).into_any_element();
        let namespace = self.query.namespace.as_deref().map(|namespace| {
            let inside = self.pods.iter().filter(|pod| {
                Some(&pod.cluster) == self.cluster.as_ref() && pod.namespace == namespace
            });
            (namespace, inside.count())
        });
        let shape = ListShape {
            shown: self.shown,
            total: self.cluster_total(),
            namespace,
            noun: Noun::of(Page::Pods),
            cluster: self.cluster_name(),
        };
        let screen = cx.weak_entity();
        let show_all: ShowAll = Rc::new(move |_, cx| {
            let _ = screen.update(cx, PodsScreen::show_all_namespaces);
        });
        let list: AnyElement = match empty_list(&shape, &self.filter, Some(show_all), cx) {
            Some(empty) => empty.into_any_element(),
            None => self.table.clone().into_any_element(),
        };
        let table = div().flex_1().min_w_0().h_full().child(list);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.filter.clone())
            .child(div().flex_1().min_h_0().flex().child(table).children(
                inspector.map(|inspector| {
                    InspectorPane::new(INSPECTOR_ID, inspector).width(INSPECTOR_POD)
                }),
            ))
            .child(dock)
    }
}
