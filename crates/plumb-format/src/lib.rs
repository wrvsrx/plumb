use std::ffi::OsString;
use std::fmt::Write;
use std::fs;
use std::io::{self, Read};
use std::ops::Range;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use plumb_core::{parse, AttrItem, Attributes, Block, Inline, InlineContent, ParsedBlock};
use unicode_width::UnicodeWidthStr;

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

const MAX_BLOCK_WIDTH: usize = 100;

#[derive(Debug, Parser)]
#[command(name = "plumb fmt", about = "Format plumb documents")]
struct Args {
    #[arg(long)]
    check: bool,
    paths: Vec<PathBuf>,
}

pub fn run_cli(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let args = match Args::try_parse_from(args) {
        Ok(args) => args,
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(error.exit_code() as u8);
        }
    };
    match run(args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(error) => {
            eprintln!("plumb fmt: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<bool, String> {
    if args.paths.is_empty() {
        let mut source = String::new();
        io::stdin()
            .read_to_string(&mut source)
            .map_err(|error| format!("cannot read stdin: {error}"))?;
        let formatted = format_source(&source, "stdin")?;
        if args.check {
            return Ok(source == formatted);
        }
        print!("{formatted}");
        return Ok(true);
    }

    let mut unchanged = true;
    for path in args.paths {
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let formatted = format_source(&source, &path.display().to_string())?;
        if source == formatted {
            continue;
        }
        unchanged = false;
        if args.check {
            eprintln!("would reformat {}", path.display());
        } else {
            fs::write(&path, formatted)
                .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        }
    }
    Ok(!args.check || unchanged)
}

fn format_source(source: &str, name: &str) -> Result<String, String> {
    format(source).map_err(|_| format!("{name} has syntax errors"))
}

pub fn format(source: &str) -> Result<String, FormatError> {
    let parsed = parse(source);
    if !parsed.is_valid() {
        return Err(FormatError::InvalidSyntax);
    }

    let mut formatter = Formatter::default();
    formatter.blocks(&parsed.syntax.blocks, 0);
    if terminal_verbatim(&parsed.syntax.blocks).is_none() && !formatter.output.is_empty() {
        formatter.output.push('\n');
    }
    Ok(formatter.output)
}

/// Formats complete sibling blocks covered by `range`. The following sibling
/// is used as read-only spacing context and is not itself reformatted.
pub fn format_block_range(source: &str, range: Range<usize>) -> Result<FormatEdit, FormatError> {
    let parsed = parse(source);
    if !parsed.is_valid() {
        return Err(FormatError::InvalidSyntax);
    }
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
    if !parsed.is_valid() {
        return Err(FormatError::InvalidSyntax);
    }
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
        .strip_prefix(&prefix)
        .unwrap_or(&formatter.output)
        .to_string();
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
                mark.range.end.max(block.head.range.end)
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

fn terminal_verbatim(blocks: &[Block]) -> Option<&plumb_core::VerbatimBlock> {
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
                if matches!(previous, Block::Verbatim(_)) {
                    if !self.output.ends_with('\n') {
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
                self.block_attributes(&block.attrs, indent + 1, 0);
                if !block.text.is_empty() {
                    self.output.push('\n');
                    let mut lines = block.text.split('\n').collect::<Vec<_>>();
                    if block.text.ends_with('\n') {
                        lines.pop();
                    }
                    for (index, line) in lines.iter().enumerate() {
                        if index > 0 {
                            self.output.push('\n');
                        }
                        if !line.is_empty() {
                            self.indent(indent + 2);
                            self.output.push_str(line);
                        }
                    }
                    if block.text.ends_with('\n') {
                        self.output.push('\n');
                    }
                }
            }
        }
    }

    fn parsed_block(&mut self, block: &ParsedBlock, indent: usize) {
        self.indent(indent);
        let continuation_indent = if let Some(mark) = &block.mark {
            self.output.push('`');
            self.output.push_str(&mark.marker);
            let hanging_indent = hanging_indent(indent, &mark.marker);
            let head_width = (!block.head.items.is_empty())
                .then(|| 1 + inline_first_line_width(&block.head))
                .unwrap_or(0);
            self.block_attributes(
                &mark.attrs,
                indent + 1 + UnicodeWidthStr::width(mark.marker.as_str()),
                head_width,
            );
            if !block.head.items.is_empty() {
                self.output.push(' ');
            }
            hanging_indent
        } else {
            indent
        };
        self.inlines(&block.head, continuation_indent, false);

        if !block.children.is_empty() {
            if block.head.items.is_empty() {
                self.output.push('\n');
            } else {
                self.output.push_str("\n\n");
            }
            let child_indent = block.mark.as_ref().map_or(indent, |mark| {
                if block.head.items.is_empty() {
                    indent + 1
                } else {
                    hanging_indent(indent, &mark.marker)
                }
            });
            self.blocks(&block.children, child_indent);
        }
    }

    fn inlines(&mut self, content: &InlineContent, continuation_indent: usize, nested: bool) {
        for inline in &content.items {
            match inline {
                Inline::Text { text, .. } => self.text(text, nested),
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
                    self.attributes(attrs);
                }
                Inline::Verbatim { text, attrs, .. } => {
                    let quotes = minimum_quote_count(text);
                    self.output.push('`');
                    for _ in 0..quotes {
                        self.output.push('"');
                    }
                    self.output.push('[');
                    self.output.push_str(text);
                    self.output.push(']');
                    for _ in 0..quotes {
                        self.output.push('"');
                    }
                    self.attributes(attrs);
                }
            }
        }
    }

    fn text(&mut self, text: &str, nested: bool) {
        for character in text.chars() {
            match character {
                '`' => self.output.push_str("``"),
                ']' if nested => self.output.push_str("`]"),
                _ => self.output.push(character),
            }
        }
    }

    fn attributes(&mut self, attrs: &Attributes) {
        let Some(attributes) = attributes_text(attrs) else {
            return;
        };
        self.output.push_str(&attributes);
    }

    fn block_attributes(&mut self, attrs: &Attributes, prefix_width: usize, suffix_width: usize) {
        let Some(attributes) = attributes_text(attrs) else {
            return;
        };
        if attrs.items.is_empty()
            || prefix_width + UnicodeWidthStr::width(attributes.as_str()) + suffix_width
                <= MAX_BLOCK_WIDTH
        {
            self.output.push_str(&attributes);
            return;
        }

        self.output.push('{');
        let item_indent = prefix_width + 1;
        let mut line_width = 0;
        for item in &attrs.items {
            let item = attribute_item_text(item);
            let item_width = UnicodeWidthStr::width(item.as_str());
            if line_width == 0 || line_width + 1 + item_width > MAX_BLOCK_WIDTH {
                self.output.push('\n');
                self.indent(item_indent);
                self.output.push_str(&item);
                line_width = item_indent + item_width;
            } else {
                self.output.push(' ');
                self.output.push_str(&item);
                line_width += 1 + item_width;
            }
        }
        self.output.push('\n');
        self.indent(prefix_width);
        self.output.push('}');
    }

    fn indent(&mut self, indent: usize) {
        self.output.extend(std::iter::repeat_n(' ', indent));
    }
}

fn attributes_text(attrs: &Attributes) -> Option<String> {
    attrs.range.as_ref()?;
    let mut output = String::from("{");
    for (index, item) in attrs.items.iter().enumerate() {
        if index > 0 {
            output.push(' ');
        }
        write_attribute_item(&mut output, item);
    }
    output.push('}');
    Some(output)
}

fn inline_first_line_width(content: &InlineContent) -> usize {
    let mut formatter = Formatter::default();
    formatter.inlines(content, 0, false);
    UnicodeWidthStr::width(formatter.output.lines().next().unwrap_or_default())
}

fn write_attribute_item(output: &mut String, item: &AttrItem) {
    match item {
        AttrItem::Id { value, .. } => {
            output.push('#');
            output.push_str(value);
        }
        AttrItem::Class { value, .. } => {
            output.push('.');
            output.push_str(value);
        }
        AttrItem::Pair { key, value, .. } => {
            let _ = write!(output, "{key}={}", value.raw);
        }
    }
}

fn attribute_item_text(item: &AttrItem) -> String {
    let mut output = String::new();
    write_attribute_item(&mut output, item);
    output
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

fn hanging_indent(owner_indent: usize, marker: &str) -> usize {
    owner_indent + 1 + UnicodeWidthStr::width(marker) + 1
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
            shape(&original.syntax.blocks),
            shape(&reparsed.syntax.blocks)
        );
        assert_eq!(format(&formatted).unwrap(), formatted);
    }

    fn shape(blocks: &[Block]) -> String {
        let mut output = String::new();
        shape_blocks(blocks, &mut output);
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
    }

    #[test]
    fn formats_blocks_attributes_and_indentation() {
        assert_formats(
            "`meta\n   `: title\n\n      Example\n\n`-{.task\n   #write\n   created=now\n   } Work\n",
            "`meta\n `: title\n\n    Example\n\n`-{.task #write created=now} Work\n",
        );
    }

    #[test]
    fn aligns_children_and_spaces_siblings_by_structure() {
        assert_formats(
            "`meta\n  `: title\n\n     this is a title\n  `: created\n\n     2026-07-20\n`- before\n\n`- something\n  `- aaa\n`- ssss\n\n`- jjjj\n",
            "`meta\n `: title\n\n    this is a title\n\n `: created\n\n    2026-07-20\n\n`- before\n`- something\n\n   `- aaa\n\n`- ssss\n`- jjjj\n",
        );
    }

    #[test]
    fn formats_a_complete_block_range_with_following_sibling_context() {
        let source =
            "`-{.task #old done=now} Work\n\n`-{.task #next} Work\n`# Following\n\nUnrelated\n";
        let parsed = parse(source);
        let first = parsed.syntax.blocks[0].range().clone();
        let second = parsed.syntax.blocks[1].range().clone();
        let edit = format_block_range(source, first.start..second.end).unwrap();

        assert_eq!(
            &source[edit.range.clone()],
            "`-{.task #old done=now} Work\n\n`-{.task #next} Work\n"
        );
        assert_eq!(
            edit.new_text,
            "`-{.task #old done=now} Work\n`-{.task #next} Work\n\n"
        );
        assert_eq!(&source[edit.range.end..], "`# Following\n\nUnrelated\n");
    }

    #[test]
    fn formats_a_range_that_contains_the_first_generated_block() {
        let source =
            "`meta\n `: title\n\n    empty\n\n `: created\n\n    2026-07-22T12:34:56+08:00\n\n";
        let edit = format_block_range(source, 0..source.len()).unwrap();
        assert_eq!(edit.range, 0..source.len() - 1);
        assert_eq!(edit.new_text, &source[..source.len() - 1]);
    }

    #[test]
    fn formats_a_nested_block_range_and_preserves_crlf() {
        let source = "`node Parent\r\n  `-{.task #old done=now} Work\r\n\r\n  `-{.task #next} Work\r\n  `note Following\r\n";
        let parsed = parse(source);
        let children = parsed.syntax.blocks[0].children();
        let edit =
            format_block_range(source, children[0].range().start..children[1].range().end).unwrap();

        assert_eq!(
            edit.new_text,
            "  `-{.task #old done=now} Work\r\n  `-{.task #next} Work\r\n\r\n"
        );
        assert_eq!(&source[edit.range.end..], "  `note Following\r\n");
    }

    #[test]
    fn nested_block_range_preserves_the_following_sibling_indent() {
        let source = "`node Parent\n   `-{.task #old} Old\n   `-{.task #next} Next\n";
        let parsed = parse(source);
        let first = &parsed.syntax.blocks[0].children()[0];
        let edit = format_block_range(source, first.range().clone()).unwrap();
        let mut edited = source.to_string();
        edited.replace_range(edit.range.clone(), &edit.new_text);

        assert_eq!(edited, source);
    }

    #[test]
    fn contained_range_formats_only_complete_maximal_blocks() {
        let source = "`node Parent\n       `-{.task\n          #one\n        } One\n\n       `-{.task #two} Two\n\n`# Following\n";
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
            "`-{.task #one} One\n       `-{.task #two} Two"
        );
        assert_eq!(&source[edits[0].range.end..], "\n\n`# Following\n");
        assert!(!edits[0].new_text.contains("`node Parent"));
    }

    #[test]
    fn contained_range_formats_a_complete_parent_subtree() {
        let source =
            "`node Parent\n       `-{.task\n          #one\n        } One\n\n`# Following\n";
        let parsed = parse(source);
        let parent_range = block_content_range(&parsed.syntax.blocks[0]);
        let edits = format_contained_blocks(source, parent_range).unwrap();
        assert_eq!(edits.len(), 1);

        let mut formatted = source.to_string();
        formatted.replace_range(edits[0].range.clone(), &edits[0].new_text);
        assert_eq!(
            formatted,
            "`node Parent\n\n      `-{.task #one} One\n\n`# Following\n"
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
        let source = "`node First\n  `-{.task\n      #one\n    } One\n`node Second\n  `-{.task\n      #two\n    } Two\n";
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
        assert!(edits[0].new_text.contains("`-{.task #one} One"));
        assert!(edits[1].new_text.contains("`-{.task #two} Two"));
    }

    #[test]
    fn contained_range_ignores_partial_and_empty_selections() {
        let source = "`-{.task #one} One\n";
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
        let source =
            "`node Parent\r\n  `-{.task\r\n      #one\r\n    } One\r\n\r\n`# Following\r\n";
        let parsed = parse(source);
        let child = &parsed.syntax.blocks[0].children()[0];
        let edits = format_contained_blocks(source, block_content_range(child)).unwrap();

        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "`-{.task #one} One");
        assert_eq!(&source[edits[0].range.end..], "\r\n\r\n`# Following\r\n");
    }

    #[test]
    fn contained_range_ending_at_the_next_block_excludes_it() {
        let source = "`-{.task\n    #one\n  } One\n`-{.task #two} Two\n";
        let parsed = parse(source);
        let second_start = parsed.syntax.blocks[1].range().start;
        let edits = format_contained_blocks(source, 0..second_start).unwrap();

        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "`-{.task #one} One");
        assert!(!edits[0].new_text.contains("Two"));
        assert_eq!(&source[edits[0].range.end..], "\n`-{.task #two} Two\n");
    }

    #[test]
    fn contained_range_supports_verbatim_blocks_and_paragraphs() {
        let source = "`{\n  language=text\n  source=test\n }\n  payload\n\nParagraph `\"\"\"[a ]\" b]\"\"\".\n\n`# Following\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let selection = block_content_range(&parsed.syntax.blocks[0]).start
            ..block_content_range(&parsed.syntax.blocks[1]).end;
        let edits = format_contained_blocks(source, selection).unwrap();

        assert_eq!(edits.len(), 1);
        assert_eq!(
            edits[0].new_text,
            "`{language=text source=test}\n  payload\n\nParagraph `\"\"[a ]\" b]\"\"."
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
    fn attributes_do_not_shift_the_conceptual_head_column() {
        assert_formats(
            "`-{.task #write created=now} Work\n  Details\n",
            "`-{.task #write created=now} Work\n   Details\n",
        );
    }

    #[test]
    fn packs_long_block_attributes_within_one_hundred_display_columns() {
        assert_formats(
            "`-{.task #write created=\"2026-07-20T12:00:00+08:00\" due=\"2026-07-21T12:00:00+08:00\" depends=\"notes/project.plumb#prepare\"} Work\n",
            "`-{\n   .task #write created=\"2026-07-20T12:00:00+08:00\" due=\"2026-07-21T12:00:00+08:00\"\n   depends=\"notes/project.plumb#prepare\"\n  } Work\n",
        );
        assert_formats(
            "`{language=text source=generated-with-a-deliberately-long-identifier-that-exceeds-the-limit-by-itself another=value}\n  payload\n",
            "`{\n  language=text\n  source=generated-with-a-deliberately-long-identifier-that-exceeds-the-limit-by-itself\n  another=value\n }\n  payload\n",
        );

        assert_formats(
            "`-{.task\r\n    #crlf\r\n  key=value} Work\r\n",
            "`-{.task #crlf key=value} Work\n",
        );

        let value = "界".repeat(45);
        assert_formats(
            &format!("`-{{.task label=\"{value}\"}} Work\n"),
            &format!("`-{{\n   .task\n   label=\"{value}\"\n  }} Work\n"),
        );
    }

    #[test]
    fn preserves_soft_breaks_and_inline_meaning() {
        assert_formats(
            "`note First `span[a `] b `` c]\n   second\n",
            "`note First `span[a `] b `` c]\n      second\n",
        );
    }

    #[test]
    fn chooses_the_minimum_safe_verbatim_delimiter() {
        assert_formats("Raw `\"\"\"[a ]\" b]\"\"\".\n", "Raw `\"\"[a ]\" b]\"\".\n");
    }

    #[test]
    fn preserves_verbatim_payload_and_its_final_newline() {
        assert_formats(
            "`{language=text}\n  a\nnext\n",
            "`{language=text}\n  a\nnext\n",
        );
        assert_formats(
            "`{language=text}\n    a\n\nnext\n",
            "`{language=text}\n    a\n\nnext\n",
        );
        assert_formats("`{}\n  final newline\n", "`{}\n  final newline\n");
        assert_formats("`{}\n  no newline", "`{}\n  no newline");
    }

    #[test]
    fn rejects_invalid_documents() {
        assert_eq!(format("`span[open\n"), Err(FormatError::InvalidSyntax));
    }
}
