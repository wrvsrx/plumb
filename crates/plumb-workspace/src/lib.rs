use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveTime, SecondsFormat, TimeZone, Timelike};
pub use plumb_edit::{apply_text_edits, TextEdit};
use plumb_edit::{
    remove_block as remove_syntax_block, replace_owned_block, replace_owned_blocks,
    AttributePosition, EditSession, OwnedAttribute, OwnedBlock, OwnedInline,
};
use plumb_semantics::analyze_document;
use plumb_semantics::{
    parse_task_reference_target, AnchorRecord, DocumentOutput, EventRecord, LinkCompletionContext,
    LinkRecord, LinkSpelling, LinkTarget, MetadataBlock, MetadataOutput, MetadataValue,
    TaskDependency, TaskDependencyCompletionContext, TaskRecord, TaskReferenceTarget, TaskState,
};
#[cfg(test)]
use plumb_semantics::{
    EventTitleCompletionContext, FileCompletionContext, ImageCompletionContext, TaskStatus,
};
#[cfg(test)]
use plumb_syntax::parse;
use plumb_syntax::{
    Attributes, Block, Diagnostic, DiagnosticSeverity, ParsedBlock, ParsedDocument,
};

mod bibliography;
mod cache;
mod completion;
mod documents;
mod index;
mod navigation;
mod scan;
mod search;
mod store;
mod task_query;
mod task_sort;
mod tasks;

#[cfg(test)]
use completion::TEST_EVENT_TITLE_COMPLETION_LIMIT as EVENT_TITLE_COMPLETION_LIMIT;

pub use bibliography::{
    load_bibliography, load_bibliography_sources, Bibliography, BibliographyRecord,
    BibliographyResolution,
};
pub use cache::{
    inspect_cache_namespace, prune_cache_namespace, CacheNamespaceState, CacheNamespaceUsage,
    CachePruneOutcome,
};
pub use index::{
    BatchIndexError, BatchIndexFailure, BatchIndexOptions, BatchIndexResult, BatchIndexedDocument,
};
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
    WorkspaceSearchError,
};
pub use task_query::{
    TaskDocumentMetrics, TaskPage, TaskPageQuery, TaskPageQueryError, TaskQueryFilter,
    TaskQueryFilterGroup, WorkspaceTask,
};
pub use task_sort::{
    sort_task_records, sort_task_records_by, truncate_complete_task_documents, TaskSortFacts,
    TaskSortOrder,
};
use tasks::{
    adjust_path_after_removal, block_index_path, child_insertion_index, owned_at_path_mut,
    owned_authored_task, remove_owned_at_path, remove_owned_descendant, updated_owned_task,
    validate_task_authoring_input, TaskTargetResolution,
};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryCompleteness {
    Complete,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryProvenance {
    Memory,
    Persistent,
    PersistentWithOverlay,
}

#[derive(Debug)]
pub enum WorkspaceQueryError {
    Store(StoreError),
    Incomplete,
}

impl std::fmt::Display for WorkspaceQueryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Incomplete => formatter.write_str("workspace query result is incomplete"),
        }
    }
}

impl std::error::Error for WorkspaceQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Incomplete => None,
        }
    }
}

impl From<StoreError> for WorkspaceQueryError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryResult<T> {
    pub value: T,
    pub completeness: QueryCompleteness,
    pub provenance: QueryProvenance,
}

#[derive(Debug)]
pub enum WorkspaceOperationError<E> {
    Operation(E),
    Query(WorkspaceQueryError),
}

impl<E: std::fmt::Display> std::fmt::Display for WorkspaceOperationError<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Operation(error) => error.fmt(formatter),
            Self::Query(error) => error.fmt(formatter),
        }
    }
}

impl<E> std::error::Error for WorkspaceOperationError<E>
where
    E: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Operation(error) => Some(error),
            Self::Query(error) => Some(error),
        }
    }
}

impl From<RenameError> for WorkspaceOperationError<RenameError> {
    fn from(error: RenameError) -> Self {
        Self::Operation(error)
    }
}

impl From<WorkspaceQueryError> for WorkspaceOperationError<RenameError> {
    fn from(error: WorkspaceQueryError) -> Self {
        Self::Query(error)
    }
}

impl From<TaskAuthoringError> for WorkspaceOperationError<TaskAuthoringError> {
    fn from(error: TaskAuthoringError) -> Self {
        Self::Operation(error)
    }
}

impl From<WorkspaceQueryError> for WorkspaceOperationError<TaskAuthoringError> {
    fn from(error: WorkspaceQueryError) -> Self {
        Self::Query(error)
    }
}

impl<T> QueryResult<T> {
    pub fn is_complete(&self) -> bool {
        self.completeness == QueryCompleteness::Complete
    }

    pub fn require_complete(self) -> Result<T, WorkspaceQueryError> {
        self.is_complete()
            .then_some(self.value)
            .ok_or(WorkspaceQueryError::Incomplete)
    }
}

#[derive(Debug, Clone)]
pub struct VersionedDocumentOutput {
    pub revision: i64,
    pub output: Arc<DocumentOutput>,
}

#[derive(Debug, Clone)]
pub struct DocumentEntry {
    pub path: PathBuf,
    pub revision: i64,
    pub parsed: Arc<ParsedDocument>,
    pub current: Option<Arc<VersionedDocumentOutput>>,
    pub last_valid: Option<Arc<VersionedDocumentOutput>>,
}

#[derive(Debug, Clone)]
pub struct PendingDocumentAnalysis {
    path: PathBuf,
    revision: i64,
    parsed: Arc<ParsedDocument>,
}

#[derive(Debug)]
pub struct PreparedDocumentAnalysis {
    path: PathBuf,
    revision: i64,
    parsed: Arc<ParsedDocument>,
    output: Arc<DocumentOutput>,
}

