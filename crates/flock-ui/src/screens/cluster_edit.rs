use flock_core::{ClusterId, Command, ContextChoice, KubeconfigFile, Update};
use gpui::{
    AnyElement, AppContext, Context, ElementId, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement, Styled,
    Subscription, Window, div,
};

use super::add_cluster::{body, file_label, footer, note};
use crate::kit::{Button, InputEvent, Modal, TextField, TextFieldEscape, TextFieldState};
use crate::theme::{
    ActiveTheme, DIALOG_WIDE, DIALOG_WIDTH, FONT_MONO, PICK_ROW_PAD_Y, Palette, RULE, SPACE_1,
    SPACE_4, SPACE_6, SPACE_7, TEXT_BODY, TEXT_MONO, TEXT_SMALL,
};

const RENAME_TITLE: &str = "Rename cluster";
const RENAME_PLACEHOLDER: &str = "Display name";
const RENAME_ID: &str = "rename-cluster";
const SAVE: &str = "Save";
const CANCEL: &str = "Cancel";
const SWITCH_TITLE: &str = "Switch context";
const SWITCH_ID: &str = "switch-context";
const LOADING: &str = "Reading kubeconfig files…";
const NOTHING_LEFT: &str = "Every context in ~/.kube and $KUBECONFIG is already in Flock.";

pub enum ClusterEditEvent {
    Send(Command),
    Close,
}

pub struct RenameDialog {
    cluster: ClusterId,
    input: Entity<TextFieldState>,
    focus: FocusHandle,
    _subscription: Subscription,
}

impl EventEmitter<ClusterEditEvent> for RenameDialog {}

impl RenameDialog {
    pub fn new(
        cluster: ClusterId,
        current: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let value = current.to_owned();
        let input = cx.new(|cx| {
            TextFieldState::new(window, cx)
                .placeholder(RENAME_PLACEHOLDER)
                .default_value(value)
        });
        window.focus(&input.focus_handle(cx), cx);
        let subscription = cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.save(cx);
            }
        });
        Self {
            cluster,
            input,
            focus: cx.focus_handle(),
            _subscription: subscription,
        }
    }

    fn save(&self, cx: &mut Context<Self>) {
        let value = self.input.read(cx).value().to_string();
        let command = Command::RenameCluster(self.cluster.clone(), value);
        cx.emit(ClusterEditEvent::Send(command));
        cx.emit(ClusterEditEvent::Close);
    }
}

impl Render for RenameDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let this = cx.entity().downgrade();
        let save = Button::new("rename-save", SAVE)
            .primary()
            .on_click(cx.listener(|this, _, _, cx| this.save(cx)));
        let cancel = Button::new("rename-cancel", CANCEL)
            .on_click(cx.listener(|_, _, _, cx| cx.emit(ClusterEditEvent::Close)));
        let field = div()
            .px(SPACE_7)
            .py(SPACE_6)
            .on_action(cx.listener(|_, _: &TextFieldEscape, _, cx| {
                cx.emit(ClusterEditEvent::Close);
            }))
            .child(TextField::new(&self.input));
        let actions = footer(&palette).justify_end().child(cancel).child(save);
        Modal::new(RENAME_ID, RENAME_TITLE, DIALOG_WIDTH)
            .focus(&self.focus)
            .on_close(move |_, cx| {
                this.update(cx, |_, cx| cx.emit(ClusterEditEvent::Close))
                    .ok();
            })
            .child(body().child(field).child(actions))
    }
}

pub struct SwitchContextDialog {
    cluster: ClusterId,
    files: Option<Vec<KubeconfigFile>>,
    focus: FocusHandle,
}

impl EventEmitter<ClusterEditEvent> for SwitchContextDialog {}

impl SwitchContextDialog {
    pub fn new(cluster: ClusterId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            cluster,
            files: None,
            focus,
        }
    }

    pub fn receive(&mut self, update: &Update, cx: &mut Context<Self>) {
        if let Update::Contexts(files) = update {
            self.files = Some(files.clone());
            cx.notify();
        }
    }

    fn row(
        &self,
        ix: usize,
        choice: &ContextChoice,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let target = choice.id.clone();
        let cluster = self.cluster.clone();
        let hover = palette.hover;
        div()
            .debug_selector(move || format!("switch-context-row-{ix}"))
            .id(ElementId::NamedInteger(SWITCH_ID.into(), ix as u64))
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .min_w_0()
            .px(SPACE_7)
            .py(PICK_ROW_PAD_Y)
            .border_t(RULE)
            .border_color(palette.line)
            .font_family(FONT_MONO)
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |_, _, _, cx| {
                let command = Command::SwitchContext(cluster.clone(), target.clone());
                cx.emit(ClusterEditEvent::Send(command));
                cx.emit(ClusterEditEvent::Close);
            }))
            .child(
                div()
                    .truncate()
                    .text_size(TEXT_MONO)
                    .child(choice.id.context.clone()),
            )
            .child(
                div()
                    .truncate()
                    .text_size(TEXT_SMALL)
                    .text_color(palette.mute)
                    .child(choice.meta()),
            )
            .into_any_element()
    }
}

impl Render for SwitchContextDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let this = cx.entity().downgrade();
        let list: Vec<AnyElement> = match &self.files {
            None => vec![note(LOADING, &palette)],
            Some(files) if files.iter().all(|file| file.contexts.is_empty()) => {
                vec![note(NOTHING_LEFT, &palette)]
            }
            Some(files) => {
                let mut ix = 0;
                let mut out = Vec::new();
                for file in files {
                    out.push(file_label(&file.label, &palette));
                    for choice in &file.contexts {
                        out.push(self.row(ix, choice, &palette, cx));
                        ix += 1;
                    }
                }
                out
            }
        };
        let cancel = Button::new("switch-cancel", CANCEL)
            .on_click(cx.listener(|_, _, _, cx| cx.emit(ClusterEditEvent::Close)));
        let actions = footer(&palette)
            .justify_end()
            .border_t(RULE)
            .border_color(palette.line)
            .child(cancel);
        let rows = div()
            .id("switch-context-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .text_size(TEXT_BODY)
            .pb(SPACE_4)
            .children(list);
        Modal::new(SWITCH_ID, SWITCH_TITLE, DIALOG_WIDE)
            .focus(&self.focus)
            .on_close(move |_, cx| {
                this.update(cx, |_, cx| cx.emit(ClusterEditEvent::Close))
                    .ok();
            })
            .child(body().child(rows).child(actions))
    }
}
