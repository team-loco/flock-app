use flock_core::palette::{COMPARE_KEY, Hit, LOGS_KEY, OWNER_KEY, Results, Section, search};
use flock_core::{ClusterId, Command, Snapshot};
use gpui::{
    AnchoredPositionMode, Animation, AnimationExt, AnyElement, AppContext, Context, ElementId,
    Entity, EventEmitter, FocusHandle, InteractiveElement, IntoElement, MouseButton, ParentElement,
    Pixels, Render, ScrollHandle, StatefulInteractiveElement, Styled, Subscription, Window,
    anchored, deferred, div, ease_out_quint, point, prelude::FluentBuilder,
};

use crate::actions::{
    CompareAcrossClusters, FollowLogs, OpenSelectedInSplit, OpenSelectedNewWindow, ShowFluxOwner,
};
use crate::kit::list_keys::Nav;
use crate::kit::status_symbols;
use crate::kit::{
    Highlighted, InputEvent, SectionLabel, Step, TextField, TextFieldDown, TextFieldEscape,
    TextFieldPageDown, TextFieldPageUp, TextFieldState, TextFieldUp, shadow, step,
};
use crate::theme::{
    ActiveTheme, FONT_MONO, PALETTE_CLUSTER, PALETTE_IN, PALETTE_INPUT, PALETTE_KIND,
    PALETTE_NAMESPACE, PALETTE_OUT, PALETTE_RESULTS, PALETTE_SCALE_FROM, PALETTE_STATUS,
    PALETTE_TOP, PALETTE_WIDTH, Palette, RADIUS_MENU, ROW_TABLE_MAX, RULE, SPACE_2, SPACE_3,
    SPACE_5, SPACE_6, SPACE_7, SPACE_8, TEXT_BODY, TEXT_LABEL, TEXT_PALETTE, TEXT_SMALL,
};

const PLACEHOLDER: &str = "Go to anything";
const NO_MATCHES: &str = "No matches.";
pub(crate) const PALETTE_CONTEXT: &str = "CommandPalette";
const OVERLAY_PRIORITY: usize = 5;
const HINTS: &[(&str, &str)] = &[
    ("@", "cluster"),
    ("ns:", "namespace"),
    ("k:", "kind"),
    ("l:", "label=value"),
    (">", "command"),
];
const FOOTER_START: &[&str] = &[
    "↑↓ move",
    "↵ open",
    "⌘↵ open in new tab",
    "⌥↵ open in new window",
    "⌘⇧↵ open in split",
];
const FOOTER_END: &str = "esc";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Closed,
    Open,
    Closing,
}

pub enum PaletteEvent {
    Run(Command),
    NewTab(Command),
    NewWindow(Command),
    Split(Command),
}

