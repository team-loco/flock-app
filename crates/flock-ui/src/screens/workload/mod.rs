use crate::kit::SelectableText;
mod dock;

pub(crate) use dock::kit_line;

use flock_core::palette::COMPARE_KEY;
use flock_core::watch::WATCH_SYNC_TIMEOUT;
use flock_core::workload::{ContainerSpec, EnvVar, Pod, Revision, short_image, short_pod_name};
use flock_core::{Command, LogTarget, ResourceRef, Seen, Sink, Status, Workload};
use gpui::{
    AnyElement, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, WeakEntity, Window, div,
    prelude::FluentBuilder, px, relative,
};

use crate::kit::status_symbols;
use crate::kit::{
    ActionMenu, Button, CodeView, Column, ConfirmDialog, DockState, EmptyState, Entrance,
    ErrorState, InspectorPane, KeyValue, LOG_FILTER_PLACEHOLDER, Link, LogView, MenuItem,
    PageHeader, Rule, SectionHeader, SectionLabel, Segmented, Skeleton, StatusText, Table,
    TableEvent, Terminal, TextFieldState, Toast, Width, confirm, min_row_width, slide_in, toast,
};
use crate::screens::{FullView, full_view_link};
use crate::theme::{
    ActiveTheme, FIELD_LABEL, FONT_MONO, LINE_PROSE, Palette, RADIUS, ROW_TABLE, RULE, SPACE_1,
    SPACE_2, SPACE_3, SPACE_5, SPACE_6, SPACE_8, TEXT_BODY, TEXT_CONTROL, TEXT_GLYPH, TEXT_MONO,
    TEXT_SMALL,
};

pub const KEY_CONTEXT: &str = "Workload";

gpui::actions!(workload, [ToggleActions, ShowLogs, ShowTerminal, ViewYaml]);

const PODS: &str = "Pods";
const PODS_HINT: &str = "click to follow logs";
const ROLLOUT_HISTORY: &str = "Rollout history";
const NO_HISTORY: &str = "No rollouts recorded for this workload.";
const CONTAINER: &str = "Container";
const ENVIRONMENT: &str = "Environment · resolved";
const CURRENT: &str = "current";
const ACTIONS: &str = "Actions";
const ACTIONS_KEYS: &str = "⌘.";
const PLAIN_GLYPH: &str = "·";
const RETRY: &str = "Retry";
const MANAGED_BY: &str = "managed by";
const META_SEPARATOR: &str = " · ";

const NAME_WEIGHT: f32 = 1.5;
const NAME_MIN: Pixels = px(210.);
const READY_WIDTH: Pixels = px(44.);
const STATUS_MIN: Pixels = px(90.);
const STATUS_MAX: Pixels = px(260.);
const RESTARTS_WIDTH: Pixels = px(58.);
const NODE_WIDTH: Pixels = px(178.);
const CPU_WIDTH: Pixels = px(52.);
const MEM_WIDTH: Pixels = px(76.);
const AGE_WIDTH: Pixels = px(34.);

const REV_WIDTH: Pixels = px(34.);
const RS_WIDTH: Pixels = px(170.);
const IMAGE_MIN: Pixels = px(170.);
const REV_PODS_WIDTH: Pixels = px(44.);
const REV_AGE_WIDTH: Pixels = px(40.);
const COMMIT_WIDTH: Pixels = px(190.);
const NOTE_WIDTH: Pixels = px(90.);

const INSPECTOR_WIDTH: Pixels = px(320.);
const INSPECTOR_ID: &str = "workload-inspector";
const INSPECTOR_HEADER: Pixels = px(30.);
const ENV_PAD_X: Pixels = px(7.);
const ENV_PAD_Y: Pixels = px(5.);
const ENV_INDENT: Pixels = px(16.);

