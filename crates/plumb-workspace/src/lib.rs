use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveTime, SecondsFormat, TimeZone, Timelike};
pub use plumb_edit::{apply_text_edits, TextEdit};
use plumb_edit::{
    remove_block as remove_syntax_block, replace_owned_block, replace_owned_blocks,
    AttributePosition, EditSession, OwnedAttribute, OwnedBlock, OwnedInline,
};
#[cfg(test)]
use plumb_semantics::TaskStatus;
use plumb_semantics::{
    analyze_document, parse_task_reference_target, AnchorRecord, DocumentOutput, EventRecord,
    FileCompletionContext, FileRecord, FileTarget, ImageCompletionContext, ImageRecord,
    ImageTarget, LinkCompletionContext, LinkRecord, LinkSpelling, LinkTarget, MetadataOutput,
    MetadataValue, TaskDependency, TaskDependencyCompletionContext, TaskRecord,
    TaskReferenceTarget, TaskState,
};
use plumb_syntax::{
    parse, Attributes, Block, Diagnostic, DiagnosticSeverity, ParsedBlock, ParsedDocument,
};

mod scan;
mod search;
mod store;
mod task_sort;
mod tasks;

pub use store::{SqliteSemanticStore, StoreError};

#[cfg(test)]
use scan::resolve_workspace_root_from;
pub use scan::{
    discover_workspace_root, display_workspace_path, resolve_workspace_root, scan_workspace_files,
    WorkspaceScan,
};
use search::derive_task_workflow_state;
pub use search::{
    search_score, SearchRecord, SearchRecordKind, SearchResults, TaskWaitReason, TaskWorkflowState,
};
pub use task_sort::{
    sort_task_records, sort_task_records_by, truncate_complete_task_documents, TaskSortFacts,
    TaskSortOrder,
};
use tasks::TaskTargetResolution;
pub use tasks::{ResolvedTaskDependency, TaskEditError, TaskRef};

pub const WORKSPACE_MARKER: &str = ".plumb";

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyDocumentEditError {
    DocumentNotEdited,
    RevisionMismatch,
    InvalidEdits,
}

