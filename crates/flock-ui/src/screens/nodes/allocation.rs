use std::cell::Cell;
use std::rc::Rc;

use flock_core::allocation::{
    Basis, PodShare, Rect, Resource, Slot, Subject, Tile, Treemap, amount,
};
use flock_core::nodes::Node;
use flock_core::overview::percent;
use gpui::{
    AnyElement, Context, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels,
    Size, StatefulInteractiveElement, Styled, canvas, div, pattern_slash, prelude::FluentBuilder,
    px, relative,
};

use super::NodesScreen;
use crate::highlight::over;
use crate::kit::{SectionLabel, Segmented};
use crate::theme::{
    ALLOCATION_CURRENT, ALLOCATION_HEADER, ALLOCATION_HEIGHT, ALLOCATION_SIDE, ActiveTheme,
    FONT_MONO, FREE_HATCH_INTERVAL, FREE_HATCH_WIDTH, FREE_LABEL_MIN_HEIGHT, FREE_LABEL_MIN_WIDTH,
    LEGEND_CHIP_SWATCH, Palette, RADIUS, RADIUS_SMALL, RULE, SIDECAR_STRIPE, SPACE_1, SPACE_2,
    SPACE_3, SPACE_4, SPACE_5, SPACE_6, SPACE_8, TEXT_AXIS, TEXT_MONO, TEXT_SMALL,
    TILE_AMOUNT_OPACITY, TILE_BORDER, TILE_DIM, TILE_FOCUS, TILE_LABEL_LINE, TILE_LABEL_MIN_HEIGHT,
    TILE_LABEL_MIN_WIDTH, TILE_LABEL_X, TILE_LABEL_Y, TILE_TINT,
};

const TITLE: &str = "Allocation";
const BASIS_ID: &str = "node-allocation-basis";
const RESOURCE_ID: &str = "node-allocation-resource";
const REST_NAMES: usize = 3;
const SIDECAR_NOTE: &str = " · sidecar";
const LIMIT_KEY: &str = "requested / limit";
const NO_LIMIT: &str = "—";
const LIMIT_KEY_FILL: f32 = 0.5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Focus {
    Pod(String),
    Rest,
}

#[derive(Default)]
pub struct Allocation {
    pub resource: Resource,
    pub basis: Basis,
    pub hovered: Option<Focus>,
    pub pinned: Option<Focus>,
    size: Rc<Cell<Size<Pixels>>>,
}

impl Allocation {
    pub fn forget_focus(&mut self) {
        self.hovered = None;
        self.pinned = None;
    }

    fn hover(&mut self, focus: Focus, hovered: bool) {
        if hovered {
            self.hovered = Some(focus);
        } else if self.hovered.as_ref() == Some(&focus) {
            self.hovered = None;
        }
    }

    fn pin(&mut self, focus: Focus) {
        self.pinned = if self.pinned.as_ref() == Some(&focus) {
            None
        } else {
            Some(focus)
        };
    }

    fn treemap(&self, node: &Node) -> Treemap {
        let size = self.size.get();
        let allocatable = match self.resource {
            Resource::Cpu => node.cpu.allocatable,
            Resource::Memory => node.memory.allocatable,
        };
        Treemap::build(
            &node.allocation,
            allocatable,
            self.resource,
            self.basis,
            f32::from(size.width),
            f32::from(size.height),
        )
    }

