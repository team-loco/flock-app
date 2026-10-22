use gpui::{
    Anchor, Animation, AnimationExt, AnyElement, BoxShadow, ElementId, InteractiveElement,
    IntoElement, ParentElement, Pixels, Point, Stateful, Styled, WeakEntity, anchored, deferred,
    div, ease_out_quint, point, px,
};

use crate::theme::{
    MENU_MAX_HEIGHT, POPOVER_IN, POPOVER_SLIDE, Palette, RADIUS_MENU, RULE, SHADOW_BLUR,
    SHADOW_OFFSET, SPACE_2, SPACE_3, SPACE_4,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit(Pixels);

impl Default for Fit {
    fn default() -> Self {
        Self(MENU_MAX_HEIGHT)
    }
}

impl Fit {
    pub fn height(self) -> Pixels {
        self.0
    }

    pub fn observe(&mut self, children: &[Pixels]) -> bool {
        let next = fit_height(children, MENU_MAX_HEIGHT);
        let changed = self.0 != next;
        self.0 = next;
        changed
    }
}

pub fn fit_height(children: &[Pixels], max: Pixels) -> Pixels {
    let border = RULE * 2.;
    let total = children.iter().fold(px(0.), |sum, height| sum + *height);
    if total + border + SPACE_2 * 2. <= max {
        return max;
    }
    let room = max - border - SPACE_2;
    let mut shown = px(0.);
    for height in children {
        if shown + *height > room {
            break;
        }
        shown += *height;
    }
    if shown <= px(0.) {
        return max;
    }
    border + SPACE_2 + shown
}

pub fn fitted_surface<T: 'static>(
    id: impl Into<ElementId>,
    palette: &Palette,
    this: WeakEntity<T>,
    fit: fn(&mut T) -> &mut Fit,
) -> Stateful<gpui::Div> {
    let measure =
        move |children: Vec<gpui::Bounds<Pixels>>, _: &mut gpui::Window, cx: &mut gpui::App| {
            let heights: Vec<Pixels> = children.iter().map(|bounds| bounds.size.height).collect();
            this.update(cx, |owner, cx| {
                if fit(owner).observe(&heights) {
                    cx.notify();
                }
            })
            .ok();
        };
    surface_in(div().on_children_prepainted(measure), id, palette)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Next,
    Previous,
}

pub fn step(
    current: Option<usize>,
    len: usize,
    enabled: impl Fn(usize) -> bool,
    direction: Step,
) -> Option<usize> {
    let candidates: Vec<usize> = (0..len).filter(|&ix| enabled(ix)).collect();
    let first = candidates.first().copied();
    let last = candidates.last().copied();
    let Some(current) = current else {
        return match direction {
            Step::Next => first,
            Step::Previous => last,
        };
    };
    match direction {
        Step::Next => candidates.into_iter().find(|&ix| ix > current).or(first),
        Step::Previous => candidates
            .into_iter()
            .rev()
            .find(|&ix| ix < current)
            .or(last),
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Dismissal {
    trigger_hovered: bool,
    suppress_click: bool,
}

impl Dismissal {
    pub fn hover(&mut self, hovered: bool) {
        self.trigger_hovered = hovered;
    }

    pub fn clicked_outside(&mut self) {
        self.suppress_click = self.trigger_hovered;
    }

    pub fn take_suppressed(&mut self) -> bool {
        std::mem::take(&mut self.suppress_click)
    }
}

pub fn shadow(palette: &Palette) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: palette.shadow,
        offset: point(Pixels::ZERO, SHADOW_OFFSET),
        blur_radius: SHADOW_BLUR,
        spread_radius: Pixels::ZERO,
        inset: false,
    }]
}

pub fn surface(id: impl Into<ElementId>, palette: &Palette) -> Stateful<gpui::Div> {
    surface_in(div(), id, palette)
}

