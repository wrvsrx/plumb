use std::{collections::BTreeSet, ops::Range};

use plumb_syntax::{parse, AttrItem, Attributes, Block, Inline, InlineContent};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    source: String,
    valid: bool,
    blocks: Vec<ExpectedBlock>,
    diagnostics: Vec<ExpectedDiagnostic>,
    required_tokens: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ExpectedBlock {
    Paragraph {
        head: String,
        inline: String,
    },
    Marked {
        marker: String,
        attrs: Vec<String>,
        head: String,
        inline: String,
        children: Vec<ExpectedBlock>,
        #[serde(default)]
        raw: Option<String>,
    },
    Verbatim {
        attrs: Vec<String>,
        text: String,
    },
}

#[derive(Debug, Deserialize)]
struct ExpectedDiagnostic {
    code: String,
    range: ExpectedRange,
    #[serde(default)]
    related: Vec<ExpectedRange>,
}

#[derive(Debug, Deserialize)]
struct ExpectedRange {
    #[serde(default)]
    text: String,
    #[serde(default)]
    occurrence: usize,
    #[serde(default)]
    at: Option<usize>,
}

#[test]
fn strict_parser_normative_corpus() {
    let cases: Vec<Case> = serde_json::from_str(include_str!("fixtures/strict-parser.json"))
        .expect("strict parser corpus is valid JSON");
    assert!(cases.len() >= 16, "the normative corpus must remain broad");
    let covered_codes = cases
        .iter()
        .flat_map(|case| {
            case.diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code.as_str())
        })
        .collect::<BTreeSet<_>>();
    let required_codes = [
        "syntax.incomplete-introducer",
        "syntax.invalid-block-dispatch",
        "syntax.invalid-inline-dispatch",
        "syntax.invalid-verbatim-block-dispatch",
        "syntax.invalid-marker",
        "syntax.unclosed-inline",
        "syntax.unclosed-verbatim",
        "syntax.tab-indentation",
        "syntax.partial-indent",
        "syntax.short-verbatim-indent",
        "syntax.unattached-raw-tail",
        "syntax.unattached-bracket",
        "syntax.trailing-after-inline-member",
        "syntax.unclosed-verbatim-member",
    ];
    for case in &cases {
        let parsed = parse(case.source.clone());
        assert_eq!(parsed.is_valid(), case.valid, "{} validity", case.name);
        assert_blocks(&parsed.syntax.blocks, &case.blocks, &case.name);
        assert_eq!(
            parsed.diagnostics.len(),
            case.diagnostics.len(),
            "{} diagnostics: {:?}",
            case.name,
            parsed.diagnostics
        );
        for (actual, expected) in parsed.diagnostics.iter().zip(&case.diagnostics) {
            assert_eq!(actual.code, expected.code, "{} diagnostic code", case.name);
            assert_eq!(
                actual.range,
                locate(&case.source, &expected.range),
                "{} primary range for {}",
                case.name,
                expected.code
            );
            let related = expected
                .related
                .iter()
                .map(|range| locate(&case.source, range))
                .collect::<Vec<_>>();
            assert_eq!(actual.related, related, "{} related ranges", case.name);
        }

        let token_kinds = parsed
            .lossless
            .tokens
            .iter()
            .map(|token| format!("{:?}", token.kind))
            .collect::<Vec<_>>();
        for required in &case.required_tokens {
            assert!(
                token_kinds.contains(required),
                "{} missing token kind {required}: {token_kinds:?}",
                case.name
            );
        }
        assert_eq!(parsed.lossless.reconstruct(&case.source), case.source);
        let repeated = parse(case.source.clone());
        assert_eq!(
            repeated.syntax, parsed.syntax,
            "{} stable syntax",
            case.name
        );
        assert_eq!(
            repeated.diagnostics, parsed.diagnostics,
            "{} stable diagnostics",
            case.name
        );
        assert_eq!(
            repeated.lossless, parsed.lossless,
            "{} stable lossless tree",
            case.name
        );
    }
    for code in required_codes {
        assert!(covered_codes.contains(code), "corpus does not cover {code}");
    }
}

