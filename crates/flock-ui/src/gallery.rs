use flock_core::Status;
use gpui::{
    AnyElement, App, AppContext, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled, Subscription,
    Window, div, px,
};

use crate::kit::{
    ActionMenu, Button, Checkbox, CodeView, Column, ConfirmDialog, DiffFile, DiffLine, DiffView,
    Dock, DockState, DockTab, IconButton, InputEvent, Kbd, KeyValue, LOG_FILTER_PLACEHOLDER, Level,
    LineChart, LineKind, Link, LogLine, LogView, MenuItem, Meter, MiniMeter, PageHeader, Rule,
    SectionHeader, Segmented, Select, Series, Split, StackedBar, StatusGlyph, StatusText, Table,
    TextField, TextFieldState, Toast, Toggle, confirm, overlay_layer, parse_unified, toast,
};
use crate::theme::{
    self, ActiveTheme, DOCK_HEIGHT, FONT_MONO, Palette, RULE, SPACE_4, SPACE_6, SPACE_8, TEXT_MONO,
    TEXT_SMALL, TEXT_UI, TITLE_ROW,
};

const SELECT_WIDTH: Pixels = px(220.);
const FIELD_WIDTH: Pixels = px(220.);
const INSPECTOR_WIDTH: Pixels = px(360.);
const NAME_WEIGHT: f32 = 1.5;
const NAME_MIN: Pixels = px(210.);
const READY_WIDTH: Pixels = px(44.);
const STATUS_WIDTH: Pixels = px(150.);
const RESTARTS_WIDTH: Pixels = px(58.);
const NODE_WIDTH: Pixels = px(178.);
const CPU_WIDTH: Pixels = px(52.);
const MEM_WIDTH: Pixels = px(60.);
const AGE_WIDTH: Pixels = px(34.);

const METER_WIDTH: Pixels = px(240.);
const SPLIT_HEIGHT: Pixels = px(420.);
const EVENT_AGE_WIDTH: Pixels = px(48.);
const EVENT_REASON_WIDTH: Pixels = px(170.);
const EVENT_COUNT_WIDTH: Pixels = px(34.);
const CHART_MAX: f32 = 100.;
const CPU: [f32; 13] = [
    39., 42., 45., 43., 48., 52., 50., 55., 58., 54., 59., 61., 60.,
];
const MEMORY: [f32; 13] = [
    52., 53., 53., 53., 54., 55., 55., 55., 56., 56., 57., 57., 57.,
];
const CPU_REQUESTS: f32 = 74.;
const LOGS: [(&str, Level, &str); 8] = [
    (
        "21:12:20.114",
        Level::Info,
        "starting checkout-api v2.14.0 (commit 9f3c1ab)",
    ),
    (
        "21:12:20.119",
        Level::Info,
        "config: loading /etc/checkout/app.yaml",
    ),
    (
        "21:12:20.125",
        Level::Info,
        "postgres: connecting payments-db.payments.svc:5432",
    ),
    ("21:12:20.228", Level::Info, "postgres: ok (pool 20)"),
    (
        "21:12:20.230",
        Level::Warn,
        "env STRIPE_WEBHOOK_SECRET is empty; falling back to /etc/checkout/stripe.env",
    ),
    (
        "21:12:20.231",
        Level::Error,
        "open /etc/checkout/stripe.env: no such file or directory",
    ),
    (
        "21:12:20.231",
        Level::Fatal,
        "config invalid: stripe webhook secret not set; exit 1",
    ),
    ("21:12:20.240", Level::Debug, "shutdown complete"),
];
const SPEC: &str = "apiVersion: kustomize.toolkit.fluxcd.io/v1
kind: Kustomization
metadata:
  name: apps-payments # owned by platform
  namespace: flux-system
spec:
  interval: 10m
  path: ./apps/payments/overlays/prod-us-east
  prune: true
  sourceRef:
    kind: GitRepository
    name: fleet-infra
  timeout: 5m";
