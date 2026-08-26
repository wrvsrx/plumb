use std::collections::HashMap;
use std::ops::Range;

use chrono::DateTime;
use plumb_syntax::{
    Block, Diagnostic, DiagnosticSeverity, Document, Inline, InlineContent, InlineMember,
    ParsedBlock,
};

use crate::text::plain_text;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionRecord {
    pub range: Range<usize>,
    pub term: InlineContent,
    pub term_range: Range<usize>,
    pub inline_body: Option<InlineContent>,
    pub body_range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionList {
    pub range: Range<usize>,
    pub definitions: Vec<DefinitionRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataBlock {
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
    pub entries: Vec<MetadataEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentFacet {
    pub range: Range<usize>,
    pub value: String,
    pub value_range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataEntry {
    pub range: Range<usize>,
    pub key: String,
    pub key_range: Range<usize>,
    pub value: MetadataValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataValue {
    Null {
        range: Range<usize>,
    },
    Scalar {
        content: InlineContent,
        range: Range<usize>,
    },
    List {
        items: Vec<MetadataListItem>,
        range: Range<usize>,
    },
    Map {
        entries: Vec<MetadataEntry>,
        range: Range<usize>,
    },
    Verbatim {
        text: String,
        range: Range<usize>,
    },
    Unsupported {
        range: Range<usize>,
    },
}

impl MetadataValue {
    pub fn range(&self) -> &Range<usize> {
        match self {
            Self::Null { range }
            | Self::Scalar { range, .. }
            | Self::List { range, .. }
            | Self::Map { range, .. }
            | Self::Verbatim { range, .. }
            | Self::Unsupported { range } => range,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataListItem {
    pub value: MetadataValue,
    pub range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BibliographySource {
    pub value: String,
    pub range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MetadataOutput {
    pub definition_lists: Vec<DefinitionList>,
    pub metadata: Option<MetadataBlock>,
    pub facets: Vec<DocumentFacet>,
    pub diagnostics: Vec<Diagnostic>,
}

impl MetadataOutput {
    pub fn definition_list_at_node_start(&self, start: usize) -> Option<&DefinitionList> {
        self.definition_lists
            .iter()
            .find(|definitions| definitions.range.start == start)
    }

    pub fn document_title(&self) -> Option<String> {
        let metadata = self.metadata.as_ref()?;
        let entry = metadata.entries.iter().find(|entry| entry.key == "title")?;
        match &entry.value {
            MetadataValue::Scalar { content, .. } => Some(plain_text(content)),
            MetadataValue::Null { .. }
            | MetadataValue::List { .. }
            | MetadataValue::Map { .. }
            | MetadataValue::Verbatim { .. }
            | MetadataValue::Unsupported { .. } => None,
        }
    }

    pub fn bibliography_sources(&self) -> Vec<BibliographySource> {
        let Some(entry) = self.metadata.as_ref().and_then(|metadata| {
            metadata
                .entries
                .iter()
                .find(|entry| entry.key == "bibliography")
        }) else {
            return Vec::new();
        };
        match &entry.value {
            MetadataValue::List { items, .. } => items
                .iter()
                .filter_map(|item| bibliography_source(&item.value))
                .collect(),
            value => bibliography_source(value).into_iter().collect(),
        }
    }
}

fn bibliography_source(value: &MetadataValue) -> Option<BibliographySource> {
    match value {
        MetadataValue::Scalar { content, range } if inline_verbatim(content).is_some() => {
            Some(BibliographySource {
                value: inline_verbatim(content)?.to_string(),
                range: range.clone(),
            })
        }
        MetadataValue::Scalar { content, range }
            if !content.items.is_empty()
                && content
                    .items
                    .iter()
                    .all(|inline| matches!(inline, Inline::Text { .. } | Inline::Space { .. })) =>
        {
            let value = content.plain_text();
            (!value.is_empty()).then(|| BibliographySource {
                value,
                range: range.clone(),
            })
        }
        MetadataValue::Verbatim { text, range } if !text.is_empty() => Some(BibliographySource {
            value: text.clone(),
            range: range.clone(),
        }),
        _ => None,
    }
}

pub fn analyze_metadata(document: &Document) -> MetadataOutput {
    let mut output = MetadataOutput::default();
    collect_definition_lists(
        document
            .blocks
            .iter()
            .filter(|block| !crate::is_document_declaration(block)),
        &mut output.definition_lists,
    );
    output
        .definition_lists
        .sort_by_key(|definitions| definitions.range.start);

    let properties = document
        .blocks
        .iter()
        .filter(|block| parsed_marker(block) == Some("="))
        .collect::<Vec<_>>();
    if let (Some(first), Some(last)) = (properties.first(), properties.last()) {
        let entries = parse_direct_entries(properties.iter().copied(), &mut output.diagnostics);
        lint_standard_entries(&entries, &mut output.diagnostics);
        let selection_range = match first {
            Block::Parsed(block) => block
                .mark
                .as_ref()
                .expect("metadata property has a marker")
                .marker_range
                .clone(),
            Block::Verbatim(_) => unreachable!("metadata property is parsed"),
        };
        output.metadata = Some(MetadataBlock {
            range: first.range().start..last.range().end,
            selection_range,
            entries,
        });
    }

    for block in &document.blocks {
        let Block::Parsed(block) = block else {
            continue;
        };
        match marker(block) {
            Some("+") => match document_facet(block) {
                Some(facet) => output.facets.push(facet),
                None => output.diagnostics.push(warning(
                    "document.invalid-facet",
                    "document facets require a nonempty plain head and no children",
                    block.range.clone(),
                )),
            },
            Some("@") => output.diagnostics.push(warning(
                "document.unsupported-identity",
                "document identity is defined by its workspace-relative path",
                block.range.clone(),
            )),
            Some(_) | None => {}
        }
    }
    output
}

fn document_facet(block: &ParsedBlock) -> Option<DocumentFacet> {
    if !block.children.is_empty()
        || block.head.items.is_empty()
        || !block
            .head
            .items
            .iter()
            .all(|inline| matches!(inline, Inline::Text { .. } | Inline::Space { .. }))
    {
        return None;
    }
    let value = block.head.plain_text();
    if value.trim().is_empty() {
        return None;
    }
    Some(DocumentFacet {
        range: block.range.clone(),
        value,
        value_range: block.head.range.clone(),
    })
}

fn parse_direct_entries<'a>(
    blocks: impl IntoIterator<Item = &'a Block>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<MetadataEntry> {
    let mut entries = Vec::new();
    let mut keys: HashMap<String, Range<usize>> = HashMap::new();
    for block in blocks {
        let Block::Parsed(property) = block else {
            diagnostics.push(warning(
                "metadata.expected-property",
                "document metadata declarations must be named block elements",
                block.range().clone(),
            ));
            continue;
        };
        let Some(mark) = property.mark.as_ref() else {
            diagnostics.push(warning(
                "metadata.expected-property",
                "document metadata declarations must be named block elements",
                block.range().clone(),
            ));
            continue;
        };
        if mark.marker != "=" {
            diagnostics.push(warning(
                "metadata.expected-property",
                "document metadata declarations must use the '=' association marker",
                mark.marker_range.clone(),
            ));
            continue;
        }
        let Some((key, key_range, scalar)) = direct_property_parts(property) else {
            diagnostics.push(warning(
                "metadata.invalid-key",
                "metadata keys must be nonempty plain text",
                property.head.range.clone(),
            ));
            continue;
        };
        if let Some(first) = keys.get(&key) {
            let mut diagnostic = warning(
                "metadata.duplicate-key",
                format!("metadata key '{key}' appears more than once"),
                key_range.clone(),
            );
            diagnostic.related.push(first.clone());
            diagnostics.push(diagnostic);
        } else {
            keys.insert(key.clone(), key_range.clone());
        }
        entries.push(MetadataEntry {
            range: property.range.clone(),
            key,
            key_range,
            value: parse_direct_value(property, scalar, diagnostics),
        });
    }
    entries
}

fn parse_direct_value(
    property: &ParsedBlock,
    scalar: Option<InlineContent>,
    diagnostics: &mut Vec<Diagnostic>,
) -> MetadataValue {
    if let Some(scalar) = scalar {
        if !property.children.is_empty() {
            diagnostics.push(warning(
                "metadata.unsupported-value",
                "metadata properties with scalar content cannot also have child blocks",
                property.range.clone(),
            ));
            return MetadataValue::Unsupported {
                range: property.range.clone(),
            };
        }
        return match inline_verbatim(&scalar) {
            Some(text) => MetadataValue::Verbatim {
                text: text.to_string(),
                range: scalar.range.clone(),
            },
            None => MetadataValue::Scalar {
                range: scalar.range.clone(),
                content: scalar,
            },
        };
    }
    parse_direct_children(&property.children, body_range(property), diagnostics)
}

fn direct_property_parts(
    property: &ParsedBlock,
) -> Option<(String, Range<usize>, Option<InlineContent>)> {
    let head = &property.head;
    if !property.children.is_empty() {
        let key = plain_association_key(head)?;
        return Some((key, head.range.clone(), None));
    }
    let first = head.items.first()?;
    let (key, key_range) = match first {
        Inline::Text { text, range }
            if !text.is_empty() && !text.chars().any(char::is_whitespace) =>
        {
            (text.clone(), range.clone())
        }
        Inline::Element {
            kind,
            members,
            attrs,
            range,
            ..
        } if kind == "()" && attrs.items.is_empty() => {
            let mut arguments = members.iter().filter_map(InlineMember::argument);
            let argument = arguments.next()?;
            if arguments.next().is_some() {
                return None;
            }
            let key = argument.plain_text();
            if key.is_empty() {
                return None;
            }
            (key, range.clone())
        }
        _ => return None,
    };
    match head.items.as_slice() {
        [_] => Some((key, key_range, None)),
        [_, Inline::Space { .. }, value @ ..] if !value.is_empty() => {
            let start = inline_source_range(value.first()?).start;
            let end = inline_source_range(value.last()?).end;
            Some((
                key,
                key_range,
                Some(InlineContent {
                    range: start..end,
                    items: value.to_vec(),
                }),
            ))
        }
        _ => None,
    }
}

fn inline_source_range(inline: &Inline) -> &Range<usize> {
    match inline {
        Inline::Text { range, .. }
        | Inline::Space { range, .. }
        | Inline::SoftBreak { range }
        | Inline::Element { range, .. }
        | Inline::Verbatim { range, .. } => range,
    }
}

fn parse_direct_children(
    blocks: &[Block],
    range: Range<usize>,
    diagnostics: &mut Vec<Diagnostic>,
) -> MetadataValue {
    if blocks.is_empty() {
        return MetadataValue::Null { range };
    }
    if let [Block::Verbatim(block)] = blocks {
        return MetadataValue::Verbatim {
            text: block.text.clone(),
            range,
        };
    }
    if let [Block::Parsed(block)] = blocks {
        if block.mark.is_none() && block.children.is_empty() {
            return match inline_verbatim(&block.head) {
                Some(text) => MetadataValue::Verbatim {
                    text: text.to_string(),
                    range,
                },
                None => MetadataValue::Scalar {
                    content: block.head.clone(),
                    range,
                },
            };
        }
    }
    if blocks.iter().all(|block| parsed_marker(block) == Some("-")) {
        let items = blocks
            .iter()
            .map(|block| {
                let Block::Parsed(item) = block else {
                    unreachable!("dash marker implies parsed block");
                };
                let value = if item.children.is_empty() {
                    match inline_verbatim(&item.head) {
                        Some(text) => MetadataValue::Verbatim {
                            text: text.to_string(),
                            range: item.head.range.clone(),
                        },
                        None => MetadataValue::Scalar {
                            content: item.head.clone(),
                            range: item.head.range.clone(),
                        },
                    }
                } else if item.head.items.is_empty() {
                    parse_direct_children(&item.children, body_range(item), diagnostics)
                } else {
                    diagnostics.push(warning(
                        "metadata.invalid-list-item",
                        "metadata list items with child blocks must have an empty head",
                        item.range.clone(),
                    ));
                    MetadataValue::Unsupported {
                        range: item.range.clone(),
                    }
                };
                MetadataListItem {
                    value,
                    range: item.range.clone(),
                }
            })
            .collect();
        return MetadataValue::List { items, range };
    }
    if blocks.iter().all(|block| parsed_marker(block) == Some("=")) {
        return MetadataValue::Map {
            entries: parse_direct_entries(blocks, diagnostics),
            range,
        };
    }
    diagnostics.push(warning(
        "metadata.unsupported-value",
        "metadata values must be scalar content, a sequence, map, verbatim block, or empty",
        range.clone(),
    ));
    MetadataValue::Unsupported { range }
}

fn collect_definition_lists<'a>(
    blocks: impl IntoIterator<Item = &'a Block>,
    output: &mut Vec<DefinitionList>,
) {
    let mut blocks = blocks.into_iter().peekable();
    while let Some(current) = blocks.next() {
        if definition_block(current).is_none() {
            if let Block::Parsed(block) = current {
                collect_definition_lists(crate::body_children(block), output);
            }
            continue;
        }

        let mut definitions = Vec::new();
        let start = current.range().start;
        let mut current = Some(current);
        while let Some(block) = current.and_then(definition_block) {
            let (term, inline_body) = if block.children.is_empty() {
                split_inline_arguments(&block.head)
            } else {
                (block.head.clone(), None)
            };
            let projected_body_range = inline_body
                .as_ref()
                .map_or_else(|| body_range(block), |body| body.range.clone());
            definitions.push(DefinitionRecord {
                range: block.range.clone(),
                term_range: term.range.clone(),
                term,
                inline_body,
                body_range: projected_body_range,
            });
            collect_definition_lists(crate::body_children(block), output);
            current = blocks.next_if(|next| definition_block(next).is_some());
        }
        output.push(DefinitionList {
            range: start
                ..definitions
                    .last()
                    .expect("definition list is nonempty")
                    .range
                    .end,
            definitions,
        });
    }
}

fn lint_standard_entries(entries: &[MetadataEntry], diagnostics: &mut Vec<Diagnostic>) {
    for entry in entries.iter().filter(|entry| entry.key == "created") {
        let valid = match &entry.value {
            MetadataValue::Scalar { content, .. } => {
                DateTime::parse_from_rfc3339(&content.plain_text()).is_ok()
            }
            MetadataValue::Null { .. }
            | MetadataValue::List { .. }
            | MetadataValue::Map { .. }
            | MetadataValue::Verbatim { .. }
            | MetadataValue::Unsupported { .. } => false,
        };
        if !valid {
            diagnostics.push(warning(
                "metadata.invalid-created",
                "metadata 'created' must be a complete RFC 3339 timestamp",
                entry.value.range().clone(),
            ));
        }
    }
}

fn inline_verbatim(content: &InlineContent) -> Option<&str> {
    let [Inline::Verbatim {
        kind, text, attrs, ..
    }] = content.items.as_slice()
    else {
        return None;
    };
    (kind.is_empty() && attrs.items.is_empty()).then_some(text)
}

fn plain_association_key(content: &InlineContent) -> Option<String> {
    if let [Inline::Element {
        kind,
        members,
        attrs,
        ..
    }] = content.items.as_slice()
    {
        let mut arguments = members.iter().filter_map(InlineMember::argument);
        if kind == "()" && attrs.items.is_empty() {
            let argument = arguments.next()?;
            if arguments.next().is_some() {
                return None;
            }
            let key = argument.plain_text();
            return (!key.is_empty()).then_some(key);
        }
    }
    if content.items.is_empty()
        || content.items.iter().any(|item| {
            !matches!(
                item,
                Inline::Text { .. } | Inline::Space { .. } | Inline::SoftBreak { .. }
            )
        })
    {
        return None;
    }
    let key = content.plain_text();
    (!key.is_empty()).then_some(key)
}

fn split_inline_arguments(content: &InlineContent) -> (InlineContent, Option<InlineContent>) {
    let Some(first) = content.items.first() else {
        return (content.clone(), None);
    };
    let term_range = inline_source_range(first).clone();
    let term = InlineContent {
        range: term_range,
        items: vec![first.clone()],
    };
    let [_, Inline::Space { .. }, value @ ..] = content.items.as_slice() else {
        return (term, None);
    };
    let Some(last) = value.last() else {
        return (term, None);
    };
    let body = InlineContent {
        range: inline_source_range(value.first().expect("nonempty value")).start
            ..inline_source_range(last).end,
        items: value.to_vec(),
    };
    (term, Some(body))
}

fn definition_block(block: &Block) -> Option<&ParsedBlock> {
    let Block::Parsed(block) = block else {
        return None;
    };
    (marker(block) == Some(":")).then_some(block)
}

fn parsed_marker(block: &Block) -> Option<&str> {
    let Block::Parsed(block) = block else {
        return None;
    };
    marker(block)
}

fn marker(block: &ParsedBlock) -> Option<&str> {
    block.mark.as_ref().map(|mark| mark.marker.as_str())
}

fn body_range(block: &ParsedBlock) -> Range<usize> {
    let mut children = block.children.iter();
    let Some(first) = children.next() else {
        return block.range.end..block.range.end;
    };
    let last = children.next_back().unwrap_or(first);
    first.range().start..last.range().end
}

fn warning(code: &'static str, message: impl Into<String>, range: Range<usize>) -> Diagnostic {
    Diagnostic {
        code,
        severity: DiagnosticSeverity::Warning,
        message: message.into(),
        range,
        related: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use plumb_syntax::parse;

    use super::*;

    #[test]
    fn groups_definition_lists_and_projects_metadata_values() {
        let parsed = parse(
            "`= title Document `em[title]\n`= tags\n\n `- plumb\n `- parser\n\n`= macros\n\n `-\n  `- `\"name\"\n  `- `\"expansion\"\n  `- 1\n\n`= author\n\n `= name Alice\n\n`= source\n\n `\"\n  raw\n\n`: term\n\n Definition.\n",
        );
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_metadata(&parsed.syntax);
        assert_eq!(output.definition_lists.len(), 1);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(output.document_title().as_deref(), Some("Document title"));
        let metadata = output.metadata.unwrap();
        assert_eq!(metadata.entries.len(), 5);
        assert!(matches!(
            metadata.entries[0].value,
            MetadataValue::Scalar { .. }
        ));
        assert!(matches!(
            metadata.entries[1].value,
            MetadataValue::List { .. }
        ));
        assert!(matches!(
            metadata.entries[2].value,
            MetadataValue::List { ref items, .. }
                if matches!(items[0].value, MetadataValue::List { .. })
        ));
        assert!(matches!(
            metadata.entries[3].value,
            MetadataValue::Map { .. }
        ));
        assert!(matches!(
            metadata.entries[4].value,
            MetadataValue::Verbatim { .. }
        ));
    }

    #[test]
    fn projects_recursive_document_metadata() {
        let parsed = parse(
            "`= title Document `em[title]\n`= tags\n `- plumb\n `- parser\n`= macros\n `-\n  `- `\"name\"\n  `- `\"expansion\"\n`= author\n `= name Alice\n`= empty\n\nBody.\n",
        );
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_metadata(&parsed.syntax);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(output.document_title().as_deref(), Some("Document title"));
        let metadata = output.metadata.unwrap();
        assert_eq!(metadata.entries.len(), 5);
        assert!(matches!(
            metadata.entries[0].value,
            MetadataValue::Scalar { .. }
        ));
        assert!(matches!(
            metadata.entries[1].value,
            MetadataValue::List { .. }
        ));
        assert!(matches!(
            metadata.entries[2].value,
            MetadataValue::List { ref items, .. }
                if matches!(items[0].value, MetadataValue::List { .. })
        ));
        assert!(matches!(
            metadata.entries[3].value,
            MetadataValue::Map { .. }
        ));
        assert!(matches!(
            metadata.entries[4].value,
            MetadataValue::Null { .. }
        ));
    }

    #[test]
    fn projects_plain_and_literal_bibliography_sources() {
        let parsed =
            parse("`= bibliography\n `- refs/library one.json\n `- `\"refs/library-two.json\"\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_metadata(&parsed.syntax);
        let sources = output.bibliography_sources();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].value, "refs/library one.json");
        assert_eq!(sources[1].value, "refs/library-two.json");
    }

    #[test]
    fn declarations_can_interleave_with_body_and_project_facets() {
        let parsed = parse(
            "`= title Root\n\nBody before.\n\n`+ journal\n\nBody after.\n\n`= created 2026-08-26T00:00:00+08:00\n",
        );
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_metadata(&parsed.syntax);
        assert_eq!(output.document_title().as_deref(), Some("Root"));
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(
            output
                .metadata
                .unwrap()
                .entries
                .iter()
                .map(|entry| entry.key.as_str())
                .collect::<Vec<_>>(),
            ["title", "created"]
        );
        assert_eq!(output.facets[0].value, "journal");
    }

    #[test]
    fn document_title_requires_a_scalar_value() {
        let parsed = parse("`= title\n `- Not a scalar\n\n`= title Later scalar\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        assert_eq!(analyze_metadata(&parsed.syntax).document_title(), None);
    }

    #[test]
    fn item_marker_is_not_a_metadata_list_item() {
        let parsed = parse("`= tags\n `item Generic block\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_metadata(&parsed.syntax);
        assert!(matches!(
            output.metadata.unwrap().entries[0].value,
            MetadataValue::Unsupported { .. }
        ));
        assert!(output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "metadata.unsupported-value"));
    }

    #[test]
    fn ordered_marker_is_not_a_metadata_list_item() {
        let parsed = parse("`= ranking\n `. First\n `. Second\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_metadata(&parsed.syntax);
        assert!(matches!(
            output.metadata.unwrap().entries[0].value,
            MetadataValue::Unsupported { .. }
        ));
        assert!(output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "metadata.unsupported-value"));
    }

    #[test]
    fn diagnoses_document_declaration_violations() {
        let parsed = parse(
            "`= `*[bad key] value\n`= duplicate\n`= duplicate\n`+\n`+ `*[not plain]\n`@ forbidden\n",
        );
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_metadata(&parsed.syntax);
        let codes = output
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();
        assert!(codes.contains(&"metadata.invalid-key"));
        assert!(codes.contains(&"metadata.duplicate-key"));
        assert_eq!(
            codes
                .iter()
                .filter(|code| **code == "document.invalid-facet")
                .count(),
            2
        );
        assert!(codes.contains(&"document.unsupported-identity"));
    }

    #[test]
    fn definitions_split_compact_heads_but_keep_heads_with_children_whole() {
        let source = "`: term inline body\n\n`: term with spaces\n\n  child body\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_metadata(&parsed.syntax);
        let definitions = &output.definition_lists[0].definitions;
        assert_eq!(definitions.len(), 2);
        assert_eq!(definitions[0].term.plain_text(), "term");
        assert_eq!(
            definitions[0]
                .inline_body
                .as_ref()
                .map(InlineContent::plain_text)
                .as_deref(),
            Some("inline body")
        );
        assert_eq!(&source[definitions[0].body_range.clone()], "inline body");
        assert_eq!(definitions[1].term.plain_text(), "term with spaces");
        assert!(definitions[1].inline_body.is_none());
        assert_eq!(
            &source[definitions[1].term_range.clone()],
            "term with spaces"
        );
    }

    #[test]
    fn document_metadata_uses_compact_or_child_bounded_association_keys() {
        let source = "`= title plumb title\n`= `()[key with spaces] inline value\n`= child key with spaces\n\n child value\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_metadata(&parsed.syntax);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let metadata = output.metadata.unwrap();
        assert_eq!(
            metadata
                .entries
                .iter()
                .map(|entry| entry.key.as_str())
                .collect::<Vec<_>>(),
            ["title", "key with spaces", "child key with spaces"]
        );
        let scalar = |entry: &MetadataEntry| match &entry.value {
            MetadataValue::Scalar { content, .. } => content.plain_text(),
            other => panic!("expected scalar metadata, got {other:?}"),
        };
        assert_eq!(scalar(&metadata.entries[0]), "plumb title");
        assert_eq!(scalar(&metadata.entries[1]), "inline value");
        assert_eq!(scalar(&metadata.entries[2]), "child value");
    }

    #[test]
    fn lints_only_the_standard_created_timestamp() {
        let parsed = parse("`= created 2026-07-22T12:34:56+08:00\n`= custom not-a-date\n");
        let output = analyze_metadata(&parsed.syntax);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);

        let parsed = parse("`= created 2026-07-22 12:34:56\n");
        let output = analyze_metadata(&parsed.syntax);
        let diagnostic = output
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "metadata.invalid-created")
            .expect("invalid created diagnostic");
        assert_eq!(diagnostic.severity, DiagnosticSeverity::Warning);
        assert_eq!(
            &parsed.source[diagnostic.range.clone()],
            "2026-07-22 12:34:56"
        );
    }

    #[test]
    fn rejects_non_scalar_created_values() {
        let parsed = parse("`= created\n `- 2026-07-22T12:34:56+08:00\n");
        let output = analyze_metadata(&parsed.syntax);
        assert!(output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "metadata.invalid-created"));
    }
}
