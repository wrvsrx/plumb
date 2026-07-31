use std::path::PathBuf;

use serde_json::{json, Value};

use crate::support::{response, run_server, run_server_with_pause, unique_temp_dir};

#[test]
fn definition_resolves_a_file_name_containing_spaces() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("source.plumb");
    let target = root.join("other file.plumb");
    let source_text = "See `->[topic]{to=\"other file.plumb#topic\"}.\n";
    std::fs::write(&source, source_text).unwrap();
    std::fs::write(&target, "`node{#topic} Topic\n").unwrap();
    let source_uri = lsp_types::Url::from_file_path(&source).unwrap();
    let target_uri = lsp_types::Url::from_file_path(&target).unwrap();
    let position = source_text.find("other file.plumb").unwrap();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
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
                "position": { "line": 0, "character": position }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let result = &response(&output, 2)["result"];
    assert_eq!(result["uri"], target_uri.as_str());
    assert_eq!(result["range"]["start"]["line"], 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn document_references_resolve_metadata_and_reference_components() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let target = root.join("target.plumb");
    let source = root.join("source.plumb");
    let lonely = root.join("lonely.plumb");
    let target_text = "`meta\n `: title\n\n    Target\n\n`#{#section} Section\nSee `->[self]{to=\"target.plumb\"}.\n";
    let source_text = "See `->[document]{to=\"target.plumb\"}.\nSee `->[section]{to=\"target.plumb#section\"}.\n`-{.task prev=\"target.plumb#section\" depends=\"target.plumb#section\"} Review\n";
    let lonely_text = "`meta\n `: title\n\n    Lonely\n";
    std::fs::write(&target, target_text).unwrap();
    std::fs::write(&source, source_text).unwrap();
    std::fs::write(&lonely, lonely_text).unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let target_uri = lsp_types::Url::from_file_path(&target).unwrap();
    let source_uri = lsp_types::Url::from_file_path(&source).unwrap();
    let lonely_uri = lsp_types::Url::from_file_path(&lonely).unwrap();
    let source_lines = source_text.lines().collect::<Vec<_>>();
    let document_path = source_lines[0].find("target.plumb").unwrap();
    let anchor_path = source_lines[1].find("target.plumb").unwrap();
    let anchor_fragment = source_lines[1].find("#section").unwrap() + 1;
    let task_prev_path = source_lines[2].find("target.plumb").unwrap();
    let depends_start = source_lines[2].find("depends=").unwrap();
    let task_depends_path = depends_start
        + source_lines[2][depends_start..]
            .find("target.plumb")
            .unwrap();
    let reference_request = |id, uri: &lsp_types::Url, line, character, include_declaration| {
        json!({
            "jsonrpc": "2.0", "id": id, "method": "textDocument/references",
            "params": {
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character },
                "context": { "includeDeclaration": include_declaration }
            }
        })
    };
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": {}
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        reference_request(2, &target_uri, 0, 1, false),
        reference_request(3, &source_uri, 0, document_path, false),
        reference_request(4, &source_uri, 1, anchor_path, false),
        reference_request(5, &source_uri, 2, task_prev_path, false),
        reference_request(6, &source_uri, 2, task_depends_path, false),
        reference_request(7, &source_uri, 1, anchor_fragment, false),
        reference_request(8, &target_uri, 0, 1, true),
        reference_request(9, &source_uri, 1, anchor_fragment, true),
        reference_request(10, &lonely_uri, 0, 1, false),
        reference_request(11, &lonely_uri, 0, 1, true),
        json!({
            "jsonrpc": "2.0", "id": 12, "method": "textDocument/codeLens",
            "params": { "textDocument": { "uri": target_uri } }
        }),
        json!({ "jsonrpc": "2.0", "id": 13, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let metadata_references = &response(&output, 2)["result"];
    assert_eq!(metadata_references.as_array().unwrap().len(), 5);
    assert_eq!(response(&output, 3)["result"], *metadata_references);
    assert_eq!(response(&output, 4)["result"], *metadata_references);
    assert_eq!(response(&output, 5)["result"], *metadata_references);
    assert_eq!(response(&output, 6)["result"], *metadata_references);

    let anchor_references = &response(&output, 7)["result"];
    assert_eq!(anchor_references.as_array().unwrap().len(), 3);
    let document_with_declaration = response(&output, 8)["result"].as_array().unwrap();
    assert_eq!(document_with_declaration.len(), 6);
    assert_eq!(document_with_declaration[0]["uri"], target_uri.as_str());
    assert_eq!(document_with_declaration[0]["range"]["start"]["line"], 0);
    let anchor_with_declaration = response(&output, 9)["result"].as_array().unwrap();
    assert_eq!(anchor_with_declaration.len(), 4);
    assert_eq!(anchor_with_declaration[0]["uri"], target_uri.as_str());
    assert_eq!(anchor_with_declaration[0]["range"]["start"]["line"], 5);
    assert!(response(&output, 10)["result"]
        .as_array()
        .unwrap()
        .is_empty());
    let lonely_with_declaration = response(&output, 11)["result"].as_array().unwrap();
    assert_eq!(lonely_with_declaration.len(), 1);
    assert_eq!(lonely_with_declaration[0]["uri"], lonely_uri.as_str());
    let lenses = response(&output, 12)["result"].as_array().unwrap();
    assert_eq!(lenses[0]["command"]["arguments"][2], *metadata_references);

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn task_references_support_navigation_and_rename() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let target = root.join("Project Plan.plumb");
    let source = root.join("review.plumb");
    let target_text = "`-{.task #draft} Draft\n";
    let source_text = "`-{.task #review prev=\"Project Plan.plumb#draft\" depends=\"Project Plan.plumb#draft\"} Review\n";
    std::fs::write(&target, target_text).unwrap();
    std::fs::write(&source, source_text).unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let target_uri = lsp_types::Url::from_file_path(&target).unwrap();
    let source_uri = lsp_types::Url::from_file_path(&source).unwrap();
    let target_id = target_text.find("#draft").unwrap() + 1;
    let prev_id = source_text.find("#draft").unwrap() + 1;
    let depends_start = source_text.find("depends=").unwrap();
    let depends_id = depends_start + source_text[depends_start..].find("#draft").unwrap() + 1;
    let task_path = source_text.find("Project Plan.plumb").unwrap();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": { "workspace": { "workspaceEdit": {
                    "documentChanges": true, "resourceOperations": ["rename"]
                } } }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": source_uri, "languageId": "plumb", "version": 7, "text": source_text
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/definition",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 0, "character": depends_id }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/references",
            "params": {
                "textDocument": { "uri": target_uri },
                "position": { "line": 0, "character": target_id },
                "context": { "includeDeclaration": false }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/references",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 0, "character": prev_id },
                "context": { "includeDeclaration": true }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 5, "method": "textDocument/rename",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 0, "character": depends_id },
                "newName": "first-draft"
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 6, "method": "textDocument/rename",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 0, "character": task_path },
                "newName": "Archived Plan.plumb"
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 7, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let output = run_server(&messages);
    assert_eq!(response(&output, 2)["result"]["uri"], target_uri.as_str());
    assert_eq!(response(&output, 3)["result"].as_array().unwrap().len(), 2);
    assert_eq!(response(&output, 4)["result"].as_array().unwrap().len(), 3);

    let anchor_changes = response(&output, 5)["result"]["documentChanges"]
        .as_array()
        .unwrap();
    assert_eq!(anchor_changes.len(), 2);
    assert_eq!(
        anchor_changes
            .iter()
            .flat_map(|change| change["edits"].as_array().into_iter().flatten())
            .filter(|edit| edit["newText"] == "first-draft")
            .count(),
        3
    );

    let path_changes = response(&output, 6)["result"]["documentChanges"]
        .as_array()
        .unwrap();
    assert_eq!(path_changes[0]["kind"], "rename");
    assert_eq!(
        path_changes[0]["newUri"],
        root_uri.join("Archived%20Plan.plumb").unwrap().as_str()
    );
    assert_eq!(
        path_changes
            .iter()
            .skip(1)
            .flat_map(|change| change["edits"].as_array().into_iter().flatten())
            .filter(|edit| edit["newText"] == "Archived Plan.plumb")
            .count(),
        2
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn path_rename_is_optimistic_and_reconciles_failed_client_application() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let old_target = root.join("old.plumb");
    let new_target = root.join("new.plumb");
    let source = root.join("source.plumb");
    let target_text = "`#{#target} Target\n";
    let old_source = "See `->[target]{to=\"old.plumb#target\"}.\n";
    let new_source = "See `->[target]{to=\"new.plumb#target\"}.\n";
    std::fs::write(&old_target, target_text).unwrap();
    std::fs::write(&source, old_source).unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let old_uri = lsp_types::Url::from_file_path(&old_target).unwrap();
    let new_uri = lsp_types::Url::from_file_path(&new_target).unwrap();
    let source_uri = lsp_types::Url::from_file_path(&source).unwrap();
    let path_position = old_source.find("old.plumb").unwrap();
    let target_position = old_source.find("#target").unwrap() + 1;
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": { "workspace": { "workspaceEdit": {
                    "documentChanges": true, "resourceOperations": ["rename"]
                } } }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": source_uri, "languageId": "plumb", "version": 1, "text": old_source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/rename",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 0, "character": path_position },
                "newName": "new.plumb"
            }
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": source_uri, "version": 2 },
                "contentChanges": [{ "text": new_source }]
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/definition",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 0, "character": target_position }
            }
        }),
        json!({
            "jsonrpc": "2.0", "method": "workspace/didChangeWatchedFiles",
            "params": { "changes": [{ "uri": old_uri, "type": 2 }] }
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": source_uri, "version": 3 },
                "contentChanges": [{ "text": old_source }]
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/definition",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 0, "character": target_position }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 5, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let output = run_server(&messages);
    assert_eq!(response(&output, 3)["result"]["uri"], new_uri.as_str());
    assert_eq!(response(&output, 4)["result"]["uri"], old_uri.as_str());
    assert!(old_target.exists());
    assert!(!new_target.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn metadata_marker_renames_the_current_document_without_changing_title() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let current = root.join("current.plumb");
    let incoming = root.join("incoming.plumb");
    let current_source = "`meta\n `: title\n\n    Stable title\n";
    std::fs::write(&current, current_source).unwrap();
    std::fs::write(&incoming, "`->[current]{to=\"current.plumb\"}\n").unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let current_uri = lsp_types::Url::from_file_path(&current).unwrap();
    let incoming_uri = lsp_types::Url::from_file_path(&incoming).unwrap();
    let renamed_uri = lsp_types::Url::from_file_path(root.join("renamed.plumb")).unwrap();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": { "workspace": { "workspaceEdit": {
                    "documentChanges": true,
                    "resourceOperations": ["rename"]
                } } }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": current_uri, "languageId": "plumb", "version": 4,
                "text": current_source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/prepareRename",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 0, "character": 2 }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/rename",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 0, "character": 2 },
                "newName": "renamed.plumb"
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let prepare = response(&output, 2);
    assert_eq!(prepare["result"]["placeholder"], "current.plumb");
    assert_eq!(
        prepare["result"]["range"],
        json!({
            "start": { "line": 0, "character": 1 },
            "end": { "line": 0, "character": 5 }
        })
    );

    let operations = response(&output, 3)["result"]["documentChanges"]
        .as_array()
        .unwrap();
    assert_eq!(operations[0]["kind"], "rename");
    assert_eq!(operations[0]["oldUri"], current_uri.as_str());
    assert_eq!(operations[0]["newUri"], renamed_uri.as_str());
    let text_edits = operations.iter().skip(1).collect::<Vec<_>>();
    assert_eq!(text_edits.len(), 1);
    assert_eq!(text_edits[0]["textDocument"]["uri"], incoming_uri.as_str());
    assert_eq!(text_edits[0]["edits"][0]["newText"], "renamed.plumb");
    assert!(operations
        .iter()
        .skip(1)
        .all(|operation| { operation["textDocument"]["uri"] != current_uri.as_str() }));

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn path_rename_watcher_confirms_a_successful_filesystem_rename() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let old_target = root.join("old.plumb");
    let new_target = root.join("new.plumb");
    let source = root.join("source.plumb");
    let target_text = "`#{#target} Target\n";
    let old_source = "See `->[target]{to=\"old.plumb#target\"}.\n";
    let new_source = "See `->[target]{to=\"new.plumb#target\"}.\n";
    std::fs::write(&old_target, target_text).unwrap();
    std::fs::write(&source, old_source).unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let old_uri = lsp_types::Url::from_file_path(&old_target).unwrap();
    let new_uri = lsp_types::Url::from_file_path(&new_target).unwrap();
    let source_uri = lsp_types::Url::from_file_path(&source).unwrap();
    let path_position = old_source.find("old.plumb").unwrap();
    let target_position = new_source.find("#target").unwrap() + 1;
    let first = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": { "workspace": { "workspaceEdit": {
                    "documentChanges": true, "resourceOperations": ["rename"]
                } } }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": source_uri, "languageId": "plumb", "version": 1, "text": old_source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/rename",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 0, "character": path_position },
                "newName": "new.plumb"
            }
        }),
    ];
    let second = [
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": source_uri, "version": 2 },
                "contentChanges": [{ "text": new_source }]
            }
        }),
        json!({
            "jsonrpc": "2.0", "method": "workspace/didChangeWatchedFiles",
            "params": { "changes": [
                { "uri": old_uri, "type": 3 },
                { "uri": new_uri, "type": 1 }
            ] }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/definition",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 0, "character": target_position }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let rename = std::thread::spawn({
        let old_target = old_target.clone();
        let new_target = new_target.clone();
        move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            std::fs::rename(old_target, new_target).unwrap();
        }
    });
    let output = run_server_with_pause(&first, &second);
    rename.join().unwrap();
    assert_eq!(response(&output, 3)["result"]["uri"], new_uri.as_str());
    assert!(!old_target.exists());
    assert!(new_target.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn path_rename_watcher_clears_a_missing_optimistic_target() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let old_target = root.join("old.plumb");
    let new_target = root.join("new.plumb");
    let source = root.join("source.plumb");
    let old_source = "See `->[target]{to=\"old.plumb#target\"}.\n";
    let new_source = "See `->[target]{to=\"new.plumb#target\"}.\n";
    std::fs::write(&old_target, "`#{#target} Target\n").unwrap();
    std::fs::write(&source, old_source).unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let old_uri = lsp_types::Url::from_file_path(&old_target).unwrap();
    let source_uri = lsp_types::Url::from_file_path(&source).unwrap();
    let path_position = old_source.find("old.plumb").unwrap();
    let target_position = new_source.find("#target").unwrap() + 1;
    let first = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": { "workspace": { "workspaceEdit": {
                    "documentChanges": true, "resourceOperations": ["rename"]
                } } }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": source_uri, "languageId": "plumb", "version": 1, "text": old_source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/rename",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 0, "character": path_position },
                "newName": "new.plumb"
            }
        }),
    ];
    let second = [
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": source_uri, "version": 2 },
                "contentChanges": [{ "text": new_source }]
            }
        }),
        json!({
            "jsonrpc": "2.0", "method": "workspace/didChangeWatchedFiles",
            "params": { "changes": [{ "uri": old_uri, "type": 3 }] }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/definition",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 0, "character": target_position }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let remove = std::thread::spawn({
        let old_target = old_target.clone();
        move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            std::fs::remove_file(old_target).unwrap();
        }
    });
    let output = run_server_with_pause(&first, &second);
    remove.join().unwrap();
    assert!(response(&output, 3)["result"].is_null());
    assert!(!old_target.exists());
    assert!(!new_target.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn path_rename_requires_resource_rename_support() {
    let (root, output) =
        run_path_rename_precondition_test(json!({ "documentChanges": true }), "new.plumb", false);
    let error = &response(&output, 2)["error"];
    assert_eq!(error["code"], -32803);
    assert!(error["message"]
        .as_str()
        .unwrap()
        .contains("resourceOperations rename support"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn path_rename_rejects_an_existing_filesystem_target() {
    let (root, output) = run_path_rename_precondition_test(
        json!({ "documentChanges": true, "resourceOperations": ["rename"] }),
        "new.plumb",
        true,
    );
    let error = &response(&output, 2)["error"];
    assert_eq!(error["code"], -32803);
    assert!(error["message"]
        .as_str()
        .unwrap()
        .contains("target already exists"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn path_rename_rejects_a_target_outside_the_workspace() {
    let (root, output) = run_path_rename_precondition_test(
        json!({ "documentChanges": true, "resourceOperations": ["rename"] }),
        "../outside.plumb",
        false,
    );
    let error = &response(&output, 2)["error"];
    assert_eq!(error["code"], -32803);
    assert!(error["message"]
        .as_str()
        .unwrap()
        .contains("outside the workspace"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn definition_and_hover_lazily_load_targets_without_a_workspace_root() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("source.plumb");
    let task_target = root.join("task target.plumb");
    let link_target = root.join("link target.plumb");
    let hover_target = root.join("hover target.plumb");
    let file_target = root.join("file target.plumb");
    let source_text = "`-{.task depends=\"task target.plumb#draft\"} Review\nSee `->[note]{to=\"link target.plumb#note\"}.\nSee `->[hover]{to=\"hover target.plumb#hover\"}.\nSee `->[file]{to=\"file target.plumb\"}.\n";
    std::fs::write(&source, source_text).unwrap();
    std::fs::write(&task_target, "`-{.task #draft} Draft\n").unwrap();
    std::fs::write(&link_target, "`node{#note} Note\n").unwrap();
    std::fs::write(&hover_target, "`node{#hover} Hover\n").unwrap();
    std::fs::write(&file_target, "\n\nFirst content\nSecond content\n").unwrap();
    let source_uri = lsp_types::Url::from_file_path(&source).unwrap();
    let task_uri = lsp_types::Url::from_file_path(&task_target).unwrap();
    let link_uri = lsp_types::Url::from_file_path(&link_target).unwrap();
    let task_position = source_text.lines().next().unwrap().find("#draft").unwrap() + 1;
    let link_position = source_text.lines().nth(1).unwrap().find("#note").unwrap() + 1;
    let hover_position = source_text.lines().nth(2).unwrap().find("#hover").unwrap() + 1;
    let file_position = source_text
        .lines()
        .nth(3)
        .unwrap()
        .find("file target.plumb")
        .unwrap();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
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
                "position": { "line": 0, "character": task_position }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/definition",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 1, "character": link_position }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/hover",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 2, "character": hover_position }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 5, "method": "textDocument/hover",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 0, "character": task_position }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 6, "method": "textDocument/hover",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 3, "character": file_position }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 7, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let output = run_server(&messages);
    assert_eq!(response(&output, 2)["result"]["uri"], task_uri.as_str());
    assert_eq!(response(&output, 3)["result"]["uri"], link_uri.as_str());
    let hover = response(&output, 4)["result"]["contents"]["value"]
        .as_str()
        .unwrap();
    assert!(hover.contains("#hover"));
    assert!(hover.contains("hover target.plumb"));
    assert!(hover.contains("`node{#hover} Hover"));
    let task_reference_hover = response(&output, 5)["result"]["contents"]["value"]
        .as_str()
        .unwrap();
    assert!(task_reference_hover.starts_with("**Anchor** `#draft`"));
    assert!(task_reference_hover.contains("`-{.task #draft} Draft"));
    let file_hover = response(&output, 6)["result"]["contents"]["value"]
        .as_str()
        .unwrap();
    assert!(file_hover.starts_with("**File**"));
    assert!(file_hover.contains(":3`"));
    assert!(file_hover.contains("First content\nSecond content"));
    std::fs::remove_dir_all(root).unwrap();
}

fn run_path_rename_precondition_test(
    workspace_edit: Value,
    new_name: &str,
    create_target: bool,
) -> (PathBuf, Vec<Value>) {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let old_target = root.join("old.plumb");
    let source = root.join("source.plumb");
    let source_text = "See `->[target]{to=\"old.plumb#target\"}.\n";
    std::fs::write(&old_target, "`#{#target} Target\n").unwrap();
    std::fs::write(&source, source_text).unwrap();
    if create_target {
        std::fs::write(root.join(new_name), "Already here.\n").unwrap();
    }
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let source_uri = lsp_types::Url::from_file_path(&source).unwrap();
    let path_position = source_text.find("old.plumb").unwrap();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": { "workspace": { "workspaceEdit": workspace_edit } }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": source_uri, "languageId": "plumb", "version": 1, "text": source_text
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/rename",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 0, "character": path_position },
                "newName": new_name
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    (root, run_server(&messages))
}
