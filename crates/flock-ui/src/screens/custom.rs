use std::collections::HashMap;
use std::rc::Rc;

use flock_core::crd::{Crd, CrdKey, CustomObjects};
use flock_core::diagnosis;
use flock_core::emptiness::Noun;
use flock_core::events::Event;
use flock_core::filter::{StatusFilter, label_values};
use flock_core::pods::PodEvent;
use flock_core::related::Related;
use flock_core::resources::{Resource, filter_counts, filter_resources, namespace_counts};
use flock_core::settings::NamespaceSelection;
use flock_core::time::age;
use flock_core::{
    ClusterDetail, ClusterId, ClusterSummary, Command, Counts, Page, Preferences, ResourceRef,
    Status,
};
use gpui::{
    AnyElement, AnyView, AppContext, Context, Entity, EventEmitter, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, Styled, Subscription, Window, div,
    prelude::FluentBuilder, px,
};

use super::ScreenEvent;
use crate::kit::{
    Button, CONDITIONS, CodeView, Column, ConditionTable, EVENTS, EmptyState, EventList, FilterBar,
    FilterEvent, IconButton, Inspector, MenuItem, Presence, Select, SelectEvent, ShowAll, Skeleton,
    Table, TableEvent, WHATS_WRONG, WhatsWrong, chip_label, reason_cell, reason_tooltip, slide_in,
    slide_out,
};
use crate::screens::edit::{EDIT_KEYS, EditYaml, YAML_CONTEXT, edit_command};
use crate::screens::related::{OnPick, Pick, RELATED, inspector_section};
use crate::screens::{
    ALL_NAMESPACES, EDIT_YAML, ListShape, chosen_namespace, edit_item, empty_list, new_tab_item,
    set_namespace_options,
};
use crate::theme::{INSPECTOR_RESOURCE, REASON_MIN, REASON_WEIGHT, SPACE_3};

const NAME_WEIGHT: f32 = 1.4;
const NAME_MIN: Pixels = px(200.);
const NAMESPACE_WIDTH: Pixels = px(130.);
const CELL_WEIGHT: f32 = 1.;
const CELL_MIN: Pixels = px(110.);
const AGE_WIDTH: Pixels = px(44.);
const SELECT_WIDTH: Pixels = px(180.);
const INSPECTOR_ID: &str = "custom-inspector";
const TABLE_PREFIX: &str = "custom";
const REASON: &str = "Reason";
const YAML: &str = "YAML";
const SUBTITLE_SEPARATOR: &str = " · ";

pub struct CustomResourcesScreen {
    cluster: Option<ClusterId>,
    cluster_name: String,
    crd: Option<Crd>,
    rows: Vec<Resource>,
    cluster_namespaces: Vec<String>,
    events: Vec<Event>,
    related: Vec<Related>,
    shown: usize,
    namespace: Option<String>,
    awaiting: Option<String>,
    preferences: Preferences,
    selected: Option<(String, String)>,
    revealed: Option<(CrdKey, String, String)>,
    tables: HashMap<CrdKey, Entity<Table<Resource>>>,
    namespaces: Entity<Select>,
    filter: Entity<FilterBar>,
    subscriptions: Vec<Subscription>,
    presence: Presence<Resource>,
}

fn presence(screen: &mut CustomResourcesScreen) -> &mut Presence<Resource> {
    &mut screen.presence
}

impl EventEmitter<ScreenEvent> for CustomResourcesScreen {}

