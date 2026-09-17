//! Version-matched, one-off migration helper; not a legacy reader in plumb.
//! The Python driver owns preview, backups and guarded filesystem writes.
use std::io::{self, Read};

use plumb_edit::{apply_text_edits, replace_owned_inline, OwnedInline};
use plumb_semantics::{analyze_document, has_embed_facet, owner_semantic_view};
use plumb_syntax::{
    attributes_from_inlines, inline_range, parse, AttrItem, Block, Inline, InlineContent,
};

fn legacy_facet(content: &InlineContent) -> Option<&'static str> {
    content.items.iter().find_map(|inline| {
        let Inline::Group {
            mark: Some(mark),
            content,
            ..
        } = inline
        else {
            return None;
        };
        if mark.marker != "+" {
            return None;
        }
        match plumb_syntax::plain_scalar(&content.trim_boundary_padding()).as_deref() {
            Some("img") => Some("img"),
            Some("file") => Some("file"),
            _ => None,
        }
    })
}

fn candidate(content: &InlineContent) -> Option<&Inline> {
    for inline in &content.items {
        if let Inline::Group { mark, content, .. } = inline {
            if let Some(nested) = candidate(content) {
                return Some(nested);
            }
            let kind = mark.as_ref().map(|mark| mark.marker.as_str());
            if matches!(kind, Some("img" | "image" | "file"))
                || (kind != Some("->") && has_embed_facet(content))
                || legacy_facet(content).is_some()
            {
                return Some(inline);
            }
        }
    }
    None
}

fn in_blocks(blocks: &[Block]) -> Result<Option<&Inline>, String> {
    for block in blocks {
        if let Block::Parsed(block) = block {
            if block
                .mark
                .as_ref()
                .is_some_and(|m| matches!(m.marker.as_str(), "img" | "image" | "file"))
            {
                return Err("legacy block resource requires manual review".into());
            }
            if let Some(inline) = candidate(&block.content) {
                return Ok(Some(inline));
            }
            if let Some(inline) = in_blocks(&block.children)? {
                return Ok(Some(inline));
            }
        }
    }
    Ok(None)
}

