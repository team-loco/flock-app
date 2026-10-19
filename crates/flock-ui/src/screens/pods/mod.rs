use std::rc::Rc;

use crate::kit::SelectableText;
use flock_core::emptiness::Noun;
use flock_core::filter::label_values;
use flock_core::multilog::LabelSelector;
use flock_core::pods::{
    Container, Pod, PodFilter, PodQuery, filter_counts, namespaces, nav_counts,
};
use flock_core::related::Related;
use flock_core::resources::{POD_LIST, namespace_choices};
use flock_core::settings::NamespaceSelection;
use flock_core::{
    ClusterId, Command, Counts, LogTarget, Page, Preferences, ResourceRef, Seen, Snapshot,
};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window, div, px,
};

mod streams;

use streams::{LogSession, ShellSession, TERMINAL_ID};

use crate::kit::{
    Button, CONDITIONS, Column, ConditionTable, ConfirmDialog, DockState, EVENTS, Entrance,
    EventList, FilterBar, FilterEvent, InputEvent, InspectorPane, KeyValue, LOG_FILTER_PLACEHOLDER,
    Link, LogView, MenuItem, SectionLabel, Select, SelectEvent, ShowAll, StatusGlyph, StatusText,
    Table, TableEvent, Terminal, TextFieldState, WHATS_WRONG, WhatsWrong, chip_label, confirm,
    reason_tooltip, swap_in,
};
use crate::screens::related::{OnPick, Pick, RELATED, inspector_section};
use crate::screens::{ListShape, edit_item, empty_list, new_tab_item};
use crate::theme::{
    ActiveTheme, CONTAINER_SQUARE, CONTAINER_SQUARE_GAP, FONT_MONO, INSPECTOR_POD, LINE_PROSE,
    MESSAGE_MIN, Palette, RADIUS_SMALL, REASON_WEIGHT, RULE, SPACE_1, SPACE_2, SPACE_3, SPACE_4,
    SPACE_5, SPACE_6, SPACE_7, SPACE_8, TEXT_BODY, TEXT_SMALL, TEXT_UI,
};

const INSPECTOR_ID: &str = "pod-inspector";
const NAME_MIN: Pixels = px(220.);
const NAME_WEIGHT: f32 = 2.;
const NAMESPACE: Pixels = px(100.);
const CONTAINERS: Pixels = px(96.);
const RESTARTS: Pixels = px(64.);
const AGE: Pixels = px(48.);
const STATUS_MIN: Pixels = px(110.);
const STATUS_MAX: Pixels = px(260.);
const NAMESPACE_SELECT: Pixels = px(180.);
const CONTAINER_SELECT: Pixels = px(160.);
const INSPECTOR_LABEL: Pixels = px(96.);

