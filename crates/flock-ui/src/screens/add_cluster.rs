use std::collections::BTreeMap;

use flock_core::{
    AddClusterFlow, AddClusterStep, Candidate, CheckLine, CheckStatus, Command, ContextChoice,
    ContextRef, KubeconfigFile, Status, Update,
};
use gpui::{
    AnyElement, App, AppContext, ClipboardItem, Context, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div,
    prelude::FluentBuilder, relative,
};

use crate::kit::status_symbols;
use crate::kit::{
    Button, Checkbox, InputEvent, Modal, RawMessage, TextArea, TextAreaState, TextField,
    TextFieldState, Toast, toast,
};
use crate::theme::{
    ActiveTheme, CHECK_GLYPH_COLUMN, CODE_PAD_Y, DIALOG_WIDE, EMPTY_WIDTH, FONT_MONO, LINE_PROSE,
    PASTE_AREA_HEIGHT, PICK_CHECK_COLUMN, PICK_NAME_COLUMN, PICK_ROW_PAD_Y, Palette, RADIUS, RULE,
    SPACE_1, SPACE_2, SPACE_3, SPACE_4, SPACE_5, SPACE_6, SPACE_7, SPACE_8, TEXT_BODY,
    TEXT_CONTROL, TEXT_GLYPH, TEXT_HEADING, TEXT_MONO, TEXT_SMALL,
};

const SUBTITLE: &str = "Contexts not added yet";
const LOADING: &str = "Reading kubeconfig files…";
const NOTHING_LEFT: &str =
    "Every context in ~/.kube and $KUBECONFIG is already in Flock, or there are none yet.";
const NAME_LABEL: &str = "name";
const PASTE: &str = "Paste kubeconfig…";
const PASTE_TITLE: &str = "Paste kubeconfig";
const PASTE_SUBTITLE: &str =
    "Flock saves the pasted text in its own folder and lists its contexts here.";
const PASTE_NAME_PLACEHOLDER: &str = "file name, e.g. lab";
const PASTE_PLACEHOLDER: &str = "apiVersion: v1\nkind: Config\n…";
const SAVE: &str = "Save";
const CANCEL: &str = "Cancel";
const BACK: &str = "Back";
const COPY: &str = "Copy";
const RETRY: &str = "Retry";
const COPIED: &str = "Copied to the clipboard";
const NO_READY: &str = "No cluster has passed its checks yet.";
const INFO_GLYPH: &str = "○";

pub enum AddClusterEvent {
    Send(Command),
    Close,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Flow,
    Paste,
}

pub struct AddClusterDialog {
    flow: AddClusterFlow,
    view: View,
    paste_name: Entity<TextFieldState>,
    paste_text: Entity<TextAreaState>,
    paste_error: Option<String>,
    fields: BTreeMap<ContextRef, Entity<TextFieldState>>,
    focus: FocusHandle,
    subscriptions: Vec<Subscription>,
}

impl EventEmitter<AddClusterEvent> for AddClusterDialog {}

impl AddClusterDialog {
    pub fn new(
        step: AddClusterStep,
        existing: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let paste_name =
            cx.new(|cx| TextFieldState::new(window, cx).placeholder(PASTE_NAME_PLACEHOLDER));
        let paste_text = cx.new(|cx| TextAreaState::new(window, cx).placeholder(PASTE_PLACEHOLDER));
        let subscriptions = vec![
            cx.subscribe(&paste_name, |this, _, _: &InputEvent, cx| {
                this.paste_error = None;
                cx.notify();
            }),
            cx.subscribe(&paste_text, |this, _, _: &InputEvent, cx| {
                this.paste_error = None;
                cx.notify();
            }),
        ];
        Self {
            flow: AddClusterFlow::new(step, existing),
            view: View::Flow,
            paste_name,
            paste_text,
            paste_error: None,
            fields: BTreeMap::new(),
            focus,
            subscriptions,
        }
    }

    pub fn receive(&mut self, update: &Update, window: &mut Window, cx: &mut Context<Self>) {
        match update {
            Update::Contexts(files) => {
                self.leave_paste(window, cx);
                self.load(files.clone(), window, cx);
            }
            Update::KubeconfigRejected(reason) => self.reject(reason, cx),
            Update::Check(id, line) => self.flow.record(id, line.clone()),
            Update::Checked(id, verdict) => self.flow.finish(id, *verdict),
            Update::Snapshot(_) => return,
            Update::Notice(_) => return,
        }
        cx.notify();
    }

