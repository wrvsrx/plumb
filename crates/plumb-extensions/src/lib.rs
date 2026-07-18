mod document;
mod headings;
mod queries;

pub use document::{
    analyze_document, AnchorKind, AnchorRecord, DocumentOutput, LinkRecord, LinkTarget,
    MetadataRecord, SourceBacked,
};
pub use headings::{analyze_headings, Heading, HeadingOutput};
pub use queries::{link_completion_context, LinkCompletionContext};
