use super::*;

impl WebWorkspace {
    pub fn set_task_status(
        &self,
        document_id: &str,
        locator: &WebTaskLocator,
        revision: &str,
        status: TaskStatus,
    ) -> Result<(), String> {
        let path = self
            .document_path(document_id)
            .ok_or_else(|| "unknown task document".to_string())?;
        let entry = self
            .document_entry(path)?
            .filter(|entry| entry.current.is_some())
            .ok_or_else(|| "task document is invalid".to_string())?;
        if entry.revision.to_string() != revision {
            return Err("task document changed; refresh before retrying".to_string());
        }
        let disk_source = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if disk_source != entry.parsed.source {
            return Err("task document changed on disk; refresh before retrying".to_string());
        }
        let timestamp = Local::now()
            .fixed_offset()
            .to_rfc3339_opts(SecondsFormat::Secs, false);
        let operation_workspace = self.operation_workspace(path)?;
        let edit = match locator {
            WebTaskLocator::Id { id } => {
                operation_workspace.set_task_status_by_id(path, id, status, &timestamp)
            }
            WebTaskLocator::Offset { offset } => {
                let indexed = entry
                    .current
                    .as_ref()
                    .expect("current output checked")
                    .output
                    .tasks
                    .tasks
                    .iter()
                    .any(|task| task.range.start == *offset);
                if !indexed {
                    return Err("task position changed; refresh before retrying".to_string());
                }
                operation_workspace.set_task_status(path, *offset, status, &timestamp)
            }
        }
        .map_err(|error| error.to_string())?;
        let updated = apply_guarded_edit(disk_source, path, entry.revision, edit, "task")?;
        validate_generated_source(path, entry.revision, &updated, "task")?;
        std::fs::write(path, updated)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))
    }

    pub fn create_task(
        &self,
        document_id: &str,
        revision: &str,
        input: &WebTaskInput,
        placement: &WebTaskPlacement,
    ) -> Result<(), String> {
        let path = self.guarded_document(document_id, revision, "task")?;
        let source = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let input = self.task_authoring_input(path, input)?;
        let operation_workspace = self.operation_workspace(path)?;
        let placement = self.task_placement(&operation_workspace, path, placement)?;
        let timestamp = Local::now()
            .fixed_offset()
            .to_rfc3339_opts(SecondsFormat::Secs, false);
        let edit = self
            .operation_workspace(path)?
            .create_task(path, &input, &placement, &timestamp)
            .map_err(task_authoring_error)?;
        self.write_workspace_edit(path, source, edit, "task")
    }

    pub fn update_task_fields(
        &self,
        document_id: &str,
        locator: &WebTaskLocator,
        revision: &str,
        input: &WebTaskInput,
        placement: Option<&WebTaskPlacement>,
    ) -> Result<(), String> {
        let path = self.guarded_document(document_id, revision, "task")?;
        let source = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let timestamp = Local::now()
            .fixed_offset()
            .to_rfc3339_opts(SecondsFormat::Secs, false);
        let operation_workspace = self.operation_workspace(path)?;
        let original_range = task_range_in(&operation_workspace, path, locator)?;
        let placement = placement
            .map(|placement| self.task_placement(&operation_workspace, path, placement))
            .transpose()?;
        let input = self.task_authoring_input(path, input)?;
        let edit = operation_workspace
            .update_and_move_task(path, original_range, &input, placement.as_ref(), &timestamp)
            .map_err(task_authoring_error)?;
        self.write_workspace_edit(path, source, edit, "task")
    }

    fn task_placement(
        &self,
        workspace: &Workspace,
        path: &Path,
        placement: &WebTaskPlacement,
    ) -> Result<TaskPlacement, String> {
        Ok(TaskPlacement {
            parent: placement
                .parent
                .as_ref()
                .map(|locator| task_range_in(workspace, path, locator))
                .transpose()?,
            after: placement
                .after
                .as_ref()
                .map(|locator| task_range_in(workspace, path, locator))
                .transpose()?,
        })
    }

    fn task_authoring_input(
        &self,
        source_path: &Path,
        input: &WebTaskInput,
    ) -> Result<TaskAuthoringInput, String> {
        Ok(TaskAuthoringInput {
            title: input.title.clone(),
            created: input.created.clone().filter(|value| !value.is_empty()),
            due: input.due.clone().filter(|value| !value.is_empty()),
            wait: input.wait.clone().filter(|value| !value.is_empty()),
            recur: input.recur.clone().filter(|value| !value.is_empty()),
            prev: input
                .prev
                .as_ref()
                .map(|reference| self.task_reference_input(source_path, reference))
                .transpose()?,
            depends: input
                .depends
                .iter()
                .map(|reference| self.task_reference_input(source_path, reference))
                .collect::<Result<Vec<_>, _>>()?,
            priority: input.priority,
        })
    }

    fn task_reference_input(
        &self,
        source_path: &Path,
        reference: &WebTaskReferenceInput,
    ) -> Result<String, String> {
        let target_path = self
            .document_path(&reference.document_id)
            .ok_or_else(|| "unknown task reference document".to_string())?;
        let entry = self
            .document_entry(target_path)?
            .and_then(|entry| entry.current.as_ref())
            .ok_or_else(|| "task reference is no longer available".to_string())?;
        let task = self
            .task_for_locator(entry.output.as_ref(), &reference.locator)
            .ok_or_else(|| "task reference is no longer available".to_string())?;
        let id = task
            .id
            .as_ref()
            .ok_or_else(|| "task references require an explicit id".to_string())?;
        if target_path == source_path {
            Ok(format!("#{}", id.value))
        } else {
            let relative = relative_web_path(source_path, target_path)
                .ok_or_else(|| "task reference path is not valid UTF-8".to_string())?;
            Ok(format!("{relative}#{}", id.value))
        }
    }

    pub fn create_event(
        &self,
        document_id: &str,
        revision: &str,
        input: &WebEventInput,
    ) -> Result<(), String> {
        let input = event_input(input);
        self.mutate_event(document_id, revision, |workspace, path| {
            workspace.create_event(path, &input)
        })
    }

    pub fn update_event(
        &self,
        document_id: &str,
        locator: &WebEventLocator,
        revision: &str,
        input: &WebEventInput,
    ) -> Result<(), String> {
        let range = locator.start..locator.end;
        let input = event_input(input);
        self.mutate_event(document_id, revision, |workspace, path| {
            workspace.update_event(path, range, &input)
        })
    }

    pub fn delete_event(
        &self,
        document_id: &str,
        locator: &WebEventLocator,
        revision: &str,
    ) -> Result<(), String> {
        let range = locator.start..locator.end;
        self.mutate_event(document_id, revision, |workspace, path| {
            workspace.delete_event(path, range)
        })
    }

    fn mutate_event(
        &self,
        document_id: &str,
        revision: &str,
        mutation: impl FnOnce(
            &Workspace,
            &Path,
        ) -> Result<plumb_workspace::WorkspaceEdit, EventEditError>,
    ) -> Result<(), String> {
        let path = self.guarded_document(document_id, revision, "event")?;
        let source = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let operation_workspace = self.operation_workspace(path)?;
        let edit = mutation(&operation_workspace, path).map_err(event_edit_error)?;
        self.write_workspace_edit(path, source, edit, "event")
    }

    fn guarded_document<'a>(
        &'a self,
        document_id: &str,
        revision: &str,
        kind: &str,
    ) -> Result<&'a Path, String> {
        let path = self
            .document_path(document_id)
            .ok_or_else(|| format!("unknown {kind} document"))?;
        let entry = self
            .document_entry(path)?
            .filter(|entry| entry.current.is_some())
            .ok_or_else(|| format!("{kind} document is invalid"))?;
        if entry.revision.to_string() != revision {
            return Err(format!("{kind} document changed; refresh before retrying"));
        }
        let disk_source = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if disk_source != entry.parsed.source {
            return Err(format!(
                "{kind} document changed on disk; refresh before retrying"
            ));
        }
        Ok(path)
    }

    fn write_workspace_edit(
        &self,
        path: &Path,
        source: String,
        edit: plumb_workspace::WorkspaceEdit,
        kind: &str,
    ) -> Result<(), String> {
        let revision = self
            .document_entry(path)?
            .ok_or_else(|| format!("{kind} document is no longer indexed"))?
            .revision;
        let updated = apply_guarded_edit(source, path, revision, edit, kind)?;
        validate_generated_source(path, revision, &updated, kind)?;
        std::fs::write(path, updated)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))
    }
}
