use crate::lossless::build_lossless;
use crate::syntax::{
    AttrItem, AttrValue, Attributes, Block, Diagnostic, Document, Inline, InlineContent, Mark,
    ParsedBlock, ParsedDocument, SourceRange, VerbatimBlock,
};

pub fn parse(source: impl Into<String>) -> ParsedDocument {
    let source = source.into();
    let (syntax, diagnostics) = {
        let mut parser = Parser::new(&source);
        let mut syntax = parser.parse_document();
        project_attributes(&source, &mut syntax);
        parser
            .diagnostics
            .sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
        (syntax, parser.diagnostics)
    };
    let lossless = build_lossless(&source, &syntax, &diagnostics);
    ParsedDocument {
        source,
        lossless,
        syntax,
        diagnostics,
    }
}

#[derive(Debug, Clone)]
struct Line {
    start: usize,
    content_end: usize,
    end: usize,
    indent: usize,
    blank: bool,
    structural_tab: Option<usize>,
}

struct Parser<'a> {
    source: &'a str,
    lines: Vec<Line>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            lines: scan_lines(source),
            diagnostics: Vec::new(),
        }
    }

    fn parse_document(&mut self) -> Document {
        let mut levels = vec![Level::root()];
        let mut index = 0;

        while index < self.lines.len() {
            let line = self.lines[index].clone();
            if line.blank {
                index += 1;
                continue;
            }
            if let Some(tab) = line.structural_tab {
                self.error(
                    "syntax.tab-indentation",
                    "tabs cannot be used for structural indentation",
                    tab..tab + 1,
                );
            }

            let (block, next_index) = self.parse_block(index);
            index = next_index;
            self.place_block(&mut levels, line.indent, block);
        }

        while levels.len() > 1 {
            close_level(&mut levels);
        }
        let blocks = levels.pop().expect("root level exists").blocks;
        Document {
            attrs: Default::default(),
            blocks,
            range: 0..self.source.len(),
        }
    }

    fn place_block(&mut self, levels: &mut Vec<Level>, indent: usize, block: Block) {
        let mut effective_indent = indent;
        let current_indent = levels.last().expect("root level exists").indent;
        if effective_indent > current_indent {
            let owner = levels.last_mut().and_then(|level| level.blocks.pop());
            match owner {
                Some(Block::Parsed(owner)) => levels.push(Level {
                    indent: effective_indent,
                    owner: Some(owner),
                    blocks: Vec::new(),
                }),
                Some(owner @ Block::Verbatim(_)) => {
                    levels.last_mut().unwrap().blocks.push(owner);
                    self.error(
                        "syntax.partial-indent",
                        "a verbatim block cannot own structural children",
                        block.range().start..block.range().start + 1,
                    );
                    effective_indent = current_indent;
                }
                None => {
                    self.error(
                        "syntax.partial-indent",
                        "top-level blocks must start at column zero",
                        block.range().start..block.range().start + 1,
                    );
                    effective_indent = current_indent;
                }
            }
        } else if effective_indent < current_indent {
            while levels.len() > 1 && effective_indent < levels.last().unwrap().indent {
                close_level(levels);
            }
            if effective_indent != levels.last().unwrap().indent {
                self.error(
                    "syntax.partial-indent",
                    "dedent must return to an existing indentation column",
                    block.range().start..block.range().start + 1,
                );
                effective_indent = levels.last().unwrap().indent;
            }
        }

        debug_assert_eq!(effective_indent, levels.last().unwrap().indent);
        levels.last_mut().unwrap().blocks.push(block);
    }

    fn parse_block(&mut self, line_index: usize) -> (Block, usize) {
        let line = self.lines[line_index].clone();
        let content_start = line.start + line.indent;
        let content = &self.source[content_start..line.content_end];

        if let Some(dispatch) = self.block_dispatch(content_start, line.content_end) {
            match dispatch {
                BlockDispatch::Parsed { mark, inline_start } => {
                    let content = self.parse_inline_content(inline_start..line.content_end);
                    let block = ParsedBlock {
                        range: content_start..line.end,
                        mark,
                        content,
                        children: Vec::new(),
                    };
                    return (Block::Parsed(block), line_index + 1);
                }
                BlockDispatch::Verbatim { mark, quote_range } => {
                    return self.parse_verbatim_block(line_index, mark, quote_range);
                }
            }
        }

        if content.starts_with('`') && content.len() == 1 {
            self.error(
                "syntax.incomplete-introducer",
                "an introducer must start an escape, marker, group, or verbatim value",
                content_start..line.content_end,
            );
        }
        let content = self.parse_inline_content(content_start..line.content_end);
        (
            Block::Parsed(ParsedBlock {
                range: content_start..line.end,
                mark: None,
                content,
                children: Vec::new(),
            }),
            line_index + 1,
        )
    }

    fn block_dispatch(&mut self, start: usize, end: usize) -> Option<BlockDispatch> {
        let bytes = self.source.as_bytes();
        if start >= end || bytes[start] != b'`' {
            return None;
        }
        if start + 1 < end && bytes[start + 1] == b'`' {
            return None;
        }
        let after = start + 1;
        if after == end {
            return None;
        }
        if bytes[after] == b'"' && after + 1 == end {
            return Some(BlockDispatch::Verbatim {
                mark: None,
                quote_range: after..after + 1,
            });
        }
        if matches!(bytes[after], b'{' | b'}') {
            return None;
        }

        let marker_end = scan_marker(self.source, after, end);
        if marker_end == after {
            return None;
        }
        let mark = Mark {
            range: start..marker_end,
            marker: self.source[after..marker_end].to_string(),
            marker_range: after..marker_end,
            attrs: Default::default(),
        };
        if marker_end == end {
            return Some(BlockDispatch::Parsed {
                mark: Some(mark),
                inline_start: end,
            });
        }
        match bytes[marker_end] {
            b' ' => Some(BlockDispatch::Parsed {
                mark: Some(mark),
                inline_start: marker_end,
            }),
            b'"' if marker_end + 1 == end => Some(BlockDispatch::Verbatim {
                mark: Some(mark),
                quote_range: marker_end..marker_end + 1,
            }),
            b'{' | b'"' => None,
            byte if byte.is_ascii_whitespace() => {
                self.error(
                    "syntax.invalid-block-dispatch",
                    "only ASCII space can separate a block marker from its content",
                    marker_end..marker_end + 1,
                );
                Some(BlockDispatch::Parsed {
                    mark: Some(mark),
                    inline_start: marker_end,
                })
            }
            _ => None,
        }
    }

    fn parse_verbatim_block(
        &mut self,
        line_index: usize,
        mark: Option<Mark>,
        quote_range: SourceRange,
    ) -> (Block, usize) {
        let opener = self.lines[line_index].clone();
        let margin = opener.indent + 1;
        let mut index = line_index + 1;
        let mut text = String::new();
        let mut text_start = opener.end;
        let mut text_end = opener.end;
        let mut has_payload = false;

        while index < self.lines.len() {
            let line = &self.lines[index];
            let prefix_end = line.start.saturating_add(margin);
            if prefix_end > line.content_end
                || !self.source[line.start..prefix_end]
                    .bytes()
                    .all(|byte| byte == b' ')
            {
                break;
            }
            if !has_payload {
                text_start = line.start;
                has_payload = true;
            }
            text.push_str(&self.source[prefix_end..line.end]);
            text_end = line.end;
            index += 1;
        }

        let block_end = if has_payload { text_end } else { opener.end };
        let content_start = opener.start + opener.indent;
        (
            Block::Verbatim(VerbatimBlock {
                range: content_start..block_end,
                opener_range: content_start..opener.content_end,
                mark,
                quote_range,
                text,
                text_range: text_start..text_end,
            }),
            index,
        )
    }

    fn parse_inline_content(&mut self, range: SourceRange) -> InlineContent {
        let (items, _) = self.parse_inline_sequence(range.start, range.end, false);
        InlineContent::from_items(range, items)
    }

    fn parse_inline_sequence(
        &mut self,
        mut cursor: usize,
        end: usize,
        stop_at_close: bool,
    ) -> (Vec<Inline>, usize) {
        let mut items = Vec::new();
        while cursor < end {
            let byte = self.source.as_bytes()[cursor];
            match byte {
                b' ' => {
                    let start = cursor;
                    while cursor < end && self.source.as_bytes()[cursor] == b' ' {
                        cursor += 1;
                    }
                    items.push(Inline::Space {
                        text: self.source[start..cursor].to_string(),
                        range: start..cursor,
                    });
                }
                b'{' => {
                    let (group, next) = self.parse_group(cursor, end, None);
                    items.push(group);
                    cursor = next;
                }
                b'}' if stop_at_close => return (items, cursor),
                b'}' => {
                    self.error(
                        "syntax.unexpected-inline-group-close",
                        "closing brace has no enclosing inline group",
                        cursor..cursor + 1,
                    );
                    items.push(Inline::Text {
                        text: "}".to_string(),
                        range: cursor..cursor + 1,
                    });
                    cursor += 1;
                }
                b'`' => {
                    cursor = self.parse_introducer_run(cursor, end, &mut items);
                }
                b'\t' | 0x00..=0x1f | 0x7f => {
                    let width = self.source[cursor..].chars().next().unwrap().len_utf8();
                    self.error(
                        "syntax.invalid-inline-dispatch",
                        "tabs and control characters are invalid in parsed inline content",
                        cursor..cursor + width,
                    );
                    items.push(Inline::Text {
                        text: self.source[cursor..cursor + width].to_string(),
                        range: cursor..cursor + width,
                    });
                    cursor += width;
                }
                _ => {
                    let start = cursor;
                    cursor += self.source[cursor..].chars().next().unwrap().len_utf8();
                    while cursor < end {
                        let next = self.source.as_bytes()[cursor];
                        if matches!(next, b' ' | b'{' | b'}' | b'`' | b'\t' | 0x00..=0x1f | 0x7f) {
                            break;
                        }
                        cursor += self.source[cursor..].chars().next().unwrap().len_utf8();
                    }
                    items.push(Inline::Text {
                        text: self.source[start..cursor].to_string(),
                        range: start..cursor,
                    });
                }
            }
        }
        (items, cursor)
    }

    fn parse_introducer_run(&mut self, start: usize, end: usize, items: &mut Vec<Inline>) -> usize {
        let mut run_end = start;
        while run_end < end && self.source.as_bytes()[run_end] == b'`' {
            run_end += 1;
        }
        let count = run_end - start;
        let pair_width = count / 2 * 2;
        if pair_width > 0 {
            items.push(Inline::Text {
                text: "`".repeat(pair_width / 2),
                range: start..start + pair_width,
            });
        }
        if count % 2 == 0 {
            return run_end;
        }

        let introducer = start + pair_width;
        let after = introducer + 1;
        if after >= end {
            self.error(
                "syntax.invalid-inline-dispatch",
                "an introducer must start an escape, marked group, or verbatim value",
                introducer..after,
            );
            return after;
        }
        match self.source.as_bytes()[after] {
            b'{' | b'}' => {
                items.push(Inline::Text {
                    text: self.source[after..after + 1].to_string(),
                    range: introducer..after + 1,
                });
                after + 1
            }
            b'"' => {
                let (verbatim, next) = self.parse_inline_verbatim(introducer, after, end, None);
                items.push(verbatim);
                next
            }
            _ => {
                let marker_end = scan_marker(self.source, after, end);
                if marker_end == after {
                    self.error(
                        "syntax.invalid-inline-dispatch",
                        "invalid character after inline introducer",
                        introducer..after + 1,
                    );
                    return after;
                }
                let mark = Mark {
                    range: introducer..marker_end,
                    marker: self.source[after..marker_end].to_string(),
                    marker_range: after..marker_end,
                    attrs: Default::default(),
                };
                if marker_end < end {
                    match self.source.as_bytes()[marker_end] {
                        b'{' => {
                            let (group, next) = self.parse_group(marker_end, end, Some(mark));
                            items.push(group);
                            return next;
                        }
                        b'"' => {
                            let (verbatim, next) =
                                self.parse_inline_verbatim(introducer, marker_end, end, Some(mark));
                            items.push(verbatim);
                            return next;
                        }
                        _ => {}
                    }
                }
                self.error(
                    "syntax.invalid-inline-dispatch",
                    "an inline marker must be followed immediately by a group or quote",
                    introducer..marker_end,
                );
                items.push(Inline::Text {
                    text: self.source[after..marker_end].to_string(),
                    range: introducer..marker_end,
                });
                marker_end
            }
        }
    }

    fn parse_group(&mut self, open: usize, end: usize, mark: Option<Mark>) -> (Inline, usize) {
        let (items, close) = self.parse_inline_sequence(open + 1, end, true);
        let (range_end, content_end) = if close < end && self.source.as_bytes()[close] == b'}' {
            (close + 1, close)
        } else {
            self.error(
                "syntax.unclosed-inline-group",
                "inline group is not closed before the physical line ends",
                open..end,
            );
            (end, end)
        };
        let range_start = mark.as_ref().map_or(open, |mark| mark.range.start);
        (
            Inline::Group {
                range: range_start..range_end,
                mark,
                content: InlineContent::from_items(open + 1..content_end, items),
            },
            range_end,
        )
    }

    fn parse_inline_verbatim(
        &mut self,
        introducer: usize,
        quote_start: usize,
        end: usize,
        mark: Option<Mark>,
    ) -> (Inline, usize) {
        let mut quote_end = quote_start;
        while quote_end < end && self.source.as_bytes()[quote_end] == b'"' {
            quote_end += 1;
        }
        let quote_count = quote_end - quote_start;
        if quote_end < end && self.source.as_bytes()[quote_end] == b'{' {
            let payload_start = quote_end + 1;
            if let Some(close) =
                find_full_verbatim_close(self.source, payload_start, end, quote_count)
            {
                let range_end = close + 1 + quote_count;
                return (
                    Inline::Verbatim {
                        range: introducer..range_end,
                        mark,
                        text: self.source[payload_start..close].to_string(),
                        text_range: payload_start..close,
                        quote_count,
                        braced: true,
                    },
                    range_end,
                );
            }
            self.error(
                "syntax.unclosed-verbatim",
                "full inline verbatim is not closed before the physical line ends",
                introducer..end,
            );
            return (
                Inline::Verbatim {
                    range: introducer..end,
                    mark,
                    text: self.source[payload_start..end].to_string(),
                    text_range: payload_start..end,
                    quote_count,
                    braced: true,
                },
                end,
            );
        }

        let payload_start = quote_start + 1;
        if let Some(relative) = self.source[payload_start..end].find('"') {
            let close = payload_start + relative;
            return (
                Inline::Verbatim {
                    range: introducer..close + 1,
                    mark,
                    text: self.source[payload_start..close].to_string(),
                    text_range: payload_start..close,
                    quote_count: 1,
                    braced: false,
                },
                close + 1,
            );
        }

        self.error(
            "syntax.unclosed-verbatim",
            "compact inline verbatim requires one closing quote",
            introducer..end,
        );
        (
            Inline::Verbatim {
                range: introducer..end,
                mark,
                text: self.source[payload_start..end].to_string(),
                text_range: payload_start..end,
                quote_count: 1,
                braced: false,
            },
            end,
        )
    }

    fn error(&mut self, code: &'static str, message: &'static str, range: SourceRange) {
        self.diagnostics
            .push(Diagnostic::error(code, message, range));
    }
}

