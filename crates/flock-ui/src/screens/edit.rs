use flock_core::changes::{LineChange, PatchLine};
use flock_core::edit::{
    Conflict, Draft, EditEvent, EditRequest, EditStep, Invalid, Live, is_secret, validate,
};
use flock_core::overview::plural;
use flock_core::{Command, ResourceRef, Sink, Status};
use gpui::{
    AnyElement, AppContext, Context, Entity, EventEmitter, FocusHandle, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Task, Window, div,
};

use crate::kit::{
    Button, CodeEditor, CodeEditorState, ConfirmDialog, DiffFile, DiffView, EmptyState, InputEvent,
    MetaLine, PageHeader, SectionHeader, Skeleton, TextFieldEnter, Toast, confirm, highlight_yaml,
    mark_error, toast, yaml_editor,
};
use crate::screens::compare::diff_line;
use crate::theme::{
    ActiveTheme, FONT_MONO, Palette, RULE, SPACE_3, SPACE_4, SPACE_5, SPACE_8, TEXT_MONO,
    TEXT_SMALL,
};

gpui::actions!(edit_yaml, [Submit, EditYaml]);

pub const KEY_CONTEXT: &str = "EditYaml";
pub const YAML_CONTEXT: &str = "YamlView";
pub const EDIT_KEYS: &str = "⌘E";
const SEPARATOR: &str = " · ";
const SUBMIT_KEYS: &str = "⌘↵";
const REVIEW: &str = "Review changes";
const APPLY: &str = "Apply";
const BACK: &str = "Back to editing";
const DISCARD: &str = "Discard changes";
const REVEAL: &str = "Reveal values…";
const REBASE: &str = "Rebase my edit";
const RELOAD: &str = "Discard my edit";
const RETRY: &str = "Retry";
const HIDDEN_NOTE: &str = "managedFields and status are hidden; they are not part of the edit";
const REDACTED_NOTE: &str = "Secret values are hidden as ••••••; untouched values are kept";
const NO_CHANGES: &str = "The dry run changes nothing on the cluster.";
const LIVE_SIDE: &str = "live";
const DRY_RUN_SIDE: &str = "after apply";
const OPENED_SIDE: &str = "when you opened it";
const NOW_SIDE: &str = "now";
const LOADING: &str = "Loading the live object…";
const REVIEWING: &str = "Running a server-side dry run…";
const APPLYING: &str = "Applying…";
const LINE_LABEL: &str = "Line";
const LINE: &str = "line";
const LINES: &str = "lines";
const FOOTER_SELECTOR: &str = "edit-footer";
const INVALID_SELECTOR: &str = "edit-invalid";
const FAILED_SELECTOR: &str = "edit-failed";

#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    Loading,
    Editing,
    Waiting(&'static str),
    Review(Vec<PatchLine>),
    Conflict(Box<Conflict>),
    Missing(String),
}

pub struct EditScreen {
    pub(crate) navigation: Option<Entity<crate::kit::HeaderNavigation>>,
    target: ResourceRef,
    editor: Entity<CodeEditorState>,
    base: Option<Live>,
    phase: Phase,
    invalid: Option<Invalid>,
    failure: Option<String>,
    confirm_apply: bool,
    palette: Option<Palette>,
    request: Option<Task<()>>,
    pending: Option<EditEvent>,
    focus: FocusHandle,
    _changes: Subscription,
}

impl EventEmitter<Command> for EditScreen {}

