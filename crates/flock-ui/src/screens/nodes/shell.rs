use flock_core::{
    Command, ExecEvent, NodeShellRequest, ResourceRef, Sender, Sink, Source, TermSize,
    node_shell_command_line,
};
use gpui::{
    AnyElement, App, Context, Focusable, InteractiveElement, IntoElement, ParentElement, Styled,
    Subscription, Task, WeakEntity, Window, div,
};

use super::NodesScreen;
use crate::kit::{
    Button, ConfirmDialog, Dock, DockTab, EmptyState, RawMessage, Terminal, TerminalEvent, confirm,
    drain,
};
use crate::screens::ScreenEvent;
use crate::screens::no_shell::DEBUG_IMAGE;
use crate::screens::pods::exit_note;
use crate::theme::{ActiveTheme, SPACE_3, SPACE_6};

pub const SHELL_IMAGE: &str = DEBUG_IMAGE;
pub const TERMINAL_ID: &str = "node-terminal";
pub const SHELL_LABEL: &str = "Shell";
const TERMINAL_LABEL: &str = "Terminal";
const NO_SHELL: &str = "No shell open";
const DOCK_EMPTY: &str = "node-dock-empty";
const FAILURE_ID: &str = "node-shell-failure";
const CONFIRM_TITLE: &str = "Open node shell";
const CONFIRM_LABEL: &str = "Open shell";

pub struct NodeShell {
    node: ResourceRef,
    open: bool,
    failure: Option<String>,
    input: Sender<Vec<u8>>,
    resize: Sender<TermSize>,
    _task: Task<()>,
    _input: Subscription,
}

pub fn confirm_body(node: &str, image: &str) -> String {
    format!("Runs a privileged {image} pod on {node}. The pod is deleted when the shell closes.")
}

pub fn ask_shell(
    node: ResourceRef,
    screen: WeakEntity<NodesScreen>,
    window: &mut Window,
    cx: &mut App,
) {
    let body = confirm_body(&node.name, SHELL_IMAGE);
    let detail = format!("{} · {}", node.cluster, node.name);
    let dialog = ConfirmDialog::new(CONFIRM_TITLE, body, CONFIRM_LABEL, move |window, cx| {
        let node = node.clone();
        let _ = screen.update(cx, |this, cx| this.open_shell(node, window, cx));
    })
    .detail(detail)
    .primary();
    confirm(dialog, window, cx);
}

impl NodesScreen {
    pub fn open_shell(&mut self, node: ResourceRef, window: &mut Window, cx: &mut Context<Self>) {
        let terminal = self.terminal.clone();
        let command_line = node_shell_command_line(&node.name, SHELL_IMAGE);
        terminal.update(cx, |terminal, cx| {
            terminal.clear(cx);
            terminal.note(command_line, cx);
            terminal.set_open(true, cx);
        });
        let forward = cx.subscribe(&terminal, |this, _, event: &TerminalEvent, _| {
            let Some(session) = this.shell.as_ref().filter(|s| s.open) else {
                return;
            };
            match event {
                TerminalEvent::Input(bytes) => {
                    session.input.try_send(bytes.clone()).ok();
                }
                TerminalEvent::Resize(size) => {
                    session.resize.try_send(*size).ok();
                }
            }
        });
        let (input, source) = Source::channel();
        let (resize, sizes) = Source::channel();
        if let Some(size) = terminal.read(cx).size() {
            resize.try_send(size).ok();
        }
        let (output, events) = Sink::channel();
        let request = NodeShellRequest {
            node: node.clone(),
            image: SHELL_IMAGE.to_owned(),
            input: source,
            resize: sizes,
            output,
        };
        cx.emit(ScreenEvent::Command(Command::NodeShell(request)));
        let task = cx.spawn(async move |this, cx| {
            drain(events, this, cx, |screen: &mut Self, batch, cx| {
                for event in batch {
                    screen.exec_event(event, cx);
                }
            })
            .await;
        });
        self.shell = Some(NodeShell {
            node,
            open: true,
            failure: None,
            input,
            resize,
            _task: task,
            _input: forward,
        });
        self.dock.update(cx, |dock, cx| dock.select(0, cx));
        let focus = terminal.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn exec_event(&mut self, event: ExecEvent, cx: &mut Context<Self>) {
        match event {
            ExecEvent::Started { note } => {
                self.terminal.update(cx, |t, cx| t.note(note, cx));
            }
            ExecEvent::Output(bytes) => {
                self.terminal.update(cx, |t, cx| t.feed(&bytes, cx));
            }
            ExecEvent::NoShell => {}
            ExecEvent::Failed(message) => {
                self.end_shell(cx);
                if let Some(session) = self.shell.as_mut() {
                    session.failure = Some(message);
                }
            }
            ExecEvent::Exited { code } => {
                self.terminal
                    .update(cx, |t, cx| t.note(exit_note(code), cx));
                self.end_shell(cx);
            }
        }
    }

    fn end_shell(&mut self, cx: &mut Context<Self>) {
        self.terminal.update(cx, |t, cx| t.set_open(false, cx));
        if let Some(session) = self.shell.as_mut() {
            session.open = false;
        }
        cx.notify();
    }

    pub(super) fn close_shell(&mut self, cx: &mut Context<Self>) {
        if self.shell.is_none() {
            return;
        }
        self.shell = None;
        self.terminal.update(cx, |t, cx| t.set_open(false, cx));
        cx.notify();
    }

    fn terminal_tools(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let open = self.shell.as_ref().is_some_and(|session| session.open);
        div()
            .flex()
            .items_center()
            .gap(SPACE_3)
            .child(
                Button::new("node-close-shell", "Close shell")
                    .small()
                    .disabled(!open)
                    .on_click(cx.listener(|this, _, _, cx| this.close_shell(cx))),
            )
            .child(
                Button::new("node-copy-terminal", "Copy").small().on_click(
                    cx.listener(|this, _, _, cx| this.terminal.update(cx, Terminal::copy)),
                ),
            )
    }

    fn terminal_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let failure = self
            .shell
            .as_ref()
            .and_then(|session| session.failure.clone());
        let Some(failure) = failure else {
            return Terminal::pane(&self.terminal).into_any_element();
        };
        let palette = *cx.palette();
        div()
            .debug_selector(|| FAILURE_ID.to_owned())
            .size_full()
            .px(SPACE_6)
            .py(SPACE_3)
            .child(RawMessage::new(failure).color(palette.err))
            .into_any_element()
    }

    pub(super) fn dock_view(&self, cx: &mut Context<Self>) -> Dock {
        let tab = DockTab::new(TERMINAL_LABEL);
        let Some(session) = self.shell.as_ref() else {
            let empty = EmptyState::new(NO_SHELL).selector(DOCK_EMPTY);
            return Dock::new(&self.dock).tab(tab.content(empty));
        };
        let tab = tab
            .detail(session.node.name.clone())
            .tools(self.terminal_tools(cx))
            .content(self.terminal_content(cx));
        Dock::new(&self.dock).tab(tab)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_names_the_node_and_the_image() {
        assert_eq!(
            confirm_body("ip-10-4-12-87", "busybox:1.36"),
            "Runs a privileged busybox:1.36 pod on ip-10-4-12-87. The pod is deleted when the shell closes."
        );
    }
}
