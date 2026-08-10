use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

pub fn run_server(messages: &[Value]) -> Vec<Value> {
    run_server_with_writer(|stdin| {
        for message in messages {
            write_message(stdin, message);
        }
    })
}

pub fn run_server_with_pause(first: &[Value], second: &[Value]) -> Vec<Value> {
    run_server_with_writer(|stdin| {
        for message in first {
            write_message(stdin, message);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        for message in second {
            write_message(stdin, message);
        }
    })
}

pub fn run_server_after_initial_index(messages: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start plumb lsp");
    let stdout = child.stdout.take().expect("child stdout");
    let (sender, receiver) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut input = BufReader::new(stdout);
        while let Some(message) = read_message(&mut input) {
            if sender.send(message).is_err() {
                break;
            }
        }
    });
    let stdin = child.stdin.as_mut().expect("child stdin");
    for message in messages.iter().take(2) {
        write_message(stdin, message);
    }
    let mut output = Vec::new();
    loop {
        let message = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("initial workspace index completes");
        let complete = message["method"] == "$/progress"
            && message["params"]["token"] == "plumb-ls-index"
            && message["params"]["value"]["kind"] == "end";
        output.push(message);
        if complete {
            break;
        }
    }
    for (index, message) in messages.iter().enumerate().skip(2) {
        write_message(stdin, message);
        if index + 3 == messages.len() {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    drop(child.stdin.take());
    let status = child.wait_with_output().expect("wait for plumb-ls");
    assert!(
        status.status.success(),
        "plumb lsp failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    reader.join().expect("join LSP stdout reader");
    output.extend(receiver.try_iter());
    output
}

pub fn run_server_with_writer(
    write_messages: impl FnOnce(&mut std::process::ChildStdin),
) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start plumb lsp");
    {
        let stdin = child.stdin.as_mut().expect("child stdin");
        write_messages(stdin);
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
        "plumb lsp failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    read_messages(&stdout)
}

pub fn response(messages: &[Value], id: u64) -> &Value {
    messages
        .iter()
        .find(|message| message.get("id") == Some(&json!(id)))
        .expect("response")
}

pub fn attribute_value<'a>(text: &'a str, key: &str) -> &'a str {
    let needle = format!("`: {key} ");
    text.split_once(&needle)
        .and_then(|(_, value)| value.lines().next())
        .map(str::trim_end)
        .expect("attribute value")
}

pub fn diagnostic_counts(messages: &[Value], uri: &str) -> Vec<usize> {
    messages
        .iter()
        .filter(|message| {
            message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri
        })
        .map(|message| message["params"]["diagnostics"].as_array().unwrap().len())
        .collect()
}

pub fn unique_temp_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "plumb-ls-test-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

pub fn write_message(output: &mut impl Write, message: &Value) {
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

fn read_message(input: &mut impl BufRead) -> Option<Value> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if line == "\r\n" {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length: ") {
            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .expect("numeric content length"),
            );
        }
    }
    let mut body = vec![0; length.expect("content length")];
    input.read_exact(&mut body).expect("JSON-RPC body");
    Some(serde_json::from_slice(&body).expect("JSON-RPC JSON"))
}
