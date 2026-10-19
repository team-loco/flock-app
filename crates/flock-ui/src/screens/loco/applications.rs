use flock_core::emptiness::Noun;
use flock_core::filter::{TextFilter, label_values};
use flock_core::loco::{
    AppFilter, Application, Applications, ControllerObject, LocoCommand, Workspace, short_digest,
};
use flock_core::{Command, LogTarget, Page, ResourceRef, Status};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FontWeight, Hsla,
    InteractiveElement, IntoElement, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window, div,
    prelude::FluentBuilder, px, relative,
};

use crate::kit::{
    Button, Column, FilterBar, FilterEvent, InspectorPane, KeyValue, Link, MenuItem, PageTitle,
    SectionHeader, StatusGlyph, StatusText, Table, TableEvent, chip_label,
};
use crate::screens::{ListShape, empty_list};
use crate::theme::{
    ActiveTheme, FONT_MONO, LINE_PROSE, Palette, RADIUS, ROW_TABLE_MAX, ROW_TABLE_MIN, RULE,
    SPACE_3, SPACE_4, SPACE_5, SPACE_6, SPACE_7, SPACE_8, TEXT_BODY, TEXT_HEADING, TEXT_LABEL,
    TEXT_MONO, TEXT_SMALL,
};

const DETAIL_WIDTH: Pixels = px(440.);
const GLYPH: Pixels = px(14.);
const NAME_MIN: Pixels = px(100.);
const ENVIRONMENT: Pixels = px(84.);
const REPLICAS: Pixels = px(56.);
const REVISION: Pixels = px(64.);
const OBJECT_KIND: Pixels = px(120.);
const DETAIL_LABEL: Pixels = px(110.);
const ROUTE_LINE: f32 = 1.7;

const TITLE: &str = "Applications";
const LOGS: &str = "Logs";
const NOTHING_SELECTED: &str = "Select an application to see what the controller made of it.";
const SEPARATOR: &str = " · ";
const MISSING: &str = "—";
const ROUTE_LINK: &str = "→";

#[derive(Clone)]
enum AppRow {
    Group(Workspace),
    App(Box<Application>),
}

impl AppRow {
    fn key(&self) -> SharedString {
        match self {
            Self::Group(workspace) => workspace.namespace.clone().into(),
            Self::App(app) => app.resource.to_string().into(),
        }
    }
}

pub struct ApplicationsScreen {
    cluster_name: String,
    shown: usize,
    pub(crate) navigation: Option<Entity<crate::kit::HeaderNavigation>>,
    applications: Applications,
    filter: Entity<FilterBar>,
    selected: Option<ResourceRef>,
    list: Entity<Table<AppRow>>,
    objects: Entity<Table<ControllerObject>>,
    _subscriptions: [Subscription; 3],
}

impl EventEmitter<Command> for ApplicationsScreen {}