const RESTART: &str = "Restart rollout";
const RESTART_BODY: &str = "Every pod is replaced by a new one, one at a time. Pods that fail for a reason outside the pod spec keep failing.";
const SCALE: &str = "Scale…";
const SCALE_TITLE: &str = "Scale";
const SCALE_BODY: &str = "Set the number of replicas. Flux sets it back on the next reconcile if the manifest in git pins replicas.";
const SCALE_CONFIRM: &str = "Scale";
const SCALE_INVALID: &str = "Replicas must be a whole number";
const ROLLBACK_DESCRIPTION: &str = "Flux re-applies git on the next reconcile unless";
const ROLLBACK_SUFFIX: &str = "is suspended.";
const LOGS_ITEM: &str = "Logs";
const ALL_LOGS_ITEM: &str = "Logs for all pods";
const ALL_LOGS_DESCRIPTION: &str = "Merge every pod's output, colour-coded, in a new tab.";
const SHELL_ITEM: &str = "Shell into pod";
const LOGS_KEYS: &str = "⌘L";
const SHELL_KEYS: &str = "⌃`";
const YAML_ITEM: &str = "View YAML";
const COMPARE_ITEM: &str = "Compare across clusters";
const YAML_KEYS: &str = "⌘Y";
const YAML_LABEL: &str = "YAML";
const YAML_NOTE: &str = "read-only";
const YAML_INSPECTOR_WIDTH: Pixels = px(480.);

pub struct WorkloadScreen {
    pub(crate) navigation: Option<Entity<crate::kit::HeaderNavigation>>,
    target: ResourceRef,
    workload: Option<Workload>,
    unavailable: bool,
    pods: Entity<Table<Pod>>,
    compact_pods: Entity<Table<Pod>>,
    compact: Option<FullView>,
    menu: Entity<ActionMenu>,
    pod: Option<String>,
    container: usize,
    dock: Entity<DockState>,
    logs: Entity<LogView>,
    filter: Entity<TextFieldState>,
    terminal: Entity<Terminal>,
    exec: Option<dock::ExecSession>,
    yaml: Option<String>,
    showing_yaml: bool,
    entrance: Entrance,
    focus: FocusHandle,
    watch_task: Option<Task<()>>,
    deadline: Option<Task<()>>,
    log_task: Option<Task<()>>,
    all_pods: Option<(Entity<crate::screens::multilog::MultiLogs>, Subscription)>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Command> for WorkloadScreen {}

fn entrance(screen: &mut WorkloadScreen) -> &mut Entrance {
    &mut screen.entrance
}

impl WorkloadScreen {
    pub fn close_inspector(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.showing_yaml {
            return false;
        }
        self.showing_yaml = false;
        cx.notify();
        true
    }

    pub fn list_focus(&self, cx: &gpui::App) -> FocusHandle {
        Focusable::focus_handle(&self.pods, cx)
    }

    pub fn dock(&self) -> &Entity<DockState> {
        &self.dock
    }

