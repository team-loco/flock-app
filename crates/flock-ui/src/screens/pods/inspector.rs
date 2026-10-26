use std::rc::Rc;

use flock_core::Command;
use flock_core::diagnosis::true_label;
use flock_core::pods::{
    Container, ContainerState, OWNER_ARROW, Pod, Resources, Termination, owner_segment,
    restarts_label,
};
use flock_core::related::Related;
use flock_core::{Condition, Status};
use gpui::{
    AnyElement, App, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Pixels,
    Render, SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, div,
    prelude::FluentBuilder, px,
};

use super::{INSPECTOR_ID, MISSING, PodsScreen};
use crate::kit::scroll_viewport;
use crate::kit::{
    Button, CONDITIONS, ConditionTable, Entrance, ErrorBlocks, KeyValue, Link, PanelHeader,
    PanelSection, SelectableText, Shown, Tooltip, YamlRow, swap_in,
};
use crate::screens::edit::edit_command;
use crate::screens::related::{OnPick, Pick, RELATED, inspector_section, related_count};
use crate::theme::{
    ActiveTheme, FONT_MONO, Palette, SPACE_2, SPACE_3, SPACE_4, SPACE_5, SPACE_6, TEXT_BODY,
    TEXT_MONO,
};

const REQUESTS: &str = "requests";
const LIMITS: &str = "limits";
const OWNER_LINK: &str = "pod-owner";
const INSPECTOR_LABEL: Pixels = px(96.);
const CONTAINER_LABEL: Pixels = px(80.);
const SEPARATOR: &str = " · ";
const NO_RESOURCES: &str = "none";
const DETAILS: &str = "Details";
const CONTAINERS: &str = "Containers";
const VOLUMES: &str = "Volumes";
const LOGS_BEFORE_RESTART: &str = "Logs before restart →";
const SHELL: &str = "Shell";
const CONTAINER_HEAD: &str = "pod-container-head";
const CONTAINER_STATE: &str = "pod-container-state";
const NO_RUNNING_CONTAINER: &str =
    "No container is running, so there is nothing to open a shell in";
const NO_SELECTION: &str = "Select a pod to see its containers, conditions and events.";

#[derive(PartialEq)]
pub(super) struct PodContent {
    pub(super) pod: Option<Pod>,
    pub(super) related: Option<Related>,
}

pub(super) struct PodInspector {
    screen: WeakEntity<PodsScreen>,
    pub(super) shown: Shown<PodContent>,
    entrance: Entrance,
}

impl PodInspector {
    pub(super) fn new(screen: WeakEntity<PodsScreen>) -> Self {
        Self {
            screen,
            shown: Shown::default(),
            entrance: Entrance::default(),
        }
    }

    fn header(&self, pod: &Pod) -> AnyElement {
        let running = pod
            .containers
            .iter()
            .any(|container| container.state == ContainerState::Running);
        let screen = self.screen.clone();
        let shell = Button::new("pod-shell", SHELL)
            .primary()
            .small()
            .disabled(!running)
            .when(!running, |shell| shell.tooltip(NO_RUNNING_CONTAINER))
            .on_click(move |_, window, cx| {
                let _ = screen.update(cx, |screen, cx| screen.open_terminal(window, cx));
            });
        PanelHeader::new(INSPECTOR_ID, pod.name.clone())
            .mono()
            .status(pod.status, pod.reason.clone())
            .action(shell)
            .into_any_element()
    }

    fn details(&self, pod: &Pod, palette: &Palette) -> gpui::Div {
        let labels: Vec<String> = pod
            .labels
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        div()
            .flex()
            .flex_col()
            .child(ErrorBlocks::new(pod.issue().into_iter().collect()))
            .child(PanelSection::new(INSPECTOR_ID, DETAILS).child(self.facts(pod, palette)))
            .child(PanelSection::labels(INSPECTOR_ID, labels))
            .child(self.containers_section(pod, palette))
            .child(conditions(pod, palette))
            .child(PanelSection::events(INSPECTOR_ID, pod.events.clone()).closed())
            .child(volumes(pod))
    }

