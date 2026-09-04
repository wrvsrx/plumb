use std::sync::Arc;

use plumb_syntax::{parse, GreenDocument, SourceChange};
use proptest::prelude::*;

#[test]
fn green_materialization_matches_fresh_parse_for_structural_cases() {
    for source in [
        "",
        "\n\n",
        "`= title Document\n\n`note First\n\n `= key value\n\n`note Last\n",
        "`note First\r\n\r\n `note Nested 😀\r\n`note Last\r\n",
        "`rust\"\n fn main() {}\n more raw\n\n`note After\n",
        "before {unclosed\n\n`note recovered\n",
        "\tinvalid structural tab\nnext\n",
    ] {
        let green = GreenDocument::parse(source);
        let fresh = parse(source);
        assert_eq!(green.diagnostics(), fresh.diagnostics, "{source:?}");
        assert_eq!(green.materialize(), fresh, "{source:?}");
    }
}

#[test]
fn green_reparse_reuses_unchanged_shards_across_range_shifts() {
    let old = "`note First\n\n`note Middle\n\n`note Last\n";
    let green = GreenDocument::parse(old);
    let old_shards = green
        .shards()
        .map(|view| Arc::clone(view.shard()))
        .collect::<Vec<_>>();
    let start = old.find("Middle").unwrap();
    let mut new = old.to_string();
    new.replace_range(start..start + "Middle".len(), "Changed middle");
    let reparsed = green.reparse_from_change(
        new.clone(),
        SourceChange {
            old_range: start..start + "Middle".len(),
            new_range: start..start + "Changed middle".len(),
        },
    );
    let new_shards = reparsed
        .document
        .shards()
        .map(|view| Arc::clone(view.shard()))
        .collect::<Vec<_>>();

    assert_eq!(reparsed.document.materialize(), parse(new));
    assert!(Arc::ptr_eq(&old_shards[0], &new_shards[0]));
    assert!(!Arc::ptr_eq(&old_shards[1], &new_shards[1]));
    assert!(Arc::ptr_eq(&old_shards[2], &new_shards[2]));
}

proptest! {
    #[test]
    fn arbitrary_green_documents_materialize_like_fresh_parse(source in any::<String>()) {
        let green = GreenDocument::parse(source.clone());
        let fresh = parse(source);
        prop_assert_eq!(green.diagnostics(), fresh.diagnostics.clone());
        prop_assert_eq!(green.materialize(), fresh);
    }

    #[test]
    fn arbitrary_green_revisions_materialize_like_fresh_parse(
        old in any::<String>(),
        new in any::<String>(),
    ) {
        let green = GreenDocument::parse(old);
        let revision = green.reparse(new.clone()).document;
        let fresh = parse(new);
        prop_assert_eq!(revision.diagnostics(), fresh.diagnostics.clone());
        prop_assert_eq!(revision.materialize(), fresh);
    }
}
