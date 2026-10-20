use serde::{Deserialize, Serialize};

use crate::crd::CrdKey;
use crate::helm::HelmKey;
use crate::{ClusterId, Command, FluxKey, LogTarget, Page, ResourceRef, Snapshot, WorkloadKind};

pub const HISTORY_LIMIT: usize = 50;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub cluster: Option<ClusterId>,
    pub page: Page,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<ResourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flux_object: Option<FluxKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logs: Option<LogTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crd: Option<CrdKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helm: Option<HelmKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspecting: Option<ResourceRef>,
}

impl Default for Location {
    fn default() -> Self {
        Self::flock(Page::Overview)
    }
}

impl Location {
    pub const fn flock(page: Page) -> Self {
        Self {
            cluster: None,
            page,
            resource: None,
            flux_object: None,
            container: None,
            logs: None,
            crd: None,
            helm: None,
            inspecting: None,
        }
    }

    pub fn helm_release(key: HelmKey) -> Self {
        Self {
            helm: Some(key.clone()),
            ..Self::page_on(key.cluster, Page::Releases)
        }
    }

    pub fn crd_on(cluster: ClusterId, crd: CrdKey) -> Self {
        Self {
            crd: Some(crd),
            ..Self::page_on(cluster, Page::CustomResources)
        }
    }

    pub fn page_on(cluster: ClusterId, page: Page) -> Self {
        Self {
            cluster: Some(cluster),
            ..Self::flock(page)
        }
    }

    pub fn history_label(&self, snapshot: &Snapshot) -> String {
        let subject = match (&self.resource, &self.flux_object) {
            (Some(resource), _) => match &resource.namespace {
                Some(namespace) => format!("{namespace}/{}", resource.name),
                None => resource.name.clone(),
            },
            (None, Some(key)) => format!("{}/{}", key.namespace, key.name),
            (None, None) if self.helm.is_some() => self
                .helm
                .as_ref()
                .map(|key| format!("{}/{}", key.namespace, key.name))
                .unwrap_or_default(),
            (None, None) => self
                .crd
                .as_ref()
                .map_or_else(|| self.page.label().to_owned(), |crd| crd.kind.clone()),
        };
        let subject = if let Some(target) = &self.logs {
            format!("Logs · {}", target.label())
        } else if self.page == Page::Logs {
            let container = self.container.as_deref().unwrap_or_default();
            format!("Logs · {subject} · {container}")
        } else if self.page == Page::Compare {
            format!("Compare · {subject}")
        } else if self.page == Page::EditYaml {
            format!("Edit · {subject}")
        } else {
            subject
        };
        match &self.cluster {
            Some(cluster) => {
                let name = crate::tabs::cluster_name(cluster, snapshot);
                format!("{subject} · {name}")
            }
            None => format!("{subject} · Flock"),
        }
    }

    pub fn workload(&self) -> Option<&ResourceRef> {
        self.resource
            .as_ref()
            .filter(|_| !matches!(self.page, Page::Compare | Page::Logs | Page::EditYaml))
            .filter(|target| WorkloadKind::from_kind(&target.kind).is_some())
    }

    pub fn editing(&self) -> Option<&ResourceRef> {
        self.resource
            .as_ref()
            .filter(|_| self.page == Page::EditYaml)
    }

    pub fn detail(&self) -> Option<&ResourceRef> {
        self.resource
            .as_ref()
            .filter(|_| !matches!(self.page, Page::Compare | Page::Logs | Page::EditYaml))
            .filter(|target| crate::related::DETAIL_KINDS.contains(&target.kind.as_str()))
    }

    pub fn commands(&self) -> Vec<Command> {
        let mut commands = self.reach();
        if let Some(target) = &self.inspecting {
            commands.push(Command::InspectResource(target.clone()));
        }
        commands
    }

