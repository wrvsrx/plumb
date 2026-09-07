use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_lsp::{ClientSocket, ErrorCode, LanguageClient, LanguageServer, ResponseError};
use chrono::{Local, SecondsFormat};
use futures::future::BoxFuture;
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOptions, CodeActionOrCommand, CodeActionParams,
    CodeActionProviderCapability, CodeActionResponse, CodeLens, CodeLensOptions, CodeLensParams,
    Command, CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams,
    CompletionResponse, CompletionTextEdit, Diagnostic as LspDiagnostic,
    DiagnosticRelatedInformation, DiagnosticSeverity, DidChangeConfigurationParams,
    DidChangeTextDocumentParams, DidChangeWatchedFilesParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentChangeOperation, DocumentChanges,
    DocumentFormattingParams, DocumentRangeFormattingParams, DocumentSymbolParams,
    DocumentSymbolResponse, FileChangeType, FileSystemWatcher, FoldingRange, FoldingRangeParams,
    FoldingRangeProviderCapability, GlobPattern, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverContents, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult,
    InitializedParams, InsertTextFormat, InsertTextMode, Location, MarkupContent, MarkupKind,
    NumberOrString, OneOf, OptionalVersionedTextDocumentIdentifier, PrepareRenameResponse,
    ProgressParams, ProgressParamsValue, PublishDiagnosticsParams, ReferenceParams, Registration,
    RegistrationParams, RenameFile, RenameFileOptions, RenameOptions, RenameParams, ResourceOp,
    ResourceOperationKind, SemanticTokenModifier, SemanticTokenType, SemanticTokensFullOptions,
    SemanticTokensLegend, SemanticTokensOptions, SemanticTokensParams, SemanticTokensResult,
    SemanticTokensServerCapabilities, ServerCapabilities, SymbolInformation, SymbolKind,
    TextDocumentContentChangeEvent, TextDocumentEdit, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextEdit as LspTextEdit, Url, WatchKind, WorkDoneProgress,
    WorkDoneProgressBegin, WorkDoneProgressEnd, WorkDoneProgressOptions, WorkDoneProgressReport,
    WorkspaceEdit as LspWorkspaceEdit, WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use plumb_semantics::{
    green_attribute_completion_context as attribute_completion_context,
    green_citation_completion_context as citation_completion_context,
    green_construct_completion_context as construct_completion_context,
    green_event_title_completion_context as event_title_completion_context,
    green_file_completion_context as file_completion_context,
    green_image_completion_context as image_completion_context,
    green_link_completion_context as link_completion_context, green_recovered_bibliography_sources,
    green_task_dependency_completion_context as task_dependency_completion_context, AnchorKind,
    ConstructCompletionContext, TaskStatus,
};
use plumb_syntax::{Diagnostic, SourceChange};
use plumb_workspace::{
    load_bibliography, load_bibliography_sources, normalize, scan_workspace_files,
    BatchIndexOptions, Bibliography, BibliographyResolution, CompletionCandidate,
    ExportedSemanticChange, PathRenameInput, PreparedDocumentAnalysis, QueryResult, RenameError,
    ResolvedTarget, ResourceOperation, SearchRecord, SearchRecordKind, SqliteSemanticStore,
    Workspace, WorkspaceDiagnosticContext, WorkspaceEdit, WorkspaceOperationError,
    WorkspaceQueryError, WorkspaceSearchError,
};
use sha2::{Digest, Sha256};

#[cfg(test)]
use crate::folding::ranges as folding_ranges;
use crate::folding::{collapsed_text_labels as fold_labels, green_ranges as green_folding_ranges};
#[cfg(test)]
use crate::hover::fenced_plumb;
use crate::hover::{
    event as event_hover, file as file_hover, image as image_hover, link as link_hover,
    metadata as metadata_hover, target as target_hover, task as task_hover,
};
use crate::position::{byte_range_to_lsp, position_to_offset, LineIndex, PositionIndex};
use crate::search::{SearchItem, SearchKind, SearchParams, SearchProvenance, SearchResult};
#[cfg(test)]
use crate::semantic_tokens::{closed_task_token_ranges, physical_line_ranges};
use crate::symbols::{
    anchor as anchor_symbol, events as event_symbols, heading as heading_symbol,
    insert_all as insert_document_symbols, metadata as metadata_symbol, tasks as task_symbols,
};

mod completion;
mod decorations;

use decorations::{semantic_tokens, PendingDocumentReads};

use completion::{
    attribute_completion_text, completion_indentation, completion_items,
    completion_items_with_kind, construct_completion_items, CompletionIndentation,
};
#[cfg(test)]
use completion::{task_construct_template, CompletionIndentationProjection};

pub(crate) struct ServerState {
    client: ClientSocket,
    workspace: Workspace,
    open_documents: HashMap<Url, PathBuf>,
    open_document_line_indexes: HashMap<PathBuf, LineIndex>,
    roots: Vec<PathBuf>,
    supports_document_changes: bool,
    supports_resource_rename: bool,
    supports_dynamic_watching: bool,
    supports_completion_snippets: bool,
    completion_indentation: CompletionIndentation,
    supports_code_lens_refresh: bool,
    code_lens_refresh_pending: bool,
    supports_folding_range_refresh: bool,
    folding_range_limit: Option<usize>,
    supports_folding_collapsed_text: bool,
    line_folding_only: bool,
    index_complete: bool,
    index_generation: u64,
    pending_path_renames: Vec<PendingPathRename>,
    document_analysis_tokens: DocumentAnalysisTokens,
    pending_document_reads: HashMap<PathBuf, PendingDocumentReads>,
    diagnostic_context: Option<Arc<WorkspaceDiagnosticContext>>,
}

#[derive(Default)]
struct DocumentAnalysisTokens {
    paths: HashMap<PathBuf, Arc<AtomicU64>>,
    next_generation: u64,
}

impl DocumentAnalysisTokens {
    fn next(&mut self, path: &Path) -> (Arc<AtomicU64>, u64) {
        let token = self
            .paths
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(AtomicU64::new(0)))
            .clone();
        self.next_generation = self.next_generation.wrapping_add(1);
        let generation = self.next_generation;
        token.store(generation, Ordering::Release);
        (token, generation)
    }

    fn is_current(&self, path: &Path, generation: u64) -> bool {
        self.paths
            .get(path)
            .is_some_and(|token| token.load(Ordering::Acquire) == generation)
    }

    fn cancel(&mut self, path: &Path) {
        if let Some(token) = self.paths.remove(path) {
            token.fetch_add(1, Ordering::AcqRel);
        }
    }
}

pub(crate) struct InitialIndexResult {
    generation: u64,
    workspace: Workspace,
    indexed: usize,
    cache_hits: usize,
    complete: bool,
}

pub(crate) struct DocumentAnalysisResult {
    path: PathBuf,
    generation: u64,
    analysis: Result<PreparedDocumentAnalysis, ()>,
}

struct PendingPathRename {
    old_path: PathBuf,
    new_path: PathBuf,
    old_removed: bool,
    new_seen: bool,
}

enum FoldingRangeRefresh {}

impl lsp_types::request::Request for FoldingRangeRefresh {
    type Params = ();
    type Result = ();
    const METHOD: &'static str = "workspace/foldingRange/refresh";
}

impl ServerState {
    pub(crate) fn new(client: ClientSocket) -> Self {
        Self {
            client,
            workspace: Workspace::new(),
            open_documents: HashMap::new(),
            open_document_line_indexes: HashMap::new(),
            roots: Vec::new(),
            supports_document_changes: false,
            supports_resource_rename: false,
            supports_dynamic_watching: false,
            supports_completion_snippets: false,
            completion_indentation: CompletionIndentation::default(),
            supports_code_lens_refresh: false,
            code_lens_refresh_pending: false,
            supports_folding_range_refresh: false,
            folding_range_limit: None,
            supports_folding_collapsed_text: false,
            line_folding_only: false,
            index_complete: false,
            index_generation: 0,
            pending_path_renames: Vec::new(),
            document_analysis_tokens: DocumentAnalysisTokens::default(),
            pending_document_reads: HashMap::new(),
            diagnostic_context: None,
        }
    }

