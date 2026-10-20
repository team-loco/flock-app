use std::rc::Rc;

use flock_core::emptiness::Noun;
use flock_core::events::{self, Event, EventFilter, filter_counts, filter_events};
use flock_core::filter::label_values;
use flock_core::overview::plural;
use flock_core::resources::{EVENT_LIST, namespace_counts};
use flock_core::settings::NamespaceSelection;
use flock_core::time::age;
use flock_core::{
    ClusterDetail, ClusterId, ClusterSummary, Command, Counts, Page, Preferences, Status,
};
use gpui::{
    AnyElement, AppContext, Context, Entity, EventEmitter, InteractiveElement, IntoElement,
    ParentElement, Render, Styled, Window, div,
};

use super::event_inspector::{EventInspector, slot};
use super::{
    ALL_NAMESPACES, ListShape, ScreenEvent, chosen_namespace, empty_list, set_namespace_options,
};
use crate::kit::{
    Column, FilterBar, FilterEvent, Presence, Select, SelectEvent, ShowAll, StatusGlyph,
    StatusText, Table, TableEvent, chip_label,
};
use crate::theme::{
    EVENT_AGE_WIDTH, EVENT_COUNT_WIDTH, EVENT_MESSAGE_MIN, EVENT_MESSAGE_WEIGHT,
    EVENT_NAMESPACE_WIDTH, EVENT_OBJECT_MIN, EVENT_OBJECT_WEIGHT, EVENT_REASON_WIDTH, GLYPH_SLOT,
    SEARCH_WIDTH, SPACE_3, SPACE_4,
};

const NO_NAMESPACE: &str = "—";

pub struct EventsScreen {
    cluster: Option<ClusterId>,
    cluster_name: String,
    shown: usize,
    in_scope: usize,
    preferences: Preferences,
    namespaces: Entity<Select>,
    selected: Option<String>,
    requested: Option<EventFilter>,
    presence: Presence<Event>,
    events: Vec<Event>,
    filter: Entity<FilterBar>,
    table: Entity<Table<Event>>,
}

fn presence_of(screen: &mut EventsScreen) -> &mut Presence<Event> {
    &mut screen.presence
}

impl EventEmitter<ScreenEvent> for EventsScreen {}

impl EventsScreen {
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

    pub fn list_focus(&self, cx: &gpui::App) -> gpui::FocusHandle {
        gpui::Focusable::focus_handle(&self.table, cx)
    }

    pub fn show_filter(&mut self, filter: EventFilter, cx: &mut Context<Self>) {
        self.requested = Some(filter);
        self.refresh(cx);
    }

    pub fn filter_bar(&self) -> &Entity<FilterBar> {
        &self.filter
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let table = cx.new(|cx| {
            Table::new("events", columns(), cx)
                .type_ahead(|event: &Event| event.object.clone().into())
                .row_key(|event: &Event| event.key().into())
                .click_opens()
                .row_status(|e: &Event| (e.status == Status::Err).then_some(Status::Err))
        });
        cx.subscribe(&table, |this, table, event: &TableEvent, cx| {
            if let TableEvent::Opened(ix) = event {
                this.selected = table.read(cx).row(*ix).map(Event::key);
                cx.notify();
            }
        })
        .detach();
        let namespaces = cx.new(|cx| {
            Select::new("events-namespace", vec![ALL_NAMESPACES.into()], cx)
                .mono()
                .width(SEARCH_WIDTH)
        });
        cx.subscribe(&namespaces, |this, select, _: &SelectEvent, cx| {
            let chosen = chosen_namespace(select.read(cx));
            let scope = NamespaceSelection::of(chosen.as_deref());
            this.choose_namespace(scope, cx);
        })
        .detach();
        let filter = cx.new(|cx| {
            FilterBar::new("event-filter", Event::FILTER.placeholder(), window, cx)
                .scope(namespaces.clone())
        });
        cx.subscribe(&filter, |this, _, FilterEvent::Changed, cx| {
            this.refresh(cx);
        })
        .detach();
        Self {
            cluster: None,
            cluster_name: String::new(),
            shown: 0,
            in_scope: 0,
            preferences: Preferences::default(),
            namespaces,
            selected: None,
            requested: None,
            presence: Presence::default(),
            events: Vec::new(),
            filter,
            table,
        }
    }

