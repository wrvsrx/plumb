use std::cmp::Ordering;
use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskSortOrder {
    Source,
    Priority,
    Due,
    Relevance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSortFacts {
    pub document: String,
    pub source_start: usize,
    pub depth: usize,
    /// Display-only: `TaskRecord::is_focused()` for this task alone. Focus never
    /// propagates to ancestors as a semantic fact; `subtree_focused` is the
    /// aggregation used for ordering.
    pub focused: bool,
    pub priority: Option<i32>,
    pub due: Option<DateTime<FixedOffset>>,
    pub relevance: Option<i64>,
}

struct TaskTree<T> {
    record: T,
    facts: TaskSortFacts,
    children: Vec<TaskTree<T>>,
    /// Display-only aggregate over this retained subtree (self or any retained
    /// descendant focused). Not written back onto any ancestor's `focused`.
    subtree_focused: bool,
    priority: i32,
    due: Option<DateTime<FixedOffset>>,
    relevance: Option<i64>,
}

struct TaskDocument<T> {
    path: String,
    children: Vec<TaskTree<T>>,
    subtree_focused: bool,
    priority: i32,
    due: Option<DateTime<FixedOffset>>,
    relevance: Option<i64>,
}

pub fn sort_task_records<T>(
    records: &mut Vec<T>,
    order: TaskSortOrder,
    facts: impl Fn(&T) -> TaskSortFacts,
) {
    let orders = match order {
        TaskSortOrder::Priority => vec![TaskSortOrder::Priority, TaskSortOrder::Due],
        order => vec![order],
    };
    sort_task_records_by(records, &orders, facts);
}

pub fn sort_task_records_by<T>(
    records: &mut Vec<T>,
    orders: &[TaskSortOrder],
    facts: impl Fn(&T) -> TaskSortFacts,
) {
    records.sort_by_key(|record| {
        let facts = facts(record);
        (facts.document, facts.source_start)
    });

    let mut grouped = BTreeMap::<String, Vec<(T, TaskSortFacts)>>::new();
    for record in std::mem::take(records) {
        let facts = facts(&record);
        grouped
            .entry(facts.document.clone())
            .or_default()
            .push((record, facts));
    }

    let mut documents = grouped
        .into_iter()
        .map(|(path, records)| {
            let mut children = task_forest(records);
            sort_forest(&mut children, orders);
            TaskDocument {
                path,
                subtree_focused: children.iter().any(|child| child.subtree_focused),
                priority: children
                    .iter()
                    .map(|child| child.priority)
                    .max()
                    .unwrap_or_default()
                    .max(0),
                due: children.iter().filter_map(|child| child.due).min(),
                relevance: children.iter().filter_map(|child| child.relevance).max(),
                children,
            }
        })
        .collect::<Vec<_>>();
    documents.sort_by(|left, right| {
        aggregate_order(
            left.subtree_focused,
            right.subtree_focused,
            left.priority,
            right.priority,
            left.due.as_ref(),
            right.due.as_ref(),
            left.relevance,
            right.relevance,
            orders,
        )
        .then_with(|| left.path.cmp(&right.path))
    });
    records.extend(
        documents
            .into_iter()
            .flat_map(|document| document.children.into_iter().flat_map(flatten_tree)),
    );
}

pub fn truncate_complete_task_documents<T>(
    records: &mut Vec<T>,
    limit: usize,
    document: impl Fn(&T) -> &str,
) {
    if records.len() <= limit {
        return;
    }
    if limit == 0 {
        records.clear();
        return;
    }
    let boundary = document(&records[limit - 1]).to_string();
    let end = records[limit..]
        .iter()
        .position(|record| document(record) != boundary)
        .map_or(records.len(), |offset| limit + offset);
    records.truncate(end);
}

fn task_forest<T>(records: Vec<(T, TaskSortFacts)>) -> Vec<TaskTree<T>> {
    let mut forest = Vec::new();
    let mut records = records.into_iter().peekable();
    while let Some((record, facts)) = records.next() {
        let mut descendants = Vec::new();
        while records
            .peek()
            .is_some_and(|(_, candidate)| candidate.depth > facts.depth)
        {
            descendants.push(records.next().expect("peeked task exists"));
        }
        let children = task_forest(descendants);
        let subtree_focused = facts.focused || children.iter().any(|child| child.subtree_focused);
        let priority = children
            .iter()
            .map(|child| child.priority)
            .chain(std::iter::once(facts.priority.unwrap_or_default()))
            .max()
            .unwrap_or_default();
        let due = children
            .iter()
            .filter_map(|child| child.due)
            .chain(facts.due)
            .min();
        let relevance = children
            .iter()
            .filter_map(|child| child.relevance)
            .chain(facts.relevance)
            .max();
        forest.push(TaskTree {
            record,
            facts,
            children,
            subtree_focused,
            priority,
            due,
            relevance,
        });
    }
    forest
}

fn sort_forest<T>(forest: &mut [TaskTree<T>], orders: &[TaskSortOrder]) {
    for tree in forest.iter_mut() {
        sort_forest(&mut tree.children, orders);
    }
    forest.sort_by(|left, right| {
        aggregate_order(
            left.subtree_focused,
            right.subtree_focused,
            left.priority,
            right.priority,
            left.due.as_ref(),
            right.due.as_ref(),
            left.relevance,
            right.relevance,
            orders,
        )
        .then_with(|| left.facts.source_start.cmp(&right.facts.source_start))
    });
}

/// Ordering shared by sibling subtrees and document groups. `subtree_focused`
/// is a fixed leading key so focus-first ordering survives empty, cleared, or
/// reordered user sort keys; the caller's stable source tie-break is unchanged.
#[allow(clippy::too_many_arguments)]
fn aggregate_order(
    left_focused: bool,
    right_focused: bool,
    left_priority: i32,
    right_priority: i32,
    left_due: Option<&DateTime<FixedOffset>>,
    right_due: Option<&DateTime<FixedOffset>>,
    left_relevance: Option<i64>,
    right_relevance: Option<i64>,
    orders: &[TaskSortOrder],
) -> Ordering {
    match (left_focused, right_focused) {
        (true, false) => return Ordering::Less,
        (false, true) => return Ordering::Greater,
        _ => {}
    }
    for order in orders {
        let ordering = match order {
            TaskSortOrder::Priority => right_priority.cmp(&left_priority),
            TaskSortOrder::Due => optional_order(left_due, right_due),
            TaskSortOrder::Relevance => match (left_relevance, right_relevance) {
                (Some(left), Some(right)) => right.cmp(&left),
                _ => Ordering::Equal,
            },
            TaskSortOrder::Source => Ordering::Equal,
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    Ordering::Equal
}

fn optional_order<T: Ord>(left: Option<&T>, right: Option<&T>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn flatten_tree<T>(tree: TaskTree<T>) -> Vec<T> {
    std::iter::once(tree.record)
        .chain(tree.children.into_iter().flat_map(flatten_tree))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Item {
        name: &'static str,
        facts: TaskSortFacts,
    }

    fn item(
        name: &'static str,
        document: &str,
        source_start: usize,
        depth: usize,
        focused: bool,
        priority: i32,
    ) -> Item {
        Item {
            name,
            facts: TaskSortFacts {
                document: document.to_string(),
                source_start,
                depth,
                focused,
                priority: Some(priority),
                due: None,
                relevance: None,
            },
        }
    }

    fn sorted(mut records: Vec<Item>, orders: &[TaskSortOrder]) -> Vec<&'static str> {
        sort_task_records_by(&mut records, orders, |item: &Item| item.facts.clone());
        records.into_iter().map(|item| item.name).collect()
    }

    fn sorted_items(mut records: Vec<Item>, orders: &[TaskSortOrder]) -> Vec<Item> {
        sort_task_records_by(&mut records, orders, |item: &Item| item.facts.clone());
        records
    }

    /// A focused task leads its own sibling list, and the containing subtree
    /// moves ahead of an unfocused sibling subtree.
    #[test]
    fn direct_focus_promotes_the_task_and_its_subtree() {
        let records = vec![
            item("A", "d", 0, 0, false, 0),
            item("A1", "d", 10, 1, false, 0),
            item("B", "d", 20, 0, false, 0),
            item("B1", "d", 30, 1, false, 0),
            item("B2", "d", 40, 1, true, 0),
        ];

        assert_eq!(
            sorted(records, &[TaskSortOrder::Source]),
            ["B", "B2", "B1", "A", "A1"]
        );
    }

    /// Any depth of focused descendant promotes every retained ancestor, and
    /// promotion never writes the focus fact onto an ancestor record.
    #[test]
    fn deep_focused_descendant_promotes_every_ancestor_subtree() {
        let records = vec![
            item("A", "d", 0, 0, false, 0),
            item("A1", "d", 10, 1, false, 0),
            item("A2", "d", 20, 2, false, 0),
            item("A3", "d", 30, 3, true, 0),
            item("B", "d", 40, 0, false, 100),
        ];

        let ordered = sorted_items(records, &[TaskSortOrder::Priority]);
        assert_eq!(
            ordered.iter().map(|item| item.name).collect::<Vec<_>>(),
            ["A", "A1", "A2", "A3", "B"]
        );
        for name in ["A", "A1", "A2"] {
            let record = ordered.iter().find(|item| item.name == name).unwrap();
            assert!(
                !record.facts.focused,
                "{name} must not be marked focused by aggregation"
            );
        }
    }

    /// A focused task in another document promotes the whole document group.
    #[test]
    fn focused_descendant_promotes_the_cross_document_group() {
        let records = vec![
            item("A", "a.plumb", 0, 0, false, 100),
            item("Z", "z.plumb", 0, 0, false, 0),
            item("Z1", "z.plumb", 10, 1, true, 0),
        ];

        assert_eq!(
            sorted(records, &[TaskSortOrder::Priority]),
            ["Z", "Z1", "A"]
        );
    }

    /// Several focused sibling subtrees fall back to the user's secondary keys,
    /// and their own focused children still lead inside each subtree.
    #[test]
    fn focused_sibling_subtrees_use_the_secondary_sort() {
        let records = vec![
            item("A", "d", 0, 0, false, 1),
            item("A1", "d", 10, 1, false, 0),
            item("A2", "d", 20, 1, true, 0),
            item("B", "d", 30, 0, false, 5),
            item("B1", "d", 40, 1, true, 0),
            item("B2", "d", 50, 1, false, 0),
        ];

        assert_eq!(
            sorted(records, &[TaskSortOrder::Priority]),
            ["B", "B1", "B2", "A", "A2", "A1"]
        );
    }

    /// Only retained tasks contribute: a filtered-out focused task cannot
    /// promote its ancestors, while a retained deep (folded-hidden) task does.
    #[test]
    fn filtered_out_tasks_do_not_promote_but_folded_hidden_ones_do() {
        let filtered = vec![
            item("A", "d", 0, 0, false, 1),
            item("A1", "d", 10, 1, false, 0),
            item("B", "d", 20, 0, false, 5),
            item("B1", "d", 30, 1, false, 0),
        ];
        assert_eq!(
            sorted(filtered, &[TaskSortOrder::Priority]),
            ["B", "B1", "A", "A1"]
        );

        let folded = vec![
            item("A", "d", 0, 0, false, 1),
            item("A1", "d", 10, 1, false, 0),
            item("A2", "d", 20, 2, true, 0),
            item("B", "d", 30, 0, false, 5),
        ];
        assert_eq!(
            sorted(folded, &[TaskSortOrder::Priority]),
            ["A", "A1", "A2", "B"]
        );
    }

    /// Focus-first is fixed: it survives cleared and reordered user keys.
    #[test]
    fn focus_first_survives_cleared_and_reordered_sort_keys() {
        let records = || {
            vec![
                item("A", "d", 0, 0, false, 9),
                item("A1", "d", 10, 1, false, 9),
                item("B", "d", 20, 0, false, 1),
                item("B1", "d", 30, 1, true, 1),
            ]
        };

        assert_eq!(sorted(records(), &[]), ["B", "B1", "A", "A1"]);
        assert_eq!(
            sorted(records(), &[TaskSortOrder::Due, TaskSortOrder::Priority]),
            ["B", "B1", "A", "A1"]
        );
    }
}
