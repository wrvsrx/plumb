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
    let (blocks, _) = parser.parse_blocks(0, 0);
    let attrs = Attributes {
        items: project_block_attributes(&source, &blocks),
        ..Attributes::default()
    };
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
            if !block.children.is_empty() || block.raw.is_some() {
                return None;
            }
            if mark.marker == "+" {
                let value = block.head.plain_text();
                if value.is_empty() {
                    return None;
                }
                return Some(AttrItem::Class {
                    value,
                    range: block.range.clone(),
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
            if mark.marker != "=" {
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

fn project_inline_member_attributes(source: &str, members: &[InlineMember]) -> Vec<AttrItem> {
    members
        .iter()
        .filter_map(InlineMember::child)
        .filter_map(|inline| project_inline_attribute(source, inline))
        .collect()
}

fn project_inline_attribute(source: &str, inline: &Inline) -> Option<AttrItem> {
    let Inline::Element {
        range,
        kind,
        kind_range: _,
        members,
        ..
    } = inline
    else {
        return None;
    };
    let arguments = members
        .iter()
        .filter_map(InlineMember::argument)
        .collect::<Vec<_>>();
    if kind == "+" {
        let [argument] = arguments.as_slice() else {
            return None;
        };
        let value = argument.plain_text();
        if value.is_empty() {
            return None;
        }
        return Some(AttrItem::Class {
            value,
            range: argument_range(argument),
        });
    }
    if kind == "@" {
        let [argument] = arguments.as_slice() else {
            return None;
        };
        let value = argument.plain_text();
        if value.is_empty() {
            return None;
        }
        return Some(AttrItem::Id {
            value,
            range: range.clone(),
        });
    }
    if kind != "=" {
        return None;
    }
    let (key, key_range, value_range, value) = match arguments.as_slice() {
        [key_argument, value_argument] => {
            let (key, key_range) = plain_argument_key(key_argument)?;
            let value = value_argument.plain_text();
            (key, key_range, argument_range(value_argument), value)
        }
        _ => return None,
    };
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
}

fn argument_range(argument: &InlineArgumentRef<'_>) -> SourceRange {
    match argument {
        InlineArgumentRef::Parsed(content) => content.range.clone(),
        InlineArgumentRef::Verbatim(argument) => argument.range.clone(),
    }
}

fn plain_argument_key(argument: &InlineArgumentRef<'_>) -> Option<(String, SourceRange)> {
    match argument {
        InlineArgumentRef::Parsed(content) => plain_key(content),
        InlineArgumentRef::Verbatim(argument) if !argument.text.is_empty() => {
            Some((argument.text.clone(), argument.text_range.clone()))
        }
        InlineArgumentRef::Verbatim(_) => None,
    }
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
            members,
            range,
            ..
        } if kind == "()" => {
            let mut arguments = members.iter().filter_map(InlineMember::argument);
            let argument = arguments.next()?;
            if arguments.next().is_some() {
                return None;
            }
            (argument.plain_text(), range.clone())
        }
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

fn plain_key(content: &InlineContent) -> Option<(String, SourceRange)> {
    if content.items.is_empty()
        || content.items.iter().any(|inline| {
            !matches!(
                inline,
                Inline::Text { .. } | Inline::Space { .. } | Inline::SoftBreak { .. }
            )
        })
    {
        return None;
    }
    let key = content.plain_text();
    (!key.is_empty()).then(|| (key, content.range.clone()))
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
        Inline::Element { members, .. } => {
            for member in members {
                match member {
                    InlineMember::ParsedArgument(argument) => {
                        output.push_str(&argument.content.plain_text());
                    }
                    InlineMember::VerbatimArgument(argument) => output.push_str(&argument.text),
                    InlineMember::Child { inline, .. } => {
                        append_inline_plain_text(inline, output);
                    }
                }
            }
        }
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
    start: usize,
    kind: String,
    kind_range: SourceRange,
    members: Vec<InlineMember>,
    child_separator: Option<SourceRange>,
}

struct InlineFrame {
    start: usize,
    text_start: usize,
    items: Vec<Inline>,
    opening: Option<InlineOpening>,
    separator_range: Option<SourceRange>,
    member_complete: bool,
}

struct BlockFrame {
    cursor: usize,
    indent: usize,
    same_line: bool,
    blocks: Vec<Block>,
    owner: Option<ParsedBlock>,
    owner_indent: Option<usize>,
}

struct MarkedParse {
    block: ParsedBlock,
    next: usize,
    child: Option<(usize, usize, bool)>,
}

fn finish_block_frame(
    source: &str,
    frames: &mut Vec<BlockFrame>,
    next: usize,
) -> Option<(Vec<Block>, usize)> {
    let frame = frames.pop().expect("block parser always has a frame");
    let Some(mut owner) = frame.owner else {
        return Some((frame.blocks, next));
    };
    owner.children = frame.blocks;
    if let Some(mark) = &mut owner.mark {
        mark.attrs.items = if mark.marker == "=" {
            Vec::new()
        } else {
            project_block_attributes(source, &owner.children)
        };
        mark.attrs.range = projected_attribute_range(&mark.attrs.items);
    }
    if let Some(last) = owner.children.last() {
        owner.range.end = last.range().end;
    }
    if let Some(raw) = &owner.raw {
        owner.range.end = raw.range.end;
    }
    let parent = frames
        .last_mut()
        .expect("an owner frame always has a parent sequence");
    parent.blocks.push(Block::Parsed(owner));
    parent.cursor = next;
    None
}

fn projected_attribute_range(items: &[AttrItem]) -> Option<SourceRange> {
    let start = items.first().map(attr_item_range)?.start;
    let end = items.last().map(attr_item_range)?.end;
    Some(start..end)
}

fn attr_item_range(item: &AttrItem) -> &SourceRange {
    match item {
        AttrItem::Id { range, .. }
        | AttrItem::Class { range, .. }
        | AttrItem::Pair { range, .. } => range,
    }
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
    fn parse_blocks(&mut self, mut index: usize, indent: usize) -> (Vec<Block>, usize) {
        let mut frames = vec![BlockFrame {
            cursor: index,
            indent,
            same_line: false,
            blocks: Vec::new(),
            owner: None,
            owner_indent: None,
        }];
        loop {
            index = frames.last().unwrap().cursor;
            if index >= self.lines.0.len() {
                if let Some(result) = finish_block_frame(self.source, &mut frames, index) {
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
                let owner_indent = frames.last().unwrap().owner_indent;
                if let Some((owner_indent, quote_count)) = owner_indent.and_then(|column| {
                    self.raw_tail_quote_count(index, column)
                        .map(|quote_count| (column, quote_count))
                }) {
                    let (raw, next) = self.parse_raw_payload(index, owner_indent, quote_count);
                    frames.last_mut().unwrap().owner.as_mut().unwrap().raw = Some(raw);
                    if let Some(result) = finish_block_frame(self.source, &mut frames, next) {
                        return result;
                    }
                    continue;
                }
                if let Some(result) = finish_block_frame(self.source, &mut frames, index) {
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
            if let Some(quote_count) = self.raw_tail_quote_count(index, effective_indent) {
                let start = current.start + effective_indent;
                self.diagnostics.push(Diagnostic::error(
                    "syntax.unattached-raw-tail",
                    "raw-tail boundary has no open marked owner at this column",
                    start..start + 1 + quote_count,
                ));
                frames.last_mut().unwrap().cursor += 1;
                continue;
            }
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
                                owner_indent: Some(effective_indent),
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

    fn raw_tail_quote_count(&self, index: usize, indent: usize) -> Option<usize> {
        let Some(line) = self.lines.0.get(index) else {
            return None;
        };
        let start = line.start + indent;
        if line.blank || line.has_tab_indent || line.indent != indent {
            return None;
        }
        if self.source.as_bytes().get(start) != Some(&b'|') {
            return None;
        }
        let quote_start = start + 1;
        let quote_count = self.source[quote_start..line.content_end]
            .bytes()
            .take_while(|byte| *byte == b'"')
            .count();
        (quote_count > 0
            && self.source[quote_start + quote_count..line.content_end]
                .bytes()
                .all(|byte| matches!(byte, b' ' | b'\t')))
        .then_some(quote_count)
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
            if kind_end == after && tail == line.content_end {
                return Some(BlockDispatch::Verbatim);
            }
            if tail == line.content_end {
                return Some(BlockDispatch::Marked);
            }
            if self.source.as_bytes()[tail] == b'[' {
                return None;
            }
            if quote_count == 1 && self.source[tail..line.content_end].contains('"') {
                return None;
            }
            if kind_end == after {
                self.diagnostics.push(Diagnostic::error(
                    "syntax.invalid-verbatim-block-dispatch",
                    "anonymous verbatim block opener must end the physical line",
                    start..line.content_end,
                ));
                return Some(BlockDispatch::Verbatim);
            }
            return Some(BlockDispatch::Marked);
        }
        if kind_end < line.content_end && self.source.as_bytes()[kind_end] == b'[' {
            return None;
        }
        Some(BlockDispatch::Marked)
    }

    fn parse_marked(&mut self, index: usize, indent: usize) -> MarkedParse {
        let line = self.lines.0[index].clone();
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
                    "marker must be followed by whitespace or end of line",
                    introducer..next_char_end(self.source, cursor),
                ));
                cursor
            }
        } else {
            cursor
        };

        if head_start < line.content_end {
            let child_indent = head_start - line.start;
            if self.block_dispatch(index, child_indent).is_some() {
                return MarkedParse {
                    block: ParsedBlock {
                        range: introducer..line.end,
                        mark: Some(Mark {
                            range: introducer..mark_end,
                            marker,
                            marker_range,
                            attrs: Attributes::default(),
                        }),
                        head: InlineContent {
                            range: head_start..head_start,
                            items: Vec::new(),
                        },
                        children: Vec::new(),
                        raw: None,
                    },
                    next: index + 1,
                    child: Some((index, child_indent, true)),
                };
            }
        }

        let mut head_segments = vec![InlineSegment {
            start: head_start,
            end: line.content_end,
        }];
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
                head_segments.push(InlineSegment {
                    start: candidate.start + candidate.indent,
                    end: candidate.content_end,
                });
                next += 1;
                continue;
            }
            break;
        }
        let head = self.parse_inline_segments(&mut head_segments, false);
        if let Some(consumed) = self.line_index_at_offset(head.range.end.saturating_sub(1)) {
            next = next.max(consumed + 1);
        }

        let mut child_start = next;
        while child_start < self.lines.0.len() && self.lines.0[child_start].blank {
            child_start += 1;
        }
        let child = if child_start < self.lines.0.len() && self.lines.0[child_start].indent > indent
        {
            Some((child_start, self.lines.0[child_start].indent, false))
        } else if self.raw_tail_quote_count(child_start, indent).is_some() {
            Some((child_start, indent + 1, false))
        } else {
            None
        };
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
                    attrs: Attributes::default(),
                }),
                head,
                children: Vec::new(),
                raw: None,
            },
            next,
            child,
        }
    }

    fn parse_verbatim(&mut self, index: usize, indent: usize) -> (VerbatimBlock, usize) {
        let line = self.lines.0[index].clone();
        let introducer = line.start + indent;
        let quote_start = introducer + 1;
        let quote_count = self.source[quote_start..line.content_end]
            .bytes()
            .take_while(|byte| *byte == b'"')
            .count();
        debug_assert!(quote_count > 0);
        let after_quote = quote_start + quote_count;
        let (raw, next) = self.parse_raw_payload(index, indent, quote_count);
        (
            VerbatimBlock {
                range: introducer..raw.range.end,
                opener_range: introducer..after_quote,
                kind: String::new(),
                kind_range: quote_start..quote_start,
                quote_count,
                text: raw.text,
                text_range: raw.text_range,
            },
            next,
        )
    }

    fn parse_raw_payload(
        &mut self,
        boundary_index: usize,
        owner_indent: usize,
        quote_count: usize,
    ) -> (RawPayload, usize) {
        let boundary = self.lines.0[boundary_index].clone();
        let boundary_start = boundary.start + owner_indent;
        let boundary_range = if self.source.as_bytes()[boundary_start] == b'`' {
            boundary_start + 1..boundary_start + 1 + quote_count
        } else {
            boundary_start..boundary_start + 1 + quote_count
        };
        let body_indent = owner_indent + quote_count;
        let mut next = boundary_index + 1;
        let mut text = String::new();
        let text_start = self
            .lines
            .0
            .get(next)
            .map_or(boundary.end, |next| next.start);
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
                if candidate.indent > owner_indent {
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
            RawPayload {
                range: boundary_start..text_end.max(boundary.end),
                boundary_range,
                quote_count,
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
                raw: None,
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
            separator_range: None,
            member_complete: false,
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
                if !frame.member_complete && (nested || !frame.items.is_empty()) {
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

            if frames.len() > 1 {
                let frame = frames.last_mut().unwrap();
                if frame.member_complete && !matches!(byte, b'|' | b']') {
                    let trailer_end = self.source[cursor..end]
                        .find(['|', ']'])
                        .map_or(end, |relative| cursor + relative);
                    self.diagnostics.push(Diagnostic::error(
                        "syntax.trailing-after-inline-member",
                        "a completed inline member must be followed by '|' or the owner close",
                        cursor..trailer_end.max(next_char_end(self.source, cursor)),
                    ));
                    position.offset = trailer_end.max(next_char_end(self.source, cursor));
                    frame.text_start = position.offset;
                    continue;
                }

                let at_member_start =
                    !frame.member_complete && frame.items.is_empty() && cursor == frame.start;
                if at_member_start
                    && frame.separator_range.is_some()
                    && byte == b'"'
                    && starts_full_verbatim_envelope(self.source, cursor, end)
                {
                    let separator_range = frame.separator_range.clone();
                    if let Some(argument) = self.parse_verbatim_argument(
                        cursor,
                        end,
                        separator_range,
                        "verbatim argument",
                    ) {
                        position.offset = argument.range.end;
                        frame
                            .opening
                            .as_mut()
                            .unwrap()
                            .members
                            .push(InlineMember::VerbatimArgument(argument));
                        frame.member_complete = true;
                        frame.text_start = position.offset;
                    } else {
                        position.offset = end;
                        frame.text_start = end;
                    }
                    continue;
                }

                if at_member_start && frame.separator_range.is_some() && byte != b'`' {
                    let kind_end = take_name_like(self.source, cursor, end, marker_char);
                    if kind_end > cursor && kind_end < end {
                        match self.source.as_bytes()[kind_end] {
                            b'[' => {
                                let separator_range = frame.separator_range.clone().unwrap();
                                position.offset = kind_end + 1;
                                frames.push(InlineFrame {
                                    start: position.offset,
                                    text_start: position.offset,
                                    items: Vec::new(),
                                    opening: Some(InlineOpening {
                                        start: cursor,
                                        kind: self.source[cursor..kind_end].to_string(),
                                        kind_range: cursor..kind_end,
                                        members: Vec::new(),
                                        child_separator: Some(separator_range),
                                    }),
                                    separator_range: None,
                                    member_complete: false,
                                });
                                continue;
                            }
                            b'"' if starts_full_verbatim_envelope(self.source, kind_end, end) => {
                                let separator_range = frame.separator_range.clone().unwrap();
                                if let Some(argument) = self.parse_verbatim_argument(
                                    kind_end,
                                    end,
                                    None,
                                    "verbatim child",
                                ) {
                                    let after = argument.range.end;
                                    let inline = Inline::Verbatim {
                                        range: cursor..after,
                                        kind: self.source[cursor..kind_end].to_string(),
                                        kind_range: cursor..kind_end,
                                        text: argument.text,
                                        text_range: argument.text_range,
                                        quote_count: argument.quote_count,
                                        bracketed: argument.bracketed,
                                        attrs: Attributes::default(),
                                    };
                                    frame.opening.as_mut().unwrap().members.push(
                                        InlineMember::Child {
                                            range: cursor..after,
                                            separator_range,
                                            inline: Box::new(inline),
                                        },
                                    );
                                    frame.member_complete = true;
                                    position.offset = after;
                                    frame.text_start = after;
                                } else {
                                    position.offset = end;
                                    frame.text_start = end;
                                }
                                continue;
                            }
                            _ => {}
                        }
                    }
                }
            }

            // Brackets remain structural. Braces are ordinary parsed text.
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
            if byte == b'|' {
                if frames.len() == 1 {
                    flush_inline_text(self.source, frames.last_mut().unwrap(), cursor);
                    self.diagnostics.push(Diagnostic::error(
                        "syntax.unexpected-member-separator",
                        "an inline member separator must occur inside an inline element",
                        cursor..cursor + 1,
                    ));
                    position.offset += 1;
                    frames.last_mut().unwrap().text_start = position.offset;
                    continue;
                }

                let frame = frames.last_mut().unwrap();
                if !frame.member_complete {
                    flush_inline_text(self.source, frame, cursor);
                    frame
                        .opening
                        .as_mut()
                        .unwrap()
                        .members
                        .push(InlineMember::ParsedArgument(InlineArgument {
                            range: frame.start..cursor,
                            separator_range: frame.separator_range.clone(),
                            content: InlineContent {
                                range: frame.start..cursor,
                                items: std::mem::take(&mut frame.items),
                            },
                        }));
                }
                position.offset += 1;
                frame.start = position.offset;
                frame.text_start = position.offset;
                frame.items.clear();
                frame.separator_range = Some(cursor..cursor + 1);
                frame.member_complete = false;
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
                let frame = frames.last_mut().unwrap();
                if !frame.member_complete {
                    flush_inline_text(self.source, frame, cursor);
                    frame
                        .opening
                        .as_mut()
                        .unwrap()
                        .members
                        .push(InlineMember::ParsedArgument(InlineArgument {
                            range: frame.start..cursor,
                            separator_range: frame.separator_range.clone(),
                            content: InlineContent {
                                range: frame.start..cursor,
                                items: std::mem::take(&mut frame.items),
                            },
                        }));
                }
                position.offset += 1;
                let after_close = position.offset;
                let mut frame = frames.pop().unwrap();
                let opening = frame.opening.take().unwrap();
                let attrs = Attributes {
                    range: None,
                    items: project_inline_member_attributes(self.source, &opening.members),
                };
                let inline = Inline::Element {
                    range: opening.start..after_close,
                    kind: opening.kind,
                    kind_range: opening.kind_range,
                    members: opening.members,
                    attrs,
                };
                let parent = frames.last_mut().unwrap();
                if let Some(separator_range) = opening.child_separator {
                    parent
                        .opening
                        .as_mut()
                        .unwrap()
                        .members
                        .push(InlineMember::Child {
                            range: opening.start..after_close,
                            separator_range,
                            inline: Box::new(inline),
                        });
                    parent.member_complete = true;
                } else {
                    parent.items.push(inline);
                }
                parent.text_start = after_close;
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
            // §2: a single introducer before an inline structural
            // delimiters is an unconditional literal escape.
            if position.offset < end
                && matches!(self.source.as_bytes()[position.offset], b'[' | b']' | b'|')
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
                    frames.last_mut().unwrap().items.push(Inline::Verbatim {
                        range: introducer..after_close,
                        kind,
                        kind_range: kind_start..kind_end,
                        text: self.source[bracket_open + 1..close].to_string(),
                        text_range: bracket_open + 1..close,
                        quote_count,
                        bracketed: true,
                        attrs: Attributes::default(),
                    });
                    position.offset = after_close;
                    frames.last_mut().unwrap().text_start = after_close;
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
                    frames.last_mut().unwrap().items.push(Inline::Verbatim {
                        range: introducer..after_close,
                        kind,
                        kind_range: kind_start..kind_end,
                        text: self.source[payload_start..close].to_string(),
                        text_range: payload_start..close,
                        quote_count: 1,
                        bracketed: false,
                        attrs: Attributes::default(),
                    });
                    position.offset = after_close;
                    frames.last_mut().unwrap().text_start = after_close;
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
                debug_assert!(kind_end > kind_start, "inline kinds are nonempty");
                position.offset += 1;
                frames.push(InlineFrame {
                    start: position.offset,
                    text_start: position.offset,
                    items: Vec::new(),
                    opening: Some(InlineOpening {
                        start: introducer,
                        kind: self.source[kind_start..kind_end].to_string(),
                        kind_range: kind_start..kind_end,
                        members: Vec::new(),
                        child_separator: None,
                    }),
                    separator_range: None,
                    member_complete: false,
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
                    opening.start..position.offset,
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

    fn parse_verbatim_argument(
        &mut self,
        quote_start: usize,
        limit: usize,
        separator_range: Option<SourceRange>,
        construct: &str,
    ) -> Option<VerbatimArgument> {
        let quote_count = self.source[quote_start..limit]
            .bytes()
            .take_while(|candidate| *candidate == b'"')
            .count();
        debug_assert!(quote_count > 0);
        let bracket_open = quote_start + quote_count;
        debug_assert!(
            bracket_open < limit && self.source.as_bytes()[bracket_open] == b'[',
            "verbatim members require a full bracket envelope"
        );
        if let Some((close, after_close)) =
            find_verbatim_close(self.source, bracket_open + 1, limit, quote_count)
        {
            return Some(VerbatimArgument {
                range: quote_start..after_close,
                separator_range,
                text: self.source[bracket_open + 1..close].to_string(),
                text_range: bracket_open + 1..close,
                quote_count,
                bracketed: true,
            });
        }

        self.diagnostics.push(Diagnostic::error(
            "syntax.unclosed-verbatim-member",
            format!("{construct} must close before the enclosing inline boundary"),
            quote_start..limit,
        ));
        None
    }

    fn line_index_at_offset(&self, offset: usize) -> Option<usize> {
        let index = self
            .lines
            .0
            .partition_point(|line| line.start <= offset)
            .checked_sub(1)?;
        (offset <= self.lines.0[index].content_end).then_some(index)
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
        && !matches!(character, '`' | '"' | '[' | ']' | '|')
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

fn has_space_prefix(source: &str, start: usize, width: usize) -> bool {
    source
        .as_bytes()
        .get(start..start.saturating_add(width))
        .is_some_and(|prefix| prefix.iter().all(|byte| *byte == b' '))
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

fn starts_full_verbatim_envelope(source: &str, quote_start: usize, limit: usize) -> bool {
    let quote_count = source[quote_start..limit]
        .bytes()
        .take_while(|byte| *byte == b'"')
        .count();
    let bracket_open = quote_start + quote_count;
    quote_count > 0 && bracket_open < limit && source.as_bytes()[bracket_open] == b'['
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
    fn projects_direct_declaration_children_without_reordering_them() {
        let source = "`task Work\n\n `note first\n\n `@ work\n\n `= due tomorrow\n\n `note last\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Parsed(task) = &parsed.syntax.blocks[0] else {
            panic!("expected task");
        };
        let attrs = &task.mark.as_ref().unwrap().attrs;
        assert_eq!(attrs.id(), Some("work"));
        assert_eq!(attrs.value("due"), Some("tomorrow"));
        assert_eq!(task.children.len(), 4);
        assert_eq!(
            task.children
                .iter()
                .filter_map(|child| match child {
                    Block::Parsed(child) => child.mark.as_ref().map(|mark| mark.marker.as_str()),
                    Block::Verbatim(_) => None,
                })
                .collect::<Vec<_>>(),
            ["note", "@", "=", "note"]
        );
    }

    #[test]
    fn parsed_owner_has_one_raw_tail_after_its_children() {
        let source = "`rust\n\n `@ example\n\n `note nested\n\n|\"\n fn main() {}\n \"\n tail\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Parsed(owner) = &parsed.syntax.blocks[0] else {
            panic!("expected parsed raw owner");
        };
        assert_eq!(owner.mark.as_ref().unwrap().marker, "rust");
        assert_eq!(owner.mark.as_ref().unwrap().attrs.id(), Some("example"));
        assert_eq!(owner.children.len(), 2);
        assert_eq!(owner.raw.as_ref().unwrap().text, "fn main() {}\n\"\ntail\n");
    }

    #[test]
    fn anonymous_raw_is_compact_and_can_be_an_ordinary_child() {
        let parsed = parse("`example\n\n `\"\n  first raw child\n\n `\"\n  second raw child\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Parsed(owner) = &parsed.syntax.blocks[0] else {
            panic!("expected owner");
        };
        assert!(owner.raw.is_none());
        assert!(
            matches!(&owner.children[..], [Block::Verbatim(first), Block::Verbatim(second)] if first.text == "first raw child\n" && second.text == "second raw child\n")
        );

        let anonymous = parse("`\"\n anonymous\n");
        assert!(anonymous.is_valid(), "{:?}", anonymous.diagnostics);
        assert!(
            matches!(&anonymous.syntax.blocks[..], [Block::Verbatim(raw)] if raw.text == "anonymous\n")
        );
    }

    #[test]
    fn quote_only_paragraph_is_ordinary_text_and_unattached_raw_tail_is_an_error() {
        let parsed = parse("\"\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let [Block::Parsed(paragraph)] = parsed.syntax.blocks.as_slice() else {
            panic!("expected quote paragraph");
        };
        assert_eq!(paragraph.head.plain_text(), "\"");

        let parsed = parse("|\"\n raw\n");
        assert!(!parsed.is_valid());
        assert!(parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "syntax.unattached-raw-tail"));
    }

    #[test]
    fn braces_are_ordinary_lossless_text() {
        let source = "Text { fn() {} }\n\n`marker{brace} Head {content}\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Parsed(paragraph) = &parsed.syntax.blocks[0] else {
            panic!("expected paragraph");
        };
        assert_eq!(paragraph.head.plain_text(), "Text { fn() {} }");
        let Block::Parsed(marked) = &parsed.syntax.blocks[1] else {
            panic!("expected marked block");
        };
        assert_eq!(marked.mark.as_ref().unwrap().marker, "marker{brace}");
        assert_eq!(marked.head.plain_text(), "Head {content}");
        assert_eq!(parsed.lossless.reconstruct(source), source);
    }

    #[test]
    fn legacy_named_raw_opener_is_not_current_syntax() {
        let parsed = parse("`rust\"\n code\n");
        assert!(!parsed.is_valid());
    }

    #[test]
    fn parses_inline_arguments_children_and_projected_declarations() {
        let source = "`pair[first|\"[second raw]\"|tag[value]|code\"[raw child]\"|@[main]|+[external]|=[to|guide.plumb]]\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Parsed(paragraph) = &parsed.syntax.blocks[0] else {
            panic!("expected paragraph");
        };
        let [Inline::Element { members, attrs, .. }] = paragraph.head.items.as_slice() else {
            panic!("expected one inline element");
        };
        assert_eq!(members.len(), 7);
        assert_eq!(attrs.id(), Some("main"));
        assert!(attrs.has_class("external"));
        assert_eq!(attrs.value("to"), Some("guide.plumb"));
        assert!(
            matches!(&members[0], InlineMember::ParsedArgument(argument) if argument.content.plain_text() == "first")
        );
        assert!(
            matches!(&members[1], InlineMember::VerbatimArgument(argument) if argument.text == "second raw")
        );
        assert!(
            matches!(&members[2], InlineMember::Child { inline, .. } if matches!(inline.as_ref(), Inline::Element { kind, .. } if kind == "tag"))
        );
        assert!(
            matches!(&members[3], InlineMember::Child { inline, .. } if matches!(inline.as_ref(), Inline::Verbatim { kind, text, .. } if kind == "code" && text == "raw child"))
        );
    }

    #[test]
    fn multiline_inline_recovers_before_block_boundaries() {
        let parsed = parse("`parent `span[open\n  `child Boundary\n");
        assert!(!parsed.is_valid());
        assert!(parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "syntax.unclosed-inline"));
        let Block::Parsed(parent) = &parsed.syntax.blocks[0] else {
            panic!("expected parent");
        };
        assert_eq!(parent.children.len(), 1);
    }

    #[test]
    fn same_line_first_child_matches_indented_children() {
        let compact = parse("`- `- a\n   `- b\n   `- c\n");
        let expanded = parse("`-\n   `- a\n   `- b\n   `- c\n");
        assert!(compact.is_valid(), "{:?}", compact.diagnostics);
        assert!(expanded.is_valid(), "{:?}", expanded.diagnostics);
        let Block::Parsed(compact_outer) = &compact.syntax.blocks[0] else {
            panic!("expected compact outer block");
        };
        let Block::Parsed(expanded_outer) = &expanded.syntax.blocks[0] else {
            panic!("expected expanded outer block");
        };
        assert!(compact_outer.head.items.is_empty());
        assert_eq!(compact_outer.children.len(), 3);
        assert_eq!(compact_outer.children.len(), expanded_outer.children.len());
    }

    #[test]
    fn inline_and_block_nesting_use_explicit_stacks() {
        const INLINE_DEPTH: usize = 4_096;
        let mut inline = "`x[".repeat(INLINE_DEPTH);
        inline.push_str("value");
        inline.push_str(&"]".repeat(INLINE_DEPTH));
        inline.push('\n');
        let parsed = parse(inline);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        const BLOCK_DEPTH: usize = 20_000;
        let mut blocks = "`x ".repeat(BLOCK_DEPTH);
        blocks.push_str("leaf\n");
        let parsed = parse(blocks);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let mut level = parsed.syntax.blocks.as_slice();
        let mut depth = 0;
        while let [Block::Parsed(block)] = level {
            depth += 1;
            level = &block.children;
        }
        assert_eq!(depth, BLOCK_DEPTH);
    }

    #[test]
    fn quote_count_declares_anonymous_raw_margin() {
        let parsed = parse("`\"\"\n  first\n    indented\nnext\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Verbatim(raw) = &parsed.syntax.blocks[0] else {
            panic!("expected anonymous raw block");
        };
        assert_eq!(raw.quote_count, 2);
        assert_eq!(raw.text, "first\n  indented\n");
        assert_eq!(parsed.syntax.blocks.len(), 2);
    }

    #[test]
    fn quote_count_declares_marked_raw_tail_margin() {
        let parsed = parse("`rust\n|\"\"\"\n   first\n     indented\nnext\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Parsed(owner) = &parsed.syntax.blocks[0] else {
            panic!("expected marked owner");
        };
        let raw = owner.raw.as_ref().expect("expected raw tail");
        assert_eq!(raw.quote_count, 3);
        assert_eq!(raw.text, "first\n  indented\n");
        assert_eq!(raw.boundary_range, 6..10);
        assert_eq!(parsed.syntax.blocks.len(), 2);
    }

    #[test]
    fn compact_quotes_inside_members_remain_parsed_arguments() {
        let parsed = parse("`owner[\"quoted\"|code\"raw\"|\"[verbatim]\"|code\"[child]\"]\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Parsed(paragraph) = &parsed.syntax.blocks[0] else {
            panic!("expected paragraph");
        };
        let [Inline::Element { members, .. }] = paragraph.head.items.as_slice() else {
            panic!("expected owner");
        };
        assert!(
            matches!(&members[0], InlineMember::ParsedArgument(argument) if argument.content.plain_text() == "\"quoted\"")
        );
        assert!(
            matches!(&members[1], InlineMember::ParsedArgument(argument) if argument.content.plain_text() == "code\"raw\"")
        );
        assert!(
            matches!(&members[2], InlineMember::VerbatimArgument(argument) if argument.text == "verbatim")
        );
        assert!(
            matches!(&members[3], InlineMember::Child { inline, .. } if matches!(inline.as_ref(), Inline::Verbatim { kind, text, .. } if kind == "code" && text == "child"))
        );
    }

    #[test]
    fn first_inline_member_must_be_a_parsed_argument() {
        for source in ["`owner[\"[raw]\"]\n", "`owner[child[value]]\n"] {
            let parsed = parse(source);
            assert!(!parsed.is_valid(), "{source:?} unexpectedly parsed");
            assert!(
                parsed
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == "syntax.unattached-bracket"),
                "{:?}",
                parsed.diagnostics
            );
        }

        let parsed = parse("`owner[|\"[raw]\"]\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let Block::Parsed(paragraph) = &parsed.syntax.blocks[0] else {
            panic!("expected paragraph");
        };
        let [Inline::Element { members, .. }] = paragraph.head.items.as_slice() else {
            panic!("expected owner");
        };
        assert!(
            matches!(&members[0], InlineMember::ParsedArgument(argument) if argument.content.items.is_empty())
        );
        assert!(
            matches!(&members[1], InlineMember::VerbatimArgument(argument) if argument.text == "raw")
        );
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
