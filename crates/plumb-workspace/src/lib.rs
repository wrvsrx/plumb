use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use plumb_core::{parse, Diagnostic, DiagnosticSeverity, ParsedDocument};
use plumb_extensions::{
    analyze_document, AnchorRecord, DocumentOutput, LinkCompletionContext, LinkRecord, LinkTarget,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub range: std::ops::Range<usize>,
    pub new_text: String,
}

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

#[derive(Debug, Default)]
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

    pub fn anchor_at(&self, path: impl AsRef<Path>, offset: usize) -> Option<&AnchorRecord> {
        self.current_output(path.as_ref())?
            .anchors
            .iter()
            .filter(|anchor| anchor.range.start <= offset && offset <= anchor.range.end)
            .max_by_key(|anchor| anchor.range.start)
    }

    pub fn references_to(
        &self,
        target_path: impl AsRef<Path>,
        target_id: &str,
    ) -> Vec<(&Path, &LinkRecord)> {
        let target_path = normalize(target_path.as_ref());
        let mut references = Vec::new();
        for entry in self.documents.values() {
            let Some(current) = &entry.current else {
                continue;
            };
            for link in &current.output.links {
                if matches!(
                    self.resolve_link(&entry.path, link),
                    ResolvedTarget::Anchor { path, ref id, .. }
                        if path == target_path && id == target_id
                ) {
                    references.push((entry.path.as_path(), link));
                }
            }
        }
        references.sort_by(|left, right| {
            left.0
                .cmp(right.0)
                .then(left.1.range.start.cmp(&right.1.range.start))
        });
        references
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
        diagnostics
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
        let link = output
            .links
            .iter()
            .find(|link| {
                link.fragment_range
                    .as_ref()
                    .is_some_and(|range| contains_inclusive(range, offset))
            })
            .ok_or(RenameError::NotRenameable)?;
        let range = link
            .fragment_range
            .clone()
            .ok_or(RenameError::NotRenameable)?;
        match self.resolve_link(&path, link) {
            ResolvedTarget::Anchor { path, id, .. } => Ok(RenameTarget { path, id, range }),
            _ => Err(RenameError::NotRenameable),
        }
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
            .push(TextEdit {
                range: anchor.id.range.clone(),
                new_text: replacement.to_string(),
            });
        for (path, link) in self.references_to(&target.path, &target.id) {
            let Some(range) = &link.fragment_range else {
                continue;
            };
            grouped
                .entry(path.to_path_buf())
                .or_default()
                .push(TextEdit {
                    range: range.clone(),
                    new_text: replacement.to_string(),
                });
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
        let link = self
            .current_output(&path)
            .and_then(|output| {
                output.links.iter().find(|link| {
                    link.path_range
                        .as_ref()
                        .is_some_and(|range| contains_inclusive(range, offset))
                })
            })
            .ok_or(RenameError::NotRenameable)?;
        let old_path = match self.resolve_link(&path, link) {
            ResolvedTarget::Anchor { path, .. } | ResolvedTarget::Document { path } => path,
            _ => return Err(RenameError::NotRenameable),
        };
        Ok(PathRenameTarget {
            old_path,
            range: link.path_range.clone().ok_or(RenameError::NotRenameable)?,
        })
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
                    .push(TextEdit {
                        range: path_range.clone(),
                        new_text: replacement,
                    });
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

        let newline = if entry.parsed.source.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let escaped_title = title.replace('`', "``");
        let escaped_created = created.replace('`', "``");
        let new_text = format!(
            "`meta{newline}  `: title{newline}{newline}     {escaped_title}{newline}{newline}  `: created{newline}{newline}     {escaped_created}{newline}{newline}"
        );
        Ok(WorkspaceEdit {
            document_changes: vec![DocumentEdit {
                path,
                expected_revision: entry.revision,
                edits: vec![TextEdit {
                    range: 0..0,
                    new_text,
                }],
            }],
            resource_operations: Vec::new(),
        })
    }

    pub fn complete_link(
        &self,
        from: impl AsRef<Path>,
        context: &LinkCompletionContext,
    ) -> Vec<CompletionCandidate> {
        let from = normalize(from.as_ref());
        let mut candidates: Vec<CompletionCandidate> = match context {
            LinkCompletionContext::Path { replace, query } => self
                .documents
                .values()
                .filter(|entry| entry.current.is_some() || entry.last_valid.is_some())
                .filter(|entry| entry.path != from)
                .filter_map(|entry| {
                    let relative = relative_path(&from, &entry.path)?;
                    fuzzy_match(&relative, query).then(|| CompletionCandidate {
                        label: relative.clone(),
                        detail: relative.clone(),
                        new_text: relative,
                        replace: replace.clone(),
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
        };
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

fn contains_inclusive(range: &std::ops::Range<usize>, offset: usize) -> bool {
    range.start <= offset && offset <= range.end
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_same_and_cross_file_explicit_anchors() {
        let mut workspace = Workspace::new();
        workspace.insert("notes/a.plumb", 1, "`#{#local} Local\n");
        workspace.insert(
            "notes/b.plumb",
            1,
            "See `link[local]{to=\"a.plumb#local\"}.\n",
        );
        let link = &workspace
            .get("notes/b.plumb")
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .output
            .links[0];
        assert!(matches!(
            workspace.resolve_link("notes/b.plumb", link),
            ResolvedTarget::Anchor { ref id, .. } if id == "local"
        ));
    }

    #[test]
    fn headings_without_ids_do_not_resolve() {
        let mut workspace = Workspace::new();
        workspace.insert(
            "a.plumb",
            1,
            "`# No anchor\nSee `link[x]{to=\"#No-anchor\"}.\n",
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
        workspace.insert("b.plumb", 1, "`link[x]{to=\"a.plumb#target\"}\n");
        assert_eq!(workspace.references_to("a.plumb", "target").len(), 1);
    }

    #[test]
    fn rename_updates_declaration_and_cross_file_fragments() {
        let mut workspace = Workspace::new();
        workspace.insert("a.plumb", 4, "`#{#target} Target\n");
        workspace.insert("b.plumb", 7, "`link[x]{to=\"a.plumb#target\"}\n");
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
        workspace.insert("notes/current.plumb", 1, "Current\n");
        workspace.insert("notes/design.plumb", 1, "`# No id\n`##{#api} API\n");
        let paths = workspace.complete_link(
            "notes/current.plumb",
            &LinkCompletionContext::Path {
                replace: 10..13,
                query: "design".to_string(),
            },
        );
        assert_eq!(paths[0].label, "design.plumb");
        assert_eq!(paths[0].new_text, "design.plumb");
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
    fn document_rename_rewrites_incoming_and_outgoing_relative_paths() {
        let mut workspace = Workspace::new();
        workspace.insert(
            "notes/a.plumb",
            1,
            "`#{#a} A\n`link[c]{to=\"../shared/c.plumb#c\"}\n",
        );
        workspace.insert("notes/b.plumb", 2, "`link[a]{to=\"a.plumb#a\"}\n");
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
            "`meta\n  `: title\n\n     my``note\n\n  `: created\n\n     2026-07-19T12:34:56+08:00\n\n"
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
            "`meta\r\n  `: title\r\n\r\n     note\r\n\r\n  `: created\r\n\r\n     2026-07-19T12:34:56+08:00\r\n\r\n"
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
}
