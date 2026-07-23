use std::io::Write;
use std::process::{Command, Output, Stdio};

#[test]
fn exposes_the_unified_command_surface() {
    let help = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    for command in ["fmt", "export", "import", "note", "task", "lsp"] {
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
