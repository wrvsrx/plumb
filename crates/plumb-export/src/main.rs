use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read};
use std::process::ExitCode;

use plumb_semantics::{
    analyze_document, is_document_declaration, CitationRecord, DocumentOutput, InlineStyleKind,
    LinkSpelling, ListGroup, ListKind, MetadataBlock, MetadataEntry, MetadataValue, TaskState,
};
use plumb_syntax::{parse, AttrItem, Attributes, Block, Inline, InlineContent, ParsedBlock};
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
        Some(path) => fs::read_to_string(path)
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

pub fn export(source: &str) -> Result<Value, String> {
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
        "blocks": lower_document_blocks(&parsed.syntax.blocks, &analysis),
    }))
}

fn lower_document_blocks(blocks: &[Block], analysis: &DocumentOutput) -> Vec<Value> {
    let body = blocks
        .iter()
        .filter(|block| !is_document_declaration(block))
        .collect::<Vec<_>>();
    lower_block_refs(&body, analysis)
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

fn lower_body(block: &ParsedBlock, analysis: &DocumentOutput) -> Vec<Value> {
    lower_block_refs(
        &plumb_semantics::body_children(block).collect::<Vec<_>>(),
        analysis,
    )
}

fn lower_block_refs(blocks: &[&Block], analysis: &DocumentOutput) -> Vec<Value> {
    let mut output = Vec::new();
    let mut index = 0;
    while index < blocks.len() {
        if let Block::Parsed(block) = blocks[index] {
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
        match blocks[index] {
            Block::Verbatim(block) => {
                output.push(json!({
                    "t": "CodeBlock",
                    "c": [lower_attrs(&Attributes::default(), None), block.text],
                }));
            }
            Block::Parsed(parsed) => lower_parsed_block(parsed, analysis, &mut output),
        }
        index += 1;
    }
    output
}

fn lower_definition_list(
    blocks: &[&Block],
    definitions: &plumb_semantics::DefinitionList,
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
            let mut body = lower_body(block, analysis);
            if let Some(inline_body) = &definition.inline_body {
                body.insert(
                    0,
                    json!({ "t": "Para", "c": lower_inlines(inline_body, analysis) }),
                );
            }
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

fn lower_list_group(blocks: &[&Block], group: &ListGroup, analysis: &DocumentOutput) -> Value {
    let items = blocks
        .iter()
        .map(|block| {
            let Block::Parsed(block) = block else {
                unreachable!("a list group contains only parsed item blocks")
            };
            let mark = block.mark.as_ref().expect("a list item has a mark");
            let mut contents = Vec::new();
            let task = analysis
                .tasks
                .tasks
                .iter()
                .find(|task| task.range.start == block.range.start);
            if let Some(task) = task {
                let mut title = vec![json!({ "t": "Str", "c": task_state_marker(task.state()) })];
                let inlines = lower_inlines(&block.head, analysis);
                if !inlines.is_empty() {
                    title.push(json!({ "t": "Space" }));
                }
                title.push(json!({
                    "t": "Span",
                    "c": [lower_attrs(&mark.attrs, None), inlines],
                }));
                contents.push(json!({ "t": "Para", "c": title }));
            } else if !block.head.items.is_empty() {
                contents.push(json!({ "t": "Para", "c": lower_inlines(&block.head, analysis) }));
            }
            contents.extend(lower_body(block, analysis));
            if task.is_some() || mark.attrs.items.is_empty() {
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

fn task_state_marker(state: TaskState) -> &'static str {
    match state {
        TaskState::Open => "☐",
        TaskState::Done => "☒",
        TaskState::Canceled => "⊘",
        TaskState::Conflicted => "⚠",
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
    if let Some(raw) = &block.raw {
        let mark = block.mark.as_ref().expect("a raw owner is marked");
        if analysis
            .math
            .math_at_node_start(block.range.start)
            .is_some()
        {
            let math = json!({
                "t": "Math",
                "c": [{ "t": "DisplayMath" }, raw.text],
            });
            let paragraph = json!({ "t": "Para", "c": [math] });
            if has_unconsumed_math_attrs(&mark.attrs) {
                output.push(json!({
                    "t": "Div",
                    "c": [lower_math_attrs(&mark.attrs), [paragraph]],
                }));
            } else {
                output.push(paragraph);
            }
        } else {
            output.push(json!({
                "t": "CodeBlock",
                "c": [lower_attrs(&mark.attrs, (mark.marker != "()").then_some(mark.marker.as_str())), raw.text],
            }));
        }
        return;
    }
    if let Some(heading) = analysis.headings.heading_at_node_start(block.range.start) {
        let attrs = &block.mark.as_ref().expect("heading has mark").attrs;
        output.push(json!({
            "t": "Header",
            "c": [heading.level, lower_attrs(attrs, None), lower_inlines(&block.head, analysis)],
        }));
        output.extend(lower_body(block, analysis));
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
        contents.extend(lower_body(block, analysis));
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
        contents.extend(lower_body(block, analysis));
        output.push(json!({
            "t": "Div",
            "c": [lower_attrs(&mark.attrs, (mark.marker != "()").then_some(mark.marker.as_str())), contents],
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
            Inline::Space { text, .. } => lower_text(text, &mut output),
            Inline::SoftBreak { .. } => output.push(json!({ "t": "SoftBreak" })),
            Inline::Verbatim {
                range,
                kind,
                text,
                attrs,
                ..
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
                        "c": [lower_attrs(attrs, (!kind.is_empty()).then_some(kind.as_str())), text],
                    }));
                }
            }
            Inline::Element {
                range,
                kind,
                members,
                attrs,
                ..
            } => {
                if let Some(style) = analysis.inline_styles.style_at_node_start(range.start) {
                    let content = lower_members(members, analysis);
                    if style.kind == InlineStyleKind::Mark {
                        output.push(json!({
                            "t": "Span",
                            "c": [lower_mark_attrs(attrs), content],
                        }));
                    } else {
                        let semantic = json!({
                            "t": match style.kind {
                                InlineStyleKind::Emphasis => "Emph",
                                InlineStyleKind::Strong => "Strong",
                                InlineStyleKind::Strikeout => "Strikeout",
                                InlineStyleKind::Superscript => "Superscript",
                                InlineStyleKind::Subscript => "Subscript",
                                InlineStyleKind::Mark => unreachable!(),
                            },
                            "c": content,
                        });
                        if attrs.items.is_empty() {
                            output.push(semantic);
                        } else {
                            output.push(json!({
                                "t": "Span",
                                "c": [lower_attrs(attrs, None), [semantic]],
                            }));
                        }
                    }
                } else if let Some(citation) =
                    analysis.citations.citation_at_node_start(range.start)
                {
                    output.push(lower_citation(citation));
                } else if let Some(image) = analysis.image_at_node_start(range.start) {
                    output.push(json!({
                        "t": "Image",
                        "c": [lower_image_attrs(attrs), lower_first_argument(members, analysis), [&image.source.value, ""]],
                    }));
                } else if let Some(file) = analysis.file_at_node_start(range.start) {
                    output.push(json!({
                        "t": "Link",
                        "c": [lower_file_attrs(attrs), lower_first_argument(members, analysis), [&file.source.value, ""]],
                    }));
                } else if let Some(link) = analysis.link_at_node_start(range.start) {
                    let label = if matches!(link.spelling, LinkSpelling::Verbatim { .. }) {
                        text_inlines(&link.target.value)
                    } else {
                        lower_link_label(members, analysis)
                    };
                    output.push(json!({
                        "t": "Link",
                        "c": [lower_link_attrs(attrs), label, [&link.target.value, ""]],
                    }));
                } else {
                    output.push(json!({
                        "t": "Span",
                        "c": [lower_attrs(attrs, (kind != "()").then_some(kind)), lower_members(members, analysis)],
                    }));
                }
            }
        }
    }
    output
}

fn lower_first_argument(
    members: &[plumb_syntax::InlineMember],
    analysis: &DocumentOutput,
) -> Vec<Value> {
    members
        .iter()
        .find_map(plumb_syntax::InlineMember::argument)
        .map_or_else(Vec::new, |argument| lower_argument(argument, analysis))
}

fn lower_members(members: &[plumb_syntax::InlineMember], analysis: &DocumentOutput) -> Vec<Value> {
    let mut output = Vec::new();
    for member in members {
        match member {
            plumb_syntax::InlineMember::ParsedArgument(argument) => {
                output.extend(lower_inlines(&argument.content, analysis));
            }
            plumb_syntax::InlineMember::VerbatimArgument(argument) => output.push(json!({
                "t": "Code",
                "c": [["", [], []], argument.text],
            })),
            plumb_syntax::InlineMember::Child { inline, .. } if !is_relation_child(inline) => {
                output.extend(lower_inline_child(inline, analysis));
            }
            plumb_syntax::InlineMember::Child { .. } => {}
        }
    }
    output
}

fn lower_link_label(
    members: &[plumb_syntax::InlineMember],
    analysis: &DocumentOutput,
) -> Vec<Value> {
    let mut output = Vec::new();
    let mut argument_index = 0;
    for member in members {
        match member {
            plumb_syntax::InlineMember::ParsedArgument(argument) => {
                if argument_index == 0 {
                    output.extend(lower_inlines(&argument.content, analysis));
                }
                argument_index += 1;
            }
            plumb_syntax::InlineMember::VerbatimArgument(argument) => {
                if argument_index == 0 {
                    output.push(json!({ "t": "Code", "c": [["", [], []], argument.text] }));
                }
                argument_index += 1;
            }
            plumb_syntax::InlineMember::Child { inline, .. } if !is_relation_child(inline) => {
                output.extend(lower_inline_child(inline, analysis));
            }
            plumb_syntax::InlineMember::Child { .. } => {}
        }
    }
    output
}

fn lower_argument(
    argument: plumb_syntax::InlineArgumentRef<'_>,
    analysis: &DocumentOutput,
) -> Vec<Value> {
    match argument {
        plumb_syntax::InlineArgumentRef::Parsed(content) => lower_inlines(content, analysis),
        plumb_syntax::InlineArgumentRef::Verbatim(argument) => {
            vec![json!({ "t": "Code", "c": [["", [], []], argument.text] })]
        }
    }
}

fn lower_inline_child(inline: &Inline, analysis: &DocumentOutput) -> Vec<Value> {
    lower_inlines(
        &InlineContent {
            range: inline_range(inline).clone(),
            items: vec![inline.clone()],
        },
        analysis,
    )
}

fn is_relation_child(inline: &Inline) -> bool {
    matches!(inline, Inline::Element { kind, .. } if matches!(kind.as_str(), "@" | "+" | "="))
}

fn inline_range(inline: &Inline) -> &std::ops::Range<usize> {
    match inline {
        Inline::Text { range, .. }
        | Inline::Space { range, .. }
        | Inline::SoftBreak { range }
        | Inline::Element { range, .. }
        | Inline::Verbatim { range, .. } => range,
    }
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

fn lower_file_attrs(attrs: &Attributes) -> Value {
    lower_attrs_filtered(attrs, Some("file"), |_| false, |key| key == "src")
}

fn lower_link_attrs(attrs: &Attributes) -> Value {
    lower_attrs_filtered(attrs, None, |_| false, |key| key == "to")
}

fn lower_mark_attrs(attrs: &Attributes) -> Value {
    let mut attrs = lower_attrs(attrs, None);
    attrs[1]
        .as_array_mut()
        .expect("Pandoc attributes contain a class array")
        .insert(0, json!("mark"));
    attrs
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
            export("`# Intro\n  `@ intro\n\nParagraph text.\n\n`note Remember this.\n  `+ tip\n")
                .unwrap();
        let blocks = document["blocks"].as_array().unwrap();
        assert_eq!(blocks[0]["t"], "Header");
        assert_eq!(blocks[1]["t"], "Para");
        assert_eq!(blocks[2]["t"], "Div");
    }

    #[test]
    fn exports_adjacent_and_nested_items_as_bullet_lists() {
        let source =
            "`- One\n\n`task Two\n  `@ two\n  `= priority -5\n\n  `- Nested\n\nParagraph.\n";
        let document = export(source).unwrap();
        let blocks = document["blocks"].as_array().unwrap();

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["t"], "BulletList");
        let items = blocks[0]["c"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0][0]["t"], "Para");
        assert_eq!(items[0][0]["c"][0]["c"], "One");

        let attributed = &items[1][0];
        assert_eq!(attributed["t"], "Para");
        assert_eq!(attributed["c"][0]["c"], "☐");
        assert_eq!(attributed["c"][2]["t"], "Span");
        assert_eq!(attributed["c"][2]["c"][0][0], "two");
        assert_eq!(attributed["c"][2]["c"][0][1], json!([]));
        assert_eq!(attributed["c"][2]["c"][0][2], json!([["priority", "-5"]]));
        assert_eq!(items[1][1]["t"], "BulletList");
        assert_eq!(items[1][1]["c"][0][0]["c"][0]["c"], "Nested");
        assert_eq!(blocks[1]["t"], "Para");
    }

    #[test]
    fn document_declarations_do_not_split_exported_body_lists() {
        let document =
            export("`- First\n`= title Between\n`+ journal\n`@ unsupported\n`- Second\n").unwrap();
        let blocks = document["blocks"].as_array().unwrap();

        assert_eq!(document["meta"]["title"]["c"][0]["c"], "Between");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["t"], "BulletList");
        let items = blocks[0]["c"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0][0]["c"][0]["c"], "First");
        assert_eq!(items[1][0]["c"][0]["c"], "Second");
    }

    #[test]
    fn exports_visible_task_state_markers() {
        let source = "`task Open\n`task Done\n  `= done 2026-07-25T15:00:00+08:00\n`task Canceled\n  `= canceled 2026-07-25T15:00:00+08:00\n`task Conflicted\n  `= done 2026-07-25T15:00:00+08:00\n  `= canceled 2026-07-25T15:01:00+08:00\n";
        let document = export(source).unwrap();
        let items = document["blocks"][0]["c"].as_array().unwrap();
        let markers = items
            .iter()
            .map(|item| item[0]["c"][0]["c"].as_str().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(markers, vec!["☐", "☒", "⊘", "⚠"]);
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
        let source = "`> Quoted head\n\n   Quoted body.\n\n   `> Nested quote\n     `@ nested\n     `+ source\n     `= cite book\n\n`quote Generic\n";
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
        let source = "`: Term\n\n   Definition.\n\n`: Tagged\n  `@ tag\n  `+ kind\n  `= key value\n\n  `- First\n  `- Second\n";
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
        assert!(export("`broken[\n").is_err());
    }

    #[test]
    fn exports_links_from_shared_document_facts() {
        let document = export("See `->[target|other.plumb#id].\n").unwrap();
        assert_eq!(document["blocks"][0]["c"][2]["t"], "Link");
        assert_eq!(document["blocks"][0]["c"][2]["c"][2][0], "other.plumb#id");
    }

    #[test]
    fn exports_verbatim_autolinks_in_body_and_metadata() {
        let source = "`= homepage `->\"https://example.test/meta\"\n\nBody `->\"https://example.test/a%20b\".\n";
        let document = export(source).unwrap();
        let metadata_link = &document["meta"]["homepage"]["c"][0];
        assert_eq!(metadata_link["t"], "Link");
        assert_eq!(metadata_link["c"][2][0], "https://example.test/meta");

        let body_link = &document["blocks"][0]["c"][2];
        assert_eq!(body_link["t"], "Link");
        assert_eq!(body_link["c"][0], json!(["", [], []]));
        assert_eq!(body_link["c"][1][0]["c"], "https://example.test/a%20b");
        assert_eq!(body_link["c"][2][0], "https://example.test/a%20b");
    }

    #[test]
    fn exports_standard_images_in_body_and_metadata() {
        let source = "`= cover `img[Cover|=[src|static/cover.png]]\n\nBefore `img[Rich `em[alt]|=[src|\"[static/a b.webp]\"]|@[image]|+[wide]|=[loading|lazy]] after.\n\n`img[|=[src|https://example.test/decorative.svg]]\n";
        let document = export(source).unwrap();

        let metadata_image = &document["meta"]["cover"]["c"][0];
        assert_eq!(metadata_image["t"], "Image");
        assert_eq!(
            metadata_image["c"][1],
            json!([{ "t": "Str", "c": "Cover" }])
        );
        assert_eq!(metadata_image["c"][2][0], "static/cover.png");
        assert_eq!(metadata_image["c"][2][1], "");

        assert_eq!(document["blocks"].as_array().unwrap().len(), 2);
        let body_image = &document["blocks"][0]["c"][2];
        assert_eq!(body_image["t"], "Image");
        assert_eq!(
            body_image["c"][0],
            json!(["image", ["wide"], [["loading", "lazy"]]])
        );
        assert_eq!(body_image["c"][1][0]["c"], "Rich");
        assert_eq!(body_image["c"][1][2]["t"], "Span");
        assert_eq!(body_image["c"][2], json!(["static/a b.webp", ""]));

        let image_only_paragraph = &document["blocks"][1];
        assert_eq!(image_only_paragraph["t"], "Para");
        assert_eq!(image_only_paragraph["c"].as_array().unwrap().len(), 1);
        let decorative = &image_only_paragraph["c"][0];
        assert_eq!(decorative["t"], "Image");
        assert_eq!(decorative["c"][0], json!(["", [], []]));
        assert_eq!(decorative["c"][1], json!([]));
        assert_eq!(
            decorative["c"][2],
            json!(["https://example.test/decorative.svg", ""])
        );
    }

    #[test]
    fn exports_file_attachments_as_portable_links_with_fallback_content() {
        let document = export(
            "Watch `file[Demo `![video]|=[src|\"[static/demo video.mp4]\"]|@[demo]|+[wide]|=[download|yes]].\n",
        )
        .unwrap();
        let file = &document["blocks"][0]["c"][2];
        assert_eq!(file["t"], "Link");
        assert_eq!(
            file["c"][0],
            json!([
                "demo",
                ["wide"],
                [["download", "yes"], ["data-plumb-marker", "file"]]
            ])
        );
        assert_eq!(file["c"][1][0]["c"], "Demo");
        assert_eq!(file["c"][1][2]["t"], "Strong");
        assert_eq!(file["c"][2], json!(["static/demo video.mp4", ""]));
    }

    #[test]
    fn exports_link_kind_as_a_generic_span() {
        let document = export("`link[target|=[to|other.plumb#id]]\n").unwrap();
        let inline = &document["blocks"][0]["c"][0];

        assert_eq!(inline["t"], "Span");
        assert_eq!(
            inline["c"][0][2],
            json!([["to", "other.plumb#id"], ["data-plumb-marker", "link"]])
        );
    }

    #[test]
    fn exports_verbatim_envelopes_as_pandoc_code() {
        let document = export("Use `\"cargo check\".\n\n`rust\n\n|\"\n fn main() {}\n").unwrap();
        assert_eq!(document["blocks"][0]["c"][2]["t"], "Code");
        assert_eq!(document["blocks"][0]["c"][2]["c"][1], "cargo check");
        assert_eq!(document["blocks"][1]["t"], "CodeBlock");
        assert_eq!(document["blocks"][1]["c"][1], "fn main() {}\n");
    }

    #[test]
    fn exports_paren_transparent_containers_without_redundant_markers() {
        let document = export("`() Body\n  `@ box\n  `+ note\n\n`()[text|+[mark]]\n").unwrap();
        let div_attrs = &document["blocks"][0]["c"][0];
        assert_eq!(div_attrs, &json!(["box", ["note"], []]));
        let span_attrs = &document["blocks"][1]["c"][0]["c"][0];
        assert_eq!(span_attrs, &json!(["", ["mark"], []]));

        // The explicit container stays a Div even without declarations;
        // paragraph and container are distinct categories.
        let explicit = export("`() Plain prose.\n").unwrap();
        assert_eq!(explicit["blocks"][0]["t"], "Div");
    }

    #[test]
    fn exports_symbolic_inline_styles_and_preserves_attributes() {
        let document = export(
            "`*[em `![strong]] `==[mark|@[marked]|+[keep]] `~[strike] `^[super] `_[sub] `**[generic]\n",
        )
        .unwrap();
        let inlines = document["blocks"][0]["c"].as_array().unwrap();

        let emphasis = inlines.iter().find(|inline| inline["t"] == "Emph").unwrap();
        assert!(emphasis["c"]
            .as_array()
            .unwrap()
            .iter()
            .any(|inline| inline["t"] == "Strong"));
        let mark = inlines
            .iter()
            .find(|inline| inline["t"] == "Span" && inline["c"][0][0] == "marked")
            .unwrap();
        assert_eq!(mark["c"][0], json!(["marked", ["mark", "keep"], []]));
        for kind in ["Strikeout", "Superscript", "Subscript"] {
            assert!(inlines.iter().any(|inline| inline["t"] == kind));
        }
        let generic = inlines
            .iter()
            .find(|inline| {
                inline["t"] == "Span" && inline["c"][0][2] == json!([["data-plumb-marker", "**"]])
            })
            .unwrap();
        assert_eq!(generic["c"][1][0]["c"], "generic");
    }

    #[test]
    fn exports_inline_and_display_math_with_attribute_wrappers() {
        let source = "Inline `$\"x^2\".\n\n`$\n\n|\"\n E = mc^2\n`$\n `@ display\n `+ numbered\n\n|\"\n a = b\n";
        let document = export(source).unwrap();
        assert_eq!(document["blocks"][0]["c"][2]["t"], "Math");
        assert_eq!(document["blocks"][0]["c"][2]["c"][0]["t"], "InlineMath");
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
        let source = "`= formula Area `$\"\\pi r^2\"\n";
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
        let source = "`= title Rich `*[title]\n`= tags\n\n `- plumb\n `- tools\n\n`= macros\n\n `-\n  `- `\"nearSet\"\n  `- `\"\\mathscr{C}\"\n  `- 0\n\n`= author\n\n `= name Alice\n\n`= source\n\n `\"\n  raw\n  \n  \n`= empty\n\n`# Section\n";
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
                "c": "raw\n\n\n",
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
        let duplicate = export("`= title First\n`= title Second\n").unwrap();
        assert_eq!(duplicate["meta"]["title"]["c"][0]["c"], "First");

        let unsupported = export("`= mixed\n\n paragraph\n `- child\n");
        assert_eq!(
            unsupported.unwrap_err(),
            "metadata field 'mixed' has an unsupported value"
        );
    }

    #[test]
    fn exports_single_citations_in_body_and_metadata_without_a_pandoc_reader() {
        let document = export("`= source `cite[roe2020]\n\nSee `cite[smith2004].\n").unwrap();

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
