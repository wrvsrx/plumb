use chrono::Local;
use serde_json::json;

use crate::support::{attribute_value, response, run_server};

#[test]
fn inserts_metadata_code_action_only_for_valid_documents_without_metadata() {
    let uri = "file:///tmp/metadata-action.plumb";
    let messages = [
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": null,
                "capabilities": {
                    "workspace": { "workspaceEdit": { "documentChanges": true } }
                }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 3, "text": "`# Section\n"
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 0 }
                },
                "context": { "diagnostics": [], "only": ["refactor"] }
            }
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 4 },
                "contentChanges": [{
                    "text": "`= title Existing\n\n`# Section\n"
                }]
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 0 }
                },
                "context": { "diagnostics": [] }
            }
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 5 },
                "contentChanges": [{ "text": "`span{open\n" }]
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 0 }
                },
                "context": { "diagnostics": [] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 5, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    assert!(
        response(&output, 1)["result"]["capabilities"]["codeActionProvider"]["codeActionKinds"]
            .as_array()
            .unwrap()
            .contains(&json!("refactor.rewrite"))
    );
    let actions = response(&output, 2)["result"].as_array().unwrap();
    let metadata = actions
        .iter()
        .find(|action| action["title"] == "Insert document metadata")
        .unwrap();
    assert_eq!(metadata["kind"], "refactor.rewrite");
    let change = &metadata["edit"]["documentChanges"][0];
    assert_eq!(change["textDocument"]["version"], 3);
    assert_eq!(change["edits"][0]["range"]["start"]["line"], 0);
    assert_eq!(change["edits"][0]["range"]["start"]["character"], 0);
    let new_text = change["edits"][0]["newText"].as_str().unwrap();
    assert_eq!(attribute_value(new_text, "title"), "metadata-action");
    let created = attribute_value(new_text, "created");
    chrono::DateTime::parse_from_rfc3339(created).expect("created is an RFC 3339 timestamp");
    assert!(response(&output, 3)["result"]
        .as_array()
        .map(|actions| actions
            .iter()
            .all(|action| action["title"] != "Insert document metadata"))
        .unwrap_or(true));
    assert!(response(&output, 4)["result"].is_null());
}

#[test]
fn omits_metadata_code_action_when_cursor_is_not_at_document_start() {
    let uri = "file:///tmp/metadata-cursor.plumb";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": null,
                "capabilities": {
                    "workspace": { "workspaceEdit": { "documentChanges": true } }
                }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 1, "text": "`# Section\n"
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 4 },
                    "end": { "line": 0, "character": 4 }
                },
                "context": { "diagnostics": [], "only": ["refactor.rewrite"] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let no_metadata = response(&output, 2)["result"]
        .as_array()
        .map(|actions| {
            !actions
                .iter()
                .any(|action| action["title"] == "Insert document metadata")
        })
        .unwrap_or(true);
    assert!(no_metadata);
}

#[test]
fn aligns_block_arguments_only_when_the_sibling_run_needs_it() {
    let uri = "file:///tmp/argument-alignment.plumb";
    let source = "`row a one x\n`row alphabet two y\n";
    let aligned = "`row a        one x\n`row alphabet two y\n";
    let tabbed = "`row a\t|one|x\n`row alphabet|two|y\n";
    let request = |id| {
        json!({
            "jsonrpc": "2.0", "id": id, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 5 },
                    "end": { "line": 0, "character": 5 }
                },
                "context": { "diagnostics": [], "only": ["refactor.rewrite"] }
            }
        })
    };
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": null,
                "capabilities": {
                    "workspace": { "workspaceEdit": { "documentChanges": true } }
                }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 3, "text": source
            }}
        }),
        request(2),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 4 },
                "contentChanges": [{ "text": aligned }]
            }
        }),
        request(3),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 5 },
                "contentChanges": [{ "text": tabbed }]
            }
        }),
        request(4),
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
    assert_eq!(action["kind"], "refactor.rewrite");
    assert_eq!(
        action["edit"]["documentChanges"][0]["textDocument"]["version"],
        3
    );
    assert_eq!(
        apply_ascii_lsp_edits(
            source,
            action["edit"]["documentChanges"][0]["edits"]
                .as_array()
                .unwrap()
        ),
        aligned
    );
    for id in [3, 4] {
        assert!(response(&output, id)["result"]
            .as_array()
            .map(|actions| actions
                .iter()
                .all(|action| action["title"] != "Align arguments"))
            .unwrap_or(true));
    }
}

