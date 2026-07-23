use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::ops::Range;
use std::path::{Path, PathBuf};

use chrono::Local;
use plumb_extensions::LinkSpelling;
use plumb_workspace::{normalize, ResolvedTarget, SearchRecordKind, Workspace};
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
        let mut paths = Vec::new();
        collect_plumb_files(&root, &mut paths)?;
        paths.sort();
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
        let (mut nodes, mut edges) = self.full_graph();
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

        let limit = query
            .limit
            .unwrap_or(DEFAULT_GRAPH_LIMIT)
            .min(MAX_GRAPH_LIMIT);
        let complete = nodes.len() <= limit;
        let retained = nodes.keys().take(limit).cloned().collect::<BTreeSet<_>>();
        nodes.retain(|id, _| retained.contains(id));
        edges.retain(|edge| retained.contains(&edge.source) && retained.contains(&edge.target));
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
        }
        for path in paths {
            let Ok(canonical) = path.canonicalize() else {
                continue;
            };
            if !canonical.starts_with(&self.root) || !canonical.is_file() {
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

fn collect_plumb_files(path: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(path)
        .map_err(|error| format!("cannot read directory {}: {error}", path.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot read directory entry: {error}"))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot stat {}: {error}", path.display()))?;
        if file_type.is_dir() {
            collect_plumb_files(&path, output)?;
        } else if (file_type.is_file() || file_type.is_symlink())
            && path
                .extension()
                .is_some_and(|extension| extension == "plumb")
        {
            output.push(normalize(&path));
        }
    }
    Ok(())
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
            "`meta\n `: title\n\n    Alpha\n\n`-{.task #old} Old\n`-{.task #a prev=\"#old\" depends=\"b.plumb#b\"} A\nSee `->[B]{to=\"b.plumb#b\"}, `[b.plumb#b]{.->}, `->[self]{to=\"#a\"}, `->[self again]{to=\"#a\"}, and `->[missing]{to=\"missing.plumb\"}.\n",
        )
        .unwrap();
        std::fs::write(root.join("b.plumb"), "`#{#b} Beta\n").unwrap();
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
            4
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
        assert_eq!(local.edges.len(), 3);
        assert!(local.edges.iter().all(|edge| edge.source == edge.target));
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

    fn temp_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "plumb-web-model-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