pub struct CommandPalette {
    input: Entity<TextFieldState>,
    snapshot: Snapshot,
    current: Option<ClusterId>,
    results: Results,
    selected: Option<usize>,
    phase: Phase,
    generation: u64,
    restore: Option<FocusHandle>,
    scroll: ScrollHandle,
    _input_events: Subscription,
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

impl CommandPalette {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextFieldState::new(window, cx).placeholder(PLACEHOLDER));
        let input_events = cx.subscribe(&input, |this, _, event: &InputEvent, cx| match event {
            InputEvent::Change => this.refresh(cx),
            InputEvent::PressEnter {
                secondary: true, ..
            } => this.open_selected_in_tab(cx),
            InputEvent::PressEnter { .. } => this.run_selected(cx),
            _ => {}
        });
        Self {
            input,
            snapshot: Snapshot::default(),
            current: None,
            results: Results::default(),
            selected: None,
            phase: Phase::Closed,
            generation: 0,
            restore: None,
            scroll: ScrollHandle::new(),
            _input_events: input_events,
        }
    }

    pub fn is_open(&self) -> bool {
        self.phase == Phase::Open
    }

    pub fn set_context(
        &mut self,
        snapshot: &Snapshot,
        current: Option<&ClusterId>,
        cx: &mut Context<Self>,
    ) {
        self.snapshot = snapshot.clone();
        self.current = current.cloned();
        if self.phase != Phase::Closed {
            self.refresh(cx);
        }
    }

    pub fn open(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.phase != Phase::Open {
            self.restore = window.focused(cx);
            self.generation += 1;
        }
        self.phase = Phase::Open;
        self.input.update(cx, |input, cx| {
            input.set_value(query.to_owned(), window, cx);
            input.focus(window, cx);
        });
        self.refresh(cx);
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.phase != Phase::Open {
            return;
        }
        self.phase = Phase::Closing;
        self.generation += 1;
        if let Some(previous) = self.restore.take() {
            window.focus(&previous, cx);
        }
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PALETTE_OUT).await;
            this.update(cx, |palette, cx| palette.finish_close(generation, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    fn finish_close(&mut self, generation: u64, cx: &mut Context<Self>) {
        if self.phase == Phase::Closing && self.generation == generation {
            self.phase = Phase::Closed;
            cx.notify();
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value();
        self.results = search(&self.snapshot, self.current.as_ref(), &query);
        self.selected = (!self.results.hits.is_empty()).then_some(0);
        self.scroll.scroll_to_item(0);
        cx.notify();
    }

    fn move_selection(&mut self, direction: Step, cx: &mut Context<Self>) {
        let len = self.results.hits.len();
        self.selected = step(self.selected, len, |_| true, direction);
        if let Some(ix) = self.selected {
            self.scroll
                .scroll_to_item(child_index(&self.results.hits, ix));
        }
        cx.notify();
    }

    fn jump(&mut self, nav: Nav, cx: &mut Context<Self>) {
        self.selected = nav.target(self.selected, self.results.hits.len());
        if let Some(ix) = self.selected {
            self.scroll
                .scroll_to_item(child_index(&self.results.hits, ix));
        }
        cx.notify();
    }

    fn run_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(ix) = self.selected {
            self.run(ix, cx);
        }
    }

    fn emit_selected(&self, event: fn(Command) -> PaletteEvent, cx: &mut Context<Self>) {
        if let Some(hit) = self.selected.and_then(|ix| self.results.hits.get(ix)) {
            cx.emit(event(hit.command.clone()));
        }
        cx.stop_propagation();
    }

    fn open_selected_in_tab(&mut self, cx: &mut Context<Self>) {
        let hit = self.selected.and_then(|ix| self.results.hits.get(ix));
        if let Some(hit) = hit {
            cx.emit(PaletteEvent::NewTab(hit.command.clone()));
        }
    }

    fn run_shortcut(&mut self, key: &str, cx: &mut Context<Self>) {
        let hit = self
            .results
            .hits
            .iter()
            .position(|hit| hit.shortcut == Some(key));
        match hit {
            Some(ix) => self.run(ix, cx),
            None => cx.propagate(),
        }
    }

    fn run(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(hit) = self.results.hits.get(ix) else {
            return;
        };
        let command = hit.command.clone();
        cx.emit(PaletteEvent::Run(command));
    }

    fn render_query(&self, palette: &Palette) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(SPACE_5)
            .h(PALETTE_INPUT)
            .px(SPACE_6)
            .border_b(RULE)
            .border_color(palette.line)
            .child(TextField::bare(&self.input, TEXT_PALETTE))
            .child(
                div()
                    .flex_none()
                    .font_family(FONT_MONO)
                    .text_size(TEXT_SMALL)
                    .text_color(palette.mute)
                    .child(self.results.summary()),
            )
    }

    fn render_results(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let mut children: Vec<AnyElement> = Vec::new();
        let mut section = None;
        for (ix, hit) in self.results.hits.iter().enumerate() {
            if section != Some(hit.section) {
                let first = section.is_none();
                children.push(section_label(hit.section, first).into_any_element());
                section = Some(hit.section);
            }
            children.push(self.render_hit(ix, hit, &palette, cx));
        }
        if children.is_empty() {
            children.push(
                div()
                    .px(SPACE_6)
                    .py(SPACE_3)
                    .text_size(TEXT_BODY)
                    .text_color(palette.mute)
                    .child(NO_MATCHES)
                    .into_any_element(),
            );
        }
        div()
            .id("palette-results")
            .flex()
            .flex_col()
            .py(SPACE_2)
            .max_h(PALETTE_RESULTS)
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(children)
    }

    fn render_hit(
        &self,
        ix: usize,
        hit: &Hit,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.selected == Some(ix);
        let hover = palette.hover;
        let open = self.is_open();
        let row = div()
            .id(ElementId::NamedInteger("palette-hit".into(), ix as u64))
            .debug_selector(move || format!("palette-hit-{ix}"))
            .flex()
            .flex_none()
            .items_center()
            .gap(SPACE_6)
            .h(ROW_TABLE_MAX)
            .px(SPACE_6)
            .border(RULE)
            .border_color(gpui::transparent_black())
            .text_size(TEXT_BODY)
            .cursor_pointer()
            .when(selected, |row| {
                row.bg(palette.sel).border_color(palette.selline)
            })
            .when(!selected, |row| row.hover(move |style| style.bg(hover)))
            .when(open, |row| {
                row.on_click(cx.listener(move |this, _, _, cx| this.run(ix, cx)))
            })
            .child(
                div()
                    .w(PALETTE_KIND)
                    .flex_none()
                    .font_family(FONT_MONO)
                    .text_size(TEXT_LABEL)
                    .text_color(palette.mute)
                    .child(hit.kind.clone()),
            );
        let row = match hit.section {
            Section::Commands => row
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(Highlighted::new(hit.name.clone(), hit.highlights.clone())),
                )
                .children(hit.shortcut.map(|keys| {
                    div()
                        .flex_none()
                        .font_family(FONT_MONO)
                        .text_size(TEXT_LABEL)
                        .text_color(palette.mute)
                        .child(keys)
                })),
            Section::Clusters => entry_columns(row, hit, palette),
            Section::Pages => entry_columns(row, hit, palette),
            Section::Resources => entry_columns(row, hit, palette),
        };
        row.into_any_element()
    }
}

