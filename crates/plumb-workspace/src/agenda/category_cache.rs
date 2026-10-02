use super::*;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CategoryValue {
    declared: bool,
    invalid: bool,
    values: Vec<String>,
}
impl From<Category> for CategoryValue {
    fn from(value: Category) -> Self {
        Self {
            declared: !value.declarations.is_empty(),
            invalid: value.invalid,
            values: value.values,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TargetValue {
    valid: bool,
    is_task: bool,
    category: CategoryValue,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Input {
    category: CategoryValue,
    tasks_override: bool,
    references: Vec<(TargetKey, TargetValue)>,
}

/// Successful category decisions survive revisions; ranges never enter a key.
/// Malformed inputs are recomputed so their precise issue locations stay current.
#[derive(Clone, Debug, Default)]
pub struct CategoryCheckState {
    graph: CategoryGraph,
    pub extracted_events: usize,
    pub dependency_propagations: usize,
    pub recomputed_events: usize,
}

impl Workspace {
    pub fn check_event_categories_incremental(
        &self,
        root: &Path,
        now: DateTime<FixedOffset>,
        state: &mut CategoryCheckState,
    ) -> Result<CategoryCheckReport, String> {
        self.check_event_categories_incremental_in_scope(root, now, &[], state)
    }

    pub(crate) fn check_event_categories_incremental_in_scope(
        &self,
        root: &Path,
        now: DateTime<FixedOffset>,
        excluded: &[PathBuf],
        state: &mut CategoryCheckState,
    ) -> Result<CategoryCheckReport, String> {
        let _ = now;
        let result = self.check_category_graph(root, excluded, state);
        if result.is_err() {
            *state = CategoryCheckState::default();
        }
        result
    }
}

type NodeKey = (PathBuf, policy_inputs::NodeIdentity);
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum TargetKey {
    Resolved { path: PathBuf, id: Option<String> },
    Invalid { path: PathBuf, spelling: String },
}
impl TargetKey {
    fn new(from: &Path, target: &TaskReferenceTarget, spelling: String) -> Self {
        match target {
            TaskReferenceTarget::Internal { id } => Self::Resolved {
                path: from.to_path_buf(),
                id: Some(id.clone()),
            },
            TaskReferenceTarget::External { path, id } => Self::Resolved {
                path: resolve_relative(from, path),
                id: Some(id.clone()),
            },
            TaskReferenceTarget::Document { path } => Self::Resolved {
                path: resolve_relative(from, path),
                id: None,
            },
            TaskReferenceTarget::Invalid => Self::Invalid {
                path: from.to_path_buf(),
                spelling,
            },
        }
    }
}
#[derive(Clone, Debug)]
struct CachedTarget {
    origin: PathBuf,
    target: TaskReferenceTarget,
    value: TargetValue,
    consumers: BTreeMap<NodeKey, bool>,
}

#[derive(Clone, Debug, Default)]
struct CategoryGraph {
    sources: policy_inputs::PolicySources,
    documents: BTreeMap<PathBuf, policy_inputs::DocumentStamp>,
    nodes: HashMap<NodeKey, CachedNode>,
    targets: HashMap<TargetKey, CachedTarget>,
    decisions: HashMap<(PathBuf, Input), (bool, u64, usize)>,
    epoch: u64,
}

#[derive(Clone, Debug)]
struct CachedNode {
    snapshot: policy_inputs::PolicyNode,
    category: CategoryValue,
    inherits_document: bool,
    dependencies: Vec<TargetKey>,
    missing: Vec<Range<usize>>,
    decisions: Vec<(PathBuf, Input)>,
    // Invalid inputs are deliberately not retained as successful conclusions.
    valid: bool,
}

impl Workspace {
    fn category_target_value(
        &self,
        path: &Path,
        target: &TaskReferenceTarget,
        context: &mut AccountingContext,
    ) -> Result<TargetValue, String> {
        let mut value = TargetValue {
            valid: false,
            is_task: false,
            category: Category::default().into(),
        };
        match self
            .resolve_task_reference_target(path, target)
            .map_err(|e| e.to_string())?
        {
            ResolvedTarget::Anchor { path, id, anchor } if anchor.list_item => {
                value.valid = true;
                value.is_task = context.is_task(self, &path, Some(&id))?;
                value.category = if anchor.category.declarations.is_empty() {
                    context.document_category(self, &path)?
                } else {
                    anchor.category
                }
                .into();
            }
            ResolvedTarget::Document { path } => {
                value.valid = true;
                value.is_task = context.is_task(self, &path, None)?;
                value.category = context.document_category(self, &path)?.into();
            }
            _ => {}
        }
        Ok(value)
    }

    fn check_category_graph(
        &self,
        root: &Path,
        excluded: &[PathBuf],
        state: &mut CategoryCheckState,
    ) -> Result<CategoryCheckReport, String> {
        let graph = &mut state.graph;
        graph.epoch += 1;
        state.extracted_events = 0;
        state.recomputed_events = 0;
        state.dependency_propagations = 0;
        let mut context = AccountingContext::default();
        let mut retired_decisions = std::collections::HashSet::new();
        let mut dirty = BTreeSet::new();
        let documents = self.policy_document_stamps()?;
        for (key, cached) in &mut graph.targets {
            let TargetKey::Resolved { path, .. } = key else {
                continue;
            };
            if graph.documents.get(path) == documents.get(path) {
                continue;
            }
            let current =
                self.category_target_value(&cached.origin, &cached.target, &mut context)?;
            if cached.value != current {
                let identity_changed =
                    cached.value.valid != current.valid || cached.value.is_task != current.is_task;
                for (consumer, category_dependency) in &cached.consumers {
                    if identity_changed || *category_dependency {
                        dirty.insert(consumer.clone());
                        state.dependency_propagations += 1;
                    }
                }
                cached.value = current;
            }
        }
        graph.documents = documents;
        let inputs = self.policy_inputs(root, excluded, &mut graph.sources)?;
        let mut report = CategoryCheckReport {
            complete: true,
            checked: 0,
            missing: Vec::new(),
            issues: inputs.issues,
        };
        let mut alive = std::collections::HashSet::new();
        for (path, node) in inputs.nodes {
            let path = &path;
            let document_category: CategoryValue = context.document_category(self, path)?.into();
            let key = (path.clone(), node.id());
            alive.insert(key.clone());
            report.checked += node.event_count();
            let reusable = graph.nodes.get(&key).is_some_and(|old| {
                old.valid
                    && old.snapshot.same_facts(&node)
                    && (!old.inherits_document || old.category == document_category)
                    && !dirty.contains(&key)
            });
            if !reusable {
                if let Some(old) = graph.nodes.remove(&key) {
                    for decision in old.decisions {
                        graph.decisions.get_mut(&decision).unwrap().2 -= 1;
                        retired_decisions.insert(decision);
                    }
                    for dep in old.dependencies {
                        if let Some(target) = graph.targets.get_mut(&dep) {
                            target.consumers.remove(&key);
                        }
                    }
                }
                let mut cached = CachedNode {
                    snapshot: node.clone(),
                    category: document_category.clone(),
                    inherits_document: false,
                    dependencies: Vec::new(),
                    missing: Vec::new(),
                    decisions: Vec::new(),
                    valid: true,
                };
                for event in node.events() {
                    if !event.accounting_valid {
                        continue;
                    }
                    state.extracted_events += 1;
                    let category: CategoryValue = if event.category.declarations.is_empty() {
                        cached.inherits_document = true;
                        context.document_category(self, path)?
                    } else {
                        event.category.clone()
                    }
                    .into();
                    let mut references = Vec::new();
                    for (target, spelling, _) in self.accounting_references(path, &event)? {
                        let dep = TargetKey::new(path, &target, spelling);
                        if !graph.targets.contains_key(&dep) {
                            let value = self.category_target_value(path, &target, &mut context)?;
                            graph.targets.insert(
                                dep.clone(),
                                CachedTarget {
                                    origin: path.clone(),
                                    target,
                                    value,
                                    consumers: BTreeMap::new(),
                                },
                            );
                        }
                        let target = graph.targets.get_mut(&dep).unwrap();
                        target
                            .consumers
                            .entry(key.clone())
                            .and_modify(|depends| *depends |= !category.declared)
                            .or_insert(!category.declared);
                        cached.dependencies.push(dep.clone());
                        let mut value = target.value.clone();
                        if category.declared {
                            value.category = Category::default().into();
                        }
                        references.push((dep, value));
                    }
                    references.sort_by(|a, b| a.0.cmp(&b.0));
                    references.dedup_by(|a, b| a.0 == b.0);
                    let input = Input {
                        category,
                        tasks_override: event.tasks_override,
                        references,
                    };
                    let decision_key = (path.clone(), input);
                    let missing = if let Some((missing, _, _)) = graph
                        .decisions
                        .get(&decision_key)
                        .filter(|(_, epoch, _)| *epoch < graph.epoch)
                    {
                        *missing
                    } else {
                        state.recomputed_events += 1;
                        let (shares, issues) =
                            self.event_accounting_with_context(path, &event, 0.0, &mut context)?;
                        let missing = issues.is_empty()
                            && shares.iter().any(|share| share.category.is_none());
                        if issues.is_empty() {
                            graph.decisions.entry(decision_key.clone()).or_insert((
                                missing,
                                graph.epoch,
                                0,
                            ));
                        } else {
                            cached.valid = false;
                            report.issues.extend(issues);
                        }
                        missing
                    };
                    if let Some(decision) = graph.decisions.get_mut(&decision_key) {
                        decision.2 += 1;
                        cached.decisions.push(decision_key);
                    }
                    if missing {
                        cached.missing.push(
                            event.selection_range.start - node.offset()
                                ..event.selection_range.end - node.offset(),
                        );
                    }
                }
                graph.nodes.insert(key.clone(), cached);
            }
            let cached = &graph.nodes[&key];
            report.missing.extend(cached.missing.iter().map(|range| {
                location(path, range.start + node.offset()..range.end + node.offset())
            }));
        }
        graph.nodes.retain(|key, node| {
            if alive.contains(key) {
                return true;
            }
            for decision in &node.decisions {
                graph.decisions.get_mut(decision).unwrap().2 -= 1;
                retired_decisions.insert(decision.clone());
            }
            for dep in &node.dependencies {
                if let Some(target) = graph.targets.get_mut(dep) {
                    target.consumers.remove(key);
                }
            }
            false
        });
        graph
            .targets
            .retain(|_, target| !target.consumers.is_empty());
        for decision in retired_decisions {
            if graph
                .decisions
                .get(&decision)
                .is_some_and(|value| value.2 == 0)
            {
                graph.decisions.remove(&decision);
            }
        }
        report.complete = !report
            .issues
            .iter()
            .any(|issue| issue.code == "agenda.invalid-document");
        Ok(report)
    }
}
