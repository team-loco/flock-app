use flock_core::filter::{TextFilter, label_values};
use flock_core::gitops::{
    FluxInstall, Source, SyncCell, SyncFilter, SyncRow, VisibleGroup, filter_counts, visible_groups,
};
use flock_core::{ClusterId, Command, GitOps, Snapshot, Status};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, Hsla, IntoElement, ParentElement,
    Pixels, Render, SharedString, Styled, Window, div, px,
};

use crate::kit::status_symbols;
use crate::kit::{
    Column, FilterBar, FilterEvent, Highlighted, Matrix, MatrixCell, SectionHeader, Table,
    chip_label,
};
use crate::theme::{
    ActiveTheme, FONT_MONO, Palette, RULE, SPACE_2, SPACE_3, SPACE_6, SPACE_8, TEXT_BODY,
    TEXT_CONTROL, TEXT_GLYPH, TEXT_MONO, TEXT_SMALL,
};

const SOURCE_KIND: Pixels = px(120.);
const SOURCE_NAME: Pixels = px(230.);
const SOURCE_URL_MIN: Pixels = px(200.);
const SOURCE_REF: Pixels = px(64.);
const SOURCE_REVISION: Pixels = px(150.);
const SOURCE_AGE: Pixels = px(56.);
const SOURCE_READY: Pixels = px(80.);

const OBJECT_MIN: Pixels = px(220.);
const CLUSTER_MIN: Pixels = px(150.);

const INSTALL_CLUSTER: Pixels = px(140.);
const INSTALL_VERSION: Pixels = px(90.);
const INSTALL_NOTE: Pixels = px(200.);

const NO_REFERENCE: &str = "—";
const NOT_DEPLOYED: &str = "–";
const SOURCES_NOTE: &str = "fetched by source-controller on each cluster";
const LEGEND: [(Option<Status>, &str); 6] = [
    (Some(Status::Ok), "ready"),
    (Some(Status::Progress), "progressing"),
    (Some(Status::Warn), "retrying"),
    (Some(Status::Err), "failed"),
    (Some(Status::Suspended), "suspended"),
    (None, "not deployed"),
];
const LEGEND_BEHIND: &str = "Dimmed revision: not yet on the latest commit";

pub struct GitOpsScreen {
    gitops: GitOps,
    columns: Vec<SharedString>,
    filter: Entity<FilterBar>,
    sources: Entity<Table<Source>>,
    installs: Entity<Table<FluxInstall>>,
}

impl EventEmitter<Command> for GitOpsScreen {}

impl GitOpsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sources = cx.new(|cx| {
            Table::new("gitops-sources", source_columns(), cx)
                .headerless()
                .fit_content()
        });
        let installs = cx.new(|cx| {
            Table::new("gitops-installs", install_columns(), cx)
                .headerless()
                .fit_content()
        });
        let filter = cx.new(|cx| FilterBar::new("gitops-filter", window, cx));
        cx.subscribe(&filter, |this, _, FilterEvent::Changed, cx| {
            this.refresh(cx);
        })
        .detach();
        Self {
            gitops: GitOps::default(),
            columns: Vec::new(),
            filter,
            sources,
            installs,
        }
    }

    pub fn set_snapshot(&mut self, snapshot: &Snapshot, cx: &mut Context<Self>) {
        self.gitops.clone_from(&snapshot.gitops);
        self.columns = self
            .gitops
            .clusters
            .iter()
            .map(|id| cluster_name(snapshot, id))
            .collect();
        let installs = self.gitops.installs.clone();
        self.installs
            .update(cx, |table, cx| table.set_rows(installs, cx));
        self.refresh(cx);
    }

    pub fn filter_bar(&self) -> &Entity<FilterBar> {
        &self.filter
    }

    fn sync_filter(&self, cx: &Context<Self>) -> SyncFilter {
        let chip = self.filter.read(cx).chip();
        SyncFilter::ALL.get(chip).copied().unwrap_or_default()
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let labels = label_values(
            self.gitops
                .groups
                .iter()
                .flat_map(|group| &group.rows)
                .flat_map(|row| &row.label_sets),
        );
        self.filter.update(cx, |bar, cx| bar.set_labels(labels, cx));
        let query = self.filter.read(cx).query().clone();
        let counts = filter_counts(&self.gitops.groups, &query);
        let chips = counts
            .into_iter()
            .map(|(filter, n)| chip_label(filter.label(), n))
            .collect();
        let filter = self.sync_filter(cx);
        let shown: usize = visible_groups(&self.gitops.groups, filter, &query)
            .iter()
            .map(|group| group.rows.len())
            .sum();
        let everything: usize = self.gitops.groups.iter().map(|g| g.rows.len()).sum();
        self.filter.update(cx, |bar, cx| {
            bar.set_chips(chips, cx);
            bar.set_matches(shown, everything, cx);
        });
        let sources: Vec<Source> = self
            .gitops
            .sources
            .iter()
            .filter(|source| source.matches(&query))
            .cloned()
            .collect();
        let words = query.words;
        self.sources.update(cx, |table, cx| {
            table.set_highlight(words, cx);
            table.set_rows(sources, cx);
        });
        cx.notify();
    }

    fn matrix(&self, palette: &Palette, cx: &mut Context<Self>) -> Matrix {
        let query = self.filter.read(cx).query().clone();
        let groups = visible_groups(&self.gitops.groups, self.sync_filter(cx), &query);
        groups.iter().fold(
            Matrix::new("gitops-sync", "Object", self.columns.clone())
                .min_widths(OBJECT_MIN, CLUSTER_MIN),
            |matrix, group| self.group(matrix, group, &query, palette, cx),
        )
    }

    fn group(
        &self,
        matrix: Matrix,
        group: &VisibleGroup<'_>,
        query: &TextFilter,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Matrix {
        let matrix = matrix.group(group.kind.plural(), group.count_label());
        group.rows.iter().fold(matrix, |matrix, row| {
            let cells = self.cells(row, palette, cx);
            matrix.row(object_label(row, query, palette), cells)
        })
    }

    fn cells(&self, row: &SyncRow, palette: &Palette, cx: &mut Context<Self>) -> Vec<MatrixCell> {
        row.cells
            .iter()
            .zip(&self.gitops.clusters)
            .map(|(cell, cluster)| {
                let content = MatrixCell::new(cell_content(cell, palette));
                let Some(status) = cell.status else {
                    return content;
                };
                let target = row.target(cluster);
                content
                    .tint(cell_tint(status, palette))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(Command::OpenResource(target.clone()));
                    }))
            })
            .collect()
    }
}