const PATCH: &str =
    "diff --git a/apps/payments/base/configmap.yaml b/apps/payments/base/configmap.yaml
--- a/apps/payments/base/configmap.yaml
+++ b/apps/payments/base/configmap.yaml
@@ -8,5 +8,3 @@ configMapGenerator:
   files:
     - app.yaml
-    - stripe.env
-      # legacy: read by checkout-api < 2.14
 generatorOptions:";

#[derive(Clone)]
struct Event {
    age: &'static str,
    status: Status,
    reason: &'static str,
    message: &'static str,
    count: u32,
}

const EVENTS: [Event; 3] = [
    Event {
        age: "2m",
        status: Status::Err,
        reason: "BackOff",
        message: "Back-off restarting failed container api in pod checkout-api-7f9c6d-x2lqp",
        count: 14,
    },
    Event {
        age: "3h",
        status: Status::Err,
        reason: "ProgressDeadlineExceeded",
        message: "ReplicaSet \"checkout-api-7f9c6d\" has timed out progressing",
        count: 1,
    },
    Event {
        age: "3h",
        status: Status::Ok,
        reason: "ScalingReplicaSet",
        message: "Scaled up replica set checkout-api-7f9c6d to 3 from 2",
        count: 1,
    },
];

const STATUSES: [(Status, &str); 6] = [
    (Status::Ok, "Ready"),
    (Status::Warn, "Degraded"),
    (Status::Err, "CrashLoopBackOff"),
    (Status::Progress, "Progressing"),
    (Status::Suspended, "Suspended"),
    (Status::Offline, "Offline"),
];

#[derive(Clone)]
struct Pod {
    name: &'static str,
    ready: &'static str,
    status: Status,
    phase: &'static str,
    restarts: u32,
    node: &'static str,
    cpu: &'static str,
    mem: &'static str,
    age: &'static str,
}

