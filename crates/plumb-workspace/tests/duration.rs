use plumb_workspace::{DurationKind, DurationValue, SqliteSemanticStore, Workspace};
use std::path::Path;

const TASKS: &str = "`+ task\n`= title Project\n`- A\n `+ task\n `@ a\n `= focused 2026-09-01T00:00:00Z--2026-09-02T00:00:00Z\n `= event-category\n  `- work\n  `- learning\n`- B\n `+ task\n `@ b\n`- Ordinary\n `@ c\n`- No id\n `+ task\n";
const EVENT: &str = "`- 2026-09-22T23:00:00Z--01:00 `->{tasks.plumb#a} `->{tasks.plumb#a} `->{tasks.plumb#b} `->{tasks.plumb#c}\n `+ event\n";

#[test]
fn all_time_totals_share_agenda_rules_across_midnight_overlaps_and_categories() {
    for persistent in [false, true] {
        let mut w = if persistent {
            Workspace::with_sqlite_store(SqliteSemanticStore::open_in_memory().unwrap())
        } else {
            Workspace::new()
        };
        w.insert("/notes/tasks.plumb", 0, TASKS);
        let events = format!("{EVENT}{EVENT}`- 2026-10-01T10:00:00Z--11:00 Project\n `+ event\n `= tasks tasks.plumb\n");
        w.insert_disk("/notes/day.plumb", 0, &events).unwrap();
        let result = w
            .document_durations(Path::new("/notes/tasks.plumb"))
            .unwrap();
        assert!(result.is_complete());
        let lenses = result.value;
        assert_eq!(
            lenses.iter().map(|l| l.value).collect::<Vec<_>>(),
            [
                DurationValue::Seconds(3600.0),
                DurationValue::Seconds(4800.0),
                DurationValue::Seconds(4800.0),
                DurationValue::Seconds(0.0),
            ]
        );
        assert!(lenses.iter().all(|l| l.kind == DurationKind::Task));
        assert_eq!(lenses[0].range, 0..0);
        assert_eq!(
            lenses[1].sources.len(),
            2,
            "category split must not duplicate source locations"
        );
        assert!(w.get(Path::new("/notes/day.plumb")).is_none() == persistent);
        // Current overlay replaces disk; ongoing and point events do not contribute.
        w.insert("/notes/day.plumb", 1, "`- 2026-10-01T10:00:00Z-- Running\n `+ event\n `= tasks tasks.plumb#a\n`- 2026-10-01T11:00:00Z Point\n `+ event\n");
        assert!(w
            .document_durations(Path::new("/notes/tasks.plumb"))
            .unwrap()
            .value
            .iter()
            .all(|l| l.value == DurationValue::Seconds(0.0)));
        let events = w
            .document_durations(Path::new("/notes/day.plumb"))
            .unwrap()
            .value;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].value, DurationValue::Ongoing);
    }
}

#[test]
fn incomplete_accounting_never_masquerades_as_total_and_event_duration_stays_local() {
    for persistent in [false, true] {
        let mut w = if persistent {
            Workspace::with_sqlite_store(SqliteSemanticStore::open_in_memory().unwrap())
        } else {
            Workspace::new()
        };
        w.insert("/notes/tasks.plumb", 0, TASKS);
        for bad in [
            "`unclosed{\n".to_owned(),
            EVENT.replace("tasks.plumb#b", "tasks.plumb#missing"),
            EVENT.replace(" `+ event", " `+ event\n `= tasks"),
            EVENT.replace("23:00:00Z--01:00", "bad-time"),
            EVENT.replace(" `+ event", " `+ event\n `= event-category {}"),
        ] {
            w.insert_disk("/notes/day.plumb", 1, bad).unwrap();
            let result = w
                .document_durations(Path::new("/notes/tasks.plumb"))
                .unwrap();
            assert!(result.is_complete(), "invalid differs from pending");
            assert!(result
                .value
                .iter()
                .all(|l| l.value == DurationValue::Incomplete));
            assert!(result.value.iter().all(|l| !l.sources.is_empty()));
        }
        w.insert(
            "/notes/day.plumb",
            2,
            EVENT.replace("tasks.plumb#b", "tasks.plumb#missing"),
        );
        let lenses = w
            .document_durations(Path::new("/notes/day.plumb"))
            .unwrap()
            .value;
        assert_eq!(lenses[0].value, DurationValue::Seconds(7200.0));
        w.insert("/notes/day.plumb", 3, "`unclosed{\n");
        assert!(
            w.document_durations(Path::new("/notes/tasks.plumb"))
                .unwrap()
                .value
                .iter()
                .all(|l| l.value == DurationValue::Incomplete),
            "never reuse last-valid events"
        );
        w.insert("/notes/day.plumb", 4, EVENT);
        assert_eq!(
            w.document_durations(Path::new("/notes/tasks.plumb"))
                .unwrap()
                .value[1]
                .value,
            DurationValue::Seconds(2400.0)
        );
    }
}

