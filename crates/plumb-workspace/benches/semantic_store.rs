use std::collections::HashSet;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::DateTime;
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use plumb_semantics::analyze_document;
use plumb_syntax::parse;
use plumb_workspace::{
    BatchIndexOptions, SqliteSemanticStore, TaskPageQuery, TaskQueryFilter, TaskQueryFilterGroup,
    TaskRef, TaskSortOrder, Workspace,
};

struct SqliteFixture {
    _directory: tempfile::TempDir,
    database: PathBuf,
    workspace: Workspace,
}

fn workload(events: usize, references: usize, suffix: &str) -> (String, String) {
    let target = "`= title|Target\n\n`- Target\n\n `+ task\n\n `@ target\n".to_string();
    let mut source = String::with_capacity(events * 90 + references * 55);
    source.push_str("`= title|Migrated events\n`= timezone|Z\n\n");
    for index in 0..events {
        let day = index % 28 + 1;
        let hour = index % 24;
        source.push_str(&format!(
            "`- 2026-08-{day:02}T{hour:02}:00|Event {index}{suffix}\n\n `+ event\n\n `@ event-{index}\n\n"
        ));
    }
    for _ in 0..references {
        source.push_str("See `->[target|target.plumb#target].\n");
    }
    (target, source)
}

fn task_document_source(document: usize, tasks: usize, suffix: &str) -> String {
    let mut source = format!("`= title|Task document {document}{suffix}\n\n");
    for task in 0..tasks {
        let id = format!("task-{document:03}-{task:02}");
        source.push_str(&format!(
            "`- Task {document:03}/{task:02}{suffix}\n\n `+ task\n\n `@ {id}\n\n `= priority|{}\n `= due|2026-08-{:02}T10:00:00Z\n",
            (document + task) % 31,
            (document + task) % 28 + 1,
        ));
        if task > 0 {
            source.push_str(&format!(
                " `= depends|#task-{document:03}-{:02}\n",
                task - 1
            ));
        }
    }
    source.push_str(&format!(
        "\n`- 09:00|Review {document:03}{suffix}\n\n `+ event\n\n `= date|2026-08-28\n `= timezone|+00:00\n `= tasks|#task-{document:03}-00\n"
    ));
    source
}

fn task_fixtures(documents: usize, tasks: usize) -> (Workspace, SqliteFixture) {
    let mut memory = Workspace::new();
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("semantic.sqlite3");
    let store = SqliteSemanticStore::open(&database).unwrap();
    let mut persistent = Workspace::with_sqlite_store(store);
    for document in 0..documents {
        let path = format!("tasks-{document:03}.plumb");
        let source = task_document_source(document, tasks, "");
        memory.insert(&path, 0, &source);
        persistent.insert_disk(&path, 0, &source).unwrap();
    }
    (
        memory,
        SqliteFixture {
            _directory: directory,
            database,
            workspace: persistent,
        },
    )
}

fn batch_index_files(documents: usize, tasks: usize) -> (tempfile::TempDir, Vec<PathBuf>) {
    let directory = tempfile::tempdir().unwrap();
    let mut paths = Vec::with_capacity(documents);
    for document in 0..documents {
        let path = directory.path().join(format!("tasks-{document:03}.plumb"));
        std::fs::write(&path, task_document_source(document, tasks, "")).unwrap();
        paths.push(path);
    }
    (directory, paths)
}

