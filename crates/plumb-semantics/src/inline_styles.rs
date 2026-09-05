use std::ops::Range;

use plumb_syntax::{Block, Inline, InlineContent, ValidDocument};

use crate::{RelativeSemanticRecord, SemanticRecordView, SemanticRecords};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineStyleKind {
    Emphasis,
    Strong,
    Mark,
    Strikeout,
    Superscript,
    Subscript,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineStyleRecord {
    pub kind: InlineStyleKind,
    pub range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InlineStyleOutput {
    pub styles: SemanticRecords<InlineStyleRecord>,
}

pub type InlineStyleRecordView<'a> = SemanticRecordView<'a, InlineStyleRecord>;

impl InlineStyleRecordView<'_> {
    pub fn range(self) -> Range<usize> {
        self.record
            .range
            .start
            .checked_add_signed(self.offset)
            .unwrap()
            ..self
                .record
                .range
                .end
                .checked_add_signed(self.offset)
                .unwrap()
    }

    pub fn kind(self) -> InlineStyleKind {
        self.record.kind
    }
}

impl InlineStyleOutput {
    pub fn style_at_node_start(&self, start: usize) -> Option<InlineStyleRecordView<'_>> {
        self.styles.view_at_start(start)
    }
}

impl RelativeSemanticRecord for InlineStyleRecord {
    fn start(&self) -> usize {
        self.range.start
    }

    fn shift(&mut self, delta: isize) {
        self.range.start = self.range.start.checked_add_signed(delta).unwrap();
        self.range.end = self.range.end.checked_add_signed(delta).unwrap();
    }
}

pub fn analyze_inline_styles(valid: ValidDocument<'_>) -> InlineStyleOutput {
    let document = valid.syntax();
    let mut output = InlineStyleOutput::default();
    collect_blocks(&document.blocks, &mut output);
    output
}

fn collect_blocks(blocks: &[Block], output: &mut InlineStyleOutput) {
    for block in blocks {
        let Block::Parsed(block) = block else {
            continue;
        };
        collect_inlines(&block.content, output);
        for child in crate::body_children(block) {
            collect_blocks(std::slice::from_ref(child), output);
        }
    }
}

fn collect_inlines(content: &InlineContent, output: &mut InlineStyleOutput) {
    for inline in &content.items {
        let Inline::Group {
            range,
            mark,
            content,
            ..
        } = inline
        else {
            continue;
        };
        let kind = match mark.as_ref().map(|mark| mark.marker.as_str()) {
            Some("*") => Some(InlineStyleKind::Emphasis),
            Some("!") => Some(InlineStyleKind::Strong),
            Some("==") => Some(InlineStyleKind::Mark),
            Some("~") => Some(InlineStyleKind::Strikeout),
            Some("^") => Some(InlineStyleKind::Superscript),
            Some("_") => Some(InlineStyleKind::Subscript),
            _ => None,
        };
        if let Some(kind) = kind {
            output.styles.push(InlineStyleRecord {
                kind,
                range: range.clone(),
            });
        }
        collect_inlines(content, output);
    }
}

#[cfg(test)]
mod tests {
    use plumb_syntax::parse;

    use super::*;

    #[test]
    fn recognizes_single_symbol_inline_styles_only() {
        let source = "`*{{em `!{strong}}} `=={mark} `~{strike} `^{super} `_{sub} `**{generic}\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_inline_styles(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(
            output
                .styles
                .iter()
                .map(|style| style.kind)
                .collect::<Vec<_>>(),
            vec![
                InlineStyleKind::Emphasis,
                InlineStyleKind::Strong,
                InlineStyleKind::Mark,
                InlineStyleKind::Strikeout,
                InlineStyleKind::Superscript,
                InlineStyleKind::Subscript,
            ]
        );
        assert_eq!(
            &source[output.styles.get(0).unwrap().range.clone()],
            "`*{{em `!{strong}}}"
        );
        assert_eq!(
            &source[output.styles.get(1).unwrap().range.clone()],
            "`!{strong}"
        );
    }

    #[test]
    fn styles_accept_zero_or_more_visible_elements() {
        let source = "`!{one`*{two}} `!{{one`*{two}}} `!{} `!{visible `@{styled} text}\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_inline_styles(parsed.valid_syntax().unwrap());
        assert_eq!(
            output
                .styles
                .iter()
                .map(|style| (style.kind, &source[style.range.clone()]))
                .collect::<Vec<_>>(),
            vec![
                (InlineStyleKind::Strong, "`!{one`*{two}}"),
                (InlineStyleKind::Emphasis, "`*{two}"),
                (InlineStyleKind::Strong, "`!{{one`*{two}}}"),
                (InlineStyleKind::Emphasis, "`*{two}"),
                (InlineStyleKind::Strong, "`!{}"),
                (InlineStyleKind::Strong, "`!{visible `@{styled} text}"),
            ]
        );
    }
}
