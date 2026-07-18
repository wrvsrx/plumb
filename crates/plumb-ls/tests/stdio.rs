use std::io::{Read, Write};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

#[test]
fn publishes_diagnostics_and_returns_heading_symbols_over_stdio() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_plumb-ls"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start plumb-ls");

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

    {
        let stdin = child.stdin.as_mut().expect("child stdin");
        for message in messages {
            write_message(stdin, &message);
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

    let messages = read_messages(&stdout);
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
