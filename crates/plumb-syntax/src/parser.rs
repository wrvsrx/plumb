use crate::lossless::build_lossless;
use crate::syntax::{
    AttrItem, AttrValue, Attributes, Block, Diagnostic, Document, Inline, InlineContent, Mark,
    ParsedBlock, ParsedDocument, SourceRange, VerbatimBlock,
};

#[derive(Debug, PartialEq, Eq)]
pub struct IncrementalParse {
    pub document: ParsedDocument,
    pub old_reparsed_range: SourceRange,
    pub reparsed_range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceChange {
    pub old_range: SourceRange,
    pub new_range: SourceRange,
}

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

pub fn parse_incremental(previous: &ParsedDocument, source: impl Into<String>) -> IncrementalParse {
    let source = source.into();
    if previous.source == source {
        let end = source.len();
        return IncrementalParse {
            document: parse(source),
            old_reparsed_range: 0..previous.source.len(),
            reparsed_range: 0..end,
        };
    }
    let (old_range, new_range) = changed_ranges(&previous.source, &source);
    parse_incremental_from_change(
        previous,
        source,
        SourceChange {
            old_range,
            new_range,
        },
    )
}

pub fn parse_incremental_from_change(
    previous: &ParsedDocument,
    source: impl Into<String>,
    change: SourceChange,
) -> IncrementalParse {
    let source = source.into();
    if !valid_source_change(&previous.source, &source, &change) {
        return parse_incremental(previous, source);
    }
    let plan = incremental_plan(previous, &source, &change.old_range);
    if plan.old.start == 0 && plan.old.end == previous.source.len() {
        let end = source.len();
        return IncrementalParse {
            document: parse(source),
            old_reparsed_range: 0..previous.source.len(),
            reparsed_range: 0..end,
        };
    }

    incremental_parse_with_plan(previous, source, plan).unwrap_or_else(|source| {
        let end = source.len();
        IncrementalParse {
            document: parse(source),
            old_reparsed_range: 0..previous.source.len(),
            reparsed_range: 0..end,
        }
    })
}

#[derive(Debug, Clone)]
struct IncrementalPlan {
    old: SourceRange,
    new: SourceRange,
}

fn incremental_plan(
    previous: &ParsedDocument,
    source: &str,
    changed_old: &SourceRange,
) -> IncrementalPlan {
    let old_start = previous
        .syntax
        .blocks
        .iter()
        .map(|block| block.range().start)
        .take_while(|start| *start < changed_old.start)
        .last()
        .unwrap_or(0);
    let (old_end, new_end) = previous
        .syntax
        .blocks
        .iter()
        .map(|block| block.range().start)
        .filter(|start| *start >= changed_old.end)
        .find_map(|old_end| {
            let suffix_len = previous.source.len().checked_sub(old_end)?;
            let new_end = source.len().checked_sub(suffix_len)?;
            (new_end >= old_start && is_line_start(source, new_end)).then_some((old_end, new_end))
        })
        .unwrap_or((previous.source.len(), source.len()));
    IncrementalPlan {
        old: old_start..old_end,
        new: old_start..new_end,
    }
}

fn valid_source_change(old: &str, new: &str, change: &SourceChange) -> bool {
    change.old_range.start <= change.old_range.end
        && change.old_range.end <= old.len()
        && change.new_range.start <= change.new_range.end
        && change.new_range.end <= new.len()
        && change.old_range.start == change.new_range.start
        && old.is_char_boundary(change.old_range.start)
        && old.is_char_boundary(change.old_range.end)
        && new.is_char_boundary(change.new_range.start)
        && new.is_char_boundary(change.new_range.end)
        && old[..change.old_range.start] == new[..change.new_range.start]
        && old[change.old_range.end..] == new[change.new_range.end..]
}

fn changed_ranges(old: &str, new: &str) -> (SourceRange, SourceRange) {
    let prefix = old
        .chars()
        .zip(new.chars())
        .take_while(|(old, new)| old == new)
        .map(|(character, _)| character.len_utf8())
        .sum::<usize>();
    let suffix = old[prefix..]
        .chars()
        .rev()
        .zip(new[prefix..].chars().rev())
        .take_while(|(old, new)| old == new)
        .map(|(character, _)| character.len_utf8())
        .sum::<usize>();
    (prefix..old.len() - suffix, prefix..new.len() - suffix)
}

fn is_line_start(source: &str, offset: usize) -> bool {
    offset == 0 || source.as_bytes().get(offset.wrapping_sub(1)) == Some(&b'\n')
}

fn incremental_parse_with_plan(
    previous: &ParsedDocument,
    source: String,
    plan: IncrementalPlan,
) -> Result<IncrementalParse, String> {
    if previous
        .syntax
        .blocks
        .iter()
        .any(|block| crosses_boundary(block.range(), &plan.old))
        || previous
            .diagnostics
            .iter()
            .any(|diagnostic| crosses_boundary(&diagnostic.range, &plan.old))
        || previous
            .lossless
            .tokens
            .iter()
            .any(|token| crosses_boundary(&token.range, &plan.old))
        || !reuse_depth_is_bounded(previous, &plan.old)
    {
        return Err(source);
    }

    let mut fragment = parse(source[plan.new.clone()].to_string());
    shift_document(&mut fragment.syntax, plan.new.start as isize);
    shift_diagnostics(&mut fragment.diagnostics, plan.new.start as isize);
    shift_tokens(&mut fragment.lossless.tokens, plan.new.start as isize);

    let suffix_delta = plan.new.end as isize - plan.old.end as isize;
    let mut blocks = previous
        .syntax
        .blocks
        .iter()
        .filter(|block| block.range().end <= plan.old.start)
        .cloned()
        .collect::<Vec<_>>();
    blocks.append(&mut fragment.syntax.blocks);
    let mut suffix_blocks = previous
        .syntax
        .blocks
        .iter()
        .filter(|block| block.range().start >= plan.old.end)
        .cloned()
        .collect::<Vec<_>>();
    shift_blocks(&mut suffix_blocks, suffix_delta);
    blocks.append(&mut suffix_blocks);

    let mut diagnostics = previous
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.range.end <= plan.old.start)
        .cloned()
        .collect::<Vec<_>>();
    diagnostics.append(&mut fragment.diagnostics);
    let mut suffix_diagnostics = previous
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.range.start >= plan.old.end)
        .cloned()
        .collect::<Vec<_>>();
    shift_diagnostics(&mut suffix_diagnostics, suffix_delta);
    diagnostics.append(&mut suffix_diagnostics);
    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));

    let mut tokens = previous
        .lossless
        .tokens
        .iter()
        .filter(|token| token.range.end <= plan.old.start)
        .cloned()
        .collect::<Vec<_>>();
    tokens.append(&mut fragment.lossless.tokens);
    let mut suffix_tokens = previous
        .lossless
        .tokens
        .iter()
        .filter(|token| token.range.start >= plan.old.end)
        .cloned()
        .collect::<Vec<_>>();
    shift_tokens(&mut suffix_tokens, suffix_delta);
    tokens.append(&mut suffix_tokens);

    let mut syntax = Document {
        attrs: Default::default(),
        blocks,
        range: 0..source.len(),
    };
    project_attributes(&source, &mut syntax);
    let document = ParsedDocument {
        lossless: crate::syntax::LosslessTree {
            range: 0..source.len(),
            tokens,
        },
        source,
        syntax,
        diagnostics,
    };
    Ok(IncrementalParse {
        document,
        old_reparsed_range: plan.old,
        reparsed_range: plan.new,
    })
}