    pub fn new(target: ResourceRef, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pods = cx.new(|cx| {
            Table::new("pods", pod_columns(), cx)
                .row_key(|pod: &Pod| pod.name.clone().into())
                .fit_content()
        });
        let compact_pods = cx.new(|cx| {
            Table::new("compact-pods", compact_pod_columns(), cx)
                .headerless()
                .row_key(|pod: &Pod| pod.name.clone().into())
                .fit_content()
        });
        let menu = cx.new(|cx| {
            ActionMenu::new("workload-actions", cx).labeled(ACTIONS, Some(ACTIONS_KEYS.into()))
        });
        let dock = cx.new(|_| DockState::new());
        let logs = cx.new(|_| LogView::new("workload-logs"));
        let filter =
            cx.new(|cx| TextFieldState::new(window, cx).placeholder(LOG_FILTER_PLACEHOLDER));
        let terminal = cx.new(|cx| Terminal::new("workload-terminal", cx));
        let mut subscriptions = vec![cx.subscribe(&pods, |this, table, event: &TableEvent, cx| {
            let TableEvent::Selected(ix) = event else {
                return;
            };
            let name = table.read(cx).row(*ix).map(|pod| pod.name.clone());
            if let Some(name) = name {
                this.select_pod(name, cx);
            }
        })];
        subscriptions.extend(dock::subscribe(&dock, &filter, &terminal, cx));
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            navigation: None,
            target,
            workload: None,
            unavailable: false,
            pods,
            compact_pods,
            compact: None,
            menu,
            pod: None,
            container: 0,
            dock,
            logs,
            filter,
            terminal,
            exec: None,
            yaml: None,
            showing_yaml: false,
            entrance: Entrance::default(),
            focus,
            watch_task: None,
            deadline: None,
            log_task: None,
            all_pods: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn target(&self) -> &ResourceRef {
        &self.target
    }

    pub fn set_compact(&mut self, compact: Option<FullView>, cx: &mut Context<Self>) {
        if self.compact.is_some() != compact.is_some() {
            cx.notify();
        }
        self.compact = compact;
    }

    fn compact_view(
        &self,
        workload: &Workload,
        open: &FullView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = *cx.palette();
        let cluster = div()
            .text_size(TEXT_SMALL)
            .text_color(palette.mute)
            .child(workload.target.cluster.to_string());
        let header = PageHeader::new(workload.target.name.clone())
            .leading(self.navigation.clone())
            .mono()
            .status(workload.status(), workload.ready_label())
            .action(cluster)
            .sentence(SelectableText::new(workload.sentence()))
            .footer(full_view_link("workload-full-view", open));
        let fields: Vec<AnyElement> = workload
            .fields()
            .into_iter()
            .map(|field| field_row(field, &palette))
            .collect();
        div()
            .id("workload-compact")
            .flex()
            .flex_col()
            .size_full()
            .overflow_y_scroll()
            .child(header)
            .child(div().flex().flex_col().children(fields))
            .child(Rule::horizontal())
            .child(SectionHeader::new(PODS))
            .child(Rule::horizontal())
            .child(self.compact_pods.clone())
            .into_any_element()
    }

    pub fn set_yaml(&mut self, yaml: Option<&str>, cx: &mut Context<Self>) {
        if self.yaml.as_deref() != yaml {
            self.yaml = yaml.map(str::to_owned);
            cx.notify();
        }
    }

    fn toggle_yaml(&mut self, cx: &mut Context<Self>) {
        self.showing_yaml = !self.showing_yaml;
        cx.notify();
    }

    pub fn watch(&mut self, cx: &mut Context<Self>) {
        self.unavailable = false;
        self.deadline = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(WATCH_SYNC_TIMEOUT).await;
            let _ = this.update(cx, |screen, cx| {
                screen.unavailable = screen.workload.is_none();
                cx.notify();
            });
        }));
        let (sink, updates) = Sink::channel();
        cx.emit(Command::WatchWorkload {
            target: self.target.clone(),
            sink,
        });
        self.watch_task = Some(cx.spawn(async move |this, cx| {
            while let Ok(workload) = updates.recv().await {
                if this
                    .update(cx, |screen, cx| screen.receive(workload, cx))
                    .is_err()
                {
                    return;
                }
            }
            let _ = this.update(cx, |screen, cx| {
                screen.unavailable = screen.workload.is_none();
                cx.notify();
            });
        }));
    }

    fn receive(&mut self, workload: Workload, cx: &mut Context<Self>) {
        let pods = workload.pods.clone();
        let keep = self
            .pod
            .as_ref()
            .is_some_and(|name| pods.iter().any(|pod| &pod.name == name));
        let pod = if keep {
            self.pod.clone()
        } else {
            default_pod(&pods)
        };
        let selected = pod
            .as_ref()
            .and_then(|name| pods.iter().position(|p| &p.name == name));
        self.compact_pods
            .update(cx, |table, cx| table.set_rows(pods.clone(), cx));
        self.pods.update(cx, |table, cx| {
            table.set_rows(pods, cx);
            table.select(selected, cx);
        });
        self.container = self
            .container
            .min(workload.containers.len().saturating_sub(1));
        let items = Self::menu_items(&workload, cx);
        self.menu.update(cx, |menu, cx| menu.set_items(items, cx));
        self.workload = Some(workload);
        if pod != self.pod {
            self.discard_shell(cx);
            self.pod = pod;
            self.follow_logs(cx);
        }
        cx.notify();
    }

    fn select_pod(&mut self, name: String, cx: &mut Context<Self>) {
        if self.pod.as_ref() != Some(&name) {
            self.discard_shell(cx);
            self.pod = Some(name);
            self.follow_logs(cx);
        }
        self.dock.update(cx, |dock, cx| dock.select(dock::LOGS, cx));
        cx.notify();
    }

    fn select_container(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix != self.container {
            self.discard_shell(cx);
            self.container = ix;
            self.follow_logs(cx);
        }
        cx.notify();
    }

    fn selected_pod(&self) -> Option<&Pod> {
        let workload = self.workload.as_ref()?;
        let name = self.pod.as_ref()?;
        workload.pods.iter().find(|pod| &pod.name == name)
    }

    fn selected_container(&self) -> Option<&ContainerSpec> {
        self.workload.as_ref()?.containers.get(self.container)
    }

    fn menu_items(workload: &Workload, cx: &mut Context<Self>) -> Vec<MenuItem> {
        let this = cx.entity().downgrade();
        let mut items = vec![
            MenuItem::new(LOGS_ITEM, {
                let this = this.clone();
                move |_, cx| {
                    this.update(cx, WorkloadScreen::show_logs).ok();
                }
            })
            .shortcut(LOGS_KEYS),
            MenuItem::new(ALL_LOGS_ITEM, {
                let this = this.clone();
                let target = LogTarget::Workload(workload.target.clone());
                move |_, cx| {
                    let command = Command::NewTab(Box::new(Command::FollowLogs(target.clone())));
                    this.update(cx, |_, cx| cx.emit(command)).ok();
                }
            })
            .description(ALL_LOGS_DESCRIPTION),
            MenuItem::new(SHELL_ITEM, {
                let this = this.clone();
                move |window, cx| {
                    this.update(cx, |screen, cx| screen.show_terminal(window, cx))
                        .ok();
                }
            })
            .shortcut(SHELL_KEYS),
            MenuItem::new(YAML_ITEM, {
                let this = this.clone();
                move |_, cx| {
                    this.update(cx, WorkloadScreen::toggle_yaml).ok();
                }
            })
            .shortcut(YAML_KEYS),
            MenuItem::new(COMPARE_ITEM, {
                let this = this.clone();
                let target = workload.target.clone();
                move |_, cx| {
                    let command = Command::Compare(target.clone());
                    this.update(cx, |_, cx| cx.emit(command)).ok();
                }
            })
            .shortcut(COMPARE_KEY),
            MenuItem::new(RESTART, restart_handler(this.clone(), workload)),
        ];
        if workload.kind.scalable() {
            items.push(MenuItem::new(SCALE, scale_handler(this.clone(), workload)));
        }
        if let Some(revision) = workload.rollback_target() {
            let label = format!("Roll back to revision {}", revision.number);
            let description = workload
                .managed_by
                .as_ref()
                .map(|owner| format!("{ROLLBACK_DESCRIPTION} {} {ROLLBACK_SUFFIX}", owner.name));
            let item = MenuItem::new(label, rollback_handler(this, workload, revision));
            items.push(match description {
                Some(text) => item.description(text),
                None => item,
            });
        }
        items
    }

    fn header(&self, workload: &Workload, cx: &mut Context<Self>) -> impl IntoElement {
        let meta = Self::meta(workload, cx);
        PageHeader::new(workload.target.name.clone())
            .leading(self.navigation.clone())
            .mono()
            .status(workload.status(), workload.ready_label())
            .action(self.menu.clone())
            .sentence(SelectableText::new(workload.sentence()))
            .meta(meta)
    }

    fn meta(workload: &Workload, cx: &mut Context<Self>) -> impl IntoElement {
        let mut parts: Vec<String> = Vec::new();
        parts.extend(workload.target.namespace.clone());
        parts.extend(workload.image().map(str::to_owned));
        let owner = workload.managed_by.clone();
        let lead = match owner {
            Some(_) => {
                parts.push(MANAGED_BY.into());
                format!("{} ", parts.join(META_SEPARATOR))
            }
            None => parts.join(META_SEPARATOR),
        };
        let link = owner.map(|owner| {
            let name = owner.name.clone();
            Link::new("managed-by", name).on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(Command::OpenResource(owner.clone()));
            }))
        });
        div().flex().whitespace_nowrap().child(lead).children(link)
    }

    fn pods_section(&self, workload: &Workload) -> impl IntoElement {
        let count = format!("{} · {PODS_HINT}", workload.pods.len());
        div()
            .flex()
            .flex_col()
            .child(SectionHeader::new(PODS).count(count))
            .child(Rule::horizontal())
            .child(self.pods.clone())
    }

    fn history(workload: &Workload, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let rows: Vec<AnyElement> = workload
            .revisions
            .iter()
            .map(|revision| Self::revision_row(&workload.target.name, revision, &palette, cx))
            .collect();
        let body: AnyElement = if workload.history_pending {
            Skeleton::rows().into_any_element()
        } else if rows.is_empty() {
            EmptyState::new(NO_HISTORY)
                .selector("workload-history-empty")
                .into_any_element()
        } else {
            div()
                .id("history-scroll")
                .overflow_x_scroll()
                .restrict_scroll_to_axis()
                .child(div().min_w(history_min_width()).children(rows))
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .child(SectionHeader::new(ROLLOUT_HISTORY))
            .child(body)
    }

    fn revision_row(
        workload: &str,
        revision: &Revision,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pods_color = revision
            .status()
            .map_or(palette.mute, |status| palette.status(status));
        let commit = revision.commit.as_ref().map(|commit| {
            let text = commit.label(workload);
            let id = SharedString::from(format!("commit-{}", revision.number));
            let sha = commit.sha.clone();
            Link::new(id, text).on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(Command::OpenCommit(sha.clone()));
            }))
        });
        let note = revision.current.then_some(CURRENT);
        div()
            .flex()
            .items_center()
            .gap(SPACE_6)
            .h(ROW_TABLE)
            .px(SPACE_8)
            .border_t(RULE)
            .border_color(palette.line)
            .font_family(FONT_MONO)
            .text_size(TEXT_MONO)
            .when(revision.current, |row| row.bg(palette.panel))
            .child(
                fixed(REV_WIDTH)
                    .text_color(palette.mute)
                    .child(revision.number.to_string()),
            )
            .child(fixed(RS_WIDTH).truncate().child(revision.owner.clone()))
            .child(
                div()
                    .flex_1()
                    .min_w(IMAGE_MIN)
                    .truncate()
                    .text_color(palette.fg2)
                    .child(short_image(&revision.image).to_owned()),
            )
            .child(
                fixed(REV_PODS_WIDTH)
                    .text_color(pods_color)
                    .child(revision.pods_label()),
            )
            .child(
                fixed(REV_AGE_WIDTH)
                    .text_color(palette.mute)
                    .child(flock_core::time::age(revision.age)),
            )
            .child(fixed(COMMIT_WIDTH).truncate().children(commit))
            .child(
                fixed(NOTE_WIDTH)
                    .flex()
                    .justify_end()
                    .text_color(palette.mute)
                    .children(note),
            )
            .into_any_element()
    }

    fn yaml_inspector(&self, palette: &Palette) -> AnyElement {
        let yaml = match self.yaml.clone() {
            Some(yaml) => CodeView::new("workload-yaml-code", yaml)
                .yaml()
                .into_any_element(),
            None => Skeleton::rows().into_any_element(),
        };
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .h(INSPECTOR_HEADER)
            .px(SPACE_6)
            .border_b(RULE)
            .border_color(palette.line)
            .child(div().flex_1().child(SectionLabel::new(YAML_LABEL)))
            .child(
                div()
                    .text_size(TEXT_SMALL)
                    .text_color(palette.mute)
                    .child(YAML_NOTE),
            );
        let content = div()
            .id("workload-yaml")
            .flex()
            .flex_col()
            .flex_none()
            .w_full()
            .h_full()
            .overflow_y_scroll()
            .border_l(RULE)
            .border_color(palette.line)
            .bg(palette.panel)
            .child(header)
            .child(div().px(SPACE_6).py(SPACE_5).child(yaml))
            .into_any_element();
        InspectorPane::new(INSPECTOR_ID, content)
            .width(YAML_INSPECTOR_WIDTH)
            .into_any_element()
    }

    fn inspector(&self, workload: &Workload, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let palette = *cx.palette();
        let names: Vec<String> = workload.containers.iter().map(|c| c.name.clone()).collect();
        let segmented = names.into_iter().fold(
            Segmented::new("containers")
                .small()
                .mono()
                .selected(self.container),
            Segmented::option,
        );
        let segmented = segmented.on_select(cx.listener(|this, ix: &usize, _, cx| {
            this.select_container(*ix, cx);
        }));
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .h(INSPECTOR_HEADER)
            .px(SPACE_6)
            .border_b(RULE)
            .border_color(palette.line)
            .child(div().flex_1().child(SectionLabel::new(CONTAINER)))
            .child(segmented);
        let spec = self
            .selected_container()
            .map(|spec| self.spec(spec, &palette));
        let env = self
            .selected_container()
            .map(|spec| env_section(&spec.env, &palette));
        let content = div()
            .id("inspector")
            .flex()
            .flex_col()
            .flex_none()
            .w_full()
            .h_full()
            .overflow_y_scroll()
            .border_l(RULE)
            .border_color(palette.line)
            .bg(palette.panel)
            .text_size(TEXT_BODY)
            .child(header)
            .children(spec)
            .children(env);
        InspectorPane::new(INSPECTOR_ID, content).width(INSPECTOR_WIDTH)
    }

    fn spec(&self, spec: &ContainerSpec, palette: &Palette) -> impl IntoElement {
        let ports: Vec<String> = spec
            .ports
            .iter()
            .map(flock_core::workload::Port::label)
            .collect();
        let last_exit = self
            .selected_pod()
            .and_then(|pod| pod.container(&spec.name))
            .and_then(|status| status.last_exit.as_ref())
            .map(flock_core::workload::Exit::label);
        let readiness = spec
            .readiness
            .as_ref()
            .map(flock_core::workload::Probe::label);
        let mut grid = KeyValue::new()
            .text("image", spec.image.clone())
            .text("ports", ports.join(", "))
            .text("requests", spec.requests.label())
            .text("limits", spec.limits.label());
        if let Some(readiness) = readiness {
            grid = grid.text("readiness", readiness);
        }
        if let Some(exit) = last_exit {
            grid = grid.row(
                "last exit",
                div()
                    .text_color(palette.err)
                    .child(SelectableText::new(exit)),
            );
        }
        div()
            .px(SPACE_6)
            .py(SPACE_5)
            .border_b(RULE)
            .border_color(palette.line)
            .child(grid)
    }
}

