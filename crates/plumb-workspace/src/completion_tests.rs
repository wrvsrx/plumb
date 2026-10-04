use super::*;
use plumb_semantics::LinkCompletionContext;

fn paths(query: &str) -> LinkCompletionContext {
    LinkCompletionContext::SingleArgumentPath {
        replace: 4..7,
        query: query.into(),
        suffix: String::new(),
    }
}
fn anchors(path: &str) -> LinkCompletionContext {
    LinkCompletionContext::Anchor {
        path: path.into(),
        replace: 8..8,
        query: String::new(),
    }
}
fn labels(workspace: &Workspace, context: &LinkCompletionContext) -> Vec<String> {
    workspace
        .complete_link("inbox.plumb", context)
        .unwrap()
        .require_complete()
        .unwrap()
        .into_iter()
        .map(|item| item.label)
        .collect()
}

#[test]
fn pending_source_and_unrelated_semantics_do_not_block_current_paths() {
    let mut workspace = Workspace::new();
    workspace.insert("inbox.plumb", 1, "`->{}\n");
    workspace.insert("target.plumb", 1, "`= title 三角洲\n");
    workspace.insert("other.plumb", 1, "Other\n");
    let _source = workspace.begin_document_revision("inbox.plumb", 2, "`->{三}\n");
    let _other = workspace.begin_document_revision("other.plumb", 2, "Pending\n");
    assert_eq!(labels(&workspace, &paths("三")), vec!["target.plumb"]);
    let items = workspace
        .complete_link("inbox.plumb", &paths("三"))
        .unwrap()
        .value;
    assert_eq!(items[0].replace, 4..7);
    assert_eq!(items[0].new_text, "target.plumb");
}

#[test]
fn pending_title_uses_current_valid_regions_and_never_last_valid() {
    let mut workspace = Workspace::new();
    workspace.insert("target.plumb", 1, "`= title 三角洲\n");
    labels(&workspace, &paths("三"));
    let pending = workspace
        .begin_document_revision("target.plumb", 2, "`= title 新标题\n\n`->{\n")
        .unwrap();
    assert!(labels(&workspace, &paths("三")).is_empty());
    assert_eq!(labels(&workspace, &paths("新")), vec!["target.plumb"]);
    workspace.install_document_analysis(pending.analyze());
    assert_eq!(labels(&workspace, &paths("新")), vec!["target.plumb"]);
    assert_eq!(workspace.link_completion_work().contributions_changed, 0);
    workspace.begin_document_revision("target.plumb", 3, "`= title {\n");
    assert!(labels(&workspace, &paths("新")).is_empty());
}

#[test]
fn anchor_readiness_is_target_local_and_missing_recovers() {
    let mut workspace = Workspace::new();
    assert!(labels(&workspace, &anchors("target.plumb")).is_empty());
    workspace.insert("target.plumb", 1, "`# Old\n `@ old\n");
    let old = workspace.clone();
    let pending = workspace
        .begin_document_revision("target.plumb", 2, "`# New\n `@ new\n")
        .unwrap();
    workspace.begin_document_revision("other.plumb", 1, "Pending\n");
    let context = anchors("target.plumb");
    assert_eq!(
        workspace.pending_link_completion_target("inbox.plumb", &context),
        Some("target.plumb".into())
    );
    let result = workspace.complete_link("inbox.plumb", &context).unwrap();
    assert!(!result.is_complete());
    assert!(result.value.is_empty());
    workspace.install_document_analysis(pending.analyze());
    assert_eq!(labels(&workspace, &context), vec!["#new"]);
    assert_eq!(labels(&old, &context), vec!["#old"]);
    workspace.remove("target.plumb");
    assert!(labels(&workspace, &context).is_empty());
}

#[test]
fn warm_path_work_is_changed_facts_and_postings_not_workspace_size() {
    let mut workspace = Workspace::new();
    workspace.insert("target.plumb", 1, "`= title 三角洲\n");
    workspace.insert("other.plumb", 1, "Other\n");
    let expected = labels(&workspace, &paths("三"));
    let snapshot = workspace.clone();
    assert_eq!(labels(&workspace, &paths("三")), expected);
    assert_eq!(
        workspace.link_completion_work(),
        LinkCompletionWork {
            candidates_visited: 1,
            ..Default::default()
        }
    );
    workspace.insert("other.plumb", 2, "Changed body\n");
    assert_eq!(labels(&workspace, &paths("三")), expected);
    assert_eq!(
        workspace.link_completion_work(),
        LinkCompletionWork {
            documents_read: 1,
            candidates_visited: 1,
            ..Default::default()
        }
    );
    for i in 0..200 {
        workspace.insert(format!("unrelated-{i}.plumb"), 1, "Other\n");
    }
    assert_eq!(labels(&workspace, &paths("三")), expected);
    assert_eq!(workspace.link_completion_work().candidates_visited, 1);
    assert_eq!(workspace.link_completion_work().documents_read, 200);
    workspace.insert("target.plumb", 2, "`= title 新标题\n");
    assert!(labels(&workspace, &paths("三")).is_empty());
    assert_eq!(workspace.link_completion_work().contributions_changed, 1);
    assert_eq!(labels(&snapshot, &paths("三")), expected);
    let mut cold = workspace.clone();
    cold.derived.completion = Default::default();
    assert_eq!(
        labels(&workspace, &paths("新")),
        labels(&cold, &paths("新"))
    );
}

#[test]
fn persistent_completion_changes_overlays_deletion_and_snapshot_parity() {
    let store = SqliteSemanticStore::open_in_memory().unwrap();
    let mut workspace = Workspace::with_sqlite_store(store.clone());
    workspace
        .insert_disk("target.plumb", 1, "`= title 三角洲\n`# Target\n `@ disk\n")
        .unwrap();
    assert_eq!(labels(&workspace, &paths("三")), vec!["target.plumb"]);
    labels(&workspace, &paths("三"));
    assert_eq!(workspace.link_completion_work().documents_read, 0);
    assert_eq!(workspace.link_completion_work().stored_changes_read, 0);
    workspace.open_document(
        "target.plumb",
        2,
        "`= title 新标题\n`# Target\n `@ overlay\n",
    );
    assert!(labels(&workspace, &paths("三")).is_empty());
    assert_eq!(
        labels(&workspace, &anchors("target.plumb")),
        vec!["#overlay"]
    );
    workspace.close_document("target.plumb");
    assert_eq!(labels(&workspace, &paths("三")), vec!["target.plumb"]);
    assert_eq!(labels(&workspace, &anchors("target.plumb")), vec!["#disk"]);
    let frozen = Workspace::with_sqlite_store(store.readonly_snapshot().unwrap());
    workspace.remove_disk("target.plumb").unwrap();
    assert!(labels(&workspace, &paths("三")).is_empty());
    assert_eq!(workspace.link_completion_work().stored_changes_read, 1);
    assert!(labels(&workspace, &anchors("target.plumb")).is_empty());
    assert_eq!(labels(&frozen, &paths("三")), vec!["target.plumb"]);
    workspace
        .insert_disk("target.plumb", 3, "`= title 三角洲\n")
        .unwrap();
    assert_eq!(labels(&workspace, &paths("三")), vec!["target.plumb"]);
    assert_eq!(workspace.link_completion_work().documents_read, 1);
    let cold = Workspace::with_sqlite_store(store);
    assert_eq!(
        labels(&workspace, &paths("三")),
        labels(&cold, &paths("三"))
    );
}
