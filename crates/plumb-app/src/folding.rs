use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, FixedOffset, Local};
use lsp_types::FoldingRange;
use plumb_semantics::{
    analyze_recovered_headings, EventRecord, MetadataValue, TaskRecord, TaskState,
};
use plumb_syntax::{Block, Document};
use plumb_workspace::{DocumentEntry, TaskWorkflowState, Workspace, WorkspaceQueryError};

use crate::position::PositionIndex;

#[derive(Clone)]
pub(crate) struct FoldLabel {
    text: String,
}

pub(crate) fn collapsed_text_labels(
    workspace: &Workspace,
    path: &Path,
    entry: &DocumentEntry,
    index_complete: bool,
) -> HashMap<(usize, usize), FoldLabel> {
    let mut labels = task_labels(workspace, path, entry, index_complete);
    labels.extend(event_labels(entry));
    labels.extend(metadata_labels(entry));
    labels
}

pub(crate) fn metadata_labels(entry: &DocumentEntry) -> HashMap<(usize, usize), FoldLabel> {
    let Some(metadata) = entry
        .current
        .as_ref()
        .and_then(|current| current.output.metadata.metadata.as_ref())
    else {
        return HashMap::new();
    };
    let source = &entry.parsed.source;
    metadata
        .entries
        .iter()
        .map(|entry| {
            let indent = line_indent(source, entry.range.start);
            let value = match &entry.value {
                MetadataValue::Scalar { content, .. } => content.plain_text(),
                MetadataValue::Verbatim { text, .. } => text.clone(),
                MetadataValue::Null { .. }
                | MetadataValue::List { .. }
                | MetadataValue::Map { .. }
                | MetadataValue::Unsupported { .. } => String::new(),
            };
            let value = single_line_label(&value, 80);
            let label = if value.is_empty() {
                format!("{indent}{}", entry.key)
            } else {
                format!("{indent}{}  {value}", entry.key)
            };
            (
                (entry.range.start, entry.range.end),
                FoldLabel { text: label },
            )
        })
        .collect()
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
    index_complete: bool,
) -> HashMap<(usize, usize), FoldLabel> {
    let Some(current) = &entry.current else {
        return HashMap::new();
    };
    let now = Local::now().fixed_offset();
    current
        .output
        .tasks
        .tasks
        .iter()
        .filter_map(|task| {
            let state = match task_label_state(workspace, path, task, now, index_complete) {
                Ok(Some(state)) => state,
                Ok(None) => return None,
                Err(error) => {
                    tracing::error!(%error, "task fold label query failed");
                    return None;
                }
            };
            let indent = line_indent(&entry.parsed.source, task.range.start);
            let marker = &entry.parsed.source[task.range.start + 1..task.range.start + 2];
            let title = if task.title.is_empty() {
                "Untitled task"
            } else {
                &task.title
            };
            Some((
                (task.range.start, task.range.end),
                FoldLabel {
                    text: format!("{indent}`{marker} {:<5}{title}", task_state_symbol(state)),
                },
            ))
        })
        .collect()
}

fn task_label_state(
    workspace: &Workspace,
    path: &Path,
    task: &TaskRecord,
    now: DateTime<FixedOffset>,
    index_complete: bool,
) -> Result<Option<TaskWorkflowState>, WorkspaceQueryError> {
    if index_complete {
        return Ok(Some(
            workspace.task_workflow_state(path, task, now)?.value.0,
        ));
    }

    Ok(match task.state() {
        TaskState::Done => Some(TaskWorkflowState::Done),
        TaskState::Canceled => Some(TaskWorkflowState::Canceled),
        TaskState::Conflicted => Some(TaskWorkflowState::Conflicted),
        TaskState::Open => {
            let waiting = task
                .wait
                .as_ref()
                .and_then(|wait| DateTime::parse_from_rfc3339(&wait.value).ok())
                .is_some_and(|wait| wait > now);
            if waiting {
                Some(TaskWorkflowState::Waiting)
            } else if task.depends.is_empty() {
                Some(TaskWorkflowState::Ready)
            } else {
                let dependencies = workspace.task_dependencies(path, task)?.value;
                if dependencies
                    .iter()
                    .any(|dependency| dependency.task.state() == TaskState::Open)
                {
                    Some(TaskWorkflowState::Blocked)
                } else if dependencies.len() == task.depends.len() {
                    Some(TaskWorkflowState::Ready)
                } else {
                    None
                }
            }
        }
    })
}

fn task_state_symbol(state: TaskWorkflowState) -> &'static str {
    match state {
        TaskWorkflowState::Ready => "[ ]",
        TaskWorkflowState::Waiting => "[~]",
        TaskWorkflowState::Blocked => "[=]",
        TaskWorkflowState::Done => "[o]",
        TaskWorkflowState::Canceled => "[x]",
        TaskWorkflowState::Conflicted => "[ox]",
    }
}

