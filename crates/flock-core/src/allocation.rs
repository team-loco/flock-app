use std::collections::BTreeMap;

pub const TILED_PODS: usize = 10;
pub const NAMESPACE_SLOTS: usize = 6;
const MILLI: f32 = 1000.;
const MIB_PER_GIB: f32 = 1024.;
const WHOLE_GIB_FROM: f32 = 100.;
const HASH_MIN: usize = 3;
const HASH_MAX: usize = 10;
const SUFFIX_LEN: usize = 5;
const NAME_SEPARATOR: char = '-';
const PATH_SEPARATOR: char = '/';
const NO_LIMIT: &str = "—";
const SAFE_ALPHABET: &str = "bcdfghjklmnpqrstvwxz2456789";

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Share {
    pub request: f32,
    pub limit: Option<f32>,
}

impl Share {
    fn value(self, basis: Basis) -> f32 {
        match basis {
            Basis::Requests => self.request,
            Basis::Limits => self.limit.unwrap_or(self.request).max(self.request),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContainerShare {
    pub name: String,
    pub sidecar: bool,
    pub cpu: Share,
    pub memory: Share,
}

impl ContainerShare {
    fn share(&self, resource: Resource) -> Share {
        match resource {
            Resource::Cpu => self.cpu,
            Resource::Memory => self.memory,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PodShare {
    pub namespace: String,
    pub name: String,
    pub containers: Vec<ContainerShare>,
}

impl PodShare {
    pub fn value(&self, resource: Resource, basis: Basis) -> f32 {
        self.containers
            .iter()
            .map(|c| c.share(resource).value(basis))
            .sum()
    }

    pub fn requested(&self, resource: Resource) -> f32 {
        self.containers
            .iter()
            .map(|c| c.share(resource).request)
            .sum()
    }

    pub fn limit(&self, resource: Resource) -> Option<f32> {
        self.containers
            .iter()
            .map(|c| c.share(resource).limit)
            .sum()
    }

    pub fn path(&self) -> String {
        format!("{}{PATH_SEPARATOR}{}", self.namespace, self.name)
    }

    pub fn workload(&self) -> &str {
        workload_name(&self.name)
    }

    pub fn summary(&self, resource: Resource) -> String {
        let limit = self
            .limit(resource)
            .map_or_else(|| NO_LIMIT.to_owned(), |limit| amount(resource, limit));
        format!(
            "{} req · {limit} lim",
            amount(resource, self.requested(resource))
        )
    }

    pub fn container_lines(&self, resource: Resource) -> Vec<(String, bool, String)> {
        self.containers
            .iter()
            .map(|c| {
                let share = c.share(resource);
                let limit = share
                    .limit
                    .map_or_else(|| NO_LIMIT.to_owned(), |limit| amount(resource, limit));
                let figures = format!("{} / {limit}", amount(resource, share.request));
                (c.name.clone(), c.sidecar, figures)
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Resource {
    #[default]
    Cpu,
    Memory,
}

impl Resource {
    pub const ALL: [Self; 2] = [Self::Cpu, Self::Memory];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Memory => "Memory",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Basis {
    #[default]
    Requests,
    Limits,
}

impl Basis {
    pub const ALL: [Self; 2] = [Self::Requests, Self::Limits];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Requests => "Requests",
            Self::Limits => "Limits",
        }
    }

    pub const fn free_label(self) -> &'static str {
        match self {
            Self::Requests => "unrequested",
            Self::Limits => "not limited",
        }
    }
}

pub fn amount(resource: Resource, value: f32) -> String {
    let value = value.max(0.);
    match resource {
        Resource::Cpu if value < 1. => format!("{:.0}m", value * MILLI),
        Resource::Cpu => trimmed(format!("{value:.2}")),
        Resource::Memory if value < 1. => format!("{:.0}Mi", value * MIB_PER_GIB),
        Resource::Memory if value >= WHOLE_GIB_FROM => format!("{value:.0}Gi"),
        Resource::Memory => format!("{}Gi", trimmed(format!("{value:.1}"))),
    }
}

fn trimmed(text: String) -> String {
    if !text.contains('.') {
        return text;
    }
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

pub fn workload_name(pod: &str) -> &str {
    let Some((rest, suffix)) = pod.rsplit_once(NAME_SEPARATOR) else {
        return pod;
    };
    let Some((workload, hash)) = rest.rsplit_once(NAME_SEPARATOR) else {
        return pod;
    };
    let token = |text: &str, min: usize, max: usize| {
        (min..=max).contains(&text.len()) && text.chars().all(|c| SAFE_ALPHABET.contains(c))
    };
    if token(suffix, SUFFIX_LEN, SUFFIX_LEN)
        && token(hash, HASH_MIN, HASH_MAX)
        && !workload.is_empty()
    {
        workload
    } else {
        pod
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    fn area(self) -> f32 {
        self.w * self.h
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Namespace(usize),
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Subject {
    Pod(usize),
    Rest,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub rect: Rect,
    pub filled: f32,
    pub sidecar: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub rect: Rect,
    pub subject: Subject,
    pub slot: Slot,
    pub value: f32,
    pub parts: Vec<Part>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LegendEntry {
    pub label: String,
    pub slot: Slot,
    pub value: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Treemap {
    pub tiles: Vec<Tile>,
    pub free: Option<Rect>,
    pub free_value: f32,
    pub total: f32,
    pub allocatable: f32,
    pub legend: Vec<LegendEntry>,
    pub rest: Vec<usize>,
}

impl Treemap {
    pub fn fraction(&self) -> f32 {
        if self.allocatable > 0. {
            self.total / self.allocatable
        } else {
            0.
        }
    }

    pub fn overcommitted(&self) -> bool {
        self.total > self.allocatable
    }

    pub fn build(
        pods: &[PodShare],
        allocatable: f32,
        resource: Resource,
        basis: Basis,
        width: f32,
        height: f32,
    ) -> Self {
        let mut ranked: Vec<(usize, f32)> = pods
            .iter()
            .enumerate()
            .map(|(ix, pod)| (ix, pod.value(resource, basis)))
            .collect();
        ranked.sort_by(|a, b| {
            b.1.total_cmp(&a.1)
                .then_with(|| pods[a.0].path().cmp(&pods[b.0].path()))
        });
        let tiled: Vec<(usize, f32)> = ranked
            .iter()
            .copied()
            .filter(|(_, value)| *value > 0.)
            .take(TILED_PODS)
            .collect();
        let rest: Vec<usize> = ranked
            .iter()
            .map(|(ix, _)| *ix)
            .filter(|ix| !tiled.iter().any(|(tiled, _)| tiled == ix))
            .collect();
        let rest_value: f32 = rest.iter().map(|ix| pods[*ix].value(resource, basis)).sum();
        let slots = namespace_slots(pods, &tiled);
        let slot_of = |pod: &PodShare| {
            slots
                .iter()
                .position(|(ns, _)| *ns == pod.namespace)
                .map_or(Slot::Other, Slot::Namespace)
        };
        let total = tiled.iter().map(|(_, v)| v).sum::<f32>() + rest_value;
        let free_value = (allocatable - total).max(0.);
        let mut areas: Vec<f32> = tiled.iter().map(|(_, v)| *v).collect();
        if rest_value > 0. {
            areas.push(rest_value);
        }
        let tile_count = areas.len();
        if free_value > 0. {
            areas.push(free_value);
        }
        let rects = squarify(
            &areas,
            Rect {
                x: 0.,
                y: 0.,
                w: width,
                h: height,
            },
        );
        let mut laid: Vec<Tile> = tiled
            .iter()
            .zip(&rects)
            .map(|((ix, value), rect)| {
                let pod = &pods[*ix];
                Tile {
                    rect: *rect,
                    subject: Subject::Pod(*ix),
                    slot: slot_of(pod),
                    value: *value,
                    parts: pod_parts(pod, *rect, resource, basis),
                }
            })
            .collect();
        if rest_value > 0.
            && let Some(rect) = rects.get(tiled.len())
        {
            let requested: f32 = rest.iter().map(|ix| pods[*ix].requested(resource)).sum();
            laid.push(Tile {
                rect: *rect,
                subject: Subject::Rest,
                slot: Slot::Other,
                value: rest_value,
                parts: vec![Part {
                    rect: *rect,
                    filled: filled(requested, rest_value, basis),
                    sidecar: false,
                }],
            });
        }
        let free = (free_value > 0.)
            .then(|| rects.get(tile_count).copied())
            .flatten();
        Self {
            tiles: laid,
            free,
            free_value,
            total,
            allocatable,
            legend: legend(&slots, total),
            rest,
        }
    }
}

pub const OTHER: &str = "other";

fn legend(slots: &[(&str, f32)], total: f32) -> Vec<LegendEntry> {
    let mut entries: Vec<LegendEntry> = slots
        .iter()
        .enumerate()
        .map(|(slot, (ns, value))| LegendEntry {
            label: (*ns).to_owned(),
            slot: Slot::Namespace(slot),
            value: *value,
        })
        .collect();
    let named: f32 = entries.iter().map(|entry| entry.value).sum();
    let other = total - named;
    if other > 0. {
        entries.push(LegendEntry {
            label: OTHER.to_owned(),
            slot: Slot::Other,
            value: other,
        });
    }
    entries
}

fn namespace_slots<'a>(pods: &'a [PodShare], tiled: &[(usize, f32)]) -> Vec<(&'a str, f32)> {
    let mut totals: BTreeMap<&str, f32> = BTreeMap::new();
    for (ix, value) in tiled {
        *totals.entry(pods[*ix].namespace.as_str()).or_default() += value;
    }
    let mut ranked: Vec<(&str, f32)> = totals.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    ranked.truncate(NAMESPACE_SLOTS);
    ranked
}

fn filled(requested: f32, value: f32, basis: Basis) -> f32 {
    match basis {
        Basis::Requests => 1.,
        Basis::Limits if value > 0. => (requested / value).clamp(0., 1.),
        Basis::Limits => 1.,
    }
}

fn pod_parts(pod: &PodShare, rect: Rect, resource: Resource, basis: Basis) -> Vec<Part> {
    let total = pod.value(resource, basis);
    let across = rect.w >= rect.h;
    let mut offset = 0.;
    pod.containers
        .iter()
        .filter_map(|container| {
            let share = container.share(resource);
            let value = share.value(basis);
            if value <= 0. || total <= 0. {
                return None;
            }
            let fraction = value / total;
            let part = if across {
                Rect {
                    x: rect.x + offset * rect.w,
                    y: rect.y,
                    w: fraction * rect.w,
                    h: rect.h,
                }
            } else {
                Rect {
                    x: rect.x,
                    y: rect.y + offset * rect.h,
                    w: rect.w,
                    h: fraction * rect.h,
                }
            };
            offset += fraction;
            Some(Part {
                rect: part,
                filled: filled(share.request, value, basis),
                sidecar: container.sidecar,
            })
        })
        .collect()
}

pub fn squarify(values: &[f32], bounds: Rect) -> Vec<Rect> {
    let sum: f32 = values.iter().sum();
    if sum <= 0. || bounds.area() <= 0. {
        return values.iter().map(|_| Rect::default()).collect();
    }
    let scale = bounds.area() / sum;
    let areas: Vec<f32> = values.iter().map(|v| v * scale).collect();
    let mut out = Vec::with_capacity(areas.len());
    let mut free = bounds;
    let mut start = 0;
    while start < areas.len() {
        let side = free.w.min(free.h);
        let mut end = start + 1;
        while end < areas.len()
            && worst(&areas[start..=end], side) <= worst(&areas[start..end], side)
        {
            end += 1;
        }
        free = lay_row(&areas[start..end], free, &mut out);
        start = end;
    }
    out
}

fn worst(row: &[f32], side: f32) -> f32 {
    let sum: f32 = row.iter().sum();
    if sum <= 0. || side <= 0. {
        return f32::INFINITY;
    }
    row.iter()
        .filter(|area| **area > 0.)
        .map(|area| {
            let wide = side * side * area / (sum * sum);
            let tall = sum * sum / (side * side * area);
            wide.max(tall)
        })
        .fold(0., f32::max)
}

fn lay_row(row: &[f32], free: Rect, out: &mut Vec<Rect>) -> Rect {
    let sum: f32 = row.iter().sum();
    if free.w >= free.h {
        let w = if free.h > 0. { sum / free.h } else { 0. };
        let mut y = free.y;
        for area in row {
            let h = if w > 0. { area / w } else { 0. };
            out.push(Rect { x: free.x, y, w, h });
            y += h;
        }
        Rect {
            x: free.x + w,
            y: free.y,
            w: (free.w - w).max(0.),
            h: free.h,
        }
    } else {
        let h = if free.w > 0. { sum / free.w } else { 0. };
        let mut x = free.x;
        for area in row {
            let w = if h > 0. { area / h } else { 0. };
            out.push(Rect { x, y: free.y, w, h });
            x += w;
        }
        Rect {
            x: free.x,
            y: free.y + h,
            w: free.w,
            h: (free.h - h).max(0.),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f32 = 1e-3;

    fn container(name: &str, sidecar: bool, cpu: f32, limit: Option<f32>) -> ContainerShare {
        ContainerShare {
            name: name.into(),
            sidecar,
            cpu: Share {
                request: cpu,
                limit,
            },
            memory: Share {
                request: cpu * 2.,
                limit: limit.map(|l| l * 2.),
            },
        }
    }

    fn pod(namespace: &str, name: &str, containers: Vec<ContainerShare>) -> PodShare {
        PodShare {
            namespace: namespace.into(),
            name: name.into(),
            containers,
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < EPSILON
    }

    #[test]
    fn squarify_fills_the_bounds_without_overlap() {
        let bounds = Rect {
            x: 0.,
            y: 0.,
            w: 760.,
            h: 270.,
        };
        let rects = squarify(&[6., 6., 4., 3., 2., 2., 1., 1.], bounds);
        let area: f32 = rects.iter().map(|r| r.area()).sum();
        assert!(close(area / bounds.area(), 1.));
        for (i, a) in rects.iter().enumerate() {
            assert!(a.x >= -EPSILON && a.y >= -EPSILON);
            assert!(a.x + a.w <= bounds.w + EPSILON && a.y + a.h <= bounds.h + EPSILON);
            for b in &rects[i + 1..] {
                let overlap_w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
                let overlap_h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
                assert!(overlap_w <= EPSILON || overlap_h <= EPSILON);
            }
        }
    }

    #[test]
    fn squarify_keeps_tiles_near_square() {
        let rects = squarify(
            &[1.; 16],
            Rect {
                x: 0.,
                y: 0.,
                w: 400.,
                h: 400.,
            },
        );
        for r in rects {
            assert!(close(r.w, 100.) && close(r.h, 100.));
        }
    }

    #[test]
    fn squarify_of_nothing_is_empty_rects() {
        assert_eq!(
            squarify(
                &[0., 0.],
                Rect {
                    x: 0.,
                    y: 0.,
                    w: 10.,
                    h: 10.
                }
            ),
            vec![Rect::default(); 2]
        );
    }

    #[test]
    fn requests_leave_the_unrequested_share_free() {
        let pods = [
            pod(
                "ml",
                "trainer-0",
                vec![container("trainer", false, 3., Some(6.))],
            ),
            pod(
                "web",
                "web-6c9-qq2lm",
                vec![container("next", false, 1., None)],
            ),
        ];
        let map = Treemap::build(&pods, 8., Resource::Cpu, Basis::Requests, 100., 100.);
        assert_eq!(map.tiles.len(), 2);
        assert!(close(map.total, 4.));
        assert!(close(map.free_value, 4.));
        assert!(map.free.is_some_and(|free| close(free.area(), 5000.)));
        assert!(close(map.tiles[0].rect.area(), 3750.));
        assert_eq!(map.tiles[0].subject, Subject::Pod(0));
        assert!(!map.overcommitted());
    }

    #[test]
    fn limits_overcommit_fills_everything_and_marks_the_requested_part() {
        let pods = [pod(
            "ml",
            "trainer-0",
            vec![
                container("trainer", false, 3., Some(6.)),
                container("envoy", true, 1., None),
            ],
        )];
        let map = Treemap::build(&pods, 5., Resource::Cpu, Basis::Limits, 70., 10.);
        assert!(close(map.total, 7.));
        assert!(map.overcommitted());
        assert_eq!(map.free, None);
        let parts = &map.tiles[0].parts;
        assert_eq!(parts.len(), 2);
        assert!(close(parts[0].filled, 0.5));
        assert!(close(parts[0].rect.w, 60.));
        assert!(close(parts[1].filled, 1.) && parts[1].sidecar);
    }

    #[test]
    fn small_pods_merge_into_one_rest_tile_coloured_other() {
        let mut pods: Vec<PodShare> = (0..TILED_PODS + 3)
            .map(|i| {
                pod(
                    "ns",
                    &format!("p{i:02}"),
                    vec![container("c", false, 1., None)],
                )
            })
            .collect();
        pods.push(pod("ns", "idle", vec![container("c", false, 0., None)]));
        let map = Treemap::build(&pods, 100., Resource::Cpu, Basis::Requests, 100., 100.);
        assert_eq!(map.tiles.len(), TILED_PODS + 1);
        let rest = map.tiles.last().map(|t| (&t.subject, t.slot, t.value));
        assert_eq!(rest, Some((&Subject::Rest, Slot::Other, 3.)));
        assert_eq!(map.rest.len(), 4);
    }

    #[test]
    fn namespaces_take_colour_slots_by_size_and_the_rest_share_other() {
        let pods: Vec<PodShare> = (0..=NAMESPACE_SLOTS)
            .map(|i| {
                let cpu = f32::from(u8::try_from(10 - i).unwrap_or_default());
                pod(
                    &format!("ns{i}"),
                    "p",
                    vec![container("c", false, cpu, None)],
                )
            })
            .collect();
        let map = Treemap::build(&pods, 100., Resource::Cpu, Basis::Requests, 100., 100.);
        assert_eq!(map.tiles[0].slot, Slot::Namespace(0));
        assert_eq!(map.tiles[NAMESPACE_SLOTS].slot, Slot::Other);
        let labels: Vec<&str> = map.legend.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, ["ns0", "ns1", "ns2", "ns3", "ns4", "ns5", OTHER]);
        assert!(close(map.legend.iter().map(|e| e.value).sum(), map.total));
    }

    #[test]
    fn amounts_read_like_kubernetes_quantities() {
        assert_eq!(amount(Resource::Cpu, 0.85), "850m");
        assert_eq!(amount(Resource::Cpu, 3.), "3");
        assert_eq!(amount(Resource::Cpu, 1.65), "1.65");
        assert_eq!(amount(Resource::Cpu, 15.891), "15.89");
        assert_eq!(amount(Resource::Memory, 0.5), "512Mi");
        assert_eq!(amount(Resource::Memory, 3.5), "3.5Gi");
        assert_eq!(amount(Resource::Memory, 16.), "16Gi");
        assert_eq!(amount(Resource::Memory, 245.4), "245Gi");
    }

    #[test]
    fn workload_names_drop_replica_set_hashes_only() {
        assert_eq!(workload_name("checkout-api-7f9c6d-m8vtn"), "checkout-api");
        assert_eq!(workload_name("ml-batch-5c7f-x8n2q"), "ml-batch");
        assert_eq!(workload_name("trino-worker-3"), "trino-worker-3");
        assert_eq!(
            workload_name("otel-collector-8k2xd"),
            "otel-collector-8k2xd"
        );
        assert_eq!(workload_name("api"), "api");
    }

    #[test]
    fn pod_lines_show_requests_against_limits() {
        let p = pod(
            "ml",
            "trainer-0",
            vec![
                container("trainer", false, 3., Some(6.)),
                container("envoy", true, 0.2, None),
            ],
        );
        assert_eq!(p.summary(Resource::Cpu), "3.2 req · — lim");
        assert_eq!(
            p.container_lines(Resource::Cpu),
            [
                ("trainer".to_owned(), false, "3 / 6".to_owned()),
                ("envoy".to_owned(), true, "200m / —".to_owned())
            ]
        );
    }
}
