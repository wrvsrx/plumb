use std::ops::Range;
use std::path::Path;

use chrono::{DateTime, Datelike, Duration, FixedOffset, SecondsFormat, TimeZone, Timelike};
use plumb_syntax::{
    AttrItem, AttrValue, Block, Diagnostic, DiagnosticSeverity, ParsedBlock, ValidDocument,
    ValidGreenDocument,
};
use serde::{Deserialize, Serialize};

use crate::document::attr_source_backed;
use crate::text::plain_text;
use crate::{RelativeSemanticRecord, SemanticDiagnostics, SemanticRecords};

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

/// One focus interval written as `start--end`, or `start--` when still open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusInterval {
    pub start: String,
    pub end: Option<String>,
    pub range: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FocusProblemCode {
    Invalid,
    Duplicate,
    Unordered,
    Overlapping,
    MultipleOpen,
    Closed,
}

impl FocusProblemCode {
    pub fn code(self) -> &'static str {
        match self {
            Self::Invalid => "task.invalid-focus",
            Self::Duplicate => "task.duplicate-focus",
            Self::Unordered => "task.unordered-focus",
            Self::Overlapping => "task.overlapping-focus",
            Self::MultipleOpen => "task.multiple-open-focus",
            Self::Closed => "task.focus-on-closed",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Invalid => {
                "'focused' must be one interval (start--end or start--) or a list of intervals"
            }
            Self::Duplicate => "a task may declare at most one 'focused' property",
            Self::Unordered => "'focused' intervals must be ordered by start time",
            Self::Overlapping => "'focused' intervals must not overlap",
            Self::MultipleOpen => {
                "'focused' may contain at most one open interval, and it must be last"
            }
            Self::Closed => "a closed task cannot keep an open 'focused' interval",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusProblem {
    pub code: FocusProblemCode,
    pub range: Range<usize>,
    pub related: Vec<Range<usize>>,
}

/// Parsed `focused` history. `present == false` means the property is absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TaskFocus {
    pub present: bool,
    pub invalid: bool,
    pub list_form: bool,
    pub range: Option<Range<usize>>,
    pub intervals: Vec<FocusInterval>,
    pub problems: Vec<FocusProblem>,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskOwner {
    Document,
    #[default]
    ListItem,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRecord {
    pub owner: TaskOwner,
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
    pub focused: TaskFocus,
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

    /// Focus history is usable only when the property itself parses.
    pub fn focus_valid(&self) -> bool {
        !self.focused.invalid
    }

    /// Currently focused: a valid open interval on a task whose closure is still open.
    /// A finished history alone never makes a task focused.
    pub fn is_focused(&self) -> bool {
        self.focused.present
            && self.focus_valid()
            && self.state() == TaskState::Open
            && self.has_open_focus_interval()
    }

    pub fn has_open_focus_interval(&self) -> bool {
        self.focused
            .intervals
            .last()
            .is_some_and(|interval| interval.end.is_none())
    }

    pub fn focused_since(&self) -> Option<&str> {
        if !self.is_focused() {
            return None;
        }
        self.focused
            .intervals
            .last()
            .map(|interval| interval.start.as_str())
    }
}

impl<'a> crate::SemanticRecordView<'a, TaskRecord> {
    /// Identity/type presence and source-backed outgoing reference inputs, not workflow state.
    pub fn reference_inputs_equal(self, other: crate::SemanticRecordView<'_, TaskRecord>) -> bool {
        self.owner() == other.owner()
            && self.range() == other.range()
            && self.id_value() == other.id_value()
            && self.previous_value() == other.previous_value()
            && self
                .record
                .depends
                .iter()
                .map(|dependency| (&dependency.source, &dependency.target))
                .eq(other
                    .record
                    .depends
                    .iter()
                    .map(|dependency| (&dependency.source, &dependency.target)))
            && self.reference_ranges().eq(other.reference_ranges())
    }

    pub fn reference_ranges(self) -> impl Iterator<Item = Range<usize>> + 'a {
        let offset = self.offset;
        self.record
            .prev
            .iter()
            .map(|field| &field.range)
            .chain(
                self.record
                    .depends
                    .iter()
                    .map(|dependency| &dependency.range),
            )
            .map(move |range| {
                range.start.checked_add_signed(offset).unwrap()
                    ..range.end.checked_add_signed(offset).unwrap()
            })
    }

    pub fn previous_value(self) -> Option<&'a str> {
        self.record.prev.as_ref().map(|field| field.value.as_str())
    }

    pub fn dependency_targets(self) -> impl Iterator<Item = &'a TaskReferenceTarget> {
        self.record
            .depends
            .iter()
            .map(|dependency| &dependency.target)
    }

