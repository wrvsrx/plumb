//! Bounded CPU work. Futures wait for capacity without occupying the dispatcher.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use async_lsp::{ErrorCode, ResponseError};
use futures::future::BoxFuture;
use tokio::sync::Semaphore;

#[derive(Clone, Copy, Hash, Eq, PartialEq)]
pub(super) enum Kind {
    Format,
    RangeFormat,
    CodeLens,
    Fold,
}

pub(super) struct Workers {
    formatting: Arc<Semaphore>,
    decorations: Arc<Semaphore>,
    workspace: Arc<AtomicU64>,
    sources: HashMap<PathBuf, Arc<AtomicU64>>,
    latest: HashMap<(PathBuf, Kind), Weak<AtomicBool>>,
}

impl Default for Workers {
    fn default() -> Self {
        Self {
            formatting: Arc::new(Semaphore::new(1)),
            decorations: Arc::new(Semaphore::new(1)),
            workspace: Arc::new(AtomicU64::new(0)),
            sources: HashMap::new(),
            latest: HashMap::new(),
        }
    }
}

impl Workers {
    pub fn generation(&self) -> u64 {
        self.workspace.load(Ordering::Acquire)
    }

    pub fn workspace_changed(&self) {
        self.workspace.fetch_add(1, Ordering::AcqRel);
    }

    pub fn source_changed(&mut self, path: &Path) {
        self.workspace_changed();
        if let Some(epoch) = self.sources.remove(path) {
            epoch.fetch_add(1, Ordering::AcqRel);
        }
        self.latest.retain(|(candidate, _), _| candidate != path);
    }

    pub fn request(&mut self, path: &Path, kind: Kind) -> Request {
        self.latest.retain(|_, token| token.strong_count() > 0);
        let canceled = Arc::new(AtomicBool::new(false));
        if let Some(old) = self
            .latest
            .insert((path.to_owned(), kind), Arc::downgrade(&canceled))
            .and_then(|old| old.upgrade())
        {
            old.store(true, Ordering::Release);
        }
        let source = self.sources.entry(path.to_owned()).or_default().clone();
        let decorative = matches!(kind, Kind::CodeLens | Kind::Fold);
        let mut epochs = vec![(source.clone(), source.load(Ordering::Acquire))];
        if decorative {
            epochs.push((
                self.workspace.clone(),
                self.workspace.load(Ordering::Acquire),
            ));
        }
        Request {
            capacity: if decorative {
                self.decorations.clone()
            } else {
                self.formatting.clone()
            },
            canceled,
            epochs,
        }
    }
}

pub(super) struct Request {
    capacity: Arc<Semaphore>,
    canceled: Arc<AtomicBool>,
    epochs: Vec<(Arc<AtomicU64>, u64)>,
}

impl Drop for Request {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}

fn check(canceled: &AtomicBool, epochs: &[(Arc<AtomicU64>, u64)]) -> Result<(), ResponseError> {
    if canceled.load(Ordering::Acquire)
        || epochs
            .iter()
            .any(|(epoch, expected)| epoch.load(Ordering::Acquire) != *expected)
    {
        Err(ResponseError::new(
            ErrorCode::CONTENT_MODIFIED,
            "request inputs changed or request superseded",
        ))
    } else {
        Ok(())
    }
}

impl Request {
    pub fn source_only(mut self) -> Self {
        self.epochs.truncate(1);
        self
    }

    pub fn at_workspace_generation(mut self, generation: u64) -> Self {
        self.epochs[1].1 = generation;
        self
    }

    pub fn run<T: Send + 'static>(
        self,
        work: impl FnOnce() -> Result<T, ResponseError> + Send + 'static,
    ) -> BoxFuture<'static, Result<T, ResponseError>> {
        Box::pin(async move {
            check(&self.canceled, &self.epochs)?;
            let permit = self.capacity.clone().acquire_owned().await.map_err(|_| {
                ResponseError::new(ErrorCode::INTERNAL_ERROR, "request worker closed")
            })?;
            check(&self.canceled, &self.epochs)?;
            let canceled = self.canceled.clone();
            let epochs = self.epochs.clone();
            let result = tokio::task::spawn_blocking(move || {
                // Keep capacity until CPU work really ends, even if the future is canceled.
                let _permit = permit;
                check(&canceled, &epochs)?;
                let result = work();
                check(&canceled, &epochs)?;
                result
            })
            .await
            .map_err(|_| ResponseError::new(ErrorCode::INTERNAL_ERROR, "request worker failed"))?;
            check(&self.canceled, &self.epochs)?;
            result
        })
    }
}

