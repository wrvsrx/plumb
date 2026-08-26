use std::collections::HashMap;
#[cfg(test)]
use std::fmt::Write;
use std::ops::Range;

use plumb_syntax::{parse, Block, Inline, InlineContent, ParsedBlock, ParsedDocument, RawPayload};
#[cfg(test)]
use plumb_syntax::{AttrItem, Attributes};
use similar::{DiffOp, TextDiff};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    InvalidSyntax,
    InvalidBlockRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatEdit {
    pub range: Range<usize>,
    pub new_text: String,
}

pub fn format(source: &str) -> Result<String, FormatError> {
    let parsed = parse(source);
    format_parsed(&parsed)
}

pub fn format_parsed(parsed: &ParsedDocument) -> Result<String, FormatError> {
    if !parsed.is_valid() {
        return Err(FormatError::InvalidSyntax);
    }

    let mut formatter = Formatter::default();
    let body = &parsed.syntax.blocks;
    formatter.blocks(&body, 0);
    if !terminal_verbatim(&body) && !formatter.output.is_empty() {
        formatter.output.push('\n');
    }
    Ok(formatter.output)
}

pub fn format_edits(source: &str) -> Result<Vec<FormatEdit>, FormatError> {
    let parsed = parse(source);
    format_parsed_edits(&parsed)
}

pub fn format_parsed_edits(parsed: &ParsedDocument) -> Result<Vec<FormatEdit>, FormatError> {
    let source = parsed.source.as_str();
    let formatted = format_parsed(parsed)?;
    if formatted == source {
        return Ok(Vec::new());
    }

    let source_offsets = line_offsets(source);
    let formatted_offsets = line_offsets(&formatted);
    let operations = anchored_line_diff(source, &formatted, &source_offsets, &formatted_offsets);
    let mut edits = Vec::new();
    let mut index = 0;
    while index < operations.len() {
        if matches!(operations[index], DiffOp::Equal { .. }) {
            index += 1;
            continue;
        }
        let mut old_start = usize::MAX;
        let mut old_end = 0;
        let mut new_start = usize::MAX;
        let mut new_end = 0;
        while index < operations.len() {
            let operation = &operations[index];
            if matches!(operation, DiffOp::Equal { .. }) {
                let equal = operation.old_range();
                if equal.len() > 1
                    || index + 1 == operations.len()
                    || matches!(operations[index + 1], DiffOp::Equal { .. })
                {
                    break;
                }
            }
            let old = operation.old_range();
            let new = operation.new_range();
            old_start = old_start.min(old.start);
            old_end = old_end.max(old.end);
            new_start = new_start.min(new.start);
            new_end = new_end.max(new.end);
            index += 1;
        }
        edits.push(FormatEdit {
            range: source_offsets[old_start]..source_offsets[old_end],
            new_text: formatted[formatted_offsets[new_start]..formatted_offsets[new_end]]
                .to_string(),
        });
    }
    let mut applied = source.to_string();
    for edit in edits.iter().rev() {
        applied.replace_range(edit.range.clone(), &edit.new_text);
    }
    if applied != formatted {
        return Ok(vec![FormatEdit {
            range: 0..source.len(),
            new_text: formatted,
        }]);
    }
    Ok(edits)
}

fn anchored_line_diff(
    source: &str,
    formatted: &str,
    source_offsets: &[usize],
    formatted_offsets: &[usize],
) -> Vec<DiffOp> {
    let source_lines = line_slices(source, source_offsets);
    let formatted_lines = line_slices(formatted, formatted_offsets);
    let source_unique = unique_line_indices(&source_lines);
    let formatted_unique = unique_line_indices(&formatted_lines);
    let mut anchors = Vec::new();
    let mut last_formatted = None;

    for (source_index, line) in source_lines.iter().enumerate() {
        if source_unique.get(line) != Some(&Some(source_index)) {
            continue;
        }
        let Some(Some(formatted_index)) = formatted_unique.get(line) else {
            continue;
        };
        if last_formatted.is_none_or(|last| *formatted_index > last) {
            anchors.push((source_index, *formatted_index));
            last_formatted = Some(*formatted_index);
        }
    }

    if anchors.is_empty() {
        return TextDiff::from_lines(source, formatted).ops().to_vec();
    }

    let mut operations = Vec::new();
    let mut source_cursor = 0;
    let mut formatted_cursor = 0;
    for (source_anchor, formatted_anchor) in anchors {
        append_line_diff(
            &mut operations,
            source,
            formatted,
            source_offsets,
            formatted_offsets,
            source_cursor..source_anchor,
            formatted_cursor..formatted_anchor,
        );
        push_equal(&mut operations, source_anchor, formatted_anchor);
        source_cursor = source_anchor + 1;
        formatted_cursor = formatted_anchor + 1;
    }
    append_line_diff(
        &mut operations,
        source,
        formatted,
        source_offsets,
        formatted_offsets,
        source_cursor..source_lines.len(),
        formatted_cursor..formatted_lines.len(),
    );
    operations
}

