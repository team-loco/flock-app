use flock_core::{Command, LogRequest, ResourceRef, Sink};
use gpui::{
    AppContext, Context, Entity, EventEmitter, IntoElement, ParentElement, Render, Styled,
    Subscription, Task, Window, div,
};

use crate::kit::{
    Button, InputEvent, LOG_FILTER_PLACEHOLDER, LogView, MetaLine, PageHeader, TextField,
    TextFieldState,
};
use crate::screens::workload::kit_line;
use crate::theme::{ActiveTheme, RULE, SPACE_3, SPACE_4, SPACE_8};

const FOLLOW: &str = "follow";
const WRAP: &str = "wrap";
const META_SEPARATOR: &str = " · ";

pub struct LogsScreen {
    pub(crate) navigation: Option<Entity<crate::kit::HeaderNavigation>>,
    pod: ResourceRef,
    container: String,
    logs: Entity<LogView>,
    filter: Entity<TextFieldState>,
    stream: Option<Task<()>>,
    compact: bool,
    _filter_events: Subscription,
}

impl EventEmitter<Command> for LogsScreen {}

impl LogsScreen {
    pub fn new(
        pod: ResourceRef,
        container: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let logs = cx.new(|_| LogView::new("tab-logs"));
        let filter =
            cx.new(|cx| TextFieldState::new(window, cx).placeholder(LOG_FILTER_PLACEHOLDER));
        let filter_events = cx.subscribe(&filter, |this, filter, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                let query = filter.read(cx).value();
                this.logs.update(cx, |logs, cx| logs.set_query(query, cx));
            }
        });
        Self {
            navigation: None,
            pod,
            container,
            logs,
            filter,
            stream: None,
            compact: false,
            _filter_events: filter_events,
        }
    }

    pub fn set_compact(&mut self, compact: bool, cx: &mut Context<Self>) {
        if self.compact != compact {
            self.compact = compact;
            cx.notify();
        }
    }

    pub fn shows(&self, pod: &ResourceRef, container: &str) -> bool {
        &self.pod == pod && self.container == container
    }

    pub fn watch(&mut self, cx: &mut Context<Self>) {
        let (sink, lines) = Sink::channel();
        self.logs.update(cx, |logs, cx| {
            logs.clear(cx);
            logs.set_follow(true, cx);
        });
        cx.emit(Command::StreamLogs(LogRequest {
            pod: self.pod.clone(),
            container: self.container.clone(),
            instance: flock_core::LogInstance::Current,
            sink,
        }));
        self.stream = Some(cx.spawn(async move |this, cx| {
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

    fn tools(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let follow = self.logs.read(cx).follow();
        let wrap = self.logs.read(cx).wrap();
        div()
            .flex()
            .items_center()
            .gap(SPACE_3)
            .child(TextField::search(&self.filter))
            .child(
                Button::new("tab-logs-follow", FOLLOW)
                    .small()
                    .selected(follow)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.logs
                            .update(cx, |logs, cx| logs.set_follow(!follow, cx));
                        cx.notify();
                    })),
            )
            .child(
                Button::new("tab-logs-wrap", WRAP)
                    .small()
                    .selected(wrap)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.logs.update(cx, |logs, cx| logs.set_wrap(!wrap, cx));
                        cx.notify();
                    })),
            )
    }
}

impl Render for LogsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        if self.compact {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .bg(palette.editor)
                .child(
                    crate::kit::PageTitle::new(self.pod.name.clone())
                        .leading(self.navigation.clone()),
                )
                .child(div().flex_1().min_h_0().child(self.logs.clone()));
        }
        let facts = [
            self.container.as_str(),
            self.pod.namespace.as_deref().unwrap_or_default(),
            self.pod.cluster.as_str(),
        ];
        let meta = MetaLine::new(facts.join(META_SEPARATOR));
        let header = PageHeader::new(self.pod.name.clone())
            .leading(self.navigation.clone())
            .mono()
            .meta(meta);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette.editor)
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .px(SPACE_8)
                    .py(SPACE_4)
                    .border_b(RULE)
                    .border_color(palette.line)
                    .child(self.tools(cx)),
            )
            .child(div().flex_1().min_h_0().child(self.logs.clone()))
    }
}
