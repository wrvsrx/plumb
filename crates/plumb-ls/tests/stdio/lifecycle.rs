use serde_json::json;

use crate::support::{
    response, run_server, run_server_with_pause, run_server_with_writer, unique_temp_dir,
    write_message,
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

    let output = run_server(&messages);
    let kinds = output
        .iter()
        .filter(|message| message["method"] == "$/progress")
        .map(|message| message["params"]["value"]["kind"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(kinds, ["begin", "report", "end"]);
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
    let shutdown = [
        json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let with_support = run_server_with_pause(&initialize_with_support, &shutdown);
    let registration = with_support
        .iter()
        .find(|message| message["method"] == "client/registerCapability")
        .expect("watcher registration request");
    let watchers = registration["params"]["registrations"][0]["registerOptions"]["watchers"]
        .as_array()
        .unwrap();
    assert_eq!(watchers.len(), 2);
    assert_eq!(watchers[0]["globPattern"], "**/*.plumb");
    assert_eq!(watchers[0]["kind"], 7);
    assert_eq!(watchers[1]["globPattern"], "**/.ignore");
    assert_eq!(watchers[1]["kind"], 7);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn watched_file_create_indexes_the_new_document() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let target = root.join("topic.plumb");
    let source = root.join("index.plumb");
    std::fs::write(&target, "`# Topic\n   {\n     `@ topic\n   }\n").unwrap();
    let source_text = "See `->[topic]{`:[to topic.plumb#topic]}.\n";
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
    let source = "`meta\n `: title\n\n    Private note\n";
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

    let output = run_server_with_writer(|stdin| {
        for message in &messages[..2] {
            write_message(stdin, message);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        for message in &messages[2..4] {
            write_message(stdin, message);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        for message in &messages[4..6] {
            write_message(stdin, message);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        write_message(stdin, &messages[6]);
        std::thread::sleep(std::time::Duration::from_millis(50));
        for message in &messages[7..] {
            write_message(stdin, message);
        }
    });
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
    std::fs::write(&note, "`meta\n `: title\n\n    Private note\n").unwrap();
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

    let output = run_server_with_writer(|stdin| {
        for message in &initialize {
            write_message(stdin, message);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        std::fs::write(&ignore, "").unwrap();
        for message in &update {
            write_message(stdin, message);
            if message.get("id") == Some(&json!(3)) {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    });
    assert!(response(&output, 2)["result"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(response(&output, 3)["result"].as_array().unwrap().len(), 1);

    std::fs::remove_dir_all(root).unwrap();
}
