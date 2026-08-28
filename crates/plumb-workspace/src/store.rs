use std::ffi::{OsStr, OsString};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::DateTime;
use diesel::connection::DefaultLoadingMode;
use diesel::connection::SimpleConnection;
use diesel::dsl::exists;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};
use plumb_semantics::{
    AnchorRecord, DocumentOutput, EventRecord, LinkRecord, LinkTarget, MetadataValue, TaskField,
    TaskRecord, TaskReferenceTarget, TaskState,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{normalize, resolve_relative, task_reference_fields, task_reference_ranges};

mod schema;

use schema::{
    anchors, cache_meta, documents, event_task_associations, events, links, semantic_references,
    task_dependencies, tasks,
};

type TaskDependencyRow = (Vec<u8>, i64, Option<String>, Vec<u8>, String, String);
type EventTaskAssociationRow = (Vec<u8>, i64, Vec<u8>, String, String, i64, i64);
type TaskFactRow = (
    Vec<u8>,
    i64,
    i64,
    i64,
    i64,
    Option<String>,
    String,
    String,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i32>,
    i64,
    Option<i64>,
    Option<String>,
    Option<String>,
);

const SCHEMA_VERSION: i64 = 6;
const PRODUCER_VERSION: &str = env!("CARGO_PKG_VERSION");
const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

#[derive(Debug)]
pub enum StoreError {
    Diesel(diesel::result::Error),
    Connection(diesel::ConnectionError),
    Migration(String),
    Bincode(Box<bincode::ErrorKind>),
    InvalidStoredValue,
    LockPoisoned,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Diesel(error) => write!(formatter, "SQLite semantic store: {error}"),
            Self::Connection(error) => write!(formatter, "SQLite connection: {error}"),
            Self::Migration(error) => write!(formatter, "SQLite migration: {error}"),
            Self::Bincode(error) => write!(formatter, "semantic record encoding: {error}"),
            Self::InvalidStoredValue => formatter.write_str("invalid persisted semantic value"),
            Self::LockPoisoned => formatter.write_str("SQLite semantic store lock poisoned"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<diesel::result::Error> for StoreError {
    fn from(error: diesel::result::Error) -> Self {
        Self::Diesel(error)
    }
}

impl From<diesel::ConnectionError> for StoreError {
    fn from(error: diesel::ConnectionError) -> Self {
        Self::Connection(error)
    }
}

impl From<Box<bincode::ErrorKind>> for StoreError {
    fn from(error: Box<bincode::ErrorKind>) -> Self {
        Self::Bincode(error)
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredDocument {
    pub path: PathBuf,
    pub revision: i64,
    pub content_hash: [u8; 32],
    pub valid: bool,
    pub title: String,
    pub title_range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredReference {
    pub source_path: PathBuf,
    pub target_path: PathBuf,
    pub target_id: Option<String>,
    pub source_range: Range<usize>,
    pub path_range: Option<Range<usize>>,
    pub id_range: Option<Range<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRecord<T> {
    pub path: PathBuf,
    pub revision: i64,
    pub record: T,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTaskDependency {
    pub source_path: PathBuf,
    pub source_start: usize,
    pub source_id: Option<String>,
    pub target_path: PathBuf,
    pub target_id: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StoredTaskKey {
    pub path: PathBuf,
    pub start: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTaskFact {
    pub path: PathBuf,
    pub revision: i64,
    pub start: usize,
    pub selection_range: Range<usize>,
    pub id: Option<String>,
    pub title: String,
    pub closure_state: String,
    pub created_millis: Option<i64>,
    pub due_millis: Option<i64>,
    pub wait_millis: Option<i64>,
    pub done_millis: Option<i64>,
    pub canceled_millis: Option<i64>,
    pub priority: Option<i32>,
    pub depth: usize,
    pub parent_start: Option<usize>,
    pub recur: Option<String>,
    pub prev: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEventKey {
    pub sort_millis: Option<i64>,
    pub path: PathBuf,
    pub start: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEventTaskAssociation {
    pub source_path: PathBuf,
    pub event_start: usize,
    pub target_path: PathBuf,
    pub target_id: String,
    pub source: String,
    pub source_range: Range<usize>,
}

#[derive(Clone)]
pub struct SqliteSemanticStore {
    connection: Arc<Mutex<SqliteConnection>>,
}

impl std::fmt::Debug for SqliteSemanticStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteSemanticStore")
            .finish_non_exhaustive()
    }
}

impl SqliteSemanticStore {
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        let url = path.as_ref().to_string_lossy();
        Self::from_connection(SqliteConnection::establish(&url)?)
    }

    pub fn open_in_memory() -> StoreResult<Self> {
        Self::from_connection(SqliteConnection::establish(":memory:")?)
    }

    #[cfg(test)]
    pub(crate) fn execute_batch_for_test(&self, sql: &str) -> StoreResult<()> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        connection.batch_execute(sql)?;
        Ok(())
    }

    pub fn readonly_snapshot(&self) -> StoreResult<Self> {
        let mut source = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        source.batch_execute("PRAGMA wal_checkpoint(FULL)")?;
        let serialized = source.serialize_database_to_buffer();
        let mut image = serialized.as_slice().to_vec();
        // A detached readonly image has no WAL sidecar. Mark the checkpointed image
        // as rollback-journal format so SQLite does not try to open one.
        if image.len() >= 20 {
            image[18] = 1;
            image[19] = 1;
        }
        let mut connection = SqliteConnection::establish(":memory:")?;
        connection.deserialize_readonly_database_from_buffer(&image)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    fn from_connection(mut connection: SqliteConnection) -> StoreResult<Self> {
        connection.batch_execute("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
        connection
            .run_pending_migrations(MIGRATIONS)
            .map_err(|error| StoreError::Migration(error.to_string()))?;
        let version = cache_meta::table
            .filter(cache_meta::key.eq("schema_version"))
            .select(cache_meta::value)
            .first::<i64>(&mut connection)
            .optional()?;
        match version {
            Some(SCHEMA_VERSION) => {}
            Some(_) => {
                clear_records(&mut connection)?;
                diesel::update(cache_meta::table.filter(cache_meta::key.eq("schema_version")))
                    .set(cache_meta::value.eq(SCHEMA_VERSION))
                    .execute(&mut connection)?;
            }
            None => {
                diesel::insert_into(cache_meta::table)
                    .values((
                        cache_meta::key.eq("schema_version"),
                        cache_meta::value.eq(SCHEMA_VERSION),
                    ))
                    .execute(&mut connection)?;
            }
        }
        let producer = producer_version_key();
        let stored_producer = cache_meta::table
            .filter(cache_meta::key.eq("producer_version"))
            .select(cache_meta::value)
            .first::<i64>(&mut connection)
            .optional()?;
        if stored_producer != Some(producer) {
            clear_records(&mut connection)?;
            diesel::insert_into(cache_meta::table)
                .values((
                    cache_meta::key.eq("producer_version"),
                    cache_meta::value.eq(producer),
                ))
                .on_conflict(cache_meta::key)
                .do_update()
                .set(cache_meta::value.eq(producer))
                .execute(&mut connection)?;
        }
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    pub fn content_hash(source: &str) -> [u8; 32] {
        Sha256::digest(source.as_bytes()).into()
    }

    pub fn contains_current(&self, path: &Path, source: &str) -> StoreResult<bool> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        Ok(diesel::select(exists(
            documents::table
                .filter(documents::path.eq(path_bytes(&normalize(path))))
                .filter(documents::content_hash.eq(Self::content_hash(source).to_vec())),
        ))
        .get_result(&mut *connection)?)
    }

    pub fn replace(
        &self,
        path: &Path,
        revision: i64,
        source: &str,
        output: Option<&DocumentOutput>,
    ) -> StoreResult<()> {
        let path = normalize(path);
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        connection.transaction::<_, StoreError, _>(|connection| {
            delete_document_rows(connection, &path)?;
            let (title, title_range) = output.map_or_else(
                || (fallback_title(&path), 0..0),
                |output| document_title(output, &path),
            );
            diesel::insert_into(documents::table)
                .values((
                    documents::path.eq(path_bytes(&path)),
                    documents::revision.eq(revision),
                    documents::content_hash.eq(Self::content_hash(source).to_vec()),
                    documents::valid.eq(output.is_some()),
                    documents::title.eq(title),
                    documents::title_start.eq(to_i64(title_range.start)?),
                    documents::title_end.eq(to_i64(title_range.end)?),
                ))
                .execute(connection)?;
            if let Some(output) = output {
                insert_output(connection, &path, output)?;
            }
            Ok(())
        })
    }

    pub fn remove(&self, path: &Path) -> StoreResult<()> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        connection.transaction::<_, StoreError, _>(|connection| {
            delete_document_rows(connection, &normalize(path))
        })
    }

    pub fn documents(&self) -> StoreResult<Vec<StoredDocument>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let rows = documents::table
            .select((
                documents::path,
                documents::revision,
                documents::content_hash,
                documents::valid,
                documents::title,
                documents::title_start,
                documents::title_end,
            ))
            .order(documents::path)
            .load::<(Vec<u8>, i64, Vec<u8>, bool, String, i64, i64)>(&mut *connection)?;
        rows.into_iter()
            .map(|(path, revision, hash, valid, title, start, end)| {
                let content_hash = hash
                    .try_into()
                    .map_err(|_| StoreError::InvalidStoredValue)?;
                Ok(StoredDocument {
                    path: path_from_bytes(path)?,
                    revision,
                    content_hash,
                    valid,
                    title,
                    title_range: to_usize(start)?..to_usize(end)?,
                })
            })
            .collect()
    }

    pub fn document_exists(&self, path: &Path) -> StoreResult<bool> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let path = path_bytes(&normalize(path));
        diesel::select(exists(documents::table.filter(documents::path.eq(path))))
            .get_result(&mut *connection)
            .map_err(Into::into)
    }

    pub fn document(&self, path: &Path) -> StoreResult<Option<StoredDocument>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let row = documents::table
            .filter(documents::path.eq(path_bytes(&normalize(path))))
            .select((
                documents::path,
                documents::revision,
                documents::content_hash,
                documents::valid,
                documents::title,
                documents::title_start,
                documents::title_end,
            ))
            .first::<(Vec<u8>, i64, Vec<u8>, bool, String, i64, i64)>(&mut *connection)
            .optional()?;
        row.map(|(path, revision, hash, valid, title, start, end)| {
            let content_hash = hash
                .try_into()
                .map_err(|_| StoreError::InvalidStoredValue)?;
            Ok(StoredDocument {
                path: path_from_bytes(path)?,
                revision,
                content_hash,
                valid,
                title,
                title_range: to_usize(start)?..to_usize(end)?,
            })
        })
        .transpose()
    }

    pub fn document_paths(&self) -> StoreResult<Vec<PathBuf>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        documents::table
            .select(documents::path)
            .order(documents::path)
            .load::<Vec<u8>>(&mut *connection)?
            .into_iter()
            .map(path_from_bytes)
            .collect()
    }

    pub fn anchors(&self, excluded: &[PathBuf]) -> StoreResult<Vec<StoredRecord<AnchorRecord>>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let rows = anchors::table
            .inner_join(documents::table.on(documents::path.eq(anchors::path)))
            .select((anchors::path, documents::revision, anchors::record))
            .order((anchors::path, anchors::start))
            .load_iter::<(Vec<u8>, i64, Vec<u8>), DefaultLoadingMode>(&mut *connection)?;
        decode_records(rows, excluded)
    }

    pub fn anchors_named(&self, path: &Path, id: &str) -> StoreResult<Vec<AnchorRecord>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let rows = anchors::table
            .filter(anchors::path.eq(path_bytes(&normalize(path))))
            .filter(anchors::id.eq(id))
            .select(anchors::record)
            .order(anchors::start)
            .load::<Vec<u8>>(&mut *connection)?;
        rows.into_iter()
            .map(|record| Ok(bincode::deserialize(&record)?))
            .collect()
    }

    pub fn links(&self, excluded: &[PathBuf]) -> StoreResult<Vec<StoredRecord<LinkRecord>>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let rows = links::table
            .inner_join(documents::table.on(documents::path.eq(links::path)))
            .select((links::path, documents::revision, links::record))
            .order((links::path, links::start))
            .load_iter::<(Vec<u8>, i64, Vec<u8>), DefaultLoadingMode>(&mut *connection)?;
        decode_records(rows, excluded)
    }

    pub fn tasks(&self, excluded: &[PathBuf]) -> StoreResult<Vec<StoredRecord<TaskRecord>>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let rows = tasks::table
            .inner_join(documents::table.on(documents::path.eq(tasks::path)))
            .select((tasks::path, documents::revision, tasks::record))
            .order((tasks::path, tasks::start))
            .load_iter::<(Vec<u8>, i64, Vec<u8>), DefaultLoadingMode>(&mut *connection)?;
        decode_records(rows, excluded)
    }

    pub fn tasks_for_path(&self, path: &Path) -> StoreResult<Vec<TaskRecord>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let rows = tasks::table
            .filter(tasks::path.eq(path_bytes(&normalize(path))))
            .select(tasks::record)
            .order(tasks::start)
            .load::<Vec<u8>>(&mut *connection)?;
        rows.into_iter()
            .map(|record| Ok(bincode::deserialize(&record)?))
            .collect()
    }

    pub fn task_facts(&self, excluded: &[PathBuf]) -> StoreResult<Vec<StoredTaskFact>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let rows = tasks::table
            .inner_join(documents::table.on(documents::path.eq(tasks::path)))
            .select((
                tasks::path,
                documents::revision,
                tasks::start,
                tasks::selection_start,
                tasks::selection_end,
                tasks::id,
                tasks::title,
                tasks::closure_state,
                tasks::created_millis,
                tasks::due_millis,
                tasks::wait_millis,
                tasks::done_millis,
                tasks::canceled_millis,
                tasks::priority,
                tasks::depth,
                tasks::parent_start,
                tasks::recur_text,
                tasks::prev_text,
            ))
            .order((tasks::path, tasks::start))
            .load::<TaskFactRow>(&mut *connection)?;
        decode_task_facts(rows, excluded)
    }

    pub fn tasks_by_keys(
        &self,
        keys: &[StoredTaskKey],
    ) -> StoreResult<Vec<StoredRecord<TaskRecord>>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let mut grouped = std::collections::BTreeMap::<Vec<u8>, Vec<i64>>::new();
        for key in keys {
            grouped
                .entry(path_bytes(&normalize(&key.path)))
                .or_default()
                .push(to_i64(key.start)?);
        }
        let mut records = Vec::<StoredRecord<TaskRecord>>::with_capacity(keys.len());
        for (path, starts) in grouped {
            let rows = tasks::table
                .inner_join(documents::table.on(documents::path.eq(tasks::path)))
                .filter(tasks::path.eq(path))
                .filter(tasks::start.eq_any(starts))
                .select((tasks::path, documents::revision, tasks::record))
                .order(tasks::start)
                .load::<(Vec<u8>, i64, Vec<u8>)>(&mut *connection)?;
            records.extend(decode_records(rows.into_iter().map(Ok), &[])?);
        }
        records.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then(left.record.range.start.cmp(&right.record.range.start))
        });
        Ok(records)
    }

    pub fn task_dependents(
        &self,
        target_path: &Path,
        target_id: &str,
        excluded: &[PathBuf],
    ) -> StoreResult<Vec<StoredTaskDependency>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let rows = task_dependencies::table
            .filter(task_dependencies::target_path.eq(path_bytes(&normalize(target_path))))
            .filter(task_dependencies::target_id.eq(target_id))
            .select((
                task_dependencies::source_path,
                task_dependencies::source_start,
                task_dependencies::source_id,
                task_dependencies::target_path,
                task_dependencies::target_id,
                task_dependencies::source_text,
            ))
            .order((
                task_dependencies::source_path,
                task_dependencies::source_start,
            ))
            .load::<TaskDependencyRow>(&mut *connection)?;
        decode_task_dependencies(rows, excluded)
    }

    pub fn task_dependency_relations(
        &self,
        excluded: &[PathBuf],
    ) -> StoreResult<Vec<StoredTaskDependency>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let rows = task_dependencies::table
            .select((
                task_dependencies::source_path,
                task_dependencies::source_start,
                task_dependencies::source_id,
                task_dependencies::target_path,
                task_dependencies::target_id,
                task_dependencies::source_text,
            ))
            .order((
                task_dependencies::target_path,
                task_dependencies::target_id,
                task_dependencies::source_path,
                task_dependencies::source_start,
            ))
            .load::<TaskDependencyRow>(&mut *connection)?;
        decode_task_dependencies(rows, excluded)
    }

    pub fn active_tasks(
        &self,
        now_millis: i64,
        excluded: &[PathBuf],
    ) -> StoreResult<Vec<StoredRecord<TaskRecord>>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let rows = tasks::table
            .inner_join(documents::table.on(documents::path.eq(tasks::path)))
            .filter(tasks::closure_state.eq("open"))
            .filter(
                tasks::wait_millis
                    .is_null()
                    .or(tasks::wait_millis.le(now_millis)),
            )
            .select((tasks::path, documents::revision, tasks::record))
            .order((tasks::path, tasks::start))
            .load_iter::<(Vec<u8>, i64, Vec<u8>), DefaultLoadingMode>(&mut *connection)?;
        decode_records(rows, excluded)
    }

    pub fn blocked_task_sources(&self, excluded: &[PathBuf]) -> StoreResult<Vec<StoredTaskKey>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let target = diesel::alias!(tasks as dependency_targets);
        let rows = task_dependencies::table
            .inner_join(
                target.on(target
                    .field(tasks::path)
                    .eq(task_dependencies::target_path)
                    .and(
                        target
                            .field(tasks::id)
                            .eq(task_dependencies::target_id.nullable()),
                    )),
            )
            .filter(target.field(tasks::closure_state).eq("open"))
            .select((
                task_dependencies::source_path,
                task_dependencies::source_start,
                task_dependencies::target_path,
            ))
            .distinct()
            .order((
                task_dependencies::source_path,
                task_dependencies::source_start,
            ))
            .load::<(Vec<u8>, i64, Vec<u8>)>(&mut *connection)?;
        let mut excluded = excluded
            .iter()
            .map(|path| normalize(path))
            .collect::<Vec<_>>();
        excluded.sort();
        rows.into_iter()
            .filter_map(|(path, start, target_path)| {
                Some((|| {
                    let path = path_from_bytes(path)?;
                    let target_path = path_from_bytes(target_path)?;
                    if excluded.binary_search(&path).is_ok()
                        || excluded.binary_search(&target_path).is_ok()
                    {
                        return Ok(None);
                    }
                    Ok(Some(StoredTaskKey {
                        path,
                        start: to_usize(start)?,
                    }))
                })())
            })
            .filter_map(Result::transpose)
            .collect()
    }

    pub fn events(&self, excluded: &[PathBuf]) -> StoreResult<Vec<StoredRecord<EventRecord>>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let rows = events::table
            .inner_join(documents::table.on(documents::path.eq(events::path)))
            .select((events::path, documents::revision, events::record))
            .order((events::path, events::start))
            .load_iter::<(Vec<u8>, i64, Vec<u8>), DefaultLoadingMode>(&mut *connection)?;
        decode_records(rows, excluded)
    }

    pub fn event_task_associations_for_event(
        &self,
        source_path: &Path,
        event_start: usize,
    ) -> StoreResult<Vec<StoredEventTaskAssociation>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let rows = event_task_associations::table
            .filter(event_task_associations::source_path.eq(path_bytes(&normalize(source_path))))
            .filter(event_task_associations::event_start.eq(to_i64(event_start)?))
            .select((
                event_task_associations::source_path,
                event_task_associations::event_start,
                event_task_associations::target_path,
                event_task_associations::target_id,
                event_task_associations::source_text,
                event_task_associations::source_start,
                event_task_associations::source_end,
            ))
            .order(event_task_associations::source_start)
            .load::<EventTaskAssociationRow>(&mut *connection)?;
        decode_event_task_associations(rows)
    }

    pub fn events_for_task(
        &self,
        target_path: &Path,
        target_id: &str,
        excluded: &[PathBuf],
    ) -> StoreResult<Vec<StoredRecord<EventRecord>>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let rows = event_task_associations::table
            .inner_join(
                events::table.on(events::path
                    .eq(event_task_associations::source_path)
                    .and(events::start.eq(event_task_associations::event_start))),
            )
            .inner_join(documents::table.on(documents::path.eq(events::path)))
            .filter(event_task_associations::target_path.eq(path_bytes(&normalize(target_path))))
            .filter(event_task_associations::target_id.eq(target_id))
            .select((events::path, documents::revision, events::record))
            .distinct()
            .order((events::path, events::start))
            .load_iter::<(Vec<u8>, i64, Vec<u8>), DefaultLoadingMode>(&mut *connection)?;
        decode_records(rows, excluded)
    }

    pub fn event_page_after(
        &self,
        boundary: Option<&StoredEventKey>,
        limit: usize,
        excluded: &[PathBuf],
    ) -> StoreResult<Vec<StoredRecord<EventRecord>>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let mut query = events::table
            .inner_join(documents::table.on(documents::path.eq(events::path)))
            .into_boxed();
        if let Some(boundary) = boundary {
            let path = path_bytes(&boundary.path);
            query = match boundary.sort_millis {
                Some(millis) => query.filter(
                    events::sort_millis
                        .gt(millis)
                        .or(events::sort_millis.eq(millis).and(
                            events::path.gt(path.clone()).or(events::path
                                .eq(path)
                                .and(events::start.gt(boundary.start as i64))),
                        ))
                        .or(events::sort_millis.is_null()),
                ),
                None => query.filter(
                    events::sort_millis.is_null().and(
                        events::path.gt(path.clone()).or(events::path
                            .eq(path)
                            .and(events::start.gt(boundary.start as i64))),
                    ),
                ),
            };
        }
        let rows = query
            .select((events::path, documents::revision, events::record))
            .order((
                events::sort_millis.is_null(),
                events::sort_millis,
                events::path,
                events::start,
            ))
            .limit(limit as i64)
            .load::<(Vec<u8>, i64, Vec<u8>)>(&mut *connection)?;
        decode_records(rows.into_iter().map(Ok), excluded)
    }

    pub fn event_page_before(
        &self,
        boundary: &StoredEventKey,
        limit: usize,
        excluded: &[PathBuf],
    ) -> StoreResult<Vec<StoredRecord<EventRecord>>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let path = path_bytes(&boundary.path);
        let mut query = events::table
            .inner_join(documents::table.on(documents::path.eq(events::path)))
            .into_boxed();
        query = match boundary.sort_millis {
            Some(millis) => query.filter(
                events::sort_millis
                    .lt(millis)
                    .or(events::sort_millis.eq(millis).and(
                        events::path.lt(path.clone()).or(events::path
                            .eq(path)
                            .and(events::start.lt(boundary.start as i64))),
                    )),
            ),
            None => query.filter(
                events::sort_millis
                    .is_not_null()
                    .or(events::sort_millis.is_null().and(
                        events::path.lt(path.clone()).or(events::path
                            .eq(path)
                            .and(events::start.lt(boundary.start as i64))),
                    )),
            ),
        };
        let mut rows = query
            .select((events::path, documents::revision, events::record))
            .order((
                events::sort_millis.is_null().desc(),
                events::sort_millis.desc(),
                events::path.desc(),
                events::start.desc(),
            ))
            .limit(limit as i64)
            .load::<(Vec<u8>, i64, Vec<u8>)>(&mut *connection)?;
        rows.reverse();
        decode_records(rows.into_iter().map(Ok), excluded)
    }

    pub fn events_overlapping(
        &self,
        start_millis: i64,
        end_millis: i64,
        excluded: &[PathBuf],
    ) -> StoreResult<Vec<StoredRecord<EventRecord>>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let point = events::is_point
            .eq(true)
            .and(events::interval_start_millis.ge(start_millis))
            .and(events::interval_start_millis.lt(end_millis));
        let interval = events::is_point
            .eq(false)
            .and(events::interval_start_millis.lt(end_millis))
            .and(
                events::interval_end_millis
                    .is_null()
                    .or(events::interval_end_millis.gt(start_millis)),
            );
        let rows = events::table
            .inner_join(documents::table.on(documents::path.eq(events::path)))
            .filter(point.or(interval))
            .select((events::path, documents::revision, events::record))
            .order((events::sort_millis, events::path, events::start))
            .load_iter::<(Vec<u8>, i64, Vec<u8>), DefaultLoadingMode>(&mut *connection)?;
        decode_records(rows, excluded)
    }

    pub fn references_to(
        &self,
        target_path: &Path,
        target_id: Option<&str>,
        excluded: &[PathBuf],
    ) -> StoreResult<Vec<StoredReference>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let mut query = semantic_references::table
            .filter(semantic_references::target_path.eq(path_bytes(&normalize(target_path))))
            .into_boxed();
        query = match target_id {
            Some(id) => query.filter(semantic_references::target_id.eq(id)),
            None => query.filter(semantic_references::target_id.is_null()),
        };
        let rows = query
            .select((
                semantic_references::source_path,
                semantic_references::target_path,
                semantic_references::target_id,
                semantic_references::source_start,
                semantic_references::source_end,
                semantic_references::path_start,
                semantic_references::path_end,
                semantic_references::id_start,
                semantic_references::id_end,
            ))
            .order((
                semantic_references::source_path,
                semantic_references::source_start,
            ))
            .load::<(
                Vec<u8>,
                Vec<u8>,
                Option<String>,
                i64,
                i64,
                Option<i64>,
                Option<i64>,
                Option<i64>,
                Option<i64>,
            )>(&mut *connection)?;
        let mut excluded = excluded
            .iter()
            .map(|path| normalize(path))
            .collect::<Vec<_>>();
        excluded.sort();
        rows.into_iter()
            .filter_map(
                |(
                    source_path,
                    target_path,
                    target_id,
                    start,
                    end,
                    path_start,
                    path_end,
                    id_start,
                    id_end,
                )| {
                    Some((|| {
                        let source_path = path_from_bytes(source_path)?;
                        if excluded.binary_search(&source_path).is_ok() {
                            return Ok(None);
                        }
                        Ok(Some(StoredReference {
                            source_path,
                            target_path: path_from_bytes(target_path)?,
                            target_id,
                            source_range: to_usize(start)?..to_usize(end)?,
                            path_range: convert_range(optional_range(path_start, path_end))?,
                            id_range: convert_range(optional_range(id_start, id_end))?,
                        }))
                    })())
                },
            )
            .filter_map(|result| result.transpose())
            .collect()
    }
}

