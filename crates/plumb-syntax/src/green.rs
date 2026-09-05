use std::ops::Range;
use std::sync::Arc;

use crate::parser::{parse, shift_attributes, shift_blocks, shift_diagnostics, shift_tokens};
use crate::{
    AttrItem, Attributes, Diagnostic, Document, LosslessTree, ParsedDocument, SourceChange,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GreenDocument {
    source: String,
    shards: Vec<Arc<GreenShard>>,
    invalid_shards: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub struct GreenShard {
    parsed: ParsedDocument,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GreenParse {
    pub document: GreenDocument,
    pub old_reparsed_range: Range<usize>,
    pub reparsed_range: Range<usize>,
}

#[derive(Debug, Clone, Copy)]
pub struct GreenShardView<'a> {
    offset: usize,
    shard: &'a Arc<GreenShard>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidGreenDocument<'a> {
    document: &'a GreenDocument,
}

impl GreenDocument {
    pub fn parse(source: impl Into<String>) -> Self {
        let source = source.into();
        let boundaries = top_level_boundaries(&source);
        let shards = boundaries
            .windows(2)
            .map(|window| {
                Arc::new(GreenShard {
                    parsed: parse(source[window[0]..window[1]].to_string()),
                })
            })
            .collect::<Vec<_>>();
        let invalid_shards = shards
            .iter()
            .filter(|shard| !shard.parsed.is_valid())
            .count();
        Self {
            source,
            shards,
            invalid_shards,
        }
    }

    pub fn reparse(&self, source: impl Into<String>) -> GreenParse {
        let source = source.into();
        let (old_range, new_range) = changed_ranges(&self.source, &source);
        self.reparse_from_change(
            source,
            SourceChange {
                old_range,
                new_range,
            },
        )
    }

    pub fn reparse_from_change(
        &self,
        source: impl Into<String>,
        change: SourceChange,
    ) -> GreenParse {
        let source = source.into();
        if !valid_source_change(&self.source, &source, &change) {
            return self.reparse(source);
        }
        let starts = self.shard_starts();
        let old_start = starts
            .iter()
            .copied()
            .take_while(|start| *start < change.old_range.start)
            .last()
            .unwrap_or(0);
        let (old_end, new_end) = starts
            .iter()
            .copied()
            .filter(|start| *start >= change.old_range.end)
            .find_map(|old_end| {
                let suffix_len = self.source.len().checked_sub(old_end)?;
                let new_end = source.len().checked_sub(suffix_len)?;
                (new_end >= old_start && is_line_start(&source, new_end))
                    .then_some((old_end, new_end))
            })
            .unwrap_or((self.source.len(), source.len()));
        if old_start == 0 && old_end == self.source.len() {
            let end = source.len();
            return GreenParse {
                document: Self::parse(source),
                old_reparsed_range: 0..self.source.len(),
                reparsed_range: 0..end,
            };
        }

        let mut invalid_shards = 0;
        let mut shards = self
            .shards
            .iter()
            .zip(starts.iter().copied())
            .take_while(|(shard, start)| *start + shard.parsed.source.len() <= old_start)
            .map(|(shard, _)| {
                invalid_shards += usize::from(!shard.parsed.is_valid());
                Arc::clone(shard)
            })
            .collect::<Vec<_>>();
        let changed = Self::parse(source[old_start..new_end].to_string());
        invalid_shards += changed.invalid_shards;
        shards.extend(changed.shards);
        shards.extend(
            self.shards
                .iter()
                .zip(starts.iter().copied())
                .filter(|(_, start)| *start >= old_end)
                .map(|(shard, _)| {
                    invalid_shards += usize::from(!shard.parsed.is_valid());
                    Arc::clone(shard)
                }),
        );
        GreenParse {
            document: Self {
                source,
                shards,
                invalid_shards,
            },
            old_reparsed_range: old_start..old_end,
            reparsed_range: old_start..new_end,
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn is_valid(&self) -> bool {
        self.invalid_shards == 0
    }

    pub fn valid_syntax(&self) -> Option<ValidGreenDocument<'_>> {
        self.is_valid()
            .then_some(ValidGreenDocument { document: self })
    }

    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        if self.is_valid() {
            return Vec::new();
        }
        let mut diagnostics = Vec::new();
        for view in self.shards() {
            let mut local = view.shard.parsed.diagnostics.clone();
            shift_diagnostics(&mut local, view.offset as isize);
            diagnostics.append(&mut local);
        }
        diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
        diagnostics
    }

    pub fn shards(&self) -> impl ExactSizeIterator<Item = GreenShardView<'_>> {
        let mut offset = 0;
        self.shards.iter().map(move |shard| {
            let view = GreenShardView { offset, shard };
            offset += shard.parsed.source.len();
            view
        })
    }

    pub fn shard_at(&self, offset: usize) -> Option<GreenShardView<'_>> {
        if offset > self.source.len() || !self.source.is_char_boundary(offset) {
            return None;
        }
        let mut selected = None;
        for view in self.shards() {
            if view.offset > offset {
                break;
            }
            selected = Some(view);
            if offset < view.range().end {
                break;
            }
        }
        selected
    }

    pub fn materialize(&self) -> ParsedDocument {
        let mut blocks = Vec::new();
        let mut diagnostics = Vec::new();
        let mut tokens = Vec::new();
        let mut attribute_items = Vec::new();
        for view in self.shards() {
            let delta = view.offset as isize;
            let mut shard_blocks = view.shard.parsed.syntax.blocks.clone();
            shift_blocks(&mut shard_blocks, delta);
            blocks.append(&mut shard_blocks);
            let mut shard_diagnostics = view.shard.parsed.diagnostics.clone();
            shift_diagnostics(&mut shard_diagnostics, delta);
            diagnostics.append(&mut shard_diagnostics);
            let mut shard_tokens = view.shard.parsed.lossless.tokens.clone();
            shift_tokens(&mut shard_tokens, delta);
            tokens.append(&mut shard_tokens);
            let mut attrs = view.shard.parsed.syntax.attrs.clone();
            shift_attributes(&mut attrs, delta);
            attribute_items.append(&mut attrs.items);
        }
        diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
        ParsedDocument {
            source: self.source.clone(),
            lossless: LosslessTree {
                range: 0..self.source.len(),
                tokens,
            },
            syntax: Document {
                attrs: attributes_from_items(attribute_items),
                blocks,
                range: 0..self.source.len(),
            },
            diagnostics,
        }
    }

    fn shard_starts(&self) -> Vec<usize> {
        self.shards().map(|view| view.offset).collect()
    }
}

impl<'a> ValidGreenDocument<'a> {
    pub fn source(self) -> &'a str {
        self.document.source()
    }

    pub fn syntax(self) -> &'a GreenDocument {
        self.document
    }
}

