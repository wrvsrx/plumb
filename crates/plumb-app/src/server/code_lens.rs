use plumb_semantics::{AnchorKind, DocumentOutput};
use plumb_syntax::GreenDocument;
use std::ops::Range;

use crate::position::{position_geometry_change_bound, PositionIndex};

fn source_ranges(output: &DocumentOutput) -> impl Iterator<Item = Range<usize>> + '_ {
    output
        .anchors()
        .views()
        .flat_map(|anchor| {
            let lens_range = if anchor.kind() == AnchorKind::Inline {
                anchor.id_range()
            } else {
                let start = anchor.owner_range().start;
                start..start
            };
            std::iter::once(lens_range).chain(anchor.category_declaration_ranges())
        })
        .chain(
            output
                .links()
                .views()
                .flat_map(|link| [link.selection_range(), link.target_source_range()]),
        )
        .chain(output.tasks().tasks.views().flat_map(|task| {
            let start = task.range().start;
            std::iter::once(start..start).chain(task.reference_ranges())
        }))
        .chain(output.events().events.views().flat_map(|event| {
            let start = event.range().start;
            [start..start, event.selection_range()]
                .into_iter()
                .chain(event.task_reference_ranges())
                .chain(event.category_declaration_ranges())
        }))
        .chain(output.document_category().declarations)
}