fn replacement(source: &str, inline: &Inline) -> Result<OwnedInline, String> {
    let Inline::Group { mark, content, .. } = inline else {
        unreachable!()
    };
    let kind = mark.as_ref().map(|mark| mark.marker.as_str());
    if !matches!(kind, Some("img" | "image" | "file")) {
        if kind.is_some() && kind != Some("->") {
            return Err("resource facet on non-link marker requires manual review".into());
        }
        if let Some(facet) = legacy_facet(content) {
            let attrs = attributes_from_inlines(source, content);
            let facets = attrs.items.iter().filter(|item| matches!(item, AttrItem::Class { value, .. } if matches!(value.as_str(), "img" | "file" | "embed"))).count();
            if facets != 1
                || attrs
                    .items
                    .iter()
                    .any(|item| matches!(item, AttrItem::Pair { key, .. } if key == "src"))
            {
                return Err("ambiguous legacy resource facets require manual review".into());
            }
            let view = owner_semantic_view(content);
            let args = view.split_first().ok_or("missing target")?;
            let target = if args.rest.is_empty() {
                plumb_semantics::semantic_plain_text(args.first)
            } else {
                args.rest_plain_text()
            };
            let explicit = attrs.items.iter().find_map(|item| match item {
                AttrItem::Pair { key, value, .. } if key == "type" => Some(value.decoded.as_str()),
                _ => None,
            });
            let embed =
                facet == "img" || plumb_semantics::embed_media_type(&target, explicit).is_some();
            let members = content
                .items
                .iter()
                .filter_map(|inline| {
                    if let Inline::Group {
                        mark: Some(mark),
                        content,
                        ..
                    } = inline
                    {
                        if mark.marker == "+"
                            && plumb_syntax::plain_scalar(&content.trim_boundary_padding())
                                .as_deref()
                                == Some(facet)
                        {
                            return embed.then(|| {
                                OwnedInline::with_arguments(
                                    "+",
                                    vec![vec![OwnedInline::Text("embed".into())]],
                                    vec![],
                                )
                            });
                        }
                    }
                    Some(OwnedInline::from_syntax(inline))
                })
                .collect();
            return Ok(OwnedInline::Element {
                kind: "->".into(),
                members: vec![plumb_edit::OwnedInlineMember::ParsedArgument(members)],
            });
        }
        let mut owned = OwnedInline::from_syntax(inline);
        let OwnedInline::Element { kind, .. } = &mut owned else {
            unreachable!()
        };
        *kind = "->".into();
        return Ok(owned);
    }
    if has_embed_facet(content) || legacy_facet(content).is_some() {
        return Err(
            "mixed legacy resource marker and resource facet requires manual review".into(),
        );
    }
    let attrs = attributes_from_inlines(source, content);
    let sources = attrs
        .items
        .iter()
        .filter_map(|item| match item {
            AttrItem::Pair {
                key, value, range, ..
            } if key == "src" => Some((value, range)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(target, source_range)] = sources.as_slice() else {
        return Err("legacy resource must have exactly one src property".into());
    };
    if !plumb_semantics::resource_target_is_valid(&target.decoded) {
        return Err("legacy resource has invalid target".into());
    }
    let view = owner_semantic_view(content);
    let label = view
        .visible_content()
        .map(|visible| visible.items.iter().map(OwnedInline::from_syntax).collect())
        .unwrap_or_default();
    let mut children = content
        .items
        .iter()
        .filter(|inline| {
            !inline.is_whitespace()
                && inline_range(inline) != *source_range
                && !view
                    .positional
                    .iter()
                    .any(|pos| &pos.range == inline_range(inline))
        })
        .map(OwnedInline::from_syntax)
        .collect::<Vec<_>>();
    let explicit = attrs.items.iter().find_map(|item| match item {
        AttrItem::Pair { key, value, .. } if key == "type" => Some(value.decoded.as_str()),
        _ => None,
    });
    if kind != Some("file")
        || plumb_semantics::embed_media_type(&target.decoded, explicit).is_some()
    {
        children.push(OwnedInline::with_arguments(
            "+",
            vec![vec![OwnedInline::Text("embed".into())]],
            vec![],
        ));
    }
    Ok(OwnedInline::with_arguments(
        "->",
        vec![
            label,
            vec![OwnedInline::Verbatim {
                kind: String::new(),
                text: target.decoded.clone(),
            }],
        ],
        children,
    ))
}

fn migrate(mut source: String) -> Result<(String, usize), String> {
    let mut count = 0;
    loop {
        let parsed = parse(&source);
        let valid = parsed
            .valid_syntax()
            .ok_or("invalid syntax; migration aborted")?;
        let Some(inline) = in_blocks(&parsed.syntax.blocks)? else {
            let output = analyze_document(valid);
            if output
                .diagnostics()
                .iter()
                .any(|d| d.code.starts_with("resource.") || d.code.starts_with("embed."))
            {
                return Err("resource diagnostics remain after migration".into());
            }
            plumb_export::export(&source)?;
            return Ok((source, count));
        };
        let owned = replacement(&source, inline)?;
        let edit = replace_owned_inline(&parsed, inline_range(inline).clone(), &owned)
            .map_err(|error| format!("{error:?}"))?;
        source = apply_text_edits(source, vec![edit]).map_err(|error| format!("{error:?}"))?;
        count += 1;
    }
}

fn main() -> Result<(), String> {
    let mut source = String::new();
    io::stdin()
        .read_to_string(&mut source)
        .map_err(|e| e.to_string())?;
    let (source, count) = migrate(source)?;
    println!("{}", serde_json::json!({"source": source, "count": count}));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn images(value: &Value, found: &mut Vec<Value>) {
        match value {
            Value::Object(object) => {
                if object.get("t").and_then(Value::as_str) == Some("Image") {
                    found.push(value.clone());
                }
                for child in object.values() {
                    images(child, found);
                }
            }
            Value::Array(array) => {
                for child in array {
                    images(child, found);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn migrates_all_legacy_markers_and_anonymous_facets_idempotently() {
        let source = "Before `image{{Rich `!{alt}} `={src {static/图 像.png}} `@{figure} `+{wide}} after.\r\n`img{{} `={src static/empty.png}}\r\n`file{Demo video `={src static/demo.mp4}}\r\n{static/anonymous.png `+{img}}\r\n";
        let (result, count) = migrate(source.into()).unwrap();
        assert_eq!(count, 4);
        assert!(result.starts_with("Before `->{"));
        assert!(result.contains(" after.\r\n"));
        assert!(!result.contains("`={src"));
        let export = plumb_export::export(&result).unwrap();
        let mut found = Vec::new();
        images(&export, &mut found);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0]["c"][2][0], "static/图 像.png");
        assert_eq!(found[0]["c"][0][0], "figure");
        assert_eq!(found[0]["c"][0][1], serde_json::json!(["wide"]));
        assert_eq!(found[1]["c"][1], serde_json::json!([]));
        assert!(export.to_string().contains("data-plumb-facet"));
        assert_eq!(migrate(result.clone()).unwrap(), (result, 0));
    }

    #[test]
    fn downloads_become_plain_links_and_media_become_embeds() {
        for legacy in [
            "`file{Manual `={src static/manual.pdf}}\n",
            "{Manual static/manual.pdf `+{ file }}\n",
            "`->{Manual static/manual.pdf `+{file}}\n",
        ] {
            let (result, count) = migrate(legacy.into()).unwrap();
            assert_eq!(count, 1);
            assert!(result.starts_with("`->{"));
            assert!(!result.contains("`+{embed}"), "{result}");
            assert_eq!(migrate(result.clone()).unwrap(), (result, 0));
        }
        for legacy in [
            "`file{Song `={src static/SONG.MP3}}\n",
            "`file{Song `={src https://example.test/stream} `={type audio/ogg}}\n",
            "{Photo static/photo.png `+{ img }}\n",
        ] {
            let (result, count) = migrate(legacy.into()).unwrap();
            assert_eq!(count, 1);
            assert!(result.contains("`+{embed}"), "{result}");
            assert_eq!(migrate(result.clone()).unwrap(), (result, 0));
        }
        let (result, _) =
            migrate("`file{X `={src photo.png} `={type unknown/type}}\n".into()).unwrap();
        assert!(!result.contains("`+{embed}"));
        assert!(result.contains("unknown/type"));
    }

    #[test]
    fn refuses_ambiguous_or_invalid_sources() {
        for source in [
            "`img{label}\n",
            "`image{x `={src a} `={src b}}\n",
            "`file{x `={src /absolute}}\n",
            "`img{x `={src a} `+{img}}\n",
            "`node{a `+{img}}\n",
            "`img label\n",
            "`image{broken\n",
        ] {
            assert!(migrate(source.into()).is_err(), "{source}");
        }
    }

    #[test]
    fn leaves_raw_examples_and_existing_links_untouched() {
        let source = "`plumb\"\n `img{x `={src a.png}}\n\n`->{a.png `+{embed}}\n";
        assert_eq!(migrate(source.into()).unwrap(), (source.into(), 0));
    }
}