fn producer_version_key() -> i64 {
    let digest = Sha256::digest(PRODUCER_VERSION.as_bytes());
    i64::from_le_bytes(
        digest[..8]
            .try_into()
            .expect("SHA-256 prefix is eight bytes"),
    )
}

fn insert_output(
    connection: &mut SqliteConnection,
    path: &Path,
    output: &DocumentOutput,
) -> StoreResult<()> {
    let encoded_path = path_bytes(path);
    for anchor in &output.anchors {
        diesel::insert_into(anchors::table)
            .values((
                anchors::path.eq(encoded_path.clone()),
                anchors::id.eq(&anchor.id.value),
                anchors::start.eq(to_i64(anchor.range.start)?),
                anchors::record.eq(encode(anchor)?),
            ))
            .execute(connection)?;
    }
    for link in &output.links {
        diesel::insert_into(links::table)
            .values((
                links::path.eq(encoded_path.clone()),
                links::start.eq(to_i64(link.range.start)?),
                links::end.eq(to_i64(link.range.end)?),
                links::record.eq(encode(link)?),
            ))
            .execute(connection)?;
        if let Some(reference) = link_reference(path, link) {
            insert_reference(connection, &reference)?;
        }
    }
    let mut task_ancestors = Vec::new();
    for task in &output.tasks.tasks {
        task_ancestors.truncate(task.depth);
        let parent_start = task_ancestors.last().copied().map(to_i64).transpose()?;
        let start = to_i64(task.range.start)?;
        diesel::insert_into(tasks::table)
            .values((
                tasks::path.eq(encoded_path.clone()),
                tasks::id.eq(task.id.as_ref().map(|id| id.value.as_str())),
                tasks::title.eq(&task.title),
                tasks::start.eq(start),
                tasks::record.eq(encode(task)?),
                tasks::closure_state.eq(task_state_name(task.state())),
                tasks::created_millis.eq(task_field_millis(task.created.as_ref())),
                tasks::due_millis.eq(task_field_millis(task.due.as_ref())),
                tasks::wait_millis.eq(task_field_millis(task.wait.as_ref())),
                tasks::done_millis.eq(task_field_millis(task.done.as_ref())),
                tasks::canceled_millis.eq(task_field_millis(task.canceled.as_ref())),
                tasks::priority.eq(task.priority),
                tasks::depth.eq(to_i64(task.depth)?),
                tasks::parent_start.eq(parent_start),
                tasks::selection_start.eq(to_i64(task.selection_range.start)?),
                tasks::selection_end.eq(to_i64(task.selection_range.end)?),
                tasks::recur_text.eq(task.recur.as_ref().map(|field| field.value.as_str())),
                tasks::prev_text.eq(task.prev.as_ref().map(|field| field.value.as_str())),
            ))
            .execute(connection)?;
        task_ancestors.push(task.range.start);
        for dependency in &task.depends {
            let Some(reference) = task_reference(
                path,
                &dependency.source,
                &dependency.range,
                &dependency.target,
            ) else {
                continue;
            };
            diesel::insert_into(task_dependencies::table)
                .values((
                    task_dependencies::source_path.eq(encoded_path.clone()),
                    task_dependencies::source_start.eq(start),
                    task_dependencies::source_id.eq(task.id.as_ref().map(|id| id.value.as_str())),
                    task_dependencies::target_path.eq(path_bytes(&reference.target_path)),
                    task_dependencies::target_id
                        .eq(reference.target_id.as_deref().expect("task target has id")),
                    task_dependencies::source_text.eq(&dependency.source),
                ))
                .execute(connection)?;
        }
        for (source, range, target) in task_reference_fields(task) {
            if let Some(reference) = task_reference(path, source, range, &target) {
                insert_reference(connection, &reference)?;
            }
        }
    }
    for event in &output.events.events {
        let interval_start = event.at_datetime().or_else(|| event.start_datetime());
        diesel::insert_into(events::table)
            .values((
                events::path.eq(encoded_path.clone()),
                events::title.eq(&event.title),
                events::start.eq(to_i64(event.range.start)?),
                events::is_point.eq(event.is_point()),
                events::sort_millis.eq(event.sort_datetime().map(|value| value.timestamp_millis())),
                events::interval_start_millis
                    .eq(interval_start.map(|value| value.timestamp_millis())),
                events::interval_end_millis
                    .eq(event.end_datetime().map(|value| value.timestamp_millis())),
                events::record.eq(encode(event)?),
            ))
            .execute(connection)?;
        for association in projected_event_task_associations(path, output, event) {
            diesel::insert_into(event_task_associations::table)
                .values((
                    event_task_associations::source_path.eq(encoded_path.clone()),
                    event_task_associations::event_start.eq(to_i64(event.range.start)?),
                    event_task_associations::target_path.eq(path_bytes(&association.target_path)),
                    event_task_associations::target_id.eq(&association.target_id),
                    event_task_associations::source_text.eq(&association.source),
                    event_task_associations::source_start
                        .eq(to_i64(association.source_range.start)?),
                    event_task_associations::source_end.eq(to_i64(association.source_range.end)?),
                ))
                .execute(connection)?;
        }
        for dependency in &event.tasks {
            if let Some(reference) = task_reference(
                path,
                &dependency.source,
                &dependency.range,
                &dependency.target,
            ) {
                insert_reference(connection, &reference)?;
            }
        }
    }
    Ok(())
}