#[test]
fn inserts_metadata_into_an_empty_document_over_stdio() {
    let uri = "file:///tmp/empty%20note.plumb";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": null,
                "capabilities": {
                    "workspace": { "workspaceEdit": { "documentChanges": true } }
                }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 7, "text": ""
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 0 }
                },
                "context": { "diagnostics": [], "only": ["refactor.rewrite"] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let metadata = response(&output, 2)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == "Insert document metadata")
        .expect("metadata action");
    let change = &metadata["edit"]["documentChanges"][0];
    assert_eq!(change["textDocument"]["version"], 7);
    assert_eq!(
        change["edits"][0]["range"]["start"],
        json!({ "line": 0, "character": 0 })
    );
    assert_eq!(
        change["edits"][0]["range"]["end"],
        json!({ "line": 0, "character": 0 })
    );
    let generated = change["edits"][0]["newText"].as_str().unwrap();
    assert_eq!(attribute_value(generated, "title"), "empty note");
    assert!(
        generated.starts_with("`= title   empty note\n"),
        "{generated:?}"
    );
    chrono::DateTime::parse_from_rfc3339(attribute_value(generated, "created")).unwrap();
    let parsed = plumb_syntax::parse(generated);
    assert!(
        plumb_edit::format(&parsed, plumb_edit::FormatScope::Document)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn omits_metadata_code_action_without_guarded_edit_support() {
    let uri = "file:///tmp/no-guarded-edits.plumb";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 1, "text": "Content\n"
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 0 }
                },
                "context": { "diagnostics": [] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    assert!(response(&output, 2)["result"].is_null());
}

#[test]
fn offers_add_explicit_id_for_the_deepest_unanchored_block() {
    let uri = "file:///tmp/add-explicit-id.plumb";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": null,
                "capabilities": {
                    "workspace": { "workspaceEdit": { "documentChanges": true } }
                }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 4,
                "text": "`# Existing\n `@ same-title\n\n`node Parent\n\n      `child 😀 Same title\n"
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 5, "character": 20 },
                    "end": { "line": 5, "character": 20 }
                },
                "context": { "diagnostics": [], "only": ["refactor.rewrite"] }
            }
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 5 },
                "contentChanges": [{
                    "text": "`# Existing\n `@ same-title\n\n`node Parent\n\n      `child 😀 Same title\n       `@ nested\n"
                }]
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 5, "character": 20 },
                    "end": { "line": 5, "character": 20 }
                },
                "context": { "diagnostics": [], "only": ["refactor.rewrite"] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let action = response(&output, 2)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == "Add explicit id")
        .unwrap();
    assert_eq!(action["kind"], "refactor.rewrite");
    assert_eq!(action["isPreferred"], true);
    let change = &action["edit"]["documentChanges"][0];
    assert_eq!(change["textDocument"]["version"], 4);
    assert!(
        change["edits"][0]["newText"]
            .as_str()
            .unwrap()
            .contains("`@ same-title-2"),
        "{change:#}"
    );
    assert_eq!(change["edits"][0]["range"]["start"]["line"], 5);
    assert_eq!(change["edits"][0]["range"]["start"]["character"], 0);

    assert!(response(&output, 3)["result"]
        .as_array()
        .map(|actions| actions
            .iter()
            .all(|action| action["title"] != "Add explicit id"))
        .unwrap_or(true));
}