fn line_slices<'a>(source: &'a str, offsets: &[usize]) -> Vec<&'a str> {
    offsets
        .windows(2)
        .map(|range| &source[range[0]..range[1]])
        .collect()
}

fn unique_line_indices<'a>(lines: &[&'a str]) -> HashMap<&'a str, Option<usize>> {
    let mut indices = HashMap::with_capacity(lines.len());
    for (index, line) in lines.iter().copied().enumerate() {
        indices
            .entry(line)
            .and_modify(|existing| *existing = None)
            .or_insert(Some(index));
    }
    indices
}

fn append_line_diff(
    operations: &mut Vec<DiffOp>,
    source: &str,
    formatted: &str,
    source_offsets: &[usize],
    formatted_offsets: &[usize],
    source_lines: Range<usize>,
    formatted_lines: Range<usize>,
) {
    if source_lines.is_empty() && formatted_lines.is_empty() {
        return;
    }
    let source_slice =
        &source[source_offsets[source_lines.start]..source_offsets[source_lines.end]];
    let formatted_slice = &formatted
        [formatted_offsets[formatted_lines.start]..formatted_offsets[formatted_lines.end]];
    for operation in TextDiff::from_lines(source_slice, formatted_slice).ops() {
        operations.push(offset_diff_op(
            *operation,
            source_lines.start,
            formatted_lines.start,
        ));
    }
}

fn offset_diff_op(operation: DiffOp, source_offset: usize, formatted_offset: usize) -> DiffOp {
    match operation {
        DiffOp::Equal {
            old_index,
            new_index,
            len,
        } => DiffOp::Equal {
            old_index: old_index + source_offset,
            new_index: new_index + formatted_offset,
            len,
        },
        DiffOp::Delete {
            old_index,
            old_len,
            new_index,
        } => DiffOp::Delete {
            old_index: old_index + source_offset,
            old_len,
            new_index: new_index + formatted_offset,
        },
        DiffOp::Insert {
            old_index,
            new_index,
            new_len,
        } => DiffOp::Insert {
            old_index: old_index + source_offset,
            new_index: new_index + formatted_offset,
            new_len,
        },
        DiffOp::Replace {
            old_index,
            old_len,
            new_index,
            new_len,
        } => DiffOp::Replace {
            old_index: old_index + source_offset,
            old_len,
            new_index: new_index + formatted_offset,
            new_len,
        },
    }
}

fn push_equal(operations: &mut Vec<DiffOp>, old_index: usize, new_index: usize) {
    if let Some(DiffOp::Equal {
        old_index: previous_old,
        new_index: previous_new,
        len,
    }) = operations.last_mut()
    {
        if *previous_old + *len == old_index && *previous_new + *len == new_index {
            *len += 1;
            return;
        }
    }
    operations.push(DiffOp::Equal {
        old_index,
        new_index,
        len: 1,
    });
}

fn line_offsets(source: &str) -> Vec<usize> {
    let mut offsets = vec![0];
    offsets.extend(source.match_indices('\n').map(|(offset, _)| offset + 1));
    if offsets.last().copied() != Some(source.len()) {
        offsets.push(source.len());
    }
    offsets
}

/// Formats complete sibling blocks covered by `range`. The following sibling
/// is used as read-only spacing context and is not itself reformatted.
pub fn format_block_range(source: &str, range: Range<usize>) -> Result<FormatEdit, FormatError> {
    let parsed = parse(source);
    format_parsed_block_range(&parsed, range)
}