fn task_page_query() -> TaskPageQuery {
    TaskPageQuery {
        root: PathBuf::new(),
        text: "Task".to_string(),
        filter_groups: vec![TaskQueryFilterGroup {
            filters: vec![TaskQueryFilter {
                source: "benchmark".to_string(),
                expression: "priority != null && priority >= 0".to_string(),
            }],
        }],
        sort: vec![TaskSortOrder::Priority, TaskSortOrder::Due],
        limit: 50,
        cursor: None,
        workspace_revision: 1,
        now: DateTime::parse_from_rfc3339("2026-08-28T00:00:00Z").unwrap(),
    }
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

fn benchmark_task_queries(c: &mut Criterion) {
    let document_count = 176;
    let tasks_per_document = 8;
    let (memory, sqlite) = task_fixtures(document_count, tasks_per_document);
    let page_query = task_page_query();
    let memory_page = memory.query_task_page(&page_query).unwrap().value;
    let sqlite_page = sqlite.workspace.query_task_page(&page_query).unwrap().value;
    assert_eq!(sqlite_page, memory_page);
    eprintln!(
        "semantic_store_task_page/requested_{}_decoded_full_records: {}",
        page_query.limit,
        sqlite_page.tasks.len()
    );

    let mut relation_query = page_query.clone();
    relation_query.text.clear();
    relation_query.filter_groups = vec![TaskQueryFilterGroup {
        filters: vec![TaskQueryFilter {
            source: "benchmark:relation".to_string(),
            expression: "directly_blocking.size() > 0".to_string(),
        }],
    }];
    let relation_page = sqlite
        .workspace
        .query_task_page(&relation_query)
        .unwrap()
        .value;
    assert!(!relation_page.tasks.is_empty());

    let target = TaskRef {
        path: PathBuf::from("tasks-000.plumb"),
        id: "task-000-00".to_string(),
    };
    assert_eq!(
        sqlite
            .workspace
            .events_for_task(&target)
            .unwrap()
            .value
            .len(),
        1
    );

    let overlay_source = task_document_source(0, tasks_per_document, " open");
    let mut memory_overlay = memory.clone();
    memory_overlay.open_document("tasks-000.plumb", 1, &overlay_source);
    let mut sqlite_overlay = sqlite.workspace.clone();
    sqlite_overlay.open_document("tasks-000.plumb", 1, &overlay_source);
    assert_eq!(
        sqlite_overlay.query_task_page(&page_query).unwrap().value,
        memory_overlay.query_task_page(&page_query).unwrap().value
    );

    let mut group = c.benchmark_group("semantic_store_task_queries_1408");
    group.sample_size(20);
    group.bench_function("memory_filtered_cursor_page", |b| {
        b.iter(|| black_box(memory.query_task_page(&page_query)));
    });
    group.bench_function("sqlite_filtered_cursor_page", |b| {
        b.iter(|| black_box(sqlite.workspace.query_task_page(&page_query)));
    });
    group.bench_function("sqlite_reverse_relation_filter", |b| {
        b.iter(|| black_box(sqlite.workspace.query_task_page(&relation_query)));
    });
    group.bench_function("sqlite_event_task_lookup", |b| {
        b.iter(|| black_box(sqlite.workspace.events_for_task(&target)));
    });
    group.bench_function("sqlite_open_overlay_page", |b| {
        b.iter(|| black_box(sqlite_overlay.query_task_page(&page_query)));
    });
    group.finish();
}

fn benchmark_batch_index(c: &mut Criterion) {
    let document_count = 176;
    let tasks_per_document = 8;
    let (_sources, paths) = batch_index_files(document_count, tasks_per_document);
    let mut group = c.benchmark_group("workspace_batch_index_1408");
    group.sample_size(10);
    group.bench_function("memory_serial_cold", |b| {
        b.iter(|| {
            let mut workspace = Workspace::new();
            for path in &paths {
                workspace.insert(path, 0, std::fs::read_to_string(path).unwrap());
            }
            black_box(workspace)
        })
    });
    group.bench_function("memory_cold", |b| {
        b.iter(|| {
            let mut workspace = Workspace::new();
            black_box(
                workspace
                    .index_disk_files(
                        &paths,
                        BatchIndexOptions {
                            prune_missing: true,
                            retain_sources: false,
                        },
                        |_| 0,
                        || false,
                    )
                    .unwrap(),
            )
        })
    });
    group.bench_function("sqlite_serial_cold", |b| {
        b.iter_batched(
            || tempfile::tempdir().unwrap(),
            |directory| {
                let store =
                    SqliteSemanticStore::open(directory.path().join("semantic.sqlite3")).unwrap();
                let mut workspace = Workspace::with_sqlite_store(store);
                for path in &paths {
                    workspace
                        .insert_disk(path, 0, std::fs::read_to_string(path).unwrap())
                        .unwrap();
                }
                black_box(workspace)
            },
            BatchSize::LargeInput,
        )
    });
    group.bench_function("sqlite_cold", |b| {
        b.iter_batched(
            || tempfile::tempdir().unwrap(),
            |directory| {
                let store =
                    SqliteSemanticStore::open(directory.path().join("semantic.sqlite3")).unwrap();
                let mut workspace = Workspace::with_sqlite_store(store);
                black_box(
                    workspace
                        .index_disk_files(
                            &paths,
                            BatchIndexOptions {
                                prune_missing: true,
                                retain_sources: false,
                            },
                            |_| 0,
                            || false,
                        )
                        .unwrap(),
                )
            },
            BatchSize::LargeInput,
        )
    });

    let cache = tempfile::tempdir().unwrap();
    let database = cache.path().join("semantic.sqlite3");
    let store = SqliteSemanticStore::open(&database).unwrap();
    let mut workspace = Workspace::with_sqlite_store(store);
    workspace
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
    group.bench_function("sqlite_serial_warm", |b| {
        b.iter(|| {
            let store = SqliteSemanticStore::open(&database).unwrap();
            let mut workspace = Workspace::with_sqlite_store(store);
            for path in &paths {
                assert!(workspace
                    .insert_disk(path, 0, std::fs::read_to_string(path).unwrap())
                    .unwrap());
            }
            black_box(workspace)
        })
    });
    group.bench_function("sqlite_warm", |b| {
        b.iter(|| {
            let store = SqliteSemanticStore::open(&database).unwrap();
            let mut workspace = Workspace::with_sqlite_store(store);
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
            assert_eq!(result.cache_hits(), paths.len());
            black_box(result)
        })
    });
    group.finish();
}

fn event_containment_source(events: usize) -> String {
    let mut source = String::with_capacity(events * 180);
    for index in 0..events {
        source.push_str(&format!(
            "`- 2026-08-28T10:00:00Z|Outer {index} `->[outer|target.plumb#task]\n\n `+ event\n\n `- 2026-08-28T11:00:00Z|Nested {index} `->[nested|target.plumb#task]\n\n  `+ event\n"
        ));
    }
    source
}

fn benchmark_event_containment(c: &mut Criterion) {
    let source = event_containment_source(2_000);
    let parsed = parse(&source);
    let output = analyze_document(parsed.valid_syntax().unwrap());
    let event = &output.events.events[1_000];
    let legacy = || {
        let first = output
            .links
            .partition_point(|link| link.range.start < event.range.start);
        output.links[first..]
            .iter()
            .take_while(|link| link.range.start < event.range.end)
            .count()
    };

    let mut group = c.benchmark_group("event_containment_4000");
    group.bench_function("analysis_build", |b| {
        b.iter(|| {
            let parsed = parse(black_box(&source));
            black_box(analyze_document(parsed.valid_syntax().unwrap()))
        })
    });
    group.bench_function("legacy_binary_range_query", |b| {
        b.iter(|| black_box(legacy()))
    });
    group.bench_function("indexed_range_query", |b| {
        b.iter(|| {
            black_box(
                output
                    .links_contained_by_event(black_box(event.range.start))
                    .unwrap()
                    .len(),
            )
        })
    });
    group.bench_function("index_memory_bytes", |b| {
        b.iter(|| {
            black_box(
                output.event_link_ranges.capacity()
                    * std::mem::size_of::<plumb_semantics::EventLinkRange>(),
            )
        })
    });
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
    targets = benchmark_build, benchmark_warm_start, benchmark_queries, benchmark_replacement,
        benchmark_task_queries, benchmark_batch_index, benchmark_event_containment
}
criterion_main!(benches);