#[test]
fn converts_event_shorthand_with_a_refactor_action() {
    let uri = "file:///tmp/event-shorthand.plumb";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": null,
                "capabilities": {
                    "workspace": { "workspaceEdit": { "documentChanges": true } }
                }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 3,
                "text": "`- 2026-05-21T11:10--11:20 relax: `\"phone\"\n"
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 28 },
                    "end": { "line": 0, "character": 28 }
                },
                "context": { "diagnostics": [], "only": ["refactor.rewrite"] }
            }
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 4 },
                "contentChanges": [{ "text": "`- Meeting at 11\n" }]
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 5 },
                    "end": { "line": 0, "character": 5 }
                },
                "context": { "diagnostics": [], "only": ["refactor.rewrite"] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let action = response(&output, 2)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == "Convert to event")
        .unwrap();
    assert_eq!(action["kind"], "refactor.rewrite");
    assert_eq!(action["isPreferred"], true);
    let change = &action["edit"]["documentChanges"][0];
    assert_eq!(change["textDocument"]["version"], 3);
    let replacement = change["edits"][0]["newText"].as_str().unwrap();
    assert!(
        replacement.starts_with("`- 11:10--11:20 {relax: `\"phone\"}\n"),
        "{replacement}"
    );
    assert!(replacement.contains(" `+ event\n"), "{replacement}");
    assert!(
        replacement.contains(" `= date     2026-05-21\n"),
        "{replacement}"
    );
    let timezone = Local::now().fixed_offset().format("%:z").to_string();
    assert!(
        replacement.contains(&format!(" `= timezone {timezone}\n")),
        "{replacement}"
    );
    assert!(!replacement.contains("#e0001"), "{replacement}");
    assert!(!replacement.contains("event-uids"), "{replacement}");
    assert!(!replacement.contains("`uid "), "{replacement}");

    assert!(response(&output, 3)["result"]
        .as_array()
        .map(|actions| {
            !actions
                .iter()
                .any(|action| action["title"] == "Convert to event")
        })
        .unwrap_or(true));
}

#[test]
fn converts_selected_event_shorthands_with_a_refactor_action() {
    let uri = "file:///tmp/event-shorthands.plumb";
    let source = "`= date 2026-08-01\n`= timezone +08:00\n\n`- 10:00-- first\n`- 10:20-- second\n`- 10:30--10:40 third\n";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": null,
                "capabilities": {
                    "workspace": { "workspaceEdit": { "documentChanges": true } }
                }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 3, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 3, "character": 0 },
                    "end": { "line": 6, "character": 0 }
                },
                "context": { "diagnostics": [], "only": ["refactor.rewrite"] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let action = response(&output, 2)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == "Convert selected items to events")
        .unwrap();
    let edits = action["edit"]["documentChanges"][0]["edits"]
        .as_array()
        .unwrap();
    assert_eq!(edits.len(), 3, "{action:#}");
    let replacements = edits
        .iter()
        .map(|edit| edit["newText"].as_str().unwrap())
        .collect::<String>();
    assert_eq!(replacements.matches("`+ event").count(), 3);
    assert!(!replacements.contains("@plumb.local"));
    assert!(replacements.contains("`- 10:00--10:20 first\n"));
    assert!(replacements.contains("`- 10:20--10:30 second\n"));
    assert!(replacements.contains("`- 10:30--10:40 third\n"));
}