impl PendingDocumentAnalysis {
    pub fn analyze(self) -> PreparedDocumentAnalysis {
        let output = analyze_document(
            self.parsed
                .valid_syntax()
                .expect("pending semantic analysis requires valid syntax"),
        );
        PreparedDocumentAnalysis {
            path: self.path,
            revision: self.revision,
            parsed: self.parsed,
            output: Arc::new(output),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentMetadataTarget {
    pub range: std::ops::Range<usize>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceEventCursor {
    pub sort_millis: Option<i64>,
    pub path: PathBuf,
    pub start: usize,
}

fn compare_workspace_events(left: &WorkspaceEvent, right: &WorkspaceEvent) -> std::cmp::Ordering {
    event_order_key(left).cmp(&event_order_key(right))
}

fn event_order_key(event: &WorkspaceEvent) -> (bool, Option<i64>, &Path, usize) {
    let millis = event
        .event
        .sort_datetime()
        .map(|value| value.timestamp_millis());
    (
        millis.is_none(),
        millis,
        &event.path,
        event.event.range.start,
    )
}

fn cursor_order_key(cursor: &WorkspaceEventCursor) -> (bool, Option<i64>, &Path, usize) {
    (
        cursor.sort_millis.is_none(),
        cursor.sort_millis,
        &cursor.path,
        cursor.start,
    )
}

fn event_after_cursor(event: &WorkspaceEvent, cursor: &WorkspaceEventCursor) -> bool {
    event_order_key(event) > cursor_order_key(cursor)
}

fn event_before_cursor(event: &WorkspaceEvent, cursor: &WorkspaceEventCursor) -> bool {
    event_order_key(event) < cursor_order_key(cursor)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspaceTaskKey {
    pub path: PathBuf,
    pub start: usize,
}

#[derive(Debug, Clone)]
pub struct WorkspaceDiagnosticContext {
    task_dependency_graph: HashMap<TaskRef, Vec<TaskRef>>,
}

#[derive(Debug, Default, Clone)]
pub struct Workspace {
    documents: HashMap<PathBuf, DocumentEntry>,
    disk_store: Option<SqliteSemanticStore>,
}

impl Workspace {
    fn open_paths(&self) -> Vec<PathBuf> {
        let mut paths = self.documents.keys().cloned().collect::<Vec<_>>();
        paths.sort();
        paths
    }

    pub fn active_task_keys(
        &self,
        now: DateTime<FixedOffset>,
    ) -> Result<QueryResult<Vec<WorkspaceTaskKey>>, WorkspaceQueryError> {
        let mut keys = Vec::new();
        for entry in self.documents.values() {
            let Some(current) = &entry.current else {
                continue;
            };
            for task in &current.output.tasks.tasks {
                let blocked = self.is_task_blocked_value(&entry.path, task)?;
                if matches!(
                    derive_task_workflow_state(task, blocked, now).0,
                    TaskWorkflowState::Ready | TaskWorkflowState::Blocked
                ) {
                    keys.push(WorkspaceTaskKey {
                        path: entry.path.clone(),
                        start: task.selection_range.start,
                    });
                }
            }
        }
        if let Some(store) = &self.disk_store {
            keys.extend(
                store
                    .active_tasks(now.timestamp_millis(), &self.open_paths())?
                    .into_iter()
                    .map(|stored| WorkspaceTaskKey {
                        path: stored.path,
                        start: stored.record.selection_range.start,
                    }),
            );
        }
        keys.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.start.cmp(&right.start))
        });
        Ok(self.query_result(keys))
    }

    pub fn task_keys_for_states(
        &self,
        states: &HashSet<TaskWorkflowState>,
        now: DateTime<FixedOffset>,
    ) -> Result<QueryResult<Vec<WorkspaceTaskKey>>, WorkspaceQueryError> {
        if states.len() == 2
            && states.contains(&TaskWorkflowState::Ready)
            && states.contains(&TaskWorkflowState::Blocked)
        {
            return self.active_task_keys(now);
        }
        let open = self.open_paths();
        let mut blocked = self
            .disk_store
            .as_ref()
            .map(|store| store.blocked_task_sources(&open))
            .transpose()?
            .unwrap_or_default()
            .into_iter()
            .map(|source| (source.path, source.start))
            .collect::<HashSet<_>>();
        for entry in self.documents.values() {
            let Some(current) = &entry.current else {
                continue;
            };
            for task in &current.output.tasks.tasks {
                if self.is_task_blocked_value(&entry.path, task)? {
                    blocked.insert((entry.path.clone(), task.range.start));
                }
            }
        }
        let mut keys = self
            .documents
            .values()
            .filter_map(|entry| entry.current.as_ref().map(|current| (entry, current)))
            .flat_map(|(entry, current)| {
                current
                    .output
                    .tasks
                    .tasks
                    .iter()
                    .filter(|task| {
                        states.contains(
                            &derive_task_workflow_state(
                                task,
                                blocked.contains(&(entry.path.clone(), task.range.start)),
                                now,
                            )
                            .0,
                        )
                    })
                    .map(|task| WorkspaceTaskKey {
                        path: entry.path.clone(),
                        start: task.selection_range.start,
                    })
            })
            .collect::<Vec<_>>();
        if let Some(store) = &self.disk_store {
            for stored in store.tasks(&open)? {
                let is_blocked = if open.is_empty() {
                    blocked.contains(&(stored.path.clone(), stored.record.range.start))
                } else {
                    self.is_task_blocked_value(&stored.path, &stored.record)?
                };
                if states.contains(&derive_task_workflow_state(&stored.record, is_blocked, now).0) {
                    keys.push(WorkspaceTaskKey {
                        path: stored.path,
                        start: stored.record.selection_range.start,
                    });
                }
            }
        }
        keys.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.start.cmp(&right.start))
        });
        Ok(self.query_result(keys))
    }

    pub fn resolve_link(
        &self,
        from: impl AsRef<Path>,
        link: &LinkRecord,
    ) -> Result<QueryResult<ResolvedTarget>, WorkspaceQueryError> {
        Ok(self.query_result(self.resolve_link_value(from.as_ref(), link)?))
    }

    fn resolve_link_value(
        &self,
        from: &Path,
        link: &LinkRecord,
    ) -> Result<ResolvedTarget, WorkspaceQueryError> {
        let from = normalize(from);
        Ok(match &link.target_kind {
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
                if self.contains_path(&target)? {
                    ResolvedTarget::Document { path: target }
                } else {
                    ResolvedTarget::UnresolvedPath { path: target }
                }
            }
            LinkTarget::Anchor { path, fragment } => {
                let target = path
                    .as_deref()
                    .map_or_else(|| from.clone(), |path| resolve_relative(&from, path));
                if !self.contains_path(&target)? {
                    return Ok(ResolvedTarget::UnresolvedPath { path: target });
                }
                let anchors = self.anchors_named(&target, fragment)?;
                let Some(anchor) = anchors.first() else {
                    return Ok(ResolvedTarget::UnresolvedAnchor {
                        path: target,
                        id: fragment.clone(),
                    });
                };
                if anchors.len() > 1 {
                    return Ok(ResolvedTarget::AmbiguousAnchor {
                        path: target,
                        id: fragment.clone(),
                    });
                }
                ResolvedTarget::Anchor {
                    path: target,
                    id: fragment.clone(),
                    anchor: anchor.clone(),
                }
            }
        })
    }

    pub fn link_at(&self, path: impl AsRef<Path>, offset: usize) -> Option<&LinkRecord> {
        self.current_output(path.as_ref())?
            .links
            .iter()
            .filter(|link| link.range.start <= offset && offset <= link.range.end)
            .max_by_key(|link| link.range.start)
    }

    pub fn document_metadata(&self, path: impl AsRef<Path>) -> Option<&MetadataBlock> {
        self.current_output(path.as_ref())?
            .metadata
            .metadata
            .as_ref()
    }

    pub fn document_metadata_target_at(
        &self,
        path: impl AsRef<Path>,
        offset: usize,
    ) -> Option<DocumentMetadataTarget> {
        let entry = self.get(path)?;
        entry.current.as_ref()?;
        if let Some(range) = entry.parsed.syntax.blocks.iter().find_map(|block| {
            let Block::Parsed(block) = block else {
                return None;
            };
            let is_metadata = block.mark.as_ref().is_some_and(|mark| mark.marker == "=");
            (is_metadata && block.range.start <= offset && offset < block.range.end)
                .then(|| block.range.clone())
        }) {
            return Some(DocumentMetadataTarget { range });
        }
        (offset == 0 && self.document_metadata(&entry.path).is_some())
            .then_some(DocumentMetadataTarget { range: 0..0 })
    }

    pub fn reference_target_at(
        &self,
        path: impl AsRef<Path>,
        offset: usize,
    ) -> Result<QueryResult<Option<ResolvedTarget>>, WorkspaceQueryError> {
        Ok(self.query_result(self.reference_target_at_value(path.as_ref(), offset)?))
    }

    fn reference_target_at_value(
        &self,
        path: &Path,
        offset: usize,
    ) -> Result<Option<ResolvedTarget>, WorkspaceQueryError> {
        let path = normalize(path);
        let Some(output) = self.current_output(&path) else {
            return Ok(None);
        };
        if let Some(link) = output
            .links
            .iter()
            .filter(|link| contains_inclusive(&link.range, offset))
            .max_by_key(|link| link.range.start)
        {
            let target = self.resolve_link_value(&path, link)?;
            if link
                .path_range
                .as_ref()
                .is_some_and(|range| contains_component(range, offset))
            {
                return Ok(Some(self.document_component_target(target)?));
            }
            return Ok(Some(target));
        }
        for task in &output.tasks.tasks {
            for (source, range, target) in task_reference_fields(task) {
                if !contains_inclusive(range, offset) {
                    continue;
                }
                let resolved = self.resolve_task_reference_target(&path, &target)?;
                let target_id = match &target {
                    TaskReferenceTarget::Internal { id }
                    | TaskReferenceTarget::External { id, .. } => id,
                    TaskReferenceTarget::Invalid => return Ok(Some(resolved)),
                };
                if task_reference_ranges(source, range, target_id)
                    .and_then(|(path_range, _)| path_range)
                    .as_ref()
                    .is_some_and(|range| contains_component(range, offset))
                {
                    return Ok(Some(self.document_component_target(resolved)?));
                }
                return Ok(Some(resolved));
            }
        }
        for event in &output.events.events {
            for reference in &self.event_task_references_in_output(&path, output, event)? {
                if !contains_inclusive(&reference.range, offset) {
                    continue;
                }
                let resolved = self.resolve_task_reference_target(&path, &reference.target)?;
                let target_id = match &reference.target {
                    TaskReferenceTarget::Internal { id }
                    | TaskReferenceTarget::External { id, .. } => id,
                    TaskReferenceTarget::Invalid => return Ok(Some(resolved)),
                };
                if task_reference_ranges(&reference.source, &reference.range, target_id)
                    .and_then(|(path_range, _)| path_range)
                    .as_ref()
                    .is_some_and(|range| contains_component(range, offset))
                {
                    return Ok(Some(self.document_component_target(resolved)?));
                }
                return Ok(Some(resolved));
            }
        }
        if let Some(image) = self.image_at(&path, offset) {
            return Ok(Some(self.resolve_image(&path, image)));
        }
        if let Some(file) = self.file_at(&path, offset) {
            return Ok(Some(self.resolve_file(&path, file)));
        }
        Ok(None)
    }

    pub fn target_at(
        &self,
        path: impl AsRef<Path>,
        offset: usize,
    ) -> Result<QueryResult<Option<ResolvedTarget>>, WorkspaceQueryError> {
        let path = normalize(path.as_ref());
        if self.current_output(&path).is_none() {
            return Ok(self.query_result(None));
        }
        if offset == 0 {
            return Ok(self.query_result(Some(ResolvedTarget::Document { path })));
        }
        if let Some(target) = self.reference_target_at_value(&path, offset)? {
            return Ok(self.query_result(Some(target)));
        }
        let target = self
            .anchor_at(&path, offset)
            .map(|anchor| ResolvedTarget::Anchor {
                path,
                id: anchor.id.value.clone(),
                anchor: anchor.clone(),
            });
        Ok(self.query_result(target))
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
    ) -> Result<QueryResult<Option<AnchorReference>>, WorkspaceQueryError> {
        Ok(self.query_result(self.anchor_reference_at_value(path.as_ref(), offset)?))
    }

    fn anchor_reference_at_value(
        &self,
        path: &Path,
        offset: usize,
    ) -> Result<Option<AnchorReference>, WorkspaceQueryError> {
        let path = normalize(path);
        let Some(output) = self.current_output(&path) else {
            return Ok(None);
        };
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
        Ok(None)
    }

    pub fn resolve_task_reference_at(
        &self,
        path: impl AsRef<Path>,
        offset: usize,
    ) -> Result<QueryResult<Option<ResolvedTarget>>, WorkspaceQueryError> {
        let path = normalize(path.as_ref());
        let Some(output) = self.current_output(&path) else {
            return Ok(self.query_result(None));
        };
        for task in &output.tasks.tasks {
            if let Some(prev) = &task.prev {
                if contains_inclusive(&prev.range, offset) {
                    return Ok(self.query_result(Some(self.resolve_task_reference_target(
                        &path,
                        &parse_task_reference_target(&prev.value),
                    )?)));
                }
            }
            if let Some(dependency) = task
                .depends
                .iter()
                .find(|dependency| contains_inclusive(&dependency.range, offset))
            {
                return Ok(self.query_result(Some(
                    self.resolve_task_reference_target(&path, &dependency.target)?,
                )));
            }
        }
        for event in &output.events.events {
            if let Some(reference) = event
                .tasks
                .iter()
                .find(|reference| contains_inclusive(&reference.range, offset))
            {
                return Ok(self.query_result(Some(
                    self.resolve_task_reference_target(&path, &reference.target)?,
                )));
            }
        }
        Ok(self.query_result(None))
    }

    pub fn references_to(
        &self,
        target_path: impl AsRef<Path>,
        target_id: &str,
    ) -> Result<QueryResult<Vec<(PathBuf, AnchorReference)>>, WorkspaceQueryError> {
        let target_path = normalize(target_path.as_ref());
        let mut references = Vec::new();
        if let Some(store) = &self.disk_store {
            let anchors = self.anchors_named(&target_path, target_id)?;
            if anchors.len() == 1 {
                let stored =
                    store.references_to(&target_path, Some(target_id), &self.open_paths())?;
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
        for entry in self.documents.values() {
            let Some(current) = &entry.current else {
                continue;
            };
            for link in &current.output.links {
                if let Some(reference) = self.link_anchor_reference(&entry.path, link)? {
                    if reference.target_path == target_path && reference.target_id == target_id {
                        references.push((entry.path.clone(), reference));
                    }
                }
            }
            for task in &current.output.tasks.tasks {
                for (source, range, target) in task_reference_fields(task) {
                    if let Some(reference) =
                        self.task_anchor_reference(&entry.path, source, range, &target)?
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
                    &self.event_task_references_in_output(&entry.path, &current.output, event)?
                {
                    if let Some(reference) = self.task_anchor_reference(
                        &entry.path,
                        &reference.source,
                        &reference.range,
                        &reference.target,
                    )? {
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
        Ok(self.query_result(references))
    }

    pub fn references_to_document(
        &self,
        target_path: impl AsRef<Path>,
    ) -> Result<QueryResult<Vec<(PathBuf, DocumentReference)>>, WorkspaceQueryError> {
        let target_path = normalize(target_path.as_ref());
        let mut references = Vec::new();
        if let Some(store) = &self.disk_store {
            let open = self.open_paths();
            let stored = store.references_to(&target_path, None, &open)?;
            references.extend(stored.into_iter().map(|reference| {
                (
                    reference.source_path,
                    DocumentReference {
                        source_range: reference.source_range,
                        target_path: target_path.clone(),
                    },
                )
            }));
            {
                let anchors = store.anchors(&[])?;
                let mut ids = anchors
                    .into_iter()
                    .filter(|anchor| anchor.path == target_path)
                    .map(|anchor| anchor.record.id.value)
                    .collect::<Vec<_>>();
                ids.sort();
                ids.dedup();
                for id in ids {
                    if self.anchors_named(&target_path, &id)?.len() != 1 {
                        continue;
                    }
                    let stored = store.references_to(&target_path, Some(&id), &open)?;
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
        for entry in self.documents.values() {
            let Some(current) = &entry.current else {
                continue;
            };
            for link in &current.output.links {
                if resolved_document_path(self.resolve_link_value(&entry.path, link)?).as_ref()
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
                        self.resolve_task_reference_target(&entry.path, &target)?,
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
                    &self.event_task_references_in_output(&entry.path, &current.output, event)?
                {
                    if resolved_document_path(
                        self.resolve_task_reference_target(&entry.path, &reference.target)?,
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
        Ok(self.query_result(references))
    }

    pub fn reverse_references_for_document(
        &self,
        target_path: impl AsRef<Path>,
        target_ids: &HashSet<String>,
    ) -> Result<QueryResult<DocumentReverseReferences>, WorkspaceQueryError> {
        let target_path = normalize(target_path.as_ref());
        let mut references = DocumentReverseReferences::default();
        if let Some(store) = &self.disk_store {
            let open = self.open_paths();
            let document_references = store.references_to(&target_path, None, &open)?;
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
            for target_id in target_ids {
                if self.anchors_named(&target_path, target_id)?.len() != 1 {
                    continue;
                }
                let anchor_references =
                    store.references_to(&target_path, Some(target_id), &open)?;
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
                    self.resolve_link_value(&entry.path, link)?,
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
                        self.resolve_task_reference_target(&entry.path, &target)?,
                    );
                }
            }
            for event in &current.output.events.events {
                for reference in
                    &self.event_task_references_in_output(&entry.path, &current.output, event)?
                {
                    collect_reverse_reference(
                        &mut references,
                        &target_path,
                        target_ids,
                        &entry.path,
                        reference.range.clone(),
                        self.resolve_task_reference_target(&entry.path, &reference.target)?,
                    );
                }
            }
        }
        references.document.sort_by(reference_occurrence_order);
        for occurrences in references.anchors.values_mut() {
            occurrences.sort_by(reference_occurrence_order);
        }
        Ok(self.query_result(references))
    }

    pub fn referenced_documents_from(
        &self,
        source_path: impl AsRef<Path>,
    ) -> Result<QueryResult<Vec<PathBuf>>, WorkspaceQueryError> {
        let source_path = normalize(source_path.as_ref());
        let Some(output) = self.current_output(&source_path) else {
            return Ok(self.query_result(Vec::new()));
        };
        let mut targets = HashSet::new();
        for link in &output.links {
            if let Some(path) = resolved_document_path(self.resolve_link_value(&source_path, link)?)
            {
                targets.insert(path);
            }
        }
        for task in &output.tasks.tasks {
            for (_, _, target) in task_reference_fields(task) {
                if let Some(path) = resolved_document_path(
                    self.resolve_task_reference_target(&source_path, &target)?,
                ) {
                    targets.insert(path);
                }
            }
        }
        for event in &output.events.events {
            for reference in &self.event_task_references_in_output(&source_path, output, event)? {
                if let Some(path) = resolved_document_path(
                    self.resolve_task_reference_target(&source_path, &reference.target)?,
                ) {
                    targets.insert(path);
                }
            }
        }
        let mut targets = targets.into_iter().collect::<Vec<_>>();
        targets.sort();
        Ok(self.query_result(targets))
    }

    fn link_anchor_reference(
        &self,
        from: &Path,
        link: &LinkRecord,
    ) -> Result<Option<AnchorReference>, WorkspaceQueryError> {
        let ResolvedTarget::Anchor { path, id, anchor } = self.resolve_link_value(from, link)?
        else {
            return Ok(None);
        };
        Ok(Some(AnchorReference {
            source_range: link.selection_range.clone(),
            path_range: link.path_range.clone(),
            id_range: match &link.fragment_range {
                Some(range) => range.clone(),
                None => return Ok(None),
            },
            target_path: path,
            target_id: id,
            anchor,
        }))
    }

    fn task_anchor_reference(
        &self,
        from: &Path,
        source: &str,
        range: &std::ops::Range<usize>,
        target: &TaskReferenceTarget,
    ) -> Result<Option<AnchorReference>, WorkspaceQueryError> {
        let Some((target_path, target_id, anchor)) = self.resolve_task_anchor(from, target)? else {
            return Ok(None);
        };
        let Some((path_range, id_range)) = task_reference_ranges(source, range, target_id.as_str())
        else {
            return Ok(None);
        };
        Ok(Some(AnchorReference {
            source_range: range.clone(),
            path_range,
            id_range,
            target_path,
            target_id,
            anchor,
        }))
    }

    fn resolve_task_anchor(
        &self,
        from: &Path,
        target: &TaskReferenceTarget,
    ) -> Result<Option<(PathBuf, String, AnchorRecord)>, WorkspaceQueryError> {
        let ResolvedTarget::Anchor { path, id, anchor } =
            self.resolve_task_reference_target(from, target)?
        else {
            return Ok(None);
        };
        Ok(Some((path, id, anchor)))
    }

    fn resolve_task_reference_target(
        &self,
        from: &Path,
        target: &TaskReferenceTarget,
    ) -> Result<ResolvedTarget, WorkspaceQueryError> {
        let (path, id) = match target {
            TaskReferenceTarget::Internal { id } => (normalize(from), id.clone()),
            TaskReferenceTarget::External { path, id } => {
                (resolve_relative(from, path), id.clone())
            }
            TaskReferenceTarget::Invalid => return Ok(ResolvedTarget::Other),
        };
        if !self.contains_path(&path)? {
            return Ok(ResolvedTarget::UnresolvedPath { path });
        }
        let anchors = self.anchors_named(&path, &id)?;
        let Some(anchor) = anchors.first().cloned() else {
            return Ok(ResolvedTarget::UnresolvedAnchor { path, id });
        };
        if anchors.len() > 1 {
            return Ok(ResolvedTarget::AmbiguousAnchor { path, id });
        }
        Ok(ResolvedTarget::Anchor { path, id, anchor })
    }

    fn document_component_target(
        &self,
        target: ResolvedTarget,
    ) -> Result<ResolvedTarget, WorkspaceQueryError> {
        let path = match target {
            ResolvedTarget::Anchor { path, .. }
            | ResolvedTarget::Document { path }
            | ResolvedTarget::UnresolvedAnchor { path, .. }
            | ResolvedTarget::AmbiguousAnchor { path, .. }
            | ResolvedTarget::UnresolvedPath { path } => path,
            other => return Ok(other),
        };
        if self.contains_path(&path)? {
            Ok(ResolvedTarget::Document { path })
        } else {
            Ok(ResolvedTarget::UnresolvedPath { path })
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
    ) -> Result<QueryResult<Vec<WorkspaceEvent>>, WorkspaceQueryError> {
        if end <= start {
            return Ok(self.query_result(Vec::new()));
        }
        let has_open_documents = !self.documents.is_empty();
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
            let stored = store.events_overlapping(
                start.timestamp_millis(),
                end.timestamp_millis(),
                &self.open_paths(),
            )?;
            events.extend(stored.into_iter().map(|stored| WorkspaceEvent {
                path: stored.path,
                revision: stored.revision,
                event: stored.record,
            }));
        }
        if !has_open_documents && self.disk_store.is_some() {
            return Ok(self.query_result(events));
        }
        events.sort_by(|left, right| {
            left.event
                .sort_datetime()
                .cmp(&right.event.sort_datetime())
                .then(left.path.cmp(&right.path))
                .then(left.event.range.start.cmp(&right.event.range.start))
        });
        Ok(self.query_result(events))
    }

    pub fn events_page_after(
        &self,
        cursor: Option<&WorkspaceEventCursor>,
        limit: usize,
    ) -> Result<QueryResult<Vec<WorkspaceEvent>>, WorkspaceQueryError> {
        let mut events = self
            .open_events_for_page()
            .into_iter()
            .filter(|event| cursor.is_none_or(|cursor| event_after_cursor(event, cursor)))
            .collect::<Vec<_>>();
        let Some(store) = &self.disk_store else {
            events.sort_by(compare_workspace_events);
            events.truncate(limit);
            return Ok(self.query_result(events));
        };
        let cursor = cursor.map(|cursor| store::StoredEventKey {
            sort_millis: cursor.sort_millis,
            path: cursor.path.clone(),
            start: cursor.start,
        });
        let stored_limit = limit.saturating_add(events.len());
        events.extend(
            store
                .event_page_after(cursor.as_ref(), stored_limit, &self.open_paths())?
                .into_iter()
                .map(|stored| WorkspaceEvent {
                    path: stored.path,
                    revision: stored.revision,
                    event: stored.record,
                }),
        );
        events.sort_by(compare_workspace_events);
        events.truncate(limit);
        Ok(self.query_result(events))
    }

    pub fn events_page_before(
        &self,
        cursor: &WorkspaceEventCursor,
        limit: usize,
    ) -> Result<QueryResult<Vec<WorkspaceEvent>>, WorkspaceQueryError> {
        let mut events = self
            .open_events_for_page()
            .into_iter()
            .filter(|event| event_before_cursor(event, cursor))
            .collect::<Vec<_>>();
        let Some(store) = &self.disk_store else {
            events.sort_by(compare_workspace_events);
            if events.len() > limit {
                events.drain(..events.len() - limit);
            }
            return Ok(self.query_result(events));
        };
        let cursor = store::StoredEventKey {
            sort_millis: cursor.sort_millis,
            path: cursor.path.clone(),
            start: cursor.start,
        };
        let stored_limit = limit.saturating_add(events.len());
        events.extend(
            store
                .event_page_before(&cursor, stored_limit, &self.open_paths())?
                .into_iter()
                .map(|stored| WorkspaceEvent {
                    path: stored.path,
                    revision: stored.revision,
                    event: stored.record,
                }),
        );
        events.sort_by(compare_workspace_events);
        if events.len() > limit {
            events.drain(..events.len() - limit);
        }
        Ok(self.query_result(events))
    }

    fn open_events_for_page(&self) -> Vec<WorkspaceEvent> {
        self.documents
            .values()
            .filter_map(|entry| entry.current.as_ref().map(|current| (entry, current)))
            .flat_map(|(entry, current)| {
                current
                    .output
                    .events
                    .events
                    .iter()
                    .cloned()
                    .map(|event| WorkspaceEvent {
                        path: entry.path.clone(),
                        revision: current.revision,
                        event,
                    })
            })
            .collect()
    }

    pub fn events_for_task(
        &self,
        target: &TaskRef,
    ) -> Result<QueryResult<Vec<WorkspaceEvent>>, WorkspaceQueryError> {
        if !self.tasks_for_path(&target.path)?.iter().any(|task| {
            task.id
                .as_ref()
                .is_some_and(|field| field.value == target.id)
        }) {
            return Ok(self.query_result(Vec::new()));
        }
        let mut events = Vec::new();
        for entry in self.documents.values() {
            let Some(current) = &entry.current else {
                continue;
            };
            for event in &current.output.events.events {
                if self
                    .event_task_references_in_output(&entry.path, &current.output, event)?
                    .iter()
                    .map(|reference| {
                        Ok(matches!(
                            self.resolve_task_target(&entry.path, &reference.target)?,
                            TaskTargetResolution::Task { target: ref resolved, .. } if resolved == target
                        ))
                    })
                    .collect::<Result<Vec<_>, WorkspaceQueryError>>()?
                    .into_iter()
                    .any(|matches| matches)
                {
                    events.push(WorkspaceEvent {
                        path: entry.path.clone(),
                        revision: current.revision,
                        event: event.clone(),
                    });
                }
            }
        }
        if let Some(store) = &self.disk_store {
            events.extend(
                store
                    .events_for_task(&target.path, &target.id, &self.open_paths())?
                    .into_iter()
                    .map(|stored| WorkspaceEvent {
                        path: stored.path,
                        revision: stored.revision,
                        event: stored.record,
                    }),
            );
        }
        events.sort_by(|left, right| {
            left.event
                .sort_datetime()
                .cmp(&right.event.sort_datetime())
                .then(left.path.cmp(&right.path))
                .then(left.event.range.start.cmp(&right.event.range.start))
        });
        Ok(self.query_result(events))
    }

    pub fn event_task_references(
        &self,
        path: impl AsRef<Path>,
        event: &EventRecord,
    ) -> Result<QueryResult<Vec<TaskDependency>>, WorkspaceQueryError> {
        if event.tasks_override {
            return Ok(self.query_result(event.tasks.clone()));
        }
        let path = normalize(path.as_ref());
        if let Some(current) = self.current_output(&path) {
            return Ok(
                self.query_result(self.event_task_references_in_output(&path, current, event)?)
            );
        }
        let Some(store) = &self.disk_store else {
            return Ok(self.query_result(Vec::new()));
        };
        let mut references = Vec::new();
        for association in store.event_task_associations_for_event(&path, event.range.start)? {
            let target = parse_task_reference_target(&association.source);
            if matches!(
                self.resolve_task_target(&path, &target)?,
                TaskTargetResolution::Task { .. }
            ) {
                references.push(TaskDependency {
                    source: association.source,
                    range: association.source_range,
                    target,
                });
            }
        }
        Ok(self.query_result(references))
    }

    fn event_task_references_in_output(
        &self,
        path: &Path,
        current: &DocumentOutput,
        event: &EventRecord,
    ) -> Result<Vec<TaskDependency>, WorkspaceQueryError> {
        if event.tasks_override {
            return Ok(event.tasks.clone());
        }
        let mut references = Vec::new();
        for link in current
            .links_contained_by_event(event.range.start)
            .unwrap_or_default()
        {
            let LinkTarget::Anchor {
                path: target_path,
                fragment,
            } = &link.target_kind
            else {
                continue;
            };
            let resolved = self.resolve_link_value(path, link)?;
            let ResolvedTarget::Anchor {
                path: resolved_path,
                id,
                ..
            } = resolved
            else {
                continue;
            };
            let Some(target_output) = self.current_output(&resolved_path) else {
                continue;
            };
            let is_task = target_output
                .tasks
                .tasks
                .iter()
                .any(|task| task.id.as_ref().is_some_and(|field| field.value == id));
            if !is_task {
                continue;
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
            references.push(TaskDependency {
                source: link.target.value.clone(),
                range: link.target.range.clone(),
                target,
            });
        }
        Ok(references)
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

    pub fn diagnostics(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<QueryResult<Vec<Diagnostic>>, WorkspaceQueryError> {
        let context = self.diagnostic_context()?;
        self.diagnostics_with_context(path, &context)
    }

    pub fn diagnostic_context(&self) -> Result<WorkspaceDiagnosticContext, WorkspaceQueryError> {
        Ok(WorkspaceDiagnosticContext {
            task_dependency_graph: self.task_dependency_graph()?,
        })
    }

    pub fn diagnostics_with_context(
        &self,
        path: impl AsRef<Path>,
        context: &WorkspaceDiagnosticContext,
    ) -> Result<QueryResult<Vec<Diagnostic>>, WorkspaceQueryError> {
        let path = normalize(path.as_ref());
        let Some(entry) = self.documents.get(&path) else {
            return Ok(self.query_result(Vec::new()));
        };
        let mut diagnostics = entry.parsed.diagnostics.clone();
        let Some(current) = &entry.current else {
            return Ok(self.query_result(diagnostics));
        };
        diagnostics.extend(current.output.headings.diagnostics.clone());
        diagnostics.extend(current.output.metadata.diagnostics.clone());
        diagnostics.extend(current.output.citations.diagnostics.clone());
        diagnostics.extend(current.output.math.diagnostics.clone());
        diagnostics.extend(current.output.tasks.diagnostics.clone());
        diagnostics.extend(current.output.events.diagnostics.clone());
        diagnostics.extend(current.output.diagnostics.clone());
        for link in &current.output.links {
            let (code, message) = match self.resolve_link_value(&path, link)? {
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
        diagnostics.extend(self.task_workspace_diagnostics(
            &path,
            current,
            &context.task_dependency_graph,
        )?);
        diagnostics.extend(self.event_workspace_diagnostics(&path, current)?);
        Ok(self.query_result(diagnostics))
    }

    fn event_workspace_diagnostics(
        &self,
        path: &Path,
        current: &VersionedDocumentOutput,
    ) -> Result<Vec<Diagnostic>, WorkspaceQueryError> {
        let mut diagnostics = Vec::new();
        for event in &current.output.events.events {
            for reference in &self.event_task_references_in_output(path, &current.output, event)? {
                if let Some(mut diagnostic) = self.task_target_diagnostic(
                    path,
                    &reference.source,
                    &reference.range,
                    &reference.target,
                    "association",
                )? {
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
        Ok(diagnostics)
    }

    fn task_workspace_diagnostics(
        &self,
        path: &Path,
        current: &VersionedDocumentOutput,
        graph: &HashMap<TaskRef, Vec<TaskRef>>,
    ) -> Result<Vec<Diagnostic>, WorkspaceQueryError> {
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
                    self.task_target_diagnostic(path, &prev.value, &prev.range, &target, "prev")?
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
                )? {
                    diagnostics.push(diagnostic);
                    continue;
                }
                if let TaskTargetResolution::Task { target, .. } =
                    self.resolve_task_target(path, &dependency.target)?
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
                if dependency_cycle_contains(graph, task_ref) {
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
                let blockers = self
                    .task_dependencies_value(path, task)?
                    .into_iter()
                    .filter(|dependency| dependency.task.state() == TaskState::Open)
                    .collect::<Vec<_>>();
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
                let blockers = self
                    .task_dependencies_value(path, task)?
                    .into_iter()
                    .filter(|dependency| dependency.task.state() == TaskState::Open)
                    .collect::<Vec<_>>();
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
        Ok(diagnostics)
    }

    fn task_target_diagnostic(
        &self,
        from: &Path,
        source: &str,
        range: &std::ops::Range<usize>,
        target: &TaskReferenceTarget,
        role: &str,
    ) -> Result<Option<Diagnostic>, WorkspaceQueryError> {
        let (code, message) = match self.resolve_task_target(from, target)? {
            TaskTargetResolution::Task { .. } => return Ok(None),
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
        Ok(Some(Diagnostic {
            code,
            severity: DiagnosticSeverity::Warning,
            message,
            range: range.clone(),
            related: Vec::new(),
        }))
    }

    pub fn anchor_rename_target_at(
        &self,
        path: impl AsRef<Path>,
        offset: usize,
    ) -> Result<RenameTarget, WorkspaceOperationError<RenameError>> {
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
            .anchor_reference_at_value(&path, offset)?
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
    ) -> Result<WorkspaceEdit, WorkspaceOperationError<RenameError>> {
        if !valid_anchor_id(replacement) {
            return Err(RenameError::InvalidId.into());
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
        for (path, reference) in self.references_to(&target.path, &target.id)?.value {
            let reference_entry = self
                .entry_for_operation(&path)?
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
                return Err(RenameError::OverlappingEdits.into());
            }
            let expected_revision = self
                .entry_for_operation(&path)?
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
    ) -> Result<PathRenameTarget, WorkspaceOperationError<RenameError>> {
        let path = normalize(path.as_ref());
        if let Some(link) = self.current_output(&path).and_then(|output| {
            output.links.iter().find(|link| {
                link.path_range
                    .as_ref()
                    .is_some_and(|range| contains_inclusive(range, offset))
            })
        }) {
            let old_path = match self.resolve_link_value(&path, link)? {
                ResolvedTarget::Anchor { path, .. } | ResolvedTarget::Document { path } => path,
                _ => return Err(RenameError::NotRenameable.into()),
            };
            return Ok(PathRenameTarget {
                old_path,
                range: link.path_range.clone().ok_or(RenameError::NotRenameable)?,
                input: PathRenameInput::Path,
            });
        }
        let reference = self
            .anchor_reference_at_value(&path, offset)?
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
    ) -> Result<PathRenameTarget, WorkspaceOperationError<RenameError>> {
        let path = normalize(path.as_ref());
        if offset == 0 && self.current_output(&path).is_some() {
            return Ok(PathRenameTarget {
                old_path: path,
                range: 0..0,
                input: PathRenameInput::FileStem,
            });
        }
        self.path_rename_target_at(path, offset)
    }

    pub fn rename_document(
        &self,
        target: &PathRenameTarget,
        new_path: impl AsRef<Path>,
    ) -> Result<WorkspaceEdit, WorkspaceOperationError<RenameError>> {
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
                    return Err(RenameError::InvalidPath.into());
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
            return Err(RenameError::InvalidPath.into());
        }
        if self.contains_path(&new_path)? || new_path.exists() {
            return Err(RenameError::TargetExists.into());
        }
        if !self.contains_path(&old_path)? {
            return Err(RenameError::NotRenameable.into());
        }

        let mut grouped: HashMap<PathBuf, Vec<TextEdit>> = HashMap::new();
        let entries = self.entries_for_operation()?;
        for entry in &entries {
            let Some(current) = &entry.current else {
                continue;
            };
            for link in &current.output.links {
                let Some(path_range) = &link.path_range else {
                    continue;
                };
                let resolved = self.resolve_link_value(&entry.path, link)?;
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
                    return Err(RenameError::InvalidPath.into());
                };
                grouped
                    .entry(entry.path.clone())
                    .or_default()
                    .push(link_path_rename_edit(entry, link, path_range, replacement)?);
            }
            for task in &current.output.tasks.tasks {
                for (source, range, target) in task_reference_fields(task) {
                    let Some(reference) =
                        self.task_anchor_reference(&entry.path, source, range, &target)?
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
                        return Err(RenameError::InvalidPath.into());
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
                return Err(RenameError::OverlappingEdits.into());
            }
            document_changes.push(DocumentEdit {
                expected_revision: self
                    .entry_for_operation(&path)?
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
        if offset != 0 {
            return Err(MetadataInsertError::CursorNotAtDocumentStart);
        }

        let metadata = [
            OwnedBlock::association("title", title),
            OwnedBlock::association("created", created),
        ];
        let affected = 0..if entry.parsed.syntax.blocks.is_empty() {
            entry.parsed.source.len()
        } else {
            0
        };
        let mut edit = EditSession::new(&entry.parsed, affected)
            .map_err(|_| MetadataInsertError::GeneratedInvalid)?;
        edit.insert_blocks(0, &metadata)
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
        if mark.attrs.has_class("event") {
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
        owned.retain_attributes(
            |attribute| !matches!(attribute, OwnedAttribute::Class(value) if value == "event"),
        );
        owned.prepend_attribute(OwnedAttribute::class("event"));
        strip_event_shorthand_prefix(&mut owned, title_start)?;
        let mut attributes = owned.attributes();
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
    ) -> Result<WorkspaceEdit, WorkspaceOperationError<TaskAuthoringError>> {
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
                return Err(TaskAuthoringError::InvalidPlacement.into());
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
                    return Err(TaskAuthoringError::InvalidPlacement.into());
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
    ) -> Result<WorkspaceEdit, WorkspaceOperationError<TaskAuthoringError>> {
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

    pub fn update_and_move_task(
        &self,
        path: impl AsRef<Path>,
        task_range: std::ops::Range<usize>,
        input: &TaskAuthoringInput,
        placement: Option<&TaskPlacement>,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, WorkspaceOperationError<TaskAuthoringError>> {
        let Some(placement) = placement else {
            return self.update_task(path, task_range, input, timestamp);
        };
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
        validate_task_authoring_input(input, timestamp)?;
        self.validate_authored_task_references(
            &path,
            task.id.as_ref().map(|id| id.value.as_str()),
            input,
        )?;
        let block = parsed_block_with_range(&entry.parsed.syntax.blocks, &task.range)
            .ok_or(TaskAuthoringError::TaskNotFound)?;
        let moved = updated_owned_task(&entry.parsed.source, block, task, input, timestamp);
        self.move_task_owned(entry, path, task.range.clone(), placement, moved)
            .map_err(Into::into)
    }

    pub fn update_task_patch(
        &self,
        path: impl AsRef<Path>,
        task_range: std::ops::Range<usize>,
        patch: &TaskAuthoringPatch,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, WorkspaceOperationError<TaskAuthoringError>> {
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
        let owned = updated_owned_task(&entry.parsed.source, block, task, &input, timestamp);
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
        let moved = OwnedBlock::from_parsed(&entry.parsed.source, source);
        self.move_task_owned(entry, path, task.range.clone(), placement, moved)
    }

    fn move_task_owned(
        &self,
        entry: &DocumentEntry,
        path: PathBuf,
        task_range: std::ops::Range<usize>,
        placement: &TaskPlacement,
        moved: OwnedBlock,
    ) -> Result<WorkspaceEdit, TaskAuthoringError> {
        if placement
            .parent
            .as_ref()
            .is_some_and(|parent| task_range.start <= parent.start && parent.end <= task_range.end)
            || placement.after.as_ref() == Some(&task_range)
        {
            return Err(TaskAuthoringError::InvalidPlacement);
        }
        let source_parent = direct_parent_range(&entry.parsed.syntax.blocks, &task_range);
        if let Some(parent_range) = &placement.parent {
            let source_path = block_index_path(&entry.parsed.syntax.blocks, &task_range)
                .ok_or(TaskAuthoringError::InvalidPlacement)?;
            let parent_path = block_index_path(&entry.parsed.syntax.blocks, parent_range)
                .ok_or(TaskAuthoringError::InvalidPlacement)?;
            if source_path.first() == parent_path.first() {
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
                let root_index = source_path[0];
                let root = parsed_block(&entry.parsed.syntax.blocks[root_index])
                    .ok_or(TaskAuthoringError::InvalidPlacement)?;
                let parent = parsed_block_with_range(&entry.parsed.syntax.blocks, parent_range)
                    .ok_or(TaskAuthoringError::InvalidPlacement)?;
                let mut insertion =
                    child_insertion_index(&parent.children, placement.after.as_ref())?;
                let source_relative = &source_path[1..];
                let mut parent_relative = parent_path[1..].to_vec();
                if source_relative.len() == parent_relative.len() + 1
                    && source_relative[..parent_relative.len()] == parent_relative
                    && insertion > source_relative[parent_relative.len()]
                {
                    insertion -= 1;
                }
                adjust_path_after_removal(&mut parent_relative, source_relative);
                let mut owned_root = OwnedBlock::from_parsed(&entry.parsed.source, root);
                remove_owned_at_path(&mut owned_root, source_relative)
                    .ok_or(TaskAuthoringError::InvalidPlacement)?;
                owned_at_path_mut(&mut owned_root, &parent_relative)
                    .and_then(OwnedBlock::children_mut)
                    .ok_or(TaskAuthoringError::InvalidPlacement)?
                    .insert(insertion, moved);
                let edit = replace_owned_block(&entry.parsed, root.range.clone(), &owned_root)
                    .map_err(|_| TaskAuthoringError::GeneratedInvalid)?;
                return Ok(single_document_edit(entry, path, edit));
            }
        }
        let top_level_after = if placement.parent.is_none() {
            placement.after.clone().or_else(|| {
                entry
                    .parsed
                    .syntax
                    .blocks
                    .iter()
                    .rev()
                    .map(Block::range)
                    .find(|range| **range != task_range)
                    .cloned()
            })
        } else {
            None
        };
        if top_level_after
            .as_ref()
            .is_some_and(|after| after.start <= task_range.start && task_range.end <= after.end)
        {
            let after = top_level_after.as_ref().expect("checked after");
            let ancestor = parsed_block_with_range(&entry.parsed.syntax.blocks, after)
                .ok_or(TaskAuthoringError::InvalidPlacement)?;
            let mut owned_ancestor = OwnedBlock::from_parsed(&entry.parsed.source, ancestor);
            if !remove_owned_descendant(ancestor, &mut owned_ancestor, &task_range) {
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
                .position(|child| child.range() == &task_range)
                .ok_or(TaskAuthoringError::InvalidPlacement)?;
            let mut owned_parent = OwnedBlock::from_parsed(&entry.parsed.source, parent);
            owned_parent
                .children_mut()
                .expect("parsed parent")
                .remove(source_index);
            replace_owned_block(&entry.parsed, parent_range.clone(), &owned_parent)
                .map_err(|_| TaskAuthoringError::GeneratedInvalid)?
        } else {
            remove_syntax_block(&entry.parsed, task_range.clone())
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
            let Some(after) = top_level_after.as_ref() else {
                let edit = replace_owned_block(&entry.parsed, task_range, &moved)
                    .map_err(|_| TaskAuthoringError::GeneratedInvalid)?;
                return Ok(single_document_edit(entry, path, edit));
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

    fn validate_authored_task_references(
        &self,
        path: &Path,
        id: Option<&str>,
        input: &TaskAuthoringInput,
    ) -> Result<(), WorkspaceOperationError<TaskAuthoringError>> {
        let mut dependencies = Vec::new();
        for reference in input.prev.iter().chain(&input.depends) {
            let target = parse_task_reference_target(reference);
            let TaskTargetResolution::Task { target, .. } =
                self.resolve_task_target(path, &target)?
            else {
                return Err(TaskAuthoringError::UnresolvedReference.into());
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
            let mut graph = self.task_dependency_graph()?;
            graph.insert(own.clone(), dependencies);
            if dependency_cycle_contains(&graph, &own) {
                return Err(TaskAuthoringError::DependencyCycle.into());
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
        let mut attributes = owned.attributes();
        attributes.retain(|attribute| {
            !matches!(attribute, OwnedAttribute::Pair { key, .. } if matches!(key.as_str(), "date" | "timezone" | "at" | "start" | "end" | "tasks"))
        });
        attributes.extend(event_attributes(input, &current.output.metadata));
        owned = owned.with_attributes(attributes);
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
    ) -> Result<QueryResult<Vec<CompletionCandidate>>, WorkspaceQueryError> {
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
                                "`->[{}|{}]",
                                escape_parsed_text(&title),
                                escape_parsed_text(&relative)
                            ),
                            replace: replace.clone(),
                        }
                    })
                })
                .collect(),
            LinkCompletionContext::Path {
                replace,
                query,
                parsed,
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
                    if !*parsed && !valid_bare_attribute_value(&relative) {
                        return None;
                    }
                    Some(CompletionCandidate {
                        label: relative.clone(),
                        detail: title,
                        new_text: if *parsed {
                            escape_parsed_text(&relative)
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
                    candidates.extend(store.documents()?.into_iter().filter_map(|document| {
                        if open.binary_search(&document.path).is_ok() || document.path == from {
                            return None;
                        }
                        let relative = relative_path(&from, &document.path)?;
                        let title = if document.title.is_empty() {
                            relative.clone()
                        } else {
                            document.title
                        };
                        (fuzzy_match(&relative, query) || fuzzy_match(&title, query)).then(|| {
                            CompletionCandidate {
                                label: title.clone(),
                                detail: relative.clone(),
                                new_text: format!(
                                    "`->[{}|{}]",
                                    escape_parsed_text(&title),
                                    escape_parsed_text(&relative)
                                ),
                                replace: replace.clone(),
                            }
                        })
                    }));
                }
                LinkCompletionContext::Path {
                    replace,
                    query,
                    parsed,
                } => {
                    candidates.extend(store.documents()?.into_iter().filter_map(|document| {
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
                            || (!*parsed && !valid_bare_attribute_value(&relative))
                        {
                            return None;
                        }
                        Some(CompletionCandidate {
                            label: relative.clone(),
                            detail: title,
                            new_text: if *parsed {
                                escape_parsed_text(&relative)
                            } else {
                                relative
                            },
                            replace: replace.clone(),
                        })
                    }));
                }
                LinkCompletionContext::AutolinkPath {
                    replace,
                    envelope,
                    quote_count,
                    suffix,
                    query,
                } => {
                    candidates.extend(store.documents()?.into_iter().filter_map(|document| {
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
                        candidates.extend(
                            store
                                .anchors(&[])?
                                .into_iter()
                                .filter(|stored| {
                                    stored.path == target_path
                                        && fuzzy_match(&stored.record.id.value, query)
                                })
                                .map(|stored| CompletionCandidate {
                                    label: format!("#{}", stored.record.id.value),
                                    detail: format!("explicit anchor in {}", target_path.display()),
                                    new_text: stored.record.id.value,
                                    replace: replace.clone(),
                                }),
                        );
                    }
                }
            }
        }
        candidates.sort_by(|left, right| left.label.cmp(&right.label));
        Ok(self.query_result(candidates))
    }

    pub fn complete_task_dependency(
        &self,
        from: impl AsRef<Path>,
        context: &TaskDependencyCompletionContext,
    ) -> Result<QueryResult<Vec<CompletionCandidate>>, WorkspaceQueryError> {
        let from = normalize(from.as_ref());
        let Some(owner) = self.current_output(&from).and_then(|output| {
            output
                .tasks
                .tasks
                .iter()
                .find(|task| task.range == context.task_range)
        }) else {
            return Ok(self.query_result(Vec::new()));
        };
        let owner_ref = owner.id.as_ref().map(|id| TaskRef {
            path: from.clone(),
            id: id.value.clone(),
        });
        let mut existing = HashSet::new();
        for target in &context.existing {
            if let TaskTargetResolution::Task { target, .. } =
                self.resolve_task_target(&from, target)?
            {
                existing.insert(target);
            }
        }
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
                for document in store.documents()? {
                    if open.binary_search(&document.path).is_ok()
                        || !self
                            .tasks_for_path(&document.path)?
                            .iter()
                            .any(|task| eligible(&document.path, task))
                    {
                        continue;
                    }
                    let Some(relative) = relative_path(&from, &document.path) else {
                        continue;
                    };
                    let reference = if document.path == from {
                        "#".to_string()
                    } else {
                        format!("{relative}#")
                    };
                    if fuzzy_match(reference.trim_end_matches('#'), &context.query) {
                        candidates.push(CompletionCandidate {
                            label: reference.clone(),
                            detail: format!("task document ({relative})"),
                            new_text: reference,
                            replace: context.replace.clone(),
                        });
                    }
                }
            }
            candidates.sort_by(|left, right| left.label.cmp(&right.label));
            return Ok(self.query_result(candidates));
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
        if !self.contains_path(&target_path)? {
            return Ok(self.query_result(Vec::new()));
        }
        let now = chrono::Local::now().fixed_offset();
        let id_replace_start = context.replace.start + path_query.len() + 1;
        let id_replace = id_replace_start..context.replace.end;
        let relative = relative_path(&from, &target_path).unwrap_or_else(|| path_query.to_string());
        let target_tasks = self.tasks_for_path(&target_path)?;
        let mut candidates = Vec::new();
        for task in target_tasks
            .iter()
            .filter(|task| eligible(&target_path, task))
        {
            let Some(id) = task.id.as_ref() else {
                continue;
            };
            if !fuzzy_match(&id.value, id_query) && !fuzzy_match(&task.title, id_query) {
                continue;
            }
            let (state, _) = derive_task_workflow_state(
                task,
                self.is_task_blocked_value(&target_path, task)?,
                now,
            );
            let title = if task.title.is_empty() {
                "Untitled task"
            } else {
                &task.title
            };
            candidates.push(CompletionCandidate {
                label: id.value.clone(),
                detail: format!(
                    "{}  {} ({relative})",
                    state.as_str().to_ascii_uppercase(),
                    title
                ),
                new_text: id.value.clone(),
                replace: id_replace.clone(),
            });
        }
        candidates.sort_by(|left, right| left.label.cmp(&right.label));
        Ok(self.query_result(candidates))
    }

    fn current_output(&self, path: &Path) -> Option<&DocumentOutput> {
        self.documents
            .get(&normalize(path))?
            .current
            .as_ref()
            .map(|versioned| versioned.output.as_ref())
    }

    fn anchors_named(
        &self,
        path: &Path,
        id: &str,
    ) -> Result<Vec<AnchorRecord>, WorkspaceQueryError> {
        let path = normalize(path);
        if let Some(entry) = self.documents.get(&path) {
            return Ok(entry
                .current
                .as_ref()
                .into_iter()
                .flat_map(|current| &current.output.anchors)
                .filter(|anchor| anchor.id.value == id)
                .cloned()
                .collect());
        }
        self.disk_store
            .as_ref()
            .map(|store| store.anchors_named(&path, id))
            .transpose()?
            .map_or_else(|| Ok(Vec::new()), Ok)
    }

    fn tasks_for_path(&self, path: &Path) -> Result<Vec<TaskRecord>, WorkspaceQueryError> {
        let path = normalize(path);
        if let Some(entry) = self.documents.get(&path) {
            return Ok(entry
                .current
                .as_ref()
                .map(|current| current.output.tasks.tasks.clone())
                .unwrap_or_default());
        }
        self.disk_store
            .as_ref()
            .map(|store| store.tasks_for_path(&path))
            .transpose()?
            .map_or_else(|| Ok(Vec::new()), Ok)
    }

    fn all_tasks(&self) -> Result<Vec<(PathBuf, TaskRecord)>, WorkspaceQueryError> {
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
            tasks.extend(
                store
                    .tasks(&self.open_paths())?
                    .into_iter()
                    .map(|stored| (stored.path, stored.record)),
            );
        }
        tasks.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then(left.1.range.start.cmp(&right.1.range.start))
        });
        Ok(tasks)
    }

    fn entry_for_operation(
        &self,
        path: &Path,
    ) -> Result<Option<DocumentEntry>, WorkspaceQueryError> {
        let path = normalize(path);
        if let Some(entry) = self.documents.get(&path) {
            return Ok(Some(entry.clone()));
        }
        let Some(store) = &self.disk_store else {
            return Ok(None);
        };
        let Some(document) = store.document(&path)? else {
            return Ok(None);
        };
        let Ok(source) = std::fs::read_to_string(&path) else {
            return Ok(None);
        };
        if document.content_hash != SqliteSemanticStore::content_hash(&source) {
            return Ok(None);
        }
        Ok(Some(Self::materialize_document(
            path,
            document.revision,
            &source,
        )))
    }

    fn entries_for_operation(&self) -> Result<Vec<DocumentEntry>, WorkspaceQueryError> {
        let open = self.open_paths();
        let mut entries = self.documents.values().cloned().collect::<Vec<_>>();
        if let Some(store) = &self.disk_store {
            for document in store.documents()? {
                if open.binary_search(&document.path).is_ok() {
                    continue;
                }
                if let Some(entry) = self.entry_for_operation(&document.path)? {
                    entries.push(entry);
                }
            }
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(entries)
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
        if !contains_component(block.range(), offset) {
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
            Block::Verbatim(_) => {}
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
    let argument = syntax
        .head
        .argument(0)
        .ok_or(EventShorthandError::InvalidShorthand)?;
    let shorthand = &source[argument.range.clone()];
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
    if !matches!(mark.marker.as_str(), "-" | ".") || mark.attrs.has_class("event") {
        return None;
    }
    let argument = sibling.head.argument(0)?;
    let shorthand = &source[argument.range.clone()];
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
    owned.set_head_text(&input.title);
    let OwnedBlock::Parsed { head, .. } = owned else {
        return;
    };
    head.splice(
        0..0,
        [OwnedInline::Text(schedule), OwnedInline::ArgumentSeparator],
    );
}

fn prepend_event_schedule(owned: &mut OwnedBlock, input: &EventInput) {
    let (_, _, schedule) =
        compact_event_schedule(input).expect("event input is validated before authoring");
    let OwnedBlock::Parsed { head, .. } = owned else {
        return;
    };
    head.splice(
        0..0,
        [OwnedInline::Text(schedule), OwnedInline::ArgumentSeparator],
    );
}

fn owned_event(input: &EventInput, metadata: &MetadataOutput) -> OwnedBlock {
    let mut attributes = vec![OwnedAttribute::class("event")];
    attributes.extend(event_attributes(input, metadata));
    let mut event = OwnedBlock::marked("-", "").with_attributes(attributes);
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
    if let OwnedBlock::Parsed { children, .. } = owned {
        for (index, (syntax_child, owned_child)) in syntax.children.iter().zip(children).enumerate()
        {
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
    }
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
            owned.prepend_attribute(OwnedAttribute::class("event"));
            owned.extend_attributes(attributes);
            prepend_event_schedule(owned, &input);
            converted += 1;
        }
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
    if let OwnedBlock::Parsed { children, .. } = owned {
        for (owned_child, syntax_child) in children.iter_mut().zip(&block.children) {
            let Block::Parsed(syntax_child) = syntax_child else {
                continue;
            };
            prepare_recurring_task_clone(owned_child, syntax_child, context);
        }
    }

    if let Some(task) = context.tasks.iter().find(|task| task.range == block.range) {
        owned.retain_attributes(persistent_task_attribute);
        if task.range == context.root.range {
            owned.push_attribute(OwnedAttribute::id(context.next_id));
            owned.push_attribute(OwnedAttribute::quoted("created", context.timestamp));
            owned.push_attribute(OwnedAttribute::quoted("due", context.next_due));
            if let Some(wait) = context.next_wait {
                owned.push_attribute(OwnedAttribute::quoted("wait", wait));
            }
            owned.push_attribute(OwnedAttribute::quoted("recur", context.recur));
            owned.push_attribute(OwnedAttribute::quoted(
                "prev",
                format!("#{}", context.current_id),
            ));
        }
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
    value
        .replace('`', "``")
        .replace('[', "`[")
        .replace(']', "`]")
        .replace('|', "`|")
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
mod tests;
