use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read};
use std::process::ExitCode;

use plumb_core::{parse, AttrItem, Attributes, Block, Inline, InlineContent, ParsedBlock};
use plumb_extensions::{
    analyze_document, CitationRecord, DocumentOutput, EmphasisKind, ListGroup, ListKind,
    MetadataBlock, MetadataEntry, MetadataValue,
};
use serde_json::{json, Map, Value};

pub fn run_cli(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let mut args = args.into_iter().skip(1);
    let path = args.next();
    if args.next().is_some() {
        eprintln!("plumb export: expected at most one input path");
        return ExitCode::from(2);
    }
    let input = match read_input(path.as_deref()) {
        Ok(input) => input,
        Err(error) => {
            eprintln!("plumb export: {error}");
            return ExitCode::FAILURE;
        }
    };
    match export(&input) {
        Ok(document) => {
            println!("{document}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("plumb export: {error}");
            ExitCode::FAILURE
        }
    }
}

fn read_input(path: Option<&OsStr>) -> Result<String, String> {
    match path {
        Some(path) => fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.to_string_lossy())),
        None => {
            let mut input = String::new();
            io::stdin()
                .read_to_string(&mut input)
                .map_err(|error| format!("cannot read stdin: {error}"))?;
            Ok(input)
        }
    }
}

