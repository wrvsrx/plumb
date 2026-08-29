use std::collections::HashSet;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::DateTime;
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use plumb_semantics::analyze_document;
use plumb_syntax::parse;
use plumb_workspace::{
    search_score, BatchIndexOptions, SearchRecordKind, SqliteSemanticStore, TaskPageQuery,
    TaskQueryFilter, TaskQueryFilterGroup, TaskRef, TaskSortOrder, Workspace,
};

#[path = "../benchmark_support.rs"]
mod benchmark_support;

use benchmark_support::semantic_store_workload as workload;

struct SqliteFixture {
    _directory: tempfile::TempDir,
    database: PathBuf,
    store: SqliteSemanticStore,
    workspace: Workspace,
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
    let mut persistent = Workspace::with_sqlite_store(store.clone());
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
            store,
            workspace: persistent,
        },
    )
}

fn done_task_fixture(documents: usize, tasks: usize) -> SqliteFixture {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("semantic.sqlite3");
    let store = SqliteSemanticStore::open(&database).unwrap();
    let mut workspace = Workspace::with_sqlite_store(store.clone());
    for document in 0..documents {
        let path = format!("done-{document:03}.plumb");
        let source = task_document_source(document, tasks, "").replace(
            "\n `= priority|",
            "\n `= done|2026-08-28T12:00:00Z\n `= priority|",
        );
        workspace.insert_disk(&path, 0, &source).unwrap();
    }
    SqliteFixture {
        _directory: directory,
        database,
        store,
        workspace,
    }
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
    let mut workspace = Workspace::with_sqlite_store(store.clone());
    workspace.insert_disk("target.plumb", 0, target).unwrap();
    workspace.insert_disk("migrated.plumb", 0, source).unwrap();
    SqliteFixture {
        _directory: directory,
        database,
        store,
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
    group.bench_function("sqlite_event_title_prefix", |b| {
        b.iter(|| black_box(sqlite.store.event_title_counts("Event 12", &[])));
    });
    group.bench_function("sqlite_event_full_decode", |b| {
        b.iter(|| black_box(sqlite.store.events(&[])));
    });
    group.bench_function("sqlite_event_legacy_decode_score_limit", |b| {
        b.iter(|| {
            let mut matches = sqlite
                .store
                .events(&[])
                .unwrap()
                .into_iter()
                .filter_map(|stored| {
                    let relative_path = stored.path.display().to_string();
                    let id = stored.record.id.as_ref().map(|field| field.value.as_str());
                    search_score(
                        "Event 12",
                        &[
                            stored.record.title.as_str(),
                            id.unwrap_or_default(),
                            &relative_path,
                        ],
                    )
                    .map(|score| (score, relative_path, stored.record))
                })
                .collect::<Vec<_>>();
            matches.sort_by(|left, right| {
                right
                    .0
                    .cmp(&left.0)
                    .then_with(|| left.1.cmp(&right.1))
                    .then_with(|| left.2.range.start.cmp(&right.2.range.start))
            });
            matches.truncate(50);
            black_box(matches)
        });
    });
    group.bench_function("sqlite_event_search_selected_decode", |b| {
        b.iter(|| {
            black_box(sqlite.workspace.search_records(
                Path::new(""),
                Some(SearchRecordKind::Event),
                "Event 12",
                50,
                start,
            ))
        });
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

    let mut all_tasks_query = page_query.clone();
    all_tasks_query.text.clear();
    all_tasks_query.filter_groups.clear();
    all_tasks_query.sort = vec![TaskSortOrder::Source];
    all_tasks_query.limit = usize::MAX;
    assert_eq!(
        sqlite
            .workspace
            .query_task_page(&all_tasks_query)
            .unwrap()
            .value
            .tasks
            .len(),
        document_count * tasks_per_document
    );
    assert_eq!(
        sqlite
            .workspace
            .task_document_metrics()
            .unwrap()
            .value
            .len(),
        document_count
    );

    let done = done_task_fixture(document_count, tasks_per_document);
    let mut done_full_query = page_query.clone();
    done_full_query.text.clear();
    done_full_query.filter_groups = vec![TaskQueryFilterGroup {
        filters: vec![TaskQueryFilter {
            source: "benchmark:done".to_string(),
            expression: "state == 'done'".to_string(),
        }],
    }];
    done_full_query.sort = vec![TaskSortOrder::Source];
    done_full_query.limit = usize::MAX;
    let done_full = done
        .workspace
        .query_task_page(&done_full_query)
        .unwrap()
        .value;
    assert_eq!(done_full.tasks.len(), document_count * tasks_per_document);
    let mut done_page_query = done_full_query.clone();
    done_page_query.limit = 100;
    let done_page = done
        .workspace
        .query_task_page(&done_page_query)
        .unwrap()
        .value;
    assert!(!done_page.complete);
    assert!(done_page.next_cursor.is_some());
    assert!(done_page.tasks.len() < done_full.tasks.len());
    eprintln!(
        "semantic_store_done_task_page/requested_{}_decoded_full_records: {}",
        done_page_query.limit,
        done_page.tasks.len()
    );

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
    group.bench_function("sqlite_all_tasks_full", |b| {
        b.iter(|| black_box(sqlite.workspace.query_task_page(&all_tasks_query)));
    });
    group.bench_function("sqlite_task_document_metrics", |b| {
        b.iter(|| black_box(sqlite.workspace.task_document_metrics()));
    });
    group.bench_function("sqlite_done_full", |b| {
        b.iter(|| black_box(done.workspace.query_task_page(&done_full_query)));
    });
    group.bench_function("sqlite_done_first_page", |b| {
        b.iter(|| black_box(done.workspace.query_task_page(&done_page_query)));
    });
    group.finish();
}

fn benchmark_diagnostic_round(c: &mut Criterion) {
    let (_, mut sqlite) = task_fixtures(176, 8);
    let open_paths = (0..8)
        .map(|document| PathBuf::from(format!("tasks-{document:03}.plumb")))
        .collect::<Vec<_>>();
    for (document, path) in open_paths.iter().enumerate() {
        sqlite
            .workspace
            .open_document(path, 1, task_document_source(document, 8, " current"));
    }
    let mut group = c.benchmark_group("workspace_diagnostic_round_8_of_176");
    group.sample_size(10);
    group.bench_function("rebuild_context_per_document", |b| {
        b.iter(|| {
            for path in &open_paths {
                black_box(sqlite.workspace.diagnostics(path).unwrap());
            }
        });
    });
    group.bench_function("shared_context", |b| {
        b.iter(|| {
            let context = sqlite.workspace.diagnostic_context().unwrap();
            for path in &open_paths {
                black_box(
                    sqlite
                        .workspace
                        .diagnostics_with_context(path, &context)
                        .unwrap(),
                );
            }
        });
    });
    group.finish();
}

fn benchmark_open_document_generation(c: &mut Criterion) {
    let count = 33_512;
    let (_, source) = workload(count, count / 10, "");
    let mut parsed_workspace = Workspace::new();
    let pending = parsed_workspace
        .begin_document_revision("events.plumb", 1, source.clone())
        .unwrap();
    let mut group = c.benchmark_group("open_document_generation_33512");
    group.sample_size(10);
    group.bench_function("synchronous_insert", |b| {
        b.iter_batched(
            || source.clone(),
            |source| {
                let mut workspace = Workspace::new();
                let revision = workspace.insert("events.plumb", 1, source).revision;
                black_box(revision)
            },
            BatchSize::LargeInput,
        );
    });
    group.bench_function("did_change_parse_stage", |b| {
        b.iter_batched(
            || source.clone(),
            |source| {
                let mut workspace = Workspace::new();
                black_box(workspace.begin_document_revision("events.plumb", 1, source))
            },
            BatchSize::LargeInput,
        );
    });
    group.bench_function("background_semantic_stage", |b| {
        b.iter(|| black_box(pending.clone().analyze()))
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
        benchmark_task_queries, benchmark_diagnostic_round, benchmark_open_document_generation,
        benchmark_batch_index, benchmark_event_containment
}
criterion_main!(benches);
