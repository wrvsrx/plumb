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

#[cfg(test)]
fn parse_legacy(source: impl Into<String>) -> plumb_syntax::ParsedDocument {
    let source = source.into();
    let migrated = plumb_migrate::migrate_member_envelope_v1(&source)
        .unwrap_or_else(|error| panic!("cannot migrate semantic fixture: {error}"));
    plumb_syntax::parse(migrated)
}

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
    let mut normalized = content
        .data
        .iter()
        .enumerate()
        .filter_map(|(index, _)| content.argument(index))
        .filter(|argument| !argument.items.is_empty());
    let Some(first) = normalized.next() else {
        return content.range.end..content.range.end;
    };
    let mut range = datum_selection_range(&first);
    for argument in normalized {
        range.end = datum_selection_range(&argument).end;
    }
    range
}

pub(crate) fn datum_selection_range(
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

pub(crate) struct OwnerSemanticView {
    pub positional: Vec<plumb_syntax::InlineContent>,
}

pub(crate) struct FirstRestView<'a> {
    pub first: &'a plumb_syntax::InlineContent,
    pub rest: &'a [plumb_syntax::InlineContent],
}

impl FirstRestView<'_> {
    pub fn rest_range(&self) -> Option<std::ops::Range<usize>> {
        Some(self.rest.first()?.range.start..self.rest.last()?.range.end)
    }

    pub fn rest_plain_text(&self) -> String {
        let mut output = String::new();
        for (index, datum) in self.rest.iter().enumerate() {
            if index > 0 {
                output.push(' ');
            }
            output.push_str(&text::plain_text(datum));
        }
        output
    }
}

impl OwnerSemanticView {
    pub fn split_first(&self) -> Option<FirstRestView<'_>> {
        let (first, rest) = self.positional.split_first()?;
        Some(FirstRestView { first, rest })
    }
}

pub(crate) fn owner_semantic_view(content: &plumb_syntax::InlineContent) -> OwnerSemanticView {
    let mut positional = Vec::new();
    for (index, datum) in content.data.iter().enumerate() {
        let items = &content.items[datum.item_range.clone()];
        let declaration = matches!(
            items,
            [plumb_syntax::Inline::Group {
                mark: Some(mark),
                ..
            }] if matches!(mark.marker.as_str(), "@" | "+" | "=")
        );
        if !declaration {
            if let Some(datum) = content.datum(index) {
                positional.push(datum);
            }
        }
    }
    OwnerSemanticView { positional }
}

pub(crate) fn positional_data(
    content: &plumb_syntax::InlineContent,
) -> Vec<plumb_syntax::InlineContent> {
    owner_semantic_view(content).positional
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
