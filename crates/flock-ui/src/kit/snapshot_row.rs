use std::ops::Deref;
use std::rc::Rc;
use std::sync::Arc;

use flock_core::filter::{Search, Searchable, TextFilter};
use flock_core::{ClusterId, Snapshot};

type Items<T> = Box<dyn Fn(&Snapshot) -> &[T]>;

enum Source<T: 'static> {
    Shared(Items<T>),
    Merged(Vec<T>),
}

pub struct SnapshotRows<T: 'static> {
    snapshot: Arc<Snapshot>,
    source: Source<T>,
    clusters: Vec<ClusterId>,
    search: Search<T>,
}

impl<T: 'static> SnapshotRows<T> {
    pub fn new(snapshot: &Arc<Snapshot>, items: impl Fn(&Snapshot) -> &[T] + 'static) -> Rc<Self> {
        Rc::new(Self {
            snapshot: Arc::clone(snapshot),
            source: Source::Shared(Box::new(items)),
            clusters: Vec::new(),
            search: Search::default(),
        })
    }

    pub fn across<'a>(
        snapshot: &'a Arc<Snapshot>,
        items: impl Fn(&'a Snapshot, &ClusterId) -> &'a [T],
    ) -> Rc<Self>
    where
        T: Clone,
    {
        let mut merged = Vec::new();
        let mut clusters = Vec::new();
        for cluster in &snapshot.clusters {
            let rows = items(snapshot, &cluster.id);
            merged.extend_from_slice(rows);
            clusters.extend(std::iter::repeat_n(cluster.id.clone(), rows.len()));
        }
        Rc::new(Self {
            snapshot: Arc::clone(snapshot),
            source: Source::Merged(merged),
            clusters,
            search: Search::default(),
        })
    }

    pub fn cluster(&self, ix: usize) -> Option<&ClusterId> {
        self.clusters.get(ix)
    }

    pub fn cluster_name(&self, ix: usize) -> &str {
        let id = self.cluster(ix);
        let summary = id.and_then(|id| self.snapshot.cluster(id));
        summary.map_or_else(|| id.map_or("", ClusterId::as_str), |c| c.name.as_str())
    }

    pub fn empty() -> Rc<Self> {
        Self::new(&Arc::default(), |_| &[])
    }

    pub fn items(&self) -> &[T] {
        match &self.source {
            Source::Shared(items) => items(&self.snapshot),
            Source::Merged(items) => items,
        }
    }

    pub fn snapshot(&self) -> &Arc<Snapshot> {
        &self.snapshot
    }

    pub fn from(&self, snapshot: &Arc<Snapshot>) -> bool {
        Arc::ptr_eq(&self.snapshot, snapshot)
    }

    pub fn rows(self: &Rc<Self>, indices: impl IntoIterator<Item = usize>) -> Vec<SnapshotRow<T>> {
        let len = self.items().len();
        indices
            .into_iter()
            .filter(|&ix| ix < len)
            .map(|ix| SnapshotRow {
                rows: Rc::clone(self),
                ix,
            })
            .collect()
    }

    pub fn matching(&self, query: &TextFilter) -> Rc<[usize]>
    where
        T: Searchable,
    {
        self.search.matching(self.items(), query)
    }

    pub fn matched_items(&self, query: &TextFilter) -> Vec<(usize, &T)>
    where
        T: Searchable,
    {
        let items = self.items();
        self.matching(query)
            .iter()
            .filter_map(|&ix| items.get(ix).map(|item| (ix, item)))
            .collect()
    }

    pub fn all(self: &Rc<Self>) -> Vec<SnapshotRow<T>> {
        self.rows(0..self.items().len())
    }
}

pub struct SnapshotRow<T: 'static> {
    rows: Rc<SnapshotRows<T>>,
    ix: usize,
}

impl<T: 'static> SnapshotRow<T> {
    pub fn index(&self) -> usize {
        self.ix
    }

    pub fn snapshot(&self) -> &Arc<Snapshot> {
        self.rows.snapshot()
    }

    pub fn cluster(&self) -> Option<&ClusterId> {
        self.rows.cluster(self.ix)
    }

    pub fn cluster_name(&self) -> &str {
        self.rows.cluster_name(self.ix)
    }
}

impl<T: 'static> Clone for SnapshotRow<T> {
    fn clone(&self) -> Self {
        Self {
            rows: Rc::clone(&self.rows),
            ix: self.ix,
        }
    }
}

