use std::collections::HashSet;
use std::hint::black_box;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use plumb_semantics::{analyze_document, DocumentOutput, EventRecord, LinkTarget};
use plumb_syntax::parse;
use plumb_workspace::Workspace;
use rusqlite::{params, Connection, Transaction};

const SOURCE_PATH: &str = "migrated.plumb";
const TARGET_PATH: &str = "target.plumb";

struct TempOverlay {
    connection: Connection,
}

impl TempOverlay {
    fn new(target: &DocumentOutput, disk: &DocumentOutput) -> Self {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "temp_store", "MEMORY")
            .unwrap();
        connection
            .execute_batch(
                "CREATE TABLE documents(path TEXT PRIMARY KEY, revision INTEGER NOT NULL);
                 CREATE TABLE anchors(path TEXT NOT NULL, id TEXT NOT NULL, start INTEGER NOT NULL,
                                      record BLOB NOT NULL);
                 CREATE INDEX anchors_identity ON anchors(path, id);
                 CREATE TABLE links(path TEXT NOT NULL, start INTEGER NOT NULL, end INTEGER NOT NULL,
                                    record BLOB NOT NULL);
                 CREATE INDEX links_source_range ON links(path, start, end);
                 CREATE TABLE semantic_references(
                     source_path TEXT NOT NULL, target_path TEXT NOT NULL, target_id TEXT,
                     source_start INTEGER NOT NULL, source_end INTEGER NOT NULL);
                 CREATE INDEX references_target
                     ON semantic_references(target_path, target_id);
                 CREATE TABLE tasks(path TEXT NOT NULL, id TEXT, start INTEGER NOT NULL,
                                    record BLOB NOT NULL);
                 CREATE INDEX tasks_identity ON tasks(path, id);
                 CREATE TABLE events(
                     path TEXT NOT NULL, start INTEGER NOT NULL, is_point INTEGER NOT NULL,
                     sort_millis INTEGER, interval_start_millis INTEGER,
                     interval_end_millis INTEGER, record BLOB NOT NULL);
                 CREATE INDEX events_time
                     ON events(interval_start_millis, interval_end_millis);

                 CREATE TEMP TABLE open_documents(
                     path TEXT PRIMARY KEY, revision INTEGER NOT NULL);
                 CREATE TEMP TABLE open_anchors(
                     path TEXT NOT NULL, id TEXT NOT NULL, start INTEGER NOT NULL,
                     record BLOB NOT NULL);
                 CREATE INDEX temp.open_anchors_identity ON open_anchors(path, id);
                 CREATE TEMP TABLE open_links(
                     path TEXT NOT NULL, start INTEGER NOT NULL, end INTEGER NOT NULL,
                     record BLOB NOT NULL);
                 CREATE INDEX temp.open_links_source_range ON open_links(path, start, end);
                 CREATE TEMP TABLE open_semantic_references(
                     source_path TEXT NOT NULL, target_path TEXT NOT NULL, target_id TEXT,
                     source_start INTEGER NOT NULL, source_end INTEGER NOT NULL);
                 CREATE INDEX temp.open_references_target
                     ON open_semantic_references(target_path, target_id);
                 CREATE TEMP TABLE open_tasks(
                     path TEXT NOT NULL, id TEXT, start INTEGER NOT NULL, record BLOB NOT NULL);
                 CREATE INDEX temp.open_tasks_identity ON open_tasks(path, id);
                 CREATE TEMP TABLE open_events(
                     path TEXT NOT NULL, start INTEGER NOT NULL, is_point INTEGER NOT NULL,
                     sort_millis INTEGER, interval_start_millis INTEGER,
                     interval_end_millis INTEGER, record BLOB NOT NULL);
                 CREATE INDEX temp.open_events_time
                     ON open_events(interval_start_millis, interval_end_millis);

                 CREATE TEMP VIEW effective_events AS
                     SELECT e.path, d.revision, e.start, e.is_point, e.sort_millis,
                            e.interval_start_millis, e.interval_end_millis, e.record
                     FROM open_events e JOIN open_documents d ON d.path = e.path
                     UNION ALL
                     SELECT e.path, d.revision, e.start, e.is_point, e.sort_millis,
                            e.interval_start_millis, e.interval_end_millis, e.record
                     FROM main.events e JOIN main.documents d ON d.path = e.path
                     WHERE NOT EXISTS (
                         SELECT 1 FROM open_documents o WHERE o.path = e.path
                     );
                 CREATE TEMP VIEW effective_references AS
                     SELECT r.source_path, r.target_path, r.target_id,
                            r.source_start, r.source_end
                     FROM open_semantic_references r
                     UNION ALL
                     SELECT r.source_path, r.target_path, r.target_id,
                            r.source_start, r.source_end
                     FROM main.semantic_references r
                     WHERE NOT EXISTS (
                         SELECT 1 FROM open_documents o WHERE o.path = r.source_path
                     );",
            )
            .unwrap();
        let mut overlay = Self { connection };
        overlay.replace("main", TARGET_PATH, 0, target);
        overlay.replace("main", SOURCE_PATH, 0, disk);
        overlay
    }

    fn replace_open(&mut self, revision: i64, output: &DocumentOutput) {
        self.replace("temp", SOURCE_PATH, revision, output);
    }

    fn replace(&mut self, schema: &str, path: &str, revision: i64, output: &DocumentOutput) {
        assert!(matches!(schema, "main" | "temp"));
        let prefix = if schema == "temp" { "open_" } else { "" };
        let transaction = self.connection.transaction().unwrap();
        for table in ["anchors", "links", "semantic_references", "tasks", "events"] {
            let path_column = if table == "semantic_references" {
                "source_path"
            } else {
                "path"
            };
            transaction
                .execute(
                    &format!("DELETE FROM {schema}.{prefix}{table} WHERE {path_column} = ?1"),
                    [path],
                )
                .unwrap();
        }
        transaction
            .execute(
                &format!("DELETE FROM {schema}.{prefix}documents WHERE path = ?1"),
                [path],
            )
            .unwrap();
        transaction
            .execute(
                &format!("INSERT INTO {schema}.{prefix}documents(path, revision) VALUES (?1, ?2)"),
                params![path, revision],
            )
            .unwrap();
        insert_output(&transaction, schema, prefix, path, output);
        transaction.commit().unwrap();
    }

    fn events_overlapping(
        &self,
        start: DateTime<FixedOffset>,
        end: DateTime<FixedOffset>,
    ) -> Vec<EventRecord> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT record FROM effective_events
                 WHERE (is_point = 1
                        AND interval_start_millis >= ?1 AND interval_start_millis < ?2)
                    OR (is_point = 0 AND interval_start_millis < ?2
                        AND (interval_end_millis IS NULL OR interval_end_millis > ?1))
                 ORDER BY sort_millis, path, start",
            )
            .unwrap();
        statement
            .query_map(
                params![start.timestamp_millis(), end.timestamp_millis()],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .unwrap()
            .map(|record| bincode::deserialize(&record.unwrap()).unwrap())
            .collect()
    }

    fn references_to(&self, path: &str, id: &str) -> Vec<(String, usize, usize)> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT source_path, source_start, source_end
                 FROM effective_references
                 WHERE target_path = ?1 AND target_id = ?2
                 ORDER BY source_path, source_start",
            )
            .unwrap();
        statement
            .query_map(params![path, id], |row| {
                Ok((
                    row.get(0)?,
                    row.get::<_, i64>(1)? as usize,
                    row.get::<_, i64>(2)? as usize,
                ))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }
}

