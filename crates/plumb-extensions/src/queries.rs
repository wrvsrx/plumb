use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkCompletionContext {
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

pub fn link_completion_context(source: &str, offset: usize) -> Option<LinkCompletionContext> {
    if offset > source.len() || !source.is_char_boundary(offset) {
        return None;
    }
    let line_start = source[..offset].rfind('\n').map_or(0, |index| index + 1);
    let prefix = &source[line_start..offset];
    let link_start = prefix.rfind("`link[")? + line_start;
    let after_label =
        source[link_start + "`link[".len()..offset].rfind("]{")? + link_start + "`link[".len() + 2;
    let attrs = &source[after_label..offset];
    let to = attrs.rfind("to=")? + after_label;
    if to > after_label {
        let previous = source[..to].chars().next_back()?;
        if !previous.is_whitespace() && previous != '{' {
            return None;
        }
    }
    let mut value_start = to + 3;
    if value_start < offset && source.as_bytes()[value_start] == b'"' {
        value_start += 1;
    }
    let query = &source[value_start..offset];
    if query.contains('"') || query.contains('}') || query.chars().any(char::is_control) {
        return None;
    }
    if let Some((path, fragment)) = query.split_once('#') {
        let fragment_start = value_start + path.len() + 1;
        Some(LinkCompletionContext::Anchor {
            path: path.to_string(),
            replace: fragment_start..offset,
            query: fragment.to_string(),
        })
    } else {
        Some(LinkCompletionContext::Path {
            replace: value_start..offset,
            query: query.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_incomplete_path_and_anchor_contexts() {
        let path = "See `link[x]{to=\"doc";
        assert_eq!(
            link_completion_context(path, path.len()),
            Some(LinkCompletionContext::Path {
                replace: 17..20,
                query: "doc".to_string(),
            })
        );
        let anchor = "See `link[x]{to=\"doc.plumb#tar";
        assert_eq!(
            link_completion_context(anchor, anchor.len()),
            Some(LinkCompletionContext::Anchor {
                path: "doc.plumb".to_string(),
                replace: 27..30,
                query: "tar".to_string(),
            })
        );
    }
}
