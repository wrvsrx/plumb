use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

#[test]
fn sqlite_disk_documents_are_shadowed_by_complete_open_snapshots() {
    let store = SqliteSemanticStore::open_in_memory().unwrap();
    let mut workspace = Workspace::with_sqlite_store(store);
    let target = "`- Target\n\n `+ task\n\n `@ target\n";
    let disk_source = "See `->[target|target.plumb#target].\n";
    assert!(!workspace.insert_disk("target.plumb", 0, target).unwrap());
    assert!(!workspace
        .insert_disk("source.plumb", 0, disk_source)
        .unwrap());
    assert!(workspace.documents().next().is_none());
    assert_eq!(
        workspace
            .reverse_references_for_document("target.plumb", &HashSet::from(["target".to_string()]))
            .unwrap()
            .value
            .anchors["target"]
            .len(),
        1
    );

    workspace.open_document("source.plumb", 1, "No reference.\n");
    assert!(workspace
        .reverse_references_for_document("target.plumb", &HashSet::from(["target".to_string()]))
        .unwrap()
        .value
        .anchors
        .get("target")
        .is_none_or(Vec::is_empty));

    workspace.close_document("source.plumb");
    assert_eq!(
        workspace
            .reverse_references_for_document("target.plumb", &HashSet::from(["target".to_string()]))
            .unwrap()
            .value
            .anchors["target"]
            .len(),
        1
    );
}

#[test]
fn sqlite_warm_insert_skips_document_analysis() {
    let store = SqliteSemanticStore::open_in_memory().unwrap();
    let mut workspace = Workspace::with_sqlite_store(store);
    let source = "`- 2026-08-11T10:00|Cached\n\n `+ event\n";
    assert!(!workspace.insert_disk("event.plumb", 0, source).unwrap());
    assert!(workspace.insert_disk("event.plumb", 0, source).unwrap());
    let paths = workspace.document_paths().unwrap();
    assert_eq!(paths.provenance, QueryProvenance::Persistent);
    assert_eq!(paths.completeness, QueryCompleteness::Complete);
    assert_eq!(paths.value, [PathBuf::from("event.plumb")]);
}

#[test]
fn sqlite_query_failures_are_not_reported_as_empty_or_negative_results() {
    let database = temp_workspace().with_extension("sqlite");
    let store = SqliteSemanticStore::open(&database).unwrap();
    let mut workspace = Workspace::with_sqlite_store(store.clone());
    workspace
        .insert_disk(
            "tasks.plumb",
            0,
            "`- Persisted\n\n `+ task\n\n `@ persisted\n",
        )
        .unwrap();

    store
        .execute_batch_for_test("DROP TABLE documents;")
        .unwrap();

    assert!(matches!(
        workspace.document_paths(),
        Err(WorkspaceQueryError::Store(StoreError::Diesel(_)))
    ));
    assert!(matches!(
        workspace.contains("tasks.plumb"),
        Err(WorkspaceQueryError::Store(StoreError::Diesel(_)))
    ));

    drop(workspace);
    std::fs::remove_file(database).unwrap();
}

#[test]
fn sqlite_task_query_failures_do_not_fall_back_to_reanalysis() {
    let database = temp_workspace().with_extension("sqlite");
    let store = SqliteSemanticStore::open(&database).unwrap();
    let mut workspace = Workspace::with_sqlite_store(store.clone());
    workspace
        .insert_disk(
            "tasks.plumb",
            0,
            "`- Persisted\n\n `+ task\n\n `@ persisted\n",
        )
        .unwrap();

    store.execute_batch_for_test("DROP TABLE tasks;").unwrap();
    let now = DateTime::parse_from_rfc3339("2026-08-27T00:00:00Z").unwrap();

    assert!(matches!(
        workspace.active_task_keys(now),
        Err(WorkspaceQueryError::Store(StoreError::Diesel(_)))
    ));

    drop(workspace);
    std::fs::remove_file(database).unwrap();
}

#[test]
fn sqlite_queries_match_memory_with_and_without_an_open_overlay() {
    let target = "`- Target\n\n `+ task\n\n `@ target\n";
    let disk_source = concat!(
        "`- 2026-08-12T10:00|Later\n\n `+ event\n",
        "`- 2026-08-11T10:00|Earlier\n\n `+ event\n",
        "See `->[target|target.plumb#target].\n",
    );
    let open_source = concat!(
        "`- 2026-08-10T10:00|Open\n\n `+ event\n",
        "See `->[target|target.plumb#target].\n",
        "See `->[target|target.plumb#target].\n",
    );
    let ids = HashSet::from(["target".to_string()]);
    let start = DateTime::parse_from_rfc3339("2026-08-01T00:00:00+00:00").unwrap();
    let end = DateTime::parse_from_rfc3339("2026-09-01T00:00:00+00:00").unwrap();

    let mut memory = Workspace::new();
    memory.insert("target.plumb", 0, target);
    memory.insert("source.plumb", 0, disk_source);
    let store = SqliteSemanticStore::open_in_memory().unwrap();
    let mut sqlite = Workspace::with_sqlite_store(store);
    sqlite.insert_disk("target.plumb", 0, target).unwrap();
    sqlite.insert_disk("source.plumb", 0, disk_source).unwrap();

    let sqlite_references = sqlite
        .reverse_references_for_document("target.plumb", &ids)
        .unwrap();
    assert_eq!(sqlite_references.provenance, QueryProvenance::Persistent);
    assert_eq!(
        sqlite_references.value,
        memory
            .reverse_references_for_document("target.plumb", &ids)
            .unwrap()
            .value
    );
    assert_eq!(
        sqlite.events_overlapping(start, end).unwrap().value,
        memory.events_overlapping(start, end).unwrap().value
    );

    memory.insert("source.plumb", 1, open_source);
    sqlite.open_document("source.plumb", 1, open_source);
    let sqlite_references = sqlite
        .reverse_references_for_document("target.plumb", &ids)
        .unwrap();
    assert_eq!(
        sqlite_references.provenance,
        QueryProvenance::PersistentWithOverlay
    );
    assert_eq!(
        sqlite_references.value,
        memory
            .reverse_references_for_document("target.plumb", &ids)
            .unwrap()
            .value
    );
    assert_eq!(
        sqlite.events_overlapping(start, end).unwrap().value,
        memory.events_overlapping(start, end).unwrap().value
    );
}

#[test]
fn sqlite_event_task_relations_match_memory_and_obey_document_overlays() {
    let target_source = "`- Target\n\n `+ task\n\n `@ target\n";
    let event_source = "`- 2026-08-28T10:00|Linked `->[Target|tasks.plumb#target]\n\n `+ event\n";
    let target = TaskRef {
        path: PathBuf::from("tasks.plumb"),
        id: "target".to_string(),
    };

    let mut memory = Workspace::new();
    memory.insert("tasks.plumb", 0, target_source);
    memory.insert("events.plumb", 0, event_source);
    let store = SqliteSemanticStore::open_in_memory().unwrap();
    let mut sqlite = Workspace::with_sqlite_store(store);
    sqlite.insert_disk("tasks.plumb", 0, target_source).unwrap();
    sqlite.insert_disk("events.plumb", 0, event_source).unwrap();

    assert_eq!(
        sqlite.events_for_task(&target).unwrap().value,
        memory.events_for_task(&target).unwrap().value
    );
    let event = sqlite
        .events_page_after(None, 1)
        .unwrap()
        .value
        .pop()
        .unwrap();
    assert_eq!(
        sqlite
            .event_task_references(&event.path, &event.event)
            .unwrap()
            .value
            .len(),
        1
    );

    memory.insert("events.plumb", 1, "No events.\n");
    sqlite.open_document("events.plumb", 1, "No events.\n");
    assert_eq!(
        sqlite.events_for_task(&target).unwrap().value,
        memory.events_for_task(&target).unwrap().value
    );
    assert!(sqlite.events_for_task(&target).unwrap().value.is_empty());

    sqlite.close_document("events.plumb");
    sqlite.open_document(
        "tasks.plumb",
        1,
        "`- Replacement\n\n `+ task\n\n `@ replacement\n",
    );
    assert!(sqlite.events_for_task(&target).unwrap().value.is_empty());
}

#[test]
fn sqlite_active_task_keys_match_memory_and_replace_open_documents() {
    let now = DateTime::parse_from_rfc3339("2026-08-11T10:00:00+00:00").unwrap();
    let disk = concat!(
        "`- Ready\n\n `+ task\n\n `@ ready\n",
        "`- Waiting\n\n `+ task\n\n `@ waiting\n\n `= wait|2026-08-12T10:00:00Z\n",
        "`- Done\n\n `+ task\n\n `@ done\n\n `= done|2026-08-10T10:00:00Z\n",
    );
    let open = "`- Open replacement\n\n `+ task\n\n `@ replacement\n";

    let mut memory = Workspace::new();
    memory.insert("tasks.plumb", 0, disk);
    let store = SqliteSemanticStore::open_in_memory().unwrap();
    let mut sqlite = Workspace::with_sqlite_store(store);
    sqlite.insert_disk("tasks.plumb", 0, disk).unwrap();
    assert_eq!(
        sqlite.active_task_keys(now).unwrap().value,
        memory.active_task_keys(now).unwrap().value
    );

    memory.insert("tasks.plumb", 1, open);
    sqlite.open_document("tasks.plumb", 1, open);
    assert_eq!(
        sqlite.active_task_keys(now).unwrap().value,
        memory.active_task_keys(now).unwrap().value
    );
    assert_eq!(
        sqlite.active_task_keys(now).unwrap().value,
        [WorkspaceTaskKey {
            path: PathBuf::from("tasks.plumb"),
            start: 3,
        }]
    );
}

#[test]
fn sqlite_state_keys_recompute_disk_sources_against_open_targets() {
    let now = DateTime::parse_from_rfc3339("2026-08-11T10:00:00+00:00").unwrap();
    let source = "`- Source\n\n `+ task\n\n `@ source\n\n `= depends|target.plumb#target\n";
    let closed_target = "`- Target\n\n `+ task\n\n `@ target\n\n `= done|2026-08-10T10:00:00Z\n";
    let open_target = "`- Target\n\n `+ task\n\n `@ target\n";
    let store = SqliteSemanticStore::open_in_memory().unwrap();
    let mut workspace = Workspace::with_sqlite_store(store);
    workspace.insert_disk("source.plumb", 0, source).unwrap();
    workspace
        .insert_disk("target.plumb", 0, closed_target)
        .unwrap();
    workspace.open_document("target.plumb", 1, open_target);

    let blocked = HashSet::from([TaskWorkflowState::Blocked]);
    assert_eq!(
        workspace.task_keys_for_states(&blocked, now).unwrap().value,
        [WorkspaceTaskKey {
            path: PathBuf::from("source.plumb"),
            start: 3,
        }]
    );
}

fn temp_workspace() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "plumb-workspace-scan-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

#[test]
fn applies_one_guarded_document_edit_with_revision_validation() {
    let edit = WorkspaceEdit {
        document_changes: vec![DocumentEdit {
            path: PathBuf::from("note.plumb"),
            expected_revision: 7,
            edits: vec![TextEdit {
                range: 0..4,
                new_text: "Task".to_string(),
            }],
        }],
        resource_operations: Vec::new(),
    };
    assert_eq!(
        apply_document_edit("Note\n".to_string(), "note.plumb", 7, edit.clone()),
        Ok("Task\n".to_string())
    );
    assert_eq!(
        apply_document_edit("Note\n".to_string(), "note.plumb", 8, edit.clone()),
        Err(ApplyDocumentEditError::RevisionMismatch)
    );
    assert_eq!(
        apply_document_edit("Note\n".to_string(), "other.plumb", 7, edit),
        Err(ApplyDocumentEditError::DocumentNotEdited)
    );
}

