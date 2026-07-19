use std::collections::HashMap;

use crate::syntax::*;

pub fn parse(source: impl Into<String>) -> ParsedDocument {
    let source = source.into();
    let lines = Lines::new(&source);
    let mut parser = Parser {
        source: &source,
        lines,
        diagnostics: Vec::new(),
    };
    let (blocks, _) = parser.parse_blocks(0, 0);
    ParsedDocument {
        syntax: Document {
            blocks,
            range: 0..source.len(),
        },
        diagnostics: parser.diagnostics,
        source,
    }
}

#[derive(Debug, Clone)]
struct Line {
    start: usize,
    content_end: usize,
    end: usize,
    indent: usize,
    blank: bool,
    has_tab_indent: bool,
}

struct Lines(Vec<Line>);

impl Lines {
    fn new(source: &str) -> Self {
        let mut output = Vec::new();
        let mut start = 0;
        for chunk in source.split_inclusive('\n') {
            let end = start + chunk.len();
            let content_end = if chunk.ends_with('\n') { end - 1 } else { end };
            output.push(line(source, start, content_end, end));
            start = end;
        }
        if source.is_empty() {
            return Self(output);
        }
        if start < source.len() {
            output.push(line(source, start, source.len(), source.len()));
        }
        Self(output)
    }
}

fn line(source: &str, start: usize, content_end: usize, end: usize) -> Line {
    let bytes = source.as_bytes();
    let mut cursor = start;
    let mut indent = 0;
    let mut has_tab_indent = false;
    while cursor < content_end {
        match bytes[cursor] {
            b' ' => {
                indent += 1;
                cursor += 1;
            }
            b'\t' => {
                has_tab_indent = true;
                cursor += 1;
            }
            _ => break,
        }
    }
    Line {
        start,
        content_end,
        end,
        indent,
        blank: source[cursor..content_end].trim().is_empty(),
        has_tab_indent,
    }
}

struct Parser<'a> {
    source: &'a str,
    lines: Lines,
    diagnostics: Vec<Diagnostic>,
}

