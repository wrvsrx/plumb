mod parser;
mod syntax;

pub use parser::parse;
pub use syntax::{
    AttrItem, AttrValue, Attributes, Block, CodeBlock, Diagnostic, DiagnosticSeverity, Document,
    Inline, InlineContent, Mark, ParsedBlock, ParsedDocument, SourceRange,
};
