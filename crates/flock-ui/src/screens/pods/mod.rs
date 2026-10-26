use std::collections::BTreeMap;
use std::ops::Deref;
use std::rc::Rc;
use std::sync::Arc;

use flock_core::emptiness::Noun;
use flock_core::filter::label_values;
use flock_core::multilog::LabelSelector;
use flock_core::pods::{
    Container, FailingElsewhere, Pod, PodFilter, PodPhase, PodQuery, Usage, filter_counts,
    nav_counts,
};
use flock_core::related::Related;
use flock_core::resources::{NamespaceCount, POD_LIST, namespace_counts};
use flock_core::settings::NamespaceSelection;
use flock_core::units::Amount;
use flock_core::{
    ClusterId, Command, LogInstance, LogTarget, Page, Preferences, ResourceRef, Seen, Snapshot,
};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement, Pixels,
    Render, SharedString, StyleRefinement, Styled, Subscription, WeakEntity, Window, div, px,
};

mod inspector;
mod streams;

pub(crate) use streams::exit_note;

use inspector::PodInspector;
use streams::{LogSession, ShellSession, TERMINAL_ID};

use crate::kit::{
    ActionMenu, Column, ConfirmDialog, DockState, FILTER_PLACEHOLDER, FilterBar, FilterEvent,
    InputEvent, InspectorPane, Link, LogView, MenuItem, Select, SelectEvent, ShowAll, StatusText,
    Table, TableEvent, Terminal, TextFieldState, chip_label, confirm,
};
use crate::screens::{
    ALL_NAMESPACES, ListShape, chosen_namespace, edit_item, empty_list, new_tab_item,
    set_namespace_options,
};
use crate::theme::{ActiveTheme, INSPECTOR_POD, Palette, SPACE_6, TEXT_SMALL};

pub(super) const INSPECTOR_ID: &str = "pod-inspector";
const FAILING_ELSEWHERE: &str = "pod-failing-elsewhere";
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
pub(super) const CONTAINER_SELECT: Pixels = px(160.);

pub(super) const MISSING: &str = "—";
const LOGS_FOR: &str = "Logs for";
const SELECTED_PODS: &str = "selected pods";
const MARKED_LOGS_DESCRIPTION: &str = "Merge the output of every ⇧-clicked pod in one tab.";
const SELECTOR_LOGS_DESCRIPTION: &str =
    "Follows every pod that matches the label filter, including new ones";
pub struct PodsScreen {
    snapshot: Arc<Snapshot>,
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
    table: Entity<Table<PodRow>>,
    filter: Entity<FilterBar>,
    namespace: Entity<Select>,
    preferences: Preferences,
    containers: Entity<Select>,
    search: Entity<TextFieldState>,
    clear_search: bool,
    options: Entity<ActionMenu>,
    logs: Entity<LogView>,
    terminal: Entity<Terminal>,
    dock: Entity<DockState>,
    inspector: Entity<PodInspector>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Command> for PodsScreen {}

#[derive(Clone)]
struct PodRow {
    snapshot: Arc<Snapshot>,
    ix: usize,
}

impl PodRow {
    fn all(snapshot: &Arc<Snapshot>, matched: &[&Pod]) -> Vec<Self> {
        matched
            .iter()
            .filter_map(|pod| snapshot.pods.element_offset(pod))
            .map(|ix| Self {
                snapshot: Arc::clone(snapshot),
                ix,
            })
            .collect()
    }
}

impl Deref for PodRow {
    type Target = Pod;

    fn deref(&self) -> &Pod {
        &self.snapshot.pods[self.ix]
    }
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
                .row_key(|pod: &PodRow| format!("{}/{}", pod.namespace, pod.name).into())
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
        let options = cx.new(|cx| ActionMenu::new("pod-logs-options", cx));
        let dock = cx.new(|_| DockState::collapsed());
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
        let screen = cx.weak_entity();
        let inspector = cx.new(|_| PodInspector::new(screen));
        Self {
            snapshot: Arc::default(),
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
            clear_search: false,
            options,
            logs,
            terminal,
            dock,
            inspector,
            _subscriptions: subscriptions,
        }
    }

