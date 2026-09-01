use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read};
use std::process::ExitCode;

use pandoc_types::definition::{
    Alignment, Attr, Block, Caption, Cell, CitationMode, ColWidth, Inline, ListNumberDelim,
    ListNumberStyle, MathType, MetaValue, Pandoc, Row, Table,
};

pub fn run_cli(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let mut args = args.into_iter().skip(1);
    let path = args.next();
    if args.next().is_some() {
        eprintln!("plumb import: expected at most one input path");
        return ExitCode::from(2);
    }
    let input = match read_input(path.as_deref()) {
        Ok(input) => input,
        Err(error) => {
            eprintln!("plumb import: {error}");
            return ExitCode::FAILURE;
        }
    };
    match import_json(&input) {
        Ok(document) => {
            print!("{document}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("plumb import: {error}");
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

pub fn import_json(source: &str) -> Result<String, String> {
    let document: Pandoc =
        serde_json::from_str(source).map_err(|error| format!("invalid Pandoc JSON: {error}"))?;
    import(&document)
}

pub fn import(document: &Pandoc) -> Result<String, String> {
    let mut blocks = Vec::new();
    if !document.meta.is_empty() {
        blocks.push(render_metadata(&document.meta)?);
    }
    for (index, block) in document.blocks.iter().enumerate() {
        if let Some(block) =
            render_block(block).map_err(|error| format!("blocks[{index}]: {error}"))?
        {
            blocks.push(block);
        }
    }
    let source = blocks.join("\n\n");
    let parsed = plumb_syntax::parse(&source);
    if !parsed.is_valid() {
        let diagnostics = parsed
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
            .join("; ");
        return Err(format!("generated plumb is invalid: {diagnostics}"));
    }
    let edits = plumb_edit::format(&parsed, plumb_edit::FormatScope::Document)
        .map_err(|_| "generated plumb failed strict validation".to_string())?;
    let formatted = plumb_edit::apply_text_edits(source, edits)
        .map_err(|_| "generated plumb failed strict validation".to_string())?;
    let parsed = plumb_syntax::parse(&formatted);
    if !parsed.is_valid() {
        return Err("generated plumb failed strict validation".to_string());
    }
    Ok(formatted)
}

fn render_metadata(meta: &HashMap<String, MetaValue>) -> Result<String, String> {
    let mut keys = meta.keys().collect::<Vec<_>>();
    keys.sort();
    let mut entries = Vec::new();
    for key in keys {
        require_attr_name(key, "metadata key")?;
        entries.push(render_metadata_entry(key, &meta[key])?);
    }
    Ok(entries.join("\n"))
}

fn render_metadata_entry(key: &str, value: &MetaValue) -> Result<String, String> {
    let value_source = render_metadata_value(value)?;
    if value_source.is_empty() {
        return Ok(format!("`= {key}"));
    }
    if matches!(value, MetaValue::MetaString(value) if !value.contains(['\n', '\r']))
        || matches!(value, MetaValue::MetaInlines(_))
    {
        return Ok(format!("`= {key}|{value_source}"));
    }
    Ok(format!("`= {key}\n{}", indent(&value_source, 2)))
}

fn render_metadata_value(value: &MetaValue) -> Result<String, String> {
    match value {
        MetaValue::MetaString(value) if value.contains(['\n', '\r']) => {
            render_verbatim_block(&Attr::default(), value)
        }
        MetaValue::MetaString(value) => render_verbatim(value, &Attr::default()),
        MetaValue::MetaInlines(inlines) => render_inlines(inlines, false),
        MetaValue::MetaList(values) => {
            let mut items = Vec::new();
            for value in values {
                let value = render_metadata_value(value)?;
                if value.contains('\n')
                    || matches!(value.lines().next(), Some(line) if line.starts_with('`'))
                {
                    items.push(format!("`+\n{}", indent(&value, 2)));
                } else {
                    items.push(format!("`+ {value}"));
                }
            }
            Ok(items.join("\n"))
        }
        MetaValue::MetaMap(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort();
            let mut entries = Vec::new();
            for key in keys {
                require_attr_name(key, "metadata key")?;
                entries.push(render_metadata_entry(key, &values[key])?);
            }
            Ok(entries.join("\n"))
        }
        MetaValue::MetaBlocks(blocks) if blocks.len() == 1 => render_block(&blocks[0])?
            .ok_or_else(|| "metadata MetaBlocks cannot contain Null".to_string()),
        MetaValue::MetaBlocks(_) => {
            Err("metadata MetaBlocks requires exactly one representable block".into())
        }
        MetaValue::MetaBool(_) => {
            Err("metadata MetaBool has no standard plumb representation".into())
        }
    }
}

fn render_block(block: &Block) -> Result<Option<String>, String> {
    match block {
        Block::Plain(inlines) | Block::Para(inlines) => Ok(Some(render_inlines(inlines, false)?)),
        Block::Header(level, attrs, inlines) if (1..=6).contains(level) => {
            let mut output = format!(
                "`{} {}",
                "#".repeat(*level as usize),
                render_inlines(inlines, false)?
            );
            append_block_attrs(&mut output, &render_attrs(attrs, None)?);
            Ok(Some(output))
        }
        Block::Header(level, ..) => Err(format!("heading level {level} is outside 1..=6")),
        Block::CodeBlock(attrs, text) => Ok(Some(render_verbatim_block(attrs, text)?)),
        Block::BlockQuote(blocks) => {
            let (attrs, blocks) = unwrap_attributed_blocks(blocks);
            Ok(Some(render_container(">", attrs, blocks)?))
        }
        Block::BulletList(items) => Ok(Some(render_list("-", items)?)),
        Block::OrderedList(attributes, items) => {
            if attributes.start_number != 1
                || !matches!(
                    attributes.style,
                    ListNumberStyle::Decimal | ListNumberStyle::DefaultStyle
                )
                || !matches!(
                    attributes.delim,
                    ListNumberDelim::Period | ListNumberDelim::DefaultDelim
                )
            {
                return Err(
                    "ordered-list style is not representable by the standard extension".into(),
                );
            }
            Ok(Some(render_list(".", items)?))
        }
        Block::DefinitionList(entries) => Ok(Some(render_definitions(entries)?)),
        Block::Div(attrs, blocks) => {
            let marker = attr_pair(attrs, "data-plumb-marker").unwrap_or("()");
            require_marker(marker)?;
            Ok(Some(render_container(marker, attrs, blocks)?))
        }
        Block::Null => Ok(None),
        Block::LineBlock(_) => Err("LineBlock has no standard plumb representation".into()),
        Block::RawBlock(_, _) => Err("RawBlock has no standard plumb representation".into()),
        Block::HorizontalRule => Err("HorizontalRule has no standard plumb representation".into()),
        Block::Table(table) => Ok(Some(render_table(table)?)),
        Block::Figure(_, _, _) => Err("Figure has no standard plumb representation".into()),
    }
}

fn render_table(table: &Table) -> Result<String, String> {
    if table.colspecs.iter().any(|spec| {
        !matches!(spec.0, Alignment::AlignDefault) || !matches!(spec.1, ColWidth::ColWidthDefault)
    }) {
        return Err("table alignment and column widths are not representable".into());
    }
    if !table.foot.rows.is_empty() {
        return Err("table foot rows are not representable".into());
    }
    if table.bodies.len() > 1 || table.bodies.iter().any(|body| !body.head.is_empty()) {
        return Err("complex table body grouping is not representable".into());
    }

    let column_count = table.colspecs.len();
    let mut rendered_rows = Vec::new();
    for row in &table.head.rows {
        rendered_rows.push(render_table_row(row, true, 0, column_count)?);
    }
    if let Some(body) = table.bodies.first() {
        let row_head_columns = usize::try_from(body.row_head_columns)
            .map_err(|_| "negative table row-head column count is invalid")?;
        if row_head_columns > column_count {
            return Err("table row-head column count exceeds the column count".into());
        }
        for row in &body.body {
            rendered_rows.push(render_table_row(
                row,
                false,
                row_head_columns,
                column_count,
            )?);
        }
    }

    let caption = render_table_caption(&table.caption)?;
    let mut output = if caption.is_empty() {
        "`table".to_string()
    } else {
        format!("`table {caption}")
    };
    append_block_attrs(&mut output, &render_attrs(&table.attr, None)?);
    if !rendered_rows.is_empty() {
        output.push('\n');
        output.push_str(&indent(&rendered_rows.join("\n"), 1));
    }
    Ok(output)
}

fn render_table_caption(caption: &Caption) -> Result<String, String> {
    if let Some(short) = &caption.short {
        if caption.long.is_empty() {
            return render_inlines(short, false);
        }
    }
    match caption.long.as_slice() {
        [] => Ok(String::new()),
        [Block::Plain(inlines) | Block::Para(inlines)] => render_inlines(inlines, false),
        _ => Err("table caption must be representable as one inline paragraph".into()),
    }
}

fn render_table_row(
    row: &Row,
    header: bool,
    row_head_columns: usize,
    column_count: usize,
) -> Result<String, String> {
    if row.cells.len() != column_count {
        return Err(format!(
            "table row has {} cells; expected {column_count}",
            row.cells.len()
        ));
    }
    for cell in &row.cells {
        validate_table_cell(cell)?;
    }

    let compact = row_head_columns == 0 && row.cells.iter().all(compact_table_cell);
    let mut output = if compact {
        let cells = row
            .cells
            .iter()
            .map(render_compact_table_cell)
            .collect::<Result<Vec<_>, _>>()?;
        format!("`- {}", cells.join("|"))
    } else {
        let cells = row
            .cells
            .iter()
            .enumerate()
            .map(|(index, cell)| render_expanded_table_cell(cell, index < row_head_columns))
            .collect::<Result<Vec<_>, _>>()?;
        format!("`-\n{}", indent(&cells.join("\n"), 1))
    };

    let mut attrs = render_attrs(&row.attr, None)?;
    if header {
        if !attrs.is_empty() {
            attrs.push('\n');
        }
        attrs.push_str("`+ header");
    }
    append_block_attrs(&mut output, &attrs);
    Ok(output)
}

fn validate_table_cell(cell: &Cell) -> Result<(), String> {
    if !matches!(cell.align, Alignment::AlignDefault) {
        return Err("table cell alignment is not representable".into());
    }
    if cell.row_span != 1 || cell.col_span != 1 {
        return Err("table row and column spans are not representable".into());
    }
    Ok(())
}

fn compact_table_cell(cell: &Cell) -> bool {
    cell.attr == Attr::default()
        && matches!(
            cell.content.as_slice(),
            [] | [Block::Plain(_) | Block::Para(_)]
        )
        && !cell.content.iter().any(|block| match block {
            Block::Plain(inlines) | Block::Para(inlines) => inlines
                .iter()
                .any(|inline| matches!(inline, Inline::SoftBreak | Inline::LineBreak)),
            _ => false,
        })
}

fn render_compact_table_cell(cell: &Cell) -> Result<String, String> {
    match cell.content.as_slice() {
        [] => Ok(String::new()),
        [Block::Plain(inlines) | Block::Para(inlines)] => render_inlines(inlines, true),
        _ => unreachable!("compact cell shape was checked"),
    }
}

fn render_expanded_table_cell(cell: &Cell, header: bool) -> Result<String, String> {
    let (head, children) = match split_head(&cell.content)? {
        Some((head, children)) => (Some(head), children),
        None => (None, cell.content.as_slice()),
    };
    let mut output = head.map_or_else(|| "`-".to_string(), |head| format!("`- {head}"));
    let mut attrs = render_attrs(&cell.attr, None)?;
    if header {
        if !attrs.is_empty() {
            attrs.push('\n');
        }
        attrs.push_str("`+ header");
    }
    append_block_attrs(&mut output, &attrs);
    if !children.is_empty() {
        output.push_str("\n\n");
        output.push_str(&indent(&render_blocks(children)?, 1));
    }
    Ok(output)
}

fn render_list(marker: &str, items: &[Vec<Block>]) -> Result<String, String> {
    let mut output = Vec::new();
    for item in items {
        let (attrs, blocks) = unwrap_attributed_blocks(item);
        let attrs = render_attrs(attrs, None)?;
        if let Some((head, children)) = split_head(blocks)? {
            let mut rendered = format!("`{marker} {head}");
            append_block_attrs(&mut rendered, &attrs);
            if !children.is_empty() {
                rendered.push('\n');
                rendered.push_str(&indent(&render_blocks(children)?, 2));
            }
            output.push(rendered);
        } else {
            let children = render_blocks(blocks)?;
            let mut rendered = format!("`{marker}");
            append_block_attrs(&mut rendered, &attrs);
            if !children.is_empty() {
                rendered.push('\n');
                rendered.push_str(&indent(&children, 2));
            }
            output.push(rendered);
        }
    }
    Ok(output.join("\n"))
}

fn render_definitions(entries: &[(Vec<Inline>, Vec<Vec<Block>>)]) -> Result<String, String> {
    let mut output = Vec::new();
    for (term, definitions) in entries {
        if definitions.len() != 1 {
            return Err(
                "definition entries with multiple definitions are not representable".into(),
            );
        }
        let (attrs, blocks) = unwrap_attributed_blocks(&definitions[0]);
        let mut entry = format!("`: {}", render_inlines(term, false)?);
        append_block_attrs(&mut entry, &render_attrs(attrs, None)?);
        let body = render_blocks(blocks)?;
        if !body.is_empty() {
            entry.push_str("\n\n");
            entry.push_str(&indent(&body, 2));
        }
        output.push(entry);
    }
    Ok(output.join("\n\n"))
}

fn render_container(marker: &str, attrs: &Attr, blocks: &[Block]) -> Result<String, String> {
    let attrs = render_attrs(attrs, Some("data-plumb-marker"))?;
    if let Some((head, children)) = split_head(blocks)? {
        let mut output = format!("`{marker} {head}");
        append_block_attrs(&mut output, &attrs);
        if !children.is_empty() {
            output.push_str("\n\n");
            output.push_str(&indent(&render_blocks(children)?, 2));
        }
        Ok(output)
    } else {
        let body = render_blocks(blocks)?;
        if body.is_empty() {
            let mut output = format!("`{marker}");
            append_block_attrs(&mut output, &attrs);
            Ok(output)
        } else {
            let mut output = format!("`{marker}");
            append_block_attrs(&mut output, &attrs);
            output.push('\n');
            output.push_str(&indent(&body, 2));
            Ok(output)
        }
    }
}

fn split_head(blocks: &[Block]) -> Result<Option<(String, &[Block])>, String> {
    let Some(Block::Para(inlines) | Block::Plain(inlines)) = blocks.first() else {
        return Ok(None);
    };
    if inlines
        .iter()
        .any(|inline| matches!(inline, Inline::SoftBreak | Inline::LineBreak))
    {
        return Ok(None);
    }
    Ok(Some((render_inlines(inlines, false)?, &blocks[1..])))
}

fn render_blocks(blocks: &[Block]) -> Result<String, String> {
    let mut output = Vec::new();
    for block in blocks {
        if let Some(block) = render_block(block)? {
            output.push(block);
        }
    }
    Ok(output.join("\n\n"))
}

fn render_inlines(inlines: &[Inline], bracketed: bool) -> Result<String, String> {
    let mut output = String::new();
    for inline in inlines {
        match inline {
            Inline::Str(text) => output.push_str(&escape_text(text, bracketed)),
            Inline::Space => output.push(' '),
            Inline::SoftBreak => output.push('\n'),
            Inline::Emph(content) => {
                output.push_str(&render_element("*", &Attr::default(), content)?)
            }
            Inline::Strong(content) => {
                output.push_str(&render_element("!", &Attr::default(), content)?)
            }
            Inline::Strikeout(content) => {
                output.push_str(&render_element("~", &Attr::default(), content)?)
            }
            Inline::Superscript(content) => {
                output.push_str(&render_element("^", &Attr::default(), content)?)
            }
            Inline::Subscript(content) => {
                output.push_str(&render_element("_", &Attr::default(), content)?)
            }
            Inline::Code(attrs, text) => output.push_str(&render_verbatim(text, attrs)?),
            Inline::Link(attrs, label, target) => {
                let mut attrs = attrs.clone();
                if attr_pair(&attrs, "data-plumb-marker") == Some("file") {
                    set_semantic_pair(&mut attrs, "src", &target.url)?;
                    output.push_str(&render_element("file", &attrs, label)?);
                } else {
                    output.push_str(&format!(
                        "`->[{}|{}{}]",
                        render_inlines(label, true)?,
                        render_verbatim_argument(&target.url),
                        render_inline_children(&attrs, Some("data-plumb-marker"))?
                    ));
                }
            }
            Inline::Image(attrs, alt, target) => {
                let mut attrs = attrs.clone();
                set_semantic_pair(&mut attrs, "src", &target.url)?;
                output.push_str(&render_element("img", &attrs, alt)?);
            }
            Inline::Math(MathType::InlineMath, text) => {
                let mut attrs = Attr::default();
                attrs.classes.push("$".into());
                output.push_str(&render_verbatim(text, &attrs)?);
            }
            Inline::Math(MathType::DisplayMath, _) => {
                return Err("DisplayMath cannot occur in plumb inline content".into())
            }
            Inline::Cite(citations, _) if citations.len() == 1 => {
                let citation = &citations[0];
                if !citation.citation_prefix.is_empty()
                    || !citation.citation_suffix.is_empty()
                    || !matches!(citation.citation_mode, CitationMode::NormalCitation)
                {
                    return Err("complex citation has no standard plumb representation".into());
                }
                require_attr_name(&citation.citation_id, "citation id")?;
                output.push_str(&format!("`cite[{}]", citation.citation_id));
            }
            Inline::Span(attrs, content) => {
                if let Some(attrs) = without_first_class(attrs, "mark") {
                    output.push_str(&render_element("==", &attrs, content)?);
                    continue;
                }
                if content.len() == 1 {
                    match &content[0] {
                        Inline::Emph(content) => {
                            output.push_str(&render_element("*", attrs, content)?);
                            continue;
                        }
                        Inline::Strong(content) => {
                            output.push_str(&render_element("!", attrs, content)?);
                            continue;
                        }
                        Inline::Strikeout(content) => {
                            output.push_str(&render_element("~", attrs, content)?);
                            continue;
                        }
                        Inline::Superscript(content) => {
                            output.push_str(&render_element("^", attrs, content)?);
                            continue;
                        }
                        Inline::Subscript(content) => {
                            output.push_str(&render_element("_", attrs, content)?);
                            continue;
                        }
                        Inline::Math(MathType::InlineMath, text) => {
                            let mut attrs = attrs.clone();
                            attrs.classes.push("$".into());
                            output.push_str(&render_verbatim(text, &attrs)?);
                            continue;
                        }
                        _ => {}
                    }
                }
                let marker = attr_pair(attrs, "data-plumb-marker").unwrap_or("()");
                require_marker(marker)?;
                output.push_str(&render_element(marker, attrs, content)?);
            }
            Inline::LineBreak => {
                return Err("LineBreak has no standard plumb representation".into())
            }
            Inline::Underline(_) => {
                return Err("Underline has no standard plumb representation".into())
            }
            Inline::SmallCaps(_) => {
                return Err("SmallCaps has no standard plumb representation".into())
            }
            Inline::Quoted(_, _) => {
                return Err("Quoted inline has no standard plumb representation".into())
            }
            Inline::Cite(_, _) => {
                return Err("citation cluster has no standard plumb representation".into())
            }
            Inline::RawInline(_, _) => {
                return Err("RawInline has no standard plumb representation".into())
            }
            Inline::Note(_) => return Err("Note has no standard plumb representation".into()),
        }
    }
    Ok(output)
}

fn render_element(marker: &str, attrs: &Attr, content: &[Inline]) -> Result<String, String> {
    require_marker(marker)?;
    Ok(format!(
        "`{marker}[{}{}]",
        render_inlines(content, true)?,
        render_inline_children(attrs, Some("data-plumb-marker"))?
    ))
}

fn render_verbatim(text: &str, attrs: &Attr) -> Result<String, String> {
    if text.contains(['\n', '\r']) {
        return Err("inline verbatim content cannot contain a line ending".into());
    }
    let (kind, attrs) = promoted_verbatim_attrs(attrs, false);
    let children = render_inline_children(&attrs, None)?;
    if children.is_empty() {
        if !text.contains('"') {
            return Ok(format!("`{kind}\"{text}\""));
        }
        let quotes = minimum_quote_count(text).max(1);
        return Ok(format!(
            "`{kind}{}[{}]{}",
            "\"".repeat(quotes),
            text,
            "\"".repeat(quotes),
        ));
    }
    let owner_kind = if kind.is_empty() { "code" } else { &kind };
    Ok(format!(
        "`{owner_kind}[|{}{}]",
        render_verbatim_argument(text),
        children
    ))
}

fn render_verbatim_argument(text: &str) -> String {
    let quotes = minimum_quote_count(text).max(1);
    format!("{}[{}]{}", "\"".repeat(quotes), text, "\"".repeat(quotes))
}

fn render_verbatim_block(attrs: &Attr, text: &str) -> Result<String, String> {
    let (kind, attrs) = promoted_verbatim_attrs(attrs, true);
    let attrs = render_attrs(&attrs, None)?;
    if kind.is_empty() && attrs.is_empty() {
        let mut output = "`\"".to_string();
        append_raw_payload(&mut output, text);
        return Ok(output);
    }

    let owner = if kind.is_empty() { "()" } else { &kind };
    let mut output = format!("`{owner}");
    append_block_attrs(&mut output, &attrs);
    output.push_str("\n\n|\"");
    append_raw_payload(&mut output, text);
    Ok(output)
}

fn append_raw_payload(output: &mut String, text: &str) {
    if !text.is_empty() {
        output.push('\n');
        let mut lines = text.split('\n').collect::<Vec<_>>();
        let has_final_newline = text.ends_with('\n');
        if has_final_newline {
            lines.pop();
        }
        let last_content = lines.iter().rposition(|line| !line.is_empty());
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                output.push('\n');
            }
            if !line.is_empty() {
                output.push(' ');
                output.push_str(line);
            } else if last_content.is_none_or(|last| index > last) {
                output.push(' ');
            }
        }
        if has_final_newline {
            output.push('\n');
        }
    }
}

fn render_attrs(attrs: &Attr, consumed_pair: Option<&str>) -> Result<String, String> {
    let mut items = Vec::new();
    if !attrs.identifier.is_empty() {
        require_attr_name(&attrs.identifier, "attribute id")?;
        items.push(format!("`@ {}", escape_block_text(&attrs.identifier)));
    }
    for class in &attrs.classes {
        require_attr_name(class, "attribute class")?;
        items.push(format!("`+ {}", escape_block_text(class)));
    }
    for (key, value) in &attrs.attributes {
        if consumed_pair == Some(key.as_str()) {
            continue;
        }
        require_attr_name(key, "attribute key")?;
        let value = if value.is_empty() {
            "`\"[]\"".to_string()
        } else {
            escape_block_text(value)
        };
        items.push(format!("`= {}|{value}", escape_block_text(key)));
    }
    Ok(if items.is_empty() {
        String::new()
    } else {
        items.join("\n")
    })
}

fn render_inline_children(attrs: &Attr, consumed_pair: Option<&str>) -> Result<String, String> {
    let mut members = Vec::new();
    if !attrs.identifier.is_empty() {
        require_attr_name(&attrs.identifier, "attribute id")?;
        members.push(format!("@[{}]", escape_text(&attrs.identifier, true)));
    }
    for class in &attrs.classes {
        require_attr_name(class, "attribute class")?;
        members.push(format!("+[{}]", escape_text(class, true)));
    }
    for (key, value) in &attrs.attributes {
        if consumed_pair == Some(key.as_str()) {
            continue;
        }
        require_attr_name(key, "attribute key")?;
        members.push(format!(
            "=[{}|{}]",
            escape_text(key, true),
            render_verbatim_argument(value)
        ));
    }
    Ok(members
        .into_iter()
        .map(|member| format!("|{member}"))
        .collect())
}

fn append_block_attrs(output: &mut String, attrs: &str) {
    if !attrs.is_empty() {
        output.push_str("\n\n");
        output.push_str(&indent(attrs, 1));
    }
}

fn escape_block_text(value: &str) -> String {
    value
        .replace('`', "``")
        .replace('[', "`[")
        .replace(']', "`]")
        .replace('|', "`|")
}

fn promoted_verbatim_attrs(attrs: &Attr, block: bool) -> (String, Attr) {
    let mut remaining = attrs.clone();
    let kind = if let Some(index) = remaining.classes.iter().position(|class| class == "->") {
        remaining.classes.remove(index);
        "->".to_string()
    } else if let Some(index) = remaining.classes.iter().position(|class| class == "$") {
        remaining.classes.remove(index);
        "$".to_string()
    } else if block {
        let language = remaining
            .attributes
            .iter()
            .position(|(key, _)| key == "language")
            .map(|index| remaining.attributes.remove(index).1)
            .unwrap_or_default();
        if language.is_empty() && !remaining.classes.is_empty() {
            remaining.classes.remove(0)
        } else {
            language
        }
    } else {
        String::new()
    };
    (kind, remaining)
}

fn unwrap_attributed_blocks(blocks: &[Block]) -> (&Attr, &[Block]) {
    static EMPTY: std::sync::OnceLock<Attr> = std::sync::OnceLock::new();
    if let [Block::Div(attrs, blocks)] = blocks {
        if attr_pair(attrs, "data-plumb-marker").is_none() {
            return (attrs, blocks);
        }
    }
    (EMPTY.get_or_init(Attr::default), blocks)
}

fn attr_pair<'a>(attrs: &'a Attr, key: &str) -> Option<&'a str> {
    attrs
        .attributes
        .iter()
        .find_map(|(candidate, value)| (candidate == key).then_some(value.as_str()))
}

