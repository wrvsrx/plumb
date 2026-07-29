use std::ops::Range;

use chrono::{DateTime, FixedOffset};
use plumb_core::{
    AttrItem, AttrValue, Block, Diagnostic, DiagnosticSeverity, Document, ParsedBlock,
};

use crate::tasks::{task_reference_fields, TaskDependency};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventField {
    pub value: String,
    pub range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRecord {
    pub range: Range<usize>,
    pub marker_range: Range<usize>,
    pub selection_range: Range<usize>,
    pub title: String,
    pub details: String,
    pub depth: usize,
    pub id: Option<EventField>,
    pub uid: Option<EventField>,
    pub start: Option<EventField>,
    pub end: Option<EventField>,
    pub tasks: Vec<TaskDependency>,
}

impl EventRecord {
    pub fn start_datetime(&self) -> Option<DateTime<FixedOffset>> {
        DateTime::parse_from_rfc3339(&self.start.as_ref()?.value).ok()
    }

    pub fn end_datetime(&self) -> Option<DateTime<FixedOffset>> {
        DateTime::parse_from_rfc3339(&self.end.as_ref()?.value).ok()
    }

    pub fn overlaps(&self, start: DateTime<FixedOffset>, end: DateTime<FixedOffset>) -> bool {
        let Some(event_start) = self.start_datetime() else {
            return false;
        };
        match self.end_datetime() {
            Some(event_end) if event_end > event_start => event_start < end && event_end > start,
            Some(_) => false,
            None => event_start >= start && event_start < end,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EventOutput {
    pub events: Vec<EventRecord>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn analyze_events(source: &str, document: &Document) -> EventOutput {
    let mut output = EventOutput::default();
    collect_blocks(source, &document.blocks, 0, &mut output);
    output
}

fn collect_blocks(source: &str, blocks: &[Block], event_depth: usize, output: &mut EventOutput) {
    for block in blocks {
        let Block::Parsed(block) = block else {
            continue;
        };
        let Some(mark) = &block.mark else {
            collect_blocks(source, &block.children, event_depth, output);
            continue;
        };
        let event_class = mark.attrs.items.iter().find_map(|item| match item {
            AttrItem::Class { value, range } if value == "event" => Some(range.clone()),
            _ => None,
        });
        let task_class = mark
            .attrs
            .items
            .iter()
            .any(|item| matches!(item, AttrItem::Class { value, .. } if value == "task"));
        let valid_owner = matches!(mark.marker.as_str(), "-" | ".");
        let is_event = event_class.is_some() && valid_owner && !task_class;

        if let Some(range) = event_class {
            if !valid_owner {
                output.diagnostics.push(Diagnostic {
                    code: "event.invalid-owner",
                    severity: DiagnosticSeverity::Warning,
                    message: "the '.event' facet is only valid on '-' and '.' list items"
                        .to_string(),
                    range,
                    related: Vec::new(),
                });
            } else if task_class {
                output.diagnostics.push(Diagnostic {
                    code: "event.conflicting-task-facet",
                    severity: DiagnosticSeverity::Warning,
                    message: "an event cannot also carry the '.task' facet".to_string(),
                    range,
                    related: Vec::new(),
                });
            }
        }

        if is_event {
            let event = event_record(source, block, event_depth);
            collect_event_diagnostics(&event, &mark.attrs.items, output);
            output.events.push(event);
        }
        collect_blocks(
            source,
            &block.children,
            event_depth + usize::from(is_event),
            output,
        );
    }
}

fn event_record(source: &str, block: &ParsedBlock, depth: usize) -> EventRecord {
    let mark = block.mark.as_ref().expect("event is a marked block");
    EventRecord {
        range: block.range.clone(),
        marker_range: mark.range.clone(),
        selection_range: block.head.range.clone(),
        title: block.head.plain_text().trim().to_string(),
        details: event_details(&block.children),
        depth,
        id: mark.attrs.items.iter().find_map(|item| match item {
            AttrItem::Id { value, range } => Some(EventField {
                value: value.clone(),
                range: range.start + 1..range.end,
            }),
            _ => None,
        }),
        uid: uid_field(&mark.attrs.items),
        start: datetime_field(&mark.attrs.items, "start"),
        end: datetime_field(&mark.attrs.items, "end"),
        tasks: task_reference_fields(source, &mark.attrs.items, "tasks"),
    }
}

fn event_details(blocks: &[Block]) -> String {
    let mut lines = Vec::new();
    collect_detail_lines(blocks, &mut lines);
    lines.join("\n")
}

fn collect_detail_lines(blocks: &[Block], lines: &mut Vec<String>) {
    for block in blocks {
        match block {
            Block::Parsed(block) => {
                if block.mark.as_ref().is_some_and(|mark| {
                    mark.attrs.items.iter().any(
                        |item| matches!(item, AttrItem::Class { value, .. } if value == "event"),
                    )
                }) {
                    continue;
                }
                let text = block.head.plain_text().trim().to_string();
                if !text.is_empty() {
                    lines.push(text);
                }
                collect_detail_lines(&block.children, lines);
            }
            Block::Verbatim(block) => {
                if !block.text.is_empty() {
                    lines.push(block.text.clone());
                }
            }
        }
    }
}

fn pair_value<'a>(items: &'a [AttrItem], wanted: &str) -> Option<&'a AttrValue> {
    items.iter().find_map(|item| match item {
        AttrItem::Pair { key, value, .. } if key == wanted => Some(value),
        _ => None,
    })
}

fn uid_field(items: &[AttrItem]) -> Option<EventField> {
    let value = pair_value(items, "uid")?;
    (value.quoted && !value.decoded.is_empty()).then(|| event_field(value))
}

fn datetime_field(items: &[AttrItem], key: &str) -> Option<EventField> {
    let value = pair_value(items, key)?;
    (value.quoted && DateTime::parse_from_rfc3339(&value.decoded).is_ok())
        .then(|| event_field(value))
}

fn event_field(value: &AttrValue) -> EventField {
    EventField {
        value: value.decoded.clone(),
        range: value.range.clone(),
    }
}

fn collect_event_diagnostics(event: &EventRecord, attrs: &[AttrItem], output: &mut EventOutput) {
    for key in ["start", "end"] {
        match pair_value(attrs, key) {
            Some(value)
                if !value.quoted || DateTime::parse_from_rfc3339(&value.decoded).is_err() =>
            {
                output.diagnostics.push(Diagnostic {
                    code: "event.invalid-datetime",
                    severity: DiagnosticSeverity::Warning,
                    message: format!("'{key}' must be a quoted RFC 3339 timestamp"),
                    range: value.range.clone(),
                    related: Vec::new(),
                });
            }
            None if key == "start" => output.diagnostics.push(Diagnostic {
                code: "event.missing-start",
                severity: DiagnosticSeverity::Warning,
                message: "an event requires a 'start' timestamp".to_string(),
                range: event.selection_range.clone(),
                related: Vec::new(),
            }),
            _ => {}
        }
    }

    if let Some(uid) = pair_value(attrs, "uid") {
        if !uid.quoted || uid.decoded.is_empty() {
            output.diagnostics.push(Diagnostic {
                code: "event.invalid-uid",
                severity: DiagnosticSeverity::Warning,
                message: "'uid' must be a nonempty quoted value".to_string(),
                range: uid.range.clone(),
                related: Vec::new(),
            });
        }
    }

    if let (Some(start), Some(end)) = (event.start_datetime(), event.end_datetime()) {
        if end <= start {
            output.diagnostics.push(Diagnostic {
                code: "event.invalid-interval",
                severity: DiagnosticSeverity::Warning,
                message: "event 'end' must be later than 'start'".to_string(),
                range: event.end.as_ref().expect("parsed end exists").range.clone(),
                related: vec![event
                    .start
                    .as_ref()
                    .expect("parsed start exists")
                    .range
                    .clone()],
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use plumb_core::parse;

    use super::*;

    #[test]
    fn collects_event_facets_ranges_and_task_references() {
        let source = "`-{.event #review uid=\"review@example\" start=\"2026-07-30T14:00:00+08:00\" end=\"2026-07-30T15:30:00+08:00\" tasks=\"#local Project A.plumb#remote\"} Review\n  `note Details\n  `-{.event start=\"2026-07-31T09:00:00+08:00\"} Follow-up\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_events(source, &parsed.syntax);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(output.events.len(), 2);
        let event = &output.events[0];
        assert_eq!(event.title, "Review");
        assert_eq!(event.details, "Details");
        assert_eq!(event.id.as_ref().unwrap().value, "review");
        assert_eq!(event.uid.as_ref().unwrap().value, "review@example");
        assert_eq!(event.tasks.len(), 2);
        assert_eq!(
            &source[event.tasks[1].range.clone()],
            "Project A.plumb#remote"
        );
        assert_eq!(output.events[1].depth, 1);
    }

    #[test]
    fn diagnoses_invalid_owners_fields_intervals_and_conflicts() {
        let source = "`node{.event start=\"2026-07-30T10:00:00Z\"} Wrong owner\n`-{.event .task start=\"2026-07-30T10:00:00Z\"} Conflict\n`-{.event uid=\"\"} Missing start\n`-{.event start=tomorrow end=\"invalid\"} Invalid dates\n`-{.event start=\"2026-07-30T11:00:00Z\" end=\"2026-07-30T10:00:00Z\"} Backward\n";
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let output = analyze_events(source, &parsed.syntax);
        assert_eq!(output.events.len(), 3);
        assert!(output.events[0].uid.is_none());
        assert_eq!(
            output
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            [
                "event.invalid-owner",
                "event.conflicting-task-facet",
                "event.missing-start",
                "event.invalid-uid",
                "event.invalid-datetime",
                "event.invalid-datetime",
                "event.invalid-interval",
            ]
        );
    }

    #[test]
    fn excludes_unquoted_uid_from_event_records() {
        let source = "`-{.event uid=calendar start=\"2026-07-30T10:00:00Z\"} Review\n";
        let parsed = parse(source);
        let output = analyze_events(source, &parsed.syntax);
        assert!(output.events[0].uid.is_none());
        assert_eq!(output.diagnostics[0].code, "event.invalid-uid");
    }

    #[test]
    fn overlap_uses_half_open_ranges_and_point_events() {
        let parsed = parse("`-{.event start=\"2026-07-30T10:00:00Z\" end=\"2026-07-30T11:00:00Z\"} Range\n`-{.event start=\"2026-07-30T11:00:00Z\"} Point\n");
        let output = analyze_events(&parsed.source, &parsed.syntax);
        let start = DateTime::parse_from_rfc3339("2026-07-30T10:30:00Z").unwrap();
        let end = DateTime::parse_from_rfc3339("2026-07-30T11:00:00Z").unwrap();
        assert!(output.events[0].overlaps(start, end));
        assert!(!output.events[1].overlaps(start, end));
    }
}
