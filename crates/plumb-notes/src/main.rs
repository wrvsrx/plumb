use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cel::{Context, ExecutionError, Program, Value};
use clap::{Args, Parser, Subcommand};
use plumb_workspace::{normalize, ResolvedTarget, Workspace};

mod interactive;
mod tasks;

use interactive::{handle_interactive_action, run_interactive};
use tasks::{print_tasks, run_task_action};

fn main() -> ExitCode {
    match run(Config::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("plumb-notes: {error}");
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
            let plan = config
                .query
                .as_deref()
                .map(QueryPlan::compile)
                .transpose()?;
            let reverse = ReverseReferences::build(&loaded.workspace);
            let mut selected_paths = Vec::new();
            for path in &loaded.paths {
                let is_match = match &plan {
                    Some(plan) => plan.matches(&root, path, &loaded.workspace, &reverse)?,
                    None => true,
                };
                if is_match {
                    selected_paths.push(path.clone());
                }
            }
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
#[command(name = "plumb-notes", about = "Query plumb documents")]
struct Config {
    #[arg(long, global = true, value_name = "DIR")]
    root: Option<PathBuf>,

    #[arg(long, global = true, value_name = "EXPR")]
    query: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Note(NoteConfig),
    Task(TaskConfig),
}

#[derive(Debug, Args)]
struct NoteConfig {
    #[arg(short, long)]
    interactive: bool,
}

#[derive(Debug, Args)]
struct TaskConfig {
    #[arg(long)]
    flat: bool,

    #[arg(long)]
    no_heading: bool,

    #[command(subcommand)]
    action: Option<TaskAction>,
}

#[derive(Debug, Subcommand)]
enum TaskAction {
    Complete(TaskTargetsConfig),
    Cancel(TaskTargetsConfig),
}

#[derive(Debug, Args)]
struct TaskTargetsConfig {
    #[arg(value_name = "TARGET", required = true)]
    targets: Vec<String>,
}

struct LoadedWorkspace {
    workspace: Workspace,
    paths: Vec<PathBuf>,
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
    Ok(LoadedWorkspace {
        workspace,
        paths,
        texts,
    })
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
        } else if file_type.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == "plumb")
        {
            output.push(normalize(&path));
        }
    }
    Ok(())
}

struct QueryPlan {
    program: Program,
}

impl QueryPlan {
    fn compile(source: &str) -> Result<Self, String> {
        Ok(Self {
            program: Program::compile(source)
                .map_err(|error| format!("invalid CEL query: {error}"))?,
        })
    }

    fn matches(
        &self,
        root: &Path,
        path: &Path,
        workspace: &Workspace,
        reverse: &ReverseReferences,
    ) -> Result<bool, String> {
        let entry = workspace
            .get(path)
            .ok_or_else(|| format!("document is not loaded: {}", path.display()))?;
        let mut context = Context::default();
        context.add_variable_from_value("path", display_path(root, path));
        context.add_variable_from_value(
            "title",
            entry
                .current
                .as_ref()
                .and_then(|current| current.output.metadata.document_title())
                .unwrap_or_default(),
        );
        context.add_variable_from_value(
            "directly_referenced_by",
            reverse
                .direct(path)
                .iter()
                .map(|path| display_path(root, path))
                .collect::<Vec<_>>(),
        );
        context.add_variable_from_value(
            "transitively_referenced_by",
            reverse
                .transitive(path)
                .iter()
                .map(|path| display_path(root, path))
                .collect::<Vec<_>>(),
        );
        match self.program.execute(&context) {
            Ok(Value::Bool(value)) => Ok(value),
            Ok(value) => Err(format!("CEL query must return bool, got {value:?}")),
            Err(ExecutionError::NoSuchKey(_)) => Ok(false),
            Err(error) => Err(format!(
                "cannot evaluate query for {}: {error}",
                path.display()
            )),
        }
    }
}

struct ReverseReferences {
    direct: HashMap<PathBuf, HashSet<PathBuf>>,
}

impl ReverseReferences {
    fn build(workspace: &Workspace) -> Self {
        let mut direct: HashMap<PathBuf, HashSet<PathBuf>> = HashMap::new();
        for entry in workspace.documents() {
            let Some(current) = &entry.current else {
                continue;
            };
            for link in &current.output.links {
                let target = match workspace.resolve_link(&entry.path, link) {
                    ResolvedTarget::Anchor { path, .. } | ResolvedTarget::Document { path } => path,
                    _ => continue,
                };
                direct.entry(target).or_default().insert(entry.path.clone());
            }
        }
        Self { direct }
    }

    fn direct(&self, path: &Path) -> Vec<PathBuf> {
        sorted(self.direct.get(path).into_iter().flatten().cloned())
    }

    fn transitive(&self, path: &Path) -> Vec<PathBuf> {
        let mut found = HashSet::new();
        let mut queue = VecDeque::from(self.direct(path));
        while let Some(source) = queue.pop_front() {
            if source == path || !found.insert(source.clone()) {
                continue;
            }
            queue.extend(self.direct(&source));
        }
        sorted(found)
    }
}

fn sorted(values: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut values = values.into_iter().collect::<Vec<_>>();
    values.sort();
    values
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

    use super::*;

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
        std::fs::write(
            root.join("index.plumb"),
            "`link[topic]{to=\"topic.plumb\"}\n",
        )
        .unwrap();
        std::fs::write(root.join("topic.plumb"), "`link[leaf]{to=\"leaf.plumb\"}\n").unwrap();
        std::fs::write(root.join("leaf.plumb"), "Leaf note.\n").unwrap();
        let loaded = load_workspace(&root).unwrap();
        let reverse = ReverseReferences::build(&loaded.workspace);
        let leaf = normalize(&root.join("leaf.plumb"));
        assert!(
            QueryPlan::compile("'index.plumb' in transitively_referenced_by")
                .unwrap()
                .matches(&root, &leaf, &loaded.workspace, &reverse)
                .unwrap()
        );
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
        let reverse = ReverseReferences::build(&loaded.workspace);
        let semantics = normalize(&root.join("docs/semantics.plumb"));
        assert!(
            QueryPlan::compile("path.startsWith('docs/') && title.matches('Semantics Guide')")
                .unwrap()
                .matches(&root, &semantics, &loaded.workspace, &reverse)
                .unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_non_boolean_query_results() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("note.plumb"), "A note.\n").unwrap();
        let loaded = load_workspace(&root).unwrap();
        let reverse = ReverseReferences::build(&loaded.workspace);
        let note = normalize(&root.join("note.plumb"));
        let error = QueryPlan::compile("path")
            .unwrap()
            .matches(&root, &note, &loaded.workspace, &reverse)
            .unwrap_err();
        assert!(error.contains("must return bool"));
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
