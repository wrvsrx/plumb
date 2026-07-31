use std::path::PathBuf;

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
