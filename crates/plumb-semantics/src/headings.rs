use std::ops::Range;

use plumb_syntax::{Block, Diagnostic, Document, ParsedBlock, ValidDocument};

use crate::text::plain_text;

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

pub fn analyze_headings(valid: ValidDocument<'_>) -> HeadingOutput {
    analyze_recovered_headings(valid.syntax())
}

pub fn analyze_recovered_headings(document: &Document) -> HeadingOutput {
    let mut flat = Vec::new();
    for block in document
        .blocks
        .iter()
        .filter(|block| !crate::is_document_declaration(block))
    {
        collect_headings(std::slice::from_ref(block), &mut flat);
    }
    build_headings(
        flat.into_iter()
            .map(|block| heading_seed(block, 0))
            .collect(),
        document.range.end,
    )
}

struct HeadingSeed {
    node_range: Range<usize>,
    selection_range: Range<usize>,
    level: u8,
    title: String,
}

pub(crate) fn reduce_heading_outputs<'a>(
    outputs: impl IntoIterator<Item = (usize, &'a HeadingOutput)>,
    document_end: usize,
) -> HeadingOutput {
    let mut flat = Vec::new();
    for (offset, output) in outputs {
        append_heading_seeds(&output.headings, offset, &mut flat);
    }
    build_headings(flat, document_end)
}

fn append_heading_seeds(headings: &[Heading], offset: usize, output: &mut Vec<HeadingSeed>) {
    for heading in headings {
        output.push(HeadingSeed {
            node_range: heading.node_range.start + offset..heading.node_range.end + offset,
            selection_range: heading.selection_range.start + offset
                ..heading.selection_range.end + offset,
            level: heading.level,
            title: heading.title.clone(),
        });
        append_heading_seeds(&heading.children, offset, output);
    }
}

pub(crate) fn heading_topology_eq(previous: &HeadingOutput, current: &HeadingOutput) -> bool {
    fn levels(headings: &[Heading], output: &mut Vec<u8>) {
        for heading in headings {
            output.push(heading.level);
            levels(&heading.children, output);
        }
    }
    let mut previous_levels = Vec::new();
    let mut current_levels = Vec::new();
    levels(&previous.headings, &mut previous_levels);
    levels(&current.headings, &mut current_levels);
    previous_levels == current_levels
}

fn heading_seed(block: &ParsedBlock, offset: usize) -> HeadingSeed {
    let shift = |range: &Range<usize>| range.start + offset..range.end + offset;
    HeadingSeed {
        node_range: shift(&block.range),
        selection_range: shift(&crate::inline_selection_range(&block.content)),
        level: heading_level(block).expect("only heading markers are collected"),
        title: plain_text(&block.content),
    }
}

fn build_headings(flat: Vec<HeadingSeed>, document_end: usize) -> HeadingOutput {
    let diagnostics = Vec::new();
    let mut roots: Vec<Heading> = Vec::new();
    let mut path: Vec<usize> = Vec::new();

    for (index, heading) in flat.iter().enumerate() {
        let level = heading.level;

        while let Some(parent) = get_heading(&roots, &path) {
            if parent.level < level {
                break;
            }
            path.pop();
        }

        let section_end = flat
            .iter()
            .skip(index + 1)
            .find(|next| next.level <= level)
            .map(|next| next.node_range.start)
            .unwrap_or(document_end);

        let heading = Heading {
            node_range: heading.node_range.clone(),
            selection_range: heading.selection_range.clone(),
            section_range: heading.node_range.start..section_end,
            level,
            title: heading.title.clone(),
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
            for child in crate::body_children(parsed) {
                collect_headings(std::slice::from_ref(child), output);
            }
        }
    }
}

fn is_heading_marker(block: &ParsedBlock) -> bool {
    block
        .mark
        .as_ref()
        .map(|mark| mark.marker.as_str())
        .and_then(hash_level)
        .is_some()
}

fn heading_level(block: &ParsedBlock) -> Option<u8> {
    let mark = block.mark.as_ref()?;
    hash_level(&mark.marker)
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
    use plumb_syntax::parse;

    use super::*;

    #[test]
    fn builds_heading_hierarchy() {
        let source = "`# One\n`## Two\n`# Three\n";
        let parsed = parse(source);
        let output = analyze_headings(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(output.headings.len(), 2);
        assert_eq!(output.headings[0].children[0].title, "Two");
        let green = plumb_syntax::GreenDocument::parse(source);
        let outputs = green
            .shards()
            .map(|shard| {
                (
                    shard.offset(),
                    analyze_headings(shard.shard().parsed().valid_syntax().unwrap()),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            reduce_heading_outputs(
                outputs.iter().map(|(offset, output)| (*offset, output)),
                source.len(),
            ),
            output
        );
    }
}