#[test]
fn discovers_the_nearest_plumb_workspace_marker() {
    let root = temp_workspace();
    let nested = root.join("notes/private/deep");
    std::fs::create_dir_all(root.join(".plumb")).unwrap();
    std::fs::create_dir_all(root.join("notes/private/.plumb")).unwrap();
    std::fs::create_dir_all(&nested).unwrap();

    assert_eq!(
        discover_workspace_root(&nested),
        normalize(&root.join("notes/private"))
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn resolves_relative_explicit_workspace_roots_from_the_current_directory() {
    let root = temp_workspace();
    std::fs::create_dir_all(&root).unwrap();

    assert_eq!(
        resolve_workspace_root_from(Some(Path::new(".")), &root),
        normalize(&root)
    );
    assert_eq!(
        resolve_workspace_root_from(Some(Path::new("notes")), &root),
        normalize(&root.join("notes"))
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn scans_dot_directories_and_applies_only_workspace_ignore_files() {
    let parent = temp_workspace();
    let root = parent.join("workspace");
    std::fs::create_dir_all(root.join(".plumb")).unwrap();
    std::fs::create_dir_all(root.join(".hidden")).unwrap();
    std::fs::create_dir_all(root.join("private")).unwrap();
    std::fs::write(parent.join(".ignore"), "workspace/\n").unwrap();
    std::fs::write(root.join(".ignore"), "private/\n").unwrap();
    std::fs::write(root.join("visible.plumb"), "Visible\n").unwrap();
    std::fs::write(root.join(".hidden/note.plumb"), "Hidden\n").unwrap();
    std::fs::write(root.join("private/note.plumb"), "Private\n").unwrap();

    let scan = scan_workspace_files(&root);
    assert!(scan.is_complete(), "{:?}", scan.errors);
    assert_eq!(
        scan.files,
        vec![
            normalize(&root.join(".hidden/note.plumb")),
            normalize(&root.join("visible.plumb")),
        ]
    );

    std::fs::remove_dir_all(parent).unwrap();
}

fn apply_single_edit(source: &str, operation: &WorkspaceEdit) -> String {
    assert_eq!(operation.document_changes.len(), 1);
    assert_eq!(operation.document_changes[0].edits.len(), 1);
    let edit = &operation.document_changes[0].edits[0];
    let mut edited = source.to_string();
    edited.replace_range(edit.range.clone(), &edit.new_text);
    edited
}

#[test]
fn resolves_same_and_cross_file_explicit_anchors() {
    let mut workspace = Workspace::new();
    workspace.insert("notes/a note.plumb", 1, "`# Local\n  `@ local\n");
    workspace.insert("notes/a%20note.plumb", 1, "`# Literal\n  `@ literal\n");
    workspace.insert(
        "notes/b.plumb",
        1,
        "See `->[local|a note.plumb#local].\nSee `->[literal|a%20note.plumb#literal].\n",
    );
    let links = &workspace
        .get("notes/b.plumb")
        .unwrap()
        .current
        .as_ref()
        .unwrap()
        .output
        .links;
    for (link, expected_path, expected_id) in [
        (&links[0], "notes/a note.plumb", "local"),
        (&links[1], "notes/a%20note.plumb", "literal"),
    ] {
        assert!(matches!(
            workspace.resolve_link("notes/b.plumb", link).unwrap().value,
            ResolvedTarget::Anchor { ref path, ref id, .. }
                if path == Path::new(expected_path) && id == expected_id
        ));
    }
}

#[test]
fn headings_without_ids_do_not_resolve() {
    let mut workspace = Workspace::new();
    workspace.insert("a.plumb", 1, "`# No anchor\n\nSee `->[x|#No-anchor].\n");
    let entry = workspace.get("a.plumb").unwrap();
    let link = &entry.current.as_ref().unwrap().output.links[0];
    assert!(matches!(
        workspace.resolve_link("a.plumb", link).unwrap().value,
        ResolvedTarget::UnresolvedAnchor { .. }
    ));
}

#[test]
fn invalid_revision_keeps_but_does_not_publish_last_valid_output() {
    let mut workspace = Workspace::new();
    workspace.insert("a.plumb", 1, "`# Valid\n  `@ ok\n");
    let valid = workspace.get("a.plumb").unwrap();
    assert!(Arc::ptr_eq(
        valid.current.as_ref().unwrap(),
        valid.last_valid.as_ref().unwrap()
    ));
    workspace.insert("a.plumb", 2, "`broken[\n");
    let entry = workspace.get("a.plumb").unwrap();
    assert!(entry.current.is_none());
    assert_eq!(entry.last_valid.as_ref().unwrap().revision, 1);
    assert!(workspace.anchor_at("a.plumb", 0).is_none());
}

#[test]
fn installs_only_the_current_parsed_revision_analysis() {
    let mut workspace = Workspace::new();
    workspace.insert("a.plumb", 1, "`# Initial\n `@ initial\n");
    let stale = workspace
        .begin_document_revision("a.plumb", 2, "`# Stale\n `@ stale\n")
        .unwrap()
        .analyze();
    assert_eq!(
        workspace.document_paths().unwrap().completeness,
        QueryCompleteness::Partial
    );
    let current = workspace
        .begin_document_revision("a.plumb", 3, "`# Current\n `@ current\n")
        .unwrap()
        .analyze();
    assert!(!workspace.install_document_analysis(stale));
    assert!(workspace.install_document_analysis(current));
    assert_eq!(
        workspace.document_paths().unwrap().completeness,
        QueryCompleteness::Complete
    );
    assert_eq!(
        workspace
            .get("a.plumb")
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .revision,
        3
    );

    let same_version_old = workspace
        .begin_document_revision("a.plumb", 4, "`# Old bytes\n `@ old\n")
        .unwrap()
        .analyze();
    let same_version_new = workspace
        .begin_document_revision("a.plumb", 4, "`# New bytes\n `@ new\n")
        .unwrap()
        .analyze();
    assert!(!workspace.install_document_analysis(same_version_old));
    assert!(workspace.install_document_analysis(same_version_new));

    let closed = workspace
        .begin_document_revision("a.plumb", 5, "`# Closed\n `@ closed\n")
        .unwrap()
        .analyze();
    workspace.remove("a.plumb");
    assert!(!workspace.install_document_analysis(closed));
}

#[test]
fn pending_and_invalid_open_revisions_do_not_fall_back_to_disk_semantics() {
    let store = SqliteSemanticStore::open_in_memory().unwrap();
    let mut workspace = Workspace::with_sqlite_store(store);
    workspace
        .insert_disk("a.plumb", 1, "`# Disk\n `@ disk\n")
        .unwrap();
    let pending = workspace
        .begin_document_revision("a.plumb", 2, "`# Pending\n `@ pending\n")
        .unwrap();
    assert!(workspace
        .anchors_named(Path::new("a.plumb"), "disk")
        .unwrap()
        .is_empty());
    assert!(workspace.install_document_analysis(pending.analyze()));
    assert_eq!(
        workspace
            .anchors_named(Path::new("a.plumb"), "pending")
            .unwrap()
            .len(),
        1
    );

    assert!(workspace
        .begin_document_revision("a.plumb", 3, "`broken[\n")
        .is_none());
    assert_eq!(
        workspace.document_paths().unwrap().completeness,
        QueryCompleteness::Complete
    );
    assert!(workspace
        .anchors_named(Path::new("a.plumb"), "disk")
        .unwrap()
        .is_empty());
}

#[test]
fn completes_only_the_current_valid_pending_document_analysis() {
    let mut workspace = Workspace::new();
    workspace
        .begin_document_revision("pending.plumb", 2, "`# Current\n `@ current\n")
        .unwrap();
    workspace.begin_document_revision("invalid.plumb", 1, "`broken[\n");

    assert!(workspace.document_analysis_pending("pending.plumb"));
    assert!(!workspace.document_analysis_pending("invalid.plumb"));
    assert!(workspace.complete_pending_document_analysis("pending.plumb"));
    assert!(!workspace.document_analysis_pending("pending.plumb"));
    assert!(!workspace.complete_pending_document_analysis("pending.plumb"));
    assert_eq!(
        workspace
            .anchors_named(Path::new("pending.plumb"), "current")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn rebinds_identical_source_without_rebuilding_document_outputs() {
    let source = "`- 2026-08-11T09:00:00+08:00|Meeting\n\n `+ event\n";
    let mut workspace = Workspace::new();
    workspace.insert("event.plumb", 7, source);
    let entry = workspace.get("event.plumb").unwrap();
    let parsed = Arc::clone(&entry.parsed);
    let output = Arc::clone(&entry.current.as_ref().unwrap().output);
    let token_storage = entry.parsed.lossless.tokens.as_ptr();
    let event_storage = entry
        .current
        .as_ref()
        .unwrap()
        .output
        .events
        .events
        .as_ptr();

    assert!(workspace.rebind_revision_if_source("event.plumb", 0, source));
    let entry = workspace.get("event.plumb").unwrap();
    assert_eq!(entry.revision, 0);
    assert_eq!(entry.current.as_ref().unwrap().revision, 0);
    assert_eq!(entry.last_valid.as_ref().unwrap().revision, 0);
    assert!(Arc::ptr_eq(&entry.parsed, &parsed));
    assert!(Arc::ptr_eq(
        &entry.current.as_ref().unwrap().output,
        &output
    ));
    assert_eq!(entry.parsed.lossless.tokens.as_ptr(), token_storage);
    assert_eq!(
        entry
            .current
            .as_ref()
            .unwrap()
            .output
            .events
            .events
            .as_ptr(),
        event_storage
    );
    assert!(!workspace.rebind_revision_if_source("event.plumb", 1, "changed\n"));
}

#[test]
fn cloned_workspaces_share_immutable_document_payloads() {
    let mut workspace = Workspace::new();
    workspace.insert("note.plumb", 1, "`# Note\n");
    let cloned = workspace.clone();
    let original = workspace.get("note.plumb").unwrap();
    let clone = cloned.get("note.plumb").unwrap();

    assert!(Arc::ptr_eq(&original.parsed, &clone.parsed));
    assert!(Arc::ptr_eq(
        &original.current.as_ref().unwrap().output,
        &clone.current.as_ref().unwrap().output
    ));

    workspace.insert("note.plumb", 2, "`# Changed\n");
    let changed = workspace.get("note.plumb").unwrap();
    assert!(!Arc::ptr_eq(&changed.parsed, &clone.parsed));
    assert!(!Arc::ptr_eq(
        &changed.current.as_ref().unwrap().output,
        &clone.current.as_ref().unwrap().output
    ));
}

#[test]
fn materializes_only_the_matching_persistent_generation() {
    let store = SqliteSemanticStore::open_in_memory().unwrap();
    let mut workspace = Workspace::with_sqlite_store(store);
    let source = "`# Stored\n";
    workspace.insert_disk("note.plumb", 7, source).unwrap();

    let entry = workspace
        .document_from_source("note.plumb", source)
        .unwrap()
        .unwrap();
    assert_eq!(entry.revision, 7);
    assert_eq!(entry.parsed.source, source);
    assert!(entry.current.is_some());
    assert!(workspace
        .document_from_source("note.plumb", "`# Changed\n")
        .unwrap()
        .is_none());
}

#[test]
fn rebinding_invalid_source_preserves_last_valid_provenance() {
    let mut workspace = Workspace::new();
    workspace.insert("note.plumb", 1, "Valid\n");
    let invalid = "`broken[\n";
    workspace.insert("note.plumb", 2, invalid);

    assert!(workspace.rebind_revision_if_source("note.plumb", 0, invalid));
    let entry = workspace.get("note.plumb").unwrap();
    assert_eq!(entry.revision, 0);
    assert!(entry.current.is_none());
    assert_eq!(entry.last_valid.as_ref().unwrap().revision, 1);
}

#[test]
fn returns_reverse_references() {
    let mut workspace = Workspace::new();
    workspace.insert("a.plumb", 1, "`# Target\n  `@ target\n");
    workspace.insert("b.plumb", 1, "`->[x|a.plumb#target]\n");
    workspace.insert("missing.plumb", 1, "`->[x|a.plumb#missing]\n");
    workspace.insert(
        "task.plumb",
        1,
        "`- Task\n\n `+ task\n\n `= depends|a.plumb#missing\n",
    );
    workspace.insert("document.plumb", 1, "`->[a|a.plumb]\n");
    workspace.insert(
        "a-local.plumb",
        1,
        "`# Local\n  `@ local\n\n`->[x|#local]\n",
    );
    assert_eq!(
        workspace
            .references_to("a.plumb", "target")
            .unwrap()
            .value
            .len(),
        1
    );
    let document_references = workspace.references_to_document("a.plumb").unwrap().value;
    assert_eq!(document_references.len(), 4);
    assert_eq!(
        document_references
            .iter()
            .map(|(path, _)| path.to_path_buf())
            .collect::<Vec<_>>(),
        vec![
            PathBuf::from("b.plumb"),
            PathBuf::from("document.plumb"),
            PathBuf::from("missing.plumb"),
            PathBuf::from("task.plumb"),
        ]
    );
    assert_eq!(
        workspace
            .references_to_document("a-local.plumb")
            .unwrap()
            .value
            .len(),
        1
    );
    let batched = workspace
        .reverse_references_for_document("a.plumb", &HashSet::from(["target".to_string()]))
        .unwrap()
        .value;
    assert_eq!(batched.document.len(), document_references.len());
    assert_eq!(batched.anchors["target"].len(), 1);
    assert_eq!(
        batched
            .document
            .iter()
            .map(|reference| reference.source_path.clone())
            .collect::<Vec<_>>(),
        document_references
            .iter()
            .map(|(path, _)| path.to_path_buf())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        workspace
            .referenced_documents_from("missing.plumb")
            .unwrap()
            .value,
        vec![PathBuf::from("a.plumb")]
    );
    assert_eq!(
        workspace
            .referenced_documents_from("task.plumb")
            .unwrap()
            .value,
        vec![PathBuf::from("a.plumb")]
    );
}

#[test]
fn batches_document_and_multiple_anchor_reverse_references() {
    let mut workspace = Workspace::new();
    workspace.insert("target.plumb", 1, "`# One\n  `@ one\n\n`# Two\n  `@ two\n");
    workspace.insert(
        "source.plumb",
        1,
        "`->[one|target.plumb#one] and `->[two|target.plumb#two]\n",
    );

    let references = workspace
        .reverse_references_for_document(
            "target.plumb",
            &HashSet::from(["one".to_string(), "two".to_string()]),
        )
        .unwrap()
        .value;
    assert_eq!(references.document.len(), 2);
    assert_eq!(references.anchors["one"].len(), 1);
    assert_eq!(references.anchors["two"].len(), 1);
    assert!(references
        .document
        .iter()
        .all(|reference| reference.source_path == Path::new("source.plumb")));
}

#[test]
fn resolves_document_and_anchor_targets_from_declarations_and_reference_components() {
    let target_source = "`= title|Target\n\n`# Section\n  `@ section\n";
    let reference_source = "See `->[named|target.plumb#section] and `->\"target.plumb#section\".\n\n`- Review\n\n `+ task\n\n `= prev|target.plumb#section\n `= depends|target.plumb#section\n";
    let mut workspace = Workspace::new();
    workspace.insert("target.plumb", 1, target_source);
    workspace.insert("reference.plumb", 1, reference_source);

    assert!(matches!(
        workspace.target_at("target.plumb", 0).unwrap().value,
        Some(ResolvedTarget::Document { path }) if path == Path::new("target.plumb")
    ));
    assert!(workspace
        .target_at("target.plumb", target_source.find("Target").unwrap())
        .unwrap()
        .value
        .is_none());
    assert!(matches!(
        workspace
            .target_at("target.plumb", target_source.find("section").unwrap())
            .unwrap()
            .value,
        Some(ResolvedTarget::Anchor { path, id, .. })
            if path == Path::new("target.plumb") && id == "section"
    ));

    for path_offset in reference_source
        .match_indices("target.plumb")
        .map(|(offset, _)| offset)
    {
        assert!(matches!(
            workspace.target_at("reference.plumb", path_offset).unwrap().value,
            Some(ResolvedTarget::Document { path })
                if path == Path::new("target.plumb")
        ));
    }
    for fragment_offset in reference_source
        .match_indices("#section")
        .map(|(offset, _)| offset + 1)
    {
        assert!(matches!(
            workspace
                .target_at("reference.plumb", fragment_offset)
                .unwrap()
                .value,
            Some(ResolvedTarget::Anchor { path, id, .. })
                if path == Path::new("target.plumb") && id == "section"
        ));
    }
    let separator_offset = reference_source.find("#section").unwrap();
    assert!(matches!(
        workspace
            .target_at("reference.plumb", separator_offset)
            .unwrap()
            .value,
        Some(ResolvedTarget::Anchor { id, .. }) if id == "section"
    ));
    assert!(matches!(
        workspace
            .target_at("reference.plumb", reference_source.find("named").unwrap())
            .unwrap()
            .value,
        Some(ResolvedTarget::Anchor { id, .. }) if id == "section"
    ));

    let lonely_source = "`= title\n\n Lonely\n";
    workspace.insert("lonely.plumb", 1, lonely_source);
    assert!(matches!(
        workspace.target_at("lonely.plumb", 0).unwrap().value,
        Some(ResolvedTarget::Document { path }) if path == Path::new("lonely.plumb")
    ));
    assert!(workspace
        .references_to_document("lonely.plumb")
        .unwrap()
        .value
        .is_empty());

    workspace.insert("target.plumb", 2, "`broken[\n");
    assert!(workspace
        .target_at("target.plumb", 1)
        .unwrap()
        .value
        .is_none());
}

#[test]
fn document_metadata_targets_only_top_level_entry_subtrees_and_offset_zero() {
    let source = "`= title|Document\n\n`note Body\n `= nested|ordinary property\n\n`= tags\n `+ plumb\n `+ notes\n";
    let mut workspace = Workspace::new();
    workspace.insert("metadata.plumb", 1, source);
    let entry = workspace.get("metadata.plumb").unwrap();
    let first = entry.parsed.syntax.blocks[0].range().clone();
    let second = entry.parsed.syntax.blocks[2].range().clone();

    assert_eq!(
        workspace
            .document_metadata_target_at("metadata.plumb", 0)
            .unwrap()
            .range,
        first
    );
    assert_eq!(
        workspace
            .document_metadata_target_at("metadata.plumb", source.find("plumb").unwrap())
            .unwrap()
            .range,
        second
    );
    assert!(workspace
        .document_metadata_target_at("metadata.plumb", source.find("Body").unwrap())
        .is_none());
    assert!(workspace
        .document_metadata_target_at("metadata.plumb", source.find("nested").unwrap())
        .is_none());

    let body_first = "Body first.\n\n`= title|Later\n";
    workspace.insert("body-first.plumb", 2, body_first);
    assert_eq!(
        workspace
            .document_metadata_target_at("body-first.plumb", 0)
            .unwrap()
            .range,
        0..0
    );
}

#[test]
fn task_fields_participate_in_navigation_references_and_anchor_rename() {
    let target_source = "`- Draft\n\n `+ task\n\n `@ draft\n\n`node Note\n  `@ note\n";
    let reference_source = "`- Review\n\n `+ task\n\n `@ review\n\n `= prev|Project Plan.plumb#draft\n `= depends|Project Plan.plumb#draft Project Plan.plumb#note Project%20Plan.plumb#literal\n\nSee `->[draft|Project Plan.plumb#draft].\n";
    let mut workspace = Workspace::new();
    workspace.insert("Project Plan.plumb", 4, target_source);
    workspace.insert("Project%20Plan.plumb", 4, "`node Literal\n  `@ literal\n");
    workspace.insert("review.plumb", 7, reference_source);

    let depends_attribute = reference_source.find("`= depends").unwrap();
    let depends = depends_attribute
        + reference_source[depends_attribute..]
            .find("#draft")
            .unwrap()
        + 1;
    let reference = workspace
        .anchor_reference_at("review.plumb", depends)
        .unwrap()
        .value
        .unwrap();
    assert_eq!(reference.target_path, PathBuf::from("Project Plan.plumb"));
    assert_eq!(reference.target_id, "draft");
    assert_eq!(
        workspace
            .references_to("Project Plan.plumb", "draft")
            .unwrap()
            .value
            .len(),
        3
    );

    let note = reference_source.find("#note").unwrap() + 1;
    assert_eq!(
        workspace
            .anchor_reference_at("review.plumb", note)
            .unwrap()
            .value
            .unwrap()
            .target_id,
        "note"
    );

    let literal = reference_source.find("#literal").unwrap() + 1;
    assert_eq!(
        workspace
            .anchor_reference_at("review.plumb", literal)
            .unwrap()
            .value
            .unwrap()
            .target_path,
        PathBuf::from("Project%20Plan.plumb")
    );

    let target = workspace
        .anchor_rename_target_at("review.plumb", depends)
        .unwrap();
    let edit = workspace.rename_anchor(&target, "first-draft").unwrap();
    assert_eq!(edit.document_changes.len(), 2);
    assert_eq!(
        edit.document_changes
            .iter()
            .flat_map(|document| &document.edits)
            .filter(|edit| edit.new_text == "first-draft")
            .count(),
        4
    );
}

#[test]
fn document_rename_rewrites_raw_task_reference_paths() {
    let target_source = "`- Draft\n\n `+ task\n\n `@ draft\n";
    let reference_source = "`- Review\n\n `+ task\n\n `= prev|Project Plan.plumb#draft\n `= depends|Project Plan.plumb#draft\n\nSee `->[draft|Project Plan.plumb#draft].\n";
    let mut workspace = Workspace::new();
    workspace.insert("Project Plan.plumb", 4, target_source);
    workspace.insert("review.plumb", 7, reference_source);

    let path_offset = reference_source.find("Project Plan.plumb").unwrap();
    let target = workspace
        .path_rename_target_at("review.plumb", path_offset)
        .unwrap();
    let edit = workspace
        .rename_document(&target, "Archived Plan.plumb")
        .unwrap();
    let reference_edits = &edit
        .document_changes
        .iter()
        .find(|document| document.path == Path::new("review.plumb"))
        .unwrap()
        .edits;
    assert_eq!(
        reference_edits
            .iter()
            .filter(|edit| edit.new_text == "Archived Plan.plumb")
            .count(),
        3
    );
    assert_eq!(
        edit.resource_operations,
        vec![ResourceOperation::Rename {
            old_path: PathBuf::from("Project Plan.plumb"),
            new_path: PathBuf::from("Archived Plan.plumb"),
        }]
    );
}

#[test]
fn document_start_targets_the_current_document_without_editing_title() {
    let source = "`= title|Stable title\n";
    let mut workspace = Workspace::new();
    workspace.insert("current.plumb", 4, source);
    workspace.insert("incoming.plumb", 7, "`->[current|current.plumb]\n");

    let target = workspace
        .document_rename_target_at("current.plumb", 0)
        .unwrap();
    assert_eq!(target.old_path, Path::new("current.plumb"));
    assert_eq!(target.range, 0..0);
    assert_eq!(&source[target.range.clone()], "");
    assert_eq!(target.input, PathRenameInput::FileStem);
    assert!(matches!(
        workspace.rename_document(&target, "archive/renamed"),
        Err(WorkspaceOperationError::Operation(RenameError::InvalidPath))
    ));
    assert!(matches!(
        workspace.rename_document(&target, "renamed.md"),
        Err(WorkspaceOperationError::Operation(RenameError::InvalidPath))
    ));

    let edit = workspace.rename_document(&target, "renamed").unwrap();
    assert!(edit
        .document_changes
        .iter()
        .all(|document| document.path != Path::new("current.plumb")));
    assert_eq!(edit.document_changes[0].edits[0].new_text, "renamed.plumb");
    assert_eq!(
        edit.resource_operations,
        vec![ResourceOperation::Rename {
            old_path: PathBuf::from("current.plumb"),
            new_path: PathBuf::from("renamed.plumb"),
        }]
    );
}

#[test]
fn rename_updates_declaration_and_cross_file_fragments() {
    let mut workspace = Workspace::new();
    workspace.insert("a.plumb", 4, "`# Target\n  `@ target\n");
    workspace.insert("b.plumb", 7, "`->[x|a.plumb#target]\n");
    let target = workspace
        .anchor_rename_target_at(
            "a.plumb",
            workspace
                .get("a.plumb")
                .unwrap()
                .parsed
                .source
                .find("target")
                .unwrap(),
        )
        .unwrap();
    let edit = workspace.rename_anchor(&target, "renamed").unwrap();
    assert_eq!(edit.document_changes.len(), 2);
    assert_eq!(edit.document_changes[0].expected_revision, 4);
    assert_eq!(edit.document_changes[1].expected_revision, 7);
    assert!(edit
        .document_changes
        .iter()
        .flat_map(|document| &document.edits)
        .all(|edit| edit.new_text == "renamed"));
}

#[test]
fn completes_event_titles_by_workspace_frequency_and_prefix() {
    let mut workspace = Workspace::new();
    workspace.insert(
            "one.plumb",
            1,
            "`= date|2026-08-13\n`= timezone|+08:00\n\n`- 09:00|relax\n\n `+ event\n`- 10:00|relax\n\n `+ event\n`- 11:00|research\n\n `+ event\n",
        );
    workspace.insert(
            "two.plumb",
            1,
            "`= date|2026-08-13\n`= timezone|+08:00\n\n`- 12:00|research\n\n `+ event\n`- 13:00|read\n\n `+ event\n",
        );
    let candidates = workspace
        .complete_event_title(&EventTitleCompletionContext {
            replace: 12..14,
            query: "re".to_string(),
        })
        .unwrap()
        .value;
    assert_eq!(
        candidates
            .iter()
            .map(|candidate| (candidate.label.as_str(), candidate.detail.as_str()))
            .collect::<Vec<_>>(),
        [
            ("relax", "event title, 2 uses"),
            ("research", "event title, 2 uses"),
            ("read", "event title, 1 uses"),
        ]
    );
    assert!(candidates
        .iter()
        .all(|candidate| candidate.replace == (12..14)));
}

#[test]
fn event_title_completion_uses_open_overlay_and_limits_results() {
    let store = SqliteSemanticStore::open_in_memory().unwrap();
    let mut workspace = Workspace::with_sqlite_store(store);
    workspace
        .insert_disk("agenda.plumb", 1, "`- 09:00|stale\n\n `+ event\n")
        .unwrap();
    workspace.open_document("agenda.plumb", 2, "`- 09:00|current\n\n `+ event\n");
    for index in 0..55 {
        workspace
            .insert_disk(
                format!("event-{index}.plumb"),
                1,
                format!("`- 09:00|title-{index:02}\n\n `+ event\n"),
            )
            .unwrap();
    }
    let candidates = workspace
        .complete_event_title(&EventTitleCompletionContext {
            replace: 0..0,
            query: String::new(),
        })
        .unwrap()
        .value;
    assert_eq!(candidates.len(), EVENT_TITLE_COMPLETION_LIMIT);
    assert!(candidates
        .iter()
        .any(|candidate| candidate.label == "current"));
    assert!(!candidates
        .iter()
        .any(|candidate| candidate.label == "stale"));
}

#[test]
fn rename_rejects_pair_style_or_invalid_ids() {
    let mut workspace = Workspace::new();
    workspace.insert("a.plumb", 1, "`# Not an anchor\n  `= id|pair\n");
    assert!(matches!(
        workspace.anchor_rename_target_at("a.plumb", 6),
        Err(WorkspaceOperationError::Operation(
            RenameError::NotRenameable
        ))
    ));
    workspace.insert("a.plumb", 2, "`# Anchor\n  `@ real\n");
    let target = workspace
        .anchor_rename_target_at(
            "a.plumb",
            workspace
                .get("a.plumb")
                .unwrap()
                .parsed
                .source
                .find("real")
                .unwrap(),
        )
        .unwrap();
    assert!(matches!(
        workspace.rename_anchor(&target, "has space"),
        Err(WorkspaceOperationError::Operation(RenameError::InvalidId))
    ));
}

#[test]
fn completes_paths_and_only_explicit_anchors() {
    let mut workspace = Workspace::new();
    let autolink_path =
        |replace: std::ops::Range<usize>, query: &str| LinkCompletionContext::AutolinkPath {
            envelope: replace.clone(),
            replace,
            quote_count: 0,
            suffix: String::new(),
            query: query.to_string(),
        };
    workspace.insert("notes/current.plumb", 1, "Current\n");
    workspace.insert(
        "notes/design.plumb",
        1,
        "`= title|Design Guide\n\n`# No id\n\n`## API\n  `@ api\n",
    );
    workspace.insert(
        "notes/Project Plan.plumb",
        1,
        "`= title|Project Plan\n\n`# Roadmap\n  `@ roadmap\n",
    );
    workspace.insert("notes/中文笔记.plumb", 1, "`# 中文内容\n  `@ 内容\n");
    workspace.insert("notes/方案 (草稿).plumb", 1, "`# 草稿\n");
    workspace.insert("notes/方案]终稿.plumb", 1, "`# 终稿\n");
    workspace.insert("notes/brace{draft}].plumb", 1, "`# Braces\n");
    workspace.insert("notes/quote\"name.plumb", 1, "`# Quote\n");
    let paths = workspace
        .complete_link("notes/current.plumb", &autolink_path(10..13, "guide"))
        .unwrap()
        .value;
    assert_eq!(paths[0].label, "design.plumb");
    assert_eq!(paths[0].detail, "Design Guide");
    assert_eq!(paths[0].new_text, "design.plumb");
    let labels = workspace
        .complete_link(
            "notes/current.plumb",
            &LinkCompletionContext::Label {
                replace: 0..8,
                query: "guide".to_string(),
            },
        )
        .unwrap()
        .value;
    assert_eq!(labels[0].label, "Design Guide");
    assert_eq!(labels[0].detail, "design.plumb");
    assert_eq!(labels[0].new_text, "`->[Design Guide|design.plumb]");
    let spaced_label = workspace
        .complete_link(
            "notes/current.plumb",
            &LinkCompletionContext::Label {
                replace: 0..0,
                query: "project".to_string(),
            },
        )
        .unwrap()
        .value;
    assert_eq!(
        spaced_label[0].new_text,
        "`->[Project Plan|Project Plan.plumb]"
    );
    let spaced_path = workspace
        .complete_link(
            "notes/current.plumb",
            &LinkCompletionContext::Path {
                replace: 0..0,
                query: "project".to_string(),
                parsed: true,
            },
        )
        .unwrap()
        .value;
    assert_eq!(spaced_path[0].new_text, "Project Plan.plumb");
    let quote_path = workspace
        .complete_link(
            "notes/current.plumb",
            &LinkCompletionContext::Path {
                replace: 0..0,
                query: "quote".to_string(),
                parsed: true,
            },
        )
        .unwrap()
        .value;
    assert_eq!(quote_path[0].label, "quote\"name.plumb");
    assert_eq!(quote_path[0].new_text, "quote\"name.plumb");
    let spaced_autolink = workspace
        .complete_link("notes/current.plumb", &autolink_path(0..0, "project"))
        .unwrap()
        .value;
    assert_eq!(spaced_autolink[0].label, "Project Plan.plumb");
    assert_eq!(spaced_autolink[0].new_text, "Project Plan.plumb");
    let unicode = workspace
        .complete_link("notes/current.plumb", &autolink_path(0..0, "中文"))
        .unwrap()
        .value;
    assert_eq!(unicode[0].label, "中文笔记.plumb");
    assert_eq!(unicode[0].new_text, "中文笔记.plumb");
    let parentheses = workspace
        .complete_link("notes/current.plumb", &autolink_path(0..0, "草稿"))
        .unwrap()
        .value;
    assert_eq!(parentheses[0].label, "方案 (草稿).plumb");
    assert_eq!(parentheses[0].new_text, "方案 (草稿).plumb");
    let closing_bracket = workspace
        .complete_link(
            "notes/current.plumb",
            &LinkCompletionContext::AutolinkPath {
                replace: 2..3,
                envelope: 0..5,
                quote_count: 0,
                suffix: String::new(),
                query: "终稿".to_string(),
            },
        )
        .unwrap()
        .value;
    assert_eq!(closing_bracket[0].label, "方案]终稿.plumb");
    assert_eq!(closing_bracket[0].new_text, "`\"[方案]终稿.plumb]\"");
    assert_eq!(closing_bracket[0].replace, 0..5);
    let structural_delimiters = workspace
        .complete_link(
            "notes/current.plumb",
            &LinkCompletionContext::Label {
                replace: 0..0,
                query: "brace".to_string(),
            },
        )
        .unwrap()
        .value;
    assert_eq!(
        structural_delimiters[0].new_text,
        "`->[brace{draft}`].plumb|brace{draft}`].plumb]"
    );
    assert!(parse(&structural_delimiters[0].new_text).is_valid());
    let spaced_anchor = workspace
        .complete_link(
            "notes/current.plumb",
            &LinkCompletionContext::AutolinkAnchor {
                path: "Project Plan.plumb".to_string(),
                replace: 0..0,
                query: "road".to_string(),
            },
        )
        .unwrap()
        .value;
    assert_eq!(spaced_anchor[0].new_text, "roadmap");
    let anchors = workspace
        .complete_link(
            "notes/current.plumb",
            &LinkCompletionContext::Anchor {
                path: "design.plumb".to_string(),
                replace: 20..20,
                query: String::new(),
            },
        )
        .unwrap()
        .value;
    assert_eq!(anchors.len(), 1);
    assert_eq!(anchors[0].new_text, "api");
}

#[test]
fn completes_and_resolves_relative_image_files() {
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "plumb-image-completion-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let static_dir = root.join("static");
    std::fs::create_dir_all(static_dir.join("nested")).unwrap();
    std::fs::write(static_dir.join("image one.PNG"), b"png").unwrap();
    std::fs::write(static_dir.join("图 像(100%).PNG"), b"png").unwrap();
    std::fs::write(static_dir.join("literal%20name.PNG"), b"png").unwrap();
    std::fs::write(static_dir.join("quote\"image.PNG"), b"png").unwrap();
    std::fs::write(static_dir.join("closing]image.PNG"), b"png").unwrap();
    std::fs::write(static_dir.join("pipe|image.PNG"), b"png").unwrap();
    std::fs::write(static_dir.join("tick`image.PNG"), b"png").unwrap();
    std::fs::write(static_dir.join("literal%20name.txt"), b"text").unwrap();
    std::fs::write(static_dir.join("ignored.txt"), b"text").unwrap();
    let source_path = root.join("current.plumb");
    let source = "`->\"static/image one.PNG\"\n`img[Result|=[src|static/image one.PNG]]\n`img[Literal percent|=[src|static/literal%20name.PNG]]\n`->\"static/literal%20name.txt\"\n";
    let mut workspace = Workspace::new();
    workspace.insert(&source_path, 3, source);

    let candidates = workspace.complete_image_path(
        &source_path,
        &ImageCompletionContext {
            replace: 18..25,
            query: "static/im".to_string(),
        },
    );
    let image_with_space = candidates
        .iter()
        .find(|candidate| candidate.label == "static/image one.PNG")
        .unwrap();
    assert_eq!(image_with_space.new_text, "static/image one.PNG");
    assert_eq!(image_with_space.detail, "image file");
    assert_eq!(image_with_space.replace, 18..25);

    let unicode = workspace.complete_image_path(
        &source_path,
        &ImageCompletionContext {
            replace: 0..0,
            query: "static/图".to_string(),
        },
    );
    assert_eq!(unicode.len(), 1);
    assert_eq!(unicode[0].label, "static/图 像(100%).PNG");
    assert_eq!(unicode[0].new_text, "static/图 像(100%).PNG");

    let quoted = workspace.complete_image_path(
        &source_path,
        &ImageCompletionContext {
            replace: 0..0,
            query: "static/quote".to_string(),
        },
    );
    assert_eq!(quoted.len(), 1);
    assert_eq!(quoted[0].label, "static/quote\"image.PNG");
    assert_eq!(quoted[0].new_text, "static/quote\"image.PNG");

    for (query, expected) in [
        ("closing", "static/closing`]image.PNG"),
        ("pipe", "static/pipe`|image.PNG"),
        ("tick", "static/tick``image.PNG"),
    ] {
        let candidate = workspace
            .complete_image_path(
                &source_path,
                &ImageCompletionContext {
                    replace: 0..0,
                    query: format!("static/{query}"),
                },
            )
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(candidate.new_text, expected);
        let completed = format!("`img[alt|=[src|{}]]\n", candidate.new_text);
        let parsed = parse(&completed);
        assert!(parsed.is_valid(), "{completed}\n{:?}", parsed.diagnostics);
        assert_eq!(
            analyze_document(
                parsed
                    .valid_syntax()
                    .expect("semantic analysis requires valid syntax")
            )
            .images[0]
                .source
                .value,
            candidate.label
        );
    }

    let directories = workspace.complete_image_path(
        &source_path,
        &ImageCompletionContext {
            replace: 0..0,
            query: "static/ne".to_string(),
        },
    );
    assert!(directories
        .iter()
        .any(|candidate| candidate.new_text == "static/nested/"));

    let link = workspace
        .link_at(&source_path, source.find("image one").unwrap())
        .unwrap();
    assert_eq!(
        workspace.resolve_link(&source_path, link).unwrap().value,
        ResolvedTarget::File {
            path: static_dir.join("image one.PNG")
        }
    );
    let literal_percent = workspace
        .link_at(&source_path, source.rfind("literal%20name").unwrap())
        .unwrap();
    assert_eq!(
        workspace
            .resolve_link(&source_path, literal_percent)
            .unwrap()
            .value,
        ResolvedTarget::File {
            path: static_dir.join("literal%20name.txt")
        }
    );
    let image = workspace
        .image_at(&source_path, source.find("Result").unwrap())
        .unwrap();
    assert_eq!(
        workspace.resolve_image(&source_path, image),
        ResolvedTarget::File {
            path: static_dir.join("image one.PNG")
        }
    );
    let literal_percent_image = workspace
        .image_at(&source_path, source.find("Literal percent").unwrap())
        .unwrap();
    assert_eq!(
        workspace.resolve_image(&source_path, literal_percent_image),
        ResolvedTarget::File {
            path: static_dir.join("literal%20name.PNG")
        }
    );
    assert!(workspace
        .diagnostics(&source_path)
        .unwrap()
        .value
        .is_empty());

    std::fs::remove_file(static_dir.join("image one.PNG")).unwrap();
    std::fs::remove_file(static_dir.join("图 像(100%).PNG")).unwrap();
    std::fs::remove_file(static_dir.join("literal%20name.PNG")).unwrap();
    std::fs::remove_file(static_dir.join("quote\"image.PNG")).unwrap();
    std::fs::remove_file(static_dir.join("literal%20name.txt")).unwrap();
    let unresolved = workspace
        .diagnostics(&source_path)
        .unwrap()
        .value
        .into_iter()
        .find(|diagnostic| diagnostic.code == "image.unresolved-file")
        .unwrap();
    assert!(unresolved
        .message
        .contains(&static_dir.join("image one.PNG").display().to_string()));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn resolves_file_attachments_and_reports_missing_targets() {
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "plumb-file-resolution-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join("static")).unwrap();
    std::fs::write(root.join("static/demo.mp4"), b"video").unwrap();
    std::fs::write(root.join("static/manual.pdf"), b"pdf").unwrap();
    let source_path = root.join("note.plumb");
    let source = "`file[Demo|=[src|static/demo.mp4]]\n`file[Missing|=[src|static/missing.pdf]]\n";
    let mut workspace = Workspace::new();
    workspace.insert(&source_path, 1, source);

    let file = workspace
        .file_at(&source_path, source.find("Demo").unwrap())
        .unwrap();
    assert_eq!(
        workspace.resolve_file(&source_path, file),
        ResolvedTarget::File {
            path: root.join("static/demo.mp4")
        }
    );
    assert_eq!(
        workspace
            .target_at(&source_path, source.find("demo.mp4").unwrap())
            .unwrap()
            .value,
        Some(ResolvedTarget::File {
            path: root.join("static/demo.mp4")
        })
    );
    let completions = workspace.complete_file_path(
        &source_path,
        &FileCompletionContext {
            replace: 0..0,
            query: "static/ma".to_string(),
        },
    );
    assert_eq!(completions.len(), 1);
    assert_eq!(completions[0].new_text, "static/manual.pdf");
    assert_eq!(completions[0].detail, "file attachment");
    let diagnostics = workspace.diagnostics(&source_path).unwrap().value;
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == "file.unresolved-file")
            .count(),
        1
    );
    assert!(diagnostics.iter().any(|diagnostic| diagnostic
        .message
        .contains(&root.join("static/missing.pdf").display().to_string())));

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_note_and_task_records_with_stable_fuzzy_results() {
    let root = Path::new("notes");
    let now = DateTime::parse_from_rfc3339("2026-07-22T12:00:00+08:00").unwrap();
    let mut workspace = Workspace::new();
    workspace.insert(
            "notes/design.plumb",
            4,
            "`= title|Design Guide\n\n`- Review parser\n\n `+ task\n\n `@ review\n\n `= due|2026-07-23T12:00:00+08:00\n",
        );
    workspace.insert("notes/fallback.plumb", 2, "Fallback body\n");

    let notes = workspace
        .search_records(root, Some(SearchRecordKind::Note), "dsg", 20, now)
        .unwrap()
        .value;
    assert!(notes.complete);
    assert_eq!(notes.items.len(), 1);
    assert_eq!(notes.items[0].title, "Design Guide");
    assert_eq!(notes.items[0].relative_path, "design.plumb");
    assert_eq!(notes.items[0].revision, 4);

    let tasks = workspace
        .search_records(root, Some(SearchRecordKind::Task), "review", 20, now)
        .unwrap()
        .value;
    assert_eq!(tasks.items.len(), 1);
    assert_eq!(tasks.items[0].id.as_deref(), Some("review"));
    assert_eq!(tasks.items[0].task_state, Some(TaskWorkflowState::Ready));
    assert_eq!(tasks.items[0].wait_reasons, Some(Vec::new()));
    assert_eq!(tasks.items[0].blocked, Some(false));
    assert_eq!(tasks.items[0].actionable, Some(true));

    let fallback = workspace
        .search_records(root, Some(SearchRecordKind::Note), "fallback", 20, now)
        .unwrap()
        .value;
    assert_eq!(fallback.items[0].title, "fallback");
}

#[test]
fn derives_mutually_exclusive_task_workflow_states_for_search_and_cel() {
    let root = Path::new("notes");
    let now = DateTime::parse_from_rfc3339("2026-07-22T12:00:00+08:00").unwrap();
    let mut workspace = Workspace::new();
    workspace.insert(
            "notes/tasks.plumb",
            1,
            "`- Blocker\n\n `+ task\n\n `@ blocker\n`- Ready\n\n `+ task\n\n `@ ready\n\n `= priority|7\n`- Time wait\n\n `+ task\n\n `@ time\n\n `= wait|2026-07-23T12:00:00+08:00\n`- Dependency blocked\n\n `+ task\n\n `@ dependency\n\n `= depends|#blocker\n`- Both reasons\n\n `+ task\n\n `@ both\n\n `= wait|2026-07-23T12:00:00+08:00\n `= depends|#blocker\n`- Done\n\n `+ task\n\n `@ done\n\n `= done|2026-07-21T12:00:00+08:00\n`- Canceled\n\n `+ task\n\n `@ canceled\n\n `= canceled|2026-07-21T12:00:00+08:00\n`- Conflicted\n\n `+ task\n\n `@ conflicted\n\n `= done|2026-07-21T12:00:00+08:00\n `= canceled|2026-07-21T13:00:00+08:00\n",
        );

    let results = workspace
        .search_records(root, Some(SearchRecordKind::Task), "", 20, now)
        .unwrap()
        .value;
    let by_id = |id: &str| {
        results
            .items
            .iter()
            .find(|record| record.id.as_deref() == Some(id))
            .unwrap()
    };
    assert_eq!(by_id("ready").task_state, Some(TaskWorkflowState::Ready));
    assert_eq!(by_id("ready").priority, Some(7));
    assert_eq!(by_id("time").task_state, Some(TaskWorkflowState::Waiting));
    assert_eq!(by_id("time").wait_reasons, Some(vec![TaskWaitReason::Time]));
    assert_eq!(
        by_id("dependency").task_state,
        Some(TaskWorkflowState::Blocked)
    );
    assert_eq!(
        by_id("dependency").wait_reasons,
        Some(vec![TaskWaitReason::Dependency])
    );
    assert_eq!(
        by_id("both").wait_reasons,
        Some(vec![TaskWaitReason::Time, TaskWaitReason::Dependency])
    );
    assert_eq!(by_id("done").task_state, Some(TaskWorkflowState::Done));
    assert_eq!(
        by_id("canceled").task_state,
        Some(TaskWorkflowState::Canceled)
    );
    assert_eq!(
        by_id("conflicted").task_state,
        Some(TaskWorkflowState::Conflicted)
    );

    let waiting = workspace
        .search_records_filtered(
            root,
            Some(SearchRecordKind::Task),
            "",
            20,
            now,
            Some("state == 'waiting'"),
        )
        .unwrap()
        .value;
    assert_eq!(waiting.items.len(), 2);
    let blocked = workspace
        .search_records_filtered(
            root,
            Some(SearchRecordKind::Task),
            "",
            20,
            now,
            Some("state == 'blocked'"),
        )
        .unwrap()
        .value;
    assert_eq!(blocked.items.len(), 1);
    assert_eq!(blocked.items[0].id.as_deref(), Some("dependency"));
    let conflicted = workspace
        .search_records_filtered(
            root,
            Some(SearchRecordKind::Task),
            "",
            20,
            now,
            Some("state == 'conflicted'"),
        )
        .unwrap()
        .value;
    assert_eq!(conflicted.items.len(), 1);
    let time_waiting = workspace
        .search_records_filtered(
            root,
            Some(SearchRecordKind::Task),
            "",
            20,
            now,
            Some("wait_reasons.exists(reason, reason == 'time')"),
        )
        .unwrap()
        .value;
    assert_eq!(time_waiting.items.len(), 2);
    let prioritized = workspace
        .search_records_filtered(
            root,
            Some(SearchRecordKind::Task),
            "",
            20,
            now,
            Some("priority != null && priority >= 7"),
        )
        .unwrap()
        .value;
    assert_eq!(prioritized.items.len(), 1);
    assert_eq!(prioritized.items[0].id.as_deref(), Some("ready"));
}

#[test]
fn batches_reverse_task_relations_with_open_document_precedence() {
    let now = DateTime::parse_from_rfc3339("2026-08-11T12:00:00+08:00").unwrap();
    let store = SqliteSemanticStore::open_in_memory().unwrap();
    let mut workspace = Workspace::with_sqlite_store(store);
    let target = "`- Target\n\n `+ task\n\n `@ target\n";
    let dependent =
        "`- Dependent\n\n `+ task\n\n `@ dependent\n\n `= depends|target.plumb#target\n";
    workspace.insert_disk("target.plumb", 1, target).unwrap();
    workspace.insert_disk("source.plumb", 1, dependent).unwrap();

    let blocking_targets = |workspace: &Workspace| {
        workspace
            .search_records_filtered(
                Path::new(""),
                Some(SearchRecordKind::Task),
                "",
                20,
                now,
                Some("directly_blocking.size() > 0"),
            )
            .unwrap()
            .value
    };
    assert_eq!(
        blocking_targets(&workspace).items[0].id.as_deref(),
        Some("target")
    );

    workspace.open_document(
        "source.plumb",
        2,
        "`- Current source\n\n `+ task\n\n `@ dependent\n",
    );
    assert!(blocking_targets(&workspace).items.is_empty());

    workspace.open_document("source.plumb", 3, dependent);
    assert_eq!(
        blocking_targets(&workspace).items[0].id.as_deref(),
        Some("target")
    );
}

#[test]
fn propagates_effective_priority_through_open_dependencies_and_ancestors() {
    let root = Path::new("notes");
    let now = DateTime::parse_from_rfc3339("2026-08-05T12:00:00+08:00").unwrap();
    let mut workspace = Workspace::new();
    workspace.insert(
            "notes/a.plumb",
            1,
            "`- Parent\n\n `+ task\n\n `@ parent\n\n `= priority|-10\n\n `- Urgent\n\n  `+ task\n\n  `@ urgent\n\n  `= priority|40\n  `= depends|b.plumb#middle #closed\n\n`- Closed\n\n `+ task\n\n `@ closed\n\n `= priority|-20\n `= done|2026-08-04T12:00:00+08:00\n",
        );
    workspace.insert(
        "notes/b.plumb",
        1,
        "`- Middle\n\n `+ task\n\n `@ middle\n\n `= priority|1\n `= depends|c.plumb#base\n",
    );
    workspace.insert("notes/c.plumb", 1, "`- Base\n\n `+ task\n\n `@ base\n");
    workspace.insert(
            "notes/cycle.plumb",
            1,
            "`- Cycle high\n\n `+ task\n\n `@ cycle-high\n\n `= priority|30\n `= depends|#cycle-low\n`- Cycle low\n\n `+ task\n\n `@ cycle-low\n\n `= priority|-10\n `= depends|#cycle-high\n",
        );

    let results = workspace
        .search_records(root, Some(SearchRecordKind::Task), "", 20, now)
        .unwrap()
        .value;
    let priority = |id: &str| {
        results
            .items
            .iter()
            .find(|record| record.id.as_deref() == Some(id))
            .unwrap()
            .effective_priority
    };
    assert_eq!(priority("urgent"), Some(40));
    assert_eq!(priority("parent"), Some(40));
    assert_eq!(priority("middle"), Some(40));
    assert_eq!(priority("base"), Some(40));
    assert_eq!(priority("closed"), Some(-20));
    assert_eq!(priority("cycle-high"), Some(30));
    assert_eq!(priority("cycle-low"), Some(30));
}

#[test]
fn propagates_search_priority_through_persistent_dependencies() {
    let now = DateTime::parse_from_rfc3339("2026-08-29T08:00:00+08:00").unwrap();
    let store = SqliteSemanticStore::open_in_memory().unwrap();
    let mut workspace = Workspace::with_sqlite_store(store);
    workspace
            .insert_disk(
                "source.plumb",
                1,
                "`- Urgent\n\n `+ task\n\n `@ urgent\n\n `= priority|40\n `= depends|target.plumb#target\n",
            )
            .unwrap();
    workspace
        .insert_disk(
            "target.plumb",
            1,
            "`- Target\n\n `+ task\n\n `@ target\n\n `= priority|-5\n",
        )
        .unwrap();

    let results = workspace
        .search_records("", Some(SearchRecordKind::Task), "", 20, now)
        .unwrap()
        .value;
    assert_eq!(
        results
            .items
            .iter()
            .find(|record| record.id.as_deref() == Some("target"))
            .unwrap()
            .effective_priority,
        Some(40)
    );
}

#[test]
fn unfiltered_search_decodes_only_selected_persistent_records() {
    let now = DateTime::parse_from_rfc3339("2026-08-29T08:00:00+08:00").unwrap();
    let store = SqliteSemanticStore::open_in_memory().unwrap();
    let mut workspace = Workspace::with_sqlite_store(store.clone());
    workspace
            .insert_disk(
                "selected.plumb",
                1,
                concat!(
                    "`- Selected task\n\n `+ task\n\n `@ selected-task\n\n `= due|2026-08-30T08:00:00+08:00\n",
                    "`- 09:00|Selected event\n\n `+ event\n\n `@ selected-event\n",
                ),
            )
            .unwrap();
    workspace
        .insert_disk(
            "other.plumb",
            1,
            concat!(
                "`- Other task\n\n `+ task\n\n `@ other-task\n",
                "`- 10:00|Other event\n\n `+ event\n\n `@ other-event\n",
            ),
        )
        .unwrap();
    store
        .execute_batch_for_test(
            "UPDATE tasks SET record = X'FF' WHERE title = 'Other task';\
                 UPDATE events SET record = X'FF' WHERE title = 'Other event';",
        )
        .unwrap();

    let results = workspace
        .search_records("", None, "Selected", 20, now)
        .unwrap()
        .value;
    assert_eq!(results.items.len(), 3);
    assert!(results.complete);
    let task = results
        .items
        .iter()
        .find(|record| record.kind == SearchRecordKind::Task)
        .unwrap();
    assert_eq!(task.due.as_deref(), Some("2026-08-30T08:00:00+08:00"));
    let event = results
        .items
        .iter()
        .find(|record| record.kind == SearchRecordKind::Event)
        .unwrap();
    assert_eq!(event.tasks, Some(Vec::new()));
}

#[test]
fn search_records_use_current_valid_snapshots_and_report_truncation() {
    let now = DateTime::parse_from_rfc3339("2026-07-22T12:00:00Z").unwrap();
    let mut workspace = Workspace::new();
    workspace.insert("a.plumb", 1, "Old title\n");
    workspace.insert("a.plumb", 2, "New title\n");
    workspace.insert("b.plumb", 1, "Another\n");

    let limited = workspace
        .search_records("", None, "", 1, now)
        .unwrap()
        .value;
    assert_eq!(limited.items.len(), 1);
    assert!(!limited.complete);
    assert!(limited
        .items
        .iter()
        .all(|record| record.revision != 1 || record.path != Path::new("a.plumb")));

    workspace.insert("a.plumb", 3, "`span[broken\n");
    let invalid = workspace
        .search_records("", None, "new", 20, now)
        .unwrap()
        .value;
    assert!(invalid.items.is_empty());
}

#[test]
fn document_rename_rewrites_incoming_and_outgoing_relative_paths() {
    let mut workspace = Workspace::new();
    workspace.insert(
        "notes/a.plumb",
        1,
        "`# A\n  `@ a\n\n`->[c|../shared/c.plumb#c]\n",
    );
    workspace.insert("notes/b.plumb", 2, "`->[a|a.plumb#a]\n");
    workspace.insert("shared/c.plumb", 3, "`# C\n  `@ c\n");
    let link = &workspace
        .get("notes/b.plumb")
        .unwrap()
        .current
        .as_ref()
        .unwrap()
        .output
        .links[0];
    let offset = link.path_range.as_ref().unwrap().start;
    let target = workspace
        .path_rename_target_at("notes/b.plumb", offset)
        .unwrap();
    assert_eq!(target.input, PathRenameInput::Path);
    let edit = workspace
        .rename_document(&target, "archive/a.plumb")
        .unwrap();
    assert_eq!(edit.resource_operations.len(), 1);
    let incoming = edit
        .document_changes
        .iter()
        .find(|document| document.path == Path::new("notes/b.plumb"))
        .unwrap();
    assert_eq!(incoming.edits[0].new_text, "archive/a.plumb");
    let outgoing = edit
        .document_changes
        .iter()
        .find(|document| document.path == Path::new("notes/a.plumb"))
        .unwrap();
    assert_eq!(outgoing.edits[0].new_text, "../../shared/c.plumb");
}

#[test]
fn document_rename_strengthens_autolink_delimiters() {
    let mut workspace = Workspace::new();
    workspace.insert("notes/a.plumb", 1, "`# A\n  `@ a\n");
    let reference = "`->\"a.plumb#a\"\n";
    workspace.insert("notes/b.plumb", 2, reference);
    let link = &workspace
        .get("notes/b.plumb")
        .unwrap()
        .current
        .as_ref()
        .unwrap()
        .output
        .links[0];
    let target = workspace
        .path_rename_target_at("notes/b.plumb", link.path_range.as_ref().unwrap().start)
        .unwrap();
    let edit = workspace
        .rename_document(&target, "archive/a] final.plumb")
        .unwrap();
    let incoming = edit
        .document_changes
        .iter()
        .find(|document| document.path == Path::new("notes/b.plumb"))
        .unwrap();
    let mut edited = reference.to_string();
    for text_edit in incoming.edits.iter().rev() {
        edited.replace_range(text_edit.range.clone(), &text_edit.new_text);
    }
    assert_eq!(edited, "`->\"archive/a] final.plumb#a\"\n");
    assert!(parse(edited).is_valid());
}

#[test]
fn resolves_open_task_dependencies_and_blocked_state() {
    let mut workspace = Workspace::new();
    workspace.insert(
            "notes/Project Plan.plumb",
            1,
            "`- Draft\n\n `+ task\n\n `@ draft\n`- Done\n\n `+ task\n\n `@ done\n\n `= done|2026-07-20T09:00:00Z\n",
        );
    workspace.insert(
            "notes/review.plumb",
            2,
            "`- Review\n\n `+ task\n\n `@ review\n\n `= depends|Project Plan.plumb#draft Project Plan.plumb#done\n",
        );

    let task = &workspace
        .get("notes/review.plumb")
        .unwrap()
        .current
        .as_ref()
        .unwrap()
        .output
        .tasks
        .tasks[0];
    let blockers = workspace
        .open_task_dependencies("notes/review.plumb", task)
        .unwrap()
        .value;
    assert_eq!(blockers.len(), 1);
    assert_eq!(blockers[0].target.id, "draft");
    assert!(
        workspace
            .is_task_blocked("notes/review.plumb", task)
            .unwrap()
            .value
    );
    assert_eq!(
        workspace
            .directly_blocking_tasks("notes/Project Plan.plumb", "draft")
            .unwrap()
            .value,
        vec![TaskRef {
            path: PathBuf::from("notes/review.plumb"),
            id: "review".to_string(),
        }]
    );
    assert_eq!(
        workspace.task_at("notes/review.plumb", task.range.start),
        Some(task)
    );

    let diagnostics = workspace.diagnostics("notes/review.plumb").unwrap().value;
    let blocked = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "task.blocked")
        .unwrap();
    assert_eq!(blocked.severity, DiagnosticSeverity::Hint);
}

#[test]
fn diagnoses_completed_tasks_with_open_dependencies_and_descendants() {
    let mut workspace = Workspace::new();
    workspace.insert(
        "remote.plumb",
        1,
        "`- Remote blocker\n\n `+ task\n\n `@ remote\n",
    );
    workspace.insert(
            "tasks.plumb",
            2,
            "`- Completed parent\n\n `+ task\n\n `@ parent\n\n `= done|2026-07-27T10:00:00Z\n `= depends|#explicit remote.plumb#remote\n\n `- Explicit child\n\n  `+ task\n\n  `@ explicit\n\n `- Implicit child\n\n  `+ task\n\n `- Canceled child\n\n  `+ task\n\n  `= canceled|2026-07-27T10:01:00Z\n\n`- Canceled parent\n\n `+ task\n\n `= canceled|2026-07-27T10:02:00Z\n\n `- Open child is allowed\n\n  `+ task\n\n`- Completed tree\n\n `+ task\n\n `= done|2026-07-27T10:03:00Z\n\n `- Completed child\n\n  `+ task\n\n  `= done|2026-07-27T10:04:00Z\n",
        );

    let diagnostics = workspace.diagnostics("tasks.plumb").unwrap().value;
    let dependency = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "task.done-with-open-dependency")
        .unwrap();
    assert_eq!(dependency.severity, DiagnosticSeverity::Warning);
    assert_eq!(
        dependency.message,
        "completed task still depends on 2 open tasks"
    );
    assert_eq!(dependency.related.len(), 1);

    let descendant = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "task.done-with-open-descendant")
        .unwrap();
    assert_eq!(descendant.severity, DiagnosticSeverity::Warning);
    assert_eq!(
        descendant.message,
        "completed task still contains 1 open descendant"
    );
    assert_eq!(descendant.related.len(), 1);
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code.starts_with("task.done-with-open-"))
            .count(),
        2
    );
}

#[test]
fn diagnoses_invalid_task_targets_self_dependencies_and_cycles() {
    let mut workspace = Workspace::new();
    workspace.insert(
            "tasks.plumb",
            1,
            "`node Plain anchor\n  `@ plain\n\n`- A\n\n `+ task\n\n `@ a\n\n `= depends|#b\n`- B\n\n `+ task\n\n `@ b\n\n `= depends|#a\n`- Self\n\n `+ task\n\n `@ self\n\n `= depends|#self\n`- Invalid targets\n\n `+ task\n\n `= prev|#plain\n `= depends|#plain #missing bare#invalid missing.plumb#x\n",
        );

    let diagnostics = workspace.diagnostics("tasks.plumb").unwrap().value;
    let codes = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();
    assert!(codes.contains(&"task.non-task-target"));
    assert!(codes.contains(&"task.unresolved-anchor"));
    assert!(codes.contains(&"task.invalid-target"));
    assert!(codes.contains(&"task.unresolved-path"));
    assert!(codes.contains(&"task.self-dependency"));
    assert!(codes.contains(&"task.dependency-cycle"));
}

#[test]
fn diagnostic_context_builds_persistent_cycles_without_decoding_task_records() {
    let store = SqliteSemanticStore::open_in_memory().unwrap();
    let mut workspace = Workspace::with_sqlite_store(store.clone());
    workspace
        .insert_disk(
            "a.plumb",
            1,
            "`- A\n\n `+ task\n\n `@ a\n\n `= depends|b.plumb#b\n",
        )
        .unwrap();
    workspace
        .insert_disk(
            "b.plumb",
            1,
            "`- B\n\n `+ task\n\n `@ b\n\n `= depends|a.plumb#a\n",
        )
        .unwrap();
    store
        .execute_batch_for_test(
            "UPDATE tasks SET record = X'FF'; UPDATE anchors SET record = X'FF';",
        )
        .unwrap();

    let context = workspace.diagnostic_context().unwrap();
    assert!(dependency_cycle_contains(
        &context.task_dependency_graph,
        &TaskRef {
            path: PathBuf::from("a.plumb"),
            id: "a".to_string(),
        }
    ));
    assert!(workspace.all_tasks().is_err());
}

#[test]
fn diagnostic_context_obeys_open_document_dependency_overlay() {
    let store = SqliteSemanticStore::open_in_memory().unwrap();
    let mut workspace = Workspace::with_sqlite_store(store);
    workspace
        .insert_disk(
            "a.plumb",
            1,
            "`- A\n\n `+ task\n\n `@ a\n\n `= depends|b.plumb#b\n",
        )
        .unwrap();
    workspace
        .insert_disk(
            "b.plumb",
            1,
            "`- B\n\n `+ task\n\n `@ b\n\n `= depends|a.plumb#a\n",
        )
        .unwrap();
    let task_a = TaskRef {
        path: PathBuf::from("a.plumb"),
        id: "a".to_string(),
    };
    assert!(dependency_cycle_contains(
        &workspace
            .diagnostic_context()
            .unwrap()
            .task_dependency_graph,
        &task_a,
    ));

    workspace.open_document("b.plumb", 2, "`- B\n\n `+ task\n\n `@ b\n");
    assert!(!dependency_cycle_contains(
        &workspace
            .diagnostic_context()
            .unwrap()
            .task_dependency_graph,
        &task_a,
    ));
}

#[test]
fn task_status_operation_is_guarded_and_formats_the_affected_block() {
    let mut workspace = Workspace::new();
    let source = "`- Write parser\n\n `+ task\n\n `@ write\n\n `= due|2026-07-21T09:00:00Z\n";
    workspace.insert("tasks.plumb", 7, source);

    let edit = workspace
        .set_task_status_by_id(
            "tasks.plumb",
            "write",
            TaskStatus::Done,
            "2026-07-20T12:00:00+08:00",
        )
        .unwrap();
    let document = &edit.document_changes[0];
    assert_eq!(document.expected_revision, 7);
    assert_eq!(document.edits.len(), 1);
    let operation = &document.edits[0];
    let mut edited = source.to_string();
    edited.replace_range(operation.range.clone(), &operation.new_text);
    assert!(edited.contains("`@ write"));
    assert!(edited.contains("`= due|2026-07-21T09:00:00Z"));
    assert!(edited.contains("`= done|2026-07-20T12:00:00+08:00"));
    assert_eq!(plumb_format::format(&edited).unwrap(), edited);
}

#[test]
fn task_status_targets_an_explicitly_anchored_nested_task() {
    let source = "`- MJCF in, USD out solver\n\n `+ task\n\n `@ task-f81deb18\n\n `= created|2026-05-24T02:35:50Z\n\n `- 刚体版本\n\n  `+ task\n\n  `@ task-9d49eb30\n\n  `= created|2026-05-24T02:35:32Z\n  `= done|2026-05-26T01:43:39Z\n\n `- parse MJCF\n\n  `+ task\n\n  `@ task-c2cf5756\n\n  `= created|2026-05-27T13:03:04Z\n\n `- solver with passive joint\n\n  `+ task\n\n  `@ task-99e28dad\n\n  `= created|2026-05-27T13:02:45Z\n";
    let mut workspace = Workspace::new();
    workspace.insert("embodied-intelligence.plumb", 12, source);

    let operation = workspace
        .set_task_status(
            "embodied-intelligence.plumb",
            source.find("parse MJCF").unwrap(),
            TaskStatus::Done,
            "2026-07-22T22:41:21+08:00",
        )
        .unwrap();
    let edit = &operation.document_changes[0].edits[0];
    let mut edited = source.to_string();
    edited.replace_range(edit.range.clone(), &edit.new_text);

    assert!(edited.contains("`@ task-c2cf5756"));
    assert!(edited.contains("`= done|2026-07-22T22:41:21+08:00"));
    assert_eq!(
        edited.matches("`= done|2026-07-22T22:41:21+08:00").count(),
        1
    );
    assert_eq!(plumb_format::format(&edited).unwrap(), edited);
}

#[test]
fn task_status_formats_multiline_attributes_with_a_long_head() {
    let source = "`- `->[如何在 nix 中检查 IFD|如何在 nix 中检查 IFD.plumb]\n\n `+ task\n\n `= created|2026-07-21T14:37:59+08:00\n";
    assert_eq!(plumb_format::format(source).unwrap(), source);
    let mut workspace = Workspace::new();
    workspace.insert("closed.plumb", 8, source);

    let operation = workspace
        .set_task_status(
            "closed.plumb",
            source.find("检查 IFD").unwrap(),
            TaskStatus::Done,
            "2026-07-21T21:52:24+08:00",
        )
        .unwrap();
    let edit = &operation.document_changes[0].edits[0];
    let mut edited = source.to_string();
    edited.replace_range(edit.range.clone(), &edit.new_text);

    assert_eq!(
            edited,
            "`- `->[如何在 nix 中检查 IFD|如何在 nix 中检查 IFD.plumb]\n\n `+ task\n\n `= created|2026-07-21T14:37:59+08:00\n `= done|2026-07-21T21:52:24+08:00\n"
        );
    assert_eq!(plumb_format::format(&edited).unwrap(), edited);
}

#[test]
fn task_status_formats_the_complete_owner_subtree() {
    let source = "`- Parent\n\n `+ task\n\n `@ parent\n\n `- Child\n\n`# Following\n";
    let mut workspace = Workspace::new();
    workspace.insert("tasks.plumb", 9, source);

    let operation = workspace
        .set_task_status_by_id(
            "tasks.plumb",
            "parent",
            TaskStatus::Done,
            "2026-07-21T22:00:00+08:00",
        )
        .unwrap();
    let edited = apply_single_edit(source, &operation);

    assert!(edited.contains("`@ parent"));
    assert!(edited.contains("`= done|2026-07-21T22:00:00+08:00"));
    assert!(edited.contains("\n `- Child\n\n`# Following"));
    assert_eq!(plumb_format::format(&edited).unwrap(), edited);
}

#[test]
fn task_authoring_operations_convert_items_and_add_created() {
    let source = "`- Outer\n\n `@ outer\n\n `+ keep\n\n `- Nested\n\n`- Closed\n\n `+ task\n\n `@ closed\n\n `= done|2026-07-20T09:00:00Z\n\n`- Existing\n\n `+ task\n\n `@ existing\n\n `= created|2026-07-19T09:00:00Z\n";
    let mut workspace = Workspace::new();
    workspace.insert("tasks.plumb", 7, source);
    let timestamp = "2026-07-20T10:00:00+08:00";

    let nested_offset = source.find("Nested").unwrap();
    let conversion = workspace
        .convert_list_item_to_task("tasks.plumb", nested_offset, timestamp)
        .unwrap();
    assert_eq!(conversion.document_changes[0].expected_revision, 7);
    let edit = &conversion.document_changes[0].edits[0];
    let mut converted = source.to_string();
    converted.replace_range(edit.range.clone(), &edit.new_text);
    assert!(
        converted.contains(" `- Nested\n\n  `+ task\n\n  `= created|2026-07-20T10:00:00+08:00\n")
    );

    let outer_conversion = workspace
        .convert_list_item_to_task("tasks.plumb", source.find("Outer").unwrap(), timestamp)
        .unwrap();
    assert!(
            outer_conversion.document_changes[0].edits[0]
                .new_text
                .contains(
                "`- Outer\n\n `+ task\n\n `@ outer\n\n `+ keep\n\n `= created|2026-07-20T10:00:00+08:00\n"
            ),
            "{}",
            outer_conversion.document_changes[0].edits[0].new_text
        );

    let closed_offset = source.find("Closed").unwrap();
    let created = workspace
        .add_task_created("tasks.plumb", closed_offset, timestamp)
        .unwrap();
    assert!(created.document_changes[0].edits[0]
        .new_text
        .contains("`= created|2026-07-20T10:00:00+08:00"));
    assert_eq!(
        workspace.add_task_created("tasks.plumb", nested_offset, timestamp),
        Err(TaskEditError::TaskNotFound)
    );
    assert_eq!(
        workspace.add_task_created("tasks.plumb", source.find("Existing").unwrap(), timestamp),
        Err(TaskEditError::CreatedAlreadyExists)
    );
}

#[test]
fn task_authoring_operations_use_valid_syntax_while_semantics_are_pending() {
    let timestamp = "2026-07-20T10:00:00+08:00";
    let mut workspace = Workspace::new();
    let list_source = "`- Convert me\n";
    workspace
        .begin_document_revision("tasks.plumb", 1, list_source)
        .unwrap();
    assert!(workspace.get("tasks.plumb").unwrap().current.is_none());
    assert!(workspace
        .convert_list_item_to_task(
            "tasks.plumb",
            list_source.find("Convert").unwrap(),
            timestamp
        )
        .is_ok());

    let task_source = "`- Add created\n\n `+ task\n\n `@ task\n";
    workspace
        .begin_document_revision("tasks.plumb", 2, task_source)
        .unwrap();
    assert!(workspace
        .add_task_created(
            "tasks.plumb",
            task_source.find("Add created").unwrap(),
            timestamp,
        )
        .is_ok());
}

#[test]
fn authoring_operations_preserve_formatter_fixed_points() {
    let timestamp = "2026-07-21T21:52:24+08:00";

    let conversion_source = "`- Convert me\n  `@ item\n  `+ kind\n";
    let mut conversion_workspace = Workspace::new();
    conversion_workspace.insert("conversion.plumb", 1, conversion_source);
    let conversion = conversion_workspace
        .convert_list_item_to_task(
            "conversion.plumb",
            conversion_source.find("Convert").unwrap(),
            timestamp,
        )
        .unwrap();
    let converted = apply_single_edit(conversion_source, &conversion);
    assert_eq!(plumb_format::format(&converted).unwrap(), converted);

    let created_source = "`- Add created\n\n `+ task\n\n `@ created\n";
    let mut created_workspace = Workspace::new();
    created_workspace.insert("created.plumb", 2, created_source);
    let created = created_workspace
        .add_task_created(
            "created.plumb",
            created_source.find("Add created").unwrap(),
            timestamp,
        )
        .unwrap();
    let with_created = apply_single_edit(created_source, &created);
    assert_eq!(plumb_format::format(&with_created).unwrap(), with_created);

    let id_source = "`note Add an explicit identifier\n  `+ class\n  `= key|value\n";
    let mut id_workspace = Workspace::new();
    id_workspace.insert("id.plumb", 3, id_source);
    let id = id_workspace
        .add_explicit_id("id.plumb", id_source.find("identifier").unwrap())
        .unwrap();
    let with_id = apply_single_edit(id_source, &id);
    assert_eq!(plumb_format::format(&with_id).unwrap(), with_id);

    let metadata_source = "`# Section\n";
    let mut metadata_workspace = Workspace::new();
    metadata_workspace.insert("metadata.plumb", 4, metadata_source);
    let metadata = metadata_workspace
        .insert_metadata("metadata.plumb", 0, "metadata", timestamp)
        .unwrap();
    let with_metadata = apply_single_edit(metadata_source, &metadata);
    assert_eq!(plumb_format::format(&with_metadata).unwrap(), with_metadata);
}

#[test]
fn add_explicit_id_targets_the_deepest_block_and_generates_unique_slugs() {
    let source = "`# Hello, World!\n  `+ keep\n\n`node Outer\n\n      `child Nested title\n\n`text\n|\"\n raw\n\n`note Multiline attrs\n  `+ keep\n\n`other Existing\n  `@ hello-world\n\n`# Hello, World!\n";
    let mut workspace = Workspace::new();
    workspace.insert("note.plumb", 7, source);

    let heading = workspace
        .add_explicit_id("note.plumb", source.find("Hello, World!").unwrap())
        .unwrap();
    assert_eq!(heading.document_changes[0].expected_revision, 7);
    let edit = &heading.document_changes[0].edits[0];
    assert!(
        edit.new_text
            .contains("`# Hello, World!\n\n `@ hello-world-2\n\n `+ keep\n"),
        "{}",
        edit.new_text
    );

    let nested = workspace
        .add_explicit_id("note.plumb", source.find("Nested title").unwrap())
        .unwrap();
    assert!(
        nested.document_changes[0].edits[0]
            .new_text
            .contains("`child Nested title\n\n       `@ nested-title\n"),
        "{}",
        nested.document_changes[0].edits[0].new_text
    );

    let sibling_boundary = workspace
        .add_explicit_id("note.plumb", source.find("`node").unwrap())
        .unwrap();
    assert!(
        sibling_boundary.document_changes[0].edits[0]
            .new_text
            .contains("`node Outer\n\n `@ outer\n"),
        "{}",
        sibling_boundary.document_changes[0].edits[0].new_text
    );

    let raw = workspace
        .add_explicit_id("note.plumb", source.find("raw").unwrap())
        .unwrap();
    assert!(raw.document_changes[0].edits[0]
        .new_text
        .contains("`text\n\n `@ text\n\n|\"\n raw"));

    let multiline = workspace
        .add_explicit_id("note.plumb", source.find("Multiline attrs").unwrap())
        .unwrap();
    assert!(
        multiline.document_changes[0].edits[0]
            .new_text
            .contains("`note Multiline attrs\n\n `@ multiline-attrs\n\n `+ keep\n"),
        "{}",
        multiline.document_changes[0].edits[0].new_text
    );

    for operation in [&heading, &nested, &sibling_boundary, &raw, &multiline] {
        let edit = &operation.document_changes[0].edits[0];
        let mut edited = source.to_string();
        edited.replace_range(edit.range.clone(), &edit.new_text);
        let parsed = parse(&edited);
        assert!(parsed.is_valid(), "{edited}\n{:?}", parsed.diagnostics);
        assert!(!analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax")
        )
        .anchors
        .is_empty());
    }

    assert_eq!(
        workspace.add_explicit_id("note.plumb", source.find("Existing").unwrap()),
        Err(ExplicitIdError::IdAlreadyExists)
    );
}

