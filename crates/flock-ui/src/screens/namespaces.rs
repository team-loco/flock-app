use flock_core::cluster::Namespace;
use flock_core::emptiness::Noun;
use flock_core::filter::{StatusFilter, label_values};
use flock_core::related::NAMESPACE;
use flock_core::time::age;
use flock_core::{
    ClusterDetail, ClusterId, ClusterSummary, Command, Counts, Page, ResourceRef, Status,
};
use gpui::{
    AnyElement, AppContext, Context, Entity, EventEmitter, IntoElement, ParentElement, Pixels,
    Render, Styled, Window, div, px,
};

use super::{ListShape, ScreenEvent, empty_list};
use crate::kit::status_symbols;
use crate::kit::{Column, FilterBar, FilterEvent, StatusGlyph, Table, TableEvent, chip_label};
use crate::theme::{ActiveTheme, FONT_MONO, GLYPH_SLOT, Palette, SPACE_4};

const NAME_WEIGHT: f32 = 1.4;
const NAME_MIN: Pixels = px(200.);
const PROBLEMS_WEIGHT: f32 = 1.;
const PROBLEMS_MIN: Pixels = px(96.);
const PODS_WIDTH: Pixels = px(52.);
const CPU_WIDTH: Pixels = px(84.);
const MEMORY_WIDTH: Pixels = px(84.);
const AGE_WIDTH: Pixels = px(44.);

fn namespace_ref((name, cluster): (String, ClusterId)) -> ResourceRef {
    ResourceRef {
        cluster,
        kind: NAMESPACE.to_owned(),
        namespace: None,
        name,
    }
}

pub struct NamespacesScreen {
    cluster: Option<ClusterId>,
    cluster_name: String,
    shown: usize,
    namespaces: Vec<Namespace>,
    table: Entity<Table<Namespace>>,
    filter: Entity<FilterBar>,
}

impl EventEmitter<ScreenEvent> for NamespacesScreen {}

impl NamespacesScreen {
    pub fn list_focus(&self, cx: &gpui::App) -> gpui::FocusHandle {
        gpui::Focusable::focus_handle(&self.table, cx)
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let table = cx.new(|cx| {
            Table::new("namespaces", columns(), cx).row_key(|n: &Namespace| n.name.clone().into())
        });
        cx.subscribe(&table, |this, table, event: &TableEvent, cx| {
            let TableEvent::Opened(ix) = *event else {
                return;
            };
            let name = table.read(cx).row(ix).map(|n| n.name.clone());
            if let Some(target) = name.zip(this.cluster.clone()).map(namespace_ref) {
                cx.emit(ScreenEvent::Command(Command::OpenResource(target)));
            }
        })
        .detach();
        let filter = cx.new(|cx| FilterBar::new("namespace-filter", window, cx));
        cx.subscribe(&filter, |this, _, FilterEvent::Changed, cx| {
            this.refresh(cx);
        })
        .detach();
        Self {
            cluster: None,
            cluster_name: String::new(),
            shown: 0,
            namespaces: Vec::new(),
            table,
            filter,
        }
    }