    pub fn render(&self, node: &Node, cx: &mut Context<NodesScreen>) -> AnyElement {
        let palette = *cx.palette();
        let map = self.treemap(node);
        let focus = self.hovered.as_ref().or(self.pinned.as_ref());
        let current = focus
            .and_then(|focus| {
                map.tiles
                    .iter()
                    .find(|t| &focus_of(t, &node.allocation) == focus)
            })
            .or_else(|| map.tiles.first());
        let header = self.header(node, &map, &palette, cx);
        let tiles = self.tiles(&map, &node.allocation, &palette, cx);
        let side = div()
            .flex_none()
            .w(ALLOCATION_SIDE)
            .min_w_0()
            .flex()
            .flex_col()
            .gap(SPACE_6)
            .font_family(FONT_MONO)
            .text_size(TEXT_SMALL)
            .child(current_block(current, &map, node, self.resource, &palette))
            .child(legend(&map, self.resource, self.basis, &palette));
        div()
            .debug_selector(|| "node-allocation".to_owned())
            .flex_none()
            .h(ALLOCATION_HEIGHT)
            .min_w_0()
            .flex()
            .flex_col()
            .border_t(RULE)
            .border_color(palette.line)
            .bg(palette.panel)
            .child(header)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap(SPACE_8)
                    .pt(SPACE_5)
                    .px(SPACE_8)
                    .pb(SPACE_6)
                    .child(tiles)
                    .child(side),
            )
            .into_any_element()
    }

    fn header(
        &self,
        node: &Node,
        map: &Treemap,
        palette: &Palette,
        cx: &mut Context<NodesScreen>,
    ) -> AnyElement {
        let basis = Basis::ALL
            .iter()
            .fold(Segmented::new(BASIS_ID).small().mono(), |seg, b| {
                seg.option(b.label())
            })
            .selected(
                Basis::ALL
                    .iter()
                    .position(|b| *b == self.basis)
                    .unwrap_or_default(),
            )
            .on_select(cx.listener(|screen, ix: &usize, _, cx| {
                screen.allocation.basis = Basis::ALL.get(*ix).copied().unwrap_or_default();
                cx.notify();
            }));
        let resource = Resource::ALL
            .iter()
            .fold(Segmented::new(RESOURCE_ID).small().mono(), |seg, r| {
                seg.option(r.label())
            })
            .selected(
                Resource::ALL
                    .iter()
                    .position(|r| *r == self.resource)
                    .unwrap_or_default(),
            )
            .on_select(cx.listener(|screen, ix: &usize, _, cx| {
                screen.allocation.resource = Resource::ALL.get(*ix).copied().unwrap_or_default();
                cx.notify();
            }));
        let share_color = if map.overcommitted() {
            palette.warn
        } else {
            palette.fg2
        };
        let sum = format!(
            "{} / {}",
            amount(self.resource, map.total),
            amount(self.resource, map.allocatable)
        );
        div()
            .flex_none()
            .h(ALLOCATION_HEADER)
            .px(SPACE_8)
            .flex()
            .items_center()
            .gap(SPACE_5)
            .border_b(RULE)
            .border_color(palette.line)
            .child(SectionLabel::new(TITLE))
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(TEXT_MONO)
                    .child(node.name.clone()),
            )
            .child(basis)
            .child(resource)
            .child(div().flex_1())
            .child(
                div()
                    .debug_selector(|| "node-allocation-sum".to_owned())
                    .flex()
                    .gap(SPACE_3)
                    .font_family(FONT_MONO)
                    .text_size(TEXT_MONO)
                    .child(sum)
                    .child(div().text_color(share_color).child(percent(map.fraction()))),
            )
            .into_any_element()
    }

    fn tiles(
        &self,
        map: &Treemap,
        pods: &[PodShare],
        palette: &Palette,
        cx: &mut Context<NodesScreen>,
    ) -> AnyElement {
        let size = self.size.clone();
        let screen = cx.entity();
        let recorder = canvas(
            move |bounds, _, cx| {
                if size.get() != bounds.size {
                    size.set(bounds.size);
                    let screen = screen.clone();
                    cx.defer(move |cx| screen.update(cx, |_, cx| cx.notify()));
                }
            },
            |_, (), _, _| {},
        )
        .absolute()
        .size_full();
        let focus = self.hovered.as_ref();
        let pinned = self.pinned.as_ref();
        let tiles = map.tiles.iter().enumerate().map(|(ix, tile)| {
            let key = focus_of(tile, pods);
            let dim = focus.is_some_and(|f| *f != key);
            let pinned = pinned == Some(&key);
            self.tile(
                ix,
                tile,
                key,
                pods,
                map.rest.len(),
                dim,
                pinned,
                palette,
                cx,
            )
        });
        let free = map.free.map(|rect| {
            let roomy = rect.w > FREE_LABEL_MIN_WIDTH && rect.h > FREE_LABEL_MIN_HEIGHT;
            let label = format!(
                "{} {}",
                self.basis.free_label(),
                amount(self.resource, map.free_value)
            );
            placed(rect)
                .p(SPACE_2)
                .font_family(FONT_MONO)
                .text_size(TEXT_AXIS)
                .text_color(palette.mute)
                .when(roomy, |free| {
                    free.child(div().bg(palette.panel).px(SPACE_1).child(label))
                })
        });
        div()
            .debug_selector(|| "node-allocation-map".to_owned())
            .flex_1()
            .min_w_0()
            .relative()
            .rounded(RADIUS)
            .overflow_hidden()
            .bg(pattern_slash(
                palette.line2,
                FREE_HATCH_WIDTH,
                FREE_HATCH_INTERVAL,
            ))
            .child(recorder)
            .children(tiles)
            .children(free)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn tile(
        &self,
        ix: usize,
        tile: &Tile,
        key: Focus,
        pods: &[PodShare],
        rest: usize,
        dim: bool,
        pinned: bool,
        palette: &Palette,
        cx: &mut Context<NodesScreen>,
    ) -> AnyElement {
        let color = slot_color(tile.slot, palette);
        let tint = over(color.opacity(TILE_TINT), palette.panel);
        let parts = tile.parts.iter().map(|part| {
            let within = Rect {
                x: part.rect.x - tile.rect.x,
                y: part.rect.y - tile.rect.y,
                ..part.rect
            };
            let fill = if part.sidecar {
                div().bg(tint).child(div().size_full().bg(pattern_slash(
                    color,
                    SIDECAR_STRIPE,
                    SIDECAR_STRIPE,
                )))
            } else if part.filled < 1. {
                div().bg(tint).child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .w_full()
                        .h(relative(part.filled))
                        .bg(color),
                )
            } else {
                div().bg(color)
            };
            placed(within).child(fill.size_full().relative())
        });
        let roomy = tile.rect.w > TILE_LABEL_MIN_WIDTH && tile.rect.h > TILE_LABEL_MIN_HEIGHT;
        let name = match tile.subject {
            Subject::Pod(pod) => pods
                .get(pod)
                .map_or_else(String::new, |pod| pod.workload().to_owned()),
            Subject::Rest => format!("+{rest} pods"),
        };
        let label = roomy.then(|| {
            div()
                .absolute()
                .top(TILE_LABEL_Y)
                .left(TILE_LABEL_X)
                .right(TILE_LABEL_X)
                .flex()
                .flex_col()
                .font_family(FONT_MONO)
                .text_size(TEXT_AXIS)
                .line_height(TILE_LABEL_LINE)
                .text_color(palette.on_tile)
                .child(div().truncate().font_weight(FontWeight::MEDIUM).child(name))
                .child(
                    div()
                        .truncate()
                        .opacity(TILE_AMOUNT_OPACITY)
                        .child(amount(self.resource, tile.value)),
                )
        });
        let edge = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .when(pinned, |edge| {
                edge.border(TILE_FOCUS).border_color(palette.fg)
            })
            .when(!pinned, |edge| {
                edge.border(TILE_BORDER).border_color(palette.panel)
            });
        let hover_key = key.clone();
        placed(tile.rect)
            .debug_selector(move || format!("node-allocation-tile-{ix}"))
            .id(("node-allocation-tile", ix))
            .cursor_pointer()
            .when(dim, |tile| tile.opacity(TILE_DIM))
            .on_hover(cx.listener(move |screen, hovered: &bool, _, cx| {
                screen.allocation.hover(hover_key.clone(), *hovered);
                cx.notify();
            }))
            .on_click(cx.listener(move |screen, _, _, cx| {
                screen.allocation.pin(key.clone());
                cx.notify();
            }))
            .children(parts)
            .children(label)
            .child(edge)
            .into_any_element()
    }
}

