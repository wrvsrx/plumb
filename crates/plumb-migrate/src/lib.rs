use std::{collections::HashMap, fmt, ops::Range};

use member_legacy::{Block, Inline, InlineMember, ParsedBlock};
use plumb_edit::{
    apply_text_edits, MarkedOwnerRewrite, OwnedAttribute, OwnedBlock, OwnedDocument, OwnedInline,
    OwnedInlineMember, OwnedValue, TextEdit,
};
use plumb_syntax_legacy_v1 as legacy;
use plumb_syntax_legacy_v2 as member_legacy;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationError {
    InvalidLegacy(Vec<MigrationDiagnostic>),
    InvalidDocumentGroup(Vec<MigrationDiagnostic>),
    InvalidHeadSpace(Vec<MigrationDiagnostic>),
    InvalidTaskEventMarkers(Vec<MigrationDiagnostic>),
    InvalidMemberEnvelope(Vec<MigrationDiagnostic>),
    UnsupportedAttachedInline { range: legacy::SourceRange },
    ConflictingLinkTarget { range: legacy::SourceRange },
    InvalidGenerated,
}

#[cfg(test)]
mod member_envelope_tests {
    use super::*;

    #[test]
    fn migrates_arguments_inline_members_and_raw_tails() {
        let source = concat!(
            "`= title|Project Guide\n\n",
            "See `->[guide page|Project Guide.plumb].\n\n",
            "`code\n `@ example\n|\"\n raw bytes\n",
        );
        let migrated = migrate_member_envelope_v1(source).unwrap();
        assert!(plumb_syntax::parse(&migrated).is_valid(), "{migrated}");
        assert!(migrated.contains("`= title Project Guide"), "{migrated}");
        assert!(
            migrated.contains("`->{{guide page} {Project Guide.plumb}}"),
            "{migrated}"
        );
        assert!(migrated.contains("`code\""), "{migrated}");
        assert!(migrated.contains("raw bytes"), "{migrated}");
        assert_eq!(migrate_member_envelope_v1(&migrated).unwrap(), migrated);
    }

    #[test]
    fn rejects_invalid_member_envelope_source() {
        let error = migrate_member_envelope_v1("`broken[\n").unwrap_err();
        assert!(matches!(error, MigrationError::InvalidMemberEnvelope(_)));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationDiagnostic {
    pub code: &'static str,
    pub range: legacy::SourceRange,
    pub message: String,
}

impl fmt::Display for MigrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLegacy(diagnostics) => {
                write!(formatter, "legacy source is invalid")?;
                for diagnostic in diagnostics {
                    write!(
                        formatter,
                        "; {} at bytes {}..{}: {}",
                        diagnostic.code,
                        diagnostic.range.start,
                        diagnostic.range.end,
                        diagnostic.message
                    )?;
                }
                Ok(())
            }
            Self::InvalidDocumentGroup(diagnostics) => {
                write!(formatter, "document-group-v1 source is invalid")?;
                for diagnostic in diagnostics {
                    write!(
                        formatter,
                        "; {} at bytes {}..{}: {}",
                        diagnostic.code,
                        diagnostic.range.start,
                        diagnostic.range.end,
                        diagnostic.message
                    )?;
                }
                Ok(())
            }
            Self::InvalidHeadSpace(diagnostics) => {
                write!(formatter, "head-space-v1 source is invalid")?;
                for diagnostic in diagnostics {
                    write!(
                        formatter,
                        "; {} at bytes {}..{}: {}",
                        diagnostic.code,
                        diagnostic.range.start,
                        diagnostic.range.end,
                        diagnostic.message
                    )?;
                }
                Ok(())
            }
            Self::InvalidTaskEventMarkers(diagnostics) => {
                write!(formatter, "task-event-markers-v1 source is invalid")?;
                for diagnostic in diagnostics {
                    write!(
                        formatter,
                        "; {} at bytes {}..{}: {}",
                        diagnostic.code,
                        diagnostic.range.start,
                        diagnostic.range.end,
                        diagnostic.message
                    )?;
                }
                Ok(())
            }
            Self::InvalidMemberEnvelope(diagnostics) => {
                write!(formatter, "member-envelope-v1 source is invalid")?;
                for diagnostic in diagnostics {
                    write!(
                        formatter,
                        "; {} at bytes {}..{}: {}",
                        diagnostic.code,
                        diagnostic.range.start,
                        diagnostic.range.end,
                        diagnostic.message
                    )?;
                }
                Ok(())
            }
            Self::UnsupportedAttachedInline { range } => write!(
                formatter,
                "legacy attached content at bytes {}..{} is not an inline element",
                range.start, range.end
            ),
            Self::ConflictingLinkTarget { range } => write!(
                formatter,
                "legacy link at bytes {}..{} has conflicting positional and 'to' targets",
                range.start, range.end
            ),
            Self::InvalidGenerated => {
                formatter.write_str("migration generated invalid current syntax")
            }
        }
    }
}

impl std::error::Error for MigrationError {}

pub fn migrate_member_envelope_v1(source: &str) -> Result<String, MigrationError> {
    if plumb_syntax::parse(source).is_valid() && !has_member_envelope_signal(source) {
        return Ok(source.to_string());
    }
    let parsed = member_legacy::parse(source);
    if !parsed.is_valid() {
        return Err(MigrationError::InvalidMemberEnvelope(
            parsed
                .diagnostics
                .iter()
                .filter(|diagnostic| {
                    diagnostic.severity == member_legacy::DiagnosticSeverity::Error
                })
                .map(|diagnostic| MigrationDiagnostic {
                    code: diagnostic.code,
                    range: diagnostic.range.clone(),
                    message: diagnostic.message.clone(),
                })
                .collect(),
        ));
    }
    let owned = convert_member_document(&parsed.syntax);
    let migrated = owned
        .format()
        .map_err(|_| MigrationError::InvalidGenerated)?;
    if !plumb_syntax::parse(&migrated).is_valid() {
        return Err(MigrationError::InvalidGenerated);
    }
    Ok(migrated)
}

fn has_member_envelope_signal(source: &str) -> bool {
    if source.contains('|') {
        return true;
    }
    let bytes = source.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] != b'`' {
            cursor += 1;
            continue;
        }
        let mut run_end = cursor;
        while run_end < bytes.len() && bytes[run_end] == b'`' {
            run_end += 1;
        }
        if (run_end - cursor) % 2 == 0 {
            cursor = run_end;
            continue;
        }
        let mut marker_end = run_end;
        while marker_end < bytes.len()
            && !matches!(
                bytes[marker_end],
                b' ' | b'\t' | b'\r' | b'\n' | b'`' | b'"' | b'[' | b']' | b'|'
            )
        {
            marker_end += 1;
        }
        if marker_end > run_end && bytes.get(marker_end) == Some(&b'[') {
            return true;
        }
        cursor = marker_end.max(run_end + 1);
    }
    false
}

fn convert_member_document(document: &member_legacy::Document) -> OwnedDocument {
    OwnedDocument {
        blocks: document
            .blocks
            .iter()
            .map(|block| convert_member_block(block, None))
            .collect(),
    }
}

