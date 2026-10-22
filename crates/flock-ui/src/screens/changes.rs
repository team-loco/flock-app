use flock_core::changes::{
    ChangesLog, Commit, FilePatch, Lane, LineChange, PatchLine, Rendering, Rollout, ViewKind,
};
use flock_core::{ClusterId, Page, Snapshot, Status};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window, div, px,
};

use crate::kit::status_symbols;
use crate::kit::{
    Column, DiffFile, DiffLine, DiffView, FilterBar, FilterEvent, LineKind, SectionHeader,
    Segmented, Select, SelectEvent, Table, TableEvent, Tooltip,
};
use crate::theme::{
    ActiveTheme, FONT_MONO, MONO_ADVANCE, PAGE_HEADER, Palette, ROW_COMMIT, ROW_ROLLOUT, RULE,
    SPACE_1, SPACE_2, SPACE_3, SPACE_4, SPACE_6, SPACE_7, SPACE_8, TEXT_AXIS, TEXT_BODY,
    TEXT_LABEL, TEXT_MONO, TEXT_SMALL, TEXT_SUBHEADING, TEXT_TITLE,
};

const COMMIT_LIST: Pixels = px(360.);
const DETAIL_MIN: Pixels = px(560.);
const DETAIL_GROW: f32 = 999.;
const LANE: Pixels = px(30.);
const LANE_MAX: Pixels = px(96.);
const CLUSTER_MENU: Pixels = px(180.);
const ROLLOUT_GLYPH: Pixels = px(14.);
const ROLLOUT_CLUSTER: Pixels = px(110.);
const ROLLOUT_OBJECT: Pixels = px(140.);

const ALL_CLUSTERS: &str = "All clusters";
const COMMIT: &str = "Commit";
const NO_COMMITS: &str = "No commits match the filter.";
const NOT_TARGETED: &str = "·";
const SEPARATOR: &str = " · ";

pub struct ChangesScreen {
    pub(crate) navigation: Option<Entity<crate::kit::HeaderNavigation>>,
    log: ChangesLog,
    selected: Option<String>,
    view: Option<ViewKind>,
    cluster: Option<ClusterId>,
    filter: Entity<FilterBar>,
    clusters: Entity<Select>,
    commits: Entity<Table<Commit>>,
    _subscriptions: [Subscription; 3],
}

impl ChangesScreen {
    pub fn list_focus(&self, cx: &gpui::App) -> gpui::FocusHandle {
        gpui::Focusable::focus_handle(&self.commits, cx)
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let clusters = cx.new(|cx| {
            Select::new("changes-cluster", vec![ALL_CLUSTERS.into()], cx)
                .mono()
                .width(CLUSTER_MENU)
        });
        let filter =
            cx.new(|cx| FilterBar::new("changes-filter", window, cx).scope(clusters.clone()));
        let commits = cx.new(|cx| {
            Table::new("changes-commits", commit_columns(&[]), cx)
                .row_height(ROW_COMMIT)
                .column_gap(SPACE_2)
                .inset(SPACE_6)
                .mono_header()
                .empty(NO_COMMITS)
        });
        let subscriptions = [
            cx.subscribe(&filter, |this, _, FilterEvent::Changed, cx| {
                this.refresh(cx);
            }),
            cx.subscribe(&clusters, |this, _, SelectEvent::Changed(ix), cx| {
                this.cluster = ix
                    .checked_sub(1)
                    .and_then(|lane| this.log.lanes.get(lane))
                    .map(|lane| lane.cluster.clone());
                this.refresh(cx);
            }),
            cx.subscribe(&commits, |this, table, event: &TableEvent, cx| {
                let TableEvent::Selected(ix) = *event else {
                    return;
                };
                this.selected = table.read(cx).row(ix).map(|c| c.sha.clone());
                cx.notify();
            }),
        ];
        Self {
            navigation: None,
            log: ChangesLog::default(),
            selected: None,
            view: None,
            cluster: None,
            filter,
            clusters,
            commits,
            _subscriptions: subscriptions,
        }
    }

