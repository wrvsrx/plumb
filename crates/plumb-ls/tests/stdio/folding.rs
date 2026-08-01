use serde_json::json;

use crate::support::{response, run_server};

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
    let source = "`-{.event at=\"2026-08-02T14:00:00+08:00\"} Standup\n  `note Detail\n`-{.event start=\"2026-08-02T09:00:00+08:00\" end=\"2026-08-02T10:30:00+08:00\"} Review\n  `note Detail\n`-{.event start=\"2026-08-02T23:30:00+08:00\" end=\"2026-08-03T01:00:00+08:00\"} Late\n  `note Detail\n`-{.event start=\"2026-08-02T09:00:00+08:00\" end=\"2026-08-03T18:00:00+08:00\"} Trip\n  `note Detail\n`-{.event start=\"2026-08-02T11:00:00+08:00\"} Ongoing\n  `note Detail\n  `-{.event at=\"2026-08-02T12:00:00+08:00\"} Nested\n    `note Detail\n`-{.event} Untimed\n  `note Detail\n";
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
            { "startLine": 4, "endLine": 5, "collapsedText": "2026-08-02T23:30--01:00  Late" },
            { "startLine": 6, "endLine": 7, "collapsedText": "2026-08-02T09:00--2026-08-03T18:00  Trip" },
            { "startLine": 8, "endLine": 11, "collapsedText": "2026-08-02T11:00-running  Ongoing" },
            { "startLine": 10, "endLine": 11, "collapsedText": "  2026-08-02T12:00  Nested" },
            { "startLine": 12, "endLine": 13 }
        ])
    );
}
