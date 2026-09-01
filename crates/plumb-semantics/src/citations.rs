use std::ops::Range;

use plumb_syntax::{Block, Diagnostic, DiagnosticSeverity, Inline, InlineContent, ValidDocument};

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

pub fn analyze_citations(valid: ValidDocument<'_>) -> CitationOutput {
    let document = valid.syntax();
    let mut output = CitationOutput::default();
    collect_blocks(&document.blocks, &mut output);
    output
}

fn collect_blocks(blocks: &[Block], output: &mut CitationOutput) {
    for block in blocks {
        if let Block::Parsed(block) = block {
            collect_inlines(&block.content, output);
            for child in crate::body_children(block) {
                collect_blocks(std::slice::from_ref(child), output);
            }
        }
    }
}

fn collect_inlines(content: &InlineContent, output: &mut CitationOutput) {
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
        if mark.as_ref().is_some_and(|mark| mark.marker == "cite") {
            match content.data.as_slice() {
                [_] => {
                    let argument = content.datum(0).expect("projected datum exists");
                    match citation_id(&argument) {
                        Some(id) => output.citations.push(CitationRecord {
                            range: range.clone(),
                            selection_range: argument.range.clone(),
                            id,
                        }),
                        None => output.diagnostics.push(Diagnostic {
                            code: "citation.invalid",
                            severity: DiagnosticSeverity::Warning,
                            message: "a citation must contain one plain id".to_string(),
                            range: argument.range.clone(),
                            related: Vec::new(),
                        }),
                    }
                }
                _ => output.diagnostics.push(Diagnostic {
                    code: "citation.invalid",
                    severity: DiagnosticSeverity::Warning,
                    message: "a citation must contain exactly one argument".to_string(),
                    range: range.clone(),
                    related: Vec::new(),
                }),
            }
        }
        collect_inlines(content, output);
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
        let parsed = parse("See `cite{smith2004}.\n`meta\n `: source\n  `cite{roe-2020}\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_citations(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert!(output.diagnostics.is_empty());
        assert_eq!(output.citations.len(), 2);
        assert_eq!(output.citations[0].id, "smith2004");
        assert_eq!(output.citations[1].id, "roe-2020");
    }

    #[test]
    fn diagnoses_invalid_citation_content() {
        let parsed = parse("`cite{{plain text}} `cite{@one} `cite{one;two} `cite{`*{nested}}.\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_citations(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert!(output.citations.is_empty());
        assert_eq!(output.diagnostics.len(), 4);
        assert!(output
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == "citation.invalid"));
    }
}