    fn leave_paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.view != View::Paste {
            return;
        }
        self.view = View::Flow;
        self.paste_error = None;
        self.paste_name
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.paste_text
            .update(cx, |field, cx| field.set_value("", window, cx));
        window.focus(&self.focus, cx);
    }

    fn reject(&mut self, reason: &str, cx: &mut Context<Self>) {
        match self.view {
            View::Paste => self.paste_error = Some(reason.to_owned()),
            View::Flow => toast(Toast::new(Status::Err, reason.to_owned()), cx),
        }
    }

    fn save_paste(&mut self, cx: &mut Context<Self>) {
        let name = self.paste_name.read(cx).value().to_string();
        let yaml = self.paste_text.read(cx).value().to_string();
        cx.emit(AddClusterEvent::Send(Command::SaveKubeconfig {
            name,
            yaml,
        }));
    }

    fn load(&mut self, files: Vec<KubeconfigFile>, window: &mut Window, cx: &mut Context<Self>) {
        let started = self.flow.load(files);
        let ids: Vec<ContextRef> = self.flow.choices().map(|c| c.id.clone()).collect();
        for id in ids {
            if self.fields.contains_key(&id) {
                continue;
            }
            let name = self.flow.name(&id).to_owned();
            let field = cx.new(|cx| TextFieldState::new(window, cx).default_value(name));
            let renamed = id.clone();
            let subscription = cx.subscribe(&field, move |this, field, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let value = field.read(cx).value();
                    this.flow.rename(&renamed, &value);
                    cx.notify();
                }
            });
            self.subscriptions.push(subscription);
            self.fields.insert(id, field);
        }
        if let Some(ids) = started {
            cx.emit(AddClusterEvent::Send(Command::CheckContexts(ids)));
        }
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        if let Some(ids) = self.flow.connect() {
            cx.emit(AddClusterEvent::Send(Command::CheckContexts(ids)));
            cx.notify();
        }
    }

    fn retry(&mut self, id: &ContextRef, cx: &mut Context<Self>) {
        if self.flow.retry(id) {
            cx.emit(AddClusterEvent::Send(Command::CheckContexts(vec![
                id.clone(),
            ])));
            cx.notify();
        }
    }

    fn add(&mut self, cx: &mut Context<Self>) {
        let ready = self.flow.ready();
        if ready.is_empty() {
            return;
        }
        let names: Vec<String> = ready.iter().map(|c| c.name.clone()).collect();
        cx.emit(AddClusterEvent::Send(Command::AddClusters(ready)));
        toast(
            Toast::new(Status::Ok, format!("Added {}", names.join(", "))),
            cx,
        );
        cx.emit(AddClusterEvent::Close);
    }

    fn pick(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let list: Vec<AnyElement> = match &self.flow.files {
            None => vec![note(LOADING, palette)],
            Some(files) if files.iter().all(|f| f.contexts.is_empty()) => {
                vec![note(NOTHING_LEFT, palette)]
            }
            Some(files) => {
                let mut ix = 0;
                let mut out = Vec::new();
                for file in files {
                    out.push(file_label(&file.label, palette));
                    for choice in &file.contexts {
                        out.push(self.context_row(ix, choice, palette, cx));
                        ix += 1;
                    }
                }
                out
            }
        };
        let problem = self.flow.name_problem().map(|p| p.sentence());
        let connect = Button::new("connect", self.flow.connect_label())
            .primary()
            .disabled(!self.flow.can_connect())
            .when_some(problem, Button::tooltip)
            .on_click(cx.listener(|this, _, _, cx| this.connect(cx)));
        let close = cx.listener(|_, _, _, cx| cx.emit(AddClusterEvent::Close));
        let footer = footer(palette)
            .border_t(RULE)
            .border_color(palette.line)
            .child(Button::new("paste", PASTE).ghost().on_click(cx.listener(
                |this, _, window, cx| {
                    this.view = View::Paste;
                    window.focus(&this.paste_name.focus_handle(cx), cx);
                    cx.notify();
                },
            )))
            .child(div().flex_1())
            .child(Button::new("cancel", CANCEL).on_click(close))
            .child(connect);
        body()
            .child(
                div()
                    .id("pick-list")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .px(SPACE_7)
                            .pt(SPACE_6)
                            .pb(SPACE_2)
                            .text_size(TEXT_BODY)
                            .text_color(palette.fg2)
                            .child(SUBTITLE),
                    )
                    .children(list),
            )
            .child(footer)
            .into_any_element()
    }

    fn paste(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let name = self.paste_name.read(cx).value();
        let text = self.paste_text.read(cx).value();
        let empty = name.trim().is_empty() || text.trim().is_empty();
        let back = Button::new("paste-back", BACK).on_click(cx.listener(|this, _, window, cx| {
            this.view = View::Flow;
            this.paste_error = None;
            window.focus(&this.focus, cx);
            cx.notify();
        }));
        let save = Button::new("paste-save", SAVE)
            .primary()
            .disabled(empty)
            .on_click(cx.listener(|this, _, _, cx| this.save_paste(cx)));
        let footer = footer(palette)
            .child(back)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(TEXT_CONTROL)
                    .text_color(palette.err)
                    .children(self.paste_error.clone()),
            )
            .child(save);
        body()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .gap(SPACE_4)
                    .px(SPACE_7)
                    .py(SPACE_6)
                    .child(
                        div()
                            .text_size(TEXT_BODY)
                            .text_color(palette.fg2)
                            .child(PASTE_SUBTITLE),
                    )
                    .child(TextField::new(&self.paste_name))
                    .child(TextArea::new(&self.paste_text, PASTE_AREA_HEIGHT)),
            )
            .child(footer)
            .into_any_element()
    }

    fn context_row(
        &self,
        ix: usize,
        choice: &ContextChoice,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on = self.flow.is_selected(&choice.id);
        let id = choice.id.clone();
        let hover = palette.hover;
        let field = self.fields.get(&choice.id).filter(|_| on).cloned();
        let label = div()
            .id(ElementId::NamedInteger("context".into(), ix as u64))
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(SPACE_5)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.flow.toggle(&id);
                cx.notify();
            }))
            .child(div().w(PICK_CHECK_COLUMN).flex_none().child(Checkbox::new(
                ElementId::NamedInteger("context-check".into(), ix as u64),
                on,
            )))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_1)
                    .min_w_0()
                    .font_family(FONT_MONO)
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
                    ),
            );
        let name = div()
            .w(PICK_NAME_COLUMN)
            .flex_none()
            .flex()
            .items_center()
            .gap(SPACE_3)
            .text_size(TEXT_SMALL)
            .text_color(palette.mute)
            .when_some(field, |name, field| {
                name.child(NAME_LABEL).child(TextField::new(&field))
            });
        div()
            .flex()
            .items_center()
            .gap(SPACE_5)
            .px(SPACE_7)
            .py(PICK_ROW_PAD_Y)
            .border_t(RULE)
            .border_color(palette.line)
            .hover(move |style| style.bg(hover))
            .child(label)
            .child(name)
            .into_any_element()
    }

    fn check(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let candidates: Vec<AnyElement> = self
            .flow
            .candidates
            .iter()
            .enumerate()
            .map(|(ix, candidate)| Self::candidate(ix, candidate, palette, cx))
            .collect();
        let ready = !self.flow.ready().is_empty();
        let add = Button::new("add", self.flow.add_label())
            .primary()
            .disabled(!ready)
            .when(!ready, |button| button.tooltip(NO_READY))
            .on_click(cx.listener(|this, _, _, cx| this.add(cx)));
        let back = Button::new("back", BACK).on_click(cx.listener(|this, _, _, cx| {
            this.flow.back();
            cx.notify();
        }));
        let footer = footer(palette)
            .child(back)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(TEXT_CONTROL)
                    .text_color(palette.mute)
                    .child(self.flow.ready_note()),
            )
            .child(add);
        body()
            .child(
                div()
                    .id("check-list")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(candidates),
            )
            .child(footer)
            .into_any_element()
    }

    fn candidate(
        ix: usize,
        candidate: &Candidate,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let status = candidate.verdict.status();
        let header = div()
            .flex()
            .items_baseline()
            .gap(SPACE_5)
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(candidate.name.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_baseline()
                    .gap(SPACE_2)
                    .text_size(TEXT_BODY)
                    .text_color(palette.status(status))
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(TEXT_GLYPH)
                            .child(status_symbols(status.glyph())),
                    )
                    .child(candidate.verdict.label()),
            )
            .child(div().flex_1())
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(FONT_MONO)
                    .text_size(TEXT_SMALL)
                    .text_color(palette.mute)
                    .child(candidate.context.context.clone()),
            );
        let lines: Vec<AnyElement> = candidate
            .checks
            .iter()
            .enumerate()
            .map(|(line_ix, line)| check_line((ix, line_ix), &candidate.context, line, palette, cx))
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(SPACE_3)
            .px(SPACE_7)
            .py(SPACE_6)
            .border_b(RULE)
            .border_color(palette.line)
            .child(header)
            .children(lines)
            .into_any_element()
    }
}

