use std::collections::BTreeMap;

use flock_core::settings::{
    ClusterAction, ClusterSettings, MetricsSource, format_labels, parse_labels, parse_namespace,
};
use flock_core::{
    ClusterCatalog, ClusterId, ClusterSummary, Command, Page, Preferences, Snapshot, Status,
};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FontWeight,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled, Subscription, WeakEntity, Window, div, px, relative,
};

use crate::kit::{
    ActionMenu, Button, Column, InputEvent, InspectorPane, KeyValue, MenuItem, SectionLabel,
    Select, SelectEvent, StatusText, Table, TableEvent, TextField, TextFieldState, Toggle,
};
use crate::theme::{
    ActiveTheme, FONT_MONO, LINE_PROSE, PAGE_HEADER, Palette, ROW_CATALOG, RULE, SETTINGS_FIELD,
    SETTINGS_INSET, SPACE_1, SPACE_2, SPACE_3, SPACE_4, SPACE_5, SPACE_6, SPACE_7, SPACE_8,
    TEXT_BODY, TEXT_SMALL, TEXT_SUBHEADING, TEXT_TITLE, TEXT_UI,
};

const NAME_MIN: Pixels = px(200.);
const NAME_WEIGHT: f32 = 1.6;
const DISTRO: Pixels = px(64.);
const VERSION: Pixels = px(150.);
const NODES: Pixels = px(56.);
const STATUS: Pixels = px(170.);
const INSPECTOR: Pixels = px(340.);
const INSPECTOR_ID: &str = "cluster-inspector";
const CONNECTION_LABEL: Pixels = px(96.);

const MISSING: &str = "—";
const CONNECTION: &str = "Connection";
const PREFERENCES: &str = "Preferences";
const LABELS: &str = "Labels";
const DEFAULT_NAMESPACE: &str = "Default namespace";
const METRICS_SOURCE: &str = "Metrics source";
const METRICS_SELECT: &str = "metrics-source";
const CONFIRM_DESTRUCTIVE: &str = "Confirm destructive actions";
const NOTIFY: &str = "Notify about new problems";
const NAMESPACE_PLACEHOLDER: &str = "default";
const LABELS_PLACEHOLDER: &str = "env=prod team=platform";
const LABELS_HINT: &str = "Space-separated. Press Return to save.";
pub(crate) const DISCONNECT: &str = "Disconnect";
pub(crate) const DISCONNECT_DESCRIPTION: &str =
    "Stop watching this cluster until you connect again. Nothing changes on the cluster.";

#[derive(Clone)]
struct CatalogRow {
    summary: ClusterSummary,
    kubeconfig: SharedString,
}

pub struct SettingsScreen {
    pub(crate) navigation: Option<Entity<crate::kit::HeaderNavigation>>,
    clusters: Vec<ClusterSummary>,
    catalog: ClusterCatalog,
    namespaces: BTreeMap<ClusterId, Vec<SharedString>>,
    selected: Option<ClusterId>,
    loaded: Option<ClusterId>,
    table: Entity<Table<CatalogRow>>,
    namespace: Entity<Select>,
    labels: Entity<TextFieldState>,
    metrics: Entity<Select>,
    menu: Entity<ActionMenu>,
    _subscriptions: [Subscription; 4],
}

impl EventEmitter<Command> for SettingsScreen {}

impl SettingsScreen {
    pub fn list_focus(&self, cx: &gpui::App) -> gpui::FocusHandle {
        gpui::Focusable::focus_handle(&self.table, cx)
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let table = cx.new(|cx| {
            Table::new("cluster-catalog", columns(), cx)
                .row_height(ROW_CATALOG)
                .column_gap(SPACE_7)
                .inset(SPACE_8)
                .row_key(|r: &CatalogRow| r.summary.id.to_string().into())
        });
        let namespace = cx.new(|cx| {
            Select::new("default-namespace", Vec::new(), cx)
                .mono()
                .width(SETTINGS_FIELD)
                .placeholder(NAMESPACE_PLACEHOLDER)
                .editable(window, cx)
        });
        let labels = cx.new(|cx| TextFieldState::new(window, cx).placeholder(LABELS_PLACEHOLDER));
        let sources = MetricsSource::ALL
            .into_iter()
            .map(|source| source.label().into())
            .collect();
        let metrics = cx.new(|cx| {
            Select::new(METRICS_SELECT, sources, cx)
                .mono()
                .width(SETTINGS_FIELD)
        });
        let menu = cx.new(|cx| ActionMenu::new("cluster-actions", cx));
        let subscriptions = [
            cx.subscribe(&table, |this, table, event: &TableEvent, cx| {
                let TableEvent::Selected(ix) = *event else {
                    return;
                };
                this.selected = table.read(cx).row(ix).map(|r| r.summary.id.clone());
                this.refresh_menu(cx);
                this.refresh_namespaces(cx);
                cx.notify();
            }),
            cx.subscribe(&namespace, |this, _, _: &SelectEvent, cx| this.commit(cx)),
            cx.subscribe(&labels, |this, _, event: &InputEvent, cx| {
                if commits(event) {
                    this.commit(cx);
                }
            }),
            cx.subscribe(&metrics, |this, _, _: &SelectEvent, cx| this.commit(cx)),
        ];
        Self {
            navigation: None,
            clusters: Vec::new(),
            catalog: ClusterCatalog::default(),
            namespaces: BTreeMap::new(),
            selected: None,
            loaded: None,
            table,
            namespace,
            labels,
            metrics,
            menu,
            _subscriptions: subscriptions,
        }
    }

