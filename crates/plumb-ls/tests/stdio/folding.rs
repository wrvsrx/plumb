use serde_json::json;

use crate::support::{response, run_server};

#[test]
fn labels_metadata_folds_with_the_document_title() {
    let uri = "file:///tmp/metadata-fold-label.plumb";
    let source = "`meta\n  `: title\n\n    项目 Overview\n\n  `: created\n\n    2026-08-05T03:46:54+08:00\n\n  `: tags\n    `- plumb\n";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": null,
                "capabilities": { "textDocument": { "foldingRange": {
                    "foldingRange": { "collapsedText": true }
                }}}
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
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/foldingRange",
            "params": { "textDocument": { "uri": uri } }
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [{ "text": "`meta\n  `: tags\n    `- plumb\n" }]
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/foldingRange",
            "params": { "textDocument": { "uri": uri } }
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 3 },
                "contentChanges": [{ "text": "`node Parent\n  `meta\n    `: title\n\n      Nested\n" }]
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/foldingRange",
            "params": { "textDocument": { "uri": uri } }
        }),
        json!({ "jsonrpc": "2.0", "id": 5, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let ranges = response(&run_server(&messages), 2)["result"]
        .as_array()
        .unwrap()
        .to_vec();
    assert_eq!(
        ranges[0],
        json!({
            "startLine": 0,
            "endLine": 10,
            "collapsedText": "METADATA  项目 Overview"
        })
    );
    assert!(ranges
        .iter()
        .any(|range| range["collapsedText"] == "  title  项目 Overview"));
    assert!(ranges
        .iter()
        .any(|range| range["collapsedText"] == "  created  2026-08-05T03:46:54+08:00"));
    assert!(ranges
        .iter()
        .any(|range| range["collapsedText"] == "  tags"));
    assert_eq!(
        response(&run_server(&messages), 3)["result"][0],
        json!({ "startLine": 0, "endLine": 2, "collapsedText": "METADATA" })
    );
    assert!(response(&run_server(&messages), 4)["result"]
        .as_array()
        .unwrap()
        .iter()
        .all(|range| range.get("collapsedText").is_none()));
}

#[test]
fn exposes_single_line_semantic_folds_to_line_and_character_range_clients() {
    let uri = "file:///tmp/single-line-folds.plumb";
    let source = "`- Ready {`-[task]}\n`- 14:00 Standup {`-[event] `:[date 2026-08-02] `:[timezone +08:00]}\n";
    let requests = |line_folding_only| {
        [
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": null,
                    "capabilities": { "textDocument": { "foldingRange": {
                        "lineFoldingOnly": line_folding_only,
                        "foldingRange": { "collapsedText": true }
                    }}}
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
                "jsonrpc": "2.0", "id": 2, "method": "textDocument/foldingRange",
                "params": { "textDocument": { "uri": uri } }
            }),
            json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
            json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
        ]
    };

    assert_eq!(
        response(&run_server(&requests(false)), 2)["result"],
        json!([
            {
                "startLine": 0,
                "startCharacter": 0,
                "endLine": 0,
                "endCharacter": source.lines().next().unwrap().len(),
                "collapsedText": "-   Ready"
            },
            {
                "startLine": 1,
                "startCharacter": 0,
                "endLine": 1,
                "endCharacter": source.lines().nth(1).unwrap().len(),
                "collapsedText": "2026-08-02T14:00  Standup"
            }
        ])
    );
    assert_eq!(
        response(&run_server(&requests(true)), 2)["result"],
        json!([
            {
                "startLine": 0,
                "endLine": 0,
                "collapsedText": "-   Ready"
            },
            {
                "startLine": 1,
                "endLine": 1,
                "collapsedText": "2026-08-02T14:00  Standup"
            }
        ])
    );
}

#[test]
fn task_fold_includes_one_trailing_separator_line() {
    let uri = "file:///tmp/task-trailing-blank-fold.plumb";
    let source = "`- aaa\n   {\n     `- task\n   }\n\n   bbb\n\n`- ccc\n   {\n     `- task\n   }\n";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": null,
                "capabilities": { "textDocument": { "foldingRange": {
                    "lineFoldingOnly": true,
                    "foldingRange": { "collapsedText": true }
                }}}
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
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/foldingRange",
            "params": { "textDocument": { "uri": uri } }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    assert_eq!(
        response(&run_server(&messages), 2)["result"],
        json!([
            { "startLine": 0, "endLine": 6, "collapsedText": "-   aaa" },
            { "startLine": 7, "endLine": 10, "collapsedText": "-   ccc" }
        ])
    );
}

