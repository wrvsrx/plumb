use plumb_syntax::{parse, Block, DiagnosticSeverity, Inline, InlineContent};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    source: String,
    valid: bool,
    blocks: String,
    diagnostics: Vec<String>,
}

#[test]
fn recursive_owner_normative_corpus() {
    let cases: Vec<Case> = serde_json::from_str(include_str!("fixtures/strict-parser.json"))
        .expect("strict parser fixture must be valid JSON");
    for case in cases {
        let parsed = parse(case.source.clone());
        assert_eq!(
            parsed.is_valid(),
            case.valid,
            "{} validity: {:?}",
            case.name,
            parsed.diagnostics
        );
        assert_eq!(
            block_shape(&parsed.syntax.blocks),
            case.blocks,
            "{} tree",
            case.name
        );
        let diagnostics = parsed
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
            .map(|diagnostic| diagnostic.code.to_string())
            .collect::<Vec<_>>();
        assert_eq!(diagnostics, case.diagnostics, "{} diagnostics", case.name);
        assert_eq!(
            parsed.lossless.reconstruct(&parsed.source),
            case.source,
            "{} lossless reconstruction",
            case.name
        );
        let repeated = parse(case.source);
        assert_eq!(
            parsed.syntax, repeated.syntax,
            "{} deterministic tree",
            case.name
        );
        assert_eq!(
            parsed.diagnostics, repeated.diagnostics,
            "{} deterministic diagnostics",
            case.name
        );
    }
}

fn block_shape(blocks: &[Block]) -> String {
    let mut output = String::new();
    for block in blocks {
        match block {
            Block::Parsed(block) => {
                output.push('P');
                output.push_str(block.mark.as_ref().map_or("_", |mark| mark.marker.as_str()));
                output.push('(');
                output.push_str(&inline_shape(&block.content));
                output.push(')');
                if !block.children.is_empty() {
                    output.push('{');
                    output.push_str(&block_shape(&block.children));
                    output.push('}');
                }
            }
            Block::Verbatim(block) => {
                output.push('V');
                output.push_str(block.mark.as_ref().map_or("_", |mark| mark.marker.as_str()));
                output.push('(');
                output.push_str(&block.text.escape_default().to_string());
                output.push(')');
            }
        }
        output.push(';');
    }
    output
}

fn inline_shape(content: &InlineContent) -> String {
    let mut output = String::new();
    append_inline_shape(&content.items, &mut output);
    output
}

fn append_inline_shape(items: &[Inline], output: &mut String) {
    for inline in items {
        match inline {
            Inline::Text { text, .. } => {
                output.push('T');
                output.push_str(&format!("[{}]", text.escape_default()));
            }
            Inline::Space { text, .. } => output.push_str(&format!("S{}", text.len())),
            Inline::SoftBreak { .. } => output.push('B'),
            Inline::Group { mark, content, .. } => {
                output.push('G');
                output.push_str(mark.as_ref().map_or("_", |mark| mark.marker.as_str()));
                output.push('[');
                output.push_str(&inline_shape(content));
                output.push(']');
            }
            Inline::Verbatim {
                mark, text, braced, ..
            } => {
                output.push('V');
                output.push_str(mark.as_ref().map_or("_", |mark| mark.marker.as_str()));
                output.push(if *braced { '{' } else { '[' });
                output.push_str(&text.escape_default().to_string());
                output.push(if *braced { '}' } else { ']' });
            }
        }
    }
}
