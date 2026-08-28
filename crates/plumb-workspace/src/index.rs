use std::path::{Path, PathBuf};
use std::sync::Arc;

use plumb_semantics::analyze_document;
use plumb_syntax::{parse, ParsedDocument};
use rayon::prelude::*;

use crate::store::StoredGeneration;
use crate::{normalize, SqliteSemanticStore, StoreError, VersionedDocumentOutput, Workspace};

#[derive(Debug, Clone)]
pub struct BatchIndexedDocument {
    pub path: PathBuf,
    pub revision: i64,
    pub source: Arc<str>,
    pub cache_hit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchIndexFailure {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct BatchIndexResult {
    pub documents: Vec<BatchIndexedDocument>,
    pub failures: Vec<BatchIndexFailure>,
}

impl BatchIndexResult {
    pub fn cache_hits(&self) -> usize {
        self.documents
            .iter()
            .filter(|document| document.cache_hit)
            .count()
    }

    pub fn is_complete(&self) -> bool {
        self.failures.is_empty()
    }
}

#[derive(Debug)]
pub enum BatchIndexError {
    Cancelled,
    Store(StoreError),
}

impl std::fmt::Display for BatchIndexError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("workspace indexing was cancelled"),
            Self::Store(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for BatchIndexError {}

impl From<StoreError> for BatchIndexError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

struct ReadDocument {
    path: PathBuf,
    revision: i64,
    source: Arc<str>,
}

struct PreparedDocument {
    path: PathBuf,
    revision: i64,
    source: Arc<str>,
    parsed: Arc<ParsedDocument>,
    current: Option<Arc<VersionedDocumentOutput>>,
}

impl Workspace {
    pub fn index_disk_files<F, C>(
        &mut self,
        paths: &[PathBuf],
        prune_missing: bool,
        revision_for: F,
        cancelled: C,
    ) -> Result<BatchIndexResult, BatchIndexError>
    where
        F: Fn(&Path) -> i64 + Sync,
        C: Fn() -> bool + Sync,
    {
        let mut paths = paths.iter().map(|path| normalize(path)).collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        if cancelled() {
            return Err(BatchIndexError::Cancelled);
        }

        let reads = paths
            .par_iter()
            .map(|path| {
                if cancelled() {
                    return None;
                }
                Some(match std::fs::read_to_string(path) {
                    Ok(source) => Ok(ReadDocument {
                        path: path.clone(),
                        revision: revision_for(path),
                        source: Arc::from(source),
                    }),
                    Err(error) => Err(BatchIndexFailure {
                        path: path.clone(),
                        message: error.to_string(),
                    }),
                })
            })
            .collect::<Vec<_>>();
        if cancelled() || reads.iter().any(Option::is_none) {
            return Err(BatchIndexError::Cancelled);
        }

        let mut failures = Vec::new();
        let mut read_documents = Vec::new();
        for read in reads.into_iter().flatten() {
            match read {
                Ok(document) => read_documents.push(document),
                Err(failure) => failures.push(failure),
            }
        }

        let mut indexed = Vec::with_capacity(read_documents.len());
        let mut misses = Vec::new();
        let mut stored_paths_changed = false;
        if let Some(store) = &self.disk_store {
            let stored_hashes = store.document_hashes()?;
            stored_paths_changed = prune_missing
                && (stored_hashes.len() != paths.len()
                    || paths.iter().any(|path| !stored_hashes.contains_key(path)));
            for document in read_documents {
                let cache_hit = stored_hashes.get(&document.path)
                    == Some(&SqliteSemanticStore::content_hash(&document.source));
                indexed.push(BatchIndexedDocument {
                    path: document.path.clone(),
                    revision: document.revision,
                    source: Arc::clone(&document.source),
                    cache_hit,
                });
                if !cache_hit {
                    misses.push(document);
                }
            }
        } else {
            indexed.extend(read_documents.iter().map(|document| BatchIndexedDocument {
                path: document.path.clone(),
                revision: document.revision,
                source: Arc::clone(&document.source),
                cache_hit: false,
            }));
            misses = read_documents;
        }

        let prepared = misses
            .into_par_iter()
            .map(|document| {
                if cancelled() {
                    return None;
                }
                let parsed = Arc::new(parse(document.source.to_string()));
                let current = parsed.valid_syntax().map(|syntax| {
                    Arc::new(VersionedDocumentOutput {
                        revision: document.revision,
                        output: Arc::new(analyze_document(syntax)),
                    })
                });
                Some(PreparedDocument {
                    path: document.path,
                    revision: document.revision,
                    source: document.source,
                    parsed,
                    current,
                })
            })
            .collect::<Vec<_>>();
        if cancelled() || prepared.iter().any(Option::is_none) {
            return Err(BatchIndexError::Cancelled);
        }
        let mut prepared = prepared.into_iter().flatten().collect::<Vec<_>>();
        prepared.sort_by(|left, right| left.path.cmp(&right.path));

        if let Some(store) = &self.disk_store {
            let generations = prepared
                .iter()
                .map(|document| StoredGeneration {
                    path: &document.path,
                    revision: document.revision,
                    source: &document.source,
                    output: document
                        .current
                        .as_ref()
                        .map(|current| current.output.as_ref()),
                })
                .collect::<Vec<_>>();
            if !generations.is_empty() || stored_paths_changed {
                store.reconcile_generations(&paths, &generations, stored_paths_changed)?;
            }
        } else {
            for document in prepared {
                let previous_last_valid = self
                    .documents
                    .get(&document.path)
                    .and_then(|entry| entry.last_valid.clone());
                let last_valid = document.current.clone().or(previous_last_valid);
                self.documents.insert(
                    document.path.clone(),
                    crate::DocumentEntry {
                        path: document.path,
                        revision: document.revision,
                        parsed: document.parsed,
                        current: document.current,
                        last_valid,
                    },
                );
            }
            if prune_missing {
                self.documents
                    .retain(|path, _| paths.binary_search(path).is_ok());
            }
        }

        indexed.sort_by(|left, right| left.path.cmp(&right.path));
        failures.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(BatchIndexResult {
            documents: indexed,
            failures,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use tempfile::tempdir;

    use super::*;
    use crate::SqliteSemanticStore;

    #[test]
    fn memory_batch_index_is_path_ordered_and_reports_read_failures() {
        let directory = tempdir().unwrap();
        let first = directory.path().join("a.plumb");
        let second = directory.path().join("b.plumb");
        let missing = directory.path().join("missing.plumb");
        std::fs::write(&first, "First\n").unwrap();
        std::fs::write(&second, "Second `span[open\n").unwrap();

        let mut workspace = Workspace::new();
        let result = workspace
            .index_disk_files(
                &[second.clone(), missing.clone(), first.clone()],
                true,
                |_| 7,
                || false,
            )
            .unwrap();

        assert_eq!(
            result
                .documents
                .iter()
                .map(|document| document.path.clone())
                .collect::<Vec<_>>(),
            [first.clone(), second.clone()]
        );
        assert_eq!(result.failures.len(), 1);
        assert_eq!(result.failures[0].path, missing);
        assert!(!result.is_complete());
        assert_eq!(workspace.documents().count(), 2);
        assert!(workspace.get(&first).unwrap().current.is_some());
        assert!(workspace.get(&second).unwrap().current.is_none());
        assert!(result
            .documents
            .iter()
            .all(|document| { document.revision == 7 && !document.cache_hit }));
    }

    #[test]
    fn persistent_batch_reuses_hits_and_prunes_complete_scans() {
        let directory = tempdir().unwrap();
        let first = directory.path().join("a.plumb");
        let second = directory.path().join("b.plumb");
        std::fs::write(&first, "First\n").unwrap();
        std::fs::write(&second, "Second\n").unwrap();
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let mut workspace = Workspace::with_sqlite_store(store);

        let cold = workspace
            .index_disk_files(&[second.clone(), first.clone()], true, |_| 1, || false)
            .unwrap();
        assert_eq!(cold.cache_hits(), 0);
        assert_eq!(workspace.document_paths().unwrap().value.len(), 2);

        std::fs::write(&second, "Second updated\n").unwrap();
        let partial = workspace
            .index_disk_files(&[first.clone(), second.clone()], true, |_| 2, || false)
            .unwrap();
        assert_eq!(partial.cache_hits(), 1);
        assert_eq!(workspace.document_paths().unwrap().value.len(), 2);

        let warm = workspace
            .index_disk_files(std::slice::from_ref(&first), true, |_| 3, || false)
            .unwrap();
        assert_eq!(warm.cache_hits(), 1);
        assert_eq!(workspace.document_paths().unwrap().value, [first]);
    }

    #[test]
    fn cancellation_does_not_publish_a_partial_batch() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("a.plumb");
        std::fs::write(&path, "Old\n").unwrap();
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let mut workspace = Workspace::with_sqlite_store(store);
        workspace
            .index_disk_files(std::slice::from_ref(&path), true, |_| 1, || false)
            .unwrap();
        std::fs::write(&path, "New\n").unwrap();

        let cancelled = AtomicBool::new(true);
        let result = workspace.index_disk_files(
            std::slice::from_ref(&path),
            true,
            |_| 2,
            || cancelled.load(Ordering::Relaxed),
        );

        assert!(matches!(result, Err(BatchIndexError::Cancelled)));
        assert!(workspace
            .disk_store
            .as_ref()
            .unwrap()
            .contains_current(&path, "Old\n")
            .unwrap());
    }
}