fn convert_member_block(block: &member_legacy::Block, parent_marker: Option<&str>) -> OwnedBlock {
    match block {
        member_legacy::Block::Verbatim(block) => OwnedBlock::Verbatim {
            text: block.text.clone(),
        },
        member_legacy::Block::Parsed(block) => {
            let marker = block.mark.as_ref().map(|mark| mark.marker.clone());
            let policy = if matches!(marker.as_deref(), Some(":" | "="))
                || block
                    .mark
                    .as_ref()
                    .is_some_and(|mark| mark.attrs.has_class("event"))
            {
                BlockArgumentPolicy::FirstThenRest
            } else if parent_marker == Some("table") || block.head.arguments.len() > 1 {
                BlockArgumentPolicy::Positional
            } else {
                BlockArgumentPolicy::Whole
            };
            let head = convert_member_content(&block.head, policy);
            let mut children = block
                .children
                .iter()
                .map(|child| convert_member_block(child, marker.as_deref()))
                .collect::<Vec<_>>();
            let Some(raw) = &block.raw else {
                return OwnedBlock::Parsed {
                    marker,
                    head,
                    children,
                    raw: None,
                };
            };
            if head.is_empty() && children.is_empty() {
                return OwnedBlock::Parsed {
                    marker,
                    head,
                    children,
                    raw: Some(raw.text.clone()),
                };
            }
            children.push(OwnedBlock::Parsed {
                marker,
                head: Vec::new(),
                children: Vec::new(),
                raw: Some(raw.text.clone()),
            });
            OwnedBlock::Parsed {
                marker: Some("()".to_string()),
                head,
                children,
                raw: None,
            }
        }
    }
}

#[derive(Clone, Copy)]
enum BlockArgumentPolicy {
    Whole,
    Positional,
    FirstThenRest,
}

fn convert_member_content(
    content: &member_legacy::InlineContent,
    policy: BlockArgumentPolicy,
) -> Vec<OwnedInline> {
    let mut output = Vec::new();
    for (index, argument) in content.arguments.iter().enumerate() {
        if index > 0 {
            output.push(OwnedInline::Space(" ".to_string()));
        }
        let items = content.items[argument.item_range.clone()]
            .iter()
            .map(convert_member_inline)
            .collect::<Vec<_>>();
        let needs_group = match policy {
            BlockArgumentPolicy::Whole => false,
            BlockArgumentPolicy::Positional => owned_data_count(&items) != 1,
            BlockArgumentPolicy::FirstThenRest => index == 0 && owned_data_count(&items) != 1,
        };
        if needs_group {
            output.push(OwnedInline::Element {
                kind: String::new(),
                members: if items.is_empty() {
                    Vec::new()
                } else {
                    vec![OwnedInlineMember::ParsedArgument(items)]
                },
            });
        } else {
            output.extend(items);
        }
    }
    output
}

fn owned_data_count(items: &[OwnedInline]) -> usize {
    let mut count = 0;
    let mut in_datum = false;
    for item in items {
        if matches!(item, OwnedInline::Space(_) | OwnedInline::SoftBreak) {
            in_datum = false;
        } else if !in_datum {
            count += 1;
            in_datum = true;
        }
    }
    count
}

fn convert_member_inline(inline: &member_legacy::Inline) -> OwnedInline {
    match inline {
        member_legacy::Inline::Text { text, .. } => OwnedInline::Text(text.clone()),
        member_legacy::Inline::Space { text, .. } => OwnedInline::Space(text.clone()),
        member_legacy::Inline::SoftBreak { .. } => OwnedInline::Space(" ".to_string()),
        member_legacy::Inline::Verbatim { kind, text, .. } => OwnedInline::Verbatim {
            kind: kind.clone(),
            text: text.clone(),
        },
        member_legacy::Inline::Element { kind, members, .. } => OwnedInline::Element {
            kind: kind.clone(),
            members: members
                .iter()
                .map(|member| match member {
                    member_legacy::InlineMember::ParsedArgument(argument) => {
                        let items = argument
                            .content
                            .items
                            .iter()
                            .map(convert_member_inline)
                            .collect::<Vec<_>>();
                        let argument = if owned_data_count(&items) == 1 {
                            items
                        } else {
                            vec![OwnedInline::Element {
                                kind: String::new(),
                                members: if items.is_empty() {
                                    Vec::new()
                                } else {
                                    vec![OwnedInlineMember::ParsedArgument(items)]
                                },
                            }]
                        };
                        OwnedInlineMember::ParsedArgument(argument)
                    }
                    member_legacy::InlineMember::VerbatimArgument(argument) => {
                        OwnedInlineMember::VerbatimArgument(argument.text.clone())
                    }
                    member_legacy::InlineMember::Child { inline, .. } => {
                        OwnedInlineMember::Child(Box::new(convert_member_inline(inline)))
                    }
                })
                .collect(),
        },
    }
}

pub fn migrate_attached_v1(source: &str) -> Result<String, MigrationError> {
    let initial = legacy::parse(source);
    let current = member_legacy::parse(source);
    let hybrid = mask_current_inline_elements(source, &initial, &current);
    let (parsed, overrides) = if let Some((masked, ranges)) = hybrid {
        let reparsed = legacy::parse(masked);
        if reparsed.is_valid() {
            (
                reparsed,
                Some(HeadOverrides::new(&current.syntax.blocks, ranges)),
            )
        } else {
            (initial, None)
        }
    } else {
        (initial, None)
    };
    if !parsed.is_valid() {
        return Err(MigrationError::InvalidLegacy(
            parsed
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.severity == legacy::DiagnosticSeverity::Error)
                .map(|diagnostic| MigrationDiagnostic {
                    code: diagnostic.code,
                    range: diagnostic.range.clone(),
                    message: diagnostic.message.clone(),
                })
                .collect(),
        ));
    }

    let owned = convert_attached_v1_with_overrides(&parsed.syntax, overrides.as_ref())?;
    let migrated = owned
        .format()
        .map_err(|_| MigrationError::InvalidGenerated)?;
    if !plumb_syntax::parse(&migrated).is_valid() {
        return Err(MigrationError::InvalidGenerated);
    }
    Ok(migrated)
}

pub fn migrate_document_group_v1(source: &str) -> Result<String, MigrationError> {
    if !source.starts_with('{') {
        let parsed = member_legacy::parse(source);
        if parsed.is_valid() {
            return Ok(source.to_string());
        }
        return Err(invalid_document_group(&parsed, 0));
    }

    let legacy = legacy::parse(source);
    if !legacy.is_valid() {
        return Err(MigrationError::InvalidDocumentGroup(
            legacy
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.severity == legacy::DiagnosticSeverity::Error)
                .map(|diagnostic| MigrationDiagnostic {
                    code: diagnostic.code,
                    range: diagnostic.range.clone(),
                    message: diagnostic.message.clone(),
                })
                .collect(),
        ));
    }
    let Some(group) = legacy.syntax.attrs.attached.as_deref() else {
        return Err(MigrationError::InvalidDocumentGroup(Vec::new()));
    };
    let legacy::AttachedContent::Blocks(declarations) = &group.content else {
        return Err(MigrationError::InvalidDocumentGroup(Vec::new()));
    };

    let mut candidate = declarations
        .iter()
        .map(|block| dedent_root_block(source, block.range().clone()))
        .collect::<Vec<_>>()
        .join("\n\n");
    let body = source[group.range.end..].trim_start_matches(['\r', '\n']);
    if !body.is_empty() {
        if !candidate.is_empty() {
            candidate.push_str("\n\n");
        }
        candidate.push_str(body);
    }
    let parsed = member_legacy::parse(&candidate);
    if !parsed.is_valid() {
        return Err(invalid_document_group(&parsed, 0));
    }
    let mut owned = OwnedDocument {
        blocks: parsed
            .syntax
            .blocks
            .iter()
            .map(|block| convert_member_block(block, None))
            .collect(),
    };
    for block in &mut owned.blocks {
        upgrade_legacy_block_arguments(block);
    }
    let migrated = owned
        .format()
        .map_err(|_| MigrationError::InvalidGenerated)?;
    if !plumb_syntax::parse(&migrated).is_valid() {
        return Err(MigrationError::InvalidGenerated);
    }
    Ok(migrated)
}

