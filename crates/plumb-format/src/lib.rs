use std::ffi::OsString;
use std::fmt::Write;
use std::fs;
use std::io::{self, Read};
use std::ops::Range;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use plumb_core::{
    parse, parse_legacy_017, AttachedContent, AttachedGroup, AttrItem, Attributes, Block, Inline,
    InlineContent, ParsedBlock,
};
use similar::{DiffOp, TextDiff};
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

#[derive(Debug, Parser)]
#[command(
    name = "plumb migrate-attributes",
    about = "Convert legacy attribute slots to attached groups"
)]
struct MigrateArgs {
    path: Option<PathBuf>,
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

pub fn run_migrate_cli(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let args = match MigrateArgs::try_parse_from(args) {
        Ok(args) => args,
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(error.exit_code() as u8);
        }
    };
    let source = match args.path {
        Some(path) => match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) => {
                eprintln!(
                    "plumb migrate-attributes: cannot read {}: {error}",
                    path.display()
                );
                return ExitCode::FAILURE;
            }
        },
        None => {
            let mut source = String::new();
            if let Err(error) = io::stdin().read_to_string(&mut source) {
                eprintln!("plumb migrate-attributes: cannot read stdin: {error}");
                return ExitCode::FAILURE;
            }
            source
        }
    };
    match migrate_attributes(&source) {
        Ok(migrated) => {
            print!("{migrated}");
            ExitCode::SUCCESS
        }
        Err(_) => {
            eprintln!("plumb migrate-attributes: input has syntax errors");
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
    render(source, false)
}

pub fn migrate_attributes(source: &str) -> Result<String, FormatError> {
    render(source, true)
}

fn render(source: &str, migrate_legacy: bool) -> Result<String, FormatError> {
    let parsed = if migrate_legacy {
        parse_legacy_017(source)
    } else {
        parse(source)
    };
    if !parsed.is_valid() {
        return Err(FormatError::InvalidSyntax);
    }

    let mut formatter = Formatter {
        output: String::new(),
        migrate_legacy,
    };
    let legacy_metadata = (migrate_legacy && parsed.syntax.attrs.attached.is_none())
        .then(|| {
            parsed
                .syntax
                .blocks
                .iter()
                .position(convertible_legacy_metadata)
        })
        .flatten();
    let migrated_metadata = legacy_metadata.map(|index| Attributes {
        range: Some(0..0),
        items: Vec::new(),
        attached: Some(Box::new(AttachedGroup {
            range: 0..0,
            open_range: 0..0,
            close_range: 0..0,
            content: AttachedContent::Blocks(convert_legacy_metadata_blocks(
                &parsed.syntax.blocks[index].children()[..],
            )),
        })),
    });
    if parsed.syntax.attrs.attached.is_some() {
        formatter.block_attached(&parsed.syntax.attrs, 0, false);
        if !parsed.syntax.blocks.is_empty() {
            formatter.output.push_str("\n\n");
        }
    } else if let Some(metadata) = migrated_metadata.as_ref() {
        formatter.block_attached(metadata, 0, false);
        if parsed.syntax.blocks.len() > 1 {
            formatter.output.push_str("\n\n");
        }
    }
    let body = legacy_metadata.map_or_else(
        || parsed.syntax.blocks.clone(),
        |metadata| {
            parsed
                .syntax
                .blocks
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != metadata)
                .map(|(_, block)| block.clone())
                .collect()
        },
    );
    formatter.blocks(&body, 0);
    if terminal_verbatim(&body).is_none() && !formatter.output.is_empty() {
        formatter.output.push('\n');
    }
    Ok(formatter.output)
}

fn convertible_legacy_metadata(block: &Block) -> bool {
    let Block::Parsed(block) = block else {
        return false;
    };
    block
        .mark
        .as_ref()
        .is_some_and(|mark| mark.marker == "meta")
        && block.head.items.is_empty()
        && legacy_metadata_keys_are_convertible(&block.children)
}

fn legacy_metadata_keys_are_convertible(blocks: &[Block]) -> bool {
    blocks.iter().all(|block| match block {
        Block::Verbatim(_) => true,
        Block::Parsed(block) => {
            let valid = block
                .mark
                .as_ref()
                .is_none_or(|mark| mark.marker != ":" || valid_marker(&block.head.plain_text()));
            valid && legacy_metadata_keys_are_convertible(&block.children)
        }
    })
}

