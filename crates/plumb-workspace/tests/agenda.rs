use chrono::{DateTime, FixedOffset};
use plumb_workspace::{SqliteSemanticStore, Workspace};
use std::path::Path;

fn dt(s: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(s).unwrap()
}
const START: &str = "2026-09-22T10:00:00Z";
const END: &str = "2026-09-22T11:00:00Z";
const ITEMS: &str = "`- A\n `+ task\n `@ a\n `= event-category work\n`- B\n `+ task\n `@ b\n `= event-category work\n`- C\n `@ c\n `= event-category learn\n";
const EVENT: &str = "`- 2026-09-22T10:00:00Z--11:00 `->{items.plumb#a} `->{items.plumb#b} `->{items.plumb#c} `->{items.plumb#a}\n `+ event\n\n See `->{items.plumb#missing}\n";
fn report(w: &Workspace, accounting: bool) -> plumb_workspace::AgendaReport {
    w.agenda_report(
        Path::new("/notes"),
        dt(START),
        dt(END),
        dt(START),
        None,
        accounting,
    )
    .unwrap()
}
#[test]
fn deduplicates_then_splits_by_item_before_category_and_task_aggregation() {
    let mut w = Workspace::new();
    w.insert("/notes/items.plumb", 0, ITEMS);
    w.insert("/notes/day.plumb", 0, EVENT);
    let r = report(&w, true);
    assert!(r.complete, "{:?}", r.issues);
    assert!(r.timeline_passed());
    assert_eq!(r.accumulated_seconds, 3600.0);
    assert_eq!(
        r.categories
            .iter()
            .map(|c| (c.category.as_deref(), c.seconds))
            .collect::<Vec<_>>(),
        [(Some("learn"), 1200.0), (Some("work"), 2400.0)]
    );
    assert_eq!(r.items.len(), 3);
    assert_eq!(r.tasks.len(), 2);
    assert!(r.items.iter().all(|i| i.seconds == 1200.0));
}

#[test]
fn referenced_list_item_inherits_document_category() {
    let mut w = Workspace::new();
    w.insert(
        "/notes/items.plumb",
        0,
        "`= event-category work\n`- A\n `@ a\n",
    );
    w.insert(
        "/notes/day.plumb",
        0,
        "`- 2026-09-22T10:00:00Z--11:00 `->{items.plumb#a}\n `+ event\n",
    );
    let r = report(&w, true);
    assert!(r.complete, "{:?}", r.issues);
    assert_eq!(r.categories[0].category.as_deref(), Some("work"));
}
#[test]
fn unresolved_references_keep_their_share_and_timeline_is_independent() {
    let mut w = Workspace::new();
    w.insert("/notes/items.plumb", 0, ITEMS);
    w.insert(
        "/notes/day.plumb",
        0,
        EVENT.replace("items.plumb#c", "items.plumb#missing"),
    );
    let r = report(&w, true);
    assert!(!r.complete);
    assert_eq!(
        r.categories
            .iter()
            .find(|c| c.category.is_none())
            .unwrap()
            .seconds,
        1200.0
    );
    assert!(report(&w, false).timeline_passed());
}
#[test]
fn explicit_category_overrides_and_empty_tasks_suppresses_inference() {
    let mut w = Workspace::new();
    w.insert("/notes/items.plumb", 0, ITEMS);
    w.insert(
        "/notes/day.plumb",
        0,
        EVENT.replace(" `+ event", " `+ event\n `= event-category personal"),
    );
    let r = report(&w, true);
    assert!(r.complete);
    assert_eq!(r.categories.len(), 1);
    assert_eq!(r.categories[0].category.as_deref(), Some("personal"));
    assert_eq!(r.tasks.len(), 2);
    w.insert(
        "/notes/day.plumb",
        1,
        EVENT.replace(" `+ event", " `+ event\n `= tasks"),
    );
    let r = report(&w, true);
    assert!(r.items.is_empty());
    // Explicit empty declaration must not fall back to title links.
    assert!(r.categories[0].category.is_none());
}
#[test]
fn interval_sweep_clips_boundaries_and_handles_nested_overlap_without_false_gaps() {
    let mut w = Workspace::new();
    w.insert("/notes/day.plumb", 0, "`= date 2026-09-22\n`= timezone +00:00\n`- 09:00--10:50 Outer\n `+ event\n`- 10:10--10:20 Inner\n `+ event\n`- 10:30--10:40 Another\n `+ event\n`- 10:50--12:00 Tail\n `+ event\n`- 10:25 Point\n `+ event\n");
    let r = report(&w, false);
    assert!(r.complete);
    assert!(r.gaps.is_empty());
    assert_eq!(r.overlaps.len(), 2);
    assert_eq!(r.covered_seconds, 3600.0);
    assert_eq!(r.accumulated_seconds, 4800.0);
    assert_eq!(r.points.len(), 1);
    assert!(r.overlaps.iter().all(|s| s.events.len() == 2));
}
#[test]
fn empty_window_and_boundary_gaps_and_invalid_time_cannot_pass() {
    let mut w = Workspace::new();
    let r = report(&w, false);
    assert_eq!(r.gaps.len(), 1);
    assert!(!r.timeline_passed());
    w.insert(
        "/notes/day.plumb",
        0,
        "`- 2026-09-22T10:10:00Z--10:50 Some\n `+ event\n",
    );
    assert_eq!(report(&w, false).gaps.len(), 2);
    w.insert("/notes/bad.plumb", 0, "`- tomorrow Bad\n `+ event\n");
    assert!(!report(&w, false).complete);
    assert!(w
        .agenda_report(
            Path::new("/notes"),
            dt(END),
            dt(START),
            dt(START),
            None,
            false
        )
        .is_err());
}
#[test]
fn persistent_and_open_overlay_recompute_category_without_stale_inheritance() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteSemanticStore::open(temp.path().join("index.sqlite")).unwrap();
    let mut disk = Workspace::with_sqlite_store(store);
    disk.insert_disk("/notes/items.plumb", 0, ITEMS).unwrap();
    disk.insert_disk("/notes/day.plumb", 0, EVENT).unwrap();
    let mut memory = Workspace::new();
    memory.insert("/notes/items.plumb", 0, ITEMS);
    memory.insert("/notes/day.plumb", 0, EVENT);
    assert_eq!(
        serde_json::to_value(report(&disk, true)).unwrap(),
        serde_json::to_value(report(&memory, true)).unwrap()
    );
    disk.open_document(
        "/notes/items.plumb",
        1,
        ITEMS.replace("category learn", "category work"),
    );
    let r = report(&disk, true);
    assert_eq!(r.categories.len(), 1);
    assert_eq!(r.categories[0].seconds, 3600.0);
    disk.close_document("/notes/items.plumb");
    assert_eq!(report(&disk, true).categories.len(), 2);
    disk.open_document("/notes/items.plumb", 2, "`broken{");
    assert!(!report(&disk, true).complete);
}
#[test]
fn document_tasks_supply_categories_and_filter_is_recorded() {
    let mut w = Workspace::new();
    w.insert(
        "/notes/project.plumb",
        0,
        "`+ task\n`= title Project\n`= event-category work\n",
    );
    w.insert(
        "/notes/day.plumb",
        0,
        "`- 2026-09-22T10:00:00Z--11:00 Work\n `+ event\n `= tasks project.plumb\n",
    );
    let r = report(&w, true);
    assert!(r.complete, "{:?}", r.issues);
    assert_eq!(r.tasks.len(), 1);
    assert_eq!(r.categories[0].category.as_deref(), Some("work"));
    let r = w
        .agenda_report(
            Path::new("/notes"),
            dt(START),
            dt(END),
            dt(START),
            Some("path == 'other.plumb'"),
            false,
        )
        .unwrap();
    assert_eq!(r.filter.as_deref(), Some("path == 'other.plumb'"));
    assert_eq!(r.gaps.len(), 1);
}

