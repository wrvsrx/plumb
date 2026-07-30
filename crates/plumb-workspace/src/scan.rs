use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::{normalize, WORKSPACE_MARKER};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkspaceScan {
    pub files: Vec<PathBuf>,
    pub errors: Vec<String>,
}

impl WorkspaceScan {
    pub fn is_complete(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn into_result(self) -> Result<Vec<PathBuf>, String> {
        if self.errors.is_empty() {
            Ok(self.files)
        } else {
            Err(self.errors.join("\n"))
        }
    }
}

pub fn discover_workspace_root(start: impl AsRef<Path>) -> PathBuf {
    let start = normalize(start.as_ref());
    let directory = if start.is_file() {
        start.parent().unwrap_or(&start)
    } else {
        &start
    };
    directory
        .ancestors()
        .find(|directory| directory.join(WORKSPACE_MARKER).is_dir())
        .map(normalize)
        .unwrap_or_else(|| normalize(directory))
}

pub fn resolve_workspace_root(explicit: Option<&Path>) -> Result<PathBuf, String> {
    let current = std::env::current_dir()
        .map_err(|error| format!("cannot read current directory: {error}"))?;
    let root = resolve_workspace_root_from(explicit, &current);
    root.is_dir()
        .then_some(root.clone())
        .ok_or_else(|| format!("workspace root is not a directory: {}", root.display()))
}

pub(crate) fn resolve_workspace_root_from(explicit: Option<&Path>, current: &Path) -> PathBuf {
    match explicit {
        Some(root) if root.is_absolute() => normalize(root),
        Some(root) => normalize(&current.join(root)),
        None => discover_workspace_root(current),
    }
}

pub fn scan_workspace_files(root: impl AsRef<Path>) -> WorkspaceScan {
    let root = normalize(root.as_ref());
    let mut builder = WalkBuilder::new(&root);
    builder
        .hidden(false)
        .parents(false)
        .ignore(true)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .follow_links(false);
    let mut scan = WorkspaceScan::default();
    for result in builder.build() {
        match result {
            Ok(entry) if is_scannable_plumb_file(&entry) => {
                scan.files.push(normalize(entry.path()))
            }
            Ok(_) => {}
            Err(error) => scan.errors.push(error.to_string()),
        }
    }
    scan.files.sort();
    scan.files.dedup();
    scan.errors.sort();
    scan
}

fn is_scannable_plumb_file(entry: &ignore::DirEntry) -> bool {
    entry
        .path()
        .extension()
        .is_some_and(|extension| extension == "plumb")
        && entry
            .file_type()
            .is_some_and(|kind| kind.is_file() || kind.is_symlink())
        && entry.path().is_file()
}