pub fn migrate_head_space_v1(source: &str) -> Result<String, MigrationError> {
    let parsed = member_legacy::parse(source);
    if !parsed.is_valid() {
        return Err(invalid_head_space(&parsed));
    }

    let mut ranges = Vec::new();
    collect_head_space_ranges(&parsed.syntax.blocks, &mut ranges);
    let edits = ranges
        .into_iter()
        .map(|range| {
            Ok(TextEdit {
                range,
                new_text: "|".to_string(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let migrated = apply_text_edits(source.to_string(), edits)
        .map_err(|_| MigrationError::InvalidGenerated)?;
    migrate_member_envelope_v1(&migrated)
}

pub fn migrate_task_event_markers_v1(source: &str) -> Result<String, MigrationError> {
    let migrated_members = migrate_member_envelope_v1(source)?;
    let parsed = plumb_syntax::parse(&migrated_members);
    if !parsed.is_valid() {
        return Err(MigrationError::InvalidTaskEventMarkers(
            parsed
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.severity == plumb_syntax::DiagnosticSeverity::Error)
                .map(|diagnostic| MigrationDiagnostic {
                    code: diagnostic.code,
                    range: diagnostic.range.clone(),
                    message: diagnostic.message.clone(),
                })
                .collect(),
        ));
    }

    let mut owners = Vec::new();
    collect_task_event_marker_owners(&parsed.syntax.blocks, &mut owners);
    let rewrites = owners
        .into_iter()
        .map(|block| {
            let mark = block.mark.as_ref().expect("legacy owner has a mark");
            let facet = mark.marker.clone();
            MarkedOwnerRewrite {
                owner_range: block.range.clone(),
                marker: "-".to_string(),
                first_attribute: (!mark.attrs.has_class(&facet))
                    .then(|| OwnedAttribute::class(facet)),
            }
        })
        .collect::<Vec<_>>();
    if rewrites.is_empty() {
        return Ok(migrated_members);
    }
    let edits = plumb_edit::rewrite_marked_owners(&parsed, &rewrites)
        .map_err(|_| MigrationError::InvalidGenerated)?;
    let migrated =
        apply_text_edits(migrated_members, edits).map_err(|_| MigrationError::InvalidGenerated)?;
    if !plumb_syntax::parse(&migrated).is_valid() {
        return Err(MigrationError::InvalidGenerated);
    }
    Ok(migrated)
}

fn collect_task_event_marker_owners<'a>(
    blocks: &'a [plumb_syntax::Block],
    owners: &mut Vec<&'a plumb_syntax::ParsedBlock>,
) {
    for block in blocks {
        let plumb_syntax::Block::Parsed(block) = block else {
            continue;
        };
        let legacy = block
            .mark
            .as_ref()
            .is_some_and(|mark| matches!(mark.marker.as_str(), "task" | "event"));
        if legacy {
            owners.push(block);
        }
        collect_task_event_marker_owners(&block.children, owners);
    }
}

fn invalid_head_space(parsed: &member_legacy::ParsedDocument) -> MigrationError {
    MigrationError::InvalidHeadSpace(
        parsed
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == member_legacy::DiagnosticSeverity::Error)
            .map(|diagnostic| MigrationDiagnostic {
                code: diagnostic.code,
                range: diagnostic.range.clone(),
                message: diagnostic.message.clone(),
            })
            .collect(),
    )
}

fn collect_head_space_ranges(blocks: &[Block], output: &mut Vec<Range<usize>>) {
    let mut stack = blocks.iter().rev().collect::<Vec<_>>();
    while let Some(block) = stack.pop() {
        let Block::Parsed(block) = block else {
            continue;
        };
        if let Some(range) = legacy_head_space_range(block) {
            output.push(range);
        }
        stack.extend(block.children.iter().rev());
    }
}

fn legacy_head_space_range(block: &ParsedBlock) -> Option<Range<usize>> {
    if block.head.arguments.len() != 1 {
        return None;
    }
    let marker = block.mark.as_ref()?.marker.as_str();
    let [Inline::Space { .. }, first, Inline::Space { range, .. }, value @ ..] =
        block.head.items.as_slice()
    else {
        return None;
    };
    if value.is_empty() {
        return None;
    }
    match marker {
        "event" if matches!(first, Inline::Text { text, .. } if !text.is_empty()) => {
            Some(range.clone())
        }
        ":" if block.children.is_empty() => Some(range.clone()),
        "=" if block.children.is_empty() && is_legacy_association_key(first) => Some(range.clone()),
        _ => None,
    }
}

fn is_legacy_association_key(inline: &Inline) -> bool {
    match inline {
        Inline::Text { text, .. } | Inline::Verbatim { text, .. } => !text.is_empty(),
        Inline::Element { kind, members, .. } if kind == "()" => {
            let mut arguments = members.iter().filter_map(InlineMember::argument);
            arguments
                .next()
                .is_some_and(|argument| !argument.plain_text().is_empty())
                && arguments.next().is_none()
        }
        _ => false,
    }
}

fn dedent_root_block(source: &str, range: legacy::SourceRange) -> String {
    let mut output = String::new();
    for (index, line) in source[range].split_inclusive('\n').enumerate() {
        let line = if index == 0 {
            line
        } else {
            line.strip_prefix(' ').unwrap_or(line)
        };
        output.push_str(line);
    }
    output.trim_end_matches(['\r', '\n']).to_string()
}

fn invalid_document_group(
    parsed: &member_legacy::ParsedDocument,
    synthetic_prefix: usize,
) -> MigrationError {
    MigrationError::InvalidDocumentGroup(
        parsed
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == member_legacy::DiagnosticSeverity::Error)
            .map(|diagnostic| MigrationDiagnostic {
                code: diagnostic.code,
                range: diagnostic.range.start.saturating_sub(synthetic_prefix)
                    ..diagnostic.range.end.saturating_sub(synthetic_prefix),
                message: diagnostic.message.clone(),
            })
            .collect(),
    )
}

fn mask_current_inline_elements(
    source: &str,
    legacy: &legacy::ParsedDocument,
    current: &member_legacy::ParsedDocument,
) -> Option<(String, Vec<Range<usize>>)> {
    let errors = legacy
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == legacy::DiagnosticSeverity::Error)
        .map(|diagnostic| diagnostic.range.clone())
        .collect::<Vec<_>>();
    let mut attached = Vec::new();
    collect_legacy_attribute_ranges(&legacy.syntax.attrs, &mut attached);
    collect_legacy_block_attribute_ranges(&legacy.syntax.blocks, &mut attached);
    let mut legacy_verbatim = Vec::new();
    collect_legacy_verbatim_ranges(&legacy.syntax.blocks, &mut legacy_verbatim);

    let mut current_inlines = Vec::new();
    collect_current_inline_ranges(&current.syntax.blocks, &mut current_inlines);
    current_inlines.retain(|range| {
        (errors.iter().any(|error| ranges_intersect(range, error))
            || has_current_member_separator(&source[range.clone()]))
            && !attached
                .iter()
                .any(|group| group.start <= range.start && range.end <= group.end)
            && !legacy_verbatim
                .iter()
                .any(|block| block.start <= range.start && range.end <= block.end)
    });
    if current_inlines.is_empty() {
        return None;
    }

    let mut bytes = source.as_bytes().to_vec();
    for range in &current_inlines {
        for byte in &mut bytes[range.clone()] {
            if !matches!(*byte, b'\r' | b'\n') {
                *byte = b'x';
            }
        }
    }
    Some((
        String::from_utf8(bytes).expect("ASCII masking preserves UTF-8"),
        current_inlines,
    ))
}

