use flock_core::overview::plural;
use flock_core::problem_groups::{self, Cause, Marker, Occurrence, Trait, WINDOW};
use flock_core::time::age;
use flock_core::{ClusterId, ClusterSummary, Command, Snapshot, Status};
use gpui::{
    AnyElement, AppContext, Context, Entity, EventEmitter, FontWeight, InteractiveElement,
    IntoElement, ParentElement, Render, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, relative,
};

use super::ScreenEvent;
use crate::kit::{
    Column, EmptyState, FilterBar, FilterEvent, InspectorPane, RawMessage, SectionLabel,
    StatusGlyph, Table, TableEvent,
};
use crate::theme::{
    AFFECTED_NAMESPACE_WIDTH, AFFECTED_NODE_WIDTH, AFFECTED_OBJECT_MIN, AFFECTED_SINCE_WIDTH,
    AFFECTED_TAG_WIDTH, AFFECTED_WORKLOAD_WIDTH, ActiveTheme, FONT_MONO, PROBLEM_BAR_EMPTY,
    PROBLEM_BAR_GAP, PROBLEM_CAUSES_WIDTH, PROBLEM_HISTOGRAM, PROBLEM_TRAIT_BAR,
    PROBLEM_TRAIT_BAR_HEIGHT, PROBLEM_TRAIT_NAME, PROBLEM_TRAIT_SHARE, Palette, RULE, SPACE_1,
    SPACE_2, SPACE_3, SPACE_4, SPACE_5, SPACE_6, SPACE_7, SPACE_8, TEXT_AXIS, TEXT_BODY,
    TEXT_CONTROL, TEXT_LABEL, TEXT_SMALL, TEXT_SUBHEADING,
};

const NOTHING: &str = "Nothing needs attention in this cluster.";
const OCCURRENCES: &str = "New occurrences · 6h";
const SHARED: &str = "What the affected objects share";
const AFFECTED: &str = "Affected";
const NONE: &str = "—";
const MARK: &str = "┆";
const AXIS: [&str; 4] = ["6h ago", "4h ago", "2h ago", "now"];
const OBJECT_ONE: &str = "object";
const OBJECT_MANY: &str = "objects";
const CAUSES_ID: &str = "problem-causes";

pub struct ClusterProblemsScreen {
    cluster: Option<ClusterId>,
    causes: Vec<Cause>,
    markers: Vec<Vec<Marker>>,
    selected: Option<(String, String)>,
    filter: Entity<FilterBar>,
    table: Entity<Table<Occurrence>>,
}

impl EventEmitter<ScreenEvent> for ClusterProblemsScreen {}

