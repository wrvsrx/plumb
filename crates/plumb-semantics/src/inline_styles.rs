use std::ops::Range;

use plumb_syntax::{Block, Inline, InlineContent, InlineMember, ValidDocument};

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
    pub styles: Vec<InlineStyleRecord>,
}

impl InlineStyleOutput {
    pub fn style_at_node_start(&self, start: usize) -> Option<&InlineStyleRecord> {
        self.styles.iter().find(|style| style.range.start == start)
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
        collect_inlines(&block.head, output);
        for child in crate::body_children(block) {
            collect_blocks(std::slice::from_ref(child), output);
        }
    }
}

fn collect_inlines(content: &InlineContent, output: &mut InlineStyleOutput) {
    for inline in &content.items {
        let Inline::Element {
            range,
            kind,
            members,
            ..
        } = inline
        else {
            continue;
        };
        let kind = match kind.as_str() {
            "*" => Some(InlineStyleKind::Emphasis),
            "!" => Some(InlineStyleKind::Strong),
            "==" => Some(InlineStyleKind::Mark),
            "~" => Some(InlineStyleKind::Strikeout),
            "^" => Some(InlineStyleKind::Superscript),
            "_" => Some(InlineStyleKind::Subscript),
            _ => None,
        };
        let argument_count = members
            .iter()
            .filter(|member| member.argument().is_some())
            .count();
        if let Some(kind) = kind.filter(|_| argument_count == 1) {
            output.styles.push(InlineStyleRecord {
                kind,
                range: range.clone(),
            });
        }
        for member in members {
            if let InlineMember::ParsedArgument(argument) = member {
                collect_inlines(&argument.content, output);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use plumb_syntax::parse;

    use super::*;

    #[test]
    fn recognizes_single_symbol_inline_styles_only() {
        let source = "`*[em `![strong]] `==[mark] `~[strike] `^[super] `_[sub] `**[generic]\n";
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
        assert_eq!(&source[output.styles[0].range.clone()], "`*[em `![strong]]");
        assert_eq!(&source[output.styles[1].range.clone()], "`![strong]");
    }
}
