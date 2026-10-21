use std::collections::BTreeMap;

use flock_core::Status;
use gpui::{
    AnyElement, App, CursorStyle, DispatchPhase, ElementId, FontWeight, Global, InteractiveElement,
    IntoElement, MouseButton, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Window, canvas, div, prelude::FluentBuilder,
};

use super::status::StatusText;
use super::text::SectionLabel;
use crate::theme::{
    ActiveTheme, FONT_MONO, INSPECTOR, INSPECTOR_MAX, INSPECTOR_MIN, RULE, SPACE_3, SPACE_4,
    SPACE_5, SPACE_6, SPLITTER, TEXT_BODY, TEXT_UI,
};

#[derive(IntoElement)]
pub struct Inspector {
    id: ElementId,
    title: SharedString,
    mono: bool,
    status: Option<(Status, SharedString)>,
    condition: Option<AnyElement>,
    actions: Option<AnyElement>,
    width: Pixels,
    sections: Vec<(Option<SharedString>, AnyElement)>,
}

impl Inspector {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            mono: false,
            status: None,
            condition: None,
            actions: None,
            width: INSPECTOR,
            sections: Vec::new(),
        }
    }

    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    pub fn status(mut self, status: Status, label: impl Into<SharedString>) -> Self {
        self.status = Some((status, label.into()));
        self
    }

    pub fn condition(mut self, condition: Option<impl IntoElement>) -> Self {
        self.condition = condition.map(IntoElement::into_any_element);
        self
    }

    pub fn actions(mut self, actions: impl IntoElement) -> Self {
        self.actions = Some(actions.into_any_element());
        self
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    pub fn section(mut self, label: Option<&str>, content: impl IntoElement) -> Self {
        let label = label.map(|l| SharedString::from(l.to_owned()));
        self.sections.push((label, content.into_any_element()));
        self
    }
}

impl RenderOnce for Inspector {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = cx.palette();
        let line = palette.line;
        let title = div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_size(TEXT_UI)
            .font_weight(FontWeight::SEMIBOLD)
            .when(self.mono, |title| title.font_family(FONT_MONO))
            .child(super::selectable::SelectableText::new(self.title));
        let header = div()
            .flex()
            .flex_col()
            .gap(SPACE_3)
            .p(SPACE_6)
            .border_b(RULE)
            .border_color(line)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_4)
                    .child(title)
                    .children(self.actions),
            )
            .when_some(self.status, |header, (status, label)| {
                header.child(
                    div()
                        .text_size(TEXT_BODY)
                        .child(StatusText::new(status, label)),
                )
            })
            .children(self.condition);
        let sections = self.sections.into_iter().map(|(label, content)| {
            div()
                .flex()
                .flex_col()
                .gap(SPACE_5)
                .p(SPACE_6)
                .border_b(RULE)
                .border_color(line)
                .when_some(label, |section, label| {
                    section.child(SectionLabel::new(label))
                })
                .child(content)
        });
        let body = div()
            .id(self.id.clone())
            .w_full()
            .h_full()
            .overflow_y_scroll()
            .border_l(RULE)
            .border_color(line)
            .bg(palette.panel)
            .text_size(TEXT_BODY)
            .child(header)
            .children(sections);
        InspectorPane::new(self.id.to_string(), body).width(self.width)
    }
}

#[derive(Default)]
struct InspectorWidths(BTreeMap<String, u16>);

impl Global for InspectorWidths {}

pub fn clamp_inspector_width(width: Pixels) -> Pixels {
    if !f32::from(width).is_finite() {
        return INSPECTOR;
    }
    width.clamp(INSPECTOR_MIN, INSPECTOR_MAX)
}

