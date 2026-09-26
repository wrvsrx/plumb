//! Configuration and background publication of workspace policy diagnostics.
use super::*;
use plumb_workspace::{AgendaLocation, DiagnosticSettings, WorkspaceConfig};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct PolicyState {
    overrides: Vec<String>,
    settings: BTreeMap<PathBuf, DiagnosticSettings>,
    pub(super) diagnostics: HashMap<PathBuf, Vec<LspDiagnostic>>,
    generation: u64,
    running: bool,
    dirty: bool,
    last_status: Option<String>,
}

pub(crate) struct PolicyDiagnosticsResult {
    generation: u64,
    result: Result<PolicyPublication, String>,
}

struct PolicyPublication {
    diagnostics: HashMap<PathBuf, Vec<LspDiagnostic>>,
}

impl ServerState {
    pub(crate) fn set_config_overrides(&mut self, overrides: Vec<String>) {
        self.policy.overrides = overrides;
    }

    pub(super) fn load_policy_configuration(&mut self) -> Result<(), String> {
        let mut errors = Vec::new();
        self.policy.settings.clear();
        for root in &self.roots {
            match WorkspaceConfig::load(root, &self.policy.overrides) {
                Ok(config) => {
                    self.policy
                        .settings
                        .insert(root.clone(), config.diagnostics);
                }
                Err(error) => errors.push(error),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }

    pub(super) fn is_policy_config(&self, path: &Path) -> bool {
        self.roots
            .iter()
            .any(|root| root.join(".plumb/config.toml") == path)
    }

    pub(super) fn reload_policy_configuration(&mut self) {
        if let Err(error) = self.load_policy_configuration() {
            self.policy_status(
                lsp_types::MessageType::ERROR,
                format!("diagnostics.configuration: {error}"),
            );
        }
        self.schedule_policy_diagnostics();
        self.publish_all_open_diagnostics();
    }

    fn policy_status(&mut self, typ: lsp_types::MessageType, message: String) {
        if self.policy.last_status.as_ref() != Some(&message) {
            let _ = self.client.notify::<lsp_types::notification::ShowMessage>(
                lsp_types::ShowMessageParams {
                    typ,
                    message: message.clone(),
                },
            );
            self.policy.last_status = Some(message);
        }
    }

    fn policy_incomplete(&mut self, message: String) {
        if self.policy.last_status.as_ref() != Some(&message) {
            let _ = self.client.notify::<lsp_types::notification::LogMessage>(
                lsp_types::LogMessageParams {
                    typ: lsp_types::MessageType::WARNING,
                    message: message.clone(),
                },
            );
            self.policy.last_status = Some(message);
        }
    }

    pub(super) fn schedule_policy_diagnostics(&mut self) {
        self.policy.generation = self.policy.generation.wrapping_add(1);
        self.policy.dirty = true;
        let had_results = !self.policy.diagnostics.is_empty();
        self.policy.diagnostics.clear();
        if had_results {
            // Invalidation clears diagnostics in *all* affected open documents immediately.
            self.publish_all_open_diagnostics_reusing_context();
        }
        self.start_policy_diagnostics();
    }

    fn start_policy_diagnostics(&mut self) {
        if self.policy.running || !self.policy.dirty {
            return;
        }
        if !self
            .policy
            .settings
            .values()
            .any(|s| s.event_category.enabled || s.event_timeline.enabled)
        {
            self.policy.dirty = false;
            return;
        }
        if !self.index_complete {
            return;
        }
        if self
            .workspace
            .documents()
            .any(|e| e.parsed.is_valid() && e.current.is_none())
        {
            return;
        }
        self.policy.dirty = false;
        self.policy.running = true;
        let generation = self.policy.generation;
        let workspace = self.workspace.clone();
        let settings = self.policy.settings.clone();
        let roots = self.roots.clone();
        let open = self
            .open_documents
            .values()
            .cloned()
            .collect::<HashSet<_>>();
        let client = self.client.clone();
        tokio::task::spawn_blocking(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                compute(workspace, settings, roots, open)
            }))
            .unwrap_or_else(|_| Err("workspace policy analysis failed".into()));
            let _ = client.emit(PolicyDiagnosticsResult { generation, result });
        });
    }

    pub(crate) fn finish_policy_diagnostics(
        &mut self,
        result: PolicyDiagnosticsResult,
    ) -> ControlFlow<async_lsp::Result<()>> {
        self.policy.running = false;
        if result.generation != self.policy.generation {
            self.start_policy_diagnostics();
            return ControlFlow::Continue(());
        }
        match result.result {
            Ok(publication) => {
                self.policy.diagnostics = publication.diagnostics;
                self.policy.last_status = None;
            }
            Err(error) => self.policy_incomplete(format!("diagnostics.incomplete: {error}")),
        }
        self.publish_all_open_diagnostics_reusing_context();
        ControlFlow::Continue(())
    }
}

