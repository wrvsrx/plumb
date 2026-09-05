use std::collections::HashSet;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::DateTime;
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use plumb_semantics::{
    analyze_citations, analyze_document, analyze_events, analyze_headings, analyze_inline_styles,
    analyze_lists, analyze_math, analyze_metadata, analyze_quotes, analyze_tables, analyze_tasks,
    green_event_title_completion_context,
};
use plumb_syntax::{
    parse, parse_incremental, GreenDocument as ProductionGreenDocument, SourceChange,
};
use plumb_workspace::{
    search_score, BatchIndexOptions, ExportedSemanticChange, SearchRecordKind, SqliteSemanticStore,
    TaskPageQuery, TaskQueryFilter, TaskQueryFilterGroup, TaskRef, TaskSortOrder, Workspace,
};

#[path = "../benchmark_support.rs"]
mod benchmark_support;
mod green_tree_prototype;

use benchmark_support::semantic_store_workload as workload;
use green_tree_prototype::{validate_revisions as validate_green_revisions, GreenDocument};

struct SqliteFixture {
    _directory: tempfile::TempDir,
    database: PathBuf,
    store: SqliteSemanticStore,
    workspace: Workspace,
}

fn task_document_source(document: usize, tasks: usize, suffix: &str) -> String {
    let mut source = format!("`= title Task document {document}{suffix}\n\n");
    for task in 0..tasks {
        let id = format!("task-{document:03}-{task:02}");
        source.push_str(&format!(
            "`- Task {document:03}/{task:02}{suffix}\n `+ task\n\n `@ {id}\n\n `= priority {}\n `= due 2026-08-{:02}T10:00:00Z\n",
            (document + task) % 31,
            (document + task) % 28 + 1,
        ));
        if task > 0 {
            source.push_str(&format!(
                " `= depends #task-{document:03}-{:02}\n",
                task - 1
            ));
        }
    }
    source.push_str(&format!(
        "\n`- 09:00 Review {document:03}{suffix}\n `+ event\n\n `= date 2026-08-28\n `= timezone +00:00\n `= tasks #task-{document:03}-00\n"
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
            "\n `= priority ",
            "\n `= done 2026-08-28T12:00:00Z\n `= priority ",
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

fn selective_done_task_fixture(documents: usize, tasks: usize) -> SqliteFixture {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("semantic.sqlite3");
    let store = SqliteSemanticStore::open(&database).unwrap();
    let mut workspace = Workspace::with_sqlite_store(store.clone());
    for document in 0..documents {
        let path = format!("selective-done-{document:03}.plumb");
        let mut source = task_document_source(document, tasks, "");
        if document % 8 == 0 {
            source = source.replace(
                "\n `= priority ",
                "\n `= done 2026-08-28T12:00:00Z\n `= priority ",
            );
        }
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

    let selective_done = selective_done_task_fixture(document_count, tasks_per_document);
    let mut candidate_done_query = done_page_query.clone();
    candidate_done_query.filter_groups[0].filters[0].expression = "state == 'done'".to_string();
    let mut full_scan_done_query = candidate_done_query.clone();
    full_scan_done_query.filter_groups[0].filters[0].expression = "state in ['done']".to_string();
    let candidate_done_page = selective_done
        .workspace
        .query_task_page(&candidate_done_query)
        .unwrap()
        .value;
    let full_scan_done_page = selective_done
        .workspace
        .query_task_page(&full_scan_done_query)
        .unwrap()
        .value;
    assert_eq!(candidate_done_page.tasks, full_scan_done_page.tasks);
    assert_eq!(candidate_done_page.complete, full_scan_done_page.complete);
    assert_eq!(candidate_done_page.tasks.len(), 104);

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
    group.bench_function("sqlite_selective_done_full_fact_scan", |b| {
        b.iter(|| {
            black_box(
                selective_done
                    .workspace
                    .query_task_page(&full_scan_done_query),
            )
        });
    });
    group.bench_function("sqlite_selective_done_sql_candidate", |b| {
        b.iter(|| {
            black_box(
                selective_done
                    .workspace
                    .query_task_page(&candidate_done_query),
            )
        });
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
    let changed = changed_event_title(&source, source.len() / 2);
    let changed_start = source
        .bytes()
        .zip(changed.bytes())
        .position(|(old, new)| old != new)
        .unwrap();
    let changed_source = SourceChange {
        old_range: changed_start..changed_start + 1,
        new_range: changed_start..changed_start + 1,
    };
    let mut previous_workspace = Workspace::new();
    previous_workspace.insert("events.plumb", 1, source.clone());
    let mut incremental_workspace = previous_workspace.clone();
    let incremental_pending = incremental_workspace
        .begin_document_revision_with_change(
            "events.plumb",
            2,
            changed.clone(),
            Some(changed_source.clone()),
        )
        .unwrap();
    assert!(incremental_workspace.install_document_analysis(incremental_pending.clone().analyze()));
    let fresh = parse(changed.clone());
    let fresh_output = analyze_document(fresh.valid_syntax().unwrap());
    assert_eq!(
        incremental_workspace
            .get("events.plumb")
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .output
            .as_ref(),
        &fresh_output
    );
    let warm_changed = changed.replacen("Event 167", "Changed event 167", 1);
    let warm_start = changed.find("Event 167").unwrap();
    let warm_change = SourceChange {
        old_range: warm_start..warm_start + "Event 167".len(),
        new_range: warm_start..warm_start + "Changed event 167".len(),
    };
    let warm_pending = incremental_workspace
        .begin_document_revision_with_change(
            "events.plumb",
            3,
            warm_changed.clone(),
            Some(warm_change),
        )
        .unwrap();
    let mut warm_workspace = incremental_workspace.clone();
    assert!(warm_workspace.install_document_analysis(warm_pending.clone().analyze()));
    let warm_fresh = parse(warm_changed);
    let warm_fresh = analyze_document(warm_fresh.valid_syntax().unwrap());
    assert_eq!(
        warm_workspace
            .get("events.plumb")
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .output
            .as_ref(),
        &warm_fresh
    );
    group.bench_function("did_change_parse_stage", |b| {
        b.iter_batched(
            || {
                (
                    previous_workspace.clone(),
                    changed.clone(),
                    changed_source.clone(),
                )
            },
            |(mut workspace, source, change)| {
                black_box(workspace.begin_document_revision_with_change(
                    "events.plumb",
                    2,
                    source,
                    Some(change),
                ))
            },
            BatchSize::LargeInput,
        );
    });
    group.bench_function("background_semantic_stage", |b| {
        b.iter(|| black_box(pending.clone().analyze()))
    });
    group.bench_function("incremental_semantic_tree", |b| {
        b.iter(|| black_box(warm_pending.clone().analyze()))
    });
    group.bench_function("incremental_revision_pipeline", |b| {
        b.iter_batched(
            || {
                (
                    previous_workspace.clone(),
                    changed.clone(),
                    changed_source.clone(),
                )
            },
            |(mut workspace, source, change)| {
                let pending = workspace
                    .begin_document_revision_with_change("events.plumb", 2, source, Some(change))
                    .unwrap();
                let prepared = pending.analyze();
                black_box(workspace.install_document_analysis(prepared))
            },
            BatchSize::LargeInput,
        )
    });
    let mut heading_source = String::new();
    for index in 0..10_000 {
        heading_source.push_str(&format!("`# Heading {index}\n\nBody {index}\n\n"));
    }
    let mut heading_workspace = Workspace::new();
    heading_workspace.insert("headings.plumb", 1, heading_source.clone());
    let heading_start = heading_source.find("Body 5000").unwrap();
    let heading_changed = heading_source.replacen("Body 5000", "Changed body 5000", 1);
    let heading_pending = heading_workspace
        .begin_document_revision_with_change(
            "headings.plumb",
            2,
            heading_changed,
            Some(SourceChange {
                old_range: heading_start..heading_start + "Body 5000".len(),
                new_range: heading_start..heading_start + "Changed body 5000".len(),
            }),
        )
        .unwrap();
    group.bench_function("heading_incremental_semantic_tree", |b| {
        b.iter(|| black_box(heading_pending.clone().analyze()))
    });
    let mut diagnostic_source = String::new();
    for index in 0..10_000 {
        diagnostic_source.push_str(&format!(
            "`- Task {index}\n `+ task\n `= priority invalid\n\n"
        ));
    }
    let mut diagnostic_workspace = Workspace::new();
    diagnostic_workspace.insert("diagnostics.plumb", 1, diagnostic_source.clone());
    let diagnostic_start = diagnostic_source.find("Task 5000").unwrap();
    let diagnostic_changed = diagnostic_source.replacen("Task 5000", "Changed task 5000", 1);
    let diagnostic_pending = diagnostic_workspace
        .begin_document_revision_with_change(
            "diagnostics.plumb",
            2,
            diagnostic_changed,
            Some(SourceChange {
                old_range: diagnostic_start..diagnostic_start + "Task 5000".len(),
                new_range: diagnostic_start..diagnostic_start + "Changed task 5000".len(),
            }),
        )
        .unwrap();
    group.bench_function("diagnostic_incremental_semantic_tree", |b| {
        b.iter(|| black_box(diagnostic_pending.clone().analyze()))
    });
    let mut root_diagnostic_source = String::new();
    for index in 0..10_000 {
        root_diagnostic_source.push_str(&format!(
            "Invalid {index}: `->\"https://example.test/bad path\"\n\n"
        ));
    }
    let mut root_diagnostic_workspace = Workspace::new();
    root_diagnostic_workspace.insert("root-diagnostics.plumb", 1, root_diagnostic_source.clone());
    let root_diagnostic_start = root_diagnostic_source.find("Invalid 5000").unwrap();
    let root_diagnostic_changed =
        root_diagnostic_source.replacen("Invalid 5000", "Changed invalid 5000", 1);
    let root_diagnostic_pending = root_diagnostic_workspace
        .begin_document_revision_with_change(
            "root-diagnostics.plumb",
            2,
            root_diagnostic_changed,
            Some(SourceChange {
                old_range: root_diagnostic_start..root_diagnostic_start + "Invalid 5000".len(),
                new_range: root_diagnostic_start
                    ..root_diagnostic_start + "Changed invalid 5000".len(),
            }),
        )
        .unwrap();
    group.bench_function("root_diagnostic_incremental_semantic_tree", |b| {
        b.iter(|| black_box(root_diagnostic_pending.clone().analyze()))
    });
    let completion_cursor = source.find("Event 16756").unwrap() + "Event 16756".len();
    let completion_green = previous_workspace
        .get("events.plumb")
        .unwrap()
        .parsed
        .green();
    group.bench_function("green_event_title_completion", |b| {
        b.iter(|| {
            black_box(
                green_event_title_completion_context(completion_green, completion_cursor).unwrap(),
            )
        })
    });
    let formatting_entry = previous_workspace.get("events.plumb").unwrap();
    let materialized_formatting = formatting_entry.parsed.green().materialize();
    group.bench_function("green_document_format", |b| {
        b.iter(|| black_box(plumb_edit::format_green(formatting_entry.parsed.green()).unwrap()))
    });
    group.bench_function("materialized_document_format", |b| {
        b.iter(|| {
            black_box(
                plumb_edit::format(&materialized_formatting, plumb_edit::FormatScope::Document)
                    .unwrap(),
            )
        })
    });
    let range_selection = formatting_entry
        .current
        .as_ref()
        .unwrap()
        .output
        .events()
        .events
        .get(16_756)
        .unwrap()
        .range;
    group.bench_function("green_range_format", |b| {
        b.iter(|| {
            black_box(
                plumb_edit::format_green_contained(
                    formatting_entry.parsed.green(),
                    range_selection.clone(),
                )
                .unwrap(),
            )
        })
    });
    group.bench_function("materialized_range_format", |b| {
        b.iter(|| {
            black_box(
                plumb_edit::format(
                    &materialized_formatting,
                    plumb_edit::FormatScope::ContainedBlocks(range_selection.clone()),
                )
                .unwrap(),
            )
        })
    });
    group.finish();
}

fn changed_event_title(source: &str, around: usize) -> String {
    let start = source[..around.min(source.len())]
        .rfind("Event ")
        .or_else(|| {
            source[around.min(source.len())..]
                .find("Event ")
                .map(|at| at + around)
        })
        .expect("event title near benchmark position");
    let mut changed = source.to_string();
    changed.replace_range(start..start + 1, "e");
    changed
}

fn benchmark_incremental_parse(c: &mut Criterion) {
    validate_green_revisions();
    let count = 33_512;
    let (_, source) = workload(count, count / 10, "");
    let previous = parse(source.clone());
    let green_previous = GreenDocument::parse(source.clone());
    let production_green_previous = ProductionGreenDocument::parse(source.clone());
    eprintln!("green_tree_shards/33512: {}", green_previous.shard_count());
    let mut group = c.benchmark_group("incremental_parse_33512");
    group.sample_size(10);
    for (position, around) in [
        ("start", source.len() / 100),
        ("middle", source.len() / 2),
        ("end", source.len() * 99 / 100),
    ] {
        let changed = changed_event_title(&source, around);
        let changed_start = source
            .bytes()
            .zip(changed.bytes())
            .position(|(old, new)| old != new)
            .expect("benchmark edit changes one byte");
        let changed_range = changed_start..changed_start + 1;
        let source_change = SourceChange {
            old_range: changed_range.clone(),
            new_range: changed_range.clone(),
        };
        let incremental = parse_incremental(&previous, changed.clone());
        let fresh = parse(changed.clone());
        assert_eq!(incremental.document, fresh);
        let green = green_previous.reparse(changed.clone());
        assert_eq!(green.materialize(), fresh);
        eprintln!(
            "green_tree_reuse/{position}: {}/{} shards, {} reparsed bytes",
            green.reused_shards_from(&green_previous),
            green.shard_count(),
            green.reparsed_bytes()
        );
        let reparsed_bytes = incremental.reparsed_range.end - incremental.reparsed_range.start;
        assert!(reparsed_bytes < source.len());
        group.bench_with_input(
            BenchmarkId::new("fresh", position),
            &changed,
            |b, changed| {
                b.iter_batched(
                    || changed.clone(),
                    |changed| black_box(parse(changed)),
                    BatchSize::LargeInput,
                )
            },
        );
        group.bench_with_input(
            BenchmarkId::new("incremental", format!("{position}-{reparsed_bytes}-bytes")),
            &changed,
            |b, changed| {
                b.iter_batched(
                    || changed.clone(),
                    |changed| black_box(parse_incremental(&previous, changed)),
                    BatchSize::LargeInput,
                )
            },
        );
        group.bench_with_input(
            BenchmarkId::new("green_revision", position),
            &changed,
            |b, changed| {
                b.iter_batched(
                    || changed.clone(),
                    |changed| black_box(green_previous.reparse(changed)),
                    BatchSize::LargeInput,
                )
            },
        );
        group.bench_with_input(
            BenchmarkId::new("green_revision_known_edit", position),
            &changed,
            |b, changed| {
                b.iter_batched(
                    || changed.clone(),
                    |changed| {
                        black_box(green_previous.reparse_changed(changed, changed_range.clone()))
                    },
                    BatchSize::LargeInput,
                )
            },
        );
        group.bench_with_input(
            BenchmarkId::new("production_green_revision_known_edit", position),
            &changed,
            |b, changed| {
                b.iter_batched(
                    || changed.clone(),
                    |changed| {
                        black_box(
                            production_green_previous
                                .reparse_from_change(changed, source_change.clone()),
                        )
                    },
                    BatchSize::LargeInput,
                )
            },
        );
        group.bench_function(BenchmarkId::new("green_materialize_owned", position), |b| {
            b.iter_batched(
                || (),
                |()| black_box(green.materialize()),
                BatchSize::LargeInput,
            )
        });
        group.bench_function(
            BenchmarkId::new("green_materialize_and_drop", position),
            |b| b.iter(|| drop(black_box(green.materialize()))),
        );
        group.bench_function(BenchmarkId::new("green_fresh_build", position), |b| {
            b.iter_batched(
                || changed.clone(),
                |changed| black_box(GreenDocument::parse(changed)),
                BatchSize::LargeInput,
            )
        });
        group.bench_function(BenchmarkId::new("production_green_fresh", position), |b| {
            b.iter_batched(
                || changed.clone(),
                |changed| black_box(ProductionGreenDocument::parse(changed)),
                BatchSize::LargeInput,
            )
        });
        let production_green = production_green_previous
            .reparse_from_change(changed.clone(), source_change.clone())
            .document;
        assert_eq!(production_green.materialize(), fresh);
        group.bench_function(
            BenchmarkId::new("production_green_materialize_owned", position),
            |b| {
                b.iter_batched(
                    || (),
                    |()| black_box(production_green.materialize()),
                    BatchSize::LargeInput,
                )
            },
        );
        group.bench_with_input(
            BenchmarkId::new("green_revision_and_materialize", position),
            &changed,
            |b, changed| {
                b.iter_batched(
                    || changed.clone(),
                    |changed| {
                        let green = green_previous.reparse(changed);
                        black_box(green.materialize())
                    },
                    BatchSize::LargeInput,
                )
            },
        );
    }
    group.finish();
}

fn benchmark_semantic_components(c: &mut Criterion) {
    let count = 33_512;
    let (_, source) = workload(count, count / 10, "");
    let parsed = parse(source);
    let valid = parsed.valid_syntax().unwrap();
    let metadata = analyze_metadata(valid);
    let mut group = c.benchmark_group("semantic_components_33512");
    group.sample_size(10);
    group.bench_function("headings", |b| {
        b.iter(|| black_box(analyze_headings(valid)))
    });
    group.bench_function("metadata", |b| {
        b.iter(|| black_box(analyze_metadata(valid)))
    });
    group.bench_function("citations", |b| {
        b.iter(|| black_box(analyze_citations(valid)))
    });
    group.bench_function("inline_styles", |b| {
        b.iter(|| black_box(analyze_inline_styles(valid)))
    });
    group.bench_function("lists", |b| b.iter(|| black_box(analyze_lists(valid))));
    group.bench_function("math", |b| b.iter(|| black_box(analyze_math(valid))));
    group.bench_function("quotes", |b| b.iter(|| black_box(analyze_quotes(valid))));
    group.bench_function("tasks", |b| b.iter(|| black_box(analyze_tasks(valid))));
    group.bench_function("events", |b| {
        b.iter(|| black_box(analyze_events(valid, &metadata)))
    });
    group.bench_function("tables", |b| b.iter(|| black_box(analyze_tables(valid))));
    group.bench_function("document", |b| {
        b.iter(|| black_box(analyze_document(valid)))
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
            "`- 2026-08-28T10:00:00Z Outer {index} `->{{outer target.plumb#task}}\n `+ event\n `- 2026-08-28T11:00:00Z Nested {index} `->{{nested target.plumb#task}}\n  `+ event\n"
        ));
    }
    source
}

fn benchmark_event_containment(c: &mut Criterion) {
    let source = event_containment_source(2_000);
    let parsed = parse(&source);
    let output = analyze_document(parsed.valid_syntax().unwrap());
    let event = output.events().events.get(1_000).unwrap();
    let links = output.links().iter().collect::<Vec<_>>();
    let legacy = || {
        let first = links.partition_point(|link| link.range.start < event.range.start);
        links[first..]
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
                output.event_link_ranges().len()
                    * std::mem::size_of::<plumb_semantics::EventLinkRange>(),
            )
        })
    });
    group.finish();
}

fn benchmark_export_record_lookup(c: &mut Criterion) {
    let mut source = String::new();
    for index in 0..2_000 {
        source.push_str(&format!(
            "See `->{{{{Guide {index}}} guide-{index}.plumb}} `!{{strong {index}}} `cite{{citation-{index}}}.\n\n"
        ));
    }
    c.bench_function("export_record_lookup_2000", |b| {
        b.iter(|| black_box(plumb_export::export(black_box(&source)).unwrap()))
    });
}

fn benchmark_semantic_equality_publication(c: &mut Criterion) {
    let mut task_source = String::new();
    for index in 0..10_000 {
        task_source.push_str(&format!(
            "`- Task {index}\n `+ task\n `@ task-{index}\n{}\n",
            (index > 0)
                .then(|| format!(" `= depends #task-{}\n", index - 1))
                .unwrap_or_default()
        ));
    }
    let mut base = Workspace::new();
    base.insert("tasks.plumb", 1, task_source);
    base.insert("note.plumb", 1, "`# Alpha\n\nBody.\n");

    let revise = |workspace: &mut Workspace, revision: i64| {
        let source = if revision % 2 == 0 {
            "`# Bravo\n\nChanged body.\n"
        } else {
            "`# Alpha\n\nBody.\n"
        };
        let prepared = workspace
            .begin_document_revision("note.plumb", revision, source)
            .unwrap()
            .analyze();
        workspace
            .install_document_analysis_with_change(prepared)
            .unwrap()
    };

    let mut unconditional = base.clone();
    let mut unconditional_revision = 2;
    c.bench_function(
        "semantic_equal_publication_10000/unconditional_context",
        |b| {
            b.iter(|| {
                assert_eq!(
                    revise(&mut unconditional, unconditional_revision),
                    ExportedSemanticChange::Unchanged
                );
                unconditional_revision += 1;
                black_box(unconditional.diagnostic_context().unwrap())
            })
        },
    );

    let mut guarded = base;
    let mut guarded_revision = 2;
    c.bench_function(
        "semantic_equal_publication_10000/exported_summary_guard",
        |b| {
            b.iter(|| {
                let change = revise(&mut guarded, guarded_revision);
                guarded_revision += 1;
                if change == ExportedSemanticChange::Changed {
                    black_box(guarded.diagnostic_context().unwrap());
                }
                black_box(change)
            })
        },
    );
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
        benchmark_incremental_parse, benchmark_semantic_components, benchmark_batch_index,
        benchmark_event_containment, benchmark_export_record_lookup,
        benchmark_semantic_equality_publication
}
criterion_main!(benches);
