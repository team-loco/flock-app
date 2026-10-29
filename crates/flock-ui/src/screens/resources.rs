use std::borrow::Cow;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use flock_core::across::{
    CLUSTER_FACET, EXTRA_FACETS, IMAGE_FACET, STATUS_FACET, Sibling, cell, image, image_name,
    keeps, siblings, split_query, status_value,
};
use flock_core::emptiness::Noun;
use flock_core::filter::{StatusFilter, facets};
use flock_core::overview::plural;
use flock_core::pods::PodEvent;
use flock_core::query::NAMESPACE_FACET;
use flock_core::resources::{Resource, filter_counts, filter_resources, namespace_counts};
use flock_core::settings::NamespaceSelection;
use flock_core::time::age;
use flock_core::{
    ClusterDetail, ClusterId, ClusterSummary, Command, Counts, Preferences, ResourceKind,
    ResourceRef, Seen, Snapshot, Status,
};
use gpui::{
    AnyElement, AppContext, Context, Entity, EventEmitter, InteractiveElement, IntoElement,
    ParentElement, Pixels, Render, Styled, Subscription, WeakEntity, Window, div,
    prelude::FluentBuilder, px,
};

use super::ScreenEvent;
use crate::kit::{
    BulkAction, BulkBar, Button, CONDITIONS, CodeView, Column, ConditionTable, ConfirmDialog,
    EmptyState, FilterBar, FilterEvent, Grouping, IconButton, Inspector, MenuItem, PanelSection,
    Presence, Select, SelectEvent, ShowAll, Skeleton, SnapshotRow, SnapshotRows, StatusGlyph,
    Table, TableEvent, TextFieldState, Toast, chip_label, confirm, reason_cell, reason_tooltip,
    slide_in, slide_out, toast,
};
use crate::screens::edit::{EDIT_KEYS, EditYaml, YAML_CONTEXT, edit_command};
use crate::screens::related::{OnPick, Pick, RELATED, inspector_section};
use crate::screens::workload::{
    RESTART, RESTART_BODY, SCALE, SCALE_BODY, SCALE_CONFIRM, SCALE_INVALID, parse_replicas,
};
use crate::screens::{
    ALL_NAMESPACES, EDIT_YAML, ListShape, chosen_namespace, edit_item, empty_list, new_tab_item,
    set_namespace_options,
};
use crate::theme::{
    ActiveTheme, FONT_MONO, INSPECTOR_RESOURCE, INSPECTOR_WORKLOAD, Palette, REASON_MIN,
    REASON_WEIGHT, SPACE_3, SPACE_4, SPACE_8, TEXT_SMALL,
};

const NAME_WEIGHT: f32 = 1.4;
const NAME_MIN: Pixels = px(200.);
const NAMESPACE_WIDTH: Pixels = px(130.);
const AGE_WIDTH: Pixels = px(44.);
const SELECT_WIDTH: Pixels = px(180.);
const INSPECTOR_ID: &str = "resource-inspector";
const REASON: &str = "Reason";
const YAML: &str = "YAML";
const STATUS_COLUMNS: [&str; 2] = ["Ready", "Status"];
const ACROSS_PREFIX: &str = "flock-resources";
const ACROSS_HIDDEN: [&str; 1] = ["Available"];
const ACROSS_NAME_MIN: Pixels = px(150.);
const ACROSS_NAME_WEIGHT: f32 = 1.6;
const CLUSTER_WIDTH: Pixels = px(104.);
const OTHER_CLUSTERS: &str = "Other clusters";
const ONLY_HERE: &str = "Not on the other selected clusters";
const OPEN: &str = "Open";
const BULK_ID: &str = "resource-bulk";
const READY_COLUMN: &str = "Ready";
const DESIRED_SEPARATOR: char = '/';
const BULK_SEPARATOR: &str = ", ";
const OTHER_TAG: Pixels = px(80.);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Cell {
    Fixed(Pixels),
    End(Pixels),
    Flex(f32, Pixels),
}

const IMAGE: Cell = Cell::Flex(3., px(300.));
const DEPLOYMENT_CELLS: [Cell; 4] = [
    Cell::Fixed(px(56.)),
    Cell::Fixed(px(76.)),
    Cell::Fixed(px(72.)),
    IMAGE,
];
const STATEFULSET_CELLS: [Cell; 3] = [Cell::Fixed(px(56.)), Cell::Fixed(px(140.)), IMAGE];
const DAEMONSET_CELLS: [Cell; 4] = [
    Cell::End(px(60.)),
    Cell::End(px(52.)),
    Cell::Fixed(px(230.)),
    IMAGE,
];
const JOB_CELLS: [Cell; 3] = [
    Cell::Fixed(px(84.)),
    Cell::Fixed(px(70.)),
    Cell::Fixed(px(90.)),
];
const CRONJOB_CELLS: [Cell; 4] = [
    Cell::Fixed(px(110.)),
    Cell::Fixed(px(64.)),
    Cell::End(px(52.)),
    Cell::Fixed(px(96.)),
];
const SERVICE_CELLS: [Cell; 3] = [
    Cell::Fixed(px(96.)),
    Cell::Fixed(px(110.)),
    Cell::Fixed(px(90.)),
];
const INGRESS_CELLS: [Cell; 3] = [
    Cell::Fixed(px(60.)),
    Cell::Fixed(px(200.)),
    Cell::Flex(1., px(200.)),
];
const CONFIGMAP_CELLS: [Cell; 1] = [Cell::End(px(48.))];
const SECRET_CELLS: [Cell; 2] = [Cell::Fixed(px(220.)), Cell::End(px(48.))];
const PVC_CELLS: [Cell; 4] = [
    Cell::Fixed(px(64.)),
    Cell::Fixed(px(70.)),
    Cell::Fixed(px(96.)),
    Cell::Flex(1., px(200.)),
];