    fn facts(&self, pod: &Pod, palette: &Palette) -> impl IntoElement {
        let owner = self.owner_chain(pod, palette);
        let created = format!("{} ago", flock_core::time::age(pod.age));
        let ready = div()
            .text_color(palette.status(pod.status))
            .child(format!("{} containers", pod.ready_label()));
        KeyValue::new()
            .label_width(INSPECTOR_LABEL)
            .text("Namespace", pod.namespace.clone())
            .text("Node", or_missing(pod.node.clone()))
            .text("Pod IP", or_missing(pod.ip.clone()))
            .row("Ready", ready)
            .row("Controlled by", owner)
            .text("QoS", pod.qos.label())
            .text("Service account", or_missing(pod.service_account.clone()))
            .text("Created", created)
    }

    fn owner_chain(&self, pod: &Pod, palette: &Palette) -> AnyElement {
        let chain = pod.owner_chain();
        if chain.is_empty() {
            return missing(palette);
        }
        let mut parts: Vec<AnyElement> = Vec::new();
        for (ix, target) in chain.into_iter().enumerate() {
            if ix > 0 {
                let arrow = div().text_color(palette.mute).child(OWNER_ARROW);
                parts.push(arrow.into_any_element());
            }
            let id = gpui::ElementId::NamedInteger(OWNER_LINK.into(), ix as u64);
            let screen = self.screen.clone();
            let link = Link::new(id, owner_segment(&target))
                .mono()
                .on_click(move |_, _, cx| {
                    emit(&screen, Command::OpenResource(target.clone()), cx);
                });
            parts.push(link.into_any_element());
        }
        div()
            .min_w_0()
            .flex()
            .flex_wrap()
            .font_family(FONT_MONO)
            .children(parts)
            .into_any_element()
    }

    fn containers_section(&self, pod: &Pod, palette: &Palette) -> PanelSection {
        let rows: Vec<AnyElement> = pod
            .containers
            .iter()
            .enumerate()
            .map(|(ix, container)| self.container_row(pod, ix, container, palette))
            .collect();
        let restarted = pod
            .containers
            .iter()
            .any(|container| container.last_termination.is_some());
        let count = pod.containers.len().to_string();
        let section = PanelSection::new(INSPECTOR_ID, CONTAINERS);
        let section = if restarted {
            section
                .count(format!(
                    "{count}{SEPARATOR}{}",
                    restarts_label(pod.restarts())
                ))
                .count_color(palette.warn)
        } else {
            section.count(count)
        };
        section.child(div().flex().flex_col().gap(SPACE_6).children(rows))
    }

    fn container_row(
        &self,
        pod: &Pod,
        ix: usize,
        container: &Container,
        palette: &Palette,
    ) -> AnyElement {
        let ports: Vec<AnyElement> = container
            .ports
            .iter()
            .enumerate()
            .map(|(port_ix, port)| {
                let target = pod.resource();
                let remote = port.port;
                let id =
                    gpui::ElementId::NamedInteger(format!("forward-{ix}").into(), port_ix as u64);
                let screen = self.screen.clone();
                div()
                    .flex()
                    .gap(SPACE_4)
                    .child(div().text_color(palette.fg).child(port.label()))
                    .child(Link::new(id, "Forward").on_click(move |_, _, cx| {
                        let forward = Command::PortForward {
                            service: target.clone(),
                            local: remote,
                            remote,
                        };
                        emit(&screen, forward, cx);
                    }))
                    .into_any_element()
            })
            .collect();
        let state_color = match (container.state, container.status) {
            (ContainerState::Terminated, Status::Ok) => palette.mute,
            (_, status) => palette.status(status),
        };
        let fields = container_fields(container, ports, palette);
        let name = container.name.clone();
        let screen = self.screen.clone();
        let before_restart = container.last_termination.as_ref().map(|_| {
            let link = Link::new(
                gpui::ElementId::NamedInteger("pod-logs-before-restart".into(), ix as u64),
                LOGS_BEFORE_RESTART,
            )
            .on_click(move |_, _, cx| {
                let _ = screen.update(cx, |screen, cx| {
                    screen.show_previous_logs(name.clone(), cx);
                });
            });
            div().pl(CONTAINER_LABEL + SPACE_5).child(link)
        });
        div()
            .flex()
            .flex_col()
            .gap(SPACE_3)
            .child(
                div()
                    .debug_selector(|| CONTAINER_HEAD.into())
                    .flex()
                    .flex_wrap()
                    .items_baseline()
                    .gap(SPACE_4)
                    .font_family(FONT_MONO)
                    .text_size(TEXT_MONO)
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(container.name.clone()),
                    )
                    .child(
                        div()
                            .debug_selector(|| CONTAINER_STATE.into())
                            .text_color(state_color)
                            .child(container.state_label()),
                    ),
            )
            .child(fields)
            .children(before_restart)
            .into_any_element()
    }
}

