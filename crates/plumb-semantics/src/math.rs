use std::ops::Range;

use plumb_syntax::{
    AttrItem, Attributes, Block, Diagnostic, DiagnosticSeverity, Inline, InlineContent,
    ValidDocument,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathKind {
    Inline,
    Display,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathRecord {
    pub range: Range<usize>,
    pub kind: MathKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MathOutput {
    pub records: Vec<MathRecord>,
    pub diagnostics: Vec<Diagnostic>,
}

impl MathOutput {
    pub fn math_at_node_start(&self, start: usize) -> Option<&MathRecord> {
        self.records
            .iter()
            .find(|record| record.range.start == start)
    }
}

pub fn analyze_math(valid: ValidDocument<'_>) -> MathOutput {
    let document = valid.syntax();
    let mut output = MathOutput::default();
    collect_blocks(&document.blocks, &mut output);
    output
}

fn collect_blocks(blocks: &[Block], output: &mut MathOutput) {
    for block in blocks {
        match block {
            Block::Verbatim(block) => {
                if let Some(mark) = &block.mark {
                    recognize_verbatim(
                        &mark.marker,
                        &mark.attrs,
                        block.range.clone(),
                        MathKind::Display,
                        output,
                    );
                }
            }
            Block::Parsed(block) => {
                collect_inlines(&block.content, output);
                collect_blocks(&block.children, output);
            }
        }
    }
}

fn collect_inlines(content: &InlineContent, output: &mut MathOutput) {
    for inline in &content.items {
        collect_inline(inline, output);
    }
}

fn collect_inline(inline: &Inline, output: &mut MathOutput) {
    match inline {
        Inline::Verbatim { range, mark, .. } => {
            if let Some(mark) = mark {
                recognize_verbatim(
                    &mark.marker,
                    &mark.attrs,
                    range.clone(),
                    MathKind::Inline,
                    output,
                );
            }
        }
        Inline::Group { content, .. } => collect_inlines(content, output),
        Inline::Text { .. } | Inline::Space { .. } => {}
    }
}

fn recognize_verbatim(
    verbatim_kind: &str,
    attrs: &Attributes,
    range: Range<usize>,
    kind: MathKind,
    output: &mut MathOutput,
) {
    if verbatim_kind != "$" {
        return;
    }
    if let Some((language, language_range)) = pair(attrs, "language") {
        if language != "tex" {
            output.diagnostics.push(Diagnostic {
                code: "math.unsupported-language",
                severity: DiagnosticSeverity::Warning,
                message: "math language must be 'tex'".to_string(),
                range: language_range,
                related: Vec::new(),
            });
            return;
        }
    }
    output.records.push(MathRecord { range, kind });
}

fn pair<'a>(attrs: &'a Attributes, wanted: &str) -> Option<(&'a str, Range<usize>)> {
    attrs.items.iter().find_map(|item| match item {
        AttrItem::Pair {
            key, value, range, ..
        } if key == wanted => Some((value.decoded.as_str(), range.clone())),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use plumb_syntax::parse;

    use super::*;

    #[test]
    fn recognizes_verbatim_math_and_ignores_dollar_facets() {
        let source = "Inline `$\"x^2\".\n\n`$\n `@ display\n\n|\"\n x^2\n\n`$\n `= language|mathml\n\n|\"\n <math/>\n\n`div Not raw\n `+ $\n\n`span[x|+[$]]\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_math(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(
            output
                .records
                .iter()
                .map(|record| record.kind)
                .collect::<Vec<_>>(),
            [MathKind::Inline, MathKind::Display]
        );
        assert_eq!(
            output
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            ["math.unsupported-language"]
        );
    }
}