pub fn inspector_width(kind: ResourceKind) -> Pixels {
    if kind.is_workload() {
        INSPECTOR_WORKLOAD
    } else {
        INSPECTOR_RESOURCE
    }
}

fn cell_widths(kind: ResourceKind) -> &'static [Cell] {
    match kind {
        ResourceKind::Deployment => &DEPLOYMENT_CELLS,
        ResourceKind::StatefulSet => &STATEFULSET_CELLS,
        ResourceKind::DaemonSet => &DAEMONSET_CELLS,
        ResourceKind::Job => &JOB_CELLS,
        ResourceKind::CronJob => &CRONJOB_CELLS,
        ResourceKind::Service => &SERVICE_CELLS,
        ResourceKind::Ingress => &INGRESS_CELLS,
        ResourceKind::ConfigMap => &CONFIGMAP_CELLS,
        ResourceKind::Secret => &SECRET_CELLS,
        ResourceKind::PersistentVolumeClaim => &PVC_CELLS,
    }
}

type ResourceRow = SnapshotRow<Resource>;
type Key = (Option<ClusterId>, String, String);

fn key_of(row: &ResourceRow) -> Key {
    (
        row.cluster().cloned(),
        row.namespace.clone(),
        row.name.clone(),
    )
}

fn has_key(row: &ResourceRow, key: &Key) -> bool {
    row.cluster() == key.0.as_ref() && row.namespace == key.1 && row.name == key.2
}

pub struct ResourcesScreen {
    across: bool,
    clusters: usize,
    cluster: Option<ClusterId>,
    cluster_name: String,
    kind: ResourceKind,
    rows: Rc<SnapshotRows<Resource>>,
    shown: usize,
    confirm_destructive: bool,
    namespace: Option<String>,
    awaiting: Option<String>,
    preferences: Preferences,
    selected: Option<Key>,
    revealed: Option<(ResourceKind, String, String)>,
    tables: HashMap<ResourceKind, Entity<Table<ResourceRow>>>,
    namespaces: Entity<Select>,
    filter: Entity<FilterBar>,
    subscriptions: Vec<Subscription>,
    presence: Presence<ResourceRow>,
}

fn presence(screen: &mut ResourcesScreen) -> &mut Presence<ResourceRow> {
    &mut screen.presence
}

impl EventEmitter<ScreenEvent> for ResourcesScreen {}

impl ResourcesScreen {
    pub fn inspecting(&self) -> bool {
        self.selected.is_some()
    }

    pub fn close_inspector(&mut self, cx: &mut Context<Self>) -> bool {
        if self.selected.is_none() {
            return false;
        }
        self.close(cx);
        true
    }

    pub fn list_focus(&mut self, cx: &mut Context<Self>) -> gpui::FocusHandle {
        gpui::Focusable::focus_handle(&self.table(cx), cx)
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::build(false, window, cx)
    }

    pub fn across(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::build(true, window, cx)
    }

    fn build(across: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let namespaces = cx.new(|cx| {
            Select::new("resource-namespace", vec![ALL_NAMESPACES.into()], cx)
                .mono()
                .width(SELECT_WIDTH)
        });
        let filter = cx.new(|cx| {
            let bar = FilterBar::new("resource-filter", window, cx);
            if across {
                bar
            } else {
                bar.scope(namespaces.clone())
            }
        });
        let subscriptions = vec![
            cx.subscribe(&namespaces, |this, select, _: &SelectEvent, cx| {
                let choice = chosen_namespace(select.read(cx));
                this.choose_namespace(choice, cx);
            }),
            cx.subscribe(&filter, |this, _, FilterEvent::Changed, cx| {
                this.refresh_rows(cx);
            }),
        ];
        Self {
            across,
            clusters: 0,
            cluster: None,
            cluster_name: String::new(),
            kind: ResourceKind::Deployment,
            rows: SnapshotRows::empty(),
            shown: 0,
            confirm_destructive: true,
            namespace: None,
            awaiting: None,
            preferences: Preferences::default(),
            selected: None,
            revealed: None,
            tables: HashMap::new(),
            namespaces,
            filter,
            subscriptions,
            presence: Presence::default(),
        }
    }