impl Render for GitOpsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let matrix = self.matrix(&palette, cx);
        div()
            .flex()
            .flex_col()
            .pb(SPACE_8)
            .child(self.filter.clone())
            .child(SectionHeader::new("Sources").count(SOURCES_NOTE))
            .child(ruled(self.sources.clone(), &palette))
            .child(
                div()
                    .pt(SPACE_2)
                    .child(SectionHeader::new("Sync state").annotation(legend(&palette))),
            )
            .child(matrix)
            .child(div().pt(SPACE_2).child(SectionHeader::new("Controllers")))
            .child(ruled(self.installs.clone(), &palette))
    }
}

fn cluster_name(snapshot: &Snapshot, id: &ClusterId) -> SharedString {
    snapshot
        .cluster(id)
        .map_or_else(|| id.to_string(), |c| c.name.clone())
        .into()
}

fn ruled(content: impl IntoElement, palette: &Palette) -> impl IntoElement {
    div()
        .border_t(RULE)
        .border_color(palette.line)
        .child(content)
}

fn legend(palette: &Palette) -> impl IntoElement {
    let items = LEGEND.iter().map(|(status, label)| {
        let (glyph, color) = glyph(*status, palette);
        div()
            .flex()
            .gap(SPACE_3)
            .child(div().text_color(color).child(status_symbols(glyph)))
            .child(*label)
    });
    div()
        .flex()
        .flex_none()
        .gap(SPACE_6)
        .ml(SPACE_3)
        .font_family(FONT_MONO)
        .text_size(TEXT_SMALL)
        .text_color(palette.mute)
        .whitespace_nowrap()
        .children(items)
        .child(div().text_color(palette.fg2).child(LEGEND_BEHIND))
}

fn glyph(status: Option<Status>, palette: &Palette) -> (&'static str, Hsla) {
    match status {
        Some(status) => (status.glyph(), palette.status(status)),
        None => (NOT_DEPLOYED, palette.sep),
    }
}

fn cell_tint(status: Status, palette: &Palette) -> Option<Hsla> {
    match status {
        Status::Err => Some(palette.errbg),
        Status::Warn => Some(palette.warnbg),
        Status::Ok => None,
        Status::Progress => None,
        Status::Suspended => None,
        Status::Offline => None,
    }
}

fn revision_color(cell: &SyncCell, palette: &Palette) -> Hsla {
    if cell.behind {
        palette.mute
    } else {
        palette.fg
    }
}

fn note_color(cell: &SyncCell, palette: &Palette) -> Hsla {
    if cell.status == Some(Status::Err) {
        palette.err
    } else {
        palette.mute
    }
}