fn assert_blocks(actual: &[Block], expected: &[ExpectedBlock], case: &str) {
    assert_eq!(actual.len(), expected.len(), "{case} block count");
    for (actual, expected) in actual.iter().zip(expected) {
        match (actual, expected) {
            (Block::Parsed(actual), ExpectedBlock::Paragraph { head, inline })
                if actual.mark.is_none() =>
            {
                assert_eq!(&actual.head.plain_text(), head, "{case} paragraph head");
                assert_eq!(
                    &inline_shape(&actual.head),
                    inline,
                    "{case} paragraph inline"
                );
                assert!(actual.children.is_empty());
            }
            (
                Block::Parsed(actual),
                ExpectedBlock::Marked {
                    marker,
                    attrs,
                    head,
                    inline,
                    children,
                    raw,
                },
            ) => {
                let mark = actual.mark.as_ref().expect("expected marked block");
                assert_eq!(&mark.marker, marker, "{case} marker");
                assert_eq!(&attrs_shape(&mark.attrs), attrs, "{case} block attrs");
                assert_eq!(&actual.head.plain_text(), head, "{case} marked head");
                assert_eq!(&inline_shape(&actual.head), inline, "{case} marked inline");
                assert_blocks(&actual.children, children, case);
                assert_eq!(
                    actual.raw.as_ref().map(|payload| &payload.text),
                    raw.as_ref(),
                    "{case} marked raw tail"
                );
            }
            (Block::Verbatim(actual), ExpectedBlock::Verbatim { attrs, text }) => {
                assert!(attrs.is_empty(), "{case} anonymous raw attributes");
                assert_eq!(&actual.text, text, "{case} verbatim text");
            }
            _ => panic!("{case} block kind mismatch: {actual:?} vs {expected:?}"),
        }
    }
}

fn attrs_shape(attrs: &Attributes) -> Vec<String> {
    attrs
        .items
        .iter()
        .map(|item| match item {
            AttrItem::Id { value, .. } => format!("#{value}"),
            AttrItem::Class { value, .. } => format!(".{value}"),
            AttrItem::Pair { key, value, .. } => format!("{key}={}", value.decoded),
        })
        .collect()
}

fn inline_shape(content: &InlineContent) -> String {
    enum Action<'a> {
        Items(&'a [Inline], usize),
        Members(&'a [plumb_syntax::InlineMember], usize),
        CloseArgument,
        Separator,
    }

    let mut output = String::new();
    let mut stack = Vec::new();
    for argument in content.arguments.iter().rev() {
        stack.push(Action::Items(
            &content.items[argument.item_range.clone()],
            0,
        ));
        if argument.separator_range.is_some() {
            stack.push(Action::Separator);
        }
    }
    while let Some(action) = stack.pop() {
        let (items, index) = match action {
            Action::Separator => {
                output.push('|');
                continue;
            }
            Action::CloseArgument => {
                output.push(']');
                continue;
            }
            Action::Members(members, index) => {
                if index >= members.len() {
                    continue;
                }
                stack.push(Action::Members(members, index + 1));
                match &members[index] {
                    plumb_syntax::InlineMember::ParsedArgument(argument) => {
                        output.push_str("A[");
                        stack.push(Action::CloseArgument);
                        stack.push(Action::Items(argument.content.items.as_slice(), 0));
                    }
                    plumb_syntax::InlineMember::VerbatimArgument(argument) => {
                        output.push_str(&format!("R{}{:?}", argument.quote_count, argument.text));
                    }
                    plumb_syntax::InlineMember::Child { inline, .. } => {
                        output.push('C');
                        stack.push(Action::Items(std::slice::from_ref(inline.as_ref()), 0));
                    }
                }
                continue;
            }
            Action::Items(items, index) => (items, index),
        };
        if index >= items.len() {
            continue;
        }
        stack.push(Action::Items(items, index + 1));
        match &items[index] {
            Inline::Text { text, .. } => output.push_str(&format!("T{text:?}")),
            Inline::Space { text, .. } => output.push_str(&format!("W{text:?}")),
            Inline::SoftBreak { .. } => output.push('S'),
            Inline::Verbatim {
                text,
                quote_count,
                attrs,
                ..
            } => output.push_str(&format!("V{quote_count}{:?}{:?}", attrs_shape(attrs), text)),
            Inline::Element {
                kind,
                members,
                attrs,
                ..
            } => {
                output.push_str(&format!("E{kind}{:?}", attrs_shape(attrs)));
                stack.push(Action::Members(members, 0));
            }
        }
    }
    output
}

fn locate(source: &str, expected: &ExpectedRange) -> Range<usize> {
    if let Some(at) = expected.at {
        assert!(
            source.is_char_boundary(at),
            "range offset {at} is not UTF-8 aligned"
        );
        return at..at;
    }
    let (start, _) = source
        .match_indices(&expected.text)
        .nth(expected.occurrence)
        .unwrap_or_else(|| panic!("range text {:?} not found", expected.text));
    start..start + expected.text.len()
}
