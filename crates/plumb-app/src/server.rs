use std::collections::{HashMap, HashSet};
use std::fs;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

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
    ResourceOperationKind, SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens,
    SemanticTokensFullOptions, SemanticTokensLegend, SemanticTokensOptions, SemanticTokensParams,
    SemanticTokensResult, SemanticTokensServerCapabilities, ServerCapabilities, SymbolInformation,
    SymbolKind, TextDocumentEdit, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextEdit as LspTextEdit, Url, WatchKind, WorkDoneProgress, WorkDoneProgressBegin,
    WorkDoneProgressEnd, WorkDoneProgressOptions, WorkDoneProgressReport,
    WorkspaceEdit as LspWorkspaceEdit, WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use plumb_semantics::{
    attribute_completion_context, citation_completion_context, construct_completion_context,
    event_title_completion_context, file_completion_context, image_completion_context,
    link_completion_context, task_dependency_completion_context, AnchorKind,
    ConstructCompletionContext, TaskStatus,
};
use plumb_syntax::Diagnostic;
use plumb_workspace::{
    load_bibliography, normalize, scan_workspace_files, Bibliography, BibliographyResolution,
    PathRenameInput, RenameError, ResolvedTarget, ResourceOperation, SearchRecord,
    SearchRecordKind, SqliteSemanticStore, Workspace, WorkspaceEdit,
};
use sha2::{Digest, Sha256};

use crate::folding::{collapsed_text_labels as fold_labels, ranges as folding_ranges};
#[cfg(test)]
use crate::hover::fenced_plumb;
use crate::hover::{
    event as event_hover, file as file_hover, image as image_hover, link as link_hover,
    target as target_hover, task as task_hover,
};
use crate::position::{byte_range_to_lsp, position_to_offset};
use crate::search::{SearchItem, SearchKind, SearchParams, SearchProvenance, SearchResult};
use crate::semantic_tokens::{closed_task_token_ranges, physical_line_ranges};
use crate::symbols::{
    anchor as anchor_symbol, events as event_symbols, heading as heading_symbol,
    insert as insert_document_symbol, metadata as metadata_symbol, tasks as task_symbols,
};

pub(crate) struct ServerState {
    client: ClientSocket,
    workspace: Workspace,
    open_documents: HashMap<Url, PathBuf>,
    roots: Vec<PathBuf>,
    supports_document_changes: bool,
    supports_resource_rename: bool,
    supports_dynamic_watching: bool,
    supports_completion_snippets: bool,
    completion_indentation: CompletionIndentation,
    supports_code_lens_refresh: bool,
    folding_range_limit: Option<usize>,
    supports_folding_collapsed_text: bool,
    line_folding_only: bool,
    index_complete: bool,
    index_generation: u64,
    pending_path_renames: Vec<PendingPathRename>,
}

pub(crate) struct InitialIndexResult {
    generation: u64,
    workspace: Workspace,
    indexed: usize,
    cache_hits: usize,
    complete: bool,
}

struct PendingPathRename {
    old_path: PathBuf,
    new_path: PathBuf,
    old_removed: bool,
    new_seen: bool,
}

impl ServerState {
    pub(crate) fn new(client: ClientSocket) -> Self {
        Self {
            client,
            workspace: Workspace::new(),
            open_documents: HashMap::new(),
            roots: Vec::new(),
            supports_document_changes: false,
            supports_resource_rename: false,
            supports_dynamic_watching: false,
            supports_completion_snippets: false,
            completion_indentation: CompletionIndentation::default(),
            supports_code_lens_refresh: false,
            folding_range_limit: None,
            supports_folding_collapsed_text: false,
            line_folding_only: false,
            index_complete: false,
            index_generation: 0,
            pending_path_renames: Vec::new(),
        }
    }

    fn update(&mut self, uri: Url, version: i32, text: String) {
        let Ok(path) = uri.to_file_path() else {
            return;
        };
        let path = normalize(&path);
        let revision = i64::from(version);
        if !self
            .workspace
            .rebind_revision_if_source(&path, revision, &text)
        {
            self.workspace.insert(&path, revision, text);
        }
        self.open_documents.insert(uri, path);
        self.publish_all_open_diagnostics();
        self.refresh_code_lenses();
    }

    fn publish_all_open_diagnostics(&self) {
        for (uri, path) in &self.open_documents {
            self.publish(uri, path);
        }
    }

