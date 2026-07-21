use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::Local;
use clap::{Args, Parser, Subcommand};
use plumb_workspace::{normalize, SearchRecordKind, Workspace};

mod interactive;
mod tasks;

use interactive::{handle_interactive_action, run_interactive};
use tasks::{print_tasks, run_task_action};

pub fn run_cli(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let config = match Config::try_parse_from(args) {
        Ok(config) => config,
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(error.exit_code() as u8);
        }
    };
    match run(config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("plumb: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(config: Config) -> Result<(), String> {
    let root = config
        .root
        .unwrap_or(std::env::current_dir().map_err(|error| error.to_string())?);
    let loaded = load_workspace(&root)?;
    match config.command {
        Command::Note(note) => {
            let selected_paths = loaded
                .workspace
                .search_records_filtered(
                    &root,
                    Some(SearchRecordKind::Note),
                    "",
                    usize::MAX,
                    Local::now().fixed_offset(),
                    config.query.as_deref(),
                )?
                .items
                .into_iter()
                .map(|record| record.path)
                .collect::<Vec<_>>();
            if note.interactive {
                let action = run_interactive(&root, &selected_paths, &loaded.texts)?;
                handle_interactive_action(&root, action)?;
            } else {
                for path in selected_paths {
                    println!("{}", display_path(&root, &path));
                }
            }
        }
        Command::Task(task) => {
            if let Some(action) = task.action {
                if config.query.is_some() {
                    return Err(
                        "task actions do not support --query; pass explicit TARGET values"
                            .to_string(),
                    );
                }
                run_task_action(&root, action)?;
            } else {
                print_tasks(
                    &root,
                    &loaded,
                    config.query.as_deref(),
                    !task.flat,
                    !task.no_heading,
                )?;
            }
        }
    }
    Ok(())
}

#[derive(Debug, Parser)]
#[command(name = "plumb", about = "Query plumb documents")]
struct Config {
    /// Directory to scan recursively. Defaults to the current directory.
    #[arg(long, global = true, value_name = "DIR")]
    root: Option<PathBuf>,

    /// Keep records whose CEL predicate evaluates to true.
    #[arg(long, global = true, value_name = "EXPR")]
    query: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Filter plumb note files under the scanned directory.
    Note(NoteConfig),
    /// Print tasks found in scanned plumb files.
    Task(TaskConfig),
}

#[derive(Debug, Args)]
struct NoteConfig {
    /// Re-filter results interactively with skim.
    #[arg(short, long)]
    interactive: bool,
}

#[derive(Debug, Args)]
struct TaskConfig {
    /// Print task titles without nested task tree markers.
    #[arg(long)]
    flat: bool,

    /// Print task rows without the table heading.
    #[arg(long)]
    no_heading: bool,

    #[command(subcommand)]
    action: Option<TaskAction>,
}

#[derive(Debug, Subcommand)]
enum TaskAction {
    /// Mark task targets complete. Recurring tasks advance to the next instance.
    Complete(TaskTargetsConfig),
    /// Mark task targets canceled. Recurring tasks advance to the next instance.
    Cancel(TaskTargetsConfig),
}

#[derive(Debug, Args)]
struct TaskTargetsConfig {
    /// Task targets, written as path.plumb#task-id.
    #[arg(value_name = "TARGET", required = true)]
    targets: Vec<String>,
}

struct LoadedWorkspace {
    workspace: Workspace,
    texts: HashMap<PathBuf, String>,
}

fn load_workspace(root: &Path) -> Result<LoadedWorkspace, String> {
    let root = normalize(root);
    let mut paths = Vec::new();
    collect_plumb_files(&root, &mut paths)?;
    paths.sort();
    let mut workspace = Workspace::new();
    let mut texts = HashMap::new();
    for path in &paths {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        workspace.insert(path, 0, text.clone());
        texts.insert(path.clone(), text);
    }
    Ok(LoadedWorkspace { workspace, texts })
}

fn collect_plumb_files(path: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(path)
        .map_err(|error| format!("cannot read directory {}: {error}", path.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot read directory entry: {error}"))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot stat {}: {error}", path.display()))?;
        if file_type.is_dir() {
            collect_plumb_files(&path, output)?;
        } else if (file_type.is_file() || file_type.is_symlink())
            && path
                .extension()
                .is_some_and(|extension| extension == "plumb")
        {
            output.push(normalize(&path));
        }
    }
    Ok(())
}

fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use clap::CommandFactory;

    use super::*;

    #[test]
    fn help_describes_commands_options_and_task_target_spelling() {
        let root_help = Config::command().render_long_help().to_string();
        assert!(root_help.contains("Filter plumb note files"));
        assert!(root_help.contains("Print tasks found"));
        assert!(root_help.contains("Directory to scan recursively"));
        assert!(root_help.contains("CEL predicate"));

        let mut command = Config::command();
        let task = command
            .find_subcommand_mut("task")
            .unwrap()
            .find_subcommand_mut("complete")
            .unwrap();
        let task_help = task.render_long_help().to_string();
        assert!(task_help.contains("path.plumb#task-id"));
    }

    #[test]
    fn accepts_interactive_note_options_after_subcommand() {
        let config = Config::parse_from([
            "plumb-notes",
            "note",
            "--root",
            "notes",
            "--query",
            "path.endsWith('topic.plumb')",
            "--interactive",
        ]);
        assert_eq!(config.root.as_deref(), Some(Path::new("notes")));
        assert_eq!(
            config.query.as_deref(),
            Some("path.endsWith('topic.plumb')")
        );
        assert!(matches!(
            config.command,
            Command::Note(NoteConfig { interactive: true })
        ));
    }

    #[test]
    fn accepts_task_listing_and_action_options() {
        let listing = Config::parse_from([
            "plumb-notes",
            "task",
            "--root",
            "notes",
            "--query",
            "actionable",
            "--flat",
            "--no-heading",
        ]);
        assert!(matches!(
            listing.command,
            Command::Task(TaskConfig {
                flat: true,
                no_heading: true,
                action: None,
            })
        ));

        let action = Config::parse_from([
            "plumb-notes",
            "task",
            "complete",
            "notes/tasks.plumb#write-parser",
        ]);
        assert!(matches!(
            action.command,
            Command::Task(TaskConfig {
                action: Some(TaskAction::Complete(TaskTargetsConfig { ref targets })),
                ..
            }) if targets == &["notes/tasks.plumb#write-parser"]
        ));
    }

    #[test]
    fn queries_transitive_referrers() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("index.plumb"), "`->[topic]{to=\"topic.plumb\"}\n").unwrap();
        std::fs::write(root.join("topic.plumb"), "`->[leaf]{to=\"leaf.plumb\"}\n").unwrap();
        std::fs::write(root.join("leaf.plumb"), "Leaf note.\n").unwrap();
        let loaded = load_workspace(&root).unwrap();
        let leaf = normalize(&root.join("leaf.plumb"));
        let results = loaded
            .workspace
            .search_records_filtered(
                &root,
                Some(SearchRecordKind::Note),
                "",
                usize::MAX,
                Local::now().fixed_offset(),
                Some("'index.plumb' in transitively_referenced_by"),
            )
            .unwrap();
        assert!(results.items.iter().any(|record| record.path == leaf));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn document_referrer_queries_include_task_prev_and_dependencies() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("index.plumb"),
            "`-{.task #index prev=\"topic.plumb#topic\"} Index\n",
        )
        .unwrap();
        std::fs::write(
            root.join("topic.plumb"),
            "`-{.task #topic depends=\"leaf.plumb#leaf\"} Topic\n",
        )
        .unwrap();
        std::fs::write(root.join("leaf.plumb"), "`-{.task #leaf} Leaf\n").unwrap();
        let loaded = load_workspace(&root).unwrap();
        let leaf = normalize(&root.join("leaf.plumb"));
        let results = loaded
            .workspace
            .search_records_filtered(
                &root,
                Some(SearchRecordKind::Note),
                "",
                usize::MAX,
                Local::now().fixed_offset(),
                Some("'topic.plumb' in directly_referenced_by && 'index.plumb' in transitively_referenced_by"),
            )
            .unwrap();
        assert!(results.items.iter().any(|record| record.path == leaf));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn queries_document_metadata_title() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(
            root.join("docs/semantics.plumb"),
            "`meta\n  `: title\n\n    Semantics `em[Guide]\n\n`# Heading\n",
        )
        .unwrap();
        std::fs::write(root.join("notes.plumb"), "`# Notes\n").unwrap();
        let loaded = load_workspace(&root).unwrap();
        let semantics = normalize(&root.join("docs/semantics.plumb"));
        let results = loaded
            .workspace
            .search_records_filtered(
                &root,
                Some(SearchRecordKind::Note),
                "",
                usize::MAX,
                Local::now().fixed_offset(),
                Some("path.startsWith('docs/') && title.matches('Semantics Guide')"),
            )
            .unwrap();
        assert!(results.items.iter().any(|record| record.path == semantics));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_non_boolean_query_results() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("note.plumb"), "A note.\n").unwrap();
        let loaded = load_workspace(&root).unwrap();
        let error = loaded
            .workspace
            .search_records_filtered(
                &root,
                Some(SearchRecordKind::Note),
                "",
                usize::MAX,
                Local::now().fixed_offset(),
                Some("path"),
            )
            .unwrap_err();
        assert!(error.contains("must return bool"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn workspace_scan_keeps_file_symlinks_without_following_directory_symlinks() {
        use std::os::unix::fs::symlink;

        let root = unique_temp_dir();
        let snapshot = unique_temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&snapshot).unwrap();
        std::fs::write(snapshot.join("hidden.plumb"), "Hidden\n").unwrap();
        std::fs::write(root.join("linked.txt"), "Linked\n").unwrap();
        symlink(&snapshot, root.join("snapshot")).unwrap();
        symlink(root.join("linked.txt"), root.join("linked.plumb")).unwrap();

        let loaded = load_workspace(&root).unwrap();
        assert!(loaded.workspace.contains(root.join("linked.plumb")));
        assert!(!loaded
            .workspace
            .contains(root.join("snapshot/hidden.plumb")));

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(snapshot).unwrap();
    }

    fn unique_temp_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "plumb-notes-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
