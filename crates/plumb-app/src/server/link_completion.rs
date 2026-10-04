//! Link requests wait for their actual dependencies, never the whole workspace.
use super::*;
use plumb_semantics::LinkCompletionContext;
use plumb_workspace::DocumentRevision;
use tokio::sync::oneshot;

type Response = Result<Option<CompletionResponse>, ResponseError>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Dependency {
    Index,
    Document(PathBuf),
}

struct Pending {
    source: PathBuf,
    target: Option<PathBuf>,
    revision: i64,
    parsed: Arc<DocumentRevision>,
    context: LinkCompletionContext,
    dependency: Dependency,
    result: oneshot::Sender<Response>,
}

#[derive(Default)]
pub(super) struct LinkCompletionWaiters {
    next: u64,
    requests: HashMap<u64, Pending>,
    sources: HashMap<PathBuf, HashSet<u64>>,
    targets: HashMap<PathBuf, HashSet<u64>>,
    dependencies: HashMap<Dependency, HashSet<u64>>,
    failed: HashMap<PathBuf, Arc<DocumentRevision>>,
}

pub(crate) struct CancelLinkCompletion(u64);
struct CancelOnDrop {
    id: u64,
    client: ClientSocket,
}
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        let _ = self.client.emit(CancelLinkCompletion(self.id));
    }
}

impl LinkCompletionWaiters {
    fn insert(&mut self, id: u64, request: Pending) {
        if let Some(target) = &request.target {
            self.targets.entry(target.clone()).or_default().insert(id);
        }
        self.sources
            .entry(request.source.clone())
            .or_default()
            .insert(id);
        self.dependencies
            .entry(request.dependency.clone())
            .or_default()
            .insert(id);
        self.requests.insert(id, request);
    }

    fn remove(&mut self, id: u64) -> Option<Pending> {
        let request = self.requests.remove(&id)?;
        if let Some(target) = &request.target {
            if let Some(ids) = self.targets.get_mut(target) {
                ids.remove(&id);
                if ids.is_empty() {
                    self.targets.remove(target);
                }
            }
        }
        if let Some(ids) = self.sources.get_mut(&request.source) {
            ids.remove(&id);
            if ids.is_empty() {
                self.sources.remove(&request.source);
            }
        }
        if let Some(ids) = self.dependencies.get_mut(&request.dependency) {
            ids.remove(&id);
            if ids.is_empty() {
                self.dependencies.remove(&request.dependency);
            }
        }
        Some(request)
    }
}

impl ServerState {
    pub(crate) fn cancel_link_completion(
        &mut self,
        event: CancelLinkCompletion,
    ) -> ControlFlow<async_lsp::Result<()>> {
        self.link_completion_waiters.remove(event.0);
        ControlFlow::Continue(())
    }

    pub(super) fn invalidate_link_completions(&mut self, path: &Path) {
        let waiters = &mut self.link_completion_waiters;
        waiters.failed.remove(path);
        let mut ids = waiters.sources.get(path).cloned().unwrap_or_default();
        ids.extend(waiters.targets.get(path).into_iter().flatten());
        for id in ids {
            if let Some(request) = waiters.remove(id) {
                let _ = request.result.send(Err(ResponseError::new(
                    ErrorCode::CONTENT_MODIFIED,
                    "completion dependency changed",
                )));
            }
        }
    }

    pub(super) fn fail_link_completions(&mut self, path: &Path) {
        if let Some(entry) = self.workspace.get(path) {
            self.link_completion_waiters
                .failed
                .insert(path.to_owned(), Arc::clone(&entry.parsed));
        }
        self.resume_link_completions(Dependency::Document(path.to_owned()));
    }

    pub(super) fn finish_link_completion_document(&mut self, path: &Path) {
        self.resume_link_completions(Dependency::Document(path.to_owned()));
    }

    pub(super) fn finish_link_completion_index(&mut self) {
        self.resume_link_completions(Dependency::Index);
    }

