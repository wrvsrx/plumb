use plumb_workspace::{DurationKind, DurationValue, SqliteSemanticStore, Workspace};
use std::path::Path;

const TARGET: &str = "`- 中文项\n `+ task\n `@ 中文项\n `= event-category daily\n";
const GOOD: &str = "`- 2026-10-03T09:00:00Z--10:00 `->{target.plumb#中文项}\n `+ event\n";
fn total(w: &Workspace) -> f64 {
    let result = w
        .document_durations(Path::new("/notes/target.plumb"))
        .unwrap();
    assert!(result.is_complete());
    let annotation = result
        .value
        .iter()
        .find(|a| a.kind == DurationKind::Task)
        .unwrap();
    let DurationValue::Seconds(seconds) = annotation.value else {
        panic!("{:?}", annotation.value)
    };
    seconds
}

#[test]
fn syntax_and_semantic_errors_are_diagnosed_without_poisoning_valid_totals() {
    for persistent in [false, true] {
        let mut w = if persistent {
            Workspace::with_sqlite_store(SqliteSemanticStore::open_in_memory().unwrap())
        } else {
            Workspace::new()
        };
        w.insert("/notes/target.plumb", 0, TARGET);
        let source = format!("{GOOD}`# Broken {{head\n `- 2026-10-03T10:00:00Z--11:00 Hidden\n  `+ event\n  `= tasks target.plumb#中文项\n`- bad-time Invalid\n `+ event\n `= tasks target.plumb#中文项\n`- 2026-10-03T11:00:00Z--12:00 {{}}\n `+ event\n `= tasks target.plumb#中文项\n");
        w.insert_disk("/notes/day.plumb", 1, &source).unwrap();
        assert_eq!(total(&w), 3600.0);
        let context = w.diagnostic_context().unwrap();
        let diagnostics = w
            .check_diagnostics_with_context(Path::new("/notes/day.plumb"), &context)
            .unwrap()
            .value;
        assert!(diagnostics
            .iter()
            .any(|d| d.code == "syntax.unclosed-inline-group"));
        assert!(diagnostics.iter().any(|d| d.code == "event.missing-title"));
        assert!(diagnostics
            .iter()
            .any(|d| d.code == "event.missing-date-context"));
        let old = w.clone();
        w.insert("/notes/day.plumb", 2, format!("{GOOD}{{unclosed\n"));
        assert_eq!(total(&w), 3600.0);
        w.insert("/notes/day.plumb", 3, "{broken\n");
        assert_eq!(total(&w), 0.0);
        assert_eq!(total(&old), 3600.0);
        if persistent {
            w.close_document("/notes/day.plumb");
            assert_eq!(total(&w), 3600.0);
        }
        w.remove_disk("/notes/day.plumb").unwrap();
        assert_eq!(total(&w), 0.0);
    }
}

#[test]
fn invalid_event_references_exclude_whole_event_and_recover_incrementally() {
    let mut w = Workspace::new();
    w.insert("/notes/target.plumb", 0, TARGET);
    w.insert("/notes/day.plumb", 0, format!("{GOOD}`- 2026-10-03T10:00:00Z--12:00 `->{{target.plumb#中文项}} `->{{missing.plumb#other}}\n `+ event\n"));
    assert_eq!(total(&w), 3600.0);
    assert_eq!(total(&w), 3600.0);
    assert_eq!(w.duration_work().events_recomputed, 0);
    for i in 0..50 {
        w.insert(format!("/notes/unrelated-{i}.plumb"), 0, "{broken\n");
    }
    assert_eq!(total(&w), 3600.0);
    assert_eq!(w.duration_work().events_recomputed, 0);
    w.insert("/notes/missing.plumb", 1, "`- Other\n `@ other\n");
    assert_eq!(total(&w), 7200.0);
    assert_eq!(w.duration_work().events_recomputed, 1);
    w.remove_disk("/notes/missing.plumb").unwrap();
    assert_eq!(total(&w), 3600.0);
    assert_eq!(w.duration_work().events_recomputed, 1);
    let mut cold = Workspace::new();
    for entry in w.documents() {
        cold.insert(&entry.path, entry.revision, entry.parsed.source());
    }
    assert_eq!(total(&w), total(&cold));
}

#[test]
fn pending_invalid_revision_is_partial_until_current_regions_are_installed() {
    let mut w = Workspace::new();
    w.insert("/notes/target.plumb", 0, TARGET);
    w.insert("/notes/day.plumb", 0, GOOD);
    assert_eq!(total(&w), 3600.0);
    let pending = w
        .begin_document_revision("/notes/day.plumb", 1, format!("{GOOD}{{broken\n"))
        .unwrap();
    assert!(!w
        .document_durations(Path::new("/notes/target.plumb"))
        .unwrap()
        .is_complete());
    assert!(w.install_document_analysis(pending.analyze()));
    assert_eq!(total(&w), 3600.0);
}

#[test]
#[ignore = "manual timing supplement to deterministic regional accounting work tests"]
fn benchmark_regional_accounting_warm_reads_and_local_updates() {
    use std::time::Instant;
    let mut workspace = Workspace::new();
    workspace.insert("/notes/target.plumb", 0, TARGET);
    workspace.insert("/notes/day.plumb", 0, GOOD);
    for i in 0..100 {
        workspace.insert(format!("/notes/unrelated-{i}.plumb"), 0, "{broken\n");
    }
    assert_eq!(total(&workspace), 3600.0);
    let started = Instant::now();
    for _ in 0..1000 {
        std::hint::black_box(total(&workspace));
    }
    eprintln!("1000 warm regional duration reads: {:?}", started.elapsed());
    assert_eq!(workspace.duration_work().events_recomputed, 0);
    let started = Instant::now();
    for revision in 1..=100 {
        workspace.insert(
            "/notes/day.plumb",
            revision,
            format!("{GOOD}{{broken-{revision}\n"),
        );
        assert_eq!(total(&workspace), 3600.0);
    }
    eprintln!(
        "100 invalid-region edits and duration reads: {:?}",
        started.elapsed()
    );
}
