use std::collections::HashMap;
use std::fs;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use async_lsp::{ClientSocket, ErrorCode, LanguageClient, LanguageServer, ResponseError};
use chrono::{Local, SecondsFormat};
use futures::future::BoxFuture;
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOptions, CodeActionOrCommand, CodeActionParams,
    CodeActionProviderCapability, CodeActionResponse, CompletionItem, CompletionItemKind,
    CompletionOptions, CompletionParams, CompletionResponse, CompletionTextEdit,
    Diagnostic as LspDiagnostic, DiagnosticRelatedInformation, DiagnosticSeverity,
    DidChangeConfigurationParams, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentChangeOperation, DocumentChanges, DocumentSymbol, DocumentSymbolParams,
    DocumentSymbolResponse, FileChangeType, FileSystemWatcher, GlobPattern, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverContents, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, InitializedParams, Location, MarkupContent, MarkupKind,
    NumberOrString, OneOf, OptionalVersionedTextDocumentIdentifier, PrepareRenameResponse,
    ProgressParams, ProgressParamsValue, PublishDiagnosticsParams, ReferenceParams, Registration,
    RegistrationParams, RenameFile, RenameFileOptions, RenameOptions, RenameParams, ResourceOp,
    ResourceOperationKind, SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens,
    SemanticTokensFullOptions, SemanticTokensLegend, SemanticTokensOptions, SemanticTokensParams,
    SemanticTokensResult, SemanticTokensServerCapabilities, ServerCapabilities, SymbolKind,
    TextDocumentEdit, TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit as LspTextEdit,
    Url, WatchKind, WorkDoneProgress, WorkDoneProgressBegin, WorkDoneProgressEnd,
    WorkDoneProgressOptions, WorkDoneProgressReport, WorkspaceEdit as LspWorkspaceEdit,
};
use plumb_core::Diagnostic;
use plumb_extensions::{
    link_completion_context, AnchorKind, AnchorRecord, Heading, MetadataBlock, MetadataEntry,
    MetadataValue, TaskRecord, TaskState, TaskStatus,
};
use plumb_workspace::{normalize, ResolvedTarget, ResourceOperation, Workspace, WorkspaceEdit};

use crate::position::{byte_range_to_lsp, position_to_offset};

