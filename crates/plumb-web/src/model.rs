use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use cel::{Context, Program, Value};
use chrono::{Local, SecondsFormat};
use plumb_semantics::{DocumentOutput, LinkSpelling, TaskRecord, TaskStatus};
use plumb_workspace::{
    apply_document_edit, display_workspace_path as display_path, load_bibliography, normalize,
    scan_workspace_files, search_score, sort_task_records_by, ApplyDocumentEditError,
    DocumentEntry, EventEditError, EventInput, ResolvedTarget, SearchRecordKind,
    SqliteSemanticStore, TaskAuthoringError, TaskAuthoringInput, TaskPageQuery, TaskPageQueryError,
    TaskPlacement, TaskQueryFilter, TaskQueryFilterGroup, TaskRef, TaskSortFacts, TaskSortOrder,
    Workspace, WorkspaceEvent, WorkspaceEventCursor, WorkspaceOperationError, WorkspaceTask,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod mutations;
mod query;

use query::{assign_task_parents, propagate_task_priorities, sort_task_tree, task_source_order};

const DEFAULT_GRAPH_LIMIT: usize = 2_000;
const MAX_GRAPH_LIMIT: usize = 20_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceLocation {
    pub path: String,
    pub start: usize,
    pub end: usize,
}

impl SourceLocation {
    fn new(root: &Path, path: &Path, range: Range<usize>) -> Self {
        Self {
            path: display_path(root, path),
            start: range.start,
            end: range.end,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNode {
    pub id: String,
    pub title: String,
    pub path: Option<String>,
    pub location: Option<SourceLocation>,
    pub unresolved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphEdge {
    pub id: String,
    pub source: String,
    pub target: String,
    pub kind: String,
    pub target_fragment: Option<String>,
    pub location: SourceLocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphSnapshot {
    pub revision: u64,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub complete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum GraphDirection {
    Incoming,
    Outgoing,
    #[default]
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GraphQuery {
    pub current: Option<String>,
    pub depth: Option<usize>,
    #[serde(default)]
    pub direction: GraphDirection,
    #[serde(default)]
    pub kinds: Vec<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteDocument {
    pub id: String,
    pub title: String,
    pub path: String,
    pub revision: i64,
    pub location: SourceLocation,
    pub source: String,
    pub backlinks: Vec<SourceLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WebTaskLocator {
    Id { id: String },
    Offset { offset: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebTask {
    pub key: String,
    pub document_id: String,
    pub title: String,
    pub path: String,
    pub revision: String,
    pub id: Option<String>,
    pub locator: WebTaskLocator,
    pub state: String,
    pub created: Option<String>,
    pub due: Option<String>,
    pub priority: Option<i32>,
    pub effective_priority: i32,
    pub wait: Option<String>,
    pub done: Option<String>,
    pub canceled: Option<String>,
    pub recur: Option<String>,
    pub prev: Option<String>,
    pub prev_on: Option<String>,
    pub depends: Vec<String>,
    pub depends_on: Vec<String>,
    pub directly_blocking: Vec<String>,
    pub blocked: bool,
    pub actionable: bool,
    pub wait_reasons: Vec<String>,
    pub depth: usize,
    pub parent_key: Option<String>,
    pub location: SourceLocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSnapshot {
    pub revision: u64,
    pub tasks: Vec<WebTask>,
    pub documents: Vec<WebTaskDocument>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebTaskDocument {
    pub id: String,
    pub path: String,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebTaskInput {
    pub title: String,
    pub created: Option<String>,
    pub due: Option<String>,
    pub wait: Option<String>,
    pub recur: Option<String>,
    pub prev: Option<WebTaskReferenceInput>,
    #[serde(default)]
    pub depends: Vec<WebTaskReferenceInput>,
    pub priority: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebTaskReferenceInput {
    pub document_id: String,
    pub locator: WebTaskLocator,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WebTaskPlacement {
    pub parent: Option<WebTaskLocator>,
    pub after: Option<WebTaskLocator>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebEventLocator {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebEvent {
    pub key: String,
    pub document_id: String,
    pub path: String,
    pub revision: String,
    pub title: String,
    pub details: String,
    pub id: Option<String>,
    pub date: Option<String>,
    pub timezone: Option<String>,
    pub when: Option<String>,
    pub at: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub tasks: Vec<String>,
    pub depth: usize,
    pub locator: WebEventLocator,
    pub location: SourceLocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventSnapshot {
    pub revision: u64,
    pub events: Vec<WebEvent>,
    pub documents: Vec<WebEventDocument>,
    pub earlier_cursor: Option<String>,
    pub later_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebEventDocument {
    pub id: String,
    pub path: String,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebEventInput {
    pub title: String,
    pub at: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    #[serde(default)]
    pub tasks: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WebView {
    #[default]
    Graph,
    Tasks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum QuerySort {
    #[default]
    Source,
    Priority,
    Due,
    Relevance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub expression: &'static str,
    pub group: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WebQuery {
    #[serde(default)]
    pub view: WebView,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub presets: Vec<String>,
    #[serde(default)]
    pub filter: String,
    #[serde(default)]
    pub filters: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_sort_keys")]
    pub sort: Vec<QuerySort>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
    #[serde(default)]
    pub traversal: GraphQuery,
}

fn deserialize_sort_keys<'de, D>(deserializer: D) -> Result<Vec<QuerySort>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum SortInput {
        One(QuerySort),
        Many(Vec<QuerySort>),
    }
    Ok(match SortInput::deserialize(deserializer)? {
        SortInput::One(sort) => vec![sort],
        SortInput::Many(sorts) => sorts,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryFailure {
    pub source: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskQuerySnapshot {
    pub revision: u64,
    pub tasks: Vec<WebTask>,
    pub all_tasks: Vec<WebTaskCandidate>,
    pub complete: bool,
    pub next_cursor: Option<String>,
    pub documents: Vec<WebTaskDocument>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebTaskCandidate {
    pub key: String,
    pub document_id: String,
    pub title: String,
    pub path: String,
    pub revision: String,
    pub id: Option<String>,
    pub locator: WebTaskLocator,
    pub depth: usize,
    pub parent_key: Option<String>,
}

pub const TASK_PRESETS: &[QueryPreset] = &[
    QueryPreset {
        id: "ready",
        label: "Ready",
        expression: "state == 'ready'",
        group: Some("state"),
    },
    QueryPreset {
        id: "waiting",
        label: "Waiting",
        expression: "state == 'waiting'",
        group: Some("state"),
    },
    QueryPreset {
        id: "blocked",
        label: "Blocked",
        expression: "state == 'blocked'",
        group: Some("state"),
    },
    QueryPreset {
        id: "done",
        label: "Done",
        expression: "state == 'done'",
        group: Some("state"),
    },
    QueryPreset {
        id: "canceled",
        label: "Canceled",
        expression: "state == 'canceled'",
        group: Some("state"),
    },
    QueryPreset {
        id: "conflicted",
        label: "Conflicted",
        expression: "state == 'conflicted'",
        group: Some("state"),
    },
];

pub const GRAPH_PRESETS: &[QueryPreset] = &[
    QueryPreset {
        id: "connected",
        label: "Connected",
        expression: "degree > 0",
        group: Some("connection"),
    },
    QueryPreset {
        id: "orphans",
        label: "Orphans",
        expression: "degree == 0 && !unresolved",
        group: Some("connection"),
    },
    QueryPreset {
        id: "unresolved",
        label: "Unresolved",
        expression: "unresolved",
        group: Some("connection"),
    },
    QueryPreset {
        id: "has-tasks",
        label: "Has tasks",
        expression: "task_count > 0",
        group: None,
    },
    QueryPreset {
        id: "has-open-tasks",
        label: "Has open tasks",
        expression: "open_task_count > 0",
        group: None,
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceRecord {
    pub id: String,
    pub path: PathBuf,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct WebWorkspace {
    root: PathBuf,
    workspace: Workspace,
    index_store: Option<SqliteSemanticStore>,
    documents: BTreeMap<PathBuf, Arc<LazyDocument>>,
    revision: u64,
    document_ids: BTreeMap<PathBuf, String>,
    paths_by_document_id: HashMap<String, PathBuf>,
    titles: HashMap<PathBuf, String>,
    resource_index: Arc<OnceLock<Result<ResourceIndex, String>>>,
}

#[derive(Debug)]
struct LazyDocument {
    revision: i64,
    source: Arc<str>,
    entry: OnceLock<DocumentEntry>,
    #[cfg(test)]
    generation_reused: bool,
}

#[derive(Debug)]
struct ResourceIndex {
    resources: BTreeMap<PathBuf, ResourceRecord>,
    resources_by_id: HashMap<String, PathBuf>,
}

impl WebWorkspace {
    pub fn load(root: impl AsRef<Path>) -> Result<Self, String> {
        Self::load_with_revision(root, 1)
    }

    pub fn load_with_revision(root: impl AsRef<Path>, revision: u64) -> Result<Self, String> {
        let root = normalize(root.as_ref());
        if !root.is_dir() {
            return Err(format!(
                "workspace root is not a directory: {}",
                root.display()
            ));
        }
        let paths = scan_workspace_files(&root).into_result()?;
        let cache_path = web_semantic_cache_path(&root);
        if let Some(parent) = cache_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create Web cache directory: {error}"))?;
        }
        let index_store = SqliteSemanticStore::open(&cache_path)
            .or_else(|_| SqliteSemanticStore::open_in_memory())
            .map_err(|error| format!("cannot open Web semantic cache: {error}"))?;
        let mut index_workspace = Workspace::with_sqlite_store(index_store.clone());
        let batch = index_workspace
            .index_disk_files(
                &paths,
                true,
                |path| file_revision(path).unwrap_or(0),
                || false,
            )
            .map_err(|error| error.to_string())?;
        if !batch.is_complete() {
            return Err(batch
                .failures
                .iter()
                .map(|failure| {
                    format!(
                        "cannot read {}: {}",
                        failure.path.display(),
                        failure.message
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"));
        }
        let mut documents = BTreeMap::new();
        for document in batch.documents {
            #[cfg(not(test))]
            let _ = document.cache_hit;
            documents.insert(
                document.path,
                Arc::new(LazyDocument {
                    revision: document.revision,
                    source: document.source,
                    entry: OnceLock::new(),
                    #[cfg(test)]
                    generation_reused: document.cache_hit,
                }),
            );
        }

        let query_store = index_store
            .readonly_snapshot()
            .map_err(|error| error.to_string())?;
        Self::from_snapshot(
            root,
            Workspace::with_sqlite_store(query_store),
            Some(index_store),
            documents,
            revision,
        )
    }

    pub fn from_workspace(
        root: impl AsRef<Path>,
        workspace: Workspace,
        revision: u64,
    ) -> Result<Self, String> {
        let documents = workspace
            .documents()
            .map(|entry| {
                (
                    entry.path.clone(),
                    Arc::new(LazyDocument {
                        revision: entry.revision,
                        source: Arc::from(entry.parsed.source.as_str()),
                        entry: OnceLock::new(),
                        #[cfg(test)]
                        generation_reused: false,
                    }),
                )
            })
            .collect();
        Self::from_snapshot(root, workspace, None, documents, revision)
    }

    fn from_snapshot(
        root: impl AsRef<Path>,
        workspace: Workspace,
        index_store: Option<SqliteSemanticStore>,
        documents: BTreeMap<PathBuf, Arc<LazyDocument>>,
        revision: u64,
    ) -> Result<Self, String> {
        let root = normalize(root.as_ref());
        if !root.is_dir() {
            return Err(format!(
                "workspace root is not a directory: {}",
                root.display()
            ));
        }

        let note_records = workspace
            .search_records(
                &root,
                Some(SearchRecordKind::Note),
                "",
                usize::MAX,
                Local::now().fixed_offset(),
            )
            .map_err(|error| error.to_string())?
            .value
            .items;
        let valid_paths = note_records
            .iter()
            .filter(|record| documents.contains_key(&record.path))
            .map(|record| record.path.clone())
            .collect::<Vec<_>>();
        let document_ids = valid_paths
            .iter()
            .map(|path| (path.clone(), opaque_id("d", &display_path(&root, path))))
            .collect::<BTreeMap<_, _>>();
        let paths_by_document_id = document_ids
            .iter()
            .map(|(path, id)| (id.clone(), path.clone()))
            .collect();
        let titles = note_records
            .into_iter()
            .filter(|record| documents.contains_key(&record.path))
            .map(|record| (record.path, record.title))
            .collect();

        Ok(Self {
            root,
            workspace,
            index_store,
            documents,
            revision,
            document_ids,
            paths_by_document_id,
            titles,
            resource_index: Arc::new(OnceLock::new()),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn document_id(&self, path: impl AsRef<Path>) -> Option<&str> {
        self.document_ids
            .get(&normalize(path.as_ref()))
            .map(String::as_str)
    }

    pub fn document_path(&self, id: &str) -> Option<&Path> {
        self.paths_by_document_id.get(id).map(PathBuf::as_path)
    }

    pub fn resource(&self, id: &str) -> Result<Option<&ResourceRecord>, String> {
        let index = self.resource_index()?;
        let Some(path) = index.resources_by_id.get(id) else {
            return Ok(None);
        };
        Ok(index.resources.get(path))
    }

    pub fn resource_for_path(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<Option<&ResourceRecord>, String> {
        Ok(self
            .resource_index()?
            .resources
            .get(&normalize(path.as_ref())))
    }

    pub fn refresh_document(
        &mut self,
        path: impl AsRef<Path>,
        revision: u64,
    ) -> Result<(), String> {
        let path = normalize(path.as_ref());
        if !self.document_ids.contains_key(&path) {
            return Err(format!("document is not indexed: {}", path.display()));
        }
        let source = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let file_revision = file_revision(&path).unwrap_or(0);
        let index_store = self
            .index_store
            .as_ref()
            .ok_or_else(|| "Web workspace has no persistent index".to_string())?;
        let mut index_workspace = Workspace::with_sqlite_store(index_store.clone());
        index_workspace
            .insert_disk(&path, file_revision, source.clone())
            .map_err(|error| format!("cannot index {}: {error}", path.display()))?;
        let entry = Workspace::materialize_document(&path, file_revision, &source);
        if entry.current.is_none() {
            return Err(format!("updated document is invalid: {}", path.display()));
        }
        let lazy = LazyDocument {
            revision: file_revision,
            source: Arc::from(source),
            entry: OnceLock::new(),
            #[cfg(test)]
            generation_reused: false,
        };
        lazy.entry
            .set(entry)
            .expect("new lazy document entry is empty");
        self.documents.insert(path.clone(), Arc::new(lazy));
        self.workspace = Workspace::with_sqlite_store(
            index_store
                .readonly_snapshot()
                .map_err(|error| error.to_string())?,
        );
        self.revision = revision;
        self.titles = self
            .workspace
            .search_records(
                &self.root,
                Some(SearchRecordKind::Note),
                "",
                usize::MAX,
                Local::now().fixed_offset(),
            )
            .map_err(|error| error.to_string())?
            .value
            .items
            .into_iter()
            .map(|record| (record.path, record.title))
            .collect();
        self.resource_index = Arc::new(OnceLock::new());
        Ok(())
    }

    pub fn document_source_matches_disk(&self, path: impl AsRef<Path>) -> bool {
        let path = normalize(path.as_ref());
        let Some(document) = self.documents.get(&path) else {
            return false;
        };
        std::fs::read_to_string(path).is_ok_and(|source| source == document.source.as_ref())
    }

    pub fn resources(&self) -> Result<impl Iterator<Item = &ResourceRecord>, String> {
        Ok(self.resource_index()?.resources.values())
    }

    pub fn tasks(&self) -> Result<TaskSnapshot, String> {
        self.task_snapshot(None, true)
    }

    fn task_snapshot(
        &self,
        retained: Option<&BTreeSet<(String, usize)>>,
        include_relations: bool,
    ) -> Result<TaskSnapshot, String> {
        let now = Local::now().fixed_offset();
        let records = self
            .workspace
            .search_records(
                &self.root,
                Some(SearchRecordKind::Task),
                "",
                usize::MAX,
                now,
            )
            .map_err(|error| error.to_string())?
            .value;
        let mut tasks = Vec::new();
        for record in records.items {
            if retained.is_some_and(|retained| {
                !retained.contains(&(record.relative_path.clone(), record.range.start))
            }) {
                continue;
            }
            let Some(document_id) = self.document_id(&record.path).map(str::to_string) else {
                continue;
            };
            let Some(state) = record.task_state.map(|state| state.as_str()) else {
                continue;
            };
            let Some(entry) = self.document_entry(&record.path)? else {
                continue;
            };
            let Some(current) = entry.current.as_ref() else {
                continue;
            };
            let Some(task) = current
                .output
                .tasks
                .tasks
                .iter()
                .find(|task| task.selection_range == record.range)
            else {
                continue;
            };
            let key = record.id.as_ref().map_or_else(
                || format!("{document_id}:{}", task.range.start),
                |id| format!("{document_id}:{id}"),
            );
            let locator = record.id.as_ref().map_or_else(
                || WebTaskLocator::Offset {
                    offset: task.range.start,
                },
                |id| WebTaskLocator::Id { id: id.clone() },
            );
            let depends_on = if include_relations {
                self.workspace
                    .task_dependencies(&record.path, task)
                    .map_err(|error| error.to_string())?
                    .value
                    .into_iter()
                    .map(|dependency| display_task_ref(&self.root, &dependency.target))
                    .collect()
            } else {
                Vec::new()
            };
            let directly_blocking = if include_relations {
                match record.id.as_deref() {
                    Some(id) => self
                        .workspace
                        .directly_blocking_tasks(&record.path, id)
                        .map_err(|error| error.to_string())?
                        .value
                        .into_iter()
                        .map(|target| display_task_ref(&self.root, &target))
                        .collect(),
                    None => Vec::new(),
                }
            } else {
                Vec::new()
            };
            let prev_on = if include_relations {
                self.workspace
                    .task_previous(&record.path, task)
                    .map_err(|error| error.to_string())?
                    .value
                    .map(|target| display_task_ref(&self.root, &target))
            } else {
                None
            };
            tasks.push(WebTask {
                key,
                document_id,
                title: record.title,
                path: record.relative_path,
                revision: current.revision.to_string(),
                id: record.id,
                locator,
                state: state.to_string(),
                created: task.created.as_ref().map(|field| field.value.clone()),
                due: task.due.as_ref().map(|field| field.value.clone()),
                priority: task.priority,
                effective_priority: record.effective_priority.unwrap_or_default(),
                wait: task.wait.as_ref().map(|field| field.value.clone()),
                done: task.done.as_ref().map(|field| field.value.clone()),
                canceled: task.canceled.as_ref().map(|field| field.value.clone()),
                recur: task.recur.as_ref().map(|field| field.value.clone()),
                prev: task.prev.as_ref().map(|field| field.value.clone()),
                prev_on,
                depends: task
                    .depends
                    .iter()
                    .map(|item| item.source.clone())
                    .collect(),
                depends_on,
                directly_blocking,
                blocked: record.blocked.unwrap_or(false),
                actionable: record.actionable.unwrap_or(false),
                wait_reasons: record
                    .wait_reasons
                    .unwrap_or_default()
                    .into_iter()
                    .map(|reason| reason.as_str().to_string())
                    .collect(),
                depth: record.depth.unwrap_or_default(),
                parent_key: None,
                location: SourceLocation::new(&self.root, &record.path, record.range),
            });
        }
        tasks.sort_by(task_source_order);
        assign_task_parents(&mut tasks);
        if include_relations {
            propagate_task_priorities(&mut tasks);
        }
        sort_task_tree(
            &mut tasks,
            &[QuerySort::Priority, QuerySort::Due],
            &HashMap::new(),
        );
        Ok(TaskSnapshot {
            revision: self.revision,
            tasks,
            documents: self.task_documents()?,
        })
    }

    pub fn task_candidates(
        &self,
        query: &str,
        requested_document: Option<&str>,
        limit: usize,
    ) -> Result<Vec<WebTaskCandidate>, String> {
        let records = self
            .workspace
            .search_records(
                &self.root,
                Some(SearchRecordKind::Task),
                query,
                requested_document.map_or(limit, |_| usize::MAX),
                Local::now().fixed_offset(),
            )
            .map_err(|error| error.to_string())?
            .value
            .items;
        let mut candidates = Vec::new();
        for record in records {
            let Some(document_id) = self.document_id(&record.path).map(str::to_string) else {
                continue;
            };
            if requested_document.is_some_and(|requested| requested != document_id) {
                continue;
            }
            let Some(entry) = self.document_entry(&record.path)? else {
                continue;
            };
            let Some(current) = entry.current.as_ref() else {
                continue;
            };
            let Some(task) = current
                .output
                .tasks
                .tasks
                .iter()
                .find(|task| task.selection_range == record.range)
            else {
                continue;
            };
            let key = record.id.as_ref().map_or_else(
                || format!("{document_id}:{}", task.range.start),
                |id| format!("{document_id}:{id}"),
            );
            let locator = record.id.as_ref().map_or_else(
                || WebTaskLocator::Offset {
                    offset: task.range.start,
                },
                |id| WebTaskLocator::Id { id: id.clone() },
            );
            candidates.push(WebTaskCandidate {
                key,
                document_id,
                title: record.title,
                path: record.relative_path,
                revision: current.revision.to_string(),
                id: record.id,
                locator,
                depth: record.depth.unwrap_or_default(),
                parent_key: None,
            });
        }
        assign_candidate_parents(&mut candidates);
        candidates.truncate(limit);
        Ok(candidates)
    }

    fn task_documents(&self) -> Result<Vec<WebTaskDocument>, String> {
        let mut documents = Vec::new();
        for (path, id) in &self.document_ids {
            let Some(document) = self.documents.get(path) else {
                continue;
            };
            documents.push(WebTaskDocument {
                id: id.clone(),
                path: display_path(&self.root, path),
                revision: document.revision.to_string(),
            });
        }
        Ok(documents)
    }

    pub fn events(&self) -> Result<EventSnapshot, String> {
        let mut events = Vec::new();
        for path in self.document_ids.keys() {
            let Some(entry) = self.document_entry(path)? else {
                continue;
            };
            let Some(current) = &entry.current else {
                continue;
            };
            let Some(document_id) = self.document_id(&entry.path).map(str::to_string) else {
                continue;
            };
            for event in &current.output.events.events {
                events.push(WebEvent {
                    key: format!("{document_id}:{}", event.range.start),
                    document_id: document_id.clone(),
                    path: display_path(&self.root, &entry.path),
                    revision: current.revision.to_string(),
                    title: event.title.clone(),
                    details: event.details.clone(),
                    id: event.id.as_ref().map(|field| field.value.clone()),
                    date: event.date.as_ref().map(|field| field.value.clone()),
                    timezone: event.timezone.as_ref().map(|field| field.value.clone()),
                    when: event.when.as_ref().map(|field| field.value.clone()),
                    at: event.at.as_ref().map(|field| field.value.clone()),
                    start: event.start.as_ref().map(|field| field.value.clone()),
                    end: event.end.as_ref().map(|field| field.value.clone()),
                    tasks: self
                        .workspace
                        .event_task_references(&entry.path, event)
                        .map_err(|error| error.to_string())?
                        .value
                        .into_iter()
                        .map(|reference| reference.source.clone())
                        .collect(),
                    depth: event.depth,
                    locator: WebEventLocator {
                        start: event.range.start,
                        end: event.range.end,
                    },
                    location: SourceLocation::new(
                        &self.root,
                        &entry.path,
                        event.selection_range.clone(),
                    ),
                });
            }
        }
        events.sort_by(|left, right| {
            left.at
                .as_deref()
                .or(left.start.as_deref())
                .and_then(|time| chrono::DateTime::parse_from_rfc3339(time).ok())
                .cmp(
                    &right
                        .at
                        .as_deref()
                        .or(right.start.as_deref())
                        .and_then(|time| chrono::DateTime::parse_from_rfc3339(time).ok()),
                )
                .then(left.path.cmp(&right.path))
                .then(left.locator.start.cmp(&right.locator.start))
        });
        Ok(EventSnapshot {
            revision: self.revision,
            events,
            documents: self.event_documents()?,
            earlier_cursor: None,
            later_cursor: None,
        })
    }

    pub fn event_page(
        &self,
        cursor: Option<&str>,
        direction: Option<&str>,
    ) -> Result<EventSnapshot, String> {
        const PAGE_SIZE: usize = 240;
        let boundary = match cursor {
            Some(cursor) => Some(self.decode_event_cursor(cursor)?),
            None => None,
        };
        let (records, has_earlier, has_later) = match (boundary.as_ref(), direction) {
            (Some(boundary), Some("earlier")) => {
                let records = self
                    .workspace
                    .events_page_before(boundary, PAGE_SIZE + 1)
                    .map_err(|error| error.to_string())?
                    .value;
                let has_earlier = records.len() > PAGE_SIZE;
                let records = if has_earlier {
                    records.into_iter().skip(1).collect()
                } else {
                    records
                };
                let has_later = !records.is_empty();
                (records, has_earlier, has_later)
            }
            (Some(boundary), Some("later")) => {
                let mut records = self
                    .workspace
                    .events_page_after(Some(boundary), PAGE_SIZE + 1)
                    .map_err(|error| error.to_string())?
                    .value;
                let has_later = records.len() > PAGE_SIZE;
                if has_later {
                    records.pop();
                }
                let has_earlier = !records.is_empty();
                (records, has_earlier, has_later)
            }
            (Some(_), _) => return Err("event cursor requires earlier or later direction".into()),
            (None, Some(_)) => return Err("event page direction requires a cursor".into()),
            (None, None) => {
                let now = WorkspaceEventCursor {
                    sort_millis: Some(Local::now().timestamp_millis()),
                    path: PathBuf::new(),
                    start: 0,
                };
                let mut earlier = self
                    .workspace
                    .events_page_before(&now, PAGE_SIZE / 2 + 1)
                    .map_err(|error| error.to_string())?
                    .value;
                let mut later = self
                    .workspace
                    .events_page_after(Some(&now), PAGE_SIZE / 2 + 1)
                    .map_err(|error| error.to_string())?
                    .value;
                let has_earlier = earlier.len() > PAGE_SIZE / 2;
                let has_later = later.len() > PAGE_SIZE / 2;
                if has_earlier {
                    earlier.remove(0);
                }
                if has_later {
                    later.pop();
                }
                earlier.extend(later);
                (earlier, has_earlier, has_later)
            }
        };
        let events = records
            .iter()
            .map(|record| self.web_event(record))
            .collect::<Result<Vec<_>, String>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let earlier_cursor = records
            .first()
            .map(|record| self.encode_event_cursor(record));
        let later_cursor = records
            .last()
            .map(|record| self.encode_event_cursor(record));
        Ok(EventSnapshot {
            revision: self.revision,
            events,
            documents: self.event_documents()?,
            earlier_cursor: earlier_cursor.filter(|_| has_earlier),
            later_cursor: later_cursor.filter(|_| has_later),
        })
    }

    pub fn event_page_for_selection(&self, selected: &str) -> Result<EventSnapshot, String> {
        const BEFORE: usize = 119;
        const AFTER: usize = 120;
        let (document, start) = selected
            .rsplit_once(':')
            .and_then(|(document, start)| {
                start.parse::<usize>().ok().map(|start| (document, start))
            })
            .ok_or("selected event identity is malformed")?;
        let path = self
            .document_path(document)
            .ok_or("selected event document is unavailable")?;
        let entry = self
            .document_entry(path)?
            .and_then(|entry| entry.current.as_ref())
            .ok_or("selected event document is invalid")?;
        let event = entry
            .output
            .events
            .events
            .iter()
            .find(|event| event.range.start == start)
            .cloned()
            .ok_or("selected event is unavailable")?;
        let boundary = WorkspaceEventCursor {
            sort_millis: event.sort_datetime().map(|value| value.timestamp_millis()),
            path: path.to_path_buf(),
            start,
        };
        let mut earlier = self
            .workspace
            .events_page_before(&boundary, BEFORE + 1)
            .map_err(|error| error.to_string())?
            .value;
        let mut later = self
            .workspace
            .events_page_after(Some(&boundary), AFTER + 1)
            .map_err(|error| error.to_string())?
            .value;
        let has_earlier = earlier.len() > BEFORE;
        let has_later = later.len() > AFTER;
        if has_earlier {
            earlier.remove(0);
        }
        if has_later {
            later.pop();
        }
        earlier.push(WorkspaceEvent {
            path: path.to_path_buf(),
            revision: entry.revision,
            event,
        });
        earlier.extend(later);
        let earlier_cursor = earlier
            .first()
            .map(|record| self.encode_event_cursor(record))
            .filter(|_| has_earlier);
        let later_cursor = earlier
            .last()
            .map(|record| self.encode_event_cursor(record))
            .filter(|_| has_later);
        Ok(EventSnapshot {
            revision: self.revision,
            events: earlier
                .iter()
                .map(|record| self.web_event(record))
                .collect::<Result<Vec<_>, String>>()?
                .into_iter()
                .flatten()
                .collect(),
            documents: self.event_documents()?,
            earlier_cursor,
            later_cursor,
        })
    }

    pub fn event_key_after_mutation(
        &self,
        document_id: &str,
        locator: Option<&WebEventLocator>,
        input: Option<&WebEventInput>,
    ) -> Option<String> {
        let path = self.document_path(document_id)?;
        let current = self.document_entry(path).ok()??.current.as_ref()?;
        let event = locator
            .and_then(|locator| {
                current
                    .output
                    .events
                    .events
                    .iter()
                    .find(|event| event.range.start == locator.start)
            })
            .or_else(|| {
                input.and_then(|input| {
                    current
                        .output
                        .events
                        .events
                        .iter()
                        .rev()
                        .find(|event| event.title == input.title)
                })
            })?;
        Some(format!("{document_id}:{}", event.range.start))
    }

    fn event_documents(&self) -> Result<Vec<WebEventDocument>, String> {
        let mut documents = Vec::new();
        for (path, id) in &self.document_ids {
            let Some(entry) = self.document_entry(path)? else {
                continue;
            };
            if entry.current.is_some() {
                documents.push(WebEventDocument {
                    id: id.clone(),
                    path: display_path(&self.root, path),
                    revision: entry.revision.to_string(),
                });
            }
        }
        Ok(documents)
    }

    fn web_event(&self, record: &WorkspaceEvent) -> Result<Option<WebEvent>, String> {
        let event = &record.event;
        let Some(document_id) = self.document_id(&record.path).map(str::to_string) else {
            return Ok(None);
        };
        Ok(Some(WebEvent {
            key: format!("{document_id}:{}", event.range.start),
            document_id,
            path: display_path(&self.root, &record.path),
            revision: record.revision.to_string(),
            title: event.title.clone(),
            details: event.details.clone(),
            id: event.id.as_ref().map(|field| field.value.clone()),
            date: event.date.as_ref().map(|field| field.value.clone()),
            timezone: event.timezone.as_ref().map(|field| field.value.clone()),
            when: event.when.as_ref().map(|field| field.value.clone()),
            at: event.at.as_ref().map(|field| field.value.clone()),
            start: event.start.as_ref().map(|field| field.value.clone()),
            end: event.end.as_ref().map(|field| field.value.clone()),
            tasks: self
                .workspace
                .event_task_references(&record.path, event)
                .map_err(|error| error.to_string())?
                .value
                .into_iter()
                .map(|reference| reference.source.clone())
                .collect(),
            depth: event.depth,
            locator: WebEventLocator {
                start: event.range.start,
                end: event.range.end,
            },
            location: SourceLocation::new(&self.root, &record.path, event.selection_range.clone()),
        }))
    }

    fn encode_event_cursor(&self, record: &WorkspaceEvent) -> String {
        let millis = record
            .event
            .sort_datetime()
            .map(|value| value.timestamp_millis())
            .map_or_else(|| "n".into(), |value| value.to_string());
        let document = self
            .document_id(&record.path)
            .expect("event document has an id");
        format!(
            "{}:{millis}:{document}:{}",
            self.revision, record.event.range.start
        )
    }

    fn decode_event_cursor(&self, cursor: &str) -> Result<WorkspaceEventCursor, String> {
        let mut parts = cursor.splitn(4, ':');
        let revision = parts.next().and_then(|value| value.parse::<u64>().ok());
        let millis = parts.next();
        let document = parts.next();
        let start = parts.next().and_then(|value| value.parse::<usize>().ok());
        if revision != Some(self.revision)
            || millis.is_none()
            || document.is_none()
            || start.is_none()
        {
            return Err("event cursor is stale or malformed".into());
        }
        let sort_millis = match millis.unwrap() {
            "n" => None,
            value => Some(value.parse().map_err(|_| "event cursor is malformed")?),
        };
        let path = self
            .document_path(document.unwrap())
            .ok_or("event cursor document is unavailable")?
            .to_path_buf();
        Ok(WorkspaceEventCursor {
            sort_millis,
            path,
            start: start.unwrap(),
        })
    }

    pub fn has_same_documents(&self, other: &Self) -> bool {
        self.documents.len() == other.documents.len()
            && self.documents.iter().all(|(path, document)| {
                other
                    .documents
                    .get(path)
                    .is_some_and(|other| document.source == other.source)
            })
    }

    pub fn note(&self, id: &str) -> Result<Option<NoteDocument>, String> {
        let Some(path) = self.document_path(id) else {
            return Ok(None);
        };
        let Some(entry) = self.document_entry(path)? else {
            return Ok(None);
        };
        let Some(current) = entry.current.as_ref() else {
            return Ok(None);
        };
        let backlinks = self
            .workspace
            .references_to_document(path)
            .map_err(|error| error.to_string())?
            .value
            .into_iter()
            .map(|(source, reference)| {
                SourceLocation::new(&self.root, &source, reference.source_range)
            })
            .collect();
        Ok(Some(NoteDocument {
            id: id.to_string(),
            title: self.title(path),
            path: display_path(&self.root, path),
            revision: current.revision,
            location: SourceLocation::new(&self.root, path, 0..entry.parsed.source.len()),
            source: entry.parsed.source.clone(),
            backlinks,
        }))
    }

    pub fn graph(&self, query: &GraphQuery) -> Result<GraphSnapshot, String> {
        self.graph_with_excluded(query, &BTreeSet::new(), true)
    }

    pub fn graph_excluding(
        &self,
        query: &GraphQuery,
        predicate: Option<&str>,
    ) -> Result<GraphSnapshot, String> {
        let excluded = self.excluded_documents(predicate)?;
        self.graph_with_excluded(query, &excluded, true)
    }

    fn excluded_documents(&self, predicate: Option<&str>) -> Result<BTreeSet<String>, String> {
        Ok(match predicate {
            Some(predicate) => self
                .workspace
                .search_records_filtered(
                    &self.root,
                    Some(SearchRecordKind::Note),
                    "",
                    usize::MAX,
                    Local::now().fixed_offset(),
                    Some(predicate),
                )
                .map_err(|error| error.to_string())?
                .value
                .items
                .into_iter()
                .filter_map(|record| self.document_ids.get(&record.path).cloned())
                .collect(),
            None => BTreeSet::new(),
        })
    }

    fn graph_with_excluded(
        &self,
        query: &GraphQuery,
        excluded: &BTreeSet<String>,
        apply_limit: bool,
    ) -> Result<GraphSnapshot, String> {
        let (mut nodes, mut edges) = self.full_graph()?;
        nodes.retain(|id, _| !excluded.contains(id));
        edges.retain(|edge| nodes.contains_key(&edge.source) && nodes.contains_key(&edge.target));
        let connected = edges
            .iter()
            .flat_map(|edge| [&edge.source, &edge.target])
            .collect::<BTreeSet<_>>();
        nodes.retain(|id, node| !node.unresolved || connected.contains(id));
        if !query.kinds.is_empty() {
            let kinds = query
                .kinds
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            edges.retain(|edge| kinds.contains(edge.kind.as_str()));
        }
        if let Some(current) = query.current.as_ref().filter(|id| nodes.contains_key(*id)) {
            let depth = query.depth.unwrap_or(1).min(32);
            let mut included = BTreeSet::from([current.clone()]);
            let mut queue = VecDeque::from([(current.clone(), 0usize)]);
            while let Some((node, distance)) = queue.pop_front() {
                if distance >= depth {
                    continue;
                }
                for edge in &edges {
                    let neighbor = match query.direction {
                        GraphDirection::Outgoing if edge.source == node => Some(&edge.target),
                        GraphDirection::Incoming if edge.target == node => Some(&edge.source),
                        GraphDirection::Both if edge.source == node => Some(&edge.target),
                        GraphDirection::Both if edge.target == node => Some(&edge.source),
                        _ => None,
                    };
                    if let Some(neighbor) = neighbor {
                        if included.insert(neighbor.clone()) {
                            queue.push_back((neighbor.clone(), distance + 1));
                        }
                    }
                }
            }
            nodes.retain(|id, _| included.contains(id));
            edges.retain(|edge| included.contains(&edge.source) && included.contains(&edge.target));
        }

        let complete = !apply_limit
            || nodes.len()
                <= query
                    .limit
                    .unwrap_or(DEFAULT_GRAPH_LIMIT)
                    .min(MAX_GRAPH_LIMIT);
        if apply_limit {
            let limit = query
                .limit
                .unwrap_or(DEFAULT_GRAPH_LIMIT)
                .min(MAX_GRAPH_LIMIT);
            let retained = nodes.keys().take(limit).cloned().collect::<BTreeSet<_>>();
            nodes.retain(|id, _| retained.contains(id));
            edges.retain(|edge| retained.contains(&edge.source) && retained.contains(&edge.target));
        }
        Ok(GraphSnapshot {
            revision: self.revision,
            nodes: nodes.into_values().collect(),
            edges,
            complete,
        })
    }

    pub fn pandoc_document(&self, id: &str) -> Result<serde_json::Value, String> {
        let note = self
            .note(id)?
            .ok_or_else(|| format!("unknown document id '{id}'"))?;
        plumb_export::export(&note.source)
    }

    pub fn bibliography(&self, id: &str) -> Result<plumb_workspace::Bibliography, String> {
        let path = self
            .document_path(id)
            .ok_or_else(|| format!("unknown document id '{id}'"))?;
        let metadata = &self
            .document_entry(path)?
            .and_then(|entry| entry.current.as_ref())
            .ok_or_else(|| format!("document '{}' is not semantically valid", path.display()))?
            .output
            .metadata;
        Ok(load_bibliography(&self.root, path, metadata))
    }

    fn full_graph(&self) -> Result<(BTreeMap<String, GraphNode>, Vec<GraphEdge>), String> {
        let mut nodes = self
            .document_ids
            .iter()
            .map(|(path, id)| {
                let source_len = self
                    .documents
                    .get(path)
                    .expect("indexed document has source bytes")
                    .source
                    .len();
                (
                    id.clone(),
                    GraphNode {
                        id: id.clone(),
                        title: self.title(path),
                        path: Some(display_path(&self.root, path)),
                        location: Some(SourceLocation::new(&self.root, path, 0..source_len)),
                        unresolved: false,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut edges = Vec::new();
        let mut ghost_ids = BTreeMap::<String, String>::new();
        for (path, source_id) in &self.document_ids {
            let entry = self
                .document_entry(path)?
                .ok_or_else(|| format!("indexed document is unavailable: {}", path.display()))?;
            let current = entry
                .current
                .as_ref()
                .expect("document id is current-valid");
            let operation_workspace = self.operation_workspace(path)?;
            for link in &current.output.links {
                let kind = match link.spelling {
                    LinkSpelling::Positional => "link",
                    LinkSpelling::Verbatim { .. } => "autolink",
                };
                self.push_resolved_edge(
                    &mut nodes,
                    &mut ghost_ids,
                    &mut edges,
                    path,
                    source_id,
                    kind,
                    link.target.value.as_str(),
                    link.selection_range.clone(),
                    operation_workspace
                        .resolve_link(path, link)
                        .map_err(|error| error.to_string())?
                        .value,
                );
            }
            for task in &current.output.tasks.tasks {
                if let Some(prev) = &task.prev {
                    self.push_resolved_edge(
                        &mut nodes,
                        &mut ghost_ids,
                        &mut edges,
                        path,
                        source_id,
                        "task-prev",
                        &prev.value,
                        prev.range.clone(),
                        operation_workspace
                            .resolve_task_reference_at(path, prev.range.start)
                            .map_err(|error| error.to_string())?
                            .value
                            .unwrap_or(ResolvedTarget::Other),
                    );
                }
                for dependency in &task.depends {
                    self.push_resolved_edge(
                        &mut nodes,
                        &mut ghost_ids,
                        &mut edges,
                        path,
                        source_id,
                        "task-depends",
                        &dependency.source,
                        dependency.range.clone(),
                        operation_workspace
                            .resolve_task_reference_at(path, dependency.range.start)
                            .map_err(|error| error.to_string())?
                            .value
                            .unwrap_or(ResolvedTarget::Other),
                    );
                }
            }
        }
        Ok((nodes, edges))
    }

    #[allow(clippy::too_many_arguments)]
    fn push_resolved_edge(
        &self,
        nodes: &mut BTreeMap<String, GraphNode>,
        ghost_ids: &mut BTreeMap<String, String>,
        edges: &mut Vec<GraphEdge>,
        source_path: &Path,
        source_id: &str,
        kind: &str,
        raw_target: &str,
        range: Range<usize>,
        resolved: ResolvedTarget,
    ) {
        let (target_path, fragment, unresolved) = match resolved {
            ResolvedTarget::Anchor { path, id, .. } => (Some(path), Some(id), false),
            ResolvedTarget::Document { path } => (Some(path), None, false),
            ResolvedTarget::UnresolvedAnchor { path, id }
            | ResolvedTarget::AmbiguousAnchor { path, id } => (Some(path), Some(id), true),
            ResolvedTarget::UnresolvedPath { path } => (Some(path), None, true),
            ResolvedTarget::External
            | ResolvedTarget::File { .. }
            | ResolvedTarget::UnresolvedFile { .. }
            | ResolvedTarget::Other => return,
        };
        let target_id = target_path
            .as_ref()
            .and_then(|path| self.document_ids.get(path).cloned())
            .unwrap_or_else(|| {
                let key = target_path
                    .as_ref()
                    .map(|path| display_path(&self.root, path))
                    .unwrap_or_else(|| raw_target.to_string());
                let next_id = format!("u{:06}", ghost_ids.len() + 1);
                ghost_ids.entry(key.clone()).or_insert(next_id).clone()
            });
        if target_id == source_id {
            return;
        }
        if unresolved && !nodes.contains_key(&target_id) {
            nodes.insert(
                target_id.clone(),
                GraphNode {
                    id: target_id.clone(),
                    title: raw_target.to_string(),
                    path: target_path
                        .as_ref()
                        .map(|path| display_path(&self.root, path)),
                    location: None,
                    unresolved: true,
                },
            );
        }
        edges.push(GraphEdge {
            id: format!("e{:06}", edges.len() + 1),
            source: source_id.to_string(),
            target: target_id,
            kind: kind.to_string(),
            target_fragment: fragment,
            location: SourceLocation::new(&self.root, source_path, range),
        });
    }

    fn title(&self, path: &Path) -> String {
        self.titles.get(path).cloned().unwrap_or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("Untitled")
                .to_string()
        })
    }

    fn document_entry(&self, path: &Path) -> Result<Option<&DocumentEntry>, String> {
        let path = normalize(path);
        if let Some(entry) = self.workspace.get(&path) {
            return Ok(Some(entry));
        }
        let Some(document) = self.documents.get(&path) else {
            return Ok(None);
        };
        if document.entry.get().is_none() {
            let entry = Workspace::materialize_document(&path, document.revision, &document.source);
            let _ = document.entry.set(entry);
        }
        Ok(document.entry.get())
    }

    fn operation_workspace(&self, path: &Path) -> Result<Workspace, String> {
        let entry = self
            .document_entry(path)?
            .ok_or_else(|| format!("document is no longer indexed: {}", path.display()))?;
        let mut workspace = self.workspace.clone();
        workspace.open_document(&entry.path, entry.revision, entry.parsed.source.clone());
        Ok(workspace)
    }

    fn task_for_locator<'a>(
        &self,
        output: &'a DocumentOutput,
        locator: &WebTaskLocator,
    ) -> Option<&'a TaskRecord> {
        output.tasks.tasks.iter().find(|task| match locator {
            WebTaskLocator::Id { id } => task.id.as_ref().is_some_and(|field| field.value == *id),
            WebTaskLocator::Offset { offset } => task.range.start == *offset,
        })
    }

    fn resource_index(&self) -> Result<&ResourceIndex, String> {
        self.resource_index
            .get_or_init(|| self.build_resource_index())
            .as_ref()
            .map_err(Clone::clone)
    }

    fn build_resource_index(&self) -> Result<ResourceIndex, String> {
        let mut paths = BTreeSet::new();
        let mut resources = BTreeMap::new();
        let mut resources_by_id = HashMap::new();
        let canonical_root = self
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone());
        for path in self.document_ids.keys() {
            let Some(entry) = self.document_entry(path)? else {
                continue;
            };
            let Some(current) = &entry.current else {
                continue;
            };
            for link in &current.output.links {
                if let ResolvedTarget::File { path } = self
                    .workspace
                    .resolve_link(&entry.path, link)
                    .map_err(|error| error.to_string())?
                    .value
                {
                    paths.insert(path);
                }
            }
            for image in &current.output.images {
                if let ResolvedTarget::File { path } =
                    self.workspace.resolve_image(&entry.path, image)
                {
                    paths.insert(path);
                }
            }
            for file in &current.output.files {
                if let ResolvedTarget::File { path } =
                    self.workspace.resolve_file(&entry.path, file)
                {
                    paths.insert(path);
                }
            }
        }
        for path in paths {
            let Ok(canonical) = path.canonicalize() else {
                continue;
            };
            if !canonical.starts_with(&canonical_root) || !canonical.is_file() {
                continue;
            }
            let id = opaque_id("r", &display_path(&self.root, &canonical));
            let record = ResourceRecord {
                id: id.clone(),
                name: canonical
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("resource")
                    .to_string(),
                path: canonical.clone(),
            };
            resources_by_id.insert(id, canonical.clone());
            resources.insert(canonical, record);
        }
        Ok(ResourceIndex {
            resources,
            resources_by_id,
        })
    }
}

fn file_revision(path: &Path) -> Option<i64> {
    let metadata = path.metadata().ok()?;
    let modified = metadata.modified().ok()?;
    let duration = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(duration.as_nanos().min(i64::MAX as u128) as i64)
}

fn web_semantic_cache_path(root: &Path) -> PathBuf {
    let base = std::env::var_os("PLUMB_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(|| std::env::temp_dir().join("plumb-cache"));
    web_semantic_cache_path_in(&base, env!("CARGO_PKG_VERSION"), root)
}

fn web_semantic_cache_path_in(base: &Path, version: &str, root: &Path) -> PathBuf {
    base.join("plumb").join("site").join(version).join(format!(
        "{}.sqlite3",
        opaque_id("w", &root.to_string_lossy())
    ))
}

fn assign_candidate_parents(tasks: &mut [WebTaskCandidate]) {
    let mut ancestors = Vec::<(usize, String)>::new();
    let mut previous_path = None;
    for task in tasks {
        if previous_path.as_ref() != Some(&task.path) {
            ancestors.clear();
            previous_path = Some(task.path.clone());
        }
        while ancestors
            .last()
            .is_some_and(|(depth, _)| *depth >= task.depth)
        {
            ancestors.pop();
        }
        task.parent_key = ancestors.last().map(|(_, key)| key.clone());
        ancestors.push((task.depth, task.key.clone()));
    }
}

fn display_task_ref(root: &Path, target: &TaskRef) -> String {
    format!("{}#{}", display_path(root, &target.path), target.id)
}

fn apply_guarded_edit(
    source: String,
    path: &Path,
    revision: i64,
    edit: plumb_workspace::WorkspaceEdit,
    kind: &str,
) -> Result<String, String> {
    apply_document_edit(source, path, revision, edit).map_err(|error| match error {
        ApplyDocumentEditError::DocumentNotEdited => {
            format!("{kind} operation produced no document edit")
        }
        ApplyDocumentEditError::RevisionMismatch => {
            format!("{kind} operation used a stale document revision")
        }
        ApplyDocumentEditError::InvalidEdits => {
            format!("{kind} edits overlap or fall outside the document")
        }
    })
}

fn validate_generated_source(
    path: &Path,
    revision: i64,
    source: &str,
    kind: &str,
) -> Result<(), String> {
    let mut verification = Workspace::new();
    verification.insert(path, revision, source);
    if verification
        .get(path)
        .is_some_and(|entry| entry.current.is_some())
    {
        Ok(())
    } else {
        Err(format!("{kind} edit produced invalid plumb source"))
    }
}

fn event_input(input: &WebEventInput) -> EventInput {
    EventInput {
        title: input.title.clone(),
        at: input.at.clone().filter(|at| !at.is_empty()),
        start: input.start.clone().filter(|start| !start.is_empty()),
        end: input.end.clone().filter(|end| !end.is_empty()),
        tasks: input.tasks.clone(),
    }
}

fn relative_web_path(from: &Path, target: &Path) -> Option<String> {
    let from = from.parent().unwrap_or_else(|| Path::new(""));
    let from = from.components().collect::<Vec<_>>();
    let target = target.components().collect::<Vec<_>>();
    let common = from
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();
    let mut relative = PathBuf::new();
    for _ in common..from.len() {
        relative.push("..");
    }
    for component in &target[common..] {
        relative.push(component.as_os_str());
    }
    relative.to_str().map(str::to_string)
}

fn task_range_in(
    workspace: &Workspace,
    path: &Path,
    locator: &WebTaskLocator,
) -> Result<std::ops::Range<usize>, String> {
    let output = workspace
        .get(path)
        .and_then(|entry| entry.current.as_ref())
        .map(|current| &current.output)
        .ok_or_else(|| "task document is invalid".to_string())?;
    output
        .tasks
        .tasks
        .iter()
        .find(|task| match locator {
            WebTaskLocator::Id { id } => task.id.as_ref().is_some_and(|field| field.value == *id),
            WebTaskLocator::Offset { offset } => task.range.start == *offset,
        })
        .map(|task| task.range.clone())
        .ok_or_else(|| "task is no longer available".to_string())
}

fn task_authoring_error(error: WorkspaceOperationError<TaskAuthoringError>) -> String {
    let error = match error {
        WorkspaceOperationError::Operation(error) => error,
        WorkspaceOperationError::Query(error) => return error.to_string(),
    };
    match error {
        TaskAuthoringError::StaleOrInvalidDocument => "task document is stale or invalid",
        TaskAuthoringError::TaskNotFound => "task is no longer available",
        TaskAuthoringError::InvalidPlacement => "task parent or position is invalid",
        TaskAuthoringError::InvalidDatetime => "created, due, and wait must be RFC 3339 timestamps",
        TaskAuthoringError::InvalidRecurrence => "recur must be PnD, PnW, PnM, or PnY",
        TaskAuthoringError::InvalidReference => "task references must use #id or path#id",
        TaskAuthoringError::UnresolvedReference => {
            "task reference does not resolve to an indexed task"
        }
        TaskAuthoringError::DependencyCycle => "task dependencies would create a cycle",
        TaskAuthoringError::GeneratedInvalid => "task edit produced invalid plumb source",
    }
    .to_string()
}

fn event_edit_error(error: EventEditError) -> String {
    match error {
        EventEditError::StaleOrInvalidDocument => "event document is stale or invalid",
        EventEditError::EventNotFound => "event is no longer available",
        EventEditError::InvalidDatetime => "event at, start, and end must be RFC 3339 timestamps",
        EventEditError::InvalidTimeShape => {
            "point events require only at; intervals require start and optional end"
        }
        EventEditError::InvalidInterval => "event end must be later than start",
        EventEditError::GeneratedInvalid => "event edit produced invalid plumb source",
    }
    .to_string()
}

fn opaque_id(prefix: &str, value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut id = String::with_capacity(prefix.len() + 24);
    id.push_str(prefix);
    for byte in &digest[..12] {
        use std::fmt::Write as _;
        write!(id, "{byte:02x}").expect("writing to String cannot fail");
    }
    id
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    #[test]
    fn web_cache_paths_are_namespaced_by_compiled_version() {
        let base = Path::new("/cache");
        let root = Path::new("/notes");
        let current = web_semantic_cache_path_in(base, "0.34.1", root);
        let next = web_semantic_cache_path_in(base, "0.34.2", root);

        assert_eq!(current.parent().unwrap().file_name().unwrap(), "0.34.1");
        assert_eq!(next.parent().unwrap().file_name().unwrap(), "0.34.2");
        assert_eq!(current.file_name(), next.file_name());
        assert_ne!(current, next);
    }

    #[test]
    fn builds_graph_with_links_tasks_ghosts_and_bounded_neighborhoods() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("a.plumb"),
            "`= title|Alpha\n\n`- Old\n\n `+ task\n\n `@ old\n`- A\n\n `+ task\n\n `@ a\n\n `= prev|b.plumb#b\n `= depends|b.plumb#b\n`- Recurring instance\n\n `+ task\n\n `@ recur\n\n `= prev|#old\n\nSee `->[B|b.plumb#b], `->\"b.plumb#b\", `->[self|#a], `->[self again|#a], and `->[missing|missing.plumb].\n",
        )
        .unwrap();
        std::fs::write(root.join("b.plumb"), "`- Beta\n\n `+ task\n\n `@ b\n").unwrap();
        std::fs::write(root.join("broken.plumb"), "`broken[\n").unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let graph = workspace.graph(&GraphQuery::default()).unwrap();
        assert_eq!(
            graph.nodes.iter().filter(|node| !node.unresolved).count(),
            2
        );
        assert!(graph.nodes.iter().any(|node| node.unresolved));
        assert_eq!(
            graph
                .edges
                .iter()
                .filter(|edge| edge.kind == "link")
                .count(),
            2
        );
        assert!(graph.edges.iter().any(|edge| edge.kind == "autolink"));
        assert!(graph.edges.iter().any(|edge| edge.kind == "task-prev"));
        assert!(graph.edges.iter().any(|edge| edge.kind == "task-depends"));
        let limited = workspace
            .graph(&GraphQuery {
                limit: Some(1),
                ..GraphQuery::default()
            })
            .unwrap();
        assert!(!limited.complete);
        assert_eq!(limited.nodes.len(), 1);

        let alpha = workspace
            .document_id(root.join("a.plumb"))
            .unwrap()
            .to_string();
        let local = workspace
            .graph(&GraphQuery {
                current: Some(alpha),
                depth: Some(0),
                ..GraphQuery::default()
            })
            .unwrap();
        assert_eq!(local.nodes.len(), 1);
        assert!(local.edges.is_empty());

        let filtered = workspace
            .graph_excluding(&GraphQuery::default(), Some("path == 'a.plumb'"))
            .unwrap();
        assert!(filtered
            .nodes
            .iter()
            .all(|node| node.path.as_deref() != Some("a.plumb")));
        assert!(filtered.edges.iter().all(|edge| {
            filtered.nodes.iter().any(|node| node.id == edge.source)
                && filtered.nodes.iter().any(|node| node.id == edge.target)
        }));
        let error = workspace
            .graph_excluding(&GraphQuery::default(), Some("path"))
            .unwrap_err();
        assert!(error.contains("must return bool"), "{error}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn existing_workspace_snapshots_preserve_open_buffer_precedence() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("note.plumb");
        std::fs::write(&path, "disk\n").unwrap();
        let mut source_workspace = Workspace::new();
        source_workspace.insert(&path, 9, "`= title|Open buffer title\n");
        let web = WebWorkspace::from_workspace(&root, source_workspace, 4).unwrap();
        let graph = web.graph(&GraphQuery::default()).unwrap();
        assert_eq!(graph.revision, 4);
        assert_eq!(graph.nodes[0].title, "Open buffer title");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lazy_document_materialization_and_queries_use_snapshot_bytes() {
        let root = temp_dir();
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tasks.plumb");
        let first_source = "`- First\n\n `+ task\n\n `@ first\n";
        std::fs::write(&path, first_source).unwrap();
        let first = WebWorkspace::load(&root).unwrap();
        let document_id = first.document_id(&path).unwrap().to_string();
        assert!(first.documents[&path].entry.get().is_none());
        let warm = WebWorkspace::load_with_revision(&root, 2).unwrap();
        assert!(warm.documents[&path].generation_reused);
        assert!(warm.documents[&path].entry.get().is_none());
        assert_eq!(
            warm.query_tasks(&WebQuery::default()).unwrap().tasks[0].title,
            "First"
        );
        assert!(warm.documents[&path].entry.get().is_none());

        std::fs::write(&path, "`- Second\n\n `+ task\n\n `@ second\n").unwrap();
        let second = WebWorkspace::load_with_revision(&root, 3).unwrap();
        assert_eq!(
            first.note(&document_id).unwrap().unwrap().source,
            first_source
        );
        assert!(first.documents[&path].entry.get().is_some());
        assert_eq!(first.tasks().unwrap().tasks[0].title, "First");
        assert_eq!(second.tasks().unwrap().tasks[0].title, "Second");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cached_task_rows_project_the_live_document_revision_for_mutations() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tasks.plumb");
        let source = "`- Current\n\n `+ task\n\n `@ current\n";
        std::fs::write(&path, source).unwrap();

        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let mut source_workspace = Workspace::with_sqlite_store(store);
        source_workspace.insert_disk(&path, 1, source).unwrap();
        source_workspace.open_document(&path, 9, source);
        let workspace = WebWorkspace::from_workspace(&root, source_workspace, 1).unwrap();

        let snapshot = workspace.tasks().unwrap();
        assert_eq!(snapshot.documents[0].revision, "9");
        assert_eq!(snapshot.tasks[0].revision, "9");
        assert_eq!(
            workspace.task_candidates("", None, 10).unwrap()[0].revision,
            "9"
        );
        workspace
            .set_task_status(
                &snapshot.tasks[0].document_id,
                &snapshot.tasks[0].locator,
                &snapshot.tasks[0].revision,
                TaskStatus::Canceled,
            )
            .unwrap();

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn task_snapshots_expose_workspace_facts_and_status_edits() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tasks.plumb");
        std::fs::write(
            &path,
            "`- Ship release\n\n `+ task\n\n `@ ship\n\n `= priority|8\n `= created|2026-07-25T10:00:00+08:00\n `= due|2099-02-01T10:00:00+08:00\n\n `- Child stays with ship\n\n  `+ task\n\n  `@ child\n\n  `= priority|2\n  `= due|2099-01-01T10:00:00+08:00\n\n `- Urgent child\n\n  `+ task\n\n  `@ urgent-child\n\n  `= priority|9\n\n`- Later\n\n `+ task\n\n `@ later\n\n `= priority|3\n `= wait|2099-01-10T00:00:00+08:00\n `= due|2099-01-15T00:00:00+08:00\n`- Broken\n\n `+ task\n\n `@ broken\n\n `= done|2026-07-25T11:00:00+08:00\n `= canceled|2026-07-25T12:00:00+08:00\n",
        )
        .unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let snapshot = workspace.tasks().unwrap();
        assert_eq!(snapshot.tasks.len(), 5);
        assert_eq!(
            snapshot
                .tasks
                .iter()
                .map(|task| task.id.as_deref().unwrap())
                .collect::<Vec<_>>(),
            ["ship", "urgent-child", "child", "later", "broken"]
        );
        let task = snapshot
            .tasks
            .iter()
            .find(|task| task.id.as_deref() == Some("ship"))
            .unwrap();
        assert_eq!(task.id.as_deref(), Some("ship"));
        assert_eq!(task.state, "ready");
        assert!(task.actionable);
        let later = snapshot
            .tasks
            .iter()
            .find(|task| task.id.as_deref() == Some("later"))
            .unwrap();
        assert_eq!(later.state, "waiting");
        assert_eq!(later.wait_reasons, ["time"]);
        let broken = snapshot
            .tasks
            .iter()
            .find(|task| task.id.as_deref() == Some("broken"))
            .unwrap();
        assert_eq!(broken.state, "conflicted");
        workspace
            .set_task_status(
                &task.document_id,
                &task.locator,
                &task.revision,
                TaskStatus::Done,
            )
            .unwrap();
        let updated = std::fs::read_to_string(&path).unwrap();
        assert!(updated.contains("`= done|2026-"), "{updated}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn event_snapshots_and_guarded_mutations_use_source_keys() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("agenda.plumb");
        std::fs::write(&path, "`# Agenda\n").unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let document = workspace.events().unwrap().documents[0].clone();
        workspace
            .create_event(
                &document.id,
                &document.revision,
                &WebEventInput {
                    title: "Review".to_string(),
                    at: None,
                    start: Some("2026-07-30T06:00:00Z".to_string()),
                    end: Some("2026-07-30T07:00:00Z".to_string()),
                    tasks: vec!["#write".to_string()],
                },
            )
            .unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let event = workspace.events().unwrap().events[0].clone();
        assert!(event.id.is_none());
        workspace
            .update_event(
                &event.document_id,
                &event.locator,
                &event.revision,
                &WebEventInput {
                    title: "Updated".to_string(),
                    at: Some("2026-07-30T08:00:00Z".to_string()),
                    start: None,
                    end: None,
                    tasks: Vec::new(),
                },
            )
            .unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let updated = workspace.events().unwrap().events[0].clone();
        assert!(updated.id.is_none());
        assert_eq!(updated.title, "Updated");
        assert_eq!(updated.at.as_deref(), Some("2026-07-30T08:00:00+00:00"));
        assert_eq!(updated.end, None);
        assert_eq!(
            workspace.update_event(
                &updated.document_id,
                &updated.locator,
                "stale",
                &WebEventInput {
                    title: "Conflict".to_string(),
                    at: Some("2026-07-30T08:00:00Z".to_string()),
                    start: None,
                    end: None,
                    tasks: Vec::new(),
                },
            ),
            Err("event document changed; refresh before retrying".to_string())
        );
        workspace
            .delete_event(&updated.document_id, &updated.locator, &updated.revision)
            .unwrap();
        assert!(WebWorkspace::load(&root)
            .unwrap()
            .events()
            .unwrap()
            .events
            .is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn event_snapshots_sort_rfc3339_values_by_instant() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("agenda.plumb"),
            "`= date|2026-07-30\n`= timezone|+00:00\n\n`- 10:30|Early\n\n `+ event\n\n `= timezone|+05:00\n`- 06:00|Later\n\n `+ event\n",
        )
        .unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        assert_eq!(
            workspace
                .events()
                .unwrap()
                .events
                .iter()
                .map(|event| event.title.as_str())
                .collect::<Vec<_>>(),
            ["Early", "Later"]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn event_pages_are_bounded_and_reach_invalid_times() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        let mut source = String::new();
        for day in 1..=250 {
            source.push_str(&format!(
                "`- Future|{day} {{\n\n `+ event\n\n `= at|2027-01-01T00:00:00+00:00\n}}\n"
            ));
        }
        source.push_str("`- Invalid\n\n `+ event\n");
        std::fs::write(root.join("agenda.plumb"), source).unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let first = workspace.event_page(None, None).unwrap();
        assert_eq!(first.events.len(), 120);
        assert!(first.earlier_cursor.is_none());
        let second = workspace
            .event_page(first.later_cursor.as_deref(), Some("later"))
            .unwrap();
        assert_eq!(second.events.len(), 131);
        assert!(second.events.last().unwrap().at.is_none());
        assert!(second.events.last().unwrap().start.is_none());
        assert!(second.later_cursor.is_none());
        assert_ne!(first.events.last().unwrap().key, second.events[0].key);
        let selected = second.events.last().unwrap().key.clone();
        let anchored = workspace.event_page_for_selection(&selected).unwrap();
        assert!(anchored.events.iter().any(|event| event.key == selected));
        assert!(anchored.events.len() <= 240);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn idless_tasks_use_exact_indexed_offsets_for_status_edits() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tasks.plumb");
        let source =
            "`- 完成任务后查看修改后任务的内容\n\n `+ task\n\n `= created|2026-07-25T10:00:00+08:00\n";
        std::fs::write(&path, source).unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let task = workspace.tasks().unwrap().tasks.into_iter().next().unwrap();
        let WebTaskLocator::Offset { offset } = task.locator else {
            panic!("idless task must use an offset locator");
        };

        let error = workspace
            .set_task_status(
                &task.document_id,
                &WebTaskLocator::Offset { offset: offset + 1 },
                &task.revision,
                TaskStatus::Done,
            )
            .unwrap_err();
        assert!(error.contains("position changed"), "{error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), source);

        workspace
            .set_task_status(
                &task.document_id,
                &WebTaskLocator::Offset { offset },
                &task.revision,
                TaskStatus::Done,
            )
            .unwrap();
        let updated = std::fs::read_to_string(&path).unwrap();
        assert!(updated.contains("`= done|2026-"), "{updated}");
        let refreshed = WebWorkspace::load_with_revision(&root, 2).unwrap();
        let completed = refreshed.tasks().unwrap().tasks.into_iter().next().unwrap();
        assert_eq!(completed.key, task.key);
        assert_eq!(completed.state, "done");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn structured_task_authoring_is_guarded_and_supports_reparenting() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tasks.plumb");
        std::fs::write(
            &path,
            "`- Parent\n\n `+ task\n\n `@ parent\n\n `= custom|keep\n",
        )
        .unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let snapshot = workspace.tasks().unwrap();
        let document = &snapshot.documents[0];
        let parent = &snapshot.tasks[0];
        workspace
            .create_task(
                &document.id,
                &document.revision,
                &WebTaskInput {
                    title: "Created child".to_string(),
                    created: None,
                    due: Some("2026-08-01T10:00:00Z".to_string()),
                    wait: None,
                    recur: None,
                    prev: None,
                    depends: Vec::new(),
                    priority: Some(4),
                },
                &WebTaskPlacement {
                    parent: Some(parent.locator.clone()),
                    after: None,
                },
            )
            .unwrap();
        let refreshed = WebWorkspace::load_with_revision(&root, 2).unwrap();
        let snapshot = refreshed.tasks().unwrap();
        let child = snapshot
            .tasks
            .iter()
            .find(|task| task.title == "Created child")
            .unwrap();
        assert_eq!(child.depth, 1);
        assert!(child.id.is_some());
        let parent = snapshot
            .tasks
            .iter()
            .find(|task| task.id.as_deref() == Some("parent"))
            .unwrap();
        refreshed
            .update_task_fields(
                &child.document_id,
                &child.locator,
                &child.revision,
                &WebTaskInput {
                    title: "Updated child".to_string(),
                    created: child.created.clone(),
                    due: None,
                    wait: None,
                    recur: None,
                    prev: None,
                    depends: Vec::new(),
                    priority: Some(-1),
                },
                Some(&WebTaskPlacement {
                    parent: None,
                    after: Some(parent.locator.clone()),
                }),
            )
            .unwrap();
        let final_workspace = WebWorkspace::load_with_revision(&root, 3).unwrap();
        let updated = final_workspace
            .tasks()
            .unwrap()
            .tasks
            .into_iter()
            .find(|task| task.title == "Updated child")
            .unwrap();
        assert_eq!(updated.depth, 0);
        assert_eq!(updated.priority, Some(-1));
        assert!(updated.due.is_none());
        assert!(updated.recur.is_none());
        let source = std::fs::read_to_string(&path).unwrap();
        assert!(source.contains("`= custom|keep"), "{source}");
        assert!(refreshed
            .create_task(
                &document.id,
                &document.revision,
                &WebTaskInput {
                    title: "Stale".to_string(),
                    created: None,
                    due: None,
                    wait: None,
                    recur: None,
                    prev: None,
                    depends: Vec::new(),
                    priority: None,
                },
                &WebTaskPlacement::default(),
            )
            .unwrap_err()
            .contains("changed"));

        let externally_changed = WebWorkspace::load_with_revision(&root, 4).unwrap();
        let snapshot = externally_changed.tasks().unwrap();
        let document = &snapshot.documents[0];
        let external = format!(
            "{}\n`note External edit\n",
            std::fs::read_to_string(&path).unwrap()
        );
        std::fs::write(&path, &external).unwrap();
        let error = externally_changed
            .create_task(
                &document.id,
                &document.revision,
                &WebTaskInput {
                    title: "Must not overwrite".to_string(),
                    created: None,
                    due: None,
                    wait: None,
                    recur: None,
                    prev: None,
                    depends: Vec::new(),
                    priority: None,
                },
                &WebTaskPlacement::default(),
            )
            .unwrap_err();
        assert!(error.contains("changed on disk"), "{error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), external);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn updates_and_reparents_idless_tasks_without_an_intermediate_parse() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tasks.plumb");
        std::fs::write(
            &path,
            "`- Parent\n\n `+ task\n\n `@ parent\n\n `- Idless child\n\n  `+ task\n\n  `= custom|keep\n",
        )
        .unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let snapshot = workspace.tasks().unwrap();
        let parent = snapshot
            .tasks
            .iter()
            .find(|task| task.id.as_deref() == Some("parent"))
            .unwrap();
        let child = snapshot
            .tasks
            .iter()
            .find(|task| task.title == "Idless child")
            .unwrap();
        assert!(child.id.is_none());
        workspace
            .update_task_fields(
                &child.document_id,
                &child.locator,
                &child.revision,
                &WebTaskInput {
                    title: "Renamed idless child".to_string(),
                    created: child.created.clone(),
                    due: None,
                    wait: None,
                    recur: None,
                    prev: None,
                    depends: Vec::new(),
                    priority: Some(-2),
                },
                Some(&WebTaskPlacement {
                    parent: None,
                    after: Some(parent.locator.clone()),
                }),
            )
            .unwrap();
        let updated = std::fs::read_to_string(&path).unwrap();
        assert!(
            updated.contains("`- Renamed idless child\n\n `+ task\n"),
            "{updated}"
        );
        assert!(updated.contains("`= custom|keep"), "{updated}");
        let refreshed = WebWorkspace::load_with_revision(&root, 2).unwrap();
        let task = refreshed
            .tasks()
            .unwrap()
            .tasks
            .into_iter()
            .find(|task| task.title == "Renamed idless child")
            .unwrap();
        assert_eq!(task.depth, 0);
        assert_eq!(task.priority, Some(-2));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn structured_task_authoring_resolves_indexed_dependencies_and_rejects_bad_fields() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        let a_path = root.join("a.plumb");
        let b_path = root.join("b.plumb");
        std::fs::write(&a_path, "`- A\n\n `+ task\n\n `@ a\n").unwrap();
        std::fs::write(&b_path, "`- B\n\n `+ task\n\n `@ b\n").unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let snapshot = workspace.tasks().unwrap();
        let a = snapshot
            .tasks
            .iter()
            .find(|task| task.id.as_deref() == Some("a"))
            .unwrap();
        let b = snapshot
            .tasks
            .iter()
            .find(|task| task.id.as_deref() == Some("b"))
            .unwrap();
        let a_document = snapshot
            .documents
            .iter()
            .find(|document| document.id == a.document_id)
            .unwrap();
        let dependency = WebTaskReferenceInput {
            document_id: b.document_id.clone(),
            locator: b.locator.clone(),
        };
        workspace
            .update_task_fields(
                &a.document_id,
                &a.locator,
                &a.revision,
                &WebTaskInput {
                    title: a.title.clone(),
                    created: a.created.clone(),
                    due: None,
                    wait: None,
                    recur: None,
                    prev: Some(dependency.clone()),
                    depends: vec![dependency],
                    priority: None,
                },
                None,
            )
            .unwrap();
        let source = std::fs::read_to_string(&a_path).unwrap();
        assert!(source.contains("`= prev|b.plumb#b"), "{source}");
        assert!(source.contains("`= depends|b.plumb#b"), "{source}");

        let refreshed = WebWorkspace::load_with_revision(&root, 2).unwrap();
        let snapshot = refreshed.tasks().unwrap();
        let b = snapshot
            .tasks
            .iter()
            .find(|task| task.id.as_deref() == Some("b"))
            .unwrap();
        let a = snapshot
            .tasks
            .iter()
            .find(|task| task.id.as_deref() == Some("a"))
            .unwrap();
        assert_eq!(a.prev_on.as_deref(), Some("b.plumb#b"));
        let original_b = std::fs::read_to_string(&b_path).unwrap();
        let cycle = refreshed
            .update_task_fields(
                &b.document_id,
                &b.locator,
                &b.revision,
                &WebTaskInput {
                    title: b.title.clone(),
                    created: b.created.clone(),
                    due: None,
                    wait: None,
                    recur: None,
                    prev: None,
                    depends: vec![WebTaskReferenceInput {
                        document_id: a.document_id.clone(),
                        locator: a.locator.clone(),
                    }],
                    priority: None,
                },
                None,
            )
            .unwrap_err();
        assert!(cycle.contains("cycle"), "{cycle}");
        assert_eq!(std::fs::read_to_string(&b_path).unwrap(), original_b);

        let bad_datetime = refreshed
            .create_task(
                &a_document.id,
                &refreshed
                    .tasks()
                    .unwrap()
                    .documents
                    .iter()
                    .find(|document| document.id == a_document.id)
                    .unwrap()
                    .revision,
                &WebTaskInput {
                    title: "Bad date".to_string(),
                    created: None,
                    due: Some("tomorrow".to_string()),
                    wait: None,
                    recur: None,
                    prev: None,
                    depends: Vec::new(),
                    priority: None,
                },
                &WebTaskPlacement::default(),
            )
            .unwrap_err();
        assert!(bad_datetime.contains("RFC 3339"), "{bad_datetime}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn web_queries_compose_filters_and_preserve_task_subtrees() {
        let legacy: WebQuery = serde_json::from_str(r#"{"sort":"priority"}"#).unwrap();
        assert_eq!(legacy.sort, [QuerySort::Priority]);
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("a.plumb"),
            "`- Parent\n\n `+ task\n\n `@ parent\n\n `= priority|3\n `= due|2099-02-01T00:00:00Z\n\n `- Needle child\n\n  `+ task\n\n  `@ matching\n\n  `= priority|20\n  `= due|2099-01-01T00:00:00Z\n\n `- Quiet child\n\n  `+ task\n\n  `@ quiet\n\n  `= priority|30\n\n`- Needle first\n\n `+ task\n\n `@ first\n\n `= priority|8\n `= due|2099-01-15T00:00:00Z\n`- Needle done\n\n `+ task\n\n `@ done\n\n `= done|2026-07-27T00:00:00Z\n",
        )
        .unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();

        let source = workspace
            .query_tasks(&WebQuery {
                view: WebView::Tasks,
                query: "needle".to_string(),
                ..WebQuery::default()
            })
            .unwrap();
        assert!(source.all_tasks.is_empty());
        let candidates = workspace.task_candidates("", None, 50).unwrap();
        assert_eq!(candidates.len(), 5);
        assert_eq!(
            workspace.task_candidates("Needle", None, 2).unwrap().len(),
            2
        );
        let document_id = candidates[0].document_id.as_str();
        assert!(workspace
            .task_candidates("", Some(document_id), 3)
            .unwrap()
            .iter()
            .all(|candidate| candidate.document_id == document_id));
        let matching = candidates
            .iter()
            .find(|task| task.id.as_deref() == Some("matching"))
            .unwrap();
        assert_eq!(
            candidates
                .iter()
                .find(|task| Some(task.key.as_str()) == matching.parent_key.as_deref())
                .and_then(|task| task.id.as_deref()),
            Some("parent")
        );
        assert_eq!(
            source
                .tasks
                .iter()
                .map(|task| task.id.as_deref().unwrap())
                .collect::<Vec<_>>(),
            ["matching", "first", "done"]
        );
        let due = workspace
            .query_tasks(&WebQuery {
                view: WebView::Tasks,
                query: "needle".to_string(),
                sort: vec![QuerySort::Due],
                ..WebQuery::default()
            })
            .unwrap();
        assert_eq!(
            due.tasks
                .iter()
                .map(|task| task.id.as_deref().unwrap())
                .collect::<Vec<_>>(),
            ["matching", "first", "done"]
        );
        let priority = workspace
            .query_tasks(&WebQuery {
                view: WebView::Tasks,
                query: "needle".to_string(),
                sort: vec![QuerySort::Priority],
                ..WebQuery::default()
            })
            .unwrap();
        assert_eq!(
            priority
                .tasks
                .iter()
                .map(|task| task.id.as_deref().unwrap())
                .collect::<Vec<_>>(),
            ["matching", "first", "done"]
        );
        let all_by_priority = workspace
            .query_tasks(&WebQuery {
                view: WebView::Tasks,
                sort: vec![QuerySort::Priority],
                ..WebQuery::default()
            })
            .unwrap();
        assert_eq!(
            all_by_priority
                .tasks
                .iter()
                .map(|task| task.id.as_deref().unwrap())
                .collect::<Vec<_>>(),
            ["parent", "quiet", "matching", "first", "done"]
        );
        let priority_filter = workspace
            .query_tasks(&WebQuery {
                view: WebView::Tasks,
                filters: vec!["priority != null && priority >= 8".to_string()],
                ..WebQuery::default()
            })
            .unwrap();
        assert_eq!(priority_filter.tasks.len(), 3);
        let ready = workspace
            .query_tasks(&WebQuery {
                view: WebView::Tasks,
                presets: vec!["ready".to_string()],
                filter: "title.contains('Needle')".to_string(),
                ..WebQuery::default()
            })
            .unwrap();
        assert_eq!(ready.tasks.len(), 2);
        let ready_or_done = workspace
            .query_tasks(&WebQuery {
                view: WebView::Tasks,
                presets: vec!["ready".to_string(), "done".to_string()],
                ..WebQuery::default()
            })
            .unwrap();
        assert_eq!(ready_or_done.tasks.len(), 5);
        let multiple_custom = workspace
            .query_tasks(&WebQuery {
                view: WebView::Tasks,
                filters: vec![
                    "title.contains('Needle')".to_string(),
                    "state == 'ready'".to_string(),
                ],
                ..WebQuery::default()
            })
            .unwrap();
        assert_eq!(multiple_custom.tasks.len(), 2);
        let numbered_error = workspace
            .query_tasks(&WebQuery {
                view: WebView::Tasks,
                filters: vec!["actionable".to_string(), "title".to_string()],
                ..WebQuery::default()
            })
            .unwrap_err();
        assert_eq!(numbered_error.source, "custom:2");
        let error = workspace
            .query_tasks(&WebQuery {
                view: WebView::Tasks,
                filter: "title".to_string(),
                ..WebQuery::default()
            })
            .unwrap_err();
        assert_eq!(error.source, "custom");
        assert!(
            error.message.contains("must return bool"),
            "{}",
            error.message
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn task_sorts_aggregate_documents_and_never_split_their_trees() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("a.plumb"),
            "`- A late\n\n `+ task\n\n `@ a-late\n\n `= priority|-5\n `= due|2099-03-01T00:00:00Z\n`- A promoted\n\n `+ task\n\n `@ a-promoted\n\n `= priority|-10\n\n `- A urgent\n\n  `+ task\n\n  `@ a-urgent\n\n  `= priority|30\n\n`- A early\n\n `+ task\n\n `@ a-early\n\n `= due|2099-01-01T00:15:00-01:00\n",
        )
        .unwrap();
        std::fs::write(
            root.join("b.plumb"),
            "`- B high\n\n `+ task\n\n `@ b-high\n\n `= priority|20\n `= due|2099-01-01T01:00:00+02:00\n`- B other\n\n `+ task\n\n `@ b-other\n\n `= priority|15\n",
        )
        .unwrap();
        std::fs::write(
            root.join("c.plumb"),
            "`- C negative\n\n `+ task\n\n `@ c-negative\n\n `= priority|-1\n",
        )
        .unwrap();
        std::fs::write(
            root.join("d.plumb"),
            "`- D default\n\n `+ task\n\n `@ d-default\n",
        )
        .unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();

        let priority = workspace
            .query_tasks(&WebQuery {
                view: WebView::Tasks,
                sort: vec![QuerySort::Priority],
                ..WebQuery::default()
            })
            .unwrap();
        assert_eq!(
            priority
                .tasks
                .iter()
                .map(|task| task.id.as_deref().unwrap())
                .collect::<Vec<_>>(),
            [
                "a-promoted",
                "a-urgent",
                "a-early",
                "a-late",
                "b-high",
                "b-other",
                "c-negative",
                "d-default",
            ]
        );

        let due = workspace
            .query_tasks(&WebQuery {
                view: WebView::Tasks,
                sort: vec![QuerySort::Due],
                ..WebQuery::default()
            })
            .unwrap();
        assert_eq!(
            due.tasks
                .iter()
                .map(|task| task.id.as_deref().unwrap())
                .collect::<Vec<_>>(),
            [
                "b-high",
                "b-other",
                "a-early",
                "a-late",
                "a-promoted",
                "a-urgent",
                "c-negative",
                "d-default",
            ]
        );
        for tasks in [&priority.tasks, &due.tasks] {
            let paths = tasks
                .iter()
                .map(|task| task.path.as_str())
                .collect::<Vec<_>>();
            for path in ["a.plumb", "b.plumb", "c.plumb", "d.plumb"] {
                let positions = paths
                    .iter()
                    .enumerate()
                    .filter_map(|(index, candidate)| (*candidate == path).then_some(index))
                    .collect::<Vec<_>>();
                assert!(positions.windows(2).all(|pair| pair[1] == pair[0] + 1));
            }
        }
        let first_page_query = WebQuery {
            view: WebView::Tasks,
            sort: vec![QuerySort::Priority],
            limit: Some(1),
            ..WebQuery::default()
        };
        let limited = workspace.query_tasks(&first_page_query).unwrap();
        assert!(!limited.complete);
        assert_eq!(limited.tasks.len(), 4);
        assert!(limited.tasks.iter().all(|task| task.path == "a.plumb"));
        let second_page = workspace
            .query_tasks(&WebQuery {
                cursor: limited.next_cursor.clone(),
                ..first_page_query.clone()
            })
            .unwrap();
        assert!(second_page.tasks.iter().all(|task| task.path != "a.plumb"));
        assert!(
            workspace
                .query_tasks(&WebQuery {
                    query: "changed".to_string(),
                    cursor: limited.next_cursor,
                    ..first_page_query
                })
                .unwrap_err()
                .source
                == "cursor"
        );

        let due_then_priority = workspace
            .query_tasks(&WebQuery {
                view: WebView::Tasks,
                sort: vec![QuerySort::Due, QuerySort::Priority, QuerySort::Due],
                ..WebQuery::default()
            })
            .unwrap();
        assert_eq!(
            due_then_priority
                .tasks
                .iter()
                .map(|task| task.id.as_deref().unwrap())
                .collect::<Vec<_>>(),
            [
                "b-high",
                "b-other",
                "a-early",
                "a-late",
                "a-promoted",
                "a-urgent",
                "c-negative",
                "d-default",
            ]
        );
        let empty_source = workspace
            .query_tasks(&WebQuery {
                view: WebView::Tasks,
                sort: Vec::new(),
                ..WebQuery::default()
            })
            .unwrap();
        assert_eq!(
            empty_source
                .tasks
                .iter()
                .map(|task| task.id.as_deref().unwrap())
                .collect::<Vec<_>>(),
            [
                "a-late",
                "a-promoted",
                "a-urgent",
                "a-early",
                "b-high",
                "b-other",
                "c-negative",
                "d-default",
            ]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn priority_sort_promotes_open_dependencies() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("tasks.plumb"),
            "`- Medium\n\n `+ task\n\n `@ medium\n\n `= priority|20\n`- Blocker\n\n `+ task\n\n `@ blocker\n\n `= priority|-5\n`- Urgent\n\n `+ task\n\n `@ urgent\n\n `= priority|50\n `= depends|#blocker\n",
        )
        .unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();

        let snapshot = workspace
            .query_tasks(&WebQuery {
                view: WebView::Tasks,
                sort: vec![QuerySort::Priority],
                ..WebQuery::default()
            })
            .unwrap();
        assert_eq!(
            snapshot
                .tasks
                .iter()
                .map(|task| (task.id.as_deref().unwrap(), task.effective_priority))
                .collect::<Vec<_>>(),
            [("blocker", 50), ("urgent", 50), ("medium", 20)]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn filtered_tasks_do_not_contribute_to_effective_priority() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("a.plumb"),
            "`- Closed\n\n `+ task\n\n `@ closed\n\n `= priority|100\n `= done|2026-07-31T10:00:00Z\n`- Low active\n\n `+ task\n\n `@ low\n\n `= priority|1\n",
        )
        .unwrap();
        std::fs::write(
            root.join("b.plumb"),
            "`- Important active\n\n `+ task\n\n `@ important\n\n `= priority|10\n",
        )
        .unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();

        let ready = workspace
            .query_tasks(&WebQuery {
                view: WebView::Tasks,
                presets: vec!["ready".to_string()],
                sort: vec![QuerySort::Priority],
                ..WebQuery::default()
            })
            .unwrap();
        assert_eq!(
            ready
                .tasks
                .iter()
                .map(|task| task.id.as_deref().unwrap())
                .collect::<Vec<_>>(),
            ["important", "low"]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn graph_queries_filter_after_traversal_and_remove_hidden_endpoints() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.plumb"), "`->[B|b.plumb]\n").unwrap();
        std::fs::write(
            root.join("b.plumb"),
            "`->[C|c.plumb]\n\n`- Work\n\n `+ task\n",
        )
        .unwrap();
        std::fs::write(root.join("c.plumb"), "C\n").unwrap();
        std::fs::write(root.join("orphan.plumb"), "Orphan\n").unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let a = workspace
            .document_id(root.join("a.plumb"))
            .unwrap()
            .to_string();
        let graph = workspace
            .query_graph(
                &WebQuery {
                    presets: vec!["has-tasks".to_string()],
                    traversal: GraphQuery {
                        current: Some(a),
                        depth: Some(2),
                        direction: GraphDirection::Outgoing,
                        ..GraphQuery::default()
                    },
                    ..WebQuery::default()
                },
                None,
            )
            .unwrap();
        assert_eq!(graph.nodes.len(), 1);
        assert_eq!(graph.nodes[0].path.as_deref(), Some("b.plumb"));
        assert!(graph.edges.is_empty());

        let orphans = workspace
            .query_graph(
                &WebQuery {
                    presets: vec!["orphans".to_string()],
                    ..WebQuery::default()
                },
                None,
            )
            .unwrap();
        assert_eq!(orphans.nodes.len(), 1);
        assert_eq!(orphans.nodes[0].path.as_deref(), Some("orphan.plumb"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn web_workspace_uses_workspace_ignore_files() {
        let root = temp_dir();
        std::fs::create_dir_all(root.join("private")).unwrap();
        std::fs::write(root.join(".ignore"), "private/\n").unwrap();
        std::fs::write(root.join("public.plumb"), "Public\n").unwrap();
        std::fs::write(root.join("private/note.plumb"), "Private\n").unwrap();

        let first = WebWorkspace::load(&root).unwrap();
        assert_eq!(first.graph(&GraphQuery::default()).unwrap().nodes.len(), 1);
        std::fs::write(root.join("private/note.plumb"), "Changed private\n").unwrap();
        let second = WebWorkspace::load_with_revision(&root, 2).unwrap();
        assert!(first.has_same_documents(&second));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn indexes_resources_when_workspace_root_contains_a_symlink() {
        use std::os::unix::fs::symlink;

        let target = temp_dir();
        let parent = temp_dir();
        let root = parent.join("workspace");
        std::fs::create_dir_all(target.join("static")).unwrap();
        std::fs::create_dir_all(&parent).unwrap();
        std::fs::write(target.join("static/image.jpg"), b"image").unwrap();
        std::fs::write(
            target.join("note.plumb"),
            "`img[|=[src|static/image.jpg]]\n",
        )
        .unwrap();
        symlink(&target, &root).unwrap();

        let workspace = WebWorkspace::load(&root).unwrap();
        let resource = workspace
            .resources()
            .unwrap()
            .next()
            .expect("indexed image");
        assert_eq!(resource.path, target.join("static/image.jpg"));

        std::fs::remove_dir_all(parent).unwrap();
        std::fs::remove_dir_all(target).unwrap();
    }

    #[test]
    fn web_task_snapshots_cover_dependencies_recurrence_staleness_and_removal() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tasks.plumb");
        std::fs::write(
            &path,
            "`- Blocker\n\n `+ task\n\n `@ blocker\n`- Dependent\n\n `+ task\n\n `@ dependent\n\n `= depends|#blocker\n`- Recurring\n\n `+ task\n\n `@ recurring\n\n `= due|2026-07-20T09:00:00+08:00\n `= recur|P1D\n`- Conflicted\n\n `+ task\n\n `@ conflicted\n\n `= done|2026-07-20T10:00:00+08:00\n `= canceled|2026-07-20T11:00:00+08:00\n",
        )
        .unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let snapshot = workspace.tasks().unwrap();
        let dependent = snapshot
            .tasks
            .iter()
            .find(|task| task.id.as_deref() == Some("dependent"))
            .unwrap();
        assert_eq!(dependent.state, "blocked");
        assert_eq!(dependent.wait_reasons, ["dependency"]);
        assert_eq!(
            snapshot
                .tasks
                .iter()
                .find(|task| task.id.as_deref() == Some("conflicted"))
                .unwrap()
                .state,
            "conflicted"
        );
        let recurring = snapshot
            .tasks
            .iter()
            .find(|task| task.id.as_deref() == Some("recurring"))
            .unwrap();
        let stale = workspace
            .set_task_status(
                &recurring.document_id,
                &recurring.locator,
                "stale-revision",
                TaskStatus::Done,
            )
            .unwrap_err();
        assert!(stale.contains("changed"), "{stale}");
        workspace
            .set_task_status(
                &recurring.document_id,
                &recurring.locator,
                &recurring.revision,
                TaskStatus::Done,
            )
            .unwrap();
        let refreshed = WebWorkspace::load_with_revision(&root, 2).unwrap();
        assert!(refreshed
            .tasks()
            .unwrap()
            .tasks
            .iter()
            .any(|task| { task.prev.as_deref() == Some("#recurring") && task.state == "ready" }));

        std::fs::remove_file(&path).unwrap();
        assert!(WebWorkspace::load_with_revision(&root, 3)
            .unwrap()
            .tasks()
            .unwrap()
            .tasks
            .is_empty());
        std::fs::write(&path, "`- Ignored\n\n `+ task\n").unwrap();
        std::fs::write(root.join(".ignore"), "tasks.plumb\n").unwrap();
        assert!(WebWorkspace::load_with_revision(&root, 4)
            .unwrap()
            .tasks()
            .unwrap()
            .tasks
            .is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    fn temp_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "plumb-web-model-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