    pub fn set(
        &mut self,
        cluster: &ClusterSummary,
        kind: ResourceKind,
        snapshot: &Arc<Snapshot>,
        confirm_destructive: bool,
        preferences: &Preferences,
        cx: &mut Context<Self>,
    ) {
        let namespace = preferences
            .namespace_selection(kind.name())
            .namespace()
            .map(str::to_owned);
        let stale = self
            .awaiting
            .as_ref()
            .is_some_and(|chosen| namespace.as_ref() != Some(chosen));
        if !stale {
            self.awaiting = None;
            self.preferences = preferences.clone();
        }
        let namespace = if stale {
            self.namespace.clone()
        } else {
            namespace
        };
        let rescoped = self.namespace != namespace;
        self.namespace = namespace;
        if self.confirm_destructive != confirm_destructive {
            self.confirm_destructive = confirm_destructive;
            self.refresh(cx);
        }
        let fresh = !self.rows.from(snapshot);
        if fresh {
            cx.notify();
        }
        let switched = self.kind != kind || self.cluster.as_ref() != Some(&cluster.id);
        if !switched && !rescoped && !fresh {
            return;
        }
        if switched {
            self.selected = None;
        }
        self.cluster = Some(cluster.id.clone());
        self.cluster_name.clone_from(&cluster.name);
        self.kind = kind;
        if let Some((_, namespace, name)) = self.revealed.take_if(|(k, _, _)| *k == kind) {
            self.show_namespace_of(&namespace, cx);
            self.selected = Some((None, namespace, name));
        }
        let id = cluster.id.clone();
        self.rows = SnapshotRows::new(snapshot, move |snapshot| {
            snapshot
                .detail(&id)
                .map_or(&[][..], |detail| detail.resources(kind))
        });
        let rows = self.rows.items();
        let namespaces = self
            .detail()
            .map_or(&[][..], |detail| detail.namespaces.as_slice());
        let counts = namespace_counts(
            namespaces.iter().map(|ns| ns.name.as_str()),
            rows.iter().map(|row| row.namespace.as_str()),
            self.namespace.as_deref(),
        );
        let chosen = self.namespace.as_deref();
        set_namespace_options(&self.namespaces, &counts, rows.len(), chosen, cx);
        self.refresh(cx);
    }

    pub fn set_across(
        &mut self,
        kind: ResourceKind,
        snapshot: &Arc<Snapshot>,
        cx: &mut Context<Self>,
    ) {
        if self.kind == kind && self.rows.from(snapshot) {
            return;
        }
        if self.kind != kind {
            self.selected = None;
        }
        self.kind = kind;
        self.clusters = snapshot.clusters.len();
        self.cluster_name = plural(self.clusters, "cluster", "clusters");
        self.rows = SnapshotRows::across(snapshot, move |snapshot, id| {
            snapshot
                .detail(id)
                .map_or(&[][..], |detail| detail.resources(kind))
        });
        cx.notify();
        self.refresh(cx);
    }

    pub fn reveal(
        &mut self,
        kind: ResourceKind,
        namespace: String,
        name: String,
        cx: &mut Context<Self>,
    ) {
        let key = (None, namespace.clone(), name.clone());
        if kind == self.kind && self.selected.as_ref() == Some(&key) {
            return;
        }
        if kind == self.kind && self.cluster.is_some() {
            self.show_namespace_of(&namespace, cx);
            self.selected = Some(key);
            self.refresh(cx);
        } else {
            self.revealed = Some((kind, namespace, name));
        }
    }

    fn show_namespace_of(&mut self, namespace: &str, cx: &mut Context<Self>) {
        if self.namespace.as_ref().is_some_and(|ns| ns != namespace) {
            self.choose_namespace(Some(namespace.to_owned()), cx);
            self.awaiting = Some(namespace.to_owned());
        }
    }

    fn choose_namespace(&mut self, choice: Option<String>, cx: &mut Context<Self>) {
        let Some(cluster) = self.cluster.clone() else {
            return;
        };
        self.awaiting = None;
        let selection = NamespaceSelection::of(choice.as_deref());
        self.preferences
            .select_namespace(self.kind.name(), selection);
        self.namespace = choice;
        self.refresh(cx);
        let command = Command::SetPreferences(cluster, self.preferences.clone());
        cx.emit(ScreenEvent::Command(command));
    }

    fn show_all_namespaces(&mut self, cx: &mut Context<Self>) {
        self.namespaces
            .update(cx, |select, cx| select.set_selected(0, cx));
        self.choose_namespace(None, cx);
    }

    fn empty_state(&self, cx: &mut Context<Self>) -> Option<EmptyState> {
        let namespace = self.namespace.as_deref().map(|namespace| {
            let inside = self
                .rows
                .items()
                .iter()
                .filter(|r| r.namespace == namespace);
            (namespace, inside.count())
        });
        let shape = ListShape {
            shown: self.shown,
            total: self.rows.items().len(),
            namespace,
            noun: Noun::of(self.kind.page()),
            cluster: &self.cluster_name,
        };
        let screen = cx.weak_entity();
        let show_all: ShowAll = Rc::new(move |_, cx| {
            let _ = screen.update(cx, ResourcesScreen::show_all_namespaces);
        });
        empty_list(&shape, &self.filter, Some(show_all), cx)
    }

    pub fn filter_bar(&self) -> &Entity<FilterBar> {
        &self.filter
    }

    pub fn groups(&self, cx: &gpui::App) -> Vec<flock_core::grouping::Group> {
        self.tables
            .get(&self.kind)
            .map(|table| table.read(cx).groups().to_vec())
            .unwrap_or_default()
    }

    pub fn header_note(&self) -> String {
        note(self.rows.items(), self.namespace.as_deref())
    }