fn compute(
    workspace: Workspace,
    settings: BTreeMap<PathBuf, DiagnosticSettings>,
    roots: Vec<PathBuf>,
    open: HashSet<PathBuf>,
) -> Result<PolicyPublication, String> {
    let workspace = workspace.readonly_diagnostic_snapshot()?;
    let now = Local::now().fixed_offset();
    let mut publication = PolicyPublication {
        diagnostics: HashMap::new(),
    };
    let mut positions = HashMap::<PathBuf, PositionIndex<'static>>::new();
    for (root, settings) in settings {
        let excluded = roots
            .iter()
            .filter(|other| **other != root && other.starts_with(&root))
            .cloned()
            .collect::<Vec<_>>();
        let report = workspace.policy_diagnostics(&root, &excluded, now, &settings)?;
        for diagnostic in report.diagnostics {
            // Incomplete input cannot establish a rule's negative conclusions.
            if report
                .incomplete_rules
                .iter()
                .any(|rule| diagnostic.code.starts_with(&format!("{rule}.")))
            {
                continue;
            }
            if !open.contains(&diagnostic.source.path)
                || workspace
                    .get(&diagnostic.source.path)
                    .is_none_or(|e| e.current.is_none())
            {
                continue;
            }
            let range = project_location(&workspace, &mut positions, &diagnostic.source)?.range;
            let related_information = diagnostic
                .related
                .iter()
                .map(|source| {
                    Ok(DiagnosticRelatedInformation {
                        location: project_location(&workspace, &mut positions, source)?,
                        message: "related event".into(),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            publication
                .diagnostics
                .entry(diagnostic.source.path)
                .or_default()
                .push(LspDiagnostic {
                    range,
                    severity: Some(DiagnosticSeverity::WARNING),
                    code: Some(NumberOrString::String(diagnostic.code)),
                    source: Some("plumb".into()),
                    message: diagnostic.message,
                    related_information: (!related_information.is_empty())
                        .then_some(related_information),
                    ..LspDiagnostic::default()
                });
        }
    }
    Ok(publication)
}

fn project_location(
    workspace: &Workspace,
    positions: &mut HashMap<PathBuf, PositionIndex<'static>>,
    source: &AgendaLocation,
) -> Result<Location, String> {
    if !positions.contains_key(&source.path) {
        positions.insert(
            source.path.clone(),
            PositionIndex::from_source(workspace.diagnostic_source(&source.path)?),
        );
    }
    let uri = Url::from_file_path(&source.path).map_err(|_| "invalid diagnostic source path")?;
    Ok(Location {
        uri,
        range: positions[&source.path].byte_range_to_lsp(&(source.range.start..source.range.end)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_index_defers_policy_without_status_notifications() {
        let (_main, client) =
            async_lsp::MainLoop::new_server(|_| async_lsp::router::Router::new(()));
        let mut state = ServerState::new(client);
        let mut settings = DiagnosticSettings::default();
        settings.event_category.enabled = true;
        state
            .policy
            .settings
            .insert(PathBuf::from("/notes"), settings);
        state.schedule_policy_diagnostics();
        assert!(state.policy.dirty);
        assert!(!state.policy.running);
        assert!(state.policy.last_status.is_none());
        state.index_pending = false;
        state.schedule_policy_diagnostics();
        assert!(state.policy.last_status.is_none());
        assert!(!state.policy.running);
    }

    #[test]
    fn incomplete_rule_conclusions_are_deferred_until_syntax_is_repaired() {
        let root = PathBuf::from("/notes");
        let a = root.join("a.plumb");
        let b = root.join("b.plumb");
        let c = root.join("c.plumb");
        let mut workspace = Workspace::new();
        workspace.open_document(&a, 1, "`- 2026-09-22T10:00:00Z--11:00 First\n `+ event\n");
        workspace.open_document(&b, 1, "`broken{");
        workspace.open_document(&c, 1, "`- 2026-09-22T12:00:00Z--13:00 Last\n `+ event\n");
        let mut settings = DiagnosticSettings::default();
        settings.event_timeline.enabled = true;
        let settings = BTreeMap::from([(root.clone(), settings)]);
        let open = HashSet::from([a.clone(), b.clone(), c]);
        let incomplete = compute(
            workspace.clone(),
            settings.clone(),
            vec![root.clone()],
            open.clone(),
        )
        .unwrap();
        assert!(incomplete.diagnostics.is_empty());
        workspace.open_document(&b, 2, "Fixed\n");
        let complete = compute(workspace, settings, vec![root], open).unwrap();
        assert!(complete.diagnostics[&a]
            .iter()
            .any(|d| d.code == Some(NumberOrString::String("event-timeline.gap".into()))));
    }

    #[test]
    fn stale_background_generation_cannot_install_diagnostics_or_error_status() {
        let (_main, client) =
            async_lsp::MainLoop::new_server(|_| async_lsp::router::Router::new(()));
        let mut state = ServerState::new(client);
        state.policy.generation = 2;
        state.policy.running = true;
        let stale = PolicyDiagnosticsResult {
            generation: 1,
            result: Ok(PolicyPublication {
                diagnostics: HashMap::from([(
                    PathBuf::from("/notes/a.plumb"),
                    vec![LspDiagnostic::default()],
                )]),
            }),
        };
        let _ = state.finish_policy_diagnostics(stale);
        assert!(state.policy.diagnostics.is_empty());
        assert!(state.policy.last_status.is_none());
        assert!(!state.policy.running);
        let _ = state.finish_policy_diagnostics(PolicyDiagnosticsResult {
            generation: 1,
            result: Err("old failure".into()),
        });
        assert!(state.policy.last_status.is_none());
    }
}
