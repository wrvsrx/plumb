//! Explicit, non-inherited owner classifications.
use crate::RelativeSemanticRecord;
use plumb_syntax::{Block, Diagnostic, DiagnosticSeverity, Inline, ParsedBlock, SemanticDocument};
use serde::{Deserialize, Serialize};
use std::ops::Range;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagSet {
    pub values: Vec<String>,
    pub declarations: Vec<Range<usize>>,
    pub invalid: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TagOwner {
    Document,
    Block,
}

/// Location identity belongs to one source revision; explicit ids remain optional.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaggedOwner {
    pub owner: TagOwner,
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
    pub title: String,
    pub id: Option<String>,
    pub tags: TagSet,
}
impl RelativeSemanticRecord for TaggedOwner {
    fn start(&self) -> usize {
        self.range.start
    }
    fn shift(&mut self, delta: isize) {
        shift(&mut self.range, delta);
        shift(&mut self.selection_range, delta);
        self.tags.shift(delta);
    }
}
fn shift(range: &mut Range<usize>, delta: isize) {
    range.start = range.start.checked_add_signed(delta).unwrap();
    range.end = range.end.checked_add_signed(delta).unwrap();
}

pub(crate) fn is_tags_property(block: &ParsedBlock) -> bool {
    block.mark.as_ref().is_some_and(|m| m.marker == "=")
        && crate::metadata::direct_property_parts(block).is_some_and(|(key, _, _)| key == "tags")
}

fn value(content: &plumb_syntax::InlineContent) -> Option<String> {
    let mut pending: Vec<_> = content.items.iter().collect();
    while let Some(inline) = pending.pop() {
        match inline {
            Inline::Text { .. }
            | Inline::Space { .. }
            | Inline::SoftBreak { .. }
            | Inline::Verbatim { mark: None, .. } => {}
            Inline::Group {
                mark: None,
                content,
                ..
            } => pending.extend(&content.items),
            _ => return None,
        }
    }
    let text = content.plain_text().trim().to_owned();
    (!text.is_empty()).then_some(text)
}