    fn link_completion_dependency(
        &self,
        source: &Path,
        context: &LinkCompletionContext,
    ) -> Result<Option<Dependency>, ResponseError> {
        if !self.implicit_root && !self.roots.is_empty() && !self.index_complete {
            return if self.index_pending {
                Ok(Some(Dependency::Index))
            } else {
                Err(workspace_query_response_error(
                    WorkspaceQueryError::Incomplete,
                ))
            };
        }
        if let Some(path) = self
            .workspace
            .pending_link_completion_target(source, context)
        {
            if self
                .link_completion_waiters
                .failed
                .get(&path)
                .is_some_and(|failed| {
                    self.workspace
                        .get(&path)
                        .is_some_and(|entry| Arc::ptr_eq(failed, &entry.parsed))
                })
            {
                return Err(ResponseError::new(
                    ErrorCode::INTERNAL_ERROR,
                    "completion target semantic analysis failed",
                ));
            }
            return Ok(Some(Dependency::Document(path)));
        }
        Ok(None)
    }

    fn link_completion_response(
        &self,
        source: &Path,
        parsed: &DocumentRevision,
        context: &LinkCompletionContext,
    ) -> Response {
        let kind = match context {
            LinkCompletionContext::Anchor { .. } | LinkCompletionContext::VerbatimAnchor { .. } => {
                CompletionItemKind::REFERENCE
            }
            _ => CompletionItemKind::FILE,
        };
        let candidates = self
            .workspace
            .complete_link(source, context)
            .and_then(QueryResult::require_complete)
            .map_err(workspace_query_response_error)?;
        Ok(Some(CompletionResponse::Array(completion_items(
            parsed.source(),
            candidates,
            kind,
        ))))
    }

