use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

use plumb_syntax::{
    AttrItem, AttrValue, Attributes, Block, Diagnostic, DiagnosticSeverity, Document, Inline,
    InlineArgumentRef, InlineContent, InlineMember, ValidDocument,
};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    analyze_citations, analyze_events, analyze_headings, analyze_inline_styles, analyze_lists,
    analyze_math, analyze_metadata, analyze_quotes, analyze_tasks, CitationOutput, EventOutput,
    HeadingOutput, InlineStyleOutput, ListOutput, MathOutput, MetadataOutput, QuoteOutput,
    TaskOutput,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceBacked<T> {
    pub value: T,
    pub raw: String,
    pub range: Range<usize>,
    decoded_boundaries: Vec<usize>,
}

impl SourceBacked<String> {
    pub fn source_range(&self, decoded: Range<usize>) -> Option<Range<usize>> {
        Some(
            *self.decoded_boundaries.get(decoded.start)?
                ..*self.decoded_boundaries.get(decoded.end)?,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnchorKind {
    Heading,
    Block,
    Inline,
    VerbatimBlock,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorRecord {
    pub id: SourceBacked<String>,
    pub kind: AnchorKind,
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkTarget {
    Anchor {
        path: Option<String>,
        fragment: String,
    },
    Document {
        path: String,
    },
    External,
    File {
        path: String,
    },
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkSpelling {
    Positional,
    Verbatim {
        envelope: Range<usize>,
        quote_count: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkRecord {
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
    pub target: SourceBacked<String>,
    pub target_kind: LinkTarget,
    pub spelling: LinkSpelling,
    pub path_range: Option<Range<usize>>,
    pub fragment_range: Option<Range<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageTarget {
    External,
    File { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRecord {
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
    pub source: SourceBacked<String>,
    pub target_kind: ImageTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileTarget {
    External,
    File { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecord {
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
    pub source: SourceBacked<String>,
    pub target_kind: FileTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DocumentOutput {
    pub headings: HeadingOutput,
    pub metadata: MetadataOutput,
    pub citations: CitationOutput,
    pub inline_styles: InlineStyleOutput,
    pub lists: ListOutput,
    pub math: MathOutput,
    pub quotes: QuoteOutput,
    pub tasks: TaskOutput,
    pub events: EventOutput,
    pub anchors: Vec<AnchorRecord>,
    pub links: Vec<LinkRecord>,
    pub images: Vec<ImageRecord>,
    pub files: Vec<FileRecord>,
    pub diagnostics: Vec<Diagnostic>,
}

impl DocumentOutput {
    pub fn link_at_node_start(&self, start: usize) -> Option<&LinkRecord> {
        self.links.iter().find(|link| link.range.start == start)
    }

    pub fn image_at_node_start(&self, start: usize) -> Option<&ImageRecord> {
        self.images.iter().find(|image| image.range.start == start)
    }

    pub fn file_at_node_start(&self, start: usize) -> Option<&FileRecord> {
        self.files.iter().find(|file| file.range.start == start)
    }
}

pub fn analyze_document(valid: ValidDocument<'_>) -> DocumentOutput {
    let source = valid.source();
    let document = valid.syntax();
    let headings = analyze_headings(valid);
    let metadata = analyze_metadata(valid);
    let citations = analyze_citations(valid);
    let inline_styles = analyze_inline_styles(valid);
    let lists = analyze_lists(valid);
    let math = analyze_math(valid);
    let quotes = analyze_quotes(valid);
    let tasks = analyze_tasks(valid);
    let events = analyze_events(valid, &metadata);
    let mut output = DocumentOutput {
        headings,
        metadata,
        citations,
        inline_styles,
        lists,
        math,
        quotes,
        tasks,
        events,
        ..DocumentOutput::default()
    };
    output
        .diagnostics
        .extend(association_arity_diagnostics(document));
    let mut first_ids: HashMap<String, Range<usize>> = HashMap::new();
    collect_blocks(source, &document.blocks, &mut first_ids, &mut output);
    output
}

fn association_arity_diagnostics(document: &Document) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut blocks = document.blocks.iter().collect::<Vec<_>>();
    let mut contents = Vec::new();
    let mut inlines = Vec::new();
    while !blocks.is_empty() || !contents.is_empty() || !inlines.is_empty() {
        if let Some(block) = blocks.pop() {
            match block {
                Block::Parsed(block) => {
                    contents.push(&block.head);
                    blocks.extend(crate::body_children(block));
                }
                Block::Verbatim(_) => {}
            }
            continue;
        }
        if let Some(content) = contents.pop() {
            inlines.extend(content.items.iter().rev());
            continue;
        }
        while let Some(inline) = inlines.pop() {
            match inline {
                Inline::Element {
                    range,
                    kind,
                    members,
                    attrs,
                    ..
                } => {
                    let argument_count = members
                        .iter()
                        .filter(|member| member.argument().is_some())
                        .count();
                    if kind == "=" && argument_count != 2 {
                        diagnostics.push(Diagnostic {
                            code: "association.invalid-arity",
                            severity: DiagnosticSeverity::Warning,
                            message: "inline '=' association requires exactly two arguments"
                                .to_string(),
                            range: range.clone(),
                            related: Vec::new(),
                        });
                    }
                    for member in members.iter().rev() {
                        match member {
                            InlineMember::ParsedArgument(argument) => {
                                contents.push(&argument.content);
                            }
                            InlineMember::Child { inline, .. } => inlines.push(inline),
                            InlineMember::VerbatimArgument(_) => {}
                        }
                    }
                    let _ = attrs;
                }
                Inline::Verbatim { .. } => {}
                Inline::Text { .. } | Inline::Space { .. } | Inline::SoftBreak { .. } => {}
            }
        }
    }
    diagnostics.sort_by_key(|diagnostic| diagnostic.range.start);
    diagnostics
}

fn collect_blocks(
    source: &str,
    blocks: &[Block],
    first_ids: &mut HashMap<String, Range<usize>>,
    output: &mut DocumentOutput,
) {
    for block in blocks {
        match block {
            Block::Parsed(parsed) => {
                if let Some(mark) = &parsed.mark {
                    let kind = if parsed.raw.is_some() {
                        AnchorKind::VerbatimBlock
                    } else if output
                        .headings
                        .heading_at_node_start(parsed.range.start)
                        .is_some()
                    {
                        AnchorKind::Heading
                    } else {
                        AnchorKind::Block
                    };
                    collect_anchor(
                        source,
                        &mark.attrs,
                        kind,
                        parsed.range.clone(),
                        parsed.head.range.clone(),
                        first_ids,
                        output,
                    );
                }
                collect_inlines(source, &parsed.head, first_ids, output);
                for child in crate::body_children(parsed) {
                    collect_blocks(source, std::slice::from_ref(child), first_ids, output);
                }
            }
            Block::Verbatim(_) => {}
        }
    }
}

fn collect_inlines(
    source: &str,
    content: &InlineContent,
    first_ids: &mut HashMap<String, Range<usize>>,
    output: &mut DocumentOutput,
) {
    for inline in &content.items {
        match inline {
            Inline::Element {
                range,
                kind,
                kind_range,
                members,
                attrs,
                ..
            } => {
                let selection_range = members
                    .iter()
                    .find_map(InlineMember::argument)
                    .map_or_else(|| range.clone(), |argument| argument_range(&argument));
                collect_anchor(
                    source,
                    attrs,
                    AnchorKind::Inline,
                    range.clone(),
                    selection_range.clone(),
                    first_ids,
                    output,
                );
                if kind == "->" {
                    collect_link(
                        source,
                        range.clone(),
                        kind_range.clone(),
                        members,
                        attrs,
                        output,
                    );
                } else if kind == "img" {
                    collect_image(
                        source,
                        range.clone(),
                        selection_range.clone(),
                        attrs,
                        output,
                    );
                } else if kind == "file" {
                    collect_file(source, range.clone(), selection_range, attrs, output);
                }
                for member in members {
                    match member {
                        InlineMember::ParsedArgument(argument) => {
                            collect_inlines(source, &argument.content, first_ids, output);
                        }
                        InlineMember::Child { inline, .. } => collect_inlines(
                            source,
                            &InlineContent {
                                range: inline_range(inline).clone(),
                                items: vec![inline.as_ref().clone()],
                            },
                            first_ids,
                            output,
                        ),
                        InlineMember::VerbatimArgument(_) => {}
                    }
                }
            }
            Inline::Verbatim {
                range,
                kind,
                kind_range,
                text,
                text_range,
                quote_count,
                attrs,
                ..
            } => {
                collect_anchor(
                    source,
                    attrs,
                    AnchorKind::Inline,
                    range.clone(),
                    range.clone(),
                    first_ids,
                    output,
                );
                if kind == "->" {
                    collect_verbatim_autolink(
                        source,
                        range.clone(),
                        kind_range.clone(),
                        text,
                        text_range.clone(),
                        *quote_count,
                        attrs,
                        output,
                    );
                }
            }
            Inline::Text { .. } | Inline::Space { .. } | Inline::SoftBreak { .. } => {}
        }
    }
}

fn collect_verbatim_autolink(
    source: &str,
    range: Range<usize>,
    kind_range: Range<usize>,
    text: &str,
    text_range: Range<usize>,
    quote_count: usize,
    attrs: &Attributes,
    output: &mut DocumentOutput,
) {
    if let Some(conflict) = attrs.items.iter().find_map(|item| match item {
        AttrItem::Pair { key, range, .. } if key == "to" => Some(range.clone()),
        _ => None,
    }) {
        output.diagnostics.push(Diagnostic {
            code: "link.conflicting-property",
            severity: DiagnosticSeverity::Warning,
            message: "the '->' inline verbatim kind cannot be combined with a 'to' property"
                .to_string(),
            range: conflict,
            related: vec![kind_range],
        });
        return;
    }
    if !valid_autolink_target(text) {
        output.diagnostics.push(Diagnostic {
            code: "link.invalid-autolink-target",
            severity: DiagnosticSeverity::Warning,
            message: "autolink target must be a nonempty absolute URI or raw relative path"
                .to_string(),
            range: text_range,
            related: Vec::new(),
        });
        return;
    }
    let (target_kind, path_decoded, fragment_decoded) = classify_raw_target(text);
    let path_range = path_decoded
        .map(|decoded| text_range.start + decoded.start..text_range.start + decoded.end);
    let fragment_range = fragment_decoded
        .map(|decoded| text_range.start + decoded.start..text_range.start + decoded.end);
    let envelope = range.start..attrs.range.as_ref().map_or(range.end, |range| range.start);
    output.links.push(LinkRecord {
        range,
        selection_range: text_range.clone(),
        target: direct_source_backed(source, text.to_string(), text_range),
        target_kind,
        spelling: LinkSpelling::Verbatim {
            envelope,
            quote_count,
        },
        path_range,
        fragment_range,
    });
}

fn valid_uri_reference(target: &str) -> bool {
    if target.is_empty()
        || target.chars().any(|character| {
            character.is_whitespace() || character.is_control() || character == '\\'
        })
    {
        return false;
    }
    let bytes = target.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] == b'%' {
            if cursor + 2 >= bytes.len()
                || !bytes[cursor + 1].is_ascii_hexdigit()
                || !bytes[cursor + 2].is_ascii_hexdigit()
            {
                return false;
            }
            cursor += 3;
        } else {
            cursor += 1;
        }
    }
    let base = Url::parse("https://plumb.invalid/").expect("static base URL is valid");
    Url::parse(target).is_ok() || base.join(target).is_ok()
}

fn valid_autolink_target(target: &str) -> bool {
    if target.is_empty()
        || target
            .chars()
            .any(|character| character.is_control() || character == '\\')
    {
        return false;
    }
    if has_uri_scheme(target) || target.starts_with("//") {
        return valid_uri_reference(target);
    }
    if target
        .split_once('#')
        .is_some_and(|(_, fragment)| fragment.is_empty() || fragment.contains('#'))
    {
        return false;
    }
    if target.chars().any(char::is_whitespace) {
        let path_end = target.find('#').unwrap_or(target.len());
        if target
            .chars()
            .any(|character| character != ' ' && character.is_whitespace())
            || target[path_end..].contains(' ')
        {
            return false;
        }
    }
    true
}

fn valid_relative_file_path(target: &str) -> bool {
    !target.is_empty()
        && !Path::new(target).is_absolute()
        && !target
            .chars()
            .any(|character| character.is_control() || character == '\\')
}

fn collect_image(
    source: &str,
    range: Range<usize>,
    selection_range: Range<usize>,
    attrs: &Attributes,
    output: &mut DocumentOutput,
) {
    let Some(value) = attrs.items.iter().find_map(|item| match item {
        AttrItem::Pair { key, value, .. } if key == "src" => Some(value),
        _ => None,
    }) else {
        output.diagnostics.push(Diagnostic {
            code: "image.missing-source",
            severity: DiagnosticSeverity::Warning,
            message: "image requires a nonempty 'src' target".to_string(),
            range,
            related: Vec::new(),
        });
        return;
    };
    let source_value = attr_source_backed(source, value);
    if source_value.value.is_empty() {
        output.diagnostics.push(Diagnostic {
            code: "image.missing-source",
            severity: DiagnosticSeverity::Warning,
            message: "image requires a nonempty 'src' target".to_string(),
            range: source_value.range,
            related: Vec::new(),
        });
        return;
    }
    let target_kind = if has_uri_scheme(&source_value.value) || source_value.value.starts_with("//")
    {
        if !valid_uri_reference(&source_value.value) {
            output.diagnostics.push(Diagnostic {
                code: "image.invalid-source",
                severity: DiagnosticSeverity::Warning,
                message: "absolute image 'src' must be a valid URI reference".to_string(),
                range: source_value.range,
                related: Vec::new(),
            });
            return;
        }
        ImageTarget::External
    } else {
        if !valid_relative_file_path(&source_value.value) {
            output.diagnostics.push(Diagnostic {
                code: "image.invalid-source",
                severity: DiagnosticSeverity::Warning,
                message: "relative image 'src' must be a valid raw file path".to_string(),
                range: source_value.range,
                related: Vec::new(),
            });
            return;
        }
        ImageTarget::File {
            path: source_value.value.clone(),
        }
    };
    output.images.push(ImageRecord {
        range,
        selection_range,
        source: source_value,
        target_kind,
    });
}

fn collect_file(
    source: &str,
    range: Range<usize>,
    selection_range: Range<usize>,
    attrs: &Attributes,
    output: &mut DocumentOutput,
) {
    let Some(value) = attrs.items.iter().find_map(|item| match item {
        AttrItem::Pair { key, value, .. } if key == "src" => Some(value),
        _ => None,
    }) else {
        output.diagnostics.push(Diagnostic {
            code: "file.missing-source",
            severity: DiagnosticSeverity::Warning,
            message: "file requires a nonempty 'src' target".to_string(),
            range,
            related: Vec::new(),
        });
        return;
    };
    let source_value = attr_source_backed(source, value);
    if source_value.value.is_empty() {
        output.diagnostics.push(Diagnostic {
            code: "file.missing-source",
            severity: DiagnosticSeverity::Warning,
            message: "file requires a nonempty 'src' target".to_string(),
            range: source_value.range,
            related: Vec::new(),
        });
        return;
    }
    let target_kind = if has_uri_scheme(&source_value.value) || source_value.value.starts_with("//")
    {
        if !valid_uri_reference(&source_value.value) {
            output.diagnostics.push(Diagnostic {
                code: "file.invalid-source",
                severity: DiagnosticSeverity::Warning,
                message: "absolute file 'src' must be a valid URI reference".to_string(),
                range: source_value.range,
                related: Vec::new(),
            });
            return;
        }
        FileTarget::External
    } else {
        if !valid_relative_file_path(&source_value.value) {
            output.diagnostics.push(Diagnostic {
                code: "file.invalid-source",
                severity: DiagnosticSeverity::Warning,
                message: "relative file 'src' must be a valid raw file path".to_string(),
                range: source_value.range,
                related: Vec::new(),
            });
            return;
        }
        FileTarget::File {
            path: source_value.value.clone(),
        }
    };
    output.files.push(FileRecord {
        range,
        selection_range,
        source: source_value,
        target_kind,
    });
}

fn collect_anchor(
    source: &str,
    attrs: &Attributes,
    kind: AnchorKind,
    range: Range<usize>,
    selection_range: Range<usize>,
    first_ids: &mut HashMap<String, Range<usize>>,
    output: &mut DocumentOutput,
) {
    let Some((value, item_range)) = attrs.items.iter().find_map(|item| match item {
        AttrItem::Id { value, range } => Some((value, range)),
        AttrItem::Class { .. } | AttrItem::Pair { .. } => None,
    }) else {
        return;
    };
    let value_range = item_range.start + 1..item_range.end;
    let id = direct_source_backed(source, value.clone(), value_range.clone());
    if let Some(first) = first_ids.get(value) {
        output.diagnostics.push(Diagnostic {
            code: "anchor.duplicate-id",
            severity: DiagnosticSeverity::Warning,
            message: format!("duplicate explicit anchor id '{value}'"),
            range: value_range,
            related: vec![first.clone()],
        });
    } else {
        first_ids.insert(value.clone(), value_range);
    }
    output.anchors.push(AnchorRecord {
        id,
        kind,
        range,
        selection_range,
    });
}

fn collect_link(
    source: &str,
    range: Range<usize>,
    kind_range: Range<usize>,
    members: &[InlineMember],
    attrs: &Attributes,
    output: &mut DocumentOutput,
) {
    let arguments = members
        .iter()
        .filter_map(InlineMember::argument)
        .collect::<Vec<_>>();
    if let [InlineArgumentRef::Verbatim(argument)] = arguments.as_slice() {
        collect_verbatim_autolink(
            source,
            range,
            kind_range,
            &argument.text,
            argument.text_range.clone(),
            argument.quote_count,
            attrs,
            output,
        );
        return;
    }
    let Some((selection_range, target)) = positional_link_parts(source, members) else {
        output.diagnostics.push(Diagnostic {
            code: "link.missing-target",
            severity: DiagnosticSeverity::Warning,
            message: "link requires exactly two positional arguments".to_string(),
            range,
            related: Vec::new(),
        });
        return;
    };
    push_link(
        range,
        selection_range,
        target,
        LinkSpelling::Positional,
        output,
    );
}

fn positional_link_parts(
    source: &str,
    members: &[InlineMember],
) -> Option<(Range<usize>, SourceBacked<String>)> {
    let arguments = members
        .iter()
        .filter_map(InlineMember::argument)
        .collect::<Vec<_>>();
    let [label, target] = arguments.as_slice() else {
        return None;
    };
    Some((
        argument_range(label),
        source_backed_argument(source, target)?,
    ))
}

fn argument_range(argument: &InlineArgumentRef<'_>) -> Range<usize> {
    match argument {
        InlineArgumentRef::Parsed(content) => content.range.clone(),
        InlineArgumentRef::Verbatim(argument) => argument.text_range.clone(),
    }
}

fn source_backed_argument(
    source: &str,
    argument: &InlineArgumentRef<'_>,
) -> Option<SourceBacked<String>> {
    match argument {
        InlineArgumentRef::Parsed(content) => source_backed_inline_items(source, &content.items),
        InlineArgumentRef::Verbatim(argument) if !argument.text.is_empty() => Some(SourceBacked {
            raw: source[argument.text_range.clone()].to_string(),
            value: argument.text.clone(),
            range: argument.text_range.clone(),
            decoded_boundaries: (argument.text_range.start..=argument.text_range.end).collect(),
        }),
        InlineArgumentRef::Verbatim(_) => None,
    }
}

fn source_backed_inline_items(source: &str, items: &[Inline]) -> Option<SourceBacked<String>> {
    let first = items.first()?;
    let last = items.last()?;
    let range = inline_range(first).start..inline_range(last).end;
    let mut value = String::new();
    let mut decoded_boundaries = vec![range.start];
    for inline in items {
        let (text, source_range) = match inline {
            Inline::Text { text, range } | Inline::Space { text, range } => {
                (text.as_str(), range.clone())
            }
            Inline::SoftBreak { range } => (" ", range.clone()),
            Inline::Element { .. } | Inline::Verbatim { .. } => return None,
        };
        let source_text = &source[source_range.clone()];
        let escaped_single = text.chars().count() == 1 && source_text.len() != text.len();
        for (offset, character) in text.char_indices() {
            value.push(character);
            for byte in 1..=character.len_utf8() {
                let decoded_end = offset + byte == text.len();
                decoded_boundaries.push(if decoded_end {
                    source_range.end
                } else if escaped_single {
                    source_range.start
                } else {
                    source_range.start + offset + byte
                });
            }
        }
    }
    if value.is_empty() {
        return None;
    }
    Some(SourceBacked {
        raw: source[range.clone()].to_string(),
        value,
        range,
        decoded_boundaries,
    })
}

fn inline_range(inline: &Inline) -> &Range<usize> {
    match inline {
        Inline::Text { range, .. }
        | Inline::Space { range, .. }
        | Inline::SoftBreak { range }
        | Inline::Element { range, .. }
        | Inline::Verbatim { range, .. } => range,
    }
}

fn push_link(
    range: Range<usize>,
    selection_range: Range<usize>,
    target: SourceBacked<String>,
    spelling: LinkSpelling,
    output: &mut DocumentOutput,
) {
    let (target_kind, path_decoded, fragment_decoded) = classify_target(&target.value);
    let path_range = path_decoded.and_then(|decoded| target.source_range(decoded));
    let fragment_range = fragment_decoded.and_then(|decoded| target.source_range(decoded));
    output.links.push(LinkRecord {
        range,
        selection_range,
        target,
        target_kind,
        spelling,
        path_range,
        fragment_range,
    });
}

fn classify_target(target: &str) -> (LinkTarget, Option<Range<usize>>, Option<Range<usize>>) {
    if Url::parse(target).is_ok() || target.starts_with("//") {
        return (LinkTarget::External, None, None);
    }
    let (path, fragment) = match target.split_once('#') {
        Some(parts) => parts,
        None if is_plumb_path(target) => {
            return (
                LinkTarget::Document {
                    path: target.to_string(),
                },
                Some(0..target.len()),
                None,
            );
        }
        None => {
            let path = uri_reference_path(target);
            if path.is_empty() {
                return (LinkTarget::Other, None, None);
            }
            return (
                LinkTarget::File {
                    path: path.to_string(),
                },
                Some(0..path.len()),
                None,
            );
        }
    };
    if fragment.is_empty() {
        return (LinkTarget::Other, None, None);
    }
    if !path.is_empty() && !is_plumb_path(path) {
        let file_path = uri_reference_path(path);
        return (
            LinkTarget::File {
                path: file_path.to_string(),
            },
            Some(0..file_path.len()),
            None,
        );
    }
    let path_value = (!path.is_empty()).then(|| path.to_string());
    let path_range = (!path.is_empty()).then_some(0..path.len());
    let fragment_start = path.len() + 1;
    (
        LinkTarget::Anchor {
            path: path_value,
            fragment: fragment.to_string(),
        },
        path_range,
        Some(fragment_start..target.len()),
    )
}

fn classify_raw_target(target: &str) -> (LinkTarget, Option<Range<usize>>, Option<Range<usize>>) {
    if has_uri_scheme(target) || target.starts_with("//") {
        return (LinkTarget::External, None, None);
    }
    let (path, fragment) = match target.split_once('#') {
        Some(parts) => parts,
        None if is_plumb_path(target) => {
            return (
                LinkTarget::Document {
                    path: target.to_string(),
                },
                Some(0..target.len()),
                None,
            );
        }
        None => {
            return (
                LinkTarget::File {
                    path: target.to_string(),
                },
                Some(0..target.len()),
                None,
            );
        }
    };
    let path_value = (!path.is_empty()).then(|| path.to_string());
    let path_range = (!path.is_empty()).then_some(0..path.len());
    let fragment_start = path.len() + 1;
    (
        LinkTarget::Anchor {
            path: path_value,
            fragment: fragment.to_string(),
        },
        path_range,
        Some(fragment_start..target.len()),
    )
}

pub(crate) fn has_uri_scheme(target: &str) -> bool {
    let Some((scheme, _)) = target.split_once(':') else {
        return false;
    };
    let mut characters = scheme.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        })
}

fn uri_reference_path(target: &str) -> &str {
    let end = target.find(['?', '#']).unwrap_or(target.len());
    &target[..end]
}

fn is_plumb_path(value: &str) -> bool {
    value.ends_with(".plumb")
}

fn direct_source_backed(source: &str, value: String, range: Range<usize>) -> SourceBacked<String> {
    let decoded_boundaries = (range.start..=range.end).collect();
    SourceBacked {
        raw: source[range.clone()].to_string(),
        value,
        range,
        decoded_boundaries,
    }
}

pub(crate) fn attr_source_backed(source: &str, value: &AttrValue) -> SourceBacked<String> {
    if !value.quoted || !(value.raw.starts_with('"') && value.raw.ends_with('"')) {
        return direct_source_backed(source, value.decoded.clone(), value.range.clone());
    }
    let mut decoded_boundaries = Vec::with_capacity(value.decoded.len() + 1);
    let mut cursor = value.range.start + 1;
    let end = value.range.end.saturating_sub(1);
    while cursor < end {
        let source_start = cursor;
        if source.as_bytes()[cursor] == b'\\' {
            cursor += 1;
        }
        let character = source[cursor..]
            .chars()
            .next()
            .expect("quoted value cursor is valid");
        for _ in 0..character.len_utf8() {
            decoded_boundaries.push(source_start);
        }
        cursor += character.len_utf8();
    }
    decoded_boundaries.push(end);
    SourceBacked {
        value: value.decoded.clone(),
        raw: value.raw.clone(),
        range: value.range.clone(),
        decoded_boundaries,
    }
}

#[cfg(test)]
mod tests {
    use plumb_syntax::parse;

    use super::*;

    #[test]
    fn only_shorthand_ids_create_anchors() {
        let parsed = parse("`# Heading\n  `@ intro\n\n`## Pair only\n  `= id pair\n");
        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(output.anchors.len(), 1);
        assert_eq!(output.anchors[0].id.value, "intro");
        assert_eq!(output.anchors[0].kind, AnchorKind::Heading);
    }

    #[test]
    fn verbatim_blocks_create_syntax_neutral_anchors() {
        let parsed = parse("`text\n `@ example\n\n|\"\n raw text\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(output.anchors.len(), 1);
        assert_eq!(output.anchors[0].kind, AnchorKind::VerbatimBlock);
    }

    #[test]
    fn recognizes_compact_and_expanded_positional_links() {
        let source = "`->[guide|target.plumb]\n`->[guide page|\"[Project Guide.plumb#intro]\"]\n`->[`*[external]|https://example.test]\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(output.links.len(), 3);
        assert!(output
            .links
            .iter()
            .all(|link| link.spelling == LinkSpelling::Positional));
        assert_eq!(output.links[0].target.value, "target.plumb");
        assert_eq!(
            output.links[0].target_kind,
            LinkTarget::Document {
                path: "target.plumb".to_string()
            }
        );
        assert_eq!(
            &source[output.links[1].selection_range.clone()],
            "guide page"
        );
        assert_eq!(output.links[1].target.value, "Project Guide.plumb#intro");
        assert_eq!(
            output.links[1].target_kind,
            LinkTarget::Anchor {
                path: Some("Project Guide.plumb".to_string()),
                fragment: "intro".to_string()
            }
        );
        assert_eq!(
            &source[output.links[2].selection_range.clone()],
            "`*[external]"
        );
        assert_eq!(output.links[2].target_kind, LinkTarget::External);
    }

    #[test]
    fn positional_link_ranges_map_utf8_and_escaped_delimiters() {
        let source = "`->[目标|目录/项`].plumb#章节]\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let link = &output.links[0];
        assert_eq!(link.target.raw, "目录/项`].plumb#章节");
        assert_eq!(link.target.value, "目录/项].plumb#章节");
        assert_eq!(&source[link.path_range.clone().unwrap()], "目录/项`].plumb");
        assert_eq!(&source[link.fragment_range.clone().unwrap()], "章节");
    }

    #[test]
    fn diagnoses_associations_with_more_than_two_slots_inside_attachments() {
        let source = "`span[value|=[key|value|extra]]\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        let diagnostic = output
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "association.invalid-arity")
            .expect("invalid association arity diagnostic");
        assert_eq!(&source[diagnostic.range.clone()], "=[key|value|extra]");
    }

    #[test]
    fn link_kind_is_not_a_standard_link() {
        let parsed = parse("`link[generic|=[to|other.plumb#target]]\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert!(output.links.is_empty());
    }

    #[test]
    fn recognizes_inline_verbatim_autolinks_without_normalizing_the_target() {
        let source = "Visit `->\"https://example.test/a%20b\" or `->\"https://[::1]/\".\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(output.links.len(), 2);
        assert_eq!(output.links[0].target.value, "https://example.test/a%20b");
        assert_eq!(output.links[0].target.raw, "https://example.test/a%20b");
        assert_eq!(output.links[0].target_kind, LinkTarget::External);
        assert_eq!(output.links[1].target.value, "https://[::1]/");
    }

    #[test]
    fn recognizes_relative_autolink_targets() {
        let source = "`->\"other.plumb\"\n`->\"other notes.plumb#section\"\n`->\"../assets/a b.pdf\"\n`->\"../assets/100% done?.pdf\"\n`->\"#local\"\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(output.links.len(), 5);
        assert!(output
            .links
            .iter()
            .all(|link| matches!(link.spelling, LinkSpelling::Verbatim { .. })));
        assert_eq!(
            output.links[0].target_kind,
            LinkTarget::Document {
                path: "other.plumb".to_string()
            }
        );
        assert_eq!(
            output.links[1].target_kind,
            LinkTarget::Anchor {
                path: Some("other notes.plumb".to_string()),
                fragment: "section".to_string()
            }
        );
        assert_eq!(
            output.links[2].target_kind,
            LinkTarget::File {
                path: "../assets/a b.pdf".to_string()
            }
        );
        assert_eq!(
            &parsed.source[output.links[1].fragment_range.clone().unwrap()],
            "section"
        );
        assert_eq!(
            output.links[3].target_kind,
            LinkTarget::File {
                path: "../assets/100% done?.pdf".to_string()
            }
        );
        assert_eq!(
            output.links[4].target_kind,
            LinkTarget::Anchor {
                path: None,
                fragment: "local".to_string()
            }
        );
    }

    #[test]
    fn recognizes_standard_images_and_diagnoses_invalid_sources() {
        let source = "`img[Alt `em[text]|=[src|\"[static/图 像(100%).png]\"]|@[figure]|+[wide]|=[loading|lazy]]\n`img[|=[src|https://example.test/a.png]]\n`img[Missing]\n`img[Empty|=[src|]]\n`img[Invalid URI|=[src|\"[https://example.test/bad path.png]\"]]\n`img[Invalid path|=[src|bad\\path.png]]\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(output.images.len(), 2);
        assert_eq!(output.images[0].source.value, "static/图 像(100%).png");
        assert_eq!(
            output.images[0].target_kind,
            ImageTarget::File {
                path: "static/图 像(100%).png".to_string()
            }
        );
        assert_eq!(output.images[1].target_kind, ImageTarget::External);
        assert_eq!(
            output
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            [
                "image.missing-source",
                "image.missing-source",
                "image.invalid-source",
                "image.invalid-source"
            ]
        );
    }

    #[test]
    fn recognizes_standard_files_and_diagnoses_invalid_sources() {
        let source = "`file[Demo|=[src|\"[static/demo video.mp4]\"]|@[demo]|+[wide]]\n`file[Remote|=[src|https://example.test/demo.mp4]]\n`file[Missing]\n`file[Empty|=[src|]]\n`file[Invalid URI|=[src|\"[https://example.test/bad path.mp4]\"]]\n`file[Invalid path|=[src|bad\\path.mp4]]\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(output.files.len(), 2);
        assert_eq!(output.files[0].source.value, "static/demo video.mp4");
        assert_eq!(
            output.files[0].target_kind,
            FileTarget::File {
                path: "static/demo video.mp4".to_string()
            }
        );
        assert_eq!(output.files[1].target_kind, FileTarget::External);
        assert_eq!(
            output
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            [
                "file.missing-source",
                "file.missing-source",
                "file.invalid-source",
                "file.invalid-source"
            ]
        );
    }

    #[test]
    fn diagnoses_invalid_autolink_targets_and_ignores_arrow_facets() {
        let source = "`->\"[]\"\n`->\"https://example.test/bad path\"\n`->\"https://example.test/%zz\"\n`->\"doc.plumb#one#two\"\n`span[text|+[->]]\n\n`note head\n `+ ->\n\n`()\n `+ ->\n\n|\"\n raw\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert!(output.links.is_empty());
        let codes = output
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();
        assert_eq!(
            codes,
            [
                "link.invalid-autolink-target",
                "link.invalid-autolink-target",
                "link.invalid-autolink-target",
                "link.invalid-autolink-target",
            ]
        );
    }

    #[test]
    fn duplicate_ids_are_semantic_diagnostics() {
        let parsed = parse("`node One\n  `@ same\n\n`other Two\n  `@ same\n");
        assert!(parsed.is_valid());
        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(output.diagnostics[0].code, "anchor.duplicate-id");
    }
}
