use crate::support::{unique_temp_dir, LspTestSession};
use lsp_types::Url;
use serde_json::{json, Value};
use std::path::Path;

fn uri(path: impl AsRef<Path>) -> Url {
    Url::from_file_path(path).unwrap()
}
fn start(roots: &[&Path], args: &[&str]) -> LspTestSession {
    let mut s = LspTestSession::with_args(args);
    let folders = roots
        .iter()
        .map(|r| json!({"uri":uri(r), "name":"notes"}))
        .collect::<Vec<_>>();
    s.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"workspaceFolders":folders,"capabilities":{"workspace":{"didChangeWatchedFiles":{"dynamicRegistration":true}}}}}));
    let init = s.wait_for_response(&json!(1));
    assert!(init.get("error").is_none(), "{init}");
    s.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}));
    s.wait_for(|m| m["method"] == "$/progress" && m["params"]["value"]["kind"] == "end");
    let registration = s.wait_for(|m| m["method"] == "client/registerCapability");
    assert!(registration.to_string().contains("**/.plumb/config.toml"));
    s.send(&json!({"jsonrpc":"2.0","id":registration["id"],"result":null}));
    s
}
fn stop(mut s: LspTestSession) -> Vec<Value> {
    s.send(&json!({"jsonrpc":"2.0","id":999,"method":"shutdown","params":null}));
    s.wait_for_response(&json!(999));
    s.send(&json!({"jsonrpc":"2.0","method":"exit","params":null}));
    s.finish()
}
fn open(s: &mut LspTestSession, path: &Path, text: &str) {
    s.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri(path),"languageId":"plumb","version":1,"text":text}}}));
}
fn change(s: &mut LspTestSession, path: &Path, version: i32, text: &str) {
    s.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri(path),"version":version},"contentChanges":[{"text":text}]}}));
}
fn watched(s: &mut LspTestSession, path: &Path, typ: i32) {
    s.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":uri(path),"type":typ}]}}));
}
fn has(m: &Value, code: &str) -> bool {
    m["params"]["diagnostics"]
        .as_array()
        .is_some_and(|ds| ds.iter().any(|d| d["code"] == code))
}
fn publication(m: &Value, path: &Path) -> bool {
    m["method"] == "textDocument/publishDiagnostics" && m["params"]["uri"] == uri(path).as_str()
}
fn configure(root: &Path, text: &str) {
    std::fs::create_dir_all(root.join(".plumb")).unwrap();
    std::fs::write(root.join(".plumb/config.toml"), text).unwrap();
}

