use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use cel::{Context, ExecutionError, Program, Value};
use chrono::{DateTime, FixedOffset};
use plumb_semantics::{TaskOwner, TaskRecord, TaskReferenceTarget, TaskState};
use sha2::{Digest, Sha256};

use super::{
    display_workspace_path, normalize, resolve_relative, sort_task_records_by,
    truncate_complete_task_documents, QueryResult, TaskRef, TaskSortFacts, TaskSortOrder,
    TaskWaitReason, TaskWorkflowState, Workspace, WorkspaceQueryError,
};
use crate::store::{StoredTaskDependency, StoredTaskFact, StoredTaskIdentity, StoredTaskKey};
use crate::task_predicate::{task_candidate_prefix, TaskCandidatePredicate};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskQueryFilter {
    pub source: String,
    pub expression: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskQueryFilterGroup {
    pub filters: Vec<TaskQueryFilter>,
}

#[derive(Debug, Clone)]
pub struct TaskPageQuery {
    pub root: PathBuf,
    pub text: String,
    pub filter_groups: Vec<TaskQueryFilterGroup>,
    pub sort: Vec<TaskSortOrder>,
    pub limit: usize,
    pub cursor: Option<String>,
    pub workspace_revision: u64,
    pub now: DateTime<FixedOffset>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceTask {
    pub matched: bool,
    pub path: PathBuf,
    pub revision: i64,
    pub task: TaskRecord,
    pub state: TaskWorkflowState,
    pub wait_reasons: Vec<TaskWaitReason>,
    pub blocked: bool,
    pub actionable: bool,
    pub effective_priority: i32,
    pub relevance: i64,
    pub depends_on: Vec<TaskRef>,
    pub directly_blocking: Vec<TaskRef>,
    pub previous: Option<TaskRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskPage {
    pub tasks: Vec<WorkspaceTask>,
    pub complete: bool,
    pub next_cursor: Option<String>,
}

/// Bounded page size for the in-flight section. The candidate limit never
/// truncates this section; callers page it through `focused_next_cursor`.
pub const NEXT_FOCUSED_PAGE: usize = 50;

/// Shared shortlist query: the tasks currently in flight plus the tasks to
/// start next. Defaults live at the adapter boundary; the shared layer clamps
/// the candidate limit to 1..=10.
#[derive(Debug, Clone)]
pub struct NextQuery {
    pub root: PathBuf,
    pub limit: usize,
    pub cursor: Option<String>,
    pub workspace_revision: u64,
    pub now: DateTime<FixedOffset>,
}

/// A task skipped because its focus history is invalid, so it can never be
/// mistaken for "not focused".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextSkippedFocus {
    pub path: PathBuf,
    pub id: Option<String>,
    pub title: String,
    pub range: std::ops::Range<usize>,
    pub codes: Vec<plumb_semantics::FocusProblemCode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextResult {
    pub focused: Vec<WorkspaceTask>,
    /// Total currently focused tasks, independent of the focused page size.
    pub focused_total: usize,
    /// False when `focused` is only the first page of `focused_total`.
    pub focused_complete: bool,
    pub focused_next_cursor: Option<String>,
    pub candidates: Vec<WorkspaceTask>,
    pub candidate_limit: usize,
    /// False when more ready candidates exist than the requested limit.
    pub candidates_complete: bool,
    pub skipped_invalid: Vec<NextSkippedFocus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDocumentMetrics {
    pub path: PathBuf,
    pub tasks: usize,
    pub open_tasks: usize,
}

#[derive(Debug)]
pub enum TaskPageQueryError {
    Filter { source: String, message: String },
    Cursor(String),
    Query(WorkspaceQueryError),
}

impl std::fmt::Display for TaskPageQueryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Filter { message, .. } | Self::Cursor(message) => formatter.write_str(message),
            Self::Query(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for TaskPageQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Query(error) => Some(error),
            Self::Filter { .. } | Self::Cursor(_) => None,
        }
    }
}

impl From<WorkspaceQueryError> for TaskPageQueryError {
    fn from(error: WorkspaceQueryError) -> Self {
        Self::Query(error)
    }
}

impl From<super::StoreError> for TaskPageQueryError {
    fn from(error: super::StoreError) -> Self {
        Self::Query(WorkspaceQueryError::Store(error))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct TaskKey {
    path: PathBuf,
    start: usize,
}

#[derive(Clone)]
struct TaskFact {
    document_task: bool,
    key: TaskKey,
    document_order: String,
    revision: i64,
    id: Option<String>,
    title: String,
    closure_state: TaskState,
    created_millis: Option<i64>,
    due_millis: Option<i64>,
    wait_millis: Option<i64>,
    done_millis: Option<i64>,
    canceled_millis: Option<i64>,
    priority: Option<i32>,
    depth: usize,
    parent_start: Option<usize>,
    recur: Option<String>,
    prev: Option<String>,
    /// Derived focus facts. `focused` mirrors `TaskRecord::is_focused()`,
    /// `focused_since_millis` is the current open interval start, and
    /// `focus_valid` keeps invalid focus history observable instead of
    /// collapsing it into `focused == false`.
    focused: bool,
    focused_since_millis: Option<i64>,
    /// Read by the protocol-neutral `next` shortlist to skip invalid-focus
    /// tasks instead of treating them as `focused == false` candidates.
    focus_valid: bool,
    state: TaskWorkflowState,
    wait_reasons: Vec<TaskWaitReason>,
    blocked: bool,
    actionable: bool,
    relevance: i64,
    effective_priority: i32,
}

#[derive(Clone)]
struct TaskRelation {
    source: TaskKey,
    target: TaskRef,
}

struct CompiledFilter {
    source: String,
    program: Program,
    variables: HashSet<String>,
}

impl Workspace {
    pub fn task_document_metrics(
        &self,
    ) -> Result<QueryResult<Vec<TaskDocumentMetrics>>, TaskPageQueryError> {
        let (facts, _) = task_facts(self, None, 0)?;
        let mut metrics = BTreeMap::<PathBuf, (usize, usize)>::new();
        for fact in facts {
            let counts = metrics.entry(fact.key.path).or_default();
            counts.0 += 1;
            counts.1 += usize::from(fact.closure_state == TaskState::Open);
        }
        Ok(self.query_result(
            metrics
                .into_iter()
                .map(|(path, (tasks, open_tasks))| TaskDocumentMetrics {
                    path,
                    tasks,
                    open_tasks,
                })
                .collect(),
        ))
    }

    pub fn query_task_page(
        &self,
        query: &TaskPageQuery,
    ) -> Result<QueryResult<TaskPage>, TaskPageQueryError> {
        self.query_task_page_impl(query, false)
    }

    pub fn query_task_tree_page(
        &self,
        query: &TaskPageQuery,
    ) -> Result<QueryResult<TaskPage>, TaskPageQueryError> {
        self.query_task_page_impl(query, true)
    }

    fn query_task_page_impl(
        &self,
        query: &TaskPageQuery,
        include_ancestors: bool,
    ) -> Result<QueryResult<TaskPage>, TaskPageQueryError> {
        let filters = compile_filters(&query.filter_groups)?;
        let candidate = if include_ancestors { None } else { task_candidate_predicate(&filters) };
        let (mut facts, open_records) =
            task_facts(self, candidate.as_ref(), query.now.timestamp_millis())?;
        let (identities, task_refs_by_key, states) =
            task_identity_context(self, &facts, candidate.is_some())?;
        let relations = task_relations(self, &open_records, &identities, &states)?;
        let dependencies = dependencies_by_source(&relations);
        let dependents = dependents_by_target(&relations, &task_refs_by_key);

        for fact in &mut facts {
            let blocked = dependencies.get(&fact.key).is_some_and(|targets| {
                targets.iter().any(|target| {
                    identities
                        .get(target)
                        .and_then(|key| states.get(key))
                        .is_some_and(|state| *state == TaskState::Open)
                })
            });
            let (state, wait_reasons) = workflow_state(fact, blocked, query.now);
            fact.state = state;
            fact.wait_reasons = wait_reasons;
            fact.blocked = blocked;
            fact.actionable = state == TaskWorkflowState::Ready;
        }

        let ancestor_facts = if include_ancestors {
            facts.iter().map(|fact| (fact.key.clone(), fact.clone())).collect::<HashMap<_, _>>()
        } else { HashMap::new() };
        let mut retained = Vec::with_capacity(facts.len());
        for mut fact in facts {
            let relative_path = display_workspace_path(&query.root, &fact.key.path);
            let Some(score) = super::search_score(
                &query.text,
                &[
                    &fact.title,
                    fact.id.as_deref().unwrap_or_default(),
                    &relative_path,
                ],
            ) else {
                continue;
            };
            fact.relevance = score;
            let targets = dependencies
                .get(&fact.key)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let blocking = TaskRef::from_parts(&fact.key.path, fact.document_task, fact.id.clone())
                .and_then(|target| dependents.get(&target))
                .map(Vec::as_slice)
                .unwrap_or_default();
            if filter_groups_match(&filters, &fact, &query.root, query.now, targets, blocking)? {
                retained.push(fact);
            }
        }

        let matched_keys = retained.iter().map(|fact| fact.key.clone()).collect::<HashSet<_>>();
        if include_ancestors {
            let mut included = matched_keys.clone();
            let mut index = 0;
            while index < retained.len() {
                let fact = &retained[index];
                if let Some(start) = fact.parent_start {
                    let key = TaskKey { path: fact.key.path.clone(), start };
                    if included.insert(key.clone()) {
                        if let Some(parent) = ancestor_facts.get(&key) { retained.push(parent.clone()); }
                    }
                }
                index += 1;
            }
        }
        propagate_effective_priorities(&mut retained, &relations, &identities, &states);
        sort_task_records_by(&mut retained, &query.sort, |fact| TaskSortFacts {
            document: fact.document_order.clone(),
            source_start: if fact.document_task {
                0
            } else {
                fact.key.start
            },
            depth: fact.depth,
            focused: fact.focused,
            priority: Some(fact.effective_priority),
            due: fact.due_millis.and_then(datetime_from_millis),
            relevance: Some(fact.relevance),
        });
        apply_cursor(&mut retained, query, include_ancestors)?;
        let remaining_count = retained.len();
        truncate_complete_task_documents(&mut retained, query.limit, |fact| &fact.document_order);
        let complete = retained.len() == remaining_count;
        let next_cursor = (!complete)
            .then(|| {
                retained
                    .last()
                    .map(|fact| encode_cursor(query, &fact.key.path, include_ancestors))
            })
            .flatten();

        let page_keys = retained
            .iter()
            .map(|fact| fact.key.clone())
            .collect::<HashSet<_>>();
        let stored_keys = retained
            .iter()
            .filter(|fact| !open_records.contains_key(&fact.key))
            .map(|fact| StoredTaskKey {
                path: fact.key.path.clone(),
                start: fact.key.start,
            })
            .collect::<Vec<_>>();
        let mut records = open_records
            .into_iter()
            .filter(|(key, _)| page_keys.contains(key))
            .collect::<HashMap<_, _>>();
        if let Some(store) = &self.disk_store {
            for stored in store.tasks_by_keys(&stored_keys)? {
                records.insert(
                    TaskKey {
                        path: stored.path,
                        start: stored.record.source_key(),
                    },
                    stored.record,
                );
            }
        }

        let mut tasks = Vec::with_capacity(retained.len());
        for fact in retained {
            let task = records.remove(&fact.key).ok_or_else(|| {
                TaskPageQueryError::Query(WorkspaceQueryError::Store(
                    super::StoreError::InvalidStoredValue,
                ))
            })?;
            let depends_on = dependencies.get(&fact.key).cloned().unwrap_or_default();
            let directly_blocking =
                TaskRef::from_parts(&fact.key.path, fact.document_task, fact.id.clone())
                    .and_then(|target| dependents.get(&target))
                    .cloned()
                    .unwrap_or_default();
            let previous = fact.prev.as_deref().and_then(|source| {
                task_reference(
                    &fact.key.path,
                    &plumb_semantics::parse_task_reference_target(source),
                )
                .filter(|target| identities.contains_key(target))
            });
            tasks.push(WorkspaceTask {
                matched: matched_keys.contains(&fact.key),
                path: fact.key.path,
                revision: fact.revision,
                task,
                state: fact.state,
                wait_reasons: fact.wait_reasons,
                blocked: fact.blocked,
                actionable: fact.actionable,
                effective_priority: fact.effective_priority,
                relevance: fact.relevance,
                depends_on,
                directly_blocking,
                previous,
            });
        }
        Ok(self.query_result(TaskPage {
            tasks,
            complete,
            next_cursor,
        }))
    }

    /// Protocol-neutral shortlist: "in flight" plus "ready to start".
    ///
    /// Both sections read one workspace snapshot and one query instant. Focus is
    /// an orthogonal fact, so a focused task stays in the in-flight section even
    /// while it is waiting or blocked, and a finished focus history never keeps a
    /// task out of the candidate list.
    pub fn query_next(
        &self,
        query: &NextQuery,
    ) -> Result<QueryResult<NextResult>, TaskPageQueryError> {
        let candidate_limit = query.limit.clamp(1, 10);
        let (mut facts, open_records) = task_facts(self, None, query.now.timestamp_millis())?;
        let (identities, task_refs_by_key, states) = task_identity_context(self, &facts, false)?;
        let relations = task_relations(self, &open_records, &identities, &states)?;
        let dependencies = dependencies_by_source(&relations);
        let dependents = dependents_by_target(&relations, &task_refs_by_key);

        for fact in &mut facts {
            let blocked = dependencies.get(&fact.key).is_some_and(|targets| {
                targets.iter().any(|target| {
                    identities
                        .get(target)
                        .and_then(|key| states.get(key))
                        .is_some_and(|state| *state == TaskState::Open)
                })
            });
            let (state, wait_reasons) = workflow_state(fact, blocked, query.now);
            fact.state = state;
            fact.wait_reasons = wait_reasons;
            fact.blocked = blocked;
            fact.actionable = state == TaskWorkflowState::Ready;
        }

        // Effective priority follows the full relation graph and is computed
        // before any focus filtering, so a focused task still propagates to the
        // candidates it blocks.
        propagate_effective_priorities(&mut facts, &relations, &identities, &states);

        let invalid = facts
            .iter()
            .filter(|fact| !fact.focus_valid)
            .cloned()
            .collect::<Vec<_>>();
        let skipped_invalid = if invalid.is_empty() {
            Vec::new()
        } else {
            let mut records = hydrate_task_records(self, &invalid, open_records.clone())?;
            invalid
                .iter()
                .map(|fact| {
                    let record = records.remove(&fact.key);
                    NextSkippedFocus {
                        path: fact.key.path.clone(),
                        id: fact.id.clone(),
                        title: record
                            .as_ref()
                            .map(|record| record.title.clone())
                            .unwrap_or_else(|| fact.title.clone()),
                        range: record
                            .as_ref()
                            .and_then(|record| record.focused.range.clone())
                            .unwrap_or(fact.key.start..fact.key.start),
                        codes: record
                            .as_ref()
                            .map(|record| {
                                record
                                    .focused
                                    .problems
                                    .iter()
                                    .map(|problem| problem.code)
                                    .collect()
                            })
                            .unwrap_or_default(),
                    }
                })
                .collect()
        };

        let mut focused = facts
            .iter()
            .filter(|fact| fact.focus_valid && fact.focused)
            .cloned()
            .collect::<Vec<_>>();
        focused.sort_by(|left, right| {
            left.focused_since_millis
                .cmp(&right.focused_since_millis)
                .then_with(|| path_order_key(&left.key.path).cmp(&path_order_key(&right.key.path)))
                .then_with(|| left.key.start.cmp(&right.key.start))
        });
        let focused_total = focused.len();
        apply_next_cursor(&mut focused, query)?;
        let focused_complete = focused.len() <= NEXT_FOCUSED_PAGE;
        focused.truncate(NEXT_FOCUSED_PAGE);
        let focused_next_cursor = (!focused_complete)
            .then(|| focused.last().map(|fact| encode_next_cursor(query, fact)))
            .flatten();

        let mut candidates = facts
            .into_iter()
            .filter(|fact| {
                fact.focus_valid && !fact.focused && fact.state == TaskWorkflowState::Ready
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            right
                .effective_priority
                .cmp(&left.effective_priority)
                .then_with(|| path_order_key(&left.key.path).cmp(&path_order_key(&right.key.path)))
                .then_with(|| left.key.start.cmp(&right.key.start))
        });
        let candidates_complete = candidates.len() <= candidate_limit;
        candidates.truncate(candidate_limit);

        let focused_len = focused.len();
        let mut selected = focused;
        selected.extend(candidates);
        let mut records = hydrate_task_records(self, &selected, open_records)?;
        let mut tasks = Vec::with_capacity(selected.len());
        for fact in selected {
            tasks.push(workspace_task(
                fact,
                &mut records,
                &dependencies,
                &dependents,
                &identities,
            )?);
        }
        let candidates = tasks.split_off(focused_len);
        let focused = tasks;

        Ok(self.query_result(NextResult {
            focused,
            focused_total,
            focused_complete,
            focused_next_cursor,
            candidates,
            candidate_limit,
            candidates_complete,
            skipped_invalid,
        }))
    }
}

/// Hydrate full records for exactly the selected keys; the lightweight facts
/// stay the only thing the query enumerates.

fn hydrate_task_records(
    workspace: &Workspace,
    facts: &[TaskFact],
    open_records: HashMap<TaskKey, TaskRecord>,
) -> Result<HashMap<TaskKey, TaskRecord>, TaskPageQueryError> {
    let page_keys = facts
        .iter()
        .map(|fact| fact.key.clone())
        .collect::<HashSet<_>>();
    let stored_keys = facts
        .iter()
        .filter(|fact| !open_records.contains_key(&fact.key))
        .map(|fact| StoredTaskKey {
            path: fact.key.path.clone(),
            start: fact.key.start,
        })
        .collect::<Vec<_>>();
    let mut records = open_records
        .into_iter()
        .filter(|(key, _)| page_keys.contains(key))
        .collect::<HashMap<_, _>>();
    if let Some(store) = &workspace.disk_store {
        for stored in store.tasks_by_keys(&stored_keys)? {
            records.insert(
                TaskKey {
                    path: stored.path,
                    start: stored.record.source_key(),
                },
                stored.record,
            );
        }
    }
    Ok(records)
}

fn workspace_task(
    fact: TaskFact,
    records: &mut HashMap<TaskKey, TaskRecord>,
    dependencies: &HashMap<TaskKey, Vec<TaskRef>>,
    dependents: &HashMap<TaskRef, Vec<TaskRef>>,
    identities: &HashMap<TaskRef, TaskKey>,
) -> Result<WorkspaceTask, TaskPageQueryError> {
    let task = records.remove(&fact.key).ok_or_else(|| {
        TaskPageQueryError::Query(WorkspaceQueryError::Store(
            super::StoreError::InvalidStoredValue,
        ))
    })?;
    let depends_on = dependencies.get(&fact.key).cloned().unwrap_or_default();
    let directly_blocking =
        TaskRef::from_parts(&fact.key.path, fact.document_task, fact.id.clone())
            .and_then(|target| dependents.get(&target))
            .cloned()
            .unwrap_or_default();
    let previous = fact.prev.as_deref().and_then(|source| {
        task_reference(
            &fact.key.path,
            &plumb_semantics::parse_task_reference_target(source),
        )
        .filter(|target| identities.contains_key(target))
    });
    Ok(WorkspaceTask {
        matched: true,
        path: fact.key.path,
        revision: fact.revision,
        task,
        state: fact.state,
        wait_reasons: fact.wait_reasons,
        blocked: fact.blocked,
        actionable: fact.actionable,
        effective_priority: fact.effective_priority,
        relevance: fact.relevance,
        depends_on,
        directly_blocking,
        previous,
    })
}

fn next_cursor_signature(query: &NextQuery) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"plumb-next-shortlist-v1");
    hash_field(&mut digest, query.root.to_string_lossy().as_bytes());
    digest.finalize().into()
}

fn apply_next_cursor(
    facts: &mut Vec<TaskFact>,
    query: &NextQuery,
) -> Result<(), TaskPageQueryError> {
    let Some(cursor) = &query.cursor else {
        return Ok(());
    };
    let parts = cursor.split(':').collect::<Vec<_>>();
    let [version, revision, signature, since, path, start] = parts.as_slice() else {
        return Err(TaskPageQueryError::Cursor(
            "next cursor is malformed".to_string(),
        ));
    };
    let since = since.parse::<i64>().ok();
    let start = start.parse::<usize>().ok();
    if *version != "v1"
        || revision.parse::<u64>().ok() != Some(query.workspace_revision)
        || *signature != hex(&next_cursor_signature(query))
        || since.is_none()
        || start.is_none()
    {
        return Err(TaskPageQueryError::Cursor(
            "next cursor is stale or does not match this query".to_string(),
        ));
    }
    let (since, start) = (since.unwrap(), start.unwrap());
    let Some(last) = facts.iter().rposition(|fact| {
        fact.focused_since_millis == Some(since)
            && path_order_key(&fact.key.path) == *path
            && fact.key.start == start
    }) else {
        return Err(TaskPageQueryError::Cursor(
            "next cursor no longer identifies a result task".to_string(),
        ));
    };
    facts.drain(..=last);
    Ok(())
}

fn encode_next_cursor(query: &NextQuery, fact: &TaskFact) -> String {
    format!(
        "v1:{}:{}:{}:{}:{}",
        query.workspace_revision,
        hex(&next_cursor_signature(query)),
        fact.focused_since_millis.unwrap_or_default(),
        path_order_key(&fact.key.path),
        fact.key.start
    )
}

fn compile_filters(
    groups: &[TaskQueryFilterGroup],
) -> Result<Vec<Vec<CompiledFilter>>, TaskPageQueryError> {
    groups
        .iter()
        .map(|group| {
            group
                .filters
                .iter()
                .map(|filter| {
                    let program = Program::compile(&filter.expression).map_err(|error| {
                        TaskPageQueryError::Filter {
                            source: filter.source.clone(),
                            message: format!("invalid CEL query: {error}"),
                        }
                    })?;
                    let variables = program
                        .references()
                        .variables()
                        .into_iter()
                        .map(str::to_string)
                        .collect();
                    Ok(CompiledFilter {
                        source: filter.source.clone(),
                        program,
                        variables,
                    })
                })
                .collect()
        })
        .collect()
}

fn task_candidate_predicate(groups: &[Vec<CompiledFilter>]) -> Option<TaskCandidatePredicate> {
    let mut candidates = Vec::new();
    for group in groups {
        let prefixes = group
            .iter()
            .map(|filter| task_candidate_prefix(filter.program.expression()))
            .collect::<Vec<_>>();
        let complete = prefixes.iter().all(|prefix| prefix.complete);
        let group_candidate =
            TaskCandidatePredicate::or(prefixes.into_iter().filter_map(|prefix| prefix.predicate))?;
        candidates.push(group_candidate);
        if !complete {
            break;
        }
    }
    TaskCandidatePredicate::and(candidates)
        .filter(|predicate| !matches!(predicate, TaskCandidatePredicate::Constant(true)))
}

fn task_facts(
    workspace: &Workspace,
    candidate: Option<&TaskCandidatePredicate>,
    now_millis: i64,
) -> Result<(Vec<TaskFact>, HashMap<TaskKey, TaskRecord>), TaskPageQueryError> {
    let mut facts = Vec::new();
    let mut open_records = HashMap::new();
    for entry in workspace.documents.values() {
        let Some(current) = &entry.current else {
            continue;
        };
        let mut ancestors = Vec::new();
        for mut task in &current.output.tasks().tasks {
            super::apply_document_task_title(&mut task, &entry.path);
            ancestors.truncate(task.depth);
            let key = TaskKey {
                path: entry.path.clone(),
                start: task.source_key(),
            };
            facts.push(fact_from_record(
                key.clone(),
                current.revision,
                &task,
                ancestors.last().copied(),
            ));
            ancestors.push(task.source_key());
            open_records.insert(key, task.clone());
        }
    }
    if let Some(store) = &workspace.disk_store {
        let stored = if let Some(candidate) = candidate {
            store.task_facts_matching(candidate, now_millis, &workspace.open_paths())?
        } else {
            store.task_facts(&workspace.open_paths())?
        };
        facts.extend(stored.into_iter().map(fact_from_stored));
    }
    facts.sort_by(|left, right| left.key.cmp(&right.key));
    Ok((facts, open_records))
}

type TaskIdentityContext = (
    HashMap<TaskRef, TaskKey>,
    HashMap<TaskKey, TaskRef>,
    HashMap<TaskKey, TaskState>,
);

fn task_identity_context(
    workspace: &Workspace,
    facts: &[TaskFact],
    candidate_applied: bool,
) -> Result<TaskIdentityContext, TaskPageQueryError> {
    let mut identities = HashMap::new();
    let mut task_refs_by_key = HashMap::new();
    let mut states = HashMap::new();
    for fact in facts {
        insert_task_identity(
            &mut identities,
            &mut task_refs_by_key,
            &mut states,
            fact.key.clone(),
            fact.id.clone(),
            fact.document_task,
            fact.closure_state,
        );
    }
    if candidate_applied {
        if let Some(store) = &workspace.disk_store {
            for identity in store.task_identities(&workspace.open_paths())? {
                insert_stored_task_identity(
                    &mut identities,
                    &mut task_refs_by_key,
                    &mut states,
                    identity,
                );
            }
        }
    }
    Ok((identities, task_refs_by_key, states))
}

fn insert_stored_task_identity(
    identities: &mut HashMap<TaskRef, TaskKey>,
    task_refs_by_key: &mut HashMap<TaskKey, TaskRef>,
    states: &mut HashMap<TaskKey, TaskState>,
    identity: StoredTaskIdentity,
) {
    let key = TaskKey {
        path: identity.path,
        start: identity.start,
    };
    insert_task_identity(
        identities,
        task_refs_by_key,
        states,
        key,
        identity.id,
        identity.document_task,
        task_state_from_name(&identity.closure_state),
    );
}

fn insert_task_identity(
    identities: &mut HashMap<TaskRef, TaskKey>,
    task_refs_by_key: &mut HashMap<TaskKey, TaskRef>,
    states: &mut HashMap<TaskKey, TaskState>,
    key: TaskKey,
    id: Option<String>,
    document_task: bool,
    state: TaskState,
) {
    states.insert(key.clone(), state);
    if let Some(task_ref) = TaskRef::from_parts(&key.path, document_task, id) {
        identities.insert(task_ref.clone(), key.clone());
        task_refs_by_key.insert(key, task_ref);
    }
}

fn task_state_from_name(name: &str) -> TaskState {
    match name {
        "done" => TaskState::Done,
        "canceled" => TaskState::Canceled,
        "conflicted" => TaskState::Conflicted,
        _ => TaskState::Open,
    }
}

fn fact_from_record(
    key: TaskKey,
    revision: i64,
    task: &TaskRecord,
    parent_start: Option<usize>,
) -> TaskFact {
    TaskFact {
        document_task: task.owner == TaskOwner::Document,
        document_order: path_order_key(&key.path),
        key,
        revision,
        id: task.id.as_ref().map(|field| field.value.clone()),
        title: task.title.clone(),
        closure_state: task.state(),
        created_millis: field_millis(task.created.as_ref()),
        due_millis: field_millis(task.due.as_ref()),
        wait_millis: field_millis(task.wait.as_ref()),
        done_millis: field_millis(task.done.as_ref()),
        canceled_millis: field_millis(task.canceled.as_ref()),
        priority: task.priority,
        depth: task.depth,
        parent_start,
        recur: task.recur.as_ref().map(|field| field.value.clone()),
        prev: task.prev.as_ref().map(|field| field.value.clone()),
        focused: task.is_focused(),
        focused_since_millis: task.focused_since().and_then(focus_since_millis),
        focus_valid: task.focus_valid(),
        state: TaskWorkflowState::Ready,
        wait_reasons: Vec::new(),
        blocked: false,
        actionable: false,
        relevance: 0,
        effective_priority: task.priority.unwrap_or_default(),
    }
}

fn fact_from_stored(stored: StoredTaskFact) -> TaskFact {
    let key = TaskKey {
        path: stored.path,
        start: stored.start,
    };
    TaskFact {
        document_task: stored.document_task,
        document_order: path_order_key(&key.path),
        key,
        revision: stored.revision,
        id: stored.id,
        title: stored.title,
        closure_state: match stored.closure_state.as_str() {
            "done" => TaskState::Done,
            "canceled" => TaskState::Canceled,
            "conflicted" => TaskState::Conflicted,
            _ => TaskState::Open,
        },
        created_millis: stored.created_millis,
        due_millis: stored.due_millis,
        wait_millis: stored.wait_millis,
        done_millis: stored.done_millis,
        canceled_millis: stored.canceled_millis,
        priority: stored.priority,
        depth: stored.depth,
        parent_start: stored.parent_start,
        recur: stored.recur,
        prev: stored.prev,
        focused: stored.focused,
        focused_since_millis: stored.focused_since_millis,
        focus_valid: stored.focus_valid,
        state: TaskWorkflowState::Ready,
        wait_reasons: Vec::new(),
        blocked: false,
        actionable: false,
        relevance: 0,
        effective_priority: stored.priority.unwrap_or_default(),
    }
}

fn task_relations(
    workspace: &Workspace,
    open_records: &HashMap<TaskKey, TaskRecord>,
    identities: &HashMap<TaskRef, TaskKey>,
    states: &HashMap<TaskKey, TaskState>,
) -> Result<Vec<TaskRelation>, TaskPageQueryError> {
    let mut relations = workspace
        .disk_store
        .as_ref()
        .map(|store| store.task_dependency_relations(&workspace.open_paths()))
        .transpose()?
        .unwrap_or_default()
        .into_iter()
        .filter_map(|relation| relation_from_stored(relation, states, identities))
        .collect::<Vec<_>>();
    for (source, task) in open_records {
        for dependency in &task.depends {
            let Some(target) = task_reference(&source.path, &dependency.target) else {
                continue;
            };
            if identities.contains_key(&target) {
                relations.push(TaskRelation {
                    source: source.clone(),
                    target,
                });
            }
        }
    }
    relations.sort_by(|left, right| {
        left.source
            .cmp(&right.source)
            .then(left.target.path.cmp(&right.target.path))
            .then(left.target.id.cmp(&right.target.id))
    });
    relations.dedup_by(|left, right| left.source == right.source && left.target == right.target);
    Ok(relations)
}

fn relation_from_stored(
    relation: StoredTaskDependency,
    states: &HashMap<TaskKey, TaskState>,
    identities: &HashMap<TaskRef, TaskKey>,
) -> Option<TaskRelation> {
    let source = TaskKey {
        path: relation.source_path,
        start: relation.source_start,
    };
    let target = TaskRef {
        path: relation.target_path,
        id: relation.target_id,
    };
    (states.contains_key(&source) && identities.contains_key(&target))
        .then_some(TaskRelation { source, target })
}

fn task_reference(source_path: &Path, target: &TaskReferenceTarget) -> Option<TaskRef> {
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

fn dependencies_by_source(relations: &[TaskRelation]) -> HashMap<TaskKey, Vec<TaskRef>> {
    let mut values = HashMap::<TaskKey, Vec<TaskRef>>::new();
    for relation in relations {
        values
            .entry(relation.source.clone())
            .or_default()
            .push(relation.target.clone());
    }
    for targets in values.values_mut() {
        targets.sort_by(|left, right| left.path.cmp(&right.path).then(left.id.cmp(&right.id)));
        targets.dedup();
    }
    values
}

fn dependents_by_target(
    relations: &[TaskRelation],
    task_refs_by_key: &HashMap<TaskKey, TaskRef>,
) -> HashMap<TaskRef, Vec<TaskRef>> {
    let mut values = HashMap::<TaskRef, Vec<TaskRef>>::new();
    for relation in relations {
        if let Some(source) = task_refs_by_key.get(&relation.source) {
            values
                .entry(relation.target.clone())
                .or_default()
                .push(source.clone());
        }
    }
    for sources in values.values_mut() {
        sources.sort_by(|left, right| left.path.cmp(&right.path).then(left.id.cmp(&right.id)));
        sources.dedup();
    }
    values
}

fn workflow_state(
    fact: &TaskFact,
    blocked: bool,
    now: DateTime<FixedOffset>,
) -> (TaskWorkflowState, Vec<TaskWaitReason>) {
    match fact.closure_state {
        TaskState::Done => (TaskWorkflowState::Done, Vec::new()),
        TaskState::Canceled => (TaskWorkflowState::Canceled, Vec::new()),
        TaskState::Conflicted => (TaskWorkflowState::Conflicted, Vec::new()),
        TaskState::Open => {
            let waiting = fact
                .wait_millis
                .is_some_and(|millis| millis > now.timestamp_millis());
            let mut reasons = Vec::new();
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

fn filter_groups_match(
    groups: &[Vec<CompiledFilter>],
    fact: &TaskFact,
    root: &Path,
    now: DateTime<FixedOffset>,
    dependencies: &[TaskRef],
    dependents: &[TaskRef],
) -> Result<bool, TaskPageQueryError> {
    for group in groups {
        let mut matched = false;
        for filter in group {
            let mut context = Context::default();
            context.add_variable_from_value("path", display_workspace_path(root, &fact.key.path));
            context.add_variable_from_value(
                "id",
                fact.id
                    .clone()
                    .map_or(Value::Null, |value| Value::String(value.into())),
            );
            context.add_variable_from_value("title", fact.title.clone());
            context.add_variable_from_value("created", timestamp_value(fact.created_millis));
            context.add_variable_from_value("due", timestamp_value(fact.due_millis));
            context.add_variable_from_value(
                "priority",
                fact.priority
                    .map_or(Value::Null, |value| Value::Int(i64::from(value))),
            );
            context.add_variable_from_value("wait", timestamp_value(fact.wait_millis));
            context.add_variable_from_value("done", timestamp_value(fact.done_millis));
            context.add_variable_from_value("canceled", timestamp_value(fact.canceled_millis));
            context.add_variable_from_value(
                "recur",
                fact.recur
                    .clone()
                    .map_or(Value::Null, |value| Value::String(value.into())),
            );
            context.add_variable_from_value(
                "prev",
                fact.prev
                    .clone()
                    .map_or(Value::Null, |value| Value::String(value.into())),
            );
            context.add_variable_from_value("focused", fact.focused);
            context.add_variable_from_value(
                "focused_since",
                timestamp_value(fact.focused_since_millis),
            );
            if filter.variables.contains("depends_on") {
                context.add_variable_from_value(
                    "depends_on",
                    dependencies
                        .iter()
                        .map(|target| display_task_ref(root, target))
                        .collect::<Vec<_>>(),
                );
            }
            if filter.variables.contains("directly_blocking") {
                context.add_variable_from_value(
                    "directly_blocking",
                    dependents
                        .iter()
                        .map(|source| display_task_ref(root, source))
                        .collect::<Vec<_>>(),
                );
            }
            context.add_variable_from_value("state", fact.state.as_str());
            context.add_variable_from_value(
                "wait_reasons",
                fact.wait_reasons
                    .iter()
                    .map(|reason| reason.as_str())
                    .collect::<Vec<_>>(),
            );
            context.add_variable_from_value("blocked", fact.blocked);
            context.add_variable_from_value("actionable", fact.actionable);
            context.add_variable_from_value("now", Value::Timestamp(now));
            match filter.program.execute(&context) {
                Ok(Value::Bool(true)) => matched = true,
                Ok(Value::Bool(false)) | Err(ExecutionError::NoSuchKey(_)) => {}
                Ok(value) => {
                    return Err(TaskPageQueryError::Filter {
                        source: filter.source.clone(),
                        message: format!("CEL query must return bool, got {value:?}"),
                    })
                }
                Err(error) => {
                    return Err(TaskPageQueryError::Filter {
                        source: filter.source.clone(),
                        message: format!("cannot evaluate CEL query for '{}': {error}", fact.title),
                    })
                }
            }
            if matched {
                break;
            }
        }
        if !matched {
            return Ok(false);
        }
    }
    Ok(true)
}

fn propagate_effective_priorities(
    facts: &mut [TaskFact],
    relations: &[TaskRelation],
    identities: &HashMap<TaskRef, TaskKey>,
    states: &HashMap<TaskKey, TaskState>,
) {
    let indexes = facts
        .iter()
        .enumerate()
        .map(|(index, fact)| (fact.key.clone(), index))
        .collect::<HashMap<_, _>>();
    let mut edges = Vec::new();
    for (index, fact) in facts.iter().enumerate() {
        if let Some(parent_start) = fact.parent_start {
            if let Some(parent) = indexes.get(&TaskKey {
                path: fact.key.path.clone(),
                start: parent_start,
            }) {
                edges.push((index, *parent));
            }
        }
    }
    for relation in relations {
        let Some(source) = indexes.get(&relation.source) else {
            continue;
        };
        let Some(target_key) = identities.get(&relation.target) else {
            continue;
        };
        let Some(target) = indexes.get(target_key) else {
            continue;
        };
        if states.get(target_key) == Some(&TaskState::Open) {
            edges.push((*source, *target));
        }
    }
    let mut priorities = facts
        .iter()
        .map(|fact| fact.priority.unwrap_or_default())
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
    for (fact, priority) in facts.iter_mut().zip(priorities) {
        fact.effective_priority = priority;
    }
}

fn apply_cursor(
    facts: &mut Vec<TaskFact>,
    query: &TaskPageQuery,
    include_ancestors: bool,
) -> Result<(), TaskPageQueryError> {
    let Some(cursor) = &query.cursor else {
        return Ok(());
    };
    let mut parts = cursor.splitn(4, ':');
    let version = parts.next();
    let revision = parts.next().and_then(|value| value.parse::<u64>().ok());
    let signature = parts.next();
    let document = parts.next();
    let expected = query_signature(query, include_ancestors);
    if version != Some("v2")
        || revision != Some(query.workspace_revision)
        || signature != Some(hex(&expected).as_str())
        || document.is_none()
    {
        return Err(TaskPageQueryError::Cursor(
            "task cursor is stale or does not match this query".to_string(),
        ));
    }
    let document = document.unwrap();
    let Some(last) = facts
        .iter()
        .rposition(|fact| hex(&path_identity(&fact.key.path)) == document)
    else {
        return Err(TaskPageQueryError::Cursor(
            "task cursor no longer identifies a result document".to_string(),
        ));
    };
    facts.drain(..=last);
    Ok(())
}

fn encode_cursor(query: &TaskPageQuery, path: &Path, include_ancestors: bool) -> String {
    format!(
        "v2:{}:{}:{}",
        query.workspace_revision,
        hex(&query_signature(query, include_ancestors)),
        hex(&path_identity(path))
    )
}

fn query_signature(query: &TaskPageQuery, include_ancestors: bool) -> [u8; 32] {
    let mut digest = Sha256::new();
    // The shared Tasks ordering now depends on the fixed subtree-focus rule in
    // addition to the caller's keys, so cursors issued before that rule must not
    // be accepted as if they described the same order.
    hash_field(&mut digest, b"subtree-focused-first-v1");
    hash_field(&mut digest, &[u8::from(include_ancestors)]);
    hash_field(&mut digest, &path_identity(&query.root));
    hash_field(&mut digest, query.text.as_bytes());
    hash_field(&mut digest, &query.limit.to_le_bytes());
    for order in &query.sort {
        hash_field(&mut digest, &[*order as u8]);
    }
    for group in &query.filter_groups {
        hash_field(&mut digest, b"group");
        for filter in &group.filters {
            hash_field(&mut digest, filter.expression.as_bytes());
        }
    }
    digest.finalize().into()
}

fn hash_field(digest: &mut Sha256, value: &[u8]) {
    digest.update(value.len().to_le_bytes());
    digest.update(value);
}

fn path_identity(path: &Path) -> [u8; 32] {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Sha256::digest(path.as_os_str().as_bytes()).into()
    }
    #[cfg(not(unix))]
    {
        Sha256::digest(path.as_os_str().to_string_lossy().as_bytes()).into()
    }
}

fn path_order_key(path: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        hex(path.as_os_str().as_bytes())
    }
    #[cfg(not(unix))]
    {
        hex(path.as_os_str().to_string_lossy().as_bytes())
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(DIGITS[(byte >> 4) as usize] as char);
        encoded.push(DIGITS[(byte & 0xf) as usize] as char);
    }
    encoded
}

fn field_millis(field: Option<&plumb_semantics::TaskField>) -> Option<i64> {
    field
        .and_then(|field| DateTime::parse_from_rfc3339(&field.value).ok())
        .map(|value| value.timestamp_millis())
}

fn focus_since_millis(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.timestamp_millis())
}

fn datetime_from_millis(millis: i64) -> Option<DateTime<FixedOffset>> {
    DateTime::from_timestamp_millis(millis).map(|value| value.fixed_offset())
}

fn timestamp_value(millis: Option<i64>) -> Value {
    millis
        .and_then(datetime_from_millis)
        .map_or(Value::Null, Value::Timestamp)
}

fn display_task_ref(root: &Path, task: &TaskRef) -> String {
    task.display(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SqliteSemanticStore;

    fn now() -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339("2026-08-28T12:00:00Z").unwrap()
    }

    #[test]
    fn document_task_relations_and_priority_match_memory_and_disk() {
        for disk in [false, true] {
            let mut workspace = if disk {
                Workspace::with_sqlite_store(SqliteSemanticStore::open_in_memory().unwrap())
            } else {
                Workspace::new()
            };
            for (path, text) in [
                ("target.plumb", "`- Anonymous child\n `+ task\n\n`+ task\n"),
                (
                    "source.plumb",
                    "`+ task\n`= depends target.plumb\n`= prev target.plumb\n`= priority 19\n",
                ),
            ] {
                if disk {
                    workspace.insert_disk(path, 0, text).unwrap();
                } else {
                    workspace.insert(path, 0, text);
                }
            }
            let page = workspace.query_task_page(&query()).unwrap().value;
            let target = page
                .tasks
                .iter()
                .find(|task| {
                    task.path == Path::new("target.plumb") && task.task.owner == TaskOwner::Document
                })
                .unwrap();
            assert_eq!(target.effective_priority, 19);
            assert_eq!(
                target.directly_blocking,
                [TaskRef {
                    path: "source.plumb".into(),
                    id: None
                }]
            );
            let source = page
                .tasks
                .iter()
                .find(|task| task.path == Path::new("source.plumb"))
                .unwrap();
            assert!(source.blocked);
            assert_eq!(
                source.depends_on,
                [TaskRef {
                    path: "target.plumb".into(),
                    id: None
                }]
            );
            assert_eq!(
                source.previous,
                Some(TaskRef {
                    path: "target.plumb".into(),
                    id: None
                })
            );
            let child = page
                .tasks
                .iter()
                .find(|task| task.task.owner == TaskOwner::ListItem)
                .unwrap();
            assert!(child.directly_blocking.is_empty());
        }
    }

    fn query() -> TaskPageQuery {
        TaskPageQuery {
            root: PathBuf::new(),
            text: String::new(),
            filter_groups: Vec::new(),
            sort: vec![TaskSortOrder::Priority],
            limit: usize::MAX,
            cursor: None,
            workspace_revision: 11,
            now: now(),
        }
    }

    fn populate(workspace: &mut Workspace, persistent: bool) {
        let documents = [
            (
                "a.plumb",
                concat!(
                    "`- Parent\n\n `+ task\n\n `@ parent\n\n `= priority -1\n",
                    " `- Dependent\n\n  `+ task\n\n  `@ dependent\n  `= priority 20\n  `= depends b.plumb#target\n",
                ),
            ),
            (
                "b.plumb",
                "`- Target\n\n `+ task\n\n `@ target\n\n `= priority 1\n",
            ),
            (
                "c.plumb",
                "`- Done\n\n `+ task\n\n `@ done\n\n `= done 2026-08-27T12:00:00Z\n",
            ),
        ];
        for (path, source) in documents {
            if persistent {
                workspace.insert_disk(path, 1, source).unwrap();
            } else {
                workspace.insert(path, 1, source);
            }
        }
    }

    #[test]
    fn document_task_keys_titles_and_tree_order_match_disk_and_overlays() {
        let source =
            "`- First\n `+ task\n `@ first\n `= priority 9\n\n`+ task\n\n`- Second\n `+ task\n";
        let mut memory = Workspace::new();
        let mut persistent =
            Workspace::with_sqlite_store(SqliteSemanticStore::open_in_memory().unwrap());
        memory.insert("project.plumb", 1, source);
        persistent.insert_disk("project.plumb", 1, source).unwrap();
        for (revision, changed) in [
            (1, source.to_owned()),
            (
                2,
                source.replace("`+ task\n\n`- Second", "`+ journal\n\n`- Second"),
            ),
            (3, source.replace("First", "Updated")),
        ] {
            if revision > 1 {
                memory.insert("project.plumb", revision, changed.clone());
                persistent.insert("project.plumb", revision, changed);
            }
            let expected = memory.query_task_page(&query()).unwrap().value;
            let actual = persistent.query_task_page(&query()).unwrap().value;
            assert_eq!(actual, expected);
            if revision != 2 {
                assert_eq!(actual.tasks.len(), 3);
                assert_eq!(actual.tasks[0].task.owner, TaskOwner::Document);
                assert_eq!(actual.tasks[0].task.title, "project");
                assert_eq!(actual.tasks[0].effective_priority, 9);
                assert_eq!(
                    actual
                        .tasks
                        .iter()
                        .map(|task| task.task.depth)
                        .collect::<Vec<_>>(),
                    [0, 1, 1]
                );
            } else {
                assert_eq!(actual.tasks.len(), 2);
            }
            assert_eq!(
                persistent.task_document_metrics().unwrap().value,
                memory.task_document_metrics().unwrap().value
            );
            assert_eq!(
                persistent
                    .search_records("", Some(crate::SearchRecordKind::Task), "", 100, now())
                    .unwrap()
                    .value,
                memory
                    .search_records("", Some(crate::SearchRecordKind::Task), "", 100, now())
                    .unwrap()
                    .value
            );
        }
    }

    #[test]
    fn task_pages_match_memory_and_persistent_relations() {
        let mut memory = Workspace::new();
        populate(&mut memory, false);
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let mut persistent = Workspace::with_sqlite_store(store);
        populate(&mut persistent, true);

        let memory_page = memory.query_task_page(&query()).unwrap().value;
        let persistent_page = persistent.query_task_page(&query()).unwrap();
        assert_eq!(
            persistent_page.provenance,
            super::super::QueryProvenance::Persistent
        );
        assert_eq!(persistent_page.value, memory_page);
        assert_eq!(
            persistent_page
                .value
                .tasks
                .iter()
                .map(|task| task.task.id.as_ref().unwrap().value.as_str())
                .collect::<Vec<_>>(),
            ["parent", "dependent", "target", "done"]
        );
        let target = persistent_page
            .value
            .tasks
            .iter()
            .find(|task| task.task.id.as_ref().unwrap().value == "target")
            .unwrap();
        assert_eq!(target.effective_priority, 20);
        assert_eq!(
            target
                .directly_blocking
                .iter()
                .map(|task| task.id.as_deref())
                .collect::<Vec<_>>(),
            [Some("dependent")]
        );
    }

    #[test]
    fn task_document_metrics_match_memory_persistent_and_open_overlays() {
        let mut memory = Workspace::new();
        populate(&mut memory, false);
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let mut persistent = Workspace::with_sqlite_store(store);
        populate(&mut persistent, true);

        assert_eq!(
            persistent.task_document_metrics().unwrap().value,
            memory.task_document_metrics().unwrap().value
        );
        assert_eq!(
            persistent.task_document_metrics().unwrap().value,
            vec![
                TaskDocumentMetrics {
                    path: PathBuf::from("a.plumb"),
                    tasks: 2,
                    open_tasks: 2,
                },
                TaskDocumentMetrics {
                    path: PathBuf::from("b.plumb"),
                    tasks: 1,
                    open_tasks: 1,
                },
                TaskDocumentMetrics {
                    path: PathBuf::from("c.plumb"),
                    tasks: 1,
                    open_tasks: 0,
                },
            ]
        );

        persistent.insert("c.plumb", 2, "`- Reopened\n\n `+ task\n\n `@ reopened\n");
        assert_eq!(
            persistent
                .task_document_metrics()
                .unwrap()
                .value
                .into_iter()
                .find(|metrics| metrics.path == Path::new("c.plumb"))
                .unwrap()
                .open_tasks,
            1
        );
    }

    #[test]
    fn task_page_cel_relations_follow_open_document_precedence() {
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let mut workspace = Workspace::with_sqlite_store(store);
        populate(&mut workspace, true);
        let mut relation_query = query();
        relation_query.sort = Vec::new();
        relation_query.filter_groups = vec![TaskQueryFilterGroup {
            filters: vec![TaskQueryFilter {
                source: "custom:1".to_string(),
                expression: "directly_blocking.size() > 0".to_string(),
            }],
        }];

        let page = workspace.query_task_page(&relation_query).unwrap().value;
        assert_eq!(page.tasks.len(), 1);
        assert_eq!(
            page.tasks.get(0).unwrap().task.id.as_ref().unwrap().value,
            "target"
        );

        workspace.open_document(
            "a.plumb",
            2,
            "`- Parent\n\n `+ task\n\n `@ parent\n\n `- Current\n\n  `+ task\n\n `@ current\n",
        );
        assert!(workspace
            .query_task_page(&relation_query)
            .unwrap()
            .value
            .tasks
            .is_empty());
    }

    #[test]
    fn task_cursor_preserves_documents_and_binds_the_query_revision() {
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let mut workspace = Workspace::with_sqlite_store(store);
        populate(&mut workspace, true);
        let mut first_query = query();
        first_query.limit = 1;
        let first = workspace.query_task_page(&first_query).unwrap().value;
        assert!(!first.complete);
        assert_eq!(first.tasks.len(), 2);
        assert!(first
            .tasks
            .iter()
            .all(|task| task.path == Path::new("a.plumb")));

        let mut second_query = first_query.clone();
        second_query.cursor = first.next_cursor.clone();
        let second = workspace.query_task_page(&second_query).unwrap().value;
        assert!(second
            .tasks
            .iter()
            .all(|task| task.path != Path::new("a.plumb")));

        let mut changed = second_query.clone();
        changed.text = "target".to_string();
        assert!(matches!(
            workspace.query_task_page(&changed),
            Err(TaskPageQueryError::Cursor(_))
        ));
        let mut stale = second_query;
        stale.workspace_revision += 1;
        assert!(matches!(
            workspace.query_task_page(&stale),
            Err(TaskPageQueryError::Cursor(_))
        ));
    }

    #[test]
    fn done_task_pages_are_bounded_and_continue_with_an_opaque_cursor() {
        let mut workspace = Workspace::new();
        for index in 0..101 {
            workspace.insert(
                format!("{index:03}.plumb"),
                1,
                format!(
                    "`- Done {index}\n\n `+ task\n\n `@ done-{index}\n\n `= done 2026-08-28T10:00:00Z\n"
                ),
            );
        }
        let mut first_query = query();
        first_query.limit = 100;
        first_query.sort = vec![TaskSortOrder::Source];
        first_query.filter_groups = vec![TaskQueryFilterGroup {
            filters: vec![TaskQueryFilter {
                source: "done".to_string(),
                expression: "state == 'done'".to_string(),
            }],
        }];

        let first = workspace.query_task_page(&first_query).unwrap().value;
        assert_eq!(first.tasks.len(), 100);
        assert!(!first.complete);
        assert!(first.next_cursor.is_some());

        let second = workspace
            .query_task_page(&TaskPageQuery {
                cursor: first.next_cursor,
                ..first_query
            })
            .unwrap()
            .value;
        assert_eq!(second.tasks.len(), 1);
        assert!(second.complete);
        assert!(second.next_cursor.is_none());
    }

    #[test]
    fn task_filter_errors_preserve_their_source() {
        let mut workspace = Workspace::new();
        populate(&mut workspace, false);
        let mut invalid = query();
        invalid.filter_groups = vec![TaskQueryFilterGroup {
            filters: vec![TaskQueryFilter {
                source: "custom:2".to_string(),
                expression: "title".to_string(),
            }],
        }];
        assert!(matches!(
            workspace.query_task_page(&invalid),
            Err(TaskPageQueryError::Filter { source, message })
                if source == "custom:2" && message.contains("must return bool")
        ));
    }

    #[test]
    fn task_filter_groups_or_within_groups_and_intersect_across_groups() {
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let mut workspace = Workspace::with_sqlite_store(store);
        populate(&mut workspace, true);
        let mut filtered = query();
        filtered.filter_groups = vec![
            TaskQueryFilterGroup {
                filters: vec![
                    TaskQueryFilter {
                        source: "preset:done".to_string(),
                        expression: "state == 'done'".to_string(),
                    },
                    TaskQueryFilter {
                        source: "custom:1".to_string(),
                        expression: "priority != null && priority >= 20".to_string(),
                    },
                ],
            },
            TaskQueryFilterGroup {
                filters: vec![TaskQueryFilter {
                    source: "custom:2".to_string(),
                    expression: "title.contains('Dep')".to_string(),
                }],
            },
        ];

        let compiled = compile_filters(&filtered.filter_groups).unwrap();
        let candidate = task_candidate_predicate(&compiled).unwrap();
        let (candidate_facts, _) = task_facts(
            &workspace,
            Some(&candidate),
            filtered.now.timestamp_millis(),
        )
        .unwrap();
        assert_eq!(
            candidate_facts
                .iter()
                .map(|fact| fact.id.as_deref().unwrap())
                .collect::<Vec<_>>(),
            ["dependent", "done"]
        );

        let page = workspace.query_task_page(&filtered).unwrap().value;
        assert_eq!(page.tasks.len(), 1);
        assert_eq!(
            page.tasks.get(0).unwrap().task.id.as_ref().unwrap().value,
            "dependent"
        );
        assert_eq!(page.tasks.get(0).unwrap().state, TaskWorkflowState::Blocked);
        assert_eq!(
            page.tasks
                .get(0)
                .unwrap()
                .depends_on
                .iter()
                .map(|task| (task.path.as_path(), task.id.as_deref()))
                .collect::<Vec<_>>(),
            [(Path::new("b.plumb"), Some("target"))]
        );
    }

    #[test]
    fn sql_candidates_keep_full_cel_dependency_context_and_skip_other_records() {
        let mut memory = Workspace::new();
        populate(&mut memory, false);
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let mut persistent = Workspace::with_sqlite_store(store.clone());
        populate(&mut persistent, true);
        let mut filtered = query();
        filtered.sort.clear();
        filtered.filter_groups = vec![TaskQueryFilterGroup {
            filters: vec![TaskQueryFilter {
                source: "custom:blocked".to_string(),
                expression: "title == 'Dependent' && blocked".to_string(),
            }],
        }];

        let expected = memory.query_task_page(&filtered).unwrap().value;
        let compiled = compile_filters(&filtered.filter_groups).unwrap();
        let candidate = task_candidate_predicate(&compiled).unwrap();
        let (candidate_facts, _) = task_facts(
            &persistent,
            Some(&candidate),
            filtered.now.timestamp_millis(),
        )
        .unwrap();
        assert_eq!(candidate_facts.len(), 1);
        assert_eq!(candidate_facts[0].id.as_deref(), Some("dependent"));

        store
            .execute_batch_for_test("UPDATE tasks SET record = X'00' WHERE title <> 'Dependent'")
            .unwrap();
        let actual = persistent.query_task_page(&filtered).unwrap().value;
        assert_eq!(actual, expected);
        assert_eq!(
            actual.tasks.get(0).unwrap().state,
            TaskWorkflowState::Blocked
        );
        assert_eq!(
            actual.tasks.get(0).unwrap().depends_on[0].id.as_deref(),
            Some("target")
        );
    }

    #[test]
    fn candidate_pushdown_preserves_nullable_error_and_short_circuit_order() {
        let mut memory = Workspace::new();
        populate(&mut memory, false);
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let mut persistent = Workspace::with_sqlite_store(store);
        populate(&mut persistent, true);

        for expression in [
            "state == 'done' && priority > 0",
            "priority > 0 && state == 'done'",
        ] {
            let mut filtered = query();
            filtered.filter_groups = vec![TaskQueryFilterGroup {
                filters: vec![TaskQueryFilter {
                    source: expression.to_string(),
                    expression: expression.to_string(),
                }],
            }];
            let memory_error = memory.query_task_page(&filtered).unwrap_err();
            let persistent_error = persistent.query_task_page(&filtered).unwrap_err();
            assert_eq!(memory_error.to_string(), persistent_error.to_string());
            assert!(memory_error.to_string().contains("Done"));
        }

        let leading = compile_filters(&[TaskQueryFilterGroup {
            filters: vec![TaskQueryFilter {
                source: "leading".to_string(),
                expression: "state == 'done' && priority > 0".to_string(),
            }],
        }])
        .unwrap();
        assert!(task_candidate_predicate(&leading).is_some());
        let unsafe_order = compile_filters(&[TaskQueryFilterGroup {
            filters: vec![TaskQueryFilter {
                source: "unsafe".to_string(),
                expression: "priority > 0 && state == 'done'".to_string(),
            }],
        }])
        .unwrap();
        assert!(task_candidate_predicate(&unsafe_order).is_none());
    }

    #[test]
    fn sql_candidates_obey_open_document_precedence() {
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let mut workspace = Workspace::with_sqlite_store(store);
        populate(&mut workspace, true);
        workspace.open_document("c.plumb", 2, "`- Reopened\n\n `+ task\n\n `@ reopened\n");
        let mut filtered = query();
        filtered.filter_groups = vec![TaskQueryFilterGroup {
            filters: vec![TaskQueryFilter {
                source: "done".to_string(),
                expression: "state == 'done'".to_string(),
            }],
        }];

        assert!(workspace
            .query_task_page(&filtered)
            .unwrap()
            .value
            .tasks
            .is_empty());
    }

    #[test]
    fn waiting_state_candidate_matches_memory_at_the_query_instant() {
        let source = concat!(
            "`- Waiting\n\n `+ task\n\n `@ waiting\n\n `= wait 2026-08-29T12:00:00Z\n",
            "`- Ready\n\n `+ task\n\n `@ ready\n\n `= wait 2026-08-27T12:00:00Z\n",
        );
        let mut memory = Workspace::new();
        memory.insert("tasks.plumb", 1, source);
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let mut persistent = Workspace::with_sqlite_store(store);
        persistent.insert_disk("tasks.plumb", 1, source).unwrap();
        let mut filtered = query();
        filtered.filter_groups = vec![TaskQueryFilterGroup {
            filters: vec![TaskQueryFilter {
                source: "waiting".to_string(),
                expression: "state == 'waiting'".to_string(),
            }],
        }];

        let expected = memory.query_task_page(&filtered).unwrap().value;
        let actual = persistent.query_task_page(&filtered).unwrap().value;
        assert_eq!(actual, expected);
        assert_eq!(actual.tasks.len(), 1);
        assert_eq!(
            actual.tasks.get(0).unwrap().task.id.as_ref().unwrap().value,
            "waiting"
        );
    }

    #[test]
    fn nullable_not_equal_candidate_matches_cel_null_semantics() {
        let mut memory = Workspace::new();
        populate(&mut memory, false);
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let mut persistent = Workspace::with_sqlite_store(store);
        populate(&mut persistent, true);
        let mut filtered = query();
        filtered.sort = vec![TaskSortOrder::Source];
        filtered.filter_groups = vec![TaskQueryFilterGroup {
            filters: vec![TaskQueryFilter {
                source: "not-three".to_string(),
                expression: "priority != 3".to_string(),
            }],
        }];

        assert_eq!(
            persistent.query_task_page(&filtered).unwrap().value,
            memory.query_task_page(&filtered).unwrap().value
        );
    }

    const FOCUS_SOURCE: &str = concat!(
        "`- Open focus\n\n `+ task\n\n `@ open-focus\n\n `= focused 2026-09-20T09:00:00+08:00--\n",
        "`- Finished history\n\n `+ task\n\n `@ finished-history\n\n",
        " `= focused 2026-09-20T09:00:00+08:00--2026-09-20T11:00:00+08:00\n",
        "`- Waiting focus\n\n `+ task\n\n `@ waiting-focus\n\n `= wait 2026-08-29T12:00:00Z\n",
        " `= focused 2026-09-20T09:00:00+08:00--\n",
        "`- Blocked focus\n\n `+ task\n\n `@ blocked-focus\n\n `= depends #open-focus\n",
        " `= focused 2026-09-20T09:00:00+08:00--\n",
    );

    fn focus_query(expression: &str) -> TaskPageQuery {
        let mut filtered = query();
        filtered.sort = vec![TaskSortOrder::Source];
        filtered.filter_groups = vec![TaskQueryFilterGroup {
            filters: vec![TaskQueryFilter {
                source: "focus".to_string(),
                expression: expression.to_string(),
            }],
        }];
        filtered
    }

    fn page_ids(page: &TaskPage) -> Vec<&str> {
        page.tasks
            .iter()
            .map(|task| task.task.id.as_ref().unwrap().value.as_str())
            .collect()
    }

    /// A finished-only history is not focused and has no `focused_since`; an
    /// open interval on an open task is focused; waiting and blocked tasks
    /// stay focused at the query instant.
    #[test]
    fn cel_focus_facts_follow_open_and_finished_history() {
        let mut workspace = Workspace::new();
        workspace.insert("tasks.plumb", 1, FOCUS_SOURCE);

        let focused = workspace
            .query_task_page(&focus_query("focused"))
            .unwrap()
            .value;
        assert_eq!(page_ids(&focused), ["open-focus", "waiting-focus", "blocked-focus"]);

        let since = workspace
            .query_task_page(&focus_query("focused_since != null"))
            .unwrap()
            .value;
        assert_eq!(page_ids(&since), page_ids(&focused));

        let waiting = workspace
            .query_task_page(&focus_query("state == 'waiting' && focused"))
            .unwrap()
            .value;
        assert_eq!(page_ids(&waiting), ["waiting-focus"]);

        let blocked = workspace
            .query_task_page(&focus_query("state == 'blocked' && focused"))
            .unwrap()
            .value;
        assert_eq!(page_ids(&blocked), ["blocked-focus"]);

        let unfocused = workspace
            .query_task_page(&focus_query("!focused"))
            .unwrap()
            .value;
        assert_eq!(page_ids(&unfocused), ["finished-history"]);

        let (facts, _) = task_facts(&workspace, None, now().timestamp_millis()).unwrap();
        let finished = facts
            .iter()
            .find(|fact| fact.id.as_deref() == Some("finished-history"))
            .unwrap();
        assert!(!finished.focused);
        assert_eq!(finished.focused_since_millis, None);
        assert!(finished.focus_valid);
        let open = facts
            .iter()
            .find(|fact| fact.id.as_deref() == Some("open-focus"))
            .unwrap();
        assert!(open.focused);
        assert_eq!(
            open.focused_since_millis,
            Some(
                DateTime::parse_from_rfc3339("2026-09-20T09:00:00+08:00")
                    .unwrap()
                    .timestamp_millis()
            )
        );
    }

    /// Invalid focus history parses as `focused == false`, so the validity fact
    /// must stay observable through the fact layer and semantic diagnostics.
    #[test]
    fn invalid_focus_history_is_not_silently_treated_as_unfocused() {
        let source = concat!(
            "`- Open\n\n `+ task\n\n `@ open\n\n `= focused 2026-09-20T09:00:00+08:00--\n",
            "`- Invalid\n\n `+ task\n\n `@ invalid\n\n `= focused nonsense--\n",
        );
        let mut workspace = Workspace::new();
        workspace.insert("tasks.plumb", 1, source);

        let unfocused = workspace.query_task_page(&focus_query("!focused")).unwrap().value;
        assert_eq!(page_ids(&unfocused), ["invalid"]);

        let (facts, _) = task_facts(&workspace, None, now().timestamp_millis()).unwrap();
        let invalid = facts
            .iter()
            .find(|fact| fact.id.as_deref() == Some("invalid"))
            .unwrap();
        assert!(!invalid.focused);
        assert!(!invalid.focus_valid, "invalid history must stay observable");
        let open = facts
            .iter()
            .find(|fact| fact.id.as_deref() == Some("open"))
            .unwrap();
        assert!(open.focus_valid);

        let diagnostics = workspace.diagnostics("tasks.plumb").unwrap().value;
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "task.invalid-focus"),
            "the invalid focus diagnostic must still be published: {diagnostics:?}"
        );
    }

    /// Memory, persistent store, and open-document overlay agree on the focus
    /// derived facts, and the persisted typed columns are used by the CEL
    /// candidate pushdown.
    #[test]
    fn focus_facts_match_memory_persistent_and_open_overlays() {
        let mut memory = Workspace::new();
        memory.insert("tasks.plumb", 1, FOCUS_SOURCE);
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let mut persistent = Workspace::with_sqlite_store(store);
        persistent.insert_disk("tasks.plumb", 1, FOCUS_SOURCE).unwrap();

        for expression in [
            "focused",
            "focused == true",
            "focused == false",
            "focused_since != null",
            "!focused",
        ] {
            let memory_page = memory.query_task_page(&focus_query(expression)).unwrap().value;
            let persistent_page = persistent
                .query_task_page(&focus_query(expression))
                .unwrap()
                .value;
            assert_eq!(
                persistent_page, memory_page,
                "memory and persistent focus facts diverged for {expression}"
            );
        }

        let closed = concat!(
            "`- Closed focus\n\n `+ task\n\n `@ closed-focus\n\n",
            " `= done 2026-09-19T12:00:00Z\n",
            " `= focused 2026-09-20T09:00:00+08:00--\n",
        );
        memory.open_document("tasks.plumb", 2, closed);
        persistent.open_document("tasks.plumb", 2, closed);
        let overlay = focus_query("focused");
        assert_eq!(
            persistent.query_task_page(&overlay).unwrap().value,
            memory.query_task_page(&overlay).unwrap().value
        );
        assert!(memory
            .query_task_page(&overlay)
            .unwrap()
            .value
            .tasks
            .is_empty());
    }

    /// The Tasks page ordering is a fixed shared rule, so cursors issued before
    /// the rule existed must be rejected rather than re-served in a new order.
    #[test]
    fn pre_focus_first_cursors_are_rejected() {
        let mut workspace = Workspace::new();
        workspace.insert("tasks.plumb", 1, FOCUS_SOURCE);
        workspace.insert("other.plumb", 1, FOCUS_SOURCE);
        let mut cursor_query = query();
        cursor_query.sort = vec![TaskSortOrder::Source];
        cursor_query.limit = 1;
        let first = workspace.query_task_page(&cursor_query).unwrap().value;
        let cursor = first.next_cursor.unwrap();
        assert!(cursor.starts_with("v2:"));

        let mut stale = cursor_query.clone();
        stale.cursor = Some(cursor.replacen("v2:", "v1:", 1));
        assert!(matches!(
            workspace.query_task_page(&stale),
            Err(TaskPageQueryError::Cursor(_))
        ));
    }

    fn next_query() -> NextQuery {
        NextQuery {
            root: PathBuf::new(),
            limit: 3,
            cursor: None,
            workspace_revision: 11,
            now: now(),
        }
    }

    fn shortlist_source() -> &'static str {
        concat!(
            "`- Focused newest\n\n `+ task\n\n `@ f-newest\n\n `= focused 2026-09-20T09:00:00Z--\n",
            "`- Focused waiting\n\n `+ task\n\n `@ f-waiting\n\n `= focused 2026-09-19T09:00:00Z--\n `= wait 2099-01-01T00:00:00Z\n",
            "`- Focused oldest\n\n `+ task\n\n `@ f-oldest\n\n `= focused 2026-09-18T09:00:00Z--\n",
            "`- Finished history\n\n `+ task\n\n `@ finished\n `= priority 5\n `= focused 2026-09-01T09:00:00Z--2026-09-01T10:00:00Z\n",
            "`- Ready low\n\n `+ task\n\n `@ ready-low\n `= priority 1\n",
            "`- Ready high\n\n `+ task\n\n `@ ready-high\n `= priority 9\n",
            "`- Invalid focus\n\n `+ task\n\n `@ invalid\n\n `= focused\n\n  `- 2026-09-20T09:00:00Z--\n  `- 2026-09-19T09:00:00Z--\n",
        )
    }

    fn shortlist(workspace: &mut Workspace, persistent: bool) -> NextResult {
        let source = shortlist_source();
        if persistent {
            workspace.insert_disk("focus.plumb", 1, source).unwrap();
        } else {
            workspace.insert("focus.plumb", 1, source);
        }
        workspace.query_next(&next_query()).unwrap().value
    }

    fn ids(tasks: &[WorkspaceTask]) -> Vec<&str> {
        tasks
            .iter()
            .map(|task| {
                task.task
                    .id
                    .as_ref()
                    .expect("fixture tasks have ids")
                    .value
                    .as_str()
            })
            .collect()
    }

    #[test]
    fn next_splits_in_flight_from_candidates_without_touching_state() {
        let mut workspace = Workspace::new();
        let result = shortlist(&mut workspace, false);
        assert_eq!(ids(&result.focused), vec!["f-oldest", "f-waiting", "f-newest"]);
        assert_eq!(result.focused_total, 3);
        assert!(result.focused_complete);
        assert!(result.focused_next_cursor.is_none());
        assert_eq!(result.candidate_limit, 3);
        assert!(result.candidates_complete);
        assert_eq!(ids(&result.candidates), vec!["ready-high", "finished", "ready-low"]);
        // Focus is orthogonal to workflow state: a focused waiting task stays in
        // flight, and a finished history still competes as a candidate.
        let waiting = result
            .focused
            .iter()
            .find(|task| task.task.id.as_ref().unwrap().value == "f-waiting")
            .unwrap();
        assert_eq!(waiting.state, TaskWorkflowState::Waiting);
        // Invalid focus data is reported instead of being treated as unfocused.
        assert_eq!(result.skipped_invalid.len(), 1);
        assert_eq!(result.skipped_invalid[0].path, Path::new("focus.plumb"));
        assert!(!result.skipped_invalid[0].codes.is_empty());
        assert!(result
            .candidates
            .iter()
            .all(|task| task.task.id.as_ref().unwrap().value != "invalid"));
        assert!(result
            .focused
            .iter()
            .all(|task| task.task.id.as_ref().unwrap().value != "invalid"));
    }

    #[test]
    fn next_candidate_limit_is_strict_and_independent_of_in_flight() {
        let mut workspace = Workspace::new();
        workspace.insert("focus.plumb", 1, shortlist_source());
        let mut query = next_query();
        query.limit = 1;
        let result = workspace.query_next(&query).unwrap().value;
        assert_eq!(ids(&result.candidates), vec!["ready-high"]);
        assert!(!result.candidates_complete);
        // The in-flight section is never truncated by the candidate limit.
        assert_eq!(result.focused.len(), 3);
        assert_eq!(result.focused_total, 3);
    }

    #[test]
    fn next_propagates_priority_from_focused_subtasks_before_filtering() {
        let mut workspace = Workspace::new();
        workspace.insert(
            "focus.plumb",
            1,
            concat!(
                "`- Parent candidate\n\n `+ task\n\n `@ parent\n",
                "\n `- Focused child\n\n  `+ task\n\n  `@ child\n  `= priority 50\n  `= focused 2026-09-20T09:00:00Z--\n",
                "`- Other ready\n\n `+ task\n\n `@ other\n `= priority 9\n",
            ),
        );
        let result = workspace.query_next(&next_query()).unwrap().value;
        // A focused subtask is excluded from the candidates itself, but its
        // priority still reaches its parent because propagation runs over the
        // full relation graph before focus filtering.
        assert_eq!(ids(&result.focused), vec!["child"]);
        assert_eq!(ids(&result.candidates), vec!["parent", "other"]);
        assert_eq!(result.candidates[0].effective_priority, 50);
    }

    fn many_focused_source(count: usize) -> String {
        let mut source = String::from("`= title many\n");
        for index in 0..count {
            source.push_str(&format!(
                "\n`- Focused {index}\n\n `+ task\n\n `@ focused-{index}\n\n `= focused 2026-09-{:02}T09:00:00Z--\n",
                index % 28 + 1
            ));
        }
        source
    }

    #[test]
    fn next_in_flight_section_pages_with_an_opaque_cursor() {
        let mut workspace = Workspace::new();
        workspace.insert("many.plumb", 1, many_focused_source(NEXT_FOCUSED_PAGE + 1));
        let first = workspace.query_next(&next_query()).unwrap().value;
        assert_eq!(first.focused_total, NEXT_FOCUSED_PAGE + 1);
        assert_eq!(first.focused.len(), NEXT_FOCUSED_PAGE);
        assert!(!first.focused_complete);
        let cursor = first.focused_next_cursor.clone().expect("continuation cursor");

        let mut paged = next_query();
        paged.cursor = Some(cursor.clone());
        let second = workspace.query_next(&paged).unwrap().value;
        assert_eq!(second.focused.len(), 1);
        assert_eq!(second.focused_total, NEXT_FOCUSED_PAGE + 1);
        assert!(second.focused_complete);
        assert!(second.focused_next_cursor.is_none());

        let mut stale = next_query();
        stale.cursor = Some(cursor);
        stale.workspace_revision = 12;
        assert!(matches!(
            workspace.query_next(&stale),
            Err(TaskPageQueryError::Cursor(_))
        ));
    }

    #[test]
    fn next_shortlist_obeys_open_document_overlay_precedence() {
        let mut memory = Workspace::new();
        memory.insert("focus.plumb", 1, shortlist_source());
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let mut persistent = Workspace::with_sqlite_store(store);
        persistent
            .insert_disk("focus.plumb", 1, shortlist_source())
            .unwrap();

        // The unsaved revision puts a ready candidate into flight.
        let overlay_source = shortlist_source().replace(
            "`- Ready low\n\n `+ task\n\n `@ ready-low\n `= priority 1\n",
            "`- Ready low\n\n `+ task\n\n `@ ready-low\n `= priority 1\n `= focused 2026-09-21T09:00:00Z--\n",
        );
        assert_ne!(overlay_source, shortlist_source());
        memory.open_document("focus.plumb", 2, &overlay_source);
        persistent.open_document("focus.plumb", 2, &overlay_source);

        let in_memory = memory.query_next(&next_query()).unwrap().value;
        let stored = persistent.query_next(&next_query()).unwrap().value;
        assert_eq!(ids(&stored.focused), ids(&in_memory.focused));
        assert_eq!(ids(&stored.candidates), ids(&in_memory.candidates));
        assert_eq!(stored.focused_total, in_memory.focused_total);
        // The overlay wins over the persisted generation.
        assert!(ids(&in_memory.focused).contains(&"ready-low"));
        assert!(!ids(&in_memory.candidates).contains(&"ready-low"));
    }

    #[test]
    fn next_shortlist_matches_memory_and_persistent_store() {
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let mut persistent = Workspace::with_sqlite_store(store);
        let stored = shortlist(&mut persistent, true);
        let mut memory = Workspace::new();
        let in_memory = shortlist(&mut memory, false);
        assert_eq!(ids(&stored.focused), ids(&in_memory.focused));
        assert_eq!(ids(&stored.candidates), ids(&in_memory.candidates));
        assert_eq!(stored.focused_total, in_memory.focused_total);
        assert_eq!(
            stored
                .skipped_invalid
                .iter()
                .map(|skipped| (skipped.path.clone(), skipped.codes.clone()))
                .collect::<Vec<_>>(),
            in_memory
                .skipped_invalid
                .iter()
                .map(|skipped| (skipped.path.clone(), skipped.codes.clone()))
                .collect::<Vec<_>>()
        );
    }
}