    fn update(
        &mut self,
        uri: Url,
        version: i32,
        text: String,
        source_change: Option<SourceChange>,
        background: bool,
    ) {
        let Ok(path) = uri.to_file_path() else {
            return;
        };
        let path = normalize(&path);
        self.pending_document_reads.remove(&path);
        let revision = i64::from(version);
        let (token, generation) = self.document_analysis_tokens.next(&path);
        let rebound = self
            .workspace
            .rebind_revision_if_source(&path, revision, &text);
        let mut semantic_analysis_pending = false;
        if !rebound {
            if background {
                let pending = self.workspace.begin_document_revision_with_change(
                    &path,
                    revision,
                    text,
                    source_change,
                );
                if let Some(pending) = pending {
                    semantic_analysis_pending = true;
                    let client = self.client.clone();
                    let analysis_path = path.clone();
                    tokio::task::spawn_blocking(move || {
                        if token.load(Ordering::Acquire) != generation {
                            return;
                        }
                        let analysis =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                pending.analyze()
                            }))
                            .map_err(|_| ());
                        if token.load(Ordering::Acquire) == generation {
                            let _ = client.emit(DocumentAnalysisResult {
                                path: analysis_path,
                                generation,
                                analysis,
                            });
                        }
                    });
                }
            } else {
                self.workspace.insert(&path, revision, text);
            }
        }
        self.open_documents.insert(uri.clone(), path.clone());
        if semantic_analysis_pending {
            self.publish_syntax_diagnostics(&uri, &path);
        } else if rebound && background {
            self.publish_open_document_diagnostics(&path);
        } else {
            self.publish_all_open_diagnostics();
            self.refresh_code_lenses();
            self.refresh_folding_ranges();
        }
    }

    pub(crate) fn finish_document_analysis(
        &mut self,
        result: DocumentAnalysisResult,
    ) -> ControlFlow<async_lsp::Result<()>> {
        if !self
            .document_analysis_tokens
            .is_current(&result.path, result.generation)
        {
            return ControlFlow::Continue(());
        }
        let Ok(analysis) = result.analysis else {
            tracing::error!(path = %result.path.display(), "document semantic analysis failed");
            self.fail_pending_document_reads(&result.path);
            return ControlFlow::Continue(());
        };
        let Some(impact) = self
            .workspace
            .install_document_analysis_with_impact(analysis)
        else {
            self.pending_document_reads.remove(&result.path);
            return ControlFlow::Continue(());
        };
        self.finish_pending_document_reads(&result.path);
        match impact.exported {
            ExportedSemanticChange::Changed => {
                if impact.task_graph_changed {
                    self.publish_all_open_diagnostics();
                } else if self.diagnostic_context.is_none() {
                    self.publish_all_open_diagnostics_reusing_context();
                } else if let Some(ids) = &impact.diagnostic_targets {
                    self.publish_diagnostics_for_targets(&result.path, ids);
                } else if impact.dependent_diagnostics_changed {
                    self.publish_all_open_diagnostics_reusing_context();
                } else {
                    self.publish_open_document_diagnostics(&result.path);
                }
                self.refresh_code_lenses();
                self.refresh_folding_ranges();
            }
            ExportedSemanticChange::Unchanged => {
                if self.diagnostic_context.is_some() {
                    self.publish_open_document_diagnostics(&result.path);
                } else {
                    self.publish_all_open_diagnostics();
                }
                if self.code_lens_refresh_pending {
                    self.refresh_code_lenses();
                }
            }
        }
        ControlFlow::Continue(())
    }

    fn publish_all_open_diagnostics(&mut self) {
        self.diagnostic_context = None;
        self.publish_all_open_diagnostics_reusing_context();
    }

    fn diagnostic_publication_context(&mut self) -> Option<Arc<WorkspaceDiagnosticContext>> {
        let pending = self
            .workspace
            .documents()
            .any(|entry| entry.parsed.is_valid() && entry.current.is_none());
        // Partial publication needs recovery even if the eventual exported facts are equal.
        if pending {
            self.diagnostic_context = None;
        }
        let context = match &self.diagnostic_context {
            Some(context) => Arc::clone(context),
            None => match self.workspace.diagnostic_context() {
                Ok(context) => Arc::new(context),
                Err(error) => {
                    tracing::error!(%error, "workspace diagnostic context query failed");
                    return None;
                }
            },
        };
        if !pending {
            self.diagnostic_context = Some(Arc::clone(&context));
        }
        Some(context)
    }

    fn publish_all_open_diagnostics_reusing_context(&mut self) {
        let Some(context) = self.diagnostic_publication_context() else {
            return;
        };
        for (uri, path) in &self.open_documents {
            self.publish(uri, path, context.as_ref());
        }
    }

    fn publish_open_document_diagnostics(&mut self, path: &Path) {
        let Some(context) = self.diagnostic_publication_context() else {
            return;
        };
        let Some((uri, _)) = self
            .open_documents
            .iter()
            .find(|(_, open_path)| open_path.as_path() == path)
        else {
            return;
        };
        self.publish(uri, path, context.as_ref());
    }

    fn publish_diagnostics_for_targets(&mut self, changed: &Path, ids: &HashSet<String>) {
        let Some(context) = self.diagnostic_publication_context() else {
            return;
        };
        let complete = self.diagnostic_context.is_some();
        for (uri, path) in &self.open_documents {
            if !complete
                || path == changed
                || self
                    .workspace
                    .document_may_reference_targets(path, changed, ids)
            {
                self.publish(uri, path, context.as_ref());
            }
        }
    }

    fn publish_syntax_diagnostics(&self, uri: &Url, path: &Path) {
        let Some(entry) = self.workspace.get(path) else {
            return;
        };
        let mut positions = None;
        let diagnostics = entry
            .parsed
            .diagnostics()
            .iter()
            .cloned()
            .map(|diagnostic| {
                to_lsp_diagnostic(
                    positions.get_or_insert_with(|| PositionIndex::new(entry.parsed.source())),
                    uri,
                    diagnostic,
                )
            })
            .collect();
        let version = i32::try_from(entry.revision).ok();
        let _ = self
            .client
            .notify::<lsp_types::notification::PublishDiagnostics>(PublishDiagnosticsParams {
                uri: uri.clone(),
                diagnostics,
                version,
            });
    }

    fn publish(&self, uri: &Url, path: &Path, context: &WorkspaceDiagnosticContext) {
        let Some(entry) = self.workspace.get(path) else {
            return;
        };
        let mut diagnostics = match self.workspace.diagnostics_with_context(path, context) {
            Ok(result) => result.value,
            Err(error) => {
                tracing::error!(%error, path = %path.display(), "workspace diagnostics query failed");
                return;
            }
        };
        if let Some(bibliography) = self.bibliography_for(path) {
            diagnostics.extend(bibliography.diagnostics.clone());
            if let Some(current) = &entry.current {
                diagnostics.extend(
                    bibliography.citation_diagnostics(&current.output.citations().citations),
                );
            }
        }
        let mut positions = None;
        let diagnostics = diagnostics
            .into_iter()
            .map(|diagnostic| {
                to_lsp_diagnostic(
                    positions.get_or_insert_with(|| PositionIndex::new(entry.parsed.source())),
                    uri,
                    diagnostic,
                )
            })
            .collect();
        let version = i32::try_from(entry.revision).ok();
        let _ = self
            .client
            .notify::<lsp_types::notification::PublishDiagnostics>(PublishDiagnosticsParams {
                uri: uri.clone(),
                diagnostics,
                version,
            });
    }

    fn workspace_root_for(&self, path: &Path) -> Option<&Path> {
        self.roots
            .iter()
            .filter(|root| path.starts_with(root))
            .max_by_key(|root| root.components().count())
            .map(PathBuf::as_path)
    }

    fn bibliography_for(&self, path: &Path) -> Option<Bibliography> {
        let root = self.workspace_root_for(path)?;
        let entry = self.workspace.get(path)?;
        let current = entry.current.as_ref()?;
        Some(load_bibliography(root, path, &current.output.metadata()))
    }

    fn bibliography_for_completion(&self, path: &Path) -> Option<Bibliography> {
        if let Some(bibliography) = self.bibliography_for(path) {
            return Some(bibliography);
        }
        let root = self.workspace_root_for(path)?;
        let entry = self.workspace.get(path)?;
        let sources = green_recovered_bibliography_sources(entry.parsed.green());
        Some(load_bibliography_sources(root, path, sources))
    }

    fn index_roots(&mut self) -> (usize, bool) {
        self.notify_index_progress(WorkDoneProgress::Begin(WorkDoneProgressBegin {
            title: "Indexing plumb workspace".to_string(),
            cancellable: Some(false),
            message: Some("Scanning .plumb files".to_string()),
            percentage: None,
        }));
        let (files, mut complete) = self.scanned_files();
        let retained = files.iter().cloned().collect::<HashSet<_>>();
        let open = self
            .open_documents
            .values()
            .cloned()
            .collect::<HashSet<_>>();
        let stale = match self.workspace.document_paths() {
            Ok(result) => result
                .value
                .into_iter()
                .filter(|path| {
                    self.roots.iter().any(|root| path.starts_with(root))
                        && !open.contains(path)
                        && !retained.contains(path)
                })
                .collect::<Vec<_>>(),
            Err(error) => {
                tracing::error!(%error, "workspace document-path query failed during indexing");
                complete = false;
                Vec::new()
            }
        };
        for path in stale {
            let _ = self.workspace.remove_disk(path);
        }

        let mut indexed = 0;
        for path in files {
            if open.contains(&path) {
                continue;
            }
            if let Ok(text) = fs::read_to_string(&path) {
                if self.workspace.insert_disk(path, 0, text).is_ok() {
                    indexed += 1;
                } else {
                    complete = false;
                }
            } else {
                complete = false;
            }
        }
        self.notify_index_progress(WorkDoneProgress::Report(WorkDoneProgressReport {
            cancellable: Some(false),
            message: Some(format!("Indexed {indexed} files")),
            percentage: None,
        }));
        self.notify_index_progress(WorkDoneProgress::End(WorkDoneProgressEnd {
            message: Some(format!("Indexed {indexed} plumb files")),
        }));
        (indexed, complete)
    }

    fn scanned_files(&self) -> (Vec<PathBuf>, bool) {
        scanned_files(&self.roots)
    }

    fn start_initial_index(&mut self) {
        self.index_generation = self.index_generation.wrapping_add(1);
        let generation = self.index_generation;
        let roots = self.roots.clone();
        let client = self.client.clone();
        self.notify_index_progress(WorkDoneProgress::Begin(WorkDoneProgressBegin {
            title: "Indexing plumb workspace".to_string(),
            cancellable: Some(false),
            message: Some("Scanning .plumb files".to_string()),
            percentage: None,
        }));
        tokio::task::spawn_blocking(move || {
            let result = build_initial_index(&roots, generation);
            let _ = client.emit(result);
        });
    }

    pub(crate) fn finish_initial_index(
        &mut self,
        result: InitialIndexResult,
    ) -> ControlFlow<async_lsp::Result<()>> {
        if result.generation != self.index_generation {
            return ControlFlow::Continue(());
        }
        let open = self
            .open_documents
            .values()
            .filter_map(|path| self.workspace.get(path).cloned())
            .collect::<Vec<_>>();
        self.workspace = result.workspace;
        for entry in open {
            self.workspace.overlay_document_entry(entry);
        }
        self.index_complete = result.complete;
        self.notify_index_progress(WorkDoneProgress::Report(WorkDoneProgressReport {
            cancellable: Some(false),
            message: Some(format!(
                "Indexed {} files ({} cached)",
                result.indexed, result.cache_hits
            )),
            percentage: None,
        }));
        self.notify_index_progress(WorkDoneProgress::End(WorkDoneProgressEnd {
            message: Some(format!(
                "Indexed {} plumb files ({} cached)",
                result.indexed, result.cache_hits
            )),
        }));
        self.register_workspace_file_watchers();
        self.publish_all_open_diagnostics();
        self.refresh_code_lenses();
        self.refresh_folding_ranges();
        ControlFlow::Continue(())
    }

    fn notify_index_progress(&self, progress: WorkDoneProgress) {
        let _ = self
            .client
            .notify::<lsp_types::notification::Progress>(ProgressParams {
                token: NumberOrString::String("plumb-ls-index".to_string()),
                value: ProgressParamsValue::WorkDone(progress),
            });
    }

    fn register_workspace_file_watchers(&self) {
        if !self.supports_dynamic_watching || self.roots.is_empty() {
            return;
        }
        let params = RegistrationParams {
            registrations: vec![Registration {
                id: "plumb-ls-workspace-files".to_string(),
                method: "workspace/didChangeWatchedFiles".to_string(),
                register_options: Some(
                    serde_json::to_value(lsp_types::DidChangeWatchedFilesRegistrationOptions {
                        watchers: vec![
                            FileSystemWatcher {
                                glob_pattern: GlobPattern::String("**/*.plumb".to_string()),
                                kind: Some(
                                    WatchKind::Create | WatchKind::Change | WatchKind::Delete,
                                ),
                            },
                            FileSystemWatcher {
                                glob_pattern: GlobPattern::String("**/.ignore".to_string()),
                                kind: Some(
                                    WatchKind::Create | WatchKind::Change | WatchKind::Delete,
                                ),
                            },
                            FileSystemWatcher {
                                glob_pattern: GlobPattern::String("**/*.json".to_string()),
                                kind: Some(
                                    WatchKind::Create | WatchKind::Change | WatchKind::Delete,
                                ),
                            },
                        ],
                    })
                    .expect("watch registration is serializable"),
                ),
            }],
        };
        let mut client = self.client.clone();
        tokio::spawn(async move {
            let _ = client.register_capability(params).await;
        });
    }

    fn refresh_code_lenses(&mut self) {
        if !self.supports_code_lens_refresh {
            self.code_lens_refresh_pending = false;
            return;
        }
        if !self.index_complete {
            return;
        }
        if self
            .workspace
            .documents()
            .any(|entry| entry.parsed.is_valid() && entry.current.is_none())
        {
            self.code_lens_refresh_pending = true;
            return;
        }
        self.code_lens_refresh_pending = false;
        let mut client = self.client.clone();
        tokio::spawn(async move {
            let _ = client.code_lens_refresh(()).await;
        });
    }

    fn refresh_folding_ranges(&self) {
        if !self.supports_folding_range_refresh || !self.index_complete {
            return;
        }
        let client = self.client.clone();
        tokio::spawn(async move {
            let _ = client.request::<FoldingRangeRefresh>(()).await;
        });
    }

    fn begin_path_rename(&mut self, old_path: PathBuf, new_path: PathBuf) {
        self.pending_document_reads.remove(&old_path);
        self.pending_document_reads.remove(&new_path);
        self.document_analysis_tokens.cancel(&old_path);
        self.document_analysis_tokens.cancel(&new_path);
        let snapshot = self
            .workspace
            .get(&old_path)
            .map(|entry| (entry.revision, entry.parsed.source().to_string()))
            .or_else(|| fs::read_to_string(&old_path).ok().map(|source| (0, source)));
        if let Some((revision, source)) = snapshot {
            self.workspace.open_document(&new_path, revision, source);
            self.workspace.remove(&old_path);
            let _ = self.workspace.remove_disk(&old_path);
        }
        self.pending_path_renames.push(PendingPathRename {
            old_path,
            new_path,
            old_removed: false,
            new_seen: false,
        });
    }

    fn confirm_pending_path_rename(&mut self, changed_path: &Path) {
        for rename in &mut self.pending_path_renames {
            if changed_path == rename.old_path {
                rename.old_removed = !rename.old_path.exists();
                if rename.old_removed {
                    self.workspace.remove(&rename.old_path);
                    let _ = self.workspace.remove_disk(&rename.old_path);
                } else if let Ok(text) = fs::read_to_string(&rename.old_path) {
                    let _ = self.workspace.insert_disk(&rename.old_path, 0, text);
                }
                if !rename.new_path.exists()
                    && !self
                        .open_documents
                        .values()
                        .any(|open| open == &rename.new_path)
                {
                    self.workspace.remove(&rename.new_path);
                    let _ = self.workspace.remove_disk(&rename.new_path);
                    rename.new_seen = false;
                }
            } else if changed_path == rename.new_path {
                rename.new_seen = rename.new_path.exists();
                if rename.new_seen {
                    if let Ok(text) = fs::read_to_string(&rename.new_path) {
                        let _ = self.workspace.insert_disk(&rename.new_path, 0, text);
                    }
                } else if !self
                    .open_documents
                    .values()
                    .any(|open| open == &rename.new_path)
                {
                    self.workspace.remove(&rename.new_path);
                    let _ = self.workspace.remove_disk(&rename.new_path);
                }
                rename.old_removed = !rename.old_path.exists();
            }
        }
        self.pending_path_renames
            .retain(|rename| !(rename.old_removed && rename.new_seen));
    }

    fn target_at_with_lazy_load(
        &mut self,
        path: &Path,
        offset: usize,
    ) -> Result<Option<ResolvedTarget>, WorkspaceQueryError> {
        self.complete_pending_navigation_documents([path.to_path_buf()]);
        let Some(mut target) = self.navigation_query(self.workspace.target_at(path, offset))?
        else {
            return Ok(None);
        };
        if let ResolvedTarget::UnresolvedAnchor {
            path: target_path, ..
        } = &target
        {
            if self.complete_pending_navigation_documents([target_path.clone()]) {
                let Some(retried) =
                    self.navigation_query(self.workspace.target_at(path, offset))?
                else {
                    return Ok(None);
                };
                target = retried;
            }
        }
        if !self.load_unresolved_target(&target) {
            return Ok(Some(target));
        }
        self.navigation_query(self.workspace.target_at(path, offset))
    }

    fn reference_target_at_with_lazy_load(
        &mut self,
        path: &Path,
        offset: usize,
    ) -> Result<Option<ResolvedTarget>, WorkspaceQueryError> {
        self.complete_pending_navigation_documents([path.to_path_buf()]);
        let Some(mut target) =
            self.navigation_query(self.workspace.reference_target_at(path, offset))?
        else {
            return Ok(None);
        };
        if let ResolvedTarget::UnresolvedAnchor {
            path: target_path, ..
        } = &target
        {
            if self.complete_pending_navigation_documents([target_path.clone()]) {
                let Some(retried) =
                    self.navigation_query(self.workspace.reference_target_at(path, offset))?
                else {
                    return Ok(None);
                };
                target = retried;
            }
        }
        if !self.load_unresolved_target(&target) {
            return Ok(Some(target));
        }
        self.navigation_query(self.workspace.reference_target_at(path, offset))
    }

    fn load_unresolved_target(&mut self, target: &ResolvedTarget) -> bool {
        let ResolvedTarget::UnresolvedPath { path: target_path } = target else {
            return false;
        };
        if !is_plumb_file(target_path) {
            return false;
        }
        let Ok(source) = fs::read_to_string(target_path) else {
            return false;
        };
        self.workspace.insert(target_path, 0, source);
        true
    }

    fn ensure_request_document(&mut self, path: &Path) {
        if self.workspace.get(path).is_some() {
            return;
        }
        if let Ok(source) = fs::read_to_string(path) {
            self.workspace.open_document(path, 0, source);
        }
    }

    fn complete_pending_navigation_documents(
        &mut self,
        paths: impl IntoIterator<Item = PathBuf>,
    ) -> bool {
        let mut completed = false;
        for path in paths {
            if !self.workspace.document_analysis_pending(&path) {
                continue;
            }
            self.document_analysis_tokens.cancel(&path);
            if self.workspace.complete_pending_document_analysis(&path) {
                self.finish_pending_document_reads(&path);
                completed = true;
            } else {
                self.pending_document_reads.remove(&path);
            }
        }
        if completed {
            self.publish_all_open_diagnostics();
            self.refresh_code_lenses();
            self.refresh_folding_ranges();
        }
        completed
    }

    fn navigation_query<T>(
        &self,
        result: Result<QueryResult<T>, WorkspaceQueryError>,
    ) -> Result<T, WorkspaceQueryError> {
        self.require_index_complete()?;
        Ok(result?.value)
    }

    fn complete_query<T>(
        &self,
        result: Result<QueryResult<T>, WorkspaceQueryError>,
    ) -> Result<T, WorkspaceQueryError> {
        self.require_index_complete()?;
        result?.require_complete()
    }

    fn require_index_complete(&self) -> Result<(), WorkspaceQueryError> {
        if !self.roots.is_empty() && !self.index_complete {
            return Err(WorkspaceQueryError::Incomplete);
        }
        Ok(())
    }

    pub(crate) fn search(
        &self,
        params: SearchParams,
    ) -> BoxFuture<'static, Result<SearchResult, ResponseError>> {
        let workspace = self.workspace.clone();
        let roots = self.roots.clone();
        let index_complete = self.index_complete;
        Box::pin(async move {
            tokio::task::yield_now().await;
            search_workspace(&workspace, &roots, index_complete, params)
        })
    }
}

fn search_workspace(
    workspace: &Workspace,
    roots: &[PathBuf],
    index_complete: bool,
    params: SearchParams,
) -> Result<SearchResult, ResponseError> {
    let kind = params.kind.map(|kind| match kind {
        SearchKind::Note => SearchRecordKind::Note,
        SearchKind::Task => SearchRecordKind::Task,
        SearchKind::Event => SearchRecordKind::Event,
    });
    let limit = params.limit.unwrap_or(100).min(200) as usize;
    let root = roots
        .first()
        .map_or_else(|| Path::new(""), PathBuf::as_path);
    let results = workspace
        .search_records_filtered(
            root,
            kind,
            &params.query,
            limit,
            Local::now().fixed_offset(),
            params.filter.as_deref(),
        )
        .map_err(|error| match error {
            WorkspaceSearchError::Filter(message) => {
                ResponseError::new(ErrorCode::INVALID_PARAMS, message)
            }
            WorkspaceSearchError::Query(error) => workspace_query_response_error(error),
        })?;
    let complete = index_complete && results.is_complete() && results.value.complete;
    let items = results
        .value
        .items
        .into_iter()
        .map(|record| search_item(workspace, record))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SearchResult {
        schema_version: 3,
        items,
        complete,
    })
}

fn search_item(workspace: &Workspace, record: SearchRecord) -> Result<SearchItem, ResponseError> {
    let disk_source;
    let source = if let Some(entry) = workspace.get(&record.path) {
        entry.parsed.source()
    } else {
        disk_source = fs::read_to_string(&record.path).map_err(|_| {
            ResponseError::new(ErrorCode::INTERNAL_ERROR, "search record lost its document")
        })?;
        &disk_source
    };
    let location = Location::new(
        Url::from_file_path(&record.path).map_err(|_| {
            ResponseError::new(
                ErrorCode::INTERNAL_ERROR,
                format!(
                    "search path is not an absolute file path: {}",
                    record.path.display()
                ),
            )
        })?,
        byte_range_to_lsp(source, &record.range),
    );
    Ok(SearchItem {
        kind: match record.kind {
            SearchRecordKind::Note => SearchKind::Note,
            SearchRecordKind::Task => SearchKind::Task,
            SearchRecordKind::Event => SearchKind::Event,
        },
        title: record.title,
        path: record.relative_path,
        location,
        provenance: SearchProvenance {
            source: "current".to_string(),
            revision: record.revision,
        },
        id: record.id,
        state: record.task_state.map(|state| state.as_str().to_string()),
        wait_reasons: record.wait_reasons.map(|reasons| {
            reasons
                .into_iter()
                .map(|reason| reason.as_str().to_string())
                .collect()
        }),
        due: record.due,
        blocked: record.blocked,
        actionable: record.actionable,
        at: record.at,
        start: record.start,
        end: record.end,
        tasks: record.tasks,
    })
}

fn apply_content_changes(
    mut text: String,
    mut line_index: LineIndex,
    changes: Vec<TextDocumentContentChangeEvent>,
) -> Result<(String, LineIndex, Option<SourceChange>), String> {
    let single_change = changes.len() == 1;
    let mut source_change = None;
    for change in changes {
        let Some(range) = change.range else {
            if single_change {
                source_change = Some(SourceChange {
                    old_range: 0..text.len(),
                    new_range: 0..change.text.len(),
                });
            }
            text = change.text;
            line_index = LineIndex::new(&text);
            continue;
        };
        let start = line_index
            .position_to_offset(&text, range.start)
            .ok_or_else(|| format!("invalid UTF-16 range start {:?}", range.start))?;
        let end = line_index
            .position_to_offset(&text, range.end)
            .ok_or_else(|| format!("invalid UTF-16 range end {:?}", range.end))?;
        if start > end {
            return Err(format!("range start {start} follows end {end}"));
        }
        if let Some(expected) = change.range_length {
            let actual = text[start..end].encode_utf16().count() as u32;
            if actual != expected {
                return Err(format!(
                    "rangeLength {expected} does not match replaced UTF-16 length {actual}"
                ));
            }
        }
        if single_change {
            source_change = Some(SourceChange {
                old_range: start..end,
                new_range: start..start + change.text.len(),
            });
        }
        line_index.apply_edit(start..end, &change.text);
        text.replace_range(start..end, &change.text);
    }
    Ok((text, line_index, source_change))
}

impl LanguageServer for ServerState {
    type Error = ResponseError;
    type NotifyResult = ControlFlow<async_lsp::Result<()>>;