    pub fn owner(self) -> TaskOwner {
        self.record.owner
    }

    pub fn title(self) -> &'a str {
        &self.record.title
    }

    pub fn id_value(self) -> Option<&'a str> {
        self.record.id.as_ref().map(|id| id.value.as_str())
    }

    pub fn selection_range(self) -> Range<usize> {
        self.record
            .selection_range
            .start
            .checked_add_signed(self.offset)
            .unwrap()
            ..self
                .record
                .selection_range
                .end
                .checked_add_signed(self.offset)
                .unwrap()
    }

    pub fn range(self) -> Range<usize> {
        self.record
            .range
            .start
            .checked_add_signed(self.offset)
            .unwrap()
            ..self
                .record
                .range
                .end
                .checked_add_signed(self.offset)
                .unwrap()
    }

    pub fn depth(self) -> usize {
        self.record.depth
    }

    pub fn state(self) -> TaskState {
        self.record.state()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskOutput {
    pub tasks: SemanticRecords<TaskRecord>,
    pub diagnostics: SemanticDiagnostics,
}

impl TaskOutput {
    pub fn document_task(&self) -> Option<crate::SemanticRecordView<'_, TaskRecord>> {
        self.tasks
            .views()
            .next()
            .filter(|task| task.owner() == TaskOwner::Document)
    }
}

impl RelativeSemanticRecord for TaskRecord {
    fn start(&self) -> usize {
        self.range.start
    }

    fn shift(&mut self, delta: isize) {
        shift_range(&mut self.range, delta);
        shift_range(&mut self.marker_range, delta);
        shift_range(&mut self.selection_range, delta);
        self.attribute_insert = self.attribute_insert.checked_add_signed(delta).unwrap();
        shift_range(&mut self.attribute_range, delta);
        for field in [
            &mut self.id,
            &mut self.created,
            &mut self.due,
            &mut self.wait,
            &mut self.done,
            &mut self.canceled,
            &mut self.recur,
            &mut self.prev,
        ] {
            if let Some(field) = field {
                shift_range(&mut field.range, delta);
            }
        }
        for dependency in &mut self.depends {
            shift_range(&mut dependency.range, delta);
        }
        shift_focus(&mut self.focused, delta);
    }
}

fn shift_focus(focus: &mut TaskFocus, delta: isize) {
    if let Some(range) = focus.range.as_mut() {
        shift_range(range, delta);
    }
    for interval in &mut focus.intervals {
        shift_range(&mut interval.range, delta);
    }
    for problem in &mut focus.problems {
        shift_range(&mut problem.range, delta);
        for range in &mut problem.related {
            shift_range(range, delta);
        }
    }
}

fn shift_range(range: &mut Range<usize>, delta: isize) {
    range.start = range.start.checked_add_signed(delta).unwrap();
    range.end = range.end.checked_add_signed(delta).unwrap();
}

pub fn analyze_tasks(valid: ValidDocument<'_>) -> TaskOutput {
    let metadata = crate::analyze_metadata(valid);
    let document_task = document_task_record(
        valid.source(),
        &metadata,
        [(valid.syntax(), valid.source(), 0)],
    );
    let mut output = analyze_list_tasks(valid, usize::from(document_task.is_some()));
    prepend_document_task(document_task, &mut output);
    output
}

pub(crate) fn analyze_list_tasks(valid: ValidDocument<'_>, depth: usize) -> TaskOutput {
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
            depth,
            &table_items,
            &mut output,
        );
    }
    output
}

pub fn analyze_green_tasks(valid: ValidGreenDocument<'_>) -> TaskOutput {
    let mut output = TaskOutput::default();
    let metadata = crate::metadata::analyze_green_metadata(valid);
    let document_task = green_document_task_record(valid, &metadata);
    let depth = usize::from(document_task.is_some());
    for shard in valid.syntax().shards() {
        let local = shard
            .shard()
            .parsed()
            .valid_syntax()
            .expect("valid green document has valid shards");
        let local = analyze_list_tasks(local, depth);
        for mut task in local.tasks.iter() {
            task.shift(shard.offset() as isize);
            output.tasks.push(task);
        }
        for mut diagnostic in local.diagnostics.iter() {
            shift_range(&mut diagnostic.range, shard.offset() as isize);
            for related in &mut diagnostic.related {
                shift_range(related, shard.offset() as isize);
            }
            output.diagnostics.push(diagnostic);
        }
    }
    prepend_document_task(document_task, &mut output);
    output
}

