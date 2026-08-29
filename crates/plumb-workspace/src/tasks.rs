use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset};
use plumb_edit::{replace_owned_block, AttributePosition, EditSession, OwnedAttribute, OwnedBlock};
use plumb_semantics::{
    analyze_tasks, next_task_datetime, parse_task_reference_target, valid_task_datetime,
    TaskRecord, TaskReferenceTarget, TaskState, TaskStatus,
};

use super::{
    deepest_list_item, derive_task_workflow_state, normalize, parsed_block_with_range,
    prepare_recurring_task_clone, resolve_relative, single_document_edit, unique_task_instance_id,
    DocumentEntry, QueryResult, RecurringTaskCloneContext, TaskAuthoringError, TaskAuthoringInput,
    TaskWaitReason, TaskWorkflowState, Workspace, WorkspaceEdit, WorkspaceOperationError,
    WorkspaceQueryError,
};
use crate::store::StoredTaskKey;
use plumb_syntax::{Block, ParsedBlock};

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

impl From<TaskEditError> for WorkspaceOperationError<TaskEditError> {
    fn from(error: TaskEditError) -> Self {
        Self::Operation(error)
    }
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
    ) -> Result<QueryResult<Vec<ResolvedTaskDependency>>, WorkspaceQueryError> {
        let dependencies = self
            .task_dependencies_value(path.as_ref(), task)?
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
        Ok(self.query_result(self.task_dependencies_value(path.as_ref(), task)?))
    }

    pub(super) fn task_dependencies_value(
        &self,
        path: &Path,
        task: &TaskRecord,
    ) -> Result<Vec<ResolvedTaskDependency>, WorkspaceQueryError> {
        let path = normalize(path.as_ref());
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
            id: target_id.to_string(),
        };
        let mut blocking = Vec::new();
        for (path, task) in self.all_tasks()? {
            let Some(id) = &task.id else {
                continue;
            };
            if self
                .task_dependencies_value(&path, &task)?
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
        Ok(self.query_result(blocking))
    }

    pub fn is_task_blocked(
        &self,
        path: impl AsRef<Path>,
        task: &TaskRecord,
    ) -> Result<QueryResult<bool>, WorkspaceQueryError> {
        Ok(self.query_result(self.is_task_blocked_value(path.as_ref(), task)?))
    }

    pub(super) fn is_task_blocked_value(
        &self,
        path: &Path,
        task: &TaskRecord,
    ) -> Result<bool, WorkspaceQueryError> {
        Ok(self
            .task_dependencies_value(path, task)?
            .iter()
            .any(|dependency| dependency.task.state() == TaskState::Open))
    }

    pub fn task_workflow_state(
        &self,
        path: impl AsRef<Path>,
        task: &TaskRecord,
        now: DateTime<FixedOffset>,
    ) -> Result<QueryResult<(TaskWorkflowState, Vec<TaskWaitReason>)>, WorkspaceQueryError> {
        let value =
            derive_task_workflow_state(task, self.is_task_blocked_value(path.as_ref(), task)?, now);
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
            .filter(|entry| entry.parsed.is_valid())
            .ok_or(TaskEditError::StaleOrInvalidDocument)?;
        let item = deepest_list_item(&entry.parsed.syntax.blocks, offset)
            .ok_or(TaskEditError::ListItemNotFound)?;
        let mark = item.mark.as_ref().expect("list item has a mark");
        if mark.attrs.has_class("task") {
            return Err(TaskEditError::TaskAlreadyExists);
        }
        let mut owned = OwnedBlock::from_parsed(&entry.parsed.source, item);
        owned.retain_attributes(
            |attribute| !matches!(attribute, OwnedAttribute::Class(value) if value == "task"),
        );
        owned.prepend_attribute(OwnedAttribute::class("task"));
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
            .filter(|entry| entry.parsed.is_valid())
            .ok_or(TaskEditError::StaleOrInvalidDocument)?;
        let pending_tasks;
        let tasks = if let Some(current) = &entry.current {
            &current.output.tasks.tasks
        } else {
            pending_tasks = analyze_tasks(
                entry
                    .parsed
                    .valid_syntax()
                    .expect("valid parsed document checked"),
            );
            &pending_tasks.tasks
        };
        let task = tasks
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
    ) -> Result<WorkspaceEdit, WorkspaceOperationError<TaskEditError>> {
        if task.state() != TaskState::Open {
            return Err(TaskEditError::TaskAlreadyClosed.into());
        }
        if task.recur.is_some() && task.due.is_some() {
            if status == TaskStatus::Done
                && self
                    .is_task_blocked_value(path, task)
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
                .is_task_blocked_value(path, task)
                .map_err(WorkspaceOperationError::Query)?
        {
            return Err(TaskEditError::TaskBlocked.into());
        }
        let block = parsed_block_with_range(&entry.parsed.syntax.blocks, &task.range)
            .ok_or(TaskEditError::TaskNotFound)?;
        if block.mark.is_none() {
            return Err(TaskEditError::TaskNotFound.into());
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
    ) -> Result<TaskTargetResolution, WorkspaceQueryError> {
        let (path, id) = match target {
            TaskReferenceTarget::Internal { id } => (normalize(from), id.clone()),
            TaskReferenceTarget::External { path, id } => {
                (resolve_relative(from, path), id.clone())
            }
            TaskReferenceTarget::Invalid => return Ok(TaskTargetResolution::Invalid),
        };
        if !self.contains_path(&path)? && !path.is_file() {
            return Ok(TaskTargetResolution::UnresolvedPath { path });
        }
        let matching_anchors = self.anchors_named(&path, &id)?.len();
        if matching_anchors == 0 {
            return Ok(TaskTargetResolution::UnresolvedAnchor { path, id });
        }
        if matching_anchors > 1 {
            return Ok(TaskTargetResolution::AmbiguousAnchor { path, id });
        }
        let Some(task) = self
            .tasks_for_path(&path)?
            .into_iter()
            .find(|task| task.id.as_ref().is_some_and(|task_id| task_id.value == id))
        else {
            return Ok(TaskTargetResolution::NotTask { path, id });
        };
        Ok(TaskTargetResolution::Task {
            target: TaskRef { path, id },
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
            for anchor in &current.output.anchors {
                *anchor_counts
                    .entry(TaskRef {
                        path: entry.path.clone(),
                        id: anchor.id.value.clone(),
                    })
                    .or_default() += 1;
            }
            for task in &current.output.tasks.tasks {
                let key = StoredTaskKey {
                    path: entry.path.clone(),
                    start: task.range.start,
                };
                if let Some(id) = &task.id {
                    let task_ref = TaskRef {
                        path: entry.path.clone(),
                        id: id.value.clone(),
                    };
                    *task_counts.entry(task_ref.clone()).or_default() += 1;
                    task_by_key.insert(key.clone(), task_ref);
                }
                for dependency in &task.depends {
                    if let Some(target) = dependency_task_ref(&entry.path, &dependency.target) {
                        relations.push((key.clone(), target));
                    }
                }
            }
        }
        if let Some(store) = &self.disk_store {
            for (path, id) in store.anchor_identities(&open_paths)? {
                *anchor_counts.entry(TaskRef { path, id }).or_default() += 1;
            }
            for fact in store.task_facts(&open_paths)? {
                let Some(id) = fact.id else {
                    continue;
                };
                let task_ref = TaskRef {
                    path: fact.path.clone(),
                    id,
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
            task_counts.get(task_ref) == Some(&1) && anchor_counts.get(task_ref) == Some(&1)
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

fn dependency_task_ref(source_path: &Path, target: &TaskReferenceTarget) -> Option<TaskRef> {
    match target {
        TaskReferenceTarget::Internal { id } => Some(TaskRef {
            path: normalize(source_path),
            id: id.clone(),
        }),
        TaskReferenceTarget::External { path, id } => Some(TaskRef {
            path: resolve_relative(source_path, path),
            id: id.clone(),
        }),
        TaskReferenceTarget::Invalid => None,
    }
}

pub(super) fn block_index_path(
    blocks: &[Block],
    target: &std::ops::Range<usize>,
) -> Option<Vec<usize>> {
    for (index, block) in blocks.iter().enumerate() {
        if block.range() == target {
            return Some(vec![index]);
        }
        let Block::Parsed(block) = block else {
            continue;
        };
        if block.range.start <= target.start && target.end <= block.range.end {
            if let Some(mut path) = block_index_path(&block.children, target) {
                path.insert(0, index);
                return Some(path);
            }
        }
    }
    None
}

pub(super) fn adjust_path_after_removal(path: &mut [usize], removed: &[usize]) {
    for (target, source) in path.iter_mut().zip(removed) {
        if *target == *source {
            continue;
        }
        if *source < *target {
            *target -= 1;
        }
        break;
    }
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

pub(super) fn remove_owned_at_path(owned: &mut OwnedBlock, path: &[usize]) -> Option<OwnedBlock> {
    let (index, parent_path) = path.split_last()?;
    let parent = owned_at_path_mut(owned, parent_path)?;
    let children = parent.children_mut()?;
    (*index < children.len()).then(|| children.remove(*index))
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
    OwnedBlock::marked("-", &input.title).with_attributes(attributes)
}

pub(super) fn updated_owned_task(
    source: &str,
    block: &ParsedBlock,
    task: &TaskRecord,
    input: &TaskAuthoringInput,
    timestamp: &str,
) -> OwnedBlock {
    let mut owned = OwnedBlock::from_parsed(source, block);
    owned.set_head_text(&input.title);
    let mut attributes = owned.attributes();
    attributes.retain(|attribute| {
        !matches!(attribute, OwnedAttribute::Pair { key, .. }
            if matches!(key.as_str(), "created" | "due" | "wait" | "recur" | "prev" | "depends" | "priority"))
    });
    append_authored_task_fields(
        &mut attributes,
        input,
        task.created
            .as_ref()
            .map_or(timestamp, |created| created.value.as_str()),
    );
    owned.with_attributes(attributes)
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

pub(super) fn child_insertion_index(
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

pub(super) fn remove_owned_descendant(
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
