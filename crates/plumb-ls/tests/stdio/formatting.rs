use serde_json::json;

use crate::support::{response, run_server};

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