impl Render for PodInspector {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let pod = self.shown.get().and_then(|shown| shown.pod.as_ref());
        let selection = pod
            .map(|pod| format!("{}/{}", pod.namespace, pod.name))
            .unwrap_or_default();
        let motion = self.entrance.track(
            Some(&selection),
            |inspector: &mut Self| &mut inspector.entrance,
            cx,
        );
        let screen = self.screen.clone();
        let pick: OnPick = Rc::new(move |pick, _, cx| {
            if let Pick::Object(target) = pick {
                emit(&screen, Command::OpenResource(target), cx);
            }
        });
        let section = PanelSection::new(INSPECTOR_ID, RELATED).closed();
        let related = match self.shown.get().and_then(|shown| shown.related.as_ref()) {
            Some(loaded) => section
                .count(related_count(loaded).to_string())
                .child(inspector_section(Some(loaded), &pick, cx)),
            None => section.pending(),
        };
        let yaml_screen = self.screen.clone();
        let yaml_target = pod.map(Pod::resource);
        let yaml = YamlRow::new(INSPECTOR_ID, move |_, cx| {
            if let Some(target) = yaml_target.clone() {
                emit(&yaml_screen, edit_command(target), cx);
            }
        });
        let (header, body) = match pod {
            Some(pod) => (
                Some(self.header(pod)),
                self.details(pod, &palette)
                    .child(related)
                    .child(yaml)
                    .into_any_element(),
            ),
            None => (
                None,
                div()
                    .p(SPACE_6)
                    .text_color(palette.mute)
                    .child(NO_SELECTION)
                    .into_any_element(),
            ),
        };
        let panel = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(palette.panel)
            .text_size(TEXT_BODY)
            .children(header)
            .child(
                scroll_viewport(
                    gpui::ElementId::Name(format!("{INSPECTOR_ID}-{selection}").into()),
                    body,
                )
                .flex_1()
                .min_h_0(),
            );
        div()
            .size_full()
            .bg(palette.panel)
            .child(swap_in(panel, INSPECTOR_ID, motion))
    }
}

fn emit(screen: &WeakEntity<PodsScreen>, command: Command, cx: &mut App) {
    let _ = screen.update(cx, |_, cx| cx.emit(command));
}

fn container_fields(container: &Container, ports: Vec<AnyElement>, palette: &Palette) -> KeyValue {
    let resources =
        |side: &str, resources: &Resources| match (resources.label(), resources.written()) {
            (Some(label), Some(written)) => div()
                .id(SharedString::from(format!(
                    "container-{side}-{}",
                    container.name
                )))
                .tooltip(Tooltip::text(written))
                .child(value(label, palette.fg))
                .into_any_element(),
            _ => value(NO_RESOURCES, palette.mute),
        };
    let restarts_color = if container.restarts > 0 {
        palette.warn
    } else {
        palette.mute
    };
    let probes = container
        .probes_label()
        .map(|probes| value(probes, palette.fg2));
    let run = container
        .terminated
        .as_ref()
        .and_then(Termination::run)
        .map(|run| value(run, palette.fg));
    let mut fields = KeyValue::new().label_width(CONTAINER_LABEL).row(
        "restarts",
        value(container.restarts.to_string(), restarts_color),
    );
    if let Some(last) = &container.last_termination {
        fields = fields.row("last exit", value(last.detail(), palette.warn));
        if let Some(run) = last.run() {
            fields = fields.row("last run", value(run, palette.fg2));
        }
    }
    let fields = fields
        .row("image", value(container.image.clone(), palette.fg))
        .row(REQUESTS, resources(REQUESTS, &container.requests))
        .row(LIMITS, resources(LIMITS, &container.limits));
    let fields = match probes {
        Some(probes) => fields.row("probes", probes),
        None => fields,
    };
    let fields = match run {
        Some(run) => fields.row("ran", run),
        None => fields,
    };
    if ports.is_empty() {
        return fields;
    }
    let ports = div().flex().flex_col().gap(SPACE_2).children(ports);
    fields.row("ports", ports)
}