    fn initialize(
        &mut self,
        params: InitializeParams,
    ) -> BoxFuture<'static, Result<InitializeResult, Self::Error>> {
        self.roots = workspace_roots(&params);
        self.supports_document_changes = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.workspace_edit.as_ref())
            .and_then(|edit| edit.document_changes)
            .unwrap_or(false);
        self.supports_resource_rename = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.workspace_edit.as_ref())
            .and_then(|edit| edit.resource_operations.as_ref())
            .is_some_and(|operations| operations.contains(&ResourceOperationKind::Rename));
        self.supports_dynamic_watching = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.did_change_watched_files.as_ref())
            .and_then(|watching| watching.dynamic_registration)
            .unwrap_or(false);
        self.supports_completion_snippets = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.completion.as_ref())
            .and_then(|completion| completion.completion_item.as_ref())
            .and_then(|item| item.snippet_support)
            .unwrap_or(false);
        self.completion_indentation = completion_indentation(
            params
                .capabilities
                .text_document
                .as_ref()
                .and_then(|text_document| text_document.completion.as_ref()),
        );
        self.supports_code_lens_refresh = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.code_lens.as_ref())
            .and_then(|code_lens| code_lens.refresh_support)
            .unwrap_or(false);
        self.supports_folding_range_refresh = params
            .capabilities
            .experimental
            .as_ref()
            .and_then(|experimental| experimental.pointer("/plumb/foldingRangeRefreshSupport"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        self.folding_range_limit = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.folding_range.as_ref())
            .and_then(|folding| folding.range_limit)
            .map(|limit| limit as usize);
        self.supports_folding_collapsed_text = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.folding_range.as_ref())
            .and_then(|folding| folding.folding_range.as_ref())
            .and_then(|folding| folding.collapsed_text)
            .unwrap_or(false);
        self.line_folding_only = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.folding_range.as_ref())
            .and_then(|folding| folding.line_folding_only)
            .unwrap_or(false);
        Box::pin(async {
            Ok(InitializeResult {
                capabilities: ServerCapabilities {
                    text_document_sync: Some(TextDocumentSyncCapability::Kind(
                        TextDocumentSyncKind::INCREMENTAL,
                    )),
                    document_symbol_provider: Some(OneOf::Left(true)),
                    folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
                    document_formatting_provider: Some(OneOf::Left(true)),
                    document_range_formatting_provider: Some(OneOf::Left(true)),
                    code_lens_provider: Some(CodeLensOptions {
                        resolve_provider: Some(false),
                    }),
                    code_action_provider: Some(CodeActionProviderCapability::Options(
                        CodeActionOptions {
                            code_action_kinds: Some(vec![
                                CodeActionKind::QUICKFIX,
                                CodeActionKind::REFACTOR_REWRITE,
                            ]),
                            resolve_provider: Some(false),
                            work_done_progress_options: WorkDoneProgressOptions::default(),
                        },
                    )),
                    semantic_tokens_provider: Some(
                        SemanticTokensServerCapabilities::SemanticTokensOptions(
                            SemanticTokensOptions {
                                work_done_progress_options: WorkDoneProgressOptions::default(),
                                legend: SemanticTokensLegend {
                                    token_types: vec![SemanticTokenType::new("task")],
                                    token_modifiers: vec![
                                        SemanticTokenModifier::new("completed"),
                                        SemanticTokenModifier::new("canceled"),
                                    ],
                                },
                                range: Some(false),
                                full: Some(SemanticTokensFullOptions::Bool(true)),
                            },
                        ),
                    ),
                    definition_provider: Some(OneOf::Left(true)),
                    references_provider: Some(OneOf::Left(true)),
                    hover_provider: Some(HoverProviderCapability::Simple(true)),
                    completion_provider: Some(CompletionOptions {
                        resolve_provider: Some(false),
                        trigger_characters: Some(vec![
                            "`".to_string(),
                            "-".to_string(),
                            ">".to_string(),
                            "\"".to_string(),
                            "/".to_string(),
                            "#".to_string(),
                            "{".to_string(),
                            " ".to_string(),
                            ".".to_string(),
                            "=".to_string(),
                        ]),
                        ..CompletionOptions::default()
                    }),
                    rename_provider: Some(OneOf::Right(RenameOptions {
                        prepare_provider: Some(true),
                        work_done_progress_options: WorkDoneProgressOptions::default(),
                    })),
                    workspace_symbol_provider: Some(OneOf::Left(true)),
                    experimental: Some(serde_json::json!({
                        "plumb": {
                            "search": {
                                "schemaVersion": 3,
                                "method": "plumb/search"
                            },
                            "foldingRangeRefresh": {
                                "method": "workspace/foldingRange/refresh",
                                "clientCapability": "experimental.plumb.foldingRangeRefreshSupport"
                            }
                        }
                    })),
                    ..ServerCapabilities::default()
                },
                server_info: None,
            })
        })
    }

    fn initialized(&mut self, _params: InitializedParams) -> Self::NotifyResult {
        self.start_initial_index();
        ControlFlow::Continue(())
    }

    fn did_open(&mut self, params: DidOpenTextDocumentParams) -> Self::NotifyResult {
        let document = params.text_document;
        let line_index = LineIndex::new(&document.text);
        let path = document
            .uri
            .to_file_path()
            .ok()
            .map(|path| normalize(&path));
        self.update(document.uri, document.version, document.text, None, false);
        if let Some(path) = path {
            self.open_document_line_indexes.insert(path, line_index);
        }
        ControlFlow::Continue(())
    }

    fn did_change(&mut self, params: DidChangeTextDocumentParams) -> Self::NotifyResult {
        if params.content_changes.is_empty() {
            return ControlFlow::Continue(());
        }
        let document = params.text_document;
        let Some(path) = self.open_documents.get(&document.uri).cloned() else {
            tracing::warn!(uri = %document.uri, "ignored didChange for a document that is not open");
            return ControlFlow::Continue(());
        };
        let Some(entry) = self.workspace.get(&path) else {
            tracing::warn!(uri = %document.uri, "ignored didChange without a current document snapshot");
            return ControlFlow::Continue(());
        };
        let text = entry.parsed.source().to_string();
        let line_index = self
            .open_document_line_indexes
            .get(&path)
            .cloned()
            .unwrap_or_else(|| LineIndex::new(&text));
        let (text, line_index, source_change) = match apply_content_changes(
            text,
            line_index,
            params.content_changes,
        ) {
            Ok(updated) => updated,
            Err(error) => {
                tracing::warn!(uri = %document.uri, version = document.version, %error, "ignored invalid didChange");
                return ControlFlow::Continue(());
            }
        };
        self.open_document_line_indexes.insert(path, line_index);
        self.update(document.uri, document.version, text, source_change, true);
        ControlFlow::Continue(())
    }

    fn did_close(&mut self, params: DidCloseTextDocumentParams) -> Self::NotifyResult {
        let uri = params.text_document.uri;
        if let Some(path) = self.open_documents.remove(&uri) {
            self.open_document_line_indexes.remove(&path);
            self.document_analysis_tokens.cancel(&path);
            self.pending_document_reads.remove(&path);
            let (files, complete) = self.scanned_files();
            self.index_complete &= complete;
            if files.binary_search(&path).is_ok() {
                if let Ok(text) = fs::read_to_string(&path) {
                    let _ = self.workspace.insert_disk(&path, 0, text);
                    self.workspace.close_document(&path);
                } else {
                    let _ = self.workspace.remove_disk(&path);
                    self.workspace.close_document(&path);
                }
            } else {
                let _ = self.workspace.remove_disk(&path);
                self.workspace.close_document(&path);
            }
        }
        let _ = self
            .client
            .notify::<lsp_types::notification::PublishDiagnostics>(PublishDiagnosticsParams {
                uri,
                diagnostics: Vec::new(),
                version: None,
            });
        self.publish_all_open_diagnostics();
        self.refresh_code_lenses();
        self.refresh_folding_ranges();
        ControlFlow::Continue(())
    }

    fn did_save(&mut self, params: DidSaveTextDocumentParams) -> Self::NotifyResult {
        if let Ok(path) = params.text_document.uri.to_file_path() {
            if let Ok(text) = fs::read_to_string(&path) {
                let _ = self.workspace.insert_disk(path, 0, text);
            }
        }
        ControlFlow::Continue(())
    }

    fn did_change_configuration(
        &mut self,
        _params: DidChangeConfigurationParams,
    ) -> Self::NotifyResult {
        ControlFlow::Continue(())
    }

    fn did_change_watched_files(
        &mut self,
        params: DidChangeWatchedFilesParams,
    ) -> Self::NotifyResult {
        let changes = params
            .changes
            .into_iter()
            .filter_map(|change| {
                let path = normalize(&change.uri.to_file_path().ok()?);
                Some((path, change.typ))
            })
            .collect::<Vec<_>>();
        if changes
            .iter()
            .any(|(path, _)| path.file_name().is_some_and(|name| name == ".ignore"))
        {
            self.index_generation = self.index_generation.wrapping_add(1);
            let (_, complete) = self.index_roots();
            self.index_complete = complete;
        } else {
            let (files, complete) = self.scanned_files();
            self.index_complete &= complete;
            let indexed = files.into_iter().collect::<HashSet<_>>();
            for (path, change_type) in changes {
                if !is_plumb_file(&path) {
                    continue;
                }
                self.confirm_pending_path_rename(&path);
                if self.open_documents.values().any(|open| open == &path) {
                    continue;
                }
                match change_type {
                    FileChangeType::CREATED | FileChangeType::CHANGED
                        if indexed.contains(&path) =>
                    {
                        if let Ok(text) = fs::read_to_string(&path) {
                            let _ = self.workspace.insert_disk(path, 0, text);
                        }
                    }
                    FileChangeType::CREATED | FileChangeType::CHANGED | FileChangeType::DELETED => {
                        let _ = self.workspace.remove_disk(path);
                    }
                    _ => {}
                }
            }
        }
        self.publish_all_open_diagnostics();
        self.refresh_code_lenses();
        self.refresh_folding_ranges();
        ControlFlow::Continue(())
    }

    fn document_symbol(
        &mut self,
        params: DocumentSymbolParams,
    ) -> BoxFuture<'static, Result<Option<DocumentSymbolResponse>, Self::Error>> {
        let symbols = params
            .text_document
            .uri
            .to_file_path()
            .ok()
            .and_then(|path| self.workspace.get(path))
            .and_then(|entry| entry.current.as_ref().map(|current| (entry, current)))
            .map(|(entry, current)| {
                let positions = PositionIndex::new(entry.parsed.source());
                let mut symbols = current
                    .output
                    .headings()
                    .headings
                    .iter()
                    .map(|heading| heading_symbol(&positions, heading))
                    .collect::<Vec<_>>();
                let mut additional = current
                    .output
                    .anchors()
                    .iter()
                    .filter(|anchor| {
                        anchor.kind != AnchorKind::Heading
                            && !current
                                .output
                                .tasks()
                                .tasks
                                .view_at_start(anchor.range.start)
                                .is_some_and(|task| task.range() == anchor.range)
                            && !current
                                .output
                                .events()
                                .events
                                .view_at_start(anchor.range.start)
                                .is_some_and(|event| event.range() == anchor.range)
                    })
                    .map(|anchor| (anchor.range.start, anchor_symbol(&positions, &anchor)))
                    .collect::<Vec<_>>();
                if let Some(metadata) = &current.output.metadata().metadata {
                    additional.push((metadata.range.start, metadata_symbol(&positions, metadata)));
                }
                additional.extend(
                    current
                        .output
                        .tasks()
                        .tasks
                        .views()
                        .filter(|task| task.depth() == 0)
                        .map(|task| task.range().start)
                        .zip(task_symbols(&positions, &current.output.tasks().tasks)),
                );
                additional.extend(
                    current
                        .output
                        .events()
                        .events
                        .views()
                        .filter(|event| event.depth() == 0)
                        .map(|event| event.range().start)
                        .zip(event_symbols(&positions, &current.output.events().events)),
                );
                additional.sort_by_key(|(start, _)| *start);
                insert_document_symbols(
                    &mut symbols,
                    additional.into_iter().map(|(_, symbol)| symbol).collect(),
                );
                symbols
            });
        Box::pin(async move { Ok(symbols.map(DocumentSymbolResponse::Nested)) })
    }

    fn folding_range(
        &mut self,
        params: FoldingRangeParams,
    ) -> BoxFuture<'static, Result<Option<Vec<FoldingRange>>, Self::Error>> {
        if let Ok(path) = params.text_document.uri.to_file_path() {
            if self.supports_folding_collapsed_text {
                if let Some(pending) = self.await_document_semantics(&path, true) {
                    let limit = self.folding_range_limit;
                    let line_only = self.line_folding_only;
                    return Box::pin(async move {
                        let snapshot = pending.await?;
                        Ok(Some(green_folding_ranges(
                            snapshot.entry.parsed.source(),
                            snapshot.entry.parsed.green(),
                            limit,
                            snapshot.labels.as_ref(),
                            line_only,
                        )))
                    });
                }
            }
        }
        let result = (|| {
            let Some(path) = params.text_document.uri.to_file_path().ok() else {
                return Ok(None);
            };
            let Some(entry) = self.workspace.get(path) else {
                return Ok(None);
            };
            let labels = if self.supports_folding_collapsed_text {
                Some(fold_labels(
                    &self.workspace,
                    &entry.path,
                    entry,
                    self.index_complete,
                ))
            } else {
                None
            };
            let ranges = green_folding_ranges(
                entry.parsed.source(),
                entry.parsed.green(),
                self.folding_range_limit,
                labels.as_ref(),
                self.line_folding_only,
            );
            Ok(Some(ranges))
        })();
        Box::pin(async move { result })
    }

    fn symbol(
        &mut self,
        params: WorkspaceSymbolParams,
    ) -> BoxFuture<'static, Result<Option<WorkspaceSymbolResponse>, Self::Error>> {
        let (kind, query) = workspace_symbol_query(&params.query);
        let search = self.search(SearchParams {
            kind,
            query,
            filter: None,
            limit: Some(100),
        });
        Box::pin(async move {
            let result = search.await?;
            let symbols = result
                .items
                .into_iter()
                .map(|item| {
                    let detail = match item.kind {
                        SearchKind::Note => item.path,
                        SearchKind::Task => {
                            let mut parts = vec![item.path];
                            if let Some(state) = item.state {
                                parts.push(state);
                            }
                            if let Some(due) = item.due {
                                parts.push(format!("due {due}"));
                            }
                            parts.join(" · ")
                        }
                        SearchKind::Event => {
                            let mut parts = vec![item.path];
                            if let Some(start) = item.start {
                                parts.push(start);
                            }
                            parts.join(" · ")
                        }
                    };
                    #[allow(deprecated)]
                    SymbolInformation {
                        name: item.title,
                        kind: match item.kind {
                            SearchKind::Note => SymbolKind::FILE,
                            SearchKind::Task => SymbolKind::EVENT,
                            SearchKind::Event => SymbolKind::EVENT,
                        },
                        tags: None,
                        deprecated: None,
                        location: item.location,
                        container_name: Some(detail),
                    }
                })
                .collect();
            Ok(Some(WorkspaceSymbolResponse::Flat(symbols)))
        })
    }

    fn formatting(
        &mut self,
        params: DocumentFormattingParams,
    ) -> BoxFuture<'static, Result<Option<Vec<LspTextEdit>>, Self::Error>> {
        let edits = params
            .text_document
            .uri
            .to_file_path()
            .ok()
            .and_then(|path| self.workspace.get(path))
            .and_then(|entry| {
                let source = entry.parsed.source();
                let edits = plumb_edit::format_green(entry.parsed.green()).ok()?;
                Some(text_edits_to_lsp(source, edits).collect())
            });
        Box::pin(async move { Ok(edits) })
    }

    fn range_formatting(
        &mut self,
        params: DocumentRangeFormattingParams,
    ) -> BoxFuture<'static, Result<Option<Vec<LspTextEdit>>, Self::Error>> {
        let edits = params
            .text_document
            .uri
            .to_file_path()
            .ok()
            .and_then(|path| self.workspace.get(path))
            .and_then(|entry| {
                let source = entry.parsed.source();
                let selection = position_to_offset(source, params.range.start)
                    ..position_to_offset(source, params.range.end);
                let edits =
                    plumb_edit::format_green_contained(entry.parsed.green(), selection).ok()?;
                Some(text_edits_to_lsp(source, edits).collect())
            });
        Box::pin(async move { Ok(edits) })
    }

    fn definition(
        &mut self,
        params: GotoDefinitionParams,
    ) -> BoxFuture<'static, Result<Option<GotoDefinitionResponse>, Self::Error>> {
        let position = params.text_document_position_params;
        if let Ok(path) = position.text_document.uri.to_file_path() {
            self.ensure_request_document(&path);
            self.complete_pending_navigation_documents([normalize(&path)]);
        }
        let result = (|| {
            let Some(path) = position.text_document.uri.to_file_path().ok() else {
                return Ok(None);
            };
            let Some(entry) = self.workspace.get(&path) else {
                return Ok(None);
            };
            let offset = position_to_offset(entry.parsed.source(), position.position);
            if let Some(citation) = entry.current.as_ref().and_then(|current| {
                current
                    .output
                    .citations()
                    .citations
                    .iter()
                    .find(|citation| {
                        citation.selection_range.start <= offset
                            && offset <= citation.selection_range.end
                    })
            }) {
                let Some(bibliography) = self.bibliography_for(&path) else {
                    return Ok(None);
                };
                let BibliographyResolution::Resolved(record) = bibliography.resolve(&citation.id)
                else {
                    return Ok(None);
                };
                let Ok(source) = std::fs::read_to_string(&record.path) else {
                    return Ok(None);
                };
                let Ok(uri) = Url::from_file_path(&record.path) else {
                    return Ok(None);
                };
                return Ok(Some(Location::new(
                    uri,
                    byte_range_to_lsp(&source, &record.range),
                )));
            }
            Ok(
                match self
                    .target_at_with_lazy_load(&path, offset)
                    .map_err(workspace_query_response_error)?
                {
                    Some(ResolvedTarget::Anchor { path, anchor, .. }) => {
                        location_for(&self.workspace, &path, &anchor.selection_range)
                    }
                    Some(ResolvedTarget::Document { path }) => {
                        location_for(&self.workspace, &path, &(0..0))
                    }
                    Some(ResolvedTarget::File { path }) => Url::from_file_path(path)
                        .ok()
                        .map(|uri| Location::new(uri, lsp_types::Range::default())),
                    _ => None,
                },
            )
        })();
        Box::pin(async move { result.map(|location| location.map(GotoDefinitionResponse::Scalar)) })
    }

    fn references(
        &mut self,
        params: ReferenceParams,
    ) -> BoxFuture<'static, Result<Option<Vec<Location>>, Self::Error>> {
        let position = params.text_document_position;
        if let Ok(path) = position.text_document.uri.to_file_path() {
            self.ensure_request_document(&path);
        }
        self.complete_pending_navigation_documents(
            self.open_documents.values().cloned().collect::<Vec<_>>(),
        );
        let result = (|| {
            let Some(path) = position.text_document.uri.to_file_path().ok() else {
                return Ok(None);
            };
            let Some(entry) = self.workspace.get(&path) else {
                return Ok(None);
            };
            let offset = position_to_offset(entry.parsed.source(), position.position);
            match self
                .target_at_with_lazy_load(&path, offset)
                .map_err(workspace_query_response_error)?
            {
                Some(ResolvedTarget::Anchor {
                    path: target_path,
                    id,
                    anchor,
                }) => {
                    let mut location_cache = ReferenceLocationCache::new(&self.workspace);
                    let mut locations = self
                        .complete_query(self.workspace.references_to(&target_path, &id))
                        .map_err(workspace_query_response_error)?
                        .into_iter()
                        .filter_map(|(source_path, reference)| {
                            location_cache.location(&source_path, &reference.source_range)
                        })
                        .collect::<Vec<_>>();
                    if params.context.include_declaration {
                        if let Some(declaration) =
                            location_cache.location(&target_path, &anchor.selection_range)
                        {
                            locations.insert(0, declaration);
                        }
                    }
                    Ok(Some(locations))
                }
                Some(ResolvedTarget::Document { path: target_path }) => {
                    let mut location_cache = ReferenceLocationCache::new(&self.workspace);
                    let mut locations = self
                        .complete_query(self.workspace.references_to_document(&target_path))
                        .map_err(workspace_query_response_error)?
                        .into_iter()
                        .filter_map(|(source_path, reference)| {
                            location_cache.location(&source_path, &reference.source_range)
                        })
                        .collect::<Vec<_>>();
                    let declaration = (params.context.include_declaration
                        && self.workspace.get(&target_path).is_some())
                    .then(|| location_cache.location(&target_path, &(0..0)))
                    .flatten();
                    if let Some(declaration) = declaration {
                        locations.insert(0, declaration);
                    }
                    Ok(Some(locations))
                }
                _ => Ok(None),
            }
        })();
        Box::pin(async move { result })
    }

    fn code_lens(
        &mut self,
        params: CodeLensParams,
    ) -> BoxFuture<'static, Result<Option<Vec<CodeLens>>, Self::Error>> {
        if let Ok(path) = params.text_document.uri.to_file_path() {
            self.ensure_request_document(&path);
        }
        let mut pending_semantics = false;
        let result = (|| {
            let Some(path) = params.text_document.uri.to_file_path().ok() else {
                return Ok(None);
            };
            let Some(entry) = self.workspace.get(&path) else {
                return Ok(None);
            };
            let Some(output) = entry.current.as_ref() else {
                pending_semantics = entry.parsed.is_valid();
                return Ok(None);
            };
            let Ok(uri) = Url::from_file_path(&entry.path) else {
                return Ok(None);
            };
            if optional_decorative_query(self.require_index_complete())
                .map_err(workspace_query_response_error)?
                .is_none()
            {
                return Ok(None);
            }
            let anchor_ids = output
                .output
                .anchors()
                .iter()
                .map(|anchor| anchor.id.value.clone())
                .collect::<HashSet<_>>();
            let Some(mut references) = optional_decorative_query(
                self.complete_query(
                    self.workspace
                        .reverse_references_for_document(&entry.path, &anchor_ids),
                ),
            )
            .map_err(workspace_query_response_error)?
            else {
                pending_semantics = true;
                return Ok(None);
            };
            let mut lenses = Vec::new();
            let mut location_cache = ReferenceLocationCache::new(&self.workspace);
            let locations = references
                .document
                .into_iter()
                .filter_map(|reference| {
                    location_cache.location(&reference.source_path, &reference.source_range)
                })
                .collect::<Vec<_>>();
            let count = locations.len();
            let title = if count == 1 {
                "1 file reference".to_string()
            } else {
                format!("{count} file references")
            };
            lenses.push(reference_code_lens(
                &uri,
                lsp_types::Range::default(),
                title,
                locations,
            ));
            lenses.extend(output.output.anchors().iter().filter_map(|anchor| {
                let locations = references
                    .anchors
                    .remove(&anchor.id.value)
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|reference| {
                        location_cache.location(&reference.source_path, &reference.source_range)
                    })
                    .collect::<Vec<_>>();
                let count = locations.len();
                let title = if count == 1 {
                    "1 reference".to_string()
                } else {
                    format!("{count} references")
                };
                let lens_range = if anchor.kind == AnchorKind::Inline {
                    anchor.id.range.clone()
                } else {
                    anchor.range.start..anchor.range.start
                };
                let range = location_cache.location(&entry.path, &lens_range)?.range;
                Some(reference_code_lens(&uri, range, title, locations))
            }));
            Ok(Some(lenses))
        })();
        if pending_semantics && self.supports_code_lens_refresh {
            self.code_lens_refresh_pending = true;
        }
        Box::pin(async move { result })
    }

    fn hover(
        &mut self,
        params: HoverParams,
    ) -> BoxFuture<'static, Result<Option<Hover>, Self::Error>> {
        let position = params.text_document_position_params;
        if let Ok(path) = position.text_document.uri.to_file_path() {
            self.ensure_request_document(&path);
            self.complete_pending_navigation_documents([normalize(&path)]);
        }
        let result = (|| {
            let Some(path) = position.text_document.uri.to_file_path().ok() else {
                return Ok(None);
            };
            let offset = {
                let Some(entry) = self.workspace.get(&path) else {
                    return Ok(None);
                };
                position_to_offset(entry.parsed.source(), position.position)
            };
            if let Some(citation) = self
                .workspace
                .get(&path)
                .and_then(|entry| entry.current.as_ref())
                .and_then(|current| {
                    current
                        .output
                        .citations()
                        .citations
                        .iter()
                        .find(|citation| {
                            citation.selection_range.start <= offset
                                && offset <= citation.selection_range.end
                        })
                })
            {
                let Some(bibliography) = self.bibliography_for(&path) else {
                    return Ok(None);
                };
                let BibliographyResolution::Resolved(record) = bibliography.resolve(&citation.id)
                else {
                    return Ok(None);
                };
                let Some(entry) = self.workspace.get(&path) else {
                    return Ok(None);
                };
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: format!("**Citation:** `{}`\n\n{}", record.id, record.detail()),
                    }),
                    range: Some(byte_range_to_lsp(
                        entry.parsed.source(),
                        &citation.selection_range,
                    )),
                }));
            }
            if let Some(file) = self.workspace.file_at(&path, offset) {
                let target = self.workspace.resolve_file(&path, &file);
                let Some(entry) = self.workspace.get(&path) else {
                    return Ok(None);
                };
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: file_hover(&target, &file),
                    }),
                    range: Some(byte_range_to_lsp(
                        entry.parsed.source(),
                        &file.selection_range,
                    )),
                }));
            }
            if let Some(image) = self.workspace.image_at(&path, offset) {
                let target = self.workspace.resolve_image(&path, &image);
                let Some(entry) = self.workspace.get(&path) else {
                    return Ok(None);
                };
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: image_hover(&target, &image),
                    }),
                    range: Some(byte_range_to_lsp(
                        entry.parsed.source(),
                        &image.selection_range,
                    )),
                }));
            }
            if let Some(link) = self.workspace.link_at(&path, offset) {
                let target = self
                    .complete_query(self.workspace.resolve_link(&path, &link))
                    .map_err(workspace_query_response_error)?;
                if matches!(target, ResolvedTarget::External | ResolvedTarget::Other) {
                    let Some(entry) = self.workspace.get(&path) else {
                        return Ok(None);
                    };
                    return Ok(Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: link_hover(&target, &link),
                        }),
                        range: Some(byte_range_to_lsp(
                            entry.parsed.source(),
                            &link.selection_range,
                        )),
                    }));
                }
            }
            if let Some(target) = self
                .reference_target_at_with_lazy_load(&path, offset)
                .map_err(workspace_query_response_error)?
            {
                let message = target_hover(&self.workspace, &target);
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: message,
                    }),
                    range: None,
                }));
            }
            if let Some(task) = self.workspace.task_at(&path, offset) {
                self.require_index_complete()
                    .map_err(workspace_query_response_error)?;
                let value = task_hover(&self.workspace, &path, &task)
                    .map_err(workspace_query_response_error)?;
                let Some(entry) = self.workspace.get(&path) else {
                    return Ok(None);
                };
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value,
                    }),
                    range: Some(byte_range_to_lsp(
                        entry.parsed.source(),
                        &task.selection_range,
                    )),
                }));
            }
            if let Some(event) = self.workspace.event_at(&path, offset) {
                let Some(entry) = self.workspace.get(&path) else {
                    return Ok(None);
                };
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: event_hover(&event),
                    }),
                    range: Some(byte_range_to_lsp(
                        entry.parsed.source(),
                        &event.selection_range,
                    )),
                }));
            }
            if let Some(target) = self.workspace.document_metadata_target_at(&path, offset) {
                let Some(metadata) = self.workspace.document_metadata(&path) else {
                    return Ok(None);
                };
                let Some(entry) = self.workspace.get(&path) else {
                    return Ok(None);
                };
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: metadata_hover(&path, metadata),
                    }),
                    range: Some(byte_range_to_lsp(entry.parsed.source(), &target.range)),
                }));
            }
            let Some(target) = self
                .target_at_with_lazy_load(&path, offset)
                .map_err(workspace_query_response_error)?
            else {
                return Ok(None);
            };
            let message = target_hover(&self.workspace, &target);
            Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: message,
                }),
                range: None,
            }))
        })();
        Box::pin(async move { result })
    }

    fn completion(
        &mut self,
        params: CompletionParams,
    ) -> BoxFuture<'static, Result<Option<CompletionResponse>, Self::Error>> {
        let position = params.text_document_position;
        let result = (|| {
            let Some(path) = position.text_document.uri.to_file_path().ok() else {
                return Ok(None);
            };
            let Some(entry) = self.workspace.get(&path) else {
                return Ok(None);
            };
            let source = entry.parsed.source();
            let green = entry.parsed.green();
            let offset = position_to_offset(source, position.position);
            if let Some(context) = construct_completion_context(green, offset) {
                let timestamp = Local::now().to_rfc3339_opts(SecondsFormat::Secs, false);
                let include_link_labels =
                    matches!(context, ConstructCompletionContext::ParsedLink { .. });
                let mut items = construct_completion_items(
                    source,
                    context,
                    self.supports_completion_snippets,
                    self.completion_indentation,
                    &timestamp,
                );
                let link_context = (include_link_labels && self.index_complete)
                    .then(|| link_completion_context(green, offset))
                    .flatten();
                if let Some(context) = link_context {
                    let candidates = self
                        .complete_query(self.workspace.complete_link(&path, &context))
                        .map_err(workspace_query_response_error)?;
                    items.extend(completion_items(
                        source,
                        candidates,
                        CompletionItemKind::FILE,
                    ));
                }
                return Ok(Some(items));
            }
            if let Some(context) = citation_completion_context(green, offset) {
                let Some(bibliography) = self.bibliography_for_completion(&path) else {
                    return Ok(None);
                };
                let query = context.query.to_lowercase();
                let mut replace = None;
                return Ok(Some(
                    bibliography
                        .records
                        .iter()
                        .filter(|record| {
                            matches!(
                                bibliography.resolve(&record.id),
                                BibliographyResolution::Resolved(_)
                            )
                        })
                        .filter(|record| {
                            query.is_empty()
                                || record.id.to_lowercase().contains(&query)
                                || record.detail().to_lowercase().contains(&query)
                        })
                        .map(|record| CompletionItem {
                            label: record.id.clone(),
                            kind: Some(CompletionItemKind::REFERENCE),
                            detail: Some(record.detail()),
                            text_edit: Some(CompletionTextEdit::Edit(LspTextEdit::new(
                                *replace.get_or_insert_with(|| {
                                    byte_range_to_lsp(source, &context.replace)
                                }),
                                record.id.clone(),
                            ))),
                            ..CompletionItem::default()
                        })
                        .collect(),
                ));
            }
            if let Some(context) = task_dependency_completion_context(green, offset) {
                let candidates = self
                    .complete_query(self.workspace.complete_task_dependency(&path, &context))
                    .map_err(workspace_query_response_error)?;
                return Ok(Some(completion_items_with_kind(
                    source,
                    candidates,
                    |candidate| {
                        if candidate.new_text.ends_with('#') {
                            CompletionItemKind::FILE
                        } else {
                            CompletionItemKind::REFERENCE
                        }
                    },
                )));
            }
            if let Some(context) = event_title_completion_context(green, offset) {
                let candidates = self
                    .complete_query(self.workspace.complete_event_title(&context))
                    .map_err(workspace_query_response_error)?;
                return Ok(Some(completion_items(
                    source,
                    candidates,
                    CompletionItemKind::VALUE,
                )));
            }
            if let Some(context) = attribute_completion_context(green, offset) {
                if !context.completions.is_empty() {
                    let replace = byte_range_to_lsp(source, &context.replace);
                    return Ok(Some(
                        context
                            .completions
                            .into_iter()
                            .map(|candidate| CompletionItem {
                                label: candidate.label.to_string(),
                                kind: Some(CompletionItemKind::PROPERTY),
                                detail: Some(candidate.detail.to_string()),
                                insert_text_format: Some(if self.supports_completion_snippets {
                                    InsertTextFormat::SNIPPET
                                } else {
                                    InsertTextFormat::PLAIN_TEXT
                                }),
                                text_edit: Some(CompletionTextEdit::Edit(LspTextEdit::new(
                                    replace,
                                    attribute_completion_text(
                                        &candidate.new_text,
                                        self.supports_completion_snippets,
                                    ),
                                ))),
                                ..CompletionItem::default()
                            })
                            .collect(),
                    ));
                }
            }
            let (candidates, kind) = if let Some(context) = link_completion_context(green, offset) {
                let kind = if matches!(
                    &context,
                    plumb_semantics::LinkCompletionContext::Path { .. }
                        | plumb_semantics::LinkCompletionContext::SingleArgumentPath { .. }
                        | plumb_semantics::LinkCompletionContext::VerbatimPath { .. }
                ) {
                    CompletionItemKind::FILE
                } else {
                    CompletionItemKind::REFERENCE
                };
                (
                    self.complete_query(self.workspace.complete_link(&path, &context))
                        .map_err(workspace_query_response_error)?,
                    kind,
                )
            } else if let Some(context) = image_completion_context(green, offset) {
                (
                    self.workspace.complete_image_path(&path, &context),
                    CompletionItemKind::FILE,
                )
            } else {
                let Some(context) = file_completion_context(green, offset) else {
                    return Ok(None);
                };
                (
                    self.workspace.complete_file_path(&path, &context),
                    CompletionItemKind::FILE,
                )
            };
            Ok(Some(completion_items(source, candidates, kind)))
        })();
        Box::pin(async move { result.map(|items| items.map(CompletionResponse::Array)) })
    }

    fn code_action(
        &mut self,
        params: CodeActionParams,
    ) -> BoxFuture<'static, Result<Option<CodeActionResponse>, Self::Error>> {
        if !self.supports_document_changes {
            return Box::pin(async { Ok(None) });
        }
        let Some(path) = params.text_document.uri.to_file_path().ok() else {
            return Box::pin(async { Ok(None) });
        };
        let now = Local::now();
        let timestamp = now.to_rfc3339_opts(SecondsFormat::Secs, false);
        let mut actions = Vec::new();
        if code_action_kind_requested(
            params.context.only.as_deref(),
            &CodeActionKind::REFACTOR_REWRITE,
        ) {
            if let Some(entry) = self.workspace.get(&path) {
                let offset = position_to_offset(entry.parsed.source(), params.range.start);
                let selection_end = position_to_offset(entry.parsed.source(), params.range.end);
                if let Some(title) = path.file_stem().and_then(|stem| stem.to_str()) {
                    if let Some(edit) = self
                        .workspace
                        .insert_metadata(&path, offset, title, &timestamp)
                        .ok()
                        .and_then(|edit| workspace_edit_to_lsp(&self.workspace, edit))
                    {
                        actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                            title: "Insert document metadata".to_string(),
                            kind: Some(CodeActionKind::REFACTOR_REWRITE),
                            edit: Some(edit),
                            is_preferred: Some(true),
                            ..CodeAction::default()
                        }));
                    }
                }
                if let Some(edit) = self
                    .workspace
                    .align_block_arguments(&path, offset)
                    .ok()
                    .and_then(|edit| workspace_edit_to_lsp(&self.workspace, edit))
                {
                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: "Align arguments".to_string(),
                        kind: Some(CodeActionKind::REFACTOR_REWRITE),
                        edit: Some(edit),
                        ..CodeAction::default()
                    }));
                }
                if let Some(edit) = self
                    .workspace
                    .add_explicit_id(&path, offset)
                    .ok()
                    .and_then(|edit| workspace_edit_to_lsp(&self.workspace, edit))
                {
                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: "Add explicit id".to_string(),
                        kind: Some(CodeActionKind::REFACTOR_REWRITE),
                        edit: Some(edit),
                        is_preferred: Some(true),
                        ..CodeAction::default()
                    }));
                }
                if let Some(edit) = self
                    .workspace
                    .convert_event_shorthand(&path, offset, now.fixed_offset())
                    .ok()
                    .and_then(|edit| workspace_edit_to_lsp(&self.workspace, edit))
                {
                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: "Convert to event".to_string(),
                        kind: Some(CodeActionKind::REFACTOR_REWRITE),
                        edit: Some(edit),
                        is_preferred: Some(true),
                        ..CodeAction::default()
                    }));
                }
                if selection_end > offset {
                    if let Some(edit) = self
                        .workspace
                        .convert_event_shorthands(&path, offset..selection_end, now.fixed_offset())
                        .ok()
                        .and_then(|edit| workspace_edit_to_lsp(&self.workspace, edit))
                    {
                        actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                            title: "Convert selected items to events".to_string(),
                            kind: Some(CodeActionKind::REFACTOR_REWRITE),
                            edit: Some(edit),
                            is_preferred: Some(true),
                            ..CodeAction::default()
                        }));
                    }
                }
                for (title, edit) in [
                    (
                        "Convert to task",
                        self.workspace
                            .convert_list_item_to_task(&path, offset, &timestamp),
                    ),
                    (
                        "Add task created timestamp",
                        self.workspace.add_task_created(&path, offset, &timestamp),
                    ),
                ] {
                    if let Some(edit) = edit
                        .ok()
                        .and_then(|edit| workspace_edit_to_lsp(&self.workspace, edit))
                    {
                        actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                            title: title.to_string(),
                            kind: Some(CodeActionKind::REFACTOR_REWRITE),
                            edit: Some(edit),
                            is_preferred: Some(true),
                            ..CodeAction::default()
                        }));
                    }
                }
            }
        }
        if code_action_kind_requested(params.context.only.as_deref(), &CodeActionKind::QUICKFIX) {
            if let Some(entry) = self.workspace.get(&path) {
                let offset = position_to_offset(entry.parsed.source(), params.range.start);
                for (status, title, preferred) in [
                    (TaskStatus::Done, "Complete task", true),
                    (TaskStatus::Canceled, "Cancel task", false),
                ] {
                    if let Some(edit) = self
                        .workspace
                        .set_task_status(&path, offset, status, &timestamp)
                        .ok()
                        .and_then(|edit| workspace_edit_to_lsp(&self.workspace, edit))
                    {
                        actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                            title: title.to_string(),
                            kind: Some(CodeActionKind::QUICKFIX),
                            edit: Some(edit),
                            is_preferred: Some(preferred),
                            ..CodeAction::default()
                        }));
                    }
                }
            }
        }
        Box::pin(async move { Ok((!actions.is_empty()).then_some(actions)) })
    }

    fn semantic_tokens_full(
        &mut self,
        params: SemanticTokensParams,
    ) -> BoxFuture<'static, Result<Option<SemanticTokensResult>, Self::Error>> {
        if let Ok(path) = params.text_document.uri.to_file_path() {
            if let Some(pending) = self.await_document_semantics(&path, false) {
                return Box::pin(async move {
                    let snapshot = pending.await?;
                    Ok(semantic_tokens(&snapshot.entry))
                });
            }
        }
        let tokens = params
            .text_document
            .uri
            .to_file_path()
            .ok()
            .and_then(|path| self.workspace.get(path))
            .and_then(semantic_tokens);
        Box::pin(async move { Ok(tokens) })
    }

    fn prepare_rename(
        &mut self,
        params: lsp_types::TextDocumentPositionParams,
    ) -> BoxFuture<'static, Result<Option<PrepareRenameResponse>, Self::Error>> {
        if let Ok(path) = params.text_document.uri.to_file_path() {
            self.ensure_request_document(&path);
        }
        let result = (|| {
            let Some(path) = params.text_document.uri.to_file_path().ok() else {
                return Ok(None);
            };
            let Some(entry) = self.workspace.get(&path) else {
                return Ok(None);
            };
            let offset = position_to_offset(entry.parsed.source(), params.position);
            let target = match self.workspace.anchor_rename_target_at(&path, offset) {
                Ok(target) => Some((target.range, target.id)),
                Err(WorkspaceOperationError::Operation(RenameError::NotRenameable)) => None,
                Err(error) => return Err(rename_operation_error("anchor rename", error)),
            };
            let (range, placeholder) = if let Some(target) = target {
                target
            } else {
                match self.workspace.document_rename_target_at(&path, offset) {
                    Ok(target) => {
                        let (name, fallback) = match target.input {
                            PathRenameInput::Path => {
                                (target.old_path.file_name(), "document.plumb")
                            }
                            PathRenameInput::FileStem => (target.old_path.file_stem(), "document"),
                        };
                        let placeholder = name
                            .and_then(|name| name.to_str())
                            .unwrap_or(fallback)
                            .to_string();
                        (target.range, placeholder)
                    }
                    Err(WorkspaceOperationError::Operation(RenameError::NotRenameable)) => {
                        return Ok(None);
                    }
                    Err(error) => {
                        return Err(rename_operation_error("document rename", error));
                    }
                }
            };
            Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
                range: byte_range_to_lsp(entry.parsed.source(), &range),
                placeholder,
            }))
        })();
        Box::pin(async move { result })
    }

    fn rename(
        &mut self,
        params: RenameParams,
    ) -> BoxFuture<'static, Result<Option<LspWorkspaceEdit>, Self::Error>> {
        if !self.supports_document_changes {
            return Box::pin(async {
                Err(ResponseError::new(
                    ErrorCode::REQUEST_FAILED,
                    "anchor rename requires workspace.workspaceEdit.documentChanges support",
                ))
            });
        }
        if let Ok(path) = params
            .text_document_position
            .text_document
            .uri
            .to_file_path()
        {
            self.ensure_request_document(&path);
        }
        let result = (|| {
            let Some(path) = params
                .text_document_position
                .text_document
                .uri
                .to_file_path()
                .ok()
            else {
                return Ok(None);
            };
            let Some(entry) = self.workspace.get(&path) else {
                return Ok(None);
            };
            let offset = position_to_offset(
                entry.parsed.source(),
                params.text_document_position.position,
            );
            match self.workspace.anchor_rename_target_at(&path, offset) {
                Ok(target) => {
                    let edit = self
                        .workspace
                        .rename_anchor(&target, &params.new_name)
                        .map_err(|error| rename_operation_error("anchor rename", error))?;
                    let edit = workspace_edit_to_lsp(&self.workspace, edit)
                        .ok_or_else(|| rename_request_error("cannot map anchor rename edit"))?;
                    return Ok(Some(edit));
                }
                Err(WorkspaceOperationError::Operation(RenameError::NotRenameable)) => {}
                Err(error) => return Err(rename_operation_error("anchor rename", error)),
            }
            let target = match self.workspace.document_rename_target_at(&path, offset) {
                Ok(target) => target,
                Err(WorkspaceOperationError::Operation(RenameError::NotRenameable)) => {
                    return Ok(None);
                }
                Err(error) => return Err(rename_operation_error("document rename", error)),
            };
            if !self.supports_resource_rename {
                return Err(rename_request_error(
                    "document rename requires workspace.workspaceEdit.resourceOperations rename support",
                ));
            }
            let edit = self
                .workspace
                .rename_document(&target, &params.new_name)
                .map_err(document_rename_error)?;
            let (old_path, new_path) = edit
                .resource_operations
                .iter()
                .map(|operation| match operation {
                    ResourceOperation::Rename { old_path, new_path } => {
                        (old_path.clone(), new_path.clone())
                    }
                })
                .next()
                .ok_or_else(|| rename_request_error("document rename produced no resource edit"))?;
            if new_path.exists() {
                return Err(rename_request_error(format!(
                    "document rename target already exists: {}",
                    new_path.display()
                )));
            }
            if !self.roots.is_empty() && !self.roots.iter().any(|root| new_path.starts_with(root)) {
                return Err(rename_request_error(format!(
                    "document rename target is outside the workspace: {}",
                    new_path.display()
                )));
            }
            let lsp_edit = workspace_edit_to_lsp(&self.workspace, edit)
                .ok_or_else(|| rename_request_error("cannot map document rename edit"))?;
            self.begin_path_rename(old_path, new_path);
            Ok(Some(lsp_edit))
        })();
        Box::pin(async move { result })
    }
}