#[test]
fn provides_structural_folding_for_valid_and_recovered_documents() {
    let uri = "file:///tmp/folding.plumb";
    let source = "`# Top\n\nIntro.\n\n`## Child\n\n`div Details\n\n     body\n\n     `text\"\n       raw\n`# Next\n\nTail.\n";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": null,
                "capabilities": {
                    "textDocument": {
                        "foldingRange": { "lineFoldingOnly": true, "rangeLimit": 3 }
                    }
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
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/foldingRange",
            "params": { "textDocument": { "uri": uri } }
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [{
                    "text": "`node Parent\n  `child Child\nordinary `span[open\n"
                }]
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/foldingRange",
            "params": { "textDocument": { "uri": uri } }
        }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    assert_eq!(
        response(&output, 1)["result"]["capabilities"]["foldingRangeProvider"],
        true
    );
    assert_eq!(
        response(&output, 2)["result"],
        json!([
            { "startLine": 0, "endLine": 11 },
            { "startLine": 4, "endLine": 11 },
            { "startLine": 6, "endLine": 11 }
        ])
    );
    assert_eq!(
        response(&output, 3)["result"],
        json!([{ "startLine": 0, "endLine": 1 }])
    );
}

#[test]
fn labels_task_folds_with_derived_workflow_states() {
    let uri = "file:///tmp/task-fold-labels.plumb";
    let source = "`- Ready task\n   {\n     `- task\n     `@ blocker\n   }\n\n   `note Detail\n\n`- Waiting task\n   {\n     `- task\n     `: wait 2099-01-01T00:00:00Z\n     `: depends #blocker\n   }\n\n   `note Detail\n\n`- Done task\n   {\n     `- task\n     `: done 2026-07-27T10:00:00Z\n   }\n\n   `note Detail\n\n`- Canceled task\n   {\n     `- task\n     `: canceled 2026-07-27T10:00:00Z\n   }\n\n   `note Detail\n\n`- Conflicted task\n   {\n     `- task\n     `: done 2026-07-27T10:00:00Z\n     `: canceled 2026-07-27T10:01:00Z\n   }\n\n   `note Detail\n\n`- Blocked task\n   {\n     `- task\n     `: depends #blocker\n   }\n\n   `note Detail\n\n`node Parent\n\n      `- Nested task\n         {\n           `- task\n           `: done 2026-07-27T10:02:00Z\n         }\n\n         `note Detail\n";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": null,
                "capabilities": {
                    "textDocument": {
                        "foldingRange": {
                            "foldingRange": { "collapsedText": true }
                        }
                    }
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
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/foldingRange",
            "params": { "textDocument": { "uri": uri } }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    assert_eq!(
        response(&run_server(&messages), 2)["result"],
        json!([
            { "startLine": 0, "endLine": 7, "collapsedText": "-   Ready task" },
            { "startLine": 8, "endLine": 16, "collapsedText": "~   Waiting task" },
            { "startLine": 17, "endLine": 24, "collapsedText": "+   Done task" },
            { "startLine": 25, "endLine": 32, "collapsedText": "x   Canceled task" },
            { "startLine": 33, "endLine": 41, "collapsedText": "+x  Conflicted task" },
            { "startLine": 42, "endLine": 49, "collapsedText": "!   Blocked task" },
            { "startLine": 50, "endLine": 58 },
            { "startLine": 52, "endLine": 58, "collapsedText": "      +   Nested task" }
        ])
    );
}

#[test]
fn labels_event_folds_with_abbreviated_times() {
    let uri = "file:///tmp/event-fold-labels.plumb";
    let source = "`- 14:00 Standup\n   {\n     `- event\n     `: date 2026-08-02\n     `: timezone +08:00\n   }\n\n   `note Detail\n\n`- 09:00--10:30 Review\n   {\n     `- event\n     `: date 2026-08-02\n     `: timezone +08:00\n   }\n\n   `note Detail\n\n`- 11:00 Parent\n   {\n     `- event\n     `: date 2026-08-02\n     `: timezone +08:00\n   }\n\n   `note Detail\n\n   `- 12:00 Nested\n      {\n        `- event\n        `: date 2026-08-02\n        `: timezone +08:00\n      }\n\n      `note Detail\n\n`- Untimed\n   {\n     `- event\n   }\n\n   `note Detail\n";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": null,
                "capabilities": {
                    "textDocument": {
                        "foldingRange": {
                            "foldingRange": { "collapsedText": true }
                        }
                    }
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
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/foldingRange",
            "params": { "textDocument": { "uri": uri } }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    assert_eq!(
        response(&run_server(&messages), 2)["result"],
        json!([
            { "startLine": 0, "endLine": 7, "collapsedText": "2026-08-02T14:00  Standup" },
            { "startLine": 9, "endLine": 16, "collapsedText": "2026-08-02T09:00--10:30  Review" },
            { "startLine": 18, "endLine": 34, "collapsedText": "2026-08-02T11:00  Parent" },
            { "startLine": 27, "endLine": 34, "collapsedText": "   2026-08-02T12:00  Nested" },
            { "startLine": 36, "endLine": 41 }
        ])
    );
}
