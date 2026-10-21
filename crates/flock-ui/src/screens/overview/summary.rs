use flock_core::overview::{Capacity, percent, plural, thousands};
use flock_core::{Page, Status};
use gpui::{
    AnyElement, Context, Div, Hsla, InteractiveElement, IntoElement, ParentElement, Styled, div,
};

use super::{Activate, OverviewData, OverviewScreen, mono, problem_link};
use crate::kit::{ClickableSection, Meter, SectionLabel, StackedBar};
use crate::theme::{
    FONT_MONO, Palette, RULE, SPACE_3, SPACE_4, SPACE_5, SPACE_8, TEXT_BODY, TEXT_SMALL,
};

const SEPARATOR: &str = " · ";
const REQUEST_GLYPH: &str = "▮";
const LIMIT_GLYPH: &str = "┆";

fn weight(count: impl TryInto<u16>) -> f32 {
    f32::from(count.try_into().unwrap_or(u16::MAX))
}

fn title(label: &'static str, figure: String, detail: Option<String>, palette: &Palette) -> Div {
    let detail = detail.map(|detail| mono(detail, palette.mute).text_size(TEXT_SMALL).truncate());
    div()
        .flex()
        .items_baseline()
        .gap(SPACE_4)
        .min_w_0()
        .child(SectionLabel::new(label))
        .child(mono(figure, palette.fg).text_size(TEXT_BODY))
        .children(detail)
}

struct Note {
    text: String,
    color: Hsla,
    drill: Option<(String, Activate)>,
}

impl Note {
    fn new(text: String, color: Hsla) -> Self {
        Self {
            text,
            color,
            drill: None,
        }
    }

    fn problem(mut self, id: String, count: u32, activate: Activate) -> Self {
        if count > 0 {
            self.drill = Some((id, activate));
        }
        self
    }
}

fn notes(parts: Vec<Note>, palette: &Palette) -> Div {
    let last = parts.len().saturating_sub(1);
    let spans = parts.into_iter().enumerate().flat_map(|(ix, note)| {
        let span = div().text_color(note.color).child(note.text);
        let span = match note.drill {
            Some((id, activate)) => problem_link(id, span, activate).into_any_element(),
            None => span.into_any_element(),
        };
        let separator = (ix < last).then(|| {
            div()
                .text_color(note.color)
                .child(SEPARATOR)
                .into_any_element()
        });
        std::iter::once(span).chain(separator)
    });
    div()
        .flex()
        .min_w_0()
        .overflow_hidden()
        .font_family(FONT_MONO)
        .text_size(TEXT_SMALL)
        .text_color(palette.mute)
        .whitespace_nowrap()
        .children(spans)
}

fn figure(count: impl TryInto<u32>) -> u32 {
    count.try_into().unwrap_or(u32::MAX)
}

fn counted(count: u32, label: &str, color: Hsla) -> Note {
    Note::new(format!("{} {label}", thousands(count)), color)
}

fn flagged(
    count: u32,
    label: &str,
    color: Hsla,
    page: Page,
    cx: &mut Context<OverviewScreen>,
) -> Note {
    let id = format!("overview-{}-{}", page.label(), label).to_lowercase();
    let id = id.replace(' ', "-");
    let activate = OverviewScreen::attention(page, None, cx);
    counted(count, label, color).problem(id, count, activate)
}

fn clickable(
    id: &'static str,
    page: Page,
    body: [AnyElement; 3],
    cx: &mut Context<OverviewScreen>,
) -> AnyElement {
    let [title, bar, notes] = body;
    ClickableSection::new(id)
        .padding(SPACE_8, SPACE_5)
        .gap(SPACE_3)
        .title(title)
        .child(bar)
        .child(notes)
        .on_activate(OverviewScreen::navigate(page, cx))
        .into_any_element()
}