fn insert_reference(
    connection: &mut SqliteConnection,
    reference: &StoredReference,
) -> StoreResult<()> {
    diesel::insert_into(semantic_references::table)
        .values((
            semantic_references::source_path.eq(path_bytes(&reference.source_path)),
            semantic_references::target_path.eq(path_bytes(&reference.target_path)),
            semantic_references::target_id.eq(reference.target_id.as_deref()),
            semantic_references::source_start.eq(to_i64(reference.source_range.start)?),
            semantic_references::source_end.eq(to_i64(reference.source_range.end)?),
            semantic_references::path_start.eq(optional_i64(
                reference.path_range.as_ref().map(|range| range.start),
            )?),
            semantic_references::path_end.eq(optional_i64(
                reference.path_range.as_ref().map(|range| range.end),
            )?),
            semantic_references::id_start.eq(optional_i64(
                reference.id_range.as_ref().map(|range| range.start),
            )?),
            semantic_references::id_end.eq(optional_i64(
                reference.id_range.as_ref().map(|range| range.end),
            )?),
        ))
        .execute(connection)?;
    Ok(())
}

fn projected_event_task_associations(
    source_path: &Path,
    output: &DocumentOutput,
    event: &EventRecord,
) -> Vec<StoredEventTaskAssociation> {
    if event.tasks_override {
        return event
            .tasks
            .iter()
            .filter_map(|dependency| {
                task_reference(
                    source_path,
                    &dependency.source,
                    &dependency.range,
                    &dependency.target,
                )
                .and_then(|reference| {
                    Some(StoredEventTaskAssociation {
                        source_path: source_path.to_path_buf(),
                        event_start: event.range.start,
                        target_path: reference.target_path,
                        target_id: reference.target_id?,
                        source: dependency.source.clone(),
                        source_range: dependency.range.clone(),
                    })
                })
            })
            .collect();
    }

    output
        .links_contained_by_event(event.range.start)
        .unwrap_or_default()
        .iter()
        .filter_map(|link| {
            let reference = link_reference(source_path, link)?;
            Some(StoredEventTaskAssociation {
                source_path: source_path.to_path_buf(),
                event_start: event.range.start,
                target_path: reference.target_path,
                target_id: reference.target_id?,
                source: link.target.value.clone(),
                source_range: link.target.range.clone(),
            })
        })
        .collect()
}

