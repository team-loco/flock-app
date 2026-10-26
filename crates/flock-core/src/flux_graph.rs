use std::collections::{BTreeMap, BTreeSet};

use crate::{ClusterId, FluxKey, FluxObject, Snapshot, Status};

const SWEEPS: usize = 8;
const NOT_APPLIED: &str = "—";
const NO_SOURCE: &str = "—";
const NO_REASON: &str = "—";
const NO_DEPENDENCIES: &str = "none";
const LIST_SEPARATOR: &str = ", ";
const STATUS_ROW: &str = "Status";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EdgeKind {
    Source,
    DependsOn,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphNode {
    pub key: FluxKey,
    pub status: Status,
    pub present: bool,
    pub layer: usize,
    pub row: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct GraphEdge {
    pub from: usize,
    pub to: usize,
    pub kind: EdgeKind,
    pub via: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FluxGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub layers: Vec<Vec<usize>>,
    pub rows: usize,
}

impl FluxGraph {
    pub fn build(cluster: &ClusterId, flux: &[FluxObject]) -> Self {
        let objects: BTreeMap<&FluxKey, &FluxObject> = flux
            .iter()
            .filter(|object| &object.key.cluster == cluster)
            .map(|object| (&object.key, object))
            .collect();
        let mut keys: BTreeSet<&FluxKey> = objects.keys().copied().collect();
        let mut links: BTreeSet<(&FluxKey, &FluxKey, EdgeKind)> = BTreeSet::new();
        for object in objects.values() {
            let sources = object.source.iter().map(|key| (key, EdgeKind::Source));
            let dependencies = object
                .depends_on
                .iter()
                .map(|key| (key, EdgeKind::DependsOn));
            for (from, kind) in sources.chain(dependencies) {
                if &from.cluster != cluster || from == &object.key {
                    continue;
                }
                keys.insert(from);
                links.insert((from, &object.key, kind));
            }
        }
        let index: BTreeMap<&FluxKey, usize> = keys
            .iter()
            .enumerate()
            .map(|(ix, key)| (*key, ix))
            .collect();
        let mut edges: Vec<GraphEdge> = links
            .into_iter()
            .filter_map(|(from, to, kind)| {
                Some(GraphEdge {
                    from: *index.get(from)?,
                    to: *index.get(to)?,
                    kind,
                    via: Vec::new(),
                })
            })
            .collect();
        let mut nodes: Vec<GraphNode> = keys
            .iter()
            .map(|key| {
                let object = objects.get(key);
                GraphNode {
                    key: (*key).clone(),
                    status: object.map_or(Status::Offline, |object| object.status()),
                    present: object.is_some(),
                    layer: 0,
                    row: 0,
                }
            })
            .collect();
        let (layers, rows) = place(&mut nodes, &mut edges);
        Self {
            nodes,
            edges,
            layers,
            rows,
        }
    }

    pub fn find(&self, key: &FluxKey) -> Option<usize> {
        self.nodes.iter().position(|node| &node.key == key)
    }

    pub fn touches(&self, edge: &GraphEdge, node: usize) -> bool {
        edge.from == node || edge.to == node
    }

    pub fn step(&self, from: Option<usize>, step: Step) -> Option<usize> {
        let Some(current) = from.and_then(|ix| self.nodes.get(ix)) else {
            return self.layers.first().and_then(|layer| layer.first()).copied();
        };
        let from = from?;
        let layer = self.layers.get(current.layer)?;
        let at = layer.iter().position(|member| *member == from)?;
        let target = match step {
            Step::Up => at.checked_sub(1).and_then(|row| layer.get(row)),
            Step::Down => layer.get(at + 1),
            Step::Left => current
                .layer
                .checked_sub(1)
                .and_then(|layer| self.nearest(from, layer)),
            Step::Right => self.nearest(from, current.layer + 1),
        };
        Some(target.copied().unwrap_or(from))
    }

    fn nearest(&self, from: usize, layer: usize) -> Option<&usize> {
        let members = self.layers.get(layer)?;
        let row = self.nodes.get(from)?.row;
        let connected = |candidate: &usize| {
            self.edges.iter().any(|edge| {
                (edge.from == from && edge.to == *candidate)
                    || (edge.to == from && edge.from == *candidate)
            })
        };
        let distance = |candidate: &&usize| {
            let candidate_row = self.nodes.get(**candidate).map_or(usize::MAX, |n| n.row);
            (candidate_row.abs_diff(row), candidate_row)
        };
        members
            .iter()
            .filter(|candidate| connected(candidate))
            .min_by_key(distance)
            .or_else(|| members.iter().min_by_key(distance))
    }
}

fn place(nodes: &mut [GraphNode], edges: &mut [GraphEdge]) -> (Vec<Vec<usize>>, usize) {
    let layer_of = longest_path_layers(nodes.len(), edges);
    let mut slot_layer = layer_of.clone();
    let mut links: Vec<(usize, usize)> = Vec::new();
    let mut passes: Vec<Vec<usize>> = Vec::new();
    for edge in edges.iter() {
        let from_layer = layer_of.get(edge.from).copied().unwrap_or_default();
        let to_layer = layer_of.get(edge.to).copied().unwrap_or_default();
        let mut previous = edge.from;
        let mut pass = Vec::new();
        for layer in from_layer + 1..to_layer {
            let slot = slot_layer.len();
            slot_layer.push(layer);
            links.push((previous, slot));
            pass.push(slot);
            previous = slot;
        }
        links.push((previous, edge.to));
        passes.push(pass);
    }
    let slots = order_layers(&slot_layer, &links);
    let mut row_of = vec![0usize; slot_layer.len()];
    for members in &slots {
        for (row, slot) in members.iter().enumerate() {
            if let Some(at) = row_of.get_mut(*slot) {
                *at = row;
            }
        }
    }
    for (ix, node) in nodes.iter_mut().enumerate() {
        node.layer = layer_of.get(ix).copied().unwrap_or_default();
        node.row = row_of.get(ix).copied().unwrap_or_default();
    }
    for (edge, pass) in edges.iter_mut().zip(passes) {
        edge.via = pass
            .into_iter()
            .filter_map(|slot| row_of.get(slot).copied())
            .collect();
    }
    let count = nodes.len();
    let layers: Vec<Vec<usize>> = slots
        .iter()
        .map(|members| {
            members
                .iter()
                .copied()
                .filter(|slot| *slot < count)
                .collect()
        })
        .collect();
    let rows = slots.iter().map(Vec::len).max().unwrap_or_default();
    (layers, rows)
}

fn longest_path_layers(count: usize, edges: &[GraphEdge]) -> Vec<usize> {
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); count];
    for edge in edges {
        if let Some(list) = preds.get_mut(edge.to) {
            list.push(edge.from);
        }
    }
    let mut layer: Vec<Option<usize>> = vec![None; count];
    let mut visiting = vec![false; count];
    for node in 0..count {
        visit(node, &preds, &mut layer, &mut visiting);
    }
    layer.into_iter().map(Option::unwrap_or_default).collect()
}

