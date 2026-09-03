use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use plumb_syntax::parse;

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
fn docs_have_explicit_titles() {
    let root = repository_root();
    let mut files = Vec::new();
    collect_plumb_files(&root.join("docs"), &mut files);

    for path in files {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{} must be valid", path.display());
        assert!(
            parsed.syntax.attrs.value("title").is_some(),
            "{} must contain explicit title metadata",
            path.strip_prefix(&root).unwrap_or(&path).display()
        );
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

#[test]
fn bundled_skill_tracks_current_standard_spellings() {
    let root = repository_root().join("skills/plumb-markup");
    let skill = fs::read_to_string(root.join("SKILL.md")).unwrap();
    let semantics = fs::read_to_string(root.join("references/standard-semantics.md")).unwrap();

    for required in [
        "`+ task` or `+ event` facets",
        "Letter prefixes such as `t`/`task` and `e`/`event` offer no",
        "`(){container `+{notice}}",
        "`->{same-file target #intro}",
    ] {
        assert!(
            skill.contains(required) || semantics.contains(required),
            "bundled skill omits current spelling {required:?}"
        );
    }
    for obsolete in [
        "A task uses the specialized `task` marker",
        "An event uses the specialized `event` marker",
        "type a backtick followed by `task`",
    ] {
        assert!(
            !skill.contains(obsolete) && !semantics.contains(obsolete),
            "bundled skill still advertises obsolete spelling {obsolete:?}"
        );
    }
}

#[test]
fn workspace_uses_only_the_current_parser_and_keeps_tests_enabled() {
    let root = repository_root();
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(&root)
        .output()
        .expect("run cargo metadata");
    assert!(output.status.success());
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let package_names = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|package| package["name"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        package_names
            .iter()
            .copied()
            .filter(|name| name.starts_with("plumb-syntax"))
            .collect::<Vec<_>>(),
        ["plumb-syntax"]
    );
    assert!(!package_names.contains(&"plumb-migrate"));

    let mut rust_files = Vec::new();
    collect_files_with_extension(&root.join("crates"), "rs", &mut rust_files);
    let disabled_cfg = ["cfg(", "any())"].concat();
    for path in rust_files {
        let source = fs::read_to_string(&path).unwrap();
        assert!(
            !source.contains(&disabled_cfg),
            "{} silently disables code or tests with an always-false cfg",
            path.display()
        );
    }
}

#[test]
fn editing_adapters_do_not_depend_directly_on_the_formatter() {
    let root = repository_root();
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(&root)
        .output()
        .expect("run cargo metadata");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse cargo metadata");
    let adapters = ["plumb", "plumb-semantics", "plumb-web", "plumb-workspace"];
    let packages = metadata["packages"].as_array().expect("metadata packages");

    for name in adapters {
        let package = packages
            .iter()
            .find(|package| package["name"] == name)
            .unwrap_or_else(|| panic!("metadata must contain editing adapter {name}"));
        let has_direct_formatter_dependency = package["dependencies"]
            .as_array()
            .expect("package dependencies")
            .iter()
            .any(|dependency| dependency["name"] == "plumb-format" && dependency["kind"].is_null());
        assert!(
            !has_direct_formatter_dependency,
            "editing adapter {name} must route formatting through plumb-edit"
        );
    }
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

fn collect_files_with_extension(directory: &Path, extension: &str, files: &mut Vec<PathBuf>) {
    let mut entries = fs::read_dir(directory)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_files_with_extension(&path, extension, files);
        } else if path
            .extension()
            .is_some_and(|candidate| candidate == extension)
        {
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
