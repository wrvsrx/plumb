use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_lsp::{ErrorCode, ResponseError};
use futures::future::BoxFuture;
use lsp_types::{SemanticToken, SemanticTokens, SemanticTokensResult};
use plumb_workspace::{DocumentEntry, DocumentRevision};
use tokio::sync::watch;

use super::ServerState;
use crate::folding::{collapsed_text_labels, FoldLabel};
use crate::position::byte_range_to_lsp;
use crate::semantic_tokens::{closed_task_token_ranges, physical_line_ranges};

pub(super) struct SemanticSnapshot {
    pub entry: DocumentEntry,
    pub labels: Option<HashMap<(usize, usize), FoldLabel>>,
}

pub(super) struct PendingDocumentReads {
    revision: i64,
    parsed: Arc<DocumentRevision>,
    fold_labels: bool,
    result: watch::Sender<Option<Result<Arc<SemanticSnapshot>, ResponseError>>>,
}

impl ServerState {
    pub(super) fn await_document_semantics(
        &mut self,
        path: &Path,
        fold_labels: bool,
    ) -> Option<BoxFuture<'static, Result<Arc<SemanticSnapshot>, ResponseError>>> {
        let entry = self.workspace.get(path)?;
        if !entry.parsed.is_valid() || entry.current.is_some() {
            return None;
        }
        let pending = self
            .pending_document_reads
            .entry(entry.path.clone())
            .or_insert_with(|| {
                let (result, _) = watch::channel(None);
                PendingDocumentReads {
                    revision: entry.revision,
                    parsed: Arc::clone(&entry.parsed),
                    fold_labels: false,
                    result,
                }
            });
        pending.fold_labels |= fold_labels;
        let mut receiver = pending.result.subscribe();
        Some(Box::pin(async move {
            // Dropping a superseded generation's sender terminates every old request.
            let result = receiver.wait_for(Option::is_some).await.map_err(|_| {
                ResponseError::new(ErrorCode::CONTENT_MODIFIED, "document revision changed")
            })?;
            result.as_ref().expect("semantic result is ready").clone()
        }))
    }

    pub(super) fn finish_pending_document_reads(&mut self, path: &Path) {
        let Some(pending) = self.pending_document_reads.remove(path) else {
            return;
        };
        if pending.result.receiver_count() == 0 {
            return;
        }
        let Some(entry) = self.workspace.get(path).filter(|entry| {
            entry.revision == pending.revision
                && Arc::ptr_eq(&entry.parsed, &pending.parsed)
                && entry.current.is_some()
        }) else {
            return;
        };
        let labels = pending.fold_labels.then(|| {
            collapsed_text_labels(&self.workspace, &entry.path, entry, self.index_complete)
        });
        let _ = pending.result.send(Some(Ok(Arc::new(SemanticSnapshot {
            entry: entry.clone(),
            labels,
        }))));
    }

    pub(super) fn fail_pending_document_reads(&mut self, path: &Path) {
        // Keep the failure for later requests to this revision; no worker can wake them.
        drop(self.await_document_semantics(path, false));
        if let Some(pending) = self.pending_document_reads.get(path) {
            pending.result.send_replace(Some(Err(ResponseError::new(
                ErrorCode::INTERNAL_ERROR,
                "document semantic analysis failed",
            ))));
        }
    }
}

