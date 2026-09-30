use plumb_workspace::{AgendaItem, SqliteSemanticStore, Workspace};
use std::{collections::BTreeMap, path::Path};
const TARGET: &str = "`- A\n `+ task\n `@ a\n`- B\n `@ b\n";
fn event(target: &str) -> String {
    format!("`- 2026-10-01T10:00:00Z--11:00 `->{{{target}}}\n `+ event\n")
}
fn requested(w: &Workspace) -> plumb_workspace::TaskDurationTotals {
    w.task_durations_for([AgendaItem {
        path: "/notes/targets.plumb".into(),
        id: Some("a".into()),
    }])
    .unwrap()
}
fn fixture(persistent: bool, size: usize) -> Workspace {
    let mut w = if persistent {
        Workspace::with_sqlite_store(SqliteSemanticStore::open_in_memory().unwrap())
    } else {
        Workspace::new()
    };
    w.insert_disk("/notes/targets.plumb", 0, TARGET).unwrap();
    w.insert_disk("/notes/day.plumb", 0, event("targets.plumb#a"))
        .unwrap();
    for i in 0..size {
        w.insert_disk(
            format!("/notes/unrelated-{i}.plumb"),
            0,
            event("unrelated-target.plumb"),
        )
        .unwrap();
    }
    w.insert_disk("/notes/unrelated-target.plumb", 0, "Ordinary document\n")
        .unwrap();
    w
}
#[test]
fn warm_reads_and_local_edits_have_work_independent_of_unrelated_workspace_size() {
    for persistent in [false, true] {
        for size in [0, 128] {
            let mut w = fixture(persistent, size);
            assert!(requested(&w).complete);
            assert_eq!(w.duration_work().events_recomputed, size + 1);
            requested(&w);
            assert_eq!(w.duration_work(), Default::default());
            w.insert_disk(
                "/notes/targets.plumb",
                1,
                TARGET.replace("A\n", "Renamed\n"),
            )
            .unwrap();
            requested(&w);
            assert_eq!(w.duration_work().documents_read, 1);
            assert_eq!(w.duration_work().events_recomputed, 0);
            assert_eq!(w.duration_work().contributions_changed, 0);
            w.insert_disk(
                "/notes/day.plumb",
                1,
                event("targets.plumb#a").replace("--11:00", "--12:00"),
            )
            .unwrap();
            let totals = requested(&w);
            assert_eq!(
                totals.seconds.values().copied().collect::<Vec<_>>(),
                [7200.0]
            );
            assert_eq!(w.duration_work().documents_read, 1);
            assert_eq!(w.duration_work().events_read, 1);
            assert_eq!(w.duration_work().events_recomputed, 1);
            assert_eq!(w.duration_work().contributions_changed, 2);
            if persistent {
                assert_eq!(w.duration_work().stored_changes_read, 1);
            }
            w.insert_disk(
                "/notes/targets.plumb",
                2,
                TARGET.replace(" `@ a", " `@ a\n `= event-category {}"),
            )
            .unwrap();
            assert!(!requested(&w).complete);
            assert_eq!(w.duration_work().events_recomputed, 1);
            w.insert_disk("/notes/targets.plumb", 3, TARGET).unwrap();
            assert!(requested(&w).complete);
            assert_eq!(w.duration_work().events_recomputed, 1);
            w.remove_disk("/notes/day.plumb").unwrap();
            assert_eq!(
                requested(&w).seconds.values().copied().collect::<Vec<_>>(),
                [0.0]
            );
            assert_eq!(w.duration_work().events_recomputed, 0);
            assert_eq!(w.duration_work().contributions_changed, 1);
        }
    }
}

#[test]
fn negative_dependencies_ambiguity_and_geometry_rebind_without_numeric_recomputation() {
    let mut w = Workspace::new();
    w.insert("/notes/day.plumb", 0, event("targets.plumb#a"));
    assert!(!requested(&w).complete);
    w.insert("/notes/targets.plumb", 0, TARGET);
    assert!(requested(&w).complete);
    assert_eq!(w.duration_work().events_recomputed, 1);
    let before = w
        .document_durations(Path::new("/notes/targets.plumb"))
        .unwrap()
        .value;
    w.insert(
        "/notes/day.plumb",
        1,
        format!("😀\n\n{}", event("targets.plumb#a")),
    );
    let after = w
        .document_durations(Path::new("/notes/targets.plumb"))
        .unwrap()
        .value;
    assert_eq!(w.duration_work().events_recomputed, 0);
    assert_eq!(after[0].value, before[0].value);
    assert_eq!(
        after[0].sources[0].range.start,
        before[0].sources[0].range.start + 6
    );
    w.insert(
        "/notes/targets.plumb",
        1,
        format!("{TARGET}`- Duplicate\n `@ a\n"),
    );
    assert!(!requested(&w).complete);
    assert_eq!(w.duration_work().events_recomputed, 1);
    assert!(
        w.task_duration_totals().unwrap().seconds.is_empty(),
        "ambiguous targets have no task membership"
    );
    w.remove("/notes/targets.plumb");
    assert!(!requested(&w).complete);
    w.insert("/notes/targets.plumb", 2, TARGET);
    assert!(requested(&w).complete);
}