impl TagSet {
    /// Decode direct tag declarations on an already parsed owner.
    pub fn from_blocks(blocks: &[Block]) -> Self {
        let mut result = Self::default();
        let mut seen = std::collections::HashSet::new();
        for block in blocks {
            let Block::Parsed(property) = block else {
                continue;
            };
            if !is_tags_property(property) {
                continue;
            }
            result.declarations.push(property.range.clone());
            let (_, _, scalar) = crate::metadata::direct_property_parts(property).unwrap();
            let mut values = Vec::new();
            if let Some(scalar) = scalar {
                match value(&scalar) {
                    Some(v) => values.push(v),
                    None => result.invalid = true,
                }
                // owner_semantic_view filters declarations, but a label is plain text.
                if property
                    .content
                    .items
                    .iter()
                    .any(|i| matches!(i, Inline::Group { mark: Some(_), .. }))
                {
                    result.invalid = true;
                }
            } else {
                for child in &property.children {
                    let Block::Parsed(item) = child else {
                        result.invalid = true;
                        continue;
                    };
                    if !item.children.is_empty()
                        || !item.mark.as_ref().is_some_and(|m| m.marker == "+")
                    {
                        result.invalid = true;
                        continue;
                    }
                    match value(&item.content) {
                        Some(v) => values.push(v),
                        None => result.invalid = true,
                    }
                }
            }
            result.invalid |= values.is_empty();
            for v in values {
                if seen.insert(v.clone()) {
                    result.values.push(v);
                }
            }
        }
        result.invalid |= result.declarations.len() > 1;
        if result.invalid {
            result.values.clear();
        }
        result
    }
    pub(crate) fn from_green(document: &plumb_syntax::GreenDocument) -> Self {
        let mut result = Self::default();
        for view in document.shards() {
            let mut local = Self::from_blocks(&view.shard().semantic_regions().syntax().blocks);
            local.shift(view.offset() as isize);
            result.invalid |= local.invalid;
            result.declarations.extend(local.declarations);
            result.values.extend(local.values);
        }
        result.invalid |= result.declarations.len() > 1
            || crate::metadata::invalid_root_properties(document)
                .iter()
                .any(|(key, _)| key.is_empty() || key == "tags");
        if result.invalid {
            result.values.clear();
        }
        result
    }
    pub(crate) fn shift(&mut self, delta: isize) {
        for range in &mut self.declarations {
            shift(range, delta);
        }
    }
    pub(crate) fn diagnostics(&self) -> Vec<Diagnostic> {
        if !self.invalid {
            return Vec::new();
        }
        if self.declarations.len() > 1 {
            self.declarations
                .iter()
                .skip(1)
                .map(|range| Diagnostic {
                    code: "tags.duplicate-property",
                    severity: DiagnosticSeverity::Warning,
                    message: "tags must be declared at most once per owner".into(),
                    range: range.clone(),
                    related: vec![self.declarations[0].clone()],
                })
                .collect()
        } else {
            self.declarations.iter().map(|range| Diagnostic {
                code: "tags.invalid-value", severity: DiagnosticSeverity::Warning,
                message: "tags must be a nonempty plain label or a sequence of direct leaf '+' labels".into(),
                range: range.clone(), related: Vec::new(),
            }).collect()
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TagOutput {
    pub owners: Vec<TaggedOwner>,
    pub diagnostics: Vec<Diagnostic>,
}
pub(crate) fn analyze_blocks(document: SemanticDocument<'_>) -> TagOutput {
    let mut output = TagOutput::default();
    let mut blocks: Vec<_> = document.syntax().blocks.iter().rev().collect();
    while let Some(block) = blocks.pop() {
        if crate::is_document_declaration(block) {
            continue;
        }
        let Block::Parsed(block) = block else {
            continue;
        };
        let tags = TagSet::from_blocks(&block.children);
        output.diagnostics.extend(tags.diagnostics());
        if !tags.values.is_empty() {
            output.owners.push(TaggedOwner {
                owner: TagOwner::Block,
                range: block.range.clone(),
                selection_range: crate::inline_selection_range(&block.content),
                title: crate::semantic_plain_text(&block.content),
                id: block
                    .mark
                    .as_ref()
                    .and_then(|m| m.attrs.id())
                    .map(str::to_owned),
                tags,
            });
        }
        blocks.extend(block.children.iter().rev());
    }
    output
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagCompletionContext {
    pub replace: Range<usize>,
    pub query: String,
}

/// Recovered plain-prefix assist; existing grouped/raw labels can be edited normally.
pub fn tag_completion_context(
    document: &plumb_syntax::ParsedDocument,
    offset: usize,
) -> Option<TagCompletionContext> {
    let source = &document.source;
    if !source.is_char_boundary(offset) {
        return None;
    }
    let context = |content: &plumb_syntax::InlineContent, start: usize| {
        let prefix = source.get(start..offset)?;
        if offset > content.range.end || prefix.contains(['\n', '\r', '`', '{', '}']) {
            return None;
        }
        let start = start + prefix.len() - prefix.trim_start_matches(' ').len();
        Some(TagCompletionContext {
            replace: start..content.range.end,
            query: source.get(start..offset)?.to_owned(),
        })
    };
    let mut owners = vec![document.syntax.blocks.as_slice()];
    while let Some(children) = owners.pop() {
        for child in children {
            let Block::Parsed(block) = child else {
                continue;
            };
            if is_tags_property(block) {
                if block.children.is_empty() {
                    let first = block.content.items.iter().find(|i| !i.is_whitespace())?;
                    let key_end = plumb_syntax::inline_range(first).end;
                    if source
                        .get(key_end..offset)
                        .is_some_and(|s| s.starts_with(' '))
                    {
                        if let Some(c) = context(&block.content, key_end) {
                            return Some(c);
                        }
                    }
                } else {
                    for item in &block.children {
                        if let Block::Parsed(item) = item {
                            if item.children.is_empty()
                                && item.mark.as_ref().is_some_and(|m| m.marker == "+")
                            {
                                if let Some(c) = context(&item.content, item.content.range.start) {
                                    return Some(c);
                                }
                            }
                        }
                    }
                }
            } else if !crate::is_document_declaration(child) {
                owners.push(&block.children);
            }
        }
    }
    None
}

pub fn green_tag_completion_context(
    document: &plumb_syntax::GreenDocument,
    offset: usize,
) -> Option<TagCompletionContext> {
    let view = document.shard_at(offset)?;
    let mut context =
        tag_completion_context(view.shard().parsed(), offset.checked_sub(view.offset())?)?;
    shift(&mut context.replace, view.offset() as isize);
    Some(context)
}

pub(crate) fn attribute_context(
    document: &plumb_syntax::ParsedDocument,
    offset: usize,
) -> Option<crate::AttributeCompletionContext> {
    let mut owners = vec![document.syntax.blocks.as_slice()];
    while let Some(children) = owners.pop() {
        for child in children {
            let Block::Parsed(block) = child else {
                continue;
            };
            if block.mark.as_ref().is_some_and(|m| m.marker == "=") && block.children.is_empty() {
                let elements = crate::owner_semantic_view(&block.content);
                if elements.positional.len() <= 1 && offset <= block.content.range.end {
                    let start = elements
                        .positional
                        .first()
                        .map_or(block.content.range.end, |v| v.range.start);
                    if let Some(prefix) = document.source.get(start..offset) {
                        if "tags".starts_with(prefix) {
                            let duplicate = children.iter().any(|other| matches!(other, Block::Parsed(b) if b.range != block.range && is_tags_property(b)));
                            return Some(crate::AttributeCompletionContext {
                                replace: block.range.start..offset,
                                completions: if duplicate {
                                    Vec::new()
                                } else {
                                    vec![crate::AttributeCompletion {
                                        label: "tags",
                                        detail: "explicit owner tags",
                                        new_text: "`= tags ".into(),
                                    }]
                                },
                            });
                        }
                    }
                }
            } else if !crate::is_document_declaration(child) {
                owners.push(&block.children);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    fn analyze(source: &str) -> crate::DocumentOutput {
        crate::analyze_green_regions(Arc::new(plumb_syntax::GreenDocument::parse(source)), None)
    }
    #[test]
    fn tags_belong_only_to_the_declaring_document_or_block() {
        let source = "`= tags journal\n`- Parent\n `@ parent\n `= tags\n  `+ neovim\n  `+ {multi word}\n  `+ neovim\n `- Child\n`- Sibling\n `= tags Exercise\n\nAnonymous\n `= tags plain\n";
        let output = analyze(source);
        assert!(output.syntax().is_valid());
        let owners = output.tagged_owners();
        assert_eq!(owners.len(), 4);
        assert_eq!(owners[0].owner, TagOwner::Document);
        assert_eq!(owners[0].tags.values, ["journal"]);
        assert_eq!(owners[1].tags.values, ["neovim", "multi word"]);
        assert_eq!(owners[1].id.as_deref(), Some("parent"));
        assert_eq!(&source[owners[1].selection_range.clone()], "Parent");
        assert_eq!(owners[2].tags.values, ["Exercise"]);
        assert_eq!(owners[3].tags.values, ["plain"]);
    }
    #[test]
    fn malformed_and_duplicate_tags_retract_the_whole_owner_set() {
        for field in [
            "`= tags",
            "`= tags {}",
            "`= tags `!{rich}",
            "`= tags\n  `- wrong",
            "`= tags\n  `+ good\n  `+ {}",
            "`= tags\n  `+ nested\n   `+ value",
            "`= tags first\n `= tags second",
            "`= tags good `+{hidden}",
        ] {
            let source = format!("`- Owner\n {field}\n`- Healthy\n `= tags yes\n");
            let output = analyze(&source);
            assert_eq!(output.tagged_owners().len(), 1, "{source}");
            assert!(
                output
                    .diagnostics()
                    .iter()
                    .any(|d| d.code.starts_with("tags.")),
                "{source}"
            );
        }
        let output = analyze("`= tags first\n\nBody\n\n`= tags second\n");
        assert!(output.tagged_owners().is_empty());
        assert!(output
            .metadata()
            .diagnostics
            .iter()
            .any(|d| d.code == "tags.duplicate-property"));
    }
    #[test]
    fn declaration_payloads_rich_inline_and_unknown_keys_do_not_create_owners() {
        let output = analyze("`= other\n `= tags nope\n`- Owner\n `= tags other\n  `+ opaque\n`text\n `= tags\n  `+ `\"{标签 space}\"\n`- Inline `(){text `={tags ignored}}\n`rust\"\n `= tags raw\n");
        let owners = output.tagged_owners();
        assert_eq!(owners.len(), 1);
        assert_eq!(owners[0].tags.values, ["标签 space"]);
    }
    #[test]
    fn syntax_invalid_declaration_never_publishes_surviving_partial_values() {
        for source in [
            "`- Parent\n `= tags\n  `+ good\n  `+ {broken\n`- Healthy\n `= tags yes\n",
            "`= tags\n `+ good\n `+ {broken\n`- Healthy\n `= tags yes\n",
        ] {
            let output = analyze(source);
            let owners = output.tagged_owners();
            assert_eq!(owners.len(), 1, "{owners:?}");
            assert_eq!(owners[0].tags.values, ["yes"]);
        }
    }
    #[test]
    fn incremental_tags_match_fresh_and_reuse_unchanged_shards() {
        let old = "`= tags journal\r\n`- A\r\n `= tags α\r\n`- B\r\n `= tags β\r\n";
        let previous = analyze(old);
        for new in [
            old.replace("journal", "notes"),
            old.replace("α", "标签"),
            format!("Paragraph\r\n\r\n{old}"),
            old.replace("β", "{broken"),
        ] {
            let revision = previous.syntax().reparse(new.clone());
            let change = crate::DocumentChange {
                old_range: revision.old_reparsed_range,
                new_range: revision.reparsed_range,
            };
            let incremental = crate::analyze_green_regions(
                Arc::new(revision.document),
                Some((&previous, &change)),
            );
            assert!(incremental.reused_semantic_node_count() > 0);
            let fresh = analyze(&new);
            assert_eq!(incremental.tagged_owners(), fresh.tagged_owners());
            assert_eq!(incremental.metadata(), fresh.metadata());
        }
    }
    #[test]
    fn tag_completion_uses_owner_scope_and_current_plain_value_range() {
        for marked in [
            "`= tags ne|ovim",
            "`- 项目\n `= tags\n  `+ ne|ovim",
            "Anonymous\n `= tags ne|ovim",
        ] {
            let offset = marked.find('|').unwrap();
            let source = marked.replace('|', "");
            let parsed = plumb_syntax::parse(&source);
            let c = tag_completion_context(&parsed, offset).unwrap();
            assert_eq!(c.query, "ne");
            assert_eq!(&source[c.replace.clone()], "neovim");
            assert_eq!(
                Some(c),
                green_tag_completion_context(&plumb_syntax::GreenDocument::parse(source), offset)
            );
        }
        for source in [
            "`= other\n `= tags ne",
            "`- A\n `= tags other\n  `+ ne",
            "`rust\"\n `= tags ne",
        ] {
            assert!(tag_completion_context(&plumb_syntax::parse(source), source.len()).is_none());
        }
    }
}

#[cfg(test)]
mod property_tests {
    #[test]
    fn tags_property_completion_preserves_owner_and_global_root_duplicate_rules() {
        for marked in [
            "`= ta|",
            "`- Item\n `= ta|",
            "`# Heading\n `= ta|",
            "Anonymous\n `= ta|",
        ] {
            let offset = marked.find('|').unwrap();
            let source = marked.replace('|', "");
            let parsed = plumb_syntax::parse(&source);
            let context = crate::attribute_completion_context(&parsed, offset).unwrap();
            assert!(context.completions.iter().any(|c| c.label == "tags"));
            assert_eq!(
                Some(context),
                crate::green_attribute_completion_context(
                    &plumb_syntax::GreenDocument::parse(source),
                    offset
                )
            );
        }
        for marked in [
            "`= tags a\n\nBody\n\n`= ta|",
            "`- Item\n `= tags\n  `+ first\n `= ta|",
            "`= other\n `= ta|",
        ] {
            let offset = marked.find('|').unwrap();
            let source = marked.replace('|', "");
            assert!(!crate::green_attribute_completion_context(
                &plumb_syntax::GreenDocument::parse(source),
                offset
            )
            .is_some_and(|c| c.completions.iter().any(|c| c.label == "tags")));
        }
    }
}
