use serde_json::json;

use crate::support::{
    response, run_server, run_server_after_initial_index, run_server_after_response,
    unique_temp_dir,
};

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
    let changed_range = response(&run_server(&messages), 3)["result"][0].clone();
    assert_eq!(changed_range["startLine"], 0);
    assert_eq!(changed_range["endLine"], 1);
    assert!(
        changed_range.get("collapsedText").is_none() || changed_range["collapsedText"] == "tags"
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
        "`= date 2026-08-02\n`= timezone +08:00\n\n`- Ready\n\n `+ task\n\n`- 14:00 Standup\n\n `+ event\n";
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
                "startLine": 3,
                "endLine": 6,
                "collapsedText": "`- [ ]  Ready"
            },
            {
                "startLine": 7,
                "endLine": 9,
                "collapsedText": "`- 2026-08-02T14:00 Standup"
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
                "startLine": 3,
                "endLine": 6,
                "collapsedText": "`- [ ]  Ready"
            },
            {
                "startLine": 7,
                "endLine": 9,
                "collapsedText": "`- 2026-08-02T14:00 Standup"
            }
        ])
    );
}

#[test]
fn same_marker_fold_consumes_separator_and_preserves_changed_marker_boundary() {
    let uri = "file:///tmp/task-trailing-blank-fold.plumb";
    let source = "`note aaa\n\n bbb\n\n`note ccc\n\n detail\n\n`- aaa\n\n `+ task\n\n bbb\n\n`- ccc\n\n `+ task\n\n detail\n\n`- regular\n";
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
            { "startLine": 8, "endLine": 13, "collapsedText": "`- [ ]  aaa" },
            { "startLine": 14, "endLine": 19, "collapsedText": "`- [ ]  ccc" }
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
        response(&output, 1)["result"]["capabilities"]["experimental"]["plumb"]
            ["foldingRangeRefresh"]["method"],
        "workspace/foldingRange/refresh"
    );
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
    assert!(!output
        .iter()
        .any(|message| message["method"] == "workspace/foldingRange/refresh"));
}