    pub fn set_snapshot(&mut self, snapshot: &Arc<Snapshot>) {
        self.snapshot = Arc::clone(snapshot);
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
        let wanted = resource.filter(|r| self.snapshot.pods.iter().any(|pod| pod.is(r)));
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
            .and_then(|cluster| nav_counts(&self.snapshot.pods, cluster).total)
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
            Some(cluster) => self.query.base(&self.snapshot.pods, cluster),
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
            .snapshot
            .pods
            .iter()
            .filter(|pod| self.cluster.as_ref() == Some(&pod.cluster))
            .map(|pod| pod.namespace.as_str());
        namespace_counts(known, rows, self.query.namespace.as_deref())
    }

    fn selected_pod(&self) -> Option<&Pod> {
        let selected = self.selected.as_ref()?;
        self.snapshot.pods.iter().find(|pod| pod.is(selected))
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
            self.snapshot
                .pods
                .iter()
                .filter(|pod| self.cluster.as_ref() == Some(&pod.cluster))
                .filter(|pod| namespace.is_none_or(|namespace| &pod.namespace == namespace))
                .map(|pod| &pod.labels),
        );
        self.filter.update(cx, |bar, cx| bar.set_labels(labels, cx));
        let rows: Vec<PodRow> = match &self.cluster {
            Some(cluster) => {
                let matched = self.query.apply(&self.snapshot.pods, cluster);
                PodRow::all(&self.snapshot, &matched)
            }
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
            .and_then(|cluster| self.query.failing_elsewhere(&self.snapshot.pods, cluster));
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
            self.selected = rows.first().map(|row| row.resource());
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
                move |pod: &PodRow, marked: &[&PodRow]| {
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
            self.snapshot
                .pods
                .iter()
                .filter(|pod| &pod.cluster == cluster)
                .count()
        })
    }

    fn select(&mut self, pod: Option<ResourceRef>, cx: &mut Context<Self>) {
        if self.selected == pod {
            return;
        }
        self.reset_dock(cx);
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
        table: &Entity<Table<PodRow>>,
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
        let pod = table.read(cx).row(ix).map(|row| row.resource());
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
        self.sync_inspector(cx);
        cx.notify();
    }

    fn sync_inspector(&mut self, cx: &mut Context<Self>) {
        let pod = self.selected_pod();
        let related = self.selected.as_ref().and_then(|target| {
            self.related
                .iter()
                .find(|related| &related.target == target)
        });
        self.inspector
            .update(cx, |inspector, cx| inspector.show(pod, related, cx));
    }
}