const ALL_NAMESPACES: &str = "All namespaces";
const MISSING: &str = "—";
const MESSAGE: &str = "Message";
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
    shown: usize,
    query: PodQuery,
    selected: Option<ResourceRef>,
    related: Vec<Related>,
    container: Option<String>,
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
                .column_gap(SPACE_7)
        });
        let search =
            cx.new(|cx| TextFieldState::new(window, cx).placeholder(LOG_FILTER_PLACEHOLDER));
        let namespace = cx.new(|cx| {
            Select::new("pod-namespace", vec![ALL_NAMESPACES.into()], cx)
                .mono()
                .width(NAMESPACE_SELECT)
        });
        let filter = cx.new(|cx| FilterBar::new("pod-filter", window, cx).scope(namespace.clone()));
        let containers = cx.new(|cx| {
            Select::new("pod-container", Vec::new(), cx)
                .mono()
                .width(CONTAINER_SELECT)
        });
        let logs = cx.new(|_| LogView::new("pod-logs"));
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
            cx.subscribe(&namespace, |this, _, SelectEvent::Changed(ix), cx| {
                let chosen = ix
                    .checked_sub(1)
                    .and_then(|ix| this.namespaces().get(ix).cloned());
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
            shown: 0,
            query: PodQuery::default(),
            selected: None,
            related: Vec::new(),
            container: None,
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

    fn namespaces(&self) -> Vec<String> {
        let known = self
            .cluster
            .as_ref()
            .map(|cluster| namespaces(&self.pods, cluster))
            .unwrap_or_default();
        namespace_choices(known, self.query.namespace.as_deref())
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
        let mut options: Vec<SharedString> = vec![ALL_NAMESPACES.into()];
        options.extend(self.namespaces().into_iter().map(SharedString::from));
        let selected = self
            .query
            .namespace
            .as_ref()
            .and_then(|ns| options.iter().position(|o| o.as_ref() == ns))
            .unwrap_or_default();
        self.namespace.update(cx, |select, cx| {
            select.set_options(options, cx);
            select.set_selected(selected, cx);
        });
        let chips = filter_counts(&self.cluster_pods())
            .into_iter()
            .map(|(filter, n)| chip_label(filter.label(), n))
            .collect();
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
        let (header, body) = match self.selected_pod() {
            Some(pod) => (
                Some(Self::pod_header(pod, &palette, cx)),
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
            .on_click(cx.listener(|this, _, window, cx| this.show_terminal(window, cx)));
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

    fn pod_details(pod: &Pod, palette: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let whats_wrong = pod.diagnosis().map(|diagnosis| {
            section(palette)
                .child(SectionLabel::new(WHATS_WRONG))
                .child(WhatsWrong::new(pod.status, diagnosis))
        });
        div()
            .flex()
            .flex_col()
            .children(whats_wrong)
            .child(conditions(pod, palette))
            .child(events(pod, palette))
            .child(Self::facts(pod, palette, cx))
            .child(labels(pod, palette))
            .child(Self::containers_section(pod, palette, cx))
            .child(volumes(pod, palette))
    }

    fn facts(pod: &Pod, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = match &pod.owner {
            Some(owner) => {
                let target = owner.clone();
                let link = Link::new("pod-owner", format!("{}/{}", owner.kind, owner.name))
                    .mono()
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(Command::OpenResource(target.clone()));
                    }));
                div().min_w_0().truncate().child(link).into_any_element()
            }
            None => missing(palette),
        };
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
                .text("Created", created),
        )
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
        section(palette)
            .gap(SPACE_5)
            .child(SectionLabel::new("Containers"))
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
                    .font_family(FONT_MONO)
                    .text_size(TEXT_SMALL)
                    .child(div().text_color(palette.fg2).child(port.label()))
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
        div()
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_3)
                    .child(StatusGlyph::new(container.status))
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::MEDIUM)
                            .child(container.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(TEXT_SMALL)
                            .text_color(palette.status(container.status))
                            .child(container.reason.clone()),
                    ),
            )
            .child(mono_line(container.image.clone(), palette.fg2))
            .child(mono_line(container.meta(), palette.mute))
            .children(ports)
            .into_any_element()
    }
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

fn mono_line(text: impl Into<SharedString>, color: gpui::Hsla) -> impl IntoElement {
    div()
        .min_w_0()
        .font_family(FONT_MONO)
        .text_size(TEXT_SMALL)
        .text_color(color)
        .child(text.into())
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

fn conditions(pod: &Pod, palette: &Palette) -> impl IntoElement {
    section(palette)
        .child(SectionLabel::new(CONDITIONS))
        .child(ConditionTable::new(&pod.conditions))
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

fn menu_items(pod: &Pod, screen: &gpui::WeakEntity<PodsScreen>) -> Vec<MenuItem> {
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

fn container_squares(pod: &Pod, palette: &Palette) -> AnyElement {
    let squares = pod.containers.iter().map(|container| {
        div()
            .size(CONTAINER_SQUARE)
            .flex_none()
            .rounded(RADIUS_SMALL)
            .bg(palette.status(container.status))
    });
    div()
        .flex()
        .gap(CONTAINER_SQUARE_GAP)
        .children(squares)
        .into_any_element()
}

fn columns() -> Vec<Column<Pod>> {
    vec![
        Column::text("Name", |p: &Pod| p.name.clone().into())
            .highlight()
            .mono()
            .flex(NAME_WEIGHT, NAME_MIN)
            .sort_by_key(|p: &Pod| p.name.clone()),
        Column::colored_text(
            "Namespace",
            |p: &Pod| p.namespace.clone().into(),
            |_, palette| palette.fg2,
        )
        .highlight()
        .fixed(NAMESPACE)
        .sort_by_key(|p: &Pod| p.namespace.clone()),
        Column::new("Containers", container_squares).fixed(CONTAINERS),
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
        Column::new("Age", |p: &Pod, palette| {
            text(flock_core::time::age(p.age), palette.mute)
        })
        .mono()
        .fixed(AGE)
        .sort_by_key(|p: &Pod| p.age),
        Column::new("Status", |p: &Pod, _| {
            StatusText::new(p.status, p.reason.clone()).into_any_element()
        })
        .fit_status_by(|p: &Pod| p.reason.clone().into())
        .auto(STATUS_MIN, STATUS_MAX)
        .sort_by_key(|p: &Pod| std::cmp::Reverse(p.status)),
        Column::colored_text(
            MESSAGE,
            |p: &Pod| p.diagnosis().map(|d| d.message).unwrap_or_default().into(),
            |_, palette| palette.mute,
        )
        .tooltip(|p: &Pod| reason_tooltip(p.diagnosis().as_ref()))
        .flex(REASON_WEIGHT, MESSAGE_MIN),
    ]
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