    pub fn filter_bar(&self) -> &Entity<FilterBar> {
        &self.filter
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let labels = label_values(self.namespaces.iter().map(|namespace| &namespace.labels));
        self.filter.update(cx, |bar, cx| bar.set_labels(labels, cx));
        let (query, chip) = {
            let bar = self.filter.read(cx);
            (bar.query().clone(), bar.chip())
        };
        let base: Vec<&Namespace> = self
            .namespaces
            .iter()
            .filter(|n| n.matches(&query))
            .collect();
        let chips = StatusFilter::counts(base.iter().map(|n| n.status))
            .into_iter()
            .map(|(filter, n)| chip_label(filter.label(), n))
            .collect();
        let chip = StatusFilter::at(chip);
        let rows: Vec<Namespace> = base
            .into_iter()
            .filter(|n| chip.keeps(n.status))
            .cloned()
            .collect();
        let shown = rows.len();
        self.shown = shown;
        let total = self.namespaces.len();
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

    pub fn set(
        &mut self,
        cluster: &ClusterSummary,
        detail: &ClusterDetail,
        cx: &mut Context<Self>,
    ) {
        self.cluster = Some(cluster.id.clone());
        self.cluster_name.clone_from(&cluster.name);
        let scope = cluster.id.as_str().to_owned();
        self.table
            .update(cx, |table, cx| table.set_scope(scope, cx));
        if self.namespaces == detail.namespaces {
            return;
        }
        self.namespaces.clone_from(&detail.namespaces);
        self.refresh(cx);
    }

    pub fn header_note(&self) -> (Counts, String) {
        let counts = Counts::of(self.namespaces.iter().map(|n| n.status));
        let text = format!("{} with problems", counts.flagged());
        (counts, text)
    }
}

impl Render for NamespacesScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shape = ListShape {
            shown: self.shown,
            total: self.namespaces.len(),
            namespace: None,
            noun: Noun::of(Page::Namespaces),
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

fn problems(namespace: &Namespace) -> AnyElement {
    let badges: Vec<AnyElement> = namespace
        .problems
        .badges()
        .into_iter()
        .map(|(status, n)| ProblemBadge { status, n }.into_any_element())
        .collect();
    div()
        .flex()
        .gap(SPACE_4)
        .children(badges)
        .into_any_element()
}

#[derive(gpui::IntoElement)]
struct ProblemBadge {
    status: Status,
    n: u32,
}

impl gpui::RenderOnce for ProblemBadge {
    fn render(self, _: &mut Window, cx: &mut gpui::App) -> impl IntoElement {
        div()
            .font_family(FONT_MONO)
            .text_color(cx.palette().status(self.status))
            .child(status_symbols(format!(
                "{} {}",
                self.status.glyph(),
                self.n
            )))
    }
}

fn name_cell(namespace: &Namespace, palette: &Palette) -> AnyElement {
    let phase = namespace.phase_note().map(|phase| {
        div()
            .flex_none()
            .text_color(palette.warn)
            .child(phase.to_owned())
    });
    div()
        .flex()
        .items_center()
        .gap(SPACE_4)
        .min_w_0()
        .child(StatusGlyph::new(namespace.status))
        .child(div().truncate().child(namespace.name.clone()))
        .children(phase)
        .into_any_element()
}

fn columns() -> Vec<Column<Namespace>> {
    vec![
        Column::new("Name", name_cell)
            .fit_by(|n: &Namespace| n.name.clone().into(), GLYPH_SLOT + SPACE_4)
            .mono()
            .flex(NAME_WEIGHT, NAME_MIN)
            .sort_by_key(|n: &Namespace| n.name.clone()),
        Column::new("Problems", |n: &Namespace, _| problems(n))
            .flex(PROBLEMS_WEIGHT, PROBLEMS_MIN)
            .sort_by_key(|n: &Namespace| std::cmp::Reverse(n.status)),
        Column::text("Pods", |n: &Namespace| n.pods.to_string().into())
            .mono()
            .end()
            .fixed(PODS_WIDTH)
            .sort_by_key(|n: &Namespace| n.pods),
        Column::colored_text("CPU", |n: &Namespace| n.cpu_label().into(), |_, p| p.fg2)
            .mono()
            .end()
            .fixed(CPU_WIDTH)
            .tooltip(|n: &Namespace| n.cpu_note().into())
            .sortable(|a: &Namespace, b: &Namespace| {
                a.cpu
                    .unwrap_or_default()
                    .total_cmp(&b.cpu.unwrap_or_default())
            }),
        Column::colored_text(
            "Memory",
            |n: &Namespace| n.memory_label().into(),
            |_, p| p.fg2,
        )
        .mono()
        .end()
        .fixed(MEMORY_WIDTH)
        .tooltip(|n: &Namespace| n.memory_note().into())
        .sortable(|a: &Namespace, b: &Namespace| {
            a.memory
                .unwrap_or_default()
                .total_cmp(&b.memory.unwrap_or_default())
        }),
        Column::colored_text("Age", |n: &Namespace| age(n.age).into(), |_, p| p.mute)
            .mono()
            .fixed(AGE_WIDTH)
            .sort_by_key(|n: &Namespace| n.age),
    ]
}