    pub fn set_snapshot(&mut self, snapshot: &Snapshot, cx: &mut Context<Self>) {
        if self.log.lanes != snapshot.changes.lanes {
            let columns = commit_columns(&snapshot.changes.lanes);
            self.commits
                .update(cx, |table, cx| table.set_columns(columns, cx));
            let options = cluster_options(&snapshot.changes.lanes);
            self.clusters
                .update(cx, |select, cx| select.set_options(options, cx));
        }
        self.log.clone_from(&snapshot.changes);
        self.refresh(cx);
    }

    pub fn open(&mut self, sha: &str, cx: &mut Context<Self>) {
        self.selected = self.log.commit(sha).map(|c| c.sha.clone());
        self.refresh(cx);
    }

    pub fn cluster(&self) -> Option<&ClusterId> {
        self.cluster.as_ref()
    }

    pub fn show_cluster(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let Some(lane) = self.log.lanes.iter().position(|l| &l.cluster == cluster) else {
            return;
        };
        self.clusters
            .update(cx, |select, cx| select.set_selected(lane + 1, cx));
        self.cluster = Some(cluster.clone());
        self.refresh(cx);
    }

    pub fn filter_bar(&self) -> &Entity<FilterBar> {
        &self.filter
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let query = self.filter.read(cx).query().clone();
        let visible: Vec<Commit> = self
            .log
            .visible(&query, self.cluster.as_ref())
            .into_iter()
            .cloned()
            .collect();
        let shown = visible.len();
        let total = self.log.commits.len();
        self.filter
            .update(cx, |bar, cx| bar.set_matches(shown, total, cx));
        let kept = visible
            .iter()
            .position(|c| Some(&c.sha) == self.selected.as_ref());
        let index = kept.or_else(|| (!visible.is_empty()).then_some(0));
        self.selected = index.and_then(|ix| visible.get(ix)).map(|c| c.sha.clone());
        self.commits.update(cx, |table, cx| {
            table.set_rows(visible, cx);
            table.select(index, cx);
        });
        cx.notify();
    }

    fn current(&self) -> Option<&Commit> {
        self.selected
            .as_deref()
            .and_then(|sha| self.log.commit(sha))
    }

    fn header(&self, palette: &Palette) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(SPACE_4)
            .h(PAGE_HEADER)
            .px(SPACE_8)
            .border_b(RULE)
            .border_color(palette.line)
            .text_size(TEXT_BODY)
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .children(self.navigation.clone())
            .child(
                div()
                    .text_size(TEXT_TITLE)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(Page::Changes.label()),
            )
            .child(
                div()
                    .pl(SPACE_2)
                    .font_family(FONT_MONO)
                    .text_size(TEXT_SMALL)
                    .text_color(palette.mute)
                    .child(self.log.source_label()),
            )
            .children(self.log.unavailable.as_ref().map(|_| {
                div()
                    .font_family(FONT_MONO)
                    .text_size(TEXT_SMALL)
                    .text_color(palette.warn)
                    .child(self.log.fetched_label())
            }))
    }

    fn detail(&self, commit: &Commit, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let view = commit.view(self.view.as_ref());
        div()
            .id("changes-detail")
            .flex()
            .flex_col()
            .flex_grow(DETAIL_GROW)
            .flex_basis(DETAIL_MIN)
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .child(commit_header(commit, palette))
            .child(SectionHeader::new("Rollout").count(commit.target_sentence()))
            .child(
                div()
                    .border_t(RULE)
                    .border_color(palette.line)
                    .children(commit.rollout.iter().map(|r| rollout_row(r, palette))),
            )
            .child(diff_header(commit, view, cx))
            .children(view.map(diff_body))
            .into_any_element()
    }
}

impl Render for ChangesScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let detail = self
            .current()
            .cloned()
            .map(|commit| self.detail(&commit, &palette, cx));
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(self.header(&palette))
            .child(self.filter.clone())
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_grow(1.)
                            .flex_basis(COMMIT_LIST)
                            .min_w_0()
                            .h_full()
                            .bg(palette.panel)
                            .border_r(RULE)
                            .border_color(palette.line)
                            .child(self.commits.clone()),
                    )
                    .children(detail),
            )
    }
}
fn diff_header(
    commit: &Commit,
    current: Option<&Rendering>,
    cx: &mut Context<ChangesScreen>,
) -> impl IntoElement {
    let kinds: Vec<ViewKind> = commit.views.iter().map(|v| v.kind.clone()).collect();
    let selected = current
        .and_then(|c| kinds.iter().position(|k| *k == c.kind))
        .unwrap_or_default();
    let segmented = kinds
        .iter()
        .fold(Segmented::new("changes-view"), |seg, kind| {
            seg.option(kind.label())
        })
        .selected(selected)
        .on_select(cx.listener(move |this, ix: &usize, _, cx| {
            this.view = kinds.get(*ix).cloned();
            cx.notify();
        }));
    SectionHeader::new("Diff").end(segmented)
}