fn cell_content(cell: &SyncCell, palette: &Palette) -> AnyElement {
    let (glyph, glyph_color) = glyph(cell.status, palette);
    let line = div()
        .flex()
        .items_baseline()
        .gap(SPACE_3)
        .font_family(FONT_MONO)
        .text_size(TEXT_MONO)
        .whitespace_nowrap()
        .child(
            div()
                .text_size(TEXT_GLYPH)
                .text_color(glyph_color)
                .child(status_symbols(glyph)),
        )
        .child(
            div()
                .text_color(revision_color(cell, palette))
                .child(cell.revision.clone()),
        );
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .child(line)
        .child(
            div()
                .truncate()
                .text_size(TEXT_SMALL)
                .text_color(note_color(cell, palette))
                .child(cell.note.clone()),
        )
        .into_any_element()
}

fn object_label(row: &SyncRow, query: &TextFilter, palette: &Palette) -> AnyElement {
    let name = Highlighted::new(row.name.clone(), query.highlights(&row.name));
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .child(
            div()
                .font_family(FONT_MONO)
                .text_size(TEXT_BODY)
                .truncate()
                .child(name),
        )
        .child(
            div()
                .text_size(TEXT_SMALL)
                .text_color(palette.mute)
                .truncate()
                .child(row.detail.clone()),
        )
        .into_any_element()
}

fn mono_text(value: impl Into<SharedString>, size: Pixels, color: Hsla) -> AnyElement {
    div()
        .truncate()
        .text_size(size)
        .text_color(color)
        .child(value.into())
        .into_any_element()
}

fn source_columns() -> Vec<Column<Source>> {
    vec![
        Column::colored_text(
            "Kind",
            |s: &Source| s.kind.clone().into(),
            |_, palette| palette.mute,
        )
        .fixed(SOURCE_KIND),
        Column::text("Name", |s: &Source| {
            format!("{}/{}", s.namespace, s.name).into()
        })
        .highlight()
        .mono()
        .fixed(SOURCE_NAME),
        Column::new("URL", |s: &Source, palette| {
            mono_text(s.url.clone(), TEXT_MONO, palette.fg2)
        })
        .mono()
        .flex(1., SOURCE_URL_MIN),
        Column::new("Ref", |s: &Source, palette| {
            let reference = s.reference.clone().unwrap_or_else(|| NO_REFERENCE.into());
            mono_text(reference, TEXT_MONO, palette.fg2)
        })
        .mono()
        .fixed(SOURCE_REF),
        Column::new("Revision", |s: &Source, palette| {
            mono_text(s.revision.clone(), TEXT_MONO, palette.acc)
        })
        .mono()
        .fixed(SOURCE_REVISION),
        Column::new("Fetched", |s: &Source, palette| {
            mono_text(flock_core::time::age(s.fetched), TEXT_SMALL, palette.mute)
        })
        .mono()
        .fixed(SOURCE_AGE),
        Column::new("Ready", |s: &Source, palette| {
            mono_text(s.ready_label(), TEXT_SMALL, palette.status(s.status()))
        })
        .mono()
        .fixed(SOURCE_READY),
    ]
}

fn controllers(install: &FluxInstall, palette: &Palette) -> AnyElement {
    let items = install.controllers.iter().map(|controller| {
        let (glyph, color) = glyph(controller.status, palette);
        div()
            .flex()
            .gap(SPACE_3)
            .child(div().text_color(color).child(status_symbols(glyph)))
            .child(controller.name.clone())
    });
    div()
        .flex()
        .gap(SPACE_3)
        .overflow_hidden()
        .text_size(TEXT_SMALL)
        .text_color(palette.fg2)
        .children(items)
        .into_any_element()
}

fn install_columns() -> Vec<Column<FluxInstall>> {
    vec![
        Column::text("Cluster", |i: &FluxInstall| i.cluster.to_string().into())
            .fixed(INSTALL_CLUSTER),
        Column::new("Version", |i: &FluxInstall, palette| {
            let color = match i.status() {
                Status::Ok => palette.fg,
                status => palette.status(status),
            };
            mono_text(i.version.clone(), TEXT_MONO, color)
        })
        .mono()
        .fixed(INSTALL_VERSION),
        Column::new("Controllers", controllers)
            .mono()
            .flex(1., Pixels::ZERO),
        Column::new("Note", |i: &FluxInstall, palette| {
            let color = match i.status() {
                Status::Ok => palette.mute,
                status => palette.status(status),
            };
            div()
                .truncate()
                .text_size(TEXT_CONTROL)
                .text_color(color)
                .child(i.note())
                .into_any_element()
        })
        .fixed(INSTALL_NOTE),
    ]
}