pub fn format_parsed_block_range(
    parsed: &ParsedDocument,
    range: Range<usize>,
) -> Result<FormatEdit, FormatError> {
    if !parsed.is_valid() {
        return Err(FormatError::InvalidSyntax);
    }
    let source = parsed.source.as_str();
    if range.start > range.end || range.end > source.len() {
        return Err(FormatError::InvalidBlockRange);
    }

    let (blocks, first, last) = sibling_block_range(source, &parsed.syntax.blocks, &range)
        .ok_or(FormatError::InvalidBlockRange)?;
    Ok(format_block_group(source, blocks, first, last))
}

/// Formats maximal complete block subtrees contained by `selection`.
pub fn format_contained_blocks(
    source: &str,
    selection: Range<usize>,
) -> Result<Vec<FormatEdit>, FormatError> {
    let parsed = parse(source);
    format_parsed_contained_blocks(&parsed, selection)
}

pub fn format_parsed_contained_blocks(
    parsed: &ParsedDocument,
    selection: Range<usize>,
) -> Result<Vec<FormatEdit>, FormatError> {
    if !parsed.is_valid() {
        return Err(FormatError::InvalidSyntax);
    }
    let source = parsed.source.as_str();
    if selection.start > selection.end
        || selection.end > source.len()
        || !source.is_char_boundary(selection.start)
        || !source.is_char_boundary(selection.end)
    {
        return Err(FormatError::InvalidBlockRange);
    }
    if selection.is_empty() {
        return Ok(Vec::new());
    }

    let mut groups = Vec::new();
    collect_contained_groups(&parsed.syntax.blocks, &selection, &mut groups);
    let mut edits = groups
        .into_iter()
        .map(|group| format_contained_group(source, group.blocks, group.first, group.last))
        .filter(|edit| source[edit.range.clone()] != edit.new_text)
        .collect::<Vec<_>>();
    edits.sort_by_key(|edit| edit.range.start);
    if edits
        .windows(2)
        .any(|edits| edits[0].range.end > edits[1].range.start)
    {
        return Err(FormatError::InvalidBlockRange);
    }
    Ok(edits)
}

fn format_contained_group(source: &str, blocks: &[Block], first: usize, last: usize) -> FormatEdit {
    let selected = &blocks[first..=last];
    let block_start = selected.first().unwrap().range().start;
    let line_start = source[..block_start]
        .rfind('\n')
        .map_or(0, |offset| offset + 1);
    let indent = source[line_start..block_start].chars().count();
    let edit_range = block_start..block_content_range(selected.last().unwrap()).end;

    let mut formatter = Formatter::default();
    formatter.blocks(selected, indent);
    let prefix = " ".repeat(indent);
    let mut new_text = formatter
        .output
        .split_inclusive('\n')
        .map(|line| line.strip_prefix(&prefix).unwrap_or(line))
        .collect::<String>();
    if source.contains("\r\n") {
        new_text = new_text.replace('\n', "\r\n");
    }
    FormatEdit {
        range: edit_range,
        new_text,
    }
}

fn format_block_group(source: &str, blocks: &[Block], first: usize, last: usize) -> FormatEdit {
    let selected = &blocks[first..=last];
    let following = blocks.get(last + 1);
    let block_start = selected.first().unwrap().range().start;
    let line_start = source[..block_start]
        .rfind('\n')
        .map_or(0, |offset| offset + 1);
    let edit_range = line_start
        ..following.map_or_else(
            || selected.last().unwrap().range().end,
            |block| {
                source[..block.range().start]
                    .rfind('\n')
                    .map_or(0, |offset| offset + 1)
            },
        );
    let indent = source[line_start..block_start].chars().count();

    let mut formatter = Formatter::default();
    formatter.blocks(selected, indent);
    if let Some(following) = following {
        if compact_siblings(selected.last().unwrap(), following) {
            formatter.output.push('\n');
        } else {
            formatter.output.push_str("\n\n");
        }
    } else if !terminal_verbatim(selected) && !formatter.output.is_empty() {
        formatter.output.push('\n');
    }
    let mut new_text = formatter.output;
    if source.contains("\r\n") {
        new_text = new_text.replace('\n', "\r\n");
    }
    FormatEdit {
        range: edit_range,
        new_text,
    }
}

