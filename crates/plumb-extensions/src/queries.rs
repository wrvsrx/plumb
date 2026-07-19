use std::ops::Range;

use plumb_core::{Block, Inline, InlineContent, ParsedDocument};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkCompletionContext {
    Label {
        replace: Range<usize>,
        query: String,
    },
    Path {
        replace: Range<usize>,
        query: String,
    },
    Anchor {
        path: String,
        replace: Range<usize>,
        query: String,
    },
}

pub fn link_completion_context(
    document: &ParsedDocument,
    offset: usize,
) -> Option<LinkCompletionContext> {
    let source = &document.source;
    if offset > source.len() || !source.is_char_boundary(offset) {
        return None;
    }
    if verbatim_at(document, offset) {
        return None;
    }
    let line_start = source[..offset].rfind('\n').map_or(0, |index| index + 1);
    let prefix = &source[line_start..offset];
    let link_start = prefix.rfind("`link[")? + line_start;
    let escaped_introducers = source[..link_start]
        .chars()
        .rev()
        .take_while(|character| *character == '`')
        .count();
    if escaped_introducers % 2 == 1 {
        return None;
    }
    let label_start = link_start + "`link[".len();
    let label_prefix = &source[label_start..offset];
    let Some(label_end) = label_prefix.rfind("]{") else {
        if label_prefix
            .chars()
            .any(|character| character == '`' || character == ']' || character.is_control())
        {
            return None;
        }
        let line_end = source[offset..]
            .find('\n')
            .map_or(source.len(), |index| offset + index);
        let suffix = &source[offset..line_end];
        let replace_end = if suffix.starts_with(']') && !suffix.starts_with("]{") {
            offset + 1
        } else if suffix.contains(']') {
            return None;
        } else {
            offset
        };
        return Some(LinkCompletionContext::Label {
            replace: link_start..replace_end,
            query: label_prefix.to_string(),
        });
    };
    let after_label = label_start + label_end + 2;
    let attrs = &source[after_label..offset];
    let to = attrs.rfind("to=")? + after_label;
    if to > after_label {
        let previous = source[..to].chars().next_back()?;
        if !previous.is_whitespace() && previous != '{' {
            return None;
        }
    }
    let raw_value_start = to + 3;
    let quoted = source.as_bytes().get(raw_value_start) == Some(&b'"');
    let value_start = raw_value_start + usize::from(quoted);
    if offset < value_start {
        return None;
    }
    let query = &source[value_start..offset];
    if query.contains('"') || query.contains('}') || query.chars().any(char::is_control) {
        return None;
    }
    let value_end = if quoted {
        closing_quote(source, offset).unwrap_or(offset)
    } else {
        source[offset..]
            .find(|character: char| character.is_whitespace() || character == '}')
            .map_or(source.len(), |end| offset + end)
    };
    if let Some((path, fragment)) = query.split_once('#') {
        let fragment_start = value_start + path.len() + 1;
        Some(LinkCompletionContext::Anchor {
            path: path.to_string(),
            replace: fragment_start..value_end,
            query: fragment.to_string(),
        })
    } else {
        let path_end = source[offset..value_end]
            .find('#')
            .map_or(value_end, |separator| offset + separator);
        Some(LinkCompletionContext::Path {
            replace: value_start..path_end,
            query: query.to_string(),
        })
    }
}

fn verbatim_at(document: &ParsedDocument, offset: usize) -> bool {
    if document.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "syntax.unclosed-verbatim"
            && diagnostic.range.start <= offset
            && offset <= diagnostic.range.end
    }) {
        return true;
    }
    blocks_contain_verbatim(&document.syntax.blocks, offset)
}

fn blocks_contain_verbatim(blocks: &[Block], offset: usize) -> bool {
    blocks.iter().any(|block| match block {
        Block::Verbatim(block) => {
            block.text_range.start <= offset && offset <= block.text_range.end
        }
        Block::Parsed(block) => {
            inlines_contain_verbatim(&block.head, offset)
                || blocks_contain_verbatim(&block.children, offset)
        }
    })
}

fn inlines_contain_verbatim(content: &InlineContent, offset: usize) -> bool {
    content.items.iter().any(|inline| match inline {
        Inline::Verbatim { text_range, .. } => {
            text_range.start <= offset && offset <= text_range.end
        }
        Inline::Element { content, .. } => inlines_contain_verbatim(content, offset),
        Inline::Text { .. } | Inline::SoftBreak { .. } => false,
    })
}

