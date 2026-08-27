use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use cel::{Context, ExecutionError, Program, Value};
use chrono::{DateTime, FixedOffset};
use plumb_semantics::{EventRecord, MetadataValue, TaskRecord, TaskState};

use crate::{display_workspace_path, normalize, TaskRef, VersionedDocumentOutput, Workspace};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchRecordKind {
    Note,
    Task,
    Event,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskWorkflowState {
    Ready,
    Waiting,
    Blocked,
    Done,
    Canceled,
    Conflicted,
}

impl TaskWorkflowState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Waiting => "waiting",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Canceled => "canceled",
            Self::Conflicted => "conflicted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskWaitReason {
    Time,
    Dependency,
}

impl TaskWaitReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Time => "time",
            Self::Dependency => "dependency",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRecord {
    pub kind: SearchRecordKind,
    pub title: String,
    pub path: PathBuf,
    pub relative_path: String,
    pub range: std::ops::Range<usize>,
    pub revision: i64,
    pub id: Option<String>,
    pub task_state: Option<TaskWorkflowState>,
    pub wait_reasons: Option<Vec<TaskWaitReason>>,
    pub due: Option<String>,
    pub priority: Option<i32>,
    pub effective_priority: Option<i32>,
    pub blocked: Option<bool>,
    pub actionable: Option<bool>,
    pub depth: Option<usize>,
    pub at: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub tasks: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResults {
    pub items: Vec<SearchRecord>,
    pub complete: bool,
}

#[derive(Debug)]
pub enum WorkspaceSearchError {
    Filter(String),
    Query(super::WorkspaceQueryError),
}

impl std::fmt::Display for WorkspaceSearchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Filter(message) => formatter.write_str(message),
            Self::Query(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for WorkspaceSearchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Filter(_) => None,
            Self::Query(error) => Some(error),
        }
    }
}

impl From<String> for WorkspaceSearchError {
    fn from(message: String) -> Self {
        Self::Filter(message)
    }
}

impl From<super::WorkspaceQueryError> for WorkspaceSearchError {
    fn from(error: super::WorkspaceQueryError) -> Self {
        Self::Query(error)
    }
}

impl From<super::StoreError> for WorkspaceSearchError {
    fn from(error: super::StoreError) -> Self {
        Self::Query(super::WorkspaceQueryError::Store(error))
    }
}

impl Workspace {
    pub fn search_records(
        &self,
        root: impl AsRef<std::path::Path>,
        kind: Option<SearchRecordKind>,
        query: &str,
        limit: usize,
        now: DateTime<FixedOffset>,
    ) -> Result<super::QueryResult<SearchResults>, super::WorkspaceQueryError> {
        self.search_records_filtered(root, kind, query, limit, now, None)
            .map_err(|error| match error {
                WorkspaceSearchError::Query(error) => error,
                WorkspaceSearchError::Filter(_) => {
                    unreachable!("search without a semantic filter cannot fail")
                }
            })
    }