#[test]
fn add_explicit_id_requires_a_valid_marked_block() {
    let mut workspace = Workspace::new();
    workspace.insert("plain.plumb", 1, "Plain paragraph\n");
    workspace.insert("raw.plumb", 1, "`\"\n raw\n");
    workspace.insert("invalid.plumb", 2, "`broken[\n");

    assert_eq!(
        workspace.add_explicit_id("plain.plumb", 2),
        Err(ExplicitIdError::BlockNotFound)
    );
    assert_eq!(
        workspace.add_explicit_id("raw.plumb", 4),
        Err(ExplicitIdError::BlockNotFound)
    );
    assert_eq!(
        workspace.add_explicit_id("invalid.plumb", 2),
        Err(ExplicitIdError::StaleOrInvalidDocument)
    );
    assert_eq!(
        workspace.add_explicit_id("missing.plumb", 0),
        Err(ExplicitIdError::StaleOrInvalidDocument)
    );
}

#[test]
fn task_status_cursor_falls_back_from_closed_child_to_open_parent() {
    let mut workspace = Workspace::new();
    let source =
            "`- Outer\n\n `+ task\n\n `@ outer\n\n  `- Inner\n\n   `+ task\n\n   `@ inner\n\n   `= done|2026-07-20T09:00:00Z\n";
    workspace.insert("tasks.plumb", 3, source);
    let tasks = &workspace
        .get("tasks.plumb")
        .unwrap()
        .current
        .as_ref()
        .unwrap()
        .output
        .tasks
        .tasks;
    let edit = workspace
        .set_task_status(
            "tasks.plumb",
            source.find("Inner").unwrap(),
            TaskStatus::Done,
            "2026-07-20T12:00:00Z",
        )
        .unwrap();
    assert_eq!(edit.document_changes[0].edits.len(), 1);
    let operation = &edit.document_changes[0].edits[0];
    assert!(operation.range.start <= tasks[0].range.start);
    assert!(operation.range.end >= tasks[0].range.end);
    assert_ne!(operation.range.start, tasks[1].attribute_insert);
    let mut edited = source.to_string();
    edited.replace_range(operation.range.clone(), &operation.new_text);
    assert!(edited.contains("`@ outer"));
    assert!(edited.contains("`= done|2026-07-20T12:00:00Z"));
    assert_eq!(edited.matches("`= done|2026-07-20T09:00:00Z").count(), 1);
    assert!(matches!(
        workspace.set_task_status_by_id(
            "tasks.plumb",
            "inner",
            TaskStatus::Done,
            "2026-07-20T12:00:00Z",
        ),
        Err(WorkspaceOperationError::Operation(
            TaskEditError::TaskAlreadyClosed
        ))
    ));
}