fn cluster_options(lanes: &[Lane]) -> Vec<SharedString> {
    let clusters = lanes
        .iter()
        .map(|l| SharedString::from(l.cluster.to_string()));
    std::iter::once(ALL_CLUSTERS.into())
        .chain(clusters)
        .collect()
}

fn commit_columns(lanes: &[Lane]) -> Vec<Column<Commit>> {
    let summary = Column::new(COMMIT, |commit: &Commit, palette| {
        commit_summary(commit, palette).into_any_element()
    })
    .flex(1., Pixels::ZERO);
    let width = lane_width(lanes);
    let lanes = lanes.iter().enumerate().map(|(ix, lane)| {
        let cluster = lane.cluster.clone();
        Column::new(lane.short.clone(), move |commit: &Commit, palette| {
            lane_cell(ix, commit, &cluster, palette)
        })
        .fixed(width)
        .center()
    });
    std::iter::once(summary).chain(lanes).collect()
}

fn lane_width(lanes: &[Lane]) -> Pixels {
    let widest = lanes
        .iter()
        .map(|lane| lane.short.chars().count())
        .max()
        .unwrap_or_default();
    let chars = u16::try_from(widest).map_or(f32::MAX, f32::from);
    (TEXT_AXIS * (MONO_ADVANCE * chars)).max(LANE).min(LANE_MAX)
}

fn commit_summary(commit: &Commit, palette: &Palette) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .min_w_0()
        .child(div().truncate().child(commit.message.clone()))
        .child(
            div()
                .flex()
                .font_family(FONT_MONO)
                .text_size(TEXT_LABEL)
                .text_color(palette.mute)
                .whitespace_nowrap()
                .child(
                    div()
                        .text_color(palette.acc)
                        .child(commit.short_sha().to_owned()),
                )
                .child(format!(
                    "{SEPARATOR}{}{SEPARATOR}{}",
                    commit.author,
                    commit.age_label()
                )),
        )
}

fn lane_cell(ix: usize, commit: &Commit, cluster: &ClusterId, palette: &Palette) -> AnyElement {
    let rollout = commit.rollout_on(cluster);
    let (glyph, color) = match rollout {
        Some(r) => (r.status.glyph(), palette.status(r.status)),
        None => (NOT_TARGETED, palette.sep),
    };
    let tooltip = lane_tooltip(cluster, rollout);
    div()
        .id(ix)
        .font_family(FONT_MONO)
        .text_size(TEXT_LABEL)
        .text_color(color)
        .child(status_symbols(glyph))
        .tooltip(Tooltip::text(tooltip))
        .into_any_element()
}

fn lane_tooltip(cluster: &ClusterId, rollout: Option<&Rollout>) -> String {
    match rollout {
        Some(r) => format!("{cluster}: {}", r.note),
        None => format!("{cluster}: not affected"),
    }
}

fn commit_header(commit: &Commit, palette: &Palette) -> impl IntoElement {
    let meta = [commit.author.as_str(), commit.when.as_str(), &commit.stat()].join(SEPARATOR);
    div()
        .flex()
        .flex_col()
        .flex_none()
        .gap(SPACE_3)
        .px(SPACE_8)
        .py(SPACE_7)
        .border_b(RULE)
        .border_color(palette.line)
        .child(
            div()
                .text_size(TEXT_SUBHEADING)
                .font_weight(FontWeight::SEMIBOLD)
                .child(commit.message.clone()),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .font_family(FONT_MONO)
                .text_size(TEXT_MONO)
                .text_color(palette.mute)
                .child(div().text_color(palette.acc).child(commit.sha.clone()))
                .child(format!("{SEPARATOR}{meta}")),
        )
}

