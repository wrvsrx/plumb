use std::env;
use std::fs;
use std::io::{self, Read};
use std::process::ExitCode;

use plumb_core::{parse, AttrItem, Attributes, Block, Inline, InlineContent, ParsedBlock};
use plumb_extensions::{
    analyze_document, DocumentOutput, MetadataBlock, MetadataEntry, MetadataValue,
};
use serde_json::{json, Map, Value};

fn main() -> ExitCode {
    let input = match read_input() {
        Ok(input) => input,
        Err(error) => {
            eprintln!("plumb-export: {error}");
            return ExitCode::FAILURE;
        }
    };
    match export(&input) {
        Ok(document) => {
            println!("{document}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("plumb-export: {error}");
            ExitCode::FAILURE
        }
    }
}

fn read_input() -> Result<String, String> {
    match env::args_os().nth(1) {
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
                .map(|item| json!({
                    "t": "MetaInlines",
                    "c": lower_inlines(&item.content, analysis),
                }))
                .collect::<Vec<_>>(),
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
    for block in blocks {
        match block {
            Block::Verbatim(block) => {
                output.push(json!({
                    "t": "CodeBlock",
                    "c": [lower_attrs(&block.attrs, None), block.text],
                }));
            }
            Block::Parsed(parsed) => lower_parsed_block(parsed, analysis, &mut output),
        }
    }
    output
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
    let marker = block.mark.as_ref().map(|mark| mark.marker.as_str());
    if let Some(heading) = analysis.headings.heading_at_node_start(block.range.start) {
        let attrs = &block.mark.as_ref().expect("heading has mark").attrs;
        output.push(json!({
            "t": "Header",
            "c": [heading.level, lower_attrs(attrs, None), lower_inlines(&block.head, analysis)],
        }));
        output.extend(lower_blocks(&block.children, analysis));
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
            "c": [lower_attrs(&mark.attrs, marker), contents],
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
            Inline::Verbatim { text, attrs, .. } => output.push(json!({
                "t": "Code",
                "c": [lower_attrs(attrs, None), text],
            })),
            Inline::Element {
                range,
                kind,
                content,
                attrs,
                ..
            } => {
                if let Some(link) = analysis.link_at_node_start(range.start) {
                    output.push(json!({
                        "t": "Link",
                        "c": [lower_attrs(attrs, None), lower_inlines(content, analysis), [&link.target.value, ""]],
                    }));
                } else {
                    output.push(json!({
                        "t": "Span",
                        "c": [lower_attrs(attrs, Some(kind)), lower_inlines(content, analysis)],
                    }));
                }
            }
        }
    }
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
    let mut id = String::new();
    let mut classes = Vec::new();
    let mut pairs = Vec::new();
    for item in &attrs.items {
        match item {
            AttrItem::Id { value, .. } => id = value.clone(),
            AttrItem::Class { value, .. } => classes.push(value.clone()),
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
    fn rejects_syntax_errors() {
        assert!(export("`node{key=a key=b} broken\n").is_err());
    }

    #[test]
    fn exports_links_from_shared_document_facts() {
        let document = export("See `link[target]{to=\"other.plumb#id\"}.\n").unwrap();
        assert_eq!(document["blocks"][0]["c"][2]["t"], "Link");
        assert_eq!(document["blocks"][0]["c"][2]["c"][2][0], "other.plumb#id");
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
    fn lifts_typed_metadata_out_of_the_document_body() {
        let source = "`meta\n  `: title\n\n     Rich `*[title]\n\n  `: tags\n    `item plumb\n    `item tools\n\n  `: author\n    `: name\n\n       Alice\n\n  `: source\n    `{language=text}\n      raw\n\n  `: empty\n\n`# Section\n";
        let document = export(source).unwrap();

        assert_eq!(document["blocks"].as_array().unwrap().len(), 1);
        assert_eq!(document["blocks"][0]["t"], "Header");
        assert_eq!(document["meta"]["title"]["t"], "MetaInlines");
        assert_eq!(document["meta"]["tags"]["t"], "MetaList");
        assert_eq!(document["meta"]["tags"]["c"].as_array().unwrap().len(), 2);
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

        let unsupported = export("`meta\n  `: mixed\n\n    paragraph\n    `item child\n");
        assert_eq!(
            unsupported.unwrap_err(),
            "metadata field 'mixed' has an unsupported value"
        );
    }
}