fn closing_quote(source: &str, start: usize) -> Option<usize> {
    let mut escaped = source[..start]
        .chars()
        .rev()
        .take_while(|character| *character == '\\')
        .count()
        % 2
        == 1;
    for (relative, character) in source[start..].char_indices() {
        if character == '"' && !escaped {
            return Some(start + relative);
        }
        if character == '\\' {
            escaped = !escaped;
        } else {
            escaped = false;
        }
        if character == '\n' || character == '\r' {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use plumb_core::parse;

    use super::*;

    #[test]
    fn finds_incomplete_path_and_anchor_contexts() {
        let label = "See `link[Usage";
        assert_eq!(
            completion_context(label, label.len()),
            Some(LinkCompletionContext::Label {
                replace: 4..15,
                query: "Usage".to_string(),
            })
        );
        let closed_label = "See `link[Usage]";
        assert_eq!(
            completion_context(closed_label, closed_label.len() - 1),
            Some(LinkCompletionContext::Label {
                replace: 4..16,
                query: "Usage".to_string(),
            })
        );
        let escaped = "See ``link[Usage";
        assert_eq!(completion_context(escaped, escaped.len()), None);
        let strengthened = "See ```link[Usage";
        assert!(matches!(
            completion_context(strengthened, strengthened.len()),
            Some(LinkCompletionContext::Label { .. })
        ));
        let path = "See `link[x]{to=\"doc";
        assert_eq!(
            completion_context(path, path.len()),
            Some(LinkCompletionContext::Path {
                replace: 17..20,
                query: "doc".to_string(),
            })
        );
        let anchor = "See `link[x]{to=\"doc.plumb#tar";
        assert_eq!(
            completion_context(anchor, anchor.len()),
            Some(LinkCompletionContext::Anchor {
                path: "doc.plumb".to_string(),
                replace: 27..30,
                query: "tar".to_string(),
            })
        );
    }

    #[test]
    fn replaces_complete_target_components_around_the_cursor() {
        let (path, cursor) = strip_cursor("See `link[x]{to=\"do|c.plumb#target\"}");
        let value_start = path.find("doc.plumb").unwrap();
        let separator = path.find("#target").unwrap();
        assert_eq!(
            completion_context(&path, cursor),
            Some(LinkCompletionContext::Path {
                replace: value_start..separator,
                query: "do".to_string(),
            })
        );

        let (anchor, cursor) = strip_cursor("See `link[x]{to=\"doc.plumb#ta|rget\"}");
        let fragment_start = anchor.find("target").unwrap();
        assert_eq!(
            completion_context(&anchor, cursor),
            Some(LinkCompletionContext::Anchor {
                path: "doc.plumb".to_string(),
                replace: fragment_start..fragment_start + "target".len(),
                query: "ta".to_string(),
            })
        );

        let (empty, cursor) = strip_cursor("See `link[x]{to=\"|\"}");
        assert_eq!(
            completion_context(&empty, cursor),
            Some(LinkCompletionContext::Path {
                replace: cursor..cursor,
                query: String::new(),
            })
        );
    }

    #[test]
    fn ignores_link_like_text_inside_verbatim_payloads() {
        let closed = "`\"[raw `link[x]{to=\"doc|\"}]\"";
        let (closed, cursor) = strip_cursor(closed);
        assert_eq!(completion_context(&closed, cursor), None);

        let unclosed = "`\"[raw `link[x]{to=\"doc|\"}";
        let (unclosed, cursor) = strip_cursor(unclosed);
        assert_eq!(completion_context(&unclosed, cursor), None);

        let block = "`{language=text}\n  raw `link[x]{to=\"doc|\"}\n";
        let (block, cursor) = strip_cursor(block);
        assert_eq!(completion_context(&block, cursor), None);
    }

    fn completion_context(source: &str, offset: usize) -> Option<LinkCompletionContext> {
        link_completion_context(&parse(source), offset)
    }

    fn strip_cursor(source: &str) -> (String, usize) {
        let offset = source.find('|').unwrap();
        (source.replacen('|', "", 1), offset)
    }
}
