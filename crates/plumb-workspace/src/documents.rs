use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use plumb_semantics::{analyze_green_document, DocumentChange};
use plumb_syntax::{GreenDocument, SourceChange};

use crate::{
    normalize, DocumentEntry, DocumentRevision, PendingDocumentAnalysis, PreparedDocumentAnalysis,
    QueryCompleteness, QueryProvenance, QueryResult, SqliteSemanticStore, StoreError,
    VersionedDocumentOutput, Workspace, WorkspaceQueryError,
};

impl Workspace {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_sqlite_store(store: SqliteSemanticStore) -> Self {
        Self {
            documents: Default::default(),
            disk_store: Some(store),
        }
    }

    pub(crate) fn query_result<T>(&self, value: T) -> QueryResult<T> {
        let provenance = match (self.disk_store.is_some(), self.documents.is_empty()) {
            (false, _) => QueryProvenance::Memory,
            (true, true) => QueryProvenance::Persistent,
            (true, false) => QueryProvenance::PersistentWithOverlay,
        };
        QueryResult {
            value,
            completeness: if self.semantic_generation_pending() {
                QueryCompleteness::Partial
            } else {
                QueryCompleteness::Complete
            },
            provenance,
        }
    }

    fn semantic_generation_pending(&self) -> bool {
        self.documents
            .values()
            .any(|entry| entry.parsed.is_valid() && entry.current.is_none())
    }

    pub fn insert_disk(
        &mut self,
        path: impl AsRef<Path>,
        revision: i64,
        source: impl Into<String>,
    ) -> Result<bool, StoreError> {
        let path = normalize(path.as_ref());
        let source = source.into();
        let Some(store) = &self.disk_store else {
            self.insert(path, revision, source);
            return Ok(false);
        };
        if store.contains_current(&path, &source)? {
            return Ok(true);
        }
        let green = Arc::new(GreenDocument::parse(source));
        let output = green
            .valid_syntax()
            .and_then(|valid| analyze_green_document(valid, Arc::clone(&green)));
        store.replace(&path, revision, green.source(), output.as_ref())?;
        Ok(false)
    }

    pub fn open_document(
        &mut self,
        path: impl AsRef<Path>,
        revision: i64,
        source: impl Into<String>,
    ) -> &DocumentEntry {
        self.insert(path, revision, source)
    }

    pub fn close_document(&mut self, path: impl AsRef<Path>) -> Option<DocumentEntry> {
        if self.disk_store.is_some() {
            self.documents.remove(&normalize(path.as_ref()))
        } else {
            None
        }
    }

    pub fn remove_disk(&mut self, path: impl AsRef<Path>) -> Result<(), StoreError> {
        let path = normalize(path.as_ref());
        if let Some(store) = &self.disk_store {
            store.remove(&path)?;
        } else {
            self.documents.remove(&path);
        }
        Ok(())
    }

    pub fn has_persistent_store(&self) -> bool {
        self.disk_store.is_some()
    }

    pub fn insert(
        &mut self,
        path: impl AsRef<Path>,
        revision: i64,
        source: impl Into<String>,
    ) -> &DocumentEntry {
        if let Some(pending) = self.begin_document_revision(path.as_ref(), revision, source) {
            let installed = self.install_document_analysis(pending.analyze());
            debug_assert!(
                installed,
                "synchronous analysis must install its own revision"
            );
        }
        self.documents
            .get(&normalize(path.as_ref()))
            .expect("just inserted")
    }

    pub fn begin_document_revision(
        &mut self,
        path: impl AsRef<Path>,
        revision: i64,
        source: impl Into<String>,
    ) -> Option<PendingDocumentAnalysis> {
        self.begin_document_revision_with_change(path, revision, source, None)
    }

    pub fn begin_document_revision_with_change(
        &mut self,
        path: impl AsRef<Path>,
        revision: i64,
        source: impl Into<String>,
        source_change: Option<SourceChange>,
    ) -> Option<PendingDocumentAnalysis> {
        let path = normalize(path.as_ref());
        let source = source.into();
        let previous = self.documents.get(&path);
        let previous_output = previous
            .and_then(|entry| entry.current.as_ref())
            .map(|current| Arc::clone(&current.output));
        let (green, change) = match previous {
            Some(entry) => {
                let incremental = match source_change {
                    Some(change) => entry.parsed.green().reparse_from_change(source, change),
                    None => entry.parsed.green().reparse(source),
                };
                let change = DocumentChange {
                    old_range: incremental.old_reparsed_range,
                    new_range: incremental.reparsed_range,
                };
                (Arc::new(incremental.document), Some(change))
            }
            None => (Arc::new(GreenDocument::parse(source)), None),
        };
        let parsed = Arc::new(DocumentRevision::from_green(green));
        let previous_last_valid = self
            .documents
            .get(&path)
            .and_then(|entry| entry.last_valid.clone());
        self.documents.insert(
            path.clone(),
            DocumentEntry {
                path: path.clone(),
                revision,
                parsed: Arc::clone(&parsed),
                current: None,
                last_valid: previous_last_valid,
            },
        );
        parsed.is_valid().then_some(PendingDocumentAnalysis {
            path,
            revision,
            parsed,
            previous_output,
            change,
        })
    }

