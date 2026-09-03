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
    pub source: Option<Arc<str>>,
    pub cache_hit: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BatchIndexOptions {
    pub prune_missing: bool,
    pub retain_sources: bool,
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
    source: String,
}

enum PreparedDocument {
    Memory {
        path: PathBuf,
        revision: i64,
        parsed: Arc<ParsedDocument>,
        current: Option<Arc<VersionedDocumentOutput>>,
    },
    Persistent {
        path: PathBuf,
        revision: i64,
        source: String,
        output: Option<Box<plumb_semantics::DocumentOutput>>,
    },
}

impl PreparedDocument {
    fn path(&self) -> &Path {
        match self {
            Self::Memory { path, .. } | Self::Persistent { path, .. } => path,
        }
    }
}

impl Workspace {
    pub fn index_disk_files<F, C>(
        &mut self,
        paths: &[PathBuf],
        options: BatchIndexOptions,
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

        let stored_hashes = self
            .disk_store
            .as_ref()
            .map(SqliteSemanticStore::document_hashes)
            .transpose()?;
        let stored_paths_match = stored_hashes.as_ref().is_some_and(|hashes| {
            hashes.len() == paths.len() && paths.iter().all(|path| hashes.contains_key(path))
        });
        let file_sizes = paths
            .iter()
            .map(|path| path.metadata().map(|metadata| metadata.len()))
            .collect::<Result<Vec<_>, _>>();
        let balanced_files = file_sizes.is_ok_and(|sizes| {
            let total = sizes.iter().copied().sum::<u64>();
            let largest = sizes.iter().copied().max().unwrap_or(0);
            sizes.len() > 1 && largest <= 256 * 1024 && largest.saturating_mul(2) <= total
        });
        let read = |path: &PathBuf| {
            if cancelled() {
                return None;
            }
            Some(match std::fs::read_to_string(path) {
                Ok(source) => Ok(ReadDocument {
                    path: path.clone(),
                    revision: revision_for(path),
                    source,
                }),
                Err(error) => Err(BatchIndexFailure {
                    path: path.clone(),
                    message: error.to_string(),
                }),
            })
        };
        let reads = if !stored_paths_match && balanced_files {
            paths.par_iter().map(read).collect::<Vec<_>>()
        } else {
            paths.iter().map(read).collect::<Vec<_>>()
        };
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
        if self.disk_store.is_some() {
            let stored_hashes = stored_hashes.expect("persistent store hashes were loaded");
            stored_paths_changed = options.prune_missing
                && (stored_hashes.len() != paths.len()
                    || paths.iter().any(|path| !stored_hashes.contains_key(path)));
            for document in read_documents {
                let cache_hit = stored_hashes.get(&document.path)
                    == Some(&SqliteSemanticStore::content_hash(&document.source));
                indexed.push(BatchIndexedDocument {
                    path: document.path.clone(),
                    revision: document.revision,
                    source: options
                        .retain_sources
                        .then(|| Arc::from(document.source.as_str())),
                    cache_hit,
                });
                if !cache_hit {
                    misses.push(document);
                }
            }
        } else {
            indexed.extend(read_documents.iter().map(|document| {
                BatchIndexedDocument {
                    path: document.path.clone(),
                    revision: document.revision,
                    source: options
                        .retain_sources
                        .then(|| Arc::from(document.source.as_str())),
                    cache_hit: false,
                }
            }));
            misses = read_documents;
        }

        let total_miss_bytes = misses
            .iter()
            .map(|document| document.source.len())
            .sum::<usize>();
        let largest_miss_bytes = misses
            .iter()
            .map(|document| document.source.len())
            .max()
            .unwrap_or(0);
        let balanced_misses = misses.len() > 1
            && largest_miss_bytes <= 256 * 1024
            && largest_miss_bytes.saturating_mul(2) <= total_miss_bytes;
        let persistent = self.disk_store.is_some();
        let prepare = |document: ReadDocument| {
            if cancelled() {
                return None;
            }
            let parsed = parse(document.source);
            if persistent {
                let output = parsed.valid_syntax().map(analyze_document).map(Box::new);
                return Some(PreparedDocument::Persistent {
                    path: document.path,
                    revision: document.revision,
                    source: parsed.source,
                    output,
                });
            }
            let parsed = Arc::new(parsed);
            let current = parsed.valid_syntax().map(analyze_document).map(|output| {
                Arc::new(VersionedDocumentOutput {
                    revision: document.revision,
                    output: Arc::new(output),
                })
            });
            Some(PreparedDocument::Memory {
                path: document.path,
                revision: document.revision,
                parsed,
                current,
            })
        };
        let prepared = if balanced_misses {
            misses.into_par_iter().map(prepare).collect::<Vec<_>>()
        } else {
            misses.into_iter().map(prepare).collect::<Vec<_>>()
        };
        if cancelled() || prepared.iter().any(Option::is_none) {
            return Err(BatchIndexError::Cancelled);
        }
        let mut prepared = prepared.into_iter().flatten().collect::<Vec<_>>();
        prepared.sort_by(|left, right| left.path().cmp(right.path()));

        if let Some(store) = &self.disk_store {
            let generations = prepared
                .iter()
                .map(|document| {
                    let PreparedDocument::Persistent {
                        path,
                        revision,
                        source,
                        output,
                    } = document
                    else {
                        unreachable!("persistent workspace prepares persistent generations")
                    };
                    StoredGeneration {
                        path,
                        revision: *revision,
                        source,
                        output: output.as_deref(),
                    }
                })
                .collect::<Vec<_>>();
            if !generations.is_empty() || stored_paths_changed {
                store.reconcile_generations(&paths, &generations, stored_paths_changed)?;
            }
        } else {
            for document in prepared {
                let PreparedDocument::Memory {
                    path,
                    revision,
                    parsed,
                    current,
                } = document
                else {
                    unreachable!("memory workspace prepares memory snapshots")
                };
                let previous_last_valid = self
                    .documents
                    .get(&path)
                    .and_then(|entry| entry.last_valid.clone());
                let last_valid = current.clone().or(previous_last_valid);
                self.documents.insert(
                    path.clone(),
                    crate::DocumentEntry {
                        path,
                        revision,
                        parsed,
                        current,
                        last_valid,
                    },
                );
            }
            if options.prune_missing {
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
                BatchIndexOptions {
                    prune_missing: true,
                    retain_sources: false,
                },
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
            .index_disk_files(
                &[second.clone(), first.clone()],
                BatchIndexOptions {
                    prune_missing: true,
                    retain_sources: false,
                },
                |_| 1,
                || false,
            )
            .unwrap();
        assert_eq!(cold.cache_hits(), 0);
        assert_eq!(workspace.document_paths().unwrap().value.len(), 2);

        std::fs::write(&second, "Second updated\n").unwrap();
        let partial = workspace
            .index_disk_files(
                &[first.clone(), second.clone()],
                BatchIndexOptions {
                    prune_missing: true,
                    retain_sources: false,
                },
                |_| 2,
                || false,
            )
            .unwrap();
        assert_eq!(partial.cache_hits(), 1);
        assert_eq!(workspace.document_paths().unwrap().value.len(), 2);

        let warm = workspace
            .index_disk_files(
                std::slice::from_ref(&first),
                BatchIndexOptions {
                    prune_missing: true,
                    retain_sources: false,
                },
                |_| 3,
                || false,
            )
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
            .index_disk_files(
                std::slice::from_ref(&path),
                BatchIndexOptions {
                    prune_missing: true,
                    retain_sources: false,
                },
                |_| 1,
                || false,
            )
            .unwrap();
        std::fs::write(&path, "New\n").unwrap();

        let cancelled = AtomicBool::new(true);
        let result = workspace.index_disk_files(
            std::slice::from_ref(&path),
            BatchIndexOptions {
                prune_missing: true,
                retain_sources: false,
            },
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