impl Render for AddClusterDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let content = match (self.view, self.flow.step) {
            (View::Paste, _) => self.paste(&palette, cx),
            (View::Flow, AddClusterStep::Pick) => self.pick(&palette, cx),
            (View::Flow, AddClusterStep::Check) => self.check(&palette, cx),
        };
        let title = match self.view {
            View::Paste => PASTE_TITLE.to_owned(),
            View::Flow => self.flow.title(),
        };
        let this = cx.entity().downgrade();
        Modal::new("add-cluster", title, DIALOG_WIDE)
            .focus(&self.focus)
            .on_close(move |_, cx| {
                this.update(cx, |_, cx| cx.emit(AddClusterEvent::Close))
                    .ok();
            })
            .child(content)
    }
}

fn check_line(
    (ix, line_ix): (usize, usize),
    context: &ContextRef,
    line: &CheckLine,
    palette: &Palette,
    cx: &mut Context<AddClusterDialog>,
) -> AnyElement {
    let (glyph, color) = check_mark(line.status, palette);
    let remedy = line.remedy.clone().map(|command| {
        let copied = command.clone();
        let retried = context.clone();
        div()
            .flex()
            .items_center()
            .gap(SPACE_4)
            .mt(SPACE_3)
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(TEXT_MONO)
                    .px(SPACE_4)
                    .py(CODE_PAD_Y)
                    .border(RULE)
                    .border_color(palette.line2)
                    .rounded(RADIUS)
                    .bg(palette.editor)
                    .text_color(palette.fg)
                    .child(command),
            )
            .child(
                Button::new(button_id("copy", ix, line_ix), COPY)
                    .small()
                    .on_click(move |_, _, cx: &mut App| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                        toast(Toast::new(Status::Ok, COPIED), cx);
                    }),
            )
            .child(
                Button::new(button_id("retry", ix, line_ix), RETRY)
                    .small()
                    .on_click(cx.listener(move |this, _, _, cx| this.retry(&retried, cx))),
            )
    });
    let raw = line
        .raw
        .clone()
        .map(|message| RawMessage::new(message).color(palette.fg2));
    let detail = (!line.detail.is_empty()).then(|| line.detail.clone());
    let text = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(SPACE_4)
        .min_w_0()
        .children(raw)
        .children(detail);
    div()
        .flex()
        .gap(SPACE_4)
        .text_size(TEXT_BODY)
        .line_height(relative(LINE_PROSE))
        .child(
            div()
                .w(CHECK_GLYPH_COLUMN)
                .flex_none()
                .pt(SPACE_1)
                .font_family(FONT_MONO)
                .text_size(TEXT_GLYPH)
                .text_color(color)
                .child(status_symbols(glyph)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .items_start()
                .min_w_0()
                .text_color(palette.fg2)
                .child(text)
                .children(remedy),
        )
        .into_any_element()
}

