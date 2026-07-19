mod document;
mod headings;
mod metadata;
mod queries;

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
