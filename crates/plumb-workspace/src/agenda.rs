//! Protocol-neutral agenda accounting and exact-coverage queries.
mod category_cache;
mod duration;
mod duration_index;
pub use duration::*;
pub(crate) use duration_index::DurationCache;
pub use duration_index::DurationWork;
mod interval_tree;
mod policy_inputs;
mod timeline_cache;
mod timeline_index;
use crate::{
    normalize, parse_task_reference_target, resolve_relative, QueryCompleteness, ResolvedTarget,
    SearchRecordKind, Workspace,
};
pub use category_cache::CategoryCheckState;
use chrono::{DateTime, FixedOffset};
use plumb_semantics::{Category, EventRecord, TaskOwner, TaskReferenceTarget};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct AgendaLocation {
    pub path: PathBuf,
    pub range: RangeKey,
}
/// Serializable, ordered source byte span.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct RangeKey {
    pub start: usize,
    pub end: usize,
}
impl From<Range<usize>> for RangeKey {
    fn from(r: Range<usize>) -> Self {
        Self {
            start: r.start,
            end: r.end,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct AgendaIssue {
    pub code: String,
    pub message: String,
    pub source: AgendaLocation,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct AgendaItem {
    pub path: PathBuf,
    pub id: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct AgendaShare {
    pub item: Option<AgendaItem>,
    pub is_task: bool,
    pub category: Option<String>,
    pub category_source: Option<AgendaLocation>,
    pub seconds: f64,
}
#[derive(Debug, Clone, Serialize)]
pub struct AgendaAllocation {
    pub source: AgendaLocation,
    pub title: String,
    pub start: DateTime<FixedOffset>,
    pub end: DateTime<FixedOffset>,
    pub seconds: f64,
    pub shares: Vec<AgendaShare>,
}
#[derive(Debug, Clone, Serialize)]
pub struct TimelineSegment {
    pub start: DateTime<FixedOffset>,
    pub end: DateTime<FixedOffset>,
    pub events: Vec<AgendaLocation>,
}
#[derive(Debug, Clone, Serialize)]
pub struct CategoryTotal {
    pub category: Option<String>,
    pub seconds: f64,
}
#[derive(Debug, Clone, Serialize)]
pub struct ItemTotal {
    pub item: AgendaItem,
    pub seconds: f64,
}
#[derive(Debug, Clone, Serialize)]
pub struct AgendaReport {
    pub from: DateTime<FixedOffset>,
    pub to: DateTime<FixedOffset>,
    pub filter: Option<String>,
    pub complete: bool,
    pub accumulated_seconds: f64,
    pub covered_seconds: f64,
    pub categories: Vec<CategoryTotal>,
    pub items: Vec<ItemTotal>,
    pub tasks: Vec<ItemTotal>,
    pub allocations: Vec<AgendaAllocation>,
    pub points: Vec<AgendaLocation>,
    pub gaps: Vec<TimelineSegment>,
    pub overlaps: Vec<TimelineSegment>,
    pub issues: Vec<AgendaIssue>,
}
impl AgendaReport {
    pub fn timeline_passed(&self) -> bool {
        self.complete && self.gaps.is_empty() && self.overlaps.is_empty()
    }
}
fn seconds(start: DateTime<FixedOffset>, end: DateTime<FixedOffset>) -> f64 {
    (end - start)
        .to_std()
        .expect("ordered interval")
        .as_secs_f64()
}
fn location(path: &Path, range: Range<usize>) -> AgendaLocation {
    AgendaLocation {
        path: path.to_path_buf(),
        range: range.into(),
    }
}
fn issue(code: &str, message: impl Into<String>, source: AgendaLocation) -> AgendaIssue {
    AgendaIssue {
        code: code.into(),
        message: message.into(),
        source,
    }
}
#[derive(Default)]
struct AccountingContext {
    categories: BTreeMap<PathBuf, Category>,
    targets: BTreeMap<AgendaItem, ResolvedTarget>,
    tasks: BTreeMap<PathBuf, BTreeSet<Option<String>>>,
}

#[derive(Clone)]
struct AccountingTarget {
    valid: bool,
    is_task: bool,
    category: Category,
    path: PathBuf,
}
struct AccountingEvent {
    category: Category,
    selection_range: Range<usize>,
    tasks_override: bool,
    references: Vec<(TaskReferenceTarget, String, Range<usize>)>,
}
trait AccountingReader {
    fn document_category(&mut self, workspace: &Workspace, path: &Path)
        -> Result<Category, String>;
    fn target(
        &mut self,
        workspace: &Workspace,
        from: &Path,
        target: &TaskReferenceTarget,
    ) -> Result<AccountingTarget, String>;
}
impl AccountingReader for AccountingContext {
    fn document_category(
        &mut self,
        workspace: &Workspace,
        path: &Path,
    ) -> Result<Category, String> {
        AccountingContext::document_category(self, workspace, path)
    }
    fn target(
        &mut self,
        workspace: &Workspace,
        from: &Path,
        target: &TaskReferenceTarget,
    ) -> Result<AccountingTarget, String> {
        let identity = accounting_identity(from, target);
        let resolved = if let Some(cached) = identity.as_ref().and_then(|key| self.targets.get(key))
        {
            cached.clone()
        } else {
            let resolved = workspace
                .resolve_task_reference_target(from, target)
                .map_err(|e| e.to_string())?;
            if let Some(identity) = identity {
                self.targets.insert(identity, resolved.clone());
            }
            resolved
        };
        let mut result = AccountingTarget {
            valid: false,
            is_task: false,
            category: Category::default(),
            path: from.to_owned(),
        };
        match resolved {
            ResolvedTarget::Anchor { path, id, anchor } if anchor.list_item => {
                result.valid = true;
                result.is_task = self.is_task(workspace, &path, Some(&id))?;
                result.category = if anchor.category.declarations.is_empty() {
                    self.document_category(workspace, &path)?
                } else {
                    anchor.category
                };
                result.path = path;
            }
            ResolvedTarget::Document { path } => {
                result.valid = true;
                result.is_task = self.is_task(workspace, &path, None)?;
                result.category = self.document_category(workspace, &path)?;
                result.path = path;
            }
            _ => {}
        }
        Ok(result)
    }
}
fn accounting_identity(from: &Path, target: &TaskReferenceTarget) -> Option<AgendaItem> {
    match target {
        TaskReferenceTarget::Internal { id } => Some(AgendaItem {
            path: from.to_owned(),
            id: Some(id.clone()),
        }),
        TaskReferenceTarget::External { path, id } => Some(AgendaItem {
            path: resolve_relative(from, path),
            id: Some(id.clone()),
        }),
        TaskReferenceTarget::Document { path } => Some(AgendaItem {
            path: resolve_relative(from, path),
            id: None,
        }),
        TaskReferenceTarget::Invalid => None,
    }
}

impl AccountingContext {
    fn document_category(
        &mut self,
        workspace: &Workspace,
        path: &Path,
    ) -> Result<Category, String> {
        if let Some(category) = self.categories.get(path) {
            return Ok(category.clone());
        }
        let category = workspace.agenda_document_category(path)?;
        self.categories.insert(path.to_owned(), category.clone());
        Ok(category)
    }

    fn is_task(
        &mut self,
        workspace: &Workspace,
        path: &Path,
        id: Option<&str>,
    ) -> Result<bool, String> {
        if !self.tasks.contains_key(path) {
            let tasks = workspace.tasks_for_path(path).map_err(|e| e.to_string())?;
            let identities = tasks
                .into_iter()
                .filter_map(|task| {
                    if task.owner == TaskOwner::Document {
                        Some(None)
                    } else {
                        task.id.map(|field| Some(field.value))
                    }
                })
                .collect();
            self.tasks.insert(path.to_owned(), identities);
        }
        Ok(self.tasks[path].contains(&id.map(str::to_owned)))
    }
}

impl Workspace {
    fn agenda_document_category(&self, path: &Path) -> Result<Category, String> {
        if self.documents.contains_key(path) {
            Ok(self
                .current_output(path)
                .map(|o| o.document_category())
                .unwrap_or_default())
        } else if let Some(store) = &self.disk_store {
            store
                .document_category(path)
                .map(|c| c.unwrap_or_default())
                .map_err(|e| e.to_string())
        } else {
            Ok(Category::default())
        }
    }

    /// Resolve one event's accounting inputs against the current workspace revision.
    /// Reference failures retain their denominator share and make the result incomplete.
    pub fn event_accounting(
        &self,
        path: &Path,
        event: &EventRecord,
        duration_seconds: f64,
    ) -> Result<(Vec<AgendaShare>, Vec<AgendaIssue>), String> {
        self.event_accounting_with_context(
            path,
            event,
            duration_seconds,
            &mut AccountingContext::default(),
        )
    }

    fn accounting_references(
        &self,
        path: &Path,
        event: &EventRecord,
    ) -> Result<Vec<(TaskReferenceTarget, String, Range<usize>)>, String> {
        Ok(if event.tasks_override {
            event
                .tasks
                .iter()
                .map(|r| (r.target.clone(), r.source.clone(), r.range.clone()))
                .collect::<Vec<_>>()
        } else if event.accounting_links.is_empty() {
            Vec::new()
        } else {
            let links = if self.documents.contains_key(path) {
                self.current_output(&path)
                    .map(|o| o.links_contained_by_record(event))
                    .unwrap_or_default()
            } else if let Some(store) = &self.disk_store {
                store
                    .links_in_range(&path, &event.selection_range)
                    .map_err(|e| e.to_string())?
            } else {
                Vec::new()
            };
            event
                .accounting_links
                .iter()
                .map(|range| match links.iter().find(|l| l.range == *range) {
                    Some(link) => (
                        parse_task_reference_target(&link.target.value),
                        link.target.value.clone(),
                        link.target.range.clone(),
                    ),
                    None => (
                        TaskReferenceTarget::Invalid,
                        format!("invalid link at {}", range.start),
                        range.clone(),
                    ),
                })
                .collect()
        })
    }

    fn event_accounting_with_context(
        &self,
        path: &Path,
        event: &EventRecord,
        duration_seconds: f64,
        context: &mut impl AccountingReader,
    ) -> Result<(Vec<AgendaShare>, Vec<AgendaIssue>), String> {
        let input = AccountingEvent {
            category: event.category.clone(),
            selection_range: event.selection_range.clone(),
            tasks_override: event.tasks_override,
            references: self.accounting_references(path, event)?,
        };
        self.allocate_event(path, input, duration_seconds, context)
    }

    fn allocate_event(
        &self,
        path: &Path,
        event: AccountingEvent,
        duration_seconds: f64,
        context: &mut impl AccountingReader,
    ) -> Result<(Vec<AgendaShare>, Vec<AgendaIssue>), String> {
        let path = normalize(path);
        let source = location(&path, event.selection_range.clone());
        let event_category = if event.category.declarations.is_empty() {
            context.document_category(self, &path)?
        } else {
            event.category.clone()
        };
        let mut issues = Vec::new();
        if event.tasks_override && event.references.is_empty() {
            issues.push(issue(
                "agenda.invalid-item",
                "explicit tasks declaration is empty",
                source.clone(),
            ));
        }
        let refs = event.references;
        let mut seen = BTreeSet::new();
        let mut shares = Vec::new();
        let mut item_categories = Vec::new();
        for (target, spelling, range) in refs {
            let identity = accounting_identity(&path, &target);
            let key = (
                identity.clone(),
                identity.is_none().then_some(spelling.clone()),
            );
            if !seen.insert(key) {
                continue;
            }
            let resolved = context.target(self, &path, &target)?;
            let is_task = resolved.is_task;
            let valid = resolved.valid && (!event.tasks_override || is_task);
            let category = resolved.category;
            let category_source = category
                .declarations
                .first()
                .map(|r| location(&resolved.path, r.clone()));
            if !valid {
                issues.push(issue(
                    "agenda.invalid-item",
                    format!("cannot resolve accounting item '{spelling}'"),
                    location(&path, range),
                ));
            }
            if category.invalid && event_category.declarations.is_empty() {
                issues.push(issue(
                    "agenda.invalid-category",
                    "item category must be a nonempty scalar or list of plain categories",
                    category_source.clone().unwrap_or_else(|| source.clone()),
                ));
            }
            item_categories.push(if valid && !category.invalid {
                category.values
            } else {
                Vec::new()
            });
            shares.push(AgendaShare {
                item: identity,
                is_task,
                category: None,
                category_source,
                seconds: 0.0,
            });
        }
        if shares.is_empty() {
            item_categories.push(Vec::new());
            shares.push(AgendaShare {
                item: None,
                is_task: false,
                category: None,
                category_source: None,
                seconds: 0.0,
            });
        }
        let divided = duration_seconds / shares.len() as f64;
        if event_category.invalid {
            issues.push(issue(
                "agenda.invalid-category",
                "event category must be a nonempty scalar or list of plain categories",
                event_category
                    .declarations
                    .first()
                    .map(|range| location(&path, range.clone()))
                    .unwrap_or_else(|| source.clone()),
            ));
        }
        let mut allocated = Vec::new();
        for (mut share, inherited) in shares.into_iter().zip(item_categories) {
            let categories = if !event_category.declarations.is_empty() {
                share.category_source =
                    Some(location(&path, event_category.declarations[0].clone()));
                if event_category.invalid {
                    Vec::new()
                } else {
                    event_category.values.clone()
                }
            } else {
                inherited
            };
            share.seconds = divided / categories.len().max(1) as f64;
            if categories.is_empty() {
                allocated.push(share);
            } else {
                for category in categories {
                    let mut part = share.clone();
                    part.category = Some(category);
                    allocated.push(part);
                }
            }
        }
        let shares = allocated;
        Ok((shares, issues))
    }

    /// `accounting=false` checks original intervals independently of category/reference issues.
    pub fn agenda_report(
        &self,
        root: &Path,
        from: DateTime<FixedOffset>,
        to: DateTime<FixedOffset>,
        now: DateTime<FixedOffset>,
        filter: Option<&str>,
        accounting: bool,
    ) -> Result<AgendaReport, String> {
        if to <= from {
            return Err("--to must be later than --from".into());
        }
        let selected = self
            .search_records_filtered(
                root,
                Some(SearchRecordKind::Event),
                "",
                usize::MAX,
                now,
                filter,
            )
            .map_err(|e| e.to_string())?;
        let mut report = AgendaReport {
            from,
            to,
            filter: filter.map(str::to_owned),
            complete: selected.completeness == QueryCompleteness::Complete
                && selected.value.complete,
            accumulated_seconds: 0.0,
            covered_seconds: 0.0,
            categories: Vec::new(),
            items: Vec::new(),
            tasks: Vec::new(),
            allocations: Vec::new(),
            points: Vec::new(),
            gaps: Vec::new(),
            overlaps: Vec::new(),
            issues: Vec::new(),
        };
        // Invalid documents cannot be proven irrelevant to the requested window or filter.
        for entry in self.documents.values().filter(|e| e.current.is_none()) {
            report.issues.push(issue(
                "agenda.invalid-document",
                "document has no valid semantic output",
                location(&entry.path, 0..0),
            ));
        }
        if let Some(store) = &self.disk_store {
            for doc in store.documents().map_err(|e| e.to_string())? {
                if !doc.valid && !self.documents.contains_key(&doc.path) {
                    report.issues.push(issue(
                        "agenda.invalid-document",
                        "document has no valid semantic output",
                        location(&doc.path, 0..0),
                    ));
                }
            }
        }
        let mut by_path = BTreeMap::<PathBuf, BTreeSet<usize>>::new();
        for record in selected.value.items {
            by_path
                .entry(record.path)
                .or_default()
                .insert(record.range.start);
        }
        let mut boundaries = BTreeMap::<DateTime<FixedOffset>, (Vec<usize>, Vec<usize>)>::new();
        boundaries.entry(from).or_default();
        boundaries.entry(to).or_default();
        let mut accounting_context = AccountingContext::default();
        for (path, starts) in by_path {
            let events = if self.documents.contains_key(&path) {
                self.current_output(&path)
                    .map(|o| o.events().events.iter().collect::<Vec<_>>())
                    .unwrap_or_default()
            } else if let Some(store) = &self.disk_store {
                store.events_for_path(&path).map_err(|e| e.to_string())?
            } else {
                Vec::new()
            };
            for event in events
                .into_iter()
                .filter(|e| starts.contains(&e.selection_range.start))
            {
                let source = location(&path, event.selection_range.clone());
                if let Some(at) = event.at_datetime() {
                    if at >= from && at < to {
                        report.points.push(source);
                    }
                    continue;
                }
                if event.is_running() {
                    continue;
                }
                let (Some(start), Some(end)) = (event.start_datetime(), event.end_datetime())
                else {
                    report.issues.push(issue(
                        "agenda.invalid-time",
                        "event has no valid finite interval",
                        source,
                    ));
                    continue;
                };
                if end <= start {
                    report.issues.push(issue(
                        "agenda.invalid-time",
                        "event end must follow start",
                        source,
                    ));
                    continue;
                }
                if start >= to || end <= from {
                    continue;
                }
                let start = start.max(from);
                let end = end.min(to);
                let duration = seconds(start, end);
                let (shares, issues) = if accounting {
                    self.event_accounting_with_context(
                        &path,
                        &event,
                        duration,
                        &mut accounting_context,
                    )?
                } else {
                    (Vec::new(), Vec::new())
                };
                report.issues.extend(issues);
                let index = report.allocations.len();
                boundaries.entry(start).or_default().0.push(index);
                boundaries.entry(end).or_default().1.push(index);
                report.accumulated_seconds += duration;
                report.allocations.push(AgendaAllocation {
                    source,
                    title: event.title,
                    start,
                    end,
                    seconds: duration,
                    shares,
                });
            }
        }
        let mut active = BTreeSet::new();
        let mut previous = from;
        for (instant, (starting, ending)) in boundaries {
            if instant > previous {
                let segment = TimelineSegment {
                    start: previous,
                    end: instant,
                    events: active
                        .iter()
                        .map(|&i: &usize| report.allocations[i].source.clone())
                        .collect(),
                };
                if active.is_empty() {
                    report.gaps.push(segment);
                } else {
                    report.covered_seconds += seconds(previous, instant);
                    if active.len() > 1 {
                        report.overlaps.push(segment);
                    }
                }
            }
            for index in ending {
                active.remove(&index);
            }
            active.extend(starting);
            previous = instant;
        }
        let mut categories = BTreeMap::new();
        let mut items = BTreeMap::new();
        let mut tasks = BTreeMap::new();
        for allocation in &report.allocations {
            for share in &allocation.shares {
                *categories.entry(share.category.clone()).or_insert(0.0) += share.seconds;
                if let Some(item) = &share.item {
                    *items.entry(item.clone()).or_insert(0.0) += share.seconds;
                    if share.is_task {
                        *tasks.entry(item.clone()).or_insert(0.0) += share.seconds;
                    }
                }
            }
        }
        report.categories = categories
            .into_iter()
            .map(|(category, seconds)| CategoryTotal { category, seconds })
            .collect();
        report.items = items
            .into_iter()
            .map(|(item, seconds)| ItemTotal { item, seconds })
            .collect();
        report.tasks = tasks
            .into_iter()
            .map(|(item, seconds)| ItemTotal { item, seconds })
            .collect();
        report.complete &= report.issues.is_empty();
        Ok(report)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CategoryCheckReport {
    pub complete: bool,
    pub checked: usize,
    pub missing: Vec<AgendaLocation>,
    pub issues: Vec<AgendaIssue>,
}
#[derive(Debug, Clone, Serialize)]
pub struct TimelineCheckReport {
    pub complete: bool,
    /// All documents are available; invalid event times alone do not prevent publication.
    pub conclusions_available: bool,
    pub checked: usize,
    pub gaps: Vec<TimelineSegment>,
    pub overlaps: Vec<TimelineSegment>,
    pub issues: Vec<AgendaIssue>,
}

pub use timeline_cache::TimelineCheckState;
impl TimelineCheckReport {
    pub fn passed(&self) -> bool {
        self.complete && self.gaps.is_empty() && self.overlaps.is_empty()
    }
}

struct SelectedEvents {
    complete: bool,
    events: Vec<(PathBuf, EventRecord)>,
    issues: Vec<AgendaIssue>,
}

impl Workspace {
    fn selected_events_in_scope(
        &self,
        in_scope: impl Fn(&Path) -> bool,
    ) -> Result<SelectedEvents, String> {
        // Policy checks have no search expression or ranking. Read typed
        // event facts directly rather than constructing all search results
        // and then loading the same event records again.
        let mut paths = BTreeSet::new();
        paths.extend(self.documents.keys().filter(|p| in_scope(p)).cloned());
        let stored = self
            .disk_store
            .as_ref()
            .map(|store| store.documents())
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        let validity = stored
            .into_iter()
            .filter(|doc| in_scope(&doc.path))
            .map(|doc| (doc.path, doc.valid))
            .collect::<BTreeMap<_, _>>();
        paths.extend(validity.keys().cloned());
        let mut result = SelectedEvents {
            complete: true,
            events: Vec::new(),
            issues: Vec::new(),
        };
        for path in paths {
            let events = if let Some(entry) = self.documents.get(&path) {
                entry
                    .current
                    .as_ref()
                    .map(|current| current.output.events().events.iter().collect::<Vec<_>>())
            } else if validity.get(&path) == Some(&true) {
                Some(
                    self.disk_store
                        .as_ref()
                        .expect("stored path has a store")
                        .events_for_path(&path)
                        .map_err(|e| e.to_string())?,
                )
            } else {
                None
            };
            if let Some(events) = events {
                result
                    .events
                    .extend(events.into_iter().map(|event| (path.clone(), event)));
            } else {
                result.issues.push(issue(
                    "agenda.invalid-document",
                    "document has no valid semantic output",
                    location(&path, 0..0),
                ));
            }
        }
        result.complete = result.issues.is_empty();
        Ok(result)
    }

    fn selected_check_events(
        &self,
        root: &Path,
        now: DateTime<FixedOffset>,
        filter: Option<&str>,
        excluded_roots: &[PathBuf],
    ) -> Result<SelectedEvents, String> {
        let root = normalize(root);
        let in_scope = |path: &Path| {
            path.starts_with(&root) && !excluded_roots.iter().any(|r| path.starts_with(r))
        };
        if filter.is_none() {
            return self.selected_events_in_scope(in_scope);
        }
        let selected = self
            .search_records_filtered(
                &root,
                Some(SearchRecordKind::Event),
                "",
                usize::MAX,
                now,
                filter,
            )
            .map_err(|e| e.to_string())?;
        // Search completeness covers every indexed root. Recompute it for this policy scope,
        // so an invalid document in a different workspace does not poison this round.
        let mut result = SelectedEvents {
            complete: true,
            events: Vec::new(),
            issues: Vec::new(),
        };
        for entry in self
            .documents
            .values()
            .filter(|e| e.current.is_none() && in_scope(&e.path))
        {
            result.issues.push(issue(
                "agenda.invalid-document",
                "document has no valid semantic output",
                location(&entry.path, 0..0),
            ));
        }
        if let Some(store) = &self.disk_store {
            for doc in store.documents().map_err(|e| e.to_string())? {
                if in_scope(&doc.path) && !doc.valid && !self.documents.contains_key(&doc.path) {
                    result.issues.push(issue(
                        "agenda.invalid-document",
                        "document has no valid semantic output",
                        location(&doc.path, 0..0),
                    ));
                }
            }
        }
        result.issues.sort_by(|a, b| a.source.cmp(&b.source));
        let mut by_path = BTreeMap::<PathBuf, BTreeSet<usize>>::new();
        for record in selected
            .value
            .items
            .into_iter()
            .filter(|r| in_scope(&r.path))
        {
            by_path
                .entry(record.path)
                .or_default()
                .insert(record.range.start);
        }
        for (path, starts) in by_path {
            let events = if self.documents.contains_key(&path) {
                self.current_output(&path)
                    .map(|o| o.events().events.iter().collect::<Vec<_>>())
                    .unwrap_or_default()
            } else if let Some(store) = &self.disk_store {
                store.events_for_path(&path).map_err(|e| e.to_string())?
            } else {
                Vec::new()
            };
            result.events.extend(
                events
                    .into_iter()
                    .filter(|e| starts.contains(&e.selection_range.start))
                    .map(|event| (path.clone(), event)),
            );
        }
        result.complete &= result.issues.is_empty();
        Ok(result)
    }

    /// Check effective categories of every share, including inherited categories and points.
    pub fn check_event_categories(
        &self,
        root: &Path,
        now: DateTime<FixedOffset>,
        filter: Option<&str>,
    ) -> Result<CategoryCheckReport, String> {
        self.check_event_categories_in_scope(root, now, filter, &[])
    }

    pub(crate) fn check_event_categories_in_scope(
        &self,
        root: &Path,
        now: DateTime<FixedOffset>,
        filter: Option<&str>,
        excluded_roots: &[PathBuf],
    ) -> Result<CategoryCheckReport, String> {
        let selected = self.selected_check_events(root, now, filter, excluded_roots)?;
        let mut report = CategoryCheckReport {
            complete: selected.complete,
            checked: selected.events.len(),
            missing: Vec::new(),
            issues: selected.issues,
        };
        let mut context = AccountingContext::default();
        for (path, event) in selected.events {
            let (shares, issues) =
                self.event_accounting_with_context(&path, &event, 0.0, &mut context)?;
            report.issues.extend(issues);
            if shares.iter().any(|s| s.category.is_none()) {
                report
                    .missing
                    .push(location(&path, event.selection_range.clone()));
            }
        }
        report.complete &= report.issues.is_empty();
        Ok(report)
    }

    /// Check continuity across all workspace intervals, without an external time window.
    /// Points do not contribute boundaries; categories and references do not affect coverage.
    pub fn check_event_timeline(
        &self,
        root: &Path,
        now: DateTime<FixedOffset>,
    ) -> Result<TimelineCheckReport, String> {
        self.check_event_timeline_in_scope(root, now, &[])
    }

    pub(crate) fn check_event_timeline_in_scope(
        &self,
        root: &Path,
        now: DateTime<FixedOffset>,
        excluded_roots: &[PathBuf],
    ) -> Result<TimelineCheckReport, String> {
        let selected = self.selected_check_events(root, now, None, excluded_roots)?;
        let mut report = TimelineCheckReport {
            complete: selected.complete,
            conclusions_available: selected.complete,
            checked: selected.events.len(),
            gaps: Vec::new(),
            overlaps: Vec::new(),
            issues: selected.issues,
        };
        let mut sources = Vec::new();
        let mut boundaries = BTreeMap::<DateTime<FixedOffset>, (Vec<usize>, Vec<usize>)>::new();
        for (path, event) in selected.events {
            if event.at_datetime().is_some() || event.is_running() {
                continue;
            }
            let source = location(&path, event.selection_range.clone());
            let (Some(start), Some(end)) = (event.start_datetime(), event.end_datetime()) else {
                report.issues.push(issue(
                    "agenda.invalid-time",
                    "event has no valid finite interval",
                    source,
                ));
                continue;
            };
            if end <= start {
                report.issues.push(issue(
                    "agenda.invalid-time",
                    "event end must follow start",
                    source,
                ));
                continue;
            }
            let index = sources.len();
            sources.push(source);
            boundaries.entry(start).or_default().0.push(index);
            boundaries.entry(end).or_default().1.push(index);
        }
        let mut active = BTreeSet::new();
        let mut previous = None;
        let mut previous_ending = Vec::new();
        for (instant, (starting, ending)) in boundaries {
            if let Some(start) = previous {
                if active.is_empty() {
                    // An internal gap is bounded by at least one ending and one starting event.
                    let events = previous_ending
                        .iter()
                        .chain(&starting)
                        .map(|&i: &usize| sources[i].clone())
                        .collect();
                    report.gaps.push(TimelineSegment {
                        start,
                        end: instant,
                        events,
                    });
                } else if active.len() > 1 {
                    let events = active.iter().map(|&i: &usize| sources[i].clone()).collect();
                    report.overlaps.push(TimelineSegment {
                        start,
                        end: instant,
                        events,
                    });
                }
            }
            for index in &ending {
                active.remove(index);
            }
            active.extend(starting);
            previous = Some(instant);
            previous_ending = ending;
        }
        report.complete &= report.issues.is_empty();
        Ok(report)
    }

    pub fn check_event_timeline_incremental(
        &self,
        root: &Path,
        now: DateTime<FixedOffset>,
        state: &mut TimelineCheckState,
    ) -> Result<TimelineCheckReport, String> {
        self.check_event_timeline_incremental_in_scope(root, now, &[], state)
    }

    pub(crate) fn check_event_timeline_incremental_in_scope(
        &self,
        root: &Path,
        now: DateTime<FixedOffset>,
        excluded_roots: &[PathBuf],
        state: &mut TimelineCheckState,
    ) -> Result<TimelineCheckReport, String> {
        let _ = now;
        let result = self.check_timeline_graph(root, excluded_roots, state);
        if result.is_err() {
            *state = TimelineCheckState::default();
        }
        result
    }
}
