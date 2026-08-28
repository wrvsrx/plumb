use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use plumb_semantics::analyze_document;
use plumb_syntax::parse;

use crate::{
    normalize, DocumentEntry, QueryCompleteness, QueryProvenance, QueryResult, SqliteSemanticStore,
    StoreError, VersionedDocumentOutput, Workspace, WorkspaceQueryError,
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
            completeness: QueryCompleteness::Complete,
            provenance,
        }
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
        let parsed = parse(source);
        let output = parsed.valid_syntax().map(analyze_document);
        store.replace(&path, revision, &parsed.source, output.as_ref())?;
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
        let path = normalize(path.as_ref());
        let parsed = Arc::new(parse(source));
        let previous_last_valid = self
            .documents
            .get(&path)
            .and_then(|entry| entry.last_valid.clone());
        let current = parsed.valid_syntax().map(|valid| {
            Arc::new(VersionedDocumentOutput {
                revision,
                output: Arc::new(analyze_document(valid)),
            })
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
        if entry.parsed.source != source {
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
            return Ok((entry.parsed.source == source).then(|| entry.clone()));
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
    let parsed = Arc::new(parse(source));
    let current = parsed.valid_syntax().map(|valid| {
        Arc::new(VersionedDocumentOutput {
            revision,
            output: Arc::new(analyze_document(valid)),
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