impl ClusterProblemsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let table = cx.new(|cx| {
            Table::new("problem-affected", columns(), cx)
                .row_key(|o: &Occurrence| o.object_label().into())
                .click_opens()
        });
        cx.subscribe(&table, |_, table, event: &TableEvent, cx| {
            let TableEvent::Opened(ix) = *event else {
                return;
            };
            let target = table.read(cx).row(ix).map(|o| o.target.clone());
            if let Some(target) = target {
                cx.emit(ScreenEvent::Command(Command::OpenResource(target)));
            }
        })
        .detach();
        let placeholder = Occurrence::FILTER.placeholder();
        let filter = cx.new(|cx| FilterBar::new("problem-filter", placeholder, window, cx));
        cx.subscribe(&filter, |this, _, FilterEvent::Changed, cx| {
            this.refresh_rows(cx);
        })
        .detach();
        Self {
            cluster: None,
            causes: Vec::new(),
            markers: Vec::new(),
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

    pub fn header_note(&self) -> String {
        plural(self.causes.len(), "cause", "causes")
    }

    pub fn set(&mut self, cluster: &ClusterSummary, snapshot: &Snapshot, cx: &mut Context<Self>) {
        if self.cluster.as_ref() != Some(&cluster.id) {
            self.selected = None;
        }
        self.cluster = Some(cluster.id.clone());
        let problems: Vec<_> = snapshot
            .home
            .problems
            .iter()
            .filter(|p| p.target.cluster == cluster.id)
            .cloned()
            .collect();
        self.causes = problem_groups::causes(&cluster.id, &snapshot.pods, &problems);
        let kept = self
            .selected
            .as_ref()
            .is_some_and(|key| self.causes.iter().any(|c| same(c, key)));
        if !kept {
            self.selected = self.causes.first().map(key_of);
        }
        self.markers = self
            .causes
            .iter()
            .map(|cause| cause.markers(&snapshot.flux, &cluster.id))
            .collect();
        self.refresh_rows(cx);
    }

    fn current_index(&self) -> Option<usize> {
        let key = self.selected.as_ref()?;
        self.causes.iter().position(|cause| same(cause, key))
    }

    fn current(&self) -> Option<&Cause> {
        self.causes.get(self.current_index()?)
    }

    fn pick(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.selected = self.causes.get(ix).map(key_of);
        self.refresh_rows(cx);
    }

    fn refresh_rows(&mut self, cx: &mut Context<Self>) {
        let query = self.filter.read(cx).query().clone();
        let rows: Vec<Occurrence> = self
            .current()
            .map(|cause| {
                cause
                    .occurrences
                    .iter()
                    .filter(|o| {
                        let namespace = o.target.namespace.as_deref();
                        Occurrence::FILTER.matches(o, &query, namespace, &[])
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        self.table.update(cx, |table, cx| table.set_rows(rows, cx));
        cx.notify();
    }

    fn cause_list(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.causes.iter().enumerate().map(|(ix, cause)| {
            let selected = self.selected.as_ref().is_some_and(|key| same(cause, key));
            cause_row(ix, cause, selected, palette).on_click(cx.listener(move |this, _, _, cx| {
                this.pick(ix, cx);
            }))
        });
        let list = div()
            .id(CAUSES_ID)
            .size_full()
            .overflow_y_scroll()
            .bg(palette.panel)
            .border_r(RULE)
            .border_color(palette.line)
            .children(rows);
        InspectorPane::new(CAUSES_ID, list)
            .width(PROBLEM_CAUSES_WIDTH)
            .trailing_handle()
            .into_any_element()
    }

    fn detail(&self, ix: usize, palette: &Palette) -> Option<AnyElement> {
        let cause = self.causes.get(ix)?;
        let markers = self.markers.get(ix).map_or(&[][..], Vec::as_slice);
        let element = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(cause_header(cause, palette))
            .child(occurrences(cause, markers, palette))
            .child(traits(&cause.traits(), palette))
            .child(self.affected(cause, palette))
            .into_any_element();
        Some(element)
    }

    fn affected(&self, cause: &Cause, palette: &Palette) -> AnyElement {
        let noun = if cause.is_pods() {
            plural(cause.count(), "pod", "pods")
        } else {
            plural(cause.count(), OBJECT_ONE, OBJECT_MANY)
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_5)
                    .px(SPACE_8)
                    .pt(SPACE_7)
                    .pb(SPACE_3)
                    .child(SectionLabel::new(AFFECTED))
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(TEXT_SMALL)
                            .text_color(palette.mute)
                            .child(noun),
                    ),
            )
            .child(self.filter.clone())
            .child(div().flex_1().min_h_0().child(self.table.clone()))
            .into_any_element()
    }
}

impl Render for ClusterProblemsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        if self.causes.is_empty() {
            return div()
                .size_full()
                .child(EmptyState::new(NOTHING).selector("problems-empty"))
                .into_any_element();
        }
        let list = self.cause_list(&palette, cx);
        let detail = self
            .current_index()
            .and_then(|ix| self.detail(ix, &palette));
        div()
            .size_full()
            .flex()
            .child(list)
            .children(detail)
            .into_any_element()
    }
}

fn key_of(cause: &Cause) -> (String, String) {
    (cause.reason.clone(), cause.template.clone())
}

fn same(cause: &Cause, key: &(String, String)) -> bool {
    cause.key() == (key.0.as_str(), key.1.as_str())
}

fn status_color(status: Status, palette: &Palette) -> gpui::Hsla {
    palette.status(status)
}

