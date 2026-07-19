use std::env;
use std::fs;
use std::io::{self, Read};
use std::process::ExitCode;

use plumb_core::{parse, AttrItem, Attributes, Block, Inline, InlineContent, ParsedBlock};
use plumb_extensions::{analyze_document, DocumentOutput};
use serde_json::{json, Value};

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
    Ok(json!({
        "pandoc-api-version": [1, 23, 1],
        "meta": {},
        "blocks": lower_blocks(&parsed.syntax.blocks, &analysis),
    }))
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
}
