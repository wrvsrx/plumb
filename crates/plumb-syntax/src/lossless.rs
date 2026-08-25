use crate::syntax::{
    AttachedContent, AttrItem, AttrValue, Attributes, Block, Diagnostic, Document, Inline,
    InlineContent, InlineMember, LosslessTree, SourceRange, SyntaxKind, SyntaxToken,
    VerbatimArgument, VerbatimBlock,
};

const BASELINE_PRIORITY: u8 = 10;
const TYPED_PRIORITY: u8 = 50;
const RAW_PRIORITY: u8 = 60;
const ERROR_PRIORITY: u8 = 100;

pub(crate) fn build_lossless(
    source: &str,
    document: &Document,
    diagnostics: &[Diagnostic],
) -> LosslessTree {
    if source.is_empty() {
        return LosslessTree {
            range: 0..0,
            tokens: Vec::new(),
        };
    }

    let mut builder = TokenBuilder::new(source);
    builder.annotate_lines();
    builder.annotate_document(document);
    for diagnostic in diagnostics {
        builder.assign(diagnostic.range.clone(), SyntaxKind::Error, ERROR_PRIORITY);
    }
    LosslessTree {
        range: 0..source.len(),
        tokens: builder.finish(),
    }
}

struct TokenBuilder<'a> {
    source: &'a str,
    kinds: Vec<Option<SyntaxKind>>,
    priorities: Vec<u8>,
    boundaries: Vec<bool>,
}

