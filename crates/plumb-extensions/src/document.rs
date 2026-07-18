use std::collections::HashMap;
use std::ops::Range;

use plumb_core::{
    AttrItem, AttrValue, Attributes, Block, Diagnostic, DiagnosticSeverity, Document, Inline,
    InlineContent,
};

use crate::{analyze_headings, HeadingOutput};

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
    CodeBlock,
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
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkRecord {
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
    pub target: SourceBacked<String>,
    pub target_kind: LinkTarget,
    pub path_range: Option<Range<usize>>,
    pub fragment_range: Option<Range<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DocumentOutput {
    pub headings: HeadingOutput,
    pub anchors: Vec<AnchorRecord>,
    pub links: Vec<LinkRecord>,
    pub diagnostics: Vec<Diagnostic>,
}

impl DocumentOutput {
    pub fn link_at_node_start(&self, start: usize) -> Option<&LinkRecord> {
        self.links.iter().find(|link| link.range.start == start)
    }
}

pub fn analyze_document(source: &str, document: &Document) -> DocumentOutput {
    let headings = analyze_headings(document);
    let mut output = DocumentOutput {
        headings,
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
            Block::Code(code) => collect_anchor(
                source,
                &code.attrs,
                AnchorKind::CodeBlock,
                code.range.clone(),
                code.marker_range.clone(),
                first_ids,
                output,
            ),
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
                collect_anchor(
                    source,
                    attrs,
                    AnchorKind::Inline,
                    range.clone(),
                    content.range.clone(),
                    first_ids,
                    output,
                );
                if kind.as_deref() == Some("link") {
                    collect_link(source, range.clone(), content.range.clone(), attrs, output);
                }
                collect_inlines(source, content, first_ids, output);
            }
            Inline::Verbatim { range, attrs, .. } => collect_anchor(
                source,
                attrs,
                AnchorKind::Inline,
                range.clone(),
                range.clone(),
                first_ids,
                output,
            ),
            Inline::Text { .. } | Inline::SoftBreak { .. } => {}
        }
    }
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
        path_range,
        fragment_range,
    });
}

fn classify_target(target: &str) -> (LinkTarget, Option<Range<usize>>, Option<Range<usize>>) {
    if target.contains("://") || target.starts_with("mailto:") {
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
        None => return (LinkTarget::Other, None, None),
    };
    if fragment.is_empty() || (!path.is_empty() && !is_plumb_path(path)) {
        return (LinkTarget::Other, None, None);
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

fn attr_source_backed(source: &str, value: &AttrValue) -> SourceBacked<String> {
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
        let parsed = parse("`# Heading\n`##{id=pair} Pair only\n`##{#real} Real\n");
        let output = analyze_document(&parsed.source, &parsed.syntax);
        assert_eq!(output.anchors.len(), 1);
        assert_eq!(output.anchors[0].id.value, "real");
        assert_eq!(output.anchors[0].kind, AnchorKind::Heading);
    }

    #[test]
    fn links_keep_component_source_ranges_through_quotes() {
        let parsed = parse("See `link[target]{to=\"docs/a.plumb#intro\"}.\n");
        let output = analyze_document(&parsed.source, &parsed.syntax);
        let link = &output.links[0];
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
    fn duplicate_ids_are_semantic_diagnostics() {
        let parsed = parse("`node{#same} One\n`other{#same} Two\n");
        assert!(parsed.is_valid());
        let output = analyze_document(&parsed.source, &parsed.syntax);
        assert_eq!(output.diagnostics[0].code, "anchor.duplicate-id");
    }
}