impl GreenShard {
    pub fn parsed(&self) -> &ParsedDocument {
        &self.parsed
    }
}

impl<'a> GreenShardView<'a> {
    pub fn offset(self) -> usize {
        self.offset
    }

    pub fn range(self) -> Range<usize> {
        self.offset..self.offset + self.shard.parsed.source.len()
    }

    pub fn shard(self) -> &'a Arc<GreenShard> {
        self.shard
    }
}

fn top_level_boundaries(source: &str) -> Vec<usize> {
    let mut boundaries = vec![0];
    let mut start = 0;
    for line in source.split_inclusive('\n') {
        let content = line
            .strip_suffix('\n')
            .unwrap_or(line)
            .strip_suffix('\r')
            .unwrap_or_else(|| line.strip_suffix('\n').unwrap_or(line));
        if start > 0
            && !content.bytes().all(|byte| matches!(byte, b' ' | b'\t'))
            && !content.starts_with(' ')
        {
            boundaries.push(start);
        }
        start += line.len();
    }
    if boundaries.last().copied() != Some(source.len()) {
        boundaries.push(source.len());
    }
    boundaries
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

fn valid_source_change(old: &str, new: &str, change: &SourceChange) -> bool {
    change.old_range.start <= change.old_range.end
        && change.old_range.end <= old.len()
        && change.new_range.start <= change.new_range.end
        && change.new_range.end <= new.len()
        && change.old_range.start == change.new_range.start
        && old.is_char_boundary(change.old_range.start)
        && old.is_char_boundary(change.old_range.end)
        && new.is_char_boundary(change.new_range.start)
        && new.is_char_boundary(change.new_range.end)
        && old[..change.old_range.start] == new[..change.new_range.start]
        && old[change.old_range.end..] == new[change.new_range.end..]
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

fn attr_range(item: &AttrItem) -> &Range<usize> {
    match item {
        AttrItem::Id { range, .. }
        | AttrItem::Class { range, .. }
        | AttrItem::Pair { range, .. } => range,
    }
}
