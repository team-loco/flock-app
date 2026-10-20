use serde::{Deserialize, Serialize};

use crate::history::{History, Location};
use crate::{ClusterId, Page, Snapshot, Status};

pub const LOGS_KIND: &str = "Logs";
pub const COMPARE_KIND: &str = "Compare";
pub const EDIT_KIND: &str = "Edit";
pub const CLUSTER_KIND: &str = "Cluster";
pub const CRD_KIND: &str = "Custom";
pub const HELM_KIND: &str = "Helm";
pub const MAX_PANES: usize = 2;
const PER_MILLE: u16 = 1000;
const EVEN: u16 = PER_MILLE / 2;
const NARROWEST: u16 = 100;
const NAME_SEPARATOR: char = '-';
const SHORT_NAME_MIN: usize = 3;
const ENVIRONMENT_PREFIXES: [&str; 11] = [
    "prod",
    "production",
    "preprod",
    "staging",
    "stage",
    "stg",
    "dev",
    "development",
    "qa",
    "test",
    "uat",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TabId(u64);

impl TabId {
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PaneId(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Divider(u16);

impl Default for Divider {
    fn default() -> Self {
        Self(EVEN)
    }
}

impl Divider {
    pub fn at(share: f32) -> Self {
        if !share.is_finite() {
            return Self::default();
        }
        let scaled = (share * f32::from(PER_MILLE)).round();
        let clamped = scaled.clamp(f32::from(NARROWEST), f32::from(PER_MILLE - NARROWEST));
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let per_mille = clamped as u16;
        Self(per_mille)
    }

    pub fn share(self) -> f32 {
        f32::from(self.0) / f32::from(PER_MILLE)
    }

    fn checked(self) -> Self {
        Self(self.0.clamp(NARROWEST, PER_MILLE - NARROWEST))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    id: TabId,
    location: Location,
    history: History,
}

impl Tab {
    fn new(id: TabId, location: Location) -> Self {
        Self {
            id,
            history: History::starting_at(location.clone()),
            location,
        }
    }

    pub const fn id(&self) -> TabId {
        self.id
    }

    pub const fn location(&self) -> &Location {
        &self.location
    }

    pub const fn history(&self) -> &History {
        &self.history
    }

    pub fn can_go_forward(&self) -> bool {
        self.history.can_go_forward()
    }

    pub fn can_go_back(&self) -> bool {
        self.history.can_go_back()
    }

    pub(crate) fn location_mut(&mut self) -> &mut Location {
        &mut self.location
    }

    pub(crate) fn record(&mut self) {
        self.history.visit(self.location.clone());
    }

    pub(crate) fn history_mut(&mut self) -> &mut History {
        &mut self.history
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cycle {
    Next,
    Previous,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pane {
    id: PaneId,
    tabs: Vec<Tab>,
    active: usize,
}

impl Pane {
    pub const fn id(&self) -> PaneId {
        self.id
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub const fn active_index(&self) -> usize {
        self.active
    }

    pub fn active(&self) -> &Tab {
        match self.tabs.get(self.active) {
            Some(tab) => tab,
            None => unreachable!("a pane always has an active tab"),
        }
    }

    fn active_mut(&mut self) -> &mut Tab {
        match self.tabs.get_mut(self.active) {
            Some(tab) => tab,
            None => unreachable!("a pane always has an active tab"),
        }
    }

    fn position(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Panes {
    all: Vec<Pane>,
    focused: usize,
    divider: Divider,
    next_id: u64,
    next_pane: u64,
}

impl Default for Panes {
    fn default() -> Self {
        Self::restore(&SavedWindow::default())
    }
}

impl Panes {
    pub fn restore(saved: &SavedWindow) -> Self {
        let mut panes = Self {
            all: Vec::new(),
            focused: 0,
            divider: saved.divider.checked(),
            next_id: 0,
            next_pane: 0,
        };
        for pane in saved.panes.iter().take(MAX_PANES) {
            let tabs: Vec<Tab> = pane
                .tabs
                .iter()
                .map(|location| {
                    let id = panes.allocate();
                    Tab::new(id, location.clone())
                })
                .collect();
            if !tabs.is_empty() {
                let active = pane.active.min(tabs.len() - 1);
                let id = panes.allocate_pane();
                panes.all.push(Pane { id, tabs, active });
            }
        }
        if panes.all.is_empty() {
            let pane = panes.pane_at(Location::default());
            panes.all.push(pane);
        }
        panes.focused = saved.focused.min(panes.all.len() - 1);
        panes
    }

    pub fn saved(&self) -> SavedWindow {
        SavedWindow {
            panes: self
                .all
                .iter()
                .map(|pane| SavedPane {
                    tabs: pane.tabs.iter().map(|tab| tab.location.clone()).collect(),
                    active: pane.active,
                })
                .collect(),
            focused: self.focused,
            divider: self.divider,
        }
    }

    pub const fn divider(&self) -> Divider {
        self.divider
    }

    pub fn set_divider(&mut self, divider: Divider) -> bool {
        let changed = self.divider != divider;
        self.divider = divider;
        changed
    }

    pub fn panes(&self) -> &[Pane] {
        &self.all
    }

    pub const fn focused_index(&self) -> usize {
        self.focused
    }

    pub fn focused(&self) -> &Pane {
        match self.all.get(self.focused) {
            Some(pane) => pane,
            None => unreachable!("a window always has a focused pane"),
        }
    }

    fn focused_mut(&mut self) -> &mut Pane {
        match self.all.get_mut(self.focused) {
            Some(pane) => pane,
            None => unreachable!("a window always has a focused pane"),
        }
    }

    pub fn active(&self) -> &Tab {
        self.focused().active()
    }

    pub(crate) fn active_mut(&mut self) -> &mut Tab {
        self.focused_mut().active_mut()
    }

    pub(crate) fn tabs_mut(&mut self) -> impl Iterator<Item = &mut Tab> {
        self.all.iter_mut().flat_map(|pane| pane.tabs.iter_mut())
    }

    fn allocate(&mut self) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;
        id
    }

    fn allocate_pane(&mut self) -> PaneId {
        let id = PaneId(self.next_pane);
        self.next_pane += 1;
        id
    }

    fn pane_at(&mut self, location: Location) -> Pane {
        let tab = Tab::new(self.allocate(), location);
        Pane {
            id: self.allocate_pane(),
            tabs: vec![tab],
            active: 0,
        }
    }

    pub fn pane_of(&self, id: TabId) -> Option<usize> {
        self.all.iter().position(|pane| pane.position(id).is_some())
    }

    pub fn focus(&mut self, ix: usize) -> bool {
        if ix >= self.all.len() || ix == self.focused {
            return false;
        }
        self.focused = ix;
        true
    }

    pub fn split(&mut self) {
        let location = self.active().location.clone();
        self.split_to(location);
    }

    pub fn split_to(&mut self, location: Location) {
        if self.all.len() < MAX_PANES {
            let pane = self.pane_at(location);
            let at = self.focused + 1;
            self.all.insert(at, pane);
            self.focused = at;
            return;
        }
        self.focused = (self.focused + 1) % self.all.len();
        self.open(location);
    }

    pub fn open(&mut self, location: Location) -> TabId {
        let pane = self.focused();
        let existing = pane
            .tabs
            .iter()
            .find(|tab| tab.location == location)
            .map(Tab::id);
        if let Some(id) = existing {
            self.activate(id);
            return id;
        }
        self.add(location)
    }

    pub fn add(&mut self, location: Location) -> TabId {
        let id = self.allocate();
        let pane = self.focused_mut();
        let at = pane.active + 1;
        pane.tabs.insert(at, Tab::new(id, location));
        pane.active = at;
        id
    }

    pub fn activate(&mut self, id: TabId) -> bool {
        let found = self
            .all
            .iter()
            .enumerate()
            .find_map(|(pane_ix, pane)| pane.position(id).map(|tab_ix| (pane_ix, tab_ix)));
        let Some((pane_ix, tab_ix)) = found else {
            return false;
        };
        self.focused = pane_ix;
        if let Some(pane) = self.all.get_mut(pane_ix) {
            pane.active = tab_ix;
        }
        true
    }

    pub fn activate_nth(&mut self, ix: usize) -> bool {
        let pane = self.focused_mut();
        if ix >= pane.tabs.len() {
            return false;
        }
        pane.active = ix;
        true
    }

    pub fn cycle(&mut self, direction: Cycle) -> bool {
        let pane = self.focused_mut();
        let len = pane.tabs.len();
        if len < 2 {
            return false;
        }
        pane.active = match direction {
            Cycle::Next => (pane.active + 1) % len,
            Cycle::Previous => (pane.active + len - 1) % len,
        };
        true
    }

    pub fn is_last(&self, id: TabId) -> bool {
        match self.all.as_slice() {
            [pane] => pane.tabs.len() == 1 && pane.position(id).is_some(),
            _ => false,
        }
    }

    pub fn close(&mut self, id: TabId) -> bool {
        if self.is_last(id) {
            return false;
        }
        let Some(pane_ix) = self.pane_of(id) else {
            return false;
        };
        if self.take(pane_ix, id).is_none() {
            return false;
        }
        self.drop_if_empty(pane_ix);
        true
    }

    fn take(&mut self, pane_ix: usize, id: TabId) -> Option<Tab> {
        let pane = self.all.get_mut(pane_ix)?;
        let ix = pane.position(id)?;
        let tab = pane.tabs.remove(ix);
        if ix < pane.active || pane.active >= pane.tabs.len() {
            pane.active = pane.active.saturating_sub(1);
        }
        Some(tab)
    }

    fn drop_if_empty(&mut self, pane_ix: usize) -> bool {
        let empty = self
            .all
            .get(pane_ix)
            .is_some_and(|pane| pane.tabs.is_empty());
        if !empty || self.all.len() == 1 {
            return false;
        }
        self.all.remove(pane_ix);
        if pane_ix < self.focused || self.focused >= self.all.len() {
            self.focused = self.focused.saturating_sub(1);
        }
        true
    }

    pub fn split_off(&mut self, id: TabId) -> bool {
        let Some(pane_ix) = self.pane_of(id) else {
            return false;
        };
        let alone = self.all.get(pane_ix).is_none_or(|pane| pane.tabs.len() < 2);
        if self.all.len() >= MAX_PANES || alone {
            return false;
        }
        let Some(tab) = self.take(pane_ix, id) else {
            return false;
        };
        let pane = Pane {
            id: self.allocate_pane(),
            tabs: vec![tab],
            active: 0,
        };
        self.all.insert(pane_ix + 1, pane);
        self.focused = pane_ix + 1;
        true
    }

    pub fn join(&mut self, id: TabId) -> bool {
        if self.all.len() < 2 || self.pane_of(id).is_none() {
            return false;
        }
        let mut panes = std::mem::take(&mut self.all).into_iter();
        let Some(mut joined) = panes.next() else {
            return false;
        };
        for pane in panes {
            joined.tabs.extend(pane.tabs);
        }
        joined.active = joined.position(id).unwrap_or_default();
        self.all.push(joined);
        self.focused = 0;
        true
    }

    pub fn move_tab(&mut self, id: TabId, pane: usize, to: usize) -> bool {
        let Some(from_pane) = self.pane_of(id) else {
            return false;
        };
        if pane >= self.all.len() {
            return false;
        }
        if from_pane == pane {
            return self.reorder(pane, id, to);
        }
        let Some(tab) = self.take(from_pane, id) else {
            return false;
        };
        let target = if self.drop_if_empty(from_pane) && from_pane < pane {
            pane - 1
        } else {
            pane
        };
        let Some(destination) = self.all.get_mut(target) else {
            return false;
        };
        let at = to.min(destination.tabs.len());
        destination.tabs.insert(at, tab);
        destination.active = at;
        self.focused = target;
        true
    }

    fn reorder(&mut self, pane: usize, id: TabId, to: usize) -> bool {
        let Some(pane) = self.all.get_mut(pane) else {
            return false;
        };
        let Some(from) = pane.position(id) else {
            return false;
        };
        let to = to.min(pane.tabs.len() - 1);
        if from == to {
            return false;
        }
        let active = pane.active().id;
        let tab = pane.tabs.remove(from);
        pane.tabs.insert(to, tab);
        pane.active = pane.position(active).unwrap_or(to);
        true
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedPane {
    #[serde(deserialize_with = "lenient::locations")]
    pub tabs: Vec<Location>,
    pub active: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedWindow {
    pub panes: Vec<SavedPane>,
    pub focused: usize,
    pub divider: Divider,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub windows: Vec<SavedWindow>,
}

mod lenient {
    use serde::{Deserialize, Deserializer, de::IgnoredAny};

    use crate::history::Location;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Entry {
        Valid(Box<Location>),
        Invalid(IgnoredAny),
    }

    pub fn locations<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<Location>, D::Error> {
        let entries = Vec::<Entry>::deserialize(deserializer)?;
        let valid = entries.into_iter().filter_map(|entry| match entry {
            Entry::Valid(location) => Some(*location),
            Entry::Invalid(_) => None,
        });
        Ok(valid.collect())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabTitle {
    pub status: Option<Status>,
    pub kind: String,
    pub label: String,
    pub cluster: Option<String>,
}

pub fn title(location: &Location, snapshot: &Snapshot) -> TabTitle {
    let names: Vec<&str> = snapshot.clusters.iter().map(|c| c.name.as_str()).collect();
    let cluster = location.cluster.as_ref().map(|id| {
        let name = snapshot
            .cluster(id)
            .map_or(id.as_str(), |c| c.name.as_str());
        short_name(name, &names)
    });
    let (status, kind, label) = describe(location, snapshot);
    let cluster = cluster.filter(|_| !location.page.is_flock());
    TabTitle {
        status,
        kind,
        label,
        cluster,
    }
}

fn describe(location: &Location, snapshot: &Snapshot) -> (Option<Status>, String, String) {
    if let Some(key) = &location.flux_object {
        let status = snapshot.flux_object(key).map(crate::FluxObject::status);
        return (status, key.kind.label().to_owned(), key.name.clone());
    }
    if location.page.is_flock() {
        return (None, String::new(), location.page.label().to_owned());
    }
    if let Some(target) = &location.logs {
        return (None, LOGS_KIND.to_owned(), target.label());
    }
    let Some(resource) = &location.resource else {
        if let Some(crd) = &location.crd {
            return (None, CRD_KIND.to_owned(), crd.kind.clone());
        }
        if let Some(key) = &location.helm {
            let status = snapshot.helm_release(key).map(|release| release.status);
            return (status, HELM_KIND.to_owned(), key.name.clone());
        }
        return (
            None,
            page_kind(location.page),
            location.page.label().to_owned(),
        );
    };
    let status = resource_status(resource, snapshot);
    match location.page {
        Page::Logs => (
            None,
            LOGS_KIND.to_owned(),
            pod_suffix(&resource.name).to_owned(),
        ),
        Page::Compare => (None, COMPARE_KIND.to_owned(), resource.name.clone()),
        Page::EditYaml => (None, EDIT_KIND.to_owned(), resource.name.clone()),
        _ => (
            status,
            crate::crd::display_kind(&resource.kind).to_owned(),
            resource.name.clone(),
        ),
    }
}

fn resource_status(resource: &crate::ResourceRef, snapshot: &Snapshot) -> Option<Status> {
    let pod = snapshot.pods.iter().find(|pod| pod.is(resource));
    if let Some(pod) = pod {
        return Some(pod.status);
    }
    snapshot
        .resources
        .iter()
        .find(|summary| &summary.resource == resource)
        .and_then(|summary| summary.status)
}

fn page_kind(page: Page) -> String {
    crate::CLUSTER_NAV
        .iter()
        .find(|section| section.pages.contains(&page))
        .and_then(|section| section.label)
        .map_or_else(|| CLUSTER_KIND.to_owned(), str::to_owned)
}

pub fn pod_suffix(name: &str) -> &str {
    name.rsplit_once(NAME_SEPARATOR)
        .map(|(_, suffix)| suffix)
        .filter(|suffix| !suffix.is_empty())
        .unwrap_or(name)
}

pub fn short_name(name: &str, names: &[&str]) -> String {
    let Some((head, rest)) = name.split_once(NAME_SEPARATOR) else {
        return name.to_owned();
    };
    let environment = ENVIRONMENT_PREFIXES.contains(&head);
    let readable = rest.chars().count() >= SHORT_NAME_MIN;
    let others = || names.iter().copied().filter(move |other| *other != name);
    let shared = others().any(|other| {
        other
            .split_once(NAME_SEPARATOR)
            .is_some_and(|(other_head, _)| other_head == head)
    });
    let collides = others().any(|other| {
        let remainder = other.split_once(NAME_SEPARATOR).map(|(_, r)| r);
        other == rest || remainder == Some(rest)
    });
    if environment && readable && shared && !collides {
        rest.to_owned()
    } else {
        name.to_owned()
    }
}

pub fn cluster_name(id: &ClusterId, snapshot: &Snapshot) -> String {
    snapshot
        .cluster(id)
        .map_or_else(|| id.to_string(), |cluster| cluster.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEMO: [&str; 6] = [
        "prod-us-east",
        "prod-eu-west",
        "staging",
        "dev-kind",
        "qa-gke",
        "edge-k3s",
    ];

    fn at(page: Page) -> Location {
        Location::page_on(ClusterId::new("a"), page)
    }

    fn ids(panes: &Panes) -> Vec<u64> {
        panes
            .focused()
            .tabs()
            .iter()
            .map(|t| t.id().as_u64())
            .collect()
    }

    #[test]
    fn only_a_shared_environment_prefix_drops_when_the_rest_stays_readable_and_unique() {
        assert_eq!(short_name("prod-us-east", &DEMO), "us-east");
        assert_eq!(short_name("prod-eu-west", &DEMO), "eu-west");
        assert_eq!(short_name("staging", &DEMO), "staging");
        assert_eq!(short_name("dev-kind", &DEMO), "dev-kind");
        assert_eq!(
            short_name("prod-us-east", &["prod-us-east"]),
            "prod-us-east"
        );
        let fleet = ["fleet-a", "fleet-b"];
        assert_eq!(short_name("fleet-a", &fleet), "fleet-a");
        assert_eq!(short_name("fleet-b", &fleet), "fleet-b");
        let teams = ["payments-east", "payments-west"];
        assert_eq!(short_name("payments-east", &teams), "payments-east");
        let short = ["prod-a", "prod-b"];
        assert_eq!(short_name("prod-a", &short), "prod-a");
        assert_eq!(short_name("prod-b", &short), "prod-b");
        let colliding = ["prod-east", "prod-west", "east"];
        assert_eq!(short_name("prod-east", &colliding), "prod-east");
        assert_eq!(short_name("prod-west", &colliding), "west");
        let twins = ["prod-us-east", "prod-xyz", "dev-us-east"];
        assert_eq!(short_name("prod-us-east", &twins), "prod-us-east");
        assert_eq!(short_name("prod-xyz", &twins), "xyz");
        assert_eq!(short_name("-odd", &["-odd", "-x"]), "-odd");
    }

    #[test]
    fn log_tabs_name_the_pod_by_its_hash_suffix() {
        assert_eq!(pod_suffix("checkout-api-7f9c6d-x2lqp"), "x2lqp");
        assert_eq!(pod_suffix("etcd"), "etcd");
        assert_eq!(pod_suffix("trailing-"), "trailing-");
    }

    #[test]
    fn a_new_window_has_one_flock_home_tab() {
        let panes = Panes::default();
        assert_eq!(panes.panes().len(), 1);
        assert_eq!(panes.focused().tabs().len(), 1);
        assert_eq!(panes.active().location(), &Location::default());
    }

    #[test]
    fn opening_inserts_after_the_active_tab_and_dedupes() {
        let mut panes = Panes::default();
        let nodes = panes.open(at(Page::Nodes));
        let pods = panes.open(at(Page::Pods));
        assert_eq!(ids(&panes), [0, 1, 2]);
        assert!(panes.activate(nodes));
        let events = panes.open(at(Page::Events));
        assert_eq!(ids(&panes), [0, 1, events.as_u64(), 2]);
        assert_eq!(panes.open(at(Page::Pods)), pods);
        assert_eq!(panes.active().id(), pods);
        assert_eq!(panes.focused().tabs().len(), 4);
    }

    #[test]
    fn closing_picks_the_right_neighbour_and_the_last_tab_becomes_flock_home() {
        let mut panes = Panes::default();
        let first = panes.active().id();
        let nodes = panes.open(at(Page::Nodes));
        let pods = panes.open(at(Page::Pods));
        assert!(panes.activate(nodes));
        assert!(panes.close(nodes));
        assert_eq!(panes.active().id(), pods);
        assert!(panes.close(pods));
        assert_eq!(panes.active().id(), first);
        assert!(panes.is_last(first));
        assert!(!panes.close(first));
        assert_eq!(panes.active().id(), first);
    }

    #[test]
    fn closing_a_tab_left_of_the_active_one_keeps_the_active_tab() {
        let mut panes = Panes::default();
        let first = panes.active().id();
        panes.open(at(Page::Nodes));
        let pods = panes.open(at(Page::Pods));
        assert!(panes.close(first));
        assert_eq!(panes.active().id(), pods);
    }

    #[test]
    fn cycling_wraps_and_numbers_select_by_position() {
        let mut panes = Panes::default();
        panes.open(at(Page::Nodes));
        panes.open(at(Page::Pods));
        assert!(panes.cycle(Cycle::Next));
        assert_eq!(panes.focused().active_index(), 0);
        assert!(panes.cycle(Cycle::Previous));
        assert_eq!(panes.focused().active_index(), 2);
        assert!(panes.activate_nth(1));
        assert_eq!(panes.active().location(), &at(Page::Nodes));
        assert!(!panes.activate_nth(3));
        assert_eq!(panes.focused().active_index(), 1);
        let mut single = Panes::default();
        assert!(!single.cycle(Cycle::Next));
    }

    #[test]
    fn moving_a_tab_reorders_and_keeps_the_active_tab() {
        let mut panes = Panes::default();
        let first = panes.active().id();
        let nodes = panes.open(at(Page::Nodes));
        let pods = panes.open(at(Page::Pods));
        assert!(panes.move_tab(pods, 0, 0));
        assert_eq!(ids(&panes), [pods.as_u64(), first.as_u64(), nodes.as_u64()]);
        assert_eq!(panes.active().id(), pods);
        assert!(!panes.move_tab(nodes, 0, 9));
        assert!(!panes.move_tab(nodes, 1, 0));
        assert!(panes.move_tab(first, 0, 2));
        assert_eq!(ids(&panes), [pods.as_u64(), nodes.as_u64(), first.as_u64()]);
        assert_eq!(panes.active().id(), pods);
    }

    fn pane_ids(panes: &Panes, ix: usize) -> Vec<TabId> {
        panes
            .panes()
            .get(ix)
            .map(|pane| pane.tabs().iter().map(Tab::id).collect())
            .unwrap_or_default()
    }

    #[test]
    fn splitting_opens_the_active_tab_in_a_focused_pane_on_the_right() {
        let mut panes = Panes::default();
        panes.open(at(Page::Nodes));
        let left = panes.focused().id();
        panes.split();
        assert_eq!(panes.panes().len(), 2);
        assert_eq!(panes.focused_index(), 1);
        assert_ne!(panes.focused().id(), left);
        assert_eq!(panes.focused().tabs().len(), 1);
        assert_eq!(panes.active().location(), &at(Page::Nodes));
        let right = panes.active().id();
        assert_eq!(pane_ids(&panes, 0).len(), 2);
        panes.open(at(Page::Pods));
        assert!(panes.focus(0));
        assert!(!panes.focus(0));
        assert!(!panes.focus(2));
        assert_eq!(panes.active().location(), &at(Page::Nodes));
        panes.split();
        assert_eq!(panes.panes().len(), MAX_PANES);
        assert_eq!(panes.focused_index(), 1);
        assert_eq!(panes.active().id(), right);
        assert!(panes.activate(pane_ids(&panes, 0)[0]));
        assert_eq!(panes.focused_index(), 0);
    }

    #[test]
    fn navigation_moves_only_the_focused_panes_tabs() {
        let mut panes = Panes::default();
        panes.split();
        panes.open(at(Page::Pods));
        panes.open(at(Page::Events));
        assert!(panes.cycle(Cycle::Next));
        assert!(panes.activate_nth(1));
        assert_eq!(panes.active().location(), &at(Page::Pods));
        assert_eq!(pane_ids(&panes, 0).len(), 1);
        assert!(panes.focus(0));
        assert!(!panes.cycle(Cycle::Next));
    }

    #[test]
    fn closing_a_panes_last_tab_closes_the_pane() {
        let mut panes = Panes::default();
        panes.open(at(Page::Nodes));
        panes.split();
        let right = panes.active().id();
        assert!(panes.close(right));
        assert_eq!(panes.panes().len(), 1);
        assert_eq!(panes.focused_index(), 0);
        assert_eq!(panes.active().location(), &at(Page::Nodes));
        panes.split();
        assert!(panes.focus(0));
        let left: Vec<TabId> = pane_ids(&panes, 0);
        for id in left {
            assert!(panes.close(id));
        }
        assert_eq!(panes.panes().len(), 1);
        assert_eq!(panes.focused_index(), 0);
        assert_eq!(panes.active().location(), &at(Page::Nodes));
        let last = panes.active().id();
        assert!(panes.is_last(last));
        assert!(!panes.close(last));
        assert_eq!(panes.active().location(), &at(Page::Nodes));
    }

    #[test]
    fn closing_the_left_pane_while_the_right_is_focused_keeps_the_right_focused() {
        let mut panes = Panes::default();
        let home = panes.active().id();
        panes.split();
        panes.open(at(Page::Pods));
        assert!(panes.close(home));
        assert_eq!(panes.panes().len(), 1);
        assert_eq!(panes.focused_index(), 0);
        assert_eq!(panes.active().location(), &at(Page::Pods));
    }

    #[test]
    fn moving_a_tab_to_the_other_pane_focuses_it_there() {
        let mut panes = Panes::default();
        let home = panes.active().id();
        let nodes = panes.open(at(Page::Nodes));
        panes.split();
        let right = panes.active().id();
        assert!(panes.move_tab(home, 1, 0));
        assert_eq!(pane_ids(&panes, 0), [nodes]);
        assert_eq!(pane_ids(&panes, 1), [home, right]);
        assert_eq!(panes.focused_index(), 1);
        assert_eq!(panes.active().id(), home);
        assert!(panes.move_tab(nodes, 1, 9));
        assert_eq!(panes.panes().len(), 1);
        assert_eq!(panes.focused_index(), 0);
        assert_eq!(pane_ids(&panes, 0), [home, right, nodes]);
        assert_eq!(panes.active().id(), nodes);
        assert!(!panes.move_tab(nodes, 1, 0));
    }

    #[test]
    fn splitting_off_a_tab_moves_it_into_a_new_right_pane() {
        let mut panes = Panes::default();
        let home = panes.active().id();
        assert!(!panes.split_off(home));
        let nodes = panes.open(at(Page::Nodes));
        assert!(panes.split_off(home));
        assert_eq!(pane_ids(&panes, 0), [nodes]);
        assert_eq!(pane_ids(&panes, 1), [home]);
        assert_eq!(panes.focused_index(), 1);
        assert!(!panes.split_off(nodes));
    }

    #[test]
    fn joining_keeps_every_tab_in_one_pane_with_the_chosen_one_active() {
        let mut panes = Panes::default();
        let home = panes.active().id();
        panes.split();
        let right = panes.active().id();
        let pods = panes.open(at(Page::Pods));
        assert!(panes.join(right));
        assert_eq!(panes.panes().len(), 1);
        assert_eq!(pane_ids(&panes, 0), [home, right, pods]);
        assert_eq!(panes.active().id(), right);
        assert!(!panes.join(right));
    }

    #[test]
    fn the_divider_keeps_both_panes_visible_and_survives_a_restore() {
        assert_eq!(Divider::default().share(), 0.5);
        assert_eq!(Divider::at(0.25).share(), 0.25);
        assert_eq!(Divider::at(0.01), Divider::at(0.1));
        assert_eq!(Divider::at(1.5), Divider::at(0.9));
        assert_eq!(Divider::at(f32::NAN), Divider::default());
        let mut panes = Panes::default();
        panes.split();
        assert!(panes.set_divider(Divider::at(0.3)));
        assert!(!panes.set_divider(Divider::at(0.3)));
        let saved = panes.saved();
        assert_eq!(Panes::restore(&saved).divider(), Divider::at(0.3));
        let text = r#"{"panes":[],"focused":0,"divider":2}"#;
        let odd: SavedWindow = serde_json::from_str(text).unwrap_or_default();
        assert_eq!(Panes::restore(&odd).divider(), Divider::at(0.1));
        let old: SavedWindow =
            serde_json::from_str(r#"{"panes":[],"focused":0}"#).unwrap_or_default();
        assert_eq!(old.divider, Divider::default());
    }

    #[test]
    fn split_windows_save_and_restore_both_panes() {
        let mut panes = Panes::default();
        panes.open(at(Page::Nodes));
        panes.split();
        panes.open(at(Page::Pods));
        let saved = panes.saved();
        assert_eq!(saved.panes.len(), 2);
        assert_eq!(saved.focused, 1);
        let restored = Panes::restore(&saved);
        assert_eq!(restored.saved(), saved);
        assert_eq!(restored.active().location(), &at(Page::Pods));
        let mut crowded = saved.clone();
        crowded.panes.push(saved.panes[0].clone());
        crowded.focused = 2;
        let capped = Panes::restore(&crowded);
        assert_eq!(capped.panes().len(), MAX_PANES);
        assert_eq!(capped.focused_index(), 1);
    }

    #[test]
    fn windows_save_and_restore_their_tabs() {
        let mut panes = Panes::default();
        panes.open(at(Page::Nodes));
        panes.open(at(Page::Pods));
        assert!(panes.activate_nth(1));
        let saved = panes.saved();
        let restored = Panes::restore(&saved);
        assert_eq!(restored.saved(), saved);
        assert_eq!(restored.active().location(), &at(Page::Nodes));
        let empty = Panes::restore(&SavedWindow {
            panes: vec![SavedPane {
                tabs: Vec::new(),
                active: 4,
            }],
            focused: 3,
            ..SavedWindow::default()
        });
        assert_eq!(empty, Panes::default());
        let clamped = Panes::restore(&SavedWindow {
            panes: vec![SavedPane {
                tabs: vec![at(Page::Nodes)],
                active: 7,
            }],
            focused: 0,
            ..SavedWindow::default()
        });
        assert_eq!(clamped.active().location(), &at(Page::Nodes));
    }

    #[test]
    fn unknown_tabs_are_skipped_when_reading_a_session() -> Result<(), serde_json::Error> {
        let text = r#"{"windows":[{"panes":[{"tabs":[
            {"cluster":"a","page":"Nodes"},
            {"cluster":"a","page":"NoSuchPage"},
            {"cluster":null,"page":"GitOps"}
        ],"active":1}],"focused":0}]}"#;
        let session: Session = serde_json::from_str(text)?;
        let tabs = session
            .windows
            .first()
            .and_then(|window| window.panes.first())
            .map(|pane| pane.tabs.clone())
            .unwrap_or_default();
        assert_eq!(tabs, [at(Page::Nodes), Location::flock(Page::GitOps)]);
        let json = serde_json::to_string(&session)?;
        assert_eq!(serde_json::from_str::<Session>(&json)?, session);
        Ok(())
    }

    #[test]
    fn titles_follow_the_design_kinds() {
        let snapshot = crate::flock::tests::sample();
        let flock = title(&Location::flock(Page::GitOps), &snapshot);
        assert_eq!(
            (flock.kind.as_str(), flock.label.as_str(), flock.cluster),
            ("", "GitOps", None)
        );
        let nodes = title(&at(Page::Nodes), &snapshot);
        assert_eq!(
            (nodes.kind.as_str(), nodes.label.as_str()),
            (CLUSTER_KIND, "Nodes")
        );
        assert_eq!(nodes.cluster.as_deref(), Some("a"));
        let deployments = title(&at(Page::Deployments), &snapshot);
        assert_eq!(deployments.kind, "Workloads");
        let pod = crate::ResourceRef {
            cluster: ClusterId::new("a"),
            kind: "Pod".into(),
            namespace: Some("shop".into()),
            name: "cart-7f9c-x2lqp".into(),
        };
        let logs = Location {
            resource: Some(pod.clone()),
            container: Some("cart".into()),
            ..at(Page::Logs)
        };
        let logs = title(&logs, &snapshot);
        assert_eq!(
            (logs.kind.as_str(), logs.label.as_str()),
            (LOGS_KIND, "x2lqp")
        );
        let workload = Location {
            resource: Some(crate::ResourceRef {
                kind: "Deployment".into(),
                name: "checkout-api".into(),
                ..pod
            }),
            ..at(Page::Deployments)
        };
        let workload = title(&workload, &snapshot);
        assert_eq!(
            (workload.kind.as_str(), workload.label.as_str()),
            ("Deployment", "checkout-api")
        );
    }
}
