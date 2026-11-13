//! Snapshot-safe, source-replaced tag postings. No workspace scans on warm reads.
use crate::{normalize, QueryResult, Workspace, WorkspaceQueryError};
use im::{OrdMap, OrdSet};
use plumb_semantics::{TagOwner, TaggedOwner};
use serde::{Deserialize, Serialize};
use std::ops::{Bound, Range};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

static GENERATION: AtomicU64 = AtomicU64::new(1);
fn generation() -> u64 {
    GENERATION.fetch_add(1, Ordering::Relaxed)
}

type OwnerKey = (TagOwner, usize);
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct Key {
    path: PathBuf,
    owner: TagOwner,
    start: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagMatch {
    pub path: PathBuf,
    pub revision: i64,
    pub record: TaggedOwner,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagCursor {
    generation: u64,
    tag: String,
    after: Key,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagPage {
    pub items: Vec<TagMatch>,
    pub next_cursor: Option<TagCursor>,
    pub generation: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagValues {
    pub values: Vec<String>,
    pub has_more: bool,
    pub generation: u64,
}

#[derive(Debug)]
pub enum TagQueryError {
    Workspace(WorkspaceQueryError),
    StaleCursor,
    InvalidLimit,
}
impl From<WorkspaceQueryError> for TagQueryError {
    fn from(e: WorkspaceQueryError) -> Self {
        Self::Workspace(e)
    }
}
impl std::fmt::Display for TagQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Workspace(e) => e.fmt(f),
            Self::StaleCursor => f.write_str("tag cursor belongs to another query or snapshot"),
            Self::InvalidLimit => f.write_str("tag page limit must be between 1 and 1000"),
        }
    }
}
impl std::error::Error for TagQueryError {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TagQueryWork {
    pub documents_read: usize,
    pub owners_read: usize,
    pub memberships_changed: usize,
    pub candidates_visited: usize,
}

pub(crate) struct StoredTagChanges {
    pub identity: Vec<u8>,
    pub sequence: i64,
    pub rebuild: bool,
    pub contributions: Vec<(PathBuf, Option<i64>, Vec<TaggedOwner>)>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Contribution {
    revision: i64,
    owners: OrdMap<OwnerKey, Arc<TaggedOwner>>,
}
/// Immutable owner lookup from one synchronized tag generation.
/// A missing owner has an empty tag set; labels never inherit.
#[derive(Debug, Clone, Default)]
pub struct OwnerTagSnapshot {
    sources: OrdMap<PathBuf, Contribution>,
}
impl OwnerTagSnapshot {
    /// `None` identifies the document root; `Some(start)` a parsed block.
    pub fn tags(&self, path: &Path, block_start: Option<usize>) -> &[String] {
        let key = block_start.map_or((TagOwner::Document, 0), |start| (TagOwner::Block, start));
        self.sources
            .get(&normalize(path))
            .and_then(|source| source.owners.get(&key))
            .map_or(&[], |owner| owner.tags.values.as_slice())
    }
}

#[derive(Debug, Clone, Default)]
struct Index {
    initialized: bool,
    identity: Option<Vec<u8>>,
    sequence: i64,
    generation: u64,
    dirty: OrdSet<PathBuf>,
    pending: OrdSet<PathBuf>,
    sources: OrdMap<PathBuf, Contribution>,
    postings: OrdMap<String, OrdSet<Key>>,
    work: TagQueryWork,
}
#[derive(Debug, Clone, Default)]
pub(crate) struct TagIndex(Arc<Mutex<Index>>);
impl TagIndex {
    pub fn fork(&self) -> Self {
        Self(Arc::new(Mutex::new(
            self.0.lock().expect("tag index").clone(),
        )))
    }
    pub fn changed(&mut self, path: PathBuf) {
        if Arc::get_mut(&mut self.0).is_none() {
            *self = self.fork();
        }
        Arc::get_mut(&mut self.0)
            .unwrap()
            .get_mut()
            .expect("tag index")
            .dirty
            .insert(path);
    }
    fn read<T>(
        &self,
        workspace: &Workspace,
        query: impl FnOnce(&mut Index) -> Result<T, TagQueryError>,
    ) -> Result<QueryResult<T>, TagQueryError> {
        let mut published = self.0.lock().expect("tag index");
        let mut next = published.clone();
        next.synchronize(workspace)?;
        let value = query(&mut next)?;
        let result = workspace.query_result_with_pending(value, !next.pending.is_empty());
        *published = next;
        Ok(result)
    }
}
impl Index {
    fn replace(
        &mut self,
        path: PathBuf,
        revision: Option<i64>,
        owners: Vec<TaggedOwner>,
        pending: bool,
    ) {
        let was_pending = self.pending.contains(&path);
        if pending {
            self.pending.insert(path.clone());
        } else {
            self.pending.remove(&path);
        }
        let contribution = revision
            .filter(|_| !owners.is_empty())
            .map(|revision| Contribution {
                revision,
                owners: owners
                    .into_iter()
                    .map(|o| ((o.owner, o.range.start), Arc::new(o)))
                    .collect(),
            });
        if self.sources.get(&path) == contribution.as_ref() && was_pending == pending {
            return;
        }
        self.generation = generation();
        let memberships = |c: Option<&Contribution>| -> OrdSet<(String, Key)> {
            c.into_iter()
                .flat_map(|c| c.owners.iter())
                .flat_map(|((owner, start), record)| {
                    let key = Key {
                        path: path.clone(),
                        owner: *owner,
                        start: *start,
                    };
                    record
                        .tags
                        .values
                        .iter()
                        .map(move |tag| (tag.clone(), key.clone()))
                })
                .collect()
        };
        let old = memberships(self.sources.get(&path));
        let new = memberships(contribution.as_ref());
        for (tag, key) in &old {
            if new.contains(&(tag.clone(), key.clone())) {
                continue;
            }
            self.work.memberships_changed += 1;
            let mut keys = self.postings[tag].clone();
            keys.remove(key);
            if keys.is_empty() {
                self.postings.remove(tag);
            } else {
                self.postings.insert(tag.clone(), keys);
            }
        }
        for (tag, key) in &new {
            if old.contains(&(tag.clone(), key.clone())) {
                continue;
            }
            self.work.memberships_changed += 1;
            let mut keys = self.postings.get(tag).cloned().unwrap_or_default();
            keys.insert(key.clone());
            self.postings.insert(tag.clone(), keys);
        }
        if let Some(c) = contribution {
            self.sources.insert(path, c);
        } else {
            self.sources.remove(&path);
        }
    }
    fn synchronize(&mut self, workspace: &Workspace) -> Result<(), WorkspaceQueryError> {
        self.work = TagQueryWork::default();
        let mut changed = self.dirty.clone();
        if let Some(store) = &workspace.disk_store {
            let requested: Vec<_> = changed
                .iter()
                .filter(|p| !workspace.documents.contains_key(*p))
                .cloned()
                .collect();
            let update = store.tag_contributions_since(
                self.identity.as_deref(),
                self.sequence,
                &requested,
            )?;
            if update.rebuild {
                *self = Self::default();
                self.generation = generation();
                changed = workspace.documents.keys().cloned().collect();
            }
            for (path, revision, owners) in update.contributions {
                self.work.documents_read += 1;
                self.work.owners_read += owners.len();
                if !workspace.documents.contains_key(&path) {
                    self.replace(path.clone(), revision, owners, false);
                    changed.remove(&path);
                }
            }
            self.identity = Some(update.identity);
            self.sequence = update.sequence;
        } else if self.identity.is_some() {
            *self = Self::default();
            self.generation = generation();
            changed = workspace.documents.keys().cloned().collect();
        }
        if !self.initialized {
            changed.extend(workspace.documents.keys().cloned());
        }
        for path in changed {
            self.work.documents_read += 1;
            let (revision, owners, pending) =
                workspace
                    .documents
                    .get(&path)
                    .map_or((None, Vec::new(), false), |entry| {
                        entry
                            .current
                            .as_ref()
                            .map_or((None, Vec::new(), true), |current| {
                                (Some(entry.revision), current.output.tagged_owners(), false)
                            })
                    });
            self.work.owners_read += owners.len();
            self.replace(path, revision, owners, pending);
        }
        self.dirty.clear();
        self.initialized = true;
        Ok(())
    }
    fn record(&self, key: &Key) -> TagMatch {
        let source = &self.sources[&key.path];
        TagMatch {
            path: key.path.clone(),
            revision: source.revision,
            record: (*source.owners[&(key.owner, key.start)]).clone(),
        }
    }
}
impl Workspace {
    /// Synchronize once per query, then resolve identities without store calls.
    pub fn owner_tag_snapshot(&self) -> Result<QueryResult<OwnerTagSnapshot>, WorkspaceQueryError> {
        self.derived
            .tags
            .read(self, |index| {
                Ok(OwnerTagSnapshot {
                    sources: index.sources.clone(),
                })
            })
            .map_err(|error| match error {
                TagQueryError::Workspace(error) => error,
                _ => unreachable!("snapshot lookup has no cursor or limit"),
            })
    }

    /// Exact, case-sensitive owner membership; no inheritance or implicit hierarchy.
    pub fn query_tag(
        &self,
        tag: &str,
        limit: usize,
        cursor: Option<&TagCursor>,
    ) -> Result<QueryResult<TagPage>, TagQueryError> {
        if !(1..=1000).contains(&limit) {
            return Err(TagQueryError::InvalidLimit);
        }
        self.derived.tags.read(self, |index| {
            if cursor.is_some_and(|c| c.generation != index.generation || c.tag != tag) {
                return Err(TagQueryError::StaleCursor);
            }
            let mut keys = Vec::new();
            if let Some(postings) = index.postings.get(tag) {
                let lower = cursor.map_or(Bound::Unbounded, |c| Bound::Excluded(&c.after));
                keys.extend(
                    postings
                        .range((lower, Bound::Unbounded))
                        .take(limit + 1)
                        .cloned(),
                );
            }
            index.work.candidates_visited += keys.len();
            let more = keys.len() > limit;
            keys.truncate(limit);
            let next_cursor = more.then(|| TagCursor {
                generation: index.generation,
                tag: tag.to_owned(),
                after: keys.last().unwrap().clone(),
            });
            Ok(TagPage {
                items: keys.iter().map(|key| index.record(key)).collect(),
                next_cursor,
                generation: index.generation,
            })
        })
    }
    /// Bounded literal-prefix vocabulary, ordered by decoded label.
    pub fn query_tag_values(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<QueryResult<TagValues>, TagQueryError> {
        if !(1..=1000).contains(&limit) {
            return Err(TagQueryError::InvalidLimit);
        }
        self.derived.tags.read(self, |index| {
            let mut values: Vec<_> = index
                .postings
                .range(prefix.to_owned()..)
                .take_while(|(label, _)| label.starts_with(prefix))
                .take(limit + 1)
                .map(|(label, _)| label.clone())
                .collect();
            index.work.candidates_visited += values.len();
            let has_more = values.len() > limit;
            values.truncate(limit);
            Ok(TagValues {
                values,
                has_more,
                generation: index.generation,
            })
        })
    }
    /// Source-preserving upward summary. A scope includes only fully contained block owners.
    /// None includes the document itself and all tagged blocks; no ancestor acquires their tags.
    pub fn document_tag_owners(
        &self,
        path: &Path,
        scope: Option<Range<usize>>,
    ) -> Result<QueryResult<Vec<TagMatch>>, TagQueryError> {
        let path = normalize(path);
        self.derived.tags.read(self, |index| {
            let mut result = Vec::new();
            if let Some(source) = index.sources.get(&path) {
                for ((owner, start), record) in &source.owners {
                    index.work.candidates_visited += 1;
                    if scope.as_ref().is_some_and(|r| {
                        *owner == TagOwner::Document
                            || r.start > record.range.start
                            || record.range.end > r.end
                    }) {
                        continue;
                    }
                    result.push(index.record(&Key {
                        path: path.clone(),
                        owner: *owner,
                        start: *start,
                    }));
                }
            }
            Ok(result)
        })
    }
    pub fn tag_query_work(&self) -> TagQueryWork {
        self.derived.tags.0.lock().expect("tag index").work
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SqliteSemanticStore;
    fn matches(w: &Workspace, tag: &str) -> Vec<TagMatch> {
        w.query_tag(tag, 100, None).unwrap().value.items
    }
    fn vocabulary(w: &Workspace) -> Vec<String> {
        w.query_tag_values("", 1000).unwrap().value.values
    }
    fn put(w: &mut Workspace, disk: bool, path: &str, revision: i64, source: &str) {
        if disk {
            w.insert_disk(path, revision, source).unwrap();
        } else {
            w.insert(path, revision, source);
        }
    }
    fn cel_titles(w: &Workspace, kind: crate::SearchRecordKind, expression: &str) -> Vec<String> {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-13T12:00:00Z").unwrap();
        let mut titles = w
            .search_records_filtered("", Some(kind), "", 100, now, Some(expression))
            .unwrap()
            .value
            .items
            .into_iter()
            .map(|r| r.title)
            .collect::<Vec<_>>();
        titles.sort();
        titles
    }

    #[test]
    fn cel_tags_are_direct_owner_lists_for_notes_tasks_events_and_pages() {
        use crate::{
            SearchRecordKind as Kind, TaskPageQuery, TaskQueryFilter, TaskQueryFilterGroup,
        };
        let source = concat!(
            "`= title Root\n`+ task\n`= tags root\n",
            "`= date 2026-11-13\n`= timezone +00:00\n",
            "`- Parent\n `+ task\n `= tags\n  `- project/xxx\n  `- spaced label\n  `- project/xxx\n",
            " `- Child\n  `+ task\n",
            "`- Invalid\n `+ task\n `= tags bad\n `= tags duplicate\n",
            "`- 10:00--11:00 Meeting\n `+ event\n `= tags project/xxx\n",
            "`- 12:00--13:00 Untagged\n `+ event\n",
        );
        for disk in [false, true] {
            let mut w = if disk {
                Workspace::with_sqlite_store(SqliteSemanticStore::open_in_memory().unwrap())
            } else {
                Workspace::new()
            };
            put(&mut w, disk, "a.plumb", 1, source);
            // A block at byte zero must never be mistaken for the document root.
            put(
                &mut w,
                disk,
                "b.plumb",
                1,
                "`- Zero\n `+ task\n `= tags zero\n",
            );
            assert_eq!(
                cel_titles(&w, Kind::Note, "tags.contains('root')"),
                ["Root"]
            );
            assert!(cel_titles(&w, Kind::Note, "tags.contains('zero')").is_empty());
            assert_eq!(
                cel_titles(&w, Kind::Task, "tags.contains('zero')"),
                ["Zero"]
            );
            assert_eq!(cel_titles(&w, Kind::Task, "'root' in tags"), ["Root"]);
            for expression in [
                "tags.contains('project/xxx')",
                "'project/xxx' in tags",
                "tags.exists(t, t.startsWith('project/'))",
            ] {
                assert_eq!(cel_titles(&w, Kind::Task, expression), ["Parent"]);
                assert_eq!(cel_titles(&w, Kind::Event, expression), ["Meeting"]);
                let query = TaskPageQuery {
                    root: PathBuf::new(),
                    text: String::new(),
                    filter_groups: vec![TaskQueryFilterGroup {
                        filters: vec![TaskQueryFilter {
                            source: "test".into(),
                            expression: expression.into(),
                        }],
                    }],
                    sort: vec![],
                    limit: 100,
                    cursor: None,
                    workspace_revision: 1,
                    now: chrono::DateTime::parse_from_rfc3339("2026-11-13T12:00:00Z").unwrap(),
                };
                let page = w.query_task_page(&query).unwrap();
                assert_eq!(
                    page.value
                        .tasks
                        .iter()
                        .map(|t| t.task.title.as_str())
                        .collect::<Vec<_>>(),
                    ["Parent"]
                );
            }
            assert_eq!(
                cel_titles(&w, Kind::Task, "tags == ['project/xxx', 'spaced label']"),
                ["Parent"]
            );
            assert_eq!(
                cel_titles(&w, Kind::Task, "size(tags) == 0"),
                ["Child", "Invalid"]
            );
            assert_eq!(cel_titles(&w, Kind::Event, "tags == []"), ["Untagged"]);
            assert!(cel_titles(
                &w,
                Kind::Task,
                "tags.contains('project') || tags.contains('PROJECT/xxx')"
            )
            .is_empty());
            let now = chrono::DateTime::parse_from_rfc3339("2026-11-13T12:00:00Z").unwrap();
            assert!(w
                .search_records_filtered(
                    "",
                    Some(Kind::Task),
                    "",
                    100,
                    now,
                    Some("document_tags.contains('root')")
                )
                .is_err());
            assert!(w
                .search_records_filtered("", Some(Kind::Task), "", 100, now, Some("tags + 1"))
                .is_err());
        }
    }

    #[test]
    fn cel_tag_snapshots_bound_work_and_preserve_overlay_revision() {
        use crate::SearchRecordKind as Kind;
        for disk in [false, true] {
            let mut w = if disk {
                Workspace::with_sqlite_store(SqliteSemanticStore::open_in_memory().unwrap())
            } else {
                Workspace::new()
            };
            let source = "`= title Root\n`= tags old\n";
            put(&mut w, disk, "a.plumb", 1, source);
            let old = w.owner_tag_snapshot().unwrap().value;
            assert_eq!(old.tags(Path::new("a.plumb"), None), ["old"]);
            assert!(old.tags(Path::new("missing.plumb"), None).is_empty());
            for _ in 0..3 {
                assert_eq!(cel_titles(&w, Kind::Note, "tags.contains('old')"), ["Root"]);
                assert_eq!(w.tag_query_work(), TagQueryWork::default());
            }
            for i in 0..100 {
                put(
                    &mut w,
                    disk,
                    &format!("unrelated-{i}.plumb"),
                    1,
                    "`= tags other\n",
                );
            }
            w.owner_tag_snapshot().unwrap();
            assert_eq!(w.tag_query_work().documents_read, 100);
            assert_eq!(cel_titles(&w, Kind::Note, "tags.contains('old')"), ["Root"]);
            assert_eq!(w.tag_query_work(), TagQueryWork::default());
            put(
                &mut w,
                disk,
                "unrelated-0.plumb",
                2,
                "`= tags other\n\nChanged body\n",
            );
            w.owner_tag_snapshot().unwrap();
            assert_eq!(w.tag_query_work().documents_read, 1);
            assert_eq!(w.tag_query_work().memberships_changed, 0);
            w.insert("a.plumb", 2, "`= title Root\n`= tags overlay\n");
            assert_eq!(
                cel_titles(&w, Kind::Note, "tags.contains('overlay')"),
                ["Root"]
            );
            assert_eq!(w.tag_query_work().documents_read, 1);
            let cold = if disk {
                let mut cold = Workspace::with_sqlite_store(
                    w.disk_store.as_ref().unwrap().readonly_snapshot().unwrap(),
                );
                cold.insert("a.plumb", 2, "`= title Root\n`= tags overlay\n");
                cold
            } else {
                let mut cold = Workspace::new();
                for (path, entry) in &w.documents {
                    cold.insert(path, entry.revision, entry.parsed.source());
                }
                cold
            };
            assert_eq!(
                cel_titles(&w, Kind::Note, "tags.contains('overlay')"),
                cel_titles(&cold, Kind::Note, "tags.contains('overlay')")
            );
            let pending = w
                .begin_document_revision("a.plumb", 3, "`= title Root\n`= tags {broken\n")
                .unwrap();
            let snapshot = w.owner_tag_snapshot().unwrap();
            assert!(!snapshot.is_complete());
            assert!(snapshot.value.tags(Path::new("a.plumb"), None).is_empty());
            w.install_document_analysis(pending.analyze());
            assert!(cel_titles(&w, Kind::Note, "tags.contains('overlay')").is_empty());
            assert_eq!(old.tags(Path::new("a.plumb"), None), ["old"]);
            w.close_document("a.plumb");
            if disk {
                assert_eq!(cel_titles(&w, Kind::Note, "tags.contains('old')"), ["Root"]);
            }
            w.remove_disk("a.plumb").unwrap();
            assert!(w
                .owner_tag_snapshot()
                .unwrap()
                .value
                .tags(Path::new("a.plumb"), None)
                .is_empty());
            put(&mut w, disk, "missing.plumb", 1, "`= tags recovered\n");
            assert_eq!(
                w.owner_tag_snapshot()
                    .unwrap()
                    .value
                    .tags(Path::new("missing.plumb"), None),
                ["recovered"]
            );
            assert!(old.tags(Path::new("missing.plumb"), None).is_empty());
        }
    }

    #[test]
    fn tag_reads_updates_growth_and_negative_lookup_have_bounded_work() {
        for disk in [false, true] {
            let mut w = if disk {
                Workspace::with_sqlite_store(SqliteSemanticStore::open_in_memory().unwrap())
            } else {
                Workspace::new()
            };
            put(&mut w, disk, "a.plumb", 1, "`- A\n `= tags target\n");
            assert_eq!(matches(&w, "target").len(), 1);
            assert_eq!(matches(&w, "missing").len(), 0);
            assert_eq!(w.tag_query_work().documents_read, 0);
            for i in 0..200 {
                put(
                    &mut w,
                    disk,
                    &format!("unrelated-{i}.plumb"),
                    1,
                    "`= tags unrelated\n",
                );
            }
            assert_eq!(matches(&w, "target").len(), 1);
            assert_eq!(w.tag_query_work().documents_read, 200);
            assert_eq!(matches(&w, "target").len(), 1);
            assert_eq!(
                w.tag_query_work(),
                TagQueryWork {
                    candidates_visited: 1,
                    ..Default::default()
                }
            );
            put(
                &mut w,
                disk,
                "unrelated-0.plumb",
                2,
                "`= tags unrelated\n\nChanged text\n",
            );
            matches(&w, "target");
            assert_eq!(w.tag_query_work().documents_read, 1);
            assert_eq!(w.tag_query_work().memberships_changed, 0);
            put(&mut w, disk, "a.plumb", 2, "`- A\n `= tags missing\n");
            assert_eq!(matches(&w, "missing").len(), 1);
            assert_eq!(w.tag_query_work().documents_read, 1);
            assert_eq!(w.tag_query_work().memberships_changed, 2);
            assert!(matches(&w, "target").is_empty());
            let cold = if disk {
                Workspace::with_sqlite_store(
                    w.disk_store.as_ref().unwrap().readonly_snapshot().unwrap(),
                )
            } else {
                let mut cold = Workspace::new();
                for (path, entry) in &w.documents {
                    cold.insert(path, entry.revision, entry.parsed.source());
                }
                cold
            };
            assert_eq!(vocabulary(&w), vocabulary(&cold));
            assert_eq!(matches(&w, "missing"), matches(&cold, "missing"));
            w.remove_disk("a.plumb").unwrap();
            assert!(matches(&w, "missing").is_empty());
        }
    }
    #[test]
    fn tag_overlays_pending_invalid_close_and_snapshots_are_current() {
        let mut w = Workspace::with_sqlite_store(SqliteSemanticStore::open_in_memory().unwrap());
        w.insert_disk("a.plumb", 1, "`= tags disk\n").unwrap();
        assert_eq!(vocabulary(&w), ["disk"]);
        let snapshot =
            w.with_updated_store(w.disk_store.as_ref().unwrap().readonly_snapshot().unwrap());
        w.insert("a.plumb", 2, "`= tags overlay\n");
        assert_eq!(vocabulary(&w), ["overlay"]);
        let pending = w
            .begin_document_revision("a.plumb", 3, "`= tags {broken\n`- Healthy\n `= tags good\n")
            .unwrap();
        let result = w.query_tag_values("", 100).unwrap();
        assert!(!result.is_complete());
        assert!(result.value.values.is_empty());
        w.install_document_analysis(pending.analyze());
        assert_eq!(vocabulary(&w), ["good"]);
        assert!(w.query_tag("good", 10, None).unwrap().is_complete());
        assert_eq!(vocabulary(&snapshot), ["disk"]);
        w.close_document("a.plumb");
        assert_eq!(vocabulary(&w), ["disk"]);
        w.insert_disk("a.plumb", 4, "`= tags latest\n").unwrap();
        assert_eq!(vocabulary(&w), ["latest"]);
        assert_eq!(vocabulary(&snapshot), ["disk"]);
    }
    #[test]
    fn tag_membership_pagination_and_subtree_summary_keep_exact_sources() {
        let mut w = Workspace::new();
        let source = "`= tags doc\n`- Parent\n `= tags shared\n `- Child\n  `= tags shared\n`- Other\n `= tags shared\n";
        w.insert("a.plumb", 7, source);
        let first = w.query_tag("shared", 1, None).unwrap().value;
        assert_eq!(first.items.len(), 1);
        let parent = &first.items[0].record;
        let subtree = w
            .document_tag_owners(Path::new("a.plumb"), Some(parent.range.clone()))
            .unwrap()
            .value;
        assert_eq!(subtree.len(), 2);
        assert_eq!(subtree[0].record.title, "Parent");
        assert_eq!(subtree[1].record.title, "Child");
        assert!(subtree.iter().all(|m| m.record.tags.values == ["shared"]));
        let second = w
            .query_tag("shared", 1, first.next_cursor.as_ref())
            .unwrap()
            .value;
        let last = w
            .query_tag("shared", 1, second.next_cursor.as_ref())
            .unwrap()
            .value;
        assert_eq!(last.items[0].record.title, "Other");
        assert!(last.next_cursor.is_none());
        assert_eq!(w.tag_query_work().candidates_visited, 1);
        let old = w.clone();
        w.insert("a.plumb", 8, format!("Intro\n\n{source}"));
        assert!(matches!(
            w.query_tag("shared", 1, first.next_cursor.as_ref()),
            Err(TagQueryError::StaleCursor)
        ));
        assert_eq!(
            old.query_tag("shared", 1, first.next_cursor.as_ref())
                .unwrap()
                .value,
            second
        );
        assert_eq!(matches(&w, "shared")[0].revision, 8);
        assert!(matches(&w, "Shared").is_empty());
    }
    #[test]
    fn tag_completion_reports_truncation_and_visits_only_a_bounded_page() {
        let mut workspace = Workspace::new();
        let labels = (0..150)
            .map(|i| format!(" `- label-{i:03}\n"))
            .collect::<String>();
        workspace.insert("a.plumb", 1, format!("`= tags\n{labels}"));
        let context = plumb_semantics::TagCompletionContext {
            replace: 0..0,
            query: "label-".into(),
        };
        let result = workspace.complete_tag(&context).unwrap();
        assert!(!result.is_complete());
        assert_eq!(result.value.len(), 100);
        assert_eq!(workspace.tag_query_work().candidates_visited, 101);
        let context = plumb_semantics::TagCompletionContext {
            query: "label-14".into(),
            ..context
        };
        let result = workspace.complete_tag(&context).unwrap();
        assert!(result.is_complete());
        assert_eq!(result.value.len(), 10);
        assert_eq!(workspace.tag_query_work().documents_read, 0);
        assert_eq!(workspace.tag_query_work().candidates_visited, 10);
        assert!(matches!(
            workspace.query_tag("label-000", 0, None),
            Err(TagQueryError::InvalidLimit)
        ));
        assert!(matches!(
            workspace.query_tag_values("", 1001),
            Err(TagQueryError::InvalidLimit)
        ));
    }
    #[test]
    fn tag_completion_uses_index_and_renders_special_characters_in_edit_layer() {
        let mut w = Workspace::new();
        w.insert("a.plumb", 1, "`= tags\n `- 中文 标签\n `- a`{b`}\n");
        let c = plumb_semantics::TagCompletionContext {
            replace: 7..8,
            query: "a".into(),
        };
        let values = w.complete_tag(&c).unwrap().value;
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].label, "a{b}");
        let source = format!("`= tags {}\n", values[0].new_text);
        w.insert("new.plumb", 1, source);
        assert_eq!(matches(&w, "a{b}").len(), 2);
    }
}

#[cfg(test)]
mod timing {
    use super::*;
    #[test]
    #[ignore = "explicit CEL owner tag lookup timing evidence"]
    fn cel_owner_tag_timing() {
        for disk in [false, true] {
            for size in [100, 1000] {
                let mut workspace = if disk {
                    Workspace::with_sqlite_store(
                        crate::SqliteSemanticStore::open_in_memory().unwrap(),
                    )
                } else {
                    Workspace::new()
                };
                for i in 0..size {
                    let path = format!("{i}.plumb");
                    let source = format!("`= tags tag-{i}\n");
                    if disk {
                        workspace.insert_disk(&path, 1, source).unwrap();
                    } else {
                        workspace.insert(&path, 1, source);
                    }
                }
                let program = cel::Program::compile("tags.contains('tag-1')").unwrap();
                workspace.owner_tag_snapshot().unwrap();
                let start = std::time::Instant::now();
                for _ in 0..1000 {
                    let snapshot = workspace.owner_tag_snapshot().unwrap();
                    let mut context = cel::Context::default();
                    context.add_variable_from_value(
                        "tags",
                        snapshot.value.tags(Path::new("1.plumb"), None).to_vec(),
                    );
                    assert_eq!(
                        std::hint::black_box(program.execute(&context).unwrap()),
                        cel::Value::Bool(true)
                    );
                }
                eprintln!(
                    "CEL tags disk={disk} owners={size}: 1000 warm snapshot/lookup/evaluate={:?}",
                    start.elapsed()
                );
                assert_eq!(workspace.tag_query_work(), TagQueryWork::default());
            }
        }
    }

    #[test]
    #[ignore = "explicit tag query timing evidence"]
    fn tag_query_timing() {
        let mut workspace = Workspace::new();
        for i in 0..1000 {
            workspace.insert(
                format!("{i}.plumb"),
                1,
                format!("`- Note {i}\n `= tags tag-{i}\n"),
            );
        }
        let cold = std::time::Instant::now();
        workspace.query_tag("tag-1", 20, None).unwrap();
        let cold_elapsed = cold.elapsed();
        let warm = std::time::Instant::now();
        for _ in 0..1000 {
            workspace.query_tag("tag-1", 20, None).unwrap();
        }
        eprintln!(
            "tags 1000 documents: cold={cold_elapsed:?}, 1000 warm queries={:?}",
            warm.elapsed()
        );
        assert_eq!(workspace.tag_query_work().documents_read, 0);
    }
}
