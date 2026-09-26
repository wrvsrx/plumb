//! Terminal projection of optional workspace diagnostics.
use super::{display_path, line_column, LoadedWorkspace};
use plumb_workspace::{AgendaLocation, DiagnosticSettings};
use std::fmt::Write as _;

pub(super) fn render(
    loaded: &LoadedWorkspace,
    settings: &DiagnosticSettings,
) -> Result<(String, u8), String> {
    let report = loaded
        .workspace
        .policy_diagnostics(&loaded.root, &[], loaded.now, settings)?;
    let mut output = String::new();
    for diagnostic in &report.diagnostics {
        writeln!(
            output,
            "{}: error[{}]: {}",
            source_position(loaded, &diagnostic.source)?,
            diagnostic.code,
            diagnostic.message
        )
        .unwrap();
        for related in &diagnostic.related {
            writeln!(
                output,
                "{}: note[{}.related]: related event",
                source_position(loaded, related)?,
                diagnostic.code
            )
            .unwrap();
        }
    }
    for rule in &report.incomplete_rules {
        writeln!(
            output,
            "error[diagnostics.incomplete]: {rule} could not be checked completely"
        )
        .unwrap();
    }
    let status = if !report.complete() {
        2
    } else if !report.diagnostics.is_empty() {
        1
    } else {
        0
    };
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