enum BlockDispatch {
    Parsed {
        mark: Option<Mark>,
        inline_start: usize,
    },
    Verbatim {
        mark: Option<Mark>,
        quote_range: SourceRange,
    },
}

struct Level {
    indent: usize,
    owner: Option<ParsedBlock>,
    blocks: Vec<Block>,
}

impl Level {
    fn root() -> Self {
        Self {
            indent: 0,
            owner: None,
            blocks: Vec::new(),
        }
    }
}

fn close_level(levels: &mut Vec<Level>) {
    let level = levels.pop().expect("nested level exists");
    let mut owner = level.owner.expect("nested level has an owner");
    owner.children = level.blocks;
    if let Some(last) = owner.children.last() {
        owner.range.end = last.range().end;
    }
    levels
        .last_mut()
        .expect("parent level exists")
        .blocks
        .push(Block::Parsed(owner));
}

fn scan_lines(source: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut start = 0;
    for chunk in source.split_inclusive('\n') {
        let end = start + chunk.len();
        let mut content_end = if chunk.ends_with('\n') { end - 1 } else { end };
        if content_end > start && source.as_bytes()[content_end - 1] == b'\r' {
            content_end -= 1;
        }
        lines.push(scan_line(source, start, content_end, end));
        start = end;
    }
    if source.is_empty() {
        return lines;
    }
    if start < source.len() {
        lines.push(scan_line(source, start, source.len(), source.len()));
    }
    lines
}