fn button_id(name: &str, ix: usize, line_ix: usize) -> ElementId {
    ElementId::NamedInteger(format!("{name}-{ix}").into(), line_ix as u64)
}

fn check_mark(status: CheckStatus, palette: &Palette) -> (SharedString, Hsla) {
    let shown = match status {
        CheckStatus::Ok => Some(Status::Ok),
        CheckStatus::Warn => Some(Status::Warn),
        CheckStatus::Err => Some(Status::Err),
        CheckStatus::Info => None,
    };
    match shown {
        Some(status) => (status.glyph().into(), palette.status(status)),
        None => (INFO_GLYPH.into(), palette.mute),
    }
}

pub(super) fn body() -> gpui::Div {
    div().flex().flex_col().flex_1().min_h_0()
}

pub(super) fn footer(palette: &Palette) -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(SPACE_4)
        .px(SPACE_7)
        .py(SPACE_6)
        .bg(palette.chrome)
}

pub(super) fn file_label(label: &str, palette: &Palette) -> AnyElement {
    div()
        .px(SPACE_7)
        .pt(SPACE_6)
        .pb(SPACE_3)
        .font_family(FONT_MONO)
        .text_size(TEXT_SMALL)
        .text_color(palette.mute)
        .child(label.to_owned())
        .into_any_element()
}

pub(super) fn note(text: &'static str, palette: &Palette) -> AnyElement {
    div()
        .px(SPACE_7)
        .py(SPACE_6)
        .text_size(TEXT_BODY)
        .text_color(palette.mute)
        .child(text)
        .into_any_element()
}

const EMPTY_TITLE: &str = "No clusters yet";
const EMPTY_ACTION: &str = "Add cluster";

pub fn empty_flock(
    on_add: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .items_start()
        .gap(SPACE_4)
        .px(SPACE_8)
        .pt(SPACE_7)
        .max_w(EMPTY_WIDTH)
        .child(
            div()
                .text_size(TEXT_HEADING)
                .font_weight(FontWeight::SEMIBOLD)
                .child(EMPTY_TITLE),
        )
        .child(
            div().pt(SPACE_2).child(
                Button::new("empty-add-cluster", EMPTY_ACTION)
                    .primary()
                    .on_click(on_add),
            ),
        )
}
