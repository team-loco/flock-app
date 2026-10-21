use flock_core::events::{ClusterEvent, EventFilter, cluster_filter_counts, filter_cluster_events};
use flock_core::filter::{highlights, label_values};
use flock_core::time::age;
use flock_core::{Command, Snapshot, Status};
use gpui::{
    AnyElement, AppContext, Context, Entity, EventEmitter, IntoElement, ParentElement, Render,
    Styled, Window, div,
};

use super::events::reason_cell;
use crate::kit::{
    Column, EmptyState, FilterBar, FilterEvent, RawMessage, Table, TableEvent, chip_label,
};
use crate::theme::{
    EVENT_AGE_WIDTH, EVENT_COUNT_WIDTH, EVENT_MESSAGE_MIN, EVENT_MESSAGE_WEIGHT,
    EVENT_NAMESPACE_WIDTH, EVENT_OBJECT_MIN, EVENT_OBJECT_WEIGHT, EVENT_REASON_WIDTH,
    FLUX_EVENT_CLUSTER_WIDTH, GLYPH_SLOT, SPACE_4,
};

const NO_NAMESPACE: &str = "—";
const NO_EVENTS: &str = "No Flux events on any connected cluster.";
const NO_MATCH: &str = "No Flux events match this filter.";

pub struct FluxEventsScreen {
    events: Vec<ClusterEvent>,
    shown: usize,
    filter: Entity<FilterBar>,
    table: Entity<Table<ClusterEvent>>,
}

impl EventEmitter<Command> for FluxEventsScreen {}

impl FluxEventsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let table = cx.new(|cx| {
            Table::new("flux-events", columns(), cx)
                .type_ahead(|event: &ClusterEvent| event.name().to_owned().into())
                .row_key(|event: &ClusterEvent| event.key().into())
                .click_opens()
                .row_status(|e: &ClusterEvent| {
                    (e.event.status == Status::Err).then_some(Status::Err)
                })
        });
        cx.subscribe(&table, |_, table, event: &TableEvent, cx| {
            if let TableEvent::Opened(ix) = *event
                && let Some(command) = table.read(cx).row(ix).and_then(ClusterEvent::open)
            {
                cx.emit(command);
            }
        })
        .detach();
        let filter = cx.new(|cx| {
            FilterBar::new(
                "flux-events-filter",
                ClusterEvent::FILTER.placeholder(),
                window,
                cx,
            )
        });
        cx.subscribe(&filter, |this, _, FilterEvent::Changed, cx| {
            this.refresh(cx);
        })
        .detach();
        Self {
            events: Vec::new(),
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

    pub fn set_snapshot(&mut self, snapshot: &Snapshot, cx: &mut Context<Self>) {
        if self.events == snapshot.flux_events {
            return;
        }
        self.events.clone_from(&snapshot.flux_events);
        self.refresh(cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let labels = label_values(self.events.iter().map(|row| &row.event.labels));
        self.filter.update(cx, |bar, cx| bar.set_labels(labels, cx));
        let query = self.filter.read(cx).query().clone();
        let chips = cluster_filter_counts(&self.events, &query)
            .into_iter()
            .map(|(filter, count)| chip_label(filter.label(), count))
            .collect();
        let chip = self.filter.update(cx, |bar, cx| {
            bar.set_chips(chips, cx);
            bar.chip()
        });
        let filter = EventFilter::ALL.get(chip).copied().unwrap_or_default();
        let rows = filter_cluster_events(&self.events, filter, &query);
        self.shown = rows.len();
        let (shown, total) = (rows.len(), self.events.len());
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
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = if self.events.is_empty() {
            EmptyState::new(NO_EVENTS).into_any_element()
        } else if self.shown == 0 {
            EmptyState::new(NO_MATCH).into_any_element()
        } else {
            self.table.clone().into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.filter.clone())
            .child(div().flex_1().min_h_0().child(body))
    }
}

fn columns() -> Vec<Column<ClusterEvent>> {
    vec![
        Column::new("Reason", |e: &ClusterEvent, _| reason_cell(&e.event))
            .fit_by(
                |e: &ClusterEvent| e.event.reason.clone().into(),
                GLYPH_SLOT + SPACE_4,
            )
            .fixed(EVENT_REASON_WIDTH)
            .sort_by_key(|e: &ClusterEvent| e.event.reason.clone()),
        Column::text("Cluster", |e: &ClusterEvent| e.cluster_name.clone().into())
            .highlight()
            .fixed(FLUX_EVENT_CLUSTER_WIDTH)
            .sort_by_key(|e: &ClusterEvent| e.cluster_name.clone()),
        Column::text("Object", |e: &ClusterEvent| e.event.object.clone().into())
            .highlight()
            .selectable()
            .mono()
            .flex(EVENT_OBJECT_WEIGHT, EVENT_OBJECT_MIN)
            .sort_by_key(|e: &ClusterEvent| e.event.object.clone()),
        Column::colored_text(
            "Namespace",
            |e: &ClusterEvent| {
                e.event
                    .namespace
                    .clone()
                    .unwrap_or_else(|| NO_NAMESPACE.into())
                    .into()
            },
            |_, p| p.fg2,
        )
        .fixed(EVENT_NAMESPACE_WIDTH)
        .sort_by_key(|e: &ClusterEvent| e.event.namespace.clone()),
        Column::highlighted("Message", |e: &ClusterEvent, _, words| {
            let text = e.event.message.clone();
            let ranges = highlights(words, &text);
            RawMessage::new(text)
                .highlights(ranges)
                .line()
                .into_any_element()
        })
        .tooltip(|e: &ClusterEvent| e.event.message.clone().into())
        .flex(EVENT_MESSAGE_WEIGHT, EVENT_MESSAGE_MIN),
        Column::colored_text(
            "Age",
            |e: &ClusterEvent| age(e.event.age).into(),
            |_, p| p.mute,
        )
        .mono()
        .fixed(EVENT_AGE_WIDTH)
        .sort_by_key(|e: &ClusterEvent| e.event.age),
        Column::colored_text(
            "Count",
            |e: &ClusterEvent| e.event.count.to_string().into(),
            |_, p| p.mute,
        )
        .mono()
        .end()
        .fixed(EVENT_COUNT_WIDTH)
        .sort_by_key(|e: &ClusterEvent| e.event.count),
    ]
}
