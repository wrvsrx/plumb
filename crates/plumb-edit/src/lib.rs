use std::ops::Range;

use plumb_syntax::{
    AttrItem, Attributes, Block, Inline, InlineMember, ParsedBlock, ParsedDocument,
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
                    "`= {}|{}",
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
                OwnedBlock::association(key, value)
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
            children.retain(|child| owned_declaration(child).as_ref().is_none_or(&mut predicate));
        }
    }

    pub fn push_attribute(&mut self, attribute: OwnedAttribute) {
        if let Self::Parsed { children, .. } = self {
            let index = children
                .iter()
                .rposition(|child| owned_declaration(child).is_some())
                .map_or(0, |index| index + 1);
            children.insert(index, attribute.into_block());
        } else {
            panic!("anonymous raw blocks have no attributes");
        }
    }

    pub fn extend_attributes(&mut self, attributes: impl IntoIterator<Item = OwnedAttribute>) {
        for attribute in attributes {
            self.push_attribute(attribute);
        }
    }

    pub fn set_head_text(&mut self, text: impl Into<String>) {
        if let Self::Parsed { head, .. } = self {
            *head = owned_authored_text(&text.into());
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
    let plain = |items: &[OwnedInline]| {
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
    };
    match marker.as_str() {
        "@" => Some(OwnedAttribute::Id(plain(head)?)),
        "+" => Some(OwnedAttribute::Class(plain(head)?)),
        "=" => {
            let separator = head
                .iter()
                .position(|inline| matches!(inline, OwnedInline::ArgumentSeparator))?;
            let key = plain(&head[..separator])?;
            let value = plain(&head[separator + 1..])?;
            (!key.is_empty() && !value.is_empty()).then_some(OwnedAttribute::Pair {
                key,
                value: OwnedValue::Bare(value),
            })
        }
        _ => None,
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
        assert_eq!(attribute.render_block(), "`= key`|part|value`[part`]");

        let formatted = attribute.into_block().format().unwrap();
        assert_eq!(formatted, "`= key`|part|value`[part`]\n");
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
            "`task Work\n\n `@ work\n\n `= created|2026-08-26T00:00:00+08:00\n"
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
        assert!(edited.contains("`= created|2026-07-20T10:00:00+08:00"));
        assert!(edited.contains("`task Existing"));
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
        assert!(inserted.find("`@ old").unwrap() < inserted.find("`= due|tomorrow").unwrap());
        assert!(inserted.find("`= due|tomorrow").unwrap() < inserted.find("`+ keep").unwrap());
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
            assert!(parse(&format!("{output}\n")).is_valid(), "{output}");
        }
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