impl ApplicationsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| FilterBar::new("loco-app-filter", window, cx));
        let screen = cx.weak_entity();
        let list = cx.new(|cx| {
            Table::new("loco-apps", app_columns(), cx)
                .row_actions(move |row: &AppRow| match row {
                    AppRow::App(app) => Self::menu_items(app, &screen),
                    AppRow::Group(_) => Vec::new(),
                })
                .headerless()
                .row_height(ROW_TABLE_MAX)
                .column_gap(SPACE_5)
                .span_rows(group_row)
                .row_key(AppRow::key)
        });
        let objects = cx.new(|cx| {
            Table::new("loco-app-objects", object_columns(), cx)
                .headerless()
                .row_height(ROW_TABLE_MIN)
                .column_gap(SPACE_5)
                .click_opens()
                .unruled()
                .fit_content()
        });
        let subscriptions = [
            cx.subscribe(&list, |this, table, event: &TableEvent, cx| {
                let TableEvent::Selected(ix) = *event else {
                    return;
                };
                let picked = match table.read(cx).row(ix) {
                    Some(AppRow::App(app)) => Some(app.resource.clone()),
                    Some(AppRow::Group(_)) => None,
                    None => None,
                };
                if picked.is_some() {
                    this.select(picked, cx);
                }
            }),
            cx.subscribe(&objects, |this, table, event: &TableEvent, cx| {
                let TableEvent::Opened(ix) = *event else {
                    return;
                };
                let cluster = this.selected.as_ref().map(|app| app.cluster.clone());
                let target = cluster.and_then(|cluster| {
                    table
                        .read(cx)
                        .row(ix)
                        .and_then(|object| object.reference(&cluster))
                });
                if let Some(target) = target {
                    cx.emit(Command::OpenResource(target));
                }
            }),
            cx.subscribe(&filter, |this, _, FilterEvent::Changed, cx| {
                this.refresh_list(cx);
                let selected = this.selected.clone();
                this.select(selected, cx);
            }),
        ];
        Self {
            cluster_name: String::new(),
            shown: 0,
            navigation: None,
            applications: Applications::default(),
            filter,
            selected: None,
            list,
            objects,
            _subscriptions: subscriptions,
        }
    }

    pub fn set_applications(&mut self, applications: Applications, cx: &mut Context<Self>) {
        self.applications = applications;
        let still_there = self
            .selected
            .as_ref()
            .is_some_and(|selected| self.applications.find(selected).is_some());
        let selected = if still_there {
            self.selected.clone()
        } else {
            self.applications
                .iter()
                .next()
                .map(|(_, app)| app.resource.clone())
        };
        self.refresh_list(cx);
        self.select(selected, cx);
    }

    pub fn select(&mut self, selected: Option<ResourceRef>, cx: &mut Context<Self>) {
        self.selected = selected;
        let app = self.selected_app().map(|(_, app)| app.clone());
        let objects = app
            .as_ref()
            .map(|app| app.objects.clone())
            .unwrap_or_default();
        self.objects
            .update(cx, |table, cx| table.set_rows(objects, cx));
        let row = self.selected_row(cx);
        self.list.update(cx, |table, cx| table.select(row, cx));
        cx.notify();
    }

    fn selected_row(&self, cx: &Context<Self>) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        let key: SharedString = selected.to_string().into();
        let table = self.list.read(cx);
        (0..)
            .map_while(|ix| table.row(ix))
            .position(|row| row.key() == key)
    }

    fn selected_app(&self) -> Option<(&Workspace, &Application)> {
        self.selected
            .as_ref()
            .and_then(|selected| self.applications.find(selected))
    }

    pub fn set_cluster_name(&mut self, name: &str) {
        self.cluster_name.clear();
        self.cluster_name.push_str(name);
    }

    pub fn filter_bar(&self) -> &Entity<FilterBar> {
        &self.filter
    }

    fn refresh_list(&mut self, cx: &mut Context<Self>) {
        let labels = label_values(
            self.applications
                .iter()
                .map(|(_, application)| &application.labels),
        );
        self.filter.update(cx, |bar, cx| bar.set_labels(labels, cx));
        let (query, chip) = {
            let bar = self.filter.read(cx);
            (bar.query().clone(), bar.chip())
        };
        let chips = self
            .applications
            .filter_counts(&query)
            .into_iter()
            .map(|(filter, n)| chip_label(filter.label(), n))
            .collect();
        let filter = AppFilter::ALL.get(chip).copied().unwrap_or_default();
        let rows = app_rows(&self.applications, filter, &query);
        let shown = rows
            .iter()
            .filter(|row| matches!(row, AppRow::App(_)))
            .count();
        self.shown = shown;
        let total = self.applications.iter().count();
        self.filter.update(cx, |bar, cx| {
            bar.set_chips(chips, cx);
            bar.set_matches(shown, total, cx);
        });
        let words = query.words;
        self.list.update(cx, |table, cx| {
            table.set_highlight(words, cx);
            table.set_rows(rows, cx);
        });
    }

    fn menu_items(app: &Application, screen: &WeakEntity<Self>) -> Vec<MenuItem> {
        let this = screen.clone();
        let emit = move |command: Command| {
            let this = this.clone();
            move |_: &mut Window, cx: &mut gpui::App| {
                let command = command.clone();
                let sent = this.update(cx, |_, cx| cx.emit(command));
                if sent.is_err() {
                    tracing::debug!("applications screen closed before its menu");
                }
            }
        };
        let restart = Command::Loco(LocoCommand::RestartApplication(app.seen()));
        let mut items = vec![
            MenuItem::new("Restart", emit(restart))
                .description("Roll the Deployment's pods one at a time; traffic keeps flowing."),
        ];
        if let Some(deployment) = app.deployment() {
            items.push(
                MenuItem::new("Open deployment", emit(Command::OpenResource(deployment)))
                    .description("Pods, rollout history and events for this application."),
            );
        }
        items.push(
            MenuItem::new("Show routes", emit(Command::SelectPage(Page::LocoRoutes)))
                .description("Listeners on Gateway eg and every HTTPRoute, problems first."),
        );
        items
    }

    fn header(&self) -> impl IntoElement {
        PageTitle::new(TITLE)
            .leading(self.navigation.clone())
            .detail(self.applications.total.to_string())
    }

    fn detail(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some((workspace, app)) = self.selected_app() else {
            return div()
                .px(SPACE_8)
                .pt(SPACE_7)
                .text_color(palette.mute)
                .child(NOTHING_SELECTED)
                .into_any_element();
        };
        let summary = Self::summary(workspace, app, palette, cx);
        let deployment = deployment_section(app, palette);
        let route = Self::route_section(app, palette, cx);
        div()
            .id("loco-app-detail")
            .flex()
            .flex_col()
            .size_full()
            .overflow_y_scroll()
            .child(summary)
            .child(deployment)
            .child(SectionHeader::new("Created by the controller"))
            .child(
                div()
                    .border_b(RULE)
                    .border_color(palette.line)
                    .child(self.objects.clone()),
            )
            .child(route)
            .into_any_element()
    }

    fn summary(
        workspace: &Workspace,
        app: &Application,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let logs_target = app.deployment();
        let logs = Button::new("loco-app-logs", LOGS)
            .disabled(logs_target.is_none())
            .on_click(cx.listener(move |_, _, _, cx| {
                if let Some(target) = logs_target.clone() {
                    cx.emit(Command::FollowLogs(LogTarget::Workload(target)));
                }
            }));
        let status = div()
            .text_size(TEXT_BODY)
            .child(StatusText::new(app.phase.status(), app.phase.label()));
        let top = div()
            .flex()
            .items_center()
            .gap(SPACE_5)
            .child(
                div()
                    .text_size(TEXT_HEADING)
                    .font_weight(FontWeight::SEMIBOLD)
                    .truncate()
                    .child(app.name.clone()),
            )
            .child(status)
            .child(div().flex_1())
            .child(logs);
        let problem = app.degraded.as_ref().map(|degraded| {
            div()
                .mt(SPACE_3)
                .px(SPACE_5)
                .py(SPACE_4)
                .rounded(RADIUS)
                .bg(palette.errbg)
                .border(RULE)
                .border_color(palette.err)
                .text_size(TEXT_BODY)
                .line_height(relative(LINE_PROSE))
                .child(degraded.message.clone())
                .child(
                    div()
                        .font_family(FONT_MONO)
                        .text_size(TEXT_SMALL)
                        .text_color(palette.mute)
                        .child(degraded.summary()),
                )
        });
        div()
            .flex()
            .flex_col()
            .gap(SPACE_3)
            .px(SPACE_8)
            .pt(SPACE_7)
            .pb(SPACE_6)
            .border_b(RULE)
            .border_color(palette.line)
            .child(top)
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(TEXT_MONO)
                    .text_color(palette.mute)
                    .child(location(workspace, app)),
            )
            .children(problem)
            .into_any_element()
    }

    fn route_section(app: &Application, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let summary = app
            .route
            .as_ref()
            .map_or_else(|| MISSING.to_owned(), flock_core::loco::AppRoute::summary);
        let link = Link::new("loco-app-route", ROUTE_LINK).on_click(cx.listener(|_, _, _, cx| {
            cx.emit(Command::SelectPage(Page::LocoRoutes));
        }));
        div()
            .flex()
            .flex_col()
            .child(SectionHeader::new("Route"))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(SPACE_3)
                    .px(SPACE_8)
                    .pb(SPACE_8)
                    .font_family(FONT_MONO)
                    .text_size(TEXT_MONO)
                    .line_height(relative(ROUTE_LINE))
                    .text_color(palette.fg2)
                    .child(div().min_w_0().child(summary))
                    .child(
                        div()
                            .flex_none()
                            .font_family(crate::theme::FONT_SANS)
                            .child(link),
                    ),
            )
            .into_any_element()
    }
}

