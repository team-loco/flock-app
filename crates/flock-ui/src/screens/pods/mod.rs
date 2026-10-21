use std::collections::BTreeMap;
use std::rc::Rc;

use crate::kit::SelectableText;
use flock_core::diagnosis::true_label;
use flock_core::emptiness::Noun;
use flock_core::filter::label_values;
use flock_core::multilog::LabelSelector;
use flock_core::pods::{
    Container, ContainerState, FailingElsewhere, OWNER_ARROW, Pod, PodFilter, PodPhase, PodQuery,
    Resources, Termination, Usage, filter_counts, nav_counts, owner_segment,
};
use flock_core::related::Related;
use flock_core::resources::{NamespaceCount, POD_LIST, namespace_counts};
use flock_core::settings::NamespaceSelection;
use flock_core::{
    ClusterId, Command, Condition, Counts, LogInstance, LogTarget, Page, Preferences, ResourceRef,
    Seen, Snapshot, Status,
};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, WeakEntity, Window, div, prelude::FluentBuilder, px, svg,
};

mod streams;

use streams::{LogSession, ShellSession, TERMINAL_ID};

use crate::assets::{CHEVRON_DOWN, CHEVRON_RIGHT};
use crate::kit::{
    Button, CONDITIONS, Column, ConditionTable, ConfirmDialog, DockState, EVENTS, Entrance,
    EventList, FilterBar, FilterEvent, InputEvent, InspectorPane, KeyValue, LOG_FILTER_PLACEHOLDER,
    Link, LogView, MenuItem, SectionLabel, Select, SelectEvent, ShowAll, StatusText, Table,
    TableEvent, Terminal, TextFieldState, WHATS_WRONG, WhatsWrong, chip_label, confirm, swap_in,
};
use crate::screens::related::{OnPick, Pick, RELATED, inspector_section};
use crate::screens::{
    ALL_NAMESPACES, ListShape, chosen_namespace, edit_item, empty_list, new_tab_item,
    set_namespace_options,
};
use crate::theme::{
    ActiveTheme, FONT_MONO, FONT_SANS, ICON, INSPECTOR_POD, LINE_PROSE, Palette, RULE, SPACE_2,
    SPACE_3, SPACE_4, SPACE_5, SPACE_6, SPACE_8, TEXT_BODY, TEXT_MONO, TEXT_SMALL, TEXT_UI,
    WHATS_WRONG_BAR,
};

const INSPECTOR_ID: &str = "pod-inspector";
const CONDITIONS_TOGGLE: &str = "pod-conditions-toggle";
const FAILING_ELSEWHERE: &str = "pod-failing-elsewhere";
const OWNER_LINK: &str = "pod-owner";
const NAME_MIN: Pixels = px(180.);
const NAME_WEIGHT: f32 = 2.;
const OWNER_MIN: Pixels = px(96.);
const OWNER_WEIGHT: f32 = 1.;
const READY: Pixels = px(48.);
const RESTARTS: Pixels = px(64.);
const STATUS_MIN: Pixels = px(92.);
const STATUS_MAX: Pixels = px(220.);
const NODE_MIN: Pixels = px(100.);
const NODE_WEIGHT: f32 = 1.;
const CPU: Pixels = px(52.);
const MEMORY: Pixels = px(60.);
const AGE: Pixels = px(44.);
const NAMESPACE_SELECT: Pixels = px(180.);
const CONTAINER_SELECT: Pixels = px(160.);
const INSPECTOR_LABEL: Pixels = px(96.);
const CONTAINER_LABEL: Pixels = px(80.);
const SEPARATOR: &str = " · ";

