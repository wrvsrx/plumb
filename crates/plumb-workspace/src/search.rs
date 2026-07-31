use std::path::PathBuf;

use chrono::{DateTime, FixedOffset};

use crate::{
    derive_task_workflow_state, normalize, note_search_title, search_kind_order, ReverseReferences,
    SemanticSearchFilter, TaskMatchFacts, Workspace,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchRecordKind {
    Note,
    Task,
    Event,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskWorkflowState {
    Ready,
    Waiting,
    Done,
    Canceled,
    Invalid,
}

impl TaskWorkflowState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Waiting => "waiting",
            Self::Done => "done",
            Self::Canceled => "canceled",
            Self::Invalid => "invalid",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskWaitReason {
    Time,
    Dependency,
}

impl TaskWaitReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Time => "time",
            Self::Dependency => "dependency",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRecord {
    pub kind: SearchRecordKind,
    pub title: String,
    pub path: PathBuf,
    pub relative_path: String,
    pub range: std::ops::Range<usize>,
    pub revision: i64,
    pub id: Option<String>,
    pub task_state: Option<TaskWorkflowState>,
    pub wait_reasons: Option<Vec<TaskWaitReason>>,
    pub due: Option<String>,
    pub priority: Option<i32>,
    pub blocked: Option<bool>,
    pub actionable: Option<bool>,
    pub depth: Option<usize>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub tasks: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResults {
    pub items: Vec<SearchRecord>,
    pub complete: bool,
}

impl Workspace {
    pub fn search_records(
        &self,
        root: impl AsRef<std::path::Path>,
        kind: Option<SearchRecordKind>,
        query: &str,
        limit: usize,
        now: DateTime<FixedOffset>,
    ) -> SearchResults {
        self.search_records_filtered(root, kind, query, limit, now, None)
            .expect("search without a semantic filter cannot fail")
    }

    pub fn search_records_filtered(
        &self,
        root: impl AsRef<std::path::Path>,
        kind: Option<SearchRecordKind>,
        query: &str,
        limit: usize,
        now: DateTime<FixedOffset>,
        filter: Option<&str>,
    ) -> Result<SearchResults, String> {
        let root = normalize(root.as_ref());
        let filter = filter
            .map(|source| SemanticSearchFilter::compile(source, now))
            .transpose()?;
        let reverse = filter.as_ref().map(|_| ReverseReferences::build(self));
        let mut matches = Vec::new();
        for entry in self.documents.values() {
            let Some(current) = &entry.current else {
                continue;
            };
            let relative_path = entry
                .path
                .strip_prefix(&root)
                .unwrap_or(&entry.path)
                .display()
                .to_string();
            if kind.is_none_or(|kind| kind == SearchRecordKind::Note) {
                let (title, range) = note_search_title(current, &relative_path);
                let filter_match = match (&filter, &reverse) {
                    (Some(filter), Some(reverse)) => {
                        filter.note_matches(&root, entry, &title, reverse)?
                    }
                    _ => true,
                };
                if filter_match {
                    if let Some(score) = search_score(query, &[&title, &relative_path]) {
                        matches.push((
                            score,
                            SearchRecord {
                                kind: SearchRecordKind::Note,
                                title,
                                path: entry.path.clone(),
                                relative_path: relative_path.clone(),
                                range,
                                revision: current.revision,
                                id: None,
                                task_state: None,
                                wait_reasons: None,
                                due: None,
                                priority: None,
                                blocked: None,
                                actionable: None,
                                depth: None,
                                start: None,
                                end: None,
                                tasks: None,
                            },
                        ));
                    }
                }
            }
            if kind.is_none_or(|kind| kind == SearchRecordKind::Task) {
                for task in &current.output.tasks.tasks {
                    let id = task.id.as_ref().map(|id| id.value.clone());
                    let fields = [
                        task.title.as_str(),
                        id.as_deref().unwrap_or_default(),
                        relative_path.as_str(),
                    ];
                    let Some(score) = search_score(query, &fields) else {
                        continue;
                    };
                    let blocked = self.is_task_blocked(&entry.path, task);
                    let (task_state, wait_reasons) = derive_task_workflow_state(task, blocked, now);
                    let actionable = task_state == TaskWorkflowState::Ready;
                    if let Some(filter) = &filter {
                        let facts = TaskMatchFacts {
                            state: task_state,
                            wait_reasons: &wait_reasons,
                            blocked,
                            actionable,
                        };
                        if !filter.task_matches(&root, entry, task, self, facts)? {
                            continue;
                        }
                    }
                    matches.push((
                        score,
                        SearchRecord {
                            kind: SearchRecordKind::Task,
                            title: task.title.clone(),
                            path: entry.path.clone(),
                            relative_path: relative_path.clone(),
                            range: task.selection_range.clone(),
                            revision: current.revision,
                            id,
                            task_state: Some(task_state),
                            wait_reasons: Some(wait_reasons),
                            due: task.due.as_ref().map(|due| due.value.clone()),
                            priority: task.priority,
                            blocked: Some(blocked),
                            actionable: Some(actionable),
                            depth: Some(task.depth),
                            start: None,
                            end: None,
                            tasks: None,
                        },
                    ));
                }
            }
            if kind.is_none_or(|kind| kind == SearchRecordKind::Event) {
                for event in &current.output.events.events {
                    let id = event.id.as_ref().map(|id| id.value.clone());
                    let fields = [
                        event.title.as_str(),
                        id.as_deref().unwrap_or_default(),
                        event
                            .uid
                            .as_ref()
                            .map(|uid| uid.value.as_str())
                            .unwrap_or_default(),
                        relative_path.as_str(),
                    ];
                    let Some(score) = search_score(query, &fields) else {
                        continue;
                    };
                    if let Some(filter) = &filter {
                        if !filter.event_matches(&root, entry, event)? {
                            continue;
                        }
                    }
                    matches.push((
                        score,
                        SearchRecord {
                            kind: SearchRecordKind::Event,
                            title: event.title.clone(),
                            path: entry.path.clone(),
                            relative_path: relative_path.clone(),
                            range: event.selection_range.clone(),
                            revision: current.revision,
                            id,
                            task_state: None,
                            wait_reasons: None,
                            due: None,
                            priority: None,
                            blocked: None,
                            actionable: None,
                            depth: Some(event.depth),
                            start: event.start.as_ref().map(|field| field.value.clone()),
                            end: event.end.as_ref().map(|field| field.value.clone()),
                            tasks: Some(
                                event
                                    .tasks
                                    .iter()
                                    .map(|reference| reference.source.clone())
                                    .collect(),
                            ),
                        },
                    ));
                }
            }
        }
        matches.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| left.relative_path.cmp(&right.relative_path))
                .then_with(|| left.range.start.cmp(&right.range.start))
                .then_with(|| search_kind_order(left.kind).cmp(&search_kind_order(right.kind)))
        });
        let complete = matches.len() <= limit;
        matches.truncate(limit);
        Ok(SearchResults {
            items: matches.into_iter().map(|(_, record)| record).collect(),
            complete,
        })
    }
}

pub fn search_score(query: &str, fields: &[&str]) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    fields
        .iter()
        .filter_map(|field| fuzzy_score(field, query))
        .max()
}

fn fuzzy_score(candidate: &str, query: &str) -> Option<i64> {
    let candidate = candidate.to_lowercase().chars().collect::<Vec<_>>();
    let query = query.to_lowercase().chars().collect::<Vec<_>>();
    if query.is_empty() {
        return Some(0);
    }
    let mut position = 0;
    let mut previous = None;
    let mut score = 0i64;
    for wanted in &query {
        let relative = candidate[position..]
            .iter()
            .position(|character| character == wanted)?;
        let found = position + relative;
        score += 20 - i64::try_from(relative.min(20)).unwrap_or(20);
        if previous.is_some_and(|previous| previous + 1 == found) {
            score += 15;
        }
        if found == 0
            || candidate
                .get(found.wrapping_sub(1))
                .is_some_and(|character| {
                    character.is_whitespace() || matches!(character, '/' | '-' | '_')
                })
        {
            score += 10;
        }
        previous = Some(found);
        position = found + 1;
    }
    if candidate == query {
        score += 1000;
    } else if candidate.starts_with(&query) {
        score += 500;
    }
    Some(score)
}
