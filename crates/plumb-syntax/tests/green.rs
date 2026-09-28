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

#[test]
fn change_set_preserves_suffix_identity_and_reports_current_ranges() {
    let source = "`note First\n`note Middle\n`note Last\n";
    let old = GreenDocument::parse(source);
    let ids = old.shards().map(|s| s.shard().id()).collect::<Vec<_>>();
    let next = old.reparse(source.replace("Middle", "Longer 中"));
    assert_eq!(next.document.materialize(), parse(next.document.source()));
    assert_eq!(next.changes.removed, vec![ids[1]]);
    assert_eq!(next.changes.added.len(), 1);
    assert_eq!(next.changes.reused.len(), 2);
    assert_eq!(next.changes.reused[1].id, ids[2]);
    assert_eq!(
        next.changes.reused[1].new_range.start as isize
            - next.changes.reused[1].old_range.start as isize,
        next.changes.offset_delta
    );
    let unchanged = next.document.reparse(next.document.source());
    assert_eq!(
        unchanged.changes.reason,
        plumb_syntax::SyntaxInvalidation::Unchanged
    );
    assert!(unchanged.changes.added.is_empty());
    assert!(unchanged.changes.removed.is_empty());
    // Identity is not part of the source/tree correctness oracle.
    assert_eq!(
        unchanged.document,
        GreenDocument::parse(next.document.source())
    );
}

#[test]
fn shard_identity_never_follows_ordinal_across_insert_delete_or_merge() {
    let source = "`note First\n`note Last\n";
    let old = GreenDocument::parse(source);
    let last = old.shards().last().unwrap().shard().id();
    let inserted = old.reparse(source.replace("`note Last", "`note Added\n`note Last"));
    assert_eq!(
        inserted.document.shards().last().unwrap().shard().id(),
        last
    );
    let removed = inserted.document.reparse("`note Last\n");
    assert_eq!(removed.document.shards().last().unwrap().shard().id(), last);
    let merged = old.reparse("`note First\n Last\n");
    assert!(merged.changes.removed.contains(&last));
    assert!(merged.changes.reused.is_empty());
    assert_eq!(
        merged.document.materialize(),
        parse(merged.document.source())
    );
}

#[test]
fn change_fields_distinguish_text_declarations_children_and_offset_projection() {
    let source = "`note First\n `= key value\n`note Last\n";
    let old = GreenDocument::parse(source);
    let text = old.reparse(source.replace("First", "Other"));
    assert!(text.changes.changed_fields.text_fields);
    assert!(!text.changes.changed_fields.declarations);
    assert!(!text.changes.changed_fields.direct_children);
    let declaration = old.reparse(source.replace("value", "updated"));
    assert!(declaration.changes.changed_fields.declarations);
    let children = old.reparse(source.replace(" `= key value\n", ""));
    assert!(children.changes.changed_fields.direct_children);
    let prefix = old.reparse(format!("`note Prefix\n{source}"));
    assert_eq!(prefix.changes.reused.len(), old.shards().len());
    assert!(prefix
        .changes
        .reused
        .iter()
        .all(|shard| shard.old_range != shard.new_range));
}

#[test]
fn sequential_byte_changes_compose_before_overlapping_and_after_prior_edit() {
    let original = "prefix 😀 middle 后缀\n";
    let boundaries = original
        .char_indices()
        .map(|(i, _)| i)
        .chain([original.len()])
        .collect::<Vec<_>>();
    for &start in &boundaries {
        for &end in boundaries.iter().filter(|end| **end >= start) {
            let mut intermediate = original.to_string();
            intermediate.replace_range(start..end, "中");
            let first = SourceChange {
                old_range: start..end,
                new_range: start..start + "中".len(),
            };
            let next_boundaries = intermediate
                .char_indices()
                .map(|(i, _)| i)
                .chain([intermediate.len()])
                .collect::<Vec<_>>();
            for &second_start in &next_boundaries {
                for &second_end in next_boundaries.iter().filter(|end| **end >= second_start) {
                    let mut final_source = intermediate.clone();
                    final_source.replace_range(second_start..second_end, "xy");
                    let next = SourceChange {
                        old_range: second_start..second_end,
                        new_range: second_start..second_start + 2,
                    };
                    let combined = first.followed_by(&next).unwrap();
                    assert_eq!(
                        &original[..combined.old_range.start],
                        &final_source[..combined.new_range.start]
                    );
                    assert_eq!(
                        &original[combined.old_range.end..],
                        &final_source[combined.new_range.end..]
                    );
                    assert!(combined.old_range.start <= start && combined.old_range.end >= end);
                }
            }
        }
    }
}
