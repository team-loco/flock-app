use flock_core::emptiness::Noun;
use flock_core::filter::label_values;
use flock_core::loco::{Build, BuildLogLine, Builds, LocoCommand, LogTone};
use flock_core::{Command, Page, ResourceRef, Status};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FontWeight, Hsla, IntoElement,
    ParentElement, Pixels, Render, SharedString, Styled, Subscription, Window, div, px,
};

use crate::kit::status_symbols;
use crate::kit::{
    Button, Column, ConfirmDialog, FilterBar, FilterEvent, Level, LogLine, LogView, PageTitle,
    StatusGlyph, Table, TableEvent, confirm,
};
use crate::screens::{ListShape, empty_list};
use crate::theme::{
    ActiveTheme, FONT_MONO, Palette, ROW_TABLE_MAX, ROW_TABLE_MIN, RULE, SPACE_3, SPACE_5, SPACE_7,
    SPACE_8, TEXT_BODY, TEXT_SMALL,
};

const GLYPH: Pixels = px(14.);
const BUILD_ID: Pixels = px(84.);
const SERVICE: Pixels = px(120.);
const WORKSPACE: Pixels = px(130.);
const PHASE: Pixels = px(104.);
const STARTED: Pixels = px(62.);
const TOOK: Pixels = px(64.);
const LIST_MAX_HEIGHT: Pixels = px(300.);

const TITLE: &str = "Builds";
const CANCEL: &str = "Cancel build";

pub struct BuildsScreen {
    cluster_name: String,
    shown: usize,
    pub(crate) navigation: Option<Entity<crate::kit::HeaderNavigation>>,
    builds: Builds,
    selected: Option<ResourceRef>,
    shown_log: Vec<BuildLogLine>,
    table: Entity<Table<Build>>,
    log: Entity<LogView>,
    filter: Entity<FilterBar>,
    _subscriptions: [Subscription; 2],
}

impl EventEmitter<Command> for BuildsScreen {}

impl BuildsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| FilterBar::new("loco-build-filter", window, cx));
        let table = cx.new(|cx| {
            Table::new("loco-builds", build_columns(), cx)
                .row_height(ROW_TABLE_MAX)
                .row_key(|build: &Build| build.resource.to_string().into())
        });
        let log = cx.new(|_| LogView::new("loco-build-log").plain().inset(SPACE_8));
        let subscription = cx.subscribe(&table, |this, table, event: &TableEvent, cx| {
            let TableEvent::Selected(ix) = *event else {
                return;
            };
            let picked = table.read(cx).row(ix).map(|build| build.resource.clone());
            this.select(picked, cx);
        });
        let filtered = cx.subscribe(&filter, |this, _, FilterEvent::Changed, cx| {
            this.refresh(cx);
        });
        Self {
            cluster_name: String::new(),
            shown: 0,
            navigation: None,
            builds: Builds::default(),
            selected: None,
            shown_log: Vec::new(),
            table,
            log,
            filter,
            _subscriptions: [subscription, filtered],
        }
    }

    pub fn filter_bar(&self) -> &Entity<FilterBar> {
        &self.filter
    }

    pub fn set_cluster_name(&mut self, name: &str) {
        self.cluster_name.clear();
        self.cluster_name.push_str(name);
    }

    pub fn set_builds(&mut self, builds: Builds, cx: &mut Context<Self>) {
        self.builds = builds;
        self.refresh(cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let labels = label_values(self.builds.builds.iter().map(|build| &build.labels));
        self.filter.update(cx, |bar, cx| bar.set_labels(labels, cx));
        let query = self.filter.read(cx).query().clone();
        let rows: Vec<Build> = self
            .builds
            .builds
            .iter()
            .filter(|build| build.matches(&query))
            .cloned()
            .collect();
        let shown = rows.len();
        self.shown = shown;
        let total = self.builds.builds.len();
        self.filter
            .update(cx, |bar, cx| bar.set_matches(shown, total, cx));
        let words = query.words;
        self.table.update(cx, |table, cx| {
            table.set_highlight(words, cx);
            table.set_rows(rows, cx);
        });
        let still_there = self.current().is_some();
        let selected = if still_there {
            self.selected.clone()
        } else {
            self.table
                .read(cx)
                .row(0)
                .map(|build| build.resource.clone())
        };
        self.select(selected, cx);
    }

    fn select(&mut self, selected: Option<ResourceRef>, cx: &mut Context<Self>) {
        self.selected = selected;
        let row = self.selected.as_ref().and_then(|selected| {
            let table = self.table.read(cx);
            (0..)
                .map_while(|ix| table.row(ix))
                .position(|build| &build.resource == selected)
        });
        self.table.update(cx, |table, cx| table.select(row, cx));
        let lines = self
            .current()
            .map(|build| build.log.clone())
            .unwrap_or_default();
        if lines != self.shown_log {
            let log_lines: Vec<LogLine> = lines.iter().map(log_line).collect();
            self.log.update(cx, |log, cx| {
                log.clear(cx);
                log.extend(log_lines, cx);
            });
            self.shown_log = lines;
        }
        cx.notify();
    }

    fn current_index(&self) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        self.builds
            .builds
            .iter()
            .position(|build| &build.resource == selected)
    }

    fn current(&self) -> Option<&Build> {
        self.current_index()
            .and_then(|ix| self.builds.builds.get(ix))
    }

    fn list_height(&self) -> Pixels {
        let rows = u16::try_from(self.builds.builds.len().max(1)).unwrap_or(u16::MAX);
        let content = ROW_TABLE_MIN + ROW_TABLE_MAX * f32::from(rows);
        content.min(LIST_MAX_HEIGHT)
    }

    fn summary(build: &Build, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let cancel = build.phase.is_active().then(|| {
            let target = build.seen();
            let detail = format!("{} · {}", target.target.cluster, build.job_label());
            let title = format!("{} · {}", build.service, build.id);
            let this = cx.entity().downgrade();
            Button::new("loco-build-cancel", CANCEL)
                .small()
                .on_click(move |_, window, cx| {
                    let this = this.clone();
                    let target = target.clone();
                    let dialog = ConfirmDialog::new(
                        format!("Cancel build {title}"),
                        "The build Job is deleted now. Its log stays here until the build is cleaned up; nothing is pushed.",
                        CANCEL,
                        move |_, cx| {
                            let command =
                                Command::Loco(LocoCommand::CancelBuild(target.clone()));
                            let sent = this.update(cx, |_, cx| cx.emit(command));
                            if sent.is_err() {
                                tracing::debug!("builds screen closed before the confirmation");
                            }
                        },
                    )
                    .detail(detail.clone());
                    confirm(dialog, window, cx);
                })
        });
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(SPACE_7)
            .px(SPACE_8)
            .py(SPACE_5)
            .border_t(RULE)
            .border_b(RULE)
            .border_color(palette.line)
            .text_size(TEXT_BODY)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("{} · {}", build.service, build.id)),
            )
            .child(mono(build.job_label(), palette.mute))
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .gap(SPACE_3)
                    .font_family(FONT_MONO)
                    .text_size(TEXT_SMALL)
                    .text_color(palette.fg2)
                    .child("fetch")
                    .child(glyph(build.fetch, palette))
                    .child(div().pl(SPACE_3).child("buildkit"))
                    .child(glyph(build.buildkit, palette)),
            )
            .children(cancel)
            .into_any_element()
    }
}

