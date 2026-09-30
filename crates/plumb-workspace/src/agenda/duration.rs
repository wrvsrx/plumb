//! All-time item accounting and local event durations for decorative consumers.
use super::*;
use crate::QueryResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationKind {
    Task,
    Item,
    Event,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DurationValue {
    Seconds(f64),
    Ongoing,
    Unavailable,
    Incomplete,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DurationAnnotation {
    pub range: Range<usize>,
    pub kind: DurationKind,
    pub value: DurationValue,
    /// Contributing events, or issue sources when accounting is incomplete.
    pub sources: Vec<AgendaLocation>,
}

pub(super) fn event_duration(event: &EventRecord) -> Option<DurationValue> {
    if event.at_datetime().is_some() {
        return None;
    }
    if event.is_running() && event.start_datetime().is_some() {
        return Some(DurationValue::Ongoing);
    }
    Some(match (event.start_datetime(), event.end_datetime()) {
        (Some(start), Some(end)) if end > start => DurationValue::Seconds(seconds(start, end)),
        _ => DurationValue::Unavailable,
    })
}

/// All-time task allocations. Missing identities mean zero only when complete.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskDurationTotals {
    pub seconds: BTreeMap<AgendaItem, f64>,
    pub complete: bool,
}

impl TaskDurationTotals {
    pub fn seconds_for(&self, path: &Path, owner: TaskOwner, id: Option<&str>) -> Option<f64> {
        if !self.complete {
            return None;
        }
        if owner == TaskOwner::ListItem && id.is_none() {
            return Some(0.0);
        }
        Some(
            self.seconds
                .get(&AgendaItem {
                    path: normalize(path),
                    id: id.map(str::to_owned),
                })
                .copied()
                .unwrap_or_default(),
        )
    }
}

struct TaskAccounting {
    totals: TaskDurationTotals,
    sources: BTreeMap<AgendaItem, BTreeSet<AgendaLocation>>,
    issues: Vec<AgendaLocation>,
    pending: bool,
}

impl Workspace {
    /// Explicit whole-result projection; interactive pages use task_durations_for.
    pub fn task_duration_totals(&self) -> Result<TaskDurationTotals, String> {
        self.with_duration_index(|index| TaskDurationTotals {
            seconds: index.task_totals(),
            complete: index.complete(),
        })
    }

    /// Only materialize totals for requested identities, never all workspace tasks.
    pub fn task_durations_for(
        &self,
        items: impl IntoIterator<Item = AgendaItem>,
    ) -> Result<TaskDurationTotals, String> {
        self.with_duration_index(|index| TaskDurationTotals {
            seconds: items
                .into_iter()
                .map(|mut item| {
                    item.path = normalize(&item.path);
                    let total = index.total(&item, true).unwrap_or_default();
                    (item, total)
                })
                .collect(),
            complete: index.complete(),
        })
    }

    fn document_accounting(&self, path: &Path) -> Result<TaskAccounting, String> {
        let output = self
            .current_output(path)
            .expect("current duration document");
        let event_owners = output
            .events()
            .events
            .iter()
            .map(|e| e.range.start)
            .collect::<BTreeSet<_>>();
        let items = output
            .anchors()
            .iter()
            .filter(|a| a.list_item && !event_owners.contains(&a.range.start))
            .map(|a| AgendaItem {
                path: path.to_owned(),
                id: Some(a.id.value),
            })
            .chain(
                output
                    .tasks()
                    .tasks
                    .views()
                    .filter(|t| t.owner() == TaskOwner::Document)
                    .map(|_| AgendaItem {
                        path: path.to_owned(),
                        id: None,
                    }),
            )
            .collect::<BTreeSet<_>>();
        self.with_duration_index(|index| {
            let mut seconds = BTreeMap::new();
            let mut sources = BTreeMap::new();
            for item in items {
                if let Some(total) = index.total(&item, false) {
                    seconds.insert(item.clone(), total);
                    if index.complete() {
                        sources.insert(item.clone(), index.sources(&item).into_iter().collect());
                    }
                }
            }
            TaskAccounting {
                totals: TaskDurationTotals {
                    seconds,
                    complete: index.complete(),
                },
                sources,
                issues: index.issue_sources(),
                pending: index.pending(),
            }
        })
    }

    /// Batch task and associated ordinary-item totals over current workspace event facts.
    /// Pending generations remain partial; invalid facts produce explicit incomplete values.
    /// Closed documents are read from semantic storage, never reparsed.
    pub fn document_durations(
        &self,
        path: &Path,
    ) -> Result<QueryResult<Vec<DurationAnnotation>>, String> {
        let path = normalize(path);
        let Some(output) = self.current_output(&path) else {
            let mut result = self.query_result(Vec::new());
            result.completeness = QueryCompleteness::Partial;
            return Ok(result);
        };
        let mut annotations = Vec::new();
        let accounting = self.document_accounting(&path)?;
        let pending = accounting.pending;
        if !output.tasks().tasks.is_empty() || output.anchors().iter().any(|a| a.list_item) {
            let issue_sources = accounting.issues;
            let specialized_owners: BTreeSet<_> = output
                .tasks()
                .tasks
                .views()
                .map(|task| task.range().start)
                .chain(output.events().events.iter().map(|event| event.range.start))
                .collect();
            for anchor in output.anchors().iter().filter(|a| a.list_item) {
                if specialized_owners.contains(&anchor.range.start) {
                    continue;
                }
                let item = AgendaItem {
                    path: path.clone(),
                    id: Some(anchor.id.value.clone()),
                };
                let Some(total) = accounting.totals.seconds.get(&item) else {
                    continue;
                };
                annotations.push(DurationAnnotation {
                    range: anchor.range.start..anchor.range.start,
                    kind: DurationKind::Item,
                    value: if issue_sources.is_empty() {
                        DurationValue::Seconds(*total)
                    } else {
                        DurationValue::Incomplete
                    },
                    sources: if issue_sources.is_empty() {
                        accounting
                            .sources
                            .get(&item)
                            .map(|s| s.iter().cloned().collect())
                            .unwrap_or_default()
                    } else {
                        issue_sources.clone()
                    },
                });
            }
            for task in output.tasks().tasks.views() {
                let identity = (task.owner() == TaskOwner::Document || task.id_value().is_some())
                    .then(|| AgendaItem {
                        path: path.clone(),
                        id: task.id_value().map(str::to_owned),
                    });
                let total = identity
                    .as_ref()
                    .and_then(|item| accounting.totals.seconds.get(item))
                    .copied()
                    .unwrap_or_default();
                let sources = identity
                    .as_ref()
                    .and_then(|item| accounting.sources.get(item))
                    .map(|sources| sources.iter().cloned().collect())
                    .unwrap_or_default();
                let start = if task.owner() == TaskOwner::Document {
                    0
                } else {
                    task.range().start
                };
                annotations.push(DurationAnnotation {
                    range: start..start,
                    kind: DurationKind::Task,
                    value: if issue_sources.is_empty() {
                        DurationValue::Seconds(total)
                    } else {
                        DurationValue::Incomplete
                    },
                    sources: if issue_sources.is_empty() {
                        sources
                    } else {
                        issue_sources.clone()
                    },
                });
            }
        }
        for event in output.events().events.iter() {
            if let Some(value) = event_duration(&event) {
                annotations.push(DurationAnnotation {
                    range: event.range.start..event.range.start,
                    kind: DurationKind::Event,
                    value,
                    sources: vec![location(&path, event.selection_range)],
                });
            }
        }
        annotations.sort_by_key(|annotation| annotation.range.start);
        Ok(self.query_result_with_pending(annotations, pending))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_failures_are_errors_not_zero_or_incomplete_totals() {
        let store = crate::SqliteSemanticStore::open_in_memory().unwrap();
        let mut workspace = Workspace::with_sqlite_store(store.clone());
        workspace.insert("/notes/task.plumb", 0, "`+ task\n");
        store
            .execute_batch_for_test("DROP TABLE documents;")
            .unwrap();
        assert!(workspace
            .document_durations(Path::new("/notes/task.plumb"))
            .is_err());
    }
}
