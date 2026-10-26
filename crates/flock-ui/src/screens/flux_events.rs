use std::rc::Rc;
use std::sync::Arc;

use flock_core::events::{ClusterEvent, EventFilter, filter_cluster_events, filter_counts};
use flock_core::filter::label_values;
use flock_core::time::age;
use flock_core::{Command, Snapshot, Status};
use gpui::{
    AnyElement, AppContext, Context, Entity, EventEmitter, IntoElement, ParentElement, Render,
    Styled, Window, div,
};

use super::events::reason_cell;
use crate::kit::{
    Column, EmptyState, FilterBar, FilterEvent, SnapshotRow, SnapshotRows, Table, TableEvent,
    chip_label,
};
use crate::theme::{
    EVENT_AGE_WIDTH, EVENT_COUNT_WIDTH, EVENT_MESSAGE_MIN, EVENT_MESSAGE_WEIGHT,
    EVENT_NAMESPACE_WIDTH, EVENT_OBJECT_MIN, EVENT_OBJECT_WEIGHT, EVENT_REASON_WIDTH,
    FLUX_EVENT_CLUSTER_WIDTH, GLYPH_SLOT, SPACE_4,
};

const NO_NAMESPACE: &str = "—";
const NO_EVENTS: &str = "No Flux events on any connected cluster.";
const NO_MATCH: &str = "No Flux events match this filter.";

type FluxEventRow = SnapshotRow<ClusterEvent>;

pub struct FluxEventsScreen {
    events: Rc<SnapshotRows<ClusterEvent>>,
    shown: usize,
    filter: Entity<FilterBar>,
    table: Entity<Table<FluxEventRow>>,
}

impl EventEmitter<Command> for FluxEventsScreen {}

impl FluxEventsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let table = cx.new(|cx| {
            Table::new("flux-events", columns(), cx)
                .type_ahead(|event: &FluxEventRow| event.name().to_owned().into())
                .row_key(|event: &FluxEventRow| event.key().into())
                .click_opens()
                .row_status(|e: &FluxEventRow| {
                    (e.event.status == Status::Err).then_some(Status::Err)
                })
        });
        cx.subscribe(&table, |_, table, event: &TableEvent, cx| {
            if let TableEvent::Opened(ix) = *event
                && let Some(command) = table.read(cx).row(ix).and_then(|row| row.open())
            {
                cx.emit(command);
            }
        })
        .detach();
        let filter = cx.new(|cx| FilterBar::new("flux-events-filter", window, cx));
        cx.subscribe(&filter, |this, _, FilterEvent::Changed, cx| {
            this.refresh_rows(cx);
        })
        .detach();
        Self {
            events: SnapshotRows::empty(),
            shown: 0,
            filter,
            table,
        }
    }

    pub fn filter_bar(&self) -> &Entity<FilterBar> {
        &self.filter
    }

    pub fn list_focus(&self, cx: &gpui::App) -> gpui::FocusHandle {
        gpui::Focusable::focus_handle(&self.table, cx)
    }

    pub fn set_snapshot(&mut self, snapshot: &Arc<Snapshot>, cx: &mut Context<Self>) {
        if self.events.from(snapshot) {
            return;
        }
        self.events = SnapshotRows::new(snapshot, |snapshot| &snapshot.flux_events);
        self.refresh(cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let events = self.events.items();
        let labels = label_values(events.iter().map(|row| &row.event.labels));
        self.filter.update(cx, |bar, cx| bar.set_labels(labels, cx));
        self.refresh_rows(cx);
    }

    fn refresh_rows(&mut self, cx: &mut Context<Self>) {
        let events = self.events.items();
        let query = self.filter.read(cx).query().clone();
        let matched = self.events.matching(&query);
        let chips = filter_counts(
            matched
                .iter()
                .filter_map(|&ix| events.get(ix))
                .map(|row| &row.event),
        )
        .into_iter()
        .map(|(filter, count)| chip_label(filter.label(), count))
        .collect();
        let chip = self.filter.update(cx, |bar, cx| {
            bar.set_chips(chips, cx);
            bar.chip()
        });
        let filter = EventFilter::ALL.get(chip).copied().unwrap_or_default();
        let kept = filter_cluster_events(events, &matched, filter);
        let total = events.len();
        let rows = self.events.rows(kept);
        self.shown = rows.len();
        let shown = rows.len();
        self.filter
            .update(cx, |bar, cx| bar.set_matches(shown, total, cx));
        let words = query.words;
        self.table.update(cx, |table, cx| {
            table.set_highlight(words, cx);
            table.set_rows(rows, cx);
        });
        cx.notify();
    }
}

impl Render for FluxEventsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = if self.events.items().is_empty() {
            EmptyState::new(NO_EVENTS).into_any_element()
        } else if self.shown == 0 {
            EmptyState::new(NO_MATCH).into_any_element()
        } else {
            Table::pane(&self.table, cx).into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.filter.clone())
            .child(div().flex_1().min_h_0().child(body))
    }
}

fn columns() -> Vec<Column<FluxEventRow>> {
    vec![
        Column::new("Reason", |e: &FluxEventRow, _| reason_cell(&e.event))
            .fit_by(
                |e: &FluxEventRow| e.event.reason.clone().into(),
                GLYPH_SLOT + SPACE_4,
            )
            .fixed(EVENT_REASON_WIDTH)
            .sort_by_key(|e: &FluxEventRow| e.event.reason.clone()),
        Column::text("Cluster", |e: &FluxEventRow| e.cluster_name.clone().into())
            .highlight()
            .fixed(FLUX_EVENT_CLUSTER_WIDTH)
            .sort_by_key(|e: &FluxEventRow| e.cluster_name.clone()),
        Column::text("Object", |e: &FluxEventRow| e.event.object.clone().into())
            .highlight()
            .selectable()
            .mono()
            .flex(EVENT_OBJECT_WEIGHT, EVENT_OBJECT_MIN)
            .sort_by_key(|e: &FluxEventRow| e.event.object.clone()),
        Column::colored_text(
            "Namespace",
            |e: &FluxEventRow| {
                e.event
                    .namespace
                    .clone()
                    .unwrap_or_else(|| NO_NAMESPACE.into())
                    .into()
            },
            |_, p| p.fg2,
        )
        .fixed(EVENT_NAMESPACE_WIDTH)
        .sort_by_key(|e: &FluxEventRow| e.event.namespace.clone()),
        Column::colored_text(
            "Message",
            |e: &FluxEventRow| e.event.message.clone().into(),
            |_, p| p.fg2,
        )
        .highlight()
        .mono()
        .tooltip(|e: &FluxEventRow| e.event.message.clone().into())
        .flex(EVENT_MESSAGE_WEIGHT, EVENT_MESSAGE_MIN),
        Column::colored_text(
            "Age",
            |e: &FluxEventRow| age(e.event.age).into(),
            |_, p| p.mute,
        )
        .mono()
        .fixed(EVENT_AGE_WIDTH)
        .sort_by_key(|e: &FluxEventRow| e.event.age),
        Column::colored_text(
            "Count",
            |e: &FluxEventRow| e.event.count.to_string().into(),
            |_, p| p.mute,
        )
        .mono()
        .end()
        .fixed(EVENT_COUNT_WIDTH)
        .sort_by_key(|e: &FluxEventRow| e.event.count),
    ]
}