fn reuse_depth_is_bounded(previous: &ParsedDocument, changed: &SourceRange) -> bool {
    const MAX_DERIVED_CLONE_DEPTH: usize = 256;
    let reused =
        previous.syntax.blocks.iter().filter(|block| {
            block.range().end <= changed.start || block.range().start >= changed.end
        });
    let mut blocks = reused.map(|block| (block, 1)).collect::<Vec<_>>();
    let mut contents = Vec::new();
    while let Some((block, depth)) = blocks.pop() {
        if depth > MAX_DERIVED_CLONE_DEPTH {
            return false;
        }
        if let Block::Parsed(block) = block {
            contents.push((&block.content, 1));
            blocks.extend(block.children.iter().map(|child| (child, depth + 1)));
        }
    }
    while let Some((content, depth)) = contents.pop() {
        if depth > MAX_DERIVED_CLONE_DEPTH {
            return false;
        }
        contents.extend(content.items.iter().filter_map(|inline| match inline {
            Inline::Group { content, .. } => Some((content, depth + 1)),
            _ => None,
        }));
    }
    true
}

fn crosses_boundary(range: &SourceRange, boundary: &SourceRange) -> bool {
    (range.start < boundary.start && range.end > boundary.start)
        || (range.start < boundary.end && range.end > boundary.end)
}