fn history_min_width() -> Pixels {
    let widths = [
        Width::Fixed(REV_WIDTH),
        Width::Fixed(RS_WIDTH),
        Width::Flex {
            weight: 1.,
            min: IMAGE_MIN,
        },
        Width::Fixed(REV_PODS_WIDTH),
        Width::Fixed(REV_AGE_WIDTH),
        Width::Fixed(COMMIT_WIDTH),
        Width::Fixed(NOTE_WIDTH),
    ];
    min_row_width(widths.into_iter(), SPACE_6, SPACE_8 + SPACE_8)
}

fn fixed(width: Pixels) -> gpui::Div {
    div()
        .w(width)
        .flex_none()
        .overflow_hidden()
        .whitespace_nowrap()
}

fn env_section(env: &[EnvVar], palette: &Palette) -> impl IntoElement {
    let boxes = env.iter().map(|var| env_box(var, palette));
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .px(SPACE_6)
                .pt(SPACE_5)
                .pb(SPACE_2)
                .child(SectionLabel::new(ENVIRONMENT)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(SPACE_3)
                .px(SPACE_6)
                .pb(SPACE_5)
                .border_b(RULE)
                .border_color(palette.line)
                .children(boxes),
        )
}

fn env_box(var: &EnvVar, palette: &Palette) -> impl IntoElement {
    let status = var.status();
    let (glyph, color) = match status {
        Some(status) => (status.glyph(), palette.status(status)),
        None => (PLAIN_GLYPH, palette.mute),
    };
    let failing = status == Some(Status::Err);
    let note = var.note().map(|note| {
        div()
            .pl(ENV_INDENT)
            .text_size(TEXT_CONTROL)
            .line_height(relative(LINE_PROSE))
            .text_color(palette.fg2)
            .child(note)
    });
    div()
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .px(ENV_PAD_X)
        .py(ENV_PAD_Y)
        .rounded(RADIUS)
        .when(failing, |row| {
            row.bg(palette.errbg).border(RULE).border_color(palette.err)
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_3)
                .font_family(FONT_MONO)
                .text_size(TEXT_SMALL)
                .child(
                    div()
                        .text_size(TEXT_GLYPH)
                        .text_color(color)
                        .child(status_symbols(glyph)),
                )
                .child(div().text_color(palette.fg).child(var.name.clone())),
        )
        .child(
            div()
                .pl(ENV_INDENT)
                .font_family(FONT_MONO)
                .text_size(TEXT_SMALL)
                .text_color(palette.fg2)
                .child(var.source.label()),
        )
        .children(note)
}

