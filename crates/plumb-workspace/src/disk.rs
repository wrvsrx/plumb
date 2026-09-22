//! Short-lived disk commands: one read/hash/index round and an isolated query snapshot.
use crate::*;
use chrono::{DateTime, FixedOffset, Local};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug)]
pub enum DiskLoadError {
    Source(String),
    Store(StoreError),
}
impl From<StoreError> for DiskLoadError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}
impl std::fmt::Display for DiskLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Source(s) => f.write_str(s),
            Self::Store(e) => write!(f, "{e}; retry with --no-cache"),
        }
    }
}
impl std::error::Error for DiskLoadError {}

pub struct DiskWorkspace {
    pub root: PathBuf,
    pub workspace: Workspace,
    pub sources: BTreeMap<PathBuf, Arc<str>>,
    pub now: DateTime<FixedOffset>,
    pub indexed: usize,
    pub cache_hits: usize,
    pub timings: BatchIndexTimings,
    pub scan_time: std::time::Duration,
    pub cache_warning: Option<String>,
    cache_path: Option<PathBuf>,
}
impl DiskWorkspace {
    pub fn from_memory(root: PathBuf, workspace: Workspace) -> Self {
        let sources = workspace
            .documents()
            .map(|e| (e.path.clone(), Arc::from(e.parsed.source())))
            .collect();
        Self {
            root,
            workspace,
            sources,
            now: Local::now().fixed_offset(),
            indexed: 0,
            cache_hits: 0,
            timings: Default::default(),
            scan_time: Default::default(),
            cache_warning: None,
            cache_path: None,
        }
    }
    pub fn load(root: &Path, cache_path: Option<&Path>) -> Result<Self, DiskLoadError> {
        let now = Local::now().fixed_offset();
        let root = normalize(root);
        let scan_started = std::time::Instant::now();
        let paths = scan_workspace_files(&root)
            .into_result()
            .map_err(DiskLoadError::Source)?;
        let scan_time = scan_started.elapsed();
        let prepare = |workspace: &mut Workspace| -> Result<BatchIndexResult, DiskLoadError> {
            let batch = workspace
                .index_disk_files(
                    &paths,
                    BatchIndexOptions {
                        prune_missing: true,
                        retain_sources: true,
                    },
                    |_| 0,
                    || false,
                )
                .map_err(|e| match e {
                    BatchIndexError::Store(e) => DiskLoadError::Store(e),
                    BatchIndexError::Cancelled => DiskLoadError::Source(e.to_string()),
                })?;
            if !batch.is_complete() {
                return Err(DiskLoadError::Source(
                    batch
                        .failures
                        .iter()
                        .map(|f| format!("cannot read {}: {}", f.path.display(), f.message))
                        .collect::<Vec<_>>()
                        .join("\n"),
                ));
            }
            Ok(batch)
        };
        let mut warning = None;
        let persistent = cache_path.map(|path| -> Result<_, DiskLoadError> {
            let store = SqliteSemanticStore::open(path)?;
            store.isolated_update(|| {
                let mut w = Workspace::with_sqlite_store(store.clone());
                let batch = prepare(&mut w)?;
                Ok((
                    Workspace::with_sqlite_store(store.readonly_snapshot()?),
                    batch,
                ))
            })
        });
        let (workspace, batch) = match persistent {
            Some(Ok(result)) => result,
            Some(Err(DiskLoadError::Source(e))) => return Err(DiskLoadError::Source(e)),
            failed => {
                if let Some(Err(e)) = failed {
                    warning = Some(format!(
                        "persistent cache unavailable; using uncached workspace: {e}"
                    ));
                }
                let mut w = Workspace::new();
                let batch = prepare(&mut w)?;
                (w, batch)
            }
        };
        let cache_hits = batch.cache_hits();
        let timings = batch.timings;
        let indexed = batch.documents.len();
        let sources = batch
            .documents
            .into_iter()
            .map(|d| (d.path, d.source.expect("retained source")))
            .collect();
        Ok(Self {
            root,
            workspace,
            sources,
            now,
            indexed,
            cache_hits,
            timings,
            scan_time,
            cache_warning: warning.clone(),
            cache_path: if warning.is_some() {
                None
            } else {
                cache_path.map(Path::to_path_buf)
            },
        })
    }
    pub fn events(
        &self,
    ) -> Result<Vec<(PathBuf, plumb_semantics::EventRecord)>, WorkspaceQueryError> {
        let mut result = Vec::new();
        for path in self.sources.keys() {
            if let Some(entry) = self.workspace.get(path) {
                if let Some(current) = &entry.current {
                    result.extend(
                        current
                            .output
                            .events()
                            .events
                            .iter()
                            .map(|e| (path.clone(), e)),
                    );
                }
            } else if let Some(store) = &self.workspace.disk_store {
                result.extend(
                    store
                        .events_for_path(path)?
                        .into_iter()
                        .map(|e| (path.clone(), e)),
                );
            }
        }
        Ok(result)
    }
    pub fn source(&self, path: &Path) -> Option<&str> {
        self.sources.get(&normalize(path)).map(|s| s.as_ref())
    }

