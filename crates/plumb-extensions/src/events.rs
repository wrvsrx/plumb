use std::ops::Range;

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveTime, TimeZone};
use plumb_core::{
    AttrItem, AttrValue, Block, Diagnostic, DiagnosticSeverity, Document, ParsedBlock,
};

use crate::tasks::{task_reference_fields, TaskDependency};
use crate::{MetadataOutput, MetadataValue};

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
    pub date: Option<EventField>,
    pub timezone: Option<EventField>,
    pub when: Option<EventField>,
    pub at: Option<EventField>,
    pub start: Option<EventField>,
    pub end: Option<EventField>,
    pub tasks: Vec<TaskDependency>,
}

impl EventRecord {
    pub fn at_datetime(&self) -> Option<DateTime<FixedOffset>> {
        DateTime::parse_from_rfc3339(&self.at.as_ref()?.value).ok()
    }

    pub fn start_datetime(&self) -> Option<DateTime<FixedOffset>> {
        DateTime::parse_from_rfc3339(&self.start.as_ref()?.value).ok()
    }

    pub fn end_datetime(&self) -> Option<DateTime<FixedOffset>> {
        DateTime::parse_from_rfc3339(&self.end.as_ref()?.value).ok()
    }

    pub fn sort_datetime(&self) -> Option<DateTime<FixedOffset>> {
        self.at_datetime().or_else(|| self.start_datetime())
    }

    pub fn is_point(&self) -> bool {
        self.at.is_some() && self.start.is_none() && self.end.is_none()
    }

    pub fn is_running(&self) -> bool {
        self.at.is_none() && self.start.is_some() && self.end.is_none()
    }

