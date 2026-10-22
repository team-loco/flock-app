use flock_core::emptiness::Noun;
use flock_core::home::{Problem, Severity, group_problems, severity_counts};
use flock_core::issue::Issue;
use flock_core::time::age;
use flock_core::{ClusterId, ClusterSummary, Command, Snapshot};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FontWeight, Hsla, IntoElement,
    ParentElement, Pixels, Render, SharedString, Styled, Window, div, px,
};

use crate::kit::{
    Button, Column, EmptyState, FilterBar, FilterEvent, Inspector, KeyValue, PanelSection,
    StatusGlyph, Table, TableEvent, chip_label, status_symbols,
};
use crate::screens::{ListShape, empty_list};
use crate::theme::{
    FONT_MONO, INSPECTOR, Palette, ROW_TABLE, SPACE_3, SPACE_5, SPACE_8, TEXT_BODY, TEXT_SMALL,
};

pub const INSPECTOR_ID: &str = "fleet-problem";
const NOTHING: &str = "Nothing needs attention.";
const OPEN: &str = "Open";
const DETAILS: &str = "Details";
const NONE: &str = "—";
const CLUSTER_KIND: &str = "Cluster";
const SINCE_SUFFIX: &str = " ago";
const NOUN_ONE: &str = "problem";
const NOUN_MANY: &str = "problems";

const GLYPH: Pixels = px(14.);
const REASON: Pixels = px(190.);
const OBJECT_MIN: Pixels = px(160.);
const OBJECT_WEIGHT: f32 = 1.;
const MESSAGE_MIN: Pixels = px(200.);
const MESSAGE_WEIGHT: f32 = 1.6;
const AGE: Pixels = px(40.);

#[derive(Clone)]
enum Row {
    Group { cluster: SharedString, count: usize },
    Item(Problem),
}

pub struct FleetProblemsScreen {
    problems: Vec<Problem>,
    clusters: Vec<ClusterSummary>,
    shown: usize,
    selected: Option<String>,
    filter: Entity<FilterBar>,
    table: Entity<Table<Row>>,
}

impl EventEmitter<Command> for FleetProblemsScreen {}

