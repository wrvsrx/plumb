mod lossless;
mod parser;
mod syntax;

pub use parser::{parse, parse_legacy_017};
pub use syntax::{
    AttachedContent, AttachedGroup, AttrItem, AttrValue, Attributes, Block, Diagnostic,
    DiagnosticSeverity, Document, Inline, InlineContent, LosslessTree, Mark, ParsedBlock,
    ParsedDocument, SourceRange, SyntaxKind, SyntaxToken, VerbatimBlock,
};