fn rename_request_error(message: impl Into<String>) -> ResponseError {
    ResponseError::new(ErrorCode::REQUEST_FAILED, message.into())
}

fn workspace_query_response_error(error: WorkspaceQueryError) -> ResponseError {
    ResponseError::new(
        ErrorCode::INTERNAL_ERROR,
        format!("workspace query failed: {error}"),
    )
}

fn optional_decorative_query<T>(
    result: Result<T, WorkspaceQueryError>,
) -> Result<Option<T>, WorkspaceQueryError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(WorkspaceQueryError::Incomplete) => Ok(None),
        Err(error) => Err(error),
    }
}

fn rename_operation_error(
    operation: &str,
    error: WorkspaceOperationError<RenameError>,
) -> ResponseError {
    match error {
        WorkspaceOperationError::Operation(error) => {
            rename_request_error(format!("{operation} failed: {error:?}"))
        }
        WorkspaceOperationError::Query(error) => workspace_query_response_error(error),
    }
}

fn document_rename_error(error: WorkspaceOperationError<RenameError>) -> ResponseError {
    match error {
        WorkspaceOperationError::Operation(RenameError::TargetExists) => {
            rename_request_error("document rename target already exists")
        }
        WorkspaceOperationError::Operation(error) => {
            rename_request_error(format!("document rename failed: {error:?}"))
        }
        WorkspaceOperationError::Query(error) => workspace_query_response_error(error),
    }
}

