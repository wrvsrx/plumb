use std::ops::Range;

use plumb_core::{Block, Document};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuoteRecord {
    pub range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuoteOutput {
    pub quotes: Vec<QuoteRecord>,
}

impl QuoteOutput {
    pub fn quote_at_node_start(&self, start: usize) -> Option<&QuoteRecord> {
        self.quotes.iter().find(|quote| quote.range.start == start)
    }
}

pub fn analyze_quotes(document: &Document) -> QuoteOutput {
    let mut output = QuoteOutput::default();
    collect_quotes(&document.blocks, &mut output);
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
        collect_quotes(&block.children, output);
    }
}

#[cfg(test)]
mod tests {
    use plumb_core::parse;

    use super::*;

    #[test]
    fn recognizes_nested_quote_blocks_only() {
        let source = "`> First\n  `> Nested\n`quote Generic\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_quotes(&parsed.syntax);
        assert_eq!(output.quotes.len(), 2);
        assert_eq!(
            &source[output.quotes[0].range.clone()],
            "`> First\n  `> Nested\n"
        );
        assert_eq!(&source[output.quotes[1].range.clone()], "`> Nested\n");
    }
}