#[test]
fn folds_with_locally_determined_labels_before_initial_index_completes() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("large.plumb"), "Paragraph.\n\n".repeat(150_000)).unwrap();
    let document = root.join("inbox.plumb");
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let document_uri = lsp_types::Url::from_file_path(&document).unwrap();
    let source = "`= title Workspace\n\n`- 14:00 Standup\n\n `+ event\n\n `= date 2026-08-02\n `= timezone +08:00\n\n `note Detail\n\n`- Blocker\n\n `+ task\n\n `@ blocker\n\n `note Detail\n\n`- Closed dependency\n\n `+ task\n\n `@ closed\n\n `= done 2026-08-01T00:00:00Z\n\n `note Detail\n\n`- Ready\n\n `+ task\n\n `note Detail\n\n`- Waiting\n\n `+ task\n\n `= wait 2099-01-01T00:00:00Z\n `= depends missing.plumb#task\n\n `note Detail\n\n`- Done\n\n `+ task\n\n `= done 2026-08-01T00:00:00Z\n\n `note Detail\n\n`- Canceled\n\n `+ task\n\n `= canceled 2026-08-01T00:00:00Z\n\n `note Detail\n\n`- Conflicted\n\n `+ task\n\n `= done 2026-08-01T00:00:00Z\n `= canceled 2026-08-01T00:01:00Z\n\n `note Detail\n\n`- Blocked\n\n `+ task\n\n `= depends #blocker\n\n `note Detail\n\n`- Resolved ready\n\n `+ task\n\n `= depends #closed\n\n `note Detail\n\n`- Unknown\n\n `+ task\n\n `= depends missing.plumb#task\n\n `note Detail\n";
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

    let output = run_server_after_response(&first, &shutdown);
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
        "`- 2026-08-02T14:00 Standup",
        "`- [ ]  Blocker",
        "`- [o]  Closed dependency",
        "`- [ ]  Ready",
        "`- [~]  Waiting",
        "`- [o]  Done",
        "`- [x]  Canceled",
        "`- [ox] Conflicted",
        "`- [=]  Blocked",
        "`- [ ]  Resolved ready",
    ] {
        assert!(
            labels.contains(&expected),
            "missing {expected:?}: {labels:?}"
        );
    }
    let unknown_line = source[..source.find("`- Unknown").unwrap()]
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
fn refreshes_folding_after_index_only_for_declared_clients() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("note.plumb"), "`- Indexed\n `+ task\n").unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let run = |support: bool| {
        let experimental =
            support.then(|| json!({ "plumb": { "foldingRangeRefreshSupport": true } }));
        let messages = [
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": root_uri,
                    "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                    "capabilities": { "experimental": experimental }
                }
            }),
            json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": null }),
            json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
        ];
        run_server_after_initial_index(&messages)
    };

    let supported = run(true);
    assert!(supported
        .iter()
        .any(|message| message["method"] == "workspace/foldingRange/refresh"));
    let unsupported = run(false);
    assert!(!unsupported
        .iter()
        .any(|message| message["method"] == "workspace/foldingRange/refresh"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn labels_task_folds_with_derived_workflow_states() {
    let uri = "file:///tmp/task-fold-labels.plumb";
    let source = "`- Ready task\n\n `+ task\n\n `@ blocker\n\n `note Detail\n\n`- Waiting task\n\n `+ task\n\n `= wait 2099-01-01T00:00:00Z\n `= depends #blocker\n\n `note Detail\n\n`- Done task\n\n `+ task\n\n `= done 2026-07-27T10:00:00Z\n\n `note Detail\n\n`- Canceled task\n\n `+ task\n\n `= canceled 2026-07-27T10:00:00Z\n\n `note Detail\n\n`- Conflicted task\n\n `+ task\n\n `= done 2026-07-27T10:00:00Z\n `= canceled 2026-07-27T10:01:00Z\n\n `note Detail\n\n`- Blocked task\n\n `+ task\n\n `= depends #blocker\n\n `note Detail\n\n`node Parent\n\n `- Nested task\n\n  `+ task\n\n  `= done 2026-07-27T10:02:00Z\n\n  `note Detail\n";
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
            { "startLine": 0, "endLine": 7, "collapsedText": "`- [ ]  Ready task" },
            { "startLine": 8, "endLine": 16, "collapsedText": "`- [~]  Waiting task" },
            { "startLine": 17, "endLine": 24, "collapsedText": "`- [o]  Done task" },
            { "startLine": 25, "endLine": 32, "collapsedText": "`- [x]  Canceled task" },
            { "startLine": 33, "endLine": 41, "collapsedText": "`- [ox] Conflicted task" },
            { "startLine": 42, "endLine": 48, "collapsedText": "`- [=]  Blocked task" },
            { "startLine": 50, "endLine": 58 },
            { "startLine": 52, "endLine": 58, "collapsedText": " `- [o]  Nested task" }
        ])
    );
}

#[test]
fn labels_event_folds_with_abbreviated_times() {
    let uri = "file:///tmp/event-fold-labels.plumb";
    let source = "`- 14:00 Standup\n\n `+ event\n\n `= date 2026-08-02\n `= timezone +08:00\n\n `note Detail\n\n`- 09:00--10:30 Review\n\n `+ event\n\n `= date 2026-08-02\n `= timezone +08:00\n\n `note Detail\n\n`- 11:00 Parent\n\n `+ event\n\n `= date 2026-08-02\n `= timezone +08:00\n\n `note Detail\n\n `- 12:00 Nested\n\n  `+ event\n\n  `= date 2026-08-02\n  `= timezone +08:00\n\n  `note Detail\n\n`- Untimed\n\n `+ event\n\n `note Detail\n";
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
            { "startLine": 0, "endLine": 8, "collapsedText": "`- 2026-08-02T14:00 Standup" },
            { "startLine": 9, "endLine": 17, "collapsedText": "`- 2026-08-02T09:00--10:30 Review" },
            { "startLine": 18, "endLine": 35, "collapsedText": "`- 2026-08-02T11:00 Parent" },
            { "startLine": 27, "endLine": 34, "collapsedText": " `- 2026-08-02T12:00 Nested" },
            { "startLine": 36, "endLine": 40 }
        ])
    );
}
