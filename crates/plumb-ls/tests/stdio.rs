use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

#[test]
fn formats_valid_documents_and_declines_invalid_revisions() {
    let uri = "file:///tmp/format.plumb";
    let source = "`meta\n   `: title\n\n      Example\n";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/formatting",
            "params": {
                "textDocument": { "uri": uri },
                "options": { "tabSize": 4, "insertSpaces": false }
            }
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [{ "text": "`span[open\n" }]
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/formatting",
            "params": {
                "textDocument": { "uri": uri },
                "options": { "tabSize": 2, "insertSpaces": true }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    assert_eq!(
        response(&output, 1)["result"]["capabilities"]["documentFormattingProvider"],
        true
    );
    assert_eq!(
        response(&output, 2)["result"][0]["newText"],
        "`meta\n `: title\n\n    Example\n"
    );
    assert_eq!(
        response(&output, 2)["result"][0]["range"]["end"],
        json!({ "line": 4, "character": 0 })
    );
    assert!(response(&output, 3)["result"].is_null());
}

#[test]
fn range_formatting_formats_only_complete_contained_blocks() {
    let uri = "file:///tmp/range-format.plumb";
    let source = "`node Parent\r\n       `-{.task\r\n          #一\r\n        } One\r\n\r\n       `-{.task #二} Two\r\n\r\n`# Following\r\n";
    let child_end = source.lines().nth(5).unwrap().encode_utf16().count() as u32;
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/rangeFormatting",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 1, "character": 7 },
                    "end": { "line": 5, "character": child_end }
                },
                "options": { "tabSize": 2, "insertSpaces": true }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/rangeFormatting",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 3, "character": 10 },
                    "end": { "line": 3, "character": 10 }
                },
                "options": { "tabSize": 4, "insertSpaces": false }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/rangeFormatting",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 3, "character": 10 },
                    "end": { "line": 3, "character": 13 }
                },
                "options": { "tabSize": 4, "insertSpaces": true }
            }
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [{ "text": "`span[open\n" }]
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 5, "method": "textDocument/rangeFormatting",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 10 }
                },
                "options": { "tabSize": 4, "insertSpaces": true }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 6, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    assert_eq!(
        response(&output, 1)["result"]["capabilities"]["documentRangeFormattingProvider"],
        true
    );
    let edits = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(edits.len(), 1);
    assert_eq!(
        edits[0]["range"]["start"],
        json!({ "line": 1, "character": 7 })
    );
    assert_eq!(
        edits[0]["newText"],
        "`-{.task #一} One\r\n       `-{.task #二} Two"
    );
    assert_eq!(response(&output, 3)["result"], json!([]));
    assert_eq!(response(&output, 4)["result"], json!([]));
    assert!(response(&output, 5)["result"].is_null());
}

#[test]
fn range_formatting_returns_multiple_maximal_groups() {
    let uri = "file:///tmp/range-format-groups.plumb";
    let source = "`node First\n  `-{.task\n      #one\n    } One\n`node Second\n  `-{.task\n      #two\n    } Two\n";
    let end_character = source.lines().nth(7).unwrap().encode_utf16().count() as u32;
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/rangeFormatting",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 1, "character": 2 },
                    "end": { "line": 7, "character": end_character }
                },
                "options": { "tabSize": 4, "insertSpaces": true }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let edits = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(edits.len(), 2);
    assert_eq!(
        edits[0]["range"]["start"],
        json!({ "line": 1, "character": 2 })
    );
    assert_eq!(edits[0]["newText"], "`-{.task #one} One");
    assert_eq!(
        edits[1]["range"]["start"],
        json!({ "line": 4, "character": 0 })
    );
    assert!(edits[1]["newText"]
        .as_str()
        .unwrap()
        .starts_with("`node Second\n"));
}

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
    let watcher = &registration["params"]["registrations"][0]["registerOptions"]["watchers"][0];
    assert_eq!(watcher["globPattern"], "**/*.plumb");
    assert_eq!(watcher["kind"], 7);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn watched_file_create_indexes_the_new_document() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let target = root.join("topic.plumb");
    let source = root.join("index.plumb");
    std::fs::write(&target, "`#{#topic} Topic\n").unwrap();
    let source_text = "See `->[topic]{to=\"topic.plumb#topic\"}.\n";
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
fn diagnostics_clear_after_a_link_is_fixed() {
    let uri = "file:///tmp/fix-link.plumb";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 1,
                "text": "See `->[missing]{to=\"#missing\"}.\n"
            }}
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [{
                    "text": "`node{#target} Target\nSee `->[target]{to=\"#target\"}.\n"
                }]
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    assert_eq!(diagnostic_counts(&run_server(&messages), uri), [1, 0]);
}