impl Render for BuildsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let shape = ListShape {
            shown: self.shown,
            total: self.builds.builds.len(),
            namespace: None,
            noun: Noun::of(Page::LocoBuilds),
            cluster: &self.cluster_name,
        };
        if let Some(empty) = empty_list(&shape, &self.filter, None, cx) {
            return div()
                .flex()
                .flex_col()
                .size_full()
                .child(PageTitle::new(TITLE).detail(self.builds.subtitle()))
                .child(self.filter.clone())
                .child(empty)
                .into_any_element();
        }
        let summary = self
            .current()
            .cloned()
            .map(|build| Self::summary(&build, &palette, cx));
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                PageTitle::new(TITLE)
                    .leading(self.navigation.clone())
                    .detail(self.builds.subtitle()),
            )
            .child(self.filter.clone())
            .child(
                div()
                    .flex_none()
                    .h(self.list_height())
                    .child(self.table.clone()),
            )
            .children(summary)
            .child(div().flex_1().min_h_0().child(self.log.clone()))
            .into_any_element()
    }
}

fn log_line(line: &BuildLogLine) -> LogLine {
    let level = match line.tone {
        LogTone::Plain => None,
        LogTone::Dim => Some(Level::Debug),
        LogTone::Error => Some(Level::Error),
    };
    LogLine::new(None, level, line.text.clone())
}

fn glyph(status: Status, palette: &Palette) -> impl IntoElement {
    div()
        .text_color(palette.status(status))
        .child(status_symbols(status.glyph()))
}

fn mono(value: impl Into<SharedString>, color: Hsla) -> AnyElement {
    div()
        .truncate()
        .font_family(FONT_MONO)
        .text_size(TEXT_SMALL)
        .text_color(color)
        .child(value.into())
        .into_any_element()
}

fn text(value: impl Into<SharedString>, color: Hsla) -> AnyElement {
    div()
        .truncate()
        .text_color(color)
        .child(value.into())
        .into_any_element()
}

fn build_columns() -> Vec<Column<Build>> {
    vec![
        Column::new("", |build: &Build, _| {
            StatusGlyph::new(build.phase.status()).into_any_element()
        })
        .fixed(GLYPH),
        Column::colored_text(
            "Build",
            |build: &Build| build.id.clone().into(),
            |_, p| p.fg2,
        )
        .highlight()
        .mono()
        .fixed(BUILD_ID),
        Column::new("Service", |build: &Build, _| {
            div()
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .child(build.service.clone())
                .into_any_element()
        })
        .fixed(SERVICE),
        Column::new("Workspace", |build: &Build, palette| {
            text(build.workspace.label().to_owned(), palette.mute)
        })
        .fixed(WORKSPACE),
        Column::new("Phase", |build: &Build, palette| {
            text(build.phase.label(), palette.status(build.phase.status()))
        })
        .fixed(PHASE),
        Column::new("Started", |build: &Build, palette| {
            let started = build.started.clone().unwrap_or_default();
            text(started, palette.mute)
        })
        .mono()
        .fixed(STARTED),
        Column::new("Took", |build: &Build, palette| {
            text(build.took_label(), palette.fg2)
        })
        .mono()
        .fixed(TOOK),
        Column::new("Image", |build: &Build, palette| {
            mono(build.image_label(), palette.mute)
        })
        .flex(1., Pixels::ZERO),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_log_tones_map_to_log_levels() {
        let lines = [
            BuildLogLine::new(LogTone::Plain, "#1 load"),
            BuildLogLine::new(LogTone::Dim, "fetch ok"),
            BuildLogLine::new(LogTone::Error, "ERROR: failed"),
        ];
        let levels: Vec<_> = lines.iter().map(|l| log_line(l).level).collect();
        assert_eq!(levels, [None, Some(Level::Debug), Some(Level::Error)]);
    }
}