    fn table(&mut self, cx: &mut Context<Self>) -> Entity<Table<ResourceRow>> {
        let kind = self.kind;
        if let Some(table) = self.tables.get(&kind) {
            return table.clone();
        }
        let across = self.across;
        let table = cx.new(|cx| {
            if across {
                let id = format!("{ACROSS_PREFIX}-{}", kind.name());
                return Table::new(id, across_columns(kind), cx)
                    .row_key(|r: &ResourceRow| {
                        format!("{}/{}/{}", r.cluster_name(), r.namespace, r.name).into()
                    })
                    .row_status(|r: &ResourceRow| (r.status != Status::Ok).then_some(r.status))
                    .checkboxes()
                    .default_group(STATUS_FACET)
                    .grouped(across_grouping(kind), cx);
            }
            Table::new(format!("resources-{}", kind.name()), columns(kind), cx)
                .row_key(|r: &ResourceRow| format!("{}/{}", r.namespace, r.name).into())
                .row_status(|r: &ResourceRow| (r.status != Status::Ok).then_some(r.status))
                .grouped(Grouping::facets(|r: &ResourceRow| r.status), cx)
        });
        let subscription = cx.subscribe(&table, |this, table, event: &TableEvent, cx| {
            let ix = match *event {
                TableEvent::Selected(ix) => ix,
                TableEvent::Opened(ix) => ix,
                TableEvent::Marked => {
                    cx.notify();
                    return;
                }
            };
            let key = table.read(cx).row(ix).map(key_of);
            this.select(key, cx);
            if let TableEvent::Opened(_) = event {
                this.open(cx);
            }
        });
        self.subscriptions.push(subscription);
        self.tables.insert(kind, table.clone());
        table
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let namespace = self.namespace.as_ref();
        let resources = self.rows.items();
        let facets = if self.across {
            let kind = self.kind;
            let rows = &self.rows;
            flock_core::across::facets(
                resources
                    .iter()
                    .enumerate()
                    .map(|(ix, row)| (row, extras(kind, row, rows.cluster_name(ix)))),
            )
        } else {
            facets(
                resources
                    .iter()
                    .filter(|row| namespace.is_none_or(|namespace| &row.namespace == namespace)),
            )
        };
        self.filter.update(cx, |bar, cx| bar.set_facets(facets, cx));
        self.refresh_rows(cx);
    }

    fn refresh_rows(&mut self, cx: &mut Context<Self>) {
        let resources = self.rows.items();
        let (query, chip) = {
            let bar = self.filter.read(cx);
            (bar.query().clone(), bar.chip())
        };
        let (kept, extra) = if self.across {
            split_query(&query, &EXTRA_FACETS)
        } else {
            (query.clone(), flock_core::filter::TextFilter::default())
        };
        let matched = self.rows.matching(&kept);
        let base = filter_resources(resources, &matched, self.namespace.as_deref());
        let kind = self.kind;
        let rows = &self.rows;
        let base: Vec<usize> = base
            .into_iter()
            .filter(|&ix| {
                resources
                    .get(ix)
                    .is_some_and(|row| keeps(&extra, &extras(kind, row, rows.cluster_name(ix))))
            })
            .collect();
        let chips = filter_counts(base.iter().filter_map(|&ix| resources.get(ix)))
            .into_iter()
            .map(|(filter, n)| chip_label(filter.label(), n))
            .collect();
        let chip = StatusFilter::at(chip);
        let kept = base
            .into_iter()
            .filter(|&ix| resources.get(ix).is_some_and(|r| chip.keeps(r.status)));
        let rows = self.rows.rows(kept);
        let shown = rows.len();
        self.shown = shown;
        let total = resources.len();
        self.filter.update(cx, |bar, cx| {
            bar.set_chips(chips, cx);
            bar.set_matches(shown, total, cx);
        });
        if self
            .selected
            .as_ref()
            .is_some_and(|key| !rows.iter().any(|r| has_key(r, key)))
        {
            self.selected = None;
        }
        let position = self
            .selected
            .as_ref()
            .and_then(|key| rows.iter().position(|r| has_key(r, key)));
        let table = self.table(cx);
        let scope = self
            .cluster
            .as_ref()
            .filter(|_| !self.across)
            .map(|c| c.as_str().to_owned());
        let actions = self.row_actions(cx);
        let words = query.words.clone();
        let group = table.read(cx).group_menu().clone();
        self.filter.update(cx, |bar, cx| bar.set_group(group, cx));
        table.update(cx, |table, cx| {
            if let Some(scope) = scope {
                table.set_scope(scope, cx);
            }
            table.set_highlight(words, cx);
            table.set_row_actions(actions);
            table.set_rows(rows, cx);
            if position.is_some() {
                table.select(position, cx);
            }
        });
        cx.notify();
    }

    fn select(&mut self, key: Option<Key>, cx: &mut Context<Self>) {
        self.selected = key;
        let target = self.selected_row().and_then(|row| self.target(&row));
        let command = target.map_or(Command::CloseInspector, Command::InspectResource);
        cx.emit(ScreenEvent::Command(command));
        cx.notify();
    }

    fn open(&mut self, cx: &mut Context<Self>) {
        let target = self.selected_row().and_then(|row| self.target(&row));
        if let Some(target) = target {
            cx.emit(ScreenEvent::Command(Command::OpenResource(target)));
        }
    }

