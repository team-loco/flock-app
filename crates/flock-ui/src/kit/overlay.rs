use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use flock_core::Status;
use gpui::{
    Anchor, AnchoredPositionMode, Animation, AnimationExt, AnyElement, App, AppContext, Context,
    ElementId, Entity, FocusHandle, Focusable, FontWeight, Global, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window, WindowId, anchored, deferred, div, ease_out_quint, point,
    prelude::FluentBuilder, relative,
};

use super::button::{Button, IconButton};
use super::input::{TextField, TextFieldState};
use super::message::RawMessage;
use super::popover;
use super::status::StatusGlyph;
use crate::theme::{
    ActiveTheme, DIALOG_TOP, DIALOG_WIDTH, FONT_MONO, LINE_PROSE, PAGE_HEADER, POPOVER_IN, Palette,
    RADIUS_MENU, RULE, SPACE_2, SPACE_4, SPACE_5, SPACE_6, SPACE_7, SPACE_8, STATUS_BAR, TEXT_BODY,
    TEXT_SMALL, TEXT_TITLE, TOAST_PROGRESS_TIMEOUT, TOAST_SUCCESS_TIMEOUT, TOAST_WARNING_TIMEOUT,
    TOAST_WIDTH,
};

const CANCEL: &str = "Cancel";
const CLOSE: &str = "×";
const CLOSE_TOOLTIP: &str = "Close";
const CONFIRM_KEYS: &str = "⌘↵";
const OVERLAY_PRIORITY: usize = 10;

pub type ConfirmHandler = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone)]
pub struct ConfirmDialog {
    title: SharedString,
    body: SharedString,
    details: Vec<SharedString>,
    field: Option<Entity<TextFieldState>>,
    primary: bool,
    confirm: SharedString,
    on_confirm: ConfirmHandler,
}

impl ConfirmDialog {
    pub fn new(
        title: impl Into<SharedString>,
        body: impl Into<SharedString>,
        confirm: impl Into<SharedString>,
        on_confirm: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            details: Vec::new(),
            field: None,
            primary: false,
            confirm: confirm.into(),
            on_confirm: Rc::new(on_confirm),
        }
    }

    pub fn detail(mut self, line: impl Into<SharedString>) -> Self {
        self.details.push(line.into());
        self
    }

    pub fn field(mut self, state: Entity<TextFieldState>) -> Self {
        self.field = Some(state);
        self
    }

    pub fn primary(mut self) -> Self {
        self.primary = true;
        self
    }
}

#[derive(Clone)]
struct ToastAction {
    label: SharedString,
    on_click: ConfirmHandler,
}

#[derive(Clone)]
pub struct Toast {
    key: Option<SharedString>,
    status: Status,
    title: SharedString,
    detail: Option<SharedString>,
    actions: Vec<ToastAction>,
}

