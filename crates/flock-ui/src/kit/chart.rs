use flock_core::Status;
use flock_core::overview::{Capacity, percent};
use flock_core::units::fixed;
use gpui::{
    AnyElement, App, Bounds, Div, Hsla, InteractiveElement, IntoElement, ParentElement,
    PathBuilder, Pixels, Point, RenderOnce, SharedString, StatefulInteractiveElement, Styled,
    Window, canvas, div, fill, point, prelude::FluentBuilder, relative,
};

use crate::theme::{
    ActiveTheme, CHART_AXIS, CHART_DASH, CHART_HEIGHT, CHART_STROKE, FONT_MONO, LEGEND_SWATCH,
    METER, METER_MARKER, METER_MARKER_WIDTH, METER_MINI, METER_MINI_WIDTH, RULE, SPACE_1, SPACE_3,
    SPACE_4, SPACE_7, TEXT_AXIS, TEXT_CONTROL,
};

const DASH_GAP_SWATCH: f32 = 0.5;
const EDGE_LABEL_SHARE: f32 = 1.;
const INNER_LABEL_SHARE: f32 = 2.;

fn share(ix: usize, steps: usize) -> f32 {
    let as_f32 = |n: usize| f32::from(u16::try_from(n).unwrap_or(u16::MAX));
    as_f32(ix) / as_f32(steps.max(1))
}

pub fn fraction(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0., 1.)
    } else {
        0.
    }
}

#[derive(IntoElement)]
pub struct Meter {
    value: f32,
    request: Option<f32>,
    limit: Option<f32>,
    scale: f32,
    color: Option<Hsla>,
}

impl Meter {
    pub fn new(value: f32) -> Self {
        Self {
            value,
            request: None,
            limit: None,
            scale: 1.,
            color: None,
        }
    }

    fn scale(mut self, scale: f32) -> Self {
        self.scale = scale;
        self
    }

    pub fn request(mut self, at: f32) -> Self {
        self.request = Some(at);
        self
    }

    pub fn limit(mut self, at: f32) -> Self {
        self.limit = Some(at);
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

fn marker(at: f32, color: Hsla) -> Div {
    div()
        .absolute()
        .left(relative(fraction(at)))
        .when(at >= 1., |marker| marker.ml(-METER_MARKER_WIDTH))
        .top((METER - METER_MARKER) / 2.)
        .w(METER_MARKER_WIDTH)
        .h(METER_MARKER)
        .bg(color)
}

impl RenderOnce for Meter {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = cx.palette();
        let color = self.color.unwrap_or(palette.acc);
        let scale = capacity_scale(&[self.scale]);
        div()
            .relative()
            .w_full()
            .h(METER)
            .bg(palette.line2)
            .child(
                div()
                    .h_full()
                    .w(relative(fraction(self.value / scale)))
                    .bg(color),
            )
            .when(scale > 1., |meter| {
                meter
                    .child(
                        div()
                            .absolute()
                            .left(relative(1. / scale))
                            .right_0()
                            .h_full()
                            .bg(palette.warnbg),
                    )
                    .child(marker(1. / scale, palette.mute))
            })
            .when_some(self.request, |meter, at| {
                meter.child(marker(at / scale, palette.fg2))
            })
            .when_some(self.limit, |meter, at| {
                meter.child(marker(at / scale, palette.warn).top(Pixels::ZERO))
            })
    }
}

fn capacity_scale(values: &[f32]) -> f32 {
    values
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .fold(1., f32::max)
}

#[derive(IntoElement)]
pub struct CapacityMeter {
    id: gpui::ElementId,
    capacity: Capacity,
    color: Hsla,
}

impl CapacityMeter {
    pub fn new(id: impl Into<gpui::ElementId>, capacity: Capacity, color: Hsla) -> Self {
        Self {
            id: id.into(),
            capacity,
            color,
        }
    }
}

impl RenderOnce for CapacityMeter {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let capacity = self.capacity;
        let usage = capacity.fraction();
        let scale = capacity_scale(&[usage, capacity.requests, capacity.limits]);
        let tooltip = format!(
            "Usage: {} {}\nRequests: {} {} ({})\nLimits: {} {} ({})\nCapacity: {} {}",
            fixed(capacity.used),
            capacity.unit,
            fixed(capacity.requests * capacity.total),
            capacity.unit,
            percent(capacity.requests),
            fixed(capacity.limits * capacity.total),
            capacity.unit,
            percent(capacity.limits),
            fixed(capacity.total),
            capacity.unit,
        );
        let legend = [
            ("Usage", usage, self.color),
            ("Requests", capacity.requests, palette.fg2),
            ("Limits", capacity.limits, palette.warn),
        ]
        .into_iter()
        .map(|(label, value, color)| {
            let swatch = if label == "Usage" {
                div().w(LEGEND_SWATCH).h(RULE).bg(color)
            } else {
                div().w(METER_MARKER_WIDTH).h(LEGEND_SWATCH).bg(color)
            };
            div()
                .flex()
                .items_center()
                .gap(SPACE_3)
                .child(swatch)
                .child(format!("{label} {}", percent(value)))
        });
        let meter = Meter::new(usage)
            .request(capacity.requests)
            .limit(capacity.limits)
            .color(self.color)
            .scale(scale);
        div()
            .debug_selector(|| self.id.to_string())
            .id(self.id)
            .flex()
            .flex_col()
            .gap(SPACE_4)
            .tooltip(super::Tooltip::text(tooltip))
            .child(meter)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(SPACE_4)
                    .text_size(TEXT_CONTROL)
                    .text_color(palette.mute)
                    .children(legend),
            )
            .when(scale > 1., |row| {
                row.child(
                    div()
                        .text_size(TEXT_CONTROL)
                        .text_color(palette.warn)
                        .child(format!("100% capacity · overcommit to {}", percent(scale))),
                )
            })
    }
}