fn nodes(data: &OverviewData, palette: &Palette, cx: &mut Context<OverviewScreen>) -> AnyElement {
    let health = data.node_health;
    let healthy = health
        .total
        .unwrap_or_default()
        .saturating_sub(health.err + health.warn + health.sus);
    let bar = StackedBar::new(vec![
        (Status::Ok, weight(healthy)),
        (Status::Warn, weight(health.warn)),
        (Status::Err, weight(health.err)),
    ]);
    let nodes = &data.nodes;
    let mut parts = vec![counted(figure(nodes.ready), "ready", palette.mute)];
    let flags = [
        (nodes.not_ready, "not ready", palette.err),
        (nodes.disk_pressure, "DiskPressure", palette.warn),
        (nodes.cordoned, "cordoned", palette.warn),
    ];
    for (count, label, color) in flags {
        if count > 0 {
            parts.push(flagged(figure(count), label, color, Page::Nodes, cx));
        }
    }
    let detail = (nodes.pools > 0).then(|| plural(nodes.pools, "pool", "pools"));
    let title = title("Nodes", nodes.total.to_string(), detail, palette);
    let body = [
        title.into_any_element(),
        bar.into_any_element(),
        notes(parts, palette).into_any_element(),
    ];
    clickable("overview-nodes", Page::Nodes, body, cx)
}

fn pods(data: &OverviewData, palette: &Palette, cx: &mut Context<OverviewScreen>) -> AnyElement {
    let pods = data.pods;
    let capacity = data.nodes.pod_capacity;
    let free = capacity.saturating_sub(pods.total());
    let bar = StackedBar::new(vec![
        (Status::Ok, weight(pods.running)),
        (Status::Warn, weight(pods.pending)),
        (Status::Err, weight(pods.failing)),
    ])
    .rest(weight(free));
    let figure = format!("{} / {}", thousands(pods.total()), thousands(capacity));
    let parts = vec![
        counted(pods.running, "running", palette.mute),
        flagged(pods.pending, "pending", palette.warn, Page::Pods, cx),
        flagged(pods.failing, "failing", palette.err, Page::Pods, cx),
    ];
    let body = [
        title("Pods", figure, None, palette).into_any_element(),
        bar.into_any_element(),
        notes(parts, palette).into_any_element(),
    ];
    clickable("overview-pods", Page::Pods, body, cx)
}

fn capacity(label: &'static str, capacity: &Capacity, palette: &Palette) -> AnyElement {
    let used = capacity.fraction();
    let meter = Meter::new(used)
        .request(capacity.requests)
        .limit(capacity.limits)
        .color(palette.acc);
    let parts = vec![
        Note::new(format!("used {}", percent(used)), palette.mute),
        Note::new(
            format!("{REQUEST_GLYPH} req {}", percent(capacity.requests)),
            palette.fg2,
        ),
        Note::new(
            format!("{LIMIT_GLYPH} lim {}", percent(capacity.limits)),
            palette.warn,
        ),
    ];
    let id = format!("overview-capacity-{label}");
    div()
        .debug_selector(|| id)
        .flex()
        .flex_col()
        .gap(SPACE_3)
        .px(SPACE_8)
        .py(SPACE_5)
        .child(title(label, capacity.amount(), None, palette))
        .child(meter)
        .child(notes(parts, palette))
        .into_any_element()
}

pub(super) fn render(
    data: &OverviewData,
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> AnyElement {
    let cells = [
        nodes(data, palette, cx),
        pods(data, palette, cx),
        capacity("CPU", &data.cpu, palette),
        capacity("Memory", &data.memory, palette),
    ];
    let last = cells.len() - 1;
    let row = cells.into_iter().enumerate().map(|(ix, cell)| {
        let column = div().flex().flex_col().flex_1().min_w_0().child(cell);
        if ix < last {
            column.border_r(RULE).border_color(palette.line)
        } else {
            column
        }
    });
    div()
        .flex()
        .border_b(RULE)
        .border_color(palette.line)
        .children(row)
        .into_any_element()
}
