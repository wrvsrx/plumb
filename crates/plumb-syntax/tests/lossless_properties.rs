use std::ops::Range;

use plumb_syntax::{
    parse, parse_incremental, parse_incremental_from_change, Block, Inline, ParsedDocument,
    SourceChange,
};
use proptest::prelude::*;

fn assert_lossless(parsed: &ParsedDocument) {
    assert_eq!(
        parsed.lossless.reconstruct(&parsed.source),
        parsed.source,
        "lossless token reconstruction"
    );
    assert_eq!(parsed.lossless.range, 0..parsed.source.len());
    let mut cursor = 0;
    for token in &parsed.lossless.tokens {
        assert_eq!(token.range.start, cursor, "tokens must be contiguous");
        assert_range(&parsed.source, &token.range);
        assert!(token.range.start < token.range.end);
        cursor = token.range.end;
    }
    assert_eq!(cursor, parsed.source.len());

    let repeated = parse(parsed.source.clone());
    assert_eq!(parsed.syntax, repeated.syntax);
    assert_eq!(parsed.diagnostics, repeated.diagnostics);

    let mut blocks = parsed.syntax.blocks.iter().rev().collect::<Vec<_>>();
    let mut contents = Vec::new();
    while let Some(block) = blocks.pop() {
        assert_range(&parsed.source, block.range());
        match block {
            Block::Parsed(block) => {
                if let Some(mark) = &block.mark {
                    assert_range(&parsed.source, &mark.range);
                    assert_range(&parsed.source, &mark.marker_range);
                }
                contents.push(&block.content);
                blocks.extend(block.children.iter().rev());
            }
            Block::Verbatim(block) => {
                assert_range(&parsed.source, &block.opener_range);
                assert_range(&parsed.source, &block.quote_range);
                assert_range_or_empty(&parsed.source, &block.text_range);
                if let Some(mark) = &block.mark {
                    assert_range(&parsed.source, &mark.range);
                    assert_range(&parsed.source, &mark.marker_range);
                }
            }
        }
    }

    while let Some(content) = contents.pop() {
        assert_range_or_empty(&parsed.source, &content.range);
        for inline in &content.items {
            match inline {
                Inline::Text { range, .. }
                | Inline::Space { range, .. }
                | Inline::SoftBreak { range } => {
                    assert_range(&parsed.source, range);
                }
                Inline::Group {
                    range,
                    mark,
                    content,
                } => {
                    assert_range(&parsed.source, range);
                    if let Some(mark) = mark {
                        assert_range(&parsed.source, &mark.range);
                        assert_range(&parsed.source, &mark.marker_range);
                    }
                    contents.push(content);
                }
                Inline::Verbatim {
                    range,
                    mark,
                    text_range,
                    ..
                } => {
                    assert_range(&parsed.source, range);
                    assert_range_or_empty(&parsed.source, text_range);
                    if let Some(mark) = mark {
                        assert_range(&parsed.source, &mark.range);
                        assert_range(&parsed.source, &mark.marker_range);
                    }
                }
            }
        }
    }
}

fn assert_range(source: &str, range: &Range<usize>) {
    assert!(
        range.start < range.end,
        "expected nonempty range: {range:?}"
    );
    assert_range_or_empty(source, range);
}

fn assert_range_or_empty(source: &str, range: &Range<usize>) {
    assert!(range.start <= range.end, "reversed range: {range:?}");
    assert!(range.end <= source.len(), "range beyond source: {range:?}");
    assert!(source.is_char_boundary(range.start));
    assert!(source.is_char_boundary(range.end));
}

#[test]
fn representative_valid_and_invalid_sources_are_lossless() {
    for source in [
        "",
        "ordinary paragraph\n",
        "`node head\n child\n",
        "`node{head {nested}}\n",
        "`$\"x^2\" and `\"{quoted \\\" value}\"\n",
        "`rust\"\n fn main() {}\n",
        "before {unclosed\n",
        "unexpected } close\n",
        "partial\n   child\n  sibling\n",
    ] {
        assert_lossless(&parse(source));
    }
}

#[test]
fn deeply_nested_groups_do_not_use_the_call_stack() {
    let depth = 20_000;
    let valid = format!("{}x{}\n", "{".repeat(depth), "}".repeat(depth));
    let parsed = parse(valid.clone());
    assert!(parsed.is_valid(), "{:?}", parsed.diagnostics.first());
    assert_eq!(parsed.lossless.reconstruct(&parsed.source), valid);

    let invalid = format!("{}x\n", "{".repeat(depth));
    let parsed = parse(invalid);
    assert!(!parsed.is_valid());
    assert_eq!(
        parsed
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == "syntax.unclosed-inline-group")
            .count(),
        depth
    );
}

