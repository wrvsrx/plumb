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
    pub priority: Option<i32>,
    pub due: Option<DateTime<FixedOffset>>,
    pub relevance: Option<i64>,
}

struct TaskTree<T> {
    record: T,
    facts: TaskSortFacts,
    children: Vec<TaskTree<T>>,
    priority: i32,
    due: Option<DateTime<FixedOffset>>,
    relevance: Option<i64>,
}

struct TaskDocument<T> {
    path: String,
    children: Vec<TaskTree<T>>,
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

fn aggregate_order(
    left_priority: i32,
    right_priority: i32,
    left_due: Option<&DateTime<FixedOffset>>,
    right_due: Option<&DateTime<FixedOffset>>,
    left_relevance: Option<i64>,
    right_relevance: Option<i64>,
    orders: &[TaskSortOrder],
) -> Ordering {
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