#[test]
fn category_lists_split_each_item_share_without_inflating_totals() {
    let mut w = Workspace::new();
    w.insert(
        "/notes/items.plumb",
        0,
        ITEMS.replace(
            " `= event-category learn",
            " `= event-category\n  `- learn\n  `- phd misc\n  `- learn",
        ),
    );
    w.insert("/notes/day.plumb", 0, EVENT);
    let r = report(&w, true);
    assert!(r.complete);
    assert_eq!(r.categories.iter().map(|c| c.seconds).sum::<f64>(), 3600.0);
    assert_eq!(
        r.categories
            .iter()
            .map(|c| (c.category.as_deref(), c.seconds))
            .collect::<Vec<_>>(),
        [
            (Some("learn"), 600.0),
            (Some("phd misc"), 600.0),
            (Some("work"), 2400.0)
        ]
    );
    assert!(r.items.iter().all(|i| i.seconds == 1200.0));
    w.insert(
        "/notes/day.plumb",
        1,
        EVENT.replace(
            " `+ event",
            " `+ event\n `= event-category\n  `- personal\n  `- work",
        ),
    );
    let r = report(&w, true);
    assert_eq!(
        r.categories.iter().map(|c| c.seconds).collect::<Vec<_>>(),
        [1800.0, 1800.0]
    );
}

#[test]
fn category_check_covers_all_dates_points_and_requires_resolvable_inheritance() {
    let mut w = Workspace::new();
    w.insert("/notes/items.plumb", 0, ITEMS);
    w.insert("/notes/day.plumb", 0, EVENT);
    let check = |w: &Workspace| {
        w.check_event_categories(Path::new("/notes"), dt(START), None)
            .unwrap()
    };
    assert!(check(&w).missing.is_empty());
    w.insert(
        "/notes/old.plumb",
        0,
        "`- 2020-01-01T10:00:00Z Point\n `+ event\n",
    );
    assert_eq!(check(&w).checked, 2);
    assert_eq!(check(&w).missing.len(), 1);
    w.insert(
        "/notes/day.plumb",
        1,
        EVENT
            .replace("items.plumb#a", "missing.plumb")
            .replace(" `+ event", " `+ event\n `= event-category phd misc"),
    );
    assert!(!check(&w).complete);
    w.insert(
        "/notes/old.plumb",
        1,
        "`- 2020-01-01T10:00:00Z Point\n `+ event\n `= event-category\n",
    );
    assert!(!check(&w).complete);
}

