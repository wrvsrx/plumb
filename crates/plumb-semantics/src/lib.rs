mod citations;
mod document;
mod events;
mod headings;
mod inline_styles;
mod lists;
mod math;
mod metadata;
mod queries;
mod quotes;
mod tables;
mod tasks;
mod text;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListItemFacet {
    None,
    Task,
    Event,
    Conflict,
}

pub(crate) fn list_item_facet(block: &plumb_syntax::ParsedBlock) -> ListItemFacet {
    let Some(mark) = &block.mark else {
        return ListItemFacet::None;
    };
    if !matches!(mark.marker.as_str(), "-" | ".") {
        return ListItemFacet::None;
    }
    match (mark.attrs.has_class("task"), mark.attrs.has_class("event")) {
        (false, false) => ListItemFacet::None,
        (true, false) => ListItemFacet::Task,
        (false, true) => ListItemFacet::Event,
        (true, true) => ListItemFacet::Conflict,
    }
}

pub(crate) fn table_structural_item_starts(
    valid: plumb_syntax::ValidDocument<'_>,
) -> std::collections::HashSet<usize> {
    let tables = tables::analyze_tables(valid);
    tables
        .tables
        .iter()
        .flat_map(|table| {
            table.rows.iter().flat_map(|row| {
                std::iter::once(row.range.start)
                    .chain(row.cells.iter().map(|cell| cell.range.start))
            })
        })
        .collect()
}

pub(crate) fn inline_selection_range(
    content: &plumb_syntax::InlineContent,
) -> std::ops::Range<usize> {
    let view = owner_semantic_view(content);
    let Some(first) = view.positional.first() else {
        return content.range.end..content.range.end;
    };
    let mut range = element_selection_range(first);
    if let Some(last) = view.positional.last() {
        range.end = element_selection_range(last).end;
    }
    range
}

pub(crate) fn element_selection_range(
    content: &plumb_syntax::InlineContent,
) -> std::ops::Range<usize> {
    match content.items.as_slice() {
        [plumb_syntax::Inline::Group {
            mark: None,
            content,
            ..
        }] => content.range.clone(),
        [plumb_syntax::Inline::Verbatim {
            mark: None,
            text_range,
            ..
        }] => text_range.clone(),
        _ => content.range.clone(),
    }
}

pub struct OwnerSemanticView<'a> {
    content: &'a plumb_syntax::InlineContent,
    pub positional: Vec<plumb_syntax::InlineContent>,
    item_indices: Vec<usize>,
}

pub struct FirstRestView<'a> {
    pub first: &'a plumb_syntax::InlineContent,
    pub rest: &'a [plumb_syntax::InlineContent],
    content: &'a plumb_syntax::InlineContent,
    rest_indices: &'a [usize],
}

impl FirstRestView<'_> {
    pub fn rest_range(&self) -> Option<std::ops::Range<usize>> {
        Some(self.rest.first()?.range.start..self.rest.last()?.range.end)
    }

    pub fn rest_plain_text(&self) -> String {
        self.rest_content()
            .map_or_else(String::new, |content| text::plain_text(&content))
    }

    pub fn rest_content(&self) -> Option<plumb_syntax::InlineContent> {
        semantic_content_from_indices(self.content, self.rest_indices)
    }

    pub fn rest_has_declarations(&self) -> bool {
        let (Some(first), Some(last)) = (self.rest_indices.first(), self.rest_indices.last())
        else {
            return false;
        };
        self.content.items[*first..=*last]
            .iter()
            .any(is_inline_declaration)
    }

    pub fn rest_declaration_ranges(&self) -> Vec<std::ops::Range<usize>> {
        let (Some(first), Some(last)) = (self.rest_indices.first(), self.rest_indices.last())
        else {
            return Vec::new();
        };
        self.content.items[*first..=*last]
            .iter()
            .filter(|inline| is_inline_declaration(inline))
            .map(|inline| plumb_syntax::inline_range(inline).clone())
            .collect()
    }
}

impl OwnerSemanticView<'_> {
    pub fn split_first(&self) -> Option<FirstRestView<'_>> {
        let (first, rest) = self.positional.split_first()?;
        Some(FirstRestView {
            first,
            rest,
            content: self.content,
            rest_indices: self.item_indices.get(1..).unwrap_or_default(),
        })
    }

    pub fn visible_content(&self) -> Option<plumb_syntax::InlineContent> {
        semantic_content_from_indices(self.content, &self.item_indices)
    }
}