fn link_reference(source_path: &Path, link: &LinkRecord) -> Option<StoredReference> {
    let (target_path, target_id) = match &link.target_kind {
        LinkTarget::Anchor { path, fragment } => (
            path.as_deref().map_or_else(
                || normalize(source_path),
                |path| resolve_relative(source_path, path),
            ),
            Some(fragment.clone()),
        ),
        LinkTarget::Document { path } => (resolve_relative(source_path, path), None),
        _ => return None,
    };
    Some(StoredReference {
        source_path: source_path.to_path_buf(),
        target_path,
        target_id,
        source_range: link.selection_range.clone(),
        path_range: link.path_range.clone(),
        id_range: link.fragment_range.clone(),
    })
}

fn task_reference(
    source_path: &Path,
    source: &str,
    range: &Range<usize>,
    target: &TaskReferenceTarget,
) -> Option<StoredReference> {
    let (target_path, target_id) = match target {
        TaskReferenceTarget::Internal { id } => (normalize(source_path), id.clone()),
        TaskReferenceTarget::External { path, id } => {
            (resolve_relative(source_path, path), id.clone())
        }
        TaskReferenceTarget::Invalid => return None,
    };
    let (path_range, id_range) = task_reference_ranges(source, range, &target_id)?;
    Some(StoredReference {
        source_path: source_path.to_path_buf(),
        target_path,
        target_id: Some(target_id),
        source_range: range.clone(),
        path_range,
        id_range: Some(id_range),
    })
}

