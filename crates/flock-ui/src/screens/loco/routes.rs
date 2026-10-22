use flock_core::emptiness::Noun;
use flock_core::filter::label_values;
use flock_core::loco::{Listener, Route, RouteFilter, Routes};
use flock_core::{Page, Status};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, Hsla, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window, div, px,
};

use super::LocoEvent;
use crate::kit::status_symbols;
use crate::kit::{
    Column, FilterBar, FilterEvent, PageTitle, SectionHeader, StatusGlyph, Table, TableEvent,
    chip_label,
};
use crate::screens::{ListShape, empty_list};
use crate::theme::{
    ActiveTheme, FONT_MONO, Palette, ROW_TABLE, RULE, SPACE_2, SPACE_8, TEXT_SMALL,
};

const TITLE: &str = "Routes";
const GLYPH: Pixels = px(14.);
const LISTENER_NAME: Pixels = px(140.);
const LISTENER_HOST: Pixels = px(170.);
const LISTENER_PORT: Pixels = px(70.);
const LISTENER_ROUTES: Pixels = px(60.);
const ROUTE_HOST: Pixels = px(210.);
const ROUTE_BACKEND_MIN: Pixels = px(180.);
const ROUTE_READY: Pixels = px(64.);
const ROUTE_STATUS_MIN: Pixels = px(200.);
const ROUTE_STATUS_WEIGHT: f32 = 1.2;
const MISSING: &str = "—";

pub struct RoutesScreen {
    cluster_name: String,
    shown: usize,
    pub(crate) navigation: Option<Entity<crate::kit::HeaderNavigation>>,
    routes: Routes,
    filter: Entity<FilterBar>,
    listeners: Entity<Table<Listener>>,
    table: Entity<Table<Route>>,
    _subscriptions: [Subscription; 2],
}

impl EventEmitter<LocoEvent> for RoutesScreen {}

impl RoutesScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| FilterBar::new("loco-route-filter", window, cx));
        let listeners = cx.new(|cx| {
            Table::new("loco-listeners", listener_columns(), cx)
                .row_height(ROW_TABLE)
                .fit_content()
        });
        let table = cx.new(|cx| {
            Table::new("loco-routes", route_columns(), cx)
                .row_height(ROW_TABLE)
                .click_opens()
                .fit_content()
        });
        let subscription = cx.subscribe(&table, |_, table, event: &TableEvent, cx| {
            let TableEvent::Opened(ix) = *event else {
                return;
            };
            let app = table.read(cx).row(ix).and_then(|r| r.application.clone());
            if let Some(app) = app {
                cx.emit(LocoEvent::ShowApplication(app));
            }
        });
        let filtered = cx.subscribe(&filter, |this, _, FilterEvent::Changed, cx| {
            this.refresh(cx);
        });
        Self {
            cluster_name: String::new(),
            shown: 0,
            navigation: None,
            routes: Routes::default(),
            filter,
            listeners,
            table,
            _subscriptions: [subscription, filtered],
        }
    }

    pub fn set_cluster_name(&mut self, name: &str) {
        self.cluster_name.clear();
        self.cluster_name.push_str(name);
    }

    pub fn set_routes(&mut self, routes: Routes, cx: &mut Context<Self>) {
        let listeners = routes.listeners.clone();
        self.listeners
            .update(cx, |table, cx| table.set_rows(listeners, cx));
        self.routes = routes;
        self.refresh(cx);
    }

    pub fn filter_bar(&self) -> &Entity<FilterBar> {
        &self.filter
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let labels = label_values(self.routes.routes.iter().map(|route| &route.labels));
        self.filter.update(cx, |bar, cx| bar.set_labels(labels, cx));
        let (query, chip) = {
            let bar = self.filter.read(cx);
            (bar.query().clone(), bar.chip())
        };
        let chips = self
            .routes
            .filter_counts(&query)
            .into_iter()
            .map(|(filter, n)| chip_label(filter.label(), n))
            .collect();
        let filter = RouteFilter::ALL.get(chip).copied().unwrap_or_default();
        let rows = self.routes.ranked(filter, &query);
        let shown = rows.len();
        self.shown = shown;
        let total = self.routes.routes.len();
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

    fn routes_header(&self, palette: &Palette) -> impl IntoElement {
        let problems = self.routes.problems();
        let count = div()
            .flex()
            .font_family(FONT_MONO)
            .text_size(TEXT_SMALL)
            .text_color(palette.mute)
            .child(self.routes.total.to_string())
            .children((problems > 0).then(|| {
                div()
                    .flex()
                    .child(" · ")
                    .child(div().text_color(palette.warn).child(status_symbols(format!(
                        "{} {problems}",
                        Status::Warn.glyph()
                    ))))
            }));
        SectionHeader::new("HTTPRoutes").annotation(count)
    }
}

