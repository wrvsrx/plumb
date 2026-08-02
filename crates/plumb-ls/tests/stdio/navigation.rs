use serde_json::json;

use crate::support::{response, run_server, run_server_with_pause, unique_temp_dir};

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
    assert_eq!(
        response(&output, 1)["result"]["capabilities"]["semanticTokensProvider"]["legend"]
            ["tokenModifiers"],
        json!(["completed", "canceled"])
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
    assert!(hover.contains("**State:** waiting"));
    assert!(hover.contains("**Waiting for:** dependency"));
    assert!(hover.contains("**Recur:** `P1M1D`"));
    assert!(hover.contains("**Depends:** `blockers.plumb#draft`"));
    assert!(hover.contains("**Open blockers:** `blockers.plumb#draft`"));

    let semantic_data = response(&output, 4)["result"]["data"].as_array().unwrap();
    assert_eq!(semantic_data.len(), 5);
    assert_eq!(semantic_data[3], 0);
    assert_eq!(semantic_data[4], 1);

    let diagnostics = output
        .iter()
        .rfind(|message| message.get("method") == Some(&json!("textDocument/publishDiagnostics")))
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
fn publishes_event_symbols_hover_references_and_diagnostics() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("agenda.plumb");
    let source = "`-{.task #write} Write\n`-{.event #review uid=\"review@example\" date=2026-07-30 timezone=\"+08:00\" when=\"14:00--15:00\" tasks=\"#write\"} Review\n`-{.event .task date=2026-07-30 timezone=\"+08:00\" when=\"16:00\"} Conflict\n";
    std::fs::write(&path, source).unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let uri = lsp_types::Url::from_file_path(&path).unwrap();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": root_uri, "capabilities": {} }
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
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/hover",
            "params": { "textDocument": { "uri": uri }, "position": { "line": 1, "character": 12 } }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/references",
            "params": { "textDocument": { "uri": uri }, "position": { "line": 0, "character": 11 }, "context": { "includeDeclaration": false } }
        }),
        json!({ "jsonrpc": "2.0", "id": 5, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let output = run_server(&messages);
    let symbols = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(symbols.len(), 3);
    assert_eq!(symbols[1]["name"], "Review");
    assert!(symbols[1]["detail"]
        .as_str()
        .unwrap()
        .contains("2026-07-30T14:00:00+08:00 #review"));
    let hover = response(&output, 3)["result"]["contents"]["value"]
        .as_str()
        .unwrap();
    assert!(hover.contains("**Event:** Review"), "{hover}");
    assert!(hover.contains("**Tasks:** `#write`"), "{hover}");
    assert_eq!(response(&output, 4)["result"].as_array().unwrap().len(), 1);
    let diagnostics = output
        .iter()
        .rfind(|message| message.get("method") == Some(&json!("textDocument/publishDiagnostics")))
        .unwrap()["params"]["diagnostics"]
        .as_array()
        .unwrap();
    assert!(diagnostics
        .iter()
        .any(|diagnostic| diagnostic["code"] == "event.conflicting-task-facet"));
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
            0, 0, 3, 0, 1, 1, 3, 5, 0, 1, 1, 3, 27, 0, 1, 1, 2, 6, 0, 1, 1, 0, 3, 0, 2, 1, 3, 5, 0,
            2, 1, 3, 31, 0, 2, 1, 2, 10, 0, 2
        ])
    );
}

#[test]
fn publishes_completed_task_consistency_diagnostics() {
    let uri = "file:///tmp/completed-task-consistency.plumb";
    let source = "`-{.task #dependency-parent done=\"2026-07-27T10:00:00Z\" depends=\"#child\"} Dependency parent\n  `-{.task #child} Open explicit child\n`-{.task done=\"2026-07-27T10:01:00Z\"} Descendant parent\n  `-{.task} Open implicit child\n";
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
        json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let diagnostics = output
        .iter()
        .rfind(|message| message.get("method") == Some(&json!("textDocument/publishDiagnostics")))
        .unwrap()["params"]["diagnostics"]
        .as_array()
        .unwrap();
    let dependency = diagnostics
        .iter()
        .find(|diagnostic| diagnostic["code"] == "task.done-with-open-dependency")
        .unwrap();
    assert_eq!(dependency["severity"], 2);
    assert_eq!(
        dependency["relatedInformation"].as_array().unwrap().len(),
        1
    );
    let descendant = diagnostics
        .iter()
        .find(|diagnostic| diagnostic["code"] == "task.done-with-open-descendant")
        .unwrap();
    assert_eq!(descendant["severity"], 2);
    assert_eq!(
        descendant["relatedInformation"].as_array().unwrap().len(),
        1
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
fn block_reference_code_lenses_use_block_openers() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let document = root.join("positions.plumb");
    let source = "`#{#heading} Heading\n`-{.task #task} Task\n`node{#block} Block\n`-{\n   .task #multiline\n  } Multiline\n`outer\n  `node{#nested} Nested\nParagraph `span[text]{#inline}.\n`{#raw}\n  payload\n";
    std::fs::write(&document, source).unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let document_uri = lsp_types::Url::from_file_path(&document).unwrap();
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
                "uri": document_uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeLens",
            "params": { "textDocument": { "uri": document_uri } }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let lenses = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(lenses.len(), 7);
    for (lens, expected_line, expected_character) in [
        (&lenses[0], 0, 0),
        (&lenses[1], 1, 0),
        (&lenses[2], 2, 0),
        (&lenses[3], 3, 0),
        (&lenses[4], 7, 2),
        (&lenses[6], 9, 0),
    ] {
        assert_eq!(lens["range"]["start"]["line"], expected_line);
        assert_eq!(lens["range"]["start"]["character"], expected_character);
        assert_eq!(lens["range"]["end"], lens["range"]["start"]);
    }
    assert_eq!(lenses[5]["range"]["start"]["line"], 8);
    assert!(lenses[5]["range"]["start"]["character"].as_u64().unwrap() > 0);
    std::fs::remove_dir_all(root).unwrap();
}
