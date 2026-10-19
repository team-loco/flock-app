use std::collections::HashMap;
use std::rc::Rc;

use flock_core::emptiness::Noun;
use flock_core::events::Event;
use flock_core::filter::{StatusFilter, label_values};
use flock_core::pods::PodEvent;
use flock_core::related::Related;
use flock_core::resources::{
    Resource, filter_counts, filter_resources, namespace_choices, namespaces_of,
};
use flock_core::settings::NamespaceSelection;
use flock_core::time::age;
use flock_core::{
    ClusterDetail, ClusterId, ClusterSummary, Command, Counts, Preferences, ResourceKind,
    ResourceRef, Seen, Status,
};
use gpui::{
    AnyElement, AppContext, Context, Entity, EventEmitter, InteractiveElement, IntoElement,
    ParentElement, Pixels, Render, SharedString, Styled, Subscription, WeakEntity, Window, div,
    prelude::FluentBuilder, px,
};

use super::ScreenEvent;
use crate::kit::{
    Button, CONDITIONS, CodeView, Column, ConditionTable, ConfirmDialog, EVENTS, EmptyState,
    EventList, FilterBar, FilterEvent, IconButton, Inspector, MenuItem, Presence, Select,
    SelectEvent, ShowAll, Skeleton, Table, TableEvent, WHATS_WRONG, WhatsWrong, chip_label,
    confirm, reason_cell, reason_tooltip, slide_in, slide_out,
};
use crate::screens::edit::{EDIT_KEYS, EditYaml, YAML_CONTEXT, edit_command};
use crate::screens::related::{OnPick, Pick, RELATED, inspector_section};
use crate::screens::{EDIT_YAML, ListShape, edit_item, empty_list, new_tab_item};
use crate::theme::{INSPECTOR_RESOURCE, INSPECTOR_WORKLOAD, REASON_MIN, REASON_WEIGHT, SPACE_3};

const ALL_NAMESPACES: &str = "All namespaces";
const NAME_WEIGHT: f32 = 1.4;
const NAME_MIN: Pixels = px(200.);
const NAMESPACE_WIDTH: Pixels = px(130.);
const AGE_WIDTH: Pixels = px(44.);
const SELECT_WIDTH: Pixels = px(180.);
const INSPECTOR_ID: &str = "resource-inspector";
const REASON: &str = "Reason";
const YAML: &str = "YAML";
const STATUS_COLUMNS: [&str; 2] = ["Ready", "Status"];

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

pub struct ResourcesScreen {
    cluster: Option<ClusterId>,
    cluster_name: String,
    kind: ResourceKind,
    rows: Vec<Resource>,
    events: Vec<Event>,
    shown: usize,
    confirm_destructive: bool,
    namespace: Option<String>,
    awaiting: Option<String>,
    preferences: Preferences,
    selected: Option<(String, String)>,
    related: Vec<Related>,
    revealed: Option<(ResourceKind, String, String)>,
    tables: HashMap<ResourceKind, Entity<Table<Resource>>>,
    namespaces: Entity<Select>,
    filter: Entity<FilterBar>,
    subscriptions: Vec<Subscription>,
    presence: Presence<Resource>,
}