#[test]
fn category_check_and_multivalue_accounting_match_persistent_queries() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteSemanticStore::open(temp.path().join("index.sqlite")).unwrap();
    let mut disk = Workspace::with_sqlite_store(store);
    let mut memory = Workspace::new();
    let items = ITEMS.replace("category learn", "category\n  `- phd misc\n  `- learn");
    for (path, text) in [
        ("/notes/items.plumb", items.as_str()),
        ("/notes/day.plumb", EVENT),
    ] {
        disk.insert_disk(path, 0, text).unwrap();
        memory.insert(path, 0, text);
    }
    assert_eq!(
        serde_json::to_value(report(&disk, true)).unwrap(),
        serde_json::to_value(report(&memory, true)).unwrap()
    );
    let check = |w: &Workspace| {
        w.check_event_categories(Path::new("/notes"), dt(START), None)
            .unwrap()
    };
    assert_eq!(
        serde_json::to_value(check(&disk)).unwrap(),
        serde_json::to_value(check(&memory)).unwrap()
    );
    let check = disk
        .check_event_categories(
            Path::new("/notes"),
            dt(START),
            Some("path == 'other.plumb'"),
        )
        .unwrap();
    assert_eq!(check.checked, 0);
    assert!(check.complete);
}

#[test]
fn ordinary_document_event_categories_are_persistent_and_not_task_shares() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteSemanticStore::open(temp.path().join("index.sqlite")).unwrap();
    let mut disk = Workspace::with_sqlite_store(store);
    let source = "`= title Dinner\n`= event-category\n `- girlfriend\n `- daily\n\nDetails.\n";
    let event = "`- 2026-09-22T10:00:00Z--11:00 `->\"dinner.plumb\"\n `+ event\n";
    disk.insert_disk("/notes/dinner.plumb", 0, source).unwrap();
    disk.insert_disk("/notes/day.plumb", 0, event).unwrap();
    let mut memory = Workspace::new();
    memory.insert("/notes/dinner.plumb", 0, source);
    memory.insert("/notes/day.plumb", 0, event);
    let r = report(&disk, true);
    assert!(r.complete, "{:?}", r.issues);
    assert!(r.tasks.is_empty());
    assert_eq!(r.categories.len(), 2);
    assert!(r.categories.iter().all(|c| c.seconds == 1800.0));
    assert_eq!(
        serde_json::to_value(&r).unwrap(),
        serde_json::to_value(report(&memory, true)).unwrap()
    );
    disk.open_document("/notes/dinner.plumb", 1, "`= event-category work\n");
    assert_eq!(
        report(&disk, true).categories[0].category.as_deref(),
        Some("work")
    );
    disk.close_document("/notes/dinner.plumb");
    assert_eq!(report(&disk, true).categories.len(), 2);
    disk.insert_disk("/notes/dinner.plumb", 2, "`= category topic\n")
        .unwrap();
    let r = report(&disk, true);
    assert!(r.complete);
    assert!(r.categories[0].category.is_none());
    disk.insert_disk("/notes/dinner.plumb", 3, "`= event-category\n")
        .unwrap();
    assert!(!report(&disk, true).complete);
    disk.insert_disk(
        "/notes/day.plumb",
        1,
        "`- 2026-09-22T10:00:00Z--11:00 Dinner\n `+ event\n `= tasks dinner.plumb\n",
    )
    .unwrap();
    assert!(
        !report(&disk, true).complete,
        "explicit tasks still requires a task"
    );
}

fn continuity(w: &Workspace) -> plumb_workspace::TimelineCheckReport {
    w.check_event_timeline(Path::new("/notes"), dt(START))
        .unwrap()
}

#[test]
fn continuity_has_no_external_boundaries_and_points_do_not_extend_coverage() {
    let mut w = Workspace::new();
    assert!(continuity(&w).passed());
    w.insert(
        "/notes/points.plumb",
        0,
        "`- 2020-01-01T00:00:00Z Before\n `+ event\n`- 2030-01-01T00:00:00Z After\n `+ event\n",
    );
    assert!(continuity(&w).passed());
    w.insert(
        "/notes/a.plumb",
        0,
        "`- 2026-09-22T10:00:00Z--11:00 One\n `+ event\n",
    );
    assert!(continuity(&w).passed());
    // Adjacent instants with different offsets join across documents.
    w.insert(
        "/notes/b.plumb",
        0,
        "`- 2026-09-22T19:00:00+08:00--20:00 Two\n `+ event\n",
    );
    assert!(continuity(&w).passed());
    w.insert(
        "/notes/c.plumb",
        0,
        "`- 2026-09-23T10:00:00Z--11:00 Next\n `+ event\n",
    );
    let r = continuity(&w);
    assert!(r.complete);
    assert!(!r.passed());
    assert_eq!(r.gaps.len(), 1);
    assert_eq!(r.gaps[0].start, dt("2026-09-22T12:00:00Z"));
    assert_eq!(r.gaps[0].end, dt("2026-09-23T10:00:00Z"));
    assert_eq!(
        r.gaps[0]
            .events
            .iter()
            .map(|e| e.path.as_path())
            .collect::<Vec<_>>(),
        [Path::new("/notes/b.plumb"), Path::new("/notes/c.plumb")]
    );
}

