//! Opt-in measurement of the production semantic pipeline, not a second analyzer.

use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use plumb_syntax::{GreenDocument, ValidGreenDocument};

use super::{
    DocumentChange, DocumentOutput, SemanticNodeOutput, SemanticRoot, SemanticStageObserver,
    SemanticTree,
};

#[derive(Debug, Default, Clone, Copy)]
pub struct SemanticStages {
    pub metadata_context: Duration,
    pub local_nodes: Duration,
    pub document_reduction: Duration,
}

struct TimedObserver {
    start: Instant,
    stages: SemanticStages,
}

impl SemanticStageObserver for TimedObserver {
    fn metadata_complete(&mut self) {
        let now = Instant::now();
        self.stages.metadata_context = now.duration_since(self.start);
        self.start = now;
    }

    fn local_complete(&mut self) {
        let now = Instant::now();
        self.stages.local_nodes = now.duration_since(self.start);
        self.start = now;
    }
}

pub fn analyze(
    valid: ValidGreenDocument<'_>,
    syntax: Arc<GreenDocument>,
    previous: Option<(&DocumentOutput, &DocumentChange)>,
) -> Option<(DocumentOutput, SemanticStages)> {
    if !std::ptr::eq(valid.syntax(), syntax.as_ref()) {
        return None;
    }
    let mut observer = TimedObserver {
        start: Instant::now(),
        stages: SemanticStages::default(),
    };
    let output = super::analyze_semantic_tree_observed(
        syntax,
        previous.map(|(output, _)| output),
        previous.map(|(_, change)| change),
        &mut observer,
    )?;
    observer.stages.document_reduction = observer.start.elapsed();
    Some((output, observer.stages))
}

pub struct LifetimeProbe {
    root: Weak<SemanticRoot>,
    tree: Weak<SemanticTree>,
    syntax: Weak<GreenDocument>,
    nodes: Vec<Weak<SemanticNodeOutput>>,
}

impl LifetimeProbe {
    pub fn root_tree_syntax_released(&self) -> bool {
        self.root.strong_count() == 0
            && self.tree.strong_count() == 0
            && self.syntax.strong_count() == 0
    }

    pub fn live_local_nodes(&self) -> usize {
        self.nodes
            .iter()
            .filter(|node| node.strong_count() != 0)
            .count()
    }
}

/// Untimed validation only: weak references defer the allocation header's deallocation.
pub fn lifetime_probe(output: &DocumentOutput) -> LifetimeProbe {
    LifetimeProbe {
        root: Arc::downgrade(&output.root),
        tree: Arc::downgrade(&output.root.tree),
        syntax: Arc::downgrade(&output.root.tree.syntax),
        nodes: output
            .root
            .tree
            .nodes
            .iter()
            .map(|node| Arc::downgrade(&node.output))
            .collect(),
    }
}

/// Internal root ownership matters independently of an outer Arc<DocumentOutput>.
pub fn root_owner_count(output: &DocumentOutput) -> usize {
    Arc::strong_count(&output.root)
}