#[test]
fn diagnostics_refresh_when_a_target_document_changes() {
    let source_uri = "file:///tmp/diagnostic-source.plumb";
    let target_uri = "file:///tmp/diagnostic-target.plumb";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": source_uri, "languageId": "plumb", "version": 1,
                "text": "See `->[target]{to=\"diagnostic-target.plumb#target\"}.\n"
            }}
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": target_uri, "languageId": "plumb", "version": 1,
                "text": "No anchor yet.\n"
            }}
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": target_uri, "version": 2 },
                "contentChanges": [{ "text": "`node{#target} Target\n" }]
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    assert_eq!(
        diagnostic_counts(&run_server(&messages), source_uri),
        [1, 1, 0]
    );
}

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
    assert!(capabilities["completionProvider"]["triggerCharacters"]
        .as_array()
        .unwrap()
        .contains(&json!("`")));
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
fn nests_anchors_and_tasks_under_their_containing_headings() {
    let uri = "file:///tmp/symbol-containment.plumb";
    let source = "`# Project\n`node{#note} Note\n`-{.task #write} Write parser\n`## Section\n`node{#inside} Inside\n";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/documentSymbol",
            "params": { "textDocument": { "uri": uri } }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let roots = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0]["name"], "Project");
    let children = roots[0]["children"].as_array().unwrap();
    assert_eq!(children.len(), 3);
    assert_eq!(children[0]["name"], "#note");
    assert_eq!(children[1]["name"], "Write parser");
    assert_eq!(children[1]["detail"], "open #write");
    assert_eq!(children[2]["name"], "Section");
    assert_eq!(children[2]["children"][0]["name"], "#inside");
}

#[test]
fn publishes_metadata_diagnostics_and_nested_symbols_over_stdio() {
    let source = "`meta\n  `: title\n\n    Document title\n\n  `: author\n    `: name\n\n      Alice\n\n  `: title\n\n  `: created\n\n    yesterday\n\nInvalid `cite[@old-style].\n";
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
    let invalid_created = diagnostics["params"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|diagnostic| diagnostic["code"] == "metadata.invalid-created")
        .expect("invalid created diagnostic");
    assert_eq!(invalid_created["severity"], 2);
    assert_eq!(
        invalid_created["range"]["start"],
        json!({ "line": 14, "character": 4 })
    );
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
    assert!(
        response(&output, 1)["result"]["capabilities"]["codeActionProvider"]["codeActionKinds"]
            .as_array()
            .unwrap()
            .contains(&json!("refactor.rewrite"))
    );
    let actions = response(&output, 2)["result"].as_array().unwrap();
    let metadata = actions
        .iter()
        .find(|action| action["title"] == "Insert document metadata")
        .unwrap();
    assert_eq!(metadata["kind"], "refactor.rewrite");
    let change = &metadata["edit"]["documentChanges"][0];
    assert_eq!(change["textDocument"]["version"], 3);
    assert_eq!(change["edits"][0]["range"]["start"]["line"], 0);
    assert_eq!(change["edits"][0]["range"]["start"]["character"], 0);
    let new_text = change["edits"][0]["newText"].as_str().unwrap();
    let prefix = "`meta\n `: title\n\n    metadata-action\n\n `: created\n\n    ";
    let created = new_text
        .strip_prefix(prefix)
        .and_then(|suffix| suffix.strip_suffix("\n\n"))
        .expect("metadata contains created after title");
    chrono::DateTime::parse_from_rfc3339(created).expect("created is an RFC 3339 timestamp");
    assert!(response(&output, 3)["result"]
        .as_array()
        .unwrap()
        .iter()
        .all(|action| action["title"] != "Insert document metadata"));
    assert!(response(&output, 4)["result"].is_null());
}

