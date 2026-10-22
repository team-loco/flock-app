use flock_core::pods::{ContainerState, Pod, Termination};
use flock_core::{
    Command, ExecEvent, ExecRequest, ExecTarget, LogInstance, LogRequest, ResourceRef, Sender,
    Sink, Source, Status, TermSize, time,
};
use gpui::{
    AnyElement, App, Context, Focusable, InteractiveElement, IntoElement, ParentElement, Styled,
    Subscription, Task, Window, div,
};

use super::{CONTAINER_SELECT, PodsScreen};
use crate::kit::{
    Button, Dock, DockState, DockTab, EmptyState, LogView, MenuItem, Terminal, TerminalEvent,
    TextField, Toast, confirm, toast,
};
use crate::screens::no_shell::{DEBUG_IMAGE, confirm_debug, no_shell_state, start_debug_button};
use crate::screens::workload::kit_line;
use crate::theme::{ActiveTheme, FONT_MONO, RULE, SPACE_2, SPACE_3, SPACE_6, TEXT_SMALL};

pub const TERMINAL_ID: &str = "pod-terminal";
const LOGS_TAB: usize = 0;
const TERMINAL_TAB: usize = 1;
const LOGS_LABEL: &str = "Logs";
const TERMINAL_LABEL: &str = "Terminal";
const SEPARATOR: &str = " · ";
const PREVIOUS_INSTANCE: &str = "previous instance";
const PREVIOUS_BANNER: &str = "pod-logs-previous-banner";
const PREVIOUS_ITEM: &str = "Previous container logs";
const DOWNLOAD_ITEM: &str = "Download logs";
const CONTAINER_LABEL: &str = "pod-container-label";
const SELECT_FOR_LOGS: &str = "Select a pod to see its logs.";
const SELECT_FOR_SHELL: &str = "Select a pod to open a shell.";
const DOCK_EMPTY: &str = "pods-dock-empty";
const DEBUG: &str = "debug container";
const DOWNLOADS: &str = "Downloads";
const SAVE_FAILED: &str = "Could not save the logs";
const SAVED: &str = "Saved logs";
const NO_SHELL_ID: &str = "pod-no-shell";
const START_DEBUG_ID: &str = "pod-start-debug";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogKey {
    pod: ResourceRef,
    container: String,
    instance: LogInstance,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellKey {
    pod: ResourceRef,
    container: String,
}

pub struct LogSession {
    key: LogKey,
    _task: Task<()>,
}

pub struct ShellSession {
    key: ShellKey,
    debug: bool,
    no_shell: bool,
    input: Sender<Vec<u8>>,
    resize: Sender<TermSize>,
    open: bool,
    _task: Task<()>,
    _input: Subscription,
}

pub fn needs_debug(pod: &Pod, container: &str) -> bool {
    pod.containers
        .iter()
        .find(|c| c.name == container)
        .is_some_and(|c| c.status == Status::Err)
}

pub fn debug_reason(container: &str, other: Option<&str>) -> String {
    let debug = format!("start a debug container that shares {container}’s process namespace");
    match other {
        Some(other) => format!("{container} is restarting. Attach to {other} instead, or {debug}."),
        None => format!("{container} is restarting, so Flock will {debug}."),
    }
}

pub fn log_file_name(pod: &str, container: &str) -> String {
    format!("{pod}-{container}.log")
}

pub fn previous_banner(last: &Termination) -> String {
    let terminated = last
        .ago
        .map(|ago| format!("terminated {} ago", time::age(ago)));
    let reason = Some(last.reason.clone()).filter(|reason| !reason.is_empty());
    let exit = format!("exit {}", last.exit_code);
    std::iter::once(PREVIOUS_INSTANCE.to_owned())
        .chain(terminated)
        .chain(reason)
        .chain(std::iter::once(exit))
        .collect::<Vec<_>>()
        .join(SEPARATOR)
}

pub fn exit_note(code: Option<i32>) -> String {
    match code {
        Some(code) => format!("Session ended (exit {code})."),
        None => "Session ended.".to_owned(),
    }
}

fn close_with(
    cx: &Context<PodsScreen>,
    close: fn(&mut PodsScreen, &mut Context<PodsScreen>),
) -> impl Fn(&mut Window, &mut App) + 'static {
    let screen = cx.weak_entity();
    move |_, cx| {
        screen.update(cx, close).ok();
    }
}