#[test]
fn policy_timeline_refreshes_other_files_and_projects_utf16_related_locations() {
    let root = unique_temp_dir();
    configure(
        &root,
        "[diagnostics.event-category]\nenabled=true\n[diagnostics.event-timeline]\nenabled=true",
    );
    let a = root.join("a.plumb");
    let b = root.join("b.plumb");
    let first = "`- 2026-09-22T10:00:00Z--11:00 中文😀\r\n `+ event\r\n";
    let second = "`- 2026-09-22T12:00:00Z--13:00 Next\n `+ event\n `= event-category work\n";
    std::fs::write(&a, first).unwrap();
    std::fs::write(&b, second).unwrap();
    let mut s = start(&[&root], &[]);
    open(&mut s, &a, first);
    let m = s.wait_for_next(|m| publication(m, &a) && has(m, "event-timeline.gap"));
    let ds = m["params"]["diagnostics"].as_array().unwrap();
    let gap = ds
        .iter()
        .find(|d| d["code"] == "event-timeline.gap")
        .unwrap();
    assert_eq!(gap["severity"], 2);
    assert_eq!(
        gap["relatedInformation"][0]["location"]["uri"],
        uri(&b).as_str()
    );
    assert_eq!(
        gap["range"]["end"]["character"].as_u64().unwrap()
            - gap["range"]["start"]["character"].as_u64().unwrap(),
        4
    );
    assert!(has(&m, "event-category.missing"));
    open(&mut s, &b, second);
    s.wait_for_next(|m| publication(m, &b) && has(m, "event-timeline.gap"));
    change(&mut s, &b, 2, &second.replace("12:00", "11:00"));
    s.wait_for_next(|m| {
        publication(m, &a) && has(m, "event-category.missing") && !has(m, "event-timeline.gap")
    });
    // Rapid changes must not let an older gap result replace the final continuous revision.
    change(&mut s, &b, 3, &second.replace("12:00", "11:30"));
    change(&mut s, &b, 4, &second.replace("12:00", "11:00"));
    // Publication iterates a HashMap: either document may arrive first.
    let mut saw_a = false;
    let mut saw_b = false;
    while !saw_a || !saw_b {
        let message = s.wait_for_next(|m| publication(m, &a) || publication(m, &b));
        saw_a |= publication(&message, &a)
            && has(&message, "event-category.missing")
            && !has(&message, "event-timeline.gap");
        saw_b |= publication(&message, &b) && message["params"]["version"] == 4;
    }
    // Closing restores the saved interval and the cross-file gap.
    s.send(&json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":uri(&b)}}}));
    s.wait_for_next(|m| publication(m, &a) && has(m, "event-timeline.gap"));
    let messages = stop(s);
    assert!(!messages.iter().any(|m| matches!(
        m["method"].as_str(),
        Some("window/showMessage" | "window/logMessage")
    ) && m["params"]["message"]
        .as_str()
        .is_some_and(|text| text.contains("diagnostics.incomplete"))));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn policy_categories_follow_unsaved_and_saved_inheritance_and_configuration_reload() {
    let root = unique_temp_dir();
    configure(&root, "[diagnostics.event-category]\nenabled=true");
    let event = root.join("event.plumb");
    let topic = root.join("topic.plumb");
    let text = "`- 2026-09-22T10:00:00Z `->{topic.plumb}\n `+ event\n";
    std::fs::write(&event, text).unwrap();
    std::fs::write(&topic, "Topic\n").unwrap();
    let mut s = start(&[&root], &[]);
    open(&mut s, &event, text);
    s.wait_for_next(|m| publication(m, &event) && has(m, "event-category.missing"));
    open(&mut s, &topic, "`= event-category work\n");
    s.wait_for_next(|m| publication(m, &event) && !has(m, "event-category.missing"));
    change(&mut s, &topic, 2, "Topic\n");
    s.wait_for_next(|m| publication(m, &event) && has(m, "event-category.missing"));
    let config = root.join(".plumb/config.toml");
    configure(&root, "[diagnostics.event-category]\nenabled=false");
    watched(&mut s, &config, 2);
    s.wait_for_next(|m| publication(m, &event) && !has(m, "event-category.missing"));
    configure(&root, "[diagnostics.event-category]\nenabled=true");
    s.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeConfiguration","params":{"settings":{}}}));
    s.wait_for_next(|m| publication(m, &event) && has(m, "event-category.missing"));
    configure(&root, "[diagnostics.event-category]\nenabled='invalid'");
    watched(&mut s, &config, 2);
    let error =
        s.wait_for_next(|m| m["method"] == "window/showMessage" && m["params"]["type"] == 1);
    assert!(error["params"]["message"]
        .as_str()
        .unwrap()
        .contains("diagnostics.configuration"));
    s.wait_for_next(|m| publication(m, &event) && !has(m, "event-category.missing"));
    configure(&root, "[diagnostics.event-category]\nenabled=true");
    watched(&mut s, &config, 2);
    s.wait_for_next(|m| publication(m, &event) && has(m, "event-category.missing"));
    s.send(&json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":uri(&topic)}}}));
    std::fs::write(&topic, "`= event-category work\n").unwrap();
    watched(&mut s, &topic, 2);
    s.wait_for_next(|m| publication(m, &event) && !has(m, "event-category.missing"));
    std::fs::write(&topic, "Topic\n").unwrap();
    watched(&mut s, &topic, 2);
    s.wait_for_next(|m| publication(m, &event) && has(m, "event-category.missing"));
    std::fs::remove_file(&config).unwrap();
    watched(&mut s, &config, 3);
    s.wait_for_next(|m| publication(m, &event) && !has(m, "event-category.missing"));
    let messages = stop(s);
    assert!(!messages.iter().any(|m| matches!(
        m["method"].as_str(),
        Some("window/showMessage" | "window/logMessage")
    ) && m["params"]["message"]
        .as_str()
        .is_some_and(|text| text.contains("diagnostics.incomplete"))));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn policy_cli_override_survives_reload_and_invalid_source_defers_silently() {
    let root = unique_temp_dir();
    configure(&root, "");
    let event = root.join("event.plumb");
    let text = "`- 2026-09-22T10:00:00Z Point\n `+ event\n";
    let mut s = start(
        &[&root],
        &["--config", "diagnostics.event-category.enabled=true"],
    );
    open(&mut s, &event, text);
    s.wait_for_next(|m| publication(m, &event) && has(m, "event-category.missing"));
    configure(&root, "[diagnostics.event-category]\nenabled=false");
    watched(&mut s, &root.join(".plumb/config.toml"), 2);
    s.wait_for_next(|m| publication(m, &event) && has(m, "event-category.missing"));
    change(&mut s, &event, 2, "`broken{");
    let syntax = s.wait_for_next(|m| {
        publication(m, &event)
            && has(m, "syntax.unclosed-inline-group")
            && !has(m, "event-category.missing")
    });
    assert_eq!(
        syntax["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["code"] == "syntax.unclosed-inline-group")
            .unwrap()["severity"],
        1
    );
    change(&mut s, &event, 3, text);
    s.wait_for_next(|m| {
        publication(m, &event) && m["params"]["version"] == 3 && has(m, "event-category.missing")
    });
    let messages = stop(s);
    assert!(!messages.iter().any(|m| matches!(
        m["method"].as_str(),
        Some("window/showMessage" | "window/logMessage")
    ) && m["params"]["message"]
        .as_str()
        .is_some_and(|text| text.contains("diagnostics.incomplete"))));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn policy_multi_root_settings_and_timeline_scopes_are_independent() {
    let root = unique_temp_dir();
    let nested = root.join("nested");
    configure(&root, "[diagnostics.event-timeline]\nenabled=true");
    configure(&nested, "[diagnostics.event-category]\nenabled=true");
    let a = root.join("a.plumb");
    let b = nested.join("b.plumb");
    let first = "`- 2026-09-22T10:00:00Z--11:00 First\n `+ event\n";
    let second = "`- 2026-09-22T12:00:00Z--13:00 Next\n `+ event\n";
    std::fs::write(&a, first).unwrap();
    std::fs::write(&b, second).unwrap();
    let mut s = start(&[&root, &nested], &[]);
    open(&mut s, &a, first);
    open(&mut s, &b, second);
    s.wait_for_next(|m| publication(m, &b) && has(m, "event-category.missing"));
    let messages = stop(s);
    assert!(messages.iter().filter(|m| publication(m, &a)).all(|m| !has(
        m,
        "event-category.missing"
    ) && !has(
        m,
        "event-timeline.gap"
    )));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn policy_invalid_startup_configuration_rejects_initialize() {
    let root = unique_temp_dir();
    configure(&root, "[check.event-category]\nenabled=true");
    let mut s = LspTestSession::new();
    s.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"rootUri":uri(&root),"capabilities":{}}}));
    let result = s.wait_for_response(&json!(1));
    assert_eq!(result["error"]["code"], -32602);
    assert!(result["error"]["message"]
        .as_str()
        .unwrap()
        .contains("unknown field"));
    // A failed initialize leaves the lifecycle uninitialized; exit directly.
    s.send(&json!({"jsonrpc":"2.0","method":"exit","params":null}));
    s.finish();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn policy_event_categories_follow_document_and_ancestor_edits() {
    let root = unique_temp_dir();
    configure(&root, "[diagnostics.event-category]\nenabled=true");
    let path = root.join("day.plumb");
    let bare = "`- 2026-09-22T10:00:00Z Work\n `+ event\n";
    std::fs::write(&path, bare).unwrap();
    let mut s = start(&[&root], &[]);
    open(&mut s, &path, bare);
    s.wait_for_next(|m| publication(m, &path) && has(m, "event-category.missing"));
    let sources = [
        format!("`= event-category work\n{bare}"),
        format!(
            "`= event-category work\n{}",
            bare.replace(" `+ event", " `+ event\n `= event-category")
        ),
        "`# Section\n `= event-category work\n `- 2026-09-22T10:00:00Z Work\n  `+ event\n"
            .to_owned(),
        format!("`# Sibling\n `= event-category work\n{bare}"),
    ];
    for (index, source) in sources.iter().enumerate() {
        let version = index as i32 + 2;
        change(&mut s, &path, version, source);
        s.wait_for_next(|m| {
            publication(m, &path)
                && m["params"]["version"] == version
                && match index {
                    1 => has(m, "agenda.invalid-category"),
                    3 => has(m, "event-category.missing"),
                    _ => !has(m, "event-category.missing") && !has(m, "agenda.invalid-category"),
                }
        });
    }
    let messages = stop(s);
    assert!(!messages.iter().any(|m| matches!(
        m["method"].as_str(),
        Some("window/showMessage" | "window/logMessage")
    ) && m["params"]["message"]
        .as_str()
        .is_some_and(|text| text.contains("diagnostics.incomplete"))));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn policy_startup_waits_without_warning_and_survives_unavailable_cache() {
    for unavailable_cache in [false, true] {
        let root = unique_temp_dir();
        configure(&root, "[diagnostics.event-category]\nenabled=true");
        let path = root.join("day.plumb");
        let source = "`- 2026-09-22T10:00:00Z Work\n `+ event\n";
        std::fs::write(&path, source).unwrap();
        let mut s = LspTestSession::new();
        if unavailable_cache {
            // A directory at the database path reliably rejects SQLite open on all hosts.
            let database = plumb_workspace::workspace_cache_path(
                s.cache_dir(),
                env!("CARGO_PKG_VERSION"),
                &[root.clone()],
            );
            std::fs::create_dir_all(database).unwrap();
        }
        s.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"rootUri":uri(&root),"capabilities":{}}}));
        assert!(s.wait_for_response(&json!(1)).get("error").is_none());
        s.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}));
        open(&mut s, &path, source);
        let publication =
            s.wait_for_next(|m| publication(m, &path) && has(m, "event-category.missing"));
        let diagnostic = publication["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["code"] == "event-category.missing")
            .unwrap();
        assert_eq!(diagnostic["severity"], 2);
        let messages = stop(s);
        assert!(
            !messages.iter().any(|m| m["method"] == "window/showMessage"
                && m["params"]["message"]
                    .as_str()
                    .is_some_and(|text| text.contains("diagnostics.incomplete"))),
            "{messages:?}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn policy_failed_index_keeps_local_diagnostics_and_recovers_after_a_watched_file_is_fixed() {
    let root = unique_temp_dir();
    configure(&root, "[diagnostics.event-category]\nenabled=true");
    let bad = root.join("bad.plumb");
    std::fs::write(&bad, [0xff]).unwrap();
    let path = root.join("day.plumb");
    let source = "`= title One\n`= title Two\n\n`- 2026-09-22T10:00:00Z Work\n `+ event\n";
    std::fs::write(&path, source).unwrap();
    let mut s = start(&[&root], &[]);
    open(&mut s, &path, source);
    let local = s.wait_for_next(|m| publication(m, &path) && has(m, "metadata.duplicate-key"));
    assert!(!has(&local, "event-category.missing"));
    std::fs::write(&bad, "Fixed\n").unwrap();
    watched(&mut s, &bad, 2);
    let full = s.wait_for_next(|m| publication(m, &path) && has(m, "event-category.missing"));
    assert!(has(&full, "metadata.duplicate-key"));
    let messages = stop(s);
    assert!(!messages.iter().any(|m| matches!(
        m["method"].as_str(),
        Some("window/showMessage" | "window/logMessage")
    ) && m["params"]["message"]
        .as_str()
        .is_some_and(|text| text.contains("diagnostics.incomplete"))));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn policy_initial_index_publishes_local_then_merged_diagnostics_without_false_references() {
    let root = unique_temp_dir();
    configure(&root, "[diagnostics.event-category]\nenabled=true");
    std::fs::write(root.join("large.plumb"), "Paragraph.\n\n".repeat(150_000)).unwrap();
    std::fs::write(root.join("target.plumb"), "Target\n").unwrap();
    let path = root.join("day.plumb");
    let source = "`= title One\n`= title Two\n\n`->{target.plumb}\n\n`- 2026-09-22T10:00:00Z Work\n `+ event\n";
    let mut s = LspTestSession::new();
    s.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"rootUri":uri(&root),"capabilities":{}}}));
    assert!(s.wait_for_response(&json!(1)).get("error").is_none());
    s.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}));
    open(&mut s, &path, source);
    let local = s.wait_for_next(|m| publication(m, &path) && has(m, "metadata.duplicate-key"));
    assert!(!has(&local, "event-category.missing"));
    let full = s.wait_for_next(|m| publication(m, &path) && has(m, "event-category.missing"));
    assert!(has(&full, "metadata.duplicate-key"));
    let messages = stop(s);
    let local_position = messages.iter().position(|m| m == &local).unwrap();
    let index_end = messages
        .iter()
        .position(|m| m["method"] == "$/progress" && m["params"]["value"]["kind"] == "end")
        .unwrap();
    assert!(local_position < index_end);
    assert!(!messages
        .iter()
        .any(|m| publication(m, &path) && has(m, "link.unresolved-path")));
    assert!(!messages.iter().any(|m| m["params"]["message"]
        .as_str()
        .is_some_and(|s| s.contains("diagnostics.incomplete"))));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn policy_incomplete_timeline_waits_for_syntax_repair_without_publishing_false_gaps() {
    let root = unique_temp_dir();
    configure(&root, "[diagnostics.event-timeline]\nenabled=true");
    let a = root.join("a.plumb");
    let b = root.join("b.plumb");
    let c = root.join("c.plumb");
    let first = "`- 2026-09-22T10:00:00Z--11:00 First\n `+ event\n";
    std::fs::write(&a, first).unwrap();
    std::fs::write(&b, "`broken{").unwrap();
    std::fs::write(&c, "`- 2026-09-22T12:00:00Z--13:00 Last\n `+ event\n").unwrap();
    let mut s = start(&[&root], &[]);
    open(&mut s, &a, first);
    open(&mut s, &b, "`broken{");
    s.wait_for_next(|m| publication(m, &b) && has(m, "syntax.unclosed-inline-group"));
    let incomplete = s.wait_for_next(|m| publication(m, &a));
    assert!(!has(&incomplete, "event-timeline.gap"));
    // Fixing to an unrelated document establishes a real gap on the next full round.
    change(&mut s, &b, 2, "Fixed\n");
    s.wait_for_next(|m| publication(m, &a) && has(m, "event-timeline.gap"));
    let messages = stop(s);
    let incomplete_position = messages.iter().position(|m| m == &incomplete).unwrap();
    assert!(!messages[..=incomplete_position]
        .iter()
        .any(|m| has(m, "event-timeline.gap")));
    assert!(!messages.iter().any(|m| m["params"]["message"]
        .as_str()
        .is_some_and(|s| s.contains("diagnostics.incomplete"))));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn policy_description_edits_publish_complete_revisions_without_clearing_warnings() {
    let root = unique_temp_dir();
    configure(
        &root,
        "[diagnostics.event-category]\nenabled=true\n[diagnostics.event-timeline]\nenabled=true",
    );
    let path = root.join("day.plumb");
    let source = "`= title One\n`= title Two\n`- 2026-09-22T10:00:00Z--11:00 中文😀\n `+ event\n`- 2026-09-22T12:00:00Z--13:00 Next\n `+ event\n";
    std::fs::write(&path, source).unwrap();
    let mut s = start(&[&root], &[]);
    open(&mut s, &path, source);
    s.wait_for_next(|m| publication(m, &path) && has(m, "event-timeline.gap"));
    for version in 2..=5 {
        change(
            &mut s,
            &path,
            version,
            &source.replace("中文😀", &format!("描述😀 {version}")),
        );
        s.wait_for_next(|m| publication(m, &path) && m["params"]["version"] == version);
    }
    // Superseded jobs must not publish an incomplete final revision either.
    change(&mut s, &path, 6, &source.replace("Next", "Interim"));
    change(&mut s, &path, 7, &source.replace("Next", "Final"));
    s.wait_for_next(|m| publication(m, &path) && m["params"]["version"] == 7);
    let messages = stop(s);
    let mut last_version = 0;
    for message in messages.iter().filter(|m| publication(m, &path)) {
        let version = message["params"]["version"].as_i64().unwrap();
        assert!(version >= last_version, "{message}");
        last_version = version;
        for code in [
            "metadata.duplicate-key",
            "event-category.missing",
            "event-timeline.gap",
        ] {
            assert!(
                has(message, code),
                "revision {version} lost {code}: {message}"
            );
        }
    }
    assert_eq!(last_version, 7);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn ongoing_event_warning_is_clock_independent_and_clears_on_closure() {
    for timeline in [false, true] {
        let root = unique_temp_dir();
        configure(
            &root,
            &format!("[diagnostics.event-timeline]\nenabled={timeline}\n"),
        );
        let path = root.join("day.plumb");
        let source = "`- 2999-01-01T08:00:00Z-- 工作😀\r\n `+ event\r\n`- 2999-01-01T10:00:00Z--11:00 Plan\r\n `+ event\r\n";
        std::fs::write(&path, source).unwrap();
        let mut session = start(&[&root], &[]);
        open(&mut session, &path, source);
        let published =
            session.wait_for_next(|m| publication(m, &path) && m["params"]["version"] == 1);
        let diagnostics = published["params"]["diagnostics"].as_array().unwrap();
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let warning = &diagnostics[0];
        assert_eq!(warning["code"], "event.ongoing");
        assert_eq!(warning["severity"], 2);
        assert_eq!(
            warning["message"],
            "event has no end time; excluded from accounting and timeline checks"
        );
        let title_start = source.find("工作").unwrap();
        assert_eq!(
            warning["range"],
            json!({
                "start":{"line":0,"character":title_start},
                "end":{"line":0,"character":title_start + 4}
            })
        );
        change(
            &mut session,
            &path,
            2,
            &source.replace("-- 工作", "--10:00 工作"),
        );
        let closed =
            session.wait_for_next(|m| publication(m, &path) && m["params"]["version"] == 2);
        assert_eq!(closed["params"]["diagnostics"], json!([]));
        change(&mut session, &path, 3, source);
        let reopened =
            session.wait_for_next(|m| publication(m, &path) && m["params"]["version"] == 3);
        assert_eq!(
            reopened["params"]["diagnostics"],
            published["params"]["diagnostics"]
        );
        stop(session);
        std::fs::remove_dir_all(root).unwrap();
    }
}