impl Toast {
    pub fn new(status: Status, title: impl Into<SharedString>) -> Self {
        Self {
            key: None,
            status,
            title: title.into(),
            detail: None,
            actions: Vec::new(),
        }
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn key(mut self, key: impl Into<SharedString>) -> Self {
        self.key = Some(key.into());
        self
    }

    pub fn action(
        mut self,
        label: impl Into<SharedString>,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        let action = ToastAction {
            label: label.into(),
            on_click: Rc::new(on_click),
        };
        self.actions.push(action);
        self
    }

    fn lifetime(&self) -> Option<Duration> {
        match self.status {
            Status::Err => None,
            Status::Warn => Some(TOAST_WARNING_TIMEOUT),
            Status::Progress => Some(TOAST_PROGRESS_TIMEOUT),
            Status::Ok => Some(TOAST_SUCCESS_TIMEOUT),
            Status::Suspended => Some(TOAST_SUCCESS_TIMEOUT),
            Status::Offline => Some(TOAST_SUCCESS_TIMEOUT),
        }
    }
}

struct Shown {
    id: u64,
    revision: u64,
    toast: Toast,
}

struct WindowDialog {
    id: u64,
    dialog: ConfirmDialog,
    focus: FocusHandle,
    restore: Option<FocusHandle>,
}

pub struct Overlays {
    dialogs: BTreeMap<WindowId, WindowDialog>,
    toasts: Vec<Shown>,
    next: u64,
}

struct OverlayHost {
    overlays: Entity<Overlays>,
    _closed: Subscription,
}

impl Global for OverlayHost {}

pub fn init(cx: &mut App) {
    let overlays = cx.new(|_| Overlays {
        dialogs: BTreeMap::new(),
        toasts: Vec::new(),
        next: 0,
    });
    let on_close = overlays.downgrade();
    let closed = cx.on_window_closed(move |cx, window| {
        if let Some(overlays) = on_close.upgrade() {
            overlays.update(cx, |overlays, cx| {
                overlays.dialogs.remove(&window);
                cx.notify();
            });
        }
    });
    cx.set_global(OverlayHost {
        overlays,
        _closed: closed,
    });
}

pub fn layer(cx: &App) -> Entity<Overlays> {
    cx.global::<OverlayHost>().overlays.clone()
}

pub fn confirm(dialog: ConfirmDialog, window: &mut Window, cx: &mut App) {
    layer(cx).update(cx, |overlays, cx| overlays.open(dialog, window, cx));
}

pub fn toast(toast: Toast, cx: &mut App) {
    layer(cx).update(cx, |overlays, cx| overlays.push(toast, cx));
}

pub fn toast_titles(cx: &App) -> Vec<SharedString> {
    let overlays = layer(cx);
    let shown = &overlays.read(cx).toasts;
    shown
        .iter()
        .map(|shown| shown.toast.title.clone())
        .collect()
}

fn toast_detail(toast: &Toast, palette: &Palette) -> Option<AnyElement> {
    let detail = toast.detail.clone()?;
    if toast.status == Status::Err {
        return Some(RawMessage::new(detail).into_any_element());
    }
    let note = div()
        .text_size(TEXT_SMALL)
        .text_color(palette.fg2)
        .child(detail);
    Some(note.into_any_element())
}

pub fn toast_details(cx: &App) -> Vec<Option<SharedString>> {
    let overlays = layer(cx);
    let shown = &overlays.read(cx).toasts;
    shown
        .iter()
        .map(|shown| shown.toast.detail.clone())
        .collect()
}

pub fn toast_statuses(cx: &App) -> Vec<Status> {
    let overlays = layer(cx);
    let shown = &overlays.read(cx).toasts;
    shown.iter().map(|shown| shown.toast.status).collect()
}

impl Overlays {
    fn open(&mut self, dialog: ConfirmDialog, window: &mut Window, cx: &mut Context<Self>) {
        self.next += 1;
        let window_id = window.window_handle().window_id();
        let restore = self
            .dialogs
            .get(&window_id)
            .map_or_else(|| window.focused(cx), |shown| shown.restore.clone());
        let focus = dialog.field.as_ref().map_or_else(
            || cx.focus_handle(),
            |field| field.read(cx).focus_handle(cx),
        );
        window.focus(&focus, cx);
        self.dialogs.insert(
            window_id,
            WindowDialog {
                id: self.next,
                dialog,
                focus,
                restore,
            },
        );
        cx.notify();
    }

    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let window_id = window.window_handle().window_id();
        if let Some(shown) = self.dialogs.remove(&window_id)
            && let Some(previous) = shown.restore
        {
            window.focus(&previous, cx);
        }
        cx.notify();
    }

