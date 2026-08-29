use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

use plumb_workspace::{scan_workspace_files, Workspace};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

#[test]
fn documentation_index_reaches_every_docs_document() {
    let docs = repository_root().join("docs");
    let paths = scan_workspace_files(&docs).into_result().unwrap();
    let expected = paths.iter().cloned().collect::<HashSet<_>>();
    let mut workspace = Workspace::new();
    for path in &paths {
        workspace.insert(path, 0, std::fs::read_to_string(path).unwrap());
    }

    let index = docs.join("index.plumb");
    let mut reached = HashSet::from([index.clone()]);
    let mut pending = VecDeque::from([index]);
    while let Some(path) = pending.pop_front() {
        for target in workspace.referenced_documents_from(&path).unwrap().value {
            if expected.contains(&target) && reached.insert(target.clone()) {
                pending.push_back(target);
            }
        }
    }

    let mut missing = expected.difference(&reached).collect::<Vec<_>>();
    missing.sort();
    assert!(
        missing.is_empty(),
        "docs/index.plumb cannot reach: {}",
        missing
            .iter()
            .map(|path| path.strip_prefix(&docs).unwrap().display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
}
