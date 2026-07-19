use std::ops::Range;

use plumb_core::{Block, Document, ParsedBlock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItemRecord {
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListGroup {
    pub range: Range<usize>,
    pub items: Vec<ListItemRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListOutput {
    pub groups: Vec<ListGroup>,
}

impl ListOutput {
    pub fn group_at_node_start(&self, start: usize) -> Option<&ListGroup> {
        self.groups.iter().find(|group| group.range.start == start)
    }
}

pub fn analyze_lists(document: &Document) -> ListOutput {
    let mut output = ListOutput::default();
    collect_groups(&document.blocks, &mut output);
    output.groups.sort_by_key(|group| group.range.start);
    output
}

fn collect_groups(blocks: &[Block], output: &mut ListOutput) {
    let mut index = 0;
    while index < blocks.len() {
        let Some(first) = list_item(&blocks[index]) else {
            collect_child_groups(&blocks[index], output);
            index += 1;
            continue;
        };
        let mut items = Vec::new();
        while let Some(item) = blocks.get(index).and_then(list_item) {
            items.push(ListItemRecord {
                range: item.range.clone(),
                selection_range: item.head.range.clone(),
            });
            collect_groups(&item.children, output);
            index += 1;
        }
        output.groups.push(ListGroup {
            range: first.range.start..items.last().expect("list has an item").range.end,
            items,
        });
    }
}

fn collect_child_groups(block: &Block, output: &mut ListOutput) {
    if let Block::Parsed(block) = block {
        collect_groups(&block.children, output);
    }
}

fn list_item(block: &Block) -> Option<&ParsedBlock> {
    let Block::Parsed(block) = block else {
        return None;
    };
    block
        .mark
        .as_ref()
        .is_some_and(|mark| matches!(mark.marker.as_str(), "item" | "-"))
        .then_some(block)
}

#[cfg(test)]
mod tests {
    use plumb_core::parse;

    use super::*;

    #[test]
    fn groups_adjacent_sibling_items_and_nested_items() {
        let parsed = parse(
            "`-{.task} One\n  `item Nested one\n  `- Nested two\n`item Two\nParagraph.\n`- Three\n",
        );
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_lists(&parsed.syntax);
        assert_eq!(output.groups.len(), 3);
        assert_eq!(output.groups[0].items.len(), 2);
        assert_eq!(output.groups[1].items.len(), 2);
        assert_eq!(output.groups[2].items.len(), 1);
        assert!(output
            .groups
            .windows(2)
            .all(|groups| groups[0].range.start < groups[1].range.start));
        assert_eq!(
            output
                .group_at_node_start(output.groups[0].range.start)
                .unwrap(),
            &output.groups[0]
        );
    }
}
