use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;

use plumb_syntax::{
    AttrItem, Attributes, Block, Diagnostic, Document, Inline, InlineContent, LosslessTree, Mark,
    ParsedDocument, SourceRange, SyntaxToken,
};

pub struct GreenDocument {
    source: String,
    shards: Vec<Arc<GreenShard>>,
    reparsed_range: SourceRange,
}

struct GreenShard {
    byte_len: usize,
    attributes: Attributes,
    blocks: Vec<Block>,
    diagnostics: Vec<Diagnostic>,
    tokens: Vec<SyntaxToken>,
}

impl GreenDocument {
    pub fn parse(source: String) -> Self {
        let parsed = plumb_syntax::parse(source.clone());
        Self::from_parsed(source, parsed)
    }

    fn from_parsed(source: String, mut parsed: ParsedDocument) -> Self {
        let mut boundaries = vec![0];
        boundaries.extend(
            parsed
                .syntax
                .blocks
                .iter()
                .map(|block| block.range().start)
                .filter(|start| *start > 0),
        );
        if boundaries.last().copied() != Some(source.len()) {
            boundaries.push(source.len());
        }
        let mut blocks = std::mem::take(&mut parsed.syntax.blocks)
            .into_iter()
            .peekable();
        let mut diagnostics = std::mem::take(&mut parsed.diagnostics)
            .into_iter()
            .peekable();
        let mut tokens = std::mem::take(&mut parsed.lossless.tokens)
            .into_iter()
            .peekable();
        let mut attributes = std::mem::take(&mut parsed.syntax.attrs.items)
            .into_iter()
            .peekable();
        let shards = boundaries
            .windows(2)
            .map(|window| {
                let range = window[0]..window[1];
                let delta = -(range.start as isize);
                let mut shard_blocks =
                    take_while(&mut blocks, |block| block.range().start < range.end);
                shift_blocks(&mut shard_blocks, delta);
                let mut shard_diagnostics = take_while(&mut diagnostics, |diagnostic| {
                    diagnostic.range.start < range.end
                });
                shift_diagnostics(&mut shard_diagnostics, delta);
                let mut shard_tokens =
                    take_while(&mut tokens, |token| token.range.start < range.end);
                shift_tokens(&mut shard_tokens, delta);
                let mut shard_attribute_items = take_while(&mut attributes, |attribute| {
                    attr_range(attribute).start < range.end
                });
                let mut shard_attributes =
                    attributes_from_items(std::mem::take(&mut shard_attribute_items));
                shift_attributes(&mut shard_attributes, delta);
                Arc::new(GreenShard {
                    byte_len: range.end - range.start,
                    attributes: shard_attributes,
                    blocks: shard_blocks,
                    diagnostics: shard_diagnostics,
                    tokens: shard_tokens,
                })
            })
            .collect();
        let end = source.len();
        Self {
            source,
            shards,
            reparsed_range: 0..end,
        }
    }

    pub fn reparse(&self, source: String) -> Self {
        let changed = changed_ranges(&self.source, &source).0;
        self.reparse_changed(source, changed)
    }

    pub fn reparse_changed(&self, source: String, changed: Range<usize>) -> Self {
        if changed.is_empty() {
            return Self::parse(source);
        }
        let starts = self.shard_starts();
        let old_start = starts
            .iter()
            .copied()
            .take_while(|start| *start < changed.start)
            .last()
            .unwrap_or(0);
        let (old_end, new_end) = starts
            .iter()
            .copied()
            .filter(|start| *start >= changed.end)
            .find_map(|old_end| {
                let suffix_len = self.source.len().checked_sub(old_end)?;
                let new_end = source.len().checked_sub(suffix_len)?;
                (new_end >= old_start && is_line_start(&source, new_end))
                    .then_some((old_end, new_end))
            })
            .unwrap_or((self.source.len(), source.len()));

        if old_start == 0 && old_end == self.source.len() {
            return Self::parse(source);
        }
        let mut shards = self
            .shards
            .iter()
            .zip(starts.iter().copied())
            .take_while(|(shard, start)| *start + shard.byte_len <= old_start)
            .map(|(shard, _)| Arc::clone(shard))
            .collect::<Vec<_>>();
        let changed_green = Self::parse(source[old_start..new_end].to_string());
        shards.extend(changed_green.shards);
        shards.extend(
            self.shards
                .iter()
                .zip(starts.iter().copied())
                .filter(|(_, start)| *start >= old_end)
                .map(|(shard, _)| Arc::clone(shard)),
        );
        Self {
            source,
            shards,
            reparsed_range: old_start..new_end,
        }
    }

