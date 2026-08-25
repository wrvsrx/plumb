use std::fmt;

use plumb_edit::{
    OwnedAttachedContent, OwnedAttributes, OwnedBlock, OwnedDocument, OwnedInline,
    OwnedInlineMember,
};
use plumb_syntax_legacy_v1 as legacy;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationError {
    InvalidLegacy(Vec<MigrationDiagnostic>),
    UnsupportedAttachedInline { range: legacy::SourceRange },
    ConflictingLinkTarget { range: legacy::SourceRange },
    InvalidGenerated,
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

pub fn migrate_attached_v1(source: &str) -> Result<String, MigrationError> {
    let parsed = legacy::parse(source);
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

    let owned = convert_attached_v1(&parsed.syntax)?;
    let migrated = owned
        .format()
        .map_err(|_| MigrationError::InvalidGenerated)?;
    if !plumb_syntax::parse(&migrated).is_valid() {
        return Err(MigrationError::InvalidGenerated);
    }
    Ok(migrated)
}

pub fn convert_attached_v1(document: &legacy::Document) -> Result<OwnedDocument, MigrationError> {
    let mut blocks = Vec::new();
    if document.attrs.attached.is_some() || document.attrs.range.is_some() {
        blocks.push(OwnedBlock::Document {
            attributes: convert_block_attributes(&document.attrs)?,
        });
    }
    blocks.extend(
        document
            .blocks
            .iter()
            .map(convert_block)
            .collect::<Result<Vec<_>, _>>()?,
    );
    Ok(OwnedDocument { blocks })
}

fn convert_block(block: &legacy::Block) -> Result<OwnedBlock, MigrationError> {
    match block {
        legacy::Block::Parsed(block) => Ok(OwnedBlock::Parsed {
            marker: block.mark.as_ref().map(|mark| mark.marker.clone()),
            attributes: block.mark.as_ref().map_or_else(
                || Ok(OwnedAttributes::default()),
                |mark| convert_block_attributes(&mark.attrs),
            )?,
            head: convert_inlines(&block.head.items)?,
            children: block
                .children
                .iter()
                .map(convert_block)
                .collect::<Result<Vec<_>, _>>()?,
        }),
        legacy::Block::Verbatim(block) => Ok(OwnedBlock::Verbatim {
            kind: block.kind.clone(),
            attributes: convert_block_attributes(&block.attrs)?,
            text: block.text.clone(),
        }),
    }
}

fn convert_attached_block(block: &legacy::Block) -> Result<OwnedBlock, MigrationError> {
    let mut converted = convert_block(block)?;
    let association = matches!(block, legacy::Block::Parsed(block) if block.mark.as_ref().is_some_and(|mark| mark.marker == ":"));
    if let OwnedBlock::Parsed {
        marker: Some(marker),
        children,
        ..
    } = &mut converted
    {
        *marker = declaration_kind(marker).to_string();
        if association {
            map_legacy_value_associations(children);
        }
    }
    Ok(converted)
}

fn map_legacy_value_associations(blocks: &mut [OwnedBlock]) {
    for block in blocks {
        if let OwnedBlock::Parsed {
            marker, children, ..
        } = block
        {
            if marker.as_deref() == Some(":") {
                *marker = Some("=".into());
            }
            map_legacy_value_associations(children);
        }
    }
}

fn convert_block_attributes(
    attributes: &legacy::Attributes,
) -> Result<OwnedAttributes, MigrationError> {
    let content = attributes
        .attached
        .as_deref()
        .map(|attached| match &attached.content {
            legacy::AttachedContent::Blocks(blocks) => Ok(OwnedAttachedContent::Blocks(
                blocks
                    .iter()
                    .map(convert_attached_block)
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            legacy::AttachedContent::Inlines(content) => Ok(OwnedAttachedContent::Inlines(
                convert_attached_inlines(&content.items)?,
            )),
        })
        .transpose()?;

    Ok(OwnedAttributes {
        present: attributes.range.is_some() || attributes.attached.is_some(),
        items: Vec::new(),
        content,
    })
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

fn convert_attached_inlines(
    inlines: &[legacy::Inline],
) -> Result<Vec<OwnedInline>, MigrationError> {
    inlines
        .iter()
        .map(|inline| match inline {
            legacy::Inline::Element { .. } | legacy::Inline::Verbatim { .. } => {
                convert_attached_inline(inline)
            }
            legacy::Inline::Text { text, .. } | legacy::Inline::Space { text, .. } => {
                Ok(OwnedInline::Text(text.clone()))
            }
            legacy::Inline::SoftBreak { .. } => Ok(OwnedInline::SoftBreak),
        })
        .collect()
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

#[cfg(test)]
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
        assert!(migrated.contains("`= title Example"), "{migrated}");
        assert!(migrated.contains("`custom root"), "{migrated}");
        assert!(migrated.contains("`= created now"), "{migrated}");
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
        assert!(migrated.contains("`= name plumb"), "{migrated}");
        assert!(migrated.contains("`= tags"), "{migrated}");
        assert!(migrated.contains("`- syntax"), "{migrated}");
    }

    #[test]
    fn expands_anonymous_verbatim_with_children_to_an_explicit_owner() {
        let migrated = migrate_attached_v1("`\"raw\"{`@[id]}\n").unwrap();
        assert_eq!(migrated, "`code[\"raw\"|@[id]]\n");
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
            "`event wheel: refactor qt`{5,6`}ct {\n `= uid example\n}\n"
        );
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
}