    fn choose_namespace(&mut self, scope: NamespaceSelection, cx: &mut Context<Self>) {
        self.preferences.select_namespace(EVENT_LIST, scope);
        self.selected = None;
        self.refresh(cx);
        if let Some(cluster) = &self.cluster {
            cx.emit(ScreenEvent::Command(Command::SetPreferences(
                cluster.clone(),
                self.preferences.clone(),
            )));
        }
    }

    fn show_all_namespaces(&mut self, cx: &mut Context<Self>) {
        self.namespaces
            .update(cx, |select, cx| select.set_selected(0, cx));
        self.choose_namespace(NamespaceSelection::All, cx);
    }

    pub fn set(
        &mut self,
        cluster: &ClusterSummary,
        detail: &ClusterDetail,
        preferences: &Preferences,
        cx: &mut Context<Self>,
    ) {
        if self.cluster.as_ref() != Some(&cluster.id) {
            self.selected = None;
        }
        let scope = cluster.id.as_str().to_owned();
        self.table
            .update(cx, |table, cx| table.set_scope(scope, cx));
        self.cluster = Some(cluster.id.clone());
        self.cluster_name.clone_from(&cluster.name);
        self.preferences = preferences.clone();
        self.events.clone_from(&detail.events);
        let scope = self.preferences.namespace_selection(EVENT_LIST);
        let counts = namespace_counts(
            detail.namespaces.iter().map(|ns| ns.name.as_str()),
            self.events
                .iter()
                .filter_map(|event| event.namespace.as_deref()),
            scope.namespace(),
        );
        let total = self.events.len();
        set_namespace_options(&self.namespaces, &counts, total, scope.namespace(), cx);
        self.refresh(cx);
    }

    pub fn header_note(&self) -> (Counts, String) {
        let scope = self.preferences.namespace_selection(EVENT_LIST);
        let events = events::in_namespace(&self.events, scope.namespace());
        let counts = events::counts(&events);
        (counts, plural(events.len(), "event", "events"))
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let labels = label_values(self.events.iter().map(|event| &event.labels));
        self.filter.update(cx, |bar, cx| bar.set_labels(labels, cx));
        let scope = self.preferences.namespace_selection(EVENT_LIST);
        let scoped = events::in_namespace(&self.events, scope.namespace());
        let query = self.filter.read(cx).query().clone();
        let chips = filter_counts(&scoped, &query)
            .into_iter()
            .map(|(filter, count)| chip_label(filter.label(), count))
            .collect();
        let requested = self
            .requested
            .take()
            .and_then(|filter| EventFilter::ALL.iter().position(|known| *known == filter));
        let chip = self.filter.update(cx, |bar, cx| {
            bar.set_chips(chips, cx);
            if let Some(ix) = requested {
                bar.select_chip(ix, cx);
            }
            bar.chip()
        });
        let filter = EventFilter::ALL.get(chip).copied().unwrap_or_default();
        let rows = filter_events(&scoped, filter, &query);
        let shown = rows.len();
        self.shown = shown;
        self.in_scope = scoped.len();
        let total = scoped.len();
        self.filter
            .update(cx, |bar, cx| bar.set_matches(shown, total, cx));
        if self
            .selected
            .as_ref()
            .is_some_and(|key| !rows.iter().any(|event| &event.key() == key))
        {
            self.selected = None;
        }
        let words = query.words;
        self.table.update(cx, |table, cx| {
            table.set_highlight(words, cx);
            table.set_rows(rows, cx);
        });
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.table.update(cx, |table, cx| table.select(None, cx));
        cx.notify();
    }

