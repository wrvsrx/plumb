use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{json, Value};

pub fn run_server(messages: &[Value]) -> Vec<Value> {
    let mut session = LspTestSession::new();
    session.send_until_shutdown(messages);
    session.wait_for_pending_responses();
    session.send_shutdown(messages, &[]);
    session.finish()
}

pub fn run_server_after_response(first: &[Value], second: &[Value]) -> Vec<Value> {
    let mut session = LspTestSession::new();
    session.send_all(first);
    let response_id = first
        .iter()
        .rev()
        .find_map(|message| message.get("id"))
        .expect("first LSP batch contains a request")
        .clone();
    session.wait_for_response(&response_id);
    session.send_until_shutdown(second);
    session.wait_for_pending_responses();
    session.send_shutdown(first, second);
    session.finish()
}

pub fn run_server_after_initial_index(messages: &[Value]) -> Vec<Value> {
    run_server_after_initial_index_with_action(messages, || {}, &[])
}

pub fn run_server_after_initial_index_with_action(
    first: &[Value],
    action: impl FnOnce(),
    second: &[Value],
) -> Vec<Value> {
    let mut session = LspTestSession::new();
    session.send_all(&first[..2]);
    session.wait_for(initial_index_complete);
    session.send_until_shutdown(&first[2..]);
    if !second.is_empty() {
        session.wait_for_pending_responses();
        action();
        session.send_until_shutdown(second);
    }
    session.wait_for_pending_responses();
    session.send_shutdown(first, second);
    session.finish()
}

pub struct LspTestSession {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    receiver: Receiver<Value>,
    reader: Option<JoinHandle<()>>,
    output: Vec<Value>,
    pending_response_ids: Vec<Value>,
    _cache: TestDirectory,
}

impl LspTestSession {
    pub fn new() -> Self {
        let cache = TestDirectory::new();
        let mut child = Command::new(env!("CARGO_BIN_EXE_plumb"))
            .arg("lsp")
            .env("PLUMB_CACHE_DIR", cache.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start plumb lsp");
        let stdin = child.stdin.take().expect("child stdin");
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
        Self {
            child: Some(child),
            stdin: Some(stdin),
            receiver,
            reader: Some(reader),
            output: Vec::new(),
            pending_response_ids: Vec::new(),
            _cache: cache,
        }
    }

    pub fn send(&mut self, message: &Value) {
        if message.get("method").is_some() && message["method"] != "shutdown" {
            if let Some(id) = message.get("id") {
                self.pending_response_ids.push(id.clone());
            }
        }
        write_message(self.stdin.as_mut().expect("open LSP stdin"), message);
    }

    pub fn send_all(&mut self, messages: &[Value]) {
        for message in messages {
            self.send(message);
        }
    }

    pub fn wait_for(&mut self, predicate: impl Fn(&Value) -> bool) -> Value {
        if let Some(message) = self.output.iter().find(|message| predicate(message)) {
            return message.clone();
        }
        loop {
            let message = match self.receiver.recv_timeout(Duration::from_secs(10)) {
                Ok(message) => message,
                Err(RecvTimeoutError::Timeout) => panic!("timed out waiting for LSP message"),
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("LSP stdout closed before expected message")
                }
            };
            let matched = predicate(&message);
            self.output.push(message.clone());
            if matched {
                return message;
            }
        }
    }

    pub fn wait_for_response(&mut self, id: &Value) -> Value {
        self.wait_for(|message| message.get("method").is_none() && message.get("id") == Some(id))
    }

    pub fn wait_for_pending_responses(&mut self) {
        let pending = std::mem::take(&mut self.pending_response_ids);
        for id in pending {
            self.wait_for_response(&id);
        }
    }

    pub fn finish(mut self) -> Vec<Value> {
        self.stdin.take();
        let status = self
            .child
            .take()
            .expect("LSP child")
            .wait_with_output()
            .expect("wait for plumb-ls");
        self.reader
            .take()
            .expect("LSP stdout reader")
            .join()
            .expect("join LSP stdout reader");
        self.output.extend(self.receiver.try_iter());
        assert!(
            status.status.success(),
            "plumb lsp failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
        std::mem::take(&mut self.output)
    }

    fn send_until_shutdown(&mut self, messages: &[Value]) {
        for message in messages {
            if message["method"] == "shutdown" {
                break;
            }
            self.send(message);
        }
    }

    fn send_shutdown(&mut self, first: &[Value], second: &[Value]) {
        let shutdown = first
            .iter()
            .chain(second)
            .find(|message| message["method"] == "shutdown")
            .expect("LSP exchange contains shutdown");
        let shutdown_id = shutdown.get("id").expect("shutdown request id").clone();
        self.send(shutdown);
        self.wait_for_response(&shutdown_id);
        let exit = first
            .iter()
            .chain(second)
            .find(|message| message["method"] == "exit")
            .expect("LSP exchange contains exit");
        self.send(exit);
    }
}

impl Drop for LspTestSession {
    fn drop(&mut self) {
        self.stdin.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn initial_index_complete(message: &Value) -> bool {
    message["method"] == "$/progress"
        && message["params"]["token"] == "plumb-ls-index"
        && message["params"]["value"]["kind"] == "end"
}

pub fn response(messages: &[Value], id: u64) -> &Value {
    messages
        .iter()
        .find(|message| message.get("method").is_none() && message.get("id") == Some(&json!(id)))
        .unwrap_or_else(|| panic!("response {id} missing from {messages:#?}"))
}

pub fn attribute_value<'a>(text: &'a str, key: &str) -> &'a str {
    text.lines()
        .filter_map(|line| line.trim_start().strip_prefix("`= "))
        .filter_map(|line| {
            let split = line.find(char::is_whitespace)?;
            Some((&line[..split], line[split..].trim()))
        })
        .find_map(|(candidate, value)| (candidate == key).then_some(value))
        .expect("property value")
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

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path = unique_temp_dir();
        std::fs::create_dir_all(&path).expect("create isolated LSP cache");
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn write_message(output: &mut impl Write, message: &Value) {
    let body = serde_json::to_vec(message).expect("encode message");
    write!(output, "Content-Length: {}\r\n\r\n", body.len()).expect("write header");
    output.write_all(&body).expect("write body");
    output.flush().expect("flush message");
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
