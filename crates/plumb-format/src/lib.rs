use std::fmt::Write;

use plumb_core::{parse, AttrItem, Attributes, Block, Inline, InlineContent, ParsedBlock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    InvalidSyntax,
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
                } else if matches!(previous, Block::Parsed(block) if block.mark.is_some())
                    && matches!(block, Block::Parsed(block) if block.mark.is_some())
                {
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
                self.attributes(&block.attrs);
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
            self.attributes(&mark.attrs);
            if !block.head.items.is_empty() {
                self.output.push(' ');
            }
            indent + 2
        } else {
            indent
        };
        self.inlines(&block.head, continuation_indent, false);

        if !block.children.is_empty() {
            self.output.push_str("\n\n");
            self.blocks(&block.children, indent + 2);
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
        if attrs.range.is_none() {
            return;
        }
        self.output.push('{');
        for (index, item) in attrs.items.iter().enumerate() {
            if index > 0 {
                self.output.push(' ');
            }
            match item {
                AttrItem::Id { value, .. } => {
                    self.output.push('#');
                    self.output.push_str(value);
                }
                AttrItem::Class { value, .. } => {
                    self.output.push('.');
                    self.output.push_str(value);
                }
                AttrItem::Pair { key, value, .. } => {
                    let _ = write!(self.output, "{key}={}", value.raw);
                }
            }
        }
        self.output.push('}');
    }

    fn indent(&mut self, indent: usize) {
        self.output.extend(std::iter::repeat_n(' ', indent));
    }
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
        let formatted = format(source).unwrap();
        assert_eq!(formatted, expected);
        assert!(parse(&formatted).is_valid());
        assert_eq!(format(&formatted).unwrap(), formatted);
    }

    #[test]
    fn formats_blocks_attributes_and_indentation() {
        assert_formats(
            "`meta\n   `: title\n\n      Example\n\n`-{.task\n   #write\n   created=now\n   } Work\n",
            "`meta\n\n  `: title\n\n    Example\n`-{.task #write created=now} Work\n",
        );
    }

    #[test]
    fn preserves_soft_breaks_and_inline_meaning() {
        assert_formats(
            "`note First `span[a `] b `` c]\n   second\n",
            "`note First `span[a `] b `` c]\n  second\n",
        );
    }

    #[test]
    fn chooses_the_minimum_safe_verbatim_delimiter() {
        assert_formats("Raw `\"\"\"[a ]\" b]\"\"\".\n", "Raw `\"\"[a ]\" b]\"\".\n");
    }

    #[test]
    fn preserves_verbatim_payload_and_its_final_newline() {
        assert_formats("`{language=text}\n  a\nnext\n", "`{language=text}\n  a\nnext\n");
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
