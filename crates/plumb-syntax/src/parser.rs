use std::collections::HashMap;

use crate::lossless::build_lossless;
use crate::syntax::*;

pub fn parse(source: impl Into<String>) -> ParsedDocument {
    let source = source.into();
    let lines = Lines::new(&source);
    let mut parser = Parser {
        source: &source,
        lines,
        diagnostics: Vec::new(),
    };
    let (attrs, body_start) = parser.parse_root_attached();
    let (blocks, _) = parser.parse_blocks(body_start, 0);
    let syntax = Document {
        attrs,
        blocks,
        range: 0..source.len(),
    };
    let diagnostics = normalize_diagnostics(parser.diagnostics);
    let lossless = build_lossless(&source, &syntax, &diagnostics);
    ParsedDocument {
        source,
        lossless,
        syntax,
        diagnostics,
    }
}

fn normalize_diagnostics(mut diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
    // The most specific error wins: generic bare-delimiter errors contained
    // in an invalid document group range share its root cause.
    let document_group_ranges: Vec<_> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "syntax.invalid-document-group")
        .map(|diagnostic| diagnostic.range.clone())
        .collect();
    if !document_group_ranges.is_empty() {
        diagnostics.retain(|diagnostic| {
            !(matches!(
                diagnostic.code,
                "syntax.unattached-group" | "syntax.unexpected-group-close"
            ) && document_group_ranges
                .iter()
                .any(|range| range.contains(&diagnostic.range.start)
                    && range.contains(&diagnostic.range.end.saturating_sub(1))))
        });
    }
    let preferred = diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.code, diagnostic.range.clone()))
        .collect::<Vec<_>>();
    diagnostics.retain(|diagnostic| {
        !preferred.iter().any(|(code, range)| {
            range == &diagnostic.range
                && matches!(
                    (*code, diagnostic.code),
                    (
                        "syntax.incomplete-introducer",
                        "syntax.invalid-inline-dispatch"
                    ) | ("syntax.incomplete-introducer", "syntax.invalid-marker")
                        | ("syntax.invalid-marker", "syntax.invalid-block-dispatch")
                        | ("syntax.short-verbatim-indent", "syntax.partial-indent")
                )
        })
    });
    diagnostics
}

fn project_block_attributes(source: &str, blocks: &[Block]) -> Vec<AttrItem> {
    blocks
        .iter()
        .filter_map(|block| {
            let Block::Parsed(block) = block else {
                return None;
            };
            let mark = block.mark.as_ref()?;
            if mark.marker == "-" {
                let value = block.head.plain_text();
                if value.is_empty() {
                    return None;
                }
                return Some(AttrItem::Class {
                    value,
                    range: block.head.range.clone(),
                });
            }
            if mark.marker == "@" {
                let value = block.head.plain_text();
                if value.is_empty() {
                    return None;
                }
                return Some(AttrItem::Id {
                    value,
                    range: block.range.clone(),
                });
            }
            if mark.marker != ":" {
                return None;
            }
            let (key, key_range, value_range) = association_parts(&block.head)?;
            let value = block.head.items[2..]
                .iter()
                .fold(String::new(), |mut output, inline| {
                    append_inline_plain_text(inline, &mut output);
                    output
                });
            Some(AttrItem::Pair {
                key,
                key_range,
                value: AttrValue {
                    decoded: value,
                    raw: source[value_range.clone()].to_string(),
                    range: value_range,
                    quoted: true,
                },
                range: block.range.clone(),
            })
        })
        .collect()
}

fn project_inline_attributes(source: &str, content: &InlineContent) -> Vec<AttrItem> {
    content
        .items
        .iter()
        .filter_map(|inline| {
            let Inline::Element {
                range,
                kind,
                kind_range: _,
                content,
                ..
            } = inline
            else {
                return None;
            };
            if kind == "-" {
                let value = content.plain_text();
                if value.is_empty() {
                    return None;
                }
                return Some(AttrItem::Class {
                    value,
                    range: content.range.clone(),
                });
            }
            if kind == "@" {
                let value = content.plain_text();
                if value.is_empty() {
                    return None;
                }
                return Some(AttrItem::Id {
                    value,
                    range: range.clone(),
                });
            }
            if kind != ":" {
                return None;
            }
            let (key, key_range, value_range) = association_parts(content)?;
            let value = content.items[2..]
                .iter()
                .fold(String::new(), |mut output, inline| {
                    append_inline_plain_text(inline, &mut output);
                    output
                });
            Some(AttrItem::Pair {
                key,
                key_range,
                value: AttrValue {
                    decoded: value,
                    raw: source[value_range.clone()].to_string(),
                    range: value_range,
                    quoted: true,
                },
                range: range.clone(),
            })
        })
        .collect()
}

fn association_parts(content: &InlineContent) -> Option<(String, SourceRange, SourceRange)> {
    let [key, Inline::Space { .. }, value @ ..] = content.items.as_slice() else {
        return None;
    };
    if value.is_empty() {
        return None;
    }
    let (key, key_range) = match key {
        Inline::Text { text, range } | Inline::Verbatim { text, range, .. } if !text.is_empty() => {
            (text.clone(), range.clone())
        }
        Inline::Element {
            kind,
            content,
            range,
            ..
        } if kind == "()" => (content.plain_text(), range.clone()),
        _ => return None,
    };
    if key.is_empty() {
        return None;
    }
    Some((
        key,
        key_range,
        inline_range(value.first()?).start..inline_range(value.last()?).end,
    ))
}

fn inline_range(inline: &Inline) -> &SourceRange {
    match inline {
        Inline::Text { range, .. }
        | Inline::Space { range, .. }
        | Inline::SoftBreak { range }
        | Inline::Element { range, .. }
        | Inline::Verbatim { range, .. } => range,
    }
}

