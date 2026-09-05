use std::ops::Range;

use plumb_syntax::{Block, ValidDocument};

use crate::{RelativeSemanticRecord, SemanticRecordView, SemanticRecords};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuoteRecord {
    pub range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuoteOutput {
    pub quotes: SemanticRecords<QuoteRecord>,
}

pub type QuoteRecordView<'a> = SemanticRecordView<'a, QuoteRecord>;

impl QuoteRecordView<'_> {
    pub fn range(self) -> Range<usize> {
        self.record
            .range
            .start
            .checked_add_signed(self.offset)
            .unwrap()
            ..self
                .record
                .range
                .end
                .checked_add_signed(self.offset)
                .unwrap()
    }
}

impl QuoteOutput {
    pub fn quote_at_node_start(&self, start: usize) -> Option<QuoteRecordView<'_>> {
        self.quotes.view_at_start(start)
    }
}

impl RelativeSemanticRecord for QuoteRecord {
    fn start(&self) -> usize {
        self.range.start
    }

    fn shift(&mut self, delta: isize) {
        self.range.start = self.range.start.checked_add_signed(delta).unwrap();
        self.range.end = self.range.end.checked_add_signed(delta).unwrap();
    }
}

pub fn analyze_quotes(valid: ValidDocument<'_>) -> QuoteOutput {
    let document = valid.syntax();
    let mut output = QuoteOutput::default();
    for block in document
        .blocks
        .iter()
        .filter(|block| !crate::is_document_declaration(block))
    {
        collect_quotes(std::slice::from_ref(block), &mut output);
    }
    output
}

fn collect_quotes(blocks: &[Block], output: &mut QuoteOutput) {
    for block in blocks {
        let Block::Parsed(block) = block else {
            continue;
        };
        if block.mark.as_ref().is_some_and(|mark| mark.marker == ">") {
            output.quotes.push(QuoteRecord {
                range: block.range.clone(),
            });
        }
        for child in crate::body_children(block) {
            collect_quotes(std::slice::from_ref(child), output);
        }
    }
}

#[cfg(test)]
mod tests {
    use plumb_syntax::parse;

    use super::*;

    #[test]
    fn recognizes_nested_quote_blocks_only() {
        let source = "`> First\n  `> Nested\n`quote Generic\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_quotes(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(output.quotes.len(), 2);
        assert_eq!(
            &source[output.quotes.get(0).unwrap().range.clone()],
            "`> First\n  `> Nested\n"
        );
        assert_eq!(
            &source[output.quotes.get(1).unwrap().range.clone()],
            "`> Nested\n"
        );
    }
}
