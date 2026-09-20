use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, FixedOffset, Local};
use lsp_types::FoldingRange;
#[cfg(test)]
use plumb_semantics::analyze_recovered_headings;
use plumb_semantics::{EventRecordView, MetadataValue, TaskRecord, TaskState};
#[cfg(test)]
use plumb_syntax::Document;
use plumb_syntax::{Block, GreenDocument};
use plumb_workspace::{DocumentEntry, TaskWorkflowState, Workspace, WorkspaceQueryError};

use crate::position::PositionIndex;

/// Prove unchanged structure conservatively without rebuilding ranges or labels.
pub(crate) fn structural_inputs_changed(previous: &GreenDocument, current: &GreenDocument) -> bool {
    let old = previous.shards();
    let new = current.shards();
    if old.len() != new.len() {
        return true;
    }
    old.zip(new).any(|(old, new)| {
        if old.offset() != new.offset() {
            return true;
        }
        if std::sync::Arc::ptr_eq(old.shard(), new.shard()) {
            return false;
        }
        let old = old.shard().parsed();
        let new = new.shard().parsed();
        let shape = |(offset, ch): (usize, char)| {
            let whitespace = match ch {
                '\r' => 1,
                '\n' => 2,
                ch if ch.is_whitespace() => 3,
                _ => 0,
            };
            (offset, ch.len_utf16(), whitespace)
        };
        if old.source.len() != new.source.len()
            || !old
                .source
                .char_indices()
                .map(shape)
                .eq(new.source.char_indices().map(shape))
        {
            return true;
        }
        let mut pending = vec![(&old.syntax.blocks, &new.syntax.blocks)];
        while let Some((old, new)) = pending.pop() {
            if old.len() != new.len() {
                return true;
            }
            for (old, new) in old.iter().zip(new) {
                match (old, new) {
                    (Block::Parsed(old), Block::Parsed(new))
                        if old.range == new.range
                            && old.mark.as_ref().map(|mark| &mark.marker)
                                == new.mark.as_ref().map(|mark| &mark.marker) =>
                    {
                        pending.push((&old.children, &new.children));
                    }
                    (Block::Verbatim(old), Block::Verbatim(new)) if old.range == new.range => {}
                    _ => return true,
                }
            }
        }
        false
    })
}

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
        .and_then(|current| current.output.metadata().metadata.as_ref())
    else {
        return HashMap::new();
    };
    let source = entry.parsed.source();
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
        .tasks()
        .tasks
        .iter()
        .filter_map(|task| {
            let state = match task_label_state(workspace, path, &task, now, index_complete) {
                Ok(Some(state)) => state,
                Ok(None) => return None,
                Err(error) => {
                    tracing::error!(%error, "task fold label query failed");
                    return None;
                }
            };
            let source = entry.parsed.source();
            let indent = line_indent(source, task.range.start);
            let marker = &source[task.range.start + 1..task.range.start + 2];
            let title = if task.title.is_empty() {
                "Untitled task"
            } else {
                &task.title
            };
            // Unfocused tasks keep their existing label byte-for-byte; a
            // focused task only gains a short marker next to the state symbol.
            let focus_marker = if task.is_focused() { "* " } else { "" };
            Some((
                (task.range.start, task.range.end),
                FoldLabel {
                    text: format!(
                        "{indent}`{marker} {:<5}{focus_marker}{title}",
                        task_state_symbol(state)
                    ),
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
        let state = workspace.task_workflow_state(path, task, now)?;
        if state.is_complete() {
            return Ok(Some(state.value.0));
        }
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
        .events()
        .events
        .views()
        .filter_map(|event| {
            let time = event_time_label(event)?;
            let range = event.range();
            let source = entry.parsed.source();
            let indent = line_indent(source, range.start);
            let marker = &source[range.start + 1..range.start + 2];
            let title = if event.title().is_empty() {
                "Untitled event"
            } else {
                event.title()
            };
            Some((
                (range.start, range.end),
                FoldLabel {
                    text: format!("{indent}`{marker} {time} {title}"),
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
fn event_time_label(event: EventRecordView<'_>) -> Option<String> {
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

#[cfg(test)]
pub(crate) fn ranges(
    source: &str,
    document: &Document,
    limit: Option<usize>,
    labels: Option<&HashMap<(usize, usize), FoldLabel>>,
    line_folding_only: bool,
) -> Vec<FoldingRange> {
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

    finish_ranges(source, byte_ranges, limit, labels, line_folding_only)
}

pub(crate) fn green_ranges(
    source: &str,
    document: &GreenDocument,
    limit: Option<usize>,
    labels: Option<&HashMap<(usize, usize), FoldLabel>>,
    line_folding_only: bool,
) -> Vec<FoldingRange> {
    let mut byte_ranges = Vec::new();
    let mut headings = Vec::new();
    let mut shards = document.shards().peekable();
    while let Some(view) = shards.next() {
        let following_marker = shards
            .peek()
            .and_then(|next| top_level_marker(&next.shard().parsed().syntax.blocks));
        let mut local_ranges = Vec::new();
        collect_block_ranges_with_following(
            &view.shard().parsed().syntax.blocks,
            following_marker,
            &mut local_ranges,
        );
        byte_ranges.extend(
            local_ranges
                .into_iter()
                .map(|(mut range, mut label, trailing)| {
                    shift_range(&mut range, view.offset());
                    shift_range(&mut label, view.offset());
                    (range, label, trailing)
                }),
        );
        collect_heading_facts(
            &view.shard().parsed().syntax.blocks,
            view.offset(),
            &mut headings,
        );
    }
    let mut next_by_level = [None; 6];
    for (range, level) in headings.into_iter().rev() {
        let end = next_by_level[..usize::from(level)]
            .iter()
            .flatten()
            .copied()
            .min()
            .unwrap_or(source.len());
        next_by_level[usize::from(level - 1)] = Some(range.start);
        byte_ranges.push((range.start..end, range, false));
    }

    finish_ranges(source, byte_ranges, limit, labels, line_folding_only)
}

fn finish_ranges(
    source: &str,
    mut byte_ranges: Vec<(std::ops::Range<usize>, std::ops::Range<usize>, bool)>,
    limit: Option<usize>,
    labels: Option<&HashMap<(usize, usize), FoldLabel>>,
    line_folding_only: bool,
) -> Vec<FoldingRange> {
    let positions = PositionIndex::new(source);

    byte_ranges.sort_by_key(|(range, _, _)| (range.start, std::cmp::Reverse(range.end)));
    byte_ranges.dedup_by(|(left, _, _), (right, _, _)| left == right);
    let mut ranges = byte_ranges
        .into_iter()
        .filter_map(|(range, label_range, include_trailing_blank)| {
            let label = labels.and_then(|table| table.get(&(label_range.start, label_range.end)));
            line_range(
                source,
                &positions,
                &range,
                label,
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
    collect_block_ranges_with_following(blocks, None, byte_ranges);
}

fn collect_block_ranges_with_following(
    blocks: &[Block],
    following_marker: Option<&str>,
    byte_ranges: &mut Vec<(std::ops::Range<usize>, std::ops::Range<usize>, bool)>,
) {
    for (index, block) in blocks.iter().enumerate() {
        match block {
            Block::Parsed(parsed) => {
                if parsed.mark.is_some() || !parsed.children.is_empty() {
                    let include_trailing_blank = parsed.mark.as_ref().is_some_and(|mark| {
                        !is_heading_marker(&mark.marker)
                            && blocks.get(index + 1).map_or_else(
                                || following_marker == Some(mark.marker.as_str()),
                                |next| matches!(next, Block::Parsed(next) if next.mark.as_ref().is_some_and(|next_mark| next_mark.marker == mark.marker)),
                            )
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

fn top_level_marker(blocks: &[Block]) -> Option<&str> {
    let Block::Parsed(block) = blocks.first()? else {
        return None;
    };
    block.mark.as_ref().map(|mark| mark.marker.as_str())
}

fn collect_heading_facts(
    blocks: &[Block],
    offset: usize,
    output: &mut Vec<(std::ops::Range<usize>, u8)>,
) {
    for block in blocks {
        let Block::Parsed(block) = block else {
            continue;
        };
        if let Some(marker) = block.mark.as_ref().map(|mark| mark.marker.as_str()) {
            if is_heading_marker(marker) {
                output.push((
                    block.range.start + offset..block.range.end + offset,
                    marker.len() as u8,
                ));
            }
        }
        collect_heading_facts(&block.children, offset, output);
    }
}

fn shift_range(range: &mut std::ops::Range<usize>, offset: usize) {
    range.start += offset;
    range.end += offset;
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
    use super::{
        green_ranges, include_one_trailing_blank_line, is_heading_marker, ranges, single_line_label,
    };

    #[test]
    #[ignore = "manual structural invalidation versus folding projection profile"]
    fn profile_folding_invalidation() {
        let source = "`note See `->{foo.plumb}\n `child Body\n\n".repeat(2000);
        let old = plumb_syntax::GreenDocument::parse(&source);
        let changed = source.replacen("foo.plumb", "bar.plumb", 1);
        let new = old.reparse(&changed).document;
        for project in [false, true] {
            let started = std::time::Instant::now();
            for _ in 0..100 {
                if project {
                    std::hint::black_box(green_ranges(&changed, &new, None, None, true));
                } else {
                    assert!(!std::hint::black_box(super::structural_inputs_changed(
                        &old, &new
                    )));
                }
            }
            eprintln!(
                "folding project={project}, 2000 owners: {:?}/call",
                started.elapsed() / 100
            );
        }
        let mut metadata = String::new();
        for index in 0..2000 {
            metadata.push_str(&format!("`= field-{index} metadata scalar value\n"));
        }
        metadata.push_str("\nSee `->{foo.plumb}\n");
        let mut old_workspace = plumb_workspace::Workspace::new();
        let mut new_workspace = plumb_workspace::Workspace::new();
        let old = old_workspace
            .open_document("metadata.plumb", 1, &metadata)
            .current
            .as_ref()
            .unwrap();
        let new = new_workspace
            .open_document(
                "metadata.plumb",
                2,
                metadata.replace("foo.plumb", "bar.plumb"),
            )
            .current
            .as_ref()
            .unwrap();
        for labels in [false, true] {
            let started = std::time::Instant::now();
            for _ in 0..100 {
                assert!(!std::hint::black_box(
                    labels && old.output.metadata() != new.output.metadata()
                ));
            }
            eprintln!(
                "metadata comparison labels={labels}, 2000 entries: {:?}/call",
                started.elapsed() / 100
            );
        }
    }

    #[test]
    fn unchanged_structural_inputs_preserve_folding_projection() {
        for source in [
            "`note First\r\n\r\n`note Second\r\n",
            "`# Heading\n\n`note Body\n `child Child\n\n`# Next\n",
            "`rust\"\n code\n \n\n`rust\"\n tail\n",
            "`note See `->{target.plumb}\n `child Body\n",
        ] {
            let old = plumb_syntax::GreenDocument::parse(source);
            for (offset, ch) in source.char_indices() {
                for replacement in ["a", " ", "\r", "\n", "😀", ""] {
                    let mut changed = source.to_owned();
                    changed.replace_range(offset..offset + ch.len_utf8(), replacement);
                    let fresh = plumb_syntax::GreenDocument::parse(&changed);
                    if !fresh.is_valid() {
                        continue;
                    }
                    let incremental = old.reparse(&changed).document;
                    for current in [&fresh, &incremental] {
                        if !super::structural_inputs_changed(&old, current) {
                            for line_only in [true, false] {
                                assert_eq!(
                                    green_ranges(source, &old, None, None, line_only),
                                    green_ranges(&changed, current, None, None, line_only),
                                    "{changed}"
                                );
                            }
                        }
                    }
                }
            }
        }
        let old = plumb_syntax::GreenDocument::parse("`note First\r\n\r\n`note Second\r\n");
        let new = plumb_syntax::GreenDocument::parse("`note First \n\r\n`note Second\r\n");
        assert!(super::structural_inputs_changed(&old, &new));
        assert_ne!(
            green_ranges(old.source(), &old, None, None, true),
            green_ranges(new.source(), &new, None, None, true)
        );
    }

    #[test]
    #[ignore = "manual folding stage profile; run with --release --ignored --nocapture"]
    fn profile_metadata_folding_stages() {
        use std::{hint::black_box, time::Instant};

        let background = std::env::var_os("PLUMB_PROFILE_BACKGROUND_SEMANTICS").is_some();
        let runtime = background.then(|| {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
        });
        let path = std::path::Path::new("/tmp/plumb-decoration-profile.plumb");
        let mut source = "`= date 2026-09-05\n`= timezone +08:00\n\n".to_owned();
        for index in 0..2000 {
            source.push_str(&format!("`- 14:30--15:15 Event {index}\n `+ event\n\n"));
        }
        source.push_str("`- Completed\n `+ task\n `= done 2026-09-05T00:00:00Z\n");
        let mut workspace = plumb_workspace::Workspace::new();
        workspace.open_document(path, 1, source.clone());
        let mut samples = [const { Vec::new() }; 6];
        for iteration in 0..55 {
            let date = if iteration % 2 == 0 {
                "2026-09-06"
            } else {
                "2026-09-05"
            };
            let changed = source.replacen("2026-09-05", date, 1);
            let start = Instant::now();
            let pending = workspace
                .begin_document_revision_with_change(
                    path,
                    iteration + 2,
                    changed,
                    Some(plumb_syntax::SourceChange {
                        old_range: 8..18,
                        new_range: 8..18,
                    }),
                )
                .unwrap();
            let parse = start.elapsed();
            let start = Instant::now();
            let analysis = if let Some(runtime) = &runtime {
                runtime.block_on(async {
                    tokio::task::spawn_blocking(move || pending.analyze())
                        .await
                        .unwrap()
                })
            } else {
                pending.analyze()
            };
            let semantics = start.elapsed();
            let start = Instant::now();
            assert!(workspace.install_document_analysis(analysis));
            let install = start.elapsed();
            let entry = workspace.get(path).unwrap();
            let start = Instant::now();
            let labels = super::collapsed_text_labels(&workspace, path, entry, true);
            let label_time = start.elapsed();
            let start = Instant::now();
            let folds = green_ranges(
                entry.parsed.source(),
                entry.parsed.green(),
                None,
                Some(&labels),
                true,
            );
            let bytes = serde_json::to_vec(&folds).unwrap();
            let projection = start.elapsed();
            let start = Instant::now();
            let context = workspace.diagnostic_context().unwrap();
            let diagnostics = workspace.diagnostics_with_context(path, &context).unwrap();
            let diagnostic_time = start.elapsed();
            assert!(diagnostics.value.is_empty());
            let expected = format!("`- {date}T14:30--15:15 Event ");
            assert_eq!(
                folds
                    .iter()
                    .filter(|fold| fold
                        .collapsed_text
                        .as_ref()
                        .is_some_and(|label| label.starts_with(&expected)))
                    .count(),
                2000
            );
            black_box(bytes);
            if iteration >= 5 {
                for (samples, elapsed) in samples.iter_mut().zip([
                    parse,
                    semantics,
                    install,
                    label_time,
                    projection,
                    diagnostic_time,
                ]) {
                    samples.push(elapsed);
                }
            }
        }
        for (name, samples) in [
            "parse",
            "semantics",
            "install",
            "labels",
            "ranges+json",
            "diagnostics",
        ]
        .into_iter()
        .zip(&mut samples)
        {
            samples.sort();
            eprintln!(
                "metadata folding stage {name}: background={background} events=2000 warmup=5 samples=50 p50={:?} p95={:?}",
                samples[24], samples[47]
            );
        }
    }

    #[test]
    fn borrowed_event_labels_project_current_ranges_after_prefix_insertion() {
        let source = "`= date 2026-09-05\r\n`= timezone +08:00\r\n\r\n`- 14:30 Point\r\n `+ event\r\n\r\n `- 23:30--00:30 Nested\r\n  `+ event\r\n\r\n`. 14:30--2026-09-07T15:30 Long\r\n `+ event\r\n\r\n`- invalid Invalid\r\n `+ event\r\n";
        let path = std::path::Path::new("events.plumb");
        let mut workspace = plumb_workspace::Workspace::new();
        workspace.insert(path, 1, source);
        let prefix = "Prelude \u{1f600}\r\n\r\n";
        let changed = format!("{prefix}{source}");
        let analysis = workspace
            .begin_document_revision_with_change(
                path,
                2,
                changed.clone(),
                Some(plumb_syntax::SourceChange {
                    old_range: 0..0,
                    new_range: 0..prefix.len(),
                }),
            )
            .unwrap()
            .analyze();
        assert!(workspace.install_document_analysis(analysis));
        let entry = workspace.get(path).unwrap();
        let events = &entry.current.as_ref().unwrap().output.events().events;
        let labels = super::event_labels(entry);
        let expected = [
            Some("`- 2026-09-05T14:30 Point"),
            Some(" `- 2026-09-05T23:30--00:30 Nested"),
            Some("`. 2026-09-05T14:30--2026-09-07T15:30 Long"),
            None,
        ];
        assert_eq!(events.len(), expected.len());
        assert_eq!(labels.len(), 3);
        for ((view, owned), expected) in events.views().zip(events.iter()).zip(expected) {
            assert_eq!(view.at_datetime(), owned.at_datetime());
            assert_eq!(view.start_datetime(), owned.start_datetime());
            assert_eq!(view.end_datetime(), owned.end_datetime());
            assert_eq!(view.range(), owned.range);
            assert!(changed[owned.range.clone()].starts_with('`'));
            assert_eq!(
                labels
                    .get(&(owned.range.start, owned.range.end))
                    .map(|label| label.text.as_str()),
                expected,
            );
        }
    }

    #[test]
    fn focused_task_label_keeps_the_state_symbol_and_adds_a_marker() {
        let mut workspace = plumb_workspace::Workspace::new();
        let path = std::path::Path::new("focus-labels.plumb");
        workspace.open_document(
            path,
            1,
            "`- Focused ready\n `+ task\n `= focused 2026-09-20T09:00:00Z--\n\n`- Plain ready\n `+ task\n",
        );
        let labels = super::task_labels(&workspace, path, workspace.get(path).unwrap(), false)
            .into_iter()
            .map(|(_, label)| label.text)
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            vec!["`- [ ]  * Focused ready", "`- [ ]  Plain ready"]
        );
    }

    #[test]
    fn partial_workspace_keeps_locally_determined_task_labels() {
        let mut workspace = plumb_workspace::Workspace::new();
        let path = std::path::Path::new("labels.plumb");
        workspace.open_document(path, 1, "`- Ready\n `+ task\n\n`- Waiting\n `+ task\n `= wait 2099-01-01T00:00:00Z\n `= depends b.plumb#b\n\n`- Done\n `+ task\n `= done 2026-09-07T00:00:00Z\n\n`- Canceled\n `+ task\n `= canceled 2026-09-07T00:00:00Z\n\n`- Conflicted\n `+ task\n `= done 2026-09-07T00:00:00Z\n `= canceled 2026-09-07T00:00:00Z\n\n`- Blocked\n `+ task\n `= depends b.plumb#b c.plumb#c\n\n`- Closed dependency\n `+ task\n `= depends d.plumb#d\n\n`- Unknown\n `+ task\n `= depends b.plumb#b\n");
        workspace.open_document("c.plumb", 1, "`- C\n `+ task\n `@ c\n");
        workspace.open_document(
            "d.plumb",
            1,
            "`- D\n `+ task\n `@ d\n `= done 2026-09-07T00:00:00Z\n",
        );
        let _pending = workspace
            .begin_document_revision("b.plumb", 1, "`- B\n `+ task\n `@ b\n")
            .unwrap();
        let mut expected = vec![
            "`- [ ]  Ready",
            "`- [~]  Waiting",
            "`- [o]  Done",
            "`- [x]  Canceled",
            "`- [ox] Conflicted",
            "`- [=]  Blocked",
            "`- [ ]  Closed dependency",
        ];
        expected.sort();
        for index_complete in [false, true] {
            let labels = super::task_labels(
                &workspace,
                path,
                workspace.get(path).unwrap(),
                index_complete,
            );
            let mut actual = labels
                .values()
                .map(|label| label.text.as_str())
                .collect::<Vec<_>>();
            actual.sort();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn pending_dependency_does_not_become_a_ready_fold_label() {
        let mut workspace = plumb_workspace::Workspace::new();
        let path = std::path::Path::new("a.plumb");
        workspace.open_document(path, 1, "`- A\n `+ task\n `= depends b.plumb#b\n");
        let dependency = "`- B\n `+ task\n `@ b\n";
        workspace.open_document("b.plumb", 1, dependency);
        let labels = super::task_labels(&workspace, path, workspace.get(path).unwrap(), true);
        assert_eq!(labels.values().next().unwrap().text, "`- [=]  A");
        let pending = workspace
            .begin_document_revision("b.plumb", 2, dependency)
            .unwrap();
        assert!(
            super::task_labels(&workspace, path, workspace.get(path).unwrap(), true).is_empty()
        );
        assert!(workspace.install_document_analysis(pending.analyze()));
        let labels = super::task_labels(&workspace, path, workspace.get(path).unwrap(), true);
        assert_eq!(labels.values().next().unwrap().text, "`- [=]  A");
    }

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

    #[test]
    fn green_folding_matches_materialized_recovered_documents() {
        for source in [
            "`# One\n\n`## Two\n\n`# Three\n",
            "`- One\n `- Nested\n\n`- Two\n\n`. Ordered\n",
            "`note Parent\n `rust\"\n  code\n\n`note Sibling\n",
            "before {unclosed\n\n`note recovered\n",
            "`note First\r\n\r\n`note Second\r\n",
        ] {
            let parsed = plumb_syntax::parse(source);
            let green = plumb_syntax::GreenDocument::parse(source);
            for line_folding_only in [false, true] {
                assert_eq!(
                    green_ranges(source, &green, None, None, line_folding_only),
                    ranges(
                        source,
                        parsed.recovered_syntax(),
                        None,
                        None,
                        line_folding_only,
                    ),
                    "{source:?}"
                );
            }
        }
    }
}