#[test]
fn task_status_operation_rejects_closed_blocked_and_recurring_tasks() {
    let mut workspace = Workspace::new();
    workspace.insert(
            "tasks.plumb",
            1,
            "`- Blocker\n\n `+ task\n\n `@ blocker\n`- Blocked\n\n `+ task\n\n `@ blocked\n\n `= depends|#blocker\n`- Closed\n\n `+ task\n\n `@ closed\n\n `= done|2026-07-20T09:00:00Z\n`- Recurring\n\n `+ task\n\n `@ recur\n\n `= due|2026-07-21T09:00:00Z\n `= recur|P1D\n",
        );
    let timestamp = "2026-07-20T12:00:00Z";
    let source = &workspace.get("tasks.plumb").unwrap().parsed.source;
    assert!(matches!(
        workspace.set_task_status(
            "tasks.plumb",
            source.find("Blocked").unwrap(),
            TaskStatus::Done,
            timestamp,
        ),
        Err(WorkspaceOperationError::Operation(
            TaskEditError::TaskBlocked
        ))
    ));
    assert!(workspace
        .set_task_status(
            "tasks.plumb",
            source.find("Blocked").unwrap(),
            TaskStatus::Canceled,
            timestamp,
        )
        .is_ok());
    assert!(matches!(
        workspace.set_task_status(
            "tasks.plumb",
            source.find("Closed").unwrap(),
            TaskStatus::Canceled,
            timestamp,
        ),
        Err(WorkspaceOperationError::Operation(
            TaskEditError::TaskAlreadyClosed
        ))
    ));
    assert!(workspace
        .set_task_status(
            "tasks.plumb",
            source.find("Recurring").unwrap(),
            TaskStatus::Done,
            timestamp,
        )
        .is_ok());
}

