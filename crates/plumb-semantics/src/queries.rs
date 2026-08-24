use std::ops::Range;

use plumb_syntax::{
    AttachedContent, AttrItem, AttrValue, Attributes, Block, Inline, InlineContent, ParsedDocument,
};

use crate::{parse_task_reference_target, TaskReferenceTarget};

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
    pub quoted: bool,
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
    Task { replace: Range<usize> },
    Event { replace: Range<usize> },
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
    let start = document.source[..offset].rfind("`cite[")? + "`cite[".len();
    if document.source[start..offset].contains([']', '\n', '\r', '`']) {
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
    Document,
    Marked(&'a str),
    ParsedInline(&'a str),
    VerbatimInline(&'a str),
    VerbatimBlock(&'a str),
}

pub fn attribute_completion_context(
    document: &ParsedDocument,
    offset: usize,
) -> Option<AttributeCompletionContext> {
    if offset > document.source.len() || !document.source.is_char_boundary(offset) {
        return None;
    }
    attached_attribute_context(
        &document.syntax.attrs,
        &document.source,
        offset,
        AttributeOwner::Document,
    )
    .or_else(|| attribute_context_in_blocks(&document.syntax.blocks, &document.source, offset))
}

fn attribute_context_in_blocks(
    blocks: &[Block],
    source: &str,
    offset: usize,
) -> Option<AttributeCompletionContext> {
    for block in blocks {
        match block {
            Block::Verbatim(block) => {
                if let Some(context) = attached_attribute_context(
                    &block.attrs,
                    source,
                    offset,
                    AttributeOwner::VerbatimBlock(&block.kind),
                ) {
                    return Some(context);
                }
                if let Some(context) = attribute_context(
                    source,
                    offset,
                    &block.attrs,
                    AttributeOwner::VerbatimBlock(&block.kind),
                ) {
                    return Some(context);
                }
            }
            Block::Parsed(block) => {
                if let Some(mark) = &block.mark {
                    if let Some(context) = attached_attribute_context(
                        &mark.attrs,
                        source,
                        offset,
                        AttributeOwner::Marked(&mark.marker),
                    ) {
                        return Some(context);
                    }
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
                kind, slots, attrs, ..
            } => {
                if let Some(context) = attached_attribute_context(
                    attrs,
                    source,
                    offset,
                    AttributeOwner::ParsedInline(kind),
                ) {
                    return Some(context);
                }
                if let Some(context) =
                    attribute_context(source, offset, attrs, AttributeOwner::ParsedInline(kind))
                {
                    return Some(context);
                }
                for slot in slots {
                    if let Some(context) =
                        attribute_context_in_inlines(&slot.content, source, offset)
                    {
                        return Some(context);
                    }
                }
            }
            Inline::Verbatim { kind, attrs, .. } => {
                if let Some(context) = attached_attribute_context(
                    attrs,
                    source,
                    offset,
                    AttributeOwner::VerbatimInline(kind),
                ) {
                    return Some(context);
                }
                if let Some(context) =
                    attribute_context(source, offset, attrs, AttributeOwner::VerbatimInline(kind))
                {
                    return Some(context);
                }
            }
            Inline::Text { .. } | Inline::Space { .. } | Inline::SoftBreak { .. } => {}
        }
    }
    None
}

fn attached_attribute_context(
    attrs: &Attributes,
    source: &str,
    offset: usize,
    owner: AttributeOwner<'_>,
) -> Option<AttributeCompletionContext> {
    let group = attrs.attached.as_ref()?;
    let content_end = group.close_range.start.max(group.open_range.end);
    if offset < group.open_range.end || offset > content_end {
        return None;
    }

    if let Some(context) = attached_value_completion_context(group, source, offset, owner) {
        return Some(context);
    }

    let nested = match &group.content {
        AttachedContent::Blocks(blocks) => attribute_context_in_blocks(blocks, source, offset),
        AttachedContent::Inlines(content) => attribute_context_in_inlines(content, source, offset),
    };
    if nested.is_some() {
        return nested;
    }

    let introducer = source[..offset].rfind('`')?;
    if introducer < group.open_range.end {
        return None;
    }
    let typed = &source[introducer + 1..offset];
    let (declaration, query) = attached_declaration_query(typed)?;
    if query
        .chars()
        .any(|character| matches!(character, '[' | ']' | '{' | '}'))
    {
        return None;
    }
    if matches!(group.content, AttachedContent::Blocks(_)) {
        let line_start = source[..introducer]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        if !source[line_start..introducer]
            .chars()
            .all(|character| character == ' ' || character == '\t')
        {
            return None;
        }
    }

    let attached_source = &source[group.open_range.end..content_end];
    let has_id = attrs.id().is_some()
        || attached_source.contains("`@[")
        || attached_source
            .lines()
            .any(|line| line.trim_start().starts_with("`@ "));
    let has_pair = |key: &str| {
        attrs.value(key).is_some()
            || attached_source.contains(&format!("`:[{key} "))
            || attached_source
                .lines()
                .any(|line| line.trim_start().starts_with(&format!("`: {key} ")))
    };
    let mut completions = Vec::new();
    match (&group.content, owner) {
        (AttachedContent::Blocks(_), AttributeOwner::Document) => {
            push_attached_completion(
                &mut completions,
                !has_pair("title"),
                "title",
                "`: title ",
                "document title",
            );
            push_attached_completion(
                &mut completions,
                !has_pair("created"),
                "created",
                "`: created ",
                "document creation datetime",
            );
            push_attached_completion(
                &mut completions,
                !has_pair("date"),
                "date",
                "`: date ",
                "document date",
            );
            push_attached_completion(
                &mut completions,
                !has_pair("timezone"),
                "timezone",
                "`: timezone ",
                "document timezone",
            );
        }
        (AttachedContent::Blocks(_), AttributeOwner::Marked("task")) => {
            push_attached_completion(&mut completions, !has_id, "id", "`@ ", "explicit id");
            for (key, detail) in task_attribute_pairs() {
                push_attached_pair_completion(&mut completions, !has_pair(key), key, detail, false);
            }
        }
        (AttachedContent::Blocks(_), AttributeOwner::Marked("event")) => {
            push_attached_completion(&mut completions, !has_id, "id", "`@ ", "explicit id");
            for (key, detail) in event_attribute_pairs() {
                push_attached_pair_completion(&mut completions, !has_pair(key), key, detail, false);
            }
        }
        (AttachedContent::Blocks(_), AttributeOwner::VerbatimBlock(_kind)) => {
            push_attached_pair_completion(
                &mut completions,
                !has_pair("language"),
                "language",
                "raw content language",
                false,
            );
        }
        (AttachedContent::Inlines(_), AttributeOwner::ParsedInline("img")) => {
            push_attached_pair_completion(
                &mut completions,
                !has_pair("src"),
                "src",
                "image source",
                true,
            );
        }
        (AttachedContent::Inlines(_), AttributeOwner::VerbatimInline(_kind)) => {
            push_attached_pair_completion(
                &mut completions,
                !has_pair("language"),
                "language",
                "raw content language",
                true,
            );
        }
        _ => {}
    }
    completions.retain(|candidate| {
        let candidate_declaration = match candidate.new_text.as_bytes().get(1) {
            Some(b'-') => '-',
            Some(b'@') => '@',
            Some(b':') => ':',
            _ => return false,
        };
        candidate_declaration == declaration && candidate.label.starts_with(query)
    });
    Some(AttributeCompletionContext {
        replace: introducer..offset,
        completions,
    })
}

fn attached_value_completion_context(
    group: &plumb_syntax::AttachedGroup,
    source: &str,
    offset: usize,
    owner: AttributeOwner<'_>,
) -> Option<AttributeCompletionContext> {
    let introducer = source[group.open_range.end..offset].rfind("`:[")? + group.open_range.end;
    let typed = &source[introducer + 3..offset];
    if typed
        .chars()
        .any(|character| matches!(character, '[' | ']' | '{' | '}'))
    {
        return None;
    }
    let (key, value) = typed.split_once(char::is_whitespace)?;
    let value = value.trim_start();
    if key != "language"
        || !matches!(
            owner,
            AttributeOwner::VerbatimInline("$") | AttributeOwner::VerbatimBlock("$")
        )
        || !"tex".starts_with(value)
    {
        return None;
    }
    let value_start = offset - value.len();
    let value_end = source[offset..group.close_range.start]
        .find(']')
        .map_or(offset, |relative| offset + relative);
    Some(AttributeCompletionContext {
        replace: value_start..value_end,
        completions: vec![AttributeCompletion {
            label: "tex",
            new_text: "tex",
            detail: "standard TeX math language",
        }],
    })
}

fn attached_declaration_query(typed: &str) -> Option<(char, &str)> {
    let mut characters = typed.chars();
    let declaration = characters.next()?;
    if !matches!(declaration, '-' | '@' | ':') {
        return None;
    }
    let remainder = characters.as_str();
    let query = remainder.strip_prefix(' ').unwrap_or(remainder);
    if query.chars().any(char::is_whitespace) {
        return None;
    }
    Some((declaration, query))
}

fn push_attached_completion(
    candidates: &mut Vec<AttributeCompletion>,
    include: bool,
    label: &'static str,
    new_text: &'static str,
    detail: &'static str,
) {
    if include {
        candidates.push(AttributeCompletion {
            label,
            new_text,
            detail,
        });
    }
}

fn push_attached_pair_completion(
    candidates: &mut Vec<AttributeCompletion>,
    include: bool,
    key: &'static str,
    detail: &'static str,
    inline: bool,
) {
    let new_text = match (key, inline) {
        ("created", false) => "`: created ",
        ("due", false) => "`: due ",
        ("wait", false) => "`: wait ",
        ("recur", false) => "`: recur ",
        ("prev", false) => "`: prev ",
        ("depends", false) => "`: depends ",
        ("priority", false) => "`: priority 0",
        ("date", false) => "`: date ",
        ("timezone", false) => "`: timezone ",
        ("tasks", false) => "`: tasks ",
        ("language", false) => "`: language ",
        ("src", true) => "`:[src ]",
        ("language", true) => "`:[language ]",
        _ => return,
    };
    push_attached_completion(candidates, include, key, new_text, detail);
}

fn attribute_context(
    source: &str,
    offset: usize,
    attrs: &Attributes,
    owner: AttributeOwner<'_>,
) -> Option<AttributeCompletionContext> {
    if attrs.attached.is_some() {
        return None;
    }
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
    let mut candidates = Vec::new();
    match owner {
        AttributeOwner::Marked("task") => {
            for (key, detail) in task_attribute_pairs() {
                push_pair_completion(&mut candidates, !existing_pairs(key), key, detail);
            }
        }
        AttributeOwner::Marked("event") => {
            for (key, detail) in event_attribute_pairs() {
                push_pair_completion(&mut candidates, !existing_pairs(key), key, detail);
            }
        }
        AttributeOwner::ParsedInline("img") => push_pair_completion(
            &mut candidates,
            !existing_pairs("src"),
            "src",
            "image source",
        ),
        AttributeOwner::VerbatimInline(_) => {
            push_pair_completion(
                &mut candidates,
                !existing_pairs("language"),
                "language",
                "raw content language",
            );
        }
        AttributeOwner::VerbatimBlock(_) => {
            push_pair_completion(
                &mut candidates,
                !existing_pairs("language"),
                "language",
                "raw content language",
            );
        }
        AttributeOwner::Document | AttributeOwner::Marked(_) | AttributeOwner::ParsedInline(_) => {}
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
        "date" => "date=",
        "timezone" => "timezone=\"\"",
        "tasks" => "tasks=\"\"",
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

fn event_attribute_pairs() -> [(&'static str, &'static str); 3] {
    [
        ("date", "event date override"),
        ("timezone", "event timezone override"),
        ("tasks", "related task references"),
    ]
}

fn value_completions(
    _attrs: &Attributes,
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
            AttributeOwner::VerbatimInline("$") | AttributeOwner::VerbatimBlock("$")
        )
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
    } else if block_position && "task".starts_with(marker_prefix) {
        Some(ConstructCompletionContext::Task { replace })
    } else if block_position && "event".starts_with(marker_prefix) {
        Some(ConstructCompletionContext::Event { replace })
    } else {
        match marker_prefix {
            "-" | "->" => Some(ConstructCompletionContext::LinkAndAutolink { replace }),
            "->[" => Some(ConstructCompletionContext::Link { replace }),
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
        if block
            .mark
            .as_ref()
            .is_some_and(|mark| mark.marker == "event")
            && offset == block.head.range.end
        {
            let [Inline::Text { .. }, Inline::Space { .. }, title @ ..] =
                block.head.items.as_slice()
            else {
                return None;
            };
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
        | Inline::Element { range, .. }
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
            if mark.marker == "task" {
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
    let line_end = source[offset..]
        .find('\n')
        .map_or(source.len(), |index| offset + index);
    let Some(label_end) = matching_slot_close(source, label_start, line_end) else {
        let label_prefix = &source[label_start..offset];
        if label_prefix
            .chars()
            .any(|character| character == '`' || character == ']' || character.is_control())
        {
            return None;
        }
        let suffix = &source[offset..line_end];
        let replace_end = if suffix.starts_with(']') {
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
    if offset <= label_end {
        let label_prefix = &source[label_start..offset];
        return (!label_prefix.chars().any(char::is_control)).then(|| {
            LinkCompletionContext::Label {
                replace: link_start..label_end + 1,
                query: label_prefix.to_string(),
            }
        });
    }

    let (value_start, value_end, parsed) = match source.as_bytes().get(label_end + 1) {
        Some(b'[') => {
            let value_start = label_end + 2;
            let value_end = matching_slot_close(source, value_start, line_end).unwrap_or(offset);
            (value_start, value_end, true)
        }
        Some(b'{') => {
            let attrs_start = label_end + 2;
            let attrs = &source[attrs_start..offset];
            let to = attrs.rfind("`:[to ")?;
            let value_start = attrs_start + to + "`:[to ".len();
            let value_end = source[offset..line_end]
                .find(']')
                .map_or(offset, |end| offset + end);
            (value_start, value_end, true)
        }
        _ => return None,
    };
    if offset < value_start {
        return None;
    }
    let query = &source[value_start..offset];
    if query.contains('"')
        || query.contains('}')
        || query.contains(']')
        || query.chars().any(char::is_control)
    {
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
            parsed,
        })
    }
}

fn matching_slot_close(source: &str, start: usize, limit: usize) -> Option<usize> {
    let mut depth = 1usize;
    for (relative, character) in source[start..limit].char_indices() {
        if !matches!(character, '[' | ']') {
            continue;
        }
        let offset = start + relative;
        let escaped = source[..offset]
            .chars()
            .rev()
            .take_while(|candidate| *candidate == '`')
            .count()
            % 2
            == 1;
        if escaped {
            continue;
        }
        if character == '[' {
            depth += 1;
        } else {
            depth -= 1;
            if depth == 0 {
                return Some(offset);
            }
        }
    }
    None
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
    let (raw_value_start, quoted, attached) = if let Some(src) = attrs.rfind("`:[src ") {
        (after_alt + src + "`:[src ".len(), true, true)
    } else {
        let src = attrs.rfind("src=")? + after_alt;
        if src > after_alt {
            let previous = source[..src].chars().next_back()?;
            if !previous.is_whitespace() && previous != '{' {
                return None;
            }
        }
        let raw_value_start = src + 4;
        (
            raw_value_start,
            source.as_bytes().get(raw_value_start) == Some(&b'"'),
            false,
        )
    };
    let value_start = raw_value_start + usize::from(quoted && !attached);
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
    let value_end = if attached {
        source[offset..]
            .find(']')
            .map_or(offset, |end| offset + end)
    } else if quoted {
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
            kind,
            text_range,
            quote_count,
            attrs,
            ..
        } if text_range.start <= offset && offset <= text_range.end && kind == "->" => {
            let envelope_end = attrs
                .attached
                .as_ref()
                .map_or(range.end, |group| group.range.start);
            component_completion_context(
                source,
                text_range,
                range.start..envelope_end,
                *quote_count,
                offset,
            )
        }
        Inline::Element { slots, .. } => slots
            .iter()
            .find_map(|slot| inlines_find_autolink(source, &slot.content, offset)),
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
            .is_some_and(|range| range.contains(&offset)),
        Block::Parsed(block) => {
            block.mark.as_ref().is_some_and(|mark| {
                mark.attrs
                    .range
                    .as_ref()
                    .is_some_and(|range| range.contains(&offset))
            }) || inlines_attributes_contain(&block.head, offset)
                || blocks_attributes_contain(&block.children, offset)
        }
    })
}

fn inlines_attributes_contain(content: &InlineContent, offset: usize) -> bool {
    content.items.iter().any(|inline| match inline {
        Inline::Element { attrs, slots, .. } => {
            attrs
                .range
                .as_ref()
                .is_some_and(|range| range.contains(&offset))
                || slots
                    .iter()
                    .any(|slot| inlines_attributes_contain(&slot.content, offset))
        }
        Inline::Verbatim { attrs, .. } => attrs
            .range
            .as_ref()
            .is_some_and(|range| range.contains(&offset)),
        Inline::Text { .. } | Inline::Space { .. } | Inline::SoftBreak { .. } => false,
    })
}

fn inlines_contain_verbatim(content: &InlineContent, offset: usize) -> bool {
    content.items.iter().any(|inline| match inline {
        Inline::Verbatim { text_range, .. } => {
            text_range.start <= offset && offset <= text_range.end
        }
        Inline::Element { slots, .. } => slots
            .iter()
            .any(|slot| inlines_contain_verbatim(&slot.content, offset)),
        Inline::Text { .. } | Inline::Space { .. } | Inline::SoftBreak { .. } => false,
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
    use plumb_syntax::parse;

    use super::*;

    #[test]
    fn classifies_construct_completion_by_source_context() {
        let block = parse("`");
        assert_eq!(construct_completion_context(&block, 1), None);
        let nested = parse("  `");
        assert_eq!(construct_completion_context(&nested, 3), None);
        let inline = parse("Text `");
        assert_eq!(construct_completion_context(&inline, 6), None);

        for source in ["`t", "`ta", "`tas", "`task"] {
            let parsed = parse(source);
            assert_eq!(
                construct_completion_context(&parsed, source.len()),
                Some(ConstructCompletionContext::Task {
                    replace: 0..source.len()
                })
            );
        }
        for prefix in ["`t", "`e"] {
            let source =
                format!("`task something {{\n `: created 2026-08-09T10:55:24+08:00\n}}\n{prefix}");
            let parsed = parse(&source);
            let replace = source.len() - prefix.len()..source.len();
            let expected = if prefix == "`t" {
                ConstructCompletionContext::Task { replace }
            } else {
                ConstructCompletionContext::Event { replace }
            };
            assert_eq!(
                construct_completion_context(&parsed, source.len()),
                Some(expected)
            );
        }
        for source in ["`span[x]{`: key value}`-", "`\"raw\"{`: key value}`-"] {
            let parsed = parse(source);
            let start = source.rfind('`').unwrap();
            assert_eq!(
                construct_completion_context(&parsed, source.len()),
                Some(ConstructCompletionContext::LinkAndAutolink {
                    replace: start..source.len()
                })
            );
        }
        let after_verbatim = "`rust\"\n raw\n`t";
        let parsed = parse(after_verbatim);
        assert_eq!(
            construct_completion_context(&parsed, after_verbatim.len()),
            Some(ConstructCompletionContext::Task {
                replace: after_verbatim.len() - 2..after_verbatim.len()
            })
        );
        for source in ["  `e", "  `ev", "  `eve", "  `even", "  `event"] {
            let parsed = parse(source);
            assert_eq!(
                construct_completion_context(&parsed, source.len()),
                Some(ConstructCompletionContext::Event {
                    replace: 2..source.len()
                })
            );
        }

        for source in ["`-", "`->", "Text `-", "Text `->"] {
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
        let attribute = parse("`node Head {\n  `: key ``\n}\n");
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
        let (source, cursor) = strip_cursor("`event 09:00 rela|");
        assert_eq!(
            event_title_completion_context(&parse(&source), cursor),
            Some(EventTitleCompletionContext {
                replace: source.find("rela").unwrap()..cursor,
                query: "rela".to_string(),
            })
        );

        let (empty, cursor) = strip_cursor("`event 09:00 |");
        assert_eq!(
            event_title_completion_context(&parse(&empty), cursor),
            Some(EventTitleCompletionContext {
                replace: cursor..cursor,
                query: String::new(),
            })
        );

        for marked in [
            "`event 09:00|",
            "`event 09:00 `*[relax]|",
            "`event 09:00 re|lax",
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
        let (task, cursor) = strip_cursor("`task Work {\n  `: created now\n  `: pr|\n}\n");
        let context = attribute_completion_context(&parse(&task), cursor).unwrap();
        assert_eq!(context.replace, task.find("`: pr").unwrap()..cursor);
        assert_eq!(
            context
                .completions
                .iter()
                .map(|item| item.label)
                .collect::<Vec<_>>(),
            ["prev", "priority"]
        );
        assert_eq!(context.completions[1].new_text, "`: priority 0");

        let (recovered, cursor) = strip_cursor("`img[Alt]{`: s|");
        let context = attribute_completion_context(&parse(&recovered), cursor).unwrap();
        assert_eq!(context.completions[0].new_text, "`:[src ]");

        let (nested, cursor) = strip_cursor("Text `span[x `img[y]{`: s|}]{`-[outer]}");
        let context = attribute_completion_context(&parse(&nested), cursor).unwrap();
        assert_eq!(context.completions[0].label, "src");
    }

    #[test]
    fn completes_attached_elements_with_the_owners_ordinary_syntax() {
        let (task, cursor) = strip_cursor("`task Work {\n  `: pr|\n}\n");
        let context = attribute_completion_context(&parse(&task), cursor).unwrap();
        assert_eq!(context.replace, task.find("`: pr").unwrap()..cursor);
        assert_eq!(
            context
                .completions
                .iter()
                .map(|item| (item.label, item.new_text))
                .collect::<Vec<_>>(),
            [("prev", "`: prev "), ("priority", "`: priority 0")]
        );

        let (link, cursor) = strip_cursor("`->[label][target]{`: t|}");
        let context = attribute_completion_context(&parse(&link), cursor).unwrap();
        assert!(context.completions.is_empty());

        let (root, cursor) = strip_cursor("{\n  `: ti|\n}\n");
        let context = attribute_completion_context(&parse(&root), cursor).unwrap();
        assert_eq!(context.completions[0].new_text, "`: title ");
    }

    #[test]
    fn identifies_task_dependency_tokens_and_preserves_other_references() {
        let (source, cursor) = strip_cursor(
            "`task Review {\n  `@ review\n  `: depends #done Project Plan.plumb#dr|aft #later\n}\n",
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

        let (empty, cursor) = strip_cursor("`task Review {\n  `: depends #done |\n}\n");
        let context = task_dependency_completion_context(&parse(&empty), cursor).unwrap();
        assert_eq!(context.replace, cursor..cursor);
        assert_eq!(context.query, "");
        assert_eq!(
            context.existing,
            vec![TaskReferenceTarget::Internal {
                id: "done".to_string()
            }]
        );

        let (non_task, cursor) = strip_cursor("`- Plain item {\n  `: depends #dr|aft\n}\n");
        assert_eq!(
            task_dependency_completion_context(&parse(&non_task), cursor),
            None
        );

        let (recovered, cursor) = strip_cursor("`task Review {\n  `: depends #dr|aft\n");
        let context = task_dependency_completion_context(&parse(&recovered), cursor).unwrap();
        assert_eq!(context.query, "#dr");
        assert_eq!(&recovered[context.replace], "#draft");
    }

    #[test]
    fn suppresses_duplicate_attributes_and_completes_enum_values() {
        let (task, cursor) = strip_cursor("`- Work {`-[task] `:[priority 2] `: |}");
        let context = attribute_completion_context(&parse(&task), cursor).unwrap();
        assert!(!context
            .completions
            .iter()
            .any(|item| item.label == "priority"));

        let (math, cursor) = strip_cursor("`$\"x\"{`:[language t|]}\n");
        let context = attribute_completion_context(&parse(&math), cursor).unwrap();
        assert_eq!(context.completions[0].label, "tex");
        assert_eq!(context.completions[0].new_text, "tex");

        let (quoted, cursor) = strip_cursor("`task Work\n{\n  `: due 2026-|\n}\n");
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
        let path = "See `->[x][doc";
        assert_eq!(
            completion_context(path, path.len()),
            Some(LinkCompletionContext::Path {
                replace: 11..14,
                query: "doc".to_string(),
                parsed: true,
            })
        );
        let anchor = "See `->[x][doc.plumb#tar";
        assert_eq!(
            completion_context(anchor, anchor.len()),
            Some(LinkCompletionContext::Anchor {
                path: "doc.plumb".to_string(),
                replace: 21..24,
                query: "tar".to_string(),
            })
        );
    }

    #[test]
    fn replaces_complete_target_components_around_the_cursor() {
        let (path, cursor) = strip_cursor("See `->[x]{`:[to do|c.plumb#target]}\n");
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

        let (anchor, cursor) = strip_cursor("See `->[x]{`:[to doc.plumb#ta|rget]}\n");
        let fragment_start = anchor.find("target").unwrap();
        assert_eq!(
            completion_context(&anchor, cursor),
            Some(LinkCompletionContext::Anchor {
                path: "doc.plumb".to_string(),
                replace: fragment_start..fragment_start + "target".len(),
                query: "ta".to_string(),
            })
        );

        let (empty, cursor) = strip_cursor("See `->[x]{`:[to |]}\n");
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

        let block = "`text\"\n  raw `->[x]{to=\"doc|\"}\n";
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
        let (valid, cursor) = strip_cursor("`img[Alt]{`:[src static/im|age.png]}\n");
        let value_start = valid.find("static/image.png").unwrap();
        assert_eq!(
            image_completion(&valid, cursor),
            Some(ImageCompletionContext {
                replace: value_start..value_start + "static/image.png".len(),
                query: "static/im".to_string(),
                quoted: true,
            })
        );

        let (recovered, cursor) = strip_cursor("`img[Alt]{`:[src static/im|");
        assert_eq!(
            image_completion(&recovered, cursor),
            Some(ImageCompletionContext {
                replace: recovered.find("static/im").unwrap()..cursor,
                query: "static/im".to_string(),
                quoted: true,
            })
        );

        let (external, cursor) = strip_cursor("`img[Alt]{`:[src https:|//example.test/a.png]}\n");
        assert_eq!(image_completion(&external, cursor), None);
    }

    #[test]
    fn completes_file_source_values_without_confusing_images() {
        let (file, cursor) = strip_cursor("`file[Demo]{`:[src static/de|mo.mp4]}\n");
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
