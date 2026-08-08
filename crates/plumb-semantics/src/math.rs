use std::ops::Range;

use plumb_syntax::{
    AttachedContent, AttrItem, Attributes, Block, Diagnostic, DiagnosticSeverity, Document, Inline,
    InlineContent,
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
    if let Some(attached) = document.attrs.attached.as_deref() {
        match &attached.content {
            AttachedContent::Blocks(blocks) => collect_blocks(blocks, &mut output),
            AttachedContent::Inlines(content) => collect_inlines(content, &mut output),
        }
    }
    collect_blocks(&document.blocks, &mut output);
    output
}

fn collect_blocks(blocks: &[Block], output: &mut MathOutput) {
    for block in blocks {
        match block {
            Block::Verbatim(block) => recognize_verbatim(
                &block.kind,
                &block.attrs,
                block.range.clone(),
                MathKind::Display,
                output,
            ),
            Block::Parsed(block) => {
                collect_inlines(&block.head, output);
                collect_blocks(&block.children, output);
            }
        }
    }
}

fn collect_inlines(content: &InlineContent, output: &mut MathOutput) {
    for inline in &content.items {
        match inline {
            Inline::Verbatim {
                range, kind, attrs, ..
            } => recognize_verbatim(kind, attrs, range.clone(), MathKind::Inline, output),
            Inline::Element {
                range,
                attrs,
                content,
                ..
            } => {
                let _ = (range, attrs);
                collect_inlines(content, output);
            }
            Inline::Text { .. } | Inline::Space { .. } | Inline::SoftBreak { .. } => {}
        }
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
    fn recognizes_verbatim_math_and_rejects_other_owners() {
        let source = "Inline `$\"x^2\".\n\n`$\" {`@[display]}\n  x^2\n\n`$\" {`:[language mathml]}\n  <math/>\n\n`div Not raw {\n  `- $\n}\n\n`span[x]{`-[$]}\n";
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
            ["math.unsupported-language"]
        );
    }
}
