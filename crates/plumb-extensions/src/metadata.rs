use std::collections::HashMap;
use std::ops::Range;

use plumb_core::{
    Block, Diagnostic, DiagnosticSeverity, Document, Inline, InlineContent, ParsedBlock,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionRecord {
    pub range: Range<usize>,
    pub term: InlineContent,
    pub term_range: Range<usize>,
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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MetadataOutput {
    pub definition_lists: Vec<DefinitionList>,
    pub metadata: Option<MetadataBlock>,
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
            MetadataValue::Scalar { content, .. } => Some(content.plain_text()),
            MetadataValue::Null { .. }
            | MetadataValue::List { .. }
            | MetadataValue::Map { .. }
            | MetadataValue::Verbatim { .. }
            | MetadataValue::Unsupported { .. } => None,
        }
    }
}

pub fn analyze_metadata(document: &Document) -> MetadataOutput {
    let mut output = MetadataOutput::default();
    collect_definition_lists(&document.blocks, &mut output.definition_lists);
    output
        .definition_lists
        .sort_by_key(|definitions| definitions.range.start);

    let mut first_meta = None;
    collect_metadata_blocks(&document.blocks, 0, &mut first_meta, &mut output);
    output
}

fn collect_definition_lists(blocks: &[Block], output: &mut Vec<DefinitionList>) {
    let mut index = 0;
    while index < blocks.len() {
        if definition_block(&blocks[index]).is_none() {
            if let Block::Parsed(block) = &blocks[index] {
                collect_definition_lists(&block.children, output);
            }
            index += 1;
            continue;
        }

        let start = index;
        let mut definitions = Vec::new();
        while let Some(block) = blocks.get(index).and_then(definition_block) {
            definitions.push(DefinitionRecord {
                range: block.range.clone(),
                term: block.head.clone(),
                term_range: block.head.range.clone(),
                body_range: body_range(block),
            });
            collect_definition_lists(&block.children, output);
            index += 1;
        }
        output.push(DefinitionList {
            range: blocks[start].range().start..blocks[index - 1].range().end,
            definitions,
        });
    }
}

fn collect_metadata_blocks(
    blocks: &[Block],
    depth: usize,
    first_meta: &mut Option<Range<usize>>,
    output: &mut MetadataOutput,
) {
    for block in blocks {
        let Block::Parsed(parsed) = block else {
            continue;
        };
        if marker(parsed) == Some("meta") {
            if depth != 0 {
                output.diagnostics.push(warning(
                    "metadata.nested-block",
                    "metadata blocks must be document-level blocks",
                    parsed.range.clone(),
                ));
            } else if let Some(first) = first_meta.as_ref() {
                let mut diagnostic = warning(
                    "metadata.multiple-blocks",
                    "a document may contain only one metadata block",
                    parsed.range.clone(),
                );
                diagnostic.related.push(first.clone());
                output.diagnostics.push(diagnostic);
            } else {
                *first_meta = Some(parsed.range.clone());
                if !parsed.head.items.is_empty() {
                    output.diagnostics.push(warning(
                        "metadata.nonempty-head",
                        "the metadata block must not have a head",
                        parsed.head.range.clone(),
                    ));
                }
                let entries = parse_entries(&parsed.children, &mut output.diagnostics);
                output.metadata = Some(MetadataBlock {
                    range: parsed.range.clone(),
                    selection_range: parsed
                        .mark
                        .as_ref()
                        .expect("metadata block has a marker")
                        .marker_range
                        .clone(),
                    entries,
                });
            }
        }
        collect_metadata_blocks(&parsed.children, depth + 1, first_meta, output);
    }
}