    fn accept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let window_id = window.window_handle().window_id();
        let Some(shown) = self.dialogs.get(&window_id) else {
            return;
        };
        let handler = shown.dialog.on_confirm.clone();
        self.dismiss(window, cx);
        handler(window, cx);
    }

    fn push(&mut self, toast: Toast, cx: &mut Context<Self>) {
        self.next += 1;
        let revision = self.next;
        let lifetime = toast.lifetime();
        let existing = toast.key.as_ref().and_then(|key| {
            self.toasts
                .iter_mut()
                .find(|shown| shown.toast.key.as_ref() == Some(key))
        });
        let id = if let Some(shown) = existing {
            shown.revision = revision;
            shown.toast = toast;
            shown.id
        } else {
            self.toasts.push(Shown {
                id: revision,
                revision,
                toast,
            });
            revision
        };
        if let Some(lifetime) = lifetime {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(lifetime).await;
                this.update(cx, |overlays, cx| overlays.expire(id, revision, cx))
                    .ok();
            })
            .detach();
        }
        cx.notify();
    }

    fn expire(&mut self, id: u64, revision: u64, cx: &mut Context<Self>) {
        let current = self
            .toasts
            .iter()
            .any(|shown| shown.id == id && shown.revision == revision);
        if current {
            self.remove(id, cx);
        }
    }

    fn remove(&mut self, id: u64, cx: &mut Context<Self>) {
        self.toasts.retain(|shown| shown.id != id);
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        match keystroke.key.as_str() {
            "escape" => self.dismiss(window, cx),
            "enter" if keystroke.modifiers.platform => self.accept(window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn render_footer(
        dialog: &ConfirmDialog,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let confirm = Button::new("dialog-confirm", dialog.confirm.clone())
            .shortcut(CONFIRM_KEYS)
            .on_click(cx.listener(|this, _, window, cx| this.accept(window, cx)));
        let confirm = if dialog.primary {
            confirm.primary()
        } else {
            confirm.danger()
        };
        div()
            .flex()
            .items_center()
            .justify_end()
            .gap(SPACE_4)
            .px(SPACE_7)
            .py(SPACE_6)
            .border_t(RULE)
            .border_color(palette.line)
            .bg(palette.chrome)
            .child(
                Button::new("dialog-cancel", CANCEL)
                    .on_click(cx.listener(|this, _, window, cx| this.dismiss(window, cx))),
            )
            .child(confirm)
    }

    fn render_dialog(
        shown: &WindowDialog,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = *cx.palette();
        let viewport = window.viewport_size();
        let dialog = &shown.dialog;
        let header = div()
            .flex()
            .items_center()
            .h(PAGE_HEADER)
            .px(SPACE_7)
            .border_b(RULE)
            .border_color(palette.line)
            .child(
                div()
                    .flex_1()
                    .text_size(TEXT_TITLE)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(dialog.title.clone()),
            )
            .child(
                IconButton::glyph("dialog-close", CLOSE, CLOSE_TOOLTIP)
                    .on_click(cx.listener(|this, _, window, cx| this.dismiss(window, cx))),
            );
        let details = dialog.details.iter().map(|line| {
            div()
                .font_family(FONT_MONO)
                .text_size(TEXT_SMALL)
                .text_color(palette.fg)
                .child(line.clone())
        });
        let body = div()
            .flex()
            .flex_col()
            .gap(SPACE_4)
            .px(SPACE_7)
            .py(SPACE_6)
            .text_size(TEXT_BODY)
            .line_height(relative(LINE_PROSE))
            .text_color(palette.fg2)
            .child(dialog.body.clone())
            .when(!dialog.details.is_empty(), |body| {
                body.child(div().flex().flex_col().gap(SPACE_2).children(details))
            })
            .when_some(dialog.field.as_ref(), |body, field| {
                body.child(TextField::new(field))
            });
        let footer = Self::render_footer(dialog, &palette, cx);
        let panel = div()
            .id("dialog")
            .w(DIALOG_WIDTH)
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(palette.panel)
            .border(RULE)
            .border_color(palette.line2)
            .rounded(RADIUS_MENU)
            .shadow(popover::shadow(&palette))
            .track_focus(&shown.focus)
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(header)
            .child(body)
            .child(footer);
        let key = ElementId::NamedInteger("dialog".into(), shown.id);
        let scrim = div()
            .id("scrim")
            .w(viewport.width)
            .h(viewport.height)
            .flex()
            .justify_center()
            .items_start()
            .pt(DIALOG_TOP)
            .bg(palette.scrim)
            .occlude()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| this.dismiss(window, cx)),
            )
            .child(popover::appear(panel, key));
        deferred(
            anchored()
                .position_mode(AnchoredPositionMode::Window)
                .position(point(gpui::Pixels::ZERO, gpui::Pixels::ZERO))
                .child(scrim),
        )
        .priority(OVERLAY_PRIORITY)
    }

    fn render_toasts(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let viewport = window.viewport_size();
        let toasts: Vec<_> = self
            .toasts
            .iter()
            .map(|shown| {
                let id = shown.id;
                let toast = &shown.toast;
                let actions: Vec<_> = toast
                    .actions
                    .iter()
                    .enumerate()
                    .map(|(index, action)| {
                        let handler = action.on_click.clone();
                        Button::new(
                            ElementId::NamedInteger("toast-action".into(), index as u64),
                            action.label.clone(),
                        )
                        .small()
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.remove(id, cx);
                                handler(window, cx);
                            },
                        ))
                    })
                    .collect();
                let body = div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap(SPACE_2)
                    .min_w_0()
                    .child(div().text_size(TEXT_BODY).child(toast.title.clone()))
                    .when_some(toast_detail(toast, &palette), |text, detail| {
                        text.child(detail)
                    })
                    .when(!actions.is_empty(), |text| {
                        text.child(div().flex().gap(SPACE_4).pt(SPACE_2).children(actions))
                    });
                let card = div()
                    .id(ElementId::NamedInteger("toast".into(), id))
                    .occlude()
                    .w(TOAST_WIDTH)
                    .flex()
                    .gap(SPACE_4)
                    .px(SPACE_6)
                    .py(SPACE_5)
                    .bg(palette.toast_ground(toast.status))
                    .border(RULE)
                    .border_color(palette.line2)
                    .rounded(RADIUS_MENU)
                    .text_color(palette.fg)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.remove(id, cx)))
                    .child(div().pt(SPACE_2).child(StatusGlyph::new(toast.status)))
                    .child(body)
                    .child(
                        IconButton::glyph("toast-close", CLOSE, CLOSE_TOOLTIP)
                            .on_click(cx.listener(move |this, _, _, cx| this.remove(id, cx))),
                    );
                let animation = Animation::new(POPOVER_IN).with_easing(ease_out_quint());
                card.with_animation(
                    ElementId::NamedInteger("toast-in".into(), id),
                    animation,
                    Styled::opacity,
                )
                .into_any_element()
            })
            .collect();
        let corner = point(
            viewport.width - SPACE_8,
            viewport.height - STATUS_BAR - SPACE_8,
        );
        deferred(
            anchored()
                .position_mode(AnchoredPositionMode::Window)
                .anchor(Anchor::BottomRight)
                .position(corner)
                .child(div().flex().flex_col().gap(SPACE_4).children(toasts)),
        )
        .priority(OVERLAY_PRIORITY)
    }
}