fn code_action_kind_requested(only: Option<&[CodeActionKind]>, candidate: &CodeActionKind) -> bool {
    only.is_none_or(|kinds| {
        kinds.iter().any(|requested| {
            candidate.as_str() == requested.as_str()
                || candidate
                    .as_str()
                    .strip_prefix(requested.as_str())
                    .is_some_and(|suffix| suffix.starts_with('.'))
        })
    })
}

fn location_for(
    workspace: &Workspace,
    path: &Path,
    range: &std::ops::Range<usize>,
) -> Option<Location> {
    let disk_source;
    let source = if let Some(entry) = workspace.get(path) {
        entry.parsed.source()
    } else {
        disk_source = fs::read_to_string(path).ok()?;
        &disk_source
    };
    let uri = Url::from_file_path(path).ok()?;
    Some(Location::new(uri, byte_range_to_lsp(source, range)))
}

struct ReferenceLocationCache<'a> {
    workspace: &'a Workspace,
    documents: HashMap<PathBuf, Option<(Url, PositionIndex<'a>)>>,
}

impl<'a> ReferenceLocationCache<'a> {
    fn new(workspace: &'a Workspace) -> Self {
        Self {
            workspace,
            documents: HashMap::new(),
        }
    }

    fn location(&mut self, path: &Path, range: &std::ops::Range<usize>) -> Option<Location> {
        if !self.documents.contains_key(path) {
            let cached = (|| {
                let source = if let Some(entry) = self.workspace.get(path) {
                    Cow::Borrowed(entry.parsed.source())
                } else {
                    Cow::Owned(fs::read_to_string(path).ok()?)
                };
                Some((
                    Url::from_file_path(path).ok()?,
                    PositionIndex::from_source(source),
                ))
            })();
            self.documents.insert(path.to_path_buf(), cached);
        }
        let (uri, positions) = self.documents.get(path)?.as_ref()?;
        Some(Location::new(
            uri.clone(),
            positions.byte_range_to_lsp(range),
        ))
    }
}

fn reference_code_lens(
    uri: &Url,
    range: lsp_types::Range,
    title: String,
    locations: Vec<Location>,
) -> CodeLens {
    CodeLens {
        range,
        command: Some(Command::new(
            title,
            "plumb.showReferences".to_string(),
            Some(vec![
                serde_json::json!(uri),
                serde_json::json!(range.start),
                serde_json::json!(locations),
            ]),
        )),
        data: None,
    }
}

fn text_edits_to_lsp(
    source: &str,
    edits: Vec<plumb_edit::TextEdit>,
) -> impl Iterator<Item = LspTextEdit> + '_ {
    let positions = (edits.len() > 1).then(|| PositionIndex::new(source));
    edits.into_iter().map(move |edit| {
        LspTextEdit::new(
            positions.as_ref().map_or_else(
                || byte_range_to_lsp(source, &edit.range),
                |positions| positions.byte_range_to_lsp(&edit.range),
            ),
            edit.new_text,
        )
    })
}

fn workspace_edit_to_lsp(workspace: &Workspace, edit: WorkspaceEdit) -> Option<LspWorkspaceEdit> {
    let has_resource_operations = !edit.resource_operations.is_empty();
    let mut document_edits = Vec::new();
    for document in edit.document_changes {
        let disk_source;
        let source = if let Some(entry) = workspace.get(&document.path) {
            entry.parsed.source()
        } else {
            disk_source = fs::read_to_string(&document.path).ok()?;
            &disk_source
        };
        let uri = Url::from_file_path(&document.path).ok()?;
        let version = (document.expected_revision > 0)
            .then(|| i32::try_from(document.expected_revision).ok())
            .flatten();
        let edits = text_edits_to_lsp(source, document.edits)
            .map(OneOf::Left)
            .collect();
        document_edits.push(TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier { uri, version },
            edits,
        });
    }
    let document_changes = if has_resource_operations {
        let mut operations = edit
            .resource_operations
            .into_iter()
            .filter_map(|operation| match operation {
                ResourceOperation::Rename { old_path, new_path } => Some(
                    DocumentChangeOperation::Op(ResourceOp::Rename(RenameFile {
                        old_uri: Url::from_file_path(old_path).ok()?,
                        new_uri: Url::from_file_path(new_path).ok()?,
                        options: Some(RenameFileOptions {
                            overwrite: Some(false),
                            ignore_if_exists: Some(false),
                        }),
                        annotation_id: None,
                    })),
                ),
            })
            .collect::<Vec<_>>();
        operations.extend(
            document_edits
                .into_iter()
                .map(DocumentChangeOperation::Edit),
        );
        DocumentChanges::Operations(operations)
    } else {
        DocumentChanges::Edits(document_edits)
    };
    Some(LspWorkspaceEdit {
        changes: None,
        document_changes: Some(document_changes),
        change_annotations: None,
    })
}

fn workspace_roots(params: &InitializeParams) -> Vec<PathBuf> {
    if let Some(folders) = &params.workspace_folders {
        return folders
            .iter()
            .filter_map(|folder| folder.uri.to_file_path().ok())
            .map(|path| normalize(&path))
            .collect();
    }
    #[allow(deprecated)]
    params
        .root_uri
        .as_ref()
        .and_then(|uri| uri.to_file_path().ok())
        .map(|path| vec![normalize(&path)])
        .unwrap_or_default()
}

