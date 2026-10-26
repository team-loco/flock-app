use flock_core::filter::node_query;
use flock_core::nodes::Node;
use flock_core::pods::PodEvent;
use flock_core::time::age;
use flock_core::units::{Amount, MEMORY_UNIT, figure};
use flock_core::{ClusterId, Command, Page};
use gpui::{
    AnyElement, Context, Entity, IntoElement, ParentElement, Render, SharedString, Styled,
    WeakEntity, Window, div,
};

use super::shell::{SHELL_LABEL, ask_shell};
use super::{
    ALL_PODS_ID, BY_REQUEST, CPU_HEADING, INSPECTOR_ID, INSPECTOR_LABEL, MEMORY_HEADING,
    NodesScreen, SHELL_ID, node_ref, node_usage, pod_line, pod_ref, pods_count,
};
use crate::kit::{
    ActionMenu, Button, CONDITIONS, ConditionTable, Inspector, KeyValue, LabelChips, Link,
    PanelSection, Shown,
};
use crate::screens::edit::edit_command;
use crate::screens::{ALL_LINK, ScreenEvent};
use crate::theme::{
    ActiveTheme, FONT_MONO, INSPECTOR_NODE, Palette, SPACE_3, TEXT_LABEL, TEXT_SMALL,
};

#[derive(PartialEq)]
pub(super) struct NodeContent {
    pub(super) node: Node,
    pub(super) cluster: Option<ClusterId>,
    pub(super) skewed: bool,
    pub(super) events: Vec<PodEvent>,
}

pub(super) struct NodeInspector {
    screen: WeakEntity<NodesScreen>,
    actions: Entity<ActionMenu>,
    pub(super) shown: Shown<NodeContent>,
}

impl NodeInspector {
    pub(super) fn new(screen: WeakEntity<NodesScreen>, actions: Entity<ActionMenu>) -> Self {
        Self {
            screen,
            actions,
            shown: Shown::default(),
        }
    }

    fn inspector(&self, shown: &NodeContent, palette: &Palette) -> AnyElement {
        let node = &shown.node;
        let skewed = shown.skewed;
        let kubelet_color = if skewed { palette.warn } else { palette.fg };
        let kubelet = div().text_color(kubelet_color).child(node.version.clone());
        let details = KeyValue::new()
            .label_width(INSPECTOR_LABEL)
            .text("pool", node.pool().to_owned())
            .text("instance", node.instance_type.clone())
            .text("zone", node.zone.clone())
            .text("internal IP", node.internal_ip.clone())
            .row("kubelet", kubelet)
            .text("OS image", node.os_image.clone())
            .text("runtime", node.runtime.clone())
            .text("age", age(node.age));
        let not_ok = node.conditions_not_ok();
        let condition_color = if not_ok > 0 {
            palette.warn
        } else {
            palette.mute
        };
        let conditions = PanelSection::new(INSPECTOR_ID, CONDITIONS)
            .count(node.conditions.len().to_string())
            .count_color(condition_color)
            .closed()
            .child(ConditionTable::new(&node.conditions));
        let taints = PanelSection::new(INSPECTOR_ID, "Taints")
            .count(node.taints.len().to_string())
            .child(LabelChips::new(node.taints.clone()));
        let filter = ScreenEvent::Filter {
            page: Page::Pods,
            tokens: vec![node_query(&node.name)],
        };
        let filter_screen = self.screen.clone();
        let all_pods = Link::new(ALL_PODS_ID, ALL_LINK).on_click(move |_, _, cx| {
            let _ = filter_screen.update(cx, |_, cx| cx.emit(filter.clone()));
        });
        let pods = PanelSection::new(INSPECTOR_ID, "Pods")
            .count(pods_count(node))
            .trailing(all_pods)
            .child(self.top_pods(shown, palette));
        let target = shown
            .cluster
            .as_ref()
            .map(|cluster| node_ref(cluster, &node.name));
        let screen = self.screen.clone();
        let yaml_screen = screen.clone();
        let yaml_target = target.clone();
        let shell = Button::new(SHELL_ID, SHELL_LABEL)
            .primary()
            .small()
            .on_click(move |_, window, cx| {
                if let Some(target) = target.clone() {
                    ask_shell(target, screen.clone(), window, cx);
                }
            });
        let inspector = Inspector::new(INSPECTOR_ID, node.name.clone())
            .mono()
            .width(INSPECTOR_NODE)
            .status(node.status, node.state.clone())
            .issues(node.issue())
            .actions(shell)
            .actions(self.actions.clone())
            .panel(PanelSection::new(INSPECTOR_ID, "Usage").child(node_usage(node, palette)))
            .panel(PanelSection::new(INSPECTOR_ID, "Details").child(details))
            .panel(taints)
            .panel(PanelSection::labels(INSPECTOR_ID, node.labels.clone()).closed())
            .panel(conditions)
            .panel(pods)
            .panel(PanelSection::events(INSPECTOR_ID, shown.events.clone()));
        match yaml_target {
            Some(target) => inspector
                .yaml(move |_, cx| {
                    let command = ScreenEvent::Command(edit_command(target.clone()));
                    let _ = yaml_screen.update(cx, |_, cx| cx.emit(command));
                })
                .into_any_element(),
            None => inspector.into_any_element(),
        }
    }