#[test]
fn offers_task_authoring_refactor_actions() {
    let uri = "file:///tmp/task-authoring.plumb";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": null,
                "capabilities": {
                    "workspace": { "workspaceEdit": { "documentChanges": true } }
                }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 1,
                "text": "`- List item\n `@ keep\n `+ kind\n"
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 5 },
                    "end": { "line": 0, "character": 5 }
                },
                "context": { "diagnostics": [], "only": ["refactor.rewrite"] }
            }
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [{
                    "text": "`- Closed\n\n `+ task\n\n `@ closed\n\n `= done 2026-07-20T09:00:00Z\n"
                }]
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 5 },
                    "end": { "line": 0, "character": 5 }
                },
                "context": { "diagnostics": [], "only": ["refactor.rewrite"] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let conversion = response(&output, 2)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == "Convert to task")
        .unwrap();
    assert_eq!(conversion["kind"], "refactor.rewrite");
    let inserted = conversion["edit"]["documentChanges"][0]["edits"][0]["newText"]
        .as_str()
        .unwrap();
    assert!(inserted.starts_with("`- List item"), "{inserted}");
    assert!(inserted.contains(" `+ task\n"), "{inserted}");
    chrono::DateTime::parse_from_rfc3339(attribute_value(inserted, "created")).unwrap();

    let created = response(&output, 3)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == "Add task created timestamp")
        .unwrap();
    assert_eq!(created["kind"], "refactor.rewrite");
    let created_text = created["edit"]["documentChanges"][0]["edits"][0]["newText"]
        .as_str()
        .unwrap();
    chrono::DateTime::parse_from_rfc3339(attribute_value(created_text, "created")).unwrap();
}

#[test]
fn offers_guarded_task_status_code_actions() {
    let uri = "file:///tmp/task-actions.plumb";
    let source = "`- MJCF in, USD out solver\n\n `+ task\n\n `@ task-f81deb18\n\n `= created 2026-05-24T02:35:50Z\n\n `- parse MJCF\n\n  `+ task\n\n  `@ task-c2cf5756\n\n  `= created 2026-05-27T13:03:04Z\n\n `- solver with passive joint\n\n  `+ task\n\n  `@ task-99e28dad\n\n  `= created 2026-05-27T13:02:45Z\n";
    let line = source
        .lines()
        .position(|line| line.contains("parse MJCF"))
        .unwrap();
    let character = source
        .lines()
        .nth(line)
        .unwrap()
        .find("parse MJCF")
        .unwrap();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": null,
                "capabilities": {
                    "workspace": { "workspaceEdit": { "documentChanges": true } }
                }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 3, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": line, "character": character },
                    "end": { "line": line, "character": character }
                },
                "context": { "diagnostics": [], "only": ["quickfix"] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let actions = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0]["title"], "Complete task");
    assert_eq!(actions[1]["title"], "Cancel task");
    for (action, attribute) in actions.iter().zip(["done", "canceled"]) {
        assert_eq!(action["kind"], "quickfix");
        let change = &action["edit"]["documentChanges"][0];
        assert_eq!(change["textDocument"]["version"], 3);
        let new_text = change["edits"][0]["newText"].as_str().unwrap();
        assert!(new_text.contains("`@ task-c2cf5756"));
        assert!(!new_text.contains("`@ task-f81deb18"));
        assert!(!new_text.contains("`@ task-99e28dad"));
        let timestamp = attribute_value(new_text, attribute);
        chrono::DateTime::parse_from_rfc3339(timestamp).unwrap();
    }
}