fn without_first_class(attrs: &Attr, class: &str) -> Option<Attr> {
    let index = attrs
        .classes
        .iter()
        .position(|candidate| candidate == class)?;
    let mut attrs = attrs.clone();
    attrs.classes.remove(index);
    Some(attrs)
}

fn set_semantic_pair(attrs: &mut Attr, key: &str, value: &str) -> Result<(), String> {
    if let Some(existing) = attr_pair(attrs, key) {
        return if existing == value {
            Ok(())
        } else {
            Err(format!(
                "Pandoc {key} attribute {existing:?} conflicts with target {value:?}"
            ))
        };
    }
    attrs.attributes.push((key.into(), value.into()));
    Ok(())
}

fn require_marker(marker: &str) -> Result<(), String> {
    if !marker.is_empty()
        && marker.chars().all(|character| {
            !character.is_whitespace()
                && !character.is_control()
                && !matches!(character, '`' | '"' | '[' | ']' | '|')
        })
    {
        Ok(())
    } else {
        Err(format!("marker {marker:?} is not representable in plumb"))
    }
}

fn require_attr_name(value: &str, what: &str) -> Result<(), String> {
    if !value.is_empty()
        && value.chars().all(|character| {
            !character.is_whitespace()
                && !character.is_control()
                && !matches!(character, '`' | '"' | '[' | ']' | '#' | '.' | '=' | '|')
        })
    {
        Ok(())
    } else {
        Err(format!("{what} {value:?} is not representable in plumb"))
    }
}