    fn edit(&mut self, cx: &mut Context<Self>) {
        let target = self.selected_row().and_then(|row| self.target(&row));
        if let Some(target) = target {
            cx.emit(ScreenEvent::Command(edit_command(target)));
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        let table = self.table(cx);
        table.update(cx, |table, cx| table.select(None, cx));
        self.select(None, cx);
    }

    fn detail(&self) -> Option<&ClusterDetail> {
        let cluster = self.cluster.as_ref()?;
        self.rows.snapshot().detail(cluster)
    }

    fn detail_of(&self, row: &ResourceRow) -> Option<&ClusterDetail> {
        let cluster = row.cluster().or(self.cluster.as_ref())?;
        self.rows.snapshot().detail(cluster)
    }

    fn selected_row(&self) -> Option<ResourceRow> {
        let (cluster, ns, name) = self.selected.as_ref()?;
        let rows = &self.rows;
        let ix = rows.items().iter().enumerate().position(|(ix, r)| {
            rows.cluster(ix) == cluster.as_ref() && &r.namespace == ns && &r.name == name
        })?;
        rows.rows([ix]).pop()
    }

    fn target(&self, row: &ResourceRow) -> Option<ResourceRef> {
        Some(ResourceRef {
            cluster: row.cluster().or(self.cluster.as_ref()).cloned()?,
            kind: self.kind.name().into(),
            namespace: Some(row.namespace.clone()),
            name: row.name.clone(),
        })
    }

    fn row_actions(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&ResourceRow, &[&ResourceRow]) -> Vec<MenuItem> + 'static {
        let cluster = self.cluster.clone();
        let kind = self.kind;
        let confirm_destructive = self.confirm_destructive;
        let screen = cx.weak_entity();
        move |row: &ResourceRow, _: &[&ResourceRow]| {
            let Some(cluster) = row.cluster().or(cluster.as_ref()).cloned() else {
                return Vec::new();
            };
            let target = ResourceRef {
                cluster,
                kind: kind.name().into(),
                namespace: Some(row.namespace.clone()),
                name: row.name.clone(),
            };
            let open_screen = screen.clone();
            let open = new_tab_item(Command::OpenResource(target.clone()), move |command, cx| {
                let _ = open_screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)));
            });
            let edit_screen = screen.clone();
            let edit = edit_item(target.clone(), move |command, cx| {
                let _ = edit_screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)));
            });
            let seen = Seen::new(target, row.uid.clone());
            let mut items = vec![open, edit];
            items.extend(delete_items(
                kind,
                seen,
                confirm_destructive,
                screen.clone(),
            ));
            items
        }
    }

    fn inspector(&self, row: &ResourceRow, cx: &mut Context<Self>) -> AnyElement {
        let close = IconButton::glyph("close-resource", "×", "Close")
            .on_click(cx.listener(|this, _, _, cx| this.close(cx)));
        let edit = Button::new("edit-resource", EDIT_YAML)
            .small()
            .tooltip(EDIT_KEYS)
            .on_click(cx.listener(|this, _, _, cx| this.edit(cx)));
        let open = self.across.then(|| {
            Button::new("open-resource", OPEN)
                .small()
                .primary()
                .on_click(cx.listener(|this, _, _, cx| this.open(cx)))
        });
        let actions = div()
            .flex()
            .gap(SPACE_3)
            .children(open)
            .child(edit)
            .child(close);
        let subtitle = if self.across {
            format!(
                "{} · {} · {}",
                self.kind.name(),
                row.namespace,
                row.cluster_name()
            )
        } else {
            format!("{} · {}", self.kind.name(), row.namespace)
        };
        let yaml: AnyElement = if row.yaml.is_empty() {
            Skeleton::rows().into_any_element()
        } else {
            CodeView::new("resource-yaml", row.yaml.clone())
                .yaml()
                .into_any_element()
        };
        let events: Vec<PodEvent> = row
            .events(
                self.kind,
                self.detail_of(row).map_or(&[][..], |d| d.events.as_slice()),
            )
            .into_iter()
            .map(PodEvent::from)
            .collect();
        let screen = cx.weak_entity();
        let pick: OnPick = Rc::new(move |pick, _, cx| {
            if let Pick::Object(target) = pick {
                let command = Command::OpenResource(target);
                let _ = screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)));
            }
        });
        let related = self.target(row).and_then(|target| {
            let related = self
                .detail_of(row)
                .map_or(&[][..], |d| d.related.as_slice());
            related.iter().find(|related| related.target == target)
        });
        let related = inspector_section(related, &pick, cx);
        let mut inspector = Inspector::new(INSPECTOR_ID, row.name.clone())
            .mono()
            .width(inspector_width(self.kind))
            .status(row.status, subtitle)
            .issues(row.issue(self.kind.name()))
            .actions(actions);
        if !row.conditions.is_empty() {
            inspector = inspector.section(Some(CONDITIONS), ConditionTable::new(&row.conditions));
        }
        if let Some(cluster) = row.cluster().filter(|_| self.across) {
            let found = siblings(
                self.rows.snapshot(),
                self.kind,
                cluster,
                &row.namespace,
                &row.name,
            );
            let title = format!("{OTHER_CLUSTERS} {}", found.len());
            let palette = *cx.palette();
            inspector = inspector.section(Some(&title), other_clusters(&found, &palette));
        }
        inspector
            .panel(PanelSection::events(INSPECTOR_ID, events))
            .section(Some(RELATED), related)
            .section(Some(YAML), yaml)
            .into_any_element()
    }
}

