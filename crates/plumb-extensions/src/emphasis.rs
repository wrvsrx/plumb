use std::ops::Range;

use plumb_core::{Block, Document, Inline, InlineContent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmphasisKind {
    Emphasis,
    Strong,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmphasisRecord {
    pub kind: EmphasisKind,
    pub range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EmphasisOutput {
    pub emphasis: Vec<EmphasisRecord>,
}

impl EmphasisOutput {
    pub fn emphasis_at_node_start(&self, start: usize) -> Option<&EmphasisRecord> {
        self.emphasis
            .iter()
            .find(|emphasis| emphasis.range.start == start)
    }
}

pub fn analyze_emphasis(document: &Document) -> EmphasisOutput {
    let mut output = EmphasisOutput::default();
    collect_blocks(&document.blocks, &mut output);
    output
}

fn collect_blocks(blocks: &[Block], output: &mut EmphasisOutput) {
    for block in blocks {
        let Block::Parsed(block) = block else {
            continue;
        };
        collect_inlines(&block.head, output);
        collect_blocks(&block.children, output);
    }
}

fn collect_inlines(content: &InlineContent, output: &mut EmphasisOutput) {
    for inline in &content.items {
        let Inline::Element {
            range,
            kind,
            content,
            ..
        } = inline
        else {
            continue;
        };
        let kind = match kind.as_str() {
            "*" => Some(EmphasisKind::Emphasis),
            "**" => Some(EmphasisKind::Strong),
            _ => None,
        };
        if let Some(kind) = kind {
            output.emphasis.push(EmphasisRecord {
                kind,
                range: range.clone(),
            });
        }
        collect_inlines(content, output);
    }
}

#[cfg(test)]
mod tests {
    use plumb_core::parse;

    use super::*;

    #[test]
    fn recognizes_nested_symbolic_emphasis_only() {
        let source = "`*[outer `**[strong]] `em[generic]\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_emphasis(&parsed.syntax);
        assert_eq!(output.emphasis.len(), 2);
        assert_eq!(output.emphasis[0].kind, EmphasisKind::Emphasis);
        assert_eq!(output.emphasis[1].kind, EmphasisKind::Strong);
        assert_eq!(
            &source[output.emphasis[0].range.clone()],
            "`*[outer `**[strong]]"
        );
        assert_eq!(&source[output.emphasis[1].range.clone()], "`**[strong]");
    }
}
