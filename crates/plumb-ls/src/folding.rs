use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, FixedOffset, Local};
use lsp_types::FoldingRange;
use plumb_core::{Block, Document};
use plumb_extensions::{analyze_headings, EventRecord};
use plumb_workspace::{DocumentEntry, Workspace};

use crate::position::byte_range_to_lsp;

pub(crate) fn collapsed_text_labels(
    workspace: &Workspace,
    path: &Path,
    entry: &DocumentEntry,
) -> HashMap<(usize, usize), String> {
    let mut labels = task_labels(workspace, path, entry);
    labels.extend(event_labels(entry));
    labels.extend(metadata_labels(entry));
    labels
}

pub(crate) fn metadata_labels(entry: &DocumentEntry) -> HashMap<(usize, usize), String> {
    let Some(metadata) = entry
        .current
        .as_ref()
        .and_then(|current| current.output.metadata.metadata.as_ref())
    else {
        return HashMap::new();
    };
    let indent = line_indent(&entry.parsed.source, metadata.range.start);
    let title = entry
        .current
        .as_ref()
        .and_then(|current| current.output.metadata.document_title())
        .map(|title| single_line_label(&title, 80))
        .filter(|title| !title.is_empty());
    let label = title.map_or_else(
        || format!("{indent}METADATA"),
        |title| format!("{indent}METADATA  {title}"),
    );
    HashMap::from([((metadata.range.start, metadata.range.end), label)])
}

fn single_line_label(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    let mut truncated = normalized
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

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
            let indent = line_indent(&entry.parsed.source, task.range.start);
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

pub(crate) fn event_labels(entry: &DocumentEntry) -> HashMap<(usize, usize), String> {
    let Some(current) = &entry.current else {
        return HashMap::new();
    };
    current
        .output
        .events
        .events
        .iter()
        .filter_map(|event| {
            let time = event_time_label(event)?;
            let indent = line_indent(&entry.parsed.source, event.range.start);
            let title = if event.title.is_empty() {
                "Untitled event"
            } else {
                &event.title
            };
            Some((
                (event.range.start, event.range.end),
                format!("{indent}{time}  {title}"),
            ))
        })
        .collect()
}

const SECONDS_PER_DAY: i64 = 24 * 60 * 60;

/// Abbreviates an event's time shape into a compact, RFC 3339-derived label
/// evaluated in the event's own declared offset (seconds and offset dropped). A
/// point `at` event renders its datetime alone; an interval renders
/// `start--end`, where `end` keeps only `HH:MM` while it is within 24 hours of
/// `start` (the cutoff beyond which a bare end time would point to the wrong
/// day) and expands to the full datetime once it spans further; a `start`-only
/// event renders `start-running`. Events without a usable time yield `None`.
fn event_time_label(event: &EventRecord) -> Option<String> {
    if let Some(at) = event.at_datetime() {
        return Some(format_datetime(&at));
    }
    let start = event.start_datetime()?;
    let start_label = format_datetime(&start);
    Some(match event.end_datetime() {
        Some(end) => {
            let end_label = if end.signed_duration_since(start).num_seconds() <= SECONDS_PER_DAY {
                format_time(&end)
            } else {
                format_datetime(&end)
            };
            format!("{start_label}--{end_label}")
        }
        None => format!("{start_label}-running"),
    })
}

fn format_datetime(datetime: &DateTime<FixedOffset>) -> String {
    datetime.format("%Y-%m-%dT%H:%M").to_string()
}

fn format_time(datetime: &DateTime<FixedOffset>) -> String {
    datetime.format("%H:%M").to_string()
}

fn line_indent<'a>(source: &'a str, range_start: usize) -> &'a str {
    let line_start = source[..range_start]
        .rfind('\n')
        .map_or(0, |newline| newline + 1);
    &source[line_start..range_start]
}

pub(crate) fn ranges(
    source: &str,
    document: &Document,
    limit: Option<usize>,
    labels: Option<&HashMap<(usize, usize), String>>,
    line_folding_only: bool,
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
            let collapsed_text = labels
                .and_then(|table| table.get(&(range.start, range.end)))
                .cloned();
            line_range(source, &range, collapsed_text, line_folding_only)
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
    line_folding_only: bool,
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
    if end_line == range.start.line && collapsed_text.is_none() {
        return None;
    }
    let same_line = end_line == range.start.line;
    Some(FoldingRange {
        start_line: range.start.line,
        start_character: (same_line && !line_folding_only).then_some(range.start.character),
        end_line,
        end_character: (same_line && !line_folding_only).then_some(range.end.character),
        kind: None,
        collapsed_text,
    })
}

#[cfg(test)]
mod tests {
    use super::single_line_label;

    #[test]
    fn normalizes_and_truncates_fold_labels_on_character_boundaries() {
        assert_eq!(
            single_line_label("  Project\n  Overview  ", 80),
            "Project Overview"
        );
        assert_eq!(single_line_label("项目项目项目项目", 6), "项目项...");
    }
}