fn delete_document_rows(connection: &mut SqliteConnection, path: &Path) -> StoreResult<()> {
    let path = path_bytes(path);
    diesel::delete(anchors::table.filter(anchors::path.eq(&path))).execute(connection)?;
    diesel::delete(links::table.filter(links::path.eq(&path))).execute(connection)?;
    diesel::delete(semantic_references::table.filter(semantic_references::source_path.eq(&path)))
        .execute(connection)?;
    diesel::delete(tasks::table.filter(tasks::path.eq(&path))).execute(connection)?;
    diesel::delete(task_dependencies::table.filter(task_dependencies::source_path.eq(&path)))
        .execute(connection)?;
    diesel::delete(
        event_task_associations::table.filter(event_task_associations::source_path.eq(&path)),
    )
    .execute(connection)?;
    diesel::delete(events::table.filter(events::path.eq(&path))).execute(connection)?;
    diesel::delete(documents::table.filter(documents::path.eq(&path))).execute(connection)?;
    Ok(())
}

fn clear_records(connection: &mut SqliteConnection) -> StoreResult<()> {
    diesel::delete(anchors::table).execute(connection)?;
    diesel::delete(links::table).execute(connection)?;
    diesel::delete(semantic_references::table).execute(connection)?;
    diesel::delete(tasks::table).execute(connection)?;
    diesel::delete(task_dependencies::table).execute(connection)?;
    diesel::delete(event_task_associations::table).execute(connection)?;
    diesel::delete(events::table).execute(connection)?;
    diesel::delete(documents::table).execute(connection)?;
    Ok(())
}

