use std::ops::Range;

use plumb_syntax::{
    Block, Diagnostic, DiagnosticSeverity, Document, Inline, InlineArgumentRef, InlineContent,
    InlineMember,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitationRecord {
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CitationOutput {
    pub citations: Vec<CitationRecord>,
    pub diagnostics: Vec<Diagnostic>,
}

impl CitationOutput {
    pub fn citation_at_node_start(&self, start: usize) -> Option<&CitationRecord> {
        self.citations
            .iter()
            .find(|citation| citation.range.start == start)
    }
}

pub fn analyze_citations(document: &Document) -> CitationOutput {
    let mut output = CitationOutput::default();
    collect_blocks(&document.blocks, &mut output);
    output
}

fn collect_blocks(blocks: &[Block], output: &mut CitationOutput) {
    for block in blocks {
        if let Block::Parsed(block) = block {
            collect_inlines(&block.head, output);
            for child in crate::body_children(block) {
                collect_blocks(std::slice::from_ref(child), output);
            }
        }
    }
}

fn collect_inlines(content: &InlineContent, output: &mut CitationOutput) {
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
        if kind == "cite" {
            let arguments = members
                .iter()
                .filter_map(InlineMember::argument)
                .collect::<Vec<_>>();
            match arguments.as_slice() {
                [argument] => match citation_argument_id(argument) {
                    Some(id) => output.citations.push(CitationRecord {
                        range: range.clone(),
                        selection_range: argument_range(argument),
                        id,
                    }),
                    None => output.diagnostics.push(Diagnostic {
                        code: "citation.invalid",
                        severity: DiagnosticSeverity::Warning,
                        message: "a citation must contain one plain id".to_string(),
                        range: argument_range(argument),
                        related: Vec::new(),
                    }),
                },
                _ => output.diagnostics.push(Diagnostic {
                    code: "citation.invalid",
                    severity: DiagnosticSeverity::Warning,
                    message: "a citation must contain exactly one argument".to_string(),
                    range: range.clone(),
                    related: Vec::new(),
                }),
            }
        }
        for member in members {
            if let InlineMember::ParsedArgument(argument) = member {
                collect_inlines(&argument.content, output);
            }
        }
    }
}

fn citation_argument_id(argument: &InlineArgumentRef<'_>) -> Option<String> {
    match argument {
        InlineArgumentRef::Parsed(content) => citation_id(content),
        InlineArgumentRef::Verbatim(argument) => {
            (!argument.text.is_empty()).then(|| argument.text.clone())
        }
    }
}

fn argument_range(argument: &InlineArgumentRef<'_>) -> Range<usize> {
    match argument {
        InlineArgumentRef::Parsed(content) => content.range.clone(),
        InlineArgumentRef::Verbatim(argument) => argument.text_range.clone(),
    }
}

fn citation_id(content: &InlineContent) -> Option<String> {
    if content.items.is_empty()
        || !content
            .items
            .iter()
            .all(|inline| matches!(inline, Inline::Text { .. }))
    {
        return None;
    }
    let id = content.plain_text();
    id.chars()
        .all(|character| {
            !character.is_whitespace()
                && !character.is_control()
                && !matches!(character, '`' | '@' | ';' | ',' | '[' | ']' | '{' | '}')
        })
        .then_some(id)
}

#[cfg(test)]
mod tests {
    use plumb_syntax::parse;

    use super::*;

    #[test]
    fn collects_single_plain_id_citations() {
        let parsed = parse("See `cite[smith2004].\n\n`meta\n  `: source\n\n     `cite[roe-2020]\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_citations(&parsed.syntax);
        assert!(output.diagnostics.is_empty());
        assert_eq!(output.citations.len(), 2);
        assert_eq!(output.citations[0].id, "smith2004");
        assert_eq!(output.citations[1].id, "roe-2020");
    }

    #[test]
    fn diagnoses_invalid_citation_content() {
        let parsed = parse("`cite[plain text] `cite[@one] `cite[one;two] `cite[`*[nested]].\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_citations(&parsed.syntax);
        assert!(output.citations.is_empty());
        assert_eq!(output.diagnostics.len(), 4);
        assert!(output
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == "citation.invalid"));
    }
}