#[test]
fn recurring_task_status_advances_and_clones_the_task_losslessly() {
    let mut workspace = Workspace::new();
    let source = "`- Monthly review\n\n `+ task\n\n `- daily\n\n `= due|2026-01-31T09:00:00+08:00\n `= wait|2026-01-30T09:00:00+08:00\n `= recur|P1M\n\n `note Keep details\n\n `- Nested\n\n  `+ task\n\n  `@ nested\n\n  `= done|2026-01-20T09:00:00+08:00\n";
    workspace.insert("tasks.plumb", 4, source);

    let edit = workspace
        .set_task_status(
            "tasks.plumb",
            source.find("Nested").unwrap(),
            TaskStatus::Done,
            "2026-01-31T10:00:00+08:00",
        )
        .unwrap();
    let mut edits = edit.document_changes[0].edits.clone();
    assert_eq!(edits.len(), 1);
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
    let mut edited = source.to_string();
    for edit in edits {
        edited.replace_range(edit.range, &edit.new_text);
    }

    assert!(edited.contains("`@ monthly-review-2026-01-31"));
    assert!(edited.contains("`= done|2026-01-31T10:00:00+08:00"));
    assert!(edited.contains("`@ monthly-review-2026-02-28"));
    assert!(edited.contains("`= created|2026-01-31T10:00:00+08:00"));
    assert!(edited.contains("`= due|2026-02-28T09:00:00+08:00"));
    assert!(edited.contains("`= wait|2026-02-28T09:00:00+08:00"));
    assert!(edited.contains("`= prev|#monthly-review-2026-01-31"));
    assert_eq!(edited.matches("nested").count(), 1);
    assert_eq!(edited.matches("`= done|2026-01-20").count(), 1);
    let parsed = parse(&edited);
    assert!(parsed.is_valid(), "{}\n{:?}", edited, parsed.diagnostics);
    let output = analyze_document(
        parsed
            .valid_syntax()
            .expect("semantic analysis requires valid syntax"),
    );
    assert_eq!(output.tasks.tasks.len(), 4);
    assert_eq!(output.tasks.tasks[2].state(), TaskState::Open);
}