fn value(text: impl Into<SharedString>, color: gpui::Hsla) -> AnyElement {
    div()
        .min_w_0()
        .text_color(color)
        .child(SelectableText::new(text))
        .into_any_element()
}

fn conditions(pod: &Pod, palette: &Palette) -> PanelSection {
    let all_true = pod.conditions.iter().all(Condition::is_true);
    let color = if all_true { palette.ok } else { palette.mute };
    PanelSection::new(INSPECTOR_ID, CONDITIONS)
        .count(true_label(&pod.conditions))
        .count_color(color)
        .closed()
        .child(ConditionTable::new(&pod.conditions))
}

fn missing(palette: &Palette) -> AnyElement {
    div()
        .text_color(palette.mute)
        .child(MISSING)
        .into_any_element()
}

fn or_missing(value: Option<String>) -> SharedString {
    value.map_or_else(|| MISSING.into(), SharedString::from)
}

fn volumes(pod: &Pod) -> PanelSection {
    let rows = pod.volumes.iter().fold(
        KeyValue::new().label_width(INSPECTOR_LABEL),
        |grid, volume| grid.text(volume.name.clone(), volume.source.clone()),
    );
    PanelSection::new(INSPECTOR_ID, VOLUMES)
        .count(pod.volumes.len().to_string())
        .child(rows)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::Arc;

    use flock_core::{ClusterId, Preferences, Snapshot};
    use gpui::{Entity, TestAppContext, WindowHandle};

    use super::*;

    const PODS: usize = 3;

    fn snapshot(cluster: &ClusterId) -> Arc<Snapshot> {
        let pods = (0..PODS).map(|ix| Pod {
            cluster: cluster.clone(),
            namespace: "default".into(),
            name: format!("web-{ix}"),
            ..Pod::default()
        });
        Arc::new(Snapshot {
            pods: pods.collect(),
            ..Snapshot::default()
        })
    }

    fn shown(window: &WindowHandle<PodsScreen>, cx: &mut TestAppContext) -> Option<String> {
        let name = window.update(cx, |screen, _, cx| {
            let inspector = screen.inspector.read(cx);
            let shown = inspector.shown.get()?;
            shown.pod.as_ref().map(|pod| pod.name.clone())
        });
        name.ok().flatten()
    }

    #[gpui::test]
    fn the_inspector_re_renders_only_when_its_pod_changes(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::init(Some(crate::theme::Appearance::Dark), cx)
                .unwrap_or_else(|error| panic!("UI init: {error}"));
        });
        let window = cx.add_window(PodsScreen::new);
        let cluster = ClusterId::new("a");
        let snapshot = snapshot(&cluster);
        let apply = |cx: &mut TestAppContext| {
            let _ = window.update(cx, |screen, _, cx| {
                screen.set_snapshot(&snapshot);
                screen.set_context(Some(&cluster), None, &Preferences::default(), cx);
            });
        };
        apply(cx);
        assert_eq!(shown(&window, cx).as_deref(), Some("web-0"));
        let inspector: Option<Entity<PodInspector>> = window
            .update(cx, |screen, _, _| screen.inspector.clone())
            .ok();
        let Some(inspector) = inspector else {
            panic!("the Pods window");
        };
        let notified = Rc::new(Cell::new(0));
        let count = Rc::clone(&notified);
        let _observer =
            cx.update(|cx| cx.observe(&inspector, move |_, _| count.set(count.get() + 1)));
        apply(cx);
        assert_eq!(notified.get(), 0);
        let next = snapshot.pods.get(1).map(Pod::resource);
        let _ = window.update(cx, |screen, _, cx| screen.select(next, cx));
        assert_eq!(notified.get(), 1);
        assert_eq!(shown(&window, cx).as_deref(), Some("web-1"));
    }
}