    fn top_pods(&self, shown: &NodeContent, palette: &Palette) -> AnyElement {
        let node = &shown.node;
        let heading = pod_line(
            div().child(BY_REQUEST).into_any_element(),
            CPU_HEADING,
            MEMORY_HEADING,
        )
        .font_family(crate::theme::FONT_SANS)
        .text_size(TEXT_LABEL)
        .text_color(palette.mute);
        let cluster = shown.cluster.clone();
        let screen = self.screen.clone();
        let rows = node.top_pods.iter().enumerate().map(|(ix, pod)| {
            let target = cluster.as_ref().map(|cluster| pod_ref(cluster, pod));
            let screen = screen.clone();
            let name = Link::new(
                SharedString::from(format!("node-pod-{ix}")),
                pod.name.clone(),
            )
            .mono()
            .color(palette.fg)
            .on_click(move |_, _, cx| {
                if let Some(target) = target.clone() {
                    let command = ScreenEvent::Command(Command::OpenResource(target));
                    let _ = screen.update(cx, |_, cx| cx.emit(command));
                }
            });
            let label = div()
                .flex()
                .gap(SPACE_3)
                .min_w_0()
                .overflow_hidden()
                .child(name)
                .child(
                    div()
                        .truncate()
                        .text_color(palette.mute)
                        .child(pod.namespace.clone()),
                )
                .into_any_element();
            pod_line(
                label,
                figure(pod.cpu),
                Amount::of(figure(pod.memory), MEMORY_UNIT).labelled(),
            )
            .text_color(palette.fg2)
            .into_any_element()
        });
        div()
            .flex()
            .flex_col()
            .gap(SPACE_3)
            .font_family(FONT_MONO)
            .text_size(TEXT_SMALL)
            .child(heading)
            .children(rows)
            .into_any_element()
    }
}

impl Render for NodeInspector {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        match self.shown.get() {
            Some(shown) => self.inspector(shown, &palette),
            None => div().into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::Duration;

    use flock_core::nodes::Usage;
    use flock_core::overview::Commitment;
    use flock_core::{ClusterDetail, ClusterSummary, Counts, Snapshot, Status, Uid};
    use gpui::{TestAppContext, WindowHandle};

    use super::*;

    const NODES: usize = 2;

    fn node(name: String) -> Node {
        Node {
            name,
            uid: Uid::default(),
            status: Status::Ok,
            state: String::new(),
            schedulable: true,
            instance_type: String::new(),
            zone: String::new(),
            version: String::new(),
            internal_ip: String::new(),
            os_image: String::new(),
            runtime: String::new(),
            cpu: Usage::default(),
            memory: Usage::default(),
            disk: None,
            cpu_commitment: Commitment::default(),
            memory_commitment: Commitment::default(),
            pods: 0,
            pod_capacity: 0,
            age: Duration::ZERO,
            conditions: Vec::new(),
            taints: Vec::new(),
            labels: Vec::new(),
            top_pods: Vec::new(),
        }
    }

    fn summary(cluster: &ClusterId) -> ClusterSummary {
        ClusterSummary {
            id: cluster.clone(),
            name: cluster.to_string(),
            status: Status::Ok,
            state: String::new(),
            distro: String::new(),
            region: String::new(),
            version: None,
            nodes: None,
            problems: Counts::default(),
            nav: BTreeMap::new(),
            metrics: None,
        }
    }

    fn snapshot(cluster: &ClusterId) -> Arc<Snapshot> {
        let detail = ClusterDetail {
            nodes: (0..NODES).map(|ix| node(format!("node-{ix}"))).collect(),
            ..ClusterDetail::default()
        };
        Arc::new(Snapshot {
            details: BTreeMap::from([(cluster.clone(), detail)]),
            ..Snapshot::default()
        })
    }

    fn shown(window: &WindowHandle<NodesScreen>, cx: &mut TestAppContext) -> Option<String> {
        let name = window.update(cx, |screen, _, cx| {
            let inspector = screen.inspector.read(cx);
            inspector.shown.get().map(|shown| shown.node.name.clone())
        });
        name.ok().flatten()
    }

    #[gpui::test]
    fn the_inspector_re_renders_only_when_its_node_changes(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::init(Some(crate::theme::Appearance::Dark), cx)
                .unwrap_or_else(|error| panic!("UI init: {error}"));
        });
        let window = cx.add_window(NodesScreen::new);
        let cluster = ClusterId::new("a");
        let apply = |cx: &mut TestAppContext| {
            let snapshot = snapshot(&cluster);
            let _ = window.update(cx, |screen, _, cx| {
                screen.set(&summary(&cluster), &snapshot, true, cx);
            });
        };
        apply(cx);
        let _ = window.update(cx, |screen, _, cx| {
            screen.reveal(cluster.clone(), "node-0".to_owned(), cx);
        });
        assert_eq!(shown(&window, cx).as_deref(), Some("node-0"));
        let inspector = window.update(cx, |screen, _, _| screen.inspector.clone());
        let Ok(inspector) = inspector else {
            panic!("the Nodes window");
        };
        let notified = Rc::new(Cell::new(0));
        let count = Rc::clone(&notified);
        let _observer =
            cx.update(|cx| cx.observe(&inspector, move |_, _| count.set(count.get() + 1)));
        apply(cx);
        assert_eq!(notified.get(), 0);
        let _ = window.update(cx, |screen, _, cx| {
            screen.reveal(cluster.clone(), "node-1".to_owned(), cx);
        });
        assert_eq!(notified.get(), 1);
        assert_eq!(shown(&window, cx).as_deref(), Some("node-1"));
    }
}
