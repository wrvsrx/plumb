use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use plumb_core::{
    parse, Attributes, Block, Diagnostic, DiagnosticSeverity, ParsedBlock, ParsedDocument,
};
use plumb_extensions::{
    analyze_document, next_task_datetime, parse_task_reference_target, valid_task_datetime,
    AnchorRecord, DocumentOutput, LinkCompletionContext, LinkRecord, LinkTarget, TaskRecord,
    TaskReferenceTarget, TaskState, TaskStatus,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub range: std::ops::Range<usize>,
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentEdit {
    pub path: PathBuf,
    pub expected_revision: i64,
    pub edits: Vec<TextEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkspaceEdit {
    pub document_changes: Vec<DocumentEdit>,
    pub resource_operations: Vec<ResourceOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceOperation {
    Rename {
        old_path: PathBuf,
        new_path: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameTarget {
    pub path: PathBuf,
    pub id: String,
    pub range: std::ops::Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameError {
    NotRenameable,
    InvalidId,
    StaleOrInvalidDocument,
    OverlappingEdits,
    InvalidPath,
    TargetExists,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataInsertError {
    StaleOrInvalidDocument,
    MetadataAlreadyExists,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExplicitIdError {
    StaleOrInvalidDocument,
    BlockNotFound,
    IdAlreadyExists,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskEditError {
    StaleOrInvalidDocument,
    TaskNotFound,
    TaskAlreadyClosed,
    TaskBlocked,
    InvalidRecurrence,
    InvalidTimestamp,
    ListItemNotFound,
    TaskAlreadyExists,
    CreatedAlreadyExists,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathRenameTarget {
    pub old_path: PathBuf,
    pub range: std::ops::Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionCandidate {
    pub label: String,
    pub detail: String,
    pub new_text: String,
    pub replace: std::ops::Range<usize>,
}

#[derive(Debug, Clone)]
pub struct VersionedDocumentOutput {
    pub revision: i64,
    pub output: DocumentOutput,
}

#[derive(Debug, Clone)]
pub struct DocumentEntry {
    pub path: PathBuf,
    pub revision: i64,
    pub parsed: ParsedDocument,
    pub current: Option<VersionedDocumentOutput>,
    pub last_valid: Option<VersionedDocumentOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedTarget {
    Anchor {
        path: PathBuf,
        id: String,
        anchor: AnchorRecord,
    },
    Document {
        path: PathBuf,
    },
    External,
    Other,
    UnresolvedPath {
        path: PathBuf,
    },
    UnresolvedAnchor {
        path: PathBuf,
        id: String,
    },
    AmbiguousAnchor {
        path: PathBuf,
        id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorReference {
    pub source_range: std::ops::Range<usize>,
    pub path_range: Option<std::ops::Range<usize>>,
    pub id_range: std::ops::Range<usize>,
    pub target_path: PathBuf,
    pub target_id: String,
    pub anchor: AnchorRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskRef {
    pub path: PathBuf,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTaskDependency {
    pub source: String,
    pub target: TaskRef,
    pub task: TaskRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TaskTargetResolution {
    Task { target: TaskRef, task: TaskRecord },
    Invalid,
    UnresolvedPath { path: PathBuf },
    UnresolvedAnchor { path: PathBuf, id: String },
    AmbiguousAnchor { path: PathBuf, id: String },
    NotTask { path: PathBuf, id: String },
}

#[derive(Debug, Default)]
pub struct Workspace {
    documents: HashMap<PathBuf, DocumentEntry>,
}

impl Workspace {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(
        &mut self,
        path: impl AsRef<Path>,
        revision: i64,
        source: impl Into<String>,
    ) -> &DocumentEntry {
        let path = normalize(path.as_ref());
        let parsed = parse(source);
        let previous_last_valid = self
            .documents
            .get(&path)
            .and_then(|entry| entry.last_valid.clone());
        let current = parsed.is_valid().then(|| VersionedDocumentOutput {
            revision,
            output: analyze_document(&parsed.source, &parsed.syntax),
        });
        let last_valid = current.clone().or(previous_last_valid);
        self.documents.insert(
            path.clone(),
            DocumentEntry {
                path: path.clone(),
                revision,
                parsed,
                current,
                last_valid,
            },
        );
        self.documents.get(&path).expect("just inserted")
    }

    pub fn remove(&mut self, path: impl AsRef<Path>) -> Option<DocumentEntry> {
        self.documents.remove(&normalize(path.as_ref()))
    }

    pub fn get(&self, path: impl AsRef<Path>) -> Option<&DocumentEntry> {
        self.documents.get(&normalize(path.as_ref()))
    }

    pub fn contains(&self, path: impl AsRef<Path>) -> bool {
        self.documents.contains_key(&normalize(path.as_ref()))
    }

    pub fn documents(&self) -> impl Iterator<Item = &DocumentEntry> {
        self.documents.values()
    }

    pub fn resolve_link(&self, from: impl AsRef<Path>, link: &LinkRecord) -> ResolvedTarget {
        let from = normalize(from.as_ref());
        match &link.target_kind {
            LinkTarget::External => ResolvedTarget::External,
            LinkTarget::Other => ResolvedTarget::Other,
            LinkTarget::Document { path } => {
                let target = resolve_relative(&from, path);
                if self.current_output(&target).is_some() {
                    ResolvedTarget::Document { path: target }
                } else {
                    ResolvedTarget::UnresolvedPath { path: target }
                }
            }
            LinkTarget::Anchor { path, fragment } => {
                let target = path
                    .as_deref()
                    .map_or_else(|| from.clone(), |path| resolve_relative(&from, path));
                let Some(output) = self.current_output(&target) else {
                    return ResolvedTarget::UnresolvedPath { path: target };
                };
                let mut anchors = output
                    .anchors
                    .iter()
                    .filter(|anchor| anchor.id.value == *fragment);
                let Some(anchor) = anchors.next() else {
                    return ResolvedTarget::UnresolvedAnchor {
                        path: target,
                        id: fragment.clone(),
                    };
                };
                if anchors.next().is_some() {
                    return ResolvedTarget::AmbiguousAnchor {
                        path: target,
                        id: fragment.clone(),
                    };
                }
                ResolvedTarget::Anchor {
                    path: target,
                    id: fragment.clone(),
                    anchor: anchor.clone(),
                }
            }
        }
    }

    pub fn link_at(&self, path: impl AsRef<Path>, offset: usize) -> Option<&LinkRecord> {
        self.current_output(path.as_ref())?
            .links
            .iter()
            .filter(|link| link.range.start <= offset && offset <= link.range.end)
            .max_by_key(|link| link.range.start)
    }

    pub fn anchor_at(&self, path: impl AsRef<Path>, offset: usize) -> Option<&AnchorRecord> {
        self.current_output(path.as_ref())?
            .anchors
            .iter()
            .filter(|anchor| anchor.range.start <= offset && offset <= anchor.range.end)
            .max_by_key(|anchor| anchor.range.start)
    }

    pub fn anchor_reference_at(
        &self,
        path: impl AsRef<Path>,
        offset: usize,
    ) -> Option<AnchorReference> {
        let path = normalize(path.as_ref());
        let output = self.current_output(&path)?;
        if let Some(link) = output
            .links
            .iter()
            .filter(|link| contains_inclusive(&link.range, offset))
            .max_by_key(|link| link.range.start)
        {
            return self.link_anchor_reference(&path, link);
        }
        for task in &output.tasks.tasks {
            if let Some(prev) = &task.prev {
                if contains_inclusive(&prev.range, offset) {
                    let target = parse_task_reference_target(&prev.value);
                    return self.task_anchor_reference(&path, &prev.value, &prev.range, &target);
                }
            }
            if let Some(dependency) = task
                .depends
                .iter()
                .find(|dependency| contains_inclusive(&dependency.range, offset))
            {
                return self.task_anchor_reference(
                    &path,
                    &dependency.source,
                    &dependency.range,
                    &dependency.target,
                );
            }
        }
        None
    }

    pub fn resolve_task_reference_at(
        &self,
        path: impl AsRef<Path>,
        offset: usize,
    ) -> Option<ResolvedTarget> {
        let path = normalize(path.as_ref());
        let output = self.current_output(&path)?;
        for task in &output.tasks.tasks {
            if let Some(prev) = &task.prev {
                if contains_inclusive(&prev.range, offset) {
                    return Some(self.resolve_task_reference_target(
                        &path,
                        &parse_task_reference_target(&prev.value),
                    ));
                }
            }
            if let Some(dependency) = task
                .depends
                .iter()
                .find(|dependency| contains_inclusive(&dependency.range, offset))
            {
                return Some(self.resolve_task_reference_target(&path, &dependency.target));
            }
        }
        None
    }

    pub fn references_to(
        &self,
        target_path: impl AsRef<Path>,
        target_id: &str,
    ) -> Vec<(&Path, AnchorReference)> {
        let target_path = normalize(target_path.as_ref());
        let mut references = Vec::new();
        for entry in self.documents.values() {
            let Some(current) = &entry.current else {
                continue;
            };
            for link in &current.output.links {
                if let Some(reference) = self.link_anchor_reference(&entry.path, link) {
                    if reference.target_path == target_path && reference.target_id == target_id {
                        references.push((entry.path.as_path(), reference));
                    }
                }
            }
            for task in &current.output.tasks.tasks {
                for (source, range, target) in task_reference_fields(task) {
                    if let Some(reference) =
                        self.task_anchor_reference(&entry.path, source, range, &target)
                    {
                        if reference.target_path == target_path && reference.target_id == target_id
                        {
                            references.push((entry.path.as_path(), reference));
                        }
                    }
                }
            }
        }
        references.sort_by(|left, right| {
            left.0
                .cmp(right.0)
                .then(left.1.source_range.start.cmp(&right.1.source_range.start))
        });
        references
    }

    pub fn referenced_documents_from(&self, source_path: impl AsRef<Path>) -> Vec<PathBuf> {
        let source_path = normalize(source_path.as_ref());
        let Some(output) = self.current_output(&source_path) else {
            return Vec::new();
        };
        let mut targets = HashSet::new();
        for link in &output.links {
            if let Some(path) = resolved_document_path(self.resolve_link(&source_path, link)) {
                targets.insert(path);
            }
        }
        for task in &output.tasks.tasks {
            for (_, _, target) in task_reference_fields(task) {
                if let Some(path) = resolved_document_path(
                    self.resolve_task_reference_target(&source_path, &target),
                ) {
                    targets.insert(path);
                }
            }
        }
        let mut targets = targets.into_iter().collect::<Vec<_>>();
        targets.sort();
        targets
    }

    fn link_anchor_reference(&self, from: &Path, link: &LinkRecord) -> Option<AnchorReference> {
        let ResolvedTarget::Anchor { path, id, anchor } = self.resolve_link(from, link) else {
            return None;
        };
        Some(AnchorReference {
            source_range: link.selection_range.clone(),
            path_range: link.path_range.clone(),
            id_range: link.fragment_range.clone()?,
            target_path: path,
            target_id: id,
            anchor,
        })
    }

    fn task_anchor_reference(
        &self,
        from: &Path,
        source: &str,
        range: &std::ops::Range<usize>,
        target: &TaskReferenceTarget,
    ) -> Option<AnchorReference> {
        let (target_path, target_id, anchor) = self.resolve_task_anchor(from, target)?;
        let (path_range, id_range) = task_reference_ranges(source, range, target_id.as_str())?;
        Some(AnchorReference {
            source_range: range.clone(),
            path_range,
            id_range,
            target_path,
            target_id,
            anchor,
        })
    }

    fn resolve_task_anchor(
        &self,
        from: &Path,
        target: &TaskReferenceTarget,
    ) -> Option<(PathBuf, String, AnchorRecord)> {
        let ResolvedTarget::Anchor { path, id, anchor } =
            self.resolve_task_reference_target(from, target)
        else {
            return None;
        };
        Some((path, id, anchor))
    }

    fn resolve_task_reference_target(
        &self,
        from: &Path,
        target: &TaskReferenceTarget,
    ) -> ResolvedTarget {
        let (path, id) = match target {
            TaskReferenceTarget::Internal { id } => (normalize(from), id.clone()),
            TaskReferenceTarget::External { path, id } => (
                resolve_relative(from, &percent_decode_path(path)),
                id.clone(),
            ),
            TaskReferenceTarget::Invalid => return ResolvedTarget::Other,
        };
        let Some(output) = self.current_output(&path) else {
            return ResolvedTarget::UnresolvedPath { path };
        };
        let mut anchors = output.anchors.iter().filter(|anchor| anchor.id.value == id);
        let Some(anchor) = anchors.next().cloned() else {
            return ResolvedTarget::UnresolvedAnchor { path, id };
        };
        if anchors.next().is_some() {
            return ResolvedTarget::AmbiguousAnchor { path, id };
        }
        ResolvedTarget::Anchor { path, id, anchor }
    }

    pub fn task_at(&self, path: impl AsRef<Path>, offset: usize) -> Option<&TaskRecord> {
        self.current_output(path.as_ref())?
            .tasks
            .tasks
            .iter()
            .filter(|task| task.range.start <= offset && offset <= task.range.end)
            .max_by_key(|task| task.range.start)
    }

    pub fn open_task_dependencies(
        &self,
        path: impl AsRef<Path>,
        task: &TaskRecord,
    ) -> Vec<ResolvedTaskDependency> {
        let path = normalize(path.as_ref());
        self.task_dependencies(path, task)
            .into_iter()
            .filter(|dependency| dependency.task.state() == TaskState::Open)
            .collect()
    }

    pub fn task_dependencies(
        &self,
        path: impl AsRef<Path>,
        task: &TaskRecord,
    ) -> Vec<ResolvedTaskDependency> {
        let path = normalize(path.as_ref());
        let mut dependencies = task
            .depends
            .iter()
            .filter_map(|dependency| {
                let TaskTargetResolution::Task {
                    target,
                    task: target_task,
                } = self.resolve_task_target(&path, &dependency.target)
                else {
                    return None;
                };
                Some(ResolvedTaskDependency {
                    source: dependency.source.clone(),
                    target,
                    task: target_task,
                })
            })
            .collect::<Vec<_>>();
        dependencies.sort_by(|left, right| {
            left.target
                .path
                .cmp(&right.target.path)
                .then(left.target.id.cmp(&right.target.id))
        });
        dependencies
    }

    pub fn directly_blocking_tasks(
        &self,
        target_path: impl AsRef<Path>,
        target_id: &str,
    ) -> Vec<TaskRef> {
        let target = TaskRef {
            path: normalize(target_path.as_ref()),
            id: target_id.to_string(),
        };
        let mut blocking = Vec::new();
        for entry in self.documents.values() {
            let Some(current) = &entry.current else {
                continue;
            };
            for task in &current.output.tasks.tasks {
                let Some(id) = &task.id else {
                    continue;
                };
                if self
                    .task_dependencies(&entry.path, task)
                    .iter()
                    .any(|dependency| dependency.target == target)
                {
                    blocking.push(TaskRef {
                        path: entry.path.clone(),
                        id: id.value.clone(),
                    });
                }
            }
        }
        blocking.sort_by(|left, right| left.path.cmp(&right.path).then(left.id.cmp(&right.id)));
        blocking
    }

    pub fn is_task_blocked(&self, path: impl AsRef<Path>, task: &TaskRecord) -> bool {
        !self.open_task_dependencies(path, task).is_empty()
    }

    pub fn set_task_status(
        &self,
        path: impl AsRef<Path>,
        offset: usize,
        status: TaskStatus,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, TaskEditError> {
        if !valid_task_datetime(timestamp) {
            return Err(TaskEditError::InvalidTimestamp);
        }
        let path = normalize(path.as_ref());
        let entry = self
            .documents
            .get(&path)
            .filter(|entry| entry.current.is_some())
            .ok_or(TaskEditError::StaleOrInvalidDocument)?;
        let tasks = &entry
            .current
            .as_ref()
            .expect("current output checked")
            .output
            .tasks
            .tasks;
        let task = tasks
            .iter()
            .filter(|task| {
                task.state() == TaskState::Open
                    && task.range.start <= offset
                    && offset <= task.range.end
            })
            .max_by_key(|task| task.range.start)
            .ok_or_else(|| {
                if tasks
                    .iter()
                    .any(|task| task.range.start <= offset && offset <= task.range.end)
                {
                    TaskEditError::TaskAlreadyClosed
                } else {
                    TaskEditError::TaskNotFound
                }
            })?;
        self.task_status_edit(entry, &path, task, status, timestamp)
    }

    pub fn convert_list_item_to_task(
        &self,
        path: impl AsRef<Path>,
        offset: usize,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, TaskEditError> {
        if !valid_task_datetime(timestamp) {
            return Err(TaskEditError::InvalidTimestamp);
        }
        let path = normalize(path.as_ref());
        let entry = self
            .documents
            .get(&path)
            .filter(|entry| entry.current.is_some())
            .ok_or(TaskEditError::StaleOrInvalidDocument)?;
        let item = deepest_list_item(&entry.parsed.syntax.blocks, offset)
            .ok_or(TaskEditError::ListItemNotFound)?;
        let mark = item.mark.as_ref().expect("list item has a mark");
        if mark.attrs.has_class("task") {
            return Err(TaskEditError::TaskAlreadyExists);
        }
        let (range, new_text) = match &mark.attrs.range {
            Some(range) => (
                range.end.saturating_sub(1)..range.end.saturating_sub(1),
                format!(" .task created=\"{timestamp}\""),
            ),
            None => (
                mark.marker_range.end..mark.marker_range.end,
                format!("{{.task created=\"{timestamp}\"}}"),
            ),
        };
        Ok(single_document_edit(
            entry,
            path,
            TextEdit { range, new_text },
        ))
    }

    pub fn add_task_created(
        &self,
        path: impl AsRef<Path>,
        offset: usize,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, TaskEditError> {
        if !valid_task_datetime(timestamp) {
            return Err(TaskEditError::InvalidTimestamp);
        }
        let path = normalize(path.as_ref());
        let entry = self
            .documents
            .get(&path)
            .filter(|entry| entry.current.is_some())
            .ok_or(TaskEditError::StaleOrInvalidDocument)?;
        let task = entry
            .current
            .as_ref()
            .expect("current output checked")
            .output
            .tasks
            .tasks
            .iter()
            .filter(|task| task.range.start <= offset && offset <= task.range.end)
            .max_by_key(|task| task.range.start)
            .ok_or(TaskEditError::TaskNotFound)?;
        if task.created.is_some() {
            return Err(TaskEditError::CreatedAlreadyExists);
        }
        Ok(single_document_edit(
            entry,
            path,
            TextEdit {
                range: task.attribute_insert..task.attribute_insert,
                new_text: format!(" created=\"{timestamp}\""),
            },
        ))
    }

    pub fn add_explicit_id(
        &self,
        path: impl AsRef<Path>,
        offset: usize,
    ) -> Result<WorkspaceEdit, ExplicitIdError> {
        let path = normalize(path.as_ref());
        let entry = self
            .documents
            .get(&path)
            .filter(|entry| entry.current.is_some())
            .ok_or(ExplicitIdError::StaleOrInvalidDocument)?;
        let target = deepest_block_id_target(&entry.parsed.syntax.blocks, offset)
            .ok_or(ExplicitIdError::BlockNotFound)?;
        if target.attrs.id().is_some() {
            return Err(ExplicitIdError::IdAlreadyExists);
        }

        let reserved = entry
            .current
            .as_ref()
            .expect("current output checked")
            .output
            .anchors
            .iter()
            .map(|anchor| anchor.id.value.clone())
            .collect::<HashSet<_>>();
        let id = unique_anchor_id(&target.seed, &reserved);
        let edit = if let Some(range) = &target.attrs.range {
            let insert = range.start + 1;
            let separator = entry.parsed.source[insert..]
                .chars()
                .next()
                .is_some_and(|character| !character.is_whitespace() && character != '}')
                .then_some(" ")
                .unwrap_or("");
            TextEdit {
                range: insert..insert,
                new_text: format!("#{id}{separator}"),
            }
        } else {
            TextEdit {
                range: target.attribute_insert..target.attribute_insert,
                new_text: format!("{{#{id}}}"),
            }
        };
        Ok(single_document_edit(entry, path, edit))
    }

    fn task_status_edit(
        &self,
        entry: &DocumentEntry,
        path: &Path,
        task: &TaskRecord,
        status: TaskStatus,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, TaskEditError> {
        if task.state() != TaskState::Open {
            return Err(TaskEditError::TaskAlreadyClosed);
        }
        if task.recur.is_some() && task.due.is_some() {
            if status == TaskStatus::Done && self.is_task_blocked(&path, task) {
                return Err(TaskEditError::TaskBlocked);
            }
            return self.recurring_task_status_edit(entry, task, status, timestamp);
        }
        if status == TaskStatus::Done && self.is_task_blocked(&path, task) {
            return Err(TaskEditError::TaskBlocked);
        }
        Ok(WorkspaceEdit {
            document_changes: vec![DocumentEdit {
                path: path.to_path_buf(),
                expected_revision: entry.revision,
                edits: vec![TextEdit {
                    range: task.attribute_insert..task.attribute_insert,
                    new_text: format!(" {}=\"{}\"", status.attribute(), timestamp),
                }],
            }],
            resource_operations: Vec::new(),
        })
    }

    pub fn set_task_status_by_id(
        &self,
        path: impl AsRef<Path>,
        id: &str,
        status: TaskStatus,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, TaskEditError> {
        if !valid_task_datetime(timestamp) {
            return Err(TaskEditError::InvalidTimestamp);
        }
        let path = normalize(path.as_ref());
        let entry = self
            .documents
            .get(&path)
            .filter(|entry| entry.current.is_some())
            .ok_or(TaskEditError::StaleOrInvalidDocument)?;
        let task = entry
            .current
            .as_ref()
            .expect("current output checked")
            .output
            .tasks
            .tasks
            .iter()
            .find(|task| task.id.as_ref().is_some_and(|task_id| task_id.value == id))
            .ok_or(TaskEditError::TaskNotFound)?;
        self.task_status_edit(entry, &path, task, status, timestamp)
    }

    fn recurring_task_status_edit(
        &self,
        entry: &DocumentEntry,
        task: &TaskRecord,
        status: TaskStatus,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, TaskEditError> {
        let recur = task
            .recur
            .as_ref()
            .ok_or(TaskEditError::InvalidRecurrence)?;
        let due = task.due.as_ref().ok_or(TaskEditError::InvalidRecurrence)?;
        let next_due =
            next_task_datetime(&due.value, &recur.value).ok_or(TaskEditError::InvalidRecurrence)?;
        let next_wait = match &task.wait {
            Some(wait) => Some(
                next_task_datetime(&wait.value, &recur.value)
                    .ok_or(TaskEditError::InvalidRecurrence)?,
            ),
            None => None,
        };
        let current = entry
            .current
            .as_ref()
            .ok_or(TaskEditError::StaleOrInvalidDocument)?;
        let mut reserved = current
            .output
            .anchors
            .iter()
            .map(|anchor| anchor.id.value.clone())
            .collect::<HashSet<_>>();
        let current_id = task
            .id
            .as_ref()
            .map(|id| id.value.clone())
            .unwrap_or_else(|| {
                let id = unique_task_instance_id(&task.title, &due.value, &reserved);
                reserved.insert(id.clone());
                id
            });
        let next_id = unique_task_instance_id(&task.title, &next_due, &reserved);

        let source = &entry.parsed.source;
        let mut clone = source[task.range.clone()].to_string();
        let mut replacements = current
            .output
            .tasks
            .tasks
            .iter()
            .filter(|candidate| {
                task.range.start <= candidate.range.start && candidate.range.end <= task.range.end
            })
            .map(|candidate| {
                let replacement = if candidate.range == task.range {
                    recurring_task_attributes(
                        task,
                        &next_id,
                        timestamp,
                        &next_due,
                        next_wait.as_deref(),
                        &recur.value,
                        &current_id,
                    )
                } else {
                    attribute_slot(&candidate.persistent_attributes)
                };
                (
                    candidate.attribute_range.start - task.range.start
                        ..candidate.attribute_range.end - task.range.start,
                    replacement,
                )
            })
            .collect::<Vec<_>>();
        replacements.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
        for (range, replacement) in replacements {
            clone.replace_range(range, &replacement);
        }

        let newline = if source.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let line_start = source[..task.range.start]
            .rfind('\n')
            .map_or(0, |offset| offset + 1);
        let indent = &source[line_start..task.range.start];
        let before = &source[..task.range.end];
        let separator = if before.ends_with(&format!("{newline}{newline}")) {
            ""
        } else if before.ends_with(newline) {
            newline
        } else {
            // A parser-valid task may be the final line without a newline.
            if newline == "\r\n" {
                "\r\n\r\n"
            } else {
                "\n\n"
            }
        };
        let current_id_edit = task
            .id
            .is_none()
            .then(|| format!(" #{current_id}"))
            .unwrap_or_default();
        Ok(WorkspaceEdit {
            document_changes: vec![DocumentEdit {
                path: entry.path.clone(),
                expected_revision: entry.revision,
                edits: vec![
                    TextEdit {
                        range: task.attribute_insert..task.attribute_insert,
                        new_text: format!(
                            "{current_id_edit} {}=\"{}\"",
                            status.attribute(),
                            timestamp
                        ),
                    },
                    TextEdit {
                        range: task.range.end..task.range.end,
                        new_text: format!("{separator}{indent}{clone}"),
                    },
                ],
            }],
            resource_operations: Vec::new(),
        })
    }

    fn resolve_task_target(
        &self,
        from: &Path,
        target: &TaskReferenceTarget,
    ) -> TaskTargetResolution {
        let (path, id) = match target {
            TaskReferenceTarget::Internal { id } => (normalize(from), id.clone()),
            TaskReferenceTarget::External { path, id } => (
                resolve_relative(from, &percent_decode_path(path)),
                id.clone(),
            ),
            TaskReferenceTarget::Invalid => return TaskTargetResolution::Invalid,
        };
        let Some(output) = self.current_output(&path) else {
            return TaskTargetResolution::UnresolvedPath { path };
        };
        let matching_anchors = output
            .anchors
            .iter()
            .filter(|anchor| anchor.id.value == id)
            .count();
        if matching_anchors == 0 {
            return TaskTargetResolution::UnresolvedAnchor { path, id };
        }
        if matching_anchors > 1 {
            return TaskTargetResolution::AmbiguousAnchor { path, id };
        }
        let Some(task) = output
            .tasks
            .tasks
            .iter()
            .find(|task| task.id.as_ref().is_some_and(|task_id| task_id.value == id))
        else {
            return TaskTargetResolution::NotTask { path, id };
        };
        TaskTargetResolution::Task {
            target: TaskRef { path, id },
            task: task.clone(),
        }
    }

    fn task_dependency_graph(&self) -> HashMap<TaskRef, Vec<TaskRef>> {
        let mut graph = HashMap::new();
        for entry in self.documents.values() {
            let Some(current) = &entry.current else {
                continue;
            };
            for task in &current.output.tasks.tasks {
                let Some(id) = &task.id else {
                    continue;
                };
                let task_ref = TaskRef {
                    path: entry.path.clone(),
                    id: id.value.clone(),
                };
                let dependencies = task
                    .depends
                    .iter()
                    .filter_map(|dependency| {
                        let TaskTargetResolution::Task { target, .. } =
                            self.resolve_task_target(&entry.path, &dependency.target)
                        else {
                            return None;
                        };
                        Some(target)
                    })
                    .collect();
                graph.insert(task_ref, dependencies);
            }
        }
        graph
    }

    pub fn diagnostics(&self, path: impl AsRef<Path>) -> Vec<Diagnostic> {
        let path = normalize(path.as_ref());
        let Some(entry) = self.documents.get(&path) else {
            return Vec::new();
        };
        let mut diagnostics = entry.parsed.diagnostics.clone();
        let Some(current) = &entry.current else {
            return diagnostics;
        };
        diagnostics.extend(current.output.headings.diagnostics.clone());
        diagnostics.extend(current.output.metadata.diagnostics.clone());
        diagnostics.extend(current.output.citations.diagnostics.clone());
        diagnostics.extend(current.output.math.diagnostics.clone());
        diagnostics.extend(current.output.tasks.diagnostics.clone());
        diagnostics.extend(current.output.diagnostics.clone());
        for link in &current.output.links {
            let (code, message) = match self.resolve_link(&path, link) {
                ResolvedTarget::UnresolvedPath { path } => (
                    "link.unresolved-path",
                    format!("unresolved plumb document '{}'", path.display()),
                ),
                ResolvedTarget::UnresolvedAnchor { id, .. } => (
                    "link.unresolved-anchor",
                    format!("unresolved explicit anchor '#{id}'"),
                ),
                ResolvedTarget::AmbiguousAnchor { id, .. } => (
                    "link.ambiguous-anchor",
                    format!("explicit anchor '#{id}' is ambiguous"),
                ),
                _ => continue,
            };
            diagnostics.push(Diagnostic {
                code,
                severity: DiagnosticSeverity::Warning,
                message,
                range: link.target.range.clone(),
                related: Vec::new(),
            });
        }
        diagnostics.extend(self.task_workspace_diagnostics(&path, current));
        diagnostics
    }

    fn task_workspace_diagnostics(
        &self,
        path: &Path,
        current: &VersionedDocumentOutput,
    ) -> Vec<Diagnostic> {
        let graph = self.task_dependency_graph();
        let mut diagnostics = Vec::new();
        for task in &current.output.tasks.tasks {
            let own_ref = task.id.as_ref().map(|id| TaskRef {
                path: path.to_path_buf(),
                id: id.value.clone(),
            });
            if let Some(prev) = &task.prev {
                let target = parse_task_reference_target(&prev.value);
                if let Some(diagnostic) =
                    self.task_target_diagnostic(path, &prev.value, &prev.range, &target, "prev")
                {
                    diagnostics.push(diagnostic);
                }
            }
            for dependency in &task.depends {
                if let Some(diagnostic) = self.task_target_diagnostic(
                    path,
                    &dependency.source,
                    &dependency.range,
                    &dependency.target,
                    "dependency",
                ) {
                    diagnostics.push(diagnostic);
                    continue;
                }
                if let TaskTargetResolution::Task { target, .. } =
                    self.resolve_task_target(path, &dependency.target)
                {
                    if own_ref.as_ref() == Some(&target) {
                        diagnostics.push(Diagnostic {
                            code: "task.self-dependency",
                            severity: DiagnosticSeverity::Warning,
                            message: format!(
                                "task depends on itself through '{}'",
                                dependency.source
                            ),
                            range: dependency.range.clone(),
                            related: Vec::new(),
                        });
                    }
                }
            }
            if let Some(task_ref) = &own_ref {
                if dependency_cycle_contains(&graph, task_ref) {
                    diagnostics.push(Diagnostic {
                        code: "task.dependency-cycle",
                        severity: DiagnosticSeverity::Warning,
                        message: format!(
                            "task '#{}' participates in a dependency cycle",
                            task_ref.id
                        ),
                        range: task.selection_range.clone(),
                        related: Vec::new(),
                    });
                }
            }
            if task.state() == TaskState::Open {
                let blockers = self.open_task_dependencies(path, task);
                if !blockers.is_empty() {
                    diagnostics.push(Diagnostic {
                        code: "task.blocked",
                        severity: DiagnosticSeverity::Hint,
                        message: format!(
                            "task is blocked by {} open {}",
                            blockers.len(),
                            if blockers.len() == 1 {
                                "dependency"
                            } else {
                                "dependencies"
                            }
                        ),
                        range: task.selection_range.clone(),
                        related: Vec::new(),
                    });
                }
            }
        }
        diagnostics
    }

    fn task_target_diagnostic(
        &self,
        from: &Path,
        source: &str,
        range: &std::ops::Range<usize>,
        target: &TaskReferenceTarget,
        role: &str,
    ) -> Option<Diagnostic> {
        let (code, message) = match self.resolve_task_target(from, target) {
            TaskTargetResolution::Task { .. } => return None,
            TaskTargetResolution::Invalid => (
                "task.invalid-target",
                format!("invalid task {role} target '{source}'"),
            ),
            TaskTargetResolution::UnresolvedPath { path } => (
                "task.unresolved-path",
                format!("unresolved task document '{}'", path.display()),
            ),
            TaskTargetResolution::UnresolvedAnchor { id, .. } => (
                "task.unresolved-anchor",
                format!("unresolved task anchor '#{id}'"),
            ),
            TaskTargetResolution::AmbiguousAnchor { id, .. } => (
                "task.ambiguous-anchor",
                format!("task anchor '#{id}' is ambiguous"),
            ),
            TaskTargetResolution::NotTask { id, .. } => (
                "task.non-task-target",
                format!("anchor '#{id}' does not identify a task"),
            ),
        };
        Some(Diagnostic {
            code,
            severity: DiagnosticSeverity::Warning,
            message,
            range: range.clone(),
            related: Vec::new(),
        })
    }

    pub fn anchor_rename_target_at(
        &self,
        path: impl AsRef<Path>,
        offset: usize,
    ) -> Result<RenameTarget, RenameError> {
        let path = normalize(path.as_ref());
        let output = self
            .current_output(&path)
            .ok_or(RenameError::StaleOrInvalidDocument)?;
        if let Some(anchor) = output
            .anchors
            .iter()
            .find(|anchor| contains_inclusive(&anchor.id.range, offset))
        {
            return Ok(RenameTarget {
                path,
                id: anchor.id.value.clone(),
                range: anchor.id.range.clone(),
            });
        }
        let reference = self
            .anchor_reference_at(&path, offset)
            .filter(|reference| contains_inclusive(&reference.id_range, offset))
            .ok_or(RenameError::NotRenameable)?;
        Ok(RenameTarget {
            path: reference.target_path,
            id: reference.target_id,
            range: reference.id_range,
        })
    }

    pub fn rename_anchor(
        &self,
        target: &RenameTarget,
        replacement: &str,
    ) -> Result<WorkspaceEdit, RenameError> {
        if !valid_anchor_id(replacement) {
            return Err(RenameError::InvalidId);
        }
        let entry = self
            .documents
            .get(&target.path)
            .filter(|entry| entry.current.is_some())
            .ok_or(RenameError::StaleOrInvalidDocument)?;
        let anchor = entry
            .current
            .as_ref()
            .and_then(|current| {
                current
                    .output
                    .anchors
                    .iter()
                    .find(|anchor| anchor.id.value == target.id)
            })
            .ok_or(RenameError::NotRenameable)?;
        let mut grouped: HashMap<PathBuf, Vec<TextEdit>> = HashMap::new();
        grouped
            .entry(target.path.clone())
            .or_default()
            .push(TextEdit {
                range: anchor.id.range.clone(),
                new_text: replacement.to_string(),
            });
        for (path, reference) in self.references_to(&target.path, &target.id) {
            grouped
                .entry(path.to_path_buf())
                .or_default()
                .push(TextEdit {
                    range: reference.id_range,
                    new_text: replacement.to_string(),
                });
        }
        let mut document_changes = Vec::new();
        for (path, mut edits) in grouped {
            edits.sort_by_key(|edit| edit.range.start);
            if edits
                .windows(2)
                .any(|pair| pair[0].range.end > pair[1].range.start)
            {
                return Err(RenameError::OverlappingEdits);
            }
            let expected_revision = self
                .documents
                .get(&path)
                .ok_or(RenameError::StaleOrInvalidDocument)?
                .revision;
            document_changes.push(DocumentEdit {
                path,
                expected_revision,
                edits,
            });
        }
        document_changes.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(WorkspaceEdit {
            document_changes,
            resource_operations: Vec::new(),
        })
    }

    pub fn path_rename_target_at(
        &self,
        path: impl AsRef<Path>,
        offset: usize,
    ) -> Result<PathRenameTarget, RenameError> {
        let path = normalize(path.as_ref());
        if let Some(link) = self.current_output(&path).and_then(|output| {
            output.links.iter().find(|link| {
                link.path_range
                    .as_ref()
                    .is_some_and(|range| contains_inclusive(range, offset))
            })
        }) {
            let old_path = match self.resolve_link(&path, link) {
                ResolvedTarget::Anchor { path, .. } | ResolvedTarget::Document { path } => path,
                _ => return Err(RenameError::NotRenameable),
            };
            return Ok(PathRenameTarget {
                old_path,
                range: link.path_range.clone().ok_or(RenameError::NotRenameable)?,
            });
        }
        let reference = self
            .anchor_reference_at(&path, offset)
            .filter(|reference| {
                reference
                    .path_range
                    .as_ref()
                    .is_some_and(|range| contains_inclusive(range, offset))
            })
            .ok_or(RenameError::NotRenameable)?;
        Ok(PathRenameTarget {
            old_path: reference.target_path,
            range: reference.path_range.ok_or(RenameError::NotRenameable)?,
        })
    }

    pub fn rename_document(
        &self,
        target: &PathRenameTarget,
        new_path: impl AsRef<Path>,
    ) -> Result<WorkspaceEdit, RenameError> {
        let old_path = normalize(&target.old_path);
        let new_path = if new_path.as_ref().is_absolute() {
            normalize(new_path.as_ref())
        } else {
            normalize(
                &old_path
                    .parent()
                    .unwrap_or_else(|| Path::new(""))
                    .join(new_path),
            )
        };
        if new_path
            .extension()
            .is_none_or(|extension| extension != "plumb")
            || new_path == old_path
        {
            return Err(RenameError::InvalidPath);
        }
        if self.documents.contains_key(&new_path) {
            return Err(RenameError::TargetExists);
        }
        if !self.documents.contains_key(&old_path) {
            return Err(RenameError::NotRenameable);
        }

        let mut grouped: HashMap<PathBuf, Vec<TextEdit>> = HashMap::new();
        for entry in self.documents.values() {
            let Some(current) = &entry.current else {
                continue;
            };
            for link in &current.output.links {
                let Some(path_range) = &link.path_range else {
                    continue;
                };
                let resolved = self.resolve_link(&entry.path, link);
                let old_target = match resolved {
                    ResolvedTarget::Anchor { path, .. } | ResolvedTarget::Document { path } => path,
                    _ => continue,
                };
                let source_moves = entry.path == old_path;
                let target_moves = old_target == old_path;
                if !source_moves && !target_moves {
                    continue;
                }
                let effective_source = if source_moves { &new_path } else { &entry.path };
                let effective_target = if target_moves { &new_path } else { &old_target };
                let Some(replacement) = relative_path(effective_source, effective_target) else {
                    return Err(RenameError::InvalidPath);
                };
                grouped
                    .entry(entry.path.clone())
                    .or_default()
                    .push(TextEdit {
                        range: path_range.clone(),
                        new_text: replacement,
                    });
            }
            for task in &current.output.tasks.tasks {
                for (source, range, target) in task_reference_fields(task) {
                    let Some(reference) =
                        self.task_anchor_reference(&entry.path, source, range, &target)
                    else {
                        continue;
                    };
                    let Some(path_range) = reference.path_range else {
                        continue;
                    };
                    let source_moves = entry.path == old_path;
                    let target_moves = reference.target_path == old_path;
                    if !source_moves && !target_moves {
                        continue;
                    }
                    let effective_source = if source_moves { &new_path } else { &entry.path };
                    let effective_target = if target_moves {
                        &new_path
                    } else {
                        &reference.target_path
                    };
                    let Some(replacement) = relative_path(effective_source, effective_target)
                    else {
                        return Err(RenameError::InvalidPath);
                    };
                    grouped
                        .entry(entry.path.clone())
                        .or_default()
                        .push(TextEdit {
                            range: path_range,
                            new_text: percent_encode_path(&replacement),
                        });
                }
            }
        }
        let mut document_changes = Vec::new();
        for (path, mut edits) in grouped {
            edits.sort_by_key(|edit| edit.range.start);
            if edits
                .windows(2)
                .any(|pair| pair[0].range.end > pair[1].range.start)
            {
                return Err(RenameError::OverlappingEdits);
            }
            document_changes.push(DocumentEdit {
                expected_revision: self
                    .documents
                    .get(&path)
                    .ok_or(RenameError::StaleOrInvalidDocument)?
                    .revision,
                path,
                edits,
            });
        }
        document_changes.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(WorkspaceEdit {
            document_changes,
            resource_operations: vec![ResourceOperation::Rename { old_path, new_path }],
        })
    }

    pub fn insert_metadata(
        &self,
        path: impl AsRef<Path>,
        title: &str,
        created: &str,
    ) -> Result<WorkspaceEdit, MetadataInsertError> {
        let path = normalize(path.as_ref());
        let entry = self
            .documents
            .get(&path)
            .ok_or(MetadataInsertError::StaleOrInvalidDocument)?;
        let current = entry
            .current
            .as_ref()
            .ok_or(MetadataInsertError::StaleOrInvalidDocument)?;
        if current.output.metadata.metadata.is_some() {
            return Err(MetadataInsertError::MetadataAlreadyExists);
        }

        let newline = if entry.parsed.source.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let escaped_title = title.replace('`', "``");
        let escaped_created = created.replace('`', "``");
        let new_text = format!(
            "`meta{newline} `: title{newline}{newline}    {escaped_title}{newline}{newline} `: created{newline}{newline}    {escaped_created}{newline}{newline}"
        );
        Ok(WorkspaceEdit {
            document_changes: vec![DocumentEdit {
                path,
                expected_revision: entry.revision,
                edits: vec![TextEdit {
                    range: 0..0,
                    new_text,
                }],
            }],
            resource_operations: Vec::new(),
        })
    }

    pub fn complete_link(
        &self,
        from: impl AsRef<Path>,
        context: &LinkCompletionContext,
    ) -> Vec<CompletionCandidate> {
        let from = normalize(from.as_ref());
        let mut candidates: Vec<CompletionCandidate> = match context {
            LinkCompletionContext::Label { replace, query } => self
                .documents
                .values()
                .filter_map(|entry| {
                    let versioned = entry.current.as_ref().or(entry.last_valid.as_ref())?;
                    if entry.path == from {
                        return None;
                    }
                    let relative = relative_path(&from, &entry.path)?;
                    let title = versioned
                        .output
                        .metadata
                        .document_title()
                        .filter(|title| !title.is_empty())
                        .unwrap_or_else(|| relative.clone());
                    (fuzzy_match(&relative, query) || fuzzy_match(&title, query)).then(|| {
                        CompletionCandidate {
                            label: title.clone(),
                            detail: relative.clone(),
                            new_text: format!(
                                "`->[{}]{{to=\"{}\"}}",
                                escape_inline_text(&title),
                                escape_quoted_value(&relative)
                            ),
                            replace: replace.clone(),
                        }
                    })
                })
                .collect(),
            LinkCompletionContext::Path { replace, query } => self
                .documents
                .values()
                .filter_map(|entry| {
                    let versioned = entry.current.as_ref().or(entry.last_valid.as_ref())?;
                    if entry.path == from {
                        return None;
                    }
                    let relative = relative_path(&from, &entry.path)?;
                    let title = versioned
                        .output
                        .metadata
                        .document_title()
                        .filter(|title| !title.is_empty())
                        .unwrap_or_else(|| relative.clone());
                    (fuzzy_match(&relative, query) || fuzzy_match(&title, query)).then(|| {
                        CompletionCandidate {
                            label: relative.clone(),
                            detail: title,
                            new_text: relative,
                            replace: replace.clone(),
                        }
                    })
                })
                .collect(),
            LinkCompletionContext::Anchor {
                path,
                replace,
                query,
            } => {
                let target_path = if path.is_empty() {
                    from.clone()
                } else {
                    resolve_relative(&from, path)
                };
                self.documents
                    .get(&target_path)
                    .and_then(|entry| entry.current.as_ref().or(entry.last_valid.as_ref()))
                    .map(|versioned| {
                        versioned
                            .output
                            .anchors
                            .iter()
                            .filter(|anchor| fuzzy_match(&anchor.id.value, query))
                            .map(|anchor| CompletionCandidate {
                                label: format!("#{}", anchor.id.value),
                                detail: format!("explicit anchor in {}", target_path.display()),
                                new_text: anchor.id.value.clone(),
                                replace: replace.clone(),
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            }
        };
        candidates.sort_by(|left, right| left.label.cmp(&right.label));
        candidates
    }

    fn current_output(&self, path: &Path) -> Option<&DocumentOutput> {
        self.documents
            .get(&normalize(path))?
            .current
            .as_ref()
            .map(|versioned| &versioned.output)
    }
}

fn deepest_list_item(blocks: &[Block], offset: usize) -> Option<&ParsedBlock> {
    let mut result = None;
    for block in blocks {
        let Block::Parsed(block) = block else {
            continue;
        };
        if block.range.start <= offset && offset <= block.range.end {
            if block
                .mark
                .as_ref()
                .is_some_and(|mark| matches!(mark.marker.as_str(), "-" | "."))
            {
                result = Some(block);
            }
            if let Some(child) = deepest_list_item(&block.children, offset) {
                result = Some(child);
            }
        }
    }
    result
}

struct BlockIdTarget<'a> {
    attrs: &'a Attributes,
    attribute_insert: usize,
    seed: String,
}

fn deepest_block_id_target(blocks: &[Block], offset: usize) -> Option<BlockIdTarget<'_>> {
    let mut pending = blocks
        .iter()
        .map(|block| (block, 0usize))
        .collect::<Vec<_>>();
    let mut result = None;
    let mut result_position = (0usize, 0usize);
    while let Some((block, depth)) = pending.pop() {
        if !contains_inclusive(block.range(), offset) {
            continue;
        }
        match block {
            Block::Parsed(block) => {
                if let Some(mark) = &block.mark {
                    if result.is_none() || (depth, block.range.start) > result_position {
                        let title = block.head.plain_text();
                        result = Some(BlockIdTarget {
                            attrs: &mark.attrs,
                            attribute_insert: mark.marker_range.end,
                            seed: if title.trim().is_empty() {
                                mark.marker.clone()
                            } else {
                                title.trim().to_string()
                            },
                        });
                        result_position = (depth, block.range.start);
                    }
                }
                pending.extend(block.children.iter().map(|child| (child, depth + 1)));
            }
            Block::Verbatim(block) => {
                if result.is_none() || (depth, block.range.start) > result_position {
                    result = Some(BlockIdTarget {
                        attrs: &block.attrs,
                        attribute_insert: block.opener_range.end,
                        seed: "block".to_string(),
                    });
                    result_position = (depth, block.range.start);
                }
            }
        }
    }
    result
}

fn single_document_edit(entry: &DocumentEntry, path: PathBuf, edit: TextEdit) -> WorkspaceEdit {
    WorkspaceEdit {
        document_changes: vec![DocumentEdit {
            path,
            expected_revision: entry.revision,
            edits: vec![edit],
        }],
        resource_operations: Vec::new(),
    }
}

pub fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component);
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn resolve_relative(from: &Path, target: &str) -> PathBuf {
    let target = Path::new(target);
    if target.is_absolute() {
        normalize(target)
    } else {
        normalize(&from.parent().unwrap_or_else(|| Path::new("")).join(target))
    }
}

fn dependency_cycle_contains(graph: &HashMap<TaskRef, Vec<TaskRef>>, start: &TaskRef) -> bool {
    fn visit(
        graph: &HashMap<TaskRef, Vec<TaskRef>>,
        current: &TaskRef,
        start: &TaskRef,
        visited: &mut HashSet<TaskRef>,
    ) -> bool {
        if !visited.insert(current.clone()) {
            return false;
        }
        graph.get(current).is_some_and(|dependencies| {
            dependencies
                .iter()
                .any(|dependency| dependency == start || visit(graph, dependency, start, visited))
        })
    }

    visit(graph, start, start, &mut HashSet::new())
}

fn recurring_task_attributes(
    task: &TaskRecord,
    next_id: &str,
    timestamp: &str,
    next_due: &str,
    next_wait: Option<&str>,
    recur: &str,
    current_id: &str,
) -> String {
    let mut attributes = task.persistent_attributes.clone();
    attributes.push(format!("#{next_id}"));
    attributes.push(format!("created=\"{}\"", escape_attribute_value(timestamp)));
    attributes.push(format!("due=\"{}\"", escape_attribute_value(next_due)));
    if let Some(wait) = next_wait {
        attributes.push(format!("wait=\"{}\"", escape_attribute_value(wait)));
    }
    attributes.push(format!("recur=\"{}\"", escape_attribute_value(recur)));
    attributes.push(format!("prev=\"#{}\"", escape_attribute_value(current_id)));
    attribute_slot(&attributes)
}

fn attribute_slot(attributes: &[String]) -> String {
    format!("{{{}}}", attributes.join(" "))
}

fn escape_attribute_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn unique_task_instance_id(title: &str, datetime: &str, reserved: &HashSet<String>) -> String {
    let slug = slugify(title, "task");
    let date = datetime.get(..10).unwrap_or("instance");
    unique_id(&format!("{slug}-{date}"), reserved)
}

fn unique_anchor_id(seed: &str, reserved: &HashSet<String>) -> String {
    unique_id(&slugify(seed, "block"), reserved)
}

fn slugify(value: &str, fallback: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() || matches!(character, '_' | '-') {
            if separator && !slug.is_empty() && !slug.ends_with('-') {
                slug.push('-');
            }
            separator = false;
            slug.push(character);
        } else {
            separator = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        slug.push_str(fallback);
    }
    slug
}

fn unique_id(base: &str, reserved: &HashSet<String>) -> String {
    if !reserved.contains(base) {
        return base.to_string();
    }
    for suffix in 2.. {
        let candidate = format!("{base}-{suffix}");
        if !reserved.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

fn percent_decode_path(path: &str) -> String {
    let mut decoded = Vec::with_capacity(path.len());
    let bytes = path.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] == b'%' && cursor + 2 < bytes.len() {
            if let Some(byte) = hex_byte(bytes[cursor + 1], bytes[cursor + 2]) {
                decoded.push(byte);
                cursor += 3;
                continue;
            }
        }
        decoded.push(bytes[cursor]);
        cursor += 1;
    }
    String::from_utf8(decoded).unwrap_or_else(|_| path.to_string())
}

fn percent_encode_path(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            write!(&mut encoded, "%{byte:02X}").expect("writing to String cannot fail");
        }
    }
    encoded
}

fn hex_byte(high: u8, low: u8) -> Option<u8> {
    Some(hex_digit(high)? * 16 + hex_digit(low)?)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn contains_inclusive(range: &std::ops::Range<usize>, offset: usize) -> bool {
    range.start <= offset && offset <= range.end
}

fn valid_anchor_id(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            !character.is_whitespace()
                && !character.is_control()
                && !matches!(
                    character,
                    '`' | '"' | '[' | ']' | '{' | '}' | '#' | '.' | '='
                )
        })
}

fn relative_path(from: &Path, target: &Path) -> Option<String> {
    let from_directory = from.parent().unwrap_or_else(|| Path::new(""));
    let from_components = from_directory.components().collect::<Vec<_>>();
    let target_components = target.components().collect::<Vec<_>>();
    let common = from_components
        .iter()
        .zip(&target_components)
        .take_while(|(left, right)| left == right)
        .count();
    let mut relative = PathBuf::new();
    for _ in common..from_components.len() {
        relative.push("..");
    }
    for component in &target_components[common..] {
        relative.push(component.as_os_str());
    }
    relative.to_str().map(str::to_string)
}

fn fuzzy_match(candidate: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let mut query = query.chars().flat_map(char::to_lowercase);
    let mut wanted = query.next();
    for character in candidate.chars().flat_map(char::to_lowercase) {
        if wanted == Some(character) {
            wanted = query.next();
            if wanted.is_none() {
                return true;
            }
        }
    }
    false
}

fn task_reference_ranges(
    source: &str,
    range: &std::ops::Range<usize>,
    target_id: &str,
) -> Option<(Option<std::ops::Range<usize>>, std::ops::Range<usize>)> {
    let separator = source.find('#')?;
    if &source[separator + 1..] != target_id {
        return None;
    }
    let path_range = (separator > 0).then(|| range.start..range.start + separator);
    let id_start = range.start + separator + 1;
    Some((path_range, id_start..range.end))
}

fn resolved_document_path(target: ResolvedTarget) -> Option<PathBuf> {
    match target {
        ResolvedTarget::Anchor { path, .. }
        | ResolvedTarget::Document { path }
        | ResolvedTarget::UnresolvedAnchor { path, .. }
        | ResolvedTarget::AmbiguousAnchor { path, .. } => Some(path),
        ResolvedTarget::External
        | ResolvedTarget::Other
        | ResolvedTarget::UnresolvedPath { .. } => None,
    }
}

fn task_reference_fields(
    task: &TaskRecord,
) -> Vec<(&str, &std::ops::Range<usize>, TaskReferenceTarget)> {
    task.prev
        .iter()
        .map(|prev| {
            (
                prev.value.as_str(),
                &prev.range,
                parse_task_reference_target(&prev.value),
            )
        })
        .chain(task.depends.iter().map(|dependency| {
            (
                dependency.source.as_str(),
                &dependency.range,
                dependency.target.clone(),
            )
        }))
        .collect()
}

fn escape_inline_text(value: &str) -> String {
    value.replace('`', "``").replace(']', "]]")
}

fn escape_quoted_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_same_and_cross_file_explicit_anchors() {
        let mut workspace = Workspace::new();
        workspace.insert("notes/a.plumb", 1, "`#{#local} Local\n");
        workspace.insert(
            "notes/b.plumb",
            1,
            "See `->[local]{to=\"a.plumb#local\"}.\n",
        );
        let link = &workspace
            .get("notes/b.plumb")
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .output
            .links[0];
        assert!(matches!(
            workspace.resolve_link("notes/b.plumb", link),
            ResolvedTarget::Anchor { ref id, .. } if id == "local"
        ));
    }

    #[test]
    fn headings_without_ids_do_not_resolve() {
        let mut workspace = Workspace::new();
        workspace.insert(
            "a.plumb",
            1,
            "`# No anchor\nSee `->[x]{to=\"#No-anchor\"}.\n",
        );
        let entry = workspace.get("a.plumb").unwrap();
        let link = &entry.current.as_ref().unwrap().output.links[0];
        assert!(matches!(
            workspace.resolve_link("a.plumb", link),
            ResolvedTarget::UnresolvedAnchor { .. }
        ));
    }

    #[test]
    fn invalid_revision_keeps_but_does_not_publish_last_valid_output() {
        let mut workspace = Workspace::new();
        workspace.insert("a.plumb", 1, "`#{#ok} Valid\n");
        workspace.insert("a.plumb", 2, "`node{key=a key=b} Invalid\n");
        let entry = workspace.get("a.plumb").unwrap();
        assert!(entry.current.is_none());
        assert_eq!(entry.last_valid.as_ref().unwrap().revision, 1);
        assert!(workspace.anchor_at("a.plumb", 0).is_none());
    }

    #[test]
    fn returns_reverse_references() {
        let mut workspace = Workspace::new();
        workspace.insert("a.plumb", 1, "`#{#target} Target\n");
        workspace.insert("b.plumb", 1, "`->[x]{to=\"a.plumb#target\"}\n");
        workspace.insert("missing.plumb", 1, "`->[x]{to=\"a.plumb#missing\"}\n");
        workspace.insert(
            "task.plumb",
            1,
            "`-{.task depends=\"a.plumb#missing\"} Task\n",
        );
        assert_eq!(workspace.references_to("a.plumb", "target").len(), 1);
        assert_eq!(
            workspace.referenced_documents_from("missing.plumb"),
            vec![PathBuf::from("a.plumb")]
        );
        assert_eq!(
            workspace.referenced_documents_from("task.plumb"),
            vec![PathBuf::from("a.plumb")]
        );
    }

    #[test]
    fn task_fields_participate_in_navigation_references_and_anchor_rename() {
        let target_source = "`-{.task #draft} Draft\n`node{#note} Note\n";
        let reference_source = "`-{.task #review prev=\"Project%20Plan.plumb#draft\" depends=\"Project%20Plan.plumb#draft Project%20Plan.plumb#note\"} Review\nSee `->[draft]{to=\"Project Plan.plumb#draft\"}.\n";
        let mut workspace = Workspace::new();
        workspace.insert("Project Plan.plumb", 4, target_source);
        workspace.insert("review.plumb", 7, reference_source);

        let depends_attribute = reference_source.find("depends=").unwrap();
        let depends = depends_attribute
            + reference_source[depends_attribute..]
                .find("#draft")
                .unwrap()
            + 1;
        let reference = workspace
            .anchor_reference_at("review.plumb", depends)
            .unwrap();
        assert_eq!(reference.target_path, PathBuf::from("Project Plan.plumb"));
        assert_eq!(reference.target_id, "draft");
        assert_eq!(
            workspace.references_to("Project Plan.plumb", "draft").len(),
            3
        );

        let note = reference_source.find("#note").unwrap() + 1;
        assert_eq!(
            workspace
                .anchor_reference_at("review.plumb", note)
                .unwrap()
                .target_id,
            "note"
        );

        let target = workspace
            .anchor_rename_target_at("review.plumb", depends)
            .unwrap();
        let edit = workspace.rename_anchor(&target, "first-draft").unwrap();
        assert_eq!(edit.document_changes.len(), 2);
        assert_eq!(
            edit.document_changes
                .iter()
                .flat_map(|document| &document.edits)
                .filter(|edit| edit.new_text == "first-draft")
                .count(),
            4
        );
    }

    #[test]
    fn document_rename_rewrites_percent_encoded_task_reference_paths() {
        let target_source = "`-{.task #draft} Draft\n";
        let reference_source = "`-{.task prev=\"Project%20Plan.plumb#draft\" depends=\"Project%20Plan.plumb#draft\"} Review\nSee `->[draft]{to=\"Project Plan.plumb#draft\"}.\n";
        let mut workspace = Workspace::new();
        workspace.insert("Project Plan.plumb", 4, target_source);
        workspace.insert("review.plumb", 7, reference_source);

        let path_offset = reference_source.find("Project%20Plan.plumb").unwrap();
        let target = workspace
            .path_rename_target_at("review.plumb", path_offset)
            .unwrap();
        let edit = workspace
            .rename_document(&target, "Archived Plan.plumb")
            .unwrap();
        let reference_edits = &edit
            .document_changes
            .iter()
            .find(|document| document.path == PathBuf::from("review.plumb"))
            .unwrap()
            .edits;
        assert_eq!(
            reference_edits
                .iter()
                .filter(|edit| edit.new_text == "Archived%20Plan.plumb")
                .count(),
            2
        );
        assert!(reference_edits
            .iter()
            .any(|edit| edit.new_text == "Archived Plan.plumb"));
        assert_eq!(
            edit.resource_operations,
            vec![ResourceOperation::Rename {
                old_path: PathBuf::from("Project Plan.plumb"),
                new_path: PathBuf::from("Archived Plan.plumb"),
            }]
        );
    }

    #[test]
    fn rename_updates_declaration_and_cross_file_fragments() {
        let mut workspace = Workspace::new();
        workspace.insert("a.plumb", 4, "`#{#target} Target\n");
        workspace.insert("b.plumb", 7, "`->[x]{to=\"a.plumb#target\"}\n");
        let target = workspace.anchor_rename_target_at("a.plumb", 5).unwrap();
        let edit = workspace.rename_anchor(&target, "renamed").unwrap();
        assert_eq!(edit.document_changes.len(), 2);
        assert_eq!(edit.document_changes[0].expected_revision, 4);
        assert_eq!(edit.document_changes[1].expected_revision, 7);
        assert!(edit
            .document_changes
            .iter()
            .flat_map(|document| &document.edits)
            .all(|edit| edit.new_text == "renamed"));
    }

    #[test]
    fn rename_rejects_pair_style_or_invalid_ids() {
        let mut workspace = Workspace::new();
        workspace.insert("a.plumb", 1, "`#{id=pair} Not an anchor\n");
        assert_eq!(
            workspace.anchor_rename_target_at("a.plumb", 6),
            Err(RenameError::NotRenameable)
        );
        workspace.insert("a.plumb", 2, "`#{#real} Anchor\n");
        let target = workspace.anchor_rename_target_at("a.plumb", 5).unwrap();
        assert_eq!(
            workspace.rename_anchor(&target, "has space"),
            Err(RenameError::InvalidId)
        );
    }

    #[test]
    fn completes_paths_and_only_explicit_anchors() {
        let mut workspace = Workspace::new();
        workspace.insert("notes/current.plumb", 1, "Current\n");
        workspace.insert(
            "notes/design.plumb",
            1,
            "`meta\n  `: title\n\n    Design Guide\n\n`# No id\n`##{#api} API\n",
        );
        let paths = workspace.complete_link(
            "notes/current.plumb",
            &LinkCompletionContext::Path {
                replace: 10..13,
                query: "guide".to_string(),
            },
        );
        assert_eq!(paths[0].label, "design.plumb");
        assert_eq!(paths[0].detail, "Design Guide");
        assert_eq!(paths[0].new_text, "design.plumb");
        let labels = workspace.complete_link(
            "notes/current.plumb",
            &LinkCompletionContext::Label {
                replace: 0..8,
                query: "guide".to_string(),
            },
        );
        assert_eq!(labels[0].label, "Design Guide");
        assert_eq!(labels[0].detail, "design.plumb");
        assert_eq!(labels[0].new_text, "`->[Design Guide]{to=\"design.plumb\"}");
        let anchors = workspace.complete_link(
            "notes/current.plumb",
            &LinkCompletionContext::Anchor {
                path: "design.plumb".to_string(),
                replace: 20..20,
                query: String::new(),
            },
        );
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].new_text, "api");
    }

    #[test]
    fn document_rename_rewrites_incoming_and_outgoing_relative_paths() {
        let mut workspace = Workspace::new();
        workspace.insert(
            "notes/a.plumb",
            1,
            "`#{#a} A\n`->[c]{to=\"../shared/c.plumb#c\"}\n",
        );
        workspace.insert("notes/b.plumb", 2, "`->[a]{to=\"a.plumb#a\"}\n");
        workspace.insert("shared/c.plumb", 3, "`#{#c} C\n");
        let link = &workspace
            .get("notes/b.plumb")
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .output
            .links[0];
        let offset = link.path_range.as_ref().unwrap().start;
        let target = workspace
            .path_rename_target_at("notes/b.plumb", offset)
            .unwrap();
        let edit = workspace
            .rename_document(&target, "archive/a.plumb")
            .unwrap();
        assert_eq!(edit.resource_operations.len(), 1);
        let incoming = edit
            .document_changes
            .iter()
            .find(|document| document.path == Path::new("notes/b.plumb"))
            .unwrap();
        assert_eq!(incoming.edits[0].new_text, "archive/a.plumb");
        let outgoing = edit
            .document_changes
            .iter()
            .find(|document| document.path == Path::new("notes/a.plumb"))
            .unwrap();
        assert_eq!(outgoing.edits[0].new_text, "../../shared/c.plumb");
    }

    #[test]
    fn resolves_open_task_dependencies_and_blocked_state() {
        let mut workspace = Workspace::new();
        workspace.insert(
            "notes/Project Plan.plumb",
            1,
            "`-{.task #draft} Draft\n`-{.task #done done=\"2026-07-20T09:00:00Z\"} Done\n",
        );
        workspace.insert(
            "notes/review.plumb",
            2,
            "`-{.task #review depends=\"Project%20Plan.plumb#draft Project%20Plan.plumb#done\"} Review\n",
        );

        let task = &workspace
            .get("notes/review.plumb")
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .output
            .tasks
            .tasks[0];
        let blockers = workspace.open_task_dependencies("notes/review.plumb", task);
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].target.id, "draft");
        assert!(workspace.is_task_blocked("notes/review.plumb", task));
        assert_eq!(
            workspace.directly_blocking_tasks("notes/Project Plan.plumb", "draft"),
            vec![TaskRef {
                path: PathBuf::from("notes/review.plumb"),
                id: "review".to_string(),
            }]
        );
        assert_eq!(
            workspace.task_at("notes/review.plumb", task.range.start),
            Some(task)
        );

        let diagnostics = workspace.diagnostics("notes/review.plumb");
        let blocked = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "task.blocked")
            .unwrap();
        assert_eq!(blocked.severity, DiagnosticSeverity::Hint);
    }

    #[test]
    fn diagnoses_invalid_task_targets_self_dependencies_and_cycles() {
        let mut workspace = Workspace::new();
        workspace.insert(
            "tasks.plumb",
            1,
            "`node{#plain} Plain anchor\n`-{.task #a depends=\"#b\"} A\n`-{.task #b depends=\"#a\"} B\n`-{.task #self depends=\"#self\"} Self\n`-{.task prev=\"#plain\" depends=\"#plain #missing bare missing.plumb#x\"} Invalid targets\n",
        );

        let diagnostics = workspace.diagnostics("tasks.plumb");
        let codes = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();
        assert!(codes.contains(&"task.non-task-target"));
        assert!(codes.contains(&"task.unresolved-anchor"));
        assert!(codes.contains(&"task.invalid-target"));
        assert!(codes.contains(&"task.unresolved-path"));
        assert!(codes.contains(&"task.self-dependency"));
        assert!(codes.contains(&"task.dependency-cycle"));
    }

    #[test]
    fn task_status_operation_is_guarded_and_preserves_the_attribute_slot() {
        let mut workspace = Workspace::new();
        let source = "`-{.task #write due=\"2026-07-21T09:00:00Z\"} Write parser\n";
        workspace.insert("tasks.plumb", 7, source);

        let edit = workspace
            .set_task_status_by_id(
                "tasks.plumb",
                "write",
                TaskStatus::Done,
                "2026-07-20T12:00:00+08:00",
            )
            .unwrap();
        let document = &edit.document_changes[0];
        assert_eq!(document.expected_revision, 7);
        assert_eq!(&source[document.edits[0].range.clone()], "");
        assert_eq!(
            document.edits[0].new_text,
            " done=\"2026-07-20T12:00:00+08:00\""
        );
        let offset = document.edits[0].range.start;
        let edited = format!(
            "{}{}{}",
            &source[..offset],
            document.edits[0].new_text,
            &source[offset..]
        );
        assert!(parse(edited).is_valid());
    }

    #[test]
    fn task_authoring_operations_convert_items_and_add_created() {
        let source = "`-{#outer .keep} Outer\n  `- Nested\n`.{.task #closed done=\"2026-07-20T09:00:00Z\"} Closed\n`-{.task #existing created=\"2026-07-19T09:00:00Z\"} Existing\n";
        let mut workspace = Workspace::new();
        workspace.insert("tasks.plumb", 7, source);
        let timestamp = "2026-07-20T10:00:00+08:00";

        let nested_offset = source.find("Nested").unwrap();
        let conversion = workspace
            .convert_list_item_to_task("tasks.plumb", nested_offset, timestamp)
            .unwrap();
        assert_eq!(conversion.document_changes[0].expected_revision, 7);
        let edit = &conversion.document_changes[0].edits[0];
        assert_eq!(
            edit.new_text,
            "{.task created=\"2026-07-20T10:00:00+08:00\"}"
        );
        let mut converted = source.to_string();
        converted.replace_range(edit.range.clone(), &edit.new_text);
        assert!(converted.contains("  `-{.task created=\"2026-07-20T10:00:00+08:00\"} Nested"));

        let outer_conversion = workspace
            .convert_list_item_to_task("tasks.plumb", source.find("Outer").unwrap(), timestamp)
            .unwrap();
        assert_eq!(
            outer_conversion.document_changes[0].edits[0].new_text,
            " .task created=\"2026-07-20T10:00:00+08:00\""
        );

        let closed_offset = source.find("Closed").unwrap();
        let created = workspace
            .add_task_created("tasks.plumb", closed_offset, timestamp)
            .unwrap();
        assert_eq!(
            created.document_changes[0].edits[0].new_text,
            " created=\"2026-07-20T10:00:00+08:00\""
        );
        assert_eq!(
            workspace.add_task_created("tasks.plumb", nested_offset, timestamp),
            Err(TaskEditError::TaskNotFound)
        );
        assert_eq!(
            workspace.add_task_created("tasks.plumb", source.find("Existing").unwrap(), timestamp),
            Err(TaskEditError::CreatedAlreadyExists)
        );
    }

    #[test]
    fn add_explicit_id_targets_the_deepest_block_and_generates_unique_slugs() {
        let source = "`#{.keep} Hello, World!\n`node Outer\n  `child Nested title\n`{language=text}\n  raw\n`note{\n  .keep\n } Multiline attrs\n`other{#hello-world} Existing\n`# Hello, World!\n";
        let mut workspace = Workspace::new();
        workspace.insert("note.plumb", 7, source);

        let heading = workspace
            .add_explicit_id("note.plumb", source.find("Hello, World!").unwrap())
            .unwrap();
        assert_eq!(heading.document_changes[0].expected_revision, 7);
        let edit = &heading.document_changes[0].edits[0];
        assert_eq!(edit.new_text, "#hello-world-2 ");
        assert_eq!(&source[edit.range.clone()], "");
        assert_eq!(&source[edit.range.start - 1..edit.range.start], "{");

        let nested = workspace
            .add_explicit_id("note.plumb", source.find("Nested title").unwrap())
            .unwrap();
        assert_eq!(
            nested.document_changes[0].edits[0].new_text,
            "{#nested-title}"
        );

        let sibling_boundary = workspace
            .add_explicit_id("note.plumb", source.find("`node").unwrap())
            .unwrap();
        assert_eq!(
            sibling_boundary.document_changes[0].edits[0].new_text,
            "{#outer}"
        );

        let raw = workspace
            .add_explicit_id("note.plumb", source.find("raw").unwrap())
            .unwrap();
        assert_eq!(raw.document_changes[0].edits[0].new_text, "#block ");

        let multiline = workspace
            .add_explicit_id("note.plumb", source.find("Multiline attrs").unwrap())
            .unwrap();
        assert_eq!(
            multiline.document_changes[0].edits[0].new_text,
            "#multiline-attrs"
        );

        for operation in [&heading, &nested, &sibling_boundary, &raw, &multiline] {
            let edit = &operation.document_changes[0].edits[0];
            let mut edited = source.to_string();
            edited.replace_range(edit.range.clone(), &edit.new_text);
            let parsed = parse(&edited);
            assert!(parsed.is_valid(), "{edited}\n{:?}", parsed.diagnostics);
            assert!(!analyze_document(&parsed.source, &parsed.syntax)
                .anchors
                .is_empty());
        }

        assert_eq!(
            workspace.add_explicit_id("note.plumb", source.find("Existing").unwrap()),
            Err(ExplicitIdError::IdAlreadyExists)
        );
    }

    #[test]
    fn add_explicit_id_requires_a_valid_marked_or_verbatim_block() {
        let mut workspace = Workspace::new();
        workspace.insert("plain.plumb", 1, "Plain paragraph\n");
        workspace.insert("invalid.plumb", 2, "`node{key=a key=b} Broken\n");

        assert_eq!(
            workspace.add_explicit_id("plain.plumb", 2),
            Err(ExplicitIdError::BlockNotFound)
        );
        assert_eq!(
            workspace.add_explicit_id("invalid.plumb", 2),
            Err(ExplicitIdError::StaleOrInvalidDocument)
        );
        assert_eq!(
            workspace.add_explicit_id("missing.plumb", 0),
            Err(ExplicitIdError::StaleOrInvalidDocument)
        );
    }

    #[test]
    fn task_status_cursor_falls_back_from_closed_child_to_open_parent() {
        let mut workspace = Workspace::new();
        let source =
            "`-{.task #outer} Outer\n  `-{.task #inner done=\"2026-07-20T09:00:00Z\"} Inner\n";
        workspace.insert("tasks.plumb", 3, source);
        let tasks = &workspace
            .get("tasks.plumb")
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .output
            .tasks
            .tasks;

        let edit = workspace
            .set_task_status(
                "tasks.plumb",
                source.find("Inner").unwrap(),
                TaskStatus::Done,
                "2026-07-20T12:00:00Z",
            )
            .unwrap();
        assert_eq!(edit.document_changes[0].edits.len(), 1);
        assert_eq!(
            edit.document_changes[0].edits[0].range.start,
            tasks[0].attribute_insert
        );
        assert_ne!(
            edit.document_changes[0].edits[0].range.start,
            tasks[1].attribute_insert
        );
        assert_eq!(
            workspace.set_task_status_by_id(
                "tasks.plumb",
                "inner",
                TaskStatus::Done,
                "2026-07-20T12:00:00Z",
            ),
            Err(TaskEditError::TaskAlreadyClosed)
        );
    }

    #[test]
    fn task_status_operation_rejects_closed_blocked_and_recurring_tasks() {
        let mut workspace = Workspace::new();
        workspace.insert(
            "tasks.plumb",
            1,
            "`-{.task #blocker} Blocker\n`-{.task #blocked depends=\"#blocker\"} Blocked\n`-{.task #closed done=\"2026-07-20T09:00:00Z\"} Closed\n`-{.task #recur due=\"2026-07-21T09:00:00Z\" recur=P1D} Recurring\n",
        );
        let timestamp = "2026-07-20T12:00:00Z";
        let source = &workspace.get("tasks.plumb").unwrap().parsed.source;
        assert_eq!(
            workspace.set_task_status(
                "tasks.plumb",
                source.find("Blocked").unwrap(),
                TaskStatus::Done,
                timestamp,
            ),
            Err(TaskEditError::TaskBlocked)
        );
        assert!(workspace
            .set_task_status(
                "tasks.plumb",
                source.find("Blocked").unwrap(),
                TaskStatus::Canceled,
                timestamp,
            )
            .is_ok());
        assert_eq!(
            workspace.set_task_status(
                "tasks.plumb",
                source.find("Closed").unwrap(),
                TaskStatus::Canceled,
                timestamp,
            ),
            Err(TaskEditError::TaskAlreadyClosed)
        );
        assert!(workspace
            .set_task_status(
                "tasks.plumb",
                source.find("Recurring").unwrap(),
                TaskStatus::Done,
                timestamp,
            )
            .is_ok());
    }

    #[test]
    fn recurring_task_status_advances_and_clones_the_task_losslessly() {
        let mut workspace = Workspace::new();
        let source = "`-{.task .daily due=\"2026-01-31T09:00:00+08:00\" wait=\"2026-01-30T09:00:00+08:00\" recur=P1M} Monthly review\n  `note Keep details\n  `-{.task #nested done=\"2026-01-20T09:00:00+08:00\"} Nested\n";
        workspace.insert("tasks.plumb", 4, source);

        let edit = workspace
            .set_task_status(
                "tasks.plumb",
                source.find("Nested").unwrap(),
                TaskStatus::Done,
                "2026-01-31T10:00:00+08:00",
            )
            .unwrap();
        let mut edits = edit.document_changes[0].edits.clone();
        assert_eq!(edits.len(), 2);
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
        let mut edited = source.to_string();
        for edit in edits {
            edited.replace_range(edit.range, &edit.new_text);
        }

        assert!(edited.contains("#monthly-review-2026-01-31 done=\"2026-01-31T10:00:00+08:00\""));
        assert!(edited.contains("#monthly-review-2026-02-28"));
        assert!(edited.contains("created=\"2026-01-31T10:00:00+08:00\""));
        assert!(edited.contains("due=\"2026-02-28T09:00:00+08:00\""));
        assert!(edited.contains("wait=\"2026-02-28T09:00:00+08:00\""));
        assert!(edited.contains("prev=\"#monthly-review-2026-01-31\""));
        assert_eq!(edited.matches("#nested").count(), 1);
        assert_eq!(edited.matches("done=\"2026-01-20").count(), 1);
        let parsed = parse(&edited);
        assert!(parsed.is_valid(), "{}\n{:?}", edited, parsed.diagnostics);
        let output = analyze_document(&parsed.source, &parsed.syntax);
        assert_eq!(output.tasks.tasks.len(), 4);
        assert_eq!(output.tasks.tasks[2].state(), TaskState::Open);
    }

    #[test]
    fn inserts_metadata_with_revision_and_escaped_title() {
        let mut workspace = Workspace::new();
        workspace.insert("notes/my`note.plumb", 7, "`# Section\n");

        let edit = workspace
            .insert_metadata(
                "notes/my`note.plumb",
                "my`note",
                "2026-07-19T12:34:56+08:00",
            )
            .unwrap();

        assert_eq!(edit.document_changes.len(), 1);
        let document = &edit.document_changes[0];
        assert_eq!(document.path, Path::new("notes/my`note.plumb"));
        assert_eq!(document.expected_revision, 7);
        assert_eq!(document.edits[0].range, 0..0);
        assert_eq!(
            document.edits[0].new_text,
            "`meta\n `: title\n\n    my``note\n\n `: created\n\n    2026-07-19T12:34:56+08:00\n\n"
        );
    }

    #[test]
    fn metadata_insertion_preserves_crlf() {
        let mut workspace = Workspace::new();
        workspace.insert("note.plumb", 1, "First\r\nSecond\r\n");

        let edit = workspace
            .insert_metadata("note.plumb", "note", "2026-07-19T12:34:56+08:00")
            .unwrap();

        assert_eq!(
            edit.document_changes[0].edits[0].new_text,
            "`meta\r\n `: title\r\n\r\n    note\r\n\r\n `: created\r\n\r\n    2026-07-19T12:34:56+08:00\r\n\r\n"
        );
    }

    #[test]
    fn metadata_insertion_rejects_existing_or_invalid_metadata_target() {
        let mut workspace = Workspace::new();
        workspace.insert("existing.plumb", 1, "`meta\n  `: title\n\n    Existing\n");
        assert_eq!(
            workspace.insert_metadata("existing.plumb", "existing", "created"),
            Err(MetadataInsertError::MetadataAlreadyExists)
        );

        workspace.insert("invalid.plumb", 2, "`node{key=a key=b} Broken\n");
        assert_eq!(
            workspace.insert_metadata("invalid.plumb", "invalid", "created"),
            Err(MetadataInsertError::StaleOrInvalidDocument)
        );
        assert_eq!(
            workspace.insert_metadata("missing.plumb", "missing", "created"),
            Err(MetadataInsertError::StaleOrInvalidDocument)
        );
    }
}