fn rollout_row(rollout: &Rollout, palette: &Palette) -> impl IntoElement {
    let note_color = if rollout.status == Status::Err {
        palette.err
    } else {
        palette.fg2
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(SPACE_6)
        .h(ROW_ROLLOUT)
        .px(SPACE_8)
        .border_b(RULE)
        .border_color(palette.line)
        .text_size(TEXT_BODY)
        .child(
            div()
                .w(ROLLOUT_GLYPH)
                .flex_none()
                .font_family(FONT_MONO)
                .text_size(TEXT_AXIS)
                .text_color(palette.status(rollout.status))
                .child(status_symbols(rollout.status.glyph())),
        )
        .child(
            div()
                .w(ROLLOUT_CLUSTER)
                .flex_none()
                .truncate()
                .child(rollout.cluster.to_string()),
        )
        .child(
            div()
                .w(ROLLOUT_OBJECT)
                .flex_none()
                .truncate()
                .font_family(FONT_MONO)
                .text_size(TEXT_MONO)
                .text_color(palette.fg2)
                .child(rollout.object.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(note_color)
                .child(rollout.note.clone()),
        )
}

fn diff_body(view: &Rendering) -> impl IntoElement {
    let files = view.files.iter().map(diff_file).collect();
    div()
        .flex()
        .flex_col()
        .gap(SPACE_4)
        .px(SPACE_8)
        .pb(SPACE_7)
        .child(DiffView::new("changes-diff", files))
}

fn diff_file(file: &FilePatch) -> DiffFile {
    let lines = file.lines.iter().map(diff_line).collect();
    DiffFile::new(file.path.clone(), lines)
}

fn diff_line(line: &PatchLine) -> DiffLine {
    let kind = match line.change {
        LineChange::Context => LineKind::Context,
        LineChange::Added => LineKind::Added,
        LineChange::Removed => LineKind::Removed,
        LineChange::Hunk => LineKind::Hunk,
    };
    let converted = DiffLine::new(kind, Some(line.number), line.text.clone());
    match &line.note {
        Some(note) => converted.note(note.clone()),
        None => converted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lanes_widen_to_fit_whole_cluster_names() {
        let lane = |short: &str| Lane {
            cluster: ClusterId::new(short),
            short: short.into(),
            label: short.into(),
        };
        assert_eq!(lane_width(&[lane("stg"), lane("use1")]), LANE);
        let fleet = lane_width(&[lane("fleet-a"), lane("fleet-b")]);
        assert!(fleet > LANE && fleet < LANE_MAX);
        let long = "a-very-long-cluster-name-indeed";
        assert_eq!(lane_width(&[lane(long)]), LANE_MAX);
    }

    #[test]
    fn cluster_options_start_with_all_clusters() {
        let lanes = [Lane {
            cluster: ClusterId::new("staging"),
            short: "stg".into(),
            label: "staging".into(),
        }];
        assert_eq!(
            cluster_options(&lanes),
            [
                SharedString::from(ALL_CLUSTERS),
                SharedString::from("staging")
            ]
        );
    }

    #[test]
    fn lane_tooltips_explain_the_outcome() {
        let cluster = ClusterId::new("prod-us-east");
        let rollout = Rollout {
            cluster: cluster.clone(),
            status: Status::Err,
            object: "apps-payments".into(),
            note: "health check failing".into(),
        };
        assert_eq!(
            lane_tooltip(&cluster, Some(&rollout)),
            "prod-us-east: health check failing"
        );
        assert_eq!(lane_tooltip(&cluster, None), "prod-us-east: not affected");
    }

    #[test]
    fn notes_survive_the_conversion_to_kit_lines() {
        let line = PatchLine {
            change: LineChange::Added,
            number: 48,
            text: "key: webhook_secret".into(),
            note: Some("set by overlay".into()),
        };
        let converted = diff_line(&line);
        assert_eq!(converted.kind, LineKind::Added);
        assert_eq!(converted.new, Some(48));
        assert_eq!(converted.note, Some("set by overlay".into()));
    }

    #[test]
    fn hunk_headers_become_unnumbered_separators() {
        let line = PatchLine {
            change: LineChange::Hunk,
            number: 48,
            text: "@@ -48,2 +48,3 @@ jobs:".into(),
            note: None,
        };
        let converted = diff_line(&line);
        assert_eq!(converted.kind, LineKind::Hunk);
        assert_eq!((converted.old, converted.new), (None, None));
        assert_eq!(converted.text, "@@ -48,2 +48,3 @@ jobs:");
    }
}
