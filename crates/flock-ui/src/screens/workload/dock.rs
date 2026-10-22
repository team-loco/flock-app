use flock_core::pods::PodEvent;
use flock_core::workload::Pod;
use flock_core::{
    Command, ExecEvent, ExecRequest, ExecTarget, LogLevel, LogRequest, LogTarget, ResourceRef,
    Sink, Source, TermSize, Workload,
};
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, Focusable, InteractiveElement, IntoElement,
    ParentElement, StatefulInteractiveElement, Styled, Subscription, Window, div,
};

use super::{WorkloadScreen, logs_tab_label};
use crate::kit::confirm;
use crate::kit::{
    Button, Dock, DockState, DockTab, EventList, InputEvent, Level, LogLine, LogView, MenuItem,
    Segmented, Terminal, TerminalEvent, TextField, TextFieldState,
};
use crate::screens::multilog::MultiLogs;
use crate::screens::no_shell::{DEBUG_IMAGE, confirm_debug, no_shell_state, start_debug_button};
use crate::theme::{ActiveTheme, SPACE_3, SPACE_4, SPACE_6};

pub const LOGS: usize = 0;
pub const EVENTS: usize = 1;
pub const TERMINAL: usize = 2;

const LOGS_LABEL: &str = "Logs";
const EVENTS_LABEL: &str = "Events";
const TERMINAL_LABEL: &str = "Terminal";
const DEBUG: &str = "debug container";
const EXEC: &str = "shell";
const OPEN_IN_TAB: &str = "Open logs in tab";
const ALL_PODS_ITEM: &str = "All pods";
const ALL_PODS_DETAIL: &str = "all pods";
const NO_SHELL_ID: &str = "workload-no-shell";
const START_DEBUG_ID: &str = "workload-start-debug";
const POD_KIND: &str = "Pod";

const EVENTS_PANE: &str = "workload-events";

pub struct ExecSession {
    input: async_channel::Sender<Vec<u8>>,
    resize: async_channel::Sender<TermSize>,
    target: ExecTarget,
    debug: bool,
    no_shell: bool,
    open: bool,
    _task: gpui::Task<()>,
}

pub fn subscribe(
    dock: &Entity<DockState>,
    filter: &Entity<TextFieldState>,
    terminal: &Entity<Terminal>,
    cx: &mut Context<WorkloadScreen>,
) -> Vec<Subscription> {
    vec![
        cx.observe(dock, |this, dock, cx| {
            if !dock.read(cx).is_open() {
                this.close_shell(cx);
            }
            cx.notify();
        }),
        cx.subscribe(filter, |this, filter, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                let query = filter.read(cx).value();
                this.logs.update(cx, |logs, cx| logs.set_query(query, cx));
            }
        }),
        cx.subscribe(terminal, |this, _, event: &TerminalEvent, _| {
            let Some(session) = this.exec.as_ref().filter(|session| session.open) else {
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
        }),
    ]
}

fn close_with(
    cx: &Context<WorkloadScreen>,
    close: fn(&mut WorkloadScreen, &mut Context<WorkloadScreen>),
) -> impl Fn(&mut Window, &mut App) + 'static {
    let screen = cx.weak_entity();
    move |_, cx| {
        screen.update(cx, close).ok();
    }
}

pub fn kit_line(line: flock_core::LogLine) -> LogLine {
    let level = line.level.map(kit_level);
    LogLine::new(line.time.as_deref(), level, line.message)
}

fn kit_level(level: LogLevel) -> Level {
    match level {
        LogLevel::Debug => Level::Debug,
        LogLevel::Info => Level::Info,
        LogLevel::Warn => Level::Warn,
        LogLevel::Error => Level::Error,
        LogLevel::Fatal => Level::Fatal,
    }
}

fn pod_ref(workload: &ResourceRef, pod: &str) -> ResourceRef {
    ResourceRef {
        cluster: workload.cluster.clone(),
        kind: POD_KIND.into(),
        namespace: workload.namespace.clone(),
        name: pod.into(),
    }
}

pub fn needs_debug(pod: &Pod, container: &str) -> bool {
    !pod.container(container).is_some_and(|c| c.ready)
}

impl WorkloadScreen {
    fn container_name(&self) -> Option<String> {
        self.selected_container().map(|spec| spec.name.clone())
    }

