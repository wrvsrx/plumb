use serde_json::json;

use crate::support::{response, run_server, run_server_with_pause, unique_temp_dir};

#[test]
fn labels_individual_metadata_entry_folds() {
    let uri = "file:///tmp/metadata-fold-label.plumb";
    let source = "`= title\n\n 项目 Overview\n\n`= created\n\n 2026-08-05T03:46:54+08:00\n\n`= tags\n `+ plumb\n";
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
                "contentChanges": [{ "text": "`= tags\n `+ plumb\n" }]
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
                "contentChanges": [{ "text": "`node Parent\n  `meta\n    `= title\n\n      Nested\n" }]
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
    assert!(ranges
        .iter()
        .any(|range| range["collapsedText"] == "title  项目 Overview"));
    assert!(ranges
        .iter()
        .any(|range| range["collapsedText"] == "created  2026-08-05T03:46:54+08:00"));
    assert!(ranges.iter().any(|range| range["collapsedText"] == "tags"));
    assert_eq!(
        response(&run_server(&messages), 3)["result"][0],
        json!({ "startLine": 0, "endLine": 1, "collapsedText": "tags" })
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
    let source = "`= date 2026-08-02\n`= timezone +08:00\n`task Ready\n`event 14:00 Standup\n";
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
                "collapsedText": "date  2026-08-02"
            },
            {
                "startLine": 1,
                "startCharacter": 0,
                "endLine": 1,
                "endCharacter": source.lines().nth(1).unwrap().len(),
                "collapsedText": "timezone  +08:00"
            },
            {
                "startLine": 2,
                "startCharacter": 0,
                "endLine": 2,
                "endCharacter": source.lines().nth(2).unwrap().len(),
                "collapsedText": "`task [ ]  Ready"
            },
            {
                "startLine": 3,
                "startCharacter": 0,
                "endLine": 3,
                "endCharacter": source.lines().nth(3).unwrap().len(),
                "collapsedText": "`event 2026-08-02T14:00  Standup"
            }
        ])
    );
    assert_eq!(
        response(&run_server(&requests(true)), 2)["result"],
        json!([
            {
                "startLine": 0,
                "endLine": 0,
                "collapsedText": "date  2026-08-02"
            },
            {
                "startLine": 1,
                "endLine": 1,
                "collapsedText": "timezone  +08:00"
            },
            {
                "startLine": 2,
                "endLine": 2,
                "collapsedText": "`task [ ]  Ready"
            },
            {
                "startLine": 3,
                "endLine": 3,
                "collapsedText": "`event 2026-08-02T14:00  Standup"
            }
        ])
    );
}

