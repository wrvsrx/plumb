use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::Local;
use clap::{Args, Parser, Subcommand, ValueEnum};
use plumb_syntax::DiagnosticSeverity;
use plumb_workspace::{
    display_workspace_path as display_path, normalize, resolve_workspace_root,
    scan_workspace_files, SearchRecordKind, Workspace,
};

mod events;
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

pub fn run_check_cli(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let config = match CheckConfig::try_parse_from(args) {
        Ok(config) => config,
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(error.exit_code() as u8);
        }
    };
    let result = (|| {
        let root = resolve_workspace_root(config.root.as_deref())?;
        let loaded = load_workspace(&root)?;
        render_workspace_diagnostics(&root, &loaded, config.level)
    })();
    match result {
        Ok((output, has_failures)) => {
            print!("{output}");
            if has_failures {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            eprintln!("plumb: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(config: Config) -> Result<(), String> {
    let root = resolve_workspace_root(config.root.as_deref())?;
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
                )
                .map_err(|error| error.to_string())?
                .value
                .items
                .into_iter()
                .map(|record| record.path)
                .collect::<Vec<_>>();
            if note.interactive {
                let action = run_interactive(&root, &selected_paths, &loaded.workspace)?;
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
        Command::Event(event) => match event.command {
            Some(EventCommand::ExportVdir(export)) => {
                if config.query.is_some() {
                    return Err("event export-vdir does not support --query".to_string());
                }
                events::export_vdir(&loaded, &export.output)?;
            }
            None => {
                let records = loaded
                    .workspace
                    .search_records_filtered(
                        &root,
                        Some(SearchRecordKind::Event),
                        "",
                        usize::MAX,
                        Local::now().fixed_offset(),
                        config.query.as_deref(),
                    )
                    .map_err(|error| error.to_string())?
                    .value;
                for event in records.items {
                    println!(
                        "{}\t{}\t{}\t{}\t{}",
                        event.at.as_deref().unwrap_or("-"),
                        event.start.as_deref().unwrap_or("-"),
                        event.end.as_deref().unwrap_or("-"),
                        event.title,
                        event.id.map_or_else(
                            || display_path(&root, &event.path),
                            |id| format!("{}#{id}", display_path(&root, &event.path)),
                        )
                    );
                }
            }
        },
    }
    Ok(())
}

#[derive(Debug, Parser)]
#[command(name = "plumb", about = "Query plumb documents")]
struct Config {
    /// Workspace root. Defaults to the nearest ancestor containing .plumb/.
    #[arg(long, global = true, value_name = "DIR")]
    root: Option<PathBuf>,

    /// Keep records whose CEL predicate evaluates to true.
    #[arg(long, global = true, value_name = "EXPR")]
    query: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Parser)]
#[command(name = "plumb check", about = "Check a plumb workspace")]
struct CheckConfig {
    /// Workspace root. Defaults to the nearest ancestor containing .plumb/.
    #[arg(long, value_name = "DIR")]
    root: Option<PathBuf>,

    /// Lowest diagnostic severity to display.
    #[arg(long, value_enum, default_value_t = CheckLevel::Warning)]
    level: CheckLevel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum CheckLevel {
    Error,
    Warning,
    Hint,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Filter plumb note files under the scanned directory.
    Note(NoteConfig),
    /// Print tasks found in scanned plumb files.
    Task(TaskConfig),
    /// Export events for calendar clients.
    Event(EventConfig),
}

#[derive(Debug, Args)]
struct EventConfig {
    #[command(subcommand)]
    command: Option<EventCommand>,
}

#[derive(Debug, Subcommand)]
enum EventCommand {
    /// Generate a managed read-only vdir calendar.
    ExportVdir(EventExportConfig),
}

#[derive(Debug, Args)]
struct EventExportConfig {
    /// Managed vdir output directory.
    #[arg(long, value_name = "DIR", required = true)]
    output: PathBuf,
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
    root: PathBuf,
    workspace: Workspace,
}

fn load_workspace(root: &Path) -> Result<LoadedWorkspace, String> {
    let root = normalize(root);
    let paths = scan_workspace_files(&root).into_result()?;
    let mut workspace = Workspace::new();
    let batch = workspace
        .index_disk_files(&paths, true, |_| 0, || false)
        .map_err(|error| error.to_string())?;
    if !batch.is_complete() {
        return Err(batch
            .failures
            .iter()
            .map(|failure| {
                format!(
                    "cannot read {}: {}",
                    failure.path.display(),
                    failure.message
                )
            })
            .collect::<Vec<_>>()
            .join("\n"));
    }
    Ok(LoadedWorkspace { root, workspace })
}

fn render_workspace_diagnostics(
    root: &Path,
    loaded: &LoadedWorkspace,
    level: CheckLevel,
) -> Result<(String, bool), String> {
    use std::fmt::Write as _;

    let root = normalize(root);
    let mut entries = loaded.workspace.documents().collect::<Vec<_>>();
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    let mut output = String::new();
    let mut has_failures = false;
    for entry in entries {
        let path = &entry.path;
        let source = &entry.parsed.source;
        let mut diagnostics = loaded
            .workspace
            .diagnostics(path)
            .map_err(|error| error.to_string())?
            .value;
        diagnostics.sort_by(|left, right| {
            (
                left.range.start,
                left.range.end,
                severity_rank(&left.severity),
                left.code,
                left.message.as_str(),
            )
                .cmp(&(
                    right.range.start,
                    right.range.end,
                    severity_rank(&right.severity),
                    right.code,
                    right.message.as_str(),
                ))
        });
        let displayed_path = display_path(&root, path);
        for diagnostic in diagnostics {
            has_failures |= matches!(diagnostic.severity, DiagnosticSeverity::Error);
            if severity_rank(&diagnostic.severity) > level.rank() {
                continue;
            }
            let (line, column) = line_column(source, diagnostic.range.start);
            let severity = severity_name(&diagnostic.severity);
            writeln!(
                output,
                "{displayed_path}:{line}:{column}: {severity}[{}]: {}",
                diagnostic.code, diagnostic.message
            )
            .expect("writing to String cannot fail");
            for related in diagnostic.related {
                let (line, column) = line_column(source, related.start);
                writeln!(
                    output,
                    "{displayed_path}:{line}:{column}: note[{}.related]: related location",
                    diagnostic.code
                )
                .expect("writing to String cannot fail");
            }
        }
    }
    Ok((output, has_failures))
}

impl CheckLevel {
    fn rank(self) -> u8 {
        match self {
            Self::Error => 0,
            Self::Warning => 1,
            Self::Hint => 2,
        }
    }
}

fn line_column(source: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(source.len());
    let line_start = source[..offset].rfind('\n').map_or(0, |index| index + 1);
    let line = source[..line_start]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    let column = source[line_start..offset].chars().count() + 1;
    (line, column)
}

fn severity_name(severity: &DiagnosticSeverity) -> &'static str {
    match severity {
        DiagnosticSeverity::Error => "error",
        DiagnosticSeverity::Warning => "warning",
        DiagnosticSeverity::Hint => "hint",
    }
}

fn severity_rank(severity: &DiagnosticSeverity) -> u8 {
    match severity {
        DiagnosticSeverity::Error => 0,
        DiagnosticSeverity::Warning => 1,
        DiagnosticSeverity::Hint => 2,
    }
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
        assert!(root_help.contains("nearest ancestor containing .plumb/"));
        assert!(root_help.contains("CEL predicate"));

        let mut command = Config::command();
        let task = command
            .find_subcommand_mut("task")
            .unwrap()
            .find_subcommand_mut("complete")
            .unwrap();
        let task_help = task.render_long_help().to_string();
        assert!(task_help.contains("path.plumb#task-id"));

        let check_help = CheckConfig::command().render_long_help().to_string();
        assert!(check_help.contains("Check a plumb workspace"));
        assert!(check_help.contains("nearest ancestor containing .plumb/"));
        assert!(check_help.contains("--level <LEVEL>"));
        assert!(check_help.contains("[default: warning]"));
    }

    #[test]
    fn renders_workspace_diagnostics_with_stable_locations_and_hint_status() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(
            root.join("a.plumb"),
            "`= title|First\n`= title|Second\n\n中文\n",
        )
        .unwrap();
        std::fs::write(
            root.join("nested/b.plumb"),
            "See `->[missing|missing.plumb#id].\n",
        )
        .unwrap();
        let loaded = load_workspace(&root).unwrap();
        let (output, has_failures) =
            render_workspace_diagnostics(&root, &loaded, CheckLevel::Warning).unwrap();
        assert!(!has_failures);
        let lines = output.lines().collect::<Vec<_>>();
        assert!(lines[0].starts_with("a.plumb:2:"), "{output}");
        assert!(
            lines[0].contains("warning[metadata.duplicate-key]"),
            "{output}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("nested/b.plumb:1:")
                    && line.contains("warning[link.unresolved-path]")),
            "{output}"
        );

        std::fs::remove_file(root.join("a.plumb")).unwrap();
        std::fs::remove_file(root.join("nested/b.plumb")).unwrap();
        std::fs::write(
            root.join("tasks.plumb"),
            "`- Draft\n\n `+ task\n\n `@ draft\n`- Review\n\n `+ task\n\n `@ review\n\n `= depends|#draft\n",
        )
        .unwrap();
        let loaded = load_workspace(&root).unwrap();
        let (output, has_failures) =
            render_workspace_diagnostics(&root, &loaded, CheckLevel::Warning).unwrap();
        assert!(!has_failures, "{output}");
        assert!(output.is_empty(), "{output}");
        let (output, has_failures) =
            render_workspace_diagnostics(&root, &loaded, CheckLevel::Hint).unwrap();
        assert!(!has_failures, "{output}");
        assert!(output.contains("hint[task.blocked]"), "{output}");
        std::fs::remove_dir_all(root).unwrap();
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
        std::fs::write(root.join("index.plumb"), "`->[topic|topic.plumb]\n").unwrap();
        std::fs::write(root.join("topic.plumb"), "`->[leaf|leaf.plumb]\n").unwrap();
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
            .unwrap()
            .value;
        assert!(results.items.iter().any(|record| record.path == leaf));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn document_referrer_queries_include_task_prev_and_dependencies() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("index.plumb"),
            "`- Index\n\n `+ task\n\n `@ index\n\n `= prev|topic.plumb#topic\n",
        )
        .unwrap();
        std::fs::write(
            root.join("topic.plumb"),
            "`- Topic\n\n `+ task\n\n `@ topic\n\n `= depends|leaf.plumb#leaf\n",
        )
        .unwrap();
        std::fs::write(root.join("leaf.plumb"), "`- Leaf\n\n `+ task\n\n `@ leaf\n").unwrap();
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
            .unwrap()
            .value;
        assert!(results.items.iter().any(|record| record.path == leaf));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn queries_document_metadata_title() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(
            root.join("docs/semantics.plumb"),
            "`= title|Semantics `em[Guide]\n\n`# Heading\n",
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
            .unwrap()
            .value;
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
        assert!(error.to_string().contains("must return bool"));
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
        assert!(
            loaded
                .workspace
                .contains(root.join("linked.plumb"))
                .unwrap()
                .value
        );
        assert!(
            !loaded
                .workspace
                .contains(root.join("snapshot/hidden.plumb"))
                .unwrap()
                .value
        );

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(snapshot).unwrap();
    }

    #[test]
    fn workspace_scan_applies_ignore_files() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(root.join("private")).unwrap();
        std::fs::write(root.join(".ignore"), "private/\n").unwrap();
        std::fs::write(root.join("public.plumb"), "Public\n").unwrap();
        std::fs::write(root.join("private/note.plumb"), "Private\n").unwrap();

        let loaded = load_workspace(&root).unwrap();
        assert!(
            loaded
                .workspace
                .contains(root.join("public.plumb"))
                .unwrap()
                .value
        );
        assert!(
            !loaded
                .workspace
                .contains(root.join("private/note.plumb"))
                .unwrap()
                .value
        );

        std::fs::remove_dir_all(root).unwrap();
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