    pub(super) fn request_link_completion(
        &mut self,
        source: PathBuf,
        context: LinkCompletionContext,
    ) -> BoxFuture<'static, Response> {
        let Some(entry) = self.workspace.get(&source) else {
            return Box::pin(async { Ok(None) });
        };
        let dependency = match self.link_completion_dependency(&source, &context) {
            Err(error) => return Box::pin(async { Err(error) }),
            Ok(None) => {
                let response = self.link_completion_response(&source, &entry.parsed, &context);
                return Box::pin(async { response });
            }
            Ok(Some(dependency)) => dependency,
        };
        let (sender, receiver) = oneshot::channel();
        let pending = Pending {
            target: self.workspace.link_completion_target(&source, &context),
            source,
            revision: entry.revision,
            parsed: Arc::clone(&entry.parsed),
            context,
            dependency,
            result: sender,
        };
        let id = self.link_completion_waiters.next;
        self.link_completion_waiters.next += 1;
        self.link_completion_waiters.insert(id, pending);
        let cancel = CancelOnDrop {
            id,
            client: self.client.clone(),
        };
        Box::pin(async move {
            let _cancel = cancel;
            receiver.await.unwrap_or_else(|_| {
                Err(ResponseError::new(
                    ErrorCode::CONTENT_MODIFIED,
                    "completion request invalidated",
                ))
            })
        })
    }

    fn resume_link_completions(&mut self, dependency: Dependency) {
        let ids = self
            .link_completion_waiters
            .dependencies
            .get(&dependency)
            .cloned()
            .unwrap_or_default();
        for id in ids {
            let Some(mut request) = self.link_completion_waiters.remove(id) else {
                continue;
            };
            if request.result.is_closed() {
                continue;
            }
            if !self.workspace.get(&request.source).is_some_and(|entry| {
                entry.revision == request.revision && Arc::ptr_eq(&entry.parsed, &request.parsed)
            }) {
                let _ = request.result.send(Err(ResponseError::new(
                    ErrorCode::CONTENT_MODIFIED,
                    "completion source changed",
                )));
                continue;
            }
            let result = match self.link_completion_dependency(&request.source, &request.context) {
                Ok(Some(dependency)) => {
                    request.dependency = dependency;
                    self.link_completion_waiters.insert(id, request);
                    continue;
                }
                Ok(None) => self.link_completion_response(
                    &request.source,
                    &request.parsed,
                    &request.context,
                ),
                Err(error) => Err(error),
            };
            let _ = request.result.send(result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_lsp::{router::Router, LanguageServer, MainLoop};
    use futures::FutureExt;
    use plumb_workspace::PendingDocumentAnalysis;

    const SOURCE: &str = "/tmp/plumb-link-wait/inbox.plumb";
    const TARGET: &str = "/tmp/plumb-link-wait/target.plumb";

    fn state(source: &str) -> ServerState {
        let (_, client) = MainLoop::new_server(|_| Router::new(()));
        let mut state = ServerState::new(client);
        state.roots = vec![PathBuf::from("/tmp/plumb-link-wait")];
        state.index_complete = true;
        state.index_pending = false;
        state.workspace.open_document(SOURCE, 1, source);
        state
            .open_documents
            .insert(Url::from_file_path(SOURCE).unwrap(), SOURCE.into());
        state
            .workspace
            .open_document(TARGET, 1, "`= title 三角洲\n`# Old\n `@ old\n");
        state
            .open_documents
            .insert(Url::from_file_path(TARGET).unwrap(), TARGET.into());
        state
    }

    fn begin(state: &mut ServerState, path: &str, source: &str) -> (u64, PendingDocumentAnalysis) {
        state.invalidate_link_completions(Path::new(path));
        let (_, generation) = state.document_analysis_tokens.next(Path::new(path));
        let pending = state
            .workspace
            .begin_document_revision(path, 2, source)
            .unwrap();
        (generation, pending)
    }

    fn finish(
        state: &mut ServerState,
        path: &str,
        generation: u64,
        pending: PendingDocumentAnalysis,
    ) {
        let _ = state.finish_document_analysis(DocumentAnalysisResult {
            path: path.into(),
            generation,
            analysis: Ok(pending.analyze()),
        });
    }

    fn request(state: &mut ServerState) -> BoxFuture<'static, Response> {
        let source = state.workspace.get(SOURCE).unwrap().parsed.source();
        let offset = source.find('}').unwrap();
        let params = serde_json::from_value(serde_json::json!({
            "textDocument": {"uri": Url::from_file_path(SOURCE).unwrap()},
            "position": byte_range_to_lsp(source, &(offset..offset)).start
        }))
        .unwrap();
        state.completion(params)
    }

    fn items(response: Response) -> Vec<CompletionItem> {
        let Some(CompletionResponse::Array(items)) = response.unwrap() else {
            panic!("completion items")
        };
        items
    }

    #[tokio::test]
    async fn snippet_replacement_returns_paths_before_source_or_unrelated_analysis_install() {
        let mut state = state("`->{target/label}");
        begin(&mut state, SOURCE, "`->{三}");
        begin(
            &mut state,
            "/tmp/plumb-link-wait/unrelated.plumb",
            "Pending\n",
        );
        let items = items(
            request(&mut state)
                .now_or_never()
                .expect("path completion must be immediate"),
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "target.plumb");
        let Some(CompletionTextEdit::Edit(edit)) = &items[0].text_edit else {
            panic!("text edit")
        };
        assert_eq!(
            edit.range,
            lsp_types::Range::new(
                lsp_types::Position::new(0, 4),
                lsp_types::Position::new(0, 5)
            )
        );
        assert_eq!(edit.new_text, "target.plumb");
        assert!(state.link_completion_waiters.requests.is_empty());
    }

    #[tokio::test]
    async fn anchors_wait_for_only_target_and_return_new_facts_without_retry() {
        let mut state = state("`->{target.plumb#}");
        let (generation, pending) = begin(&mut state, TARGET, "`# New\n `@ new\n");
        begin(
            &mut state,
            "/tmp/plumb-link-wait/unrelated.plumb",
            "Pending\n",
        );
        let mut response = request(&mut state);
        assert!(response.as_mut().now_or_never().is_none());
        assert_eq!(state.link_completion_waiters.requests.len(), 1);
        finish(&mut state, TARGET, generation, pending);
        assert_eq!(
            items(response.await)
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            vec!["#new"]
        );
        assert!(state.link_completion_waiters.requests.is_empty());
    }

    #[tokio::test]
    async fn pending_anchor_requests_are_invalidated_by_source_or_target_edits_and_close() {
        for path in [SOURCE, TARGET] {
            for close in [false, true] {
                let mut state = state("`->{target.plumb#}");
                begin(&mut state, TARGET, "`# New\n `@ new\n");
                let response = request(&mut state);
                if close {
                    let _ = state.did_close(
                        serde_json::from_value(serde_json::json!({
                            "textDocument": {"uri": Url::from_file_path(path).unwrap()}
                        }))
                        .unwrap(),
                    );
                } else {
                    begin(&mut state, path, "`->{other}");
                }
                assert_eq!(
                    response.await.unwrap_err().code,
                    ErrorCode::CONTENT_MODIFIED
                );
                assert!(state.link_completion_waiters.requests.is_empty());
                assert!(state.link_completion_waiters.sources.is_empty());
                assert!(state.link_completion_waiters.dependencies.is_empty());
            }
        }
    }

    #[tokio::test]
    async fn worker_failure_finishes_waiting_and_future_requests() {
        let mut state = state("`->{target.plumb#}");
        let (generation, _) = begin(&mut state, TARGET, "`# New\n `@ new\n");
        let response = request(&mut state);
        let _ = state.finish_document_analysis(DocumentAnalysisResult {
            path: TARGET.into(),
            generation,
            analysis: Err(()),
        });
        assert_eq!(response.await.unwrap_err().code, ErrorCode::INTERNAL_ERROR);
        assert!(request(&mut state).now_or_never().unwrap().is_err());
        assert!(state.link_completion_waiters.requests.is_empty());
    }

    #[tokio::test]
    async fn cold_index_waits_and_then_chains_to_the_target_dependency() {
        let mut state = state("`->{target.plumb#}");
        state.index_complete = false;
        state.index_pending = true;
        let (generation, pending) = begin(&mut state, TARGET, "`# New\n `@ new\n");
        let mut response = request(&mut state);
        assert!(response.as_mut().now_or_never().is_none());
        state.index_pending = false;
        state.index_complete = true;
        state.finish_link_completion_index();
        assert!(response.as_mut().now_or_never().is_none());
        finish(&mut state, TARGET, generation, pending);
        assert_eq!(items(response.await)[0].label, "#new");
    }

    #[tokio::test]
    async fn target_change_during_initial_index_wait_invalidates_anchor_request() {
        let mut state = state("`->{target.plumb#}");
        state.index_complete = false;
        state.index_pending = true;
        let response = request(&mut state);
        begin(&mut state, TARGET, "`# New\n `@ new\n");
        assert_eq!(
            response.await.unwrap_err().code,
            ErrorCode::CONTENT_MODIFIED
        );
        assert!(state.link_completion_waiters.requests.is_empty());
        assert!(state.link_completion_waiters.targets.is_empty());
        assert!(state.link_completion_waiters.dependencies.is_empty());
    }

    #[tokio::test]
    async fn disk_target_change_during_initial_index_wait_invalidates_anchor_request() {
        let mut state = state("`->{target.plumb#}");
        state
            .open_documents
            .remove(&Url::from_file_path(TARGET).unwrap());
        state.index_complete = false;
        state.index_pending = true;
        let response = request(&mut state);
        let _ = state.did_change_watched_files(
            serde_json::from_value(serde_json::json!({
                "changes": [{"uri": Url::from_file_path(TARGET).unwrap(), "type": 2}]
            }))
            .unwrap(),
        );
        assert_eq!(
            response.await.unwrap_err().code,
            ErrorCode::CONTENT_MODIFIED
        );
        assert!(state.link_completion_waiters.targets.is_empty());
    }

    #[tokio::test]
    async fn failed_initial_index_wakes_requests_as_errors() {
        let mut state = state("`->{三}");
        state.index_complete = false;
        state.index_pending = true;
        let mut response = request(&mut state);
        assert!(response.as_mut().now_or_never().is_none());
        state.index_pending = false;
        state.finish_link_completion_index();
        assert!(response.await.is_err());
    }

    #[tokio::test]
    async fn canceled_request_removes_reverse_waiting_edges() {
        let mut state = state("`->{target.plumb#}");
        begin(&mut state, TARGET, "`# New\n `@ new\n");
        let response = request(&mut state);
        let id = *state
            .link_completion_waiters
            .requests
            .keys()
            .next()
            .unwrap();
        drop(response);
        // The main loop receives this event from the future's drop guard.
        let _ = state.cancel_link_completion(CancelLinkCompletion(id));
        assert!(state.link_completion_waiters.requests.is_empty());
        assert!(state.link_completion_waiters.sources.is_empty());
        assert!(state.link_completion_waiters.dependencies.is_empty());
    }
}
