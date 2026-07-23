mod citations;
mod document;
mod headings;
mod lists;
mod math;
mod metadata;
mod queries;
mod quotes;
mod tasks;

pub use citations::{analyze_citations, CitationOutput, CitationRecord};
pub use document::{
    analyze_document, AnchorKind, AnchorRecord, DocumentOutput, ImageRecord, ImageTarget,
    LinkRecord, LinkSpelling, LinkTarget, SourceBacked,
};
pub use headings::{analyze_headings, Heading, HeadingOutput};
pub use lists::{analyze_lists, ListGroup, ListItemRecord, ListKind, ListOutput};
pub use math::{analyze_math, MathKind, MathOutput, MathRecord};
pub use metadata::{
    analyze_metadata, DefinitionList, DefinitionRecord, MetadataBlock, MetadataEntry,
    MetadataListItem, MetadataOutput, MetadataValue,
};
pub use queries::{
    construct_completion_context, image_completion_context, link_completion_context,
    ConstructCompletionContext, ImageCompletionContext, LinkCompletionContext,
};
pub use quotes::{analyze_quotes, QuoteOutput, QuoteRecord};
pub use tasks::{
    analyze_tasks, next_task_datetime, parse_task_reference_target, valid_task_datetime,
    TaskDependency, TaskField, TaskOutput, TaskRecord, TaskReferenceTarget, TaskState, TaskStatus,
};
