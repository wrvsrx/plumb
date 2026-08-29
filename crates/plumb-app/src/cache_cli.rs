use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use plumb_workspace::{
    inspect_cache_namespace, prune_cache_namespace, CacheNamespaceState, CachePruneOutcome,
};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn run(args: Vec<OsString>) -> ExitCode {
    match args.as_slice() {
        [command] if command == "status" => status(),
        [command] if command == "prune" => prune(false),
        [command, option] if command == "prune" && option == "--all" => prune(true),
        [argument] if argument == "--help" || argument == "-h" || argument == "help" => {
            print_help();
            ExitCode::SUCCESS
        }
        [] => {
            eprintln!("plumb cache: missing command\n");
            print_help();
            ExitCode::from(2)
        }
        _ => {
            eprintln!("plumb cache: expected 'status' or 'prune [--all]'");
            ExitCode::from(2)
        }
    }
}

pub(crate) fn cache_base_dir() -> PathBuf {
    std::env::var_os("PLUMB_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(|| std::env::temp_dir().join("plumb-cache"))
}

fn cache_root() -> PathBuf {
    cache_base_dir().join("plumb")
}

fn status() -> ExitCode {
    let root = cache_root();
    let namespaces = match namespaces(&root) {
        Ok(namespaces) => namespaces,
        Err(error) => {
            eprintln!("plumb cache status: {error}");
            return ExitCode::FAILURE;
        }
    };
    println!("root {}", root.display());
    let mut total_databases = 0;
    let mut total_bytes = 0;
    for namespace in namespaces {
        let usage = match inspect_cache_namespace(&namespace.path) {
            Ok(usage) => usage,
            Err(error) => {
                eprintln!(
                    "plumb cache status: cannot inspect {}: {error}",
                    namespace.path.display()
                );
                return ExitCode::FAILURE;
            }
        };
        total_databases += usage.databases;
        total_bytes += usage.bytes;
        println!(
            "{} {} {} databases={} files={} bytes={}",
            namespace.kind,
            namespace.version,
            state_name(usage.state),
            usage.databases,
            usage.files,
            usage.bytes
        );
    }
    let (legacy_files, legacy_bytes) = match legacy_usage(&root) {
        Ok(usage) => usage,
        Err(error) => {
            eprintln!("plumb cache status: {error}");
            return ExitCode::FAILURE;
        }
    };
    if legacy_files > 0 {
        println!("legacy unmanaged files={legacy_files} bytes={legacy_bytes}");
    }
    println!(
        "total databases={} bytes={} unmanaged_files={}",
        total_databases, total_bytes, legacy_files
    );
    ExitCode::SUCCESS
}

fn prune(include_current: bool) -> ExitCode {
    let root = cache_root();
    let namespaces = match namespaces(&root) {
        Ok(namespaces) => namespaces,
        Err(error) => {
            eprintln!("plumb cache prune: {error}");
            return ExitCode::FAILURE;
        }
    };
    let mut removed_files = 0;
    let mut removed_bytes = 0;
    let mut preserved = 0;
    let mut active = 0;
    let mut unmanaged = 0;
    for namespace in namespaces {
        if !include_current && namespace.version == CURRENT_VERSION {
            preserved += 1;
            continue;
        }
        match prune_cache_namespace(&namespace.path) {
            Ok(CachePruneOutcome::Pruned { files, bytes }) => {
                removed_files += files;
                removed_bytes += bytes;
            }
            Ok(CachePruneOutcome::Active) => active += 1,
            Ok(CachePruneOutcome::Unmanaged) => unmanaged += 1,
            Err(error) => {
                eprintln!(
                    "plumb cache prune: cannot prune {}: {error}",
                    namespace.path.display()
                );
                return ExitCode::FAILURE;
            }
        }
    }
    let (legacy_files, _) = match legacy_usage(&root) {
        Ok(usage) => usage,
        Err(error) => {
            eprintln!("plumb cache prune: {error}");
            return ExitCode::FAILURE;
        }
    };
    unmanaged += usize::from(legacy_files > 0);
    println!(
        "removed_files={removed_files} removed_bytes={removed_bytes} preserved={preserved} active={active} unmanaged={unmanaged}"
    );
    ExitCode::SUCCESS
}

fn print_help() {
    println!(
        "Inspect or prune semantic caches\n\nUsage: plumb cache <COMMAND>\n\nCommands:\n  status       Report cache namespaces and sizes\n  prune        Remove inactive non-current namespaces\n  prune --all  Also consider the current version"
    );
}

struct Namespace {
    kind: &'static str,
    version: String,
    path: PathBuf,
}

fn namespaces(root: &Path) -> std::io::Result<Vec<Namespace>> {
    let mut namespaces = Vec::new();
    for kind in ["workspaces", "site"] {
        let parent = root.join(kind);
        let entries = match fs::read_dir(&parent) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                namespaces.push(Namespace {
                    kind,
                    version: entry.file_name().to_string_lossy().into_owned(),
                    path: entry.path(),
                });
            }
        }
    }
    namespaces.sort_by(|left, right| {
        left.kind
            .cmp(right.kind)
            .then_with(|| left.version.cmp(&right.version))
    });
    Ok(namespaces)
}

fn legacy_usage(root: &Path) -> std::io::Result<(usize, u64)> {
    let mut files = 0;
    let mut bytes = 0;
    for kind in ["workspaces", "site"] {
        let parent = root.join(kind);
        let entries = match fs::read_dir(parent) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                files += 1;
                bytes += entry.metadata()?.len();
            }
        }
    }
    Ok((files, bytes))
}

fn state_name(state: CacheNamespaceState) -> &'static str {
    match state {
        CacheNamespaceState::Active => "active",
        CacheNamespaceState::Inactive => "inactive",
        CacheNamespaceState::Unmanaged => "unmanaged",
    }
}
