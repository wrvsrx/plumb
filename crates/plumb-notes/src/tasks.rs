use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset, Local, SecondsFormat};
use comfy_table::{presets::NOTHING, ContentArrangement, Table};
use plumb_extensions::TaskStatus;
use plumb_workspace::{normalize, SearchRecordKind, TaskEditError, TaskWorkflowState, TextEdit};

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
    let mut records = loaded
        .workspace
        .search_records_filtered(
            root,
            Some(SearchRecordKind::Task),
            "",
            usize::MAX,
            Local::now().fixed_offset(),
            None,
        )?
        .items;
    let retained = if let Some(query) = query {
        Some(
            loaded
                .workspace
                .search_records_filtered(
                    root,
                    Some(SearchRecordKind::Task),
                    "",
                    usize::MAX,
                    Local::now().fixed_offset(),
                    Some(query),
                )?
                .items
                .into_iter()
                .map(|record| (record.relative_path, record.range.start))
                .collect::<BTreeSet<_>>(),
        )
    } else {
        None
    };
    sort_task_subtrees(&mut records);
    if let Some(retained) = retained {
        records.retain(|record| {
            retained.contains(&(record.relative_path.clone(), record.range.start))
        });
    }
    Ok(records
        .into_iter()
        .map(|record| {
            let status = match record.task_state.expect("task search record has state") {
                TaskWorkflowState::Done => "o",
                TaskWorkflowState::Canceled | TaskWorkflowState::Invalid => "x",
                TaskWorkflowState::Ready | TaskWorkflowState::Waiting => "-",
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

fn sort_task_subtrees(records: &mut Vec<plumb_workspace::SearchRecord>) {
    records.sort_by(|left, right| {
        left.relative_path
            .cmp(&right.relative_path)
            .then(left.range.start.cmp(&right.range.start))
    });
    let mut documents = BTreeMap::<String, Vec<plumb_workspace::SearchRecord>>::new();
    for record in std::mem::take(records) {
        documents
            .entry(record.relative_path.clone())
            .or_default()
            .push(record);
    }
    let mut documents = documents
        .into_iter()
        .map(|(path, records)| {
            let mut children = task_forest(&records);
            sort_task_forest(&mut children);
            let priority = children
                .iter()
                .map(|child| child.effective_priority)
                .max()
                .unwrap_or_default()
                .max(0);
            let due = children.iter().filter_map(|child| child.earliest_due).min();
            (path, priority, due, children)
        })
        .collect::<Vec<_>>();
    documents.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| optional_due_order(left.2.as_ref(), right.2.as_ref()))
            .then(left.0.cmp(&right.0))
    });
    records.extend(
        documents
            .into_iter()
            .flat_map(|(_, _, _, children)| children.into_iter().flat_map(flatten_task_tree)),
    );
}

struct TaskTree {
    root: plumb_workspace::SearchRecord,
    children: Vec<TaskTree>,
    effective_priority: i32,
    earliest_due: Option<DateTime<FixedOffset>>,
}

fn task_forest(records: &[plumb_workspace::SearchRecord]) -> Vec<TaskTree> {
    let mut forest = Vec::new();
    let mut index = 0;
    while index < records.len() {
        let root = records[index].clone();
        let root_depth = root.depth.unwrap_or_default();
        let mut end = index + 1;
        while end < records.len()
            && records[end].relative_path == root.relative_path
            && records[end].depth.unwrap_or_default() > root_depth
        {
            end += 1;
        }
        let children = task_forest(&records[index + 1..end]);
        let effective_priority = children
            .iter()
            .map(|child| child.effective_priority)
            .chain(std::iter::once(root.priority.unwrap_or_default()))
            .max()
            .unwrap_or_default();
        let earliest_due = children
            .iter()
            .filter_map(|child| child.earliest_due)
            .chain(
                root.due
                    .as_deref()
                    .and_then(|due| DateTime::parse_from_rfc3339(due).ok()),
            )
            .min();
        forest.push(TaskTree {
            root,
            children,
            effective_priority,
            earliest_due,
        });
        index = end;
    }
    forest
}

fn sort_task_forest(forest: &mut [TaskTree]) {
    for tree in forest.iter_mut() {
        sort_task_forest(&mut tree.children);
    }
    forest.sort_by(|left, right| {
        right
            .effective_priority
            .cmp(&left.effective_priority)
            .then_with(|| {
                optional_due_order(left.earliest_due.as_ref(), right.earliest_due.as_ref())
            })
            .then(left.root.range.start.cmp(&right.root.range.start))
    });
}

fn optional_due_order(
    left: Option<&DateTime<FixedOffset>>,
    right: Option<&DateTime<FixedOffset>>,
) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn flatten_task_tree(tree: TaskTree) -> Vec<plumb_workspace::SearchRecord> {
    std::iter::once(tree.root)
        .chain(tree.children.into_iter().flat_map(flatten_task_tree))
        .collect()
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
    fn task_output_sorts_priority_subtrees_without_detaching_descendants() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("tasks.plumb"),
            "`-{.task #low priority=1} Low\n  `-{.task #low-child priority=99} Low child\n`-{.task #high priority=10} High\n  `-{.task #later-child priority=2} Later child\n    `-{.task #grandchild} Grandchild\n  `-{.task #first-child priority=8} First child\n",
        )
        .unwrap();

        let loaded = load_workspace(&root).unwrap();
        let records = task_records(&root, &loaded, None, true).unwrap();
        assert_eq!(
            records
                .iter()
                .map(|record| record.source.as_str())
                .collect::<Vec<_>>(),
            [
                "tasks.plumb#low",
                "tasks.plumb#low-child",
                "tasks.plumb#high",
                "tasks.plumb#first-child",
                "tasks.plumb#later-child",
                "tasks.plumb#grandchild",
            ]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn task_output_aggregates_document_priority_and_keeps_files_contiguous() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("a.plumb"),
            "`-{.task #deferred priority=-10} Deferred\n  `-{.task #more-deferred priority=-20} More deferred\n`-{.task #normal} Normal\n",
        )
        .unwrap();
        std::fs::write(
            root.join("b.plumb"),
            "`-{.task #important priority=10} Important\n",
        )
        .unwrap();
        std::fs::write(
            root.join("c.plumb"),
            "`-{.task #promoted priority=-5} Promoted root\n  `-{.task #urgent priority=50} Urgent descendant\n",
        )
        .unwrap();

        let loaded = load_workspace(&root).unwrap();
        let records = task_records(&root, &loaded, None, true).unwrap();
        assert_eq!(
            records
                .iter()
                .map(|record| record.source.as_str())
                .collect::<Vec<_>>(),
            [
                "c.plumb#promoted",
                "c.plumb#urgent",
                "b.plumb#important",
                "a.plumb#normal",
                "a.plumb#deferred",
                "a.plumb#more-deferred",
            ]
        );
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
