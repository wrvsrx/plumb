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

impl Workspace {
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
            let selected = self.selected_events_in_scope(|_| true)?;
            let mut issues = selected.issues;
            let mut context = AccountingContext::default();
            let mut totals = BTreeMap::<AgendaItem, (f64, BTreeSet<AgendaLocation>)>::new();
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
                            if let Some(item) = share.item.filter(|item| item.path == path) {
                                let (total, sources) = totals.entry(item).or_default();
                                *total += share.seconds;
                                sources.insert(source.clone());
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
            let issue_sources = issues
                .into_iter()
                .map(|issue| issue.source)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            for task in output.tasks().tasks.views() {
                let identity = (task.owner() == TaskOwner::Document || task.id_value().is_some())
                    .then(|| AgendaItem {
                        path: path.clone(),
                        id: task.id_value().map(str::to_owned),
                    });
                let (total, sources) = identity
                    .and_then(|item| totals.get(&item))
                    .map(|(total, sources)| (*total, sources.iter().cloned().collect()))
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
