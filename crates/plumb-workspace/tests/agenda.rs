use chrono::{DateTime, FixedOffset};
use plumb_workspace::{SqliteSemanticStore, Workspace};
use std::path::Path;

fn dt(s: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(s).unwrap()
}
const START: &str = "2026-09-22T10:00:00Z";
const END: &str = "2026-09-22T11:00:00Z";
const ITEMS: &str = "`- A\n `+ task\n `@ a\n `= category work\n`- B\n `+ task\n `@ b\n `= category work\n`- C\n `@ c\n `= category learn\n";
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
        EVENT.replace(" `+ event", " `+ event\n `= category personal"),
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
        "`+ task\n`= title Project\n`= category work\n",
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
            " `= category learn",
            " `= category\n  `- learn\n  `- phd misc\n  `- learn",
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
            " `+ event\n `= category\n  `- personal\n  `- work",
        ),
    );
    let r = report(&w, true);
    assert_eq!(
        r.categories.iter().map(|c| c.seconds).collect::<Vec<_>>(),
        [1800.0, 1800.0]
    );
}

#[test]
fn category_check_covers_all_dates_points_and_explicit_mode_ignores_references() {
    let mut w = Workspace::new();
    w.insert("/notes/items.plumb", 0, ITEMS);
    w.insert("/notes/day.plumb", 0, EVENT);
    let check = |w: &Workspace, explicit| {
        w.check_event_categories(Path::new("/notes"), dt(START), None, explicit)
            .unwrap()
    };
    assert!(check(&w, false).missing.is_empty());
    assert_eq!(check(&w, true).missing.len(), 1);
    w.insert(
        "/notes/old.plumb",
        0,
        "`- 2020-01-01T10:00:00Z Point\n `+ event\n",
    );
    assert_eq!(check(&w, false).checked, 2);
    assert_eq!(check(&w, false).missing.len(), 1);
    w.insert(
        "/notes/day.plumb",
        1,
        EVENT
            .replace("items.plumb#a", "missing.plumb")
            .replace(" `+ event", " `+ event\n `= category phd misc"),
    );
    assert!(check(&w, true).complete);
    assert!(!check(&w, false).complete);
    w.insert(
        "/notes/old.plumb",
        1,
        "`- 2020-01-01T10:00:00Z Point\n `+ event\n `= category\n",
    );
    assert!(!check(&w, true).complete);
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
    for explicit in [true, false] {
        let check = |w: &Workspace| {
            w.check_event_categories(Path::new("/notes"), dt(START), None, explicit)
                .unwrap()
        };
        assert_eq!(
            serde_json::to_value(check(&disk)).unwrap(),
            serde_json::to_value(check(&memory)).unwrap()
        );
    }
    let check = disk
        .check_event_categories(
            Path::new("/notes"),
            dt(START),
            Some("path == 'other.plumb'"),
            true,
        )
        .unwrap();
    assert_eq!(check.checked, 0);
    assert!(check.complete);
}
