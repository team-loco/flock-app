use flock_core::crd::Definition;
use flock_core::emptiness::Noun;
use flock_core::{ClusterId, ClusterSummary};
use gpui::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, FocusHandle, IntoElement,
    ParentElement, Pixels, Render, Styled, Subscription, Window, div, px,
};

use super::{ListShape, ScreenEvent, empty_list};
use crate::kit::{Column, FilterBar, FilterEvent, Table, TableEvent};
use crate::theme::{FONT_MONO, Palette, RADIUS_SMALL, RULE, SPACE_2, SPACE_4, TEXT_LABEL};

const TABLE_ID: &str = "crds";
const FILTER_ID: &str = "crds-filter";
const SINGULAR: &str = "CustomResourceDefinition";
const PLURAL: &str = "CustomResourceDefinitions";
const KIND_WEIGHT: f32 = 1.2;
const KIND_MIN: Pixels = px(200.);
const GROUP_WEIGHT: f32 = 1.4;
const GROUP_MIN: Pixels = px(220.);
const VERSION_WIDTH: Pixels = px(88.);
const SCOPE_WIDTH: Pixels = px(96.);
const OBJECTS_WIDTH: Pixels = px(64.);

pub struct DefinitionsScreen {
    cluster: Option<ClusterId>,
    cluster_name: String,
    rows: Vec<Definition>,
    shown: usize,
    table: Entity<Table<Definition>>,
    filter: Entity<FilterBar>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ScreenEvent> for DefinitionsScreen {}

impl DefinitionsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let table = cx.new(|cx| {
            Table::new(TABLE_ID, columns(), cx)
                .row_key(|d: &Definition| d.name().into())
                .click_opens()
        });
        let filter = cx.new(|cx| FilterBar::new(FILTER_ID, window, cx));
        let subscriptions = vec![
            cx.subscribe(&table, |this, table, event: &TableEvent, cx| {
                let TableEvent::Opened(ix) = *event else {
                    return;
                };
                let definition = table.read(cx).row(ix).cloned();
                if let Some((definition, cluster)) = definition.zip(this.cluster.clone()) {
                    let command = definition.command(cluster);
                    cx.emit(ScreenEvent::Command(command));
                }
            }),
            cx.subscribe(&filter, |this, _, FilterEvent::Changed, cx| {
                this.refresh(cx);
            }),
        ];
        Self {
            cluster: None,
            cluster_name: String::new(),
            rows: Vec::new(),
            shown: 0,
            table,
            filter,
            _subscriptions: subscriptions,
        }
    }

    pub fn filter_bar(&self) -> &Entity<FilterBar> {
        &self.filter
    }

    pub fn list_focus(&self, cx: &App) -> FocusHandle {
        gpui::Focusable::focus_handle(&self.table, cx)
    }

    pub fn header_note(&self) -> String {
        self.rows.len().to_string()
    }

    pub fn set(
        &mut self,
        cluster: &ClusterSummary,
        definitions: Vec<Definition>,
        cx: &mut Context<Self>,
    ) {
        self.cluster = Some(cluster.id.clone());
        self.cluster_name.clone_from(&cluster.name);
        let scope = cluster.id.as_str().to_owned();
        self.table
            .update(cx, |table, cx| table.set_scope(scope, cx));
        if self.rows == definitions {
            return;
        }
        self.rows = definitions;
        self.refresh(cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let query = self.filter.read(cx).query().clone();
        let rows: Vec<Definition> = self
            .rows
            .iter()
            .filter(|definition| definition.matches(&query))
            .cloned()
            .collect();
        let shown = rows.len();
        self.shown = shown;
        let total = self.rows.len();
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

impl Render for DefinitionsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shape = ListShape {
            shown: self.shown,
            total: self.rows.len(),
            namespace: None,
            noun: Noun::named(SINGULAR, PLURAL),
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

fn kind_cell(definition: &Definition, palette: &Palette) -> AnyElement {
    let tag = definition.tag().map(|tag| {
        div()
            .flex_none()
            .px(SPACE_2)
            .rounded(RADIUS_SMALL)
            .border(RULE)
            .border_color(palette.line)
            .font_family(FONT_MONO)
            .text_size(TEXT_LABEL)
            .text_color(palette.mute)
            .child(tag)
    });
    div()
        .flex()
        .items_center()
        .gap(SPACE_4)
        .min_w_0()
        .child(div().truncate().child(definition.key.kind.clone()))
        .children(tag)
        .into_any_element()
}

fn columns() -> Vec<Column<Definition>> {
    vec![
        Column::new("Kind", kind_cell)
            .fit_by(|d: &Definition| d.key.kind.clone().into(), Pixels::ZERO)
            .flex(KIND_WEIGHT, KIND_MIN)
            .sort_by_key(|d: &Definition| d.key.kind.clone()),
        Column::colored_text(
            "Group",
            |d: &Definition| d.key.group.clone().into(),
            |_, p| p.fg2,
        )
        .highlight()
        .mono()
        .flex(GROUP_WEIGHT, GROUP_MIN)
        .sort_by_key(|d: &Definition| d.key.group.clone()),
        Column::colored_text(
            "Version",
            |d: &Definition| d.version.clone().into(),
            |_, p| p.fg2,
        )
        .mono()
        .fixed(VERSION_WIDTH)
        .sort_by_key(|d: &Definition| d.version.clone()),
        Column::colored_text("Scope", |d: &Definition| d.scope().into(), |_, p| p.fg2)
            .fixed(SCOPE_WIDTH)
            .sort_by_key(|d: &Definition| d.namespaced),
        Column::text("Objects", |d: &Definition| {
            d.objects.map(|n| n.to_string()).unwrap_or_default().into()
        })
        .mono()
        .end()
        .fixed(OBJECTS_WIDTH)
        .sort_by_key(|d: &Definition| d.objects),
    ]
}