impl EditScreen {
    pub fn new(target: ResourceRef, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| yaml_editor(window, cx));
        let changes = cx.subscribe(&editor, |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                this.edited(cx);
            }
        });
        Self {
            navigation: None,
            target,
            editor,
            base: None,
            phase: Phase::Loading,
            invalid: None,
            failure: None,
            confirm_apply: true,
            palette: None,
            request: None,
            pending: None,
            focus: cx.focus_handle(),
            _changes: changes,
        }
    }

    pub fn target(&self) -> &ResourceRef {
        &self.target
    }

    pub fn set_confirm_apply(&mut self, confirm_apply: bool) {
        self.confirm_apply = confirm_apply;
    }

    pub fn load(&mut self, cx: &mut Context<Self>) {
        let reveal = self.base.as_ref().is_some_and(|base| base.revealed);
        self.send(EditStep::Load { reveal }, Phase::Loading, cx);
    }

    fn reveal(&mut self, cx: &mut Context<Self>) {
        self.send(EditStep::Load { reveal: true }, Phase::Loading, cx);
    }

    fn text(&self, cx: &Context<Self>) -> String {
        self.editor.read(cx).value().to_string()
    }

    fn dirty(&self, cx: &Context<Self>) -> bool {
        self.base
            .as_ref()
            .is_some_and(|base| base.text != self.text(cx))
    }

    fn draft(&self, cx: &Context<Self>) -> Option<Draft> {
        let base = self.base.clone()?;
        Some(Draft {
            base,
            text: self.text(cx),
        })
    }

    fn edited(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx);
        self.invalid = validate(&self.target, &text).err();
        self.failure = None;
        let line = self
            .invalid
            .as_ref()
            .and_then(Invalid::line)
            .map(|line| line.saturating_sub(1));
        let message = self.invalid.as_ref().map(ToString::to_string);
        self.editor.update(cx, |editor, cx| {
            mark_error(editor, line, message.as_deref(), cx);
        });
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.phase {
            Phase::Editing => self.review(cx),
            Phase::Review(_) => self.apply(window, cx),
            Phase::Loading => {}
            Phase::Waiting(_) => {}
            Phase::Conflict(_) => {}
            Phase::Missing(_) => {}
        }
    }

    fn review(&mut self, cx: &mut Context<Self>) {
        if self.invalid.is_some() || !self.dirty(cx) {
            return;
        }
        if let Some(draft) = self.draft(cx) {
            self.send(EditStep::Review(draft), Phase::Waiting(REVIEWING), cx);
        }
    }

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.confirm_apply {
            self.apply_now(cx);
            return;
        }
        let screen = cx.weak_entity();
        let title = format!("Apply to {}", self.target.name);
        let body = format!(
            "The edited {} is applied with server-side apply as flock. If Flux or another controller manages it, it may revert the change on its next reconcile.",
            self.target.kind
        );
        let detail = format!("{}{SEPARATOR}{}", self.target.cluster, self.target);
        let dialog = ConfirmDialog::new(title, body, APPLY, move |_, cx| {
            let _ = screen.update(cx, EditScreen::apply_now);
        })
        .detail(detail)
        .primary();
        confirm(dialog, window, cx);
    }

    fn apply_now(&mut self, cx: &mut Context<Self>) {
        if let Some(draft) = self.draft(cx) {
            self.send(EditStep::Apply(draft), Phase::Waiting(APPLYING), cx);
        }
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        self.phase = Phase::Editing;
        cx.notify();
    }

    fn discard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(base) = self.base.clone() {
            self.set_text(&base.text, window, cx);
        }
    }

    fn set_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.to_owned();
        self.editor
            .update(cx, |editor, cx| editor.set_value(text, window, cx));
        self.edited(cx);
    }

    fn adopt(&mut self, live: Live, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.base = Some(live);
        self.phase = Phase::Editing;
        self.set_text(text, window, cx);
        self.editor
            .update(cx, |editor, cx| editor.focus(window, cx));
    }

    fn send(&mut self, step: EditStep, phase: Phase, cx: &mut Context<Self>) {
        let (sink, events) = Sink::channel();
        self.phase = phase;
        self.failure = None;
        cx.emit(Command::Edit(Box::new(EditRequest {
            target: self.target.clone(),
            step,
            sink,
        })));
        self.request = Some(cx.spawn(async move |this, cx| {
            let Ok(event) = events.recv().await else {
                return;
            };
            let _ = this.update(cx, |screen, cx| {
                screen.pending = Some(event);
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub fn receive(&mut self, event: EditEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            EditEvent::Loaded(live) => {
                let text = live.text.clone();
                self.adopt(live, &text, window, cx);
            }
            EditEvent::Reviewed(lines) => {
                self.phase = Phase::Review(lines);
                window.focus(&self.focus, cx);
            }
            EditEvent::Applied(live) => {
                let title = format!("Applied {}/{}", self.target.kind, self.target.name);
                let detail = format!("{}{SEPARATOR}{}", self.target.cluster, self.target);
                toast(Toast::new(Status::Ok, title).detail(detail), cx);
                let text = live.text.clone();
                self.adopt(live, &text, window, cx);
            }
            EditEvent::Conflict(conflict) => {
                self.phase = Phase::Conflict(conflict);
                window.focus(&self.focus, cx);
            }
            EditEvent::Failed(reason) if self.base.is_none() => {
                self.phase = Phase::Missing(reason);
            }
            EditEvent::Failed(reason) => {
                self.failure = Some(reason);
                self.phase = Phase::Editing;
            }
        }
        cx.notify();
    }

    fn rebase(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Phase::Conflict(conflict) = &self.phase else {
            return;
        };
        let live = conflict.live.clone();
        let text = conflict.rebased.text.clone();
        self.adopt(live, &text, window, cx);
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Phase::Conflict(conflict) = &self.phase else {
            return;
        };
        let live = conflict.live.clone();
        let text = live.text.clone();
        self.adopt(live, &text, window, cx);
    }

    fn confirm_reveal(&self, window: &mut Window, cx: &mut Context<Self>) {
        let screen = cx.weak_entity();
        let title = format!("Reveal values of {}", self.target.name);
        let body = "Secret values show in plain text in this tab until you close it. The editor reloads from the cluster, so unsaved edits are lost.";
        let detail = format!("{}{SEPARATOR}{}", self.target.cluster, self.target);
        let dialog = ConfirmDialog::new(title, body, "Reveal values", move |_, cx| {
            let _ = screen.update(cx, EditScreen::reveal);
        })
        .detail(detail);
        confirm(dialog, window, cx);
    }

    fn actions(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        match &self.phase {
            Phase::Editing => {
                let mut actions = Vec::new();
                let revealable = is_secret(&self.target)
                    && self.base.as_ref().is_some_and(|base| !base.revealed);
                if revealable {
                    actions.push(
                        Button::new("edit-reveal", REVEAL)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.confirm_reveal(window, cx);
                            }))
                            .into_any_element(),
                    );
                }
                let dirty = self.dirty(cx);
                actions.push(
                    Button::new("edit-discard", DISCARD)
                        .disabled(!dirty)
                        .on_click(cx.listener(|this, _, window, cx| this.discard(window, cx)))
                        .into_any_element(),
                );
                actions.push(
                    Button::new("edit-review", REVIEW)
                        .primary()
                        .shortcut(SUBMIT_KEYS)
                        .disabled(!dirty || self.invalid.is_some())
                        .on_click(cx.listener(|this, _, _, cx| this.review(cx)))
                        .into_any_element(),
                );
                actions
            }
            Phase::Review(lines) => vec![
                Button::new("edit-back", BACK)
                    .on_click(cx.listener(|this, _, _, cx| this.back(cx)))
                    .into_any_element(),
                Button::new("edit-apply", APPLY)
                    .primary()
                    .shortcut(SUBMIT_KEYS)
                    .disabled(lines.is_empty())
                    .on_click(cx.listener(|this, _, window, cx| this.apply(window, cx)))
                    .into_any_element(),
            ],
            Phase::Conflict(_) => vec![
                Button::new("edit-reload", RELOAD)
                    .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx)))
                    .into_any_element(),
                Button::new("edit-rebase", REBASE)
                    .primary()
                    .on_click(cx.listener(|this, _, window, cx| this.rebase(window, cx)))
                    .into_any_element(),
            ],
            Phase::Loading => Vec::new(),
            Phase::Waiting(_) => Vec::new(),
            Phase::Missing(_) => Vec::new(),
        }
    }

    fn meta(&self) -> String {
        let mut facts = vec![self.target.kind.clone()];
        facts.extend(self.target.namespace.clone());
        facts.push(self.target.cluster.to_string());
        if let Some(base) = &self.base {
            facts.push(format!("resourceVersion {}", base.resource_version));
        }
        facts.join(SEPARATOR)
    }

    fn footer(&self, palette: &Palette) -> impl IntoElement + use<> {
        let (text, color, selector) = match (&self.invalid, &self.failure) {
            (Some(invalid), _) => (invalid_line(invalid), palette.err, INVALID_SELECTOR),
            (None, Some(failure)) => (failure.clone(), palette.err, FAILED_SELECTOR),
            (None, None) => (self.note(), palette.mute, FOOTER_SELECTOR),
        };
        div()
            .id("edit-footer")
            .debug_selector(move || selector.to_owned())
            .flex_none()
            .px(SPACE_8)
            .py(SPACE_4)
            .border_t(RULE)
            .border_color(palette.line)
            .font_family(FONT_MONO)
            .text_size(TEXT_SMALL)
            .text_color(color)
            .truncate()
            .child(text)
    }

    fn note(&self) -> String {
        let redacted =
            is_secret(&self.target) && self.base.as_ref().is_some_and(|base| !base.revealed);
        if redacted {
            format!("{HIDDEN_NOTE}{SEPARATOR}{REDACTED_NOTE}")
        } else {
            HIDDEN_NOTE.to_owned()
        }
    }

    fn body(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        match &self.phase {
            Phase::Loading => waiting(LOADING, palette),
            Phase::Waiting(label) => waiting(label, palette),
            Phase::Missing(reason) => EmptyState::new(reason.clone())
                .action(
                    Button::new("edit-retry", RETRY)
                        .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                )
                .into_any_element(),
            Phase::Editing => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .id("edit-editor")
                        .debug_selector(|| "edit-editor".to_owned())
                        .flex_1()
                        .min_h_0()
                        .px(SPACE_5)
                        .py(SPACE_3)
                        .child(CodeEditor::new(&self.editor)),
                )
                .child(self.footer(palette))
                .into_any_element(),
            Phase::Review(lines) => review(lines, &self.target, palette),
            Phase::Conflict(conflict) => conflicted(conflict, &self.target, palette),
        }
    }
}

