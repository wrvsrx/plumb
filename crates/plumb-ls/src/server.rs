use std::collections::HashMap;
use std::ops::ControlFlow;

use async_lsp::{ClientSocket, LanguageServer, ResponseError};
use futures::future::BoxFuture;
use lsp_types::{
    Diagnostic as LspDiagnostic, DiagnosticRelatedInformation, DiagnosticSeverity,
    DidChangeConfigurationParams, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentSymbol, DocumentSymbolParams,
    DocumentSymbolResponse, InitializeParams, InitializeResult, InitializedParams, Location,
    NumberOrString, OneOf, PublishDiagnosticsParams, ServerCapabilities, SymbolKind,
    TextDocumentSyncCapability, TextDocumentSyncKind, Url,
};
use plumb_core::{parse, Diagnostic, ParsedDocument};
use plumb_extensions::{analyze_headings, Heading};

use crate::position::byte_range_to_lsp;

struct OpenDocument {
    parsed: ParsedDocument,
}

pub(crate) struct ServerState {
    client: ClientSocket,
    documents: HashMap<Url, OpenDocument>,
}

impl ServerState {
    pub(crate) fn new(client: ClientSocket) -> Self {
        Self {
            client,
            documents: HashMap::new(),
        }
    }

    fn update(&mut self, uri: Url, version: i32, text: String) {
        let parsed = parse(text);
        self.publish(&uri, version, &parsed);
        self.documents.insert(uri, OpenDocument { parsed });
    }

    fn publish(&self, uri: &Url, version: i32, parsed: &ParsedDocument) {
        let mut diagnostics = parsed.diagnostics.clone();
        if parsed.is_valid() {
            diagnostics.extend(analyze_headings(&parsed.syntax).diagnostics);
        }
        let diagnostics = diagnostics
            .into_iter()
            .map(|diagnostic| to_lsp_diagnostic(&parsed.source, uri, diagnostic))
            .collect();
        let _ = self
            .client
            .notify::<lsp_types::notification::PublishDiagnostics>(PublishDiagnosticsParams {
                uri: uri.clone(),
                diagnostics,
                version: Some(version),
            });
    }
}

impl LanguageServer for ServerState {
    type Error = ResponseError;
    type NotifyResult = ControlFlow<async_lsp::Result<()>>;

    fn initialize(
        &mut self,
        _params: InitializeParams,
    ) -> BoxFuture<'static, Result<InitializeResult, Self::Error>> {
        Box::pin(async {
            Ok(InitializeResult {
                capabilities: ServerCapabilities {
                    text_document_sync: Some(TextDocumentSyncCapability::Kind(
                        TextDocumentSyncKind::FULL,
                    )),
                    document_symbol_provider: Some(OneOf::Left(true)),
                    ..ServerCapabilities::default()
                },
                server_info: None,
            })
        })
    }

    fn initialized(&mut self, _params: InitializedParams) -> Self::NotifyResult {
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
        self.documents.remove(&uri);
        let _ = self
            .client
            .notify::<lsp_types::notification::PublishDiagnostics>(PublishDiagnosticsParams {
                uri,
                diagnostics: Vec::new(),
                version: None,
            });
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
        let symbols = self
            .documents
            .get(&params.text_document.uri)
            .and_then(|document| {
                if !document.parsed.is_valid() {
                    return None;
                }
                let output = analyze_headings(&document.parsed.syntax);
                Some(
                    output
                        .headings
                        .iter()
                        .map(|heading| heading_symbol(&document.parsed.source, heading))
                        .collect(),
                )
            });
        Box::pin(async move { Ok(symbols.map(DocumentSymbolResponse::Nested)) })
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
