use flock_core::changes::{ChangesLog, Commit, Lane, outcome_label};
use flock_core::filter::TextFilter;
use flock_core::home::{Problem, Severity, group_problems, problem_counts};
use flock_core::{
    ClusterId, ClusterSummary, Command, Connection, Counts, FlockHome, Page, Snapshot, Status,
};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FontWeight, Hsla, IntoElement,
    ParentElement, Pixels, Render, SharedString, Styled, Subscription, Window, div,
    prelude::FluentBuilder, px, relative,
};

use crate::kit::status_symbols;
use crate::kit::{
    Column, EmptyState, Link, RawMessage, SectionHeader, Segmented, Skeleton, StatusGlyph, Table,
    TableEvent,
};
use crate::screens::new_tab_item;
use crate::shell::switcher::{Run, cluster_items};
use crate::theme::{
    ActiveTheme, FONT_MONO, METER_MINI, METER_MINI_WIDTH, Palette, ROW_TABLE, ROW_TABLE_MAX, RULE,
    SPACE_3, SPACE_4, SPACE_5, SPACE_6, SPACE_7, SPACE_8, TEXT_BODY, TEXT_CONTROL, TEXT_SMALL,
};

const CLUSTER_NAME_MIN: Pixels = px(140.);
const CLUSTER_STATE_MIN: Pixels = px(96.);
const CLUSTER_STATE_MAX: Pixels = px(200.);
const CLUSTER_VERSION_MIN: Pixels = px(64.);
const CLUSTER_VERSION_MAX: Pixels = px(140.);
const CLUSTER_NODES_MIN: Pixels = px(48.);
const CLUSTER_NODES_MAX: Pixels = px(72.);
const CLUSTER_PODS_MIN: Pixels = px(56.);
const CLUSTER_PODS_MAX: Pixels = px(140.);
const CLUSTER_USAGE: Pixels = px(92.);
const CLUSTER_FLUX_MIN: Pixels = px(56.);
const CLUSTER_FLUX_MAX: Pixels = px(140.);
const CLUSTER_REVISION_MIN: Pixels = px(64.);
const CLUSTER_REVISION_MAX: Pixels = px(110.);
const CLUSTER_PROBLEMS_MIN: Pixels = px(72.);
const CLUSTER_PROBLEMS_MAX: Pixels = px(200.);
const BADGE_GAP: &str = "  ";
const CLUSTER_NAME_WEIGHT: f32 = 1.3;

const PROBLEM_GLYPH: Pixels = px(14.);
const PROBLEM_KIND: Pixels = px(104.);
const PROBLEM_OBJECT_MIN: Pixels = px(220.);
const PROBLEM_OBJECT_WEIGHT: f32 = 1.;
const PROBLEM_MESSAGE_MIN: Pixels = px(280.);
const PROBLEM_MESSAGE_WEIGHT: f32 = 3.;
const PROBLEM_AGE: Pixels = px(44.);

const COMMIT_SHA: Pixels = px(72.);
const COMMIT_MESSAGE_MIN: Pixels = px(260.);
const COMMIT_AUTHOR: Pixels = px(120.);
const COMMIT_AGE: Pixels = px(40.);
const COMMIT_LANE: Pixels = px(74.);

const PERCENT_MAX: f32 = 100.;
const MISSING: &str = "—";
const NOT_TARGETED: &str = "–";
const ALL_CHANGES: &str = "All changes →";
const RECENT_COMMITS: usize = 3;

#[derive(Clone)]
enum ProblemRow {
    Group {
        cluster: SharedString,
        counts: Counts,
    },
    Item(Problem),
}

pub struct FlockScreen {
    home: FlockHome,
    changes: ChangesLog,
    clusters: Vec<ClusterSummary>,
    lanes: Vec<Lane>,
    severity: Severity,
    cluster_table: Entity<Table<ClusterSummary>>,
    problem_table: Entity<Table<ProblemRow>>,
    commit_table: Entity<Table<Commit>>,
    _subscriptions: [Subscription; 3],
}

impl EventEmitter<Command> for FlockScreen {}

impl FlockScreen {
    pub fn list_focus(&self, cx: &gpui::App) -> gpui::FocusHandle {
        gpui::Focusable::focus_handle(&self.cluster_table, cx)
    }

