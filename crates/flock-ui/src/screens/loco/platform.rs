use flock_core::loco::{
    AgentLevel, AgentLogLine, Application, BuildQueue, Component, LocoCluster, PlacementSummary,
    Workspace,
};
use flock_core::{Command, Page, Status};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FontWeight, Hsla,
    InteractiveElement, IntoElement, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, div, px,
};

use super::LocoEvent;
use crate::kit::status_symbols;
use crate::kit::{
    Column, KeyValue, Level, Link, LogLine, LogView, PageTitle, RawMessage, SectionHeader,
    StatusGlyph, Table, TableEvent,
};
use crate::theme::{
    ActiveTheme, FONT_MONO, LOG_LINE, Palette, ROW_TABLE, RULE, SPACE_2, SPACE_3, SPACE_4, SPACE_8,
    TEXT_SMALL,
};

const TITLE: &str = "Loco platform";
const AGENT_LABEL: Pixels = px(150.);
const GLYPH: Pixels = px(14.);
const PLACEMENT_NAME: Pixels = px(170.);
const PLACEMENT_WHERE: Pixels = px(180.);
const PLACEMENT_REVISION: Pixels = px(74.);
const COMPONENT_NAME: Pixels = px(200.);
const COMPONENT_NAMESPACE: Pixels = px(170.);
const COMPONENT_READY: Pixels = px(56.);
const COMPONENT_VERSION: Pixels = px(110.);
const UNAVAILABLE: &str =
    "not observable in-cluster · the agent reports it only to the control plane";
const NO_LOG: &str = "Agent logs need pod log access; none loaded yet.";
const ALL_SETTLED: &str = "Every placement is at its desired revision.";
const SEPARATOR: &str = " · ";
const BUILDS_LINK: &str = "view";

#[derive(Clone)]
struct PlacementRow {
    workspace: Workspace,
    app: Application,
}

pub struct PlatformScreen {
    pub(crate) navigation: Option<Entity<crate::kit::HeaderNavigation>>,
    loco: LocoCluster,
    placements: Entity<Table<PlacementRow>>,
    components: Entity<Table<Component>>,
    log: Entity<LogView>,
    _subscription: Subscription,
}

impl EventEmitter<LocoEvent> for PlatformScreen {}

