use std::ops::Range;

use plumb_core::{AttrItem, Attributes, Block, Inline, InlineContent, ParsedDocument};

const LINK_OPEN: &str = "`->[";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkCompletionContext {
    Label {
        replace: Range<usize>,
        query: String,
    },
    Path {
        replace: Range<usize>,
        query: String,
        quoted: bool,
    },
    AutolinkPath {
        replace: Range<usize>,
        envelope: Range<usize>,
        quote_count: usize,
        suffix: String,
        query: String,
    },
    Anchor {
        path: String,
        replace: Range<usize>,
        query: String,
    },
    AutolinkAnchor {
        path: String,
        replace: Range<usize>,
        query: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageCompletionContext {
    pub replace: Range<usize>,
    pub query: String,
    pub quoted: bool,
}

pub type FileCompletionContext = ImageCompletionContext;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstructCompletionContext {
    Block { replace: Range<usize> },
    Inline { replace: Range<usize> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributeCompletion {
    pub label: &'static str,
    pub new_text: &'static str,
    pub detail: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributeCompletionContext {
    pub replace: Range<usize>,
    pub completions: Vec<AttributeCompletion>,
}

#[derive(Clone, Copy)]
enum AttributeOwner<'a> {
    Marked(&'a str),
    ParsedInline(&'a str),
    VerbatimInline,
    VerbatimBlock,
}

pub fn attribute_completion_context(
    document: &ParsedDocument,
    offset: usize,
) -> Option<AttributeCompletionContext> {
    if offset > document.source.len() || !document.source.is_char_boundary(offset) {
        return None;
    }
    attribute_context_in_blocks(&document.syntax.blocks, &document.source, offset)
}

fn attribute_context_in_blocks(
    blocks: &[Block],
    source: &str,
    offset: usize,
) -> Option<AttributeCompletionContext> {
    for block in blocks {
        match block {
            Block::Verbatim(block) => {
                if let Some(context) =
                    attribute_context(source, offset, &block.attrs, AttributeOwner::VerbatimBlock)
                {
                    return Some(context);
                }
            }
            Block::Parsed(block) => {
                if let Some(mark) = &block.mark {
                    if let Some(context) = attribute_context(
                        source,
                        offset,
                        &mark.attrs,
                        AttributeOwner::Marked(&mark.marker),
                    ) {
                        return Some(context);
                    }
                }
                if let Some(context) = attribute_context_in_inlines(&block.head, source, offset) {
                    return Some(context);
                }
                if let Some(context) = attribute_context_in_blocks(&block.children, source, offset)
                {
                    return Some(context);
                }
            }
        }
    }
    None
}

fn attribute_context_in_inlines(
    content: &InlineContent,
    source: &str,
    offset: usize,
) -> Option<AttributeCompletionContext> {
    for inline in &content.items {
        match inline {
            Inline::Element {
                kind,
                content,
                attrs,
                ..
            } => {
                if let Some(context) =
                    attribute_context(source, offset, attrs, AttributeOwner::ParsedInline(kind))
                {
                    return Some(context);
                }
                if let Some(context) = attribute_context_in_inlines(content, source, offset) {
                    return Some(context);
                }
            }
            Inline::Verbatim { attrs, .. } => {
                if let Some(context) =
                    attribute_context(source, offset, attrs, AttributeOwner::VerbatimInline)
                {
                    return Some(context);
                }
            }
            Inline::Text { .. } | Inline::SoftBreak { .. } => {}
        }
    }
    None
}

fn attribute_context(
    source: &str,
    offset: usize,
    attrs: &Attributes,
    owner: AttributeOwner<'_>,
) -> Option<AttributeCompletionContext> {
    let range = attrs.range.as_ref()?;
    if offset <= range.start || offset > range.end {
        return None;
    }
    let content_end = if source.as_bytes().get(range.end.saturating_sub(1)) == Some(&b'}') {
        range.end - 1
    } else {
        range.end
    };
    if offset > content_end {
        return None;
    }
    let mut start = offset;
    while start > range.start + 1 {
        let previous = source[..start].char_indices().next_back()?.0;
        let character = source[previous..start].chars().next()?;
        if character.is_whitespace() || character == '{' {
            break;
        }
        start = previous;
    }
    let typed = &source[start..offset];
    if typed.contains('=') {
        return value_completions(attrs, owner, start..offset, typed);
    }
    let has_id = attrs
        .items
        .iter()
        .any(|item| matches!(item, AttrItem::Id { .. }));
    let existing_pairs = |wanted: &str| {
        attrs
            .items
            .iter()
            .any(|item| matches!(item, AttrItem::Pair { key, .. } if key == wanted))
    };
    let existing_class = |wanted: &str| {
        attrs
            .items
            .iter()
            .any(|item| matches!(item, AttrItem::Class { value, .. } if value == wanted))
    };
    let mut candidates = Vec::new();
    match owner {
        AttributeOwner::Marked("-" | ".") => {
            push_completion(
                &mut candidates,
                !existing_class("task"),
                ".task",
                "standard task facet",
            );
            push_completion(
                &mut candidates,
                !existing_class("event"),
                ".event",
                "standard event facet",
            );
            if attrs.has_class("task") {
                for (key, detail) in task_attribute_pairs() {
                    push_pair_completion(&mut candidates, !existing_pairs(key), key, detail);
                }
            }
            if attrs.has_class("event") {
                for (key, detail) in event_attribute_pairs() {
                    push_pair_completion(&mut candidates, !existing_pairs(key), key, detail);
                }
            }
        }
        AttributeOwner::ParsedInline("->") => {
            push_pair_completion(&mut candidates, !existing_pairs("to"), "to", "link target")
        }
        AttributeOwner::ParsedInline("img") => push_pair_completion(
            &mut candidates,
            !existing_pairs("src"),
            "src",
            "image source",
        ),
        AttributeOwner::VerbatimInline => {
            push_completion(
                &mut candidates,
                !existing_class("->"),
                ".->",
                "standard autolink facet",
            );
            push_completion(
                &mut candidates,
                !existing_class("$"),
                ".$",
                "standard math facet",
            );
            push_pair_completion(
                &mut candidates,
                !existing_pairs("language"),
                "language",
                "raw content language",
            );
        }
        AttributeOwner::VerbatimBlock => {
            push_completion(
                &mut candidates,
                !existing_class("$"),
                ".$",
                "standard math facet",
            );
            push_pair_completion(
                &mut candidates,
                !existing_pairs("language"),
                "language",
                "raw content language",
            );
        }
        AttributeOwner::Marked(_) | AttributeOwner::ParsedInline(_) => {}
    }
    if typed.starts_with('#') && !has_id {
        candidates.clear();
    } else if typed.starts_with('.') {
        candidates.retain(|candidate| candidate.new_text.starts_with('.'));
    } else if !typed.is_empty() {
        candidates.retain(|candidate| candidate.new_text.starts_with(typed));
    }
    Some(AttributeCompletionContext {
        replace: start..offset,
        completions: candidates,
    })
}

fn push_completion(
    candidates: &mut Vec<AttributeCompletion>,
    include: bool,
    spelling: &'static str,
    detail: &'static str,
) {
    if include {
        candidates.push(AttributeCompletion {
            label: spelling,
            new_text: spelling,
            detail,
        });
    }
}

fn push_pair_completion(
    candidates: &mut Vec<AttributeCompletion>,
    include: bool,
    key: &'static str,
    detail: &'static str,
) {
    if !include {
        return;
    }
    let new_text = match key {
        "created" => "created=\"\"",
        "due" => "due=\"\"",
        "wait" => "wait=\"\"",
        "recur" => "recur=\"\"",
        "prev" => "prev=\"\"",
        "depends" => "depends=\"\"",
        "priority" => "priority=0",
        "uid" => "uid=\"\"",
        "at" => "at=\"\"",
        "start" => "start=\"\"",
        "end" => "end=\"\"",
        "tasks" => "tasks=\"\"",
        "to" => "to=\"\"",
        "src" => "src=\"\"",
        "language" => "language=\"\"",
        _ => return,
    };
    candidates.push(AttributeCompletion {
        label: key,
        new_text,
        detail,
    });
}

fn task_attribute_pairs() -> [(&'static str, &'static str); 7] {
    [
        ("created", "task creation datetime"),
        ("due", "task due datetime"),
        ("wait", "task wait datetime"),
        ("recur", "task recurrence"),
        ("prev", "previous task reference"),
        ("depends", "task dependencies"),
        ("priority", "task priority"),
    ]
}

fn event_attribute_pairs() -> [(&'static str, &'static str); 5] {
    [
        ("uid", "event UID"),
        ("at", "event point datetime"),
        ("start", "event start datetime"),
        ("end", "event end datetime"),
        ("tasks", "related task references"),
    ]
}

fn value_completions(
    attrs: &Attributes,
    owner: AttributeOwner<'_>,
    replace: Range<usize>,
    typed: &str,
) -> Option<AttributeCompletionContext> {
    let (key, value) = typed.split_once('=')?;
    let quoted = value.starts_with('"');
    let value = value.trim_start_matches('"');
    let replacement = (replace.start + key.len() + 1 + usize::from(quoted))..replace.end;
    let mut completions = Vec::new();
    if key == "language"
        && matches!(
            owner,
            AttributeOwner::VerbatimInline | AttributeOwner::VerbatimBlock
        )
        && attrs.has_class("$")
        && "tex".starts_with(value)
    {
        completions.push(AttributeCompletion {
            label: "tex",
            new_text: "tex",
            detail: "standard TeX math language",
        });
    }
    (!completions.is_empty()).then_some(AttributeCompletionContext {
        replace: replacement,
        completions,
    })
}

pub fn construct_completion_context(
    document: &ParsedDocument,
    offset: usize,
) -> Option<ConstructCompletionContext> {
    let source = &document.source;
    if offset == 0 || offset > source.len() || !source.is_char_boundary(offset) {
        return None;
    }
    let introducer = source[..offset].char_indices().next_back()?.0;
    if &source[introducer..offset] != "`"
        || source[..introducer].ends_with('`')
        || verbatim_at(document, introducer)
        || blocks_attributes_contain(&document.syntax.blocks, introducer)
    {
        return None;
    }
    let replace = introducer..offset;
    let line_start = source[..introducer]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    if source[line_start..introducer]
        .chars()
        .all(|character| character == ' ')
    {
        Some(ConstructCompletionContext::Block { replace })
    } else {
        Some(ConstructCompletionContext::Inline { replace })
    }
}

pub fn link_completion_context(
    document: &ParsedDocument,
    offset: usize,
) -> Option<LinkCompletionContext> {
    let source = &document.source;
    if offset > source.len() || !source.is_char_boundary(offset) {
        return None;
    }
    if let Some(context) = autolink_completion_context(document, offset) {
        return Some(context);
    }
    if verbatim_at(document, offset) {
        return None;
    }
    let line_start = source[..offset].rfind('\n').map_or(0, |index| index + 1);
    let prefix = &source[line_start..offset];
    let link_start = prefix.rfind(LINK_OPEN)? + line_start;
    let escaped_introducers = source[..link_start]
        .chars()
        .rev()
        .take_while(|character| *character == '`')
        .count();
    if escaped_introducers % 2 == 1 {
        return None;
    }
    let label_start = link_start + LINK_OPEN.len();
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
            quoted,
        })
    }
}

pub fn image_completion_context(
    document: &ParsedDocument,
    offset: usize,
) -> Option<ImageCompletionContext> {
    resource_completion_context(document, offset, "img")
}

pub fn file_completion_context(
    document: &ParsedDocument,
    offset: usize,
) -> Option<FileCompletionContext> {
    resource_completion_context(document, offset, "file")
}

fn resource_completion_context(
    document: &ParsedDocument,
    offset: usize,
    kind: &str,
) -> Option<ImageCompletionContext> {
    let source = &document.source;
    if offset > source.len() || !source.is_char_boundary(offset) || verbatim_at(document, offset) {
        return None;
    }
    let line_start = source[..offset].rfind('\n').map_or(0, |index| index + 1);
    let prefix = &source[line_start..offset];
    let element_start = prefix.rfind(&format!("`{kind}["))? + line_start;
    let escaped_introducers = source[..element_start]
        .chars()
        .rev()
        .take_while(|character| *character == '`')
        .count();
    if escaped_introducers % 2 == 1 {
        return None;
    }
    let after_alt = source[element_start..offset].rfind("]{")? + element_start + 2;
    let attrs = &source[after_alt..offset];
    let src = attrs.rfind("src=")? + after_alt;
    if src > after_alt {
        let previous = source[..src].chars().next_back()?;
        if !previous.is_whitespace() && previous != '{' {
            return None;
        }
    }
    let raw_value_start = src + 4;
    let quoted = source.as_bytes().get(raw_value_start) == Some(&b'"');
    let value_start = raw_value_start + usize::from(quoted);
    if offset < value_start {
        return None;
    }
    let query = &source[value_start..offset];
    if query.contains('"')
        || query.contains('}')
        || query.contains('#')
        || query.contains('?')
        || query.contains(':')
        || query.chars().any(char::is_control)
    {
        return None;
    }
    let value_end = if quoted {
        closing_quote(source, offset).unwrap_or(offset)
    } else {
        source[offset..]
            .find(|character: char| character.is_whitespace() || character == '}')
            .map_or(source.len(), |end| offset + end)
    };
    Some(ImageCompletionContext {
        replace: value_start..value_end,
        query: query.to_string(),
        quoted,
    })
}

fn autolink_completion_context(
    document: &ParsedDocument,
    offset: usize,
) -> Option<LinkCompletionContext> {
    blocks_find_autolink(&document.source, &document.syntax.blocks, offset)
}

fn blocks_find_autolink(
    source: &str,
    blocks: &[Block],
    offset: usize,
) -> Option<LinkCompletionContext> {
    blocks.iter().find_map(|block| match block {
        Block::Verbatim(_) => None,
        Block::Parsed(block) => inlines_find_autolink(source, &block.head, offset)
            .or_else(|| blocks_find_autolink(source, &block.children, offset)),
    })
}

fn inlines_find_autolink(
    source: &str,
    content: &InlineContent,
    offset: usize,
) -> Option<LinkCompletionContext> {
    content.items.iter().find_map(|inline| match inline {
        Inline::Verbatim {
            range,
            text_range,
            quote_count,
            attrs,
            ..
        } if text_range.start <= offset
            && offset <= text_range.end
            && attrs.items.iter().any(
                |item| matches!(item, plumb_core::AttrItem::Class { value, .. } if value == "->"),
            ) =>
        {
            let envelope_end = attrs.range.as_ref().map_or(range.end, |range| range.start);
            component_completion_context(
                source,
                text_range,
                range.start..envelope_end,
                *quote_count,
                offset,
            )
        }
        Inline::Element { content, .. } => inlines_find_autolink(source, content, offset),
        Inline::Verbatim { .. } | Inline::Text { .. } | Inline::SoftBreak { .. } => None,
    })
}

fn component_completion_context(
    source: &str,
    range: &Range<usize>,
    envelope: Range<usize>,
    quote_count: usize,
    offset: usize,
) -> Option<LinkCompletionContext> {
    let prefix = &source[range.start..offset];
    if prefix.chars().any(|character| character.is_control()) {
        return None;
    }
    if let Some((path, fragment)) = prefix.split_once('#') {
        let fragment_start = range.start + path.len() + 1;
        return Some(LinkCompletionContext::AutolinkAnchor {
            path: path.to_string(),
            replace: fragment_start..range.end,
            query: fragment.to_string(),
        });
    }
    let path_end = source[offset..range.end]
        .find('#')
        .map_or(range.end, |separator| offset + separator);
    Some(LinkCompletionContext::AutolinkPath {
        replace: range.start..path_end,
        envelope,
        quote_count,
        suffix: source[path_end..range.end].to_string(),
        query: prefix.to_string(),
    })
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

fn blocks_attributes_contain(blocks: &[Block], offset: usize) -> bool {
    blocks.iter().any(|block| match block {
        Block::Verbatim(block) => block
            .attrs
            .range
            .as_ref()
            .is_some_and(|range| range.start <= offset && offset <= range.end),
        Block::Parsed(block) => {
            block.mark.as_ref().is_some_and(|mark| {
                mark.attrs
                    .range
                    .as_ref()
                    .is_some_and(|range| range.start <= offset && offset <= range.end)
            }) || inlines_attributes_contain(&block.head, offset)
                || blocks_attributes_contain(&block.children, offset)
        }
    })
}

fn inlines_attributes_contain(content: &InlineContent, offset: usize) -> bool {
    content.items.iter().any(|inline| match inline {
        Inline::Element { attrs, content, .. } => {
            attrs
                .range
                .as_ref()
                .is_some_and(|range| range.start <= offset && offset <= range.end)
                || inlines_attributes_contain(content, offset)
        }
        Inline::Verbatim { attrs, .. } => attrs
            .range
            .as_ref()
            .is_some_and(|range| range.start <= offset && offset <= range.end),
        Inline::Text { .. } | Inline::SoftBreak { .. } => false,
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
    fn classifies_construct_completion_by_source_context() {
        let block = parse("`");
        assert_eq!(
            construct_completion_context(&block, 1),
            Some(ConstructCompletionContext::Block { replace: 0..1 })
        );
        let nested = parse("  `");
        assert_eq!(
            construct_completion_context(&nested, 3),
            Some(ConstructCompletionContext::Block { replace: 2..3 })
        );
        let inline = parse("Text `");
        assert_eq!(
            construct_completion_context(&inline, 6),
            Some(ConstructCompletionContext::Inline { replace: 5..6 })
        );

        let escaped = parse("Text ``");
        assert_eq!(construct_completion_context(&escaped, 7), None);
        let verbatim = parse("`\"[raw ` content]\"");
        let verbatim_offset = verbatim.source.find("` content").unwrap() + 1;
        assert_eq!(
            construct_completion_context(&verbatim, verbatim_offset),
            None
        );
        let attribute = parse("`node{key=\"`\"} Head\n");
        let attribute_offset = attribute.source.find("`\"").unwrap() + 1;
        assert_eq!(
            construct_completion_context(&attribute, attribute_offset),
            None
        );
    }

    #[test]
    fn completes_standard_attributes_from_recovered_owner_context() {
        let (task, cursor) = strip_cursor("`-{.task created=now pr|} Work");
        let context = attribute_completion_context(&parse(&task), cursor).unwrap();
        assert_eq!(context.replace, task.find("pr}").unwrap()..cursor);
        assert_eq!(
            context
                .completions
                .iter()
                .map(|item| item.label)
                .collect::<Vec<_>>(),
            ["prev", "priority"]
        );
        assert_eq!(context.completions[1].new_text, "priority=0");

        let (recovered, cursor) = strip_cursor("`img[Alt]{s|");
        let context = attribute_completion_context(&parse(&recovered), cursor).unwrap();
        assert_eq!(context.completions[0].new_text, "src=\"\"");

        let (nested, cursor) = strip_cursor("Text `span[x `img[y]{s|}]{.outer}");
        let context = attribute_completion_context(&parse(&nested), cursor).unwrap();
        assert_eq!(context.completions[0].label, "src");
    }

    #[test]
    fn suppresses_duplicate_attributes_and_completes_enum_values() {
        let (task, cursor) = strip_cursor("`-{.task priority=2 |} Work");
        let context = attribute_completion_context(&parse(&task), cursor).unwrap();
        assert!(!context
            .completions
            .iter()
            .any(|item| item.label == "priority"));

        let (math, cursor) = strip_cursor("`[x]{.$ language=\"t|\"}");
        let context = attribute_completion_context(&parse(&math), cursor).unwrap();
        assert_eq!(context.completions[0].label, "tex");
        assert_eq!(context.completions[0].new_text, "tex");

        let (quoted, cursor) = strip_cursor("`-{.task due=\"2026-|\"} Work");
        assert_eq!(attribute_completion_context(&parse(&quoted), cursor), None);
    }

    #[test]
    fn finds_incomplete_path_and_anchor_contexts() {
        let label = "See `->[Usage";
        assert_eq!(
            completion_context(label, label.len()),
            Some(LinkCompletionContext::Label {
                replace: 4..13,
                query: "Usage".to_string(),
            })
        );
        let closed_label = "See `->[Usage]";
        assert_eq!(
            completion_context(closed_label, closed_label.len() - 1),
            Some(LinkCompletionContext::Label {
                replace: 4..14,
                query: "Usage".to_string(),
            })
        );
        let escaped = "See ``->[Usage";
        assert_eq!(completion_context(escaped, escaped.len()), None);
        let old_kind = "See `link[Usage";
        assert_eq!(completion_context(old_kind, old_kind.len()), None);
        let strengthened = "See ```->[Usage";
        assert!(matches!(
            completion_context(strengthened, strengthened.len()),
            Some(LinkCompletionContext::Label { .. })
        ));
        let path = "See `->[x]{to=\"doc";
        assert_eq!(
            completion_context(path, path.len()),
            Some(LinkCompletionContext::Path {
                replace: 15..18,
                query: "doc".to_string(),
                quoted: true,
            })
        );
        let anchor = "See `->[x]{to=\"doc.plumb#tar";
        assert_eq!(
            completion_context(anchor, anchor.len()),
            Some(LinkCompletionContext::Anchor {
                path: "doc.plumb".to_string(),
                replace: 25..28,
                query: "tar".to_string(),
            })
        );
    }

    #[test]
    fn replaces_complete_target_components_around_the_cursor() {
        let (path, cursor) = strip_cursor("See `->[x]{to=\"do|c.plumb#target\"}");
        let value_start = path.find("doc.plumb").unwrap();
        let separator = path.find("#target").unwrap();
        assert_eq!(
            completion_context(&path, cursor),
            Some(LinkCompletionContext::Path {
                replace: value_start..separator,
                query: "do".to_string(),
                quoted: true,
            })
        );

        let (anchor, cursor) = strip_cursor("See `->[x]{to=\"doc.plumb#ta|rget\"}");
        let fragment_start = anchor.find("target").unwrap();
        assert_eq!(
            completion_context(&anchor, cursor),
            Some(LinkCompletionContext::Anchor {
                path: "doc.plumb".to_string(),
                replace: fragment_start..fragment_start + "target".len(),
                query: "ta".to_string(),
            })
        );

        let (empty, cursor) = strip_cursor("See `->[x]{to=\"|\"}");
        assert_eq!(
            completion_context(&empty, cursor),
            Some(LinkCompletionContext::Path {
                replace: cursor..cursor,
                query: String::new(),
                quoted: true,
            })
        );
    }

    #[test]
    fn ignores_link_like_text_inside_verbatim_payloads() {
        let closed = "`\"[raw `->[x]{to=\"doc|\"}]\"";
        let (closed, cursor) = strip_cursor(closed);
        assert_eq!(completion_context(&closed, cursor), None);

        let unclosed = "`\"[raw `->[x]{to=\"doc|\"}";
        let (unclosed, cursor) = strip_cursor(unclosed);
        assert_eq!(completion_context(&unclosed, cursor), None);

        let block = "`{language=text}\n  raw `->[x]{to=\"doc|\"}\n";
        let (block, cursor) = strip_cursor(block);
        assert_eq!(completion_context(&block, cursor), None);
    }

    #[test]
    fn completes_paths_and_anchors_inside_autolinks() {
        let (path, cursor) = strip_cursor("See `[do|c.plumb]{.->}");
        let value_start = path.find("doc.plumb").unwrap();
        assert_eq!(
            completion_context(&path, cursor),
            Some(LinkCompletionContext::AutolinkPath {
                replace: value_start..value_start + "doc.plumb".len(),
                envelope: path.find('`').unwrap()..path.find("{.->}").unwrap(),
                quote_count: 0,
                suffix: String::new(),
                query: "do".to_string(),
            })
        );

        let (anchor, cursor) = strip_cursor("See `\"[doc.plumb#ta|rget]\"{.->}");
        let fragment_start = anchor.find("target").unwrap();
        assert_eq!(
            completion_context(&anchor, cursor),
            Some(LinkCompletionContext::AutolinkAnchor {
                path: "doc.plumb".to_string(),
                replace: fragment_start..fragment_start + "target".len(),
                query: "ta".to_string(),
            })
        );

        let (ordinary, cursor) = strip_cursor("See `[doc.pl|umb]");
        assert_eq!(completion_context(&ordinary, cursor), None);
    }

    #[test]
    fn does_not_guess_that_verbatim_is_an_autolink() {
        let incomplete = "See `[do";
        assert_eq!(completion_context(incomplete, incomplete.len()), None);

        let closed_code = "See `[doc]";
        assert_eq!(completion_context(closed_code, closed_code.len() - 1), None);
        let empty = "See `[]";
        assert_eq!(completion_context(empty, empty.len() - 1), None);
    }

    #[test]
    fn completes_image_source_values_in_valid_and_recovered_documents() {
        let (valid, cursor) = strip_cursor("`img[Alt]{src=\"static/im|age.png\"}");
        let value_start = valid.find("static/image.png").unwrap();
        assert_eq!(
            image_completion(&valid, cursor),
            Some(ImageCompletionContext {
                replace: value_start..value_start + "static/image.png".len(),
                query: "static/im".to_string(),
                quoted: true,
            })
        );

        let (recovered, cursor) = strip_cursor("`img[Alt]{src=\"static/im|");
        assert_eq!(
            image_completion(&recovered, cursor),
            Some(ImageCompletionContext {
                replace: recovered.find("static/im").unwrap()..cursor,
                query: "static/im".to_string(),
                quoted: true,
            })
        );

        let (external, cursor) = strip_cursor("`img[Alt]{src=\"https:|//example.test/a.png\"}");
        assert_eq!(image_completion(&external, cursor), None);
    }

    #[test]
    fn completes_file_source_values_without_confusing_images() {
        let (file, cursor) = strip_cursor("`file[Demo]{src=\"static/de|mo.mp4\"}");
        let value_start = file.find("static/demo.mp4").unwrap();
        assert_eq!(
            file_completion_context(&parse(&file), cursor),
            Some(FileCompletionContext {
                replace: value_start..value_start + "static/demo.mp4".len(),
                query: "static/de".to_string(),
                quoted: true,
            })
        );
        assert_eq!(image_completion_context(&parse(&file), cursor), None);
    }

    fn completion_context(source: &str, offset: usize) -> Option<LinkCompletionContext> {
        link_completion_context(&parse(source), offset)
    }

    fn image_completion(source: &str, offset: usize) -> Option<ImageCompletionContext> {
        image_completion_context(&parse(source), offset)
    }

    fn strip_cursor(source: &str) -> (String, usize) {
        let offset = source.find('|').unwrap();
        (source.replacen('|', "", 1), offset)
    }
}