#[derive(Debug, Clone, Copy)]
struct BlockGroup<'a> {
    blocks: &'a [Block],
    first: usize,
    last: usize,
}

fn collect_contained_groups<'a>(
    blocks: &'a [Block],
    selection: &Range<usize>,
    groups: &mut Vec<BlockGroup<'a>>,
) {
    let mut group_start = None;
    for (index, block) in blocks.iter().enumerate() {
        let content = block_content_range(block);
        if selection.start <= content.start && content.end <= selection.end {
            group_start.get_or_insert(index);
            continue;
        }

        if let Some(first) = group_start.take() {
            groups.push(BlockGroup {
                blocks,
                first,
                last: index - 1,
            });
        }
        collect_contained_groups(block.children(), selection, groups);
    }
    if let Some(first) = group_start {
        groups.push(BlockGroup {
            blocks,
            first,
            last: blocks.len() - 1,
        });
    }
}

fn block_content_range(block: &Block) -> Range<usize> {
    match block {
        Block::Parsed(block) => {
            let own_end = block.mark.as_ref().map_or(block.head.range.end, |mark| {
                mark.range.end.max(block.head.range.end)
            });
            let end = block
                .children
                .last()
                .map_or(own_end, |child| block_content_range(child).end.max(own_end));
            let end = block.raw.as_ref().map_or(end, |raw| end.max(raw.range.end));
            block.range.start..end
        }
        Block::Verbatim(block) => {
            block.range.start..block.text_range.end.max(block.opener_range.end)
        }
    }
}

fn sibling_block_range<'a>(
    source: &str,
    blocks: &'a [Block],
    range: &Range<usize>,
) -> Option<(&'a [Block], usize, usize)> {
    if let Some(first) = blocks
        .iter()
        .position(|block| block.range().start == range.start)
    {
        let last = blocks[first..]
            .iter()
            .take_while(|block| block.range().end <= range.end)
            .count()
            .checked_sub(1)?
            + first;
        if source[blocks[last].range().end..range.end]
            .chars()
            .all(|character| matches!(character, '\r' | '\n'))
        {
            return Some((blocks, first, last));
        }
    }

    blocks.iter().find_map(|block| {
        (block.range().start <= range.start && range.end <= block.range().end)
            .then(|| sibling_block_range(source, block.children(), range))
            .flatten()
    })
}

fn terminal_verbatim(blocks: &[Block]) -> bool {
    let Some(last) = blocks.last() else {
        return false;
    };
    match last {
        Block::Verbatim(_) => true,
        Block::Parsed(block) if block.raw.is_some() => true,
        Block::Parsed(block) => terminal_verbatim(&block.children),
    }
}

#[derive(Default)]
struct Formatter {
    output: String,
}

impl Formatter {
    fn blocks(&mut self, blocks: &[Block], indent: usize) {
        for (index, block) in blocks.iter().enumerate() {
            if index > 0 {
                let previous = &blocks[index - 1];
                if terminal_verbatim(std::slice::from_ref(previous)) {
                    while !self.output.ends_with("\n\n") {
                        self.output.push('\n');
                    }
                } else if compact_siblings(previous, block) {
                    self.output.push('\n');
                } else {
                    self.output.push_str("\n\n");
                }
            }
            self.block(block, indent);
        }
    }

    fn block(&mut self, block: &Block, indent: usize) {
        match block {
            Block::Parsed(block) => self.parsed_block(block, indent),
            Block::Verbatim(block) => {
                self.indent(indent);
                self.output.push_str("`\"");
                self.raw_text(&block.text, indent + 1);
            }
        }
    }

    fn parsed_block(&mut self, block: &ParsedBlock, indent: usize) {
        self.indent(indent);
        let continuation_indent = if let Some(mark) = &block.mark {
            let marker = mark.marker.as_str();
            self.output.push('`');
            self.output.push_str(marker);
            if !block.head.items.is_empty() {
                self.output.push(' ');
            }
            indent + 1
        } else {
            indent
        };
        self.inlines(&block.head, continuation_indent, false);

        if !block.children.is_empty() {
            if block.head.items.is_empty() && block.raw.is_none() {
                self.output.push('\n');
            } else {
                self.output.push_str("\n\n");
            }
            let child_indent = block.mark.as_ref().map_or(indent, |_| indent + 1);
            self.blocks(&block.children, child_indent);
        }
        if let Some(raw) = &block.raw {
            self.output.push_str("\n\n");
            self.indent(indent);
            self.output.push('"');
            self.raw_payload(raw, indent + 1);
        }
    }

