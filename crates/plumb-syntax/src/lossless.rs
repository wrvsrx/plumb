use crate::syntax::{
    Block, Diagnostic, Document, Inline, InlineContent, LosslessTree, SourceRange, SyntaxKind,
    SyntaxToken, VerbatimBlock,
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
        while let Some(block) = blocks.pop() {
            match block {
                Block::Parsed(block) => {
                    if let Some(mark) = &block.mark {
                        self.assign(
                            mark.range.start..mark.marker_range.start,
                            SyntaxKind::Introducer,
                            TYPED_PRIORITY,
                        );
                        self.assign(
                            mark.marker_range.clone(),
                            SyntaxKind::Marker,
                            TYPED_PRIORITY,
                        );
                    }
                    self.annotate_inlines(&block.content);
                    blocks.extend(block.children.iter().rev());
                }
                Block::Verbatim(block) => self.annotate_raw_block(block),
            }
        }
    }

    fn annotate_inlines(&mut self, content: &InlineContent) {
        let mut contents = vec![content];
        while let Some(content) = contents.pop() {
            for inline in content.items.iter().rev() {
                match inline {
                    Inline::Text { range, .. } => {
                        let kind = self
                            .source
                            .get(range.clone())
                            .is_some_and(|text| text.starts_with('`'))
                            .then_some(SyntaxKind::Escape)
                            .unwrap_or(SyntaxKind::Text);
                        self.assign(range.clone(), kind, TYPED_PRIORITY);
                    }
                    Inline::Space { range, .. } => {
                        self.assign(range.clone(), SyntaxKind::Whitespace, TYPED_PRIORITY);
                    }
                    Inline::Group {
                        range,
                        mark,
                        content,
                    } => {
                        let open = mark.as_ref().map_or(range.start, |mark| {
                            self.assign(
                                mark.range.start..mark.marker_range.start,
                                SyntaxKind::Introducer,
                                TYPED_PRIORITY,
                            );
                            self.assign(
                                mark.marker_range.clone(),
                                SyntaxKind::InlineKind,
                                TYPED_PRIORITY,
                            );
                            mark.range.end
                        });
                        self.assign(open..open + 1, SyntaxKind::Delimiter, TYPED_PRIORITY);
                        if range.end > content.range.end {
                            self.assign(
                                range.end - 1..range.end,
                                SyntaxKind::Delimiter,
                                TYPED_PRIORITY,
                            );
                        }
                        contents.push(content);
                    }
                    Inline::Verbatim {
                        range,
                        mark,
                        text_range,
                        quote_count,
                        braced,
                        ..
                    } => {
                        let quote_start = mark.as_ref().map_or(range.start + 1, |mark| {
                            self.assign(
                                mark.range.start..mark.marker_range.start,
                                SyntaxKind::Introducer,
                                TYPED_PRIORITY,
                            );
                            self.assign(
                                mark.marker_range.clone(),
                                SyntaxKind::InlineKind,
                                TYPED_PRIORITY,
                            );
                            mark.range.end
                        });
                        if mark.is_none() {
                            self.assign(
                                range.start..range.start + 1,
                                SyntaxKind::Introducer,
                                TYPED_PRIORITY,
                            );
                        }
                        let opening_end = quote_start + quote_count + usize::from(*braced);
                        self.assign(
                            quote_start..opening_end,
                            SyntaxKind::Delimiter,
                            TYPED_PRIORITY,
                        );
                        self.assign(text_range.clone(), SyntaxKind::RawPayload, RAW_PRIORITY);
                        self.assign(
                            text_range.end..range.end,
                            SyntaxKind::Delimiter,
                            TYPED_PRIORITY,
                        );
                    }
                }
            }
        }
    }

    fn annotate_raw_block(&mut self, block: &VerbatimBlock) {
        let introducer = block.opener_range.start;
        self.assign(
            introducer..introducer + 1,
            SyntaxKind::Introducer,
            TYPED_PRIORITY,
        );
        if let Some(mark) = &block.mark {
            self.assign(
                mark.marker_range.clone(),
                SyntaxKind::Marker,
                TYPED_PRIORITY,
            );
        }
        self.assign(
            block.quote_range.clone(),
            SyntaxKind::Delimiter,
            TYPED_PRIORITY,
        );

        let line_start = self.source[..introducer]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let margin = introducer - line_start + 1;
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
            let prefix_end = (start + margin).min(content_end);
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
        '{' | '}' => SyntaxKind::Delimiter,
        character if character.is_whitespace() => SyntaxKind::Whitespace,
        _ => SyntaxKind::Text,
    }
}

fn atomic_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::Introducer | SyntaxKind::Delimiter | SyntaxKind::Escape
    )
}