#[test]
fn disk_deltas_overlay_pending_invalid_and_immutable_snapshots_remain_consistent() {
    let store = SqliteSemanticStore::open_in_memory().unwrap();
    let mut writer = Workspace::with_sqlite_store(store.clone());
    writer
        .insert_disk("/notes/targets.plumb", 0, TARGET)
        .unwrap();
    writer
        .insert_disk("/notes/day.plumb", 0, event("targets.plumb#a"))
        .unwrap();
    let old = Workspace::with_sqlite_store(store.readonly_snapshot().unwrap());
    let original = requested(&old);
    writer
        .insert_disk(
            "/notes/day.plumb",
            1,
            event("targets.plumb#a").replace("--11:00", "--12:00"),
        )
        .unwrap();
    let mut next = old.with_updated_store(store.readonly_snapshot().unwrap());
    assert_eq!(
        requested(&next)
            .seconds
            .values()
            .copied()
            .collect::<Vec<_>>(),
        [7200.0]
    );
    assert_eq!(next.duration_work().documents_read, 1);
    assert_eq!(requested(&old), original);
    next.open_document(
        "/notes/day.plumb",
        2,
        event("targets.plumb#a").replace("--11:00", "--13:00"),
    );
    assert_eq!(
        requested(&next)
            .seconds
            .values()
            .copied()
            .collect::<Vec<_>>(),
        [10800.0]
    );
    let frozen = next.clone();
    next.begin_document_revision("/notes/day.plumb", 3, event("targets.plumb#a"));
    assert!(!requested(&next).complete);
    next.complete_pending_document_analysis("/notes/day.plumb");
    assert_eq!(requested(&next), original);
    next.open_document("/notes/day.plumb", 4, "`broken{\n");
    assert!(!requested(&next).complete);
    next.close_document("/notes/day.plumb");
    assert_eq!(
        requested(&next)
            .seconds
            .values()
            .copied()
            .collect::<Vec<_>>(),
        [7200.0]
    );
    assert_eq!(
        requested(&frozen)
            .seconds
            .values()
            .copied()
            .collect::<Vec<_>>(),
        [10800.0]
    );
    let empty_store = SqliteSemanticStore::open_in_memory().unwrap();
    let other = next.with_updated_store(empty_store);
    assert_eq!(
        requested(&other)
            .seconds
            .values()
            .copied()
            .collect::<Vec<_>>(),
        [0.0]
    );
}