#[test]
fn incremental_revisions_match_fresh_parse_across_structural_boundaries() {
    let revisions = [
        (
            "`note First\n\n`note Middle\n\n child\n\n`note Last\n",
            "`note First\n\n`note Changed\n\n child\n\n`note Last\n",
        ),
        (
            "`note First\r\n\r\n`note Second 😀\r\n",
            "`note First\r\n\r\n `note Second x\r\n",
        ),
        (
            "`rust\"\n fn main() {}\n\n`note After\n",
            "`rust\"\n fn main() { println!(\"x\"); }\n\n`note After\n",
        ),
        (
            "before {valid}\n\n`note After\n",
            "before {invalid\n\n`note After\n",
        ),
        ("`note Existing\n", "`note Inserted\n\n`note Existing\n"),
        ("`note First\n\n`note Removed\n", "`note First\n"),
    ];
    for (old, new) in revisions {
        let previous = parse(old);
        let incremental = parse_incremental(&previous, new).document;
        let fresh = parse(new);
        assert_eq!(incremental, fresh, "{old:?} -> {new:?}");
    }
}

#[test]
fn incremental_parse_falls_back_before_cloning_deep_reused_subtrees() {
    let nested = format!("{}x{}", "{".repeat(2_000), "}".repeat(2_000));
    let old = format!("{nested}\n\n`note Old\n");
    let new = format!("{nested}\n\n`note New\n");
    let previous = parse(old);
    let incremental = parse_incremental(&previous, new.clone());
    assert_eq!(incremental.reparsed_range, 0..new.len());
    assert_eq!(incremental.document, parse(new));
}

#[test]
fn known_source_change_matches_diff_discovery_and_rejects_bad_provenance() {
    let old = "`note First\n\n`note Middle\n\n`note Last\n";
    let previous = parse(old);
    let start = old.find("Middle").unwrap();
    let mut new = old.to_string();
    new.replace_range(start..start + "Middle".len(), "Changed middle");
    let change = SourceChange {
        old_range: start..start + "Middle".len(),
        new_range: start..start + "Changed middle".len(),
    };
    let known = parse_incremental_from_change(&previous, new.clone(), change);
    assert_eq!(known.document, parse(new.clone()));
    assert_eq!(known, parse_incremental(&previous, new.clone()));

    let invalid = parse_incremental_from_change(
        &previous,
        new.clone(),
        SourceChange {
            old_range: 0..1,
            new_range: 0..1,
        },
    );
    assert_eq!(invalid.document, parse(new));
}

#[test]
fn edits_at_every_character_boundary_match_fresh_parse() {
    let sources = [
        "`note First\n\n `= key value\n\n`note Last\n",
        "paragraph {with `strong{nested} content}\r\n\r\nnext\r\n",
        "`rust\"\n fn main() {}\n more raw\n\n`note After\n",
        "`note Parent\n child continuation\n\n `note Nested\n\n`note Sibling\n",
        "before {unclosed\n\n`note recovered\n",
    ];
    for source in sources {
        let previous = parse(source);
        let boundaries = source
            .char_indices()
            .map(|(offset, _)| offset)
            .chain(std::iter::once(source.len()))
            .collect::<Vec<_>>();
        for offset in boundaries {
            for inserted in ["x", " ", "\n", "{", "`", "\""] {
                let mut changed = source.to_string();
                changed.insert_str(offset, inserted);
                assert_eq!(
                    parse_incremental(&previous, changed.clone()).document,
                    parse(changed),
                    "insert {inserted:?} at {offset} in {source:?}"
                );
            }
            if offset < source.len() {
                let next = offset + source[offset..].chars().next().unwrap().len_utf8();
                let mut changed = source.to_string();
                changed.replace_range(offset..next, "");
                assert_eq!(
                    parse_incremental(&previous, changed.clone()).document,
                    parse(changed),
                    "delete at {offset} in {source:?}"
                );
            }
        }
    }
}

proptest! {
    #[test]
    fn arbitrary_utf8_is_lossless(source in any::<String>()) {
        assert_lossless(&parse(source));
    }

    #[test]
    fn arbitrary_incremental_revision_matches_fresh_parse(
        old in any::<String>(),
        new in any::<String>(),
    ) {
        let previous = parse(old);
        let incremental = parse_incremental(&previous, new.clone()).document;
        prop_assert_eq!(incremental, parse(new));
    }

    #[test]
    fn local_owner_edits_reuse_boundaries_and_match_fresh_parse(
        titles in prop::collection::vec("[a-z]{1,20}", 3..30),
        selected in any::<usize>(),
        make_invalid in any::<bool>(),
    ) {
        let source = titles
            .iter()
            .map(|title| format!("`note {title}\n\n"))
            .collect::<String>();
        let selected = selected % titles.len();
        let mut changed_titles = titles;
        if make_invalid {
            changed_titles[selected].push('{');
        } else {
            changed_titles[selected].push('x');
        }
        let changed = changed_titles
            .iter()
            .map(|title| format!("`note {title}\n\n"))
            .collect::<String>();
        let previous = parse(source);
        let incremental = parse_incremental(&previous, changed.clone());
        prop_assert!(incremental.reparsed_range.end - incremental.reparsed_range.start < changed.len());
        prop_assert_eq!(incremental.document, parse(changed));
    }
}
