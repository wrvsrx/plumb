use std::ops::Range;

use plumb_syntax::{
    AttachedContent, AttrItem, Attributes, Block, Inline, InlineMember, ParsedBlock,
    ParsedDocument,
};

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
pub enum OwnedBlock {
    Document {
        attributes: OwnedAttributes,
    },
    Parsed {
        marker: Option<String>,
        attributes: OwnedAttributes,
        head: Vec<OwnedInline>,
        children: Vec<OwnedBlock>,
    },
    Verbatim {
        kind: String,
        attributes: OwnedAttributes,
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedInline {
    Text(String),
    Space(String),
    SoftBreak,
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
pub struct OwnedAttributes {
    pub present: bool,
    pub items: Vec<OwnedAttribute>,
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

    fn render(&self) -> String {
        match self {
            Self::Id(value) => format!("#{value}"),
            Self::Class(value) => format!(".{value}"),
            Self::Pair { key, value } => match value {
                OwnedValue::Bare(value) => format!("{key}={value}"),
                OwnedValue::Quoted(value) => {
                    let value = value.replace('\\', "\\\\").replace('"', "\\\"");
                    format!("{key}=\"{value}\"")
                }
            },
        }
    }

    fn render_attached(&self, inline: bool) -> String {
        let escape = |value: &str| {
            let value = value.replace('`', "``");
            if inline {
                value
                    .replace(']', "`]")
                    .replace('{', "`{")
                    .replace('}', "`}")
            } else {
                value
            }
        };
        match self {
            Self::Id(value) if inline => format!("`@[{}]", escape(value)),
            Self::Id(value) => format!("`@ {}", escape(value)),
            Self::Class(value) if inline => format!("`+[{}]", escape(value)),
            Self::Class(value) => format!("`+ {}", escape(value)),
            Self::Pair { key, value } => {
                let value = match value {
                    OwnedValue::Bare(value) | OwnedValue::Quoted(value) => value,
                };
                if inline {
                    format!("`=[{}|{}]", escape(key), escape(value))
                } else {
                    format!("`= {} {}", escape(key), escape(value))
                }
            }
        }
    }
}

impl OwnedBlock {
    pub fn document(attributes: Vec<OwnedAttribute>) -> Self {
        Self::Document {
            attributes: OwnedAttributes {
                present: true,
                items: attributes,
            },
        }
    }

    pub fn marked(marker: impl Into<String>, head: impl Into<String>) -> Self {
        Self::Parsed {
            marker: Some(marker.into()),
            attributes: OwnedAttributes::default(),
            head: vec![OwnedInline::Text(head.into())],
            children: Vec::new(),
        }
    }

    pub fn paragraph(text: impl Into<String>) -> Self {
        Self::Parsed {
            marker: None,
            attributes: OwnedAttributes::default(),
            head: vec![OwnedInline::Text(text.into())],
            children: Vec::new(),
        }
    }

    pub fn with_attributes(mut self, attributes: Vec<OwnedAttribute>) -> Self {
        match &mut self {
            Self::Document {
                attributes: current,
            }
            | Self::Parsed {
                attributes: current,
                ..
            }
            | Self::Verbatim {
                attributes: current,
                ..
            } => {
                current.present = true;
                current.items = attributes;
            }
        }
        self
    }

    pub fn with_children(mut self, children: Vec<OwnedBlock>) -> Self {
        match &mut self {
            Self::Parsed {
                children: current, ..
            } => *current = children,
            Self::Document { .. } | Self::Verbatim { .. } => {
                debug_assert!(children.is_empty())
            }
        }
        self
    }

    pub fn attributes(&self) -> &[OwnedAttribute] {
        match self {
            Self::Document { attributes }
            | Self::Parsed { attributes, .. }
            | Self::Verbatim { attributes, .. } => &attributes.items,
        }
    }

    pub fn attributes_mut(&mut self) -> &mut Vec<OwnedAttribute> {
        match self {
            Self::Document { attributes }
            | Self::Parsed { attributes, .. }
            | Self::Verbatim { attributes, .. } => {
                attributes.present = true;
                &mut attributes.items
            }
        }
    }

    pub fn set_head_text(&mut self, text: impl Into<String>) {
        if let Self::Parsed { head, .. } = self {
            *head = vec![OwnedInline::Text(text.into())];
        }
    }

    pub fn set_marker(&mut self, value: impl Into<String>) {
        if let Self::Parsed { marker, .. } = self {
            *marker = Some(value.into());
        }
    }

    pub fn children_mut(&mut self) -> Option<&mut Vec<OwnedBlock>> {
        match self {
            Self::Parsed { children, .. } => Some(children),
            Self::Document { .. } | Self::Verbatim { .. } => None,
        }
    }

    pub fn from_syntax(source: &str, block: &Block) -> Self {
        match block {
            Block::Parsed(block) => Self::from_parsed(source, block),
            Block::Verbatim(block) => Self::Verbatim {
                kind: block.kind.clone(),
                attributes: owned_attributes(&block.attrs),
                text: block.text.clone(),
            },
        }
    }

    pub fn from_parsed(source: &str, block: &ParsedBlock) -> Self {
        Self::Parsed {
            marker: block.mark.as_ref().map(|mark| mark.marker.clone()),
            attributes: block
                .mark
                .as_ref()
                .map_or_else(OwnedAttributes::default, |mark| {
                    owned_attributes(&mark.attrs)
                }),
            head: block
                .head
                .items
                .iter()
                .map(OwnedInline::from_syntax)
                .collect(),
            children: block
                .children
                .iter()
                .map(|child| Self::from_syntax(source, child))
                .collect(),
        }
    }

    pub fn format(&self) -> Result<String, EditError> {
        format_owned_blocks(std::slice::from_ref(self), "\n")
    }
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

pub fn remove_block(parsed: &ParsedDocument, range: Range<usize>) -> Result<TextEdit, EditError> {
    if !has_block_range(&parsed.syntax.blocks, &range) {
        return Err(EditError::InvalidRange);
    }
    let mut edit = EditSession::new(parsed, range.clone())?;
    edit.remove_block(range)?;
    edit.finish()
}

pub fn rewrite_legacy_link(
    parsed: &ParsedDocument,
    link_range: Range<usize>,
    property_range: Range<usize>,
    value_range: Range<usize>,
) -> Result<TextEdit, EditError> {
    validate_range(&parsed.source, &link_range)?;
    validate_range(&parsed.source, &property_range)?;
    validate_range(&parsed.source, &value_range)?;
    if parsed.valid_syntax().is_none()
        || property_range.start < link_range.start
        || property_range.end > link_range.end
        || value_range.start < property_range.start
        || value_range.end > property_range.end
    {
        return Err(EditError::InvalidRange);
    }
    let _ = find_inline(parsed, &link_range).ok_or(EditError::InvalidRange)?;
    let _ = (property_range, value_range);
    // Legacy Link source is parsed and rewritten by the versioned document
    // migrator before it enters the current editing pipeline.
    Err(EditError::InvalidRange)
}

fn find_inline<'a>(parsed: &'a ParsedDocument, target: &Range<usize>) -> Option<&'a Inline> {
    let mut blocks = parsed.syntax.blocks.iter().collect::<Vec<_>>();
    let mut contents = Vec::new();
    push_attached_content(&parsed.syntax.attrs, &mut blocks, &mut contents);
    while let Some(block) = blocks.pop() {
        match block {
            Block::Parsed(block) => {
                contents.push(&block.head);
                blocks.extend(&block.children);
                if let Some(mark) = &block.mark {
                    push_attached_content(&mark.attrs, &mut blocks, &mut contents);
                }
            }
            Block::Verbatim(block) => {
                push_attached_content(&block.attrs, &mut blocks, &mut contents);
            }
        }
    }
    while let Some(content) = contents.pop() {
        for inline in &content.items {
            match inline {
                Inline::Element {
                    range,
                    members,
                    attrs,
                    ..
                } => {
                    if range == target {
                        return Some(inline);
                    }
                    for member in members {
                        match member {
                            InlineMember::ParsedArgument(argument) => {
                                contents.push(&argument.content);
                            }
                            InlineMember::Child { inline, .. } if inline_range(inline) == target => {
                                return Some(inline);
                            }
                            InlineMember::Child { .. }
                            | InlineMember::VerbatimArgument(_) => {}
                        }
                    }
                    push_inline_attached_content(attrs, &mut contents);
                }
                Inline::Verbatim { attrs, .. } => {
                    push_inline_attached_content(attrs, &mut contents);
                }
                Inline::Text { .. } | Inline::Space { .. } | Inline::SoftBreak { .. } => {}
            }
        }
    }
    None
}

fn push_attached_content<'a>(
    attrs: &'a Attributes,
    blocks: &mut Vec<&'a Block>,
    contents: &mut Vec<&'a plumb_syntax::InlineContent>,
) {
    let Some(attached) = attrs.attached.as_deref() else {
        return;
    };
    match &attached.content {
        AttachedContent::Blocks(attached_blocks) => blocks.extend(attached_blocks),
        AttachedContent::Inlines(content) => contents.push(content),
    }
}

fn push_inline_attached_content<'a>(
    attrs: &'a Attributes,
    contents: &mut Vec<&'a plumb_syntax::InlineContent>,
) {
    if let Some(AttachedContent::Inlines(content)) =
        attrs.attached.as_deref().map(|attached| &attached.content)
    {
        contents.push(content);
    }
}