impl FleetProblemsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let table = cx.new(|cx| {
            Table::new("fleet-problems", columns(), cx)
                .row_key(row_key)
                .row_height(ROW_TABLE)
                .span_rows(group_row)
                .fit_content()
        });
        cx.subscribe(&table, |this, table, event: &TableEvent, cx| {
            let (TableEvent::Selected(ix) | TableEvent::Opened(ix)) = *event else {
                return;
            };
            let Some(Row::Item(problem)) = table.read(cx).row(ix).cloned() else {
                return;
            };
            this.selected = Some(key(&problem));
            if matches!(event, TableEvent::Opened(_)) {
                cx.emit(open(&problem));
            }
            cx.notify();
        })
        .detach();
        let placeholder = Problem::FILTER.placeholder();
        let filter = cx.new(|cx| FilterBar::new("fleet-problem-filter", placeholder, window, cx));
        cx.subscribe(&filter, |this, _, FilterEvent::Changed, cx| {
            this.refresh(cx);
        })
        .detach();
        Self {
            problems: Vec::new(),
            clusters: Vec::new(),
            shown: 0,
            selected: None,
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
        self.problems.clone_from(&snapshot.home.problems);
        self.clusters.clone_from(&snapshot.clusters);
        self.refresh(cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let (query, chip) = {
            let bar = self.filter.read(cx);
            (bar.query().clone(), bar.chip())
        };
        let chips = severity_counts(&self.problems, &query)
            .into_iter()
            .map(|(severity, n)| chip_label(severity.label(), n))
            .collect();
        let severity = Severity::ALL.get(chip).copied().unwrap_or_default();
        let rows = rows(&self.problems, severity, &query, &self.clusters);
        let items: Vec<&Problem> = rows
            .iter()
            .filter_map(|row| match row {
                Row::Item(problem) => Some(problem),
                Row::Group { .. } => None,
            })
            .collect();
        self.shown = items.len();
        let kept = self
            .selected
            .as_ref()
            .is_some_and(|selected| items.iter().any(|p| &key(p) == selected));
        if !kept {
            self.selected = items.first().map(|p| key(p));
        }
        let position = self.selected.as_ref().and_then(|selected| {
            rows.iter()
                .position(|row| matches!(row, Row::Item(p) if &key(p) == selected))
        });
        let shown = self.shown;
        let total = self.problems.len();
        self.filter.update(cx, |bar, cx| {
            bar.set_chips(chips, cx);
            bar.set_matches(shown, total, cx);
        });
        let words = query.words;
        self.table.update(cx, |table, cx| {
            table.set_highlight(words, cx);
            table.set_rows(rows, cx);
            table.select(position, cx);
        });
        cx.notify();
    }

    fn current(&self) -> Option<&Problem> {
        let selected = self.selected.as_ref()?;
        self.problems.iter().find(|p| &key(p) == selected)
    }

    fn cluster_name(&self, id: &ClusterId) -> String {
        cluster_name(&self.clusters, id)
    }

    fn inspector(&self, problem: &Problem, cx: &mut Context<Self>) -> AnyElement {
        let command = open(problem);
        let open = Button::new("fleet-problem-open", OPEN)
            .primary()
            .small()
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(command.clone())));
        let target = &problem.target;
        let issue = Issue::new(
            problem.status,
            problem.reason.clone(),
            problem.message.clone(),
            problem.condition.clone(),
        );
        let since = format!("{}{SINCE_SUFFIX}", age(problem.age));
        let details = KeyValue::new()
            .sans()
            .text("cluster", self.cluster_name(&target.cluster))
            .text("kind", target.kind.clone())
            .text(
                "namespace",
                target.namespace.clone().unwrap_or_else(|| NONE.to_owned()),
            )
            .text("name", target.name.clone())
            .text("since", since);
        Inspector::new(INSPECTOR_ID, format!("{} {target}", target.kind))
            .mono()
            .actions(open)
            .issues([issue])
            .panel(PanelSection::new(INSPECTOR_ID, DETAILS).child(details))
            .panel(PanelSection::events(INSPECTOR_ID, problem.events.clone()))
            .width(INSPECTOR)
            .into_any_element()
    }
}

impl Render for FleetProblemsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.problems.is_empty() {
            return div()
                .size_full()
                .child(EmptyState::new(NOTHING).selector("fleet-problems-empty"))
                .into_any_element();
        }
        let shape = ListShape {
            shown: self.shown,
            total: self.problems.len(),
            namespace: None,
            noun: Noun::named(NOUN_ONE, NOUN_MANY),
            cluster: "",
        };
        let body = match empty_list(&shape, &self.filter, None, cx) {
            Some(empty) => empty.into_any_element(),
            None => self.table.clone().into_any_element(),
        };
        let current = self.current().cloned();
        let inspector = current.map(|problem| self.inspector(&problem, cx));
        div()
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
            .into_any_element()
    }
}

fn open(problem: &Problem) -> Command {
    let target = &problem.target;
    if target.kind == CLUSTER_KIND {
        return Command::SelectCluster(Some(target.cluster.clone()));
    }
    Command::OpenResource(target.clone())
}

fn key(problem: &Problem) -> String {
    let target = &problem.target;
    format!(
        "{}/{}/{target}/{}",
        target.cluster, target.kind, problem.reason
    )
}

fn row_key(row: &Row) -> SharedString {
    match row {
        Row::Group { cluster, .. } => format!("cluster:{cluster}").into(),
        Row::Item(problem) => key(problem).into(),
    }
}

fn cluster_name(clusters: &[ClusterSummary], id: &ClusterId) -> String {
    clusters
        .iter()
        .find(|c| &c.id == id)
        .map_or_else(|| id.to_string(), |c| c.name.clone())
}

fn rows(
    problems: &[Problem],
    severity: Severity,
    query: &flock_core::filter::TextFilter,
    clusters: &[ClusterSummary],
) -> Vec<Row> {
    group_problems(problems, severity, query)
        .into_iter()
        .flat_map(|group| {
            let head = Row::Group {
                cluster: cluster_name(clusters, group.cluster).into(),
                count: group.problems.len(),
            };
            let items = group.problems.into_iter().cloned().map(Row::Item);
            std::iter::once(head).chain(items)
        })
        .collect()
}