fn scan_line(source: &str, start: usize, content_end: usize, end: usize) -> Line {
    let bytes = source.as_bytes();
    let mut cursor = start;
    while cursor < content_end && bytes[cursor] == b' ' {
        cursor += 1;
    }
    let structural_tab = (cursor < content_end && bytes[cursor] == b'\t').then_some(cursor);
    let blank = source[start..content_end]
        .bytes()
        .all(|byte| matches!(byte, b' ' | b'\t'));
    Line {
        start,
        content_end,
        end,
        indent: cursor - start,
        blank,
        structural_tab,
    }
}

fn scan_marker(source: &str, mut cursor: usize, end: usize) -> usize {
    while cursor < end {
        let character = source[cursor..].chars().next().unwrap();
        if character.is_whitespace()
            || character.is_control()
            || matches!(character, '`' | '"' | '{' | '}')
        {
            break;
        }
        cursor += character.len_utf8();
    }
    cursor
}

fn find_full_verbatim_close(
    source: &str,
    mut cursor: usize,
    end: usize,
    quote_count: usize,
) -> Option<usize> {
    while cursor < end {
        let relative = source[cursor..end].find('}')?;
        let close = cursor + relative;
        let quotes_end = close + 1 + quote_count;
        if quotes_end <= end
            && source.as_bytes()[close + 1..quotes_end]
                .iter()
                .all(|byte| *byte == b'"')
        {
            return Some(close);
        }
        cursor = close + 1;
    }
    None
}

