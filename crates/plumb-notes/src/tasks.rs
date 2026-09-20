use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, SecondsFormat};
use comfy_table::{presets::NOTHING, ContentArrangement, Table};
use plumb_semantics::TaskStatus;
use plumb_workspace::{
    apply_document_edit, display_workspace_path, normalize, sort_task_records, NextQuery,
    SearchRecordKind, TaskSortFacts, TaskSortOrder, TaskWorkflowState, WorkspaceTask,
};

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

/// Print the shared shortlist: in-flight tasks (with their focus start), then
/// ready-to-start candidates, then any task skipped for invalid focus history.
pub(crate) fn print_task_next(
    root: &Path,
    loaded: &LoadedWorkspace,
    limit: u8,
) -> Result<(), String> {
    let query = NextQuery {
        root: root.to_path_buf(),
        limit: usize::from(limit),
        cursor: None,
        workspace_revision: 0,
        now: Local::now().fixed_offset(),
    };
    let result = loaded
        .workspace
        .query_next(&query)
        .map_err(|error| error.to_string())?;
    let complete = result.is_complete();
    let next = result.value;

    println!("In flight ({})", next.focused_total);
    if next.focused.is_empty() {
        println!("  (none)");
    } else {
        println!(
            "{}",
            render_task_table(&next_records(root, &next.focused), false)
        );
    }
    if !next.focused_complete {
        println!(
            "  ... {} more in flight",
            next.focused_total.saturating_sub(next.focused.len())
        );
    }

    println!("Ready to start (limit {})", next.candidate_limit);
    if next.candidates.is_empty() {
        println!("  (none)");
    } else {
        println!(
            "{}",
            render_task_table(&next_records(root, &next.candidates), false)
        );
    }
    if !next.candidates_complete {
        println!("  ... more ready tasks exist than the requested limit");
    }

    for skipped in &next.skipped_invalid {
        let mut codes: Vec<&str> = Vec::new();
        for code in skipped.codes.iter().map(|code| code.code()) {
            if !codes.contains(&code) {
                codes.push(code);
            }
        }
        let path = display_workspace_path(root, &skipped.path);
        let source = skipped
            .id
            .as_ref()
            .map_or_else(|| path.clone(), |id| format!("{path}#{id}"));
        println!(
            "  skipped invalid focus history: {source} ({}) - {}",
            codes.join(", "),
            skipped.title
        );
    }
    if !complete {
        println!("warning: workspace index is incomplete; the shortlist may be missing tasks");
    }
    Ok(())
}