fn parse_entries(blocks: &[Block], diagnostics: &mut Vec<Diagnostic>) -> Vec<MetadataEntry> {
    let mut entries = Vec::new();
    let mut keys: HashMap<String, Range<usize>> = HashMap::new();
    for block in blocks {
        let Some(definition) = definition_block(block) else {
            diagnostics.push(warning(
                "metadata.expected-definition",
                "metadata children must use the ':' definition marker",
                block.range().clone(),
            ));
            continue;
        };
        let key_range = definition.head.range.clone();
        let Some(key) = metadata_key(definition) else {
            diagnostics.push(warning(
                "metadata.invalid-key",
                "metadata keys must be nonempty plain text without whitespace",
                key_range,
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
        let value = parse_value(definition, diagnostics);
        entries.push(MetadataEntry {
            range: definition.range.clone(),
            key,
            key_range,
            value,
        });
    }
    entries
}

fn parse_value(block: &ParsedBlock, diagnostics: &mut Vec<Diagnostic>) -> MetadataValue {
    let range = body_range(block);
    if block.children.is_empty() {
        return MetadataValue::Null { range };
    }
    if block.children.len() == 1 {
        match &block.children[0] {
            Block::Parsed(child) if child.mark.is_none() => {
                if let Some(text) = inline_verbatim(&child.head) {
                    return MetadataValue::Verbatim {
                        text: text.to_string(),
                        range,
                    };
                }
                return MetadataValue::Scalar {
                    content: child.head.clone(),
                    range,
                };
            }
            Block::Verbatim(child) => {
                return MetadataValue::Verbatim {
                    text: child.text.clone(),
                    range,
                };
            }
            Block::Parsed(_) => {}
        }
    }
    if block
        .children
        .iter()
        .all(|child| parsed_marker(child) == Some("-"))
    {
        let mut items = Vec::new();
        for child in &block.children {
            let Block::Parsed(item) = child else {
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
                parse_value(item, diagnostics)
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
            items.push(MetadataListItem {
                value,
                range: item.range.clone(),
            });
        }
        return MetadataValue::List { items, range };
    }
    if block
        .children
        .iter()
        .all(|child| definition_block(child).is_some())
    {
        return MetadataValue::Map {
            entries: parse_entries(&block.children, diagnostics),
            range,
        };
    }

    diagnostics.push(warning(
        "metadata.unsupported-value",
        "metadata values must be a paragraph, list, definition map, verbatim block, or empty",
        range.clone(),
    ));
    MetadataValue::Unsupported { range }
}

fn inline_verbatim(content: &InlineContent) -> Option<&str> {
    let [Inline::Verbatim { text, attrs, .. }] = content.items.as_slice() else {
        return None;
    };
    attrs.items.is_empty().then_some(text)
}

fn metadata_key(block: &ParsedBlock) -> Option<String> {
    if block.head.items.is_empty()
        || !block
            .head
            .items
            .iter()
            .all(|item| matches!(item, Inline::Text { .. }))
    {
        return None;
    }
    let key = block.head.plain_text();
    (!key.is_empty() && !key.chars().any(char::is_whitespace)).then_some(key)
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
    block
        .children
        .first()
        .zip(block.children.last())
        .map_or(block.range.end..block.range.end, |(first, last)| {
            first.range().start..last.range().end
        })
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
    use plumb_core::parse;

    use super::*;

    #[test]
    fn groups_definition_lists_and_projects_metadata_values() {
        let parsed = parse(
            "`: term\n\n  Definition.\n\n`meta\n  `: title\n\n    Document `em[title]\n\n  `: tags\n    `- plumb\n    `- parser\n\n  `: macros\n    `-\n      `- `[name]\n      `- `[expansion]\n      `- 1\n\n  `: author\n    `: name\n\n      Alice\n\n  `: source\n    `{language=text}\n      raw\n",
        );
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_metadata(&parsed.syntax);
        assert_eq!(output.definition_lists.len(), 3);
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
    fn document_title_requires_a_scalar_value() {
        let parsed =
            parse("`meta\n  `: title\n    `- Not a scalar\n\n  `: title\n\n    Later scalar\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        assert_eq!(analyze_metadata(&parsed.syntax).document_title(), None);
    }

    #[test]
    fn item_marker_is_not_a_metadata_list_item() {
        let parsed = parse("`meta\n  `: tags\n    `item Generic block\n");
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
    fn diagnoses_metadata_profile_violations() {
        let parsed = parse(
            "`meta head\n  `: bad key\n\n    value\n  `: duplicate\n  `: duplicate\n  paragraph\n\n`meta\n  `: other\n",
        );
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_metadata(&parsed.syntax);
        let codes = output
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();
        assert!(codes.contains(&"metadata.nonempty-head"));
        assert!(codes.contains(&"metadata.invalid-key"));
        assert!(codes.contains(&"metadata.duplicate-key"));
        assert!(codes.contains(&"metadata.expected-definition"));
        assert!(codes.contains(&"metadata.multiple-blocks"));
    }
}