fn default_pod(pods: &[Pod]) -> Option<String> {
    let failing = pods.iter().find(|pod| pod.status == Status::Err);
    failing.or(pods.first()).map(|pod| pod.name.clone())
}

fn field_row(field: flock_core::workload::Field, palette: &Palette) -> AnyElement {
    let color = field
        .status
        .map_or(palette.fg, |status| palette.status(status));
    div()
        .flex()
        .items_center()
        .gap(SPACE_3)
        .min_h(ROW_TABLE)
        .px(SPACE_8)
        .border_t(RULE)
        .border_color(palette.line)
        .text_size(TEXT_BODY)
        .child(
            div()
                .w(FIELD_LABEL)
                .flex_none()
                .text_color(palette.mute)
                .truncate()
                .child(field.label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .font_family(FONT_MONO)
                .text_size(TEXT_MONO)
                .text_color(color)
                .child(field.value),
        )
        .into_any_element()
}

fn compact_pod_columns() -> Vec<Column<Pod>> {
    vec![
        Column::text("Name", |p: &Pod| p.name.clone().into())
            .mono()
            .flex(NAME_WEIGHT, NAME_MIN),
        Column::new("Status", |p: &Pod, _| {
            StatusText::new(p.status, p.phase.clone()).into_any_element()
        })
        .mono()
        .fit_status_by(|p: &Pod| p.phase.clone().into())
        .auto(STATUS_MIN, STATUS_MAX),
        Column::colored_text(
            "Restarts",
            |p: &Pod| p.restarts().to_string().into(),
            |_, palette| palette.mute,
        )
        .mono()
        .end()
        .fixed(RESTARTS_WIDTH),
    ]
}

fn pod_columns() -> Vec<Column<Pod>> {
    vec![
        Column::text("Name", |p: &Pod| p.name.clone().into())
            .mono()
            .flex(NAME_WEIGHT, NAME_MIN),
        Column::colored_text(
            "Ready",
            |p: &Pod| p.ready_label().into(),
            |p, palette| palette.status(p.status),
        )
        .mono()
        .fixed(READY_WIDTH),
        Column::new("Status", |p: &Pod, _| {
            StatusText::new(p.status, p.phase.clone()).into_any_element()
        })
        .mono()
        .fit_status_by(|p: &Pod| p.phase.clone().into())
        .auto(STATUS_MIN, STATUS_MAX),
        Column::colored_text(
            "Restarts",
            |p: &Pod| p.restarts().to_string().into(),
            |p, palette| {
                if p.restarts() > 0 {
                    palette.err
                } else {
                    palette.mute
                }
            },
        )
        .mono()
        .end()
        .fixed(RESTARTS_WIDTH),
        Column::colored_text(
            "Node",
            |p: &Pod| p.node.clone().into(),
            |_, palette| palette.fg2,
        )
        .mono()
        .fixed(NODE_WIDTH),
        Column::colored_text(
            "CPU",
            |p: &Pod| p.cpu_label().into(),
            |_, palette| palette.fg2,
        )
        .mono()
        .end()
        .fixed(CPU_WIDTH),
        Column::colored_text(
            "Memory",
            |p: &Pod| p.memory_label().into(),
            |_, palette| palette.fg2,
        )
        .mono()
        .end()
        .fixed(MEM_WIDTH),
        Column::colored_text(
            "Age",
            |p: &Pod| p.age_label().into(),
            |_, palette| palette.mute,
        )
        .mono()
        .fixed(AGE_WIDTH),
    ]
}

fn restart_handler(
    this: WeakEntity<WorkloadScreen>,
    workload: &Workload,
) -> impl Fn(&mut Window, &mut gpui::App) + 'static {
    let target = Seen::new(workload.target.clone(), workload.uid.clone());
    let detail = detail_line(workload);
    move |window, cx| {
        let this = this.clone();
        let target = target.clone();
        let dialog = ConfirmDialog::new(RESTART, RESTART_BODY, RESTART, move |_, cx| {
            let command = Command::RestartRollout(target.clone());
            this.update(cx, |_, cx| cx.emit(command)).ok();
        })
        .detail(detail.clone());
        confirm(dialog, window, cx);
    }
}

