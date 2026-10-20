use std::collections::BTreeMap;

use flock_core::switcher::ALL_CLUSTERS;
use flock_core::{ClusterSummary, Command, Counts, NavSection, Page};
use gpui::{
    Animation, AnimationExt, AnyElement, ClickEvent, Context, ElementId, FontWeight, Hsla,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, SharedString,
    StatefulInteractiveElement, Styled, Window, div, ease_out_quint, prelude::FluentBuilder, svg,
};

use super::Shell;
use super::keys::SIDEBAR_CONTEXT;
use crate::assets::{CHEVRON_LEFT, CHEVRON_RIGHT};
use crate::kit::{Tooltip, status_symbols};
use crate::theme::{
    ActiveTheme, COLLAPSE, FONT_MONO, ICON, ICON_BUTTON, PAGE_HEADER, Palette, RADIUS, ROW_NAV,
    RULE, SIDEBAR, SPACE_2, SPACE_3, SPACE_4, SPACE_6, SPACE_8, TEXT_LABEL, TEXT_SMALL, TEXT_UI,
    TITLE_ROW, TRAFFIC_LIGHTS_INSET,
};

const COLLAPSE_TOOLTIP: &str = "Collapse sidebar  ⌘B";
const EXPAND_TOOLTIP: &str = "Expand sidebar  ⌘B";

impl Shell {
    pub(super) fn sidebar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let palette = *cx.palette();
        let open = self.state.sidebar_open;
        let cursor = self
            .keys
            .sidebar_focus
            .is_focused(window)
            .then(|| self.keys.nav_cursor.unwrap_or(self.state.location().page));
        let content = open.then(|| self.sidebar_open(cursor, &palette, cx));
        let (from, to) = if open {
            (Pixels::ZERO, SIDEBAR)
        } else {
            (SIDEBAR, Pixels::ZERO)
        };
        let panel = div()
            .debug_selector(|| "sidebar".to_owned())
            .id("sidebar")
            .h_full()
            .flex_none()
            .overflow_hidden()
            .w(to)
            .bg(palette.panel)
            .when(open, |panel| {
                panel.border_r(RULE).border_color(palette.line)
            })
            .children(content);
        self.slide(panel, "sidebar", from, to)
    }

    pub(super) fn window_controls(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.state.sidebar_open;
        if open && self.sidebar_generation == 0 {
            return None;
        }
        let palette = *cx.palette();
        let expand = icon_button("expand-sidebar", CHEVRON_RIGHT, EXPAND_TOOLTIP, &palette)
            .on_click(cx.listener(|this, _, _, cx| this.dispatch(Command::ToggleSidebar, cx)));
        let width = TRAFFIC_LIGHTS_INSET + ICON_BUTTON + SPACE_3;
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
            .child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .w(width)
                    .pl(TRAFFIC_LIGHTS_INSET)
                    .child(expand),
            )
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
            return self.state.snapshot.flock_nav.clone();
        };
        let mut counts = cluster.nav.clone();
        let definitions = self.state.snapshot.crds(&cluster.id).len();
        let definitions = u32::try_from(definitions).unwrap_or(u32::MAX);
        counts.insert(Page::CustomResources, Counts::total(definitions));
        counts
    }

    fn sidebar_open(
        &self,
        cursor: Option<Page>,
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
            items.extend(section_rows(
                section,
                self.state.location().page,
                cursor,
                &counts,
                palette,
                cx,
            ));
        }
        let collapse = icon_button("collapse-sidebar", CHEVRON_LEFT, COLLAPSE_TOOLTIP, palette)
            .on_click(cx.listener(|this, _, _, cx| this.dispatch(Command::ToggleSidebar, cx)));
        let scope: SharedString = match &self.state.location().cluster {
            Some(id) => flock_core::tabs::cluster_name(id, &self.state.snapshot).into(),
            None => ALL_CLUSTERS.into(),
        };
        let header = div()
            .debug_selector(|| "sidebar-scope".to_owned())
            .flex()
            .flex_none()
            .items_center()
            .h(PAGE_HEADER)
            .pl(SPACE_8)
            .pr(SPACE_3)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(TEXT_UI)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(scope),
            )
            .child(collapse);
        div()
            .w(SIDEBAR)
            .h_full()
            .flex()
            .flex_col()
            .child(top_strip(palette))
            .child(header)
            .child(
                div()
                    .id("sidebar-scroll")
                    .debug_selector(|| "sidebar-nav".to_owned())
                    .key_context(SIDEBAR_CONTEXT)
                    .track_focus(&self.keys.sidebar_focus)
                    .on_key_down(cx.listener(Self::sidebar_key))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(SPACE_4)
                    .pb(SPACE_3)
                    .children(items),
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
    current: Page,
    cursor: Option<Page>,
    counts: &BTreeMap<Page, Counts>,
    palette: &Palette,
    cx: &mut Context<Shell>,
) -> Vec<AnyElement> {
    section
        .pages
        .iter()
        .map(|&page| {
            let selected = page == cursor.unwrap_or(current);
            let selector = format!("nav-{}", page.label().to_lowercase());
            let badges = counts
                .get(&page)
                .map(|c| badges(c, &selector, palette))
                .unwrap_or_default();
            let key = ElementId::NamedInteger("page".into(), page as u64);
            nav_row(key, ROW_NAV, selected, palette)
                .debug_selector(move || selector.clone())
                .when(!selected, |row| row.text_color(palette.fg2))
                .child(div().flex_1().truncate().child(page.label()))
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
    let flagged = counts.badges().into_iter().map(|(status, n)| {
        let selector = format!("{row}-{status:?}").to_lowercase();
        let text = format!("{} {n}", status.glyph());
        badge(selector, text, palette.status(status))
    });
    let total = counts
        .total
        .map(|n| badge(format!("{row}-total"), n.to_string(), palette.mute));
    flagged.chain(total).collect()
}

fn badge(selector: String, text: String, color: Hsla) -> AnyElement {
    div()
        .debug_selector(move || selector)
        .ml(SPACE_4)
        .font_family(FONT_MONO)
        .text_size(TEXT_SMALL)
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

fn icon_button(
    id: &'static str,
    path: &'static str,
    tooltip: &'static str,
    palette: &Palette,
) -> gpui::Stateful<gpui::Div> {
    let hover = palette.hover;
    div()
        .debug_selector(|| id.to_owned())
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .size(ICON_BUTTON)
        .rounded(RADIUS)
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .tooltip(Tooltip::text(tooltip))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(svg().path(path).size(ICON).text_color(palette.mute))
}