impl PlatformScreen {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let placements = cx.new(|cx| {
            Table::new("loco-placements", placement_columns(), cx)
                .headerless()
                .row_height(ROW_TABLE)
                .click_opens()
                .empty(ALL_SETTLED)
                .fit_content()
        });
        let components = cx.new(|cx| {
            Table::new("loco-components", component_columns(), cx)
                .row_height(ROW_TABLE)
                .fit_content()
        });
        let log = cx.new(|_| {
            LogView::new("loco-agent-log")
                .inset(SPACE_8)
                .timestamp_width(Pixels::ZERO)
        });
        let subscription = cx.subscribe(&placements, |_, table, event: &TableEvent, cx| {
            let TableEvent::Opened(ix) = *event else {
                return;
            };
            let app = table.read(cx).row(ix).map(|row| row.app.resource.clone());
            if let Some(app) = app {
                cx.emit(LocoEvent::ShowApplication(app));
            }
        });
        Self {
            navigation: None,
            loco: LocoCluster::default(),
            placements,
            components,
            log,
            _subscription: subscription,
        }
    }

    pub fn set_loco(&mut self, loco: LocoCluster, cx: &mut Context<Self>) {
        let rows: Vec<PlacementRow> = loco
            .applications
            .unsettled()
            .into_iter()
            .map(|(workspace, app)| PlacementRow {
                workspace: workspace.clone(),
                app: app.clone(),
            })
            .collect();
        self.placements
            .update(cx, |table, cx| table.set_rows(rows, cx));
        let components = loco.platform.components.clone();
        self.components
            .update(cx, |table, cx| table.set_rows(components, cx));
        if loco.platform.log != self.loco.platform.log {
            let lines: Vec<LogLine> = loco.platform.log.iter().map(log_line).collect();
            self.log.update(cx, |log, cx| {
                log.clear(cx);
                log.extend(lines, cx);
            });
        }
        self.loco = loco;
        cx.notify();
    }

    fn agent_section(&self, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let agent = &self.loco.platform.agent;
        let stream = agent.stream().map_or_else(
            || unavailable(palette),
            |(status, line)| glyph_line(status, line, palette),
        );
        let heartbeat = agent.last_heartbeat.map_or_else(
            || unavailable(palette),
            |ago| plain(format!("{} ago", flock_core::time::age(ago))),
        );
        let health = match (agent.kubernetes_healthy, agent.controller_healthy) {
            (Some(kubernetes), Some(controller)) => div()
                .flex()
                .gap(SPACE_4)
                .child(glyph_line(healthy(kubernetes), "kubernetes", palette))
                .child(glyph_line(healthy(controller), "controller", palette))
                .into_any_element(),
            _ => unavailable(palette),
        };
        let reconnects = agent.reconnects.as_ref().map_or_else(
            || unavailable(palette),
            |reconnects| plain(reconnects.summary()),
        );
        let builds = self.builds_line(palette, cx);
        let grid = KeyValue::new()
            .label_width(AGENT_LABEL)
            .row("Sync stream", stream)
            .row("Last heartbeat", heartbeat)
            .row("Reported health", health)
            .row("Reconnects · 24h", reconnects)
            .row("Builds", builds);
        div()
            .flex()
            .flex_col()
            .child(SectionHeader::new("Agent ↔ control plane"))
            .child(
                div()
                    .px(SPACE_8)
                    .pb(SPACE_2)
                    .text_size(crate::theme::TEXT_BODY)
                    .child(grid),
            )
    }

    fn builds_line(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let BuildQueue { running, queued } = self.loco.builds.queue();
        let enabled = match self.loco.platform.agent.builds_enabled {
            Some(true) => "enabled",
            Some(false) => "disabled",
            None => "unknown",
        };
        let link =
            Link::new("loco-platform-builds", BUILDS_LINK).on_click(cx.listener(|_, _, _, cx| {
                cx.emit(LocoEvent::Command(Command::SelectPage(Page::LocoBuilds)));
            }));
        div()
            .flex()
            .flex_wrap()
            .text_color(palette.fg)
            .child(format!(
                "{enabled} · {running} running · {queued} queued{SEPARATOR}"
            ))
            .child(link)
            .into_any_element()
    }

    fn placements_section(&self, palette: &Palette) -> impl IntoElement {
        let summary = self.loco.applications.placement_summary();
        div()
            .flex()
            .flex_col()
            .pt(SPACE_2)
            .child(SectionHeader::new("Placements").annotation(placement_counts(&summary, palette)))
            .child(
                div()
                    .border_t(RULE)
                    .border_color(palette.line)
                    .child(self.placements.clone()),
            )
    }

    fn components_section(&self, palette: &Palette) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .pt(SPACE_2)
            .child(SectionHeader::new("Components"))
            .child(
                div()
                    .border_t(RULE)
                    .border_color(palette.line)
                    .child(self.components.clone()),
            )
    }

    fn log_section(&self, palette: &Palette) -> impl IntoElement {
        let lines = self.loco.platform.log.len();
        let body = if lines == 0 {
            div()
                .px(SPACE_8)
                .py(SPACE_3)
                .text_color(palette.mute)
                .child(NO_LOG)
                .into_any_element()
        } else {
            let rows = u16::try_from(lines).unwrap_or(u16::MAX);
            let height = LOG_LINE * f32::from(rows) + SPACE_3 + SPACE_3;
            div().h(height).child(self.log.clone()).into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .pt(SPACE_2)
            .pb(SPACE_8)
            .child(SectionHeader::new("Agent log"))
            .child(div().border_t(RULE).border_color(palette.line).child(body))
    }
}

impl Render for PlatformScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let agent = self.agent_section(&palette, cx);
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                PageTitle::new(TITLE)
                    .leading(self.navigation.clone())
                    .detail(self.loco.platform.subtitle()),
            )
            .child(
                div()
                    .id("loco-platform")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(agent)
                    .child(self.placements_section(&palette))
                    .child(self.components_section(&palette))
                    .child(self.log_section(&palette)),
            )
    }
}

fn healthy(ok: bool) -> Status {
    if ok { Status::Ok } else { Status::Err }
}

fn plain(text: impl Into<SharedString>) -> AnyElement {
    div().child(text.into()).into_any_element()
}

fn unavailable(palette: &Palette) -> AnyElement {
    div()
        .text_color(palette.mute)
        .child(UNAVAILABLE)
        .into_any_element()
}

fn glyph_line(status: Status, text: impl Into<SharedString>, palette: &Palette) -> AnyElement {
    div()
        .flex()
        .gap(SPACE_3)
        .child(
            div()
                .text_color(palette.status(status))
                .child(status_symbols(status.glyph())),
        )
        .child(text.into())
        .into_any_element()
}