    pub fn new(cx: &mut Context<Self>) -> Self {
        let screen = cx.weak_entity();
        let run: Run = std::rc::Rc::new(move |command, cx| {
            let _ = screen.update(cx, |_, cx| cx.emit(command));
        });
        let cluster_table = cx.new(|cx| {
            Table::new("flock-clusters", cluster_columns(), cx)
                .row_actions(move |cluster: &ClusterSummary| {
                    let open_run = run.clone();
                    let open = Command::SelectCluster(Some(cluster.id.clone()));
                    let mut items = vec![new_tab_item(open, move |command, cx| {
                        open_run(command, cx);
                    })];
                    items.extend(cluster_items(cluster, &run));
                    items
                })
                .type_ahead(|c: &ClusterSummary| c.name.clone().into())
                .row_height(ROW_TABLE_MAX)
                .column_gap(SPACE_7)
                .click_opens()
                .fit_content()
        });
        let problem_table = cx.new(|cx| {
            Table::new("flock-problems", problem_columns(), cx)
                .headerless()
                .row_height(ROW_TABLE)
                .inset(SPACE_8 + SPACE_6)
                .span_rows(group_row)
                .click_opens()
                .empty("Nothing needs attention.")
                .fit_content()
        });
        let commit_table = commit_table(&[], cx);
        let subscriptions = [
            cx.subscribe(&cluster_table, |_, table, event, cx| {
                cluster_opened(&table, event, cx);
            }),
            cx.subscribe(&problem_table, |_, table, event, cx| {
                problem_opened(&table, event, cx);
            }),
            cx.subscribe(&commit_table, |_, table, event: &TableEvent, cx| {
                commit_opened(&table, event, cx);
            }),
        ];
        Self {
            home: FlockHome::default(),
            changes: ChangesLog::default(),
            clusters: Vec::new(),
            lanes: Vec::new(),
            severity: Severity::All,
            cluster_table,
            problem_table,
            commit_table,
            _subscriptions: subscriptions,
        }
    }

    pub fn set_snapshot(&mut self, snapshot: &Snapshot, cx: &mut Context<Self>) {
        self.clusters.clone_from(&snapshot.clusters);
        self.home.clone_from(&snapshot.home);
        let clusters = self.clusters.clone();
        self.cluster_table
            .update(cx, |table, cx| table.set_rows(clusters, cx));
        self.changes.clone_from(&snapshot.changes);
        if self.lanes != self.changes.lanes {
            self.lanes.clone_from(&self.changes.lanes);
            let columns = commit_columns(&self.lanes);
            self.commit_table
                .update(cx, |table, cx| table.set_columns(columns, cx));
        }
        let commits = self.changes.recent(RECENT_COMMITS).to_vec();
        self.commit_table
            .update(cx, |table, cx| table.set_rows(commits, cx));
        self.refresh_problems(cx);
    }

    fn set_severity(&mut self, severity: Severity, cx: &mut Context<Self>) {
        self.severity = severity;
        self.refresh_problems(cx);
    }

    fn refresh_problems(&mut self, cx: &mut Context<Self>) {
        let rows = problem_rows(&self.home.problems, self.severity, &self.clusters);
        self.problem_table
            .update(cx, |table, cx| table.set_rows(rows, cx));
        cx.notify();
    }

    fn clusters_section(&self) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .child(SectionHeader::new("Clusters").count(self.clusters.len().to_string()))
            .child(self.cluster_table.clone())
    }

    fn problems_section(&self, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let totals = problem_counts(&self.home.problems);
        let selected = Severity::OVERVIEW
            .iter()
            .position(|s| *s == self.severity)
            .unwrap_or_default();
        let filter = Severity::OVERVIEW
            .iter()
            .fold(Segmented::new("severity"), |seg, s| seg.option(s.label()))
            .selected(selected)
            .on_select(cx.listener(|this, ix: &usize, _, cx| {
                let severity = Severity::OVERVIEW.get(*ix).copied().unwrap_or_default();
                this.set_severity(severity, cx);
            }));
        div()
            .flex()
            .flex_col()
            .child(
                SectionHeader::new("Problems")
                    .annotation(badges(&totals, SPACE_3, palette))
                    .end(filter),
            )
            .child(
                div()
                    .border_t(RULE)
                    .border_color(palette.line)
                    .child(self.problem_table.clone()),
            )
    }

    fn commits_body(&self) -> AnyElement {
        if !self.changes.commits.is_empty() {
            return self.commit_table.clone().into_any_element();
        }
        if self.changes.pending {
            return Skeleton::rows().into_any_element();
        }
        EmptyState::new(self.changes.empty_reason())
            .selector("flock-commits-empty")
            .into_any_element()
    }

    fn changes_section(&self, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let all = Link::new("all-changes", ALL_CHANGES).on_click(cx.listener(|_, _, _, cx| {
            cx.emit(Command::SelectPage(Page::Changes));
        }));
        div()
            .flex()
            .flex_col()
            .pb(SPACE_8)
            .child(
                SectionHeader::new("Recent GitOps changes")
                    .count(self.changes.source_label())
                    .end(div().text_size(TEXT_CONTROL).child(all)),
            )
            .child(
                div()
                    .border_t(RULE)
                    .border_color(palette.line)
                    .child(self.commits_body()),
            )
    }
}

