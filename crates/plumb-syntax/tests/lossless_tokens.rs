use plumb_syntax::{parse, SyntaxKind};

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
fn direct_declaration_children_have_token_granularity() {
    let source = "`node Head\n `@ id\n `+ class\n `= key a\"b\n";
    assert_eq!(
        tokens(source),
        vec![
            (SyntaxKind::Introducer, "`"),
            (SyntaxKind::Marker, "node"),
            (SyntaxKind::Whitespace, " "),
            (SyntaxKind::Text, "Head"),
            (SyntaxKind::LineEnding, "\n"),
            (SyntaxKind::Indentation, " "),
            (SyntaxKind::Introducer, "`"),
            (SyntaxKind::Marker, "@"),
            (SyntaxKind::Whitespace, " "),
            (SyntaxKind::Text, "id"),
            (SyntaxKind::LineEnding, "\n"),
            (SyntaxKind::Indentation, " "),
            (SyntaxKind::Introducer, "`"),
            (SyntaxKind::Marker, "+"),
            (SyntaxKind::Whitespace, " "),
            (SyntaxKind::Text, "class"),
            (SyntaxKind::LineEnding, "\n"),
            (SyntaxKind::Indentation, " "),
            (SyntaxKind::Introducer, "`"),
            (SyntaxKind::Marker, "="),
            (SyntaxKind::Whitespace, " "),
            (SyntaxKind::Text, "key"),
            (SyntaxKind::Whitespace, " "),
            (SyntaxKind::Text, "a\"b"),
            (SyntaxKind::LineEnding, "\n"),
        ]
    );
}

#[test]
fn parsed_and_verbatim_inlines_expose_their_delimiters() {
    let source = "A `span[x|+[c]] `\"raw\" Z\n";
    assert_eq!(
        tokens(source),
        vec![
            (SyntaxKind::Text, "A"),
            (SyntaxKind::Whitespace, " "),
            (SyntaxKind::Introducer, "`"),
            (SyntaxKind::InlineKind, "span"),
            (SyntaxKind::Delimiter, "["),
            (SyntaxKind::Text, "x"),
            (SyntaxKind::Delimiter, "|"),
            (SyntaxKind::InlineKind, "+"),
            (SyntaxKind::Delimiter, "["),
            (SyntaxKind::Text, "c"),
            (SyntaxKind::Delimiter, "]"),
            (SyntaxKind::Delimiter, "]"),
            (SyntaxKind::Whitespace, " "),
            (SyntaxKind::Introducer, "`"),
            (SyntaxKind::Delimiter, "\""),
            (SyntaxKind::RawPayload, "raw"),
            (SyntaxKind::Delimiter, "\""),
            (SyntaxKind::Whitespace, " "),
            (SyntaxKind::Text, "Z"),
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
    let source = "`text\n|\"\n raw\r\n \r\n";
    assert_eq!(
        tokens(source),
        vec![
            (SyntaxKind::Introducer, "`"),
            (SyntaxKind::Marker, "text"),
            (SyntaxKind::LineEnding, "\n"),
            (SyntaxKind::Delimiter, "|"),
            (SyntaxKind::Delimiter, "\""),
            (SyntaxKind::LineEnding, "\n"),
            (SyntaxKind::Indentation, " "),
            (SyntaxKind::RawPayload, "raw"),
            (SyntaxKind::LineEnding, "\r\n"),
            (SyntaxKind::Indentation, " "),
            (SyntaxKind::LineEnding, "\r\n"),
        ]
    );
}

#[test]
fn malformed_region_is_preserved_as_an_error_token() {
    let source = "`\n";
    assert_eq!(
        tokens(source),
        vec![(SyntaxKind::Error, "`"), (SyntaxKind::LineEnding, "\n"),]
    );
}