#[test]
fn same_marker_fold_consumes_separator_and_preserves_changed_marker_boundary() {
    let uri = "file:///tmp/task-trailing-blank-fold.plumb";
    let source = "`note aaa\n\n bbb\n\n`note ccc\n\n detail\n\n`task aaa\n\n bbb\n\n`task ccc\n\n detail\n\n`- regular\n";
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
            { "startLine": 0, "endLine": 3 },
            { "startLine": 4, "endLine": 6 },
            { "startLine": 8, "endLine": 11, "collapsedText": "`task [ ]  aaa" },
            { "startLine": 12, "endLine": 14, "collapsedText": "`task [ ]  ccc" }
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
fn folds_with_locally_determined_labels_before_initial_index_completes() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("large.plumb"), "Paragraph.\n\n".repeat(150_000)).unwrap();
    let document = root.join("inbox.plumb");
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let document_uri = lsp_types::Url::from_file_path(&document).unwrap();
    let source = "`= title Workspace\n\n`event 14:00 Standup\n `= date 2026-08-02\n `= timezone +08:00\n `note Detail\n\n`task Blocker\n `@ blocker\n `note Detail\n\n`task Closed dependency\n `@ closed\n `= done 2026-08-01T00:00:00Z\n `note Detail\n\n`task Ready\n `note Detail\n\n`task Waiting\n `= wait 2099-01-01T00:00:00Z\n `= depends missing.plumb#task\n `note Detail\n\n`task Done\n `= done 2026-08-01T00:00:00Z\n `note Detail\n\n`task Canceled\n `= canceled 2026-08-01T00:00:00Z\n `note Detail\n\n`task Conflicted\n `= done 2026-08-01T00:00:00Z\n `= canceled 2026-08-01T00:01:00Z\n `note Detail\n\n`task Blocked\n `= depends #blocker\n `note Detail\n\n`task Resolved ready\n `= depends #closed\n `note Detail\n\n`task Unknown\n `= depends missing.plumb#task\n `note Detail\n";
    let first = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": { "textDocument": { "foldingRange": {
                    "foldingRange": { "collapsedText": true }
                }}}
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
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/foldingRange",
            "params": { "textDocument": { "uri": document_uri } }
        }),
    ];
    let shutdown = [
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server_with_pause(&first, &shutdown);
    let folding_index = output
        .iter()
        .position(|message| message.get("id") == Some(&json!(2)))
        .expect("folding response while initial indexing is running");
    if let Some(index_end) = output.iter().position(|message| {
        message["method"] == "$/progress" && message["params"]["value"]["kind"] == "end"
    }) {
        assert!(folding_index < index_end);
    }
    let ranges = response(&output, 2)["result"].as_array().unwrap();
    let labels = ranges
        .iter()
        .filter_map(|range| range["collapsedText"].as_str())
        .collect::<Vec<_>>();
    for expected in [
        "title  Workspace",
        "`event 2026-08-02T14:00  Standup",
        "`task [ ]  Blocker",
        "`task [o]  Closed dependency",
        "`task [ ]  Ready",
        "`task [~]  Waiting",
        "`task [o]  Done",
        "`task [x]  Canceled",
        "`task [ox] Conflicted",
        "`task [=]  Blocked",
        "`task [ ]  Resolved ready",
    ] {
        assert!(
            labels.contains(&expected),
            "missing {expected:?}: {labels:?}"
        );
    }
    let unknown_line = source[..source.find("`task Unknown").unwrap()]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count() as u64;
    let unknown = ranges
        .iter()
        .find(|range| range["startLine"] == unknown_line)
        .expect("unknown task retains its structural fold");
    assert!(unknown.get("collapsedText").is_none());

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn labels_task_folds_with_derived_workflow_states() {
    let uri = "file:///tmp/task-fold-labels.plumb";
    let source = "`task Ready task\n\n `@ blocker\n\n `note Detail\n\n`task Waiting task\n\n `= wait 2099-01-01T00:00:00Z\n `= depends #blocker\n\n `note Detail\n\n`task Done task\n\n `= done 2026-07-27T10:00:00Z\n\n `note Detail\n\n`task Canceled task\n\n `= canceled 2026-07-27T10:00:00Z\n\n `note Detail\n\n`task Conflicted task\n\n `= done 2026-07-27T10:00:00Z\n `= canceled 2026-07-27T10:01:00Z\n\n `note Detail\n\n`task Blocked task\n\n `= depends #blocker\n\n `note Detail\n\n`node Parent\n\n `task Nested task\n\n  `= done 2026-07-27T10:02:00Z\n\n  `note Detail\n";
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
            { "startLine": 0, "endLine": 5, "collapsedText": "`task [ ]  Ready task" },
            { "startLine": 6, "endLine": 12, "collapsedText": "`task [~]  Waiting task" },
            { "startLine": 13, "endLine": 18, "collapsedText": "`task [o]  Done task" },
            { "startLine": 19, "endLine": 24, "collapsedText": "`task [x]  Canceled task" },
            { "startLine": 25, "endLine": 31, "collapsedText": "`task [ox] Conflicted task" },
            { "startLine": 32, "endLine": 36, "collapsedText": "`task [=]  Blocked task" },
            { "startLine": 38, "endLine": 44 },
            { "startLine": 40, "endLine": 44, "collapsedText": " `task [o]  Nested task" }
        ])
    );
}

#[test]
fn labels_event_folds_with_abbreviated_times() {
    let uri = "file:///tmp/event-fold-labels.plumb";
    let source = "`event 14:00 Standup\n\n `= date 2026-08-02\n `= timezone +08:00\n\n `note Detail\n\n`event 09:00--10:30 Review\n\n `= date 2026-08-02\n `= timezone +08:00\n\n `note Detail\n\n`event 11:00 Parent\n\n `= date 2026-08-02\n `= timezone +08:00\n\n `note Detail\n\n `event 12:00 Nested\n\n  `= date 2026-08-02\n  `= timezone +08:00\n\n  `note Detail\n\n`event Untimed\n\n `note Detail\n";
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
            { "startLine": 0, "endLine": 6, "collapsedText": "`event 2026-08-02T14:00  Standup" },
            { "startLine": 7, "endLine": 13, "collapsedText": "`event 2026-08-02T09:00--10:30  Review" },
            { "startLine": 14, "endLine": 27, "collapsedText": "`event 2026-08-02T11:00  Parent" },
            { "startLine": 21, "endLine": 26, "collapsedText": " `event 2026-08-02T12:00  Nested" },
            { "startLine": 28, "endLine": 30 }
        ])
    );
}