fn next_records(root: &Path, tasks: &[WorkspaceTask]) -> Vec<TaskOutputRecord> {
    tasks
        .iter()
        .map(|task| {
            let status = match task.state {
                TaskWorkflowState::Done => "o",
                TaskWorkflowState::Canceled | TaskWorkflowState::Conflicted => "x",
                TaskWorkflowState::Ready
                | TaskWorkflowState::Waiting
                | TaskWorkflowState::Blocked => "-",
            };
            let path = display_workspace_path(root, &task.path);
            let source = task
                .task
                .id
                .as_ref()
                .map_or_else(|| path.clone(), |id| format!("{path}#{}", id.value));
            let title = task.task.focused_since().map_or_else(
                || task.task.title.clone(),
                |since| {
                    let stamp = DateTime::parse_from_rfc3339(since)
                        .map(|parsed| parsed.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_else(|_| since.to_string());
                    format!("{}  [focused {stamp}]", task.task.title)
                },
            );
            TaskOutputRecord {
                status: status.to_string(),
                title,
                source,
            }
        })
        .collect()
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
        )
        .map_err(|error| error.to_string())?
        .value
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
                )
                .map_err(|error| error.to_string())?
                .value
                .items
                .into_iter()
                .map(|record| (record.relative_path, record.range.start))
                .collect::<BTreeSet<_>>(),
        )
    } else {
        None
    };
    if let Some(retained) = retained {
        records.retain(|record| {
            retained.contains(&(record.relative_path.clone(), record.range.start))
        });
    }
    sort_task_subtrees(&mut records);
    Ok(records
        .into_iter()
        .map(|record| {
            let status = match record.task_state.expect("task search record has state") {
                TaskWorkflowState::Done => "o",
                TaskWorkflowState::Canceled | TaskWorkflowState::Conflicted => "x",
                TaskWorkflowState::Ready
                | TaskWorkflowState::Waiting
                | TaskWorkflowState::Blocked => "-",
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
    sort_task_records(records, TaskSortOrder::Priority, |record| TaskSortFacts {
        document: record.relative_path.clone(),
        source_start: record.range.start,
        depth: record.depth.unwrap_or_default(),
        focused: record.focused,
        priority: record.effective_priority,
        due: record
            .due
            .as_deref()
            .and_then(|due| chrono::DateTime::parse_from_rfc3339(due).ok()),
        relevance: None,
    });
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
    let timestamp = Local::now()
        .fixed_offset()
        .to_rfc3339_opts(SecondsFormat::Secs, false);
    match action {
        TaskAction::Complete(config) => {
            for target in config.targets {
                set_task_status_target(root, &target, TaskStatus::Done, &timestamp)?;
            }
        }
        TaskAction::Cancel(config) => {
            for target in config.targets {
                set_task_status_target(root, &target, TaskStatus::Canceled, &timestamp)?;
            }
        }
        TaskAction::Focus(config) => {
            for target in config.targets {
                set_task_focus_target(root, &target, true, &timestamp)?;
            }
        }
        TaskAction::Unfocus(config) => {
            for target in config.targets {
                set_task_focus_target(root, &target, false, &timestamp)?;
            }
        }
        TaskAction::Next(_) => {
            return Err("`task next` is a query; it does not accept TARGET values".to_string())
        }
    }
    Ok(())
}

fn set_task_focus_target(
    root: &Path,
    target: &str,
    focused: bool,
    timestamp: &str,
) -> Result<(), String> {
    let (path, id) = parse_task_target(root, target)?;
    let loaded = load_workspace(root)?;
    let edit = match (id.as_deref(), focused) {
        (Some(id), true) => loaded.workspace.focus_task_by_id(&path, id, timestamp),
        (Some(id), false) => loaded.workspace.unfocus_task_by_id(&path, id, timestamp),
        (None, true) => loaded.workspace.focus_document_task(&path, timestamp),
        (None, false) => loaded.workspace.unfocus_document_task(&path, timestamp),
    }
    .map_err(|error| error.to_string())?;
    let entry = loaded
        .workspace
        .get(&path)
        .ok_or_else(|| format!("task document is not indexed: {}", path.display()))?;
    let source = entry.parsed.source().to_string();
    let revision = entry.revision;
    let updated = apply_document_edit(source, &path, revision, edit)
        .map_err(|error| format!("cannot apply task edit: {error:?}"))?;
    std::fs::write(&path, updated)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn set_task_status_target(
    root: &Path,
    target: &str,
    status: TaskStatus,
    timestamp: &str,
) -> Result<(), String> {
    let (path, id) = parse_task_target(root, target)?;
    let loaded = load_workspace(root)?;
    let edit = match id.as_deref() {
        Some(id) => loaded.workspace.set_task_status_by_id(&path, id, status, timestamp),
        None => loaded.workspace.set_document_task_status(&path, status, timestamp),
    }.map_err(|error| error.to_string())?;
    let entry = loaded
        .workspace
        .get(&path)
        .ok_or_else(|| format!("task document is not indexed: {}", path.display()))?;
    let source = entry.parsed.source().to_string();
    let revision = entry.revision;
    let updated = apply_document_edit(source, &path, revision, edit)
        .map_err(|error| format!("cannot apply task edit: {error:?}"))?;
    std::fs::write(&path, updated)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn parse_task_target(root: &Path, target: &str) -> Result<(PathBuf, Option<String>), String> {
    let (path, id) = match target.split_once('#') {
        Some((path, id)) if !path.is_empty() && !id.is_empty() => (path, Some(id.to_string())),
        None if !target.is_empty() => (target, None),
        _ => return Err(format!("task target must be path.plumb or path.plumb#task-id: {target}")),
    };
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
    Ok((path, id))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    fn load_workspace(root: &Path) -> Result<LoadedWorkspace, String> {
        super::load_workspace(root)
    }

    #[test]
    fn task_queries_and_tree_output_use_workspace_facts() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("deps.plumb"),
            "`- Draft\n\n `+ task\n\n `@ draft\n",
        )
        .unwrap();
        std::fs::write(
            root.join("tasks.plumb"),
            "`- Review\n\n `+ task\n\n `@ review\n\n `= depends deps.plumb#draft\n\n `- Nested\n\n  `+ task\n\n  `@ nested\n\n  `= done 2026-07-20T09:00:00Z\n",
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
            "`- Low\n\n `+ task\n\n `@ low\n\n `= priority 1\n\n `- Low child\n\n  `+ task\n\n  `@ low-child\n\n  `= priority 99\n\n`- High\n\n `+ task\n\n `@ high\n\n `= priority 10\n\n `- Later child\n\n  `+ task\n\n  `@ later-child\n\n  `= priority 2\n\n  `- Grandchild\n\n   `+ task\n\n   `@ grandchild\n\n `- First child\n\n  `+ task\n\n  `@ first-child\n\n  `= priority 8\n",
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
    fn task_output_propagates_priority_to_open_dependencies() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("tasks.plumb"),
            "`- Medium\n\n `+ task\n\n `@ medium\n\n `= priority 20\n\n`- Blocker\n\n `+ task\n\n `@ blocker\n\n `= priority -5\n\n`- Urgent\n\n `+ task\n\n `@ urgent\n\n `= priority 50\n `= depends #blocker\n",
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
                "tasks.plumb#blocker",
                "tasks.plumb#urgent",
                "tasks.plumb#medium",
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
            "`- Deferred\n\n `+ task\n\n `@ deferred\n\n `= priority -10\n\n `- More deferred\n\n  `+ task\n\n  `@ more-deferred\n\n  `= priority -20\n\n`- Normal\n\n `+ task\n\n `@ normal\n",
        )
        .unwrap();
        std::fs::write(
            root.join("b.plumb"),
            "`- Important\n\n `+ task\n\n `@ important\n\n `= priority 10\n",
        )
        .unwrap();
        std::fs::write(
            root.join("c.plumb"),
            "`- Promoted root\n\n `+ task\n\n `@ promoted\n\n `= priority -5\n\n `- Urgent descendant\n\n  `+ task\n\n  `@ urgent\n\n  `= priority 50\n",
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
    fn filtered_tasks_do_not_contribute_to_cli_priority_order() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("a.plumb"),
            "`- Closed\n\n `+ task\n\n `@ closed\n\n `= priority 100\n `= done 2026-07-31T10:00:00Z\n\n`- Low active\n\n `+ task\n\n `@ low\n\n `= priority 1\n",
        )
        .unwrap();
        std::fs::write(
            root.join("b.plumb"),
            "`- Important active\n\n `+ task\n\n `@ important\n\n `= priority 10\n",
        )
        .unwrap();

        let loaded = load_workspace(&root).unwrap();
        let records = task_records(&root, &loaded, Some("state == 'ready'"), true).unwrap();
        assert_eq!(
            records
                .iter()
                .map(|record| record.source.as_str())
                .collect::<Vec<_>>(),
            ["b.plumb#important", "a.plumb#low"]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn direct_task_action_writes_shared_status_edits() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tasks.plumb");
        std::fs::write(&path, "`- Write parser\n\n `+ task\n\n `@ write\n").unwrap();
        set_task_status_target(
            &root,
            "tasks.plumb#write",
            TaskStatus::Done,
            "2026-07-20T12:00:00+08:00",
        )
        .unwrap();
        let updated = std::fs::read_to_string(path).unwrap();
        assert!(updated.contains("`= done 2026-07-20T12:00:00+08:00"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn task_query_ignores_invalid_task_owners() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("tasks.plumb"),
            "`note Invalid owner\n  `- task\n  `@ invalid\n\n`- Valid task\n\n `+ task\n\n `@ valid\n",
        )
        .unwrap();

        let loaded = load_workspace(&root).unwrap();
        let records = task_records(&root, &loaded, None, true).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].source, "tasks.plumb#valid");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn document_path_target_focuses_and_completes_only_the_document_task() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("project.plumb");
        std::fs::write(&path, "`+ task\n\n`- Child\n `+ task\n").unwrap();
        set_task_focus_target(&root, "project.plumb", true, "2026-09-21T09:00:00+08:00").unwrap();
        set_task_status_target(&root, "project.plumb", TaskStatus::Done, "2026-09-21T10:00:00+08:00").unwrap();
        let source = std::fs::read_to_string(&path).unwrap();
        let parsed = plumb_syntax::parse(source);
        let output = plumb_semantics::analyze_tasks(parsed.valid_syntax().unwrap());
        assert_eq!(output.document_task().unwrap().state(), plumb_semantics::TaskState::Done);
        assert!(!output.document_task().unwrap().to_owned().has_open_focus_interval());
        assert_eq!(output.tasks.get(1).unwrap().state(), plumb_semantics::TaskState::Open);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn task_action_updates_multiple_explicit_targets() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        let first = root.join("first.plumb");
        let second = root.join("second.plumb");
        std::fs::write(&first, "`- First\n\n `+ task\n\n `@ first\n").unwrap();
        std::fs::write(&second, "`- Second\n\n `+ task\n\n `@ second\n").unwrap();

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

        assert!(std::fs::read_to_string(first).unwrap().contains("`= done "));
        assert!(std::fs::read_to_string(second)
            .unwrap()
            .contains("`= done "));
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