    pub fn install_document_analysis(&mut self, analysis: PreparedDocumentAnalysis) -> bool {
        let Some(entry) = self.documents.get_mut(&analysis.path) else {
            return false;
        };
        if entry.revision != analysis.revision || !Arc::ptr_eq(&entry.parsed, &analysis.parsed) {
            return false;
        }
        let current = Arc::new(VersionedDocumentOutput {
            revision: analysis.revision,
            output: analysis.output,
        });
        entry.current = Some(Arc::clone(&current));
        entry.last_valid = Some(current);
        true
    }

    pub fn document_analysis_pending(&self, path: impl AsRef<Path>) -> bool {
        self.documents
            .get(&normalize(path.as_ref()))
            .is_some_and(|entry| entry.parsed.is_valid() && entry.current.is_none())
    }

    pub fn complete_pending_document_analysis(&mut self, path: impl AsRef<Path>) -> bool {
        let path = normalize(path.as_ref());
        let Some(entry) = self.documents.get(&path) else {
            return false;
        };
        if !entry.parsed.is_valid() || entry.current.is_some() {
            return false;
        }
        let pending = PendingDocumentAnalysis {
            path,
            revision: entry.revision,
            parsed: Arc::clone(&entry.parsed),
            previous_output: None,
            change: None,
        };
        self.install_document_analysis(pending.analyze())
    }

    pub fn rebind_revision_if_source(
        &mut self,
        path: impl AsRef<Path>,
        revision: i64,
        source: &str,
    ) -> bool {
        let path = normalize(path.as_ref());
        let Some(entry) = self.documents.get_mut(&path) else {
            return false;
        };
        if entry.parsed.source() != source {
            return false;
        }
        if entry.parsed.is_valid() && entry.current.is_none() {
            return false;
        }
        entry.revision = revision;
        let Some(current) = entry.current.take() else {
            return true;
        };
        entry.last_valid = None;
        let rebound = match Arc::try_unwrap(current) {
            Ok(mut output) => {
                output.revision = revision;
                Arc::new(output)
            }
            Err(output) => Arc::new(VersionedDocumentOutput {
                revision,
                output: Arc::clone(&output.output),
            }),
        };
        entry.current = Some(rebound.clone());
        entry.last_valid = Some(rebound);
        true
    }

    pub fn overlay_document_entry(&mut self, entry: DocumentEntry) {
        self.documents.insert(normalize(&entry.path), entry);
    }

    pub fn remove(&mut self, path: impl AsRef<Path>) -> Option<DocumentEntry> {
        self.documents.remove(&normalize(path.as_ref()))
    }

    pub fn get(&self, path: impl AsRef<Path>) -> Option<&DocumentEntry> {
        self.documents.get(&normalize(path.as_ref()))
    }

    pub fn document_from_source(
        &self,
        path: impl AsRef<Path>,
        source: &str,
    ) -> Result<Option<DocumentEntry>, WorkspaceQueryError> {
        let path = normalize(path.as_ref());
        if let Some(entry) = self.documents.get(&path) {
            return Ok((entry.parsed.source() == source).then(|| entry.clone()));
        }
        let Some(store) = &self.disk_store else {
            return Ok(None);
        };
        let Some(document) = store.document(&path)? else {
            return Ok(None);
        };
        if document.content_hash != SqliteSemanticStore::content_hash(source) {
            return Ok(None);
        }
        Ok(Some(Self::materialize_document(
            path,
            document.revision,
            source,
        )))
    }

    pub fn materialize_document(
        path: impl AsRef<Path>,
        revision: i64,
        source: &str,
    ) -> DocumentEntry {
        document_entry_from_source(normalize(path.as_ref()), revision, source)
    }

    pub fn contains(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<QueryResult<bool>, WorkspaceQueryError> {
        Ok(self.query_result(self.contains_path(path.as_ref())?))
    }

    pub(crate) fn contains_path(&self, path: &Path) -> Result<bool, WorkspaceQueryError> {
        let path = normalize(path);
        let contains = if self.documents.contains_key(&path) {
            true
        } else if let Some(store) = &self.disk_store {
            store.document_exists(&path)?
        } else {
            false
        };
        Ok(contains)
    }

    pub fn documents(&self) -> impl Iterator<Item = &DocumentEntry> {
        self.documents.values()
    }

    pub fn document_paths(&self) -> Result<QueryResult<Vec<PathBuf>>, WorkspaceQueryError> {
        let mut paths = self.documents.keys().cloned().collect::<HashSet<_>>();
        if let Some(store) = &self.disk_store {
            paths.extend(store.document_paths()?);
        }
        let mut paths = paths.into_iter().collect::<Vec<_>>();
        paths.sort();
        Ok(self.query_result(paths))
    }
}

fn document_entry_from_source(path: PathBuf, revision: i64, source: &str) -> DocumentEntry {
    let parsed = Arc::new(DocumentRevision::from_green(Arc::new(
        GreenDocument::parse(source),
    )));
    let current = parsed.green().valid_syntax().and_then(|valid| {
        analyze_green_document(valid, Arc::clone(parsed.green())).map(|output| {
            Arc::new(VersionedDocumentOutput {
                revision,
                output: Arc::new(output),
            })
        })
    });
    DocumentEntry {
        path,
        revision,
        parsed,
        current: current.clone(),
        last_valid: current,
    }
}
