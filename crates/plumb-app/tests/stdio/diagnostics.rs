use serde_json::json;

use crate::support::{diagnostic_counts, response, run_server};

#[test]
fn diagnostics_clear_after_a_link_is_fixed() {
    let uri = "file:///tmp/fix-link.plumb";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": "plumb", "version": 1,
                "text": "See `->[missing|#missing].\n"
            }}
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [{
                    "text": "`node Target\n\n `@ target\n\nSee `->[target|#target].\n"
                }]
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let counts = diagnostic_counts(&run_server(&messages), uri);
    assert_eq!(counts.first(), Some(&1));
    assert_eq!(counts.last(), Some(&0));
}

#[test]
fn diagnostics_refresh_when_a_target_document_changes() {
    let source_uri = "file:///tmp/diagnostic-source.plumb";
    let target_uri = "file:///tmp/diagnostic-target.plumb";
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": source_uri, "languageId": "plumb", "version": 1,
                "text": "See `->[target|diagnostic-target.plumb#target].\n"
            }}
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": target_uri, "languageId": "plumb", "version": 1,
                "text": "No anchor yet.\n"
            }}
        }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": target_uri, "version": 2 },
                "contentChanges": [{ "text": "`node Target\n\n `@ target\n" }]
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let counts = diagnostic_counts(&run_server(&messages), source_uri);
    assert_eq!(counts.first(), Some(&1));
    assert_eq!(counts.last(), Some(&0));
}

#[test]
fn publishes_diagnostics_and_returns_heading_symbols_over_stdio() {
    let messages = [
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": "file:///tmp/first.plumb",
                    "languageId": "plumb",
                    "version": 1,
                    "text": "`# Root\n`## Child\n"
                }
            }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "textDocument/documentSymbol",
            "params": { "textDocument": { "uri": "file:///tmp/first.plumb" } }
        }),
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": "file:///tmp/first.plumb", "version": 2 },
                "contentChanges": [{ "text": "`span[open\n" }]
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let messages = run_server(&messages);
    let capabilities = &response(&messages, 1)["result"]["capabilities"];
    assert_eq!(
        capabilities["codeActionProvider"]["codeActionKinds"],
        json!(["quickfix", "refactor.rewrite"])
    );
    assert!(capabilities["completionProvider"]["triggerCharacters"]
        .as_array()
        .unwrap()
        .contains(&json!("[")));
    assert!(capabilities["completionProvider"]["triggerCharacters"]
        .as_array()
        .unwrap()
        .contains(&json!("`")));
    let symbols = messages
        .iter()
        .find(|message| message.get("id") == Some(&json!(2)))
        .expect("documentSymbol response");
    assert_eq!(symbols["result"][0]["name"], "Root");
    assert_eq!(symbols["result"][0]["children"][0]["name"], "Child");

    let diagnostics = messages
        .iter()
        .rfind(|message| message.get("method") == Some(&json!("textDocument/publishDiagnostics")))
        .expect("diagnostics notification");
    assert_eq!(diagnostics["params"]["version"], 2);
    assert_eq!(
        diagnostics["params"]["diagnostics"][0]["code"],
        "syntax.unclosed-inline"
    );
}

#[test]
fn nests_anchors_and_tasks_under_their_containing_headings() {
    let uri = "file:///tmp/symbol-containment.plumb";
    let source = "`# Project\n\n`node Note\n\n `@ note\n\n`task Write parser\n\n `@ write\n\n`## Section\n\n`node Inside\n\n `@ inside\n";
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
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/documentSymbol",
            "params": { "textDocument": { "uri": uri } }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let roots = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0]["name"], "Project");
    let children = roots[0]["children"].as_array().unwrap();
    assert_eq!(children.len(), 3);
    assert_eq!(children[0]["name"], "#note");
    assert_eq!(children[1]["name"], "Write parser");
    assert_eq!(children[1]["detail"], "open #write");
    assert_eq!(children[2]["name"], "Section");
    assert_eq!(children[2]["children"][0]["name"], "#inside");
}

#[test]
fn publishes_metadata_diagnostics_and_nested_symbols_over_stdio() {
    let source = "`= title\n\n Document title\n\n`= author\n `= name\n\n  Alice\n\n`= title\n\n`= created\n\n yesterday\n\nInvalid `cite[@old-style].\n";
    let messages = [
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": "file:///tmp/metadata.plumb",
                    "languageId": "plumb",
                    "version": 1,
                    "text": source
                }
            }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "textDocument/documentSymbol",
            "params": { "textDocument": { "uri": "file:///tmp/metadata.plumb" } }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let symbols = response(&output, 2);
    let metadata = symbols["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|symbol| symbol["name"] == "metadata")
        .expect("metadata symbol");
    assert_eq!(metadata["children"][0]["name"], "title");
    assert_eq!(metadata["children"][1]["name"], "author");
    assert_eq!(metadata["children"][1]["children"][0]["name"], "name");

    let diagnostics = output
        .iter()
        .rfind(|message| message.get("method") == Some(&json!("textDocument/publishDiagnostics")))
        .expect("diagnostics notification");
    assert!(diagnostics["params"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|diagnostic| diagnostic["code"] == "metadata.duplicate-key"));
    assert!(diagnostics["params"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|diagnostic| diagnostic["code"] == "citation.invalid"));
    let invalid_created = diagnostics["params"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|diagnostic| diagnostic["code"] == "metadata.invalid-created")
        .expect("invalid created diagnostic");
    assert_eq!(invalid_created["severity"], 2);
    assert_eq!(
        invalid_created["range"]["start"],
        json!({ "line": 13, "character": 1 })
    );
}