    pub fn search_records_filtered(
        &self,
        root: impl AsRef<std::path::Path>,
        kind: Option<SearchRecordKind>,
        query: &str,
        limit: usize,
        now: DateTime<FixedOffset>,
        filter: Option<&str>,
    ) -> Result<super::QueryResult<SearchResults>, WorkspaceSearchError> {
        let root = normalize(root.as_ref());
        let filter = filter
            .map(|source| SemanticSearchFilter::compile(source, now))
            .transpose()?;
        let reverse = filter
            .as_ref()
            .filter(|filter| filter.needs_reverse_references())
            .map(|_| ReverseReferences::build(self))
            .transpose()?;
        let task_dependents = filter
            .as_ref()
            .filter(|filter| filter.needs_task_dependents())
            .filter(|_| kind.is_none_or(|kind| kind == SearchRecordKind::Task))
            .map(|_| DirectTaskDependents::build(self))
            .transpose()?;
        let mut matches = Vec::new();
        for entry in self.documents.values() {
            let Some(current) = &entry.current else {
                continue;
            };
            let relative_path = entry
                .path
                .strip_prefix(&root)
                .unwrap_or(&entry.path)
                .display()
                .to_string();
            if kind.is_none_or(|kind| kind == SearchRecordKind::Note) {
                let (title, range) = note_search_title(current, &relative_path);
                let filter_match = match &filter {
                    Some(filter) => {
                        filter.note_matches(&root, &entry.path, &title, reverse.as_ref())?
                    }
                    None => true,
                };
                if filter_match {
                    if let Some(score) = search_score(query, &[&title, &relative_path]) {
                        matches.push((
                            score,
                            SearchRecord {
                                kind: SearchRecordKind::Note,
                                title,
                                path: entry.path.clone(),
                                relative_path: relative_path.clone(),
                                range,
                                revision: current.revision,
                                id: None,
                                task_state: None,
                                wait_reasons: None,
                                due: None,
                                priority: None,
                                effective_priority: None,
                                blocked: None,
                                actionable: None,
                                depth: None,
                                at: None,
                                start: None,
                                end: None,
                                tasks: None,
                            },
                        ));
                    }
                }
            }
            if kind.is_none_or(|kind| kind == SearchRecordKind::Task) {
                for task in &current.output.tasks.tasks {
                    let id = task.id.as_ref().map(|id| id.value.clone());
                    let fields = [
                        task.title.as_str(),
                        id.as_deref().unwrap_or_default(),
                        relative_path.as_str(),
                    ];
                    let Some(score) = search_score(query, &fields) else {
                        continue;
                    };
                    let blocked = self.is_task_blocked_value(&entry.path, task)?;
                    let (task_state, wait_reasons) = derive_task_workflow_state(task, blocked, now);
                    let actionable = task_state == TaskWorkflowState::Ready;
                    if let Some(filter) = &filter {
                        let facts = TaskMatchFacts {
                            state: task_state,
                            wait_reasons: &wait_reasons,
                            blocked,
                            actionable,
                        };
                        if !filter.task_matches(
                            &root,
                            &entry.path,
                            task,
                            self,
                            task_dependents.as_ref(),
                            facts,
                        )? {
                            continue;
                        }
                    }
                    matches.push((
                        score,
                        SearchRecord {
                            kind: SearchRecordKind::Task,
                            title: task.title.clone(),
                            path: entry.path.clone(),
                            relative_path: relative_path.clone(),
                            range: task.selection_range.clone(),
                            revision: current.revision,
                            id,
                            task_state: Some(task_state),
                            wait_reasons: Some(wait_reasons),
                            due: task.due.as_ref().map(|due| due.value.clone()),
                            priority: task.priority,
                            effective_priority: None,
                            blocked: Some(blocked),
                            actionable: Some(actionable),
                            depth: Some(task.depth),
                            at: None,
                            start: None,
                            end: None,
                            tasks: None,
                        },
                    ));
                }
            }
            if kind.is_none_or(|kind| kind == SearchRecordKind::Event) {
                for event in &current.output.events.events {
                    let id = event.id.as_ref().map(|id| id.value.clone());
                    let fields = [
                        event.title.as_str(),
                        id.as_deref().unwrap_or_default(),
                        relative_path.as_str(),
                    ];
                    let Some(score) = search_score(query, &fields) else {
                        continue;
                    };
                    if let Some(filter) = &filter {
                        if !filter.event_matches(&root, &entry.path, event)? {
                            continue;
                        }
                    }
                    matches.push((
                        score,
                        SearchRecord {
                            kind: SearchRecordKind::Event,
                            title: event.title.clone(),
                            path: entry.path.clone(),
                            relative_path: relative_path.clone(),
                            range: event.selection_range.clone(),
                            revision: current.revision,
                            id,
                            task_state: None,
                            wait_reasons: None,
                            due: None,
                            priority: None,
                            effective_priority: None,
                            blocked: None,
                            actionable: None,
                            depth: Some(event.depth),
                            at: event.at.as_ref().map(|field| field.value.clone()),
                            start: event.start.as_ref().map(|field| field.value.clone()),
                            end: event.end.as_ref().map(|field| field.value.clone()),
                            tasks: Some(
                                event
                                    .tasks
                                    .iter()
                                    .map(|reference| reference.source.clone())
                                    .collect(),
                            ),
                        },
                    ));
                }
            }
        }
        if let Some(store) = &self.disk_store {
            let open = self.open_paths();
            let open_set = open.iter().collect::<HashSet<_>>();
            if kind.is_none_or(|kind| kind == SearchRecordKind::Note) {
                for document in store.documents()? {
                    if open_set.contains(&document.path) || !document.valid {
                        continue;
                    }
                    let relative_path = document
                        .path
                        .strip_prefix(&root)
                        .unwrap_or(&document.path)
                        .display()
                        .to_string();
                    if let (Some(score), true) = (
                        search_score(query, &[&document.title, &relative_path]),
                        match &filter {
                            Some(filter) => filter.note_matches(
                                &root,
                                &document.path,
                                &document.title,
                                reverse.as_ref(),
                            )?,
                            None => true,
                        },
                    ) {
                        matches.push((
                            score,
                            SearchRecord {
                                kind: SearchRecordKind::Note,
                                title: document.title,
                                path: document.path,
                                relative_path,
                                range: document.title_range,
                                revision: document.revision,
                                id: None,
                                task_state: None,
                                wait_reasons: None,
                                due: None,
                                priority: None,
                                effective_priority: None,
                                blocked: None,
                                actionable: None,
                                depth: None,
                                at: None,
                                start: None,
                                end: None,
                                tasks: None,
                            },
                        ));
                    }
                }
            }
            if kind.is_none_or(|kind| kind == SearchRecordKind::Task) {
                let blocked_sources = store
                    .blocked_task_sources(&open)?
                    .into_iter()
                    .map(|source| (source.path, source.start))
                    .collect::<HashSet<_>>();
                for stored in store.tasks(&open)? {
                    let task = stored.record;
                    let relative_path = stored
                        .path
                        .strip_prefix(&root)
                        .unwrap_or(&stored.path)
                        .display()
                        .to_string();
                    let id = task.id.as_ref().map(|id| id.value.clone());
                    let Some(score) = search_score(
                        query,
                        &[
                            task.title.as_str(),
                            id.as_deref().unwrap_or_default(),
                            &relative_path,
                        ],
                    ) else {
                        continue;
                    };
                    let blocked = if open.is_empty() {
                        blocked_sources.contains(&(stored.path.clone(), task.range.start))
                    } else {
                        self.is_task_blocked_value(&stored.path, &task)?
                    };
                    let (task_state, wait_reasons) =
                        derive_task_workflow_state(&task, blocked, now);
                    let actionable = task_state == TaskWorkflowState::Ready;
                    if let Some(filter) = &filter {
                        let facts = TaskMatchFacts {
                            state: task_state,
                            wait_reasons: &wait_reasons,
                            blocked,
                            actionable,
                        };
                        if !filter.task_matches(
                            &root,
                            &stored.path,
                            &task,
                            self,
                            task_dependents.as_ref(),
                            facts,
                        )? {
                            continue;
                        }
                    }
                    matches.push((
                        score,
                        SearchRecord {
                            kind: SearchRecordKind::Task,
                            title: task.title,
                            path: stored.path,
                            relative_path,
                            range: task.selection_range,
                            revision: stored.revision,
                            id,
                            task_state: Some(task_state),
                            wait_reasons: Some(wait_reasons),
                            due: task.due.map(|due| due.value),
                            priority: task.priority,
                            effective_priority: None,
                            blocked: Some(blocked),
                            actionable: Some(actionable),
                            depth: Some(task.depth),
                            at: None,
                            start: None,
                            end: None,
                            tasks: None,
                        },
                    ));
                }
            }
            if kind.is_none_or(|kind| kind == SearchRecordKind::Event) {
                for stored in store.events(&open)? {
                    let event = stored.record;
                    let relative_path = stored
                        .path
                        .strip_prefix(&root)
                        .unwrap_or(&stored.path)
                        .display()
                        .to_string();
                    let id = event.id.as_ref().map(|id| id.value.clone());
                    let Some(score) = search_score(
                        query,
                        &[
                            event.title.as_str(),
                            id.as_deref().unwrap_or_default(),
                            &relative_path,
                        ],
                    ) else {
                        continue;
                    };
                    if let Some(filter) = &filter {
                        if !filter.event_matches(&root, &stored.path, &event)? {
                            continue;
                        }
                    }
                    matches.push((
                        score,
                        SearchRecord {
                            kind: SearchRecordKind::Event,
                            title: event.title,
                            path: stored.path,
                            relative_path,
                            range: event.selection_range,
                            revision: stored.revision,
                            id,
                            task_state: None,
                            wait_reasons: None,
                            due: None,
                            priority: None,
                            effective_priority: None,
                            blocked: None,
                            actionable: None,
                            depth: Some(event.depth),
                            at: event.at.map(|field| field.value),
                            start: event.start.map(|field| field.value),
                            end: event.end.map(|field| field.value),
                            tasks: Some(
                                event
                                    .tasks
                                    .into_iter()
                                    .map(|reference| reference.source)
                                    .collect(),
                            ),
                        },
                    ));
                }
            }
        }
        self.propagate_task_priorities(&mut matches)?;
        matches.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| left.relative_path.cmp(&right.relative_path))
                .then_with(|| left.range.start.cmp(&right.range.start))
                .then_with(|| search_kind_order(left.kind).cmp(&search_kind_order(right.kind)))
        });
        let complete = matches.len() <= limit;
        matches.truncate(limit);
        Ok(self.query_result(SearchResults {
            items: matches.into_iter().map(|(_, record)| record).collect(),
            complete,
        }))
    }

    fn propagate_task_priorities(
        &self,
        matches: &mut [(i64, SearchRecord)],
    ) -> Result<(), super::WorkspaceQueryError> {
        let mut task_indexes = matches
            .iter()
            .enumerate()
            .filter(|(_, (_, record))| record.kind == SearchRecordKind::Task)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        task_indexes.sort_by_key(|index| {
            let record = &matches[*index].1;
            (record.path.clone(), record.range.start)
        });
        let node_by_record = task_indexes
            .iter()
            .enumerate()
            .map(|(node, record)| (*record, node))
            .collect::<HashMap<_, _>>();
        let mut node_by_ref = HashMap::new();
        for (node, record_index) in task_indexes.iter().copied().enumerate() {
            let record = &matches[record_index].1;
            if let Some(id) = &record.id {
                node_by_ref.insert(
                    TaskRef {
                        path: record.path.clone(),
                        id: id.clone(),
                    },
                    node,
                );
            }
        }

        let mut edges = Vec::new();
        let mut ancestors = Vec::<(usize, usize)>::new();
        let mut previous_path = None;
        for record_index in task_indexes.iter().copied() {
            let record = &matches[record_index].1;
            if previous_path.as_ref() != Some(&record.path) {
                ancestors.clear();
                previous_path = Some(record.path.clone());
            }
            let depth = record.depth.unwrap_or_default();
            while ancestors
                .last()
                .is_some_and(|(ancestor_depth, _)| *ancestor_depth >= depth)
            {
                ancestors.pop();
            }
            let node = node_by_record[&record_index];
            if let Some((_, parent)) = ancestors.last() {
                edges.push((node, *parent));
            }
            ancestors.push((depth, node));

            let Some(task) = self.get(&record.path).and_then(|entry| {
                entry
                    .current
                    .as_ref()?
                    .output
                    .tasks
                    .tasks
                    .iter()
                    .find(|task| task.selection_range == record.range)
            }) else {
                continue;
            };
            for dependency in self
                .task_dependencies_value(&record.path, task)?
                .into_iter()
                .filter(|dependency| dependency.task.state() == TaskState::Open)
            {
                if let Some(target) = node_by_ref.get(&dependency.target) {
                    edges.push((node, *target));
                }
            }
        }

        let mut priorities = task_indexes
            .iter()
            .map(|index| matches[*index].1.priority.unwrap_or_default())
            .collect::<Vec<_>>();
        loop {
            let mut changed = false;
            for &(source, target) in &edges {
                if priorities[source] > priorities[target] {
                    priorities[target] = priorities[source];
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        for (node, record_index) in task_indexes.into_iter().enumerate() {
            matches[record_index].1.effective_priority = Some(priorities[node]);
        }
        Ok(())
    }
}

pub fn search_score(query: &str, fields: &[&str]) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    fields
        .iter()
        .filter_map(|field| fuzzy_score(field, query))
        .max()
}

fn fuzzy_score(candidate: &str, query: &str) -> Option<i64> {
    let candidate = candidate.to_lowercase().chars().collect::<Vec<_>>();
    let query = query.to_lowercase().chars().collect::<Vec<_>>();
    if query.is_empty() {
        return Some(0);
    }
    let mut position = 0;
    let mut previous = None;
    let mut score = 0i64;
    for wanted in &query {
        let relative = candidate[position..]
            .iter()
            .position(|character| character == wanted)?;
        let found = position + relative;
        score += 20 - i64::try_from(relative.min(20)).unwrap_or(20);
        if previous.is_some_and(|previous| previous + 1 == found) {
            score += 15;
        }
        if found == 0
            || candidate
                .get(found.wrapping_sub(1))
                .is_some_and(|character| {
                    character.is_whitespace() || matches!(character, '/' | '-' | '_')
                })
        {
            score += 10;
        }
        previous = Some(found);
        position = found + 1;
    }
    if candidate == query {
        score += 1000;
    } else if candidate.starts_with(&query) {
        score += 500;
    }
    Some(score)
}

fn note_search_title(
    current: &VersionedDocumentOutput,
    relative_path: &str,
) -> (String, std::ops::Range<usize>) {
    let title = current
        .output
        .metadata
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.entries.iter().find(|entry| entry.key == "title"))
        .and_then(|entry| match &entry.value {
            MetadataValue::Scalar { content, .. } if !content.plain_text().is_empty() => {
                Some((content.plain_text(), content.range.clone()))
            }
            _ => None,
        });
    title.unwrap_or_else(|| {
        let fallback = Path::new(relative_path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.is_empty())
            .unwrap_or(relative_path)
            .to_string();
        (fallback, 0..0)
    })
}