impl ResourcesScreen {
    fn footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = *cx.palette();
        let noun = Noun::of(self.kind.page());
        let total = self.rows.items().len();
        let text = footer_text(self.shown, total, &noun.plural, self.clusters);
        div()
            .debug_selector(|| "resource-footer".to_owned())
            .flex_none()
            .px(SPACE_8)
            .py(SPACE_4)
            .font_family(FONT_MONO)
            .text_size(TEXT_SMALL)
            .text_color(palette.mute)
            .child(text)
            .into_any_element()
    }

    fn bulk_bar(
        &self,
        table: &Entity<Table<ResourceRow>>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.across {
            return None;
        }
        let marked: Vec<ResourceRow> = table.read(cx).marked_rows().into_iter().cloned().collect();
        if marked.is_empty() {
            return None;
        }
        let clear_table = table.clone();
        let mut bar = BulkBar::new(BULK_ID, marked.len(), move |_, cx| {
            clear_table.update(cx, Table::clear_marks);
        });
        if self.kind == ResourceKind::Deployment {
            let seen: Vec<Seen> = marked.iter().filter_map(|row| self.seen(row)).collect();
            let screen = cx.weak_entity();
            let wanted = desired(self.kind, &marked);
            bar = bar
                .action(BulkAction::new(
                    RESTART,
                    restart_handler(screen.clone(), seen.clone()),
                ))
                .action(BulkAction::new(SCALE, scale_handler(screen, seen, wanted)));
        }
        Some(bar.into_any_element())
    }

    fn seen(&self, row: &ResourceRow) -> Option<Seen> {
        self.target(row)
            .map(|target| Seen::new(target, row.uid.clone()))
    }
}

fn emit_all(screen: &WeakEntity<ResourcesScreen>, commands: Vec<Command>, cx: &mut gpui::App) {
    let _ = screen.update(cx, |_, cx| {
        for command in commands {
            cx.emit(ScreenEvent::Command(command));
        }
    });
}

fn bulk_detail(seen: &[Seen]) -> String {
    let named: Vec<String> = seen
        .iter()
        .map(|seen| format!("{} · {}", seen.target.cluster, seen.target))
        .collect();
    named.join(BULK_SEPARATOR)
}

fn restart_handler(
    screen: WeakEntity<ResourcesScreen>,
    seen: Vec<Seen>,
) -> impl Fn(&mut Window, &mut gpui::App) + 'static {
    move |window, cx| {
        let screen = screen.clone();
        let targets = seen.clone();
        let title = format!("{RESTART} · {}", targets.len());
        let dialog = ConfirmDialog::new(title, RESTART_BODY, RESTART, move |_, cx| {
            let commands = targets
                .iter()
                .cloned()
                .map(Command::RestartRollout)
                .collect();
            emit_all(&screen, commands, cx);
        })
        .detail(bulk_detail(&seen));
        confirm(dialog, window, cx);
    }
}

fn scale_handler(
    screen: WeakEntity<ResourcesScreen>,
    seen: Vec<Seen>,
    desired: String,
) -> impl Fn(&mut Window, &mut gpui::App) + 'static {
    move |window, cx| {
        let screen = screen.clone();
        let targets = seen.clone();
        let field = cx.new(|cx| TextFieldState::new(window, cx).default_value(desired.clone()));
        let value = field.clone();
        let title = format!("{SCALE_CONFIRM} · {}", targets.len());
        let dialog = ConfirmDialog::new(title, SCALE_BODY, SCALE_CONFIRM, move |_, cx| {
            let text = value.read(cx).value();
            let Some(replicas) = parse_replicas(&text) else {
                toast(
                    Toast::new(Status::Warn, SCALE_INVALID).detail(text.to_string()),
                    cx,
                );
                return;
            };
            let commands = targets
                .iter()
                .cloned()
                .map(|target| Command::Scale { target, replicas })
                .collect();
            emit_all(&screen, commands, cx);
        })
        .detail(bulk_detail(&seen))
        .field(field)
        .primary();
        confirm(dialog, window, cx);
    }
}

fn desired(kind: ResourceKind, rows: &[ResourceRow]) -> String {
    rows.first()
        .and_then(|row| cell(kind, row, READY_COLUMN))
        .and_then(|ready| ready.split(DESIRED_SEPARATOR).nth(1))
        .unwrap_or_default()
        .to_owned()
}

fn footer_text(shown: usize, total: usize, plural_noun: &str, clusters: usize) -> String {
    let scope = plural(clusters, "cluster", "clusters");
    let noun = plural_noun.to_lowercase();
    format!("{shown} of {total} {noun} · {scope}")
}

fn extras<'a>(
    kind: ResourceKind,
    row: &'a Resource,
    cluster: &'a str,
) -> Vec<(&'static str, Option<Cow<'a, str>>)> {
    let image_value = image(kind, row).map(image_name).map(Cow::Borrowed);
    vec![
        (STATUS_FACET, Some(Cow::Borrowed(status_value(row.status)))),
        (CLUSTER_FACET, Some(Cow::Borrowed(cluster))),
        (IMAGE_FACET, image_value),
    ]
}

fn across_grouping(kind: ResourceKind) -> Grouping<ResourceRow> {
    Grouping::new(|r: &ResourceRow| r.status)
        .by(STATUS_FACET, |r: &ResourceRow| {
            Some(Cow::Borrowed(status_value(r.status)))
        })
        .by(NAMESPACE_FACET, |r: &ResourceRow| {
            Some(Cow::Borrowed(r.namespace.as_str()))
        })
        .by(CLUSTER_FACET, |r: &ResourceRow| {
            Some(Cow::Borrowed(r.cluster_name()))
        })
        .by(IMAGE_FACET, move |r: &ResourceRow| {
            image(kind, r).map(image_name).map(Cow::Borrowed)
        })
}

