use std::collections::HashMap;
use std::path::Path;

use chrono::Local;
use lsp_types::FoldingRange;
use plumb_core::{Block, Document};
use plumb_extensions::analyze_headings;
use plumb_workspace::{DocumentEntry, Workspace};

use crate::position::byte_range_to_lsp;

pub(crate) fn task_labels(
    workspace: &Workspace,
    path: &Path,
    entry: &DocumentEntry,
) -> HashMap<(usize, usize), String> {
    let Some(current) = &entry.current else {
        return HashMap::new();
    };
    let now = Local::now().fixed_offset();
    current
        .output
        .tasks
        .tasks
        .iter()
        .map(|task| {
            let (state, _) = workspace.task_workflow_state(path, task, now);
            let line_start = entry.parsed.source[..task.range.start]
                .rfind('\n')
                .map_or(0, |newline| newline + 1);
            let indent = &entry.parsed.source[line_start..task.range.start];
            let title = if task.title.is_empty() {
                "Untitled task"
            } else {
                &task.title
            };
            (
                (task.range.start, task.range.end),
                format!("{indent}{}  {title}", state.as_str().to_ascii_uppercase()),
            )
        })
        .collect()
}

pub(crate) fn ranges(
    source: &str,
    document: &Document,
    limit: Option<usize>,
    task_labels: Option<&HashMap<(usize, usize), String>>,
) -> Vec<FoldingRange> {
    let headings = analyze_headings(document);
    let mut byte_ranges = Vec::new();
    let mut pending_headings = headings.headings.iter().collect::<Vec<_>>();
    while let Some(heading) = pending_headings.pop() {
        byte_ranges.push(heading.section_range.clone());
        pending_headings.extend(heading.children.iter().rev());
    }

    let mut pending_blocks = document.blocks.iter().rev().collect::<Vec<_>>();
    while let Some(block) = pending_blocks.pop() {
        match block {
            Block::Parsed(parsed) => {
                if parsed.mark.is_some() {
                    byte_ranges.push(parsed.range.clone());
                }
                pending_blocks.extend(parsed.children.iter().rev());
            }
            Block::Verbatim(verbatim) => byte_ranges.push(verbatim.range.clone()),
        }
    }

    byte_ranges.sort_by_key(|range| (range.start, std::cmp::Reverse(range.end)));
    byte_ranges.dedup();
    let mut ranges = byte_ranges
        .into_iter()
        .filter_map(|range| {
            let collapsed_text = task_labels
                .and_then(|labels| labels.get(&(range.start, range.end)))
                .cloned();
            line_range(source, &range, collapsed_text)
        })
        .collect::<Vec<_>>();
    ranges.dedup();
    if let Some(limit) = limit {
        ranges.truncate(limit);
    }
    ranges
}

fn line_range(
    source: &str,
    range: &std::ops::Range<usize>,
    collapsed_text: Option<String>,
) -> Option<FoldingRange> {
    let content_end = range.start
        + source[range.clone()]
            .trim_end_matches(char::is_whitespace)
            .len();
    let range = byte_range_to_lsp(source, &(range.start..content_end));
    let end_line = if range.end.character == 0 && range.end.line > range.start.line {
        range.end.line - 1
    } else {
        range.end.line
    };
    (end_line > range.start.line).then_some(FoldingRange {
        start_line: range.start.line,
        start_character: None,
        end_line,
        end_character: None,
        kind: None,
        collapsed_text,
    })
}
