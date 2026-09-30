use std::path::Path;

#[test]
fn interactive_duration_consumers_cannot_regain_full_collection_queries() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for path in [
        "crates/plumb-web/src/model.rs",
        "crates/plumb-web/src/model/query.rs",
        "crates/plumb-app/src/server/code_lens.rs",
    ] {
        let source = std::fs::read_to_string(root.join(path)).unwrap();
        assert!(
            !source.contains(".task_duration_totals("),
            "{path} must request only displayed identities"
        );
    }
    let duration = include_str!("../src/agenda/duration.rs");
    assert!(!duration.contains("selected_events_in_scope"));
    assert!(!duration.contains(".events_for_path("));
    let graph = include_str!("../src/derived.rs");
    assert!(
        !graph.contains("use crate::Workspace"),
        "tracked computation has no workspace enumeration capability"
    );
}
