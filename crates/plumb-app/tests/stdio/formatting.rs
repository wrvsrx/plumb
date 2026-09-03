use serde_json::json;

use crate::support::{response, run_server};

#[test]
fn whole_document_formatting_keeps_unchanged_blocks_out_of_edits() {
    let uri = "file:///tmp/minimal-format.plumb";
    let source = "`node One\n\n       `child A\n\n`node Stable\n\n `child B\n\n`node Three\n\n       `child C\n";
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
                "options": { "tabSize": 2, "insertSpaces": true }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let edits = response(&run_server(&messages), 2)["result"]
        .as_array()
        .unwrap()
        .to_vec();
    assert_eq!(edits.len(), 2);
    assert_eq!(edits[0]["range"]["start"]["line"], 2);
    assert_eq!(edits[0]["range"]["end"]["line"], 3);
    assert_eq!(edits[0]["newText"], " `child A\n");
    assert_eq!(edits[1]["range"]["start"]["line"], 10);
    assert_eq!(edits[1]["range"]["end"]["line"], 11);
    assert_eq!(edits[1]["newText"], " `child C\n");
}

#[test]
fn whole_document_formatting_handles_repeated_marker_lines() {
    let uri = "file:///tmp/repeated-marker-format.plumb";
    let source = "`task aaa bbb ccc ddd eee fff ggg hhh iii jjj kkk lll mmm nnn ooo ppp\n\n       `= created now\n\n        `note Detail\n\n`task Following\n\n       `= created later\n";
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
                "options": { "tabSize": 2, "insertSpaces": true }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let edits = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(edits.len(), 2);
    let formatted = apply_ascii_lsp_edits(source, edits);
    assert!(plumb_syntax::parse(&formatted).is_valid(), "{formatted}");
    let parsed = plumb_syntax::parse(&formatted);
    assert!(
        plumb_edit::format(&parsed, plumb_edit::FormatScope::Document)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn formats_valid_documents_and_declines_invalid_revisions() {
    let uri = "file:///tmp/format.plumb";
    let source = "`node Parent\n\n       `child Example\n";
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
                "contentChanges": [{ "text": "`span{open\n" }]
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
        apply_ascii_lsp_edits(source, response(&output, 2)["result"].as_array().unwrap()),
        "`node Parent\n\n `child Example\n"
    );
    assert!(response(&output, 3)["result"].is_null());
}

fn apply_ascii_lsp_edits(source: &str, edits: &[serde_json::Value]) -> String {
    let mut edits = edits
        .iter()
        .map(|edit| {
            let range = &edit["range"];
            let offset = |position: &serde_json::Value| {
                let line = position["line"].as_u64().unwrap() as usize;
                let character = position["character"].as_u64().unwrap() as usize;
                let line_start = if line == 0 {
                    0
                } else {
                    source
                        .match_indices('\n')
                        .map(|(offset, _)| offset + 1)
                        .nth(line - 1)
                        .unwrap()
                };
                line_start + character
            };
            (
                offset(&range["start"])..offset(&range["end"]),
                edit["newText"].as_str().unwrap().to_string(),
            )
        })
        .collect::<Vec<_>>();
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut output = source.to_string();
    for (range, new_text) in edits {
        output.replace_range(range, &new_text);
    }
    output
}

#[test]
fn range_formatting_formats_only_complete_contained_blocks() {
    let uri = "file:///tmp/range-format.plumb";
    let source = "`node Parent\n\n      `task One\n       `@ 一\n      `task Two\n       `@ 二\n\n`# Following\n";
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
                    "start": { "line": 2, "character": 6 },
                    "end": { "line": 4, "character": 0 }
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
                "contentChanges": [{ "text": "`span{open\n" }]
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
        json!({ "line": 2, "character": 6 })
    );
    assert_eq!(edits[0]["newText"], "`task One\n\n `@ 一");
    assert_eq!(response(&output, 3)["result"], json!([]));
    assert_eq!(response(&output, 4)["result"], json!([]));
    assert!(response(&output, 5)["result"].is_null());
}

#[test]
fn range_formatting_returns_multiple_maximal_groups() {
    let uri = "file:///tmp/range-format-groups.plumb";
    let source = "`node First\n\n       `task One\n        `@ one\n\n`node Second\n\n       `task Two\n        `@ two\n";
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
                    "end": { "line": 9, "character": 0 }
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
        json!({ "line": 2, "character": 7 })
    );
    assert_eq!(edits[0]["newText"], "`task One\n\n `@ one");
    assert_eq!(
        edits[1]["range"]["start"],
        json!({ "line": 5, "character": 0 })
    );
    assert!(edits[1]["newText"]
        .as_str()
        .unwrap()
        .starts_with("`node Second\n"));
}