const PODS: [Pod; 4] = [
    Pod {
        name: "checkout-api-7f9c6d-x2lqp",
        ready: "1/2",
        status: Status::Err,
        phase: "CrashLoopBackOff",
        restarts: 14,
        node: "ip-10-4-12-87",
        cpu: "4m",
        mem: "38Mi",
        age: "3h",
    },
    Pod {
        name: "checkout-api-7f9c6d-m8vtn",
        ready: "2/2",
        status: Status::Ok,
        phase: "Running",
        restarts: 0,
        node: "ip-10-4-14-201",
        cpu: "212m",
        mem: "301Mi",
        age: "3h",
    },
    Pod {
        name: "checkout-api-7f9c6d-q4zhk",
        ready: "2/2",
        status: Status::Ok,
        phase: "Running",
        restarts: 0,
        node: "ip-10-4-22-63",
        cpu: "187m",
        mem: "288Mi",
        age: "3h",
    },
    Pod {
        name: "checkout-api-7f9c6d-r7wbn",
        ready: "1/2",
        status: Status::Warn,
        phase: "Readiness failing",
        restarts: 2,
        node: "ip-10-4-22-63",
        cpu: "96m",
        mem: "120Mi",
        age: "12m",
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GalleryPage {
    Primitives,
    Overlays,
    Panes,
}

pub struct Gallery {
    page: GalleryPage,
    more: Entity<ActionMenu>,
    actions: Entity<ActionMenu>,
    select: Entity<Select>,
    pods: Entity<Table<Pod>>,
    empty: Entity<Table<Pod>>,
    path: Entity<TextFieldState>,
    filter: Entity<TextFieldState>,
    logs: Entity<LogView>,
    events: Entity<Table<Event>>,
    dock: Entity<DockState>,
    split_collapsed: bool,
    notify: bool,
    include: bool,
    severity: usize,
    focus: FocusHandle,
    _appearance: Subscription,
}

impl Gallery {
    pub fn new(page: GalleryPage, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let more = cx.new(|cx| ActionMenu::new("more", cx).items(more_items()));
        let actions = cx.new(|cx| {
            ActionMenu::new("actions", cx)
                .labeled("Actions", Some("⌘.".into()))
                .items(workload_items())
        });
        let select = cx.new(|cx| {
            Select::new(
                "context",
                vec![
                    "prod-us-east".into(),
                    "prod-eu-west".into(),
                    "staging".into(),
                ],
                cx,
            )
            .mono()
            .width(SELECT_WIDTH)
        });
        let pods = cx.new(|cx| {
            Table::new("pods", pod_columns(), cx)
                .rows(PODS.to_vec())
                .row_status(|pod: &Pod| (pod.status != Status::Ok).then_some(pod.status))
                .row_key(|pod: &Pod| pod.name.into())
                .fit_content()
        });
        let empty = cx.new(|cx| {
            Table::new("empty", pod_columns(), cx)
                .empty("No pods match the filter.")
                .fit_content()
        });
        let path = cx.new(|cx| TextFieldState::new(window, cx).default_value("apps/payments"));
        let filter =
            cx.new(|cx| TextFieldState::new(window, cx).placeholder(LOG_FILTER_PLACEHOLDER));
        let logs = cx.new(|cx| {
            let mut view = LogView::new("logs");
            let lines = LOGS.map(|(ts, level, msg)| LogLine::new(Some(ts), Some(level), msg));
            view.extend(lines, cx);
            view.push(
                LogLine::parse("\u{1b}[2m── container exited 1 · restart 14 in 5m0s (back-off) ──"),
                cx,
            );
            view
        });
        let query = logs.clone();
        cx.subscribe(&filter, move |_, state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let text = state.read(cx).value();
                query.update(cx, |logs, cx| logs.set_query(text, cx));
            }
        })
        .detach();
        let events = cx.new(|cx| {
            Table::new("events", event_columns(), cx)
                .rows(EVENTS.to_vec())
                .headerless()
                .row_height(theme::LOG_LINE)
        });
        let dock = cx.new(|_| DockState::new());
        let appearance = cx.observe_window_appearance(window, |_, window, cx| {
            theme::follow_window(window, cx);
            cx.notify();
        });
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        theme::follow_window(window, cx);
        if page == GalleryPage::Overlays {
            open_samples(&more, window, cx);
        }
        Self {
            page,
            more,
            actions,
            select,
            pods,
            empty,
            path,
            filter,
            logs,
            events,
            dock,
            split_collapsed: false,
            notify: true,
            include: true,
            severity: 0,
            focus,
            _appearance: appearance,
        }
    }

    fn toggle_follow(&mut self, cx: &mut Context<Self>) {
        self.logs.update(cx, |logs, cx| {
            let follow = !logs.follow();
            logs.set_follow(follow, cx);
        });
        cx.notify();
    }

    fn statuses(palette: &Palette) -> impl IntoElement {
        let glyphs = STATUSES.map(|(status, _)| StatusGlyph::new(status));
        let texts = STATUSES.map(|(status, label)| StatusText::new(status, label));
        div()
            .flex()
            .flex_col()
            .gap(SPACE_4)
            .px(SPACE_8)
            .child(div().flex().gap(SPACE_6).children(glyphs))
            .child(
                div()
                    .flex()
                    .gap(SPACE_8)
                    .text_color(palette.fg)
                    .children(texts),
            )
    }

    fn header(&self) -> impl IntoElement {
        PageHeader::new("apps-payments")
            .mono()
            .status(Status::Err, "Health check failed")
            .action(
                Button::new("reconcile", "Reconcile")
                    .primary()
                    .shortcut("⌘R")
                    .on_click(|_, _, cx| {
                        toast(
                            Toast::new(Status::Progress, "Reconciling apps-payments")
                                .key("reconcile"),
                            cx,
                        )
                    }),
            )
            .action(self.more.clone())
            .meta(
                div()
                    .flex()
                    .gap(SPACE_4)
                    .child("applied")
                    .child(Link::new("applied", "a41e07c").mono())
                    .child("3h ago · last healthy")
                    .child(Link::new("healthy", "7c2d9e0").mono())
                    .child("1d ago"),
            )
    }

    fn buttons(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(SPACE_4)
            .px(SPACE_8)
            .child(
                Button::new("b-primary", "Reconcile")
                    .primary()
                    .shortcut("⌘R"),
            )
            .child(Button::new("b-secondary", "Cancel"))
            .child(Button::new("b-ghost", "Paste kubeconfig…").ghost())
            .child(Button::new("b-danger", "Delete pod").danger())
            .child(Button::new("b-disabled", "Disabled").disabled(true))
            .child(
                Button::new("b-follow", "follow")
                    .small()
                    .selected(self.logs.read(cx).follow())
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_follow(cx))),
            )
            .child(IconButton::glyph("b-close", "×", "Close"))
            .child(self.actions.clone())
            .child(self.select.clone())
            .child(Kbd::new("⌘K"))
            .child(Link::new("b-link", "Compare rendered output →"))
    }

    fn controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(SPACE_8)
            .px(SPACE_8)
            .child(
                TextField::new(&self.path)
                    .prefix("path:")
                    .width(FIELD_WIDTH),
            )
            .child(TextField::search(&self.filter))
            .child(
                Toggle::new("t-notify", self.notify)
                    .label("Confirm destructive actions")
                    .on_toggle(cx.listener(|this, on: &bool, _, cx| {
                        this.notify = *on;
                        cx.notify();
                    })),
            )
            .child(
                Checkbox::new("c-include", self.include)
                    .label("prod-us-east")
                    .on_toggle(cx.listener(|this, on: &bool, _, cx| {
                        this.include = *on;
                        cx.notify();
                    })),
            )
            .child(
                Segmented::new("severity")
                    .option("All")
                    .option("Errors")
                    .option("Warnings")
                    .selected(self.severity)
                    .on_select(cx.listener(|this, ix: &usize, _, cx| {
                        this.severity = *ix;
                        cx.notify();
                    })),
            )
            .child(
                Segmented::new("container")
                    .option("api")
                    .option("envoy")
                    .small()
                    .mono(),
            )
    }

    fn inspector(palette: &Palette) -> impl IntoElement {
        div().w(INSPECTOR_WIDTH).px(SPACE_8).child(
            KeyValue::new()
                .text("image", "ghcr.io/acme/checkout-api:2.14.0")
                .text("ports", "8080/TCP http")
                .text("requests", "CPU 500m · memory 512Mi")
                .text("limits", "CPU 1 · memory 1Gi")
                .row(
                    "last exit",
                    div().text_color(palette.err).child("1 (Error) · 2m ago"),
                ),
        )
    }

    fn charts(palette: &Palette) -> impl IntoElement {
        let percent = |labels: [&'static str; 5]| labels.map(SharedString::from).to_vec();
        div()
            .flex()
            .gap(SPACE_8 * 2.)
            .px(SPACE_8)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_6)
                    .w(METER_WIDTH)
                    .flex_none()
                    .text_size(TEXT_SMALL)
                    .text_color(palette.mute)
                    .child("CPU 61% · requests 74% · limits 131%")
                    .child(Meter::new(0.61).request(0.74).limit(1.0))
                    .child("Pods 398 running · 9 pending · 5 failing")
                    .child(StackedBar::new(vec![
                        (Status::Ok, 398.),
                        (Status::Warn, 9.),
                        (Status::Err, 5.),
                    ]))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(SPACE_4)
                            .font_family(FONT_MONO)
                            .child(MiniMeter::new(0.42))
                            .child("42%"),
                    ),
            )
            .child(
                div().flex_1().child(
                    LineChart::new(CHART_MAX)
                        .series(Series::new("CPU", palette.acc, CPU.to_vec()).area(palette.accbg))
                        .series(Series::new("Memory", palette.ok, MEMORY.to_vec()))
                        .threshold(CPU_REQUESTS, "CPU requests", palette.warn)
                        .y_labels(percent(["100%", "75%", "50%", "25%", "0"]))
                        .x_labels(percent(["−60m", "−45m", "−30m", "−15m", "now"])),
                ),
            )
    }

    fn panes(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let removed = DiffLine::new(
            LineKind::Removed,
            Some(42),
            "        image: ghcr.io/acme/checkout-api:2.13.4",
        );
        let rendered = DiffFile::new(
            "Deployment/payments/checkout-api · rendered for prod-us-east",
            vec![
                DiffLine::new(LineKind::Context, Some(41), "      - name: api"),
                removed,
                DiffLine::new(
                    LineKind::Added,
                    Some(42),
                    "        image: ghcr.io/acme/checkout-api:2.14.0",
                ),
                DiffLine::new(
                    LineKind::Added,
                    Some(48),
                    "              key: webhook_secret",
                )
                .note("set by overlays/prod-us-east/patches/stripe.yaml:9 · unchanged 214d"),
            ],
        );
        let mut files = parse_unified(PATCH);
        files.push(rendered);
        let left = div()
            .id("split-left")
            .size_full()
            .overflow_y_scroll()
            .p(SPACE_8)
            .child(CodeView::new("spec", SPEC).yaml());
        let right = div()
            .id("split-right")
            .size_full()
            .overflow_y_scroll()
            .p(SPACE_8)
            .child(DiffView::new("diff", files).sides("live", "a41e07c"));
        div()
            .flex()
            .flex_col()
            .gap(SPACE_4)
            .child(
                div().flex().px(SPACE_8).child(
                    Button::new("split-toggle", "Collapse right pane")
                        .small()
                        .selected(self.split_collapsed)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.split_collapsed = !this.split_collapsed;
                            cx.notify();
                        })),
                ),
            )
            .child(
                div()
                    .h(SPLIT_HEIGHT)
                    .border_t(RULE)
                    .border_b(RULE)
                    .border_color(cx.palette().line)
                    .child(
                        Split::horizontal("gallery-split", left, right)
                            .ratio(0.45)
                            .collapsed(self.split_collapsed),
                    ),
            )
    }

    fn dock(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tools = div()
            .flex()
            .items_center()
            .gap(SPACE_4)
            .child(TextField::search(&self.filter))
            .child(
                Button::new("dock-follow", "follow")
                    .small()
                    .selected(self.logs.read(cx).follow())
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_follow(cx))),
            );
        let terminal = div()
            .p(SPACE_6)
            .font_family(FONT_MONO)
            .text_size(TEXT_MONO)
            .child("/ # ls /etc/checkout")
            .child("app.yaml");
        div().h(DOCK_HEIGHT).flex().flex_col().justify_end().child(
            Dock::new(&self.dock)
                .tab(
                    DockTab::new("Logs")
                        .detail("x2lqp · api")
                        .tools(tools)
                        .content(self.logs.clone()),
                )
                .tab(
                    DockTab::new("Events")
                        .detail("7")
                        .content(self.events.clone()),
                )
                .tab(
                    DockTab::new("Terminal")
                        .detail("debug container")
                        .content(terminal),
                ),
        )
    }

    fn feedback(cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .gap(SPACE_4)
            .px(SPACE_8)
            .child(Button::new("toast-ok", "Toast").on_click(|_, _, cx| {
                toast(
                    Toast::new(Status::Ok, "Scaled checkout-api to 3")
                        .detail("prod-us-east · payments"),
                    cx,
                );
            }))
            .child(
                Button::new("toast-err", "Error toast").on_click(|_, _, cx| {
                    toast(
                        Toast::new(Status::Err, "Reconcile failed")
                            .detail("kustomize build: accumulating resources")
                            .action("Retry", |_, _| {}),
                        cx,
                    );
                }),
            )
            .child(
                Button::new("open-confirm", "Confirm dialog")
                    .danger()
                    .on_click(cx.listener(|_, _, window, cx| {
                        confirm(delete_dialog(), window, cx);
                    })),
            )
    }
}

