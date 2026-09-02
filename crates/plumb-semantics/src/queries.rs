use std::ops::Range;

use plumb_edit::render_authored_text_arguments;
use plumb_syntax::{
    AttrItem, AttrValue, Attributes, Block, Inline, InlineContent, ParsedBlock, ParsedDocument,
};

use crate::document::has_uri_scheme;
use crate::{parse_task_reference_target, TaskReferenceTarget};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkCompletionContext {
    Label {
        replace: Range<usize>,
        query: String,
    },
    Path {
        replace: Range<usize>,
        query: String,
        parsed: bool,
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
}

pub type FileCompletionContext = ImageCompletionContext;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDependencyCompletionContext {
    pub replace: Range<usize>,
    pub query: String,
    pub task_range: Range<usize>,
    pub existing: Vec<TaskReferenceTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventTitleCompletionContext {
    pub replace: Range<usize>,
    pub query: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitationCompletionContext {
    pub replace: Range<usize>,
    pub query: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstructCompletionContext {
    Citation { replace: Range<usize> },
    TaskEventLinkAndAutolink { replace: Range<usize> },
    LinkAndAutolink { replace: Range<usize> },
    Autolink { replace: Range<usize> },
    Link { replace: Range<usize> },
}

pub fn citation_completion_context(
    document: &ParsedDocument,
    offset: usize,
) -> Option<CitationCompletionContext> {
    if offset > document.source.len() || !document.source.is_char_boundary(offset) {
        return None;
    }
    let start = document.source[..offset].rfind("`cite{")? + "`cite{".len();
    if document.source[start..offset].contains(['}', '\n', '\r', '`']) {
        return None;
    }
    Some(CitationCompletionContext {
        replace: start..offset,
        query: document.source[start..offset].to_string(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributeCompletion {
    pub label: &'static str,
    pub new_text: String,
    pub detail: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributeCompletionContext {
    pub replace: Range<usize>,
    pub completions: Vec<AttributeCompletion>,
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
            Block::Verbatim(_) => {}
            Block::Parsed(block) => {
                if let Some(context) = direct_block_attribute_context(block, source, offset) {
                    return Some(context);
                }
                if let Some(context) = attribute_context_in_inlines(&block.content, source, offset)
                {
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

fn direct_block_attribute_context(
    owner: &ParsedBlock,
    source: &str,
    offset: usize,
) -> Option<AttributeCompletionContext> {
    let owner_mark = owner.mark.as_ref()?;
    for child in &owner.children {
        let Block::Parsed(declaration) = child else {
            continue;
        };
        let Some(declaration_mark) = declaration.mark.as_ref() else {
            continue;
        };
        if !declaration.children.is_empty()
            || !matches!(declaration_mark.marker.as_str(), "@" | "+" | "=")
            || offset < declaration.range.start
            || offset > declaration.content.range.end
        {
            continue;
        }

        if let Some(context) =
            direct_block_value_context(owner_mark.marker.as_str(), declaration, source, offset)
        {
            return Some(context);
        }

        if declaration.content.data.len() > 1 {
            continue;
        }
        let query_start = declaration
            .content
            .argument(0)
            .map_or(declaration.content.range.end, |argument| {
                argument.range.start
            });
        if offset < query_start {
            continue;
        }
        let query = &source[query_start..offset];
        if query.chars().any(char::is_whitespace) {
            continue;
        }
        let mut completions = Vec::new();
        match declaration_mark.marker.as_str() {
            "@" if query.is_empty() && owner_mark.attrs.id().is_none() => {
                completions.push(AttributeCompletion {
                    label: "id",
                    new_text: "`@ ".to_string(),
                    detail: "explicit id",
                });
            }
            "=" => {
                let existing = |key: &str| {
                    owner_mark
                        .attrs
                        .items
                        .iter()
                        .any(|item| matches!(item, AttrItem::Pair { key: existing, .. } if existing == key))
                };
                match crate::list_item_facet(owner) {
                    crate::ListItemFacet::Task => {
                        for (key, detail) in task_attribute_pairs() {
                            push_block_pair_completion(
                                &mut completions,
                                !existing(key),
                                key,
                                detail,
                            );
                        }
                    }
                    crate::ListItemFacet::Event => {
                        for (key, detail) in event_attribute_pairs() {
                            push_block_pair_completion(
                                &mut completions,
                                !existing(key),
                                key,
                                detail,
                            );
                        }
                    }
                    _ => {}
                }
                completions.retain(|candidate| candidate.label.starts_with(query));
            }
            _ => {}
        }
        return Some(AttributeCompletionContext {
            replace: declaration.range.start..offset,
            completions,
        });
    }
    None
}

fn direct_block_value_context(
    owner_marker: &str,
    declaration: &ParsedBlock,
    source: &str,
    offset: usize,
) -> Option<AttributeCompletionContext> {
    if owner_marker != "$"
        || declaration.mark.as_ref()?.marker != "="
        || declaration.content.argument_plain_text(0)?.as_str() != "language"
    {
        return None;
    }
    let key = declaration.content.argument(0)?;
    let value = declaration.content.argument(1)?;
    let [Inline::Text { text: key, .. }] = key.items.as_slice() else {
        return None;
    };
    let [Inline::Text { range, .. }] = value.items.as_slice() else {
        return None;
    };
    if key != "language" || offset < range.start || offset > range.end {
        return None;
    }
    let query = &source[range.start..offset];
    "tex"
        .starts_with(query)
        .then_some(AttributeCompletionContext {
            replace: range.clone(),
            completions: vec![AttributeCompletion {
                label: "tex",
                new_text: "tex".to_string(),
                detail: "standard TeX math language",
            }],
        })
}

fn push_block_pair_completion(
    candidates: &mut Vec<AttributeCompletion>,
    include: bool,
    key: &'static str,
    detail: &'static str,
) {
    if !include {
        return;
    }
    let value = match key {
        "created" | "due" | "wait" | "recur" | "prev" | "depends" | "date" | "timezone"
        | "tasks" | "language" => "",
        "priority" => "0",
        _ => return,
    };
    let arguments = render_authored_text_arguments(&[key, value]);
    candidates.push(AttributeCompletion {
        label: key,
        new_text: format!("`= {arguments}"),
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

fn event_attribute_pairs() -> [(&'static str, &'static str); 3] {
    [
        ("date", "event date override"),
        ("timezone", "event timezone override"),
        ("tasks", "related task references"),
    ]
}

fn attribute_context_in_inlines(
    content: &InlineContent,
    source: &str,
    offset: usize,
) -> Option<AttributeCompletionContext> {
    for inline in &content.items {
        match inline {
            Inline::Group { mark, content, .. } => {
                if let Some(mark) = mark {
                    if let Some(context) = inline_member_attribute_context(
                        &mark.marker,
                        content,
                        &mark.attrs,
                        source,
                        offset,
                    ) {
                        return Some(context);
                    }
                }
                if let Some(context) = attribute_context_in_inlines(content, source, offset) {
                    return Some(context);
                }
            }
            Inline::Verbatim { .. } => {}
            Inline::Text { .. } | Inline::Space { .. } | Inline::SoftBreak { .. } => {}
        }
    }
    None
}

fn inline_member_attribute_context(
    owner_kind: &str,
    content: &InlineContent,
    attrs: &Attributes,
    source: &str,
    offset: usize,
) -> Option<AttributeCompletionContext> {
    for datum in &content.data {
        let [Inline::Group {
            range,
            mark: Some(mark),
            content: declaration,
            ..
        }] = &content.items[datum.item_range.clone()]
        else {
            continue;
        };
        if offset < range.start || offset > range.end || mark.marker != "=" {
            continue;
        }
        let arguments = crate::positional_data(declaration);
        if let [key, value] = arguments.as_slice() {
            let value_range = value.range.clone();
            if owner_kind == "$"
                && key.plain_text() == "language"
                && value_range.start <= offset
                && offset <= value_range.end
            {
                let query = &source[value_range.start..offset];
                if "tex".starts_with(query) {
                    return Some(AttributeCompletionContext {
                        replace: value_range,
                        completions: vec![AttributeCompletion {
                            label: "tex",
                            new_text: "tex".to_string(),
                            detail: "standard TeX math language",
                        }],
                    });
                }
            }
            continue;
        }
        let [key] = arguments.as_slice() else {
            continue;
        };
        let key_range = key.range.clone();
        if offset < key_range.start || offset > key_range.end {
            continue;
        }
        let query = &source[key_range.start..offset];
        let candidate = match owner_kind {
            "img" | "file" if attrs.value("src").is_none() && "src".starts_with(query) => {
                AttributeCompletion {
                    label: "src",
                    new_text: "`={src {}}".to_string(),
                    detail: "resource source",
                }
            }
            "$" if attrs.value("language").is_none() && "language".starts_with(query) => {
                AttributeCompletion {
                    label: "language",
                    new_text: "`={language {}}".to_string(),
                    detail: "raw content language",
                }
            }
            _ => continue,
        };
        return Some(AttributeCompletionContext {
            replace: range.clone(),
            completions: vec![candidate],
        });
    }
    None
}

pub fn construct_completion_context(
    document: &ParsedDocument,
    offset: usize,
) -> Option<ConstructCompletionContext> {
    let source = &document.source;
    if offset == 0 || offset > source.len() || !source.is_char_boundary(offset) {
        return None;
    }
    let introducer = source[..offset]
        .char_indices()
        .rev()
        .find_map(|(index, character)| (character == '`').then_some(index))?;
    let prefix = &source[introducer..offset];
    let marker_prefix = prefix.strip_prefix('`')?;
    let autolink_opener = marker_prefix == "->\"";
    if source[..introducer].ends_with('`')
        || (!autolink_opener && blocks_contain_verbatim(&document.syntax.blocks, introducer))
        || document.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "syntax.unclosed-verbatim"
                && diagnostic.range.start < introducer
                && introducer <= diagnostic.range.end
        })
        || blocks_attributes_contain(&document.syntax.blocks, introducer)
    {
        return None;
    }
    let replace = introducer..offset;
    let line_start = source[..introducer]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let block_position = source[line_start..introducer]
        .chars()
        .all(|character| character == ' ');
    if marker_prefix.is_empty() {
        return None;
    }
    if "cite".starts_with(marker_prefix) {
        Some(ConstructCompletionContext::Citation { replace })
    } else {
        match marker_prefix {
            "-" if block_position => {
                Some(ConstructCompletionContext::TaskEventLinkAndAutolink { replace })
            }
            "-" | "->" => Some(ConstructCompletionContext::LinkAndAutolink { replace }),
            "->{" => Some(ConstructCompletionContext::Link { replace }),
            "->\"" => Some(ConstructCompletionContext::Autolink { replace }),
            _ => None,
        }
    }
}

pub fn event_title_completion_context(
    document: &ParsedDocument,
    offset: usize,
) -> Option<EventTitleCompletionContext> {
    if offset > document.source.len() || !document.source.is_char_boundary(offset) {
        return None;
    }
    event_title_context_in_blocks(&document.syntax.blocks, &document.source, offset)
}

fn event_title_context_in_blocks(
    blocks: &[Block],
    source: &str,
    offset: usize,
) -> Option<EventTitleCompletionContext> {
    for block in blocks {
        let Block::Parsed(block) = block else {
            continue;
        };
        if crate::list_item_facet(block) == crate::ListItemFacet::Event
            && offset == block.content.range.end
        {
            if block.content.data.len() < 2 {
                return None;
            }
            let first = &block.content.data[1];
            let last = block.content.data.last()?;
            let title_content = InlineContent::from_items(
                first.range.start..last.range.end,
                block.content.items[first.item_range.start..last.item_range.end].to_vec(),
            );
            let title = title_content.items.as_slice();
            if title
                .iter()
                .any(|inline| !matches!(inline, Inline::Text { .. } | Inline::Space { .. }))
            {
                return None;
            }
            let replace_start = title
                .first()
                .map_or(offset, |inline| inline_range(inline).start);
            let query = source.get(replace_start..offset)?.to_string();
            if query.contains(['\n', '\r']) {
                return None;
            }
            return Some(EventTitleCompletionContext {
                replace: replace_start..offset,
                query,
            });
        }
        if let Some(context) = event_title_context_in_blocks(&block.children, source, offset) {
            return Some(context);
        }
    }
    None
}

fn inline_range(inline: &Inline) -> &Range<usize> {
    match inline {
        Inline::Text { range, .. }
        | Inline::Space { range, .. }
        | Inline::SoftBreak { range }
        | Inline::Group { range, .. }
        | Inline::Verbatim { range, .. } => range,
    }
}

pub fn task_dependency_completion_context(
    document: &ParsedDocument,
    offset: usize,
) -> Option<TaskDependencyCompletionContext> {
    if offset > document.source.len() || !document.source.is_char_boundary(offset) {
        return None;
    }
    task_dependency_context_in_blocks(&document.syntax.blocks, &document.source, offset)
}

fn task_dependency_context_in_blocks(
    blocks: &[Block],
    source: &str,
    offset: usize,
) -> Option<TaskDependencyCompletionContext> {
    for block in blocks {
        let Block::Parsed(block) = block else {
            continue;
        };
        if let Some(mark) = &block.mark {
            if crate::list_item_facet(block) == crate::ListItemFacet::Task {
                let value = mark.attrs.items.iter().find_map(|item| match item {
                    AttrItem::Pair { key, value, .. } if key == "depends" => Some(value),
                    _ => None,
                });
                if let Some(value) = value {
                    if let Some(context) =
                        task_dependency_context(source, offset, value, block.range.clone())
                    {
                        return Some(context);
                    }
                }
            }
        }
        if let Some(context) = task_dependency_context_in_blocks(&block.children, source, offset) {
            return Some(context);
        }
    }
    None
}

fn task_dependency_context(
    source: &str,
    offset: usize,
    value: &AttrValue,
    task_range: Range<usize>,
) -> Option<TaskDependencyCompletionContext> {
    let delimited = source.as_bytes().get(value.range.start) == Some(&b'"');
    let content_start = value.range.start + usize::from(delimited);
    let content_end =
        if delimited && source.as_bytes().get(value.range.end.saturating_sub(1)) == Some(&b'"') {
            value.range.end - 1
        } else {
            value.range.end
        };
    let content_range = content_start..content_end;
    if offset < content_range.start || offset > content_range.end {
        return None;
    }
    if source[content_range.clone()].contains('\\') {
        return None;
    }
    let tokens = task_dependency_tokens(source, content_range.clone());
    let current = tokens
        .iter()
        .find(|(_, range)| range.start <= offset && offset <= range.end);
    let replace = if let Some((_, range)) = current {
        range.clone()
    } else {
        let start = tokens
            .iter()
            .filter(|(_, range)| range.end <= offset)
            .map(|(_, range)| range.end)
            .max()
            .unwrap_or(content_range.start);
        let start = source[start..offset]
            .char_indices()
            .find(|(_, character)| !character.is_whitespace())
            .map_or(offset, |(relative, _)| start + relative);
        start..offset
    };
    let query = source[replace.start..offset].to_string();
    let current_range = current.map(|(_, range)| range.clone());
    let existing = tokens
        .iter()
        .filter(|(_, range)| Some(range.clone()) != current_range)
        .map(|(token, _)| parse_task_reference_target(token))
        .collect();
    Some(TaskDependencyCompletionContext {
        replace,
        query,
        task_range,
        existing,
    })
}

fn task_dependency_tokens(source: &str, range: Range<usize>) -> Vec<(&str, Range<usize>)> {
    let value = &source[range.clone()];
    let mut output = Vec::new();
    let mut cursor = 0;
    while cursor < value.len() {
        cursor += value[cursor..]
            .chars()
            .take_while(|character| character.is_whitespace())
            .map(char::len_utf8)
            .sum::<usize>();
        if cursor == value.len() {
            break;
        }
        let start = cursor;
        let id_start = if value[start..].starts_with('#') {
            start + 1
        } else if let Some(separator) = value[start..]
            .find(".plumb#")
            .filter(|separator| !value[start..start + separator].contains('#'))
        {
            start + separator + ".plumb#".len()
        } else {
            start
        };
        let end = value[id_start..]
            .find(char::is_whitespace)
            .map_or(value.len(), |relative| id_start + relative);
        output.push((&value[start..end], range.start + start..range.start + end));
        cursor = end;
    }
    output
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
    let (_, content) = find_marked_group(&document.syntax.blocks, offset, "->")?;
    let positional = crate::positional_data(content);
    let value = match positional.as_slice() {
        [target] => target,
        [_, target] => target,
        _ => return None,
    };
    if offset < value.range.start || offset > value.range.end {
        return None;
    }
    let (value_start, value_end) = editable_datum_range(value);
    if offset < value_start || offset > value_end {
        return None;
    }
    let query = &source[value_start..offset];
    if query.contains('"') || query.contains('}') || query.chars().any(char::is_control) {
        return None;
    }
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
            parsed: true,
        })
    }
}

fn find_marked_group<'a>(
    blocks: &'a [Block],
    offset: usize,
    marker: &str,
) -> Option<(Range<usize>, &'a InlineContent)> {
    for block in blocks {
        let Block::Parsed(block) = block else {
            continue;
        };
        if let Some(found) = find_marked_group_in_content(&block.content, offset, marker) {
            return Some(found);
        }
        if let Some(found) = find_marked_group(&block.children, offset, marker) {
            return Some(found);
        }
    }
    None
}

fn find_marked_group_in_content<'a>(
    content: &'a InlineContent,
    offset: usize,
    marker: &str,
) -> Option<(Range<usize>, &'a InlineContent)> {
    for inline in &content.items {
        let Inline::Group {
            range,
            mark,
            content,
        } = inline
        else {
            continue;
        };
        if range.start <= offset && offset <= range.end {
            if mark.as_ref().is_some_and(|mark| mark.marker == marker) {
                return Some((range.clone(), content));
            }
            if let Some(found) = find_marked_group_in_content(content, offset, marker) {
                return Some(found);
            }
        }
    }
    None
}

fn editable_datum_range(content: &InlineContent) -> (usize, usize) {
    if let [Inline::Group {
        mark: None,
        content,
        ..
    }] = content.items.as_slice()
    {
        return (content.range.start, content.range.end);
    }
    if let [Inline::Verbatim {
        mark: None,
        text_range,
        ..
    }] = content.items.as_slice()
    {
        return (text_range.start, text_range.end);
    }
    (content.range.start, content.range.end)
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
    let (_, owner) = find_marked_group(&document.syntax.blocks, offset, kind)?;
    let property = owner.items.iter().find_map(|inline| {
        let Inline::Group {
            range,
            mark: Some(mark),
            content,
        } = inline
        else {
            return None;
        };
        (mark.marker == "=" && range.start <= offset && offset <= range.end).then_some(content)
    })?;
    let data = crate::positional_data(property);
    let key = data.first()?;
    if key.plain_text() != "src" {
        return None;
    }
    let (value_start, value_end) = if let Some(value) = data.get(1) {
        if offset < value.range.start || offset > value.range.end {
            return None;
        }
        editable_datum_range(value)
    } else {
        if offset < key.range.end
            || !source[key.range.end..offset]
                .chars()
                .all(|character| character == ' ')
        {
            return None;
        }
        (offset, offset)
    };
    let query = &source[value_start..offset];
    if query
        .chars()
        .any(|character| character.is_control() || character == '\\')
        || has_uri_scheme(query)
        || query.starts_with("//")
    {
        return None;
    }
    Some(ImageCompletionContext {
        replace: value_start..value_end,
        query: query.to_string(),
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
        Block::Parsed(block) => inlines_find_autolink(source, &block.content, offset)
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
            mark,
            text_range,
            quote_count,
            ..
        } if text_range.start <= offset
            && offset <= text_range.end
            && mark.as_ref().is_some_and(|mark| mark.marker == "->") =>
        {
            let envelope_end = range.end;
            component_completion_context(
                source,
                text_range,
                range.start..envelope_end,
                *quote_count,
                offset,
            )
        }
        Inline::Group { content, .. } => inlines_find_autolink(source, content, offset),
        Inline::Verbatim { .. }
        | Inline::Text { .. }
        | Inline::Space { .. }
        | Inline::SoftBreak { .. } => None,
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
        Block::Verbatim(block) => block.text_range.contains(&offset),
        Block::Parsed(block) => {
            inlines_contain_verbatim(&block.content, offset)
                || blocks_contain_verbatim(&block.children, offset)
        }
    })
}

fn blocks_attributes_contain(blocks: &[Block], offset: usize) -> bool {
    blocks.iter().any(|block| match block {
        Block::Verbatim(_) => false,
        Block::Parsed(block) => {
            block.mark.as_ref().is_some_and(|mark| {
                mark.attrs
                    .range
                    .as_ref()
                    .is_some_and(|range| range.contains(&offset))
            }) || inlines_attributes_contain(&block.content, offset)
                || blocks_attributes_contain(&block.children, offset)
        }
    })
}

fn inlines_attributes_contain(content: &InlineContent, offset: usize) -> bool {
    content.items.iter().any(|inline| match inline {
        Inline::Group { mark, content, .. } => {
            mark.as_ref().is_some_and(|mark| {
                mark.attrs
                    .range
                    .as_ref()
                    .is_some_and(|range| range.contains(&offset))
            }) || inlines_attributes_contain(content, offset)
        }
        Inline::Verbatim { mark, .. } => mark.as_ref().is_some_and(|mark| {
            mark.attrs
                .range
                .as_ref()
                .is_some_and(|range| range.contains(&offset))
        }),
        Inline::Text { .. } | Inline::Space { .. } | Inline::SoftBreak { .. } => false,
    })
}

fn inlines_contain_verbatim(content: &InlineContent, offset: usize) -> bool {
    content.items.iter().any(|inline| match inline {
        Inline::Verbatim { text_range, .. } => {
            text_range.start <= offset && offset <= text_range.end
        }
        Inline::Group { content, .. } => inlines_contain_verbatim(content, offset),
        Inline::Text { .. } | Inline::Space { .. } | Inline::SoftBreak { .. } => false,
    })
}

#[cfg(any())]
mod tests {
    use crate::parse_legacy as parse;

    use super::*;

    #[test]
    fn classifies_construct_completion_by_source_context() {
        let block = parse("`");
        assert_eq!(construct_completion_context(&block, 1), None);
        let nested = parse("  `");
        assert_eq!(construct_completion_context(&nested, 3), None);
        let inline = parse("Text `");
        assert_eq!(construct_completion_context(&inline, 6), None);

        for prefix in ["`t", "`ta", "`task", "`e", "`ev", "`event"] {
            let source = format!(
                "`- something\n `+ task\n `= created|2026-08-09T10:55:24+08:00\n\n{prefix}"
            );
            let parsed = parse(&source);
            assert_eq!(construct_completion_context(&parsed, source.len()), None);
        }
        for source in ["`span[x|=[key|value]]`-", "`\"raw\"`-"] {
            let parsed = parse(source);
            let start = source.rfind('`').unwrap();
            assert_eq!(
                construct_completion_context(&parsed, source.len()),
                Some(ConstructCompletionContext::LinkAndAutolink {
                    replace: start..source.len()
                })
            );
        }
        let after_verbatim = "`rust\n|\"\n raw\n`t";
        let parsed = parse(after_verbatim);
        assert_eq!(
            construct_completion_context(&parsed, after_verbatim.len()),
            None
        );

        for source in ["`-", "  `-"] {
            let parsed = parse(source);
            let start = source.rfind('`').unwrap();
            assert_eq!(
                construct_completion_context(&parsed, source.len()),
                Some(ConstructCompletionContext::TaskEventLinkAndAutolink {
                    replace: start..source.len()
                })
            );
        }
        for source in ["`->", "Text `-", "Text `->"] {
            let parsed = parse(source);
            let start = source.rfind('`').unwrap();
            assert_eq!(
                construct_completion_context(&parsed, source.len()),
                Some(ConstructCompletionContext::LinkAndAutolink {
                    replace: start..source.len()
                })
            );
        }
        let block_autolink = parse("`[");
        assert_eq!(construct_completion_context(&block_autolink, 2), None);
        let old_autolink = parse("Text `[");
        assert_eq!(
            construct_completion_context(&old_autolink, old_autolink.source.len()),
            None
        );
        let link = parse("Text `->[");
        assert_eq!(
            construct_completion_context(&link, link.source.len()),
            Some(ConstructCompletionContext::Link { replace: 5..9 })
        );
        let autolink = parse("Text `->\"");
        assert_eq!(
            construct_completion_context(&autolink, autolink.source.len()),
            Some(ConstructCompletionContext::Autolink { replace: 5..9 })
        );
        for source in ["Text `t", "Text `e", "`tx", "`eventual"] {
            let parsed = parse(source);
            assert_eq!(construct_completion_context(&parsed, source.len()), None);
        }
        for source in ["Text `c", "Text `ci", "Text `cit", "Text `cite"] {
            let parsed = parse(source);
            let start = source.rfind('`').unwrap();
            assert_eq!(
                construct_completion_context(&parsed, source.len()),
                Some(ConstructCompletionContext::Citation {
                    replace: start..source.len()
                })
            );
        }
        assert_eq!(
            construct_completion_context(&parse("`ci"), 3),
            Some(ConstructCompletionContext::Citation { replace: 0..3 })
        );

        let escaped = parse("Text ``");
        assert_eq!(construct_completion_context(&escaped, 7), None);
        let verbatim = parse("`\"[raw ` content]\"");
        let verbatim_offset = verbatim.source.find("` content").unwrap() + 1;
        assert_eq!(
            construct_completion_context(&verbatim, verbatim_offset),
            None
        );
        let attribute = parse("`node Head\n `= key|``\n");
        let attribute_offset = attribute.source.rfind("``").unwrap() + 1;
        assert_eq!(
            construct_completion_context(&attribute, attribute_offset),
            None
        );
    }

    #[test]
    fn identifies_recovered_citation_id_completion() {
        let recovered = parse("See `cite[smi");
        assert_eq!(
            citation_completion_context(&recovered, recovered.source.len()),
            Some(CitationCompletionContext {
                replace: 10..13,
                query: "smi".to_string(),
            })
        );
        let complete = parse("See `cite[smith2004].");
        assert_eq!(
            citation_completion_context(&complete, 13),
            Some(CitationCompletionContext {
                replace: 10..13,
                query: "smi".to_string(),
            })
        );
    }

    #[test]
    fn locates_plain_event_title_completion_at_head_end() {
        let source = "`- 09:00|rela\n `+ event".to_string();
        let cursor = source.find("rela").unwrap() + "rela".len();
        assert_eq!(
            event_title_completion_context(&parse(&source), cursor),
            Some(EventTitleCompletionContext {
                replace: source.find("rela").unwrap()..cursor,
                query: "rela".to_string(),
            })
        );

        let empty = "`- 09:00|\n `+ event".to_string();
        let cursor = "`- 09:00|".len();
        assert_eq!(
            event_title_completion_context(&parse(&empty), cursor),
            Some(EventTitleCompletionContext {
                replace: cursor..cursor,
                query: String::new(),
            })
        );

        for marked in [
            "`event 09:00|",
            "`event 09:00|`*[relax]|",
            "`event 09:00|re|lax",
            "`task 09:00 relax|",
        ] {
            let (source, cursor) = strip_cursor(marked);
            assert_eq!(
                event_title_completion_context(&parse(&source), cursor),
                None
            );
        }
    }

    #[test]
    fn completes_standard_attributes_from_recovered_owner_context() {
        let (task, cursor) = strip_cursor("`- Work\n `+ task\n `= created|now\n `= pr|\n");
        let context = attribute_completion_context(&parse(&task), cursor).unwrap();
        assert_eq!(context.replace, task.find("`= pr").unwrap()..cursor);
        assert_eq!(
            context
                .completions
                .iter()
                .map(|item| item.label)
                .collect::<Vec<_>>(),
            ["prev", "priority"]
        );
        assert_eq!(context.completions[1].new_text, "`= priority | 0");
    }

    #[test]
    fn completes_direct_declaration_children_with_the_owners_ordinary_syntax() {
        let (task, cursor) = strip_cursor("`- Work\n `+ task\n `= pr|\n");
        let context = attribute_completion_context(&parse(&task), cursor).unwrap();
        assert_eq!(context.replace, task.find("`= pr").unwrap()..cursor);
        assert_eq!(
            context
                .completions
                .iter()
                .map(|item| (item.label, item.new_text.as_str()))
                .collect::<Vec<_>>(),
            [("prev", "`= prev | "), ("priority", "`= priority | 0")]
        );
    }

    #[test]
    fn identifies_task_dependency_tokens_and_preserves_other_references() {
        let (source, cursor) = strip_cursor(
            "`- Review\n `+ task\n `@ review\n `= depends|#done Project Plan.plumb#dr|aft #later\n",
        );
        let current_start = source.find("Project Plan.plumb#draft").unwrap();
        let context = task_dependency_completion_context(&parse(&source), cursor).unwrap();
        assert_eq!(
            context.replace,
            current_start..current_start + "Project Plan.plumb#draft".len()
        );
        assert_eq!(context.query, "Project Plan.plumb#dr");
        assert_eq!(
            context.existing,
            vec![
                TaskReferenceTarget::Internal {
                    id: "done".to_string()
                },
                TaskReferenceTarget::Internal {
                    id: "later".to_string()
                }
            ]
        );

        let (empty, cursor) = strip_cursor("`- Review\n `+ task\n `= depends|#done |\n");
        let context = task_dependency_completion_context(&parse(&empty), cursor).unwrap();
        assert_eq!(context.replace, cursor..cursor);
        assert_eq!(context.query, "");
        assert_eq!(
            context.existing,
            vec![TaskReferenceTarget::Internal {
                id: "done".to_string()
            }]
        );

        let (non_task, cursor) = strip_cursor("`- Plain item\n `= depends|#dr|aft\n");
        assert_eq!(
            task_dependency_completion_context(&parse(&non_task), cursor),
            None
        );

        let (recovered, cursor) = strip_cursor("`- Review\n `+ task\n `= depends|#dr|aft\n");
        let context = task_dependency_completion_context(&parse(&recovered), cursor).unwrap();
        assert_eq!(context.query, "#dr");
        assert_eq!(&recovered[context.replace], "#draft");
    }

    #[test]
    fn suppresses_duplicate_attributes_and_completes_enum_values() {
        let (task, cursor) = strip_cursor("`- Work\n `+ task\n `= priority|2\n `= |\n");
        let context = attribute_completion_context(&parse(&task), cursor).unwrap();
        assert!(!context
            .completions
            .iter()
            .any(|item| item.label == "priority"));

        let (quoted, cursor) = strip_cursor("`- Work\n `+ task\n `= due|2026-|\n");
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
        let path = "See `->[x|doc";
        let path_start = path.find("doc").unwrap();
        assert_eq!(
            completion_context(path, path.len()),
            Some(LinkCompletionContext::Path {
                replace: path_start..path.len(),
                query: "doc".to_string(),
                parsed: true,
            })
        );
        let anchor = "See `->[x|doc.plumb#tar";
        let fragment_start = anchor.find("tar").unwrap();
        assert_eq!(
            completion_context(anchor, anchor.len()),
            Some(LinkCompletionContext::Anchor {
                path: "doc.plumb".to_string(),
                replace: fragment_start..anchor.len(),
                query: "tar".to_string(),
            })
        );
    }

    #[test]
    fn replaces_complete_target_components_around_the_cursor() {
        let (path, cursor) = strip_cursor("See `->[x|do|c.plumb#target]\n");
        let value_start = path.find("doc.plumb").unwrap();
        let separator = path.find("#target").unwrap();
        assert_eq!(
            completion_context(&path, cursor),
            Some(LinkCompletionContext::Path {
                replace: value_start..separator,
                query: "do".to_string(),
                parsed: true,
            })
        );

        let (anchor, cursor) = strip_cursor("See `->[x|doc.plumb#ta|rget]\n");
        let fragment_start = anchor.find("target").unwrap();
        assert_eq!(
            completion_context(&anchor, cursor),
            Some(LinkCompletionContext::Anchor {
                path: "doc.plumb".to_string(),
                replace: fragment_start..fragment_start + "target".len(),
                query: "ta".to_string(),
            })
        );

        let (empty, cursor) = strip_cursor("See `->[x||]\n");
        assert_eq!(
            completion_context(&empty, cursor),
            Some(LinkCompletionContext::Path {
                replace: cursor..cursor,
                query: String::new(),
                parsed: true,
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

        let block = "`text\n|\"\n raw `->[x]{to=\"doc|\"}\n";
        let (block, cursor) = strip_cursor(block);
        assert_eq!(completion_context(&block, cursor), None);
    }

    #[test]
    fn completes_paths_and_anchors_inside_autolinks() {
        let (path, cursor) = strip_cursor("See `->\"do|c.plumb\"\n");
        let value_start = path.find("doc.plumb").unwrap();
        assert_eq!(
            completion_context(&path, cursor),
            Some(LinkCompletionContext::AutolinkPath {
                replace: value_start..value_start + "doc.plumb".len(),
                envelope: path.find('`').unwrap()..path.find('"').unwrap() + "\"doc.plumb\"".len(),
                quote_count: 1,
                suffix: String::new(),
                query: "do".to_string(),
            })
        );

        let (anchor, cursor) = strip_cursor("See `->\"doc.plumb#ta|rget\"\n");
        let fragment_start = anchor.find("target").unwrap();
        assert_eq!(
            completion_context(&anchor, cursor),
            Some(LinkCompletionContext::AutolinkAnchor {
                path: "doc.plumb".to_string(),
                replace: fragment_start..fragment_start + "target".len(),
                query: "ta".to_string(),
            })
        );

        let (ordinary, cursor) = strip_cursor("See `\"doc.pl|umb\"");
        assert_eq!(completion_context(&ordinary, cursor), None);
    }

    #[test]
    fn does_not_guess_that_verbatim_is_an_autolink() {
        let incomplete = "See `\"do";
        assert_eq!(completion_context(incomplete, incomplete.len()), None);

        let closed_code = "See `\"doc\"";
        assert_eq!(completion_context(closed_code, closed_code.len() - 1), None);
        let empty = "See `\"[]\"";
        assert_eq!(completion_context(empty, empty.len() - 1), None);
    }

    #[test]
    fn completes_image_source_values_in_valid_and_recovered_documents() {
        let (valid, cursor) = strip_cursor("`img[Alt|=[src|static/im|age.png]]\n");
        let value_start = valid.find("static/image.png").unwrap();
        assert_eq!(
            image_completion(&valid, cursor),
            Some(ImageCompletionContext {
                replace: value_start..value_start + "static/image.png".len(),
                query: "static/im".to_string(),
            })
        );

        let (recovered, cursor) = strip_cursor("`img[Alt|=[src|static/im|");
        assert_eq!(
            image_completion(&recovered, cursor),
            Some(ImageCompletionContext {
                replace: recovered.find("static/im").unwrap()..cursor,
                query: "static/im".to_string(),
            })
        );

        let (external, cursor) = strip_cursor("`img[Alt|=[src|https:|//example.test/a.png]]\n");
        assert_eq!(image_completion(&external, cursor), None);

        let (literal_path, cursor) = strip_cursor("`img[Alt|=[src|static/a#b?quote\"|]]\n");
        let value_start = literal_path.find("static/a#b?quote\"").unwrap();
        assert_eq!(
            image_completion(&literal_path, cursor),
            Some(ImageCompletionContext {
                replace: value_start..value_start + "static/a#b?quote\"".len(),
                query: "static/a#b?quote\"".to_string(),
            })
        );
    }

    #[test]
    fn completes_file_source_values_without_confusing_images() {
        let (file, cursor) = strip_cursor("`file[Demo|=[src|static/de|mo.mp4]]\n");
        let value_start = file.find("static/demo.mp4").unwrap();
        assert_eq!(
            file_completion_context(&parse(&file), cursor),
            Some(FileCompletionContext {
                replace: value_start..value_start + "static/demo.mp4".len(),
                query: "static/de".to_string(),
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
        let offset = source.rfind('|').unwrap();
        let mut source = source.to_string();
        source.remove(offset);
        (source, offset)
    }
}
