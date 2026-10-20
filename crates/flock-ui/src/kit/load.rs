use std::rc::Rc;

use flock_core::Status;
use flock_core::emptiness::{Emptiness, Noun};
use gpui::{
    AnimationExt, AnyElement, App, Entity, Hsla, InteractiveElement, IntoElement, ParentElement,
    Pixels, RenderOnce, SharedString, Styled, Window, div, linear_color_stop, linear_gradient,
    prelude::FluentBuilder,
};

use super::{Button, FilterBar, StatusText};
use crate::theme::{
    self, ActiveTheme, CHART_HEIGHT, Palette, ROW_TABLE, RULE, SKELETON_CHART_WIDTH,
    SKELETON_GRADIENT_ANGLE, SKELETON_GRADIENT_END, SKELETON_GRADIENT_START, SKELETON_METER_HEIGHT,
    SKELETON_METER_WIDTH, SKELETON_NAME_WIDTH, SKELETON_NAMESPACE_WIDTH, SKELETON_ROWS,
    SKELETON_SHIMMER, SKELETON_SHIMMER_OPACITY, SKELETON_SHIMMER_WIDTH, SKELETON_STATUS_WIDTH,
    SKELETON_TITLE_WIDTH, SPACE_4, SPACE_5, SPACE_8, TEXT_LABEL, TEXT_TITLE,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Rows,
    Blocks,
}

#[derive(IntoElement)]
pub struct Skeleton {
    shape: Shape,
}

impl Skeleton {
    pub fn rows() -> Self {
        Self { shape: Shape::Rows }
    }

    pub fn blocks() -> Self {
        Self {
            shape: Shape::Blocks,
        }
    }
}

fn shimmer_color(palette: &Palette) -> Hsla {
    if palette.line.l > palette.panel.l {
        palette.line
    } else {
        palette.panel
    }
}

fn block(id: String, width: Pixels, height: Pixels, palette: &Palette, still: bool) -> AnyElement {
    let base = div()
        .relative()
        .overflow_hidden()
        .w(width)
        .h(height)
        .rounded(theme::RADIUS_SMALL)
        .bg(palette.hover);
    if still {
        return base.into_any_element();
    }
    let travel = width + SKELETON_SHIMMER_WIDTH;
    let shimmer = shimmer_color(palette);
    let start = linear_color_stop(palette.hover, SKELETON_GRADIENT_START);
    let end = linear_color_stop(shimmer, SKELETON_GRADIENT_END);
    let fade_in = linear_gradient(SKELETON_GRADIENT_ANGLE, start, end);
    let start = linear_color_stop(shimmer, SKELETON_GRADIENT_START);
    let end = linear_color_stop(palette.hover, SKELETON_GRADIENT_END);
    let fade_out = linear_gradient(SKELETON_GRADIENT_ANGLE, start, end);
    let shine = div()
        .absolute()
        .top_0()
        .h_full()
        .w(SKELETON_SHIMMER_WIDTH)
        .flex()
        .child(div().h_full().flex_1().bg(fade_in))
        .child(div().h_full().flex_1().bg(fade_out))
        .opacity(SKELETON_SHIMMER_OPACITY)
        .with_animation(
            id,
            gpui::Animation::new(SKELETON_SHIMMER).repeat_synced(),
            move |shine, delta| shine.left(travel * delta - SKELETON_SHIMMER_WIDTH),
        );
    base.child(shine).into_any_element()
}

impl RenderOnce for Skeleton {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let still = cx.reduce_motion();
        let mut next = 0_usize;
        let mut piece = |width, height| {
            next += 1;
            block(format!("skeleton-{next}"), width, height, &palette, still)
        };
        let content = match self.shape {
            Shape::Rows => div().flex().flex_col().children(
                (0..SKELETON_ROWS)
                    .map(|_| {
                        div()
                            .flex()
                            .items_center()
                            .gap(SPACE_8)
                            .h(ROW_TABLE)
                            .border_b(RULE)
                            .border_color(palette.line)
                            .child(piece(SKELETON_NAME_WIDTH, TEXT_LABEL))
                            .child(piece(SKELETON_NAMESPACE_WIDTH, TEXT_LABEL))
                            .child(piece(SKELETON_STATUS_WIDTH, TEXT_LABEL))
                    })
                    .collect::<Vec<_>>(),
            ),
            Shape::Blocks => div()
                .flex()
                .flex_col()
                .gap(SPACE_8)
                .child(piece(SKELETON_TITLE_WIDTH, TEXT_TITLE))
                .child(
                    div()
                        .flex()
                        .gap(SPACE_8)
                        .child(piece(SKELETON_METER_WIDTH, SKELETON_METER_HEIGHT))
                        .child(piece(SKELETON_METER_WIDTH, SKELETON_METER_HEIGHT))
                        .child(piece(SKELETON_METER_WIDTH, SKELETON_METER_HEIGHT)),
                )
                .child(piece(SKELETON_CHART_WIDTH, CHART_HEIGHT)),
        };
        div()
            .debug_selector(|| "loading-skeleton".to_owned())
            .px(SPACE_8)
            .py(SPACE_5)
            .child(content)
    }
}

#[derive(IntoElement)]
pub struct ConnectionProgress {
    cluster: SharedString,
    step: SharedString,
}

