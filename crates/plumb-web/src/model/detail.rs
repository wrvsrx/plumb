use super::*;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DetailQuery {
    pub start: Option<usize>,
    pub task_id: Option<String>,
    pub revision: Option<String>,
}

impl WebWorkspace {
    /// One request owns one parsed revision; no complete tree escapes this call.
    pub(crate) fn node_detail(
        &self,
        id: &str,
        query: &DetailQuery,
    ) -> Result<serde_json::Value, String> {
        let path = self.document_path(id).ok_or("unknown document")?;
        let document = self.documents.get(path).ok_or("unknown document")?;
        if (query.start.is_some() || query.task_id.is_some()) && query.revision.is_none() {
            return Err("node selection requires a document revision".into());
        }
        if query
            .revision
            .as_deref()
            .is_some_and(|revision| revision != document.revision.to_string())
        {
            return Err("node selection is stale; refresh the view".into());
        }
        if query.start.is_some() && query.task_id.is_some() {
            return Err("node selection must use one locator".into());
        }
        let entry = self.document_entry(path)?.ok_or("unknown document")?;
        let analysis = &entry
            .current
            .as_ref()
            .ok_or("document has syntax errors")?
            .output;
        let tasks = analysis.tasks();
        let start = if let Some(task_id) = &query.task_id {
            Some(
                tasks
                    .tasks
                    .iter()
                    .find(|task| {
                        task.id
                            .as_ref()
                            .is_some_and(|field| field.value == *task_id)
                    })
                    .ok_or("task is no longer available")?
                    .range
                    .start,
            )
        } else {
            query.start.map(|start| {
                tasks
                    .tasks
                    .iter()
                    .find(|task| {
                        task.owner == plumb_semantics::TaskOwner::ListItem
                            && task.source_key() == start
                    })
                    .map_or(start, |task| task.range.start)
            })
        };
        let task = tasks.tasks.iter().find(|task| match start {
            Some(start) => {
                task.owner == plumb_semantics::TaskOwner::ListItem && task.range.start == start
            }
            None => task.owner == plumb_semantics::TaskOwner::Document,
        });
        let children = if let Some(parent) = &task {
            tasks
                .tasks
                .iter()
                .filter(|child| {
                    child.depth == parent.depth + 1
                        && child.owner == plumb_semantics::TaskOwner::ListItem
                        && child.range.start >= parent.range.start
                        && child.range.end <= parent.range.end
                })
                .map(|child| {
                    let (key, locator) = web_task_identity(id, &child);
                    serde_json::json!({"key": key, "locator": locator, "title": child.title,
                        "revision": document.revision.to_string(), "documentId": id})
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let event = start.and_then(|start| {
            analysis
                .events()
                .events
                .iter()
                .find(|event| event.range.start == start)
        });
        let task = task.map(|task| self.detail_task(path, task)).transpose()?;
        let event = event
            .map(|event| {
                self.web_event(&WorkspaceEvent {
                    path: path.to_path_buf(),
                    revision: document.revision,
                    event,
                })
            })
            .transpose()?
            .flatten();
        let mut pandoc =
            plumb_export::export_detail(&entry.parsed.green().materialize(), analysis, start)?;
        let bibliography = load_bibliography(&self.root, path, analysis.metadata());
        let html = crate::pandoc::render_document_html(self, path, &mut pandoc, &bibliography)?;
        Ok(serde_json::json!({
            "id": id,
            "title": task.as_ref().map(|task| task.title.as_str())
                .or_else(|| event.as_ref().map(|event| event.title.as_str()))
                .unwrap_or_else(|| self.titles.get(path).map(String::as_str).unwrap_or("")),
            "path": display_path(&self.root, path),
            "revision": document.revision.to_string(),
            "html": html,
            "task": task,
            "event": event,
            "children": children,
        }))
    }

    fn detail_task(&self, path: &Path, task: TaskRecord) -> Result<WebTask, String> {
        let (state, wait_reasons) = self
            .workspace
            .task_workflow_state(path, &task, Local::now().fixed_offset())
            .map_err(|error| error.to_string())?
            .value;
        let blocked = self
            .workspace
            .is_task_blocked(path, &task)
            .map_err(|error| error.to_string())?
            .value;
        let durations = self
            .workspace
            .task_durations_for([plumb_workspace::AgendaItem {
                path: path.to_path_buf(),
                id: task.id.as_ref().map(|field| field.value.clone()),
            }])?;
        let depends_on = self
            .workspace
            .task_dependencies(path, &task)
            .map_err(|error| error.to_string())?
            .value
            .into_iter()
            .map(|dependency| dependency.target)
            .collect();
        let previous = self
            .workspace
            .task_previous(path, &task)
            .map_err(|error| error.to_string())?
            .value;
        self.web_task(
            WorkspaceTask {
                matched: true,
                path: path.to_path_buf(),
                revision: self.documents[path].revision,
                effective_priority: task.priority.unwrap_or_default(),
                task,
                state,
                wait_reasons,
                blocked,
                actionable: state == plumb_workspace::TaskWorkflowState::Ready,
                relevance: 0,
                depends_on,
                directly_blocking: Vec::new(),
                previous,
            },
            &durations,
        )
        .ok_or_else(|| "task document is unavailable".into())
    }
}
