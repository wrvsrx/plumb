//! All-time task accounting and local event durations for decorative consumers.
use super::*;
use crate::QueryResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationKind {
    Task,
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

fn event_duration(event: &EventRecord) -> Option<DurationValue> {
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
}

impl Workspace {
    /// Aggregate current memory/store facts once, without exporting event collections.
    pub fn task_duration_totals(&self) -> Result<TaskDurationTotals, String> {
        let mut totals = self.task_accounting(None, false)?.totals;
        totals.complete &= self.query_result(()).is_complete();
        Ok(totals)
    }

    fn task_accounting(
        &self,
        path: Option<&Path>,
        include_sources: bool,
    ) -> Result<TaskAccounting, String> {
        let selected = self.selected_events_in_scope(|_| true)?;
        let mut issues = selected.issues;
        let mut context = AccountingContext::default();
        let mut totals = BTreeMap::<AgendaItem, f64>::new();
        let mut sources = BTreeMap::<AgendaItem, BTreeSet<AgendaLocation>>::new();
        for (event_path, event) in selected.events {
            let source = location(&event_path, event.selection_range.clone());
            match event_duration(&event) {
                Some(DurationValue::Seconds(duration)) => {
                    let (shares, event_issues) = self.event_accounting_with_context(
                        &event_path,
                        &event,
                        duration,
                        &mut context,
                    )?;
                    issues.extend(event_issues);
                    for share in shares {
                        if !share.is_task {
                            continue;
                        }
                        if let Some(item) = share
                            .item
                            .filter(|item| path.is_none_or(|path| item.path == path))
                        {
                            *totals.entry(item.clone()).or_default() += share.seconds;
                            if include_sources {
                                sources.entry(item).or_default().insert(source.clone());
                            }
                        }
                    }
                }
                Some(DurationValue::Unavailable) => issues.push(issue(
                    "agenda.invalid-time",
                    "event has no valid finite interval",
                    source,
                )),
                _ => {}
            }
        }
        let complete = issues.is_empty();
        let issues = if include_sources {
            issues
                .into_iter()
                .map(|issue| issue.source)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        } else {
            Vec::new()
        };
        Ok(TaskAccounting {
            totals: TaskDurationTotals {
                seconds: totals,
                complete,
            },
            sources,
            issues,
        })
    }

    /// Batch all task totals in one document over current workspace event facts.
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
        if !output.tasks().tasks.is_empty() {
            let accounting = self.task_accounting(Some(&path), true)?;
            let issue_sources = accounting.issues;
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
        Ok(self.query_result(annotations))
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