struct SemanticSearchFilter {
    program: Program,
    variables: HashSet<String>,
    now: DateTime<FixedOffset>,
}

struct TaskMatchFacts<'a> {
    state: TaskWorkflowState,
    wait_reasons: &'a [TaskWaitReason],
    blocked: bool,
    actionable: bool,
}

impl SemanticSearchFilter {
    fn compile(source: &str, now: DateTime<FixedOffset>) -> Result<Self, String> {
        let program =
            Program::compile(source).map_err(|error| format!("invalid CEL query: {error}"))?;
        let variables = program
            .references()
            .variables()
            .into_iter()
            .map(str::to_string)
            .collect();
        Ok(Self {
            program,
            variables,
            now,
        })
    }

    fn uses(&self, variable: &str) -> bool {
        self.variables.contains(variable)
    }

    fn needs_reverse_references(&self) -> bool {
        self.uses("directly_referenced_by") || self.uses("transitively_referenced_by")
    }

    fn needs_task_dependents(&self) -> bool {
        self.uses("directly_blocking")
    }

    fn note_matches(
        &self,
        root: &Path,
        path: &Path,
        title: &str,
        reverse: Option<&ReverseReferences>,
    ) -> Result<bool, WorkspaceSearchError> {
        let mut context = Context::default();
        context.add_variable_from_value("path", display_workspace_path(root, path));
        context.add_variable_from_value("title", title.to_string());
        if self.uses("directly_referenced_by") {
            let reverse = reverse.expect("reverse references requested by CEL filter");
            context.add_variable_from_value(
                "directly_referenced_by",
                reverse
                    .direct(path)
                    .iter()
                    .map(|path| display_workspace_path(root, path))
                    .collect::<Vec<_>>(),
            );
        }
        if self.uses("transitively_referenced_by") {
            let reverse = reverse.expect("reverse references requested by CEL filter");
            context.add_variable_from_value(
                "transitively_referenced_by",
                reverse
                    .transitive(path)
                    .iter()
                    .map(|path| display_workspace_path(root, path))
                    .collect::<Vec<_>>(),
            );
        }
        execute_search_filter(&self.program, &context, path).map_err(Into::into)
    }