impl Parser<'_> {
    fn parse_blocks(&mut self, mut index: usize, indent: usize) -> (Vec<Block>, usize) {
        let mut blocks = Vec::new();
        while index < self.lines.0.len() {
            let current = &self.lines.0[index];
            if current.blank {
                index += 1;
                continue;
            }
            if current.has_tab_indent {
                self.diagnostics.push(Diagnostic::error(
                    "syntax.tab-indentation",
                    "tabs are not allowed in structural indentation",
                    current.start..current.start + current.indent + 1,
                ));
            }
            if current.indent < indent {
                break;
            }
            if current.indent > indent {
                self.diagnostics.push(Diagnostic::error(
                    "syntax.partial-indent",
                    format!("expected indentation column {indent}"),
                    current.start..current.start + current.indent,
                ));
            }

            let effective_indent = current.indent;
            if let Some(kind) = self.block_dispatch(index, effective_indent) {
                match kind {
                    BlockDispatch::Marked => {
                        let (block, next) = self.parse_marked(index, effective_indent);
                        blocks.push(Block::Parsed(block));
                        index = next;
                    }
                    BlockDispatch::Verbatim => {
                        let (block, next) = self.parse_verbatim(index, effective_indent);
                        blocks.push(Block::Verbatim(block));
                        index = next;
                    }
                }
            } else {
                let (block, next) = self.parse_paragraph(index, effective_indent);
                blocks.push(Block::Parsed(block));
                index = next;
            }
        }
        (blocks, index)
    }

    fn block_dispatch(&mut self, index: usize, indent: usize) -> Option<BlockDispatch> {
        let line = &self.lines.0[index];
        let start = line.start + indent;
        let text = &self.source[start..line.content_end];
        let ticks = text.bytes().take_while(|byte| *byte == b'`').count();
        if ticks == 0 || ticks % 2 == 0 {
            return None;
        }
        if ticks > 1 {
            return None;
        }
        let after = start + 1;
        if after >= line.content_end {
            self.diagnostics.push(Diagnostic::error(
                "syntax.incomplete-introducer",
                "block introducer requires a marker, attributes, or inline delimiter",
                start..after,
            ));
            return None;
        }
        let byte = self.source.as_bytes()[after];
        if byte == b'[' {
            return None;
        }
        if byte == b'{' {
            return Some(BlockDispatch::Verbatim);
        }
        if byte == b'"' {
            let quotes = self.source[after..line.content_end]
                .bytes()
                .take_while(|candidate| *candidate == b'"')
                .count();
            let tail = after + quotes;
            if tail < line.content_end && self.source.as_bytes()[tail] == b'[' {
                return None;
            }
        } else {
            let marker_end = take_name_like(self.source, after, line.content_end, marker_char);
            if marker_end < line.content_end && self.source.as_bytes()[marker_end] == b'[' {
                return None;
            }
        }
        Some(BlockDispatch::Marked)
    }

    fn parse_marked(&mut self, index: usize, indent: usize) -> (ParsedBlock, usize) {
        let line = self.lines.0[index].clone();
        let introducer = line.start + indent;
        let mut cursor = introducer + 1;
        let marker_start = cursor;
        cursor = take_name_like(self.source, cursor, line.content_end, marker_char);
        if cursor == marker_start {
            self.diagnostics.push(Diagnostic::error(
                "syntax.invalid-marker",
                "invalid or missing marker token",
                introducer..(cursor + 1).min(line.content_end),
            ));
        }
        let marker = self.source[marker_start..cursor].to_string();
        let marker_range = marker_start..cursor;

        let attrs = if cursor < line.content_end && self.source.as_bytes()[cursor] == b'{' {
            let (attrs, next) = self.parse_attributes(cursor, line.content_end);
            cursor = next;
            attrs
        } else {
            Attributes::default()
        };
        let mark_end = cursor;
        let head_start = if cursor < line.content_end {
            if matches!(self.source.as_bytes()[cursor], b' ' | b'\t') {
                while cursor < line.content_end
                    && matches!(self.source.as_bytes()[cursor], b' ' | b'\t')
                {
                    cursor += 1;
                }
                cursor
            } else {
                self.diagnostics.push(Diagnostic::error(
                    "syntax.invalid-block-dispatch",
                    "marker must be followed by attributes, whitespace, or end of line",
                    introducer..next_char_end(self.source, cursor),
                ));
                cursor
            }
        } else {
            cursor
        };

        let mut head = self.parse_inline(head_start, line.content_end, false);
        let mut next = index + 1;
        let mut saw_blank = false;
        let mut body_indent = None;

        while next < self.lines.0.len() {
            let candidate = self.lines.0[next].clone();
            if candidate.blank {
                saw_blank = true;
                next += 1;
                continue;
            }
            if candidate.indent <= indent {
                break;
            }
            body_indent.get_or_insert(candidate.indent);
            if candidate.indent != body_indent.unwrap() {
                break;
            }
            if !saw_blank && self.block_dispatch(next, candidate.indent).is_none() {
                if !head.items.is_empty() {
                    head.items.push(Inline::SoftBreak {
                        range: line.content_end..candidate.start + candidate.indent,
                    });
                }
                let continuation = self.parse_inline(
                    candidate.start + candidate.indent,
                    candidate.content_end,
                    false,
                );
                head.items.extend(continuation.items);
                head.range.end = candidate.content_end;
                next += 1;
                continue;
            }
            break;
        }

        let mut child_start = next;
        while child_start < self.lines.0.len() && self.lines.0[child_start].blank {
            child_start += 1;
        }
        let (children, after_children) =
            if child_start < self.lines.0.len() && self.lines.0[child_start].indent > indent {
                self.parse_blocks(child_start, self.lines.0[child_start].indent)
            } else {
                (Vec::new(), next)
            };
        if !children.is_empty() {
            next = after_children;
        }
        let end = children
            .last()
            .map(|child| child.range().end)
            .or_else(|| {
                next.checked_sub(1)
                    .and_then(|i| self.lines.0.get(i).map(|l| l.end))
            })
            .unwrap_or(line.end);

        (
            ParsedBlock {
                range: introducer..end,
                mark: Some(Mark {
                    range: introducer..mark_end,
                    marker,
                    marker_range,
                    attrs,
                }),
                head,
                children,
            },
            next,
        )
    }

    fn parse_verbatim(&mut self, index: usize, indent: usize) -> (VerbatimBlock, usize) {
        let line = self.lines.0[index].clone();
        let introducer = line.start + indent;
        let attr_start = introducer + 1;
        let (attrs, after_attrs) = self.parse_attributes(attr_start, line.content_end);
        if after_attrs != line.content_end {
            self.diagnostics.push(Diagnostic::error(
                "syntax.invalid-verbatim-block-dispatch",
                "verbatim block attributes must be followed by end of line",
                introducer..line.content_end,
            ));
        }
        let body_indent = indent + 2;
        let mut next = index + 1;
        let mut text = String::new();
        let text_start = self.lines.0.get(next).map_or(line.end, |next| next.start);
        let mut text_end = text_start;
        while next < self.lines.0.len() {
            let candidate = &self.lines.0[next];
            if !candidate.blank && candidate.indent < body_indent {
                break;
            }
            if candidate.blank {
                text.push('\n');
            } else {
                let content = candidate.start + body_indent;
                if content > candidate.content_end {
                    self.diagnostics.push(Diagnostic::error(
                        "syntax.short-verbatim-indent",
                        format!("verbatim payload requires at least {body_indent} spaces"),
                        candidate.start..candidate.content_end,
                    ));
                } else {
                    text.push_str(&self.source[content..candidate.content_end]);
                    if candidate.end > candidate.content_end {
                        text.push('\n');
                    }
                }
            }
            text_end = candidate.end;
            next += 1;
        }
        (
            VerbatimBlock {
                range: introducer..text_end.max(line.end),
                opener_range: introducer..introducer + 2,
                attrs,
                text,
                text_range: text_start..text_end,
            },
            next,
        )
    }

    fn parse_paragraph(&mut self, index: usize, indent: usize) -> (ParsedBlock, usize) {
        let first = self.lines.0[index].clone();
        let start = first.start + indent;
        let mut head = self.parse_inline(start, first.content_end, false);
        let mut next = index + 1;
        let mut end = first.end;
        while next < self.lines.0.len() {
            let candidate = self.lines.0[next].clone();
            if candidate.blank
                || candidate.indent != indent
                || self.block_dispatch(next, indent).is_some()
            {
                break;
            }
            head.items.push(Inline::SoftBreak {
                range: end.saturating_sub(1)..candidate.start + indent,
            });
            let continuation =
                self.parse_inline(candidate.start + indent, candidate.content_end, false);
            head.items.extend(continuation.items);
            head.range.end = candidate.content_end;
            end = candidate.end;
            next += 1;
        }
        (
            ParsedBlock {
                range: start..end,
                mark: None,
                head,
                children: Vec::new(),
            },
            next,
        )
    }

    fn parse_inline(&mut self, start: usize, end: usize, nested: bool) -> InlineContent {
        let mut items = Vec::new();
        let mut cursor = start;
        let mut text_start = start;
        while cursor < end {
            let byte = self.source.as_bytes()[cursor];
            if nested && byte == b']' {
                break;
            }
            if byte != b'`' {
                cursor = next_char_end(self.source, cursor);
                continue;
            }
            if text_start < cursor {
                items.push(Inline::Text {
                    text: self.source[text_start..cursor].to_string(),
                    range: text_start..cursor,
                });
            }
            let ticks = self.source[cursor..end]
                .bytes()
                .take_while(|candidate| *candidate == b'`')
                .count();
            for pair in 0..ticks / 2 {
                let pair_start = cursor + pair * 2;
                items.push(Inline::Text {
                    text: "`".to_string(),
                    range: pair_start..pair_start + 2,
                });
            }
            cursor += (ticks / 2) * 2;
            if ticks % 2 == 0 {
                text_start = cursor;
                continue;
            }
            let introducer = cursor;
            cursor += 1;
            if nested && cursor < end && self.source.as_bytes()[cursor] == b']' {
                items.push(Inline::Text {
                    text: "]".to_string(),
                    range: introducer..cursor + 1,
                });
                cursor += 1;
                text_start = cursor;
                continue;
            }
            let quotes = self.source[cursor..end]
                .bytes()
                .take_while(|candidate| *candidate == b'"')
                .count();
            let open = cursor + quotes;
            if open < end && self.source.as_bytes()[open] == b'[' {
                if let Some((close, after_close)) =
                    find_verbatim_close(self.source, open + 1, end, quotes)
                {
                    let (attrs, after_attrs) =
                        if after_close < end && self.source.as_bytes()[after_close] == b'{' {
                            self.parse_attributes(after_close, end)
                        } else {
                            (Attributes::default(), after_close)
                        };
                    items.push(Inline::Verbatim {
                        range: introducer..after_attrs,
                        text: self.source[open + 1..close].to_string(),
                        text_range: open + 1..close,
                        quote_count: quotes,
                        attrs,
                    });
                    cursor = after_attrs;
                    text_start = cursor;
                    continue;
                }
                self.diagnostics.push(Diagnostic::error(
                    "syntax.unclosed-verbatim",
                    "inline verbatim must close on the same physical line",
                    introducer..end,
                ));
                cursor = end;
                text_start = end;
                continue;
            }

            let kind_start = cursor;
            cursor = take_name_like(self.source, cursor, end, marker_char);
            let kind_end = cursor;
            if cursor < end && self.source.as_bytes()[cursor] == b'[' {
                let content_start = cursor + 1;
                let content = self.parse_inline(content_start, end, true);
                let close = content.range.end;
                if close < end && self.source.as_bytes()[close] == b']' {
                    let after_close = close + 1;
                    let (attrs, after_attrs) =
                        if after_close < end && self.source.as_bytes()[after_close] == b'{' {
                            self.parse_attributes(after_close, end)
                        } else {
                            (Attributes::default(), after_close)
                        };
                    items.push(Inline::Element {
                        range: introducer..after_attrs,
                        kind: self.source[kind_start..kind_end].to_string(),
                        kind_range: kind_start..kind_end,
                        content,
                        attrs,
                    });
                    cursor = after_attrs;
                    text_start = cursor;
                    continue;
                }
                self.diagnostics.push(Diagnostic::error(
                    "syntax.unclosed-inline",
                    "parsed inline element is not closed before the line boundary",
                    introducer..end,
                ));
                cursor = end;
                text_start = end;
                continue;
            }

            self.diagnostics.push(Diagnostic::error(
                "syntax.invalid-inline-dispatch",
                "inline introducer requires an inline kind followed by '['",
                introducer..next_char_end(self.source, cursor.min(end.saturating_sub(1))),
            ));
            text_start = cursor;
        }
        if text_start < cursor {
            items.push(Inline::Text {
                text: self.source[text_start..cursor].to_string(),
                range: text_start..cursor,
            });
        }
        InlineContent {
            range: start..cursor,
            items,
        }
    }

    fn parse_attributes(&mut self, start: usize, limit: usize) -> (Attributes, usize) {
        let mut cursor = start + 1;
        let mut items = Vec::new();
        let mut id_range: Option<SourceRange> = None;
        let mut keys: HashMap<String, SourceRange> = HashMap::new();
        while cursor < limit {
            while cursor < limit && self.source.as_bytes()[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if cursor >= limit {
                break;
            }
            if self.source.as_bytes()[cursor] == b'}' {
                let end = cursor + 1;
                return (
                    Attributes {
                        range: Some(start..end),
                        items,
                    },
                    end,
                );
            }
            let item_start = cursor;
            match self.source.as_bytes()[cursor] {
                b'#' | b'.' => {
                    let prefix = self.source.as_bytes()[cursor];
                    cursor += 1;
                    let name_start = cursor;
                    cursor = take_name_like(self.source, cursor, limit, attr_name_char);
                    if cursor == name_start {
                        self.diagnostics.push(Diagnostic::error(
                            "syntax.empty-attribute-name",
                            "attribute id/class requires a name",
                            item_start..cursor,
                        ));
                        continue;
                    }
                    let range = item_start..cursor;
                    let value = self.source[name_start..cursor].to_string();
                    if prefix == b'#' {
                        if let Some(first) = &id_range {
                            let mut diagnostic = Diagnostic::error(
                                "syntax.duplicate-id",
                                "an attribute slot may contain only one id",
                                range.clone(),
                            );
                            diagnostic.related.push(first.clone());
                            self.diagnostics.push(diagnostic);
                        } else {
                            id_range = Some(range.clone());
                        }
                        items.push(AttrItem::Id { value, range });
                    } else {
                        items.push(AttrItem::Class { value, range });
                    }
                }
                _ => {
                    let key_start = cursor;
                    cursor = take_name_like(self.source, cursor, limit, attr_name_char);
                    if cursor == key_start
                        || cursor >= limit
                        || self.source.as_bytes()[cursor] != b'='
                    {
                        self.diagnostics.push(Diagnostic::error(
                            "syntax.malformed-attribute-item",
                            "attribute item must be #id, .class, or key=value",
                            item_start..next_token_end(self.source, cursor, limit),
                        ));
                        cursor = next_token_end(self.source, cursor, limit);
                        continue;
                    }
                    let key = self.source[key_start..cursor].to_string();
                    let key_range = key_start..cursor;
                    cursor += 1;
                    let value_start = cursor;
                    let value = if cursor < limit && self.source.as_bytes()[cursor] == b'"' {
                        cursor += 1;
                        let mut decoded = String::new();
                        let mut closed = false;
                        while cursor < limit {
                            let byte = self.source.as_bytes()[cursor];
                            if byte == b'"' {
                                cursor += 1;
                                closed = true;
                                break;
                            }
                            if byte == b'\\' {
                                if cursor + 1 < limit
                                    && matches!(self.source.as_bytes()[cursor + 1], b'"' | b'\\')
                                {
                                    decoded.push(self.source.as_bytes()[cursor + 1] as char);
                                    cursor += 2;
                                    continue;
                                }
                                self.diagnostics.push(Diagnostic::error(
                                    "syntax.unknown-quoted-escape",
                                    "quoted values only allow escaping quote and backslash",
                                    cursor..(cursor + 2).min(limit),
                                ));
                            }
                            let next = next_char_end(self.source, cursor);
                            decoded.push_str(&self.source[cursor..next]);
                            cursor = next;
                        }
                        if !closed {
                            self.diagnostics.push(Diagnostic::error(
                                "syntax.unclosed-quoted-value",
                                "quoted attribute value is not closed",
                                value_start..limit,
                            ));
                        }
                        AttrValue {
                            decoded,
                            raw: self.source[value_start..cursor].to_string(),
                            range: value_start..cursor,
                            quoted: true,
                        }
                    } else {
                        cursor = take_name_like(self.source, cursor, limit, attr_name_char);
                        if cursor == value_start {
                            self.diagnostics.push(Diagnostic::error(
                                "syntax.empty-attribute-value",
                                "attribute pair requires a value",
                                value_start..cursor,
                            ));
                        }
                        AttrValue {
                            decoded: self.source[value_start..cursor].to_string(),
                            raw: self.source[value_start..cursor].to_string(),
                            range: value_start..cursor,
                            quoted: false,
                        }
                    };
                    let range = item_start..cursor;
                    if let Some(first) = keys.get(&key) {
                        let mut diagnostic = Diagnostic::error(
                            "syntax.duplicate-key",
                            format!("attribute key '{key}' appears more than once"),
                            key_range.clone(),
                        );
                        diagnostic.related.push(first.clone());
                        self.diagnostics.push(diagnostic);
                    } else {
                        keys.insert(key.clone(), key_range.clone());
                    }
                    items.push(AttrItem::Pair {
                        key,
                        key_range,
                        value,
                        range,
                    });
                }
            }
        }
        self.diagnostics.push(Diagnostic::error(
            "syntax.unclosed-attributes",
            "attribute slot is not closed before the line boundary",
            start..limit,
        ));
        (
            Attributes {
                range: Some(start..limit),
                items,
            },
            limit,
        )
    }
}