    /// Only targets that still match this command's disk read can become editing overlays.
    pub fn materialize_target(&mut self, path: &Path) -> Result<(), String> {
        let path = normalize(path);
        let source = self
            .source(&path)
            .ok_or_else(|| format!("task document is not indexed: {}", path.display()))?;
        let actual = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if actual != source {
            return Err(format!("source changed since indexing: {}", path.display()));
        }
        if self.workspace.get(&path).is_none() {
            let entry = self
                .workspace
                .document_from_source(&path, source)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("stale cached document: {}", path.display()))?;
            self.workspace.overlay_document_entry(entry);
        }
        Ok(())
    }
    pub fn apply_target_edit(&mut self, path: &Path, edit: WorkspaceEdit) -> Result<(), String> {
        let path = normalize(path);
        let entry = self
            .workspace
            .get(&path)
            .ok_or_else(|| "target is not materialized".to_string())?;
        let source = entry.parsed.source();
        let updated = apply_document_edit(source.to_owned(), &path, entry.revision, edit)
            .map_err(|e| format!("cannot apply task edit: {e:?}"))?;
        let actual = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if actual != source {
            return Err(format!("source changed before write: {}", path.display()));
        }
        if actual != updated {
            std::fs::write(&path, &updated)
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        }
        // One new revision after writing; the old parsed revision was reused to propose the edit.
        self.workspace
            .open_document(&path, entry.revision + 1, updated.clone());
        self.sources
            .insert(path.clone(), Arc::from(updated.as_str()));
        if let Some(cache_path) = &self.cache_path {
            let store = SqliteSemanticStore::open(cache_path)
                .map_err(|e| format!("source saved but cache refresh failed: {e}"))?;
            let entry = self.workspace.get(&path).expect("installed target");
            let output = entry.current.as_ref().map(|c| c.output.as_ref());
            let diagnostics = crate::diagnostics::CachedDiagnosticInputs::new(
                &entry.parsed.diagnostics(),
                output,
            );
            store
                .replace_with_diagnostics(&path, entry.revision, &updated, output, &diagnostics)
                .map_err(|e| format!("source saved but cache refresh failed: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn diagnostics(loaded: &DiskWorkspace) -> BTreeMap<PathBuf, Vec<WorkspaceDiagnostic>> {
        let context = loaded.workspace.diagnostic_context().unwrap();
        loaded
            .sources
            .keys()
            .map(|path| {
                (
                    path.clone(),
                    loaded
                        .workspace
                        .check_diagnostics_with_context(path, &context)
                        .unwrap()
                        .value,
                )
            })
            .collect()
    }
    #[test]
    fn cold_warm_and_memory_diagnostics_match_across_source_and_target_changes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let db = temp.path().join("cache/index.sqlite3");
        let a = root.join("a.plumb");
        let b = root.join("b.plumb");
        let bad = root.join("bad.plumb");
        std::fs::write(&a,"`= title 中文\n`= title 重复\n\nSee `->{b.plumb#b} and `->{asset.png `+{embed}}.\n\n`- Closed\n `+ task\n `@ a\n `= depends b.plumb#b\n `= prev missing.plumb#x\n `= done 2026-09-22T00:00:00Z\n\n`- 2026-09-22T00:00:00Z Event\n `+ event\n `= tasks b.plumb#b\n").unwrap();
        std::fs::write(&b, "`- Open\n `+ task\n `@ b\n `= depends a.plumb#a\n").unwrap();
        std::fs::write(&bad, "broken {\n").unwrap();
        std::fs::write(root.join("asset.png"), b"asset").unwrap();
        for step in 0..8 {
            match step {
                1 => {
                    std::fs::write(&b, "`# Not a task\n `@ b\n\n`# Duplicate\n `@ b\n").unwrap();
                }
                2 => {
                    std::fs::remove_file(root.join("asset.png")).unwrap();
                    std::fs::write(&bad, "Fixed\n").unwrap();
                }
                3 => {
                    std::fs::write(&b, "broken {\n").unwrap();
                }
                4 => {
                    std::fs::rename(&b, root.join("renamed.plumb")).unwrap();
                }
                5 => {
                    std::fs::write(&b, "`+ task\n`= title Document\n").unwrap();
                    std::fs::write(&a,"`- Depends\n `+ task\n `= depends b.plumb\n\n`- 2026-09-22T00:00:00Z Event\n `+ event\n `= tasks b.plumb\n").unwrap();
                }
                6 => {
                    std::fs::write(root.join(".ignore"), "b.plumb\n").unwrap();
                }
                7 => {
                    std::fs::remove_file(root.join(".ignore")).unwrap();
                    std::fs::write(&b, "`+ task\n`= done 2026-09-22T00:00:00Z\n").unwrap();
                }
                _ => {}
            }
            let cold = DiskWorkspace::load(&root, Some(&db)).unwrap();
            assert!(cold.cache_warning.is_none(), "{:?}", cold.cache_warning);
            let warm = DiskWorkspace::load(&root, Some(&db)).unwrap();
            let memory = DiskWorkspace::load(&root, None).unwrap();
            assert_eq!(warm.cache_hits, warm.indexed);
            assert_eq!(
                warm.workspace.documents().count(),
                0,
                "warm check must not materialize syntax"
            );
            assert_eq!(diagnostics(&cold), diagnostics(&memory), "step {step}");
            assert_eq!(diagnostics(&warm), diagnostics(&memory), "step {step}");
        }
    }
    #[test]
    fn snapshot_is_detached_and_target_edits_reject_disk_changes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let db = temp.path().join("cache/index.sqlite3");
        let path = root.join("task.plumb");
        let source = "`- Task\n `+ task\n `@ t\n";
        std::fs::write(&path, source).unwrap();
        let mut first = DiskWorkspace::load(&root, Some(&db)).unwrap();
        first.materialize_target(&path).unwrap();
        let edit = first
            .workspace
            .focus_task_by_id(&path, "t", "2026-09-22T00:00:00Z")
            .unwrap();
        std::fs::write(&path, "Externally changed\n").unwrap();
        let second = DiskWorkspace::load(&root, Some(&db)).unwrap();
        assert!(second
            .workspace
            .search_records_filtered(
                &root,
                Some(SearchRecordKind::Task),
                "",
                10,
                second.now,
                None
            )
            .unwrap()
            .value
            .items
            .is_empty());
        assert!(first
            .apply_target_edit(&path, edit)
            .unwrap_err()
            .contains("source changed"));
        assert_eq!(first.source(&path), Some(source));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "Externally changed\n"
        );
        let mut stale = DiskWorkspace::load(&root, Some(&db)).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(stale.materialize_target(&path).is_err());
    }
    #[test]
    fn corrupt_cache_falls_back_visibly_and_missing_root_fails() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("a.plumb"), "broken {\n").unwrap();
        let db = temp.path().join("broken.sqlite3");
        std::fs::write(&db, b"not SQLite").unwrap();
        let loaded = DiskWorkspace::load(&root, Some(&db)).unwrap();
        assert!(loaded.cache_warning.is_some());
        assert_eq!(
            diagnostics(&loaded),
            diagnostics(&DiskWorkspace::load(&root, None).unwrap())
        );
        assert!(DiskWorkspace::load(&root.join("missing"), Some(&db)).is_err());
    }
    #[test]
    fn committed_mutation_updates_generation_and_followup_query() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let db = temp.path().join("cache/index.sqlite3");
        let path = root.join("task.plumb");
        std::fs::write(&path, "`- Task\n `+ task\n `@ t\n").unwrap();
        let mut loaded = DiskWorkspace::load(&root, Some(&db)).unwrap();
        loaded.materialize_target(&path).unwrap();
        let edit = loaded
            .workspace
            .focus_task_by_id(&path, "t", "2026-09-22T00:00:00Z")
            .unwrap();
        loaded.apply_target_edit(&path, edit).unwrap();
        let next = DiskWorkspace::load(&root, Some(&db)).unwrap();
        assert_eq!(next.cache_hits, 1);
        assert!(next.source(&path).unwrap().contains("focused"));
        let items = next
            .workspace
            .search_records_filtered(
                &root,
                Some(SearchRecordKind::Task),
                "",
                10,
                next.now,
                Some("focused"),
            )
            .unwrap()
            .value
            .items;
        assert_eq!(items.len(), 1);
    }
}