fn cause_row(
    ix: usize,
    cause: &Cause,
    selected: bool,
    palette: &Palette,
) -> gpui::Stateful<gpui::Div> {
    let color = status_color(cause.status, palette);
    let selector = format!("problem-cause-{ix}");
    let short = if cause.template.is_empty() {
        cause.reason.clone()
    } else {
        cause.template.clone()
    };
    div()
        .id(("problem-cause", ix))
        .debug_selector(move || selector.clone())
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .px(SPACE_6)
        .py(SPACE_4)
        .border_b(RULE)
        .border_color(palette.line)
        .cursor_pointer()
        .when(selected, |row| {
            row.bg(palette.sel).border_1().border_color(palette.selline)
        })
        .when(!selected, |row| row.hover(|row| row.bg(palette.hover)))
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_4)
                .child(StatusGlyph::new(cause.status))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(TEXT_BODY)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(color)
                        .child(cause.reason.clone()),
                )
                .child(
                    div()
                        .font_family(FONT_MONO)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(cause.count().to_string()),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_4)
                .pl(crate::theme::GLYPH_SLOT + SPACE_4)
                .font_family(FONT_MONO)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(TEXT_SMALL)
                        .text_color(palette.fg2)
                        .child(short),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(TEXT_LABEL)
                        .text_color(palette.mute)
                        .child(cause.meta()),
                ),
        )
}

fn cause_header(cause: &Cause, palette: &Palette) -> AnyElement {
    let template = if cause.template.is_empty() {
        cause.reason.clone()
    } else {
        cause.template.clone()
    };
    div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(SPACE_4)
        .px(SPACE_8)
        .pt(SPACE_7)
        .pb(SPACE_6)
        .border_b(RULE)
        .border_color(palette.line)
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_5)
                .child(StatusGlyph::new(cause.status))
                .child(
                    div()
                        .debug_selector(|| "problem-reason".to_owned())
                        .text_size(TEXT_SUBHEADING)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(cause.reason.clone()),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(FONT_MONO)
                        .text_size(TEXT_CONTROL)
                        .text_color(palette.fg2)
                        .child(cause.scope()),
                ),
        )
        .child(
            div()
                .debug_selector(|| "problem-template".to_owned())
                .min_w_0()
                .child(RawMessage::new(template)),
        )
        .into_any_element()
}

#[allow(clippy::cast_precision_loss)]
fn occurrences(cause: &Cause, markers: &[Marker], palette: &Palette) -> AnyElement {
    let bars = cause.histogram();
    let peak = bars.iter().copied().max().unwrap_or_default().max(1);
    let color = status_color(cause.status, palette);
    let columns = bars.iter().map(|count| {
        let bar = div().flex_1();
        if *count == 0 {
            return bar.h(PROBLEM_BAR_EMPTY).bg(palette.line2);
        }
        let height = *count as f32 / peak as f32;
        bar.h(relative(height)).bg(color)
    });
    let window = WINDOW.as_secs_f32();
    let lines = markers.iter().map(|marker| {
        let at = 1. - (marker.age.as_secs_f32() / window).min(1.);
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(relative(at))
            .w(RULE)
            .bg(palette.fg2)
    });
    let axis = AXIS.iter().map(|label| div().child(*label));
    let notes = markers.iter().map(|marker| {
        div()
            .flex()
            .gap(SPACE_5)
            .child(
                div()
                    .text_color(palette.mute)
                    .child(format!("{MARK} {} ago", age(marker.age))),
            )
            .child(div().text_color(palette.fg2).child(marker.what.clone()))
    });
    div()
        .flex_none()
        .px(SPACE_8)
        .pt(SPACE_7)
        .pb(SPACE_5)
        .border_b(RULE)
        .border_color(palette.line)
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_5)
                .mb(SPACE_4)
                .child(SectionLabel::new(OCCURRENCES))
                .child(
                    div()
                        .font_family(FONT_MONO)
                        .text_size(TEXT_SMALL)
                        .text_color(palette.fg2)
                        .child(cause.started()),
                ),
        )
        .child(
            div()
                .debug_selector(|| "problem-histogram".to_owned())
                .relative()
                .h(PROBLEM_HISTOGRAM)
                .flex()
                .items_end()
                .gap(PROBLEM_BAR_GAP)
                .border_b(RULE)
                .border_color(palette.line2)
                .children(columns)
                .children(lines),
        )
        .child(
            div()
                .flex()
                .justify_between()
                .mt(SPACE_2)
                .font_family(FONT_MONO)
                .text_size(TEXT_AXIS)
                .text_color(palette.mute)
                .children(axis),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .mt(SPACE_4)
                .font_family(FONT_MONO)
                .text_size(TEXT_SMALL)
                .children(notes),
        )
        .into_any_element()
}