impl PodsScreen {
    fn container_name(&self) -> Option<String> {
        self.selected_container().map(|c| c.name.clone())
    }

    fn previous_termination(&self) -> Option<&Termination> {
        self.selected_container()?.last_termination.as_ref()
    }

    fn log_instance(&self) -> LogInstance {
        match self.previous_termination() {
            Some(_) => self.instance,
            None => LogInstance::Current,
        }
    }

    pub(super) fn show_previous_logs(&mut self, container: String, cx: &mut Context<Self>) {
        self.pick_container(Some(container));
        self.instance = LogInstance::Previous;
        self.sync_selection(cx);
        self.show_logs(cx);
    }

    fn set_instance(&mut self, instance: LogInstance, cx: &mut Context<Self>) {
        self.instance = instance;
        self.sync_streams(cx);
        cx.notify();
    }

    fn log_key(&self) -> Option<LogKey> {
        let pod = self.selected_pod().map(Pod::resource)?;
        let container = self.container_name()?;
        Some(LogKey {
            pod,
            container,
            instance: self.log_instance(),
        })
    }

    fn follow_logs(&mut self, cx: &mut Context<Self>) {
        let key = self.log_key();
        if self.log.as_ref().map(|s| &s.key) == key.as_ref() {
            return;
        }
        self.log = None;
        self.logs.update(cx, LogView::clear);
        if let Some(key) = key {
            self.open_log(key, cx);
        }
    }

    fn close_logs_tab(&mut self, cx: &mut Context<Self>) {
        self.log = None;
        self.logs.update(cx, |logs, cx| {
            logs.clear(cx);
            logs.set_query("", cx);
        });
        self.clear_search = true;
        self.leave_closed_tab(TERMINAL_TAB, self.shell.is_some(), cx);
    }

    fn close_terminal_tab(&mut self, cx: &mut Context<Self>) {
        self.terminal.update(cx, Terminal::clear);
        self.close_shell(cx);
        self.leave_closed_tab(LOGS_TAB, self.log.is_some(), cx);
    }

    fn leave_closed_tab(&mut self, other: usize, other_open: bool, cx: &mut Context<Self>) {
        self.dock.update(cx, |dock, cx| {
            if other_open {
                dock.select(other, cx);
            } else {
                dock.close(cx);
            }
        });
        cx.notify();
    }

    pub(super) fn reset_dock(&mut self, cx: &mut Context<Self>) {
        self.log = None;
        self.logs.update(cx, |logs, cx| {
            logs.clear(cx);
            logs.set_query("", cx);
        });
        self.clear_search = true;
        self.terminal.update(cx, Terminal::clear);
        self.close_shell(cx);
        self.dock.update(cx, DockState::close);
    }

    pub(super) fn sync_streams(&mut self, cx: &mut Context<Self>) {
        let live = self
            .log
            .as_ref()
            .map(|s| &s.key.pod)
            .or(self.shell.as_ref().map(|s| &s.key.pod));
        let Some(live) = live else {
            return;
        };
        let pod = self.selected_pod().map(Pod::resource);
        if Some(live) != pod.as_ref() || !self.dock.read(cx).is_open() {
            self.reset_dock(cx);
            return;
        }
        if self.log.is_some() {
            self.follow_logs(cx);
        }
        let shell_key = self
            .selected_pod()
            .map(Pod::resource)
            .zip(self.container_name())
            .map(|(pod, container)| ShellKey { pod, container });
        let moved = self
            .shell
            .as_ref()
            .is_some_and(|s| Some(&s.key) != shell_key.as_ref());
        if moved {
            self.terminal.update(cx, Terminal::clear);
            self.close_shell(cx);
        }
    }