impl<'a> TokenBuilder<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            kinds: vec![None; source.len()],
            priorities: vec![0; source.len()],
            boundaries: vec![false; source.len() + 1],
        }
    }

    fn assign(&mut self, range: SourceRange, kind: SyntaxKind, priority: u8) {
        let start = range.start.min(self.source.len());
        let end = range.end.min(self.source.len());
        if start >= end
            || !self.source.is_char_boundary(start)
            || !self.source.is_char_boundary(end)
        {
            return;
        }
        self.boundaries[start] = true;
        self.boundaries[end] = true;
        for index in start..end {
            if priority >= self.priorities[index] {
                self.priorities[index] = priority;
                self.kinds[index] = Some(kind);
            }
        }
    }

    fn annotate_lines(&mut self) {
        let mut start = 0;
        for chunk in self.source.split_inclusive('\n') {
            let end = start + chunk.len();
            let mut content_end = if chunk.ends_with('\n') { end - 1 } else { end };
            if content_end > start && self.source.as_bytes()[content_end - 1] == b'\r' {
                content_end -= 1;
            }
            let indent_end = self.source[start..content_end]
                .bytes()
                .take_while(|byte| matches!(byte, b' ' | b'\t'))
                .count()
                + start;
            self.assign(
                start..indent_end,
                SyntaxKind::Indentation,
                BASELINE_PRIORITY,
            );
            self.assign(content_end..end, SyntaxKind::LineEnding, BASELINE_PRIORITY);
            start = end;
        }
    }

    fn annotate_document(&mut self, document: &Document) {
        let mut blocks = document.blocks.iter().rev().collect::<Vec<_>>();
        self.annotate_block_attributes(&document.attrs, &mut blocks);
        while let Some(block) = blocks.pop() {
            match block {
                Block::Parsed(block) => {
                    if let Some(mark) = &block.mark {
                        self.assign(
                            mark.range.start..mark.range.start + 1,
                            SyntaxKind::Introducer,
                            TYPED_PRIORITY,
                        );
                        self.assign(
                            mark.marker_range.clone(),
                            SyntaxKind::Marker,
                            TYPED_PRIORITY,
                        );
                        self.annotate_block_attributes(&mark.attrs, &mut blocks);
                    }
                    self.annotate_inlines(&block.head);
                    blocks.extend(block.children.iter().rev());
                }
                Block::Verbatim(block) => {
                    self.assign(
                        block.opener_range.start..block.opener_range.start + 1,
                        SyntaxKind::Introducer,
                        TYPED_PRIORITY,
                    );
                    self.assign(
                        block.kind_range.clone(),
                        SyntaxKind::InlineKind,
                        TYPED_PRIORITY,
                    );
                    if block.quote_count > 0 {
                        self.assign(
                            block.kind_range.end..block.kind_range.end + block.quote_count,
                            SyntaxKind::Delimiter,
                            TYPED_PRIORITY,
                        );
                    }
                    self.annotate_block_attributes(&block.attrs, &mut blocks);
                    self.annotate_raw_block(block);
                }
            }
        }
    }

    fn annotate_inlines(&mut self, content: &InlineContent) {
        let mut contents = vec![content];
        let mut pending = Vec::new();
        while !contents.is_empty() || !pending.is_empty() {
            if let Some(content) = contents.pop() {
                pending.extend(content.items.iter().rev());
            }
            while let Some(inline) = pending.pop() {
                match inline {
                    Inline::Text { text, range } => {
                        let kind = if matches!(
                            self.source.get(range.clone()),
                            Some("``" | "`]" | "`{" | "`}")
                        ) && matches!(text.as_str(), "`" | "]" | "{" | "}")
                        {
                            SyntaxKind::Escape
                        } else {
                            SyntaxKind::Text
                        };
                        self.assign(range.clone(), kind, TYPED_PRIORITY);
                    }
                    Inline::Space { range, .. } => {
                        self.assign(range.clone(), SyntaxKind::Whitespace, TYPED_PRIORITY);
                    }
                    Inline::SoftBreak { .. } => {}
                    Inline::Element {
                        range,
                        kind_range,
                        members,
                        attrs,
                        ..
                    } => {
                        self.assign(
                            range.start..kind_range.start,
                            SyntaxKind::Introducer,
                            TYPED_PRIORITY,
                        );
                        self.assign(kind_range.clone(), SyntaxKind::InlineKind, TYPED_PRIORITY);
                        self.assign(
                            kind_range.end..kind_range.end + 1,
                            SyntaxKind::Delimiter,
                            TYPED_PRIORITY,
                        );
                        self.assign(
                            range.end - 1..range.end,
                            SyntaxKind::Delimiter,
                            TYPED_PRIORITY,
                        );
                        for member in members.iter().rev() {
                            match member {
                                InlineMember::ParsedArgument(argument) => {
                                    if let Some(separator) = &argument.separator_range {
                                        self.assign(
                                            separator.clone(),
                                            SyntaxKind::Delimiter,
                                            TYPED_PRIORITY,
                                        );
                                    }
                                    contents.push(&argument.content);
                                }
                                InlineMember::VerbatimArgument(argument) => {
                                    if let Some(separator) = &argument.separator_range {
                                        self.assign(
                                            separator.clone(),
                                            SyntaxKind::Delimiter,
                                            TYPED_PRIORITY,
                                        );
                                    }
                                    self.annotate_verbatim_member(argument);
                                }
                                InlineMember::Child {
                                    separator_range,
                                    inline,
                                    ..
                                } => {
                                    self.assign(
                                        separator_range.clone(),
                                        SyntaxKind::Delimiter,
                                        TYPED_PRIORITY,
                                    );
                                    pending.push(inline);
                                }
                            }
                        }
                        self.annotate_inline_attributes(attrs, &mut contents);
                    }
                    Inline::Verbatim {
                        range,
                        kind_range,
                        text_range,
                        quote_count,
                        bracketed,
                        attrs,
                        ..
                    } => {
                        self.assign(
                            range.start..range.start + 1,
                            SyntaxKind::Introducer,
                            TYPED_PRIORITY,
                        );
                        self.assign(kind_range.clone(), SyntaxKind::InlineKind, TYPED_PRIORITY);
                        let delimiter_start = kind_range.end;
                        if *bracketed {
                            self.assign(
                                delimiter_start..delimiter_start + quote_count,
                                SyntaxKind::Delimiter,
                                TYPED_PRIORITY,
                            );
                            self.assign(
                                delimiter_start + quote_count..delimiter_start + quote_count + 1,
                                SyntaxKind::Delimiter,
                                TYPED_PRIORITY,
                            );
                        } else {
                            self.assign(
                                delimiter_start..delimiter_start + 1,
                                SyntaxKind::Delimiter,
                                TYPED_PRIORITY,
                            );
                        }
                        self.assign(text_range.clone(), SyntaxKind::RawPayload, TYPED_PRIORITY);
                        let close_width = if *bracketed { 1 + quote_count } else { 1 };
                        self.assign(
                            text_range.end..text_range.end + close_width,
                            SyntaxKind::Delimiter,
                            TYPED_PRIORITY,
                        );
                        self.annotate_inline_attributes(attrs, &mut contents);
                    }
                }
            }
        }
    }

    fn annotate_verbatim_member(&mut self, argument: &VerbatimArgument) {
        let delimiter_start = argument.range.start;
        if argument.bracketed {
            self.assign(
                delimiter_start..delimiter_start + argument.quote_count,
                SyntaxKind::Delimiter,
                TYPED_PRIORITY,
            );
            self.assign(
                delimiter_start + argument.quote_count..delimiter_start + argument.quote_count + 1,
                SyntaxKind::Delimiter,
                TYPED_PRIORITY,
            );
        } else {
            self.assign(
                delimiter_start..delimiter_start + 1,
                SyntaxKind::Delimiter,
                TYPED_PRIORITY,
            );
        }
        self.assign(
            argument.text_range.clone(),
            SyntaxKind::RawPayload,
            TYPED_PRIORITY,
        );
        let close_width = if argument.bracketed {
            1 + argument.quote_count
        } else {
            1
        };
        self.assign(
            argument.range.end - close_width..argument.range.end,
            SyntaxKind::Delimiter,
            TYPED_PRIORITY,
        );
    }

    fn annotate_attributes(&mut self, attrs: &Attributes) {
        let Some(range) = &attrs.range else {
            return;
        };
        if self.source.as_bytes().get(range.start) == Some(&b'{') {
            self.assign(
                range.start..range.start + 1,
                SyntaxKind::Delimiter,
                TYPED_PRIORITY,
            );
        }
        if range.end > range.start && self.source.as_bytes().get(range.end - 1) == Some(&b'}') {
            self.assign(
                range.end - 1..range.end,
                SyntaxKind::Delimiter,
                TYPED_PRIORITY,
            );
        }
        for item in &attrs.items {
            match item {
                AttrItem::Id { range, .. } | AttrItem::Class { range, .. } => {
                    self.assign(
                        range.start..range.start + 1,
                        SyntaxKind::AttributePunctuation,
                        TYPED_PRIORITY,
                    );
                    self.assign(
                        range.start + 1..range.end,
                        SyntaxKind::AttributeName,
                        TYPED_PRIORITY,
                    );
                }
                AttrItem::Pair {
                    key_range, value, ..
                } => {
                    self.assign(key_range.clone(), SyntaxKind::AttributeName, TYPED_PRIORITY);
                    self.assign(
                        key_range.end..key_range.end + 1,
                        SyntaxKind::AttributePunctuation,
                        TYPED_PRIORITY,
                    );
                    self.annotate_attribute_value(value);
                }
            }
        }
    }

    fn annotate_block_attributes<'b>(
        &mut self,
        attrs: &'b Attributes,
        blocks: &mut Vec<&'b Block>,
    ) {
        let Some(attached) = attrs.attached.as_deref() else {
            self.annotate_attributes(attrs);
            return;
        };
        self.annotate_attached_delimiters(attached);
        if let AttachedContent::Blocks(content) = &attached.content {
            blocks.extend(content.iter().rev());
        }
    }

    fn annotate_inline_attributes<'b>(
        &mut self,
        attrs: &'b Attributes,
        contents: &mut Vec<&'b InlineContent>,
    ) {
        let Some(attached) = attrs.attached.as_deref() else {
            self.annotate_attributes(attrs);
            return;
        };
        self.annotate_attached_delimiters(attached);
        if let AttachedContent::Inlines(content) = &attached.content {
            contents.push(content);
        }
    }

    fn annotate_attached_delimiters(&mut self, attached: &crate::syntax::AttachedGroup) {
        self.assign(
            attached.open_range.clone(),
            SyntaxKind::Delimiter,
            TYPED_PRIORITY,
        );
        self.assign(
            attached.close_range.clone(),
            SyntaxKind::Delimiter,
            TYPED_PRIORITY,
        );
    }

    fn annotate_attribute_value(&mut self, value: &AttrValue) {
        if !value.quoted {
            self.assign(
                value.range.clone(),
                SyntaxKind::AttributeValue,
                TYPED_PRIORITY,
            );
            return;
        }
        self.assign(
            value.range.start..value.range.start + 1,
            SyntaxKind::Delimiter,
            TYPED_PRIORITY,
        );
        self.assign(
            value.range.end - 1..value.range.end,
            SyntaxKind::Delimiter,
            TYPED_PRIORITY,
        );
        let mut cursor = value.range.start + 1;
        let end = value.range.end - 1;
        while cursor < end {
            if self.source.as_bytes()[cursor] == b'\\' && cursor + 1 < end {
                self.assign(
                    cursor..cursor + 2,
                    SyntaxKind::AttributeEscape,
                    TYPED_PRIORITY,
                );
                cursor += 2;
            } else {
                let next = cursor + self.source[cursor..].chars().next().unwrap().len_utf8();
                self.assign(cursor..next, SyntaxKind::AttributeValue, TYPED_PRIORITY);
                cursor = next;
            }
        }
    }

    fn annotate_raw_block(&mut self, block: &VerbatimBlock) {
        let line_start = self.source[..block.range.start]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let body_indent = block.range.start - line_start + 2;
        let mut start = block.text_range.start;
        while start < block.text_range.end {
            let newline = self.source[start..block.text_range.end]
                .find('\n')
                .map(|offset| start + offset);
            let end = newline.map_or(block.text_range.end, |index| index + 1);
            let mut content_end = newline.unwrap_or(end);
            if content_end > start && self.source.as_bytes()[content_end - 1] == b'\r' {
                content_end -= 1;
            }
            let prefix_end = (start + body_indent).min(content_end);
            self.assign(start..prefix_end, SyntaxKind::Indentation, RAW_PRIORITY);
            self.assign(
                prefix_end..content_end,
                SyntaxKind::RawPayload,
                RAW_PRIORITY,
            );
            self.assign(content_end..end, SyntaxKind::LineEnding, RAW_PRIORITY);
            start = end;
        }
    }

    fn finish(self) -> Vec<SyntaxToken> {
        let mut tokens = Vec::new();
        let mut cursor = 0;
        while cursor < self.source.len() {
            let kind = self.kinds[cursor].unwrap_or_else(|| fallback_kind(self.source, cursor));
            let mut end = cursor + self.source[cursor..].chars().next().unwrap().len_utf8();
            while end < self.source.len()
                && !self.boundaries[end]
                && self.kinds[end].unwrap_or_else(|| fallback_kind(self.source, end)) == kind
                && !atomic_kind(kind)
            {
                end += self.source[end..].chars().next().unwrap().len_utf8();
            }
            tokens.push(SyntaxToken {
                kind,
                range: cursor..end,
            });
            cursor = end;
        }
        tokens
    }
}

fn fallback_kind(source: &str, cursor: usize) -> SyntaxKind {
    match source[cursor..].chars().next().unwrap() {
        '`' => SyntaxKind::Introducer,
        '[' | ']' | '{' | '}' | '"' => SyntaxKind::Delimiter,
        '#' | '.' | '=' => SyntaxKind::AttributePunctuation,
        character if character.is_whitespace() => SyntaxKind::Whitespace,
        _ => SyntaxKind::Text,
    }
}

fn atomic_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::Introducer
            | SyntaxKind::Delimiter
            | SyntaxKind::AttributePunctuation
            | SyntaxKind::Escape
    )
}
