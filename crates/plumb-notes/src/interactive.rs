use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use skim::prelude::*;

use crate::display_path;
use plumb_workspace::normalize;

pub(crate) enum InteractiveAction {
    Open(Vec<String>),
    Create(String),
}

#[derive(Clone)]
struct FilterItem {
    path: String,
    searchable: String,
    display: String,
    preview: String,
}

impl FilterItem {
    fn new(path: String, text: String) -> Self {
        Self {
            searchable: format!("{path}\n{text}"),
            display: ansi(&path, "1"),
            preview: preview_text(&path, &text),
            path,
        }
    }
}

impl SkimItem for FilterItem {
    fn text(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.searchable)
    }

    fn display<'a>(&'a self, _context: DisplayContext<'a>) -> AnsiString<'a> {
        AnsiString::parse(&self.display)
    }

    fn preview(&self, _context: PreviewContext) -> ItemPreview {
        ItemPreview::AnsiText(self.preview.clone())
    }

    fn output(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.path)
    }
}

pub(crate) fn run_interactive(
    root: &Path,
    paths: &[PathBuf],
    texts: &HashMap<PathBuf, String>,
) -> Result<InteractiveAction, String> {
    let options = SkimOptionsBuilder::default()
        .height(Some("100%"))
        .multi(true)
        .preview(Some(""))
        .bind(vec!["ctrl-n:accept"])
        .build()
        .map_err(|error| error.to_string())?;
    let (sender, receiver): (SkimItemSender, SkimItemReceiver) = unbounded();
    for path in paths {
        let Some(source) = texts.get(path) else {
            continue;
        };
        sender
            .send(Arc::new(FilterItem::new(
                display_path(root, path),
                source.clone(),
            )))
            .map_err(|error| format!("cannot send item to skim: {error}"))?;
    }
    drop(sender);

    let Some(output) = Skim::run_with(&options, Some(receiver)) else {
        return Ok(InteractiveAction::Open(Vec::new()));
    };
    if output.final_key == Key::Ctrl('n') {
        return Ok(InteractiveAction::Create(output.query));
    }
    Ok(InteractiveAction::Open(
        output
            .selected_items
            .into_iter()
            .map(|item| item.output().into_owned())
            .collect(),
    ))
}

pub(crate) fn handle_interactive_action(
    root: &Path,
    action: InteractiveAction,
) -> Result<(), String> {
    let paths = match action {
        InteractiveAction::Open(selected) => selected
            .iter()
            .map(|path| normalize(&root.join(path)))
            .collect::<Vec<_>>(),
        InteractiveAction::Create(query) => vec![create_file_from_query(root, &query)?],
    };
    open_in_editor(&paths)
}

fn open_in_editor(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let editor = std::env::var("EDITOR")
        .map_err(|_| "--interactive selected files, but EDITOR is not set".to_string())?;
    let (program, args) = editor_command(&editor)?;
    let status = Command::new(&program)
        .args(args)
        .args(paths)
        .status()
        .map_err(|error| format!("cannot run editor `{program}`: {error}"))?;
    if !status.success() {
        return Err(format!("editor `{program}` exited with {status}"));
    }
    Ok(())
}

fn create_file_from_query(root: &Path, query: &str) -> Result<PathBuf, String> {
    let name = query.trim();
    if name.is_empty() {
        return Err("cannot create a file from an empty query".to_string());
    }
    let relative = Path::new(name);
    let relative = if relative.extension().is_some_and(|ext| ext == "plumb") {
        relative.to_path_buf()
    } else {
        relative.with_extension("plumb")
    };
    let root = normalize(root);
    let path = normalize(&root.join(relative));
    if !path.starts_with(&root) {
        return Err(format!("new file path escapes root: {name}"));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create directory {}: {error}", parent.display()))?;
    }
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
    Ok(path)
}

fn editor_command(editor: &str) -> Result<(String, Vec<String>), String> {
    let mut parts =
        shlex::split(editor).ok_or_else(|| format!("cannot parse EDITOR={editor:?}"))?;
    if parts.is_empty() {
        return Err("EDITOR is empty".to_string());
    }
    let program = parts.remove(0);
    Ok((program, parts))
}

fn preview_text(path: &str, source: &str) -> String {
    format!(
        "{}\n{}\n{}",
        ansi(path, "1"),
        ansi(&"-".repeat(path.len()), "2"),
        highlight_plumb(source)
    )
}

fn highlight_plumb(source: &str) -> String {
    let mut verbatim_margin = None;
    let mut lines = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if let Some(margin) = verbatim_margin {
            if trimmed.is_empty() {
                lines.push(line.to_string());
                continue;
            }
            if indent >= margin {
                lines.push(ansi(line, "90"));
                continue;
            }
            verbatim_margin = None;
        }
        if trimmed.starts_with("`{") {
            verbatim_margin = Some(indent + 2);
            lines.push(ansi(line, "36"));
        } else if marked_kind(trimmed, "meta") {
            lines.push(ansi(line, "35"));
        } else if trimmed.starts_with("`#") {
            lines.push(ansi(line, "1;34"));
        } else if line.contains("`->[") {
            lines.push(ansi(line, "32"));
        } else {
            lines.push(line.to_string());
        }
    }
    lines.join("\n")
}

fn marked_kind(source: &str, kind: &str) -> bool {
    source
        .strip_prefix('`')
        .and_then(|source| source.strip_prefix(kind))
        .is_some_and(|rest| {
            rest.is_empty()
                || rest.starts_with('{')
                || rest.chars().next().is_some_and(char::is_whitespace)
        })
}

fn ansi(text: &str, code: &str) -> String {
    format!("\x1b[{code}m{text}\x1b[0m")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_root_relative_plumb_file() {
        let root =
            std::env::temp_dir().join(format!("plumb-notes-interactive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let created = create_file_from_query(&root, "notes/topic").unwrap();
        assert_eq!(created, normalize(&root.join("notes/topic.plumb")));
        assert!(create_file_from_query(&root, "../outside").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_editor_arguments_and_highlights_preview() {
        assert_eq!(
            editor_command("'code editor' --wait").unwrap(),
            ("code editor".to_string(), vec!["--wait".to_string()])
        );
        let preview = preview_text("topic.plumb", "`#{#topic} Topic\nSee `->[x]{to=\"x\"}.\n");
        assert!(preview.contains("topic.plumb"));
        assert!(preview.contains("\x1b[1;34m"));
        assert!(preview.contains("\x1b[32m"));
    }

    #[test]
    fn preview_highlights_metadata_and_verbatim_blocks() {
        let preview = highlight_plumb(
            "`meta\n  `: title\n\n    Preview\n\n`{language=rust}\n  fn main() {}\n\n`# Heading\n",
        );
        assert!(preview.contains("\x1b[35m`meta\x1b[0m"));
        assert!(preview.contains("\x1b[36m`{language=rust}\x1b[0m"));
        assert!(preview.contains("\x1b[90m  fn main() {}\x1b[0m"));
        assert!(preview.contains("\x1b[1;34m`# Heading\x1b[0m"));
    }
}