    fn reach(&self) -> Vec<Command> {
        if let Some(key) = &self.flux_object {
            return vec![Command::OpenFluxObject(key.clone())];
        }
        if let Some(target) = &self.logs {
            return vec![Command::FollowLogs(target.clone())];
        }
        if let Some(key) = &self.helm {
            return vec![Command::OpenHelmRelease(key.clone())];
        }
        match (&self.resource, self.page, &self.container) {
            (Some(pod), Page::Logs, Some(container)) => {
                return vec![Command::OpenLogs {
                    pod: pod.clone(),
                    container: container.clone(),
                }];
            }
            (Some(resource), Page::Compare, _) => {
                return vec![Command::Compare(resource.clone())];
            }
            (Some(resource), Page::EditYaml, _) => {
                return vec![Command::EditYaml(resource.clone())];
            }
            (Some(resource), _, _) => return vec![Command::OpenResource(resource.clone())],
            (None, _, _) => {}
        }
        if let (Some(cluster), Some(crd)) = (&self.cluster, &self.crd) {
            return vec![Command::OpenCrd {
                cluster: cluster.clone(),
                crd: crd.clone(),
            }];
        }
        match &self.cluster {
            Some(cluster) if !self.page.is_flock() => {
                let select = Command::SelectCluster(Some(cluster.clone()));
                if self.page == Page::ClusterOverview {
                    vec![select]
                } else {
                    vec![select, Command::SelectPage(self.page)]
                }
            }
            Some(_) => vec![Command::SelectPage(self.page)],
            None => vec![Command::SelectPage(self.page)],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Back,
    Forward,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    current: Option<Location>,
    back: Vec<Location>,
    forward: Vec<Location>,
}

impl History {
    pub fn starting_at(location: Location) -> Self {
        Self {
            current: Some(location),
            ..Self::default()
        }
    }

    pub fn visit(&mut self, location: Location) {
        if self.current.as_ref() == Some(&location) {
            return;
        }
        if let Some(previous) = self.current.replace(location) {
            self.back.push(previous);
            trim(&mut self.back);
        }
        self.forward.clear();
    }

    pub fn back(&mut self) -> Option<Location> {
        let target = self.back.pop()?;
        if let Some(current) = self.current.replace(target.clone()) {
            self.forward.push(current);
        }
        Some(target)
    }

    pub fn forward(&mut self) -> Option<Location> {
        let target = self.forward.pop()?;
        if let Some(current) = self.current.replace(target.clone()) {
            self.back.push(current);
        }
        Some(target)
    }

    pub fn entries(&self, direction: Direction) -> impl ExactSizeIterator<Item = &Location> {
        let stack = match direction {
            Direction::Back => &self.back,
            Direction::Forward => &self.forward,
        };
        stack.iter().rev()
    }

    pub fn jump(&mut self, direction: Direction, steps: usize) -> Option<Location> {
        if steps == 0 || steps > self.entries(direction).len() {
            return None;
        }
        let mut target = None;
        for _ in 0..steps {
            target = match direction {
                Direction::Back => self.back(),
                Direction::Forward => self.forward(),
            };
        }
        target
    }

    pub fn amend(&mut self, location: Location) {
        self.current = Some(location);
    }

    pub fn current(&self) -> Option<&Location> {
        self.current.as_ref()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }
}

fn trim(stack: &mut Vec<Location>) {
    let excess = stack.len().saturating_sub(HISTORY_LIMIT);
    stack.drain(..excess);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(page: Page) -> Location {
        Location {
            cluster: (!page.is_flock()).then(|| ClusterId::new("a")),
            ..Location::flock(page)
        }
    }

    #[test]
    fn only_an_edit_yaml_location_is_editing_its_resource() {
        let target = ResourceRef {
            cluster: ClusterId::new("a"),
            kind: "ConfigMap".into(),
            namespace: Some("web".into()),
            name: "settings".into(),
        };
        let editing = Location {
            resource: Some(target.clone()),
            ..page(Page::EditYaml)
        };
        assert_eq!(editing.editing(), Some(&target));
        let listed = Location {
            resource: Some(target),
            ..page(Page::ConfigMaps)
        };
        assert_eq!(listed.editing(), None);
        assert_eq!(page(Page::EditYaml).editing(), None);
    }

    #[test]
    fn history_entries_and_jumps_preserve_the_opposite_stack() {
        let mut history = History::starting_at(page(Page::Overview));
        for p in [Page::Nodes, Page::Pods, Page::Events] {
            history.visit(page(p));
        }
        let entries: Vec<_> = history
            .entries(Direction::Back)
            .map(|entry| entry.page)
            .collect();
        assert_eq!(entries, [Page::Pods, Page::Nodes, Page::Overview]);
        let unchanged = history.clone();
        assert_eq!(history.jump(Direction::Back, 0), None);
        assert_eq!(history.jump(Direction::Back, HISTORY_LIMIT), None);
        assert_eq!(history, unchanged);
        assert_eq!(history.jump(Direction::Back, 2), Some(page(Page::Nodes)));
        assert!(history.can_go_forward());
        let entries: Vec<_> = history
            .entries(Direction::Forward)
            .map(|entry| entry.page)
            .collect();
        assert_eq!(entries, [Page::Pods, Page::Events]);
        assert_eq!(
            history.jump(Direction::Forward, 2),
            Some(page(Page::Events))
        );
        history.back();
        history.visit(page(Page::Namespaces));
        assert_eq!(history.entries(Direction::Forward).len(), 0);
        assert!(!history.can_go_forward());
    }

    #[test]
    fn history_labels_identify_pages_objects_logs_and_missing_clusters() {
        let snapshot = Snapshot::default();
        assert_eq!(
            Location::default().history_label(&snapshot),
            "Overview · Flock"
        );
        assert_eq!(page(Page::Nodes).history_label(&snapshot), "Nodes · a");
        let resource = ResourceRef {
            cluster: ClusterId::new("a"),
            kind: "Pod".into(),
            namespace: Some("shop".into()),
            name: "cart-abc".into(),
        };
        let mut location = Location {
            resource: Some(resource),
            ..page(Page::Pods)
        };
        assert_eq!(location.history_label(&snapshot), "shop/cart-abc · a");
        location.page = Page::Logs;
        location.container = Some("api".into());
        assert_eq!(
            location.history_label(&snapshot),
            "Logs · shop/cart-abc · api · a"
        );
        location.page = Page::Compare;
        assert_eq!(
            location.history_label(&snapshot),
            "Compare · shop/cart-abc · a"
        );
        location.resource = None;
        location.flux_object = Some(FluxKey {
            cluster: ClusterId::new("a"),
            kind: crate::FluxKind::Kustomization,
            namespace: "flux-system".into(),
            name: "infra".into(),
        });
        location.page = Page::GitOps;
        assert_eq!(location.history_label(&snapshot), "flux-system/infra · a");
    }

    #[test]
    fn history_labels_use_the_clusters_display_name() {
        let mut snapshot = Snapshot::default();
        let mut cluster =
            crate::flock::tests::cluster("a", crate::Status::Ok, crate::Counts::default());
        cluster.name = "prod-us-east".into();
        snapshot.clusters.push(cluster);
        assert_eq!(
            page(Page::Nodes).history_label(&snapshot),
            "Nodes · prod-us-east"
        );
    }

    #[test]
    fn back_and_forward_walk_the_visits() {
        let mut history = History::default();
        for p in [Page::Overview, Page::Nodes, Page::Pods] {
            history.visit(page(p));
        }
        assert_eq!(history.back(), Some(page(Page::Nodes)));
        history.visit(page(Page::Nodes));
        assert_eq!(history.back(), Some(page(Page::Overview)));
        assert_eq!(history.back(), None);
        assert_eq!(history.forward(), Some(page(Page::Nodes)));
        assert_eq!(history.forward(), Some(page(Page::Pods)));
        assert_eq!(history.forward(), None);
        history.back();
        history.visit(page(Page::Events));
        assert_eq!(history.forward(), None);
        assert_eq!(history.back(), Some(page(Page::Nodes)));
    }

    #[test]
    fn repeated_visits_to_the_same_place_are_one_entry() {
        let mut history = History::starting_at(page(Page::Overview));
        history.visit(page(Page::Overview));
        assert!(!history.can_go_back());
        assert_eq!(history.current(), Some(&page(Page::Overview)));
    }

    #[test]
    fn history_is_bounded() {
        let mut history = History::default();
        let pages = [Page::Nodes, Page::Pods];
        for ix in 0..=HISTORY_LIMIT * 2 {
            history.visit(page(pages[ix % 2]));
        }
        let mut steps = 0;
        while history.back().is_some() {
            steps += 1;
        }
        assert_eq!(steps, HISTORY_LIMIT);
    }

    #[test]
    fn locations_restore_through_commands() {
        let nodes = page(Page::Nodes);
        assert_eq!(
            nodes.commands(),
            [
                Command::SelectCluster(Some(ClusterId::new("a"))),
                Command::SelectPage(Page::Nodes)
            ]
        );
        assert_eq!(
            page(Page::GitOps).commands(),
            [Command::SelectPage(Page::GitOps)]
        );
        let pod = ResourceRef {
            cluster: ClusterId::new("a"),
            kind: "Pod".into(),
            namespace: Some("shop".into()),
            name: "cart".into(),
        };
        let open = Location {
            resource: Some(pod.clone()),
            ..page(Page::Pods)
        };
        assert_eq!(open.commands(), [Command::OpenResource(pod.clone())]);
        let logs = Location {
            resource: Some(pod.clone()),
            container: Some("api".into()),
            ..page(Page::Logs)
        };
        assert_eq!(
            logs.commands(),
            [Command::OpenLogs {
                pod: pod.clone(),
                container: "api".into()
            }]
        );
        let compare = Location {
            resource: Some(pod.clone()),
            ..page(Page::Compare)
        };
        assert_eq!(compare.commands(), [Command::Compare(pod.clone())]);
        let inspected = Location {
            inspecting: Some(pod.clone()),
            ..page(Page::Pods)
        };
        assert_eq!(
            inspected.commands(),
            [
                Command::SelectCluster(Some(ClusterId::new("a"))),
                Command::SelectPage(Page::Pods),
                Command::InspectResource(pod)
            ]
        );
        let workload = ResourceRef {
            cluster: ClusterId::new("a"),
            kind: "Deployment".into(),
            namespace: Some("shop".into()),
            name: "cart".into(),
        };
        let merged = Location {
            logs: Some(LogTarget::Workload(workload.clone())),
            ..page(Page::Logs)
        };
        assert_eq!(
            merged.commands(),
            [Command::FollowLogs(LogTarget::Workload(workload))]
        );
        assert_eq!(
            merged.history_label(&Snapshot::default()),
            "Logs · cart · a"
        );
        let mut state = crate::AppState::default();
        state.update(crate::Update::Snapshot(Box::new(
            crate::flock::tests::sample(),
        )));
        for command in nodes.commands() {
            state.apply(&command);
        }
        assert_eq!(state.location(), &nodes);
    }
}
