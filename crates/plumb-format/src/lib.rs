use std::ffi::OsString;
use std::fmt::Write;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use plumb_core::{parse, AttrItem, Attributes, Block, Inline, InlineContent, ParsedBlock};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    InvalidSyntax,
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