fn insert_output(
    transaction: &Transaction<'_>,
    schema: &str,
    prefix: &str,
    path: &str,
    output: &DocumentOutput,
) {
    let mut insert_anchor = transaction
        .prepare(&format!(
            "INSERT INTO {schema}.{prefix}anchors(path, id, start, record)
             VALUES (?1, ?2, ?3, ?4)"
        ))
        .unwrap();
    for anchor in &output.anchors {
        insert_anchor
            .execute(params![
                path,
                anchor.id.value,
                anchor.range.start as i64,
                bincode::serialize(anchor).unwrap()
            ])
            .unwrap();
    }
    let mut insert_link = transaction
        .prepare(&format!(
            "INSERT INTO {schema}.{prefix}links(path, start, end, record)
             VALUES (?1, ?2, ?3, ?4)"
        ))
        .unwrap();
    let mut insert_reference = transaction
        .prepare(&format!(
            "INSERT INTO {schema}.{prefix}semantic_references(
                 source_path, target_path, target_id, source_start, source_end)
             VALUES (?1, ?2, ?3, ?4, ?5)"
        ))
        .unwrap();
    for link in &output.links {
        insert_link
            .execute(params![
                path,
                link.range.start as i64,
                link.range.end as i64,
                bincode::serialize(link).unwrap()
            ])
            .unwrap();
        let LinkTarget::Anchor {
            path: Some(target_path),
            fragment,
        } = &link.target_kind
        else {
            continue;
        };
        insert_reference
            .execute(params![
                path,
                target_path,
                fragment,
                link.selection_range.start as i64,
                link.selection_range.end as i64
            ])
            .unwrap();
    }
    let mut insert_task = transaction
        .prepare(&format!(
            "INSERT INTO {schema}.{prefix}tasks(path, id, start, record)
             VALUES (?1, ?2, ?3, ?4)"
        ))
        .unwrap();
    for task in &output.tasks.tasks {
        insert_task
            .execute(params![
                path,
                task.id.as_ref().map(|id| id.value.as_str()),
                task.range.start as i64,
                bincode::serialize(task).unwrap()
            ])
            .unwrap();
    }
    let mut insert_event = transaction
        .prepare(&format!(
            "INSERT INTO {schema}.{prefix}events(
                 path, start, is_point, sort_millis, interval_start_millis,
                 interval_end_millis, record)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
        ))
        .unwrap();
    for event in &output.events.events {
        let interval_start = event.at_datetime().or_else(|| event.start_datetime());
        insert_event
            .execute(params![
                path,
                event.range.start as i64,
                i64::from(event.is_point()),
                event.sort_datetime().map(|value| value.timestamp_millis()),
                interval_start.map(|value| value.timestamp_millis()),
                event.end_datetime().map(|value| value.timestamp_millis()),
                bincode::serialize(event).unwrap()
            ])
            .unwrap();
    }
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
        source.push_str("See `->[target|target.plumb#target].\n");
    }
    (target, source)
}