fn inline_range(inline: &Inline) -> &Range<usize> {
    match inline {
        Inline::Text { range, .. }
        | Inline::Space { range, .. }
        | Inline::SoftBreak { range }
        | Inline::Element { range, .. }
        | Inline::Verbatim { range, .. } => range,
    }
}

impl OwnedInline {
    fn from_syntax(inline: &Inline) -> Self {
        match inline {
            Inline::Text { text, .. } => Self::Text(text.clone()),
            Inline::Space { text, .. } => Self::Space(text.clone()),
            Inline::SoftBreak { .. } => Self::SoftBreak,
            Inline::Element {
                kind, members, ..
            } => Self::Element {
                kind: kind.clone(),
                members: members
                    .iter()
                    .map(|member| match member {
                        InlineMember::ParsedArgument(argument) => {
                            OwnedInlineMember::ParsedArgument(
                                argument.content.items.iter().map(Self::from_syntax).collect(),
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
        let mut items = owned_attribute_items(attributes);
        for (position, item) in additions {
            let index = insertion_index(position, items.len())?;
            items.insert(index, item);
        }
        self.replace_attribute_slot(attributes, owner_insert, items)
    }

    pub fn replace_attribute(
        &mut self,
        attributes: &Attributes,
        index: usize,
        item: OwnedAttribute,
    ) -> Result<(), EditError> {
        let mut items = owned_attribute_items(attributes);
        let target = items
            .get_mut(index)
            .ok_or(EditError::InvalidAttributePosition)?;
        *target = item;
        self.replace_attribute_slot(attributes, 0, items)
    }

    pub fn remove_attribute(
        &mut self,
        attributes: &Attributes,
        index: usize,
    ) -> Result<(), EditError> {
        let mut items = owned_attribute_items(attributes);
        if index >= items.len() {
            return Err(EditError::InvalidAttributePosition);
        }
        items.remove(index);
        self.replace_attribute_slot(attributes, 0, items)
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
        let newline = line_ending(&self.parsed.source);
        self.replace(
            range,
            format_owned_blocks(std::slice::from_ref(block), newline)?,
        )
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

    fn replace_attribute_slot(
        &mut self,
        attributes: &Attributes,
        owner_insert: usize,
        items: Vec<OwnedAttribute>,
    ) -> Result<(), EditError> {
        let (range, new_text) = if let Some(range) = &attributes.range {
            let mut rendered = if let Some(attached) = attributes.attached.as_deref() {
                render_attached_attribute_slot(
                    &items,
                    &attached.content,
                    line_indent(&self.parsed.source, attached.open_range.start),
                    line_ending(&self.parsed.source),
                )
            } else {
                render_attribute_slot(&items.iter().map(OwnedAttribute::render).collect::<Vec<_>>())
            };
            let newline = line_ending(&self.parsed.source);
            if self.parsed.source[range.clone()].ends_with(newline) && !rendered.ends_with(newline)
            {
                rendered.push_str(newline);
            }
            (range.clone(), rendered)
        } else {
            if owner_insert > self.parsed.source.len()
                || !self.parsed.source.is_char_boundary(owner_insert)
            {
                return Err(EditError::InvalidRange);
            }
            let line_end = self.parsed.source[owner_insert..]
                .find('\n')
                .map_or(self.parsed.source.len(), |relative| owner_insert + relative);
            let newline = line_ending(&self.parsed.source);
            if self.parsed.source[..owner_insert].ends_with('"') {
                let group = render_attached_attribute_slot(
                    &items,
                    &AttachedContent::Inlines(plumb_syntax::InlineContent {
                        range: 0..0,
                        items: Vec::new(),
                    }),
                    0,
                    newline,
                );
                return self.replace(owner_insert..owner_insert, format!(" {group}"));
            }
            let line_start = self.parsed.source[..owner_insert]
                .rfind('\n')
                .map_or(0, |index| index + 1);
            let group_indent = self.parsed.source[line_start..owner_insert]
                .bytes()
                .take_while(|byte| *byte == b' ')
                .count();
            let group = render_attached_attribute_slot(
                &items,
                &AttachedContent::Blocks(Vec::new()),
                group_indent,
                newline,
            );
            (line_end..line_end, format!(" {group}"))
        };
        self.replace(range, new_text)
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

fn owned_attribute_items(attributes: &Attributes) -> Vec<OwnedAttribute> {
    attributes
        .items
        .iter()
        .map(|item| match item {
            AttrItem::Id { value, .. } => OwnedAttribute::Id(value.clone()),
            AttrItem::Class { value, .. } => OwnedAttribute::Class(value.clone()),
            AttrItem::Pair { key, value, .. } => OwnedAttribute::Pair {
                key: key.clone(),
                value: if value.quoted {
                    OwnedValue::Quoted(value.decoded.clone())
                } else {
                    OwnedValue::Bare(value.decoded.clone())
                },
            },
        })
        .collect()
}

fn owned_attributes(attributes: &Attributes) -> OwnedAttributes {
    OwnedAttributes {
        present: attributes.range.is_some(),
        items: attributes
            .items
            .iter()
            .map(|item| match item {
                AttrItem::Id { value, .. } => OwnedAttribute::Id(value.clone()),
                AttrItem::Class { value, .. } => OwnedAttribute::Class(value.clone()),
                AttrItem::Pair { key, value, .. } => OwnedAttribute::Pair {
                    key: key.clone(),
                    value: if value.quoted {
                        OwnedValue::Quoted(value.decoded.clone())
                    } else {
                        OwnedValue::Bare(value.decoded.clone())
                    },
                },
            })
            .collect(),
    }
}

fn render_attribute_slot(items: &[String]) -> String {
    format!("{{{}}}", items.join(" "))
}

fn render_attached_attribute_slot(
    items: &[OwnedAttribute],
    content: &AttachedContent,
    indent: usize,
    newline: &str,
) -> String {
    match content {
        AttachedContent::Inlines(_) => format!(
            "{{{}}}",
            items
                .iter()
                .map(|item| item.render_attached(true))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        AttachedContent::Blocks(_) => {
            let item_indent = " ".repeat(indent + 1);
            let close_indent = " ".repeat(indent);
            if items.is_empty() {
                format!("{{{newline}{close_indent}}}")
            } else {
                format!(
                    "{{{newline}{item_indent}{}{newline}{close_indent}}}",
                    items
                        .iter()
                        .map(|item| item.render_attached(false))
                        .collect::<Vec<_>>()
                        .join(&format!("{newline}{item_indent}"))
                )
            }
        }
    }
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
        OwnedBlock::Document { attributes } => {
            output.push_str(&render_attached_attribute_slot(
                &attributes.items,
                &AttachedContent::Blocks(Vec::new()),
                indent,
                "\n",
            ));
        }
        OwnedBlock::Parsed {
            marker,
            attributes,
            head,
            children,
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
            if attributes.present {
                output.push(' ');
                output.push_str(&render_attached_attribute_slot(
                    &attributes.items,
                    &AttachedContent::Blocks(Vec::new()),
                    indent,
                    "\n",
                ));
            }
            if !children.is_empty() {
                if head.is_empty() && !attributes.present {
                    output.push('\n');
                } else {
                    output.push_str("\n\n");
                }
                render_owned_blocks(children, indent + 1, output);
            }
        }
        OwnedBlock::Verbatim {
            kind,
            attributes,
            text,
        } => {
            output.push('`');
            output.push_str(kind);
            output.push('"');
            if attributes.present {
                output.push(' ');
            }
            render_owned_attached(attributes, output);
            if !text.is_empty() {
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
        }
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
    nested: bool,
    continuation_indent: usize,
    output: &mut String,
    introduced: bool,
) {
        match inline {
            OwnedInline::Text(text) => {
                for character in text.chars() {
                    match character {
                        '`' => output.push_str("``"),
                        '[' | ']' | '|' if nested => {
                            output.push('`');
                            output.push(character);
                        }
                        _ => output.push(character),
                    }
                }
            }
            OwnedInline::Space(space) => output.push_str(space),
            OwnedInline::SoftBreak => {
                output.push('\n');
                output.extend(std::iter::repeat_n(' ', continuation_indent));
            }
            OwnedInline::Element {
                kind,
                members,
            } => {
                if introduced {
                    output.push('`');
                }
                output.push_str(kind);
                output.push('[');
                for (index, member) in members.iter().enumerate() {
                    if index > 0 {
                        output.push('|');
                    }
                    match member {
                        OwnedInlineMember::ParsedArgument(argument) => {
                            render_owned_inlines(argument, true, continuation_indent, output);
                        }
                        OwnedInlineMember::VerbatimArgument(argument) => {
                            render_owned_verbatim_payload(argument, output);
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
                }
                output.push_str(kind);
                render_owned_verbatim_payload(text, output);
            }
        }
}

fn render_owned_verbatim_payload(text: &str, output: &mut String) {
    if !text.contains('"') && !text.starts_with('[') {
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

fn render_owned_attached(attributes: &OwnedAttributes, output: &mut String) {
    if !attributes.present {
        return;
    }
    output.push('{');
    for (index, attribute) in attributes.items.iter().enumerate() {
        if index > 0 {
            output.push(' ');
        }
        output.push_str(&attribute.render_attached(true));
    }
    output.push('}');
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
    if affected.start == 0 && modified_parsed.syntax.attrs.attached.is_some() {
        let mut formatted = plumb_format::format_parsed(&modified_parsed)
            .map_err(|_| EditError::GeneratedInvalid)?;
        if line_ending(source) == "\r\n" {
            formatted = formatted.replace('\n', "\r\n");
        }
        if let Some(prefix) = formatted.strip_suffix(source.as_str()) {
            return Ok(TextEdit {
                range: 0..0,
                new_text: prefix.to_string(),
            });
        }
        return Ok(TextEdit {
            range: 0..source.len(),
            new_text: formatted,
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
    fn formats_a_parsed_revision_through_the_edit_boundary() {
        let source = "`meta\n   `: title\n\n      Unified command\n";
        let parsed = parse(source);
        let edits = format(&parsed, FormatScope::Document).unwrap();
        let formatted = apply_text_edits(source.to_string(), edits).unwrap();
        assert_eq!(formatted, "`meta\n `: title\n\n  Unified command\n");
        assert!(format(&parse(&formatted), FormatScope::Document)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn formats_only_complete_blocks_contained_by_a_selection() {
        let source = "`first One\n\n      Child\n\n`second Two\n";
        let parsed = parse(source);
        let first = parsed.syntax.blocks[0].range().clone();
        let edits = format(&parsed, FormatScope::ContainedBlocks(first.clone())).unwrap();
        let formatted = apply_text_edits(source.to_string(), edits).unwrap();
        assert_eq!(formatted, "`first One\n\n Child\n\n`second Two\n");
        assert!(format(
            &parsed,
            FormatScope::ContainedBlocks(first.start..first.start)
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn formatting_rejects_invalid_revisions_and_ranges() {
        assert_eq!(
            format(&parse("`broken[\n"), FormatScope::Document),
            Err(EditError::GeneratedInvalid)
        );
        let parsed = parse("Paragraph.\n");
        assert_eq!(
            format(
                &parsed,
                FormatScope::ContainedBlocks(0..parsed.source.len() + 1)
            ),
            Err(EditError::InvalidRange)
        );
    }

    #[test]
    fn owned_replacement_preserves_nested_crlf_layout() {
        let source = "`outer Parent\r\n\r\n   `old Child\r\n\r\n`next Keep\r\n";
        let parsed = parse(source);
        let Block::Parsed(outer) = &parsed.syntax.blocks[0] else {
            unreachable!();
        };
        let nested = outer.children[0].range().clone();
        let edit = replace_owned_block(&parsed, nested, &OwnedBlock::marked("new", "Replacement"))
            .unwrap();
        assert_eq!(edit.new_text, "`new Replacement\r\n\r\n");
        let edited = apply_text_edits(source.to_string(), vec![edit]).unwrap();
        assert!(edited.contains("   `new Replacement\r\n\r\n`next Keep"));
    }

    #[test]
    fn structural_removal_rejects_non_block_ranges() {
        let parsed = parse("`item Keep\n");
        assert_eq!(remove_block(&parsed, 0..5), Err(EditError::InvalidRange));
    }

    fn first_mark(source: &str) -> (ParsedDocument, Range<usize>, usize) {
        let parsed = parse(source);
        let Block::Parsed(block) = &parsed.syntax.blocks[0] else {
            panic!("expected parsed block");
        };
        let mark = block.mark.as_ref().unwrap();
        (parsed.clone(), block.range.clone(), mark.marker_range.end)
    }

    #[test]
    fn inserts_attributes_at_explicit_positions() {
        let source = "`task Work {\n  `@ id\n  `= created now\n}\n";
        let parsed = parse(source);
        let Block::Parsed(block) = &parsed.syntax.blocks[0] else {
            panic!("expected parsed block");
        };
        let mark = block.mark.as_ref().unwrap();
        let mut edit = EditSession::new(&parsed, block.range.clone()).unwrap();
        edit.insert_attribute(
            &mark.attrs,
            mark.marker_range.end,
            AttributePosition::First,
            OwnedAttribute::class("next"),
        )
        .unwrap();
        let edit = edit.finish().unwrap();
        assert_eq!(
            edit.new_text,
            "`task Work {\n `+ next\n\n `@ id\n\n `= created now\n}\n"
        );
    }

    #[test]
    fn creates_an_attribute_slot_and_quotes_values() {
        let source = "`- Work\n";
        let (parsed, range, insert) = first_mark(source);
        let Block::Parsed(block) = &parsed.syntax.blocks[0] else {
            unreachable!();
        };
        let mark = block.mark.as_ref().unwrap();
        let mut edit = EditSession::new(&parsed, range).unwrap();
        edit.insert_attribute(
            &mark.attrs,
            insert,
            AttributePosition::First,
            OwnedAttribute::quoted("created", "2026-07-23T03:00:00+08:00"),
        )
        .unwrap();
        let edit = edit.finish().unwrap();
        assert_eq!(
            edit.new_text,
            "`- Work {\n `= created 2026-07-23T03:00:00+08:00\n}\n"
        );
    }

    #[test]
    fn creates_an_attribute_slot_for_a_nested_marked_block() {
        let source = "`- Outer\n\n   `- Nested\n";
        let parsed = parse(source);
        let Block::Parsed(outer) = &parsed.syntax.blocks[0] else {
            unreachable!();
        };
        let Block::Parsed(nested) = &outer.children[0] else {
            unreachable!();
        };
        let mark = nested.mark.as_ref().unwrap();
        let mut edit = EditSession::new(&parsed, nested.range.clone()).unwrap();
        edit.insert_attributes(
            &mark.attrs,
            mark.marker_range.end,
            [
                (AttributePosition::Last, OwnedAttribute::class("kind")),
                (
                    AttributePosition::Last,
                    OwnedAttribute::bare("created", "2026-07-20T10:00:00+08:00"),
                ),
            ],
        )
        .unwrap();
        let edit = edit.finish().unwrap();
        assert_eq!(
            edit.new_text,
            "   `- Nested {\n    `+ kind\n\n    `= created 2026-07-20T10:00:00+08:00\n   }\n"
        );
    }

    #[test]
    fn creates_a_nested_slot_before_a_top_level_sibling() {
        let source = "`- Outer {\n  `@ outer\n  `+ keep\n}\n\n   `- Nested\n\n`task Closed {\n  `@ closed\n  `= done 2026-07-20T09:00:00Z\n}\n";
        let parsed = parse(source);
        let Block::Parsed(outer) = &parsed.syntax.blocks[0] else {
            unreachable!();
        };
        let Block::Parsed(nested) = &outer.children[0] else {
            unreachable!();
        };
        let mark = nested.mark.as_ref().unwrap();
        let mut edit = EditSession::new(&parsed, nested.range.clone()).unwrap();
        edit.insert_attributes(
            &mark.attrs,
            mark.marker_range.end,
            [
                (AttributePosition::Last, OwnedAttribute::class("kind")),
                (
                    AttributePosition::Last,
                    OwnedAttribute::bare("created", "2026-07-20T10:00:00+08:00"),
                ),
            ],
        )
        .unwrap();
        let edit = edit.finish().unwrap();
        let edited = apply_text_edits(source.to_string(), vec![edit]).unwrap();
        assert!(parse(&edited).is_valid(), "{edited}");
        assert!(edited.contains("`- Nested {\n    `+ kind"), "{edited}");
    }

    #[test]
    fn inserts_an_id_first_in_an_existing_attached_group() {
        let source = "`# Hello, World! {\n  `+ keep\n}\n";
        let parsed = parse(source);
        let Block::Parsed(block) = &parsed.syntax.blocks[0] else {
            unreachable!();
        };
        let mark = block.mark.as_ref().unwrap();
        let mut edit = EditSession::new(&parsed, block.range.clone()).unwrap();
        edit.insert_attribute(
            &mark.attrs,
            mark.marker_range.end,
            AttributePosition::First,
            OwnedAttribute::id("hello-world"),
        )
        .unwrap();
        let edit = edit.finish().unwrap();
        assert_eq!(
            edit.new_text,
            "`# Hello, World! {\n `@ hello-world\n\n `+ keep\n}\n"
        );
    }

    #[test]
    fn inserts_an_id_before_unrelated_following_blocks() {
        let source = "`# Hello, World! {\n  `- keep\n}\n\n`node Outer\n\n      `child Nested title\n\n`text\"\n  raw\n`note Multiline attrs {\n  `- keep\n}\n\n`other Existing {\n  `@ hello-world\n}\n\n`# Hello, World!\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Parsed(block) = &parsed.syntax.blocks[0] else {
            unreachable!();
        };
        let mark = block.mark.as_ref().unwrap();
        let mut edit = EditSession::new(&parsed, block.range.clone()).unwrap();
        edit.insert_attribute(
            &mark.attrs,
            mark.marker_range.end,
            AttributePosition::First,
            OwnedAttribute::id("hello-world-2"),
        )
        .unwrap();
        let mut logical = source.to_string();
        for pending in edit.edits.iter().rev() {
            logical.replace_range(pending.range.clone(), &pending.new_text);
        }
        let delta = edit.edits[0].new_text.len() as isize - edit.edits[0].range.len() as isize;
        let end = block.range.end.checked_add_signed(delta).unwrap();
        eprintln!(
            "block={:?} pending={:?} end={end} parsed={:?} format={:?}",
            block.range,
            edit.edits,
            parse(&logical).syntax.blocks[0].range(),
            plumb_format::format_block_range(&logical, block.range.start..end)
        );
        let edit = edit.finish().unwrap();
        assert!(edit.new_text.contains("`@ hello-world-2"), "{edit:?}");
    }

    #[test]
    fn creates_a_compact_group_for_a_verbatim_block() {
        let source = "`rust\"\n  fn main() {}\n";
        let parsed = parse(source);
        let Block::Verbatim(block) = &parsed.syntax.blocks[0] else {
            unreachable!();
        };
        let mut edit = EditSession::new(&parsed, block.range.clone()).unwrap();
        edit.insert_attribute(
            &block.attrs,
            block.opener_range.end,
            AttributePosition::First,
            OwnedAttribute::id("example"),
        )
        .unwrap();
        let edit = edit.finish().unwrap();
        assert_eq!(edit.new_text, "`rust\" {`@[example]}\n  fn main() {}\n");
    }

    #[test]
    fn inserts_a_status_between_compact_top_level_siblings() {
        let source = "`task Blocker {\n  `@ blocker\n}\n`task Blocked {\n  `@ blocked\n  `= depends #blocker\n}\n`task Closed {\n  `@ closed\n}\n";
        let parsed = parse(source);
        let Block::Parsed(block) = &parsed.syntax.blocks[1] else {
            unreachable!();
        };
        let mark = block.mark.as_ref().unwrap();
        let mut edit = EditSession::new(&parsed, block.range.clone()).unwrap();
        edit.insert_attribute(
            &mark.attrs,
            mark.marker_range.end,
            AttributePosition::Last,
            OwnedAttribute::bare("canceled", "2026-07-20T12:00:00Z"),
        )
        .unwrap();
        let edit = edit.finish().unwrap();
        let edited = apply_text_edits(source.to_string(), vec![edit]).unwrap();
        assert!(parse(&edited).is_valid(), "{edited}");
        assert!(edited.contains("`= canceled 2026-07-20T12:00:00Z"));
    }

    #[test]
    fn updates_and_clones_a_recurring_task_before_a_heading() {
        let source = "`task Repeat {\n  `@ repeat\n  `: due 2026-07-20T23:59:59+08:00\n  `: recur P1D\n}\n\n`# Following\n";
        let parsed = parse(source);
        let Block::Parsed(block) = &parsed.syntax.blocks[0] else {
            unreachable!();
        };
        let mark = block.mark.as_ref().unwrap();
        let mut next = OwnedBlock::from_parsed(source, block);
        let attributes = next.attributes_mut();
        attributes.retain(|attribute| matches!(attribute, OwnedAttribute::Class(_)));
        attributes.push(OwnedAttribute::id("repeat-next"));
        attributes.push(OwnedAttribute::bare("due", "2026-07-21T23:59:59+08:00"));
        attributes.push(OwnedAttribute::bare("recur", "P1D"));

        let mut edit = EditSession::new(&parsed, block.range.clone()).unwrap();
        edit.insert_attribute(
            &mark.attrs,
            mark.marker_range.end,
            AttributePosition::Last,
            OwnedAttribute::bare("done", "2026-07-21T18:01:12+08:00"),
        )
        .unwrap();
        edit.insert_sibling_blocks(&block.range, &[next]).unwrap();
        let edit = edit.finish().unwrap();
        let edited = apply_text_edits(source.to_string(), vec![edit]).unwrap();
        assert!(parse(&edited).is_valid(), "{edited}");
        assert!(edited.contains("`@ repeat-next"));
        assert!(edited.contains("`# Following"));
    }

    #[test]
    fn mutable_attributes_create_a_missing_slot() {
        let mut block = OwnedBlock::marked("-", "Work");
        block.attributes_mut().push(OwnedAttribute::class("event"));
        let mut output = String::new();
        render_owned_blocks(&[block], 0, &mut output);
        assert_eq!(output, "`- Work {\n `+ event\n}");
    }

    #[test]
    fn rejects_implicit_or_out_of_bounds_positions() {
        let source = "`task Work {\n}\n";
        let parsed = parse(source);
        let Block::Parsed(block) = &parsed.syntax.blocks[0] else {
            unreachable!();
        };
        let mark = block.mark.as_ref().unwrap();
        let mut edit = EditSession::new(&parsed, block.range.clone()).unwrap();
        assert_eq!(
            edit.insert_attribute(
                &mark.attrs,
                mark.marker_range.end,
                AttributePosition::After(1),
                OwnedAttribute::id("work"),
            ),
            Err(EditError::InvalidAttributePosition)
        );
    }

    #[test]
    fn rejects_overlapping_logical_edits() {
        let parsed = parse("`- Work\n");
        assert_eq!(
            finalize(
                &parsed,
                0..8,
                vec![
                    TextEdit {
                        range: 1..2,
                        new_text: "a".to_string(),
                    },
                    TextEdit {
                        range: 1..1,
                        new_text: "b".to_string(),
                    },
                ],
            ),
            Err(EditError::OverlappingEdits)
        );
        assert_eq!(
            finalize(
                &parsed,
                0..8,
                vec![TextEdit {
                    range: 9..9,
                    new_text: "outside".to_string(),
                }],
            ),
            Err(EditError::InvalidRange)
        );
    }

    #[test]
    fn inserts_owned_metadata_before_existing_blocks() {
        let parsed = parse("`# Existing\n");
        let metadata = OwnedBlock::document(vec![
            OwnedAttribute::bare("title", "Example"),
            OwnedAttribute::bare("created", "2026-07-23T03:00:00+08:00"),
        ]);
        let mut edit = EditSession::new(&parsed, 0..0).unwrap();
        edit.insert_blocks(0, &[metadata]).unwrap();
        let edit = edit.finish().unwrap();
        assert_eq!(edit.range, 0..0);
        assert_eq!(
            edit.new_text,
            "{\n `= title Example\n `= created 2026-07-23T03:00:00+08:00\n}\n\n"
        );
    }

    #[test]
    fn round_trips_owned_syntax_without_extension_knowledge() {
        let source = "`node Head `span[text|@[id]|+[opaque]|=[key|bare]] and `\"raw\" {\n  `@ id\n  `+ opaque\n  `= key bare\n}\n\n      `child Body\n";
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
        assert!(formatted.contains("`+ opaque"));
        assert!(formatted.contains("`span[text|@[id]|+[opaque]|=[key|bare]]"));
        assert!(formatted.contains("`\"raw\""));
        assert!(formatted.contains("`child Body"));
    }

    #[test]
    fn preserves_empty_attribute_slots_and_soft_breaks() {
        let source = "`node Head `span[first\n      second|] and `\"raw\"\n";
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
        assert!(formatted.contains("`node Head"));
        assert!(formatted.contains("`span[first\n"));
        assert!(formatted.contains("`span[first\n second|]"));
        assert!(formatted.contains("`\"raw\""));
    }

    #[test]
    fn preserves_verbatim_payload_when_detaching_a_block() {
        let source = "`text\" {`@[raw]}\n  first\n    second\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let owned = OwnedBlock::from_syntax(source, &parsed.syntax.blocks[0]);
        assert_eq!(
            owned.format().unwrap(),
            "`text\" {`@[raw]}\n  first\n    second\n"
        );
    }

    #[test]
    fn replaces_and_removes_attributes_by_explicit_index() {
        let source = "`node Head {\n  `@ old\n  `+ keep\n  `= key value\n}\n";
        let parsed = parse(source);
        let Block::Parsed(block) = &parsed.syntax.blocks[0] else {
            unreachable!();
        };
        let mark = block.mark.as_ref().unwrap();
        let mut replace = EditSession::new(&parsed, block.range.clone()).unwrap();
        replace
            .replace_attribute(&mark.attrs, 0, OwnedAttribute::id("new"))
            .unwrap();
        let replacement = replace.finish().unwrap();
        assert_eq!(
            replacement.new_text,
            "`node Head {\n `@ new\n\n `+ keep\n\n `= key value\n}\n"
        );

        let mut remove = EditSession::new(&parsed, block.range.clone()).unwrap();
        remove.remove_attribute(&mark.attrs, 2).unwrap();
        let removal = remove.finish().unwrap();
        assert_eq!(removal.new_text, "`node Head {\n `@ old\n\n `+ keep\n}\n");
    }

    #[test]
    fn edits_block_attached_elements_without_reintroducing_legacy_attributes() {
        let source = "`task Work {\n  `@ old\n}\n";
        let parsed = parse(source);
        let Block::Parsed(block) = &parsed.syntax.blocks[0] else {
            unreachable!();
        };
        let attrs = &block.mark.as_ref().unwrap().attrs;
        let mut edit = EditSession::new(&parsed, block.range.clone()).unwrap();
        edit.replace_attribute(attrs, 0, OwnedAttribute::id("new"))
            .unwrap();
        let edit = edit.finish().unwrap();
        assert!(edit.new_text.contains("`@ new"), "{}", edit.new_text);
        assert!(!edit.new_text.contains("#new"), "{}", edit.new_text);
        assert!(parse(&edit.new_text).is_valid());
    }

    #[test]
    fn edits_attached_elements_to_the_canonical_opener_placement() {
        // A single-line head canonicalizes to the trailing opener.
        let source = "`task Work\n      {\n        `@ old\n      }\n";
        let parsed = parse(source);
        let Block::Parsed(block) = &parsed.syntax.blocks[0] else {
            unreachable!();
        };
        let attrs = &block.mark.as_ref().unwrap().attrs;
        let mut edit = EditSession::new(&parsed, block.range.clone()).unwrap();
        edit.replace_attribute(attrs, 0, OwnedAttribute::id("new"))
            .unwrap();
        let edit = edit.finish().unwrap();
        assert_eq!(edit.new_text, "`task Work {\n `@ new\n}\n");
        assert!(parse(&edit.new_text).is_valid());

        // A wrapped head keeps the own-line opener.
        let source = "`task Work\n      spans lines\n      {\n        `@ old\n      }\n";
        let parsed = parse(source);
        let Block::Parsed(block) = &parsed.syntax.blocks[0] else {
            unreachable!();
        };
        let attrs = &block.mark.as_ref().unwrap().attrs;
        let mut edit = EditSession::new(&parsed, block.range.clone()).unwrap();
        edit.replace_attribute(attrs, 0, OwnedAttribute::id("new"))
            .unwrap();
        let edit = edit.finish().unwrap();
        assert_eq!(
            edit.new_text,
            "`task Work\n spans lines\n {\n  `@ new\n }\n"
        );
        assert!(parse(&edit.new_text).is_valid());
    }

    #[test]
    fn replaces_and_removes_complete_blocks() {
        let source = "`old Head\n`next Keep\n";
        let parsed = parse(source);
        let first = parsed.syntax.blocks[0].range().clone();
        let mut replace = EditSession::new(&parsed, first.clone()).unwrap();
        replace
            .replace_block(first.clone(), &OwnedBlock::marked("new", "Replacement"))
            .unwrap();
        let replacement = replace.finish().unwrap();
        assert_eq!(replacement.new_text, "`new Replacement\n\n");

        let mut remove = EditSession::new(&parsed, first.clone()).unwrap();
        remove.remove_block(first.clone()).unwrap();
        let removal = remove.finish().unwrap();
        assert_eq!(removal.range, first);
        assert!(removal.new_text.is_empty());
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