fn valid_marker(marker: &str) -> bool {
    !marker.is_empty()
        && marker.chars().all(|character| {
            !character.is_whitespace()
                && !character.is_control()
                && !matches!(character, '`' | '"' | '[' | ']' | '{' | '}')
        })
}

fn convert_legacy_metadata_blocks(blocks: &[Block]) -> Vec<Block> {
    blocks
        .iter()
        .cloned()
        .map(|block| match block {
            Block::Verbatim(block) => Block::Verbatim(block),
            Block::Parsed(mut block) => {
                block.children = convert_legacy_metadata_blocks(&block.children);
                if block.mark.as_ref().is_some_and(|mark| mark.marker == ":") {
                    let scalar = match block.children.as_slice() {
                        [Block::Parsed(value)] if value.mark.is_none() => Some(value.head.clone()),
                        _ => None,
                    };
                    if let Some(mut scalar) = scalar {
                        let mut items = std::mem::take(&mut block.head.items);
                        let separator = block.head.range.end..block.head.range.end;
                        items.push(Inline::Space {
                            text: " ".to_string(),
                            range: separator,
                        });
                        items.append(&mut scalar.items);
                        block.head.items = items;
                        block.head.range.end = scalar.range.end;
                        block.children.clear();
                    }
                }
                Block::Parsed(block)
            }
        })
        .collect()
}