/// Reduce root declarations independently of list-owner shard analysis.
pub(crate) fn green_document_task_record(
    valid: ValidGreenDocument<'_>,
    metadata: &crate::MetadataOutput,
) -> Option<(TaskRecord, Vec<AttrItem>)> {
    document_task_record(
        valid.source(),
        metadata,
        valid.syntax().shards().map(|view| {
            let parsed = view.shard().parsed();
            (&parsed.syntax, parsed.source.as_str(), view.offset())
        }),
    )
}

fn document_task_record<'a>(
    source: &str,
    metadata: &crate::MetadataOutput,
    documents: impl IntoIterator<Item = (&'a plumb_syntax::Document, &'a str, usize)>,
) -> Option<(TaskRecord, Vec<AttrItem>)> {
    let facet = metadata.facets.iter().find(|facet| facet.name == "task")?;
    let mut attrs = Vec::new();
    let mut focus_occurrences = Vec::new();
    for (document, local_source, offset) in documents {
        let mut focus = focus_field(local_source, &document.blocks, &document.attrs.items);
        if focus.present {
            shift_focus(&mut focus, offset as isize);
            focus_occurrences.push(focus);
        }
        for item in &document.attrs.items {
            let mut item = item.clone();
            let delta = offset as isize;
            match &mut item {
                AttrItem::Id {
                    range, value_range, ..
                }
                | AttrItem::Class {
                    range, value_range, ..
                } => {
                    shift_range(range, delta);
                    shift_range(value_range, delta);
                }
                AttrItem::Pair {
                    range,
                    key_range,
                    value,
                    ..
                } => {
                    shift_range(range, delta);
                    shift_range(key_range, delta);
                    shift_range(&mut value.range, delta);
                }
            }
            attrs.push(item);
        }
    }
    let selection_range = metadata
        .metadata
        .as_ref()
        .and_then(|metadata| {
            metadata
                .entries
                .iter()
                .find(|entry| entry.key == "title")
                .map(|entry| entry.value.range().clone())
        })
        .unwrap_or_else(|| facet.selection_range.clone());
    let task = TaskRecord {
        owner: TaskOwner::Document,
        range: 0..source.len(),
        marker_range: facet.range.clone(),
        selection_range,
        title: metadata.document_title().unwrap_or_default(),
        depth: 0,
        attribute_insert: facet.range.end,
        attribute_range: 0..source.len(),
        id: None,
        ..task_properties(source, &attrs, merge_focus_occurrences(focus_occurrences))
    };
    Some((task, attrs))
}

pub(crate) fn prepend_document_task(
    document: Option<(TaskRecord, Vec<AttrItem>)>,
    output: &mut TaskOutput,
) {
    let Some((task, attrs)) = document else {
        return;
    };
    let mut combined = TaskOutput::default();
    collect_task_diagnostics(&task, &attrs, &mut combined);
    combined.tasks.push(task);
    for task in output.tasks.iter() {
        combined.tasks.push(task);
    }
    for diagnostic in output.diagnostics.iter() {
        combined.diagnostics.push(diagnostic);
    }
    *output = combined;
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
                range: mark
                    .attrs
                    .range
                    .clone()
                    .unwrap_or(mark.marker_range.clone()),
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
        selection_range: crate::inline_selection_range(&block.content),
        title: plain_text(&block.content).trim().to_string(),
        depth,
        attribute_insert: block.content.range.end.max(mark.marker_range.end),
        attribute_range: attrs
            .range
            .clone()
            .unwrap_or(mark.marker_range.end..mark.marker_range.end),
        ..task_properties(
            source,
            &attrs.items,
            focus_field(source, &block.children, &attrs.items),
        )
    }
}