impl Render for ApplicationsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let shape = ListShape {
            shown: self.shown,
            total: self.applications.iter().count(),
            namespace: None,
            noun: Noun::of(Page::LocoApplications),
            cluster: &self.cluster_name,
        };
        let rows: AnyElement = match empty_list(&shape, &self.filter, None, cx) {
            Some(empty) => empty.into_any_element(),
            None => self.list.clone().into_any_element(),
        };
        let list = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .border_r(RULE)
            .border_color(palette.line)
            .child(self.header())
            .child(self.filter.clone())
            .child(div().flex_1().min_h_0().child(rows));
        let content = self.detail(&palette, cx);
        let detail = InspectorPane::new("loco-application-inspector", content).width(DETAIL_WIDTH);
        div().flex().size_full().child(list).child(detail)
    }
}

fn location(workspace: &Workspace, app: &Application) -> String {
    let parts = [
        Some(workspace.label()),
        Some(app.environment.as_str()),
        app.region.as_deref(),
        app.hostname.as_deref(),
    ];
    parts
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(SEPARATOR)
}

fn deployment_section(app: &Application, palette: &Palette) -> impl IntoElement {
    let value = |text: Option<String>| text.unwrap_or_else(|| MISSING.to_owned());
    let image = value(app.image.as_deref().map(short_digest));
    let rollout = value(app.rollout.as_ref().map(flock_core::loco::Rollout::summary));
    let resources = value(
        app.resources
            .as_ref()
            .map(flock_core::loco::Resources::summary),
    );
    let health = value(
        app.health_check
            .as_ref()
            .map(flock_core::loco::HealthCheck::summary),
    );
    let grid = KeyValue::new()
        .label_width(DETAIL_LABEL)
        .text("Image", image)
        .text("Deployment", rollout)
        .text("Placement", app.placement.detail())
        .text("Resources", resources)
        .text("Health check", health);
    div()
        .flex()
        .flex_col()
        .border_b(RULE)
        .border_color(palette.line)
        .child(SectionHeader::new("Deployment"))
        .child(
            div()
                .px(SPACE_8)
                .pb(SPACE_6)
                .text_size(TEXT_BODY)
                .child(grid),
        )
}