impl Render for Overlays {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let window_id = window.window_handle().window_id();
        let dialog = self
            .dialogs
            .get(&window_id)
            .map(|shown| Self::render_dialog(shown, window, cx).into_any_element());
        let toasts = (!self.toasts.is_empty()).then(|| self.render_toasts(window, cx));
        div().children(dialog).children(toasts)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use gpui::TestAppContext;

    struct TestView;

    impl Render for TestView {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(layer(cx))
        }
    }

    #[gpui::test]
    fn confirmations_only_accept_in_their_own_window_and_drop_on_close(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::init(Some(crate::theme::Appearance::Dark), cx)
                .unwrap_or_else(|error| panic!("UI init: {error}"));
        });
        let first = cx.add_window(|_, _| TestView);
        let second = cx.add_window(|_, _| TestView);
        let first_accepted = Rc::new(Cell::new(false));
        let second_accepted = Rc::new(Cell::new(false));
        for (window, accepted) in [
            (first, first_accepted.clone()),
            (second, second_accepted.clone()),
        ] {
            assert!(
                window
                    .update(cx, |_, window, cx| {
                        let dialog = ConfirmDialog::new(
                            "Delete",
                            "Delete this resource?",
                            "Delete",
                            move |_, _| accepted.set(true),
                        );
                        confirm(dialog, window, cx);
                    })
                    .is_ok()
            );
        }
        cx.run_until_parked();
        let overlays = cx.read(layer);
        assert_eq!(
            overlays.read_with(cx, |overlays, _| overlays.dialogs.len()),
            2
        );
        cx.simulate_keystrokes(first.into(), "cmd-enter");
        assert!(first_accepted.get());
        assert!(!second_accepted.get());
        assert_eq!(
            overlays.read_with(cx, |overlays, _| overlays.dialogs.len()),
            1
        );
        assert!(
            second
                .update(cx, |_, window, _| window.remove_window())
                .is_ok()
        );
        assert!(overlays.read_with(cx, |overlays, _| overlays.dialogs.is_empty()));
    }

    #[gpui::test(iterations = 20)]
    fn backend_notices_are_shown_once_for_multiple_subscribers(cx: &mut TestAppContext) {
        cx.update(init);
        let (backend, updates) = async_channel::unbounded();
        let (commands, _) = async_channel::unbounded();
        let store = cx.new(|cx| crate::Store::new(updates, commands, cx));
        let first = store.update(cx, |store, _| store.subscribe());
        let second = store.update(cx, |store, _| store.subscribe());
        let notice = flock_core::Notice {
            status: Status::Ok,
            title: "Connected".into(),
            detail: None,
        };
        let update = flock_core::Update::Notice(notice);
        assert!(backend.try_send(update.clone()).is_ok());
        cx.run_until_parked();
        assert_eq!(first.try_recv(), Ok(update.clone()));
        assert_eq!(second.try_recv(), Ok(update));
        let overlays = cx.read(layer);
        assert_eq!(
            overlays.read_with(cx, |overlays, _| overlays.toasts.len()),
            1
        );
    }
}

#[cfg(test)]
mod lifetime_tests {
    use super::*;

    fn lifetime(status: Status) -> Option<Duration> {
        Toast::new(status, "t").lifetime()
    }

    #[test]
    fn only_errors_persist_and_warnings_outlast_successes() {
        assert_eq!(lifetime(Status::Err), None);
        assert_eq!(lifetime(Status::Ok), Some(TOAST_SUCCESS_TIMEOUT));
        assert_eq!(lifetime(Status::Warn), Some(TOAST_WARNING_TIMEOUT));
        assert_eq!(lifetime(Status::Progress), Some(TOAST_PROGRESS_TIMEOUT));
        assert!(TOAST_SUCCESS_TIMEOUT < TOAST_WARNING_TIMEOUT);
    }
}