fn entry_columns(
    row: gpui::Stateful<gpui::Div>,
    hit: &Hit,
    palette: &Palette,
) -> gpui::Stateful<gpui::Div> {
    let (status, status_color) = match hit.status {
        Some(status) => (
            format!("{} {}", status.glyph(), hit.detail),
            palette.status(status),
        ),
        None => (hit.detail.clone(), palette.mute),
    };
    row.child(
        div()
            .flex_1()
            .min_w_0()
            .truncate()
            .font_family(FONT_MONO)
            .child(Highlighted::new(hit.name.clone(), hit.highlights.clone())),
    )
    .child(fixed_cell(PALETTE_NAMESPACE, palette.mute).child(hit.namespace.clone()))
    .child(fixed_cell(PALETTE_CLUSTER, palette.fg2).child(hit.cluster.clone()))
    .child(
        fixed_cell(PALETTE_STATUS, status_color)
            .font_family(FONT_MONO)
            .text_size(TEXT_SMALL)
            .child(status_symbols(status)),
    )
}

fn fixed_cell(width: Pixels, color: gpui::Hsla) -> gpui::Div {
    div().w(width).flex_none().truncate().text_color(color)
}

fn section_label(section: Section, first: bool) -> impl IntoElement {
    div()
        .flex_none()
        .px(SPACE_6)
        .pt(if first { SPACE_3 } else { SPACE_5 })
        .pb(SPACE_2)
        .child(SectionLabel::new(section.label()))
}

fn hints(palette: &Palette) -> impl IntoElement {
    let hints = HINTS.iter().map(|(prefix, rest)| {
        div()
            .flex()
            .child(div().text_color(palette.fg2).child(*prefix))
            .child(*rest)
    });
    div()
        .flex()
        .flex_wrap()
        .gap(SPACE_7)
        .px(SPACE_6)
        .py(SPACE_3)
        .border_b(RULE)
        .border_color(palette.line)
        .font_family(FONT_MONO)
        .text_size(TEXT_SMALL)
        .text_color(palette.mute)
        .children(hints)
}

fn footer(palette: &Palette) -> impl IntoElement {
    div()
        .flex()
        .gap(SPACE_8)
        .px(SPACE_6)
        .py(SPACE_3)
        .border_t(RULE)
        .border_color(palette.line)
        .bg(palette.chrome)
        .font_family(FONT_MONO)
        .text_size(TEXT_LABEL)
        .text_color(palette.mute)
        .children(FOOTER_START.iter().map(|hint| div().child(*hint)))
        .child(div().flex_1())
        .child(FOOTER_END)
}

pub fn child_index(hits: &[Hit], ix: usize) -> usize {
    let labels_before = hits
        .iter()
        .take(ix + 1)
        .enumerate()
        .filter(|(at, hit)| {
            at.checked_sub(1)
                .and_then(|before| hits.get(before))
                .is_none_or(|previous| previous.section != hit.section)
        })
        .count();
    ix + labels_before
}

fn reveal(t: f32, phase: Phase) -> f32 {
    match phase {
        Phase::Open => t,
        Phase::Closing => 1. - t,
        Phase::Closed => 0.,
    }
}