    fn task_matches(
        &self,
        root: &Path,
        path: &Path,
        task: &TaskRecord,
        workspace: &Workspace,
        task_dependents: Option<&DirectTaskDependents>,
        facts: TaskMatchFacts<'_>,
    ) -> Result<bool, WorkspaceSearchError> {
        let mut context = Context::default();
        context.add_variable_from_value("path", display_workspace_path(root, path));
        context.add_variable_from_value(
            "id",
            optional_search_string(task.id.as_ref().map(|id| &id.value)),
        );
        context.add_variable_from_value("title", task.title.clone());
        context.add_variable_from_value("created", search_datetime_value(task.created.as_ref()));
        context.add_variable_from_value("due", search_datetime_value(task.due.as_ref()));
        context.add_variable_from_value(
            "priority",
            task.priority
                .map_or(Value::Null, |value| Value::Int(i64::from(value))),
        );
        context.add_variable_from_value("wait", search_datetime_value(task.wait.as_ref()));
        context.add_variable_from_value("done", search_datetime_value(task.done.as_ref()));
        context.add_variable_from_value("canceled", search_datetime_value(task.canceled.as_ref()));
        context.add_variable_from_value(
            "recur",
            optional_search_string(task.recur.as_ref().map(|field| &field.value)),
        );
        context.add_variable_from_value(
            "prev",
            optional_search_string(task.prev.as_ref().map(|field| &field.value)),
        );
        if self.uses("depends_on") {
            context.add_variable_from_value(
                "depends_on",
                workspace
                    .task_dependencies_value(path, task)?
                    .into_iter()
                    .map(|dependency| display_search_task_ref(root, &dependency.target))
                    .collect::<Vec<_>>(),
            );
        }
        if self.uses("directly_blocking") {
            let task_dependents = task_dependents.expect("task dependents requested by CEL filter");
            context.add_variable_from_value(
                "directly_blocking",
                task.id
                    .as_ref()
                    .map(|id| {
                        task_dependents
                            .get(path, &id.value)
                            .iter()
                            .map(|target| display_search_task_ref(root, target))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            );
        }
        context.add_variable_from_value("state", facts.state.as_str());
        context.add_variable_from_value(
            "wait_reasons",
            facts
                .wait_reasons
                .iter()
                .map(|reason| reason.as_str())
                .collect::<Vec<_>>(),
        );
        context.add_variable_from_value("blocked", facts.blocked);
        context.add_variable_from_value("actionable", facts.actionable);
        context.add_variable_from_value("now", Value::Timestamp(self.now));
        execute_search_filter(&self.program, &context, path).map_err(Into::into)
    }

    fn event_matches(&self, root: &Path, path: &Path, event: &EventRecord) -> Result<bool, String> {
        let mut context = Context::default();
        context.add_variable_from_value("path", display_workspace_path(root, path));
        context.add_variable_from_value(
            "id",
            optional_search_string(event.id.as_ref().map(|id| &id.value)),
        );
        context.add_variable_from_value("title", event.title.clone());
        context.add_variable_from_value(
            "uid",
            optional_search_string(event.uid.as_ref().map(|uid| &uid.value)),
        );
        context.add_variable_from_value(
            "date",
            optional_search_string(event.date.as_ref().map(|field| &field.value)),
        );
        context.add_variable_from_value(
            "timezone",
            optional_search_string(event.timezone.as_ref().map(|field| &field.value)),
        );
        context.add_variable_from_value(
            "when",
            optional_search_string(event.when.as_ref().map(|field| &field.value)),
        );
        context.add_variable_from_value("at", event_search_datetime_value(&event.at));
        context.add_variable_from_value("start", event_search_datetime_value(&event.start));
        context.add_variable_from_value("end", event_search_datetime_value(&event.end));
        context.add_variable_from_value(
            "tasks",
            event
                .tasks
                .iter()
                .map(|reference| reference.source.clone())
                .collect::<Vec<_>>(),
        );
        context.add_variable_from_value("now", Value::Timestamp(self.now));
        execute_search_filter(&self.program, &context, path)
    }
}

pub(crate) fn derive_task_workflow_state(
    task: &TaskRecord,
    blocked: bool,
    now: DateTime<FixedOffset>,
) -> (TaskWorkflowState, Vec<TaskWaitReason>) {
    match task.state() {
        TaskState::Done => (TaskWorkflowState::Done, Vec::new()),
        TaskState::Canceled => (TaskWorkflowState::Canceled, Vec::new()),
        TaskState::Conflicted => (TaskWorkflowState::Conflicted, Vec::new()),
        TaskState::Open => {
            let mut reasons = Vec::new();
            let waiting = task
                .wait
                .as_ref()
                .and_then(|wait| DateTime::parse_from_rfc3339(&wait.value).ok())
                .is_some_and(|wait| wait > now);
            if waiting {
                reasons.push(TaskWaitReason::Time);
            }
            if blocked {
                reasons.push(TaskWaitReason::Dependency);
            }
            let state = if waiting {
                TaskWorkflowState::Waiting
            } else if blocked {
                TaskWorkflowState::Blocked
            } else {
                TaskWorkflowState::Ready
            };
            (state, reasons)
        }
    }
}

fn execute_search_filter(
    program: &Program,
    context: &Context,
    path: &Path,
) -> Result<bool, String> {
    match program.execute(context) {
        Ok(Value::Bool(value)) => Ok(value),
        Ok(value) => Err(format!("CEL query must return bool, got {value:?}")),
        Err(ExecutionError::NoSuchKey(_)) => Ok(false),
        Err(error) => Err(format!(
            "cannot evaluate query for {}: {error}",
            path.display()
        )),
    }
}

fn optional_search_string(value: Option<&String>) -> Value {
    value
        .cloned()
        .map_or(Value::Null, |value| Value::String(value.into()))
}

fn search_datetime_value(field: Option<&plumb_semantics::TaskField>) -> Value {
    field
        .and_then(|field| DateTime::parse_from_rfc3339(&field.value).ok())
        .map_or(Value::Null, Value::Timestamp)
}

fn event_search_datetime_value(field: &Option<plumb_semantics::EventField>) -> Value {
    field
        .as_ref()
        .and_then(|field| DateTime::parse_from_rfc3339(&field.value).ok())
        .map_or(Value::Null, Value::Timestamp)
}

fn display_search_task_ref(root: &Path, task_ref: &TaskRef) -> String {
    format!(
        "{}#{}",
        display_workspace_path(root, &task_ref.path),
        task_ref.id
    )
}

#[derive(Debug, Default)]
struct DirectTaskDependents {
    by_target: HashMap<TaskRef, Vec<TaskRef>>,
}

impl DirectTaskDependents {
    fn build(workspace: &Workspace) -> Result<Self, super::WorkspaceQueryError> {
        let open = workspace.open_paths();
        let mut by_target = HashMap::<TaskRef, Vec<TaskRef>>::new();
        if let Some(store) = &workspace.disk_store {
            for relation in store.task_dependency_relations(&open)? {
                let Some(source_id) = relation.source_id else {
                    continue;
                };
                by_target
                    .entry(TaskRef {
                        path: relation.target_path,
                        id: relation.target_id,
                    })
                    .or_default()
                    .push(TaskRef {
                        path: relation.source_path,
                        id: source_id,
                    });
            }
        }
        for entry in workspace.documents.values() {
            let Some(current) = &entry.current else {
                continue;
            };
            for task in &current.output.tasks.tasks {
                let Some(id) = &task.id else {
                    continue;
                };
                let source = TaskRef {
                    path: entry.path.clone(),
                    id: id.value.clone(),
                };
                for dependency in workspace.task_dependencies_value(&entry.path, task)? {
                    by_target
                        .entry(dependency.target)
                        .or_default()
                        .push(source.clone());
                }
            }
        }
        for sources in by_target.values_mut() {
            sources.sort_by(|left, right| left.path.cmp(&right.path).then(left.id.cmp(&right.id)));
            sources.dedup();
        }
        Ok(Self { by_target })
    }

    fn get(&self, path: &Path, id: &str) -> &[TaskRef] {
        self.by_target
            .get(&TaskRef {
                path: normalize(path),
                id: id.to_string(),
            })
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

struct ReverseReferences {
    direct: HashMap<PathBuf, HashSet<PathBuf>>,
}

impl ReverseReferences {
    fn build(workspace: &Workspace) -> Result<Self, super::WorkspaceQueryError> {
        let mut direct: HashMap<PathBuf, HashSet<PathBuf>> = HashMap::new();
        for entry in workspace.documents() {
            for target in workspace.referenced_documents_from(&entry.path)?.value {
                direct.entry(target).or_default().insert(entry.path.clone());
            }
        }
        Ok(Self { direct })
    }

    fn direct(&self, path: &Path) -> Vec<PathBuf> {
        sorted_search_paths(self.direct.get(path).into_iter().flatten().cloned())
    }

    fn transitive(&self, path: &Path) -> Vec<PathBuf> {
        let mut found = HashSet::new();
        let mut queue = VecDeque::from(self.direct(path));
        while let Some(source) = queue.pop_front() {
            if source == path || !found.insert(source.clone()) {
                continue;
            }
            queue.extend(self.direct(&source));
        }
        sorted_search_paths(found)
    }
}

fn sorted_search_paths(values: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut values = values.into_iter().collect::<Vec<_>>();
    values.sort();
    values
}

fn search_kind_order(kind: SearchRecordKind) -> u8 {
    match kind {
        SearchRecordKind::Note => 0,
        SearchRecordKind::Task => 1,
        SearchRecordKind::Event => 2,
    }
}
