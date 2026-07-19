use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

#[test]
fn publishes_diagnostics_and_returns_heading_symbols_over_stdio() {
    let messages = [
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": "file:///tmp/first.plumb",
                    "languageId": "plumb",
                    "version": 1,
                    "text": "`# Root\n`## Child\n"
                }
            }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "textDocument/documentSymbol",
            "params": { "textDocument": { "uri": "file:///tmp/first.plumb" } }
        }),
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": "file:///tmp/first.plumb", "version": 2 },
                "contentChanges": [{ "text": "`node{key=a key=b} Broken\n" }]
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let messages = run_server(&messages);
    let capabilities = &response(&messages, 1)["result"]["capabilities"];
    assert_eq!(
        capabilities["codeActionProvider"]["codeActionKinds"],
        json!(["quickfix", "refactor.rewrite"])
    );
    assert!(capabilities["completionProvider"]["triggerCharacters"]
        .as_array()
        .unwrap()
        .contains(&json!("[")));
    let symbols = messages
        .iter()
        .find(|message| message.get("id") == Some(&json!(2)))
        .expect("documentSymbol response");
    assert_eq!(symbols["result"][0]["name"], "Root");
    assert_eq!(symbols["result"][0]["children"][0]["name"], "Child");

    let diagnostics = messages
        .iter()
        .filter(|message| message.get("method") == Some(&json!("textDocument/publishDiagnostics")))
        .last()
        .expect("diagnostics notification");
    assert_eq!(diagnostics["params"]["version"], 2);
    assert_eq!(
        diagnostics["params"]["diagnostics"][0]["code"],
        "syntax.duplicate-key"
    );
}

#[test]
fn publishes_metadata_diagnostics_and_nested_symbols_over_stdio() {
    let source = "`meta\n  `: title\n\n    Document title\n\n  `: author\n    `: name\n\n      Alice\n\n  `: title\n\nInvalid `cite[@old-style].\n";
    let messages = [
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": "file:///tmp/metadata.plumb",
                    "languageId": "plumb",
                    "version": 1,
                    "text": source
                }
            }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "textDocument/documentSymbol",
            "params": { "textDocument": { "uri": "file:///tmp/metadata.plumb" } }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let symbols = response(&output, 2);
    let metadata = symbols["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|symbol| symbol["name"] == "metadata")
        .expect("metadata symbol");
    assert_eq!(metadata["children"][0]["name"], "title");
    assert_eq!(metadata["children"][1]["name"], "author");
    assert_eq!(metadata["children"][1]["children"][0]["name"], "name");

    let diagnostics = output
        .iter()
        .filter(|message| message.get("method") == Some(&json!("textDocument/publishDiagnostics")))
        .last()
        .expect("diagnostics notification");
    assert!(diagnostics["params"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|diagnostic| diagnostic["code"] == "metadata.duplicate-key"));
    assert!(diagnostics["params"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|diagnostic| diagnostic["code"] == "citation.invalid"));
}

#[test]
fn inserts_metadata_code_action_only_for_valid_documents_without_metadata() {
    let uri = "file:///tmp/metadata-action.plumb";
    let messages = [
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": null,
                "capabilities": {
                    "workspace": { "workspaceEdit": { "documentChanges": true } }
                }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 3, "text": "`# Section\n"
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 0 }
                },
                "context": { "diagnostics": [], "only": ["refactor"] }
            }
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 4 },
                "contentChanges": [{
                    "text": "`meta\n  `: title\n\n    Existing\n\n`# Section\n"
                }]
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 0 }
                },
                "context": { "diagnostics": [] }
            }
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 5 },
                "contentChanges": [{ "text": "`node{key=a key=b} Broken\n" }]
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 0 }
                },
                "context": { "diagnostics": [] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 5, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    assert!(response(&output, 1)["result"]["capabilities"]["codeActionProvider"]
        ["codeActionKinds"]
        .as_array()
        .unwrap()
        .contains(&json!("refactor.rewrite")));
    let actions = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0]["title"], "Insert document metadata");
    assert_eq!(actions[0]["kind"], "refactor.rewrite");
    let change = &actions[0]["edit"]["documentChanges"][0];
    assert_eq!(change["textDocument"]["version"], 3);
    assert_eq!(change["edits"][0]["range"]["start"]["line"], 0);
    assert_eq!(change["edits"][0]["range"]["start"]["character"], 0);
    let new_text = change["edits"][0]["newText"].as_str().unwrap();
    let prefix = "`meta\n  `: title\n\n     metadata-action\n\n  `: created\n\n     ";
    let created = new_text
        .strip_prefix(prefix)
        .and_then(|suffix| suffix.strip_suffix("\n\n"))
        .expect("metadata contains created after title");
    chrono::DateTime::parse_from_rfc3339(created).expect("created is an RFC 3339 timestamp");
    assert!(response(&output, 3)["result"].is_null());
    assert!(response(&output, 4)["result"].is_null());
}