fn invalid_line(invalid: &Invalid) -> String {
    match invalid.line() {
        Some(line) => format!("{LINE_LABEL} {line}: {invalid}"),
        None => invalid.to_string(),
    }
}

fn waiting(label: &'static str, palette: &Palette) -> AnyElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .gap(SPACE_4)
        .px(SPACE_8)
        .py(SPACE_5)
        .child(
            div()
                .text_size(TEXT_SMALL)
                .text_color(palette.mute)
                .child(label),
        )
        .child(Skeleton::rows())
        .into_any_element()
}

fn diff(id: &'static str, title: String, lines: &[PatchLine]) -> DiffView {
    let lines = lines.iter().map(diff_line).collect();
    DiffView::new(id, vec![DiffFile::new(title, lines)])
}

fn scrolled(id: &'static str, header: SectionHeader, content: impl IntoElement) -> AnyElement {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .py(SPACE_5)
        .child(header)
        .child(div().px(SPACE_8).child(content))
        .into_any_element()
}

fn changes(lines: &[PatchLine]) -> String {
    let changed = lines
        .iter()
        .filter(|line| matches!(line.change, LineChange::Added | LineChange::Removed))
        .count();
    plural(changed, LINE, LINES)
}

fn review(lines: &[PatchLine], target: &ResourceRef, palette: &Palette) -> AnyElement {
    if lines.is_empty() {
        return EmptyState::new(NO_CHANGES).into_any_element();
    }
    let title = format!("{}/{}", target.kind, target);
    let content = div()
        .flex()
        .flex_col()
        .gap(SPACE_4)
        .child(
            div()
                .text_size(TEXT_SMALL)
                .text_color(palette.fg2)
                .child("What the cluster would store, compared with the live object. Nothing is written until you apply."),
        )
        .child(diff("edit-review-diff", title, lines).sides(LIVE_SIDE, DRY_RUN_SIDE));
    let header = SectionHeader::new("Server-side dry run").count(changes(lines));
    scrolled("edit-review-view", header, content)
}