    pub fn overlaps(&self, start: DateTime<FixedOffset>, end: DateTime<FixedOffset>) -> bool {
        if let Some(at) = self.at_datetime() {
            return at >= start && at < end;
        }
        let Some(event_start) = self.start_datetime() else {
            return false;
        };
        match self.end_datetime() {
            Some(event_end) if event_end > event_start => event_start < end && event_end > start,
            Some(_) => false,
            None => event_start < end,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EventOutput {
    pub events: Vec<EventRecord>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn analyze_events(source: &str, document: &Document, metadata: &MetadataOutput) -> EventOutput {
    let mut output = EventOutput::default();
    let context = EventContext::from_metadata(metadata);
    collect_blocks(source, &document.blocks, 0, &context, &mut output);
    output
}

#[derive(Clone, Default)]
struct EventContext {
    date: Option<String>,
    timezone: Option<String>,
}

impl EventContext {
    fn from_metadata(metadata: &MetadataOutput) -> Self {
        let scalar = |key: &str| {
            let entry = metadata
                .metadata
                .as_ref()?
                .entries
                .iter()
                .find(|entry| entry.key == key)?;
            match &entry.value {
                MetadataValue::Scalar { content, .. } => Some(content.plain_text()),
                MetadataValue::Verbatim { text, .. } => Some(text.clone()),
                _ => None,
            }
        };
        Self {
            date: scalar("date"),
            timezone: scalar("timezone"),
        }
    }

    fn with_attributes(&self, attrs: &[AttrItem]) -> Self {
        Self {
            date: pair_value(attrs, "date")
                .map_or_else(|| self.date.clone(), |value| Some(value.decoded.clone())),
            timezone: pair_value(attrs, "timezone").map_or_else(
                || self.timezone.clone(),
                |value| Some(value.decoded.clone()),
            ),
        }
    }
}

fn collect_blocks(
    source: &str,
    blocks: &[Block],
    event_depth: usize,
    context: &EventContext,
    output: &mut EventOutput,
) {
    for block in blocks {
        let Block::Parsed(block) = block else {
            continue;
        };
        let Some(mark) = &block.mark else {
            collect_blocks(source, &block.children, event_depth, context, output);
            continue;
        };
        let scoped_context = context.with_attributes(&mark.attrs.items);
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
            let event = event_record(source, block, event_depth, &scoped_context);
            collect_event_diagnostics(&event, &mark.attrs.items, &scoped_context, output);
            output.events.push(event);
        }
        collect_blocks(
            source,
            &block.children,
            event_depth + usize::from(is_event),
            &scoped_context,
            output,
        );
    }
}

fn event_record(
    source: &str,
    block: &ParsedBlock,
    depth: usize,
    context: &EventContext,
) -> EventRecord {
    let mark = block.mark.as_ref().expect("event is a marked block");
    let date = text_field(&mark.attrs.items, "date");
    let timezone = text_field(&mark.attrs.items, "timezone");
    let when = quoted_field(&mark.attrs.items, "when");
    let resolved = resolve_when(
        when.as_ref(),
        date.as_ref()
            .map_or(context.date.as_deref(), |field| Some(&field.value)),
        timezone
            .as_ref()
            .map_or(context.timezone.as_deref(), |field| Some(&field.value)),
    );
    let (at, start, end) = match resolved {
        Ok(ResolvedWhen::Point(value)) => (Some(resolved_field(value, &when)), None, None),
        Ok(ResolvedWhen::Interval(start, end)) => (
            None,
            Some(resolved_field(start, &when)),
            Some(resolved_field(end, &when)),
        ),
        Err(_) => (None, None, None),
    };
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
        date,
        timezone,
        when,
        at,
        start,
        end,
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

fn quoted_field(items: &[AttrItem], key: &str) -> Option<EventField> {
    let value = pair_value(items, key)?;
    value.quoted.then(|| event_field(value))
}

fn text_field(items: &[AttrItem], key: &str) -> Option<EventField> {
    pair_value(items, key).map(event_field)
}

fn event_field(value: &AttrValue) -> EventField {
    EventField {
        value: value.decoded.clone(),
        range: value.range.clone(),
    }
}

fn collect_event_diagnostics(
    event: &EventRecord,
    attrs: &[AttrItem],
    context: &EventContext,
    output: &mut EventOutput,
) {
    let when = pair_value(attrs, "when");
    if when.is_none() {
        output.diagnostics.push(Diagnostic {
            code: "event.missing-time",
            severity: DiagnosticSeverity::Warning,
            message: "an event requires a quoted 'when' schedule".to_string(),
            range: event.selection_range.clone(),
            related: Vec::new(),
        });
    }
    if when.is_some_and(|when| !when.quoted) {
        let when = when.expect("unquoted when exists");
        output.diagnostics.push(Diagnostic {
            code: "event.invalid-when",
            severity: DiagnosticSeverity::Warning,
            message: "'when' must be a quoted reduced-precision time or interval".to_string(),
            range: when.range.clone(),
            related: Vec::new(),
        });
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

    let date = event.date.as_ref().map(|field| field.value.as_str());
    let timezone = event.timezone.as_ref().map(|field| field.value.as_str());
    let result = resolve_when(
        event.when.as_ref(),
        date.or(context.date.as_deref()),
        timezone.or(context.timezone.as_deref()),
    );
    if event.at.is_none() && event.start.is_none() && when.is_some_and(|when| when.quoted) {
        let when = when.expect("quoted when exists");
        let code = match result {
            Err(EventWhenError::MissingDate) => "event.missing-date-context",
            Err(EventWhenError::InvalidDate) => "event.invalid-date",
            Err(EventWhenError::MissingTimezone) => "event.missing-timezone-context",
            Err(EventWhenError::InvalidTimezone) => "event.invalid-timezone",
            Err(EventWhenError::InvalidInterval) => "event.invalid-interval",
            Err(EventWhenError::InvalidWhen) | Ok(_) => "event.invalid-when",
        };
        output.diagnostics.push(Diagnostic {
            code,
            severity: DiagnosticSeverity::Warning,
            message: "event schedule cannot be resolved from 'when', date, and timezone"
                .to_string(),
            range: when.range.clone(),
            related: Vec::new(),
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventWhenError {
    MissingDate,
    InvalidDate,
    MissingTimezone,
    InvalidTimezone,
    InvalidWhen,
    InvalidInterval,
}

enum ResolvedWhen {
    Point(DateTime<FixedOffset>),
    Interval(DateTime<FixedOffset>, DateTime<FixedOffset>),
}

fn resolve_when(
    when: Option<&EventField>,
    date: Option<&str>,
    timezone: Option<&str>,
) -> Result<ResolvedWhen, EventWhenError> {
    let when = when.ok_or(EventWhenError::InvalidWhen)?;
    let date = date.ok_or(EventWhenError::MissingDate)?;
    let (date, inherited_offset) = match NaiveDate::parse_from_str(date, "%Y-%m-%d") {
        Ok(date) => (date, None),
        Err(_) => {
            let datetime =
                DateTime::parse_from_rfc3339(date).map_err(|_| EventWhenError::InvalidDate)?;
            (datetime.date_naive(), Some(*datetime.offset()))
        }
    };
    let offset = match timezone {
        Some(timezone) => DateTime::parse_from_rfc3339(&format!("2000-01-01T00:00:00{timezone}"))
            .map_err(|_| EventWhenError::InvalidTimezone)?
            .offset()
            .to_owned(),
        None => inherited_offset.ok_or(EventWhenError::MissingTimezone)?,
    };
    let (start, end) = match when.value.split_once("--") {
        Some((start, end)) if !start.is_empty() && !end.is_empty() && !end.contains("--") => {
            (start, Some(end))
        }
        Some(_) => return Err(EventWhenError::InvalidWhen),
        None => (when.value.as_str(), None),
    };
    let start = local_datetime(date, parse_time(start)?, offset)?;
    let Some(end) = end else {
        return Ok(ResolvedWhen::Point(start));
    };
    let end_time = parse_time(end)?;
    if end_time == start.time() {
        return Err(EventWhenError::InvalidInterval);
    }
    let end_date = if end_time < start.time() {
        date.succ_opt().ok_or(EventWhenError::InvalidInterval)?
    } else {
        date
    };
    let end = local_datetime(end_date, end_time, offset)?;
    Ok(ResolvedWhen::Interval(start, end))
}

fn parse_time(value: &str) -> Result<NaiveTime, EventWhenError> {
    let parts = value.split(':').collect::<Vec<_>>();
    if !(1..=3).contains(&parts.len())
        || parts
            .iter()
            .any(|part| part.len() != 2 || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(EventWhenError::InvalidWhen);
    }
    let component = |index: usize| {
        parts
            .get(index)
            .map_or(Ok(0), |part| part.parse::<u32>())
            .map_err(|_| EventWhenError::InvalidWhen)
    };
    NaiveTime::from_hms_opt(component(0)?, component(1)?, component(2)?)
        .ok_or(EventWhenError::InvalidWhen)
}

fn local_datetime(
    date: NaiveDate,
    time: NaiveTime,
    offset: FixedOffset,
) -> Result<DateTime<FixedOffset>, EventWhenError> {
    offset
        .from_local_datetime(&date.and_time(time))
        .single()
        .ok_or(EventWhenError::InvalidWhen)
}

fn resolved_field(value: DateTime<FixedOffset>, when: &Option<EventField>) -> EventField {
    EventField {
        value: value.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        range: when.as_ref().expect("resolved when exists").range.clone(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use plumb_core::parse;

    use super::*;

    fn analyze(source: &str) -> EventOutput {
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let metadata = crate::analyze_metadata(&parsed.syntax);
        analyze_events(source, &parsed.syntax, &metadata)
    }

    #[test]
    fn collects_event_facets_ranges_and_task_references() {
        let source = "`meta\n  `: date\n\n    2026-07-30\n\n  `: timezone\n\n    +08:00\n\n`-{.event #review uid=\"review@example\" when=\"14:00--15:30\" tasks=\"#local Project A.plumb#remote\"} Review\n  `note Details\n  `-{.event date=2026-07-31 when=\"09:00\"} Follow-up\n";
        let output = analyze(source);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(output.events.len(), 2);
        let event = &output.events[0];
        assert_eq!(event.title, "Review");
        assert_eq!(event.details, "Details");
        assert_eq!(event.id.as_ref().unwrap().value, "review");
        assert_eq!(event.uid.as_ref().unwrap().value, "review@example");
        assert_eq!(
            event.start_datetime().unwrap().to_rfc3339(),
            "2026-07-30T14:00:00+08:00"
        );
        assert_eq!(
            output.events[1].at_datetime().unwrap().to_rfc3339(),
            "2026-07-31T09:00:00+08:00"
        );
        assert_eq!(event.tasks.len(), 2);
        assert_eq!(
            &source[event.tasks[1].range.clone()],
            "Project A.plumb#remote"
        );
        assert_eq!(output.events[1].depth, 1);
    }

    #[test]
    fn diagnoses_invalid_owners_fields_intervals_and_conflicts() {
        let source = "`meta\n  `: date\n\n    2026-07-30\n\n  `: timezone\n\n    +08:00\n\n`node{.event when=\"10:00\"} Wrong owner\n`-{.event .task when=\"10:00\"} Conflict\n`-{.event uid=\"\"} Missing time\n`-{.event when=tomorrow} Invalid when\n`-{.event when=\"11:00--11:00\"} Empty interval\n";
        let output = analyze(source);
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
                "event.missing-time",
                "event.invalid-uid",
                "event.invalid-when",
                "event.invalid-interval",
            ]
        );
    }

    #[test]
    fn excludes_unquoted_uid_from_event_records() {
        let source = "`meta\n  `: date\n\n    2026-07-30\n\n  `: timezone\n\n    +00:00\n\n`-{.event uid=calendar when=\"10:00\"} Review\n";
        let output = analyze(source);
        assert!(output.events[0].uid.is_none());
        assert_eq!(output.diagnostics[0].code, "event.invalid-uid");
    }

    #[test]
    fn overlap_uses_half_open_ranges_and_point_events() {
        let source = "`meta\n  `: date\n\n    2026-07-30\n\n  `: timezone\n\n    +00:00\n\n`-{.event when=\"10:00--11:00\"} Range\n`-{.event when=\"11:00\"} Point\n`-{.event when=\"23:40--00:00\"} Cross midnight\n";
        let output = analyze(source);
        let start = DateTime::parse_from_rfc3339("2026-07-30T10:30:00Z").unwrap();
        let end = DateTime::parse_from_rfc3339("2026-07-30T11:00:00Z").unwrap();
        assert!(output.events[0].overlaps(start, end));
        assert!(!output.events[1].overlaps(start, end));
        assert!(output.events[1].is_point());
        assert!(!output.events[0].is_running());
        assert_eq!(
            output.events[2].end_datetime().unwrap().to_rfc3339(),
            "2026-07-31T00:00:00+00:00"
        );
    }

    #[test]
    fn old_datetime_fields_do_not_define_event_time() {
        let source = "`-{.event at=\"2026-07-30T10:00:00Z\"} Old\n";
        let output = analyze(source);
        assert!(output.events[0].sort_datetime().is_none());
        assert_eq!(output.diagnostics[0].code, "event.missing-time");
    }

    #[test]
    fn rfc3339_metadata_date_supplies_date_and_offset() {
        let source = "`meta\n  `: date\n\n    `[2026-07-30T09:15:00+08:00]\n\n`-{.event when=\"23:40--00:00\"} Cross midnight\n";
        let output = analyze(source);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(
            output.events[0].start_datetime().unwrap().to_rfc3339(),
            "2026-07-30T23:40:00+08:00"
        );
        assert_eq!(
            output.events[0].end_datetime().unwrap().to_rfc3339(),
            "2026-07-31T00:00:00+08:00"
        );
    }

    #[test]
    fn date_and_timezone_context_follow_tree_scope() {
        let source = "`meta\n  `: date\n\n    2026-07-30\n\n  `: timezone\n\n    +08:00\n\n`div{date=2026-07-31}\n  `-{.event when=\"09:00\"} Inherited date\n  `-{.event timezone=\"+09:00\" when=\"10:00\"} Timezone override\n    `-{.event when=\"11:00\"} Nested inheritance\n`-{.event when=\"12:00\"} Root sibling\n";
        let output = analyze(source);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(
            output
                .events
                .iter()
                .map(|event| event.sort_datetime().unwrap().to_rfc3339())
                .collect::<Vec<_>>(),
            [
                "2026-07-31T09:00:00+08:00",
                "2026-07-31T10:00:00+09:00",
                "2026-07-31T11:00:00+09:00",
                "2026-07-30T12:00:00+08:00",
            ]
        );
    }
}