    fn inspector(&self, event: &Event, cx: &mut Context<Self>) -> AnyElement {
        let target = self
            .cluster
            .as_ref()
            .and_then(|cluster| event.target(cluster));
        let close_screen = cx.weak_entity();
        let open_screen = cx.weak_entity();
        EventInspector {
            event,
            events: &self.events,
            target,
            close: Rc::new(move |_, cx| {
                let _ = close_screen.update(cx, EventsScreen::close);
            }),
            open: Rc::new(move |target, _, cx| {
                let command = Command::OpenResource(target);
                let _ = open_screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)));
            }),
        }
        .into_element()
    }
}

impl Render for EventsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self
            .selected
            .as_ref()
            .and_then(|key| self.events.iter().find(|event| &event.key() == key))
            .cloned();
        let scope = self.preferences.namespace_selection(EVENT_LIST);
        let shape = ListShape {
            shown: self.shown,
            total: self.events.len(),
            namespace: scope
                .namespace()
                .map(|namespace| (namespace, self.in_scope)),
            noun: Noun::of(Page::Events),
            cluster: &self.cluster_name,
        };
        let screen = cx.weak_entity();
        let show_all: ShowAll = Rc::new(move |_, cx| {
            let _ = screen.update(cx, EventsScreen::show_all_namespaces);
        });
        let body: AnyElement = match empty_list(&shape, &self.filter, Some(show_all), cx) {
            Some(empty) => empty.into_any_element(),
            None => self.table.clone().into_any_element(),
        };
        let mut presence = std::mem::take(&mut self.presence);
        let inspector = slot(
            current,
            &mut presence,
            presence_of,
            |event, cx| self.inspector(event, cx),
            cx,
        );
        self.presence = presence;
        div()
            .size_full()
            .flex()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && this.selected.is_some() {
                    this.close(cx);
                    cx.stop_propagation();
                }
            }))
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

pub fn reason_cell(event: &Event) -> AnyElement {
    if event.is_warning() {
        return StatusText::new(event.status, event.reason.clone()).into_any_element();
    }
    div()
        .flex()
        .items_center()
        .gap(SPACE_3)
        .min_w_0()
        .child(StatusGlyph::new(Status::Ok))
        .child(div().truncate().child(event.reason.clone()))
        .into_any_element()
}

fn columns() -> Vec<Column<Event>> {
    vec![
        Column::new("Reason", |e: &Event, _| reason_cell(e))
            .fit_by(|e: &Event| e.reason.clone().into(), GLYPH_SLOT + SPACE_4)
            .fixed(EVENT_REASON_WIDTH)
            .sort_by_key(|e: &Event| e.reason.clone()),
        Column::text("Object", |e: &Event| e.object.clone().into())
            .highlight()
            .selectable()
            .mono()
            .flex(EVENT_OBJECT_WEIGHT, EVENT_OBJECT_MIN)
            .sort_by_key(|e: &Event| e.object.clone()),
        Column::colored_text(
            "Namespace",
            |e: &Event| {
                e.namespace
                    .clone()
                    .unwrap_or_else(|| NO_NAMESPACE.into())
                    .into()
            },
            |_, p| p.fg2,
        )
        .fixed(EVENT_NAMESPACE_WIDTH)
        .sort_by_key(|e: &Event| e.namespace.clone()),
        Column::colored_text(
            "Message",
            |event: &Event| event.message.clone().into(),
            |_, palette| palette.fg2,
        )
        .highlight()
        .selectable()
        .tooltip(|event: &Event| event.message.clone().into())
        .flex(EVENT_MESSAGE_WEIGHT, EVENT_MESSAGE_MIN),
        Column::colored_text("Age", |e: &Event| age(e.age).into(), |_, p| p.mute)
            .mono()
            .fixed(EVENT_AGE_WIDTH)
            .sort_by_key(|e: &Event| e.age),
        Column::colored_text(
            "Count",
            |e: &Event| e.count.to_string().into(),
            |_, p| p.mute,
        )
        .mono()
        .end()
        .fixed(EVENT_COUNT_WIDTH)
        .sort_by_key(|e: &Event| e.count),
    ]
}
