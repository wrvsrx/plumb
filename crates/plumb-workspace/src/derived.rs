//! Snapshot-safe input tracking and source contribution replacement.
//! No workspace enumeration is available inside a tracked computation.
use im::{OrdMap, OrdSet};
use std::collections::BTreeMap;

/// One lifecycle boundary for the statically composed derived consumers.
/// Add a new consumer here instead of adding hooks to each document adapter.
#[derive(Debug, Clone, Default)]
pub(crate) struct DerivedState {
    pub duration: crate::agenda::DurationCache,
    pub completion: crate::completion_index::LinkCompletionIndex,
}
impl DerivedState {
    pub fn document_changed(&mut self, path: std::path::PathBuf) {
        self.duration.changed(path.clone());
        self.completion.changed(path);
    }
    pub fn fork(&self) -> Self {
        Self {
            duration: self.duration.fork(),
            completion: self.completion.fork(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DerivedGraph<K: Ord + Clone, V: Clone, N: Ord + Clone, O: Clone> {
    inputs: OrdMap<K, V>,
    dependencies: OrdMap<N, OrdSet<K>>,
    reverse: OrdMap<K, OrdSet<N>>,
    outputs: OrdMap<N, O>,
    dirty: OrdSet<N>,
}
impl<K: Ord + Clone, V: Clone, N: Ord + Clone, O: Clone> Default for DerivedGraph<K, V, N, O> {
    fn default() -> Self {
        Self {
            inputs: OrdMap::new(),
            dependencies: OrdMap::new(),
            reverse: OrdMap::new(),
            outputs: OrdMap::new(),
            dirty: OrdSet::new(),
        }
    }
}

pub(crate) struct TrackedRead<'a, K: Ord + Clone, V: Clone> {
    inputs: &'a OrdMap<K, V>,
    reads: OrdSet<K>,
}
impl<K: Ord + Clone, V: Clone> TrackedRead<'_, K, V> {
    pub fn get(&mut self, key: K) -> Option<&V> {
        self.reads.insert(key.clone());
        self.inputs.get(&key)
    }
}

impl<K: Ord + Clone, V: Clone + PartialEq, N: Ord + Clone, O: Clone + PartialEq>
    DerivedGraph<K, V, N, O>
{
    pub fn set(&mut self, key: K, value: Option<V>) {
        if self.inputs.get(&key) == value.as_ref() {
            return;
        }
        match value {
            Some(v) => {
                self.inputs.insert(key.clone(), v);
            }
            None => {
                self.inputs.remove(&key);
            }
        }
        if let Some(consumers) = self.reverse.get(&key) {
            for node in consumers {
                self.dirty.insert(node.clone());
            }
        }
    }
    pub fn schedule(&mut self, node: N) {
        self.dirty.insert(node);
    }
    pub fn dirty(&self) -> Vec<N> {
        self.dirty.iter().cloned().collect()
    }
    pub fn output(&self, node: &N) -> Option<&O> {
        self.outputs.get(node)
    }
    pub fn evaluate<E>(
        &mut self,
        node: N,
        compute: impl FnOnce(&mut TrackedRead<'_, K, V>) -> Result<O, E>,
    ) -> Result<bool, E> {
        let mut context = TrackedRead {
            inputs: &self.inputs,
            reads: OrdSet::new(),
        };
        let output = compute(&mut context)?;
        let reads = context.reads;
        self.unlink(&node);
        for key in &reads {
            let mut consumers = self.reverse.get(key).cloned().unwrap_or_default();
            consumers.insert(node.clone());
            self.reverse.insert(key.clone(), consumers);
        }
        self.dependencies.insert(node.clone(), reads);
        self.dirty.remove(&node);
        if self.outputs.get(&node) == Some(&output) {
            return Ok(false);
        }
        self.outputs.insert(node, output);
        Ok(true)
    }
    fn unlink(&mut self, node: &N) {
        if let Some(reads) = self.dependencies.remove(node) {
            for key in reads {
                if let Some(mut consumers) = self.reverse.get(&key).cloned() {
                    consumers.remove(node);
                    if consumers.is_empty() {
                        self.reverse.remove(&key);
                    } else {
                        self.reverse.insert(key, consumers);
                    }
                }
            }
        }
    }
    pub fn remove(&mut self, node: &N) {
        self.unlink(node);
        self.outputs.remove(node);
        self.dirty.remove(node);
    }
}

/// The aggregate defines reversible contribution updates. Membership is retained
/// separately: a zero total is not the same as having no contributors.
pub(crate) trait Aggregate<V>: Clone + Default {
    fn add(&mut self, value: &V);
    fn retract(&mut self, value: &V);
}
#[derive(Debug, Clone)]
pub(crate) struct ContributionIndex<S: Ord + Clone, K: Ord + Clone, V: Clone, A: Clone> {
    sources: OrdMap<S, OrdMap<K, V>>,
    targets: OrdMap<K, (A, OrdSet<S>)>,
}
impl<S: Ord + Clone, K: Ord + Clone, V: Clone, A: Clone> Default for ContributionIndex<S, K, V, A> {
    fn default() -> Self {
        Self {
            sources: OrdMap::new(),
            targets: OrdMap::new(),
        }
    }
}
impl<S: Ord + Clone, K: Ord + Clone, V: Clone + PartialEq, A: Aggregate<V>>
    ContributionIndex<S, K, V, A>
{
    pub fn replace(&mut self, source: S, values: BTreeMap<K, V>) -> usize {
        let old = self.sources.remove(&source).unwrap_or_default();
        let mut changed = 0;
        for (key, value) in &old {
            if values.get(key) == Some(value) {
                continue;
            }
            changed += 1;
            let (mut aggregate, mut members) =
                self.targets.get(key).cloned().expect("contribution target");
            aggregate.retract(value);
            members.remove(&source);
            if members.is_empty() {
                self.targets.remove(key);
            } else {
                self.targets.insert(key.clone(), (aggregate, members));
            }
        }
        for (key, value) in &values {
            if old.get(key) == Some(value) {
                continue;
            }
            changed += 1;
            let (mut aggregate, mut members) = self.targets.get(key).cloned().unwrap_or_default();
            aggregate.add(value);
            members.insert(source.clone());
            self.targets.insert(key.clone(), (aggregate, members));
        }
        if !values.is_empty() {
            self.sources.insert(source, values.into_iter().collect());
        }
        changed
    }
    pub fn get(&self, key: &K) -> Option<&A> {
        self.targets.get(key).map(|(a, _)| a)
    }
    pub fn sources(&self, key: &K) -> impl Iterator<Item = &S> {
        self.targets
            .get(key)
            .into_iter()
            .flat_map(|(_, s)| s.iter())
    }
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.targets.keys()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tracked_negative_reads_equal_inputs_dependency_replacement_and_snapshots() {
        let mut graph = DerivedGraph::<&str, i32, &str, i32>::default();
        graph
            .evaluate("a", |c| Ok::<_, ()>(c.get("missing").copied().unwrap_or(0)))
            .unwrap();
        let old = graph.clone();
        graph.set("unrelated", Some(7));
        assert!(graph.dirty().is_empty());
        graph.set("missing", Some(3));
        assert_eq!(graph.dirty(), ["a"]);
        graph
            .evaluate("a", |c| Ok::<_, ()>(*c.get("unrelated").unwrap()))
            .unwrap();
        graph.set("missing", None);
        graph.set("unrelated", Some(7));
        assert!(graph.dirty().is_empty());
        assert_eq!(old.output(&"a"), Some(&0));
        assert_eq!(graph.output(&"a"), Some(&7));
        graph.set("unrelated", Some(8));
        assert!(graph.evaluate("a", |_| Err::<i32, _>(())).is_err());
        assert_eq!(graph.output(&"a"), Some(&7));
        assert_eq!(graph.dirty(), ["a"]);
        graph.remove(&"a");
        graph.set("unrelated", Some(9));
        assert!(graph.dirty().is_empty());
    }
    #[derive(Clone, Default)]
    struct Sum(i64);
    impl Aggregate<i64> for Sum {
        fn add(&mut self, v: &i64) {
            self.0 += v;
        }
        fn retract(&mut self, v: &i64) {
            self.0 -= v;
        }
    }
    #[test]
    fn contribution_replacement_retracts_removed_targets_without_touching_other_sources() {
        let mut index = ContributionIndex::<_, _, _, Sum>::default();
        index.replace("e", BTreeMap::from([("a", 1200), ("b", 1200)]));
        index.replace("other", BTreeMap::from([("a", 20)]));
        let old = index.clone();
        assert_eq!(
            index.replace("e", BTreeMap::from([("a", 1800), ("c", 1800)])),
            4
        );
        assert_eq!(index.get(&"a").unwrap().0, 1820);
        assert!(index.get(&"b").is_none());
        assert_eq!(old.get(&"a").unwrap().0, 1220);
        assert_eq!(
            index.replace("e", BTreeMap::from([("a", 1800), ("c", 1800)])),
            0
        );
        index.replace("e", BTreeMap::new());
        assert_eq!(index.get(&"a").unwrap().0, 20);
        assert!(index.get(&"c").is_none());
    }
}