fn traits(traits: &[Trait], palette: &Palette) -> AnyElement {
    let rows = traits.iter().map(|shared| {
        let strong = shared.is_universal();
        let value_color = if strong { palette.fg } else { palette.fg2 };
        let bar_color = if strong { palette.err } else { palette.fg2 };
        div()
            .flex()
            .items_center()
            .gap(SPACE_6)
            .child(
                div()
                    .flex_none()
                    .w(PROBLEM_TRAIT_NAME)
                    .text_color(palette.mute)
                    .child(shared.name),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(FONT_MONO)
                    .text_size(TEXT_CONTROL)
                    .text_color(value_color)
                    .child(shared.value.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .w(PROBLEM_TRAIT_BAR)
                    .h(PROBLEM_TRAIT_BAR_HEIGHT)
                    .bg(palette.line2)
                    .child(div().h_full().w(relative(shared.fraction())).bg(bar_color)),
            )
            .child(
                div()
                    .flex_none()
                    .w(PROBLEM_TRAIT_SHARE)
                    .flex()
                    .justify_end()
                    .font_family(FONT_MONO)
                    .text_size(TEXT_SMALL)
                    .text_color(value_color)
                    .child(shared.share_label()),
            )
    });
    div()
        .flex_none()
        .px(SPACE_8)
        .pt(SPACE_7)
        .pb(SPACE_5)
        .border_b(RULE)
        .border_color(palette.line)
        .child(div().mb(SPACE_4).child(SectionLabel::new(SHARED)))
        .child(
            div()
                .debug_selector(|| "problem-traits".to_owned())
                .flex()
                .flex_col()
                .gap(SPACE_3)
                .text_size(TEXT_BODY)
                .children(rows),
        )
        .into_any_element()
}

fn or_none(value: Option<&str>) -> gpui::SharedString {
    value.unwrap_or(NONE).to_owned().into()
}

fn columns() -> Vec<Column<Occurrence>> {
    vec![
        Column::colored_text(
            "Namespace",
            |o: &Occurrence| or_none(o.target.namespace.as_deref()),
            |_, p| p.fg2,
        )
        .mono()
        .fixed(AFFECTED_NAMESPACE_WIDTH)
        .sort_by_key(|o: &Occurrence| o.target.namespace.clone()),
        Column::text("Workload", |o: &Occurrence| or_none(o.workload.as_deref()))
            .mono()
            .fixed(AFFECTED_WORKLOAD_WIDTH)
            .sort_by_key(|o: &Occurrence| o.workload.clone()),
        Column::colored_text(
            "Pod",
            |o: &Occurrence| o.object_label().into(),
            |_, p| p.fg2,
        )
        .highlight()
        .selectable()
        .mono()
        .flex(1., AFFECTED_OBJECT_MIN)
        .sort_by_key(Occurrence::object_label),
        Column::colored_text(
            "Node",
            |o: &Occurrence| or_none(o.node.as_deref()),
            |_, p| p.mute,
        )
        .mono()
        .fixed(AFFECTED_NODE_WIDTH)
        .sort_by_key(|o: &Occurrence| o.node.clone()),
        Column::colored_text(
            "Image tag",
            |o: &Occurrence| or_none(o.tag()),
            |_, p| p.mute,
        )
        .mono()
        .fixed(AFFECTED_TAG_WIDTH),
        Column::colored_text("Since", |o: &Occurrence| age(o.age).into(), |_, p| p.mute)
            .mono()
            .end()
            .fixed(AFFECTED_SINCE_WIDTH)
            .sort_by_key(|o: &Occurrence| o.age),
    ]
}