fn app_rows(applications: &Applications, filter: AppFilter, query: &TextFilter) -> Vec<AppRow> {
    applications
        .filtered(filter, query)
        .into_iter()
        .flat_map(|group| {
            let head = AppRow::Group(group.workspace);
            let apps = group
                .applications
                .into_iter()
                .map(|app| AppRow::App(Box::new(app)));
            std::iter::once(head).chain(apps)
        })
        .collect()
}

fn group_row(row: &AppRow, palette: &Palette) -> Option<AnyElement> {
    let AppRow::Group(workspace) = row else {
        return None;
    };
    let element = div()
        .flex()
        .items_center()
        .gap(SPACE_5)
        .size_full()
        .px(SPACE_8)
        .bg(palette.panel)
        .child(
            div()
                .text_size(TEXT_BODY)
                .font_weight(FontWeight::SEMIBOLD)
                .child(workspace.label().to_owned()),
        )
        .when(workspace.name.is_some(), |row| {
            row.child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(TEXT_LABEL)
                    .text_color(palette.mute)
                    .child(workspace.namespace.clone()),
            )
        })
        .into_any_element();
    Some(element)
}

fn text(value: impl Into<SharedString>, color: Hsla) -> AnyElement {
    div()
        .truncate()
        .text_color(color)
        .child(value.into())
        .into_any_element()
}

