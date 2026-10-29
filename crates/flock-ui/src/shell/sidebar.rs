use std::collections::BTreeMap;

use flock_core::{ClusterSummary, Command, Counts, NavSection, Page, Status};
use gpui::{
    Animation, AnimationExt, AnyElement, App, ClickEvent, Context, CursorStyle, DispatchPhase,
    ElementId, FontWeight, Hsla, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, SharedString, StatefulInteractiveElement,
    Styled, Window, canvas, div, ease_out_quint, prelude::FluentBuilder, px,
};

use super::Shell;
use super::keys::SIDEBAR_CONTEXT;
use crate::kit::scroll_viewport;
use crate::kit::{Toast, status_symbols, toast};
use crate::theme::{
    ActiveTheme, COLLAPSE, FONT_MONO, PAGE_HEADER, Palette, RADIUS, ROW_NAV, RULE, SIDEBAR,
    SIDEBAR_CLOSE, SIDEBAR_MAX, SIDEBAR_MIN, SPACE_1, SPACE_2, SPACE_3, SPACE_4, SPACE_6, SPACE_8,
    SPLITTER, TEXT_LABEL, TEXT_UI, TITLE_ROW, TRAFFIC_LIGHTS_INSET,
};

const EDGE_GROUP: &str = "sidebar-edge";
const REOPEN_GROUP: &str = "sidebar-reopen";
const SAVE_FAILED: &str = "Could not save the sidebar width";

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SidebarDrag {
    start: Pixels,
    initial: Pixels,
    width: Pixels,
}

fn dragged_width(initial: Pixels, start: Pixels, now: Pixels) -> Option<Pixels> {
    let width = initial + now - start;
    (width >= SIDEBAR_CLOSE).then(|| width.clamp(SIDEBAR_MIN, SIDEBAR_MAX))
}

fn sidebar_width(cx: &App) -> Pixels {
    let saved = crate::settings::current(cx).sidebar_width;
    saved.map_or(SIDEBAR, |width| {
        px(f32::from(width)).clamp(SIDEBAR_MIN, SIDEBAR_MAX)
    })
}

impl Shell {
    pub(super) fn sidebar(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let palette = *cx.palette();
        let open = self.state.sidebar_open;
        let width = self.open_width(cx);
        self.forget_stale_nav_cursor();
        let highlight = self.sidebar_highlight(window);
        let content = open.then(|| {
            self.sidebar_open(highlight, width, &palette, cx)
                .into_any_element()
        });
        let edge = open.then(|| self.sidebar_edge(&palette, cx));
        let (from, to) = if open {
            (Pixels::ZERO, width)
        } else {
            (width, Pixels::ZERO)
        };
        let panel = div()
            .debug_selector(|| "sidebar".to_owned())
            .id("sidebar")
            .h_full()
            .flex_none()
            .overflow_hidden()
            .w(to)
            .bg(palette.panel)
            .children(content)
            .children(edge);
        self.slide(panel, "sidebar", from, to)
    }

    fn open_width(&self, cx: &App) -> Pixels {
        self.sidebar_drag
            .map_or_else(|| sidebar_width(cx), |drag| drag.width)
    }