pub(super) fn semantic_tokens(entry: &DocumentEntry) -> Option<SemanticTokensResult> {
    let current = entry.current.as_ref()?;
    let mut previous_line = 0;
    let mut previous_start = 0;
    let data = closed_task_token_ranges(&current.output.tasks().tasks)
        .into_iter()
        .flat_map(|(byte_range, modifiers)| {
            physical_line_ranges(entry.parsed.source(), &byte_range)
                .into_iter()
                .map(move |range| (range, modifiers))
        })
        .map(|(byte_range, token_modifiers_bitset)| {
            let range = byte_range_to_lsp(entry.parsed.source(), &byte_range);
            let delta_line = range.start.line - previous_line;
            let delta_start = if delta_line == 0 {
                range.start.character - previous_start
            } else {
                range.start.character
            };
            previous_line = range.start.line;
            previous_start = range.start.character;
            SemanticToken {
                delta_line,
                delta_start,
                length: range.end.character - range.start.character,
                token_type: 0,
                token_modifiers_bitset,
            }
        })
        .collect();
    Some(SemanticTokensResult::Tokens(SemanticTokens {
        result_id: None,
        data,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_lsp::{router::Router, LanguageServer, MainLoop};
    use futures::FutureExt;
    use lsp_types::{DidCloseTextDocumentParams, FoldingRangeParams, SemanticTokensParams, Url};
    use plumb_workspace::PendingDocumentAnalysis;
    use serde_json::json;

    const PATH: &str = "/tmp/plumb-pending-decoration-test.plumb";
    const DONE: &str = "`- Closed\n `+ task\n `= done 2026-09-06T00:00:00Z\n";

    fn server() -> ServerState {
        let (_main, client) = MainLoop::new_server(|_| Router::new(()));
        let mut state = ServerState::new(client);
        state.supports_folding_collapsed_text = true;
        state
            .workspace
            .open_document(PATH, 1, "`- Open\n `+ task\n");
        state.open_documents.insert(uri(), PATH.into());
        state
    }

    fn uri() -> Url {
        Url::from_file_path(PATH).unwrap()
    }

    fn begin(
        state: &mut ServerState,
        version: i64,
        source: &str,
    ) -> (u64, PendingDocumentAnalysis) {
        state.pending_document_reads.remove(Path::new(PATH));
        let (_, generation) = state.document_analysis_tokens.next(Path::new(PATH));
        let pending = state
            .workspace
            .begin_document_revision_with_change(PATH, version, source, None)
            .unwrap();
        (generation, pending)
    }

    fn finish(state: &mut ServerState, generation: u64, pending: PendingDocumentAnalysis) {
        let _ = state.finish_document_analysis(super::super::DocumentAnalysisResult {
            path: PATH.into(),
            generation,
            analysis: Ok(pending.analyze()),
        });
    }

    fn tokens_params() -> SemanticTokensParams {
        serde_json::from_value(json!({"textDocument":{"uri":uri()}})).unwrap()
    }

    fn fold_params() -> FoldingRangeParams {
        serde_json::from_value(json!({"textDocument":{"uri":uri()}})).unwrap()
    }

    #[tokio::test]
    async fn semantic_reads_wait_for_one_revision_without_waiting_for_initial_index() {
        let mut state = server();
        let (generation, pending) = begin(&mut state, 2, DONE);
        let mut tokens = state.semantic_tokens_full(tokens_params());
        let mut folds = state.folding_range(fold_params());
        assert!(tokens.as_mut().now_or_never().is_none());
        assert!(folds.as_mut().now_or_never().is_none());
        assert_eq!(state.pending_document_reads.len(), 1);

        let formatting = state.formatting(
            serde_json::from_value(json!({
                "textDocument":{"uri":uri()},"options":{"tabSize":1,"insertSpaces":true}
            }))
            .unwrap(),
        );
        assert!(formatting.now_or_never().unwrap().is_ok());
        assert!(!state.index_complete);
        finish(&mut state, generation, pending);

        let tokens = serde_json::to_value(tokens.await.unwrap()).unwrap();
        assert!(!tokens["data"].as_array().unwrap().is_empty());
        assert!(folds
            .await
            .unwrap()
            .unwrap()
            .iter()
            .any(|range| { range.collapsed_text.as_deref() == Some("`- [o]  Closed") }));
        assert!(state.pending_document_reads.is_empty());
        assert!(!state.index_complete);
    }

    #[tokio::test]
    async fn folds_without_labels_and_invalid_semantic_reads_do_not_wait() {
        let mut state = server();
        let (_, _pending) = begin(&mut state, 2, DONE);
        state.supports_folding_collapsed_text = false;
        let folds = state
            .folding_range(fold_params())
            .now_or_never()
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!folds.is_empty());
        assert!(folds.iter().all(|range| range.collapsed_text.is_none()));
        assert!(state.pending_document_reads.is_empty());

        state.update(uri(), 3, "`- {invalid\n `+ task\n".into(), None, false);
        state.supports_folding_collapsed_text = true;
        assert!(state
            .folding_range(fold_params())
            .now_or_never()
            .unwrap()
            .is_ok());
        assert!(state
            .semantic_tokens_full(tokens_params())
            .now_or_never()
            .unwrap()
            .unwrap()
            .is_none());
        assert!(state.pending_document_reads.is_empty());
    }

    #[tokio::test]
    async fn newer_source_rejects_old_requests_and_stale_analysis_cannot_wake_new_reads() {
        let mut state = server();
        let (old_generation, old_analysis) = begin(&mut state, 2, DONE);
        let old_tokens = state.semantic_tokens_full(tokens_params());
        let old_folds = state.folding_range(fold_params());
        state.update(uri(), 2, "`- New\n `+ task\n".into(), None, false);
        assert_eq!(
            old_tokens.await.unwrap_err().code,
            ErrorCode::CONTENT_MODIFIED
        );
        assert_eq!(
            old_folds.await.unwrap_err().code,
            ErrorCode::CONTENT_MODIFIED
        );

        let (new_generation, new_analysis) =
            begin(&mut state, 3, &DONE.replace("Closed", "Newest"));
        let mut new_folds = state.folding_range(fold_params());
        finish(&mut state, old_generation, old_analysis);
        assert!(new_folds.as_mut().now_or_never().is_none());
        finish(&mut state, new_generation, new_analysis);
        assert!(new_folds
            .await
            .unwrap()
            .unwrap()
            .iter()
            .any(|range| { range.collapsed_text.as_deref() == Some("`- [o]  Newest") }));
        assert!(state.pending_document_reads.is_empty());
    }

    #[tokio::test]
    async fn closing_or_renaming_a_document_releases_pending_requests() {
        for rename in [false, true] {
            let mut state = server();
            let (_, _analysis) = begin(&mut state, 2, DONE);
            let tokens = state.semantic_tokens_full(tokens_params());
            let folds = state.folding_range(fold_params());
            if rename {
                state.begin_path_rename(PATH.into(), "/tmp/plumb-renamed-test.plumb".into());
            } else {
                let _ = state.did_close(DidCloseTextDocumentParams {
                    text_document: lsp_types::TextDocumentIdentifier { uri: uri() },
                });
            }
            assert_eq!(tokens.await.unwrap_err().code, ErrorCode::CONTENT_MODIFIED);
            assert_eq!(folds.await.unwrap_err().code, ErrorCode::CONTENT_MODIFIED);
            assert!(state.pending_document_reads.is_empty());
        }
    }

    #[tokio::test]
    async fn reopened_document_does_not_reuse_an_old_analysis_generation() {
        let mut state = server();
        let (old_generation, old_analysis) = begin(&mut state, 2, DONE);
        let old_tokens = state.semantic_tokens_full(tokens_params());
        let _ = state.did_close(DidCloseTextDocumentParams {
            text_document: lsp_types::TextDocumentIdentifier { uri: uri() },
        });
        assert_eq!(
            old_tokens.await.unwrap_err().code,
            ErrorCode::CONTENT_MODIFIED
        );
        state
            .workspace
            .open_document(PATH, 1, "`- Reopened\n `+ task\n");
        state.open_documents.insert(uri(), PATH.into());
        let (generation, analysis) = begin(&mut state, 2, &DONE.replace("Closed", "Reopened"));
        let mut folds = state.folding_range(fold_params());
        finish(&mut state, old_generation, old_analysis);
        assert!(folds.as_mut().now_or_never().is_none());
        finish(&mut state, generation, analysis);
        assert!(folds
            .await
            .unwrap()
            .unwrap()
            .iter()
            .any(|range| { range.collapsed_text.as_deref() == Some("`- [o]  Reopened") }));
    }

    #[tokio::test]
    async fn cancellation_releases_its_receiver_without_canceling_shared_analysis() {
        let mut state = server();
        let (generation, pending) = begin(&mut state, 2, DONE);
        let canceled = state.semantic_tokens_full(tokens_params());
        let folds = state.folding_range(fold_params());
        assert_eq!(
            state.pending_document_reads[Path::new(PATH)]
                .result
                .receiver_count(),
            2
        );
        drop(canceled);
        assert_eq!(
            state.pending_document_reads[Path::new(PATH)]
                .result
                .receiver_count(),
            1
        );
        finish(&mut state, generation, pending);
        assert!(folds.await.unwrap().unwrap()[0].collapsed_text.is_some());
        assert!(state.pending_document_reads.is_empty());
    }

    #[tokio::test]
    async fn synchronous_navigation_completion_also_resolves_pending_decorations() {
        let mut state = server();
        let (generation, analysis) = begin(&mut state, 2, DONE);
        let tokens = state.semantic_tokens_full(tokens_params());
        let folds = state.folding_range(fold_params());
        assert!(state.complete_pending_navigation_documents([PATH.into()]));
        assert!(tokens.await.unwrap().is_some());
        assert!(folds.await.unwrap().unwrap()[0].collapsed_text.is_some());
        finish(&mut state, generation, analysis);
        assert!(state.pending_document_reads.is_empty());
    }

    #[tokio::test]
    async fn worker_failure_ends_current_and_later_reads_until_the_revision_recovers() {
        for observed in [false, true] {
            let mut state = server();
            let (generation, _analysis) = begin(&mut state, 2, DONE);
            let request = observed.then(|| state.semantic_tokens_full(tokens_params()));
            let _ = state.finish_document_analysis(super::super::DocumentAnalysisResult {
                path: PATH.into(),
                generation,
                analysis: Err(()),
            });
            if let Some(request) = request {
                assert_eq!(request.await.unwrap_err().code, ErrorCode::INTERNAL_ERROR);
            }
            assert_eq!(
                state
                    .semantic_tokens_full(tokens_params())
                    .now_or_never()
                    .unwrap()
                    .unwrap_err()
                    .code,
                ErrorCode::INTERNAL_ERROR
            );
            assert_eq!(
                state
                    .folding_range(fold_params())
                    .now_or_never()
                    .unwrap()
                    .unwrap_err()
                    .code,
                ErrorCode::INTERNAL_ERROR
            );
            assert!(state.complete_pending_navigation_documents([PATH.into()]));
            assert!(state
                .semantic_tokens_full(tokens_params())
                .await
                .unwrap()
                .is_some());
            assert!(state.pending_document_reads.is_empty());
        }
    }
}