    fn publish(&self, uri: &Url, path: &Path) {
        let Some(entry) = self.workspace.get(path) else {
            return;
        };
        let mut diagnostics = self.workspace.diagnostics(path);
        if let Some(bibliography) = self.bibliography_for(path) {
            diagnostics.extend(bibliography.diagnostics.clone());
            if let Some(current) = &entry.current {
                diagnostics
                    .extend(bibliography.citation_diagnostics(&current.output.citations.citations));
            }
        }
        let diagnostics = diagnostics
            .into_iter()
            .map(|diagnostic| to_lsp_diagnostic(&entry.parsed.source, uri, diagnostic))
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
        if let Some(current) = &entry.current {
            Some(load_bibliography(root, path, &current.output.metadata))
        } else {
            let metadata = plumb_semantics::analyze_metadata(&entry.parsed.syntax);
            Some(load_bibliography(root, path, &metadata))
        }
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
        let stale = self
            .workspace
            .document_paths()
            .into_iter()
            .filter(|path| {
                self.roots.iter().any(|root| path.starts_with(root))
                    && !open.contains(path)
                    && !retained.contains(path)
            })
            .collect::<Vec<_>>();
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
            .filter_map(|path| {
                let entry = self.workspace.get(path)?;
                Some((path.clone(), entry.revision, entry.parsed.source.clone()))
            })
            .collect::<Vec<_>>();
        self.workspace = result.workspace;
        for (path, revision, source) in open {
            self.workspace.insert(path, revision, source);
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

    fn refresh_code_lenses(&self) {
        if !self.supports_code_lens_refresh {
            return;
        }
        let mut client = self.client.clone();
        tokio::spawn(async move {
            let _ = client.code_lens_refresh(()).await;
        });
    }

    fn begin_path_rename(&mut self, old_path: PathBuf, new_path: PathBuf) {
        let snapshot = self
            .workspace
            .get(&old_path)
            .map(|entry| (entry.revision, entry.parsed.source.clone()))
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

    fn reference_target_at(&self, path: &Path, offset: usize) -> Option<ResolvedTarget> {
        self.workspace.reference_target_at(path, offset)
    }

    fn target_at(&self, path: &Path, offset: usize) -> Option<ResolvedTarget> {
        self.workspace.target_at(path, offset)
    }

    fn target_at_with_lazy_load(&mut self, path: &Path, offset: usize) -> Option<ResolvedTarget> {
        let target = self.target_at(path, offset)?;
        if !self.load_unresolved_target(&target) {
            return Some(target);
        }
        self.target_at(path, offset)
    }

    fn reference_target_at_with_lazy_load(
        &mut self,
        path: &Path,
        offset: usize,
    ) -> Option<ResolvedTarget> {
        let target = self.reference_target_at(path, offset)?;
        if !self.load_unresolved_target(&target) {
            return Some(target);
        }
        self.reference_target_at(path, offset)
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
        .map_err(|message| ResponseError::new(ErrorCode::INVALID_PARAMS, message))?;
    let items = results
        .items
        .into_iter()
        .map(|record| search_item(workspace, record))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SearchResult {
        schema_version: 3,
        items,
        complete: index_complete && results.complete,
    })
}

fn search_item(workspace: &Workspace, record: SearchRecord) -> Result<SearchItem, ResponseError> {
    let disk_source;
    let source = if let Some(entry) = workspace.get(&record.path) {
        &entry.parsed.source
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
                        TextDocumentSyncKind::FULL,
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
                            "t".to_string(),
                            "e".to_string(),
                            "-".to_string(),
                            ">".to_string(),
                            "[".to_string(),
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
        self.update(document.uri, document.version, document.text);
        ControlFlow::Continue(())
    }

    fn did_change(&mut self, params: DidChangeTextDocumentParams) -> Self::NotifyResult {
        if let Some(change) = params.content_changes.into_iter().last() {
            self.update(
                params.text_document.uri,
                params.text_document.version,
                change.text,
            );
        }
        ControlFlow::Continue(())
    }

    fn did_close(&mut self, params: DidCloseTextDocumentParams) -> Self::NotifyResult {
        let uri = params.text_document.uri;
        if let Some(path) = self.open_documents.remove(&uri) {
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
                let mut symbols = current
                    .output
                    .headings
                    .headings
                    .iter()
                    .map(|heading| heading_symbol(&entry.parsed.source, heading))
                    .collect::<Vec<_>>();
                let mut additional = current
                    .output
                    .anchors
                    .iter()
                    .filter(|anchor| {
                        anchor.kind != AnchorKind::Heading
                            && !current
                                .output
                                .tasks
                                .tasks
                                .iter()
                                .any(|task| task.range == anchor.range)
                            && !current
                                .output
                                .events
                                .events
                                .iter()
                                .any(|event| event.range == anchor.range)
                    })
                    .map(|anchor| {
                        (
                            anchor.range.start,
                            anchor_symbol(&entry.parsed.source, anchor),
                        )
                    })
                    .collect::<Vec<_>>();
                if let Some(metadata) = &current.output.metadata.metadata {
                    additional.push((
                        metadata.range.start,
                        metadata_symbol(&entry.parsed.source, metadata),
                    ));
                }
                additional.extend(
                    current
                        .output
                        .tasks
                        .tasks
                        .iter()
                        .filter(|task| task.depth == 0)
                        .map(|task| task.range.start)
                        .zip(task_symbols(
                            &entry.parsed.source,
                            &current.output.tasks.tasks,
                        )),
                );
                additional.extend(
                    current
                        .output
                        .events
                        .events
                        .iter()
                        .filter(|event| event.depth == 0)
                        .map(|event| event.range.start)
                        .zip(event_symbols(
                            &entry.parsed.source,
                            &current.output.events.events,
                        )),
                );
                additional.sort_by_key(|(start, _)| *start);
                for (_, symbol) in additional {
                    insert_document_symbol(&mut symbols, symbol);
                }
                symbols
            });
        Box::pin(async move { Ok(symbols.map(DocumentSymbolResponse::Nested)) })
    }

    fn folding_range(
        &mut self,
        params: FoldingRangeParams,
    ) -> BoxFuture<'static, Result<Option<Vec<FoldingRange>>, Self::Error>> {
        let ranges = params
            .text_document
            .uri
            .to_file_path()
            .ok()
            .and_then(|path| self.workspace.get(path))
            .map(|entry| {
                let labels = self
                    .supports_folding_collapsed_text
                    .then(|| fold_labels(&self.workspace, &entry.path, entry));
                folding_ranges(
                    &entry.parsed.source,
                    entry.parsed.recovered_syntax(),
                    self.folding_range_limit,
                    labels.as_ref(),
                    self.line_folding_only,
                )
            });
        Box::pin(async move { Ok(ranges) })
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
                let source = &entry.parsed.source;
                let edits =
                    plumb_edit::format(&entry.parsed, plumb_edit::FormatScope::Document).ok()?;
                Some(
                    edits
                        .into_iter()
                        .map(|edit| {
                            LspTextEdit::new(byte_range_to_lsp(source, &edit.range), edit.new_text)
                        })
                        .collect(),
                )
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
                let source = &entry.parsed.source;
                let selection = position_to_offset(source, params.range.start)
                    ..position_to_offset(source, params.range.end);
                let edits = plumb_edit::format(
                    &entry.parsed,
                    plumb_edit::FormatScope::ContainedBlocks(selection),
                )
                .ok()?;
                Some(
                    edits
                        .into_iter()
                        .map(|edit| {
                            LspTextEdit::new(byte_range_to_lsp(source, &edit.range), edit.new_text)
                        })
                        .collect(),
                )
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
        }
        let location = position
            .text_document
            .uri
            .to_file_path()
            .ok()
            .and_then(|path| {
                let entry = self.workspace.get(&path)?;
                let offset = position_to_offset(&entry.parsed.source, position.position);
                if let Some(citation) = entry.current.as_ref().and_then(|current| {
                    current.output.citations.citations.iter().find(|citation| {
                        citation.selection_range.start <= offset
                            && offset <= citation.selection_range.end
                    })
                }) {
                    let bibliography = self.bibliography_for(&path)?;
                    let BibliographyResolution::Resolved(record) =
                        bibliography.resolve(&citation.id)
                    else {
                        return None;
                    };
                    let source = std::fs::read_to_string(&record.path).ok()?;
                    return Some(Location::new(
                        Url::from_file_path(&record.path).ok()?,
                        byte_range_to_lsp(&source, &record.range),
                    ));
                }
                match self.target_at_with_lazy_load(&path, offset)? {
                    ResolvedTarget::Anchor { path, anchor, .. } => {
                        location_for(&self.workspace, &path, &anchor.selection_range)
                    }
                    ResolvedTarget::Document { path } => {
                        location_for(&self.workspace, &path, &(0..0))
                    }
                    ResolvedTarget::File { path } => Some(Location::new(
                        Url::from_file_path(path).ok()?,
                        lsp_types::Range::default(),
                    )),
                    _ => None,
                }
            });
        Box::pin(async move { Ok(location.map(GotoDefinitionResponse::Scalar)) })
    }