fn scale_handler(
    this: WeakEntity<WorkloadScreen>,
    workload: &Workload,
) -> impl Fn(&mut Window, &mut gpui::App) + 'static {
    let target = Seen::new(workload.target.clone(), workload.uid.clone());
    let detail = detail_line(workload);
    let desired = workload.desired.to_string();
    move |window, cx| {
        let this = this.clone();
        let target = target.clone();
        let field = cx.new(|cx| TextFieldState::new(window, cx).default_value(desired.clone()));
        let value = field.clone();
        let title = format!("{SCALE_TITLE} {}", target.target.name);
        let dialog = ConfirmDialog::new(title, SCALE_BODY, SCALE_CONFIRM, move |_, cx| {
            let text = value.read(cx).value();
            let Some(replicas) = parse_replicas(&text) else {
                toast(
                    Toast::new(Status::Warn, SCALE_INVALID).detail(text.to_string()),
                    cx,
                );
                return;
            };
            let command = Command::Scale {
                target: target.clone(),
                replicas,
            };
            this.update(cx, |_, cx| cx.emit(command)).ok();
        })
        .detail(detail.clone())
        .field(field)
        .primary();
        confirm(dialog, window, cx);
    }
}

fn rollback_handler(
    this: WeakEntity<WorkloadScreen>,
    workload: &Workload,
    revision: &Revision,
) -> impl Fn(&mut Window, &mut gpui::App) + 'static {
    let target = Seen::new(workload.target.clone(), workload.uid.clone());
    let number = revision.number;
    let title = format!("Roll back to revision {number}");
    let body = format!(
        "{} goes back to {} ({}). New pods replace the current ones.",
        target.target.name,
        short_image(&revision.image),
        revision.owner
    );
    let detail = detail_line(workload);
    move |window, cx| {
        let this = this.clone();
        let target = target.clone();
        let dialog =
            ConfirmDialog::new(title.clone(), body.clone(), title.clone(), move |_, cx| {
                let command = Command::Rollback {
                    target: target.clone(),
                    revision: number,
                };
                this.update(cx, |_, cx| cx.emit(command)).ok();
            })
            .detail(detail.clone());
        confirm(dialog, window, cx);
    }
}