#[test]
fn inserts_metadata_into_an_empty_document_over_stdio() {
    let uri = "file:///tmp/empty-note.plumb";
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
                "uri": uri, "languageId": "plumb", "version": 7, "text": ""
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
                "context": { "diagnostics": [], "only": ["refactor.rewrite"] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let metadata = response(&output, 2)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == "Insert document metadata")
        .expect("metadata action");
    let change = &metadata["edit"]["documentChanges"][0];
    assert_eq!(change["textDocument"]["version"], 7);
    assert_eq!(
        change["edits"][0]["range"]["start"],
        json!({ "line": 0, "character": 0 })
    );
    assert_eq!(
        change["edits"][0]["range"]["end"],
        json!({ "line": 0, "character": 0 })
    );
    let generated = change["edits"][0]["newText"].as_str().unwrap();
    assert!(generated.starts_with("`meta\n `: title\n\n    empty-note\n"));
    assert_eq!(plumb_format::format(generated).unwrap(), generated);
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
fn offers_add_explicit_id_for_the_deepest_unanchored_block() {
    let uri = "file:///tmp/add-explicit-id.plumb";
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
                "uri": uri, "languageId": "plumb", "version": 4,
                "text": "`#{#same-title} Existing\n`node Parent\n  `child 😀 Same title\n"
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 2, "character": 14 },
                    "end": { "line": 2, "character": 14 }
                },
                "context": { "diagnostics": [], "only": ["refactor.rewrite"] }
            }
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 5 },
                "contentChanges": [{
                    "text": "`#{#same-title} Existing\n`node Parent\n  `child{#nested} 😀 Same title\n"
                }]
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 2, "character": 23 },
                    "end": { "line": 2, "character": 23 }
                },
                "context": { "diagnostics": [], "only": ["refactor.rewrite"] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let action = response(&output, 2)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == "Add explicit id")
        .unwrap();
    assert_eq!(action["kind"], "refactor.rewrite");
    assert_eq!(action["isPreferred"], true);
    let change = &action["edit"]["documentChanges"][0];
    assert_eq!(change["textDocument"]["version"], 4);
    assert!(change["edits"][0]["newText"]
        .as_str()
        .unwrap()
        .contains("`child{#same-title-2} 😀 Same title"));
    assert_eq!(change["edits"][0]["range"]["start"]["line"], 2);
    assert_eq!(change["edits"][0]["range"]["start"]["character"], 0);

    assert!(response(&output, 3)["result"]
        .as_array()
        .unwrap()
        .iter()
        .all(|action| action["title"] != "Add explicit id"));
}

#[test]
fn offers_task_authoring_refactor_actions() {
    let uri = "file:///tmp/task-authoring.plumb";
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
                "uri": uri, "languageId": "plumb", "version": 1,
                "text": "`-{#keep .kind} List item\n"
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 20 },
                    "end": { "line": 0, "character": 20 }
                },
                "context": { "diagnostics": [], "only": ["refactor.rewrite"] }
            }
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [{
                    "text": "`.{.task #closed done=\"2026-07-20T09:00:00Z\"} Closed\n"
                }]
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 55 },
                    "end": { "line": 0, "character": 55 }
                },
                "context": { "diagnostics": [], "only": ["refactor.rewrite"] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let conversion = response(&output, 2)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == "Convert to task")
        .unwrap();
    assert_eq!(conversion["kind"], "refactor.rewrite");
    let inserted = conversion["edit"]["documentChanges"][0]["edits"][0]["newText"]
        .as_str()
        .unwrap();
    assert!(inserted.contains(".task"));
    chrono::DateTime::parse_from_rfc3339(attribute_value(inserted, "created")).unwrap();

    let created = response(&output, 3)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == "Add task created timestamp")
        .unwrap();
    assert_eq!(created["kind"], "refactor.rewrite");
    let created_text = created["edit"]["documentChanges"][0]["edits"][0]["newText"]
        .as_str()
        .unwrap();
    chrono::DateTime::parse_from_rfc3339(attribute_value(created_text, "created")).unwrap();
}

#[test]
fn offers_guarded_task_status_code_actions() {
    let uri = "file:///tmp/task-actions.plumb";
    let source = "`-{#task-f81deb18 .task created=\"2026-05-24T02:35:50Z\"} MJCF in, USD out solver\n   `-{#task-c2cf5756 .task created=\"2026-05-27T13:03:04Z\"} parse MJCF\n   `-{#task-99e28dad .task created=\"2026-05-27T13:02:45Z\"} solver with passive joint\n";
    let character = source.lines().nth(1).unwrap().find("parse MJCF").unwrap();
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
    assert_eq!(actions[0]["title"], "Complete task");
    assert_eq!(actions[1]["title"], "Cancel task");
    for (action, attribute) in actions.iter().zip(["done", "canceled"]) {
        assert_eq!(action["kind"], "quickfix");
        let change = &action["edit"]["documentChanges"][0];
        assert_eq!(change["textDocument"]["version"], 3);
        let new_text = change["edits"][0]["newText"].as_str().unwrap();
        assert!(new_text.contains("#task-c2cf5756"));
        assert!(!new_text.contains("#task-f81deb18"));
        assert!(!new_text.contains("#task-99e28dad"));
        let timestamp = attribute_value(new_text, attribute);
        chrono::DateTime::parse_from_rfc3339(timestamp).unwrap();
    }
}

