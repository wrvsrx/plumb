use std::path::{Path, PathBuf};

use cel::{Context, ExecutionError, Program, Value};
use chrono::{DateTime, FixedOffset, Local, SecondsFormat};
use comfy_table::{presets::NOTHING, ContentArrangement, Table};
use plumb_extensions::{TaskRecord, TaskState, TaskStatus};
use plumb_workspace::{normalize, TaskEditError, TaskRef, TextEdit, Workspace};

use crate::{display_path, load_workspace, LoadedWorkspace, TaskAction};

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
    let plan = query.map(TaskQueryPlan::compile).transpose()?;
    let records = task_records(root, loaded, plan.as_ref(), tree)?;
    if !records.is_empty() {
        println!("{}", render_task_table(&records, heading));
    }
    Ok(())
}

fn task_records(
    root: &Path,
    loaded: &LoadedWorkspace,
    plan: Option<&TaskQueryPlan>,
    tree: bool,
) -> Result<Vec<TaskOutputRecord>, String> {
    let mut records = Vec::new();
    for path in &loaded.paths {
        let Some(current) = loaded
            .workspace
            .get(path)
            .and_then(|entry| entry.current.as_ref())
        else {
            continue;
        };
        for task in &current.output.tasks.tasks {
            let is_match = match plan {
                Some(plan) => plan.matches(root, path, task, &loaded.workspace)?,
                None => true,
            };
            if is_match {
                records.push(task_output_record(root, path, task, tree));
            }
        }
    }
    Ok(records)
}

fn task_output_record(root: &Path, path: &Path, task: &TaskRecord, tree: bool) -> TaskOutputRecord {
    let status = match task.state() {
        TaskState::Done => "o",
        TaskState::Canceled | TaskState::Conflicted => "x",
        TaskState::Open => "-",
    };
    let title = if tree && task.depth > 0 {
        format!("{}> {}", "  ".repeat(task.depth - 1), task.title)
    } else {
        task.title.clone()
    };
    let source = task.id.as_ref().map_or_else(
        || display_path(root, path),
        |id| format!("{}#{}", display_path(root, path), id.value),
    );
    TaskOutputRecord {
        status: status.to_string(),
        title,
        source,
    }
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

struct TaskQueryPlan {
    program: Program,
    now: DateTime<FixedOffset>,
}

impl TaskQueryPlan {
    fn compile(source: &str) -> Result<Self, String> {
        Self::compile_at(source, Local::now().fixed_offset())
    }

    fn compile_at(source: &str, now: DateTime<FixedOffset>) -> Result<Self, String> {
        Ok(Self {
            program: Program::compile(source)
                .map_err(|error| format!("invalid CEL query: {error}"))?,
            now,
        })
    }

    fn matches(
        &self,
        root: &Path,
        path: &Path,
        task: &TaskRecord,
        workspace: &Workspace,
    ) -> Result<bool, String> {
        let depends_on = workspace
            .task_dependencies(path, task)
            .into_iter()
            .map(|dependency| display_task_ref(root, &dependency.target))
            .collect::<Vec<_>>();
        let directly_blocking = task
            .id
            .as_ref()
            .map(|id| {
                workspace
                    .directly_blocking_tasks(path, &id.value)
                    .iter()
                    .map(|target| display_task_ref(root, target))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let blocked = workspace.is_task_blocked(path, task);
        let actionable = task.state() == TaskState::Open
            && !blocked
            && task
                .wait
                .as_ref()
                .and_then(|wait| DateTime::parse_from_rfc3339(&wait.value).ok())
                .is_none_or(|wait| wait <= self.now);

        let mut context = Context::default();
        context.add_variable_from_value("path", display_path(root, path));
        context
            .add_variable_from_value("id", optional_string(task.id.as_ref().map(|id| &id.value)));
        context.add_variable_from_value("title", task.title.clone());
        context.add_variable_from_value("created", datetime_value(task.created.as_ref()));
        context.add_variable_from_value("due", datetime_value(task.due.as_ref()));
        context.add_variable_from_value("wait", datetime_value(task.wait.as_ref()));
        context.add_variable_from_value("done", datetime_value(task.done.as_ref()));
        context.add_variable_from_value("canceled", datetime_value(task.canceled.as_ref()));
        context.add_variable_from_value(
            "recur",
            optional_string(task.recur.as_ref().map(|field| &field.value)),
        );
        context.add_variable_from_value(
            "prev",
            optional_string(task.prev.as_ref().map(|field| &field.value)),
        );
        context.add_variable_from_value("depends_on", depends_on);
        context.add_variable_from_value("directly_blocking", directly_blocking);
        context.add_variable_from_value("blocked", blocked);
        context.add_variable_from_value("actionable", actionable);
        context.add_variable_from_value("now", Value::Timestamp(self.now));
        match self.program.execute(&context) {
            Ok(Value::Bool(value)) => Ok(value),
            Ok(value) => Err(format!("CEL query must return bool, got {value:?}")),
            Err(ExecutionError::NoSuchKey(_)) => Ok(false),
            Err(error) => Err(format!(
                "cannot evaluate task query for {}: {error}",
                path.display()
            )),
        }
    }
}

fn optional_string(value: Option<&String>) -> Value {
    value
        .cloned()
        .map_or(Value::Null, |value| Value::String(value.into()))
}

fn datetime_value(field: Option<&plumb_extensions::TaskField>) -> Value {
    field
        .and_then(|field| DateTime::parse_from_rfc3339(&field.value).ok())
        .map_or(Value::Null, Value::Timestamp)
}

fn display_task_ref(root: &Path, task_ref: &TaskRef) -> String {
    format!("{}#{}", display_path(root, &task_ref.path), task_ref.id)
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
        let plan = TaskQueryPlan::compile("blocked && id == 'review'").unwrap();
        let records = task_records(&root, &loaded, Some(&plan), true).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, "-");
        assert_eq!(records[0].source, "tasks.plumb#review");

        let deps_path = normalize(&root.join("deps.plumb"));
        let draft = &loaded
            .workspace
            .get(&deps_path)
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .output
            .tasks
            .tasks[0];
        let reverse = TaskQueryPlan::compile("'tasks.plumb#review' in directly_blocking").unwrap();
        assert!(reverse
            .matches(&root, &deps_path, draft, &loaded.workspace)
            .unwrap());

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