fn open_samples(more: &Entity<ActionMenu>, window: &mut Window, cx: &mut App) {
    more.update(cx, |menu, cx| menu.show(window, cx));
    confirm(delete_dialog(), window, cx);
    let samples = [
        Toast::new(Status::Progress, "Reconciling apps-payments")
            .detail("prod-us-east · flux-system"),
        Toast::new(Status::Ok, "Scaled checkout-api to 3").detail("prod-us-east · payments"),
        Toast::new(Status::Warn, "Drain ip-10-0-4-17 incomplete")
            .detail("prod-us-east · 2 pods blocked by PodDisruptionBudgets"),
        Toast::new(Status::Err, "Reconcile failed")
            .detail("kustomize build: accumulating resources")
            .action("Retry", |_, _| {})
            .action("View logs", |_, _| {}),
    ];
    for sample in samples {
        toast(sample, cx);
    }
}

fn delete_dialog() -> ConfirmDialog {
    ConfirmDialog::new(
        "Delete pod",
        "The pod is deleted now. Its ReplicaSet creates a replacement within seconds; logs from this container are lost.",
        "Delete pod",
        |_, cx| toast(Toast::new(Status::Ok, "Deleted checkout-api-7f9c6d-x2lqp"), cx),
    )
    .detail("prod-us-east · payments/checkout-api-7f9c6d-x2lqp")
}

