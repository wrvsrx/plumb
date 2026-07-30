use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::ops::Range;
use std::path::{Path, PathBuf};

use cel::{Context, Program, Value};
use chrono::{Local, SecondsFormat};
use plumb_extensions::{LinkSpelling, TaskStatus};
use plumb_workspace::{
    normalize, scan_workspace_files, search_score, sort_task_records,
    truncate_complete_task_documents, EventEditError, EventInput, ResolvedTarget, SearchRecordKind,
    TaskEditError, TaskRef, TaskSortFacts, TaskSortOrder, TextEdit, Workspace,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum GraphDirection {
    Incoming,
    Outgoing,
    #[default]
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
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
    pub wait: Option<String>,
    pub done: Option<String>,
    pub canceled: Option<String>,
    pub recur: Option<String>,
    pub prev: Option<String>,
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
    pub uid: Option<String>,
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
    pub start: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
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
    #[serde(default)]
    pub sort: QuerySort,
    pub limit: Option<usize>,
    #[serde(default)]
    pub traversal: GraphQuery,
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
    pub all_tasks: Vec<WebTask>,
    pub complete: bool,
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
        id: "invalid",
        label: "Invalid",
        expression: "state == 'invalid'",
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
    revision: u64,
    document_ids: BTreeMap<PathBuf, String>,
    paths_by_document_id: HashMap<String, PathBuf>,
    titles: HashMap<PathBuf, String>,
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
        let mut workspace = Workspace::new();
        for path in &paths {
            let source = std::fs::read_to_string(path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            let file_revision = file_revision(path).unwrap_or(0);
            workspace.insert(path, file_revision, source);
        }

        Self::from_workspace(root, workspace, revision)
    }

    pub fn from_workspace(
        root: impl AsRef<Path>,
        workspace: Workspace,
        revision: u64,
    ) -> Result<Self, String> {
        let root = normalize(root.as_ref());
        if !root.is_dir() {
            return Err(format!(
                "workspace root is not a directory: {}",
                root.display()
            ));
        }

        let mut valid_paths = workspace
            .documents()
            .filter(|entry| entry.current.is_some())
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        valid_paths.sort();
        let document_ids = valid_paths
            .iter()
            .map(|path| (path.clone(), opaque_id("d", &display_path(&root, path))))
            .collect::<BTreeMap<_, _>>();
        let paths_by_document_id = document_ids
            .iter()
            .map(|(path, id)| (id.clone(), path.clone()))
            .collect();
        let titles = workspace
            .search_records(
                &root,
                Some(SearchRecordKind::Note),
                "",
                usize::MAX,
                Local::now().fixed_offset(),
            )
            .items
            .into_iter()
            .map(|record| (record.path, record.title))
            .collect();

        let mut result = Self {
            root,
            workspace,
            revision,
            document_ids,
            paths_by_document_id,
            titles,
            resources: BTreeMap::new(),
            resources_by_id: HashMap::new(),
        };
        result.index_resources();
        Ok(result)
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

    pub fn resource(&self, id: &str) -> Option<&ResourceRecord> {
        let path = self.resources_by_id.get(id)?;
        self.resources.get(path)
    }

    pub fn resource_for_path(&self, path: impl AsRef<Path>) -> Option<&ResourceRecord> {
        self.resources.get(&normalize(path.as_ref()))
    }

    pub fn resources(&self) -> impl Iterator<Item = &ResourceRecord> {
        self.resources.values()
    }

    pub fn tasks(&self) -> TaskSnapshot {
        let now = Local::now().fixed_offset();
        let records = self.workspace.search_records(
            &self.root,
            Some(SearchRecordKind::Task),
            "",
            usize::MAX,
            now,
        );
        let mut tasks = records
            .items
            .into_iter()
            .filter_map(|record| {
                let document_id = self.document_id(&record.path)?.to_string();
                let state = record.task_state?.as_str();
                let task = self
                    .workspace
                    .documents()
                    .find(|entry| entry.path == record.path)?
                    .current
                    .as_ref()?
                    .output
                    .tasks
                    .tasks
                    .iter()
                    .find(|task| task.selection_range == record.range)?;
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
                let depends_on = self
                    .workspace
                    .task_dependencies(&record.path, task)
                    .into_iter()
                    .map(|dependency| display_task_ref(&self.root, &dependency.target))
                    .collect();
                let directly_blocking = record
                    .id
                    .as_deref()
                    .map(|id| {
                        self.workspace
                            .directly_blocking_tasks(&record.path, id)
                            .into_iter()
                            .map(|target| display_task_ref(&self.root, &target))
                            .collect()
                    })
                    .unwrap_or_default();
                Some(WebTask {
                    key,
                    document_id,
                    title: record.title,
                    path: record.relative_path,
                    revision: record.revision.to_string(),
                    id: record.id,
                    locator,
                    state: state.to_string(),
                    created: task.created.as_ref().map(|field| field.value.clone()),
                    due: task.due.as_ref().map(|field| field.value.clone()),
                    priority: task.priority,
                    wait: task.wait.as_ref().map(|field| field.value.clone()),
                    done: task.done.as_ref().map(|field| field.value.clone()),
                    canceled: task.canceled.as_ref().map(|field| field.value.clone()),
                    recur: task.recur.as_ref().map(|field| field.value.clone()),
                    prev: task.prev.as_ref().map(|field| field.value.clone()),
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
                })
            })
            .collect::<Vec<_>>();
        tasks.sort_by(task_source_order);
        assign_task_parents(&mut tasks);
        sort_task_tree(&mut tasks, QuerySort::Priority, &HashMap::new());
        TaskSnapshot {
            revision: self.revision,
            tasks,
        }
    }

    pub fn events(&self) -> EventSnapshot {
        let mut events = self
            .workspace
            .documents()
            .filter_map(|entry| entry.current.as_ref().map(|current| (entry, current)))
            .flat_map(|(entry, current)| {
                let document_id = self.document_id(&entry.path).map(str::to_string);
                current
                    .output
                    .events
                    .events
                    .iter()
                    .filter_map(move |event| {
                        let document_id = document_id.clone()?;
                        let uid = event.uid.as_ref().map(|field| field.value.clone());
                        Some(WebEvent {
                            key: uid
                                .clone()
                                .unwrap_or_else(|| format!("{document_id}:{}", event.range.start)),
                            document_id,
                            path: display_path(&self.root, &entry.path),
                            revision: current.revision.to_string(),
                            title: event.title.clone(),
                            details: event.details.clone(),
                            id: event.id.as_ref().map(|field| field.value.clone()),
                            uid,
                            start: event.start.as_ref().map(|field| field.value.clone()),
                            end: event.end.as_ref().map(|field| field.value.clone()),
                            tasks: event
                                .tasks
                                .iter()
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
                        })
                    })
            })
            .collect::<Vec<_>>();
        events.sort_by(|left, right| {
            left.start
                .as_deref()
                .and_then(|start| chrono::DateTime::parse_from_rfc3339(start).ok())
                .cmp(
                    &right
                        .start
                        .as_deref()
                        .and_then(|start| chrono::DateTime::parse_from_rfc3339(start).ok()),
                )
                .then(left.path.cmp(&right.path))
                .then(left.locator.start.cmp(&right.locator.start))
        });
        EventSnapshot {
            revision: self.revision,
            events,
            documents: self
                .document_ids
                .iter()
                .filter_map(|(path, id)| {
                    let entry = self.workspace.get(path)?;
                    entry.current.as_ref()?;
                    Some(WebEventDocument {
                        id: id.clone(),
                        path: display_path(&self.root, path),
                        revision: entry.revision.to_string(),
                    })
                })
                .collect(),
        }
    }

    pub fn query_tasks(&self, query: &WebQuery) -> Result<TaskQuerySnapshot, QueryFailure> {
        let now = Local::now().fixed_offset();
        let preset_groups = resolve_presets(&query.presets, TASK_PRESETS)?;
        let mut retained: Option<BTreeSet<(String, usize)>> = None;
        for group in preset_groups {
            let mut matching = BTreeSet::new();
            for (source, expression) in group.expressions {
                let records = self
                    .workspace
                    .search_records_filtered(
                        &self.root,
                        Some(SearchRecordKind::Task),
                        "",
                        usize::MAX,
                        now,
                        Some(expression),
                    )
                    .map_err(|message| QueryFailure { source, message })?;
                matching.extend(
                    records
                        .items
                        .into_iter()
                        .map(|record| (record.relative_path, record.range.start)),
                );
            }
            intersect_keys(&mut retained, matching);
        }
        for (source, expression) in custom_filters(query) {
            let records = self
                .workspace
                .search_records_filtered(
                    &self.root,
                    Some(SearchRecordKind::Task),
                    "",
                    usize::MAX,
                    now,
                    Some(expression),
                )
                .map_err(|message| QueryFailure { source, message })?;
            intersect_keys(
                &mut retained,
                records
                    .items
                    .into_iter()
                    .map(|record| (record.relative_path, record.range.start)),
            );
        }

        let all_tasks = self.tasks().tasks;
        let mut tasks = all_tasks.clone();
        let mut scores = HashMap::new();
        let retained = tasks
            .iter()
            .filter_map(|task| {
                let key = (task.path.clone(), task.location.start);
                if retained.as_ref().is_some_and(|items| !items.contains(&key)) {
                    return None;
                }
                let score = search_score(
                    &query.query,
                    &[
                        &task.title,
                        task.id.as_deref().unwrap_or_default(),
                        &task.path,
                    ],
                );
                if let Some(score) = score {
                    scores.insert(task.key.clone(), score);
                    Some(task.key.clone())
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();
        sort_task_tree(&mut tasks, query.sort, &scores);
        tasks.retain(|task| retained.contains(&task.key));
        let limit = query.limit.unwrap_or(usize::MAX);
        let complete = tasks.len() <= limit;
        truncate_complete_task_documents(&mut tasks, limit, |task| &task.path);
        Ok(TaskQuerySnapshot {
            revision: self.revision,
            tasks,
            all_tasks,
            complete,
        })
    }

    pub fn query_graph(
        &self,
        query: &WebQuery,
        excluded: Option<&str>,
    ) -> Result<GraphSnapshot, QueryFailure> {
        let excluded = self
            .excluded_documents(excluded)
            .map_err(|message| QueryFailure {
                source: "exclude".to_string(),
                message,
            })?;
        let mut graph = self.graph_with_excluded(&query.traversal, &excluded, false);
        let mut program_groups = resolve_presets(&query.presets, GRAPH_PRESETS)?
            .into_iter()
            .map(|group| compile_program_group(group.expressions))
            .collect::<Result<Vec<_>, _>>()?;
        program_groups.extend(
            custom_filters(query)
                .map(|expression| compile_program_group(vec![expression]))
                .collect::<Result<Vec<_>, _>>()?,
        );
        let metrics = self.graph_metrics(&graph);
        let mut scores = HashMap::new();
        let mut retained = Vec::new();
        'nodes: for node in std::mem::take(&mut graph.nodes) {
            let Some(score) = search_score(
                &query.query,
                &[&node.title, node.path.as_deref().unwrap_or_default()],
            ) else {
                continue;
            };
            let metric = metrics.get(&node.id).expect("every graph node has metrics");
            for group in &program_groups {
                let mut matches = false;
                for (source, program) in group {
                    match graph_node_matches(program, &node, metric) {
                        Ok(true) => matches = true,
                        Ok(false) => {}
                        Err(message) => {
                            return Err(QueryFailure {
                                source: source.clone(),
                                message,
                            })
                        }
                    }
                }
                if !matches {
                    continue 'nodes;
                }
            }
            scores.insert(node.id.clone(), score);
            retained.push(node);
        }
        graph.nodes = retained;
        let visible = graph
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<BTreeSet<_>>();
        graph.edges.retain(|edge| {
            visible.contains(edge.source.as_str()) && visible.contains(edge.target.as_str())
        });
        graph.nodes.sort_by(|left, right| match query.sort {
            QuerySort::Relevance if !query.query.is_empty() => scores
                .get(&right.id)
                .cmp(&scores.get(&left.id))
                .then_with(|| graph_source_order(left, right)),
            _ => graph_source_order(left, right),
        });
        let limit = query
            .limit
            .unwrap_or(DEFAULT_GRAPH_LIMIT)
            .min(MAX_GRAPH_LIMIT);
        graph.complete &= graph.nodes.len() <= limit;
        graph.nodes.truncate(limit);
        let visible = graph
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<BTreeSet<_>>();
        graph.edges.retain(|edge| {
            visible.contains(edge.source.as_str()) && visible.contains(edge.target.as_str())
        });
        Ok(graph)
    }

    fn graph_metrics(&self, graph: &GraphSnapshot) -> HashMap<String, GraphMetric> {
        let mut metrics = graph
            .nodes
            .iter()
            .map(|node| (node.id.clone(), GraphMetric::default()))
            .collect::<HashMap<_, _>>();
        for edge in &graph.edges {
            if let Some(metric) = metrics.get_mut(&edge.source) {
                metric.outgoing += 1;
                metric.degree += 1;
            }
            if let Some(metric) = metrics.get_mut(&edge.target) {
                metric.incoming += 1;
                metric.degree += 1;
            }
        }
        for task in self.tasks().tasks {
            if let Some(metric) = metrics.get_mut(&task.document_id) {
                metric.task_count += 1;
                if matches!(task.state.as_str(), "ready" | "waiting") {
                    metric.open_task_count += 1;
                }
            }
        }
        metrics
    }

    pub fn set_task_status(
        &self,
        document_id: &str,
        locator: &WebTaskLocator,
        revision: &str,
        status: TaskStatus,
    ) -> Result<(), String> {
        let path = self
            .document_path(document_id)
            .ok_or_else(|| "unknown task document".to_string())?;
        let entry = self
            .workspace
            .documents()
            .find(|entry| entry.path == path)
            .filter(|entry| entry.current.is_some())
            .ok_or_else(|| "task document is invalid".to_string())?;
        if entry.revision.to_string() != revision {
            return Err("task document changed; refresh before retrying".to_string());
        }
        let disk_source = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if disk_source != entry.parsed.source {
            return Err("task document changed on disk; refresh before retrying".to_string());
        }
        let timestamp = Local::now()
            .fixed_offset()
            .to_rfc3339_opts(SecondsFormat::Secs, false);
        let edit = match locator {
            WebTaskLocator::Id { id } => self
                .workspace
                .set_task_status_by_id(path, id, status, &timestamp),
            WebTaskLocator::Offset { offset } => {
                let indexed = entry
                    .current
                    .as_ref()
                    .expect("current output checked")
                    .output
                    .tasks
                    .tasks
                    .iter()
                    .any(|task| task.range.start == *offset);
                if !indexed {
                    return Err("task position changed; refresh before retrying".to_string());
                }
                self.workspace
                    .set_task_status(path, *offset, status, &timestamp)
            }
        }
        .map_err(task_edit_error)?;
        let document = edit
            .document_changes
            .into_iter()
            .find(|document| document.path == path)
            .ok_or_else(|| "task operation produced no document edit".to_string())?;
        let updated = apply_text_edits(disk_source, document.edits)?;
        std::fs::write(path, updated)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))
    }

    pub fn create_event(
        &self,
        document_id: &str,
        revision: &str,
        input: &WebEventInput,
    ) -> Result<(), String> {
        let path = self.guarded_document(document_id, revision, "event")?;
        let source = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let edit = self
            .workspace
            .create_event(path, &event_input(input))
            .map_err(event_edit_error)?;
        self.write_workspace_edit(path, source, edit, "event")
    }

    pub fn update_event(
        &self,
        document_id: &str,
        locator: &WebEventLocator,
        revision: &str,
        input: &WebEventInput,
    ) -> Result<(), String> {
        let path = self.guarded_document(document_id, revision, "event")?;
        let source = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let edit = self
            .workspace
            .update_event(path, locator.start..locator.end, &event_input(input))
            .map_err(event_edit_error)?;
        self.write_workspace_edit(path, source, edit, "event")
    }

    pub fn delete_event(
        &self,
        document_id: &str,
        locator: &WebEventLocator,
        revision: &str,
    ) -> Result<(), String> {
        let path = self.guarded_document(document_id, revision, "event")?;
        let source = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let edit = self
            .workspace
            .delete_event(path, locator.start..locator.end)
            .map_err(event_edit_error)?;
        self.write_workspace_edit(path, source, edit, "event")
    }

    fn guarded_document<'a>(
        &'a self,
        document_id: &str,
        revision: &str,
        kind: &str,
    ) -> Result<&'a Path, String> {
        let path = self
            .document_path(document_id)
            .ok_or_else(|| format!("unknown {kind} document"))?;
        let entry = self
            .workspace
            .documents()
            .find(|entry| entry.path == path)
            .filter(|entry| entry.current.is_some())
            .ok_or_else(|| format!("{kind} document is invalid"))?;
        if entry.revision.to_string() != revision {
            return Err(format!("{kind} document changed; refresh before retrying"));
        }
        let disk_source = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if disk_source != entry.parsed.source {
            return Err(format!(
                "{kind} document changed on disk; refresh before retrying"
            ));
        }
        Ok(path)
    }

    fn write_workspace_edit(
        &self,
        path: &Path,
        source: String,
        edit: plumb_workspace::WorkspaceEdit,
        kind: &str,
    ) -> Result<(), String> {
        let document = edit
            .document_changes
            .into_iter()
            .find(|document| document.path == path)
            .ok_or_else(|| format!("{kind} operation produced no document edit"))?;
        let updated = apply_text_edits(source, document.edits)?;
        std::fs::write(path, updated)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))
    }

    pub fn has_same_documents(&self, other: &Self) -> bool {
        let documents = |workspace: &Workspace| {
            let mut documents = workspace
                .documents()
                .map(|entry| {
                    (
                        entry.path.clone(),
                        entry.revision,
                        entry.parsed.source.clone(),
                    )
                })
                .collect::<Vec<_>>();
            documents.sort_by(|left, right| left.0.cmp(&right.0));
            documents
        };
        documents(&self.workspace) == documents(&other.workspace)
    }

    pub fn note(&self, id: &str) -> Option<NoteDocument> {
        let path = self.document_path(id)?;
        let entry = self.workspace.get(path)?;
        let current = entry.current.as_ref()?;
        let backlinks = self
            .workspace
            .references_to_document(path)
            .into_iter()
            .map(|(source, reference)| {
                SourceLocation::new(&self.root, source, reference.source_range)
            })
            .collect();
        Some(NoteDocument {
            id: id.to_string(),
            title: self.title(path),
            path: display_path(&self.root, path),
            revision: current.revision,
            location: SourceLocation::new(&self.root, path, 0..entry.parsed.source.len()),
            source: entry.parsed.source.clone(),
            backlinks,
        })
    }

    pub fn graph(&self, query: &GraphQuery) -> GraphSnapshot {
        self.graph_with_excluded(query, &BTreeSet::new(), true)
    }

    pub fn graph_excluding(
        &self,
        query: &GraphQuery,
        predicate: Option<&str>,
    ) -> Result<GraphSnapshot, String> {
        let excluded = self.excluded_documents(predicate)?;
        Ok(self.graph_with_excluded(query, &excluded, true))
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
                )?
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
    ) -> GraphSnapshot {
        let (mut nodes, mut edges) = self.full_graph();
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
        GraphSnapshot {
            revision: self.revision,
            nodes: nodes.into_values().collect(),
            edges,
            complete,
        }
    }

    pub fn pandoc_document(&self, id: &str) -> Result<serde_json::Value, String> {
        let note = self
            .note(id)
            .ok_or_else(|| format!("unknown document id '{id}'"))?;
        plumb_export::export(&note.source)
    }

    fn full_graph(&self) -> (BTreeMap<String, GraphNode>, Vec<GraphEdge>) {
        let mut nodes = self
            .document_ids
            .iter()
            .map(|(path, id)| {
                let entry = self.workspace.get(path).expect("indexed document exists");
                (
                    id.clone(),
                    GraphNode {
                        id: id.clone(),
                        title: self.title(path),
                        path: Some(display_path(&self.root, path)),
                        location: Some(SourceLocation::new(
                            &self.root,
                            path,
                            0..entry.parsed.source.len(),
                        )),
                        unresolved: false,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut edges = Vec::new();
        let mut ghost_ids = BTreeMap::<String, String>::new();
        for (path, source_id) in &self.document_ids {
            let entry = self.workspace.get(path).expect("indexed document exists");
            let current = entry
                .current
                .as_ref()
                .expect("document id is current-valid");
            for link in &current.output.links {
                let kind = match link.spelling {
                    LinkSpelling::Explicit => "link",
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
                    self.workspace.resolve_link(path, link),
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
                        self.workspace
                            .resolve_task_reference_at(path, prev.range.start)
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
                        self.workspace
                            .resolve_task_reference_at(path, dependency.range.start)
                            .unwrap_or(ResolvedTarget::Other),
                    );
                }
            }
        }
        (nodes, edges)
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

    fn index_resources(&mut self) {
        let mut paths = BTreeSet::new();
        let canonical_root = self
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone());
        for entry in self.workspace.documents() {
            let Some(current) = &entry.current else {
                continue;
            };
            for link in &current.output.links {
                if let ResolvedTarget::File { path } =
                    self.workspace.resolve_link(&entry.path, link)
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
            self.resources_by_id.insert(id, canonical.clone());
            self.resources.insert(canonical, record);
        }
    }
}

fn custom_filters(query: &WebQuery) -> impl Iterator<Item = (String, &str)> {
    let mut filters = query
        .filters
        .iter()
        .map(String::as_str)
        .filter(|expression| !expression.trim().is_empty())
        .enumerate()
        .map(|(index, expression)| (format!("custom:{}", index + 1), expression))
        .collect::<Vec<_>>();
    if !query.filter.trim().is_empty() {
        filters.push(("custom".to_string(), query.filter.as_str()));
    }
    filters.into_iter()
}

#[derive(Debug, Clone, Default)]
struct GraphMetric {
    degree: usize,
    incoming: usize,
    outgoing: usize,
    task_count: usize,
    open_task_count: usize,
}

struct ResolvedPresetGroup<'a> {
    key: String,
    expressions: Vec<(String, &'a str)>,
}

fn resolve_presets<'a>(
    ids: &[String],
    registry: &'a [QueryPreset],
) -> Result<Vec<ResolvedPresetGroup<'a>>, QueryFailure> {
    let mut groups = Vec::<ResolvedPresetGroup<'a>>::new();
    for id in ids {
        let preset = registry
            .iter()
            .find(|preset| preset.id == id)
            .ok_or_else(|| QueryFailure {
                source: format!("preset:{id}"),
                message: format!("unknown query preset '{id}'"),
            })?;
        let key = preset.group.unwrap_or(preset.id).to_string();
        let expression = (format!("preset:{id}"), preset.expression);
        if let Some(group) = groups.iter_mut().find(|group| group.key == key) {
            group.expressions.push(expression);
        } else {
            groups.push(ResolvedPresetGroup {
                key,
                expressions: vec![expression],
            });
        }
    }
    Ok(groups)
}

fn compile_program_group(
    expressions: Vec<(String, &str)>,
) -> Result<Vec<(String, Program)>, QueryFailure> {
    expressions
        .into_iter()
        .map(|(source, expression)| {
            Program::compile(expression)
                .map(|program| (source.clone(), program))
                .map_err(|error| QueryFailure {
                    source,
                    message: format!("invalid CEL query: {error}"),
                })
        })
        .collect()
}

fn intersect_keys<T: Ord>(retained: &mut Option<BTreeSet<T>>, values: impl IntoIterator<Item = T>) {
    let values = values.into_iter().collect::<BTreeSet<_>>();
    match retained {
        Some(retained) => retained.retain(|value| values.contains(value)),
        None => *retained = Some(values),
    }
}

fn graph_node_matches(
    program: &Program,
    node: &GraphNode,
    metric: &GraphMetric,
) -> Result<bool, String> {
    let mut context = Context::default();
    context.add_variable_from_value(
        "path",
        node.path
            .clone()
            .map_or(Value::Null, |value| Value::String(value.into())),
    );
    context.add_variable_from_value("title", node.title.clone());
    context.add_variable_from_value("unresolved", node.unresolved);
    context.add_variable_from_value("degree", i64::try_from(metric.degree).unwrap_or(i64::MAX));
    context.add_variable_from_value(
        "incoming",
        i64::try_from(metric.incoming).unwrap_or(i64::MAX),
    );
    context.add_variable_from_value(
        "outgoing",
        i64::try_from(metric.outgoing).unwrap_or(i64::MAX),
    );
    context.add_variable_from_value(
        "task_count",
        i64::try_from(metric.task_count).unwrap_or(i64::MAX),
    );
    context.add_variable_from_value(
        "open_task_count",
        i64::try_from(metric.open_task_count).unwrap_or(i64::MAX),
    );
    match program.execute(&context) {
        Ok(Value::Bool(value)) => Ok(value),
        Ok(value) => Err(format!("CEL query must return bool, got {value:?}")),
        Err(error) => Err(format!(
            "cannot evaluate CEL query for '{}': {error}",
            node.title
        )),
    }
}

fn graph_source_order(left: &GraphNode, right: &GraphNode) -> std::cmp::Ordering {
    left.path
        .as_deref()
        .unwrap_or("\u{10ffff}")
        .cmp(right.path.as_deref().unwrap_or("\u{10ffff}"))
        .then(left.id.cmp(&right.id))
}

fn task_source_order(left: &WebTask, right: &WebTask) -> std::cmp::Ordering {
    left.path
        .cmp(&right.path)
        .then(left.location.start.cmp(&right.location.start))
        .then(left.key.cmp(&right.key))
}

fn assign_task_parents(tasks: &mut [WebTask]) {
    let mut path = None::<String>;
    let mut ancestors = Vec::<String>::new();
    for task in tasks {
        if path.as_deref() != Some(task.path.as_str()) {
            path = Some(task.path.clone());
            ancestors.clear();
        }
        ancestors.truncate(task.depth);
        ancestors.resize(task.depth, String::new());
        task.parent_key = task
            .depth
            .checked_sub(1)
            .and_then(|depth| ancestors.get(depth).filter(|key| !key.is_empty()).cloned());
        ancestors.push(task.key.clone());
    }
}

fn sort_task_tree(tasks: &mut Vec<WebTask>, sort: QuerySort, scores: &HashMap<String, i64>) {
    let order = match sort {
        QuerySort::Source => TaskSortOrder::Source,
        QuerySort::Priority => TaskSortOrder::Priority,
        QuerySort::Due => TaskSortOrder::Due,
        QuerySort::Relevance => TaskSortOrder::Relevance,
    };
    sort_task_records(tasks, order, |task| TaskSortFacts {
        document: task.path.clone(),
        source_start: task.location.start,
        depth: task.depth,
        priority: task.priority,
        due: task
            .due
            .as_deref()
            .and_then(|due| chrono::DateTime::parse_from_rfc3339(due).ok()),
        relevance: scores.get(&task.key).copied(),
    });
}

fn file_revision(path: &Path) -> Option<i64> {
    let metadata = path.metadata().ok()?;
    let modified = metadata.modified().ok()?;
    let duration = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(duration.as_nanos().min(i64::MAX as u128) as i64)
}

fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

fn display_task_ref(root: &Path, target: &TaskRef) -> String {
    format!("{}#{}", display_path(root, &target.path), target.id)
}

fn apply_text_edits(mut source: String, mut edits: Vec<TextEdit>) -> Result<String, String> {
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
    let mut previous_start = source.len();
    for edit in edits {
        if edit.range.end > previous_start || edit.range.end > source.len() {
            return Err("task edits overlap or fall outside the document".to_string());
        }
        previous_start = edit.range.start;
        source.replace_range(edit.range, &edit.new_text);
    }
    Ok(source)
}

fn task_edit_error(error: TaskEditError) -> String {
    match error {
        TaskEditError::StaleOrInvalidDocument => "task document is invalid",
        TaskEditError::TaskNotFound => "task id was not found",
        TaskEditError::TaskAlreadyClosed => "task is already closed",
        TaskEditError::TaskBlocked => "task is blocked by open dependencies",
        TaskEditError::InvalidRecurrence => "task recurrence is invalid",
        TaskEditError::InvalidTimestamp => "operation timestamp is invalid",
        TaskEditError::ListItemNotFound => "task list item was not found",
        TaskEditError::TaskAlreadyExists => "the list item is already a task",
        TaskEditError::CreatedAlreadyExists => "the task already has a created timestamp",
        TaskEditError::GeneratedInvalid => "the generated task edit is invalid",
    }
    .to_string()
}

fn event_input(input: &WebEventInput) -> EventInput {
    EventInput {
        title: input.title.clone(),
        start: input.start.clone(),
        end: input.end.clone().filter(|end| !end.is_empty()),
        tasks: input.tasks.clone(),
    }
}

fn event_edit_error(error: EventEditError) -> String {
    match error {
        EventEditError::StaleOrInvalidDocument => "event document is stale or invalid",
        EventEditError::EventNotFound => "event is no longer available",
        EventEditError::InvalidDatetime => "event start and end must be RFC 3339 timestamps",
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
    fn builds_graph_with_links_tasks_ghosts_and_bounded_neighborhoods() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("a.plumb"),
            "`meta\n `: title\n\n    Alpha\n\n`-{.task #old} Old\n`-{.task #a prev=\"b.plumb#b\" depends=\"b.plumb#b\"} A\n`-{.task #recur prev=\"#old\"} Recurring instance\nSee `->[B]{to=\"b.plumb#b\"}, `[b.plumb#b]{.->}, `->[self]{to=\"#a\"}, `->[self again]{to=\"#a\"}, and `->[missing]{to=\"missing.plumb\"}.\n",
        )
        .unwrap();
        std::fs::write(root.join("b.plumb"), "`-{.task #b} Beta\n").unwrap();
        std::fs::write(root.join("broken.plumb"), "`node{key=a key=b} Broken\n").unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let graph = workspace.graph(&GraphQuery::default());
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
        let limited = workspace.graph(&GraphQuery {
            limit: Some(1),
            ..GraphQuery::default()
        });
        assert!(!limited.complete);
        assert_eq!(limited.nodes.len(), 1);

        let alpha = workspace
            .document_id(root.join("a.plumb"))
            .unwrap()
            .to_string();
        let local = workspace.graph(&GraphQuery {
            current: Some(alpha),
            depth: Some(0),
            ..GraphQuery::default()
        });
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
        source_workspace.insert(&path, 9, "`meta\n `: title\n\n    Open buffer title\n");
        let web = WebWorkspace::from_workspace(&root, source_workspace, 4).unwrap();
        let graph = web.graph(&GraphQuery::default());
        assert_eq!(graph.revision, 4);
        assert_eq!(graph.nodes[0].title, "Open buffer title");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn task_snapshots_expose_workspace_facts_and_status_edits() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tasks.plumb");
        std::fs::write(
            &path,
            "`-{.task #ship priority=8 created=\"2026-07-25T10:00:00+08:00\" due=\"2099-02-01T10:00:00+08:00\"} Ship release\n  `-{.task #child priority=2 due=\"2099-01-01T10:00:00+08:00\"} Child stays with ship\n  `-{.task #urgent-child priority=9} Urgent child\n`-{.task #later priority=3 wait=\"2099-01-10T00:00:00+08:00\" due=\"2099-01-15T00:00:00+08:00\"} Later\n`-{.task #broken done=\"2026-07-25T11:00:00+08:00\" canceled=\"2026-07-25T12:00:00+08:00\"} Broken\n",
        )
        .unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let snapshot = workspace.tasks();
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
        assert_eq!(broken.state, "invalid");
        workspace
            .set_task_status(
                &task.document_id,
                &task.locator,
                &task.revision,
                TaskStatus::Done,
            )
            .unwrap();
        let updated = std::fs::read_to_string(&path).unwrap();
        assert!(updated.contains("done=\"2026-"), "{updated}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn event_snapshots_and_guarded_mutations_preserve_uid() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("agenda.plumb");
        std::fs::write(&path, "`# Agenda\n").unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let document = workspace.events().documents[0].clone();
        workspace
            .create_event(
                &document.id,
                &document.revision,
                &WebEventInput {
                    title: "Review".to_string(),
                    start: "2026-07-30T06:00:00Z".to_string(),
                    end: Some("2026-07-30T07:00:00Z".to_string()),
                    tasks: vec!["#write".to_string()],
                },
            )
            .unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let event = workspace.events().events[0].clone();
        let uid = event.uid.clone().unwrap();
        workspace
            .update_event(
                &event.document_id,
                &event.locator,
                &event.revision,
                &WebEventInput {
                    title: "Updated".to_string(),
                    start: "2026-07-30T08:00:00Z".to_string(),
                    end: None,
                    tasks: Vec::new(),
                },
            )
            .unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let updated = workspace.events().events[0].clone();
        assert_eq!(updated.uid.as_deref(), Some(uid.as_str()));
        assert_eq!(updated.title, "Updated");
        assert_eq!(updated.end, None);
        assert_eq!(
            workspace.update_event(
                &updated.document_id,
                &updated.locator,
                "stale",
                &WebEventInput {
                    title: "Conflict".to_string(),
                    start: "2026-07-30T08:00:00Z".to_string(),
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
            "`-{.event start=\"2026-07-30T10:30:00+05:00\"} Early\n`-{.event start=\"2026-07-30T06:00:00Z\"} Later\n",
        )
        .unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        assert_eq!(
            workspace
                .events()
                .events
                .iter()
                .map(|event| event.title.as_str())
                .collect::<Vec<_>>(),
            ["Early", "Later"]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn idless_tasks_use_exact_indexed_offsets_for_status_edits() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tasks.plumb");
        let source =
            "`-{.task created=\"2026-07-25T10:00:00+08:00\"} 完成任务后查看修改后任务的内容\n";
        std::fs::write(&path, source).unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let task = workspace.tasks().tasks.into_iter().next().unwrap();
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
        assert!(updated.contains("done=\"2026-"), "{updated}");
        let refreshed = WebWorkspace::load_with_revision(&root, 2).unwrap();
        let completed = refreshed.tasks().tasks.into_iter().next().unwrap();
        assert_eq!(completed.key, task.key);
        assert_eq!(completed.state, "done");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn web_queries_compose_filters_and_preserve_task_subtrees() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("a.plumb"),
            "`-{.task #parent priority=3 due=\"2099-02-01T00:00:00Z\"} Parent\n  `-{.task #matching priority=20 due=\"2099-01-01T00:00:00Z\"} Needle child\n  `-{.task #quiet priority=30} Quiet child\n`-{.task #first priority=8 due=\"2099-01-15T00:00:00Z\"} Needle first\n`-{.task #done done=\"2026-07-27T00:00:00Z\"} Needle done\n",
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
        assert_eq!(source.all_tasks.len(), 5);
        let matching = source
            .all_tasks
            .iter()
            .find(|task| task.id.as_deref() == Some("matching"))
            .unwrap();
        assert_eq!(
            source
                .all_tasks
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
                sort: QuerySort::Due,
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
                sort: QuerySort::Priority,
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
                sort: QuerySort::Priority,
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
            "`-{.task #a-late priority=-5 due=\"2099-03-01T00:00:00Z\"} A late\n`-{.task #a-promoted priority=-10} A promoted\n  `-{.task #a-urgent priority=30} A urgent\n`-{.task #a-early due=\"2099-01-01T00:15:00-01:00\"} A early\n",
        )
        .unwrap();
        std::fs::write(
            root.join("b.plumb"),
            "`-{.task #b-high priority=20 due=\"2099-01-01T01:00:00+02:00\"} B high\n`-{.task #b-other priority=15} B other\n",
        )
        .unwrap();
        std::fs::write(
            root.join("c.plumb"),
            "`-{.task #c-negative priority=-1} C negative\n",
        )
        .unwrap();
        std::fs::write(root.join("d.plumb"), "`-{.task #d-default} D default\n").unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();

        let priority = workspace
            .query_tasks(&WebQuery {
                view: WebView::Tasks,
                sort: QuerySort::Priority,
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
                sort: QuerySort::Due,
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
        let limited = workspace
            .query_tasks(&WebQuery {
                view: WebView::Tasks,
                sort: QuerySort::Priority,
                limit: Some(1),
                ..WebQuery::default()
            })
            .unwrap();
        assert!(!limited.complete);
        assert_eq!(limited.tasks.len(), 4);
        assert!(limited.tasks.iter().all(|task| task.path == "a.plumb"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn graph_queries_filter_after_traversal_and_remove_hidden_endpoints() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.plumb"), "`->[B]{to=\"b.plumb\"}\n").unwrap();
        std::fs::write(
            root.join("b.plumb"),
            "`->[C]{to=\"c.plumb\"}\n`-{.task} Work\n",
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
        assert_eq!(first.graph(&GraphQuery::default()).nodes.len(), 1);
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
            "`img[]{src=\"static/image.jpg\"}\n",
        )
        .unwrap();
        symlink(&target, &root).unwrap();

        let workspace = WebWorkspace::load(&root).unwrap();
        let resource = workspace.resources().next().expect("indexed image");
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
            "`-{.task #blocker} Blocker\n`-{.task #dependent depends=\"#blocker\"} Dependent\n`-{.task #recurring due=\"2026-07-20T09:00:00+08:00\" recur=P1D} Recurring\n`-{.task #invalid done=\"2026-07-20T10:00:00+08:00\" canceled=\"2026-07-20T11:00:00+08:00\"} Invalid\n",
        )
        .unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let snapshot = workspace.tasks();
        let dependent = snapshot
            .tasks
            .iter()
            .find(|task| task.id.as_deref() == Some("dependent"))
            .unwrap();
        assert_eq!(dependent.state, "waiting");
        assert_eq!(dependent.wait_reasons, ["dependency"]);
        assert_eq!(
            snapshot
                .tasks
                .iter()
                .find(|task| task.id.as_deref() == Some("invalid"))
                .unwrap()
                .state,
            "invalid"
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
            .tasks
            .iter()
            .any(|task| { task.prev.as_deref() == Some("#recurring") && task.state == "ready" }));

        std::fs::remove_file(&path).unwrap();
        assert!(WebWorkspace::load_with_revision(&root, 3)
            .unwrap()
            .tasks()
            .tasks
            .is_empty());
        std::fs::write(&path, "`-{.task} Ignored\n").unwrap();
        std::fs::write(root.join(".ignore"), "tasks.plumb\n").unwrap();
        assert!(WebWorkspace::load_with_revision(&root, 4)
            .unwrap()
            .tasks()
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