    pub fn materialize(&self) -> ParsedDocument {
        let mut blocks = Vec::new();
        let mut diagnostics = Vec::new();
        let mut tokens = Vec::new();
        let mut attribute_items = Vec::new();
        let mut offset = 0;
        for shard in &self.shards {
            let delta = offset as isize;
            let mut shard_blocks = shard.blocks.clone();
            shift_blocks(&mut shard_blocks, delta);
            blocks.append(&mut shard_blocks);

            let mut shard_diagnostics = shard.diagnostics.clone();
            shift_diagnostics(&mut shard_diagnostics, delta);
            diagnostics.append(&mut shard_diagnostics);

            let mut shard_tokens = shard.tokens.clone();
            shift_tokens(&mut shard_tokens, delta);
            tokens.append(&mut shard_tokens);

            let mut attrs = shard.attributes.clone();
            shift_attributes(&mut attrs, delta);
            attribute_items.append(&mut attrs.items);
            offset += shard.byte_len;
        }
        diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
        let attributes = attributes_from_items(attribute_items);
        ParsedDocument {
            source: self.source.clone(),
            lossless: LosslessTree {
                range: 0..self.source.len(),
                tokens,
            },
            syntax: Document {
                attrs: attributes,
                blocks,
                range: 0..self.source.len(),
            },
            diagnostics,
        }
    }

    pub fn reparsed_bytes(&self) -> usize {
        self.reparsed_range.end - self.reparsed_range.start
    }

    pub fn reused_shards_from(&self, previous: &Self) -> usize {
        let previous = previous
            .shards
            .iter()
            .map(Arc::as_ptr)
            .collect::<HashSet<_>>();
        self.shards
            .iter()
            .filter(|shard| previous.contains(&Arc::as_ptr(shard)))
            .count()
    }

    pub fn shard_count(&self) -> usize {
        self.shards.len()
    }

    fn shard_starts(&self) -> Vec<usize> {
        let mut offset = 0;
        self.shards
            .iter()
            .map(|shard| {
                let start = offset;
                offset += shard.byte_len;
                start
            })
            .collect()
    }
}

pub fn validate_revisions() {
    for (old, new) in [
        (
            "`= title Old\n\n`note First\n\n`note Last\n",
            "`= title New\n\n`note First\n\n`note Last\n",
        ),
        (
            "`note First\r\n\r\n`note Second 😀\r\n",
            "`note First\r\n\r\n `note Second x\r\n",
        ),
        (
            "`rust\"\n fn main() {}\n\n`note After\n",
            "`rust\"\n fn main() { println!(\"x\"); }\n\n`note After\n",
        ),
        (
            "before {valid}\n\n`note After\n",
            "before {invalid\n\n`note After\n",
        ),
        ("`note Existing\n", "`note Inserted\n\n`note Existing\n"),
        ("`note First\n\n`note Removed\n", "`note First\n"),
    ] {
        let previous = GreenDocument::parse(old.to_string());
        assert_eq!(previous.materialize(), plumb_syntax::parse(old));
        let current = previous.reparse(new.to_string());
        assert_eq!(current.materialize(), plumb_syntax::parse(new));
    }
}

fn take_while<T>(
    values: &mut std::iter::Peekable<impl Iterator<Item = T>>,
    predicate: impl Fn(&T) -> bool,
) -> Vec<T> {
    let mut selected = Vec::new();
    while values.peek().is_some_and(&predicate) {
        selected.push(values.next().expect("peeked value exists"));
    }
    selected
}

