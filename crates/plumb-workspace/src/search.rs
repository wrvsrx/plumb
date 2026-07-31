use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use cel::{Context, ExecutionError, Program, Value};
use chrono::{DateTime, FixedOffset};
use plumb_extensions::{EventRecord, MetadataValue, TaskRecord, TaskState};

use crate::{
    display_workspace_path, normalize, DocumentEntry, TaskRef, VersionedDocumentOutput, Workspace,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchRecordKind {
    Note,
    Task,
    Event,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskWorkflowState {
    Ready,
    Waiting,
    Done,
    Canceled,
    Invalid,
}

impl TaskWorkflowState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Waiting => "waiting",
            Self::Done => "done",
            Self::Canceled => "canceled",
            Self::Invalid => "invalid",
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

impl Workspace {
    pub fn search_records(
        &self,
        root: impl AsRef<std::path::Path>,
        kind: Option<SearchRecordKind>,
        query: &str,
        limit: usize,
        now: DateTime<FixedOffset>,
    ) -> SearchResults {
        self.search_records_filtered(root, kind, query, limit, now, None)
            .expect("search without a semantic filter cannot fail")
    }

    pub fn search_records_filtered(
        &self,
        root: impl AsRef<std::path::Path>,
        kind: Option<SearchRecordKind>,
        query: &str,
        limit: usize,
        now: DateTime<FixedOffset>,
        filter: Option<&str>,
    ) -> Result<SearchResults, String> {
        let root = normalize(root.as_ref());
        let filter = filter
            .map(|source| SemanticSearchFilter::compile(source, now))
            .transpose()?;
        let reverse = filter.as_ref().map(|_| ReverseReferences::build(self));
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
                let filter_match = match (&filter, &reverse) {
                    (Some(filter), Some(reverse)) => {
                        filter.note_matches(&root, entry, &title, reverse)?
                    }
                    _ => true,
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
                    let blocked = self.is_task_blocked(&entry.path, task);
                    let (task_state, wait_reasons) = derive_task_workflow_state(task, blocked, now);
                    let actionable = task_state == TaskWorkflowState::Ready;
                    if let Some(filter) = &filter {
                        let facts = TaskMatchFacts {
                            state: task_state,
                            wait_reasons: &wait_reasons,
                            blocked,
                            actionable,
                        };
                        if !filter.task_matches(&root, entry, task, self, facts)? {
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
                        event
                            .uid
                            .as_ref()
                            .map(|uid| uid.value.as_str())
                            .unwrap_or_default(),
                        relative_path.as_str(),
                    ];
                    let Some(score) = search_score(query, &fields) else {
                        continue;
                    };
                    if let Some(filter) = &filter {
                        if !filter.event_matches(&root, entry, event)? {
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
        matches.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| left.relative_path.cmp(&right.relative_path))
                .then_with(|| left.range.start.cmp(&right.range.start))
                .then_with(|| search_kind_order(left.kind).cmp(&search_kind_order(right.kind)))
        });
        let complete = matches.len() <= limit;
        matches.truncate(limit);
        Ok(SearchResults {
            items: matches.into_iter().map(|(_, record)| record).collect(),
            complete,
        })
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
        Ok(Self {
            program: Program::compile(source)
                .map_err(|error| format!("invalid CEL query: {error}"))?,
            now,
        })
    }

    fn note_matches(
        &self,
        root: &Path,
        entry: &DocumentEntry,
        title: &str,
        reverse: &ReverseReferences,
    ) -> Result<bool, String> {
        let mut context = Context::default();
        context.add_variable_from_value("path", display_workspace_path(root, &entry.path));
        context.add_variable_from_value("title", title.to_string());
        context.add_variable_from_value(
            "directly_referenced_by",
            reverse
                .direct(&entry.path)
                .iter()
                .map(|path| display_workspace_path(root, path))
                .collect::<Vec<_>>(),
        );
        context.add_variable_from_value(
            "transitively_referenced_by",
            reverse
                .transitive(&entry.path)
                .iter()
                .map(|path| display_workspace_path(root, path))
                .collect::<Vec<_>>(),
        );
        execute_search_filter(&self.program, &context, &entry.path)
    }

    fn task_matches(
        &self,
        root: &Path,
        entry: &DocumentEntry,
        task: &TaskRecord,
        workspace: &Workspace,
        facts: TaskMatchFacts<'_>,
    ) -> Result<bool, String> {
        let depends_on = workspace
            .task_dependencies(&entry.path, task)
            .into_iter()
            .map(|dependency| display_search_task_ref(root, &dependency.target))
            .collect::<Vec<_>>();
        let directly_blocking = task
            .id
            .as_ref()
            .map(|id| {
                workspace
                    .directly_blocking_tasks(&entry.path, &id.value)
                    .iter()
                    .map(|target| display_search_task_ref(root, target))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut context = Context::default();
        context.add_variable_from_value("path", display_workspace_path(root, &entry.path));
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
        context.add_variable_from_value("depends_on", depends_on);
        context.add_variable_from_value("directly_blocking", directly_blocking);
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
        execute_search_filter(&self.program, &context, &entry.path)
    }

    fn event_matches(
        &self,
        root: &Path,
        entry: &DocumentEntry,
        event: &EventRecord,
    ) -> Result<bool, String> {
        let mut context = Context::default();
        context.add_variable_from_value("path", display_workspace_path(root, &entry.path));
        context.add_variable_from_value(
            "id",
            optional_search_string(event.id.as_ref().map(|id| &id.value)),
        );
        context.add_variable_from_value(
            "uid",
            optional_search_string(event.uid.as_ref().map(|uid| &uid.value)),
        );
        context.add_variable_from_value("title", event.title.clone());
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
        execute_search_filter(&self.program, &context, &entry.path)
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
        TaskState::Conflicted => (TaskWorkflowState::Invalid, Vec::new()),
        TaskState::Open => {
            let mut reasons = Vec::new();
            if task
                .wait
                .as_ref()
                .and_then(|wait| DateTime::parse_from_rfc3339(&wait.value).ok())
                .is_some_and(|wait| wait > now)
            {
                reasons.push(TaskWaitReason::Time);
            }
            if blocked {
                reasons.push(TaskWaitReason::Dependency);
            }
            let state = if reasons.is_empty() {
                TaskWorkflowState::Ready
            } else {
                TaskWorkflowState::Waiting
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

fn search_datetime_value(field: Option<&plumb_extensions::TaskField>) -> Value {
    field
        .and_then(|field| DateTime::parse_from_rfc3339(&field.value).ok())
        .map_or(Value::Null, Value::Timestamp)
}

fn event_search_datetime_value(field: &Option<plumb_extensions::EventField>) -> Value {
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

struct ReverseReferences {
    direct: HashMap<PathBuf, HashSet<PathBuf>>,
}

impl ReverseReferences {
    fn build(workspace: &Workspace) -> Self {
        let mut direct: HashMap<PathBuf, HashSet<PathBuf>> = HashMap::new();
        for entry in workspace.documents() {
            for target in workspace.referenced_documents_from(&entry.path) {
                direct.entry(target).or_default().insert(entry.path.clone());
            }
        }
        Self { direct }
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