impl Render for CommandPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.phase == Phase::Closed {
            return div().into_any_element();
        }
        let palette = *cx.palette();
        let viewport = window.viewport_size();
        let phase = self.phase;
        let duration = if phase == Phase::Open {
            PALETTE_IN
        } else {
            PALETTE_OUT
        };
        let panel = div()
            .id("command-palette")
            .key_context(PALETTE_CONTEXT)
            .capture_action(cx.listener(|this, _: &OpenSelectedNewWindow, _, cx| {
                this.emit_selected(PaletteEvent::NewWindow, cx);
            }))
            .capture_action(cx.listener(|this, _: &OpenSelectedInSplit, _, cx| {
                this.emit_selected(PaletteEvent::Split, cx);
            }))
            .flex()
            .flex_col()
            .w(PALETTE_WIDTH)
            .overflow_hidden()
            .bg(palette.panel)
            .border(RULE)
            .border_color(palette.line2)
            .rounded(RADIUS_MENU)
            .shadow(shadow(&palette))
            .capture_action(cx.listener(|this, _: &TextFieldUp, _, cx| {
                this.move_selection(Step::Previous, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &TextFieldDown, _, cx| {
                this.move_selection(Step::Next, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &TextFieldPageUp, _, cx| {
                this.jump(Nav::PageUp, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &TextFieldPageDown, _, cx| {
                this.jump(Nav::PageDown, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|this, _: &TextFieldEscape, window, cx| {
                this.close(window, cx);
            }))
            .on_action(cx.listener(|this, _: &FollowLogs, _, cx| {
                this.run_shortcut(LOGS_KEY, cx);
            }))
            .on_action(cx.listener(|this, _: &CompareAcrossClusters, _, cx| {
                this.run_shortcut(COMPARE_KEY, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowFluxOwner, _, cx| {
                this.run_shortcut(OWNER_KEY, cx);
            }))
            .when(phase == Phase::Open, |panel| {
                panel.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            })
            .child(self.render_query(&palette))
            .child(hints(&palette))
            .child(self.render_results(cx))
            .child(footer(&palette));
        let key = ElementId::NamedInteger("command-palette-reveal".into(), self.generation);
        let animation = Animation::new(duration).with_easing(ease_out_quint());
        let panel = panel.with_animation(key, animation, move |panel, t| {
            let shown = reveal(t, phase);
            let scale = PALETTE_SCALE_FROM + (1. - PALETTE_SCALE_FROM) * shown;
            panel.opacity(shown).w(PALETTE_WIDTH * scale)
        });
        let scrim_key = ElementId::NamedInteger("command-palette-scrim".into(), self.generation);
        let scrim_animation = Animation::new(duration).with_easing(ease_out_quint());
        let scrim_color = palette.scrim;
        let scrim = div()
            .id("command-palette-scrim")
            .w(viewport.width)
            .h(viewport.height)
            .flex()
            .justify_center()
            .items_start()
            .pt(PALETTE_TOP)
            .when(phase == Phase::Open, |scrim| {
                scrim.occlude().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close(window, cx)),
                )
            })
            .child(panel)
            .with_animation(scrim_key, scrim_animation, move |scrim, t| {
                scrim.bg(scrim_color.opacity(reveal(t, phase)))
            });
        deferred(
            anchored()
                .position_mode(AnchoredPositionMode::Window)
                .position(point(Pixels::ZERO, Pixels::ZERO))
                .child(scrim),
        )
        .priority(OVERLAY_PRIORITY)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(section: Section) -> Hit {
        Hit {
            section,
            kind: String::new(),
            name: String::new(),
            highlights: Vec::new(),
            namespace: String::new(),
            cluster: String::new(),
            status: None,
            detail: String::new(),
            shortcut: None,
            command: Command::ToggleSidebar,
        }
    }

    #[test]
    fn child_index_skips_section_labels() {
        let hits = [
            hit(Section::Resources),
            hit(Section::Resources),
            hit(Section::Commands),
            hit(Section::Commands),
        ];
        let children: Vec<_> = (0..hits.len()).map(|ix| child_index(&hits, ix)).collect();
        assert_eq!(children, [1, 2, 4, 5]);
    }

    #[test]
    fn closing_runs_the_reveal_backwards() {
        assert!((reveal(0.25, Phase::Open) - 0.25).abs() < f32::EPSILON);
        assert!((reveal(0.25, Phase::Closing) - 0.75).abs() < f32::EPSILON);
        assert!(reveal(0.5, Phase::Closed).abs() < f32::EPSILON);
    }
}
