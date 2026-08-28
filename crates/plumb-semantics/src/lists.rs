use std::ops::Range;

use plumb_syntax::{Block, ParsedBlock, ValidDocument};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItemRecord {
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    Bullet,
    Ordered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListGroup {
    pub range: Range<usize>,
    pub kind: ListKind,
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

pub fn analyze_lists(valid: ValidDocument<'_>) -> ListOutput {
    let document = valid.syntax();
    let mut output = ListOutput::default();
    collect_groups(
        document
            .blocks
            .iter()
            .filter(|block| !crate::is_document_declaration(block)),
        &mut output,
    );
    output.groups.sort_by_key(|group| group.range.start);
    output
}

fn collect_groups<'a>(blocks: impl IntoIterator<Item = &'a Block>, output: &mut ListOutput) {
    let mut blocks = blocks.into_iter().peekable();
    while let Some(current) = blocks.next() {
        let Some((first, kind)) = list_item(current) else {
            collect_child_groups(current, output);
            continue;
        };
        let mut items = Vec::new();
        let mut current = Some((first, kind));
        while let Some((item, item_kind)) = current {
            debug_assert_eq!(item_kind, kind);
            items.push(ListItemRecord {
                range: item.range.clone(),
                selection_range: crate::inline_selection_range(&item.head),
            });
            collect_groups(crate::body_children(item), output);
            current = if blocks
                .peek()
                .and_then(|next| list_item(next))
                .is_some_and(|(_, next_kind)| next_kind == kind)
            {
                blocks.next().and_then(list_item)
            } else {
                None
            };
        }
        output.groups.push(ListGroup {
            range: first.range.start..items.last().expect("list has an item").range.end,
            kind,
            items,
        });
    }
}

fn collect_child_groups(block: &Block, output: &mut ListOutput) {
    if let Block::Parsed(block) = block {
        if block
            .mark
            .as_ref()
            .is_some_and(|mark| mark.marker == "table")
        {
            collect_table_cell_groups(block, output);
            return;
        }
        collect_groups(crate::body_children(block), output);
    }
}

fn collect_table_cell_groups(table: &ParsedBlock, output: &mut ListOutput) {
    for row in crate::body_children(table).filter_map(parsed_dash) {
        if !row.head.is_empty() {
            continue;
        }
        for cell in crate::body_children(row).filter_map(parsed_dash) {
            collect_groups(crate::body_children(cell), output);
        }
    }
}

fn parsed_dash(block: &Block) -> Option<&ParsedBlock> {
    let Block::Parsed(block) = block else {
        return None;
    };
    block
        .mark
        .as_ref()
        .is_some_and(|mark| mark.marker == "-")
        .then_some(block)
}

fn list_item(block: &Block) -> Option<(&ParsedBlock, ListKind)> {
    let Block::Parsed(block) = block else {
        return None;
    };
    let kind = match block.mark.as_ref()?.marker.as_str() {
        "-" => ListKind::Bullet,
        "." => ListKind::Ordered,
        _ => return None,
    };
    Some((block, kind))
}

#[cfg(test)]
mod tests {
    use plumb_syntax::parse;

    use super::*;

    #[test]
    fn groups_adjacent_sibling_items_and_nested_items() {
        let parsed = parse(
            "`- One\n `+ task\n `- Nested one\n `- Nested two\n\n`- Two\n\nParagraph.\n\n`- Three\n",
        );
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_lists(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(output.groups.len(), 3);
        assert_eq!(output.groups[0].kind, ListKind::Bullet);
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

    #[test]
    fn separates_bullet_and_ordered_groups_and_recognizes_nested_lists() {
        let parsed = parse(
            "`- Bullet\n`. Ordered one\n  `. Nested ordered\n`. Ordered two\n`- Bullet again\n",
        );
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_lists(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(output.groups.len(), 4);
        assert_eq!(output.groups[0].kind, ListKind::Bullet);
        assert_eq!(output.groups[0].items.len(), 1);
        assert_eq!(output.groups[1].kind, ListKind::Ordered);
        assert_eq!(output.groups[1].items.len(), 2);
        assert_eq!(output.groups[2].kind, ListKind::Ordered);
        assert_eq!(output.groups[2].items.len(), 1);
        assert_eq!(output.groups[3].kind, ListKind::Bullet);
        assert_eq!(output.groups[3].items.len(), 1);
    }

    #[test]
    fn item_marker_is_not_a_list_item() {
        let parsed = parse("`item Generic block\n`- List item\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_lists(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(output.groups.len(), 1);
        assert_eq!(output.groups[0].range.start, "`item Generic block\n".len());
    }

    #[test]
    fn document_declarations_do_not_split_adjacent_body_lists() {
        let parsed = parse("`- First\n`= title|Between\n`+ journal\n`@ unsupported\n`- Second\n");
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_lists(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(output.groups.len(), 1);
        assert_eq!(output.groups[0].items.len(), 2);
    }

    #[test]
    fn table_rows_and_cells_are_not_lists_but_rich_cell_body_lists_are() {
        let parsed = parse(
            "`table\n `- name|age\n `- Alice|10\n `-\n  `- Rich\n\n   `- nested item\n  `- 20\n",
        );
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_lists(parsed.valid_syntax().unwrap());
        assert_eq!(output.groups.len(), 1);
        assert_eq!(output.groups[0].items.len(), 1);
        let nested = "nested item";
        assert_eq!(
            output.groups[0].range.start,
            parsed.source.find(nested).unwrap() - "`- ".len()
        );
    }
}
