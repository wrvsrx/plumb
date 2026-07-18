use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use plumb_core::{parse, Diagnostic, DiagnosticSeverity, ParsedDocument};
use plumb_extensions::{analyze_document, AnchorRecord, DocumentOutput, LinkRecord, LinkTarget};

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
}
