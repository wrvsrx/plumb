use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset};
use plumb_edit::{replace_owned_block, AttributePosition, EditSession, OwnedAttribute, OwnedBlock};
use plumb_semantics::{
    next_task_datetime, parse_task_reference_target, valid_task_datetime, TaskRecord,
    TaskReferenceTarget, TaskState, TaskStatus,
};

use super::{
    deepest_list_item, derive_task_workflow_state, normalize, parsed_block_with_range,
    prepare_recurring_task_clone, resolve_relative, single_document_edit, unique_task_instance_id,
    DocumentEntry, RecurringTaskCloneContext, TaskWaitReason, TaskWorkflowState, Workspace,
    WorkspaceEdit,
};

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
        })
    }
}

impl std::error::Error for TaskEditError {}

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
    NotTask {
        path: PathBuf,
        id: String,
    },
}

impl Workspace {
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
                    task: *target_task,
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

    pub fn task_previous(&self, path: impl AsRef<Path>, task: &TaskRecord) -> Option<TaskRef> {
        let path = normalize(path.as_ref());
        let previous = task.prev.as_ref()?;
        let target = parse_task_reference_target(&previous.value);
        let TaskTargetResolution::Task { target, .. } = self.resolve_task_target(&path, &target)
        else {
            return None;
        };
        Some(target)
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
        for (path, task) in self.all_tasks() {
            let Some(id) = &task.id else {
                continue;
            };
            if self
                .task_dependencies(&path, &task)
                .iter()
                .any(|dependency| dependency.target == target)
            {
                blocking.push(TaskRef {
                    path,
                    id: id.value.clone(),
                });
            }
        }
        blocking.sort_by(|left, right| left.path.cmp(&right.path).then(left.id.cmp(&right.id)));
        blocking
    }

    pub fn is_task_blocked(&self, path: impl AsRef<Path>, task: &TaskRecord) -> bool {
        !self.open_task_dependencies(path, task).is_empty()
    }

    pub fn task_workflow_state(
        &self,
        path: impl AsRef<Path>,
        task: &TaskRecord,
        now: DateTime<FixedOffset>,
    ) -> (TaskWorkflowState, Vec<TaskWaitReason>) {
        derive_task_workflow_state(task, self.is_task_blocked(path, task), now)
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
        if mark.marker == "task" {
            return Err(TaskEditError::TaskAlreadyExists);
        }
        let mut owned = OwnedBlock::from_parsed(&entry.parsed.source, item);
        owned.set_marker("task");
        owned.retain_attributes(
            |attribute| !matches!(attribute, OwnedAttribute::Class(value) if value == "task"),
        );
        owned.push_attribute(OwnedAttribute::quoted("created", timestamp));
        let edit = replace_owned_block(&entry.parsed, item.range.clone(), &owned)
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
        let block = parsed_block_with_range(&entry.parsed.syntax.blocks, &task.range)
            .ok_or(TaskEditError::TaskNotFound)?;
        let mark = block.mark.as_ref().ok_or(TaskEditError::TaskNotFound)?;
        let mut edit = EditSession::new(&entry.parsed, block.range.clone())
            .map_err(|_| TaskEditError::GeneratedInvalid)?;
        edit.insert_attribute(
            &mark.attrs,
            mark.marker_range.end,
            AttributePosition::Last,
            OwnedAttribute::quoted("created", timestamp),
        )
        .map_err(|_| TaskEditError::GeneratedInvalid)?;
        let edit = edit.finish().map_err(|_| TaskEditError::GeneratedInvalid)?;
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
            if status == TaskStatus::Done && self.is_task_blocked(path, task) {
                return Err(TaskEditError::TaskBlocked);
            }
            return self.recurring_task_status_edit(entry, task, status, timestamp);
        }
        if status == TaskStatus::Done && self.is_task_blocked(path, task) {
            return Err(TaskEditError::TaskBlocked);
        }
        let block = parsed_block_with_range(&entry.parsed.syntax.blocks, &task.range)
            .ok_or(TaskEditError::TaskNotFound)?;
        if block.mark.is_none() {
            return Err(TaskEditError::TaskNotFound);
        }
        let mut owned = OwnedBlock::from_parsed(&entry.parsed.source, block);
        owned.push_attribute(OwnedAttribute::quoted(status.attribute(), timestamp));
        let mut edit = EditSession::new(&entry.parsed, block.range.clone())
            .map_err(|_| TaskEditError::GeneratedInvalid)?;
        edit.replace_block(block.range.clone(), &owned)
            .map_err(|_| TaskEditError::GeneratedInvalid)?;
        let edit = edit.finish().map_err(|_| TaskEditError::GeneratedInvalid)?;
        Ok(single_document_edit(entry, path.to_path_buf(), edit))
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
        let block = parsed_block_with_range(&entry.parsed.syntax.blocks, &task.range)
            .ok_or(TaskEditError::TaskNotFound)?;
        if block.mark.is_none() {
            return Err(TaskEditError::TaskNotFound);
        }
        let mut next = OwnedBlock::from_parsed(source, block);
        let clone_context = RecurringTaskCloneContext {
            tasks: &current.output.tasks.tasks,
            root: task,
            next_id: &next_id,
            timestamp,
            next_due: &next_due,
            next_wait: next_wait.as_deref(),
            recur: &recur.value,
            current_id: &current_id,
        };
        prepare_recurring_task_clone(&mut next, block, &clone_context);