fn presence(screen: &mut ResourcesScreen) -> &mut Presence<Resource> {
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
        let namespaces = cx.new(|cx| {
            Select::new("resource-namespace", vec![ALL_NAMESPACES.into()], cx)
                .mono()
                .width(SELECT_WIDTH)
        });
        let filter =
            cx.new(|cx| FilterBar::new("resource-filter", window, cx).scope(namespaces.clone()));
        let subscriptions = vec![
            cx.subscribe(&namespaces, |this, select, _: &SelectEvent, cx| {
                let label = select.read(cx).selected_label().cloned();
                let choice = label
                    .filter(|l| l.as_ref() != ALL_NAMESPACES)
                    .map(|l| l.to_string());
                this.choose_namespace(choice, cx);
            }),
            cx.subscribe(&filter, |this, _, FilterEvent::Changed, cx| {
                this.refresh(cx);
            }),
        ];
        Self {
            cluster: None,
            cluster_name: String::new(),
            kind: ResourceKind::Deployment,
            rows: Vec::new(),
            events: Vec::new(),
            shown: 0,
            confirm_destructive: true,
            namespace: None,
            awaiting: None,
            preferences: Preferences::default(),
            selected: None,
            related: Vec::new(),
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
        detail: &ClusterDetail,
        confirm_destructive: bool,
        preferences: &Preferences,
        cx: &mut Context<Self>,
    ) {
        let rows = detail.resources(kind);
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
        if self.related != detail.related {
            self.related.clone_from(&detail.related);
            cx.notify();
        }
        let switched = self.kind != kind || self.cluster.as_ref() != Some(&cluster.id);
        if self.events != detail.events {
            self.events.clone_from(&detail.events);
            cx.notify();
        }
        if !switched && !rescoped && self.rows == rows {
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
            self.selected = Some((namespace, name));
        }
        self.rows = rows.to_vec();
        let known = detail
            .namespaces
            .iter()
            .map(|n| n.name.clone())
            .chain(namespaces_of(&self.rows));
        let mut options: Vec<SharedString> = vec![ALL_NAMESPACES.into()];
        options.extend(
            namespace_choices(known, self.namespace.as_deref())
                .into_iter()
                .map(SharedString::from),
        );
        let current = self.namespace.clone();
        self.namespaces.update(cx, |select, cx| {
            select.set_options(options.clone(), cx);
            let ix = current
                .and_then(|ns| options.iter().position(|o| o.as_ref() == ns))
                .unwrap_or_default();
            select.set_selected(ix, cx);
        });
        self.refresh(cx);
    }

    pub fn reveal(
        &mut self,
        kind: ResourceKind,
        namespace: String,
        name: String,
        cx: &mut Context<Self>,
    ) {
        let key = (namespace.clone(), name.clone());
        if kind == self.kind && self.selected.as_ref() == Some(&key) {
            return;
        }
        if kind == self.kind && self.cluster.is_some() {
            self.show_namespace_of(&namespace, cx);
            self.selected = Some((namespace, name));
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
            let inside = self.rows.iter().filter(|r| r.namespace == namespace);
            (namespace, inside.count())
        });
        let shape = ListShape {
            shown: self.shown,
            total: self.rows.len(),
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

    pub fn header_note(&self) -> (Counts, String) {
        note(&self.rows, self.namespace.as_deref())
    }

    fn table(&mut self, cx: &mut Context<Self>) -> Entity<Table<Resource>> {
        let kind = self.kind;
        if let Some(table) = self.tables.get(&kind) {
            return table.clone();
        }
        let table = cx.new(|cx| {
            Table::new(format!("resources-{}", kind.name()), columns(kind), cx)
                .row_key(|r: &Resource| format!("{}/{}", r.namespace, r.name).into())
                .row_status(|r: &Resource| (r.status != Status::Ok).then_some(r.status))
        });
        let subscription = cx.subscribe(&table, |this, table, event: &TableEvent, cx| {
            let ix = match *event {
                TableEvent::Selected(ix) => ix,
                TableEvent::Opened(ix) => ix,
                TableEvent::Marked => return,
            };
            let key = table
                .read(cx)
                .row(ix)
                .map(|r| (r.namespace.clone(), r.name.clone()));
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
        let labels = label_values(
            self.rows
                .iter()
                .filter(|row| namespace.is_none_or(|namespace| &row.namespace == namespace))
                .map(|row| &row.labels),
        );
        self.filter.update(cx, |bar, cx| bar.set_labels(labels, cx));
        let (query, chip) = {
            let bar = self.filter.read(cx);
            (bar.query().clone(), bar.chip())
        };
        let base = filter_resources(&self.rows, self.namespace.as_deref(), &query);
        let chips = filter_counts(&base)
            .into_iter()
            .map(|(filter, n)| chip_label(filter.label(), n))
            .collect();
        let chip = StatusFilter::at(chip);
        let rows: Vec<Resource> = base.into_iter().filter(|r| chip.keeps(r.status)).collect();
        let shown = rows.len();
        self.shown = shown;
        let total = self.rows.len();
        self.filter.update(cx, |bar, cx| {
            bar.set_chips(chips, cx);
            bar.set_matches(shown, total, cx);
        });
        if self
            .selected
            .as_ref()
            .is_some_and(|(ns, name)| !rows.iter().any(|r| &r.namespace == ns && &r.name == name))
        {
            self.selected = None;
        }
        let position = self.selected.as_ref().and_then(|(ns, name)| {
            rows.iter()
                .position(|r| &r.namespace == ns && &r.name == name)
        });
        let table = self.table(cx);
        let scope = self.cluster.as_ref().map(|c| c.as_str().to_owned());
        let actions = self.row_actions(cx);
        let words = query.words.clone();
        table.update(cx, |table, cx| {
            if let Some(scope) = scope {
                table.set_scope(scope, cx);
            }
            table.set_highlight(words, cx);
            table.set_row_actions(actions, cx);
            table.set_rows(rows, cx);
            if position.is_some() {
                table.select(position, cx);
            }
        });
        cx.notify();
    }

    fn select(&mut self, key: Option<(String, String)>, cx: &mut Context<Self>) {
        self.selected = key;
        let target = self.selected_row().and_then(|row| self.target(row));
        if let Some(target) = target {
            cx.emit(ScreenEvent::Command(Command::InspectResource(target)));
        }
        cx.notify();
    }

    fn open(&mut self, cx: &mut Context<Self>) {
        let target = self.selected_row().and_then(|row| self.target(row));
        if let Some(target) = target {
            cx.emit(ScreenEvent::Command(Command::OpenResource(target)));
        }
    }

    fn edit(&mut self, cx: &mut Context<Self>) {
        let target = self.selected_row().and_then(|row| self.target(row));
        if let Some(target) = target {
            cx.emit(ScreenEvent::Command(edit_command(target)));
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        let table = self.table(cx);
        table.update(cx, |table, cx| table.select(None, cx));
        self.select(None, cx);
    }

    fn selected_row(&self) -> Option<&Resource> {
        let (ns, name) = self.selected.as_ref()?;
        self.rows
            .iter()
            .find(|r| &r.namespace == ns && &r.name == name)
    }

    fn target(&self, row: &Resource) -> Option<ResourceRef> {
        Some(ResourceRef {
            cluster: self.cluster.clone()?,
            kind: self.kind.name().into(),
            namespace: Some(row.namespace.clone()),
            name: row.name.clone(),
        })
    }

    fn row_actions(&self, cx: &mut Context<Self>) -> impl Fn(&Resource) -> Vec<MenuItem> + 'static {
        let cluster = self.cluster.clone();
        let kind = self.kind;
        let confirm_destructive = self.confirm_destructive;
        let screen = cx.weak_entity();
        move |row: &Resource| {
            let Some(cluster) = cluster.clone() else {
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

    fn inspector(&self, row: &Resource, cx: &mut Context<Self>) -> AnyElement {
        let close = IconButton::glyph("close-resource", "×", "Close")
            .on_click(cx.listener(|this, _, _, cx| this.close(cx)));
        let edit = Button::new("edit-resource", EDIT_YAML)
            .small()
            .tooltip(EDIT_KEYS)
            .on_click(cx.listener(|this, _, _, cx| this.edit(cx)));
        let actions = div().flex().gap(SPACE_3).child(edit).child(close);
        let subtitle = format!("{} · {}", self.kind.name(), row.namespace);
        let yaml: AnyElement = if row.yaml.is_empty() {
            Skeleton::rows().into_any_element()
        } else {
            CodeView::new("resource-yaml", row.yaml.clone())
                .yaml()
                .into_any_element()
        };
        let events: Vec<PodEvent> = row
            .events(self.kind, &self.events)
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
        let related = self
            .target(row)
            .and_then(|target| self.related.iter().find(|related| related.target == target));
        let related = inspector_section(related, &pick, cx);
        let mut inspector = Inspector::new(INSPECTOR_ID, row.name.clone())
            .mono()
            .width(inspector_width(self.kind))
            .status(row.status, subtitle)
            .actions(actions);
        if let Some(diagnosis) = row.diagnosis.clone() {
            inspector =
                inspector.section(Some(WHATS_WRONG), WhatsWrong::new(row.status, diagnosis));
        }
        if !row.conditions.is_empty() {
            inspector = inspector.section(Some(CONDITIONS), ConditionTable::new(&row.conditions));
        }
        inspector
            .section(Some(EVENTS), EventList::new(events))
            .section(Some(RELATED), related)
            .section(Some(YAML), yaml)
            .into_any_element()
    }
}

impl Render for ResourcesScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let table = self.table(cx);
        let body: AnyElement = match self.empty_state(cx) {
            Some(empty) => empty.into_any_element(),
            None => table.into_any_element(),
        };
        let current = self.selected_row().cloned();
        let selection = current
            .as_ref()
            .map(|row| format!("{}/{}", row.namespace, row.name));
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
        div()
            .when(inspecting, |root| root.key_context(YAML_CONTEXT))
            .on_action(cx.listener(|this, _: &EditYaml, _, cx| this.edit(cx)))
            .size_full()
            .flex()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(self.filter.clone())
                    .child(div().flex_1().min_h_0().child(body)),
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

fn columns(kind: ResourceKind) -> Vec<Column<Resource>> {
    let name = Column::status_text(
        "Name",
        |r: &Resource| r.status,
        |r: &Resource| r.name.clone().into(),
    )
    .highlight()
    .mono()
    .flex(NAME_WEIGHT, NAME_MIN)
    .sort_by_key(|r: &Resource| r.name.clone());
    let namespace = Column::colored_text(
        "Namespace",
        |r: &Resource| r.namespace.clone().into(),
        |_, p| p.fg2,
    )
    .highlight()
    .fixed(NAMESPACE_WIDTH)
    .sort_by_key(|r: &Resource| r.namespace.clone());
    let cells = kind
        .columns()
        .iter()
        .zip(cell_widths(kind))
        .enumerate()
        .map(|(ix, (title, width))| cell_column(ix, title, *width));
    let reason = Column::new(REASON, |r: &Resource, palette| {
        reason_cell(r.status, r.diagnosis.as_ref(), palette)
    })
    .tooltip(|r: &Resource| reason_tooltip(r.diagnosis.as_ref()))
    .flex(REASON_WEIGHT, REASON_MIN)
    .sort_by_key(|r: &Resource| r.diagnosis.as_ref().map(|d| d.reason.clone()));
    let age_column = Column::colored_text("Age", |r: &Resource| age(r.age).into(), |_, p| p.mute)
        .mono()
        .fixed(AGE_WIDTH)
        .sort_by_key(|r: &Resource| r.age);
    std::iter::once(name)
        .chain(std::iter::once(namespace))
        .chain(cells)
        .chain(std::iter::once(reason))
        .chain(std::iter::once(age_column))
        .collect()
}

fn cell_column(ix: usize, title: &'static str, width: Cell) -> Column<Resource> {
    let tinted = STATUS_COLUMNS.contains(&title);
    let column = Column::colored_text(
        title,
        move |r: &Resource| r.cells.get(ix).cloned().unwrap_or_default().into(),
        move |r, p| {
            if tinted && r.status != Status::Ok {
                p.status(r.status)
            } else {
                p.fg2
            }
        },
    )
    .mono()
    .sort_by_key(move |r: &Resource| r.cells.get(ix).cloned());
    match width {
        Cell::Fixed(w) => column.fixed(w),
        Cell::End(w) => column.end().fixed(w),
        Cell::Flex(weight, min) => column.flex(weight, min),
    }
}

fn note(rows: &[Resource], namespace: Option<&str>) -> (Counts, String) {
    let shown: Vec<&Resource> = rows
        .iter()
        .filter(|row| namespace.is_none_or(|namespace| row.namespace == namespace))
        .collect();
    let counts = Counts::of(shown.iter().map(|row| row.status));
    let flagged = counts.flagged();
    let text = if flagged > 0 {
        format!("{flagged} of {} need attention", shown.len())
    } else {
        shown.len().to_string()
    };
    (counts, text)
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
        let (counts, text) = note(&rows, Some("web"));
        assert_eq!(counts, Counts::total(2).with_err(1));
        assert_eq!(text, "1 of 2 need attention");
        assert_eq!(note(&rows, None).1, "2 of 3 need attention");
        assert_eq!(note(&rows, Some("search")).1, "0");
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
