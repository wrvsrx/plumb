use crate::*;
use plumb_semantics::EmbedRecord;
use serde::{Deserialize, Serialize};

/// Owned diagnostic projection for disk generations; codes are data, never leaked strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceDiagnostic {
    pub code: String,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub range: std::ops::Range<usize>,
    pub related: Vec<std::ops::Range<usize>>,
}
impl From<Diagnostic> for WorkspaceDiagnostic {
    fn from(value: Diagnostic) -> Self {
        Self {
            code: value.code.to_owned(),
            severity: value.severity,
            message: value.message,
            range: value.range,
            related: value.related,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CachedDiagnosticInputs {
    pub local: Vec<WorkspaceDiagnostic>,
    pub embeds: Vec<EmbedRecord>,
}
impl CachedDiagnosticInputs {
    pub(crate) fn new(syntax: &[Diagnostic], output: Option<&DocumentOutput>) -> Self {
        let mut local = syntax.iter().cloned().map(Into::into).collect::<Vec<_>>();
        if let Some(output) = output {
            local.extend(
                local_diagnostics(output)
                    .into_iter()
                    .map(WorkspaceDiagnostic::from),
            );
        }
        Self {
            local,
            embeds: output
                .map(|o| o.embeds().iter().collect())
                .unwrap_or_default(),
        }
    }
}
fn local_diagnostics(output: &DocumentOutput) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    diagnostics.extend(output.headings().diagnostics.clone());
    diagnostics.extend(output.metadata().diagnostics.clone());
    diagnostics.extend(output.citations().diagnostics.iter());
    diagnostics.extend(output.math().diagnostics.iter());
    diagnostics.extend(output.tasks().diagnostics.iter());
    diagnostics.extend(output.events().diagnostics.iter());
    diagnostics.extend(output.diagnostics().iter());
    diagnostics
}

impl Workspace {
    /// Checks cached generations without materializing syntax, respecting open overlays.
    pub fn check_diagnostics_with_context(
        &self,
        path: &Path,
        context: &WorkspaceDiagnosticContext,
    ) -> Result<QueryResult<Vec<WorkspaceDiagnostic>>, WorkspaceQueryError> {
        let path = normalize(path);
        if self.documents.contains_key(&path) {
            return Ok(self.query_result(
                self.diagnostics_with_context(&path, context)?
                    .value
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            ));
        }
        let Some(store) = &self.disk_store else {
            return Ok(self.query_result(Vec::new()));
        };
        let Some(inputs) = store.diagnostic_inputs(&path)? else {
            return Ok(self.query_result(Vec::new()));
        };
        let mut diagnostics = inputs.local;
        let mut cross =
            self.reference_diagnostics(&path, store.links_for_path(&path)?, inputs.embeds)?;
        cross.extend(self.task_workspace_diagnostics(
            &path,
            &store.tasks_for_path(&path)?,
            &context.cycle_members,
        )?);
        cross.extend(self.event_workspace_diagnostics(&path, store.events_for_path(&path)?)?);
        diagnostics.extend(cross.into_iter().map(WorkspaceDiagnostic::from));
        Ok(self.query_result(diagnostics))
    }
    fn reference_diagnostics(
        &self,
        path: &Path,
        links: impl IntoIterator<Item = LinkRecord>,
        embeds: impl IntoIterator<Item = EmbedRecord>,
    ) -> Result<Vec<Diagnostic>, WorkspaceQueryError> {
        let mut diagnostics = Vec::new();
        for link in links {
            let (code, message) = match self.resolve_link_value(&path, &link)? {
                ResolvedTarget::UnresolvedPath { path } => (
                    "link.unresolved-path",
                    format!("unresolved plumb document '{}'", path.display()),
                ),
                ResolvedTarget::UnresolvedAnchor { id, .. } => (
                    "link.unresolved-anchor",
                    format!("unresolved explicit anchor '#{id}'"),
                ),
                ResolvedTarget::AmbiguousAnchor { id, .. } => (
                    "link.ambiguous-anchor",
                    format!("explicit anchor '#{id}' is ambiguous"),
                ),
                ResolvedTarget::UnresolvedFile { path } => (
                    "link.unresolved-file",
                    format!("unresolved file reference '{}'", path.display()),
                ),
                _ => continue,
            };
            diagnostics.push(Diagnostic {
                code,
                severity: DiagnosticSeverity::Warning,
                message,
                range: link.target.range.clone(),
                related: Vec::new(),
            });
        }
        for embed in embeds {
            let ResolvedTarget::UnresolvedFile { path: target } = self.resolve_embed(&path, &embed)
            else {
                continue;
            };
            diagnostics.push(Diagnostic {
                code: "embed.unresolved-file",
                severity: DiagnosticSeverity::Warning,
                message: format!("unresolved embed file '{}'", target.display()),
                range: embed.source.range.clone(),
                related: Vec::new(),
            });
        }
        Ok(diagnostics)
    }
    pub fn diagnostics(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<QueryResult<Vec<Diagnostic>>, WorkspaceQueryError> {
        let context = self.diagnostic_context()?;
        self.diagnostics_with_context(path, &context)
    }

    pub fn diagnostic_context(&self) -> Result<WorkspaceDiagnosticContext, WorkspaceQueryError> {
        let graph = self.task_dependency_graph()?;
        Ok(WorkspaceDiagnosticContext {
            cycle_members: dependency_cycle_members(&graph),
            #[cfg(test)]
            task_dependency_graph: graph,
        })
    }

    pub fn diagnostics_with_context(
        &self,
        path: impl AsRef<Path>,
        context: &WorkspaceDiagnosticContext,
    ) -> Result<QueryResult<Vec<Diagnostic>>, WorkspaceQueryError> {
        let path = normalize(path.as_ref());
        let Some(entry) = self.documents.get(&path) else {
            return Ok(self.query_result(Vec::new()));
        };
        let mut diagnostics = entry.parsed.diagnostics().to_vec();
        let Some(current) = &entry.current else {
            return Ok(self.query_result(diagnostics));
        };
        diagnostics.extend(local_diagnostics(&current.output));
        diagnostics.extend(self.reference_diagnostics(
            &path,
            current.output.links().iter(),
            current.output.embeds().iter(),
        )?);
        diagnostics.extend(self.task_workspace_diagnostics(
            &path,
            &current.output.tasks().tasks.iter().collect::<Vec<_>>(),
            &context.cycle_members,
        )?);
        diagnostics.extend(
            self.event_workspace_diagnostics(&path, current.output.events().events.iter())?,
        );
        Ok(self.query_result(diagnostics))
    }

    fn event_workspace_diagnostics(
        &self,
        path: &Path,
        events: impl IntoIterator<Item = EventRecord>,
    ) -> Result<Vec<Diagnostic>, WorkspaceQueryError> {
        let mut diagnostics = Vec::new();
        for event in events {
            // Inferred links only associate successfully resolved tasks; they cannot add diagnostics.
            if !event.tasks_override {
                continue;
            }
            for reference in &self.event_task_references(path, &event)?.value {
                if let Some(mut diagnostic) = self.task_target_diagnostic(
                    path,
                    &reference.source,
                    &reference.range,
                    &reference.target,
                    "association",
                )? {
                    diagnostic.code = match diagnostic.code {
                        "task.invalid-target" => "event.invalid-task-reference",
                        "task.unresolved-path" => "event.unresolved-task-path",
                        "task.unresolved-anchor" => "event.unresolved-task",
                        "task.ambiguous-anchor" => "event.ambiguous-task",
                        "task.non-task-target" => "event.target-not-task",
                        code => code,
                    };
                    diagnostics.push(diagnostic);
                }
            }
        }
        Ok(diagnostics)
    }

    fn task_workspace_diagnostics(
        &self,
        path: &Path,
        tasks: &[TaskRecord],
        cycle_members: &HashSet<TaskRef>,
    ) -> Result<Vec<Diagnostic>, WorkspaceQueryError> {
        let mut diagnostics = Vec::new();
        for (task_index, task) in tasks.iter().enumerate() {
            let own_ref = TaskRef::from_task(path, &task);
            if let Some(prev) = &task.prev {
                let target = parse_task_reference_target(&prev.value);
                if let Some(diagnostic) =
                    self.task_target_diagnostic(path, &prev.value, &prev.range, &target, "prev")?
                {
                    diagnostics.push(diagnostic);
                }
            }
            let mut blockers = Vec::new();
            for dependency in &task.depends {
                let resolution = self.resolve_task_target(path, &dependency.target)?;
                if let Some(diagnostic) = Self::task_resolution_diagnostic(
                    &resolution,
                    &dependency.source,
                    &dependency.range,
                    "dependency",
                ) {
                    diagnostics.push(diagnostic);
                    continue;
                }
                if let TaskTargetResolution::Task {
                    target,
                    task: target_task,
                } = resolution
                {
                    if own_ref.as_ref() == Some(&target) {
                        diagnostics.push(Diagnostic {
                            code: "task.self-dependency",
                            severity: DiagnosticSeverity::Warning,
                            message: format!(
                                "task depends on itself through '{}'",
                                dependency.source
                            ),
                            range: dependency.range.clone(),
                            related: Vec::new(),
                        });
                    }
                    if task.state() == TaskState::Done && target_task.state() == TaskState::Open {
                        blockers.push(ResolvedTaskDependency {
                            source: dependency.source.clone(),
                            target,
                            task: *target_task,
                        });
                    }
                }
            }
            if let Some(task_ref) = &own_ref {
                if cycle_members.contains(task_ref) {
                    diagnostics.push(Diagnostic {
                        code: "task.dependency-cycle",
                        severity: DiagnosticSeverity::Warning,
                        message: format!(
                            "task '{}' participates in a dependency cycle",
                            task_ref.display(Path::new(""))
                        ),
                        range: task.selection_range.clone(),
                        related: Vec::new(),
                    });
                }
            }
            if task.state() == TaskState::Done {
                blockers.sort_by(|left, right| {
                    left.target
                        .path
                        .cmp(&right.target.path)
                        .then(left.target.id.cmp(&right.target.id))
                });
                let blocker_targets = blockers
                    .iter()
                    .map(|dependency| dependency.target.clone())
                    .collect::<HashSet<_>>();
                if !blockers.is_empty() {
                    diagnostics.push(Diagnostic {
                        code: "task.done-with-open-dependency",
                        severity: DiagnosticSeverity::Warning,
                        message: format!(
                            "completed task still depends on {} open {}",
                            blockers.len(),
                            if blockers.len() == 1 { "task" } else { "tasks" }
                        ),
                        range: task
                            .done
                            .as_ref()
                            .expect("done task has a done field")
                            .range
                            .clone(),
                        related: blockers
                            .iter()
                            .filter(|dependency| dependency.target.path == path)
                            .map(|dependency| dependency.task.selection_range.clone())
                            .collect(),
                    });
                }

                let open_descendants = tasks[task_index + 1..]
                    .iter()
                    .take_while(|descendant| descendant.depth > task.depth)
                    .filter(|descendant| descendant.state() == TaskState::Open)
                    .filter(|descendant| {
                        descendant
                            .id
                            .as_ref()
                            .map(|id| id.value.as_str())
                            .is_none_or(|id| {
                                !blocker_targets.contains(&TaskRef {
                                    path: path.to_path_buf(),
                                    id: Some(id.to_owned()),
                                })
                            })
                    })
                    .collect::<Vec<_>>();
                if !open_descendants.is_empty() {
                    diagnostics.push(Diagnostic {
                        code: "task.done-with-open-descendant",
                        severity: DiagnosticSeverity::Warning,
                        message: format!(
                            "completed task still contains {} open {}",
                            open_descendants.len(),
                            if open_descendants.len() == 1 {
                                "descendant"
                            } else {
                                "descendants"
                            }
                        ),
                        range: task
                            .done
                            .as_ref()
                            .expect("done task has a done field")
                            .range
                            .clone(),
                        related: open_descendants
                            .iter()
                            .map(|descendant| descendant.selection_range.clone())
                            .collect(),
                    });
                }
            }
            if task.state() == TaskState::Open {
                let blockers = self
                    .task_dependencies_value(path, &task)?
                    .into_iter()
                    .filter(|dependency| dependency.task.state() == TaskState::Open)
                    .collect::<Vec<_>>();
                if !blockers.is_empty() {
                    diagnostics.push(Diagnostic {
                        code: "task.blocked",
                        severity: DiagnosticSeverity::Hint,
                        message: format!(
                            "task is blocked by {} open {}",
                            blockers.len(),
                            if blockers.len() == 1 {
                                "dependency"
                            } else {
                                "dependencies"
                            }
                        ),
                        range: task.selection_range.clone(),
                        related: Vec::new(),
                    });
                }
            }
        }
        Ok(diagnostics)
    }

    pub(crate) fn task_target_diagnostic(
        &self,
        from: &Path,
        source: &str,
        range: &std::ops::Range<usize>,
        target: &TaskReferenceTarget,
        role: &str,
    ) -> Result<Option<Diagnostic>, WorkspaceQueryError> {
        Ok(Self::task_resolution_diagnostic(
            &self.resolve_task_target(from, target)?,
            source,
            range,
            role,
        ))
    }

    pub(crate) fn task_resolution_diagnostic(
        resolution: &TaskTargetResolution,
        source: &str,
        range: &std::ops::Range<usize>,
        role: &str,
    ) -> Option<Diagnostic> {
        let (code, message) = match resolution {
            TaskTargetResolution::Task { .. } => return None,
            TaskTargetResolution::Invalid => (
                "task.invalid-target",
                format!("invalid task {role} target '{source}'"),
            ),
            TaskTargetResolution::UnresolvedPath { path } => (
                "task.unresolved-path",
                format!("unresolved task document '{}'", path.display()),
            ),
            TaskTargetResolution::UnresolvedAnchor { id, .. } => (
                "task.unresolved-anchor",
                format!("unresolved task anchor '#{id}'"),
            ),
            TaskTargetResolution::AmbiguousAnchor { id, .. } => (
                "task.ambiguous-anchor",
                format!("task anchor '#{id}' is ambiguous"),
            ),
            TaskTargetResolution::NotDocumentTask { path } => (
                "task.non-task-target",
                format!("document '{}' does not have the task facet", path.display()),
            ),
            TaskTargetResolution::NotTask { id, .. } => (
                "task.non-task-target",
                format!("anchor '#{id}' does not identify a task"),
            ),
        };
        Some(Diagnostic {
            code,
            severity: DiagnosticSeverity::Warning,
            message,
            range: range.clone(),
            related: Vec::new(),
        })
    }
}
