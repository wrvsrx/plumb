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
mod tasks;
mod text;

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
    owner: &plumb_syntax::ParsedBlock,
    child: &plumb_syntax::Block,
) -> bool {
    let Some(mark) = &owner.mark else {
        return false;
    };
    mark.attrs.items.iter().any(|item| {
        let range = match item {
            plumb_syntax::AttrItem::Id { range, .. }
            | plumb_syntax::AttrItem::Class { range, .. }
            | plumb_syntax::AttrItem::Pair { range, .. } => range,
        };
        range == child.range()
    })
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
    analyze_document, AnchorKind, AnchorRecord, DocumentOutput, FileRecord, FileTarget,
    ImageRecord, ImageTarget, LinkRecord, LinkSpelling, LinkTarget, SourceBacked,
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
pub use tasks::{
    analyze_tasks, next_task_datetime, parse_task_reference_target, valid_task_datetime,
    TaskDependency, TaskField, TaskOutput, TaskRecord, TaskReferenceTarget, TaskState, TaskStatus,
};
