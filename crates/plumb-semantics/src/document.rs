use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

use plumb_syntax::{
    AttrItem, AttrValue, Attributes, Block, Diagnostic, DiagnosticSeverity, Document, Inline,
    InlineContent, ValidDocument,
};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    analyze_citations, analyze_events, analyze_headings, analyze_inline_styles, analyze_lists,
    analyze_math, analyze_metadata, analyze_quotes, analyze_tables, analyze_tasks, CitationOutput,
    EventOutput, HeadingOutput, InlineStyleOutput, ListOutput, MathOutput, MetadataOutput,
    QuoteOutput, TableOutput, TaskOutput,
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
    pub target_range: Range<usize>,
    pub target_element_count: usize,
    pub target_declaration_ranges: Vec<Range<usize>>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventLinkRange {
    pub event_start: usize,
    pub links: Range<usize>,
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
    pub tables: TableOutput,
    pub anchors: Vec<AnchorRecord>,
    pub links: Vec<LinkRecord>,
    pub event_link_ranges: Vec<EventLinkRange>,
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

    pub fn links_contained_by_event(&self, event_start: usize) -> Option<&[LinkRecord]> {
        let index = self
            .event_link_ranges
            .binary_search_by_key(&event_start, |range| range.event_start)
            .ok()?;
        Some(&self.links[self.event_link_ranges[index].links.clone()])
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
    let tables = analyze_tables(valid);
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
        tables,
        ..DocumentOutput::default()
    };
    output
        .diagnostics
        .extend(association_arity_diagnostics(document));
    let mut first_ids: HashMap<String, Range<usize>> = HashMap::new();
    collect_blocks(source, &document.blocks, &mut first_ids, &mut output);
    output.event_link_ranges = build_event_link_ranges(&output.events.events, &output.links);
    output
        .diagnostics
        .extend(output.tables.diagnostics.iter().cloned());
    output
}

fn build_event_link_ranges(
    events: &[crate::EventRecord],
    links: &[LinkRecord],
) -> Vec<EventLinkRange> {
    debug_assert!(events
        .windows(2)
        .all(|events| events[0].range.start <= events[1].range.start));
    debug_assert!(links
        .windows(2)
        .all(|links| links[0].range.start <= links[1].range.start));
    events
        .iter()
        .map(|event| {
            let start = links.partition_point(|link| link.range.start < event.range.start);
            let end = links.partition_point(|link| link.range.start < event.range.end);
            debug_assert!(links[start..end]
                .iter()
                .all(|link| link.range.end <= event.range.end));
            EventLinkRange {
                event_start: event.range.start,
                links: start..end,
            }
        })
        .collect()
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
                    contents.push(&block.content);
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
                Inline::Group {
                    range,
                    mark,
                    content,
                } => {
                    let argument_count = crate::positional_elements(content).len();
                    if mark.as_ref().is_some_and(|mark| mark.marker == "=") && argument_count < 2 {
                        diagnostics.push(Diagnostic {
                            code: "association.invalid-arity",
                            severity: DiagnosticSeverity::Warning,
                            message: "inline '=' association requires a key and value".to_string(),
                            range: range.clone(),
                            related: Vec::new(),
                        });
                    }
                    contents.push(content);
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
                    let kind = if output
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
                        crate::inline_selection_range(&parsed.content),
                        first_ids,
                        output,
                    );
                }
                collect_inlines(source, &parsed.content, first_ids, output);
                for child in crate::body_children(parsed) {
                    collect_blocks(source, std::slice::from_ref(child), first_ids, output);
                }
            }
            Block::Verbatim(block) => {
                if let Some(mark) = &block.mark {
                    collect_anchor(
                        source,
                        &mark.attrs,
                        AnchorKind::VerbatimBlock,
                        block.range.clone(),
                        block.range.clone(),
                        first_ids,
                        output,
                    );
                }
            }
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
            Inline::Group {
                range,
                mark,
                content,
            } => {
                let selection_range = crate::positional_elements(content)
                    .first()
                    .map_or_else(|| range.clone(), |element| element.range.clone());
                if let Some(mark) = mark {
                    collect_anchor(
                        source,
                        &mark.attrs,
                        AnchorKind::Inline,
                        range.clone(),
                        selection_range.clone(),
                        first_ids,
                        output,
                    );
                    match mark.marker.as_str() {
                        "->" => collect_link(source, range.clone(), content, output),
                        "img" => collect_image(
                            source,
                            range.clone(),
                            selection_range.clone(),
                            &mark.attrs,
                            output,
                        ),
                        "file" => collect_file(
                            source,
                            range.clone(),
                            selection_range,
                            &mark.attrs,
                            output,
                        ),
                        _ => {}
                    }
                }
                collect_inlines(source, content, first_ids, output);
            }
            Inline::Verbatim {
                range,
                mark,
                text,
                text_range,
                quote_count,
                ..
            } => {
                if let Some(mark) = mark {
                    collect_anchor(
                        source,
                        &mark.attrs,
                        AnchorKind::Inline,
                        range.clone(),
                        range.clone(),
                        first_ids,
                        output,
                    );
                    if mark.marker == "->" {
                        collect_verbatim_link(
                            source,
                            VerbatimLink {
                                range: range.clone(),
                                kind_range: mark.marker_range.clone(),
                                text,
                                text_range: text_range.clone(),
                                quote_count: *quote_count,
                                attrs: &mark.attrs,
                            },
                            output,
                        );
                    }
                }
            }
            Inline::Text { .. } | Inline::Space { .. } | Inline::SoftBreak { .. } => {}
        }
    }
}

