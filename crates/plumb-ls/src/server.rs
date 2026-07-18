use std::collections::HashMap;
use std::fs;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use async_lsp::{ClientSocket, LanguageServer, ResponseError};
use futures::future::BoxFuture;
use lsp_types::{
    Diagnostic as LspDiagnostic, DiagnosticRelatedInformation, DiagnosticSeverity,
    DidChangeConfigurationParams, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentSymbol, DocumentSymbolParams,
    DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents,
    HoverParams, HoverProviderCapability, InitializeParams, InitializeResult, InitializedParams,
    Location, MarkupContent, MarkupKind, NumberOrString, OneOf, PublishDiagnosticsParams,
    ReferenceParams, ServerCapabilities, SymbolKind, TextDocumentSyncCapability,
    TextDocumentSyncKind, Url,
};
use plumb_core::Diagnostic;
use plumb_extensions::{AnchorKind, AnchorRecord, Heading};
use plumb_workspace::{normalize, ResolvedTarget, Workspace};

use crate::position::{byte_range_to_lsp, position_to_offset};

pub(crate) struct ServerState {
    client: ClientSocket,
    workspace: Workspace,
    open_documents: HashMap<Url, PathBuf>,
    roots: Vec<PathBuf>,
}

impl ServerState {
    pub(crate) fn new(client: ClientSocket) -> Self {
        Self {
            client,
            workspace: Workspace::new(),
            open_documents: HashMap::new(),
            roots: Vec::new(),
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

    fn index_roots(&mut self) {
        let roots = self.roots.clone();
        for root in roots {
            self.index_directory(&root);
        }
    }

    fn index_directory(&mut self, directory: &Path) {
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                self.index_directory(&path);
            } else if is_plumb_file(&path)
                && !self.open_documents.values().any(|open| open == &path)
            {
                if let Ok(text) = fs::read_to_string(&path) {
                    self.workspace.insert(path, 0, text);
                }
            }
        }
    }

    fn target_at(&self, path: &Path, offset: usize) -> Option<ResolvedTarget> {
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
        Box::pin(async {
            Ok(InitializeResult {
                capabilities: ServerCapabilities {
                    text_document_sync: Some(TextDocumentSyncCapability::Kind(
                        TextDocumentSyncKind::FULL,
                    )),
                    document_symbol_provider: Some(OneOf::Left(true)),
                    definition_provider: Some(OneOf::Left(true)),
                    references_provider: Some(OneOf::Left(true)),
                    hover_provider: Some(HoverProviderCapability::Simple(true)),
                    ..ServerCapabilities::default()
                },
                server_info: None,
            })
        })
    }

    fn initialized(&mut self, _params: InitializedParams) -> Self::NotifyResult {
        self.index_roots();
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
                    (anchor.kind != AnchorKind::Heading)
                        .then(|| anchor_symbol(&entry.parsed.source, anchor))
                }));
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
                    .filter_map(|(source_path, link)| {
                        location_for(&self.workspace, source_path, &link.selection_range)
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
    use plumb_extensions::analyze_headings;

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
}