#[derive(IntoElement)]
pub struct MiniMeter {
    value: f32,
    color: Option<Hsla>,
}

impl MiniMeter {
    pub fn new(value: f32) -> Self {
        Self { value, color: None }
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for MiniMeter {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = cx.palette();
        div()
            .flex_none()
            .w(METER_MINI_WIDTH)
            .h(METER_MINI)
            .bg(palette.line2)
            .child(
                div()
                    .h_full()
                    .w(relative(fraction(self.value)))
                    .bg(self.color.unwrap_or(palette.fg2)),
            )
    }
}

#[derive(IntoElement)]
pub struct StackedBar {
    segments: Vec<(Status, f32)>,
    thin: bool,
}

impl StackedBar {
    pub fn new(segments: Vec<(Status, f32)>) -> Self {
        Self {
            segments,
            thin: false,
        }
    }

    pub fn thin(mut self) -> Self {
        self.thin = true;
        self
    }
}

impl RenderOnce for StackedBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let parts = self
            .segments
            .into_iter()
            .filter(|(_, weight)| weight.is_finite() && *weight > 0.)
            .map(|(status, weight)| {
                div()
                    .h_full()
                    .flex_grow(weight)
                    .flex_basis(Pixels::ZERO)
                    .bg(palette.status(status))
            });
        div()
            .flex()
            .gap(SPACE_1)
            .w_full()
            .h(if self.thin { METER_MINI } else { METER })
            .bg(palette.line2)
            .children(parts)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    label: SharedString,
    color: Hsla,
    area: Option<Hsla>,
    values: Vec<f32>,
    positions: Vec<f32>,
}

impl Series {
    pub fn new(label: impl Into<SharedString>, color: Hsla, values: Vec<f32>) -> Self {
        Self {
            label: label.into(),
            color,
            area: None,
            values,
            positions: Vec::new(),
        }
    }

    pub fn positions(mut self, positions: Vec<f32>) -> Self {
        self.positions = positions;
        self
    }