fn logs_target(
    marked: &[&PodRow],
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

fn columns() -> Vec<Column<PodRow>> {
    vec![
        Column::text("Name", |p: &PodRow| p.name.clone().into())
            .highlight()
            .mono()
            .flex(NAME_WEIGHT, NAME_MIN)
            .sort_by_key(|p: &PodRow| p.name.clone()),
        Column::colored_text(
            "Owner",
            |p: &PodRow| p.owner_label().unwrap_or_else(|| MISSING.into()).into(),
            |_, palette| palette.fg2,
        )
        .mono()
        .flex(OWNER_WEIGHT, OWNER_MIN)
        .sort_by_key(|p: &PodRow| p.owner_label()),
        Column::new("Ready", |p: &PodRow, palette| {
            text(p.ready_label(), ready_color(p, palette))
        })
        .mono()
        .end()
        .fixed(READY)
        .sort_by_key(|p: &PodRow| p.all_ready()),
        Column::new("Restarts", |p: &PodRow, palette| {
            let color = p
                .restart_status()
                .map_or(palette.mute, |status| palette.status(status));
            text(p.restarts().to_string(), color)
        })
        .mono()
        .end()
        .fixed(RESTARTS)
        .sort_by_key(|p: &PodRow| std::cmp::Reverse(p.restarts())),
        Column::new("Status", |p: &PodRow, _| {
            StatusText::new(p.status, p.reason.clone()).into_any_element()
        })
        .fit_status_by(|p: &PodRow| p.reason.clone().into())
        .auto(STATUS_MIN, STATUS_MAX)
        .sort_by_key(|p: &PodRow| std::cmp::Reverse(p.status)),
        Column::colored_text(
            "Node",
            |p: &PodRow| p.node_short().unwrap_or(MISSING).to_owned().into(),
            |_, palette| palette.fg2,
        )
        .highlight()
        .mono()
        .flex(NODE_WEIGHT, NODE_MIN)
        .sort_by_key(|p: &PodRow| p.node.clone()),
        Column::amount("CPU", |p: &PodRow| usage(p, Usage::cpu_label))
            .mono()
            .end()
            .fixed(CPU)
            .sort_by_key(|p: &PodRow| std::cmp::Reverse(p.usage.map(|u| u.cpu_millis))),
        Column::amount("Memory", |p: &PodRow| usage(p, Usage::memory_label))
            .mono()
            .end()
            .fixed(MEMORY)
            .sort_by_key(|p: &PodRow| std::cmp::Reverse(p.usage.map(|u| u.memory_mib))),
        Column::new("Age", |p: &PodRow, palette| {
            text(flock_core::time::age(p.age), palette.mute)
        })
        .mono()
        .end()
        .fixed(AGE)
        .sort_by_key(|p: &PodRow| p.age),
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.clear_search) {
            self.search
                .update(cx, |field, cx| field.set_value("", window, cx));
        }
        let inspector = self.panel_open.then(|| {
            self.inspector
                .clone()
                .cached(StyleRefinement::default().size_full())
        });
        let dock = self.dock_view(cx).into_any_element();
        let namespace = self.query.namespace.as_deref().map(|namespace| {
            let inside = self.snapshot.pods.iter().filter(|pod| {
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

#[cfg(test)]
mod tests {
    use super::*;

    const PODS: usize = 3;

    fn snapshot(cluster: &ClusterId) -> Arc<Snapshot> {
        let pods = (0..PODS).map(|ix| Pod {
            cluster: cluster.clone(),
            namespace: "default".into(),
            name: format!("web-{ix}"),
            ..Pod::default()
        });
        Arc::new(Snapshot {
            pods: pods.collect(),
            ..Snapshot::default()
        })
    }

    #[gpui::test]
    fn a_snapshot_refreshes_the_table_once_with_its_context_and_shares_its_pods(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            crate::init(Some(crate::theme::Appearance::Dark), cx)
                .unwrap_or_else(|error| panic!("UI init: {error}"));
        });
        let window = cx.add_window(PodsScreen::new);
        let cluster = ClusterId::new("a");
        let snapshot = snapshot(&cluster);
        let shown = window.update(cx, |screen, _, cx| {
            screen.set_snapshot(&snapshot);
            screen.table.read(cx).row(0).is_some()
        });
        assert_eq!(shown.ok(), Some(false));
        let shared = window.update(cx, |screen, _, cx| {
            screen.set_context(Some(&cluster), None, &Preferences::default(), cx);
            let table = screen.table.read(cx);
            let rows: Vec<&PodRow> = (0..=PODS).map_while(|ix| table.row(ix)).collect();
            let shares = |(ix, row): (usize, &&PodRow)| {
                row.ix == ix && Arc::ptr_eq(&row.snapshot, &snapshot)
            };
            rows.len() == PODS && rows.iter().enumerate().all(shares)
        });
        assert_eq!(shared.ok(), Some(true));
    }
}
