//! Valid block forests over the original recovered revision, never a valid document.
use std::ops::Range;

use crate::{Block, DiagnosticSeverity, Document, ParsedBlock, ParsedDocument, ValidDocument};

/// Source-backed syntax checked for semantic consumption. This cannot be used
/// where export or authoring requires whole-document `ValidDocument`.
#[derive(Clone, Copy, Debug)]
pub struct SemanticDocument<'a> {
    source: &'a str,
    syntax: &'a Document,
}

impl<'a> SemanticDocument<'a> {
    pub fn source(self) -> &'a str {
        self.source
    }
    pub fn syntax(self) -> &'a Document {
        self.syntax
    }
}

impl<'a> From<ValidDocument<'a>> for SemanticDocument<'a> {
    fn from(valid: ValidDocument<'a>) -> Self {
        Self {
            source: valid.source(),
            syntax: valid.syntax(),
        }
    }
}

#[derive(Debug)]
pub struct ValidRegions {
    syntax: Document,
    excluded: Vec<Range<usize>>,
}

impl ValidRegions {
    pub fn excluded(&self) -> &[Range<usize>] {
        &self.excluded
    }
    pub(crate) fn view<'a>(&'a self, parsed: &'a ParsedDocument) -> SemanticDocument<'a> {
        SemanticDocument {
            source: &parsed.source,
            syntax: &self.syntax,
        }
    }
}

impl ParsedDocument {
    /// Isolate erroneous block owners and their descendants without reparsing.
    pub fn valid_regions(&self) -> ValidRegions {
        let mut nodes = Vec::new();
        let mut pending = self.syntax.blocks.iter().rev().collect::<Vec<_>>();
        while let Some(block) = pending.pop() {
            nodes.push(block);
            pending.extend(block.children().iter().rev());
        }
        let starts = nodes
            .iter()
            .map(|block| {
                let start = block.range().start;
                self.source[..start].rfind('\n').map_or(0, |i| i + 1)
            })
            .collect::<Vec<_>>();
        let mut excluded = Vec::new();
        for diagnostic in &self.diagnostics {
            if diagnostic.severity != DiagnosticSeverity::Error {
                continue;
            }
            let index = starts.partition_point(|start| *start <= diagnostic.range.start);
            if let Some(block) = index.checked_sub(1).and_then(|index| nodes.get(index)) {
                excluded.push(block.range().clone());
            }
        }
        excluded.sort_by_key(|range| (range.start, std::cmp::Reverse(range.end)));
        let mut roots: Vec<Range<usize>> = Vec::new();
        for range in excluded {
            if roots.last().is_none_or(|parent| range.start >= parent.end) {
                roots.push(range);
            }
        }
        // Postorder construction avoids recursive traversal and does not clone
        // discarded subtrees. Attribute projections are rebuilt from survivors.
        enum Work<'a> {
            Visit(&'a Block),
            Finish(&'a ParsedBlock, usize),
        }
        let mut work = self
            .syntax
            .blocks
            .iter()
            .rev()
            .map(Work::Visit)
            .collect::<Vec<_>>();
        let mut built = Vec::new();
        while let Some(item) = work.pop() {
            match item {
                Work::Visit(block) => {
                    let i = roots.partition_point(|range| range.start <= block.range().start);
                    if i > 0 && block.range().start < roots[i - 1].end {
                        continue;
                    }
                    match block {
                        Block::Verbatim(block) => built.push(Block::Verbatim(block.clone())),
                        Block::Parsed(block) => {
                            work.push(Work::Finish(block, built.len()));
                            work.extend(block.children.iter().rev().map(Work::Visit));
                        }
                    }
                }
                Work::Finish(block, child_start) => {
                    let children = built.split_off(child_start);
                    let mut mark = block.mark.clone();
                    if let Some(mark) = &mut mark {
                        mark.attrs = crate::parser::attributes_from_blocks(&self.source, &children);
                    }
                    built.push(Block::Parsed(ParsedBlock {
                        range: block.range.clone(),
                        mark,
                        content: block.content.clone(),
                        children,
                    }));
                }
            }
        }
        ValidRegions {
            syntax: Document {
                attrs: crate::parser::attributes_from_blocks(&self.source, &built),
                blocks: built,
                range: self.syntax.range.clone(),
            },
            excluded: roots,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::GreenDocument;

    #[test]
    fn invalid_owner_excludes_subtree_but_keeps_parent_and_siblings() {
        let source = "`parent Root\n `child Good\n `child {broken\n  `child Hidden\n `child 中文\n`next Healthy\n";
        let green = GreenDocument::parse(source);
        assert!(green.valid_syntax().is_none());
        let first = green.shards().next().unwrap().shard();
        let region = first.semantic_regions();
        let children = region.syntax().blocks[0].children();
        assert_eq!(children.len(), 2);
        assert_eq!(&source[children[1].range().clone()], "`child 中文\n");
        assert_eq!(first.excluded_regions().len(), 1);
        assert!(std::ptr::eq(
            region.syntax(),
            first.semantic_regions().syntax()
        ));
        assert_eq!(green.materialize().lossless.reconstruct(source), source);
        assert!(green.valid_syntax().is_none());
        assert_eq!(green.diagnostics().len(), 1);
    }

    #[test]
    fn indentation_and_eof_errors_exclude_their_owners() {
        for source in [
            "\t`bad Child\n`good Sibling\n",
            "`parent Root\n `child {broken",
        ] {
            let green = GreenDocument::parse(source);
            assert!(!green.is_valid());
            let first = green.shards().next().unwrap().shard();
            let view = first.semantic_regions();
            if source.starts_with('\t') {
                assert!(view.syntax().blocks.is_empty());
            } else {
                assert!(view.syntax().blocks[0].children().is_empty());
            }
        }
    }

    #[test]
    fn healthy_shards_borrow_original_tree_and_crlf_ranges_stay_source_backed() {
        let green = GreenDocument::parse("`good 中文\r\n`bad {broken\r\n`good 后续\r\n");
        for shard in green.shards() {
            let parsed = shard.shard().parsed();
            let view = shard.shard().semantic_regions();
            if parsed.is_valid() {
                assert!(std::ptr::eq(view.syntax(), &parsed.syntax));
                assert_eq!(view.source(), parsed.source);
            } else {
                assert!(view.syntax().blocks.is_empty());
            }
        }
    }
}
