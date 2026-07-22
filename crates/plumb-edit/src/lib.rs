use std::ops::Range;

use plumb_core::{AttrItem, Attributes, Block, Inline, ParsedDocument};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range<usize>,
    pub new_text: String,
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
        attributes: Vec<OwnedAttribute>,
        head: Vec<OwnedInline>,
        children: Vec<OwnedBlock>,
    },
    Verbatim {
        attributes: Vec<OwnedAttribute>,
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedInline {
    Text(String),
    SoftBreak,
    Element {
        kind: String,
        content: Vec<OwnedInline>,
        attributes: Vec<OwnedAttribute>,
    },
    Verbatim {
        text: String,
        attributes: Vec<OwnedAttribute>,
    },
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
}

impl OwnedBlock {
    pub fn marked(marker: impl Into<String>, head: impl Into<String>) -> Self {
        Self::Parsed {
            marker: Some(marker.into()),
            attributes: Vec::new(),
            head: vec![OwnedInline::Text(head.into())],
            children: Vec::new(),
        }
    }

    pub fn paragraph(text: impl Into<String>) -> Self {
        Self::Parsed {
            marker: None,
            attributes: Vec::new(),
            head: vec![OwnedInline::Text(text.into())],
            children: Vec::new(),
        }
    }

    pub fn with_attributes(mut self, attributes: Vec<OwnedAttribute>) -> Self {
        *self.attributes_mut() = attributes;
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

    pub fn attributes(&self) -> &[OwnedAttribute] {
        match self {
            Self::Parsed { attributes, .. } | Self::Verbatim { attributes, .. } => attributes,
        }
    }

    pub fn attributes_mut(&mut self) -> &mut Vec<OwnedAttribute> {
        match self {
            Self::Parsed { attributes, .. } | Self::Verbatim { attributes, .. } => attributes,
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
            Block::Parsed(block) => Self::Parsed {
                marker: block.mark.as_ref().map(|mark| mark.marker.clone()),
                attributes: block
                    .mark
                    .as_ref()
                    .map_or_else(Vec::new, |mark| owned_attributes(&mark.attrs)),
                head: block
                    .head
                    .items
                    .iter()
                    .map(|inline| OwnedInline::from_syntax(source, inline))
                    .collect(),
                children: block
                    .children
                    .iter()
                    .map(|child| Self::from_syntax(source, child))
                    .collect(),
            },
            Block::Verbatim(block) => Self::Verbatim {
                attributes: owned_attributes(&block.attrs),
                text: block.text.clone(),
            },
        }
    }

    pub fn format(&self) -> Result<String, EditError> {
        format_owned_blocks(std::slice::from_ref(self), "\n")
    }
}

impl OwnedInline {
    fn from_syntax(source: &str, inline: &Inline) -> Self {
        match inline {
            Inline::Text { text, .. } => Self::Text(text.clone()),
            Inline::SoftBreak { .. } => Self::SoftBreak,
            Inline::Element {
                kind,
                content,
                attrs,
                ..
            } => Self::Element {
                kind: kind.clone(),
                content: content
                    .items
                    .iter()
                    .map(|inline| Self::from_syntax(source, inline))
                    .collect(),
                attributes: owned_attributes(attrs),
            },
            Inline::Verbatim { text, attrs, .. } => Self::Verbatim {
                text: text.clone(),
                attributes: owned_attributes(attrs),
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
        let mut items = attribute_sources(&self.parsed.source, attributes);
        for (position, item) in additions {
            let index = insertion_index(position, items.len())?;
            items.insert(index, item.render());
        }
        self.replace_attribute_slot(attributes, owner_insert, items)
    }

    pub fn replace_attribute(
        &mut self,
        attributes: &Attributes,
        index: usize,
        item: OwnedAttribute,
    ) -> Result<(), EditError> {
        let mut items = attribute_sources(&self.parsed.source, attributes);
        let target = items
            .get_mut(index)
            .ok_or(EditError::InvalidAttributePosition)?;
        *target = item.render();
        self.replace_attribute_slot(attributes, 0, items)
    }

    pub fn remove_attribute(
        &mut self,
        attributes: &Attributes,
        index: usize,
    ) -> Result<(), EditError> {
        let mut items = attribute_sources(&self.parsed.source, attributes);
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
        items: Vec<String>,
    ) -> Result<(), EditError> {
        let (range, new_text) = if let Some(range) = &attributes.range {
            (range.clone(), render_attribute_slot(&items))
        } else {
            if owner_insert > self.parsed.source.len()
                || !self.parsed.source.is_char_boundary(owner_insert)
            {
                return Err(EditError::InvalidRange);
            }
            (owner_insert..owner_insert, render_attribute_slot(&items))
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

fn attribute_sources(source: &str, attributes: &Attributes) -> Vec<String> {
    attributes
        .items
        .iter()
        .map(|item| source[item_range(item)].to_string())
        .collect()
}

fn owned_attributes(attributes: &Attributes) -> Vec<OwnedAttribute> {
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

fn item_range(item: &AttrItem) -> Range<usize> {
    match item {
        AttrItem::Id { range, .. }
        | AttrItem::Class { range, .. }
        | AttrItem::Pair { range, .. } => range.clone(),
    }
}

fn render_attribute_slot(items: &[String]) -> String {
    format!("{{{}}}", items.join(" "))
}

fn render_owned_attributes(attributes: &[OwnedAttribute], output: &mut String) {
    if attributes.is_empty() {
        return;
    }
    output.push('{');
    for (index, attribute) in attributes.iter().enumerate() {
        if index > 0 {
            output.push(' ');
        }
        output.push_str(&attribute.render());
    }
    output.push('}');
}

fn format_owned_blocks(blocks: &[OwnedBlock], newline: &str) -> Result<String, EditError> {
    if blocks.is_empty() {
        return Ok(String::new());
    }
    let mut source = String::new();
    render_owned_blocks(blocks, 0, &mut source);
    source.push('\n');
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
            attributes,
            head,
            children,
        } => {
            if let Some(marker) = marker {
                output.push('`');
                output.push_str(marker);
                render_owned_attributes(attributes, output);
                if !head.is_empty() {
                    output.push(' ');
                }
            }
            render_owned_inlines(head, marker.is_some(), output);
            if !children.is_empty() {
                if head.is_empty() {
                    output.push('\n');
                } else {
                    output.push_str("\n\n");
                }
                render_owned_blocks(children, indent + 2, output);
            }
        }
        OwnedBlock::Verbatim { attributes, text } => {
            output.push('`');
            output.push('{');
            for (index, attribute) in attributes.iter().enumerate() {
                if index > 0 {
                    output.push(' ');
                }
                output.push_str(&attribute.render());
            }
            output.push('}');
            if !text.is_empty() {
                output.push('\n');
                for (index, line) in text.split_terminator('\n').enumerate() {
                    if index > 0 {
                        output.push('\n');
                    }
                    if !line.is_empty() {
                        output.extend(std::iter::repeat_n(' ', indent + 2));
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

fn render_owned_inlines(inlines: &[OwnedInline], nested: bool, output: &mut String) {
    for inline in inlines {
        match inline {
            OwnedInline::Text(text) => {
                for character in text.chars() {
                    match character {
                        '`' => output.push_str("``"),
                        ']' if nested => output.push_str("`]"),
                        _ => output.push(character),
                    }
                }
            }
            OwnedInline::SoftBreak => output.push('\n'),
            OwnedInline::Element {
                kind,
                content,
                attributes,
            } => {
                output.push('`');
                output.push_str(kind);
                output.push('[');
                render_owned_inlines(content, true, output);
                output.push(']');
                render_owned_attributes(attributes, output);
            }
            OwnedInline::Verbatim { text, attributes } => {
                let quotes = minimum_quote_count(text);
                output.push('`');
                output.push_str(&"\"".repeat(quotes));
                output.push('[');
                output.push_str(text);
                output.push(']');
                output.push_str(&"\"".repeat(quotes));
                render_owned_attributes(attributes, output);
            }
        }
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
    if parsed.syntax.blocks.is_empty() {
        let new_text = plumb_format::format(&modified).map_err(|_| EditError::GeneratedInvalid)?;
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
    let formatted = plumb_format::format_block_range(&modified, affected.start..modified_end)
        .map_err(|_| EditError::GeneratedInvalid)?;
    if formatted.range.end < modified_end {
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

#[cfg(test)]
mod tests {
    use super::*;
    use plumb_core::{parse, Block};

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
        let source = "`-{.task #id created=now} Work\n";
        let parsed = parse(source);
        let Block::Parsed(block) = &parsed.syntax.blocks[0] else {
            panic!("expected parsed block");
        };
        let mark = block.mark.as_ref().unwrap();
        let mut edit = EditSession::new(&parsed, block.range.clone()).unwrap();
        edit.insert_attribute(
            &mark.attrs,
            mark.marker_range.end,
            AttributePosition::Before(1),
            OwnedAttribute::class("next"),
        )
        .unwrap();
        let edit = edit.finish().unwrap();
        assert_eq!(edit.new_text, "`-{.task .next #id created=now} Work\n");
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
            "`-{created=\"2026-07-23T03:00:00+08:00\"} Work\n"
        );
    }

    #[test]
    fn rejects_implicit_or_out_of_bounds_positions() {
        let source = "`-{.task} Work\n";
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
    }

    #[test]
    fn inserts_owned_metadata_before_existing_blocks() {
        let parsed = parse("`# Existing\n");
        let metadata = OwnedBlock::marked("meta", "").with_children(vec![
            OwnedBlock::marked(":", "title").with_children(vec![OwnedBlock::paragraph("Example")]),
            OwnedBlock::marked(":", "created")
                .with_children(vec![OwnedBlock::paragraph("2026-07-23T03:00:00+08:00")]),
        ]);
        let mut edit = EditSession::new(&parsed, 0..0).unwrap();
        edit.insert_blocks(0, &[metadata]).unwrap();
        let edit = edit.finish().unwrap();
        assert_eq!(edit.range, 0..0);
        assert_eq!(
            edit.new_text,
            "`meta\n `: title\n\n    Example\n\n `: created\n\n    2026-07-23T03:00:00+08:00\n\n"
        );
    }

    #[test]
    fn round_trips_owned_syntax_without_extension_knowledge() {
        let source = "`node{#id .opaque key=bare} Head `span[text] and `[raw]\n\n  `child Body\n";
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
        assert!(formatted.contains(".opaque"));
        assert!(formatted.contains("`span[text]"));
        assert!(formatted.contains("`[raw]"));
        assert!(formatted.contains("`child Body"));
    }
}
