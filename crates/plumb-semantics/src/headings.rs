use crate::text::plain_text;
use plumb_syntax::{Block, Diagnostic, Document, ParsedBlock, SemanticDocument};
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    pub node_range: Range<usize>,
    pub selection_range: Range<usize>,
    pub section_range: Range<usize>,
    pub level: usize,
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
/// Table structural owners are excluded by the contextual traversal.
pub fn is_section(block: &ParsedBlock) -> bool {
    block.mark.as_ref().is_some_and(|mark| {
        mark.marker == "#"
            || (matches!(mark.marker.as_str(), "-" | ".") && mark.attrs.has_class("section"))
    })
}
pub fn analyze_headings<'a>(valid: impl Into<SemanticDocument<'a>>) -> HeadingOutput {
    analyze_recovered_headings(valid.into().syntax())
}

pub fn analyze_recovered_headings(document: &Document) -> HeadingOutput {
    let mut output = HeadingOutput::default();
    collect_headings(
        document
            .blocks
            .iter()
            .filter(|block| !crate::is_document_declaration(block)),
        1,
        &mut output.headings,
    );
    output
}
pub(crate) fn reduce_heading_outputs<'a>(
    outputs: impl IntoIterator<Item = (usize, &'a HeadingOutput)>,
    _document_end: usize,
) -> HeadingOutput {
    fn shifted(heading: &Heading, offset: usize) -> Heading {
        let shift = |range: &Range<usize>| range.start + offset..range.end + offset;
        Heading {
            node_range: shift(&heading.node_range),
            selection_range: shift(&heading.selection_range),
            section_range: shift(&heading.section_range),
            level: heading.level,
            title: heading.title.clone(),
            children: heading
                .children
                .iter()
                .map(|child| shifted(child, offset))
                .collect(),
        }
    }
    let mut output = HeadingOutput::default();
    for (offset, local) in outputs {
        output.headings.extend(
            local
                .headings
                .iter()
                .map(|heading| shifted(heading, offset)),
        );
    }
    output
}
pub(crate) fn heading_topology_eq(previous: &HeadingOutput, current: &HeadingOutput) -> bool {
    fn equal(previous: &[Heading], current: &[Heading]) -> bool {
        previous.len() == current.len()
            && previous
                .iter()
                .zip(current)
                .all(|(a, b)| a.level == b.level && equal(&a.children, &b.children))
    }
    equal(&previous.headings, &current.headings)
}
fn collect_headings<'a>(
    blocks: impl IntoIterator<Item = &'a Block>,
    level: usize,
    output: &mut Vec<Heading>,
) {
    for block in blocks {
        let Block::Parsed(block) = block else {
            continue;
        };
        if block.mark.as_ref().is_some_and(|mark| mark.marker == "|") {
            for row in crate::body_children(block) {
                let Block::Parsed(row) = row else {
                    continue;
                };
                if !row.mark.as_ref().is_some_and(|mark| mark.marker == "-")
                    || !row.content.is_empty()
                {
                    continue;
                }
                for cell in crate::body_children(row) {
                    if let Block::Parsed(cell) = cell {
                        collect_headings(crate::body_children(cell), level, output);
                    }
                }
            }
        } else if is_section(block) {
            let mut children = Vec::new();
            collect_headings(crate::body_children(block), level + 1, &mut children);
            output.push(Heading {
                node_range: block.range.clone(),
                selection_range: crate::inline_selection_range(&block.content),
                section_range: block.range.clone(),
                level,
                title: plain_text(&block.content),
                children,
            });
        } else {
            collect_headings(crate::body_children(block), level, output);
        }
    }
}

fn find_heading(mut headings: &[Heading], start: usize) -> Option<&Heading> {
    loop {
        let index = headings
            .partition_point(|heading| heading.node_range.start <= start)
            .checked_sub(1)?;
        let heading = &headings[index];
        if heading.node_range.start == start {
            return Some(heading);
        }
        headings = &heading.children;
    }
}

#[cfg(test)]
mod tests {
    use plumb_syntax::parse;

    use super::*;

