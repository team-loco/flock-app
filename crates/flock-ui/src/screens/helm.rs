use flock_core::emptiness::Noun;
use flock_core::filter::StatusFilter;
use flock_core::flux::HelmRevision;
use flock_core::helm::{HelmKey, HelmRelease, HelmReleaseDetail, filter};
use flock_core::time::age;
use flock_core::{ClusterId, ClusterSummary, Command, Counts, Page, Snapshot, Status};
use gpui::{
    AnyElement, AppContext, Context, Entity, EventEmitter, InteractiveElement, IntoElement,
    ParentElement, Pixels, Render, StatefulInteractiveElement, Styled, Subscription, Window, div,
    px,
};

use super::{ListShape, ScreenEvent, empty_list, new_tab_item};
use crate::kit::{
    Button, CodeView, Column, EmptyState, FilterBar, FilterEvent, MenuItem, PageHeader,
    SectionHeader, Skeleton, StatusGlyph, Table, TableEvent, chip_label,
};
use crate::theme::{ActiveTheme, RULE, SPACE_3, SPACE_8};

const NAME_WEIGHT: f32 = 1.2;
const NAME_MIN: Pixels = px(180.);
const NAMESPACE_WIDTH: Pixels = px(130.);
const CHART_WIDTH: Pixels = px(220.);
const APP_WIDTH: Pixels = px(96.);
const REVISION_WIDTH: Pixels = px(72.);
const STATE_WIDTH: Pixels = px(120.);
const AGE_WIDTH: Pixels = px(64.);
const FLUX_WEIGHT: f32 = 1.;
const FLUX_MIN: Pixels = px(160.);
const GLYPH_WIDTH: Pixels = px(24.);
const DESCRIPTION_WEIGHT: f32 = 1.;
const NOT_FLUX: &str = "—";
const OPEN_FLUX: &str = "Open Flux HelmRelease";
const OPEN_FLUX_DESCRIPTION: &str = "Show the Flux object that manages this release.";
const HISTORY: &str = "History";
const VALUES: &str = "Values";
const MANIFEST: &str = "Manifest";
const USER_SUPPLIED: &str = "User-supplied";
const COMPUTED: &str = "Computed";
const NO_VALUES: &str = "# no values";
const SEPARATOR: &str = " · ";
const REVISION_PREFIX: &str = "revision ";
const MANAGED_BY: &str = "managed by Flux";
const GONE: &str = "This release is not in Helm storage on this cluster any more.";

fn flux_label(release: &HelmRelease) -> String {
    release.flux.as_ref().map_or_else(
        || NOT_FLUX.to_owned(),
        |key| format!("{}/{}", key.namespace, key.name),
    )
}