const MISSING: &str = "—";
const NO_RESOURCES: &str = "none";
const LAST_TERMINATION: &str = "Last termination";
const LOGS_BEFORE_RESTART: &str = "Logs before restart →";
const LOGS_FOR: &str = "Logs for";
const LOGS_FOR_SELECTED: &str = "Logs for selected pods";
const SELECTED_PODS: &str = "selected pods";
const CLEAR_SELECTION: &str = "Clear selection";
const MARK_HINT: &str = "⇧-click rows to add or remove pods";
const SELECTOR_HINT: &str = "Follows every pod that matches the label filter, including new ones";
const NO_SELECTION: &str = "Select a pod to see its containers, conditions and events.";
pub struct PodsScreen {
    pods: Vec<Pod>,
    cluster: Option<ClusterId>,
    names: Vec<(ClusterId, String)>,
    cluster_namespaces: BTreeMap<ClusterId, Vec<String>>,
    shown: usize,
    query: PodQuery,
    selected: Option<ResourceRef>,
    related: Vec<Related>,
    container: Option<String>,
    instance: LogInstance,
    conditions_open: bool,
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
        let screen = cx.weak_entity();
        let table = cx.new(|cx| {
            Table::new("pods", columns(), cx)
                .row_actions(move |pod: &Pod| menu_items(pod, &screen))
                .row_key(|pod: &Pod| format!("{}/{}", pod.namespace, pod.name).into())
                .multi_select()
                .column_gap(SPACE_6)
        });
        let search =
            cx.new(|cx| TextFieldState::new(window, cx).placeholder(LOG_FILTER_PLACEHOLDER));
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
                    this.select(Some(pod), cx);
                }
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
            related: Vec::new(),
            container: None,
            instance: LogInstance::Current,
            conditions_open: false,
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
            self.pick_container(None);
        }
        self.refresh(cx);
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

    pub fn header_note(&self) -> (Counts, String) {
        let total = self
            .cluster
            .as_ref()
            .and_then(|cluster| nav_counts(&self.pods, cluster).total)
            .unwrap_or_default();
        (Counts::default(), total.to_string())
    }

    fn logs_target(&self, cx: &gpui::App) -> Option<(LogTarget, String)> {
        let marked: Vec<ResourceRef> = self
            .table
            .read(cx)
            .marked_rows()
            .into_iter()
            .map(Pod::resource)
            .collect();
        if marked.len() > 1 {
            let label = format!("{} {SELECTED_PODS}", marked.len());
            return LogTarget::pods(&marked).map(|target| (target, label));
        }
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

    fn logs_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (target, label) = self.logs_target(cx)?;
        let palette = *cx.palette();
        let marked = matches!(target, LogTarget::Pods { .. });
        let open = Button::new("pods-logs-selected", format!("{LOGS_FOR} {label}"))
            .small()
            .on_click(cx.listener(move |_, _, _, cx| open_logs(target.clone(), cx)));
        let clear = marked.then(|| {
            Button::new("pods-clear-selection", CLEAR_SELECTION)
                .small()
                .ghost()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.table.update(cx, Table::clear_marks);
                }))
        });
        let hint = if marked { MARK_HINT } else { SELECTOR_HINT };
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(SPACE_4)
                .px(SPACE_8)
                .py(SPACE_3)
                .border_b(RULE)
                .border_color(palette.line)
                .child(open)
                .children(clear)
                .child(
                    div()
                        .text_size(TEXT_SMALL)
                        .text_color(palette.mute)
                        .child(hint),
                )
                .into_any_element(),
        )
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
        self.table.update(cx, |table, cx| {
            if let Some(scope) = scope {
                table.set_scope(scope, cx);
            }
            table.set_highlight(words, cx);
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

    fn toggle_conditions(&mut self, cx: &mut Context<Self>) {
        self.conditions_open = !self.conditions_open;
        cx.notify();
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
        if let Some(pod) = self.selected.clone() {
            cx.emit(Command::InspectResource(pod));
        }
        self.pick_container(None);
        self.instance = LogInstance::Current;
        self.sync_selection(cx);
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
        let related = section(&palette)
            .child(SectionLabel::new(RELATED))
            .child(inspector_section(related, &pick, cx));
        let conditions_open = self.conditions_open;
        let (header, body) = match self.selected_pod() {
            Some(pod) => (
                Some(Self::pod_header(pod, &palette, cx)),
                Self::pod_details(pod, conditions_open, &palette, cx)
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
                    .id(INSPECTOR_ID)
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

    fn pod_header(pod: &Pod, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let shell = Button::new("pod-shell", "Shell")
            .primary()
            .on_click(cx.listener(|this, _, window, cx| this.open_terminal(window, cx)));
        let header = section(palette)
            .gap(SPACE_3)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_4)
                    .child(SectionLabel::new("Pod"))
                    .child(div().flex_1())
                    .child(shell),
            )
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(TEXT_UI)
                    .font_weight(FontWeight::MEDIUM)
                    .child(pod.name.clone()),
            )
            .child(StatusText::new(pod.status, pod.reason.clone()))
            .child(
                div()
                    .text_color(palette.fg2)
                    .line_height(gpui::relative(LINE_PROSE))
                    .child(SelectableText::new(pod.sentence())),
            );
        header.flex_none().into_any_element()
    }

    fn pod_details(
        pod: &Pod,
        conditions_open: bool,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let whats_wrong = pod.diagnosis().map(|diagnosis| {
            section(palette)
                .child(SectionLabel::new(WHATS_WRONG))
                .child(WhatsWrong::new(pod.status, diagnosis))
        });
        div()
            .flex()
            .flex_col()
            .children(whats_wrong)
            .child(Self::facts(pod, palette, cx))
            .child(Self::containers_section(pod, palette, cx))
            .child(Self::conditions(pod, conditions_open, palette, cx))
            .child(events(pod, palette))
            .child(labels(pod, palette))
            .child(volumes(pod, palette))
    }

    fn facts(pod: &Pod, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = Self::owner_chain(pod, palette, cx);
        let created = format!("{} ago", flock_core::time::age(pod.age));
        let ready = div()
            .text_color(palette.status(pod.status))
            .child(format!("{} containers", pod.ready_label()));
        section(palette).child(
            KeyValue::new()
                .label_width(INSPECTOR_LABEL)
                .text("Namespace", pod.namespace.clone())
                .text("Node", or_missing(pod.node.clone()))
                .text("Pod IP", or_missing(pod.ip.clone()))
                .row("Ready", ready)
                .row("Controlled by", owner)
                .text("QoS", pod.qos.label())
                .text("Service account", or_missing(pod.service_account.clone()))
                .text("Created", created),
        )
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

    fn conditions(
        pod: &Pod,
        open: bool,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let all_true = pod.conditions.iter().all(Condition::is_true);
        let summary_color = if all_true { palette.ok } else { palette.fg2 };
        let chevron = if open { CHEVRON_DOWN } else { CHEVRON_RIGHT };
        let toggle = div()
            .debug_selector(|| CONDITIONS_TOGGLE.into())
            .id(CONDITIONS_TOGGLE)
            .flex()
            .items_center()
            .gap(SPACE_4)
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.toggle_conditions(cx)))
            .child(SectionLabel::new(CONDITIONS))
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(TEXT_SMALL)
                    .text_color(summary_color)
                    .child(true_label(&pod.conditions)),
            )
            .child(div().flex_1())
            .child(svg().path(chevron).size(ICON).text_color(palette.mute));
        section(palette).child(toggle).when(open, |section| {
            section.child(ConditionTable::new(&pod.conditions))
        })
    }

    fn containers_section(
        pod: &Pod,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rows: Vec<AnyElement> = pod
            .containers
            .iter()
            .enumerate()
            .map(|(ix, container)| Self::container_row(pod, ix, container, palette, cx))
            .collect();
        let label = format!("Containers{SEPARATOR}{}", pod.containers.len());
        section(palette)
            .gap(SPACE_3)
            .child(SectionLabel::new(label))
            .children(rows)
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
        let restarts_color = if container.restarts > 0 {
            palette.warn
        } else {
            palette.mute
        };
        let fields = container_fields(container, ports, palette);
        let name = container.name.clone();
        let before_restart = Link::new(
            gpui::ElementId::NamedInteger("pod-logs-before-restart".into(), ix as u64),
            LOGS_BEFORE_RESTART,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.show_previous_logs(name.clone(), cx);
        }));
        let last = container
            .last_termination
            .as_ref()
            .map(|last| last_termination(last, before_restart, palette));
        div()
            .flex()
            .flex_col()
            .gap(SPACE_3)
            .pt(SPACE_3)
            .border_t(RULE)
            .border_color(palette.line)
            .child(
                div()
                    .flex()
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
                            .min_w_0()
                            .truncate()
                            .text_color(state_color)
                            .child(container.state_label()),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex_none()
                            .text_size(TEXT_SMALL)
                            .text_color(restarts_color)
                            .child(container.restarts_label()),
                    ),
            )
            .child(fields)
            .children(last)
            .into_any_element()
    }
}

