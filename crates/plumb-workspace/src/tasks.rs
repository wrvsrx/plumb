use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset};
use plumb_edit::{
    append_green_declaration_item, edit_green_root_declarations, green_declaration_at,
    green_root_declaration, own_deepest_green_marked_block, own_green_block, own_green_block_paths,
    replace_green_block, replace_green_blocks, set_green_declaration, OwnedAttribute, OwnedBlock,
    OwnedDeclaration, OwnedDeclarationValue, RootDeclarationEdit,
};
use plumb_semantics::{
    analyze_green_document_task, analyze_green_tasks, next_task_datetime,
    parse_task_reference_target, valid_task_datetime, SemanticRecords, TaskOwner, TaskRecord,
    TaskReferenceTarget, TaskState, TaskStatus,
};

use super::{
    derive_task_workflow_state, normalize, prepare_recurring_task_clone, resolve_relative,
    single_document_edit, single_document_edits, unique_task_instance_id, DocumentEntry,
    QueryResult, RecurringTaskCloneContext, TaskAuthoringError, TaskAuthoringInput, TaskWaitReason,
    TaskWorkflowState, Workspace, WorkspaceEdit, WorkspaceOperationError, WorkspaceQueryError,
};
use crate::store::StoredTaskKey;

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
    GeneratedInvalid,
    InvalidFocusHistory,
    ClockRegression,
}

impl std::fmt::Display for TaskEditError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::StaleOrInvalidDocument => "task document is stale or invalid",
            Self::TaskNotFound => "task was not found",
            Self::TaskAlreadyClosed => "task is already closed",
            Self::TaskBlocked => "task is blocked by open dependencies",
            Self::InvalidRecurrence => "task recurrence is invalid",
            Self::InvalidTimestamp => "operation timestamp is invalid",
            Self::ListItemNotFound => "task list item was not found",
            Self::TaskAlreadyExists => "the list item is already a task",
            Self::CreatedAlreadyExists => "the task already has a created timestamp",
            Self::GeneratedInvalid => "the generated task edit is invalid",
            Self::InvalidFocusHistory => "task focus history is invalid",
            Self::ClockRegression => {
                "the operation timestamp would violate focus interval ordering"
            }
        })
    }
}

impl std::error::Error for TaskEditError {}

