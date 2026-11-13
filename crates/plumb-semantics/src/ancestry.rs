use plumb_syntax::{Block, ParsedBlock, ParsedDocument};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeAncestor {
    pub start: usize,
    pub title: String,
}

pub(crate) fn ancestor_label(block: &ParsedBlock) -> Option<NodeAncestor> {
    if block.mark.as_ref().is_some_and(|mark| mark.marker == "()") {
        return None;
    }
    let title = crate::semantic_plain_text(&block.content)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!title.is_empty()).then(|| NodeAncestor {
        start: block.range.start,
        title,
    })
}

/// Structural ownership only; declarations and transparent/empty heads add no label.
pub fn node_ancestors(parsed: &ParsedDocument, start: usize) -> Vec<NodeAncestor> {
    let mut result = Vec::new();
    let mut blocks = parsed
        .syntax
        .blocks
        .iter()
        .filter(|b| !crate::is_document_declaration(b))
        .collect::<Vec<_>>();
    loop {
        let Some(owner) = blocks.into_iter().find_map(|block| match block {
            Block::Parsed(owner) if owner.range.start < start && start < owner.range.end => {
                Some(owner)
            }
            _ => None,
        }) else {
            break;
        };
        if let Some(label) = ancestor_label(owner) {
            result.push(label);
        }
        blocks = crate::body_children(owner).collect();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_and_detail_paths_follow_owners_independent_of_facets_and_shift_with_edits() {
        for unrelated in [1, 100] {
            let source = format!("{}\n`- 找工作\n `() invisible\n  `- 字节 `!{{团队}}\n\n   Plain parent\n\n    `- 约面试情况确认\n     `+ task\n     `@ interview\n", "Unrelated\n\n".repeat(unrelated));
            let parsed = plumb_syntax::parse(&source);
            assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
            let output = crate::analyze_document(parsed.valid_syntax().unwrap());
            let task = output.tasks().tasks.iter().next().unwrap();
            let expected = ["找工作", "字节 团队", "Plain parent"];
            assert_eq!(
                task.ancestors
                    .iter()
                    .map(|a| a.title.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(task.ancestors, node_ancestors(&parsed, task.range.start));
            let changed = source.replacen("Unrelated", "Longer unrelated text", 1);
            let next = plumb_syntax::parse(&changed);
            let incremental = crate::analyze_document_incremental(
                next.valid_syntax().unwrap(),
                &output,
                &crate::DocumentChange {
                    old_range: 0..9,
                    new_range: 0..21,
                },
            );
            assert_eq!(
                incremental,
                crate::analyze_document(next.valid_syntax().unwrap())
            );
            assert_eq!(
                incremental.semantic_node_count() - incremental.reused_semantic_node_count(),
                1
            );
            let shifted = incremental.tasks().tasks.iter().next().unwrap();
            assert_eq!(
                shifted.ancestors,
                node_ancestors(&next, shifted.range.start)
            );
            assert_eq!(task.ancestors[0].title, "找工作");
            let renamed = source.replace("字节", "新的团队");
            let next = plumb_syntax::parse(&renamed);
            let start = source.find("字节").unwrap();
            let incremental = crate::analyze_document_incremental(
                next.valid_syntax().unwrap(),
                &output,
                &crate::DocumentChange {
                    old_range: start..start + "字节".len(),
                    new_range: start..start + "新的团队".len(),
                },
            );
            assert_eq!(
                incremental,
                crate::analyze_document(next.valid_syntax().unwrap())
            );
            assert_eq!(
                incremental.semantic_node_count() - incremental.reused_semantic_node_count(),
                1
            );
            assert_eq!(
                incremental.tasks().tasks.iter().next().unwrap().ancestors[1].title,
                "新的团队 团队"
            );
        }
    }
}
