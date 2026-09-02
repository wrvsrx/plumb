use std::ops::Range;

use plumb_syntax::{parse, Block, Inline, ParsedDocument};
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
        for datum in &content.data {
            assert_range_or_empty(&parsed.source, &datum.range);
            assert!(datum.item_range.end <= content.items.len());
        }
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

proptest! {
    #[test]
    fn arbitrary_utf8_is_lossless(source in any::<String>()) {
        assert_lossless(&parse(source));
    }
}