pub(super) fn positions_changed(previous: &DocumentOutput, current: &GreenDocument) -> bool {
    let mut ranges = source_ranges(previous).peekable();
    if ranges.peek().is_none() {
        return false;
    }
    let Some(bound) = position_geometry_change_bound(previous.syntax(), current) else {
        return false;
    };
    let mut positions = None;
    ranges.filter(|range| range.end > bound).any(|range| {
        let (old, new) = positions.get_or_insert_with(|| {
            (
                PositionIndex::new(previous.syntax().source()),
                PositionIndex::new(current.source()),
            )
        });
        old.byte_range_to_lsp(&range) != new.byte_range_to_lsp(&range)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn output(source: &str) -> DocumentOutput {
        let syntax = Arc::new(GreenDocument::parse(source));
        plumb_semantics::analyze_green_document(syntax.valid_syntax().unwrap(), Arc::clone(&syntax))
            .unwrap()
    }

    #[test]
    fn duration_display_rounds_only_final_totals_and_keeps_long_hours() {
        for (seconds, expected) in [
            (0.0, "0s"),
            (1.0 / 3.0, "<1s"),
            (59.6, "1m"),
            (3600.0, "1h"),
            (4801.0, "1h 20m 1s"),
            (90000.0, "25h"),
        ] {
            assert_eq!(format_duration(seconds), expected);
        }
    }

    #[test]
    fn duration_lenses_project_utf16_owner_positions_and_keep_reference_lenses() {
        use std::path::Path;
        let mut workspace = super::super::Workspace::new();
        let source = "😀\r\n\r\n`- Task\r\n `+ task\r\n `@ task\r\n`- 2026-10-01T10:00:00Z--11:30 `->{#task}\r\n `+ event\r\n`- 2026-10-01T12:00:00Z-- Running\r\n `+ event\r\n`- 2026-10-01T13:00:00Z Point\r\n `+ event\r\n`- invalid Bad\r\n `+ event\r\n";
        workspace.insert("/notes/day.plumb", 0, source);
        let result = lenses(&workspace, Path::new("/notes/day.plumb"))
            .unwrap()
            .unwrap();
        let titles = result
            .iter()
            .map(|l| l.command.as_ref().unwrap().title.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            &titles[2..],
            [
                "total 1h 30m",
                "duration 1h 30m",
                "ongoing",
                "duration unavailable"
            ]
        );
        assert_eq!(result[2].range.start, lsp_types::Position::new(2, 0));
        assert_eq!(result[3].range.start, lsp_types::Position::new(5, 0));
        assert_eq!(result[4].range.start, lsp_types::Position::new(7, 0));
        assert_eq!(result[5].range.start, lsp_types::Position::new(11, 0));
        let command = result[3].command.as_ref().unwrap();
        assert_eq!(command.command, "plumb.showReferences");
        let locations = &command.arguments.as_ref().unwrap()[2];
        assert_eq!(locations[0]["range"]["start"]["line"], 5);
        assert_eq!(locations[0]["uri"], "file:///notes/day.plumb");
    }

    #[test]
    fn chinese_item_lens_counts_valid_events_despite_local_and_workspace_errors() {
        let mut workspace = super::super::Workspace::new();
        let source = "`= title test\n`= date 2026-10-03\n`= timezone +08:00\n\n`- 中文项\n `@ 中文项\n `= event-category daily\n`- 09:00--10:00 `->{#中文项}\n `+ event\n\n{broken\n";
        workspace.insert("/notes/test.plumb", 0, source);
        workspace.insert("/notes/bad.plumb", 0, "`- tomorrow Invalid\n `+ event\n");
        let result = lenses(&workspace, std::path::Path::new("/notes/test.plumb"))
            .unwrap()
            .unwrap();
        let total = result
            .iter()
            .find(|lens| lens.command.as_ref().unwrap().title == "total 1h")
            .unwrap();
        assert_eq!(total.range.start, lsp_types::Position::new(4, 0));
        assert!(workspace
            .document_local_diagnostics("/notes/test.plumb")
            .iter()
            .any(|d| d.code == "syntax.unclosed-inline-group"));
    }

    #[test]
    fn ordinary_item_duration_uses_total_label() {
        let mut workspace = super::super::Workspace::new();
        workspace.insert("/notes/items.plumb", 0, "😀\r\n`- Item\r\n `@ item\r\n");
        workspace.insert(
            "/notes/day.plumb",
            0,
            "`- 2026-10-01T10:00:00Z--10:20 `->{items.plumb#item}\n `+ event\n",
        );
        let result = lenses(&workspace, std::path::Path::new("/notes/items.plumb"))
            .unwrap()
            .unwrap();
        let total = result
            .iter()
            .find(|lens| lens.command.as_ref().unwrap().title == "total 20m")
            .unwrap();
        assert_eq!(total.range.start, lsp_types::Position::new(1, 0));
        let command = total.command.as_ref().unwrap();
        assert_eq!(command.command, "plumb.showReferences");
        assert_eq!(
            command.arguments.as_ref().unwrap()[2][0]["uri"],
            "file:///notes/day.plumb"
        );
    }

    #[test]
    fn duration_geometry_covers_owners_contributions_and_category_issue_locations() {
        for body in [
            "`- Task\n `+ task\n",
            "`- 2026-10-01T10:00:00Z--11:00 Work\n `+ event\n",
            "`= event-category {}\n",
            "`- Item\n `@ item\n `= event-category {}\n",
            "`- 2026-10-01T10:00:00Z--11:00 Work\n `+ event\n `= event-category {}\n",
        ] {
            let previous = output(&format!("AA\nBB\n\n{body}"));
            let current = output(&format!("ABCDEF\n{body}"));
            assert_eq!(
                previous.exported_semantic_summary(),
                current.exported_semantic_summary()
            );
            assert!(positions_changed(&previous, current.syntax()));
        }
    }

    #[test]
    fn reference_geometry_is_independent_of_semantic_byte_ranges() {
        for (old, new, changed) in [
            (
                "AA\nBB\n\n`# Target\n `@ target\n",
                "ABCDEF\n`# Target\n `@ target\n",
                true,
            ),
            ("`# AAAA\n `@ target\n", "`# 😀\n `@ target\n", false),
            (
                "AA\nBB\n\nSee `->{label target.plumb}\n",
                "ABCDEF\nSee `->{label target.plumb}\n",
                true,
            ),
            (
                "ABCD `->{label target.plumb}\n",
                "😀 `->{label target.plumb}\n",
                true,
            ),
            (
                "See `->{{`aaaa{label}} target.plumb}\n",
                "See `->{{`😀{label}} target.plumb}\n",
                true,
            ),
            ("`aaaa{label `@{id}}\n", "`😀{label `@{id}}\n", true),
            (
                "Old text\n\n`->{label target.plumb}\n",
                "New text\n\n`->{label target.plumb}\n",
                false,
            ),
            (
                "`->{label target.plumb}\n\nABCD\n",
                "`->{label target.plumb}\n\n😀\n",
                false,
            ),
            ("ABCD\n", "😀\n", false),
        ] {
            let previous = output(old);
            let current = output(new);
            assert_eq!(
                previous.exported_semantic_summary(),
                current.exported_semantic_summary(),
                "{old}"
            );
            assert_eq!(
                positions_changed(&previous, current.syntax()),
                changed,
                "{old}"
            );
            let incremental = previous.syntax().reparse(new).document;
            assert_eq!(positions_changed(&previous, &incremental), changed, "{old}");
        }
        for body in [
            "`- Task\n `+ task\n `= prev #previous\n `= depends #dependency\n",
            "`- 10:00 Event\n `+ event\n `= date 2026-09-07\n `= timezone +08:00\n `= tasks #task\n",
        ] {
            let old = format!("AA\nBB\n\n{body}");
            let new = format!("ABCDEF\n{body}");
            let previous = output(&old);
            let current = output(&new);
            assert_eq!(previous.exported_semantic_summary(), current.exported_semantic_summary());
            assert!(positions_changed(&previous, current.syntax()));
        }
    }

    #[test]
    #[ignore = "manual task reference-input comparison profile"]
    fn profile_task_reference_input_comparison() {
        let source = "`- Task\n `+ task\n `@ task\n `= wait 2099-01-01T00:00:00Z\n `= prev target.plumb#task\n `= depends target.plumb#task\n\n".repeat(2000);
        let previous = output(&source);
        let current = output(&source.replace("wait", "done"));
        let started = std::time::Instant::now();
        for _ in 0..1000 {
            assert!(std::hint::black_box(
                previous
                    .tasks()
                    .tasks
                    .views()
                    .zip(current.tasks().tasks.views())
                    .all(|(old, new)| old.reference_inputs_equal(new))
            ));
        }
        eprintln!(
            "task reference input comparison: {:?}/call, 2000 tasks with prev/depends",
            started.elapsed() / 1000
        );
    }

    #[test]
    #[ignore = "manual event reference-input comparison profile"]
    fn profile_event_reference_input_comparison() {
        let source = format!(
            "`= date 2026-09-07\n`= timezone +08:00\n\n{}",
            "`- 10:00 Event\n `+ event\n `= tasks target.plumb#task\n\n".repeat(2000)
        );
        let previous = output(&source);
        let current = output(&source.replace("09-07", "09-08"));
        let started = std::time::Instant::now();
        for _ in 0..1000 {
            assert!(std::hint::black_box(
                previous
                    .events()
                    .events
                    .views()
                    .zip(current.events().events.views())
                    .all(|(old, new)| old.reference_inputs_equal(new))
            ));
        }
        eprintln!(
            "event reference input comparison: {:?}/call, 2000 explicit-reference events",
            started.elapsed() / 1000
        );
    }

    #[test]
    #[ignore = "manual CodeLens geometry invalidation profile"]
    fn profile_reference_geometry_invalidation() {
        let source = format!(
            "AAAA\n\n{}",
            "See `->{label target.plumb#target}\n".repeat(20_000)
        );
        let previous = output(&source);
        for (name, prefix, changed) in [
            ("same_shape", "BBBB", false),
            ("changed_shape", "A\nBB", true),
        ] {
            let next = format!("{prefix}{}", &source[4..]);
            let current = previous.syntax().reparse(next).document;
            let started = std::time::Instant::now();
            for _ in 0..100 {
                assert_eq!(
                    std::hint::black_box(positions_changed(&previous, &current)),
                    changed
                );
            }
            eprintln!(
                "{name}: {:?}/call, {} bytes, 20000 references",
                started.elapsed() / 100,
                source.len()
            );
        }
    }
}

pub(super) fn lenses(
    workspace: &super::Workspace,
    path: &std::path::Path,
) -> Result<Option<Vec<lsp_types::CodeLens>>, async_lsp::ResponseError> {
    use super::{
        optional_decorative_query, reference_code_lens, workspace_query_response_error,
        QueryResult, ReferenceLocationCache,
    };
    use std::collections::HashSet;
    let Some(entry) = workspace.get(path) else {
        return Ok(None);
    };
    let Some(output) = entry.current.as_ref() else {
        return Ok(None);
    };
    let Ok(uri) = lsp_types::Url::from_file_path(&entry.path) else {
        return Ok(None);
    };
    let anchor_ids = output
        .output
        .anchors()
        .iter()
        .map(|anchor| anchor.id.value.clone())
        .collect::<HashSet<_>>();
    let Some(mut references) = optional_decorative_query(
        workspace
            .reverse_references_for_document(&entry.path, &anchor_ids)
            .and_then(QueryResult::require_complete),
    )
    .map_err(workspace_query_response_error)?
    else {
        return Ok(None);
    };
    let mut lenses = Vec::new();
    let mut location_cache = ReferenceLocationCache::new(workspace);
    let locations = references
        .document
        .into_iter()
        .filter_map(|reference| {
            location_cache.location(&reference.source_path, &reference.source_range)
        })
        .collect::<Vec<_>>();
    let count = locations.len();
    let title = if count == 1 {
        "1 file reference".to_string()
    } else {
        format!("{count} file references")
    };
    lenses.push(reference_code_lens(
        &uri,
        lsp_types::Range::default(),
        title,
        locations,
    ));
    lenses.extend(output.output.anchors().iter().filter_map(|anchor| {
        let locations = references
            .anchors
            .remove(&anchor.id.value)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|reference| {
                location_cache.location(&reference.source_path, &reference.source_range)
            })
            .collect::<Vec<_>>();
        let count = locations.len();
        let title = if count == 1 {
            "1 reference".to_string()
        } else {
            format!("{count} references")
        };
        let lens_range = if anchor.kind == AnchorKind::Inline {
            anchor.id.range.clone()
        } else {
            anchor.range.start..anchor.range.start
        };
        let range = location_cache.location(&entry.path, &lens_range)?.range;
        Some(reference_code_lens(&uri, range, title, locations))
    }));
    let durations = workspace
        .document_durations(&entry.path)
        .map_err(|message| {
            async_lsp::ResponseError::new(async_lsp::ErrorCode::INTERNAL_ERROR, message)
        })?;
    if !durations.is_complete() {
        return Ok(None);
    }
    for annotation in durations.value {
        let Some(range) = location_cache
            .location(&entry.path, &annotation.range)
            .map(|l| l.range)
        else {
            continue;
        };
        let title = duration_title(annotation.kind, annotation.value);
        let locations = annotation
            .sources
            .into_iter()
            .filter_map(|source| {
                location_cache.location(&source.path, &(source.range.start..source.range.end))
            })
            .collect();
        lenses.push(reference_code_lens(&uri, range, title, locations));
    }
    Ok(Some(lenses))
}

fn duration_title(
    kind: plumb_workspace::DurationKind,
    value: plumb_workspace::DurationValue,
) -> String {
    use plumb_workspace::{DurationKind, DurationValue};
    let prefix = if matches!(kind, DurationKind::Task | DurationKind::Item) {
        "total"
    } else {
        "duration"
    };
    match value {
        DurationValue::Seconds(seconds) => format!("{prefix} {}", format_duration(seconds)),
        DurationValue::Ongoing => "ongoing".into(),
        DurationValue::Unavailable => format!("{prefix} unavailable"),
        DurationValue::Incomplete => format!("{prefix} unavailable (incomplete)"),
    }
}

fn format_duration(seconds: f64) -> String {
    if seconds > 0.0 && seconds < 1.0 {
        return "<1s".into();
    }
    let seconds = seconds.round() as u64;
    let mut parts = Vec::new();
    if seconds >= 3600 {
        parts.push(format!("{}h", seconds / 3600));
    }
    if seconds % 3600 >= 60 {
        parts.push(format!("{}m", seconds % 3600 / 60));
    }
    if seconds % 60 != 0 || parts.is_empty() {
        parts.push(format!("{}s", seconds % 60));
    }
    parts.join(" ")
}
