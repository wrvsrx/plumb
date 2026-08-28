use std::hint::black_box;
use std::path::PathBuf;
use std::time::Instant;

use plumb_workspace::{scan_workspace_files, BatchIndexOptions, SqliteSemanticStore, Workspace};

fn main() {
    let mut arguments = std::env::args().skip(1);
    let mode = arguments
        .next()
        .expect("mode: memory-serial, memory-batch, sqlite-serial, or sqlite-batch");
    let root = PathBuf::from(arguments.next().expect("workspace root"));
    let scan_started = Instant::now();
    let paths = scan_workspace_files(&root).into_result().unwrap();
    let scan = scan_started.elapsed();
    let index_started = Instant::now();
    let mut temporary = None;
    let mut workspace = if mode.starts_with("sqlite-") {
        let database = arguments.next().map(PathBuf::from).unwrap_or_else(|| {
            let directory = tempfile::tempdir().unwrap();
            let database = directory.path().join("semantic.sqlite3");
            temporary = Some(directory);
            database
        });
        Workspace::with_sqlite_store(SqliteSemanticStore::open(database).unwrap())
    } else {
        Workspace::new()
    };
    let mut cache_hits = 0;
    match mode.as_str() {
        "memory-serial" => {
            for path in &paths {
                workspace.insert(path, 0, std::fs::read_to_string(path).unwrap());
            }
        }
        "sqlite-serial" => {
            for path in &paths {
                cache_hits += usize::from(
                    workspace
                        .insert_disk(path, 0, std::fs::read_to_string(path).unwrap())
                        .unwrap(),
                );
            }
        }
        "memory-batch" | "sqlite-batch" => {
            let result = workspace
                .index_disk_files(
                    &paths,
                    BatchIndexOptions {
                        prune_missing: true,
                        retain_sources: false,
                    },
                    |_| 0,
                    || false,
                )
                .unwrap();
            assert!(result.is_complete());
            cache_hits = result.cache_hits();
        }
        _ => panic!("unknown workspace index profile mode: {mode}"),
    }
    let index = index_started.elapsed();
    black_box((&workspace, &temporary));
    println!("mode={mode}");
    println!("files={}", paths.len());
    println!("cache_hits={cache_hits}");
    println!("scan_micros={}", scan.as_micros());
    println!("index_micros={}", index.as_micros());
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        if let Some(high_water) = status.lines().find(|line| line.starts_with("VmHWM:")) {
            println!("{high_water}");
        }
        if let Some(resident) = status.lines().find(|line| line.starts_with("VmRSS:")) {
            println!("{resident}");
        }
    }
}