impl From<TaskEditError> for WorkspaceOperationError<TaskEditError> {
    fn from(error: TaskEditError) -> Self {
        Self::Operation(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskRef {
    pub path: PathBuf,
    /// None identifies the document task, never an anonymous list item.
    pub id: Option<String>,
}

impl TaskRef {
    pub fn display(&self, root: &Path) -> String {
        let path = crate::display_workspace_path(root, &self.path);
        self.id
            .as_ref()
            .map_or_else(|| path.clone(), |id| format!("{path}#{id}"))
    }

    pub(crate) fn from_task(path: &Path, task: &TaskRecord) -> Option<Self> {
        Self::from_parts(
            path,
            task.owner == TaskOwner::Document,
            task.id.as_ref().map(|id| id.value.clone()),
        )
    }

    pub(crate) fn from_parts(path: &Path, document_task: bool, id: Option<String>) -> Option<Self> {
        (document_task || id.is_some()).then(|| Self {
            path: normalize(path),
            id: if document_task { None } else { id },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTaskDependency {
    pub source: String,
    pub target: TaskRef,
    pub task: TaskRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TaskTargetResolution {
    Task {
        target: TaskRef,
        task: Box<TaskRecord>,
    },
    Invalid,
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
    NotDocumentTask {
        path: PathBuf,
    },
    NotTask {
        path: PathBuf,
        id: String,
    },
}

impl Workspace {
    /// Mark the document itself as a task; adding the identity never wraps its body.
    pub fn mark_document_task(
        &self,
        path: impl AsRef<Path>,
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
        let green = entry.parsed.green();
        let mut created = None;
        for key in [
            "created", "due", "wait", "done", "canceled", "recur", "priority", "prev", "depends",
            "focused",
        ] {
            let declaration =
                green_root_declaration(green, key).map_err(|_| TaskEditError::GeneratedInvalid)?;
            if !matches!(key, "focused" | "depends")
                && declaration.as_ref().is_some_and(|declaration| {
                    !matches!(declaration.value, OwnedDeclarationValue::Scalar(_))
                })
            {
                return Err(TaskEditError::GeneratedInvalid);
            }
            if key == "created" {
                created = declaration;
            }
        }
        let mut intents = vec![RootDeclarationEdit::SetFacet {
            name: "task".into(),
            present: true,
        }];
        if created.is_none() {
            intents.push(RootDeclarationEdit::SetProperty(OwnedDeclaration::scalar(
                "created", timestamp,
            )));
        }
        let edits = edit_green_root_declarations(green, &intents)
            .map_err(|_| TaskEditError::GeneratedInvalid)?;
        let revision;
        let valid = if edits.is_empty() {
            green.valid_syntax()
        } else {
            let proposed = plumb_edit::apply_text_edits(green.source().to_owned(), edits.clone())
                .map_err(|_| TaskEditError::GeneratedInvalid)?;
            revision = green.reparse(proposed);
            revision.document.valid_syntax()
        }
        .ok_or(TaskEditError::GeneratedInvalid)?;
        if !analyze_green_document_task(valid).diagnostics.is_empty() {
            return Err(TaskEditError::GeneratedInvalid);
        }
        Ok(single_document_edits(entry, path, edits))
    }

    /// Remove only the root task facet, including duplicate spellings of that facet.
    pub fn remove_document_task(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<WorkspaceEdit, TaskEditError> {
        let path = normalize(path.as_ref());
        let entry = self
            .documents
            .get(&path)
            .filter(|entry| entry.current.is_some())
            .ok_or(TaskEditError::StaleOrInvalidDocument)?;
        let edits = edit_green_root_declarations(
            entry.parsed.green(),
            &[RootDeclarationEdit::SetFacet {
                name: "task".into(),
                present: false,
            }],
        )
        .map_err(|_| TaskEditError::GeneratedInvalid)?;
        Ok(single_document_edits(entry, path, edits))
    }

    pub fn document_task(&self, path: impl AsRef<Path>) -> Option<TaskRecord> {
        self.current_output(path.as_ref())?
            .tasks()
            .document_task()
            .map(|task| {
                let mut task = task.to_owned();
                super::apply_document_task_title(&mut task, path.as_ref());
                task
            })
    }

    pub fn focus_document_task(
        &self,
        path: impl AsRef<Path>,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, WorkspaceOperationError<TaskEditError>> {
        if !valid_task_datetime(timestamp) {
            return Err(TaskEditError::InvalidTimestamp.into());
        }
        let (entry, task) = self.document_task_entry(path.as_ref())?;
        self.focus_edit(entry, &entry.path, &task, timestamp)
            .map_err(Into::into)
    }

    pub fn unfocus_document_task(
        &self,
        path: impl AsRef<Path>,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, WorkspaceOperationError<TaskEditError>> {
        if !valid_task_datetime(timestamp) {
            return Err(TaskEditError::InvalidTimestamp.into());
        }
        let (entry, task) = self.document_task_entry(path.as_ref())?;
        self.unfocus_edit(entry, &entry.path, &task, timestamp)
            .map_err(Into::into)
    }

    pub fn set_document_task_status(
        &self,
        path: impl AsRef<Path>,
        status: TaskStatus,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, WorkspaceOperationError<TaskEditError>> {
        if !valid_task_datetime(timestamp) {
            return Err(TaskEditError::InvalidTimestamp.into());
        }
        let (entry, task) = self.document_task_entry(path.as_ref())?;
        self.task_status_edit(entry, &entry.path, &task, status, timestamp)
    }

    fn document_task_entry(
        &self,
        path: &Path,
    ) -> Result<(&DocumentEntry, TaskRecord), TaskEditError> {
        let entry = self
            .documents
            .get(&normalize(path))
            .filter(|entry| entry.current.is_some())
            .ok_or(TaskEditError::StaleOrInvalidDocument)?;
        let task = entry
            .current
            .as_ref()
            .unwrap()
            .output
            .tasks()
            .document_task()
            .ok_or(TaskEditError::TaskNotFound)?
            .to_owned();
        Ok((entry, task))
    }

    pub fn task_at(&self, path: impl AsRef<Path>, offset: usize) -> Option<TaskRecord> {
        self.current_output(path.as_ref())?
            .tasks()
            .tasks
            .iter()
            .filter(|task| task.range.start <= offset && offset <= task.range.end)
            .max_by_key(|task| (task.depth, task.range.start))
    }

    pub fn open_task_dependencies(
        &self,
        path: impl AsRef<Path>,
        task: &TaskRecord,
    ) -> Result<QueryResult<Vec<ResolvedTaskDependency>>, WorkspaceQueryError> {
        let dependencies = self
            .task_dependencies_value(path.as_ref(), &task)?
            .into_iter()
            .filter(|dependency| dependency.task.state() == TaskState::Open)
            .collect();
        Ok(self.query_result(dependencies))
    }

    pub fn task_dependencies(
        &self,
        path: impl AsRef<Path>,
        task: &TaskRecord,
    ) -> Result<QueryResult<Vec<ResolvedTaskDependency>>, WorkspaceQueryError> {
        Ok(self.query_result(self.task_dependencies_value(path.as_ref(), &task)?))
    }

    pub(super) fn task_dependencies_value(
        &self,
        path: &Path,
        task: &TaskRecord,
    ) -> Result<Vec<ResolvedTaskDependency>, WorkspaceQueryError> {
        let path = normalize(path);
        let mut dependencies = task
            .depends
            .iter()
            .map(|dependency| {
                let TaskTargetResolution::Task {
                    target,
                    task: target_task,
                } = self.resolve_task_target(&path, &dependency.target)?
                else {
                    return Ok(None);
                };
                Ok(Some(ResolvedTaskDependency {
                    source: dependency.source.clone(),
                    target,
                    task: *target_task,
                }))
            })
            .collect::<Result<Vec<_>, WorkspaceQueryError>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        dependencies.sort_by(|left, right| {
            left.target
                .path
                .cmp(&right.target.path)
                .then(left.target.id.cmp(&right.target.id))
        });
        Ok(dependencies)
    }

    pub fn task_previous(
        &self,
        path: impl AsRef<Path>,
        task: &TaskRecord,
    ) -> Result<QueryResult<Option<TaskRef>>, WorkspaceQueryError> {
        let path = normalize(path.as_ref());
        let Some(previous) = task.prev.as_ref() else {
            return Ok(self.query_result(None));
        };
        let target = parse_task_reference_target(&previous.value);
        let TaskTargetResolution::Task { target, .. } = self.resolve_task_target(&path, &target)?
        else {
            return Ok(self.query_result(None));
        };
        Ok(self.query_result(Some(target)))
    }

    pub fn directly_blocking_tasks(
        &self,
        target_path: impl AsRef<Path>,
        target_id: &str,
    ) -> Result<QueryResult<Vec<TaskRef>>, WorkspaceQueryError> {
        let target = TaskRef {
            path: normalize(target_path.as_ref()),
            id: Some(target_id.to_string()),
        };
        self.directly_blocking_task(&target)
    }

    pub fn directly_blocking_task(
        &self,
        target: &TaskRef,
    ) -> Result<QueryResult<Vec<TaskRef>>, WorkspaceQueryError> {
        let mut blocking = Vec::new();
        for (path, task) in self.all_tasks()? {
            let Some(task_ref) = TaskRef::from_task(&path, &task) else {
                continue;
            };
            if self
                .task_dependencies_value(&path, &task)?
                .iter()
                .any(|dependency| &dependency.target == target)
            {
                blocking.push(task_ref);
            }
        }
        blocking.sort_by(|left, right| left.path.cmp(&right.path).then(left.id.cmp(&right.id)));
        Ok(self.query_result(blocking))
    }

    pub fn is_task_blocked(
        &self,
        path: impl AsRef<Path>,
        task: &TaskRecord,
    ) -> Result<QueryResult<bool>, WorkspaceQueryError> {
        Ok(self.query_result(self.is_task_blocked_value(path.as_ref(), &task)?))
    }

    pub(super) fn is_task_blocked_value(
        &self,
        path: &Path,
        task: &TaskRecord,
    ) -> Result<bool, WorkspaceQueryError> {
        Ok(self
            .task_dependencies_value(path, &task)?
            .iter()
            .any(|dependency| dependency.task.state() == TaskState::Open))
    }

    pub fn task_workflow_state(
        &self,
        path: impl AsRef<Path>,
        task: &TaskRecord,
        now: DateTime<FixedOffset>,
    ) -> Result<QueryResult<(TaskWorkflowState, Vec<TaskWaitReason>)>, WorkspaceQueryError> {
        let value = derive_task_workflow_state(
            &task,
            self.is_task_blocked_value(path.as_ref(), &task)?,
            now,
        );
        Ok(self.query_result(value))
    }

    pub fn set_task_status(
        &self,
        path: impl AsRef<Path>,
        offset: usize,
        status: TaskStatus,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, WorkspaceOperationError<TaskEditError>> {
        if !valid_task_datetime(timestamp) {
            return Err(TaskEditError::InvalidTimestamp.into());
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
            .tasks()
            .tasks;
        let task = focus_task_at(tasks, offset)?;
        self.task_status_edit(entry, &path, &task, status, timestamp)
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
            .filter(|entry| entry.parsed.is_valid())
            .ok_or(TaskEditError::StaleOrInvalidDocument)?;
        let item = own_deepest_green_marked_block(entry.parsed.green(), offset, &["-", "."])
            .ok_or(TaskEditError::ListItemNotFound)?;
        if item
            .block
            .attributes()
            .iter()
            .any(|attribute| matches!(attribute, OwnedAttribute::Class(value) if value == "task"))
        {
            return Err(TaskEditError::TaskAlreadyExists);
        }
        let mut owned = item.block;
        owned.retain_attributes(
            |attribute| !matches!(attribute, OwnedAttribute::Class(value) if value == "task"),
        );
        owned.prepend_attribute(OwnedAttribute::class("task"));
        owned.push_attribute(OwnedAttribute::quoted("created", timestamp));
        let edit = replace_green_block(entry.parsed.green(), item.range, &owned)
            .map_err(|_| TaskEditError::GeneratedInvalid)?;
        Ok(single_document_edit(entry, path, edit))
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
            .filter(|entry| entry.parsed.is_valid())
            .ok_or(TaskEditError::StaleOrInvalidDocument)?;
        let pending_tasks;
        let tasks = if let Some(current) = &entry.current {
            &current.output.tasks().tasks
        } else {
            pending_tasks = analyze_green_tasks(
                entry
                    .parsed
                    .green()
                    .valid_syntax()
                    .expect("valid green document checked"),
            );
            &pending_tasks.tasks
        };
        let task = tasks
            .iter()
            .filter(|task| task.range.start <= offset && offset <= task.range.end)
            .max_by_key(|task| (task.depth, task.range.start))
            .ok_or(TaskEditError::TaskNotFound)?;
        if task.created.is_some() {
            return Err(TaskEditError::CreatedAlreadyExists);
        }
        if task.owner == TaskOwner::Document {
            let edits = edit_green_root_declarations(
                entry.parsed.green(),
                &[RootDeclarationEdit::SetProperty(OwnedDeclaration::scalar(
                    "created", timestamp,
                ))],
            )
            .map_err(|_| TaskEditError::GeneratedInvalid)?;
            return Ok(single_document_edits(entry, path, edits));
        }
        let mut owned = own_green_block(entry.parsed.green(), task.range.clone())
            .map_err(|_| TaskEditError::TaskNotFound)?;
        owned.push_attribute(OwnedAttribute::quoted("created", timestamp));
        let edit = replace_green_block(entry.parsed.green(), task.range.clone(), &owned)
            .map_err(|_| TaskEditError::GeneratedInvalid)?;
        Ok(single_document_edit(entry, path, edit))
    }

    fn task_status_edit(
        &self,
        entry: &DocumentEntry,
        path: &Path,
        task: &TaskRecord,
        status: TaskStatus,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, WorkspaceOperationError<TaskEditError>> {
        if task.state() != TaskState::Open {
            return Err(TaskEditError::TaskAlreadyClosed.into());
        }
        if task.owner == TaskOwner::Document && task.recur.is_some() {
            return Err(TaskEditError::InvalidRecurrence.into());
        }
        if task.recur.is_some() && task.due.is_some() {
            if status == TaskStatus::Done
                && self
                    .is_task_blocked_value(path, &task)
                    .map_err(WorkspaceOperationError::Query)?
            {
                return Err(TaskEditError::TaskBlocked.into());
            }
            return self
                .recurring_task_status_edit(entry, task, status, timestamp)
                .map_err(Into::into);
        }
        if status == TaskStatus::Done
            && self
                .is_task_blocked_value(path, &task)
                .map_err(WorkspaceOperationError::Query)?
        {
            return Err(TaskEditError::TaskBlocked.into());
        }
        if task.owner == TaskOwner::Document {
            let mut intents = vec![RootDeclarationEdit::SetProperty(OwnedDeclaration::scalar(
                status.attribute(),
                timestamp,
            ))];
            if !task.focus_valid() {
                return Err(TaskEditError::InvalidFocusHistory.into());
            }
            if task.has_open_focus_interval() {
                let existing = green_root_declaration(entry.parsed.green(), "focused")
                    .map_err(|_| TaskEditError::InvalidFocusHistory)?
                    .ok_or(TaskEditError::InvalidFocusHistory)?;
                intents.push(RootDeclarationEdit::SetProperty(closed_focus_declaration(
                    existing, task, timestamp,
                )?));
            }
            let edits = edit_green_root_declarations(entry.parsed.green(), &intents)
                .map_err(|_| TaskEditError::GeneratedInvalid)?;
            return Ok(single_document_edits(entry, path.to_path_buf(), edits));
        }
        let mut owned = own_green_block(entry.parsed.green(), task.range.clone())
            .map_err(|_| TaskEditError::TaskNotFound)?;
        owned.push_attribute(OwnedAttribute::quoted(status.attribute(), timestamp));
        // The closure field and the focus interval end share one owned rewrite, so
        // the produced proposal carries both in a single revision edit.
        close_task_focus(&mut owned, task, timestamp)?;
        let edit = replace_green_block(entry.parsed.green(), task.range.clone(), &owned)
            .map_err(|_| TaskEditError::GeneratedInvalid)?;
        Ok(single_document_edit(entry, path.to_path_buf(), edit))
    }

    pub fn set_task_status_by_id(
        &self,
        path: impl AsRef<Path>,
        id: &str,
        status: TaskStatus,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, WorkspaceOperationError<TaskEditError>> {
        if !valid_task_datetime(timestamp) {
            return Err(TaskEditError::InvalidTimestamp.into());
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
            .tasks()
            .tasks
            .iter()
            .find(|task| task.id.as_ref().is_some_and(|task_id| task_id.value == id))
            .ok_or(TaskEditError::TaskNotFound)?;
        self.task_status_edit(entry, &path, &task, status, timestamp)
    }

    /// Focus the deepest open task covering `offset` by appending one open focus
    /// interval that starts at `timestamp`.
    ///
    /// Idempotent when the task already has an open interval, and rejected for a
    /// closed task. A timestamp that would violate interval ordering fails with
    /// [`TaskEditError::ClockRegression`] and writes nothing.
    pub fn focus_task(
        &self,
        path: impl AsRef<Path>,
        offset: usize,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, WorkspaceOperationError<TaskEditError>> {
        if !valid_task_datetime(timestamp) {
            return Err(TaskEditError::InvalidTimestamp.into());
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
            .tasks()
            .tasks;
        let task = focus_task_at(tasks, offset)?;
        self.focus_edit(entry, &path, &task, timestamp)
            .map_err(WorkspaceOperationError::from)
    }

    /// Focus an explicitly identified task by appending one open focus interval.
    pub fn focus_task_by_id(
        &self,
        path: impl AsRef<Path>,
        id: &str,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, WorkspaceOperationError<TaskEditError>> {
        if !valid_task_datetime(timestamp) {
            return Err(TaskEditError::InvalidTimestamp.into());
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
            .tasks()
            .tasks
            .iter()
            .find(|task| task.id.as_ref().is_some_and(|task_id| task_id.value == id))
            .ok_or(TaskEditError::TaskNotFound)?;
        self.focus_edit(entry, &path, &task, timestamp)
            .map_err(WorkspaceOperationError::from)
    }

    /// Close the open focus interval of the deepest task covering `offset` with
    /// `timestamp`, in any closure state.
    ///
    /// Idempotent when no open interval exists. Invalid history and a timestamp
    /// that precedes the open interval's start fail without producing an edit.
    pub fn unfocus_task(
        &self,
        path: impl AsRef<Path>,
        offset: usize,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, WorkspaceOperationError<TaskEditError>> {
        if !valid_task_datetime(timestamp) {
            return Err(TaskEditError::InvalidTimestamp.into());
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
            .tasks()
            .tasks;
        let task = any_task_at(tasks, offset)?;
        self.unfocus_edit(entry, &path, &task, timestamp)
            .map_err(WorkspaceOperationError::from)
    }

    /// Close the open focus interval of an explicitly identified task.
    pub fn unfocus_task_by_id(
        &self,
        path: impl AsRef<Path>,
        id: &str,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, WorkspaceOperationError<TaskEditError>> {
        if !valid_task_datetime(timestamp) {
            return Err(TaskEditError::InvalidTimestamp.into());
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
            .tasks()
            .tasks
            .iter()
            .find(|task| task.id.as_ref().is_some_and(|task_id| task_id.value == id))
            .ok_or(TaskEditError::TaskNotFound)?;
        self.unfocus_edit(entry, &path, &task, timestamp)
            .map_err(WorkspaceOperationError::from)
    }

    fn focus_edit(
        &self,
        entry: &DocumentEntry,
        path: &Path,
        task: &TaskRecord,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, TaskEditError> {
        if task.state() != TaskState::Open {
            return Err(TaskEditError::TaskAlreadyClosed);
        }
        if !task.focus_valid() {
            return Err(TaskEditError::InvalidFocusHistory);
        }
        if task.is_focused() {
            return Ok(no_focus_edit(entry, path));
        }
        let green = entry.parsed.green();
        let existing = task_declaration(green, task, "focused")
            .map_err(|_| TaskEditError::InvalidFocusHistory)?;
        if existing.is_some() != task.focused.present {
            return Err(TaskEditError::InvalidFocusHistory);
        }
        if let Some(last) = task.focused.intervals.last() {
            let end = last
                .end
                .as_deref()
                .ok_or(TaskEditError::InvalidFocusHistory)?;
            if focus_precedes(timestamp, end) {
                return Err(TaskEditError::ClockRegression);
            }
        }
        let interval = format!("{timestamp}--");
        if task.owner == TaskOwner::Document {
            let declaration = match existing {
                None => OwnedDeclaration::scalar("focused", interval),
                Some(existing) => {
                    let mut items = existing
                        .items()
                        .into_iter()
                        .map(str::to_owned)
                        .collect::<Vec<_>>();
                    items.push(interval);
                    OwnedDeclaration::list("focused", items)
                }
            };
            let edits = edit_green_root_declarations(
                green,
                &[RootDeclarationEdit::SetProperty(declaration)],
            )
            .map_err(|_| TaskEditError::GeneratedInvalid)?;
            return Ok(single_document_edits(entry, path.to_path_buf(), edits));
        }
        let edit = append_green_declaration_item(green, task.range.clone(), "focused", &interval)
            .map_err(|_| TaskEditError::GeneratedInvalid)?
            .ok_or(TaskEditError::GeneratedInvalid)?;
        Ok(single_document_edit(entry, path.to_path_buf(), edit))
    }

    fn unfocus_edit(
        &self,
        entry: &DocumentEntry,
        path: &Path,
        task: &TaskRecord,
        timestamp: &str,
    ) -> Result<WorkspaceEdit, TaskEditError> {
        if !task.focus_valid() {
            return Err(TaskEditError::InvalidFocusHistory);
        }
        if !task.has_open_focus_interval() {
            return Ok(no_focus_edit(entry, path));
        }
        let green = entry.parsed.green();
        let existing = task_declaration(green, task, "focused")
            .map_err(|_| TaskEditError::InvalidFocusHistory)?
            .ok_or(TaskEditError::InvalidFocusHistory)?;
        let declaration = closed_focus_declaration(existing, task, timestamp)?;
        if task.owner == TaskOwner::Document {
            let edits = edit_green_root_declarations(
                green,
                &[RootDeclarationEdit::SetProperty(declaration)],
            )
            .map_err(|_| TaskEditError::GeneratedInvalid)?;
            return Ok(single_document_edits(entry, path.to_path_buf(), edits));
        }
        let edit = set_green_declaration(green, task.range.clone(), declaration)
            .map_err(|_| TaskEditError::GeneratedInvalid)?
            .ok_or(TaskEditError::GeneratedInvalid)?;
        Ok(single_document_edit(entry, path.to_path_buf(), edit))
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
            .anchors()
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

        let task_ranges = current
            .output
            .tasks()
            .tasks
            .iter()
            .filter(|candidate| {
                task.range.start <= candidate.range.start && candidate.range.end <= task.range.end
            })
            .map(|candidate| candidate.range.clone())
            .collect::<Vec<_>>();
        let owned = own_green_block_paths(entry.parsed.green(), task.range.clone(), &task_ranges)
            .map_err(|_| TaskEditError::TaskNotFound)?;
        let mut current = owned.block.clone();
        let mut next = owned.block;
        let clone_context = RecurringTaskCloneContext {
            next_id: &next_id,
            timestamp,
            next_due: &next_due,
            next_wait: next_wait.as_deref(),
            recur: &recur.value,
            current_id: &current_id,
        };
        prepare_recurring_task_clone(&mut next, &owned.target_paths, &clone_context);
        // `persistent_task_attribute` drops the leaf `focused` form while cloning.
        // The child-bearing interval list has no attribute projection, so remove it
        // explicitly: the next instance starts with no focus history of its own.
        next.remove_declaration("focused");

        if task.id.is_none() {
            current.push_attribute(OwnedAttribute::id(current_id));
        }
        current.push_attribute(OwnedAttribute::quoted(status.attribute(), timestamp));
        close_task_focus(&mut current, task, timestamp)?;
        let edit = replace_green_blocks(entry.parsed.green(), task.range.clone(), &[current, next])
            .map_err(|_| TaskEditError::GeneratedInvalid)?;
        Ok(single_document_edit(entry, entry.path.clone(), edit))
    }

    pub(super) fn resolve_task_target(
        &self,
        from: &Path,
        target: &TaskReferenceTarget,
    ) -> Result<TaskTargetResolution, WorkspaceQueryError> {
        let (path, id) = match target {
            TaskReferenceTarget::Internal { id } => (normalize(from), id.clone()),
            TaskReferenceTarget::External { path, id } => {
                (resolve_relative(from, path), id.clone())
            }
            TaskReferenceTarget::Document { path } => {
                let path = resolve_relative(from, path);
                if !self.contains_path(&path)? && !path.is_file() {
                    return Ok(TaskTargetResolution::UnresolvedPath { path });
                }
                return Ok(
                    match self
                        .tasks_for_path(&path)?
                        .into_iter()
                        .find(|task| task.owner == TaskOwner::Document)
                    {
                        Some(task) => TaskTargetResolution::Task {
                            target: TaskRef { path, id: None },
                            task: Box::new(task),
                        },
                        None => TaskTargetResolution::NotDocumentTask { path },
                    },
                );
            }
            TaskReferenceTarget::Invalid => return Ok(TaskTargetResolution::Invalid),
        };
        if !self.contains_path(&path)? && !path.is_file() {
            return Ok(TaskTargetResolution::UnresolvedPath { path });
        }
        let matching_anchors = self.anchors_named(&path, &id)?;
        if matching_anchors.is_empty() {
            return Ok(TaskTargetResolution::UnresolvedAnchor { path, id });
        }
        if matching_anchors.len() > 1 {
            return Ok(TaskTargetResolution::AmbiguousAnchor { path, id });
        }
        let start = matching_anchors[0].range.start;
        let task = if let Some(entry) = self.documents.get(&path) {
            entry.current.as_ref().and_then(|current| {
                current
                    .output
                    .tasks()
                    .list_task_at(start)
                    .filter(|task| task.id_value() == Some(id.as_str()))
                    .map(|task| task.to_owned())
            })
        } else if let Some(store) = &self.disk_store {
            store
                .tasks_by_keys(&[StoredTaskKey {
                    path: path.clone(),
                    start,
                }])?
                .into_iter()
                .map(|stored| stored.record)
                .find(|task| task.id.as_ref().is_some_and(|task_id| task_id.value == id))
        } else {
            None
        };
        let Some(task) = task else {
            return Ok(TaskTargetResolution::NotTask { path, id });
        };
        Ok(TaskTargetResolution::Task {
            target: TaskRef { path, id: Some(id) },
            task: Box::new(task),
        })
    }

    pub(super) fn task_dependency_graph(
        &self,
    ) -> Result<HashMap<TaskRef, Vec<TaskRef>>, WorkspaceQueryError> {
        let open_paths = self.open_paths();
        let mut task_by_key = HashMap::<StoredTaskKey, TaskRef>::new();
        let mut task_counts = HashMap::<TaskRef, usize>::new();
        let mut anchor_counts = HashMap::<TaskRef, usize>::new();
        let mut relations = Vec::<(StoredTaskKey, TaskRef)>::new();

        for entry in self.documents.values() {
            let Some(current) = &entry.current else {
                continue;
            };
            for anchor in current.output.anchors().views() {
                *anchor_counts
                    .entry(TaskRef {
                        path: entry.path.clone(),
                        id: Some(anchor.id_value().to_owned()),
                    })
                    .or_default() += 1;
            }
            for task in current.output.tasks().tasks.views() {
                let Some(task_ref) = TaskRef::from_parts(
                    &entry.path,
                    task.owner() == TaskOwner::Document,
                    task.id_value().map(str::to_owned),
                ) else {
                    continue;
                };
                let key = StoredTaskKey {
                    path: entry.path.clone(),
                    start: task.source_key(),
                };
                *task_counts.entry(task_ref.clone()).or_default() += 1;
                task_by_key.insert(key.clone(), task_ref);
                for dependency in task.dependency_targets() {
                    if let Some(target) = dependency_task_ref(&entry.path, dependency) {
                        relations.push((key.clone(), target));
                    }
                }
            }
        }
        if let Some(store) = &self.disk_store {
            for (path, id) in store.anchor_identities(&open_paths)? {
                *anchor_counts
                    .entry(TaskRef { path, id: Some(id) })
                    .or_default() += 1;
            }
            for fact in store.task_facts(&open_paths)? {
                let Some(task_ref) = TaskRef::from_parts(&fact.path, fact.document_task, fact.id)
                else {
                    continue;
                };
                *task_counts.entry(task_ref.clone()).or_default() += 1;
                task_by_key.insert(
                    StoredTaskKey {
                        path: fact.path,
                        start: fact.start,
                    },
                    task_ref,
                );
            }
            relations.extend(
                store
                    .task_dependency_relations(&open_paths)?
                    .into_iter()
                    .map(|relation| {
                        (
                            StoredTaskKey {
                                path: relation.source_path,
                                start: relation.source_start,
                            },
                            TaskRef {
                                path: relation.target_path,
                                id: relation.target_id,
                            },
                        )
                    }),
            );
        }

        let unique = |task_ref: &TaskRef| {
            task_counts.get(task_ref) == Some(&1)
                && (task_ref.id.is_none() || anchor_counts.get(task_ref) == Some(&1))
        };
        let mut graph = task_counts
            .keys()
            .filter(|task_ref| unique(task_ref))
            .cloned()
            .map(|task_ref| (task_ref, Vec::new()))
            .collect::<HashMap<_, _>>();
        for (source_key, target) in relations {
            let Some(source) = task_by_key.get(&source_key) else {
                continue;
            };
            if unique(source) && unique(&target) {
                graph.entry(source.clone()).or_default().push(target);
            }
        }
        for dependencies in graph.values_mut() {
            dependencies
                .sort_by(|left, right| left.path.cmp(&right.path).then(left.id.cmp(&right.id)));
            dependencies.dedup();
        }
        Ok(graph)
    }
}

pub(super) fn task_graph_inputs_equal(
    left: &plumb_semantics::DocumentOutput,
    right: &plumb_semantics::DocumentOutput,
) -> bool {
    (left.anchors() == right.anchors()
        || left
            .anchors()
            .views()
            .map(|anchor| anchor.id_value())
            .eq(right.anchors().views().map(|anchor| anchor.id_value())))
        && (left.tasks().tasks == right.tasks().tasks || {
            let mut left =
                left.tasks().tasks.views().filter(|task| {
                    task.owner() == TaskOwner::Document || task.id_value().is_some()
                });
            let mut right =
                right.tasks().tasks.views().filter(|task| {
                    task.owner() == TaskOwner::Document || task.id_value().is_some()
                });
            loop {
                match (left.next(), right.next()) {
                    (None, None) => break true,
                    (Some(left), Some(right))
                        if left.owner() == right.owner()
                            && left.id_value() == right.id_value()
                            && left.dependency_targets().eq(right.dependency_targets()) => {}
                    _ => break false,
                }
            }
        })
}

fn dependency_task_ref(source_path: &Path, target: &TaskReferenceTarget) -> Option<TaskRef> {
    match target {
        TaskReferenceTarget::Internal { id } => Some(TaskRef {
            path: normalize(source_path),
            id: Some(id.clone()),
        }),
        TaskReferenceTarget::External { path, id } => Some(TaskRef {
            path: resolve_relative(source_path, path),
            id: Some(id.clone()),
        }),
        TaskReferenceTarget::Document { path } => Some(TaskRef {
            path: resolve_relative(source_path, path),
            id: None,
        }),
        TaskReferenceTarget::Invalid => None,
    }
}

/// The deepest task covering `offset` in any closure state.
///
/// `unfocus` uses this so an explicitly requested interval end can still repair
/// a closed task whose hand-edited history kept an open interval. `focus` uses
/// the open-only locator below instead.
fn any_task_at(
    tasks: &SemanticRecords<TaskRecord>,
    offset: usize,
) -> Result<TaskRecord, TaskEditError> {
    tasks
        .iter()
        .filter(|task| task.range.start <= offset && offset <= task.range.end)
        .max_by_key(|task| (task.depth, task.range.start))
        .ok_or(TaskEditError::TaskNotFound)
}

/// Preserve list-ancestor fallback, but never promote a child-targeted action
/// to the document task just because no containing list task is open.
fn focus_task_at(
    tasks: &SemanticRecords<TaskRecord>,
    offset: usize,
) -> Result<TaskRecord, TaskEditError> {
    let deepest = any_task_at(tasks, offset)?;
    tasks
        .iter()
        .filter(|task| {
            task.state() == TaskState::Open
                && task.range.start <= offset
                && offset <= task.range.end
                && (task.owner != TaskOwner::Document || deepest.owner == TaskOwner::Document)
        })
        .max_by_key(|task| (task.depth, task.range.start))
        .ok_or(TaskEditError::TaskAlreadyClosed)
}

/// A successful idempotent operation still reports the revision it inspected,
/// but proposes no text edit.
fn no_focus_edit(entry: &DocumentEntry, path: &Path) -> WorkspaceEdit {
    single_document_edits(entry, path.to_path_buf(), Vec::new())
}

/// Whether `timestamp` is strictly before `reference` on the actual instant
/// timeline. Unparseable values count as a regression rather than a guess.
fn focus_precedes(timestamp: &str, reference: &str) -> bool {
    match (
        DateTime::parse_from_rfc3339(timestamp),
        DateTime::parse_from_rfc3339(reference),
    ) {
        (Ok(timestamp), Ok(reference)) => timestamp < reference,
        _ => true,
    }
}

/// End the open focus interval of `task` inside an already owned task subtree.
///
/// This is how Complete and Cancel stay atomic: the caller writes the closure
/// field and the interval end through the same owned rewrite. A task without a
/// `focused` property or without an open interval is left untouched. An
/// unrepresentable or invalid history and a timestamp that precedes the open
/// interval's start fail before any edit is produced.
fn close_task_focus(
    owned: &mut OwnedBlock,
    task: &TaskRecord,
    timestamp: &str,
) -> Result<(), TaskEditError> {
    if !task.focused.present || !task.has_open_focus_interval() {
        return Ok(());
    }
    if !task.focus_valid() {
        return Err(TaskEditError::InvalidFocusHistory);
    }
    let declaration = owned
        .declaration("focused")
        .ok_or(TaskEditError::InvalidFocusHistory)?;
    owned
        .set_declaration(closed_focus_declaration(declaration, task, timestamp)?)
        .map_err(|_| TaskEditError::GeneratedInvalid)?;
    Ok(())
}

fn task_declaration(
    green: &plumb_syntax::GreenDocument,
    task: &TaskRecord,
    key: &str,
) -> Result<Option<OwnedDeclaration>, plumb_edit::EditError> {
    match task.owner {
        TaskOwner::Document => green_root_declaration(green, key),
        TaskOwner::ListItem => green_declaration_at(green, task.range.clone(), key),
    }
}

fn closed_focus_declaration(
    mut declaration: OwnedDeclaration,
    task: &TaskRecord,
    timestamp: &str,
) -> Result<OwnedDeclaration, TaskEditError> {
    if !task.focus_valid() {
        return Err(TaskEditError::InvalidFocusHistory);
    }
    let mut items = declaration
        .items()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if items.len() != task.focused.intervals.len() {
        return Err(TaskEditError::InvalidFocusHistory);
    }
    let last = task
        .focused
        .intervals
        .last()
        .ok_or(TaskEditError::InvalidFocusHistory)?;
    if focus_precedes(timestamp, &last.start) {
        return Err(TaskEditError::ClockRegression);
    }
    let item = items.last_mut().ok_or(TaskEditError::InvalidFocusHistory)?;
    let start = item.strip_suffix("--").unwrap_or(item.as_str()).to_owned();
    *item = format!("{start}--{timestamp}");
    declaration.value = match declaration.value {
        OwnedDeclarationValue::Scalar(_) => OwnedDeclarationValue::Scalar(items.remove(0)),
        OwnedDeclarationValue::List(_) => OwnedDeclarationValue::List(items),
    };
    Ok(declaration)
}

pub(super) fn owned_at_path_mut<'a>(
    owned: &'a mut OwnedBlock,
    path: &[usize],
) -> Option<&'a mut OwnedBlock> {
    let Some((index, remaining)) = path.split_first() else {
        return Some(owned);
    };
    let child = owned.children_mut()?.get_mut(*index)?;
    owned_at_path_mut(child, remaining)
}

pub(super) fn validate_task_authoring_input(
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

pub(super) fn owned_authored_task(
    input: &TaskAuthoringInput,
    id: &str,
    timestamp: &str,
) -> OwnedBlock {
    let mut attributes = vec![OwnedAttribute::class("task"), OwnedAttribute::id(id)];
    append_authored_task_fields(&mut attributes, input, timestamp);
    OwnedBlock::marked("-", &input.title).with_aligned_attributes(attributes)
}

pub(super) fn update_owned_task(
    mut owned: OwnedBlock,
    task: &TaskRecord,
    input: &TaskAuthoringInput,
    timestamp: &str,
) -> OwnedBlock {
    owned.set_head_text(&input.title);
    owned.retain_attributes(|attribute| {
        !matches!(attribute, OwnedAttribute::Pair { key, .. }
            if matches!(key.as_str(), "created" | "due" | "wait" | "recur" | "prev" | "depends" | "priority"))
    });
    let mut attributes = Vec::new();
    append_authored_task_fields(
        &mut attributes,
        input,
        task.created
            .as_ref()
            .map_or(timestamp, |created| created.value.as_str()),
    );
    owned.extend_attributes(attributes);
    owned
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