    fn inlines(&mut self, content: &InlineContent, continuation_indent: usize, nested: bool) {
        for inline in &content.items {
            self.inline(inline, continuation_indent, nested, true);
        }
    }

    fn inline(
        &mut self,
        inline: &Inline,
        continuation_indent: usize,
        nested: bool,
        introduced: bool,
    ) {
        match inline {
            Inline::Text { text, .. } => self.text(text, nested),
            Inline::Space { text, .. } => self.output.push_str(text),
            Inline::SoftBreak { .. } => {
                self.output.push('\n');
                self.indent(continuation_indent);
            }
            Inline::Element { kind, members, .. } => {
                if introduced {
                    self.output.push('`');
                }
                self.output.push_str(kind);
                self.output.push('[');
                for (index, member) in members.iter().enumerate() {
                    if index > 0 {
                        self.output.push('|');
                    }
                    match member {
                        plumb_syntax::InlineMember::ParsedArgument(argument) => {
                            self.inlines(&argument.content, continuation_indent, true);
                        }
                        plumb_syntax::InlineMember::VerbatimArgument(argument) => {
                            self.verbatim_payload(&argument.text);
                        }
                        plumb_syntax::InlineMember::Child { inline, .. } => {
                            self.inline(inline, continuation_indent, true, false);
                        }
                    }
                }
                self.output.push(']');
            }
            Inline::Verbatim { kind, text, .. } => {
                if introduced {
                    self.output.push('`');
                }
                self.output.push_str(kind);
                self.verbatim_payload(text);
            }
        }
    }

    fn verbatim_payload(&mut self, text: &str) {
        // A compact payload beginning with `[` would be reparsed as a
        // bracket envelope. Keep the bracketed spelling so the bracket is raw.
        if !text.is_empty() && !text.contains('"') && !text.starts_with('[') {
            self.output.push('"');
            self.output.push_str(text);
            self.output.push('"');
        } else {
            let quotes = minimum_quote_count(text).max(1);
            for _ in 0..quotes {
                self.output.push('"');
            }
            self.output.push('[');
            self.output.push_str(text);
            self.output.push(']');
            for _ in 0..quotes {
                self.output.push('"');
            }
        }
    }

    fn raw_payload(&mut self, raw: &RawPayload, body_indent: usize) {
        self.raw_text(&raw.text, body_indent);
    }

    fn raw_text(&mut self, text: &str, body_indent: usize) {
        if text.is_empty() {
            return;
        }
        self.output.push('\n');
        let mut lines = text.split('\n').collect::<Vec<_>>();
        let has_final_newline = text.ends_with('\n');
        if has_final_newline {
            lines.pop();
        }
        let last_content = lines.iter().rposition(|line| !line.is_empty());
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                self.output.push('\n');
            }
            if !line.is_empty() {
                self.indent(body_indent);
                self.output.push_str(line);
            } else if last_content.is_none_or(|last| index > last) {
                self.indent(body_indent);
            }
        }
        if has_final_newline {
            self.output.push('\n');
        }
    }

    fn text(&mut self, text: &str, _nested: bool) {
        // §2: structural delimiters never become bare when rendered as text.
        for character in text.chars() {
            match character {
                '`' => self.output.push_str("``"),
                '[' => self.output.push_str("`["),
                ']' => self.output.push_str("`]"),
                '{' | '}' => self.output.push(character),
                '|' => self.output.push_str("`|"),
                _ => self.output.push(character),
            }
        }
    }

    fn indent(&mut self, indent: usize) {
        self.output.extend(std::iter::repeat_n(' ', indent));
    }
}

fn compact_siblings(previous: &Block, current: &Block) -> bool {
    let (Block::Parsed(previous), Block::Parsed(current)) = (previous, current) else {
        return false;
    };
    let (Some(previous_mark), Some(current_mark)) = (&previous.mark, &current.mark) else {
        return false;
    };
    previous.children.is_empty()
        && previous.raw.is_none()
        && previous_mark.marker == current_mark.marker
}

