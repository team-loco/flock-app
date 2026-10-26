use std::rc::Rc;
use std::sync::Arc;

use flock_core::emptiness::Noun;
use flock_core::filter::{StatusFilter, label_values};
use flock_core::overview::plural;
use flock_core::{ClusterId, ClusterSummary, Command, FluxAction, FluxObject, Page, Snapshot};
use gpui::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, IntoElement, ParentElement, Pixels,
    Render, Styled, WeakEntity, Window, div, px,
};

use super::{ListShape, ScreenEvent, empty_list};
use crate::kit::{
    Column, FilterBar, FilterEvent, MenuItem, SnapshotRow, SnapshotRows, Table, TableEvent,
    chip_label, reason_cell, reason_tooltip,
};
use crate::screens::{edit_item, new_tab_item};
use crate::theme::{REASON_MIN, REASON_WEIGHT};

const NAME_WEIGHT: f32 = 1.;
const NAME_MIN: Pixels = px(220.);
const NAMESPACE_WIDTH: Pixels = px(130.);
const KIND_WIDTH: Pixels = px(130.);
const STATUS_WEIGHT: f32 = 2.;
const STATUS_MIN: Pixels = px(300.);
const REASON: &str = "Reason";

type FluxRow = SnapshotRow<FluxObject>;

pub struct FluxListScreen {
    cluster: Option<ClusterId>,
    cluster_name: String,
    page: Page,
    shown: usize,
    flux: Rc<SnapshotRows<FluxObject>>,
    objects: Vec<usize>,
    table: Entity<Table<FluxRow>>,
    filter: Entity<FilterBar>,
}

impl EventEmitter<ScreenEvent> for FluxListScreen {}

impl FluxListScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let screen = cx.weak_entity();
        let table = cx.new(|cx| {
            Table::new("flux-list", columns(), cx)
                .click_opens()
                .row_actions(move |object: &FluxRow, _: &[&FluxRow]| row_items(object, &screen))
                .row_key(|o: &FluxRow| format!("{}/{}", o.key.namespace, o.key.name).into())
                .type_ahead(|o: &FluxRow| o.key.name.clone().into())
        });
        let filter = cx.new(|cx| FilterBar::new("flux-filter", window, cx));
        cx.subscribe(&filter, |this, _, FilterEvent::Changed, cx| {
            this.refresh_rows(cx);
        })
        .detach();
        cx.subscribe(&table, |_, table, event: &TableEvent, cx| {
            if let TableEvent::Opened(ix) = *event
                && let Some(object) = table.read(cx).row(ix)
            {
                let open = Command::OpenFluxObject(object.key.clone());
                cx.emit(ScreenEvent::Command(open));
            }
        })
        .detach();
        Self {
            cluster: None,
            cluster_name: String::new(),
            page: Page::Kustomizations,
            shown: 0,
            flux: SnapshotRows::empty(),
            objects: Vec::new(),
            table,
            filter,
        }
    }

    pub fn list_focus(&self, cx: &App) -> gpui::FocusHandle {
        gpui::Focusable::focus_handle(&self.table, cx)
    }

    pub fn filter_bar(&self) -> &Entity<FilterBar> {
        &self.filter
    }

    pub fn set(
        &mut self,
        cluster: &ClusterSummary,
        page: Page,
        snapshot: &Arc<Snapshot>,
        cx: &mut Context<Self>,
    ) {
        self.cluster_name.clone_from(&cluster.name);
        let changed = self.page != page || self.cluster.as_ref() != Some(&cluster.id);
        self.page = page;
        self.cluster = Some(cluster.id.clone());
        let cluster = &cluster.id;
        let scope = format!("{}/{}", cluster.as_str(), page.label());
        self.table
            .update(cx, |table, cx| table.set_scope(scope, cx));
        if !changed && self.flux.from(snapshot) {
            return;
        }
        self.flux = SnapshotRows::new(snapshot, |snapshot| &snapshot.flux);
        self.objects = snapshot
            .flux
            .iter()
            .enumerate()
            .filter(|(_, o)| &o.key.cluster == cluster && o.key.kind.page() == page)
            .map(|(ix, _)| ix)
            .collect();
        self.refresh(cx);
    }

    pub fn header_note(&self) -> String {
        plural(self.objects.len(), "object", "objects")
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let flux = self.flux.items();
        let labels = label_values(
            self.objects
                .iter()
                .filter_map(|&ix| flux.get(ix))
                .map(|object| &object.labels),
        );
        self.filter.update(cx, |bar, cx| bar.set_labels(labels, cx));
        self.refresh_rows(cx);
    }

    fn refresh_rows(&mut self, cx: &mut Context<Self>) {
        let (query, chip) = {
            let bar = self.filter.read(cx);
            (bar.query().clone(), bar.chip())
        };
        let base: Vec<(usize, &FluxObject)> = self
            .flux
            .matched_items(&query)
            .into_iter()
            .filter(|(ix, _)| self.objects.binary_search(ix).is_ok())
            .collect();
        let chips = StatusFilter::counts(base.iter().map(|(_, o)| o.status()))
            .into_iter()
            .map(|(filter, n)| chip_label(filter.label(), n))
            .collect();
        let chip = StatusFilter::at(chip);
        let kept = base
            .into_iter()
            .filter(|(_, o)| chip.keeps(o.status()))
            .map(|(ix, _)| ix);
        let rows = self.flux.rows(kept);
        let shown = rows.len();
        self.shown = shown;
        let total = self.objects.len();
        self.filter.update(cx, |bar, cx| {
            bar.set_chips(chips, cx);
            bar.set_matches(shown, total, cx);
        });
        let words = query.words;
        self.table.update(cx, |table, cx| {
            table.set_highlight(words, cx);
            table.set_rows(rows, cx);
        });
        cx.notify();
    }
}