    pub fn set_snapshot(&mut self, snapshot: &Snapshot, cx: &mut Context<Self>) {
        self.clusters.clone_from(&snapshot.clusters);
        self.catalog.clone_from(&snapshot.catalog);
        self.namespaces = snapshot
            .details
            .iter()
            .map(|(cluster, detail)| {
                let options = detail
                    .namespaces
                    .iter()
                    .map(|namespace| namespace.name.clone().into())
                    .collect();
                (cluster.clone(), options)
            })
            .collect();
        let rows: Vec<CatalogRow> = self
            .clusters
            .iter()
            .map(|summary| CatalogRow {
                kubeconfig: self
                    .catalog
                    .get(&summary.id)
                    .map(|s| s.connection.kubeconfig.clone().into())
                    .unwrap_or_default(),
                summary: summary.clone(),
            })
            .collect();
        self.table.update(cx, |table, cx| table.set_rows(rows, cx));
        let still_there = self
            .selected
            .as_ref()
            .is_some_and(|id| snapshot.cluster(id).is_some());
        if !still_there {
            self.selected = self.clusters.first().map(|c| c.id.clone());
        }
        self.sync_selection(cx);
    }

    pub fn selected(&self) -> Option<&ClusterId> {
        self.selected.as_ref()
    }

    pub fn open(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        self.selected = Some(cluster.clone());
        self.sync_selection(cx);
    }

    fn sync_selection(&mut self, cx: &mut Context<Self>) {
        let index = self
            .selected
            .as_ref()
            .and_then(|id| self.clusters.iter().position(|c| &c.id == id));
        self.table.update(cx, |table, cx| table.select(index, cx));
        self.refresh_menu(cx);
        self.refresh_namespaces(cx);
        cx.notify();
    }

    fn refresh_namespaces(&self, cx: &mut Context<Self>) {
        let options = self
            .selected
            .as_ref()
            .and_then(|id| self.namespaces.get(id))
            .cloned()
            .unwrap_or_default();
        let value = if self.loaded == self.selected {
            self.namespace
                .read(cx)
                .selected_label()
                .cloned()
                .unwrap_or_default()
        } else {
            self.settings()
                .and_then(|settings| settings.preferences.default_namespace.clone())
                .unwrap_or_default()
                .into()
        };
        self.namespace.update(cx, |select, cx| {
            select.set_options(options, cx);
            select.set_value(value, cx);
        });
    }

    fn summary(&self) -> Option<&ClusterSummary> {
        let id = self.selected.as_ref()?;
        self.clusters.iter().find(|c| &c.id == id)
    }

    fn settings(&self) -> Option<&ClusterSettings> {
        self.catalog.get(self.selected.as_ref()?)
    }

    fn refresh_menu(&self, cx: &mut Context<Self>) {
        let Some(summary) = self.summary() else {
            return;
        };
        let id = summary.id.clone();
        let offline = summary.status == Status::Offline;
        let this = cx.entity().downgrade();
        let disconnect = MenuItem::new(DISCONNECT, move |_, cx| {
            emit(&this, Command::Disconnect(id.clone()), cx);
        })
        .description(DISCONNECT_DESCRIPTION)
        .disabled(offline);
        self.menu
            .update(cx, |menu, cx| menu.set_items(vec![disconnect], cx));
    }

