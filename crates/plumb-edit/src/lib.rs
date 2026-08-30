use std::{collections::HashMap, ops::Range};

use plumb_syntax::{
    AttrItem, Attributes, Block, Inline, InlineMember, ParsedBlock, ParsedDocument,
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
        validate_range(&parsed.source, &range)?;
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
    Ok(edits
        .into_iter()
        .map(|edit| TextEdit {
            range: edit.range,
            new_text: edit.new_text,
        })
        .collect())
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
        for column in 0..argument_count {
            let raw = &block.head.arguments[column].range;
            let content = block
                .head
                .argument(column)
                .filter(|content| !content.items.is_empty());
            let leading = content
                .as_ref()
                .map_or_else(|| raw.clone(), |content| raw.start..content.range.start);
            let trailing = content
                .as_ref()
                .map_or_else(|| raw.clone(), |content| content.range.end..raw.end);
            let leading_spaces = usize::from(column > 0);
            let trailing_spaces = (column + 1 < argument_count).then(|| {
                widths[column] - argument_alignment_width(&parsed.source, block, column) + 1
            });

            if content.is_none() {
                let spaces = leading_spaces + trailing_spaces.unwrap_or(0);
                push_changed_padding_edit(parsed, &mut edits, raw.clone(), spaces)?;
                continue;
            }
            if column > 0 {
                push_changed_padding_edit(parsed, &mut edits, leading, leading_spaces)?;
            }
            if let Some(spaces) = trailing_spaces {
                push_changed_padding_edit(parsed, &mut edits, trailing, spaces)?;
            }
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
pub enum OwnedBlock {
    Parsed {
        marker: Option<String>,
        head: Vec<OwnedInline>,
        children: Vec<OwnedBlock>,
        raw: Option<String>,
    },
    Verbatim {
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
                format!(
                    "`= {} | {}",
                    escape_authored_text(key),
                    escape_authored_text(value)
                )
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
            Self::Verbatim { text } => {
                if attributes.is_empty() {
                    Self::Verbatim { text }
                } else {
                    Self::Parsed {
                        marker: Some("()".into()),
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
                text: block.text.clone(),
            },
        }
    }

    pub fn from_parsed(source: &str, block: &ParsedBlock) -> Self {
        let mut head = block
            .head
            .arguments
            .iter()
            .flat_map(|argument| {
                argument
                    .separator_range
                    .is_some()
                    .then_some(OwnedInline::ArgumentSeparator)
                    .into_iter()
                    .chain(
                        block.head.items[argument.item_range.clone()]
                            .iter()
                            .map(OwnedInline::from_syntax),
                    )
            })
            .collect::<Vec<_>>();
        if block.mark.is_some() {
            if let Some(OwnedInline::Space(space)) = head.first_mut() {
                debug_assert!(space.starts_with(' '));
                space.remove(0);
                if space.is_empty() {
                    head.remove(0);
                }
            }
        }
        Self::Parsed {
            marker: block.mark.as_ref().map(|mark| mark.marker.clone()),
            head,
            children: block
                .children
                .iter()
                .map(|child| Self::from_syntax(source, child))
                .collect(),
            raw: block.raw.as_ref().map(|raw| raw.text.clone()),
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
        "@" => Some(OwnedAttribute::Id(plain_owned_argument(head)?)),
        "+" => Some(OwnedAttribute::Class(plain_owned_argument(head)?)),
        "=" => {
            let separator = head
                .iter()
                .position(|inline| matches!(inline, OwnedInline::ArgumentSeparator))?;
            let key = plain_owned_argument(&head[..separator])?;
            let value = plain_owned_argument(&head[separator + 1..])?;
            (!key.is_empty() && !value.is_empty()).then_some(OwnedAttribute::Pair {
                key,
                value: OwnedValue::Bare(value),
            })
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
            OwnedInline::SoftBreak
            | OwnedInline::ArgumentSeparator
            | OwnedInline::Element { .. }
            | OwnedInline::Verbatim { .. } => return None,
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
        .map(|range| range.start)
        .or_else(|| owner.raw.as_ref().map(|raw| raw.boundary_range.start));

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
    let replacement = format!(
        "{newline}{newline}{indent}{}{newline}",
        attribute.render_block()
    );
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

impl OwnedInline {
    pub fn from_syntax(inline: &Inline) -> Self {
        match inline {
            Inline::Text { text, .. } => Self::Text(text.clone()),
            Inline::Space { text, .. } => Self::Space(text.clone()),
            Inline::SoftBreak { .. } => Self::SoftBreak,
            Inline::Element { kind, members, .. } => Self::Element {
                kind: kind.clone(),
                members: members
                    .iter()
                    .map(|member| match member {
                        InlineMember::ParsedArgument(argument) => {
                            OwnedInlineMember::ParsedArgument(
                                argument
                                    .content
                                    .items
                                    .iter()
                                    .map(Self::from_syntax)
                                    .collect(),
                            )
                        }
                        InlineMember::VerbatimArgument(argument) => {
                            OwnedInlineMember::VerbatimArgument(argument.text.clone())
                        }
                        InlineMember::Child { inline, .. } => {
                            OwnedInlineMember::Child(Box::new(Self::from_syntax(inline)))
                        }
                    })
                    .collect(),
            },
            Inline::Verbatim { kind, text, .. } => Self::Verbatim {
                kind: kind.clone(),
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
            if let Some(marker) = marker {
                output.push('`');
                output.push_str(marker);
                if !head.is_empty() {
                    output.push(' ');
                }
            }
            let continuation_indent = if marker.is_some() { indent + 1 } else { indent };
            render_owned_inlines(head, marker.is_some(), continuation_indent, output);
            if !children.is_empty() {
                if head.is_empty() {
                    output.push('\n');
                } else {
                    output.push_str("\n\n");
                }
                render_owned_blocks(children, indent + 1, output);
            }
            if let Some(text) = raw {
                if children.is_empty() {
                    output.push('\n');
                } else {
                    output.push_str("\n\n");
                }
                output.extend(std::iter::repeat_n(' ', indent));
                output.push('|');
                output.push('"');
                render_owned_raw_text(text, indent, output);
            }
        }
        OwnedBlock::Verbatim { text } => {
            output.push_str("`\"");
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
        if !line.is_empty() {
            output.extend(std::iter::repeat_n(' ', indent + 1));
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

fn render_owned_inline(
    inline: &OwnedInline,
    _nested: bool,
    continuation_indent: usize,
    output: &mut String,
    introduced: bool,
) {
    match inline {
        OwnedInline::Text(text) => {
            output.push_str(&escape_parsed_text(text));
        }
        OwnedInline::Space(space) => output.push_str(space),
        OwnedInline::SoftBreak => {
            output.push('\n');
            output.extend(std::iter::repeat_n(' ', continuation_indent));
        }
        OwnedInline::ArgumentSeparator => output.push('|'),
        OwnedInline::Element { kind, members } => {
            if introduced {
                output.push('`');
            }
            output.push_str(kind);
            output.push('[');
            let needs_empty_first_argument = !matches!(
                members.first(),
                Some(OwnedInlineMember::ParsedArgument(_)) | None
            );
            for (index, member) in members.iter().enumerate() {
                if index > 0 || needs_empty_first_argument {
                    output.push('|');
                }
                match member {
                    OwnedInlineMember::ParsedArgument(argument) => {
                        render_owned_inlines(argument, true, continuation_indent, output);
                    }
                    OwnedInlineMember::VerbatimArgument(argument) => {
                        render_owned_full_verbatim_payload(argument, output);
                    }
                    OwnedInlineMember::Child(child) => {
                        render_owned_inline(child, true, continuation_indent, output, false);
                    }
                }
            }
            output.push(']');
        }
        OwnedInline::Verbatim { kind, text } => {
            if introduced {
                output.push('`');
                output.push_str(kind);
                render_owned_verbatim_payload(text, output);
            } else {
                output.push_str(kind);
                render_owned_full_verbatim_payload(text, output);
            }
        }
    }
}

fn escape_parsed_text(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '`' => output.push_str("``"),
            ' ' | '[' | ']' | '|' => {
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
            '[' | ']' | '|' => {
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

fn render_owned_full_verbatim_payload(text: &str, output: &mut String) {
    let quotes = minimum_quote_count(text).max(1);
    output.push_str(&"\"".repeat(quotes));
    output.push('[');
    output.push_str(text);
    output.push(']');
    output.push_str(&"\"".repeat(quotes));
}

fn render_owned_verbatim_payload(text: &str, output: &mut String) {
    if !text.is_empty() && !text.contains('"') && !text.starts_with('[') {
        output.push('"');
        output.push_str(text);
        output.push('"');
    } else {
        let quotes = minimum_quote_count(text).max(1);
        output.push_str(&"\"".repeat(quotes));
        output.push('[');
        output.push_str(text);
        output.push(']');
        output.push_str(&"\"".repeat(quotes));
    }
}

fn minimum_quote_count(text: &str) -> usize {
    (0..)
        .find(|quotes| !text.contains(&format!("]{}", "\"".repeat(*quotes))))
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
    let argument_count = block.head.arguments.len();
    let head = &source[block.head.range.clone()];
    (block.children.is_empty()
        && block.raw.is_none()
        && argument_count >= 2
        && !head.contains(['\r', '\n', '\t']))
    .then_some((marker, argument_count))
}

fn argument_alignment_width(source: &str, block: &ParsedBlock, column: usize) -> usize {
    let raw = &block.head.arguments[column].range;
    let content = block
        .head
        .argument(column)
        .filter(|content| !content.items.is_empty());
    if column == 0 {
        let line_start = source[..block.range.start]
            .rfind('\n')
            .map_or(0, |newline| newline + 1);
        let content_end = content
            .as_ref()
            .map_or(raw.start, |content| content.range.end);
        UnicodeWidthStr::width(&source[line_start..content_end])
    } else {
        content.as_ref().map_or(0, |content| {
            UnicodeWidthStr::width(&source[content.range.clone()])
        })
    }
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
    let argument_count = head
        .iter()
        .filter(|inline| matches!(inline, OwnedInline::ArgumentSeparator))
        .count()
        + 1;
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
            if column > 0 {
                head.push(OwnedInline::Space(" ".to_string()));
            }
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
    let mut arguments = split_owned_arguments(head.to_vec());
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
    }
    let mut head = Vec::new();
    for (index, argument) in arguments.into_iter().enumerate() {
        if index > 0 {
            head.push(OwnedInline::Space(" ".to_string()));
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
    fn block_attributes_escape_argument_delimiters() {
        let attribute = OwnedAttribute::bare("key|part", "value[part]");
        assert_eq!(attribute.render_block(), "`= key`|part | value`[part`]");

        let formatted = attribute.into_block().format().unwrap();
        assert_eq!(formatted, "`= key`|part | value`[part`]\n");
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
            "`rust\n\n `@ example\n\n `note nested\n\n|\"\n fn main() {}\n"
        );
        let parsed = parse(&formatted);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let [Block::Parsed(owner)] = parsed.syntax.blocks.as_slice() else {
            panic!("expected parsed raw owner");
        };
        assert_eq!(owner.children.len(), 2);
        assert!(owner.raw.is_some());
    }

    #[test]
    fn owned_childless_marked_raw_tail_is_adjacent_to_its_head() {
        let block = OwnedBlock::Parsed {
            marker: Some("rust".into()),
            head: Vec::new(),
            children: Vec::new(),
            raw: Some("fn main() {}\n".into()),
        };
        assert_eq!(block.format().unwrap(), "`rust\n|\"\n fn main() {}\n");
    }

    #[test]
    fn adding_attributes_explicitizes_anonymous_raw() {
        let block = OwnedBlock::Verbatim { text: "raw".into() }
            .with_attributes(vec![OwnedAttribute::id("example")]);
        assert_eq!(block.format().unwrap(), "`()\n\n `@ example\n\n|\"\n raw");
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
            "`task Work\n\n `@ work\n\n `= created | 2026-08-26T00:00:00+08:00\n"
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
        let source = "`task Closed\n\n `@ closed\n\n `= done|2026-07-20T09:00:00Z\n\n`task Existing\n\n `@ existing\n";
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
        assert!(edited.contains("`= created | 2026-07-20T10:00:00+08:00"));
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
            "`- First\n\n `+ task\n\n `@ first\n\n`- Second\n\n `+ event\n"
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
        assert_eq!(edited, "`- Work\r\n\r\n `+ task\r\n");
        assert!(parse(&edited).is_valid(), "{edited:?}");
    }

    #[test]
    fn formats_parsed_revisions_only_through_the_edit_boundary() {
        let source = "`meta\n   `= title\n\n      Unified command\n";
        let parsed = parse(source);
        let edits = format(&parsed, FormatScope::Document).unwrap();
        let formatted = apply_text_edits(source.to_string(), edits).unwrap();
        assert_eq!(formatted, "`meta\n `= title\n\n  Unified command\n");
        assert!(format(&parse(&formatted), FormatScope::Document)
            .unwrap()
            .is_empty());

        assert_eq!(
            format(&parse("`broken[\n"), FormatScope::Document),
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
            "`task Work\n `note before\n `@ old\n `+ keep\n `= created|now\n `note after\n";
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
        assert!(inserted.find("`@ old").unwrap() < inserted.find("`= due | tomorrow").unwrap());
        assert!(inserted.find("`= due | tomorrow").unwrap() < inserted.find("`+ keep").unwrap());
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
        assert!(!removed.contains("`= created|now"));
        assert!(removed.contains("`+ keep"));
    }

    #[test]
    fn owned_syntax_round_trips_inline_members_children_and_raw() {
        let source = "`node Head `span[text|@[id]|+[opaque]|=[key|bare]] and `\"raw\"\n `@ owner\n `child Body\n\n|\"\n payload\n";
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
        assert!(formatted.contains("`span[text|@[id]|+[opaque]|=[key|bare]]"));
        assert!(formatted.contains("`@ owner"));
        assert!(formatted.contains("`child Body"));
        assert!(formatted.contains("\n|\"\n payload"));
    }

    #[test]
    fn owned_elements_insert_an_empty_first_argument_before_other_member_forms() {
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
            assert!(output.starts_with("`owner[|"), "{output}");
            assert!(parse(format!("{output}\n")).is_valid(), "{output}");
        }
    }

    #[test]
    fn aligns_all_argument_columns_by_unicode_display_width() {
        let source = "`row 名|一|x\n`row alphabet | 二二 |yy\n";
        let parsed = parse(source);
        let edits = align_block_arguments(&parsed, source.find('名').unwrap()).unwrap();
        let aligned = apply_text_edits(source.to_string(), edits).unwrap();
        assert_eq!(
            aligned,
            "`row 名       | 一   | x\n`row alphabet | 二二 | yy\n"
        );
        assert!(
            align_block_arguments(&parse(&aligned), aligned.find('名').unwrap())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn alignment_uses_combining_width_and_stays_within_the_maximal_sibling_run() {
        let source =
            "`outer\n `row e\u{301}|x\n `row 界|yy\n `other break|run\n `row a|z\n `row aa|zz\n";
        let parsed = parse(source);
        let edits = align_block_arguments(&parsed, source.find("e\u{301}").unwrap()).unwrap();
        let aligned = apply_text_edits(source.to_string(), edits).unwrap();
        assert!(aligned.contains(" `row e\u{301}  | x\n `row 界 | yy\n"));
        assert!(aligned.contains(" `row a|z\n `row aa|zz\n"));
    }

    #[test]
    fn alignment_is_unavailable_for_ineligible_or_already_aligned_runs() {
        for source in [
            "`row a  | b\n`row aa | b\n",
            "`row a\t|b\n`row aa|b\n",
            "`row a|b\n`row aa|b\n `child detail\n",
            "`row a|b\n`row aa|b\n\n|\"\n payload\n",
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
            "`row |x||z\n`row long|yy|q|z\n`other break|run\n`row ` a` |x\n`row longer|y\n";
        let parsed = parse(source);
        let before = parsed.syntax.blocks[..2]
            .iter()
            .map(|block| match block {
                Block::Parsed(block) => (0..block.head.arguments.len())
                    .map(|index| block.head.argument_plain_text(index).unwrap())
                    .collect::<Vec<_>>(),
                Block::Verbatim(_) => unreachable!(),
            })
            .collect::<Vec<_>>();
        let edits = align_block_arguments(&parsed, source.find("|x||").unwrap()).unwrap();
        let aligned = apply_text_edits(source.to_string(), edits).unwrap();
        let reparsed = parse(&aligned);
        let after = reparsed.syntax.blocks[..2]
            .iter()
            .map(|block| match block {
                Block::Parsed(block) => (0..block.head.arguments.len())
                    .map(|index| block.head.argument_plain_text(index).unwrap())
                    .collect::<Vec<_>>(),
                Block::Verbatim(_) => unreachable!(),
            })
            .collect::<Vec<_>>();
        assert_eq!(after, before);

        let escaped = aligned.find("` a` ").unwrap();
        let escaped_edits = align_block_arguments(&reparsed, escaped).unwrap();
        let escaped_aligned = apply_text_edits(aligned, escaped_edits).unwrap();
        assert!(
            escaped_aligned.contains("`row ` a`   | x\n"),
            "{escaped_aligned:?}"
        );
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
            "`= title   | Example\n`= created | 2026-08-26T00:00:00+08:00\n"
        );
    }

    #[test]
    fn structured_arguments_share_one_padding_renderer() {
        assert_eq!(
            render_authored_text_arguments(&["09:00", "Event"]),
            "09:00 | Event"
        );
        assert_eq!(
            render_authored_text_arguments(&["key|part", "value[part]"]),
            "key`|part | value`[part`]"
        );

        let mut event = OwnedBlock::marked("-", "old");
        event.set_head_text_arguments(["09:00", "Event"]);
        assert_eq!(event.format().unwrap(), "`- 09:00 | Event\n");
    }

    #[test]
    fn prepending_a_structured_argument_preserves_rich_title_content() {
        let source = "`- title `*[rich]\n";
        let parsed = parse(source);
        let mut owned = OwnedBlock::from_syntax(source, &parsed.syntax.blocks[0]);
        owned.prepend_head_text_argument("09:00");
        let formatted = owned.format().unwrap();
        assert_eq!(formatted, "`- 09:00 | title `*[rich]\n");
        assert!(parse(&formatted).is_valid(), "{formatted}");
    }

    #[test]
    fn property_mutations_align_only_the_affected_direct_runs() {
        let source =
            "`- Task\n `+ task\n `@ task-id\n `= due|tomorrow\n `= priority|20\n `note Keep\n";
        let parsed = parse(source);
        let mut owned = OwnedBlock::from_syntax(source, &parsed.syntax.blocks[0]);
        owned.push_attribute(OwnedAttribute::quoted("created", "now"));
        let formatted = owned.format().unwrap();
        assert!(
            formatted.contains(" `= due      | tomorrow\n `= priority | 20\n `= created  | now\n"),
            "{formatted:?}"
        );
        assert!(formatted.contains(" `note Keep\n"));

        owned.retain_attributes(
            |attribute| !matches!(attribute, OwnedAttribute::Pair { key, .. } if key == "priority"),
        );
        let removed = owned.format().unwrap();
        assert!(
            removed.contains(" `= due     | tomorrow\n `= created | now\n"),
            "{removed:?}"
        );
    }

    #[test]
    fn non_property_mutations_do_not_align_existing_runs() {
        let source = "`- Task\n `= due|tomorrow\n `= priority|20\n";
        let parsed = parse(source);
        let mut owned = OwnedBlock::from_syntax(source, &parsed.syntax.blocks[0]);
        owned.prepend_attribute(OwnedAttribute::id("task-id"));
        assert!(owned
            .format()
            .unwrap()
            .contains(" `= due|tomorrow\n `= priority|20\n"));

        let attributes = owned.attributes();
        let unaligned = owned.clone().with_attributes(attributes.clone());
        assert!(unaligned
            .format()
            .unwrap()
            .contains(" `= due | tomorrow\n `= priority | 20\n"));
        let aligned = owned.with_aligned_attributes(attributes);
        assert!(aligned
            .format()
            .unwrap()
            .contains(" `= due      | tomorrow\n `= priority | 20\n"));
    }

    #[test]
    fn property_removal_does_not_align_a_separate_opaque_run() {
        let source = "`- Event\n `= date|2026-08-30\n `= timezone|+08:00\n `@ split\n `= uid|opaque\n `= when|legacy\n";
        let parsed = parse(source);
        let mut owned = OwnedBlock::from_syntax(source, &parsed.syntax.blocks[0]);
        owned.retain_attributes(
            |attribute| !matches!(attribute, OwnedAttribute::Pair { key, .. } if key == "date"),
        );
        let formatted = owned.format().unwrap();
        assert!(formatted.contains(" `= timezone|+08:00\n"), "{formatted:?}");
        assert!(
            formatted.contains(" `= uid|opaque\n `= when|legacy\n"),
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
            "`= title|Example\n`= created|2026-08-26T00:00:00+08:00\n\n"
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