fn decode_records<T: DeserializeOwned>(
    rows: impl Iterator<Item = diesel::QueryResult<(Vec<u8>, i64, Vec<u8>)>>,
    excluded: &[PathBuf],
) -> StoreResult<Vec<StoredRecord<T>>> {
    let mut excluded = excluded
        .iter()
        .map(|path| normalize(path))
        .collect::<Vec<_>>();
    excluded.sort();
    rows.filter_map(|row| {
        Some((|| {
            let (path, revision, record) = row?;
            let path = path_from_bytes(path)?;
            if excluded.binary_search(&path).is_ok() {
                return Ok(None);
            }
            Ok(Some(StoredRecord {
                path,
                revision,
                record: bincode::deserialize(&record)?,
            }))
        })())
    })
    .filter_map(Result::transpose)
    .collect()
}

fn decode_task_dependencies(
    rows: Vec<TaskDependencyRow>,
    excluded: &[PathBuf],
) -> StoreResult<Vec<StoredTaskDependency>> {
    let mut excluded = excluded
        .iter()
        .map(|path| normalize(path))
        .collect::<Vec<_>>();
    excluded.sort();
    rows.into_iter()
        .filter_map(
            |(source_path, source_start, source_id, target_path, target_id, source)| {
                Some((|| {
                    let source_path = path_from_bytes(source_path)?;
                    if excluded.binary_search(&source_path).is_ok() {
                        return Ok(None);
                    }
                    Ok(Some(StoredTaskDependency {
                        source_path,
                        source_start: to_usize(source_start)?,
                        source_id,
                        target_path: path_from_bytes(target_path)?,
                        target_id,
                        source,
                    }))
                })())
            },
        )
        .filter_map(Result::transpose)
        .collect()
}

fn decode_task_facts(
    rows: Vec<TaskFactRow>,
    excluded: &[PathBuf],
) -> StoreResult<Vec<StoredTaskFact>> {
    let mut excluded = excluded
        .iter()
        .map(|path| normalize(path))
        .collect::<Vec<_>>();
    excluded.sort();
    rows.into_iter()
        .filter_map(
            |(
                path,
                revision,
                start,
                selection_start,
                selection_end,
                id,
                title,
                closure_state,
                created_millis,
                due_millis,
                wait_millis,
                done_millis,
                canceled_millis,
                priority,
                depth,
                parent_start,
                recur,
                prev,
            )| {
                Some((|| {
                    let path = path_from_bytes(path)?;
                    if excluded.binary_search(&path).is_ok() {
                        return Ok(None);
                    }
                    Ok(Some(StoredTaskFact {
                        path,
                        revision,
                        start: to_usize(start)?,
                        selection_range: to_usize(selection_start)?..to_usize(selection_end)?,
                        id,
                        title,
                        closure_state,
                        created_millis,
                        due_millis,
                        wait_millis,
                        done_millis,
                        canceled_millis,
                        priority,
                        depth: to_usize(depth)?,
                        parent_start: parent_start.map(to_usize).transpose()?,
                        recur,
                        prev,
                    }))
                })())
            },
        )
        .filter_map(Result::transpose)
        .collect()
}

fn decode_event_task_associations(
    rows: Vec<EventTaskAssociationRow>,
) -> StoreResult<Vec<StoredEventTaskAssociation>> {
    rows.into_iter()
        .map(
            |(source_path, event_start, target_path, target_id, source, start, end)| {
                Ok(StoredEventTaskAssociation {
                    source_path: path_from_bytes(source_path)?,
                    event_start: to_usize(event_start)?,
                    target_path: path_from_bytes(target_path)?,
                    target_id,
                    source,
                    source_range: to_usize(start)?..to_usize(end)?,
                })
            },
        )
        .collect()
}

fn document_title(output: &DocumentOutput, path: &Path) -> (String, Range<usize>) {
    output
        .metadata
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.entries.iter().find(|entry| entry.key == "title"))
        .and_then(|entry| match &entry.value {
            MetadataValue::Scalar { content, .. } if !content.plain_text().is_empty() => {
                Some((content.plain_text(), content.range.clone()))
            }
            _ => None,
        })
        .unwrap_or_else(|| (fallback_title(path), 0..0))
}

fn task_state_name(state: TaskState) -> &'static str {
    match state {
        TaskState::Open => "open",
        TaskState::Done => "done",
        TaskState::Canceled => "canceled",
        TaskState::Conflicted => "conflicted",
    }
}

fn task_field_millis(field: Option<&TaskField>) -> Option<i64> {
    field
        .and_then(|field| DateTime::parse_from_rfc3339(&field.value).ok())
        .map(|value| value.timestamp_millis())
}

fn fallback_title(path: &Path) -> String {
    path.file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_string()
}