impl Render for FluxListScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shape = ListShape {
            shown: self.shown,
            total: self.objects.len(),
            namespace: None,
            noun: Noun::of(self.page),
            cluster: &self.cluster_name,
        };
        let body: AnyElement = match empty_list(&shape, &self.filter, None, cx) {
            Some(empty) => empty.into_any_element(),
            None => self.table.clone().into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.filter.clone())
            .child(div().flex_1().min_h_0().child(body))
    }
}

fn row_items(object: &FluxObject, screen: &WeakEntity<FluxListScreen>) -> Vec<MenuItem> {
    let emit = |command: Command| {
        let screen = screen.clone();
        move |_: &mut Window, cx: &mut App| {
            let command = command.clone();
            let _ = screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)));
        }
    };
    let key = object.key.clone();
    let uid = object.uid.clone();
    let reconcile = Command::Flux(key.clone(), uid.clone(), FluxAction::Reconcile);
    let suspend = if object.phase.is_suspended() {
        MenuItem::new(
            "Resume",
            emit(Command::Flux(key.clone(), uid, FluxAction::Resume)),
        )
        .description("Let Flux apply changes here again, starting with a reconcile now.")
    } else {
        MenuItem::new(
            "Suspend",
            emit(Command::Flux(key.clone(), uid, FluxAction::Suspend)),
        )
        .description("Stop Flux applying changes here until resumed.")
    };
    let open_screen = screen.clone();
    let open = new_tab_item(Command::OpenFluxObject(key.clone()), move |command, cx| {
        let _ = open_screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)));
    });
    let edit_screen = screen.clone();
    let yaml_item = edit_item(key.resource(), move |command, cx| {
        let _ = edit_screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)));
    });
    let mut items = vec![
        open,
        yaml_item,
        MenuItem::new("Reconcile", emit(reconcile))
            .description("Ask Flux to apply the current revision now."),
        suspend,
    ];
    if let Some(source) = object.source.clone() {
        let description = format!(
            "{} {}: URL, ref and last fetch.",
            source.kind.label(),
            source.name
        );
        items.push(
            MenuItem::new("Open source", emit(Command::OpenFluxObject(source)))
                .description(description),
        );
    }
    items
}

fn columns() -> Vec<Column<FluxRow>> {
    vec![
        Column::status_text(
            "Name",
            |o: &FluxRow| o.status(),
            |o: &FluxRow| o.key.name.clone().into(),
        )
        .mono()
        .highlight()
        .flex(NAME_WEIGHT, NAME_MIN)
        .sort_by_key(|o: &FluxRow| o.key.name.clone()),
        Column::text("Namespace", |o: &FluxRow| o.key.namespace.clone().into())
            .mono()
            .fixed(NAMESPACE_WIDTH)
            .sort_by_key(|o: &FluxRow| o.key.namespace.clone()),
        Column::text("Kind", |o: &FluxRow| o.key.kind.label().into())
            .fixed(KIND_WIDTH)
            .sort_by_key(|o: &FluxRow| o.key.kind.label()),
        Column::text("Status", |o: &FluxRow| o.phase.label().into())
            .flex(STATUS_WEIGHT, STATUS_MIN),
        Column::new(REASON, |o: &FluxRow, palette| {
            reason_cell(o.status(), o.current_diagnosis(), palette)
        })
        .tooltip(|o: &FluxRow| reason_tooltip(o.current_diagnosis()))
        .flex(REASON_WEIGHT, REASON_MIN)
        .sort_by_key(|o: &FluxRow| o.current_diagnosis().map(|d| d.reason.clone())),
    ]
}