fn append_inline_plain_text(inline: &Inline, output: &mut String) {
    match inline {
        Inline::Text { text, .. } | Inline::Space { text, .. } | Inline::Verbatim { text, .. } => {
            output.push_str(text)
        }
        Inline::SoftBreak { .. } => output.push(' '),
        Inline::Element { content, .. } => output.push_str(&content.plain_text()),
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

#[derive(Debug, Clone, Copy)]
struct InlineSegment {
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, Copy)]
struct InlinePosition {
    segment: usize,
    offset: usize,
}

struct InlineOpening {
    introducer: usize,
    kind: String,
    kind_range: SourceRange,
}

struct InlineFrame {
    start: usize,
    text_start: usize,
    items: Vec<Inline>,
    opening: Option<InlineOpening>,
}

struct BlockFrame {
    cursor: usize,
    indent: usize,
    same_line: bool,
    blocks: Vec<Block>,
    owner: Option<ParsedBlock>,
}

struct MarkedParse {
    block: ParsedBlock,
    next: usize,
    child: Option<(usize, usize, bool)>,
}

fn finish_block_frame(frames: &mut Vec<BlockFrame>, next: usize) -> Option<(Vec<Block>, usize)> {
    let frame = frames.pop().expect("block parser always has a frame");
    let Some(mut owner) = frame.owner else {
        return Some((frame.blocks, next));
    };
    owner.children = frame.blocks;
    if let Some(last) = owner.children.last() {
        owner.range.end = last.range().end;
    }
    let parent = frames
        .last_mut()
        .expect("an owner frame always has a parent sequence");
    parent.blocks.push(Block::Parsed(owner));
    parent.cursor = next;
    None
}

struct Lines(Vec<Line>);

impl Lines {
    fn new(source: &str) -> Self {
        let mut output = Vec::new();
        let mut start = 0;
        for chunk in source.split_inclusive('\n') {
            let end = start + chunk.len();
            let mut content_end = if chunk.ends_with('\n') { end - 1 } else { end };
            if content_end > start && source.as_bytes()[content_end - 1] == b'\r' {
                content_end -= 1;
            }
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
    fn parse_root_attached(&mut self) -> (Attributes, usize) {
        let Some(index) = self.lines.0.iter().position(|line| !line.blank) else {
            return (Attributes::default(), 0);
        };
        let line = &self.lines.0[index];
        let content = self.source[line.start + line.indent..line.content_end].trim_end();
        if line.indent == 0 && content == "{}" {
            self.diagnostics.push(Diagnostic::error(
                "syntax.invalid-document-group",
                "document attached group must use expanded form",
                line.start..line.start + 2,
            ));
            return (Attributes::default(), 0);
        }
        if !self.is_group_delimiter(index, 0, b'{') {
            return (Attributes::default(), 0);
        }
        self.parse_expanded_attached(index, line.start, 0)
    }

    /// Parses an expanded attached group whose opening brace sits at byte
    /// `open` on line `opener_index`. `opener_column` is the structural
    /// column of the opener's line: group content must be deeper, and the
    /// closing brace returns to it.
    fn parse_expanded_attached(
        &mut self,
        opener_index: usize,
        open: usize,
        opener_column: usize,
    ) -> (Attributes, usize) {
        let mut content_start = opener_index + 1;
        while self
            .lines
            .0
            .get(content_start)
            .is_some_and(|line| line.blank)
        {
            content_start += 1;
        }
        let (blocks, next) = if self.is_group_delimiter(content_start, opener_column, b'}') {
            (Vec::new(), content_start)
        } else if self
            .lines
            .0
            .get(content_start)
            .is_some_and(|line| line.indent > opener_column)
        {
            let content_indent = self.lines.0[content_start].indent;
            self.parse_blocks(content_start, content_indent)
        } else {
            (Vec::new(), content_start)
        };
        let (close_range, after, range_end) = if self.is_group_delimiter(next, opener_column, b'}')
        {
            let line = &self.lines.0[next];
            let start = line.start + opener_column;
            (start..start + 1, next + 1, line.end)
        } else {
            let end = self
                .lines
                .0
                .get(next)
                .map_or(self.source.len(), |line| line.start + line.indent);
            self.diagnostics.push(Diagnostic::error(
                "syntax.unclosed-attached-group",
                "expanded attached group must close with '}' at the opener line's column",
                open..end,
            ));
            (end..end, next, end)
        };
        // The opener is the sole structure of its line exactly when it sits
        // at the line's structural column; a trailing opener always has the
        // head and its separator before the brace.
        let opener_line = &self.lines.0[opener_index];
        let opener_on_own_line = open == opener_line.start + opener_line.indent;
        let range_start = if opener_on_own_line {
            self.lines.0[opener_index].start + opener_column
        } else {
            open
        };
        let range = range_start..range_end;
        let items = project_block_attributes(self.source, &blocks);
        (
            Attributes {
                range: Some(range.clone()),
                items,
                attached: Some(Box::new(AttachedGroup {
                    range,
                    open_range: open..open + 1,
                    close_range,
                    opener_on_own_line,
                    content: AttachedContent::Blocks(blocks),
                })),
            },
            after,
        )
    }

    fn is_group_delimiter(&self, index: usize, indent: usize, delimiter: u8) -> bool {
        let Some(line) = self.lines.0.get(index) else {
            return false;
        };
        let delimiter_offset = line.start + indent;
        !line.blank
            && !line.has_tab_indent
            && line.indent == indent
            && self.source.as_bytes()[delimiter_offset] == delimiter
            && self.source[delimiter_offset + 1..line.content_end]
                .bytes()
                .all(|byte| matches!(byte, b' ' | b'\t'))
    }

    fn parse_blocks(&mut self, mut index: usize, indent: usize) -> (Vec<Block>, usize) {
        let mut frames = vec![BlockFrame {
            cursor: index,
            indent,
            same_line: false,
            blocks: Vec::new(),
            owner: None,
        }];
        loop {
            index = frames.last().unwrap().cursor;
            if index >= self.lines.0.len() {
                if let Some(result) = finish_block_frame(&mut frames, index) {
                    return result;
                }
                continue;
            }
            let current = &self.lines.0[index];
            if current.blank {
                frames.last_mut().unwrap().cursor += 1;
                continue;
            }
            let expected_indent = frames.last().unwrap().indent;
            let same_line = frames.last().unwrap().same_line;
            if !same_line && current.indent < expected_indent {
                if let Some(result) = finish_block_frame(&mut frames, index) {
                    return result;
                }
                continue;
            }
            if !same_line && current.has_tab_indent {
                self.diagnostics.push(Diagnostic::error(
                    "syntax.tab-indentation",
                    "tabs are not allowed in structural indentation",
                    current.start..current.start + current.indent + 1,
                ));
            }
            if !same_line && current.indent > expected_indent {
                self.diagnostics.push(Diagnostic::error(
                    "syntax.partial-indent",
                    format!("expected indentation column {expected_indent}"),
                    current.start..current.start + current.indent,
                ));
            }

            let effective_indent = if same_line {
                frames.last_mut().unwrap().same_line = false;
                expected_indent
            } else {
                current.indent
            };
            if let Some(kind) = self.block_dispatch(index, effective_indent) {
                match kind {
                    BlockDispatch::Marked => {
                        let parsed = self.parse_marked(index, effective_indent);
                        if let Some((child_index, child_indent, same_line)) = parsed.child {
                            frames.push(BlockFrame {
                                cursor: child_index,
                                indent: child_indent,
                                same_line,
                                blocks: Vec::new(),
                                owner: Some(parsed.block),
                            });
                        } else {
                            let frame = frames.last_mut().unwrap();
                            frame.blocks.push(Block::Parsed(parsed.block));
                            frame.cursor = parsed.next;
                        }
                    }
                    BlockDispatch::Verbatim => {
                        let (block, next) = self.parse_verbatim(index, effective_indent);
                        let frame = frames.last_mut().unwrap();
                        frame.blocks.push(Block::Verbatim(block));
                        frame.cursor = next;
                    }
                }
            } else {
                let (block, next) = self.parse_paragraph(index, effective_indent);
                let frame = frames.last_mut().unwrap();
                frame.blocks.push(Block::Parsed(block));
                frame.cursor = next;
            }
        }
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
            return Some(BlockDispatch::Marked);
        }
        let byte = self.source.as_bytes()[after];
        if byte == b'}'
            || (byte == b'{' && self.source.as_bytes().get(after + 1).copied() != Some(b'}'))
        {
            return None;
        }
        if byte == b'[' {
            return None;
        }
        let kind_end = take_name_like(self.source, after, line.content_end, marker_char);
        if kind_end < line.content_end && self.source.as_bytes()[kind_end] == b'"' {
            let quote_count = self.source[kind_end..line.content_end]
                .bytes()
                .take_while(|byte| *byte == b'"')
                .count();
            let tail = kind_end + quote_count;
            if tail == line.content_end {
                return Some(BlockDispatch::Verbatim);
            }
            if self.source.as_bytes()[tail] == b'[' {
                return None;
            }
            let mut group_start = tail;
            while group_start < line.content_end
                && matches!(self.source.as_bytes()[group_start], b' ' | b'\t')
            {
                group_start += 1;
            }
            if group_start > tail && self.source.as_bytes().get(group_start) == Some(&b'{') {
                return Some(BlockDispatch::Verbatim);
            }
            if quote_count == 1 && self.source[tail..line.content_end].contains('"') {
                return None;
            }
            return Some(BlockDispatch::Verbatim);
        }
        if kind_end < line.content_end && self.source.as_bytes()[kind_end] == b'[' {
            return None;
        }
        Some(BlockDispatch::Marked)
    }

    fn parse_marked(&mut self, index: usize, indent: usize) -> MarkedParse {
        let mut line = self.lines.0[index].clone();
        let mut header_index = index;
        let introducer = line.start + indent;
        let mut cursor = introducer + 1;
        let marker_start = cursor;
        cursor = take_name_like(self.source, cursor, line.content_end, marker_char);
        if cursor == marker_start {
            self.diagnostics.push(Diagnostic::error(
                "syntax.invalid-marker",
                "invalid or missing marker token",
                introducer..next_char_end(self.source, cursor).min(line.content_end),
            ));
        }
        let marker = self.source[marker_start..cursor].to_string();
        let marker_range = marker_start..cursor;

        let mut attrs = if cursor < line.content_end && self.source.as_bytes()[cursor] == b'{' {
            self.diagnostics.push(Diagnostic::error(
                "syntax.legacy-attributes",
                "legacy attribute slots are not part of the current syntax",
                cursor..line.content_end,
            ));
            let diagnostic_count = self.diagnostics.len();
            let (limit, _) = self.attribute_extent(cursor, index, indent);
            let (_, next) = self.parse_attributes(cursor, limit);
            self.diagnostics.truncate(diagnostic_count);
            cursor = next;
            header_index = self
                .line_index_at_offset(cursor)
                .expect("attribute cursor remains on a source line");
            line = self.lines.0[header_index].clone();
            Attributes::default()
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

        if head_start < line.content_end {
            let child_indent = head_start - line.start;
            if self.block_dispatch(header_index, child_indent).is_some() {
                return MarkedParse {
                    block: ParsedBlock {
                        range: introducer..line.end,
                        mark: Some(Mark {
                            range: introducer..mark_end,
                            marker,
                            marker_range,
                            attrs,
                        }),
                        head: InlineContent {
                            range: head_start..head_start,
                            items: Vec::new(),
                        },
                        children: Vec::new(),
                    },
                    next: header_index + 1,
                    child: Some((header_index, child_indent, true)),
                };
            }
        }

        let header_attached = block_attached_start(self.source, head_start, line.content_end);
        let head_end = header_attached.map_or(line.content_end, |(separator, _)| separator);
        // The attached-group opener is the last structure of the complete
        // head: it either trails the header line or occupies a head
        // continuation line on its own. Both shapes produce the same opener
        // description (opener line index, brace offset).
        let mut head_opener = header_attached.map(|(_, group_start)| (header_index, group_start));
        let mut head_segments = vec![InlineSegment {
            start: head_start,
            end: head_end,
        }];
        let mut next = header_index + 1;
        let mut saw_blank = false;
        let mut body_indent = None;
        let mut attached_body_indent = None;

        while head_opener.is_none() && next < self.lines.0.len() {
            let candidate = self.lines.0[next].clone();
            if candidate.blank {
                saw_blank = true;
                next += 1;
                continue;
            }
            if candidate.indent <= indent {
                break;
            }
            if !saw_blank && body_indent.is_none_or(|column| column == candidate.indent) {
                let group_start = candidate.start + candidate.indent;
                if self.is_group_delimiter(next, candidate.indent, b'{') {
                    head_opener = Some((next, group_start));
                    break;
                }
                if self.source.as_bytes()[group_start] == b'{' {
                    self.diagnostics.push(Diagnostic::error(
                        "syntax.trailing-after-attached-group",
                        "a head continuation line opener holds only the brace",
                        group_start + 1..candidate.content_end,
                    ));
                    break;
                }
            }
            body_indent.get_or_insert(candidate.indent);
            if candidate.indent != body_indent.unwrap() {
                break;
            }
            if !saw_blank && self.block_dispatch(next, candidate.indent).is_none() {
                // A trailing opener can end any head line: the brace is
                // spaced off the line's head source and cuts the head there.
                match block_attached_start(
                    self.source,
                    candidate.start + candidate.indent,
                    candidate.content_end,
                ) {
                    Some((separator, group_start)) => {
                        head_segments.push(InlineSegment {
                            start: candidate.start + candidate.indent,
                            end: separator,
                        });
                        head_opener = Some((next, group_start));
                    }
                    None => {
                        head_segments.push(InlineSegment {
                            start: candidate.start + candidate.indent,
                            end: candidate.content_end,
                        });
                        next += 1;
                    }
                }
                continue;
            }
            break;
        }
        let head = self.parse_inline_segments(&mut head_segments, false);
        if let Some((opener_index, group_start)) = head_opener {
            let opener_line = &self.lines.0[opener_index];
            let opener_column = if opener_index == header_index {
                indent
            } else {
                opener_line.indent
            };
            let content_end = opener_line.content_end;
            if find_inline_group_close(self.source, group_start + 1, content_end).is_some() {
                // Compact form: only a trailing opener reaches here; an
                // own-line opener holds nothing but the brace.
                let mut position = InlinePosition {
                    segment: 0,
                    offset: group_start,
                };
                let (group, after_group) =
                    self.parse_inline_attached(&mut position, group_start, content_end);
                self.diagnose_block_group_trailer(group_start, after_group, content_end);
                attrs = group;
            } else if self.source[group_start + 1..content_end]
                .trim_matches([' ', '\t'])
                .is_empty()
            {
                let (group, after_group) =
                    self.parse_expanded_attached(opener_index, group_start, opener_column);
                attrs = group;
                next = after_group;
                if opener_index > header_index {
                    // An own-line opener establishes the body column shared
                    // by the close and later child siblings.
                    attached_body_indent = Some(opener_column);
                }
            } else {
                let mut position = InlinePosition {
                    segment: 0,
                    offset: group_start,
                };
                let (group, _) =
                    self.parse_inline_attached(&mut position, group_start, content_end);
                attrs = group;
            }
        }
        if let Some(consumed) = self.line_index_at_offset(head.range.end.saturating_sub(1)) {
            next = next.max(consumed + 1);
        }

        let mut child_start = next;
        while child_start < self.lines.0.len() && self.lines.0[child_start].blank {
            child_start += 1;
        }
        let child = (child_start < self.lines.0.len() && self.lines.0[child_start].indent > indent)
            .then(|| {
                (
                    child_start,
                    attached_body_indent.unwrap_or(self.lines.0[child_start].indent),
                    false,
                )
            });
        let end = next
            .checked_sub(1)
            .and_then(|i| self.lines.0.get(i).map(|line| line.end))
            .unwrap_or(line.end);

        MarkedParse {
            block: ParsedBlock {
                range: introducer..end,
                mark: Some(Mark {
                    range: introducer..mark_end,
                    marker,
                    marker_range,
                    attrs,
                }),
                head,
                children: Vec::new(),
            },
            next,
            child,
        }
    }

    fn parse_verbatim(&mut self, index: usize, indent: usize) -> (VerbatimBlock, usize) {
        let line = self.lines.0[index].clone();
        let introducer = line.start + indent;
        let attr_start = introducer + 1;
        let kind_end = take_name_like(self.source, attr_start, line.content_end, marker_char);
        let kind_range = attr_start..kind_end;
        let quote_count = self.source[kind_end..line.content_end]
            .bytes()
            .take_while(|byte| *byte == b'"')
            .count();
        debug_assert!(quote_count > 0);
        let after_quote = kind_end + quote_count;
        let mut group_start = after_quote;
        while group_start < line.content_end
            && matches!(self.source.as_bytes()[group_start], b' ' | b'\t')
        {
            group_start += 1;
        }
        let (attrs, body_start) = if group_start < line.content_end
            && group_start > after_quote
            && self.source.as_bytes()[group_start] == b'{'
            && find_inline_group_close(self.source, group_start + 1, line.content_end).is_some()
        {
            let mut position = InlinePosition {
                segment: 0,
                offset: group_start,
            };
            let (attrs, after_group) =
                self.parse_inline_attached(&mut position, group_start, line.content_end);
            self.diagnose_block_group_trailer(group_start, after_group, line.content_end);
            (attrs, index + 1)
        } else if group_start < line.content_end
            && group_start > after_quote
            && self.source.as_bytes()[group_start] == b'{'
            && self.source[group_start + 1..line.content_end]
                .trim_matches([' ', '\t'])
                .is_empty()
        {
            self.parse_expanded_attached(index, group_start, indent)
        } else {
            (Attributes::default(), index + 1)
        };
        if group_start < line.content_end
            && (group_start == after_quote || self.source.as_bytes()[group_start] != b'{')
        {
            self.diagnostics.push(Diagnostic::error(
                "syntax.invalid-verbatim-block-dispatch",
                "verbatim block header must end after its attached group",
                introducer..line.content_end,
            ));
        }
        let body_indent = indent + quote_count;
        let mut next = body_start;
        let mut text = String::new();
        let text_start = self.lines.0.get(next).map_or(line.end, |next| next.start);
        let mut text_end = text_start;
        while next < self.lines.0.len() {
            let candidate = &self.lines.0[next];
            if candidate.blank {
                let mut after_blank = next;
                while self.lines.0.get(after_blank).is_some_and(|line| line.blank) {
                    after_blank += 1;
                }
                let blank_run_is_internal = self
                    .lines
                    .0
                    .get(after_blank)
                    .is_some_and(|line| line.indent >= body_indent);
                if blank_run_is_internal {
                    while next < after_blank {
                        let blank = &self.lines.0[next];
                        text.push_str(&self.source[blank.content_end..blank.end]);
                        text_end = blank.end;
                        next += 1;
                    }
                    continue;
                }
                while next < after_blank {
                    let blank = &self.lines.0[next];
                    if !has_space_prefix(self.source, blank.start, body_indent) {
                        break;
                    }
                    text.push_str(&self.source[blank.start + body_indent..blank.content_end]);
                    text.push_str(&self.source[blank.content_end..blank.end]);
                    text_end = blank.end;
                    next += 1;
                }
                if next < after_blank {
                    break;
                }
                continue;
            }
            if !candidate.blank && candidate.indent < body_indent {
                if candidate.indent > indent {
                    self.diagnostics.push(Diagnostic::error(
                        "syntax.short-verbatim-indent",
                        format!(
                            "verbatim payload requires {quote_count} structural spaces after the owner indentation"
                        ),
                        candidate.start..candidate.start + candidate.indent,
                    ));
                }
                break;
            }
            let content = candidate.start + body_indent;
            if content > candidate.content_end {
                self.diagnostics.push(Diagnostic::error(
                    "syntax.short-verbatim-indent",
                    format!(
                        "verbatim payload requires {quote_count} structural spaces after the owner indentation"
                    ),
                    candidate.start..candidate.content_end,
                ));
            } else {
                text.push_str(&self.source[content..candidate.content_end]);
                text.push_str(&self.source[candidate.content_end..candidate.end]);
            }
            text_end = candidate.end;
            next += 1;
        }
        (
            VerbatimBlock {
                range: introducer..text_end.max(line.end),
                opener_range: introducer..after_quote,
                kind: self.source[kind_range.clone()].to_string(),
                kind_range,
                quote_count,
                attrs,
                text,
                text_range: text_start..text_end,
            },
            next,
        )
    }

    fn diagnose_block_group_trailer(&mut self, open: usize, after: usize, limit: usize) {
        let mut cursor = after;
        while cursor < limit && matches!(self.source.as_bytes()[cursor], b' ' | b'\t') {
            cursor += 1;
        }
        if cursor == limit {
            return;
        }
        if self.source.as_bytes()[cursor] == b'{' {
            let mut diagnostic = Diagnostic::error(
                "syntax.duplicate-attached-group",
                "an owner may have at most one attached group",
                cursor..cursor + 1,
            );
            diagnostic.related.push(open..open + 1);
            self.diagnostics.push(diagnostic);
        } else {
            self.diagnostics.push(Diagnostic::error(
                "syntax.trailing-after-attached-group",
                "only horizontal whitespace may follow a compact block attached group",
                cursor..limit,
            ));
        }
    }

    fn parse_paragraph(&mut self, index: usize, indent: usize) -> (ParsedBlock, usize) {
        let first = self.lines.0[index].clone();
        let start = first.start + indent;
        if self.is_group_delimiter(index, indent, b'{') {
            self.diagnostics.push(Diagnostic::error(
                "syntax.unattached-group",
                "an attached group must immediately follow its owner header",
                start..start + 1,
            ));
        } else if self.is_group_delimiter(index, indent, b'}') {
            self.diagnostics.push(Diagnostic::error(
                "syntax.unexpected-group-close",
                "attached group close has no matching opener",
                start..start + 1,
            ));
        }
        let mut head_segments = vec![InlineSegment {
            start,
            end: first.content_end,
        }];
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
            head_segments.push(InlineSegment {
                start: candidate.start + indent,
                end: candidate.content_end,
            });
            end = candidate.end;
            next += 1;
        }
        let diagnostic_start = self.diagnostics.len();
        let head = loop {
            let head = self.parse_inline_segments(&mut head_segments, false);
            let Some(consumed) = self.line_index_at_offset(head.range.end.saturating_sub(1)) else {
                break head;
            };
            next = next.max(consumed + 1);
            end = self.lines.0[consumed].end;
            let mut scan = next;
            let mut extended = false;
            while let Some(candidate) = self.lines.0.get(scan).cloned() {
                if candidate.blank
                    || candidate.indent != indent
                    || self.block_dispatch(scan, indent).is_some()
                {
                    break;
                }
                head_segments.push(InlineSegment {
                    start: candidate.start + indent,
                    end: candidate.content_end,
                });
                end = candidate.end;
                scan += 1;
                extended = true;
            }
            if !extended {
                break head;
            }
            next = scan;
            head_segments.sort_by_key(|segment| segment.start);
            head_segments.dedup_by_key(|segment| segment.start);
            self.diagnostics.truncate(diagnostic_start);
        };
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

    fn parse_inline_segments(
        &mut self,
        segments: &mut Vec<InlineSegment>,
        _group_content: bool,
    ) -> InlineContent {
        let start = segments.first().map_or(0, |segment| segment.start);
        let mut position = InlinePosition {
            segment: 0,
            offset: start,
        };
        let mut frames = vec![InlineFrame {
            start,
            text_start: start,
            items: Vec::new(),
            opening: None,
        }];

        while position.segment < segments.len() {
            let segment = segments[position.segment];
            if position.offset >= segment.end {
                flush_inline_text(self.source, frames.last_mut().unwrap(), position.offset);
                let previous_end = segment.end;
                position.segment += 1;
                let Some(next) = segments.get(position.segment) else {
                    break;
                };
                let nested = frames.len() > 1;
                let frame = frames.last_mut().unwrap();
                if nested || !frame.items.is_empty() {
                    frame.items.push(Inline::SoftBreak {
                        range: previous_end..next.start,
                    });
                }
                position.offset = next.start;
                frame.text_start = position.offset;
                continue;
            }

            let end = segment.end;
            let cursor = position.offset;
            let byte = self.source.as_bytes()[cursor];
            // §2: bare delimiters never fall back to text. An unescaped
            // opening brace must open an attached group, an unescaped
            // closing brace must close one, and brackets are legal only in
            // the introducer dispatch or when closing an open element.
            if byte == b'{' {
                flush_inline_text(self.source, frames.last_mut().unwrap(), cursor);
                self.diagnostics.push(Diagnostic::error(
                    "syntax.unattached-group",
                    "an opening brace must be escaped or open an attached group",
                    cursor..cursor + 1,
                ));
                position.offset += 1;
                frames.last_mut().unwrap().text_start = position.offset;
                continue;
            }
            if byte == b'}' {
                flush_inline_text(self.source, frames.last_mut().unwrap(), cursor);
                self.diagnostics.push(Diagnostic::error(
                    "syntax.unexpected-group-close",
                    "group close has no matching opener",
                    cursor..cursor + 1,
                ));
                position.offset += 1;
                frames.last_mut().unwrap().text_start = position.offset;
                continue;
            }
            if byte == b'[' {
                flush_inline_text(self.source, frames.last_mut().unwrap(), cursor);
                self.diagnostics.push(Diagnostic::error(
                    "syntax.unattached-bracket",
                    "an opening bracket must follow a nonempty inline kind or be escaped",
                    cursor..cursor + 1,
                ));
                position.offset += 1;
                frames.last_mut().unwrap().text_start = position.offset;
                continue;
            }
            if byte == b']' && frames.len() == 1 {
                flush_inline_text(self.source, frames.last_mut().unwrap(), cursor);
                self.diagnostics.push(Diagnostic::error(
                    "syntax.unexpected-element-close",
                    "inline element close has no matching opener",
                    cursor..cursor + 1,
                ));
                position.offset += 1;
                frames.last_mut().unwrap().text_start = position.offset;
                continue;
            }
            if frames.len() > 1 && byte == b']' {
                flush_inline_text(self.source, frames.last_mut().unwrap(), cursor);
                position.offset += 1;
                let after_close = position.offset;
                let (attrs, after_attrs) =
                    if after_close < end && self.source.as_bytes()[after_close] == b'{' {
                        self.parse_inline_postfix(segments, &mut position, after_close, end)
                    } else {
                        (Attributes::default(), after_close)
                    };
                position.offset = after_attrs;
                let frame = frames.pop().unwrap();
                let opening = frame.opening.unwrap();
                let content = InlineContent {
                    range: frame.start..cursor,
                    items: frame.items,
                };
                frames.last_mut().unwrap().items.push(Inline::Element {
                    range: opening.introducer..after_attrs,
                    kind: opening.kind,
                    kind_range: opening.kind_range,
                    content,
                    attrs,
                });
                frames.last_mut().unwrap().text_start = after_attrs;
                continue;
            }
            if byte != b'`' {
                position.offset = next_char_end(self.source, cursor);
                continue;
            }

            flush_inline_text(self.source, frames.last_mut().unwrap(), cursor);
            let ticks = self.source[cursor..end]
                .bytes()
                .take_while(|candidate| *candidate == b'`')
                .count();
            for pair in 0..ticks / 2 {
                let pair_start = cursor + pair * 2;
                frames.last_mut().unwrap().items.push(Inline::Text {
                    text: "`".to_string(),
                    range: pair_start..pair_start + 2,
                });
            }
            position.offset += (ticks / 2) * 2;
            if ticks % 2 == 0 {
                frames.last_mut().unwrap().text_start = position.offset;
                continue;
            }

            let introducer = position.offset;
            position.offset += 1;
            // §2: a single introducer before any of the four structural
            // delimiters is an unconditional literal escape.
            if position.offset < end
                && matches!(
                    self.source.as_bytes()[position.offset],
                    b'{' | b'}' | b'[' | b']'
                )
            {
                frames.last_mut().unwrap().items.push(Inline::Text {
                    text: self.source[position.offset..position.offset + 1].to_string(),
                    range: introducer..position.offset + 1,
                });
                position.offset += 1;
                frames.last_mut().unwrap().text_start = position.offset;
                continue;
            }

            let kind_start = position.offset;
            position.offset = take_name_like(self.source, position.offset, end, marker_char);
            let kind_end = position.offset;
            let kind = self.source[kind_start..kind_end].to_string();

            let quote_count = self.source[position.offset..end]
                .bytes()
                .take_while(|candidate| *candidate == b'"')
                .count();
            let bracket_open = position.offset + quote_count;
            if quote_count > 0 && bracket_open < end && self.source.as_bytes()[bracket_open] == b'['
            {
                if let Some((close, after_close)) =
                    find_verbatim_close(self.source, bracket_open + 1, end, quote_count)
                {
                    let (attrs, after_attrs) =
                        if after_close < end && self.source.as_bytes()[after_close] == b'{' {
                            self.parse_inline_postfix(segments, &mut position, after_close, end)
                        } else {
                            (Attributes::default(), after_close)
                        };
                    frames.last_mut().unwrap().items.push(Inline::Verbatim {
                        range: introducer..after_attrs,
                        kind,
                        kind_range: kind_start..kind_end,
                        text: self.source[bracket_open + 1..close].to_string(),
                        text_range: bracket_open + 1..close,
                        quote_count,
                        bracketed: true,
                        attrs,
                    });
                    position.offset = after_attrs;
                    frames.last_mut().unwrap().text_start = after_attrs;
                    continue;
                }
                self.diagnostics.push(Diagnostic::error(
                    "syntax.unclosed-verbatim",
                    "inline verbatim must close on the same physical line",
                    introducer..end,
                ));
                position.offset = end;
                frames.last_mut().unwrap().text_start = end;
                continue;
            }

            if quote_count == 1 {
                let payload_start = position.offset + 1;
                if let Some(relative_close) = self.source[payload_start..end].find('"') {
                    let close = payload_start + relative_close;
                    let after_close = close + 1;
                    if close == payload_start {
                        self.diagnostics.push(Diagnostic::error(
                            "syntax.invalid-inline-dispatch",
                            "empty inline verbatim must use a bracket envelope",
                            introducer..after_close,
                        ));
                        position.offset = after_close;
                        frames.last_mut().unwrap().text_start = after_close;
                        continue;
                    }
                    let (attrs, after_attrs) =
                        if after_close < end && self.source.as_bytes()[after_close] == b'{' {
                            self.parse_inline_postfix(segments, &mut position, after_close, end)
                        } else {
                            (Attributes::default(), after_close)
                        };
                    frames.last_mut().unwrap().items.push(Inline::Verbatim {
                        range: introducer..after_attrs,
                        kind,
                        kind_range: kind_start..kind_end,
                        text: self.source[payload_start..close].to_string(),
                        text_range: payload_start..close,
                        quote_count: 1,
                        bracketed: false,
                        attrs,
                    });
                    position.offset = after_attrs;
                    frames.last_mut().unwrap().text_start = after_attrs;
                    continue;
                }
                self.diagnostics.push(Diagnostic::error(
                    "syntax.unclosed-verbatim",
                    "compact inline verbatim must close on the same physical line",
                    introducer..end,
                ));
                position.offset = end;
                frames.last_mut().unwrap().text_start = end;
                continue;
            }

            // §8: the inline kind must be nonempty — there is no anonymous
            // inline element, so an empty kind before '[' falls through to
            // the incomplete-dispatch diagnostic.
            if position.offset < end
                && self.source.as_bytes()[position.offset] == b'['
                && kind_end > kind_start
            {
                position.offset += 1;
                frames.push(InlineFrame {
                    start: position.offset,
                    text_start: position.offset,
                    items: Vec::new(),
                    opening: Some(InlineOpening {
                        introducer,
                        kind: self.source[kind_start..kind_end].to_string(),
                        kind_range: kind_start..kind_end,
                    }),
                });
                continue;
            }

            let diagnostic_end = if position.offset < end {
                next_char_end(self.source, position.offset)
            } else {
                end
            };
            self.diagnostics.push(Diagnostic::error(
                "syntax.invalid-inline-dispatch",
                "inline introducer requires an inline kind followed by '['",
                introducer..diagnostic_end,
            ));
            frames.last_mut().unwrap().text_start = position.offset;
        }

        if frames.len() > 1 {
            for frame in frames.iter().skip(1).rev() {
                let opening = frame.opening.as_ref().unwrap();
                self.diagnostics.push(Diagnostic::error(
                    "syntax.unclosed-inline",
                    "parsed inline element is not closed before the enclosing inline boundary",
                    opening.introducer..position.offset,
                ));
            }
            frames.truncate(1);
            frames[0].text_start = position.offset;
        }
        let mut root = frames.pop().unwrap();
        flush_inline_text(self.source, &mut root, position.offset);
        InlineContent {
            range: root.start..position.offset,
            items: root.items,
        }
    }

    fn attribute_extent(
        &self,
        start: usize,
        line_index: usize,
        owner_indent: usize,
    ) -> (usize, usize) {
        let first = &self.lines.0[line_index];
        if has_unquoted_closing_brace(self.source, start + 1, first.content_end) {
            return (first.content_end, line_index);
        }

        let mut index = line_index + 1;
        let mut last = line_index;
        while let Some(line) = self.lines.0.get(index) {
            if line.blank || line.indent <= owner_indent || line.has_tab_indent {
                break;
            }
            if self.block_dispatch_readonly(index, line.indent).is_some() {
                break;
            }
            last = index;
            if has_unquoted_closing_brace(self.source, line.start + line.indent, line.content_end) {
                return (line.content_end, line_index.max(index));
            }
            index += 1;
        }
        (self.lines.0[last].content_end, last)
    }

    fn parse_inline_postfix(
        &mut self,
        segments: &mut Vec<InlineSegment>,
        position: &mut InlinePosition,
        start: usize,
        limit: usize,
    ) -> (Attributes, usize) {
        let mut content_start = start + 1;
        while content_start < limit && self.source.as_bytes()[content_start].is_ascii_whitespace() {
            content_start += 1;
        }
        let _ = (segments, content_start);
        self.parse_inline_attached(position, start, limit)
    }

    fn parse_inline_attached(
        &mut self,
        position: &mut InlinePosition,
        start: usize,
        limit: usize,
    ) -> (Attributes, usize) {
        let Some(close) = find_inline_group_close(self.source, start + 1, limit) else {
            self.diagnostics.push(Diagnostic::error(
                "syntax.unclosed-attached-group",
                "inline attached group must close before the inline boundary",
                start..limit,
            ));
            position.offset = limit;
            return (
                Attributes {
                    range: Some(start..limit),
                    items: Vec::new(),
                    attached: Some(Box::new(AttachedGroup {
                        range: start..limit,
                        open_range: start..start + 1,
                        close_range: limit..limit,
                        opener_on_own_line: false,
                        content: AttachedContent::Inlines(InlineContent {
                            range: start + 1..limit,
                            items: Vec::new(),
                        }),
                    })),
                },
                limit,
            );
        };
        let mut inner = vec![InlineSegment {
            start: start + 1,
            end: close,
        }];
        let content = self.parse_inline_segments(&mut inner, true);
        let items = project_inline_attributes(self.source, &content);
        let end = close + 1;
        position.offset = end;
        (
            Attributes {
                range: Some(start..end),
                items,
                attached: Some(Box::new(AttachedGroup {
                    range: start..end,
                    open_range: start..start + 1,
                    close_range: close..end,
                    opener_on_own_line: false,
                    content: AttachedContent::Inlines(content),
                })),
            },
            end,
        )
    }

    fn line_index_at_offset(&self, offset: usize) -> Option<usize> {
        let index = self
            .lines
            .0
            .partition_point(|line| line.start <= offset)
            .checked_sub(1)?;
        (offset <= self.lines.0[index].content_end).then_some(index)
    }

    fn block_dispatch_readonly(&self, index: usize, indent: usize) -> Option<BlockDispatch> {
        let line = &self.lines.0[index];
        let start = line.start + indent;
        let text = &self.source[start..line.content_end];
        let ticks = text.bytes().take_while(|byte| *byte == b'`').count();
        if ticks != 1 {
            return None;
        }
        let after = start + 1;
        if after >= line.content_end || self.source.as_bytes()[after] == b'[' {
            return None;
        }
        let marker_end = take_name_like(self.source, after, line.content_end, marker_char);
        (marker_end == line.content_end || self.source.as_bytes().get(marker_end) != Some(&b'['))
            .then_some(BlockDispatch::Marked)
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
                        attached: None,
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
                        let mut reported_unclosed = false;
                        while cursor < limit {
                            let byte = self.source.as_bytes()[cursor];
                            if matches!(byte, b'\n' | b'\r') {
                                self.diagnostics.push(Diagnostic::error(
                                    "syntax.unclosed-quoted-value",
                                    "quoted attribute values must close on the same physical line",
                                    value_start..cursor,
                                ));
                                reported_unclosed = true;
                                break;
                            }
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
                                    cursor..if cursor + 1 < limit {
                                        next_char_end(self.source, cursor + 1)
                                    } else {
                                        limit
                                    },
                                ));
                            }
                            let next = next_char_end(self.source, cursor);
                            decoded.push_str(&self.source[cursor..next]);
                            cursor = next;
                        }
                        if !closed && !reported_unclosed {
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
                attached: None,
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

fn has_space_prefix(source: &str, start: usize, width: usize) -> bool {
    source
        .as_bytes()
        .get(start..start.saturating_add(width))
        .is_some_and(|prefix| prefix.iter().all(|byte| *byte == b' '))
}

/// Finds the trailing attachment opener on one head line: the first
/// unescaped opening brace with a preceding separator. The scan is local —
/// it skips escapes and verbatim envelopes as single tokens and tracks no
/// nesting depth, which the unified escape rules make sound: in valid
/// documents no bare brace can hide inside inline element content.
fn block_attached_start(source: &str, start: usize, end: usize) -> Option<(usize, usize)> {
    let mut cursor = start;
    while cursor < end {
        if source.as_bytes()[cursor] == b'`' {
            let ticks = source[cursor..end]
                .bytes()
                .take_while(|byte| *byte == b'`')
                .count();
            cursor += ticks;
            if ticks % 2 == 0 || cursor >= end {
                continue;
            }
            if matches!(
                source.as_bytes()[cursor],
                b'{' | b'}' | b'[' | b']'
            ) {
                cursor += 1;
                continue;
            }
            let kind_end = take_name_like(source, cursor, end, marker_char);
            let quote_count = source[kind_end..end]
                .bytes()
                .take_while(|byte| *byte == b'"')
                .count();
            let delimiter = kind_end + quote_count;
            if quote_count > 0 && delimiter < end && source.as_bytes()[delimiter] == b'[' {
                if let Some((_, after)) =
                    find_verbatim_close(source, delimiter + 1, end, quote_count)
                {
                    cursor = after;
                    continue;
                }
            } else if quote_count == 1 {
                if let Some(close) = source[delimiter..end].find('"') {
                    cursor = delimiter + close + 1;
                    continue;
                }
            }
            cursor = delimiter.max(cursor);
            continue;
        }
        if source.as_bytes()[cursor] == b'{' {
            if cursor == start {
                return Some((start, cursor));
            }
            let mut separator = cursor;
            while separator > start && matches!(source.as_bytes()[separator - 1], b' ' | b'\t') {
                separator -= 1;
            }
            if separator < cursor {
                return Some((separator, cursor));
            }
        }
        cursor = next_char_end(source, cursor);
    }
    None
}

fn flush_inline_text(source: &str, frame: &mut InlineFrame, end: usize) {
    let mut cursor = frame.text_start;
    while cursor < end {
        let is_space = matches!(source.as_bytes()[cursor], b' ' | b'\t');
        let start = cursor;
        while cursor < end && matches!(source.as_bytes()[cursor], b' ' | b'\t') == is_space {
            cursor = next_char_end(source, cursor);
        }
        let text = source[start..cursor].to_string();
        if is_space {
            frame.items.push(Inline::Space {
                text,
                range: start..cursor,
            });
        } else {
            frame.items.push(Inline::Text {
                text,
                range: start..cursor,
            });
        }
    }
    frame.text_start = end;
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

fn find_inline_group_close(source: &str, mut cursor: usize, limit: usize) -> Option<usize> {
    let mut quoted = false;
    while cursor < limit {
        match source.as_bytes()[cursor] {
            b'\\' if quoted && cursor + 1 < limit => {
                cursor = next_char_end(source, cursor + 1);
            }
            b'"' => {
                quoted = !quoted;
                cursor += 1;
            }
            b'`' if !quoted => {
                let after_tick = cursor + 1;
                if after_tick < limit && matches!(source.as_bytes()[after_tick], b'{' | b'}') {
                    cursor = after_tick + 1;
                    continue;
                }
                let quotes = source[after_tick..limit]
                    .bytes()
                    .take_while(|candidate| *candidate == b'"')
                    .count();
                let open = after_tick + quotes;
                if open < limit && source.as_bytes()[open] == b'[' {
                    if let Some((_, after_close)) =
                        find_verbatim_close(source, open + 1, limit, quotes)
                    {
                        cursor = after_close;
                    } else {
                        return None;
                    }
                } else {
                    cursor += 1;
                }
            }
            b'}' if !quoted => return Some(cursor),
            _ => cursor = next_char_end(source, cursor),
        }
    }
    None
}

fn has_unquoted_closing_brace(source: &str, mut cursor: usize, limit: usize) -> bool {
    let mut quoted = false;
    while cursor < limit {
        match source.as_bytes()[cursor] {
            b'\\' if quoted => {
                cursor += 1;
                if cursor < limit {
                    cursor = next_char_end(source, cursor);
                }
            }
            b'"' => {
                quoted = !quoted;
                cursor += 1;
            }
            b'}' if !quoted => return true,
            _ => cursor = next_char_end(source, cursor),
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_root_and_marked_block_attached_groups_with_ordinary_blocks() {
        let source = "{\n  `: title Document title\n  `: tags plumb\n}\n\n`- Buy milk {\n  `- task\n  `@ shopping\n  `: due 2026-08-07\n}\n\n  Details.\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        assert_eq!(parsed.syntax.attrs.value("title"), Some("Document title"));
        let root = parsed.syntax.attrs.attached.as_deref().unwrap();
        let AttachedContent::Blocks(root_blocks) = &root.content else {
            panic!("expected block attached content");
        };
        assert_eq!(root_blocks.len(), 2);

        let Block::Parsed(item) = &parsed.syntax.blocks[0] else {
            panic!("expected list item");
        };
        let attrs = &item.mark.as_ref().unwrap().attrs;
        assert!(attrs.has_class("task"));
        assert_eq!(attrs.id(), Some("shopping"));
        assert_eq!(attrs.value("due"), Some("2026-08-07"));
        assert_eq!(item.children.len(), 1);
        assert_eq!(item.children[0].children().len(), 0);
    }

    #[test]
    fn parses_inline_attached_groups_with_ordinary_inline_elements() {
        let source = "See `->[guide]{`@[main] `-[external] `:[to guide.plumb#intro]}.\nRaw `->\"target.plumb\"\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Parsed(paragraph) = &parsed.syntax.blocks[0] else {
            panic!("expected paragraph");
        };
        let Inline::Element { attrs, .. } = paragraph
            .head
            .items
            .iter()
            .find(|inline| matches!(inline, Inline::Element { kind, .. } if kind == "->"))
            .unwrap()
        else {
            panic!("expected link");
        };
        assert_eq!(attrs.id(), Some("main"));
        assert!(attrs.has_class("external"));
        assert_eq!(attrs.value("to"), Some("guide.plumb#intro"));
        assert!(matches!(
            attrs.attached.as_deref().map(|group| &group.content),
            Some(AttachedContent::Inlines(_))
        ));
    }

    #[test]
    fn parses_verbatim_block_with_structural_attached_opener() {
        let source = "`rust\" {`@[example]}\n fn main() {\n     println!(\"hi\");\n }\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Verbatim(block) = &parsed.syntax.blocks[0] else {
            panic!("expected verbatim block");
        };
        assert_eq!(block.attrs.id(), Some("example"));
        assert_eq!(block.kind, "rust");
        assert_eq!(block.text, "fn main() {\n    println!(\"hi\");\n}\n");
    }

    #[test]
    fn attached_group_must_be_the_header_trailer() {
        let parsed = parse("`- Item\n\n  Details.\n  {\n    `task\n  }\n");
        assert!(!parsed.is_valid());
        assert!(parsed.syntax.blocks[0]
            .children()
            .iter()
            .all(|block| block.range().start != 20));
    }

    #[test]
    fn parses_heading_and_nested_blocks() {
        let parsed =
            parse("`heading Intro {`@[intro] `:[level 1]}\n  child text\n\n  `task Work\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Parsed(heading) = &parsed.syntax.blocks[0] else {
            panic!("expected heading");
        };
        assert_eq!(heading.head.plain_text(), "Intro");
        assert_eq!(heading.children.len(), 2);
    }

    #[test]
    fn parses_attached_groups_on_marked_and_verbatim_blocks() {
        let source = "`- Work {\n  `- task\n  `@ write\n  `: created 2026-07-20T09:00:00+08:00\n}\n\n`tex\" {`-[$] `@[equation]}\n E = mc^2\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let Block::Parsed(task) = &parsed.syntax.blocks[0] else {
            panic!("expected task block");
        };
        let attrs = &task.mark.as_ref().unwrap().attrs;
        assert!(attrs.has_class("task"));
        assert_eq!(attrs.id(), Some("write"));
        assert_eq!(task.head.plain_text(), "Work");

        let Block::Verbatim(math) = &parsed.syntax.blocks[1] else {
            panic!("expected verbatim block");
        };
        assert_eq!(math.kind, "tex");
        assert!(math.attrs.has_class("$"));
        assert_eq!(math.attrs.value("language"), None);
        assert_eq!(math.text, "E = mc^2\n");
    }

    #[test]
    fn block_attached_groups_allow_compact_and_brace_aligned_closing_delimiters() {
        let source = "`- Work {\n  `- task\n  `@ write\n}\n\n`text\"\n payload\n`x {\n  `- class\n}\n\n  `child Nested\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let Block::Parsed(task) = &parsed.syntax.blocks[0] else {
            panic!("expected marked block");
        };
        assert!(task.mark.as_ref().unwrap().attrs.has_class("task"));
        assert_eq!(task.mark.as_ref().unwrap().attrs.id(), Some("write"));
        assert_eq!(task.head.plain_text(), "Work");

        let Block::Verbatim(verbatim) = &parsed.syntax.blocks[1] else {
            panic!("expected verbatim block");
        };
        assert_eq!(verbatim.kind, "text");
        assert_eq!(verbatim.attrs.value("language"), None);
        assert_eq!(verbatim.text, "payload\n");

        let Block::Parsed(container) = &parsed.syntax.blocks[2] else {
            panic!("expected empty-head container");
        };
        assert!(container.mark.as_ref().unwrap().attrs.has_class("class"));
        assert_eq!(container.children.len(), 1);

        let next_line = parse("`- Work\n  {\n    `- task\n  }\n");
        assert!(next_line.is_valid(), "{:?}", next_line.diagnostics);
        let Block::Parsed(task) = &next_line.syntax.blocks[0] else {
            panic!("expected next-line attached group");
        };
        assert!(
            task.mark
                .as_ref()
                .unwrap()
                .attrs
                .attached
                .as_ref()
                .unwrap()
                .opener_on_own_line
        );

        let compact = parse("`- Something {`-[task] `@[id]}\n");
        assert!(compact.is_valid(), "{:?}", compact.diagnostics);

        let crlf = parse("`- Work {\r\n  `- task\r\n  `@ crlf\r\n  `: key value\r\n}\r\n");
        assert!(crlf.is_valid(), "{:?}", crlf.diagnostics);
        let Block::Parsed(task) = &crlf.syntax.blocks[0] else {
            panic!("expected CRLF task block");
        };
        assert_eq!(task.mark.as_ref().unwrap().attrs.id(), Some("crlf"));
        assert_eq!(
            task.mark.as_ref().unwrap().attrs.value("key"),
            Some("value")
        );
        assert_eq!(task.head.plain_text(), "Work");
    }

    #[test]
    fn parses_inline_elements_and_verbatim() {
        let parsed = parse("Text `em[inside] and `\"raw\".\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Parsed(block) = &parsed.syntax.blocks[0] else {
            panic!("expected paragraph");
        };
        assert_eq!(block.head.plain_text(), "Text inside and raw.");
    }

    #[test]
    fn exposes_horizontal_whitespace_as_typed_inline_space() {
        let parsed = parse("`node one   two\tthree\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Parsed(block) = &parsed.syntax.blocks[0] else {
            panic!("expected marked block");
        };
        assert!(matches!(
            block.head.items.as_slice(),
            [
                Inline::Text { text: one, .. },
                Inline::Space { text: spaces, .. },
                Inline::Text { text: two, .. },
                Inline::Space { text: tab, .. },
                Inline::Text { text: three, .. },
            ] if one == "one" && spaces == "   " && two == "two" && tab == "\t" && three == "three"
        ));
        assert_eq!(block.head.plain_text(), "one   two\tthree");
    }

    #[test]
    fn parses_multiline_elements_in_paragraphs_and_marked_heads() {
        let source = "Before `span[first\nsecond `em[嵌套]\nthird] after\n`note Head `span[one\n  two] tail\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let Block::Parsed(paragraph) = &parsed.syntax.blocks[0] else {
            panic!("expected paragraph");
        };
        assert_eq!(
            paragraph.head.plain_text(),
            "Before first second 嵌套 third after"
        );
        let Some(Inline::Element { content, .. }) = paragraph
            .head
            .items
            .iter()
            .find(|inline| matches!(inline, Inline::Element { .. }))
        else {
            panic!("expected multiline span");
        };
        assert_eq!(
            content
                .items
                .iter()
                .filter(|inline| matches!(inline, Inline::SoftBreak { .. }))
                .count(),
            2
        );

        let Block::Parsed(note) = &parsed.syntax.blocks[1] else {
            panic!("expected marked block");
        };
        assert_eq!(note.head.plain_text(), "Head one two tail");
    }

    #[test]
    fn multiline_element_recovers_before_hard_boundaries() {
        let blank = parse("`span[open\ncontinued\n\nNext paragraph.\n");
        assert!(!blank.is_valid());
        assert_eq!(blank.syntax.blocks.len(), 2);
        assert!(blank
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "syntax.unclosed-inline"));

        let block = parse("`parent `span[open\n  `child Boundary\n");
        assert!(!block.is_valid());
        let Block::Parsed(parent) = &block.syntax.blocks[0] else {
            panic!("expected parent");
        };
        assert_eq!(parent.children.len(), 1);
    }

    #[test]
    fn inline_element_nesting_uses_an_explicit_stack() {
        let depth = 4096;
        let mut source = String::new();
        for _ in 0..depth {
            source.push_str("`x[");
        }
        source.push_str("value");
        for _ in 0..depth {
            source.push(']');
        }
        source.push('\n');

        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
    }

    #[test]
    fn quote_count_strengthens_inline_verbatim_delimiters() {
        let parsed = parse("`\"plain\" `\"[contains ] safely]\" `\"\"[contains ]\" safely]\"\"\n");
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
                ("plain", 1),
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
    fn quote_count_declares_the_verbatim_block_margin() {
        let parsed = parse("`rust\"\"\n  fn main() {}\n    indented\nnext\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Verbatim(block) = &parsed.syntax.blocks[0] else {
            panic!("expected verbatim block");
        };
        assert_eq!(block.kind, "rust");
        assert_eq!(block.quote_count, 2);
        assert_eq!(block.text, "fn main() {}\n  indented\n");
        assert!(matches!(parsed.syntax.blocks[1], Block::Parsed(_)));
    }

    #[test]
    fn parses_same_line_first_child_like_an_indented_first_child() {
        let compact = parse("`- `- a\n   `- b\n   `- c\n");
        let expanded = parse("`-\n   `- a\n   `- b\n   `- c\n");
        assert!(compact.is_valid(), "{:?}", compact.diagnostics);
        assert!(expanded.is_valid(), "{:?}", expanded.diagnostics);

        let Block::Parsed(compact_outer) = &compact.syntax.blocks[0] else {
            panic!("expected compact outer item");
        };
        let Block::Parsed(expanded_outer) = &expanded.syntax.blocks[0] else {
            panic!("expected expanded outer item");
        };
        assert!(compact_outer.head.items.is_empty());
        assert_eq!(compact_outer.children.len(), 3);
        assert_eq!(
            compact_outer
                .children
                .iter()
                .map(|child| match child {
                    Block::Parsed(child) => child.head.plain_text(),
                    Block::Verbatim(_) => panic!("expected parsed child"),
                })
                .collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        assert_eq!(compact_outer.children.len(), expanded_outer.children.len());
    }

    #[test]
    fn same_line_first_child_requires_an_empty_head_and_supports_recursion() {
        let nested = parse("`- `- `- deep\n");
        assert!(nested.is_valid(), "{:?}", nested.diagnostics);
        let Block::Parsed(outer) = &nested.syntax.blocks[0] else {
            panic!("expected outer item");
        };
        let Block::Parsed(middle) = &outer.children[0] else {
            panic!("expected middle item");
        };
        assert_eq!(middle.children.len(), 1);

        let invalid = parse("`- text `- child\n");
        assert!(!invalid.is_valid());
        assert!(invalid
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "syntax.invalid-inline-dispatch"));
    }

    #[test]
    fn block_nesting_uses_an_explicit_stack() {
        const DEPTH: usize = 20_000;
        let mut source = "`x ".repeat(DEPTH);
        source.push_str("leaf\n");
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let mut blocks = parsed.syntax.blocks.as_slice();
        let mut depth = 0;
        while let [Block::Parsed(block)] = blocks {
            depth += 1;
            blocks = &block.children;
        }
        assert_eq!(depth, DEPTH);
    }

    #[test]
    fn deeply_nested_malformed_blocks_recover_without_call_stack_recursion() {
        const DEPTH: usize = 20_000;
        let mut source = "`x ".repeat(DEPTH);
        source.push_str("`\n");
        let parsed = parse(source);
        assert!(!parsed.is_valid());
        assert!(parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "syntax.incomplete-introducer"));
    }

    #[test]
    fn parsed_inline_and_marked_block_require_names() {
        let inline = parse("`\"[not parsed]\"\n");
        let Block::Parsed(paragraph) = &inline.syntax.blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(matches!(paragraph.head.items[0], Inline::Verbatim { .. }));

        let block = parse("`\" {`-[note]}\n  raw `em[not parsed]\n");
        assert!(matches!(block.syntax.blocks[0], Block::Verbatim(_)));

        let quote_block = parse("`\"\n  code block\n");
        assert!(quote_block.is_valid(), "{:?}", quote_block.diagnostics);

        let verbatim_head = parse("`{} head is forbidden\n");
        assert!(!verbatim_head.is_valid());
    }

    #[test]
    fn malformed_quoted_attached_group_keeps_utf8_cursors_on_boundaries() {
        let source = " `!\"{\"\\¡";
        let parsed = parse(source);
        assert_eq!(parsed.lossless.reconstruct(&parsed.source), source);
        assert!(parsed.diagnostics.iter().all(|diagnostic| {
            source.is_char_boundary(diagnostic.range.start)
                && source.is_char_boundary(diagnostic.range.end)
        }));
    }

    #[test]
    fn enforces_block_group_ownership_and_trailers() {
        let duplicate = parse("`x Head {} {}\n");
        assert_eq!(
            duplicate.diagnostics[0].code,
            "syntax.duplicate-attached-group"
        );
        assert_eq!(duplicate.diagnostics[0].related, vec![8..9]);

        let trailing = parse("`x Head {} tail\n");
        assert_eq!(
            trailing.diagnostics[0].code,
            "syntax.trailing-after-attached-group"
        );

        let document = parse("{}\n");
        assert!(document
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "syntax.invalid-document-group"));

        let tight_verbatim = parse("`rust\"{`@[id]}\n  payload\n");
        assert!(!tight_verbatim.is_valid());
    }

    #[test]
    fn parses_empty_and_verbatim_expanded_groups_and_brace_escapes() {
        let parsed = parse(
            "`x {}\n`x Head {\n}\n`rust\" {\n  `@ code\n}\n\n payload\nText `span[x]{literal `{ and `} braces}\n",
        );
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Verbatim(verbatim) = &parsed.syntax.blocks[2] else {
            panic!("expected verbatim block");
        };
        assert_eq!(verbatim.attrs.id(), Some("code"));
        assert_eq!(verbatim.text, "\npayload\n");
    }

    #[test]
    fn parses_own_line_opener_groups_before_children() {
        let parsed = parse(
            "`task Work\n      {\n        `: created now\n      }\n\n      Details\n\n`note first\n      second\n      {\n        `- cited\n      }\n",
        );
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let Block::Parsed(task) = &parsed.syntax.blocks[0] else {
            panic!("expected task block");
        };
        let attrs = &task.mark.as_ref().unwrap().attrs;
        assert_eq!(attrs.value("created"), Some("now"));
        assert!(attrs.attached.as_ref().unwrap().opener_on_own_line);
        assert_eq!(task.children.len(), 1);
        let Block::Parsed(details) = &task.children[0] else {
            panic!("expected child paragraph");
        };
        assert_eq!(details.head.plain_text(), "Details");

        let Block::Parsed(note) = &parsed.syntax.blocks[1] else {
            panic!("expected note block");
        };
        assert_eq!(note.head.plain_text(), "first second");
        assert_eq!(note.mark.as_ref().unwrap().attrs.items.len(), 1);
    }

    #[test]
    fn own_line_opener_requires_adjacency_and_the_continuation_column() {
        for source in [
            "`task Work\n\n      {\n        `: created now\n      }\n",
            "`note first\n      second\n    {\n      `- cited\n    }\n",
        ] {
            let parsed = parse(source);
            assert!(!parsed.is_valid(), "{source}");
            assert!(parsed.diagnostics.iter().any(|diagnostic| {
                matches!(
                    diagnostic.code,
                    "syntax.unattached-group" | "syntax.partial-indent"
                )
            }));
        }

        let verbatim = parse("`rust\"\n {\n raw\n");
        assert!(verbatim.is_valid(), "{:?}", verbatim.diagnostics);
        let Block::Verbatim(verbatim) = &verbatim.syntax.blocks[0] else {
            panic!("expected verbatim block");
        };
        assert_eq!(verbatim.text, "{\nraw\n");
        assert!(verbatim.attrs.attached.is_none());
    }

    #[test]
    fn own_line_opener_delimiters_allow_trailing_horizontal_whitespace() {
        let parsed = parse("`task Work\n      { \t\n        `: created now\n      }  \t\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let Block::Parsed(task) = &parsed.syntax.blocks[0] else {
            panic!("expected task block");
        };
        assert_eq!(
            task.mark.as_ref().unwrap().attrs.value("created"),
            Some("now")
        );
    }

    #[test]
    fn trailing_verbatim_blank_lines_require_the_declared_margin() {
        let parsed = parse("`text\"\"\n  code\n  \n\n`note Next\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Verbatim(verbatim) = &parsed.syntax.blocks[0] else {
            panic!("expected verbatim block");
        };
        assert_eq!(verbatim.text, "code\n\n");
        assert_eq!(parsed.syntax.blocks.len(), 2);

        let internal = parse("`text\"\"\n  first\n\n  second\n");
        let Block::Verbatim(verbatim) = &internal.syntax.blocks[0] else {
            panic!("expected verbatim block");
        };
        assert_eq!(verbatim.text, "first\n\nsecond\n");
    }

    #[test]
    fn own_line_opener_column_is_the_following_child_column() {
        let valid = parse("`task Work\n {\n  `@ work\n }\n `note Child\n");
        assert!(valid.is_valid(), "{:?}", valid.diagnostics);

        let invalid = parse("`task Work\n  {\n   `@ work\n  }\n `note Child\n");
        assert!(!invalid.is_valid());
        assert!(invalid
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "syntax.partial-indent"));
    }

    #[test]
    fn own_line_opener_rejects_trailing_source() {
        for source in ["`task Work\n      { extra\n", "`task Work\n      {}\n"] {
            let parsed = parse(source);
            assert!(!parsed.is_valid(), "{source}");
            assert!(parsed
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.code == "syntax.trailing-after-attached-group" }));
        }
    }

    #[test]
    fn closing_brace_returns_to_the_opener_line_column() {
        // Trailing opener: the close returns to the header line's column.
        let trailing = parse("`- Work {\n  `- task\n}\n");
        assert!(trailing.is_valid(), "{:?}", trailing.diagnostics);
        // Own-line opener: the close returns to the head continuation column.
        let own_line = parse("`- Work\n  {\n   `- task\n  }\n");
        assert!(own_line.is_valid(), "{:?}", own_line.diagnostics);
        // Document opener: the close returns to the file column.
        let document = parse("{\n `: title T\n}\n");
        assert!(document.is_valid(), "{:?}", document.diagnostics);

        for source in [
            "`- Work {\n  `- task\n  }\n",
            "`- Work\n  {\n   `- task\n   }\n",
            "{\n `: title T\n }\n",
        ] {
            let parsed = parse(source);
            assert!(!parsed.is_valid(), "{source}");
            assert!(parsed.diagnostics.iter().any(|diagnostic| {
                matches!(
                    diagnostic.code,
                    "syntax.unclosed-attached-group" | "syntax.unexpected-group-close"
                )
            }));
        }
    }

    #[test]
    fn attachment_scan_skips_escapes_and_envelopes_locally() {
        // The opener scan treats escapes and verbatim envelopes as single
        // tokens and tracks no element nesting; neither hides a later
        // trailing opener on the head line.
        let escaped = parse("`note `x[a `{b`} c] tail {\n `- done\n}\n");
        assert!(escaped.is_valid(), "{:?}", escaped.diagnostics);
        let Block::Parsed(note) = &escaped.syntax.blocks[0] else {
            panic!("expected marked block");
        };
        assert_eq!(note.head.plain_text(), "a {b} c tail");
        assert_eq!(note.mark.as_ref().unwrap().attrs.items.len(), 1);

        let envelope = parse("`note `\"[p {q]\" tail {\n `- done\n}\n");
        assert!(envelope.is_valid(), "{:?}", envelope.diagnostics);
        let Block::Parsed(note) = &envelope.syntax.blocks[0] else {
            panic!("expected marked block");
        };
        assert_eq!(note.head.plain_text(), "p {q tail");
    }

    #[test]
    fn anonymous_inline_elements_are_rejected() {
        // §2/§8: the introducer-plus-bracket spelling is a literal escape,
        // so an empty-kind element cannot even be written; the leftovers
        // surface as bare-delimiter errors.
        let empty = parse("`[] content\n");
        assert!(!empty.is_valid());
        assert!(empty
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "syntax.unexpected-element-close"));
        let raw = parse("`[raw] tail\n");
        assert!(!raw.is_valid());
        assert!(raw
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "syntax.unexpected-element-close"));

        // Empty content with a nonempty kind stays valid.
        let empty = parse("`k[] tail\n");
        assert!(empty.is_valid(), "{:?}", empty.diagnostics);
    }

    #[test]
    fn bare_delimiters_never_fall_back_to_text() {
        // Every bare delimiter in content is structural or an error.
        for (source, code) in [
            ("Text { brace\n", "syntax.unattached-group"),
            ("Text } brace\n", "syntax.unexpected-group-close"),
            ("Text [ bracket\n", "syntax.unattached-bracket"),
            ("Text ] bracket\n", "syntax.unexpected-element-close"),
        ] {
            let parsed = parse(source);
            assert!(!parsed.is_valid(), "{source}");
            assert!(
                parsed.diagnostics.iter().any(|d| d.code == code),
                "{source}: {:?}",
                parsed.diagnostics
            );
        }

        // Escapes are unconditional literals in every position, including
        // inside element content and at block start.
        let escaped = parse("`k[a `[b`] c `{ `} x]\n");
        assert!(escaped.is_valid(), "{:?}", escaped.diagnostics);
        let block_start = parse("`{ starts a paragraph and `[ x\n");
        assert!(block_start.is_valid(), "{:?}", block_start.diagnostics);
    }

    #[test]
    fn trailing_opener_ends_any_head_line() {
        // Expanded trailing opener on a continuation line: close and later
        // children sit at the opener line's column.
        let expanded = parse("`note first\n  second {\n   `- cited\n  }\n  Details\n");
        assert!(expanded.is_valid(), "{:?}", expanded.diagnostics);
        let Block::Parsed(note) = &expanded.syntax.blocks[0] else {
            panic!("expected marked block");
        };
        assert_eq!(note.head.plain_text(), "first second");
        let attrs = &note.mark.as_ref().unwrap().attrs;
        assert_eq!(attrs.items.len(), 1);
        assert!(!attrs.attached.as_ref().unwrap().opener_on_own_line);
        assert_eq!(note.children.len(), 1);
        let Block::Parsed(details) = &note.children[0] else {
            panic!("expected child paragraph");
        };
        assert_eq!(details.head.plain_text(), "Details");

        // Compact trailing opener on a continuation line.
        let compact = parse("`note first\n  second {`@[x]}\n");
        assert!(compact.is_valid(), "{:?}", compact.diagnostics);
        let Block::Parsed(note) = &compact.syntax.blocks[0] else {
            panic!("expected marked block");
        };
        assert_eq!(note.mark.as_ref().unwrap().attrs.id(), Some("x"));

        // The close must return to the opener line's column.
        let misaligned = parse("`note first\n  second {\n   `- cited\n   }\n");
        assert!(!misaligned.is_valid());
        assert!(misaligned
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "syntax.unclosed-attached-group"));

        // Braces on a continuation line join the attachment triage exactly
        // as on the header line; a literal brace needs the introducer.
        let escaped = parse("`note first\n  second `{ third\n");
        assert!(escaped.is_valid(), "{:?}", escaped.diagnostics);
        let Block::Parsed(note) = &escaped.syntax.blocks[0] else {
            panic!("expected marked block");
        };
        assert!(note.head.plain_text().contains('{'));
        assert!(note.mark.as_ref().unwrap().attrs.attached.is_none());

        let junk = parse("`note first\n  second { third\n");
        assert!(!junk.is_valid());
        assert!(junk
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "syntax.unclosed-attached-group"));
    }

    #[test]
    fn delimiter_escapes_apply_only_at_active_structural_sites() {
        let escaped = parse(
            "`{ starts a paragraph\n\n`span[text]`{ stays text and `} also stays text\n\n`span[x]{literal `{ and `} braces}\n",
        );
        assert!(escaped.is_valid(), "{:?}", escaped.diagnostics);
        assert_eq!(escaped.syntax.blocks.len(), 3);
        let Block::Parsed(block_start) = &escaped.syntax.blocks[0] else {
            panic!("expected paragraph from escaped block-start brace");
        };
        assert_eq!(block_start.head.plain_text(), "{ starts a paragraph");
        let Block::Parsed(attachment_site) = &escaped.syntax.blocks[1] else {
            panic!("expected paragraph with escaped attachment-site braces");
        };
        assert_eq!(
            attachment_site.head.plain_text(),
            "text{ stays text and } also stays text"
        );

        let incomplete_dispatch = parse("Text `q ordinary dispatch remains strict\n");
        assert!(incomplete_dispatch
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "syntax.invalid-inline-dispatch"));
    }

    #[test]
    fn malformed_old_attribute_spelling_keeps_ranges_in_bounds() {
        let source = "`node{.one`text\"`node{key=\"bad\\q\"}`node\n      {\n        `: key quote\" slash\\\n      }\n\n";
        let parsed = parse(source);
        assert_eq!(parsed.lossless.reconstruct(&parsed.source), source);
        assert!(parsed
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.range.end <= source.len()));
    }

    #[test]
    fn line_index_lookup_preserves_content_boundaries() {
        let source = "alpha\r\nbeta\n\nγ";
        let parser = Parser {
            source,
            lines: Lines::new(source),
            diagnostics: Vec::new(),
        };
        for (offset, expected) in [
            (0, Some(0)),
            (5, Some(0)),
            (6, None),
            (7, Some(1)),
            (11, Some(1)),
            (12, Some(2)),
            (13, Some(3)),
            (14, Some(3)),
            (15, Some(3)),
            (16, None),
        ] {
            assert_eq!(parser.line_index_at_offset(offset), expected, "{offset}");
        }
    }
}