fn detail_line(workload: &Workload) -> String {
    format!("{} · {}", workload.target.cluster, workload.target)
}

pub fn parse_replicas(text: &str) -> Option<u32> {
    text.trim().parse().ok()
}

pub fn logs_tab_label(pod: &str, container: &str) -> String {
    format!("{} · {container}", short_pod_name(pod))
}

impl Focusable for WorkloadScreen {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for WorkloadScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let root = div()
            .id("workload")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ToggleActions, window, cx| {
                this.menu.update(cx, |menu, cx| menu.toggle(window, cx));
            }))
            .on_action(cx.listener(|this, _: &ShowLogs, _, cx| this.show_logs(cx)))
            .on_action(cx.listener(|this, _: &ViewYaml, _, cx| this.toggle_yaml(cx)))
            .on_action(cx.listener(|this, _: &ShowTerminal, window, cx| {
                this.show_terminal(window, cx);
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(palette.editor);
        let Some(workload) = self.workload.clone() else {
            let header = crate::kit::PageTitle::new(self.target.name.clone())
                .leading(self.navigation.clone());
            let root = root.child(header);
            if !self.unavailable {
                return root.child(Skeleton::blocks());
            }
            let retry = Button::new("workload-retry", RETRY)
                .on_click(cx.listener(|this, _, _, cx| this.watch(cx)));
            return root.child(ErrorState::new(&unavailable_note(&self.target), retry));
        };
        if let Some(open) = &self.compact {
            return root.child(self.compact_view(&workload, open, cx));
        }
        let main = div()
            .id("workload-main")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .child(self.header(&workload, cx))
            .child(self.pods_section(&workload))
            .child(Self::history(&workload, cx));
        let shown = if self.showing_yaml {
            YAML_LABEL
        } else {
            CONTAINER
        };
        let motion = self.entrance.track(Some(shown), entrance, cx);
        let inspector = if self.showing_yaml {
            let panel = self.yaml_inspector(&palette);
            slide_in(panel, INSPECTOR_ID, motion, YAML_INSPECTOR_WIDTH)
        } else {
            let panel = self.inspector(&workload, cx);
            slide_in(panel, INSPECTOR_ID, motion, INSPECTOR_WIDTH)
        };
        let dock = self.render_dock(&workload, cx);
        root.child(div().flex().flex_1().min_h_0().child(main).child(inspector))
            .child(dock)
    }
}

fn unavailable_note(target: &ResourceRef) -> String {
    format!(
        "{} {target} is not available on {}.",
        target.kind, target.cluster
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_note_names_the_workload_and_cluster() {
        let target = ResourceRef {
            cluster: flock_core::ClusterId::new("qa-gke"),
            kind: "Deployment".into(),
            namespace: Some("payments".into()),
            name: "checkout-api".into(),
        };
        assert_eq!(
            unavailable_note(&target),
            "Deployment payments/checkout-api is not available on qa-gke."
        );
    }

    #[test]
    fn replicas_parse_whole_numbers_only() {
        assert_eq!(parse_replicas(" 4 "), Some(4));
        assert_eq!(parse_replicas("0"), Some(0));
        assert_eq!(parse_replicas("-1"), None);
        assert_eq!(parse_replicas("two"), None);
    }

    #[test]
    fn logs_tab_names_the_pod_suffix_and_container() {
        assert_eq!(
            logs_tab_label("checkout-api-7f9c6d-x2lqp", "api"),
            "x2lqp · api"
        );
    }
}