fn changed_ranges(old: &str, new: &str) -> (Range<usize>, Range<usize>) {
    let prefix = old
        .chars()
        .zip(new.chars())
        .take_while(|(old, new)| old == new)
        .map(|(character, _)| character.len_utf8())
        .sum::<usize>();
    let suffix = old[prefix..]
        .chars()
        .rev()
        .zip(new[prefix..].chars().rev())
        .take_while(|(old, new)| old == new)
        .map(|(character, _)| character.len_utf8())
        .sum::<usize>();
    (prefix..old.len() - suffix, prefix..new.len() - suffix)
}

fn is_line_start(source: &str, offset: usize) -> bool {
    offset == 0 || source.as_bytes().get(offset.wrapping_sub(1)) == Some(&b'\n')
}

fn attributes_from_items(items: Vec<AttrItem>) -> Attributes {
    let range = match (items.first(), items.last()) {
        (Some(first), Some(last)) => Some(attr_range(first).start..attr_range(last).end),
        _ => None,
    };
    Attributes { range, items }
}

fn attr_range(item: &AttrItem) -> &SourceRange {
    match item {
        AttrItem::Id { range, .. }
        | AttrItem::Class { range, .. }
        | AttrItem::Pair { range, .. } => range,
    }
}

fn shift_blocks(blocks: &mut [Block], delta: isize) {
    let mut pending = blocks.iter_mut().rev().collect::<Vec<_>>();
    while let Some(block) = pending.pop() {
        match block {
            Block::Parsed(block) => {
                shift_range(&mut block.range, delta);
                if let Some(mark) = &mut block.mark {
                    shift_mark(mark, delta);
                }
                shift_inline_content(&mut block.content, delta);
                pending.extend(block.children.iter_mut().rev());
            }
            Block::Verbatim(block) => {
                shift_range(&mut block.range, delta);
                shift_range(&mut block.opener_range, delta);
                if let Some(mark) = &mut block.mark {
                    shift_mark(mark, delta);
                }
                shift_range(&mut block.quote_range, delta);
                shift_range(&mut block.text_range, delta);
            }
        }
    }
}

fn shift_inline_content(content: &mut InlineContent, delta: isize) {
    let mut pending = vec![content];
    while let Some(content) = pending.pop() {
        shift_range(&mut content.range, delta);
        for inline in &mut content.items {
            match inline {
                Inline::Text { range, .. }
                | Inline::Space { range, .. }
                | Inline::SoftBreak { range } => shift_range(range, delta),
                Inline::Group {
                    range,
                    mark,
                    content,
                } => {
                    shift_range(range, delta);
                    if let Some(mark) = mark {
                        shift_mark(mark, delta);
                    }
                    pending.push(content);
                }
                Inline::Verbatim {
                    range,
                    mark,
                    text_range,
                    ..
                } => {
                    shift_range(range, delta);
                    if let Some(mark) = mark {
                        shift_mark(mark, delta);
                    }
                    shift_range(text_range, delta);
                }
            }
        }
    }
}

fn shift_mark(mark: &mut Mark, delta: isize) {
    shift_range(&mut mark.range, delta);
    shift_range(&mut mark.marker_range, delta);
    shift_attributes(&mut mark.attrs, delta);
}

fn shift_attributes(attributes: &mut Attributes, delta: isize) {
    if let Some(range) = &mut attributes.range {
        shift_range(range, delta);
    }
    for item in &mut attributes.items {
        match item {
            AttrItem::Id {
                value_range, range, ..
            }
            | AttrItem::Class {
                value_range, range, ..
            } => {
                shift_range(value_range, delta);
                shift_range(range, delta);
            }
            AttrItem::Pair {
                key_range,
                value,
                range,
                ..
            } => {
                shift_range(key_range, delta);
                shift_range(&mut value.range, delta);
                shift_range(range, delta);
            }
        }
    }
}

fn shift_diagnostics(diagnostics: &mut [Diagnostic], delta: isize) {
    for diagnostic in diagnostics {
        shift_range(&mut diagnostic.range, delta);
        for related in &mut diagnostic.related {
            shift_range(related, delta);
        }
    }
}

fn shift_tokens(tokens: &mut [SyntaxToken], delta: isize) {
    for token in tokens {
        shift_range(&mut token.range, delta);
    }
}

fn shift_range(range: &mut SourceRange, delta: isize) {
    range.start = range.start.checked_add_signed(delta).unwrap();
    range.end = range.end.checked_add_signed(delta).unwrap();
}