fn more_items() -> Vec<MenuItem> {
    vec![
        MenuItem::new("Suspend", |_, _| {})
            .shortcut("⌘⇧S")
            .description("Stop reconciling until resumed. Drift stays in place."),
        MenuItem::new("Reconcile with source", |_, _| {})
            .description("Fetch fleet-infra first, then apply."),
        MenuItem::new("View YAML", |_, _| {})
            .shortcut("⌘Y")
            .description("Read the live manifest without editing it."),
        MenuItem::new("Delete…", |window, cx| {
            confirm(delete_dialog(), window, cx);
        })
        .destructive()
        .description("Prune every object this Kustomization manages."),
    ]
}

fn workload_items() -> Vec<MenuItem> {
    vec![
        MenuItem::new("Logs", |_, _| {}).shortcut("⌘L"),
        MenuItem::new("Shell into pod", |_, _| {}).shortcut("⌃`"),
        MenuItem::new("Restart rollout", |_, _| {}),
        MenuItem::new("Scale…", |_, _| {}).disabled(true),
        MenuItem::new("Roll back to revision 36", |_, _| {}).description(
            "Flux re-applies git on the next reconcile unless apps-payments is suspended.",
        ),
    ]
}

fn event_columns() -> Vec<Column<Event>> {
    vec![
        Column::colored_text("Age", |e: &Event| e.age.into(), |_, p| p.mute)
            .mono()
            .fixed(EVENT_AGE_WIDTH),
        Column::new("Reason", |e: &Event, _| {
            StatusText::new(e.status, e.reason).into_any_element()
        })
        .mono()
        .fixed(EVENT_REASON_WIDTH),
        Column::colored_text("Message", |e: &Event| e.message.into(), |_, p| p.fg2).mono(),
        Column::colored_text(
            "Count",
            |e: &Event| flock_core::units::times(e.count).into(),
            |_, p| p.mute,
        )
        .mono()
        .end()
        .fixed(EVENT_COUNT_WIDTH),
    ]
}