#[test]
fn omits_metadata_code_action_without_guarded_edit_support() {
    let uri = "file:///tmp/no-guarded-edits.plumb";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 1, "text": "Content\n"
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 0 }
                },
                "context": { "diagnostics": [] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    assert!(response(&output, 2)["result"].is_null());
}

#[test]
fn offers_guarded_task_status_code_actions() {
    let uri = "file:///tmp/task-actions.plumb";
    let source = "`item{.task #write} Write parser\n";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": null,
                "capabilities": {
                    "workspace": { "workspaceEdit": { "documentChanges": true } }
                }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 3, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 25 },
                    "end": { "line": 0, "character": 25 }
                },
                "context": { "diagnostics": [], "only": ["quickfix"] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let actions = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0]["title"], "Complete task");
    assert_eq!(actions[1]["title"], "Cancel task");
    for (action, attribute) in actions.iter().zip(["done", "canceled"]) {
        assert_eq!(action["kind"], "quickfix");
        let change = &action["edit"]["documentChanges"][0];
        assert_eq!(change["textDocument"]["version"], 3);
        let new_text = change["edits"][0]["newText"].as_str().unwrap();
        let timestamp = new_text
            .strip_prefix(&format!(" {attribute}=\""))
            .and_then(|value| value.strip_suffix('"'))
            .unwrap();
        chrono::DateTime::parse_from_rfc3339(timestamp).unwrap();
    }
}

#[test]
fn recurring_task_action_closes_current_and_appends_next_instance() {
    let uri = "file:///tmp/recurring-task.plumb";
    let source = "`item{.task due=\"2026-07-20T09:00:00+08:00\" recur=P1W} Weekly review\n";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": null,
                "capabilities": {
                    "workspace": { "workspaceEdit": { "documentChanges": true } }
                }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 2, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 68 },
                    "end": { "line": 0, "character": 68 }
                },
                "context": { "diagnostics": [], "only": ["quickfix"] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let actions = response(&output, 2)["result"].as_array().unwrap();
    let complete = actions
        .iter()
        .find(|action| action["title"] == "Complete task")
        .unwrap();
    let edits = complete["edit"]["documentChanges"][0]["edits"]
        .as_array()
        .unwrap();
    assert_eq!(edits.len(), 2);
    assert!(edits[0]["newText"]
        .as_str()
        .unwrap()
        .contains("#weekly-review-2026-07-20 done="));
    let next = edits[1]["newText"].as_str().unwrap();
    assert!(next.contains("#weekly-review-2026-07-27"));
    assert!(next.contains("due=\"2026-07-27T09:00:00+08:00\""));
    assert!(next.contains("prev=\"#weekly-review-2026-07-20\""));
}

#[test]
fn task_actions_fall_back_from_closed_child_to_open_parent() {
    let uri = "file:///tmp/nested-task-actions.plumb";
    let source =
        "`item{.task #outer} Outer\n  `item{.task #inner done=\"2026-07-20T09:00:00Z\"} Inner\n";
    let cursor = source.find("Inner").unwrap();
    let line_start = source.find('\n').unwrap() + 1;
    let character = cursor - line_start;
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": null,
                "capabilities": {
                    "workspace": { "workspaceEdit": { "documentChanges": true } }
                }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 1, "character": character },
                    "end": { "line": 1, "character": character }
                },
                "context": { "diagnostics": [], "only": ["quickfix"] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let output = run_server(&messages);
    let actions = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(actions.len(), 2);
    for action in actions {
        let edit = &action["edit"]["documentChanges"][0]["edits"][0];
        assert_eq!(edit["range"]["start"]["line"], 0);
        assert!(edit["newText"].as_str().unwrap().contains("2026-"));
    }
}

