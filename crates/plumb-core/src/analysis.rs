use std::ops::Range;

use crate::{Block, Diagnostic, DiagnosticSeverity, Document, ParsedBlock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    pub node_range: Range<usize>,
    pub selection_range: Range<usize>,
    pub section_range: Range<usize>,
    pub level: u8,
    pub title: String,
    pub children: Vec<Heading>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HeadingOutput {
    pub headings: Vec<Heading>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn analyze_headings(document: &Document) -> HeadingOutput {
    let mut flat = Vec::new();
    collect_headings(&document.blocks, &mut flat);
    let mut diagnostics = Vec::new();
    let mut roots: Vec<Heading> = Vec::new();
    let mut path: Vec<usize> = Vec::new();

    for (index, block) in flat.iter().enumerate() {
        let Some(level) = heading_level(block, &mut diagnostics) else {
            continue;
        };

        while let Some(parent) = get_heading(&roots, &path) {
            if parent.level < level {
                break;
            }
            path.pop();
        }

        let section_end = flat
            .iter()
            .skip(index + 1)
            .find(|next| heading_level_quiet(next).is_some_and(|next| next <= level))
            .map(|next| next.range.start)
            .unwrap_or(document.range.end);

        let heading = Heading {
            node_range: block.range.clone(),
            selection_range: block.head.range.clone(),
            section_range: block.range.start..section_end,
            level,
            title: block.head.plain_text(),
            children: Vec::new(),
        };
        let siblings = get_heading_children_mut(&mut roots, &path);
        siblings.push(heading);
        path.push(siblings.len() - 1);
    }

    HeadingOutput {
        headings: roots,
        diagnostics,
    }
}

fn collect_headings<'a>(blocks: &'a [Block], output: &mut Vec<&'a ParsedBlock>) {
    for block in blocks {
        if let Block::Parsed(parsed) = block {
            if is_heading_marker(parsed) {
                output.push(parsed);
            }
            collect_headings(&parsed.children, output);
        }
    }
}

fn is_heading_marker(block: &ParsedBlock) -> bool {
    block
        .mark
        .as_ref()
        .and_then(|mark| mark.marker.as_deref())
        .is_some_and(|marker| marker == "heading" || hash_level(marker).is_some())
}

fn heading_level(block: &ParsedBlock, diagnostics: &mut Vec<Diagnostic>) -> Option<u8> {
    let mark = block.mark.as_ref()?;
    let marker = mark.marker.as_deref()?;
    if let Some(level) = hash_level(marker) {
        return Some(level);
    }
    let Some(level_text) = mark.attrs.value("level") else {
        diagnostics.push(Diagnostic {
            code: "heading.missing-level",
            severity: DiagnosticSeverity::Warning,
            message: "heading requires a level attribute from 1 through 6".to_string(),
            range: block.range.clone(),
            related: Vec::new(),
        });
        return None;
    };
    match level_text.parse::<u8>() {
        Ok(level) if (1..=6).contains(&level) => Some(level),
        _ => {
            diagnostics.push(invalid_level(block));
            None
        }
    }
}

fn heading_level_quiet(block: &ParsedBlock) -> Option<u8> {
    let mark = block.mark.as_ref()?;
    let marker = mark.marker.as_deref()?;
    hash_level(marker).or_else(|| {
        (marker == "heading")
            .then(|| mark.attrs.value("level")?.parse::<u8>().ok())
            .flatten()
            .filter(|level| (1..=6).contains(level))
    })
}

fn hash_level(marker: &str) -> Option<u8> {
    let count = marker.bytes().take_while(|byte| *byte == b'#').count();
    (count == marker.len() && (1..=6).contains(&count)).then_some(count as u8)
}

fn invalid_level(block: &ParsedBlock) -> Diagnostic {
    Diagnostic {
        code: "heading.invalid-level",
        severity: DiagnosticSeverity::Warning,
        message: "heading level must be an integer from 1 through 6".to_string(),
        range: block.range.clone(),
        related: Vec::new(),
    }
}

fn get_heading<'a>(roots: &'a [Heading], path: &[usize]) -> Option<&'a Heading> {
    let (first, rest) = path.split_first()?;
    let mut current = roots.get(*first)?;
    for index in rest {
        current = current.children.get(*index)?;
    }
    Some(current)
}

fn get_heading_children_mut<'a>(
    roots: &'a mut Vec<Heading>,
    path: &[usize],
) -> &'a mut Vec<Heading> {
    let mut children = roots;
    for index in path {
        children = &mut children[*index].children;
    }
    children
}