#[derive(Clone, Copy)]
enum BlockDispatch {
    Marked,
    Verbatim,
}

fn marker_char(character: char) -> bool {
    !character.is_whitespace()
        && !character.is_control()
        && !matches!(character, '`' | '"' | '[' | ']' | '{' | '}')
}

fn attr_name_char(character: char) -> bool {
    marker_char(character) && !matches!(character, '#' | '.' | '=')
}

fn take_name_like(
    source: &str,
    mut cursor: usize,
    limit: usize,
    predicate: fn(char) -> bool,
) -> usize {
    while cursor < limit {
        let character = source[cursor..]
            .chars()
            .next()
            .expect("cursor is on char boundary");
        if !predicate(character) {
            break;
        }
        cursor += character.len_utf8();
    }
    cursor
}

fn next_char_end(source: &str, cursor: usize) -> usize {
    if cursor >= source.len() {
        return source.len();
    }
    cursor + source[cursor..].chars().next().map_or(0, char::len_utf8)
}

fn next_token_end(source: &str, mut cursor: usize, limit: usize) -> usize {
    while cursor < limit {
        let byte = source.as_bytes()[cursor];
        if byte.is_ascii_whitespace() || byte == b'}' {
            break;
        }
        cursor = next_char_end(source, cursor);
    }
    cursor
}

