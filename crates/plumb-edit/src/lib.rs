use std::{collections::HashMap, ops::Range};

use plumb_syntax::{
    inline_range, AttrItem, Attributes, Block, GreenDocument, Inline, InlineContent, ParsedBlock,
    ParsedDocument,
};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range<usize>,
    pub new_text: String,
}

impl TextEdit {
    pub fn replace(
        parsed: &ParsedDocument,
        range: Range<usize>,
        new_text: impl Into<String>,
    ) -> Result<Self, EditError> {
        Self::replace_source(&parsed.source, range, new_text)
    }

    pub fn replace_source(
        source: &str,
        range: Range<usize>,
        new_text: impl Into<String>,
    ) -> Result<Self, EditError> {
        validate_range(source, &range)?;
        Ok(Self {
            range,
            new_text: new_text.into(),
        })
    }
}

pub fn apply_text_edits(mut source: String, mut edits: Vec<TextEdit>) -> Result<String, EditError> {
    if edits.iter().any(|edit| {
        edit.range.start > edit.range.end
            || edit.range.end > source.len()
            || !source.is_char_boundary(edit.range.start)
            || !source.is_char_boundary(edit.range.end)
    }) {
        return Err(EditError::InvalidRange);
    }
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
    let mut previous_start = source.len();
    for edit in edits {
        if edit.range.end > previous_start {
            return Err(EditError::OverlappingEdits);
        }
        previous_start = edit.range.start;
        source.replace_range(edit.range, &edit.new_text);
    }
    Ok(source)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatScope {
    Document,
    ContainedBlocks(Range<usize>),
}

pub fn format(parsed: &ParsedDocument, scope: FormatScope) -> Result<Vec<TextEdit>, EditError> {
    let edits = match scope {
        FormatScope::Document => plumb_format::format_parsed_edits(parsed),
        FormatScope::ContainedBlocks(selection) => {
            plumb_format::format_parsed_contained_blocks(parsed, selection)
        }
    }
    .map_err(|error| match error {
        plumb_format::FormatError::InvalidBlockRange => EditError::InvalidRange,
        plumb_format::FormatError::InvalidSyntax => EditError::GeneratedInvalid,
    })?;
    Ok(format_edits(edits))
}

pub fn format_green(document: &GreenDocument) -> Result<Vec<TextEdit>, EditError> {
    let edits = plumb_format::format_green_edits(document).map_err(|error| match error {
        plumb_format::FormatError::InvalidBlockRange => EditError::InvalidRange,
        plumb_format::FormatError::InvalidSyntax => EditError::GeneratedInvalid,
    })?;
    Ok(format_edits(edits))
}

pub fn format_green_contained(
    document: &GreenDocument,
    selection: Range<usize>,
) -> Result<Vec<TextEdit>, EditError> {
    let edits =
        plumb_format::format_green_contained_blocks(document, selection).map_err(|error| {
            match error {
                plumb_format::FormatError::InvalidBlockRange => EditError::InvalidRange,
                plumb_format::FormatError::InvalidSyntax => EditError::GeneratedInvalid,
            }
        })?;
    Ok(format_edits(edits))
}

fn format_edits(edits: Vec<plumb_format::FormatEdit>) -> Vec<TextEdit> {
    edits
        .into_iter()
        .map(|edit| TextEdit {
            range: edit.range,
            new_text: edit.new_text,
        })
        .collect()
}

pub fn align_block_arguments(
    parsed: &ParsedDocument,
    offset: usize,
) -> Result<Vec<TextEdit>, EditError> {
    if parsed.valid_syntax().is_none() || offset > parsed.source.len() {
        return Ok(Vec::new());
    }
    let Some((siblings, index)) = deepest_sibling_at(&parsed.syntax.blocks, offset) else {
        return Ok(Vec::new());
    };
    let Some((marker, argument_count)) = alignment_shape(&parsed.source, &siblings[index]) else {
        return Ok(Vec::new());
    };

    let same_shape = |block: &Block| {
        alignment_shape(&parsed.source, block)
            .is_some_and(|shape| shape == (marker, argument_count))
    };
    let start = (0..index)
        .rev()
        .take_while(|candidate| same_shape(&siblings[*candidate]))
        .last()
        .unwrap_or(index);
    let end = (index + 1..siblings.len())
        .take_while(|candidate| same_shape(&siblings[*candidate]))
        .last()
        .map_or(index + 1, |candidate| candidate + 1);
    if end - start < 2 {
        return Ok(Vec::new());
    }

    let blocks = siblings[start..end]
        .iter()
        .map(|block| match block {
            Block::Parsed(block) => block,
            Block::Verbatim(_) => unreachable!("alignment shape accepts only parsed blocks"),
        })
        .collect::<Vec<_>>();
    let mut widths = vec![0; argument_count - 1];
    for block in &blocks {
        for (column, maximum) in widths.iter_mut().enumerate() {
            *maximum = (*maximum).max(argument_alignment_width(&parsed.source, block, column));
        }
    }

    let mut edits = Vec::new();
    for block in blocks {
        let elements = block.content.positional_elements().collect::<Vec<_>>();
        for (column, width) in widths.iter().enumerate() {
            let separator =
                inline_range(elements[column]).end..inline_range(elements[column + 1]).start;
            let spaces = *width - argument_alignment_width(&parsed.source, block, column) + 1;
            push_changed_padding_edit(parsed, &mut edits, separator, spaces)?;
        }
    }
    edits.sort_by_key(|edit| edit.range.start);
    Ok(edits)
}

pub fn align_green_block_arguments(
    document: &GreenDocument,
    offset: usize,
) -> Result<Vec<TextEdit>, EditError> {
    if !document.is_valid() || offset > document.source().len() {
        return Ok(Vec::new());
    }
    let Some(shard) = document.shard_at(offset) else {
        return Ok(Vec::new());
    };
    let parsed = shard.shard().parsed();
    let local_offset = offset - shard.offset();
    let Some((siblings, local_index)) = deepest_sibling_at(&parsed.syntax.blocks, local_offset)
    else {
        return Ok(Vec::new());
    };
    if !std::ptr::eq(siblings, parsed.syntax.blocks.as_slice()) {
        return align_block_arguments(parsed, local_offset).and_then(|edits| {
            edits
                .into_iter()
                .map(|edit| rebase_edit(edit, shard.offset()))
                .collect()
        });
    }

    let Some((marker, argument_count)) = alignment_shape(&parsed.source, &siblings[local_index])
    else {
        return Ok(Vec::new());
    };
    let marker = marker.to_string();
    let target = siblings[local_index].range();
    let target = target.start + shard.offset()..target.end + shard.offset();

    struct Candidate<'a> {
        parsed: &'a ParsedDocument,
        block: &'a Block,
        offset: usize,
    }
    let mut candidates = Vec::new();
    for view in document.shards() {
        let parsed = view.shard().parsed();
        candidates.extend(parsed.syntax.blocks.iter().map(|block| Candidate {
            parsed,
            block,
            offset: view.offset(),
        }));
    }
    let Some(index) = candidates.iter().position(|candidate| {
        let range = candidate.block.range();
        range.start + candidate.offset == target.start && range.end + candidate.offset == target.end
    }) else {
        return Ok(Vec::new());
    };
    let same_shape = |candidate: &Candidate<'_>| {
        alignment_shape(&candidate.parsed.source, candidate.block)
            .is_some_and(|shape| shape.0 == marker && shape.1 == argument_count)
    };
    let start = (0..index)
        .rev()
        .take_while(|candidate| same_shape(&candidates[*candidate]))
        .last()
        .unwrap_or(index);
    let end = (index + 1..candidates.len())
        .take_while(|candidate| same_shape(&candidates[*candidate]))
        .last()
        .map_or(index + 1, |candidate| candidate + 1);
    if end - start < 2 {
        return Ok(Vec::new());
    }

    let mut widths = vec![0; argument_count - 1];
    for candidate in &candidates[start..end] {
        let Block::Parsed(block) = candidate.block else {
            unreachable!("alignment shape accepts only parsed blocks")
        };
        for (column, maximum) in widths.iter_mut().enumerate() {
            *maximum = (*maximum).max(argument_alignment_width(
                &candidate.parsed.source,
                block,
                column,
            ));
        }
    }

    let mut edits = Vec::new();
    for candidate in &candidates[start..end] {
        let Block::Parsed(block) = candidate.block else {
            unreachable!("alignment shape accepts only parsed blocks")
        };
        let elements = block.content.positional_elements().collect::<Vec<_>>();
        let mut local = Vec::new();
        for (column, width) in widths.iter().enumerate() {
            let separator =
                inline_range(elements[column]).end..inline_range(elements[column + 1]).start;
            let spaces =
                *width - argument_alignment_width(&candidate.parsed.source, block, column) + 1;
            push_changed_padding_edit(candidate.parsed, &mut local, separator, spaces)?;
        }
        for edit in local {
            edits.push(rebase_edit(edit, candidate.offset)?);
        }
    }
    edits.sort_by_key(|edit| edit.range.start);
    Ok(edits)
}

pub fn aligned_associations(entries: &[(&str, &str)]) -> Vec<OwnedBlock> {
    let mut blocks = entries
        .iter()
        .map(|(key, value)| OwnedBlock::padded_association(*key, *value))
        .collect::<Vec<_>>();
    align_owned_sibling_arguments(&mut blocks);
    blocks
}