    pub(super) fn follow_logs(&mut self, cx: &mut Context<Self>) {
        let (Some(pod), Some(container)) = (self.selected_pod().cloned(), self.container_name())
        else {
            return;
        };
        self.request_logs(&pod, container, cx);
    }

    fn request_logs(&mut self, pod: &Pod, container: String, cx: &mut Context<Self>) {
        let (sink, lines) = Sink::channel();
        self.logs.update(cx, LogView::clear);
        cx.emit(Command::StreamLogs(LogRequest {
            pod: pod_ref(&self.target, &pod.name),
            container,
            instance: flock_core::LogInstance::Current,
            sink,
        }));
        self.log_task = Some(cx.spawn(async move |this, cx| {
            while let Ok(line) = lines.recv().await {
                let pushed = this.update(cx, |screen, cx| {
                    let line = kit_line(line);
                    screen.logs.update(cx, |logs, cx| logs.push(line, cx));
                });
                if pushed.is_err() {
                    break;
                }
            }
        }));
    }

    pub(super) fn show_logs(&mut self, cx: &mut Context<Self>) {
        self.dock.update(cx, |dock, cx| dock.select(LOGS, cx));
        if self.log_task.is_none() {
            self.follow_logs(cx);
        }
    }

    fn close_logs_tab(&mut self, cx: &mut Context<Self>) {
        self.log_task = None;
        self.all_pods = None;
        self.logs.update(cx, |logs, cx| {
            logs.clear(cx);
            logs.set_query("", cx);
        });
        self.clear_filter = true;
        self.leave_closed_tab(cx);
    }

    fn close_terminal_tab(&mut self, cx: &mut Context<Self>) {
        self.discard_shell(cx);
        self.leave_closed_tab(cx);
    }

    fn leave_closed_tab(&mut self, cx: &mut Context<Self>) {
        self.dock.update(cx, |dock, cx| dock.select(EVENTS, cx));
        cx.notify();
    }

    pub(super) fn reset_dock(&mut self, cx: &mut Context<Self>) {
        self.discard_shell(cx);
        self.log_task = None;
        self.logs.update(cx, |logs, cx| {
            logs.clear(cx);
            logs.set_query("", cx);
        });
        self.clear_filter = true;
        self.dock.update(cx, |dock, cx| dock.close_on(EVENTS, cx));
    }

