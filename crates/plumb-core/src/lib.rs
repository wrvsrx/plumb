mod parser;
mod syntax;

pub use parser::parse;
pub use syntax::{
    AttrItem, AttrValue, Attributes, Block, Diagnostic, DiagnosticSeverity, Document, Inline,
    InlineContent, Mark, ParsedBlock, ParsedDocument, SourceRange, VerbatimBlock,
};