#[test]
fn pending_analysis_defers_totals_and_restores_current_results() {
    let mut w = Workspace::new();
    w.insert("/notes/tasks.plumb", 0, TASKS);
    w.insert("/notes/day.plumb", 0, EVENT);
    w.begin_document_revision("/notes/day.plumb", 1, EVENT.replace("01:00", "02:00"))
        .unwrap();
    assert!(!w
        .document_durations(Path::new("/notes/tasks.plumb"))
        .unwrap()
        .is_complete());
    assert!(w.complete_pending_document_analysis("/notes/day.plumb"));
    let result = w
        .document_durations(Path::new("/notes/tasks.plumb"))
        .unwrap();
    assert!(result.is_complete());
    assert_eq!(result.value[1].value, DurationValue::Seconds(3600.0));
}

#[test]
fn duration_invalidation_tracks_time_and_accounting_but_not_workflow_or_plain_titles() {
    let event = "`- 2026-10-01T10:00:00Z--11:00 Work\n `+ event\n";
    let task = "`- Task\n `+ task\n `@ a\n `= wait 2026-10-01T00:00:00Z\n";
    let category = "`= event-category good\n";
    let item = "`- Item\n `@ a\n `= event-category good\n";
    for (old, new, expected) in [
        (event, event.replace("11:00", "12:00"), true),
        (event, event.replace("Work", "Rest"), false),
        (event, event.replace("--11:00", "--"), true),
        (event, event.replace("--11:00", ""), true),
        (task, task.replace("wait", "done"), false),
        (task, task.replace("Task", "Next"), false),
        (category, category.replace("good", "{}  "), true),
        (item, item.replace("good", "{}  "), true),
        (item, item.replace("`-", "`#"), true),
    ] {
        let mut w = Workspace::new();
        w.insert("/notes/test.plumb", 0, old);
        let prepared = w
            .begin_document_revision("/notes/test.plumb", 1, &new)
            .unwrap()
            .analyze();
        let impact = w.install_document_analysis_with_impact(prepared).unwrap();
        assert_eq!(impact.duration_inputs_changed, expected, "{new}");
    }
}

#[test]
fn explicit_tasks_override_title_links_and_preserve_fractional_seconds() {
    let mut w = Workspace::new();
    w.insert("/notes/tasks.plumb", 0, TASKS);
    w.insert("/notes/day.plumb", 0,
        "`- 2026-10-01T10:00:00Z--10:00:01 `->{tasks.plumb#c}\n `+ event\n `= tasks tasks.plumb#a tasks.plumb#b tasks.plumb\n");
    let lenses = w
        .document_durations(Path::new("/notes/tasks.plumb"))
        .unwrap()
        .value;
    for lens in &lenses[..3] {
        assert_eq!(lens.value, DurationValue::Seconds(1.0 / 3.0));
        assert_eq!(lens.sources.len(), 1);
    }
}
