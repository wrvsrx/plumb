use std::ffi::{OsStr, OsString};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use plumb_semantics::{
    AnchorRecord, DocumentOutput, EventRecord, LinkRecord, LinkTarget, MetadataValue, TaskRecord,
    TaskReferenceTarget,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{normalize, resolve_relative, task_reference_fields, task_reference_ranges};

const SCHEMA_VERSION: i64 = 2;
const PRODUCER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    Bincode(Box<bincode::ErrorKind>),
    InvalidStoredValue,
    LockPoisoned,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "SQLite semantic store: {error}"),
            Self::Bincode(error) => write!(formatter, "semantic record encoding: {error}"),
            Self::InvalidStoredValue => formatter.write_str("invalid persisted semantic value"),
            Self::LockPoisoned => formatter.write_str("SQLite semantic store lock poisoned"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
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

#[derive(Debug, Clone)]
pub struct SqliteSemanticStore {
    connection: Arc<Mutex<Connection>>,
}

impl SqliteSemanticStore {
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn open_in_memory() -> StoreResult<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> StoreResult<Self> {
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS cache_meta (
                 key TEXT PRIMARY KEY, value INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS documents (
                 path BLOB PRIMARY KEY, revision INTEGER NOT NULL,
                 content_hash BLOB NOT NULL, valid INTEGER NOT NULL,
                 title TEXT NOT NULL, title_start INTEGER NOT NULL, title_end INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS anchors (
                 path BLOB NOT NULL, id TEXT NOT NULL, start INTEGER NOT NULL, record BLOB NOT NULL
             );
             CREATE INDEX IF NOT EXISTS anchors_identity ON anchors(path, id);
             CREATE TABLE IF NOT EXISTS links (
                 path BLOB NOT NULL, start INTEGER NOT NULL, end INTEGER NOT NULL, record BLOB NOT NULL
             );
             CREATE INDEX IF NOT EXISTS links_source_range ON links(path, start, end);
             CREATE TABLE IF NOT EXISTS semantic_references (
                 source_path BLOB NOT NULL, target_path BLOB NOT NULL, target_id TEXT,
                 source_start INTEGER NOT NULL, source_end INTEGER NOT NULL,
                 path_start INTEGER, path_end INTEGER, id_start INTEGER, id_end INTEGER
             );
             CREATE INDEX IF NOT EXISTS references_target
                 ON semantic_references(target_path, target_id);
             CREATE INDEX IF NOT EXISTS references_source ON semantic_references(source_path);
             CREATE TABLE IF NOT EXISTS tasks (
                 path BLOB NOT NULL, id TEXT, title TEXT NOT NULL,
                 start INTEGER NOT NULL, record BLOB NOT NULL
             );
             CREATE INDEX IF NOT EXISTS tasks_identity ON tasks(path, id);
             CREATE TABLE IF NOT EXISTS events (
                 path BLOB NOT NULL, title TEXT NOT NULL, start INTEGER NOT NULL,
                 is_point INTEGER NOT NULL, sort_millis INTEGER, interval_start_millis INTEGER,
                 interval_end_millis INTEGER, record BLOB NOT NULL
             );
             CREATE INDEX IF NOT EXISTS events_time
                 ON events(interval_start_millis, interval_end_millis);",
        )?;
        let version = connection
            .query_row(
                "SELECT value FROM cache_meta WHERE key = 'schema_version'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        match version {
            Some(SCHEMA_VERSION) => {}
            Some(_) => {
                connection.execute_batch(
                    "DELETE FROM anchors; DELETE FROM links; DELETE FROM semantic_references;
                     DELETE FROM tasks; DELETE FROM events; DELETE FROM documents;",
                )?;
                connection.execute(
                    "UPDATE cache_meta SET value = ?1 WHERE key = 'schema_version'",
                    [SCHEMA_VERSION],
                )?;
            }
            None => {
                connection.execute(
                    "INSERT INTO cache_meta(key, value) VALUES ('schema_version', ?1)",
                    [SCHEMA_VERSION],
                )?;
            }
        }
        let producer = producer_version_key();
        let stored_producer = connection
            .query_row(
                "SELECT value FROM cache_meta WHERE key = 'producer_version'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if stored_producer != Some(producer) {
            connection.execute_batch(
                "DELETE FROM anchors; DELETE FROM links; DELETE FROM semantic_references;
                 DELETE FROM tasks; DELETE FROM events; DELETE FROM documents;",
            )?;
            connection.execute(
                "INSERT INTO cache_meta(key, value) VALUES ('producer_version', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [producer],
            )?;
        }
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    pub fn content_hash(source: &str) -> [u8; 32] {
        Sha256::digest(source.as_bytes()).into()
    }

    pub fn contains_current(&self, path: &Path, source: &str) -> StoreResult<bool> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        Ok(connection
            .query_row(
                "SELECT 1 FROM documents WHERE path = ?1 AND content_hash = ?2",
                params![
                    path_bytes(&normalize(path)),
                    Self::content_hash(source).as_slice()
                ],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
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
        let transaction = connection.transaction()?;
        delete_document_rows(&transaction, &path)?;
        let (title, title_range) = output.map_or_else(
            || (fallback_title(&path), 0..0),
            |output| document_title(output, &path),
        );
        transaction.execute(
            "INSERT INTO documents(
                 path, revision, content_hash, valid, title, title_start, title_end
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                path_bytes(&path),
                revision,
                Self::content_hash(source).as_slice(),
                i64::from(output.is_some()),
                title,
                to_i64(title_range.start)?,
                to_i64(title_range.end)?
            ],
        )?;
        if let Some(output) = output {
            insert_output(&transaction, &path, output)?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn remove(&self, path: &Path) -> StoreResult<()> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let transaction = connection.transaction()?;
        delete_document_rows(&transaction, &normalize(path))?;
        transaction.commit()?;
        Ok(())
    }

    pub fn documents(&self) -> StoreResult<Vec<StoredDocument>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let mut statement = connection.prepare(
            "SELECT path, revision, content_hash, valid, title, title_start, title_end
             FROM documents ORDER BY path",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, i64>(3)? != 0,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })?;
        rows.map(|row| {
            let (path, revision, hash, valid, title, start, end) = row?;
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

    pub fn anchors(&self, excluded: &[PathBuf]) -> StoreResult<Vec<StoredRecord<AnchorRecord>>> {
        self.records("anchors", excluded)
    }

    pub fn anchors_named(&self, path: &Path, id: &str) -> StoreResult<Vec<AnchorRecord>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let mut statement = connection
            .prepare("SELECT record FROM anchors WHERE path = ?1 AND id = ?2 ORDER BY start")?;
        let rows = statement.query_map(params![path_bytes(&normalize(path)), id], |row| {
            row.get::<_, Vec<u8>>(0)
        })?;
        rows.map(|row| Ok(bincode::deserialize(&row?)?)).collect()
    }

    pub fn links(&self, excluded: &[PathBuf]) -> StoreResult<Vec<StoredRecord<LinkRecord>>> {
        self.records("links", excluded)
    }

    pub fn tasks(&self, excluded: &[PathBuf]) -> StoreResult<Vec<StoredRecord<TaskRecord>>> {
        self.records("tasks", excluded)
    }

    pub fn events(&self, excluded: &[PathBuf]) -> StoreResult<Vec<StoredRecord<EventRecord>>> {
        self.records("events", excluded)
    }

    pub fn events_overlapping(
        &self,
        start_millis: i64,
        end_millis: i64,
        excluded: &[PathBuf],
    ) -> StoreResult<Vec<StoredRecord<EventRecord>>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let mut statement = connection.prepare(
            "SELECT e.path, d.revision, e.record FROM events e
             JOIN documents d ON d.path = e.path
             WHERE (e.is_point = 1
                    AND e.interval_start_millis >= ?1 AND e.interval_start_millis < ?2)
                OR (e.is_point = 0 AND e.interval_start_millis < ?2
                    AND (e.interval_end_millis IS NULL OR e.interval_end_millis > ?1))
             ORDER BY e.sort_millis, e.path, e.start",
        )?;
        let rows = statement.query_map(params![start_millis, end_millis], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        let mut excluded = excluded
            .iter()
            .map(|path| normalize(path))
            .collect::<Vec<_>>();
        excluded.sort();
        rows.filter_map(|row| match row {
            Ok((path, revision, record)) => Some((|| {
                let path = path_from_bytes(path)?;
                if excluded.binary_search(&path).is_ok() {
                    return Ok(None);
                }
                Ok(Some(StoredRecord {
                    path,
                    revision,
                    record: bincode::deserialize(&record)?,
                }))
            })()),
            Err(error) => Some(Err(StoreError::Sqlite(error))),
        })
        .filter_map(|result| result.transpose())
        .collect()
    }

    fn records<T: DeserializeOwned>(
        &self,
        table: &str,
        excluded: &[PathBuf],
    ) -> StoreResult<Vec<StoredRecord<T>>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let sql = format!(
            "SELECT r.path, d.revision, r.record FROM {table} r
             JOIN documents d ON d.path = r.path ORDER BY r.path, r.start"
        );
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        let mut excluded = excluded
            .iter()
            .map(|path| normalize(path))
            .collect::<Vec<_>>();
        excluded.sort();
        rows.filter_map(|row| match row {
            Ok((path, revision, record)) => Some((|| {
                let path = path_from_bytes(path)?;
                if excluded.binary_search(&path).is_ok() {
                    return Ok(None);
                }
                Ok(Some(StoredRecord {
                    path,
                    revision,
                    record: bincode::deserialize(&record)?,
                }))
            })()),
            Err(error) => Some(Err(StoreError::Sqlite(error))),
        })
        .filter_map(|result| result.transpose())
        .collect()
    }

    pub fn references_to(
        &self,
        target_path: &Path,
        target_id: Option<&str>,
        excluded: &[PathBuf],
    ) -> StoreResult<Vec<StoredReference>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?;
        let mut statement = connection.prepare(
            "SELECT source_path, target_path, target_id, source_start, source_end,
                    path_start, path_end, id_start, id_end
             FROM semantic_references
             WHERE target_path = ?1
               AND ((?2 IS NULL AND target_id IS NULL) OR target_id = ?2)
             ORDER BY source_path, source_start",
        )?;
        let rows = statement.query_map(
            params![path_bytes(&normalize(target_path)), target_id],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    optional_range(row.get(5)?, row.get(6)?),
                    optional_range(row.get(7)?, row.get(8)?),
                ))
            },
        )?;
        let mut excluded = excluded
            .iter()
            .map(|path| normalize(path))
            .collect::<Vec<_>>();
        excluded.sort();
        rows.filter_map(|row| match row {
            Ok((source_path, target_path, target_id, start, end, path_range, id_range)) => {
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
                        path_range: convert_range(path_range)?,
                        id_range: convert_range(id_range)?,
                    }))
                })())
            }
            Err(error) => Some(Err(StoreError::Sqlite(error))),
        })
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
    transaction: &Transaction<'_>,
    path: &Path,
    output: &DocumentOutput,
) -> StoreResult<()> {
    let encoded_path = path_bytes(path);
    for anchor in &output.anchors {
        transaction.execute(
            "INSERT INTO anchors(path, id, start, record) VALUES (?1, ?2, ?3, ?4)",
            params![
                encoded_path,
                anchor.id.value,
                to_i64(anchor.range.start)?,
                encode(anchor)?
            ],
        )?;
    }
    for link in &output.links {
        transaction.execute(
            "INSERT INTO links(path, start, end, record) VALUES (?1, ?2, ?3, ?4)",
            params![
                encoded_path,
                to_i64(link.range.start)?,
                to_i64(link.range.end)?,
                encode(link)?
            ],
        )?;
        if let Some(reference) = link_reference(path, link) {
            insert_reference(transaction, &reference)?;
        }
    }
    for task in &output.tasks.tasks {
        transaction.execute(
            "INSERT INTO tasks(path, id, title, start, record) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                encoded_path,
                task.id.as_ref().map(|id| id.value.as_str()),
                task.title,
                to_i64(task.range.start)?,
                encode(task)?
            ],
        )?;
        for (source, range, target) in task_reference_fields(task) {
            if let Some(reference) = task_reference(path, source, range, &target) {
                insert_reference(transaction, &reference)?;
            }
        }
    }
    for event in &output.events.events {
        let interval_start = event.at_datetime().or_else(|| event.start_datetime());
        transaction.execute(
            "INSERT INTO events(path, title, start, is_point, sort_millis, interval_start_millis,
                                interval_end_millis, record)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                encoded_path,
                event.title,
                to_i64(event.range.start)?,
                i64::from(event.is_point()),
                event.sort_datetime().map(|value| value.timestamp_millis()),
                interval_start.map(|value| value.timestamp_millis()),
                event.end_datetime().map(|value| value.timestamp_millis()),
                encode(event)?
            ],
        )?;
        for dependency in &event.tasks {
            if let Some(reference) = task_reference(
                path,
                &dependency.source,
                &dependency.range,
                &dependency.target,
            ) {
                insert_reference(transaction, &reference)?;
            }
        }
    }
    Ok(())
}

