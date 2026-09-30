//! Duration's statically typed instance of the shared dependency/contribution graph.
use super::duration::{event_duration, DurationValue};
use super::*;
use crate::derived::{Aggregate, ContributionIndex, DerivedGraph, TrackedRead};
use im::{OrdMap, OrdSet};
use std::sync::{Arc, Mutex};

type EventKey = (PathBuf, usize);
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum InputKey {
    Event(EventKey),
    Identity(AgendaItem),
    Category(AgendaItem),
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct CategoryValue {
    declared: bool,
    invalid: bool,
    values: Vec<String>,
}
impl From<&Category> for CategoryValue {
    fn from(c: &Category) -> Self {
        Self {
            declared: !c.declarations.is_empty(),
            invalid: c.invalid,
            values: c.values.clone(),
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
struct EventValue {
    duration: Option<DurationValue>,
    category: CategoryValue,
    tasks_override: bool,
    references: Vec<(TaskReferenceTarget, String)>,
}
#[derive(Debug, Clone, PartialEq)]
enum InputValue {
    Event(EventValue),
    Identity { valid: bool, task: bool },
    Category(CategoryValue),
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum GeometryKey {
    Event(EventKey),
    Reference(EventKey, usize),
    EventCategory(EventKey),
    Category(AgendaItem),
}
#[derive(Debug, Clone, PartialEq, Default)]
struct EventOutput {
    contributions: BTreeMap<(AgendaItem, bool), f64>,
    issues: Vec<GeometryKey>,
}

/// Compensated addition supports retractions without repeatedly summing history.
#[derive(Debug, Clone, Default)]
struct Sum {
    value: f64,
    correction: f64,
}
impl Aggregate<f64> for Sum {
    fn add(&mut self, value: &f64) {
        let y = value - self.correction;
        let next = self.value + y;
        self.correction = (next - self.value) - y;
        self.value = next;
    }
    fn retract(&mut self, value: &f64) {
        self.add(&-*value);
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DurationWork {
    pub documents_read: usize,
    pub stored_changes_read: usize,
    pub events_read: usize,
    pub events_recomputed: usize,
    pub contributions_changed: usize,
}
#[derive(Debug, Clone, Default)]
struct DocumentKeys {
    inputs: OrdSet<InputKey>,
    geometry: OrdSet<GeometryKey>,
    events: usize,
}
#[derive(Debug, Clone, Default)]
pub(crate) struct DurationIndex {
    initialized: bool,
    store_identity: Option<Vec<u8>>,
    store_cursor: i64,
    dirty_documents: OrdSet<PathBuf>,
    documents: OrdMap<PathBuf, DocumentKeys>,
    graph: DerivedGraph<InputKey, InputValue, EventKey, EventOutput>,
    contributions: ContributionIndex<EventKey, (AgendaItem, bool), f64, Sum>,
    geometry: OrdMap<GeometryKey, AgendaLocation>,
    issues: OrdMap<EventKey, Vec<GeometryKey>>,
    invalid: OrdSet<PathBuf>,
    pending: OrdSet<PathBuf>,
    pub work: DurationWork,
}
#[derive(Debug, Default)]
struct CacheRevision {
    state: Mutex<DurationIndex>,
    // Expensive evaluation is serialized, but never holds the state lock.
    // An editor can fork the previous completed state without waiting for work.
    writer: Mutex<()>,
}
#[derive(Debug, Default, Clone)]
pub(crate) struct DurationCache(Arc<CacheRevision>);
impl DurationCache {
    pub fn fork(&self) -> Self {
        Self(Arc::new(CacheRevision {
            state: Mutex::new(self.0.state.lock().expect("duration cache").clone()),
            writer: Mutex::new(()),
        }))
    }
    pub fn changed(&mut self, path: PathBuf) {
        if Arc::get_mut(&mut self.0).is_none() {
            *self = self.fork();
        }
        Arc::get_mut(&mut self.0)
            .expect("exclusive duration revision")
            .state
            .get_mut()
            .expect("duration cache")
            .dirty_documents
            .insert(path);
    }
}

struct Reader<'a, 'b> {
    reads: &'a mut TrackedRead<'b, InputKey, InputValue>,
    slots: BTreeMap<AgendaLocation, GeometryKey>,
}
impl Reader<'_, '_> {
    // The shared allocator carries locations. These private slots are symbolic,
    // immediately lowered to GeometryKey, and never exposed as source offsets.
    fn slot(&mut self, path: &Path, key: GeometryKey) -> Range<usize> {
        let index = self.slots.len();
        let range = index..index;
        self.slots.insert(location(path, range.clone()), key);
        range
    }
    fn category(&mut self, item: AgendaItem) -> Category {
        let Some(InputValue::Category(value)) = self.reads.get(InputKey::Category(item.clone()))
        else {
            return Category::default();
        };
        let value = value.clone();
        self.bind_category(value, GeometryKey::Category(item.clone()), &item.path)
    }
    fn bind_category(&mut self, value: CategoryValue, key: GeometryKey, path: &Path) -> Category {
        Category {
            values: value.values,
            invalid: value.invalid,
            declarations: if value.declared {
                vec![self.slot(path, key)]
            } else {
                vec![]
            },
        }
    }
}
impl AccountingReader for Reader<'_, '_> {
    fn document_category(&mut self, _: &Workspace, path: &Path) -> Result<Category, String> {
        Ok(self.category(AgendaItem {
            path: path.to_owned(),
            id: None,
        }))
    }
    fn target(
        &mut self,
        _: &Workspace,
        from: &Path,
        target: &TaskReferenceTarget,
    ) -> Result<AccountingTarget, String> {
        let mut result = AccountingTarget {
            valid: false,
            is_task: false,
            category: Category::default(),
            path: from.to_owned(),
        };
        let Some(item) = accounting_identity(from, target) else {
            return Ok(result);
        };
        result.path = item.path.clone();
        if let Some(InputValue::Identity { valid, task }) =
            self.reads.get(InputKey::Identity(item.clone()))
        {
            result.valid = *valid;
            result.is_task = *task;
        }
        if result.valid {
            result.category = self.category(item.clone());
            if result.category.declarations.is_empty() && item.id.is_some() {
                result.category = self.category(AgendaItem {
                    path: item.path,
                    id: None,
                });
            }
        }
        Ok(result)
    }
}

impl DurationIndex {
    fn compute(
        workspace: &Workspace,
        key: &EventKey,
        reads: &mut TrackedRead<'_, InputKey, InputValue>,
    ) -> Result<EventOutput, String> {
        let Some(InputValue::Event(value)) = reads.get(InputKey::Event(key.clone())) else {
            return Ok(EventOutput::default());
        };
        let value = value.clone();
        let duration = match value.duration {
            Some(DurationValue::Seconds(seconds)) => seconds,
            Some(DurationValue::Unavailable) => {
                return Ok(EventOutput {
                    issues: vec![GeometryKey::Event(key.clone())],
                    ..Default::default()
                })
            }
            _ => return Ok(EventOutput::default()),
        };
        let mut reader = Reader {
            reads,
            slots: BTreeMap::new(),
        };
        let range = reader.slot(&key.0, GeometryKey::Event(key.clone()));
        let category = reader.bind_category(
            value.category,
            GeometryKey::EventCategory(key.clone()),
            &key.0,
        );
        let references = value
            .references
            .into_iter()
            .enumerate()
            .map(|(i, (target, text))| {
                let range = reader.slot(&key.0, GeometryKey::Reference(key.clone(), i));
                (target, text, range)
            })
            .collect();
        let event = AccountingEvent {
            category,
            selection_range: range,
            tasks_override: value.tasks_override,
            references,
        };
        let (shares, issues) = workspace.allocate_event(&key.0, event, duration, &mut reader)?;
        let mut output = EventOutput::default();
        for share in shares {
            if let Some(item) = share.item {
                *output
                    .contributions
                    .entry((item, share.is_task))
                    .or_default() += share.seconds;
            }
        }
        output.issues = issues
            .into_iter()
            .map(|issue| {
                reader
                    .slots
                    .get(&issue.source)
                    .expect("allocator source slot")
                    .clone()
            })
            .collect();
        Ok(output)
    }

    fn update_document(&mut self, workspace: &Workspace, path: &Path) -> Result<(), String> {
        self.work.documents_read += 1;
        let old = self.documents.remove(path).unwrap_or_default();
        let mut next = DocumentKeys::default();
        let mut inputs = BTreeMap::new();
        let mut geometry = BTreeMap::new();
        let (exists, valid, pending, category, anchors, tasks, events) =
            if let Some(entry) = workspace.documents.get(path) {
                let output = entry.current.as_ref().map(|c| &c.output);
                (
                    true,
                    output.is_some(),
                    entry.parsed.is_valid() && output.is_none(),
                    output.map(|o| o.document_category()).unwrap_or_default(),
                    output
                        .map(|o| o.anchors().iter().collect::<Vec<_>>())
                        .unwrap_or_default(),
                    output
                        .map(|o| o.tasks().tasks.iter().collect::<Vec<_>>())
                        .unwrap_or_default(),
                    output
                        .map(|o| o.events().events.iter().collect::<Vec<_>>())
                        .unwrap_or_default(),
                )
            } else if let Some(store) = &workspace.disk_store {
                let doc = store.document(path).map_err(|e| e.to_string())?;
                let valid = doc.as_ref().is_some_and(|d| d.valid);
                (
                    doc.is_some(),
                    valid,
                    false,
                    if valid {
                        store
                            .document_category(path)
                            .map_err(|e| e.to_string())?
                            .unwrap_or_default()
                    } else {
                        Category::default()
                    },
                    if valid {
                        store.anchors_for_path(path).map_err(|e| e.to_string())?
                    } else {
                        vec![]
                    },
                    if valid {
                        store.tasks_for_path(path).map_err(|e| e.to_string())?
                    } else {
                        vec![]
                    },
                    if valid {
                        store.events_for_path(path).map_err(|e| e.to_string())?
                    } else {
                        vec![]
                    },
                )
            } else {
                (
                    false,
                    false,
                    false,
                    Category::default(),
                    vec![],
                    vec![],
                    vec![],
                )
            };
        self.work.events_read += events.len();
        self.invalid.remove(path);
        self.pending.remove(path);
        if pending {
            self.pending.insert(path.to_owned());
        } else if exists && !valid {
            self.invalid.insert(path.to_owned());
        }
        if exists {
            let root = AgendaItem {
                path: path.to_owned(),
                id: None,
            };
            let task_ids = tasks
                .iter()
                .filter_map(|t| {
                    if t.owner == TaskOwner::Document {
                        Some(None)
                    } else {
                        t.id.as_ref().map(|id| Some(id.value.clone()))
                    }
                })
                .collect::<BTreeSet<_>>();
            inputs.insert(
                InputKey::Identity(root.clone()),
                InputValue::Identity {
                    valid: true,
                    task: task_ids.contains(&None),
                },
            );
            inputs.insert(
                InputKey::Category(root.clone()),
                InputValue::Category((&category).into()),
            );
            if let Some(range) = category.declarations.first() {
                geometry.insert(GeometryKey::Category(root), location(path, range.clone()));
            }
            let mut groups = BTreeMap::<String, Vec<plumb_semantics::AnchorRecord>>::new();
            for anchor in anchors {
                groups
                    .entry(anchor.id.value.clone())
                    .or_default()
                    .push(anchor);
            }
            for (id, anchors) in groups {
                let item = AgendaItem {
                    path: path.to_owned(),
                    id: Some(id.clone()),
                };
                let anchor = &anchors[0];
                inputs.insert(
                    InputKey::Identity(item.clone()),
                    InputValue::Identity {
                        valid: anchors.len() == 1 && anchor.list_item,
                        task: anchors.len() == 1
                            && anchor.list_item
                            && task_ids.contains(&Some(id)),
                    },
                );
                inputs.insert(
                    InputKey::Category(item.clone()),
                    InputValue::Category((&anchor.category).into()),
                );
                if let Some(range) = anchor.category.declarations.first() {
                    geometry.insert(GeometryKey::Category(item), location(path, range.clone()));
                }
            }
            next.events = events.len();
            for (i, event) in events.into_iter().enumerate() {
                let key = (path.to_owned(), i);
                let refs = workspace.accounting_references(path, &event)?;
                geometry.insert(
                    GeometryKey::Event(key.clone()),
                    location(path, event.selection_range.clone()),
                );
                for (index, (_, _, range)) in refs.iter().enumerate() {
                    geometry.insert(
                        GeometryKey::Reference(key.clone(), index),
                        location(path, range.clone()),
                    );
                }
                if let Some(range) = event.category.declarations.first() {
                    geometry.insert(
                        GeometryKey::EventCategory(key.clone()),
                        location(path, range.clone()),
                    );
                }
                inputs.insert(
                    InputKey::Event(key.clone()),
                    InputValue::Event(EventValue {
                        duration: event_duration(&event),
                        category: (&event.category).into(),
                        tasks_override: event.tasks_override,
                        references: refs
                            .into_iter()
                            .map(|(target, text, _)| (target, text))
                            .collect(),
                    }),
                );
                if i >= old.events {
                    self.graph.schedule(key);
                }
            }
        }
        for key in &old.inputs {
            if !inputs.contains_key(key) {
                self.graph.set(key.clone(), None);
            }
        }
        for (key, value) in inputs {
            next.inputs.insert(key.clone());
            self.graph.set(key, Some(value));
        }
        for key in &old.geometry {
            self.geometry.remove(key);
        }
        for (key, value) in geometry {
            next.geometry.insert(key.clone());
            self.geometry.insert(key, value);
        }
        for i in next.events..old.events {
            let key = (path.to_owned(), i);
            self.graph.remove(&key);
            self.issues.remove(&key);
            self.work.contributions_changed += self.contributions.replace(key, BTreeMap::new());
        }
        if exists {
            self.documents.insert(path.to_owned(), next);
        }
        Ok(())
    }
    fn synchronize(&mut self, workspace: &Workspace) -> Result<(), String> {
        self.work = DurationWork::default();
        let mut changed = self.dirty_documents.clone();
        if let Some(store) = &workspace.disk_store {
            let (identity, cursor, paths) = store
                .generation_changes_since(self.store_cursor)
                .map_err(|e| e.to_string())?;
            if self
                .store_identity
                .as_ref()
                .is_some_and(|old| *old != identity)
                || cursor < self.store_cursor
            {
                *self = Self::default();
                changed = workspace.documents.keys().cloned().collect();
            }
            self.work.stored_changes_read = paths.len();
            if !self.initialized {
                for path in store.document_paths().map_err(|e| e.to_string())? {
                    changed.insert(path);
                }
            } else {
                for path in paths {
                    if !workspace.documents.contains_key(&path) {
                        changed.insert(path);
                    }
                }
            }
            self.store_identity = Some(identity);
            self.store_cursor = cursor;
        }
        if !self.initialized {
            for path in workspace.documents.keys() {
                changed.insert(path.clone());
            }
        }
        for path in changed {
            self.update_document(workspace, &path)?;
        }
        for key in self.graph.dirty() {
            self.work.events_recomputed += 1;
            if self
                .graph
                .evaluate(key.clone(), |reads| Self::compute(workspace, &key, reads))?
            {
                let output = self.graph.output(&key).expect("evaluated");
                self.work.contributions_changed += self
                    .contributions
                    .replace(key.clone(), output.contributions.clone());
                if output.issues.is_empty() {
                    self.issues.remove(&key);
                } else {
                    self.issues.insert(key, output.issues.clone());
                }
            }
        }
        self.dirty_documents.clear();
        self.initialized = true;
        Ok(())
    }
    pub fn complete(&self) -> bool {
        self.invalid.is_empty() && self.pending.is_empty() && self.issues.is_empty()
    }
    pub fn pending(&self) -> bool {
        !self.pending.is_empty()
    }
    pub fn total(&self, item: &AgendaItem, task_only: bool) -> Option<f64> {
        let task = self.contributions.get(&(item.clone(), true));
        let ordinary = (!task_only)
            .then(|| self.contributions.get(&(item.clone(), false)))
            .flatten();
        (task.is_some() || ordinary.is_some())
            .then(|| task.map_or(0.0, |s| s.value) + ordinary.map_or(0.0, |s| s.value))
    }
    pub fn sources(&self, item: &AgendaItem) -> Vec<AgendaLocation> {
        [true, false]
            .into_iter()
            .flat_map(|task| self.contributions.sources(&(item.clone(), task)))
            .map(|key| {
                self.geometry
                    .get(&GeometryKey::Event(key.clone()))
                    .expect("current event geometry")
                    .clone()
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    pub fn issue_sources(&self) -> Vec<AgendaLocation> {
        self.invalid
            .iter()
            .map(|path| location(path, 0..0))
            .chain(self.pending.iter().map(|path| location(path, 0..0)))
            .chain(self.issues.values().flatten().map(|key| {
                self.geometry
                    .get(key)
                    .expect("current issue geometry")
                    .clone()
            }))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    pub fn task_totals(&self) -> BTreeMap<AgendaItem, f64> {
        self.contributions
            .keys()
            .filter(|(_, task)| *task)
            .map(|(item, _)| (item.clone(), self.total(item, true).unwrap()))
            .collect()
    }
}
impl Workspace {
    pub(crate) fn with_duration_index<T>(
        &self,
        read: impl FnOnce(&DurationIndex) -> T,
    ) -> Result<T, String> {
        let _writer = self
            .derived
            .duration
            .0
            .writer
            .lock()
            .map_err(|_| "duration writer lock poisoned")?;
        let before = self.query_store_version().map_err(|e| e.to_string())?;
        let mut updated = self
            .derived
            .duration
            .0
            .state
            .lock()
            .map_err(|_| "duration cache lock poisoned")?
            .clone();
        updated.synchronize(self)?;
        if self.query_store_version().map_err(|e| e.to_string())? != before {
            return Err("semantic store changed during duration update; retry query".into());
        }
        let result = read(&updated);
        *self
            .derived
            .duration
            .0
            .state
            .lock()
            .map_err(|_| "duration cache lock poisoned")? = updated;
        Ok(result)
    }
    /// Deterministic work counters for the last successful duration synchronization.
    pub fn duration_work(&self) -> DurationWork {
        self.derived
            .duration
            .0
            .state
            .lock()
            .expect("duration cache")
            .work
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_synchronization_does_not_publish_cursor_or_half_updated_contributions() {
        let store = crate::SqliteSemanticStore::open_in_memory().unwrap();
        let mut w = Workspace::with_sqlite_store(store.clone());
        w.insert_disk("/t.plumb", 0, "`+ task\n").unwrap();
        let event = "`- 2026-10-01T10:00:00Z--11:00 `->{t.plumb}\n `+ event\n";
        w.insert_disk("/e.plumb", 0, event).unwrap();
        let before = w.task_duration_totals().unwrap();
        let cursor = w.derived.duration.0.state.lock().unwrap().store_cursor;
        w.insert_disk("/e.plumb", 1, event.replace("--11:00", "--12:00"))
            .unwrap();
        store
            .execute_batch_for_test("ALTER TABLE events RENAME TO unavailable_events;")
            .unwrap();
        assert!(w.task_duration_totals().is_err());
        assert_eq!(
            w.derived.duration.0.state.lock().unwrap().store_cursor,
            cursor
        );
        assert_eq!(
            w.derived.duration.0.state.lock().unwrap().task_totals(),
            before.seconds
        );
        store
            .execute_batch_for_test("ALTER TABLE unavailable_events RENAME TO events;")
            .unwrap();
        assert_eq!(
            w.task_duration_totals()
                .unwrap()
                .seconds
                .values()
                .copied()
                .collect::<Vec<_>>(),
            [7200.0]
        );
        assert_eq!(w.duration_work().events_recomputed, 1);
    }
}
