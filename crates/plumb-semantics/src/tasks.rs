use std::ops::Range;
use std::path::Path;

use chrono::{DateTime, Datelike, Duration, FixedOffset, SecondsFormat, TimeZone, Timelike};
use plumb_syntax::{
    AttrItem, AttrValue, Block, Diagnostic, DiagnosticSeverity, ParsedBlock, ValidDocument,
};
use serde::{Deserialize, Serialize};

use crate::document::attr_source_backed;
use crate::text::plain_text;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskField {
    pub value: String,
    pub range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskReferenceTarget {
    Internal { id: String },
    External { path: String, id: String },
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDependency {
    pub source: String,
    pub range: Range<usize>,
    pub target: TaskReferenceTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    Open,
    Done,
    Canceled,
    Conflicted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Done,
    Canceled,
}

impl TaskStatus {
    pub fn attribute(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Canceled => "canceled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRecord {
    pub range: Range<usize>,
    pub marker_range: Range<usize>,
    pub selection_range: Range<usize>,
    pub title: String,
    pub depth: usize,
    pub attribute_insert: usize,
    pub attribute_range: Range<usize>,
    pub persistent_attributes: Vec<String>,
    pub id: Option<TaskField>,
    pub created: Option<TaskField>,
    pub due: Option<TaskField>,
    pub wait: Option<TaskField>,
    pub done: Option<TaskField>,
    pub canceled: Option<TaskField>,
    pub recur: Option<TaskField>,
    pub prev: Option<TaskField>,
    pub priority: Option<i32>,
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

pub fn analyze_tasks(valid: ValidDocument<'_>) -> TaskOutput {
    let source = valid.source();
    let document = valid.syntax();
    let mut output = TaskOutput::default();
    let table_items = crate::table_structural_item_starts(valid);
    for block in document
        .blocks
        .iter()
        .filter(|block| !crate::is_document_declaration(block))
    {
        collect_blocks(
            source,
            std::slice::from_ref(block),
            0,
            &table_items,
            &mut output,
        );
    }
    output
}

fn collect_blocks(
    source: &str,
    blocks: &[Block],
    task_depth: usize,
    table_items: &std::collections::HashSet<usize>,
    output: &mut TaskOutput,
) {
    for block in blocks {
        let Block::Parsed(block) = block else {
            continue;
        };
        let facet = if table_items.contains(&block.range.start) {
            crate::ListItemFacet::None
        } else {
            crate::list_item_facet(block)
        };
        let is_task = facet == crate::ListItemFacet::Task;
        if facet == crate::ListItemFacet::Conflict {
            let mark = block.mark.as_ref().expect("a list item has a mark");
            output.diagnostics.push(Diagnostic {
                code: "facet.task-event-conflict",
                severity: DiagnosticSeverity::Warning,
                message: "a list item cannot have both task and event facets".to_string(),
                range: mark.attrs.range.clone().unwrap_or(mark.marker_range.clone()),
                related: Vec::new(),
            });
        }
        if is_task {
            let task = task_record(source, block, task_depth);
            let attrs = &block.mark.as_ref().expect("task is a marked block").attrs;
            collect_task_diagnostics(&task, attrs.items.as_slice(), output);
            output.tasks.push(task);
        }
        for child in crate::body_children(block) {
            collect_blocks(
                source,
                std::slice::from_ref(child),
                task_depth + usize::from(is_task),
                table_items,
                output,
            );
        }
    }
}

fn task_record(source: &str, block: &ParsedBlock, depth: usize) -> TaskRecord {
    let mark = block.mark.as_ref().expect("task is a marked block");
    let attrs = &mark.attrs;
    TaskRecord {
        range: block.range.clone(),
        marker_range: mark.range.clone(),
        selection_range: crate::inline_selection_range(&block.head),
        title: plain_text(&block.head).trim().to_string(),
        depth,
        attribute_insert: block.head.range.end.max(mark.marker_range.end),
        attribute_range: attrs
            .range
            .clone()
            .unwrap_or(mark.marker_range.end..mark.marker_range.end),
        persistent_attributes: attrs
            .items
            .iter()
            .filter(|item| !transient_task_attribute(item))
            .map(|item| match item {
                AttrItem::Id { range, .. }
                | AttrItem::Class { range, .. }
                | AttrItem::Pair { range, .. } => source[range.clone()].to_string(),
            })
            .collect(),
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
        priority: priority_field(attrs.items.as_slice()),
        depends: dependency_fields(source, attrs.items.as_slice()),
    }
}

fn datetime_field(items: &[AttrItem], key: &str) -> Option<TaskField> {
    let value = pair_value(items, key)?;
    (value.quoted && valid_task_datetime(&value.decoded)).then(|| task_field(value))
}

pub fn valid_task_datetime(value: &str) -> bool {
    DateTime::parse_from_rfc3339(value).is_ok()
}

fn string_field(items: &[AttrItem], key: &str) -> Option<TaskField> {
    pair_value(items, key).map(task_field)
}

fn priority_field(items: &[AttrItem]) -> Option<i32> {
    pair_value(items, "priority")?.decoded.parse().ok()
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

fn transient_task_attribute(item: &AttrItem) -> bool {
    match item {
        AttrItem::Id { .. } => true,
        AttrItem::Pair { key, .. } => matches!(
            key.as_str(),
            "created" | "due" | "wait" | "done" | "canceled" | "recur" | "prev"
        ),
        AttrItem::Class { .. } => false,
    }
}

fn dependency_fields(source: &str, items: &[AttrItem]) -> Vec<TaskDependency> {
    task_reference_fields(source, items, "depends")
}

pub(crate) fn task_reference_fields(
    source: &str,
    items: &[AttrItem],
    key: &str,
) -> Vec<TaskDependency> {
    let Some(value) = pair_value(items, key) else {
        return Vec::new();
    };
    let source_backed = attr_source_backed(source, value);
    dependency_tokens(&source_backed.value)
        .into_iter()
        .filter_map(|(token, decoded_range)| {
            Some(TaskDependency {
                source: token.to_string(),
                range: source_backed.source_range(decoded_range)?,
                target: parse_task_reference_target(token),
            })
        })
        .collect()
}

fn dependency_tokens(value: &str) -> Vec<(&str, Range<usize>)> {
    let mut output = Vec::new();
    let mut cursor = 0;
    while cursor < value.len() {
        cursor += value[cursor..]
            .chars()
            .take_while(|character| character.is_whitespace())
            .map(char::len_utf8)
            .sum::<usize>();
        if cursor == value.len() {
            break;
        }
        let start = cursor;
        let id_start = if value[start..].starts_with('#') {
            start + 1
        } else if let Some(separator) = value[start..]
            .find(".plumb#")
            .filter(|separator| !value[start..start + separator].contains('#'))
        {
            start + separator + ".plumb#".len()
        } else {
            start
        };
        let end = value[id_start..]
            .find(char::is_whitespace)
            .map_or(value.len(), |offset| id_start + offset);
        output.push((&value[start..end], start..end));
        cursor = end;
    }
    output
}

pub fn parse_task_reference_target(source: &str) -> TaskReferenceTarget {
    if let Some(id) = source
        .strip_prefix('#')
        .filter(|id| valid_task_reference_id(id))
    {
        TaskReferenceTarget::Internal { id: id.to_string() }
    } else if let Some((path, id)) = source.split_once('#').filter(|(path, id)| {
        path.ends_with(".plumb") && valid_task_reference_path(path) && valid_task_reference_id(id)
    }) {
        TaskReferenceTarget::External {
            path: path.to_string(),
            id: id.to_string(),
        }
    } else {
        TaskReferenceTarget::Invalid
    }
}

fn valid_task_reference_path(path: &str) -> bool {
    !path.is_empty()
        && !Path::new(path).is_absolute()
        && !path
            .chars()
            .any(|character| character.is_control() || matches!(character, '\\' | '#'))
}

fn valid_task_reference_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().all(|character| {
            !character.is_whitespace()
                && !character.is_control()
                && !matches!(
                    character,
                    '`' | '"' | '[' | ']' | '{' | '}' | '#' | '.' | '='
                )
        })
}

fn collect_task_diagnostics(task: &TaskRecord, attrs: &[AttrItem], output: &mut TaskOutput) {
    for key in ["created", "due", "wait", "done", "canceled"] {
        let Some(value) = pair_value(attrs, key) else {
            continue;
        };
        if !value.quoted || !valid_task_datetime(&value.decoded) {
            output.diagnostics.push(Diagnostic {
                code: "task.invalid-datetime",
                severity: DiagnosticSeverity::Warning,
                message: format!(
                    "'{key}' must be an RFC 3339 timestamp property or quoted legacy value"
                ),
                range: value.range.clone(),
                related: Vec::new(),
            });
        }
    }

    if let Some(value) = pair_value(attrs, "priority") {
        if value.decoded.parse::<i32>().is_err() {
            output.diagnostics.push(Diagnostic {
                code: "task.invalid-priority",
                severity: DiagnosticSeverity::Warning,
                message: "'priority' must be a signed 32-bit integer".to_string(),
                range: value.range.clone(),
                related: Vec::new(),
            });
        }
    }

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
    if pair_value(attrs, "due").is_none() {
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
    parse_repeat_rule(value).is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepeatRule {
    Days(i64),
    Weeks(i64),
    Months(i32),
    Years(i32),
}

fn parse_repeat_rule(value: &str) -> Option<RepeatRule> {
    let value = value.strip_prefix('P')?;
    let (unit, digits) = value
        .chars()
        .last()
        .map(|unit| (unit, &value[..value.len() - unit.len_utf8()]))?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let count = digits.parse::<u64>().ok().filter(|count| *count > 0)?;
    match unit {
        'D' => i64::try_from(count).ok().map(RepeatRule::Days),
        'W' => i64::try_from(count).ok().map(RepeatRule::Weeks),
        'M' => i32::try_from(count).ok().map(RepeatRule::Months),
        'Y' => i32::try_from(count).ok().map(RepeatRule::Years),
        _ => None,
    }
}

pub fn next_task_datetime(datetime: &str, recur: &str) -> Option<String> {
    let datetime = DateTime::parse_from_rfc3339(datetime).ok()?;
    let next = match parse_repeat_rule(recur)? {
        RepeatRule::Days(days) => datetime + Duration::days(days),
        RepeatRule::Weeks(weeks) => datetime + Duration::weeks(weeks),
        RepeatRule::Months(months) => add_months(datetime, months)?,
        RepeatRule::Years(years) => add_months(datetime, years.checked_mul(12)?)?,
    };
    Some(next.to_rfc3339_opts(SecondsFormat::Secs, false))
}

fn add_months(datetime: DateTime<FixedOffset>, months: i32) -> Option<DateTime<FixedOffset>> {
    let month0 = datetime.month0() as i32 + months;
    let year = datetime.year() + month0.div_euclid(12);
    let month = (month0.rem_euclid(12) + 1) as u32;
    let day = datetime.day().min(last_day_of_month(year, month)?);
    datetime
        .timezone()
        .with_ymd_and_hms(
            year,
            month,
            day,
            datetime.hour(),
            datetime.minute(),
            datetime.second(),
        )
        .single()
}

fn last_day_of_month(year: i32, month: u32) -> Option<u32> {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let first_next = chrono::NaiveDate::from_ymd_opt(next_year, next_month, 1)?;
    Some((first_next - Duration::days(1)).day())
}

#[cfg(test)]
mod tests {
    use plumb_syntax::parse;

    use super::*;

    #[test]
    fn collects_task_facets_fields_dependencies_and_nesting() {
        let source = "`- Write parser\n `+ task\n `@ write\n `= created|2026-07-20T09:00:00+08:00\n `= due|2026-07-21T09:00:00+08:00\n `= wait|2026-07-20T12:00:00+08:00\n `= recur|P1W\n `= prev|#old\n `= depends|#draft other notes.plumb#review third.plumb#done\n\n `note Details\n\n `- Nested task\n  `+ task\n  `= done|2026-07-20T10:00:00+08:00\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_tasks(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(output.tasks.len(), 2);
        let task = &output.tasks[0];
        assert_eq!(task.title, "Write parser");
        assert_eq!(task.depth, 0);
        assert_eq!(
            &source[task.attribute_insert..task.attribute_insert + 1],
            "\n"
        );
        assert_eq!(task.id.as_ref().unwrap().value, "write");
        assert_eq!(task.state(), TaskState::Open);
        assert_eq!(task.depends.len(), 3);
        assert_eq!(&source[task.depends[0].range.clone()], "#draft");
        assert!(matches!(
            task.depends[1].target,
            TaskReferenceTarget::External { ref path, ref id }
                if path == "other notes.plumb" && id == "review"
        ));
        assert_eq!(
            &source[task.depends[1].range.clone()],
            "other notes.plumb#review"
        );
        assert!(matches!(
            task.depends[2].target,
            TaskReferenceTarget::External { ref path, ref id }
                if path == "third.plumb" && id == "done"
        ));
        assert_eq!(output.tasks[1].depth, 1);
        assert_eq!(output.tasks[1].state(), TaskState::Done);
    }

    #[test]
    fn parses_raw_task_reference_paths_and_reserves_hash_for_the_anchor() {
        let dependencies = dependency_tokens(
            "#local Project A.plumb#build Project%20A.plumb#literal third.plumb#done",
        );
        assert_eq!(
            dependencies
                .iter()
                .map(|(source, _)| *source)
                .collect::<Vec<_>>(),
            [
                "#local",
                "Project A.plumb#build",
                "Project%20A.plumb#literal",
                "third.plumb#done"
            ]
        );
        assert_eq!(
            dependency_tokens("bare#invalid missing.plumb#x")
                .iter()
                .map(|(source, _)| *source)
                .collect::<Vec<_>>(),
            ["bare#invalid", "missing.plumb#x"]
        );
        assert!(matches!(
            parse_task_reference_target("Project A.plumb#build"),
            TaskReferenceTarget::External { ref path, ref id }
                if path == "Project A.plumb" && id == "build"
        ));
        assert!(matches!(
            parse_task_reference_target("Project%20A.plumb#literal"),
            TaskReferenceTarget::External { ref path, ref id }
                if path == "Project%20A.plumb" && id == "literal"
        ));
        for invalid in [
            "Project#A.plumb#build",
            "/Project A.plumb#build",
            "Project A.plumb#bad.id",
            "Project A.plumb#",
        ] {
            assert_eq!(
                parse_task_reference_target(invalid),
                TaskReferenceTarget::Invalid
            );
        }
    }

    #[test]
    fn direct_dependency_values_keep_exact_source_ranges() {
        let source = "`- Review\n  `+ task\n  `@ review\n  `= depends|Project Plan.plumb#build #local\n";
        let parsed = plumb_syntax::parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_tasks(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        let task = &output.tasks[0];
        assert_eq!(task.depends.len(), 2);
        assert_eq!(
            &source[task.depends[0].range.clone()],
            "Project Plan.plumb#build"
        );
        assert_eq!(&source[task.depends[1].range.clone()], "#local");
    }

    #[test]
    fn reports_local_task_state_and_recurrence_diagnostics() {
        let source = "`- Conflict\n  `+ task\n  `= done|2026-07-20T09:00:00Z\n  `= canceled|2026-07-20T10:00:00Z\n`- Invalid recurrence\n  `+ task\n  `= due|not-a-date\n  `= recur|P1M1D\n`- Invalid datetimes\n  `+ task\n  `= created|2026-07-20T09:00:00Z\n  `= wait|tomorrow\n  `= done|later\n  `= canceled|never\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_tasks(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
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
                "task.invalid-datetime",
                "task.invalid-recur",
                "task.invalid-datetime",
                "task.invalid-datetime",
                "task.invalid-datetime",
            ]
        );
        assert_eq!(output.tasks[1].due, None);
        assert_eq!(
            output.tasks[2]
                .created
                .as_ref()
                .map(|field| field.value.as_str()),
            Some("2026-07-20T09:00:00Z")
        );
        assert_eq!(output.tasks[2].state(), TaskState::Open);
    }

    #[test]
    fn parses_signed_task_priority_and_rejects_out_of_range_values() {
        let source = "`- Maximum\n  `+ task\n  `= priority|2147483647\n`- Deferred\n  `+ task\n  `= priority|-12\n`- Minimum\n  `+ task\n  `= priority|-2147483648\n`- Too large\n  `+ task\n  `= priority|2147483648\n`- Too small\n  `+ task\n  `= priority|-2147483649\n`- Invalid\n  `+ task\n  `= priority|soon\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_tasks(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(output.tasks[0].priority, Some(i32::MAX));
        assert_eq!(output.tasks[1].priority, Some(-12));
        assert_eq!(output.tasks[2].priority, Some(i32::MIN));
        assert_eq!(output.tasks[3].priority, None);
        assert_eq!(output.tasks[4].priority, None);
        assert_eq!(output.tasks[5].priority, None);
        assert_eq!(
            output
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            [
                "task.invalid-priority",
                "task.invalid-priority",
                "task.invalid-priority"
            ]
        );
    }

    #[test]
    fn reports_missing_due_only_when_the_attribute_is_absent() {
        let source =
            "`- Missing due\n  `+ task\n  `= recur|P1W\n`- Invalid due\n  `+ task\n  `= due|invalid\n  `= recur|P1W\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_tasks(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        let codes = output
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();
        assert_eq!(
            codes,
            vec!["task.missing-due-for-recur", "task.invalid-datetime"]
        );
    }

    #[test]
    fn advances_task_datetimes_by_calendar_repeat_rules() {
        assert_eq!(
            next_task_datetime("2026-07-20T09:00:00+08:00", "P2W").as_deref(),
            Some("2026-08-03T09:00:00+08:00")
        );
        assert_eq!(
            next_task_datetime("2026-01-31T09:00:00+08:00", "P1M").as_deref(),
            Some("2026-02-28T09:00:00+08:00")
        );
        assert_eq!(
            next_task_datetime("2024-02-29T09:00:00Z", "P1Y").as_deref(),
            Some("2025-02-28T09:00:00+00:00")
        );
        assert!(next_task_datetime("2026-07-20T09:00:00Z", "P1M1D").is_none());
    }

    #[test]
    fn task_facet_requires_a_list_item_and_specialized_markers_are_ordinary() {
        let source = "`note Not a task\n  `+ task\n\n`task Legacy marker\n\n`. Work\n `+ task\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_tasks(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(output.tasks.len(), 1);
        assert!(output.diagnostics.is_empty());
    }

    #[test]
    fn conflicting_facets_create_neither_record_and_table_items_are_consumed() {
        let source = "`- Conflict\n `+ task\n `+ event\n\n`table\n `- Cell\n  `+ task\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_tasks(parsed.valid_syntax().unwrap());
        assert!(output.tasks.is_empty());
        assert_eq!(output.diagnostics.len(), 1);
        assert_eq!(output.diagnostics[0].code, "facet.task-event-conflict");
    }
}