fn placed(rect: Rect) -> gpui::Div {
    div()
        .absolute()
        .left(px(rect.x))
        .top(px(rect.y))
        .w(px(rect.w))
        .h(px(rect.h))
        .overflow_hidden()
}

fn focus_of(tile: &Tile, pods: &[PodShare]) -> Focus {
    match tile.subject {
        Subject::Pod(ix) => pods
            .get(ix)
            .map_or(Focus::Rest, |pod| Focus::Pod(pod.path())),
        Subject::Rest => Focus::Rest,
    }
}

fn slot_color(slot: Slot, palette: &Palette) -> Hsla {
    match slot {
        Slot::Namespace(ix) => palette.categorical.get(ix).copied().unwrap_or(palette.sep),
        Slot::Other => palette.sep,
    }
}

fn swatch(background: impl Into<gpui::Fill>) -> gpui::Div {
    div()
        .flex_none()
        .size(LEGEND_CHIP_SWATCH)
        .rounded(RADIUS_SMALL)
        .bg(background)
}

fn spread(name: impl IntoElement, value: impl IntoElement) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(SPACE_4)
        .min_w_0()
        .child(div().min_w_0().truncate().child(name))
        .child(div().flex_1())
        .child(div().flex_none().child(value))
}

fn current_block(
    current: Option<&Tile>,
    map: &Treemap,
    node: &Node,
    resource: Resource,
    palette: &Palette,
) -> impl IntoElement {
    let block = div()
        .debug_selector(|| "node-allocation-current".to_owned())
        .min_h(ALLOCATION_CURRENT)
        .flex()
        .flex_col()
        .gap(SPACE_1);
    let Some(tile) = current else {
        return block;
    };
    let color = slot_color(tile.slot, palette);
    let indent = LEGEND_CHIP_SWATCH + SPACE_3;
    let (heading, summary, lines): (AnyElement, String, Vec<(String, String)>) = match tile.subject
    {
        Subject::Pod(ix) => {
            let Some(pod) = node.allocation.get(ix) else {
                return block;
            };
            let heading = div()
                .truncate()
                .child(
                    div()
                        .flex()
                        .child(
                            div()
                                .text_color(palette.mute)
                                .child(format!("{}/", pod.namespace)),
                        )
                        .child(div().truncate().child(pod.name.clone())),
                )
                .into_any_element();
            let lines = pod
                .container_lines(resource)
                .into_iter()
                .map(|(name, sidecar, figures)| {
                    let name = if sidecar {
                        format!("{name}{SIDECAR_NOTE}")
                    } else {
                        name
                    };
                    (name, figures)
                })
                .collect();
            (heading, pod.summary(resource), lines)
        }
        Subject::Rest => {
            let rest: Vec<&PodShare> = map
                .rest
                .iter()
                .filter_map(|ix| node.allocation.get(*ix))
                .collect();
            let requested: f32 = rest.iter().map(|pod| pod.requested(resource)).sum();
            let limit: Option<f32> = rest.iter().map(|pod| pod.limit(resource)).sum();
            let limit = limit.map_or_else(|| NO_LIMIT.to_owned(), |limit| amount(resource, limit));
            let lines = rest
                .iter()
                .take(REST_NAMES)
                .map(|pod| (pod.path(), amount(resource, pod.requested(resource))))
                .collect();
            let heading = format!("+{} pods", rest.len()).into_any_element();
            (
                heading,
                format!("{} req · {limit} lim", amount(resource, requested)),
                lines,
            )
        }
    };
    block
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_3)
                .min_w_0()
                .child(swatch(color))
                .child(heading),
        )
        .child(div().pl(indent).text_color(palette.fg2).child(summary))
        .children(lines.into_iter().map(|(name, figures)| {
            spread(div().text_color(palette.mute).child(name), figures)
                .pl(indent)
                .text_color(palette.fg2)
        }))
}