pub fn render_authored_text_arguments(arguments: &[&str]) -> String {
    let arguments = arguments
        .iter()
        .map(|argument| owned_authored_text(argument))
        .collect::<Vec<_>>();
    let head = padded_owned_arguments(arguments);
    let mut rendered = String::new();
    render_owned_inlines(&head, false, 0, &mut rendered);
    rendered
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditError {
    InvalidRange,
    InvalidAttributePosition,
    OverlappingEdits,
    GeneratedInvalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributePosition {
    First,
    Last,
    Before(usize),
    After(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedAttribute {
    Id(String),
    Class(String),
    Pair { key: String, value: OwnedValue },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedValue {
    Bare(String),
    Quoted(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkedOwnerRewrite {
    pub owner_range: Range<usize>,
    pub marker: String,
    pub first_attribute: Option<OwnedAttribute>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GreenBlockAttributeTarget {
    pub range: Range<usize>,
    pub seed: String,
    pub has_id: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GreenOwnedBlockTarget {
    pub range: Range<usize>,
    pub block: OwnedBlock,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GreenOwnedBlockPaths {
    pub block: OwnedBlock,
    pub target_paths: Vec<Vec<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GreenOwnedSibling {
    pub content_range: Range<usize>,
    pub block: OwnedBlock,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GreenOwnedBlockCandidate {
    pub range: Range<usize>,
    pub content_range: Range<usize>,
    pub path: Vec<usize>,
    pub next_sibling: Option<GreenOwnedSibling>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GreenOwnedBlockGroup {
    pub range: Range<usize>,
    pub block: OwnedBlock,
    pub candidates: Vec<GreenOwnedBlockCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedBlock {
    Parsed {
        marker: Option<String>,
        head: Vec<OwnedInline>,
        children: Vec<OwnedBlock>,
        raw: Option<String>,
    },
    Verbatim {
        marker: Option<String>,
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedInline {
    Text(String),
    Space(String),
    SoftBreak,
    ArgumentSeparator,
    Element {
        kind: String,
        members: Vec<OwnedInlineMember>,
    },
    Verbatim {
        kind: String,
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedInlineMember {
    ParsedArgument(Vec<OwnedInline>),
    VerbatimArgument(String),
    Child(Box<OwnedInline>),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OwnedDocument {
    pub blocks: Vec<OwnedBlock>,
}

impl OwnedAttribute {
    pub fn id(value: impl Into<String>) -> Self {
        Self::Id(value.into())
    }

    pub fn class(value: impl Into<String>) -> Self {
        Self::Class(value.into())
    }

    pub fn bare(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self::Pair {
            key: key.into(),
            value: OwnedValue::Bare(value.into()),
        }
    }

    pub fn quoted(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self::Pair {
            key: key.into(),
            value: OwnedValue::Quoted(value.into()),
        }
    }

    fn render_block(&self) -> String {
        match self {
            Self::Id(value) => format!("`@ {}", escape_authored_text(value)),
            Self::Class(value) => format!("`+ {}", escape_authored_text(value)),
            Self::Pair { key, value } => {
                let value = match value {
                    OwnedValue::Bare(value) | OwnedValue::Quoted(value) => value,
                };
                format!("`= {}", render_authored_text_arguments(&[key, value]))
            }
        }
    }

    fn into_block(self) -> OwnedBlock {
        match self {
            Self::Id(value) => OwnedBlock::marked("@", value),
            Self::Class(value) => OwnedBlock::marked("+", value),
            Self::Pair { key, value } => {
                let value = match value {
                    OwnedValue::Bare(value) | OwnedValue::Quoted(value) => value,
                };
                OwnedBlock::padded_association(key, value)
            }
        }
    }
}

impl OwnedBlock {
    pub fn association(key: impl Into<String>, value: impl Into<String>) -> Self {
        let mut head = owned_authored_text(&key.into());
        head.push(OwnedInline::Space(" ".to_string()));
        head.push(OwnedInline::ArgumentSeparator);
        head.extend(owned_authored_text(&value.into()));
        Self::Parsed {
            marker: Some("=".into()),
            head,
            children: Vec::new(),
            raw: None,
        }
    }

    fn padded_association(key: impl Into<String>, value: impl Into<String>) -> Self {
        let mut block = Self::marked("=", "");
        block.set_head_text_arguments([key.into(), value.into()]);
        block
    }

    pub fn marked(marker: impl Into<String>, head: impl Into<String>) -> Self {
        Self::Parsed {
            marker: Some(marker.into()),
            head: owned_authored_text(&head.into()),
            children: Vec::new(),
            raw: None,
        }
    }

    pub fn paragraph(text: impl Into<String>) -> Self {
        Self::Parsed {
            marker: None,
            head: owned_authored_text(&text.into()),
            children: Vec::new(),
            raw: None,
        }
    }

    pub fn with_attributes(self, attributes: Vec<OwnedAttribute>) -> Self {
        match self {
            Self::Parsed {
                marker,
                head,
                mut children,
                raw,
            } => {
                children.retain(|child| owned_declaration(child).is_none());
                let mut declarations = attributes
                    .into_iter()
                    .map(OwnedAttribute::into_block)
                    .collect::<Vec<_>>();
                declarations.append(&mut children);
                Self::Parsed {
                    marker,
                    head,
                    children: declarations,
                    raw,
                }
            }
            Self::Verbatim { marker, text } => {
                if attributes.is_empty() {
                    Self::Verbatim { marker, text }
                } else {
                    Self::Parsed {
                        marker: marker.or_else(|| Some("()".into())),
                        head: Vec::new(),
                        children: attributes
                            .into_iter()
                            .map(OwnedAttribute::into_block)
                            .collect(),
                        raw: Some(text),
                    }
                }
            }
        }
    }

    pub fn with_aligned_attributes(mut self, attributes: Vec<OwnedAttribute>) -> Self {
        self = self.with_attributes(attributes);
        self.align_direct_property_runs();
        self
    }

    pub fn with_children(mut self, children: Vec<OwnedBlock>) -> Self {
        match &mut self {
            Self::Parsed {
                children: current, ..
            } => *current = children,
            Self::Verbatim { .. } => debug_assert!(children.is_empty()),
        }
        self
    }

    pub fn attributes(&self) -> Vec<OwnedAttribute> {
        match self {
            Self::Parsed { children, .. } => {
                children.iter().filter_map(owned_declaration).collect()
            }
            Self::Verbatim { .. } => Vec::new(),
        }
    }

    pub fn head_plain_text(&self) -> Option<String> {
        let Self::Parsed { head, .. } = self else {
            return None;
        };
        let mut output = String::new();
        append_owned_plain_text(head, &mut output);
        Some(output.trim().to_string())
    }

    pub fn retain_attributes(&mut self, mut predicate: impl FnMut(&OwnedAttribute) -> bool) {
        if let Self::Parsed { children, .. } = self {
            let mut retained = Vec::with_capacity(children.len());
            let mut removed_property_positions = Vec::new();
            for child in std::mem::take(children) {
                let Some(attribute) = owned_declaration(&child) else {
                    retained.push(child);
                    continue;
                };
                if predicate(&attribute) {
                    retained.push(child);
                } else if matches!(attribute, OwnedAttribute::Pair { .. }) {
                    removed_property_positions.push(retained.len());
                }
            }
            *children = retained;
            for position in removed_property_positions {
                align_owned_property_run_near(children, position);
            }
        }
    }

    pub fn push_attribute(&mut self, attribute: OwnedAttribute) {
        if let Self::Parsed { children, .. } = self {
            let align_properties = matches!(attribute, OwnedAttribute::Pair { .. });
            let index = children
                .iter()
                .rposition(|child| owned_declaration(child).is_some())
                .map_or(0, |index| index + 1);
            children.insert(index, attribute.into_block());
            if align_properties {
                align_owned_property_run_at(children, index);
            }
        } else {
            panic!("anonymous raw blocks have no attributes");
        }
    }

    pub fn prepend_attribute(&mut self, attribute: OwnedAttribute) {
        if let Self::Parsed { children, .. } = self {
            let align_properties = matches!(attribute, OwnedAttribute::Pair { .. });
            children.insert(0, attribute.into_block());
            if align_properties {
                align_owned_property_run_at(children, 0);
            }
        } else {
            panic!("anonymous raw blocks have no attributes");
        }
    }

    pub fn extend_attributes(&mut self, attributes: impl IntoIterator<Item = OwnedAttribute>) {
        for attribute in attributes {
            self.push_attribute(attribute);
        }
    }

    fn align_direct_property_runs(&mut self) {
        if let Self::Parsed { children, .. } = self {
            align_owned_sibling_arguments(children);
        }
    }

    pub fn set_head_text(&mut self, text: impl Into<String>) {
        if let Self::Parsed { head, .. } = self {
            *head = owned_authored_text(&text.into());
        }
    }

    pub fn set_head_arguments(&mut self, arguments: Vec<Vec<OwnedInline>>) {
        if let Self::Parsed { head, .. } = self {
            *head = padded_owned_arguments(arguments);
        }
    }

    pub fn set_head_text_arguments<I, S>(&mut self, arguments: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.set_head_arguments(
            arguments
                .into_iter()
                .map(|argument| owned_authored_text(&argument.into()))
                .collect(),
        );
    }

    pub fn prepend_head_argument(&mut self, argument: Vec<OwnedInline>) {
        if let Self::Parsed { head, .. } = self {
            let mut arguments = split_owned_arguments(std::mem::take(head));
            arguments.insert(0, argument);
            *head = padded_owned_arguments(arguments);
        }
    }

    pub fn prepend_head_text_argument(&mut self, argument: impl Into<String>) {
        self.prepend_head_argument(owned_authored_text(&argument.into()));
    }

    pub fn set_marker(&mut self, value: impl Into<String>) {
        if let Self::Parsed { marker, .. } = self {
            *marker = Some(value.into());
        }
    }

    pub fn children_mut(&mut self) -> Option<&mut Vec<OwnedBlock>> {
        match self {
            Self::Parsed { children, .. } => Some(children),
            Self::Verbatim { .. } => None,
        }
    }

    pub fn from_syntax(source: &str, block: &Block) -> Self {
        match block {
            Block::Parsed(block) => Self::from_parsed(source, block),
            Block::Verbatim(block) => Self::Verbatim {
                marker: block.mark.as_ref().map(|mark| mark.marker.clone()),
                text: block.text.clone(),
            },
        }
    }

    pub fn from_parsed(source: &str, block: &ParsedBlock) -> Self {
        let marker = block.mark.as_ref().map(|mark| mark.marker.clone());
        let content = block.content.trim_boundary_padding();
        let head = content.items.iter().map(OwnedInline::from_syntax).collect();
        Self::Parsed {
            marker,
            head,
            children: block
                .children
                .iter()
                .map(|child| Self::from_syntax(source, child))
                .collect(),
            raw: None,
        }
    }

    pub fn format(&self) -> Result<String, EditError> {
        format_owned_blocks(std::slice::from_ref(self), "\n")
    }
}

impl OwnedDocument {
    pub fn format(&self) -> Result<String, EditError> {
        format_owned_blocks(&self.blocks, "\n")
    }
}

fn owned_declaration(block: &OwnedBlock) -> Option<OwnedAttribute> {
    let OwnedBlock::Parsed {
        marker: Some(marker),
        head,
        children,
        raw: None,
    } = block
    else {
        return None;
    };
    if !children.is_empty() {
        return None;
    }
    match marker.as_str() {
        "@" => {
            let elements = owned_positional_indices(head);
            (elements.len() == 1)
                .then(|| plain_owned_element(&head[elements[0]]))
                .flatten()
                .map(OwnedAttribute::Id)
        }
        "+" => {
            let elements = owned_positional_indices(head);
            (elements.len() == 1)
                .then(|| plain_owned_element(&head[elements[0]]))
                .flatten()
                .map(OwnedAttribute::Class)
        }
        "=" => {
            let elements = owned_positional_indices(head);
            let key = plain_owned_element(head.get(*elements.first()?)?)?;
            let value_start = *elements.get(1)?;
            let value_end = *elements.last()?;
            let value = plain_owned_argument(
                &head[value_start..=value_end]
                    .iter()
                    .filter(|inline| {
                        !matches!(inline, OwnedInline::ArgumentSeparator)
                            && !owned_inline_declaration(inline)
                    })
                    .cloned()
                    .collect::<Vec<_>>(),
            )?;
            (!key.is_empty() && !value.is_empty()).then_some(OwnedAttribute::Pair {
                key,
                value: OwnedValue::Bare(value),
            })
        }
        _ => None,
    }
}

fn owned_positional_indices(items: &[OwnedInline]) -> Vec<usize> {
    items
        .iter()
        .enumerate()
        .filter_map(|(index, inline)| {
            (!matches!(
                inline,
                OwnedInline::Space(_) | OwnedInline::SoftBreak | OwnedInline::ArgumentSeparator
            ) && !owned_inline_declaration(inline))
            .then_some(index)
        })
        .collect()
}

fn owned_inline_declaration(inline: &OwnedInline) -> bool {
    matches!(
        inline,
        OwnedInline::Element { kind, .. } if matches!(kind.as_str(), "@" | "+" | "=")
    )
}

fn plain_owned_element(inline: &OwnedInline) -> Option<String> {
    match inline {
        OwnedInline::Text(text) => Some(text.clone()),
        OwnedInline::Verbatim { kind, text } if kind.is_empty() => Some(text.clone()),
        OwnedInline::Element { kind, members } if kind.is_empty() => {
            let [OwnedInlineMember::ParsedArgument(content)] = members.as_slice() else {
                return None;
            };
            plain_owned_argument(content)
        }
        _ => None,
    }
}

fn plain_owned_argument(items: &[OwnedInline]) -> Option<String> {
    let mut items = items.to_vec();
    trim_owned_padding_start(&mut items);
    trim_owned_padding_end(&mut items);
    items.iter().try_fold(String::new(), |mut output, inline| {
        match inline {
            OwnedInline::Text(text) | OwnedInline::Space(text) => output.push_str(text),
            OwnedInline::SoftBreak => output.push(' '),
            OwnedInline::Element { .. } | OwnedInline::Verbatim { .. } => {
                output.push_str(&plain_owned_element(inline)?)
            }
            OwnedInline::ArgumentSeparator => return None,
        }
        Some(output)
    })
}

pub fn replace_owned_block(
    parsed: &ParsedDocument,
    range: Range<usize>,
    block: &OwnedBlock,
) -> Result<TextEdit, EditError> {
    replace_owned_blocks(parsed, range, std::slice::from_ref(block))
}

pub fn own_green_block(
    document: &GreenDocument,
    range: Range<usize>,
) -> Result<OwnedBlock, EditError> {
    let (parsed, local, _) = green_block_target(document, &range)?;
    let block = block_with_range(&parsed.syntax.blocks, &local).ok_or(EditError::InvalidRange)?;
    Ok(OwnedBlock::from_syntax(&parsed.source, block))
}

pub fn own_green_block_paths(
    document: &GreenDocument,
    root: Range<usize>,
    targets: &[Range<usize>],
) -> Result<GreenOwnedBlockPaths, EditError> {
    let (parsed, local_root, offset) = green_block_target(document, &root)?;
    let root =
        block_with_range(&parsed.syntax.blocks, &local_root).ok_or(EditError::InvalidRange)?;
    let mut target_paths = Vec::with_capacity(targets.len());
    for target in targets {
        validate_range(document.source(), target)?;
        let start = target
            .start
            .checked_sub(offset)
            .ok_or(EditError::InvalidRange)?;
        let end = target
            .end
            .checked_sub(offset)
            .ok_or(EditError::InvalidRange)?;
        let path = block_path_with_range(root, &(start..end)).ok_or(EditError::InvalidRange)?;
        target_paths.push(path);
    }
    Ok(GreenOwnedBlockPaths {
        block: OwnedBlock::from_syntax(&parsed.source, root),
        target_paths,
    })
}

pub fn own_green_marked_block_groups(
    document: &GreenDocument,
    selection: Range<usize>,
    markers: &[&str],
) -> Result<Vec<GreenOwnedBlockGroup>, EditError> {
    validate_range(document.source(), &selection)?;
    struct Root<'a> {
        parsed: &'a ParsedDocument,
        block: &'a Block,
        offset: usize,
    }
    let mut roots = Vec::new();
    for shard in document.shards() {
        let parsed = shard.shard().parsed();
        roots.extend(parsed.syntax.blocks.iter().map(|block| Root {
            parsed,
            block,
            offset: shard.offset(),
        }));
    }

    let mut groups = Vec::new();
    for (index, root) in roots.iter().enumerate() {
        let next = roots
            .get(index + 1)
            .map(|next| (next.parsed, next.block, next.offset));
        let mut candidates = Vec::new();
        collect_green_marked_candidates(
            root.parsed,
            root.block,
            next,
            root.offset,
            &selection,
            markers,
            &mut Vec::new(),
            &mut candidates,
        );
        if !candidates.is_empty() {
            groups.push(GreenOwnedBlockGroup {
                range: root.block.range().start + root.offset..root.block.range().end + root.offset,
                block: OwnedBlock::from_syntax(&root.parsed.source, root.block),
                candidates,
            });
        }
    }
    Ok(groups)
}

#[allow(clippy::too_many_arguments)]
fn collect_green_marked_candidates(
    parsed: &ParsedDocument,
    block: &Block,
    next_sibling: Option<(&ParsedDocument, &Block, usize)>,
    offset: usize,
    selection: &Range<usize>,
    markers: &[&str],
    path: &mut Vec<usize>,
    output: &mut Vec<GreenOwnedBlockCandidate>,
) {
    let Block::Parsed(block) = block else {
        return;
    };
    let content_range = block.content.range.start + offset..block.content.range.end + offset;
    if content_range.start < selection.end
        && selection.start < content_range.end
        && block
            .mark
            .as_ref()
            .is_some_and(|mark| markers.iter().any(|marker| *marker == mark.marker.as_str()))
    {
        let next_sibling = next_sibling.and_then(|(parsed, sibling, offset)| {
            let Block::Parsed(sibling) = sibling else {
                return None;
            };
            Some(GreenOwnedSibling {
                content_range: sibling.content.range.start + offset
                    ..sibling.content.range.end + offset,
                block: OwnedBlock::from_parsed(&parsed.source, sibling),
            })
        });
        output.push(GreenOwnedBlockCandidate {
            range: block.range.start + offset..block.range.end + offset,
            content_range,
            path: path.clone(),
            next_sibling,
        });
    }
    for (index, child) in block.children.iter().enumerate() {
        path.push(index);
        collect_green_marked_candidates(
            parsed,
            child,
            block
                .children
                .get(index + 1)
                .map(|next| (parsed, next, offset)),
            offset,
            selection,
            markers,
            path,
            output,
        );
        path.pop();
    }
}

pub fn own_deepest_green_marked_block(
    document: &GreenDocument,
    offset: usize,
    markers: &[&str],
) -> Option<GreenOwnedBlockTarget> {
    if offset > document.source().len() || !document.source().is_char_boundary(offset) {
        return None;
    }
    let current = document.shard_at(offset)?;
    if let Some(target) = own_deepest_marked_in_shard(current, offset, markers) {
        return Some(target);
    }
    let previous = document.source()[..offset]
        .char_indices()
        .next_back()
        .and_then(|(previous, _)| document.shard_at(previous))?;
    (previous.offset() != current.offset())
        .then(|| own_deepest_marked_in_shard(previous, offset, markers))
        .flatten()
}

pub fn next_green_sibling_block(
    document: &GreenDocument,
    range: Range<usize>,
) -> Option<GreenOwnedBlockTarget> {
    let location = green_block_location(document, &range).ok()?;
    if location.path.is_empty() {
        let mut found = false;
        for shard in document.shards() {
            let parsed = shard.shard().parsed();
            for block in &parsed.syntax.blocks {
                let absolute =
                    block.range().start + shard.offset()..block.range().end + shard.offset();
                if found {
                    return Some(GreenOwnedBlockTarget {
                        range: absolute,
                        block: OwnedBlock::from_syntax(&parsed.source, block),
                    });
                }
                found = absolute == range;
            }
        }
        return None;
    }

    let (parsed, local, offset) = green_block_target(document, &range).ok()?;
    for root in &parsed.syntax.blocks {
        let Some(path) = block_path_with_range(root, &local) else {
            continue;
        };
        let (&index, parent_path) = path.split_last()?;
        let sibling = block_at_path(root, parent_path)?
            .children()
            .get(index + 1)?;
        return Some(GreenOwnedBlockTarget {
            range: sibling.range().start + offset..sibling.range().end + offset,
            block: OwnedBlock::from_syntax(&parsed.source, sibling),
        });
    }
    None
}

fn own_deepest_marked_in_shard(
    shard: plumb_syntax::GreenShardView<'_>,
    offset: usize,
    markers: &[&str],
) -> Option<GreenOwnedBlockTarget> {
    let parsed = shard.shard().parsed();
    let local_offset = offset.checked_sub(shard.offset())?;
    let mut pending = parsed
        .syntax
        .blocks
        .iter()
        .map(|block| (block, 0usize))
        .collect::<Vec<_>>();
    let mut result = None;
    let mut result_position = (0usize, 0usize);
    while let Some((block, depth)) = pending.pop() {
        let range = block.range();
        if !(range.start <= local_offset && local_offset <= range.end) {
            continue;
        }
        if let Block::Parsed(block) = block {
            if block
                .mark
                .as_ref()
                .is_some_and(|mark| markers.iter().any(|marker| *marker == mark.marker.as_str()))
                && (result.is_none() || (depth, block.range.start) > result_position)
            {
                result = Some(GreenOwnedBlockTarget {
                    range: block.range.start + shard.offset()..block.range.end + shard.offset(),
                    block: OwnedBlock::from_parsed(&parsed.source, block),
                });
                result_position = (depth, block.range.start);
            }
            pending.extend(block.children.iter().map(|child| (child, depth + 1)));
        }
    }
    result
}

pub fn replace_green_block(
    document: &GreenDocument,
    range: Range<usize>,
    block: &OwnedBlock,
) -> Result<TextEdit, EditError> {
    replace_green_blocks(document, range, std::slice::from_ref(block))
}

pub fn replace_green_blocks(
    document: &GreenDocument,
    range: Range<usize>,
    blocks: &[OwnedBlock],
) -> Result<TextEdit, EditError> {
    let (parsed, local, offset) = green_block_target(document, &range)?;
    rebase_edit(replace_owned_blocks(parsed, local, blocks)?, offset)
}

pub fn prepend_green_blocks(
    document: &GreenDocument,
    blocks: &[OwnedBlock],
) -> Result<TextEdit, EditError> {
    let Some(shard) = document.shards().next() else {
        return Ok(TextEdit {
            range: 0..0,
            new_text: format_owned_blocks(blocks, line_ending(document.source()))?,
        });
    };
    let parsed = shard.shard().parsed();
    let mut edit = EditSession::new(parsed, 0..0)?;
    edit.insert_blocks(0, blocks)?;
    rebase_edit(edit.finish()?, shard.offset())
}

pub fn append_green_blocks(
    document: &GreenDocument,
    blocks: &[OwnedBlock],
) -> Result<TextEdit, EditError> {
    let mut last = None;
    for shard in document.shards() {
        if let Some(block) = shard.shard().parsed().syntax.blocks.last() {
            last = Some((shard, block.range().clone()));
        }
    }
    let Some((shard, local)) = last else {
        return prepend_green_blocks(document, blocks);
    };
    let parsed = shard.shard().parsed();
    let mut edit = EditSession::new(parsed, local.clone())?;
    edit.insert_sibling_blocks(&local, blocks)?;
    rebase_edit(edit.finish()?, shard.offset())
}

pub fn insert_green_top_level_blocks_after(
    document: &GreenDocument,
    after: Range<usize>,
    blocks: &[OwnedBlock],
) -> Result<TextEdit, EditError> {
    let (parsed, local, offset) = green_block_target(document, &after)?;
    if !parsed
        .syntax
        .blocks
        .iter()
        .any(|block| block.range() == &local)
    {
        return Err(EditError::InvalidRange);
    }
    let mut edit = EditSession::new(parsed, local.clone())?;
    edit.insert_sibling_blocks(&local, blocks)?;
    rebase_edit(edit.finish()?, offset)
}

pub fn insert_green_child_blocks(
    document: &GreenDocument,
    parent: Range<usize>,
    after: Option<&Range<usize>>,
    blocks: &[OwnedBlock],
) -> Result<TextEdit, EditError> {
    let (parsed, local_parent, offset) = green_block_target(document, &parent)?;
    let parent = parsed_block_with_range(&parsed.syntax.blocks, &local_parent)
        .ok_or(EditError::InvalidRange)?;
    let index = if let Some(after) = after {
        if after.start < offset || after.end < offset {
            return Err(EditError::InvalidRange);
        }
        let local_after = after.start - offset..after.end - offset;
        parent
            .children
            .iter()
            .position(|child| child.range() == &local_after)
            .map(|index| index + 1)
            .ok_or(EditError::InvalidRange)?
    } else {
        parent.children.len()
    };
    let mut owned = OwnedBlock::from_parsed(&parsed.source, parent);
    owned
        .children_mut()
        .ok_or(EditError::InvalidRange)?
        .splice(index..index, blocks.iter().cloned());
    rebase_edit(replace_owned_block(parsed, local_parent, &owned)?, offset)
}

pub fn move_green_block(
    document: &GreenDocument,
    source: Range<usize>,
    parent: Option<&Range<usize>>,
    after: Option<&Range<usize>>,
    moved: OwnedBlock,
) -> Result<Vec<TextEdit>, EditError> {
    if parent.is_some_and(|parent| source.start <= parent.start && parent.end <= source.end)
        || after == Some(&source)
    {
        return Err(EditError::InvalidRange);
    }
    let source_location = green_block_location(document, &source)?;

    if let Some(parent) = parent {
        let parent_location = green_block_location(document, parent)?;
        let after_location = after
            .map(|after| green_block_location(document, after))
            .transpose()?;
        if after_location
            .as_ref()
            .is_some_and(|after| after.parent.as_ref() != Some(parent))
        {
            return Err(EditError::InvalidRange);
        }
        if source_location.root == parent_location.root {
            let mut targets = vec![source.clone(), parent.clone()];
            if let Some(after) = after {
                targets.push(after.clone());
            }
            let mut owned =
                own_green_block_paths(document, source_location.root.clone(), &targets)?;
            let source_path = owned.target_paths[0].clone();
            let mut parent_path = owned.target_paths[1].clone();
            let mut insertion = if let Some(after_path) = owned.target_paths.get(2) {
                after_path
                    .last()
                    .copied()
                    .map(|index| index + 1)
                    .ok_or(EditError::InvalidRange)?
            } else {
                owned_at_path_mut(&mut owned.block, &parent_path)
                    .and_then(OwnedBlock::children_mut)
                    .ok_or(EditError::InvalidRange)?
                    .len()
            };
            if source_path.len() == parent_path.len() + 1
                && source_path[..parent_path.len()] == parent_path
                && insertion > source_path[parent_path.len()]
            {
                insertion -= 1;
            }
            adjust_owned_path_after_removal(&mut parent_path, &source_path);
            remove_owned_at_path(&mut owned.block, &source_path).ok_or(EditError::InvalidRange)?;
            owned_at_path_mut(&mut owned.block, &parent_path)
                .and_then(OwnedBlock::children_mut)
                .ok_or(EditError::InvalidRange)?
                .insert(insertion, moved);
            return Ok(vec![replace_green_block(
                document,
                source_location.root,
                &owned.block,
            )?]);
        }

        let remove = remove_green_location(document, &source_location)?;
        let insert = insert_green_child_blocks(document, parent.clone(), after, &[moved])?;
        return Ok(vec![remove, insert]);
    }

    let top_level_after = match after {
        Some(after) => Some(after.clone()),
        None => last_green_top_level_range_excluding(document, &source),
    };
    let Some(after) = top_level_after else {
        return Ok(vec![replace_green_block(document, source, &moved)?]);
    };
    let after_location = green_block_location(document, &after)?;
    if !after_location.path.is_empty() {
        return Err(EditError::InvalidRange);
    }
    if after.start <= source.start && source.end <= after.end {
        let mut owned = own_green_block_paths(document, after.clone(), &[source])?;
        remove_owned_at_path(&mut owned.block, &owned.target_paths[0])
            .ok_or(EditError::InvalidRange)?;
        return Ok(vec![replace_green_blocks(
            document,
            after,
            &[owned.block, moved],
        )?]);
    }

    let remove = remove_green_location(document, &source_location)?;
    let insert = insert_green_top_level_blocks_after(document, after, &[moved])?;
    Ok(vec![remove, insert])
}

#[derive(Debug)]
struct GreenBlockLocation {
    range: Range<usize>,
    root: Range<usize>,
    parent: Option<Range<usize>>,
    path: Vec<usize>,
}

fn green_block_location(
    document: &GreenDocument,
    range: &Range<usize>,
) -> Result<GreenBlockLocation, EditError> {
    let (parsed, local, offset) = green_block_target(document, range)?;
    for root in &parsed.syntax.blocks {
        let Some(path) = block_path_with_range(root, &local) else {
            continue;
        };
        let parent = (!path.is_empty())
            .then(|| block_at_path(root, &path[..path.len() - 1]))
            .flatten()
            .map(|block| block.range().start + offset..block.range().end + offset);
        return Ok(GreenBlockLocation {
            range: range.clone(),
            root: root.range().start + offset..root.range().end + offset,
            parent,
            path,
        });
    }
    Err(EditError::InvalidRange)
}

fn remove_green_location(
    document: &GreenDocument,
    location: &GreenBlockLocation,
) -> Result<TextEdit, EditError> {
    let Some(parent) = &location.parent else {
        return remove_green_block(document, location.range.clone());
    };
    let mut owned = own_green_block_paths(
        document,
        parent.clone(),
        std::slice::from_ref(&location.range),
    )?;
    remove_owned_at_path(&mut owned.block, &owned.target_paths[0])
        .ok_or(EditError::InvalidRange)?;
    replace_green_block(document, parent.clone(), &owned.block)
}

fn last_green_top_level_range_excluding(
    document: &GreenDocument,
    excluded: &Range<usize>,
) -> Option<Range<usize>> {
    let mut last = None;
    for shard in document.shards() {
        for block in &shard.shard().parsed().syntax.blocks {
            let range = block.range().start + shard.offset()..block.range().end + shard.offset();
            if &range != excluded {
                last = Some(range);
            }
        }
    }
    last
}

pub fn green_block_attribute_target(
    document: &GreenDocument,
    offset: usize,
) -> Option<GreenBlockAttributeTarget> {
    let shard = document.shard_at(offset)?;
    let local_offset = offset.checked_sub(shard.offset())?;
    let mut pending = shard
        .shard()
        .parsed()
        .syntax
        .blocks
        .iter()
        .map(|block| (block, 0usize))
        .collect::<Vec<_>>();
    let mut result = None;
    let mut result_position = (0usize, 0usize);
    while let Some((block, depth)) = pending.pop() {
        let range = block.range();
        if !(range.start <= local_offset && local_offset < range.end) {
            continue;
        }
        if let Block::Parsed(block) = block {
            if let Some(mark) = &block.mark {
                if result.is_none() || (depth, block.range.start) > result_position {
                    let title = block.content.plain_text();
                    result = Some(GreenBlockAttributeTarget {
                        range: block.range.start + shard.offset()..block.range.end + shard.offset(),
                        seed: if title.trim().is_empty() {
                            mark.marker.clone()
                        } else {
                            title.trim().to_string()
                        },
                        has_id: mark.attrs.id().is_some(),
                    });
                    result_position = (depth, block.range.start);
                }
            }
            pending.extend(block.children.iter().map(|child| (child, depth + 1)));
        }
    }
    result
}

pub fn insert_green_block_attribute(
    document: &GreenDocument,
    owner: Range<usize>,
    position: AttributePosition,
    item: OwnedAttribute,
) -> Result<TextEdit, EditError> {
    let (parsed, local, offset) = green_block_target(document, &owner)?;
    let owner =
        parsed_block_with_range(&parsed.syntax.blocks, &local).ok_or(EditError::InvalidRange)?;
    let mark = owner.mark.as_ref().ok_or(EditError::InvalidRange)?;
    let mut edit = EditSession::new(parsed, local)?;
    edit.insert_attribute(&mark.attrs, mark.marker_range.end, position, item)?;
    rebase_edit(edit.finish()?, offset)
}

fn green_block_target<'a>(
    document: &'a GreenDocument,
    range: &Range<usize>,
) -> Result<(&'a ParsedDocument, Range<usize>, usize), EditError> {
    validate_range(document.source(), range)?;
    let shard = document
        .shard_at(range.start)
        .ok_or(EditError::InvalidRange)?;
    if range.end > shard.range().end {
        return Err(EditError::InvalidRange);
    }
    let local = range.start - shard.offset()..range.end - shard.offset();
    Ok((shard.shard().parsed(), local, shard.offset()))
}

fn rebase_edit(mut edit: TextEdit, offset: usize) -> Result<TextEdit, EditError> {
    edit.range.start = edit
        .range
        .start
        .checked_add(offset)
        .ok_or(EditError::InvalidRange)?;
    edit.range.end = edit
        .range
        .end
        .checked_add(offset)
        .ok_or(EditError::InvalidRange)?;
    Ok(edit)
}

pub fn replace_owned_blocks(
    parsed: &ParsedDocument,
    range: Range<usize>,
    blocks: &[OwnedBlock],
) -> Result<TextEdit, EditError> {
    validate_range(&parsed.source, &range)?;
    if !has_block_range(&parsed.syntax.blocks, &range) {
        return Err(EditError::InvalidRange);
    }
    let line_start = parsed.source[..range.start]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let indent = &parsed.source[line_start..range.start];
    if !indent.chars().all(|character| character == ' ') {
        return Err(EditError::InvalidRange);
    }
    let newline = line_ending(&parsed.source);
    let formatted = format_owned_blocks(blocks, newline)?;
    let mut replacement = String::new();
    for (index, line) in formatted.split_inclusive(newline).enumerate() {
        let content = line.strip_suffix(newline).unwrap_or(line);
        if !content.is_empty() {
            if index > 0 {
                replacement.push_str(indent);
            }
            replacement.push_str(content);
        }
        if line.ends_with(newline) {
            replacement.push_str(newline);
        }
    }
    let original = &parsed.source[range.clone()];
    let original_breaks = trailing_line_breaks(original);
    let replacement_breaks = trailing_line_breaks(&replacement);
    for _ in replacement_breaks..original_breaks {
        replacement.push_str(newline);
    }
    TextEdit::replace(parsed, range, replacement)
}

pub fn rewrite_marked_owners(
    parsed: &ParsedDocument,
    rewrites: &[MarkedOwnerRewrite],
) -> Result<Vec<TextEdit>, EditError> {
    let mut owners = HashMap::new();
    collect_parsed_blocks(&parsed.syntax.blocks, &mut owners);
    let newline = line_ending(&parsed.source);
    let mut edits = Vec::with_capacity(rewrites.len() * 2);
    for rewrite in rewrites {
        let owner = owners
            .get(&(rewrite.owner_range.start, rewrite.owner_range.end))
            .ok_or(EditError::InvalidRange)?;
        let mark = owner.mark.as_ref().ok_or(EditError::InvalidRange)?;
        edits.push(TextEdit::replace(
            parsed,
            mark.marker_range.clone(),
            rewrite.marker.clone(),
        )?);
        if let Some(attribute) = &rewrite.first_attribute {
            edits.push(prepend_owner_attribute(parsed, owner, attribute, newline)?);
        }
    }
    let mut ranges = edits.iter().map(|edit| &edit.range).collect::<Vec<_>>();
    ranges.sort_by_key(|range| (range.start, range.end));
    if ranges
        .windows(2)
        .any(|ranges| ranges[0].end > ranges[1].start || ranges[0].start == ranges[1].start)
    {
        return Err(EditError::OverlappingEdits);
    }
    Ok(edits)
}

fn collect_parsed_blocks<'a>(
    blocks: &'a [Block],
    owners: &mut HashMap<(usize, usize), &'a ParsedBlock>,
) {
    for block in blocks {
        if let Block::Parsed(block) = block {
            owners.insert((block.range.start, block.range.end), block);
            collect_parsed_blocks(&block.children, owners);
        }
    }
}

fn prepend_owner_attribute(
    parsed: &ParsedDocument,
    owner: &ParsedBlock,
    attribute: &OwnedAttribute,
    newline: &str,
) -> Result<TextEdit, EditError> {
    let mark = owner.mark.as_ref().ok_or(EditError::InvalidRange)?;
    let source = &parsed.source;
    let owner_line_start = source[..owner.range.start]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let owner_indent = &source[owner_line_start..owner.range.start];
    if !owner_indent.chars().all(|character| character == ' ') {
        return Err(EditError::InvalidRange);
    }
    let owner_line_end = source[mark.marker_range.end..]
        .find('\n')
        .map(|relative| mark.marker_range.end + relative);
    let structural_start = owner
        .children
        .first()
        .map(Block::range)
        .map(|range| range.start);

    if let Some(structural_start) = structural_start {
        let structural_line_start = source[..structural_start]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        if structural_line_start == owner_line_start {
            let indent = " ".repeat(owner_indent.len() + 1);
            let text = format!(
                "{newline}{newline}{indent}{}{newline}{newline}{indent}",
                attribute.render_block()
            );
            return TextEdit::replace(parsed, structural_start..structural_start, text);
        }
        let indent = &source[structural_line_start..structural_start];
        if !indent.chars().all(|character| character == ' ') {
            return Err(EditError::InvalidRange);
        }
        let line_end = owner_line_end.ok_or(EditError::InvalidRange)?;
        let gap = &source[line_end..structural_line_start];
        let prefix = if gap.bytes().filter(|byte| *byte == b'\n').count() >= 2 {
            ""
        } else {
            newline
        };
        let text = format!(
            "{prefix}{indent}{}{newline}{newline}",
            attribute.render_block()
        );
        return TextEdit::replace(parsed, structural_line_start..structural_line_start, text);
    }

    let indent = " ".repeat(owner_indent.len() + 1);
    let replacement = format!("{newline}{indent}{}{newline}", attribute.render_block());
    if let Some(line_end) = owner_line_end {
        let break_start = if source[..line_end].ends_with('\r') {
            line_end - 1
        } else {
            line_end
        };
        TextEdit::replace(parsed, break_start..line_end + 1, replacement)
    } else {
        TextEdit::replace(parsed, source.len()..source.len(), replacement)
    }
}

pub fn remove_block(parsed: &ParsedDocument, range: Range<usize>) -> Result<TextEdit, EditError> {
    if !has_block_range(&parsed.syntax.blocks, &range) {
        return Err(EditError::InvalidRange);
    }
    let mut edit = EditSession::new(parsed, range.clone())?;
    edit.remove_block(range)?;
    edit.finish()
}

pub fn remove_green_block(
    document: &GreenDocument,
    range: Range<usize>,
) -> Result<TextEdit, EditError> {
    let (parsed, local, offset) = green_block_target(document, &range)?;
    rebase_edit(remove_block(parsed, local)?, offset)
}

impl OwnedInline {
    pub fn from_syntax(inline: &Inline) -> Self {
        match inline {
            Inline::Text { text, .. } => Self::Text(text.clone()),
            Inline::Space { text, .. } => Self::Space(text.clone()),
            Inline::SoftBreak { .. } => Self::Space(" ".to_string()),
            Inline::Group { mark, content, .. } => Self::Element {
                kind: mark
                    .as_ref()
                    .map_or_else(String::new, |mark| mark.marker.clone()),
                members: (!content.is_empty())
                    .then(|| {
                        let content = content.trim_boundary_padding();
                        OwnedInlineMember::ParsedArgument(
                            content.items.iter().map(Self::from_syntax).collect(),
                        )
                    })
                    .into_iter()
                    .collect(),
            },
            Inline::Verbatim { mark, text, .. } => Self::Verbatim {
                kind: mark
                    .as_ref()
                    .map_or_else(String::new, |mark| mark.marker.clone()),
                text: text.clone(),
            },
        }
    }
}

pub struct EditSession<'a> {
    parsed: &'a ParsedDocument,
    affected: Range<usize>,
    edits: Vec<TextEdit>,
}

impl<'a> EditSession<'a> {
    pub fn new(parsed: &'a ParsedDocument, affected: Range<usize>) -> Result<Self, EditError> {
        validate_range(&parsed.source, &affected)?;
        Ok(Self {
            parsed,
            affected,
            edits: Vec::new(),
        })
    }

    pub fn insert_attribute(
        &mut self,
        attributes: &Attributes,
        owner_insert: usize,
        position: AttributePosition,
        item: OwnedAttribute,
    ) -> Result<(), EditError> {
        self.insert_attributes(attributes, owner_insert, [(position, item)])
    }

    pub fn insert_attributes(
        &mut self,
        attributes: &Attributes,
        owner_insert: usize,
        additions: impl IntoIterator<Item = (AttributePosition, OwnedAttribute)>,
    ) -> Result<(), EditError> {
        enum Entry {
            Existing(usize),
            Added(OwnedAttribute),
        }
        let mut entries = (0..attributes.items.len())
            .map(Entry::Existing)
            .collect::<Vec<_>>();
        for (position, item) in additions {
            let index = insertion_index(position, entries.len())?;
            entries.insert(index, Entry::Added(item));
        }
        let newline = line_ending(&self.parsed.source);
        let mut index = 0;
        while index < entries.len() {
            if matches!(entries[index], Entry::Existing(_)) {
                index += 1;
                continue;
            }
            let start = index;
            while index < entries.len() && matches!(entries[index], Entry::Added(_)) {
                index += 1;
            }
            let additions = entries[start..index]
                .iter()
                .map(|entry| match entry {
                    Entry::Added(item) => item.clone(),
                    Entry::Existing(_) => unreachable!(),
                })
                .collect::<Vec<_>>();
            let next_existing = entries[index..].iter().find_map(|entry| match entry {
                Entry::Existing(existing) => Some(*existing),
                Entry::Added(_) => None,
            });
            if let Some(existing) = next_existing {
                let offset = attr_item_range(&attributes.items[existing]).start;
                let indent = line_indent(&self.parsed.source, offset);
                let prefix = " ".repeat(indent);
                let rendered = render_block_attributes(&additions, indent, newline);
                let mut text = rendered
                    .strip_prefix(&prefix)
                    .ok_or(EditError::GeneratedInvalid)?
                    .to_string();
                text.push_str(newline);
                text.push_str(newline);
                text.push_str(&prefix);
                self.replace(offset..offset, text)?;
            } else if let Some(last) = attributes.items.last() {
                let range = attr_item_range(last);
                let content = self.parsed.source[range.clone()].trim_end_matches(['\r', '\n']);
                let offset = range.start + content.len();
                let indent = line_indent(&self.parsed.source, range.start);
                let text = format!(
                    "{newline}{}",
                    render_block_attributes(&additions, indent, newline)
                );
                self.replace(offset..offset, text)?;
            } else {
                let line_end = self.parsed.source[owner_insert..]
                    .find('\n')
                    .map_or(self.parsed.source.len(), |relative| owner_insert + relative);
                let owner_indent = line_indent(&self.parsed.source, owner_insert);
                let child_indent =
                    parsed_block_with_range(&self.parsed.syntax.blocks, &self.affected)
                        .and_then(|owner| owner.children.first())
                        .map_or(owner_indent + 1, |child| {
                            line_indent(&self.parsed.source, child.range().start)
                        });
                let text = format!(
                    "{newline}{newline}{}",
                    render_block_attributes(&additions, child_indent, newline)
                );
                self.replace(line_end..line_end, text)?;
            }
        }
        Ok(())
    }

    pub fn replace_attribute(
        &mut self,
        attributes: &Attributes,
        index: usize,
        item: OwnedAttribute,
    ) -> Result<(), EditError> {
        let target = attributes
            .items
            .get(index)
            .ok_or(EditError::InvalidAttributePosition)?;
        let range = attr_item_range(target).clone();
        let mut replacement = item.render_block();
        let newline = line_ending(&self.parsed.source);
        if self.parsed.source[range.clone()].ends_with(newline) {
            replacement.push_str(newline);
        }
        self.replace(range, replacement)
    }

    pub fn remove_attribute(
        &mut self,
        attributes: &Attributes,
        index: usize,
    ) -> Result<(), EditError> {
        let target = attributes
            .items
            .get(index)
            .ok_or(EditError::InvalidAttributePosition)?;
        self.replace(attr_item_range(target).clone(), String::new())
    }

    pub fn insert_blocks(&mut self, offset: usize, blocks: &[OwnedBlock]) -> Result<(), EditError> {
        if offset < self.affected.start || offset > self.affected.end {
            return Err(EditError::InvalidRange);
        }
        let newline = line_ending(&self.parsed.source);
        let new_text = format_owned_blocks(blocks, newline)?;
        self.replace(offset..offset, new_text)
    }

    pub fn insert_sibling_blocks(
        &mut self,
        after: &Range<usize>,
        blocks: &[OwnedBlock],
    ) -> Result<(), EditError> {
        validate_range(&self.parsed.source, after)?;
        let newline = line_ending(&self.parsed.source);
        let formatted = format_owned_blocks(blocks, newline)?;
        let line_start = self.parsed.source[..after.start]
            .rfind('\n')
            .map_or(0, |offset| offset + 1);
        let indent = &self.parsed.source[line_start..after.start];
        if !indent.chars().all(|character| character == ' ') {
            return Err(EditError::InvalidRange);
        }
        let mut new_text = String::new();
        if !self.parsed.source[..after.end].ends_with(newline) {
            new_text.push_str(newline);
        }
        new_text.push_str(&indent_fragment(&formatted, indent, newline));
        self.replace(after.end..after.end, new_text)
    }

    pub fn replace_block(
        &mut self,
        range: Range<usize>,
        block: &OwnedBlock,
    ) -> Result<(), EditError> {
        self.replace_block_with_blocks(range, std::slice::from_ref(block))
    }

    pub fn replace_block_with_blocks(
        &mut self,
        range: Range<usize>,
        blocks: &[OwnedBlock],
    ) -> Result<(), EditError> {
        let newline = line_ending(&self.parsed.source);
        let formatted = format_owned_blocks(blocks, newline)?;
        let indent = " ".repeat(line_indent(&self.parsed.source, range.start));
        let new_text = if indent.is_empty() {
            formatted
        } else {
            let indented = indent_fragment(&formatted, &indent, newline);
            indented
                .strip_prefix(&indent)
                .ok_or(EditError::GeneratedInvalid)?
                .to_string()
        };
        self.replace(range, new_text)
    }

    pub fn remove_block(&mut self, range: Range<usize>) -> Result<(), EditError> {
        self.replace(range, String::new())
    }

    pub fn replace(
        &mut self,
        range: Range<usize>,
        new_text: impl Into<String>,
    ) -> Result<(), EditError> {
        validate_range(&self.parsed.source, &range)?;
        if range.start < self.affected.start || range.end > self.affected.end {
            return Err(EditError::InvalidRange);
        }
        self.edits.push(TextEdit {
            range,
            new_text: new_text.into(),
        });
        Ok(())
    }

    pub fn finish(self) -> Result<TextEdit, EditError> {
        finalize(self.parsed, self.affected, self.edits)
    }
}

fn attr_item_range(item: &AttrItem) -> &Range<usize> {
    match item {
        AttrItem::Id { range, .. }
        | AttrItem::Class { range, .. }
        | AttrItem::Pair { range, .. } => range,
    }
}

fn insertion_index(position: AttributePosition, len: usize) -> Result<usize, EditError> {
    match position {
        AttributePosition::First => Ok(0),
        AttributePosition::Last => Ok(len),
        AttributePosition::Before(index) if index <= len => Ok(index),
        AttributePosition::After(index) if index < len => Ok(index + 1),
        AttributePosition::Before(_) | AttributePosition::After(_) => {
            Err(EditError::InvalidAttributePosition)
        }
    }
}

fn render_block_attributes(items: &[OwnedAttribute], indent: usize, newline: &str) -> String {
    let item_indent = " ".repeat(indent);
    items
        .iter()
        .map(|item| format!("{item_indent}{}", item.render_block()))
        .collect::<Vec<_>>()
        .join(&format!("{newline}{newline}"))
}

fn line_indent(source: &str, offset: usize) -> usize {
    let start = source[..offset].rfind('\n').map_or(0, |index| index + 1);
    source[start..offset]
        .bytes()
        .take_while(|byte| *byte == b' ')
        .count()
}

fn format_owned_blocks(blocks: &[OwnedBlock], newline: &str) -> Result<String, EditError> {
    if blocks.is_empty() {
        return Ok(String::new());
    }
    let mut source = String::new();
    render_owned_blocks(blocks, 0, &mut source);
    let formatted = plumb_format::format(&source).map_err(|_| EditError::GeneratedInvalid)?;
    if newline == "\r\n" {
        Ok(formatted.replace('\n', "\r\n"))
    } else {
        Ok(formatted)
    }
}

fn render_owned_blocks(blocks: &[OwnedBlock], indent: usize, output: &mut String) {
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 {
            output.push_str("\n\n");
        }
        render_owned_block(block, indent, output);
    }
}

fn render_owned_block(block: &OwnedBlock, indent: usize, output: &mut String) {
    output.extend(std::iter::repeat_n(' ', indent));
    match block {
        OwnedBlock::Parsed {
            marker,
            head,
            children,
            raw,
        } => {
            if head.is_empty() && children.is_empty() {
                if let Some(text) = raw {
                    output.push('`');
                    if let Some(marker) = marker {
                        output.push_str(marker);
                    }
                    output.push('"');
                    render_owned_raw_text(text, indent, output);
                    return;
                }
            }
            if let Some(marker) = marker {
                output.push('`');
                output.push_str(marker);
                if !head.is_empty() {
                    output.push(' ');
                }
            }
            render_owned_inlines(head, marker.is_some(), indent, output);
            if !children.is_empty() {
                if !head.is_empty()
                    || matches!(
                        children.first(),
                        Some(OwnedBlock::Parsed { marker: None, .. })
                    )
                {
                    output.push_str("\n\n");
                } else {
                    output.push('\n');
                }
                render_owned_blocks(children, indent + 1, output);
            }
            if let Some(text) = raw {
                output.push('\n');
                output.extend(std::iter::repeat_n(' ', indent + 1));
                output.push_str("`\"");
                render_owned_raw_text(text, indent + 1, output);
            }
        }
        OwnedBlock::Verbatim { marker, text } => {
            output.push('`');
            if let Some(marker) = marker {
                output.push_str(marker);
            }
            output.push('"');
            render_owned_raw_text(text, indent, output);
        }
    }
}

fn render_owned_raw_text(text: &str, indent: usize, output: &mut String) {
    if text.is_empty() {
        return;
    }
    output.push('\n');
    for (index, line) in text.split_terminator('\n').enumerate() {
        if index > 0 {
            output.push('\n');
        }
        output.extend(std::iter::repeat_n(' ', indent + 1));
        if !line.is_empty() {
            output.push_str(line);
        }
    }
    if text.ends_with('\n') {
        output.push('\n');
    }
}

fn render_owned_inlines(
    inlines: &[OwnedInline],
    nested: bool,
    continuation_indent: usize,
    output: &mut String,
) {
    for inline in inlines {
        render_owned_inline(inline, nested, continuation_indent, output, true);
    }
}

fn append_owned_plain_text(inlines: &[OwnedInline], output: &mut String) {
    for inline in inlines {
        match inline {
            OwnedInline::Text(text) | OwnedInline::Verbatim { text, .. } => output.push_str(text),
            OwnedInline::Space(_) | OwnedInline::SoftBreak => output.push(' '),
            OwnedInline::ArgumentSeparator => {}
            OwnedInline::Element { members, .. } => {
                for (index, member) in members.iter().enumerate() {
                    if index > 0 {
                        output.push(' ');
                    }
                    match member {
                        OwnedInlineMember::ParsedArgument(argument) => {
                            append_owned_plain_text(argument, output);
                        }
                        OwnedInlineMember::VerbatimArgument(argument) => output.push_str(argument),
                        OwnedInlineMember::Child(child) => {
                            append_owned_plain_text(std::slice::from_ref(child.as_ref()), output);
                        }
                    }
                }
            }
        }
    }
}

fn render_owned_inline(
    inline: &OwnedInline,
    _nested: bool,
    continuation_indent: usize,
    output: &mut String,
    _introduced: bool,
) {
    match inline {
        OwnedInline::Text(text) => {
            output.push_str(&escape_parsed_text(text));
        }
        OwnedInline::Space(space) => output.push_str(space),
        OwnedInline::SoftBreak => output.push(' '),
        OwnedInline::ArgumentSeparator => {}
        OwnedInline::Element { kind, members } => {
            if !kind.is_empty() {
                output.push('`');
                output.push_str(kind);
            }
            output.push('{');
            for (index, member) in members.iter().enumerate() {
                if index > 0 {
                    output.push(' ');
                }
                match member {
                    OwnedInlineMember::ParsedArgument(argument) => {
                        render_owned_inlines(argument, true, continuation_indent, output);
                    }
                    OwnedInlineMember::VerbatimArgument(argument) => {
                        output.push('`');
                        render_owned_verbatim_payload(argument, output);
                    }
                    OwnedInlineMember::Child(child) => {
                        render_owned_inline(child, true, continuation_indent, output, true);
                    }
                }
            }
            output.push('}');
        }
        OwnedInline::Verbatim { kind, text } => {
            output.push('`');
            if !kind.is_empty() {
                output.push_str(kind);
            }
            render_owned_verbatim_payload(text, output);
        }
    }
}

fn escape_parsed_text(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '`' => output.push_str("``"),
            '{' | '}' => {
                output.push('`');
                output.push(character);
            }
            _ => output.push(character),
        }
    }
    output
}

fn escape_authored_text(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '`' => output.push_str("``"),
            '{' | '}' => {
                output.push('`');
                output.push(character);
            }
            _ => output.push(character),
        }
    }
    output
}

fn owned_authored_text(text: &str) -> Vec<OwnedInline> {
    let mut output = Vec::new();
    let mut start = 0;
    let mut in_spaces = text.starts_with(' ');
    for (index, character) in text.char_indices() {
        let is_space = character == ' ';
        if is_space == in_spaces {
            continue;
        }
        let segment = text[start..index].to_string();
        output.push(if in_spaces {
            OwnedInline::Space(segment)
        } else {
            OwnedInline::Text(segment)
        });
        start = index;
        in_spaces = is_space;
    }
    if start < text.len() {
        let segment = text[start..].to_string();
        output.push(if in_spaces {
            OwnedInline::Space(segment)
        } else {
            OwnedInline::Text(segment)
        });
    }
    output
}

fn render_owned_verbatim_payload(text: &str, output: &mut String) {
    if !text.contains('"') && !text.starts_with('{') {
        output.push('"');
        output.push_str(text);
        output.push('"');
    } else {
        let quotes = minimum_quote_count(text).max(1);
        output.push_str(&"\"".repeat(quotes));
        output.push('{');
        output.push_str(text);
        output.push('}');
        output.push_str(&"\"".repeat(quotes));
    }
}

fn minimum_quote_count(text: &str) -> usize {
    (0..)
        .find(|quotes| !text.contains(&format!("}}{}", "\"".repeat(*quotes))))
        .expect("a finite string has a safe quote count")
}

fn line_ending(source: &str) -> &str {
    if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn has_block_range(blocks: &[Block], target: &Range<usize>) -> bool {
    blocks
        .iter()
        .any(|block| block.range() == target || has_block_range(block.children(), target))
}

fn block_with_range<'a>(blocks: &'a [Block], target: &Range<usize>) -> Option<&'a Block> {
    for block in blocks {
        if block.range() == target {
            return Some(block);
        }
        if let Some(found) = block_with_range(block.children(), target) {
            return Some(found);
        }
    }
    None
}

fn block_path_with_range(block: &Block, target: &Range<usize>) -> Option<Vec<usize>> {
    if block.range() == target {
        return Some(Vec::new());
    }
    if !(block.range().start <= target.start && target.end <= block.range().end) {
        return None;
    }
    for (index, child) in block.children().iter().enumerate() {
        if let Some(mut path) = block_path_with_range(child, target) {
            path.insert(0, index);
            return Some(path);
        }
    }
    None
}

fn block_at_path<'a>(mut block: &'a Block, path: &[usize]) -> Option<&'a Block> {
    for index in path {
        block = block.children().get(*index)?;
    }
    Some(block)
}

fn owned_at_path_mut<'a>(
    mut block: &'a mut OwnedBlock,
    path: &[usize],
) -> Option<&'a mut OwnedBlock> {
    for index in path {
        block = block.children_mut()?.get_mut(*index)?;
    }
    Some(block)
}

fn remove_owned_at_path(block: &mut OwnedBlock, path: &[usize]) -> Option<OwnedBlock> {
    let (index, parent_path) = path.split_last()?;
    let parent = owned_at_path_mut(block, parent_path)?;
    let children = parent.children_mut()?;
    (*index < children.len()).then(|| children.remove(*index))
}

fn adjust_owned_path_after_removal(path: &mut [usize], removed: &[usize]) {
    for (target, source) in path.iter_mut().zip(removed) {
        if *target == *source {
            continue;
        }
        if *source < *target {
            *target -= 1;
        }
        break;
    }
}

fn parsed_block_with_range<'a>(
    blocks: &'a [Block],
    target: &Range<usize>,
) -> Option<&'a plumb_syntax::ParsedBlock> {
    for block in blocks {
        let Block::Parsed(block) = block else {
            continue;
        };
        if &block.range == target {
            return Some(block);
        }
        if let Some(found) = parsed_block_with_range(&block.children, target) {
            return Some(found);
        }
    }
    None
}

fn deepest_sibling_at(blocks: &[Block], offset: usize) -> Option<(&[Block], usize)> {
    for (index, block) in blocks.iter().enumerate() {
        if block.range().start <= offset && offset <= block.range().end {
            if let Some(found) = deepest_sibling_at(block.children(), offset) {
                return Some(found);
            }
            return Some((blocks, index));
        }
    }
    None
}

fn alignment_shape<'a>(source: &str, block: &'a Block) -> Option<(&'a str, usize)> {
    let Block::Parsed(block) = block else {
        return None;
    };
    let marker = block.mark.as_ref()?.marker.as_str();
    let argument_count = block.content.positional_elements().count();
    let head = &source[block.content.range.clone()];
    (block.children.is_empty()
        && argument_count >= 2
        && !head.contains(['\r', '\n', '\t'])
        && positional_elements_have_space_boundaries(source, &block.content))
    .then_some((marker, argument_count))
}

fn argument_alignment_width(source: &str, block: &ParsedBlock, column: usize) -> usize {
    let element = block
        .content
        .positional_elements()
        .nth(column)
        .expect("alignment column exists");
    let raw = inline_range(element);
    if column == 0 {
        let line_start = source[..block.range.start]
            .rfind('\n')
            .map_or(0, |newline| newline + 1);
        UnicodeWidthStr::width(&source[line_start..raw.end])
    } else {
        UnicodeWidthStr::width(&source[raw.clone()])
    }
}

fn positional_elements_have_space_boundaries(source: &str, content: &InlineContent) -> bool {
    if content.items.iter().any(|inline| {
        matches!(
            inline,
            Inline::Group { mark: Some(mark), .. }
                if matches!(mark.marker.as_str(), "@" | "+" | "=")
        )
    }) {
        return false;
    }
    let elements = content.positional_elements().collect::<Vec<_>>();
    elements.windows(2).all(|elements| {
        let gap = inline_range(elements[0]).end..inline_range(elements[1]).start;
        !gap.is_empty() && source[gap].bytes().all(|byte| byte == b' ')
    })
}

fn push_changed_padding_edit(
    parsed: &ParsedDocument,
    edits: &mut Vec<TextEdit>,
    range: Range<usize>,
    spaces: usize,
) -> Result<(), EditError> {
    let replacement = " ".repeat(spaces);
    if parsed.source[range.clone()] != replacement {
        edits.push(TextEdit::replace(parsed, range, replacement)?);
    }
    Ok(())
}

fn align_owned_sibling_arguments(blocks: &mut [OwnedBlock]) {
    let mut start = 0;
    while start < blocks.len() {
        let Some(argument_count) = owned_property_argument_count(&blocks[start]) else {
            start += 1;
            continue;
        };
        let mut end = start + 1;
        while end < blocks.len()
            && owned_property_argument_count(&blocks[end]) == Some(argument_count)
        {
            end += 1;
        }
        if end - start >= 2 {
            align_owned_argument_run(&mut blocks[start..end], argument_count);
        }
        start = end;
    }
}

fn align_owned_property_run_near(blocks: &mut [OwnedBlock], position: usize) {
    if position < blocks.len() && owned_property_argument_count(&blocks[position]).is_some() {
        align_owned_property_run_at(blocks, position);
    } else if position > 0 && owned_property_argument_count(&blocks[position - 1]).is_some() {
        align_owned_property_run_at(blocks, position - 1);
    }
}

fn align_owned_property_run_at(blocks: &mut [OwnedBlock], index: usize) {
    let Some(argument_count) = blocks.get(index).and_then(owned_property_argument_count) else {
        return;
    };
    let start = (0..index)
        .rev()
        .take_while(|candidate| {
            owned_property_argument_count(&blocks[*candidate]) == Some(argument_count)
        })
        .last()
        .unwrap_or(index);
    let end = (index + 1..blocks.len())
        .take_while(|candidate| {
            owned_property_argument_count(&blocks[*candidate]) == Some(argument_count)
        })
        .last()
        .map_or(index + 1, |candidate| candidate + 1);
    if end - start >= 2 {
        align_owned_argument_run(&mut blocks[start..end], argument_count);
    }
}

fn owned_property_argument_count(block: &OwnedBlock) -> Option<usize> {
    let OwnedBlock::Parsed {
        marker,
        head,
        children,
        raw,
    } = block
    else {
        return None;
    };
    if head.iter().any(owned_inline_declaration) {
        return None;
    }
    let argument_count = owned_positional_indices(head).len();
    let mut rendered = String::new();
    render_owned_inlines(head, true, 0, &mut rendered);
    (marker.as_deref() == Some("=")
        && children.is_empty()
        && raw.is_none()
        && argument_count >= 2
        && !rendered.contains(['\r', '\n', '\t']))
    .then_some(argument_count)
}

fn align_owned_argument_run(blocks: &mut [OwnedBlock], argument_count: usize) {
    let mut normalized = blocks
        .iter()
        .map(|block| {
            let OwnedBlock::Parsed { head, .. } = block else {
                unreachable!("owned property run contains parsed blocks")
            };
            normalized_owned_arguments(head)
        })
        .collect::<Vec<_>>();
    let mut widths = vec![0; argument_count - 1];
    for arguments in &normalized {
        for (column, maximum) in widths.iter_mut().enumerate() {
            *maximum = (*maximum).max(owned_argument_width(&arguments[column]));
        }
    }
    for (block, arguments) in blocks.iter_mut().zip(&mut normalized) {
        let OwnedBlock::Parsed { head, .. } = block else {
            unreachable!("owned property run contains parsed blocks")
        };
        head.clear();
        for (column, argument) in std::mem::take(arguments).into_iter().enumerate() {
            let width = owned_argument_width(&argument);
            head.extend(argument);
            if column + 1 < argument_count {
                head.push(OwnedInline::Space(" ".repeat(widths[column] - width + 1)));
                head.push(OwnedInline::ArgumentSeparator);
            }
        }
    }
}

fn normalized_owned_arguments(head: &[OwnedInline]) -> Vec<Vec<OwnedInline>> {
    let mut arguments = if head
        .iter()
        .any(|inline| matches!(inline, OwnedInline::ArgumentSeparator))
    {
        split_owned_arguments(head.to_vec())
    } else {
        owned_positional_indices(head)
            .into_iter()
            .map(|index| vec![head[index].clone()])
            .collect()
    };
    if arguments.is_empty() {
        return arguments;
    }
    let last = arguments.len() - 1;
    for (index, argument) in arguments.iter_mut().enumerate() {
        if index > 0 {
            trim_owned_padding_start(argument);
        }
        if index < last {
            trim_owned_padding_end(argument);
        }
    }
    arguments
}

fn split_owned_arguments(head: Vec<OwnedInline>) -> Vec<Vec<OwnedInline>> {
    if head.is_empty() {
        return Vec::new();
    }
    let mut arguments = vec![Vec::new()];
    for inline in head {
        if matches!(inline, OwnedInline::ArgumentSeparator) {
            arguments.push(Vec::new());
        } else {
            arguments.last_mut().unwrap().push(inline);
        }
    }
    arguments
}

fn padded_owned_arguments(mut arguments: Vec<Vec<OwnedInline>>) -> Vec<OwnedInline> {
    for argument in &mut arguments {
        trim_owned_padding_start(argument);
        trim_owned_padding_end(argument);
        if owned_positional_indices(argument).len() > 1 {
            *argument = vec![OwnedInline::Element {
                kind: String::new(),
                members: vec![OwnedInlineMember::ParsedArgument(std::mem::take(argument))],
            }];
        }
    }
    let mut head = Vec::new();
    for (index, argument) in arguments.into_iter().enumerate() {
        if index > 0 {
            head.push(OwnedInline::ArgumentSeparator);
            head.push(OwnedInline::Space(" ".to_string()));
        }
        head.extend(argument);
    }
    head
}

fn trim_owned_padding_start(argument: &mut Vec<OwnedInline>) {
    while let Some(OwnedInline::Space(space)) = argument.first_mut() {
        *space = space.trim_start_matches(' ').to_string();
        if !space.is_empty() {
            break;
        }
        argument.remove(0);
    }
}

fn trim_owned_padding_end(argument: &mut Vec<OwnedInline>) {
    while let Some(OwnedInline::Space(space)) = argument.last_mut() {
        *space = space.trim_end_matches(' ').to_string();
        if !space.is_empty() {
            break;
        }
        argument.pop();
    }
}

fn owned_argument_width(argument: &[OwnedInline]) -> usize {
    let mut rendered = String::new();
    render_owned_inlines(argument, true, 0, &mut rendered);
    UnicodeWidthStr::width(rendered.as_str())
}

fn trailing_line_breaks(source: &str) -> usize {
    source
        .as_bytes()
        .iter()
        .rev()
        .take_while(|byte| **byte == b'\n' || **byte == b'\r')
        .filter(|byte| **byte == b'\n')
        .count()
}

fn indent_fragment(formatted: &str, indent: &str, newline: &str) -> String {
    let mut output = String::with_capacity(formatted.len() + indent.len());
    for line in formatted.split_inclusive(newline) {
        let content = line.strip_suffix(newline).unwrap_or(line);
        if !content.is_empty() {
            output.push_str(indent);
            output.push_str(content);
        }
        if line.ends_with(newline) {
            output.push_str(newline);
        }
    }
    if !formatted.is_empty() && !formatted.ends_with(newline) {
        output.push_str(newline);
    }
    output
}

fn validate_range(source: &str, range: &Range<usize>) -> Result<(), EditError> {
    if range.start > range.end
        || range.end > source.len()
        || !source.is_char_boundary(range.start)
        || !source.is_char_boundary(range.end)
    {
        return Err(EditError::InvalidRange);
    }
    Ok(())
}

pub fn finalize(
    parsed: &ParsedDocument,
    affected: Range<usize>,
    mut logical_edits: Vec<TextEdit>,
) -> Result<TextEdit, EditError> {
    let source = &parsed.source;
    validate_range(source, &affected)?;
    if logical_edits.iter().any(|edit| {
        validate_range(source, &edit.range).is_err()
            || edit.range.start < affected.start
            || edit.range.end > affected.end
    }) {
        return Err(EditError::InvalidRange);
    }

    logical_edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
    if logical_edits.windows(2).any(|edits| {
        edits[0].range.end > edits[1].range.start || edits[0].range.start == edits[1].range.start
    }) {
        return Err(EditError::OverlappingEdits);
    }

    let delta = logical_edits.iter().try_fold(0isize, |delta, edit| {
        let removed = isize::try_from(edit.range.len()).ok()?;
        let inserted = isize::try_from(edit.new_text.len()).ok()?;
        delta.checked_add(inserted.checked_sub(removed)?)
    });
    let delta = delta.ok_or(EditError::InvalidRange)?;
    let modified_end = affected
        .end
        .checked_add_signed(delta)
        .ok_or(EditError::InvalidRange)?;

    let mut modified = source.clone();
    for edit in logical_edits.iter().rev() {
        modified.replace_range(edit.range.clone(), &edit.new_text);
    }
    let modified_parsed = plumb_syntax::parse(&modified);
    if parsed.syntax.blocks.is_empty() {
        let new_text = plumb_format::format_parsed(&modified_parsed)
            .map_err(|_| EditError::GeneratedInvalid)?;
        return Ok(TextEdit {
            range: affected,
            new_text,
        });
    }
    if modified_end == affected.start {
        return Ok(TextEdit {
            range: affected,
            new_text: String::new(),
        });
    }
    let formatted = match plumb_format::format_parsed_block_range(
        &modified_parsed,
        affected.start..modified_end,
    ) {
        Ok(formatted) => formatted,
        Err(plumb_format::FormatError::InvalidBlockRange) => {
            let block_end = block_end_with_start(&modified_parsed.syntax.blocks, affected.start)
                .ok_or(EditError::GeneratedInvalid)?;
            let boundary_gap = block_end.min(modified_end)..block_end.max(modified_end);
            if !modified[boundary_gap]
                .chars()
                .all(|character| matches!(character, '\r' | '\n'))
            {
                return Err(EditError::GeneratedInvalid);
            }
            plumb_format::format_parsed_block_range(&modified_parsed, affected.start..block_end)
                .map_err(|_| EditError::GeneratedInvalid)?
        }
        Err(plumb_format::FormatError::InvalidSyntax) => {
            return Err(EditError::GeneratedInvalid);
        }
    };
    if formatted.range.end < modified_end
        && !modified[formatted.range.end..modified_end]
            .chars()
            .all(|character| matches!(character, '\r' | '\n'))
    {
        return Err(EditError::InvalidRange);
    }
    let original_end = formatted
        .range
        .end
        .checked_add_signed(delta.checked_neg().ok_or(EditError::InvalidRange)?)
        .ok_or(EditError::InvalidRange)?;
    Ok(TextEdit {
        range: formatted.range.start..original_end,
        new_text: formatted.new_text,
    })
}

fn block_end_with_start(blocks: &[Block], start: usize) -> Option<usize> {
    for block in blocks {
        if block.range().start == start {
            return Some(block.range().end);
        }
        if block.range().start < start && start < block.range().end {
            if let Some(end) = block_end_with_start(block.children(), start) {
                return Some(end);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use plumb_syntax::{parse, Block};

    #[test]
    fn block_attributes_group_keys_containing_spaces() {
        let attribute = OwnedAttribute::bare("key part", "value[part]");
        assert_eq!(attribute.render_block(), "`= {key part} value[part]");

        let formatted = attribute.into_block().format().unwrap();
        assert_eq!(formatted, "`= {key part} value[part]\n");
        assert!(parse(&formatted).is_valid(), "{formatted}");
    }

    #[test]
    fn owned_marked_block_renders_children_before_one_raw_tail() {
        let block = OwnedBlock::Parsed {
            marker: Some("rust".into()),
            head: Vec::new(),
            children: vec![
                OwnedAttribute::id("example").into_block(),
                OwnedBlock::marked("note", "nested"),
            ],
            raw: Some("fn main() {}\n".into()),
        };
        let formatted = block.format().unwrap();
        assert_eq!(
            formatted,
            "`rust\n `@ example\n\n `note nested\n\n `\"\n  fn main() {}\n"
        );
        let parsed = parse(&formatted);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let [Block::Parsed(owner)] = parsed.syntax.blocks.as_slice() else {
            panic!("expected parsed raw owner");
        };
        assert_eq!(owner.children.len(), 3);
        assert!(matches!(owner.children.last(), Some(Block::Verbatim(_))));
    }

    #[test]
    fn owned_childless_marked_raw_tail_is_adjacent_to_its_head() {
        let block = OwnedBlock::Parsed {
            marker: Some("rust".into()),
            head: Vec::new(),
            children: Vec::new(),
            raw: Some("fn main() {}\n".into()),
        };
        assert_eq!(block.format().unwrap(), "`rust\"\n fn main() {}\n");
    }

    #[test]
    fn adding_attributes_explicitizes_anonymous_raw() {
        let block = OwnedBlock::Verbatim {
            marker: None,
            text: "raw".into(),
        }
        .with_attributes(vec![OwnedAttribute::id("example")]);
        assert_eq!(block.format().unwrap(), "`()\n `@ example\n\n `\"\n  raw");
    }

    #[test]
    fn replacing_an_interleaved_declaration_preserves_ordinary_children() {
        let source = "`task Work\n\n `note before\n\n `@ old\n\n `note after\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Parsed(task) = &parsed.syntax.blocks[0] else {
            panic!("expected task");
        };
        let attrs = &task.mark.as_ref().unwrap().attrs;
        let mut session = EditSession::new(&parsed, task.range.clone()).unwrap();
        session
            .replace_attribute(attrs, 0, OwnedAttribute::id("new"))
            .unwrap();
        let edit = session.finish().unwrap();
        let edited = apply_text_edits(source.to_string(), vec![edit]).unwrap();
        assert!(edited.contains("`note before"));
        assert!(edited.contains("`@ new"));
        assert!(edited.contains("`note after"));
        assert!(!edited.contains("`@ old"));
        assert!(parse(&edited).is_valid(), "{edited}");
    }

    #[test]
    fn inserts_direct_declarations_into_an_owner_without_a_slot() {
        let source = "`task Work\n";
        let parsed = parse(source);
        let Block::Parsed(task) = &parsed.syntax.blocks[0] else {
            panic!("expected task");
        };
        let mark = task.mark.as_ref().unwrap();
        let mut session = EditSession::new(&parsed, task.range.clone()).unwrap();
        session
            .insert_attributes(
                &mark.attrs,
                mark.marker_range.end,
                [
                    (AttributePosition::Last, OwnedAttribute::id("work")),
                    (
                        AttributePosition::Last,
                        OwnedAttribute::bare("created", "2026-08-26T00:00:00+08:00"),
                    ),
                ],
            )
            .unwrap();
        let edit = session.finish().unwrap();
        assert_eq!(
            edit.new_text,
            "`task Work\n\n `@ work\n\n `= created 2026-08-26T00:00:00+08:00\n"
        );
    }

    #[test]
    fn inserting_before_a_noncanonical_declaration_preserves_ownership() {
        let source = "`task Work\n  `+ keep\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Parsed(task) = &parsed.syntax.blocks[0] else {
            panic!("expected task");
        };
        let mark = task.mark.as_ref().unwrap();
        let mut session = EditSession::new(&parsed, task.range.clone()).unwrap();
        session
            .insert_attribute(
                &mark.attrs,
                mark.marker_range.end,
                AttributePosition::First,
                OwnedAttribute::id("work"),
            )
            .unwrap();
        let edit = session.finish().unwrap();
        assert_eq!(edit.new_text, "`task Work\n\n `@ work\n\n `+ keep\n");
    }

    #[test]
    fn appending_a_declaration_before_a_following_sibling_stays_valid() {
        let source = "`task Closed\n\n `@ closed\n\n `= done 2026-07-20T09:00:00Z\n\n`task Existing\n\n `@ existing\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Parsed(task) = &parsed.syntax.blocks[0] else {
            panic!("expected task");
        };
        let mark = task.mark.as_ref().unwrap();
        let mut session = EditSession::new(&parsed, task.range.clone()).unwrap();
        session
            .insert_attribute(
                &mark.attrs,
                mark.marker_range.end,
                AttributePosition::Last,
                OwnedAttribute::quoted("created", "2026-07-20T10:00:00+08:00"),
            )
            .unwrap();
        let edit = session.finish().unwrap();
        let edited = apply_text_edits(source.to_string(), vec![edit]).unwrap();
        assert!(parse(&edited).is_valid(), "{edited}");
        assert!(edited.contains("`= created 2026-07-20T10:00:00+08:00\n"));
        assert!(edited.contains("`task Existing"));
    }

    #[test]
    fn rewrites_marked_owners_with_first_attributes_in_one_revision() {
        let source = "`task First\n `@ first\n\n`event Second\n";
        let parsed = parse(source);
        let first = parsed.syntax.blocks[0].range().clone();
        let second = parsed.syntax.blocks[1].range().clone();
        let rewrites = vec![
            MarkedOwnerRewrite {
                owner_range: first,
                marker: "-".to_string(),
                first_attribute: Some(OwnedAttribute::class("task")),
            },
            MarkedOwnerRewrite {
                owner_range: second,
                marker: "-".to_string(),
                first_attribute: Some(OwnedAttribute::class("event")),
            },
        ];
        let edits = rewrite_marked_owners(&parsed, &rewrites).unwrap();
        assert_eq!(edits.len(), 4);
        let edited = apply_text_edits(source.to_string(), edits).unwrap();
        assert_eq!(
            edited,
            "`- First\n\n `+ task\n\n `@ first\n\n`- Second\n `+ event\n"
        );
        assert!(parse(&edited).is_valid(), "{edited}");
        assert_eq!(
            rewrite_marked_owners(
                &parsed,
                &[MarkedOwnerRewrite {
                    owner_range: 1..3,
                    marker: "-".to_string(),
                    first_attribute: None,
                }]
            ),
            Err(EditError::InvalidRange)
        );
    }

    #[test]
    fn marked_owner_rewrite_preserves_crlf_when_adding_a_first_attribute() {
        let source = "`task Work\r\n";
        let parsed = parse(source);
        let edits = rewrite_marked_owners(
            &parsed,
            &[MarkedOwnerRewrite {
                owner_range: parsed.syntax.blocks[0].range().clone(),
                marker: "-".to_string(),
                first_attribute: Some(OwnedAttribute::class("task")),
            }],
        )
        .unwrap();
        let edited = apply_text_edits(source.to_string(), edits).unwrap();
        assert_eq!(edited, "`- Work\r\n `+ task\r\n");
        assert!(parse(&edited).is_valid(), "{edited:?}");
    }

    #[test]
    fn formats_parsed_revisions_only_through_the_edit_boundary() {
        let source = "`meta\n   `= title\n\n      Unified command\n";
        let parsed = parse(source);
        let edits = format(&parsed, FormatScope::Document).unwrap();
        assert_eq!(format_green(&GreenDocument::parse(source)).unwrap(), edits);
        let formatted = apply_text_edits(source.to_string(), edits).unwrap();
        assert_eq!(formatted, "`meta\n `= title\n\n  Unified command\n");
        assert!(format(&parse(&formatted), FormatScope::Document)
            .unwrap()
            .is_empty());

        assert_eq!(
            format(&parse("`broken{\n"), FormatScope::Document),
            Err(EditError::GeneratedInvalid)
        );
        assert_eq!(
            format(
                &parse("Paragraph.\n"),
                FormatScope::ContainedBlocks(0..usize::MAX)
            ),
            Err(EditError::InvalidRange)
        );
    }

    #[test]
    fn green_block_ownership_and_replacement_match_materialized_edits() {
        let source = "Prelude\n\n`- Task\n `+ task\n `@ task\n\n`note After\n";
        let parsed = parse(source);
        let green = GreenDocument::parse(source);
        let Block::Parsed(task) = &parsed.syntax.blocks[1] else {
            panic!("task is parsed")
        };
        let mut expected = OwnedBlock::from_parsed(source, task);
        assert_eq!(
            own_green_block(&green, task.range.clone()).unwrap(),
            expected
        );
        expected.push_attribute(OwnedAttribute::quoted("done", "2026-09-05T12:00:00Z"));
        assert_eq!(
            replace_green_block(&green, task.range.clone(), &expected).unwrap(),
            replace_owned_block(&parsed, task.range.clone(), &expected).unwrap()
        );
        assert_eq!(
            replace_green_blocks(
                &green,
                task.range.clone(),
                &[expected.clone(), expected.clone()],
            )
            .unwrap(),
            replace_owned_blocks(&parsed, task.range.clone(), &[expected.clone(), expected])
                .unwrap()
        );
        assert_eq!(
            remove_green_block(&green, task.range.clone()).unwrap(),
            remove_block(&parsed, task.range.clone()).unwrap()
        );
    }

    #[test]
    fn green_block_insertions_match_materialized_edits() {
        let source = "Prelude\n\n`parent\n `a One\n `b Two\n\nTail\n";
        let parsed = parse(source);
        let green = GreenDocument::parse(source);
        let inserted = OwnedBlock::marked("note", "Inserted");
        let assert_same_source = |actual: TextEdit, expected: TextEdit| {
            assert_eq!(
                apply_text_edits(source.to_string(), vec![actual]).unwrap(),
                apply_text_edits(source.to_string(), vec![expected]).unwrap()
            );
        };

        let mut expected = EditSession::new(&parsed, 0..0).unwrap();
        expected.insert_blocks(0, &[inserted.clone()]).unwrap();
        assert_same_source(
            prepend_green_blocks(&green, &[inserted.clone()]).unwrap(),
            expected.finish().unwrap(),
        );

        let last = parsed.syntax.blocks.last().unwrap().range().clone();
        let mut expected = EditSession::new(&parsed, last.clone()).unwrap();
        expected
            .insert_sibling_blocks(&last, &[inserted.clone()])
            .unwrap();
        assert_same_source(
            append_green_blocks(&green, &[inserted.clone()]).unwrap(),
            expected.finish().unwrap(),
        );

        let first = parsed.syntax.blocks[0].range().clone();
        let mut expected = EditSession::new(&parsed, first.clone()).unwrap();
        expected
            .insert_sibling_blocks(&first, &[inserted.clone()])
            .unwrap();
        assert_same_source(
            insert_green_top_level_blocks_after(&green, first, &[inserted.clone()]).unwrap(),
            expected.finish().unwrap(),
        );

        let Block::Parsed(parent) = &parsed.syntax.blocks[1] else {
            panic!("parent is parsed")
        };
        let after = parent.children[0].range().clone();
        let mut expected_parent = OwnedBlock::from_parsed(source, parent);
        expected_parent
            .children_mut()
            .unwrap()
            .insert(1, inserted.clone());
        assert_same_source(
            insert_green_child_blocks(&green, parent.range.clone(), Some(&after), &[inserted])
                .unwrap(),
            replace_owned_block(&parsed, parent.range.clone(), &expected_parent).unwrap(),
        );
        let appended_child = OwnedBlock::marked("note", "Appended");
        let mut expected_parent = OwnedBlock::from_parsed(source, parent);
        expected_parent
            .children_mut()
            .unwrap()
            .push(appended_child.clone());
        assert_same_source(
            insert_green_child_blocks(&green, parent.range.clone(), None, &[appended_child])
                .unwrap(),
            replace_owned_block(&parsed, parent.range.clone(), &expected_parent).unwrap(),
        );

        let Block::Parsed(child) = &parent.children[0] else {
            panic!("child is parsed")
        };
        assert_eq!(
            own_green_block_paths(
                &green,
                parent.range.clone(),
                &[parent.range.clone(), parent.children[1].range().clone()],
            )
            .unwrap(),
            GreenOwnedBlockPaths {
                block: OwnedBlock::from_parsed(source, parent),
                target_paths: vec![vec![], vec![1]],
            }
        );
        assert_eq!(
            own_deepest_green_marked_block(&green, child.range.end, &["a", "b"]).unwrap(),
            GreenOwnedBlockTarget {
                range: child.range.clone(),
                block: OwnedBlock::from_parsed(source, child),
            }
        );
        let next = next_green_sibling_block(&green, child.range.clone()).unwrap();
        assert_eq!(next.range, parent.children[1].range().clone());
        assert_eq!(next.block.head_plain_text().as_deref(), Some("Two"));
        let top_level_next =
            next_green_sibling_block(&green, parsed.syntax.blocks[0].range().clone()).unwrap();
        assert_eq!(top_level_next.range, parent.range);
        let groups = own_green_marked_block_groups(
            &green,
            source.find("One").unwrap()..source.find("Two").unwrap() + "Two".len(),
            &["a", "b"],
        )
        .unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].range, parent.range);
        assert_eq!(
            groups[0]
                .candidates
                .iter()
                .map(|candidate| candidate.path.clone())
                .collect::<Vec<_>>(),
            vec![vec![0], vec![1]]
        );
        assert_eq!(
            groups[0].candidates[0]
                .next_sibling
                .as_ref()
                .and_then(|sibling| sibling.block.head_plain_text())
                .as_deref(),
            Some("Two")
        );
        assert_eq!(
            green_block_attribute_target(&green, source.find("One").unwrap()).unwrap(),
            GreenBlockAttributeTarget {
                range: child.range.clone(),
                seed: "One".to_string(),
                has_id: false,
            }
        );
        let mark = child.mark.as_ref().unwrap();
        let mut expected = EditSession::new(&parsed, child.range.clone()).unwrap();
        expected
            .insert_attribute(
                &mark.attrs,
                mark.marker_range.end,
                AttributePosition::First,
                OwnedAttribute::id("one"),
            )
            .unwrap();
        assert_same_source(
            insert_green_block_attribute(
                &green,
                child.range.clone(),
                AttributePosition::First,
                OwnedAttribute::id("one"),
            )
            .unwrap(),
            expected.finish().unwrap(),
        );
    }

    #[test]
    fn green_document_insertions_support_empty_source() {
        let inserted = OwnedBlock::marked("note", "Inserted");
        assert_eq!(
            prepend_green_blocks(&GreenDocument::parse(""), &[inserted]).unwrap(),
            TextEdit {
                range: 0..0,
                new_text: "`note Inserted\n".to_string(),
            }
        );
    }

    #[test]
    fn green_alignment_matches_materialized_nested_and_top_level_runs() {
        let source = "`= a one\n`= longer two\n\n`parent\n `= x one\n `= wider two\n";
        let parsed = parse(source);
        let green = GreenDocument::parse(source);
        for offset in [source.find("a one").unwrap(), source.find("x one").unwrap()] {
            assert_eq!(
                align_green_block_arguments(&green, offset).unwrap(),
                align_block_arguments(&parsed, offset).unwrap()
            );
        }
    }

    #[test]
    fn green_move_handles_same_root_cross_root_promotion_and_append() {
        let source = "`root Left\n `item A\n `item B\n\n`root Right\n `item C\n";
        let parsed = parse(source);
        let green = GreenDocument::parse(source);
        let Block::Parsed(left) = &parsed.syntax.blocks[0] else {
            panic!("left root is parsed")
        };
        let Block::Parsed(right) = &parsed.syntax.blocks[1] else {
            panic!("right root is parsed")
        };
        let a = left.children[0].range().clone();
        let b = left.children[1].range().clone();
        let c = right.children[0].range().clone();
        let moved = own_green_block(&green, a.clone()).unwrap();

        let same_root = apply_text_edits(
            source.to_string(),
            move_green_block(
                &green,
                a.clone(),
                Some(&left.range),
                Some(&b),
                moved.clone(),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(same_root.find("`item B").unwrap() < same_root.find("`item A").unwrap());

        let cross_root = apply_text_edits(
            source.to_string(),
            move_green_block(
                &green,
                a.clone(),
                Some(&right.range),
                Some(&c),
                moved.clone(),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(!cross_root[..cross_root.find("`root Right").unwrap()].contains("`item A"));
        assert!(cross_root.find("`item C").unwrap() < cross_root.find("`item A").unwrap());

        let promoted = apply_text_edits(
            source.to_string(),
            move_green_block(&green, a, None, Some(&left.range), moved.clone()).unwrap(),
        )
        .unwrap();
        assert!(promoted.contains("`item B\n\n`item A\n\n`root Right"));

        let appended = apply_text_edits(
            source.to_string(),
            move_green_block(&green, left.range.clone(), None, None, moved).unwrap(),
        )
        .unwrap();
        assert!(appended.find("`root Right").unwrap() < appended.find("`item A").unwrap());
        assert!(parse(&same_root).is_valid());
        assert!(parse(&cross_root).is_valid());
        assert!(parse(&promoted).is_valid());
        assert!(parse(&appended).is_valid());
    }

    #[test]
    fn owned_replacement_preserves_nested_crlf_layout() {
        let source = "`outer Parent\r\n\r\n   `old Child\r\n\r\n`next Keep\r\n";
        let parsed = parse(source);
        let Block::Parsed(outer) = &parsed.syntax.blocks[0] else {
            panic!("expected outer block");
        };
        let nested = outer.children[0].range().clone();
        let edit = replace_owned_block(&parsed, nested, &OwnedBlock::marked("new", "Replacement"))
            .unwrap();
        assert!(edit.new_text.contains("\r\n"));
        let edited = apply_text_edits(source.to_string(), vec![edit]).unwrap();
        assert!(edited.contains("   `new Replacement\r\n\r\n`next Keep"));
        assert!(parse(&edited).is_valid(), "{edited}");
    }

    #[test]
    fn attribute_positions_replace_and_remove_single_declaration_blocks() {
        let source =
            "`task Work\n\n `note before\n\n `@ old\n\n `+ keep\n\n `= created now\n\n `note after\n";
        let parsed = parse(source);
        let Block::Parsed(task) = &parsed.syntax.blocks[0] else {
            panic!("expected task");
        };
        let mark = task.mark.as_ref().unwrap();

        let mut insert = EditSession::new(&parsed, task.range.clone()).unwrap();
        insert
            .insert_attribute(
                &mark.attrs,
                mark.marker_range.end,
                AttributePosition::After(0),
                OwnedAttribute::bare("due", "tomorrow"),
            )
            .unwrap();
        let inserted =
            apply_text_edits(source.to_string(), vec![insert.finish().unwrap()]).unwrap();
        assert!(inserted.find("`@ old").unwrap() < inserted.find("`= due tomorrow\n").unwrap());
        assert!(inserted.find("`= due tomorrow\n").unwrap() < inserted.find("`+ keep").unwrap());
        assert!(inserted.contains("`note before"));
        assert!(inserted.contains("`note after"));

        let mut replace = EditSession::new(&parsed, task.range.clone()).unwrap();
        replace
            .replace_attribute(&mark.attrs, 0, OwnedAttribute::id("new"))
            .unwrap();
        let replaced =
            apply_text_edits(source.to_string(), vec![replace.finish().unwrap()]).unwrap();
        assert!(replaced.contains("`@ new"));
        assert!(!replaced.contains("`@ old"));

        let mut remove = EditSession::new(&parsed, task.range.clone()).unwrap();
        remove.remove_attribute(&mark.attrs, 2).unwrap();
        let removed = apply_text_edits(source.to_string(), vec![remove.finish().unwrap()]).unwrap();
        assert!(!removed.contains("`= created now\n"));
        assert!(removed.contains("`+ keep"));
    }

    #[test]
    fn owned_syntax_round_trips_inline_members_children_and_raw() {
        let source = "`() Head `span{text `@{id} `+{opaque} `={key bare}} and `\"raw\"\n\n `@ owner\n\n `child Body\n\n `node\"\n  payload\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let owned = OwnedBlock::from_syntax(source, &parsed.syntax.blocks[0]);
        let formatted = owned.format().unwrap();
        let reparsed = parse(&formatted);
        assert!(
            reparsed.is_valid(),
            "{formatted}\n{:?}",
            reparsed.diagnostics
        );
        assert!(formatted.contains("`span{text `@{id} `+{opaque} `={key bare}}"));
        assert!(formatted.contains("`@ owner"));
        assert!(formatted.contains("`child Body"));
        assert!(formatted.contains("`node\"\n  payload"));
    }

    #[test]
    fn owned_elements_accept_non_text_first_elements() {
        for first in [
            OwnedInlineMember::VerbatimArgument("raw".into()),
            OwnedInlineMember::Child(Box::new(OwnedInline::Element {
                kind: "note".into(),
                members: vec![OwnedInlineMember::ParsedArgument(vec![OwnedInline::Text(
                    "child".into(),
                )])],
            })),
        ] {
            let inline = OwnedInline::Element {
                kind: "owner".into(),
                members: vec![first],
            };
            let mut output = String::new();
            render_owned_inline(&inline, true, 0, &mut output, true);
            assert!(output.starts_with("`owner{"), "{output}");
            assert!(parse(format!("{output}\n")).is_valid(), "{output}");
        }
    }

    #[test]
    fn aligns_all_argument_columns_by_unicode_display_width() {
        let source = "`row 名 一 x\n`row alphabet 二二 yy\n";
        let parsed = parse(source);
        let edits = align_block_arguments(&parsed, source.find('名').unwrap()).unwrap();
        let aligned = apply_text_edits(source.to_string(), edits).unwrap();
        assert_eq!(aligned, "`row 名       一   x\n`row alphabet 二二 yy\n");
        assert!(
            align_block_arguments(&parse(&aligned), aligned.find('名').unwrap())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn alignment_uses_combining_width_and_stays_within_the_maximal_sibling_run() {
        let source =
            "`outer\n `row e\u{301} x\n `row 界 yy\n\n `other break run\n\n `row a z\n `row aa zz\n";
        let parsed = parse(source);
        let edits = align_block_arguments(&parsed, source.find("e\u{301}").unwrap()).unwrap();
        let aligned = apply_text_edits(source.to_string(), edits).unwrap();
        assert!(aligned.contains(" `row e\u{301}  x\n `row 界 yy\n"));
        assert!(aligned.contains(" `row a z\n `row aa zz\n"));
    }

    #[test]
    fn alignment_is_unavailable_for_ineligible_or_already_aligned_runs() {
        for source in [
            "`row a  b\n`row aa b\n",
            "`row a b\n`other aa b\n",
            "`row a b\n`row aa b\n\n `child detail\n",
            "`row a b\n\n`() aa b\n\n `row\"\n  payload\n",
        ] {
            let parsed = parse(source);
            assert!(parsed.is_valid(), "{source:?}: {:?}", parsed.diagnostics);
            assert!(align_block_arguments(&parsed, source.find("row").unwrap())
                .unwrap()
                .is_empty());
        }
    }

    #[test]
    fn alignment_preserves_empty_arguments_and_escaped_boundary_spaces() {
        let source =
            "`row {} x {} z\n`row long yy q z\n\n`other break run\n\n`row {a} x\n`row longer y\n";
        let parsed = parse(source);
        let before = parsed.syntax.blocks[..2]
            .iter()
            .map(|block| match block {
                Block::Parsed(block) => block
                    .content
                    .positional_elements()
                    .map(|inline| {
                        plumb_syntax::InlineContent::from_items(
                            plumb_syntax::inline_range(inline).clone(),
                            vec![inline.clone()],
                        )
                        .plain_text()
                    })
                    .collect::<Vec<_>>(),
                Block::Verbatim(_) => unreachable!(),
            })
            .collect::<Vec<_>>();
        let edits = align_block_arguments(&parsed, source.find("{} x {} z\n").unwrap()).unwrap();
        let aligned = apply_text_edits(source.to_string(), edits).unwrap();
        let reparsed = parse(&aligned);
        let after = reparsed.syntax.blocks[..2]
            .iter()
            .map(|block| match block {
                Block::Parsed(block) => block
                    .content
                    .positional_elements()
                    .map(|inline| {
                        plumb_syntax::InlineContent::from_items(
                            plumb_syntax::inline_range(inline).clone(),
                            vec![inline.clone()],
                        )
                        .plain_text()
                    })
                    .collect::<Vec<_>>(),
                Block::Verbatim(_) => unreachable!(),
            })
            .collect::<Vec<_>>();
        assert_eq!(after, before);

        let escaped = aligned.find("{a}").unwrap();
        let escaped_edits = align_block_arguments(&reparsed, escaped).unwrap();
        let escaped_aligned = apply_text_edits(aligned, escaped_edits).unwrap();
        assert!(parse(&escaped_aligned).is_valid(), "{escaped_aligned:?}");
        assert!(align_block_arguments(&parse(&escaped_aligned), escaped)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn creates_aligned_association_groups() {
        let metadata = aligned_associations(&[
            ("title", "Example"),
            ("created", "2026-08-26T00:00:00+08:00"),
        ]);
        let formatted = format_owned_blocks(&metadata, "\n").unwrap();
        assert_eq!(
            formatted,
            "`= title   Example\n`= created 2026-08-26T00:00:00+08:00\n"
        );
    }

    #[test]
    fn structured_arguments_share_one_padding_renderer() {
        assert_eq!(
            render_authored_text_arguments(&["09:00", "Event"]),
            "09:00 Event"
        );
        assert_eq!(
            render_authored_text_arguments(&["key part", "value[part]"]),
            "{key part} value[part]"
        );

        let mut event = OwnedBlock::marked("-", "old");
        event.set_head_text_arguments(["09:00", "Event"]);
        assert_eq!(event.format().unwrap(), "`- 09:00 Event\n");
    }

    #[test]
    fn prepending_a_structured_argument_preserves_rich_title_content() {
        let source = "`- title `*{rich}\n";
        let parsed = parse(source);
        let mut owned = OwnedBlock::from_syntax(source, &parsed.syntax.blocks[0]);
        owned.prepend_head_text_argument("09:00");
        let formatted = owned.format().unwrap();
        assert_eq!(formatted, "`- 09:00 {title `*{rich}}\n");
        assert!(parse(&formatted).is_valid(), "{formatted}");
    }

    #[test]
    fn property_mutations_align_only_the_affected_direct_runs() {
        let source =
            "`- Task\n\n `+ task\n\n `@ task-id\n\n `= due tomorrow\n `= priority 20\n\n `note Keep\n";
        let parsed = parse(source);
        let mut owned = OwnedBlock::from_syntax(source, &parsed.syntax.blocks[0]);
        owned.push_attribute(OwnedAttribute::quoted("created", "now"));
        let formatted = owned.format().unwrap();
        assert!(
            formatted.contains(" `= due      tomorrow\n `= priority 20\n `= created  now\n"),
            "{formatted:?}"
        );
        assert!(formatted.contains(" `note Keep\n"));

        owned.retain_attributes(
            |attribute| !matches!(attribute, OwnedAttribute::Pair { key, .. } if key == "priority"),
        );
        let removed = owned.format().unwrap();
        assert!(
            removed.contains(" `= due     tomorrow\n `= created now\n"),
            "{removed:?}"
        );
    }

    #[test]
    fn non_property_mutations_do_not_align_existing_runs() {
        let source = "`- Task\n\n `= due tomorrow\n `= priority 20\n";
        let parsed = parse(source);
        let mut owned = OwnedBlock::from_syntax(source, &parsed.syntax.blocks[0]);
        owned.prepend_attribute(OwnedAttribute::id("task-id"));
        assert!(owned
            .format()
            .unwrap()
            .contains(" `= due tomorrow\n `= priority 20\n"));

        let attributes = owned.attributes();
        let unaligned = owned.clone().with_attributes(attributes.clone());
        assert!(unaligned
            .format()
            .unwrap()
            .contains(" `= due tomorrow\n `= priority 20\n"));
        let aligned = owned.with_aligned_attributes(attributes);
        assert!(aligned
            .format()
            .unwrap()
            .contains(" `= due      tomorrow\n `= priority 20\n"));
    }

    #[test]
    fn property_removal_does_not_align_a_separate_opaque_run() {
        let source = "`- Event\n\n `= date 2026-08-30\n `= timezone +08:00\n\n `@ split\n\n `= uid opaque\n `= when legacy\n";
        let parsed = parse(source);
        let mut owned = OwnedBlock::from_syntax(source, &parsed.syntax.blocks[0]);
        owned.retain_attributes(
            |attribute| !matches!(attribute, OwnedAttribute::Pair { key, .. } if key == "date"),
        );
        let formatted = owned.format().unwrap();
        assert!(formatted.contains(" `= timezone +08:00\n"), "{formatted:?}");
        assert!(
            formatted.contains(" `= uid opaque\n `= when legacy\n"),
            "{formatted:?}"
        );
    }

    #[test]
    fn inserts_owned_metadata_and_replaces_or_removes_complete_blocks() {
        let source = "`# Existing\n";
        let parsed = parse(source);
        let metadata = [
            OwnedBlock::association("title", "Example"),
            OwnedBlock::association("created", "2026-08-26T00:00:00+08:00"),
        ];
        let mut insert = EditSession::new(&parsed, 0..0).unwrap();
        insert.insert_blocks(0, &metadata).unwrap();
        let edit = insert.finish().unwrap();
        assert_eq!(edit.range, 0..0);
        assert_eq!(
            edit.new_text,
            "`= title Example\n`= created 2026-08-26T00:00:00+08:00\n\n"
        );

        let first = parsed.syntax.blocks[0].range().clone();
        let replacement = replace_owned_block(
            &parsed,
            first.clone(),
            &OwnedBlock::marked("heading", "Replacement"),
        )
        .unwrap();
        assert_eq!(replacement.new_text, "`heading Replacement\n");
        let removal = remove_block(&parsed, first.clone()).unwrap();
        assert_eq!(removal.range, first);
        assert!(removal.new_text.is_empty());
        assert_eq!(remove_block(&parsed, 0..2), Err(EditError::InvalidRange));
    }
}

#[cfg(test)]
mod apply_tests {
    use super::*;

    #[test]
    fn structured_multi_element_arguments_remain_recognizable_properties() {
        let mut block = OwnedBlock::marked("=", "");
        block.set_head_text_arguments(["title", "Project Guide"]);
        assert_eq!(
            owned_declaration(&block),
            Some(OwnedAttribute::bare("title", "Project Guide"))
        );
        assert_eq!(block.format().unwrap(), "`= title {Project Guide}\n");
    }

    #[test]
    fn applies_valid_edits_back_to_front_and_rejects_invalid_sets() {
        let edits = vec![
            TextEdit {
                range: 0..1,
                new_text: "A".to_string(),
            },
            TextEdit {
                range: 2..3,
                new_text: "C".to_string(),
            },
        ];
        assert_eq!(
            apply_text_edits("abc".to_string(), edits),
            Ok("AbC".to_string())
        );
        assert_eq!(
            apply_text_edits(
                "abc".to_string(),
                vec![
                    TextEdit {
                        range: 0..2,
                        new_text: String::new()
                    },
                    TextEdit {
                        range: 1..3,
                        new_text: String::new()
                    },
                ],
            ),
            Err(EditError::OverlappingEdits)
        );
        assert_eq!(
            apply_text_edits(
                "é".to_string(),
                vec![TextEdit {
                    range: 1..2,
                    new_text: String::new()
                }],
            ),
            Err(EditError::InvalidRange)
        );
    }
}