fn surface_in(base: gpui::Div, id: impl Into<ElementId>, palette: &Palette) -> Stateful<gpui::Div> {
    base.id(id)
        .occlude()
        .flex()
        .flex_col()
        .p(SPACE_2)
        .bg(palette.panel)
        .border(RULE)
        .border_color(palette.line2)
        .rounded(RADIUS_MENU)
        .shadow(shadow(palette))
}

pub fn appear<E: IntoElement + Styled + 'static>(element: E, key: ElementId) -> AnyElement {
    let animation = Animation::new(POPOVER_IN).with_easing(ease_out_quint());
    element
        .with_animation(key, animation, |element, delta| {
            element.opacity(delta).mt(POPOVER_SLIDE * (1. - delta))
        })
        .into_any_element()
}

pub fn below_right(trigger_height: Pixels, popover: AnyElement) -> impl IntoElement {
    div()
        .absolute()
        .right_0()
        .top(trigger_height + SPACE_3)
        .child(
            deferred(
                anchored()
                    .anchor(Anchor::TopRight)
                    .snap_to_window_with_margin(SPACE_4)
                    .child(popover),
            )
            .priority(1),
        )
}

pub fn below_left(trigger_height: Pixels, popover: AnyElement) -> impl IntoElement {
    div()
        .absolute()
        .left_0()
        .top(trigger_height + SPACE_3)
        .child(
            deferred(
                anchored()
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(SPACE_4)
                    .child(popover),
            )
            .priority(1),
        )
}

pub fn at(position: Point<Pixels>, popover: AnyElement) -> impl IntoElement {
    deferred(
        anchored()
            .position(position)
            .snap_to_window_with_margin(SPACE_4)
            .child(popover),
    )
    .priority(1)
}

pub fn above_left(popover: AnyElement) -> impl IntoElement {
    div().absolute().left(SPACE_3).top(-SPACE_2).child(
        deferred(
            anchored()
                .anchor(Anchor::BottomLeft)
                .snap_to_window_with_margin(SPACE_4)
                .child(popover),
        )
        .priority(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_skip_disabled_items_and_wrap() {
        let enabled = |ix: usize| ix != 2;
        assert_eq!(step(None, 4, enabled, Step::Next), Some(0));
        assert_eq!(step(None, 4, enabled, Step::Previous), Some(3));
        assert_eq!(step(Some(1), 4, enabled, Step::Next), Some(3));
        assert_eq!(step(Some(3), 4, enabled, Step::Next), Some(0));
        assert_eq!(step(Some(3), 4, enabled, Step::Previous), Some(1));
        assert_eq!(step(Some(0), 4, enabled, Step::Previous), Some(3));
    }

    #[test]
    fn steps_nowhere_when_nothing_is_enabled() {
        assert_eq!(step(None, 3, |_| false, Step::Next), None);
        assert_eq!(step(Some(1), 0, |_| true, Step::Previous), None);
    }

    fn rows(heights: &[f32]) -> Vec<Pixels> {
        heights.iter().map(|height| px(*height)).collect()
    }

    #[test]
    fn a_list_that_fits_keeps_the_max_height() {
        let max = px(360.);
        assert_eq!(fit_height(&rows(&[100., 100.]), max), max);
    }

    #[test]
    fn a_tall_list_shows_only_whole_rows() {
        let max = px(360.);
        let children = rows(&[40., 52., 40., 52., 40., 52., 40., 52.]);
        let height = fit_height(&children, max);
        assert!(height <= max);
        let shown = height - RULE * 2. - SPACE_2;
        let mut edge = px(0.);
        let on_edge = std::iter::once(px(0.)).chain(children.iter().map(|row| {
            edge += *row;
            edge
        }));
        assert!(on_edge.into_iter().any(|bottom| bottom == shown));
        assert!(height > max - px(52.));
    }

    #[test]
    fn a_first_row_taller_than_the_max_falls_back_to_the_max() {
        let max = px(100.);
        assert_eq!(fit_height(&rows(&[200., 40.]), max), max);
    }
}
