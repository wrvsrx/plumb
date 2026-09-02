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

    pub fn valid_syntax(&self) -> Option<ValidDocument<'_>> {
        self.is_valid().then_some(ValidDocument { parsed: self })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidDocument<'a> {
    parsed: &'a ParsedDocument,
}

impl<'a> ValidDocument<'a> {
    pub fn source(self) -> &'a str {
        &self.parsed.source
    }

    pub fn syntax(self) -> &'a Document {
        &self.parsed.syntax
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
        while let Some(block) = pending.pop() {
            if let Block::Parsed(mut block) = block {
                pending.append(&mut block.children);
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

    pub fn marker(&self) -> Option<&Mark> {
        match self {
            Self::Parsed(block) => block.mark.as_ref(),
            Self::Verbatim(block) => block.mark.as_ref(),
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
    pub content: InlineContent,
    pub children: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerbatimBlock {
    pub range: SourceRange,
    pub opener_range: SourceRange,
    pub mark: Option<Mark>,
    pub quote_range: SourceRange,
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
        value_range: SourceRange,
        range: SourceRange,
    },
    Class {
        value: String,
        value_range: SourceRange,
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
            if let Inline::Group { mut content, .. } = inline {
                pending.append(&mut content.items);
            }
        }
    }
}

impl InlineContent {
    pub fn from_items(range: SourceRange, items: Vec<Inline>) -> Self {
        Self { range, items }
    }

    pub fn is_empty(&self) -> bool {
        self.positional_elements().next().is_none()
    }

    pub fn positional_elements(&self) -> impl DoubleEndedIterator<Item = &Inline> {
        self.items.iter().filter(|inline| !inline.is_whitespace())
    }

    pub fn plain_text(&self) -> String {
        let mut output = String::new();
        let trimmed = self.trim_boundary_padding();
        append_plain_text(&trimmed.items, &mut output);
        output
    }

    pub fn trim_boundary_padding(&self) -> InlineContent {
        let mut start = 0;
        let mut end = self.items.len();
        while start < end
            && matches!(
                self.items[start],
                Inline::Space { .. } | Inline::SoftBreak { .. }
            )
        {
            start += 1;
        }
        while end > start
            && matches!(
                self.items[end - 1],
                Inline::Space { .. } | Inline::SoftBreak { .. }
            )
        {
            end -= 1;
        }
        let items = self.items[start..end].to_vec();
        let range = match (items.first(), items.last()) {
            (Some(first), Some(last)) => inline_range(first).start..inline_range(last).end,
            _ => self.range.end..self.range.end,
        };
        InlineContent::from_items(range, items)
    }
}

pub fn inline_range(inline: &Inline) -> &SourceRange {
    match inline {
        Inline::Text { range, .. }
        | Inline::Space { range, .. }
        | Inline::SoftBreak { range }
        | Inline::Group { range, .. }
        | Inline::Verbatim { range, .. } => range,
    }
}

fn append_plain_text(items: &[Inline], output: &mut String) {
    let mut stack = vec![(items, 0)];
    while let Some((items, index)) = stack.pop() {
        if index >= items.len() {
            continue;
        }
        stack.push((items, index + 1));
        match &items[index] {
            Inline::Text { text, .. }
            | Inline::Space { text, .. }
            | Inline::Verbatim { text, .. } => output.push_str(text),
            Inline::SoftBreak { .. } => output.push(' '),
            Inline::Group { content, .. } => stack.push((&content.items, 0)),
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
    Group {
        range: SourceRange,
        mark: Option<Mark>,
        content: InlineContent,
    },
    Verbatim {
        range: SourceRange,
        mark: Option<Mark>,
        text: String,
        text_range: SourceRange,
        quote_count: usize,
        braced: bool,
    },
}

impl Inline {
    pub fn is_whitespace(&self) -> bool {
        matches!(self, Self::Space { .. } | Self::SoftBreak { .. })
    }
}