impl Render for FlockScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let problems = self.problems_section(&palette, cx).into_any_element();
        let clusters = self.clusters_section().into_any_element();
        let changes = self.changes_section(&palette, cx);
        div()
            .flex()
            .flex_col()
            .child(clusters)
            .child(problems)
            .child(changes)
    }
}

fn cluster_opened(
    table: &Entity<Table<ClusterSummary>>,
    event: &TableEvent,
    cx: &mut Context<FlockScreen>,
) {
    let TableEvent::Opened(ix) = *event else {
        return;
    };
    let id = table.read(cx).row(ix).map(|c| c.id.clone());
    if let Some(id) = id {
        cx.emit(Command::SelectCluster(Some(id)));
    }
}

fn commit_opened(table: &Entity<Table<Commit>>, event: &TableEvent, cx: &mut Context<FlockScreen>) {
    let TableEvent::Opened(ix) = *event else {
        return;
    };
    let sha = table.read(cx).row(ix).map(|c| c.sha.clone());
    if let Some(sha) = sha {
        cx.emit(Command::OpenCommit(sha));
    }
}

fn problem_opened(
    table: &Entity<Table<ProblemRow>>,
    event: &TableEvent,
    cx: &mut Context<FlockScreen>,
) {
    let TableEvent::Opened(ix) = *event else {
        return;
    };
    let target = match table.read(cx).row(ix) {
        Some(ProblemRow::Item(problem)) => Some(problem.target.clone()),
        Some(ProblemRow::Group { .. }) => None,
        None => None,
    };
    if let Some(target) = target {
        cx.emit(Command::OpenResource(target));
    }
}

fn problem_rows(
    problems: &[Problem],
    severity: Severity,
    clusters: &[ClusterSummary],
) -> Vec<ProblemRow> {
    group_problems(problems, severity, &TextFilter::default())
        .into_iter()
        .flat_map(|group| {
            let name = clusters
                .iter()
                .find(|c| &c.id == group.cluster)
                .map_or_else(|| group.cluster.to_string(), |c| c.name.clone());
            let head = ProblemRow::Group {
                cluster: name.into(),
                counts: group.counts(),
            };
            let items = group.problems.into_iter().cloned().map(ProblemRow::Item);
            std::iter::once(head).chain(items)
        })
        .collect()
}

fn badges(counts: &Counts, gap: Pixels, palette: &Palette) -> impl IntoElement {
    let items = counts.badges().into_iter().map(|(status, n)| {
        div()
            .text_color(palette.status(status))
            .child(status_symbols(format!("{} {n}", status.glyph())))
    });
    div()
        .flex()
        .flex_none()
        .gap(gap)
        .font_family(FONT_MONO)
        .text_size(TEXT_SMALL)
        .whitespace_nowrap()
        .children(items)
}