fn collect_legacy_verbatim_ranges(blocks: &[legacy::Block], output: &mut Vec<Range<usize>>) {
    for block in blocks {
        match block {
            legacy::Block::Parsed(block) => collect_legacy_verbatim_ranges(&block.children, output),
            legacy::Block::Verbatim(block) => output.push(block.range.clone()),
        }
    }
}

fn has_current_member_separator(source: &str) -> bool {
    source.char_indices().any(|(offset, character)| {
        if character != '|' {
            return false;
        }
        source[..offset]
            .bytes()
            .rev()
            .take_while(|byte| *byte == b'`')
            .count()
            % 2
            == 0
    })
}

fn ranges_intersect(left: &Range<usize>, right: &Range<usize>) -> bool {
    left.start < right.end && right.start < left.end
}

fn collect_legacy_attribute_ranges(
    attributes: &legacy::Attributes,
    output: &mut Vec<Range<usize>>,
) {
    let Some(group) = attributes.attached.as_deref() else {
        return;
    };
    output.push(group.range.clone());
    match &group.content {
        legacy::AttachedContent::Blocks(blocks) => {
            collect_legacy_block_attribute_ranges(blocks, output)
        }
        legacy::AttachedContent::Inlines(content) => {
            collect_legacy_inline_attribute_ranges(&content.items, output)
        }
    }
}

fn collect_legacy_block_attribute_ranges(blocks: &[legacy::Block], output: &mut Vec<Range<usize>>) {
    for block in blocks {
        match block {
            legacy::Block::Parsed(block) => {
                if let Some(mark) = &block.mark {
                    collect_legacy_attribute_ranges(&mark.attrs, output);
                }
                collect_legacy_inline_attribute_ranges(&block.head.items, output);
                collect_legacy_block_attribute_ranges(&block.children, output);
            }
            legacy::Block::Verbatim(block) => collect_legacy_attribute_ranges(&block.attrs, output),
        }
    }
}

fn collect_legacy_inline_attribute_ranges(
    inlines: &[legacy::Inline],
    output: &mut Vec<Range<usize>>,
) {
    for inline in inlines {
        match inline {
            legacy::Inline::Element { slots, attrs, .. } => {
                collect_legacy_attribute_ranges(attrs, output);
                for slot in slots {
                    collect_legacy_inline_attribute_ranges(&slot.content.items, output);
                }
            }
            legacy::Inline::Verbatim { attrs, .. } => {
                collect_legacy_attribute_ranges(attrs, output)
            }
            legacy::Inline::Text { .. }
            | legacy::Inline::Space { .. }
            | legacy::Inline::SoftBreak { .. } => {}
        }
    }
}

fn collect_current_inline_ranges(blocks: &[member_legacy::Block], output: &mut Vec<Range<usize>>) {
    for block in blocks {
        let member_legacy::Block::Parsed(block) = block else {
            continue;
        };
        collect_current_ranges_in_content(&block.head, output);
        collect_current_inline_ranges(&block.children, output);
    }
}

fn collect_current_ranges_in_content(
    content: &member_legacy::InlineContent,
    output: &mut Vec<Range<usize>>,
) {
    for inline in &content.items {
        match inline {
            member_legacy::Inline::Element { range, members, .. } => {
                output.push(range.clone());
                for member in members {
                    match member {
                        member_legacy::InlineMember::ParsedArgument(argument) => {
                            collect_current_ranges_in_content(&argument.content, output)
                        }
                        member_legacy::InlineMember::Child { inline, .. } => {
                            collect_current_range_in_inline(inline, output)
                        }
                        member_legacy::InlineMember::VerbatimArgument(_) => {}
                    }
                }
            }
            member_legacy::Inline::Verbatim { range, .. } => output.push(range.clone()),
            member_legacy::Inline::Text { .. }
            | member_legacy::Inline::Space { .. }
            | member_legacy::Inline::SoftBreak { .. } => {}
        }
    }
}

fn collect_current_range_in_inline(inline: &member_legacy::Inline, output: &mut Vec<Range<usize>>) {
    let content = member_legacy::InlineContent::from_items(
        current_inline_range(inline).clone(),
        vec![inline.clone()],
    );
    collect_current_ranges_in_content(&content, output);
}

fn current_inline_range(inline: &member_legacy::Inline) -> &Range<usize> {
    match inline {
        member_legacy::Inline::Text { range, .. }
        | member_legacy::Inline::Space { range, .. }
        | member_legacy::Inline::SoftBreak { range }
        | member_legacy::Inline::Element { range, .. }
        | member_legacy::Inline::Verbatim { range, .. } => range,
    }
}

struct HeadOverrides<'a> {
    blocks: HashMap<usize, &'a member_legacy::ParsedBlock>,
    masked: Vec<Range<usize>>,
}

impl<'a> HeadOverrides<'a> {
    fn new(blocks: &'a [member_legacy::Block], masked: Vec<Range<usize>>) -> Self {
        let mut current = Self {
            blocks: HashMap::new(),
            masked,
        };
        current.collect_blocks(blocks);
        current
    }

    fn collect_blocks(&mut self, blocks: &'a [member_legacy::Block]) {
        for block in blocks {
            let member_legacy::Block::Parsed(block) = block else {
                continue;
            };
            self.blocks.insert(block.range.start, block);
            self.collect_blocks(&block.children);
        }
    }

    fn head(&self, block_start: usize, range: &Range<usize>) -> Option<Vec<OwnedInline>> {
        if !self
            .masked
            .iter()
            .any(|masked| ranges_intersect(masked, range))
        {
            return None;
        }
        let block = self.blocks.get(&block_start)?;
        let items = block
            .head
            .items
            .iter()
            .filter(|inline| {
                let inline = current_inline_range(inline);
                range.start <= inline.start && inline.end <= range.end
            })
            .map(convert_member_inline)
            .collect::<Vec<_>>();
        (!items.is_empty() || range.is_empty()).then_some(items)
    }
}

pub fn convert_attached_v1(document: &legacy::Document) -> Result<OwnedDocument, MigrationError> {
    convert_attached_v1_with_overrides(document, None)
}