fn scanned_files(roots: &[PathBuf]) -> (Vec<PathBuf>, bool) {
    let mut files = Vec::new();
    let mut complete = true;
    for root in roots {
        let scan = scan_workspace_files(root);
        complete &= scan.is_complete();
        files.extend(scan.files);
    }
    files.sort();
    files.dedup();
    (files, complete)
}

fn build_initial_index(roots: &[PathBuf], generation: u64) -> InitialIndexResult {
    let (files, mut complete) = scanned_files(roots);
    let cache_path = semantic_cache_path(roots);
    if let Some(parent) = cache_path.parent() {
        complete &= fs::create_dir_all(parent).is_ok();
    }
    let store = SqliteSemanticStore::open(&cache_path).or_else(|_| {
        complete = false;
        SqliteSemanticStore::open_in_memory()
    });
    let mut workspace = match store {
        Ok(store) => Workspace::with_sqlite_store(store),
        Err(_) => {
            complete = false;
            Workspace::new()
        }
    };
    let batch = workspace.index_disk_files(
        &files,
        BatchIndexOptions {
            prune_missing: complete,
            retain_sources: false,
        },
        |_| 0,
        || false,
    );
    let (indexed, cache_hits) = match batch {
        Ok(batch) => {
            complete &= batch.is_complete();
            (batch.documents.len(), batch.cache_hits())
        }
        Err(_) => {
            complete = false;
            workspace = Workspace::new();
            match workspace.index_disk_files(&files, BatchIndexOptions::default(), |_| 0, || false)
            {
                Ok(batch) => (batch.documents.len(), 0),
                Err(_) => (0, 0),
            }
        }
    };
    InitialIndexResult {
        generation,
        workspace,
        indexed,
        cache_hits,
        complete,
    }
}

fn semantic_cache_path(roots: &[PathBuf]) -> PathBuf {
    let base = crate::cache_cli::cache_base_dir();
    semantic_cache_path_in(&base, env!("CARGO_PKG_VERSION"), roots)
}

fn semantic_cache_path_in(base: &Path, version: &str, roots: &[PathBuf]) -> PathBuf {
    let mut hasher = Sha256::new();
    for root in roots {
        hasher.update(root.as_os_str().to_string_lossy().as_bytes());
        hasher.update([0]);
    }
    let digest = hasher.finalize();
    let key = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    base.join("plumb")
        .join("workspaces")
        .join(version)
        .join(format!("{key}.sqlite3"))
}

fn workspace_symbol_query(query: &str) -> (Option<SearchKind>, String) {
    let query = query.trim();
    for (prefix, kind) in [
        ("note", SearchKind::Note),
        ("task", SearchKind::Task),
        ("event", SearchKind::Event),
    ] {
        if query == prefix {
            return (Some(kind), String::new());
        }
        if let Some(query) = query
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_prefix(' '))
        {
            return (Some(kind), query.trim_start().to_string());
        }
    }
    (None, query.to_string())
}

fn is_plumb_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "plumb")
}

