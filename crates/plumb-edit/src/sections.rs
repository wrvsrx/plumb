//! Explicit conversion of legacy section ownership and table spellings.
use crate::{EditError, TextEdit};
use plumb_syntax::{AttrItem, Block, Inline, ParsedDocument};

fn level(block: &Block, max: usize, prefix: Option<&str>) -> Option<usize> {
    let Block::Parsed(block) = block else {
        return None;
    };
    let marker = &block.mark.as_ref()?.marker;
    if let Some(prefix) = prefix {
        return marker
            .strip_prefix(prefix)?
            .parse::<usize>()
            .ok()
            .filter(|level| *level > 0);
    }
    ((1..=max).contains(&marker.len()) && marker.bytes().all(|b| b == b'#')).then_some(marker.len())
}
fn convert(blocks: Vec<Block>, max: usize, prefix: Option<&str>) -> Vec<Block> {
    let mut input = blocks.into_iter().peekable();
    let mut output = Vec::new();
    while let Some(mut block) = input.next() {
        if let Some(depth) = level(&block, max, prefix) {
            let mut body = Vec::new();
            while input
                .peek()
                .is_some_and(|next| level(next, max, prefix).is_none_or(|next| next > depth))
            {
                body.push(input.next().unwrap());
            }
            if let Block::Parsed(block) = &mut block {
                block.mark.as_mut().unwrap().marker = "#".into();
                block.children.extend(body);
            }
        }
        if let Block::Parsed(block) = &mut block {
            block.children = convert(std::mem::take(&mut block.children), max, prefix);
        }
        output.push(block);
    }
    output
}
/// Canonical spellings only: never infer section ownership from following siblings.
pub fn canonicalize_structure_markers(parsed: &ParsedDocument) -> Result<Vec<TextEdit>, EditError> {
    parsed.valid_syntax().ok_or(EditError::GeneratedInvalid)?;
    let mut render = parsed.clone();
    canonicalize(&mut render.syntax.blocks, Context::Document);
    finish(parsed, render)
}
#[derive(Clone, Copy)]
enum Context {
    Document,
    Body,
    Table,
    Row,
    Cell,
    Data,
}
fn declaration(block: &plumb_syntax::ParsedBlock) -> bool {
    block
        .mark
        .as_ref()
        .is_some_and(|mark| matches!(mark.marker.as_str(), "@" | "+" | "="))
}
fn canonicalize(blocks: &mut [Block], context: Context) {
    for block in blocks {
        let Block::Parsed(block) = block else {
            continue;
        };
        // Root declarations and property values are data, not document body.
        let data = matches!(context, Context::Data)
            || (matches!(context, Context::Document) && declaration(block))
            || block.mark.as_ref().is_some_and(|mark| mark.marker == "=");
        if data {
            continue;
        }
        let structural = matches!(context, Context::Table | Context::Row);
        if !structural {
            if let Some(mark) = &mut block.mark {
                if mark.marker == "table" {
                    mark.marker = "|".into();
                }
                if matches!(mark.marker.as_str(), "-" | "." | "#")
                    && mark.attrs.has_class("section")
                {
                    mark.marker = "#".into();
                    let declarations = mark
                        .attrs
                        .items
                        .iter()
                        .filter_map(|item| match item {
                            AttrItem::Class { value, range, .. } if value == "section" => {
                                Some(range.clone())
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    block.children.retain(|child| {
                        !declarations
                            .iter()
                            .any(|range| range.start == child.range().start)
                    });
                    mark.attrs.items.retain(
                        |item| !matches!(item, AttrItem::Class { value, .. } if value == "section"),
                    );
                }
            }
        }
        let child_context = if matches!(context, Context::Table) {
            if block.mark.as_ref().is_some_and(|mark| mark.marker == "-")
                && block.content.is_empty()
            {
                Context::Row
            } else {
                Context::Data
            }
        } else if matches!(context, Context::Row) {
            Context::Cell
        } else if block.mark.as_ref().is_some_and(|mark| mark.marker == "|") && !structural {
            Context::Table
        } else {
            Context::Body
        };
        canonicalize(&mut block.children, child_context);
    }
}
/// Explicit legacy migration; never invoked by parsing or ordinary formatting.
pub fn migrate_hash_sections(parsed: &ParsedDocument) -> Result<Vec<TextEdit>, EditError> {
    sectionize(parsed, 6, None)
}
/// Import-only conversion of validated Pandoc heading intermediates.
pub fn sectionize_imported_headings(
    parsed: &ParsedDocument,
    marker_prefix: &str,
) -> Result<Vec<TextEdit>, EditError> {
    sectionize(parsed, usize::MAX, Some(marker_prefix))
}
fn sectionize(
    parsed: &ParsedDocument,
    max: usize,
    prefix: Option<&str>,
) -> Result<Vec<TextEdit>, EditError> {
    parsed.valid_syntax().ok_or(EditError::GeneratedInvalid)?;
    // Own structure while retaining original inline envelopes, escapes and spaces.
    // This private formatter input is never published as a source revision.
    let mut render = parsed.clone();
    let mut declarations = Vec::new();
    let mut body = Vec::new();
    for block in std::mem::take(&mut render.syntax.blocks) {
        let declaration = matches!(&block, Block::Parsed(block) if block.mark.as_ref().is_some_and(|mark| matches!(mark.marker.as_str(), "=" | "+" | "@")));
        if declaration {
            declarations.push(block);
        } else {
            body.push(block);
        }
    }
    declarations.extend(convert(body, max, prefix));
    render.syntax.blocks = declarations;
    // Only the legacy operation recognizes old table and explicit section spellings.
    if prefix.is_none() {
        canonicalize(&mut render.syntax.blocks, Context::Document);
    }
    finish(parsed, render)
}
fn finish(parsed: &ParsedDocument, mut render: ParsedDocument) -> Result<Vec<TextEdit>, EditError> {
    fn reindent(blocks: &mut [Block], depth: usize, source: &mut String) {
        for block in blocks {
            if let Block::Parsed(block) = block {
                for inline in &mut block.content.items {
                    if let Inline::SoftBreak { range } = inline {
                        let start = source.len();
                        source.push('\n');
                        source.extend(std::iter::repeat_n(' ', depth + 1));
                        *range = start..source.len();
                    }
                }
                reindent(&mut block.children, depth + 1, source);
            }
        }
    }
    reindent(&mut render.syntax.blocks, 0, &mut render.source);
    let rendered = plumb_format::format_parsed(&render).map_err(|_| EditError::GeneratedInvalid)?;
    if !plumb_syntax::parse(&rendered).is_valid() {
        return Err(EditError::GeneratedInvalid);
    }
    if rendered == parsed.source {
        return Ok(Vec::new());
    }
    Ok(vec![TextEdit::replace(
        parsed,
        0..parsed.source.len(),
        rendered,
    )?])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_markers_preserve_siblings_and_table_structural_roles() {
        let source = "`- Title\n `+ section\n `+ task\n\n Body\n\nFollowing sibling\n\n`table Caption\n `- compact\n  `+ section\n `-\n  `- Cell\n   `+ section\n\n   `- Inner\n    `+ section\n\n`= data\n `- Value\n  `+ section\n";
        let parsed = plumb_syntax::parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let result = crate::apply_text_edits(
            source.into(),
            canonicalize_structure_markers(&parsed).unwrap(),
        )
        .unwrap();
        let compact = result.replace("\n\n", "\n");
        assert!(compact.starts_with("`# Title\n `+ task"), "{result}");
        assert!(result.contains("\nFollowing sibling\n"), "{result}");
        assert!(
            compact.contains("`| Caption\n `- compact\n  `+ section"),
            "{result}"
        );
        assert!(compact.contains("  `- Cell\n   `+ section"), "{result}");
        assert!(result.contains("   `# Inner"), "{result}");
        assert!(
            compact.contains("`= data\n `- Value\n  `+ section"),
            "{result}"
        );
        let parsed = plumb_syntax::parse(result);
        assert!(parsed.is_valid());
        assert!(canonicalize_structure_markers(&parsed).unwrap().is_empty());
    }
    #[test]
    fn legacy_migration_converts_table_and_ignores_raw_payload() {
        let source = "`# Heading\n\n`table Caption\n `- a b\n\n`text\"\n `table literal\n";
        let parsed = plumb_syntax::parse(source);
        let result =
            crate::apply_text_edits(source.into(), migrate_hash_sections(&parsed).unwrap())
                .unwrap();
        assert!(result.contains(" `| Caption"), "{result}");
        assert!(result.contains("  `table literal"), "{result}");
    }
    #[test]
    fn migration_preserves_inline_spelling_and_wrapping() {
        let source = concat!(
            "`# Head `!{bold}\n",
            " continuation `\"{}\" `\"{literal { braces }}\"\n\n",
            "`node Escapes `{ `} ``   { group } `\"\"{quote }\" safe}\"\"\n\n",
            "`text\"\n raw { x }\n",
        );
        let parsed = plumb_syntax::parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let result =
            crate::apply_text_edits(source.into(), migrate_hash_sections(&parsed).unwrap())
                .unwrap();
        for spelling in [
            "`\"{}\"",
            "`\"{literal { braces }}\"",
            "Escapes `{ `} ``   {group}",
            "`\"\"{quote }\" safe}\"\"",
        ] {
            assert!(result.contains(spelling), "missing {spelling:?}: {result}");
        }
        assert!(result.contains("\n continuation"));
        assert!(result.contains("\n  raw { x }\n"));
        let parsed = plumb_syntax::parse(&result);
        assert!(parsed.is_valid(), "{result}");
        assert!(migrate_hash_sections(&parsed).unwrap().is_empty());
    }
    #[test]
    fn migration_preserves_crlf_and_document_declarations() {
        let source = "`# One\r\n\r\nBody\r\n\r\n`= title Document\r\n\r\n`## Two\r\n\r\nTail\r\n";
        let parsed = plumb_syntax::parse(source);
        let result =
            crate::apply_text_edits(source.to_owned(), migrate_hash_sections(&parsed).unwrap())
                .unwrap();
        assert!(result.starts_with("`= title Document\r\n"), "{result:?}");
        assert!(!result.replace("\r\n", "").contains('\n'));
        let parsed = plumb_syntax::parse(result);
        assert!(parsed.is_valid());
        assert!(migrate_hash_sections(&parsed).unwrap().is_empty());
    }
    #[test]
    fn migration_is_explicit_and_rejects_invalid_source() {
        let source = "`# Generic\n";
        let parsed = plumb_syntax::parse(source);
        assert!(crate::format(&parsed, crate::FormatScope::Document)
            .unwrap()
            .is_empty());
        assert!(migrate_hash_sections(&parsed).unwrap().is_empty());
        assert!(migrate_hash_sections(&plumb_syntax::parse("`# Broken `span{\n")).is_err());
    }
    #[test]
    fn migration_preserves_ids_and_verbatim() {
        let source = "`# One\n `@ one\n\nBody\n\n`## Two\n\n`rust\"\n `# literal\n\n`# End\n";
        let parsed = plumb_syntax::parse(source);
        let result =
            crate::apply_text_edits(source.to_string(), migrate_hash_sections(&parsed).unwrap())
                .unwrap();
        let parsed = plumb_syntax::parse(&result);
        assert!(parsed.is_valid(), "{result}");
        assert!(result.contains("`@ one"));
        assert!(result.contains("`# literal"));
        assert!(migrate_hash_sections(&parsed).unwrap().is_empty());
        assert_eq!(parsed.syntax.blocks.len(), 2);
    }
}