    pub fn area(mut self, fill: Hsla) -> Self {
        self.area = Some(fill);
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Threshold {
    value: f32,
    label: SharedString,
    color: Hsla,
}

pub fn plot_points(values: &[f32], max: f32, bounds: Bounds<Pixels>) -> Vec<Point<Pixels>> {
    let steps = values.len().saturating_sub(1).max(1);
    let positions: Vec<f32> = (0..values.len()).map(|ix| share(ix, steps)).collect();
    positioned_points(values, &positions, max, bounds)
}

pub fn positioned_points(
    values: &[f32],
    positions: &[f32],
    max: f32,
    bounds: Bounds<Pixels>,
) -> Vec<Point<Pixels>> {
    values
        .iter()
        .zip(positions)
        .map(|(value, at)| {
            let x = bounds.origin.x + bounds.size.width * fraction(*at);
            let level = if max > 0. { fraction(value / max) } else { 0. };
            let y = bounds.origin.y + bounds.size.height * (1. - level);
            point(x, y)
        })
        .collect()
}

#[derive(IntoElement)]
pub struct LineChart {
    series: Vec<Series>,
    max: f32,
    y_labels: Vec<SharedString>,
    x_labels: Vec<SharedString>,
    threshold: Option<Threshold>,
    height: Pixels,
    title: Option<AnyElement>,
    message: Option<AnyElement>,
}

impl LineChart {
    pub fn new(max: f32) -> Self {
        Self {
            series: Vec::new(),
            max,
            y_labels: Vec::new(),
            x_labels: Vec::new(),
            threshold: None,
            height: CHART_HEIGHT,
            title: None,
            message: None,
        }
    }

    pub fn series(mut self, series: Series) -> Self {
        self.series.push(series);
        self
    }

    pub fn y_labels(mut self, labels: Vec<SharedString>) -> Self {
        self.y_labels = labels;
        self
    }

    pub fn x_labels(mut self, labels: Vec<SharedString>) -> Self {
        self.x_labels = labels;
        self
    }

    pub fn threshold(mut self, value: f32, label: impl Into<SharedString>, color: Hsla) -> Self {
        self.threshold = Some(Threshold {
            value,
            label: label.into(),
            color,
        });
        self
    }

    pub fn height(mut self, height: Pixels) -> Self {
        self.height = height;
        self
    }

    pub fn message(mut self, message: impl IntoElement) -> Self {
        self.message = Some(message.into_any_element());
        self
    }

    pub fn title(mut self, title: impl IntoElement) -> Self {
        self.title = Some(title.into_any_element());
        self
    }
}

fn paint_chart(
    bounds: Bounds<Pixels>,
    series: &[Series],
    max: f32,
    gridlines: usize,
    grid: Hsla,
    threshold: Option<&Threshold>,
    window: &mut Window,
) {
    let rows = gridlines.saturating_sub(1).max(1);
    for ix in 0..gridlines {
        let y = bounds.origin.y + (bounds.size.height - RULE) * share(ix, rows);
        let line = Bounds::new(
            point(bounds.origin.x, y),
            gpui::size(bounds.size.width, RULE),
        );
        window.paint_quad(fill(line, grid));
    }
    for line in series {
        let points = if line.positions.is_empty() {
            plot_points(&line.values, max, bounds)
        } else {
            positioned_points(&line.values, &line.positions, max, bounds)
        };
        if let [only] = points.as_slice() {
            let half_stroke = CHART_STROKE / 2.;
            let origin = *only - point(half_stroke, half_stroke);
            let size = gpui::size(CHART_STROKE, CHART_STROKE);
            let dot = Bounds::new(origin, size);
            window.paint_quad(fill(dot, line.color));
        }
        if let (Some(area), Some(first), Some(last)) = (line.area, points.first(), points.last()) {
            let bottom = bounds.origin.y + bounds.size.height;
            let mut shape = PathBuilder::fill();
            shape.move_to(point(first.x, bottom));
            for p in &points {
                shape.line_to(*p);
            }
            shape.line_to(point(last.x, bottom));
            shape.close();
            if let Ok(path) = shape.build() {
                window.paint_path(path, area);
            }
        }
        let mut stroke = PathBuilder::stroke(CHART_STROKE);
        if let Some((first, rest)) = points.split_first() {
            stroke.move_to(*first);
            for p in rest {
                stroke.line_to(*p);
            }
        }
        if let Ok(path) = stroke.build() {
            window.paint_path(path, line.color);
        }
    }
    if let Some(threshold) = threshold {
        let level = if max > 0. {
            fraction(threshold.value / max)
        } else {
            0.
        };
        let y = bounds.origin.y + bounds.size.height * (1. - level);
        let mut dash = PathBuilder::stroke(RULE).dash_array(&[CHART_DASH, CHART_DASH]);
        dash.move_to(point(bounds.origin.x, y));
        dash.line_to(point(bounds.origin.x + bounds.size.width, y));
        if let Ok(path) = dash.build() {
            window.paint_path(path, threshold.color);
        }
    }
}

fn legend_item(label: SharedString, swatch: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(SPACE_3)
        .child(swatch)
        .child(label)
}

fn x_axis(labels: Vec<SharedString>) -> impl Iterator<Item = Div> {
    let last = labels.len().saturating_sub(1);
    labels.into_iter().enumerate().map(move |(ix, label)| {
        let cell = div()
            .flex()
            .flex_basis(Pixels::ZERO)
            .min_w_0()
            .whitespace_nowrap()
            .child(
                div()
                    .debug_selector(move || format!("chart-x-label-{ix}"))
                    .flex_none()
                    .child(label),
            );
        if ix == 0 {
            cell.flex_grow(EDGE_LABEL_SHARE).justify_start()
        } else if ix == last {
            cell.flex_grow(EDGE_LABEL_SHARE).justify_end()
        } else {
            cell.flex_grow(INNER_LABEL_SHARE).justify_center()
        }
    })
}

impl RenderOnce for LineChart {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let mut legend: Vec<_> = self
            .series
            .iter()
            .map(|s| {
                legend_item(
                    s.label.clone(),
                    div().w(LEGEND_SWATCH).h(CHART_STROKE).bg(s.color),
                )
                .into_any_element()
            })
            .collect();
        if let Some(threshold) = &self.threshold {
            let dash = LEGEND_SWATCH * DASH_GAP_SWATCH;
            let swatch = div()
                .flex()
                .gap(dash / 2.)
                .child(div().w(dash / 2.).h(RULE).bg(threshold.color))
                .child(div().w(dash / 2.).h(RULE).bg(threshold.color));
            legend.push(legend_item(threshold.label.clone(), swatch).into_any_element());
        }
        let header = div()
            .flex()
            .items_center()
            .gap(SPACE_7)
            .text_size(TEXT_CONTROL)
            .text_color(palette.mute)
            .children(self.title)
            .child(div().flex_1())
            .children(legend);
        let chart = div().flex().flex_col().gap(SPACE_4).w_full().child(header);
        if let Some(message) = self.message {
            return chart.child(
                div()
                    .debug_selector(|| "chart-message".to_owned())
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(self.height)
                    .child(message),
            );
        }
        let gridlines = self.y_labels.len().max(2);
        let series = self.series;
        let max = self.max;
        let threshold = self.threshold;
        let grid = palette.line;
        let plot = canvas(
            |_, _, _| {},
            move |bounds, (), window, _| {
                paint_chart(
                    bounds,
                    &series,
                    max,
                    gridlines,
                    grid,
                    threshold.as_ref(),
                    window,
                );
            },
        )
        .size_full();
        let y_axis = div()
            .debug_selector(|| "chart-y-axis".to_owned())
            .flex()
            .flex_col()
            .justify_between()
            .items_end()
            .w(CHART_AXIS)
            .h(self.height)
            .font_family(FONT_MONO)
            .text_size(TEXT_AXIS)
            .text_color(palette.mute)
            .children(self.y_labels.into_iter().map(|l| div().child(l)));
        let plot = div()
            .debug_selector(|| "chart-plot".to_owned())
            .flex_1()
            .h(self.height)
            .child(plot);
        let x_axis = div()
            .debug_selector(|| "chart-x-axis".to_owned())
            .flex()
            .pl(CHART_AXIS + SPACE_4)
            .font_family(FONT_MONO)
            .text_size(TEXT_AXIS)
            .text_color(palette.mute)
            .children(x_axis(self.x_labels));
        chart
            .child(div().flex().gap(SPACE_4).child(y_axis).child(plot))
            .child(x_axis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    #[test]
    fn fractions_clamp_and_reject_nan() {
        assert!((fraction(1.31) - 1.).abs() < f32::EPSILON);
        assert!(fraction(-0.2).abs() < f32::EPSILON);
        assert!(fraction(f32::NAN).abs() < f32::EPSILON);
        assert!((fraction(0.61) - 0.61).abs() < f32::EPSILON);
    }

    #[test]
    fn points_span_the_bounds_and_invert_y() {
        let bounds = Bounds::new(point(px(10.), px(0.)), gpui::size(px(100.), px(50.)));
        let points = plot_points(&[0., 50., 100.], 100., bounds);
        assert_eq!(
            points,
            [
                point(px(10.), px(50.)),
                point(px(60.), px(25.)),
                point(px(110.), px(0.)),
            ]
        );
    }

    #[test]
    fn timestamp_positions_preserve_partial_history_and_gaps() {
        let bounds = Bounds::new(point(px(0.), px(0.)), gpui::size(px(100.), px(50.)));
        assert_eq!(
            positioned_points(&[50., 100.], &[0.9, 1.], 100., bounds),
            [point(px(90.), px(25.)), point(px(100.), px(0.))]
        );
        assert_eq!(positioned_points(&[], &[], 100., bounds), []);
    }

    #[test]
    fn a_single_value_and_zero_max_stay_inside() {
        let bounds = Bounds::new(point(px(0.), px(0.)), gpui::size(px(10.), px(10.)));
        assert_eq!(plot_points(&[5.], 0., bounds), [point(px(0.), px(10.))]);
    }
}

#[cfg(test)]
mod capacity_tests {
    use super::*;

    #[test]
    fn capacity_scale_preserves_overcommit_and_ignores_invalid_values() {
        assert_eq!(capacity_scale(&[0.61, 0.74, 1.31]), 1.31);
        assert_eq!(capacity_scale(&[0.61, 0.66, 0.92]), 1.);
        assert_eq!(capacity_scale(&[f32::NAN, f32::INFINITY, -1.]), 1.);
        assert_eq!(capacity_scale(&[2., 1.5, 1.31]), 2.);
    }
}