fn minimum_quote_count(text: &str) -> usize {
    let mut quotes = 0;
    for (index, _) in text.match_indices(']') {
        let following = text[index + 1..]
            .chars()
            .take_while(|character| *character == '"')
            .count();
        quotes = quotes.max(following + 1);
    }
    quotes
}

fn escape_text(text: &str, bracketed: bool) -> String {
    let text = text.replace('`', "``");
    if bracketed {
        text.replace('[', "`[")
            .replace(']', "`]")
            .replace('|', "`|")
    } else {
        text
    }
}

fn indent(source: &str, columns: usize) -> String {
    let prefix = " ".repeat(columns);
    source
        .lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{prefix}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(any())]
mod tests {
    use serde_json::{json, Value};

    use super::*;

    #[test]
    fn imports_the_exported_standard_profile() {
        let document = json!({
            "pandoc-api-version": [1, 23, 1],
            "meta": {
                "tags": {"t": "MetaList", "c": [
                    {"t": "MetaInlines", "c": [{"t": "Str", "c": "plumb"}]},
                    {"t": "MetaInlines", "c": [{"t": "Str", "c": "markup"}]}
                ]},
                "title": {"t": "MetaInlines", "c": [{"t": "Str", "c": "Example"}]}
            },
            "blocks": [
                {"t": "Header", "c": [1, ["intro", [], []], [{"t": "Str", "c": "Intro"}]]},
                {"t": "Para", "c": [
                    {"t": "Emph", "c": [{"t": "Str", "c": "em"}]},
                    {"t": "Space"},
                    {"t": "Strong", "c": [{"t": "Str", "c": "strong"}]},
                    {"t": "Space"},
                    {"t": "Span", "c": [["marked", ["mark", "keep"], []], [{"t": "Str", "c": "marked"}]]},
                    {"t": "Space"},
                    {"t": "Strikeout", "c": [{"t": "Str", "c": "strike"}]},
                    {"t": "Space"},
                    {"t": "Superscript", "c": [{"t": "Str", "c": "super"}]},
                    {"t": "Space"},
                    {"t": "Subscript", "c": [{"t": "Str", "c": "sub"}]},
                    {"t": "Space"},
                    {"t": "Link", "c": [["", [], []], [{"t": "Str", "c": "target"}], ["other.plumb#id", ""]]},
                    {"t": "Space"},
                    {"t": "Link", "c": [["demo", ["wide"], [["download", "yes"], ["data-plumb-marker", "file"]]], [{"t": "Str", "c": "video"}], ["static/demo.mp4", ""]]}
                ]},
                {"t": "BlockQuote", "c": [{"t": "Para", "c": [{"t": "Str", "c": "quoted"}]}]},
                {"t": "BulletList", "c": [[{"t": "Para", "c": [{"t": "Str", "c": "item"}]}]]},
                {"t": "CodeBlock", "c": [["code", ["rust"], []], "fn main() {}\n"]}
            ]
        });

        let source = import_json(&document.to_string()).unwrap();
        assert!(
            source.starts_with("`= tags\n\n `+ plumb\n `+ markup\n\n"),
            "{source}"
        );
        assert!(source.contains("`= title|Example\n"), "{source}");
        assert!(source.contains("`# Intro\n\n `@ intro"), "{source}");
        assert!(source.contains("`*[em] `![strong] `==[marked|@[marked]|+[keep]]"));
        assert!(source.contains("`~[strike] `^[super] `_[sub]"));
        assert!(source.contains("`->[target|\"[other.plumb#id]\"]"));
        assert!(
            source.contains(
                "`file[video|@[demo]|+[wide]|=[download|\"[yes]\"]|=[src|\"[static/demo.mp4]\"]]"
            ),
            "{source}"
        );
        assert!(source.contains("`> quoted"));
        assert!(source.contains("`- item"));
        assert!(source.contains("`rust\n\n `@ code\n\n|\"\n fn main() {}"));
        assert!(plumb_syntax::parse(&source).is_valid());
    }