#[test]
fn continuity_sweep_handles_nested_and_duplicate_intervals_without_false_gaps() {
    let mut w = Workspace::new();
    w.insert("/notes/day.plumb", 0, "`= date 2026-09-22\n`= timezone +00:00\n`- 09:00--12:00 Outer\n `+ event\n`- 10:00--11:00 Inner\n `+ event\n`- 10:00--11:00 Duplicate\n `+ event\n`- 12:00--13:00 Tail\n `+ event\n");
    let r = continuity(&w);
    assert!(r.complete);
    assert!(r.gaps.is_empty());
    assert_eq!(r.overlaps.len(), 1);
    assert_eq!(r.overlaps[0].events.len(), 3);
    assert_eq!(r.overlaps[0].start, dt(START));
    assert_eq!(r.overlaps[0].end, dt(END));
}

#[test]
fn continuity_is_independent_of_references_but_invalid_and_open_times_are_incomplete() {
    let mut w = Workspace::new();
    w.insert("/notes/day.plumb", 0, EVENT);
    assert!(continuity(&w).passed());
    for source in [
        "`- tomorrow Invalid\n `+ event\n",
        "`- 2026-09-22T10:00:00Z-- Open\n `+ event\n",
        "`- 2026-09-22T10:00:00Z--2026-09-22T09:00:00Z Reversed\n `+ event\n",
        "`broken{",
    ] {
        w.insert("/notes/bad.plumb", 1, source);
        let r = continuity(&w);
        assert!(!r.complete, "{source}");
        assert!(!r.passed());
        assert!(!r.issues.is_empty());
    }
}

#[test]
fn continuity_matches_persistent_and_overlay_revisions() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteSemanticStore::open(temp.path().join("index.sqlite")).unwrap();
    let mut disk = Workspace::with_sqlite_store(store);
    let mut memory = Workspace::new();
    let source = "`= date 2026-09-22\n`= timezone +00:00\n`- 09:00--10:00 First\n `+ event\n`- 11:00--12:00 Second\n `+ event\n";
    disk.insert_disk("/notes/day.plumb", 0, source).unwrap();
    memory.insert("/notes/day.plumb", 0, source);
    assert_eq!(
        serde_json::to_value(continuity(&disk)).unwrap(),
        serde_json::to_value(continuity(&memory)).unwrap()
    );
    disk.open_document("/notes/day.plumb", 1, source.replace("11:00--", "10:00--"));
    assert!(continuity(&disk).passed());
    disk.close_document("/notes/day.plumb");
    assert_eq!(continuity(&disk).gaps.len(), 1);
    disk.open_document("/notes/day.plumb", 2, "`broken{");
    assert!(!continuity(&disk).complete);
    assert!(
        continuity(&disk).gaps.is_empty(),
        "last-valid intervals cannot stand in for invalid current source"
    );
}

#[test]
fn policy_diagnostics_share_codes_locations_and_related_events_across_consumers() {
    let mut w = Workspace::new();
    let first = "`- 2026-09-22T10:00:00Z--11:00 中文😀\n `+ event\n";
    w.insert("/notes/a.plumb", 1, first);
    w.insert(
        "/notes/b.plumb",
        1,
        "`- 2026-09-22T12:00:00Z--13:00 Next\n `+ event\n `= event-category work\n",
    );
    let settings = plumb_workspace::WorkspaceConfig::parse(
        "[diagnostics.event-category]\nenabled=true\n[diagnostics.event-timeline]\nenabled=true",
        &[],
    )
    .unwrap()
    .diagnostics;
    let report = w
        .policy_diagnostics(Path::new("/notes"), &[], dt(START), &settings)
        .unwrap();
    assert!(report.complete());
    assert_eq!(report.diagnostics.len(), 3);
    let missing = report
        .diagnostics
        .iter()
        .find(|d| d.code == "event-category.missing")
        .unwrap();
    assert_eq!(missing.source.range.start, first.find("中文😀").unwrap());
    assert_eq!(
        missing.source.range.end,
        first.find("中文😀").unwrap() + "中文😀".len()
    );
    assert!(missing.related.is_empty());
    let gaps = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "event-timeline.gap")
        .collect::<Vec<_>>();
    assert_eq!(gaps.len(), 2);
    assert_eq!(gaps[0].source, gaps[1].related[0]);
    assert_eq!(gaps[1].source, gaps[0].related[0]);
    assert_eq!(gaps[0].message, gaps[1].message);
}