fn convert_attached_v1_with_overrides(
    document: &legacy::Document,
    overrides: Option<&HeadOverrides<'_>>,
) -> Result<OwnedDocument, MigrationError> {
    let mut blocks = Vec::new();
    if let Some(attached) = document.attrs.attached.as_deref() {
        let legacy::AttachedContent::Blocks(document_declarations) = &attached.content else {
            return Err(MigrationError::UnsupportedAttachedInline {
                range: attached.range.clone(),
            });
        };
        blocks.extend(
            document_declarations
                .iter()
                .map(|block| convert_attached_block(block, overrides))
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    blocks.extend(
        document
            .blocks
            .iter()
            .map(|block| convert_block(block, overrides))
            .collect::<Result<Vec<_>, _>>()?,
    );
    Ok(OwnedDocument { blocks })
}

fn convert_block(
    block: &legacy::Block,
    overrides: Option<&HeadOverrides<'_>>,
) -> Result<OwnedBlock, MigrationError> {
    match block {
        legacy::Block::Parsed(block) => {
            let attributes = block.mark.as_ref().map_or_else(
                || Ok(Vec::new()),
                |mark| convert_block_attributes(&mark.attrs),
            )?;
            let mut children = attributes
                .into_iter()
                .map(owned_attribute_block)
                .collect::<Vec<_>>();
            children.extend(
                block
                    .mark
                    .as_ref()
                    .map(|mark| convert_attached_block_children(&mark.attrs, overrides))
                    .transpose()?
                    .unwrap_or_default(),
            );
            children.extend(
                block
                    .children
                    .iter()
                    .map(|block| convert_block(block, overrides))
                    .collect::<Result<Vec<_>, _>>()?,
            );
            let marker = block.mark.as_ref().map(|mark| mark.marker.clone());
            let mut head = overrides
                .and_then(|overrides| overrides.head(block.range.start, &block.head.range))
                .unwrap_or(convert_inlines(&block.head.items)?);
            if marker.as_deref() == Some("event")
                || (marker.as_deref() == Some(":") && children.is_empty())
            {
                split_legacy_compact_head(&mut head);
            }
            Ok(OwnedBlock::Parsed {
                marker,
                head,
                children,
                raw: None,
            })
        }
        legacy::Block::Verbatim(block) => {
            let attributes = convert_block_attributes(&block.attrs)?;
            let mut children = attributes
                .into_iter()
                .map(owned_attribute_block)
                .collect::<Vec<_>>();
            children.extend(convert_attached_block_children(&block.attrs, overrides)?);
            if block.kind.is_empty() && children.is_empty() {
                Ok(OwnedBlock::Verbatim {
                    text: block.text.clone(),
                })
            } else {
                Ok(OwnedBlock::Parsed {
                    marker: Some(if block.kind.is_empty() {
                        "()".into()
                    } else {
                        block.kind.clone()
                    }),
                    head: Vec::new(),
                    children,
                    raw: Some(block.text.clone()),
                })
            }
        }
    }
}

fn convert_attached_block(
    block: &legacy::Block,
    overrides: Option<&HeadOverrides<'_>>,
) -> Result<OwnedBlock, MigrationError> {
    let mut converted = convert_block(block, overrides)?;
    let association = matches!(block, legacy::Block::Parsed(block) if block.mark.as_ref().is_some_and(|mark| mark.marker == ":"));
    if let OwnedBlock::Parsed {
        marker: Some(marker),
        head,
        children,
        ..
    } = &mut converted
    {
        *marker = declaration_kind(marker).to_string();
        if association {
            if children.is_empty() {
                split_legacy_compact_head(head);
            }
            map_legacy_value_associations(children);
        }
    }
    Ok(converted)
}

fn map_legacy_value_associations(blocks: &mut [OwnedBlock]) {
    for block in blocks {
        if let OwnedBlock::Parsed {
            marker,
            head,
            children,
            ..
        } = block
        {
            if marker.as_deref() == Some(":") {
                *marker = Some("=".into());
                if children.is_empty() {
                    split_legacy_compact_head(head);
                }
            }
            map_legacy_value_associations(children);
        }
    }
}

fn split_legacy_compact_head(head: &mut [OwnedInline]) {
    if head
        .iter()
        .any(|inline| matches!(inline, OwnedInline::ArgumentSeparator))
    {
        return;
    }
    let Some(separator) = head
        .iter()
        .position(|inline| matches!(inline, OwnedInline::Space(_)))
    else {
        return;
    };
    head[separator] = OwnedInline::ArgumentSeparator;
}

fn upgrade_legacy_block_arguments(block: &mut OwnedBlock) {
    let OwnedBlock::Parsed {
        marker,
        head,
        children,
        ..
    } = block
    else {
        return;
    };
    if matches!(marker.as_deref(), Some("event"))
        || (matches!(marker.as_deref(), Some(":" | "=")) && children.is_empty())
    {
        split_legacy_compact_head(head);
    }
    for child in children {
        upgrade_legacy_block_arguments(child);
    }
}

fn convert_block_attributes(
    attributes: &legacy::Attributes,
) -> Result<Vec<OwnedAttribute>, MigrationError> {
    let mut items = attributes
        .items
        .iter()
        .map(convert_attribute)
        .collect::<Vec<_>>();
    if let Some(attached) = attributes.attached.as_deref() {
        if let legacy::AttachedContent::Inlines(content) = &attached.content {
            items.extend(convert_inline_declaration_attributes(
                &content.items,
                &attached.range,
            )?);
        }
    }
    Ok(items)
}

fn owned_attribute_block(attribute: OwnedAttribute) -> OwnedBlock {
    match attribute {
        OwnedAttribute::Id(value) => OwnedBlock::marked("@", value),
        OwnedAttribute::Class(value) => OwnedBlock::marked("+", value),
        OwnedAttribute::Pair { key, value } => {
            let value = match value {
                OwnedValue::Bare(value) | OwnedValue::Quoted(value) => value,
            };
            OwnedBlock::association(key, value)
        }
    }
}

fn convert_attribute(item: &legacy::AttrItem) -> OwnedAttribute {
    match item {
        legacy::AttrItem::Id { value, .. } => OwnedAttribute::Id(value.clone()),
        legacy::AttrItem::Class { value, .. } => OwnedAttribute::Class(value.clone()),
        legacy::AttrItem::Pair { key, value, .. } => OwnedAttribute::Pair {
            key: key.clone(),
            value: if value.quoted {
                OwnedValue::Quoted(value.decoded.clone())
            } else {
                OwnedValue::Bare(value.decoded.clone())
            },
        },
    }
}

fn convert_attached_block_children(
    attributes: &legacy::Attributes,
    overrides: Option<&HeadOverrides<'_>>,
) -> Result<Vec<OwnedBlock>, MigrationError> {
    let Some(attached) = attributes.attached.as_deref() else {
        return Ok(Vec::new());
    };
    match &attached.content {
        legacy::AttachedContent::Blocks(blocks) => blocks
            .iter()
            .map(|block| convert_attached_block(block, overrides))
            .collect(),
        legacy::AttachedContent::Inlines(content) => {
            convert_inline_declaration_attributes(&content.items, &attached.range)?;
            Ok(Vec::new())
        }
    }
}

fn convert_inline_declaration_attributes(
    inlines: &[legacy::Inline],
    group_range: &legacy::SourceRange,
) -> Result<Vec<OwnedAttribute>, MigrationError> {
    let mut attributes = Vec::new();
    for inline in inlines {
        match inline {
            legacy::Inline::Space { .. } | legacy::Inline::SoftBreak { .. } => {}
            legacy::Inline::Element { kind, slots, .. }
                if matches!(kind.as_str(), "@" | "-" | "+") =>
            {
                let [slot] = slots.as_slice() else {
                    return Err(MigrationError::UnsupportedAttachedInline {
                        range: group_range.clone(),
                    });
                };
                let value = slot.content.plain_text();
                if value.is_empty() {
                    return Err(MigrationError::UnsupportedAttachedInline {
                        range: group_range.clone(),
                    });
                }
                attributes.push(if kind == "@" {
                    OwnedAttribute::Id(value)
                } else {
                    OwnedAttribute::Class(value)
                });
            }
            legacy::Inline::Element { kind, slots, .. } if kind == ":" || kind == "=" => {
                let (key, value) = match slots.as_slice() {
                    [key, value] => (key.content.plain_text(), value.content.plain_text()),
                    [combined] => combined
                        .content
                        .plain_text()
                        .split_once(' ')
                        .map(|(key, value)| (key.to_string(), value.to_string()))
                        .ok_or_else(|| MigrationError::UnsupportedAttachedInline {
                            range: group_range.clone(),
                        })?,
                    _ => {
                        return Err(MigrationError::UnsupportedAttachedInline {
                            range: group_range.clone(),
                        })
                    }
                };
                if key.is_empty() || value.is_empty() {
                    return Err(MigrationError::UnsupportedAttachedInline {
                        range: group_range.clone(),
                    });
                }
                attributes.push(OwnedAttribute::Pair {
                    key,
                    value: OwnedValue::Bare(value),
                });
            }
            _ => {
                return Err(MigrationError::UnsupportedAttachedInline {
                    range: group_range.clone(),
                })
            }
        }
    }
    Ok(attributes)
}

fn convert_inlines(inlines: &[legacy::Inline]) -> Result<Vec<OwnedInline>, MigrationError> {
    inlines.iter().map(convert_inline).collect()
}

fn convert_inline(inline: &legacy::Inline) -> Result<OwnedInline, MigrationError> {
    match inline {
        legacy::Inline::Text { text, .. } => Ok(OwnedInline::Text(text.clone())),
        legacy::Inline::Space { text, .. } => Ok(OwnedInline::Space(text.clone())),
        legacy::Inline::SoftBreak { .. } => Ok(OwnedInline::SoftBreak),
        legacy::Inline::Element {
            range,
            kind,
            slots,
            attrs,
            ..
        } => convert_element(range, current_inline_kind(kind), slots, attrs),
        legacy::Inline::Verbatim {
            range,
            kind,
            text,
            attrs,
            ..
        } => convert_verbatim(range, kind, text, attrs),
    }
}

fn convert_element(
    range: &legacy::SourceRange,
    kind: &str,
    slots: &[legacy::InlineSlot],
    attributes: &legacy::Attributes,
) -> Result<OwnedInline, MigrationError> {
    let property_link_target = kind == "->" && has_attached_link_target(attributes);
    let mut members = if kind == "->" && slots.len() == 1 && !property_link_target {
        convert_compact_link_slot(&slots[0].content.items)?
    } else {
        slots
            .iter()
            .map(|slot| {
                Ok(OwnedInlineMember::ParsedArgument(convert_inlines(
                    &slot.content.items,
                )?))
            })
            .collect::<Result<Vec<_>, MigrationError>>()?
    };

    if let Some(attached) = attributes.attached.as_deref() {
        let legacy::AttachedContent::Inlines(content) = &attached.content else {
            return Err(MigrationError::UnsupportedAttachedInline {
                range: attached.range.clone(),
            });
        };
        append_attached_members(kind, range, &mut members, &content.items)?;
    }

    Ok(OwnedInline::Element {
        kind: kind.to_string(),
        members,
    })
}

fn has_attached_link_target(attributes: &legacy::Attributes) -> bool {
    let Some(attached) = attributes.attached.as_deref() else {
        return false;
    };
    let legacy::AttachedContent::Inlines(content) = &attached.content else {
        return false;
    };
    content.items.iter().any(|inline| {
        matches!(
            inline,
            legacy::Inline::Element { kind, slots, .. }
                if kind == ":" && association_key(slots) == Some("to")
        )
    })
}

fn convert_verbatim(
    range: &legacy::SourceRange,
    kind: &str,
    text: &str,
    attributes: &legacy::Attributes,
) -> Result<OwnedInline, MigrationError> {
    let Some(attached) = attributes.attached.as_deref() else {
        return Ok(OwnedInline::Verbatim {
            kind: kind.to_string(),
            text: text.to_string(),
        });
    };
    let legacy::AttachedContent::Inlines(content) = &attached.content else {
        return Err(MigrationError::UnsupportedAttachedInline {
            range: attached.range.clone(),
        });
    };
    let owner_kind = if kind.is_empty() { "code" } else { kind };
    let mut members = vec![OwnedInlineMember::VerbatimArgument(text.to_string())];
    append_attached_members(owner_kind, range, &mut members, &content.items)?;
    Ok(OwnedInline::Element {
        kind: current_inline_kind(owner_kind).to_string(),
        members,
    })
}

fn append_attached_members(
    owner_kind: &str,
    owner_range: &legacy::SourceRange,
    members: &mut Vec<OwnedInlineMember>,
    attached: &[legacy::Inline],
) -> Result<(), MigrationError> {
    let mut link_target = None;
    let mut children = Vec::new();
    for inline in attached {
        match inline {
            legacy::Inline::Space { .. } | legacy::Inline::SoftBreak { .. } => {}
            legacy::Inline::Text { range, .. } => {
                return Err(MigrationError::UnsupportedAttachedInline {
                    range: range.clone(),
                });
            }
            legacy::Inline::Element { kind, slots, .. }
                if owner_kind == "->" && kind == ":" && association_key(slots) == Some("to") =>
            {
                let target = association_value(slots)?;
                if link_target.replace(target).is_some() {
                    return Err(MigrationError::ConflictingLinkTarget {
                        range: owner_range.clone(),
                    });
                }
            }
            _ => children.push(OwnedInlineMember::Child(Box::new(convert_attached_inline(
                inline,
            )?))),
        }
    }

    if let Some(target) = link_target {
        let argument_count = members
            .iter()
            .filter(|member| {
                matches!(
                    member,
                    OwnedInlineMember::ParsedArgument(_) | OwnedInlineMember::VerbatimArgument(_)
                )
            })
            .count();
        if argument_count != 1 {
            return Err(MigrationError::ConflictingLinkTarget {
                range: owner_range.clone(),
            });
        }
        members.push(OwnedInlineMember::ParsedArgument(target));
    }
    members.extend(children);
    Ok(())
}

fn convert_attached_inline(inline: &legacy::Inline) -> Result<OwnedInline, MigrationError> {
    match inline {
        legacy::Inline::Element {
            range,
            kind,
            slots,
            attrs,
            ..
        } => {
            let kind = declaration_kind(kind);
            let mut converted = convert_element(range, kind, slots, attrs)?;
            if kind == "=" {
                split_owned_association(&mut converted);
            }
            Ok(converted)
        }
        legacy::Inline::Verbatim {
            range,
            kind,
            text,
            attrs,
            ..
        } => {
            if kind.is_empty() && attrs.attached.is_none() {
                Ok(OwnedInline::Verbatim {
                    kind: "code".into(),
                    text: text.clone(),
                })
            } else {
                convert_verbatim(range, kind, text, attrs)
            }
        }
        legacy::Inline::Text { range, .. }
        | legacy::Inline::Space { range, .. }
        | legacy::Inline::SoftBreak { range } => Err(MigrationError::UnsupportedAttachedInline {
            range: range.clone(),
        }),
    }
}

fn convert_compact_link_slot(
    inlines: &[legacy::Inline],
) -> Result<Vec<OwnedInlineMember>, MigrationError> {
    if let [label, legacy::Inline::Space { .. }, target @ ..] = inlines {
        return Ok(vec![
            OwnedInlineMember::ParsedArgument(vec![convert_inline(label)?]),
            OwnedInlineMember::ParsedArgument(convert_inlines(target)?),
        ]);
    }
    Ok(vec![OwnedInlineMember::ParsedArgument(convert_inlines(
        inlines,
    )?)])
}

fn association_key(slots: &[legacy::InlineSlot]) -> Option<&str> {
    match slots {
        [key, _] => single_text(&key.content.items),
        [slot] => match slot.content.items.as_slice() {
            [legacy::Inline::Text { text, .. }, legacy::Inline::Space { .. }, ..] => Some(text),
            _ => None,
        },
        _ => None,
    }
}

fn association_value(slots: &[legacy::InlineSlot]) -> Result<Vec<OwnedInline>, MigrationError> {
    match slots {
        [_, value] => convert_inlines(&value.content.items),
        [slot] => match slot.content.items.as_slice() {
            [_, legacy::Inline::Space { .. }, value @ ..] => convert_inlines(value),
            _ => Ok(Vec::new()),
        },
        _ => Ok(Vec::new()),
    }
}

fn single_text(inlines: &[legacy::Inline]) -> Option<&str> {
    match inlines {
        [legacy::Inline::Text { text, .. }] => Some(text),
        _ => None,
    }
}

fn split_owned_association(inline: &mut OwnedInline) {
    let OwnedInline::Element { members, .. } = inline else {
        return;
    };
    let [OwnedInlineMember::ParsedArgument(argument)] = members.as_mut_slice() else {
        return;
    };
    let Some(index) = argument
        .iter()
        .position(|inline| matches!(inline, OwnedInline::Space(_)))
    else {
        return;
    };
    let value = argument.split_off(index + 1);
    argument.pop();
    members.push(OwnedInlineMember::ParsedArgument(value));
}

fn declaration_kind(kind: &str) -> &str {
    match kind {
        "-" => "+",
        ":" => "=",
        _ => current_inline_kind(kind),
    }
}

fn current_inline_kind(kind: &str) -> &str {
    if kind == "=" {
        "=="
    } else {
        kind
    }
}

#[cfg(any())]
mod tests {
    use super::*;

    #[test]
    fn migrates_slots_attached_children_and_declaration_spellings() {
        let source = "`pair[first][second]{`@[id] `-[facet] `:[key value] `custom[child]}\n";
        let migrated = migrate_attached_v1(source).unwrap();
        assert_eq!(
            migrated,
            "`pair[first|second|@[id]|+[facet]|=[key|value]|custom[child]]\n"
        );
    }

    #[test]
    fn migrates_block_attachments_around_current_inline_members() {
        let source = "`# Links {\n `@ links\n}\n\nSee `->[guide|guide.plumb].\n\n`img[icon|=[src|asset.png]]\n";
        let migrated = migrate_attached_v1(source).unwrap();
        assert_eq!(
            migrated,
            "`# Links\n\n `@ links\n\nSee `->[guide|guide.plumb].\n\n`img[icon|=[src|asset.png]]\n"
        );
        assert!(plumb_syntax::parse(&migrated).is_valid());
    }

    #[test]
    fn preserves_current_inline_spelling_inside_legacy_verbatim_payload() {
        let source = "`plumb\"\n `->[guide|guide.plumb]\n `img[icon|=[src|asset.png]]\n";
        let migrated = migrate_attached_v1(source).unwrap();
        assert_eq!(
            migrated,
            "`plumb\n|\"\n `->[guide|guide.plumb]\n `img[icon|=[src|asset.png]]\n"
        );
    }

    #[test]
    fn preserves_crlf_tabs_braces_and_trailing_empty_raw_bytes() {
        let source = "`plumb\"\r\n \t{ `->[guide|guide.plumb] }\r\n \r\n";
        let migrated = migrate_attached_v1(source).unwrap();
        assert_eq!(
            migrated,
            "`plumb\n|\"\n \t{ `->[guide|guide.plumb] }\r\n \r\n"
        );
        let parsed = plumb_syntax::parse(&migrated);
        let [plumb_syntax::Block::Parsed(owner)] = parsed.syntax.blocks.as_slice() else {
            panic!("expected one marked raw owner");
        };
        assert_eq!(
            owner.raw.as_ref().unwrap().text.as_bytes(),
            b"\t{ `->[guide|guide.plumb] }\r\n\r\n"
        );
    }

    #[test]
    fn migrates_compact_and_property_links_to_positional_arguments() {
        let source = "`->[guide target.plumb]\n`->[guide]{`:[to Project Guide.plumb]}\n`->[Get cookies.txt LOCALLY]{`:[to https://example.test]}\n";
        let migrated = migrate_attached_v1(source).unwrap();
        assert_eq!(
            migrated,
            "`->[guide|target.plumb]\n`->[guide|Project Guide.plumb]\n`->[Get cookies.txt LOCALLY|https://example.test]\n"
        );
    }

    #[test]
    fn preserves_opaque_block_attached_content_and_ordinary_children() {
        let source = "{\n `: title Example\n `custom root\n}\n\n`task Work {\n `@ work\n `: created now\n `opaque value\n}\n\n  `note ordinary child\n";
        let migrated = migrate_attached_v1(source).unwrap();
        assert!(migrated.contains("`= title|Example"), "{migrated}");
        assert!(migrated.contains("`custom root"), "{migrated}");
        assert!(migrated.contains("`= created|now"), "{migrated}");
        assert!(migrated.contains("`opaque value"), "{migrated}");
        assert!(migrated.contains("`note ordinary child"), "{migrated}");
        assert!(plumb_syntax::parse(&migrated).is_valid(), "{migrated}");
    }

    #[test]
    fn migrates_nested_map_entries_but_preserves_sequence_items() {
        let source =
            "{\n `: project\n   `: name plumb\n   `: tags\n     `- syntax\n     `- tools\n}\n";
        let migrated = migrate_attached_v1(source).unwrap();
        assert!(migrated.contains("`= project"), "{migrated}");
        assert!(migrated.contains("`= name|plumb"), "{migrated}");
        assert!(migrated.contains("`= tags"), "{migrated}");
        assert!(migrated.contains("`- syntax"), "{migrated}");
    }

    #[test]
    fn expands_anonymous_verbatim_with_children_to_an_explicit_owner() {
        let migrated = migrate_attached_v1("`\"raw\"{`@[id]}\n").unwrap();
        assert_eq!(migrated, "`code[|\"[raw]\"|@[id]]\n");
    }

    #[test]
    fn renames_the_legacy_inline_mark_kind() {
        let migrated = migrate_attached_v1("Before `=[marked] after.\n").unwrap();
        assert_eq!(migrated, "Before `==[marked] after.\n");
    }

    #[test]
    fn preserves_escaped_delimiters_in_legacy_text() {
        let source = "`event wheel: refactor qt`{5,6`}ct {\n `: uid example\n}\n";
        let migrated = migrate_attached_v1(source).unwrap();
        assert_eq!(
            migrated,
            "`event wheel:|refactor qt{5,6}ct\n\n `= uid|example\n"
        );
    }

    #[test]
    fn converts_named_verbatim_to_a_marked_owner_with_one_raw_tail() {
        let source = "`tex\" {`+[$] `@[equation]}\n E = mc^2\n";
        let migrated = migrate_attached_v1(source).unwrap();
        assert_eq!(
            migrated,
            "`tex\n\n `+ $\n\n `@ equation\n\n|\"\n E = mc^2\n"
        );
        let parsed = plumb_syntax::parse(&migrated);
        let [plumb_syntax::Block::Parsed(owner)] = parsed.syntax.blocks.as_slice() else {
            panic!("expected one marked raw owner");
        };
        assert_eq!(owner.children.len(), 2);
        assert_eq!(owner.raw.as_ref().unwrap().text, "E = mc^2\n");
    }

    #[test]
    fn explicitizes_an_attributed_anonymous_verbatim_block() {
        let source = "`\" {`@[example]}\n raw { bytes }\n";
        let migrated = migrate_attached_v1(source).unwrap();
        assert_eq!(migrated, "`()\n\n `@ example\n\n|\"\n raw { bytes }\n");
    }

    #[test]
    fn rejects_conflicting_legacy_link_targets() {
        let error = migrate_attached_v1("`->[guide][positional.plumb]{`:[to property.plumb]}\n")
            .unwrap_err();
        assert!(matches!(
            error,
            MigrationError::ConflictingLinkTarget { .. }
        ));
    }

    #[test]
    fn rejects_invalid_legacy_source_before_conversion() {
        let error = migrate_attached_v1("`broken[\n").unwrap_err();
        assert!(matches!(error, MigrationError::InvalidLegacy(_)));
    }

    #[test]
    fn migration_output_is_valid_current_syntax() {
        let source = "`pair[first][second]{`-[facet]}\n";
        let once = migrate_attached_v1(source).unwrap();
        assert!(plumb_syntax::parse(&once).is_valid());
        assert_ne!(once, source);
    }

    #[test]
    fn migrates_only_legacy_semantic_head_spaces_with_minimal_edits() {
        let source = "`= title Plumb notes\r\n`= child key with spaces\r\n\r\n child value\r\n\r\n`: Term Inline body\r\n\r\n`: Term with spaces\r\n\r\n child definition\r\n\r\n`event 14:00 Parser review\r\n `note details\r\n\r\n`note Generic head stays flat\r\n\r\nParagraph stays flat.\r\n\r\n`= current|value\r\n";
        let migrated = migrate_head_space_v1(source).unwrap();
        assert_eq!(
            migrated,
            "`= title|Plumb notes\r\n`= child key with spaces\r\n\r\n child value\r\n\r\n`: Term|Inline body\r\n\r\n`: Term with spaces\r\n\r\n child definition\r\n\r\n`event 14:00|Parser review\r\n `note details\r\n\r\n`note Generic head stays flat\r\n\r\nParagraph stays flat.\r\n\r\n`= current|value\r\n"
        );
        assert_eq!(migrate_head_space_v1(&migrated).unwrap(), migrated);
    }

    #[test]
    fn head_space_migration_preserves_unrecognized_keys_and_event_schedules() {
        let source = "`= `()[key with spaces] scalar value\n`= `*[not a key] scalar value\n`event `*[14:00] Styled schedule\n";
        assert_eq!(
            migrate_head_space_v1(source).unwrap(),
            "`= `()[key with spaces]|scalar value\n`= `*[not a key] scalar value\n`event `*[14:00] Styled schedule\n"
        );
    }

    #[test]
    fn head_space_migration_rejects_current_syntax_errors() {
        let error = migrate_head_space_v1("`broken[\n").unwrap_err();
        assert!(matches!(error, MigrationError::InvalidHeadSpace(_)));
    }

    #[test]
    fn lifts_a_document_group_around_current_inline_syntax() {
        let source = "{\n `= title Current\n}\n\n`->[guide|guide.plumb]\n";
        let migrated = migrate_document_group_v1(source).unwrap();
        assert_eq!(migrated, "`= title|Current\n\n`->[guide|guide.plumb]\n");
        assert!(plumb_syntax::parse(&migrated).is_valid());
    }

    #[test]
    fn document_group_migration_upgrades_semantic_block_arguments() {
        let source =
            "{\n `= title Project guide\n}\n\n`: Term Inline body\n\n`event 14:00 Review notes\n";
        let migrated = migrate_document_group_v1(source).unwrap();
        assert_eq!(
            migrated,
            "`= title|Project guide\n\n`: Term|Inline body\n\n`event 14:00|Review notes\n"
        );
        assert!(plumb_syntax::parse(&migrated).is_valid());
    }

    #[test]
    fn document_group_migration_keeps_current_documents_unchanged() {
        let source = "`= title Current\n\nBody.\n";
        assert_eq!(migrate_document_group_v1(source).unwrap(), source);
    }

    #[test]
    fn document_group_migration_rejects_other_current_errors() {
        let error = migrate_document_group_v1("{\n `= title Current\n}\n\n`broken[\n").unwrap_err();
        assert!(matches!(error, MigrationError::InvalidDocumentGroup(_)));
    }

    #[test]
    fn migrates_task_and_event_markers_to_first_facets() {
        let source = "`task Work\n `@ work\n\n `event 10:00|Review\n  `= date|2026-08-29\n\n`note Untouched\n";
        let migrated = migrate_task_event_markers_v1(source).unwrap();
        assert_eq!(
            migrated,
            "`- Work\n\n `+ task\n\n `@ work\n\n `- 10:00|Review\n\n  `+ event\n\n  `= date|2026-08-29\n\n`note Untouched\n"
        );
        assert_eq!(migrate_task_event_markers_v1(&migrated).unwrap(), migrated);
    }

    #[test]
    fn task_event_marker_migration_preserves_crlf_raw_tails_and_deduplicates_facets() {
        let source = "`task Code\r\n `+ task\r\n\r\n|\"\r\n raw\r\n";
        let migrated = migrate_task_event_markers_v1(source).unwrap();
        assert_eq!(migrated, "`- Code\r\n `+ task\r\n\r\n|\"\r\n raw\r\n");
        assert!(!migrated.contains("`+ task\r\n\r\n `+ task"));
    }

    #[test]
    fn task_event_marker_migration_rejects_invalid_current_syntax() {
        let error = migrate_task_event_markers_v1("`broken[\n").unwrap_err();
        assert!(matches!(error, MigrationError::InvalidTaskEventMarkers(_)));
    }

    #[test]
    fn migrates_many_task_event_owners_in_one_batch() {
        let mut source = String::new();
        for index in 0..5_000 {
            source.push_str(&format!("`event 09:00|Event {index}\n"));
        }
        let migrated = migrate_task_event_markers_v1(&source).unwrap();
        assert_eq!(migrated.matches("`+ event").count(), 5_000);
        assert!(!migrated.contains("`event "));
        assert!(plumb_syntax::parse(&migrated).is_valid());
    }
}