#[test]
fn incremental_results_and_locations_match_cold_rebuild_and_full_agenda_oracle() {
    use chrono::DateTime;
    let from = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").unwrap();
    let to = DateTime::parse_from_rfc3339("2027-01-01T00:00:00Z").unwrap();
    for persistent in [false, true] {
        let mut sources = BTreeMap::from([
            ("/notes/targets.plumb", TARGET.to_string()),
            ("/notes/day.plumb", event("targets.plumb#a")),
        ]);
        let mut w = if persistent {
            Workspace::with_sqlite_store(SqliteSemanticStore::open_in_memory().unwrap())
        } else {
            Workspace::new()
        };
        for (path, source) in &sources {
            w.insert_disk(path, 0, source.clone()).unwrap();
        }
        for (path, source) in [
            ("/notes/day.plumb", event("targets.plumb#a")),
            (
                "/notes/day.plumb",
                event("targets.plumb#a").replace(
                    " `+ event",
                    " `+ event\n `= tasks targets.plumb#a targets.plumb#b",
                ),
            ),
            ("/notes/day.plumb", event("targets.plumb#b")),
            (
                "/notes/targets.plumb",
                TARGET.replace("`- B", "`- B\n `+ task"),
            ),
            (
                "/notes/targets.plumb",
                TARGET.replace(" `@ b", " `@ b\n `= event-category {}"),
            ),
            ("/notes/targets.plumb", format!("Header\n\n{TARGET}")),
            ("/notes/day.plumb", event("targets.plumb#missing")),
            (
                "/notes/day.plumb",
                format!("😀\n\n{}", event("targets.plumb#missing")),
            ),
            (
                "/notes/day.plumb",
                event("targets.plumb#a").replace("--11:00", "--"),
            ),
            (
                "/notes/day.plumb",
                event("targets.plumb#a").replace("--11:00", ""),
            ),
            (
                "/notes/day.plumb",
                event("targets.plumb#a").replace("2026-10-01T10:00:00Z--11:00", "bad"),
            ),
            ("/notes/day.plumb", "`broken{\n".into()),
            ("/notes/day.plumb", event("targets.plumb#a")),
        ] {
            sources.insert(path, source.clone());
            w.insert_disk(path, 1, source).unwrap();
            let mut cold = Workspace::new();
            for (path, source) in &sources {
                cold.insert(path, 1, source.clone());
            }
            let warm = w.task_duration_totals().unwrap();
            assert_eq!(warm, cold.task_duration_totals().unwrap());
            let report = cold
                .agenda_report(Path::new("/notes"), from, to, from, None, true)
                .unwrap();
            assert_eq!(warm.complete, report.complete);
            let expected = report
                .tasks
                .into_iter()
                .map(|item| (item.item, item.seconds))
                .collect::<BTreeMap<_, _>>();
            assert_eq!(warm.seconds, expected);
            w.open_document(
                "/notes/targets.plumb",
                1,
                sources["/notes/targets.plumb"].clone(),
            );
            assert_eq!(
                w.document_durations(Path::new("/notes/targets.plumb"))
                    .unwrap()
                    .value,
                cold.document_durations(Path::new("/notes/targets.plumb"))
                    .unwrap()
                    .value
            );
            if persistent {
                w.close_document("/notes/targets.plumb");
            }
        }
        assert_eq!(
            w.document_durations(Path::new("/notes/day.plumb"))
                .unwrap()
                .value
                .len(),
            if persistent { 0 } else { 1 }
        );
    }
}

#[test]
fn external_store_writes_and_shadowed_generations_use_the_same_incremental_path() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("store.sqlite");
    let reader_store = SqliteSemanticStore::open(&path).unwrap();
    let mut writer = Workspace::with_sqlite_store(SqliteSemanticStore::open(&path).unwrap());
    writer
        .insert_disk("/notes/targets.plumb", 0, TARGET)
        .unwrap();
    writer
        .insert_disk("/notes/day.plumb", 0, event("targets.plumb#a"))
        .unwrap();
    let mut reader = Workspace::with_sqlite_store(reader_store);
    let first = requested(&reader);
    writer
        .insert_disk(
            "/notes/day.plumb",
            1,
            event("targets.plumb#a").replace("--11:00", "--12:00"),
        )
        .unwrap();
    assert_ne!(requested(&reader), first);
    assert_eq!(reader.duration_work().documents_read, 1);
    reader.open_document("/notes/day.plumb", 2, event("targets.plumb#a"));
    assert_eq!(requested(&reader), first);
    writer.remove_disk("/notes/day.plumb").unwrap();
    assert_eq!(requested(&reader), first);
    assert_eq!(reader.duration_work().documents_read, 0);
    reader.close_document("/notes/day.plumb");
    assert_eq!(
        requested(&reader)
            .seconds
            .values()
            .copied()
            .collect::<Vec<_>>(),
        [0.0]
    );
}

#[test]
fn changing_one_event_in_a_large_document_only_reallocates_that_event() {
    let mut w = Workspace::new();
    w.insert("/notes/targets.plumb", 0, TARGET);
    let source = event("targets.plumb#a").repeat(64);
    w.insert("/notes/day.plumb", 0, &source);
    requested(&w);
    w.insert(
        "/notes/day.plumb",
        1,
        source.replacen("--11:00", "--12:00", 1),
    );
    let result = w
        .document_durations(Path::new("/notes/targets.plumb"))
        .unwrap()
        .value;
    assert_eq!(
        w.duration_work().events_read,
        64,
        "matching is document-local"
    );
    assert_eq!(w.duration_work().events_recomputed, 1);
    assert_eq!(w.duration_work().contributions_changed, 2);
    assert_eq!(result[0].sources.len(), 64);
    assert_eq!(
        result[0].value,
        plumb_workspace::DurationValue::Seconds(65.0 * 3600.0)
    );
}