    fn sidebar_edge(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let line = self.drag_line(palette.line, EDGE_GROUP, palette);
        div()
            .debug_selector(|| "sidebar-resize".to_owned())
            .id("sidebar-resize")
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(SPLITTER)
            .flex()
            .justify_end()
            .group(EDGE_GROUP)
            .cursor(CursorStyle::ResizeLeftRight)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.start_sidebar_drag(event.position.x, cx);
                }),
            )
            .child(line)
            .into_any_element()
    }

    pub(super) fn sidebar_reopen(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.state.sidebar_open {
            return None;
        }
        let palette = *cx.palette();
        let line = self.drag_line(gpui::transparent_black(), REOPEN_GROUP, &palette);
        let handle = div()
            .debug_selector(|| "sidebar-reopen".to_owned())
            .id("sidebar-reopen")
            .absolute()
            .top(TITLE_ROW)
            .bottom_0()
            .left_0()
            .w(SPLITTER)
            .group(REOPEN_GROUP)
            .cursor(CursorStyle::ResizeLeftRight)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.start_sidebar_drag(event.position.x, cx);
                }),
            )
            .child(line);
        Some(handle.into_any_element())
    }

    fn drag_line(&self, rest: Hsla, group: &'static str, palette: &Palette) -> gpui::Div {
        let accent = palette.acc;
        let color = if self.sidebar_drag.is_some() {
            accent
        } else {
            rest
        };
        div()
            .w(RULE)
            .h_full()
            .bg(color)
            .group_hover(group, move |style| style.bg(accent))
    }

    pub(super) fn sidebar_drag_tracker(cx: &mut Context<Self>) -> AnyElement {
        let shell = cx.entity();
        canvas(
            |_, _, _| (),
            move |_, (), window, _| {
                let moving = shell.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        moving.update(cx, |shell, cx| shell.drag_sidebar(event, cx));
                    }
                });
                let releasing = shell.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                        releasing.update(cx, Self::end_sidebar_drag);
                    }
                });
            },
        )
        .absolute()
        .size_full()
        .into_any_element()
    }

    fn start_sidebar_drag(&mut self, start: Pixels, cx: &mut Context<Self>) {
        let width = sidebar_width(cx);
        let initial = if self.state.sidebar_open {
            width
        } else {
            Pixels::ZERO
        };
        self.sidebar_generation = 0;
        self.sidebar_drag = Some(SidebarDrag {
            start,
            initial,
            width,
        });
        cx.notify();
    }

    fn drag_sidebar(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.sidebar_drag.is_none() {
            return;
        }
        if !event.dragging() {
            self.end_sidebar_drag(cx);
            return;
        }
        let Some(drag) = self.sidebar_drag.as_mut() else {
            return;
        };
        let resized = dragged_width(drag.initial, drag.start, event.position.x);
        if let Some(width) = resized {
            drag.width = width;
        }
        self.state.sidebar_open = resized.is_some();
        cx.notify();
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn end_sidebar_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.sidebar_drag.take() else {
            return;
        };
        let open = self.state.sidebar_open;
        let width = f32::from(drag.width).round() as u16;
        let saved = crate::settings::update(cx, |settings| {
            settings.sidebar_closed = !open;
            if open {
                settings.sidebar_width = Some(width);
            }
        });
        if let Err(error) = saved {
            toast(Toast::new(Status::Err, SAVE_FAILED).detail(error), cx);
        }
        cx.notify();
    }

    pub(super) fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_generation += 1;
        let closed = self.state.sidebar_open;
        let saved = crate::settings::update(cx, |settings| settings.sidebar_closed = closed);
        if let Err(error) = saved {
            toast(Toast::new(Status::Err, SAVE_FAILED).detail(error), cx);
        }
    }

    pub(super) fn window_controls(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.state.sidebar_open;
        if open && self.sidebar_generation == 0 {
            return None;
        }
        let palette = *cx.palette();
        let width = TRAFFIC_LIGHTS_INSET;
        let (from, to) = if open {
            (width, Pixels::ZERO)
        } else {
            (Pixels::ZERO, width)
        };
        let controls = div()
            .debug_selector(|| "window-controls".to_owned())
            .id("window-controls")
            .flex()
            .flex_col()
            .flex_none()
            .h_full()
            .overflow_hidden()
            .w(to)
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .child(div().h(RULE))
            .child(div().flex_1().w(width))
            .child(div().h(RULE).bg(palette.line));
        Some(self.slide(controls, "window-controls", from, to))
    }

    fn slide(
        &self,
        element: gpui::Stateful<gpui::Div>,
        name: &'static str,
        from: Pixels,
        to: Pixels,
    ) -> AnyElement {
        if self.sidebar_generation == 0 {
            return element.into_any_element();
        }
        let animation = Animation::new(COLLAPSE).with_easing(ease_out_quint());
        let key = ElementId::NamedInteger(name.into(), self.sidebar_generation as u64);
        element
            .with_animation(key, animation, move |element, delta| {
                element.w(from + (to - from) * delta)
            })
            .into_any_element()
    }

    pub(super) fn selected_cluster(&self) -> Option<&ClusterSummary> {
        let id = self.state.location().cluster.as_ref()?;
        self.state.snapshot.cluster(id)
    }

    fn nav_counts(&self) -> BTreeMap<Page, Counts> {
        let Some(cluster) = self.selected_cluster() else {
            let shown = self.focused_screens().scoped_snapshot();
            let snapshot = shown.unwrap_or(&self.state.snapshot);
            let mut counts = snapshot.flock_nav.clone();
            counts.extend(flock_core::across::badges(snapshot));
            return counts;
        };
        let mut counts = cluster.nav.clone();
        let definitions = self.state.snapshot.crds(&cluster.id).len();
        let definitions = u32::try_from(definitions).unwrap_or(u32::MAX);
        counts.insert(Page::CustomResources, Counts::total(definitions));
        let problems = self.state.snapshot.home.problems.iter();
        let found =
            flock_core::home::problem_counts(problems.filter(|p| p.target.cluster == cluster.id));
        let badge = Counts::default().with_err(found.err).with_warn(found.warn);
        counts.insert(Page::ClusterProblems, badge);
        counts
    }

    fn sidebar_open(
        &self,
        highlight: Page,
        width: Pixels,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let sections = self
            .state
            .snapshot
            .nav_sections(self.state.location().cluster.as_ref());
        let counts = self.nav_counts();
        let mut items: Vec<AnyElement> = Vec::new();
        for section in sections {
            if let Some(label) = section.label {
                items.push(section_head(label, palette).into_any_element());
            }
            items.extend(section_rows(section, highlight, &counts, palette, cx));
        }
        let scope: SharedString = self
            .state
            .location()
            .scope()
            .name(&self.state.snapshot.clusters)
            .into();
        let header = div()
            .debug_selector(|| "sidebar-scope".to_owned())
            .flex()
            .flex_none()
            .items_center()
            .h(PAGE_HEADER)
            .px(SPACE_8)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(TEXT_UI)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(scope),
            );
        div()
            .w(width)
            .h_full()
            .flex()
            .flex_col()
            .child(top_strip(palette))
            .child(header)
            .child(
                scroll_viewport("sidebar-nav", div().px(SPACE_4).pb(SPACE_3).children(items))
                    .key_context(SIDEBAR_CONTEXT)
                    .track_focus(&self.keys.sidebar_focus)
                    .on_key_down(cx.listener(Self::sidebar_key))
                    .flex_1()
                    .min_h_0(),
            )
    }
}

