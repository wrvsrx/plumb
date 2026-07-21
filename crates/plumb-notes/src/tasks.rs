use std::path::{Path, PathBuf};

use chrono::{Local, SecondsFormat};
use comfy_table::{presets::NOTHING, ContentArrangement, Table};
use plumb_extensions::{TaskState, TaskStatus};
use plumb_workspace::{normalize, SearchRecordKind, TaskEditError, TextEdit};

use crate::{load_workspace, LoadedWorkspace, TaskAction};

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskOutputRecord {
    status: String,
    title: String,
    source: String,
}

pub(crate) fn print_tasks(
    root: &Path,
    loaded: &LoadedWorkspace,
    query: Option<&str>,
    tree: bool,
    heading: bool,
) -> Result<(), String> {
    let records = task_records(root, loaded, query, tree)?;
    if !records.is_empty() {
        println!("{}", render_task_table(&records, heading));
    }
    Ok(())
}

fn task_records(
    root: &Path,
    loaded: &LoadedWorkspace,
    query: Option<&str>,
    tree: bool,
) -> Result<Vec<TaskOutputRecord>, String> {
    let results = loaded.workspace.search_records_filtered(
        root,
        Some(SearchRecordKind::Task),
        "",
        usize::MAX,
        Local::now().fixed_offset(),
        query,
    )?;
    Ok(results
        .items
        .into_iter()
        .map(|record| {
            let status = match record.task_state.expect("task search record has state") {
                TaskState::Done => "o",
                TaskState::Canceled | TaskState::Conflicted => "x",
                TaskState::Open => "-",
            };
            let depth = record.depth.unwrap_or_default();
            let title = if tree && depth > 0 {
                format!("{}> {}", "  ".repeat(depth - 1), record.title)
            } else {
                record.title
            };
            let source = record.id.map_or_else(
                || record.relative_path.clone(),
                |id| format!("{}#{id}", record.relative_path),
            );
            TaskOutputRecord {
                status: status.to_string(),
                title,
                source,
            }
        })
        .collect())
}

fn render_task_table(records: &[TaskOutputRecord], heading: bool) -> String {
    render_task_table_with_width(records, heading, Some(terminal_width()))
}

fn terminal_width() -> u16 {
    crossterm::terminal::size()
        .ok()
        .map(|(width, _)| width)
        .filter(|width| *width > 0)
        .or_else(|| {
            std::env::var("COLUMNS")
                .ok()
                .and_then(|width| width.parse().ok())
                .filter(|width| *width > 0)
        })
        .unwrap_or(120)
}

fn render_task_table_with_width(
    records: &[TaskOutputRecord],
    heading: bool,
    width: Option<u16>,
) -> String {
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_content_arrangement(ContentArrangement::Dynamic);
    if let Some(width) = width {
        table.set_width(width);
    }
    if heading {
        table.set_header(["S", "Task", "Source"]);
    }
    for record in records {
        table.add_row([&record.status, &record.title, &record.source]);
    }
    table.to_string()
}

pub(crate) fn run_task_action(root: &Path, action: TaskAction) -> Result<(), String> {
    let (status, targets) = match action {
        TaskAction::Complete(config) => (TaskStatus::Done, config.targets),
        TaskAction::Cancel(config) => (TaskStatus::Canceled, config.targets),
    };
    let timestamp = Local::now()
        .fixed_offset()
        .to_rfc3339_opts(SecondsFormat::Secs, false);
    for target in targets {
        set_task_status_target(root, &target, status, &timestamp)?;
    }
    Ok(())
}