fn group_row(row: &Row, palette: &Palette) -> Option<AnyElement> {
    let Row::Group { cluster, count } = row else {
        return None;
    };
    let element = div()
        .flex()
        .items_center()
        .gap(SPACE_5)
        .size_full()
        .px(SPACE_8)
        .bg(palette.panel)
        .text_size(TEXT_BODY)
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(cluster.clone()),
        )
        .child(
            div()
                .font_family(FONT_MONO)
                .text_size(TEXT_SMALL)
                .text_color(palette.mute)
                .child(count.to_string()),
        )
        .into_any_element();
    Some(element)
}

fn text(value: impl Into<SharedString>, color: Hsla) -> AnyElement {
    div()
        .truncate()
        .text_color(color)
        .child(status_symbols(value))
        .into_any_element()
}

fn object(problem: &Problem, palette: &Palette) -> AnyElement {
    let target = &problem.target;
    div()
        .flex()
        .min_w_0()
        .gap(SPACE_3)
        .child(
            div()
                .flex_none()
                .text_color(palette.mute)
                .child(target.kind.clone()),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(palette.fg)
                .child(target.to_string()),
        )
        .into_any_element()
}

fn row_message(row: &Row) -> SharedString {
    match row {
        Row::Item(p) => p.message.clone().into(),
        Row::Group { .. } => SharedString::default(),
    }
}

fn columns() -> Vec<Column<Row>> {
    vec![
        Column::new("", |row: &Row, _| match row {
            Row::Item(p) => StatusGlyph::new(p.status).into_any_element(),
            Row::Group { .. } => div().into_any_element(),
        })
        .fixed(GLYPH),
        Column::new("Reason", |row: &Row, palette| match row {
            Row::Item(p) => text(p.reason.clone(), palette.status(p.status)),
            Row::Group { .. } => div().into_any_element(),
        })
        .mono()
        .fixed(REASON),
        Column::new("Object", |row: &Row, palette| match row {
            Row::Item(p) => object(p, palette),
            Row::Group { .. } => div().into_any_element(),
        })
        .mono()
        .flex(OBJECT_WEIGHT, OBJECT_MIN),
        Column::colored_text("Message", row_message, |_, p| p.fg2)
            .highlight()
            .tooltip(row_message)
            .flex(MESSAGE_WEIGHT, MESSAGE_MIN),
        Column::new("Age", |row: &Row, palette| match row {
            Row::Item(p) => text(age(p.age), palette.mute),
            Row::Group { .. } => div().into_any_element(),
        })
        .mono()
        .end()
        .fixed(AGE),
    ]
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use flock_core::filter::TextFilter;
    use flock_core::{ResourceRef, Status};

    use super::*;

    fn problem(cluster: &str, status: Status, kind: &str, name: &str) -> Problem {
        Problem {
            status,
            target: ResourceRef {
                cluster: ClusterId::new(cluster),
                kind: kind.into(),
                namespace: None,
                name: name.into(),
            },
            reason: "Unauthorized".into(),
            message: String::new(),
            condition: String::new(),
            age: Duration::ZERO,
            events: Vec::new(),
        }
    }

    #[test]
    fn groups_show_a_plain_count_and_clusters_open_themselves() {
        let problems = [
            problem("a", Status::Err, "Pod", "x"),
            problem("a", Status::Warn, "Pod", "y"),
            problem("b", Status::Err, CLUSTER_KIND, "b"),
        ];
        let shape: Vec<(bool, usize)> = rows(&problems, Severity::All, &TextFilter::default(), &[])
            .iter()
            .map(|row| match row {
                Row::Group { count, .. } => (true, *count),
                Row::Item(_) => (false, 0),
            })
            .collect();
        assert_eq!(
            shape,
            [(true, 2), (false, 0), (false, 0), (true, 1), (false, 0)]
        );
        assert_eq!(
            open(&problems[2]),
            Command::SelectCluster(Some(ClusterId::new("b")))
        );
        assert_eq!(
            open(&problems[0]),
            Command::OpenResource(problems[0].target.clone())
        );
    }
}
