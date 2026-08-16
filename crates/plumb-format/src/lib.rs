use std::collections::HashMap;
#[cfg(test)]
use std::fmt::Write;
use std::ops::Range;

#[cfg(test)]
use plumb_syntax::AttrItem;
use plumb_syntax::{
    parse, AttachedContent, Attributes, Block, Inline, InlineContent, ParsedBlock, ParsedDocument,
};
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
    if parsed.syntax.attrs.attached.is_some() {
        formatter.block_attached(&parsed.syntax.attrs, 0, true, true, None);
        if !parsed.syntax.blocks.is_empty() {
            formatter.output.push_str("\n\n");
        }
    }
    let body = &parsed.syntax.blocks;
    formatter.blocks(&body, 0);
    if terminal_verbatim(&body).is_none() && !formatter.output.is_empty() {
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
    } else if terminal_verbatim(selected).is_none() && !formatter.output.is_empty() {
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
                let attached_end = mark
                    .attrs
                    .attached
                    .as_deref()
                    .map_or(mark.range.end, |attached| attached.close_range.end);
                mark.range.end.max(block.head.range.end).max(attached_end)
            });
            let end = block
                .children
                .last()
                .map_or(own_end, |child| block_content_range(child).end.max(own_end));
            block.range.start..end
        }
        Block::Verbatim(block) => {
            let attributes_end = block
                .attrs
                .range
                .as_ref()
                .map_or(block.opener_range.end, |range| range.end);
            block.range.start..attributes_end.max(block.text_range.end)
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

fn terminal_verbatim(blocks: &[Block]) -> Option<&plumb_syntax::VerbatimBlock> {
    match blocks.last()? {
        Block::Verbatim(block) => Some(block),
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
                if terminal_verbatim(std::slice::from_ref(previous)).is_some() {
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
                self.output.push('`');
                self.output.push_str(&block.kind);
                self.output.push('"');
                match block.attrs.attached.as_deref().map(|group| &group.content) {
                    Some(AttachedContent::Inlines(_)) => {
                        self.output.push(' ');
                        self.inline_attributes(&block.attrs, indent + 1);
                    }
                    Some(AttachedContent::Blocks(_)) => {
                        self.block_attached(&block.attrs, indent, false, false, None);
                    }
                    None => {}
                }
                if !block.text.is_empty() {
                    self.output.push('\n');
                    let mut lines = block.text.split('\n').collect::<Vec<_>>();
                    let has_final_newline = block.text.ends_with('\n');
                    if has_final_newline {
                        lines.pop();
                    }
                    let last_content = lines.iter().rposition(|line| !line.is_empty());
                    for (index, line) in lines.iter().enumerate() {
                        if index > 0 {
                            self.output.push('\n');
                        }
                        if !line.is_empty() {
                            self.indent(indent + 1);
                            self.output.push_str(line);
                        } else if last_content.is_none_or(|last| index > last) {
                            self.indent(indent + 1);
                        }
                    }
                    if has_final_newline {
                        self.output.push('\n');
                    }
                }
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

        let compact_attached = block.mark.as_ref().is_some_and(|mark| {
            matches!(
                mark.attrs.attached.as_deref().map(|group| &group.content),
                Some(AttachedContent::Inlines(_))
            )
        });
        if compact_attached {
            self.output.push(' ');
            self.inline_attributes(
                &block.mark.as_ref().expect("marked block").attrs,
                continuation_indent,
            );
        }

        let has_attached = block
            .mark
            .as_ref()
            .is_some_and(|mark| mark.attrs.attached.is_some());
        // Canonical placement: the opener trails the header line while the
        // head fits on one line and moves to a continuation line once the
        // head wraps.
        let head_wrapped = block
            .head
            .items
            .iter()
            .any(|inline| matches!(inline, Inline::SoftBreak { .. }));
        if let Some(mark) = &block.mark {
            if mark.attrs.attached.is_some() && !compact_attached {
                self.block_attached(&mark.attrs, indent, false, head_wrapped, Some(indent + 1));
            }
        }

        if !block.children.is_empty() {
            let rendered_own_line = head_wrapped && has_attached && !compact_attached;
            if (block.head.items.is_empty() && !has_attached) || rendered_own_line {
                self.output.push('\n');
            } else {
                self.output.push_str("\n\n");
            }
            let child_indent = block.mark.as_ref().map_or(indent, |_| indent + 1);
            self.blocks(&block.children, child_indent);
        }
    }

    fn inlines(&mut self, content: &InlineContent, continuation_indent: usize, nested: bool) {
        for inline in &content.items {
            match inline {
                Inline::Text { text, .. } => self.text(text, nested),
                Inline::Space { text, .. } => self.output.push_str(text),
                Inline::SoftBreak { .. } => {
                    self.output.push('\n');
                    self.indent(continuation_indent);
                }
                Inline::Element {
                    kind,
                    content,
                    attrs,
                    ..
                } => {
                    self.output.push('`');
                    self.output.push_str(kind);
                    self.output.push('[');
                    self.inlines(content, continuation_indent, true);
                    self.output.push(']');
                    self.inline_attributes(attrs, continuation_indent);
                }
                Inline::Verbatim {
                    kind, text, attrs, ..
                } => {
                    self.output.push('`');
                    self.output.push_str(kind);
                    // A compact payload beginning with `[` would be reparsed
                    // as a bracket envelope. Keep the bracketed spelling so
                    // its leading bracket remains raw text.
                    if !text.contains('"') && !text.starts_with('[') {
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
                    self.inline_attributes(attrs, continuation_indent);
                }
            }
        }
    }

    fn text(&mut self, text: &str, _nested: bool) {
        // §2: bare delimiters never appear in text, so every literal one is
        // escaped unconditionally.
        for character in text.chars() {
            match character {
                '`' => self.output.push_str("``"),
                '[' => self.output.push_str("`["),
                ']' => self.output.push_str("`]"),
                '{' => self.output.push_str("`{"),
                '}' => self.output.push_str("`}"),
                _ => self.output.push(character),
            }
        }
    }

    fn inline_attributes(&mut self, attrs: &Attributes, continuation_indent: usize) {
        let Some(attached) = attrs.attached.as_deref() else {
            return;
        };
        self.output.push('{');
        if let AttachedContent::Inlines(content) = &attached.content {
            self.inlines(content, continuation_indent, true);
        }
        self.output.push('}');
    }

    /// Renders an expanded attached group. The canonical placement follows
    /// the head shape: the opener trails the header line while the head
    /// fits on one line, and occupies a continuation line once the head
    /// wraps. The document group always opens its own first structural
    /// line.
    fn block_attached(
        &mut self,
        attrs: &Attributes,
        indent: usize,
        document: bool,
        render_own_line: bool,
        next_line_indent: Option<usize>,
    ) {
        let Some(attached) = attrs.attached.as_deref() else {
            return;
        };
        let group_indent = if render_own_line && !document {
            let group_indent = next_line_indent.expect("marked next-line group has an indent");
            self.output.push('\n');
            self.indent(group_indent);
            group_indent
        } else if document {
            self.indent(indent);
            indent
        } else {
            self.output.push(' ');
            indent
        };
        self.output.push('{');
        let AttachedContent::Blocks(blocks) = &attached.content else {
            self.output.push_str("\n");
            self.indent(group_indent);
            self.output.push('}');
            return;
        };
        self.output.push('\n');
        for (index, block) in blocks.iter().enumerate() {
            if index > 0 {
                self.output.push('\n');
            }
            self.block(block, group_indent + 1);
        }
        if !self.output.ends_with('\n') {
            self.output.push('\n');
        }
        self.indent(group_indent);
        self.output.push('}');
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
    previous.children.is_empty() && previous_mark.marker == current_mark.marker
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
        assert!(original.is_valid());
        let formatted = format(source).unwrap();
        assert_eq!(formatted, expected);
        let reparsed = parse(&formatted);
        assert!(reparsed.is_valid());
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
                }
                Block::Verbatim(block) => {
                    output.push('V');
                    shape_attrs(&block.attrs, output);
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
                    content,
                    attrs,
                    ..
                } => {
                    let _ = write!(output, "E{kind:?}");
                    shape_inlines(content, output);
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
        if let Some(attached) = attrs.attached.as_deref() {
            output.push('<');
            match &attached.content {
                AttachedContent::Blocks(blocks) => shape_blocks(blocks, output),
                AttachedContent::Inlines(content) => shape_inlines(content, output),
            }
            output.push('>');
        }
    }

    #[test]
    fn formats_recursive_attached_groups() {
        assert_formats(
            "{\n  `:   title Document title\n\n  `: tags plumb\n}\n\n`-   Buy milk {\n  `-   task\n  `@   shopping\n}\n\n   Details.\n",
            "{\n `: title Document title\n `: tags plumb\n}\n\n`- Buy milk {\n `- task\n `@ shopping\n}\n\n Details.\n",
        );
        assert_formats("{\n}\n", "{\n}\n");
        assert_formats("`\"\"\n  payload\n", "`\"\n payload\n");
        assert_formats(
            "See `->[guide]{`@[main] `-[external] `:[to guide.plumb]}.\n",
            "See `->[guide]{`@[main] `-[external] `:[to guide.plumb]}.\n",
        );
        assert_formats(
            "`task Work\n   {\n     `:   created now\n   }\n\n   Details\n",
            "`task Work {\n `: created now\n}\n\n Details\n",
        );
    }

    #[test]
    fn canonical_opener_placement_follows_the_head_shape() {
        // A single-line head canonicalizes to the trailing opener.
        assert_formats(
            "`task Work\n {\n  `: created now\n }\n",
            "`task Work {\n `: created now\n}\n",
        );
        // A wrapped head keeps the own-line opener and aligns the close and
        // children with it.
        assert_formats(
            "`task Buy milk\n and eggs\n {\n  `: created now\n }\n Details\n",
            "`task Buy milk\n and eggs\n {\n  `: created now\n }\n Details\n",
        );
        // A trailing opener on a continuation line canonicalizes to the
        // own-line placement.
        assert_formats(
            "`note first\n second {\n  `- cited\n }\n",
            "`note first\n second\n {\n  `- cited\n }\n",
        );
        // Compact groups are unaffected.
        assert_formats(
            "`note first\n second {`@[x]}\n",
            "`note first\n second {`@[x]}\n",
        );
    }

    #[test]
    fn preserves_markers_and_opaque_attached_spellings() {
        // §2: literal delimiters stay escaped in rendered text.
        assert_formats(
            "escaped `[ `] `{ `} delims\n",
            "escaped `[ `] `{ `} delims\n",
        );
        assert_formats("`- Work {`-[task]}\n", "`- Work {`-[task]}\n");
        assert_formats("`node Meeting {`-[event]}\n", "`node Meeting {`-[event]}\n");
        assert_formats(
            "`task Work\n`event Meeting\n",
            "`task Work\n\n`event Meeting\n",
        );
    }

    #[test]
    fn formats_blocks_attributes_and_indentation() {
        assert_formats(
            "`node\n   `: title Example\n\n`- Work {\n  `- task\n  `@ write\n  `: created now\n}\n",
            "`node\n `: title Example\n\n`- Work {\n `- task\n `@ write\n `: created now\n}\n",
        );
    }

    #[test]
    fn whole_document_edits_preserve_a_task_before_a_repeated_marker() {
        let source = "`- Before {`-[task] `:[created one]}\n`- 实现 task snippet 的时候有问题 aaa aaa aaa aaa aaa aaa aaa {`-[task] `:[created 2026-08-05T03:25:50+08:00]}\n`- task fold 的时候没包含最后一行 {\n  `- task\n  `: created 2026-08-05T03:26:23+08:00\n  `: done 2026-08-05T04:03:22+08:00\n}\n`- state 默认显示 ready 跟 blocked {`-[task] `:[created 2026-08-05T03:43:34+08:00] `:[done 2026-08-05T04:32:23+08:00]}\n";
        let canonical = format(source).unwrap();
        let edits = format_edits(source).unwrap();
        let mut edited = source.to_string();
        for edit in edits.iter().rev() {
            edited.replace_range(edit.range.clone(), &edit.new_text);
        }

        assert_eq!(edited, canonical);
        assert!(edited.contains("`:[created 2026-08-05T03:25:50+08:00]"));
        assert!(edited.contains("aaa aaa aaa"));
        assert!(parse(&edited).is_valid());
    }

    #[test]
    fn whole_document_edits_anchor_large_repeated_block_layout() {
        let mut source = String::new();
        for index in 0..512 {
            let _ = write!(source, "`event Event {index} {{\n  `: uid repeated\n}}\n\n");
        }
        let canonical = format(&source).unwrap();
        let edits = format_edits(&source).unwrap();

        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].range.start, source.find('\n').unwrap() + 1);
        let mut edited = source;
        edited.replace_range(edits[0].range.clone(), &edits[0].new_text);
        assert_eq!(edited, canonical);
        assert!(format_edits(&edited).unwrap().is_empty());
    }

    #[test]
    fn aligns_children_and_spaces_siblings_by_structure() {
        assert_formats(
            "`meta\n  `: title\n\n     this is a title\n  `: created\n\n     2026-07-20\n`- before\n\n`- something\n  `- aaa\n`- ssss\n\n`- jjjj\n",
            "`meta\n `: title\n\n  this is a title\n\n `: created\n\n  2026-07-20\n\n`- before\n`- something\n\n `- aaa\n\n`- ssss\n`- jjjj\n",
        );
    }

    #[test]
    fn formats_a_complete_block_range_with_following_sibling_context() {
        let source =
            "`- Work {`-[task] `@[old] `:[done now]}\n\n`- Work {`-[task] `@[next]}\n`# Following\n\nUnrelated\n";
        let parsed = parse(source);
        let first = parsed.syntax.blocks[0].range().clone();
        let second = parsed.syntax.blocks[1].range().clone();
        let edit = format_block_range(source, first.start..second.end).unwrap();

        assert_eq!(
            &source[edit.range.clone()],
            "`- Work {`-[task] `@[old] `:[done now]}\n\n`- Work {`-[task] `@[next]}\n"
        );
        assert_eq!(
            edit.new_text,
            "`- Work {`-[task] `@[old] `:[done now]}\n`- Work {`-[task] `@[next]}\n\n"
        );
        assert_eq!(&source[edit.range.end..], "`# Following\n\nUnrelated\n");
    }

    #[test]
    fn formats_a_range_that_contains_the_first_generated_block() {
        let source =
            "`meta\n `: title\n\n  empty\n\n `: created\n\n  2026-07-22T12:34:56+08:00\n\n";
        let edit = format_block_range(source, 0..source.len()).unwrap();
        assert_eq!(edit.range, 0..source.len() - 1);
        assert_eq!(edit.new_text, &source[..source.len() - 1]);
    }

    #[test]
    fn formats_a_nested_block_range_and_preserves_crlf() {
        let source = "`node Parent\r\n  `- Work {`-[task] `@[old] `:[done now]}\r\n\r\n  `- Work {`-[task] `@[next]}\r\n  `note Following\r\n";
        let parsed = parse(source);
        let children = parsed.syntax.blocks[0].children();
        let edit =
            format_block_range(source, children[0].range().start..children[1].range().end).unwrap();

        assert_eq!(
            edit.new_text,
            "  `- Work {`-[task] `@[old] `:[done now]}\r\n  `- Work {`-[task] `@[next]}\r\n\r\n"
        );
        assert_eq!(&source[edit.range.end..], "  `note Following\r\n");
    }

    #[test]
    fn nested_block_range_preserves_the_following_sibling_indent() {
        let source = "`node Parent\n   `- Old {`-[task] `@[old]}\n   `- Next {`-[task] `@[next]}\n";
        let parsed = parse(source);
        let first = &parsed.syntax.blocks[0].children()[0];
        let edit = format_block_range(source, first.range().clone()).unwrap();
        let mut edited = source.to_string();
        edited.replace_range(edit.range.clone(), &edit.new_text);

        assert_eq!(edited, source);
    }

    #[test]
    fn contained_range_formats_only_complete_maximal_blocks() {
        let source = "`node Parent\n       `- One {\n         `- task\n         `@ one\n       }\n\n       `- Two {`-[task] `@[two]}\n\n`# Following\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let parent = &parsed.syntax.blocks[0];
        let children = parent.children();
        let selection =
            block_content_range(&children[0]).start..block_content_range(&children[1]).end;
        let edits = format_contained_blocks(source, selection).unwrap();

        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].range.start, children[0].range().start);
        assert_eq!(edits[0].range.end, block_content_range(&children[1]).end);
        assert_eq!(
            edits[0].new_text,
            "`- One {\n `- task\n `@ one\n}\n`- Two {`-[task] `@[two]}"
        );
        assert_eq!(&source[edits[0].range.end..], "\n\n`# Following\n");
        assert!(!edits[0].new_text.contains("`node Parent"));
    }

    #[test]
    fn contained_range_formats_a_complete_parent_subtree() {
        let source =
            "`node Parent\n       `- One {\n         `- task\n         `@ one\n       }\n\n`# Following\n";
        let parsed = parse(source);
        let parent_range = block_content_range(&parsed.syntax.blocks[0]);
        let edits = format_contained_blocks(source, parent_range).unwrap();
        assert_eq!(edits.len(), 1);

        let mut formatted = source.to_string();
        formatted.replace_range(edits[0].range.clone(), &edits[0].new_text);
        assert_eq!(
            formatted,
            "`node Parent\n\n `- One {\n  `- task\n  `@ one\n }\n\n`# Following\n"
        );
        assert_eq!(format(&formatted).unwrap(), formatted);
        let reparsed = parse(&formatted);
        assert!(format_contained_blocks(
            &formatted,
            block_content_range(&reparsed.syntax.blocks[0]),
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn contained_range_returns_non_overlapping_maximal_groups() {
        let source = "`node First\n  `-   One {`-[task] `@[one]}\n`node   Second\n  `- Two {`-[task] `@[two]}\n";
        let parsed = parse(source);
        let first_child = &parsed.syntax.blocks[0].children()[0];
        let second_parent = &parsed.syntax.blocks[1];
        let selection =
            block_content_range(first_child).start..block_content_range(second_parent).end;
        let edits = format_contained_blocks(source, selection).unwrap();

        assert_eq!(edits.len(), 2);
        assert!(edits[0].range.end <= edits[1].range.start);
        assert!(!edits[0].new_text.contains("`node First"));
        assert!(edits[1].new_text.starts_with("`node Second"));
        assert!(edits[0].new_text.contains("`- One {`-[task] `@[one]}"));
        assert!(edits[1].new_text.contains("`- Two {`-[task] `@[two]}"));
    }

    #[test]
    fn contained_range_ignores_partial_and_empty_selections() {
        let source = "`- One {`-[task] `@[one]}\n";
        let head = source.find("One").unwrap();
        assert!(format_contained_blocks(source, head..head + 3)
            .unwrap()
            .is_empty());
        assert!(format_contained_blocks(source, head..head)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn contained_range_preserves_crlf_and_external_layout() {
        let source = "`node Parent\r\n  `-   One {`-[task] `@[one]}\r\n\r\n`# Following\r\n";
        let parsed = parse(source);
        let child = &parsed.syntax.blocks[0].children()[0];
        let edits = format_contained_blocks(source, block_content_range(child)).unwrap();

        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "`- One {`-[task] `@[one]}");
        assert_eq!(&source[edits[0].range.end..], "\r\n\r\n`# Following\r\n");
    }

    #[test]
    fn contained_range_ending_at_the_next_block_excludes_it() {
        let source = "`-   One {`-[task] `@[one]}\n`- Two {`-[task] `@[two]}\n";
        let parsed = parse(source);
        let second_start = parsed.syntax.blocks[1].range().start;
        let edits = format_contained_blocks(source, 0..second_start).unwrap();

        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "`- One {`-[task] `@[one]}");
        assert!(!edits[0].new_text.contains("Two"));
        assert_eq!(
            &source[edits[0].range.end..],
            "\n`- Two {`-[task] `@[two]}\n"
        );
    }

    #[test]
    fn contained_range_supports_verbatim_blocks_and_paragraphs() {
        let source = "`text\"\" {`:[source test]}\n  payload\n\nParagraph `\"\"\"[a ]\" b]\"\"\".\n\n`# Following\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let selection = block_content_range(&parsed.syntax.blocks[0]).start
            ..block_content_range(&parsed.syntax.blocks[1]).end;
        let edits = format_contained_blocks(source, selection).unwrap();

        assert_eq!(edits.len(), 1);
        assert_eq!(
            edits[0].new_text,
            "`text\" {`:[source test]}\n payload\n\nParagraph `\"\"[a ]\" b]\"\"."
        );
        assert_eq!(&source[edits[0].range.end..], "\n\n`# Following\n");
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
    fn attributes_do_not_shift_the_fixed_body_column() {
        assert_formats(
            "`- Work {`-[task] `@[write] `:[created now]}\n  `note Details\n",
            "`- Work {`-[task] `@[write] `:[created now]}\n\n `note Details\n",
        );
    }

    #[test]
    fn preserves_long_attached_groups() {
        assert_formats(
            "`- Work {`-[task] `@[write] `:[created 2026-07-20T12:00:00+08:00] `:[due 2026-07-21T12:00:00+08:00] `:[depends notes/project.plumb#prepare]}\n",
            "`- Work {`-[task] `@[write] `:[created 2026-07-20T12:00:00+08:00] `:[due 2026-07-21T12:00:00+08:00] `:[depends notes/project.plumb#prepare]}\n",
        );
        assert_formats(
            "`text\"\" {`:[source generated-with-a-deliberately-long-identifier-that-exceeds-the-limit-by-itself] `:[another value]}\n  payload\n",
            "`text\" {`:[source generated-with-a-deliberately-long-identifier-that-exceeds-the-limit-by-itself] `:[another value]}\n payload\n",
        );

        assert_formats(
            "`- Work {`-[task] `@[crlf] `:[key value]}\r\n",
            "`- Work {`-[task] `@[crlf] `:[key value]}\n",
        );

        let value = "界".repeat(45);
        assert_formats(
            &format!("`- Work {{`-[task] `:[label {value}]}}\n"),
            &format!("`- Work {{`-[task] `:[label {value}]}}\n"),
        );
    }

    #[test]
    fn preserves_soft_breaks_and_inline_meaning() {
        assert_formats(
            "`note First `span[a `] b `` c]\n   second\n",
            "`note First `span[a `] b `` c]\n second\n",
        );
    }

    #[test]
    fn chooses_the_minimum_safe_verbatim_delimiter() {
        assert_formats("Raw `\"\"\"[a ]\" b]\"\"\".\n", "Raw `\"\"[a ]\" b]\"\".\n");
    }

    #[test]
    fn bracketed_verbatim_formatting_is_idempotent() {
        let source = "`\"[[]]\"\n";
        let formatted = format(source).unwrap();
        assert_eq!(formatted, source);
        assert_eq!(format(&formatted).unwrap(), formatted);
    }

    #[test]
    fn preserves_verbatim_payload_and_its_final_newline() {
        assert_formats("`text\"\"\n  a\nnext\n", "`text\"\n a\n\nnext\n");
        assert_formats(
            "`text\"\"\n    a\n  \n\nnext\n",
            "`text\"\n   a\n \n\nnext\n",
        );
        assert_formats("`\"\"\n  final newline\n", "`\"\n final newline\n");
        assert_formats("`\"\"\n  no newline", "`\"\n no newline");
    }

    #[test]
    fn terminal_verbatim_descendants_do_not_accumulate_sibling_spacing() {
        let source = "`. config\n\n   `json\"\n     {\"enabled\": true}\n\n\n`# Following\n";
        let formatted = format(source).unwrap();
        assert_eq!(format(&formatted).unwrap(), formatted);
        assert_eq!(formatted.matches("\n\n`# Following").count(), 1);
    }

    #[test]
    fn rejects_invalid_documents() {
        assert_eq!(format("`span[open\n"), Err(FormatError::InvalidSyntax));
    }
}