fn set_task_status_target(
    root: &Path,
    target: &str,
    status: TaskStatus,
    timestamp: &str,
) -> Result<(), String> {
    let (path, id) = parse_task_target(root, target)?;
    let loaded = load_workspace(root)?;
    let edit = loaded
        .workspace
        .set_task_status_by_id(&path, &id, status, timestamp)
        .map_err(task_edit_error)?;
    let document = edit
        .document_changes
        .into_iter()
        .find(|document| document.path == path)
        .ok_or_else(|| "task operation did not edit its target document".to_string())?;
    let source = loaded
        .texts
        .get(&path)
        .cloned()
        .ok_or_else(|| format!("task document is not loaded: {}", path.display()))?;
    let updated = apply_text_edits(source, document.edits)?;
    std::fs::write(&path, updated)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn parse_task_target(root: &Path, target: &str) -> Result<(PathBuf, String), String> {
    let (path, id) = target
        .split_once('#')
        .filter(|(path, id)| !path.is_empty() && !id.is_empty())
        .ok_or_else(|| format!("task target must be path.plumb#task-id: {target}"))?;
    let root = normalize(root);
    let path = normalize(&root.join(path));
    if !path.starts_with(&root) {
        return Err(format!("task target escapes root: {target}"));
    }
    if path
        .extension()
        .is_none_or(|extension| extension != "plumb")
    {
        return Err(format!("task target is not a .plumb file: {target}"));
    }
    Ok((path, id.to_string()))
}

fn apply_text_edits(mut source: String, mut edits: Vec<TextEdit>) -> Result<String, String> {
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
    let mut previous_start = source.len();
    for edit in edits {
        if edit.range.end > previous_start || edit.range.end > source.len() {
            return Err("task edits overlap or fall outside the document".to_string());
        }
        previous_start = edit.range.start;
        source.replace_range(edit.range, &edit.new_text);
    }
    Ok(source)
}

fn task_edit_error(error: TaskEditError) -> String {
    match error {
        TaskEditError::StaleOrInvalidDocument => "task document is invalid".to_string(),
        TaskEditError::TaskNotFound => "task id not found".to_string(),
        TaskEditError::TaskAlreadyClosed => "task is already closed".to_string(),
        TaskEditError::TaskBlocked => "task is blocked by open dependencies".to_string(),
        TaskEditError::InvalidRecurrence => "task recurrence is invalid".to_string(),
        TaskEditError::InvalidTimestamp => "operation timestamp is invalid".to_string(),
        TaskEditError::ListItemNotFound => "no list item exists at the target".to_string(),
        TaskEditError::TaskAlreadyExists => "the list item is already a task".to_string(),
        TaskEditError::CreatedAlreadyExists => {
            "the task already has a created timestamp".to_string()
        }
        TaskEditError::GeneratedInvalid => "the generated task edit is invalid".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    #[test]
    fn task_queries_and_tree_output_use_workspace_facts() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("deps.plumb"), "`-{.task #draft} Draft\n").unwrap();
        std::fs::write(
            root.join("tasks.plumb"),
            "`-{.task #review depends=\"deps.plumb#draft\"} Review\n  `-{.task #nested done=\"2026-07-20T09:00:00Z\"} Nested\n",
        )
        .unwrap();
        let loaded = load_workspace(&root).unwrap();
        let records =
            task_records(&root, &loaded, Some("blocked && id == 'review'"), true).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, "-");
        assert_eq!(records[0].source, "tasks.plumb#review");

        let reverse = task_records(
            &root,
            &loaded,
            Some("'tasks.plumb#review' in directly_blocking"),
            true,
        )
        .unwrap();
        assert_eq!(reverse.len(), 1);
        assert_eq!(reverse[0].source, "deps.plumb#draft");

        let all = task_records(&root, &loaded, None, true).unwrap();
        assert!(all.iter().any(|record| record.title == "> Nested"));
        let rendered = render_task_table(&records, true);
        assert!(rendered.contains('S'));
        assert!(rendered.contains("Task"));
        assert!(rendered.contains("Source"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn direct_task_action_writes_shared_status_edits() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tasks.plumb");
        std::fs::write(&path, "`-{.task #write} Write parser\n").unwrap();
        set_task_status_target(
            &root,
            "tasks.plumb#write",
            TaskStatus::Done,
            "2026-07-20T12:00:00+08:00",
        )
        .unwrap();
        let updated = std::fs::read_to_string(path).unwrap();
        assert!(updated.contains("done=\"2026-07-20T12:00:00+08:00\""));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn task_query_ignores_invalid_task_owners() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("tasks.plumb"),
            "`note{.task #invalid} Invalid owner\n`.{.task #valid} Valid task\n",
        )
        .unwrap();

        let loaded = load_workspace(&root).unwrap();
        let records = task_records(&root, &loaded, None, true).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].source, "tasks.plumb#valid");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn task_action_updates_multiple_explicit_targets() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        let first = root.join("first.plumb");
        let second = root.join("second.plumb");
        std::fs::write(&first, "`-{.task #first} First\n").unwrap();
        std::fs::write(&second, "`-{.task #second} Second\n").unwrap();

        run_task_action(
            &root,
            TaskAction::Complete(crate::TaskTargetsConfig {
                targets: vec![
                    "first.plumb#first".to_string(),
                    "second.plumb#second".to_string(),
                ],
            }),
        )
        .unwrap();

        assert!(std::fs::read_to_string(first).unwrap().contains(" done=\""));
        assert!(std::fs::read_to_string(second)
            .unwrap()
            .contains(" done=\""));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn task_table_wraps_to_requested_width() {
        let records = [TaskOutputRecord {
            status: "-".to_string(),
            title: "Write a parser that handles narrow terminals".to_string(),
            source: "tasks.plumb#write-parser".to_string(),
        }];

        let rendered = render_task_table_with_width(&records, true, Some(40));
        assert!(rendered.lines().all(|line| line.chars().count() <= 40));
        assert!(rendered.lines().count() > 2);
        let without_heading = render_task_table_with_width(&records, false, Some(100));
        assert!(!without_heading.contains("Task"));
        assert!(without_heading.contains("Write a parser"));
    }

    fn unique_temp_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "plumb-task-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