fn apply_ascii_lsp_edits(source: &str, edits: &[serde_json::Value]) -> String {
    let mut edits = edits
        .iter()
        .map(|edit| {
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
            let range = &edit["range"];
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
fn recurring_task_action_closes_current_and_appends_next_instance() {
    let uri = "file:///tmp/recurring-task.plumb";
    let source =
        "`- Weekly review\n\n `+ task\n\n `= due 2026-07-20T09:00:00+08:00\n `= recur P1W\n";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": null,
                "capabilities": {
                    "workspace": { "workspaceEdit": { "documentChanges": true } }
                }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 2, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 5 },
                    "end": { "line": 0, "character": 5 }
                },
                "context": { "diagnostics": [], "only": ["quickfix"] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let actions = response(&output, 2)["result"].as_array().unwrap();
    let complete = actions
        .iter()
        .find(|action| action["title"] == "Complete task")
        .unwrap();
    let edits = complete["edit"]["documentChanges"][0]["edits"]
        .as_array()
        .unwrap();
    assert_eq!(edits.len(), 1);
    let replacement = edits[0]["newText"].as_str().unwrap();
    assert!(replacement.contains("`@ weekly-review-2026-07-20"));
    assert!(replacement.contains("`= done "));
    assert!(replacement.contains("`@ weekly-review-2026-07-27"));
    assert!(replacement.contains("2026-07-27T09:00:00+08:00"));
    assert!(replacement.contains("#weekly-review-2026-07-20"));
}

#[test]
fn blocked_task_offers_cancel_but_not_complete() {
    let uri = "file:///tmp/blocked-task-actions.plumb";
    let source = "`- Draft\n\n `+ task\n\n `@ draft\n\n`- Review\n\n `+ task\n\n `@ review\n\n `= depends #draft\n";
    let line = source
        .lines()
        .position(|line| line.contains("Review"))
        .unwrap();
    let character = source.lines().nth(line).unwrap().find("Review").unwrap();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": null,
                "capabilities": {
                    "workspace": { "workspaceEdit": { "documentChanges": true } }
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
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": line, "character": character },
                    "end": { "line": line, "character": character }
                },
                "context": { "diagnostics": [], "only": ["quickfix"] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let actions = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0]["title"], "Cancel task");
    let new_text = actions[0]["edit"]["documentChanges"][0]["edits"][0]["newText"]
        .as_str()
        .unwrap();
    chrono::DateTime::parse_from_rfc3339(attribute_value(new_text, "canceled")).unwrap();
}

#[test]
fn canceling_a_recurring_task_appends_the_next_instance() {
    let uri = "file:///tmp/cancel-recurring-task.plumb";
    let source =
        "`- Weekly review\n\n `+ task\n\n `= due 2026-07-20T09:00:00+08:00\n `= recur P1W\n";
    let cursor = source.find("Weekly review").unwrap();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": null,
                "capabilities": {
                    "workspace": { "workspaceEdit": { "documentChanges": true } }
                }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 4, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": cursor },
                    "end": { "line": 0, "character": cursor }
                },
                "context": { "diagnostics": [], "only": ["quickfix"] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let cancel = response(&output, 2)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == "Cancel task")
        .expect("Cancel task action");
    let edits = cancel["edit"]["documentChanges"][0]["edits"]
        .as_array()
        .unwrap();
    assert_eq!(edits.len(), 1);
    let replacement = edits[0]["newText"].as_str().unwrap();
    assert!(replacement.contains("`@ weekly-review-2026-07-20"));
    assert!(replacement.contains("`= canceled "));
    assert!(replacement.contains("`@ weekly-review-2026-07-27"));
    assert!(replacement.contains("2026-07-27T09:00:00+08:00"));
    assert!(replacement.contains("#weekly-review-2026-07-20"));
}

#[test]
fn task_actions_fall_back_from_closed_child_to_open_parent() {
    let uri = "file:///tmp/nested-task-actions.plumb";
    let source = "`- Outer\n\n `+ task\n\n `@ outer\n\n `- Inner\n\n  `+ task\n\n  `@ inner\n\n  `= done 2026-07-20T09:00:00Z\n";
    let cursor = source.find("Inner").unwrap();
    let line_start = source[..cursor].rfind('\n').unwrap() + 1;
    let character = cursor - line_start;
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": null,
                "capabilities": {
                    "workspace": { "workspaceEdit": { "documentChanges": true } }
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
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 1, "character": character },
                    "end": { "line": 1, "character": character }
                },
                "context": { "diagnostics": [], "only": ["quickfix"] }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let output = run_server(&messages);
    let actions = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(actions.len(), 2);
    for action in actions {
        let edit = &action["edit"]["documentChanges"][0]["edits"][0];
        assert_eq!(edit["range"]["start"]["line"], 0);
        assert!(edit["newText"].as_str().unwrap().contains("2026-"));
    }
}
