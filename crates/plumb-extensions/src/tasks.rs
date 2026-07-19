use std::ops::Range;

use chrono::DateTime;
use plumb_core::{
    AttrItem, AttrValue, Block, Diagnostic, DiagnosticSeverity, Document, ParsedBlock,
};

use crate::document::attr_source_backed;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskField {
    pub value: String,
    pub range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskReferenceTarget {
    Internal { id: String },
    External { path: String, id: String },
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDependency {
    pub source: String,
    pub range: Range<usize>,
    pub target: TaskReferenceTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Open,
    Done,
    Canceled,
    Conflicted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRecord {
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
    pub title: String,
    pub depth: usize,
    pub id: Option<TaskField>,
    pub created: Option<TaskField>,
    pub due: Option<TaskField>,
    pub wait: Option<TaskField>,
    pub done: Option<TaskField>,
    pub canceled: Option<TaskField>,
    pub recur: Option<TaskField>,
    pub prev: Option<TaskField>,
    pub depends: Vec<TaskDependency>,
}

impl TaskRecord {
    pub fn state(&self) -> TaskState {
        match (self.done.is_some(), self.canceled.is_some()) {
            (false, false) => TaskState::Open,
            (true, false) => TaskState::Done,
            (false, true) => TaskState::Canceled,
            (true, true) => TaskState::Conflicted,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskOutput {
    pub tasks: Vec<TaskRecord>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn analyze_tasks(source: &str, document: &Document) -> TaskOutput {
    let mut output = TaskOutput::default();
    collect_blocks(source, &document.blocks, 0, &mut output);
    output
}

fn collect_blocks(source: &str, blocks: &[Block], task_depth: usize, output: &mut TaskOutput) {
    for block in blocks {
        let Block::Parsed(block) = block else {
            continue;
        };
        let is_task = block
            .mark
            .as_ref()
            .is_some_and(|mark| mark.attrs.has_class("task"));
        if is_task {
            let task = task_record(source, block, task_depth);
            collect_task_diagnostics(&task, output);
            output.tasks.push(task);
        }
        collect_blocks(
            source,
            &block.children,
            task_depth + usize::from(is_task),
            output,
        );
    }
}

fn task_record(source: &str, block: &ParsedBlock, depth: usize) -> TaskRecord {
    let attrs = &block.mark.as_ref().expect("task is a marked block").attrs;
    TaskRecord {
        range: block.range.clone(),
        selection_range: block.head.range.clone(),
        title: block.head.plain_text().trim().to_string(),
        depth,
        id: attrs.items.iter().find_map(|item| match item {
            AttrItem::Id { value, range } => Some(TaskField {
                value: value.clone(),
                range: range.start + 1..range.end,
            }),
            AttrItem::Class { .. } | AttrItem::Pair { .. } => None,
        }),
        created: datetime_field(attrs.items.as_slice(), "created"),
        due: datetime_field(attrs.items.as_slice(), "due"),
        wait: datetime_field(attrs.items.as_slice(), "wait"),
        done: datetime_field(attrs.items.as_slice(), "done"),
        canceled: datetime_field(attrs.items.as_slice(), "canceled"),
        recur: string_field(attrs.items.as_slice(), "recur"),
        prev: string_field(attrs.items.as_slice(), "prev"),
        depends: dependency_fields(source, attrs.items.as_slice()),
    }
}

fn datetime_field(items: &[AttrItem], key: &str) -> Option<TaskField> {
    let value = pair_value(items, key)?;
    DateTime::parse_from_rfc3339(&value.decoded)
        .is_ok()
        .then(|| task_field(value))
}

fn string_field(items: &[AttrItem], key: &str) -> Option<TaskField> {
    pair_value(items, key).map(task_field)
}

fn pair_value<'a>(items: &'a [AttrItem], wanted: &str) -> Option<&'a AttrValue> {
    items.iter().find_map(|item| match item {
        AttrItem::Pair { key, value, .. } if key == wanted => Some(value),
        AttrItem::Id { .. } | AttrItem::Class { .. } | AttrItem::Pair { .. } => None,
    })
}

fn task_field(value: &AttrValue) -> TaskField {
    TaskField {
        value: value.decoded.clone(),
        range: value.range.clone(),
    }
}

fn dependency_fields(source: &str, items: &[AttrItem]) -> Vec<TaskDependency> {
    let Some(value) = pair_value(items, "depends") else {
        return Vec::new();
    };
    let source_backed = attr_source_backed(source, value);
    dependency_tokens(&source_backed.value)
        .into_iter()
        .filter_map(|(token, decoded_range)| {
            Some(TaskDependency {
                source: token.to_string(),
                range: source_backed.source_range(decoded_range)?,
                target: task_reference_target(token),
            })
        })
        .collect()
}

fn dependency_tokens(value: &str) -> Vec<(&str, Range<usize>)> {
    let mut output = Vec::new();
    let mut start = None;
    for (offset, character) in value.char_indices() {
        if character.is_whitespace() {
            if let Some(start) = start.take() {
                output.push((&value[start..offset], start..offset));
            }
        } else if start.is_none() {
            start = Some(offset);
        }
    }
    if let Some(start) = start {
        output.push((&value[start..], start..value.len()));
    }
    output
}

fn task_reference_target(source: &str) -> TaskReferenceTarget {
    if let Some(id) = source.strip_prefix('#').filter(|id| !id.is_empty()) {
        TaskReferenceTarget::Internal { id: id.to_string() }
    } else if let Some((path, id)) = source
        .split_once('#')
        .filter(|(path, id)| path.ends_with(".plumb") && !id.is_empty())
    {
        TaskReferenceTarget::External {
            path: path.to_string(),
            id: id.to_string(),
        }
    } else {
        TaskReferenceTarget::Invalid
    }
}

fn collect_task_diagnostics(task: &TaskRecord, output: &mut TaskOutput) {
    if let (Some(done), Some(canceled)) = (&task.done, &task.canceled) {
        output.diagnostics.push(Diagnostic {
            code: "task.conflicting-closed-state",
            severity: DiagnosticSeverity::Warning,
            message: "a task cannot be both done and canceled".to_string(),
            range: canceled.range.clone(),
            related: vec![done.range.clone()],
        });
    }

    let Some(recur) = &task.recur else {
        return;
    };
    if !valid_repeat_rule(&recur.value) {
        output.diagnostics.push(Diagnostic {
            code: "task.invalid-recur",
            severity: DiagnosticSeverity::Warning,
            message: "recur must be PnD, PnW, PnM, or PnY with a positive integer n".to_string(),
            range: recur.range.clone(),
            related: Vec::new(),
        });
    }
    if task.due.is_none() {
        output.diagnostics.push(Diagnostic {
            code: "task.missing-due-for-recur",
            severity: DiagnosticSeverity::Warning,
            message: "a recurring task requires an RFC 3339 due datetime".to_string(),
            range: recur.range.clone(),
            related: Vec::new(),
        });
    }
}

fn valid_repeat_rule(value: &str) -> bool {
    let Some(value) = value.strip_prefix('P') else {
        return false;
    };
    let Some((unit, digits)) = value
        .chars()
        .last()
        .map(|unit| (unit, &value[..value.len() - unit.len_utf8()]))
    else {
        return false;
    };
    matches!(unit, 'D' | 'W' | 'M' | 'Y')
        && !digits.is_empty()
        && digits.bytes().all(|byte| byte.is_ascii_digit())
        && digits.parse::<u64>().is_ok_and(|count| count > 0)
}

#[cfg(test)]
mod tests {
    use plumb_core::parse;

    use super::*;

    #[test]
    fn collects_task_facets_fields_dependencies_and_nesting() {
        let source = "`item{.task #write created=\"2026-07-20T09:00:00+08:00\" due=\"2026-07-21T09:00:00+08:00\" wait=\"2026-07-20T12:00:00+08:00\" recur=P1W prev=\"#old\" depends=\"#draft other.plumb#review\"} Write parser\n  `note Details\n  `item{.task done=\"2026-07-20T10:00:00+08:00\"} Nested task\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_tasks(source, &parsed.syntax);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(output.tasks.len(), 2);
        let task = &output.tasks[0];
        assert_eq!(task.title, "Write parser");
        assert_eq!(task.depth, 0);
        assert_eq!(task.id.as_ref().unwrap().value, "write");
        assert_eq!(task.state(), TaskState::Open);
        assert_eq!(task.depends.len(), 2);
        assert_eq!(&source[task.depends[0].range.clone()], "#draft");
        assert!(matches!(
            task.depends[1].target,
            TaskReferenceTarget::External { ref path, ref id }
                if path == "other.plumb" && id == "review"
        ));
        assert_eq!(output.tasks[1].depth, 1);
        assert_eq!(output.tasks[1].state(), TaskState::Done);
    }

    #[test]
    fn reports_local_task_state_and_recurrence_diagnostics() {
        let source = "`item{.task done=\"2026-07-20T09:00:00Z\" canceled=\"2026-07-20T10:00:00Z\"} Conflict\n`item{.task due=\"not-a-date\" recur=P1M1D} Invalid recurrence\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_tasks(source, &parsed.syntax);
        assert_eq!(output.tasks[0].state(), TaskState::Conflicted);
        let codes = output
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();
        assert_eq!(
            codes,
            vec![
                "task.conflicting-closed-state",
                "task.invalid-recur",
                "task.missing-due-for-recur",
            ]
        );
    }
}
