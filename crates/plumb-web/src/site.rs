use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use serde_json::json;

use crate::server::{render_backlinks, render_index, render_note_page, write_assets};
use crate::{render_note_html, GraphQuery, WebTargetMode, WebWorkspace};

#[derive(Debug, Parser)]
#[command(name = "plumb site", about = "Build a static plumb workspace site")]
struct SiteConfig {
    #[command(subcommand)]
    command: SiteCommand,
}

#[derive(Debug, Subcommand)]
enum SiteCommand {
    /// Build a static site containing notes and the workspace graph.
    Build(BuildConfig),
}

#[derive(Debug, Args)]
struct BuildConfig {
    /// Directory to scan recursively. Defaults to the current directory.
    #[arg(long, value_name = "DIR")]
    root: Option<PathBuf>,

    /// Directory to write. It must be absent or empty.
    #[arg(long, value_name = "DIR", required = true)]
    output: PathBuf,
}

pub fn run_site_cli(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let config = match SiteConfig::try_parse_from(args) {
        Ok(config) => config,
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(error.exit_code() as u8);
        }
    };
    let result = match config.command {
        SiteCommand::Build(config) => build(config),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("plumb site: {error}");
            ExitCode::FAILURE
        }
    }
}

fn build(config: BuildConfig) -> Result<(), String> {
    let root = config
        .root
        .unwrap_or(std::env::current_dir().map_err(|error| error.to_string())?);
    ensure_empty_output(&config.output)?;
    let workspace = WebWorkspace::load(root)?;
    std::fs::create_dir_all(&config.output)
        .map_err(|error| format!("cannot create {}: {error}", config.output.display()))?;
    write_assets(&config.output.join("assets"))?;

    let graph = workspace.graph(&GraphQuery {
        limit: Some(20_000),
        ..GraphQuery::default()
    });
    write_json(config.output.join("graph.json"), &graph)?;
    let search = graph
        .nodes
        .iter()
        .filter(|node| !node.unresolved)
        .map(|node| json!({ "id": node.id, "title": node.title, "path": node.path }))
        .collect::<Vec<_>>();
    write_json(config.output.join("search-index.json"), &search)?;

    let index_config = json!({
        "mode": "static",
        "graphUrl": "graph.json",
        "noteApiBase": "notes/",
        "noteApiSuffix": "/note.json",
        "notePageBase": "notes/",
        "notePageSuffix": "/",
        "eventsUrl": null,
        "current": null,
    });
    std::fs::write(
        config.output.join("index.html"),
        render_index(&index_config, "assets/", "./"),
    )
    .map_err(|error| format!("cannot write index.html: {error}"))?;

    for node in graph.nodes.iter().filter(|node| !node.unresolved) {
        let Some(note) = workspace.note(&node.id) else {
            continue;
        };
        let html = render_note_html(&workspace, &node.id, WebTargetMode::StaticNote)?;
        let directory = config.output.join("notes").join(&node.id);
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
        write_json(
            directory.join("note.json"),
            &json!({
                "id": note.id,
                "title": note.title,
                "path": note.path,
                "revision": note.revision,
                "location": note.location,
                "backlinks": note.backlinks,
                "html": html,
            }),
        )?;
        let backlinks = render_backlinks(&workspace, &note.backlinks, "../../notes/", "/");
        let page = render_note_page(
            &note.title,
            &note.path,
            &node.id,
            &html,
            &backlinks,
            "../../assets/",
            "../../",
        );
        std::fs::write(directory.join("index.html"), page)
            .map_err(|error| format!("cannot write note page: {error}"))?;
    }

    for resource in workspace.resources() {
        let directory = config.output.join("resources").join(&resource.id);
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
        std::fs::copy(&resource.path, directory.join(&resource.name)).map_err(|error| {
            format!("cannot copy resource {}: {error}", resource.path.display())
        })?;
    }
    Ok(())
}

fn ensure_empty_output(output: &PathBuf) -> Result<(), String> {
    if !output.exists() {
        return Ok(());
    }
    if !output.is_dir() {
        return Err(format!("output is not a directory: {}", output.display()));
    }
    if std::fs::read_dir(output)
        .map_err(|error| format!("cannot inspect {}: {error}", output.display()))?
        .next()
        .is_some()
    {
        return Err(format!(
            "output directory is not empty: {}",
            output.display()
        ));
    }
    Ok(())
}

fn write_json(path: PathBuf, value: &impl serde::Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("cannot encode {}: {error}", path.display()))?;
    std::fs::write(&path, bytes)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    #[test]
    fn builds_static_graph_notes_resources_and_relative_links() {
        if std::process::Command::new("pandoc")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let root = temp_dir("source");
        let output = temp_dir("output");
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/image.png"), b"png").unwrap();
        std::fs::write(
            root.join("a.plumb"),
            "See `->[B]{to=\"b.plumb#b\"}.\n`img[x]{src=\"assets/image.png\"}\n",
        )
        .unwrap();
        std::fs::write(root.join("b.plumb"), "`#{#b} B\n").unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let a_id = workspace
            .document_id(root.join("a.plumb"))
            .unwrap()
            .to_string();
        let b_id = workspace
            .document_id(root.join("b.plumb"))
            .unwrap()
            .to_string();
        let resource = workspace.resources().next().unwrap().clone();
        build(BuildConfig {
            root: Some(root.clone()),
            output: output.clone(),
        })
        .unwrap();
        assert!(output.join("index.html").is_file());
        assert!(output.join("graph.json").is_file());
        assert!(output.join("assets/vendor/force-graph.min.js").is_file());
        assert!(output
            .join("assets/vendor/FORCE-GRAPH-LICENSE.txt")
            .is_file());
        let note =
            std::fs::read_to_string(output.join("notes").join(&a_id).join("note.json")).unwrap();
        assert!(note.contains(&format!("../../notes/{b_id}/#b")), "{note}");
        assert!(
            note.contains(&format!("../../resources/{}/image%2Epng", resource.id)),
            "{note}"
        );
        assert!(output
            .join("resources")
            .join(&resource.id)
            .join("image.png")
            .is_file());
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(output).unwrap();
    }

    fn temp_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "plumb-web-site-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
