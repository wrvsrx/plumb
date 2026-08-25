mod lossless;
mod parser;
mod syntax;

pub use parser::parse;
pub use syntax::{
    AttachedContent, AttachedGroup, AttrItem, AttrValue, Attributes, Block, Diagnostic,
    DiagnosticSeverity, Document, Inline, InlineArgument, InlineArgumentRef, InlineContent,
    InlineMember, LosslessTree, Mark, ParsedBlock, ParsedDocument, SourceRange, SyntaxKind,
    SyntaxToken, VerbatimArgument, VerbatimBlock,
};