#[test]
fn policy_scopes_exclude_other_and_nested_roots_but_resolve_cross_root_categories() {
    let mut w = Workspace::new();
    w.insert(
        "/notes/a.plumb",
        1,
        "`- 2026-09-22T10:00:00Z--11:00 `->{../other/topic.plumb}\n `+ event\n",
    );
    w.insert(
        "/notes/nested/b.plumb",
        1,
        "`- 2026-09-22T12:00:00Z--13:00 Next\n `+ event\n",
    );
    w.insert("/other/topic.plumb", 1, "`= event-category work\n");
    w.insert("/other/bad.plumb", 1, "`broken{");
    let settings = plumb_workspace::WorkspaceConfig::parse(
        "[diagnostics.event-category]\nenabled=true\n[diagnostics.event-timeline]\nenabled=true",
        &[],
    )
    .unwrap()
    .diagnostics;
    let report = w
        .policy_diagnostics(
            Path::new("/notes"),
            &["/notes/nested".into()],
            dt(START),
            &settings,
        )
        .unwrap();
    assert!(report.complete());
    assert!(report.diagnostics.is_empty());
    w.insert("/notes/a.plumb", 2, "`broken{");
    let report = w
        .policy_diagnostics(
            Path::new("/notes"),
            &["/notes/nested".into()],
            dt(START),
            &settings,
        )
        .unwrap();
    assert!(!report.complete());
    assert_eq!(
        report.incomplete_rules,
        ["event-category", "event-timeline"]
    );
    assert_eq!(
        report.diagnostics.len(),
        1,
        "identical query issues should be deduplicated"
    );
}

#[test]
fn diagnostic_snapshot_is_detached_and_rejects_changed_closed_source_positions() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteSemanticStore::open(temp.path().join("index.sqlite")).unwrap();
    let mut w = Workspace::with_sqlite_store(store);
    let path = temp.path().join("a.plumb");
    let first = "`- 2026-09-22T10:00:00Z Point\n `+ event\n";
    std::fs::write(&path, first).unwrap();
    w.insert_disk(&path, 1, first).unwrap();
    let snapshot = w.readonly_diagnostic_snapshot().unwrap();
    assert_eq!(snapshot.diagnostic_source(&path).unwrap(), first);
    let second = format!("{first} `= event-category work\n");
    std::fs::write(&path, &second).unwrap();
    w.insert_disk(&path, 2, &second).unwrap();
    assert!(snapshot.diagnostic_source(&path).is_err());
    assert_eq!(
        snapshot
            .check_event_categories(temp.path(), dt(START), None)
            .unwrap()
            .missing
            .len(),
        1
    );
    assert!(w
        .check_event_categories(temp.path(), dt(START), None)
        .unwrap()
        .missing
        .is_empty());
    w.open_document(&path, 3, first);
    assert_eq!(
        w.diagnostic_source(&path).unwrap(),
        first,
        "open bytes override saved source"
    );
}

