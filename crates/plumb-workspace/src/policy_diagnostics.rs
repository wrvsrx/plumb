//! Optional workspace rules shared by batch checking and editor publication.
use crate::{AgendaIssue, AgendaLocation, DiagnosticSettings, TimelineSegment, Workspace};
use chrono::{DateTime, FixedOffset};
use plumb_syntax::DiagnosticSeverity;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct EventPolicyState {
    pub category: crate::CategoryCheckState,
    pub timeline: crate::TimelineCheckState,
}

#[derive(Debug, Clone)]
pub struct PolicyDiagnostic {
    pub code: String,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub source: AgendaLocation,
    pub related: Vec<AgendaLocation>,
}

#[derive(Debug, Default, Clone)]
pub struct PolicyDiagnosticReport {
    pub diagnostics: Vec<PolicyDiagnostic>,
    pub incomplete_rules: Vec<String>,
}

impl PolicyDiagnosticReport {
    pub fn complete(&self) -> bool {
        self.incomplete_rules.is_empty()
    }

    fn issues(&mut self, issues: Vec<AgendaIssue>) {
        self.diagnostics
            .extend(issues.into_iter().map(|issue| PolicyDiagnostic {
                code: issue.code,
                severity: DiagnosticSeverity::Error,
                message: issue.message,
                source: issue.source,
                related: Vec::new(),
            }));
    }

    fn segments(&mut self, kind: &str, segments: Vec<TimelineSegment>) {
        for segment in segments {
            for source in &segment.events {
                self.diagnostics.push(PolicyDiagnostic {
                    code: format!("event-timeline.{kind}"),
                    severity: DiagnosticSeverity::Error,
                    message: format!(
                        "{} -- {}",
                        segment.start.to_rfc3339(),
                        segment.end.to_rfc3339()
                    ),
                    source: source.clone(),
                    related: segment
                        .events
                        .iter()
                        .filter(|other| *other != source)
                        .cloned()
                        .collect(),
                });
            }
        }
    }
}

impl Workspace {
    /// `excluded_roots` gives nested LSP workspaces independent policy scopes.
    pub fn policy_diagnostics(
        &self,
        root: &Path,
        excluded_roots: &[PathBuf],
        now: DateTime<FixedOffset>,
        settings: &DiagnosticSettings,
    ) -> Result<PolicyDiagnosticReport, String> {
        self.policy_diagnostics_with_state(root, excluded_roots, now, settings, None)
    }

    pub fn policy_diagnostics_incremental(
        &self,
        root: &Path,
        excluded_roots: &[PathBuf],
        now: DateTime<FixedOffset>,
        settings: &DiagnosticSettings,
        state: &mut EventPolicyState,
    ) -> Result<PolicyDiagnosticReport, String> {
        self.policy_diagnostics_with_state(root, excluded_roots, now, settings, Some(state))
    }

    fn policy_diagnostics_with_state(
        &self,
        root: &Path,
        excluded_roots: &[PathBuf],
        now: DateTime<FixedOffset>,
        settings: &DiagnosticSettings,
        mut state: Option<&mut EventPolicyState>,
    ) -> Result<PolicyDiagnosticReport, String> {
        let mut report = PolicyDiagnosticReport::default();
        if settings.event_category.enabled {
            let categories = match state.as_deref_mut() {
                Some(state) => self.check_event_categories_incremental_in_scope(
                    root,
                    now,
                    excluded_roots,
                    &mut state.category,
                )?,
                None => self.check_event_categories_in_scope(root, now, None, excluded_roots)?,
            };
            if !categories.complete {
                report.incomplete_rules.push("event-category".into());
            }
            for source in categories.missing {
                report.diagnostics.push(PolicyDiagnostic {
                    code: "event-category.missing".into(),
                    severity: DiagnosticSeverity::Error,
                    message: "event has an uncategorized accounting share".into(),
                    source,
                    related: Vec::new(),
                });
            }
            report.issues(categories.issues);
        }
        if settings.event_timeline.enabled {
            let timeline = match state {
                Some(state) => self.check_event_timeline_incremental_in_scope(
                    root,
                    now,
                    excluded_roots,
                    &mut state.timeline,
                )?,
                None => self.check_event_timeline_in_scope(root, now, excluded_roots)?,
            };
            if !timeline.complete {
                report.incomplete_rules.push("event-timeline".into());
            }
            report.segments("gap", timeline.gaps);
            report.segments("overlap", timeline.overlaps);
            report.issues(timeline.issues);
        }
        report.diagnostics.sort_by(|a, b| {
            (&a.source, &a.code, &a.message).cmp(&(&b.source, &b.code, &b.message))
        });
        report.diagnostics.dedup_by(|a, b| {
            a.source == b.source
                && a.code == b.code
                && a.message == b.message
                && a.related == b.related
        });
        Ok(report)
    }

    /// Detach mutable database state before a background diagnostic round.
    pub fn readonly_diagnostic_snapshot(&self) -> Result<Self, String> {
        Ok(Self {
            documents: self.documents.clone(),
            disk_store: self
                .disk_store
                .as_ref()
                .map(|s| s.readonly_snapshot())
                .transpose()
                .map_err(|e| e.to_string())?,
        })
    }

    /// Source bytes for adapter projection must match the analyzed revision.
    pub fn diagnostic_source(&self, path: &Path) -> Result<String, String> {
        if let Some(entry) = self.get(path) {
            return Ok(entry.parsed.source().to_owned());
        }
        let source =
            std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if self
            .disk_store
            .as_ref()
            .ok_or("missing diagnostic source snapshot")?
            .contains_current(path, &source)
            .map_err(|e| e.to_string())?
        {
            Ok(source)
        } else {
            Err(format!("diagnostic source changed: {}", path.display()))
        }
    }
}
