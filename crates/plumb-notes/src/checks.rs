//! Terminal projection of optional workspace check results.
use super::{display_path, line_column, LoadedWorkspace};
use plumb_workspace::{AgendaIssue, AgendaLocation, CheckSettings, TimelineSegment};
use std::fmt::Write as _;

pub(super) fn render(
    loaded: &LoadedWorkspace,
    settings: &CheckSettings,
) -> Result<(String, u8), String> {
    let mut output = String::new();
    let mut status = 0;
    if settings.event_category.enabled {
        let report = loaded
            .workspace
            .check_event_categories(&loaded.root, loaded.now, None)?;
        for source in &report.missing {
            diagnostic(
                &mut output,
                loaded,
                source,
                "check.event-category.missing",
                "event has an uncategorized accounting share",
            )?;
        }
        render_issues(&mut output, loaded, &report.issues)?;
        status = if !report.complete {
            2
        } else if !report.missing.is_empty() {
            1
        } else {
            0
        };
        if !report.complete {
            output.push_str(
                "error[check.incomplete]: event-category could not be checked completely\n",
            );
        }
    }
    if settings.event_timeline.enabled {
        let report = loaded
            .workspace
            .check_event_timeline(&loaded.root, loaded.now)?;
        render_segments(&mut output, loaded, "gap", &report.gaps)?;
        render_segments(&mut output, loaded, "overlap", &report.overlaps)?;
        render_issues(&mut output, loaded, &report.issues)?;
        status = status.max(if !report.complete {
            2
        } else if !report.passed() {
            1
        } else {
            0
        });
        if !report.complete {
            output.push_str(
                "error[check.incomplete]: event-timeline could not be checked completely\n",
            );
        }
    }
    Ok((output, status))
}

fn source_position(loaded: &LoadedWorkspace, location: &AgendaLocation) -> Result<String, String> {
    let source = loaded
        .source(&location.path)
        .ok_or_else(|| format!("missing source snapshot: {}", location.path.display()))?;
    let (line, column) = line_column(source, location.range.start);
    let (end_line, end_column) = line_column(source, location.range.end);
    Ok(format!(
        "{}:{line}:{column}..{end_line}:{end_column}",
        display_path(&loaded.root, &location.path)
    ))
}

fn diagnostic(
    output: &mut String,
    loaded: &LoadedWorkspace,
    location: &AgendaLocation,
    code: &str,
    message: &str,
) -> Result<(), String> {
    writeln!(
        output,
        "{}: error[{code}]: {message}",
        source_position(loaded, location)?
    )
    .unwrap();
    Ok(())
}

fn render_issues(
    output: &mut String,
    loaded: &LoadedWorkspace,
    issues: &[AgendaIssue],
) -> Result<(), String> {
    for issue in issues {
        diagnostic(output, loaded, &issue.source, &issue.code, &issue.message)?;
    }
    Ok(())
}

fn render_segments(
    output: &mut String,
    loaded: &LoadedWorkspace,
    kind: &str,
    segments: &[TimelineSegment],
) -> Result<(), String> {
    for segment in segments {
        let (source, related) = segment
            .events
            .split_first()
            .ok_or_else(|| format!("timeline {kind} has no source locations"))?;
        let code = format!("check.event-timeline.{kind}");
        diagnostic(
            output,
            loaded,
            source,
            &code,
            &format!(
                "{} -- {}",
                segment.start.to_rfc3339(),
                segment.end.to_rfc3339()
            ),
        )?;
        for location in related {
            writeln!(
                output,
                "{}: note[{code}.related]: related event",
                source_position(loaded, location)?
            )
            .unwrap();
        }
    }
    Ok(())
}
