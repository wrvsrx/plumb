//! Augmented AVL tree for point stabbing, including long spanning intervals.
use super::timeline_index::EventId;
use chrono::{DateTime, FixedOffset};
use std::collections::BTreeSet;
type Instant = DateTime<FixedOffset>;
type Key = (Instant, EventId);
type Link = Option<Box<Node>>;

#[derive(Clone, Debug)]
struct Node {
    key: Key,
    end: Instant,
    max_end: Instant,
    height: usize,
    left: Link,
    right: Link,
}
impl Node {
    fn update(&mut self) {
        self.height = 1 + height(&self.left).max(height(&self.right));
        self.max_end = self
            .end
            .max(self.left.as_ref().map_or(self.end, |n| n.max_end))
            .max(self.right.as_ref().map_or(self.end, |n| n.max_end));
    }
}
fn height(node: &Link) -> usize {
    node.as_ref().map_or(0, |node| node.height)
}
fn rotate_left(mut root: Box<Node>) -> Box<Node> {
    let mut next = root.right.take().unwrap();
    root.right = next.left.take();
    root.update();
    next.left = Some(root);
    next.update();
    next
}
fn rotate_right(mut root: Box<Node>) -> Box<Node> {
    let mut next = root.left.take().unwrap();
    root.left = next.right.take();
    root.update();
    next.right = Some(root);
    next.update();
    next
}
fn balance(mut root: Box<Node>) -> Box<Node> {
    root.update();
    if height(&root.left) > height(&root.right) + 1 {
        let left = root.left.as_ref().unwrap();
        if height(&left.right) > height(&left.left) {
            root.left = root.left.take().map(rotate_left);
        }
        return rotate_right(root);
    }
    if height(&root.right) > height(&root.left) + 1 {
        let right = root.right.as_ref().unwrap();
        if height(&right.left) > height(&right.right) {
            root.right = root.right.take().map(rotate_right);
        }
        return rotate_left(root);
    }
    root
}
fn insert(root: Link, key: Key, end: Instant) -> Box<Node> {
    let Some(mut root) = root else {
        return Box::new(Node {
            key,
            end,
            max_end: end,
            height: 1,
            left: None,
            right: None,
        });
    };
    match key.cmp(&root.key) {
        std::cmp::Ordering::Less => root.left = Some(insert(root.left.take(), key, end)),
        std::cmp::Ordering::Greater => root.right = Some(insert(root.right.take(), key, end)),
        std::cmp::Ordering::Equal => root.end = end,
    }
    balance(root)
}
fn remove(root: Link, key: Key) -> Link {
    let mut root = root?;
    match key.cmp(&root.key) {
        std::cmp::Ordering::Less => root.left = remove(root.left.take(), key),
        std::cmp::Ordering::Greater => root.right = remove(root.right.take(), key),
        std::cmp::Ordering::Equal => {
            if root.left.is_none() {
                return root.right;
            }
            if root.right.is_none() {
                return root.left;
            }
            let mut next = root.right.as_ref().unwrap();
            while let Some(left) = &next.left {
                next = left;
            }
            root.key = next.key;
            root.end = next.end;
            root.right = remove(root.right.take(), root.key);
        }
    }
    Some(balance(root))
}
fn query(root: &Link, at: Instant, result: &mut BTreeSet<EventId>, visited: &mut usize) {
    let Some(root) = root else {
        return;
    };
    *visited += 1;
    if root.max_end <= at {
        return;
    }
    query(&root.left, at, result, visited);
    if root.key.0 <= at {
        if at < root.end {
            result.insert(root.key.1);
        }
        query(&root.right, at, result, visited);
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct IntervalTree {
    root: Link,
}
impl IntervalTree {
    pub fn insert(&mut self, start: Instant, end: Instant, id: EventId) {
        self.root = Some(insert(self.root.take(), (start, id), end));
    }
    pub fn remove(&mut self, start: Instant, id: EventId) {
        self.root = remove(self.root.take(), (start, id));
    }
    pub fn containing(&self, at: Instant) -> (BTreeSet<EventId>, usize) {
        let mut result = BTreeSet::new();
        let mut visited = 0;
        query(&self.root, at, &mut result, &mut visited);
        (result, visited)
    }
}