fn minimum_quote_count(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut maximum = None;
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] != b']' {
            cursor += 1;
            continue;
        }
        let mut quotes = 0;
        while cursor + 1 + quotes < bytes.len() && bytes[cursor + 1 + quotes] == b'"' {
            quotes += 1;
        }
        maximum = Some(maximum.map_or(quotes, |current: usize| current.max(quotes)));
        cursor += 1 + quotes;
    }
    maximum.map_or(0, |quotes| quotes + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_formats(source: &str, expected: &str) {
        let original = parse(source);
        assert!(original.is_valid(), "{:?}", original.diagnostics);
        let formatted = format(source).unwrap();
        assert_eq!(formatted, expected);
        let reparsed = parse(&formatted);
        assert!(reparsed.is_valid(), "{:?}", reparsed.diagnostics);
        assert_eq!(
            shape_document(&original.syntax),
            shape_document(&reparsed.syntax)
        );
        assert_eq!(format(&formatted).unwrap(), formatted);
    }

    fn shape_document(document: &plumb_syntax::Document) -> String {
        let mut output = String::new();
        shape_attrs(&document.attrs, &mut output);
        shape_blocks(&document.blocks, &mut output);
        output
    }

    fn shape_blocks(blocks: &[Block], output: &mut String) {
        output.push('[');
        for block in blocks {
            match block {
                Block::Parsed(block) => {
                    output.push('P');
                    match &block.mark {
                        Some(mark) => {
                            let _ = write!(output, "{:?}", mark.marker);
                            shape_attrs(&mark.attrs, output);
                        }
                        None => output.push('-'),
                    }
                    shape_inlines(&block.head, output);
                    shape_blocks(&block.children, output);
                    if let Some(raw) = &block.raw {
                        let _ = write!(output, "R{:?}", raw.text);
                    }
                }
                Block::Verbatim(block) => {
                    output.push('V');
                    let _ = write!(output, "{:?}", block.text);
                }
            }
        }
        output.push(']');
    }

    fn shape_inlines(content: &InlineContent, output: &mut String) {
        output.push('(');
        for inline in &content.items {
            match inline {
                Inline::Text { text, .. } => {
                    let _ = write!(output, "T{text:?}");
                }
                Inline::Space { text, .. } => {
                    let _ = write!(output, "W{text:?}");
                }
                Inline::SoftBreak { .. } => output.push('S'),
                Inline::Element {
                    kind,
                    members,
                    attrs,
                    ..
                } => {
                    let _ = write!(output, "E{kind:?}");
                    for member in members {
                        match member {
                            plumb_syntax::InlineMember::ParsedArgument(argument) => {
                                output.push_str("A[");
                                shape_inlines(&argument.content, output);
                                output.push(']');
                            }
                            plumb_syntax::InlineMember::VerbatimArgument(argument) => {
                                let _ = write!(output, "R{:?}", argument.text);
                            }
                            plumb_syntax::InlineMember::Child { inline, .. } => {
                                output.push('C');
                                let content = InlineContent {
                                    range: 0..0,
                                    items: vec![inline.as_ref().clone()],
                                };
                                shape_inlines(&content, output);
                            }
                        }
                    }
                    shape_attrs(attrs, output);
                }
                Inline::Verbatim { text, attrs, .. } => {
                    let _ = write!(output, "V{text:?}");
                    shape_attrs(attrs, output);
                }
            }
        }
        output.push(')');
    }

    fn shape_attrs(attrs: &Attributes, output: &mut String) {
        match &attrs.range {
            None => output.push('-'),
            Some(_) => {
                output.push('{');
                for item in &attrs.items {
                    match item {
                        AttrItem::Id { value, .. } => {
                            let _ = write!(output, "I{value:?}");
                        }
                        AttrItem::Class { value, .. } => {
                            let _ = write!(output, "C{value:?}");
                        }
                        AttrItem::Pair { key, value, .. } => {
                            let _ = write!(output, "K{key:?}={:?}", value.decoded);
                        }
                    }
                }
                output.push('}');
            }
        }
    }

    #[test]
    fn formats_direct_declarations_and_recursive_children() {
        assert_formats(
            "`=   title Document title\n\n`= tags plumb\n\n`task   Buy milk\n   `+ task\n   `@ shopping\n\n   `note Details\n",
            "`= title Document title\n`= tags plumb\n\n`task Buy milk\n\n `+ task\n\n `@ shopping\n\n `note Details\n",
        );
        assert_formats("", "");
    }

    #[test]
    fn formats_anonymous_and_owned_raw_payloads() {
        assert_formats("`\"\"\n  payload\n", "`\"\n payload\n");
        assert_formats(
            "`rust\n   `@ example\n\n\"\n fn main() {}\n",
            "`rust\n\n `@ example\n\n\"\n fn main() {}\n",
        );
        assert_formats(
            "`example\n   `\"\n    child raw\n",
            "`example\n `\"\n  child raw\n",
        );
    }

    #[test]
    fn braces_are_preserved_as_ordinary_text() {
        assert_formats("Text { fn() {} }\n", "Text { fn() {} }\n");
        assert_formats(
            "`marker{brace} value{inside}\n",
            "`marker{brace} value{inside}\n",
        );
    }

    #[test]
    fn preserves_inline_members_and_soft_breaks() {
        assert_formats(
            "See `->[guide|@[main]|+[external]|=[to|guide.plumb]].\n",
            "See `->[guide|@[main]|+[external]|=[to|guide.plumb]].\n",
        );
        assert_formats(
            "`note First `span[a `] b `` c]\n   second\n",
            "`note First `span[a `] b `` c]\n second\n",
        );
    }

    #[test]
    fn chooses_minimum_safe_verbatim_delimiters() {
        assert_formats("Raw `\"\"\"[a ]\" b]\"\"\".\n", "Raw `\"\"[a ]\" b]\"\".\n");
        assert_formats("`\"[[]]\"\n", "`\"[[]]\"\n");
        assert_formats("Before `\"[]\" after.\n", "Before `\"[]\" after.\n");
    }

    #[test]
    fn aligns_children_and_compacts_same_marker_siblings() {
        assert_formats(
            "`meta\n  `= title\n\n     this is a title\n  `= created\n\n     2026-07-20\n`- before\n\n`- something\n  `- nested\n`- after\n",
            "`meta\n `= title\n\n  this is a title\n\n `= created\n\n  2026-07-20\n\n`- before\n`- something\n\n `- nested\n\n`- after\n",
        );
    }

    #[test]
    fn raw_payload_preserves_content_and_final_newline() {
        assert_formats("`\"\"\n  final newline\n", "`\"\n final newline\n");
        assert_formats("`\"\"\n  no newline", "`\"\n no newline");
        assert_formats(
            "`text\n\"\n   leading\n \n\nnext\n",
            "`text\n\n\"\n   leading\n \n\nnext\n",
        );
    }

    #[test]
    fn rejects_invalid_documents() {
        assert_eq!(format("`span[open\n"), Err(FormatError::InvalidSyntax));
        assert_eq!(format("`rust\"\n raw\n"), Err(FormatError::InvalidSyntax));
    }

    #[test]
    fn whole_document_edits_apply_to_the_canonical_result() {
        let source = "`task   First\n   `@ one\n\n`task   Second\n   `@ two\n";
        let canonical = format(source).unwrap();
        let edits = format_edits(source).unwrap();
        let mut edited = source.to_string();
        for edit in edits.iter().rev() {
            edited.replace_range(edit.range.clone(), &edit.new_text);
        }
        assert_eq!(edited, canonical);
        assert!(format_edits(&edited).unwrap().is_empty());
    }

    #[test]
    fn whole_document_diff_anchors_large_repeated_layout() {
        let mut source = String::new();
        for index in 0..512 {
            let _ = write!(source, "`event   Event {index}\n   `= uid repeated\n\n");
        }
        let canonical = format(&source).unwrap();
        let edits = format_edits(&source).unwrap();
        let mut edited = source;
        for edit in edits.iter().rev() {
            edited.replace_range(edit.range.clone(), &edit.new_text);
        }
        assert_eq!(edited, canonical);
        assert!(edits.len() < 512);
    }

    #[test]
    fn block_range_formats_complete_siblings_with_following_context() {
        let source = "`task   First\n   `@ one\n\n`task   Second\n   `@ two\n\n`event Following\n";
        let parsed = parse(source);
        let first = parsed.syntax.blocks[0].range().clone();
        let second = parsed.syntax.blocks[1].range().clone();
        let edit = format_block_range(source, first.start..second.end).unwrap();
        let mut edited = source.to_string();
        edited.replace_range(edit.range.clone(), &edit.new_text);
        assert_eq!(
            edited,
            "`task First\n\n `@ one\n\n`task Second\n\n `@ two\n\n`event Following\n"
        );
    }

    #[test]
    fn nested_block_range_preserves_crlf_and_external_layout() {
        let source = "`node Parent\r\n   `task   First\r\n    `@ one\r\n\r\n   `task Second\r\n    `@ two\r\n\r\n`event Following\r\n";
        let parsed = parse(source);
        let children = parsed.syntax.blocks[0].children();
        let edit =
            format_block_range(source, children[0].range().start..children[1].range().end).unwrap();
        assert!(edit.new_text.contains("\r\n"));
        assert!(!edit.new_text.contains("\n") || edit.new_text.contains("\r\n"));
        assert_eq!(&source[edit.range.end..], "`event Following\r\n");
    }

    #[test]
    fn block_range_rejects_partial_blocks() {
        let source = "`- First\n`- Second\n";
        assert_eq!(
            format_block_range(source, 1..source.len()),
            Err(FormatError::InvalidBlockRange)
        );
    }

    #[test]
    fn contained_range_formats_complete_maximal_blocks() {
        let source = "`node Parent\n       `task   One\n         `@ one\n\n       `task   Two\n         `@ two\n\n`event Following\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let parent = &parsed.syntax.blocks[0];
        let children = parent.children();
        let selection =
            block_content_range(&children[0]).start..block_content_range(&children[1]).end;
        let edits = format_contained_blocks(source, selection).unwrap();
        assert_eq!(edits.len(), 1);
        assert!(!edits[0].new_text.contains("`node Parent"));
        assert!(edits[0].new_text.contains("`task One"));
        assert!(edits[0].new_text.contains("`task Two"));
        assert_eq!(&source[edits[0].range.end..], "\n\n`event Following\n");
    }

    #[test]
    fn contained_range_formats_a_complete_parent_subtree() {
        let source = "`node   Parent\n       `task   One\n         `@ one\n\n`event Following\n";
        let parsed = parse(source);
        let parent_range = block_content_range(&parsed.syntax.blocks[0]);
        let edits = format_contained_blocks(source, parent_range).unwrap();
        assert_eq!(edits.len(), 1);
        let mut edited = source.to_string();
        edited.replace_range(edits[0].range.clone(), &edits[0].new_text);
        assert_eq!(format(&edited).unwrap(), edited);
        assert!(edited.ends_with("\n\n`event Following\n"));
    }

    #[test]
    fn contained_range_ignores_partial_empty_and_next_block_selections() {
        let source = "`-   One\n`- Two\n";
        let parsed = parse(source);
        let head = source.find("One").unwrap();
        assert!(format_contained_blocks(source, head..head + 3)
            .unwrap()
            .is_empty());
        assert!(format_contained_blocks(source, head..head)
            .unwrap()
            .is_empty());
        let second_start = parsed.syntax.blocks[1].range().start;
        let edits = format_contained_blocks(source, 0..second_start).unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "`- One");
    }

    #[test]
    fn contained_range_supports_anonymous_raw_and_paragraphs() {
        let source = "`\"\"\n  payload\n\nParagraph `\"\"\"[a ]\" b]\"\"\".\n\n`event Following\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let selection = block_content_range(&parsed.syntax.blocks[0]).start
            ..block_content_range(&parsed.syntax.blocks[1]).end;
        let edits = format_contained_blocks(source, selection).unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(
            edits[0].new_text,
            "`\"\n payload\n\nParagraph `\"\"[a ]\" b]\"\"."
        );
    }

    #[test]
    fn terminal_raw_descendants_do_not_accumulate_spacing() {
        let source = "`config\n   `json\n\n   \"\n    {\"enabled\": true}\n\n\n`event Following\n";
        let formatted = format(source).unwrap();
        assert_eq!(format(&formatted).unwrap(), formatted);
        assert_eq!(formatted.matches("\n\n`event Following").count(), 1);
    }
}