fn visit(
    node: usize,
    preds: &[Vec<usize>],
    layer: &mut [Option<usize>],
    visiting: &mut [bool],
) -> Option<usize> {
    if let Some(done) = layer.get(node).copied().flatten() {
        return Some(done);
    }
    if visiting.get(node).copied().unwrap_or(true) {
        return None;
    }
    if let Some(flag) = visiting.get_mut(node) {
        *flag = true;
    }
    let mut deepest: Option<usize> = None;
    for pred in preds.get(node).into_iter().flatten() {
        if let Some(depth) = visit(*pred, preds, layer, visiting) {
            deepest = Some(deepest.map_or(depth, |d| d.max(depth)));
        }
    }
    let depth = deepest.map_or(0, |d| d + 1);
    if let Some(slot) = layer.get_mut(node) {
        *slot = Some(depth);
    }
    if let Some(flag) = visiting.get_mut(node) {
        *flag = false;
    }
    Some(depth)
}

fn order_layers(layer_of: &[usize], links: &[(usize, usize)]) -> Vec<Vec<usize>> {
    let depth = layer_of.iter().max().map_or(0, |max| max + 1);
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); depth];
    for (node, layer) in layer_of.iter().enumerate() {
        if let Some(members) = layers.get_mut(*layer) {
            members.push(node);
        }
    }
    let mut parents: Vec<Vec<usize>> = vec![Vec::new(); layer_of.len()];
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); layer_of.len()];
    for (from, to) in links {
        if let Some(list) = parents.get_mut(*to) {
            list.push(*from);
        }
        if let Some(list) = children.get_mut(*from) {
            list.push(*to);
        }
    }
    let mut row = vec![0usize; layer_of.len()];
    for members in &layers {
        place_rows(members, &mut row);
    }
    for sweep in 0..SWEEPS {
        let downward = sweep % 2 == 0;
        let neighbours = if downward { &parents } else { &children };
        let order: Vec<usize> = if downward {
            (1..depth).collect()
        } else {
            (0..depth.saturating_sub(1)).rev().collect()
        };
        let mut changed = false;
        for layer in order {
            let Some(members) = layers.get_mut(layer) else {
                continue;
            };
            let mut keyed: Vec<(f32, usize, usize)> = members
                .iter()
                .enumerate()
                .map(|(at, node)| {
                    let linked = neighbours.get(*node).map_or(&[][..], Vec::as_slice);
                    let rows = linked.iter().filter_map(|other| row.get(*other).copied());
                    let (sum, n) = rows.fold((0usize, 0usize), |(s, n), r| (s + r, n + 1));
                    let centre = if n == 0 {
                        position(at)
                    } else {
                        position(sum) / position(n)
                    };
                    (centre, at, *node)
                })
                .collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            let moved = keyed.iter().enumerate().any(|(to, (_, at, _))| to != *at);
            if moved {
                changed = true;
                *members = keyed.into_iter().map(|(_, _, node)| node).collect();
                place_rows(members, &mut row);
            }
        }
        if !changed && sweep > 0 {
            break;
        }
    }
    layers
}

