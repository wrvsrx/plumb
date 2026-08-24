use std::collections::HashSet;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::DateTime;
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use plumb_workspace::{SqliteSemanticStore, Workspace};

struct SqliteFixture {
    _directory: tempfile::TempDir,
    database: PathBuf,
    workspace: Workspace,
}

fn workload(events: usize, references: usize, suffix: &str) -> (String, String) {
    let target = "{\n `: title Target\n}\n\n`task Target {\n `@ target\n}\n".to_string();
    let mut source = String::with_capacity(events * 90 + references * 55);
    source.push_str("{\n `: title Migrated events\n `: timezone Z\n}\n\n");
    for index in 0..events {
        let day = index % 28 + 1;
        let hour = index % 24;
        source.push_str(&format!(
            "`event 2026-08-{day:02}T{hour:02}:00 Event {index}{suffix} {{\n `@ event-{index}\n}}\n\n"
        ));
    }
    for _ in 0..references {
        source.push_str("See `->[target][target.plumb#target].\n");
    }
    (target, source)
}

fn memory_workspace(target: &str, source: &str) -> Workspace {
    let mut workspace = Workspace::new();
    workspace.insert("target.plumb", 0, target);
    workspace.insert("migrated.plumb", 0, source);
    workspace
}

fn sqlite_fixture(target: &str, source: &str) -> SqliteFixture {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("semantic.sqlite3");
    let store = SqliteSemanticStore::open(&database).unwrap();
    let mut workspace = Workspace::with_sqlite_store(store);
    workspace.insert_disk("target.plumb", 0, target).unwrap();
    workspace.insert_disk("migrated.plumb", 0, source).unwrap();
    SqliteFixture {
        _directory: directory,
        database,
        workspace,
    }
}

fn benchmark_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("semantic_store_build");
    group.sample_size(10);
    for count in [1_000, 10_000, 33_512] {
        let (target, source) = workload(count, count / 10, "");
        group.bench_with_input(BenchmarkId::new("memory_cold", count), &count, |b, _| {
            b.iter(|| black_box(memory_workspace(&target, &source)));
        });
        group.bench_with_input(BenchmarkId::new("sqlite_cold", count), &count, |b, _| {
            b.iter_batched(
                || tempfile::tempdir().unwrap(),
                |directory| {
                    let store =
                        SqliteSemanticStore::open(directory.path().join("semantic.sqlite3"))
                            .unwrap();
                    let mut workspace = Workspace::with_sqlite_store(store);
                    workspace.insert_disk("target.plumb", 0, &target).unwrap();
                    workspace.insert_disk("migrated.plumb", 0, &source).unwrap();
                    black_box(workspace)
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn benchmark_warm_start(c: &mut Criterion) {
    let mut group = c.benchmark_group("semantic_store_warm_start");
    group.sample_size(20);
    for count in [1_000, 10_000, 33_512] {
        let (target, source) = workload(count, count / 10, "");
        let fixture = sqlite_fixture(&target, &source);
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, _| {
            b.iter(|| {
                let store = SqliteSemanticStore::open(&fixture.database).unwrap();
                let mut workspace = Workspace::with_sqlite_store(store);
                assert!(workspace.insert_disk("target.plumb", 0, &target).unwrap());
                assert!(workspace.insert_disk("migrated.plumb", 0, &source).unwrap());
                black_box(workspace)
            });
        });
    }
    group.finish();
}

fn benchmark_queries(c: &mut Criterion) {
    let count = 33_512;
    let (target, source) = workload(count, 3_351, "");
    let memory = memory_workspace(&target, &source);
    let sqlite = sqlite_fixture(&target, &source);
    let ids = HashSet::from(["target".to_string()]);
    let start = DateTime::parse_from_rfc3339("2026-08-10T00:00:00+00:00").unwrap();
    let end = DateTime::parse_from_rfc3339("2026-08-20T00:00:00+00:00").unwrap();
    let database_bytes = [
        sqlite.database.clone(),
        PathBuf::from(format!("{}-wal", sqlite.database.display())),
        PathBuf::from(format!("{}-shm", sqlite.database.display())),
    ]
    .iter()
    .filter_map(|path| std::fs::metadata(path).ok())
    .map(|metadata| metadata.len())
    .sum::<u64>();
    eprintln!("semantic_store_database_bytes/{count}: {database_bytes}");
    let mut group = c.benchmark_group("semantic_store_queries_33512");
    group.sample_size(30);
    group.bench_function("memory_reverse_references", |b| {
        b.iter(|| black_box(memory.reverse_references_for_document("target.plumb", &ids)));
    });
    group.bench_function("sqlite_reverse_references", |b| {
        b.iter(|| {
            black_box(
                sqlite
                    .workspace
                    .reverse_references_for_document("target.plumb", &ids),
            )
        });
    });
    group.bench_function("memory_agenda_range", |b| {
        b.iter(|| black_box(memory.events_overlapping(start, end)));
    });
    group.bench_function("sqlite_agenda_range", |b| {
        b.iter(|| black_box(sqlite.workspace.events_overlapping(start, end)));
    });
    group.bench_function("sqlite_open_overlay", |b| {
        b.iter_batched(
            || sqlite.workspace.clone(),
            |mut workspace| {
                workspace.open_document("migrated.plumb", 1, "No references.\n");
                black_box(workspace.reverse_references_for_document("target.plumb", &ids))
            },
            BatchSize::LargeInput,
        );
    });
    group.finish();
}

fn benchmark_replacement(c: &mut Criterion) {
    let mut group = c.benchmark_group("semantic_store_replacement");
    group.sample_size(10);
    for count in [1_000, 10_000, 33_512] {
        let (target, old_source) = workload(count, count / 10, "");
        let (_, new_source) = workload(count, count / 10, " updated");
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, _| {
            b.iter_batched(
                || sqlite_fixture(&target, &old_source),
                |mut fixture| {
                    assert!(!fixture
                        .workspace
                        .insert_disk(Path::new("migrated.plumb"), 0, &new_source)
                        .unwrap());
                    black_box(fixture)
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn configuration() -> Criterion {
    Criterion::default()
        .measurement_time(Duration::from_secs(5))
        .warm_up_time(Duration::from_secs(1))
}

criterion_group! {
    name = benches;
    config = configuration();
    targets = benchmark_build, benchmark_warm_start, benchmark_queries, benchmark_replacement
}
criterion_main!(benches);
