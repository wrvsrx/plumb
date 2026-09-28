//! Closed-document queries read persistent facts, never materialize syntax.
//! An open revision shadows the entire disk generation, including when invalid.
use crate::*;
use plumb_semantics::{EmbedRecord, TaskReferenceTarget};

impl Workspace {
    pub fn document_links(
        &self,
        path: &Path,
    ) -> Result<QueryResult<Vec<LinkRecord>>, WorkspaceQueryError> {
        let path = normalize(path);
        let records = if self.documents.contains_key(&path) {
            self.current_output(&path)
                .map(|o| o.links().iter().collect())
                .unwrap_or_default()
        } else if let Some(store) = &self.disk_store {
            store.links_for_path(&path)?
        } else {
            Vec::new()
        };
        Ok(self.query_result(records))
    }
    pub fn document_tasks(
        &self,
        path: &Path,
    ) -> Result<QueryResult<Vec<TaskRecord>>, WorkspaceQueryError> {
        Ok(self.query_result(self.tasks_for_path(path)?))
    }
    pub fn document_events(
        &self,
        path: &Path,
    ) -> Result<QueryResult<Vec<EventRecord>>, WorkspaceQueryError> {
        let path = normalize(path);
        let records = if self.documents.contains_key(&path) {
            self.current_output(&path)
                .map(|o| o.events().events.iter().collect())
                .unwrap_or_default()
        } else if let Some(store) = &self.disk_store {
            store.events_for_path(&path)?
        } else {
            Vec::new()
        };
        Ok(self.query_result(records))
    }
    pub fn document_embeds(
        &self,
        path: &Path,
    ) -> Result<QueryResult<Vec<EmbedRecord>>, WorkspaceQueryError> {
        let path = normalize(path);
        let records = if self.documents.contains_key(&path) {
            self.current_output(&path)
                .map(|o| o.embeds().iter().collect())
                .unwrap_or_default()
        } else if let Some(store) = &self.disk_store {
            store
                .diagnostic_inputs(&path)?
                .map(|inputs| inputs.embeds)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        Ok(self.query_result(records))
    }
    pub fn resolve_task_reference(
        &self,
        from: &Path,
        target: &TaskReferenceTarget,
    ) -> Result<QueryResult<ResolvedTarget>, WorkspaceQueryError> {
        Ok(self.query_result(self.resolve_task_reference_target(from, target)?))
    }
}

impl Workspace {
    /// Hydrate one selected task, without loading the source document.
    pub fn indexed_task_at_selection(
        &self,
        path: &Path,
        start: usize,
    ) -> Result<QueryResult<Option<TaskRecord>>, WorkspaceQueryError> {
        let path = normalize(path);
        let record = if self.documents.contains_key(&path) {
            self.current_output(&path).and_then(|o| {
                o.tasks()
                    .tasks
                    .iter()
                    .find(|t| t.selection_range.start == start)
            })
        } else if let Some(store) = &self.disk_store {
            store.task_at_selection(&path, start)?
        } else {
            None
        };
        Ok(self.query_result(record))
    }
    /// Hydrate one event by source identity, not by a cursor's time ordering.
    pub fn indexed_event(
        &self,
        path: &Path,
        start: usize,
    ) -> Result<QueryResult<Option<EventRecord>>, WorkspaceQueryError> {
        let path = normalize(path);
        let record = if self.documents.contains_key(&path) {
            self.current_output(&path)
                .and_then(|o| o.events().events.iter().find(|e| e.range.start == start))
        } else if let Some(store) = &self.disk_store {
            store
                .events_by_source_keys(&[store::StoredEventSourceKey { path, start }])?
                .into_iter()
                .next()
                .map(|r| r.record)
        } else {
            None
        };
        Ok(self.query_result(record))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_facts_shadow_disk_and_close_releases_the_unsaved_tree() {
        let path = Path::new("source.plumb");
        let source = "`- Task\n `+ task\n `@ task\n`- 2026-09-28T10:00:00Z Event\n `+ event\n`->{#task}\n`->{image.png `+{embed}}\n";
        let store = SqliteSemanticStore::open_in_memory().unwrap();
        let mut disk = Workspace::with_sqlite_store(store);
        disk.insert_disk(path, 1, source).unwrap();
        let mut memory = Workspace::new();
        memory.open_document(path, 1, source);
        assert_eq!(
            disk.document_links(path).unwrap().value,
            memory.document_links(path).unwrap().value
        );
        assert_eq!(
            disk.document_tasks(path).unwrap().value,
            memory.document_tasks(path).unwrap().value
        );
        assert_eq!(
            disk.document_events(path).unwrap().value,
            memory.document_events(path).unwrap().value
        );
        assert_eq!(
            disk.document_embeds(path).unwrap().value,
            memory.document_embeds(path).unwrap().value
        );
        let task = disk.document_tasks(path).unwrap().value.remove(0);
        let event = disk.document_events(path).unwrap().value.remove(0);
        assert_eq!(
            disk.indexed_task_at_selection(path, task.selection_range.start)
                .unwrap()
                .value,
            Some(task)
        );
        assert_eq!(
            disk.indexed_event(path, event.range.start).unwrap().value,
            Some(event)
        );
        assert!(disk.documents().next().is_none());
        let entry = disk.open_document(path, 2, "`- Unsaved\n `+ task\n `@ unsaved\n");
        let tree = Arc::downgrade(entry.parsed.green());
        assert_eq!(disk.document_tasks(path).unwrap().value[0].title, "Unsaved");
        assert!(disk.document_links(path).unwrap().value.is_empty());
        assert!(disk.document_events(path).unwrap().value.is_empty());
        assert!(disk.document_embeds(path).unwrap().value.is_empty());
        drop(disk.close_document(path));
        assert!(tree.upgrade().is_none());
        assert_eq!(disk.document_tasks(path).unwrap().value[0].title, "Task");
        disk.open_document(path, 3, "`->{unclosed\n");
        assert!(disk.document_tasks(path).unwrap().value.is_empty());
        assert!(disk.document_events(path).unwrap().value.is_empty());
        assert!(disk.document_links(path).unwrap().value.is_empty());
        assert!(disk.document_embeds(path).unwrap().value.is_empty());
        drop(disk.close_document(path));
        assert_eq!(disk.document_events(path).unwrap().value.len(), 1);
        let link = disk.document_links(path).unwrap().value.remove(0);
        assert!(matches!(
            disk.resolve_link(path, &link).unwrap().value,
            ResolvedTarget::Anchor { .. }
        ));
        assert!(matches!(
            disk.resolve_task_reference(path, &TaskReferenceTarget::Internal { id: "task".into() })
                .unwrap()
                .value,
            ResolvedTarget::Anchor { .. }
        ));
        assert!(disk.documents().next().is_none());
    }
}
