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
    let source = "Text `->\n`-\nText `->\"";
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
            "params": { "textDocument": { "uri": uri }, "position": { "line": 0, "character": 8 } }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/completion",
            "params": { "textDocument": { "uri": uri }, "position": { "line": 1, "character": 2 } }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/completion",
            "params": { "textDocument": { "uri": uri }, "position": { "line": 2, "character": 9 } }
        }),
        json!({ "jsonrpc": "2.0", "id": 5, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let output = run_server(&messages);
    let link = &response(&output, 2)["result"][0];
    assert_eq!(link["label"], "Link");
    assert_eq!(link["textEdit"]["newText"], "`->{${1:target/label}}");
    let constructs = response(&output, 3)["result"].as_array().unwrap();
    assert_eq!(
        constructs
            .iter()
            .map(|item| item["label"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["Task", "Event", "Link"]
    );
    let task = constructs
        .iter()
        .find(|item| item["label"] == "Task")
        .unwrap();
    assert!(task["textEdit"]["newText"]
        .as_str()
        .unwrap()
        .contains("`= created "));
    let verbatim_link = &response(&output, 4)["result"][0];
    assert_eq!(verbatim_link["label"], "Link");
    assert_eq!(verbatim_link["textEdit"]["newText"], "`->\"${1:path}\"");
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
fn aligned_arguments_are_idempotent_across_code_action_and_formatting() {
    let uri = "file:///tmp/current-alignment.plumb";
    let source = "`= a one\n`= long two\n";
    let aligned = "`= a    one\n`= long two\n";
    let code_action = |id| {
        json!({
            "jsonrpc": "2.0", "id": id, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 4 },
                    "end": { "line": 0, "character": 4 }
                },
                "context": { "diagnostics": [], "only": ["refactor.rewrite"] }
            }
        })
    };
    let messages = [
        initialize("file:///tmp"),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        code_action(2),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [{ "text": aligned }]
            }
        }),
        code_action(3),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/formatting",
            "params": { "textDocument": { "uri": uri }, "options": {
                "tabSize": 4, "insertSpaces": true
            }}
        }),
        json!({ "jsonrpc": "2.0", "id": 5, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let output = run_server(&messages);
    let action = response(&output, 2)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == "Align arguments")
        .unwrap();
    let edits = action["edit"]["documentChanges"][0]["edits"]
        .as_array()
        .unwrap();
    assert_eq!(apply_ascii_edits(source, edits), aligned);
    assert!(response(&output, 3)["result"]
        .as_array()
        .is_none_or(|actions| actions
            .iter()
            .all(|action| action["title"] != "Align arguments")));
    assert_eq!(response(&output, 4)["result"], json!([]));
}

fn apply_ascii_edits(source: &str, edits: &[serde_json::Value]) -> String {
    let mut output = source.to_string();
    for edit in edits.iter().rev() {
        let range = &edit["range"];
        let start_line = range["start"]["line"].as_u64().unwrap() as usize;
        let start_character = range["start"]["character"].as_u64().unwrap() as usize;
        let end_line = range["end"]["line"].as_u64().unwrap() as usize;
        let end_character = range["end"]["character"].as_u64().unwrap() as usize;
        let offsets = |line: usize, character: usize| {
            let line_start = if line == 0 {
                0
            } else {
                output
                    .match_indices('\n')
                    .map(|(offset, _)| offset + 1)
                    .nth(line - 1)
                    .unwrap()
            };
            line_start + character
        };
        let start = offsets(start_line, start_character);
        let end = offsets(end_line, end_character);
        output.replace_range(start..end, edit["newText"].as_str().unwrap());
    }
    output
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