    fn references(
        &mut self,
        params: ReferenceParams,
    ) -> BoxFuture<'static, Result<Option<Vec<Location>>, Self::Error>> {
        let position = params.text_document_position;
        if let Ok(path) = position.text_document.uri.to_file_path() {
            self.ensure_request_document(&path);
        }
        let locations = position
            .text_document
            .uri
            .to_file_path()
            .ok()
            .and_then(|path| {
                let entry = self.workspace.get(&path)?;
                let offset = position_to_offset(&entry.parsed.source, position.position);
                match self.target_at_with_lazy_load(&path, offset)? {
                    ResolvedTarget::Anchor {
                        path: target_path,
                        id,
                        anchor,
                    } => {
                        let mut locations = self
                            .workspace
                            .references_to(&target_path, &id)
                            .into_iter()
                            .filter_map(|(source_path, reference)| {
                                location_for(&self.workspace, &source_path, &reference.source_range)
                            })
                            .collect::<Vec<_>>();
                        if params.context.include_declaration {
                            if let Some(declaration) =
                                location_for(&self.workspace, &target_path, &anchor.selection_range)
                            {
                                locations.insert(0, declaration);
                            }
                        }
                        Some(locations)
                    }
                    ResolvedTarget::Document { path: target_path } => {
                        let mut locations = self
                            .workspace
                            .references_to_document(&target_path)
                            .into_iter()
                            .filter_map(|(source_path, reference)| {
                                location_for(&self.workspace, &source_path, &reference.source_range)
                            })
                            .collect::<Vec<_>>();
                        if params.context.include_declaration {
                            if let Some(metadata) = self
                                .workspace
                                .get(&target_path)
                                .and_then(|entry| entry.current.as_ref())
                                .and_then(|current| current.output.metadata.metadata.as_ref())
                            {
                                if let Some(declaration) = location_for(
                                    &self.workspace,
                                    &target_path,
                                    &metadata.selection_range,
                                ) {
                                    locations.insert(0, declaration);
                                }
                            }
                        }
                        Some(locations)
                    }
                    _ => None,
                }
            });
        Box::pin(async move { Ok(locations) })
    }

    fn code_lens(
        &mut self,
        params: CodeLensParams,
    ) -> BoxFuture<'static, Result<Option<Vec<CodeLens>>, Self::Error>> {
        if let Ok(path) = params.text_document.uri.to_file_path() {
            self.ensure_request_document(&path);
        }
        let lenses = params
            .text_document
            .uri
            .to_file_path()
            .ok()
            .and_then(|path| {
                let entry = self.workspace.get(&path)?;
                let output = entry.current.as_ref()?;
                let uri = Url::from_file_path(&entry.path).ok()?;
                let anchor_ids = output
                    .output
                    .anchors
                    .iter()
                    .map(|anchor| anchor.id.value.clone())
                    .collect::<HashSet<_>>();
                let mut references = self
                    .workspace
                    .reverse_references_for_document(&entry.path, &anchor_ids);
                let mut lenses = Vec::new();
                if let Some(metadata) = &output.output.metadata.metadata {
                    let locations = references
                        .document
                        .into_iter()
                        .filter_map(|reference| {
                            location_for(
                                &self.workspace,
                                &reference.source_path,
                                &reference.source_range,
                            )
                        })
                        .collect::<Vec<_>>();
                    let count = locations.len();
                    let title = if count == 1 {
                        "1 file reference".to_string()
                    } else {
                        format!("{count} file references")
                    };
                    lenses.push(reference_code_lens(
                        &entry.parsed.source,
                        &uri,
                        &metadata.selection_range,
                        title,
                        locations,
                    ));
                }
                lenses.extend(output.output.anchors.iter().map(|anchor| {
                    let locations = references
                        .anchors
                        .remove(&anchor.id.value)
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|reference| {
                            location_for(
                                &self.workspace,
                                &reference.source_path,
                                &reference.source_range,
                            )
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
                    reference_code_lens(&entry.parsed.source, &uri, &lens_range, title, locations)
                }));
                Some(lenses)
            });
        Box::pin(async move { Ok(lenses) })
    }

    fn hover(
        &mut self,
        params: HoverParams,
    ) -> BoxFuture<'static, Result<Option<Hover>, Self::Error>> {
        let position = params.text_document_position_params;
        if let Ok(path) = position.text_document.uri.to_file_path() {
            self.ensure_request_document(&path);
        }
        let hover = position
            .text_document
            .uri
            .to_file_path()
            .ok()
            .and_then(|path| {
                let offset = {
                    let entry = self.workspace.get(&path)?;
                    position_to_offset(&entry.parsed.source, position.position)
                };
                if let Some(citation) =
                    self.workspace
                        .get(&path)?
                        .current
                        .as_ref()
                        .and_then(|current| {
                            current.output.citations.citations.iter().find(|citation| {
                                citation.selection_range.start <= offset
                                    && offset <= citation.selection_range.end
                            })
                        })
                {
                    let bibliography = self.bibliography_for(&path)?;
                    let BibliographyResolution::Resolved(record) =
                        bibliography.resolve(&citation.id)
                    else {
                        return None;
                    };
                    let entry = self.workspace.get(&path)?;
                    return Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: format!("**Citation:** `{}`\n\n{}", record.id, record.detail()),
                        }),
                        range: Some(byte_range_to_lsp(
                            &entry.parsed.source,
                            &citation.selection_range,
                        )),
                    });
                }
                if let Some(file) = self.workspace.file_at(&path, offset).cloned() {
                    let target = self.workspace.resolve_file(&path, &file);
                    let entry = self.workspace.get(&path)?;
                    return Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: file_hover(&target, &file),
                        }),
                        range: Some(byte_range_to_lsp(
                            &entry.parsed.source,
                            &file.selection_range,
                        )),
                    });
                }
                if let Some(image) = self.workspace.image_at(&path, offset).cloned() {
                    let target = self.workspace.resolve_image(&path, &image);
                    let entry = self.workspace.get(&path)?;
                    return Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: image_hover(&target, &image),
                        }),
                        range: Some(byte_range_to_lsp(
                            &entry.parsed.source,
                            &image.selection_range,
                        )),
                    });
                }
                if let Some(link) = self.workspace.link_at(&path, offset).cloned() {
                    let target = self.workspace.resolve_link(&path, &link);
                    if matches!(target, ResolvedTarget::External | ResolvedTarget::Other) {
                        let entry = self.workspace.get(&path)?;
                        return Some(Hover {
                            contents: HoverContents::Markup(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: link_hover(&target, &link),
                            }),
                            range: Some(byte_range_to_lsp(
                                &entry.parsed.source,
                                &link.selection_range,
                            )),
                        });
                    }
                }
                if let Some(target) = self.reference_target_at_with_lazy_load(&path, offset) {
                    let message = target_hover(&self.workspace, &target);
                    return Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: message,
                        }),
                        range: None,
                    });
                }
                if let Some(task) = self.workspace.task_at(&path, offset).cloned() {
                    let entry = self.workspace.get(&path)?;
                    return Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: task_hover(&self.workspace, &path, &task),
                        }),
                        range: Some(byte_range_to_lsp(
                            &entry.parsed.source,
                            &task.selection_range,
                        )),
                    });
                }
                if let Some(event) = self.workspace.event_at(&path, offset).cloned() {
                    let entry = self.workspace.get(&path)?;
                    return Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: event_hover(&event),
                        }),
                        range: Some(byte_range_to_lsp(
                            &entry.parsed.source,
                            &event.selection_range,
                        )),
                    });
                }
                let target = self.target_at(&path, offset)?;
                let message = target_hover(&self.workspace, &target);
                Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: message,
                    }),
                    range: None,
                })
            });
        Box::pin(async move { Ok(hover) })
    }

    fn completion(
        &mut self,
        params: CompletionParams,
    ) -> BoxFuture<'static, Result<Option<CompletionResponse>, Self::Error>> {
        let position = params.text_document_position;
        let items = position
            .text_document
            .uri
            .to_file_path()
            .ok()
            .and_then(|path| {
                let entry = self.workspace.get(&path)?;
                let offset = position_to_offset(&entry.parsed.source, position.position);
                if let Some(context) = construct_completion_context(&entry.parsed, offset) {
                    let timestamp = Local::now().to_rfc3339_opts(SecondsFormat::Secs, false);
                    let include_link_labels =
                        matches!(context, ConstructCompletionContext::Link { .. });
                    let mut items = construct_completion_items(
                        &entry.parsed.source,
                        context,
                        self.supports_completion_snippets,
                        self.completion_indentation,
                        &timestamp,
                    );
                    if include_link_labels {
                        if let Some(context) = link_completion_context(&entry.parsed, offset) {
                            items.extend(
                                self.workspace
                                    .complete_link(&path, &context)
                                    .into_iter()
                                    .map(|candidate| CompletionItem {
                                        label: candidate.label,
                                        kind: Some(CompletionItemKind::FILE),
                                        detail: Some(candidate.detail),
                                        text_edit: Some(CompletionTextEdit::Edit(
                                            LspTextEdit::new(
                                                byte_range_to_lsp(
                                                    &entry.parsed.source,
                                                    &candidate.replace,
                                                ),
                                                candidate.new_text,
                                            ),
                                        )),
                                        ..CompletionItem::default()
                                    }),
                            );
                        }
                    }
                    return Some(items);
                }
                if let Some(context) = citation_completion_context(&entry.parsed, offset) {
                    let bibliography = self.bibliography_for(&path)?;
                    let query = context.query.to_lowercase();
                    return Some(
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
                                    byte_range_to_lsp(&entry.parsed.source, &context.replace),
                                    record.id.clone(),
                                ))),
                                ..CompletionItem::default()
                            })
                            .collect(),
                    );
                }
                if let Some(context) = task_dependency_completion_context(&entry.parsed, offset) {
                    let candidates = self.workspace.complete_task_dependency(&path, &context);
                    return Some(
                        candidates
                            .into_iter()
                            .map(|candidate| CompletionItem {
                                kind: Some(if candidate.new_text.ends_with('#') {
                                    CompletionItemKind::FILE
                                } else {
                                    CompletionItemKind::REFERENCE
                                }),
                                label: candidate.label,
                                detail: Some(candidate.detail),
                                text_edit: Some(CompletionTextEdit::Edit(LspTextEdit::new(
                                    byte_range_to_lsp(&entry.parsed.source, &candidate.replace),
                                    candidate.new_text,
                                ))),
                                ..CompletionItem::default()
                            })
                            .collect(),
                    );
                }
                if let Some(context) = event_title_completion_context(&entry.parsed, offset) {
                    return Some(
                        self.workspace
                            .complete_event_title(&context)
                            .into_iter()
                            .map(|candidate| CompletionItem {
                                label: candidate.label,
                                kind: Some(CompletionItemKind::VALUE),
                                detail: Some(candidate.detail),
                                text_edit: Some(CompletionTextEdit::Edit(LspTextEdit::new(
                                    byte_range_to_lsp(&entry.parsed.source, &candidate.replace),
                                    candidate.new_text,
                                ))),
                                ..CompletionItem::default()
                            })
                            .collect(),
                    );
                }
                if let Some(context) = attribute_completion_context(&entry.parsed, offset) {
                    if !context.completions.is_empty() {
                        return Some(
                            context
                                .completions
                                .into_iter()
                                .map(|candidate| CompletionItem {
                                    label: candidate.label.to_string(),
                                    kind: Some(CompletionItemKind::PROPERTY),
                                    detail: Some(candidate.detail.to_string()),
                                    insert_text_format: Some(
                                        if self.supports_completion_snippets {
                                            InsertTextFormat::SNIPPET
                                        } else {
                                            InsertTextFormat::PLAIN_TEXT
                                        },
                                    ),
                                    text_edit: Some(CompletionTextEdit::Edit(LspTextEdit::new(
                                        byte_range_to_lsp(&entry.parsed.source, &context.replace),
                                        attribute_completion_text(
                                            candidate.new_text,
                                            self.supports_completion_snippets,
                                        ),
                                    ))),
                                    ..CompletionItem::default()
                                })
                                .collect(),
                        );
                    }
                }
                let (candidates, kind) =
                    if let Some(context) = link_completion_context(&entry.parsed, offset) {
                        let kind = if matches!(
                            &context,
                            plumb_semantics::LinkCompletionContext::Label { .. }
                                | plumb_semantics::LinkCompletionContext::Path { .. }
                                | plumb_semantics::LinkCompletionContext::AutolinkPath { .. }
                        ) {
                            CompletionItemKind::FILE
                        } else {
                            CompletionItemKind::REFERENCE
                        };
                        (self.workspace.complete_link(&path, &context), kind)
                    } else if let Some(context) = image_completion_context(&entry.parsed, offset) {
                        (
                            self.workspace.complete_image_path(&path, &context),
                            CompletionItemKind::FILE,
                        )
                    } else {
                        let context = file_completion_context(&entry.parsed, offset)?;
                        (
                            self.workspace.complete_file_path(&path, &context),
                            CompletionItemKind::FILE,
                        )
                    };
                Some(
                    candidates
                        .into_iter()
                        .map(|candidate| CompletionItem {
                            label: candidate.label,
                            kind: Some(kind),
                            detail: Some(candidate.detail),
                            text_edit: Some(CompletionTextEdit::Edit(LspTextEdit::new(
                                byte_range_to_lsp(&entry.parsed.source, &candidate.replace),
                                candidate.new_text,
                            ))),
                            ..CompletionItem::default()
                        })
                        .collect::<Vec<_>>(),
                )
            });
        Box::pin(async move { Ok(items.map(CompletionResponse::Array)) })
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
                let offset = position_to_offset(&entry.parsed.source, params.range.start);
                let selection_end = position_to_offset(&entry.parsed.source, params.range.end);
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
                let offset = position_to_offset(&entry.parsed.source, params.range.start);
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
        let tokens = params
            .text_document
            .uri
            .to_file_path()
            .ok()
            .and_then(|path| self.workspace.get(path))
            .and_then(|entry| entry.current.as_ref().map(|current| (entry, current)))
            .map(|(entry, current)| {
                let mut previous_line = 0;
                let mut previous_start = 0;
                let data = closed_task_token_ranges(&current.output.tasks.tasks)
                    .into_iter()
                    .flat_map(|(byte_range, modifiers)| {
                        physical_line_ranges(&entry.parsed.source, &byte_range)
                            .into_iter()
                            .map(move |range| (range, modifiers))
                    })
                    .map(|(byte_range, token_modifiers_bitset)| {
                        let range = byte_range_to_lsp(&entry.parsed.source, &byte_range);
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
                SemanticTokensResult::Tokens(SemanticTokens {
                    result_id: None,
                    data,
                })
            });
        Box::pin(async move { Ok(tokens) })
    }

    fn prepare_rename(
        &mut self,
        params: lsp_types::TextDocumentPositionParams,
    ) -> BoxFuture<'static, Result<Option<PrepareRenameResponse>, Self::Error>> {
        if let Ok(path) = params.text_document.uri.to_file_path() {
            self.ensure_request_document(&path);
        }
        let response = params
            .text_document
            .uri
            .to_file_path()
            .ok()
            .and_then(|path| {
                let entry = self.workspace.get(&path)?;
                let offset = position_to_offset(&entry.parsed.source, params.position);
                let (range, placeholder) = self
                    .workspace
                    .anchor_rename_target_at(&path, offset)
                    .map(|target| (target.range, target.id))
                    .or_else(|_| {
                        self.workspace
                            .document_rename_target_at(&path, offset)
                            .map(|target| {
                                let (name, fallback) = match target.input {
                                    PathRenameInput::Path => {
                                        (target.old_path.file_name(), "document.plumb")
                                    }
                                    PathRenameInput::FileStem => {
                                        (target.old_path.file_stem(), "document")
                                    }
                                };
                                let placeholder = name
                                    .and_then(|name| name.to_str())
                                    .unwrap_or(fallback)
                                    .to_string();
                                (target.range, placeholder)
                            })
                    })
                    .ok()?;
                Some(PrepareRenameResponse::RangeWithPlaceholder {
                    range: byte_range_to_lsp(&entry.parsed.source, &range),
                    placeholder,
                })
            });
        Box::pin(async move { Ok(response) })
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
            let offset =
                position_to_offset(&entry.parsed.source, params.text_document_position.position);
            if let Ok(target) = self.workspace.anchor_rename_target_at(&path, offset) {
                let edit = self
                    .workspace
                    .rename_anchor(&target, &params.new_name)
                    .map_err(|error| {
                        rename_request_error(format!("anchor rename failed: {error:?}"))
                    })?;
                let edit = workspace_edit_to_lsp(&self.workspace, edit)
                    .ok_or_else(|| rename_request_error("cannot map anchor rename edit"))?;
                return Ok(Some(edit));
            }
            let Ok(target) = self.workspace.document_rename_target_at(&path, offset) else {
                return Ok(None);
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

fn attribute_completion_text(text: &str, snippets: bool) -> String {
    if !snippets {
        return text.to_string();
    }
    if let Some(prefix) = text.strip_suffix(" ]") {
        format!("{prefix} ${{1}}]")
    } else if let Some(prefix) = text.strip_suffix("|]") {
        format!("{prefix}|${{1}}]")
    } else if let Some(prefix) = text.strip_suffix("[]") {
        format!("{prefix}[${{1}}]")
    } else if text == "`= priority 0" {
        "`= priority ${1:0}".to_string()
    } else if text.ends_with(' ') {
        format!("{text}${{1}}")
    } else {
        text.to_string()
    }
}

fn rename_request_error(message: impl Into<String>) -> ResponseError {
    ResponseError::new(ErrorCode::REQUEST_FAILED, message.into())
}

fn document_rename_error(error: RenameError) -> ResponseError {
    let message = match error {
        RenameError::TargetExists => "document rename target already exists".to_string(),
        error => format!("document rename failed: {error:?}"),
    };
    rename_request_error(message)
}

struct ConstructTemplate {
    label: &'static str,
    detail: &'static str,
    snippet: String,
    plain: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompletionIndentationProjection {
    AsIs,
    AdjustIndentation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CompletionIndentation {
    projection: CompletionIndentationProjection,
    item_mode: Option<InsertTextMode>,
}

impl Default for CompletionIndentation {
    fn default() -> Self {
        Self {
            projection: CompletionIndentationProjection::AdjustIndentation,
            item_mode: None,
        }
    }
}

fn completion_indentation(
    capabilities: Option<&lsp_types::CompletionClientCapabilities>,
) -> CompletionIndentation {
    let Some(capabilities) = capabilities else {
        return CompletionIndentation::default();
    };
    let supported = capabilities
        .completion_item
        .as_ref()
        .and_then(|item| item.insert_text_mode_support.as_ref())
        .map(|support| support.value_set.as_slice())
        .unwrap_or_default();

    if supported.contains(&InsertTextMode::AS_IS) {
        CompletionIndentation {
            projection: CompletionIndentationProjection::AsIs,
            item_mode: Some(InsertTextMode::AS_IS),
        }
    } else if supported.contains(&InsertTextMode::ADJUST_INDENTATION) {
        CompletionIndentation {
            projection: CompletionIndentationProjection::AdjustIndentation,
            item_mode: Some(InsertTextMode::ADJUST_INDENTATION),
        }
    } else if capabilities.insert_text_mode == Some(InsertTextMode::AS_IS) {
        CompletionIndentation {
            projection: CompletionIndentationProjection::AsIs,
            item_mode: None,
        }
    } else {
        CompletionIndentation::default()
    }
}

fn task_construct_template(block_indent: &str, timestamp: &str) -> ConstructTemplate {
    ConstructTemplate {
        label: "Task",
        detail: "plumb task list item",
        snippet: format!(
            "`task ${{1:Task}} {{\n{block_indent} `= created {timestamp}\n{block_indent}}}"
        ),
        plain: format!("`task  {{\n{block_indent} `= created {timestamp}\n{block_indent}}}"),
    }
}

fn construct_completion_items(
    source: &str,
    context: ConstructCompletionContext,
    snippets: bool,
    completion_indentation: CompletionIndentation,
    timestamp: &str,
) -> Vec<CompletionItem> {
    let task_completion = matches!(&context, ConstructCompletionContext::Task { .. });
    let block_indent = match (&context, completion_indentation.projection) {
        (
            ConstructCompletionContext::Task { replace }
            | ConstructCompletionContext::Event { replace },
            CompletionIndentationProjection::AsIs,
        ) => {
            let line_start = source[..replace.start]
                .rfind('\n')
                .map_or(0, |index| index + 1);
            source[line_start..replace.start].to_string()
        }
        _ => String::new(),
    };
    let (replace, templates) = match context {
        ConstructCompletionContext::Citation { replace } => (
            replace,
            vec![ConstructTemplate {
                label: "Citation",
                detail: "plumb citation",
                snippet: "`cite[${1:id}]".to_string(),
                plain: "`cite[]".to_string(),
            }],
        ),
        ConstructCompletionContext::Task { replace } => (
            replace,
            vec![task_construct_template(&block_indent, timestamp)],
        ),
        ConstructCompletionContext::Event { replace } => (
            replace,
            vec![ConstructTemplate {
                label: "Event",
                detail: "plumb event list item",
                snippet: "`event ${1:09:00} ${2:Event}".to_string(),
                plain: "`event ".to_string(),
            }],
        ),
        ConstructCompletionContext::LinkAndAutolink { replace } => (
            replace,
            vec![link_construct_template(), autolink_construct_template()],
        ),
        ConstructCompletionContext::Autolink { replace } => {
            (replace, vec![autolink_construct_template()])
        }
        ConstructCompletionContext::Link { replace } => (replace, vec![link_construct_template()]),
    };
    templates
        .into_iter()
        .map(|template| CompletionItem {
            label: template.label.to_string(),
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some(template.detail.to_string()),
            insert_text_format: Some(if snippets {
                InsertTextFormat::SNIPPET
            } else {
                InsertTextFormat::PLAIN_TEXT
            }),
            insert_text_mode: task_completion
                .then_some(completion_indentation.item_mode)
                .flatten(),
            text_edit: Some(CompletionTextEdit::Edit(LspTextEdit::new(
                byte_range_to_lsp(source, &replace),
                if snippets {
                    template.snippet
                } else {
                    template.plain
                },
            ))),
            ..CompletionItem::default()
        })
        .collect()
}

fn link_construct_template() -> ConstructTemplate {
    ConstructTemplate {
        label: "Link",
        detail: "plumb link",
        snippet: "`->[${1:label}|${2:target}]".to_string(),
        plain: "`->[|]".to_string(),
    }
}

fn autolink_construct_template() -> ConstructTemplate {
    ConstructTemplate {
        label: "Autolink",
        detail: "plumb autolink",
        snippet: "`->\"${1:path}\"".to_string(),
        plain: "`->\"[]\"".to_string(),
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
        &entry.parsed.source
    } else {
        disk_source = fs::read_to_string(path).ok()?;
        &disk_source
    };
    let uri = Url::from_file_path(path).ok()?;
    Some(Location::new(uri, byte_range_to_lsp(source, range)))
}

fn reference_code_lens(
    source: &str,
    uri: &Url,
    source_range: &std::ops::Range<usize>,
    title: String,
    locations: Vec<Location>,
) -> CodeLens {
    let range = byte_range_to_lsp(source, source_range);
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

fn workspace_edit_to_lsp(workspace: &Workspace, edit: WorkspaceEdit) -> Option<LspWorkspaceEdit> {
    let has_resource_operations = !edit.resource_operations.is_empty();
    let mut document_edits = Vec::new();
    for document in edit.document_changes {
        let disk_source;
        let source = if let Some(entry) = workspace.get(&document.path) {
            &entry.parsed.source
        } else {
            disk_source = fs::read_to_string(&document.path).ok()?;
            &disk_source
        };
        let uri = Url::from_file_path(&document.path).ok()?;
        let version = (document.expected_revision > 0)
            .then(|| i32::try_from(document.expected_revision).ok())
            .flatten();
        let edits = document
            .edits
            .into_iter()
            .map(|edit| {
                OneOf::Left(LspTextEdit::new(
                    byte_range_to_lsp(source, &edit.range),
                    edit.new_text,
                ))
            })
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
    let mut indexed = 0;
    let mut cache_hits = 0;
    for path in files {
        if let Ok(text) = fs::read_to_string(&path) {
            match workspace.insert_disk(path, 0, text) {
                Ok(hit) => cache_hits += usize::from(hit),
                Err(_) => complete = false,
            }
            indexed += 1;
        } else {
            complete = false;
        }
    }
    InitialIndexResult {
        generation,
        workspace,
        indexed,
        cache_hits,
        complete,
    }
}

fn semantic_cache_path(roots: &[PathBuf]) -> PathBuf {
    let base = std::env::var_os("PLUMB_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(|| std::env::temp_dir().join("plumb-cache"));
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

fn to_lsp_diagnostic(source: &str, uri: &Url, diagnostic: Diagnostic) -> LspDiagnostic {
    let related_information = (!diagnostic.related.is_empty()).then(|| {
        diagnostic
            .related
            .iter()
            .map(|range| DiagnosticRelatedInformation {
                location: Location::new(uri.clone(), byte_range_to_lsp(source, range)),
                message: "Related source location".to_string(),
            })
            .collect()
    });
    LspDiagnostic {
        range: byte_range_to_lsp(source, &diagnostic.range),
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

    use super::*;

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
    fn task_completion_projections_produce_the_same_source() {
        let timestamp = "2026-08-10T12:00:00+08:00";
        let absolute = task_construct_template(" ", timestamp).snippet;
        let relative = task_construct_template("", timestamp).snippet;
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

        assert_eq!(adjusted, absolute);
        assert!(absolute.contains("\n  `= created "));
        assert!(absolute.ends_with("\n }"));
        assert!(relative.contains("\n `= created "));
        assert!(relative.ends_with("\n}"));
    }

    #[test]
    fn maps_nested_heading_facts_to_nested_symbols() {
        let parsed = parse("`# One\n`## Two\n");
        let output = analyze_headings(&parsed.syntax);
        let symbols = output
            .headings
            .iter()
            .map(|heading| heading_symbol(&parsed.source, heading))
            .collect::<Vec<_>>();
        assert_eq!(symbols[0].name, "One");
        assert_eq!(symbols[0].children.as_ref().unwrap()[0].name, "Two");
    }

    #[test]
    fn folds_heading_sections_and_multiline_syntax_blocks() {
        let parsed = parse(
            "`# Top\n\nIntro.\n\n`## Child\n\n`div Details\n\n     body\n\n     `text\"\n       raw\n`# Next\n\nTail.\n",
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
    fn layers_owner_fold_without_extending_leaf_over_trailing_blank_lines() {
        let parsed = parse("`task Parent {\n}\n\n      `task Leaf {\n      }\n\n`- Next\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        assert_eq!(
            folding_ranges(&parsed.source, &parsed.syntax, None, None, false)
                .iter()
                .map(|range| (range.start_line, range.end_line))
                .collect::<Vec<_>>(),
            [(0, 4), (0, 1), (3, 4)]
        );
    }

    #[test]
    fn folds_separator_between_same_marker_tasks_but_preserves_changed_marker_boundary() {
        let parsed = parse(
            "`task First {\n}\n\n      detail\n\n`task Second {\n}\n\n      detail\n\n`- Regular\n",
        );
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        assert_eq!(
            folding_ranges(&parsed.source, &parsed.syntax, None, None, false)
                .iter()
                .map(|range| (range.start_line, range.end_line))
                .collect::<Vec<_>>(),
            [(0, 4), (0, 1), (5, 8), (5, 6)]
        );
    }

    #[test]
    fn closed_task_tokens_preserve_nested_task_states() {
        let parsed = parse(
            "`task Closed parent {\n  `= done 2026-07-27T10:00:00+08:00\n}\n\n      `note Parent detail\n\n      `task Open child {\n      }\n\n      `note Parent tail\n\n`task Canceled {\n  `= canceled 2026-07-27T10:01:00+08:00\n}\n`task Conflicted {\n  `= done 2026-07-27T10:02:00+08:00\n  `= canceled 2026-07-27T10:03:00+08:00\n}\n",
        );
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let tasks = analyze_tasks(&parsed.source, &parsed.syntax).tasks;
        let ranges = closed_task_token_ranges(&tasks);
        let open_child = &tasks[1].range;
        assert!(ranges
            .iter()
            .all(|(range, _)| { range.end <= open_child.start || range.start >= open_child.end }));
        assert!(ranges.iter().any(|(_, modifiers)| *modifiers == 1));
        assert!(ranges.iter().any(|(_, modifiers)| *modifiers == 2));
        assert!(ranges.iter().any(|(_, modifiers)| *modifiers == 3));
    }

    #[test]
    fn maps_metadata_facts_to_nested_symbols() {
        let parsed = parse(
            "`meta\n  `: title\n\n    Document title\n\n  `: author\n    `: name\n\n      Alice\n",
        );
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_metadata(&parsed.syntax);
        let symbol = metadata_symbol(&parsed.source, output.metadata.as_ref().unwrap());
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