fn place_rows(members: &[usize], row: &mut [usize]) {
    for (at, node) in members.iter().enumerate() {
        if let Some(slot) = row.get_mut(*node) {
            *slot = at;
        }
    }
}

#[allow(clippy::cast_precision_loss)]
const fn position(value: usize) -> f32 {
    value as f32
}

type Field = (&'static str, fn(&FluxObject) -> String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompareCell {
    pub text: String,
    pub status: Option<Status>,
    pub differs: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompareRow {
    pub label: &'static str,
    pub cells: Vec<CompareCell>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossCluster {
    pub key: FluxKey,
    pub clusters: Vec<(ClusterId, String)>,
    pub rows: Vec<CompareRow>,
}

impl Snapshot {
    pub fn flux_across(&self, key: &FluxKey) -> Option<CrossCluster> {
        let base = self.flux_object(key)?;
        let objects: Vec<&FluxObject> = std::iter::once(base).chain(self.flux_peers(key)).collect();
        let clusters = objects
            .iter()
            .map(|object| {
                let id = object.key.cluster.clone();
                let name = crate::tabs::cluster_name(&id, self);
                (id, name)
            })
            .collect();
        let fields: [Field; 5] = [
            (STATUS_ROW, |o| o.phase.label().to_owned()),
            ("Revision", |o| {
                o.applied
                    .as_ref()
                    .map_or_else(|| NOT_APPLIED.to_owned(), |r| r.id.clone())
            }),
            ("Source", |o| {
                o.source.as_ref().map_or_else(
                    || NO_SOURCE.to_owned(),
                    |s| format!("{} {}", s.kind.label(), s.name),
                )
            }),
            ("Depends on", |o| {
                if o.depends_on.is_empty() {
                    return NO_DEPENDENCIES.to_owned();
                }
                let names: Vec<&str> = o.depends_on.iter().map(|k| k.name.as_str()).collect();
                names.join(LIST_SEPARATOR)
            }),
            ("Reason", |o| {
                o.current_diagnosis()
                    .map_or_else(|| NO_REASON.to_owned(), |d| d.reason.clone())
            }),
        ];
        let rows = fields
            .into_iter()
            .map(|(label, value)| {
                let base_value = value(base);
                let cells = objects
                    .iter()
                    .map(|object| {
                        let text = value(object);
                        CompareCell {
                            differs: text != base_value,
                            status: (label == STATUS_ROW).then(|| object.status()),
                            text,
                        }
                    })
                    .collect();
                CompareRow { label, cells }
            })
            .collect();
        Some(CrossCluster {
            key: key.clone(),
            clusters,
            rows,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FluxKind;
    use crate::flock::tests::cluster;
    use crate::flux::Phase;
    use crate::flux::tests::object;
    use crate::{Counts, Snapshot};

    const EAST: &str = "east";

    fn named(kind: FluxKind, name: &str, phase: Phase) -> FluxObject {
        let mut o = object(EAST, phase, "a41e07c");
        o.key = FluxKey::new(EAST, kind, "flux-system", name);
        o
    }

    fn ks(name: &str) -> FluxKey {
        FluxKey::new(EAST, FluxKind::Kustomization, "flux-system", name)
    }

    fn git(name: &str) -> FluxKey {
        FluxKey::new(EAST, FluxKind::GitRepository, "flux-system", name)
    }

    fn fixture() -> Vec<FluxObject> {
        let repo = named(FluxKind::GitRepository, "fleet", Phase::Ready);
        let mut infra = named(FluxKind::Kustomization, "infra", Phase::Ready);
        infra.source = Some(git("fleet"));
        let mut configs = named(FluxKind::Kustomization, "configs", Phase::Failing);
        configs.source = Some(git("fleet"));
        configs.depends_on = vec![ks("infra")];
        let mut apps = named(FluxKind::Kustomization, "apps", Phase::Ready);
        apps.source = Some(git("fleet"));
        apps.depends_on = vec![ks("configs"), ks("gone")];
        let elsewhere = object("west", Phase::Ready, "a41e07c");
        vec![apps, configs, elsewhere, infra, repo]
    }

    fn layer_names(graph: &FluxGraph) -> Vec<Vec<&str>> {
        graph
            .layers
            .iter()
            .map(|layer| {
                layer
                    .iter()
                    .map(|ix| graph.nodes[*ix].key.name.as_str())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn sources_lead_and_dependents_follow_their_longest_chain() {
        let graph = FluxGraph::build(&ClusterId::new(EAST), &fixture());
        assert_eq!(
            layer_names(&graph),
            [
                vec!["gone", "fleet"],
                vec!["infra"],
                vec!["configs"],
                vec!["apps"]
            ]
        );
        let gone = graph.find(&ks("gone")).map(|ix| &graph.nodes[ix]);
        assert_eq!(
            gone.map(|n| (n.present, n.status)),
            Some((false, Status::Offline))
        );
        let configs = graph.find(&ks("configs")).map(|ix| graph.nodes[ix].status);
        assert_eq!(configs, Some(Status::Err));
        let kinds: Vec<EdgeKind> = graph.edges.iter().map(|e| e.kind).collect();
        assert_eq!(kinds.iter().filter(|k| **k == EdgeKind::Source).count(), 3);
        assert_eq!(
            kinds.iter().filter(|k| **k == EdgeKind::DependsOn).count(),
            3
        );
    }

    #[test]
    fn layout_is_the_same_whatever_order_objects_arrive_in() {
        let cluster = ClusterId::new(EAST);
        let forward = FluxGraph::build(&cluster, &fixture());
        let mut reversed = fixture();
        reversed.reverse();
        assert_eq!(FluxGraph::build(&cluster, &reversed), forward);
    }

    #[test]
    fn rows_follow_their_parents_to_keep_edges_short() {
        let mut objects = Vec::new();
        for (repo, app) in [("b-repo", "a-app"), ("a-repo", "b-app")] {
            objects.push(named(FluxKind::GitRepository, repo, Phase::Ready));
            let mut user = named(FluxKind::Kustomization, app, Phase::Ready);
            user.source = Some(git(repo));
            objects.push(user);
        }
        let graph = FluxGraph::build(&ClusterId::new(EAST), &objects);
        assert_eq!(
            layer_names(&graph),
            [vec!["a-repo", "b-repo"], vec!["b-app", "a-app"]]
        );
    }

    #[test]
    fn dependency_cycles_still_lay_out() {
        let mut a = named(FluxKind::Kustomization, "a", Phase::Failing);
        a.depends_on = vec![ks("b"), ks("a")];
        let mut b = named(FluxKind::Kustomization, "b", Phase::Failing);
        b.depends_on = vec![ks("a")];
        let graph = FluxGraph::build(&ClusterId::new(EAST), &[a, b]);
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges.len(), 2);
        assert_eq!(layer_names(&graph), [vec!["b"], vec!["a"]]);
    }

    #[test]
    fn arrows_walk_rows_and_layers_preferring_linked_nodes() {
        let graph = FluxGraph::build(&ClusterId::new(EAST), &fixture());
        let at = |key: &FluxKey| graph.find(key);
        let name = |ix: Option<usize>| ix.map(|ix| graph.nodes[ix].key.name.clone());
        assert_eq!(name(graph.step(None, Step::Down)), Some("gone".into()));
        let fleet = at(&git("fleet"));
        assert_eq!(name(graph.step(fleet, Step::Down)), Some("fleet".into()));
        assert_eq!(name(graph.step(fleet, Step::Up)), Some("gone".into()));
        assert_eq!(name(graph.step(fleet, Step::Left)), Some("fleet".into()));
        assert_eq!(name(graph.step(fleet, Step::Right)), Some("infra".into()));
        let apps = at(&ks("apps"));
        assert_eq!(name(graph.step(apps, Step::Left)), Some("configs".into()));
        assert_eq!(name(graph.step(apps, Step::Right)), Some("apps".into()));
    }

    #[test]
    fn the_same_object_lines_up_across_clusters() {
        let mut east = object(EAST, Phase::Failing, "a41e07c");
        east.source = Some(git("fleet"));
        let mut west = object("west", Phase::Ready, "7c2d9e0");
        west.source = Some(git("fleet").on(&ClusterId::new("west")));
        let snapshot = Snapshot {
            clusters: [EAST, "west"]
                .map(|name| cluster(name, Status::Ok, Counts::default()))
                .into(),
            flux: vec![west, east],
            ..Snapshot::default()
        };
        let key = FluxKey::new(EAST, FluxKind::Kustomization, "flux-system", "apps");
        let across = snapshot.flux_across(&key);
        let clusters: Vec<&str> = across
            .iter()
            .flat_map(|a| a.clusters.iter().map(|(id, _)| id.as_str()))
            .collect();
        assert_eq!(clusters, [EAST, "west"]);
        let row = |label: &str| {
            across
                .as_ref()
                .and_then(|a| a.rows.iter().find(|row| row.label == label))
                .map(|row| {
                    row.cells
                        .iter()
                        .map(|c| (c.text.clone(), c.differs))
                        .collect::<Vec<_>>()
                })
        };
        assert_eq!(
            row("Revision"),
            Some(vec![("a41e07c".into(), false), ("7c2d9e0".into(), true)])
        );
        assert_eq!(
            row("Source"),
            Some(vec![
                ("GitRepository fleet".into(), false),
                ("GitRepository fleet".into(), false)
            ])
        );
        assert_eq!(row("Status").map(|cells| cells.len()), Some(2));
    }
}