    pub(super) fn open_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dock.update(cx, |dock, cx| dock.select(TERMINAL, cx));
        self.start_shell(window, cx);
    }

    pub(super) fn close_shell(&mut self, cx: &mut Context<Self>) {
        self.exec = None;
        self.terminal
            .update(cx, |terminal, cx| terminal.set_open(false, cx));
    }

    pub(super) fn discard_shell(&mut self, cx: &mut Context<Self>) {
        self.close_shell(cx);
        self.terminal.update(cx, Terminal::clear);
    }

    fn start_shell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_exec(false, cx);
        let focus = self.terminal.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn confirm_debug_container(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.exec.as_ref().map(|session| session.target.clone()) else {
            return;
        };
        let screen = cx.weak_entity();
        let dialog = confirm_debug(&target.pod.name, &target.container, move |window, cx| {
            let _ = screen.update(cx, |this, cx| {
                this.ensure_exec(true, cx);
                let focus = this.terminal.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
                cx.notify();
            });
        });
        confirm(dialog, window, cx);
    }

    fn ensure_exec(&mut self, force_debug: bool, cx: &mut Context<Self>) {
        if self.exec.as_ref().is_some_and(|session| session.open) {
            return;
        }
        let (Some(pod), Some(container)) = (self.selected_pod().cloned(), self.container_name())
        else {
            return;
        };
        let debug = force_debug || needs_debug(&pod, &container);
        let target = ExecTarget {
            pod: pod_ref(&self.target, &pod.name),
            container,
            debug_image: debug.then(|| DEBUG_IMAGE.to_owned()),
        };
        let (input, source) = Source::channel();
        let (resize, sizes) = Source::channel();
        if let Some(size) = self.terminal.read(cx).size() {
            resize.try_send(size).ok();
        }
        let (output, events) = Sink::channel();
        let command_line = target.command_line();
        self.terminal.update(cx, |terminal, cx| {
            terminal.clear(cx);
            terminal.note(command_line, cx);
            terminal.set_open(true, cx);
        });
        cx.emit(Command::Exec(ExecRequest {
            target: target.clone(),
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
        self.exec = Some(ExecSession {
            input,
            resize,
            target,
            debug,
            no_shell: false,
            open: true,
            _task: task,
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
                if let Some(session) = self.exec.as_mut() {
                    session.no_shell = true;
                }
                cx.notify();
            }
            ExecEvent::Failed(message) => {
                self.terminal.update(cx, |t, cx| t.line(message, cx));
                self.exec_event(ExecEvent::Exited { code: None }, cx);
            }
            ExecEvent::Exited { code } => {
                let no_shell = self.exec.as_ref().is_some_and(|s| s.no_shell);
                let note = match code {
                    Some(code) => format!("Session ended (exit {code})."),
                    None => "Session ended.".to_owned(),
                };
                self.terminal.update(cx, |t, cx| {
                    if !no_shell {
                        t.note(note, cx);
                    }
                    t.set_open(false, cx);
                });
                if let Some(session) = self.exec.as_mut() {
                    session.open = false;
                }
                cx.notify();
            }
        }
    }

    fn terminal_detail(&self) -> &'static str {
        let debug = if let Some(session) = &self.exec {
            session.debug
        } else {
            let container = self.container_name().unwrap_or_default();
            self.selected_pod()
                .is_some_and(|pod| needs_debug(pod, &container))
        };
        if debug { DEBUG } else { EXEC }
    }

    fn toggle_all_pods(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.all_pods.take().is_some() {
            cx.notify();
            return;
        }
        let target = LogTarget::Workload(self.target.clone());
        let view = cx.new(|cx| MultiLogs::new("workload-pod-logs", target, window, cx));
        let subscription = cx.subscribe(&view, |_, _, command: &Command, cx| {
            cx.emit(command.clone());
        });
        view.update(cx, MultiLogs::watch);
        self.all_pods = Some((view, subscription));
        cx.notify();
    }

    fn all_pods_tools(
        &self,
        view: &Entity<MultiLogs>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let target = LogTarget::Workload(self.target.clone());
        let screen = cx.weak_entity();
        let extra = vec![
            MenuItem::new(ALL_PODS_ITEM, move |window, cx| {
                screen
                    .update(cx, |this, cx| this.toggle_all_pods(window, cx))
                    .ok();
            })
            .checked(true),
            MenuItem::new(OPEN_IN_TAB, {
                let screen = cx.weak_entity();
                move |_, cx| {
                    let open = Command::FollowLogs(target.clone());
                    screen
                        .update(cx, |_, cx| cx.emit(Command::NewTab(Box::new(open.clone()))))
                        .ok();
                }
            }),
        ];
        view.update(cx, |view, cx| view.tools(extra, cx).into_any_element())
    }

    fn log_tools(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mut items = LogView::option_items(&self.logs, false, cx);
        let screen = cx.weak_entity();
        items.push(MenuItem::new(ALL_PODS_ITEM, {
            let screen = screen.clone();
            move |window, cx| {
                screen
                    .update(cx, |this, cx| this.toggle_all_pods(window, cx))
                    .ok();
            }
        }));
        let openable = self.selected_pod().is_some() && self.container_name().is_some();
        items.push(
            MenuItem::new(OPEN_IN_TAB, move |_, cx| {
                screen.update(cx, WorkloadScreen::open_logs_tab).ok();
            })
            .disabled(!openable),
        );
        self.log_options
            .update(cx, |menu, cx| menu.set_items(items, cx));
        div()
            .flex()
            .items_center()
            .gap(SPACE_3)
            .child(TextField::search(&self.filter))
            .child(self.log_options.clone())
    }

    fn open_logs_tab(&mut self, cx: &mut Context<Self>) {
        let (Some(pod), Some(container)) = (self.selected_pod(), self.container_name()) else {
            return;
        };
        let pod = pod_ref(&self.target, &pod.name);
        let open = Command::OpenLogs { pod, container };
        cx.emit(Command::NewTab(Box::new(open)));
    }

    fn terminal_tools(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let open = self.exec.as_ref().is_some_and(|session| session.open);
        let button = if open {
            Button::new("workload-close-shell", "Close shell")
                .small()
                .on_click(cx.listener(|this, _, _, cx| this.close_terminal_tab(cx)))
        } else {
            Button::new("workload-open-shell", "Open shell")
                .small()
                .disabled(self.selected_container().is_none() || self.selected_pod().is_none())
                .on_click(cx.listener(|this, _, window, cx| this.start_shell(window, cx)))
        };
        let names: Vec<String> = self
            .workload
            .as_ref()
            .map(|workload| {
                workload
                    .containers
                    .iter()
                    .map(|container| container.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        let picker = (names.len() > 1).then(|| {
            names
                .into_iter()
                .fold(
                    Segmented::new("terminal-containers")
                        .small()
                        .mono()
                        .selected(self.container),
                    Segmented::option,
                )
                .on_select(cx.listener(|this, ix: &usize, _, cx| this.select_container(*ix, cx)))
        });
        div()
            .flex()
            .items_center()
            .gap(SPACE_3)
            .children(picker)
            .child(button)
            .child(
                Button::new("workload-copy-terminal", "Copy")
                    .small()
                    .on_click(
                        cx.listener(|this, _, _, cx| this.terminal.update(cx, Terminal::copy)),
                    ),
            )
    }

    pub(super) fn render_dock(&self, workload: &Workload, cx: &mut Context<Self>) -> AnyElement {
        let logs_detail = match (&self.pod, self.container_name()) {
            (Some(pod), Some(container)) => logs_tab_label(pod, &container),
            _ => String::new(),
        };
        let events = Self::events_pane(workload.events.clone(), cx);
        let logs_tab = match self.all_pods.as_ref().map(|(view, _)| view.clone()) {
            Some(view) => DockTab::new(LOGS_LABEL)
                .closable(close_with(cx, Self::close_logs_tab))
                .detail(ALL_PODS_DETAIL)
                .tools(self.all_pods_tools(&view, cx).into_any_element())
                .content(view),
            None => DockTab::new(LOGS_LABEL)
                .hidden(self.log_task.is_none())
                .closable(close_with(cx, Self::close_logs_tab))
                .detail(logs_detail)
                .tools(self.log_tools(cx).into_any_element())
                .content(self.logs.clone()),
        };
        Dock::new(&self.dock)
            .tab(logs_tab)
            .tab(
                DockTab::new(EVENTS_LABEL)
                    .detail(workload.events.len().to_string())
                    .content(events),
            )
            .tab(
                DockTab::new(TERMINAL_LABEL)
                    .hidden(self.exec.is_none())
                    .closable(close_with(cx, Self::close_terminal_tab))
                    .detail(self.terminal_detail())
                    .tools(self.terminal_tools(cx))
                    .content(self.terminal_content(cx)),
            )
            .into_any_element()
    }

    fn terminal_content(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.exec.as_ref().is_some_and(|s| s.no_shell) {
            return self.terminal.clone().into_any_element();
        }
        let start = start_debug_button(START_DEBUG_ID)
            .on_click(cx.listener(|this, _, window, cx| this.confirm_debug_container(window, cx)));
        no_shell_state(NO_SHELL_ID, start).into_any_element()
    }

    fn events_pane(events: Vec<PodEvent>, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let palette = *cx.palette();
        div()
            .id(EVENTS_PANE)
            .debug_selector(|| EVENTS_PANE.into())
            .size_full()
            .overflow_y_scroll()
            .px(SPACE_6)
            .py(SPACE_4)
            .bg(palette.editor)
            .child(EventList::new(events))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use flock_core::Status;
    use flock_core::workload::ContainerStatus;

    use super::*;

    fn pod(ready: bool, restarts: u32) -> Pod {
        Pod {
            name: "checkout-api-7f9c6d-x2lqp".into(),
            phase: "Running".into(),
            status: Status::Ok,
            node: String::new(),
            cpu_millis: None,
            memory_bytes: None,
            age: Duration::ZERO,
            containers: vec![ContainerStatus {
                name: "api".into(),
                ready,
                restarts,
                last_exit: None,
            }],
        }
    }

    #[test]
    fn unready_containers_get_a_debug_container() {
        assert!(needs_debug(&pod(false, 14), "api"));
        assert!(!needs_debug(&pod(true, 0), "api"));
        assert!(needs_debug(&pod(true, 0), "missing"));
    }

    #[test]
    fn core_lines_keep_time_level_and_message() {
        let line = kit_line(flock_core::LogLine::new(
            "21:12:20.231",
            LogLevel::Fatal,
            "config invalid",
        ));
        assert_eq!(line.timestamp.as_deref(), Some("21:12:20.231"));
        assert_eq!(line.level, Some(Level::Fatal));
        assert_eq!(line.message.as_ref(), "config invalid");
    }
}
