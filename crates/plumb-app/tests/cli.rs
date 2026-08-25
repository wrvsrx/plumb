use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
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
        "check", "event", "fmt", "export", "import", "migrate", "note", "site", "task", "lsp",
    ] {
        assert!(help.contains(command));
    }
    let removed_command = ["migrate", "attributes"].join("-");
    let removed_migration = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .arg(&removed_command)
        .output()
        .unwrap();
    assert!(!removed_migration.status.success());
    assert!(String::from_utf8_lossy(&removed_migration.stderr).contains("unknown command"));

    let serve_help = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .args(["site", "serve", "--help"])
        .output()
        .unwrap();
    assert!(serve_help.status.success());
    let serve_help = String::from_utf8(serve_help.stdout).unwrap();
    assert!(serve_help.contains("--public-origin"));
    assert!(!serve_help.contains("--no-open"));

    for origin in ["file:///tmp/site", "https://example.test/path"] {
        let invalid_origin = Command::new(env!("CARGO_BIN_EXE_plumb"))
            .args(["site", "serve", "--public-origin", origin])
            .output()
            .unwrap();
        assert!(!invalid_origin.status.success());
        assert!(String::from_utf8_lossy(&invalid_origin.stderr)
            .contains("origin must contain only an http(s) scheme and authority"));
    }

    let obsolete_option = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .args(["site", "serve", "--no-open"])
        .output()
        .unwrap();
    assert!(!obsolete_option.status.success());
    assert!(String::from_utf8(obsolete_option.stderr)
        .unwrap()
        .contains("unexpected argument '--no-open'"));

    let formatted = run_with_stdin(&["fmt"], "`meta\n   `: title\n\n      Unified command\n");
    assert!(formatted.status.success());
    assert_eq!(
        String::from_utf8(formatted.stdout).unwrap(),
        "`meta\n `: title\n\n  Unified command\n"
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
fn migrates_an_explicit_syntax_epoch_from_stdin_and_paths() {
    let source = "`->[guide]{`:[to guide.plumb] `-[external]}\n";
    let expected = "`->[guide|guide.plumb|+[external]]\n";

    let stdin = run_with_stdin(&["migrate", "--from", "attached-v1"], source);
    assert!(
        stdin.status.success(),
        "{}",
        String::from_utf8_lossy(&stdin.stderr)
    );
    assert_eq!(String::from_utf8(stdin.stdout).unwrap(), expected);

    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("legacy.plumb");
    std::fs::write(&path, source).unwrap();
    let check = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .args(["migrate", "--from", "attached-v1", "--check"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(check.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&check.stderr).contains("would migrate"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), source);

    let migrated = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .args(["migrate", "--from", "attached-v1"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        migrated.status.success(),
        "{}",
        String::from_utf8_lossy(&migrated.stderr)
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn exports_events_as_a_khal_readonly_vdir() {
    let root = unique_temp_dir();
    let output = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("agenda.plumb"),
        "{\n  `= date 2026-07-30\n  `= timezone +08:00\n}\n\n`event 14:00--15:00 Parser review {\n  `@ review\n  `= tasks #write\n}\n",
    )
    .unwrap();
    let exported = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .args(["event", "--root"])
        .arg(&root)
        .arg("export-vdir")
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        exported.status.success(),
        "{}",
        String::from_utf8_lossy(&exported.stderr)
    );
    assert!(output.join(".plumb-vdir").is_file());
    assert_eq!(
        std::fs::read_dir(&output)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "ics"))
            .count(),
        1
    );

    if Command::new("khal").arg("--version").output().is_ok() {
        let config = root.join("khal.conf");
        std::fs::write(
            &config,
            format!(
                "[calendars]\n[[plumb]]\npath = {}\ntype = calendar\nreadonly = true\n\n[locale]\ntimeformat = %H:%M\ndateformat = %Y-%m-%d\nlongdateformat = %Y-%m-%d\ndatetimeformat = %Y-%m-%d %H:%M\nlongdatetimeformat = %Y-%m-%d %H:%M\ndefault_timezone = UTC\nlocal_timezone = UTC\n",
                output.display()
            ),
        )
        .unwrap();
        let listed = Command::new("khal")
            .args(["--no-color", "--config"])
            .arg(config)
            .args(["list", "2026-07-30", "2026-07-31"])
            .output()
            .unwrap();
        assert!(
            listed.status.success(),
            "{}",
            String::from_utf8_lossy(&listed.stderr)
        );
        assert!(
            String::from_utf8_lossy(&listed.stdout).contains("Parser review"),
            "{}",
            String::from_utf8_lossy(&listed.stdout)
        );
    }
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(output).unwrap();
}

#[test]
fn bundled_neovim_plugin_matches_the_binary_version() {
    let version = include_str!("../../../contrib/nvim/lua/plumb/version.lua");
    assert!(
        version.contains(&format!("version = '{}'", env!("CARGO_PKG_VERSION"))),
        "{version}"
    );
}

#[test]
fn generated_readme_matches_its_plumb_source() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let exported = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .arg("export")
        .arg(root.join("README.plumb"))
        .output()
        .unwrap();
    assert!(
        exported.status.success(),
        "{}",
        String::from_utf8_lossy(&exported.stderr)
    );

    let mut pandoc = Command::new("pandoc")
        .args(["--from=json", "--to=gfm", "--wrap=none"])
        .arg(format!(
            "--lua-filter={}",
            root.join("scripts/readme.lua").display()
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("pandoc is required to verify the generated README");
    pandoc
        .stdin
        .take()
        .unwrap()
        .write_all(&exported.stdout)
        .unwrap();
    let markdown = pandoc.wait_with_output().unwrap();
    assert!(
        markdown.status.success(),
        "{}",
        String::from_utf8_lossy(&markdown.stderr)
    );

    let committed = std::fs::read(root.join("README.md")).unwrap();
    assert_eq!(
        markdown.stdout, committed,
        "README.md is stale; regenerate it with the command documented in AGENTS.md"
    );
}

#[test]
fn checks_a_workspace_with_configurable_severity_and_error_exit_status() {
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
        "See `->[missing|missing.plumb#id].\n",
    )
    .unwrap();
    let broken = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .args(["check", "--root"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(broken.status.success());
    assert!(broken.stderr.is_empty());
    let output = String::from_utf8(broken.stdout).unwrap();
    assert!(
        output.contains("nested/broken.plumb:1:")
            && output.contains("warning[link.unresolved-path]"),
        "{output}"
    );

    std::fs::write(
        root.join("tasks.plumb"),
        "`task Draft {\n  `@ draft\n}\n`task Review {\n  `@ review\n  `= depends #draft\n}\n",
    )
    .unwrap();
    let default = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .args(["check", "--root"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(default.status.success());
    assert!(!String::from_utf8_lossy(&default.stdout).contains("task.blocked"));

    let hints = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .args(["check", "--root"])
        .arg(&root)
        .args(["--level", "hint"])
        .output()
        .unwrap();
    assert!(hints.status.success());
    assert!(String::from_utf8_lossy(&hints.stdout).contains("hint[task.blocked]"));

    std::fs::write(root.join("syntax-error.plumb"), "See `broken[\n").unwrap();
    let errors = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .args(["check", "--root"])
        .arg(&root)
        .args(["--level", "error"])
        .output()
        .unwrap();
    assert!(!errors.status.success());
    let output = String::from_utf8(errors.stdout).unwrap();
    assert!(output.contains("error[syntax."), "{output}");
    assert!(!output.contains("warning["), "{output}");
    assert!(!output.contains("hint["), "{output}");

    let invalid_level = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .args(["check", "--level", "diagnostic"])
        .output()
        .unwrap();
    assert!(!invalid_level.status.success());
    assert!(String::from_utf8_lossy(&invalid_level.stderr).contains("invalid value 'diagnostic'"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn discovers_workspace_markers_and_applies_ignore_files() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(root.join(".plumb")).unwrap();
    std::fs::create_dir_all(root.join("nested")).unwrap();
    std::fs::create_dir_all(root.join("private")).unwrap();
    std::fs::write(root.join(".ignore"), "private/\n").unwrap();
    std::fs::write(root.join("visible.plumb"), "Visible\n").unwrap();
    std::fs::write(root.join("private/note.plumb"), "Private\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .arg("note")
        .current_dir(root.join("nested"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "visible.plumb\n");

    let explicit = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .args(["note", "--root", "."])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        explicit.status.success(),
        "{}",
        String::from_utf8_lossy(&explicit.stderr)
    );
    assert_eq!(
        String::from_utf8(explicit.stdout).unwrap(),
        "visible.plumb\n"
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn round_trips_the_exported_standard_profile_through_import() {
    let source = "{\n  `= title Import test\n}\n\n`# Intro {\n  `@ intro\n}\n\nParagraph with `*[emphasis], `![strong], `==[mark], `~[strike], `^[super], `_[sub], and `->[a link|other.plumb#id].\n\n`> Quoted {\n  `@ quote\n  `+ source\n}\n\n`task Item {\n  `@ task\n  `= created 2026-07-23T17:00:00+08:00\n}\n\n`rust\"\" {`@[code]}\n  fn main() {}\n";
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
fn serves_the_workspace_site_with_notes_and_tasks() {
    if Command::new("pandoc").arg("--version").output().is_err() {
        return;
    }
    let root = unique_temp_dir();
    std::fs::create_dir_all(root.join("assets")).unwrap();
    std::fs::create_dir_all(root.join("private")).unwrap();
    std::fs::write(root.join(".ignore"), "private/\n").unwrap();
    std::fs::write(root.join("assets/icon.png"), b"png").unwrap();
    std::fs::write(root.join("private/note.plumb"), "Private note.\n").unwrap();
    std::fs::write(
        root.join("a.plumb"),
        "{\n  `= title Alpha\n}\n\nSee `->[Beta|b.plumb#beta].\n\n`img[icon|=[src|assets/icon.png]]\n\n`task Ship release {\n  `= created 2026-07-25T10:00:00+08:00\n}\n",
    )
    .unwrap();
    std::fs::write(root.join("b.plumb"), "`# Beta {\n  `@ beta\n}\n").unwrap();
    std::fs::write(
        root.join("hidden.plumb"),
        "Hidden index. `->[Alpha|a.plumb].\n",
    )
    .unwrap();

    let port = available_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .args(["site", "serve", "--root"])
        .arg(&root)
        .arg("--port")
        .arg(port.to_string())
        .args(["--exclude", "path == 'hidden.plumb'"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output_reader = BufReader::new(child.stdout.take().unwrap());
    let mut error_reader = BufReader::new(child.stderr.take().unwrap());
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
    let revision = graph["revision"].as_u64().unwrap();
    std::fs::write(root.join("private/note.plumb"), "Changed private note.\n").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(400));
    let (_, _, unchanged) = http_get(address, "/api/graph");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&unchanged).unwrap()["revision"],
        revision
    );
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

    let (status, _, tasks) = http_get(address, "/api/tasks");
    assert_eq!(status, 200, "{tasks}");
    let tasks: serde_json::Value = serde_json::from_str(&tasks).unwrap();
    let task = &tasks["tasks"][0];
    assert_eq!(task["title"], "Ship release");
    assert_eq!(task["state"], "ready");
    assert_eq!(task["locator"]["kind"], "offset");
    assert!(task["revision"].is_string());
    let action_path = format!(
        "/api/task/{}/complete",
        task["documentId"].as_str().unwrap()
    );
    let (status, headers, body) = http_post_json(
        address,
        &action_path,
        &serde_json::json!({
            "revision": task["revision"],
            "locator": task["locator"],
        })
        .to_string(),
    );
    assert_eq!(status, 204, "{body}");
    assert!(body.is_empty());
    assert!(headers.contains("x-plumb-revision: 2"), "{headers}");
    let (status, _, updated_tasks) = http_get(address, "/api/tasks");
    assert_eq!(status, 200, "{updated_tasks}");
    let updated_tasks = serde_json::from_str::<serde_json::Value>(&updated_tasks).unwrap();
    assert_eq!(updated_tasks["tasks"][0]["state"], "done");
    assert!(updated_tasks["tasks"][0]["done"]
        .as_str()
        .is_some_and(|timestamp| timestamp.starts_with("2026-")));
    assert!(std::fs::read_to_string(root.join("a.plumb"))
        .unwrap()
        .contains("`= done 2026-"));

    std::fs::write(
        root.join("a.plumb"),
        "{\n  `= title Alpha updated\n}\n\nSee `->[Beta|b.plumb#beta].\n",
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
    let (status, headers, _) = http_get(address, "/");
    assert_eq!(status, 308);
    assert!(headers.contains("location: /graph"), "{headers}");
    let (status, headers, index) = http_get(address, "/graph");
    assert_eq!(status, 200, "{index}");
    assert!(headers.contains("content-security-policy"), "{headers}");
    assert!(headers.contains("cache-control: no-store"), "{headers}");
    assert!(index.contains("Workspace graph"));
    child.kill().unwrap();
    child.wait().unwrap();
    let mut server_log = String::new();
    error_reader.read_to_string(&mut server_log).unwrap();
    assert!(
        server_log.contains("task mutations enabled"),
        "{server_log}"
    );
    assert!(
        server_log.contains(&format!("POST {action_path} -> 204")),
        "{server_log}"
    );
    assert!(
        server_log.contains("task complete completed"),
        "{server_log}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn site_renders_and_refreshes_csl_json_citations() {
    if Command::new("pandoc").arg("--version").output().is_err() {
        return;
    }
    let root = unique_temp_dir();
    std::fs::create_dir_all(root.join("static")).unwrap();
    std::fs::write(
        root.join("note.plumb"),
        "{\n `= title Citation note\n `= bibliography static/library.json\n}\n\nSee `cite[smith2004].\n",
    )
    .unwrap();
    let bibliography = root.join("static/library.json");
    std::fs::write(
        &bibliography,
        r#"[{"id":"smith2004","type":"book","title":"First Edition","author":[{"family":"Smith"}],"issued":{"date-parts":[[2004]]}}]"#,
    )
    .unwrap();
    let port = available_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .args(["site", "serve", "--root"])
        .arg(&root)
        .arg("--port")
        .arg(port.to_string())
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
    let graph: serde_json::Value =
        serde_json::from_str(&http_get(address, "/api/graph").2).unwrap();
    let id = graph["nodes"][0]["id"].as_str().unwrap();
    let first = http_get(address, &format!("/api/note/{id}"));
    assert_eq!(first.0, 200, "{}", first.2);
    assert!(first.2.contains("Smith"), "{}", first.2);
    assert!(first.2.contains("First Edition"), "{}", first.2);
    assert!(first.2.contains("id=\\\"refs\\\""), "{}", first.2);

    std::fs::write(
        &bibliography,
        r#"[{"id":"smith2004","type":"book","title":"Revised Edition","author":[{"family":"Smith"}],"issued":{"date-parts":[[2005]]}}]"#,
    )
    .unwrap();
    let mut refreshed = false;
    for _ in 0..60 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let response = http_get(address, &format!("/api/note/{id}"));
        if response.0 == 200 && response.2.contains("Revised Edition") {
            refreshed = true;
            break;
        }
    }
    assert!(
        refreshed,
        "bibliography change did not refresh rendered citation"
    );
    child.kill().unwrap();
    child.wait().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn site_build_is_not_a_supported_subcommand() {
    let output = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .args(["site", "build"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand 'build'"));
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

fn http_post_json(address: &str, path: &str, body: &str) -> (u16, String, String) {
    let mut stream = TcpStream::connect(address).unwrap();
    write!(
        stream,
        "POST {path} HTTP/1.0\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
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

fn available_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