fn shift_document(document: &mut Document, delta: isize) {
    shift_attributes(&mut document.attrs, delta);
    shift_range(&mut document.range, delta);
    shift_blocks(&mut document.blocks, delta);
}

fn shift_blocks(blocks: &mut [Block], delta: isize) {
    let mut pending = blocks.iter_mut().rev().collect::<Vec<_>>();
    while let Some(block) = pending.pop() {
        match block {
            Block::Parsed(block) => {
                shift_range(&mut block.range, delta);
                if let Some(mark) = &mut block.mark {
                    shift_mark(mark, delta);
                }
                shift_inline_content(&mut block.content, delta);
                pending.extend(block.children.iter_mut().rev());
            }
            Block::Verbatim(block) => {
                shift_range(&mut block.range, delta);
                shift_range(&mut block.opener_range, delta);
                if let Some(mark) = &mut block.mark {
                    shift_mark(mark, delta);
                }
                shift_range(&mut block.quote_range, delta);
                shift_range(&mut block.text_range, delta);
            }
        }
    }
}

fn shift_inline_content(content: &mut InlineContent, delta: isize) {
    let mut pending = vec![content];
    while let Some(content) = pending.pop() {
        shift_range(&mut content.range, delta);
        for inline in &mut content.items {
            match inline {
                Inline::Text { range, .. }
                | Inline::Space { range, .. }
                | Inline::SoftBreak { range } => shift_range(range, delta),
                Inline::Group {
                    range,
                    mark,
                    content,
                } => {
                    shift_range(range, delta);
                    if let Some(mark) = mark {
                        shift_mark(mark, delta);
                    }
                    pending.push(content);
                }
                Inline::Verbatim {
                    range,
                    mark,
                    text_range,
                    ..
                } => {
                    shift_range(range, delta);
                    if let Some(mark) = mark {
                        shift_mark(mark, delta);
                    }
                    shift_range(text_range, delta);
                }
            }
        }
    }
}

fn shift_mark(mark: &mut Mark, delta: isize) {
    shift_range(&mut mark.range, delta);
    shift_range(&mut mark.marker_range, delta);
    shift_attributes(&mut mark.attrs, delta);
}

fn shift_attributes(attributes: &mut Attributes, delta: isize) {
    if let Some(range) = &mut attributes.range {
        shift_range(range, delta);
    }
    for item in &mut attributes.items {
        match item {
            AttrItem::Id {
                value_range, range, ..
            }
            | AttrItem::Class {
                value_range, range, ..
            } => {
                shift_range(value_range, delta);
                shift_range(range, delta);
            }
            AttrItem::Pair {
                key_range,
                value,
                range,
                ..
            } => {
                shift_range(key_range, delta);
                shift_range(&mut value.range, delta);
                shift_range(range, delta);
            }
        }
    }
}

fn shift_diagnostics(diagnostics: &mut [Diagnostic], delta: isize) {
    for diagnostic in diagnostics {
        shift_range(&mut diagnostic.range, delta);
        for related in &mut diagnostic.related {
            shift_range(related, delta);
        }
    }
}

fn shift_tokens(tokens: &mut [crate::syntax::SyntaxToken], delta: isize) {
    for token in tokens {
        shift_range(&mut token.range, delta);
    }
}