fn legend(map: &Treemap, resource: Resource, basis: Basis, palette: &Palette) -> impl IntoElement {
    let entries = map.legend.iter().map(|entry| {
        let color = slot_color(entry.slot, palette);
        div()
            .flex()
            .items_center()
            .gap(SPACE_3)
            .child(swatch(color))
            .child(spread(entry.label.clone(), amount(resource, entry.value)).flex_1())
    });
    let free = (map.free_value > 0.).then(|| {
        div()
            .flex()
            .items_center()
            .gap(SPACE_3)
            .child(
                swatch(palette.panel)
                    .border(RULE)
                    .border_color(palette.line2)
                    .child(div().size_full().bg(pattern_slash(
                        palette.sep,
                        FREE_HATCH_WIDTH,
                        FREE_HATCH_INTERVAL,
                    ))),
            )
            .child(spread(basis.free_label(), amount(resource, map.free_value)).flex_1())
    });
    let limit_key = (basis == Basis::Limits).then(|| {
        div()
            .flex()
            .items_center()
            .gap(SPACE_3)
            .child(
                swatch(palette.line2).relative().child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .w_full()
                        .h(relative(LIMIT_KEY_FILL))
                        .bg(palette.fg2),
                ),
            )
            .child(LIMIT_KEY)
    });
    div()
        .debug_selector(|| "node-allocation-legend".to_owned())
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .text_color(palette.fg2)
        .children(entries)
        .children(free)
        .children(limit_key)
}