    #[test]
    fn rejects_unsupported_nodes_instead_of_dropping_them() {
        let document = Pandoc {
            blocks: vec![Block::HorizontalRule],
            meta: HashMap::new(),
        };
        assert!(import(&document).unwrap_err().contains("HorizontalRule"));
    }

    #[test]
    fn imports_compact_headers_and_expanded_rich_table_cells() {
        let cell = |content: Value| json!([["", [], []], {"t": "AlignDefault"}, 1, 1, content]);
        let document = json!({
            "pandoc-api-version": [1, 23, 1],
            "meta": {},
            "blocks": [{
                "t": "Table",
                "c": [
                    ["people", ["wide"], []],
                    [null, [{"t": "Plain", "c": [{"t": "Str", "c": "People"}]}]],
                    [
                        [{"t": "AlignDefault"}, {"t": "ColWidthDefault"}],
                        [{"t": "AlignDefault"}, {"t": "ColWidthDefault"}]
                    ],
                    [["", [], []], [[
                        ["", [], []],
                        [
                            cell(json!([{"t": "Plain", "c": [{"t": "Str", "c": "name"}]}])),
                            cell(json!([{"t": "Plain", "c": [{"t": "Str", "c": "age"}]}]))
                        ]
                    ]]],
                    [[ ["", [], []], 1, [], [[
                        ["", [], []],
                        [
                            cell(json!([{"t": "Plain", "c": [{"t": "Str", "c": "Alice"}]}])),
                            cell(json!([
                                {"t": "Plain", "c": [{"t": "Str", "c": "10"}]},
                                {"t": "Div", "c": [["", [], [["data-plumb-marker", "note"]]], [
                                    {"t": "Para", "c": [{"t": "Str", "c": "Approximate"}]}
                                ]]}
                            ]))
                        ]
                    ]]]],
                    [["", [], []], []]
                ]
            }]
        });

        let source = import_json(&document.to_string()).unwrap();
        assert!(source.contains("`table People"), "{source}");
        assert!(source.contains("`@ people"), "{source}");
        assert!(source.contains("`+ wide"), "{source}");
        assert!(source.contains("`- name|age"), "{source}");
        assert!(source.contains("`+ header"), "{source}");
        assert!(source.contains("`- Alice"), "{source}");
        assert!(source.contains("`note Approximate"), "{source}");
        assert!(plumb_syntax::parse(&source).is_valid(), "{source}");
    }