pub fn apply_document_edit(
    source: String,
    path: impl AsRef<Path>,
    revision: i64,
    edit: WorkspaceEdit,
) -> Result<String, ApplyDocumentEditError> {
    let path = path.as_ref();
    let document = edit
        .document_changes
        .into_iter()
        .find(|document| document.path == path)
        .ok_or(ApplyDocumentEditError::DocumentNotEdited)?;
    if document.expected_revision != revision {
        return Err(ApplyDocumentEditError::RevisionMismatch);
    }
    apply_text_edits(source, document.edits).map_err(|_| ApplyDocumentEditError::InvalidEdits)
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
    CursorNotAtDocumentStart,
    GeneratedInvalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExplicitIdError {
    StaleOrInvalidDocument,
    BlockNotFound,
    IdAlreadyExists,
    GeneratedInvalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventInput {
    pub title: String,
    pub at: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub tasks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskAuthoringInput {
    pub title: String,
    pub created: Option<String>,
    pub due: Option<String>,
    pub wait: Option<String>,
    pub recur: Option<String>,
    pub prev: Option<String>,
    pub depends: Vec<String>,
    pub priority: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskPlacement {
    pub parent: Option<std::ops::Range<usize>>,
    pub after: Option<std::ops::Range<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskAuthoringPatch {
    pub title: Option<String>,
    pub created: Option<Option<String>>,
    pub due: Option<Option<String>>,
    pub wait: Option<Option<String>>,
    pub recur: Option<Option<String>>,
    pub prev: Option<Option<String>>,
    pub depends: Option<Vec<String>>,
    pub priority: Option<Option<i32>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskAuthoringError {
    StaleOrInvalidDocument,
    TaskNotFound,
    InvalidPlacement,
    InvalidDatetime,
    InvalidRecurrence,
    InvalidReference,
    UnresolvedReference,
    DependencyCycle,
    GeneratedInvalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventEditError {
    StaleOrInvalidDocument,
    EventNotFound,
    InvalidDatetime,
    InvalidTimeShape,
    InvalidInterval,
    GeneratedInvalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventShorthandError {
    StaleOrInvalidDocument,
    ListItemNotFound,
    EventAlreadyExists,
    InvalidShorthand,
    InvalidInterval,
    GeneratedInvalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathRenameTarget {
    pub old_path: PathBuf,
    pub range: std::ops::Range<usize>,
    pub input: PathRenameInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathRenameInput {
    Path,
    FileStem,
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
    pub current: Option<Arc<VersionedDocumentOutput>>,
    pub last_valid: Option<Arc<VersionedDocumentOutput>>,
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
    File {
        path: PathBuf,
    },
    UnresolvedFile {
        path: PathBuf,
    },
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentReference {
    pub source_range: std::ops::Range<usize>,
    pub target_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceOccurrence {
    pub source_path: PathBuf,
    pub source_range: std::ops::Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DocumentReverseReferences {
    pub document: Vec<ReferenceOccurrence>,
    pub anchors: HashMap<String, Vec<ReferenceOccurrence>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceEvent {
    pub path: PathBuf,
    pub revision: i64,
    pub event: EventRecord,
}

#[derive(Debug, Default, Clone)]
pub struct Workspace {
    documents: HashMap<PathBuf, DocumentEntry>,
    disk_store: Option<SqliteSemanticStore>,
}

impl Workspace {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_sqlite_store(store: SqliteSemanticStore) -> Self {
        Self {
            documents: HashMap::new(),
            disk_store: Some(store),
        }
    }

    pub fn insert_disk(
        &mut self,
        path: impl AsRef<Path>,
        revision: i64,
        source: impl Into<String>,
    ) -> Result<bool, StoreError> {
        let path = normalize(path.as_ref());
        let source = source.into();
        let Some(store) = &self.disk_store else {
            self.insert(path, revision, source);
            return Ok(false);
        };
        if store.contains_current(&path, &source)? {
            return Ok(true);
        }
        let parsed = parse(source);
        let output = parsed
            .is_valid()
            .then(|| analyze_document(&parsed.source, &parsed.syntax));
        store.replace(&path, revision, &parsed.source, output.as_ref())?;
        Ok(false)
    }

    pub fn open_document(
        &mut self,
        path: impl AsRef<Path>,
        revision: i64,
        source: impl Into<String>,
    ) -> &DocumentEntry {
        self.insert(path, revision, source)
    }

    pub fn close_document(&mut self, path: impl AsRef<Path>) -> Option<DocumentEntry> {
        if self.disk_store.is_some() {
            self.documents.remove(&normalize(path.as_ref()))
        } else {
            None
        }
    }

    pub fn remove_disk(&mut self, path: impl AsRef<Path>) -> Result<(), StoreError> {
        let path = normalize(path.as_ref());
        if let Some(store) = &self.disk_store {
            store.remove(&path)?;
        } else {
            self.documents.remove(&path);
        }
        Ok(())
    }

    pub fn has_persistent_store(&self) -> bool {
        self.disk_store.is_some()
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
        let current = parsed.is_valid().then(|| {
            Arc::new(VersionedDocumentOutput {
                revision,
                output: analyze_document(&parsed.source, &parsed.syntax),
            })
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

    pub fn rebind_revision_if_source(
        &mut self,
        path: impl AsRef<Path>,
        revision: i64,
        source: &str,
    ) -> bool {
        let path = normalize(path.as_ref());
        let Some(entry) = self.documents.get_mut(&path) else {
            return false;
        };
        if entry.parsed.source != source {
            return false;
        }
        entry.revision = revision;
        let Some(current) = entry.current.take() else {
            return true;
        };
        entry.last_valid = None;
        let rebound = match Arc::try_unwrap(current) {
            Ok(mut output) => {
                output.revision = revision;
                Arc::new(output)
            }
            Err(output) => Arc::new(VersionedDocumentOutput {
                revision,
                output: output.output.clone(),
            }),
        };
        entry.current = Some(rebound.clone());
        entry.last_valid = Some(rebound);
        true
    }

    pub fn remove(&mut self, path: impl AsRef<Path>) -> Option<DocumentEntry> {
        self.documents.remove(&normalize(path.as_ref()))
    }

    pub fn get(&self, path: impl AsRef<Path>) -> Option<&DocumentEntry> {
        self.documents.get(&normalize(path.as_ref()))
    }

    pub fn contains(&self, path: impl AsRef<Path>) -> bool {
        let path = normalize(path.as_ref());
        self.documents.contains_key(&path)
            || self.disk_store.as_ref().is_some_and(|store| {
                store
                    .documents()
                    .is_ok_and(|documents| documents.iter().any(|document| document.path == path))
            })
    }

    pub fn documents(&self) -> impl Iterator<Item = &DocumentEntry> {
        self.documents.values()
    }

    pub fn document_paths(&self) -> Vec<PathBuf> {
        let mut paths = self.documents.keys().cloned().collect::<HashSet<_>>();
        if let Some(store) = &self.disk_store {
            if let Ok(documents) = store.documents() {
                paths.extend(documents.into_iter().map(|document| document.path));
            }
        }
        let mut paths = paths.into_iter().collect::<Vec<_>>();
        paths.sort();
        paths
    }

    fn open_paths(&self) -> Vec<PathBuf> {
        let mut paths = self.documents.keys().cloned().collect::<Vec<_>>();
        paths.sort();
        paths
    }

    pub fn resolve_link(&self, from: impl AsRef<Path>, link: &LinkRecord) -> ResolvedTarget {
        let from = normalize(from.as_ref());
        match &link.target_kind {
            LinkTarget::External => ResolvedTarget::External,
            LinkTarget::File { path } => {
                let target = resolve_relative(&from, path);
                if target.is_file() {
                    ResolvedTarget::File { path: target }
                } else {
                    ResolvedTarget::UnresolvedFile { path: target }
                }
            }
            LinkTarget::Other => ResolvedTarget::Other,
            LinkTarget::Document { path } => {
                let target = resolve_relative(&from, path);
                if self.contains(&target) {
                    ResolvedTarget::Document { path: target }
                } else {
                    ResolvedTarget::UnresolvedPath { path: target }
                }
            }
            LinkTarget::Anchor { path, fragment } => {
                let target = path
                    .as_deref()
                    .map_or_else(|| from.clone(), |path| resolve_relative(&from, path));
                if !self.contains(&target) {
                    return ResolvedTarget::UnresolvedPath { path: target };
                }
                let anchors = self.anchors_named(&target, fragment);
                let Some(anchor) = anchors.first() else {
                    return ResolvedTarget::UnresolvedAnchor {
                        path: target,
                        id: fragment.clone(),
                    };
                };
                if anchors.len() > 1 {
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

    pub fn resolve_image(&self, from: impl AsRef<Path>, image: &ImageRecord) -> ResolvedTarget {
        match &image.target_kind {
            ImageTarget::External => ResolvedTarget::External,
            ImageTarget::File { path } => {
                let target = resolve_relative(from.as_ref(), path);
                if target.is_file() {
                    ResolvedTarget::File { path: target }
                } else {
                    ResolvedTarget::UnresolvedFile { path: target }
                }
            }
        }
    }

    pub fn image_at(&self, path: impl AsRef<Path>, offset: usize) -> Option<&ImageRecord> {
        self.current_output(path.as_ref())?
            .images
            .iter()
            .filter(|image| contains_inclusive(&image.range, offset))
            .max_by_key(|image| image.range.start)
    }

    pub fn resolve_file(&self, from: impl AsRef<Path>, file: &FileRecord) -> ResolvedTarget {
        match &file.target_kind {
            FileTarget::External => ResolvedTarget::External,
            FileTarget::File { path } => {
                let target = resolve_relative(from.as_ref(), path);
                if target.is_file() {
                    ResolvedTarget::File { path: target }
                } else {
                    ResolvedTarget::UnresolvedFile { path: target }
                }
            }
        }
    }

    pub fn file_at(&self, path: impl AsRef<Path>, offset: usize) -> Option<&FileRecord> {
        self.current_output(path.as_ref())?
            .files
            .iter()
            .filter(|file| contains_inclusive(&file.range, offset))
            .max_by_key(|file| file.range.start)
    }

    pub fn reference_target_at(
        &self,
        path: impl AsRef<Path>,
        offset: usize,
    ) -> Option<ResolvedTarget> {
        let path = normalize(path.as_ref());
        let output = self.current_output(&path)?;
        if let Some(link) = output
            .links
            .iter()
            .filter(|link| contains_inclusive(&link.range, offset))
            .max_by_key(|link| link.range.start)
        {
            let target = self.resolve_link(&path, link);
            if link
                .path_range
                .as_ref()
                .is_some_and(|range| contains_component(range, offset))
            {
                return Some(self.document_component_target(target));
            }
            return Some(target);
        }
        for task in &output.tasks.tasks {
            for (source, range, target) in task_reference_fields(task) {
                if !contains_inclusive(range, offset) {
                    continue;
                }
                let resolved = self.resolve_task_reference_target(&path, &target);
                let target_id = match &target {
                    TaskReferenceTarget::Internal { id }
                    | TaskReferenceTarget::External { id, .. } => id,
                    TaskReferenceTarget::Invalid => return Some(resolved),
                };
                if task_reference_ranges(source, range, target_id)
                    .and_then(|(path_range, _)| path_range)
                    .as_ref()
                    .is_some_and(|range| contains_component(range, offset))
                {
                    return Some(self.document_component_target(resolved));
                }
                return Some(resolved);
            }
        }
        for event in &output.events.events {
            for reference in &self.event_task_references_in_output(&path, output, event) {
                if !contains_inclusive(&reference.range, offset) {
                    continue;
                }
                let resolved = self.resolve_task_reference_target(&path, &reference.target);
                let target_id = match &reference.target {
                    TaskReferenceTarget::Internal { id }
                    | TaskReferenceTarget::External { id, .. } => id,
                    TaskReferenceTarget::Invalid => return Some(resolved),
                };
                if task_reference_ranges(&reference.source, &reference.range, target_id)
                    .and_then(|(path_range, _)| path_range)
                    .as_ref()
                    .is_some_and(|range| contains_component(range, offset))
                {
                    return Some(self.document_component_target(resolved));
                }
                return Some(resolved);
            }
        }
        if let Some(image) = self.image_at(&path, offset) {
            return Some(self.resolve_image(&path, image));
        }
        if let Some(file) = self.file_at(&path, offset) {
            return Some(self.resolve_file(&path, file));
        }
        None
    }

    pub fn target_at(&self, path: impl AsRef<Path>, offset: usize) -> Option<ResolvedTarget> {
        let path = normalize(path.as_ref());
        if self
            .current_output(&path)?
            .metadata
            .metadata
            .as_ref()
            .is_some_and(|metadata| contains_inclusive(&metadata.selection_range, offset))
        {
            return Some(ResolvedTarget::Document { path });
        }
        if let Some(target) = self.reference_target_at(&path, offset) {
            return Some(target);
        }
        self.anchor_at(&path, offset)
            .map(|anchor| ResolvedTarget::Anchor {
                path,
                id: anchor.id.value.clone(),
                anchor: anchor.clone(),
            })
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
        for event in &output.events.events {
            if let Some(reference) = event
                .tasks
                .iter()
                .find(|reference| contains_inclusive(&reference.range, offset))
            {
                return self.task_anchor_reference(
                    &path,
                    &reference.source,
                    &reference.range,
                    &reference.target,
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
        for event in &output.events.events {
            if let Some(reference) = event
                .tasks
                .iter()
                .find(|reference| contains_inclusive(&reference.range, offset))
            {
                return Some(self.resolve_task_reference_target(&path, &reference.target));
            }
        }
        None
    }

    pub fn references_to(
        &self,
        target_path: impl AsRef<Path>,
        target_id: &str,
    ) -> Vec<(PathBuf, AnchorReference)> {
        let target_path = normalize(target_path.as_ref());
        let mut references = Vec::new();
        if let Some(store) = &self.disk_store {
            let anchors = self.anchors_named(&target_path, target_id);
            if anchors.len() == 1 {
                if let Ok(stored) =
                    store.references_to(&target_path, Some(target_id), &self.open_paths())
                {
                    let anchor = anchors[0].clone();
                    references.extend(stored.into_iter().filter_map(|reference| {
                        Some((
                            reference.source_path,
                            AnchorReference {
                                source_range: reference.source_range,
                                path_range: reference.path_range,
                                id_range: reference.id_range?,
                                target_path: target_path.clone(),
                                target_id: target_id.to_string(),
                                anchor: anchor.clone(),
                            },
                        ))
                    }));
                }
            }
        }
        for entry in self.documents.values() {
            let Some(current) = &entry.current else {
                continue;
            };
            for link in &current.output.links {
                if let Some(reference) = self.link_anchor_reference(&entry.path, link) {
                    if reference.target_path == target_path && reference.target_id == target_id {
                        references.push((entry.path.clone(), reference));
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
                            references.push((entry.path.clone(), reference));
                        }
                    }
                }
            }
            for event in &current.output.events.events {
                for reference in
                    &self.event_task_references_in_output(&entry.path, &current.output, event)
                {
                    if let Some(reference) = self.task_anchor_reference(
                        &entry.path,
                        &reference.source,
                        &reference.range,
                        &reference.target,
                    ) {
                        if reference.target_path == target_path && reference.target_id == target_id
                        {
                            references.push((entry.path.clone(), reference));
                        }
                    }
                }
            }
        }
        references.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then(left.1.source_range.start.cmp(&right.1.source_range.start))
        });
        references
    }

    pub fn references_to_document(
        &self,
        target_path: impl AsRef<Path>,
    ) -> Vec<(PathBuf, DocumentReference)> {
        let target_path = normalize(target_path.as_ref());
        let mut references = Vec::new();
        if let Some(store) = &self.disk_store {
            let open = self.open_paths();
            if let Ok(stored) = store.references_to(&target_path, None, &open) {
                references.extend(stored.into_iter().map(|reference| {
                    (
                        reference.source_path,
                        DocumentReference {
                            source_range: reference.source_range,
                            target_path: target_path.clone(),
                        },
                    )
                }));
            }
            if let Ok(anchors) = store.anchors(&[]) {
                let mut ids = anchors
                    .into_iter()
                    .filter(|anchor| anchor.path == target_path)
                    .map(|anchor| anchor.record.id.value)
                    .collect::<Vec<_>>();
                ids.sort();
                ids.dedup();
                for id in ids {
                    if self.anchors_named(&target_path, &id).len() != 1 {
                        continue;
                    }
                    if let Ok(stored) = store.references_to(&target_path, Some(&id), &open) {
                        references.extend(stored.into_iter().map(|reference| {
                            (
                                reference.source_path,
                                DocumentReference {
                                    source_range: reference.source_range,
                                    target_path: target_path.clone(),
                                },
                            )
                        }));
                    }
                }
            }
        }
        for entry in self.documents.values() {
            let Some(current) = &entry.current else {
                continue;
            };
            for link in &current.output.links {
                if resolved_document_path(self.resolve_link(&entry.path, link)).as_ref()
                    == Some(&target_path)
                {
                    references.push((
                        entry.path.clone(),
                        DocumentReference {
                            source_range: link.selection_range.clone(),
                            target_path: target_path.clone(),
                        },
                    ));
                }
            }
            for task in &current.output.tasks.tasks {
                for (_, range, target) in task_reference_fields(task) {
                    if resolved_document_path(
                        self.resolve_task_reference_target(&entry.path, &target),
                    )
                    .as_ref()
                        == Some(&target_path)
                    {
                        references.push((
                            entry.path.clone(),
                            DocumentReference {
                                source_range: range.clone(),
                                target_path: target_path.clone(),
                            },
                        ));
                    }
                }
            }
            for event in &current.output.events.events {
                for reference in
                    &self.event_task_references_in_output(&entry.path, &current.output, event)
                {
                    if resolved_document_path(
                        self.resolve_task_reference_target(&entry.path, &reference.target),
                    )
                    .as_ref()
                        == Some(&target_path)
                    {
                        references.push((
                            entry.path.clone(),
                            DocumentReference {
                                source_range: reference.range.clone(),
                                target_path: target_path.clone(),
                            },
                        ));
                    }
                }
            }
        }
        references.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then(left.1.source_range.start.cmp(&right.1.source_range.start))
        });
        references
    }

    pub fn reverse_references_for_document(
        &self,
        target_path: impl AsRef<Path>,
        target_ids: &HashSet<String>,
    ) -> DocumentReverseReferences {
        let target_path = normalize(target_path.as_ref());
        let mut references = DocumentReverseReferences::default();
        if let Some(store) = &self.disk_store {
            let open = self.open_paths();
            if let Ok(document_references) = store.references_to(&target_path, None, &open) {
                references
                    .document
                    .extend(
                        document_references
                            .into_iter()
                            .map(|reference| ReferenceOccurrence {
                                source_path: reference.source_path,
                                source_range: reference.source_range,
                            }),
                    );
            }
            for target_id in target_ids {
                if self.anchors_named(&target_path, target_id).len() != 1 {
                    continue;
                }
                if let Ok(anchor_references) =
                    store.references_to(&target_path, Some(target_id), &open)
                {
                    let occurrences = anchor_references
                        .into_iter()
                        .map(|reference| ReferenceOccurrence {
                            source_path: reference.source_path,
                            source_range: reference.source_range,
                        })
                        .collect::<Vec<_>>();
                    references.document.extend(occurrences.iter().cloned());
                    references
                        .anchors
                        .entry(target_id.clone())
                        .or_default()
                        .extend(occurrences);
                }
            }
        }
        for entry in self.documents.values() {
            let Some(current) = &entry.current else {
                continue;
            };
            for link in &current.output.links {
                collect_reverse_reference(
                    &mut references,
                    &target_path,
                    target_ids,
                    &entry.path,
                    link.selection_range.clone(),
                    self.resolve_link(&entry.path, link),
                );
            }
            for task in &current.output.tasks.tasks {
                for (_, range, target) in task_reference_fields(task) {
                    collect_reverse_reference(
                        &mut references,
                        &target_path,
                        target_ids,
                        &entry.path,
                        range.clone(),
                        self.resolve_task_reference_target(&entry.path, &target),
                    );
                }
            }
            for event in &current.output.events.events {
                for reference in
                    &self.event_task_references_in_output(&entry.path, &current.output, event)
                {
                    collect_reverse_reference(
                        &mut references,
                        &target_path,
                        target_ids,
                        &entry.path,
                        reference.range.clone(),
                        self.resolve_task_reference_target(&entry.path, &reference.target),
                    );
                }
            }
        }
        references.document.sort_by(reference_occurrence_order);
        for occurrences in references.anchors.values_mut() {
            occurrences.sort_by(reference_occurrence_order);
        }
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
        for event in &output.events.events {
            for reference in &self.event_task_references_in_output(&source_path, output, event) {
                if let Some(path) = resolved_document_path(
                    self.resolve_task_reference_target(&source_path, &reference.target),
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
            TaskReferenceTarget::External { path, id } => {
                (resolve_relative(from, path), id.clone())
            }
            TaskReferenceTarget::Invalid => return ResolvedTarget::Other,
        };
        if !self.contains(&path) {
            return ResolvedTarget::UnresolvedPath { path };
        }
        let anchors = self.anchors_named(&path, &id);
        let Some(anchor) = anchors.first().cloned() else {
            return ResolvedTarget::UnresolvedAnchor { path, id };
        };
        if anchors.len() > 1 {
            return ResolvedTarget::AmbiguousAnchor { path, id };
        }
        ResolvedTarget::Anchor { path, id, anchor }
    }

    fn document_component_target(&self, target: ResolvedTarget) -> ResolvedTarget {
        let path = match target {
            ResolvedTarget::Anchor { path, .. }
            | ResolvedTarget::Document { path }
            | ResolvedTarget::UnresolvedAnchor { path, .. }
            | ResolvedTarget::AmbiguousAnchor { path, .. }
            | ResolvedTarget::UnresolvedPath { path } => path,
            other => return other,
        };
        if self.contains(&path) {
            ResolvedTarget::Document { path }
        } else {
            ResolvedTarget::UnresolvedPath { path }
        }
    }

    pub fn event_at(&self, path: impl AsRef<Path>, offset: usize) -> Option<&EventRecord> {
        self.current_output(path.as_ref())?
            .events
            .events
            .iter()
            .filter(|event| event.range.start <= offset && offset <= event.range.end)
            .max_by_key(|event| event.range.start)
    }

    pub fn events_overlapping(
        &self,
        start: DateTime<FixedOffset>,
        end: DateTime<FixedOffset>,
    ) -> Vec<WorkspaceEvent> {
        if end <= start {
            return Vec::new();
        }
        let mut events = self
            .documents
            .values()
            .filter_map(|entry| entry.current.as_ref().map(|current| (entry, current)))
            .flat_map(|(entry, current)| {
                current
                    .output
                    .events
                    .events
                    .iter()
                    .filter(|event| event.overlaps(start, end))
                    .cloned()
                    .map(|event| WorkspaceEvent {
                        path: entry.path.clone(),
                        revision: current.revision,
                        event,
                    })
            })
            .collect::<Vec<_>>();
        if let Some(store) = &self.disk_store {
            if let Ok(stored) = store.events_overlapping(
                start.timestamp_millis(),
                end.timestamp_millis(),
                &self.open_paths(),
            ) {
                events.extend(stored.into_iter().map(|stored| WorkspaceEvent {
                    path: stored.path,
                    revision: stored.revision,
                    event: stored.record,
                }));
            }
        }
        events.sort_by(|left, right| {
            left.event
                .sort_datetime()
                .cmp(&right.event.sort_datetime())
                .then(left.path.cmp(&right.path))
                .then(left.event.range.start.cmp(&right.event.range.start))
        });
        events
    }

    pub fn events_for_task(&self, target: &TaskRef) -> Vec<WorkspaceEvent> {
        let mut events = Vec::new();
        for entry in self.documents.values() {
            let Some(current) = &entry.current else {
                continue;
            };
            for event in &current.output.events.events {
                if self
                    .event_task_references_in_output(&entry.path, &current.output, event)
                    .iter()
                    .any(|reference| {
                        matches!(
                            self.resolve_task_target(&entry.path, &reference.target),
                            TaskTargetResolution::Task { target: ref resolved, .. } if resolved == target
                        )
                    })
                {
                    events.push(WorkspaceEvent {
                        path: entry.path.clone(),
                        revision: current.revision,
                        event: event.clone(),
                    });
                }
            }
        }
        events.sort_by(|left, right| {
            left.event
                .sort_datetime()
                .cmp(&right.event.sort_datetime())
                .then(left.path.cmp(&right.path))
                .then(left.event.range.start.cmp(&right.event.range.start))
        });
        events
    }

    pub fn event_task_references(
        &self,
        path: impl AsRef<Path>,
        event: &EventRecord,
    ) -> Vec<TaskDependency> {
        if event.tasks_override {
            return event.tasks.clone();
        }
        let path = normalize(path.as_ref());
        let Some(current) = self.current_output(&path) else {
            return Vec::new();
        };
        self.event_task_references_in_output(&path, current, event)
    }

    fn event_task_references_in_output(
        &self,
        path: &Path,
        current: &DocumentOutput,
        event: &EventRecord,
    ) -> Vec<TaskDependency> {
        if event.tasks_override {
            return event.tasks.clone();
        }
        let first = current
            .links
            .partition_point(|link| link.range.start < event.range.start);
        current.links[first..]
            .iter()
            .take_while(|link| link.range.start <= event.range.end)
            .filter(|link| {
                event.range.start <= link.range.start && link.range.end <= event.range.end
            })
            .filter_map(|link| {
                let LinkTarget::Anchor {
                    path: target_path,
                    fragment,
                } = &link.target_kind
                else {
                    return None;
                };
                let resolved = self.resolve_link(&path, link);
                let ResolvedTarget::Anchor {
                    path: resolved_path,
                    id,
                    ..
                } = resolved
                else {
                    return None;
                };
                let target_output = self.current_output(&resolved_path)?;
                let is_task = target_output
                    .tasks
                    .tasks
                    .iter()
                    .any(|task| task.id.as_ref().is_some_and(|field| field.value == id));
                if !is_task {
                    return None;
                }
                let target = match target_path {
                    Some(target_path) => TaskReferenceTarget::External {
                        path: target_path.clone(),
                        id: fragment.clone(),
                    },
                    None => TaskReferenceTarget::Internal {
                        id: fragment.clone(),
                    },
                };
                Some(TaskDependency {
                    source: link.target.value.clone(),
                    range: link.target.range.clone(),
                    target,
                })
            })
            .collect()
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
        let mut edit = EditSession::new(&entry.parsed, target.block_range)
            .map_err(|_| ExplicitIdError::GeneratedInvalid)?;
        edit.insert_attribute(
            target.attrs,
            target.attribute_insert,
            AttributePosition::First,
            OwnedAttribute::id(id),
        )
        .map_err(|_| ExplicitIdError::GeneratedInvalid)?;
        let edit = edit
            .finish()
            .map_err(|_| ExplicitIdError::GeneratedInvalid)?;
        Ok(single_document_edit(entry, path, edit))
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
        diagnostics.extend(current.output.events.diagnostics.clone());
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
                ResolvedTarget::UnresolvedFile { path } => (
                    "link.unresolved-file",
                    format!("unresolved file reference '{}'", path.display()),
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
        for image in &current.output.images {
            let ResolvedTarget::UnresolvedFile { path: target } = self.resolve_image(&path, image)
            else {
                continue;
            };
            diagnostics.push(Diagnostic {
                code: "image.unresolved-file",
                severity: DiagnosticSeverity::Warning,
                message: format!("unresolved image file '{}'", target.display()),
                range: image.source.range.clone(),
                related: Vec::new(),
            });
        }
        for file in &current.output.files {
            let ResolvedTarget::UnresolvedFile { path: target } = self.resolve_file(&path, file)
            else {
                continue;
            };
            diagnostics.push(Diagnostic {
                code: "file.unresolved-file",
                severity: DiagnosticSeverity::Warning,
                message: format!("unresolved file attachment '{}'", target.display()),
                range: file.source.range.clone(),
                related: Vec::new(),
            });
        }
        diagnostics.extend(self.task_workspace_diagnostics(&path, current));
        diagnostics.extend(self.event_workspace_diagnostics(&path, current));
        diagnostics
    }

    fn event_workspace_diagnostics(
        &self,
        path: &Path,
        current: &VersionedDocumentOutput,
    ) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        for event in &current.output.events.events {
            for reference in &self.event_task_references_in_output(path, &current.output, event) {
                if let Some(mut diagnostic) = self.task_target_diagnostic(
                    path,
                    &reference.source,
                    &reference.range,
                    &reference.target,
                    "association",
                ) {
                    diagnostic.code = match diagnostic.code {
                        "task.invalid-target" => "event.invalid-task-reference",
                        "task.unresolved-path" => "event.unresolved-task-path",
                        "task.unresolved-anchor" => "event.unresolved-task",
                        "task.ambiguous-anchor" => "event.ambiguous-task",
                        "task.non-task-target" => "event.target-not-task",
                        code => code,
                    };
                    diagnostics.push(diagnostic);
                }
            }
        }
        diagnostics
    }

    fn task_workspace_diagnostics(
        &self,
        path: &Path,
        current: &VersionedDocumentOutput,
    ) -> Vec<Diagnostic> {
        let graph = self.task_dependency_graph();
        let mut diagnostics = Vec::new();
        let tasks = &current.output.tasks.tasks;
        for (task_index, task) in tasks.iter().enumerate() {
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
            if task.state() == TaskState::Done {
                let blockers = self.open_task_dependencies(path, task);
                let blocker_targets = blockers
                    .iter()
                    .map(|dependency| dependency.target.clone())
                    .collect::<HashSet<_>>();
                if !blockers.is_empty() {
                    diagnostics.push(Diagnostic {
                        code: "task.done-with-open-dependency",
                        severity: DiagnosticSeverity::Warning,
                        message: format!(
                            "completed task still depends on {} open {}",
                            blockers.len(),
                            if blockers.len() == 1 { "task" } else { "tasks" }
                        ),
                        range: task
                            .done
                            .as_ref()
                            .expect("done task has a done field")
                            .range
                            .clone(),
                        related: blockers
                            .iter()
                            .filter(|dependency| dependency.target.path == path)
                            .map(|dependency| dependency.task.selection_range.clone())
                            .collect(),
                    });
                }

                let open_descendants = tasks
                    .iter()
                    .skip(task_index + 1)
                    .take_while(|descendant| descendant.depth > task.depth)
                    .filter(|descendant| descendant.state() == TaskState::Open)
                    .filter(|descendant| {
                        descendant.id.as_ref().is_none_or(|id| {
                            !blocker_targets.contains(&TaskRef {
                                path: path.to_path_buf(),
                                id: id.value.clone(),
                            })
                        })
                    })
                    .collect::<Vec<_>>();
                if !open_descendants.is_empty() {
                    diagnostics.push(Diagnostic {
                        code: "task.done-with-open-descendant",
                        severity: DiagnosticSeverity::Warning,
                        message: format!(
                            "completed task still contains {} open {}",
                            open_descendants.len(),
                            if open_descendants.len() == 1 {
                                "descendant"
                            } else {
                                "descendants"
                            }
                        ),
                        range: task
                            .done
                            .as_ref()
                            .expect("done task has a done field")
                            .range
                            .clone(),
                        related: open_descendants
                            .iter()
                            .map(|descendant| descendant.selection_range.clone())
                            .collect(),
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
            .push(validated_token_edit(
                entry,
                anchor.id.range.clone(),
                replacement,
            )?);
        for (path, reference) in self.references_to(&target.path, &target.id) {
            let reference_entry = self
                .entry_for_operation(&path)
                .ok_or(RenameError::StaleOrInvalidDocument)?;
            grouped.entry(path).or_default().push(validated_token_edit(
                &reference_entry,
                reference.id_range,
                replacement,
            )?);
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
                .entry_for_operation(&path)
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
                input: PathRenameInput::Path,
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
            input: PathRenameInput::Path,
        })
    }

    pub fn document_rename_target_at(
        &self,
        path: impl AsRef<Path>,
        offset: usize,
    ) -> Result<PathRenameTarget, RenameError> {
        let path = normalize(path.as_ref());
        if let Some(metadata) = self
            .current_output(&path)
            .and_then(|output| output.metadata.metadata.as_ref())
            .filter(|metadata| contains_inclusive(&metadata.selection_range, offset))
        {
            return Ok(PathRenameTarget {
                old_path: path,
                range: metadata.selection_range.clone(),
                input: PathRenameInput::FileStem,
            });
        }
        self.path_rename_target_at(path, offset)
    }

    pub fn rename_document(
        &self,
        target: &PathRenameTarget,
        new_path: impl AsRef<Path>,
    ) -> Result<WorkspaceEdit, RenameError> {
        let old_path = normalize(&target.old_path);
        let requested_path = new_path.as_ref();
        let new_path = match target.input {
            PathRenameInput::Path => {
                if requested_path.is_absolute() {
                    normalize(requested_path)
                } else {
                    normalize(
                        &old_path
                            .parent()
                            .unwrap_or_else(|| Path::new(""))
                            .join(requested_path),
                    )
                }
            }
            PathRenameInput::FileStem => {
                if requested_path.is_absolute()
                    || requested_path.file_name().is_none()
                    || requested_path
                        .parent()
                        .is_some_and(|parent| !parent.as_os_str().is_empty())
                    || requested_path
                        .extension()
                        .is_some_and(|extension| extension != "plumb")
                {
                    return Err(RenameError::InvalidPath);
                }
                let file_name = if requested_path.extension().is_some() {
                    requested_path.to_path_buf()
                } else {
                    requested_path.with_extension("plumb")
                };
                normalize(
                    &old_path
                        .parent()
                        .unwrap_or_else(|| Path::new(""))
                        .join(file_name),
                )
            }
        };
        if new_path
            .extension()
            .is_none_or(|extension| extension != "plumb")
            || new_path == old_path
        {
            return Err(RenameError::InvalidPath);
        }
        if self.contains(&new_path) || new_path.exists() {
            return Err(RenameError::TargetExists);
        }
        if !self.contains(&old_path) {
            return Err(RenameError::NotRenameable);
        }

        let mut grouped: HashMap<PathBuf, Vec<TextEdit>> = HashMap::new();
        let entries = self.entries_for_operation();
        for entry in &entries {
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
                    .push(link_path_rename_edit(entry, link, path_range, replacement)?);
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
                        .push(validated_token_edit(entry, path_range, replacement)?);
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
                    .entry_for_operation(&path)
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
        offset: usize,
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
        if !entry.parsed.source.get(..offset).is_some_and(|prefix| {
            prefix
                .bytes()
                .all(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
        }) {
            return Err(MetadataInsertError::CursorNotAtDocumentStart);
        }

        let metadata = OwnedBlock::document(vec![
            OwnedAttribute::bare("title", title),
            OwnedAttribute::bare("created", created),
        ]);
        let affected = 0..if entry.parsed.syntax.blocks.is_empty() {
            entry.parsed.source.len()
        } else {
            0
        };
        let mut edit = EditSession::new(&entry.parsed, affected)
            .map_err(|_| MetadataInsertError::GeneratedInvalid)?;
        edit.insert_blocks(0, &[metadata])
            .map_err(|_| MetadataInsertError::GeneratedInvalid)?;
        let edit = edit
            .finish()
            .map_err(|_| MetadataInsertError::GeneratedInvalid)?;
        Ok(single_document_edit(entry, path, edit))
    }

    pub fn create_event(
        &self,
        path: impl AsRef<Path>,
        input: &EventInput,
    ) -> Result<WorkspaceEdit, EventEditError> {
        validate_event_input(input)?;
        let path = normalize(path.as_ref());
        let entry = self
            .documents
            .get(&path)
            .filter(|entry| entry.current.is_some())
            .ok_or(EventEditError::StaleOrInvalidDocument)?;
        let current = entry.current.as_ref().expect("current output checked");
        let event = owned_event(input, &current.output.metadata);
        let (affected, after) = entry
            .parsed
            .syntax
            .blocks
            .last()
            .map(|block| (block.range().clone(), Some(block.range().clone())))
            .unwrap_or_else(|| (0..entry.parsed.source.len(), None));
        let mut edit = EditSession::new(&entry.parsed, affected)
            .map_err(|_| EventEditError::GeneratedInvalid)?;
        if let Some(after) = after {
            edit.insert_sibling_blocks(&after, &[event])
                .map_err(|_| EventEditError::GeneratedInvalid)?;
        } else {
            edit.insert_blocks(0, &[event])
                .map_err(|_| EventEditError::GeneratedInvalid)?;
        }
        let edit = edit
            .finish()
            .map_err(|_| EventEditError::GeneratedInvalid)?;
        Ok(single_document_edit(entry, path, edit))
    }

    pub fn convert_event_shorthand(
        &self,
        path: impl AsRef<Path>,
        offset: usize,
        now: DateTime<FixedOffset>,
    ) -> Result<WorkspaceEdit, EventShorthandError> {
        let path = normalize(path.as_ref());
        let entry = self
            .documents
            .get(&path)
            .filter(|entry| entry.current.is_some())
            .ok_or(EventShorthandError::StaleOrInvalidDocument)?;
        let item = deepest_list_item(&entry.parsed.syntax.blocks, offset)
            .ok_or(EventShorthandError::ListItemNotFound)?;
        let mark = item.mark.as_ref().expect("list item has a mark");
        if mark.marker == "event" {
            return Err(EventShorthandError::EventAlreadyExists);
        }
        let current = entry.current.as_ref().expect("current output checked");
        let next = next_parsed_sibling(&entry.parsed.syntax.blocks, &item.range);
        let inferred_end = next.and_then(|next| {
            inferred_end_from_sibling(&entry.parsed.source, next, now, &current.output.metadata)
        });
        let (input, title_start) = parse_event_shorthand_head(
            &entry.parsed.source,
            item,
            now,
            &current.output.metadata,
            inferred_end,
        )?;
        let mut owned = OwnedBlock::from_parsed(&entry.parsed.source, item);
        owned.set_marker("event");
        owned.attributes_mut().retain(
            |attribute| !matches!(attribute, OwnedAttribute::Class(value) if value == "event"),
        );
        strip_event_shorthand_prefix(&mut owned, title_start)?;
        let mut attributes = owned.attributes().to_vec();
        attributes.extend(event_attributes(&input, &current.output.metadata));
        if !attributes.is_empty() {
            owned = owned.with_attributes(attributes);
        }
        prepend_event_schedule(&mut owned, &input);
        let event_edit = replace_owned_block(&entry.parsed, item.range.clone(), &owned)
            .map_err(|_| EventShorthandError::GeneratedInvalid)?;
        Ok(single_document_edit(entry, path, event_edit))
    }

    pub fn convert_event_shorthands(
        &self,
        path: impl AsRef<Path>,
        selection: std::ops::Range<usize>,
        now: DateTime<FixedOffset>,
    ) -> Result<WorkspaceEdit, EventShorthandError> {
        let path = normalize(path.as_ref());
        let entry = self
            .documents
            .get(&path)
            .filter(|entry| entry.current.is_some())
            .ok_or(EventShorthandError::StaleOrInvalidDocument)?;
        let metadata = &entry
            .current
            .as_ref()
            .expect("current output checked")
            .output
            .metadata;
        let mut edits = Vec::new();
        let mut converted = 0;
        for (index, block) in entry.parsed.syntax.blocks.iter().enumerate() {
            let Block::Parsed(parsed) = block else {
                continue;
            };
            let next_sibling = entry
                .parsed
                .syntax
                .blocks
                .get(index + 1)
                .and_then(parsed_block);
            let mut owned = OwnedBlock::from_parsed(&entry.parsed.source, parsed);
            let count = convert_shorthands_in_block(
                &entry.parsed.source,
                parsed,
                next_sibling,
                &mut owned,
                &selection,
                now,
                metadata,
            );
            if count == 0 {
                continue;
            }
            converted += count;
            edits.push(
                replace_owned_block(&entry.parsed, parsed.range.clone(), &owned)
                    .map_err(|_| EventShorthandError::GeneratedInvalid)?,
            );
        }
        if converted == 0 {
            return Err(EventShorthandError::ListItemNotFound);
        }
        Ok(WorkspaceEdit {
            document_changes: vec![DocumentEdit {
                path,
                expected_revision: entry.revision,
                edits,
            }],
            resource_operations: Vec::new(),
        })
    }

    pub fn create_task(
        &self,
        path: impl AsRef<Path>,
        input: &TaskAuthoringInput,
        placement: &TaskPlacement,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, TaskAuthoringError> {
        validate_task_authoring_input(input, timestamp)?;
        let path = normalize(path.as_ref());
        let entry = self
            .documents
            .get(&path)
            .filter(|entry| entry.current.is_some())
            .ok_or(TaskAuthoringError::StaleOrInvalidDocument)?;
        let id = format!("task-{}", uuid::Uuid::new_v4().simple());
        self.validate_authored_task_references(&path, Some(&id), input)?;
        let task = owned_authored_task(input, &id, timestamp);
        let edit = if let Some(parent_range) = &placement.parent {
            let parent = parsed_block_with_range(&entry.parsed.syntax.blocks, parent_range)
                .ok_or(TaskAuthoringError::InvalidPlacement)?;
            let parent_task = entry
                .current
                .as_ref()
                .expect("current output checked")
                .output
                .tasks
                .tasks
                .iter()
                .any(|candidate| candidate.range == *parent_range);
            if !parent_task {
                return Err(TaskAuthoringError::InvalidPlacement);
            }
            let mut owned = OwnedBlock::from_parsed(&entry.parsed.source, parent);
            let children = owned.children_mut().expect("parsed block has children");
            let index = child_insertion_index(&parent.children, placement.after.as_ref())?;
            children.insert(index, task);
            let mut edit = EditSession::new(&entry.parsed, parent_range.clone())
                .map_err(|_| TaskAuthoringError::GeneratedInvalid)?;
            edit.replace_block(parent_range.clone(), &owned)
                .map_err(|_| TaskAuthoringError::GeneratedInvalid)?;
            edit
        } else {
            let after = placement
                .after
                .as_ref()
                .or_else(|| entry.parsed.syntax.blocks.last().map(Block::range));
            let affected = after.cloned().unwrap_or(0..entry.parsed.source.len());
            let mut edit = EditSession::new(&entry.parsed, affected.clone())
                .map_err(|_| TaskAuthoringError::GeneratedInvalid)?;
            if let Some(after) = after {
                if !entry
                    .parsed
                    .syntax
                    .blocks
                    .iter()
                    .any(|block| block.range() == after)
                {
                    return Err(TaskAuthoringError::InvalidPlacement);
                }
                edit.insert_sibling_blocks(after, &[task])
                    .map_err(|_| TaskAuthoringError::GeneratedInvalid)?;
            } else {
                edit.insert_blocks(0, &[task])
                    .map_err(|_| TaskAuthoringError::GeneratedInvalid)?;
            }
            edit
        };
        let edit = edit
            .finish()
            .map_err(|_| TaskAuthoringError::GeneratedInvalid)?;
        Ok(single_document_edit(entry, path, edit))
    }

    pub fn update_task(
        &self,
        path: impl AsRef<Path>,
        task_range: std::ops::Range<usize>,
        input: &TaskAuthoringInput,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, TaskAuthoringError> {
        self.update_task_patch(
            path,
            task_range,
            &TaskAuthoringPatch {
                title: Some(input.title.clone()),
                created: Some(input.created.clone()),
                due: Some(input.due.clone()),
                wait: Some(input.wait.clone()),
                recur: Some(input.recur.clone()),
                prev: Some(input.prev.clone()),
                depends: Some(input.depends.clone()),
                priority: Some(input.priority),
            },
            timestamp,
        )
    }

    pub fn update_task_patch(
        &self,
        path: impl AsRef<Path>,
        task_range: std::ops::Range<usize>,
        patch: &TaskAuthoringPatch,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, TaskAuthoringError> {
        let path = normalize(path.as_ref());
        let entry = self
            .documents
            .get(&path)
            .ok_or(TaskAuthoringError::StaleOrInvalidDocument)?;
        let current = entry
            .current
            .as_ref()
            .ok_or(TaskAuthoringError::StaleOrInvalidDocument)?;
        let task = current
            .output
            .tasks
            .tasks
            .iter()
            .find(|task| task.range == task_range)
            .ok_or(TaskAuthoringError::TaskNotFound)?;
        let mut input = TaskAuthoringInput {
            title: task.title.clone(),
            created: task.created.as_ref().map(|field| field.value.clone()),
            due: task.due.as_ref().map(|field| field.value.clone()),
            wait: task.wait.as_ref().map(|field| field.value.clone()),
            recur: task.recur.as_ref().map(|field| field.value.clone()),
            prev: task.prev.as_ref().map(|field| field.value.clone()),
            depends: task
                .depends
                .iter()
                .map(|dependency| dependency.source.clone())
                .collect(),
            priority: task.priority,
        };
        if let Some(title) = &patch.title {
            input.title = title.clone();
        }
        if let Some(created) = &patch.created {
            input.created = created.clone();
        }
        if let Some(due) = &patch.due {
            input.due = due.clone();
        }
        if let Some(wait) = &patch.wait {
            input.wait = wait.clone();
        }
        if let Some(recur) = &patch.recur {
            input.recur = recur.clone();
        }
        if let Some(prev) = &patch.prev {
            input.prev = prev.clone();
        }
        if let Some(depends) = &patch.depends {
            input.depends = depends.clone();
        }
        if let Some(priority) = patch.priority {
            input.priority = priority;
        }
        validate_task_authoring_input(&input, timestamp)?;
        self.validate_authored_task_references(
            &path,
            task.id.as_ref().map(|id| id.value.as_str()),
            &input,
        )?;
        let block = parsed_block_with_range(&entry.parsed.syntax.blocks, &task.range)
            .ok_or(TaskAuthoringError::TaskNotFound)?;
        let mut owned = OwnedBlock::from_parsed(&entry.parsed.source, block);
        owned.set_head_text(&input.title);
        let attributes = owned.attributes_mut();
        attributes.retain(|attribute| {
            !matches!(attribute, OwnedAttribute::Pair { key, .. }
                if matches!(key.as_str(), "created" | "due" | "wait" | "recur" | "prev" | "depends" | "priority"))
        });
        append_authored_task_fields(
            attributes,
            &input,
            task.created
                .as_ref()
                .map_or(timestamp, |created| created.value.as_str()),
        );
        let edit = replace_owned_block(&entry.parsed, task.range.clone(), &owned)
            .map_err(|_| TaskAuthoringError::GeneratedInvalid)?;
        Ok(single_document_edit(entry, path, edit))
    }

    pub fn move_task(
        &self,
        path: impl AsRef<Path>,
        task_range: std::ops::Range<usize>,
        placement: &TaskPlacement,
    ) -> Result<WorkspaceEdit, TaskAuthoringError> {
        let path = normalize(path.as_ref());
        let entry = self
            .documents
            .get(&path)
            .filter(|entry| entry.current.is_some())
            .ok_or(TaskAuthoringError::StaleOrInvalidDocument)?;
        let task = entry
            .current
            .as_ref()
            .expect("current output checked")
            .output
            .tasks
            .tasks
            .iter()
            .find(|task| task.range == task_range)
            .ok_or(TaskAuthoringError::TaskNotFound)?;
        let source = parsed_block_with_range(&entry.parsed.syntax.blocks, &task.range)
            .ok_or(TaskAuthoringError::TaskNotFound)?;
        if placement
            .parent
            .as_ref()
            .is_some_and(|parent| task.range.start <= parent.start && parent.end <= task.range.end)
            || placement.after.as_ref() == Some(&task.range)
        {
            return Err(TaskAuthoringError::InvalidPlacement);
        }
        let moved = OwnedBlock::from_parsed(&entry.parsed.source, source);
        let source_parent = direct_parent_range(&entry.parsed.syntax.blocks, &task.range);
        if placement.parent.as_ref() == source_parent.as_ref() {
            if let Some(parent_range) = source_parent {
                return self.reorder_task_children(
                    entry,
                    path,
                    parent_range,
                    task.range.clone(),
                    placement.after.as_ref(),
                );
            }
        }
        if placement.parent.is_none()
            && placement
                .after
                .as_ref()
                .is_some_and(|after| after.start <= task.range.start && task.range.end <= after.end)
        {
            let after = placement.after.as_ref().expect("checked after");
            let ancestor = parsed_block_with_range(&entry.parsed.syntax.blocks, after)
                .ok_or(TaskAuthoringError::InvalidPlacement)?;
            let mut owned_ancestor = OwnedBlock::from_parsed(&entry.parsed.source, ancestor);
            if !remove_owned_descendant(ancestor, &mut owned_ancestor, &task.range) {
                return Err(TaskAuthoringError::InvalidPlacement);
            }
            let edit = replace_owned_blocks(&entry.parsed, after.clone(), &[owned_ancestor, moved])
                .map_err(|_| TaskAuthoringError::GeneratedInvalid)?;
            return Ok(single_document_edit(entry, path, edit));
        }
        let target_parent = placement.parent.as_ref();
        let remove = if let Some(parent_range) = source_parent.as_ref().filter(|source_parent| {
            target_parent.is_none_or(|target_parent| {
                source_parent.end <= target_parent.start || target_parent.end <= source_parent.start
            })
        }) {
            let parent = parsed_block_with_range(&entry.parsed.syntax.blocks, parent_range)
                .ok_or(TaskAuthoringError::InvalidPlacement)?;
            let source_index = parent
                .children
                .iter()
                .position(|child| child.range() == &task.range)
                .ok_or(TaskAuthoringError::InvalidPlacement)?;
            let mut owned_parent = OwnedBlock::from_parsed(&entry.parsed.source, parent);
            owned_parent
                .children_mut()
                .expect("parsed parent")
                .remove(source_index);
            replace_owned_block(&entry.parsed, parent_range.clone(), &owned_parent)
                .map_err(|_| TaskAuthoringError::GeneratedInvalid)?
        } else {
            remove_syntax_block(&entry.parsed, task.range.clone())
                .map_err(|_| TaskAuthoringError::GeneratedInvalid)?
        };
        let target_edit = if let Some(parent_range) = &placement.parent {
            let parent = parsed_block_with_range(&entry.parsed.syntax.blocks, parent_range)
                .ok_or(TaskAuthoringError::InvalidPlacement)?;
            let is_task = entry
                .current
                .as_ref()
                .expect("current output checked")
                .output
                .tasks
                .tasks
                .iter()
                .any(|task| task.range == *parent_range);
            if !is_task {
                return Err(TaskAuthoringError::InvalidPlacement);
            }
            let mut owned = OwnedBlock::from_parsed(&entry.parsed.source, parent);
            let index = child_insertion_index(&parent.children, placement.after.as_ref())?;
            owned
                .children_mut()
                .expect("parsed parent")
                .insert(index, moved);
            replace_owned_block(&entry.parsed, parent_range.clone(), &owned)
                .map_err(|_| TaskAuthoringError::GeneratedInvalid)?
        } else {
            let after = placement.after.as_ref().or_else(|| {
                entry
                    .parsed
                    .syntax
                    .blocks
                    .iter()
                    .rev()
                    .map(Block::range)
                    .find(|range| **range != task.range)
            });
            let Some(after) = after else {
                return Ok(WorkspaceEdit::default());
            };
            if !entry
                .parsed
                .syntax
                .blocks
                .iter()
                .any(|block| block.range() == after)
            {
                return Err(TaskAuthoringError::InvalidPlacement);
            }
            let mut insert = EditSession::new(&entry.parsed, after.clone())
                .map_err(|_| TaskAuthoringError::GeneratedInvalid)?;
            insert
                .insert_sibling_blocks(after, &[moved])
                .map_err(|_| TaskAuthoringError::GeneratedInvalid)?;
            insert
                .finish()
                .map_err(|_| TaskAuthoringError::GeneratedInvalid)?
        };
        Ok(WorkspaceEdit {
            document_changes: vec![DocumentEdit {
                path,
                expected_revision: entry.revision,
                edits: vec![remove, target_edit],
            }],
            resource_operations: Vec::new(),
        })
    }

    fn reorder_task_children(
        &self,
        entry: &DocumentEntry,
        path: PathBuf,
        parent_range: std::ops::Range<usize>,
        task_range: std::ops::Range<usize>,
        after: Option<&std::ops::Range<usize>>,
    ) -> Result<WorkspaceEdit, TaskAuthoringError> {
        let parent = parsed_block_with_range(&entry.parsed.syntax.blocks, &parent_range)
            .ok_or(TaskAuthoringError::InvalidPlacement)?;
        let source_index = parent
            .children
            .iter()
            .position(|child| child.range() == &task_range)
            .ok_or(TaskAuthoringError::InvalidPlacement)?;
        let mut target_index = child_insertion_index(&parent.children, after)?;
        let mut owned = OwnedBlock::from_parsed(&entry.parsed.source, parent);
        let children = owned.children_mut().expect("parsed parent");
        let moved = children.remove(source_index);
        if target_index > source_index {
            target_index -= 1;
        }
        children.insert(target_index, moved);
        let edit = replace_owned_block(&entry.parsed, parent_range, &owned)
            .map_err(|_| TaskAuthoringError::GeneratedInvalid)?;
        Ok(single_document_edit(entry, path, edit))
    }

    fn validate_authored_task_references(
        &self,
        path: &Path,
        id: Option<&str>,
        input: &TaskAuthoringInput,
    ) -> Result<(), TaskAuthoringError> {
        let mut dependencies = Vec::new();
        for reference in input.prev.iter().chain(&input.depends) {
            let target = parse_task_reference_target(reference);
            let TaskTargetResolution::Task { target, .. } = self.resolve_task_target(path, &target)
            else {
                return Err(TaskAuthoringError::UnresolvedReference);
            };
            if input
                .depends
                .iter()
                .any(|dependency| dependency == reference)
            {
                dependencies.push(target);
            }
        }
        if let Some(id) = id {
            let own = TaskRef {
                path: path.to_path_buf(),
                id: id.to_string(),
            };
            let mut graph = self.task_dependency_graph();
            graph.insert(own.clone(), dependencies);
            if dependency_cycle_contains(&graph, &own) {
                return Err(TaskAuthoringError::DependencyCycle);
            }
        }
        Ok(())
    }

    pub fn update_event(
        &self,
        path: impl AsRef<Path>,
        event_range: std::ops::Range<usize>,
        input: &EventInput,
    ) -> Result<WorkspaceEdit, EventEditError> {
        validate_event_input(input)?;
        let path = normalize(path.as_ref());
        let entry = self
            .documents
            .get(&path)
            .ok_or(EventEditError::StaleOrInvalidDocument)?;
        let current = entry
            .current
            .as_ref()
            .ok_or(EventEditError::StaleOrInvalidDocument)?;
        let event = current
            .output
            .events
            .events
            .iter()
            .find(|event| event.range == event_range)
            .ok_or(EventEditError::EventNotFound)?;
        let block = parsed_block_with_range(&entry.parsed.syntax.blocks, &event.range)
            .ok_or(EventEditError::EventNotFound)?;
        let mut owned = OwnedBlock::from_parsed(&entry.parsed.source, block);
        set_event_head(&mut owned, input);
        let attributes = owned.attributes_mut();
        attributes.retain(|attribute| {
            !matches!(attribute, OwnedAttribute::Class(value) if value == "event")
                && !matches!(attribute, OwnedAttribute::Pair { key, .. } if matches!(key.as_str(), "date" | "timezone" | "at" | "start" | "end" | "tasks"))
        });
        attributes.extend(event_attributes(input, &current.output.metadata));
        let mut edit = EditSession::new(&entry.parsed, event.range.clone())
            .map_err(|_| EventEditError::GeneratedInvalid)?;
        edit.replace_block(event.range.clone(), &owned)
            .map_err(|_| EventEditError::GeneratedInvalid)?;
        let edit = edit
            .finish()
            .map_err(|_| EventEditError::GeneratedInvalid)?;
        Ok(single_document_edit(entry, path, edit))
    }

    pub fn delete_event(
        &self,
        path: impl AsRef<Path>,
        event_range: std::ops::Range<usize>,
    ) -> Result<WorkspaceEdit, EventEditError> {
        let path = normalize(path.as_ref());
        let entry = self
            .documents
            .get(&path)
            .ok_or(EventEditError::StaleOrInvalidDocument)?;
        let current = entry
            .current
            .as_ref()
            .ok_or(EventEditError::StaleOrInvalidDocument)?;
        let event = current
            .output
            .events
            .events
            .iter()
            .find(|event| event.range == event_range)
            .ok_or(EventEditError::EventNotFound)?;
        let mut edit = EditSession::new(&entry.parsed, event.range.clone())
            .map_err(|_| EventEditError::GeneratedInvalid)?;
        edit.remove_block(event.range.clone())
            .map_err(|_| EventEditError::GeneratedInvalid)?;
        let edit = edit
            .finish()
            .map_err(|_| EventEditError::GeneratedInvalid)?;
        Ok(single_document_edit(entry, path, edit))
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
                                "`->[{}]{{`:[to {}]}}",
                                escape_parsed_text(&title),
                                escape_attached_text(&relative)
                            ),
                            replace: replace.clone(),
                        }
                    })
                })
                .collect(),
            LinkCompletionContext::Path {
                replace,
                query,
                quoted,
            } => self
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
                    if !fuzzy_match(&relative, query) && !fuzzy_match(&title, query) {
                        return None;
                    }
                    if !*quoted && !valid_bare_attribute_value(&relative) {
                        return None;
                    }
                    Some(CompletionCandidate {
                        label: relative.clone(),
                        detail: title,
                        new_text: if *quoted {
                            escape_quoted_value(&relative)
                        } else {
                            relative
                        },
                        replace: replace.clone(),
                    })
                })
                .collect(),
            LinkCompletionContext::AutolinkPath {
                replace,
                envelope,
                quote_count,
                suffix,
                query,
            } => self
                .documents
                .values()
                .filter_map(|entry| {
                    let versioned = entry.current.as_ref().or(entry.last_valid.as_ref())?;
                    if entry.path == from {
                        return None;
                    }
                    let relative = relative_path(&from, &entry.path)?;
                    if !valid_autolink_completion_path(&relative) {
                        return None;
                    }
                    let title = versioned
                        .output
                        .metadata
                        .document_title()
                        .filter(|title| !title.is_empty())
                        .unwrap_or_else(|| relative.clone());
                    (fuzzy_match(&relative, query) || fuzzy_match(&title, query)).then(|| {
                        let payload = format!("{relative}{suffix}");
                        let (new_text, replace) =
                            if verbatim_payload_is_safe(&payload, *quote_count) {
                                (relative.clone(), replace.clone())
                            } else {
                                (format_inline_verbatim(&payload), envelope.clone())
                            };
                        CompletionCandidate {
                            label: relative.clone(),
                            detail: title,
                            new_text,
                            replace,
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
            LinkCompletionContext::AutolinkAnchor {
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
        if let Some(store) = &self.disk_store {
            let open = self.open_paths();
            match context {
                LinkCompletionContext::Label { replace, query } => {
                    if let Ok(documents) = store.documents() {
                        candidates.extend(documents.into_iter().filter_map(|document| {
                            if open.binary_search(&document.path).is_ok() || document.path == from {
                                return None;
                            }
                            let relative = relative_path(&from, &document.path)?;
                            let title = if document.title.is_empty() {
                                relative.clone()
                            } else {
                                document.title
                            };
                            (fuzzy_match(&relative, query) || fuzzy_match(&title, query)).then(
                                || CompletionCandidate {
                                    label: title.clone(),
                                    detail: relative.clone(),
                                    new_text: format!(
                                        "`->[{}]{{`:[to {}]}}",
                                        escape_parsed_text(&title),
                                        escape_attached_text(&relative)
                                    ),
                                    replace: replace.clone(),
                                },
                            )
                        }));
                    }
                }
                LinkCompletionContext::Path {
                    replace,
                    query,
                    quoted,
                } => {
                    if let Ok(documents) = store.documents() {
                        candidates.extend(documents.into_iter().filter_map(|document| {
                            if open.binary_search(&document.path).is_ok() || document.path == from {
                                return None;
                            }
                            let relative = relative_path(&from, &document.path)?;
                            let title = if document.title.is_empty() {
                                relative.clone()
                            } else {
                                document.title
                            };
                            if (!fuzzy_match(&relative, query) && !fuzzy_match(&title, query))
                                || (!*quoted && !valid_bare_attribute_value(&relative))
                            {
                                return None;
                            }
                            Some(CompletionCandidate {
                                label: relative.clone(),
                                detail: title,
                                new_text: if *quoted {
                                    escape_quoted_value(&relative)
                                } else {
                                    relative
                                },
                                replace: replace.clone(),
                            })
                        }));
                    }
                }
                LinkCompletionContext::AutolinkPath {
                    replace,
                    envelope,
                    quote_count,
                    suffix,
                    query,
                } => {
                    if let Ok(documents) = store.documents() {
                        candidates.extend(documents.into_iter().filter_map(|document| {
                            if open.binary_search(&document.path).is_ok() || document.path == from {
                                return None;
                            }
                            let relative = relative_path(&from, &document.path)?;
                            if !valid_autolink_completion_path(&relative) {
                                return None;
                            }
                            let title = if document.title.is_empty() {
                                relative.clone()
                            } else {
                                document.title
                            };
                            if !fuzzy_match(&relative, query) && !fuzzy_match(&title, query) {
                                return None;
                            }
                            let payload = format!("{relative}{suffix}");
                            let (new_text, replace) =
                                if verbatim_payload_is_safe(&payload, *quote_count) {
                                    (relative.clone(), replace.clone())
                                } else {
                                    (format_inline_verbatim(&payload), envelope.clone())
                                };
                            Some(CompletionCandidate {
                                label: relative,
                                detail: title,
                                new_text,
                                replace,
                            })
                        }));
                    }
                }
                LinkCompletionContext::Anchor {
                    path,
                    replace,
                    query,
                }
                | LinkCompletionContext::AutolinkAnchor {
                    path,
                    replace,
                    query,
                } => {
                    let target_path = if path.is_empty() {
                        from.clone()
                    } else {
                        resolve_relative(&from, path)
                    };
                    if !self.documents.contains_key(&target_path) {
                        if let Ok(anchors) = store.anchors(&[]) {
                            candidates.extend(
                                anchors
                                    .into_iter()
                                    .filter(|stored| {
                                        stored.path == target_path
                                            && fuzzy_match(&stored.record.id.value, query)
                                    })
                                    .map(|stored| CompletionCandidate {
                                        label: format!("#{}", stored.record.id.value),
                                        detail: format!(
                                            "explicit anchor in {}",
                                            target_path.display()
                                        ),
                                        new_text: stored.record.id.value,
                                        replace: replace.clone(),
                                    }),
                            );
                        }
                    }
                }
            }
        }
        candidates.sort_by(|left, right| left.label.cmp(&right.label));
        candidates
    }

    pub fn complete_task_dependency(
        &self,
        from: impl AsRef<Path>,
        context: &TaskDependencyCompletionContext,
    ) -> Vec<CompletionCandidate> {
        let from = normalize(from.as_ref());
        let Some(owner) = self.current_output(&from).and_then(|output| {
            output
                .tasks
                .tasks
                .iter()
                .find(|task| task.range == context.task_range)
        }) else {
            return Vec::new();
        };
        let owner_ref = owner.id.as_ref().map(|id| TaskRef {
            path: from.clone(),
            id: id.value.clone(),
        });
        let existing = context
            .existing
            .iter()
            .filter_map(|target| match self.resolve_task_target(&from, target) {
                TaskTargetResolution::Task { target, .. } => Some(target),
                _ => None,
            })
            .collect::<HashSet<_>>();
        let eligible = |path: &Path, task: &TaskRecord| {
            let Some(id) = &task.id else {
                return false;
            };
            let target = TaskRef {
                path: path.to_path_buf(),
                id: id.value.clone(),
            };
            owner_ref.as_ref() != Some(&target) && !existing.contains(&target)
        };
        let Some((path_query, id_query)) = context.query.rsplit_once('#') else {
            let mut candidates = self
                .documents
                .values()
                .filter_map(|entry| {
                    let versioned = entry.current.as_ref().or(entry.last_valid.as_ref())?;
                    if !versioned
                        .output
                        .tasks
                        .tasks
                        .iter()
                        .any(|task| eligible(&entry.path, task))
                    {
                        return None;
                    }
                    let relative = relative_path(&from, &entry.path)?;
                    let reference = if entry.path == from {
                        "#".to_string()
                    } else {
                        format!("{relative}#")
                    };
                    if !fuzzy_match(reference.trim_end_matches('#'), &context.query) {
                        return None;
                    }
                    Some(CompletionCandidate {
                        label: reference.clone(),
                        detail: format!("task document ({relative})"),
                        new_text: reference,
                        replace: context.replace.clone(),
                    })
                })
                .collect::<Vec<_>>();
            if let Some(store) = &self.disk_store {
                let open = self.open_paths();
                if let Ok(documents) = store.documents() {
                    candidates.extend(documents.into_iter().filter_map(|document| {
                        if open.binary_search(&document.path).is_ok()
                            || !self
                                .tasks_for_path(&document.path)
                                .iter()
                                .any(|task| eligible(&document.path, task))
                        {
                            return None;
                        }
                        let relative = relative_path(&from, &document.path)?;
                        let reference = if document.path == from {
                            "#".to_string()
                        } else {
                            format!("{relative}#")
                        };
                        fuzzy_match(reference.trim_end_matches('#'), &context.query).then(|| {
                            CompletionCandidate {
                                label: reference.clone(),
                                detail: format!("task document ({relative})"),
                                new_text: reference,
                                replace: context.replace.clone(),
                            }
                        })
                    }));
                }
            }
            candidates.sort_by(|left, right| left.label.cmp(&right.label));
            return candidates;
        };

        let target_path = if path_query.is_empty() {
            from.clone()
        } else {
            normalize(
                &from
                    .parent()
                    .unwrap_or_else(|| Path::new(""))
                    .join(path_query),
            )
        };
        if !self.contains(&target_path) {
            return Vec::new();
        }
        let now = chrono::Local::now().fixed_offset();
        let id_replace_start = context.replace.start + path_query.len() + 1;
        let id_replace = id_replace_start..context.replace.end;
        let relative = relative_path(&from, &target_path).unwrap_or_else(|| path_query.to_string());
        let target_tasks = self.tasks_for_path(&target_path);
        let mut candidates = target_tasks
            .iter()
            .filter(|task| eligible(&target_path, task))
            .filter_map(|task| {
                let id = task.id.as_ref()?;
                if !fuzzy_match(&id.value, id_query) && !fuzzy_match(&task.title, id_query) {
                    return None;
                }
                let (state, _) = self.task_workflow_state(&target_path, task, now);
                let title = if task.title.is_empty() {
                    "Untitled task"
                } else {
                    &task.title
                };
                Some(CompletionCandidate {
                    label: id.value.clone(),
                    detail: format!(
                        "{}  {} ({relative})",
                        state.as_str().to_ascii_uppercase(),
                        title
                    ),
                    new_text: id.value.clone(),
                    replace: id_replace.clone(),
                })
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.label.cmp(&right.label));
        candidates
    }

    pub fn complete_image_path(
        &self,
        from: impl AsRef<Path>,
        context: &ImageCompletionContext,
    ) -> Vec<CompletionCandidate> {
        self.complete_resource_path(from.as_ref(), context, true)
    }

    pub fn complete_file_path(
        &self,
        from: impl AsRef<Path>,
        context: &FileCompletionContext,
    ) -> Vec<CompletionCandidate> {
        self.complete_resource_path(from.as_ref(), context, false)
    }

    fn complete_resource_path(
        &self,
        from: &Path,
        context: &ImageCompletionContext,
        images_only: bool,
    ) -> Vec<CompletionCandidate> {
        let from = normalize(from);
        if Path::new(&context.query).is_absolute() {
            return Vec::new();
        }
        let (directory_prefix, name_query) = context
            .query
            .rsplit_once('/')
            .map_or(("", context.query.as_str()), |(directory, name)| {
                (&context.query[..directory.len() + 1], name)
            });
        let directory = normalize(
            &from
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .join(directory_prefix),
        );
        let Ok(entries) = std::fs::read_dir(directory) else {
            return Vec::new();
        };
        let mut candidates = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_str()?.to_string();
                if !fuzzy_match(&name, name_query) {
                    return None;
                }
                let path = entry.path();
                let (suffix, detail) = if path.is_dir() {
                    (
                        "/",
                        if images_only {
                            "image directory"
                        } else {
                            "file directory"
                        },
                    )
                } else if path.is_file() && (!images_only || is_image_path(&path)) {
                    (
                        "",
                        if images_only {
                            "image file"
                        } else {
                            "file attachment"
                        },
                    )
                } else {
                    return None;
                };
                let path = format!("{directory_prefix}{name}{suffix}");
                if path
                    .chars()
                    .any(|character| character.is_control() || character == '\\')
                {
                    return None;
                }
                if !context.quoted && !valid_bare_attribute_value(&path) {
                    return None;
                }
                let new_text = if context.quoted {
                    escape_quoted_value(&path)
                } else {
                    path.clone()
                };
                Some(CompletionCandidate {
                    label: path,
                    detail: detail.to_string(),
                    new_text,
                    replace: context.replace.clone(),
                })
            })
            .collect::<Vec<_>>();
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

    fn anchors_named(&self, path: &Path, id: &str) -> Vec<AnchorRecord> {
        let path = normalize(path);
        if let Some(output) = self.current_output(&path) {
            return output
                .anchors
                .iter()
                .filter(|anchor| anchor.id.value == id)
                .cloned()
                .collect();
        }
        let stored = self
            .disk_store
            .as_ref()
            .and_then(|store| store.anchors_named(&path, id).ok())
            .unwrap_or_default();
        if !stored.is_empty() {
            return stored;
        }
        std::fs::read_to_string(&path)
            .ok()
            .map(parse)
            .filter(ParsedDocument::is_valid)
            .map(|parsed| analyze_document(&parsed.source, &parsed.syntax))
            .map(|output| {
                output
                    .anchors
                    .into_iter()
                    .filter(|anchor| anchor.id.value == id)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn tasks_for_path(&self, path: &Path) -> Vec<TaskRecord> {
        let path = normalize(path);
        if let Some(output) = self.current_output(&path) {
            return output.tasks.tasks.clone();
        }
        let stored = self
            .disk_store
            .as_ref()
            .and_then(|store| store.tasks(&[]).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|stored| stored.path == path)
            .map(|stored| stored.record)
            .collect::<Vec<_>>();
        if !stored.is_empty() {
            return stored;
        }
        std::fs::read_to_string(&path)
            .ok()
            .map(parse)
            .filter(ParsedDocument::is_valid)
            .map(|parsed| analyze_document(&parsed.source, &parsed.syntax).tasks.tasks)
            .unwrap_or_default()
    }

    fn all_tasks(&self) -> Vec<(PathBuf, TaskRecord)> {
        let mut tasks = self
            .documents
            .values()
            .filter_map(|entry| entry.current.as_ref().map(|current| (entry, current)))
            .flat_map(|(entry, current)| {
                current
                    .output
                    .tasks
                    .tasks
                    .iter()
                    .cloned()
                    .map(|task| (entry.path.clone(), task))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        if let Some(store) = &self.disk_store {
            if let Ok(stored) = store.tasks(&self.open_paths()) {
                tasks.extend(
                    stored
                        .into_iter()
                        .map(|stored| (stored.path, stored.record)),
                );
            }
        }
        tasks.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then(left.1.range.start.cmp(&right.1.range.start))
        });
        tasks
    }

    fn entry_for_operation(&self, path: &Path) -> Option<DocumentEntry> {
        let path = normalize(path);
        if let Some(entry) = self.documents.get(&path) {
            return Some(entry.clone());
        }
        let revision = self
            .disk_store
            .as_ref()?
            .documents()
            .ok()?
            .into_iter()
            .find(|document| document.path == path)?
            .revision;
        let parsed = parse(std::fs::read_to_string(&path).ok()?);
        let current = parsed.is_valid().then(|| {
            Arc::new(VersionedDocumentOutput {
                revision,
                output: analyze_document(&parsed.source, &parsed.syntax),
            })
        });
        Some(DocumentEntry {
            path,
            revision,
            parsed,
            current: current.clone(),
            last_valid: current,
        })
    }

    fn entries_for_operation(&self) -> Vec<DocumentEntry> {
        let open = self.open_paths();
        let mut entries = self.documents.values().cloned().collect::<Vec<_>>();
        if let Some(store) = &self.disk_store {
            if let Ok(documents) = store.documents() {
                entries.extend(
                    documents
                        .into_iter()
                        .filter(|document| open.binary_search(&document.path).is_err())
                        .filter_map(|document| self.entry_for_operation(&document.path)),
                );
            }
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        entries
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
                .is_some_and(|mark| matches!(mark.marker.as_str(), "-" | "." | "task" | "event"))
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

fn parsed_block_with_range<'a>(
    blocks: &'a [Block],
    range: &std::ops::Range<usize>,
) -> Option<&'a ParsedBlock> {
    for block in blocks {
        let Block::Parsed(block) = block else {
            continue;
        };
        if &block.range == range {
            return Some(block);
        }
        if block.range.start <= range.start && range.end <= block.range.end {
            if let Some(found) = parsed_block_with_range(&block.children, range) {
                return Some(found);
            }
        }
    }
    None
}

fn parsed_block(block: &Block) -> Option<&ParsedBlock> {
    match block {
        Block::Parsed(block) => Some(block),
        Block::Verbatim(_) => None,
    }
}

fn next_parsed_sibling<'a>(
    blocks: &'a [Block],
    target: &std::ops::Range<usize>,
) -> Option<&'a ParsedBlock> {
    for (index, block) in blocks.iter().enumerate() {
        let Block::Parsed(block) = block else {
            continue;
        };
        if &block.range == target {
            return blocks.get(index + 1).and_then(parsed_block);
        }
        if block.range.start <= target.start && target.end <= block.range.end {
            return next_parsed_sibling(&block.children, target);
        }
    }
    None
}

fn direct_parent_range(
    blocks: &[Block],
    child_range: &std::ops::Range<usize>,
) -> Option<std::ops::Range<usize>> {
    for block in blocks {
        let Block::Parsed(block) = block else {
            continue;
        };
        if block
            .children
            .iter()
            .any(|child| child.range() == child_range)
        {
            return Some(block.range.clone());
        }
        if block.range.start <= child_range.start && child_range.end <= block.range.end {
            if let Some(parent) = direct_parent_range(&block.children, child_range) {
                return Some(parent);
            }
        }
    }
    None
}

struct BlockIdTarget<'a> {
    block_range: std::ops::Range<usize>,
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
                            block_range: block.range.clone(),
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
                        block_range: block.range.clone(),
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
    single_document_edits(entry, path, vec![edit])
}

fn single_document_edits(
    entry: &DocumentEntry,
    path: PathBuf,
    edits: Vec<TextEdit>,
) -> WorkspaceEdit {
    WorkspaceEdit {
        document_changes: vec![DocumentEdit {
            path,
            expected_revision: entry.revision,
            edits,
        }],
        resource_operations: Vec::new(),
    }
}

fn validate_event_input(input: &EventInput) -> Result<(), EventEditError> {
    let at = input
        .at
        .as_deref()
        .map(DateTime::parse_from_rfc3339)
        .transpose()
        .map_err(|_| EventEditError::InvalidDatetime)?;
    let start = input
        .start
        .as_deref()
        .map(DateTime::parse_from_rfc3339)
        .transpose()
        .map_err(|_| EventEditError::InvalidDatetime)?;
    if at.is_none() && start.is_none() {
        return Err(EventEditError::InvalidTimeShape);
    }
    if at.is_some() && (start.is_some() || input.end.is_some()) {
        return Err(EventEditError::InvalidTimeShape);
    }
    if let Some(end) = &input.end {
        let end = DateTime::parse_from_rfc3339(end).map_err(|_| EventEditError::InvalidDatetime)?;
        let Some(start) = start else {
            return Err(EventEditError::InvalidTimeShape);
        };
        if end <= start {
            return Err(EventEditError::InvalidInterval);
        }
        if end.offset() != start.offset() {
            return Err(EventEditError::InvalidInterval);
        }
    } else if start.is_some() {
        return Err(EventEditError::InvalidTimeShape);
    }
    Ok(())
}

#[cfg(test)]
fn parse_event_shorthand(
    input: &str,
    now: DateTime<FixedOffset>,
) -> Result<EventInput, EventShorthandError> {
    parse_event_shorthand_with_title_start(input, now, None, None).map(|(input, _)| input)
}

#[derive(Clone, Copy)]
struct ShorthandStart {
    datetime: DateTime<FixedOffset>,
    explicit_date: bool,
}

fn parse_event_shorthand_with_title_start(
    input: &str,
    now: DateTime<FixedOffset>,
    metadata: Option<&MetadataOutput>,
    inferred_end: Option<ShorthandStart>,
) -> Result<(EventInput, usize), EventShorthandError> {
    let separator = input
        .char_indices()
        .find_map(|(index, character)| matches!(character, ' ' | '\t').then_some(index))
        .ok_or(EventShorthandError::InvalidShorthand)?;
    let schedule = &input[..separator];
    let untrimmed_title = &input[separator..];
    let title = untrimmed_title.trim_start();
    if title.is_empty() {
        return Err(EventShorthandError::InvalidShorthand);
    }
    let title_start = input.len() - title.len();
    let (start, end) = match schedule.split_once("--") {
        Some((start, end)) if !start.is_empty() && !end.contains("--") => (start, Some(end)),
        Some(_) => return Err(EventShorthandError::InvalidShorthand),
        None => (schedule, None),
    };
    let (date, offset) = shorthand_context(now, metadata);
    let start = parse_shorthand_start(start, date, offset)?;
    if let Some(end) = end {
        let end = if end.is_empty() {
            let inferred = inferred_end.ok_or(EventShorthandError::InvalidShorthand)?;
            if inferred.datetime.offset() != start.datetime.offset() {
                return Err(EventShorthandError::InvalidInterval);
            }
            if !inferred.explicit_date && inferred.datetime.time() < start.datetime.time() {
                shorthand_datetime(
                    start
                        .datetime
                        .date_naive()
                        .succ_opt()
                        .ok_or(EventShorthandError::InvalidInterval)?,
                    inferred.datetime.time(),
                    *start.datetime.offset(),
                )?
            } else {
                inferred.datetime
            }
        } else {
            if let Some((date, time)) = end.split_once('T') {
                let date = NaiveDate::parse_from_str(date, "%Y-%m-%d")
                    .map_err(|_| EventShorthandError::InvalidShorthand)?;
                let time = parse_shorthand_time(time)?;
                shorthand_datetime(date, time, *start.datetime.offset())?
            } else {
                let end_time = parse_shorthand_time(end)?;
                let end_date = if end_time < start.datetime.time() {
                    start
                        .datetime
                        .date_naive()
                        .succ_opt()
                        .ok_or(EventShorthandError::InvalidInterval)?
                } else {
                    start.datetime.date_naive()
                };
                shorthand_datetime(end_date, end_time, *start.datetime.offset())?
            }
        };
        if end <= start.datetime {
            return Err(EventShorthandError::InvalidInterval);
        }
        Ok((
            EventInput {
                title: title.trim_end().to_string(),
                at: None,
                start: Some(event_datetime(start.datetime)),
                end: Some(event_datetime(end)),
                tasks: Vec::new(),
            },
            title_start,
        ))
    } else {
        Ok((
            EventInput {
                title: title.trim_end().to_string(),
                at: Some(event_datetime(start.datetime)),
                start: None,
                end: None,
                tasks: Vec::new(),
            },
            title_start,
        ))
    }
}

fn parse_event_shorthand_head(
    source: &str,
    syntax: &ParsedBlock,
    now: DateTime<FixedOffset>,
    metadata: &MetadataOutput,
    inferred_end: Option<ShorthandStart>,
) -> Result<(EventInput, usize), EventShorthandError> {
    let shorthand = &source[syntax.head.range.clone()];
    let (mut input, title_start) =
        parse_event_shorthand_with_title_start(shorthand, now, Some(metadata), inferred_end)?;
    let plain = syntax.head.plain_text();
    let title = plain
        .get(title_start..)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .ok_or(EventShorthandError::InvalidShorthand)?;
    input.title = title.to_string();
    Ok((input, title_start))
}

fn inferred_end_from_sibling(
    source: &str,
    sibling: &ParsedBlock,
    now: DateTime<FixedOffset>,
    metadata: &MetadataOutput,
) -> Option<ShorthandStart> {
    let mark = sibling.mark.as_ref()?;
    if !matches!(mark.marker.as_str(), "-" | "." | "task" | "event") || mark.marker == "event" {
        return None;
    }
    let shorthand = &source[sibling.head.range.clone()];
    let separator = shorthand
        .char_indices()
        .find_map(|(index, character)| matches!(character, ' ' | '\t').then_some(index))?;
    if shorthand[separator..].trim().is_empty() {
        return None;
    }
    let schedule = &shorthand[..separator];
    let start = match schedule.split_once("--") {
        Some((start, end))
            if !start.is_empty()
                && !end.contains("--")
                && (end.is_empty()
                    || (!end.contains('T') && parse_shorthand_time(end).is_ok())) =>
        {
            start
        }
        Some(_) => return None,
        None => schedule,
    };
    let (date, offset) = shorthand_context(now, Some(metadata));
    parse_shorthand_start(start, date, offset).ok()
}

fn shorthand_context(
    now: DateTime<FixedOffset>,
    metadata: Option<&MetadataOutput>,
) -> (NaiveDate, FixedOffset) {
    let mut date = now.date_naive();
    let mut offset = *now.offset();
    if let Some(value) = metadata.and_then(|metadata| metadata_scalar(metadata, "date")) {
        if let Ok(inherited) = NaiveDate::parse_from_str(&value, "%Y-%m-%d") {
            date = inherited;
        } else if let Ok(inherited) = DateTime::parse_from_rfc3339(&value) {
            date = inherited.date_naive();
            offset = *inherited.offset();
        }
    }
    if let Some(inherited) = metadata
        .and_then(|metadata| metadata_scalar(metadata, "timezone"))
        .and_then(|timezone| {
            DateTime::parse_from_rfc3339(&format!("2000-01-01T00:00:00{timezone}"))
                .ok()
                .map(|datetime| *datetime.offset())
        })
    {
        offset = inherited;
    }
    (date, offset)
}

fn strip_event_shorthand_prefix(
    owned: &mut OwnedBlock,
    _title_start: usize,
) -> Result<(), EventShorthandError> {
    let OwnedBlock::Parsed { head, .. } = owned else {
        return Err(EventShorthandError::GeneratedInvalid);
    };
    if !matches!(head.first(), Some(OwnedInline::Text(_)))
        || !matches!(head.get(1), Some(OwnedInline::Space(_)))
    {
        return Err(EventShorthandError::InvalidShorthand);
    }
    head.drain(..2);
    Ok(())
}

fn parse_shorthand_start(
    input: &str,
    default_date: NaiveDate,
    offset: FixedOffset,
) -> Result<ShorthandStart, EventShorthandError> {
    let (date, time, explicit_date) = if let Some((date, time)) = input.split_once('T') {
        if time.contains('T') {
            return Err(EventShorthandError::InvalidShorthand);
        }
        let date = NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map_err(|_| EventShorthandError::InvalidShorthand)?;
        (date, time, true)
    } else {
        (default_date, input, false)
    };
    Ok(ShorthandStart {
        datetime: shorthand_datetime(date, parse_shorthand_time(time)?, offset)?,
        explicit_date,
    })
}

fn parse_shorthand_time(input: &str) -> Result<NaiveTime, EventShorthandError> {
    let parts = input.split(':').collect::<Vec<_>>();
    if !(1..=3).contains(&parts.len())
        || parts
            .iter()
            .any(|part| part.len() != 2 || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(EventShorthandError::InvalidShorthand);
    }
    let hour = parts[0]
        .parse::<u32>()
        .map_err(|_| EventShorthandError::InvalidShorthand)?;
    let minute = parts
        .get(1)
        .map_or(Ok(0), |part| part.parse::<u32>())
        .map_err(|_| EventShorthandError::InvalidShorthand)?;
    let second = parts
        .get(2)
        .map_or(Ok(0), |part| part.parse::<u32>())
        .map_err(|_| EventShorthandError::InvalidShorthand)?;
    NaiveTime::from_hms_opt(hour, minute, second).ok_or(EventShorthandError::InvalidShorthand)
}

fn shorthand_datetime(
    date: NaiveDate,
    time: NaiveTime,
    offset: FixedOffset,
) -> Result<DateTime<FixedOffset>, EventShorthandError> {
    offset
        .from_local_datetime(&date.and_time(time))
        .single()
        .ok_or(EventShorthandError::InvalidShorthand)
}

fn event_datetime(datetime: DateTime<FixedOffset>) -> String {
    datetime.to_rfc3339_opts(SecondsFormat::Secs, false)
}

fn validate_task_authoring_input(
    input: &TaskAuthoringInput,
    timestamp: &str,
) -> Result<(), TaskAuthoringError> {
    for value in [
        Some(timestamp),
        input.created.as_deref(),
        input.due.as_deref(),
        input.wait.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        DateTime::parse_from_rfc3339(value).map_err(|_| TaskAuthoringError::InvalidDatetime)?;
    }
    if input.recur.as_deref().is_some_and(|value| {
        let Some(number) = value
            .strip_prefix('P')
            .and_then(|value| value.get(..value.len().saturating_sub(1)))
        else {
            return true;
        };
        !matches!(value.chars().last(), Some('D' | 'W' | 'M' | 'Y'))
            || number.parse::<u64>().ok().is_none_or(|number| number == 0)
    }) {
        return Err(TaskAuthoringError::InvalidRecurrence);
    }
    if input.recur.is_some() && input.due.is_none() {
        return Err(TaskAuthoringError::InvalidRecurrence);
    }
    for reference in input.prev.iter().chain(&input.depends) {
        if matches!(
            parse_task_reference_target(reference),
            TaskReferenceTarget::Invalid
        ) {
            return Err(TaskAuthoringError::InvalidReference);
        }
    }
    Ok(())
}

fn owned_authored_task(input: &TaskAuthoringInput, id: &str, timestamp: &str) -> OwnedBlock {
    let mut attributes = vec![OwnedAttribute::id(id)];
    append_authored_task_fields(&mut attributes, input, timestamp);
    OwnedBlock::marked("task", &input.title).with_attributes(attributes)
}

fn append_authored_task_fields(
    attributes: &mut Vec<OwnedAttribute>,
    input: &TaskAuthoringInput,
    default_created: &str,
) {
    attributes.push(OwnedAttribute::quoted(
        "created",
        input.created.as_deref().unwrap_or(default_created),
    ));
    for (key, value) in [
        ("due", input.due.as_deref()),
        ("wait", input.wait.as_deref()),
        ("recur", input.recur.as_deref()),
        ("prev", input.prev.as_deref()),
    ] {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            attributes.push(OwnedAttribute::quoted(key, value));
        }
    }
    if !input.depends.is_empty() {
        attributes.push(OwnedAttribute::quoted("depends", input.depends.join(" ")));
    }
    if let Some(priority) = input.priority {
        attributes.push(OwnedAttribute::bare("priority", priority.to_string()));
    }
}

fn child_insertion_index(
    children: &[Block],
    after: Option<&std::ops::Range<usize>>,
) -> Result<usize, TaskAuthoringError> {
    let Some(after) = after else {
        return Ok(children.len());
    };
    children
        .iter()
        .position(|child| child.range() == after)
        .map(|index| index + 1)
        .ok_or(TaskAuthoringError::InvalidPlacement)
}

fn remove_owned_descendant(
    syntax: &ParsedBlock,
    owned: &mut OwnedBlock,
    target: &std::ops::Range<usize>,
) -> bool {
    let Some(children) = owned.children_mut() else {
        return false;
    };
    if let Some(index) = syntax
        .children
        .iter()
        .position(|child| child.range() == target)
    {
        children.remove(index);
        return true;
    }
    for (index, syntax_child) in syntax.children.iter().enumerate() {
        let Block::Parsed(syntax_child) = syntax_child else {
            continue;
        };
        if syntax_child.range.start <= target.start
            && target.end <= syntax_child.range.end
            && remove_owned_descendant(syntax_child, &mut children[index], target)
        {
            return true;
        }
    }
    false
}

fn event_attributes(input: &EventInput, metadata: &MetadataOutput) -> Vec<OwnedAttribute> {
    let mut attributes = Vec::new();
    let (date, timezone, _) =
        compact_event_schedule(input).expect("event input is validated before authoring");
    if metadata_scalar(metadata, "date").as_deref() != Some(&date) {
        attributes.push(OwnedAttribute::bare("date", date));
    }
    if metadata_scalar(metadata, "timezone").as_deref() != Some(&timezone) {
        attributes.push(OwnedAttribute::quoted("timezone", timezone));
    }
    if !input.tasks.is_empty() {
        attributes.push(OwnedAttribute::quoted("tasks", input.tasks.join(" ")));
    }
    attributes
}

fn set_event_head(owned: &mut OwnedBlock, input: &EventInput) {
    let (_, _, schedule) =
        compact_event_schedule(input).expect("event input is validated before authoring");
    let OwnedBlock::Parsed { head, .. } = owned else {
        return;
    };
    *head = vec![
        OwnedInline::Text(schedule),
        OwnedInline::Space(" ".to_string()),
        OwnedInline::Text(input.title.clone()),
    ];
}

fn prepend_event_schedule(owned: &mut OwnedBlock, input: &EventInput) {
    let (_, _, schedule) =
        compact_event_schedule(input).expect("event input is validated before authoring");
    let OwnedBlock::Parsed { head, .. } = owned else {
        return;
    };
    head.splice(
        0..0,
        [
            OwnedInline::Text(schedule),
            OwnedInline::Space(" ".to_string()),
        ],
    );
}

fn owned_event(input: &EventInput, metadata: &MetadataOutput) -> OwnedBlock {
    let attributes = event_attributes(input, metadata);
    let mut event = OwnedBlock::marked("event", "");
    if !attributes.is_empty() {
        event = event.with_attributes(attributes);
    }
    set_event_head(&mut event, input);
    event
}

fn convert_shorthands_in_block(
    source: &str,
    syntax: &ParsedBlock,
    next_sibling: Option<&ParsedBlock>,
    owned: &mut OwnedBlock,
    selection: &std::ops::Range<usize>,
    now: DateTime<FixedOffset>,
    metadata: &MetadataOutput,
) -> usize {
    let mut converted = 0;
    if syntax.head.range.start < selection.end
        && selection.start < syntax.head.range.end
        && syntax
            .mark
            .as_ref()
            .is_some_and(|mark| matches!(mark.marker.as_str(), "-" | "."))
    {
        let inferred_end = next_sibling
            .filter(|next| {
                next.head.range.start < selection.end && selection.start < next.head.range.end
            })
            .and_then(|next| inferred_end_from_sibling(source, next, now, metadata));
        if let Ok((input, title_start)) =
            parse_event_shorthand_head(source, syntax, now, metadata, inferred_end)
        {
            if strip_event_shorthand_prefix(owned, title_start).is_err() {
                return converted;
            }
            let attributes = event_attributes(&input, metadata);
            if !attributes.is_empty() {
                owned.attributes_mut().extend(attributes);
            }
            owned.set_marker("event");
            prepend_event_schedule(owned, &input);
            converted += 1;
        }
    }
    let OwnedBlock::Parsed { children, .. } = owned else {
        return converted;
    };
    for (index, (syntax_child, owned_child)) in syntax.children.iter().zip(children).enumerate() {
        let Block::Parsed(syntax_child) = syntax_child else {
            continue;
        };
        let next_sibling = syntax.children.get(index + 1).and_then(parsed_block);
        converted += convert_shorthands_in_block(
            source,
            syntax_child,
            next_sibling,
            owned_child,
            selection,
            now,
            metadata,
        );
    }
    converted
}

fn compact_event_schedule(input: &EventInput) -> Result<(String, String, String), EventEditError> {
    let (start, end) = if let Some(at) = &input.at {
        (
            DateTime::parse_from_rfc3339(at).map_err(|_| EventEditError::InvalidDatetime)?,
            None,
        )
    } else {
        let start = input
            .start
            .as_deref()
            .ok_or(EventEditError::InvalidTimeShape)?;
        let start =
            DateTime::parse_from_rfc3339(start).map_err(|_| EventEditError::InvalidDatetime)?;
        let end = input
            .end
            .as_deref()
            .ok_or(EventEditError::InvalidTimeShape)?;
        let end = DateTime::parse_from_rfc3339(end).map_err(|_| EventEditError::InvalidDatetime)?;
        (start, Some(end))
    };
    let time = |value: DateTime<FixedOffset>| {
        if value.second() == 0 {
            value.format("%H:%M").to_string()
        } else {
            value.format("%H:%M:%S").to_string()
        }
    };
    let when = end.map_or_else(
        || time(start),
        |end| {
            let next_day_rollover = start.date_naive().succ_opt() == Some(end.date_naive())
                && end.time() < start.time();
            let end = if end.date_naive() == start.date_naive() || next_day_rollover {
                time(end)
            } else {
                format!("{}T{}", end.date_naive().format("%Y-%m-%d"), time(end))
            };
            format!("{}--{end}", time(start))
        },
    );
    Ok((
        start.date_naive().format("%Y-%m-%d").to_string(),
        start.offset().to_string(),
        when,
    ))
}

fn metadata_scalar(metadata: &MetadataOutput, key: &str) -> Option<String> {
    let entry = metadata
        .metadata
        .as_ref()?
        .entries
        .iter()
        .find(|entry| entry.key == key)?;
    match &entry.value {
        MetadataValue::Scalar { content, .. } => Some(content.plain_text()),
        MetadataValue::Verbatim { text, .. } => Some(text.clone()),
        _ => None,
    }
}

fn validated_token_edit(
    entry: &DocumentEntry,
    range: std::ops::Range<usize>,
    replacement: impl Into<String>,
) -> Result<TextEdit, RenameError> {
    TextEdit::replace(&entry.parsed, range, replacement)
        .map_err(|_| RenameError::StaleOrInvalidDocument)
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

struct RecurringTaskCloneContext<'a> {
    tasks: &'a [TaskRecord],
    root: &'a TaskRecord,
    next_id: &'a str,
    timestamp: &'a str,
    next_due: &'a str,
    next_wait: Option<&'a str>,
    recur: &'a str,
    current_id: &'a str,
}

fn prepare_recurring_task_clone(
    owned: &mut OwnedBlock,
    block: &ParsedBlock,
    context: &RecurringTaskCloneContext<'_>,
) {
    if let Some(task) = context.tasks.iter().find(|task| task.range == block.range) {
        owned.attributes_mut().retain(persistent_task_attribute);
        if task.range == context.root.range {
            let attributes = owned.attributes_mut();
            attributes.push(OwnedAttribute::id(context.next_id));
            attributes.push(OwnedAttribute::quoted("created", context.timestamp));
            attributes.push(OwnedAttribute::quoted("due", context.next_due));
            if let Some(wait) = context.next_wait {
                attributes.push(OwnedAttribute::quoted("wait", wait));
            }
            attributes.push(OwnedAttribute::quoted("recur", context.recur));
            attributes.push(OwnedAttribute::quoted(
                "prev",
                format!("#{}", context.current_id),
            ));
        }
    }

    let OwnedBlock::Parsed { children, .. } = owned else {
        return;
    };
    for (owned_child, syntax_child) in children.iter_mut().zip(&block.children) {
        let Block::Parsed(syntax_child) = syntax_child else {
            continue;
        };
        prepare_recurring_task_clone(owned_child, syntax_child, context);
    }
}

fn persistent_task_attribute(attribute: &OwnedAttribute) -> bool {
    match attribute {
        OwnedAttribute::Id(_) => false,
        OwnedAttribute::Class(_) => true,
        OwnedAttribute::Pair { key, .. } => !matches!(
            key.as_str(),
            "created" | "due" | "wait" | "done" | "canceled" | "recur" | "prev"
        ),
    }
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

fn valid_autolink_completion_path(path: &str) -> bool {
    !path.contains('#')
        && !path
            .chars()
            .any(|character| character.is_control() || character == '\\')
        && !path
            .chars()
            .any(|character| character.is_whitespace() && character != ' ')
}

fn link_path_rename_edit(
    entry: &DocumentEntry,
    link: &LinkRecord,
    path_range: &std::ops::Range<usize>,
    replacement: String,
) -> Result<TextEdit, RenameError> {
    let LinkSpelling::Verbatim {
        envelope,
        quote_count,
    } = &link.spelling
    else {
        return validated_token_edit(entry, path_range.clone(), replacement);
    };
    let suffix_start = path_range.end - link.target.range.start;
    let payload = format!("{replacement}{}", &link.target.value[suffix_start..]);
    if verbatim_payload_is_safe(&payload, *quote_count) {
        validated_token_edit(entry, path_range.clone(), replacement)
    } else {
        validated_token_edit(entry, envelope.clone(), format_inline_verbatim(&payload))
    }
}

fn verbatim_payload_is_safe(payload: &str, quote_count: usize) -> bool {
    !payload.contains(&format!("]{}", "\"".repeat(quote_count)))
}

fn format_inline_verbatim(payload: &str) -> String {
    let quote_count = (0..)
        .find(|quote_count| verbatim_payload_is_safe(payload, *quote_count))
        .expect("a finite payload always has a safe verbatim delimiter");
    let quotes = "\"".repeat(quote_count);
    format!("`{quotes}[{payload}]{quotes}")
}

fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "gif" | "svg" | "avif"
            )
        })
}

fn contains_inclusive(range: &std::ops::Range<usize>, offset: usize) -> bool {
    range.start <= offset && offset <= range.end
}

fn contains_component(range: &std::ops::Range<usize>, offset: usize) -> bool {
    range.start <= offset && offset < range.end
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
        | ResolvedTarget::File { .. }
        | ResolvedTarget::UnresolvedFile { .. }
        | ResolvedTarget::Other
        | ResolvedTarget::UnresolvedPath { .. } => None,
    }
}

fn collect_reverse_reference(
    references: &mut DocumentReverseReferences,
    target_path: &Path,
    target_ids: &HashSet<String>,
    source_path: &Path,
    source_range: std::ops::Range<usize>,
    resolved: ResolvedTarget,
) {
    if resolved_document_path(resolved.clone()).as_deref() == Some(target_path) {
        references.document.push(ReferenceOccurrence {
            source_path: source_path.to_path_buf(),
            source_range: source_range.clone(),
        });
    }
    let ResolvedTarget::Anchor { path, id, .. } = resolved else {
        return;
    };
    if path == target_path && target_ids.contains(&id) {
        references
            .anchors
            .entry(id)
            .or_default()
            .push(ReferenceOccurrence {
                source_path: source_path.to_path_buf(),
                source_range,
            });
    }
}

fn reference_occurrence_order(
    left: &ReferenceOccurrence,
    right: &ReferenceOccurrence,
) -> std::cmp::Ordering {
    left.source_path
        .cmp(&right.source_path)
        .then(left.source_range.start.cmp(&right.source_range.start))
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

fn escape_parsed_text(value: &str) -> String {
    value.replace('`', "``").replace(']', "`]")
}

fn escape_attached_text(value: &str) -> String {
    escape_parsed_text(value)
        .replace('{', "`{")
        .replace('}', "`}")
}

fn escape_quoted_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn valid_bare_attribute_value(value: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    #[test]
    fn sqlite_disk_documents_are_shadowed_by_complete_open_snapshots() {
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let mut workspace = Workspace::with_sqlite_store(store);
        let target = "`task Target {\n `@ target\n}\n";
        let disk_source = "See `->[target]{`:[to target.plumb#target]}.\n";
        assert!(!workspace.insert_disk("target.plumb", 0, target).unwrap());
        assert!(!workspace
            .insert_disk("source.plumb", 0, disk_source)
            .unwrap());
        assert!(workspace.documents().next().is_none());
        assert_eq!(
            workspace
                .reverse_references_for_document(
                    "target.plumb",
                    &HashSet::from(["target".to_string()])
                )
                .anchors["target"]
                .len(),
            1
        );

        workspace.open_document("source.plumb", 1, "No reference.\n");
        assert!(workspace
            .reverse_references_for_document("target.plumb", &HashSet::from(["target".to_string()]))
            .anchors
            .get("target")
            .is_none_or(Vec::is_empty));

        workspace.close_document("source.plumb");
        assert_eq!(
            workspace
                .reverse_references_for_document(
                    "target.plumb",
                    &HashSet::from(["target".to_string()])
                )
                .anchors["target"]
                .len(),
            1
        );
    }

    #[test]
    fn sqlite_warm_insert_skips_document_analysis() {
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let mut workspace = Workspace::with_sqlite_store(store);
        let source = "`event 2026-08-11T10:00 Cached\n";
        assert!(!workspace.insert_disk("event.plumb", 0, source).unwrap());
        assert!(workspace.insert_disk("event.plumb", 0, source).unwrap());
        assert_eq!(workspace.document_paths(), [PathBuf::from("event.plumb")]);
    }

    fn temp_workspace() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "plumb-workspace-scan-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn applies_one_guarded_document_edit_with_revision_validation() {
        let edit = WorkspaceEdit {
            document_changes: vec![DocumentEdit {
                path: PathBuf::from("note.plumb"),
                expected_revision: 7,
                edits: vec![TextEdit {
                    range: 0..4,
                    new_text: "Task".to_string(),
                }],
            }],
            resource_operations: Vec::new(),
        };
        assert_eq!(
            apply_document_edit("Note\n".to_string(), "note.plumb", 7, edit.clone()),
            Ok("Task\n".to_string())
        );
        assert_eq!(
            apply_document_edit("Note\n".to_string(), "note.plumb", 8, edit.clone()),
            Err(ApplyDocumentEditError::RevisionMismatch)
        );
        assert_eq!(
            apply_document_edit("Note\n".to_string(), "other.plumb", 7, edit),
            Err(ApplyDocumentEditError::DocumentNotEdited)
        );
    }

    #[test]
    fn discovers_the_nearest_plumb_workspace_marker() {
        let root = temp_workspace();
        let nested = root.join("notes/private/deep");
        std::fs::create_dir_all(root.join(".plumb")).unwrap();
        std::fs::create_dir_all(root.join("notes/private/.plumb")).unwrap();
        std::fs::create_dir_all(&nested).unwrap();

        assert_eq!(
            discover_workspace_root(&nested),
            normalize(&root.join("notes/private"))
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_relative_explicit_workspace_roots_from_the_current_directory() {
        let root = temp_workspace();
        std::fs::create_dir_all(&root).unwrap();

        assert_eq!(
            resolve_workspace_root_from(Some(Path::new(".")), &root),
            normalize(&root)
        );
        assert_eq!(
            resolve_workspace_root_from(Some(Path::new("notes")), &root),
            normalize(&root.join("notes"))
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scans_dot_directories_and_applies_only_workspace_ignore_files() {
        let parent = temp_workspace();
        let root = parent.join("workspace");
        std::fs::create_dir_all(root.join(".plumb")).unwrap();
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        std::fs::create_dir_all(root.join("private")).unwrap();
        std::fs::write(parent.join(".ignore"), "workspace/\n").unwrap();
        std::fs::write(root.join(".ignore"), "private/\n").unwrap();
        std::fs::write(root.join("visible.plumb"), "Visible\n").unwrap();
        std::fs::write(root.join(".hidden/note.plumb"), "Hidden\n").unwrap();
        std::fs::write(root.join("private/note.plumb"), "Private\n").unwrap();

        let scan = scan_workspace_files(&root);
        assert!(scan.is_complete(), "{:?}", scan.errors);
        assert_eq!(
            scan.files,
            vec![
                normalize(&root.join(".hidden/note.plumb")),
                normalize(&root.join("visible.plumb")),
            ]
        );

        std::fs::remove_dir_all(parent).unwrap();
    }

    fn apply_single_edit(source: &str, operation: &WorkspaceEdit) -> String {
        assert_eq!(operation.document_changes.len(), 1);
        assert_eq!(operation.document_changes[0].edits.len(), 1);
        let edit = &operation.document_changes[0].edits[0];
        let mut edited = source.to_string();
        edited.replace_range(edit.range.clone(), &edit.new_text);
        edited
    }

    #[test]
    fn resolves_same_and_cross_file_explicit_anchors() {
        let mut workspace = Workspace::new();
        workspace.insert("notes/a note.plumb", 1, "`# Local {\n  `@ local\n}\n");
        workspace.insert("notes/a%20note.plumb", 1, "`# Literal {\n  `@ literal\n}\n");
        workspace.insert(
            "notes/b.plumb",
            1,
            "See `->[local]{`:[to a note.plumb#local]}.\nSee `->[literal]{`:[to a%20note.plumb#literal]}.\n",
        );
        let links = &workspace
            .get("notes/b.plumb")
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .output
            .links;
        for (link, expected_path, expected_id) in [
            (&links[0], "notes/a note.plumb", "local"),
            (&links[1], "notes/a%20note.plumb", "literal"),
        ] {
            assert!(matches!(
                workspace.resolve_link("notes/b.plumb", link),
                ResolvedTarget::Anchor { ref path, ref id, .. }
                    if path == Path::new(expected_path) && id == expected_id
            ));
        }
    }

    #[test]
    fn headings_without_ids_do_not_resolve() {
        let mut workspace = Workspace::new();
        workspace.insert(
            "a.plumb",
            1,
            "`# No anchor\n\nSee `->[x]{`:[to #No-anchor]}.\n",
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
        workspace.insert("a.plumb", 1, "`# Valid {\n  `@ ok\n}\n");
        let valid = workspace.get("a.plumb").unwrap();
        assert!(Arc::ptr_eq(
            valid.current.as_ref().unwrap(),
            valid.last_valid.as_ref().unwrap()
        ));
        workspace.insert("a.plumb", 2, "`node{key=a key=b} Invalid\n");
        let entry = workspace.get("a.plumb").unwrap();
        assert!(entry.current.is_none());
        assert_eq!(entry.last_valid.as_ref().unwrap().revision, 1);
        assert!(workspace.anchor_at("a.plumb", 0).is_none());
    }

    #[test]
    fn rebinds_identical_source_without_rebuilding_document_outputs() {
        let source = "`event 2026-08-11T09:00:00+08:00 Meeting\n";
        let mut workspace = Workspace::new();
        workspace.insert("event.plumb", 7, source);
        let entry = workspace.get("event.plumb").unwrap();
        let token_storage = entry.parsed.lossless.tokens.as_ptr();
        let event_storage = entry
            .current
            .as_ref()
            .unwrap()
            .output
            .events
            .events
            .as_ptr();

        assert!(workspace.rebind_revision_if_source("event.plumb", 0, source));
        let entry = workspace.get("event.plumb").unwrap();
        assert_eq!(entry.revision, 0);
        assert_eq!(entry.current.as_ref().unwrap().revision, 0);
        assert_eq!(entry.last_valid.as_ref().unwrap().revision, 0);
        assert_eq!(entry.parsed.lossless.tokens.as_ptr(), token_storage);
        assert_eq!(
            entry
                .current
                .as_ref()
                .unwrap()
                .output
                .events
                .events
                .as_ptr(),
            event_storage
        );
        assert!(!workspace.rebind_revision_if_source("event.plumb", 1, "changed\n"));
    }

    #[test]
    fn rebinding_invalid_source_preserves_last_valid_provenance() {
        let mut workspace = Workspace::new();
        workspace.insert("note.plumb", 1, "Valid\n");
        let invalid = "`broken[\n";
        workspace.insert("note.plumb", 2, invalid);

        assert!(workspace.rebind_revision_if_source("note.plumb", 0, invalid));
        let entry = workspace.get("note.plumb").unwrap();
        assert_eq!(entry.revision, 0);
        assert!(entry.current.is_none());
        assert_eq!(entry.last_valid.as_ref().unwrap().revision, 1);
    }

    #[test]
    fn returns_reverse_references() {
        let mut workspace = Workspace::new();
        workspace.insert("a.plumb", 1, "`# Target {\n  `@ target\n}\n");
        workspace.insert("b.plumb", 1, "`->[x]{`:[to a.plumb#target]}\n");
        workspace.insert("missing.plumb", 1, "`->[x]{`:[to a.plumb#missing]}\n");
        workspace.insert(
            "task.plumb",
            1,
            "`task Task {\n  `: depends a.plumb#missing\n}\n",
        );
        workspace.insert("document.plumb", 1, "`->[a]{`:[to a.plumb]}\n");
        workspace.insert(
            "a-local.plumb",
            1,
            "`# Local {\n  `@ local\n}\n\n`->[x]{`:[to #local]}\n",
        );
        assert_eq!(workspace.references_to("a.plumb", "target").len(), 1);
        let document_references = workspace.references_to_document("a.plumb");
        assert_eq!(document_references.len(), 4);
        assert_eq!(
            document_references
                .iter()
                .map(|(path, _)| path.to_path_buf())
                .collect::<Vec<_>>(),
            vec![
                PathBuf::from("b.plumb"),
                PathBuf::from("document.plumb"),
                PathBuf::from("missing.plumb"),
                PathBuf::from("task.plumb"),
            ]
        );
        assert_eq!(workspace.references_to_document("a-local.plumb").len(), 1);
        let batched = workspace
            .reverse_references_for_document("a.plumb", &HashSet::from(["target".to_string()]));
        assert_eq!(batched.document.len(), document_references.len());
        assert_eq!(batched.anchors["target"].len(), 1);
        assert_eq!(
            batched
                .document
                .iter()
                .map(|reference| reference.source_path.clone())
                .collect::<Vec<_>>(),
            document_references
                .iter()
                .map(|(path, _)| path.to_path_buf())
                .collect::<Vec<_>>()
        );
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
    fn batches_document_and_multiple_anchor_reverse_references() {
        let mut workspace = Workspace::new();
        workspace.insert(
            "target.plumb",
            1,
            "`# One {\n  `@ one\n}\n\n`# Two {\n  `@ two\n}\n",
        );
        workspace.insert(
            "source.plumb",
            1,
            "`->[one]{`:[to target.plumb#one]} and `->[two]{`:[to target.plumb#two]}\n",
        );

        let references = workspace.reverse_references_for_document(
            "target.plumb",
            &HashSet::from(["one".to_string(), "two".to_string()]),
        );
        assert_eq!(references.document.len(), 2);
        assert_eq!(references.anchors["one"].len(), 1);
        assert_eq!(references.anchors["two"].len(), 1);
        assert!(references
            .document
            .iter()
            .all(|reference| reference.source_path == Path::new("source.plumb")));
    }

    #[test]
    fn resolves_document_and_anchor_targets_from_declarations_and_reference_components() {
        let target_source = "{\n  `: title Target\n}\n\n`# Section {\n  `@ section\n}\n";
        let reference_source = "See `->[named]{`:[to target.plumb#section]} and `->\"target.plumb#section\".\n\n`task Review {\n  `: prev target.plumb#section\n  `: depends target.plumb#section\n}\n";
        let mut workspace = Workspace::new();
        workspace.insert("target.plumb", 1, target_source);
        workspace.insert("reference.plumb", 1, reference_source);

        assert!(matches!(
            workspace.target_at("target.plumb", target_source.find('{').unwrap()),
            Some(ResolvedTarget::Document { path }) if path == Path::new("target.plumb")
        ));
        assert!(workspace
            .target_at("target.plumb", target_source.find("Target").unwrap())
            .is_none());
        assert!(matches!(
            workspace.target_at("target.plumb", target_source.find("section").unwrap()),
            Some(ResolvedTarget::Anchor { path, id, .. })
                if path == Path::new("target.plumb") && id == "section"
        ));

        for path_offset in reference_source
            .match_indices("target.plumb")
            .map(|(offset, _)| offset)
        {
            assert!(matches!(
                workspace.target_at("reference.plumb", path_offset),
                Some(ResolvedTarget::Document { path })
                    if path == Path::new("target.plumb")
            ));
        }
        for fragment_offset in reference_source
            .match_indices("#section")
            .map(|(offset, _)| offset + 1)
        {
            assert!(matches!(
                workspace.target_at("reference.plumb", fragment_offset),
                Some(ResolvedTarget::Anchor { path, id, .. })
                    if path == Path::new("target.plumb") && id == "section"
            ));
        }
        let separator_offset = reference_source.find("#section").unwrap();
        assert!(matches!(
            workspace.target_at("reference.plumb", separator_offset),
            Some(ResolvedTarget::Anchor { id, .. }) if id == "section"
        ));
        assert!(matches!(
            workspace.target_at("reference.plumb", reference_source.find("named").unwrap()),
            Some(ResolvedTarget::Anchor { id, .. }) if id == "section"
        ));

        let lonely_source = "{\n  `: title\n\n    Lonely\n}\n";
        workspace.insert("lonely.plumb", 1, lonely_source);
        assert!(matches!(
            workspace.target_at("lonely.plumb", lonely_source.find('{').unwrap()),
            Some(ResolvedTarget::Document { path }) if path == Path::new("lonely.plumb")
        ));
        assert!(workspace.references_to_document("lonely.plumb").is_empty());

        workspace.insert("target.plumb", 2, "`node{key=a key=b} Invalid\n");
        assert!(workspace.target_at("target.plumb", 1).is_none());
    }

    #[test]
    fn task_fields_participate_in_navigation_references_and_anchor_rename() {
        let target_source = "`task Draft {\n  `@ draft\n}\n\n`node Note {\n  `@ note\n}\n";
        let reference_source = "`task Review {\n  `@ review\n  `: prev Project Plan.plumb#draft\n  `: depends Project Plan.plumb#draft Project Plan.plumb#note Project%20Plan.plumb#literal\n}\n\nSee `->[draft]{`:[to Project Plan.plumb#draft]}.\n";
        let mut workspace = Workspace::new();
        workspace.insert("Project Plan.plumb", 4, target_source);
        workspace.insert(
            "Project%20Plan.plumb",
            4,
            "`node Literal {\n  `@ literal\n}\n",
        );
        workspace.insert("review.plumb", 7, reference_source);

        let depends_attribute = reference_source.find("`: depends").unwrap();
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

        let literal = reference_source.find("#literal").unwrap() + 1;
        assert_eq!(
            workspace
                .anchor_reference_at("review.plumb", literal)
                .unwrap()
                .target_path,
            PathBuf::from("Project%20Plan.plumb")
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
    fn document_rename_rewrites_raw_task_reference_paths() {
        let target_source = "`task Draft {\n  `@ draft\n}\n";
        let reference_source = "`task Review {\n  `: prev Project Plan.plumb#draft\n  `: depends Project Plan.plumb#draft\n}\n\nSee `->[draft]{`:[to Project Plan.plumb#draft]}.\n";
        let mut workspace = Workspace::new();
        workspace.insert("Project Plan.plumb", 4, target_source);
        workspace.insert("review.plumb", 7, reference_source);

        let path_offset = reference_source.find("Project Plan.plumb").unwrap();
        let target = workspace
            .path_rename_target_at("review.plumb", path_offset)
            .unwrap();
        let edit = workspace
            .rename_document(&target, "Archived Plan.plumb")
            .unwrap();
        let reference_edits = &edit
            .document_changes
            .iter()
            .find(|document| document.path == Path::new("review.plumb"))
            .unwrap()
            .edits;
        assert_eq!(
            reference_edits
                .iter()
                .filter(|edit| edit.new_text == "Archived Plan.plumb")
                .count(),
            3
        );
        assert_eq!(
            edit.resource_operations,
            vec![ResourceOperation::Rename {
                old_path: PathBuf::from("Project Plan.plumb"),
                new_path: PathBuf::from("Archived Plan.plumb"),
            }]
        );
    }

    #[test]
    fn metadata_marker_targets_the_current_document_without_editing_title() {
        let source = "{\n  `: title Stable title\n}\n";
        let mut workspace = Workspace::new();
        workspace.insert("current.plumb", 4, source);
        workspace.insert("incoming.plumb", 7, "`->[current]{`:[to current.plumb]}\n");

        let target = workspace
            .document_rename_target_at("current.plumb", source.find('{').unwrap())
            .unwrap();
        assert_eq!(target.old_path, Path::new("current.plumb"));
        assert_eq!(&source[target.range.clone()], "{");
        assert_eq!(target.input, PathRenameInput::FileStem);
        assert_eq!(
            workspace.rename_document(&target, "archive/renamed"),
            Err(RenameError::InvalidPath)
        );
        assert_eq!(
            workspace.rename_document(&target, "renamed.md"),
            Err(RenameError::InvalidPath)
        );

        let edit = workspace.rename_document(&target, "renamed").unwrap();
        assert!(edit
            .document_changes
            .iter()
            .all(|document| document.path != Path::new("current.plumb")));
        assert_eq!(edit.document_changes[0].edits[0].new_text, "renamed.plumb");
        assert_eq!(
            edit.resource_operations,
            vec![ResourceOperation::Rename {
                old_path: PathBuf::from("current.plumb"),
                new_path: PathBuf::from("renamed.plumb"),
            }]
        );
    }

    #[test]
    fn rename_updates_declaration_and_cross_file_fragments() {
        let mut workspace = Workspace::new();
        workspace.insert("a.plumb", 4, "`# Target {\n  `@ target\n}\n");
        workspace.insert("b.plumb", 7, "`->[x]{`:[to a.plumb#target]}\n");
        let target = workspace
            .anchor_rename_target_at(
                "a.plumb",
                workspace
                    .get("a.plumb")
                    .unwrap()
                    .parsed
                    .source
                    .find("target")
                    .unwrap(),
            )
            .unwrap();
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
        workspace.insert("a.plumb", 1, "`# Not an anchor {\n  `: id pair\n}\n");
        assert_eq!(
            workspace.anchor_rename_target_at("a.plumb", 6),
            Err(RenameError::NotRenameable)
        );
        workspace.insert("a.plumb", 2, "`# Anchor {\n  `@ real\n}\n");
        let target = workspace
            .anchor_rename_target_at(
                "a.plumb",
                workspace
                    .get("a.plumb")
                    .unwrap()
                    .parsed
                    .source
                    .find("real")
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(
            workspace.rename_anchor(&target, "has space"),
            Err(RenameError::InvalidId)
        );
    }

    #[test]
    fn completes_paths_and_only_explicit_anchors() {
        let mut workspace = Workspace::new();
        let autolink_path =
            |replace: std::ops::Range<usize>, query: &str| LinkCompletionContext::AutolinkPath {
                envelope: replace.clone(),
                replace,
                quote_count: 0,
                suffix: String::new(),
                query: query.to_string(),
            };
        workspace.insert("notes/current.plumb", 1, "Current\n");
        workspace.insert(
            "notes/design.plumb",
            1,
            "{\n  `: title Design Guide\n}\n\n`# No id\n\n`## API {\n  `@ api\n}\n",
        );
        workspace.insert(
            "notes/Project Plan.plumb",
            1,
            "{\n  `: title Project Plan\n}\n\n`# Roadmap {\n  `@ roadmap\n}\n",
        );
        workspace.insert("notes/中文笔记.plumb", 1, "`# 中文内容 {\n  `@ 内容\n}\n");
        workspace.insert("notes/方案 (草稿).plumb", 1, "`# 草稿\n");
        workspace.insert("notes/方案]终稿.plumb", 1, "`# 终稿\n");
        workspace.insert("notes/brace{draft}].plumb", 1, "`# Braces\n");
        workspace.insert("notes/quote\"name.plumb", 1, "`# Quote\n");
        let paths = workspace.complete_link("notes/current.plumb", &autolink_path(10..13, "guide"));
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
        assert_eq!(labels[0].new_text, "`->[Design Guide]{`:[to design.plumb]}");
        let spaced_label = workspace.complete_link(
            "notes/current.plumb",
            &LinkCompletionContext::Label {
                replace: 0..0,
                query: "project".to_string(),
            },
        );
        assert_eq!(
            spaced_label[0].new_text,
            "`->[Project Plan]{`:[to Project Plan.plumb]}"
        );
        let spaced_path = workspace.complete_link(
            "notes/current.plumb",
            &LinkCompletionContext::Path {
                replace: 0..0,
                query: "project".to_string(),
                quoted: true,
            },
        );
        assert_eq!(spaced_path[0].new_text, "Project Plan.plumb");
        let quote_path = workspace.complete_link(
            "notes/current.plumb",
            &LinkCompletionContext::Path {
                replace: 0..0,
                query: "quote".to_string(),
                quoted: true,
            },
        );
        assert_eq!(quote_path[0].label, "quote\"name.plumb");
        assert_eq!(quote_path[0].new_text, "quote\\\"name.plumb");
        let spaced_autolink =
            workspace.complete_link("notes/current.plumb", &autolink_path(0..0, "project"));
        assert_eq!(spaced_autolink[0].label, "Project Plan.plumb");
        assert_eq!(spaced_autolink[0].new_text, "Project Plan.plumb");
        let unicode = workspace.complete_link("notes/current.plumb", &autolink_path(0..0, "中文"));
        assert_eq!(unicode[0].label, "中文笔记.plumb");
        assert_eq!(unicode[0].new_text, "中文笔记.plumb");
        let parentheses =
            workspace.complete_link("notes/current.plumb", &autolink_path(0..0, "草稿"));
        assert_eq!(parentheses[0].label, "方案 (草稿).plumb");
        assert_eq!(parentheses[0].new_text, "方案 (草稿).plumb");
        let closing_bracket = workspace.complete_link(
            "notes/current.plumb",
            &LinkCompletionContext::AutolinkPath {
                replace: 2..3,
                envelope: 0..5,
                quote_count: 0,
                suffix: String::new(),
                query: "终稿".to_string(),
            },
        );
        assert_eq!(closing_bracket[0].label, "方案]终稿.plumb");
        assert_eq!(closing_bracket[0].new_text, "`\"[方案]终稿.plumb]\"");
        assert_eq!(closing_bracket[0].replace, 0..5);
        let structural_delimiters = workspace.complete_link(
            "notes/current.plumb",
            &LinkCompletionContext::Label {
                replace: 0..0,
                query: "brace".to_string(),
            },
        );
        assert_eq!(
            structural_delimiters[0].new_text,
            "`->[brace{draft}`].plumb]{`:[to brace`{draft`}`].plumb]}"
        );
        assert!(parse(&structural_delimiters[0].new_text).is_valid());
        let spaced_anchor = workspace.complete_link(
            "notes/current.plumb",
            &LinkCompletionContext::AutolinkAnchor {
                path: "Project Plan.plumb".to_string(),
                replace: 0..0,
                query: "road".to_string(),
            },
        );
        assert_eq!(spaced_anchor[0].new_text, "roadmap");
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
    fn completes_and_resolves_relative_image_files() {
        use std::sync::atomic::{AtomicU64, Ordering};

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "plumb-image-completion-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let static_dir = root.join("static");
        std::fs::create_dir_all(static_dir.join("nested")).unwrap();
        std::fs::write(static_dir.join("image one.PNG"), b"png").unwrap();
        std::fs::write(static_dir.join("图 像(100%).PNG"), b"png").unwrap();
        std::fs::write(static_dir.join("literal%20name.PNG"), b"png").unwrap();
        std::fs::write(static_dir.join("quote\"image.PNG"), b"png").unwrap();
        std::fs::write(static_dir.join("literal%20name.txt"), b"text").unwrap();
        std::fs::write(static_dir.join("ignored.txt"), b"text").unwrap();
        let source_path = root.join("current.plumb");
        let source = "`->\"static/image one.PNG\"\n`img[Result]{`:[src static/image one.PNG]}\n`img[Literal percent]{`:[src static/literal%20name.PNG]}\n`->\"static/literal%20name.txt\"\n";
        let mut workspace = Workspace::new();
        workspace.insert(&source_path, 3, source);

        let candidates = workspace.complete_image_path(
            &source_path,
            &ImageCompletionContext {
                replace: 18..25,
                query: "static/im".to_string(),
                quoted: true,
            },
        );
        let image_with_space = candidates
            .iter()
            .find(|candidate| candidate.label == "static/image one.PNG")
            .unwrap();
        assert_eq!(image_with_space.new_text, "static/image one.PNG");
        assert_eq!(image_with_space.detail, "image file");
        assert_eq!(image_with_space.replace, 18..25);

        let unicode = workspace.complete_image_path(
            &source_path,
            &ImageCompletionContext {
                replace: 0..0,
                query: "static/图".to_string(),
                quoted: true,
            },
        );
        assert_eq!(unicode.len(), 1);
        assert_eq!(unicode[0].label, "static/图 像(100%).PNG");
        assert_eq!(unicode[0].new_text, "static/图 像(100%).PNG");

        let quoted = workspace.complete_image_path(
            &source_path,
            &ImageCompletionContext {
                replace: 0..0,
                query: "static/quote".to_string(),
                quoted: true,
            },
        );
        assert_eq!(quoted.len(), 1);
        assert_eq!(quoted[0].label, "static/quote\"image.PNG");
        assert_eq!(quoted[0].new_text, "static/quote\\\"image.PNG");

        let directories = workspace.complete_image_path(
            &source_path,
            &ImageCompletionContext {
                replace: 0..0,
                query: "static/ne".to_string(),
                quoted: true,
            },
        );
        assert!(directories
            .iter()
            .any(|candidate| candidate.new_text == "static/nested/"));

        let link = workspace
            .link_at(&source_path, source.find("image one").unwrap())
            .unwrap();
        assert_eq!(
            workspace.resolve_link(&source_path, link),
            ResolvedTarget::File {
                path: static_dir.join("image one.PNG")
            }
        );
        let literal_percent = workspace
            .link_at(&source_path, source.rfind("literal%20name").unwrap())
            .unwrap();
        assert_eq!(
            workspace.resolve_link(&source_path, literal_percent),
            ResolvedTarget::File {
                path: static_dir.join("literal%20name.txt")
            }
        );
        let image = workspace
            .image_at(&source_path, source.find("Result").unwrap())
            .unwrap();
        assert_eq!(
            workspace.resolve_image(&source_path, image),
            ResolvedTarget::File {
                path: static_dir.join("image one.PNG")
            }
        );
        let literal_percent_image = workspace
            .image_at(&source_path, source.find("Literal percent").unwrap())
            .unwrap();
        assert_eq!(
            workspace.resolve_image(&source_path, literal_percent_image),
            ResolvedTarget::File {
                path: static_dir.join("literal%20name.PNG")
            }
        );
        assert!(workspace.diagnostics(&source_path).is_empty());

        std::fs::remove_file(static_dir.join("image one.PNG")).unwrap();
        std::fs::remove_file(static_dir.join("图 像(100%).PNG")).unwrap();
        std::fs::remove_file(static_dir.join("literal%20name.PNG")).unwrap();
        std::fs::remove_file(static_dir.join("quote\"image.PNG")).unwrap();
        std::fs::remove_file(static_dir.join("literal%20name.txt")).unwrap();
        let unresolved = workspace
            .diagnostics(&source_path)
            .into_iter()
            .find(|diagnostic| diagnostic.code == "image.unresolved-file")
            .unwrap();
        assert!(unresolved
            .message
            .contains(&static_dir.join("image one.PNG").display().to_string()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_file_attachments_and_reports_missing_targets() {
        use std::sync::atomic::{AtomicU64, Ordering};

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "plumb-file-resolution-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("static")).unwrap();
        std::fs::write(root.join("static/demo.mp4"), b"video").unwrap();
        std::fs::write(root.join("static/manual.pdf"), b"pdf").unwrap();
        let source_path = root.join("note.plumb");
        let source =
            "`file[Demo]{`:[src static/demo.mp4]}\n`file[Missing]{`:[src static/missing.pdf]}\n";
        let mut workspace = Workspace::new();
        workspace.insert(&source_path, 1, source);

        let file = workspace
            .file_at(&source_path, source.find("Demo").unwrap())
            .unwrap();
        assert_eq!(
            workspace.resolve_file(&source_path, file),
            ResolvedTarget::File {
                path: root.join("static/demo.mp4")
            }
        );
        assert_eq!(
            workspace.target_at(&source_path, source.find("demo.mp4").unwrap()),
            Some(ResolvedTarget::File {
                path: root.join("static/demo.mp4")
            })
        );
        let completions = workspace.complete_file_path(
            &source_path,
            &FileCompletionContext {
                replace: 0..0,
                query: "static/ma".to_string(),
                quoted: true,
            },
        );
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].new_text, "static/manual.pdf");
        assert_eq!(completions[0].detail, "file attachment");
        let diagnostics = workspace.diagnostics(&source_path);
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == "file.unresolved-file")
                .count(),
            1
        );
        assert!(diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains(&root.join("static/missing.pdf").display().to_string())));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn searches_note_and_task_records_with_stable_fuzzy_results() {
        let root = Path::new("notes");
        let now = DateTime::parse_from_rfc3339("2026-07-22T12:00:00+08:00").unwrap();
        let mut workspace = Workspace::new();
        workspace.insert(
            "notes/design.plumb",
            4,
            "{\n  `: title Design Guide\n}\n\n`task Review parser {\n  `@ review\n  `: due 2026-07-23T12:00:00+08:00\n}\n",
        );
        workspace.insert("notes/fallback.plumb", 2, "Fallback body\n");

        let notes = workspace.search_records(root, Some(SearchRecordKind::Note), "dsg", 20, now);
        assert!(notes.complete);
        assert_eq!(notes.items.len(), 1);
        assert_eq!(notes.items[0].title, "Design Guide");
        assert_eq!(notes.items[0].relative_path, "design.plumb");
        assert_eq!(notes.items[0].revision, 4);

        let tasks = workspace.search_records(root, Some(SearchRecordKind::Task), "review", 20, now);
        assert_eq!(tasks.items.len(), 1);
        assert_eq!(tasks.items[0].id.as_deref(), Some("review"));
        assert_eq!(tasks.items[0].task_state, Some(TaskWorkflowState::Ready));
        assert_eq!(tasks.items[0].wait_reasons, Some(Vec::new()));
        assert_eq!(tasks.items[0].blocked, Some(false));
        assert_eq!(tasks.items[0].actionable, Some(true));

        let fallback =
            workspace.search_records(root, Some(SearchRecordKind::Note), "fallback", 20, now);
        assert_eq!(fallback.items[0].title, "fallback");
    }

    #[test]
    fn derives_mutually_exclusive_task_workflow_states_for_search_and_cel() {
        let root = Path::new("notes");
        let now = DateTime::parse_from_rfc3339("2026-07-22T12:00:00+08:00").unwrap();
        let mut workspace = Workspace::new();
        workspace.insert(
            "notes/tasks.plumb",
            1,
            "`task Blocker {\n  `@ blocker\n}\n`task Ready {\n  `@ ready\n  `: priority 7\n}\n`task Time wait {\n  `@ time\n  `: wait 2026-07-23T12:00:00+08:00\n}\n`task Dependency blocked {\n  `@ dependency\n  `: depends #blocker\n}\n`task Both reasons {\n  `@ both\n  `: wait 2026-07-23T12:00:00+08:00\n  `: depends #blocker\n}\n`task Done {\n  `@ done\n  `: done 2026-07-21T12:00:00+08:00\n}\n`task Canceled {\n  `@ canceled\n  `: canceled 2026-07-21T12:00:00+08:00\n}\n`task Conflicted {\n  `@ conflicted\n  `: done 2026-07-21T12:00:00+08:00\n  `: canceled 2026-07-21T13:00:00+08:00\n}\n",
        );

        let results = workspace.search_records(root, Some(SearchRecordKind::Task), "", 20, now);
        let by_id = |id: &str| {
            results
                .items
                .iter()
                .find(|record| record.id.as_deref() == Some(id))
                .unwrap()
        };
        assert_eq!(by_id("ready").task_state, Some(TaskWorkflowState::Ready));
        assert_eq!(by_id("ready").priority, Some(7));
        assert_eq!(by_id("time").task_state, Some(TaskWorkflowState::Waiting));
        assert_eq!(by_id("time").wait_reasons, Some(vec![TaskWaitReason::Time]));
        assert_eq!(
            by_id("dependency").task_state,
            Some(TaskWorkflowState::Blocked)
        );
        assert_eq!(
            by_id("dependency").wait_reasons,
            Some(vec![TaskWaitReason::Dependency])
        );
        assert_eq!(
            by_id("both").wait_reasons,
            Some(vec![TaskWaitReason::Time, TaskWaitReason::Dependency])
        );
        assert_eq!(by_id("done").task_state, Some(TaskWorkflowState::Done));
        assert_eq!(
            by_id("canceled").task_state,
            Some(TaskWorkflowState::Canceled)
        );
        assert_eq!(
            by_id("conflicted").task_state,
            Some(TaskWorkflowState::Conflicted)
        );

        let waiting = workspace
            .search_records_filtered(
                root,
                Some(SearchRecordKind::Task),
                "",
                20,
                now,
                Some("state == 'waiting'"),
            )
            .unwrap();
        assert_eq!(waiting.items.len(), 2);
        let blocked = workspace
            .search_records_filtered(
                root,
                Some(SearchRecordKind::Task),
                "",
                20,
                now,
                Some("state == 'blocked'"),
            )
            .unwrap();
        assert_eq!(blocked.items.len(), 1);
        assert_eq!(blocked.items[0].id.as_deref(), Some("dependency"));
        let conflicted = workspace
            .search_records_filtered(
                root,
                Some(SearchRecordKind::Task),
                "",
                20,
                now,
                Some("state == 'conflicted'"),
            )
            .unwrap();
        assert_eq!(conflicted.items.len(), 1);
        let time_waiting = workspace
            .search_records_filtered(
                root,
                Some(SearchRecordKind::Task),
                "",
                20,
                now,
                Some("wait_reasons.exists(reason, reason == 'time')"),
            )
            .unwrap();
        assert_eq!(time_waiting.items.len(), 2);
        let prioritized = workspace
            .search_records_filtered(
                root,
                Some(SearchRecordKind::Task),
                "",
                20,
                now,
                Some("priority != null && priority >= 7"),
            )
            .unwrap();
        assert_eq!(prioritized.items.len(), 1);
        assert_eq!(prioritized.items[0].id.as_deref(), Some("ready"));
    }

    #[test]
    fn propagates_effective_priority_through_open_dependencies_and_ancestors() {
        let root = Path::new("notes");
        let now = DateTime::parse_from_rfc3339("2026-08-05T12:00:00+08:00").unwrap();
        let mut workspace = Workspace::new();
        workspace.insert(
            "notes/a.plumb",
            1,
            "`task Parent {\n  `@ parent\n  `: priority -10\n}\n\n      `task Urgent {\n        `@ urgent\n        `: priority 40\n        `: depends b.plumb#middle #closed\n      }\n\n`task Closed {\n  `@ closed\n  `: priority -20\n  `: done 2026-08-04T12:00:00+08:00\n}\n",
        );
        workspace.insert(
            "notes/b.plumb",
            1,
            "`task Middle {\n  `@ middle\n  `: priority 1\n  `: depends c.plumb#base\n}\n",
        );
        workspace.insert("notes/c.plumb", 1, "`task Base {\n  `@ base\n}\n");
        workspace.insert(
            "notes/cycle.plumb",
            1,
            "`task Cycle high {\n  `@ cycle-high\n  `: priority 30\n  `: depends #cycle-low\n}\n`task Cycle low {\n  `@ cycle-low\n  `: priority -10\n  `: depends #cycle-high\n}\n",
        );

        let results = workspace.search_records(root, Some(SearchRecordKind::Task), "", 20, now);
        let priority = |id: &str| {
            results
                .items
                .iter()
                .find(|record| record.id.as_deref() == Some(id))
                .unwrap()
                .effective_priority
        };
        assert_eq!(priority("urgent"), Some(40));
        assert_eq!(priority("parent"), Some(40));
        assert_eq!(priority("middle"), Some(40));
        assert_eq!(priority("base"), Some(40));
        assert_eq!(priority("closed"), Some(-20));
        assert_eq!(priority("cycle-high"), Some(30));
        assert_eq!(priority("cycle-low"), Some(30));
    }

    #[test]
    fn search_records_use_current_valid_snapshots_and_report_truncation() {
        let now = DateTime::parse_from_rfc3339("2026-07-22T12:00:00Z").unwrap();
        let mut workspace = Workspace::new();
        workspace.insert("a.plumb", 1, "Old title\n");
        workspace.insert("a.plumb", 2, "New title\n");
        workspace.insert("b.plumb", 1, "Another\n");

        let limited = workspace.search_records("", None, "", 1, now);
        assert_eq!(limited.items.len(), 1);
        assert!(!limited.complete);
        assert!(limited
            .items
            .iter()
            .all(|record| record.revision != 1 || record.path != Path::new("a.plumb")));

        workspace.insert("a.plumb", 3, "`span[broken\n");
        let invalid = workspace.search_records("", None, "new", 20, now);
        assert!(invalid.items.is_empty());
    }

    #[test]
    fn document_rename_rewrites_incoming_and_outgoing_relative_paths() {
        let mut workspace = Workspace::new();
        workspace.insert(
            "notes/a.plumb",
            1,
            "`# A {\n  `@ a\n}\n\n`->[c]{`:[to ../shared/c.plumb#c]}\n",
        );
        workspace.insert("notes/b.plumb", 2, "`->[a]{`:[to a.plumb#a]}\n");
        workspace.insert("shared/c.plumb", 3, "`# C {\n  `@ c\n}\n");
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
        assert_eq!(target.input, PathRenameInput::Path);
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
    fn document_rename_strengthens_autolink_delimiters() {
        let mut workspace = Workspace::new();
        workspace.insert("notes/a.plumb", 1, "`# A {\n  `@ a\n}\n");
        let reference = "`->\"a.plumb#a\"\n";
        workspace.insert("notes/b.plumb", 2, reference);
        let link = &workspace
            .get("notes/b.plumb")
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .output
            .links[0];
        let target = workspace
            .path_rename_target_at("notes/b.plumb", link.path_range.as_ref().unwrap().start)
            .unwrap();
        let edit = workspace
            .rename_document(&target, "archive/a] final.plumb")
            .unwrap();
        let incoming = edit
            .document_changes
            .iter()
            .find(|document| document.path == Path::new("notes/b.plumb"))
            .unwrap();
        let mut edited = reference.to_string();
        for text_edit in incoming.edits.iter().rev() {
            edited.replace_range(text_edit.range.clone(), &text_edit.new_text);
        }
        assert_eq!(edited, "`->\"archive/a] final.plumb#a\"\n");
        assert!(parse(edited).is_valid());
    }

    #[test]
    fn resolves_open_task_dependencies_and_blocked_state() {
        let mut workspace = Workspace::new();
        workspace.insert(
            "notes/Project Plan.plumb",
            1,
            "`task Draft {\n  `@ draft\n}\n`task Done {\n  `@ done\n  `: done 2026-07-20T09:00:00Z\n}\n",
        );
        workspace.insert(
            "notes/review.plumb",
            2,
            "`task Review {\n  `@ review\n  `: depends Project Plan.plumb#draft Project Plan.plumb#done\n}\n",
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
    fn diagnoses_completed_tasks_with_open_dependencies_and_descendants() {
        let mut workspace = Workspace::new();
        workspace.insert(
            "remote.plumb",
            1,
            "`task Remote blocker {\n  `@ remote\n}\n",
        );
        workspace.insert(
            "tasks.plumb",
            2,
            "`task Completed parent {\n  `@ parent\n  `: done 2026-07-27T10:00:00Z\n  `: depends #explicit remote.plumb#remote\n}\n\n      `task Explicit child {\n        `@ explicit\n      }\n      `task Implicit child {\n      }\n      `task Canceled child {\n        `: canceled 2026-07-27T10:01:00Z\n      }\n\n`task Canceled parent {\n  `: canceled 2026-07-27T10:02:00Z\n}\n\n      `task Open child is allowed {\n      }\n\n`task Completed tree {\n  `: done 2026-07-27T10:03:00Z\n}\n\n      `task Completed child {\n        `: done 2026-07-27T10:04:00Z\n      }\n",
        );

        let diagnostics = workspace.diagnostics("tasks.plumb");
        let dependency = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "task.done-with-open-dependency")
            .unwrap();
        assert_eq!(dependency.severity, DiagnosticSeverity::Warning);
        assert_eq!(
            dependency.message,
            "completed task still depends on 2 open tasks"
        );
        assert_eq!(dependency.related.len(), 1);

        let descendant = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "task.done-with-open-descendant")
            .unwrap();
        assert_eq!(descendant.severity, DiagnosticSeverity::Warning);
        assert_eq!(
            descendant.message,
            "completed task still contains 1 open descendant"
        );
        assert_eq!(descendant.related.len(), 1);
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code.starts_with("task.done-with-open-"))
                .count(),
            2
        );
    }

    #[test]
    fn diagnoses_invalid_task_targets_self_dependencies_and_cycles() {
        let mut workspace = Workspace::new();
        workspace.insert(
            "tasks.plumb",
            1,
            "`node Plain anchor {\n  `@ plain\n}\n\n`task A {\n  `@ a\n  `: depends #b\n}\n`task B {\n  `@ b\n  `: depends #a\n}\n`task Self {\n  `@ self\n  `: depends #self\n}\n`task Invalid targets {\n  `: prev #plain\n  `: depends #plain #missing bare#invalid missing.plumb#x\n}\n",
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
    fn task_status_operation_is_guarded_and_formats_the_affected_block() {
        let mut workspace = Workspace::new();
        let source = "`task Write parser {\n  `@ write\n  `: due 2026-07-21T09:00:00Z\n}\n";
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
        assert_eq!(document.edits.len(), 1);
        let operation = &document.edits[0];
        let mut edited = source.to_string();
        edited.replace_range(operation.range.clone(), &operation.new_text);
        assert!(edited.contains("`@ write"));
        assert!(edited.contains("`: due 2026-07-21T09:00:00Z"));
        assert!(edited.contains("`: done 2026-07-20T12:00:00+08:00"));
        assert_eq!(plumb_format::format(&edited).unwrap(), edited);
    }

    #[test]
    fn task_status_targets_an_explicitly_anchored_nested_task() {
        let source = "`task MJCF in, USD out solver {\n `@ task-f81deb18\n `: created 2026-05-24T02:35:50Z\n}\n\n `task 刚体版本 {\n  `@ task-9d49eb30\n  `: created 2026-05-24T02:35:32Z\n  `: done 2026-05-26T01:43:39Z\n }\n `task parse MJCF {\n  `@ task-c2cf5756\n  `: created 2026-05-27T13:03:04Z\n }\n `task solver with passive joint {\n  `@ task-99e28dad\n  `: created 2026-05-27T13:02:45Z\n }\n";
        let mut workspace = Workspace::new();
        workspace.insert("embodied-intelligence.plumb", 12, source);

        let operation = workspace
            .set_task_status(
                "embodied-intelligence.plumb",
                source.find("parse MJCF").unwrap(),
                TaskStatus::Done,
                "2026-07-22T22:41:21+08:00",
            )
            .unwrap();
        let edit = &operation.document_changes[0].edits[0];
        let mut edited = source.to_string();
        edited.replace_range(edit.range.clone(), &edit.new_text);

        assert!(edited.contains("`@ task-c2cf5756"));
        assert!(edited.contains("`: done 2026-07-22T22:41:21+08:00"));
        assert_eq!(
            edited.matches("`: done 2026-07-22T22:41:21+08:00").count(),
            1
        );
        assert_eq!(plumb_format::format(&edited).unwrap(), edited);
    }

    #[test]
    fn task_status_formats_multiline_attributes_with_a_long_head() {
        let source = "`task `->[如何在 nix 中检查 IFD]{`:[to 如何在 nix 中检查 IFD.plumb]} {\n `: created 2026-07-21T14:37:59+08:00\n}\n";
        assert_eq!(plumb_format::format(source).unwrap(), source);
        let mut workspace = Workspace::new();
        workspace.insert("closed.plumb", 8, source);

        let operation = workspace
            .set_task_status(
                "closed.plumb",
                source.find("检查 IFD").unwrap(),
                TaskStatus::Done,
                "2026-07-21T21:52:24+08:00",
            )
            .unwrap();
        let edit = &operation.document_changes[0].edits[0];
        let mut edited = source.to_string();
        edited.replace_range(edit.range.clone(), &edit.new_text);

        assert_eq!(
            edited,
            "`task `->[如何在 nix 中检查 IFD]{`:[to 如何在 nix 中检查 IFD.plumb]} {\n `: created 2026-07-21T14:37:59+08:00\n `: done 2026-07-21T21:52:24+08:00\n}\n"
        );
        assert_eq!(plumb_format::format(&edited).unwrap(), edited);
    }

    #[test]
    fn task_status_formats_the_complete_owner_subtree() {
        let source = "`task Parent {\n  `@ parent\n}\n\n      `- Child\n\n`# Following\n";
        let mut workspace = Workspace::new();
        workspace.insert("tasks.plumb", 9, source);

        let operation = workspace
            .set_task_status_by_id(
                "tasks.plumb",
                "parent",
                TaskStatus::Done,
                "2026-07-21T22:00:00+08:00",
            )
            .unwrap();
        let edited = apply_single_edit(source, &operation);

        assert!(edited.contains("`@ parent"));
        assert!(edited.contains("`: done 2026-07-21T22:00:00+08:00"));
        assert!(edited.contains("\n `- Child\n\n`# Following"));
        assert_eq!(plumb_format::format(&edited).unwrap(), edited);
    }

    #[test]
    fn task_authoring_operations_convert_items_and_add_created() {
        let source = "`- Outer {\n  `@ outer\n  `- keep\n}\n\n   `- Nested\n\n`task Closed {\n  `@ closed\n  `: done 2026-07-20T09:00:00Z\n}\n`task Existing {\n  `@ existing\n  `: created 2026-07-19T09:00:00Z\n}\n";
        let mut workspace = Workspace::new();
        workspace.insert("tasks.plumb", 7, source);
        let timestamp = "2026-07-20T10:00:00+08:00";

        let nested_offset = source.find("Nested").unwrap();
        let conversion = workspace
            .convert_list_item_to_task("tasks.plumb", nested_offset, timestamp)
            .unwrap();
        assert_eq!(conversion.document_changes[0].expected_revision, 7);
        let edit = &conversion.document_changes[0].edits[0];
        let mut converted = source.to_string();
        converted.replace_range(edit.range.clone(), &edit.new_text);
        assert!(
            converted.contains("   `task Nested {\n    `: created 2026-07-20T10:00:00+08:00\n   }")
        );

        let outer_conversion = workspace
            .convert_list_item_to_task("tasks.plumb", source.find("Outer").unwrap(), timestamp)
            .unwrap();
        assert!(outer_conversion.document_changes[0].edits[0]
            .new_text
            .contains(
                "`task Outer {\n `@ outer\n `- keep\n `: created 2026-07-20T10:00:00+08:00\n}\n"
            ));

        let closed_offset = source.find("Closed").unwrap();
        let created = workspace
            .add_task_created("tasks.plumb", closed_offset, timestamp)
            .unwrap();
        assert!(created.document_changes[0].edits[0]
            .new_text
            .contains("`: created 2026-07-20T10:00:00+08:00"));
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
    fn authoring_operations_preserve_formatter_fixed_points() {
        let timestamp = "2026-07-21T21:52:24+08:00";

        let conversion_source = "`- Convert me {\n  `@ item\n  `- kind\n}\n";
        let mut conversion_workspace = Workspace::new();
        conversion_workspace.insert("conversion.plumb", 1, conversion_source);
        let conversion = conversion_workspace
            .convert_list_item_to_task(
                "conversion.plumb",
                conversion_source.find("Convert").unwrap(),
                timestamp,
            )
            .unwrap();
        let converted = apply_single_edit(conversion_source, &conversion);
        assert_eq!(plumb_format::format(&converted).unwrap(), converted);

        let created_source = "`task Add created {\n  `@ created\n}\n";
        let mut created_workspace = Workspace::new();
        created_workspace.insert("created.plumb", 2, created_source);
        let created = created_workspace
            .add_task_created(
                "created.plumb",
                created_source.find("Add created").unwrap(),
                timestamp,
            )
            .unwrap();
        let with_created = apply_single_edit(created_source, &created);
        assert_eq!(plumb_format::format(&with_created).unwrap(), with_created);

        let id_source = "`note Add an explicit identifier {\n  `- class\n  `: key value\n}\n";
        let mut id_workspace = Workspace::new();
        id_workspace.insert("id.plumb", 3, id_source);
        let id = id_workspace
            .add_explicit_id("id.plumb", id_source.find("identifier").unwrap())
            .unwrap();
        let with_id = apply_single_edit(id_source, &id);
        assert_eq!(plumb_format::format(&with_id).unwrap(), with_id);

        let metadata_source = "`# Section\n";
        let mut metadata_workspace = Workspace::new();
        metadata_workspace.insert("metadata.plumb", 4, metadata_source);
        let metadata = metadata_workspace
            .insert_metadata("metadata.plumb", 0, "metadata", timestamp)
            .unwrap();
        let with_metadata = apply_single_edit(metadata_source, &metadata);
        assert_eq!(plumb_format::format(&with_metadata).unwrap(), with_metadata);
    }

    #[test]
    fn add_explicit_id_targets_the_deepest_block_and_generates_unique_slugs() {
        let source = "`# Hello, World! {\n  `- keep\n}\n\n`node Outer\n\n      `child Nested title\n\n`text\"\n  raw\n`note Multiline attrs {\n  `- keep\n}\n\n`other Existing {\n  `@ hello-world\n}\n\n`# Hello, World!\n";
        let mut workspace = Workspace::new();
        workspace.insert("note.plumb", 7, source);

        let heading = workspace
            .add_explicit_id("note.plumb", source.find("Hello, World!").unwrap())
            .unwrap();
        assert_eq!(heading.document_changes[0].expected_revision, 7);
        let edit = &heading.document_changes[0].edits[0];
        assert!(edit
            .new_text
            .contains("`# Hello, World! {\n `@ hello-world-2\n `- keep\n}\n"));

        let nested = workspace
            .add_explicit_id("note.plumb", source.find("Nested title").unwrap())
            .unwrap();
        assert!(
            nested.document_changes[0].edits[0]
                .new_text
                .contains("`child Nested title {\n       `@ nested-title\n      }\n"),
            "{}",
            nested.document_changes[0].edits[0].new_text
        );

        let sibling_boundary = workspace
            .add_explicit_id("note.plumb", source.find("`node").unwrap())
            .unwrap();
        assert!(sibling_boundary.document_changes[0].edits[0]
            .new_text
            .contains("`node Outer {\n `@ outer\n}\n"));

        let raw = workspace
            .add_explicit_id("note.plumb", source.find("raw").unwrap())
            .unwrap();
        assert!(raw.document_changes[0].edits[0]
            .new_text
            .contains("`text\" {`@[block]}"));

        let multiline = workspace
            .add_explicit_id("note.plumb", source.find("Multiline attrs").unwrap())
            .unwrap();
        assert!(multiline.document_changes[0].edits[0]
            .new_text
            .contains("`note Multiline attrs {\n `@ multiline-attrs\n `- keep\n}\n"));

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
            "`task Outer {\n  `@ outer\n}\n\n      `task Inner {\n        `@ inner\n        `: done 2026-07-20T09:00:00Z\n      }\n";
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
        let operation = &edit.document_changes[0].edits[0];
        assert!(operation.range.start <= tasks[0].range.start);
        assert!(operation.range.end >= tasks[0].range.end);
        assert_ne!(operation.range.start, tasks[1].attribute_insert);
        let mut edited = source.to_string();
        edited.replace_range(operation.range.clone(), &operation.new_text);
        assert!(edited.contains("`@ outer"));
        assert!(edited.contains("`: done 2026-07-20T12:00:00Z"));
        assert_eq!(edited.matches("`: done 2026-07-20T09:00:00Z").count(), 1);
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
            "`task Blocker {\n  `@ blocker\n}\n`task Blocked {\n  `@ blocked\n  `: depends #blocker\n}\n`task Closed {\n  `@ closed\n  `: done 2026-07-20T09:00:00Z\n}\n`task Recurring {\n  `@ recur\n  `: due 2026-07-21T09:00:00Z\n  `: recur P1D\n}\n",
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
        let source = "`task Monthly review {\n  `- daily\n  `: due 2026-01-31T09:00:00+08:00\n  `: wait 2026-01-30T09:00:00+08:00\n  `: recur P1M\n}\n\n      `note Keep details\n\n      `task Nested {\n        `@ nested\n        `: done 2026-01-20T09:00:00+08:00\n      }\n";
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
        assert_eq!(edits.len(), 1);
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
        let mut edited = source.to_string();
        for edit in edits {
            edited.replace_range(edit.range, &edit.new_text);
        }

        assert!(edited.contains("`@ monthly-review-2026-01-31"));
        assert!(edited.contains("`: done 2026-01-31T10:00:00+08:00"));
        assert!(edited.contains("`@ monthly-review-2026-02-28"));
        assert!(edited.contains("`: created 2026-01-31T10:00:00+08:00"));
        assert!(edited.contains("`: due 2026-02-28T09:00:00+08:00"));
        assert!(edited.contains("`: wait 2026-02-28T09:00:00+08:00"));
        assert!(edited.contains("`: prev #monthly-review-2026-01-31"));
        assert_eq!(edited.matches("nested").count(), 1);
        assert_eq!(edited.matches("`: done 2026-01-20").count(), 1);
        let parsed = parse(&edited);
        assert!(parsed.is_valid(), "{}\n{:?}", edited, parsed.diagnostics);
        let output = analyze_document(&parsed.source, &parsed.syntax);
        assert_eq!(output.tasks.tasks.len(), 4);
        assert_eq!(output.tasks.tasks[2].state(), TaskState::Open);
    }

    #[test]
    fn recurring_task_clone_preserves_crlf_and_nested_base_indent() {
        let source = "`node Parent\r\n\r\n      `task Weekly review {\r\n        `: due 2026-07-20T09:00:00+08:00\r\n        `: recur P1W\r\n      }\r\n";
        let mut workspace = Workspace::new();
        workspace.insert("tasks.plumb", 5, source);
        let task = &workspace
            .get("tasks.plumb")
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .output
            .tasks
            .tasks[0];
        let line_start = source[..task.range.start]
            .rfind('\n')
            .map_or(0, |offset| offset + 1);
        assert_eq!(&source[line_start..task.range.start], "      ");

        let edit = workspace
            .set_task_status(
                "tasks.plumb",
                source.find("Weekly review").unwrap(),
                TaskStatus::Done,
                "2026-07-20T10:00:00+08:00",
            )
            .unwrap();
        assert_eq!(edit.document_changes[0].edits.len(), 1);
        let replacement = &edit.document_changes[0].edits[0].new_text;
        assert!(replacement.starts_with("      `task"), "{replacement:?}");
        assert!(!replacement.starts_with("\r\n"));
        assert!(!replacement.replace("\r\n", "").contains('\n'));

        let mut edits = edit.document_changes[0].edits.clone();
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
        let mut edited = source.to_string();
        for edit in edits {
            edited.replace_range(edit.range, &edit.new_text);
        }
        let parsed = parse(&edited);
        assert!(parsed.is_valid(), "{edited:?}\n{:?}", parsed.diagnostics);
        assert!(!edited.contains("\r\n\r\n\r\n"));
    }

    #[test]
    fn recurring_task_completion_preserves_canonical_layout() {
        let source = "`# 饮食相关任务\n\n`task 控制饮食 {\n `@ 控制饮食-2026-07-20\n `: priority -5\n `: created 2026-07-20T01:06:48+08:00\n `: due 2026-07-20T23:59:59+08:00\n `: wait 2026-07-20T00:00:00+08:00\n `: recur P1D\n `: prev #控制饮食-2026-07-19\n}\n\n`# 锻炼相关任务\n";
        assert_eq!(plumb_format::format(source).unwrap(), source);
        let mut workspace = Workspace::new();
        workspace.insert("减肥.plumb", 6, source);

        let operation = workspace
            .set_task_status_by_id(
                "减肥.plumb",
                "控制饮食-2026-07-20",
                TaskStatus::Done,
                "2026-07-21T18:01:12+08:00",
            )
            .unwrap();
        assert_eq!(operation.document_changes[0].edits.len(), 1);
        let edit = &operation.document_changes[0].edits[0];
        let mut edited = source.to_string();
        edited.replace_range(edit.range.clone(), &edit.new_text);

        assert!(edited.contains("`: done 2026-07-21T18:01:12+08:00"));
        assert!(edited.contains("`: prev #控制饮食-2026-07-20"));
        assert!(edited.contains("`# 锻炼相关任务"));
        assert_eq!(edited.matches("`: priority -5").count(), 2);
        assert_eq!(plumb_format::format(&edited).unwrap(), edited);
    }

    #[test]
    fn inserts_metadata_with_revision_and_escaped_title() {
        let mut workspace = Workspace::new();
        workspace.insert("notes/my`note.plumb", 7, "`# Section\n");

        let edit = workspace
            .insert_metadata(
                "notes/my`note.plumb",
                0,
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
            "{\n `: title my``note\n `: created 2026-07-19T12:34:56+08:00\n}\n\n"
        );
    }

    #[test]
    fn inserts_formatted_metadata_into_an_empty_document() {
        let mut workspace = Workspace::new();
        workspace.insert("notes/empty.plumb", 11, "");

        let edit = workspace
            .insert_metadata("notes/empty.plumb", 0, "empty", "2026-07-22T12:34:56+08:00")
            .unwrap();

        let document = &edit.document_changes[0];
        assert_eq!(document.expected_revision, 11);
        assert_eq!(document.edits[0].range, 0..0);
        assert_eq!(
            document.edits[0].new_text,
            "{\n `: title empty\n `: created 2026-07-22T12:34:56+08:00\n}\n"
        );
        assert_eq!(
            plumb_format::format(&document.edits[0].new_text).unwrap(),
            document.edits[0].new_text
        );
    }

    #[test]
    fn metadata_insertion_preserves_crlf() {
        let mut workspace = Workspace::new();
        workspace.insert("note.plumb", 1, "First\r\nSecond\r\n");

        let edit = workspace
            .insert_metadata("note.plumb", 0, "note", "2026-07-19T12:34:56+08:00")
            .unwrap();

        assert_eq!(
            edit.document_changes[0].edits[0].new_text,
            "{\r\n `: title note\r\n `: created 2026-07-19T12:34:56+08:00\r\n}\r\n\r\n"
        );
    }

    #[test]
    fn metadata_insertion_rejects_existing_or_invalid_metadata_target() {
        let mut workspace = Workspace::new();
        workspace.insert("existing.plumb", 1, "{\n  `: title Existing\n}\n");
        assert_eq!(
            workspace.insert_metadata("existing.plumb", 0, "existing", "created"),
            Err(MetadataInsertError::MetadataAlreadyExists)
        );

        workspace.insert("invalid.plumb", 2, "`node{key=a key=b} Broken\n");
        assert_eq!(
            workspace.insert_metadata("invalid.plumb", 0, "invalid", "created"),
            Err(MetadataInsertError::StaleOrInvalidDocument)
        );
        assert_eq!(
            workspace.insert_metadata("missing.plumb", 0, "missing", "created"),
            Err(MetadataInsertError::StaleOrInvalidDocument)
        );
    }

    #[test]
    fn metadata_insertion_requires_cursor_at_document_start() {
        let mut workspace = Workspace::new();
        workspace.insert("doc.plumb", 1, "`# Section\n");
        // Cursor at the very first byte: offered.
        assert!(workspace
            .insert_metadata("doc.plumb", 0, "doc", "2026-07-19T12:34:56+08:00")
            .is_ok());
        // Cursor past the first non-whitespace byte: rejected.
        assert_eq!(
            workspace.insert_metadata("doc.plumb", 3, "doc", "2026-07-19T12:34:56+08:00"),
            Err(MetadataInsertError::CursorNotAtDocumentStart)
        );

        // A cursor inside leading blank lines still counts as the document start.
        workspace.insert("blank.plumb", 2, "\n\n`# Section\n");
        assert!(workspace
            .insert_metadata("blank.plumb", 2, "blank", "2026-07-19T12:34:56+08:00")
            .is_ok());
    }

    #[test]
    fn resolves_event_task_associations_and_queries_time_ranges() {
        let mut workspace = Workspace::new();
        workspace.insert(
            "tasks.plumb",
            1,
            "`task Write {\n  `@ write\n}\n\n`node Plain {\n  `@ plain\n}\n",
        );
        let events = "{\n  `: date 2026-07-30\n  `: timezone +08:00\n}\n\n`event 10:30 Early {\n  `: timezone +05:00\n}\n`event 11:00 `->[Write]{`:[to tasks.plumb#write]} {\n}\n`event 12:00 `->[Write]{`:[to tasks.plumb#write]} {\n  `: tasks \n}\n`event 14:00--15:00 Review {\n  `@ review\n  `: uid review@example\n  `: tasks tasks.plumb#write\n}\n`event 15:00 Point {\n  `: tasks tasks.plumb#plain missing.plumb#task bad\n}\n";
        workspace.insert("events.plumb", 2, events);

        let target = TaskRef {
            path: PathBuf::from("tasks.plumb"),
            id: "write".to_string(),
        };
        let associated = workspace.events_for_task(&target);
        assert_eq!(associated.len(), 3);
        assert_eq!(
            associated
                .iter()
                .map(|event| event.event.title.as_str())
                .collect::<Vec<_>>(),
            ["Write", "Write", "Review"]
        );

        let day_start = DateTime::parse_from_rfc3339("2026-07-30T05:00:00Z").unwrap();
        let day_end = DateTime::parse_from_rfc3339("2026-07-30T08:00:00Z").unwrap();
        assert_eq!(
            workspace
                .events_overlapping(day_start, day_end)
                .iter()
                .map(|event| event.event.title.as_str())
                .collect::<Vec<_>>(),
            ["Early", "Review", "Point"]
        );

        let start = DateTime::parse_from_rfc3339("2026-07-30T14:30:00+08:00").unwrap();
        let end = DateTime::parse_from_rfc3339("2026-07-30T15:01:00+08:00").unwrap();
        assert_eq!(
            workspace
                .events_overlapping(start, end)
                .iter()
                .map(|event| event.event.title.as_str())
                .collect::<Vec<_>>(),
            ["Review", "Point"]
        );

        let reference_offset = events.find("tasks.plumb#write").unwrap();
        assert!(matches!(
            workspace.reference_target_at("events.plumb", reference_offset),
            Some(ResolvedTarget::Document { ref path }) if path == Path::new("tasks.plumb")
        ));
        assert_eq!(workspace.references_to("tasks.plumb", "write").len(), 5);
        assert_eq!(
            workspace.referenced_documents_from("events.plumb"),
            [PathBuf::from("tasks.plumb")]
        );

        let codes = workspace
            .diagnostics("events.plumb")
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();
        assert!(codes.contains(&"event.target-not-task"), "{codes:?}");
        assert!(codes.contains(&"event.unresolved-task-path"), "{codes:?}");
        assert!(codes.contains(&"event.invalid-task-reference"), "{codes:?}");

        let filtered = workspace
            .search_records_filtered(
                Path::new(""),
                Some(SearchRecordKind::Event),
                "review",
                20,
                DateTime::parse_from_rfc3339("2026-07-30T00:00:00Z").unwrap(),
                Some("uid == 'review@example' && when == '14:00--15:00' && start < timestamp('2026-07-30T07:00:00Z')"),
            )
            .unwrap();
        assert_eq!(filtered.items.len(), 1);
        assert_eq!(filtered.items[0].kind, SearchRecordKind::Event);
        assert_eq!(filtered.items[0].title, "Review");
        assert_eq!(
            filtered.items[0]
                .tasks
                .as_ref()
                .unwrap()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["tasks.plumb#write"]
        );

        let point = workspace
            .search_records_filtered(
                Path::new(""),
                Some(SearchRecordKind::Event),
                "point",
                20,
                DateTime::parse_from_rfc3339("2026-07-30T00:00:00Z").unwrap(),
                Some("at == timestamp('2026-07-30T07:00:00Z')"),
            )
            .unwrap();
        assert_eq!(point.items.len(), 1);
        assert_eq!(
            point.items[0].at.as_deref(),
            Some("2026-07-30T15:00:00+08:00")
        );
    }

    #[test]
    fn event_task_associations_binary_search_source_ordered_links() {
        let mut workspace = Workspace::new();
        workspace.insert("tasks.plumb", 1, "`task Write {\n  `@ write\n}\n");
        workspace.insert(
            "events.plumb",
            2,
            "{\n  `: date 2026-08-11\n  `: timezone +08:00\n}\n\n`->[Before]{`:[to tasks.plumb#write]}\n\n`event 10:00 Outer `->[Outer]{`:[to tasks.plumb#write]}\n  `event 11:00 Nested `->[Nested]{`:[to tasks.plumb#write]}\n\n`->[After]{`:[to tasks.plumb#write]}\n",
        );

        let output = workspace.current_output(Path::new("events.plumb")).unwrap();
        let outer = &output.events.events[0];
        let nested = &output.events.events[1];

        assert_eq!(
            workspace.event_task_references("events.plumb", outer).len(),
            2
        );
        assert_eq!(
            workspace
                .event_task_references("events.plumb", nested)
                .len(),
            1
        );
    }

    #[test]
    fn parses_reduced_precision_event_shorthand() {
        let now = DateTime::parse_from_rfc3339("2026-08-01T08:00:00+08:00").unwrap();
        for (source, at) in [
            ("11 relax: phone", "2026-08-01T11:00:00+08:00"),
            ("11:10 relax: phone", "2026-08-01T11:10:00+08:00"),
            ("11:10:24 relax: phone", "2026-08-01T11:10:24+08:00"),
            ("2026-05-21T11 relax: phone", "2026-05-21T11:00:00+08:00"),
            ("2026-05-21T11:10 relax: phone", "2026-05-21T11:10:00+08:00"),
            (
                "2026-05-21T11:10:24 relax: phone",
                "2026-05-21T11:10:24+08:00",
            ),
        ] {
            let input = parse_event_shorthand(source, now).unwrap();
            assert_eq!(input.title, "relax: phone");
            assert_eq!(input.at.as_deref(), Some(at), "{source}");
            assert!(input.start.is_none());
            assert!(input.end.is_none());
        }

        let interval = parse_event_shorthand("2026-05-21T11--11:20 review", now).unwrap();
        assert_eq!(interval.start.as_deref(), Some("2026-05-21T11:00:00+08:00"));
        assert_eq!(interval.end.as_deref(), Some("2026-05-21T11:20:00+08:00"));
        assert!(interval.at.is_none());

        let multi_day = parse_event_shorthand("2026-05-21T11--2026-05-23T11 review", now).unwrap();
        assert_eq!(
            multi_day.start.as_deref(),
            Some("2026-05-21T11:00:00+08:00")
        );
        assert_eq!(multi_day.end.as_deref(), Some("2026-05-23T11:00:00+08:00"));
    }

    #[test]
    fn rejects_ambiguous_or_invalid_event_shorthand() {
        let now = DateTime::parse_from_rfc3339("2026-08-01T08:00:00+08:00").unwrap();
        for source in [
            "11",
            "9 meeting",
            "09:5 meeting",
            "09:05:7 meeting",
            "24 meeting",
            "2026-02-30T11 meeting",
            "2026-05-21 11 meeting",
            "11am meeting",
            "11--2026-08-01T12:00:00Z meeting",
            "11--2026-08-01T12:00:00+08:00 meeting",
        ] {
            assert_eq!(
                parse_event_shorthand(source, now),
                Err(EventShorthandError::InvalidShorthand),
                "{source}"
            );
        }
        assert_eq!(
            parse_event_shorthand("11:20--11:20 meeting", now),
            Err(EventShorthandError::InvalidInterval)
        );
        assert_eq!(
            parse_event_shorthand("2026-08-02T11:20--2026-08-01T11:20 meeting", now),
            Err(EventShorthandError::InvalidInterval)
        );
        let cross_midnight = parse_event_shorthand("23:40--00:00 meeting", now).unwrap();
        assert_eq!(
            cross_midnight.end.as_deref(),
            Some("2026-08-02T00:00:00+08:00")
        );
    }

    #[test]
    fn converts_event_shorthand_list_item_in_place() {
        let source = "`- 2026-05-21T11:10--11:20 relax: phone\n";
        let mut workspace = Workspace::new();
        workspace.insert("agenda.plumb", 7, source);
        let now = DateTime::parse_from_rfc3339("2026-08-01T08:00:00+08:00").unwrap();
        let operation = workspace
            .convert_event_shorthand("agenda.plumb", source.find("relax").unwrap(), now)
            .unwrap();
        assert_eq!(operation.document_changes[0].expected_revision, 7);
        let converted = apply_text_edits(
            source.to_string(),
            operation.document_changes[0].edits.clone(),
        )
        .unwrap();
        assert!(converted.contains("`event"), "{converted}");
        assert!(!converted.contains("#e0001"), "{converted}");
        assert!(!converted.contains("event-uids"), "{converted}");
        assert!(converted.contains("`: date 2026-05-21"));
        assert!(converted.contains("`: timezone +08:00"));
        assert!(converted.contains("`event 11:10--11:20 relax: phone"));
        assert!(!converted.contains("start="));
        assert!(!converted.contains("end="));
        assert_eq!(plumb_format::format(&converted).unwrap(), converted);

        // Existing id/classes are preserved and the schedule remains the first head argument.
        let kept_source = "`- 11:00--11:20 review {\n  `@ mine\n  `- kind\n}\n";
        workspace.insert("keep.plumb", 8, kept_source);
        let kept = apply_single_edit(
            kept_source,
            &workspace
                .convert_event_shorthand("keep.plumb", 5, now)
                .unwrap(),
        );
        assert!(kept.contains("`@ mine"), "{kept}");
        assert!(kept.contains("`- kind"), "{kept}");
        assert!(kept.contains("`event"), "{kept}");
        assert!(kept.contains("`event 11:00--11:20 review {\n"), "{kept}");

        // Parsed and verbatim inline structure survives prefix removal.
        let rich_source =
            "`- 11 wheel: distinguish `\"nix develop\"{`:[language sh]} and `*[normal] shell\n";
        workspace.insert("markup.plumb", 9, rich_source);
        let rich = apply_single_edit(
            rich_source,
            &workspace
                .convert_event_shorthand("markup.plumb", 3, now)
                .unwrap(),
        );
        assert!(
            rich.contains("`event 11:00 wheel: distinguish `\"nix develop\"{`:[language sh]} and `*[normal] shell"),
            "{rich}"
        );

        // A list item that is already an event is left alone.
        workspace.insert("done.plumb", 10, "`event 11:00--11:20 review {\n}\n");
        assert_eq!(
            workspace.convert_event_shorthand("done.plumb", 5, now),
            Err(EventShorthandError::EventAlreadyExists)
        );

        // A plain paragraph (no list marker) no longer offers the action.
        workspace.insert("plain.plumb", 11, "11:00--11:20 review\n");
        assert_eq!(
            workspace.convert_event_shorthand("plain.plumb", 3, now),
            Err(EventShorthandError::ListItemNotFound)
        );
    }

    #[test]
    fn converts_selected_event_shorthands_in_one_edit() {
        let source = "{\n  `: date 2026-08-01\n  `: timezone +08:00\n}\n\n`event 09:00 Existing {\n  `@ e0015\n}\n\n`- 10:00--10:20 first\n`- ordinary item\n`- 10:20--10:30 second `\"code\"\n";
        let mut workspace = Workspace::new();
        workspace.insert("agenda.plumb", 9, source);
        let now = DateTime::parse_from_rfc3339("2026-08-03T08:00:00+09:00").unwrap();
        let start = source.find("10:00").unwrap();
        let end = source.len();
        let operation = workspace
            .convert_event_shorthands("agenda.plumb", start..end, now)
            .unwrap();
        let converted = apply_text_edits(
            source.to_string(),
            operation.document_changes[0].edits.clone(),
        )
        .unwrap();
        assert_eq!(converted.matches("`event").count(), 3, "{converted}");
        assert!(!converted.contains("event-uids"), "{converted}");
        assert!(converted.contains("`event 10:00--10:20 first"));
        assert!(converted.contains("`event 10:20--10:30 second `\"code\""));
        assert!(converted.contains("`- ordinary item"));
        assert!(!converted.contains("date=2026-08-01"));
        assert!(!converted.contains("timezone=\"+08:00\""));
        workspace.insert("agenda.plumb", 10, converted);
        let events = &workspace
            .current_output(Path::new("agenda.plumb"))
            .unwrap()
            .events
            .events;
        assert_eq!(
            events[1].start.as_ref().unwrap().value,
            "2026-08-01T10:00:00+08:00"
        );
    }

    #[test]
    fn infers_open_event_ends_from_adjacent_selected_siblings() {
        let source = "{\n  `: date 2026-08-01\n  `: timezone +08:00\n}\n\n`- 18:00-- 事件 1\n`- 18:30-- 事件 2\n";
        let mut workspace = Workspace::new();
        workspace.insert("agenda.plumb", 1, source);
        let now = DateTime::parse_from_rfc3339("2026-08-03T08:00:00+09:00").unwrap();
        let operation = workspace
            .convert_event_shorthands(
                "agenda.plumb",
                source.find("18:00").unwrap()..source.len(),
                now,
            )
            .unwrap();
        let converted = apply_text_edits(
            source.to_string(),
            operation.document_changes[0].edits.clone(),
        )
        .unwrap();
        assert!(
            converted.contains("`event 18:00--18:30 事件 1"),
            "{converted}"
        );
        assert!(converted.contains("`- 18:30-- 事件 2"), "{converted}");
        assert_eq!(converted.matches("`event").count(), 1, "{converted}");

        workspace.insert("agenda.plumb", 2, source);
        let first = workspace
            .convert_event_shorthand("agenda.plumb", source.find("事件 1").unwrap(), now)
            .unwrap();
        let first_converted =
            apply_text_edits(source.to_string(), first.document_changes[0].edits.clone()).unwrap();
        assert!(
            first_converted.contains("`event 18:00--18:30 事件 1"),
            "{first_converted}"
        );
        assert_eq!(
            workspace.convert_event_shorthand("agenda.plumb", source.find("事件 2").unwrap(), now,),
            Err(EventShorthandError::InvalidShorthand)
        );

        let chain = "{\n  `: date 2026-08-01\n  `: timezone +08:00\n}\n\n`- 18:00-- first\n`- 18:30-- second\n`- 19:00--20:00 third\n";
        workspace.insert("chain.plumb", 3, chain);
        let chained = apply_text_edits(
            chain.to_string(),
            workspace
                .convert_event_shorthands(
                    "chain.plumb",
                    chain.find("18:00").unwrap()..chain.len(),
                    now,
                )
                .unwrap()
                .document_changes[0]
                .edits
                .clone(),
        )
        .unwrap();
        assert!(chained.contains("`event 18:00--18:30 first"), "{chained}");
        assert!(chained.contains("`event 18:30--19:00 second"), "{chained}");
        assert!(chained.contains("`event 19:00--20:00 third"), "{chained}");
        assert_eq!(chained.matches("`event").count(), 3, "{chained}");

        let interrupted = "`- 18:00-- first\n`- ordinary\n`- 18:30 next\n";
        workspace.insert("interrupted.plumb", 4, interrupted);
        assert_eq!(
            workspace.convert_event_shorthand(
                "interrupted.plumb",
                interrupted.find("first").unwrap(),
                now,
            ),
            Err(EventShorthandError::InvalidShorthand)
        );
    }

    #[test]
    fn creates_updates_and_deletes_events_with_guarded_canonical_edits() {
        let mut workspace = Workspace::new();
        let source = "`# Agenda\n";
        workspace.insert("agenda.plumb", 7, source);
        let created = workspace
            .create_event(
                "agenda.plumb",
                &EventInput {
                    title: "Review".to_string(),
                    at: None,
                    start: Some("2026-07-30T14:00:00+08:00".to_string()),
                    end: Some("2026-07-30T15:00:00+08:00".to_string()),
                    tasks: vec!["tasks.plumb#write".to_string()],
                },
            )
            .unwrap();
        assert_eq!(created.document_changes[0].expected_revision, 7);
        let created_source = apply_single_edit(source, &created);
        assert!(created_source.contains("`event"), "{created_source}");
        assert!(!created_source.contains("#e0001"), "{created_source}");
        assert!(!created_source.contains("event-uids"), "{created_source}");
        assert!(
            created_source.contains("`event 14:00--15:00 Review"),
            "{created_source}"
        );
        assert_eq!(
            plumb_format::format(&created_source).unwrap(),
            created_source
        );

        let multi_day = workspace
            .create_event(
                "agenda.plumb",
                &EventInput {
                    title: "Conference".to_string(),
                    at: None,
                    start: Some("2026-07-30T14:00:00+08:00".to_string()),
                    end: Some("2026-08-02T14:00:00+08:00".to_string()),
                    tasks: Vec::new(),
                },
            )
            .unwrap();
        let multi_day_source = apply_single_edit(source, &multi_day);
        assert!(
            multi_day_source.contains("`event 14:00--2026-08-02T14:00 Conference"),
            "{multi_day_source}"
        );
        let multi_day_parsed = plumb_syntax::parse(multi_day_source);
        assert!(
            multi_day_parsed.is_valid(),
            "{:?}",
            multi_day_parsed.diagnostics
        );

        workspace.insert("agenda.plumb", 8, created_source.clone());
        let event = workspace
            .current_output(Path::new("agenda.plumb"))
            .unwrap()
            .events
            .events[0]
            .clone();
        let updated = workspace
            .update_event(
                "agenda.plumb",
                event.range.clone(),
                &EventInput {
                    title: "Updated review".to_string(),
                    at: Some("2026-07-30T16:00:00+08:00".to_string()),
                    start: None,
                    end: None,
                    tasks: Vec::new(),
                },
            )
            .unwrap();
        let updated_source = apply_single_edit(&created_source, &updated);
        assert!(updated_source.contains("Updated review"));
        assert!(
            updated_source.contains("`event 16:00 Updated review"),
            "{updated_source}"
        );
        assert!(!updated_source.contains("tasks.plumb#write"));

        workspace.insert("agenda.plumb", 9, updated_source.clone());
        let updated_event = workspace
            .current_output(Path::new("agenda.plumb"))
            .unwrap()
            .events
            .events[0]
            .clone();
        let deleted = workspace
            .delete_event("agenda.plumb", updated_event.range)
            .unwrap();
        let deleted_source = apply_text_edits(
            updated_source.clone(),
            deleted.document_changes[0].edits.clone(),
        )
        .unwrap();
        assert!(!deleted_source.contains("event-uids"));
        assert!(deleted_source.contains("`# Agenda"));
        assert!(!deleted_source.contains("Updated review"));

        workspace.insert("agenda.plumb", 10, deleted_source.clone());
        let recreated = workspace
            .create_event(
                "agenda.plumb",
                &EventInput {
                    title: "Next".to_string(),
                    at: Some("2026-07-30T17:00:00+08:00".to_string()),
                    start: None,
                    end: None,
                    tasks: Vec::new(),
                },
            )
            .unwrap();
        let recreated_source = apply_text_edits(
            deleted_source.clone(),
            recreated.document_changes[0].edits.clone(),
        )
        .unwrap();
        assert!(recreated_source.contains("`event"), "{recreated_source}");
        assert!(
            recreated_source.contains("`event 17:00 Next"),
            "{recreated_source}"
        );

        assert_eq!(
            workspace.create_event(
                "agenda.plumb",
                &EventInput {
                    title: "Bad".to_string(),
                    at: None,
                    start: Some("2026-07-30T16:00:00+08:00".to_string()),
                    end: Some("2026-07-30T15:00:00+08:00".to_string()),
                    tasks: Vec::new(),
                },
            ),
            Err(EventEditError::InvalidInterval)
        );
    }

    #[test]
    fn updating_an_event_preserves_semantic_uid_and_opaque_when_property() {
        let source = "{\n  `: date 2026-07-30\n  `: timezone +08:00\n}\n\n`event 14:00 Review {\n  `@ review\n  `: uid legacy@example\n  `: when 14:00\n}\n";
        let mut workspace = Workspace::new();
        workspace.insert("agenda.plumb", 1, source);
        let event = workspace
            .current_output(Path::new("agenda.plumb"))
            .unwrap()
            .events
            .events[0]
            .clone();
        let operation = workspace
            .update_event(
                "agenda.plumb",
                event.range,
                &EventInput {
                    title: "Updated".to_string(),
                    at: Some("2026-07-30T15:00:00+08:00".to_string()),
                    start: None,
                    end: None,
                    tasks: Vec::new(),
                },
            )
            .unwrap();
        let updated = apply_text_edits(
            source.to_string(),
            operation.document_changes[0].edits.clone(),
        )
        .unwrap();
        assert!(updated.contains("`@ review"), "{updated}");
        assert_eq!(updated.matches("`event").count(), 1, "{updated}");
        assert!(updated.contains("`: uid legacy@example"), "{updated}");
        assert!(updated.contains("`: when 14:00"), "{updated}");
        assert!(updated.contains("`event 15:00 Updated"), "{updated}");
    }

    #[test]
    fn creates_nested_tasks_and_updates_fields_without_losing_owned_content() {
        let mut workspace = Workspace::new();
        let source = "`task Parent {\n  `@ parent\n  `: custom keep\n  `: created 2026-07-01T09:00:00Z\n}\n\n      `note Keep details\n\n`task Other {\n  `@ other\n}\n\n`# Following\n";
        workspace.insert("tasks.plumb", 4, source);
        let parent = workspace
            .current_output(Path::new("tasks.plumb"))
            .unwrap()
            .tasks
            .tasks[0]
            .clone();
        let created = workspace
            .create_task(
                "tasks.plumb",
                &TaskAuthoringInput {
                    title: "Nested".to_string(),
                    due: Some("2026-08-01T10:00:00Z".to_string()),
                    priority: Some(-2),
                    ..TaskAuthoringInput::default()
                },
                &TaskPlacement {
                    parent: Some(parent.range.clone()),
                    after: None,
                },
                "2026-07-31T10:00:00Z",
            )
            .unwrap();
        assert_eq!(created.document_changes[0].expected_revision, 4);
        let created_source = apply_single_edit(source, &created);
        assert!(created_source.contains("`task"), "{created_source}");
        assert!(created_source.contains("`@ task-"), "{created_source}");
        assert!(created_source.contains("`: priority -2"));
        assert!(created_source.contains("`note Keep details"));

        workspace.insert("tasks.plumb", 5, created_source.clone());
        let parent = workspace
            .current_output(Path::new("tasks.plumb"))
            .unwrap()
            .tasks
            .tasks[0]
            .clone();
        let updated = workspace
            .update_task(
                "tasks.plumb",
                parent.range,
                &TaskAuthoringInput {
                    title: "Renamed parent".to_string(),
                    due: Some("2026-09-01T10:00:00Z".to_string()),
                    depends: vec!["#other".to_string()],
                    ..TaskAuthoringInput::default()
                },
                "2026-07-31T11:00:00Z",
            )
            .unwrap();
        let updated_source = apply_single_edit(&created_source, &updated);
        assert!(updated_source.contains("`: custom keep"));
        assert!(updated_source.contains("`@ parent"));
        assert!(updated_source.contains("`: created 2026-07-01T09:00:00Z"));
        assert!(updated_source.contains("`note Keep details"));
        assert!(updated_source.contains("Nested"));
        assert!(updated_source.contains("Renamed parent"));
        assert!(!updated_source.contains("priority=-2\n`# Following"));

        workspace.insert("tasks.plumb", 6, updated_source.clone());
        let parent = workspace
            .current_output(Path::new("tasks.plumb"))
            .unwrap()
            .tasks
            .tasks[0]
            .clone();
        let patched = workspace
            .update_task_patch(
                "tasks.plumb",
                parent.range,
                &TaskAuthoringPatch {
                    priority: Some(Some(9)),
                    ..TaskAuthoringPatch::default()
                },
                "2026-07-31T12:00:00Z",
            )
            .unwrap();
        let patched_source = apply_single_edit(&updated_source, &patched);
        assert!(patched_source.contains("`: priority 9"));
        assert!(patched_source.contains("`: due 2026-09-01T10:00:00Z"));
        assert!(patched_source.contains("#other"), "{patched_source}");
    }

    #[test]
    fn task_authoring_rejects_invalid_fields_and_placements() {
        let mut workspace = Workspace::new();
        workspace.insert("tasks.plumb", 1, "`# Tasks\n");
        let invalid = |input: TaskAuthoringInput| {
            workspace.create_task(
                "tasks.plumb",
                &input,
                &TaskPlacement::default(),
                "2026-07-31T10:00:00Z",
            )
        };
        assert_eq!(
            invalid(TaskAuthoringInput {
                title: "Bad datetime".to_string(),
                due: Some("tomorrow".to_string()),
                ..TaskAuthoringInput::default()
            }),
            Err(TaskAuthoringError::InvalidDatetime)
        );
        assert_eq!(
            invalid(TaskAuthoringInput {
                title: "Bad recurrence".to_string(),
                recur: Some("P0D".to_string()),
                ..TaskAuthoringInput::default()
            }),
            Err(TaskAuthoringError::InvalidRecurrence)
        );
        assert_eq!(
            invalid(TaskAuthoringInput {
                title: "Bad reference".to_string(),
                depends: vec!["missing-hash".to_string()],
                ..TaskAuthoringInput::default()
            }),
            Err(TaskAuthoringError::InvalidReference)
        );
        assert_eq!(
            invalid(TaskAuthoringInput {
                title: "Missing dependency".to_string(),
                depends: vec!["#missing".to_string()],
                ..TaskAuthoringInput::default()
            }),
            Err(TaskAuthoringError::UnresolvedReference)
        );

        workspace.insert(
            "tasks.plumb",
            2,
            "`task A {\n  `@ a\n  `: depends #b\n}\n`task B {\n  `@ b\n}\n",
        );
        let b = workspace
            .current_output(Path::new("tasks.plumb"))
            .unwrap()
            .tasks
            .tasks[1]
            .clone();
        assert_eq!(
            workspace.update_task_patch(
                "tasks.plumb",
                b.range,
                &TaskAuthoringPatch {
                    depends: Some(vec!["#a".to_string()]),
                    ..TaskAuthoringPatch::default()
                },
                "2026-07-31T10:00:00Z",
            ),
            Err(TaskAuthoringError::DependencyCycle)
        );
    }

    #[test]
    fn moves_task_subtrees_within_and_between_parents() {
        let mut workspace = Workspace::new();
        let source = plumb_format::format(
            "`task Left {\n  `@ left\n}\n\n      `task A {\n        `@ a\n      }\n\n            `note A details\n\n      `task B {\n        `@ b\n      }\n\n`task Right {\n  `@ right\n}\n",
        )
        .unwrap();
        workspace.insert("tasks.plumb", 1, &source);
        let tasks = &workspace
            .current_output(Path::new("tasks.plumb"))
            .unwrap()
            .tasks
            .tasks;
        assert_eq!(
            tasks
                .iter()
                .map(|task| (task.id.as_ref().unwrap().value.as_str(), task.depth))
                .collect::<Vec<_>>(),
            [("left", 0), ("a", 1), ("b", 1), ("right", 0)]
        );
        let by_id = |id: &str| {
            tasks
                .iter()
                .find(|task| task.id.as_ref().is_some_and(|field| field.value == id))
                .unwrap()
                .range
                .clone()
        };
        let reordered = workspace
            .move_task(
                "tasks.plumb",
                by_id("a"),
                &TaskPlacement {
                    parent: Some(by_id("left")),
                    after: Some(by_id("b")),
                },
            )
            .unwrap();
        let reordered_source = apply_document_edit(source, "tasks.plumb", 1, reordered).unwrap();
        assert!(reordered_source.find("`@ b").unwrap() < reordered_source.find("`@ a").unwrap());
        assert!(reordered_source.contains("`note A details"));
        assert!(parse(&reordered_source).is_valid(), "{reordered_source}");
        assert!(
            reordered_source.contains("`task Right {\n `@ right\n}\n"),
            "{reordered_source}"
        );

        workspace.insert("tasks.plumb", 2, reordered_source.clone());
        let tasks = &workspace
            .current_output(Path::new("tasks.plumb"))
            .unwrap()
            .tasks
            .tasks;
        let range = |id: &str| {
            tasks
                .iter()
                .find(|task| task.id.as_ref().is_some_and(|field| field.value == id))
                .unwrap()
                .range
                .clone()
        };
        let reparented = workspace
            .move_task(
                "tasks.plumb",
                range("a"),
                &TaskPlacement {
                    parent: Some(range("right")),
                    after: None,
                },
            )
            .unwrap();
        let reparented_source =
            apply_document_edit(reordered_source.clone(), "tasks.plumb", 2, reparented).unwrap();
        workspace.insert("tasks.plumb", 3, reparented_source.clone());
        let tasks = &workspace
            .current_output(Path::new("tasks.plumb"))
            .unwrap()
            .tasks
            .tasks;
        let a = tasks
            .iter()
            .find(|task| task.id.as_ref().is_some_and(|field| field.value == "a"))
            .unwrap();
        let right = tasks
            .iter()
            .find(|task| task.id.as_ref().is_some_and(|field| field.value == "right"))
            .unwrap();
        assert_eq!(a.depth, right.depth + 1, "{reparented_source}");
        assert!(reparented_source.contains("`note A details"));

        let updated = workspace
            .update_task_patch(
                "tasks.plumb",
                a.range.clone(),
                &TaskAuthoringPatch {
                    due: Some(Some("2026-08-15T02:30:00Z".to_string())),
                    priority: Some(Some(-7)),
                    ..TaskAuthoringPatch::default()
                },
                "2026-07-31T10:00:00Z",
            )
            .unwrap();
        let updated_source =
            apply_document_edit(reparented_source, "tasks.plumb", 3, updated).unwrap();
        let parsed = parse(&updated_source);
        assert!(
            parsed.is_valid(),
            "{updated_source}\n{:?}",
            parsed.diagnostics
        );
        let formatted = plumb_format::format(&updated_source).expect("updated task source formats");
        assert_eq!(formatted, updated_source);
    }
}