fn insert_reference(transaction: &Transaction<'_>, reference: &StoredReference) -> StoreResult<()> {
    transaction.execute(
        "INSERT INTO semantic_references(source_path, target_path, target_id, source_start,
             source_end, path_start, path_end, id_start, id_end)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            path_bytes(&reference.source_path),
            path_bytes(&reference.target_path),
            reference.target_id,
            to_i64(reference.source_range.start)?,
            to_i64(reference.source_range.end)?,
            optional_i64(reference.path_range.as_ref().map(|range| range.start))?,
            optional_i64(reference.path_range.as_ref().map(|range| range.end))?,
            optional_i64(reference.id_range.as_ref().map(|range| range.start))?,
            optional_i64(reference.id_range.as_ref().map(|range| range.end))?
        ],
    )?;
    Ok(())
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

fn delete_document_rows(transaction: &Transaction<'_>, path: &Path) -> StoreResult<()> {
    let path = path_bytes(path);
    for (table, column) in [
        ("anchors", "path"),
        ("links", "path"),
        ("semantic_references", "source_path"),
        ("tasks", "path"),
        ("events", "path"),
    ] {
        transaction.execute(&format!("DELETE FROM {table} WHERE {column} = ?1"), [&path])?;
    }
    transaction.execute("DELETE FROM documents WHERE path = ?1", [&path])?;
    Ok(())
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
    use plumb_semantics::analyze_document;
    use plumb_syntax::parse;

    fn analyzed(source: &str) -> DocumentOutput {
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        analyze_document(&parsed.source, &parsed.syntax)
    }

    #[test]
    fn persists_semantic_records_without_source_or_syntax_tree() {
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let source = "{\n `: title Stored\n}\n\n`task Do it {\n `@ item\n}\n\n`event 2026-08-11T10:00 Work {\n `@ meeting\n}\n";
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
        let old = "Paragraph `->[old]{`:[to #target]}.\n\n`task Old {\n `@ target\n}\n";
        store
            .replace(Path::new("a.plumb"), 0, old, Some(&analyzed(old)))
            .unwrap();
        let new = "Paragraph `->[new]{`:[to #next]}.\n\n`task New {\n `@ next\n}\n";
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
    fn excludes_open_documents_at_document_granularity() {
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let source = "`task Stored {\n `@ item\n}\n";
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
        let source = "`task Persistent {\n `@ item\n}\n";
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
}
