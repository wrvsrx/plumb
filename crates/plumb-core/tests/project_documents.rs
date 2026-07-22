use std::fs;
use std::path::{Path, PathBuf};

use plumb_core::parse;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn collect_plumb_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let mut entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", directory.display()))
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|error| panic!("failed to enumerate {}: {error}", directory.display()));
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            collect_plumb_files(&path, files);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "plumb")
        {
            files.push(path);
        }
    }
}

fn assert_valid_plumb(label: &str, source: String) {
    let parsed = parse(source);
    assert!(
        parsed.is_valid(),
        "{label} is not valid plumb: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn project_plumb_documents_are_strictly_valid() {
    let root = repository_root();
    let mut files = Vec::new();
    for directory in ["docs", "examples", "contrib"] {
        collect_plumb_files(&root.join(directory), &mut files);
    }
    files.push(root.join("tree-sitter-plumb/README.plumb"));

    assert!(!files.is_empty(), "project document set must not be empty");
    for path in files {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let label = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        assert_valid_plumb(&label, source);
    }
}

#[test]
fn guide_does_not_duplicate_project_status() {
    let guide = repository_root().join("docs/guide");
    let mut files = Vec::new();
    collect_plumb_files(&guide, &mut files);

    let forbidden = ["当前限制", "尚未", "未实现", "TODO", "deferred"];
    for path in files {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        for phrase in forbidden {
            assert!(
                !source.contains(phrase),
                "{} duplicates project status with forbidden phrase {phrase:?}; record progress in docs/project/roadmap.plumb or tasks.plumb",
                path.display()
            );
        }
    }
}

#[test]
fn bundled_skill_plumb_examples_are_strictly_valid() {
    let skill = repository_root().join("skills/plumb-markup");
    let mut markdown_files = Vec::new();
    collect_markdown_files(&skill, &mut markdown_files);

    let mut example_count = 0;
    for path in markdown_files {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        for (index, example) in plumb_fences(&source).into_iter().enumerate() {
            example_count += 1;
            assert_valid_plumb(
                &format!("{} plumb fence {}", path.display(), index + 1),
                example,
            );
        }
    }
    assert!(
        example_count > 0,
        "bundled skill must contain plumb examples"
    );
}

fn collect_markdown_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let mut entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", directory.display()))
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|error| panic!("failed to enumerate {}: {error}", directory.display()));
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_markdown_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "md") {
            files.push(path);
        }
    }
}

fn plumb_fences(source: &str) -> Vec<String> {
    let mut examples: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    for line in source.lines() {
        if let Some(example) = &mut current {
            if line == "```" {
                examples.push(current.take().expect("open fence has content buffer"));
            } else {
                example.push_str(line);
                example.push('\n');
            }
        } else if line == "```plumb" {
            current = Some(String::new());
        }
    }
    assert!(current.is_none(), "unclosed plumb Markdown fence");
    examples
}