#[test]
fn recurring_task_action_closes_current_and_appends_next_instance() {
    let uri = "file:///tmp/recurring-task.plumb";
    let source = "`-{.task due=\"2026-07-20T09:00:00+08:00\" recur=P1W} Weekly review\n";
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
    assert_eq!(edits.len(), 1);
    let replacement = edits[0]["newText"].as_str().unwrap();
    assert!(replacement.contains("#weekly-review-2026-07-20"));
    assert!(replacement.contains("done="));
    assert!(replacement.contains("#weekly-review-2026-07-27"));
    assert!(replacement.contains("due=\"2026-07-27T09:00:00+08:00\""));
    assert!(replacement.contains("prev=\"#weekly-review-2026-07-20\""));
}

#[test]
fn blocked_task_offers_cancel_but_not_complete() {
    let uri = "file:///tmp/blocked-task-actions.plumb";
    let source = "`-{.task #draft} Draft\n`-{.task #review depends=\"#draft\"} Review\n";
    let cursor = source.find("Review").unwrap();
    let line_start = source.find('\n').unwrap() + 1;
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
                    "start": { "line": 1, "character": cursor - line_start },
                    "end": { "line": 1, "character": cursor - line_start }
                },
                "context": { "diagnostics": [], "only": ["quickfix"] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let actions = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0]["title"], "Cancel task");
    let new_text = actions[0]["edit"]["documentChanges"][0]["edits"][0]["newText"]
        .as_str()
        .unwrap();
    chrono::DateTime::parse_from_rfc3339(attribute_value(new_text, "canceled")).unwrap();
}

#[test]
fn canceling_a_recurring_task_appends_the_next_instance() {
    let uri = "file:///tmp/cancel-recurring-task.plumb";
    let source = "`-{.task due=\"2026-07-20T09:00:00+08:00\" recur=P1W} Weekly review\n";
    let cursor = source.find("Weekly review").unwrap();
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
                "uri": uri, "languageId": "plumb", "version": 4, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": cursor },
                    "end": { "line": 0, "character": cursor }
                },
                "context": { "diagnostics": [], "only": ["quickfix"] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let cancel = response(&output, 2)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == "Cancel task")
        .expect("Cancel task action");
    let edits = cancel["edit"]["documentChanges"][0]["edits"]
        .as_array()
        .unwrap();
    assert_eq!(edits.len(), 1);
    let replacement = edits[0]["newText"].as_str().unwrap();
    assert!(replacement.contains("#weekly-review-2026-07-20"));
    assert!(replacement.contains("canceled="));
    assert!(replacement.contains("#weekly-review-2026-07-27"));
    assert!(replacement.contains("due=\"2026-07-27T09:00:00+08:00\""));
    assert!(replacement.contains("prev=\"#weekly-review-2026-07-20\""));
}

