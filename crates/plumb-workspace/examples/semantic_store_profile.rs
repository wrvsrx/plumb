use std::collections::HashSet;
use std::hint::black_box;
use std::time::Instant;

use chrono::DateTime;
use plumb_workspace::{SqliteSemanticStore, Workspace};

fn main() {
    let mut arguments = std::env::args().skip(1);
    let backend = arguments.next().expect("backend: memory or sqlite");
    let count = arguments
        .next()
        .expect("event count")
        .parse::<usize>()
        .expect("numeric event count");
    let (target, source) = workload(count, count / 10);
    let started = Instant::now();
    let mut database = None;
    let workspace = match backend.as_str() {
        "memory" => {
            let mut workspace = Workspace::new();
            workspace.insert("target.plumb", 0, &target);
            workspace.insert("migrated.plumb", 0, &source);
            workspace
        }
        "sqlite" => {
            let path = arguments.next().map_or_else(
                || tempfile::tempdir().unwrap().keep().join("semantic.sqlite3"),
                std::path::PathBuf::from,
            );
            let store = SqliteSemanticStore::open(&path).unwrap();
            let mut workspace = Workspace::with_sqlite_store(store);
            workspace.insert_disk("target.plumb", 0, &target).unwrap();
            workspace.insert_disk("migrated.plumb", 0, &source).unwrap();
            database = Some(path);
            workspace
        }
        _ => panic!("backend must be memory or sqlite"),
    };
    let build = started.elapsed();
    let ids = HashSet::from(["target".to_string()]);
    let query_started = Instant::now();
    let references = workspace
        .reverse_references_for_document("target.plumb", &ids)
        .expect("reverse-reference query should succeed");
    let start = DateTime::parse_from_rfc3339("2026-08-10T00:00:00+00:00").unwrap();
    let end = DateTime::parse_from_rfc3339("2026-08-20T00:00:00+00:00").unwrap();
    let events = workspace
        .events_overlapping(start, end)
        .expect("event query should succeed");
    let query = query_started.elapsed();
    black_box((&workspace, references, events));
    println!("backend={backend}");
    println!("events={count}");
    println!("source_bytes={}", source.len());
    println!("build_micros={}", build.as_micros());
    println!("queries_micros={}", query.as_micros());
    if let Some(path) = database {
        let bytes = [
            path.clone(),
            path.with_file_name(format!(
                "{}-wal",
                path.file_name().unwrap().to_string_lossy()
            )),
            path.with_file_name(format!(
                "{}-shm",
                path.file_name().unwrap().to_string_lossy()
            )),
        ]
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok())
        .map(|metadata| metadata.len())
        .sum::<u64>();
        println!("database_bytes={bytes}");
    }
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        if let Some(high_water) = status.lines().find(|line| line.starts_with("VmHWM:")) {
            println!("{high_water}");
        }
        if let Some(resident) = status.lines().find(|line| line.starts_with("VmRSS:")) {
            println!("{resident}");
        }
    }
}

fn workload(events: usize, references: usize) -> (String, String) {
    let target = "`= title|Target\n\n`task Target\n `@ target\n".to_string();
    let mut source = String::with_capacity(events * 90 + references * 55);
    source.push_str("`= title|Migrated events\n`= timezone|Z\n\n");
    for index in 0..events {
        let day = index % 28 + 1;
        let hour = index % 24;
        source.push_str(&format!(
            "`event 2026-08-{day:02}T{hour:02}:00|Event {index} {{\n `@ event-{index}\n}}\n\n"
        ));
    }
    for _ in 0..references {
        source.push_str("See `->[target|target.plumb#target].\n");
    }
    (target, source)
}