pub fn owner_semantic_view(content: &plumb_syntax::InlineContent) -> OwnerSemanticView<'_> {
    let mut positional = Vec::new();
    let mut item_indices = Vec::new();
    for (index, inline) in content.items.iter().enumerate() {
        if !inline.is_whitespace() && !is_inline_declaration(inline) {
            positional.push(plumb_syntax::InlineContent::from_items(
                plumb_syntax::inline_range(inline).clone(),
                vec![inline.clone()],
            ));
            item_indices.push(index);
        }
    }
    OwnerSemanticView {
        content,
        positional,
        item_indices,
    }
}

pub(crate) fn positional_elements(
    content: &plumb_syntax::InlineContent,
) -> Vec<plumb_syntax::InlineContent> {
    owner_semantic_view(content).positional
}

fn semantic_content_from_indices(
    content: &plumb_syntax::InlineContent,
    indices: &[usize],
) -> Option<plumb_syntax::InlineContent> {
    let (first, last) = (*indices.first()?, *indices.last()?);
    let mut items = Vec::new();
    for inline in &content.items[first..=last] {
        if is_inline_declaration(inline) {
            continue;
        }
        if inline.is_whitespace()
            && items
                .last()
                .is_some_and(plumb_syntax::Inline::is_whitespace)
        {
            continue;
        }
        items.push(inline.clone());
    }
    Some(plumb_syntax::InlineContent::from_items(
        plumb_syntax::inline_range(&content.items[first]).start
            ..plumb_syntax::inline_range(&content.items[last]).end,
        items,
    ))
}

fn is_inline_declaration(inline: &plumb_syntax::Inline) -> bool {
    matches!(
        inline,
        plumb_syntax::Inline::Group {
            mark: Some(mark),
            ..
        } if matches!(mark.marker.as_str(), "@" | "+" | "=")
    )
}

pub fn is_document_declaration(block: &plumb_syntax::Block) -> bool {
    let plumb_syntax::Block::Parsed(block) = block else {
        return false;
    };
    block
        .mark
        .as_ref()
        .is_some_and(|mark| matches!(mark.marker.as_str(), "=" | "+" | "@"))
}

pub fn is_block_declaration(
    _owner: &plumb_syntax::ParsedBlock,
    child: &plumb_syntax::Block,
) -> bool {
    let plumb_syntax::Block::Parsed(child) = child else {
        return false;
    };
    child.children.is_empty()
        && child
            .mark
            .as_ref()
            .is_some_and(|mark| matches!(mark.marker.as_str(), "@" | "+" | "="))
}

pub fn body_children(
    owner: &plumb_syntax::ParsedBlock,
) -> impl DoubleEndedIterator<Item = &plumb_syntax::Block> {
    let association_value = owner.mark.as_ref().is_some_and(|mark| mark.marker == "=");
    owner
        .children
        .iter()
        .filter(move |child| !association_value && !is_block_declaration(owner, child))
}

pub use citations::{analyze_citations, CitationOutput, CitationRecord};
pub use document::{
    analyze_document, AnchorKind, AnchorRecord, DocumentOutput, EventLinkRange, FileRecord,
    FileTarget, ImageRecord, ImageTarget, LinkRecord, LinkSpelling, LinkTarget, SourceBacked,
};
pub use events::{analyze_events, EventField, EventOutput, EventRecord};
pub use headings::{analyze_headings, analyze_recovered_headings, Heading, HeadingOutput};
pub use inline_styles::{
    analyze_inline_styles, InlineStyleKind, InlineStyleOutput, InlineStyleRecord,
};
pub use lists::{analyze_lists, ListGroup, ListItemRecord, ListKind, ListOutput};
pub use math::{analyze_math, MathKind, MathOutput, MathRecord};
pub use metadata::{
    analyze_metadata, recovered_bibliography_sources, BibliographySource, DefinitionList,
    DefinitionRecord, MetadataBlock, MetadataEntry, MetadataListItem, MetadataOutput,
    MetadataValue,
};
pub use queries::{
    attribute_completion_context, citation_completion_context, construct_completion_context,
    event_title_completion_context, file_completion_context, image_completion_context,
    link_completion_context, task_dependency_completion_context, AttributeCompletion,
    AttributeCompletionContext, CitationCompletionContext, ConstructCompletionContext,
    EventTitleCompletionContext, FileCompletionContext, ImageCompletionContext,
    LinkCompletionContext, TaskDependencyCompletionContext,
};
pub use quotes::{analyze_quotes, QuoteOutput, QuoteRecord};
pub use tables::{analyze_tables, TableCellRecord, TableOutput, TableRecord, TableRowRecord};
pub use tasks::{
    analyze_tasks, next_task_datetime, parse_task_reference_target, valid_task_datetime,
    TaskDependency, TaskField, TaskOutput, TaskRecord, TaskReferenceTarget, TaskState, TaskStatus,
};
pub use text::plain_text as semantic_plain_text;