#[test]
fn publishes_task_symbols_hover_and_workspace_diagnostics() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let blockers_path = root.join("blockers.plumb");
    let tasks_path = root.join("tasks.plumb");
    let blocker_source = "`item{.task #draft} Draft dependency\n";
    let task_source = "`item{.task #review due=\"not-a-date\" recur=P1M1D depends=\"blockers.plumb#draft\"} Review task\n  `item{.task #nested done=\"2026-07-20T10:00:00Z\"} Nested task\n";
    std::fs::write(&blockers_path, blocker_source).unwrap();
    std::fs::write(&tasks_path, task_source).unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let tasks_uri = lsp_types::Url::from_file_path(&tasks_path).unwrap();
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
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": tasks_uri, "languageId": "plumb", "version": 3, "text": task_source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/documentSymbol",
            "params": { "textDocument": { "uri": tasks_uri } }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/hover",
            "params": {
                "textDocument": { "uri": tasks_uri },
                "position": { "line": 0, "character": 1 }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/semanticTokens/full",
            "params": { "textDocument": { "uri": tasks_uri } }
        }),
        json!({ "jsonrpc": "2.0", "id": 5, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    assert_eq!(
        response(&output, 1)["result"]["capabilities"]["semanticTokensProvider"]["legend"]
            ["tokenTypes"][0],
        "task"
    );
    let symbols = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0]["name"], "Review task");
    assert_eq!(symbols[0]["detail"], "open #review");
    assert_eq!(symbols[0]["children"][0]["name"], "Nested task");
    assert_eq!(symbols[0]["children"][0]["detail"], "done #nested");

    let hover = response(&output, 3)["result"]["contents"]["value"]
        .as_str()
        .unwrap();
    assert!(hover.contains("**State:** blocked"));
    assert!(hover.contains("**Recur:** `P1M1D`"));
    assert!(hover.contains("**Depends:** `blockers.plumb#draft`"));
    assert!(hover.contains("**Open blockers:** `blockers.plumb#draft`"));

    let semantic_data = response(&output, 4)["result"]["data"].as_array().unwrap();
    assert_eq!(semantic_data.len(), 5);
    assert_eq!(semantic_data[3], 0);
    assert_eq!(semantic_data[4], 1);

    let diagnostics = output
        .iter()
        .filter(|message| message.get("method") == Some(&json!("textDocument/publishDiagnostics")))
        .last()
        .unwrap();
    let diagnostics = diagnostics["params"]["diagnostics"].as_array().unwrap();
    assert!(diagnostics
        .iter()
        .any(|diagnostic| diagnostic["code"] == "task.invalid-recur"));
    assert!(diagnostics
        .iter()
        .any(|diagnostic| diagnostic["code"] == "task.missing-due-for-recur"));
    let blocked = diagnostics
        .iter()
        .find(|diagnostic| diagnostic["code"] == "task.blocked")
        .unwrap();
    assert_eq!(blocked["severity"], 4);

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn resolves_cross_file_navigation_over_stdio() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let target = root.join("a.plumb");
    let source = root.join("b.plumb");
    std::fs::write(&target, "`#{#target} Target\n").unwrap();
    let source_text = "See `link[target]{to=\"a.plumb#target\"}.\n";
    std::fs::write(&source, source_text).unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let target_uri = lsp_types::Url::from_file_path(&target).unwrap();
    let source_uri = lsp_types::Url::from_file_path(&source).unwrap();

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
                "uri": source_uri, "languageId": "plumb", "version": 1, "text": source_text
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/definition",
            "params": { "textDocument": { "uri": source_uri }, "position": { "line": 0, "character": 10 } }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/references",
            "params": {
                "textDocument": { "uri": target_uri },
                "position": { "line": 0, "character": 4 },
                "context": { "includeDeclaration": false }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/hover",
            "params": { "textDocument": { "uri": source_uri }, "position": { "line": 0, "character": 10 } }
        }),
        json!({
            "jsonrpc": "2.0", "id": 8, "method": "textDocument/completion",
            "params": { "textDocument": { "uri": source_uri }, "position": { "line": 0, "character": 32 } }
        }),
        json!({
            "jsonrpc": "2.0", "id": 5, "method": "textDocument/prepareRename",
            "params": { "textDocument": { "uri": source_uri }, "position": { "line": 0, "character": 32 } }
        }),
        json!({
            "jsonrpc": "2.0", "id": 6, "method": "textDocument/rename",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 0, "character": 32 },
                "newName": "renamed"
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 9, "method": "textDocument/prepareRename",
            "params": { "textDocument": { "uri": source_uri }, "position": { "line": 0, "character": 24 } }
        }),
        json!({
            "jsonrpc": "2.0", "id": 10, "method": "textDocument/rename",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 0, "character": 24 },
                "newName": "moved.plumb"
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 7, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let output = run_server(&messages);
    let definition = response(&output, 2);
    assert_eq!(definition["result"]["uri"], target_uri.as_str());
    let references = response(&output, 3);
    assert_eq!(references["result"].as_array().unwrap().len(), 1);
    assert_eq!(references["result"][0]["uri"], source_uri.as_str());
    let hover = response(&output, 4);
    assert!(hover["result"]["contents"]["value"]
        .as_str()
        .unwrap()
        .contains("#target"));
    let prepare = response(&output, 5);
    assert_eq!(prepare["result"]["placeholder"], "target");
    let rename = response(&output, 6);
    let changes = rename["result"]["documentChanges"].as_array().unwrap();
    assert_eq!(changes.len(), 2);
    assert!(changes
        .iter()
        .all(|change| change["edits"][0]["newText"] == "renamed"));
    let completion = response(&output, 8);
    assert_eq!(completion["result"][0]["label"], "#target");
    assert_eq!(completion["result"][0]["textEdit"]["newText"], "target");
    let path_prepare = response(&output, 9);
    assert_eq!(path_prepare["result"]["placeholder"], "a.plumb");
    let path_rename = response(&output, 10);
    let operations = path_rename["result"]["documentChanges"].as_array().unwrap();
    assert_eq!(operations[0]["kind"], "rename");
    assert!(operations
        .iter()
        .skip(1)
        .flat_map(|operation| operation["edits"].as_array().into_iter().flatten())
        .any(|edit| edit["newText"] == "moved.plumb"));

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn completes_links_by_document_metadata_title() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("current.plumb");
    let target = root.join("usage.plumb");
    let source_text = "`link[Us\n`link[x]{to=\"Guide\n";
    std::fs::write(&source, source_text).unwrap();
    std::fs::write(
        &target,
        "`meta\n  `: title\n\n    Usage Guide\n\n`# Usage\n",
    )
    .unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let source_uri = lsp_types::Url::from_file_path(&source).unwrap();
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
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": source_uri, "languageId": "plumb", "version": 1, "text": source_text
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 0, "character": 8 }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 1, "character": 18 }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let output = run_server(&messages);
    let label = &response(&output, 2)["result"][0];
    assert_eq!(label["label"], "Usage Guide");
    assert_eq!(label["detail"], "usage.plumb");
    assert_eq!(
        label["textEdit"]["newText"],
        "`link[Usage Guide]{to=\"usage.plumb\"}"
    );
    let path = &response(&output, 3)["result"][0];
    assert_eq!(path["label"], "usage.plumb");
    assert_eq!(path["detail"], "Usage Guide");
    assert_eq!(path["textEdit"]["newText"], "usage.plumb");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn task_references_support_navigation_and_rename() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let target = root.join("Project Plan.plumb");
    let source = root.join("review.plumb");
    let target_text = "`item{.task #draft} Draft\n";
    let source_text = "`item{.task #review prev=\"Project%20Plan.plumb#draft\" depends=\"Project%20Plan.plumb#draft\"} Review\n";
    std::fs::write(&target, target_text).unwrap();
    std::fs::write(&source, source_text).unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let target_uri = lsp_types::Url::from_file_path(&target).unwrap();
    let source_uri = lsp_types::Url::from_file_path(&source).unwrap();
    let target_id = target_text.find("#draft").unwrap() + 1;
    let prev_id = source_text.find("#draft").unwrap() + 1;
    let depends_start = source_text.find("depends=").unwrap();
    let depends_id = depends_start + source_text[depends_start..].find("#draft").unwrap() + 1;
    let task_path = source_text.find("Project%20Plan.plumb").unwrap();
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
            .filter(|edit| edit["newText"] == "Archived%20Plan.plumb")
            .count(),
        2
    );
    std::fs::remove_dir_all(root).unwrap();
}

