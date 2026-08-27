mod lossless;
mod parser;
mod syntax;

pub use parser::parse;
pub use syntax::{
    AttrItem, AttrValue, Attributes, Block, Diagnostic, DiagnosticSeverity, Document, Inline,
    InlineArgument, InlineArgumentRef, InlineContent, InlineMember, LosslessTree, Mark,
    ParsedBlock, ParsedDocument, RawPayload, SourceRange, SyntaxKind, SyntaxToken, ValidDocument,
    VerbatimArgument, VerbatimBlock,
};