fn encode(value: &impl Serialize) -> StoreResult<Vec<u8>> {
    Ok(bincode::serialize(value)?)
}
fn optional_range(start: Option<i64>, end: Option<i64>) -> Option<Range<i64>> {
    Some(start?..end?)
}
fn convert_range(range: Option<Range<i64>>) -> StoreResult<Option<Range<usize>>> {
    range
        .map(|range| Ok(to_usize(range.start)?..to_usize(range.end)?))
        .transpose()
}
fn optional_i64(value: Option<usize>) -> StoreResult<Option<i64>> {
    value.map(to_i64).transpose()
}
fn to_i64(value: usize) -> StoreResult<i64> {
    i64::try_from(value).map_err(|_| StoreError::InvalidStoredValue)
}
fn to_usize(value: i64) -> StoreResult<usize> {
    usize::try_from(value).map_err(|_| StoreError::InvalidStoredValue)
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}
#[cfg(unix)]
fn path_from_bytes(bytes: Vec<u8>) -> StoreResult<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}
#[cfg(windows)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}
#[cfg(windows)]
fn path_from_bytes(bytes: Vec<u8>) -> StoreResult<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    if bytes.len() % 2 != 0 {
        return Err(StoreError::InvalidStoredValue);
    }
    let wide = bytes
        .chunks_exact(2)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
        .collect::<Vec<_>>();
    Ok(PathBuf::from(OsString::from_wide(&wide)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use diesel::sql_types::Text;
    use plumb_semantics::analyze_document;
    use plumb_syntax::parse;

    fn analyzed(source: &str) -> DocumentOutput {
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        analyze_document(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        )
    }

    #[test]
    fn persists_semantic_records_without_source_or_syntax_tree() {
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let source = "`= title|Stored\n\n`- Do it\n\n `+ task\n\n `@ item\n\n`- 2026-08-11T10:00|Work\n\n `+ event\n\n `@ meeting\n";
        store
            .replace(
                Path::new("notes/a.plumb"),
                0,
                source,
                Some(&analyzed(source)),
            )
            .unwrap();
        assert!(store
            .contains_current(Path::new("notes/a.plumb"), source)
            .unwrap());
        assert_eq!(store.documents().unwrap()[0].title, "Stored");
        assert_eq!(store.anchors(&[]).unwrap().len(), 2);
        assert_eq!(store.tasks(&[]).unwrap().len(), 1);
        assert_eq!(store.events(&[]).unwrap().len(), 1);
    }

    #[test]
    fn atomically_replaces_a_documents_generation() {
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let old = "Paragraph `->[old|#target].\n\n`- Old\n\n `+ task\n\n `@ target\n";
        store
            .replace(Path::new("a.plumb"), 0, old, Some(&analyzed(old)))
            .unwrap();
        let new = "Paragraph `->[new|#next].\n\n`- New\n\n `+ task\n\n `@ next\n";
        store
            .replace(Path::new("a.plumb"), 0, new, Some(&analyzed(new)))
            .unwrap();
        assert!(store
            .references_to(Path::new("a.plumb"), Some("target"), &[])
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .references_to(Path::new("a.plumb"), Some("next"), &[])
                .unwrap()
                .len(),
            1
        );
        assert_eq!(store.tasks(&[]).unwrap()[0].record.title, "New");
    }

    #[test]
    fn readonly_snapshot_is_isolated_from_later_replacements() {
        let directory = tempfile::tempdir().unwrap();
        let store = SqliteSemanticStore::open(&directory.path().join("semantic.sqlite3")).unwrap();
        let first = "`- First\n\n `+ task\n\n `@ first\n";
        store
            .replace(Path::new("tasks.plumb"), 1, first, Some(&analyzed(first)))
            .unwrap();
        let snapshot = store.readonly_snapshot().unwrap();

        let second = "`- Second\n\n `+ task\n\n `@ second\n";
        store
            .replace(Path::new("tasks.plumb"), 2, second, Some(&analyzed(second)))
            .unwrap();

        assert_eq!(snapshot.tasks(&[]).unwrap()[0].record.title, "First");
        assert_eq!(store.tasks(&[]).unwrap()[0].record.title, "Second");
        assert!(snapshot
            .replace(Path::new("other.plumb"), 1, "", None)
            .is_err());
    }

    #[test]
    fn excludes_open_documents_at_document_granularity() {
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let source = "`- Stored\n\n `+ task\n\n `@ item\n";
        store
            .replace(Path::new("a.plumb"), 0, source, Some(&analyzed(source)))
            .unwrap();
        let excluded = [PathBuf::from("a.plumb")];
        assert!(store.tasks(&excluded).unwrap().is_empty());
        assert!(store.anchors(&excluded).unwrap().is_empty());
    }

    #[test]
    fn reopens_a_persistent_store_without_rebuilding_records() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("semantic.sqlite3");
        let source = "`- Persistent\n\n `+ task\n\n `@ item\n";
        {
            let store = SqliteSemanticStore::open(&database).unwrap();
            store
                .replace(Path::new("a.plumb"), 0, source, Some(&analyzed(source)))
                .unwrap();
        }
        let reopened = SqliteSemanticStore::open(&database).unwrap();
        assert!(reopened
            .contains_current(Path::new("a.plumb"), source)
            .unwrap());
        assert_eq!(reopened.tasks(&[]).unwrap()[0].record.title, "Persistent");
    }

    #[cfg(unix)]
    #[test]
    fn round_trips_non_utf8_paths_as_sqlite_blobs() {
        use std::os::unix::ffi::OsStringExt;

        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let path = PathBuf::from(OsString::from_vec(b"notes/\xff.plumb".to_vec()));
        let source = "`- Stored\n\n `+ task\n";
        store
            .replace(&path, 7, source, Some(&analyzed(source)))
            .unwrap();

        assert!(store.contains_current(&path, source).unwrap());
        assert_eq!(store.documents().unwrap()[0].path, path);
        assert_eq!(store.tasks(&[]).unwrap()[0].path, path);
    }

    #[test]
    fn indexes_task_dependencies_by_target_and_replaces_their_generation() {
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let source = "`- Source\n\n `+ task\n\n `@ source\n\n `= depends|target.plumb#target\n";
        store
            .replace(
                Path::new("source.plumb"),
                3,
                source,
                Some(&analyzed(source)),
            )
            .unwrap();

        let dependencies = store
            .task_dependents(Path::new("target.plumb"), "target", &[])
            .unwrap();
        assert_eq!(dependencies.len(), 1);
        assert_eq!(dependencies[0].source_path, Path::new("source.plumb"));
        assert_eq!(dependencies[0].source_id.as_deref(), Some("source"));
        assert_eq!(dependencies[0].source, "target.plumb#target");
        assert_eq!(store.task_dependency_relations(&[]).unwrap(), dependencies);
        assert!(store
            .task_dependency_relations(&[PathBuf::from("source.plumb")])
            .unwrap()
            .is_empty());
        assert!(store
            .task_dependents(
                Path::new("target.plumb"),
                "target",
                &[PathBuf::from("source.plumb")],
            )
            .unwrap()
            .is_empty());

        let updated = "`- Source\n\n `+ task\n\n `@ source\n";
        store
            .replace(
                Path::new("source.plumb"),
                4,
                updated,
                Some(&analyzed(updated)),
            )
            .unwrap();
        assert!(store
            .task_dependents(Path::new("target.plumb"), "target", &[])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn task_facts_do_not_decode_records_and_page_lookup_decodes_only_selected_keys() {
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let source = concat!(
            "`- First\n\n `+ task\n\n `@ first\n\n `= priority|3\n `= recur|P1D\n `= due|2026-08-29T10:00:00Z\n",
            "`- Second\n\n `+ task\n\n `@ second\n\n `= prev|#first\n",
        );
        let output = analyzed(source);
        let first_start = output.tasks.tasks[0].range.start;
        let second_start = output.tasks.tasks[1].range.start;
        store
            .replace(Path::new("tasks.plumb"), 9, source, Some(&output))
            .unwrap();

        {
            let mut connection = store.connection.lock().unwrap();
            diesel::update(
                tasks::table
                    .filter(tasks::path.eq(path_bytes(Path::new("tasks.plumb"))))
                    .filter(tasks::start.eq(to_i64(first_start).unwrap())),
            )
            .set(tasks::record.eq(vec![0xff]))
            .execute(&mut *connection)
            .unwrap();
        }

        let facts = store.task_facts(&[]).unwrap();
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].priority, Some(3));
        assert_eq!(facts[0].recur.as_deref(), Some("P1D"));
        assert_eq!(facts[1].prev.as_deref(), Some("#first"));
        assert_eq!(
            store
                .tasks_by_keys(&[StoredTaskKey {
                    path: PathBuf::from("tasks.plumb"),
                    start: second_start,
                }])
                .unwrap()[0]
                .record
                .title,
            "Second"
        );
        assert!(store
            .tasks_by_keys(&[StoredTaskKey {
                path: PathBuf::from("tasks.plumb"),
                start: first_start,
            }])
            .is_err());
    }

    #[test]
    fn queries_only_open_tasks_whose_wait_has_elapsed() {
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let source = "`- Ready\n\n `+ task\n\n `@ ready\n\n`- Waiting\n\n `+ task\n\n `@ waiting\n\n `= wait|2026-08-12T10:00:00Z\n\n`- Done\n\n `+ task\n\n `@ done\n\n `= done|2026-08-10T10:00:00Z\n\n`- Canceled\n\n `+ task\n\n `@ canceled\n\n `= canceled|2026-08-10T10:00:00Z\n";
        store
            .replace(Path::new("tasks.plumb"), 0, source, Some(&analyzed(source)))
            .unwrap();

        let now = chrono::DateTime::parse_from_rfc3339("2026-08-11T10:00:00Z")
            .unwrap()
            .timestamp_millis();
        let tasks = store
            .active_tasks(now, &[])
            .unwrap()
            .into_iter()
            .map(|task| task.record.title)
            .collect::<Vec<_>>();
        assert_eq!(tasks, ["Ready"]);
        assert!(store
            .active_tasks(now, &[PathBuf::from("tasks.plumb")])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn blocked_sources_follow_open_target_generations_and_overlay_exclusions() {
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let source = "`- Source\n\n `+ task\n\n `@ source\n\n `= depends|target.plumb#target\n";
        let open_target = "`- Target\n\n `+ task\n\n `@ target\n";
        store
            .replace(
                Path::new("source.plumb"),
                1,
                source,
                Some(&analyzed(source)),
            )
            .unwrap();
        store
            .replace(
                Path::new("target.plumb"),
                1,
                open_target,
                Some(&analyzed(open_target)),
            )
            .unwrap();

        assert_eq!(
            store.blocked_task_sources(&[]).unwrap(),
            [StoredTaskKey {
                path: PathBuf::from("source.plumb"),
                start: 0,
            }]
        );
        assert!(store
            .blocked_task_sources(&[PathBuf::from("source.plumb")])
            .unwrap()
            .is_empty());
        assert!(store
            .blocked_task_sources(&[PathBuf::from("target.plumb")])
            .unwrap()
            .is_empty());

        let closed_target =
            "`- Target\n\n `+ task\n\n `@ target\n\n `= done|2026-08-11T10:00:00Z\n";
        store
            .replace(
                Path::new("target.plumb"),
                2,
                closed_target,
                Some(&analyzed(closed_target)),
            )
            .unwrap();
        assert!(store.blocked_task_sources(&[]).unwrap().is_empty());
    }

    #[test]
    fn indexes_event_task_associations_and_replaces_their_generation() {
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let source = concat!(
            "`- 2026-08-28T10:00|Linked `->[Task|tasks.plumb#task]\n\n `+ event\n",
            "`- 2026-08-28T11:00|Explicit\n\n `+ event\n\n `= tasks|tasks.plumb#task\n",
        );
        let output = analyzed(source);
        let first_start = output.events.events[0].range.start;
        let second_start = output.events.events[1].range.start;
        store
            .replace(Path::new("events.plumb"), 7, source, Some(&output))
            .unwrap();

        let linked = store
            .event_task_associations_for_event(Path::new("events.plumb"), first_start)
            .unwrap();
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].target_path, Path::new("tasks.plumb"));
        assert_eq!(linked[0].target_id, "task");
        assert_eq!(linked[0].source, "tasks.plumb#task");
        let explicit = store
            .event_task_associations_for_event(Path::new("events.plumb"), second_start)
            .unwrap();
        assert_eq!(explicit.len(), 1);
        assert_eq!(explicit[0].target_id, "task");
        assert_eq!(
            store
                .events_for_task(Path::new("tasks.plumb"), "task", &[])
                .unwrap()
                .into_iter()
                .map(|event| event.record.title)
                .collect::<Vec<_>>(),
            ["Linked Task", "Explicit"]
        );
        assert!(store
            .events_for_task(
                Path::new("tasks.plumb"),
                "task",
                &[PathBuf::from("events.plumb")],
            )
            .unwrap()
            .is_empty());

        let updated = "`- 2026-08-28T12:00|Unrelated\n\n `+ event\n";
        store
            .replace(
                Path::new("events.plumb"),
                8,
                updated,
                Some(&analyzed(updated)),
            )
            .unwrap();
        assert!(store
            .events_for_task(Path::new("tasks.plumb"), "task", &[])
            .unwrap()
            .is_empty());
    }

    #[derive(QueryableByName)]
    struct QueryPlanRow {
        #[diesel(sql_type = Text)]
        detail: String,
    }

    #[test]
    fn active_task_query_uses_the_state_wait_index() {
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let mut connection = store.connection.lock().unwrap();
        let plan = diesel::sql_query(
            "EXPLAIN QUERY PLAN SELECT path, start FROM tasks \
             WHERE closure_state = 'open' AND (wait_millis IS NULL OR wait_millis <= 0) \
             ORDER BY path, start",
        )
        .load::<QueryPlanRow>(&mut *connection)
        .unwrap();
        assert!(
            plan.iter()
                .any(|row| row.detail.contains("tasks_state_wait")),
            "{}",
            plan.iter()
                .map(|row| row.detail.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        );
    }

    #[test]
    fn event_task_lookup_uses_the_target_index() {
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let mut connection = store.connection.lock().unwrap();
        let plan = diesel::sql_query(
            "EXPLAIN QUERY PLAN SELECT source_path, event_start \
             FROM event_task_associations \
             WHERE target_path = x'7461736b732e706c756d62' AND target_id = 'task' \
             ORDER BY source_path, event_start",
        )
        .load::<QueryPlanRow>(&mut *connection)
        .unwrap();
        assert!(
            plan.iter()
                .any(|row| row.detail.contains("event_task_associations_target")),
            "{}",
            plan.iter()
                .map(|row| row.detail.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        );
    }

    #[test]
    fn task_fact_order_queries_use_typed_indexes() {
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let mut connection = store.connection.lock().unwrap();
        let source_plan = diesel::sql_query(
            "EXPLAIN QUERY PLAN SELECT path, start FROM tasks ORDER BY path, start",
        )
        .load::<QueryPlanRow>(&mut *connection)
        .unwrap();
        assert!(
            source_plan.iter().any(|row| {
                row.detail.contains("tasks_source_order")
                    || row.detail.contains("sqlite_autoindex_tasks_1")
            }),
            "{}",
            source_plan
                .iter()
                .map(|row| row.detail.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        );

        let due_plan = diesel::sql_query(
            "EXPLAIN QUERY PLAN SELECT due_millis, path, start FROM tasks \
             ORDER BY due_millis, path, start",
        )
        .load::<QueryPlanRow>(&mut *connection)
        .unwrap();
        assert!(
            due_plan
                .iter()
                .any(|row| row.detail.contains("tasks_due_order")),
            "{}",
            due_plan
                .iter()
                .map(|row| row.detail.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
}