fn project_attributes(source: &str, document: &mut Document) {
    project_block_list(source, &mut document.blocks);
    document.attrs = attributes_from_blocks(source, &document.blocks);
}

fn project_block_list(source: &str, blocks: &mut [Block]) {
    for block in blocks {
        let Block::Parsed(block) = block else {
            continue;
        };
        project_inline_attributes(source, &mut block.content);
        project_block_list(source, &mut block.children);
        let attrs = attributes_from_blocks(source, &block.children);
        if let Some(mark) = &mut block.mark {
            mark.attrs = attrs;
        }
    }
}

fn project_inline_attributes(source: &str, content: &mut InlineContent) {
    for inline in &mut content.items {
        let Inline::Group {
            mark,
            content: nested,
            ..
        } = inline
        else {
            continue;
        };
        project_inline_attributes(source, nested);
        let attrs = attributes_from_inlines(source, nested);
        if let Some(mark) = mark {
            mark.attrs = attrs;
        }
    }
}

fn attributes_from_blocks(source: &str, blocks: &[Block]) -> Attributes {
    attributes_from_items(blocks.iter().filter_map(|block| {
        let Block::Parsed(block) = block else {
            return None;
        };
        (!block.children.is_empty())
            .then_some(None)
            .unwrap_or_else(|| {
                declaration_from_content(source, block.mark.as_ref()?, &block.content)
            })
    }))
}