#[test]
fn event_category_scope_precedence_and_invalid_barriers_match_disk_and_memory() {
    let temp = tempfile::tempdir().unwrap();
    let mut disk = Workspace::with_sqlite_store(
        SqliteSemanticStore::open(temp.path().join("index.sqlite")).unwrap(),
    );
    let mut memory = Workspace::new();
    disk.insert_disk("/notes/items.plumb", 0, ITEMS).unwrap();
    memory.insert("/notes/items.plumb", 0, ITEMS);
    let cases = [
        (
            format!(
                "`= event-category\n{}",
                EVENT.replace(" `+ event", " `+ event\n `= event-category own")
            ),
            vec!["own"],
            true,
        ),
        (
            format!("`= event-category root\n{EVENT}"),
            vec!["root"],
            true,
        ),
        (
            format!("{EVENT}\n`= event-category root\n"),
            vec!["root"],
            true,
        ),
        (
            format!(
                "`= event-category root\n`# Section\n `= event-category section\n{}",
                EVENT.lines().map(|l| format!(" {l}\n")).collect::<String>()
            ),
            vec!["section"],
            true,
        ),
        (
            format!(
                "`= event-category root\n`# Section\n `= event-category section\n{}",
                EVENT
                    .replace(" `+ event", " `+ event\n `= event-category own")
                    .lines()
                    .map(|l| format!(" {l}\n"))
                    .collect::<String>()
            ),
            vec!["own"],
            true,
        ),
        (
            format!(
                "`= event-category root\n\nAnonymous\n `= event-category anonymous\n{}",
                EVENT.lines().map(|l| format!(" {l}\n")).collect::<String>()
            ),
            vec!["anonymous"],
            true,
        ),
        (
            format!("`= event-category root\n`# Sibling\n `= event-category section\n{EVENT}"),
            vec!["root"],
            true,
        ),
        (
            format!("`= event-category\n `- one\n `- two\n{EVENT}"),
            vec!["one", "two"],
            true,
        ),
        (
            format!(
                "`= event-category root\n{}",
                EVENT.replace(" `+ event", " `+ event\n `= event-category")
            ),
            vec![],
            false,
        ),
        (
            format!(
                "`= event-category root\n`# Section\n `= event-category\n{}",
                EVENT.lines().map(|l| format!(" {l}\n")).collect::<String>()
            ),
            vec![],
            false,
        ),
        (format!("`= event-category\n{EVENT}"), vec![], false),
        (
            format!("`= event-category root\n`= event-category duplicate\n{EVENT}"),
            vec![],
            false,
        ),
        (
            format!("`= event-category `!{{rich}}\n{EVENT}"),
            vec![],
            false,
        ),
        (EVENT.to_owned(), vec!["learn", "work"], true),
    ];
    let mut memory_state = plumb_workspace::CategoryCheckState::default();
    let mut disk_state = plumb_workspace::CategoryCheckState::default();
    for (revision, (source, expected, complete)) in cases.iter().enumerate() {
        memory.insert("/notes/day.plumb", revision as i64, source.clone());
        disk.insert_disk("/notes/day.plumb", revision as i64, source.clone())
            .unwrap();
        for (workspace, state) in [(&memory, &mut memory_state), (&disk, &mut disk_state)] {
            let cached = workspace
                .check_event_categories_incremental(Path::new("/notes"), dt(START), state)
                .unwrap();
            let fresh = workspace
                .check_event_categories(Path::new("/notes"), dt(START), None)
                .unwrap();
            assert_eq!(
                serde_json::to_value(cached).unwrap(),
                serde_json::to_value(fresh).unwrap()
            );
        }
        let result = report(&memory, true);
        assert_eq!(result.complete, *complete, "{source}: {:?}", result.issues);
        assert_eq!(
            result
                .categories
                .iter()
                .filter_map(|c| c.category.as_deref())
                .collect::<Vec<_>>(),
            *expected,
            "{source}"
        );
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            serde_json::to_value(report(&disk, true)).unwrap()
        );
        let check = memory
            .check_event_categories(Path::new("/notes"), dt(START), None)
            .unwrap();
        assert_eq!(check.complete, *complete, "{source}");
        assert_eq!(check.missing.is_empty(), *complete, "{source}");
        assert_eq!(
            serde_json::to_value(&check).unwrap(),
            serde_json::to_value(
                disk.check_event_categories(Path::new("/notes"), dt(START), None)
                    .unwrap()
            )
            .unwrap()
        );
        assert_eq!(result.items.len(), 3);
        assert!(result.items.iter().all(|item| item.seconds == 1200.0));
        if let Some(issue) = result
            .issues
            .iter()
            .find(|i| i.code == "agenda.invalid-category")
        {
            assert!(source[issue.source.range.start..issue.source.range.end]
                .starts_with("`= event-category"));
        }
    }
}

#[test]
fn document_category_defaults_follow_unsaved_overlay_and_close() {
    let temp = tempfile::tempdir().unwrap();
    let mut w = Workspace::with_sqlite_store(
        SqliteSemanticStore::open(temp.path().join("index.sqlite")).unwrap(),
    );
    let event = "`- 2026-09-22T10:00:00Z--11:00 Work\n `+ event\n";
    let source = format!("`= event-category saved\n{event}");
    w.insert_disk("/notes/day.plumb", 0, source.clone())
        .unwrap();
    assert_eq!(
        report(&w, true).categories[0].category.as_deref(),
        Some("saved")
    );
    w.open_document("/notes/day.plumb", 1, source.replace("saved", "unsaved"));
    assert_eq!(
        report(&w, true).categories[0].category.as_deref(),
        Some("unsaved")
    );
    w.open_document("/notes/day.plumb", 2, event);
    assert!(report(&w, true).categories[0].category.is_none());
    w.close_document("/notes/day.plumb");
    assert_eq!(
        report(&w, true).categories[0].category.as_deref(),
        Some("saved")
    );
}