    fn load_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loaded == self.selected {
            return;
        }
        self.loaded.clone_from(&self.selected);
        let Some(settings) = self.settings().cloned() else {
            return;
        };
        let preferences = &settings.preferences;
        let namespace = preferences.default_namespace.clone().unwrap_or_default();
        let labels = format_labels(&preferences.labels);
        self.namespace
            .update(cx, |select, cx| select.set_value(namespace.into(), cx));
        self.labels
            .update(cx, |field, cx| field.set_value(labels, window, cx));
        let source = MetricsSource::ALL
            .iter()
            .position(|source| *source == preferences.metrics)
            .unwrap_or_default();
        let details = MetricsSource::ALL
            .into_iter()
            .map(|source| match (source, settings.prometheus.as_deref()) {
                (MetricsSource::Auto, Some(target)) => target.to_owned().into(),
                _ => SharedString::default(),
            })
            .collect();
        self.metrics.update(cx, |select, cx| {
            select.set_selected(source, cx);
            select.set_details(details, cx);
        });
    }

    fn edited(&self, cx: &Context<Self>) -> Option<(ClusterId, Preferences)> {
        let settings = self.settings()?;
        let namespace = self
            .namespace
            .read(cx)
            .selected_label()
            .cloned()
            .unwrap_or_default();
        let labels = self.labels.read(cx).value();
        let metrics = MetricsSource::ALL
            .get(self.metrics.read(cx).selected_index())
            .copied()
            .unwrap_or_default();
        let preferences = Preferences {
            namespaces: settings.preferences.namespaces.clone(),
            default_namespace: parse_namespace(&namespace),
            metrics,
            confirm_destructive: settings.preferences.confirm_destructive,
            notify: settings.preferences.notify,
            labels: parse_labels(&labels),
        };
        Some((settings.cluster.clone(), preferences))
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        if self.loaded != self.selected {
            return;
        }
        let Some((cluster, preferences)) = self.edited(cx) else {
            self.loaded = None;
            cx.notify();
            return;
        };
        let unchanged = self
            .catalog
            .get(&cluster)
            .is_some_and(|s| s.preferences == preferences);
        if !unchanged {
            cx.emit(Command::SetPreferences(cluster, preferences));
        }
    }

    fn header(&self, palette: &Palette) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(SPACE_4)
            .h(PAGE_HEADER)
            .px(SPACE_8)
            .border_b(RULE)
            .border_color(palette.line)
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .children(self.navigation.clone())
            .child(
                div()
                    .text_size(TEXT_TITLE)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(Page::ClusterSettings.label()),
            )
            .child(
                div()
                    .pl(SPACE_2)
                    .font_family(FONT_MONO)
                    .text_size(TEXT_SMALL)
                    .text_color(palette.mute)
                    .child(self.clusters.len().to_string()),
            )
    }

    fn inspector(&self, palette: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let summary = self.summary()?.clone();
        let settings = self.settings()?.clone();
        let action = ClusterAction::for_cluster(
            summary.status,
            settings.auth_expired,
            settings.context_change.is_some(),
        );
        let id = summary.id.clone();
        let cta = Button::new("cluster-cta", action.label())
            .primary()
            .disabled(summary.status == Status::Progress)
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(action_command(action, &id));
            }));
        let top = div()
            .flex()
            .flex_col()
            .gap(SPACE_6)
            .px(SPACE_8)
            .pt(SPACE_8)
            .pb(SPACE_7)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_1)
                    .child(
                        div()
                            .text_size(TEXT_SUBHEADING)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(summary.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(TEXT_BODY)
                            .child(StatusText::new(summary.status, summary.state.clone())),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_4)
                    .child(cta)
                    .child(self.menu.clone()),
            );
        let sentence = settings
            .context_sentence()
            .or_else(|| settings.auth_sentence());
        let auth = sentence.map(|sentence| {
            div()
                .px(SPACE_8)
                .py(SPACE_5)
                .bg(palette.errbg)
                .text_size(TEXT_BODY)
                .line_height(relative(LINE_PROSE))
                .child(sentence)
        });
        let connection = &settings.connection;
        let details = KeyValue::new()
            .label_width(CONNECTION_LABEL)
            .text("Context", connection.context.clone())
            .text("Server", connection.server.clone())
            .text("Auth", connection.auth.label())
            .text("Kubeconfig", connection.kubeconfig.clone());
        let preferences = self.preferences(&settings, palette, cx);
        let labels = section(LABELS, palette)
            .child(
                div()
                    .max_w(SETTINGS_FIELD)
                    .child(TextField::new(&self.labels)),
            )
            .child(
                div()
                    .text_size(TEXT_SMALL)
                    .text_color(palette.mute)
                    .child(LABELS_HINT),
            );
        let inspector = div()
            .id(INSPECTOR_ID)
            .flex()
            .flex_col()
            .size_full()
            .overflow_y_scroll()
            .bg(palette.panel)
            .border_l(RULE)
            .border_color(palette.line)
            .text_size(TEXT_BODY)
            .child(top)
            .children(auth)
            .child(section(CONNECTION, palette).child(details))
            .child(preferences)
            .child(labels);
        let pane = InspectorPane::new(INSPECTOR_ID, inspector).width(INSPECTOR);
        Some(pane.into_any_element())
    }
}