impl Render for RoutesScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let header = self.routes_header(&palette);
        let shape = ListShape {
            shown: self.shown,
            total: self.routes.routes.len(),
            namespace: None,
            noun: Noun::of(Page::LocoRoutes),
            cluster: &self.cluster_name,
        };
        let routes: AnyElement = match empty_list(&shape, &self.filter, None, cx) {
            Some(empty) => empty.into_any_element(),
            None => self.table.clone().into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                PageTitle::new(TITLE)
                    .leading(self.navigation.clone())
                    .detail(self.routes.subtitle()),
            )
            .child(self.filter.clone())
            .child(
                div()
                    .id("loco-routes-page")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(SectionHeader::new("Listeners"))
                    .child(
                        div()
                            .border_t(RULE)
                            .border_color(palette.line)
                            .child(self.listeners.clone()),
                    )
                    .child(div().pt(SPACE_2).child(header))
                    .child(
                        div()
                            .pb(SPACE_8)
                            .border_t(RULE)
                            .border_color(palette.line)
                            .child(routes),
                    ),
            )
    }
}

fn text(value: impl Into<SharedString>, color: Hsla) -> AnyElement {
    div()
        .truncate()
        .text_color(color)
        .child(value.into())
        .into_any_element()
}

fn small(value: impl Into<SharedString>, color: Hsla) -> AnyElement {
    div()
        .truncate()
        .text_size(TEXT_SMALL)
        .text_color(color)
        .child(value.into())
        .into_any_element()
}

fn listener_columns() -> Vec<Column<Listener>> {
    vec![
        Column::new("", |l: &Listener, _| {
            StatusGlyph::new(l.status).into_any_element()
        })
        .fixed(GLYPH),
        Column::new("Listener", |l: &Listener, palette| {
            text(l.name.clone(), palette.fg)
        })
        .mono()
        .fixed(LISTENER_NAME),
        Column::new("Hostname", |l: &Listener, palette| {
            let host = l.hostname.clone().unwrap_or_else(|| MISSING.to_owned());
            text(host, palette.fg2)
        })
        .mono()
        .fixed(LISTENER_HOST),
        Column::new("Port", |l: &Listener, palette| {
            text(l.port_label(), palette.fg2)
        })
        .mono()
        .fixed(LISTENER_PORT),
        Column::new("Routes", |l: &Listener, palette| {
            text(l.routes_label(), palette.fg2)
        })
        .mono()
        .fixed(LISTENER_ROUTES),
        Column::new("TLS", |l: &Listener, palette| {
            text(l.tls.clone(), palette.mute)
        })
        .mono()
        .flex(1., Pixels::ZERO),
    ]
}

fn route_columns() -> Vec<Column<Route>> {
    vec![
        Column::new("", |r: &Route, _| {
            StatusGlyph::new(r.status).into_any_element()
        })
        .fixed(GLYPH),
        Column::text("Host", |r: &Route| r.host_label().to_owned().into())
            .highlight()
            .mono()
            .fixed(ROUTE_HOST),
        Column::new("Backend", |r: &Route, palette| {
            small(r.backend.clone(), palette.fg2)
        })
        .mono()
        .flex(1., ROUTE_BACKEND_MIN),
        Column::new("Ready", |r: &Route, palette| {
            let color = if r.no_ready_endpoints() {
                palette.warn
            } else {
                palette.fg2
            };
            text(r.ready_label(), color)
        })
        .mono()
        .fixed(ROUTE_READY),
        Column::new("Status", |r: &Route, palette| {
            let color = if r.status == Status::Ok {
                palette.mute
            } else {
                palette.status(r.status)
            };
            text(r.detail.clone(), color)
        })
        .flex(ROUTE_STATUS_WEIGHT, ROUTE_STATUS_MIN),
    ]
}
