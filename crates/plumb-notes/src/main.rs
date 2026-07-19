use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cel::{Context, ExecutionError, Program, Value};
use clap::{Args, Parser, Subcommand};
use plumb_workspace::{normalize, ResolvedTarget, Workspace};

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
        Command::Note(_) => {
            let plan = config
                .query
                .as_deref()
                .map(QueryPlan::compile)
                .transpose()?;
            let reverse = ReverseReferences::build(&loaded.workspace);
            for path in &loaded.paths {
                let matches = match &plan {
                    Some(plan) => plan.matches(&root, path, &loaded.workspace, &reverse)?,
                    None => true,
                };
                if matches {
                    println!("{}", display_path(&root, path));
                }
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
}

#[derive(Debug, Args)]
struct NoteConfig {}

struct LoadedWorkspace {
    workspace: Workspace,
    paths: Vec<PathBuf>,
}

fn load_workspace(root: &Path) -> Result<LoadedWorkspace, String> {
    let root = normalize(root);
    let mut paths = Vec::new();
    collect_plumb_files(&root, &mut paths)?;
    paths.sort();
    let mut workspace = Workspace::new();
    for path in &paths {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        workspace.insert(path, 0, text);
    }
    Ok(LoadedWorkspace { workspace, paths })
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
        let output = entry.current.as_ref().map(|current| &current.output);
        let title = output
            .and_then(|output| output.metadata.as_ref())
            .and_then(|metadata| metadata.table.get("title"))
            .and_then(|title| title.as_str())
            .unwrap_or_default();
        let mut context = Context::default();
        context.add_variable_from_value("path", display_path(root, path));
        context.add_variable_from_value("title", title.to_string());
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
    fn queries_title_and_transitive_referrers() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("index.plumb"),
            "`link[topic]{to=\"topic.plumb\"}\n",
        )
        .unwrap();
        std::fs::write(root.join("topic.plumb"), "`link[leaf]{to=\"leaf.plumb\"}\n").unwrap();
        std::fs::write(
            root.join("leaf.plumb"),
            "`\"{.metadata}\n  title = \"Leaf Note\"\n",
        )
        .unwrap();
        let loaded = load_workspace(&root).unwrap();
        let reverse = ReverseReferences::build(&loaded.workspace);
        let leaf = normalize(&root.join("leaf.plumb"));
        assert!(QueryPlan::compile(
            "title == 'Leaf Note' && 'index.plumb' in transitively_referenced_by"
        )
        .unwrap()
        .matches(&root, &leaf, &loaded.workspace, &reverse)
        .unwrap());
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
        let error = QueryPlan::compile("title")
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