impl SettingsScreen {
    fn preferences(
        &self,
        settings: &ClusterSettings,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let stored = &settings.preferences;
        let confirm = preference_toggle(
            "cluster-confirm",
            CONFIRM_DESTRUCTIVE,
            settings,
            stored.confirm_destructive,
            |preferences, on| preferences.confirm_destructive = on,
            cx,
        );
        let notify = preference_toggle(
            "cluster-notify",
            NOTIFY,
            settings,
            stored.notify,
            |preferences, on| preferences.notify = on,
            cx,
        );
        section(PREFERENCES, palette)
            .gap(SPACE_6)
            .child(labelled(DEFAULT_NAMESPACE, self.namespace.clone()))
            .child(labelled(METRICS_SOURCE, self.metrics.clone()))
            .child(confirm)
            .child(notify)
    }
}

impl Render for SettingsScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.load_fields(window, cx);
        let palette = *cx.palette();
        let inspector = self.inspector(&palette, cx);
        let list = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                div()
                    .flex_none()
                    .px(SPACE_8)
                    .py(SPACE_6)
                    .border_b(RULE)
                    .border_color(palette.line)
                    .text_size(TEXT_UI)
                    .text_color(palette.fg2)
                    .child(self.catalog.sentence()),
            )
            .child(div().flex_1().min_h_0().child(Table::pane(&self.table, cx)));
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(self.header(&palette))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(list)
                    .children(inspector),
            )
    }
}

fn preference_toggle(
    id: &'static str,
    label: &'static str,
    settings: &ClusterSettings,
    on: bool,
    set: impl Fn(&mut Preferences, bool) + 'static,
    cx: &mut Context<SettingsScreen>,
) -> Toggle {
    let cluster = settings.cluster.clone();
    let stored = settings.preferences.clone();
    Toggle::new(id, on)
        .label(label)
        .on_toggle(cx.listener(move |_, on: &bool, _, cx| {
            let mut preferences = stored.clone();
            set(&mut preferences, *on);
            cx.emit(Command::SetPreferences(cluster.clone(), preferences));
        }))
}

fn emit(this: &WeakEntity<SettingsScreen>, command: Command, cx: &mut gpui::App) {
    let sent = this.update(cx, |_, cx| cx.emit(command));
    if sent.is_err() {
        tracing::debug!("settings screen closed before the command was sent");
    }
}

fn commits(event: &InputEvent) -> bool {
    match event {
        InputEvent::PressEnter { .. } => true,
        InputEvent::Blur => true,
        InputEvent::Change => false,
        InputEvent::Focus => false,
    }
}

fn action_command(action: ClusterAction, cluster: &ClusterId) -> Command {
    match action {
        ClusterAction::Open => Command::SelectCluster(Some(cluster.clone())),
        ClusterAction::Reauthenticate => Command::Reauthenticate(cluster.clone()),
        ClusterAction::Connect => Command::Connect(cluster.clone()),
        ClusterAction::ConfirmContext => Command::ConfirmContext(cluster.clone()),
    }
}

fn section(label: &str, palette: &Palette) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(SPACE_4)
        .px(SETTINGS_INSET)
        .py(SPACE_7)
        .border_t(RULE)
        .border_color(palette.line)
        .child(SectionLabel::new(SharedString::from(label.to_owned())))
}

fn labelled(label: &'static str, control: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(SPACE_3)
        .child(label)
        .child(div().flex().min_w_0().max_w(SETTINGS_FIELD).child(control))
}