fn shift_range(range: &mut SourceRange, delta: isize) {
    range.start = range
        .start
        .checked_add_signed(delta)
        .expect("incremental range start remains in source");
    range.end = range
        .end
        .checked_add_signed(delta)
        .expect("incremental range end remains in source");
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

            let preceded_by_blank = index > 0 && self.lines[index - 1].blank;
            let previous_indent = index
                .checked_sub(1)
                .map_or(line.indent, |previous| self.lines[previous].indent);
            let (block, next_index) = self.parse_block(index);
            index = next_index;
            if !preceded_by_blank && line.indent > levels.last().expect("root level exists").indent
            {
                match self.append_plain_continuation(&mut levels, &line, previous_indent, block) {
                    None => continue,
                    Some(block) => self.place_block(&mut levels, line.indent, block),
                }
            } else {
                self.place_block(&mut levels, line.indent, block);
            }
        }

        while levels.len() > 1 {
            close_level(self.source, &mut levels);
        }
        let blocks = levels.pop().expect("root level exists").blocks;
        Document {
            attrs: Default::default(),
            blocks,
            range: 0..self.source.len(),
        }
    }

    fn append_plain_continuation(
        &mut self,
        levels: &mut [Level],
        line: &Line,
        previous_indent: usize,
        block: Block,
    ) -> Option<Block> {
        let Block::Parsed(mut continuation) = block else {
            return Some(block);
        };
        if continuation.mark.is_some() {
            return Some(Block::Parsed(continuation));
        }
        let level = levels.last_mut().expect("root level exists");
        let Some(Block::Parsed(owner)) = level.blocks.last_mut() else {
            return Some(Block::Parsed(continuation));
        };

        if previous_indent > level.indent && previous_indent != line.indent {
            self.error(
                "syntax.partial-indent",
                "continuation indentation must match its established column",
                line.start..line.start + line.indent,
            );
        }

        let boundary_start = owner.content.range.end;
        let boundary_end = line.start + line.indent;
        owner.content.items.push(Inline::SoftBreak {
            range: boundary_start..boundary_end,
        });
        owner.content.items.append(&mut continuation.content.items);
        owner.content = InlineContent::from_items(
            owner.content.range.start..continuation.content.range.end,
            std::mem::take(&mut owner.content.items),
        );
        owner.range.end = continuation.range.end;
        None
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
                close_level(self.source, levels);
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
        let mut frames = vec![InlineFrame::root(range.clone())];
        let mut cursor = range.start;
        while cursor < range.end {
            let byte = self.source.as_bytes()[cursor];
            match byte {
                b' ' => {
                    let start = cursor;
                    while cursor < range.end && self.source.as_bytes()[cursor] == b' ' {
                        cursor += 1;
                    }
                    frames.last_mut().unwrap().items.push(Inline::Space {
                        text: self.source[start..cursor].to_string(),
                        range: start..cursor,
                    });
                }
                b'{' => {
                    frames.push(InlineFrame::group(cursor, None));
                    cursor += 1;
                }
                b'}' if frames.len() > 1 => {
                    close_inline_frame(self.source, &mut frames, cursor + 1, cursor);
                    cursor += 1;
                }
                b'}' => {
                    self.error(
                        "syntax.unexpected-inline-group-close",
                        "closing brace has no enclosing inline group",
                        cursor..cursor + 1,
                    );
                    frames.last_mut().unwrap().items.push(Inline::Text {
                        text: "}".to_string(),
                        range: cursor..cursor + 1,
                    });
                    cursor += 1;
                }
                b'`' => {
                    let start = cursor;
                    while cursor < range.end && self.source.as_bytes()[cursor] == b'`' {
                        cursor += 1;
                    }
                    let count = cursor - start;
                    let pair_width = count / 2 * 2;
                    if pair_width > 0 {
                        frames.last_mut().unwrap().items.push(Inline::Text {
                            text: "`".repeat(pair_width / 2),
                            range: start..start + pair_width,
                        });
                    }
                    if count.is_multiple_of(2) {
                        continue;
                    }
                    let introducer = start + pair_width;
                    let after = introducer + 1;
                    if after >= range.end {
                        self.error(
                            "syntax.invalid-inline-dispatch",
                            "an introducer must start an escape, marked group, or verbatim value",
                            introducer..after,
                        );
                        cursor = after;
                        continue;
                    }
                    match self.source.as_bytes()[after] {
                        b'{' | b'}' => {
                            frames.last_mut().unwrap().items.push(Inline::Text {
                                text: self.source[after..after + 1].to_string(),
                                range: introducer..after + 1,
                            });
                            cursor = after + 1;
                        }
                        b'"' => {
                            let (verbatim, next) =
                                self.parse_inline_verbatim(introducer, after, range.end, None);
                            frames.last_mut().unwrap().items.push(verbatim);
                            cursor = next;
                        }
                        _ => {
                            let marker_end = scan_marker(self.source, after, range.end);
                            if marker_end == after {
                                self.error(
                                    "syntax.invalid-inline-dispatch",
                                    "invalid character after inline introducer",
                                    introducer..after + 1,
                                );
                                cursor = after;
                                continue;
                            }
                            let mark = Mark {
                                range: introducer..marker_end,
                                marker: self.source[after..marker_end].to_string(),
                                marker_range: after..marker_end,
                                attrs: Default::default(),
                            };
                            if marker_end < range.end {
                                match self.source.as_bytes()[marker_end] {
                                    b'{' => {
                                        frames.push(InlineFrame::group(marker_end, Some(mark)));
                                        cursor = marker_end + 1;
                                        continue;
                                    }
                                    b'"' => {
                                        let (verbatim, next) = self.parse_inline_verbatim(
                                            introducer,
                                            marker_end,
                                            range.end,
                                            Some(mark),
                                        );
                                        frames.last_mut().unwrap().items.push(verbatim);
                                        cursor = next;
                                        continue;
                                    }
                                    _ => {}
                                }
                            }
                            self.error(
                                "syntax.invalid-inline-dispatch",
                                "an inline marker must be followed immediately by a group or quote",
                                introducer..marker_end,
                            );
                            frames.last_mut().unwrap().items.push(Inline::Text {
                                text: self.source[after..marker_end].to_string(),
                                range: introducer..marker_end,
                            });
                            cursor = marker_end;
                        }
                    }
                }
                b'\t' | 0x00..=0x1f | 0x7f => {
                    let width = self.source[cursor..].chars().next().unwrap().len_utf8();
                    self.error(
                        "syntax.invalid-inline-dispatch",
                        "tabs and control characters are invalid in parsed inline content",
                        cursor..cursor + width,
                    );
                    frames.last_mut().unwrap().items.push(Inline::Text {
                        text: self.source[cursor..cursor + width].to_string(),
                        range: cursor..cursor + width,
                    });
                    cursor += width;
                }
                _ => {
                    let start = cursor;
                    cursor += self.source[cursor..].chars().next().unwrap().len_utf8();
                    while cursor < range.end {
                        let next = self.source.as_bytes()[cursor];
                        if matches!(next, b' ' | b'{' | b'}' | b'`' | b'\t' | 0x00..=0x1f | 0x7f) {
                            break;
                        }
                        cursor += self.source[cursor..].chars().next().unwrap().len_utf8();
                    }
                    frames.last_mut().unwrap().items.push(Inline::Text {
                        text: self.source[start..cursor].to_string(),
                        range: start..cursor,
                    });
                }
            }
        }
        while frames.len() > 1 {
            let open = frames.last().unwrap().open;
            self.error(
                "syntax.unclosed-inline-group",
                "inline group is not closed before the physical line ends",
                open..range.end,
            );
            close_inline_frame(self.source, &mut frames, range.end, range.end);
        }
        InlineContent::from_items(range, frames.pop().unwrap().items)
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

struct InlineFrame {
    open: usize,
    mark: Option<Mark>,
    items: Vec<Inline>,
    root_range: Option<SourceRange>,
}

impl InlineFrame {
    fn root(range: SourceRange) -> Self {
        Self {
            open: range.start,
            mark: None,
            items: Vec::new(),
            root_range: Some(range),
        }
    }

    fn group(open: usize, mark: Option<Mark>) -> Self {
        Self {
            open,
            mark,
            items: Vec::new(),
            root_range: None,
        }
    }
}

fn close_inline_frame(
    source: &str,
    frames: &mut Vec<InlineFrame>,
    range_end: usize,
    content_end: usize,
) {
    let frame = frames.pop().expect("group frame exists");
    debug_assert!(frame.root_range.is_none());
    let range_start = frame
        .mark
        .as_ref()
        .map_or(frame.open, |mark| mark.range.start);
    let content = InlineContent::from_items(frame.open + 1..content_end, frame.items);
    let mut mark = frame.mark;
    if let Some(mark) = &mut mark {
        mark.attrs = attributes_from_inlines(source, &content);
    }
    let group = Inline::Group {
        range: range_start..range_end,
        mark,
        content,
    };
    frames
        .last_mut()
        .expect("group has a parent frame")
        .items
        .push(group);
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

fn close_level(source: &str, levels: &mut Vec<Level>) {
    let level = levels.pop().expect("nested level exists");
    let mut owner = level.owner.expect("nested level has an owner");
    owner.children = level.blocks;
    let attrs = attributes_from_blocks(source, &owner.children);
    if let Some(mark) = &mut owner.mark {
        mark.attrs = attrs;
    }
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
    document.attrs = attributes_from_blocks(source, &document.blocks);
}

fn attributes_from_blocks(source: &str, blocks: &[Block]) -> Attributes {
    attributes_from_items(blocks.iter().filter_map(|block| {
        let Block::Parsed(block) = block else {
            return None;
        };
        if !block.children.is_empty() {
            None
        } else {
            let mut item = declaration_from_content(source, block.mark.as_ref()?, &block.content)?;
            *attr_range_mut(&mut item) = block.range.clone();
            Some(item)
        }
    }))
}

fn attributes_from_inlines(source: &str, content: &InlineContent) -> Attributes {
    attributes_from_items(content.items.iter().filter_map(|inline| {
        let Inline::Group {
            range,
            mark: Some(mark),
            content,
            ..
        } = inline
        else {
            return None;
        };
        let mut item = declaration_from_content(source, mark, content)?;
        *attr_range_mut(&mut item) = range.clone();
        Some(item)
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
    let elements = content
        .items
        .iter()
        .enumerate()
        .filter(|(_, inline)| !inline.is_whitespace() && !is_direct_declaration(inline))
        .collect::<Vec<_>>();
    match mark.marker.as_str() {
        "@" | "+" if elements.len() == 1 => {
            let value = content_from_element(elements[0].1);
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
        "=" if elements.len() >= 2 => {
            let key_content = content_from_element(elements[0].1);
            let key = plain_scalar(&key_content)?;
            if key.is_empty() {
                return None;
            }
            let value_content = content_from_elements(content, &elements[1..]);
            let value_range = value_content.range.clone();
            let value = value_content.plain_text();
            Some(AttrItem::Pair {
                key,
                key_range: key_content.range.clone(),
                value: AttrValue {
                    decoded: value,
                    raw: source[value_range.clone()].to_string(),
                    range: value_range.clone(),
                    quoted: true,
                },
                range: mark.range.start..value_range.end,
            })
        }
        _ => None,
    }
}

fn is_direct_declaration(inline: &Inline) -> bool {
    matches!(
        inline,
        Inline::Group {
            mark: Some(mark),
            ..
        } if matches!(mark.marker.as_str(), "@" | "+" | "=")
    )
}

fn content_from_element(inline: &Inline) -> InlineContent {
    InlineContent::from_items(crate::inline_range(inline).clone(), vec![inline.clone()])
}

fn content_from_elements(content: &InlineContent, elements: &[(usize, &Inline)]) -> InlineContent {
    let (first, last) = (elements.first().unwrap().0, elements.last().unwrap().0);
    let range = crate::inline_range(elements.first().unwrap().1).start
        ..crate::inline_range(elements.last().unwrap().1).end;
    let mut items = Vec::new();
    for inline in &content.items[first..=last] {
        if is_direct_declaration(inline)
            || (inline.is_whitespace() && items.last().is_some_and(Inline::is_whitespace))
        {
            continue;
        }
        items.push(inline.clone());
    }
    InlineContent::from_items(range, items)
}

fn plain_scalar(content: &InlineContent) -> Option<String> {
    let content = content.trim_boundary_padding();
    if let [Inline::Group {
        mark: None,
        content,
        ..
    }] = content.items.as_slice()
    {
        return plain_scalar(content);
    }
    content
        .items
        .iter()
        .all(|inline| {
            matches!(
                inline,
                Inline::Text { .. }
                    | Inline::Space { .. }
                    | Inline::SoftBreak { .. }
                    | Inline::Verbatim { mark: None, .. }
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

fn attr_range_mut(item: &mut AttrItem) -> &mut SourceRange {
    match item {
        AttrItem::Id { range, .. }
        | AttrItem::Class { range, .. }
        | AttrItem::Pair { range, .. } => range,
    }
}
