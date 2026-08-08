use std::path::Path;

use chrono::Local;
use plumb_semantics::{AnchorKind, EventRecord, FileRecord, ImageRecord, LinkRecord, TaskRecord};
use plumb_workspace::{ResolvedTarget, Workspace};

pub(crate) fn target(workspace: &Workspace, target: &ResolvedTarget) -> String {
    match target {
        ResolvedTarget::Anchor { path, id, anchor } => {
            let Some(entry) = workspace.get(path) else {
                return format!("Explicit anchor `#{id}` in `{}`", path.display());
            };
            let kind = if anchor.kind == AnchorKind::Heading {
                "Heading"
            } else {
                "Anchor"
            };
            let line = line_number(&entry.parsed.source, anchor.range.start);
            let preview = preview_from_offset(&entry.parsed.source, anchor.range.start, 5);
            format!(
                "**{kind}** `#{}`\n\n`{}:{line}`\n\n{}",
                escape_markdown_code(id),
                escape_markdown_code(&path.display().to_string()),
                fenced_plumb(&preview)
            )
        }
        ResolvedTarget::Document { path } => {
            let Some(entry) = workspace.get(path) else {
                return format!("Plumb document `{}`", path.display());
            };
            let (line, offset) = first_preview_offset(&entry.parsed.source);
            let preview = preview_from_offset(&entry.parsed.source, offset, 5);
            let location = format!("{}:{line}", path.display());
            if preview.is_empty() {
                format!("**File**\n\n`{}`", escape_markdown_code(&location))
            } else {
                format!(
                    "**File**\n\n`{}`\n\n{}",
                    escape_markdown_code(&location),
                    fenced_plumb(&preview)
                )
            }
        }
        ResolvedTarget::External => "External link".to_string(),
        ResolvedTarget::File { path } => format!(
            "**File**\n\n`{}`",
            escape_markdown_code(&path.display().to_string())
        ),
        ResolvedTarget::UnresolvedFile { path } => format!(
            "**Unresolved file**\n\n`{}`",
            escape_markdown_code(&path.display().to_string())
        ),
        ResolvedTarget::Other => "Non-plumb link".to_string(),
        ResolvedTarget::UnresolvedPath { path } => {
            format!("Unresolved plumb document `{}`", path.display())
        }
        ResolvedTarget::UnresolvedAnchor { path, id } => {
            format!("Unresolved explicit anchor `#{id}` in `{}`", path.display())
        }
        ResolvedTarget::AmbiguousAnchor { path, id } => {
            format!("Ambiguous explicit anchor `#{id}` in `{}`", path.display())
        }
    }
}

pub(crate) fn image(target: &ResolvedTarget, image: &ImageRecord) -> String {
    match target {
        ResolvedTarget::External => format!(
            "**External image**\n\n`{}`",
            escape_markdown_code(&image.source.value)
        ),
        ResolvedTarget::File { path } => format!(
            "**Image file**\n\n`{}`",
            escape_markdown_code(&path.display().to_string())
        ),
        ResolvedTarget::UnresolvedFile { path } => format!(
            "**Unresolved image file**\n\n`{}`",
            escape_markdown_code(&path.display().to_string())
        ),
        _ => "Image".to_string(),
    }
}

pub(crate) fn file(target: &ResolvedTarget, file: &FileRecord) -> String {
    match target {
        ResolvedTarget::External => format!(
            "**External file attachment**\n\n`{}`",
            escape_markdown_code(&file.source.value)
        ),
        ResolvedTarget::File { path } => format!(
            "**File attachment**\n\n`{}`",
            escape_markdown_code(&path.display().to_string())
        ),
        ResolvedTarget::UnresolvedFile { path } => format!(
            "**Unresolved file attachment**\n\n`{}`",
            escape_markdown_code(&path.display().to_string())
        ),
        _ => "File attachment".to_string(),
    }
}

pub(crate) fn link(target: &ResolvedTarget, link: &LinkRecord) -> String {
    match target {
        ResolvedTarget::External => format!(
            "**External link**\n\n`{}`",
            escape_markdown_code(&link.target.value)
        ),
        ResolvedTarget::Other => format!(
            "**Non-plumb link**\n\n`{}`",
            escape_markdown_code(&link.target.value)
        ),
        _ => unreachable!("only external and non-plumb links use the direct link hover"),
    }
}

pub(crate) fn task(workspace: &Workspace, path: &Path, task: &TaskRecord) -> String {
    let (state, wait_reasons) =
        workspace.task_workflow_state(path, task, Local::now().fixed_offset());
    let mut lines = vec![
        format!("**Task:** {}", task.title),
        format!("**State:** {}", state.as_str()),
    ];
    if !wait_reasons.is_empty() {
        lines.push(format!(
            "**Waiting for:** {}",
            wait_reasons
                .iter()
                .map(|reason| reason.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(id) = &task.id {
        lines.push(format!("**ID:** `#{}`", id.value));
    }
    for (label, field) in [
        ("Created", &task.created),
        ("Due", &task.due),
        ("Wait", &task.wait),
        ("Done", &task.done),
        ("Canceled", &task.canceled),
        ("Recur", &task.recur),
        ("Previous", &task.prev),
    ] {
        if let Some(field) = field {
            lines.push(format!("**{label}:** `{}`", field.value));
        }
    }
    if !task.depends.is_empty() {
        lines.push(format!(
            "**Depends:** {}",
            task.depends
                .iter()
                .map(|dependency| format!("`{}`", dependency.source))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let blockers = workspace.open_task_dependencies(path, task);
    if !blockers.is_empty() {
        lines.push(format!(
            "**Open blockers:** {}",
            blockers
                .iter()
                .map(|dependency| format!("`{}`", dependency.source))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    lines.join("\n\n")
}

pub(crate) fn event(event: &EventRecord) -> String {
    let mut lines = vec![format!("**Event:** {}", event.title)];
    if let Some(id) = &event.id {
        lines.push(format!("**ID:** `#{}`", id.value));
    }
    for (label, field) in [
        ("Date", &event.date),
        ("Timezone", &event.timezone),
        ("When", &event.when),
    ] {
        if let Some(field) = field {
            lines.push(format!("**{label}:** `{}`", field.value));
        }
    }
    if !event.tasks.is_empty() {
        lines.push(format!(
            "**Tasks:** {}",
            event
                .tasks
                .iter()
                .map(|reference| format!("`{}`", reference.source))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    lines.join("  \n")
}

pub(crate) fn fenced_plumb(source: &str) -> String {
    let longest_run = source
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat((longest_run + 1).max(3));
    format!("{fence}plumb\n{source}\n{fence}")
}

fn first_preview_offset(source: &str) -> (usize, usize) {
    source
        .lines()
        .scan(0usize, |offset, line| {
            let current = *offset;
            *offset += line.len() + 1;
            Some((current, line))
        })
        .enumerate()
        .find(|(_, (_, line))| !line.trim().is_empty())
        .map(|(line, (offset, _))| (line + 1, offset))
        .unwrap_or((1, 0))
}

fn preview_from_offset(source: &str, offset: usize, max_lines: usize) -> String {
    let start = source[..offset].rfind('\n').map_or(0, |index| index + 1);
    source[start..]
        .lines()
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

fn line_number(source: &str, offset: usize) -> usize {
    source[..offset]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

fn escape_markdown_code(source: &str) -> String {
    source.replace('`', "\\`")
}
