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
    pub raw: Option<RawPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPayload {
    pub range: SourceRange,
    pub boundary_range: SourceRange,
    pub quote_count: usize,
    pub text: String,
    pub text_range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerbatimBlock {
    pub range: SourceRange,
    pub opener_range: SourceRange,
    /// Opaque verbatim kind; an empty string is the anonymous form (§10
    /// makes the kind optional).
    pub kind: String,
    pub kind_range: SourceRange,
    pub quote_count: usize,
    pub text: String,
    pub text_range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    pub range: SourceRange,
    /// Nonempty in valid trees (§3 requires a nonempty marker token); an
    /// empty string appears only in the invalid-marker recovery placeholder.
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
    pub arguments: Vec<InlineContentArgument>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineContentArgument {
    pub range: SourceRange,
    pub separator_range: Option<SourceRange>,
    pub item_range: std::ops::Range<usize>,
}

impl Drop for InlineContent {
    fn drop(&mut self) {
        let mut pending = std::mem::take(&mut self.items);
        while let Some(inline) = pending.pop() {
            match inline {
                Inline::Element { members, .. } => {
                    for member in members {
                        match member {
                            InlineMember::ParsedArgument(mut argument) => {
                                pending.append(&mut argument.content.items);
                            }
                            InlineMember::VerbatimArgument(_) => {}
                            InlineMember::Child { inline, .. } => pending.push(*inline),
                        }
                    }
                }
                Inline::Verbatim { .. } => {}
                Inline::Text { .. } | Inline::Space { .. } | Inline::SoftBreak { .. } => {}
            }
        }
    }
}

impl InlineContent {
    pub fn from_items(range: SourceRange, items: Vec<Inline>) -> Self {
        let item_count = items.len();
        Self {
            arguments: vec![InlineContentArgument {
                range: range.clone(),
                separator_range: None,
                item_range: 0..item_count,
            }],
            range,
            items,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.arguments.iter().enumerate().all(|(index, _)| {
            self.argument(index)
                .is_none_or(|argument| argument.items.is_empty())
        })
    }

    pub fn argument(&self, index: usize) -> Option<InlineContent> {
        let argument = self.arguments.get(index)?;
        Some(trim_argument_content(
            &self.items[argument.item_range.clone()],
            argument.range.clone(),
        ))
    }

    pub fn argument_plain_text(&self, index: usize) -> Option<String> {
        Some(self.argument(index)?.plain_text())
    }

    pub fn plain_text(&self) -> String {
        let mut output = String::new();
        for index in 0..self.arguments.len() {
            if let Some(argument) = self.argument(index) {
                append_plain_text(&argument.items, &mut output);
            }
        }
        output
    }

    pub fn trim_boundary_padding(&self) -> InlineContent {
        trim_argument_content(&self.items, self.range.clone())
    }
}

fn trim_argument_content(items: &[Inline], source_range: SourceRange) -> InlineContent {
    let mut items = items.to_vec();

    while let Some(Inline::Space { text, range }) = items.first_mut() {
        let trimmed = text.trim_start_matches(' ');
        let removed = text.len() - trimmed.len();
        if removed == 0 {
            break;
        }
        range.start += removed;
        *text = trimmed.to_string();
        if text.is_empty() {
            items.remove(0);
        } else {
            break;
        }
    }

    while let Some(Inline::Space { text, range }) = items.last_mut() {
        let trimmed = text.trim_end_matches(' ');
        let removed = text.len() - trimmed.len();
        if removed == 0 {
            break;
        }
        range.end -= removed;
        *text = trimmed.to_string();
        if text.is_empty() {
            items.pop();
        } else {
            break;
        }
    }

    let range = match (items.first(), items.last()) {
        (Some(first), Some(last)) => inline_range(first).start..inline_range(last).end,
        _ => source_range.end..source_range.end,
    };
    InlineContent::from_items(range, items)
}

fn inline_range(inline: &Inline) -> &SourceRange {
    match inline {
        Inline::Text { range, .. }
        | Inline::Space { range, .. }
        | Inline::SoftBreak { range }
        | Inline::Element { range, .. }
        | Inline::Verbatim { range, .. } => range,
    }
}

fn append_plain_text(items: &[Inline], output: &mut String) {
    enum Frame<'a> {
        Inlines(&'a [Inline], usize),
        Members(&'a [InlineMember], usize),
        OwnedInline(Inline),
        OwnedText(String),
    }

    let mut stack = vec![Frame::Inlines(items, 0)];
    while let Some(frame) = stack.pop() {
        match frame {
            Frame::Inlines(items, index) => {
                if index >= items.len() {
                    continue;
                }
                stack.push(Frame::Inlines(items, index + 1));
                match &items[index] {
                    Inline::Text { text, .. } | Inline::Verbatim { text, .. } => {
                        output.push_str(text);
                    }
                    Inline::Space { text, .. } => output.push_str(text),
                    Inline::SoftBreak { .. } => output.push(' '),
                    Inline::Element { members, .. } => {
                        stack.push(Frame::Members(members, 0));
                    }
                }
            }
            Frame::Members(members, index) => {
                if index >= members.len() {
                    continue;
                }
                stack.push(Frame::Members(members, index + 1));
                match &members[index] {
                    InlineMember::ParsedArgument(argument) => {
                        let mut trimmed = argument.content.trim_boundary_padding();
                        for inline in std::mem::take(&mut trimmed.items).into_iter().rev() {
                            stack.push(Frame::OwnedInline(inline));
                        }
                    }
                    InlineMember::VerbatimArgument(argument) => {
                        output.push_str(&argument.text);
                    }
                    InlineMember::Child { inline, .. } => {
                        stack.push(Frame::Inlines(std::slice::from_ref(inline.as_ref()), 0));
                    }
                }
            }
            Frame::OwnedInline(inline) => match inline {
                Inline::Text { text, .. }
                | Inline::Space { text, .. }
                | Inline::Verbatim { text, .. } => output.push_str(&text),
                Inline::SoftBreak { .. } => output.push(' '),
                Inline::Element { members, .. } => {
                    for member in members.into_iter().rev() {
                        match member {
                            InlineMember::ParsedArgument(argument) => {
                                let mut trimmed = argument.content.trim_boundary_padding();
                                for inline in std::mem::take(&mut trimmed.items).into_iter().rev() {
                                    stack.push(Frame::OwnedInline(inline));
                                }
                            }
                            InlineMember::VerbatimArgument(argument) => {
                                stack.push(Frame::OwnedText(argument.text));
                            }
                            InlineMember::Child { inline, .. } => {
                                stack.push(Frame::OwnedInline(*inline));
                            }
                        }
                    }
                }
            },
            Frame::OwnedText(text) => output.push_str(&text),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineArgument {
    pub range: SourceRange,
    pub separator_range: Option<SourceRange>,
    pub content: InlineContent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerbatimArgument {
    pub range: SourceRange,
    pub separator_range: Option<SourceRange>,
    pub leading_padding: Option<InlinePadding>,
    pub trailing_padding: Option<InlinePadding>,
    pub text: String,
    pub text_range: SourceRange,
    pub quote_count: usize,
    pub bracketed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlinePadding {
    pub text: String,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InlineMember {
    ParsedArgument(InlineArgument),
    VerbatimArgument(VerbatimArgument),
    Child {
        range: SourceRange,
        separator_range: SourceRange,
        leading_padding: Option<InlinePadding>,
        trailing_padding: Option<InlinePadding>,
        inline: Box<Inline>,
    },
}

impl InlineMember {
    pub fn range(&self) -> &SourceRange {
        match self {
            Self::ParsedArgument(argument) => &argument.range,
            Self::VerbatimArgument(argument) => &argument.range,
            Self::Child { range, .. } => range,
        }
    }

    pub fn argument(&self) -> Option<InlineArgumentRef<'_>> {
        match self {
            Self::ParsedArgument(argument) => Some(InlineArgumentRef::Parsed(&argument.content)),
            Self::VerbatimArgument(argument) => Some(InlineArgumentRef::Verbatim(argument)),
            Self::Child { .. } => None,
        }
    }

    pub fn child(&self) -> Option<&Inline> {
        match self {
            Self::Child { inline, .. } => Some(inline),
            Self::ParsedArgument(_) | Self::VerbatimArgument(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineArgumentRef<'a> {
    Parsed(&'a InlineContent),
    Verbatim(&'a VerbatimArgument),
}

impl InlineArgumentRef<'_> {
    pub fn range(&self) -> SourceRange {
        match self {
            Self::Parsed(content) => content.trim_boundary_padding().range.clone(),
            Self::Verbatim(argument) => argument.text_range.clone(),
        }
    }

    pub fn plain_text(&self) -> String {
        match self {
            Self::Parsed(content) => content.trim_boundary_padding().plain_text(),
            Self::Verbatim(argument) => argument.text.clone(),
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
        /// Nonempty (§8 forbids anonymous elements; the introducer-plus-
        /// bracket spelling is a literal escape).
        kind: String,
        kind_range: SourceRange,
        members: Vec<InlineMember>,
        attrs: Attributes,
    },
    Verbatim {
        range: SourceRange,
        /// Opaque kind; empty is the anonymous inline verbatim (§9).
        kind: String,
        kind_range: SourceRange,
        text: String,
        text_range: SourceRange,
        quote_count: usize,
        bracketed: bool,
        attrs: Attributes,
    },
}