fn other_clusters(found: &[Sibling], palette: &Palette) -> AnyElement {
    if found.is_empty() {
        return div()
            .text_color(palette.mute)
            .child(ONLY_HERE)
            .into_any_element();
    }
    let rows = found.iter().map(|sibling| {
        let tag = sibling.tag.clone().unwrap_or_default();
        div()
            .flex()
            .items_center()
            .gap(SPACE_4)
            .font_family(FONT_MONO)
            .text_size(TEXT_SMALL)
            .child(StatusGlyph::new(sibling.status))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(palette.fg)
                    .child(sibling.name.clone()),
            )
            .child(div().text_color(palette.fg2).child(sibling.ready.clone()))
            .child(
                div()
                    .w(OTHER_TAG)
                    .flex()
                    .justify_end()
                    .truncate()
                    .text_color(palette.mute)
                    .child(tag),
            )
    });
    div()
        .debug_selector(|| "resource-other-clusters".to_owned())
        .flex()
        .flex_col()
        .gap(SPACE_3)
        .children(rows)
        .into_any_element()
}

fn across_columns(kind: ResourceKind) -> Vec<Column<ResourceRow>> {
    let name = Column::status_text(
        "Name",
        |r: &ResourceRow| r.status,
        |r: &ResourceRow| r.name.clone().into(),
    )
    .highlight()
    .mono()
    .flex(ACROSS_NAME_WEIGHT, ACROSS_NAME_MIN)
    .sort_by_key(|r: &ResourceRow| r.name.clone());
    let cluster = Column::colored_text(
        "Cluster",
        |r: &ResourceRow| r.cluster_name().to_owned().into(),
        |_, p| p.fg2,
    )
    .highlight()
    .mono()
    .fixed(CLUSTER_WIDTH)
    .sort_by_key(|r: &ResourceRow| r.cluster_name().to_owned());
    let namespace = Column::colored_text(
        "Namespace",
        |r: &ResourceRow| r.namespace.clone().into(),
        |_, p| p.fg2,
    )
    .highlight()
    .mono()
    .fixed(NAMESPACE_WIDTH)
    .sort_by_key(|r: &ResourceRow| r.namespace.clone());
    let cells = kind
        .columns()
        .iter()
        .zip(cell_widths(kind))
        .enumerate()
        .filter(|(_, (title, _))| !ACROSS_HIDDEN.contains(title))
        .map(|(ix, (title, width))| cell_column(ix, title, *width));
    let age_column =
        Column::colored_text("Age", |r: &ResourceRow| age(r.age).into(), |_, p| p.mute)
            .mono()
            .fixed(AGE_WIDTH)
            .sort_by_key(|r: &ResourceRow| r.age);
    [name, cluster, namespace]
        .into_iter()
        .chain(cells)
        .chain(std::iter::once(age_column))
        .collect()
}

impl Render for ResourcesScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let table = self.table(cx);
        let body: AnyElement = match self.empty_state(cx) {
            Some(empty) => empty.into_any_element(),
            None => Table::pane(&table, cx).into_any_element(),
        };
        let current = self.selected_row();
        let selection = current
            .as_ref()
            .map(|row| format!("{}/{}/{}", row.cluster_name(), row.namespace, row.name));
        let shown = current.as_ref().zip(selection.as_deref());
        let motion = self.presence.track(shown, presence, cx);
        let width = inspector_width(self.kind);
        let inspector = match current {
            Some(row) => {
                let panel = self.inspector(&row, cx);
                Some(slide_in(panel, INSPECTOR_ID, motion, width))
            }
            None => self.presence.leaving().cloned().map(|row| {
                let panel = self.inspector(&row, cx);
                slide_out(panel, INSPECTOR_ID, width)
            }),
        };
        let inspecting = inspector.is_some();
        let footer = self.across.then(|| self.footer(cx));
        let bulk = self.bulk_bar(&table, cx);
        div()
            .when(inspecting, |root| root.key_context(YAML_CONTEXT))
            .on_action(cx.listener(|this, _: &EditYaml, _, cx| this.edit(cx)))
            .size_full()
            .flex()
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(self.filter.clone())
                    .child(div().flex_1().min_h_0().child(body))
                    .children(footer)
                    .children(bulk),
            )
            .children(inspector)
    }
}

fn delete_items(
    kind: ResourceKind,
    seen: Seen,
    confirm_destructive: bool,
    screen: WeakEntity<ResourcesScreen>,
) -> Vec<MenuItem> {
    let title = format!("Delete {}", kind.name());
    let detail = format!("{} · {}", seen.target.cluster, seen.target);
    vec![
        MenuItem::new("Delete…", move |window, cx| {
            let screen = screen.clone();
            let seen = seen.clone();
            if !confirm_destructive {
                let command = Command::Delete(seen);
                let _ = screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)));
                return;
            }
            let dialog = ConfirmDialog::new(
                title.clone(),
                "It is removed from the cluster now. If Flux or another controller manages it, it comes back on the next reconcile.",
                title.clone(),
                move |_, cx| {
                    let command = Command::Delete(seen.clone());
                    let _ = screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)));
                },
            )
            .detail(detail.clone());
            confirm(dialog, window, cx);
        })
        .destructive()
        .description("Remove it from the cluster. Managed objects come back on reconcile."),
    ]
}