#[test]
fn task_actions_fall_back_from_closed_child_to_open_parent() {
    let uri = "file:///tmp/nested-task-actions.plumb";
    let source = "`-{.task #outer} Outer\n  `-{.task #inner done=\"2026-07-20T09:00:00Z\"} Inner\n";
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
    let blocker_source = "`-{.task #draft} Draft dependency\n";
    let task_source = "`-{.task #review due=\"not-a-date\" recur=P1M1D depends=\"blockers.plumb#draft\"} Review task\n  `-{.task #nested done=\"2026-07-20T10:00:00Z\"} Nested task\n`note{.task} Invalid owner\n`span[not raw]{.$}\n";
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
    let invalid_due = diagnostics
        .iter()
        .find(|diagnostic| diagnostic["code"] == "task.invalid-datetime")
        .unwrap();
    let invalid_due_start = task_source.find("\"not-a-date\"").unwrap();
    assert_eq!(
        invalid_due["range"],
        json!({
            "start": { "line": 0, "character": invalid_due_start },
            "end": { "line": 0, "character": invalid_due_start + "\"not-a-date\"".len() }
        })
    );
    assert!(!diagnostics
        .iter()
        .any(|diagnostic| diagnostic["code"] == "task.missing-due-for-recur"));
    assert!(diagnostics
        .iter()
        .any(|diagnostic| diagnostic["code"] == "task.invalid-owner"));
    assert!(diagnostics
        .iter()
        .any(|diagnostic| diagnostic["code"] == "math.invalid-owner"));
    let blocked = diagnostics
        .iter()
        .find(|diagnostic| diagnostic["code"] == "task.blocked")
        .unwrap();
    assert_eq!(blocked["severity"], 4);

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn highlights_closed_tasks_with_multiline_attributes() {
    let uri = "file:///tmp/multiline-closed-tasks.plumb";
    let source = "`-{\n   .task\n   done=\"2026-07-20T10:00:00Z\"\n  } Done\n`-{\n   .task\n   canceled=\"2026-07-20T11:00:00Z\"\n  } Canceled\n";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/semanticTokens/full",
            "params": { "textDocument": { "uri": uri } }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    assert_eq!(
        response(&run_server(&messages), 2)["result"]["data"],
        json!([
            0, 0, 3, 0, 1, 1, 3, 5, 0, 1, 1, 3, 27, 0, 1, 1, 2, 1, 0, 1, 1, 0, 3, 0, 1, 1, 3, 5, 0,
            1, 1, 3, 31, 0, 1, 1, 2, 1, 0, 1
        ])
    );
}

#[test]
fn hovers_verbatim_autolinks_with_the_original_uri() {
    let uri = "file:///tmp/verbatim-autolink.plumb";
    let source = "Visit `[https://example.test/a%20b]{.->}.\n";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/hover",
            "params": {
                "textDocument": { "uri": uri },
                "position": { "line": 0, "character": 12 }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let hover = &response(&output, 2)["result"];
    assert_eq!(
        hover["contents"]["value"],
        "**External link**\n\n`https://example.test/a%20b`"
    );
    assert_eq!(
        hover["range"],
        json!({
            "start": { "line": 0, "character": 8 },
            "end": { "line": 0, "character": 34 }
        })
    );
}

#[test]
fn resolves_cross_file_navigation_over_stdio() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let target = root.join("a.plumb");
    let source = root.join("b.plumb");
    std::fs::write(&target, "`#{#target} Target\n").unwrap();
    let source_text = "See `->[target]{to=\"a.plumb#target\"}.\n";
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
fn code_lenses_count_anchor_references_and_ignore_last_valid_output() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let target = root.join("target.plumb");
    let source = root.join("source.plumb");
    let target_text = "`meta\n `: title\n\n    Target\n\n`#{#used} Used\n`##{#unused} Unused\n";
    let source_text = "See `->[used]{to=\"target.plumb#used\"}.\n`-{.task depends=\"target.plumb#used\"} Review\n";
    std::fs::write(&target, target_text).unwrap();
    std::fs::write(&source, source_text).unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let target_uri = lsp_types::Url::from_file_path(&target).unwrap();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": { "workspace": { "codeLens": { "refreshSupport": true } } }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeLens",
            "params": { "textDocument": { "uri": target_uri } }
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": target_uri, "languageId": "plumb", "version": 1, "text": target_text
            }}
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": target_uri, "version": 2 },
                "contentChanges": [{ "text": "`span[open\n" }]
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/codeLens",
            "params": { "textDocument": { "uri": target_uri } }
        }),
    ];
    let shutdown = [
        json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server_with_pause(&messages, &shutdown);
    assert_eq!(
        response(&output, 1)["result"]["capabilities"]["codeLensProvider"],
        json!({ "resolveProvider": false })
    );
    let lenses = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(lenses.len(), 3);
    assert_eq!(lenses[0]["command"]["title"], "2 file references");
    assert_eq!(lenses[0]["command"]["command"], "plumb.showReferences");
    assert_eq!(lenses[0]["command"]["arguments"][0], target_uri.as_str());
    assert_eq!(
        lenses[0]["command"]["arguments"][2]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(lenses[1]["command"]["title"], "2 references");
    assert_eq!(lenses[2]["command"]["title"], "0 references");
    assert!(response(&output, 3)["result"].is_null());
    assert!(output
        .iter()
        .any(|message| message["method"] == "workspace/codeLens/refresh"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn completes_links_by_document_metadata_title() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("current.plumb");
    let target = root.join("usage.plumb");
    let closed_path = "`->[x]{to=\"usXXX\"}";
    let closed_anchor = "`->[x]{to=\"usage.plumb#usXXX\"}";
    let raw = "`\"[raw `->[x]{to=\"us\"}]\"";
    let source_text =
        format!("`->[Us\n\n`->[x]{{to=\"Guide\n\n{closed_path}\n{closed_anchor}\n{raw}\n");
    std::fs::write(&source, &source_text).unwrap();
    std::fs::write(
        &target,
        "`meta\n  `: title\n\n    Usage Guide\n\n`#{#usage} Usage\n",
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
                "position": { "line": 2, "character": 18 }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": {
                    "line": 4,
                    "character": closed_path.find("usXXX").unwrap() + 2
                }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 5, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": {
                    "line": 5,
                    "character": closed_anchor.find("usXXX").unwrap() + 2
                }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 6, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": {
                    "line": 6,
                    "character": raw.find("us").unwrap() + 2
                }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 7, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let output = run_server(&messages);
    let label = &response(&output, 2)["result"][0];
    assert_eq!(label["label"], "Usage Guide");
    assert_eq!(label["detail"], "usage.plumb");
    assert_eq!(
        label["textEdit"]["newText"],
        "`->[Usage Guide]{to=\"usage.plumb\"}"
    );
    let path = &response(&output, 3)["result"][0];
    assert_eq!(path["label"], "usage.plumb");
    assert_eq!(path["detail"], "Usage Guide");
    assert_eq!(path["textEdit"]["newText"], "usage.plumb");
    let closed_path_item = &response(&output, 4)["result"][0];
    assert_eq!(closed_path_item["textEdit"]["newText"], "usage.plumb");
    assert_eq!(
        closed_path_item["textEdit"]["range"],
        json!({
            "start": { "line": 4, "character": closed_path.find("usXXX").unwrap() },
            "end": { "line": 4, "character": closed_path.find("usXXX").unwrap() + 5 }
        })
    );
    let closed_anchor_item = &response(&output, 5)["result"][0];
    assert_eq!(closed_anchor_item["textEdit"]["newText"], "usage");
    assert_eq!(
        closed_anchor_item["textEdit"]["range"],
        json!({
            "start": { "line": 5, "character": closed_anchor.find("usXXX").unwrap() },
            "end": { "line": 5, "character": closed_anchor.find("usXXX").unwrap() + 5 }
        })
    );
    assert!(response(&output, 6)["result"].is_null());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn completion_from_a_subdirectory_inserts_a_relative_path() {
    let root = unique_temp_dir();
    let source_dir = root.join("b");
    let target_dir = root.join("a");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::create_dir_all(&target_dir).unwrap();
    let source = source_dir.join("current.plumb");
    let target = target_dir.join("target.plumb");
    let source_text = "`->[Target";
    std::fs::write(&source, source_text).unwrap();
    std::fs::write(
        &target,
        "`meta\n  `: title\n\n    Target A\n\n`#{#target} Target\n",
    )
    .unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let source_uri = lsp_types::Url::from_file_path(&source).unwrap();
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
                "uri": source_uri, "languageId": "plumb", "version": 1, "text": source_text
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 0, "character": source_text.len() }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let item = response(&output, 2)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "Target A")
        .expect("Target A completion");
    assert_eq!(item["detail"], "../a/target.plumb");
    assert_eq!(
        item["textEdit"]["newText"],
        "`->[Target A]{to=\"../a/target.plumb\"}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn completes_and_navigates_relative_autolinks_files_and_images() {
    let root = unique_temp_dir();
    let static_dir = root.join("static");
    std::fs::create_dir_all(&static_dir).unwrap();
    let current = root.join("current.plumb");
    let target = root.join("target note.plumb");
    let unicode_target = root.join("中文笔记 [草稿].plumb");
    let image = static_dir.join("image one.PNG");
    let attachment = static_dir.join("manual draft.pdf");
    let source = "`[tar]{.->}\n`\"[target note.plumb#an]\"{.->}\n`img[Query]{src=\"static/im\"}\n`img[Missing]{src=\"static/missing.png\"}\n`[target note.plumb]{.->}\n`img[Result]{src=\"static/image%20one.PNG\"}\n`[中文]{.->}\n`[static/manual draft.pdf]{.->}\n`->[manual]{to=\"static/manual draft.pdf\"}\n`[static/missing guide.pdf]{.->}\n";
    std::fs::write(&current, source).unwrap();
    std::fs::write(
        &target,
        "`meta\n `: title\n\n    Target note\n\n`#{#anchor} Anchor\n",
    )
    .unwrap();
    std::fs::write(&unicode_target, "`# 中文笔记\n").unwrap();
    std::fs::write(&image, b"png").unwrap();
    std::fs::write(&attachment, b"pdf").unwrap();

    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let current_uri = lsp_types::Url::from_file_path(&current).unwrap();
    let target_uri = lsp_types::Url::from_file_path(&target).unwrap();
    let image_uri = lsp_types::Url::from_file_path(&image).unwrap();
    let attachment_uri = lsp_types::Url::from_file_path(&attachment).unwrap();
    let lines = source.lines().collect::<Vec<_>>();
    let autolink_path_cursor = lines[0].find("tar").unwrap() + "tar".len();
    let autolink_anchor_cursor = lines[1].find("#an").unwrap() + "#an".len();
    let image_query_cursor = lines[2].find("static/im").unwrap() + "static/im".len();
    let autolink_definition = lines[4].find("target note.plumb").unwrap() + 2;
    let image_definition = lines[5].find("static/image").unwrap() + 2;
    let unicode_cursor = lines[6][..lines[6].find("中文").unwrap() + "中文".len()]
        .encode_utf16()
        .count();
    let attachment_autolink = lines[7].find("manual draft").unwrap() + 2;
    let attachment_link = lines[8].find("manual draft").unwrap() + 2;
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
                "uri": current_uri, "languageId": "plumb", "version": 4, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 0, "character": autolink_path_cursor }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 1, "character": autolink_anchor_cursor }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 2, "character": image_query_cursor }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 5, "method": "textDocument/hover",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 5, "character": image_definition }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 6, "method": "textDocument/definition",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 5, "character": image_definition }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 7, "method": "textDocument/definition",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 4, "character": autolink_definition }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 8, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 6, "character": unicode_cursor }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 9, "method": "textDocument/hover",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 7, "character": attachment_autolink }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 10, "method": "textDocument/definition",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 7, "character": attachment_autolink }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 11, "method": "textDocument/definition",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 8, "character": attachment_link }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 12, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let autolink_path = response(&output, 2)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "target note.plumb")
        .expect("autolink document path completion");
    assert_eq!(autolink_path["detail"], "Target note");
    assert_eq!(autolink_path["textEdit"]["newText"], "target note.plumb");

    let anchor = response(&output, 3)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "#anchor")
        .expect("raw anchor completion");
    assert_eq!(anchor["textEdit"]["newText"], "anchor");

    let image_completion = response(&output, 4)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "static/image%20one.PNG")
        .expect("image path completion");
    assert_eq!(image_completion["kind"], 17);
    assert_eq!(
        image_completion["textEdit"]["newText"],
        "static/image%20one.PNG"
    );

    assert!(response(&output, 5)["result"]["contents"]["value"]
        .as_str()
        .unwrap()
        .contains("Image file"));
    assert_eq!(response(&output, 6)["result"]["uri"], image_uri.as_str());
    assert_eq!(response(&output, 7)["result"]["uri"], target_uri.as_str());

    let unicode_completion = response(&output, 8)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "中文笔记 [草稿].plumb")
        .expect("Unicode autolink completion");
    assert_eq!(
        unicode_completion["textEdit"]["newText"],
        "`\"[中文笔记 [草稿].plumb]\""
    );
    assert_eq!(
        unicode_completion["textEdit"]["range"],
        json!({
            "start": { "line": 6, "character": 0 },
            "end": { "line": 6, "character": 5 }
        })
    );
    assert!(response(&output, 9)["result"]["contents"]["value"]
        .as_str()
        .unwrap()
        .contains("File"));
    assert_eq!(
        response(&output, 10)["result"]["uri"],
        attachment_uri.as_str()
    );
    assert_eq!(
        response(&output, 11)["result"]["uri"],
        attachment_uri.as_str()
    );

    let diagnostics = output
        .iter()
        .filter(|message| {
            message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == current_uri.as_str()
        })
        .last()
        .expect("current diagnostics");
    assert!(diagnostics["params"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|diagnostic| diagnostic["code"] == "image.unresolved-file"));
    assert!(diagnostics["params"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|diagnostic| diagnostic["code"] == "link.unresolved-file"));

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn completes_constructs_after_a_single_backtick() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let document = root.join("constructs.plumb");
    let source = "`\nText `";
    std::fs::write(&document, source).unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let document_uri = lsp_types::Url::from_file_path(&document).unwrap();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": { "textDocument": { "completion": {
                    "completionItem": { "snippetSupport": true }
                } } }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": document_uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": document_uri },
                "position": { "line": 0, "character": 1 }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": document_uri },
                "position": { "line": 1, "character": 6 }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let block = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(
        block
            .iter()
            .map(|item| item["label"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["Task", "Autolink", "Link"]
    );
    let task = block[0]["textEdit"]["newText"].as_str().unwrap();
    assert!(task.starts_with("`-{.task created=\""));
    assert!(task.ends_with("\"} ${1:Task}"));
    chrono::DateTime::parse_from_rfc3339(attribute_value(task, "created")).unwrap();
    assert_eq!(block[0]["insertTextFormat"], 2);

    let inline = response(&output, 3)["result"].as_array().unwrap();
    assert_eq!(
        inline
            .iter()
            .map(|item| item["label"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["Autolink", "Link"]
    );
    let autolink = inline
        .iter()
        .find(|item| item["label"] == "Autolink")
        .unwrap();
    assert_eq!(autolink["textEdit"]["newText"], "`[${1:path}]{.->}");
    assert_eq!(autolink["insertTextFormat"], 2);

    let fallback_messages = [
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
                "uri": document_uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": document_uri },
                "position": { "line": 1, "character": 6 }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": document_uri },
                "position": { "line": 0, "character": 1 }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let fallback_output = run_server(&fallback_messages);
    let fallback = response(&fallback_output, 2)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "Autolink")
        .unwrap();
    assert_eq!(fallback["textEdit"]["newText"], "`[]{.->}");
    assert_eq!(fallback["insertTextFormat"], 1);
    let fallback_block = response(&fallback_output, 3)["result"].as_array().unwrap();
    assert_eq!(
        fallback_block
            .iter()
            .map(|item| item["label"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["Task", "Autolink", "Link"]
    );
    let fallback_task = fallback_block
        .iter()
        .find(|item| item["label"] == "Task")
        .unwrap()["textEdit"]["newText"]
        .as_str()
        .unwrap();
    assert!(fallback_task.starts_with("`-{.task created=\""));
    assert!(fallback_task.ends_with("\"} "));
    chrono::DateTime::parse_from_rfc3339(attribute_value(fallback_task, "created")).unwrap();
    assert_eq!(fallback_block[0]["insertTextFormat"], 1);
    std::fs::remove_dir_all(root).unwrap();
}

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
        "`meta\n `: title\n\n    Target\n\n`#{#anchor} Target\n`-{.task #work} Target work\n";
    std::fs::write(&source, "`->[").unwrap();
    std::fs::write(&target, target_source).unwrap();
    std::fs::write(snapshot.join("target.plumb"), target_source).unwrap();
    let reference_source = "`->[Target]{to=\"target.plumb#anchor\"}\n";
    std::fs::write(root.join("reference.plumb"), reference_source).unwrap();
    std::fs::write(snapshot.join("reference.plumb"), reference_source).unwrap();
    std::fs::write(
        root.join("linked-source.txt"),
        "`meta\n `: title\n\n    Linked file\n",
    )
    .unwrap();
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
                "position": { "line": 5, "character": 5 },
                "context": { "includeDeclaration": false }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 5, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server_with_pause(
        &messages[..messages.len() - 2],
        &messages[messages.len() - 2..],
    );
    let items = response(&output, 2)["result"].as_array().unwrap();
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
    std::fs::write(&note, "`meta\n `: title\n\n    Disk title\n").unwrap();
    std::fs::write(
        &tasks,
        "`-{.task #review due=\"2026-07-23T12:00:00+08:00\"} Review parser\n",
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
    let open_note = "`meta\n `: title\n\n    Open title\n";
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

    let output = run_server_with_pause(
        &messages[..messages.len() - 2],
        &messages[messages.len() - 2..],
    );
    let capabilities = &response(&output, 1)["result"]["capabilities"];
    assert_eq!(capabilities["workspaceSymbolProvider"], true);
    assert_eq!(
        capabilities["experimental"]["plumb"]["search"]["schemaVersion"],
        1
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
    assert_eq!(structured["schemaVersion"], 1);
    assert_eq!(structured["complete"], true);
    assert_eq!(structured["items"][0]["kind"], "task");
    assert_eq!(structured["items"][0]["id"], "review");
    assert_eq!(structured["items"][0]["state"], "open");
    assert_eq!(structured["items"][0]["blocked"], false);
    assert_eq!(structured["items"][0]["provenance"]["source"], "current");
    assert_eq!(structured["items"][0]["provenance"]["revision"], 0);
    assert_eq!(response(&output, 5)["result"]["complete"], false);
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
fn task_references_support_navigation_and_rename() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let target = root.join("Project Plan.plumb");
    let source = root.join("review.plumb");
    let target_text = "`-{.task #draft} Draft\n";
    let source_text = "`-{.task #review prev=\"Project%20Plan.plumb#draft\" depends=\"Project%20Plan.plumb#draft\"} Review\n";
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
    let source_text = "`-{.task depends=\"task%20target.plumb#draft\"} Review\nSee `->[note]{to=\"link target.plumb#note\"}.\nSee `->[hover]{to=\"hover target.plumb#hover\"}.\nSee `->[file]{to=\"file target.plumb\"}.\n";
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

fn run_server(messages: &[Value]) -> Vec<Value> {
    run_server_with_writer(|stdin| {
        for message in messages {
            write_message(stdin, message);
        }
    })
}

fn run_server_with_pause(first: &[Value], second: &[Value]) -> Vec<Value> {
    run_server_with_writer(|stdin| {
        for message in first {
            write_message(stdin, message);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        for message in second {
            write_message(stdin, message);
        }
    })
}

fn run_server_with_writer(
    write_messages: impl FnOnce(&mut std::process::ChildStdin),
) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start plumb lsp");
    {
        let stdin = child.stdin.as_mut().expect("child stdin");
        write_messages(stdin);
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
        "plumb lsp failed: {}",
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

fn attribute_value<'a>(text: &'a str, key: &str) -> &'a str {
    let needle = format!("{key}=\"");
    text.split_once(&needle)
        .and_then(|(_, value)| value.split_once('"'))
        .map(|(value, _)| value)
        .expect("attribute value")
}

fn diagnostic_counts(messages: &[Value], uri: &str) -> Vec<usize> {
    messages
        .iter()
        .filter(|message| {
            message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri
        })
        .map(|message| message["params"]["diagnostics"].as_array().unwrap().len())
        .collect()
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