impl ConnectionProgress {
    pub fn new(cluster: impl Into<SharedString>, step: impl Into<SharedString>) -> Self {
        Self {
            cluster: cluster.into(),
            step: step.into(),
        }
    }
}

impl RenderOnce for ConnectionProgress {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let title = format!("Connecting to {}", self.cluster);
        div()
            .debug_selector(|| "connection-progress".to_owned())
            .flex()
            .flex_col()
            .gap(SPACE_4)
            .px(SPACE_8)
            .pt(SPACE_8)
            .child(StatusText::new(Status::Progress, title))
            .child(div().text_color(cx.palette().fg2).child(self.step))
    }
}

pub type ShowAll = Rc<dyn Fn(&mut Window, &mut App)>;

const CLEAR_FILTER: &str = "Clear filter";
const SHOW_ALL_NAMESPACES: &str = "Show all namespaces";

#[derive(IntoElement)]
pub struct EmptyState {
    selector: &'static str,
    sentence: SharedString,
    detail: Option<SharedString>,
    actions: Vec<Button>,
    centered: bool,
}

impl EmptyState {
    pub fn new(sentence: impl Into<SharedString>) -> Self {
        Self {
            selector: "empty-state",
            sentence: sentence.into(),
            detail: None,
            actions: Vec::new(),
            centered: false,
        }
    }

    pub fn centered(mut self) -> Self {
        self.centered = true;
        self
    }

    pub fn selector(mut self, selector: &'static str) -> Self {
        self.selector = selector;
        self
    }

    pub fn sentence(&self) -> &str {
        &self.sentence
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn action(mut self, action: Button) -> Self {
        self.actions.push(action);
        self
    }

    pub fn for_list(
        emptiness: &Emptiness,
        noun: &Noun,
        cluster: &str,
        filter: &Entity<FilterBar>,
        show_all: Option<ShowAll>,
    ) -> Self {
        let mut state = Self::new(emptiness.sentence(noun, cluster));
        state.selector = match emptiness {
            Emptiness::Cluster => "empty-state-cluster",
            Emptiness::Namespace { .. } => "empty-state-namespace",
            Emptiness::Filter { .. } => "empty-state-filter",
        };
        state.detail = emptiness.detail(noun).map(SharedString::from);
        match (emptiness, show_all) {
            (Emptiness::Namespace { .. }, Some(show_all)) => {
                let all = Button::new("empty-show-all-namespaces", SHOW_ALL_NAMESPACES)
                    .primary()
                    .on_click(move |_, window, cx| show_all(window, cx));
                state.actions.push(all);
            }
            (Emptiness::Filter { .. }, _) => {
                let filter = filter.clone();
                let clear = Button::new("empty-clear-filter", CLEAR_FILTER)
                    .primary()
                    .on_click(move |_, window, cx| {
                        filter.update(cx, |bar, cx| bar.clear(window, cx));
                    });
                state.actions.push(clear);
            }
            (Emptiness::Cluster | Emptiness::Namespace { .. }, _) => {}
        }
        state
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let selector = self.selector;
        let actions =
            (!self.actions.is_empty()).then(|| div().flex().gap(SPACE_4).children(self.actions));
        div()
            .debug_selector(|| selector.to_owned())
            .flex()
            .flex_col()
            .gap(SPACE_5)
            .px(SPACE_8)
            .map(|state| {
                if self.centered {
                    state.items_center().text_center()
                } else {
                    state.items_start().pt(SPACE_8)
                }
            })
            .text_color(palette.fg2)
            .child(self.sentence)
            .children(
                self.detail
                    .map(|detail| div().text_color(palette.mute).child(detail)),
            )
            .children(actions)
    }
}

#[derive(IntoElement)]
pub struct ErrorState {
    reason: SharedString,
    retry: Button,
}

impl ErrorState {
    pub fn new(reason: &str, retry: Button) -> Self {
        let words: Vec<&str> = reason.split_whitespace().collect();
        Self {
            reason: words.join(" ").into(),
            retry,
        }
    }
}

impl RenderOnce for ErrorState {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .debug_selector(|| "error-state".to_owned())
            .flex()
            .flex_col()
            .items_start()
            .gap(SPACE_5)
            .px(SPACE_8)
            .pt(SPACE_8)
            .child(StatusText::new(Status::Err, self.reason))
            .child(self.retry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{highlight, theme::Appearance};
    use flock_core::app_settings::Highlight;

    const MAX_BLOCK_LIGHTNESS_GAP: f32 = 0.06;
    const MAX_SHIMMER_LIGHTNESS_GAP: f32 = 0.05;

    #[test]
    fn skeletons_stay_close_to_the_surface_and_shimmer_never_darkens_them() {
        for (appearance, base) in [
            (Appearance::Dark, Palette::dark()),
            (Appearance::Light, Palette::light()),
        ] {
            for primary in Highlight::PRESETS {
                for secondary in Highlight::PRESETS {
                    let palette = highlight::apply(base, appearance, primary, secondary);
                    let shine = shimmer_color(&palette);
                    assert!((palette.hover.l - palette.editor.l).abs() < MAX_BLOCK_LIGHTNESS_GAP);
                    assert!(shine.l >= palette.hover.l);
                    assert!(shine.l - palette.hover.l < MAX_SHIMMER_LIGHTNESS_GAP);
                    assert_eq!(palette.hover, base.hover);
                    assert_eq!(shine, shimmer_color(&base));
                }
            }
        }
    }
}
