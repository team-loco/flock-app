use gpui::{
    App, AppContext, Context, ElementId, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Pixels, Render,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div,
    prelude::FluentBuilder, svg,
};

use super::popover::{self, Dismissal, Step};
use super::{InputEvent, TextField, TextFieldDown, TextFieldEscape, TextFieldState, TextFieldUp};
use crate::assets::CHEVRON_DOWN;
use crate::theme::{
    ActiveTheme, CONTROL, CONTROL_COMPACT, FONT_MONO, ICON, MENU_MAX_HEIGHT, MENU_WIDTH, Palette,
    RADIUS, ROW_TABLE, RULE, SPACE_4, SPACE_5, TEXT_CONTROL, TEXT_SMALL,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectEvent {
    Changed(usize),
}

pub struct Select {
    id: SharedString,
    options: Vec<SharedString>,
    details: Vec<SharedString>,
    selected: usize,
    placeholder: SharedString,
    width: Pixels,
    mono: bool,
    compact: bool,
    open: bool,
    highlighted: Option<usize>,
    generation: u64,
    dismissal: Dismissal,
    focus: FocusHandle,
    restore: Option<FocusHandle>,
    editor: Option<Entity<TextFieldState>>,
    editor_events: Option<Subscription>,
}

impl EventEmitter<SelectEvent> for Select {}

impl Select {
    pub fn new(
        id: impl Into<SharedString>,
        options: Vec<SharedString>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            id: id.into(),
            options,
            details: Vec::new(),
            selected: 0,
            placeholder: SharedString::default(),
            width: MENU_WIDTH,
            mono: false,
            compact: false,
            open: false,
            highlighted: None,
            generation: 0,
            dismissal: Dismissal::default(),
            focus: cx.focus_handle(),
            restore: None,
            editor: None,
            editor_events: None,
        }
    }

    pub fn editable(mut self, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor =
            cx.new(|cx| TextFieldState::new(window, cx).placeholder("Filter or enter a value"));
        let subscription = cx.subscribe_in(
            &editor,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.highlighted = this.visible_options(cx).first().copied();
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.accept(window, cx),
                InputEvent::Blur => {}
                InputEvent::Focus => {}
            },
        );
        self.editor_events = Some(subscription);
        self.editor = Some(editor);
        self
    }

    pub fn set_value(&mut self, value: SharedString, cx: &mut Context<Self>) {
        let index = self
            .options
            .iter()
            .position(|option| option == &value)
            .unwrap_or_else(|| {
                self.options.push(value);
                self.options.len().saturating_sub(1)
            });
        self.set_selected(index, cx);
    }

    fn visible_options(&self, cx: &App) -> Vec<usize> {
        let query = self
            .editor
            .as_ref()
            .map(|editor| editor.read(cx).value().to_lowercase())
            .unwrap_or_default();
        let mut visible: Vec<usize> = self
            .options
            .iter()
            .enumerate()
            .filter_map(|(ix, option)| option.to_lowercase().contains(&query).then_some(ix))
            .collect();
        if self.custom_value(cx).is_some() {
            visible.push(self.options.len());
        }
        visible
    }

    fn custom_value(&self, cx: &App) -> Option<SharedString> {
        let editor = self.editor.as_ref()?;
        let value = editor.read(cx).value();
        let value = value.trim();
        let exists = self.options.iter().any(|option| option.as_ref() == value);
        (!value.is_empty() && !exists).then(|| value.to_owned().into())
    }

    fn move_highlight(&mut self, step: Step, cx: &mut Context<Self>) {
        let options = self.visible_options(cx);
        let current = self
            .highlighted
            .and_then(|ix| options.iter().position(|option| *option == ix));
        self.highlighted = popover::step(current, options.len(), |_| true, step)
            .and_then(|ix| options.get(ix).copied());
        cx.notify();
    }

    fn accept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.highlighted {
            self.choose(ix, window, cx);
            return;
        }
        if let Some(value) = self.custom_value(cx) {
            self.choose_value(value, window, cx);
        }
    }

    fn choose_value(&mut self, value: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let previous = self.selected_label().cloned();
        self.set_value(value, cx);
        self.close(window, cx);
        if self.selected_label() != previous.as_ref() {
            cx.emit(SelectEvent::Changed(self.selected));
        }
    }

    pub fn selected(mut self, index: usize) -> Self {
        self.selected = index;
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected_label(&self) -> Option<&SharedString> {
        self.options.get(self.selected)
    }

    pub fn set_selected(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected = index.min(self.options.len().saturating_sub(1));
        cx.notify();
    }

    pub fn set_options(&mut self, options: Vec<SharedString>, cx: &mut Context<Self>) {
        let highlighted = self.highlighted.and_then(|ix| {
            self.options
                .get(ix)
                .cloned()
                .or_else(|| self.custom_value(cx))
        });
        self.options = options;
        self.selected = self.selected.min(self.options.len().saturating_sub(1));
        if self.open {
            self.highlighted = highlighted
                .and_then(|label| {
                    self.options
                        .iter()
                        .position(|option| option == &label)
                        .or_else(|| {
                            let custom = self.custom_value(cx);
                            (custom.as_ref() == Some(&label)).then_some(self.options.len())
                        })
                })
                .or_else(|| self.visible_options(cx).first().copied());
        }
        cx.notify();
    }

    pub fn set_details(&mut self, details: Vec<SharedString>, cx: &mut Context<Self>) {
        self.details = details;
        cx.notify();
    }

    fn detail(&self, ix: usize) -> Option<SharedString> {
        self.details
            .get(ix)
            .filter(|detail| !detail.is_empty())
            .cloned()
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.close(window, cx);
            return;
        }
        self.open = true;
        self.generation += 1;
        self.highlighted = Some(self.selected);
        self.restore = window.focused(cx);
        if let Some(editor) = &self.editor {
            editor.update(cx, |editor, cx| {
                editor.set_value("", window, cx);
                let focus = editor.focus_handle(cx);
                window.focus(&focus, cx);
            });
        } else {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        if let Some(previous) = self.restore.take() {
            window.focus(&previous, cx);
        }
        cx.notify();
    }

    fn choose(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix == self.options.len() {
            if let Some(value) = self.custom_value(cx) {
                self.choose_value(value, window, cx);
            }
            return;
        }
        self.close(window, cx);
        if ix < self.options.len() && ix != self.selected {
            self.selected = ix;
            cx.emit(SelectEvent::Changed(ix));
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "down" => self.move_highlight(Step::Next, cx),
            "up" => self.move_highlight(Step::Previous, cx),
            "enter" => self.accept(window, cx),
            "escape" => self.close(window, cx),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Focusable for Select {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Select {
    fn height(&self) -> Pixels {
        if self.compact {
            CONTROL_COMPACT
        } else {
            CONTROL
        }
    }

    fn trigger(&self, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let label = self
            .selected_label()
            .filter(|label| !label.is_empty())
            .cloned()
            .unwrap_or_else(|| self.placeholder.clone());
        let hover = palette.hover;
        let height = self.height();
        let chevron = (!self.compact).then(|| {
            svg()
                .path(CHEVRON_DOWN)
                .size(ICON)
                .flex_none()
                .text_color(palette.mute)
        });
        div()
            .debug_selector(|| self.id.to_string())
            .id(ElementId::Name(self.id.clone()))
            .flex()
            .items_center()
            .gap(SPACE_4)
            .h(height)
            .px(SPACE_4)
            .rounded(RADIUS)
            .border(RULE)
            .border_color(palette.line2)
            .text_size(if self.mono { TEXT_SMALL } else { TEXT_CONTROL })
            .text_color(if self.compact {
                palette.fg2
            } else {
                palette.fg
            })
            .whitespace_nowrap()
            .cursor_pointer()
            .when(self.open, |trigger| trigger.bg(palette.sel))
            .when(!self.open, |trigger| {
                trigger.hover(move |style| style.bg(hover))
            })
            .child(div().truncate().child(label))
            .children(detail(self.detail(self.selected), palette))
            .children(chevron)
            .on_click(cx.listener(|this, _, window, cx| {
                if this.dismissal.take_suppressed() {
                    return;
                }
                this.toggle(window, cx);
            }))
    }
}

fn detail(text: Option<SharedString>, palette: &Palette) -> Option<gpui::Div> {
    text.map(|text| {
        div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_color(palette.mute)
            .child(text)
    })
}

impl Render for Select {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let height = self.height();
        let trigger = self.trigger(&palette, cx).into_any_element();
        let popover = self.open.then(|| {
            let rows: Vec<_> = self
                .visible_options(cx)
                .into_iter()
                .map(|ix| {
                    let custom = self.custom_value(cx);
                    let option = self.options.get(ix).cloned().unwrap_or_else(|| {
                        format!("Use \"{}\"", custom.unwrap_or_default()).into()
                    });
                    let option = if option.is_empty() {
                        self.placeholder.clone()
                    } else {
                        option
                    };
                    let selected = ix == self.selected;
                    let highlighted = self.highlighted == Some(ix);
                    div()
                        .debug_selector(|| format!("{}-option-{ix}", self.id))
                        .id(ix)
                        .flex()
                        .flex_none()
                        .items_center()
                        .h(ROW_TABLE)
                        .px(SPACE_4)
                        .rounded(RADIUS)
                        .cursor_pointer()
                        .when(highlighted && !selected, |row| row.bg(palette.hover))
                        .when(selected, |row| {
                            row.bg(palette.sel).text_color(palette.selfg)
                        })
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered {
                                this.highlighted = Some(ix);
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.choose(ix, window, cx);
                        }))
                        .gap(SPACE_4)
                        .child(div().flex_none().child(option.clone()))
                        .children(detail(self.detail(ix), &palette))
                        .into_any_element()
                })
                .collect();
            let surface = popover::surface("options", &palette)
                .w(self.width)
                .max_h(MENU_MAX_HEIGHT)
                .overflow_y_scroll()
                .track_focus(&self.focus)
                .on_key_down(cx.listener(Self::key_down))
                .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                    this.dismissal.clicked_outside();
                    this.close(window, cx);
                }))
                .capture_action(cx.listener(|this, _: &TextFieldUp, _, cx| {
                    this.move_highlight(Step::Previous, cx);
                    cx.stop_propagation();
                }))
                .capture_action(cx.listener(|this, _: &TextFieldDown, _, cx| {
                    this.move_highlight(Step::Next, cx);
                    cx.stop_propagation();
                }))
                .on_action(cx.listener(|this, _: &TextFieldEscape, window, cx| {
                    this.close(window, cx);
                    cx.stop_propagation();
                }))
                .children(self.editor.as_ref().map(|editor| {
                    div()
                        .debug_selector(|| format!("{}-filter", self.id))
                        .p(SPACE_4)
                        .child(TextField::new(editor))
                }))
                .children(rows);
            let key = ElementId::NamedInteger(self.id.clone(), self.generation);
            popover::below_left(height, popover::appear(surface, key))
        });
        div()
            .id(ElementId::NamedInteger(self.id.clone(), 0))
            .relative()
            .flex_none()
            .gap(SPACE_5)
            .when(self.mono, |select| select.font_family(FONT_MONO))
            .on_hover(cx.listener(|this, hovered: &bool, _, _| {
                this.dismissal.hover(*hovered);
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(trigger)
            .children(popover)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn option_refresh_keeps_the_keyboard_position_by_value(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            crate::init(Some(crate::theme::Appearance::Dark), cx)
                .unwrap_or_else(|error| panic!("UI init: {error}"));
        });
        let window = cx.add_window(|window, cx| {
            Select::new("picker", vec!["a".into(), "b".into()], cx).editable(window, cx)
        });
        window
            .update(cx, |select, window, cx| {
                select.open = true;
                select.highlighted = Some(1);
                select.set_options(vec!["a".into(), "b".into()], cx);
                assert_eq!(select.highlighted, Some(1));
                select.set_options(vec!["b".into(), "a".into()], cx);
                assert_eq!(select.highlighted, Some(0));
                let editor = select.editor.clone().unwrap_or_else(|| panic!("editor"));
                editor.update(cx, |editor, cx| editor.set_value("future", window, cx));
                select.highlighted = Some(select.options.len());
                select.set_options(vec!["a".into(), "future".into()], cx);
                assert_eq!(select.highlighted, Some(1));
            })
            .unwrap_or_else(|error| panic!("test window: {error}"));
    }
}