#[cfg(test)]
mod consistency_tests {
    use super::*;
    use chrono::DateTime;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    #[test]
    fn immutable_query_snapshot_survives_another_writer_and_supports_sql_functions() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("a.plumb");
        let db = temp.path().join("cache/a.sqlite3");
        std::fs::write(&path, "`- Original task\n `+ task\n `@ a\n").unwrap();
        let first = DiskWorkspace::load(temp.path(), Some(&db)).unwrap();
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    std::fs::write(&path, "`- Replacement\n `+ task\n `@ b\n").unwrap();
                    DiskWorkspace::load(temp.path(), Some(&db)).unwrap();
                })
                .join()
                .unwrap();
        });
        let records = first
            .workspace
            .search_records_filtered(
                temp.path(),
                Some(SearchRecordKind::Task),
                "Original",
                10,
                first.now,
                None,
            )
            .unwrap()
            .value
            .items;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id.as_deref(), Some("a"));
        assert_eq!(first.workspace.documents().count(), 0);
        let next = DiskWorkspace::load(temp.path(), Some(&db)).unwrap();
        assert_eq!(next.cache_hits, 1);
        let records = next
            .workspace
            .search_records_filtered(
                temp.path(),
                Some(SearchRecordKind::Task),
                "",
                10,
                next.now,
                None,
            )
            .unwrap()
            .value
            .items;
        assert_eq!(records[0].id.as_deref(), Some("b"));
    }
    #[test]
    fn schema_and_producer_changes_rebuild_diagnostics_and_read_failure_is_not_success() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = root.join("a.plumb");
        let db = temp.path().join("cache/index.sqlite3");
        std::fs::write(&path, "broken {\n").unwrap();
        DiskWorkspace::load(&root, Some(&db)).unwrap();
        for key in ["schema_version", "producer_version"] {
            let store = SqliteSemanticStore::open(&db).unwrap();
            store
                .execute_batch_for_test(&format!(
                    "UPDATE cache_meta SET value=-1 WHERE key='{key}'"
                ))
                .unwrap();
            drop(store);
            let loaded = DiskWorkspace::load(&root, Some(&db)).unwrap();
            assert_eq!(loaded.cache_hits, 0);
            assert!(loaded.cache_warning.is_none());
            let context = loaded.workspace.diagnostic_context().unwrap();
            assert!(!loaded
                .workspace
                .check_diagnostics_with_context(&path, &context)
                .unwrap()
                .value
                .is_empty());
        }
        std::fs::write(&path, [0xff]).unwrap();
        assert!(matches!(
            DiskWorkspace::load(&root, Some(&db)),
            Err(DiskLoadError::Source(_))
        ));
        assert!(matches!(
            DiskWorkspace::load(&root, None),
            Err(DiskLoadError::Source(_))
        ));
    }
    #[test]
    fn interrupted_snapshot_update_rolls_back_every_generation() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("cache/index.sqlite3");
        let store = SqliteSemanticStore::open(&db).unwrap();
        let path = temp.path().join("a.plumb");
        let mut writer = Workspace::with_sqlite_store(store.clone());
        writer.insert_disk(&path, 0, "Before\n").unwrap();
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _: Result<(), StoreError> = store.isolated_update(|| {
                writer.insert_disk(&path, 1, "After\n")?;
                writer.insert_disk(temp.path().join("b.plumb"), 0, "Added\n")?;
                panic!("simulated interruption before snapshot commit");
            });
        }));
        assert!(result.is_err());
        assert!(store.contains_current(&path, "Before\n").unwrap());
        assert_eq!(store.documents().unwrap().len(), 1);
    }
    #[test]
    fn time_dependent_queries_and_next_recompute_from_cached_facts() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("cache/index.sqlite3");
        std::fs::write(
            temp.path().join("a.plumb"),
            "`- Wait\n `+ task\n `@ wait\n `= wait 2026-09-22T12:00:00Z\n",
        )
        .unwrap();
        DiskWorkspace::load(temp.path(), Some(&db)).unwrap();
        let cached = DiskWorkspace::load(temp.path(), Some(&db)).unwrap();
        let memory = DiskWorkspace::load(temp.path(), None).unwrap();
        for (stamp, expected) in [("2026-09-22T11:00:00Z", 0), ("2026-09-22T13:00:00Z", 1)] {
            let now = DateTime::parse_from_rfc3339(stamp).unwrap();
            let run = |loaded: &DiskWorkspace| {
                loaded
                    .workspace
                    .search_records_filtered(
                        temp.path(),
                        Some(SearchRecordKind::Task),
                        "",
                        20,
                        now,
                        Some("state == 'ready'"),
                    )
                    .unwrap()
                    .value
                    .items
            };
            assert_eq!(run(&cached), run(&memory));
            assert_eq!(run(&cached).len(), expected);
            let query = NextQuery {
                root: temp.path().to_path_buf(),
                limit: 3,
                cursor: None,
                workspace_revision: 0,
                now,
            };
            let cold = memory.workspace.query_next(&query).unwrap().value;
            let warm = cached.workspace.query_next(&query).unwrap().value;
            assert_eq!(warm.candidates.len(), cold.candidates.len());
            assert_eq!(warm.candidates.len(), expected);
        }
    }
}