fn analyzed(source: &str) -> DocumentOutput {
    let parsed = parse(source);
    assert!(parsed.is_valid());
    analyze_document(&parsed.source, &parsed.syntax)
}

fn benchmark_publication(c: &mut Criterion) {
    let mut group = c.benchmark_group("semantic_overlay_publication");
    group.sample_size(10);
    for count in [1_000, 10_000, 33_512] {
        let references = count / 10;
        let (target_source, old_source) = workload(count, references, "");
        let (_, new_source) = workload(count, references, " updated");
        let target = analyzed(&target_source);
        let old = analyzed(&old_source);
        let new = analyzed(&new_source);

        let mut rust = Workspace::new();
        rust.insert(TARGET_PATH, 0, &target_source);
        rust.insert(SOURCE_PATH, 0, &old_source);
        let mut rust_revision = 0;
        group.bench_with_input(
            BenchmarkId::new("rust_full_update", count),
            &count,
            |b, _| {
                b.iter(|| {
                    rust_revision += 1;
                    let source = if rust_revision % 2 == 0 {
                        &old_source
                    } else {
                        &new_source
                    };
                    black_box(rust.insert(SOURCE_PATH, rust_revision, source));
                });
            },
        );

        let mut storage = TempOverlay::new(&target, &old);
        storage.replace_open(1, &old);
        let mut storage_revision = 1;
        group.bench_with_input(
            BenchmarkId::new("temp_store_preanalyzed", count),
            &count,
            |b, _| {
                b.iter(|| {
                    storage_revision += 1;
                    let output = if storage_revision % 2 == 0 {
                        &old
                    } else {
                        &new
                    };
                    storage.replace_open(storage_revision, output);
                    black_box(&storage);
                });
            },
        );

        let mut full = TempOverlay::new(&target, &old);
        full.replace_open(1, &old);
        let mut full_revision = 1;
        group.bench_with_input(
            BenchmarkId::new("temp_full_update", count),
            &count,
            |b, _| {
                b.iter(|| {
                    full_revision += 1;
                    let source = if full_revision % 2 == 0 {
                        &old_source
                    } else {
                        &new_source
                    };
                    let output = analyzed(source);
                    full.replace_open(full_revision, &output);
                    black_box(&full);
                });
            },
        );
    }
    group.finish();
}