struct VerbatimLink<'a> {
    range: Range<usize>,
    kind_range: Range<usize>,
    text: &'a str,
    text_range: Range<usize>,
    quote_count: usize,
    attrs: &'a Attributes,
}

fn collect_verbatim_link(source: &str, input: VerbatimLink<'_>, output: &mut DocumentOutput) {
    let VerbatimLink {
        range,
        kind_range,
        text,
        text_range,
        quote_count,
        attrs,
    } = input;
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
    if !valid_derived_link_target(text) {
        output.diagnostics.push(Diagnostic {
            code: "link.invalid-target",
            severity: DiagnosticSeverity::Warning,
            message: "link target must be a nonempty absolute URI or raw relative path".to_string(),
            range: text_range,
            related: Vec::new(),
        });
        return;
    }
    let envelope = range.start..attrs.range.as_ref().map_or(range.end, |range| range.start);
    let target_range = text_range.clone();
    push_link(
        range,
        text_range.clone(),
        direct_source_backed(source, text.to_string(), text_range),
        LinkSourceProjection {
            spelling: LinkSpelling::Verbatim {
                envelope,
                quote_count,
            },
            target_range,
            target_element_count: 1,
            target_declaration_ranges: Vec::new(),
        },
        classify_raw_target(text),
        output,
    );
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

fn valid_derived_link_target(target: &str) -> bool {
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
    let Some((value, value_range)) = attrs.items.iter().find_map(|item| match item {
        AttrItem::Id {
            value, value_range, ..
        } => Some((value, value_range)),
        AttrItem::Class { .. } | AttrItem::Pair { .. } => None,
    }) else {
        return;
    };
    let id = direct_source_backed(source, value.clone(), value_range.clone());
    if let Some(first) = first_ids.get(value) {
        output.diagnostics.push(Diagnostic {
            code: "anchor.duplicate-id",
            severity: DiagnosticSeverity::Warning,
            message: format!("duplicate explicit anchor id '{value}'"),
            range: value_range.clone(),
            related: vec![first.clone()],
        });
    } else {
        first_ids.insert(value.clone(), value_range.clone());
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
    content: &InlineContent,
    output: &mut DocumentOutput,
) {
    let view = crate::owner_semantic_view(content);
    let Some(arguments) = view.split_first() else {
        output.diagnostics.push(Diagnostic {
            code: "link.missing-target",
            severity: DiagnosticSeverity::Warning,
            message: "link requires at least one positional argument".to_string(),
            range,
            related: Vec::new(),
        });
        return;
    };
    let derived_label = arguments.rest.is_empty();
    let target_range = if derived_label {
        arguments.first.range.clone()
    } else {
        arguments
            .rest_range()
            .expect("explicit Link has target elements")
    };
    let target_element_count = if derived_label {
        1
    } else {
        arguments.rest.len()
    };
    let target_declaration_ranges = if derived_label {
        Vec::new()
    } else {
        arguments.rest_declaration_ranges()
    };
    let target_content = if derived_label {
        Some(arguments.first.clone())
    } else {
        arguments.rest_content()
    };
    let Some(target) = target_content
        .as_ref()
        .and_then(|content| stringify_target(source, content))
    else {
        output.diagnostics.push(Diagnostic {
            code: "link.invalid-target",
            severity: DiagnosticSeverity::Warning,
            message: "link target must stringify to a nonempty value".to_string(),
            range: arguments
                .rest_range()
                .unwrap_or_else(|| arguments.first.range.clone()),
            related: Vec::new(),
        });
        return;
    };
    if derived_label && !valid_derived_link_target(&target.value) {
        output.diagnostics.push(Diagnostic {
            code: "link.invalid-target",
            severity: DiagnosticSeverity::Warning,
            message: "link target must be a nonempty absolute URI or raw relative path".to_string(),
            range: target.range.clone(),
            related: Vec::new(),
        });
        return;
    }
    let classification = if derived_label {
        classify_raw_target(&target.value)
    } else {
        classify_target(&target.value)
    };
    push_link(
        range,
        crate::element_selection_range(arguments.first),
        target,
        LinkSourceProjection {
            spelling: LinkSpelling::Positional,
            target_range,
            target_element_count,
            target_declaration_ranges,
        },
        classification,
        output,
    );
}

fn stringify_target(source: &str, content: &InlineContent) -> Option<SourceBacked<String>> {
    let mut builder = StringifyBuilder::default();
    stringify_content(source, content, &mut builder);
    builder.finish(source)
}

fn stringify_content(source: &str, content: &InlineContent, output: &mut StringifyBuilder) {
    let view = crate::owner_semantic_view(content);
    if let Some(content) = view.visible_content() {
        for inline in &content.items {
            match inline {
                Inline::Text { text, range } => {
                    output.append_text(source, text, range.clone());
                }
                Inline::Space { range, .. } | Inline::SoftBreak { range } => {
                    output.append_text(source, " ", range.clone());
                }
                Inline::Verbatim {
                    text, text_range, ..
                } => output.append_text(source, text, text_range.clone()),
                Inline::Group { content, .. } => stringify_content(source, content, output),
            }
        }
    }
}

#[derive(Default)]
struct StringifyBuilder {
    value: String,
    decoded_boundaries: Vec<usize>,
    range: Option<Range<usize>>,
}

impl StringifyBuilder {
    fn append_text(&mut self, source: &str, text: &str, source_range: Range<usize>) {
        if text.is_empty() {
            return;
        }
        if self.decoded_boundaries.is_empty() {
            self.decoded_boundaries.push(source_range.start);
            self.range = Some(source_range.clone());
        } else {
            *self.decoded_boundaries.last_mut().unwrap() = source_range.start;
            self.range.as_mut().unwrap().end = source_range.end;
        }
        let source_text = &source[source_range.clone()];
        let escaped_single = text.chars().count() == 1 && source_text.len() != text.len();
        for (offset, character) in text.char_indices() {
            self.value.push(character);
            for byte in 1..=character.len_utf8() {
                let decoded_end = offset + byte == text.len();
                self.decoded_boundaries.push(if decoded_end {
                    source_range.end
                } else if escaped_single {
                    source_range.start
                } else {
                    source_range.start + offset + byte
                });
            }
        }
    }

    fn finish(self, source: &str) -> Option<SourceBacked<String>> {
        let range = self.range?;
        (!self.value.is_empty()).then(|| SourceBacked {
            raw: source[range.clone()].to_string(),
            value: self.value,
            range,
            decoded_boundaries: self.decoded_boundaries,
        })
    }
}

struct LinkSourceProjection {
    spelling: LinkSpelling,
    target_range: Range<usize>,
    target_element_count: usize,
    target_declaration_ranges: Vec<Range<usize>>,
}

fn push_link(
    range: Range<usize>,
    selection_range: Range<usize>,
    target: SourceBacked<String>,
    source: LinkSourceProjection,
    classification: (LinkTarget, Option<Range<usize>>, Option<Range<usize>>),
    output: &mut DocumentOutput,
) {
    let LinkSourceProjection {
        spelling,
        target_range,
        target_element_count,
        target_declaration_ranges,
    } = source;
    let (target_kind, path_decoded, fragment_decoded) = classification;
    let path_range = path_decoded.and_then(|decoded| target.source_range(decoded));
    let fragment_range = fragment_decoded.and_then(|decoded| target.source_range(decoded));
    output.links.push(LinkRecord {
        range,
        selection_range,
        target,
        target_kind,
        spelling,
        target_range,
        target_element_count,
        target_declaration_ranges,
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
        let parsed = parse("`# Heading\n  `@ intro\n\n`## Pair only\n  `= id|pair\n");
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
    fn verbatim_wrappers_create_syntax_neutral_anchors() {
        let parsed = plumb_syntax::parse("`()\n `@ example\n `text\"\n  raw text\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(output.anchors.len(), 1);
        assert_eq!(output.anchors[0].kind, AnchorKind::Block);
    }

    #[test]
    fn recognizes_compact_and_expanded_positional_links() {
        let source = "`->{guide.plumb}\n`->{`!{styled.plumb}}\n`->{`\"Project Guide.plumb#intro\" `@{rich}}\n`->{guide target.plumb}\n`->{{guide page} `\"Project Guide.plumb#intro\"}\n`->{`*{external} https://example.test}\n";
        let parsed = plumb_syntax::parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(output.links.len(), 6);
        assert!(output
            .links
            .iter()
            .all(|link| link.spelling == LinkSpelling::Positional));
        assert_eq!(output.links[0].target.value, "guide.plumb");
        assert_eq!(output.links[1].target.value, "styled.plumb");
        assert_eq!(
            &source[output.links[1].path_range.clone().unwrap()],
            "styled.plumb"
        );
        assert_eq!(output.links[2].target.value, "Project Guide.plumb#intro");
        assert_eq!(
            &source[output.links[2].fragment_range.clone().unwrap()],
            "intro"
        );
        assert_eq!(output.links[3].target.value, "target.plumb");
        assert_eq!(
            output.links[3].target_kind,
            LinkTarget::Document {
                path: "target.plumb".to_string()
            }
        );
        assert_eq!(
            &source[output.links[4].selection_range.clone()],
            "guide page"
        );
        assert_eq!(output.links[4].target.value, "Project Guide.plumb#intro");
        assert_eq!(
            output.links[4].target_kind,
            LinkTarget::Anchor {
                path: Some("Project Guide.plumb".to_string()),
                fragment: "intro".to_string()
            }
        );
        assert_eq!(
            &source[output.links[5].selection_range.clone()],
            "`*{external}"
        );
        assert_eq!(output.links[5].target_kind, LinkTarget::External);
    }

    #[test]
    fn indexes_overlapping_event_containment_without_copying_links() {
        let parsed = parse(
            "`->{Before before.plumb}\n\n`- 10:00 {Outer `->{Outer outer.plumb}}\n `+ event\n `- 11:00 {Nested `->{Nested nested.plumb}}\n  `+ event\n\n`->{After after.plumb}\n",
        );
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_document(parsed.valid_syntax().unwrap());
        assert_eq!(output.links.len(), 4);
        assert_eq!(output.event_link_ranges.len(), 2);

        let outer = &output.events.events[0];
        let nested = &output.events.events[1];
        assert_eq!(
            output
                .links_contained_by_event(outer.range.start)
                .unwrap()
                .iter()
                .map(|link| link.target.value.as_str())
                .collect::<Vec<_>>(),
            ["outer.plumb", "nested.plumb"]
        );
        assert_eq!(
            output
                .links_contained_by_event(nested.range.start)
                .unwrap()
                .iter()
                .map(|link| link.target.value.as_str())
                .collect::<Vec<_>>(),
            ["nested.plumb"]
        );
        assert!(output.links_contained_by_event(usize::MAX).is_none());
        assert_eq!(
            std::mem::size_of::<EventLinkRange>(),
            3 * std::mem::size_of::<usize>()
        );
    }

    #[test]
    fn positional_link_ranges_map_utf8_and_escaped_delimiters() {
        let source = "`->{目标 目录/项].plumb#章节}\n";
        let parsed = plumb_syntax::parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let link = &output.links[0];
        assert_eq!(link.target.raw, "目录/项].plumb#章节");
        assert_eq!(link.target.value, "目录/项].plumb#章节");
        assert_eq!(&source[link.path_range.clone().unwrap()], "目录/项].plumb");
        assert_eq!(&source[link.fragment_range.clone().unwrap()], "章节");
    }

    #[test]
    fn associations_bind_all_elements_after_the_key_as_value() {
        let source = "`span{value `={key value extra}}\n";
        let parsed = plumb_syntax::parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let Block::Parsed(block) = &parsed.syntax.blocks[0] else {
            panic!("expected parsed block");
        };
        let Inline::Group {
            mark: Some(mark), ..
        } = &block.content.items[0]
        else {
            panic!("expected marked inline group");
        };
        assert_eq!(mark.attrs.value("key"), Some("value extra"));
    }

    #[test]
    fn link_kind_is_not_a_standard_link() {
        let parsed = plumb_syntax::parse("`link{generic `={to other.plumb#target}}\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert!(output.links.is_empty());
    }

    #[test]
    fn recognizes_inline_verbatim_links_without_normalizing_the_target() {
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
    fn recognizes_relative_verbatim_link_targets() {
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
        let source = "`img{{Alt `*{text}} `={src `\"static/图 像(100%).png\"} `@{figure} `+{wide} `={loading lazy}}\n`img{`={src https://example.test/a.png}}\n`img{Missing}\n`img{Empty `={src {}}}\n`img{{Invalid URI} `={src `\"https://example.test/bad path.png\"}}\n`img{{Invalid path} `={src bad\\path.png}}\n";
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
        let source = "`file{Demo `={src `\"static/demo video.mp4\"} `@{demo} `+{wide}}\n`file{Remote `={src https://example.test/demo.mp4}}\n`file{Missing}\n`file{Empty `={src {}}}\n`file{{Invalid URI} `={src `\"https://example.test/bad path.mp4\"}}\n`file{{Invalid path} `={src bad\\path.mp4}}\n";
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
    fn diagnoses_invalid_derived_link_targets_and_ignores_arrow_facets() {
        let source = "`->\"\"\n`->\"https://example.test/bad path\"\n`->\"https://example.test/%zz\"\n`->\"doc.plumb#one#two\"\n`span{text `+{->}}\n\n`note head\n `+ ->\n\n`()\n `+ ->\n\n `\"\n  raw\n";
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
                "link.invalid-target",
                "link.invalid-target",
                "link.invalid-target",
                "link.invalid-target",
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