pub struct HelmReleasesScreen {
    cluster: Option<ClusterId>,
    cluster_name: String,
    rows: Vec<HelmRelease>,
    shown: usize,
    table: Entity<Table<HelmRelease>>,
    filter: Entity<FilterBar>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ScreenEvent> for HelmReleasesScreen {}

impl HelmReleasesScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx
            .new(|cx| FilterBar::new("helm-filter", HelmRelease::FILTER.placeholder(), window, cx));
        let table = cx.new(|cx| {
            Table::new("helm-releases", columns(), cx)
                .row_key(|r: &HelmRelease| format!("{}/{}", r.key.namespace, r.key.name).into())
                .row_status(|r: &HelmRelease| (r.status != Status::Ok).then_some(r.status))
        });
        let subscriptions = vec![
            cx.subscribe(&filter, |this, _, FilterEvent::Changed, cx| {
                this.refresh(cx);
            }),
            cx.subscribe(&table, |_, table, event: &TableEvent, cx| {
                if let TableEvent::Opened(ix) = *event
                    && let Some(row) = table.read(cx).row(ix)
                {
                    let open = Command::OpenHelmRelease(row.key.clone());
                    cx.emit(ScreenEvent::Command(open));
                }
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

    pub fn list_focus(&self, cx: &gpui::App) -> gpui::FocusHandle {
        gpui::Focusable::focus_handle(&self.table, cx)
    }

    pub fn header_note(&self) -> String {
        let counts = Counts::of(self.rows.iter().map(|row| row.status));
        let flagged = counts.flagged();
        if flagged > 0 {
            format!("{flagged} of {} need attention", self.rows.len())
        } else {
            self.rows.len().to_string()
        }
    }

    pub fn set(&mut self, cluster: &ClusterSummary, snapshot: &Snapshot, cx: &mut Context<Self>) {
        let rows: Vec<HelmRelease> = snapshot
            .helm_releases(&cluster.id)
            .into_iter()
            .cloned()
            .collect();
        let switched = self.cluster.as_ref() != Some(&cluster.id);
        if !switched && rows == self.rows {
            return;
        }
        self.cluster = Some(cluster.id.clone());
        self.cluster_name.clone_from(&cluster.name);
        self.rows = rows;
        self.refresh(cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let (query, chip) = {
            let bar = self.filter.read(cx);
            (bar.query().clone(), bar.chip())
        };
        let base: Vec<HelmRelease> = filter(&self.rows, &query).into_iter().cloned().collect();
        let chips = StatusFilter::counts(base.iter().map(|row| row.status))
            .into_iter()
            .map(|(filter, n)| chip_label(filter.label(), n))
            .collect();
        let chip = StatusFilter::at(chip);
        let rows: Vec<HelmRelease> = base.into_iter().filter(|r| chip.keeps(r.status)).collect();
        self.shown = rows.len();
        let (shown, total) = (self.shown, self.rows.len());
        self.filter.update(cx, |bar, cx| {
            bar.set_chips(chips, cx);
            bar.set_matches(shown, total, cx);
        });
        let scope = self.cluster.as_ref().map(|c| c.as_str().to_owned());
        let screen = cx.weak_entity();
        let actions = move |row: &HelmRelease| -> Vec<MenuItem> {
            let open_screen = screen.clone();
            let open = new_tab_item(Command::OpenHelmRelease(row.key.clone()), move |c, cx| {
                let _ = open_screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(c)));
            });
            let flux = row.flux.clone().map(|key| {
                let flux_screen = screen.clone();
                MenuItem::new(OPEN_FLUX, move |_, cx| {
                    let command = Command::OpenFluxObject(key.clone());
                    let _ = flux_screen.update(cx, |_, cx| cx.emit(ScreenEvent::Command(command)));
                })
                .description(OPEN_FLUX_DESCRIPTION)
            });
            std::iter::once(open).chain(flux).collect()
        };
        let words = query.words.clone();
        self.table.update(cx, |table, cx| {
            if let Some(scope) = scope {
                table.set_scope(scope, cx);
            }
            table.set_highlight(words, cx);
            table.set_row_actions(actions, cx);
            table.set_rows(rows, cx);
        });
        cx.notify();
    }

    fn empty_state(&self, cx: &mut Context<Self>) -> Option<EmptyState> {
        let shape = ListShape {
            shown: self.shown,
            total: self.rows.len(),
            namespace: None,
            noun: Noun::of(Page::Releases),
            cluster: &self.cluster_name,
        };
        empty_list(&shape, &self.filter, None, cx)
    }
}

impl Render for HelmReleasesScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = match self.empty_state(cx) {
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

fn columns() -> Vec<Column<HelmRelease>> {
    vec![
        Column::status_text(
            "Name",
            |r: &HelmRelease| r.status,
            |r: &HelmRelease| r.key.name.clone().into(),
        )
        .highlight()
        .mono()
        .flex(NAME_WEIGHT, NAME_MIN)
        .sort_by_key(|r: &HelmRelease| r.key.name.clone()),
        Column::colored_text(
            "Namespace",
            |r: &HelmRelease| r.key.namespace.clone().into(),
            |_, p| p.fg2,
        )
        .highlight()
        .fixed(NAMESPACE_WIDTH)
        .sort_by_key(|r: &HelmRelease| r.key.namespace.clone()),
        Column::colored_text(
            "Chart",
            |r: &HelmRelease| r.chart_label().into(),
            |_, p| p.fg2,
        )
        .mono()
        .fixed(CHART_WIDTH)
        .sort_by_key(HelmRelease::chart_label),
        Column::colored_text(
            "App version",
            |r: &HelmRelease| r.app_version.clone().into(),
            |_, p| p.fg2,
        )
        .mono()
        .fixed(APP_WIDTH)
        .sort_by_key(|r: &HelmRelease| r.app_version.clone()),
        Column::colored_text(
            "Revision",
            |r: &HelmRelease| r.revision.to_string().into(),
            |_, p| p.fg2,
        )
        .mono()
        .end()
        .fixed(REVISION_WIDTH)
        .sort_by_key(|r: &HelmRelease| r.revision),
        Column::colored_text(
            "Status",
            |r: &HelmRelease| r.state.clone().into(),
            |r, p| {
                if r.status == Status::Ok {
                    p.fg2
                } else {
                    p.status(r.status)
                }
            },
        )
        .mono()
        .fixed(STATE_WIDTH)
        .sort_by_key(|r: &HelmRelease| r.state.clone()),
        Column::colored_text(
            "Updated",
            |r: &HelmRelease| r.updated.map(age).unwrap_or_default().into(),
            |_, p| p.mute,
        )
        .mono()
        .fixed(AGE_WIDTH)
        .sort_by_key(|r: &HelmRelease| r.updated),
        Column::colored_text(
            "Flux",
            |r: &HelmRelease| flux_label(r).into(),
            |r, p| if r.flux.is_some() { p.acc } else { p.mute },
        )
        .mono()
        .flex(FLUX_WEIGHT, FLUX_MIN)
        .sort_by_key(flux_label),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Values {
    User,
    Computed,
}

pub struct HelmReleaseScreen {
    pub(crate) navigation: Option<Entity<crate::kit::HeaderNavigation>>,
    key: Option<HelmKey>,
    release: Option<HelmRelease>,
    detail: Option<HelmReleaseDetail>,
    loaded: bool,
    values: Values,
    history: Entity<Table<HelmRevision>>,
}

impl EventEmitter<Command> for HelmReleaseScreen {}

impl HelmReleaseScreen {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let history = cx.new(|cx| {
            Table::new("helm-history", history_columns(), cx)
                .row_key(|r: &HelmRevision| r.revision.to_string().into())
                .fit_content()
        });
        Self {
            navigation: None,
            key: None,
            release: None,
            detail: None,
            loaded: false,
            values: Values::User,
            history,
        }
    }

    pub fn set(&mut self, key: Option<&HelmKey>, snapshot: &Snapshot, cx: &mut Context<Self>) {
        let Some(key) = key else {
            return;
        };
        let release = snapshot.helm_release(key).cloned();
        let detail = snapshot.helm_detail(key).cloned();
        let loaded = snapshot
            .page_state(&key.cluster, Page::Releases)
            .is_some_and(flock_core::WatchState::is_loaded);
        let same = self.key.as_ref() == Some(key)
            && self.release == release
            && self.detail == detail
            && self.loaded == loaded;
        if same {
            return;
        }
        if self.key.as_ref() != Some(key) {
            self.values = Values::User;
        }
        let rows = detail
            .as_ref()
            .map(|detail| detail.history.clone())
            .unwrap_or_default();
        self.history
            .update(cx, |table, cx| table.set_rows(rows, cx));
        self.key = Some(key.clone());
        self.release = release;
        self.detail = detail;
        self.loaded = loaded;
        cx.notify();
    }

    fn header(&self, cx: &mut Context<Self>) -> PageHeader {
        let name = self
            .key
            .as_ref()
            .map(|k| k.name.clone())
            .unwrap_or_default();
        let header = PageHeader::new(name)
            .leading(self.navigation.clone())
            .mono();
        let Some(release) = &self.release else {
            return header;
        };
        let mut meta = vec![
            release.chart_label(),
            release.key.namespace.clone(),
            format!("{REVISION_PREFIX}{}", release.revision),
        ];
        if release.flux.is_some() {
            meta.push(MANAGED_BY.to_owned());
        }
        let header = header
            .status(release.status, release.state.clone())
            .meta(meta.join(SEPARATOR));
        match release.flux.clone() {
            Some(key) => header.action(Button::new("helm-open-flux", OPEN_FLUX).small().on_click(
                cx.listener(move |_, _, _, cx| cx.emit(Command::OpenFluxObject(key.clone()))),
            )),
            None => header,
        }
    }

    fn values(&self, detail: &HelmReleaseDetail, cx: &mut Context<Self>) -> AnyElement {
        let user = Button::new("helm-values-user", USER_SUPPLIED)
            .small()
            .selected(self.values == Values::User)
            .on_click(cx.listener(|this, _, _, cx| {
                this.values = Values::User;
                cx.notify();
            }));
        let computed = Button::new("helm-values-computed", COMPUTED)
            .small()
            .selected(self.values == Values::Computed)
            .on_click(cx.listener(|this, _, _, cx| {
                this.values = Values::Computed;
                cx.notify();
            }));
        let text = match self.values {
            Values::User => &detail.user_values,
            Values::Computed => &detail.computed_values,
        };
        let text = if text.is_empty() {
            NO_VALUES.to_owned()
        } else {
            text.clone()
        };
        let id = match self.values {
            Values::User => "helm-user-values",
            Values::Computed => "helm-computed-values",
        };
        div()
            .child(
                SectionHeader::new(VALUES)
                    .end(div().flex().gap(SPACE_3).child(user).child(computed)),
            )
            .child(
                div()
                    .debug_selector(move || id.to_owned())
                    .px(SPACE_8)
                    .pb(SPACE_8)
                    .child(CodeView::new(id, text).yaml()),
            )
            .into_any_element()
    }
}

impl Render for HelmReleaseScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let root = div()
            .debug_selector(|| "helm-release".to_owned())
            .size_full()
            .flex()
            .flex_col()
            .bg(palette.editor)
            .child(self.header(cx));
        let Some(detail) = self.detail.clone() else {
            if self.loaded {
                return root.child(EmptyState::new(GONE).selector("helm-release-gone"));
            }
            return root.child(Skeleton::rows());
        };
        let history = div()
            .child(SectionHeader::new(HISTORY).count(detail.history.len().to_string()))
            .child(
                div()
                    .border_t(RULE)
                    .border_color(palette.line)
                    .child(self.history.clone()),
            );
        let manifest = div().child(SectionHeader::new(MANIFEST)).child(
            div()
                .px(SPACE_8)
                .pb(SPACE_8)
                .child(CodeView::new("helm-manifest", detail.manifest.clone()).yaml()),
        );
        let values = self.values(&detail, cx);
        root.child(
            div()
                .id("helm-release-body")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(history)
                .child(values)
                .child(manifest),
        )
    }
}

fn history_columns() -> Vec<Column<HelmRevision>> {
    vec![
        Column::new("", |r: &HelmRevision, _| {
            StatusGlyph::new(r.status).into_any_element()
        })
        .fixed(GLYPH_WIDTH),
        Column::colored_text(
            "Revision",
            |r: &HelmRevision| r.revision.to_string().into(),
            |_, p| p.fg2,
        )
        .mono()
        .end()
        .fixed(REVISION_WIDTH),
        Column::colored_text(
            "Status",
            |r: &HelmRevision| r.state.clone().into(),
            |r, p| {
                if r.status == Status::Ok {
                    p.fg2
                } else {
                    p.status(r.status)
                }
            },
        )
        .mono()
        .fixed(STATE_WIDTH),
        Column::colored_text(
            "Chart",
            |r: &HelmRevision| r.chart_version.clone().into(),
            |_, p| p.fg2,
        )
        .mono()
        .fixed(APP_WIDTH),
        Column::colored_text(
            "Description",
            |r: &HelmRevision| r.description.clone().into(),
            |_, p| p.fg2,
        )
        .flex(DESCRIPTION_WEIGHT, NAME_MIN),
        Column::colored_text(
            "Updated",
            |r: &HelmRevision| age(r.age).into(),
            |_, p| p.mute,
        )
        .mono()
        .fixed(AGE_WIDTH),
    ]
}