impl CustomResourcesScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let namespaces = cx.new(|cx| {
            Select::new("custom-namespace", vec![ALL_NAMESPACES.into()], cx)
                .mono()
                .width(SELECT_WIDTH)
        });
        let filter =
            cx.new(|cx| FilterBar::new("custom-filter", window, cx).scope(namespaces.clone()));
        let subscriptions = vec![
            cx.subscribe(&namespaces, |this, select, _: &SelectEvent, cx| {
                let choice = chosen_namespace(select.read(cx));
                this.choose_namespace(choice, cx);
            }),
            cx.subscribe(&filter, |this, _, FilterEvent::Changed, cx| {
                this.refresh(cx);
            }),
        ];
        Self {
            cluster: None,
            cluster_name: String::new(),
            crd: None,
            rows: Vec::new(),
            cluster_namespaces: Vec::new(),
            events: Vec::new(),
            related: Vec::new(),
            shown: 0,
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

    pub fn filter_bar(&self) -> &Entity<FilterBar> {
        &self.filter
    }

    pub fn list_focus(&mut self, cx: &mut Context<Self>) -> Option<gpui::FocusHandle> {
        let table = self.table(cx)?;
        Some(gpui::Focusable::focus_handle(&table, cx))
    }

    pub fn header_note(&self) -> (Counts, String) {
        let shown: Vec<&Resource> = self
            .rows
            .iter()
            .filter(|row| self.in_namespace(row))
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

    fn in_namespace(&self, row: &Resource) -> bool {
        self.namespace
            .as_ref()
            .is_none_or(|namespace| &row.namespace == namespace)
    }

    pub fn set(
        &mut self,
        cluster: &ClusterSummary,
        crd: &Crd,
        objects: Option<&CustomObjects>,
        detail: Option<&ClusterDetail>,
        preferences: &Preferences,
        cx: &mut Context<Self>,
    ) {
        let rows = objects.map_or(&[][..], |objects| objects.rows.as_slice());
        let list = crd.key.qualified();
        let namespace = preferences
            .namespace_selection(&list)
            .namespace()
            .map(str::to_owned)
            .filter(|_| crd.namespaced);
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
        let events = detail.map_or(&[][..], |detail| detail.events.as_slice());
        if self.events != events {
            self.events = events.to_vec();
            cx.notify();
        }
        let related = detail.map_or(&[][..], |detail| detail.related.as_slice());
        if self.related != related {
            self.related = related.to_vec();
            cx.notify();
        }
        let switched = self.crd.as_ref() != Some(crd) || self.cluster.as_ref() != Some(&cluster.id);
        if !switched && !rescoped && self.rows == rows {
            return;
        }
        if switched {
            self.selected = None;
            let scope: Option<AnyView> = crd.namespaced.then(|| self.namespaces.clone().into());
            self.filter.update(cx, |bar, cx| bar.set_scope(scope, cx));
        }
        self.cluster = Some(cluster.id.clone());
        self.cluster_name.clone_from(&cluster.name);
        self.crd = Some(crd.clone());
        if let Some((_, namespace, name)) = self.revealed.take_if(|(key, _, _)| key == &crd.key) {
            self.show_namespace_of(&namespace, cx);
            self.selected = Some((namespace, name));
        }
        self.rows = rows.to_vec();
        self.cluster_namespaces = detail
            .into_iter()
            .flat_map(|detail| detail.namespaces.iter().map(|ns| ns.name.clone()))
            .collect();
        self.sync_namespaces(cx);
        self.refresh(cx);
    }

    fn sync_namespaces(&self, cx: &mut Context<Self>) {
        if !self.crd.as_ref().is_some_and(|crd| crd.namespaced) {
            return;
        }
        let counts = namespace_counts(
            self.cluster_namespaces.iter().map(String::as_str),
            self.rows.iter().map(|row| row.namespace.as_str()),
            self.namespace.as_deref(),
        );
        let chosen = self.namespace.as_deref();
        set_namespace_options(&self.namespaces, &counts, self.rows.len(), chosen, cx);
    }

    pub fn reveal(&mut self, key: CrdKey, namespace: String, name: String, cx: &mut Context<Self>) {
        let showing = self.crd.as_ref().is_some_and(|crd| crd.key == key);
        let wanted = (namespace.clone(), name.clone());
        if showing && self.selected.as_ref() == Some(&wanted) {
            return;
        }
        if showing && self.cluster.is_some() {
            self.show_namespace_of(&namespace, cx);
            self.selected = Some(wanted);
            self.refresh(cx);
        } else {
            self.revealed = Some((key, namespace, name));
        }
    }

    fn show_namespace_of(&mut self, namespace: &str, cx: &mut Context<Self>) {
        if self.namespace.as_ref().is_some_and(|ns| ns != namespace) {
            self.choose_namespace(Some(namespace.to_owned()), cx);
            self.awaiting = Some(namespace.to_owned());
            self.sync_namespaces(cx);
        }
    }

    fn choose_namespace(&mut self, choice: Option<String>, cx: &mut Context<Self>) {
        let (Some(cluster), Some(crd)) = (self.cluster.clone(), self.crd.as_ref()) else {
            return;
        };
        self.awaiting = None;
        let selection = NamespaceSelection::of(choice.as_deref());
        self.preferences
            .select_namespace(&crd.key.qualified(), selection);
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
            noun: Noun::of(Page::CustomResources),
            cluster: &self.cluster_name,
        };
        let screen = cx.weak_entity();
        let show_all: ShowAll = Rc::new(move |_, cx| {
            let _ = screen.update(cx, CustomResourcesScreen::show_all_namespaces);
        });
        empty_list(&shape, &self.filter, Some(show_all), cx)
    }

    fn table(&mut self, cx: &mut Context<Self>) -> Option<Entity<Table<Resource>>> {
        let crd = self.crd.clone()?;
        if let Some(table) = self.tables.get(&crd.key) {
            return Some(table.clone());
        }
        let id = format!("{TABLE_PREFIX}-{}", crd.name());
        let table = cx.new(|cx| {
            Table::new(id, columns(&crd), cx)
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
        });
        self.subscriptions.push(subscription);
        self.tables.insert(crd.key, table.clone());
        Some(table)
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let labels = label_values(
            self.rows
                .iter()
                .filter(|row| self.in_namespace(row))
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
        let Some(table) = self.table(cx) else {
            return;
        };
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

    fn edit(&mut self, cx: &mut Context<Self>) {
        let target = self.selected_row().and_then(|row| self.target(row));
        if let Some(target) = target {
            cx.emit(ScreenEvent::Command(edit_command(target)));
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if let Some(table) = self.table(cx) {
            table.update(cx, |table, cx| table.select(None, cx));
        }
        self.select(None, cx);
    }

    fn selected_row(&self) -> Option<&Resource> {
        let (ns, name) = self.selected.as_ref()?;
        self.rows
            .iter()
            .find(|r| &r.namespace == ns && &r.name == name)
    }

    fn target(&self, row: &Resource) -> Option<ResourceRef> {
        let crd = self.crd.as_ref()?;
        Some(object_ref(self.cluster.clone()?, crd, row))
    }

    fn row_actions(&self, cx: &mut Context<Self>) -> impl Fn(&Resource) -> Vec<MenuItem> + 'static {
        let cluster = self.cluster.clone();
        let crd = self.crd.clone();
        let screen = cx.weak_entity();
        move |row: &Resource| {
            let (Some(cluster), Some(crd)) = (cluster.clone(), crd.as_ref()) else {
                return Vec::new();
            };
            let target = object_ref(cluster, crd, row);
            let open_screen = screen.clone();
            let open = new_tab_item(Command::OpenResource(target.clone()), move |command, cx| {
                let _ = open_screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)));
            });
            let edit_screen = screen.clone();
            let edit = edit_item(target, move |command, cx| {
                let _ = edit_screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)));
            });
            vec![open, edit]
        }
    }

    fn inspector(&self, row: &Resource, cx: &mut Context<Self>) -> AnyElement {
        let close = IconButton::glyph("close-custom", "×", "Close")
            .on_click(cx.listener(|this, _, _, cx| this.close(cx)));
        let edit = Button::new("edit-custom", EDIT_YAML)
            .small()
            .tooltip(EDIT_KEYS)
            .on_click(cx.listener(|this, _, _, cx| this.edit(cx)));
        let actions = div().flex().gap(SPACE_3).child(edit).child(close);
        let kind = self
            .crd
            .as_ref()
            .map(|crd| crd.key.kind.clone())
            .unwrap_or_default();
        let subtitle = if row.namespace.is_empty() {
            kind.clone()
        } else {
            format!("{kind}{SUBTITLE_SEPARATOR}{}", row.namespace)
        };
        let yaml: AnyElement = if row.yaml.is_empty() {
            Skeleton::rows().into_any_element()
        } else {
            CodeView::new("custom-yaml", row.yaml.clone())
                .yaml()
                .into_any_element()
        };
        let namespace = (!row.namespace.is_empty()).then_some(row.namespace.as_str());
        let events: Vec<PodEvent> = diagnosis::about(&self.events, &kind, namespace, &row.name)
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
            .width(INSPECTOR_RESOURCE)
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

fn object_ref(cluster: ClusterId, crd: &Crd, row: &Resource) -> ResourceRef {
    let namespace = crd.namespaced.then(|| row.namespace.clone());
    crd.key.object(cluster, namespace, row.name.clone())
}

impl Render for CustomResourcesScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = match (self.empty_state(cx), self.table(cx)) {
            (Some(empty), _) => empty.into_any_element(),
            (None, Some(table)) => table.into_any_element(),
            (None, None) => Skeleton::rows().into_any_element(),
        };
        let current = self.selected_row().cloned();
        let selection = current
            .as_ref()
            .map(|row| format!("{}/{}", row.namespace, row.name));
        let shown = current.as_ref().zip(selection.as_deref());
        let motion = self.presence.track(shown, presence, cx);
        let inspector = match current {
            Some(row) => {
                let panel = self.inspector(&row, cx);
                Some(slide_in(panel, INSPECTOR_ID, motion, INSPECTOR_RESOURCE))
            }
            None => self.presence.leaving().cloned().map(|row| {
                let panel = self.inspector(&row, cx);
                slide_out(panel, INSPECTOR_ID, INSPECTOR_RESOURCE)
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

fn columns(crd: &Crd) -> Vec<Column<Resource>> {
    let name = Column::status_text(
        "Name",
        |r: &Resource| r.status,
        |r: &Resource| r.name.clone().into(),
    )
    .highlight()
    .mono()
    .flex(NAME_WEIGHT, NAME_MIN)
    .sort_by_key(|r: &Resource| r.name.clone());
    let namespace = crd.namespaced.then(|| {
        Column::colored_text(
            "Namespace",
            |r: &Resource| r.namespace.clone().into(),
            |_, p| p.fg2,
        )
        .highlight()
        .fixed(NAMESPACE_WIDTH)
        .sort_by_key(|r: &Resource| r.namespace.clone())
    });
    let cells = crd.columns.iter().enumerate().map(|(ix, column)| {
        Column::colored_text(
            column.name.clone(),
            move |r: &Resource| r.cells.get(ix).cloned().unwrap_or_default().into(),
            |_, p| p.fg2,
        )
        .mono()
        .flex(CELL_WEIGHT, CELL_MIN)
        .sort_by_key(move |r: &Resource| r.cells.get(ix).cloned())
    });
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
        .chain(namespace)
        .chain(cells)
        .chain(std::iter::once(reason))
        .chain(std::iter::once(age_column))
        .collect()
}