/// Reject mixed SQLite generations, including writes from another process sharing the cache.
pub(super) fn query<T>(
    workspace: &plumb_workspace::Workspace,
    work: impl FnOnce() -> Result<T, ResponseError>,
) -> Result<T, ResponseError> {
    let version = workspace
        .query_store_version()
        .map_err(super::workspace_query_response_error)?;
    let result = work();
    if version
        != workspace
            .query_store_version()
            .map_err(super::workspace_query_response_error)?
    {
        return Err(ResponseError::new(
            ErrorCode::CONTENT_MODIFIED,
            "workspace store changed during request",
        ));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_lsp::LanguageServer;
    use serde_json::json;

    fn server() -> super::super::ServerState {
        let (_, client) = async_lsp::MainLoop::new_server(|_| async_lsp::router::Router::new(()));
        let mut server = super::super::ServerState::new(client);
        server
            .workspace
            .open_document("/tmp/worker.plumb", 1, "`# Header\n Child\n");
        server
    }

    #[tokio::test]
    async fn formatting_finishes_while_decoration_capacity_is_occupied() {
        let mut server = server();
        let permit = server
            .workers
            .decorations
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        let folds = server.folding_range(
            serde_json::from_value(json!({"textDocument":{"uri":"file:///tmp/worker.plumb"}}))
                .unwrap(),
        );
        let formatting = server.formatting(serde_json::from_value(json!({"textDocument":{"uri":"file:///tmp/worker.plumb"},"options":{"tabSize":1,"insertSpaces":true}})).unwrap());
        let folds = tokio::spawn(folds);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(5), formatting)
                .await
                .unwrap()
                .is_ok()
        );
        assert!(!folds.is_finished());
        drop(permit);
        assert!(folds.await.unwrap().unwrap().is_some());
    }

    #[tokio::test]
    async fn superseded_and_source_stale_queued_jobs_never_execute() {
        for supersede in [false, true] {
            let mut workers = Workers::default();
            let path = Path::new("doc.plumb");
            let permit = workers.decorations.clone().acquire_owned().await.unwrap();
            let old = workers
                .request(path, Kind::Fold)
                .run(|| -> Result<(), ResponseError> { panic!("stale job executed") });
            let job = tokio::spawn(old);
            tokio::task::yield_now().await;
            let newest = if supersede {
                Some(workers.request(path, Kind::Fold))
            } else {
                workers.source_changed(path);
                None
            };
            drop(permit);
            assert_eq!(
                job.await.unwrap().unwrap_err().code,
                ErrorCode::CONTENT_MODIFIED
            );
            if let Some(newest) = newest {
                newest.run(|| Ok(())).await.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn running_cancellation_retains_capacity_until_cpu_work_exits() {
        let mut workers = Workers::default();
        let path = Path::new("doc.plumb");
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let first = tokio::spawn(workers.request(path, Kind::Fold).run(move || {
            started.send(()).unwrap();
            wait.recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            Ok(())
        }));
        ready.await.unwrap();
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        assert_eq!(workers.decorations.available_permits(), 0);
        let next = workers.request(path, Kind::Fold).run(|| Ok(()));
        release.send(()).unwrap();
        next.await.unwrap();
        assert_eq!(workers.decorations.available_permits(), 1);
    }

    #[tokio::test]
    async fn workspace_changes_reject_running_decorations_but_not_formatting() {
        for kind in [Kind::Fold, Kind::CodeLens, Kind::Format, Kind::RangeFormat] {
            let mut workers = Workers::default();
            let (started, ready) = tokio::sync::oneshot::channel();
            let (release, wait) = std::sync::mpsc::channel();
            let job = tokio::spawn(workers.request(Path::new("doc.plumb"), kind).run(move || {
                started.send(()).unwrap();
                wait.recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                Ok(42)
            }));
            ready.await.unwrap();
            workers.workspace_changed();
            release.send(()).unwrap();
            let result = job.await.unwrap();
            if matches!(kind, Kind::Fold | Kind::CodeLens) {
                assert_eq!(result.unwrap_err().code, ErrorCode::CONTENT_MODIFIED);
            } else {
                assert_eq!(result.unwrap(), 42);
            }
        }
    }

    #[tokio::test]
    async fn change_close_and_rename_reject_already_requested_formatting() {
        for action in ["change", "close", "rename"] {
            let mut server = server();
            let uri = lsp_types::Url::parse("file:///tmp/worker.plumb").unwrap();
            server
                .open_documents
                .insert(uri.clone(), PathBuf::from("/tmp/worker.plumb"));
            let formatting = server.formatting(
                serde_json::from_value(
                    json!({"textDocument":{"uri":uri},"options":{"tabSize":1,"insertSpaces":true}}),
                )
                .unwrap(),
            );
            match action {
                "change" => server.update(uri, 1, "Replacement\n".into(), None, false),
                "close" => {
                    let _ = server.did_close(
                        serde_json::from_value(json!({"textDocument":{"uri":uri}})).unwrap(),
                    );
                }
                _ => server
                    .begin_path_rename("/tmp/worker.plumb".into(), "/tmp/worker-new.plumb".into()),
            }
            assert_eq!(
                formatting.await.unwrap_err().code,
                ErrorCode::CONTENT_MODIFIED
            );
        }
    }
    #[tokio::test]
    async fn canceled_queued_work_releases_its_snapshot_and_never_starts() {
        let mut workers = Workers::default();
        let permit = workers.decorations.clone().acquire_owned().await.unwrap();
        let snapshot = Arc::new(());
        let weak = Arc::downgrade(&snapshot);
        let job = tokio::spawn(workers.request(Path::new("doc.plumb"), Kind::Fold).run(
            move || -> Result<(), ResponseError> {
                drop(snapshot);
                panic!("canceled queued work ran")
            },
        ));
        tokio::task::yield_now().await;
        job.abort();
        assert!(job.await.unwrap_err().is_cancelled());
        assert!(weak.upgrade().is_none());
        drop(permit);
    }

    #[tokio::test]
    async fn failed_worker_returns_internal_error_and_releases_capacity() {
        let mut workers = Workers::default();
        let error = workers
            .request(Path::new("doc.plumb"), Kind::Format)
            .run(|| -> Result<(), ResponseError> { panic!("injected worker failure") })
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::INTERNAL_ERROR);
        assert_eq!(workers.formatting.available_permits(), 1);
    }

    #[test]
    fn composed_queries_reject_interleaved_store_writes() {
        let store = plumb_workspace::SqliteSemanticStore::open_in_memory().unwrap();
        let workspace = plumb_workspace::Workspace::with_sqlite_store(store);
        let mut writer = workspace.clone();
        let error = query(&workspace, || {
            writer.insert_disk("other.plumb", 1, "Changed\n").unwrap();
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::CONTENT_MODIFIED);
    }
}
