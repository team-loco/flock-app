use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};

use crate::Status;

pub const NO_VALUE: &str = "—";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub value: String,
    pub rows: usize,
    pub errors: usize,
    pub warnings: usize,
    pub collapsed: bool,
}

impl Group {
    fn tier(&self) -> u8 {
        match (self.errors, self.warnings) {
            (0, 0) => 2,
            (0, _) => 1,
            _ => 0,
        }
    }

    fn order(&self, other: &Self) -> Ordering {
        let missing = |group: &Self| group.value == NO_VALUE;
        self.tier()
            .cmp(&other.tier())
            .then_with(|| missing(self).cmp(&missing(other)))
            .then_with(|| self.value.cmp(&other.value))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Group(usize),
    Row(usize),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grouped {
    pub groups: Vec<Group>,
    pub items: Vec<Item>,
}

pub fn group<'a>(
    rows: impl IntoIterator<Item = (usize, Option<Cow<'a, str>>, Status)>,
    collapsed: &BTreeSet<String>,
) -> Grouped {
    let mut index: HashMap<Cow<'a, str>, usize> = HashMap::new();
    let mut groups: Vec<Group> = Vec::new();
    let mut members: Vec<Vec<usize>> = Vec::new();
    for (row, value, status) in rows {
        let value = value
            .filter(|value| !value.is_empty())
            .unwrap_or(Cow::Borrowed(NO_VALUE));
        let next = groups.len();
        let at = *index.entry(value).or_insert_with_key(|value| {
            groups.push(Group {
                value: value.clone().into_owned(),
                rows: 0,
                errors: 0,
                warnings: 0,
                collapsed: collapsed.contains(value.as_ref()),
            });
            members.push(Vec::new());
            next
        });
        let (Some(group), Some(rows)) = (groups.get_mut(at), members.get_mut(at)) else {
            continue;
        };
        group.rows += 1;
        match status {
            Status::Err => group.errors += 1,
            Status::Warn => group.warnings += 1,
            Status::Ok => {}
            Status::Suspended => {}
            Status::Progress => {}
            Status::Offline => {}
        }
        rows.push(row);
    }
    let mut order: Vec<usize> = (0..groups.len()).collect();
    order.sort_by(|&a, &b| match (groups.get(a), groups.get(b)) {
        (Some(a), Some(b)) => a.order(b),
        _ => Ordering::Equal,
    });
    let mut sorted = Vec::with_capacity(groups.len());
    let mut items = Vec::new();
    for at in order {
        let (Some(group), Some(rows)) = (groups.get(at), members.get_mut(at)) else {
            continue;
        };
        items.push(Item::Group(sorted.len()));
        if !group.collapsed {
            items.extend(rows.drain(..).map(Item::Row));
        }
        sorted.push(group.clone());
    }
    Grouped {
        groups: sorted,
        items,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows<'a>(
        rows: &'a [(&'a str, Status)],
    ) -> impl Iterator<Item = (usize, Option<Cow<'a, str>>, Status)> {
        rows.iter()
            .enumerate()
            .map(|(ix, (value, status))| (ix, Some(Cow::Borrowed(*value)), *status))
    }

    fn values(grouped: &Grouped) -> Vec<&str> {
        grouped.groups.iter().map(|g| g.value.as_str()).collect()
    }

    #[test]
    fn groups_order_by_problems_then_name_and_keep_row_order_inside() {
        let data = [
            ("web", Status::Ok),
            ("api", Status::Ok),
            ("pay", Status::Warn),
            ("web", Status::Err),
            ("api", Status::Ok),
            ("db", Status::Ok),
            ("pay", Status::Ok),
        ];
        let grouped = group(rows(&data), &BTreeSet::new());
        assert_eq!(values(&grouped), ["web", "pay", "api", "db"]);
        assert_eq!(
            grouped.items,
            [
                Item::Group(0),
                Item::Row(0),
                Item::Row(3),
                Item::Group(1),
                Item::Row(2),
                Item::Row(6),
                Item::Group(2),
                Item::Row(1),
                Item::Row(4),
                Item::Group(3),
                Item::Row(5),
            ]
        );
    }

    #[test]
    fn headers_count_rows_errors_and_warnings() {
        let data = [
            ("a", Status::Err),
            ("a", Status::Warn),
            ("a", Status::Err),
            ("a", Status::Progress),
        ];
        let grouped = group(rows(&data), &BTreeSet::new());
        let expected = Group {
            value: "a".into(),
            rows: 4,
            errors: 2,
            warnings: 1,
            collapsed: false,
        };
        assert_eq!(grouped.groups, [expected]);
    }

    #[test]
    fn collapsed_groups_keep_their_header_and_hide_their_rows() {
        let data = [("a", Status::Ok), ("b", Status::Ok), ("a", Status::Ok)];
        let collapsed = BTreeSet::from(["a".to_owned()]);
        let grouped = group(rows(&data), &collapsed);
        assert_eq!(
            grouped.items,
            [Item::Group(0), Item::Group(1), Item::Row(1)]
        );
        assert!(
            grouped
                .groups
                .first()
                .is_some_and(|g| g.collapsed && g.rows == 2)
        );
    }

    #[test]
    fn rows_without_a_value_share_a_group_after_named_ones() {
        let data = vec![
            (0, None, Status::Ok),
            (1, Some(Cow::Borrowed("")), Status::Ok),
            (2, Some(Cow::Borrowed("zeta")), Status::Ok),
        ];
        let grouped = group(data, &BTreeSet::new());
        assert_eq!(values(&grouped), ["zeta", NO_VALUE]);
        assert_eq!(grouped.groups.get(1).map(|g| g.rows), Some(2));
    }

    #[test]
    fn no_rows_make_no_groups() {
        assert_eq!(group(Vec::new(), &BTreeSet::new()), Grouped::default());
    }
}
