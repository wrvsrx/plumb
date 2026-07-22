use std::collections::HashMap;
use std::ops::Range;

use plumb_core::{
    AttrItem, AttrValue, Attributes, Block, Diagnostic, DiagnosticSeverity, Document, Inline,
    InlineContent,
};
use url::Url;

use crate::{
    analyze_citations, analyze_headings, analyze_lists, analyze_math, analyze_metadata,
    analyze_tasks, CitationOutput, HeadingOutput, ListOutput, MathOutput, MetadataOutput,
    TaskOutput,
};

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorKind {
    Heading,
    Block,
    Inline,
    VerbatimBlock,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorRecord {
    pub id: SourceBacked<String>,
    pub kind: AnchorKind,
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkSpelling {
    Explicit,
    Verbatim {
        envelope: Range<usize>,
        quote_count: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkRecord {
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
    pub target: SourceBacked<String>,
    pub target_kind: LinkTarget,
    pub spelling: LinkSpelling,
    pub path_range: Option<Range<usize>>,
    pub fragment_range: Option<Range<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageTarget {
    External,
    File { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRecord {
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
    pub source: SourceBacked<String>,
    pub target_kind: ImageTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DocumentOutput {
    pub headings: HeadingOutput,
    pub metadata: MetadataOutput,
    pub citations: CitationOutput,
    pub lists: ListOutput,
    pub math: MathOutput,
    pub tasks: TaskOutput,
    pub anchors: Vec<AnchorRecord>,
    pub links: Vec<LinkRecord>,
    pub images: Vec<ImageRecord>,
    pub diagnostics: Vec<Diagnostic>,
}

impl DocumentOutput {
    pub fn link_at_node_start(&self, start: usize) -> Option<&LinkRecord> {
        self.links.iter().find(|link| link.range.start == start)
    }

    pub fn image_at_node_start(&self, start: usize) -> Option<&ImageRecord> {
        self.images.iter().find(|image| image.range.start == start)
    }
}

pub fn analyze_document(source: &str, document: &Document) -> DocumentOutput {
    let headings = analyze_headings(document);
    let metadata = analyze_metadata(document);
    let citations = analyze_citations(document);
    let lists = analyze_lists(document);
    let math = analyze_math(document);
    let tasks = analyze_tasks(source, document);
    let mut output = DocumentOutput {
        headings,
        metadata,
        citations,
        lists,
        math,
        tasks,
        ..DocumentOutput::default()
    };
    let mut first_ids: HashMap<String, Range<usize>> = HashMap::new();
    collect_blocks(source, &document.blocks, &mut first_ids, &mut output);
    output
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
                    diagnose_autolink_owner(&mark.attrs, output);
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
                        parsed.head.range.clone(),
                        first_ids,
                        output,
                    );
                }
                collect_inlines(source, &parsed.head, first_ids, output);
                collect_blocks(source, &parsed.children, first_ids, output);
            }
            Block::Verbatim(block) => {
                diagnose_autolink_owner(&block.attrs, output);
                collect_anchor(
                    source,
                    &block.attrs,
                    AnchorKind::VerbatimBlock,
                    block.range.clone(),
                    block.opener_range.clone(),
                    first_ids,
                    output,
                );
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
            Inline::Element {
                range,
                kind,
                content,
                attrs,
                ..
            } => {
                diagnose_autolink_owner(attrs, output);
                collect_anchor(
                    source,
                    attrs,
                    AnchorKind::Inline,
                    range.clone(),
                    content.range.clone(),
                    first_ids,
                    output,
                );
                if kind == "->" {
                    collect_link(source, range.clone(), content.range.clone(), attrs, output);
                } else if kind == "img" {
                    collect_image(source, range.clone(), content.range.clone(), attrs, output);
                }
                collect_inlines(source, content, first_ids, output);
            }
            Inline::Verbatim {
                range,
                text,
                text_range,
                quote_count,
                attrs,
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
                collect_verbatim_autolink(
                    source,
                    range.clone(),
                    text,
                    text_range.clone(),
                    *quote_count,
                    attrs,
                    output,
                );
            }
            Inline::Text { .. } | Inline::SoftBreak { .. } => {}
        }
    }
}

fn diagnose_autolink_owner(attrs: &Attributes, output: &mut DocumentOutput) {
    for range in attrs.items.iter().filter_map(|item| match item {
        AttrItem::Class { value, range } if value == "->" => Some(range.clone()),
        _ => None,
    }) {
        output.diagnostics.push(Diagnostic {
            code: "link.invalid-owner",
            severity: DiagnosticSeverity::Warning,
            message: "the '.->' facet is only valid on inline verbatim".to_string(),
            range,
            related: Vec::new(),
        });
    }
}

fn collect_verbatim_autolink(
    source: &str,
    range: Range<usize>,
    text: &str,
    text_range: Range<usize>,
    quote_count: usize,
    attrs: &Attributes,
    output: &mut DocumentOutput,
) {
    let Some(class_range) = attrs.items.iter().find_map(|item| match item {
        AttrItem::Class { value, range } if value == "->" => Some(range.clone()),
        _ => None,
    }) else {
        return;
    };
    if let Some(conflict) = attrs.items.iter().find_map(|item| match item {
        AttrItem::Pair { key, range, .. } if key == "to" => Some(range.clone()),
        AttrItem::Class { value, range } if value == "$" => Some(range.clone()),
        _ => None,
    }) {
        output.diagnostics.push(Diagnostic {
            code: "link.conflicting-facet",
            severity: DiagnosticSeverity::Warning,
            message: "the '.->' facet cannot be combined with 'to' or '.$'".to_string(),
            range: conflict,
            related: vec![class_range],
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
            message: "image requires a nonempty 'src' URI reference".to_string(),
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
            message: "image requires a nonempty 'src' URI reference".to_string(),
            range: source_value.range,
            related: Vec::new(),
        });
        return;
    }
    if !valid_uri_reference(&source_value.value) {
        output.diagnostics.push(Diagnostic {
            code: "image.invalid-source",
            severity: DiagnosticSeverity::Warning,
            message: "image 'src' must be a valid URI reference".to_string(),
            range: source_value.range,
            related: Vec::new(),
        });
        return;
    }
    let target_kind =
        if Url::parse(&source_value.value).is_ok() || source_value.value.starts_with("//") {
            ImageTarget::External
        } else {
            let path = uri_reference_path(&source_value.value);
            if path.is_empty() {
                output.diagnostics.push(Diagnostic {
                    code: "image.invalid-source",
                    severity: DiagnosticSeverity::Warning,
                    message: "relative image 'src' must contain a file path".to_string(),
                    range: source_value.range,
                    related: Vec::new(),
                });
                return;
            }
            ImageTarget::File {
                path: path.to_string(),
            }
        };
    output.images.push(ImageRecord {
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
    selection_range: Range<usize>,
    attrs: &Attributes,
    output: &mut DocumentOutput,
) {
    let Some(value) = attrs.items.iter().find_map(|item| match item {
        AttrItem::Pair { key, value, .. } if key == "to" => Some(value),
        _ => None,
    }) else {
        output.diagnostics.push(Diagnostic {
            code: "link.missing-target",
            severity: DiagnosticSeverity::Warning,
            message: "link requires a 'to' attribute".to_string(),
            range,
            related: Vec::new(),
        });
        return;
    };
    let target = attr_source_backed(source, value);
    let (target_kind, path_decoded, fragment_decoded) = classify_target(&target.value);
    let path_range = path_decoded.and_then(|decoded| target.source_range(decoded));
    let fragment_range = fragment_decoded.and_then(|decoded| target.source_range(decoded));
    output.links.push(LinkRecord {
        range,
        selection_range,
        target,
        target_kind,
        spelling: LinkSpelling::Explicit,
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

fn has_uri_scheme(target: &str) -> bool {
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
    if !value.quoted {
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
    use plumb_core::parse;

    use super::*;

    #[test]
    fn only_shorthand_ids_create_anchors() {
        let parsed = parse("`#{#intro} Heading\n`##{id=pair} Pair only\n");
        let output = analyze_document(&parsed.source, &parsed.syntax);
        assert_eq!(output.anchors.len(), 1);
        assert_eq!(output.anchors[0].id.value, "intro");
        assert_eq!(output.anchors[0].kind, AnchorKind::Heading);
    }

    #[test]
    fn verbatim_blocks_create_syntax_neutral_anchors() {
        let parsed = parse("`{#example language=text}\n  raw text\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_document(&parsed.source, &parsed.syntax);
        assert_eq!(output.anchors.len(), 1);
        assert_eq!(output.anchors[0].kind, AnchorKind::VerbatimBlock);
    }

    #[test]
    fn links_keep_component_source_ranges_through_quotes() {
        let parsed = parse("See `->[target]{to=\"docs/a.plumb#intro\"}.\n");
        let output = analyze_document(&parsed.source, &parsed.syntax);
        let link = &output.links[0];
        assert_eq!(link.spelling, LinkSpelling::Explicit);
        assert_eq!(
            &parsed.source[link.path_range.clone().unwrap()],
            "docs/a.plumb"
        );
        assert_eq!(
            &parsed.source[link.fragment_range.clone().unwrap()],
            "intro"
        );
        assert_eq!(
            link.target_kind,
            LinkTarget::Anchor {
                path: Some("docs/a.plumb".to_string()),
                fragment: "intro".to_string(),
            }
        );
    }

    #[test]
    fn link_kind_is_not_a_standard_link() {
        let parsed = parse("`link[generic]{to=\"other.plumb#target\"}\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(&parsed.source, &parsed.syntax);
        assert!(output.links.is_empty());
    }

    #[test]
    fn recognizes_inline_verbatim_autolinks_without_normalizing_the_target() {
        let source = "Visit `[https://example.test/a%20b]{.-> #site .keep rel=nofollow} or `\"[https://[::1]/]\"{.->}.\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(&parsed.source, &parsed.syntax);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(output.links.len(), 2);
        assert_eq!(output.links[0].target.value, "https://example.test/a%20b");
        assert_eq!(output.links[0].target.raw, "https://example.test/a%20b");
        assert_eq!(output.links[0].target_kind, LinkTarget::External);
        assert_eq!(output.links[1].target.value, "https://[::1]/");
    }

    #[test]
    fn recognizes_relative_autolink_targets() {
        let source = "`[other.plumb]{.->}\n`[other notes.plumb#section]{.->}\n`[../assets/a b.pdf]{.->}\n`[../assets/100% done?.pdf]{.->}\n`[#local]{.->}\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(&parsed.source, &parsed.syntax);
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
        let source = "`img[Alt `em[text]]{src=\"static/a%20b.png\" #figure .wide loading=lazy}\n`img[]{src=\"https://example.test/a.png\"}\n`img[Missing]\n`img[Empty]{src=\"\"}\n`img[Invalid]{src=\"bad path.png\"}\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(&parsed.source, &parsed.syntax);
        assert_eq!(output.images.len(), 2);
        assert_eq!(output.images[0].source.value, "static/a%20b.png");
        assert_eq!(
            output.images[0].target_kind,
            ImageTarget::File {
                path: "static/a%20b.png".to_string()
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
                "image.invalid-source"
            ]
        );
    }

    #[test]
    fn diagnoses_invalid_autolink_targets_owners_and_conflicts() {
        let source = "`[]{.->}\n`[https://example.test/bad path]{.->}\n`[https://example.test/%zz]{.->}\n`[doc.plumb#one#two]{.->}\n`[https://example.test]{.-> to=other}\n`[https://example.test]{.-> .$}\n`span[text]{.->}\n`note{.->} head\n`{.->}\n  raw\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_document(&parsed.source, &parsed.syntax);
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
                "link.conflicting-facet",
                "link.conflicting-facet",
                "link.invalid-owner",
                "link.invalid-owner",
                "link.invalid-owner",
            ]
        );
    }

    #[test]
    fn duplicate_ids_are_semantic_diagnostics() {
        let parsed = parse("`node{#same} One\n`other{#same} Two\n");
        assert!(parsed.is_valid());
        let output = analyze_document(&parsed.source, &parsed.syntax);
        assert_eq!(output.diagnostics[0].code, "anchor.duplicate-id");
    }
}