fn group_row(row: &ProblemRow, palette: &Palette) -> Option<AnyElement> {
    let ProblemRow::Group { cluster, counts } = row else {
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
        .child(badges(counts, SPACE_4, palette))
        .into_any_element();
    Some(element)
}

fn dimmed(cluster: &ClusterSummary, palette: &Palette) -> Hsla {
    if cluster.metrics.is_some() {
        palette.fg
    } else {
        palette.mute
    }
}

fn text(value: impl Into<SharedString>, color: Hsla) -> AnyElement {
    div()
        .truncate()
        .text_color(color)
        .child(status_symbols(value))
        .into_any_element()
}

fn mini_meter(percent: Option<u8>, palette: &Palette) -> AnyElement {
    let fraction = percent.map_or(0., |p| f32::from(p) / PERCENT_MAX);
    let label = percent.map_or_else(|| MISSING.to_owned(), |p| format!("{p}%"));
    div()
        .flex()
        .items_center()
        .gap(SPACE_4)
        .child(
            div()
                .flex_none()
                .w(METER_MINI_WIDTH)
                .h(METER_MINI)
                .bg(palette.line2)
                .child(div().h_full().w(relative(fraction)).bg(palette.fg2)),
        )
        .child(label)
        .into_any_element()
}

fn cluster_columns() -> Vec<Column<ClusterSummary>> {
    vec![
        Column::new("Cluster", |c: &ClusterSummary, palette| {
            div()
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .text_color(dimmed(c, palette))
                .child(c.name.clone())
                .into_any_element()
        })
        .flex(CLUSTER_NAME_WEIGHT, CLUSTER_NAME_MIN),
        Column::new("Connection", |c: &ClusterSummary, palette| {
            let connection = c.connection();
            let color = if connection == Connection::AuthExpired {
                palette.err
            } else {
                palette.mute
            };
            text(connection.label(), color)
        })
        .fit_by(
            |c: &ClusterSummary| c.connection().label().into(),
            Pixels::ZERO,
        )
        .auto(CLUSTER_STATE_MIN, CLUSTER_STATE_MAX),
        Column::new("Version", |c: &ClusterSummary, palette| {
            text(version_label(c), palette.fg2)
        })
        .mono()
        .fit_by(|c: &ClusterSummary| version_label(c).into(), Pixels::ZERO)
        .auto(CLUSTER_VERSION_MIN, CLUSTER_VERSION_MAX),
        Column::new("Nodes", |c: &ClusterSummary, palette| {
            text(nodes_label(c), dimmed(c, palette))
        })
        .mono()
        .end()
        .fit_by(|c: &ClusterSummary| nodes_label(c).into(), Pixels::ZERO)
        .auto(CLUSTER_NODES_MIN, CLUSTER_NODES_MAX),
        Column::new("Pods", pods_cell)
            .mono()
            .fit_by(|c: &ClusterSummary| pods_label(c).into(), SPACE_3)
            .auto(CLUSTER_PODS_MIN, CLUSTER_PODS_MAX),
        Column::new("CPU", |c: &ClusterSummary, palette| {
            mini_meter(c.metrics.as_ref().and_then(|m| m.cpu_percent), palette)
        })
        .mono()
        .fixed(CLUSTER_USAGE),
        Column::new("Memory", |c: &ClusterSummary, palette| {
            mini_meter(c.metrics.as_ref().and_then(|m| m.memory_percent), palette)
        })
        .mono()
        .fixed(CLUSTER_USAGE),
        Column::new("Flux", |c: &ClusterSummary, palette| match &c.metrics {
            Some(m) => text(flux_label(c), palette.status(m.flux.status())),
            None => text(MISSING, palette.mute),
        })
        .mono()
        .fit_by(|c: &ClusterSummary| flux_label(c).into(), Pixels::ZERO)
        .auto(CLUSTER_FLUX_MIN, CLUSTER_FLUX_MAX),
        Column::new("Revision", |c: &ClusterSummary, palette| match &c.metrics {
            Some(m) if m.revision.held => text(m.revision.sha.clone(), palette.sus),
            Some(m) => text(m.revision.sha.clone(), palette.fg2),
            None => text(MISSING, palette.mute),
        })
        .mono()
        .fit_by(|c: &ClusterSummary| revision_label(c).into(), Pixels::ZERO)
        .auto(CLUSTER_REVISION_MIN, CLUSTER_REVISION_MAX),
        Column::new("Problems", |c: &ClusterSummary, palette| {
            badges(&c.problems, SPACE_4, palette).into_any_element()
        })
        .mono()
        .fit_by(
            |c: &ClusterSummary| badges_label(&c.problems).into(),
            Pixels::ZERO,
        )
        .auto(CLUSTER_PROBLEMS_MIN, CLUSTER_PROBLEMS_MAX),
    ]
}

fn version_label(cluster: &ClusterSummary) -> String {
    cluster.version.clone().unwrap_or_else(|| MISSING.into())
}

fn nodes_label(cluster: &ClusterSummary) -> String {
    cluster
        .nodes
        .map_or_else(|| MISSING.to_owned(), |n| n.to_string())
}

fn revision_label(cluster: &ClusterSummary) -> String {
    cluster
        .metrics
        .as_ref()
        .map_or_else(|| MISSING.to_owned(), |m| m.revision.sha.clone())
}

fn flux_label(cluster: &ClusterSummary) -> String {
    let Some(m) = &cluster.metrics else {
        return MISSING.to_owned();
    };
    let total = m.flux.total.unwrap_or_default();
    format!("{} {}/{total}", m.flux.status().glyph(), m.flux_ready())
}

fn pods_label(cluster: &ClusterSummary) -> String {
    let Some(metrics) = &cluster.metrics else {
        return MISSING.to_owned();
    };
    let total = metrics.pods.total.unwrap_or_default();
    match metrics.pods.badges().into_iter().next() {
        Some((status, n)) => format!("{total} {} {n}", status.glyph()),
        None => total.to_string(),
    }
}

fn badges_label(counts: &Counts) -> String {
    let labels: Vec<String> = counts
        .badges()
        .into_iter()
        .map(|(status, n)| format!("{} {n}", status.glyph()))
        .collect();
    labels.join(BADGE_GAP)
}

fn pods_cell(cluster: &ClusterSummary, palette: &Palette) -> AnyElement {
    let Some(metrics) = &cluster.metrics else {
        return text(MISSING, palette.mute);
    };
    let total = metrics.pods.total.unwrap_or_default().to_string();
    let worst = metrics.pods.badges().into_iter().next();
    div()
        .flex()
        .gap(SPACE_3)
        .whitespace_nowrap()
        .child(total)
        .when_some(worst, |cell, (status, n)| {
            cell.child(
                div()
                    .text_color(palette.status(status))
                    .child(status_symbols(format!("{} {n}", status.glyph()))),
            )
        })
        .into_any_element()
}

fn problem_columns() -> Vec<Column<ProblemRow>> {
    vec![
        Column::new("", |row: &ProblemRow, _| match row {
            ProblemRow::Item(p) => StatusGlyph::new(p.status).into_any_element(),
            ProblemRow::Group { .. } => div().into_any_element(),
        })
        .fixed(PROBLEM_GLYPH),
        Column::new("Kind", |row: &ProblemRow, palette| match row {
            ProblemRow::Item(p) => text(p.target.kind.clone(), palette.mute),
            ProblemRow::Group { .. } => div().into_any_element(),
        })
        .fixed(PROBLEM_KIND),
        Column::new("Object", |row: &ProblemRow, palette| match row {
            ProblemRow::Item(p) => text(p.target.to_string(), palette.fg),
            ProblemRow::Group { .. } => div().into_any_element(),
        })
        .mono()
        .flex(PROBLEM_OBJECT_WEIGHT, PROBLEM_OBJECT_MIN),
        Column::new("Message", |row: &ProblemRow, _| match row {
            ProblemRow::Item(p) => RawMessage::new(p.message.clone()).line().into_any_element(),
            ProblemRow::Group { .. } => div().into_any_element(),
        })
        .flex(PROBLEM_MESSAGE_WEIGHT, PROBLEM_MESSAGE_MIN),
        Column::new("Age", |row: &ProblemRow, palette| match row {
            ProblemRow::Item(p) => small(flock_core::time::age(p.age), palette.mute),
            ProblemRow::Group { .. } => div().into_any_element(),
        })
        .mono()
        .end()
        .fixed(PROBLEM_AGE),
    ]
}

fn small(value: impl Into<SharedString>, color: Hsla) -> AnyElement {
    div()
        .truncate()
        .text_size(TEXT_SMALL)
        .text_color(color)
        .child(status_symbols(value))
        .into_any_element()
}

fn outcome(commit: &Commit, cluster: &ClusterId) -> (Option<Status>, String) {
    match commit.rollout_on(cluster) {
        Some(rollout) => (
            Some(rollout.status),
            format!(
                "{} {}",
                rollout.status.glyph(),
                outcome_label(rollout.status)
            ),
        ),
        None => (None, NOT_TARGETED.to_owned()),
    }
}

fn commit_columns(lanes: &[Lane]) -> Vec<Column<Commit>> {
    let fixed = vec![
        Column::new("Commit", |c: &Commit, palette| {
            text(c.short_sha().to_owned(), palette.acc)
        })
        .mono()
        .fixed(COMMIT_SHA),
        Column::text("Message", |c: &Commit| c.message.clone().into()).flex(1., COMMIT_MESSAGE_MIN),
        Column::new("Author", |c: &Commit, palette| {
            small(c.author.clone(), palette.fg2)
        })
        .mono()
        .fixed(COMMIT_AUTHOR),
        Column::new("Age", |c: &Commit, palette| {
            small(flock_core::time::age(c.age), palette.mute)
        })
        .mono()
        .fixed(COMMIT_AGE),
    ];
    let lane_columns = lanes.iter().map(|lane| {
        let cluster = lane.cluster.clone();
        Column::new(lane.label.clone(), move |c: &Commit, palette| {
            let (status, label) = outcome(c, &cluster);
            let color = status.map_or(palette.mute, |status| palette.status(status));
            small(label, color)
        })
        .mono()
        .fixed(COMMIT_LANE)
    });
    fixed.into_iter().chain(lane_columns).collect()
}

fn commit_table(lanes: &[Lane], cx: &mut Context<FlockScreen>) -> Entity<Table<Commit>> {
    let columns = commit_columns(lanes);
    cx.new(|cx| {
        Table::new("flock-commits", columns, cx)
            .row_height(ROW_TABLE_MAX)
            .click_opens()
            .fit_content()
    })
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn outcomes_pair_glyph_and_word_from_the_rollout() {
        let commit = Commit {
            sha: "a41e07c3f91d".into(),
            message: String::new(),
            author: String::new(),
            age: std::time::Duration::ZERO,
            when: String::new(),
            target: flock_core::changes::Target {
                kind: flock_core::changes::TargetKind::Kustomization,
                name: "apps".into(),
            },
            rollout: vec![flock_core::changes::Rollout {
                cluster: ClusterId::new("east"),
                status: Status::Suspended,
                object: "apps".into(),
                note: String::new(),
            }],
            views: Vec::new(),
        };
        assert_eq!(
            outcome(&commit, &ClusterId::new("east")),
            (Some(Status::Suspended), "‖ held".to_owned())
        );
        assert_eq!(
            outcome(&commit, &ClusterId::new("west")),
            (None, "–".to_owned())
        );
    }

    #[test]
    fn problem_rows_put_a_header_before_each_cluster() {
        let snapshot = Snapshot::default();
        assert!(problem_rows(&snapshot.home.problems, Severity::All, &[]).is_empty());
        let problem = Problem {
            status: Status::Warn,
            target: flock_core::ResourceRef {
                cluster: ClusterId::new("staging"),
                kind: "Pod".into(),
                namespace: Some("search".into()),
                name: "search-indexer-0".into(),
            },
            reason: String::new(),
            message: String::new(),
            condition: String::new(),
            age: std::time::Duration::ZERO,
            events: Vec::new(),
        };
        let rows = problem_rows(&[problem.clone(), problem], Severity::All, &[]);
        let shape: Vec<bool> = rows
            .iter()
            .map(|r| matches!(r, ProblemRow::Group { .. }))
            .collect();
        assert_eq!(shape, [true, false, false]);
        assert!(problem_rows(&[], Severity::Errors, &[]).is_empty());
    }

    #[test]
    fn content_widths_measure_the_text_each_cell_draws() {
        let problems = Counts::total(0).with_err(2).with_warn(13);
        assert_eq!(badges_label(&problems), "✕ 2  ▲ 13");
        assert_eq!(badges_label(&Counts::total(4)), "");
        let offline = ClusterSummary {
            id: ClusterId::new("edge"),
            name: "edge".into(),
            status: Status::Offline,
            state: "Offline".into(),
            distro: String::new(),
            region: String::new(),
            version: None,
            nodes: None,
            problems,
            nav: std::collections::BTreeMap::new(),
            metrics: None,
        };
        assert_eq!(
            [
                version_label(&offline),
                nodes_label(&offline),
                pods_label(&offline),
                flux_label(&offline),
                revision_label(&offline),
            ],
            [MISSING; 5].map(str::to_owned)
        );
    }
}
