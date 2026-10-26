use std::rc::Rc;

use flock_core::related::Related;
use flock_core::workload::{ContainerSpec, Pod};
use flock_core::{Command, Workload};
use gpui::{
    AnyElement, App, Context, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, WeakEntity, Window, div,
};

use super::{
    CONTAINER, INSPECTOR_ID, SPEC, WorkloadScreen, conditions_section, env_section, with_written,
};
use crate::kit::{
    EVENTS, ErrorBlocks, EventList, KeyValue, PanelSection, SectionLabel, Segmented,
    SelectableText, Shown, YamlRow,
};
use crate::screens::related::{OnPick, Pick, RELATED, inspector_section, related_count};
use crate::theme::{ActiveTheme, Palette, RULE, SECTION_HEADER, SPACE_6, TEXT_BODY};

#[derive(PartialEq)]
pub(super) struct WorkloadContent {
    pub(super) workload: Workload,
    pub(super) container: usize,
    pub(super) pod: Option<Pod>,
    pub(super) related: Option<Related>,
}

pub(super) struct WorkloadInspector {
    screen: WeakEntity<WorkloadScreen>,
    pub(super) shown: Shown<WorkloadContent>,
}

impl WorkloadInspector {
    pub(super) fn new(screen: WeakEntity<WorkloadScreen>) -> Self {
        Self {
            screen,
            shown: Shown::default(),
        }
    }

    fn inspector(&self, shown: &WorkloadContent, cx: &App) -> AnyElement {
        let palette = *cx.palette();
        let workload = &shown.workload;
        let names: Vec<String> = workload.containers.iter().map(|c| c.name.clone()).collect();
        let segmented = names.into_iter().fold(
            Segmented::new("containers")
                .small()
                .mono()
                .selected(shown.container),
            Segmented::option,
        );
        let container_screen = self.screen.clone();
        let segmented = segmented.on_select(move |ix: &usize, _, cx| {
            let _ = container_screen.update(cx, |screen, cx| screen.select_container(*ix, cx));
        });
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .h(SECTION_HEADER)
            .px(SPACE_6)
            .border_b(RULE)
            .border_color(palette.line)
            .child(div().flex_1().child(SectionLabel::new(CONTAINER)))
            .child(segmented);
        let selected = workload.containers.get(shown.container);
        let spec = selected.map(|spec| self.spec(spec, &palette));
        let env = selected.map(|spec| env_section(&spec.env, &palette));
        let related = self.related_section(cx);
        let screen = self.screen.clone();
        let events = PanelSection::new(INSPECTOR_ID, EVENTS)
            .count(workload.events.len().to_string())
            .child(EventList::new(workload.events.clone()));
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
            .child(ErrorBlocks::new(workload.issues()))
            .child(conditions_section(&workload.conditions, &palette))
            .child(header)
            .children(spec)
            .children(env)
            .child(related)
            .child(events)
            .child(YamlRow::new(INSPECTOR_ID, move |_, cx| {
                screen.update(cx, WorkloadScreen::toggle_yaml).ok();
            }));
        content.into_any_element()
    }

    fn spec(&self, spec: &ContainerSpec, palette: &Palette) -> PanelSection {
        let ports: Vec<String> = spec
            .ports
            .iter()
            .map(flock_core::workload::Port::label)
            .collect();
        let last_exit = self
            .shown
            .get()
            .and_then(|shown| shown.pod.as_ref())
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
            .row(
                "requests",
                with_written(
                    format!("spec-requests-{}", spec.name),
                    Some(spec.requests.written()),
                    div().child(SelectableText::new(spec.requests.label())),
                ),
            )
            .row(
                "limits",
                with_written(
                    format!("spec-limits-{}", spec.name),
                    Some(spec.limits.written()),
                    div().child(SelectableText::new(spec.limits.label())),
                ),
            );
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
        PanelSection::new(INSPECTOR_ID, SPEC).child(grid)
    }

    fn related_section(&self, cx: &App) -> PanelSection {
        let screen = self.screen.clone();
        let pick: OnPick = Rc::new(move |pick, _, cx| {
            if let Pick::Object(target) = pick {
                let _ = screen.update(cx, |_, cx| cx.emit(Command::OpenResource(target)));
            }
        });
        let section = PanelSection::new(INSPECTOR_ID, RELATED).closed();
        match self.shown.get().and_then(|shown| shown.related.as_ref()) {
            Some(related) => section
                .count(related_count(related).to_string())
                .child(inspector_section(Some(related), &pick, cx)),
            None => section.pending(),
        }
    }
}

impl Render for WorkloadInspector {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.shown.get() {
            Some(shown) => self.inspector(shown, cx),
            None => div().into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::time::Duration;

    use flock_core::workload::WorkloadKind;
    use flock_core::{ClusterId, ResourceRef, Status, Uid};
    use gpui::{TestAppContext, WindowHandle};

    use super::*;

    const PODS: usize = 2;

    fn pod(name: String) -> Pod {
        Pod {
            name,
            phase: String::new(),
            status: Status::Ok,
            node: String::new(),
            cpu_millis: None,
            memory_bytes: None,
            age: Duration::ZERO,
            containers: Vec::new(),
        }
    }

    fn workload() -> Workload {
        Workload {
            target: ResourceRef {
                cluster: ClusterId::new("a"),
                kind: "Deployment".to_owned(),
                namespace: Some("default".to_owned()),
                name: "web".to_owned(),
            },
            uid: Uid::default(),
            kind: WorkloadKind::Deployment,
            desired: 2,
            conditions: Vec::new(),
            pending: None,
            managed_by: None,
            pods: (0..PODS).map(|ix| pod(format!("web-{ix}"))).collect(),
            revisions: Vec::new(),
            history_pending: false,
            containers: Vec::new(),
            events: Vec::new(),
        }
    }

    fn shown_pod(window: &WindowHandle<WorkloadScreen>, cx: &mut TestAppContext) -> Option<String> {
        let name = window.update(cx, |screen, _, cx| {
            let inspector = screen.inspector.read(cx);
            let shown = inspector.shown.get()?;
            shown.pod.as_ref().map(|pod| pod.name.clone())
        });
        name.ok().flatten()
    }

    #[gpui::test]
    fn the_inspector_re_renders_only_when_what_it_shows_changes(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::init(Some(crate::theme::Appearance::Dark), cx)
                .unwrap_or_else(|error| panic!("UI init: {error}"));
        });
        let target = workload().target;
        let window = cx.add_window(|window, cx| WorkloadScreen::new(target, window, cx));
        let receive = |cx: &mut TestAppContext| {
            let _ = window.update(cx, |screen, _, cx| screen.receive(workload(), cx));
        };
        receive(cx);
        assert_eq!(shown_pod(&window, cx).as_deref(), Some("web-0"));
        let inspector = window.update(cx, |screen, _, _| screen.inspector.clone());
        let Ok(inspector) = inspector else {
            panic!("the Workload window");
        };
        let notified = Rc::new(Cell::new(0));
        let count = Rc::clone(&notified);
        let _observer =
            cx.update(|cx| cx.observe(&inspector, move |_, _| count.set(count.get() + 1)));
        receive(cx);
        assert_eq!(notified.get(), 0);
        let _ = window.update(cx, |screen, _, cx| {
            screen.select_pod("web-1".to_owned(), cx);
        });
        assert_eq!(notified.get(), 1);
        assert_eq!(shown_pod(&window, cx).as_deref(), Some("web-1"));
    }
}
