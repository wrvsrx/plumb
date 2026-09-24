//! Owned edits to direct document declarations, independent of domain semantics.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootDeclarationEdit {
    SetProperty(OwnedDeclaration),
    RemoveProperty(String),
    SetFacet { name: String, present: bool },
}

struct RootDeclaration {
    range: Option<Range<usize>>,
    original: Option<OwnedBlock>,
    current: Option<OwnedBlock>,
}

fn root_declarations(document: &GreenDocument) -> Result<Vec<RootDeclaration>, EditError> {
    if !document.is_valid() {
        return Err(EditError::GeneratedInvalid);
    }
    let mut result = Vec::new();
    for shard in document.shards() {
        let parsed = shard.shard().parsed();
        for block in &parsed.syntax.blocks {
            let Block::Parsed(owner) = block else {
                continue;
            };
            if !owner
                .mark
                .as_ref()
                .is_some_and(|mark| matches!(mark.marker.as_str(), "+" | "="))
            {
                continue;
            }
            let owned = OwnedBlock::from_parsed(&parsed.source, owner);
            result.push(RootDeclaration {
                range: Some(owner.range.start + shard.offset()..owner.range.end + shard.offset()),
                original: Some(owned.clone()),
                current: Some(owned),
            });
        }
    }
    Ok(result)
}

fn root_property_key(block: &OwnedBlock) -> Option<String> {
    if let OwnedBlock::Parsed {
        marker: Some(marker),
        head,
        children,
        raw: None,
    } = block
    {
        if marker == "=" && !children.is_empty() {
            return plain_owned_argument(head)
                .map(|key| key.trim().to_owned())
                .filter(|key| !key.is_empty());
        }
    }
    owned_declaration_key(block)
}

fn facet_name(block: &OwnedBlock) -> Option<String> {
    let OwnedBlock::Parsed {
        marker: Some(marker),
        head,
        children,
        raw: None,
    } = block
    else {
        return None;
    };
    if marker != "+" || !children.is_empty() {
        return None;
    }
    let positions = owned_positional_indices(head);
    if positions.len() != 1 {
        return None;
    }
    plain_owned_element(&head[positions[0]]).filter(|name| !name.is_empty())
}

/// Read a root scalar/list property without materializing a document-wide syntax tree.
/// Duplicate matching declarations are ambiguous and are rejected.
pub fn green_root_declaration(
    document: &GreenDocument,
    key: &str,
) -> Result<Option<OwnedDeclaration>, EditError> {
    let mut found = None;
    for declaration in root_declarations(document)? {
        let block = declaration.current.as_ref().unwrap();
        if root_property_key(block).as_deref() == Some(key) {
            if found.is_some() {
                return Err(EditError::GeneratedInvalid);
            }
            found = Some(owned_block_declaration(block).ok_or(EditError::GeneratedInvalid)?);
        }
    }
    Ok(found)
}