fn placement_counts(summary: &PlacementSummary, palette: &Palette) -> impl IntoElement {
    let badges = summary.badges.iter().map(|(status, n)| {
        div()
            .text_color(palette.status(*status))
            .child(status_symbols(format!("{} {n}", status.glyph())))
    });
    let prefix = format!(
        "{} · {} at desired revision",
        summary.total, summary.at_desired
    );
    let separated: Vec<AnyElement> = badges
        .flat_map(|badge| {
            [
                div().child(SEPARATOR).into_any_element(),
                badge.into_any_element(),
            ]
        })
        .collect();
    div()
        .flex()
        .font_family(FONT_MONO)
        .text_size(TEXT_SMALL)
        .text_color(palette.mute)
        .whitespace_nowrap()
        .child(prefix)
        .children(separated)
}

fn log_line(line: &AgentLogLine) -> LogLine {
    let level = match line.level {
        AgentLevel::Info => Level::Info,
        AgentLevel::Warn => Level::Warn,
        AgentLevel::Error => Level::Error,
    };
    LogLine::new(Some(&line.time), Some(level), line.message.clone())
}

fn text(value: impl Into<SharedString>, color: Hsla) -> AnyElement {
    div()
        .truncate()
        .text_color(color)
        .child(value.into())
        .into_any_element()
}

fn quiet_when_ok(status: Status, quiet: Hsla, palette: &Palette) -> Hsla {
    if status == Status::Ok {
        quiet
    } else {
        palette.status(status)
    }
}

fn placement_columns() -> Vec<Column<PlacementRow>> {
    vec![
        Column::new("", |row: &PlacementRow, _| {
            let status = row.app.placement_status().unwrap_or_default();
            StatusGlyph::new(status).into_any_element()
        })
        .fixed(GLYPH),
        Column::new("Application", |row: &PlacementRow, _| {
            div()
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .child(row.app.name.clone())
                .into_any_element()
        })
        .fixed(PLACEMENT_NAME),
        Column::new("Where", |row: &PlacementRow, palette| {
            let place = format!(
                "{}{SEPARATOR}{}",
                row.workspace.label(),
                row.app.environment
            );
            text(place, palette.mute)
        })
        .fixed(PLACEMENT_WHERE),
        Column::new("Revision", |row: &PlacementRow, palette| {
            let revision = row
                .app
                .placement
                .desired
                .map_or_else(|| "—".to_owned(), |desired| format!("rev {desired}"));
            text(revision, palette.fg2)
        })
        .mono()
        .fixed(PLACEMENT_REVISION),
        Column::new("Message", |row: &PlacementRow, palette| {
            let Some(issue) = row.app.placement.issue.as_ref() else {
                return div()
                    .truncate()
                    .text_size(TEXT_SMALL)
                    .text_color(palette.fg2)
                    .child(row.app.placement.detail())
                    .into_any_element();
            };
            let color = if issue.status == Status::Err {
                palette.err
            } else {
                palette.fg2
            };
            RawMessage::new(issue.message.clone())
                .color(color)
                .line()
                .into_any_element()
        })
        .mono()
        .flex(1., Pixels::ZERO),
    ]
}

fn component_columns() -> Vec<Column<Component>> {
    vec![
        Column::new("", |c: &Component, _| {
            StatusGlyph::new(c.status).into_any_element()
        })
        .fixed(GLYPH),
        Column::new("Name", |c: &Component, palette| {
            text(c.name.clone(), palette.fg)
        })
        .mono()
        .fixed(COMPONENT_NAME),
        Column::new("Namespace", |c: &Component, palette| {
            text(c.namespace.clone(), palette.mute)
        })
        .fixed(COMPONENT_NAMESPACE),
        Column::new("Ready", |c: &Component, palette| {
            text(
                c.ready_label(),
                quiet_when_ok(c.status, palette.fg, palette),
            )
        })
        .mono()
        .fixed(COMPONENT_READY),
        Column::new("Version", |c: &Component, palette| {
            text(c.version.clone().unwrap_or_default(), palette.fg2)
        })
        .mono()
        .fixed(COMPONENT_VERSION),
        Column::new("Detail", |c: &Component, palette| {
            text(
                c.detail.clone(),
                quiet_when_ok(c.status, palette.mute, palette),
            )
        })
        .flex(1., Pixels::ZERO),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_levels_map_to_log_levels() {
        let line = AgentLogLine {
            time: "21:13:41".into(),
            level: AgentLevel::Error,
            message: "apply failed".into(),
        };
        let mapped = log_line(&line);
        assert_eq!(mapped.level, Some(Level::Error));
        assert_eq!(mapped.timestamp.as_deref(), Some("21:13:41"));
    }

    #[test]
    fn failed_health_reads_as_an_error() {
        assert_eq!(healthy(true), Status::Ok);
        assert_eq!(healthy(false), Status::Err);
    }
}
