use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use crate::parser::{
    parse, reusable_boundary, shift_attributes, shift_blocks, shift_diagnostics, shift_tokens,
    starts_block_dispatch,
};
use crate::{
    AttrItem, Attributes, Diagnostic, Document, LosslessTree, ParsedDocument, SourceChange,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GreenDocument {
    source: String,
    shards: Vec<Arc<GreenShard>>,
    invalid_shards: usize,
}

/// Opaque process-local identity. Never persisted or derived from an address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GreenShardId(u64);

static NEXT_SHARD_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub struct GreenShard {
    id: GreenShardId,
    parsed: ParsedDocument,
    regions: OnceLock<crate::ValidRegions>,
}

impl PartialEq for GreenShard {
    fn eq(&self, other: &Self) -> bool {
        self.parsed == other.parsed
    }
}
impl Eq for GreenShard {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxInvalidation {
    Unchanged,
    OwnerReplacement,
    StructuralBoundary,
    FullParse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardProjection {
    pub id: GreenShardId,
    pub old_range: Range<usize>,
    pub new_range: Range<usize>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyntaxChangedFields {
    pub text_fields: bool,
    pub direct_children: bool,
    pub declarations: bool,
    pub structure: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxChangeSet {
    pub old_range: Range<usize>,
    pub new_range: Range<usize>,
    pub offset_delta: isize,
    pub removed: Vec<GreenShardId>,
    pub added: Vec<GreenShardId>,
    pub reused: Vec<ShardProjection>,
    pub reason: SyntaxInvalidation,
    pub changed_fields: SyntaxChangedFields,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GreenParse {
    pub document: GreenDocument,
    pub old_reparsed_range: Range<usize>,
    pub reparsed_range: Range<usize>,
    pub changes: SyntaxChangeSet,
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
                    id: GreenShardId(
                        NEXT_SHARD_ID
                            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| {
                                id.checked_add(1)
                            })
                            .expect("shard identity exhausted"),
                    ),
                    regions: OnceLock::new(),
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
        if source == self.source {
            return self.finish_reparse(self.clone(), 0..0, 0..0, SyntaxInvalidation::Unchanged);
        }
        let starts = self.shard_starts();
        let old_start = starts
            .iter()
            .copied()
            .filter(|start| *start <= change.old_range.start && reusable_boundary(&source, *start))
            .last()
            .unwrap_or(0);
        let (old_end, new_end) = starts
            .iter()
            .copied()
            .filter(|start| *start >= change.old_range.end)
            .find_map(|old_end| {
                let suffix_len = self.source.len().checked_sub(old_end)?;
                let new_end = source.len().checked_sub(suffix_len)?;
                (new_end >= old_start
                    && is_line_start(&source, new_end)
                    && reusable_boundary(&source, new_end))
                .then_some((old_end, new_end))
            })
            .unwrap_or((self.source.len(), source.len()));
        if old_start == 0 && old_end == self.source.len() {
            let end = source.len();
            return self.finish_reparse(
                Self::parse(source),
                0..self.source.len(),
                0..end,
                SyntaxInvalidation::FullParse,
            );
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
        let reason = if starts
            .iter()
            .any(|start| *start > old_start && *start <= change.old_range.start)
            || starts
                .iter()
                .any(|start| *start >= change.old_range.end && *start < old_end)
        {
            SyntaxInvalidation::StructuralBoundary
        } else {
            SyntaxInvalidation::OwnerReplacement
        };
        self.finish_reparse(
            Self {
                source,
                shards,
                invalid_shards,
            },
            old_start..old_end,
            old_start..new_end,
            reason,
        )
    }

    fn finish_reparse(
        &self,
        mut document: Self,
        old_range: Range<usize>,
        new_range: Range<usize>,
        reason: SyntaxInvalidation,
    ) -> GreenParse {
        // A byte hint can end inside a spelling shared by an inserted owner
        // and its successor. Recover exact prefix/suffix shard correspondence
        // after parsing; duplicate interior spellings are never matched.
        let prefix = self
            .shards
            .iter()
            .zip(&document.shards)
            .take_while(|(old, new)| Arc::ptr_eq(old, new) || old.parsed == new.parsed)
            .count();
        let suffix = self.shards[prefix..]
            .iter()
            .rev()
            .zip(document.shards[prefix..].iter().rev())
            .take_while(|(old, new)| Arc::ptr_eq(old, new) || old.parsed == new.parsed)
            .count();
        for index in 0..prefix {
            document.shards[index] = Arc::clone(&self.shards[index]);
        }
        for index in 0..suffix {
            let new_index = document.shards.len() - 1 - index;
            document.shards[new_index] = Arc::clone(&self.shards[self.shards.len() - 1 - index]);
        }
        let old = self
            .shards()
            .map(|view| (view.shard.id, view.range()))
            .collect::<std::collections::HashMap<_, _>>();
        let new = document
            .shards()
            .map(|view| (view.shard.id, view.range()))
            .collect::<std::collections::HashMap<_, _>>();
        let before = self
            .shards
            .iter()
            .filter(|shard| !new.contains_key(&shard.id))
            .map(|shard| &shard.parsed)
            .collect::<Vec<_>>();
        let after = document
            .shards
            .iter()
            .filter(|shard| !old.contains_key(&shard.id))
            .map(|shard| &shard.parsed)
            .collect::<Vec<_>>();
        let changed_fields = changed_fields(&before, &after);
        let changes = SyntaxChangeSet {
            changed_fields,
            offset_delta: document.source.len() as isize - self.source.len() as isize,
            old_range: old_range.clone(),
            new_range: new_range.clone(),
            reason,
            removed: self
                .shards()
                .filter(|view| !new.contains_key(&view.shard.id))
                .map(|view| view.shard.id)
                .collect(),
            added: document
                .shards()
                .filter(|view| !old.contains_key(&view.shard.id))
                .map(|view| view.shard.id)
                .collect(),
            reused: document
                .shards()
                .filter_map(|view| {
                    old.get(&view.shard.id).map(|range| ShardProjection {
                        id: view.shard.id,
                        old_range: range.clone(),
                        new_range: view.range(),
                    })
                })
                .collect(),
        };
        GreenParse {
            document,
            old_reparsed_range: old_range,
            reparsed_range: new_range,
            changes,
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
    /// Healthy shards borrow their original tree; invalid shards cache one
    /// source-preserving forest. The recovered tree and validity never change.
    pub fn semantic_regions(&self) -> crate::SemanticDocument<'_> {
        match self.parsed.valid_syntax() {
            Some(valid) => valid.into(),
            None => self.regions.get_or_init(|| self.parsed.valid_regions()).view(&self.parsed),
        }
    }

    pub fn excluded_regions(&self) -> &[Range<usize>] {
        if self.parsed.is_valid() { return &[]; }
        self.regions.get_or_init(|| self.parsed.valid_regions()).excluded()
    }

    pub fn id(&self) -> GreenShardId {
        self.id
    }

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

/// Top-level block starts. A line opens a new top-level block when it follows a
/// blank line or opens a marked/verbatim block entry; a plain line after a
/// nonblank line continues the open block, so it stays in the same shard.
fn top_level_boundaries(source: &str) -> Vec<usize> {
    let mut boundaries = vec![0];
    let mut start = 0;
    let mut after_blank = true;
    for line in source.split_inclusive('\n') {
        let content = line
            .strip_suffix('\n')
            .unwrap_or(line)
            .strip_suffix('\r')
            .unwrap_or_else(|| line.strip_suffix('\n').unwrap_or(line));
        let blank = content.bytes().all(|byte| matches!(byte, b' ' | b'\t'));
        if start > 0
            && !blank
            && !content.starts_with(' ')
            && (after_blank || starts_block_dispatch(source, start, start + content.len()))
        {
            boundaries.push(start);
        }
        after_blank = blank;
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

fn same_declarations(a: &Attributes, b: &Attributes) -> bool {
    fn value(item: &AttrItem) -> (u8, &str, &str) {
        match item {
            AttrItem::Id { value, .. } => (0, "", value.as_str()),
            AttrItem::Class { value, .. } => (1, "", value.as_str()),
            AttrItem::Pair { key, value, .. } => (2, key.as_str(), value.decoded.as_str()),
        }
    }
    a.items.iter().map(value).eq(b.items.iter().map(value))
}

fn changed_fields(before: &[&ParsedDocument], after: &[&ParsedDocument]) -> SyntaxChangedFields {
    use crate::Block;
    let mut fields = SyntaxChangedFields::default();
    if before.len() != after.len() {
        // No correspondence is asserted for added/removed owner runs.
        return SyntaxChangedFields {
            text_fields: true,
            direct_children: true,
            declarations: true,
            structure: true,
        };
    }
    for (old, new) in before.iter().zip(after) {
        fields.declarations |= !same_declarations(&old.syntax.attrs, &new.syntax.attrs);
        fields.structure |= old.is_valid() != new.is_valid();
        let mut stack = vec![(old.syntax.blocks.as_slice(), new.syntax.blocks.as_slice())];
        while let Some((old, new)) = stack.pop() {
            if old.len() != new.len() {
                fields.direct_children = true;
                fields.structure = true;
                // Added children may contain declaration-bearing descendants.
                fields.declarations = true;
            }
            for (old, new) in old.iter().zip(new) {
                match (old, new) {
                    (Block::Parsed(old), Block::Parsed(new)) => {
                        fields.structure |= old.mark.as_ref().map(|m| &m.marker)
                            != new.mark.as_ref().map(|m| &m.marker);
                        let empty = Attributes::default();
                        fields.declarations |= !same_declarations(
                            old.mark.as_ref().map_or(&empty, |m| &m.attrs),
                            new.mark.as_ref().map_or(&empty, |m| &m.attrs),
                        );
                        fields.text_fields |= old.content != new.content;
                        stack.push((&old.children, &new.children));
                    }
                    (Block::Verbatim(old), Block::Verbatim(new)) => {
                        fields.structure |= old.mark.as_ref().map(|m| &m.marker)
                            != new.mark.as_ref().map(|m| &m.marker);
                        fields.text_fields |= old.text != new.text;
                    }
                    _ => {
                        fields.structure = true;
                        fields.text_fields = true;
                        fields.direct_children = true;
                        fields.declarations = true;
                    }
                }
            }
        }
    }
    fields
}
