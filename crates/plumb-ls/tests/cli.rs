use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

#[test]
fn exposes_the_unified_command_surface() {
    let help = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    for command in [
        "check", "fmt", "export", "graph", "import", "note", "site", "task", "lsp",
    ] {
        assert!(help.contains(command));
    }

    let formatted = run_with_stdin(&["fmt"], "`meta\n   `: title\n\n      Unified command\n");
    assert!(formatted.status.success());
    assert_eq!(
        String::from_utf8(formatted.stdout).unwrap(),
        "`meta\n `: title\n\n    Unified command\n"
    );

    let exported = run_with_stdin(&["export"], "Paragraph.\n");
    assert!(exported.status.success());
    let document: serde_json::Value = serde_json::from_slice(&exported.stdout).unwrap();
    assert_eq!(document["blocks"][0]["t"], "Para");

    let imported = run_with_stdin(&["import"], &String::from_utf8(exported.stdout).unwrap());
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    assert_eq!(String::from_utf8(imported.stdout).unwrap(), "Paragraph.\n");
}

#[test]
fn checks_a_workspace_recursively_and_sets_the_exit_status() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(root.join("nested")).unwrap();
    std::fs::write(root.join("valid.plumb"), "Paragraph.\n").unwrap();
    let valid = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .args(["check", "--root"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        valid.status.success(),
        "{}",
        String::from_utf8_lossy(&valid.stderr)
    );
    assert!(valid.stdout.is_empty());

    std::fs::write(
        root.join("nested/broken.plumb"),
        "See `->[missing]{to=\"missing.plumb#id\"}.\n",
    )
    .unwrap();
    let broken = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .args(["check", "--root"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(!broken.status.success());
    assert!(broken.stderr.is_empty());
    let output = String::from_utf8(broken.stdout).unwrap();
    assert!(
        output.contains("nested/broken.plumb:1:")
            && output.contains("warning[link.unresolved-path]"),
        "{output}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn round_trips_the_exported_standard_profile_through_import() {
    let source = "`meta\n `: title\n\n    Import test\n\n`#{#intro} Intro\nParagraph with `*[emphasis], `![strong], `=[mark], `~[strike], `^[super], `_[sub], and `->[a link]{to=\"other.plumb#id\"}.\n\n`>{#quote .source} Quoted\n\n`-{.task #task created=\"2026-07-23T17:00:00+08:00\"} Item\n\n`{language=rust #code}\n  fn main() {}\n";
    let first = run_with_stdin(&["export"], source);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );

    let imported = run_with_stdin(&["import"], &String::from_utf8_lossy(&first.stdout));
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    let second = run_with_stdin(&["export"], &String::from_utf8_lossy(&imported.stdout));
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );

    let first: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    let second: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(
        second,
        first,
        "{}",
        String::from_utf8_lossy(&imported.stdout)
    );
}

#[test]
fn builds_and_serves_the_workspace_graph_with_rendered_notes() {
    if Command::new("pandoc").arg("--version").output().is_err() {
        return;
    }
    let root = unique_temp_dir();
    let output = unique_temp_dir();
    std::fs::create_dir_all(root.join("assets")).unwrap();
    std::fs::write(root.join("assets/icon.png"), b"png").unwrap();
    std::fs::write(
        root.join("a.plumb"),
        "`meta\n `: title\n\n    Alpha\n\nSee `->[Beta]{to=\"b.plumb#beta\"}.\n\n`img[icon]{src=\"assets/icon.png\"}\n",
    )
    .unwrap();
    std::fs::write(root.join("b.plumb"), "`#{#beta} Beta\n").unwrap();

    let built = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .args(["site", "build", "--root"])
        .arg(&root)
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    assert!(output.join("index.html").is_file());
    assert!(output.join("graph.json").is_file());

    let mut child = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .args(["graph", "--root"])
        .arg(&root)
        .arg("--no-open")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output_reader = BufReader::new(child.stdout.take().unwrap());
    let mut url = String::new();
    output_reader.read_line(&mut url).unwrap();
    let address = url
        .trim()
        .strip_prefix("http://")
        .unwrap()
        .trim_end_matches('/');
    let (status, headers, body) = http_get(address, "/api/graph");
    assert_eq!(status, 200, "{body}");
    assert!(headers.contains("application/json"), "{headers}");
    let graph: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(graph["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(graph["edges"].as_array().unwrap().len(), 1);
    let alpha = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["title"] == "Alpha")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let (status, _, note) = http_get(address, &format!("/api/note/{alpha}"));
    assert_eq!(status, 200, "{note}");
    let note: serde_json::Value = serde_json::from_str(&note).unwrap();
    assert_eq!(note["title"], "Alpha");
    assert!(note["html"].as_str().unwrap().contains("/note/"));
    let resource_path = note["html"]
        .as_str()
        .unwrap()
        .split("src=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let (status, _, resource) = http_get(address, resource_path);
    assert_eq!(status, 200);
    assert_eq!(resource, "png");
    let (status, _, _) = http_get(address, "/resource/../../Cargo.toml");
    assert_eq!(status, 404);

    std::fs::write(
        root.join("a.plumb"),
        "`meta\n `: title\n\n    Alpha updated\n\nSee `->[Beta]{to=\"b.plumb#beta\"}.\n",
    )
    .unwrap();
    let mut refreshed = false;
    for _ in 0..60 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let (status, _, body) = http_get(address, &format!("/api/note/{alpha}"));
        if status == 200 && body.contains("Alpha updated") {
            refreshed = true;
            break;
        }
    }
    assert!(
        refreshed,
        "workspace watcher did not invalidate the note cache"
    );
    let (status, headers, index) = http_get(address, "/");
    assert_eq!(status, 200, "{index}");
    assert!(headers.contains("content-security-policy"), "{headers}");
    assert!(index.contains("Workspace graph"));
    child.kill().unwrap();
    child.wait().unwrap();
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(output).unwrap();
}

fn http_get(address: &str, path: &str) -> (u16, String, String) {
    let mut stream = TcpStream::connect(address).unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.0\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (headers, body) = response.split_once("\r\n\r\n").unwrap();
    let status = headers
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    (status, headers.to_ascii_lowercase(), body.to_string())
}

fn run_with_stdin(args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn unique_temp_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "plumb-cli-test-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}
