use super::*;

/// Drive publications through the real JSON-RPC writer, without worker timing.
pub(super) async fn publications(
    run: impl Fn(&mut ServerState) + Send + 'static,
) -> Vec<serde_json::Value> {
    use std::io::{BufRead, Read};
    struct Run;
    struct Stop;
    let (server, client) = async_lsp::MainLoop::new_server(|client| {
        let completion = client.clone();
        let mut router = async_lsp::router::Router::new(ServerState::new(client));
        router.event::<Run>(move |state, _| {
            run(state);
            completion.emit(Stop).unwrap();
            ControlFlow::Continue(())
        });
        router.event::<Stop>(|_, _| ControlFlow::Break(Ok(())));
        router
    });
    client.emit(Run).unwrap();
    let mut output = Vec::new();
    server
        .run_buffered(futures::io::Cursor::new(Vec::<u8>::new()), &mut output)
        .await
        .unwrap();
    let mut input = std::io::Cursor::new(output);
    let mut messages = Vec::new();
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
        if message["method"] == "textDocument/publishDiagnostics" {
            messages.push(message["params"].clone());
        }
    }
    messages
}

#[tokio::test]
async fn local_scope_waits_for_own_analysis_but_not_other_documents() {
    let messages = publications(|state| {
        state.roots = vec![PathBuf::from("/notes")];
        let path = PathBuf::from("/notes/day.plumb");
        let uri = Url::from_file_path(&path).unwrap();
        let source = "`= title One\n`= title Two\nSee `->{missing.plumb}\n";
        state.update(uri, 1, source.into(), None, false);
        let (_, generation) = state.document_analysis_tokens.next(&path);
        let pending = state
            .workspace
            .begin_document_revision(&path, 2, source.replace("One", "New"))
            .unwrap();
        let _other = state
            .workspace
            .begin_document_revision("/notes/other.plumb", 1, "Other\n")
            .unwrap();
        state.publish_all_open_diagnostics();
        let _ = state.finish_document_analysis(DocumentAnalysisResult {
            path,
            generation,
            analysis: Ok(pending.analyze()),
        });
    })
    .await;
    assert_eq!(messages.len(), 2, "{messages:?}");
    for (message, version) in messages.iter().zip([1, 2]) {
        assert_eq!(message["version"], version);
        let diagnostics = message["diagnostics"].as_array().unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0]["code"], "metadata.duplicate-key");
    }
}

#[tokio::test]
async fn stale_document_analysis_cannot_publish_or_install() {
    let messages = publications(|state| {
        let path = PathBuf::from("/notes/day.plumb");
        let uri = Url::from_file_path(&path).unwrap();
        state.update(uri, 1, "`= title One\n`= title Two\n".into(), None, false);
        let (_, stale_generation) = state.document_analysis_tokens.next(&path);
        let stale = state
            .workspace
            .begin_document_revision(&path, 2, "Interim\n")
            .unwrap()
            .analyze();
        let (_, generation) = state.document_analysis_tokens.next(&path);
        let current = state
            .workspace
            .begin_document_revision(&path, 3, "Final\n")
            .unwrap();
        let _ = state.finish_document_analysis(DocumentAnalysisResult {
            path: path.clone(),
            generation: stale_generation,
            analysis: Ok(stale),
        });
        assert!(state.workspace.document_analysis_pending(&path));
        state.publish_all_open_diagnostics();
        let _ = state.finish_document_analysis(DocumentAnalysisResult {
            path,
            generation,
            analysis: Ok(current.analyze()),
        });
    })
    .await;
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert_eq!(messages[0]["version"], 1);
    assert_eq!(messages[1]["version"], 3);
    assert_eq!(messages[1]["diagnostics"], serde_json::json!([]));
}

#[tokio::test]
async fn invalid_source_waits_for_current_regional_analysis_and_workspace_scope() {
    let messages = publications(|state| {
        state.index_complete = true;
        let _other = state
            .workspace
            .begin_document_revision("/notes/other.plumb", 1, "Other\n")
            .unwrap();
        let path = PathBuf::from("/notes/day.plumb");
        state
            .open_documents
            .insert(Url::from_file_path(&path).unwrap(), path.clone());
        state
            .workspace
            .begin_document_revision(&path, 2, "`broken{")
            .unwrap();
        state.publish_all_open_diagnostics();
        assert!(state
            .workspace
            .complete_pending_document_analysis("/notes/day.plumb"));
        assert!(state
            .workspace
            .complete_pending_document_analysis("/notes/other.plumb"));
        state.publish_all_open_diagnostics();
    })
    .await;
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0]["version"], 2);
    assert_eq!(
        messages[0]["diagnostics"][0]["code"],
        "syntax.unclosed-inline-group"
    );
}

#[tokio::test]
async fn index_promotion_waits_for_pending_analysis_before_publishing_workspace_results() {
    let messages = publications(|state| {
        state.roots = vec![PathBuf::from("/notes")];
        let path = PathBuf::from("/notes/day.plumb");
        let source = "`= title One\n`= title Two\nSee `->{missing.plumb}\n";
        state.update(
            Url::from_file_path(&path).unwrap(),
            1,
            source.into(),
            None,
            false,
        );
        let (_, generation) = state.document_analysis_tokens.next(&path);
        let pending = state
            .workspace
            .begin_document_revision(&path, 2, source.replace("One", "New"))
            .unwrap();
        state.index_complete = true;
        state.publish_all_open_diagnostics();
        let _ = state.finish_document_analysis(DocumentAnalysisResult {
            path,
            generation,
            analysis: Ok(pending.analyze()),
        });
    })
    .await;
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert_eq!(messages[0]["version"], 1);
    assert_eq!(messages[0]["diagnostics"].as_array().unwrap().len(), 1);
    assert_eq!(messages[1]["version"], 2);
    let diagnostics = messages[1]["diagnostics"].as_array().unwrap();
    assert!(diagnostics
        .iter()
        .any(|d| d["code"] == "metadata.duplicate-key"));
    assert!(diagnostics
        .iter()
        .any(|d| d["code"] == "link.unresolved-path"));
}
