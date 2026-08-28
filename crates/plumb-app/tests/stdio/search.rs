use serde_json::json;

use crate::support::{
    response, run_server, run_server_after_initial_index, run_server_with_pause, unique_temp_dir,
};

#[cfg(unix)]
#[test]
fn workspace_index_does_not_follow_directory_symlinks() {
    use std::os::unix::fs::symlink;

    let root = unique_temp_dir();
    let snapshot = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&snapshot).unwrap();
    let source = root.join("current.plumb");
    let target = root.join("target.plumb");
    let target_source =
        "`= title|Target\n\n`# Target\n\n `@ anchor\n\n`task Target work\n\n `@ work\n";
    let anchor_line = target_source
        .lines()
        .position(|line| line.contains("`@ anchor"))
        .unwrap();
    std::fs::write(&source, "`->[").unwrap();
    std::fs::write(&target, target_source).unwrap();
    std::fs::write(snapshot.join("target.plumb"), target_source).unwrap();
    let reference_source = "`->[Target|target.plumb#anchor]\n";
    std::fs::write(root.join("reference.plumb"), reference_source).unwrap();
    std::fs::write(snapshot.join("reference.plumb"), reference_source).unwrap();
    std::fs::write(root.join("linked-source.txt"), "`= title\n\n Linked file\n").unwrap();
    symlink(&snapshot, root.join("snapshot")).unwrap();
    symlink(&root, root.join("cycle")).unwrap();
    symlink(root.join("linked-source.txt"), root.join("linked.plumb")).unwrap();

    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let source_uri = lsp_types::Url::from_file_path(&source).unwrap();
    let target_uri = lsp_types::Url::from_file_path(&target).unwrap();
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
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": source_uri, "languageId": "plumb", "version": 1, "text": "`->["
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 0, "character": 4 }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "plumb/search",
            "params": { "query": "Target", "limit": 20 }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/references",
            "params": {
                "textDocument": { "uri": target_uri },
                "position": { "line": anchor_line, "character": 3 },
                "context": { "includeDeclaration": false }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 5, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server_after_initial_index(&messages);
    let items = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(
        items.iter().filter(|item| item["label"] == "Link").count(),
        1
    );
    assert_eq!(
        items
            .iter()
            .filter(|item| item["label"] == "Target")
            .count(),
        1
    );
    assert_eq!(
        items
            .iter()
            .filter(|item| item["label"] == "Linked file")
            .count(),
        1
    );
    assert!(items.iter().all(|item| !item["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("snapshot"))));
    let records = response(&output, 3)["result"]["items"].as_array().unwrap();
    assert_eq!(
        records
            .iter()
            .filter(|item| item["kind"] == "note" && item["title"] == "Target")
            .count(),
        1
    );
    assert_eq!(
        records
            .iter()
            .filter(|item| item["kind"] == "task" && item["title"] == "Target work")
            .count(),
        1
    );
    let references = response(&output, 4)["result"].as_array().unwrap();
    assert_eq!(references.len(), 1);
    assert!(references[0]["uri"]
        .as_str()
        .unwrap()
        .ends_with("/reference.plumb"));

    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(snapshot).unwrap();
}

#[test]
fn searches_workspace_symbols_and_structured_records_over_stdio() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let note = root.join("note.plumb");
    let tasks = root.join("tasks.plumb");
    let events = root.join("events.plumb");
    std::fs::write(&note, "`= title\n\n Disk title\n").unwrap();
    std::fs::write(
        &tasks,
        "`task Review parser\n\n `@ review\n `= due|2026-07-23T12:00:00+08:00\n",
    )
    .unwrap();
    std::fs::write(
        &events,
        "`= date|2026-07-30\n`= timezone|+08:00\n\n`event 14:00--15:00|Review meeting\n\n `@ review-event\n `= tasks|tasks.plumb#review\n",
    )
    .unwrap();
    for index in 0..105 {
        std::fs::write(
            root.join(format!("extra-{index:03}.plumb")),
            format!("Extra note {index}\n"),
        )
        .unwrap();
    }
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let note_uri = lsp_types::Url::from_file_path(&note).unwrap();
    let open_note = "`= title\n\n Open title\n";
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
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": note_uri, "languageId": "plumb", "version": 9, "text": open_note
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "workspace/symbol",
            "params": { "query": "note Open" }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "workspace/symbol",
            "params": { "query": "task review" }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "plumb/search",
            "params": {
                "kind": "task", "query": "review", "filter": "actionable", "limit": 20
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 5, "method": "plumb/search",
            "params": { "query": "", "limit": 1 }
        }),
        json!({
            "jsonrpc": "2.0", "id": 10, "method": "plumb/search",
            "params": {
                "kind": "event", "query": "meeting",
                "filter": "start < timestamp('2026-07-30T07:00:00Z')", "limit": 20
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 9, "method": "workspace/symbol",
            "params": { "query": "" }
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": note_uri, "version": 10 },
                "contentChanges": [{ "text": "`span[broken\n" }]
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 6, "method": "plumb/search",
            "params": { "kind": "note", "query": "Open", "limit": 20 }
        }),
        json!({
            "jsonrpc": "2.0", "id": 7, "method": "plumb/search",
            "params": { "kind": "note", "filter": "path", "limit": 20 }
        }),
        json!({ "jsonrpc": "2.0", "id": 8, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server_after_initial_index(&messages);
    let capabilities = &response(&output, 1)["result"]["capabilities"];
    assert_eq!(capabilities["workspaceSymbolProvider"], true);
    assert_eq!(
        capabilities["experimental"]["plumb"]["search"]["schemaVersion"],
        3
    );
    assert_eq!(
        capabilities["experimental"]["plumb"]["search"]["method"],
        "plumb/search"
    );

    let notes = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["name"], "Open title");
    assert_eq!(notes[0]["kind"], 1);
    assert_eq!(notes[0]["location"]["uri"], note_uri.as_str());
    let task_symbols = response(&output, 3)["result"].as_array().unwrap();
    assert_eq!(task_symbols.len(), 1);
    assert_eq!(task_symbols[0]["name"], "Review parser");

    let structured = &response(&output, 4)["result"];
    assert_eq!(structured["schemaVersion"], 3);
    assert_eq!(structured["complete"], true);
    assert_eq!(structured["items"][0]["kind"], "task");
    assert_eq!(structured["items"][0]["id"], "review");
    assert_eq!(structured["items"][0]["state"], "ready");
    assert_eq!(structured["items"][0]["waitReasons"], json!([]));
    assert_eq!(structured["items"][0]["blocked"], false);
    assert_eq!(structured["items"][0]["provenance"]["source"], "current");
    assert_eq!(structured["items"][0]["provenance"]["revision"], 0);
    assert_eq!(response(&output, 5)["result"]["complete"], false);
    let event = &response(&output, 10)["result"]["items"][0];
    assert_eq!(event["kind"], "event");
    assert_eq!(event["id"], "review-event");
    assert_eq!(event["start"], "2026-07-30T14:00:00+08:00");
    assert_eq!(event["end"], "2026-07-30T15:00:00+08:00");
    assert_eq!(event["tasks"], json!(["tasks.plumb#review"]));
    assert_eq!(
        response(&output, 9)["result"].as_array().unwrap().len(),
        100
    );
    assert!(response(&output, 6)["result"]["items"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(response(&output, 7)["error"]["code"], -32602);
    assert!(response(&output, 7)["error"]["message"]
        .as_str()
        .unwrap()
        .contains("must return bool"));

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancels_standard_and_structured_search_before_result_publication() {
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "plumb/search",
            "params": { "query": "", "limit": 100 }
        }),
        json!({
            "jsonrpc": "2.0", "method": "$/cancelRequest", "params": { "id": 2 }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "workspace/symbol",
            "params": { "query": "" }
        }),
        json!({
            "jsonrpc": "2.0", "method": "$/cancelRequest", "params": { "id": 3 }
        }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    assert_eq!(response(&output, 2)["error"]["code"], -32800);
    assert_eq!(response(&output, 3)["error"]["code"], -32800);
}

#[test]
fn structured_search_rejects_requests_before_initial_index() {
    let first = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "plumb/search",
            "params": { "query": "", "limit": 100 }
        }),
    ];
    let second = [
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server_with_pause(&first, &second);
    assert_eq!(response(&output, 2)["error"]["code"], -32002);
}

#[test]
fn structured_search_marks_failed_workspace_scans_incomplete() {
    let root = unique_temp_dir();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "missing" }],
                "capabilities": {}
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "plumb/search",
            "params": { "query": "", "limit": 100 }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server_with_pause(
        &messages[..messages.len() - 2],
        &messages[messages.len() - 2..],
    );
    assert_eq!(response(&output, 2)["result"]["complete"], false);
}
