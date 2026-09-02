use serde_json::json;

use crate::support::{response, run_server};

fn initialize(root_uri: &str) -> serde_json::Value {
    json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "processId": null,
            "rootUri": root_uri,
            "workspaceFolders": null,
            "capabilities": {
                "textDocument": {
                    "completion": { "completionItem": { "snippetSupport": true } },
                    "foldingRange": { "lineFoldingOnly": true }
                },
                "workspace": { "workspaceEdit": { "documentChanges": true } }
            }
        }
    })
}

#[test]
fn publishes_new_group_diagnostics_and_heading_symbols() {
    let uri = "file:///tmp/current-diagnostics.plumb";
    let source = "`# Heading\nBroken `span{open\n";
    let messages = [
        initialize("file:///tmp"),
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
    assert!(output.iter().any(|message| {
        message["method"] == "textDocument/publishDiagnostics"
            && message["params"]["diagnostics"]
                .as_array()
                .is_some_and(|diagnostics| {
                    diagnostics
                        .iter()
                        .any(|diagnostic| diagnostic["code"] == "syntax.unclosed-inline-group")
                })
    }));
    assert!(response(&output, 2)["result"].is_null());
}

#[test]
fn completes_current_link_and_task_constructs() {
    let uri = "file:///tmp/current-completion.plumb";
    let source = "Text `->{\n`-";
    let messages = [
        initialize("file:///tmp"),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
            "params": { "textDocument": { "uri": uri }, "position": { "line": 0, "character": 9 } }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/completion",
            "params": { "textDocument": { "uri": uri }, "position": { "line": 1, "character": 2 } }
        }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let output = run_server(&messages);
    let link = &response(&output, 2)["result"][0];
    assert_eq!(link["label"], "Link");
    assert_eq!(link["textEdit"]["newText"], "`->{{${1:label}} ${2:target}}");
    let task = response(&output, 3)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "Task")
        .unwrap();
    assert!(task["textEdit"]["newText"]
        .as_str()
        .unwrap()
        .contains("`= created "));
}

#[test]
fn formats_recursive_children_with_one_structural_space() {
    let uri = "file:///tmp/current-format.plumb";
    let source = "`node Parent\n   `child Example\n";
    let messages = [
        initialize("file:///tmp"),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/formatting",
            "params": { "textDocument": { "uri": uri }, "options": { "tabSize": 4, "insertSpaces": true } }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let output = run_server(&messages);
    let edits = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0]["newText"], "\n `child Example\n");
}

#[test]
fn returns_task_fold_and_status_actions_for_current_properties() {
    let uri = "file:///tmp/current-task.plumb";
    let source = "`- Current task\n `+ task\n `@ current\n `= created 2026-09-02T09:00:00+08:00\n";
    let messages = [
        initialize("file:///tmp"),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/foldingRange",
            "params": { "textDocument": { "uri": uri } }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 4 } },
                "context": { "diagnostics": [] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let output = run_server(&messages);
    assert!(!response(&output, 2)["result"]
        .as_array()
        .unwrap()
        .is_empty());
    let titles = response(&output, 3)["result"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|action| action["title"].as_str())
        .collect::<Vec<_>>();
    assert!(titles.contains(&"Complete task"));
    assert!(titles.contains(&"Cancel task"));
}
