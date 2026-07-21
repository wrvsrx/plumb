use std::ops::Range;

use plumb_core::{parse, AttrItem, Attributes, Block, DiagnosticSeverity, Inline, ParsedDocument};
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

fn assert_lossless(parsed: &ParsedDocument) {
    assert_eq!(parsed.lossless.range, 0..parsed.source.len());
    assert_eq!(parsed.lossless.reconstruct(&parsed.source), parsed.source);
    if parsed.source.is_empty() {
        assert!(parsed.lossless.tokens.is_empty());
    } else {
        assert!(!parsed.lossless.tokens.is_empty());
    }

    let mut cursor = 0;
    for token in &parsed.lossless.tokens {
        assert_eq!(token.range.start, cursor);
        assert!(token.range.start < token.range.end);
        assert!(token.range.end <= parsed.source.len());
        assert!(parsed.source.is_char_boundary(token.range.start));
        assert!(parsed.source.is_char_boundary(token.range.end));
        cursor = token.range.end;
    }
    assert_eq!(cursor, parsed.source.len());

    assert_typed_ranges(parsed);

    for diagnostic in &parsed.diagnostics {
        assert!(diagnostic.range.start <= diagnostic.range.end);
        assert!(diagnostic.range.end <= parsed.source.len());
        assert!(parsed.source.is_char_boundary(diagnostic.range.start));
        assert!(parsed.source.is_char_boundary(diagnostic.range.end));
        for related in &diagnostic.related {
            assert!(related.start <= related.end);
            assert!(related.end <= parsed.source.len());
            assert!(parsed.source.is_char_boundary(related.start));
            assert!(parsed.source.is_char_boundary(related.end));
        }
    }

    assert_eq!(parsed.valid_syntax().is_some(), parsed.is_valid());
    assert_eq!(parsed.recovered_syntax(), &parsed.syntax);
    assert_eq!(
        parsed.is_valid(),
        !parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
    );

    let repeated = parse(parsed.source.clone());
    assert_eq!(repeated.syntax, parsed.syntax);
    assert_eq!(repeated.diagnostics, parsed.diagnostics);
    assert_eq!(repeated.lossless, parsed.lossless);
}

fn assert_typed_ranges(parsed: &ParsedDocument) {
    assert_range(&parsed.source, &parsed.syntax.range);
    let mut blocks = parsed.syntax.blocks.iter().collect::<Vec<_>>();
    let mut inline_contents = Vec::new();
    while let Some(block) = blocks.pop() {
        assert_range(&parsed.source, block.range());
        match block {
            Block::Parsed(block) => {
                if let Some(mark) = &block.mark {
                    assert_range(&parsed.source, &mark.range);
                    assert_range(&parsed.source, &mark.marker_range);
                    assert_attributes(&parsed.source, &mark.attrs);
                }
                inline_contents.push(&block.head);
                blocks.extend(&block.children);
            }
            Block::Verbatim(block) => {
                assert_range(&parsed.source, &block.opener_range);
                assert_range(&parsed.source, &block.text_range);
                assert_attributes(&parsed.source, &block.attrs);
            }
        }
    }

    while let Some(content) = inline_contents.pop() {
        assert_range(&parsed.source, &content.range);
        for inline in &content.items {
            match inline {
                Inline::Text { range, .. } | Inline::SoftBreak { range } => {
                    assert_range(&parsed.source, range);
                }
                Inline::Element {
                    range,
                    kind_range,
                    content,
                    attrs,
                    ..
                } => {
                    assert_range(&parsed.source, range);
                    assert_range(&parsed.source, kind_range);
                    assert_attributes(&parsed.source, attrs);
                    inline_contents.push(content);
                }
                Inline::Verbatim {
                    range,
                    text_range,
                    attrs,
                    ..
                } => {
                    assert_range(&parsed.source, range);
                    assert_range(&parsed.source, text_range);
                    assert_attributes(&parsed.source, attrs);
                }
            }
        }
    }
}

fn assert_attributes(source: &str, attrs: &Attributes) {
    if let Some(range) = &attrs.range {
        assert_range(source, range);
    }
    for item in &attrs.items {
        match item {
            AttrItem::Id { range, .. } | AttrItem::Class { range, .. } => {
                assert_range(source, range);
            }
            AttrItem::Pair {
                key_range,
                value,
                range,
                ..
            } => {
                assert_range(source, key_range);
                assert_range(source, &value.range);
                assert_range(source, range);
            }
        }
    }
}

fn assert_range(source: &str, range: &Range<usize>) {
    assert!(range.start <= range.end);
    assert!(range.end <= source.len());
    assert!(source.is_char_boundary(range.start));
    assert!(source.is_char_boundary(range.end));
}

fn arbitrary_utf8() -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..256)
        .prop_map(|characters| characters.into_iter().collect())
}

fn structural_source() -> impl Strategy<Value = String> {
    let atom = prop_oneof![
        Just("text".to_string()),
        Just("多字节 π".to_string()),
        Just("`node head".to_string()),
        Just("`-{.task #id key=value} item".to_string()),
        Just("`span[nested `em[value]]".to_string()),
        Just("`\"[raw ] safely]\"".to_string()),
        Just("```[escaped run]".to_string()),
        Just("`node{key=\"quote\\\" slash\\\\\"}".to_string()),
        Just("`node{key=\"bad\\q\"}".to_string()),
        Just("`node{.one".to_string()),
        Just("`kind[unclosed".to_string()),
        Just("`{language=text}".to_string()),
    ];
    let indentation = prop_oneof![
        (0usize..12).prop_map(|count| " ".repeat(count)),
        proptest::string::string_regex("[ \\t]{0,8}").unwrap(),
    ];
    let ending = prop_oneof![Just("\n"), Just("\r\n"), Just("")];
    prop::collection::vec((indentation, atom, ending), 0..64).prop_map(|lines| {
        let mut source = String::new();
        for (indent, atom, ending) in lines {
            source.push_str(&indent);
            source.push_str(&atom);
            source.push_str(ending);
        }
        source
    })
}

fn nested_source() -> impl Strategy<Value = String> {
    (0usize..256, any::<bool>(), any::<bool>()).prop_map(|(depth, inline, malformed)| {
        if inline {
            let mut source = "`x[".repeat(depth);
            source.push_str("深度");
            let closes = if malformed { depth / 2 } else { depth };
            source.push_str(&"]".repeat(closes));
            source.push('\n');
            source
        } else {
            let mut source = "`x ".repeat(depth);
            source.push_str(if malformed { "`" } else { "leaf" });
            source.push('\n');
            source
        }
    })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 512,
        max_shrink_iters: 10_000,
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "tests/lossless_properties.proptest-regressions",
        ))),
        ..ProptestConfig::default()
    })]

    #[test]
    fn arbitrary_utf8_sources_are_lossless_and_deterministic(source in arbitrary_utf8()) {
        assert_lossless(&parse(source));
    }

    #[test]
    fn structured_valid_and_malformed_sources_are_lossless(source in structural_source()) {
        assert_lossless(&parse(source));
    }

    #[test]
    fn nested_valid_and_malformed_sources_are_lossless(source in nested_source()) {
        assert_lossless(&parse(source));
    }
}
