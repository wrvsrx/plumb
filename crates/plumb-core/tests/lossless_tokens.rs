use plumb_core::{parse, SyntaxKind};

fn tokens(source: &str) -> Vec<(SyntaxKind, &str)> {
    let parsed = parse(source.to_owned());
    parsed
        .lossless
        .tokens
        .iter()
        .map(|token| (token.kind, token.text(source)))
        .collect()
}

#[test]
fn empty_source_has_an_empty_lossless_root() {
    let parsed = parse(String::new());
    assert_eq!(parsed.lossless.range, 0..0);
    assert!(parsed.lossless.tokens.is_empty());
}

#[test]
fn marked_block_attributes_have_token_granularity() {
    let source = "`node{#id .class key=\"a\\\"b\"} Head\r\n";
    assert_eq!(
        tokens(source),
        vec![
            (SyntaxKind::Introducer, "`"),
            (SyntaxKind::Marker, "node"),
            (SyntaxKind::Delimiter, "{"),
            (SyntaxKind::AttributePunctuation, "#"),
            (SyntaxKind::AttributeName, "id"),
            (SyntaxKind::Whitespace, " "),
            (SyntaxKind::AttributePunctuation, "."),
            (SyntaxKind::AttributeName, "class"),
            (SyntaxKind::Whitespace, " "),
            (SyntaxKind::AttributeName, "key"),
            (SyntaxKind::AttributePunctuation, "="),
            (SyntaxKind::Delimiter, "\""),
            (SyntaxKind::AttributeValue, "a"),
            (SyntaxKind::AttributeEscape, "\\\""),
            (SyntaxKind::AttributeValue, "b"),
            (SyntaxKind::Delimiter, "\""),
            (SyntaxKind::Delimiter, "}"),
            (SyntaxKind::Whitespace, " "),
            (SyntaxKind::Text, "Head"),
            (SyntaxKind::LineEnding, "\r\n"),
        ]
    );
}

#[test]
fn parsed_and_verbatim_inlines_expose_their_delimiters() {
    let source = "A `span[x]{.c} `[raw] Z\n";
    assert_eq!(
        tokens(source),
        vec![
            (SyntaxKind::Text, "A "),
            (SyntaxKind::Introducer, "`"),
            (SyntaxKind::InlineKind, "span"),
            (SyntaxKind::Delimiter, "["),
            (SyntaxKind::Text, "x"),
            (SyntaxKind::Delimiter, "]"),
            (SyntaxKind::Delimiter, "{"),
            (SyntaxKind::AttributePunctuation, "."),
            (SyntaxKind::AttributeName, "c"),
            (SyntaxKind::Delimiter, "}"),
            (SyntaxKind::Text, " "),
            (SyntaxKind::Introducer, "`"),
            (SyntaxKind::Delimiter, "["),
            (SyntaxKind::RawPayload, "raw"),
            (SyntaxKind::Delimiter, "]"),
            (SyntaxKind::Text, " Z"),
            (SyntaxKind::LineEnding, "\n"),
        ]
    );
}

#[test]
fn strengthened_verbatim_quotes_are_individual_delimiters() {
    let source = "`\"\"[a ]\" b]\"\"\n";
    assert_eq!(
        tokens(source),
        vec![
            (SyntaxKind::Introducer, "`"),
            (SyntaxKind::Delimiter, "\""),
            (SyntaxKind::Delimiter, "\""),
            (SyntaxKind::Delimiter, "["),
            (SyntaxKind::RawPayload, "a ]\" b"),
            (SyntaxKind::Delimiter, "]"),
            (SyntaxKind::Delimiter, "\""),
            (SyntaxKind::Delimiter, "\""),
            (SyntaxKind::LineEnding, "\n"),
        ]
    );
}

#[test]
fn raw_block_separates_structural_prefix_payload_and_crlf() {
    let source = "`{language=text}\r\n  raw\r\n\r\n";
    assert_eq!(
        tokens(source),
        vec![
            (SyntaxKind::Introducer, "`"),
            (SyntaxKind::Delimiter, "{"),
            (SyntaxKind::AttributeName, "language"),
            (SyntaxKind::AttributePunctuation, "="),
            (SyntaxKind::AttributeValue, "text"),
            (SyntaxKind::Delimiter, "}"),
            (SyntaxKind::LineEnding, "\r\n"),
            (SyntaxKind::Indentation, "  "),
            (SyntaxKind::RawPayload, "raw"),
            (SyntaxKind::LineEnding, "\r\n"),
            (SyntaxKind::LineEnding, "\r\n"),
        ]
    );
}

#[test]
fn malformed_region_is_preserved_as_an_error_token() {
    let source = "`node}bad\n";
    assert_eq!(
        tokens(source),
        vec![
            (SyntaxKind::Error, "`"),
            (SyntaxKind::Error, "node"),
            (SyntaxKind::Error, "}"),
            (SyntaxKind::Text, "bad"),
            (SyntaxKind::LineEnding, "\n"),
        ]
    );
}