fn columns(kind: ResourceKind) -> Vec<Column<ResourceRow>> {
    let name = Column::status_text(
        "Name",
        |r: &ResourceRow| r.status,
        |r: &ResourceRow| r.name.clone().into(),
    )
    .highlight()
    .mono()
    .flex(NAME_WEIGHT, NAME_MIN)
    .sort_by_key(|r: &ResourceRow| r.name.clone());
    let namespace = Column::colored_text(
        "Namespace",
        |r: &ResourceRow| r.namespace.clone().into(),
        |_, p| p.fg2,
    )
    .highlight()
    .fixed(NAMESPACE_WIDTH)
    .sort_by_key(|r: &ResourceRow| r.namespace.clone());
    let cells = kind
        .columns()
        .iter()
        .zip(cell_widths(kind))
        .enumerate()
        .map(|(ix, (title, width))| cell_column(ix, title, *width));
    let reason = Column::new(REASON, |r: &ResourceRow, palette| {
        reason_cell(r.status, r.diagnosis.as_ref(), palette)
    })
    .tooltip(|r: &ResourceRow| reason_tooltip(r.diagnosis.as_ref()))
    .flex(REASON_WEIGHT, REASON_MIN)
    .sort_by_key(|r: &ResourceRow| r.diagnosis.as_ref().map(|d| d.reason.clone()));
    let age_column =
        Column::colored_text("Age", |r: &ResourceRow| age(r.age).into(), |_, p| p.mute)
            .mono()
            .fixed(AGE_WIDTH)
            .sort_by_key(|r: &ResourceRow| r.age);
    std::iter::once(name)
        .chain(std::iter::once(namespace))
        .chain(cells)
        .chain(std::iter::once(reason))
        .chain(std::iter::once(age_column))
        .collect()
}

fn cell_column(ix: usize, title: &'static str, width: Cell) -> Column<ResourceRow> {
    let tinted = STATUS_COLUMNS.contains(&title);
    let column = Column::colored_text(
        title,
        move |r: &ResourceRow| r.cells.get(ix).cloned().unwrap_or_default().into(),
        move |r, p| {
            if tinted && r.status != Status::Ok {
                p.status(r.status)
            } else {
                p.fg2
            }
        },
    )
    .mono()
    .sort_by_key(move |r: &ResourceRow| r.cells.get(ix).cloned());
    match width {
        Cell::Fixed(w) => column.fixed(w),
        Cell::End(w) => column.end().fixed(w),
        Cell::Flex(weight, min) => column.flex(weight, min),
    }
}

fn note(rows: &[Resource], namespace: Option<&str>) -> String {
    let shown: Vec<&Resource> = rows
        .iter()
        .filter(|row| namespace.is_none_or(|namespace| row.namespace == namespace))
        .collect();
    let counts = Counts::of(shown.iter().map(|row| row.status));
    let flagged = counts.flagged();
    if flagged > 0 {
        format!("{flagged} of {} need attention", shown.len())
    } else {
        shown.len().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(namespace: &str, status: Status) -> Resource {
        Resource {
            namespace: namespace.into(),
            name: "podinfo".into(),
            uid: flock_core::Uid::new(String::new()),
            status,
            cells: Vec::new(),
            age: std::time::Duration::ZERO,
            yaml: String::new(),
            labels: std::collections::BTreeMap::new(),
            conditions: Vec::new(),
            diagnosis: None,
        }
    }

    #[test]
    fn the_header_counts_only_the_picked_namespace() {
        let rows = [
            row("web", Status::Err),
            row("web", Status::Ok),
            row("payments", Status::Err),
        ];
        assert_eq!(note(&rows, Some("web")), "1 of 2 need attention");
        assert_eq!(note(&rows, None), "2 of 3 need attention");
        assert_eq!(note(&rows, Some("search")), "0");
    }

    #[test]
    fn the_flock_footer_counts_rows_and_clusters() {
        assert_eq!(
            footer_text(3, 20, "Deployments", 2),
            "3 of 20 deployments · 2 clusters"
        );
        assert_eq!(footer_text(1, 1, "Pods", 1), "1 of 1 pods · 1 cluster");
    }

    #[test]
    fn flock_deployments_drop_available_and_reason_and_add_a_cluster_column() {
        let titles: Vec<String> = across_columns(ResourceKind::Deployment)
            .iter()
            .map(|c| c.title().to_owned())
            .collect();
        assert_eq!(
            titles,
            [
                "Name",
                "Cluster",
                "Namespace",
                "Ready",
                "Up-to-date",
                "Image",
                "Age"
            ]
        );
    }

    #[test]
    fn workloads_and_jobs_open_a_wider_inspector() {
        assert_eq!(inspector_width(ResourceKind::Job), INSPECTOR_WORKLOAD);
        assert_eq!(
            inspector_width(ResourceKind::Deployment),
            INSPECTOR_WORKLOAD
        );
        assert_eq!(inspector_width(ResourceKind::ConfigMap), INSPECTOR_RESOURCE);
        assert!(INSPECTOR_WORKLOAD > INSPECTOR_RESOURCE);
    }

    #[test]
    fn every_kind_has_a_width_per_column() {
        for kind in ResourceKind::ALL {
            assert_eq!(cell_widths(kind).len(), kind.columns().len(), "{kind:?}");
        }
    }
}