fn to_lsp_diagnostic(
    positions: &PositionIndex<'_>,
    uri: &Url,
    diagnostic: Diagnostic,
) -> LspDiagnostic {
    let related_information = (!diagnostic.related.is_empty()).then(|| {
        diagnostic
            .related
            .iter()
            .map(|range| DiagnosticRelatedInformation {
                location: Location::new(uri.clone(), positions.byte_range_to_lsp(range)),
                message: "Related source location".to_string(),
            })
            .collect()
    });
    LspDiagnostic {
        range: positions.byte_range_to_lsp(&diagnostic.range),
        severity: Some(match diagnostic.severity {
            plumb_syntax::DiagnosticSeverity::Error => DiagnosticSeverity::ERROR,
            plumb_syntax::DiagnosticSeverity::Warning => DiagnosticSeverity::WARNING,
            plumb_syntax::DiagnosticSeverity::Hint => DiagnosticSeverity::HINT,
        }),
        code: Some(NumberOrString::String(diagnostic.code.to_string())),
        code_description: None,
        source: Some("plumb".to_string()),
        message: diagnostic.message,
        related_information,
        tags: None,
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use lsp_types::CompletionClientCapabilities;
    use plumb_semantics::{analyze_headings, analyze_metadata, analyze_tasks};
    use plumb_syntax::parse;
    use plumb_workspace::StoreError;

    use super::*;

    #[test]
    fn formatting_projections_match_edit_layer_with_utf8_and_crlf() {
        for ending in ["\n", "\r\n"] {
            let (_main, client) =
                async_lsp::MainLoop::new_server(|_| async_lsp::router::Router::new(()));
            let mut state = ServerState::new(client);
            let path = "/tmp/plumb-format-projection.plumb";
            let mut source = String::new();
            for index in 0..3 {
                source.push_str(&format!("`note Head {index} \u{1f600}{ending}  `child \u{4e2d}{ending}{ending}Stable {index}{ending}Another {index}{ending}{ending}"));
            }
            state.workspace.open_document(path, 1, source.clone());
            let uri = Url::from_file_path(path).unwrap();
            let expected = |edits: Vec<plumb_edit::TextEdit>| {
                edits
                    .into_iter()
                    .map(|edit| {
                        LspTextEdit::new(byte_range_to_lsp(&source, &edit.range), edit.new_text)
                    })
                    .collect::<Vec<_>>()
            };
            let full = expected(
                plumb_edit::format_green(state.workspace.get(path).unwrap().parsed.green())
                    .unwrap(),
            );
            assert!(full.len() > 1);
            let params = serde_json::from_value(serde_json::json!({"textDocument":{"uri":uri},"options":{"tabSize":1,"insertSpaces":true}})).unwrap();
            assert_eq!(
                futures::FutureExt::now_or_never(state.formatting(params))
                    .unwrap()
                    .unwrap()
                    .unwrap(),
                full
            );
            let selection = 0..source.find("Stable 0").unwrap();
            let partial = expected(
                plumb_edit::format_green_contained(
                    state.workspace.get(path).unwrap().parsed.green(),
                    selection.clone(),
                )
                .unwrap(),
            );
            assert!(!partial.is_empty());
            let params = serde_json::from_value(serde_json::json!({"textDocument":{"uri":uri},"range":byte_range_to_lsp(&source, &selection),"options":{"tabSize":1,"insertSpaces":true}})).unwrap();
            assert_eq!(
                futures::FutureExt::now_or_never(state.range_formatting(params))
                    .unwrap()
                    .unwrap()
                    .unwrap(),
                partial
            );
        }
    }

    #[test]
    fn indexed_workspace_edits_preserve_ranges_versions_and_resource_order() {
        for ending in ["\n", "\r\n"] {
            for count in [0, 1, 12] {
                let path = PathBuf::from("/tmp/plumb-projection.plumb");
                let mut source = format!("`# \u{1f600} Heading{ending} `@ target{ending}{ending}");
                for _ in 0..count {
                    source.push_str(&format!("\u{4e2d} `->{{label #target}}{ending}"));
                }
                let mut workspace = Workspace::new();
                workspace.open_document(&path, 42, source.clone());
                let target = workspace
                    .anchor_rename_target_at(&path, source.find("target").unwrap())
                    .unwrap();
                let mut edit = workspace.rename_anchor(&target, "renamed").unwrap();
                assert_eq!(edit.document_changes[0].edits.len(), count + 1);
                let expected = edit.document_changes[0]
                    .edits
                    .iter()
                    .map(|edit| {
                        OneOf::Left(LspTextEdit::new(
                            byte_range_to_lsp(&source, &edit.range),
                            edit.new_text.clone(),
                        ))
                    })
                    .collect::<Vec<_>>();
                let converted = workspace_edit_to_lsp(&workspace, edit.clone()).unwrap();
                let Some(DocumentChanges::Edits(documents)) = converted.document_changes else {
                    panic!("text-only document changes");
                };
                assert_eq!(documents.len(), 1);
                assert_eq!(documents[0].text_document.version, Some(42));
                assert_eq!(
                    documents[0].text_document.uri,
                    Url::from_file_path(&path).unwrap()
                );
                assert_eq!(documents[0].edits, expected);
                edit.resource_operations.push(ResourceOperation::Rename {
                    old_path: path.clone(),
                    new_path: PathBuf::from("/tmp/plumb-renamed.plumb"),
                });
                let converted = workspace_edit_to_lsp(&workspace, edit.clone()).unwrap();
                let Some(DocumentChanges::Operations(operations)) = converted.document_changes
                else {
                    panic!("resource operations precede text changes");
                };
                assert_eq!(operations.len(), 2);
                assert!(matches!(
                    operations[0],
                    DocumentChangeOperation::Op(ResourceOp::Rename(_))
                ));
                assert_eq!(
                    operations[1],
                    DocumentChangeOperation::Edit(documents[0].clone())
                );
                edit.resource_operations.clear();
                edit.document_changes[0].edits.clear();
                let Some(DocumentChanges::Edits(documents)) =
                    workspace_edit_to_lsp(&workspace, edit)
                        .unwrap()
                        .document_changes
                else {
                    panic!("empty edit projection preserves document entry");
                };
                assert!(documents[0].edits.is_empty());
                assert_eq!(documents[0].text_document.version, Some(42));
            }
        }
    }

    #[test]
    fn cached_references_match_unindexed_positions_and_declaration_order() {
        let (_main, client) =
            async_lsp::MainLoop::new_server(|_| async_lsp::router::Router::new(()));
        let mut state = ServerState::new(client);
        let path = Path::new("/tmp/plumb-ref-target.plumb");
        state.workspace.open_document(
            path,
            1,
            "`= title Target\r\n\r\n`# Heading\r\n `@ target\r\n",
        );
        state.workspace.open_document("/tmp/plumb-ref-source.plumb", 1, "\u{1f600} See `->{label plumb-ref-target.plumb#target}\r\nSee `->{plumb-ref-target.plumb}\r\nSee `->{second plumb-ref-target.plumb#target}\r\n");
        state.index_complete = true;
        for anchor in [false, true] {
            let references = if anchor {
                state
                    .workspace
                    .references_to(path, "target")
                    .unwrap()
                    .value
                    .into_iter()
                    .map(|(path, reference)| (path, reference.source_range))
                    .collect::<Vec<_>>()
            } else {
                state
                    .workspace
                    .references_to_document(path)
                    .unwrap()
                    .value
                    .into_iter()
                    .map(|(path, reference)| (path, reference.source_range))
                    .collect::<Vec<_>>()
            };
            let expected = references
                .iter()
                .map(|(path, range)| location_for(&state.workspace, path, range).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(expected.len(), if anchor { 2 } else { 3 });
            for include_declaration in [false, true] {
                let mut expected = expected.clone();
                if include_declaration {
                    let range = if anchor {
                        state
                            .workspace
                            .get(path)
                            .unwrap()
                            .current
                            .as_ref()
                            .unwrap()
                            .output
                            .anchors()
                            .first()
                            .unwrap()
                            .selection_range
                    } else {
                        0..0
                    };
                    expected.insert(0, location_for(&state.workspace, path, &range).unwrap());
                }
                let position = if anchor {
                    lsp_types::Position::new(3, 4)
                } else {
                    lsp_types::Position::new(0, 0)
                };
                let params = serde_json::from_value(serde_json::json!({"textDocument":{"uri":Url::from_file_path(path).unwrap()},"position":position,"context":{"includeDeclaration":include_declaration}})).unwrap();
                let actual = futures::FutureExt::now_or_never(state.references(params))
                    .unwrap()
                    .unwrap()
                    .unwrap();
                assert_eq!(actual, expected);
            }
        }
    }

    #[test]
    fn reference_location_cache_preserves_positions_and_request_local_source_precedence() {
        let root = std::env::temp_dir().join(format!(
            "plumb-lens-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let path = root.join("source.plumb");
        let source = "Prelude \u{1f600}\r\nSecond \u{4e2d}\n";
        fs::write(&path, "different disk bytes\n").unwrap();
        let mut workspace = Workspace::new();
        workspace.open_document(&path, 1, source);
        let mut cache = ReferenceLocationCache::new(&workspace);
        for offset in 0..source.len() + 3 {
            assert_eq!(
                cache.location(&path, &(offset..offset)),
                location_for(&workspace, &path, &(offset..offset))
            );
        }
        assert_eq!(cache.documents.len(), 1);
        drop(cache);
        workspace.open_document(&path, 2, "New revision\n");
        assert_eq!(
            ReferenceLocationCache::new(&workspace).location(&path, &(0..20)),
            location_for(&workspace, &path, &(0..20))
        );

        let workspace = Workspace::new();
        fs::write(&path, source).unwrap();
        let mut cache = ReferenceLocationCache::new(&workspace);
        let original = cache.location(&path, &(0..source.len()));
        assert_eq!(
            original,
            location_for(&workspace, &path, &(0..source.len()))
        );
        fs::write(&path, "New disk bytes\n").unwrap();
        assert_eq!(cache.location(&path, &(0..source.len())), original);
        let mut next_request = ReferenceLocationCache::new(&workspace);
        let refreshed = next_request.location(&path, &(0..source.len()));
        assert_eq!(
            refreshed,
            location_for(&workspace, &path, &(0..source.len()))
        );
        assert_ne!(refreshed, original);
        let missing = root.join("missing.plumb");
        assert!(cache.location(&missing, &(0..0)).is_none());
        fs::write(&missing, "Now present\n").unwrap();
        assert!(cache.location(&missing, &(0..0)).is_none());
        assert!(ReferenceLocationCache::new(&workspace)
            .location(&missing, &(0..0))
            .is_some());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn event_only_install_reuses_context_but_anchor_change_rebuilds_it() {
        let (_main, client) =
            async_lsp::MainLoop::new_server(|_| async_lsp::router::Router::new(()));
        let mut state = ServerState::new(client);
        let path = PathBuf::from("/tmp/impact-event.plumb");
        let old = "`- 2026-09-07T10:00:00+08:00 Old\n `+ event\n";
        state.workspace.open_document(&path, 1, old);
        state.publish_all_open_diagnostics();
        let original = state.diagnostic_context.clone().unwrap();
        let (_, generation) = state.document_analysis_tokens.next(&path);
        let prepared = state
            .workspace
            .begin_document_revision(&path, 2, old.replace("Old", "New"))
            .unwrap()
            .analyze();
        let _ = state.finish_document_analysis(DocumentAnalysisResult {
            path: path.clone(),
            generation,
            analysis: Ok(prepared),
        });
        assert!(Arc::ptr_eq(
            &original,
            state.diagnostic_context.as_ref().unwrap()
        ));
        let (_, generation) = state.document_analysis_tokens.next(&path);
        let prepared = state
            .workspace
            .begin_document_revision(&path, 3, format!("{old} `@ event-id\n"))
            .unwrap()
            .analyze();
        let _ = state.finish_document_analysis(DocumentAnalysisResult {
            path,
            generation,
            analysis: Ok(prepared),
        });
        assert!(!Arc::ptr_eq(
            &original,
            state.diagnostic_context.as_ref().unwrap()
        ));
    }

    #[tokio::test]
    async fn code_lenses_recover_once_after_semantic_equal_pending_revisions() {
        use std::io::{BufRead, Read};
        for supported in [true, false] {
            for (own, finish) in [
                (false, "worker"),
                (true, "worker"),
                (false, "navigation"),
                (true, "navigation"),
                (false, "close"),
                (true, "close"),
            ] {
                for trigger in ["query", "refresh", "both"] {
                    struct Stop;
                    let (server, client) = async_lsp::MainLoop::new_server(|_| {
                        let mut router = async_lsp::router::Router::new(());
                        router.event::<Stop>(|_, _| ControlFlow::Break(Ok(())));
                        router
                    });
                    let mut state = ServerState::new(client.clone());
                    state.index_complete = true;
                    state.supports_code_lens_refresh = supported;
                    let root = Path::new("/tmp/plumb-lens-recovery");
                    let target = root.join("target.plumb");
                    state
                        .workspace
                        .open_document(&target, 1, "`# Target\n `@ target\n");
                    let source = "Old text\n\nSee `->{target target.plumb#target}\n";
                    let paths = [root.join("a.plumb"), root.join("b.plumb")];
                    for path in &paths {
                        state.workspace.open_document(path, 1, source);
                        state
                            .open_documents
                            .insert(Url::from_file_path(path).unwrap(), path.clone());
                    }
                    let mut pending = Vec::new();
                    for path in &paths {
                        let (_, generation) = state.document_analysis_tokens.next(path);
                        let analysis = state
                            .workspace
                            .begin_document_revision(path, 2, source.replace("Old", "New"))
                            .unwrap()
                            .analyze();
                        pending.push(DocumentAnalysisResult {
                            path: path.clone(),
                            generation,
                            analysis: Ok(analysis),
                        });
                    }
                    let query_path = if own { &paths[0] } else { &target };
                    let params: CodeLensParams = serde_json::from_value(serde_json::json!({"textDocument":{"uri":Url::from_file_path(query_path).unwrap()}})).unwrap();
                    if trigger != "refresh" {
                        for _ in 0..2 {
                            assert!(futures::FutureExt::now_or_never(
                                state.code_lens(params.clone())
                            )
                            .unwrap()
                            .unwrap()
                            .is_none());
                        }
                    }
                    if trigger != "query" {
                        state.refresh_code_lenses();
                        state.refresh_code_lenses();
                    }
                    let _ = state.finish_document_analysis(pending.remove(0));
                    if trigger != "refresh" {
                        assert!(
                            futures::FutureExt::now_or_never(state.code_lens(params.clone()))
                                .unwrap()
                                .unwrap()
                                .is_none()
                        );
                    }
                    assert_eq!(state.code_lens_refresh_pending, supported);
                    match finish {
                        "worker" => {
                            let _ = state.finish_document_analysis(pending.remove(0));
                        }
                        "navigation" => {
                            assert!(state.complete_pending_navigation_documents([paths[1].clone()]));
                            let _ = state.finish_document_analysis(pending.remove(0));
                        }
                        "close" => {
                            let _ = state.did_close(DidCloseTextDocumentParams {
                                text_document: lsp_types::TextDocumentIdentifier {
                                    uri: Url::from_file_path(&paths[1]).unwrap(),
                                },
                            });
                            let _ = state.finish_document_analysis(pending.remove(0));
                        }
                        _ => unreachable!(),
                    }
                    assert!(!state.code_lens_refresh_pending);
                    let complete = futures::FutureExt::now_or_never(state.code_lens(params))
                        .unwrap()
                        .unwrap()
                        .unwrap();
                    assert_eq!(
                        complete[0].command.as_ref().unwrap().title,
                        if own {
                            "0 file references"
                        } else if finish == "close" {
                            "1 file reference"
                        } else {
                            "2 file references"
                        }
                    );
                    tokio::task::yield_now().await;
                    client.emit(Stop).unwrap();
                    let mut output = Vec::new();
                    server
                        .run_buffered(futures::io::Cursor::new(Vec::<u8>::new()), &mut output)
                        .await
                        .unwrap();
                    let mut input = std::io::Cursor::new(output);
                    let mut refreshes = 0;
                    while input.position() < input.get_ref().len() as u64 {
                        let mut header = String::new();
                        input.read_line(&mut header).unwrap();
                        let length: usize = header
                            .trim()
                            .strip_prefix("Content-Length: ")
                            .unwrap()
                            .parse()
                            .unwrap();
                        let mut separator = [0; 2];
                        input.read_exact(&mut separator).unwrap();
                        let mut body = vec![0; length];
                        input.read_exact(&mut body).unwrap();
                        let message: serde_json::Value = serde_json::from_slice(&body).unwrap();
                        if message["method"] == "workspace/codeLens/refresh" {
                            refreshes += 1;
                        }
                    }
                    assert_eq!(
                        refreshes,
                        usize::from(supported),
                        "own={own} supported={supported} finish={finish} trigger={trigger}"
                    );
                }
            }
        }
    }

    #[test]
    fn pending_publication_does_not_reuse_a_previously_complete_cycle_graph() {
        let (_main, client) =
            async_lsp::MainLoop::new_server(|_| async_lsp::router::Router::new(()));
        let mut state = ServerState::new(client);
        let a = "/tmp/plumb-cycle-a.plumb";
        let b = "/tmp/plumb-cycle-b.plumb";
        let source_b = "`- B\n `+ task\n `@ b\n `= depends plumb-cycle-a.plumb#a\n";
        state.workspace.open_document(
            a,
            1,
            "`- A\n `+ task\n `@ a\n `= depends plumb-cycle-b.plumb#b\n",
        );
        state.workspace.open_document(b, 1, source_b);
        state.publish_all_open_diagnostics();
        let cached = state.diagnostic_context.clone().unwrap();
        let _pending = state
            .workspace
            .begin_document_revision(b, 2, source_b)
            .unwrap();
        let temporary = state.diagnostic_publication_context().unwrap();
        assert!(state.diagnostic_context.is_none());
        assert!(!Arc::ptr_eq(&cached, &temporary));
        let stale = state
            .workspace
            .diagnostics_with_context(a, &cached)
            .unwrap();
        assert!(stale
            .value
            .iter()
            .any(|diagnostic| diagnostic.code == "task.dependency-cycle"));
        let current = state
            .workspace
            .diagnostics_with_context(a, &temporary)
            .unwrap();
        assert!(!current
            .value
            .iter()
            .any(|diagnostic| diagnostic.code == "task.dependency-cycle"));
    }

    #[test]
    fn pending_publication_does_not_cache_incomplete_dependency_context() {
        let (_main, client) =
            async_lsp::MainLoop::new_server(|_| async_lsp::router::Router::new(()));
        let mut state = ServerState::new(client);
        let path = PathBuf::from("/tmp/context-a.plumb");
        let source = "`- A\n `+ task\n `@ a\n `= depends context-b.plumb#b\n";
        state.workspace.open_document(&path, 1, source);
        state.workspace.open_document(
            "/tmp/context-b.plumb",
            1,
            "`- B\n `+ task\n `@ b\n `= depends context-a.plumb#a\n",
        );
        state.publish_all_open_diagnostics();
        assert!(state.diagnostic_context.is_some());
        let (_, generation) = state.document_analysis_tokens.next(&path);
        let pending = state
            .workspace
            .begin_document_revision(&path, 2, source)
            .unwrap();
        state.publish_all_open_diagnostics();
        assert!(state.diagnostic_context.is_none());
        let _ = state.finish_document_analysis(DocumentAnalysisResult {
            path: path.clone(),
            generation,
            analysis: Ok(pending.analyze()),
        });
        let context = state.diagnostic_context.as_ref().unwrap();
        let diagnostics = state
            .workspace
            .diagnostics_with_context(&path, context)
            .unwrap();
        assert!(diagnostics
            .value
            .iter()
            .any(|diagnostic| diagnostic.code == "task.dependency-cycle"));
    }

    #[tokio::test]
    async fn completion_clears_temporary_diagnostics_after_every_pending_publication_path() {
        async fn run(scope: usize, title: &'static str) {
            use std::io::{BufRead, Read};
            struct Run;
            struct Stop;
            let (server, client) = async_lsp::MainLoop::new_server(|client| {
                let completion = client.clone();
                let mut router = async_lsp::router::Router::new(ServerState::new(client));
                router.event::<Run>(move |state, _| {
                    let path = PathBuf::from("/tmp/plumb-context-event.plumb");
                    let other = PathBuf::from("/tmp/plumb-context-other.plumb");
                    let source = "`- 2026-09-07T10:00:00Z Old\n `+ event\n `@ event\n";
                    for (path, text) in [
                        (&path, source),
                        (&other, "See `->{plumb-context-event.plumb#event}\n"),
                    ] {
                        state.workspace.open_document(path, 1, text);
                        state
                            .open_documents
                            .insert(Url::from_file_path(path).unwrap(), path.clone());
                    }
                    state.publish_all_open_diagnostics();
                    assert!(state.diagnostic_context.is_some());
                    let (_, generation) = state.document_analysis_tokens.next(&path);
                    let pending = state
                        .workspace
                        .begin_document_revision(&path, 2, source.replace("Old", title))
                        .unwrap();
                    match scope {
                        0 => state.publish_all_open_diagnostics(),
                        1 => state.publish_all_open_diagnostics_reusing_context(),
                        2 => state.publish_open_document_diagnostics(&other),
                        3 => state.publish_diagnostics_for_targets(&path, &HashSet::new()),
                        _ => unreachable!(),
                    }
                    let result = state.finish_document_analysis(DocumentAnalysisResult {
                        path,
                        generation,
                        analysis: Ok(pending.analyze()),
                    });
                    assert!(state.diagnostic_context.is_some());
                    completion.emit(Stop).unwrap();
                    result
                });
                router.event::<Stop>(|_, _| ControlFlow::Break(Ok(())));
                router
            });
            client.emit(Run).unwrap();
            let mut output = Vec::new();
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                server.run_buffered(futures::io::Cursor::new(Vec::<u8>::new()), &mut output),
            )
            .await
            .unwrap()
            .unwrap();
            let mut input = std::io::Cursor::new(output);
            let mut publications = Vec::new();
            while input.position() < input.get_ref().len() as u64 {
                let mut header = String::new();
                input.read_line(&mut header).unwrap();
                let length: usize = header
                    .trim()
                    .strip_prefix("Content-Length: ")
                    .unwrap()
                    .parse()
                    .unwrap();
                let mut separator = [0; 2];
                input.read_exact(&mut separator).unwrap();
                assert_eq!(&separator, b"\r\n");
                let mut body = vec![0; length];
                input.read_exact(&mut body).unwrap();
                let message: serde_json::Value = serde_json::from_slice(&body).unwrap();
                if message["method"] == "textDocument/publishDiagnostics"
                    && message["params"]["uri"] == "file:///tmp/plumb-context-other.plumb"
                {
                    publications.push(message["params"]["diagnostics"].clone());
                }
            }
            assert_eq!(publications[0], serde_json::json!([]));
            assert!(publications[1]
                .as_array()
                .unwrap()
                .iter()
                .any(|diagnostic| diagnostic["code"] == "link.unresolved-anchor"));
            assert_eq!(
                publications.last().unwrap(),
                &serde_json::json!([]),
                "scope={scope} title={title}: recovery must clear the temporary unresolved warning"
            );
            assert_eq!(publications.len(), 3);
        }
        for scope in 0..4 {
            for title in ["Old", "New"] {
                run(scope, title).await;
            }
        }
    }

    #[test]
    #[ignore = "manual release-profile timing; no wall-clock correctness threshold"]
    fn profile_formatting_response() {
        let (_main, client) =
            async_lsp::MainLoop::new_server(|_| async_lsp::router::Router::new(()));
        let mut state = ServerState::new(client);
        let path = "/tmp/plumb-format-profile.plumb";
        let mut source = String::new();
        for index in 0..2_000 {
            source.push_str(&format!(
                "`note Head {index}\n  `child Body\n\nStable {index}\nAnother stable {index}\n\n"
            ));
        }
        state.workspace.open_document(path, 1, source);
        let params: DocumentFormattingParams = serde_json::from_value(serde_json::json!({"textDocument":{"uri":Url::from_file_path(path).unwrap()},"options":{"tabSize":1,"insertSpaces":true}})).unwrap();
        let mut run = || {
            let edits = futures::FutureExt::now_or_never(state.formatting(params.clone()))
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(edits.len() >= 2_000, "edits={}", edits.len());
            serde_json::to_vec(&edits).unwrap()
        };
        let expected = run();
        for _ in 0..3 {
            std::hint::black_box(run());
        }
        let mut samples = Vec::new();
        for _ in 0..10 {
            let start = std::time::Instant::now();
            let bytes = run();
            samples.push(start.elapsed());
            assert_eq!(bytes, expected);
        }
        samples.sort();
        eprintln!("formatting_response: changed_subtrees=2000 bytes={} warmup=3 samples=10 median={:?}; includes formatting/diff/projection/json, excludes initial parsing/transport/application", expected.len(), samples[5]);
    }

    #[test]
    #[ignore = "manual release-profile timing; no wall-clock correctness threshold"]
    fn profile_workspace_edit_projection() {
        let path = "/tmp/plumb-edit-profile.plumb";
        let mut source = "`# Section\n `@ target\n\n".to_owned();
        source.push_str(&"See `->{label #target}\n".repeat(2_000));
        let offset = source.find("target").unwrap();
        let mut workspace = Workspace::new();
        workspace.open_document(path, 42, source);
        let target = workspace.anchor_rename_target_at(path, offset).unwrap();
        let edit = workspace.rename_anchor(&target, "renamed").unwrap();
        assert_eq!(edit.document_changes.len(), 1);
        assert_eq!(edit.document_changes[0].edits.len(), 2_001);
        let run = || {
            serde_json::to_vec(&workspace_edit_to_lsp(&workspace, edit.clone()).unwrap()).unwrap()
        };
        let expected = run();
        for _ in 0..3 {
            std::hint::black_box(run());
        }
        let mut samples = Vec::new();
        for _ in 0..10 {
            let start = std::time::Instant::now();
            let bytes = run();
            samples.push(start.elapsed());
            assert_eq!(bytes, expected);
        }
        samples.sort();
        eprintln!("workspace_edit_projection: edits=2001 bytes={} warmup=3 samples=10 median={:?}; includes input clone/projection/json, excludes rename planning/transport/application", expected.len(), samples[5]);
    }

    #[test]
    #[ignore = "manual release-profile timing; no wall-clock correctness threshold"]
    fn profile_references_response() {
        let (_main, client) =
            async_lsp::MainLoop::new_server(|_| async_lsp::router::Router::new(()));
        let mut state = ServerState::new(client);
        let path = "/tmp/plumb-reference-target.plumb";
        state
            .workspace
            .open_document(path, 1, "`# Target\n `@ target\n");
        state.workspace.open_document(
            "/tmp/plumb-reference-source.plumb",
            1,
            "See `->{label plumb-reference-target.plumb#target}\n".repeat(2_000),
        );
        state.index_complete = true;
        let params: ReferenceParams = serde_json::from_value(serde_json::json!({
            "textDocument":{"uri":Url::from_file_path(path).unwrap()},
            "position":{"line":1,"character":4},"context":{"includeDeclaration":true}
        }))
        .unwrap();
        let mut run = || {
            let locations = futures::FutureExt::now_or_never(state.references(params.clone()))
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(locations.len(), 2_001);
            assert_eq!(locations[0].uri, Url::from_file_path(path).unwrap());
            serde_json::to_vec(&locations).unwrap()
        };
        let expected = run();
        for _ in 0..3 {
            std::hint::black_box(run());
        }
        let mut samples = Vec::new();
        for _ in 0..10 {
            let start = std::time::Instant::now();
            let bytes = run();
            samples.push(start.elapsed());
            assert_eq!(bytes, expected);
        }
        samples.sort();
        eprintln!("references_response: references=2000 bytes={} warmup=3 samples=10 median={:?}; includes query/projection/json, excludes analysis/transport/rendering", expected.len(), samples[5]);
    }

    #[test]
    #[ignore = "manual release-profile timing; no wall-clock correctness threshold"]
    fn profile_code_lens_response() {
        let (_main, client) =
            async_lsp::MainLoop::new_server(|_| async_lsp::router::Router::new(()));
        let mut state = ServerState::new(client);
        let path = "/tmp/plumb-lens-target.plumb";
        let mut target = String::new();
        let mut references = String::new();
        for index in 0..2_000 {
            target.push_str(&format!("`# Heading {index}\n `@ id-{index}\n\n"));
            references.push_str(&format!(
                "See `->{{label plumb-lens-target.plumb#id-{index}}}\n"
            ));
        }
        state.workspace.open_document(path, 1, target);
        state
            .workspace
            .open_document("/tmp/plumb-lens-source.plumb", 1, references);
        state.index_complete = true;
        let params: CodeLensParams = serde_json::from_value(
            serde_json::json!({"textDocument":{"uri":Url::from_file_path(path).unwrap()}}),
        )
        .unwrap();
        let mut run = || {
            let lenses = futures::FutureExt::now_or_never(state.code_lens(params.clone()))
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(lenses.len(), 2_001);
            assert_eq!(
                lenses[0]
                    .command
                    .as_ref()
                    .unwrap()
                    .arguments
                    .as_ref()
                    .unwrap()[2]
                    .as_array()
                    .unwrap()
                    .len(),
                2_000
            );
            assert!(lenses[1..].iter().all(|lens| lens
                .command
                .as_ref()
                .unwrap()
                .arguments
                .as_ref()
                .unwrap()[2]
                .as_array()
                .unwrap()
                .len()
                == 1));
            serde_json::to_vec(&lenses).unwrap()
        };
        let expected = run();
        for _ in 0..3 {
            std::hint::black_box(run());
        }
        let mut samples = Vec::new();
        for _ in 0..10 {
            let start = std::time::Instant::now();
            let bytes = run();
            samples.push(start.elapsed());
            assert_eq!(bytes, expected);
        }
        samples.sort();
        eprintln!("code_lens_response: anchors=2000 references=2000 bytes={} warmup=3 samples=10 median={:?}; includes query/projection/json, excludes document analysis/transport/rendering", expected.len(), samples[5]);
    }

    #[test]
    #[ignore = "manual release-profile timing; no wall-clock correctness threshold"]
    fn profile_document_symbol_response() {
        let count: usize = std::env::var("PLUMB_PROFILE_SYMBOL_COUNT")
            .map(|value| value.parse().unwrap())
            .unwrap_or(2_000);
        let (_main, client) =
            async_lsp::MainLoop::new_server(|_| async_lsp::router::Router::new(()));
        let mut state = ServerState::new(client);
        let path = "/tmp/plumb-symbol-profile.plumb";
        let mut source = String::new();
        for index in 0..count {
            source.push_str(&format!(
                "`# Section {index}\n\n`- Task {index}\n `+ task\n `@ task-{index}\n\n"
            ));
        }
        state.workspace.open_document(path, 1, source);
        let params: DocumentSymbolParams = serde_json::from_value(serde_json::json!({
            "textDocument": {"uri": Url::from_file_path(path).unwrap()}
        }))
        .unwrap();
        let mut run = || {
            let result = futures::FutureExt::now_or_never(state.document_symbol(params.clone()))
                .expect("current-valid symbol projection completes immediately")
                .unwrap()
                .unwrap();
            let DocumentSymbolResponse::Nested(symbols) = &result else {
                panic!("nested symbols expected")
            };
            assert_eq!(symbols.len(), count);
            assert!(symbols
                .iter()
                .all(|symbol| symbol.children.as_ref().unwrap().len() == 1));
            serde_json::to_vec(&result).unwrap()
        };
        let expected = run();
        for _ in 0..5 {
            std::hint::black_box(run());
        }
        let mut samples = Vec::new();
        for _ in 0..20 {
            let start = std::time::Instant::now();
            let bytes = run();
            samples.push(start.elapsed());
            assert_eq!(bytes, expected);
            std::hint::black_box(bytes);
        }
        samples.sort();
        eprintln!(
            "document_symbol_response: headings={count} tasks={count} bytes={} samples=20 median={:?}",
            expected.len(),
            samples[10]
        );
    }

    #[test]
    fn decorative_queries_distinguish_incomplete_from_complete_empty_results() {
        assert_eq!(
            optional_decorative_query::<Vec<()>>(Err(WorkspaceQueryError::Incomplete)).unwrap(),
            None
        );
        assert_eq!(
            optional_decorative_query(Ok(Vec::<()>::new())).unwrap(),
            Some(Vec::new())
        );
        assert!(matches!(
            optional_decorative_query::<Vec<()>>(Err(WorkspaceQueryError::Store(
                StoreError::InvalidStoredValue
            ))),
            Err(WorkspaceQueryError::Store(StoreError::InvalidStoredValue))
        ));
    }

    #[test]
    fn applies_incremental_changes_sequentially_in_utf16_coordinates() {
        let text = "a😀\nsecond\n".to_string();
        let lines = LineIndex::new(&text);
        let changes = vec![
            TextDocumentContentChangeEvent {
                range: Some(lsp_types::Range::new(
                    lsp_types::Position::new(0, 0),
                    lsp_types::Position::new(0, 1),
                )),
                range_length: Some(1),
                text: "alpha".to_string(),
            },
            TextDocumentContentChangeEvent {
                range: Some(lsp_types::Range::new(
                    lsp_types::Position::new(0, 5),
                    lsp_types::Position::new(0, 7),
                )),
                range_length: Some(2),
                text: "x".to_string(),
            },
            TextDocumentContentChangeEvent {
                range: Some(lsp_types::Range::new(
                    lsp_types::Position::new(1, 6),
                    lsp_types::Position::new(1, 6),
                )),
                range_length: Some(0),
                text: "!".to_string(),
            },
        ];

        let (text, lines, change) = apply_content_changes(text, lines, changes).unwrap();
        assert_eq!(text, "alphax\nsecond!\n");
        assert_eq!(lines, LineIndex::new(&text));
        assert!(change.is_none());
    }

    #[test]
    fn full_change_resets_the_base_for_following_ranged_changes() {
        let text = "old".to_string();
        let lines = LineIndex::new(&text);
        let changes = vec![
            TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "first\nsecond".to_string(),
            },
            TextDocumentContentChangeEvent {
                range: Some(lsp_types::Range::new(
                    lsp_types::Position::new(1, 0),
                    lsp_types::Position::new(1, 6),
                )),
                range_length: Some(6),
                text: "next".to_string(),
            },
        ];

        let (text, lines, change) = apply_content_changes(text, lines, changes).unwrap();
        assert_eq!(text, "first\nnext");
        assert_eq!(lines, LineIndex::new(&text));
        assert!(change.is_none());
    }

    #[test]
    fn single_ranged_change_preserves_byte_change_provenance() {
        let text = "a😀\nsecond\n".to_string();
        let lines = LineIndex::new(&text);
        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(lsp_types::Range::new(
                lsp_types::Position::new(0, 1),
                lsp_types::Position::new(0, 3),
            )),
            range_length: Some(2),
            text: "emoji".to_string(),
        }];

        let (text, lines, change) = apply_content_changes(text, lines, changes).unwrap();
        assert_eq!(text, "aemoji\nsecond\n");
        assert_eq!(lines, LineIndex::new(&text));
        assert_eq!(
            change,
            Some(SourceChange {
                old_range: 1..5,
                new_range: 1..6,
            })
        );
    }

    #[test]
    fn rejects_invalid_incremental_ranges_and_lengths() {
        let text = "a😀\n";
        for change in [
            TextDocumentContentChangeEvent {
                range: Some(lsp_types::Range::new(
                    lsp_types::Position::new(0, 2),
                    lsp_types::Position::new(0, 3),
                )),
                range_length: None,
                text: String::new(),
            },
            TextDocumentContentChangeEvent {
                range: Some(lsp_types::Range::new(
                    lsp_types::Position::new(0, 1),
                    lsp_types::Position::new(0, 3),
                )),
                range_length: Some(1),
                text: String::new(),
            },
        ] {
            assert!(
                apply_content_changes(text.to_string(), LineIndex::new(text), vec![change])
                    .is_err()
            );
        }
    }

    #[test]
    fn rejects_a_change_batch_without_returning_a_partially_updated_document() {
        let text = "first\nsecond\n";
        let changes = vec![
            TextDocumentContentChangeEvent {
                range: Some(lsp_types::Range::new(
                    lsp_types::Position::new(0, 0),
                    lsp_types::Position::new(0, 5),
                )),
                range_length: Some(5),
                text: "changed".to_string(),
            },
            TextDocumentContentChangeEvent {
                range: Some(lsp_types::Range::new(
                    lsp_types::Position::new(9, 0),
                    lsp_types::Position::new(9, 0),
                )),
                range_length: Some(0),
                text: "invalid".to_string(),
            },
        ];
        assert!(apply_content_changes(text.to_string(), LineIndex::new(text), changes).is_err());
        assert_eq!(text, "first\nsecond\n");
    }

    #[test]
    fn newer_document_analysis_generations_cancel_queued_and_closed_work() {
        let path = Path::new("note.plumb");
        let mut tokens = DocumentAnalysisTokens::default();
        let (first_token, first) = tokens.next(path);
        let (_, second) = tokens.next(path);

        assert_ne!(first, second);
        assert_eq!(first_token.load(Ordering::Acquire), second);
        assert!(!tokens.is_current(path, first));
        assert!(tokens.is_current(path, second));

        tokens.cancel(path);
        assert!(!tokens.is_current(path, second));
        assert_ne!(first_token.load(Ordering::Acquire), second);
        let (_, reopened) = tokens.next(path);
        assert_ne!(reopened, first);
        assert_ne!(reopened, second);
        assert!(!tokens.is_current(path, second));
    }

    #[test]
    fn semantic_cache_paths_are_namespaced_by_compiled_version() {
        let base = Path::new("/cache");
        let roots = [PathBuf::from("/notes"), PathBuf::from("/projects")];
        let current = semantic_cache_path_in(base, "0.34.1", &roots);
        let next = semantic_cache_path_in(base, "0.34.2", &roots);

        assert_eq!(current.parent().unwrap().file_name().unwrap(), "0.34.1");
        assert_eq!(next.parent().unwrap().file_name().unwrap(), "0.34.2");
        assert_eq!(current.file_name(), next.file_name());
        assert_ne!(current, next);
    }

    #[test]
    fn negotiates_completion_indentation_modes() {
        let both: CompletionClientCapabilities = serde_json::from_value(serde_json::json!({
            "completionItem": { "insertTextModeSupport": { "valueSet": [1, 2] } },
            "insertTextMode": 2
        }))
        .unwrap();
        assert_eq!(
            completion_indentation(Some(&both)),
            CompletionIndentation {
                projection: CompletionIndentationProjection::AsIs,
                item_mode: Some(InsertTextMode::AS_IS),
            }
        );

        let adjusted: CompletionClientCapabilities = serde_json::from_value(serde_json::json!({
            "completionItem": { "insertTextModeSupport": { "valueSet": [2] } },
            "insertTextMode": 2
        }))
        .unwrap();
        assert_eq!(
            completion_indentation(Some(&adjusted)),
            CompletionIndentation {
                projection: CompletionIndentationProjection::AdjustIndentation,
                item_mode: Some(InsertTextMode::ADJUST_INDENTATION),
            }
        );

        let default_as_is: CompletionClientCapabilities =
            serde_json::from_value(serde_json::json!({ "insertTextMode": 1 })).unwrap();
        assert_eq!(
            completion_indentation(Some(&default_as_is)),
            CompletionIndentation {
                projection: CompletionIndentationProjection::AsIs,
                item_mode: None,
            }
        );
        let default_adjusted: CompletionClientCapabilities =
            serde_json::from_value(serde_json::json!({ "insertTextMode": 2 })).unwrap();
        assert_eq!(
            completion_indentation(Some(&default_adjusted)),
            CompletionIndentation::default()
        );
        assert_eq!(
            completion_indentation(None),
            CompletionIndentation::default()
        );
    }

    #[test]
    fn task_completion_projections_preserve_canonical_layout() {
        let timestamp = "2026-08-10T12:00:00+08:00";
        let absolute = task_construct_template("  ", timestamp).snippet;
        let relative = task_construct_template(" ", timestamp).snippet;
        let adjusted = relative
            .split('\n')
            .enumerate()
            .map(|(index, line)| {
                if index == 0 {
                    line.to_string()
                } else {
                    format!(" {line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(adjusted, absolute.replace("\n\n", "\n \n"));
        assert!(absolute.contains("\n\n  `= created "));
        assert!(relative.contains("\n\n `= created "));
        assert!(!absolute.contains(" {"));
        assert!(!absolute.contains("\n}"));
        assert!(!relative.contains(" {"));
        assert!(!relative.contains("\n}"));

        let expanded = format!("{}\n", relative.replace("${1:Task}", "Task"));
        let expanded = plumb_syntax::parse(expanded);
        assert!(expanded.is_valid(), "{:?}", expanded.diagnostics);
        let edits = plumb_edit::format(&expanded, plumb_edit::FormatScope::Document).unwrap();
        assert!(edits.is_empty(), "{edits:?}");
        let plain = format!("{}\n", task_construct_template(" ", timestamp).plain);
        let plain = plumb_syntax::parse(plain);
        assert!(plain.is_valid(), "{:?}", plain.diagnostics);
        let edits = plumb_edit::format(&plain, plumb_edit::FormatScope::Document).unwrap();
        assert!(edits.is_empty(), "{edits:?}");
    }

    #[test]
    fn maps_nested_heading_facts_to_nested_symbols() {
        let parsed = parse("`# One\n`## Two\n");
        let output = analyze_headings(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        let symbols = output
            .headings
            .iter()
            .map(|heading| heading_symbol(&PositionIndex::new(&parsed.source), heading))
            .collect::<Vec<_>>();
        assert_eq!(symbols[0].name, "One");
        assert_eq!(symbols[0].children.as_ref().unwrap()[0].name, "Two");
    }

    #[test]
    fn folds_heading_sections_and_multiline_syntax_blocks() {
        let parsed = parse(
            "`# Top\n\nIntro.\n\n`## Child\n\n`div Details\n\n body\n\n `text\"\n  raw\n`# Next\n\nTail.\n",
        );
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let ranges = folding_ranges(&parsed.source, &parsed.syntax, None, None, false);
        assert_eq!(
            ranges
                .iter()
                .map(|range| (range.start_line, range.end_line))
                .collect::<Vec<_>>(),
            [(0, 11), (4, 11), (6, 11), (10, 11), (12, 14)]
        );
        assert!(ranges.iter().all(|range| {
            range.start_character.is_none()
                && range.end_character.is_none()
                && range.kind.is_none()
                && range.collapsed_text.is_none()
        }));
        assert_eq!(
            folding_ranges(&parsed.source, &parsed.syntax, Some(2), None, false).len(),
            2
        );
    }

    #[test]
    fn folds_recovered_marked_blocks_but_not_multiline_paragraphs() {
        let parsed = parse("`node Parent\n  `child Child\nordinary\ncontinued `span[open\n");
        assert!(!parsed.is_valid());
        assert_eq!(
            folding_ranges(&parsed.source, &parsed.syntax, None, None, false)
                .iter()
                .map(|range| (range.start_line, range.end_line))
                .collect::<Vec<_>>(),
            [(0, 1)]
        );
    }

    #[test]
    fn keeps_task_owner_fold_while_typing_its_marker() {
        for opener in ["`", "`t", "`ta", "`tas", "`task"] {
            let parsed = parse(format!("{opener}\n `= created now\n"));
            assert_eq!(
                folding_ranges(&parsed.source, &parsed.syntax, None, None, false)
                    .iter()
                    .map(|range| (range.start_line, range.end_line))
                    .collect::<Vec<_>>(),
                [(0, 1)],
                "fold changed for {opener:?}"
            );
        }
    }

    #[test]
    fn layers_nested_subtree_folds_without_extending_leaf_layout() {
        let parsed = parse("`task Parent\n\n `@ parent\n\n `task Leaf\n\n  `@ leaf\n\n`- Next\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        assert_eq!(
            folding_ranges(&parsed.source, &parsed.syntax, None, None, false)
                .iter()
                .map(|range| (range.start_line, range.end_line))
                .collect::<Vec<_>>(),
            [(0, 6), (4, 6)]
        );
    }

    #[test]
    fn folds_separator_between_same_marker_blocks_but_preserves_changed_marker_boundary() {
        let parsed = parse("`task First\n\n detail\n\n`task Second\n\n detail\n\n`- Regular\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        assert_eq!(
            folding_ranges(&parsed.source, &parsed.syntax, None, None, false)
                .iter()
                .map(|range| (range.start_line, range.end_line))
                .collect::<Vec<_>>(),
            [(0, 3), (4, 6)]
        );
    }

    #[test]
    fn closed_task_tokens_preserve_nested_task_states() {
        let parsed = parse(
            "`- Closed parent\n\n `+ task\n\n `= done 2026-07-27T10:00:00+08:00\n\n `note Parent detail\n\n `- Open child\n\n  `+ task\n\n `note Parent tail\n\n`- Canceled\n\n `+ task\n\n `= canceled 2026-07-27T10:01:00+08:00\n\n`- Conflicted\n\n `+ task\n\n `= done 2026-07-27T10:02:00+08:00\n\n `= canceled 2026-07-27T10:03:00+08:00\n",
        );
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let tasks = analyze_tasks(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        )
        .tasks;
        let ranges = closed_task_token_ranges(&tasks);
        let open_child = &tasks.get(1).unwrap().range;
        assert!(ranges
            .iter()
            .all(|(range, _)| { range.end <= open_child.start || range.start >= open_child.end }));
        assert!(ranges.iter().any(|(_, modifiers)| *modifiers == 1));
        assert!(ranges.iter().any(|(_, modifiers)| *modifiers == 2));
        assert!(ranges.iter().any(|(_, modifiers)| *modifiers == 3));
    }

    #[test]
    fn maps_metadata_facts_to_nested_symbols() {
        let parsed = parse("`= title Document title\n`= author\n `= name Alice\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_metadata(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        let symbol = metadata_symbol(
            &PositionIndex::new(&parsed.source),
            output.metadata.as_ref().unwrap(),
        );
        assert_eq!(symbol.name, "metadata");
        let children = symbol.children.unwrap();
        assert_eq!(children[0].name, "title");
        assert_eq!(children[0].detail.as_deref(), Some("Document title"));
        assert_eq!(children[1].children.as_ref().unwrap()[0].name, "name");
    }

    #[test]
    fn hover_preview_fence_exceeds_source_backtick_runs() {
        let preview = fenced_plumb("before ```` after");
        assert!(preview.starts_with("`````plumb\n"));
        assert!(preview.ends_with("\n`````"));
    }

    #[test]
    fn splits_multiline_semantic_ranges_at_crlf_boundaries() {
        let source = "before `-{\r\n   .任务\r\n  } Head";
        let start = source.find('`').unwrap();
        let end = source.find('}').unwrap() + 1;
        let segments = physical_line_ranges(source, &(start..end))
            .into_iter()
            .map(|range| &source[range])
            .collect::<Vec<_>>();
        assert_eq!(segments, ["`-{", ".任务", "}"]);
    }

    #[test]
    fn structured_search_reports_internal_location_failures() {
        let mut workspace = Workspace::new();
        workspace.insert("relative.plumb", 1, "Relative\n");
        let error = search_workspace(
            &workspace,
            &[],
            true,
            SearchParams {
                kind: Some(SearchKind::Note),
                query: String::new(),
                filter: None,
                limit: Some(20),
            },
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::INTERNAL_ERROR);
    }
}