#[test]
fn invalid_document_diagnostics_match_memory_and_persistent_category_checks() {
    let temp = tempfile::tempdir().unwrap();
    let mut disk = Workspace::with_sqlite_store(
        SqliteSemanticStore::open(temp.path().join("index.sqlite")).unwrap(),
    );
    let mut memory = Workspace::new();
    let bad = "`- {unclosed\n";
    memory.insert("/notes/invalid.plumb", 0, bad);
    disk.insert_disk("/notes/invalid.plumb", 0, bad).unwrap();
    memory.insert("/notes/day.plumb", 0, EVENT);
    disk.insert_disk("/notes/day.plumb", 0, EVENT).unwrap();
    for workspace in [&memory, &disk] {
        let check = workspace
            .check_event_categories(Path::new("/notes"), dt(START), None)
            .unwrap();
        assert!(!check.complete);
        assert!(check
            .issues
            .iter()
            .any(|issue| issue.code == "agenda.invalid-document"));
    }
    assert_eq!(
        serde_json::to_value(
            memory
                .check_event_categories(Path::new("/notes"), dt(START), None)
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(
            disk.check_event_categories(Path::new("/notes"), dt(START), None)
                .unwrap()
        )
        .unwrap(),
    );
    assert_eq!(
        serde_json::to_value(report(&memory, true)).unwrap(),
        serde_json::to_value(report(&disk, true)).unwrap()
    );
    assert_eq!(
        serde_json::to_value(continuity(&memory)).unwrap(),
        serde_json::to_value(continuity(&disk)).unwrap()
    );
}

#[test]
fn accounting_reuse_preserves_each_reference_policy_location_and_overlay_revision() {
    let temp = tempfile::tempdir().unwrap();
    let mut w = Workspace::with_sqlite_store(
        SqliteSemanticStore::open(temp.path().join("index.sqlite")).unwrap(),
    );
    let items = "`- Activity\n `@ a\n `= event-category work\n";
    let source = concat!(
        "`- 2026-09-22T10:00:00Z `->{items.plumb#a}\n `+ event\n",
        "`- 2026-09-22T10:01:00Z Task only\n `+ event\n `= tasks items.plumb#a\n",
        "`- 2026-09-22T10:02:00Z `->{items.plumb#missing}\n `+ event\n",
        "`- 2026-09-22T10:03:00Z `->{./items.plumb#missing}\n `+ event\n",
    );
    w.insert_disk("/notes/items.plumb", 0, items).unwrap();
    w.insert_disk("/notes/day.plumb", 0, source).unwrap();
    // Same source spelling in a different directory must resolve independently.
    w.insert_disk("/notes/sub/items.plumb", 0, "`- Local\n `@ a\n")
        .unwrap();
    w.insert_disk(
        "/notes/sub/day.plumb",
        0,
        "`- 2026-09-22T10:04:00Z `->{items.plumb#a}\n `+ event\n",
    )
    .unwrap();
    let check = |w: &Workspace| {
        w.check_event_categories(Path::new("/notes"), dt(START), None)
            .unwrap()
    };
    let first = check(&w);
    assert_eq!(first.checked, 5);
    assert_eq!(first.missing.len(), 4);
    let issues = first
        .issues
        .iter()
        .filter(|issue| issue.code == "agenda.invalid-item")
        .collect::<Vec<_>>();
    assert_eq!(issues.len(), 3);
    assert!(issues
        .windows(2)
        .all(|pair| pair[0].source.range.start < pair[1].source.range.start));
    assert!(issues[2].message.contains("./items.plumb#missing"));
    w.open_document(
        "/notes/items.plumb",
        1,
        format!(
            "{}\n`- Found\n `@ missing\n `= event-category fixed\n",
            items.replace(" `@ a", " `+ task\n `@ a")
        ),
    );
    let changed = check(&w);
    assert!(changed.complete);
    assert!(changed.issues.is_empty());
    assert_eq!(changed.missing.len(), 1);
    assert_eq!(changed.missing[0].path, Path::new("/notes/sub/day.plumb"));
    w.close_document("/notes/items.plumb");
    assert_eq!(
        serde_json::to_value(first).unwrap(),
        serde_json::to_value(check(&w)).unwrap()
    );
}

#[test]
fn incremental_timeline_rebinds_geometry_and_matches_fresh_after_interval_changes() {
    let mut workspace = Workspace::new();
    let mut state = plumb_workspace::TimelineCheckState::default();
    let root = Path::new("/notes");
    let source = "`- 2026-09-22T10:00:00Z--10:20 First\n `+ event\n`- 2026-09-22T10:30:00Z--11:00 Second\n `+ event\n";
    for (revision, text) in [
        source.to_owned(),
        format!("\n{source}"),
        source.replace("10:20", "10:40"),
        source.replace("10:30", "10:20"),
        String::new(),
    ]
    .into_iter()
    .enumerate()
    {
        workspace.insert("/notes/day.plumb", revision as i64, text);
        let actual = workspace
            .check_event_timeline_incremental(root, dt(START), &mut state)
            .unwrap();
        let fresh = workspace.check_event_timeline(root, dt(START)).unwrap();
        assert_eq!(
            serde_json::to_value(&actual).unwrap(),
            serde_json::to_value(&fresh).unwrap()
        );
        if revision == 1 {
            assert_eq!(state.recomputed_segments, 0);
            assert_eq!(
                actual.gaps[0].events[0].range.start,
                source.find("First").unwrap() + 1
            );
        }
    }
}

#[test]
fn incremental_categories_track_target_changes_and_rebind_missing_locations() {
    let temp = tempfile::tempdir().unwrap();
    let mut workspace = Workspace::with_sqlite_store(
        SqliteSemanticStore::open(temp.path().join("index.sqlite")).unwrap(),
    );
    workspace
        .insert_disk("/notes/items.plumb", 0, ITEMS)
        .unwrap();
    workspace.open_document("/notes/day.plumb", 0, EVENT);
    let mut state = plumb_workspace::CategoryCheckState::default();
    let root = Path::new("/notes");
    for revision in 0..6 {
        match revision {
            1 => {
                workspace.open_document("/notes/day.plumb", 1, format!("\n{EVENT}"));
            }
            2 => {
                workspace.open_document(
                    "/notes/items.plumb",
                    1,
                    ITEMS.replace(" `= event-category work\n", ""),
                );
            }
            3 => {
                workspace.open_document(
                    "/notes/items.plumb",
                    2,
                    ITEMS.replace(" `@ a", " `@ renamed"),
                );
            }
            4 => {
                workspace.close_document("/notes/items.plumb");
            }
            5 => {
                workspace.open_document("/notes/day.plumb", 2, EVENT.replace("11:00", "12:00"));
            }
            _ => {}
        }
        let result = workspace
            .check_event_categories_incremental(root, dt(START), &mut state)
            .unwrap();
        let fresh = workspace
            .check_event_categories(root, dt(START), None)
            .unwrap();
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::to_value(fresh).unwrap(),
            "revision {revision}"
        );
        if revision == 1 || revision == 5 {
            assert_eq!(state.recomputed_events, 0);
        }
        if revision == 2 || revision == 3 {
            assert_eq!(state.recomputed_events, 1);
        }
    }
}