    #[test]
    fn decodes_pandoc_types_metadata_shape() {
        let value = json!({"t": "MetaInlines", "c": [{"t": "Str", "c": "Example"}]});
        let decoded = serde_json::from_value::<MetaValue>(value);
        assert!(decoded.is_ok(), "{decoded:?}");
    }

    #[test]
    fn imports_empty_code_attributes_and_chooses_verbatim_delimiters() {
        let mut attrs = Attr::default();
        attrs
            .attributes
            .push(("data".into(), "value]/{draft}".into()));
        let document = Pandoc {
            blocks: vec![
                Block::Para(vec![
                    Inline::Emph(vec![Inline::Str("a]b".into())]),
                    Inline::Space,
                    Inline::Code(attrs, "a]b".into()),
                ]),
                Block::CodeBlock(Attr::default(), "raw\n".into()),
            ],
            meta: HashMap::new(),
        };
        let source = import(&document).unwrap();
        assert!(source.contains("`*[a`]b]"), "{source}");
        assert!(source.contains("`code[|\"[a]b]\"|"));
        assert!(
            source.contains("`code[|\"[a]b]\"|=[data|\"[value]/{draft}]\"]]"),
            "{source}"
        );
        assert!(source.contains("`\"\n raw"), "{source}");
        let parsed = plumb_syntax::parse(&source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let plumb_syntax::Block::Parsed(paragraph) = &parsed.syntax.blocks[0] else {
            unreachable!();
        };
        let plumb_syntax::Inline::Element { attrs, .. } = &paragraph.head.items[2] else {
            unreachable!();
        };
        assert_eq!(attrs.value("data"), Some("value]/{draft}"));
    }
}
