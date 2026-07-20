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
                introducer..(cursor + 1).min(line.content_end),
            ));
        }
        let marker = self.source[marker_start..cursor].to_string();
        let marker_range = marker_start..cursor;

        let attrs = if cursor < line.content_end && self.source.as_bytes()[cursor] == b'{' {
            let (limit, last_line) = self.attribute_extent(cursor, index, indent);
            let (attrs, next) = self.parse_attributes(cursor, limit);
            cursor = next;
            header_index = last_line;
            line = self.lines.0[last_line].clone();
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

        if head_start < line.content_end {
            let child_indent = head_start - line.start;
            if let Some(dispatch) = self.block_dispatch(header_index, child_indent) {
                let (first_child, mut next) = match dispatch {
                    BlockDispatch::Marked => {
                        let (child, next) = self.parse_marked(header_index, child_indent);
                        (Block::Parsed(child), next)
                    }
                    BlockDispatch::Verbatim => {
                        let (child, next) = self.parse_verbatim(header_index, child_indent);
                        (Block::Verbatim(child), next)
                    }
                };
                let mut children = vec![first_child];
                let (siblings, after_siblings) = self.parse_blocks(next, child_indent);
                if !siblings.is_empty() {
                    children.extend(siblings);
                    next = after_siblings;
                }
                let end = children
                    .last()
                    .map(|child| child.range().end)
                    .unwrap_or(line.end);
                return (
                    ParsedBlock {
                        range: introducer..end,
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
                        children,
                    },
                    next,
                );
            }
        }

        let mut head_segments = vec![InlineSegment {
            start: head_start,
            end: line.content_end,
        }];
        let mut next = header_index + 1;
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
                head_segments.push(InlineSegment {
                    start: candidate.start + candidate.indent,
                    end: candidate.content_end,
                });
                next += 1;
                continue;
            }
            break;
        }
        let head = self.parse_inline_segments(&mut head_segments);
        if let Some(consumed) = self.line_index_at_offset(head.range.end.saturating_sub(1)) {
            next = next.max(consumed + 1);
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
        let mut line = self.lines.0[index].clone();
        let introducer = line.start + indent;
        let attr_start = introducer + 1;
        let (limit, header_index) = self.attribute_extent(attr_start, index, indent);
        let (attrs, after_attrs) = self.parse_attributes(attr_start, limit);
        line = self.lines.0[header_index].clone();
        if after_attrs != line.content_end {
            self.diagnostics.push(Diagnostic::error(
                "syntax.invalid-verbatim-block-dispatch",
                "verbatim block attributes must be followed by end of line",
                introducer..line.content_end,
            ));
        }
        let body_indent = indent + 2;
        let mut next = header_index + 1;
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
            let head = self.parse_inline_segments(&mut head_segments);
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

    fn parse_inline_segments(&mut self, segments: &mut Vec<InlineSegment>) -> InlineContent {
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
            if frames.len() > 1 && byte == b']' {
                flush_inline_text(self.source, frames.last_mut().unwrap(), cursor);
                position.offset += 1;
                let after_close = position.offset;
                let (attrs, after_attrs) =
                    if after_close < end && self.source.as_bytes()[after_close] == b'{' {
                        self.parse_inline_attributes(segments, &mut position, after_close)
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
            if frames.len() > 1
                && position.offset < end
                && self.source.as_bytes()[position.offset] == b']'
            {
                frames.last_mut().unwrap().items.push(Inline::Text {
                    text: "]".to_string(),
                    range: introducer..position.offset + 1,
                });
                position.offset += 1;
                frames.last_mut().unwrap().text_start = position.offset;
                continue;
            }

            let quotes = self.source[position.offset..end]
                .bytes()
                .take_while(|candidate| *candidate == b'"')
                .count();
            let open = position.offset + quotes;
            if open < end && self.source.as_bytes()[open] == b'[' {
                if let Some((close, after_close)) =
                    find_verbatim_close(self.source, open + 1, end, quotes)
                {
                    let (attrs, after_attrs) =
                        if after_close < end && self.source.as_bytes()[after_close] == b'{' {
                            self.parse_inline_attributes(segments, &mut position, after_close)
                        } else {
                            (Attributes::default(), after_close)
                        };
                    frames.last_mut().unwrap().items.push(Inline::Verbatim {
                        range: introducer..after_attrs,
                        text: self.source[open + 1..close].to_string(),
                        text_range: open + 1..close,
                        quote_count: quotes,
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

            let kind_start = position.offset;
            position.offset = take_name_like(self.source, position.offset, end, marker_char);
            let kind_end = position.offset;
            if position.offset < end && self.source.as_bytes()[position.offset] == b'[' {
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

            self.diagnostics.push(Diagnostic::error(
                "syntax.invalid-inline-dispatch",
                "inline introducer requires an inline kind followed by '['",
                introducer..next_char_end(self.source, position.offset.min(end.saturating_sub(1))),
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
        let mut attr_indent = None;
        let mut last = line_index;
        while let Some(line) = self.lines.0.get(index) {
            if line.blank || line.indent <= owner_indent || line.has_tab_indent {
                break;
            }
            let expected = *attr_indent.get_or_insert(line.indent);
            if line.indent != expected || self.block_dispatch_readonly(index, line.indent).is_some()
            {
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

    fn parse_inline_attributes(
        &mut self,
        segments: &mut Vec<InlineSegment>,
        position: &mut InlinePosition,
        start: usize,
    ) -> (Attributes, usize) {
        let line_index = self
            .line_index_at_offset(start)
            .expect("inline attribute starts on a source line");
        let owner_indent = self.lines.0[line_index].indent;
        let (limit, last_line) = self.attribute_extent(start, line_index, owner_indent);
        for line_index in line_index + 1..=last_line {
            let line = &self.lines.0[line_index];
            let segment = InlineSegment {
                start: line.start + line.indent,
                end: line.content_end,
            };
            if !segments
                .iter()
                .any(|candidate| candidate.start == segment.start)
            {
                segments.push(segment);
            }
        }
        segments.sort_by_key(|segment| segment.start);
        let (attrs, after_attrs) = self.parse_attributes(start, limit);
        if let Some(segment) = segments
            .iter()
            .position(|segment| segment.start <= after_attrs && after_attrs <= segment.end)
        {
            position.segment = segment;
        }
        position.offset = after_attrs;
        (attrs, after_attrs)
    }

    fn line_index_at_offset(&self, offset: usize) -> Option<usize> {
        self.lines
            .0
            .iter()
            .position(|line| line.start <= offset && offset <= line.content_end)
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
        if self.source.as_bytes()[after] == b'{' {
            return Some(BlockDispatch::Verbatim);
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
                                    cursor..(cursor + 2).min(limit),
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

fn flush_inline_text(source: &str, frame: &mut InlineFrame, end: usize) {
    if frame.text_start < end {
        frame.items.push(Inline::Text {
            text: source[frame.text_start..end].to_string(),
            range: frame.text_start..end,
        });
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

fn has_unquoted_closing_brace(source: &str, mut cursor: usize, limit: usize) -> bool {
    let mut quoted = false;
    while cursor < limit {
        match source.as_bytes()[cursor] {
            b'\\' if quoted => cursor = (cursor + 2).min(limit),
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
    fn parses_multiline_attributes_on_marked_and_verbatim_blocks() {
        let source = "`-{.task\n   #write\n   created=\"2026-07-20T09:00:00+08:00\"\n   } Work\n`{.$\n  #equation\n  language=tex\n  }\n  E = mc^2\n";
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
        assert!(math.attrs.has_class("$"));
        assert_eq!(math.attrs.value("language"), Some("tex"));
        assert_eq!(math.text, "E = mc^2\n");
    }

    #[test]
    fn parses_multiline_attributes_on_inline_owners() {
        let source = "Text `span[value]{\n  #inline\n  .mark\n  key=value\n  } tail\ncontinued paragraph\n\nRaw `[x]{\n  .$\n  language=tex\n  } end\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let Block::Parsed(first) = &parsed.syntax.blocks[0] else {
            panic!("expected first paragraph");
        };
        let Inline::Element { attrs, .. } = &first.head.items[1] else {
            panic!("expected span");
        };
        assert_eq!(attrs.id(), Some("inline"));
        assert!(attrs.has_class("mark"));
        assert_eq!(attrs.value("key"), Some("value"));
        assert_eq!(
            first.head.plain_text(),
            "Text value tail continued paragraph"
        );

        let Block::Parsed(second) = &parsed.syntax.blocks[1] else {
            panic!("expected second paragraph");
        };
        let Inline::Verbatim { attrs, .. } = &second.head.items[1] else {
            panic!("expected verbatim inline");
        };
        assert!(attrs.has_class("$"));
        assert_eq!(attrs.value("language"), Some("tex"));
        assert_eq!(second.head.plain_text(), "Raw x end");
    }

    #[test]
    fn multiline_attributes_require_aligned_continuations_and_single_line_quotes() {
        let misaligned = parse("`node{.one\n   key=value\n  } Head\n");
        assert!(!misaligned.is_valid());
        assert!(misaligned
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "syntax.unclosed-attributes"));

        let quoted = parse("`node{key=\"first\n  second\"\n  } Head\n");
        assert!(!quoted.is_valid());
        assert!(quoted
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "syntax.unclosed-quoted-value"));

        let duplicate =
            parse("`node{#one\n  #two\n  key=one\n  key=two\n  escaped=\"bad\\q\"\n  } Head\n");
        let codes = duplicate
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();
        assert!(codes.contains(&"syntax.duplicate-id"));
        assert!(codes.contains(&"syntax.duplicate-key"));
        assert!(codes.contains(&"syntax.unknown-quoted-escape"));
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
        let Inline::Element { content, .. } = &paragraph.head.items[1] else {
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

        let attributed = parse(format!("`-{{.class}} `- a\n{}`- b\n", " ".repeat(11)));
        assert!(attributed.is_valid(), "{:?}", attributed.diagnostics);
        let Block::Parsed(outer) = &attributed.syntax.blocks[0] else {
            panic!("expected attributed outer item");
        };
        assert!(outer.mark.as_ref().unwrap().attrs.has_class("class"));
        assert_eq!(outer.children.len(), 2);
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