fn pod_columns() -> Vec<Column<Pod>> {
    vec![
        Column::text("Name", |pod: &Pod| pod.name.into())
            .mono()
            .flex(NAME_WEIGHT, NAME_MIN)
            .sort_by_key(|pod: &Pod| pod.name),
        Column::colored_text(
            "Ready",
            |pod: &Pod| pod.ready.into(),
            |pod, palette| palette.status(pod.status),
        )
        .mono()
        .fixed(READY_WIDTH),
        Column::new("Status", |pod: &Pod, _| {
            StatusText::new(pod.status, pod.phase).into_any_element()
        })
        .mono()
        .fixed(STATUS_WIDTH)
        .sort_by_key(|pod: &Pod| pod.status),
        Column::colored_text(
            "Restarts",
            |pod: &Pod| pod.restarts.to_string().into(),
            |pod, palette| {
                if pod.restarts > 0 {
                    palette.err
                } else {
                    palette.mute
                }
            },
        )
        .mono()
        .end()
        .fixed(RESTARTS_WIDTH)
        .sort_by_key(|pod: &Pod| pod.restarts),
        Column::colored_text("Node", |pod: &Pod| pod.node.into(), |_, p| p.fg2)
            .mono()
            .fixed(NODE_WIDTH),
        Column::colored_text("CPU", |pod: &Pod| pod.cpu.into(), |_, p| p.fg2)
            .mono()
            .end()
            .fixed(CPU_WIDTH),
        Column::colored_text("Mem", |pod: &Pod| pod.mem.into(), |_, p| p.fg2)
            .mono()
            .end()
            .fixed(MEM_WIDTH),
        Column::colored_text("Age", |pod: &Pod| pod.age.into(), |_, p| p.mute)
            .mono()
            .fixed(AGE_WIDTH),
    ]
}

