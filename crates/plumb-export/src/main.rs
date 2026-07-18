use std::env;
use std::fs;
use std::io::{self, Read};
use std::process::ExitCode;

use plumb_core::{parse, AttrItem, Attributes, Block, Inline, InlineContent, ParsedBlock};
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
    Ok(json!({
        "pandoc-api-version": [1, 23, 1],
        "meta": {},
        "blocks": lower_blocks(&parsed.syntax.blocks),
    }))
}

fn lower_blocks(blocks: &[Block]) -> Vec<Value> {
    let mut output = Vec::new();
    for block in blocks {
        match block {
            Block::Code(code) => output.push(json!({
                "t": "CodeBlock",
                "c": [lower_attrs(&code.attrs, None), code.text],
            })),
            Block::Parsed(parsed) => lower_parsed_block(parsed, &mut output),
        }
    }
    output
}

fn lower_parsed_block(block: &ParsedBlock, output: &mut Vec<Value>) {
    let marker = block.mark.as_ref().and_then(|mark| mark.marker.as_deref());
    if let Some(level) = heading_level(block) {
        let attrs = &block.mark.as_ref().expect("heading has mark").attrs;
        output.push(json!({
            "t": "Header",
            "c": [level, lower_attrs(attrs, None), lower_inlines(&block.head)],
        }));
        output.extend(lower_blocks(&block.children));
        return;
    }

    if let Some(mark) = &block.mark {
        let mut contents = Vec::new();
        if !block.head.items.is_empty() {
            contents.push(json!({ "t": "Para", "c": lower_inlines(&block.head) }));
        }
        contents.extend(lower_blocks(&block.children));
        output.push(json!({
            "t": "Div",
            "c": [lower_attrs(&mark.attrs, marker), contents],
        }));
    } else {
        output.push(json!({ "t": "Para", "c": lower_inlines(&block.head) }));
    }
}

fn heading_level(block: &ParsedBlock) -> Option<u8> {
    let mark = block.mark.as_ref()?;
    let marker = mark.marker.as_deref()?;
    let hashes = marker.bytes().take_while(|byte| *byte == b'#').count();
    if hashes == marker.len() && (1..=6).contains(&hashes) {
        return Some(hashes as u8);
    }
    (marker == "heading")
        .then(|| mark.attrs.value("level")?.parse::<u8>().ok())
        .flatten()
        .filter(|level| (1..=6).contains(level))
}

fn lower_inlines(content: &InlineContent) -> Vec<Value> {
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
                kind,
                content,
                attrs,
                ..
            } => output.push(json!({
                "t": "Span",
                "c": [lower_attrs(attrs, kind.as_deref()), lower_inlines(content)],
            })),
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
            export("`heading{#intro level=1} Intro\nParagraph text.\n`note{.tip} Remember this.\n")
                .unwrap();
        let blocks = document["blocks"].as_array().unwrap();
        assert_eq!(blocks[0]["t"], "Header");
        assert_eq!(blocks[1]["t"], "Para");
        assert_eq!(blocks[2]["t"], "Div");
    }

    #[test]
    fn rejects_syntax_errors() {
        assert!(export("`node{key=a key=b} broken\n").is_err());
    }
}