pub fn format_edits(source: &str) -> Result<Vec<FormatEdit>, FormatError> {
    let formatted = format(source)?;
    if formatted == source {
        return Ok(Vec::new());
    }

    let source_offsets = line_offsets(source);
    let formatted_offsets = line_offsets(&formatted);
    let diff = TextDiff::from_lines(source, &formatted);
    let operations = diff.ops();
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

fn terminal_verbatim(blocks: &[Block]) -> Option<&plumb_core::VerbatimBlock> {
    match blocks.last()? {
        Block::Verbatim(block) => Some(block),
        Block::Parsed(block) => terminal_verbatim(&block.children),
    }
}

#[derive(Default)]
struct Formatter {
    output: String,
    migrate_legacy: bool,
}

impl Formatter {
    fn blocks(&mut self, blocks: &[Block], indent: usize) {
        for (index, block) in blocks.iter().enumerate() {
            if index > 0 {
                let previous = &blocks[index - 1];
                if terminal_verbatim(std::slice::from_ref(previous)).is_some() {
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
                let promoted_language = block
                    .kind
                    .is_empty()
                    .then(|| block.attrs.value("language"))
                    .flatten();
                self.output
                    .push_str(promoted_language.unwrap_or(block.kind.as_str()));
                self.output.push('"');
                if matches!(
                    block.attrs.attached.as_deref().map(|group| &group.content),
                    Some(AttachedContent::Inlines(_))
                ) {
                    self.inline_attributes(&block.attrs, indent + 2);
                } else if !block.attrs.items.is_empty() {
                    self.projected_inline_attached(&block.attrs, promoted_language.is_some(), None);
                }
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
            let head_width = if block.head.items.is_empty() {
                0
            } else {
                1 + inline_first_line_width(&block.head)
            };
            if mark.attrs.attached.is_none() && !(self.migrate_legacy && mark.attrs.range.is_some())
            {
                self.block_attributes(
                    &mark.attrs,
                    indent + 1 + UnicodeWidthStr::width(mark.marker.as_str()),
                    head_width,
                );
            }
            if !block.head.items.is_empty() {
                self.output.push(' ');
            }
            hanging_indent
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

        let has_attached = block.mark.as_ref().is_some_and(|mark| {
            mark.attrs.attached.is_some() || (self.migrate_legacy && mark.attrs.range.is_some())
        });
        if let Some(mark) = &block.mark {
            if mark.attrs.attached.is_some() && !compact_attached {
                self.output.push('\n');
                self.block_attached(&mark.attrs, continuation_indent, false);
            } else if self.migrate_legacy && mark.attrs.range.is_some() {
                self.output.push('\n');
                self.legacy_block_attached(&mark.attrs, continuation_indent, false);
            }
        }

        if !block.children.is_empty() {
            if block.head.items.is_empty() && !has_attached {
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
                    let promoted_kind = self
                        .migrate_legacy
                        .then(|| {
                            attrs.items.iter().find_map(|item| match item {
                                AttrItem::Class { value, .. }
                                    if matches!(value.as_str(), "->" | "$") =>
                                {
                                    Some(value.as_str())
                                }
                                _ => None,
                            })
                        })
                        .flatten();
                    self.output.push('`');
                    self.output.push_str(promoted_kind.unwrap_or(kind));
                    if !text.contains('"') {
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
                    if let Some(promoted) = promoted_kind {
                        self.inline_attributes_skipping_class(attrs, continuation_indent, promoted);
                    } else {
                        self.inline_attributes(attrs, continuation_indent);
                    }
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

    fn inline_attributes(&mut self, attrs: &Attributes, continuation_indent: usize) {
        let Some(attached) = attrs.attached.as_deref() else {
            if self.migrate_legacy && attrs.range.is_some() {
                self.legacy_inline_attached(attrs);
            } else {
                self.attributes(attrs);
            }
            return;
        };
        self.output.push('{');
        if let AttachedContent::Inlines(content) = &attached.content {
            self.inlines(content, continuation_indent, true);
        }
        self.output.push('}');
    }

    fn inline_attributes_skipping_class(
        &mut self,
        attrs: &Attributes,
        continuation_indent: usize,
        skipped: &str,
    ) {
        if attrs.attached.is_none() {
            self.projected_inline_attached(attrs, false, Some(skipped));
            return;
        }
        let retained = attrs
            .items
            .iter()
            .filter(|item| !matches!(item, AttrItem::Class { value, .. } if value == skipped))
            .count();
        if retained == 0 {
            return;
        }
        self.output.push('{');
        let mut wrote = false;
        for item in &attrs.items {
            if matches!(item, AttrItem::Class { value, .. } if value == skipped) {
                continue;
            }
            if wrote {
                self.output.push(' ');
            }
            wrote = true;
            self.write_projected_inline_item(item);
        }
        self.output.push('}');
        let _ = continuation_indent;
    }

    fn block_attached(&mut self, attrs: &Attributes, indent: usize, opener_after_tick: bool) {
        let Some(attached) = attrs.attached.as_deref() else {
            return;
        };
        if !opener_after_tick {
            self.indent(indent);
        }
        self.output.push('{');
        let AttachedContent::Blocks(blocks) = &attached.content else {
            self.output.push_str("\n");
            self.indent(indent);
            self.output.push('}');
            return;
        };
        self.output.push('\n');
        for (index, block) in blocks.iter().enumerate() {
            if index > 0 {
                self.output.push('\n');
            }
            self.block(block, indent + 2);
        }
        if !self.output.ends_with('\n') {
            self.output.push('\n');
        }
        self.indent(indent);
        self.output.push('}');
    }

    fn legacy_block_attached(
        &mut self,
        attrs: &Attributes,
        indent: usize,
        opener_after_tick: bool,
    ) {
        if !opener_after_tick {
            self.indent(indent);
        }
        self.output.push_str("{\n");
        for item in &attrs.items {
            self.indent(indent + 2);
            self.output.push('`');
            match item {
                AttrItem::Class { value, .. } => {
                    self.output.push_str("- ");
                    self.text(value, false);
                }
                AttrItem::Id { value, .. } => {
                    self.output.push_str("@ ");
                    self.text(value, false);
                }
                AttrItem::Pair { key, value, .. } => {
                    self.output.push_str(": ");
                    self.text(key, false);
                    self.output.push(' ');
                    self.text(&value.decoded, false);
                }
            }
            self.output.push('\n');
        }
        self.indent(indent);
        self.output.push('}');
    }

    fn legacy_inline_attached(&mut self, attrs: &Attributes) {
        self.projected_inline_attached(attrs, false, None);
    }

    fn projected_inline_attached(
        &mut self,
        attrs: &Attributes,
        skip_language: bool,
        skip_class: Option<&str>,
    ) {
        if attrs.items.iter().all(|item| {
            (skip_language && matches!(item, AttrItem::Pair { key, .. } if key == "language"))
                || matches!(item, AttrItem::Class { value, .. } if skip_class == Some(value))
        }) {
            return;
        }
        self.output.push('{');
        let mut wrote_item = false;
        for item in &attrs.items {
            if skip_language && matches!(item, AttrItem::Pair { key, .. } if key == "language") {
                continue;
            }
            if matches!(item, AttrItem::Class { value, .. } if skip_class == Some(value)) {
                continue;
            }
            if wrote_item {
                self.output.push(' ');
            }
            wrote_item = true;
            self.write_projected_inline_item(item);
        }
        self.output.push('}');
    }

    fn write_projected_inline_item(&mut self, item: &AttrItem) {
        self.output.push('`');
        match item {
            AttrItem::Class { value, .. } => {
                self.output.push_str("-[");
                self.text(value, true);
                self.output.push(']');
            }
            AttrItem::Id { value, .. } => {
                self.output.push_str("@[");
                self.text(value, true);
                self.output.push(']');
            }
            AttrItem::Pair { key, value, .. } => {
                self.output.push_str(":[");
                self.text(key, true);
                self.output.push(' ');
                self.text(&value.decoded, true);
                self.output.push(']');
            }
        }
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
            shape_document(&original.syntax),
            shape_document(&reparsed.syntax)
        );
        assert_eq!(format(&formatted).unwrap(), formatted);
    }

    fn shape_document(document: &plumb_core::Document) -> String {
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
            "{\n  `:   title Document title\n\n  `: tags plumb\n}\n\n`-   Buy milk\n   {\n     `-   task\n     `@   shopping\n   }\n\n   Details.\n",
            "{\n  `: title Document title\n  `: tags plumb\n}\n\n`- Buy milk\n   {\n     `- task\n     `@ shopping\n   }\n\n   Details.\n",
        );
        assert_formats("{\n}\n", "{\n}\n");
        assert_formats("`\"\n  payload\n", "`\"\n  payload\n");
        assert_formats(
            "See `->[guide]{`@[main] `-[external] `:[to guide.plumb]}.\n",
            "See `->[guide]{`@[main] `-[external] `:[to guide.plumb]}.\n",
        );
    }

    #[test]
    fn migrates_legacy_attributes_for_every_owner_category() {
        let source = "`-{.task #shopping due=\"2026-08-07\"} Buy milk\n\nSee `->[guide]{.external #main to=\"guide.plumb#intro\"}.\n\n`{language=rust #example}\n  fn main() {}\n";
        let expected = "`- Buy milk\n   {\n     `- task\n     `@ shopping\n     `: due 2026-08-07\n   }\n\nSee `->[guide]{`-[external] `@[main] `:[to guide.plumb#intro]}.\n\n`rust\"{`@[example]}\n  fn main() {}\n";

        let migrated = migrate_attributes(source).unwrap();
        assert_eq!(migrated, expected);
        assert!(parse(&migrated).is_valid());
        assert_eq!(migrate_attributes(&migrated).unwrap(), migrated);
        assert_eq!(format(source), Err(FormatError::InvalidSyntax));
    }

    #[test]
    fn migration_rejects_invalid_input_without_partial_output() {
        assert_eq!(
            migrate_attributes("`span[unclosed\n"),
            Err(FormatError::InvalidSyntax)
        );
    }

    #[test]
    fn migration_lifts_legacy_metadata_to_the_document_owner() {
        let source = "`meta\n `: title\n\n    Document `em[title]\n\n `: tags\n  `- plumb\n  `- parser\n\n `: author\n  `: name\n\n     Alice\n\nBody.\n";
        let expected = "{\n  `: title Document `em[title]\n  `: tags\n\n     `- plumb\n     `- parser\n  `: author\n\n     `: name Alice\n}\n\nBody.\n";

        let migrated = migrate_attributes(source).unwrap();
        assert_eq!(migrated, expected);
        assert!(parse(&migrated).is_valid());
        assert_eq!(migrate_attributes(&migrated).unwrap(), migrated);
    }

    #[test]
    fn formats_blocks_attributes_and_indentation() {
        assert_formats(
            "`node\n   `: title Example\n\n`- Work\n   {\n     `- task\n     `@ write\n     `: created now\n   }\n",
            "`node\n `: title Example\n\n`- Work\n   {\n     `- task\n     `@ write\n     `: created now\n   }\n",
        );
    }

    #[test]
    fn whole_document_edits_preserve_a_task_before_a_repeated_marker() {
        let source = "`- Before {`-[task] `:[created one]}\n`- 实现 task snippet 的时候有问题 aaa aaa aaa aaa aaa aaa aaa {`-[task] `:[created 2026-08-05T03:25:50+08:00]}\n`- task fold 的时候没包含最后一行\n   {\n     `- task\n     `: created 2026-08-05T03:26:23+08:00\n     `: done 2026-08-05T04:03:22+08:00\n   }\n`- state 默认显示 ready 跟 blocked {`-[task] `:[created 2026-08-05T03:43:34+08:00] `:[done 2026-08-05T04:32:23+08:00]}\n";
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
    fn aligns_children_and_spaces_siblings_by_structure() {
        assert_formats(
            "`meta\n  `: title\n\n     this is a title\n  `: created\n\n     2026-07-20\n`- before\n\n`- something\n  `- aaa\n`- ssss\n\n`- jjjj\n",
            "`meta\n `: title\n\n    this is a title\n\n `: created\n\n    2026-07-20\n\n`- before\n`- something\n\n   `- aaa\n\n`- ssss\n`- jjjj\n",
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
            "`meta\n `: title\n\n    empty\n\n `: created\n\n    2026-07-22T12:34:56+08:00\n\n";
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
        let source = "`node Parent\n       `- One\n          {\n            `- task\n            `@ one\n          }\n\n       `- Two {`-[task] `@[two]}\n\n`# Following\n";
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
            "`- One\n   {\n     `- task\n     `@ one\n   }\n`- Two {`-[task] `@[two]}"
        );
        assert_eq!(&source[edits[0].range.end..], "\n\n`# Following\n");
        assert!(!edits[0].new_text.contains("`node Parent"));
    }

    #[test]
    fn contained_range_formats_a_complete_parent_subtree() {
        let source =
            "`node Parent\n       `- One\n          {\n            `- task\n            `@ one\n          }\n\n`# Following\n";
        let parsed = parse(source);
        let parent_range = block_content_range(&parsed.syntax.blocks[0]);
        let edits = format_contained_blocks(source, parent_range).unwrap();
        assert_eq!(edits.len(), 1);

        let mut formatted = source.to_string();
        formatted.replace_range(edits[0].range.clone(), &edits[0].new_text);
        assert_eq!(
            formatted,
            "`node Parent\n\n      `- One\n         {\n           `- task\n           `@ one\n         }\n\n`# Following\n"
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
        let source = "`text\"{`:[source test]}\n  payload\n\nParagraph `\"\"\"[a ]\" b]\"\"\".\n\n`# Following\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let selection = block_content_range(&parsed.syntax.blocks[0]).start
            ..block_content_range(&parsed.syntax.blocks[1]).end;
        let edits = format_contained_blocks(source, selection).unwrap();

        assert_eq!(edits.len(), 1);
        assert_eq!(
            edits[0].new_text,
            "`text\"{`:[source test]}\n  payload\n\nParagraph `\"\"[a ]\" b]\"\"."
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
            "`- Work {`-[task] `@[write] `:[created now]}\n  `note Details\n",
            "`- Work {`-[task] `@[write] `:[created now]}\n\n   `note Details\n",
        );
    }

    #[test]
    fn packs_long_block_attributes_within_one_hundred_display_columns() {
        assert_formats(
            "`- Work {`-[task] `@[write] `:[created 2026-07-20T12:00:00+08:00] `:[due 2026-07-21T12:00:00+08:00] `:[depends notes/project.plumb#prepare]}\n",
            "`- Work {`-[task] `@[write] `:[created 2026-07-20T12:00:00+08:00] `:[due 2026-07-21T12:00:00+08:00] `:[depends notes/project.plumb#prepare]}\n",
        );
        assert_formats(
            "`text\"{`:[source generated-with-a-deliberately-long-identifier-that-exceeds-the-limit-by-itself] `:[another value]}\n  payload\n",
            "`text\"{`:[source generated-with-a-deliberately-long-identifier-that-exceeds-the-limit-by-itself] `:[another value]}\n  payload\n",
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
            "`note First `span[a `] b `` c]\n      second\n",
        );
    }

    #[test]
    fn chooses_the_minimum_safe_verbatim_delimiter() {
        assert_formats("Raw `\"\"\"[a ]\" b]\"\"\".\n", "Raw `\"\"[a ]\" b]\"\".\n");
    }

    #[test]
    fn preserves_verbatim_payload_and_its_final_newline() {
        assert_formats("`text\"\n  a\nnext\n", "`text\"\n  a\nnext\n");
        assert_formats("`text\"\n    a\n\nnext\n", "`text\"\n    a\n\nnext\n");
        assert_formats("`\"\n  final newline\n", "`\"\n  final newline\n");
        assert_formats("`\"\n  no newline", "`\"\n  no newline");
    }

    #[test]
    fn terminal_verbatim_descendants_do_not_accumulate_sibling_spacing() {
        let source = "`. config\n\n   `json\"\n     {\"enabled\": true}\n\n\n`# Following\n";
        let formatted = format(source).unwrap();
        assert_eq!(format(&formatted).unwrap(), formatted);
        assert_eq!(formatted.matches("\n\n\n`# Following").count(), 1);
    }

    #[test]
    fn rejects_invalid_documents() {
        assert_eq!(format("`span[open\n"), Err(FormatError::InvalidSyntax));
    }
}