impl<T: 'static> Deref for SnapshotRow<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.rows.items()[self.ix]
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use flock_core::events::{Event, EventDetails};
    use flock_core::{ClusterDetail, ClusterId, Status};

    use super::*;

    pub(crate) fn event(reason: &str, status: Status) -> Event {
        Event {
            labels: BTreeMap::new(),
            details: EventDetails::default(),
            status,
            reason: reason.into(),
            object: format!("pod/{reason}"),
            namespace: None,
            message: String::new(),
            age: Duration::ZERO,
            count: 1,
        }
    }

    pub(crate) fn events(cluster: &ClusterId, events: Vec<Event>) -> Rc<SnapshotRows<Event>> {
        let detail = ClusterDetail {
            events,
            ..ClusterDetail::default()
        };
        let snapshot = Arc::new(Snapshot {
            details: BTreeMap::from([(cluster.clone(), detail)]),
            ..Snapshot::default()
        });
        let id = cluster.clone();
        SnapshotRows::new(&snapshot, move |snapshot| {
            snapshot
                .detail(&id)
                .map_or(&[][..], |detail| detail.events.as_slice())
        })
    }

    #[test]
    fn rows_read_their_items_through_the_shared_snapshot() {
        let cluster = ClusterId::new("a");
        let source = events(
            &cluster,
            vec![event("first", Status::Ok), event("second", Status::Err)],
        );
        let rows = source.rows([1, 0, 2]);
        let reasons: Vec<&str> = rows.iter().map(|row| row.reason.as_str()).collect();
        assert_eq!(reasons, ["second", "first"]);
        assert!(
            rows.iter()
                .all(|row| Arc::ptr_eq(row.snapshot(), source.snapshot()))
        );
        assert_eq!(rows.first().map(SnapshotRow::index), Some(1));
        assert_eq!(source.all().len(), 2);
        assert_eq!(*source.matching(&TextFilter::parse("SEC")), [1]);
        let matched = source.matched_items(&TextFilter::parse("fir"));
        let found: Vec<(usize, &str)> = matched
            .iter()
            .map(|(ix, event)| (*ix, event.reason.as_str()))
            .collect();
        assert_eq!(found, [(0, "first")]);
    }

    #[test]
    fn rows_across_clusters_merge_in_cluster_order_and_remember_their_cluster() {
        let a = ClusterId::new("a");
        let b = ClusterId::new("b");
        let detail = |reasons: &[&str]| ClusterDetail {
            events: reasons.iter().map(|r| event(r, Status::Ok)).collect(),
            ..ClusterDetail::default()
        };
        let summary = |id: &ClusterId, name: &str| flock_core::ClusterSummary {
            id: id.clone(),
            name: name.into(),
            status: Status::Ok,
            state: String::new(),
            distro: String::new(),
            region: String::new(),
            version: None,
            nodes: None,
            problems: flock_core::Counts::default(),
            nav: BTreeMap::new(),
            metrics: None,
        };
        let snapshot = Arc::new(Snapshot {
            clusters: vec![summary(&b, "bee"), summary(&a, "ay")],
            details: BTreeMap::from([
                (a.clone(), detail(&["a1", "a2"])),
                (b.clone(), detail(&["b1"])),
            ]),
            ..Snapshot::default()
        });
        let source = SnapshotRows::across(&snapshot, |snapshot, id| {
            snapshot
                .detail(id)
                .map_or(&[][..], |detail| detail.events.as_slice())
        });
        let rows = source.all();
        let shown: Vec<(&str, &str)> = rows
            .iter()
            .map(|row| (row.reason.as_str(), row.cluster_name()))
            .collect();
        assert_eq!(shown, [("b1", "bee"), ("a1", "ay"), ("a2", "ay")]);
        assert_eq!(rows.get(1).and_then(SnapshotRow::cluster), Some(&a));
        assert!(source.from(&snapshot));
        assert_eq!(SnapshotRows::<Event>::empty().cluster(0), None);
    }

    #[test]
    fn a_source_knows_the_snapshot_it_reads() {
        let cluster = ClusterId::new("a");
        let source = events(&cluster, vec![event("first", Status::Ok)]);
        let same = Arc::clone(source.snapshot());
        let copy = Arc::new(Snapshot::clone(source.snapshot()));
        assert!(source.from(&same));
        assert!(!source.from(&copy));
        assert_eq!(SnapshotRows::<Event>::empty().items().len(), 0);
    }
}