#[test]
fn recurring_task_clone_preserves_crlf_and_nested_base_indent() {
    let source = "`node Parent\r\n\r\n      `- Weekly review\r\n\r\n       `+ task\r\n\r\n       `= due|2026-07-20T09:00:00+08:00\r\n       `= recur|P1W\r\n";
    let mut workspace = Workspace::new();
    workspace.insert("tasks.plumb", 5, source);
    let task = &workspace
        .get("tasks.plumb")
        .unwrap()
        .current
        .as_ref()
        .unwrap()
        .output
        .tasks
        .tasks[0];
    let line_start = source[..task.range.start]
        .rfind('\n')
        .map_or(0, |offset| offset + 1);
    assert_eq!(&source[line_start..task.range.start], "      ");

    let edit = workspace
        .set_task_status(
            "tasks.plumb",
            source.find("Weekly review").unwrap(),
            TaskStatus::Done,
            "2026-07-20T10:00:00+08:00",
        )
        .unwrap();
    assert_eq!(edit.document_changes[0].edits.len(), 1);
    let replacement = &edit.document_changes[0].edits[0].new_text;
    assert!(replacement.starts_with("      `-"), "{replacement:?}");
    assert!(
        replacement.contains("\r\n\r\n       `+ task\r\n"),
        "{replacement:?}"
    );
    assert!(!replacement.starts_with("\r\n"));
    assert!(!replacement.replace("\r\n", "").contains('\n'));

    let mut edits = edit.document_changes[0].edits.clone();
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
    let mut edited = source.to_string();
    for edit in edits {
        edited.replace_range(edit.range, &edit.new_text);
    }
    let parsed = parse(&edited);
    assert!(parsed.is_valid(), "{edited:?}\n{:?}", parsed.diagnostics);
    assert!(!edited.contains("\r\n\r\n\r\n"));
}

#[test]
fn recurring_task_completion_preserves_canonical_layout() {
    let source = "`# 饮食相关任务\n\n`- 控制饮食\n\n `+ task\n\n `@ 控制饮食-2026-07-20\n\n `= priority|-5\n `= created|2026-07-20T01:06:48+08:00\n `= due|2026-07-20T23:59:59+08:00\n `= wait|2026-07-20T00:00:00+08:00\n `= recur|P1D\n `= prev|#控制饮食-2026-07-19\n\n`# 锻炼相关任务\n";
    assert_eq!(plumb_format::format(source).unwrap(), source);
    let mut workspace = Workspace::new();
    workspace.insert("减肥.plumb", 6, source);

    let operation = workspace
        .set_task_status_by_id(
            "减肥.plumb",
            "控制饮食-2026-07-20",
            TaskStatus::Done,
            "2026-07-21T18:01:12+08:00",
        )
        .unwrap();
    assert_eq!(operation.document_changes[0].edits.len(), 1);
    let edit = &operation.document_changes[0].edits[0];
    let mut edited = source.to_string();
    edited.replace_range(edit.range.clone(), &edit.new_text);

    assert!(edited.contains("`= done|2026-07-21T18:01:12+08:00"));
    assert!(edited.contains("`= prev|#控制饮食-2026-07-20"));
    assert!(edited.contains("`# 锻炼相关任务"));
    assert_eq!(edited.matches("`= priority|-5").count(), 2);
    assert_eq!(plumb_format::format(&edited).unwrap(), edited);
}

#[test]
fn inserts_metadata_with_revision_and_escaped_title() {
    let mut workspace = Workspace::new();
    workspace.insert("notes/my`note.plumb", 7, "`# Section\n");

    let edit = workspace
        .insert_metadata(
            "notes/my`note.plumb",
            0,
            "my`note",
            "2026-07-19T12:34:56+08:00",
        )
        .unwrap();

    assert_eq!(edit.document_changes.len(), 1);
    let document = &edit.document_changes[0];
    assert_eq!(document.path, Path::new("notes/my`note.plumb"));
    assert_eq!(document.expected_revision, 7);
    assert_eq!(document.edits[0].range, 0..0);
    assert_eq!(
        document.edits[0].new_text,
        "`= title|my``note\n`= created|2026-07-19T12:34:56+08:00\n\n"
    );
}

#[test]
fn inserts_formatted_metadata_into_an_empty_document() {
    let mut workspace = Workspace::new();
    workspace.insert("notes/empty.plumb", 11, "");

    let edit = workspace
        .insert_metadata("notes/empty.plumb", 0, "empty", "2026-07-22T12:34:56+08:00")
        .unwrap();

    let document = &edit.document_changes[0];
    assert_eq!(document.expected_revision, 11);
    assert_eq!(document.edits[0].range, 0..0);
    assert_eq!(
        document.edits[0].new_text,
        "`= title|empty\n`= created|2026-07-22T12:34:56+08:00\n"
    );
    assert_eq!(
        plumb_format::format(&document.edits[0].new_text).unwrap(),
        document.edits[0].new_text
    );
}

#[test]
fn metadata_insertion_preserves_crlf() {
    let mut workspace = Workspace::new();
    workspace.insert("note.plumb", 1, "First\r\nSecond\r\n");

    let edit = workspace
        .insert_metadata("note.plumb", 0, "note", "2026-07-19T12:34:56+08:00")
        .unwrap();

    assert_eq!(
        edit.document_changes[0].edits[0].new_text,
        "`= title|note\r\n`= created|2026-07-19T12:34:56+08:00\r\n\r\n"
    );
}

#[test]
fn metadata_insertion_rejects_existing_or_invalid_metadata_target() {
    let mut workspace = Workspace::new();
    workspace.insert("existing.plumb", 1, "`= title|Existing\n");
    assert_eq!(
        workspace.insert_metadata("existing.plumb", 0, "existing", "created"),
        Err(MetadataInsertError::MetadataAlreadyExists)
    );

    workspace.insert("invalid.plumb", 2, "`broken[\n");
    assert_eq!(
        workspace.insert_metadata("invalid.plumb", 0, "invalid", "created"),
        Err(MetadataInsertError::StaleOrInvalidDocument)
    );
    assert_eq!(
        workspace.insert_metadata("missing.plumb", 0, "missing", "created"),
        Err(MetadataInsertError::StaleOrInvalidDocument)
    );
}

#[test]
fn metadata_insertion_requires_cursor_at_document_start() {
    let mut workspace = Workspace::new();
    workspace.insert("doc.plumb", 1, "`# Section\n");
    // Cursor at the very first byte: offered.
    assert!(workspace
        .insert_metadata("doc.plumb", 0, "doc", "2026-07-19T12:34:56+08:00")
        .is_ok());
    // Cursor past the first non-whitespace byte: rejected.
    assert_eq!(
        workspace.insert_metadata("doc.plumb", 3, "doc", "2026-07-19T12:34:56+08:00"),
        Err(MetadataInsertError::CursorNotAtDocumentStart)
    );

    // Leading blank lines do not create an alternate document-start target.
    workspace.insert("blank.plumb", 2, "\n\n`# Section\n");
    assert_eq!(
        workspace.insert_metadata("blank.plumb", 2, "blank", "2026-07-19T12:34:56+08:00"),
        Err(MetadataInsertError::CursorNotAtDocumentStart)
    );
}

