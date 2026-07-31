use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, FixedOffset};
use plumb_core::{
    parse, Attributes, Block, Diagnostic, DiagnosticSeverity, ParsedBlock, ParsedDocument,
};
pub use plumb_edit::{apply_text_edits, TextEdit};
use plumb_edit::{AttributePosition, EditSession, OwnedAttribute, OwnedBlock};
#[cfg(test)]
use plumb_extensions::TaskStatus;
use plumb_extensions::{
    analyze_document, parse_task_reference_target, AnchorRecord, DocumentOutput, EventRecord,
    FileCompletionContext, FileRecord, FileTarget, ImageCompletionContext, ImageRecord,
    ImageTarget, LinkCompletionContext, LinkRecord, LinkSpelling, LinkTarget, TaskRecord,
    TaskReferenceTarget, TaskState,
};

mod scan;
mod search;
mod task_sort;
mod tasks;

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
pub struct WorkspaceEvent {
    pub path: PathBuf,
    pub revision: i64,
    pub event: EventRecord,
}

#[derive(Debug, Default, Clone)]
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
            for reference in &event.tasks {
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
            for event in &current.output.events.events {
                for reference in &event.tasks {
                    if let Some(reference) = self.task_anchor_reference(
                        &entry.path,
                        &reference.source,
                        &reference.range,
                        &reference.target,
                    ) {
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

    pub fn references_to_document(
        &self,
        target_path: impl AsRef<Path>,
    ) -> Vec<(&Path, DocumentReference)> {
        let target_path = normalize(target_path.as_ref());
        let mut references = Vec::new();
        for entry in self.documents.values() {
            let Some(current) = &entry.current else {
                continue;
            };
            for link in &current.output.links {
                if resolved_document_path(self.resolve_link(&entry.path, link)).as_ref()
                    == Some(&target_path)
                {
                    references.push((
                        entry.path.as_path(),
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
                            entry.path.as_path(),
                            DocumentReference {
                                source_range: range.clone(),
                                target_path: target_path.clone(),
                            },
                        ));
                    }
                }
            }
            for event in &current.output.events.events {
                for reference in &event.tasks {
                    if resolved_document_path(
                        self.resolve_task_reference_target(&entry.path, &reference.target),
                    )
                    .as_ref()
                        == Some(&target_path)
                    {
                        references.push((
                            entry.path.as_path(),
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
        for event in &output.events.events {
            for reference in &event.tasks {
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

    fn document_component_target(&self, target: ResolvedTarget) -> ResolvedTarget {
        let path = match target {
            ResolvedTarget::Anchor { path, .. }
            | ResolvedTarget::Document { path }
            | ResolvedTarget::UnresolvedAnchor { path, .. }
            | ResolvedTarget::AmbiguousAnchor { path, .. }
            | ResolvedTarget::UnresolvedPath { path } => path,
            other => return other,
        };
        if self.current_output(&path).is_some() {
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
                if event.tasks.iter().any(|reference| {
                    matches!(
                        self.resolve_task_target(&entry.path, &reference.target),
                        TaskTargetResolution::Task { target: ref resolved, .. } if resolved == target
                    )
                }) {
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
            for reference in &event.tasks {
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
            let Some(uid) = &event.uid else {
                continue;
            };
            let occurrences = self
                .documents
                .values()
                .filter_map(|entry| entry.current.as_ref())
                .flat_map(|current| current.output.events.events.iter())
                .filter(|candidate| {
                    candidate
                        .uid
                        .as_ref()
                        .is_some_and(|candidate_uid| candidate_uid.value == uid.value)
                })
                .count();
            if occurrences > 1 {
                diagnostics.push(Diagnostic {
                    code: "event.duplicate-uid",
                    severity: DiagnosticSeverity::Warning,
                    message: format!("event UID '{}' is not unique in the workspace", uid.value),
                    range: uid.range.clone(),
                    related: Vec::new(),
                });
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
                .documents
                .get(path)
                .ok_or(RenameError::StaleOrInvalidDocument)?;
            grouped
                .entry(path.to_path_buf())
                .or_default()
                .push(validated_token_edit(
                    reference_entry,
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

        let metadata = OwnedBlock::marked("meta", "").with_children(vec![
            OwnedBlock::marked(":", "title").with_children(vec![OwnedBlock::paragraph(title)]),
            OwnedBlock::marked(":", "created").with_children(vec![OwnedBlock::paragraph(created)]),
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
        let uid = format!("{}@plumb.local", uuid::Uuid::new_v4());
        let event = owned_event(input, &uid);
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
            let affected = after
                .cloned()
                .unwrap_or(0..entry.parsed.source.len());
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
        let edit = exact_owned_block_edit(&entry.parsed, task.range.clone(), &owned)?;
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
            let edit =
                exact_owned_blocks_edit(&entry.parsed, after.clone(), &[owned_ancestor, moved])?;
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
            exact_owned_block_edit(&entry.parsed, parent_range.clone(), &owned_parent)?
        } else {
            let removal_start = entry.parsed.source[..task.range.start]
                .rfind('\n')
                .map_or(0, |index| index + 1);
            TextEdit::replace(&entry.parsed, removal_start..task.range.end, "")
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
            exact_owned_block_edit(&entry.parsed, parent_range.clone(), &owned)?
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
        let edit = exact_owned_block_edit(&entry.parsed, parent_range, &owned)?;
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
        owned.set_head_text(&input.title);
        let attributes = owned.attributes_mut();
        attributes.retain(|attribute| {
            !matches!(attribute, OwnedAttribute::Pair { key, .. } if matches!(key.as_str(), "uid" | "at" | "start" | "end" | "tasks"))
        });
        let uid = event
            .uid
            .as_ref()
            .map(|field| field.value.clone())
            .unwrap_or_else(|| format!("{}@plumb.local", uuid::Uuid::new_v4()));
        attributes.push(OwnedAttribute::quoted("uid", uid));
        if let Some(at) = &input.at {
            attributes.push(OwnedAttribute::quoted("at", at));
        }
        if let Some(start) = &input.start {
            attributes.push(OwnedAttribute::quoted("start", start));
        }
        if let Some(end) = &input.end {
            attributes.push(OwnedAttribute::quoted("end", end));
        }
        if !input.tasks.is_empty() {
            attributes.push(OwnedAttribute::quoted("tasks", input.tasks.join(" ")));
        }
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
                                "`->[{}]{{to=\"{}\"}}",
                                escape_inline_text(&title),
                                escape_quoted_value(&relative)
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
    WorkspaceEdit {
        document_changes: vec![DocumentEdit {
            path,
            expected_revision: entry.revision,
            edits: vec![edit],
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
    }
    Ok(())
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
    let mut attributes = vec![OwnedAttribute::class("task"), OwnedAttribute::id(id)];
    append_authored_task_fields(&mut attributes, input, timestamp);
    OwnedBlock::marked("-", &input.title).with_attributes(attributes)
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

fn exact_owned_block_edit(
    parsed: &ParsedDocument,
    range: std::ops::Range<usize>,
    block: &OwnedBlock,
) -> Result<TextEdit, TaskAuthoringError> {
    exact_owned_blocks_edit(parsed, range, std::slice::from_ref(block))
}

fn exact_owned_blocks_edit(
    parsed: &ParsedDocument,
    range: std::ops::Range<usize>,
    blocks: &[OwnedBlock],
) -> Result<TextEdit, TaskAuthoringError> {
    let line_start = parsed.source[..range.start]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let indent = &parsed.source[line_start..range.start];
    if !indent.chars().all(|character| character == ' ') {
        return Err(TaskAuthoringError::GeneratedInvalid);
    }
    let mut formatted = String::new();
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 && !formatted.ends_with("\n\n") {
            if !formatted.ends_with('\n') {
                formatted.push('\n');
            }
            formatted.push('\n');
        }
        formatted.push_str(
            &block
                .format()
                .map_err(|_| TaskAuthoringError::GeneratedInvalid)?,
        );
    }
    let newline = if parsed.source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let formatted = if newline == "\r\n" {
        formatted.replace('\n', newline)
    } else {
        formatted
    };
    let mut replacement = String::new();
    for (index, line) in formatted.split_inclusive(newline).enumerate() {
        let content = line.strip_suffix(newline).unwrap_or(line);
        if !content.is_empty() {
            // The syntax range starts after the first line's structural indent,
            // which remains outside the edit. Continuation lines need it added.
            if index > 0 {
                replacement.push_str(indent);
            }
            replacement.push_str(content);
        }
        if line.ends_with(newline) {
            replacement.push_str(newline);
        }
    }
    let original = &parsed.source[range.clone()];
    let original_breaks = original
        .as_bytes()
        .iter()
        .rev()
        .take_while(|byte| **byte == b'\n' || **byte == b'\r')
        .filter(|byte| **byte == b'\n')
        .count();
    let replacement_breaks = replacement
        .as_bytes()
        .iter()
        .rev()
        .take_while(|byte| **byte == b'\n' || **byte == b'\r')
        .filter(|byte| **byte == b'\n')
        .count();
    for _ in replacement_breaks..original_breaks {
        replacement.push_str(newline);
    }
    TextEdit::replace(parsed, range, replacement).map_err(|_| TaskAuthoringError::GeneratedInvalid)
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

fn owned_event(input: &EventInput, uid: &str) -> OwnedBlock {
    let mut attributes = vec![
        OwnedAttribute::class("event"),
        OwnedAttribute::quoted("uid", uid),
    ];
    if let Some(at) = &input.at {
        attributes.push(OwnedAttribute::quoted("at", at));
    }
    if let Some(start) = &input.start {
        attributes.push(OwnedAttribute::quoted("start", start));
    }
    if let Some(end) = &input.end {
        attributes.push(OwnedAttribute::quoted("end", end));
    }
    if !input.tasks.is_empty() {
        attributes.push(OwnedAttribute::quoted("tasks", input.tasks.join(" ")));
    }
    OwnedBlock::marked("-", &input.title).with_attributes(attributes)
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
        workspace.insert("notes/a note.plumb", 1, "`#{#local} Local\n");
        workspace.insert("notes/a%20note.plumb", 1, "`#{#literal} Literal\n");
        workspace.insert(
            "notes/b.plumb",
            1,
            "See `->[local]{to=\"a note.plumb#local\"}.\nSee `->[literal]{to=\"a%20note.plumb#literal\"}.\n",
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
        workspace.insert("document.plumb", 1, "`->[a]{to=\"a.plumb\"}\n");
        workspace.insert(
            "a-local.plumb",
            1,
            "`#{#local} Local\n`->[x]{to=\"#local\"}\n",
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
    fn resolves_document_and_anchor_targets_from_declarations_and_reference_components() {
        let target_source = "`meta\n `: title\n\n    Target\n\n`#{#section} Section\n";
        let reference_source = "See `->[named]{to=\"target.plumb#section\"} and `[target.plumb#section]{.->}.\n`-{.task prev=\"target.plumb#section\" depends=\"target.plumb#section\"} Review\n";
        let mut workspace = Workspace::new();
        workspace.insert("target.plumb", 1, target_source);
        workspace.insert("reference.plumb", 1, reference_source);

        assert!(matches!(
            workspace.target_at("target.plumb", target_source.find("meta").unwrap()),
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

        let lonely_source = "`meta\n `: title\n\n    Lonely\n";
        workspace.insert("lonely.plumb", 1, lonely_source);
        assert!(matches!(
            workspace.target_at("lonely.plumb", lonely_source.find("meta").unwrap()),
            Some(ResolvedTarget::Document { path }) if path == Path::new("lonely.plumb")
        ));
        assert!(workspace.references_to_document("lonely.plumb").is_empty());

        workspace.insert("target.plumb", 2, "`node{key=a key=b} Invalid\n");
        assert!(workspace.target_at("target.plumb", 1).is_none());
    }

    #[test]
    fn task_fields_participate_in_navigation_references_and_anchor_rename() {
        let target_source = "`-{.task #draft} Draft\n`node{#note} Note\n";
        let reference_source = "`-{.task #review prev=\"Project Plan.plumb#draft\" depends=\"Project Plan.plumb#draft Project Plan.plumb#note Project%20Plan.plumb#literal\"} Review\nSee `->[draft]{to=\"Project Plan.plumb#draft\"}.\n";
        let mut workspace = Workspace::new();
        workspace.insert("Project Plan.plumb", 4, target_source);
        workspace.insert("Project%20Plan.plumb", 4, "`node{#literal} Literal\n");
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
        let target_source = "`-{.task #draft} Draft\n";
        let reference_source = "`-{.task prev=\"Project Plan.plumb#draft\" depends=\"Project Plan.plumb#draft\"} Review\nSee `->[draft]{to=\"Project Plan.plumb#draft\"}.\n";
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
        let source = "`meta\n `: title\n\n    Stable title\n";
        let mut workspace = Workspace::new();
        workspace.insert("current.plumb", 4, source);
        workspace.insert("incoming.plumb", 7, "`->[current]{to=\"current.plumb\"}\n");

        let target = workspace
            .document_rename_target_at("current.plumb", source.find("meta").unwrap())
            .unwrap();
        assert_eq!(target.old_path, Path::new("current.plumb"));
        assert_eq!(&source[target.range.clone()], "meta");

        let edit = workspace.rename_document(&target, "renamed.plumb").unwrap();
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
            "`meta\n  `: title\n\n    Design Guide\n\n`# No id\n`##{#api} API\n",
        );
        workspace.insert(
            "notes/Project Plan.plumb",
            1,
            "`meta\n `: title\n\n    Project Plan\n\n`#{#roadmap} Roadmap\n",
        );
        workspace.insert("notes/中文笔记.plumb", 1, "`#{#内容} 中文内容\n");
        workspace.insert("notes/方案 (草稿).plumb", 1, "`# 草稿\n");
        workspace.insert("notes/方案]终稿.plumb", 1, "`# 终稿\n");
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
        assert_eq!(labels[0].new_text, "`->[Design Guide]{to=\"design.plumb\"}");
        let spaced_label = workspace.complete_link(
            "notes/current.plumb",
            &LinkCompletionContext::Label {
                replace: 0..0,
                query: "project".to_string(),
            },
        );
        assert_eq!(
            spaced_label[0].new_text,
            "`->[Project Plan]{to=\"Project Plan.plumb\"}"
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
        let source = "`[static/image one.PNG]{.->}\n`img[Result]{src=\"static/image one.PNG\"}\n`img[Literal percent]{src=\"static/literal%20name.PNG\"}\n`[static/literal%20name.txt]{.->}\n";
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
            "`file[Demo]{src=\"static/demo.mp4\"}\n`file[Missing]{src=\"static/missing.pdf\"}\n";
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
            "`meta\n `: title\n\n    Design Guide\n\n`-{.task #review due=\"2026-07-23T12:00:00+08:00\"} Review parser\n",
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
            "`-{.task #blocker} Blocker\n`-{.task #ready priority=7} Ready\n`-{.task #time wait=\"2026-07-23T12:00:00+08:00\"} Time wait\n`-{.task #dependency depends=\"#blocker\"} Dependency wait\n`-{.task #both wait=\"2026-07-23T12:00:00+08:00\" depends=\"#blocker\"} Both waits\n`-{.task #done done=\"2026-07-21T12:00:00+08:00\"} Done\n`-{.task #canceled canceled=\"2026-07-21T12:00:00+08:00\"} Canceled\n`-{.task #invalid done=\"2026-07-21T12:00:00+08:00\" canceled=\"2026-07-21T13:00:00+08:00\"} Invalid\n",
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
            by_id("invalid").task_state,
            Some(TaskWorkflowState::Invalid)
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
        assert_eq!(waiting.items.len(), 3);
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
    fn document_rename_strengthens_autolink_delimiters() {
        let mut workspace = Workspace::new();
        workspace.insert("notes/a.plumb", 1, "`#{#a} A\n");
        let reference = "`[a.plumb#a]{.->}\n";
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
        assert_eq!(edited, "`\"[archive/a] final.plumb#a]\"{.->}\n");
        assert!(parse(edited).is_valid());
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
            "`-{.task #review depends=\"Project Plan.plumb#draft Project Plan.plumb#done\"} Review\n",
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
        workspace.insert("remote.plumb", 1, "`-{.task #remote} Remote blocker\n");
        workspace.insert(
            "tasks.plumb",
            2,
            "`-{.task #parent done=\"2026-07-27T10:00:00Z\" depends=\"#explicit remote.plumb#remote\"} Completed parent\n  `-{.task #explicit} Explicit child\n  `-{.task} Implicit child\n  `-{.task canceled=\"2026-07-27T10:01:00Z\"} Canceled child\n`-{.task canceled=\"2026-07-27T10:02:00Z\"} Canceled parent\n  `-{.task} Open child is allowed\n`-{.task done=\"2026-07-27T10:03:00Z\"} Completed tree\n  `-{.task done=\"2026-07-27T10:04:00Z\"} Completed child\n",
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
            "`node{#plain} Plain anchor\n`-{.task #a depends=\"#b\"} A\n`-{.task #b depends=\"#a\"} B\n`-{.task #self depends=\"#self\"} Self\n`-{.task prev=\"#plain\" depends=\"#plain #missing bare#invalid missing.plumb#x\"} Invalid targets\n",
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
        assert_eq!(document.edits.len(), 1);
        let operation = &document.edits[0];
        let mut edited = source.to_string();
        edited.replace_range(operation.range.clone(), &operation.new_text);
        assert!(edited
            .contains("#write due=\"2026-07-21T09:00:00Z\" done=\"2026-07-20T12:00:00+08:00\""));
        assert_eq!(plumb_format::format(&edited).unwrap(), edited);
    }

    #[test]
    fn task_status_targets_an_explicitly_anchored_nested_task() {
        let source = "`-{#task-f81deb18 .task created=\"2026-05-24T02:35:50Z\"} MJCF in, USD out solver\n\n   `-{#task-9d49eb30 .task created=\"2026-05-24T02:35:32Z\" done=\"2026-05-26T01:43:39Z\"} 刚体版本\n   `-{#task-c2cf5756 .task created=\"2026-05-27T13:03:04Z\"} parse MJCF\n   `-{#task-99e28dad .task created=\"2026-05-27T13:02:45Z\"} solver with passive joint\n";
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

        assert!(edited.contains("#task-c2cf5756 .task created=\"2026-05-27T13:03:04Z\" done=\"2026-07-22T22:41:21+08:00\""));
        assert!(!edited.contains("#task-f81deb18 .task created=\"2026-05-24T02:35:50Z\" done="));
        assert!(!edited.contains("#task-99e28dad .task created=\"2026-05-27T13:02:45Z\" done="));
        assert_eq!(plumb_format::format(&edited).unwrap(), edited);
    }

    #[test]
    fn task_status_formats_multiline_attributes_with_a_long_head() {
        let source = "`-{\n   .task created=\"2026-07-21T14:37:59+08:00\"\n  } `->[如何在 nix 中检查 IFD]{to=\"如何在 nix 中检查 IFD.plumb\"}\n";
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
            "`-{\n   .task created=\"2026-07-21T14:37:59+08:00\" done=\"2026-07-21T21:52:24+08:00\"\n  } `->[如何在 nix 中检查 IFD]{to=\"如何在 nix 中检查 IFD.plumb\"}\n"
        );
        assert_eq!(plumb_format::format(&edited).unwrap(), edited);
    }

    #[test]
    fn task_status_formats_the_complete_owner_subtree() {
        let source = "`-{.task #parent} Parent\n       `- Child\n\n`# Following\n";
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

        assert!(edited.contains("#parent done=\"2026-07-21T22:00:00+08:00\""));
        assert!(edited.contains("\n   `- Child\n\n`# Following"));
        assert_eq!(plumb_format::format(&edited).unwrap(), edited);
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
        let mut converted = source.to_string();
        converted.replace_range(edit.range.clone(), &edit.new_text);
        assert!(converted.contains("  `-{.task created=\"2026-07-20T10:00:00+08:00\"} Nested"));

        let outer_conversion = workspace
            .convert_list_item_to_task("tasks.plumb", source.find("Outer").unwrap(), timestamp)
            .unwrap();
        assert!(outer_conversion.document_changes[0].edits[0]
            .new_text
            .contains("`-{#outer .keep .task created=\"2026-07-20T10:00:00+08:00\"} Outer"));

        let closed_offset = source.find("Closed").unwrap();
        let created = workspace
            .add_task_created("tasks.plumb", closed_offset, timestamp)
            .unwrap();
        assert!(created.document_changes[0].edits[0].new_text.contains(
            "#closed done=\"2026-07-20T09:00:00Z\" created=\"2026-07-20T10:00:00+08:00\""
        ));
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

        let conversion_source = "`-{#item .kind} Convert me\n";
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

        let created_source = "`-{.task #created} Add created\n";
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

        let id_source = "`note{.class key=value} Add an explicit identifier\n";
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
            .insert_metadata("metadata.plumb", "metadata", timestamp)
            .unwrap();
        let with_metadata = apply_single_edit(metadata_source, &metadata);
        assert_eq!(plumb_format::format(&with_metadata).unwrap(), with_metadata);
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
        assert!(edit
            .new_text
            .contains("`#{#hello-world-2 .keep} Hello, World!"));

        let nested = workspace
            .add_explicit_id("note.plumb", source.find("Nested title").unwrap())
            .unwrap();
        assert!(nested.document_changes[0].edits[0]
            .new_text
            .contains("`child{#nested-title} Nested title"));

        let sibling_boundary = workspace
            .add_explicit_id("note.plumb", source.find("`node").unwrap())
            .unwrap();
        assert!(sibling_boundary.document_changes[0].edits[0]
            .new_text
            .contains("`node{#outer} Outer"));

        let raw = workspace
            .add_explicit_id("note.plumb", source.find("raw").unwrap())
            .unwrap();
        assert!(raw.document_changes[0].edits[0]
            .new_text
            .contains("`{#block language=text}"));

        let multiline = workspace
            .add_explicit_id("note.plumb", source.find("Multiline attrs").unwrap())
            .unwrap();
        assert!(multiline.document_changes[0].edits[0]
            .new_text
            .contains("`note{#multiline-attrs .keep} Multiline attrs"));

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
        let operation = &edit.document_changes[0].edits[0];
        assert!(operation.range.start <= tasks[0].range.start);
        assert!(operation.range.end >= tasks[0].range.end);
        assert_ne!(operation.range.start, tasks[1].attribute_insert);
        let mut edited = source.to_string();
        edited.replace_range(operation.range.clone(), &operation.new_text);
        assert!(edited.contains("#outer done=\"2026-07-20T12:00:00Z\""));
        assert_eq!(edited.matches("#inner done=").count(), 1);
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
        assert_eq!(edits.len(), 1);
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
        let mut edited = source.to_string();
        for edit in edits {
            edited.replace_range(edit.range, &edit.new_text);
        }

        assert!(!edited.contains("\n\n`-{.task"));
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
    fn recurring_task_clone_preserves_crlf_and_nested_base_indent() {
        let source = "`node Parent\r\n  `-{.task due=\"2026-07-20T09:00:00+08:00\" recur=P1W} Weekly review\r\n";
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
        assert_eq!(&source[line_start..task.range.start], "  ");

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
        assert!(replacement.starts_with("  `-"), "{replacement:?}");
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
        assert!(!edited.contains("\r\n\r\n  `-{.task"));
    }

    #[test]
    fn recurring_task_completion_preserves_canonical_layout() {
        let source = "`# 饮食相关任务\n\n`-{\n   #控制饮食-2026-07-20 .task priority=-5 created=\"2026-07-20T01:06:48+08:00\"\n   due=\"2026-07-20T23:59:59+08:00\" wait=\"2026-07-20T00:00:00+08:00\" recur=\"P1D\"\n   prev=\"#控制饮食-2026-07-19\"\n  } 控制饮食\n\n`# 锻炼相关任务\n";
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

        assert!(edited.contains("done=\"2026-07-21T18:01:12+08:00\"\n  } 控制饮食\n`-{"));
        assert!(edited.contains("prev=\"#控制饮食-2026-07-20\"\n  } 控制饮食\n\n`# 锻炼相关任务"));
        assert_eq!(edited.matches("priority=-5").count(), 2);
        assert_eq!(plumb_format::format(&edited).unwrap(), edited);
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
    fn inserts_formatted_metadata_into_an_empty_document() {
        let mut workspace = Workspace::new();
        workspace.insert("notes/empty.plumb", 11, "");

        let edit = workspace
            .insert_metadata("notes/empty.plumb", "empty", "2026-07-22T12:34:56+08:00")
            .unwrap();

        let document = &edit.document_changes[0];
        assert_eq!(document.expected_revision, 11);
        assert_eq!(document.edits[0].range, 0..0);
        assert_eq!(
            document.edits[0].new_text,
            "`meta\n `: title\n\n    empty\n\n `: created\n\n    2026-07-22T12:34:56+08:00\n"
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

    #[test]
    fn resolves_event_task_associations_and_queries_time_ranges() {
        let mut workspace = Workspace::new();
        workspace.insert(
            "tasks.plumb",
            1,
            "`-{.task #write} Write\n`node{#plain} Plain\n",
        );
        let events = "`-{.event at=\"2026-07-30T10:30:00+05:00\"} Early\n`-{.event #review uid=\"same@example\" start=\"2026-07-30T14:00:00+08:00\" end=\"2026-07-30T15:00:00+08:00\" tasks=\"tasks.plumb#write\"} Review\n`-{.event uid=\"same@example\" at=\"2026-07-30T15:00:00+08:00\" tasks=\"tasks.plumb#plain missing.plumb#task bad\"} Point\n";
        workspace.insert("events.plumb", 2, events);

        let target = TaskRef {
            path: PathBuf::from("tasks.plumb"),
            id: "write".to_string(),
        };
        let associated = workspace.events_for_task(&target);
        assert_eq!(associated.len(), 1);
        assert_eq!(associated[0].event.title, "Review");

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
        assert_eq!(workspace.references_to("tasks.plumb", "write").len(), 1);
        assert_eq!(
            workspace.referenced_documents_from("events.plumb"),
            [PathBuf::from("tasks.plumb")]
        );

        let codes = workspace
            .diagnostics("events.plumb")
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();
        assert!(codes.contains(&"event.duplicate-uid"), "{codes:?}");
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
                Some("uid == 'same@example' && start < timestamp('2026-07-30T07:00:00Z')"),
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
        assert!(created_source.contains(".event"), "{created_source}");
        assert!(created_source.contains("uid=\""), "{created_source}");
        assert_eq!(
            plumb_format::format(&created_source).unwrap(),
            created_source
        );

        workspace.insert("agenda.plumb", 8, created_source.clone());
        let event = workspace
            .current_output(Path::new("agenda.plumb"))
            .unwrap()
            .events
            .events[0]
            .clone();
        let uid = event.uid.as_ref().unwrap().value.clone();
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
        assert!(updated_source.contains(&format!("uid=\"{uid}\"")));
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
        assert_eq!(
            apply_single_edit(&updated_source, &deleted),
            "`# Agenda\n\n"
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
    fn creates_nested_tasks_and_updates_fields_without_losing_owned_content() {
        let mut workspace = Workspace::new();
        let source = "`-{.task #parent custom=keep created=\"2026-07-01T09:00:00Z\"}\n  Parent\n\n  `note Keep details\n`-{.task #other} Other\n`# Following\n";
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
        assert!(created_source.contains(".task"), "{created_source}");
        assert!(created_source.contains("#task-"), "{created_source}");
        assert!(created_source.contains("priority=-2"));
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
        assert!(updated_source.contains("custom=keep"));
        assert!(updated_source.contains("#parent"));
        assert!(updated_source.contains("created=\"2026-07-01T09:00:00Z\""));
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
        assert!(patched_source.contains("priority=9"));
        assert!(patched_source.contains("due=\"2026-09-01T10:00:00Z\""));
        assert!(patched_source.contains("depends=\"#other\""));
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
            "`-{.task #a depends=\"#b\"} A\n`-{.task #b} B\n",
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
            "`-{.task #left} Left\n  `-{.task #a} A\n    `note A details\n  `-{.task #b} B\n`-{.task #right} Right\n",
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
        assert!(reordered_source.find("#b").unwrap() < reordered_source.find("#a").unwrap());
        assert!(reordered_source.contains("`note A details"));
        assert!(parse(&reordered_source).is_valid(), "{reordered_source}");
        assert!(
            reordered_source.contains("\n`-{.task #right}"),
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