        let mut current = OwnedBlock::from_parsed(source, block);
        if task.id.is_none() {
            current.push_attribute(OwnedAttribute::id(current_id));
        }
        current.push_attribute(OwnedAttribute::quoted(status.attribute(), timestamp));
        let mut edit = EditSession::new(&entry.parsed, task.range.clone())
            .map_err(|_| TaskEditError::GeneratedInvalid)?;
        edit.replace_block_with_blocks(task.range.clone(), &[current, next])
            .map_err(|_| TaskEditError::GeneratedInvalid)?;
        let edit = edit.finish().map_err(|_| TaskEditError::GeneratedInvalid)?;
        Ok(single_document_edit(entry, entry.path.clone(), edit))
    }

    pub(super) fn resolve_task_target(
        &self,
        from: &Path,
        target: &TaskReferenceTarget,
    ) -> TaskTargetResolution {
        let (path, id) = match target {
            TaskReferenceTarget::Internal { id } => (normalize(from), id.clone()),
            TaskReferenceTarget::External { path, id } => {
                (resolve_relative(from, path), id.clone())
            }
            TaskReferenceTarget::Invalid => return TaskTargetResolution::Invalid,
        };
        if !self.contains(&path) && !path.is_file() {
            return TaskTargetResolution::UnresolvedPath { path };
        }
        let matching_anchors = self.anchors_named(&path, &id).len();
        if matching_anchors == 0 {
            return TaskTargetResolution::UnresolvedAnchor { path, id };
        }
        if matching_anchors > 1 {
            return TaskTargetResolution::AmbiguousAnchor { path, id };
        }
        let Some(task) = self
            .tasks_for_path(&path)
            .into_iter()
            .find(|task| task.id.as_ref().is_some_and(|task_id| task_id.value == id))
        else {
            return TaskTargetResolution::NotTask { path, id };
        };
        TaskTargetResolution::Task {
            target: TaskRef { path, id },
            task: Box::new(task),
        }
    }

    pub(super) fn task_dependency_graph(&self) -> HashMap<TaskRef, Vec<TaskRef>> {
        let mut graph = HashMap::new();
        for (path, task) in self.all_tasks() {
            let Some(id) = &task.id else {
                continue;
            };
            let task_ref = TaskRef {
                path: path.clone(),
                id: id.value.clone(),
            };
            let dependencies = task
                .depends
                .iter()
                .filter_map(|dependency| {
                    let TaskTargetResolution::Task { target, .. } =
                        self.resolve_task_target(&path, &dependency.target)
                    else {
                        return None;
                    };
                    Some(target)
                })
                .collect();
            graph.insert(task_ref, dependencies);
        }
        graph
    }
}