fn conflicted(conflict: &Conflict, target: &ResourceRef, palette: &Palette) -> AnyElement {
    let sentence = format!(
        "{} changed on the cluster since you opened it (resourceVersion is now {}). Rebase keeps your edits on top of the new version; Discard reloads it.",
        target.name, conflict.live.resource_version
    );
    let contested = (!conflict.rebased.contested.is_empty()).then(|| {
        format!(
            "Both you and the cluster changed {}; the rebase keeps your value.",
            conflict.rebased.contested.join(", ")
        )
    });
    let title = format!("{}/{}", target.kind, target);
    let banner = div()
        .debug_selector(|| "edit-conflict".to_owned())
        .flex_none()
        .px(SPACE_8)
        .py(SPACE_4)
        .border_b(RULE)
        .border_color(palette.line)
        .bg(palette.warnbg)
        .text_size(TEXT_MONO)
        .text_color(palette.fg)
        .child(sentence);
    let content = div()
        .flex()
        .flex_col()
        .gap(SPACE_4)
        .children(contested.map(|text| {
            div()
                .text_size(TEXT_SMALL)
                .text_color(palette.warn)
                .child(text)
        }))
        .child(diff("edit-conflict-diff", title, &conflict.changed).sides(OPENED_SIDE, NOW_SIDE));
    let header =
        SectionHeader::new("What changed on the cluster").count(changes(&conflict.changed));
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(banner)
        .child(scrolled("edit-conflict-view", header, content))
        .into_any_element()
}