    fn open_log(&mut self, key: LogKey, cx: &mut Context<Self>) {
        let (sink, lines) = Sink::channel();
        let follow = true;
        self.logs.update(cx, |logs, cx| logs.set_follow(follow, cx));
        cx.emit(Command::StreamLogs(LogRequest {
            pod: key.pod.clone(),
            container: key.container.clone(),
            instance: key.instance,
            sink,
        }));
        let task = cx.spawn(async move |this, cx| {
            while let Ok(line) = lines.recv().await {
                let pushed = this.update(cx, |screen, cx| {
                    let line = kit_line(line).with_fields();
                    screen.logs.update(cx, |logs, cx| logs.push(line, cx));
                });
                if pushed.is_err() {
                    break;
                }
            }
        });
        self.log = Some(LogSession { key, _task: task });
    }

    fn open_shell(&mut self, key: ShellKey, force_debug: bool, cx: &mut Context<Self>) {
        let debug = force_debug
            || self
                .selected_pod()
                .is_some_and(|pod| needs_debug(pod, &key.container));
        let target = ExecTarget {
            pod: key.pod.clone(),
            container: key.container.clone(),
            debug_image: debug.then(|| DEBUG_IMAGE.to_owned()),
        };
        let terminal = self.terminal.clone();
        let command_line = target.command_line();
        let other = self.selected_pod().and_then(|pod| {
            pod.containers
                .iter()
                .find(|c| c.name != key.container && c.state == ContainerState::Running)
                .map(|c| c.name.clone())
        });
        let reason =
            (debug && !force_debug).then(|| debug_reason(&key.container, other.as_deref()));
        terminal.update(cx, |terminal, cx| {
            terminal.clear(cx);
            if let Some(reason) = reason {
                terminal.note(reason, cx);
                terminal.note(String::new(), cx);
            }
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
        cx.emit(Command::Exec(ExecRequest {
            target,
            input: source,
            resize: sizes,
            output,
        }));
        let task = cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                let applied = this.update(cx, |screen, cx| screen.exec_event(event, cx));
                if applied.is_err() {
                    break;
                }
            }
        });
        self.shell = Some(ShellSession {
            key,
            debug,
            no_shell: false,
            input,
            resize,
            open: true,
            _task: task,
            _input: forward,
        });
    }

    fn exec_event(&mut self, event: ExecEvent, cx: &mut Context<Self>) {
        match event {
            ExecEvent::Started { note } => {
                self.terminal.update(cx, |t, cx| t.note(note, cx));
            }
            ExecEvent::Output(bytes) => {
                self.terminal.update(cx, |t, cx| t.feed(&bytes, cx));
            }
            ExecEvent::NoShell => {
                if let Some(session) = self.shell.as_mut() {
                    session.no_shell = true;
                }
                cx.notify();
            }
            ExecEvent::Failed(message) => {
                self.terminal.update(cx, |t, cx| t.line(message, cx));
                self.exec_event(ExecEvent::Exited { code: None }, cx);
            }
            ExecEvent::Exited { code } => {
                let no_shell = self.shell.as_ref().is_some_and(|s| s.no_shell);
                self.terminal.update(cx, |t, cx| {
                    if !no_shell {
                        t.note(exit_note(code), cx);
                    }
                    t.set_open(false, cx);
                });
                if let Some(session) = self.shell.as_mut() {
                    session.open = false;
                }
                cx.notify();
            }
        }
    }

    fn close_shell(&mut self, cx: &mut Context<Self>) {
        self.shell = None;
        self.terminal
            .update(cx, |terminal, cx| terminal.set_open(false, cx));
        cx.notify();
    }

    fn start_shell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self
            .selected_pod()
            .map(Pod::resource)
            .zip(self.container_name());
        if let Some((pod, container)) = target {
            self.open_shell(ShellKey { pod, container }, false, cx);
            let focus = self.terminal.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
            cx.notify();
        }
    }

    fn confirm_debug_container(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.shell.as_ref().map(|session| session.key.clone()) else {
            return;
        };
        let screen = cx.weak_entity();
        let start = key.clone();
        let dialog = confirm_debug(&key.pod.name, &key.container, move |window, cx| {
            let key = start.clone();
            let _ = screen.update(cx, |this, cx| {
                this.open_shell(key, true, cx);
                let focus = this.terminal.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
                cx.notify();
            });
        });
        confirm(dialog, window, cx);
    }

    fn download_logs(&self, cx: &mut Context<Self>) {
        let text = self.logs.read(cx).export();
        let name = match (self.selected_pod(), self.container_name()) {
            (Some(pod), Some(container)) => log_file_name(&pod.name, &container),
            (Some(pod), None) => log_file_name(&pod.name, LOGS_LABEL),
            (None, _) => return,
        };
        let directory = std::env::home_dir().unwrap_or_default().join(DOWNLOADS);
        let chosen = cx.prompt_for_new_path(&directory, Some(&name));
        cx.spawn(async move |_, cx| {
            let Ok(Ok(Some(path))) = chosen.await else {
                return;
            };
            let detail = path.display().to_string();
            let result = std::fs::write(&path, text);
            cx.update(|cx| {
                let message = match result {
                    Ok(()) => Toast::new(Status::Ok, SAVED).detail(detail),
                    Err(error) => Toast::new(Status::Err, SAVE_FAILED).detail(error.to_string()),
                };
                toast(message, cx);
            });
        })
        .detach();
    }

    pub fn open_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dock
            .update(cx, |dock, cx| dock.select(TERMINAL_TAB, cx));
        let focus = self.terminal.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        if !self.shell.as_ref().is_some_and(|session| session.open) {
            self.start_shell(window, cx);
        }
    }

    pub(super) fn show_logs(&mut self, cx: &mut Context<Self>) {
        self.dock.update(cx, |dock, cx| dock.select(LOGS_TAB, cx));
        self.follow_logs(cx);
        cx.notify();
    }

    fn container_picker(&self) -> AnyElement {
        let single = self
            .selected_pod()
            .is_some_and(|pod| pod.containers.len() < 2);
        if !single {
            return self.containers.clone().into_any_element();
        }
        let name = self.container_name().unwrap_or_default();
        div()
            .debug_selector(|| CONTAINER_LABEL.to_owned())
            .flex_none()
            .w(CONTAINER_SELECT)
            .truncate()
            .font_family(FONT_MONO)
            .text_size(TEXT_SMALL)
            .child(name)
            .into_any_element()
    }

    fn log_tools(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let previous = self.log_instance() == LogInstance::Previous;
        let mut items = LogView::option_items(&self.logs, true, cx);
        if self.previous_termination().is_some() {
            let screen = cx.weak_entity();
            items.push(
                MenuItem::new(PREVIOUS_ITEM, move |_, cx| {
                    let instance = if previous {
                        LogInstance::Current
                    } else {
                        LogInstance::Previous
                    };
                    screen
                        .update(cx, |this, cx| this.set_instance(instance, cx))
                        .ok();
                })
                .checked(previous),
            );
        }
        let screen = cx.weak_entity();
        items.push(MenuItem::new(DOWNLOAD_ITEM, move |_, cx| {
            screen.update(cx, |this, cx| this.download_logs(cx)).ok();
        }));
        self.options
            .update(cx, |menu, cx| menu.set_items(items, cx));
        div()
            .flex()
            .items_center()
            .gap(SPACE_3)
            .child(self.container_picker())
            .child(TextField::search(&self.search))
            .child(self.options.clone())
    }

    fn terminal_tools(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let open = self.shell.as_ref().is_some_and(|session| session.open);
        let button = if open {
            Button::new("pod-close-shell", "Close shell")
                .small()
                .on_click(cx.listener(|this, _, _, cx| this.close_terminal_tab(cx)))
        } else {
            Button::new("pod-open-shell", "Open shell")
                .small()
                .disabled(self.selected_container().is_none())
                .on_click(cx.listener(|this, _, window, cx| this.start_shell(window, cx)))
        };
        div()
            .flex()
            .items_center()
            .gap(SPACE_3)
            .child(self.containers.clone())
            .child(button)
            .child(
                Button::new("pod-copy-terminal", "Copy").small().on_click(
                    cx.listener(|this, _, _, cx| this.terminal.update(cx, Terminal::copy)),
                ),
            )
    }

    pub(super) fn dock_view(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let detail = match (self.selected_pod(), self.container_name()) {
            (Some(pod), Some(container)) => format!("{} · {container}", pod.short_name()),
            (Some(pod), None) => pod.short_name().to_owned(),
            (None, _) => String::new(),
        };
        let terminal_detail = if self.shell.as_ref().is_some_and(|s| s.debug) {
            DEBUG.to_owned()
        } else {
            detail.clone()
        };
        if self.selected_pod().is_none() {
            return Dock::new(&self.dock)
                .tab(
                    DockTab::new(LOGS_LABEL)
                        .content(EmptyState::new(SELECT_FOR_LOGS).selector(DOCK_EMPTY)),
                )
                .tab(
                    DockTab::new(TERMINAL_LABEL)
                        .content(EmptyState::new(SELECT_FOR_SHELL).selector(DOCK_EMPTY)),
                );
        }
        Dock::new(&self.dock)
            .tab(
                DockTab::new(LOGS_LABEL)
                    .hidden(self.log.is_none())
                    .closable(close_with(cx, Self::close_logs_tab))
                    .detail(detail.clone())
                    .tools(self.log_tools(cx))
                    .content(self.log_content(cx)),
            )
            .tab(
                DockTab::new(TERMINAL_LABEL)
                    .hidden(self.shell.is_none())
                    .closable(close_with(cx, Self::close_terminal_tab))
                    .detail(terminal_detail)
                    .tools(self.terminal_tools(cx))
                    .content(self.terminal_content(cx)),
            )
    }

    fn log_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let banner = self
            .previous_termination()
            .filter(|_| self.log_instance() == LogInstance::Previous)
            .map(previous_banner);
        let Some(banner) = banner else {
            return self.logs.clone().into_any_element();
        };
        let palette = *cx.palette();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .debug_selector(|| PREVIOUS_BANNER.to_owned())
                    .flex_none()
                    .px(SPACE_6)
                    .py(SPACE_2)
                    .border_b(RULE)
                    .border_color(palette.line)
                    .bg(palette.warnbg)
                    .text_color(palette.warn)
                    .font_family(FONT_MONO)
                    .text_size(TEXT_SMALL)
                    .child(banner),
            )
            .child(div().flex_1().min_h_0().child(self.logs.clone()))
            .into_any_element()
    }

    fn terminal_content(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.shell.as_ref().is_some_and(|s| s.no_shell) {
            return self.terminal.clone().into_any_element();
        }
        let start = start_debug_button(START_DEBUG_ID)
            .on_click(cx.listener(|this, _, window, cx| this.confirm_debug_container(window, cx)));
        no_shell_state(NO_SHELL_ID, start).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use flock_core::pods::Container;

    use super::*;

    fn pod(status: Status, restarts: u32) -> Pod {
        Pod {
            name: "checkout-api-7f9c6d-x2lqp".into(),
            containers: vec![Container {
                name: "api".into(),
                state: ContainerState::Waiting,
                status,
                restarts,
                ..Container::default()
            }],
            ..Pod::default()
        }
    }

    #[test]
    fn crashing_containers_get_a_debug_container() {
        assert!(needs_debug(&pod(Status::Err, 14), "api"));
        assert!(!needs_debug(&pod(Status::Warn, 0), "api"));
        assert!(!needs_debug(&pod(Status::Err, 14), "envoy"));
    }

    #[test]
    fn debug_sessions_explain_why_and_logs_save_per_container() {
        assert_eq!(
            debug_reason("api", Some("envoy")),
            "api is restarting. Attach to envoy instead, or start a debug container that shares api’s process namespace."
        );
        assert_eq!(
            debug_reason("api", None),
            "api is restarting, so Flock will start a debug container that shares api’s process namespace."
        );
        assert_eq!(
            log_file_name("checkout-api-7f9c6d-x2lqp", "api"),
            "checkout-api-7f9c6d-x2lqp-api.log"
        );
    }

    #[test]
    fn previous_banners_say_when_and_how_the_instance_ended() {
        let last = Termination {
            reason: "Error".into(),
            exit_code: 1,
            ago: Some(std::time::Duration::from_hours(15)),
            ..Termination::default()
        };
        assert_eq!(
            previous_banner(&last),
            "previous instance · terminated 15h ago · Error · exit 1"
        );
        let unknown = Termination {
            exit_code: 137,
            ..Termination::default()
        };
        assert_eq!(previous_banner(&unknown), "previous instance · exit 137");
    }

    #[test]
    fn exit_notes_include_the_code_when_known() {
        assert_eq!(exit_note(Some(1)), "Session ended (exit 1).");
        assert_eq!(exit_note(None), "Session ended.");
    }
}
