use flock_core::overview::{Capacity, percent, plural, thousands};
use flock_core::{Page, Status};
use gpui::{AnyElement, Context, Hsla, IntoElement};

use super::{OverviewData, OverviewScreen};
use crate::kit::{HealthTile, HealthTiles, Meter, StackedBar, TileNote};
use crate::theme::Palette;

const REQUEST_GLYPH: &str = "▮";
const LIMIT_GLYPH: &str = "┆";

fn weight(count: impl TryInto<u16>) -> f32 {
    f32::from(count.try_into().unwrap_or(u16::MAX))
}

fn figure(count: impl TryInto<u32>) -> u32 {
    count.try_into().unwrap_or(u32::MAX)
}

fn counted(count: u32, label: &str, color: Hsla) -> TileNote {
    TileNote::new(format!("{} {label}", thousands(count)), color)
}

fn flagged(
    count: u32,
    label: &str,
    color: Hsla,
    page: Page,
    cx: &mut Context<OverviewScreen>,
) -> TileNote {
    let note = counted(count, label, color);
    if count == 0 {
        return note;
    }
    let id = format!("overview-{}-{}", page.label(), label).to_lowercase();
    let id = id.replace(' ', "-");
    note.drill(id, OverviewScreen::attention(page, None, cx))
}

fn nodes(data: &OverviewData, palette: &Palette, cx: &mut Context<OverviewScreen>) -> HealthTile {
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
    HealthTile::new("overview-nodes", "Nodes", nodes.total.to_string(), bar)
        .detail(detail)
        .notes(parts)
        .on_activate(OverviewScreen::navigate(Page::Nodes, cx))
}

fn pods(data: &OverviewData, palette: &Palette, cx: &mut Context<OverviewScreen>) -> HealthTile {
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
    HealthTile::new("overview-pods", "Pods", figure, bar)
        .notes(parts)
        .on_activate(OverviewScreen::navigate(Page::Pods, cx))
}

fn capacity(label: &'static str, capacity: &Capacity, palette: &Palette) -> HealthTile {
    let used = capacity.fraction();
    let meter = Meter::new(used)
        .request(capacity.requests)
        .limit(capacity.limits)
        .color(palette.acc);
    let parts = vec![
        TileNote::new(format!("used {}", percent(used)), palette.mute),
        TileNote::new(
            format!("{REQUEST_GLYPH} req {}", percent(capacity.requests)),
            palette.fg2,
        ),
        TileNote::new(
            format!("{LIMIT_GLYPH} lim {}", percent(capacity.limits)),
            palette.warn,
        ),
    ];
    let id = format!("overview-capacity-{label}");
    HealthTile::new(id, label, String::new(), meter)
        .amount(capacity.amount())
        .notes(parts)
}

pub(super) fn render(
    data: &OverviewData,
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> AnyElement {
    let tiles = vec![
        nodes(data, palette, cx),
        pods(data, palette, cx),
        capacity("CPU", &data.cpu, palette),
        capacity("Memory", &data.memory, palette),
    ];
    HealthTiles::new(tiles).into_any_element()
}
