use serde_json::json;

use crate::support::{response, run_server};

#[test]
fn labels_metadata_folds_with_the_document_title() {
    let uri = "file:///tmp/metadata-fold-label.plumb";
    let source = "`meta\n  `: title\n\n    项目 Overview\n\n  `: tags\n    `- plumb\n";
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
            "endLine": 6,
            "collapsedText": "METADATA  项目 Overview"
        })
    );
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
    let source =
        "`-{.task} Ready\n`-{.event date=2026-08-02 timezone=\"+08:00\" when=\"14:00\"} Standup\n";
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
                "collapsedText": "READY  Ready"
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
                "collapsedText": "READY  Ready"
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
fn provides_structural_folding_for_valid_and_recovered_documents() {
    let uri = "file:///tmp/folding.plumb";
    let source = "`# Top\nIntro.\n`## Child\n`div Details\n\n  body\n  `{language=text}\n    raw\n`# Next\nTail.\n";
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
            { "startLine": 0, "endLine": 7 },
            { "startLine": 2, "endLine": 7 },
            { "startLine": 3, "endLine": 7 }
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
    let source = "`-{.task} Ready task\n  `note Detail\n`-{.task wait=\"2099-01-01T00:00:00Z\"} Waiting task\n  `note Detail\n`-{.task done=\"2026-07-27T10:00:00Z\"} Done task\n  `note Detail\n`-{.task canceled=\"2026-07-27T10:00:00Z\"} Canceled task\n  `note Detail\n`-{.task done=\"2026-07-27T10:00:00Z\" canceled=\"2026-07-27T10:01:00Z\"} Invalid task\n  `note Detail\n`node Parent\n  `-{.task done=\"2026-07-27T10:02:00Z\"} Nested task\n    `note Detail\n";
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
            { "startLine": 0, "endLine": 1, "collapsedText": "READY  Ready task" },
            { "startLine": 2, "endLine": 3, "collapsedText": "WAITING  Waiting task" },
            { "startLine": 4, "endLine": 5, "collapsedText": "DONE  Done task" },
            { "startLine": 6, "endLine": 7, "collapsedText": "CANCELED  Canceled task" },
            { "startLine": 8, "endLine": 9, "collapsedText": "INVALID  Invalid task" },
            { "startLine": 10, "endLine": 12 },
            { "startLine": 11, "endLine": 12, "collapsedText": "  DONE  Nested task" }
        ])
    );
}

#[test]
fn labels_event_folds_with_abbreviated_times() {
    let uri = "file:///tmp/event-fold-labels.plumb";
    let source = "`-{.event date=2026-08-02 timezone=\"+08:00\" when=\"14:00\"} Standup\n  `note Detail\n`-{.event date=2026-08-02 timezone=\"+08:00\" when=\"09:00--10:30\"} Review\n  `note Detail\n`-{.event date=2026-08-02 timezone=\"+08:00\" when=\"11:00\"} Parent\n  `note Detail\n  `-{.event date=2026-08-02 timezone=\"+08:00\" when=\"12:00\"} Nested\n    `note Detail\n`-{.event} Untimed\n  `note Detail\n";
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
            { "startLine": 0, "endLine": 1, "collapsedText": "2026-08-02T14:00  Standup" },
            { "startLine": 2, "endLine": 3, "collapsedText": "2026-08-02T09:00--10:30  Review" },
            { "startLine": 4, "endLine": 7, "collapsedText": "2026-08-02T11:00  Parent" },
            { "startLine": 6, "endLine": 7, "collapsedText": "  2026-08-02T12:00  Nested" },
            { "startLine": 8, "endLine": 9 }
        ])
    );
}
