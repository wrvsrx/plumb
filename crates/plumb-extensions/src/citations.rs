use std::ops::Range;

use plumb_core::{Block, Diagnostic, DiagnosticSeverity, Document, Inline, InlineContent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CitationMode {
    Normal,
    SuppressAuthor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitationItem {
    pub id: String,
    pub prefix: String,
    pub suffix: String,
    pub mode: CitationMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitationRecord {
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
    pub text: String,
    pub items: Vec<CitationItem>,
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
            collect_blocks(&block.children, output);
        }
    }
}

fn collect_inlines(content: &InlineContent, output: &mut CitationOutput) {
    for inline in &content.items {
        let Inline::Element {
            range,
            kind,
            content,
            ..
        } = inline
        else {
            continue;
        };
        if kind == "cite" {
            let text = content.plain_text();
            match parse_citation_items(&text) {
                Some(items) => output.citations.push(CitationRecord {
                    range: range.clone(),
                    selection_range: content.range.clone(),
                    text,
                    items,
                }),
                None => output.diagnostics.push(Diagnostic {
                    code: "citation.invalid",
                    severity: DiagnosticSeverity::Warning,
                    message: "citations must contain semicolon-separated @id items".to_string(),
                    range: content.range.clone(),
                    related: Vec::new(),
                }),
            }
        }
        collect_inlines(content, output);
    }
}

fn parse_citation_items(text: &str) -> Option<Vec<CitationItem>> {
    let mut items = Vec::new();
    for segment in text.split(';') {
        let segment = segment.trim();
        let at = segment.find('@')?;
        let suppress_author = at > 0 && segment.as_bytes()[at - 1] == b'-';
        let prefix_end = at - usize::from(suppress_author);
        let prefix = segment[..prefix_end].trim();
        let after_at = &segment[at + 1..];
        let id_end = after_at
            .find(|character: char| character.is_whitespace() || character == ',')
            .unwrap_or(after_at.len());
        let id = &after_at[..id_end];
        let suffix = after_at[id_end..].trim();
        if id.is_empty()
            || id.chars().any(|character| {
                character.is_control() || matches!(character, '@' | '[' | ']' | '{' | '}')
            })
            || prefix.contains('@')
            || suffix.contains('@')
        {
            return None;
        }
        items.push(CitationItem {
            id: id.to_string(),
            prefix: prefix.to_string(),
            suffix: suffix.to_string(),
            mode: if suppress_author {
                CitationMode::SuppressAuthor
            } else {
                CitationMode::Normal
            },
        });
    }
    (!items.is_empty()).then_some(items)
}

#[cfg(test)]
mod tests {
    use plumb_core::parse;

    use super::*;

    #[test]
    fn parses_citation_items_and_modes() {
        let parsed = parse(
            "See `cite[see @smith2004, pp. 33-35; -@doe2010].\n\n`meta\n  `: source\n\n     `cite[@roe2020]\n",
        );
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_citations(&parsed.syntax);
        assert!(output.diagnostics.is_empty());
        assert_eq!(output.citations.len(), 2);
        assert_eq!(
            output.citations[0].items,
            vec![
                CitationItem {
                    id: "smith2004".to_string(),
                    prefix: "see".to_string(),
                    suffix: ", pp. 33-35".to_string(),
                    mode: CitationMode::Normal,
                },
                CitationItem {
                    id: "doe2010".to_string(),
                    prefix: String::new(),
                    suffix: String::new(),
                    mode: CitationMode::SuppressAuthor,
                },
            ]
        );
        assert_eq!(output.citations[1].items[0].id, "roe2020");
    }

    #[test]
    fn diagnoses_invalid_citation_content() {
        let parsed = parse("`cite[plain text] and `cite[@one @two].\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_citations(&parsed.syntax);
        assert!(output.citations.is_empty());
        assert_eq!(output.diagnostics.len(), 2);
        assert!(output
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == "citation.invalid"));
    }
}