fn top_strip(palette: &Palette) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .h(TITLE_ROW)
        .flex_none()
        .bg(palette.chrome)
        .border_b(RULE)
        .border_color(palette.line)
        .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
}

fn section_rows(
    section: &NavSection,
    highlight: Page,
    counts: &BTreeMap<Page, Counts>,
    palette: &Palette,
    cx: &mut Context<Shell>,
) -> Vec<AnyElement> {
    section
        .pages
        .iter()
        .map(|&page| {
            let selected = page == highlight;
            let selector = format!("nav-{}", page.label().to_lowercase());
            let label_selector = format!("{selector}-label");
            let badges = counts
                .get(&page)
                .map(|c| badges(c, &selector, palette))
                .unwrap_or_default();
            let key = ElementId::NamedInteger("page".into(), page as u64);
            nav_row(key, ROW_NAV, selected, palette)
                .debug_selector(move || selector.clone())
                .when(!selected, |row| row.text_color(palette.fg2))
                .child(
                    div()
                        .debug_selector(move || label_selector)
                        .flex_1()
                        .truncate()
                        .child(page.label()),
                )
                .children(badges)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.dispatch(Command::SelectPage(page), cx);
                }))
                .on_aux_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                    if event.is_middle_click() {
                        this.dispatch(Command::SelectPage(page), cx);
                    }
                }))
                .into_any_element()
        })
        .collect()
}

fn badges(counts: &Counts, row: &str, palette: &Palette) -> Vec<AnyElement> {
    counts
        .badges()
        .into_iter()
        .map(|(status, n)| {
            let selector = format!("{row}-{status:?}").to_lowercase();
            let text = format!("{} {n}", status.glyph());
            badge(selector, text, palette.status(status))
        })
        .collect()
}

fn badge(selector: String, text: String, color: Hsla) -> AnyElement {
    div()
        .debug_selector(move || selector)
        .flex_none()
        .ml(SPACE_1)
        .whitespace_nowrap()
        .font_family(FONT_MONO)
        .text_size(TEXT_LABEL)
        .text_color(color)
        .child(status_symbols(text))
        .into_any_element()
}

fn nav_row(
    id: impl Into<ElementId>,
    height: Pixels,
    selected: bool,
    palette: &Palette,
) -> gpui::Stateful<gpui::Div> {
    let hover = palette.hover;
    let id = id.into();
    div()
        .debug_selector(|| id.to_string())
        .id(id)
        .flex()
        .items_center()
        .h(height)
        .px(SPACE_4)
        .rounded(RADIUS)
        .cursor_pointer()
        .when(selected, |row| {
            row.bg(palette.sel)
                .text_color(palette.selfg)
                .font_weight(FontWeight::MEDIUM)
        })
        .when(!selected, |row| row.hover(move |style| style.bg(hover)))
}

fn section_head(text: &str, palette: &Palette) -> impl IntoElement {
    div()
        .pt(SPACE_6)
        .pb(SPACE_2)
        .px(SPACE_4)
        .text_size(TEXT_LABEL)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(palette.mute)
        .child(text.to_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_clamps_to_the_limits_and_closes_below_the_threshold() {
        let start = px(200.);
        assert_eq!(dragged_width(SIDEBAR, start, px(260.)), Some(px(260.)));
        assert_eq!(dragged_width(SIDEBAR, start, px(900.)), Some(SIDEBAR_MAX));
        assert_eq!(
            dragged_width(SIDEBAR, start, SIDEBAR_CLOSE),
            Some(SIDEBAR_MIN)
        );
        assert_eq!(dragged_width(SIDEBAR, start, SIDEBAR_CLOSE - px(1.)), None);
        assert_eq!(dragged_width(Pixels::ZERO, px(2.), px(42.)), None);
        assert_eq!(
            dragged_width(Pixels::ZERO, px(2.), px(302.)),
            Some(px(300.))
        );
    }
}