    #[test]
    fn hash_shorthand_matches_section_facet_for_tasks_events_and_categories() {
        for marker in ["`- Parent\n `+ section", "`# Parent"] {
            let source = format!("{marker}\n `= task-category work\n `= event-category work\n `# Task\n  `+ task\n  `@ task\n  `= created 2026-10-23T09:00:00+08:00\n `# Event\n  `+ event\n  `@ event\n  `= date 2026-10-23\n  `= timezone +08:00\n\n`## Generic\n\n`table Generic\n");
            let parsed = parse(&source);
            assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
            let output = crate::analyze_document(parsed.valid_syntax().unwrap());
            assert_eq!(output.headings().headings.len(), 1);
            assert_eq!(output.headings().headings[0].children.len(), 2);
            assert_eq!(output.headings().headings[0].children[1].level, 2);
            assert_eq!(output.tasks().tasks.len(), 1);
            assert_eq!(output.events().events.len(), 1);
            assert!(output.lists().groups.is_empty());
            assert!(output.tables().tables.is_empty());
            assert!(output
                .anchors
                .iter()
                .all(|anchor| anchor.kind == crate::AnchorKind::Heading));
            let changed = source.replacen("`# Task", "`- Task\n  `+ section", 1);
            let next = parse(&changed);
            let start = source.find("`# Task").unwrap();
            let incremental = crate::analyze_document_incremental(
                next.valid_syntax().unwrap(),
                &output,
                &crate::DocumentChange {
                    old_range: start..start + 2,
                    new_range: start..start + 15,
                },
            );
            assert_eq!(
                incremental,
                crate::analyze_document(next.valid_syntax().unwrap())
            );
        }
    }
    #[test]
    fn section_ancestry_ignores_ordinary_containers_and_hash_markers() {
        let source = "`## Generic\n\n`- Root\n `+ section\n `+ task\n `@ root\n\n `node Container\n  `- Child\n   `+ section\n\n`- Sibling\n `+ section\n";
        let parsed = parse(source);
        let output = crate::analyze_document(parsed.valid_syntax().unwrap());
        assert_eq!(output.headings().headings.len(), 2);
        let root = &output.headings().headings[0];
        assert_eq!(root.children[0].level, 2);
        assert_eq!(root.section_range, root.node_range);
        assert!(root.section_range.end <= output.headings().headings[1].node_range.start);
        assert_eq!(output.tasks().tasks.len(), 1);
        assert_eq!(
            output.anchors.get(0).unwrap().kind,
            crate::AnchorKind::Heading
        );
        assert!(output.lists().groups.is_empty());
    }

    #[test]
    fn sections_interrupt_lists_but_not_table_structural_roles() {
        let source = "`- Before\n`- Section\n `+ section\n\n `- Body\n`- After\n\n`|\n `- cell\n  `+ section\n `-\n  `- Cell\n   `+ section\n\n   `- Inner\n    `+ section\n";
        let parsed = parse(source);
        let output = crate::analyze_document(parsed.valid_syntax().unwrap());
        assert_eq!(output.headings().headings.len(), 2);
        assert_eq!(output.headings().headings[1].title, "Inner");
        assert_eq!(output.headings().headings[1].level, 1);
        assert_eq!(output.lists().groups.len(), 3);
        assert!(output
            .lists()
            .groups
            .iter()
            .all(|group| group.items.len() == 1));
    }

    #[test]
    fn structural_section_updates_reuse_unrelated_nodes_and_preserve_snapshots() {
        for unrelated in [1, 200] {
            let source = format!(
                "`- Root\n `+ section\n\n Body\n\n{}",
                "Unrelated\n\n".repeat(unrelated)
            );
            let parsed = parse(&source);
            let previous = crate::analyze_document(parsed.valid_syntax().unwrap());
            for replacement in ["notice", "section\n `- Child\n  `+ section"] {
                let changed = source.replacen("section", replacement, 1);
                let parsed = parse(&changed);
                let start = source.find("section").unwrap();
                let incremental = crate::analyze_document_incremental(
                    parsed.valid_syntax().unwrap(),
                    &previous,
                    &crate::DocumentChange {
                        old_range: start..start + 7,
                        new_range: start..start + replacement.len(),
                    },
                );
                assert_eq!(
                    incremental,
                    crate::analyze_document(parsed.valid_syntax().unwrap())
                );
                assert_eq!(
                    incremental.semantic_node_count() - incremental.reused_semantic_node_count(),
                    1
                );
                assert_eq!(previous.headings().headings.len(), 1);
                assert!(previous.headings().headings[0].children.is_empty());
            }
        }
    }

    #[test]
    fn exact_heading_lookup_matches_recursive_order_at_every_byte() {
        let source = "Prelude\n\n`- One\n `+ section\n `- Deep\n  `+ section\n `- Two\n  `+ section\n\n  Body\n\n`- Three\n `+ section\n `- Last\n  `+ section\n";
        let parsed = parse(source);
        let output = analyze_headings(parsed.valid_syntax().unwrap());
        fn scan(headings: &[Heading], start: usize) -> Option<&Heading> {
            headings.iter().find_map(|heading| {
                if heading.node_range.start == start {
                    Some(heading)
                } else {
                    scan(&heading.children, start)
                }
            })
        }
        for start in 0..=source.len() {
            assert_eq!(
                output.heading_at_node_start(start),
                scan(&output.headings, start)
            );
        }
    }

    #[test]
    fn builds_heading_hierarchy() {
        let source = "`- One\n `+ section\n `- Two\n  `+ section\n`- Three\n `+ section\n";
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
