mod lossless;
mod parser;
mod syntax;

pub use parser::parse;
pub use syntax::{
    inline_range, AttrItem, AttrValue, Attributes, Block, Diagnostic, DiagnosticSeverity, Document,
    Inline, InlineContent, InlineDatum, LosslessTree, Mark, ParsedBlock, ParsedDocument,
    SourceRange, SyntaxKind, SyntaxToken, ValidDocument, VerbatimBlock,
};
