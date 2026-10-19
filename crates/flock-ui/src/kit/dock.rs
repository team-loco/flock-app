use gpui::{
    Animation, AnimationExt, AnyElement, App, Context, ElementId, Entity, InteractiveElement,
    IntoElement, ParentElement, Pixels, RenderOnce, SharedString, StatefulInteractiveElement,
    Styled, Task, Window, div, ease_out_quint, prelude::FluentBuilder,
};

use super::button::IconButton;
use crate::assets::{CHEVRON_DOWN, CHEVRON_UP};
use crate::theme::{
    ActiveTheme, COLLAPSE, DOCK_BAR, DOCK_HEIGHT, FONT_MONO, Palette, RULE, SPACE_3, SPACE_4,
    SPACE_6, TEXT_CONTROL, TEXT_LABEL,
};

const COLLAPSE_TOOLTIP: &str = "Hide dock";
const EXPAND_TOOLTIP: &str = "Show dock";

pub struct DockState {
    active: usize,
    open: bool,
    generation: u64,
    moving: bool,
    settle: Option<Task<()>>,
}

impl Default for DockState {
    fn default() -> Self {
        Self::new()
    }
}

impl DockState {
    pub fn new() -> Self {
        Self {
            active: 0,
            open: true,
            generation: 0,
            moving: false,
            settle: None,
        }
    }

    pub fn active(&self) -> usize {
        self.active
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn select(&mut self, tab: usize, cx: &mut Context<Self>) {
        self.active = tab;
        if !self.open {
            self.toggle(cx);
        }
        cx.notify();
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.open = !self.open;
        self.generation += 1;
        self.moving = true;
        self.settle = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COLLAPSE).await;
            this.update(cx, |state, cx| {
                state.moving = false;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn click_tab(&mut self, tab: usize, cx: &mut Context<Self>) {
        if tab == self.active {
            self.toggle(cx);
        } else {
            self.select(tab, cx);
        }
    }
}

pub struct DockTab {
    label: SharedString,
    detail: Option<SharedString>,
    tools: Option<AnyElement>,
    content: Option<AnyElement>,
}

impl DockTab {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            detail: None,
            tools: None,
            content: None,
        }
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn tools(mut self, tools: impl IntoElement) -> Self {
        self.tools = Some(tools.into_any_element());
        self
    }

    pub fn content(mut self, content: impl IntoElement) -> Self {
        self.content = Some(content.into_any_element());
        self
    }
}

#[derive(IntoElement)]
pub struct Dock {
    state: Entity<DockState>,
    tabs: Vec<DockTab>,
    height: Pixels,
}

impl Dock {
    pub fn new(state: &Entity<DockState>) -> Self {
        Self {
            state: state.clone(),
            tabs: Vec::new(),
            height: DOCK_HEIGHT,
        }
    }

    pub fn tab(mut self, tab: DockTab) -> Self {
        self.tabs.push(tab);
        self
    }

    pub fn height(mut self, height: Pixels) -> Self {
        self.height = height;
        self
    }
}

fn render_tab(
    ix: usize,
    label: SharedString,
    detail: Option<SharedString>,
    joined: bool,
    state: &Entity<DockState>,
    palette: &Palette,
) -> impl IntoElement {
    let state = state.clone();
    let hover = palette.hover;
    div()
        .id(ix)
        .debug_selector(move || format!("dock-tab-{ix}"))
        .flex()
        .items_center()
        .gap(SPACE_3)
        .h_full()
        .px(SPACE_6)
        .border_r(RULE)
        .border_color(palette.line)
        .cursor_pointer()
        .whitespace_nowrap()
        .when(joined, |tab| tab.bg(palette.editor).text_color(palette.fg))
        .when(!joined, |tab| {
            tab.border_b(RULE)
                .text_color(palette.mute)
                .hover(move |style| style.bg(hover))
        })
        .on_click(move |_, _, cx| state.update(cx, |state, cx| state.click_tab(ix, cx)))
        .child(label)
        .when_some(detail, |tab, detail| {
            tab.child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(TEXT_LABEL)
                    .text_color(palette.mute)
                    .child(detail),
            )
        })
}

impl RenderOnce for Dock {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let (active, open, generation) = {
            let state = self.state.read(cx);
            (
                state.active,
                state.open,
                state.moving.then_some(state.generation),
            )
        };
        let mut active_tools = None;
        let mut active_content = None;
        let mut tabs = Vec::with_capacity(self.tabs.len());
        for (ix, tab) in self.tabs.into_iter().enumerate() {
            let selected = ix == active;
            if selected {
                active_tools = tab.tools;
                active_content = tab.content;
            }
            let element = render_tab(
                ix,
                tab.label,
                tab.detail,
                selected && open,
                &self.state,
                &palette,
            );
            tabs.push(element.into_any_element());
        }
        let toggle_state = self.state.clone();
        let (chevron, tooltip) = if open {
            (CHEVRON_DOWN, COLLAPSE_TOOLTIP)
        } else {
            (CHEVRON_UP, EXPAND_TOOLTIP)
        };
        let bar = div()
            .flex()
            .flex_none()
            .h(DOCK_BAR)
            .bg(palette.chrome)
            .text_size(TEXT_CONTROL)
            .children(tabs)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_end()
                    .gap(SPACE_4)
                    .px(SPACE_4)
                    .border_b(RULE)
                    .border_color(palette.line)
                    .when(open, |tools| tools.children(active_tools))
                    .child(IconButton::svg("dock-toggle", chevron, tooltip).on_click(
                        move |_, _, cx| {
                            toggle_state.update(cx, DockState::toggle);
                        },
                    )),
            );
        let height = self.height;
        let dock = div()
            .flex()
            .flex_col()
            .flex_none()
            .w_full()
            .overflow_hidden()
            .border_t(RULE)
            .border_color(palette.line)
            .bg(palette.editor)
            .h(if open { height } else { DOCK_BAR + RULE })
            .child(bar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .when(open, |body| body.children(active_content)),
            );
        let Some(generation) = generation else {
            return dock.into_any_element();
        };
        let (from, to) = if open {
            (DOCK_BAR + RULE, height)
        } else {
            (height, DOCK_BAR + RULE)
        };
        let animation = Animation::new(COLLAPSE).with_easing(ease_out_quint());
        dock.with_animation(
            ElementId::NamedInteger("dock".into(), generation),
            animation,
            move |dock, delta| dock.h(from + (to - from) * delta),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext as _, TestAppContext};

    use super::*;

    #[gpui::test]
    fn the_dock_moves_only_while_its_toggle_plays(cx: &mut TestAppContext) {
        let dock = cx.new(|_| DockState::new());
        assert!(!dock.read_with(cx, |dock, _| dock.moving));
        dock.update(cx, DockState::toggle);
        assert!(dock.read_with(cx, |dock, _| dock.moving));
        cx.executor().advance_clock(COLLAPSE);
        cx.run_until_parked();
        assert!(!dock.read_with(cx, |dock, _| dock.moving));
    }
}