#[test]
fn resolves_event_task_associations_and_queries_time_ranges() {
    let mut workspace = Workspace::new();
    workspace.insert(
        "tasks.plumb",
        1,
        "`- Write\n\n `+ task\n\n `@ write\n\n`node Plain\n  `@ plain\n",
    );
    let events = "`= date|2026-07-30\n`= timezone|+08:00\n\n`- 10:30|Early\n\n `+ event\n\n `= timezone|+05:00\n`- 11:00|`->[Write|tasks.plumb#write]\n\n `+ event\n`- 12:00|`->[Write|tasks.plumb#write]\n\n `+ event\n\n `= tasks\n`- 14:00--15:00|Review\n\n `+ event\n\n `@ review\n\n `= uid|review@example\n `= tasks|tasks.plumb#write\n`- 15:00|Point\n\n `+ event\n\n `= tasks|tasks.plumb#plain missing.plumb#task bad\n";
    workspace.insert("events.plumb", 2, events);

    let target = TaskRef {
        path: PathBuf::from("tasks.plumb"),
        id: "write".to_string(),
    };
    let associated = workspace.events_for_task(&target).unwrap().value;
    assert_eq!(associated.len(), 3);
    assert_eq!(
        associated
            .iter()
            .map(|event| event.event.title.as_str())
            .collect::<Vec<_>>(),
        ["Write", "Write", "Review"]
    );

    let day_start = DateTime::parse_from_rfc3339("2026-07-30T05:00:00Z").unwrap();
    let day_end = DateTime::parse_from_rfc3339("2026-07-30T08:00:00Z").unwrap();
    assert_eq!(
        workspace
            .events_overlapping(day_start, day_end)
            .unwrap()
            .value
            .iter()
            .map(|event| event.event.title.as_str())
            .collect::<Vec<_>>(),
        ["Early", "Review", "Point"]
    );

    let start = DateTime::parse_from_rfc3339("2026-07-30T14:30:00+08:00").unwrap();
    let end = DateTime::parse_from_rfc3339("2026-07-30T15:01:00+08:00").unwrap();
    assert_eq!(
        workspace
            .events_overlapping(start, end)
            .unwrap()
            .value
            .iter()
            .map(|event| event.event.title.as_str())
            .collect::<Vec<_>>(),
        ["Review", "Point"]
    );

    let reference_offset = events.find("tasks.plumb#write").unwrap();
    assert!(matches!(
        workspace
            .reference_target_at("events.plumb", reference_offset)
            .unwrap()
            .value,
        Some(ResolvedTarget::Document { ref path }) if path == Path::new("tasks.plumb")
    ));
    assert_eq!(
        workspace
            .references_to("tasks.plumb", "write")
            .unwrap()
            .value
            .len(),
        5
    );
    assert_eq!(
        workspace
            .referenced_documents_from("events.plumb")
            .unwrap()
            .value,
        [PathBuf::from("tasks.plumb")]
    );

    let codes = workspace
        .diagnostics("events.plumb")
        .unwrap()
        .value
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();
    assert!(codes.contains(&"event.target-not-task"), "{codes:?}");
    assert!(codes.contains(&"event.unresolved-task-path"), "{codes:?}");
    assert!(codes.contains(&"event.invalid-task-reference"), "{codes:?}");

    let filtered = workspace
            .search_records_filtered(
                Path::new(""),
                Some(SearchRecordKind::Event),
                "review",
                20,
                DateTime::parse_from_rfc3339("2026-07-30T00:00:00Z").unwrap(),
                Some("uid == 'review@example' && when == '14:00--15:00' && start < timestamp('2026-07-30T07:00:00Z')"),
            )
            .unwrap()
            .value;
    assert_eq!(filtered.items.len(), 1);
    assert_eq!(filtered.items[0].kind, SearchRecordKind::Event);
    assert_eq!(filtered.items[0].title, "Review");
    assert_eq!(
        filtered.items[0]
            .tasks
            .as_ref()
            .unwrap()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["tasks.plumb#write"]
    );

    let point = workspace
        .search_records_filtered(
            Path::new(""),
            Some(SearchRecordKind::Event),
            "point",
            20,
            DateTime::parse_from_rfc3339("2026-07-30T00:00:00Z").unwrap(),
            Some("at == timestamp('2026-07-30T07:00:00Z')"),
        )
        .unwrap()
        .value;
    assert_eq!(point.items.len(), 1);
    assert_eq!(
        point.items[0].at.as_deref(),
        Some("2026-07-30T15:00:00+08:00")
    );
}

#[test]
fn event_task_associations_use_overlapping_containment_index_ranges() {
    let mut workspace = Workspace::new();
    workspace.insert("tasks.plumb", 1, "`- Write\n\n `+ task\n\n `@ write\n");
    workspace.insert(
            "events.plumb",
            2,
            "`= date|2026-08-11\n`= timezone|+08:00\n\n`->[Before|tasks.plumb#write]\n\n`- 10:00|Outer `->[Outer|tasks.plumb#write]\n\n `+ event\n\n `- 11:00|Nested `->[Nested|tasks.plumb#write]\n\n  `+ event\n\n`->[After|tasks.plumb#write]\n",
        );

    let output = workspace.current_output(Path::new("events.plumb")).unwrap();
    let outer = &output.events.events[0];
    let nested = &output.events.events[1];
    assert_eq!(output.event_link_ranges.len(), 2);
    assert_eq!(output.event_link_ranges[0].links, 1..3);
    assert_eq!(output.event_link_ranges[1].links, 2..3);

    assert_eq!(
        workspace
            .event_task_references("events.plumb", outer)
            .unwrap()
            .value
            .len(),
        2
    );
    assert_eq!(
        workspace
            .event_task_references("events.plumb", nested)
            .unwrap()
            .value
            .len(),
        1
    );

    workspace.insert(
            "events.plumb",
            3,
            "`= date|2026-08-11\n`= timezone|+08:00\n\n`- 12:00|Replacement\n\n `+ event\n\n `->[Only|tasks.plumb#write]\n",
        );
    let replacement = workspace.current_output(Path::new("events.plumb")).unwrap();
    assert_eq!(replacement.event_link_ranges.len(), 1);
    assert_eq!(replacement.event_link_ranges[0].links, 0..1);
    assert_eq!(
        workspace
            .event_task_references("events.plumb", &replacement.events.events[0])
            .unwrap()
            .value
            .len(),
        1
    );
}

#[test]
fn parses_reduced_precision_event_shorthand() {
    let now = DateTime::parse_from_rfc3339("2026-08-01T08:00:00+08:00").unwrap();
    for (source, at) in [
        ("11 relax: phone", "2026-08-01T11:00:00+08:00"),
        ("11:10 relax: phone", "2026-08-01T11:10:00+08:00"),
        ("11:10:24 relax: phone", "2026-08-01T11:10:24+08:00"),
        ("2026-05-21T11 relax: phone", "2026-05-21T11:00:00+08:00"),
        ("2026-05-21T11:10 relax: phone", "2026-05-21T11:10:00+08:00"),
        (
            "2026-05-21T11:10:24 relax: phone",
            "2026-05-21T11:10:24+08:00",
        ),
    ] {
        let input = parse_event_shorthand(source, now).unwrap();
        assert_eq!(input.title, "relax: phone");
        assert_eq!(input.at.as_deref(), Some(at), "{source}");
        assert!(input.start.is_none());
        assert!(input.end.is_none());
    }

    let interval = parse_event_shorthand("2026-05-21T11--11:20 review", now).unwrap();
    assert_eq!(interval.start.as_deref(), Some("2026-05-21T11:00:00+08:00"));
    assert_eq!(interval.end.as_deref(), Some("2026-05-21T11:20:00+08:00"));
    assert!(interval.at.is_none());

    let multi_day = parse_event_shorthand("2026-05-21T11--2026-05-23T11 review", now).unwrap();
    assert_eq!(
        multi_day.start.as_deref(),
        Some("2026-05-21T11:00:00+08:00")
    );
    assert_eq!(multi_day.end.as_deref(), Some("2026-05-23T11:00:00+08:00"));
}

#[test]
fn rejects_ambiguous_or_invalid_event_shorthand() {
    let now = DateTime::parse_from_rfc3339("2026-08-01T08:00:00+08:00").unwrap();
    for source in [
        "11",
        "9 meeting",
        "09:5 meeting",
        "09:05:7 meeting",
        "24 meeting",
        "2026-02-30T11 meeting",
        "2026-05-21 11 meeting",
        "11am meeting",
        "11--2026-08-01T12:00:00Z meeting",
        "11--2026-08-01T12:00:00+08:00 meeting",
    ] {
        assert_eq!(
            parse_event_shorthand(source, now),
            Err(EventShorthandError::InvalidShorthand),
            "{source}"
        );
    }
    assert_eq!(
        parse_event_shorthand("11:20--11:20 meeting", now),
        Err(EventShorthandError::InvalidInterval)
    );
    assert_eq!(
        parse_event_shorthand("2026-08-02T11:20--2026-08-01T11:20 meeting", now),
        Err(EventShorthandError::InvalidInterval)
    );
    let cross_midnight = parse_event_shorthand("23:40--00:00 meeting", now).unwrap();
    assert_eq!(
        cross_midnight.end.as_deref(),
        Some("2026-08-02T00:00:00+08:00")
    );
}

#[test]
fn converts_event_shorthand_list_item_in_place() {
    let source = "`- 2026-05-21T11:10--11:20 relax: phone\n";
    let mut workspace = Workspace::new();
    workspace.insert("agenda.plumb", 7, source);
    let now = DateTime::parse_from_rfc3339("2026-08-01T08:00:00+08:00").unwrap();
    let operation = workspace
        .convert_event_shorthand("agenda.plumb", source.find("relax").unwrap(), now)
        .unwrap();
    assert_eq!(operation.document_changes[0].expected_revision, 7);
    let converted = apply_text_edits(
        source.to_string(),
        operation.document_changes[0].edits.clone(),
    )
    .unwrap();
    assert!(converted.contains("\n `+ event\n"), "{converted}");
    assert!(!converted.contains("#e0001"), "{converted}");
    assert!(!converted.contains("event-uids"), "{converted}");
    assert!(converted.contains("`= date|2026-05-21"));
    assert!(converted.contains("`= timezone|+08:00"));
    assert!(converted.contains("`- 11:10--11:20|relax: phone\n\n `+ event\n"));
    assert!(!converted.contains("start="));
    assert!(!converted.contains("end="));
    assert_eq!(plumb_format::format(&converted).unwrap(), converted);

    // Existing id/classes are preserved and the schedule remains the first head argument.
    let kept_source = "`- 11:00--11:20 review\n  `@ mine\n  `+ kind\n";
    workspace.insert("keep.plumb", 8, kept_source);
    let kept = apply_single_edit(
        kept_source,
        &workspace
            .convert_event_shorthand("keep.plumb", 5, now)
            .unwrap(),
    );
    assert!(kept.contains("`@ mine"), "{kept}");
    assert!(kept.contains("`+ kind"), "{kept}");
    assert!(kept.contains("\n `+ event\n"), "{kept}");
    assert!(
        kept.contains("`- 11:00--11:20|review\n\n `+ event\n"),
        "{kept}"
    );

    // Parsed and verbatim inline structure survives prefix removal.
    let rich_source =
        "`- 11 wheel: distinguish `code[|\"[nix develop]\"|=[language|sh]] and `*[normal] shell\n";
    workspace.insert("markup.plumb", 9, rich_source);
    let rich = apply_single_edit(
        rich_source,
        &workspace
            .convert_event_shorthand("markup.plumb", 3, now)
            .unwrap(),
    );
    assert!(
            rich.contains("`- 11:00|wheel: distinguish `code[|\"[nix develop]\"|=[language|sh]] and `*[normal] shell\n\n `+ event\n"),
            "{rich}"
        );

    // A list item that is already an event is left alone.
    workspace.insert("done.plumb", 10, "`- 11:00--11:20|review\n\n `+ event\n");
    assert_eq!(
        workspace.convert_event_shorthand("done.plumb", 5, now),
        Err(EventShorthandError::EventAlreadyExists)
    );

    // A plain paragraph (no list marker) no longer offers the action.
    workspace.insert("plain.plumb", 11, "11:00--11:20 review\n");
    assert_eq!(
        workspace.convert_event_shorthand("plain.plumb", 3, now),
        Err(EventShorthandError::ListItemNotFound)
    );
}

#[test]
fn converts_selected_event_shorthands_in_one_edit() {
    let source = "`= date|2026-08-01\n`= timezone|+08:00\n\n`- 09:00|Existing\n\n `+ event\n\n `@ e0015\n\n`- 10:00--10:20 first\n`- ordinary item\n`- 10:20--10:30 second `\"code\"\n";
    let mut workspace = Workspace::new();
    workspace.insert("agenda.plumb", 9, source);
    let now = DateTime::parse_from_rfc3339("2026-08-03T08:00:00+09:00").unwrap();
    let start = source.find("10:00").unwrap();
    let end = source.len();
    let operation = workspace
        .convert_event_shorthands("agenda.plumb", start..end, now)
        .unwrap();
    let converted = apply_text_edits(
        source.to_string(),
        operation.document_changes[0].edits.clone(),
    )
    .unwrap();
    assert_eq!(converted.matches("`+ event").count(), 3, "{converted}");
    assert!(!converted.contains("event-uids"), "{converted}");
    assert!(converted.contains("`- 10:00--10:20|first\n\n `+ event\n"));
    assert!(converted.contains("`- 10:20--10:30|second `\"code\"\n\n `+ event\n"));
    assert!(converted.contains("`- ordinary item"));
    assert!(!converted.contains("date=2026-08-01"));
    assert!(!converted.contains("timezone=\"+08:00\""));
    workspace.insert("agenda.plumb", 10, converted);
    let events = &workspace
        .current_output(Path::new("agenda.plumb"))
        .unwrap()
        .events
        .events;
    assert_eq!(
        events[1].start.as_ref().unwrap().value,
        "2026-08-01T10:00:00+08:00"
    );
}

#[test]
fn infers_open_event_ends_from_adjacent_selected_siblings() {
    let source = "`= date|2026-08-01\n`= timezone|+08:00\n\n`- 18:00-- 事件 1\n`- 18:30-- 事件 2\n";
    let mut workspace = Workspace::new();
    workspace.insert("agenda.plumb", 1, source);
    let now = DateTime::parse_from_rfc3339("2026-08-03T08:00:00+09:00").unwrap();
    let operation = workspace
        .convert_event_shorthands(
            "agenda.plumb",
            source.find("18:00").unwrap()..source.len(),
            now,
        )
        .unwrap();
    let converted = apply_text_edits(
        source.to_string(),
        operation.document_changes[0].edits.clone(),
    )
    .unwrap();
    assert!(
        converted.contains("`- 18:00--18:30|事件 1\n\n `+ event\n"),
        "{converted}"
    );
    assert!(converted.contains("`- 18:30-- 事件 2"), "{converted}");
    assert_eq!(converted.matches("`+ event").count(), 1, "{converted}");

    workspace.insert("agenda.plumb", 2, source);
    let first = workspace
        .convert_event_shorthand("agenda.plumb", source.find("事件 1").unwrap(), now)
        .unwrap();
    let first_converted =
        apply_text_edits(source.to_string(), first.document_changes[0].edits.clone()).unwrap();
    assert!(
        first_converted.contains("`- 18:00--18:30|事件 1\n\n `+ event\n"),
        "{first_converted}"
    );
    assert_eq!(
        workspace.convert_event_shorthand("agenda.plumb", source.find("事件 2").unwrap(), now,),
        Err(EventShorthandError::InvalidShorthand)
    );

    let chain = "`= date|2026-08-01\n`= timezone|+08:00\n\n`- 18:00-- first\n`- 18:30-- second\n`- 19:00--20:00 third\n";
    workspace.insert("chain.plumb", 3, chain);
    let chained = apply_text_edits(
        chain.to_string(),
        workspace
            .convert_event_shorthands(
                "chain.plumb",
                chain.find("18:00").unwrap()..chain.len(),
                now,
            )
            .unwrap()
            .document_changes[0]
            .edits
            .clone(),
    )
    .unwrap();
    assert!(
        chained.contains("`- 18:00--18:30|first\n\n `+ event\n"),
        "{chained}"
    );
    assert!(
        chained.contains("`- 18:30--19:00|second\n\n `+ event\n"),
        "{chained}"
    );
    assert!(
        chained.contains("`- 19:00--20:00|third\n\n `+ event\n"),
        "{chained}"
    );
    assert_eq!(chained.matches("`+ event").count(), 3, "{chained}");

    let interrupted = "`- 18:00-- first\n`- ordinary\n`- 18:30 next\n";
    workspace.insert("interrupted.plumb", 4, interrupted);
    assert_eq!(
        workspace.convert_event_shorthand(
            "interrupted.plumb",
            interrupted.find("first").unwrap(),
            now,
        ),
        Err(EventShorthandError::InvalidShorthand)
    );
}

#[test]
fn creates_updates_and_deletes_events_with_guarded_canonical_edits() {
    let mut workspace = Workspace::new();
    let source = "`# Agenda\n";
    workspace.insert("agenda.plumb", 7, source);
    let created = workspace
        .create_event(
            "agenda.plumb",
            &EventInput {
                title: "Review".to_string(),
                at: None,
                start: Some("2026-07-30T14:00:00+08:00".to_string()),
                end: Some("2026-07-30T15:00:00+08:00".to_string()),
                tasks: vec!["tasks.plumb#write".to_string()],
            },
        )
        .unwrap();
    assert_eq!(created.document_changes[0].expected_revision, 7);
    let created_source = apply_single_edit(source, &created);
    assert!(created_source.contains("\n `+ event\n"), "{created_source}");
    assert!(!created_source.contains("#e0001"), "{created_source}");
    assert!(!created_source.contains("event-uids"), "{created_source}");
    assert!(
        created_source.contains("`- 14:00--15:00|Review\n\n `+ event\n"),
        "{created_source}"
    );
    assert_eq!(
        plumb_format::format(&created_source).unwrap(),
        created_source
    );

    let multi_day = workspace
        .create_event(
            "agenda.plumb",
            &EventInput {
                title: "Conference".to_string(),
                at: None,
                start: Some("2026-07-30T14:00:00+08:00".to_string()),
                end: Some("2026-08-02T14:00:00+08:00".to_string()),
                tasks: Vec::new(),
            },
        )
        .unwrap();
    let multi_day_source = apply_single_edit(source, &multi_day);
    assert!(
        multi_day_source.contains("`- 14:00--2026-08-02T14:00|Conference\n\n `+ event\n"),
        "{multi_day_source}"
    );
    let multi_day_parsed = plumb_syntax::parse(multi_day_source);
    assert!(
        multi_day_parsed.is_valid(),
        "{:?}",
        multi_day_parsed.diagnostics
    );

    workspace.insert("agenda.plumb", 8, created_source.clone());
    let event = workspace
        .current_output(Path::new("agenda.plumb"))
        .unwrap()
        .events
        .events[0]
        .clone();
    let updated = workspace
        .update_event(
            "agenda.plumb",
            event.range.clone(),
            &EventInput {
                title: "Updated review".to_string(),
                at: Some("2026-07-30T16:00:00+08:00".to_string()),
                start: None,
                end: None,
                tasks: Vec::new(),
            },
        )
        .unwrap();
    let updated_source = apply_single_edit(&created_source, &updated);
    assert!(updated_source.contains("Updated review"));
    assert!(
        updated_source.contains("`- 16:00|Updated review\n\n `+ event\n"),
        "{updated_source}"
    );
    assert!(!updated_source.contains("tasks.plumb#write"));

    workspace.insert("agenda.plumb", 9, updated_source.clone());
    let updated_event = workspace
        .current_output(Path::new("agenda.plumb"))
        .unwrap()
        .events
        .events[0]
        .clone();
    let deleted = workspace
        .delete_event("agenda.plumb", updated_event.range)
        .unwrap();
    let deleted_source = apply_text_edits(
        updated_source.clone(),
        deleted.document_changes[0].edits.clone(),
    )
    .unwrap();
    assert!(!deleted_source.contains("event-uids"));
    assert!(deleted_source.contains("`# Agenda"));
    assert!(!deleted_source.contains("Updated review"));

    workspace.insert("agenda.plumb", 10, deleted_source.clone());
    let recreated = workspace
        .create_event(
            "agenda.plumb",
            &EventInput {
                title: "Next".to_string(),
                at: Some("2026-07-30T17:00:00+08:00".to_string()),
                start: None,
                end: None,
                tasks: Vec::new(),
            },
        )
        .unwrap();
    let recreated_source = apply_text_edits(
        deleted_source.clone(),
        recreated.document_changes[0].edits.clone(),
    )
    .unwrap();
    assert!(
        recreated_source.contains("\n `+ event\n"),
        "{recreated_source}"
    );
    assert!(
        recreated_source.contains("`- 17:00|Next\n\n `+ event\n"),
        "{recreated_source}"
    );

    assert_eq!(
        workspace.create_event(
            "agenda.plumb",
            &EventInput {
                title: "Bad".to_string(),
                at: None,
                start: Some("2026-07-30T16:00:00+08:00".to_string()),
                end: Some("2026-07-30T15:00:00+08:00".to_string()),
                tasks: Vec::new(),
            },
        ),
        Err(EventEditError::InvalidInterval)
    );
}

