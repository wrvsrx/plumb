use std::ops::Range;

use plumb_core::{Block, Diagnostic, Document, ParsedBlock};

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

impl HeadingOutput {
    pub fn heading_at_node_start(&self, start: usize) -> Option<&Heading> {
        find_heading(&self.headings, start)
    }
}

pub fn analyze_headings(document: &Document) -> HeadingOutput {
    let mut flat = Vec::new();
    collect_headings(&document.blocks, &mut flat);
    let diagnostics = Vec::new();
    let mut roots: Vec<Heading> = Vec::new();
    let mut path: Vec<usize> = Vec::new();

    for (index, block) in flat.iter().enumerate() {
        let level = heading_level(block).expect("only heading markers are collected");

        while let Some(parent) = get_heading(&roots, &path) {
            if parent.level < level {
                break;
            }
            path.pop();
        }

        let section_end = flat
            .iter()
            .skip(index + 1)
            .find(|next| heading_level(next).is_some_and(|next| next <= level))
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
        .and_then(hash_level)
        .is_some()
}

fn heading_level(block: &ParsedBlock) -> Option<u8> {
    let mark = block.mark.as_ref()?;
    let marker = mark.marker.as_deref()?;
    hash_level(marker)
}

fn hash_level(marker: &str) -> Option<u8> {
    let count = marker.bytes().take_while(|byte| *byte == b'#').count();
    (count == marker.len() && (1..=6).contains(&count)).then_some(count as u8)
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

fn find_heading(headings: &[Heading], start: usize) -> Option<&Heading> {
    for heading in headings {
        if heading.node_range.start == start {
            return Some(heading);
        }
        if let Some(found) = find_heading(&heading.children, start) {
            return Some(found);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use plumb_core::parse;

    use super::*;

    #[test]
    fn builds_heading_hierarchy() {
        let parsed = parse("`# One\n`## Two\n`# Three\n");
        let output = analyze_headings(&parsed.syntax);
        assert_eq!(output.headings.len(), 2);
        assert_eq!(output.headings[0].children[0].title, "Two");
    }
}