fn container_fields(container: &Container, ports: Vec<AnyElement>, palette: &Palette) -> KeyValue {
    let resources = |resources: &Resources| match resources.label() {
        Some(label) => value(label, palette.fg),
        None => value(NO_RESOURCES, palette.mute),
    };
    let probes = container
        .probes_label()
        .map(|probes| value(probes, palette.fg2));
    let run = container
        .terminated
        .as_ref()
        .and_then(Termination::run)
        .map(|run| value(run, palette.fg));
    let fields = KeyValue::new()
        .label_width(CONTAINER_LABEL)
        .row("image", value(container.image.clone(), palette.fg))
        .row("requests", resources(&container.requests))
        .row("limits", resources(&container.limits));
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

fn last_termination(last: &Termination, logs: Link, palette: &Palette) -> AnyElement {
    let reason = Some(last.reason.clone()).filter(|reason| !reason.is_empty());
    let summary = div()
        .flex()
        .flex_wrap()
        .children(reason.map(|reason| {
            div()
                .text_color(palette.warn)
                .child(format!("{reason}{SEPARATOR}"))
        }))
        .child(div().text_color(palette.fg).child(last.detail()));
    let span = last
        .span()
        .map(|span| div().text_color(palette.mute).child(span));
    div()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .px(SPACE_4)
        .py(SPACE_3)
        .border_l(WHATS_WRONG_BAR)
        .border_color(palette.warn)
        .bg(palette.warnbg)
        .font_family(FONT_MONO)
        .text_size(TEXT_SMALL)
        .child(SectionLabel::new(LAST_TERMINATION))
        .child(summary)
        .children(span)
        .child(div().font_family(FONT_SANS).text_size(TEXT_UI).child(logs))
        .into_any_element()
}

fn section(palette: &Palette) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(SPACE_4)
        .px(SPACE_6)
        .py(SPACE_5)
        .border_b(RULE)
        .border_color(palette.line)
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

fn labels(pod: &Pod, palette: &Palette) -> impl IntoElement {
    let items = pod.labels.iter().map(|(key, value)| {
        div()
            .min_w_0()
            .max_w_full()
            .font_family(FONT_MONO)
            .text_size(TEXT_SMALL)
            .text_color(palette.fg2)
            .child(SelectableText::new(format!("{key}={value}")))
    });
    section(palette).child(SectionLabel::new("Labels")).child(
        div()
            .flex()
            .flex_wrap()
            .gap_x(SPACE_5)
            .gap_y(SPACE_2)
            .children(items),
    )
}

fn volumes(pod: &Pod, palette: &Palette) -> impl IntoElement {
    let rows = pod.volumes.iter().fold(
        KeyValue::new().label_width(INSPECTOR_LABEL),
        |grid, volume| grid.text(volume.name.clone(), volume.source.clone()),
    );
    section(palette)
        .child(SectionLabel::new("Volumes"))
        .child(rows)
}

fn events(pod: &Pod, palette: &Palette) -> impl IntoElement {
    section(palette)
        .child(SectionLabel::new(EVENTS))
        .child(EventList::new(pod.events.clone()))
}

fn open_logs(target: LogTarget, cx: &mut Context<PodsScreen>) {
    cx.emit(Command::NewTab(Box::new(Command::FollowLogs(target))));
}

fn menu_items(pod: &Pod, screen: &WeakEntity<PodsScreen>) -> Vec<MenuItem> {
    let screen = screen.clone();
    let logs_screen = screen.clone();
    let logs_pod = pod.resource();
    let selected_screen = screen.clone();
    let selected_pod = pod.resource();
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
        MenuItem::new(LOGS_FOR_SELECTED, move |_, cx| {
            let _ = selected_screen.update(cx, |this, cx| {
                let marked: Vec<ResourceRef> = this
                    .table
                    .read(cx)
                    .marked_rows()
                    .into_iter()
                    .map(Pod::resource)
                    .collect();
                let pods = if marked.contains(&selected_pod) {
                    marked
                } else {
                    vec![selected_pod.clone()]
                };
                if let Some(target) = LogTarget::pods(&pods) {
                    open_logs(target, cx);
                }
            });
        })
        .description("Merge the output of every ⇧-clicked pod in one tab."),
    ];
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

fn usage_text(pod: &Pod, label: fn(Usage) -> String) -> SharedString {
    pod.usage
        .map_or_else(|| MISSING.into(), |usage| label(usage).into())
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
        Column::colored_text(
            "CPU",
            |p: &Pod| usage_text(p, Usage::cpu_label),
            |_, palette| palette.fg2,
        )
        .mono()
        .end()
        .fixed(CPU)
        .sort_by_key(|p: &Pod| std::cmp::Reverse(p.usage.map(|u| u.cpu_millis))),
        Column::colored_text(
            "Memory",
            |p: &Pod| usage_text(p, Usage::memory_label),
            |_, palette| palette.fg2,
        )
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
        FilterBar::new("pod-filter", Pod::FILTER.placeholder(), window, cx)
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
        div().text_size(TEXT_SMALL).children(link)
    }
}

impl Render for PodsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let inspector = self.inspector(cx);
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
            .children(self.logs_bar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(table)
                    .child(InspectorPane::new(INSPECTOR_ID, inspector).width(INSPECTOR_POD)),
            )
            .child(dock)
    }
}