fn quiet_when_ok(status: Status, palette: &Palette) -> Hsla {
    if status == Status::Ok {
        palette.fg2
    } else {
        palette.status(status)
    }
}

fn app_columns() -> Vec<Column<AppRow>> {
    vec![
        Column::new("", |row: &AppRow, _| match row {
            AppRow::App(app) => StatusGlyph::new(app.phase.status()).into_any_element(),
            AppRow::Group(_) => div().into_any_element(),
        })
        .fixed(GLYPH),
        Column::new("Name", |row: &AppRow, _| match row {
            AppRow::App(app) => div()
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .child(app.name.clone())
                .into_any_element(),
            AppRow::Group(_) => div().into_any_element(),
        })
        .flex(1., NAME_MIN),
        Column::new("Environment", |row: &AppRow, palette| match row {
            AppRow::App(app) => text(app.environment.clone(), palette.mute),
            AppRow::Group(_) => div().into_any_element(),
        })
        .fixed(ENVIRONMENT),
        Column::new("Replicas", |row: &AppRow, palette| match row {
            AppRow::App(app) => text(app.replicas(), quiet_when_ok(app.phase.status(), palette)),
            AppRow::Group(_) => div().into_any_element(),
        })
        .mono()
        .fixed(REPLICAS),
        Column::new("Revision", |row: &AppRow, palette| match row {
            AppRow::App(app) => div()
                .truncate()
                .text_size(TEXT_SMALL)
                .text_color(palette.mute)
                .child(app.placement.short())
                .into_any_element(),
            AppRow::Group(_) => div().into_any_element(),
        })
        .mono()
        .fixed(REVISION),
    ]
}

fn object_columns() -> Vec<Column<ControllerObject>> {
    vec![
        Column::new("", |object: &ControllerObject, _| {
            StatusGlyph::new(object.status).into_any_element()
        })
        .fixed(GLYPH),
        Column::new("Kind", |object: &ControllerObject, palette| {
            text(object.kind.clone(), palette.mute)
        })
        .fixed(OBJECT_KIND),
        Column::new("Object", |object: &ControllerObject, palette| {
            div()
                .truncate()
                .text_size(TEXT_SMALL)
                .text_color(quiet_when_ok(object.status, palette))
                .child(object.label())
                .into_any_element()
        })
        .mono()
        .flex(1., Pixels::ZERO),
    ]
}

#[cfg(test)]
mod tests {
    use flock_core::loco::{AppPhase, Placement, WorkspaceApps};
    use flock_core::{ClusterId, Uid};

    use super::*;

    fn app(name: &str, phase: AppPhase) -> Application {
        Application {
            labels: std::collections::BTreeMap::new(),
            resource: ResourceRef {
                cluster: ClusterId::new("c"),
                kind: "Application".into(),
                namespace: Some("loco-system".into()),
                name: name.into(),
            },
            name: name.into(),
            environment: "production".into(),
            region: Some("us-east".into()),
            phase,
            ready_replicas: None,
            desired_replicas: None,
            placement: Placement::default(),
            hostname: Some(format!("{name}.onloco.app")),
            degraded: None,
            image: None,
            rollout: None,
            resources: None,
            health_check: None,
            workload_uid: Uid::default(),
            objects: Vec::new(),
            route: None,
        }
    }

    #[test]
    fn rows_put_a_workspace_header_before_its_apps() {
        let workspace = Workspace {
            namespace: "ws-1".into(),
            name: Some("acme / web".into()),
        };
        let applications = Applications {
            total: 2,
            workspaces: vec![WorkspaceApps {
                workspace: workspace.clone(),
                applications: vec![app("web", AppPhase::Ready), app("docs", AppPhase::Failed)],
            }],
        };
        let shape: Vec<String> =
            app_rows(&applications, AppFilter::NotReady, &TextFilter::default())
                .iter()
                .map(|row| row.key().to_string())
                .collect();
        assert_eq!(shape, ["ws-1", "loco-system/docs"]);
        let first = applications.iter().next().map(|(w, a)| location(w, a));
        assert_eq!(
            first.as_deref(),
            Some("acme / web · production · us-east · web.onloco.app")
        );
    }
}