impl Render for EditScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(event) = self.pending.take() {
            self.receive(event, window, cx);
        }
        let palette = *cx.palette();
        if self.palette != Some(palette) {
            self.palette = Some(palette);
            self.editor
                .update(cx, |editor, cx| highlight_yaml(editor, palette, cx));
        }
        let header = self.actions(cx).into_iter().fold(
            PageHeader::new(SharedString::from(self.target.name.clone()))
                .leading(self.navigation.clone())
                .mono(),
            PageHeader::action,
        );
        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(palette.editor)
            .on_action(cx.listener(|this, _: &Submit, window, cx| this.submit(window, cx)))
            .capture_action(cx.listener(|this, enter: &TextFieldEnter, window, cx| {
                if enter.secondary {
                    cx.stop_propagation();
                    this.submit(window, cx);
                }
            }))
            .child(header)
            .child(MetaLine::new(self.meta()))
            .child(self.body(&palette, cx))
    }
}

pub fn edit_command(target: ResourceRef) -> Command {
    Command::NewTab(Box::new(Command::EditYaml(target)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(change: LineChange) -> PatchLine {
        PatchLine {
            change,
            number: 1,
            text: String::new(),
            note: None,
        }
    }

    #[test]
    fn the_review_counts_changed_lines_without_hunk_headers() {
        let one = [
            line(LineChange::Hunk),
            line(LineChange::Context),
            line(LineChange::Added),
        ];
        assert_eq!(changes(&one), "1 line");
        let two = [line(LineChange::Removed), line(LineChange::Added)];
        assert_eq!(changes(&two), "2 lines");
    }
}