#[test]
fn incremental_timeline_matches_full_across_disk_overlay_invalid_and_offset_changes() {
    let temp = tempfile::tempdir().unwrap();
    let mut workspace = Workspace::with_sqlite_store(
        SqliteSemanticStore::open(temp.path().join("index.sqlite")).unwrap(),
    );
    workspace
        .insert_disk(
            "/notes/a.plumb",
            0,
            "`- 2026-09-22T10:00:00Z--11:00 A\n `+ event\n",
        )
        .unwrap();
    let mut state = plumb_workspace::TimelineCheckState::default();
    let root = Path::new("/notes");
    let cases = [
        "`- 2026-09-22T12:00:00Z--13:00 B\n `+ event\n",
        "`- 2026-09-22T10:30:00Z--12:00 Nested\n `+ event\n",
        "`- 2026-09-22T18:30:00+08:00--20:00 Same instant\n `+ event\n",
        "`- 2026-09-22T12:00:00Z Point\n `+ event\n",
        "`- 2026-09-22T12:00:00Z-- Running\n `+ event\n",
        "`broken{",
        "",
    ];
    for (revision, source) in cases.into_iter().enumerate() {
        workspace.open_document("/notes/b.plumb", revision as i64, source);
        let result = workspace
            .check_event_timeline_incremental(root, dt(START), &mut state)
            .unwrap();
        let fresh = workspace.check_event_timeline(root, dt(START)).unwrap();
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::to_value(fresh).unwrap(),
            "case {revision}"
        );
    }
}

#[test]
fn inserting_another_document_preserves_existing_event_policy_identities() {
    let mut workspace = Workspace::new();
    let root = Path::new("/notes");
    workspace.insert("/notes/z.plumb", 1, "`= event-category work\n`- 2026-09-22T10:00:00Z--11:00 Z\n `+ event\n");
    let mut category = plumb_workspace::CategoryCheckState::default();
    let mut timeline = plumb_workspace::TimelineCheckState::default();
    workspace.check_event_categories_incremental(root, dt(START), &mut category).unwrap();
    workspace.check_event_timeline_incremental(root, dt(START), &mut timeline).unwrap();
    workspace.insert("/notes/a.plumb", 1, "`= event-category work\n`- 2026-09-22T18:00:00+08:00--19:00 A\n `+ event\n");
    workspace.check_event_categories_incremental(root, dt(START), &mut category).unwrap();
    assert_eq!(category.recomputed_events, 1);
    let actual = workspace.check_event_timeline_incremental(root, dt(START), &mut timeline).unwrap();
    let fresh = workspace.check_event_timeline(root, dt(START)).unwrap();
    assert_eq!(serde_json::to_value(actual).unwrap(), serde_json::to_value(fresh).unwrap());
}

#[test]
fn explicit_event_category_does_not_depend_on_target_category_values() {
    let mut workspace = Workspace::new();
    let root = Path::new("/notes");
    workspace.insert("/notes/items.plumb", 0, ITEMS);
    workspace.insert("/notes/day.plumb", 0, format!("`= event-category fixed\n{EVENT}"));
    let mut state = plumb_workspace::CategoryCheckState::default();
    workspace.check_event_categories_incremental(root, dt(START), &mut state).unwrap();
    workspace.insert("/notes/items.plumb", 1, ITEMS.replace("work", "personal"));
    let actual = workspace.check_event_categories_incremental(root, dt(START), &mut state).unwrap();
    assert_eq!(state.recomputed_events, 0);
    let full = workspace.check_event_categories(root, dt(START), None).unwrap();
    assert_eq!(serde_json::to_value(actual).unwrap(), serde_json::to_value(full).unwrap());
}