fn columns() -> Vec<Column<CatalogRow>> {
    vec![
        Column::new("Name", |row: &CatalogRow, palette| {
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .font_weight(FontWeight::MEDIUM)
                        .child(row.summary.name.clone()),
                )
                .child(
                    div()
                        .truncate()
                        .font_family(FONT_MONO)
                        .text_size(TEXT_SMALL)
                        .text_color(palette.mute)
                        .child(row.kubeconfig.clone()),
                )
                .into_any_element()
        })
        .flex(NAME_WEIGHT, NAME_MIN)
        .sort_by_key(|r: &CatalogRow| r.summary.name.clone()),
        Column::colored_text(
            "Distro",
            |r: &CatalogRow| r.summary.distro.clone().into(),
            |_, palette| palette.fg2,
        )
        .fixed(DISTRO),
        Column::colored_text(
            "Version",
            |r: &CatalogRow| {
                r.summary
                    .version
                    .clone()
                    .map_or_else(|| MISSING.into(), Into::into)
            },
            |_, palette| palette.fg2,
        )
        .mono()
        .fixed(VERSION),
        Column::text("Nodes", |r: &CatalogRow| {
            r.summary
                .nodes
                .map_or_else(|| MISSING.into(), |n| n.to_string().into())
        })
        .mono()
        .end()
        .fixed(NODES),
        Column::new("Status", |r: &CatalogRow, _| {
            StatusText::new(r.summary.status, r.summary.state.clone()).into_any_element()
        })
        .fixed(STATUS),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: &str) -> ClusterSummary {
        ClusterSummary {
            id: ClusterId::new(id),
            name: id.to_owned(),
            status: Status::Ok,
            state: String::new(),
            distro: String::new(),
            region: String::new(),
            version: None,
            nodes: None,
            problems: flock_core::Counts::default(),
            nav: BTreeMap::new(),
            metrics: None,
        }
    }

    fn detail(name: &str) -> flock_core::ClusterDetail {
        let namespace = flock_core::cluster::Namespace {
            labels: BTreeMap::new(),
            name: name.to_owned(),
            status: Status::Ok,
            phase: String::new(),
            pods: 0,
            problems: flock_core::Counts::default(),
            cpu: None,
            memory: None,
            cpu_requests: 0.,
            memory_requests: 0.,
            restarts: 0,
            age: std::time::Duration::ZERO,
        };
        flock_core::ClusterDetail {
            namespaces: vec![namespace],
            ..Default::default()
        }
    }

    #[gpui::test]
    fn namespace_options_follow_live_snapshots_and_the_selected_cluster(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            crate::init(Some(crate::theme::Appearance::Dark), cx)
                .unwrap_or_else(|error| panic!("UI init: {error}"));
        });
        let window = cx.add_window(SettingsScreen::new);
        let first = summary("first");
        let second = summary("second");
        let mut snapshot = Snapshot {
            clusters: vec![first.clone(), second.clone()],
            details: BTreeMap::from([
                (first.id.clone(), detail("old")),
                (second.id.clone(), detail("other")),
            ]),
            ..Default::default()
        };
        window
            .update(cx, |screen, _, cx| {
                screen.set_snapshot(&snapshot, cx);
                screen.namespace.update(cx, |select, cx| {
                    select.set_selected(0, cx);
                    assert_eq!(
                        select.selected_label().map(SharedString::as_ref),
                        Some("old")
                    );
                    select.set_value("future".into(), cx);
                });
                screen.loaded = Some(first.id.clone());
                snapshot.details.insert(first.id.clone(), detail("new"));
                screen.set_snapshot(&snapshot, cx);
                screen.namespace.update(cx, |select, cx| {
                    assert_eq!(
                        select.selected_label().map(SharedString::as_ref),
                        Some("future")
                    );
                    select.set_selected(0, cx);
                    assert_eq!(
                        select.selected_label().map(SharedString::as_ref),
                        Some("new")
                    );
                });
                screen.open(&second.id, cx);
                screen.namespace.update(cx, |select, cx| {
                    select.set_selected(0, cx);
                    assert_eq!(
                        select.selected_label().map(SharedString::as_ref),
                        Some("other")
                    );
                    select.set_selected(usize::MAX, cx);
                    assert_eq!(select.selected_label().map(SharedString::as_ref), Some(""));
                });
            })
            .unwrap_or_else(|error| panic!("test window: {error}"));
    }

    #[test]
    fn only_return_and_blur_save_a_field() {
        assert!(commits(&InputEvent::PressEnter {
            secondary: false,
            shift: false
        }));
        assert!(commits(&InputEvent::Blur));
        assert!(!commits(&InputEvent::Change));
    }

    #[test]
    fn the_primary_action_becomes_a_command() {
        let id = ClusterId::new("qa-gke");
        assert_eq!(
            action_command(ClusterAction::Reauthenticate, &id),
            Command::Reauthenticate(id.clone())
        );
        assert_eq!(
            action_command(ClusterAction::Open, &id),
            Command::SelectCluster(Some(id.clone()))
        );
        assert_eq!(
            action_command(ClusterAction::Connect, &id),
            Command::Connect(id)
        );
    }
}
