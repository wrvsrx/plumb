mod document;
mod headings;

pub use document::{
    analyze_document, AnchorKind, AnchorRecord, DocumentOutput, LinkRecord, LinkTarget,
    SourceBacked,
};
pub use headings::{analyze_headings, Heading, HeadingOutput};