fn attributes_from_inlines(source: &str, content: &InlineContent) -> Attributes {
    attributes_from_items(content.data.iter().filter_map(|datum| {
        let items = &content.items[datum.item_range.clone()];
        let [Inline::Group {
            mark: Some(mark),
            content,
            ..
        }] = items
        else {
            return None;
        };
        declaration_from_content(source, mark, content)
    }))
}

fn attributes_from_items(items: impl Iterator<Item = AttrItem>) -> Attributes {
    let items = items.collect::<Vec<_>>();
    let range = match (items.first(), items.last()) {
        (Some(first), Some(last)) => attr_range(first).start..attr_range(last).end,
        _ => return Attributes::default(),
    };
    Attributes {
        range: Some(range),
        items,
    }
}

fn declaration_from_content(
    source: &str,
    mark: &Mark,
    content: &InlineContent,
) -> Option<AttrItem> {
    match mark.marker.as_str() {
        "@" | "+" if content.data.len() == 1 => {
            let value = content.datum(0)?;
            let value_text = plain_scalar(&value)?;
            let value_range = value.range.clone();
            let range = mark.range.start..value_range.end;
            if mark.marker == "@" {
                Some(AttrItem::Id {
                    value: value_text,
                    value_range,
                    range,
                })
            } else {
                Some(AttrItem::Class {
                    value: value_text,
                    value_range,
                    range,
                })
            }
        }
        "=" if content.data.len() >= 2 => {
            let key_content = content.datum(0)?;
            let key = plain_scalar(&key_content)?;
            if key.is_empty() {
                return None;
            }
            let first_value = content.data.get(1)?;
            let last_value = content.data.last()?;
            let value_range = first_value.range.start..last_value.range.end;
            let value = content_for_range(content, 1, content.data.len()).plain_text();
            Some(AttrItem::Pair {
                key,
                key_range: key_content.range.clone(),
                value: AttrValue {
                    decoded: value,
                    raw: source[value_range.clone()].to_string(),
                    range: value_range.clone(),
                    quoted: matches!(
                        content_for_range(content, 1, content.data.len())
                            .items
                            .as_slice(),
                        [Inline::Verbatim { .. }]
                    ),
                },
                range: mark.range.start..value_range.end,
            })
        }
        _ => None,
    }
}

fn content_for_range(content: &InlineContent, start: usize, end: usize) -> InlineContent {
    let first = &content.data[start];
    let last = &content.data[end - 1];
    InlineContent::from_items(
        first.range.start..last.range.end,
        content.items[first.item_range.start..last.item_range.end].to_vec(),
    )
}

fn plain_scalar(content: &InlineContent) -> Option<String> {
    content
        .items
        .iter()
        .all(|inline| {
            matches!(
                inline,
                Inline::Text { .. } | Inline::Space { .. } | Inline::Verbatim { mark: None, .. }
            )
        })
        .then(|| content.plain_text())
}

fn attr_range(item: &AttrItem) -> &SourceRange {
    match item {
        AttrItem::Id { range, .. }
        | AttrItem::Class { range, .. }
        | AttrItem::Pair { range, .. } => range,
    }
}