fn run_server(messages: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_plumb-ls"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start plumb-ls");
    {
        let stdin = child.stdin.as_mut().expect("child stdin");
        for message in messages {
            write_message(stdin, message);
        }
    }
    drop(child.stdin.take());
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .expect("child stdout")
        .read_to_string(&mut stdout)
        .expect("read stdout");
    let output = child.wait_with_output().expect("wait for plumb-ls");
    assert!(
        output.status.success(),
        "plumb-ls failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    read_messages(&stdout)
}

fn response(messages: &[Value], id: u64) -> &Value {
    messages
        .iter()
        .find(|message| message.get("id") == Some(&json!(id)))
        .expect("response")
}

fn unique_temp_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "plumb-ls-test-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

fn write_message(output: &mut impl Write, message: &Value) {
    let body = serde_json::to_vec(message).expect("encode message");
    write!(output, "Content-Length: {}\r\n\r\n", body.len()).expect("write header");
    output.write_all(&body).expect("write body");
    output.flush().expect("flush message");
}

fn read_messages(mut input: &str) -> Vec<Value> {
    let mut messages = Vec::new();
    while let Some(header_end) = input.find("\r\n\r\n") {
        let header = &input[..header_end];
        let length = header
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .expect("content length")
            .parse::<usize>()
            .expect("numeric content length");
        let body_start = header_end + 4;
        let body_end = body_start + length;
        messages.push(serde_json::from_str(&input[body_start..body_end]).expect("JSON-RPC body"));
        input = &input[body_end..];
    }
    messages
}