fn export(source: &str) -> Result<Value, String> {
    let parsed = parse(source);
    if !parsed.is_valid() {
        let summary = parsed
            .diagnostics
            .iter()
            .map(|diagnostic| {
                format!(
                    "{} at bytes {}..{}: {}",
                    diagnostic.code,
                    diagnostic.range.start,
                    diagnostic.range.end,
                    diagnostic.message
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!("document has syntax errors:\n{summary}"));
    }
    let analysis = analyze_document(&parsed.source, &parsed.syntax);
    let metadata = lower_metadata(analysis.metadata.metadata.as_ref(), &analysis)?;
    Ok(json!({
        "pandoc-api-version": [1, 23, 1],
        "meta": metadata,
        "blocks": lower_blocks(&parsed.syntax.blocks, &analysis),
    }))
}

fn lower_metadata(
    metadata: Option<&MetadataBlock>,
    analysis: &DocumentOutput,
) -> Result<Value, String> {
    let Some(metadata) = metadata else {
        return Ok(json!({}));
    };
    Ok(Value::Object(lower_metadata_entries(
        &metadata.entries,
        analysis,
    )?))
}

fn lower_metadata_entries(
    entries: &[MetadataEntry],
    analysis: &DocumentOutput,
) -> Result<Map<String, Value>, String> {
    let mut output = Map::new();
    for entry in entries {
        if output.contains_key(&entry.key) {
            continue;
        }
        output.insert(
            entry.key.clone(),
            lower_metadata_value(&entry.key, &entry.value, analysis)?,
        );
    }
    Ok(output)
}

fn lower_metadata_value(
    key: &str,
    value: &MetadataValue,
    analysis: &DocumentOutput,
) -> Result<Value, String> {
    match value {
        MetadataValue::Null { .. } => Ok(json!({ "t": "MetaString", "c": "" })),
        MetadataValue::Scalar { content, .. } => Ok(json!({
            "t": "MetaInlines",
            "c": lower_inlines(content, analysis),
        })),
        MetadataValue::List { items, .. } => Ok(json!({
            "t": "MetaList",
            "c": items
                .iter()
                .map(|item| lower_metadata_value(key, &item.value, analysis))
                .collect::<Result<Vec<_>, _>>()?,
        })),
        MetadataValue::Map { entries, .. } => Ok(json!({
            "t": "MetaMap",
            "c": lower_metadata_entries(entries, analysis)?,
        })),
        MetadataValue::Verbatim { text, .. } => Ok(json!({ "t": "MetaString", "c": text })),
        MetadataValue::Unsupported { .. } => {
            Err(format!("metadata field '{key}' has an unsupported value"))
        }
    }
}

fn lower_blocks(blocks: &[Block], analysis: &DocumentOutput) -> Vec<Value> {
    let mut output = Vec::new();
    let mut index = 0;
    while index < blocks.len() {
        if let Block::Parsed(block) = &blocks[index] {
            if let Some(definitions) = analysis
                .metadata
                .definition_list_at_node_start(block.range.start)
            {
                let end = index + definitions.definitions.len();
                output.push(lower_definition_list(
                    &blocks[index..end],
                    definitions,
                    analysis,
                ));
                index = end;
                continue;
            }
            if let Some(group) = analysis.lists.group_at_node_start(block.range.start) {
                let end = index + group.items.len();
                output.push(lower_list_group(&blocks[index..end], group, analysis));
                index = end;
                continue;
            }
        }
        match &blocks[index] {
            Block::Verbatim(block) => {
                if analysis
                    .math
                    .math_at_node_start(block.range.start)
                    .is_some()
                {
                    let math = json!({
                        "t": "Math",
                        "c": [{ "t": "DisplayMath" }, block.text],
                    });
                    let paragraph = json!({ "t": "Para", "c": [math] });
                    if has_unconsumed_math_attrs(&block.attrs) {
                        output.push(json!({
                            "t": "Div",
                            "c": [lower_math_attrs(&block.attrs), [paragraph]],
                        }));
                    } else {
                        output.push(paragraph);
                    }
                } else {
                    output.push(json!({
                        "t": "CodeBlock",
                        "c": [lower_attrs(&block.attrs, None), block.text],
                    }));
                }
            }
            Block::Parsed(parsed) => lower_parsed_block(parsed, analysis, &mut output),
        }
        index += 1;
    }
    output
}

fn lower_definition_list(
    blocks: &[Block],
    definitions: &plumb_extensions::DefinitionList,
    analysis: &DocumentOutput,
) -> Value {
    let entries = blocks
        .iter()
        .zip(&definitions.definitions)
        .map(|(block, definition)| {
            let Block::Parsed(block) = block else {
                unreachable!("a definition list contains only parsed definition blocks")
            };
            let mark = block.mark.as_ref().expect("a definition has a mark");
            let mut body = lower_blocks(&block.children, analysis);
            if !mark.attrs.items.is_empty() {
                body = vec![json!({
                    "t": "Div",
                    "c": [lower_attrs(&mark.attrs, None), body],
                })];
            }
            json!([lower_inlines(&definition.term, analysis), [body],])
        })
        .collect::<Vec<_>>();
    json!({ "t": "DefinitionList", "c": entries })
}

fn lower_list_group(blocks: &[Block], group: &ListGroup, analysis: &DocumentOutput) -> Value {
    let items = blocks
        .iter()
        .map(|block| {
            let Block::Parsed(block) = block else {
                unreachable!("a list group contains only parsed item blocks")
            };
            let mark = block.mark.as_ref().expect("a list item has a mark");
            let mut contents = Vec::new();
            if !block.head.items.is_empty() {
                contents.push(json!({ "t": "Para", "c": lower_inlines(&block.head, analysis) }));
            }
            contents.extend(lower_blocks(&block.children, analysis));
            if mark.attrs.items.is_empty() {
                contents
            } else {
                vec![json!({
                    "t": "Div",
                    "c": [lower_attrs(&mark.attrs, None), contents],
                })]
            }
        })
        .collect::<Vec<_>>();
    match group.kind {
        ListKind::Bullet => json!({ "t": "BulletList", "c": items }),
        ListKind::Ordered => json!({
            "t": "OrderedList",
            "c": [[1, { "t": "Decimal" }, { "t": "Period" }], items],
        }),
    }
}

fn lower_parsed_block(block: &ParsedBlock, analysis: &DocumentOutput, output: &mut Vec<Value>) {
    if analysis
        .metadata
        .metadata
        .as_ref()
        .is_some_and(|metadata| metadata.range.start == block.range.start)
    {
        return;
    }
    if let Some(heading) = analysis.headings.heading_at_node_start(block.range.start) {
        let attrs = &block.mark.as_ref().expect("heading has mark").attrs;
        output.push(json!({
            "t": "Header",
            "c": [heading.level, lower_attrs(attrs, None), lower_inlines(&block.head, analysis)],
        }));
        output.extend(lower_blocks(&block.children, analysis));
        return;
    }

    if analysis
        .quotes
        .quote_at_node_start(block.range.start)
        .is_some()
    {
        let mark = block.mark.as_ref().expect("a quote has a mark");
        let mut contents = Vec::new();
        if !block.head.items.is_empty() {
            contents.push(json!({ "t": "Para", "c": lower_inlines(&block.head, analysis) }));
        }
        contents.extend(lower_blocks(&block.children, analysis));
        if !mark.attrs.items.is_empty() {
            contents = vec![json!({
                "t": "Div",
                "c": [lower_attrs(&mark.attrs, None), contents],
            })];
        }
        output.push(json!({ "t": "BlockQuote", "c": contents }));
        return;
    }

    if let Some(mark) = &block.mark {
        let mut contents = Vec::new();
        if !block.head.items.is_empty() {
            contents.push(json!({ "t": "Para", "c": lower_inlines(&block.head, analysis) }));
        }
        contents.extend(lower_blocks(&block.children, analysis));
        output.push(json!({
            "t": "Div",
            "c": [lower_attrs(&mark.attrs, (mark.marker != "div").then_some(mark.marker.as_str())), contents],
        }));
    } else {
        output.push(json!({ "t": "Para", "c": lower_inlines(&block.head, analysis) }));
    }
}

fn lower_inlines(content: &InlineContent, analysis: &DocumentOutput) -> Vec<Value> {
    let mut output = Vec::new();
    for inline in &content.items {
        match inline {
            Inline::Text { text, .. } => lower_text(text, &mut output),
            Inline::SoftBreak { .. } => output.push(json!({ "t": "SoftBreak" })),
            Inline::Verbatim {
                range, text, attrs, ..
            } => {
                if let Some(link) = analysis.link_at_node_start(range.start) {
                    output.push(json!({
                        "t": "Link",
                        "c": [lower_autolink_attrs(attrs), text_inlines(text), [&link.target.value, ""]],
                    }));
                } else if analysis.math.math_at_node_start(range.start).is_some() {
                    let math = json!({
                        "t": "Math",
                        "c": [{ "t": "InlineMath" }, text],
                    });
                    if has_unconsumed_math_attrs(attrs) {
                        output.push(json!({
                            "t": "Span",
                            "c": [lower_math_attrs(attrs), [math]],
                        }));
                    } else {
                        output.push(math);
                    }
                } else {
                    output.push(json!({
                        "t": "Code",
                        "c": [lower_attrs(attrs, None), text],
                    }));
                }
            }
            Inline::Element {
                range,
                kind,
                content,
                attrs,
                ..
            } => {
                if let Some(emphasis) = analysis.emphasis.emphasis_at_node_start(range.start) {
                    let semantic = json!({
                        "t": match emphasis.kind {
                            EmphasisKind::Emphasis => "Emph",
                            EmphasisKind::Strong => "Strong",
                        },
                        "c": lower_inlines(content, analysis),
                    });
                    if attrs.items.is_empty() {
                        output.push(semantic);
                    } else {
                        output.push(json!({
                            "t": "Span",
                            "c": [lower_attrs(attrs, None), [semantic]],
                        }));
                    }
                } else if let Some(citation) =
                    analysis.citations.citation_at_node_start(range.start)
                {
                    output.push(lower_citation(citation));
                } else if let Some(image) = analysis.image_at_node_start(range.start) {
                    output.push(json!({
                        "t": "Image",
                        "c": [lower_image_attrs(attrs), lower_inlines(content, analysis), [&image.source.value, ""]],
                    }));
                } else if let Some(link) = analysis.link_at_node_start(range.start) {
                    output.push(json!({
                        "t": "Link",
                        "c": [lower_attrs(attrs, None), lower_inlines(content, analysis), [&link.target.value, ""]],
                    }));
                } else {
                    output.push(json!({
                        "t": "Span",
                        "c": [lower_attrs(attrs, (kind != "span").then_some(kind)), lower_inlines(content, analysis)],
                    }));
                }
            }
        }
    }
    output
}

fn lower_citation(citation: &CitationRecord) -> Value {
    json!({
        "t": "Cite",
        "c": [[{
            "citationId": citation.id,
            "citationPrefix": [],
            "citationSuffix": [],
            "citationMode": { "t": "NormalCitation" },
            "citationNoteNum": 0,
            "citationHash": 0,
        }], text_inlines(&format!("[{}]", citation.id))],
    })
}

fn text_inlines(text: &str) -> Vec<Value> {
    let mut output = Vec::new();
    lower_text(text, &mut output);
    output
}

fn lower_text(text: &str, output: &mut Vec<Value>) {
    for (index, part) in text.split(' ').enumerate() {
        if index > 0 {
            output.push(json!({ "t": "Space" }));
        }
        if !part.is_empty() {
            output.push(json!({ "t": "Str", "c": part }));
        }
    }
}

fn lower_attrs(attrs: &Attributes, semantic_marker: Option<&str>) -> Value {
    lower_attrs_filtered(attrs, semantic_marker, |_| false, |_| false)
}

fn lower_math_attrs(attrs: &Attributes) -> Value {
    lower_attrs_filtered(attrs, None, |class| class == "$", |key| key == "language")
}

fn lower_autolink_attrs(attrs: &Attributes) -> Value {
    lower_attrs_filtered(attrs, None, |class| class == "->", |_| false)
}

fn lower_image_attrs(attrs: &Attributes) -> Value {
    lower_attrs_filtered(attrs, None, |_| false, |key| key == "src")
}

fn lower_attrs_filtered(
    attrs: &Attributes,
    semantic_marker: Option<&str>,
    consume_class: impl Fn(&str) -> bool,
    consume_pair: impl Fn(&str) -> bool,
) -> Value {
    let mut id = String::new();
    let mut classes = Vec::new();
    let mut pairs = Vec::new();
    for item in &attrs.items {
        match item {
            AttrItem::Id { value, .. } => id = value.clone(),
            AttrItem::Class { value, .. } if !consume_class(value) => classes.push(value.clone()),
            AttrItem::Class { .. } => {}
            AttrItem::Pair { key, .. } if consume_pair(key) => {}
            AttrItem::Pair { key, value, .. } if key != "level" => {
                pairs.push(json!([key, value.decoded]));
            }
            AttrItem::Pair { .. } => {}
        }
    }
    if let Some(marker) = semantic_marker {
        pairs.push(json!(["data-plumb-marker", marker]));
    }
    json!([id, classes, pairs])
}

fn has_unconsumed_math_attrs(attrs: &Attributes) -> bool {
    attrs.items.iter().any(|item| match item {
        AttrItem::Id { .. } => true,
        AttrItem::Class { value, .. } => value != "$",
        AttrItem::Pair { key, .. } => key != "language",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_heading_paragraph_and_generic_block() {
        let document =
            export("`#{#intro} Intro\nParagraph text.\n`note{.tip} Remember this.\n").unwrap();
        let blocks = document["blocks"].as_array().unwrap();
        assert_eq!(blocks[0]["t"], "Header");
        assert_eq!(blocks[1]["t"], "Para");
        assert_eq!(blocks[2]["t"], "Div");
    }

    #[test]
    fn exports_adjacent_and_nested_items_as_bullet_lists() {
        let source = "`- One\n`-{.task #two priority=high} Two\n  `- Nested\nParagraph.\n";
        let document = export(source).unwrap();
        let blocks = document["blocks"].as_array().unwrap();

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["t"], "BulletList");
        let items = blocks[0]["c"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0][0]["t"], "Para");
        assert_eq!(items[0][0]["c"][0]["c"], "One");

        let attributed = &items[1][0];
        assert_eq!(attributed["t"], "Div");
        assert_eq!(attributed["c"][0][0], "two");
        assert_eq!(attributed["c"][0][1], json!(["task"]));
        assert_eq!(attributed["c"][0][2], json!([["priority", "high"]]));
        assert_eq!(attributed["c"][1][0]["t"], "Para");
        assert_eq!(attributed["c"][1][1]["t"], "BulletList");
        assert_eq!(attributed["c"][1][1]["c"][0][0]["c"][0]["c"], "Nested");
        assert_eq!(blocks[1]["t"], "Para");
    }

    #[test]
    fn exports_adjacent_and_nested_ordered_items() {
        let source = "`. One\n`. Two\n  `. Nested one\n  `. Nested two\n`- Bullet\n";
        let document = export(source).unwrap();
        let blocks = document["blocks"].as_array().unwrap();

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["t"], "OrderedList");
        assert_eq!(
            blocks[0]["c"][0],
            json!([1, { "t": "Decimal" }, { "t": "Period" }])
        );
        let items = blocks[0]["c"][1].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0][0]["c"][0]["c"], "One");
        assert_eq!(items[1][1]["t"], "OrderedList");
        assert_eq!(items[1][1]["c"][1].as_array().unwrap().len(), 2);
        assert_eq!(blocks[1]["t"], "BulletList");
    }

    #[test]
    fn exports_item_marker_as_a_generic_block() {
        let document = export("`item Not a list item\n").unwrap();
        let blocks = document["blocks"].as_array().unwrap();

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["t"], "Div");
        assert_eq!(blocks[0]["c"][0][2], json!([["data-plumb-marker", "item"]]));
    }

    #[test]
    fn exports_quote_head_children_nesting_and_attributes() {
        let source = "`> Quoted head\n\n  Quoted body.\n\n  `>{#nested .source cite=book} Nested quote\n`quote Generic\n";
        let document = export(source).unwrap();
        let blocks = document["blocks"].as_array().unwrap();

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["t"], "BlockQuote");
        assert_eq!(blocks[0]["c"][0]["t"], "Para");
        assert_eq!(blocks[0]["c"][0]["c"][0]["c"], "Quoted");
        assert_eq!(blocks[0]["c"][1]["t"], "Para");

        let nested = &blocks[0]["c"][2];
        assert_eq!(nested["t"], "BlockQuote");
        assert_eq!(nested["c"][0]["t"], "Div");
        assert_eq!(nested["c"][0]["c"][0][0], "nested");
        assert_eq!(nested["c"][0]["c"][0][1], json!(["source"]));
        assert_eq!(nested["c"][0]["c"][0][2], json!([["cite", "book"]]));
        assert_eq!(nested["c"][0]["c"][1][0]["t"], "Para");

        assert_eq!(blocks[1]["t"], "Div");
        assert_eq!(
            blocks[1]["c"][0][2],
            json!([["data-plumb-marker", "quote"]])
        );
    }

    #[test]
    fn exports_empty_quote() {
        let document = export("`>\n").unwrap();
        assert_eq!(document["blocks"], json!([{"t": "BlockQuote", "c": []}]));
    }

    #[test]
    fn exports_adjacent_definitions_and_preserves_definition_attributes() {
        let source = "`: Term\n\n  Definition.\n\n`:{#tag .kind key=value} Tagged\n  `- First\n  `- Second\n";
        let document = export(source).unwrap();
        let blocks = document["blocks"].as_array().unwrap();

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["t"], "DefinitionList");
        let entries = blocks[0]["c"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0][0][0]["c"], "Term");
        assert_eq!(entries[0][1][0][0]["t"], "Para");

        let attributed = &entries[1][1][0][0];
        assert_eq!(attributed["t"], "Div");
        assert_eq!(attributed["c"][0][0], "tag");
        assert_eq!(attributed["c"][0][1], json!(["kind"]));
        assert_eq!(attributed["c"][0][2], json!([["key", "value"]]));
        assert_eq!(attributed["c"][1][0]["t"], "BulletList");
        assert_eq!(attributed["c"][1][0]["c"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn rejects_syntax_errors() {
        assert!(export("`node{key=a key=b} broken\n").is_err());
    }

    #[test]
    fn exports_links_from_shared_document_facts() {
        let document = export("See `->[target]{to=\"other.plumb#id\"}.\n").unwrap();
        assert_eq!(document["blocks"][0]["c"][2]["t"], "Link");
        assert_eq!(document["blocks"][0]["c"][2]["c"][2][0], "other.plumb#id");
    }

    #[test]
    fn exports_verbatim_autolinks_in_body_and_metadata() {
        let source = "`meta\n  `: homepage\n\n    `[https://example.test/meta]{.->}\n\nBody `[https://example.test/a%20b]{.-> #site .keep rel=nofollow}.\n";
        let document = export(source).unwrap();

        let metadata_link = &document["meta"]["homepage"]["c"][0];
        assert_eq!(metadata_link["t"], "Link");
        assert_eq!(metadata_link["c"][2][0], "https://example.test/meta");

        let body_link = &document["blocks"][0]["c"][2];
        assert_eq!(body_link["t"], "Link");
        assert_eq!(
            body_link["c"][0],
            json!(["site", ["keep"], [["rel", "nofollow"]]])
        );
        assert_eq!(body_link["c"][1][0]["c"], "https://example.test/a%20b");
        assert_eq!(body_link["c"][2][0], "https://example.test/a%20b");
    }

    #[test]
    fn exports_standard_images_in_body_and_metadata() {
        let source = "`meta\n `: cover\n\n    `img[Cover]{src=\"static/cover.png\"}\n\nBefore `img[Rich `em[alt]]{src=\"static/a%20b.webp\" #image .wide loading=lazy} after.\n\n`img[]{src=\"https://example.test/decorative.svg\"}\n";
        let document = export(source).unwrap();

        let metadata_image = &document["meta"]["cover"]["c"][0];
        assert_eq!(metadata_image["t"], "Image");
        assert_eq!(metadata_image["c"][2][0], "static/cover.png");

        let body_image = &document["blocks"][0]["c"][2];
        assert_eq!(body_image["t"], "Image");
        assert_eq!(
            body_image["c"][0],
            json!(["image", ["wide"], [["loading", "lazy"]]])
        );
        assert_eq!(body_image["c"][1][0]["c"], "Rich");
        assert_eq!(body_image["c"][1][2]["t"], "Span");
        assert_eq!(body_image["c"][2][0], "static/a%20b.webp");

        let image_only_paragraph = &document["blocks"][1];
        assert_eq!(image_only_paragraph["t"], "Para");
        assert_eq!(image_only_paragraph["c"][0]["t"], "Image");
        assert!(image_only_paragraph["c"][0]["c"][1]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn exports_link_kind_as_a_generic_span() {
        let document = export("`link[target]{to=\"other.plumb#id\"}\n").unwrap();
        let inline = &document["blocks"][0]["c"][0];

        assert_eq!(inline["t"], "Span");
        assert_eq!(
            inline["c"][0][2],
            json!([["to", "other.plumb#id"], ["data-plumb-marker", "link"]])
        );
    }

    #[test]
    fn exports_verbatim_envelopes_as_pandoc_code() {
        let document =
            export("Use `[cargo check]{language=sh}.\n\n`{language=rust}\n  fn main() {}\n")
                .unwrap();
        assert_eq!(document["blocks"][0]["c"][2]["t"], "Code");
        assert_eq!(document["blocks"][0]["c"][2]["c"][1], "cargo check");
        assert_eq!(document["blocks"][1]["t"], "CodeBlock");
        assert_eq!(document["blocks"][1]["c"][1], "fn main() {}\n");
    }

    #[test]
    fn exports_standard_div_and_span_without_redundant_markers() {
        let document = export("`div{#box .note} Body\n`span[text]{.mark}\n").unwrap();
        let div_attrs = &document["blocks"][0]["c"][0];
        assert_eq!(div_attrs, &json!(["box", ["note"], []]));
        let span_attrs = &document["blocks"][1]["c"][0]["c"][0];
        assert_eq!(span_attrs, &json!(["", ["mark"], []]));
    }

    #[test]
    fn exports_symbolic_emphasis_and_preserves_attributes() {
        let document =
            export("Plain `*[emphasis with `**[strong]] and `**[attributed]{#id .keep}.\n")
                .unwrap();
        let inlines = document["blocks"][0]["c"].as_array().unwrap();

        let emphasis = inlines.iter().find(|inline| inline["t"] == "Emph").unwrap();
        assert!(emphasis["c"]
            .as_array()
            .unwrap()
            .iter()
            .any(|inline| inline["t"] == "Strong"));
        let wrapper = inlines.iter().find(|inline| inline["t"] == "Span").unwrap();
        assert_eq!(wrapper["t"], "Span");
        assert_eq!(wrapper["c"][0], json!(["id", ["keep"], []]));
        assert_eq!(wrapper["c"][1][0]["t"], "Strong");
    }

    #[test]
    fn exports_inline_and_display_math_with_attribute_wrappers() {
        let source = "Inline `[x^2]{.$}. Wrapped `[y]{.$ #inline .keep key=value}.\n\n`{.$}\n  E = mc^2\n`{.$ #display .numbered language=tex}\n  a = b\n";
        let document = export(source).unwrap();
        assert_eq!(document["blocks"][0]["c"][2]["t"], "Math");
        assert_eq!(document["blocks"][0]["c"][2]["c"][0]["t"], "InlineMath");
        let inline_wrapper = document["blocks"][0]["c"]
            .as_array()
            .unwrap()
            .iter()
            .find(|inline| inline["t"] == "Span")
            .unwrap();
        assert_eq!(inline_wrapper["t"], "Span");
        assert_eq!(
            inline_wrapper["c"][0],
            json!(["inline", ["keep"], [["key", "value"]]])
        );
        assert_eq!(inline_wrapper["c"][1][0]["t"], "Math");
        assert_eq!(document["blocks"][1]["t"], "Para");
        assert_eq!(document["blocks"][1]["c"][0]["c"][0]["t"], "DisplayMath");
        let display_wrapper = &document["blocks"][2];
        assert_eq!(display_wrapper["t"], "Div");
        assert_eq!(
            display_wrapper["c"][0],
            json!(["display", ["numbered"], []])
        );
        assert_eq!(
            display_wrapper["c"][1][0]["c"][0]["c"][0]["t"],
            "DisplayMath"
        );
    }

    #[test]
    fn exports_math_inside_rich_metadata_scalars() {
        let source = "`meta\n  `: formula\n\n    Area `[\\pi r^2]{.$}\n";
        let document = export(source).unwrap();
        assert_eq!(document["meta"]["formula"]["t"], "MetaInlines");
        let math = document["meta"]["formula"]["c"]
            .as_array()
            .unwrap()
            .iter()
            .find(|inline| inline["t"] == "Math")
            .unwrap();
        assert_eq!(math["c"][0]["t"], "InlineMath");
        assert_eq!(math["c"][1], "\\pi r^2");
    }

    #[test]
    fn lifts_typed_metadata_out_of_the_document_body() {
        let source = "`meta\n  `: title\n\n     Rich `*[title]\n\n  `: tags\n    `- plumb\n    `- tools\n\n  `: macros\n    `-\n      `- `[nearSet]\n      `- `[\\mathscr{C}]\n      `- 0\n\n  `: author\n    `: name\n\n       Alice\n\n  `: source\n    `{language=text}\n      raw\n\n  `: empty\n\n`# Section\n";
        let document = export(source).unwrap();

        assert_eq!(document["blocks"].as_array().unwrap().len(), 1);
        assert_eq!(document["blocks"][0]["t"], "Header");
        assert_eq!(document["meta"]["title"]["t"], "MetaInlines");
        assert_eq!(document["meta"]["tags"]["t"], "MetaList");
        assert_eq!(document["meta"]["tags"]["c"].as_array().unwrap().len(), 2);
        assert_eq!(document["meta"]["macros"]["t"], "MetaList");
        assert_eq!(document["meta"]["macros"]["c"][0]["t"], "MetaList");
        assert_eq!(document["meta"]["macros"]["c"][0]["c"][0]["c"], "nearSet");
        assert_eq!(
            document["meta"]["macros"]["c"][0]["c"][1]["c"],
            "\\mathscr{C}"
        );
        assert_eq!(document["meta"]["author"]["t"], "MetaMap");
        assert_eq!(
            document["meta"]["author"]["c"]["name"]["c"][0]["c"],
            "Alice"
        );
        assert_eq!(
            document["meta"]["source"],
            json!({
                "t": "MetaString",
                "c": "raw\n\n",
            })
        );
        assert_eq!(
            document["meta"]["empty"],
            json!({
                "t": "MetaString",
                "c": "",
            })
        );
    }

    #[test]
    fn metadata_export_keeps_first_duplicate_and_rejects_unsupported_values() {
        let duplicate =
            export("`meta\n  `: title\n\n     First\n\n  `: title\n\n     Second\n").unwrap();
        assert_eq!(duplicate["meta"]["title"]["c"][0]["c"], "First");

        let unsupported = export("`meta\n  `: mixed\n\n    paragraph\n    `- child\n");
        assert_eq!(
            unsupported.unwrap_err(),
            "metadata field 'mixed' has an unsupported value"
        );
    }

    #[test]
    fn exports_single_citations_in_body_and_metadata_without_a_pandoc_reader() {
        let document =
            export("`meta\n  `: source\n\n     `cite[roe2020]\n\nSee `cite[smith2004].\n").unwrap();

        assert_eq!(document["meta"]["source"]["c"][0]["t"], "Cite");
        let cite = &document["blocks"][0]["c"][2];
        assert_eq!(cite["t"], "Cite");
        assert_eq!(cite["c"][0].as_array().unwrap().len(), 1);
        assert_eq!(cite["c"][0][0]["citationId"], "smith2004");
        assert_eq!(cite["c"][0][0]["citationMode"]["t"], "NormalCitation");
        assert!(cite["c"][0][0]["citationPrefix"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(cite["c"][0][0]["citationSuffix"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(cite["c"][1][0]["c"], "[smith2004]");
    }
}