fn benchmark_queries(c: &mut Criterion) {
    let count = 33_512;
    let references = 3_351;
    let (target_source, disk_source) = workload(count, references, "");
    let (_, open_source) = workload(count, references, " updated");
    let target = analyzed(&target_source);
    let disk = analyzed(&disk_source);
    let open = analyzed(&open_source);
    let mut sql = TempOverlay::new(&target, &disk);
    sql.replace_open(1, &open);

    let mut rust = Workspace::new();
    rust.insert(TARGET_PATH, 0, &target_source);
    rust.insert(SOURCE_PATH, 0, &disk_source);
    rust.insert(SOURCE_PATH, 1, &open_source);
    let ids = HashSet::from(["target".to_string()]);
    let start = DateTime::parse_from_rfc3339("2026-08-10T00:00:00+00:00").unwrap();
    let end = DateTime::parse_from_rfc3339("2026-08-20T00:00:00+00:00").unwrap();

    let rust_events = rust.events_overlapping(start, end);
    let sql_events = sql.events_overlapping(start, end);
    assert_eq!(
        rust_events
            .iter()
            .map(|event| &event.event)
            .collect::<Vec<_>>(),
        sql_events.iter().collect::<Vec<_>>()
    );
    let rust_references = rust.reverse_references_for_document(TARGET_PATH, &ids);
    let rust_anchor_references = &rust_references.anchors["target"];
    let sql_references = sql.references_to(TARGET_PATH, "target");
    assert_eq!(rust_anchor_references.len(), sql_references.len());
    for (rust, sql) in rust_anchor_references.iter().zip(&sql_references) {
        assert_eq!(rust.source_path, Path::new(&sql.0));
        assert_eq!(rust.source_range, sql.1..sql.2);
    }

    let mut group = c.benchmark_group("semantic_overlay_queries_33512");
    group.sample_size(30);
    group.bench_function("rust_agenda", |b| {
        b.iter(|| black_box(rust.events_overlapping(start, end)));
    });
    group.bench_function("sql_effective_agenda", |b| {
        b.iter(|| black_box(sql.events_overlapping(start, end)));
    });
    group.bench_function("rust_reverse_references", |b| {
        b.iter(|| black_box(rust.reverse_references_for_document(TARGET_PATH, &ids)));
    });
    group.bench_function("sql_effective_reverse_references", |b| {
        b.iter(|| black_box(sql.references_to(TARGET_PATH, "target")));
    });
    group.finish();
}

fn benchmark_edit_burst(c: &mut Criterion) {
    let count = 33_512;
    let references = 3_351;
    let (target_source, old_source) = workload(count, references, "");
    let (_, new_source) = workload(count, references, " updated");
    let target = analyzed(&target_source);
    let old = analyzed(&old_source);
    let new = analyzed(&new_source);
    let mut group = c.benchmark_group("semantic_overlay_ten_update_burst_33512");
    group.sample_size(10);

    let mut rust = Workspace::new();
    rust.insert(TARGET_PATH, 0, &target_source);
    rust.insert(SOURCE_PATH, 0, &old_source);
    let mut rust_revision = 0;
    group.bench_function("rust_full_updates", |b| {
        b.iter(|| {
            for _ in 0..10 {
                rust_revision += 1;
                let source = if rust_revision % 2 == 0 {
                    &old_source
                } else {
                    &new_source
                };
                black_box(rust.insert(SOURCE_PATH, rust_revision, source));
            }
        });
    });

    let mut storage = TempOverlay::new(&target, &old);
    storage.replace_open(1, &old);
    let mut storage_revision = 1;
    group.bench_function("temp_store_preanalyzed", |b| {
        b.iter(|| {
            for _ in 0..10 {
                storage_revision += 1;
                let output = if storage_revision % 2 == 0 {
                    &old
                } else {
                    &new
                };
                storage.replace_open(storage_revision, output);
                black_box(&storage);
            }
        });
    });

    let mut full = TempOverlay::new(&target, &old);
    full.replace_open(1, &old);
    let mut full_revision = 1;
    group.bench_function("temp_full_updates", |b| {
        b.iter(|| {
            for _ in 0..10 {
                full_revision += 1;
                let source = if full_revision % 2 == 0 {
                    &old_source
                } else {
                    &new_source
                };
                let output = analyzed(source);
                full.replace_open(full_revision, &output);
                black_box(&full);
            }
        });
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
    targets = benchmark_publication, benchmark_queries, benchmark_edit_burst
}
criterion_main!(benches);
