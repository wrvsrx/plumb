use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

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
                "contentChanges": [{ "text": "`node{key=a key=b} Broken\n" }]
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let messages = run_server(&messages);
    let symbols = messages
        .iter()
        .find(|message| message.get("id") == Some(&json!(2)))
        .expect("documentSymbol response");
    assert_eq!(symbols["result"][0]["name"], "Root");
    assert_eq!(symbols["result"][0]["children"][0]["name"], "Child");

    let diagnostics = messages
        .iter()
        .filter(|message| message.get("method") == Some(&json!("textDocument/publishDiagnostics")))
        .last()
        .expect("diagnostics notification");
    assert_eq!(diagnostics["params"]["version"], 2);
    assert_eq!(
        diagnostics["params"]["diagnostics"][0]["code"],
        "syntax.duplicate-key"
    );
}

#[test]
fn resolves_cross_file_navigation_over_stdio() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let target = root.join("a.plumb");
    let source = root.join("b.plumb");
    std::fs::write(&target, "`#{#target} Target\n").unwrap();
    let source_text = "See `link[target]{to=\"a.plumb#target\"}.\n";
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
                "capabilities": { "workspace": { "workspaceEdit": { "documentChanges": true } } }
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

    std::fs::remove_dir_all(root).unwrap();
}

fn run_server(messages: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_plumb-ls"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start plumb-ls");
    {
        let stdin = child.stdin.as_mut().expect("child stdin");
        for message in messages {
            write_message(stdin, message);
        }
    }
    drop(child.stdin.take());
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .expect("child stdout")
        .read_to_string(&mut stdout)
        .expect("read stdout");
    let output = child.wait_with_output().expect("wait for plumb-ls");
    assert!(
        output.status.success(),
        "plumb-ls failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    read_messages(&stdout)
}

fn response(messages: &[Value], id: u64) -> &Value {
    messages
        .iter()
        .find(|message| message.get("id") == Some(&json!(id)))
        .expect("response")
}

fn unique_temp_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "plumb-ls-test-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

fn write_message(output: &mut impl Write, message: &Value) {
    let body = serde_json::to_vec(message).expect("encode message");
    write!(output, "Content-Length: {}\r\n\r\n", body.len()).expect("write header");
    output.write_all(&body).expect("write body");
    output.flush().expect("flush message");
}

fn read_messages(mut input: &str) -> Vec<Value> {
    let mut messages = Vec::new();
    while let Some(header_end) = input.find("\r\n\r\n") {
        let header = &input[..header_end];
        let length = header
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .expect("content length")
            .parse::<usize>()
            .expect("numeric content length");
        let body_start = header_end + 4;
        let body_end = body_start + length;
        messages.push(serde_json::from_str(&input[body_start..body_end]).expect("JSON-RPC body"));
        input = &input[body_end..];
    }
    messages
}