fn section(label: &'static str, count: Option<&'static str>, body: impl IntoElement) -> AnyElement {
    let header = match count {
        Some(count) => SectionHeader::new(label).count(count),
        None => SectionHeader::new(label),
    };
    div()
        .flex()
        .flex_col()
        .gap(SPACE_4)
        .pb(SPACE_8)
        .child(header)
        .child(body)
        .child(div().pt(SPACE_8).child(Rule::horizontal()))
        .into_any_element()
}

impl Gallery {
    fn primitive_sections(&self, palette: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            self.header().into_any_element(),
            section("Status", Some("shape and colour"), Self::statuses(palette)),
            section("Buttons", None, self.buttons(cx)),
            section("Controls", None, self.controls(cx)),
            section("Key / value", None, Self::inspector(palette)),
            section(
                "Pods",
                Some("4 · click a header to sort"),
                div()
                    .flex()
                    .flex_col()
                    .child(Rule::horizontal())
                    .child(self.pods.clone()),
            ),
            section("Empty table", None, self.empty.clone()),
            section("Feedback", None, Self::feedback(cx)),
        ]
    }

    fn pane_sections(&self, palette: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            section("Charts", None, Self::charts(palette)),
            section(
                "Split · code · diff",
                Some("drag the divider"),
                self.panes(cx),
            ),
            section(
                "Dock",
                Some("click the active tab to collapse"),
                self.dock(cx),
            ),
        ]
    }
}

impl Render for Gallery {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let note: SharedString =
            "Every primitive in flock_ui::kit, drawn from theme tokens.".into();
        let sections = match self.page {
            GalleryPage::Panes => self.pane_sections(&palette, cx),
            GalleryPage::Primitives => self.primitive_sections(&palette, cx),
            GalleryPage::Overlays => self.primitive_sections(&palette, cx),
        };
        div()
            .id("gallery")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(palette.editor)
            .text_color(palette.fg)
            .font_family(theme::FONT_SANS)
            .text_size(TEXT_UI)
            .child(
                div()
                    .h(TITLE_ROW)
                    .flex_none()
                    .bg(palette.chrome)
                    .border_b(RULE)
                    .border_color(palette.line),
            )
            .child(
                div()
                    .id("gallery-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(sections)
                    .child(
                        div()
                            .px(SPACE_8)
                            .pb(SPACE_8)
                            .font_family(FONT_MONO)
                            .text_size(TEXT_MONO)
                            .text_color(palette.mute)
                            .child(note),
                    ),
            )
            .child(overlay_layer(cx))
    }
}
