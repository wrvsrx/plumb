//! Protocol-neutral agenda accounting and exact-coverage queries.
use crate::{
    normalize, parse_task_reference_target, resolve_relative, QueryCompleteness, ResolvedTarget,
    SearchRecordKind, Workspace,
};
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
impl Workspace {
    /// Resolve one event's accounting inputs against the current workspace revision.
    /// Reference failures retain their denominator share and make the result incomplete.
    pub fn event_accounting(
        &self,
        path: &Path,
        event: &EventRecord,
        duration_seconds: f64,
    ) -> Result<(Vec<AgendaShare>, Vec<AgendaIssue>), String> {
        let path = normalize(path);
        let source = location(&path, event.selection_range.clone());
        let mut issues = Vec::new();
        if event.tasks_override && event.tasks.is_empty() {
            issues.push(issue(
                "agenda.invalid-item",
                "explicit tasks declaration is empty",
                source.clone(),
            ));
        }
        let refs = if event.tasks_override {
            event
                .tasks
                .iter()
                .map(|r| (r.target.clone(), r.source.clone(), r.range.clone()))
                .collect::<Vec<_>>()
        } else if event.accounting_links.is_empty() {
            Vec::new()
        } else {
            let links = if self.documents.contains_key(&path) {
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
        };
        let mut seen = BTreeSet::new();
        let mut shares = Vec::new();
        let mut item_categories = Vec::new();
        for (target, spelling, range) in refs {
            let identity = match &target {
                TaskReferenceTarget::Internal { id } => Some(AgendaItem {
                    path: path.clone(),
                    id: Some(id.clone()),
                }),
                TaskReferenceTarget::External {
                    path: target_path,
                    id,
                } => Some(AgendaItem {
                    path: resolve_relative(&path, target_path),
                    id: Some(id.clone()),
                }),
                TaskReferenceTarget::Document { path: target_path } => Some(AgendaItem {
                    path: resolve_relative(&path, target_path),
                    id: None,
                }),
                TaskReferenceTarget::Invalid => None,
            };
            let key = (
                identity.clone(),
                identity.is_none().then_some(spelling.clone()),
            );
            if !seen.insert(key) {
                continue;
            }
            let mut category = Category::default();
            let mut category_source = None;
            let mut is_task = false;
            let mut valid = false;
            let resolved = self
                .resolve_task_reference_target(&path, &target)
                .map_err(|e| e.to_string())?;
            match resolved {
                ResolvedTarget::Anchor {
                    path: target_path,
                    id,
                    anchor,
                } if anchor.list_item => {
                    is_task = self
                        .tasks_for_path(&target_path)
                        .map_err(|e| e.to_string())?
                        .iter()
                        .any(|t| t.id.as_ref().is_some_and(|f| f.value == id));
                    valid = !event.tasks_override || is_task;
                    category = anchor.category;
                    category_source = category
                        .declarations
                        .first()
                        .map(|r| location(&target_path, r.clone()));
                }
                ResolvedTarget::Document { path: target_path } => {
                    is_task = self.tasks_for_path(&target_path).map_err(|e| e.to_string())?
                        .iter().any(|t| t.owner == TaskOwner::Document);
                    valid = !event.tasks_override || is_task;
                    category = if self.documents.contains_key(&target_path) {
                        self.current_output(&target_path).map(|o| o.document_category()).unwrap_or_default()
                    } else if let Some(store) = &self.disk_store {
                        store.document_category(&target_path).map_err(|e| e.to_string())?.unwrap_or_default()
                    } else { Category::default() };
                    category_source = category.declarations.first().map(|r| location(&target_path, r.clone()));
                }
                _ => {}
            }
            if !valid {
                issues.push(issue(
                    "agenda.invalid-item",
                    format!("cannot resolve accounting item '{spelling}'"),
                    location(&path, range),
                ));
            }
            if category.invalid && event.category.declarations.is_empty() {
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
        if event.category.invalid {
            issues.push(issue(
                "agenda.invalid-category",
                "event category must be a nonempty scalar or list of plain categories",
                source.clone(),
            ));
        }
        let mut allocated = Vec::new();
        for (mut share, inherited) in shares.into_iter().zip(item_categories) {
            let categories = if !event.category.declarations.is_empty() {
                share.category_source =
                    Some(location(&path, event.category.declarations[0].clone()));
                if event.category.invalid {
                    Vec::new()
                } else {
                    event.category.values.clone()
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
                "document has no current valid semantic output",
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
                    self.event_accounting(&path, &event, duration)?
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
impl Workspace {
    /// Check selected event categories without imposing a time window.
    pub fn check_event_categories(
        &self,
        root: &Path,
        now: DateTime<FixedOffset>,
        filter: Option<&str>,
        explicit: bool,
    ) -> Result<CategoryCheckReport, String> {
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
        let mut report = CategoryCheckReport {
            complete: selected.completeness == QueryCompleteness::Complete
                && selected.value.complete,
            checked: 0,
            missing: Vec::new(),
            issues: Vec::new(),
        };
        let mut by_path = BTreeMap::<PathBuf, BTreeSet<usize>>::new();
        for record in selected.value.items {
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
            for event in events
                .into_iter()
                .filter(|e| starts.contains(&e.selection_range.start))
            {
                report.checked += 1;
                let source = location(&path, event.selection_range.clone());
                let missing = if explicit {
                    if event.category.invalid {
                        report.issues.push(issue(
                            "agenda.invalid-category",
                            "invalid event category",
                            source.clone(),
                        ));
                    }
                    event.category.values.is_empty() || event.category.invalid
                } else {
                    let (shares, issues) = self.event_accounting(&path, &event, 0.0)?;
                    report.issues.extend(issues);
                    shares.iter().any(|s| s.category.is_none())
                };
                if missing {
                    report.missing.push(source);
                }
            }
        }
        report.complete &= report.issues.is_empty();
        Ok(report)
    }
}