/// Apply a declaration transaction against one valid green revision.
/// Existing body blocks and unrelated declarations retain their exact source bytes.
/// Duplicate properties cannot be silently overwritten; removal explicitly removes all
/// declarations with the requested key/facet. New declarations are inserted at the root.
pub fn edit_green_root_declarations(
    document: &GreenDocument,
    intents: &[RootDeclarationEdit],
) -> Result<Vec<TextEdit>, EditError> {
    let mut declarations = root_declarations(document)?;
    for intent in intents {
        let matches = declarations
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let block = entry.current.as_ref()?;
                let matches = match intent {
                    RootDeclarationEdit::SetProperty(declaration) => {
                        root_property_key(block).as_deref() == Some(declaration.key.as_str())
                    }
                    RootDeclarationEdit::RemoveProperty(key) => {
                        root_property_key(block).as_deref() == Some(key.as_str())
                    }
                    RootDeclarationEdit::SetFacet { name, .. } => {
                        facet_name(block).as_deref() == Some(name.as_str())
                    }
                };
                matches.then_some(index)
            })
            .collect::<Vec<_>>();
        match intent {
            RootDeclarationEdit::SetProperty(declaration) => {
                let replacement = declaration.clone().into_block()?;
                match matches.as_slice() {
                    [] => declarations.push(RootDeclaration {
                        range: None,
                        original: None,
                        current: Some(replacement),
                    }),
                    [index] => {
                        let existing = declarations[*index].current.as_ref().unwrap();
                        if owned_block_declaration(existing).as_ref() != Some(declaration) {
                            declarations[*index].current = Some(replacement);
                        }
                    }
                    _ => return Err(EditError::GeneratedInvalid),
                }
            }
            RootDeclarationEdit::SetFacet {
                name,
                present: true,
            } => {
                if name.is_empty() {
                    return Err(EditError::GeneratedInvalid);
                }
                if matches.is_empty() {
                    let mut block = OwnedBlock::marked("+", "");
                    block.set_head_text_arguments([name]);
                    declarations.push(RootDeclaration {
                        range: None,
                        original: None,
                        current: Some(block),
                    });
                }
            }
            RootDeclarationEdit::RemoveProperty(_)
            | RootDeclarationEdit::SetFacet { present: false, .. } => {
                for index in matches {
                    declarations[index].current = None;
                }
            }
        }
    }
    let mut edits = Vec::new();
    let mut additions = Vec::new();
    for declaration in declarations {
        if declaration.current == declaration.original {
            continue;
        }
        match (declaration.range, declaration.current) {
            (Some(range), Some(block)) => edits.push(replace_green_block(document, range, &block)?),
            (Some(range), None) => {
                edits.push(TextEdit::replace_source(document.source(), range, "")?)
            }
            (None, Some(block)) => additions.push(block),
            (None, None) => {}
        }
    }
    if !additions.is_empty() {
        // Keep root authoring order stable: properties come before the task
        // facet, while later properties are appended after existing metadata.
        additions.sort_by_key(|block| match block {
            OwnedBlock::Parsed { marker: Some(marker), .. } if marker == "+" => 1,
            _ => 0,
        });
        let insertion = prepend_green_blocks(document, &additions)?;
        if let Some(edit) = edits.iter_mut().find(|edit| edit.range.end == document.source().len()) {
            edit.new_text.push_str(&insertion.new_text);
        } else {
            edits.push(insertion);
        }
    }
    edits.sort_by_key(|edit| edit.range.start);
    if let (Some(first), Some(last)) = (edits.first(), edits.last()) {
        let changed = apply_text_edits(document.source().to_owned(), edits.clone())?;
        let new_end = last
            .range
            .end
            .checked_add_signed(changed.len() as isize - document.source().len() as isize)
            .ok_or(EditError::InvalidRange)?;
        let parsed = document.reparse_from_change(
            changed,
            plumb_syntax::SourceChange {
                old_range: first.range.start..last.range.end,
                new_range: first.range.start..new_end,
            },
        );
        if !parsed.document.is_valid() {
            return Err(EditError::GeneratedInvalid);
        }
    }
    Ok(edits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_root_declarations_keep_properties_before_task_and_append_after_metadata() {
        let document = GreenDocument::parse(
            "`= title Plan\n\nBody\n",
        );
        let edits = edit_green_root_declarations(
            &document,
            &[
                RootDeclarationEdit::SetFacet { name: "task".into(), present: true },
                RootDeclarationEdit::SetProperty(OwnedDeclaration::scalar("created", "2026-09-24T09:00:00+08:00")),
            ],
        )
        .unwrap();
        let source = apply_text_edits(document.source().to_owned(), edits).unwrap();
        assert_eq!(
            source,
            "`= created 2026-09-24T09:00:00+08:00\n\n`+ task\n\n`= title Plan\n\nBody\n"
        );

        let document = GreenDocument::parse(&source);
        let edits = edit_green_root_declarations(
            &document,
            &[RootDeclarationEdit::SetProperty(OwnedDeclaration::scalar(
                "focused",
                "2026-09-24T10:00:00+08:00--",
            ))],
        )
        .unwrap();
        let source = apply_text_edits(document.source().to_owned(), edits).unwrap();
        assert_eq!(
            source,
            "`= focused 2026-09-24T10:00:00+08:00--\n\n`= created 2026-09-24T09:00:00+08:00\n\n`+ task\n\n`= title Plan\n\nBody\n"
        );
    }

    #[test]
    fn root_transaction_updates_cross_shard_properties_and_preserves_body() {
        for newline in ["\n", "\r\n"] {
            let body = "`- Child\n `+ task\n\nUnusual   spacing `!{重要}.\n".replace('\n', newline);
            let source = format!("`= title Original{newline}{newline}{body}{newline}`= focused 2026-09-21T09:00:00+08:00--{newline}");
            let document = GreenDocument::parse(&source);
            let intents = [
                RootDeclarationEdit::SetFacet {
                    name: "task".into(),
                    present: true,
                },
                RootDeclarationEdit::SetProperty(OwnedDeclaration::scalar("title", "New title")),
                RootDeclarationEdit::SetProperty(OwnedDeclaration::scalar(
                    "done",
                    "2026-09-21T10:00:00+08:00",
                )),
                RootDeclarationEdit::SetProperty(OwnedDeclaration::scalar(
                    "focused",
                    "2026-09-21T09:00:00+08:00--2026-09-21T10:00:00+08:00",
                )),
            ];
            let edits = edit_green_root_declarations(&document, &intents).unwrap();
            assert!(edits
                .windows(2)
                .all(|pair| pair[0].range.end <= pair[1].range.start));
            let result = apply_text_edits(source, edits).unwrap();
            assert!(result.contains(&body));
            if newline == "\r\n" {
                assert!(!result.replace("\r\n", "").contains('\n'));
            }
            let updated = GreenDocument::parse(result);
            assert!(updated.is_valid());
            assert_eq!(
                green_root_declaration(&updated, "title").unwrap(),
                Some(OwnedDeclaration::scalar("title", "New title"))
            );
            assert!(edit_green_root_declarations(&updated, &intents)
                .unwrap()
                .is_empty());
        }
    }

    #[test]
    fn removing_root_facet_preserves_properties_nested_facets_and_history() {
        let source = "`+ task\n`= focused 2026-09-21T09:00:00+08:00--\n\n`- Child\n `+ task\n\n`+ task\n`+ custom\n";
        let edits = edit_green_root_declarations(
            &GreenDocument::parse(source),
            &[RootDeclarationEdit::SetFacet {
                name: "task".into(),
                present: false,
            }],
        )
        .unwrap();
        assert_eq!(
            apply_text_edits(source.into(), edits).unwrap(),
            source
                .replacen("`+ task\n", "", 1)
                .replace("\n`+ task\n", "\n")
        );
    }

    #[test]
    fn root_transaction_rejects_duplicate_properties_without_partial_result() {
        let document = GreenDocument::parse("`= title One\n\nBody.\n\n`= title Two\n");
        assert_eq!(
            edit_green_root_declarations(
                &document,
                &[
                    RootDeclarationEdit::SetFacet {
                        name: "task".into(),
                        present: true
                    },
                    RootDeclarationEdit::SetProperty(OwnedDeclaration::scalar(
                        "title",
                        "Replacement"
                    )),
                ]
            ),
            Err(EditError::GeneratedInvalid)
        );
        assert!(green_root_declaration(&document, "title").is_err());
    }

    #[test]
    fn root_property_identity_uses_full_head_for_child_bearing_declarations() {
        let source = "`= due dates\n `+ tomorrow\n\nBody.\n";
        let green = GreenDocument::parse(source);
        assert_eq!(green_root_declaration(&green, "due").unwrap(), None);
        let edits = edit_green_root_declarations(
            &green,
            &[RootDeclarationEdit::SetProperty(OwnedDeclaration::scalar(
                "due",
                "2026-09-21T09:00:00+08:00",
            ))],
        )
        .unwrap();
        let result = apply_text_edits(source.into(), edits).unwrap();
        assert!(result.contains(source));
    }

    #[test]
    fn root_properties_convert_between_scalar_and_history_list() {
        let document = GreenDocument::parse("`= focused start--end\n\nBody.\n");
        let history = OwnedDeclaration::list("focused", ["start--end", "later--"]);
        let edits = edit_green_root_declarations(
            &document,
            &[RootDeclarationEdit::SetProperty(history.clone())],
        )
        .unwrap();
        let result =
            GreenDocument::parse(apply_text_edits(document.source().into(), edits).unwrap());
        assert_eq!(
            green_root_declaration(&result, "focused").unwrap(),
            Some(history)
        );
        let scalar = OwnedDeclaration::scalar("focused", "start--end");
        let edits = edit_green_root_declarations(
            &result,
            &[RootDeclarationEdit::SetProperty(scalar.clone())],
        )
        .unwrap();
        let result = GreenDocument::parse(apply_text_edits(result.source().into(), edits).unwrap());
        assert_eq!(
            green_root_declaration(&result, "focused").unwrap(),
            Some(scalar)
        );
    }
}