pub(crate) struct ServerState {
    client: ClientSocket,
    workspace: Workspace,
    open_documents: HashMap<Url, PathBuf>,
    roots: Vec<PathBuf>,
    supports_document_changes: bool,
    supports_resource_rename: bool,
    supports_dynamic_watching: bool,
    pending_path_renames: Vec<PendingPathRename>,
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
            pending_path_renames: Vec::new(),
        }
    }

    fn update(&mut self, uri: Url, version: i32, text: String) {
        let Ok(path) = uri.to_file_path() else {
            return;
        };
        let path = normalize(&path);
        self.workspace.insert(&path, i64::from(version), text);
        self.open_documents.insert(uri, path);
        self.publish_all_open_diagnostics();
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
        let diagnostics = self
            .workspace
            .diagnostics(path)
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

    fn index_roots(&mut self) -> usize {
        self.notify_index_progress(WorkDoneProgress::Begin(WorkDoneProgressBegin {
            title: "Indexing plumb workspace".to_string(),
            cancellable: Some(false),
            message: Some("Scanning .plumb files".to_string()),
            percentage: None,
        }));
        let roots = self.roots.clone();
        let mut indexed = 0;
        for root in roots {
            indexed += self.index_directory(&root);
        }
        self.notify_index_progress(WorkDoneProgress::Report(WorkDoneProgressReport {
            cancellable: Some(false),
            message: Some(format!("Indexed {indexed} files")),
            percentage: None,
        }));
        self.notify_index_progress(WorkDoneProgress::End(WorkDoneProgressEnd {
            message: Some(format!("Indexed {indexed} plumb files")),
        }));
        indexed
    }

    fn index_directory(&mut self, directory: &Path) -> usize {
        let Ok(entries) = fs::read_dir(directory) else {
            return 0;
        };
        let mut indexed = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                indexed += self.index_directory(&path);
            } else if is_plumb_file(&path)
                && !self.open_documents.values().any(|open| open == &path)
            {
                if let Ok(text) = fs::read_to_string(&path) {
                    self.workspace.insert(path, 0, text);
                    indexed += 1;
                }
            }
        }
        indexed
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
                        watchers: vec![FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/*.plumb".to_string()),
                            kind: Some(WatchKind::Create | WatchKind::Change | WatchKind::Delete),
                        }],
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

    fn begin_path_rename(&mut self, old_path: PathBuf, new_path: PathBuf) {
        if let Some(entry) = self.workspace.get(&old_path) {
            let revision = entry.revision;
            let source = entry.parsed.source.clone();
            self.workspace.insert(&new_path, revision, source);
            self.workspace.remove(&old_path);
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
                }
                if !rename.new_path.exists()
                    && !self
                        .open_documents
                        .values()
                        .any(|open| open == &rename.new_path)
                {
                    self.workspace.remove(&rename.new_path);
                    rename.new_seen = false;
                }
            } else if changed_path == rename.new_path {
                rename.new_seen = rename.new_path.exists();
                if rename.new_seen {
                    if let Ok(text) = fs::read_to_string(&rename.new_path) {
                        self.workspace.insert(&rename.new_path, 0, text);
                    }
                } else if !self
                    .open_documents
                    .values()
                    .any(|open| open == &rename.new_path)
                {
                    self.workspace.remove(&rename.new_path);
                }
                rename.old_removed = !rename.old_path.exists();
            }
        }
        self.pending_path_renames
            .retain(|rename| !(rename.old_removed && rename.new_seen));
    }

    fn target_at(&self, path: &Path, offset: usize) -> Option<ResolvedTarget> {
        if let Some(reference) = self.workspace.anchor_reference_at(path, offset) {
            return Some(ResolvedTarget::Anchor {
                path: reference.target_path,
                id: reference.target_id,
                anchor: reference.anchor,
            });
        }
        if let Some(anchor) = self.workspace.anchor_at(path, offset) {
            return Some(ResolvedTarget::Anchor {
                path: normalize(path),
                id: anchor.id.value.clone(),
                anchor: anchor.clone(),
            });
        }
        let link = self.workspace.link_at(path, offset)?;
        Some(self.workspace.resolve_link(path, link))
    }
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
        Box::pin(async {
            Ok(InitializeResult {
                capabilities: ServerCapabilities {
                    text_document_sync: Some(TextDocumentSyncCapability::Kind(
                        TextDocumentSyncKind::FULL,
                    )),
                    document_symbol_provider: Some(OneOf::Left(true)),
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
                                    token_modifiers: vec![SemanticTokenModifier::new("completed")],
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
                            "[".to_string(),
                            "\"".to_string(),
                            "/".to_string(),
                            "#".to_string(),
                        ]),
                        ..CompletionOptions::default()
                    }),
                    rename_provider: Some(OneOf::Right(RenameOptions {
                        prepare_provider: Some(true),
                        work_done_progress_options: WorkDoneProgressOptions::default(),
                    })),
                    ..ServerCapabilities::default()
                },
                server_info: None,
            })
        })
    }

    fn initialized(&mut self, _params: InitializedParams) -> Self::NotifyResult {
        self.index_roots();
        self.register_workspace_file_watchers();
        self.publish_all_open_diagnostics();
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
            if self.roots.iter().any(|root| path.starts_with(root)) {
                if let Ok(text) = fs::read_to_string(&path) {
                    self.workspace.insert(&path, 0, text);
                } else {
                    self.workspace.remove(&path);
                }
            } else {
                self.workspace.remove(&path);
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
        ControlFlow::Continue(())
    }

    fn did_save(&mut self, _params: DidSaveTextDocumentParams) -> Self::NotifyResult {
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
        for change in params.changes {
            let Ok(path) = change.uri.to_file_path() else {
                continue;
            };
            let path = normalize(&path);
            if !is_plumb_file(&path) {
                continue;
            }
            self.confirm_pending_path_rename(&path);
            if self.open_documents.values().any(|open| open == &path) {
                continue;
            }
            match change.typ {
                FileChangeType::CREATED | FileChangeType::CHANGED => {
                    if let Ok(text) = fs::read_to_string(&path) {
                        self.workspace.insert(path, 0, text);
                    }
                }
                FileChangeType::DELETED => {
                    self.workspace.remove(path);
                }
                _ => {}
            }
        }
        self.publish_all_open_diagnostics();
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
                symbols.extend(current.output.anchors.iter().filter_map(|anchor| {
                    (anchor.kind != AnchorKind::Heading
                        && !current
                            .output
                            .tasks
                            .tasks
                            .iter()
                            .any(|task| task.range == anchor.range))
                    .then(|| anchor_symbol(&entry.parsed.source, anchor))
                }));
                if let Some(metadata) = &current.output.metadata.metadata {
                    symbols.push(metadata_symbol(&entry.parsed.source, metadata));
                }
                symbols.extend(task_symbols(
                    &entry.parsed.source,
                    &current.output.tasks.tasks,
                ));
                symbols
            });
        Box::pin(async move { Ok(symbols.map(DocumentSymbolResponse::Nested)) })
    }

    fn definition(
        &mut self,
        params: GotoDefinitionParams,
    ) -> BoxFuture<'static, Result<Option<GotoDefinitionResponse>, Self::Error>> {
        let position = params.text_document_position_params;
        let location = position
            .text_document
            .uri
            .to_file_path()
            .ok()
            .and_then(|path| {
                let entry = self.workspace.get(&path)?;
                let offset = position_to_offset(&entry.parsed.source, position.position);
                match self.target_at(&path, offset)? {
                    ResolvedTarget::Anchor { path, anchor, .. } => {
                        location_for(&self.workspace, &path, &anchor.selection_range)
                    }
                    ResolvedTarget::Document { path } => {
                        location_for(&self.workspace, &path, &(0..0))
                    }
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
        let locations = position
            .text_document
            .uri
            .to_file_path()
            .ok()
            .and_then(|path| {
                let entry = self.workspace.get(&path)?;
                let offset = position_to_offset(&entry.parsed.source, position.position);
                let ResolvedTarget::Anchor {
                    path: target_path,
                    id,
                    anchor,
                } = self.target_at(&path, offset)?
                else {
                    return None;
                };
                let mut locations = self
                    .workspace
                    .references_to(&target_path, &id)
                    .into_iter()
                    .filter_map(|(source_path, reference)| {
                        location_for(&self.workspace, source_path, &reference.source_range)
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
            });
        Box::pin(async move { Ok(locations) })
    }

    fn hover(
        &mut self,
        params: HoverParams,
    ) -> BoxFuture<'static, Result<Option<Hover>, Self::Error>> {
        let position = params.text_document_position_params;
        let hover = position
            .text_document
            .uri
            .to_file_path()
            .ok()
            .and_then(|path| {
                let entry = self.workspace.get(&path)?;
                let offset = position_to_offset(&entry.parsed.source, position.position);
                if let Some(task) = self.workspace.task_at(&path, offset) {
                    return Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: task_hover(&self.workspace, &path, task),
                        }),
                        range: Some(byte_range_to_lsp(
                            &entry.parsed.source,
                            &task.selection_range,
                        )),
                    });
                }
                let target = self.target_at(&path, offset)?;
                let message = match target {
                    ResolvedTarget::Anchor { path, id, .. } => {
                        format!("Explicit anchor `#{id}` in `{}`", path.display())
                    }
                    ResolvedTarget::Document { path } => {
                        format!("Plumb document `{}`", path.display())
                    }
                    ResolvedTarget::External => "External link".to_string(),
                    ResolvedTarget::Other => "Non-plumb link".to_string(),
                    ResolvedTarget::UnresolvedPath { path } => {
                        format!("Unresolved plumb document `{}`", path.display())
                    }
                    ResolvedTarget::UnresolvedAnchor { path, id } => {
                        format!("Unresolved explicit anchor `#{id}` in `{}`", path.display())
                    }
                    ResolvedTarget::AmbiguousAnchor { path, id } => {
                        format!("Ambiguous explicit anchor `#{id}` in `{}`", path.display())
                    }
                };
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
                let context = link_completion_context(&entry.parsed.source, offset)?;
                let is_document = matches!(
                    &context,
                    plumb_extensions::LinkCompletionContext::Label { .. }
                        | plumb_extensions::LinkCompletionContext::Path { .. }
                );
                Some(
                    self.workspace
                        .complete_link(&path, &context)
                        .into_iter()
                        .map(|candidate| CompletionItem {
                            label: candidate.label,
                            kind: Some(if is_document {
                                CompletionItemKind::FILE
                            } else {
                                CompletionItemKind::REFERENCE
                            }),
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
        let timestamp = Local::now().to_rfc3339_opts(SecondsFormat::Secs, false);
        let mut actions = Vec::new();
        if code_action_kind_requested(
            params.context.only.as_deref(),
            &CodeActionKind::REFACTOR_REWRITE,
        ) {
            if let Some(title) = path.file_stem().and_then(|stem| stem.to_str()) {
                if let Some(edit) = self
                    .workspace
                    .insert_metadata(&path, title, &timestamp)
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
                let data = current
                    .output
                    .tasks
                    .tasks
                    .iter()
                    .filter(|task| task.state() != TaskState::Open)
                    .filter_map(|task| {
                        let range = byte_range_to_lsp(&entry.parsed.source, &task.marker_range);
                        if range.start.line != range.end.line
                            || range.end.character <= range.start.character
                        {
                            return None;
                        }
                        let delta_line = range.start.line - previous_line;
                        let delta_start = if delta_line == 0 {
                            range.start.character - previous_start
                        } else {
                            range.start.character
                        };
                        previous_line = range.start.line;
                        previous_start = range.start.character;
                        Some(SemanticToken {
                            delta_line,
                            delta_start,
                            length: range.end.character - range.start.character,
                            token_type: 0,
                            token_modifiers_bitset: 1,
                        })
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
                            .path_rename_target_at(&path, offset)
                            .map(|target| {
                                let placeholder = target
                                    .old_path
                                    .file_name()
                                    .and_then(|name| name.to_str())
                                    .unwrap_or("document.plumb")
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
        let result = params
            .text_document_position
            .text_document
            .uri
            .to_file_path()
            .ok()
            .and_then(|path| {
                let entry = self.workspace.get(&path)?;
                let offset = position_to_offset(
                    &entry.parsed.source,
                    params.text_document_position.position,
                );
                if let Ok(target) = self.workspace.anchor_rename_target_at(&path, offset) {
                    let edit = self
                        .workspace
                        .rename_anchor(&target, &params.new_name)
                        .ok()?;
                    return workspace_edit_to_lsp(&self.workspace, edit);
                }
                if !self.supports_resource_rename {
                    return None;
                }
                let target = self.workspace.path_rename_target_at(&path, offset).ok()?;
                let edit = self
                    .workspace
                    .rename_document(&target, &params.new_name)
                    .ok()?;
                let (old_path, new_path) =
                    edit.resource_operations.iter().find_map(|operation| {
                        let ResourceOperation::Rename { old_path, new_path } = operation;
                        Some((old_path.clone(), new_path.clone()))
                    })?;
                let lsp_edit = workspace_edit_to_lsp(&self.workspace, edit)?;
                self.begin_path_rename(old_path, new_path);
                Some(lsp_edit)
            });
        Box::pin(async move { Ok(result) })
    }
}

fn heading_symbol(source: &str, heading: &Heading) -> DocumentSymbol {
    #[allow(deprecated)]
    DocumentSymbol {
        name: if heading.title.is_empty() {
            format!("Heading {}", heading.level)
        } else {
            heading.title.clone()
        },
        detail: Some(format!("level {}", heading.level)),
        kind: SymbolKind::STRING,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(source, &heading.section_range),
        selection_range: byte_range_to_lsp(source, &heading.selection_range),
        children: (!heading.children.is_empty()).then(|| {
            heading
                .children
                .iter()
                .map(|child| heading_symbol(source, child))
                .collect()
        }),
    }
}

fn anchor_symbol(source: &str, anchor: &AnchorRecord) -> DocumentSymbol {
    #[allow(deprecated)]
    DocumentSymbol {
        name: format!("#{}", anchor.id.value),
        detail: Some("explicit anchor".to_string()),
        kind: SymbolKind::KEY,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(source, &anchor.range),
        selection_range: byte_range_to_lsp(source, &anchor.id.range),
        children: None,
    }
}

fn metadata_symbol(source: &str, metadata: &MetadataBlock) -> DocumentSymbol {
    #[allow(deprecated)]
    DocumentSymbol {
        name: "metadata".to_string(),
        detail: Some("document metadata".to_string()),
        kind: SymbolKind::OBJECT,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(source, &metadata.range),
        selection_range: byte_range_to_lsp(source, &metadata.selection_range),
        children: (!metadata.entries.is_empty()).then(|| {
            metadata
                .entries
                .iter()
                .map(|entry| metadata_entry_symbol(source, entry))
                .collect()
        }),
    }
}

fn metadata_entry_symbol(source: &str, entry: &MetadataEntry) -> DocumentSymbol {
    let (detail, children) = match &entry.value {
        MetadataValue::Null { .. } => ("null".to_string(), None),
        MetadataValue::Scalar { content, .. } => (content.plain_text(), None),
        MetadataValue::List { items, .. } => (format!("list ({} items)", items.len()), None),
        MetadataValue::Map { entries, .. } => (
            "map".to_string(),
            (!entries.is_empty()).then(|| {
                entries
                    .iter()
                    .map(|entry| metadata_entry_symbol(source, entry))
                    .collect()
            }),
        ),
        MetadataValue::Verbatim { .. } => ("verbatim".to_string(), None),
        MetadataValue::Unsupported { .. } => ("unsupported value".to_string(), None),
    };
    #[allow(deprecated)]
    DocumentSymbol {
        name: entry.key.clone(),
        detail: Some(detail),
        kind: SymbolKind::PROPERTY,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(source, &entry.range),
        selection_range: byte_range_to_lsp(source, &entry.key_range),
        children,
    }
}

fn task_symbols(source: &str, tasks: &[TaskRecord]) -> Vec<DocumentSymbol> {
    let mut roots = Vec::new();
    let mut path = Vec::new();
    for task in tasks {
        while path.len() > task.depth {
            path.pop();
        }
        let siblings = task_symbol_children_mut(&mut roots, &path);
        siblings.push(task_symbol(source, task));
        path.push(siblings.len() - 1);
    }
    roots
}

fn task_symbol(source: &str, task: &TaskRecord) -> DocumentSymbol {
    let id = task
        .id
        .as_ref()
        .map(|id| format!(" #{}", id.value))
        .unwrap_or_default();
    #[allow(deprecated)]
    DocumentSymbol {
        name: if task.title.is_empty() {
            "Untitled task".to_string()
        } else {
            task.title.clone()
        },
        detail: Some(format!("{}{}", task_state_name(task.state()), id)),
        kind: SymbolKind::EVENT,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(source, &task.range),
        selection_range: byte_range_to_lsp(source, &task.selection_range),
        children: None,
    }
}

fn task_symbol_children_mut<'a>(
    roots: &'a mut Vec<DocumentSymbol>,
    path: &[usize],
) -> &'a mut Vec<DocumentSymbol> {
    let mut children = roots;
    for index in path {
        children = children[*index].children.get_or_insert_with(Vec::new);
    }
    children
}

fn task_hover(workspace: &Workspace, path: &Path, task: &TaskRecord) -> String {
    let state = match task.state() {
        TaskState::Open if workspace.is_task_blocked(path, task) => "blocked",
        state => task_state_name(state),
    };
    let mut lines = vec![
        format!("**Task:** {}", task.title),
        format!("**State:** {state}"),
    ];
    if let Some(id) = &task.id {
        lines.push(format!("**ID:** `#{}`", id.value));
    }
    for (label, field) in [
        ("Created", &task.created),
        ("Due", &task.due),
        ("Wait", &task.wait),
        ("Done", &task.done),
        ("Canceled", &task.canceled),
        ("Recur", &task.recur),
        ("Previous", &task.prev),
    ] {
        if let Some(field) = field {
            lines.push(format!("**{label}:** `{}`", field.value));
        }
    }
    if !task.depends.is_empty() {
        lines.push(format!(
            "**Depends:** {}",
            task.depends
                .iter()
                .map(|dependency| format!("`{}`", dependency.source))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let blockers = workspace.open_task_dependencies(path, task);
    if !blockers.is_empty() {
        lines.push(format!(
            "**Open blockers:** {}",
            blockers
                .iter()
                .map(|dependency| format!("`{}`", dependency.source))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    lines.join("\n\n")
}

fn task_state_name(state: TaskState) -> &'static str {
    match state {
        TaskState::Open => "open",
        TaskState::Done => "done",
        TaskState::Canceled => "canceled",
        TaskState::Conflicted => "conflicted",
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
    let entry = workspace.get(path)?;
    let uri = Url::from_file_path(path).ok()?;
    Some(Location::new(
        uri,
        byte_range_to_lsp(&entry.parsed.source, range),
    ))
}

fn workspace_edit_to_lsp(workspace: &Workspace, edit: WorkspaceEdit) -> Option<LspWorkspaceEdit> {
    let has_resource_operations = !edit.resource_operations.is_empty();
    let mut document_edits = Vec::new();
    for document in edit.document_changes {
        let entry = workspace.get(&document.path)?;
        let uri = Url::from_file_path(&document.path).ok()?;
        let version = (document.expected_revision > 0)
            .then(|| i32::try_from(document.expected_revision).ok())
            .flatten();
        let edits = document
            .edits
            .into_iter()
            .map(|edit| {
                OneOf::Left(LspTextEdit::new(
                    byte_range_to_lsp(&entry.parsed.source, &edit.range),
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
            plumb_core::DiagnosticSeverity::Error => DiagnosticSeverity::ERROR,
            plumb_core::DiagnosticSeverity::Warning => DiagnosticSeverity::WARNING,
            plumb_core::DiagnosticSeverity::Hint => DiagnosticSeverity::HINT,
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
    use plumb_core::parse;
    use plumb_extensions::{analyze_headings, analyze_metadata};

    use super::*;

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
}