pub(crate) fn event_labels(entry: &DocumentEntry) -> HashMap<(usize, usize), FoldLabel> {
    let Some(current) = &entry.current else {
        return HashMap::new();
    };
    current
        .output
        .events
        .events
        .iter()
        .filter_map(|event| {
            let time = event_time_label(&event)?;
            let indent = line_indent(&entry.parsed.source, event.range.start);
            let marker = &entry.parsed.source[event.range.start + 1..event.range.start + 2];
            let title = if event.title.is_empty() {
                "Untitled event"
            } else {
                &event.title
            };
            Some((
                (event.range.start, event.range.end),
                FoldLabel {
                    text: format!("{indent}`{marker} {time}| {title}"),
                },
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

fn line_indent(source: &str, range_start: usize) -> &str {
    let line_start = source[..range_start]
        .rfind('\n')
        .map_or(0, |newline| newline + 1);
    &source[line_start..range_start]
}

pub(crate) fn ranges(
    source: &str,
    document: &Document,
    limit: Option<usize>,
    labels: Option<&HashMap<(usize, usize), FoldLabel>>,
    line_folding_only: bool,
) -> Vec<FoldingRange> {
    let positions = PositionIndex::new(source);
    let headings = analyze_recovered_headings(document);
    let mut byte_ranges = Vec::new();
    let mut pending_headings = headings.headings.iter().collect::<Vec<_>>();
    while let Some(heading) = pending_headings.pop() {
        byte_ranges.push((
            heading.section_range.clone(),
            heading.section_range.clone(),
            false,
        ));
        pending_headings.extend(heading.children.iter().rev());
    }

    collect_block_ranges(&document.blocks, &mut byte_ranges);

    byte_ranges.sort_by_key(|(range, _, _)| (range.start, std::cmp::Reverse(range.end)));
    byte_ranges.dedup_by(|(left, _, _), (right, _, _)| left == right);
    let mut ranges = byte_ranges
        .into_iter()
        .filter_map(|(range, label_range, include_trailing_blank)| {
            let label =
                labels.and_then(|table| table.get(&(label_range.start, label_range.end)).cloned());
            line_range(
                source,
                &positions,
                &range,
                label.as_ref(),
                include_trailing_blank,
                line_folding_only,
            )
        })
        .collect::<Vec<_>>();
    ranges.dedup();
    if let Some(limit) = limit {
        ranges.truncate(limit);
    }
    ranges
}

fn collect_block_ranges(
    blocks: &[Block],
    byte_ranges: &mut Vec<(std::ops::Range<usize>, std::ops::Range<usize>, bool)>,
) {
    for (index, block) in blocks.iter().enumerate() {
        match block {
            Block::Parsed(parsed) => {
                if parsed.mark.is_some() || !parsed.children.is_empty() {
                    let include_trailing_blank = parsed.mark.as_ref().is_some_and(|mark| {
                        !is_heading_marker(&mark.marker)
                        && blocks.get(index + 1).is_some_and(|next| {
                            matches!(next, Block::Parsed(next) if next.mark.as_ref().is_some_and(|next_mark| next_mark.marker == mark.marker))
                        })
                    });
                    byte_ranges.push((
                        parsed.range.clone(),
                        parsed.range.clone(),
                        include_trailing_blank,
                    ));
                }
                collect_block_ranges(&parsed.children, byte_ranges);
            }
            Block::Verbatim(verbatim) => {
                byte_ranges.push((verbatim.range.clone(), verbatim.range.clone(), false));
            }
        }
    }
}

fn is_heading_marker(marker: &str) -> bool {
    (1..=6).contains(&marker.len()) && marker.bytes().all(|byte| byte == b'#')
}

fn line_range(
    source: &str,
    positions: &PositionIndex<'_>,
    range: &std::ops::Range<usize>,
    label: Option<&FoldLabel>,
    include_trailing_blank: bool,
    line_folding_only: bool,
) -> Option<FoldingRange> {
    let trimmed_end = range.start
        + source[range.clone()]
            .trim_end_matches(char::is_whitespace)
            .len();
    let content_end = if include_trailing_blank {
        include_one_trailing_blank_line(source, trimmed_end)
    } else {
        trimmed_end
    };
    let range = positions.byte_range_to_lsp(&(range.start..content_end));
    let end_line = if range.end.character == 0 && range.end.line > range.start.line {
        range.end.line - 1
    } else {
        range.end.line
    };
    if end_line == range.start.line && label.is_none() {
        return None;
    }
    let same_line = end_line == range.start.line;
    Some(FoldingRange {
        start_line: range.start.line,
        start_character: (same_line && !line_folding_only).then_some(range.start.character),
        end_line,
        end_character: (same_line && !line_folding_only).then_some(range.end.character),
        kind: None,
        collapsed_text: label.map(|label| label.text.clone()),
    })
}

fn include_one_trailing_blank_line(source: &str, mut end: usize) -> usize {
    let content_end = end;
    let mut line_endings = 0;
    for _ in 0..2 {
        if end >= source.len() {
            break;
        }
        if source[end..].starts_with("\r\n") {
            end += 2;
            line_endings += 1;
        } else if source.as_bytes().get(end) == Some(&b'\n') {
            end += 1;
            line_endings += 1;
        } else {
            break;
        }
    }
    if line_endings == 2 {
        end
    } else {
        content_end
    }
}

#[cfg(test)]
mod tests {
    use super::{include_one_trailing_blank_line, is_heading_marker, single_line_label};

    #[test]
    fn normalizes_and_truncates_fold_labels_on_character_boundaries() {
        assert_eq!(
            single_line_label("  Project\n  Overview  ", 80),
            "Project Overview"
        );
        assert_eq!(single_line_label("项目项目项目项目", 6), "项目项...");
    }

    #[test]
    fn includes_exactly_one_trailing_blank_line_for_lf_and_crlf() {
        assert_eq!(include_one_trailing_blank_line("body\nnext", 4), 4);
        assert_eq!(include_one_trailing_blank_line("body\n\nnext", 4), 6);
        assert_eq!(include_one_trailing_blank_line("body\n\n\nnext", 4), 6);
        assert_eq!(include_one_trailing_blank_line("body\r\n\r\nnext", 4), 8);
        assert_eq!(include_one_trailing_blank_line("body", 4), 4);
    }

    #[test]
    fn recognizes_only_standard_heading_markers() {
        for marker in ["#", "##", "######"] {
            assert!(is_heading_marker(marker));
        }
        for marker in ["", "#######", "#note", "task"] {
            assert!(!is_heading_marker(marker));
        }
    }
}