fn task_properties(source: &str, items: &[AttrItem], focused: TaskFocus) -> TaskRecord {
    TaskRecord {
        persistent_attributes: items
            .iter()
            .filter(|item| !transient_task_attribute(item))
            .map(|item| match item {
                AttrItem::Id { range, .. }
                | AttrItem::Class { range, .. }
                | AttrItem::Pair { range, .. } => source[range.clone()].to_string(),
            })
            .collect(),
        id: items.iter().find_map(|item| match item {
            AttrItem::Id {
                value, value_range, ..
            } => Some(TaskField {
                value: value.clone(),
                range: value_range.clone(),
            }),
            AttrItem::Class { .. } | AttrItem::Pair { .. } => None,
        }),
        created: datetime_field(items, "created"),
        due: datetime_field(items, "due"),
        wait: datetime_field(items, "wait"),
        done: datetime_field(items, "done"),
        canceled: datetime_field(items, "canceled"),
        recur: string_field(items, "recur"),
        prev: string_field(items, "prev"),
        priority: priority_field(items),
        depends: dependency_fields(source, items),
        focused,
        ..TaskRecord::default()
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

/// `focused` is either one leaf interval value or a child-bearing `=` declaration whose
/// direct `-` children each hold exactly one interval. The child-bearing form is not an
/// `AttrItem`, so it is read from the owning block's children.
fn focus_field(source: &str, children: &[Block], items: &[AttrItem]) -> TaskFocus {
    let mut occurrences: Vec<TaskFocus> = Vec::new();

    for item in items {
        if let AttrItem::Pair { key, value, .. } = item {
            if key == "focused" {
                let mut focus = TaskFocus {
                    present: true,
                    range: Some(value.range.clone()),
                    ..TaskFocus::default()
                };
                parse_focus_scalar(
                    value.decoded.trim(),
                    trim_source_range(source, &value.range),
                    &mut focus,
                );
                validate_focus_history(&mut focus);
                occurrences.push(focus);
            }
        }
    }

    for child in children {
        let Block::Parsed(child) = child else {
            continue;
        };
        let Some(mark) = child.mark.as_ref() else {
            continue;
        };
        if mark.marker != "=" {
            continue;
        }
        if child.children.is_empty() {
            // An empty `= focused` is dropped by the attribute view; report it here.
            if plain_text(&child.content).trim() == "focused" {
                let mut focus = TaskFocus {
                    present: true,
                    range: Some(child.range.clone()),
                    ..TaskFocus::default()
                };
                push_focus_problem(
                    &mut focus,
                    FocusProblemCode::Invalid,
                    child.content.range.clone(),
                    Vec::new(),
                );
                occurrences.push(focus);
            }
            continue;
        }
        let head = plain_text(&child.content).trim().to_string();
        if head != "focused" && !head.starts_with("focused ") {
            continue;
        }
        let mut focus = TaskFocus {
            present: true,
            list_form: true,
            range: Some(child.range.clone()),
            ..TaskFocus::default()
        };
        if head != "focused" {
            push_focus_problem(
                &mut focus,
                FocusProblemCode::Invalid,
                child.content.range.clone(),
                Vec::new(),
            );
        }
        for entry in &child.children {
            let Block::Parsed(entry) = entry else {
                push_focus_problem(
                    &mut focus,
                    FocusProblemCode::Invalid,
                    child.range.clone(),
                    Vec::new(),
                );
                continue;
            };
            let range = trim_source_range(source, &entry.content.range);
            let marker_is_list_item = entry.mark.as_ref().is_some_and(|mark| mark.marker == "-");
            if !marker_is_list_item || !entry.children.is_empty() {
                push_focus_problem(&mut focus, FocusProblemCode::Invalid, range, Vec::new());
                continue;
            }
            parse_focus_scalar(plain_text(&entry.content).trim(), range, &mut focus);
        }
        if focus.intervals.is_empty() && !focus.invalid {
            push_focus_problem(
                &mut focus,
                FocusProblemCode::Invalid,
                child.range.clone(),
                Vec::new(),
            );
        }
        validate_focus_history(&mut focus);
        occurrences.push(focus);
    }

    merge_focus_occurrences(occurrences)
}

fn merge_focus_occurrences(mut occurrences: Vec<TaskFocus>) -> TaskFocus {
    match occurrences.len() {
        0 => TaskFocus::default(),
        1 => occurrences.pop().expect("one focus declaration"),
        _ => {
            let related: Vec<Range<usize>> = occurrences
                .iter()
                .filter_map(|focus| focus.range.clone())
                .collect();
            let range = related
                .get(1)
                .cloned()
                .unwrap_or_else(|| related.first().cloned().unwrap_or(0..0));
            let mut first = occurrences.remove(0);
            push_focus_problem(&mut first, FocusProblemCode::Duplicate, range, related);
            first
        }
    }
}

fn trim_source_range(source: &str, range: &Range<usize>) -> Range<usize> {
    let Some(slice) = source.get(range.clone()) else {
        return range.clone();
    };
    let leading = slice.len() - slice.trim_start().len();
    let trailing = slice.len() - slice.trim_end().len();
    range.start + leading..range.end.saturating_sub(trailing)
}

fn parse_focus_scalar(text: &str, range: Range<usize>, focus: &mut TaskFocus) {
    let Some((start, end)) = split_focus_interval(text) else {
        push_focus_problem(focus, FocusProblemCode::Invalid, range, Vec::new());
        return;
    };
    if !valid_task_datetime(start) || end.is_some_and(|value| !valid_task_datetime(value)) {
        push_focus_problem(focus, FocusProblemCode::Invalid, range, Vec::new());
        return;
    }
    if let (Some(end), Some(start_millis)) = (end, focus_millis(start)) {
        if focus_millis(end).is_some_and(|end_millis| end_millis < start_millis) {
            push_focus_problem(focus, FocusProblemCode::Invalid, range, Vec::new());
            return;
        }
    }
    focus.intervals.push(FocusInterval {
        start: start.to_string(),
        end: end.map(str::to_string),
        range,
    });
}

/// Intervals use a literal `--` separator; single `-` only appears inside offsets.
fn split_focus_interval(text: &str) -> Option<(&str, Option<&str>)> {
    let (start, end) = text.split_once("--")?;
    let start = start.trim();
    if start.is_empty() || end.contains("--") {
        return None;
    }
    let end = end.trim();
    (!start.is_empty()).then_some((start, (!end.is_empty()).then_some(end)))
}

fn focus_millis(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|datetime| datetime.timestamp_millis())
}

fn validate_focus_history(focus: &mut TaskFocus) {
    let count = focus.intervals.len();
    let mut previous_start: Option<i64> = None;
    let mut previous_end: Option<i64> = None;
    let mut open_seen = false;
    let mut problems: Vec<FocusProblem> = Vec::new();
    for (index, interval) in focus.intervals.iter().enumerate() {
        let Some(start) = focus_millis(&interval.start) else {
            continue;
        };
        let end = interval.end.as_deref().and_then(focus_millis);
        if let Some(previous_start) = previous_start {
            let code = if start < previous_start {
                Some(FocusProblemCode::Unordered)
            } else if previous_end.is_some_and(|previous_end| start < previous_end) {
                Some(FocusProblemCode::Overlapping)
            } else {
                None
            };
            if let Some(code) = code {
                problems.push(FocusProblem {
                    code,
                    range: interval.range.clone(),
                    related: Vec::new(),
                });
            }
        }
        if end.is_none() {
            if open_seen || index + 1 != count {
                problems.push(FocusProblem {
                    code: FocusProblemCode::MultipleOpen,
                    range: interval.range.clone(),
                    related: Vec::new(),
                });
            }
            open_seen = true;
        }
        previous_start = Some(start);
        previous_end = end;
    }
    if !problems.is_empty() {
        focus.invalid = true;
        focus.problems.extend(problems);
    }
}

fn push_focus_problem(
    focus: &mut TaskFocus,
    code: FocusProblemCode,
    range: Range<usize>,
    related: Vec<Range<usize>>,
) {
    focus.invalid = true;
    focus.problems.push(FocusProblem {
        code,
        range,
        related,
    });
}

fn transient_task_attribute(item: &AttrItem) -> bool {
    match item {
        AttrItem::Id { .. } => true,
        AttrItem::Pair { key, .. } => matches!(
            key.as_str(),
            "created" | "due" | "wait" | "done" | "canceled" | "recur" | "prev" | "focused"
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

    for problem in &task.focused.problems {
        output.diagnostics.push(Diagnostic {
            code: problem.code.code(),
            severity: DiagnosticSeverity::Warning,
            message: problem.code.message().to_string(),
            range: problem.range.clone(),
            related: problem.related.clone(),
        });
    }
    if task.focused.present
        && task.focus_valid()
        && task.state() != TaskState::Open
        && task.has_open_focus_interval()
    {
        output.diagnostics.push(Diagnostic {
            code: FocusProblemCode::Closed.code(),
            severity: DiagnosticSeverity::Warning,
            message: FocusProblemCode::Closed.message().to_string(),
            range: task
                .focused
                .intervals
                .last()
                .map(|interval| interval.range.clone())
                .unwrap_or(0..0),
            related: Vec::new(),
        });
    }

    let Some(recur) = &task.recur else {
        return;
    };
    if task.owner == TaskOwner::Document {
        output.diagnostics.push(Diagnostic {
            code: "task.document-recur",
            severity: DiagnosticSeverity::Warning,
            message: "document tasks do not support recurrence".to_owned(),
            range: recur.range.clone(),
            related: Vec::new(),
        });
        return;
    }
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
    fn document_task_reduces_interleaved_declarations_once_and_organizes_children() {
        let source = "`- First\n `+ task\n\n `- Child\n  `+ task\n\n`= title Whole document\n`+ task\n`= priority 7\n\nDetails.\n\n`+ task\n`= focused\n `- 2026-09-20T09:00:00+08:00--2026-09-20T10:00:00+08:00\n `- 2026-09-21T09:00:00+08:00--\n\n`- Last\n `+ task\n";
        let parsed = parse(source);
        let green = plumb_syntax::GreenDocument::parse(source);
        let output = analyze_tasks(parsed.valid_syntax().unwrap());
        assert_eq!(output, analyze_green_tasks(green.valid_syntax().unwrap()));
        assert_eq!(
            output,
            *crate::analyze_document(parsed.valid_syntax().unwrap()).tasks()
        );
        let tasks = output.tasks.iter().collect::<Vec<_>>();
        assert_eq!(tasks.len(), 4);
        assert_eq!(
            tasks.iter().map(|task| task.depth).collect::<Vec<_>>(),
            [0, 1, 2, 1]
        );
        let document = &tasks[0];
        assert_eq!(document.owner, TaskOwner::Document);
        assert_eq!(document.title, "Whole document");
        assert_eq!(&source[document.selection_range.clone()], "Whole document");
        assert_eq!(document.range, 0..source.len());
        assert_eq!(document.id, None);
        assert_eq!(document.priority, Some(7));
        assert!(document.is_focused());
        assert_eq!(document.focused.intervals.len(), 2);
        assert!(tasks[1..]
            .iter()
            .all(|task| task.owner == TaskOwner::ListItem && !task.is_focused()));
        assert!(output.diagnostics.is_empty());
    }

    #[test]
    fn document_task_uses_shared_validation_and_rejects_recurrence() {
        let source = "`+ task\n`= title Project\n`= due bad\n`= priority bad\n`= done 2026-09-21T10:00:00+08:00\n`= recur P1D\n\nBody.\n\n`= focused 2026-09-21T09:00:00+08:00--\n";
        let parsed = parse(source);
        let green = plumb_syntax::GreenDocument::parse(source);
        let output = analyze_tasks(parsed.valid_syntax().unwrap());
        assert_eq!(output, analyze_green_tasks(green.valid_syntax().unwrap()));
        let codes = output
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();
        assert_eq!(
            codes,
            [
                "task.invalid-datetime",
                "task.invalid-priority",
                "task.focus-on-closed",
                "task.document-recur"
            ]
        );
        assert!(!output.document_task().unwrap().to_owned().is_focused());
    }

    #[test]
    fn duplicate_document_focus_properties_are_diagnosed_across_shards() {
        let source = "`+ task\n`= focused 2026-09-20T09:00:00+08:00--\n\nBody.\n\n`= focused 2026-09-21T09:00:00+08:00--\n";
        let parsed = parse(source);
        let green = plumb_syntax::GreenDocument::parse(source);
        let output = analyze_tasks(parsed.valid_syntax().unwrap());
        assert_eq!(output, analyze_green_tasks(green.valid_syntax().unwrap()));
        assert!(!output.document_task().unwrap().to_owned().focus_valid());
    }

    #[test]
    fn green_task_projection_matches_complete_analysis() {
        let source = "Prelude\n\n`- Parent\n `+ task\n `= due invalid\n\n `- Child\n  `+ task\n\n`- Sibling\n `+ task\n `= priority invalid\n";
        let parsed = parse(source);
        let green = plumb_syntax::GreenDocument::parse(source);
        assert_eq!(
            analyze_green_tasks(green.valid_syntax().unwrap()),
            analyze_tasks(parsed.valid_syntax().unwrap())
        );
    }

    #[test]
    fn collects_task_facets_fields_dependencies_and_nesting() {
        let source = "`- Write parser\n `+ task\n `@ write\n `= created 2026-07-20T09:00:00+08:00\n `= due 2026-07-21T09:00:00+08:00\n `= wait 2026-07-20T12:00:00+08:00\n `= recur P1W\n `= prev #old\n `= depends #draft other notes.plumb#review third.plumb#done\n\n `note Details\n\n `- Nested task\n  `+ task\n  `= done 2026-07-20T10:00:00+08:00\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_tasks(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(output.tasks.len(), 2);
        let task = &output.tasks.get(0).unwrap();
        assert_eq!(task.title, "Write parser");
        assert_eq!(task.depth, 0);
        assert_eq!(
            &parsed.source[task.attribute_insert..task.attribute_insert + 1],
            "\n"
        );
        assert_eq!(task.id.as_ref().unwrap().value, "write");
        assert_eq!(task.state(), TaskState::Open);
        assert_eq!(task.depends.len(), 3);
        assert_eq!(&parsed.source[task.depends[0].range.clone()], "#draft");
        assert!(matches!(
            task.depends[1].target,
            TaskReferenceTarget::External { ref path, ref id }
                if path == "other notes.plumb" && id == "review"
        ));
        assert_eq!(
            &parsed.source[task.depends[1].range.clone()],
            "other notes.plumb#review"
        );
        assert!(matches!(
            task.depends[2].target,
            TaskReferenceTarget::External { ref path, ref id }
                if path == "third.plumb" && id == "done"
        ));
        assert_eq!(output.tasks.get(1).unwrap().depth, 1);
        assert_eq!(output.tasks.get(1).unwrap().state(), TaskState::Done);
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
        let source =
            "`- Review\n `+ task\n `@ review\n `= depends Project Plan.plumb#build #local\n";
        let parsed = plumb_syntax::parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_tasks(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        let task = &output.tasks.get(0).unwrap();
        assert_eq!(task.depends.len(), 2);
        assert_eq!(
            &source[task.depends[0].range.clone()],
            "Project Plan.plumb#build"
        );
        assert_eq!(&source[task.depends[1].range.clone()], "#local");
    }

    #[test]
    fn reports_local_task_state_and_recurrence_diagnostics() {
        let source = "`- Conflict\n `+ task\n `= done 2026-07-20T09:00:00Z\n `= canceled 2026-07-20T10:00:00Z\n`- Invalid recurrence\n `+ task\n `= due not-a-date\n `= recur P1M1D\n`- Invalid datetimes\n `+ task\n `= created 2026-07-20T09:00:00Z\n `= wait tomorrow\n `= done later\n `= canceled never\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_tasks(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(output.tasks.get(0).unwrap().state(), TaskState::Conflicted);
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
        assert_eq!(output.tasks.get(1).unwrap().due, None);
        assert_eq!(
            output
                .tasks
                .get(2)
                .unwrap()
                .created
                .as_ref()
                .map(|field| field.value.as_str()),
            Some("2026-07-20T09:00:00Z")
        );
        assert_eq!(output.tasks.get(2).unwrap().state(), TaskState::Open);
    }

    #[test]
    fn parses_signed_task_priority_and_rejects_out_of_range_values() {
        let source = "`- Maximum\n `+ task\n `= priority 2147483647\n`- Deferred\n `+ task\n `= priority -12\n`- Minimum\n `+ task\n `= priority -2147483648\n`- Too large\n `+ task\n `= priority 2147483648\n`- Too small\n `+ task\n `= priority -2147483649\n`- Invalid\n `+ task\n `= priority soon\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

        let output = analyze_tasks(
            parsed
                .valid_syntax()
                .expect("semantic analysis requires valid syntax"),
        );
        assert_eq!(output.tasks.get(0).unwrap().priority, Some(i32::MAX));
        assert_eq!(output.tasks.get(1).unwrap().priority, Some(-12));
        assert_eq!(output.tasks.get(2).unwrap().priority, Some(i32::MIN));
        assert_eq!(output.tasks.get(3).unwrap().priority, None);
        assert_eq!(output.tasks.get(4).unwrap().priority, None);
        assert_eq!(output.tasks.get(5).unwrap().priority, None);
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
            "`- Missing due\n `+ task\n `= recur P1W\n`- Invalid due\n `+ task\n `= due invalid\n `= recur P1W\n";
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
    fn non_document_task_facets_require_list_items_and_specialized_markers_are_ordinary() {
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
        assert_eq!(
            output.diagnostics.get(0).unwrap().code,
            "facet.task-event-conflict"
        );
    }

    fn analyze(source: &str) -> (plumb_syntax::ParsedDocument, TaskOutput) {
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_tasks(parsed.valid_syntax().expect("valid syntax"));
        (parsed, output)
    }

    fn diagnostic_codes(output: &TaskOutput) -> Vec<&'static str> {
        output
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect()
    }

    #[test]
    fn parses_focus_history_in_all_three_shapes() {
        let source = "`- Leaf open\n `+ task\n `= focused 2026-09-20T09:00:00+08:00--\n\n`- Leaf closed\n `+ task\n `= focused 2026-09-20T09:00:00+08:00--2026-09-20T11:00:00+08:00\n\n`- List\n `+ task\n `= focused\n  `- 2026-09-20T09:00:00+08:00--2026-09-20T11:00:00+08:00\n  `- 2026-09-20T14:00:00+08:00--\n";
        let (parsed, output) = analyze(source);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(output.tasks.len(), 3);

        let leaf_open = &output.tasks.get(0).unwrap();
        assert!(leaf_open.is_focused());
        assert_eq!(leaf_open.focused_since(), Some("2026-09-20T09:00:00+08:00"));
        assert_eq!(leaf_open.focused.intervals.len(), 1);
        assert!(!leaf_open.focused.list_form);

        let leaf_closed = &output.tasks.get(1).unwrap();
        assert!(leaf_closed.focus_valid());
        assert!(!leaf_closed.is_focused());
        assert_eq!(leaf_closed.focused_since(), None);
        assert_eq!(
            leaf_closed.focused.intervals[0].end.as_deref(),
            Some("2026-09-20T11:00:00+08:00")
        );

        let list = &output.tasks.get(2).unwrap();
        assert!(list.focused.list_form);
        assert_eq!(list.focused.intervals.len(), 2);
        assert!(list.is_focused());
        assert_eq!(list.focused_since(), Some("2026-09-20T14:00:00+08:00"));
        assert_eq!(
            &parsed.source[list.focused.intervals[1].range.clone()],
            "2026-09-20T14:00:00+08:00--"
        );
    }

    #[test]
    fn accepts_blank_separated_interval_list_children() {
        // plumb-edit renders a non-empty declaration head with a blank line
        // before its first direct child, so the canonical spelling of a focus
        // history list separates the head from its interval children.
        let source = "`- Separated\n `+ task\n `= focused\n\n  `- 2026-09-20T09:00:00+08:00--2026-09-20T11:00:00+08:00\n  `- 2026-09-20T14:00:00+08:00--\n";
        let (_, output) = analyze(source);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let task = &output.tasks.get(0).unwrap();
        assert!(task.focused.list_form);
        assert_eq!(task.focused.intervals.len(), 2);
        assert!(task.is_focused());
        assert_eq!(task.focused_since(), Some("2026-09-20T14:00:00+08:00"));
    }

    #[test]
    fn accepts_adjacent_zero_length_and_offset_equivalent_intervals() {
        let source = "`- History\n `+ task\n `= focused\n  `- 2026-09-20T09:00:00+08:00--2026-09-20T11:00:00+08:00\n  `- 2026-09-20T03:00:00Z--2026-09-20T03:00:00Z\n  `- 2026-09-20T04:00:00Z--\n";
        let (_, output) = analyze(source);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let task = &output.tasks.get(0).unwrap();
        assert!(task.focus_valid());
        assert!(task.is_focused());
        assert_eq!(task.focused_since(), Some("2026-09-20T04:00:00Z"));
    }

    #[test]
    fn reports_invalid_focus_histories() {
        let cases = [
            (
                "`- Bad\n `+ task\n `= focused 2026-09-20T09:00:00+08:00\n",
                "task.invalid-focus",
            ),
            (
                "`- Bad\n `+ task\n `= focused nonsense--\n",
                "task.invalid-focus",
            ),
            (
                "`- Bad\n `+ task\n `= focused 2026-09-20T09:00:00+08:00--2026-09-20T08:00:00+08:00\n",
                "task.invalid-focus",
            ),
            ("`- Bad\n `+ task\n `= focused\n", "task.invalid-focus"),
            (
                "`- Bad\n `+ task\n `= focused\n  `- 2026-09-20T09:00:00+08:00--\n  `- 2026-09-20T10:00:00+08:00--\n",
                "task.multiple-open-focus",
            ),
            (
                "`- Bad\n `+ task\n `= focused\n  `- 2026-09-20T09:00:00+08:00--\n  `- 2026-09-20T10:00:00+08:00--2026-09-20T11:00:00+08:00\n",
                "task.multiple-open-focus",
            ),
            (
                "`- Bad\n `+ task\n `= focused\n  `- 2026-09-20T11:00:00+08:00--2026-09-20T12:00:00+08:00\n  `- 2026-09-20T09:00:00+08:00--2026-09-20T10:00:00+08:00\n",
                "task.unordered-focus",
            ),
            (
                "`- Bad\n `+ task\n `= focused\n  `- 2026-09-20T09:00:00+08:00--2026-09-20T11:00:00+08:00\n  `- 2026-09-20T10:00:00+08:00--2026-09-20T12:00:00+08:00\n",
                "task.overlapping-focus",
            ),
            (
                "`- Bad\n `+ task\n `= focused 2026-09-20T09:00:00+08:00--\n `= focused 2026-09-20T10:00:00+08:00--\n",
                "task.duplicate-focus",
            ),
        ];
        for (source, code) in cases {
            let (_, output) = analyze(source);
            assert!(
                diagnostic_codes(&output).contains(&code),
                "missing {code}: {:?}",
                output.diagnostics
            );
            assert!(output.tasks.get(0).unwrap().focused.invalid);
        }
    }

    #[test]
    fn closed_task_with_open_interval_is_diagnosed_and_not_focused() {
        let source = "`- Done but open\n `+ task\n `= done 2026-09-20T12:00:00+08:00\n `= focused 2026-09-20T09:00:00+08:00--\n";
        let (_, output) = analyze(source);
        assert!(diagnostic_codes(&output).contains(&"task.focus-on-closed"));
        let task = &output.tasks.get(0).unwrap();
        assert!(task.focus_valid());
        assert!(!task.is_focused());
        assert_eq!(task.focused_since(), None);
    }
}