pub(crate) fn stored_width(kind: &str, fallback: Pixels, cx: &App) -> Pixels {
    let pending = cx
        .try_global::<InspectorWidths>()
        .and_then(|widths| widths.0.get(kind));
    let saved = crate::settings::current(cx).inspector_widths.get(kind);
    let width = pending.or(saved).map_or(fallback, |width| {
        let width = f32::from(*width);
        gpui::px(width)
    });
    clamp_inspector_width(width)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn save_width(kind: &str, width: Pixels, persist: bool, cx: &mut App) {
    let clamped = clamp_inspector_width(width);
    let width = f32::from(clamped).round() as u16;
    if persist {
        cx.default_global::<InspectorWidths>().0.remove(kind);
        let saved = crate::settings::update(cx, |settings| {
            settings.inspector_widths.insert(kind.to_owned(), width);
        });
        if let Err(error) = saved {
            let notice = super::overlay::Toast::new(Status::Err, "Could not save inspector width")
                .detail(error);
            super::overlay::toast(notice, cx);
        }
    } else {
        cx.default_global::<InspectorWidths>()
            .0
            .insert(kind.to_owned(), width);
    }
}

const HANDLE_GROUP: &str = "inspector-resize-handle";

#[derive(Default)]
struct ResizeState {
    drag: Option<(Pixels, Pixels)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handle {
    Leading,
    Trailing,
}

impl Handle {
    fn dragged(self, initial: Pixels, start: Pixels, now: Pixels) -> Pixels {
        match self {
            Self::Leading => initial + start - now,
            Self::Trailing => initial + now - start,
        }
    }
}

fn divider(dragging: bool, cx: &App) -> gpui::Div {
    let palette = cx.palette();
    let accent = palette.acc;
    div()
        .w(RULE)
        .h_full()
        .bg(if dragging { accent } else { palette.sep })
        .group_hover(HANDLE_GROUP, move |style| style.bg(accent))
}

#[derive(IntoElement)]
pub struct InspectorPane {
    kind: SharedString,
    width: Pixels,
    handle: Handle,
    content: AnyElement,
}

impl InspectorPane {
    pub fn new(kind: impl Into<SharedString>, content: impl IntoElement) -> Self {
        Self {
            kind: kind.into(),
            width: INSPECTOR,
            handle: Handle::Leading,
            content: content.into_any_element(),
        }
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = clamp_inspector_width(width);
        self
    }

    pub fn trailing_handle(mut self) -> Self {
        self.handle = Handle::Trailing;
        self
    }
}

impl RenderOnce for InspectorPane {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let width = stored_width(&self.kind, self.width, cx);
        let handle = self.handle;
        let state = window.use_keyed_state(
            (ElementId::Name(self.kind.clone()), "resize"),
            cx,
            |_, _| ResizeState::default(),
        );
        let moving = state.clone();
        let releasing = state.clone();
        let move_kind = self.kind.clone();
        let release_kind = self.kind.clone();
        let recorder = canvas(
            |_, _, _| (),
            move |_, (), window, _| {
                let moving = moving.clone();
                let kind = move_kind.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    let Some((start, initial)) = moving.read(cx).drag else {
                        return;
                    };
                    if event.dragging() {
                        let dragged = handle.dragged(initial, start, event.position.x);
                        save_width(&kind, dragged, false, cx);
                        window.refresh();
                    } else {
                        moving.update(cx, |state, _| state.drag = None);
                        let width = stored_width(&kind, initial, cx);
                        save_width(&kind, width, true, cx);
                    }
                });
                let releasing = releasing.clone();
                let kind = release_kind.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || event.button != MouseButton::Left {
                        return;
                    }
                    let drag = releasing.update(cx, |state, _| state.drag.take());
                    if let Some((start, initial)) = drag {
                        let dragged = handle.dragged(initial, start, event.position.x);
                        save_width(&kind, dragged, true, cx);
                        window.refresh();
                    }
                });
            },
        )
        .absolute()
        .size_full();
        let divider = divider(state.read(cx).drag.is_some(), cx);
        let reset_kind = self.kind.clone();
        let fallback = self.width;
        let selector = format!("{}-resize", self.kind);
        let edge = div()
            .id("inspector-resize")
            .debug_selector(move || selector.clone())
            .absolute()
            .map(|edge| match handle {
                Handle::Leading => edge.left_0(),
                Handle::Trailing => edge.right_0(),
            })
            .top_0()
            .bottom_0()
            .w(SPLITTER)
            .group(HANDLE_GROUP)
            .flex()
            .map(|edge| match handle {
                Handle::Leading => edge.justify_start(),
                Handle::Trailing => edge.justify_end(),
            })
            .child(divider)
            .cursor(CursorStyle::ResizeLeftRight)
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                cx.stop_propagation();
                if event.click_count == 2 {
                    save_width(&reset_kind, fallback, true, cx);
                    window.refresh();
                } else {
                    state.update(cx, |state, _| state.drag = Some((event.position.x, width)));
                }
            });
        let selector = format!("{}-pane", self.kind);
        div()
            .id(ElementId::Name(self.kind))
            .debug_selector(move || selector.clone())
            .relative()
            .w(width)
            .flex_none()
            .h_full()
            .min_w_0()
            .child(recorder)
            .child(self.content)
            .child(edge)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, px};

    #[test]
    fn widths_are_clamped_and_non_finite_values_reset() {
        assert_eq!(clamp_inspector_width(px(1.)), INSPECTOR_MIN);
        assert_eq!(clamp_inspector_width(px(10000.)), INSPECTOR_MAX);
        assert_eq!(clamp_inspector_width(px(f32::NAN)), INSPECTOR);
        assert_eq!(clamp_inspector_width(px(480.)), px(480.));
    }

    #[test]
    fn dragging_widens_the_pane_away_from_its_handle() {
        let initial = px(360.);
        assert_eq!(
            Handle::Leading.dragged(initial, px(500.), px(450.)),
            px(410.)
        );
        assert_eq!(
            Handle::Trailing.dragged(initial, px(500.), px(550.)),
            px(410.)
        );
    }

    #[gpui::test]
    fn preferences_restore_per_kind_and_only_persist_on_release(cx: &mut TestAppContext) {
        use flock_core::AppSettings;
        use std::cell::RefCell;
        use std::rc::Rc;
        let saved = Rc::new(RefCell::new(Vec::new()));
        let log = saved.clone();
        cx.update(|cx| {
            crate::init(None, cx).unwrap_or_else(|error| panic!("UI init: {error}"));
            let persist: crate::settings::Saver = Rc::new(move |settings| {
                log.borrow_mut().push(settings.clone());
                Ok(())
            });
            crate::settings::install(AppSettings::default(), persist.clone(), false, cx);
            save_width("node", px(600.), false, cx);
            assert!(saved.borrow().is_empty());
            save_width("node", px(600.), true, cx);
            assert_eq!(saved.borrow().len(), 1);
            let settings = saved.borrow()[0].clone();
            cx.set_global(InspectorWidths::default());
            crate::settings::install(settings, persist, false, cx);
            assert_eq!(stored_width("node", INSPECTOR, cx), px(600.));
            assert_eq!(stored_width("pod", INSPECTOR, cx), INSPECTOR);
        });
    }
}