fn find_verbatim_close(
    source: &str,
    mut cursor: usize,
    limit: usize,
    quotes: usize,
) -> Option<(usize, usize)> {
    while cursor < limit {
        if source.as_bytes()[cursor] == b']' {
            let quote_start = cursor + 1;
            let count = source[quote_start..limit]
                .bytes()
                .take_while(|candidate| *candidate == b'"')
                .count();
            if count >= quotes {
                return Some((cursor, quote_start + quotes));
            }
        }
        cursor = next_char_end(source, cursor);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_heading_and_nested_blocks() {
        let parsed = parse("`heading{#intro level=1} Intro\n  child text\n\n  `task Work\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Parsed(heading) = &parsed.syntax.blocks[0] else {
            panic!("expected heading");
        };
        assert_eq!(heading.head.plain_text(), "Intro child text");
        assert_eq!(heading.children.len(), 1);
    }

    #[test]
    fn reports_duplicate_attributes() {
        let parsed = parse("`node{#one #two key=a key=b} head\n");
        assert_eq!(
            parsed
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            ["syntax.duplicate-id", "syntax.duplicate-key"]
        );
    }

    #[test]
    fn parses_inline_elements_and_verbatim() {
        let parsed = parse("Text `em[inside] and `[raw].\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Parsed(block) = &parsed.syntax.blocks[0] else {
            panic!("expected paragraph");
        };
        assert_eq!(block.head.plain_text(), "Text inside and raw.");
    }

    #[test]
    fn quote_count_strengthens_inline_verbatim_delimiters() {
        let parsed = parse("`[plain] `\"[contains ] safely]\" `\"\"[contains ]\" safely]\"\"\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Parsed(block) = &parsed.syntax.blocks[0] else {
            panic!("expected paragraph");
        };
        let verbatim = block
            .head
            .items
            .iter()
            .filter_map(|inline| match inline {
                Inline::Verbatim {
                    text, quote_count, ..
                } => Some((text.as_str(), *quote_count)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            verbatim,
            [
                ("plain", 0),
                ("contains ] safely", 1),
                ("contains ]\" safely", 2)
            ]
        );
    }

    #[test]
    fn strengthened_inline_verbatim_can_start_a_physical_line() {
        let parsed = parse("`\"[raw]\" tail\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Parsed(block) = &parsed.syntax.blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(matches!(block.head.items[0], Inline::Verbatim { .. }));
    }

    #[test]
    fn parses_verbatim_block_with_fixed_two_column_margin() {
        let parsed = parse("`{language=rust}\n  fn main() {}\n    indented\nnext\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Verbatim(block) = &parsed.syntax.blocks[0] else {
            panic!("expected verbatim block");
        };
        assert_eq!(block.attrs.value("language"), Some("rust"));
        assert_eq!(block.text, "fn main() {}\n  indented\n");
        assert!(matches!(parsed.syntax.blocks[1], Block::Parsed(_)));
    }

    #[test]
    fn parsed_inline_and_marked_block_require_names() {
        let inline = parse("`[not parsed]\n");
        let Block::Parsed(paragraph) = &inline.syntax.blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(matches!(paragraph.head.items[0], Inline::Verbatim { .. }));

        let block = parse("`{.note}\n  raw `em[not parsed]\n");
        assert!(matches!(block.syntax.blocks[0], Block::Verbatim(_)));

        let old_quote_block = parse("`\"\n  old code block spelling\n");
        assert!(!old_quote_block.is_valid());

        let verbatim_head = parse("`{} head is forbidden\n");
        assert!(!verbatim_head.is_valid());
    }
}
