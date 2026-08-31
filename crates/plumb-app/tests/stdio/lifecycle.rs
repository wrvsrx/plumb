use serde_json::json;

use crate::support::{
    response, run_server, run_server_after_initial_index, run_server_after_response,
    unique_temp_dir, LspTestSession,
};

#[test]
fn did_save_does_not_crash_the_server() {
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": "file:///tmp/save.plumb", "languageId": "plumb", "version": 1,
                "text": "`# Title\n"
            }}
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didSave",
            "params": { "textDocument": { "uri": "file:///tmp/save.plumb" } }
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/documentSymbol",
            "params": { "textDocument": { "uri": "file:///tmp/save.plumb" } }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    assert_eq!(
        response(&run_server(&messages), 2)["result"][0]["name"],
        "Title"
    );
}
#[test]
fn initialized_reports_workspace_index_progress() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.plumb"), "`# A\n").unwrap();
    std::fs::write(root.join("b.plumb"), "`# B\n").unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": root_uri, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server_after_initial_index(&messages);
    let kinds = output
        .iter()
        .filter(|message| message["method"] == "$/progress")
        .map(|message| message["params"]["value"]["kind"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(kinds, ["begin", "report", "end"]);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn formatting_an_open_document_does_not_wait_for_initial_index() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("large.plumb"), "Paragraph.\n\n".repeat(150_000)).unwrap();
    let document = root.join("inbox.plumb");
    let document_uri = lsp_types::Url::from_file_path(&document).unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let first = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": root_uri, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": document_uri, "languageId": "plumb", "version": 1,
                "text": "`node Parent\n\n       `child Child\n"
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/formatting",
            "params": {
                "textDocument": { "uri": document_uri },
                "options": { "tabSize": 2, "insertSpaces": true }
            }
        }),
    ];
    let shutdown = [
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server_after_response(&first, &shutdown);
    let formatting_index = output
        .iter()
        .position(|message| message.get("id") == Some(&json!(2)))
        .expect("formatting response while initial indexing is running");
    if let Some(index_end) = output.iter().position(|message| {
        message["method"] == "$/progress" && message["params"]["value"]["kind"] == "end"
    }) {
        assert!(formatting_index < index_end);
    }
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn watcher_registration_follows_client_capability() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let messages_without_support = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": root_uri, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let without_support = run_server(&messages_without_support);
    assert!(!without_support
        .iter()
        .any(|message| message["method"] == "client/registerCapability"));

    let initialize_with_support = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": root_uri,
                "capabilities": { "workspace": { "didChangeWatchedFiles": {
                    "dynamicRegistration": true, "relativePatternSupport": true
                } } }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
    ];
    let mut session = LspTestSession::new();
    session.send_all(&initialize_with_support);
    let registration = session.wait_for(|message| message["method"] == "client/registerCapability");
    let watchers = registration["params"]["registrations"][0]["registerOptions"]["watchers"]
        .as_array()
        .unwrap();
    assert_eq!(watchers.len(), 3);
    assert_eq!(watchers[0]["globPattern"], "**/*.plumb");
    assert_eq!(watchers[0]["kind"], 7);
    assert_eq!(watchers[1]["globPattern"], "**/.ignore");
    assert_eq!(watchers[1]["kind"], 7);
    assert_eq!(watchers[2]["globPattern"], "**/*.json");
    assert_eq!(watchers[2]["kind"], 7);
    session.send(&json!({
        "jsonrpc": "2.0", "id": registration["id"], "result": null
    }));
    session.send(&json!({
        "jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": null
    }));
    session.wait_for_response(&json!(2));
    session.send(&json!({ "jsonrpc": "2.0", "method": "exit", "params": null }));
    session.finish();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn watched_file_create_indexes_the_new_document() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let target = root.join("topic.plumb");
    let source = root.join("index.plumb");
    std::fs::write(&target, "`# Topic\n  `@ topic\n").unwrap();
    let source_text = "See `->[topic|topic.plumb#topic].\n";
    let target_uri = lsp_types::Url::from_file_path(&target).unwrap();
    let source_uri = lsp_types::Url::from_file_path(&source).unwrap();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "workspace/didChangeWatchedFiles",
            "params": { "changes": [{ "uri": target_uri, "type": 1 }] }
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": source_uri, "languageId": "plumb", "version": 1, "text": source_text
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/definition",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 0, "character": 30 }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    assert_eq!(
        response(&run_server(&messages), 2)["result"]["uri"],
        target_uri.as_str()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn ignored_documents_are_indexed_only_while_open() {
    let root = unique_temp_dir();
    let private = root.join("private");
    std::fs::create_dir_all(&private).unwrap();
    std::fs::write(root.join(".ignore"), "private/\n").unwrap();
    let note = private.join("note.plumb");
    let source = "`= title\n\n Private note\n";
    std::fs::write(&note, source).unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let note_uri = lsp_types::Url::from_file_path(&note).unwrap();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": {}
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "workspace/symbol",
            "params": { "query": "note Private" }
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": note_uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "workspace/symbol",
            "params": { "query": "note Private" }
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didClose",
            "params": { "textDocument": { "uri": note_uri } }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "workspace/symbol",
            "params": { "query": "note Private" }
        }),
        json!({ "jsonrpc": "2.0", "id": 5, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let mut session = LspTestSession::new();
    session.send_all(&messages[..2]);
    session.wait_for(|message| {
        message["method"] == "$/progress"
            && message["params"]["token"] == "plumb-ls-index"
            && message["params"]["value"]["kind"] == "end"
    });
    session.send(&messages[2]);
    session.wait_for_response(&json!(2));
    session.send_all(&messages[3..5]);
    session.wait_for_response(&json!(3));
    session.send_all(&messages[5..7]);
    session.wait_for_response(&json!(4));
    session.send(&messages[7]);
    session.wait_for_response(&json!(5));
    session.send(&messages[8]);
    let output = session.finish();
    assert!(response(&output, 2)["result"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(response(&output, 3)["result"].as_array().unwrap().len(), 1);
    assert!(response(&output, 4)["result"]
        .as_array()
        .unwrap()
        .is_empty());

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn ignore_file_changes_resynchronize_the_workspace_index() {
    let root = unique_temp_dir();
    let private = root.join("private");
    std::fs::create_dir_all(&private).unwrap();
    let ignore = root.join(".ignore");
    let note = private.join("note.plumb");
    std::fs::write(&ignore, "private/\n").unwrap();
    std::fs::write(&note, "`= title\n\n Private note\n").unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let ignore_uri = lsp_types::Url::from_file_path(&ignore).unwrap();

    let initialize = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": {}
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "workspace/symbol",
            "params": { "query": "note Private" }
        }),
    ];
    let update = [
        json!({
            "jsonrpc": "2.0", "method": "workspace/didChangeWatchedFiles",
            "params": { "changes": [{ "uri": ignore_uri, "type": 2 }] }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "workspace/symbol",
            "params": { "query": "note Private" }
        }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let mut session = LspTestSession::new();
    session.send_all(&initialize[..2]);
    session.wait_for(|message| {
        message["method"] == "$/progress"
            && message["params"]["token"] == "plumb-ls-index"
            && message["params"]["value"]["kind"] == "end"
    });
    session.send(&initialize[2]);
    session.wait_for_response(&json!(2));
    std::fs::write(&ignore, "").unwrap();
    session.send_all(&update[..2]);
    session.wait_for_response(&json!(3));
    session.send(&update[2]);
    session.wait_for_response(&json!(4));
    session.send(&update[3]);
    let output = session.finish();
    assert!(response(&output, 2)["result"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(response(&output, 3)["result"].as_array().unwrap().len(), 1);

    std::fs::remove_dir_all(root).unwrap();
}
