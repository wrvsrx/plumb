use serde::{Deserialize, Serialize};

pub type SourceRange = std::ops::Range<usize>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Hint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: &'static str,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub range: SourceRange,
    pub related: Vec<SourceRange>,
}

impl Diagnostic {
    pub(crate) fn error(
        code: &'static str,
        message: impl Into<String>,
        range: SourceRange,
    ) -> Self {
        Self {
            code,
            severity: DiagnosticSeverity::Error,
            message: message.into(),
            range,
            related: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedDocument {
    pub source: String,
    pub lossless: LosslessTree,
    pub syntax: Document,
    pub diagnostics: Vec<Diagnostic>,
}

impl ParsedDocument {
    pub fn is_valid(&self) -> bool {
        self.diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity != DiagnosticSeverity::Error)
    }

    pub fn recovered_syntax(&self) -> &Document {
        &self.syntax
    }

    pub fn valid_syntax(&self) -> Option<&Document> {
        self.is_valid().then_some(&self.syntax)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyntaxKind {
    Text,
    Whitespace,
    Indentation,
    LineEnding,
    Introducer,
    Escape,
    Marker,
    InlineKind,
    Delimiter,
    AttributePunctuation,
    AttributeName,
    AttributeValue,
    AttributeEscape,
    RawPayload,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyntaxToken {
    pub kind: SyntaxKind,
    pub range: SourceRange,
}

impl SyntaxToken {
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.range.clone()]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LosslessTree {
    pub range: SourceRange,
    pub tokens: Vec<SyntaxToken>,
}

impl LosslessTree {
    pub fn reconstruct(&self, source: &str) -> String {
        let mut output = String::with_capacity(source.len());
        for token in &self.tokens {
            output.push_str(token.text(source));
        }
        output
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Document {
    pub attrs: Attributes,
    pub blocks: Vec<Block>,
    pub range: SourceRange,
}

impl Drop for Document {
    fn drop(&mut self) {
        let mut pending = std::mem::take(&mut self.blocks);
        if let Some(attached) = self.attrs.attached.take() {
            attached.append_blocks_to(&mut pending);
        }
        while let Some(block) = pending.pop() {
            if let Block::Parsed(mut block) = block {
                if let Some(mark) = &mut block.mark {
                    if let Some(attached) = mark.attrs.attached.take() {
                        attached.append_blocks_to(&mut pending);
                    }
                }
                pending.append(&mut block.children);
            } else if let Block::Verbatim(mut block) = block {
                if let Some(attached) = block.attrs.attached.take() {
                    attached.append_blocks_to(&mut pending);
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Parsed(ParsedBlock),
    Verbatim(VerbatimBlock),
}

impl Block {
    pub fn range(&self) -> &SourceRange {
        match self {
            Self::Parsed(block) => &block.range,
            Self::Verbatim(block) => &block.range,
        }
    }

    pub fn children(&self) -> &[Block] {
        match self {
            Self::Parsed(block) => &block.children,
            Self::Verbatim(_) => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedBlock {
    pub range: SourceRange,
    pub mark: Option<Mark>,
    pub head: InlineContent,
    pub children: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerbatimBlock {
    pub range: SourceRange,
    pub opener_range: SourceRange,
    pub kind: String,
    pub kind_range: SourceRange,
    pub attrs: Attributes,
    pub text: String,
    pub text_range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    pub range: SourceRange,
    pub marker: String,
    pub marker_range: SourceRange,
    pub attrs: Attributes,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Attributes {
    pub range: Option<SourceRange>,
    pub items: Vec<AttrItem>,
    pub attached: Option<Box<AttachedGroup>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedGroup {
    pub range: SourceRange,
    pub open_range: SourceRange,
    pub close_range: SourceRange,
    pub content: AttachedContent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachedContent {
    Blocks(Vec<Block>),
    Inlines(InlineContent),
}

impl AttachedGroup {
    fn append_blocks_to(mut self: Box<Self>, output: &mut Vec<Block>) {
        if let AttachedContent::Blocks(blocks) = &mut self.content {
            output.append(blocks);
        }
    }

    fn append_inlines_to(mut self: Box<Self>, output: &mut Vec<Inline>) {
        if let AttachedContent::Inlines(content) = &mut self.content {
            output.append(&mut content.items);
        }
    }
}

impl Attributes {
    pub fn id(&self) -> Option<&str> {
        self.items.iter().find_map(|item| match item {
            AttrItem::Id { value, .. } => Some(value.as_str()),
            _ => None,
        })
    }

    pub fn value(&self, key: &str) -> Option<&str> {
        self.items.iter().find_map(|item| match item {
            AttrItem::Pair {
                key: candidate,
                value,
                ..
            } if candidate == key => Some(value.decoded.as_str()),
            _ => None,
        })
    }

    pub fn has_class(&self, class: &str) -> bool {
        self.items
            .iter()
            .any(|item| matches!(item, AttrItem::Class { value, .. } if value == class))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttrItem {
    Id {
        value: String,
        range: SourceRange,
    },
    Class {
        value: String,
        range: SourceRange,
    },
    Pair {
        key: String,
        key_range: SourceRange,
        value: AttrValue,
        range: SourceRange,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttrValue {
    pub decoded: String,
    pub raw: String,
    pub range: SourceRange,
    pub quoted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InlineContent {
    pub range: SourceRange,
    pub items: Vec<Inline>,
}

impl Drop for InlineContent {
    fn drop(&mut self) {
        let mut pending = std::mem::take(&mut self.items);
        while let Some(inline) = pending.pop() {
            match inline {
                Inline::Element {
                    mut content,
                    mut attrs,
                    ..
                } => {
                    pending.append(&mut content.items);
                    if let Some(attached) = attrs.attached.take() {
                        attached.append_inlines_to(&mut pending);
                    }
                }
                Inline::Verbatim { mut attrs, .. } => {
                    if let Some(attached) = attrs.attached.take() {
                        attached.append_inlines_to(&mut pending);
                    }
                }
                Inline::Text { .. } | Inline::Space { .. } | Inline::SoftBreak { .. } => {}
            }
        }
    }
}

impl InlineContent {
    pub fn plain_text(&self) -> String {
        let mut output = String::new();
        append_plain_text(&self.items, &mut output);
        output
    }
}

fn append_plain_text(items: &[Inline], output: &mut String) {
    let mut stack = vec![(items, 0usize)];
    while let Some((items, index)) = stack.pop() {
        if index >= items.len() {
            continue;
        }
        stack.push((items, index + 1));
        match &items[index] {
            Inline::Text { text, .. } | Inline::Verbatim { text, .. } => output.push_str(text),
            Inline::Space { text, .. } => output.push_str(text),
            Inline::SoftBreak { .. } => output.push(' '),
            Inline::Element { content, .. } => stack.push((&content.items, 0)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text {
        text: String,
        range: SourceRange,
    },
    Space {
        text: String,
        range: SourceRange,
    },
    SoftBreak {
        range: SourceRange,
    },
    Element {
        range: SourceRange,
        kind: String,
        kind_range: SourceRange,
        content: InlineContent,
        attrs: Attributes,
    },
    Verbatim {
        range: SourceRange,
        kind: String,
        kind_range: SourceRange,
        text: String,
        text_range: SourceRange,
        quote_count: usize,
        bracketed: bool,
        attrs: Attributes,
    },
}
