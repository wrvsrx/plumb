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
    pub syntax: Document,
    pub diagnostics: Vec<Diagnostic>,
}

impl ParsedDocument {
    pub fn is_valid(&self) -> bool {
        self.diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity != DiagnosticSeverity::Error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Document {
    pub blocks: Vec<Block>,
    pub range: SourceRange,
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

impl InlineContent {
    pub fn plain_text(&self) -> String {
        let mut output = String::new();
        append_plain_text(&self.items, &mut output);
        output
    }
}

fn append_plain_text(items: &[Inline], output: &mut String) {
    for item in items {
        match item {
            Inline::Text { text, .. } | Inline::Verbatim { text, .. } => output.push_str(text),
            Inline::SoftBreak { .. } => output.push(' '),
            Inline::Element { content, .. } => append_plain_text(&content.items, output),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text {
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
        text: String,
        text_range: SourceRange,
        quote_count: usize,
        attrs: Attributes,
    },
}
