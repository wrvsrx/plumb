use std::ops::Range;

use plumb_core::{
    AttrItem, Attributes, Block, Diagnostic, DiagnosticSeverity, Document, Inline, InlineContent,
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

pub fn analyze_math(document: &Document) -> MathOutput {
    let mut output = MathOutput::default();
    collect_blocks(&document.blocks, &mut output);
    output
}

fn collect_blocks(blocks: &[Block], output: &mut MathOutput) {
    for block in blocks {
        match block {
            Block::Verbatim(block) => recognize(
                &block.attrs,
                block.range.clone(),
                MathKind::Display,
                true,
                output,
            ),
            Block::Parsed(block) => {
                if let Some(mark) = &block.mark {
                    recognize(
                        &mark.attrs,
                        block.range.clone(),
                        MathKind::Display,
                        false,
                        output,
                    );
                }
                collect_inlines(&block.head, output);
                collect_blocks(&block.children, output);
            }
        }
    }
}

fn collect_inlines(content: &InlineContent, output: &mut MathOutput) {
    for inline in &content.items {
        match inline {
            Inline::Verbatim { range, attrs, .. } => {
                recognize(attrs, range.clone(), MathKind::Inline, true, output)
            }
            Inline::Element {
                range,
                attrs,
                content,
                ..
            } => {
                recognize(attrs, range.clone(), MathKind::Inline, false, output);
                collect_inlines(content, output);
            }
            Inline::Text { .. } | Inline::SoftBreak { .. } => {}
        }
    }
}

fn recognize(
    attrs: &Attributes,
    range: Range<usize>,
    kind: MathKind,
    valid_owner: bool,
    output: &mut MathOutput,
) {
    let Some(math_range) = class_range(attrs, "$") else {
        return;
    };
    if !valid_owner {
        output.diagnostics.push(Diagnostic {
            code: "math.invalid-owner",
            severity: DiagnosticSeverity::Warning,
            message: "the '.$' math facet is only valid on verbatim inline and block nodes"
                .to_string(),
            range: math_range,
            related: Vec::new(),
        });
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

fn class_range(attrs: &Attributes, wanted: &str) -> Option<Range<usize>> {
    attrs.items.iter().find_map(|item| match item {
        AttrItem::Class { value, range } if value == wanted => Some(range.clone()),
        _ => None,
    })
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
    use plumb_core::parse;

    use super::*;

    #[test]
    fn recognizes_verbatim_math_and_rejects_other_owners() {
        let source = "Inline `[x^2]{.$}.\n`{.$ #display}\n  x^2\n`div{.$} Not raw\n`span[x]{.$}\n`{.$ language=mathml}\n  <math/>\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_math(&parsed.syntax);
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
            [
                "math.invalid-owner",
                "math.invalid-owner",
                "math.unsupported-language"
            ]
        );
    }
}