#[test]
fn updating_an_event_preserves_semantic_uid_and_opaque_when_property() {
    let source = "`= date|2026-07-30\n`= timezone|+08:00\n\n`- 14:00|Review\n\n `+ event\n\n `@ review\n\n `= uid|legacy@example\n `= when|14:00\n";
    let mut workspace = Workspace::new();
    workspace.insert("agenda.plumb", 1, source);
    let event = workspace
        .current_output(Path::new("agenda.plumb"))
        .unwrap()
        .events
        .events[0]
        .clone();
    let operation = workspace
        .update_event(
            "agenda.plumb",
            event.range,
            &EventInput {
                title: "Updated".to_string(),
                at: Some("2026-07-30T15:00:00+08:00".to_string()),
                start: None,
                end: None,
                tasks: Vec::new(),
            },
        )
        .unwrap();
    let updated = apply_text_edits(
        source.to_string(),
        operation.document_changes[0].edits.clone(),
    )
    .unwrap();
    assert!(updated.contains("`@ review"), "{updated}");
    assert_eq!(updated.matches("`+ event").count(), 1, "{updated}");
    assert!(updated.contains("`= uid|legacy@example"), "{updated}");
    assert!(updated.contains("`= when|14:00"), "{updated}");
    assert!(
        updated.contains("`- 15:00|Updated\n\n `+ event\n"),
        "{updated}"
    );
}

#[test]
fn creates_nested_tasks_and_updates_fields_without_losing_owned_content() {
    let mut workspace = Workspace::new();
    let source = "`- Parent\n\n `+ task\n\n `@ parent\n\n `= custom|keep\n `= created|2026-07-01T09:00:00Z\n\n  `note Keep details\n\n`- Other\n\n `+ task\n\n `@ other\n\n`# Following\n";
    workspace.insert("tasks.plumb", 4, source);
    let parent = workspace
        .current_output(Path::new("tasks.plumb"))
        .unwrap()
        .tasks
        .tasks[0]
        .clone();
    let created = workspace
        .create_task(
            "tasks.plumb",
            &TaskAuthoringInput {
                title: "Nested".to_string(),
                due: Some("2026-08-01T10:00:00Z".to_string()),
                priority: Some(-2),
                ..TaskAuthoringInput::default()
            },
            &TaskPlacement {
                parent: Some(parent.range.clone()),
                after: None,
            },
            "2026-07-31T10:00:00Z",
        )
        .unwrap();
    assert_eq!(created.document_changes[0].expected_revision, 4);
    let created_source = apply_single_edit(source, &created);
    assert!(created_source.contains("\n  `+ task\n"), "{created_source}");
    assert!(created_source.contains("`@ task-"), "{created_source}");
    assert!(created_source.contains("`= priority|-2"));
    assert!(created_source.contains("`note Keep details"));

    workspace.insert("tasks.plumb", 5, created_source.clone());
    let parent = workspace
        .current_output(Path::new("tasks.plumb"))
        .unwrap()
        .tasks
        .tasks[0]
        .clone();
    let updated = workspace
        .update_task(
            "tasks.plumb",
            parent.range,
            &TaskAuthoringInput {
                title: "Renamed parent".to_string(),
                due: Some("2026-09-01T10:00:00Z".to_string()),
                depends: vec!["#other".to_string()],
                ..TaskAuthoringInput::default()
            },
            "2026-07-31T11:00:00Z",
        )
        .unwrap();
    let updated_source = apply_single_edit(&created_source, &updated);
    assert!(updated_source.contains("`= custom|keep"));
    assert!(updated_source.contains("`@ parent"));
    assert!(updated_source.contains("`= created|2026-07-01T09:00:00Z"));
    assert!(updated_source.contains("`note Keep details"));
    assert!(updated_source.contains("Nested"));
    assert!(updated_source.contains("Renamed parent"));
    assert!(!updated_source.contains("priority=-2\n`# Following"));

    workspace.insert("tasks.plumb", 6, updated_source.clone());
    let parent = workspace
        .current_output(Path::new("tasks.plumb"))
        .unwrap()
        .tasks
        .tasks[0]
        .clone();
    let patched = workspace
        .update_task_patch(
            "tasks.plumb",
            parent.range,
            &TaskAuthoringPatch {
                priority: Some(Some(9)),
                ..TaskAuthoringPatch::default()
            },
            "2026-07-31T12:00:00Z",
        )
        .unwrap();
    let patched_source = apply_single_edit(&updated_source, &patched);
    assert!(patched_source.contains("`= priority|9"));
    assert!(patched_source.contains("`= due|2026-09-01T10:00:00Z"));
    assert!(patched_source.contains("#other"), "{patched_source}");
}

#[test]
fn task_authoring_rejects_invalid_fields_and_placements() {
    let mut workspace = Workspace::new();
    workspace.insert("tasks.plumb", 1, "`# Tasks\n");
    let invalid = |input: TaskAuthoringInput| {
        workspace.create_task(
            "tasks.plumb",
            &input,
            &TaskPlacement::default(),
            "2026-07-31T10:00:00Z",
        )
    };
    assert!(matches!(
        invalid(TaskAuthoringInput {
            title: "Bad datetime".to_string(),
            due: Some("tomorrow".to_string()),
            ..TaskAuthoringInput::default()
        }),
        Err(WorkspaceOperationError::Operation(
            TaskAuthoringError::InvalidDatetime
        ))
    ));
    assert!(matches!(
        invalid(TaskAuthoringInput {
            title: "Bad recurrence".to_string(),
            recur: Some("P0D".to_string()),
            ..TaskAuthoringInput::default()
        }),
        Err(WorkspaceOperationError::Operation(
            TaskAuthoringError::InvalidRecurrence
        ))
    ));
    assert!(matches!(
        invalid(TaskAuthoringInput {
            title: "Bad reference".to_string(),
            depends: vec!["missing-hash".to_string()],
            ..TaskAuthoringInput::default()
        }),
        Err(WorkspaceOperationError::Operation(
            TaskAuthoringError::InvalidReference
        ))
    ));
    assert!(matches!(
        invalid(TaskAuthoringInput {
            title: "Missing dependency".to_string(),
            depends: vec!["#missing".to_string()],
            ..TaskAuthoringInput::default()
        }),
        Err(WorkspaceOperationError::Operation(
            TaskAuthoringError::UnresolvedReference
        ))
    ));

    workspace.insert(
        "tasks.plumb",
        2,
        "`- A\n\n `+ task\n\n `@ a\n\n `= depends|#b\n`- B\n\n `+ task\n\n `@ b\n",
    );
    let b = workspace
        .current_output(Path::new("tasks.plumb"))
        .unwrap()
        .tasks
        .tasks[1]
        .clone();
    assert!(matches!(
        workspace.update_task_patch(
            "tasks.plumb",
            b.range,
            &TaskAuthoringPatch {
                depends: Some(vec!["#a".to_string()]),
                ..TaskAuthoringPatch::default()
            },
            "2026-07-31T10:00:00Z",
        ),
        Err(WorkspaceOperationError::Operation(
            TaskAuthoringError::DependencyCycle
        ))
    ));
}

#[test]
fn moves_task_subtrees_within_and_between_parents() {
    let mut workspace = Workspace::new();
    let source = plumb_format::format(
            "`- Left\n\n `+ task\n\n `@ left\n\n `- A\n\n  `+ task\n\n  `@ a\n\n  `note A details\n\n `- B\n\n  `+ task\n\n  `@ b\n\n`- Right\n\n `+ task\n\n `@ right\n",
        )
        .unwrap();
    workspace.insert("tasks.plumb", 1, &source);
    let tasks = &workspace
        .current_output(Path::new("tasks.plumb"))
        .unwrap()
        .tasks
        .tasks;
    assert_eq!(
        tasks
            .iter()
            .map(|task| (task.id.as_ref().unwrap().value.as_str(), task.depth))
            .collect::<Vec<_>>(),
        [("left", 0), ("a", 1), ("b", 1), ("right", 0)]
    );
    let by_id = |id: &str| {
        tasks
            .iter()
            .find(|task| task.id.as_ref().is_some_and(|field| field.value == id))
            .unwrap()
            .range
            .clone()
    };
    let reordered = workspace
        .move_task(
            "tasks.plumb",
            by_id("a"),
            &TaskPlacement {
                parent: Some(by_id("left")),
                after: Some(by_id("b")),
            },
        )
        .unwrap();
    let reordered_source = apply_document_edit(source, "tasks.plumb", 1, reordered).unwrap();
    assert!(reordered_source.find("`@ b").unwrap() < reordered_source.find("`@ a").unwrap());
    assert!(reordered_source.contains("`note A details"));
    assert!(parse(&reordered_source).is_valid(), "{reordered_source}");
    assert!(
        reordered_source.contains("`- Right\n\n `+ task\n\n `@ right\n"),
        "{reordered_source}"
    );

    workspace.insert("tasks.plumb", 2, reordered_source.clone());
    let tasks = &workspace
        .current_output(Path::new("tasks.plumb"))
        .unwrap()
        .tasks
        .tasks;
    let range = |id: &str| {
        tasks
            .iter()
            .find(|task| task.id.as_ref().is_some_and(|field| field.value == id))
            .unwrap()
            .range
            .clone()
    };
    let reparented = workspace
        .move_task(
            "tasks.plumb",
            range("a"),
            &TaskPlacement {
                parent: Some(range("right")),
                after: None,
            },
        )
        .unwrap();
    let reparented_source =
        apply_document_edit(reordered_source.clone(), "tasks.plumb", 2, reparented).unwrap();
    workspace.insert("tasks.plumb", 3, reparented_source.clone());
    let tasks = &workspace
        .current_output(Path::new("tasks.plumb"))
        .unwrap()
        .tasks
        .tasks;
    let a = tasks
        .iter()
        .find(|task| task.id.as_ref().is_some_and(|field| field.value == "a"))
        .unwrap();
    let right = tasks
        .iter()
        .find(|task| task.id.as_ref().is_some_and(|field| field.value == "right"))
        .unwrap();
    assert_eq!(a.depth, right.depth + 1, "{reparented_source}");
    assert!(reparented_source.contains("`note A details"));

    let updated = workspace
        .update_task_patch(
            "tasks.plumb",
            a.range.clone(),
            &TaskAuthoringPatch {
                due: Some(Some("2026-08-15T02:30:00Z".to_string())),
                priority: Some(Some(-7)),
                ..TaskAuthoringPatch::default()
            },
            "2026-07-31T10:00:00Z",
        )
        .unwrap();
    let updated_source = apply_document_edit(reparented_source, "tasks.plumb", 3, updated).unwrap();
    let parsed = parse(&updated_source);
    assert!(
        parsed.is_valid(),
        "{updated_source}\n{:?}",
        parsed.diagnostics
    );
    let formatted = plumb_format::format(&updated_source).expect("updated task source formats");
    assert_eq!(formatted, updated_source);
}

#[test]
fn updates_and_moves_task_subtrees_in_one_original_revision_operation() {
    let mut workspace = Workspace::new();
    let source = plumb_format::format(
            "`- Parent\n\n `+ task\n\n `@ parent\n\n `- Group\n\n  `+ task\n\n  `@ group\n\n  `- Idless child\n\n   `+ task\n\n   `= custom|keep\n\n   `note Keep details\n\n `- Sibling\n\n  `+ task\n\n  `@ sibling\n\n`- Destination\n\n `+ task\n\n `@ destination\n",
        )
        .unwrap();
    workspace.insert("tasks.plumb", 17, &source);
    let tasks = &workspace
        .current_output(Path::new("tasks.plumb"))
        .unwrap()
        .tasks
        .tasks;
    let child = tasks
        .iter()
        .find(|task| task.title == "Idless child")
        .unwrap();
    assert!(child.id.is_none());
    let parent = tasks
        .iter()
        .find(|task| task.id.as_ref().is_some_and(|id| id.value == "parent"))
        .unwrap();
    let operation = workspace
        .update_and_move_task(
            "tasks.plumb",
            child.range.clone(),
            &TaskAuthoringInput {
                title: "Updated idless child".to_string(),
                due: Some("2026-08-15T02:30:00Z".to_string()),
                priority: Some(-3),
                ..TaskAuthoringInput::default()
            },
            Some(&TaskPlacement {
                parent: Some(parent.range.clone()),
                after: Some(
                    tasks
                        .iter()
                        .find(|task| task.id.as_ref().is_some_and(|id| id.value == "sibling"))
                        .unwrap()
                        .range
                        .clone(),
                ),
            }),
            "2026-08-01T10:00:00Z",
        )
        .unwrap();
    assert_eq!(operation.document_changes.len(), 1);
    assert_eq!(operation.document_changes[0].expected_revision, 17);
    assert_eq!(operation.document_changes[0].edits.len(), 1);
    let updated = apply_document_edit(source, "tasks.plumb", 17, operation).unwrap();
    assert!(
        updated.contains(" `- Updated idless child\n\n  `+ task\n"),
        "{updated}"
    );
    assert!(updated.contains("`= custom|keep"), "{updated}");
    assert!(updated.contains("`note Keep details"), "{updated}");
    assert!(updated.contains("`= due|2026-08-15T02:30:00Z"), "{updated}");
    assert!(updated.contains("`= priority|-3"), "{updated}");
    let parsed = parse(&updated);
    assert!(parsed.is_valid(), "{updated}\n{:?}", parsed.diagnostics);
    assert_eq!(plumb_format::format(&updated).unwrap(), updated);

    workspace.insert("tasks.plumb", 18, updated.clone());
    let tasks = &workspace
        .current_output(Path::new("tasks.plumb"))
        .unwrap()
        .tasks
        .tasks;
    let child = tasks
        .iter()
        .find(|task| task.title == "Updated idless child")
        .unwrap();
    let parent = tasks
        .iter()
        .find(|task| task.id.as_ref().is_some_and(|id| id.value == "parent"))
        .unwrap();
    assert_eq!(child.depth, parent.depth + 1, "{updated}");

    let destination = tasks
        .iter()
        .find(|task| task.id.as_ref().is_some_and(|id| id.value == "destination"))
        .unwrap();
    let cross_root = workspace
        .update_and_move_task(
            "tasks.plumb",
            child.range.clone(),
            &TaskAuthoringInput {
                title: "Cross-root child".to_string(),
                due: Some("2026-08-16T02:30:00Z".to_string()),
                priority: Some(-4),
                ..TaskAuthoringInput::default()
            },
            Some(&TaskPlacement {
                parent: Some(destination.range.clone()),
                after: None,
            }),
            "2026-08-01T11:00:00Z",
        )
        .unwrap();
    assert_eq!(cross_root.document_changes.len(), 1);
    assert_eq!(cross_root.document_changes[0].expected_revision, 18);
    assert_eq!(cross_root.document_changes[0].edits.len(), 2);
    let cross_root_updated = apply_document_edit(updated, "tasks.plumb", 18, cross_root).unwrap();
    assert!(cross_root_updated.contains(" `- Cross-root child\n\n  `+ task\n"));
    assert!(cross_root_updated.contains("`= custom|keep"));
    assert!(
        parse(&cross_root_updated).is_valid(),
        "{cross_root_updated}"
    );
    assert_eq!(
        plumb_format::format(&cross_root_updated).unwrap(),
        cross_root_updated
    );
}
