mod citations;
mod document;
mod headings;
mod metadata;
mod queries;
mod tasks;

pub use citations::{analyze_citations, CitationOutput, CitationRecord};
pub use document::{
    analyze_document, AnchorKind, AnchorRecord, DocumentOutput, LinkRecord, LinkTarget,
    SourceBacked,
};
pub use headings::{analyze_headings, Heading, HeadingOutput};
pub use metadata::{
    analyze_metadata, DefinitionList, DefinitionRecord, MetadataBlock, MetadataEntry,
    MetadataListItem, MetadataOutput, MetadataValue,
};
pub use queries::{link_completion_context, LinkCompletionContext};
pub use tasks::{
    analyze_tasks, parse_task_reference_target, TaskDependency, TaskField, TaskOutput, TaskRecord,
    TaskReferenceTarget, TaskState,
};
