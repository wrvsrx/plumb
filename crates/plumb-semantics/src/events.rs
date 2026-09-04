use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveTime, TimeZone};
use plumb_syntax::{
    AttrItem, AttrValue, Block, Diagnostic, DiagnosticSeverity, GreenDocument, GreenShard, Inline,
    ParsedBlock, ValidDocument,
};
use serde::{Deserialize, Serialize};

use crate::tasks::{task_reference_fields, TaskDependency};
use crate::text::plain_text;
use crate::{MetadataOutput, MetadataValue};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventField {
    pub value: String,
    pub range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    pub tasks_override: bool,
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
    pub events: EventRecords,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone)]
pub struct EventRecords {
    storage: EventRecordStorage,
}

#[derive(Debug, Clone, Copy)]
pub struct EventRecordView<'a> {
    record: &'a EventRecord,
    offset: isize,
}

impl<'a> EventRecordView<'a> {
    pub fn range(self) -> Range<usize> {
        shifted_range(&self.record.range, self.offset)
    }

    pub fn selection_range(self) -> Range<usize> {
        shifted_range(&self.record.selection_range, self.offset)
    }

    pub fn title(self) -> &'a str {
        &self.record.title
    }

    pub fn id_value(self) -> Option<&'a str> {
        self.record.id.as_ref().map(|id| id.value.as_str())
    }

    pub fn depth(self) -> usize {
        self.record.depth
    }

    pub fn overlaps(self, start: DateTime<FixedOffset>, end: DateTime<FixedOffset>) -> bool {
        self.record.overlaps(start, end)
    }

    pub fn sort_datetime(self) -> Option<DateTime<FixedOffset>> {
        self.record.sort_datetime()
    }

    pub fn to_owned(self) -> EventRecord {
        let mut record = self.record.clone();
        shift_events(std::slice::from_mut(&mut record), self.offset);
        record
    }
}

#[derive(Debug, Clone)]
enum EventRecordStorage {
    Owned(Vec<EventRecord>),
    Green(Arc<GreenEventRevision>),
}

impl Default for EventRecords {
    fn default() -> Self {
        Self {
            storage: EventRecordStorage::Owned(Vec::new()),
        }
    }
}

impl PartialEq for EventRecords {
    fn eq(&self, other: &Self) -> bool {
        self.iter().eq(other.iter())
    }
}

impl Eq for EventRecords {}

impl EventRecords {
    pub fn len(&self) -> usize {
        match &self.storage {
            EventRecordStorage::Owned(events) => events.len(),
            EventRecordStorage::Green(revision) => revision.event_count(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, index: usize) -> Option<EventRecord> {
        self.iter().nth(index)
    }

    pub fn iter(&self) -> Box<dyn Iterator<Item = EventRecord> + '_> {
        match &self.storage {
            EventRecordStorage::Owned(events) => Box::new(events.iter().cloned()),
            EventRecordStorage::Green(revision) => Box::new(revision.events()),
        }
    }

    pub fn views(&self) -> Box<dyn Iterator<Item = EventRecordView<'_>> + '_> {
        match &self.storage {
            EventRecordStorage::Owned(events) => Box::new(
                events
                    .iter()
                    .map(|record| EventRecordView { record, offset: 0 }),
            ),
            EventRecordStorage::Green(revision) => Box::new(revision.event_views()),
        }
    }

    pub fn ranges(&self) -> Box<dyn Iterator<Item = Range<usize>> + '_> {
        match &self.storage {
            EventRecordStorage::Owned(events) => {
                Box::new(events.iter().map(|event| event.range.clone()))
            }
            EventRecordStorage::Green(revision) => Box::new(revision.event_ranges()),
        }
    }

    fn push(&mut self, event: EventRecord) {
        match &mut self.storage {
            EventRecordStorage::Owned(events) => events.push(event),
            EventRecordStorage::Green(_) => panic!("cannot append to an analyzed green revision"),
        }
    }

    fn append_owned(&mut self, events: &mut Vec<EventRecord>) {
        match &mut self.storage {
            EventRecordStorage::Owned(current) => current.append(events),
            EventRecordStorage::Green(_) => panic!("cannot append to an analyzed green revision"),
        }
    }

    fn from_green(revision: Arc<GreenEventRevision>) -> Self {
        Self {
            storage: EventRecordStorage::Green(revision),
        }
    }

    fn from_owned(events: Vec<EventRecord>) -> Self {
        Self {
            storage: EventRecordStorage::Owned(events),
        }
    }
}

impl<'a> IntoIterator for &'a EventRecords {
    type Item = EventRecord;
    type IntoIter = Box<dyn Iterator<Item = EventRecord> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl EventOutput {
    pub fn from_green(revision: Arc<GreenEventRevision>) -> Self {
        let diagnostics = revision.diagnostics();
        Self {
            events: EventRecords::from_green(revision),
            diagnostics,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GreenEventRevision {
    context: EventContext,
    shards: Vec<GreenEventShard>,
    index: HashMap<usize, usize>,
    cache_hits: usize,
}

#[derive(Debug, Clone)]
struct GreenEventShard {
    _syntax: Arc<GreenShard>,
    offset: usize,
    output: Arc<EventOutput>,
}

impl GreenEventRevision {
    pub fn from_output(
        document: &GreenDocument,
        metadata: &MetadataOutput,
        output: &EventOutput,
    ) -> Option<Self> {
        if !document.is_valid() {
            return None;
        }
        let mut index = HashMap::with_capacity(document.shards().len());
        let output_events = output.events.iter().collect::<Vec<_>>();
        let mut event_index = 0;
        let mut diagnostic_index = 0;
        let mut shards = Vec::with_capacity(document.shards().len());
        for (shard_index, view) in document.shards().enumerate() {
            let syntax = Arc::clone(view.shard());
            index.insert(Arc::as_ptr(&syntax) as usize, shard_index);
            let range = view.range();
            let mut events = Vec::new();
            while output_events
                .get(event_index)
                .is_some_and(|event| event.range.start < range.end)
            {
                let event = &output_events[event_index];
                if event.range.start < range.start || event.range.end > range.end {
                    return None;
                }
                events.push(event.clone());
                event_index += 1;
            }
            shift_events(&mut events, -(range.start as isize));
            let mut diagnostics = Vec::new();
            while output
                .diagnostics
                .get(diagnostic_index)
                .is_some_and(|diagnostic| diagnostic.range.start < range.end)
            {
                let diagnostic = &output.diagnostics[diagnostic_index];
                if diagnostic.range.start < range.start || diagnostic.range.end > range.end {
                    return None;
                }
                diagnostics.push(diagnostic.clone());
                diagnostic_index += 1;
            }
            shift_diagnostics(&mut diagnostics, -(range.start as isize));
            shards.push(GreenEventShard {
                _syntax: syntax,
                offset: range.start,
                output: Arc::new(EventOutput {
                    events: EventRecords::from_owned(events),
                    diagnostics,
                }),
            });
        }
        if event_index != output_events.len() || diagnostic_index != output.diagnostics.len() {
            return None;
        }
        Some(Self {
            context: EventContext::from_metadata(metadata),
            shards,
            index,
            cache_hits: 0,
        })
    }

    pub fn analyze(
        document: &GreenDocument,
        metadata: &MetadataOutput,
        previous: Option<&Self>,
    ) -> Option<Self> {
        if !document.is_valid() {
            return None;
        }
        let context = EventContext::from_metadata(metadata);
        let reusable = previous.filter(|previous| previous.context == context);
        let mut cache_hits = 0;
        let mut index = HashMap::with_capacity(document.shards().len());
        let shards = document
            .shards()
            .enumerate()
            .map(|(shard_index, view)| {
                let syntax = Arc::clone(view.shard());
                let identity = Arc::as_ptr(&syntax) as usize;
                let output = reusable
                    .and_then(|previous| {
                        previous
                            .index
                            .get(&identity)
                            .map(|index| Arc::clone(&previous.shards[*index].output))
                    })
                    .map(|output| {
                        cache_hits += 1;
                        output
                    })
                    .unwrap_or_else(|| {
                        Arc::new(analyze_events(
                            syntax
                                .parsed()
                                .valid_syntax()
                                .expect("valid green document has valid shards"),
                            metadata,
                        ))
                    });
                index.insert(identity, shard_index);
                GreenEventShard {
                    _syntax: syntax,
                    offset: view.offset(),
                    output,
                }
            })
            .collect();
        Some(Self {
            context,
            shards,
            index,
            cache_hits,
        })
    }

    pub fn cache_hits(&self) -> usize {
        self.cache_hits
    }

    pub fn shard_count(&self) -> usize {
        self.shards.len()
    }

    pub fn materialize(&self) -> EventOutput {
        let mut output = EventOutput::default();
        for shard in &self.shards {
            let mut local = (*shard.output).clone();
            shift_event_output(&mut local, shard.offset as isize);
            let mut events = local.events.iter().collect::<Vec<_>>();
            output.events.append_owned(&mut events);
            output.diagnostics.append(&mut local.diagnostics);
        }
        output
            .diagnostics
            .sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
        output
    }

    pub fn events(&self) -> impl Iterator<Item = EventRecord> + '_ {
        self.event_views().map(EventRecordView::to_owned)
    }

    pub fn event_views(&self) -> impl Iterator<Item = EventRecordView<'_>> + '_ {
        self.shards.iter().flat_map(|shard| {
            shard
                .output
                .events
                .views()
                .map(move |view| EventRecordView {
                    record: view.record,
                    offset: view.offset + shard.offset as isize,
                })
        })
    }

    pub fn event_ranges(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        self.shards.iter().flat_map(|shard| {
            shard.output.events.ranges().map(move |mut range| {
                shift_range(&mut range, shard.offset as isize);
                range
            })
        })
    }

    pub fn event_count(&self) -> usize {
        self.shards
            .iter()
            .map(|shard| shard.output.events.len())
            .sum()
    }

    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        for shard in &self.shards {
            let mut local = shard.output.diagnostics.clone();
            shift_diagnostics(&mut local, shard.offset as isize);
            diagnostics.append(&mut local);
        }
        diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
        diagnostics
    }
}

pub fn analyze_events(valid: ValidDocument<'_>, metadata: &MetadataOutput) -> EventOutput {
    let source = valid.source();
    let document = valid.syntax();
    let mut output = EventOutput::default();
    let context = EventContext::from_metadata(metadata);
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
            &context,
            &table_items,
            &mut output,
        );
    }
    output
}

pub(crate) fn analyze_events_incremental(
    valid: ValidDocument<'_>,
    metadata: &MetadataOutput,
    previous_metadata: &MetadataOutput,
    previous: &EventOutput,
    old_range: &Range<usize>,
    new_range: &Range<usize>,
) -> EventOutput {
    if EventContext::from_metadata(metadata) != EventContext::from_metadata(previous_metadata)
        || (old_range.start == 0
            && old_range.end == valid.source().len()
            && new_range.start == 0
            && new_range.end == valid.source().len())
    {
        return analyze_events(valid, metadata);
    }
    let source = valid.source();
    let fragment = plumb_syntax::parse(source[new_range.clone()].to_string());
    let Some(fragment) = fragment.valid_syntax() else {
        return analyze_events(valid, metadata);
    };
    let mut changed = analyze_events(fragment, metadata);
    shift_event_output(&mut changed, new_range.start as isize);
    let suffix_delta = new_range.end as isize - old_range.end as isize;

    let mut output = EventOutput::default();
    let mut prefix_events = previous
        .events
        .iter()
        .filter(|event| event.range.end <= old_range.start)
        .collect::<Vec<_>>();
    output.events.append_owned(&mut prefix_events);
    output
        .events
        .append_owned(&mut changed.events.iter().collect::<Vec<_>>());
    let mut suffix_events = previous
        .events
        .iter()
        .filter(|event| event.range.start >= old_range.end)
        .collect::<Vec<_>>();
    shift_events(&mut suffix_events, suffix_delta);
    output.events.append_owned(&mut suffix_events);

    output.diagnostics.extend(
        previous
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.range.end <= old_range.start)
            .cloned(),
    );
    output.diagnostics.append(&mut changed.diagnostics);
    let mut suffix_diagnostics = previous
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.range.start >= old_range.end)
        .cloned()
        .collect::<Vec<_>>();
    shift_diagnostics(&mut suffix_diagnostics, suffix_delta);
    output.diagnostics.append(&mut suffix_diagnostics);
    output
        .diagnostics
        .sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
    output
}

fn shift_event_output(output: &mut EventOutput, delta: isize) {
    let mut events = output.events.iter().collect::<Vec<_>>();
    shift_events(&mut events, delta);
    output.events = EventRecords::from_owned(events);
    shift_diagnostics(&mut output.diagnostics, delta);
}

fn shift_events(events: &mut [EventRecord], delta: isize) {
    for event in events {
        shift_range(&mut event.range, delta);
        shift_range(&mut event.marker_range, delta);
        shift_range(&mut event.selection_range, delta);
        for field in [
            &mut event.id,
            &mut event.uid,
            &mut event.date,
            &mut event.timezone,
            &mut event.when,
            &mut event.at,
            &mut event.start,
            &mut event.end,
        ] {
            if let Some(field) = field {
                shift_range(&mut field.range, delta);
            }
        }
        for task in &mut event.tasks {
            shift_range(&mut task.range, delta);
        }
    }
}

fn shift_diagnostics(diagnostics: &mut [Diagnostic], delta: isize) {
    for diagnostic in diagnostics {
        shift_range(&mut diagnostic.range, delta);
        for related in &mut diagnostic.related {
            shift_range(related, delta);
        }
    }
}

fn shift_range(range: &mut Range<usize>, delta: isize) {
    range.start = range.start.checked_add_signed(delta).unwrap();
    range.end = range.end.checked_add_signed(delta).unwrap();
}

fn shifted_range(range: &Range<usize>, delta: isize) -> Range<usize> {
    range.start.checked_add_signed(delta).unwrap()..range.end.checked_add_signed(delta).unwrap()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
                MetadataValue::Scalar { content, .. } => Some(plain_text(content)),
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
    table_items: &std::collections::HashSet<usize>,
    output: &mut EventOutput,
) {
    for block in blocks {
        let Block::Parsed(block) = block else {
            continue;
        };
        let Some(mark) = &block.mark else {
            for child in crate::body_children(block) {
                collect_blocks(
                    source,
                    std::slice::from_ref(child),
                    event_depth,
                    context,
                    table_items,
                    output,
                );
            }
            continue;
        };
        let scoped_context = context.with_attributes(&mark.attrs.items);
        let is_event = !table_items.contains(&block.range.start)
            && crate::list_item_facet(block) == crate::ListItemFacet::Event;

        if is_event {
            let event = event_record(source, block, event_depth, &scoped_context);
            if crate::owner_semantic_view(&block.content).positional.len() < 2 {
                output.diagnostics.push(Diagnostic {
                    code: "event.invalid-head-arity",
                    severity: DiagnosticSeverity::Warning,
                    message: "an event head requires a schedule followed by title content"
                        .to_string(),
                    range: block.content.range.clone(),
                    related: Vec::new(),
                });
            }
            collect_event_diagnostics(&event, &scoped_context, output);
            output.events.push(event);
        }
        for child in crate::body_children(block) {
            collect_blocks(
                source,
                std::slice::from_ref(child),
                event_depth + usize::from(is_event),
                &scoped_context,
                table_items,
                output,
            );
        }
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
    let uid = text_field(&mark.attrs.items, "uid");
    let (when, title, selection_range) = event_head(block);
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
    let id = mark.attrs.items.iter().find_map(|item| match item {
        AttrItem::Id {
            value, value_range, ..
        } => Some(EventField {
            value: value.clone(),
            range: value_range.clone(),
        }),
        _ => None,
    });
    EventRecord {
        range: block.range.clone(),
        marker_range: mark.range.clone(),
        selection_range,
        title,
        details: event_details(block),
        depth,
        id,
        uid,
        date,
        timezone,
        when,
        at,
        start,
        end,
        tasks: task_reference_fields(source, &mark.attrs.items, "tasks"),
        tasks_override: mark
            .attrs
            .items
            .iter()
            .any(|item| matches!(item, AttrItem::Pair { key, .. } if key == "tasks")),
    }
}

fn event_head(block: &ParsedBlock) -> (Option<EventField>, String, Range<usize>) {
    let view = crate::owner_semantic_view(&block.content);
    let arguments = view.split_first();
    let when = arguments.as_ref().and_then(|arguments| {
        let [Inline::Text { text, range }] = arguments.first.items.as_slice() else {
            return None;
        };
        Some(EventField {
            value: text.clone(),
            range: range.clone(),
        })
    });
    let title_range = arguments
        .as_ref()
        .and_then(|arguments| arguments.rest_range())
        .unwrap_or(block.content.range.end..block.content.range.end);
    let title = arguments
        .as_ref()
        .map(|arguments| arguments.rest_plain_text().trim().to_string())
        .unwrap_or_default();
    (when, title, title_range)
}

fn event_details(owner: &ParsedBlock) -> String {
    let mut lines = Vec::new();
    for child in crate::body_children(owner) {
        collect_detail_lines(std::slice::from_ref(child), &mut lines);
    }
    lines.join("\n")
}

fn collect_detail_lines(blocks: &[Block], lines: &mut Vec<String>) {
    for block in blocks {
        match block {
            Block::Parsed(block) => {
                if crate::list_item_facet(block) == crate::ListItemFacet::Event {
                    continue;
                }
                let text = plain_text(&block.content).trim().to_string();
                if !text.is_empty() {
                    lines.push(text);
                }
                for child in crate::body_children(block) {
                    collect_detail_lines(std::slice::from_ref(child), lines);
                }
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
    context: &EventContext,
    output: &mut EventOutput,
) {
    if event.when.is_none() {
        output.diagnostics.push(Diagnostic {
            code: "event.missing-time",
            severity: DiagnosticSeverity::Warning,
            message: "an event requires a schedule as its first head argument".to_string(),
            range: event.selection_range.clone(),
            related: Vec::new(),
        });
    }
    if event.title.is_empty() {
        output.diagnostics.push(Diagnostic {
            code: "event.missing-title",
            severity: DiagnosticSeverity::Warning,
            message: "an event requires a title after its schedule".to_string(),
            range: event.selection_range.clone(),
            related: Vec::new(),
        });
    }
    if event.uid.as_ref().is_some_and(|uid| uid.value.is_empty()) {
        let uid = event.uid.as_ref().expect("empty uid exists");
        output.diagnostics.push(Diagnostic {
            code: "event.invalid-uid",
            severity: DiagnosticSeverity::Warning,
            message: "an explicit event uid must not be empty".to_string(),
            range: uid.range.clone(),
            related: Vec::new(),
        });
    }

    let date = event.date.as_ref().map(|field| field.value.as_str());
    let timezone = event.timezone.as_ref().map(|field| field.value.as_str());
    let result = resolve_when(
        event.when.as_ref(),
        date.or(context.date.as_deref()),
        timezone.or(context.timezone.as_deref()),
    );
    if let (None, None, Some(when)) = (&event.at, &event.start, &event.when) {
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
            message: "event schedule cannot be resolved from its head, date, and timezone"
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
    let (start, end) = match when.value.split_once("--") {
        Some((start, end)) if !start.is_empty() && !end.is_empty() && !end.contains("--") => {
            (start, Some(end))
        }
        Some(_) => return Err(EventWhenError::InvalidWhen),
        None => (when.value.as_str(), None),
    };
    let start = resolve_start(start, date, timezone)?;
    let Some(end) = end else {
        return Ok(ResolvedWhen::Point(start));
    };
    let end = if let Some((date, time)) = end.split_once('T') {
        let date = parse_date(date)?;
        let time = parse_time(time)?;
        local_datetime(date, time, *start.offset())?
    } else {
        let time = parse_time(end)?;
        let date = if time < start.time() {
            start
                .date_naive()
                .succ_opt()
                .ok_or(EventWhenError::InvalidInterval)?
        } else {
            start.date_naive()
        };
        local_datetime(date, time, *start.offset())?
    };
    if end <= start {
        return Err(EventWhenError::InvalidInterval);
    }
    Ok(ResolvedWhen::Interval(start, end))
}

fn resolve_start(
    value: &str,
    inherited_date: Option<&str>,
    inherited_timezone: Option<&str>,
) -> Result<DateTime<FixedOffset>, EventWhenError> {
    if let Some((date, time)) = value.split_once('T') {
        let date = parse_date(date).map_err(|_| EventWhenError::InvalidWhen)?;
        if let Ok(time) = parse_time(time) {
            return local_datetime(date, time, inherited_offset(inherited_timezone, None)?);
        }
        return parse_full_rfc3339(value);
    }

    let date = inherited_date.ok_or(EventWhenError::MissingDate)?;
    let (date, date_offset) = match parse_date(date) {
        Ok(date) => (date, None),
        Err(_) => {
            let datetime =
                DateTime::parse_from_rfc3339(date).map_err(|_| EventWhenError::InvalidDate)?;
            (datetime.date_naive(), Some(*datetime.offset()))
        }
    };
    local_datetime(
        date,
        parse_time(value)?,
        inherited_offset(inherited_timezone, date_offset)?,
    )
}

fn parse_date(value: &str) -> Result<NaiveDate, EventWhenError> {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || !bytes[..4].iter().all(u8::is_ascii_digit)
        || !bytes[5..7].iter().all(u8::is_ascii_digit)
        || !bytes[8..].iter().all(u8::is_ascii_digit)
    {
        return Err(EventWhenError::InvalidWhen);
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| EventWhenError::InvalidWhen)
}

fn inherited_offset(
    timezone: Option<&str>,
    fallback: Option<FixedOffset>,
) -> Result<FixedOffset, EventWhenError> {
    match timezone {
        Some(timezone) => DateTime::parse_from_rfc3339(&format!("2000-01-01T00:00:00{timezone}"))
            .map_err(|_| EventWhenError::InvalidTimezone)
            .map(|datetime| *datetime.offset()),
        None => fallback.ok_or(EventWhenError::MissingTimezone),
    }
}

fn parse_full_rfc3339(value: &str) -> Result<DateTime<FixedOffset>, EventWhenError> {
    let (_, time_and_offset) = value.split_once('T').ok_or(EventWhenError::InvalidWhen)?;
    let bytes = time_and_offset.as_bytes();
    if bytes.len() < 9
        || bytes.get(2) != Some(&b':')
        || bytes.get(5) != Some(&b':')
        || !bytes[..2].iter().all(u8::is_ascii_digit)
        || !bytes[3..5].iter().all(u8::is_ascii_digit)
        || !bytes[6..8].iter().all(u8::is_ascii_digit)
        || !matches!(bytes[8], b'Z' | b'+' | b'-' | b'.')
    {
        return Err(EventWhenError::InvalidWhen);
    }
    DateTime::parse_from_rfc3339(value).map_err(|_| EventWhenError::InvalidWhen)
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
    use plumb_syntax::parse;

    use super::*;

    #[test]
    fn green_event_revision_reuses_relative_semantic_shards() {
        let old = "`= date 2026-09-05\n`= timezone +08:00\n\n`- 09:00 First\n `+ event\n\n`- 10:00 Middle\n `+ event\n\n`- 11:00 Last\n `+ event\n";
        let old_green = GreenDocument::parse(old);
        let old_parsed = old_green.materialize();
        let old_metadata = crate::analyze_metadata(old_parsed.valid_syntax().unwrap());
        let old_events = analyze_events(old_parsed.valid_syntax().unwrap(), &old_metadata);
        let previous =
            GreenEventRevision::from_output(&old_green, &old_metadata, &old_events).unwrap();
        assert_eq!(previous.materialize(), old_events);

        let start = old.find("Middle").unwrap();
        let mut new = old.to_string();
        new.replace_range(start..start + "Middle".len(), "Changed middle");
        let new_green = old_green
            .reparse_from_change(
                new.clone(),
                plumb_syntax::SourceChange {
                    old_range: start..start + "Middle".len(),
                    new_range: start..start + "Changed middle".len(),
                },
            )
            .document;
        let new_parsed = new_green.materialize();
        let new_metadata = crate::analyze_metadata(new_parsed.valid_syntax().unwrap());
        let current =
            GreenEventRevision::analyze(&new_green, &new_metadata, Some(&previous)).unwrap();

        assert_eq!(current.cache_hits() + 1, current.shard_count());
        assert_eq!(
            current.materialize(),
            analyze_events(new_parsed.valid_syntax().unwrap(), &new_metadata)
        );
    }

    #[test]
    fn green_event_revision_invalidates_all_shards_when_context_changes() {
        let old = "`= date 2026-09-05\n`= timezone +08:00\n\n`- 09:00 Event\n `+ event\n";
        let old_green = GreenDocument::parse(old);
        let old_parsed = old_green.materialize();
        let old_metadata = crate::analyze_metadata(old_parsed.valid_syntax().unwrap());
        let old_events = analyze_events(old_parsed.valid_syntax().unwrap(), &old_metadata);
        let previous =
            GreenEventRevision::from_output(&old_green, &old_metadata, &old_events).unwrap();
        let new = old.replace("2026-09-05", "2026-09-06");
        let new_green = old_green.reparse(new.clone()).document;
        let new_parsed = new_green.materialize();
        let new_metadata = crate::analyze_metadata(new_parsed.valid_syntax().unwrap());
        let current =
            GreenEventRevision::analyze(&new_green, &new_metadata, Some(&previous)).unwrap();

        assert_eq!(current.cache_hits(), 0);
        assert_eq!(
            current.materialize(),
            analyze_events(new_parsed.valid_syntax().unwrap(), &new_metadata)
        );
    }

    fn analyze(source: &str) -> EventOutput {
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let valid = parsed
            .valid_syntax()
            .expect("semantic analysis requires valid syntax");
        let metadata = crate::analyze_metadata(valid);
        analyze_events(valid, &metadata)
    }

    #[test]
    fn collects_event_facets_ranges_and_task_references() {
        let source = "`= date 2026-07-30\n`= timezone +08:00\n\n`- 14:00--15:30 Review\n\n `+ event\n\n `@ review\n\n `= tasks #local Project A.plumb#remote\n\n `note Details\n\n `- 09:00 Follow-up\n\n  `+ event\n\n  `= date 2026-07-31\n";
        let output = analyze(source);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(output.events.len(), 2);
        let event = output.events.get(0).unwrap();
        assert_eq!(event.title, "Review");
        assert_eq!(event.details, "Details");
        assert_eq!(event.id.as_ref().unwrap().value, "review");
        assert_eq!(
            event.start_datetime().unwrap().to_rfc3339(),
            "2026-07-30T14:00:00+08:00"
        );
        assert_eq!(
            output
                .events
                .get(1)
                .unwrap()
                .at_datetime()
                .unwrap()
                .to_rfc3339(),
            "2026-07-31T09:00:00+08:00"
        );
        assert_eq!(event.tasks.len(), 2);
        assert_eq!(event.tasks[1].source, "Project A.plumb#remote");
        assert_eq!(output.events.get(1).unwrap().depth, 1);
    }

    #[test]
    fn diagnoses_invalid_event_heads_and_intervals() {
        let source = "`= date 2026-07-30\n`= timezone +08:00\n\n`node {10:00 Generic}\n\n `+ event\n\n`- 10:00 Valid\n\n `+ event\n\n`- {}\n\n `+ event\n\n`- tomorrow Invalid when\n\n `+ event\n\n`- 11:00--11:00 Empty interval\n\n `+ event\n";
        let output = analyze(source);
        assert_eq!(output.events.len(), 4);
        assert_eq!(
            output
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            [
                "event.invalid-head-arity",
                "event.missing-time",
                "event.missing-title",
                "event.invalid-when",
                "event.invalid-interval",
            ]
        );
    }

    #[test]
    fn projects_direct_uid_property() {
        let source = "`= date 2026-07-30\n`= timezone +00:00\n\n`- 10:00 Review\n\n `+ event\n\n `= uid calendar@example\n";
        let output = analyze(source);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(
            output.events.get(0).unwrap().uid.unwrap().value,
            "calendar@example"
        );
    }

    #[test]
    fn diagnoses_empty_explicit_uid_without_treating_it_as_missing() {
        let source = "`- 2026-07-30T10:00:00Z Review\n\n `+ event\n\n `= uid `\"\"\n";
        let output = analyze(source);
        assert_eq!(output.events.get(0).unwrap().uid.unwrap().value, "");
        assert_eq!(
            output
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            ["event.invalid-uid"]
        );
    }

    #[test]
    fn overlap_uses_half_open_ranges_and_point_events() {
        let source = "`= date 2026-07-30\n`= timezone +00:00\n\n`- 10:00--11:00 Range\n\n `+ event\n\n`- 11:00 Point\n\n `+ event\n\n`- 23:40--00:00 Cross midnight\n\n `+ event\n";
        let output = analyze(source);
        let start = DateTime::parse_from_rfc3339("2026-07-30T10:30:00Z").unwrap();
        let end = DateTime::parse_from_rfc3339("2026-07-30T11:00:00Z").unwrap();
        assert!(output.events.get(0).unwrap().overlaps(start, end));
        assert!(!output.events.get(1).unwrap().overlaps(start, end));
        assert!(output.events.get(1).unwrap().is_point());
        assert!(!output.events.get(0).unwrap().is_running());
        assert_eq!(
            output
                .events
                .get(2)
                .unwrap()
                .end_datetime()
                .unwrap()
                .to_rfc3339(),
            "2026-07-31T00:00:00+00:00"
        );
    }

    #[test]
    fn old_datetime_fields_do_not_define_event_time() {
        let source = "`- Old title\n\n `+ event\n\n `= at 2026-07-30T10:00:00Z\n";
        let output = analyze(source);
        assert!(output.events.get(0).unwrap().sort_datetime().is_none());
        assert_eq!(output.diagnostics[0].code, "event.missing-date-context");
    }

    #[test]
    fn rfc3339_metadata_date_supplies_date_and_offset() {
        let source =
            "`= date `\"2026-07-30T09:15:00+08:00\"\n\n`- 23:40--00:00 Cross midnight\n\n `+ event\n";
        let output = analyze(source);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(
            output
                .events
                .get(0)
                .unwrap()
                .start_datetime()
                .unwrap()
                .to_rfc3339(),
            "2026-07-30T23:40:00+08:00"
        );
        assert_eq!(
            output
                .events
                .get(0)
                .unwrap()
                .end_datetime()
                .unwrap()
                .to_rfc3339(),
            "2026-07-31T00:00:00+08:00"
        );
    }

    #[test]
    fn date_and_timezone_context_follow_tree_scope() {
        let source = "`= date 2026-07-30\n`= timezone +08:00\n\n`div\n `= date 2026-07-31\n\n `- 09:00 Inherited date\n\n  `+ event\n\n `- 10:00 Timezone override\n\n  `+ event\n\n  `= timezone +09:00\n\n  `- 11:00 Nested inheritance\n\n   `+ event\n\n`- 12:00 Root sibling\n\n `+ event\n";
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

    #[test]
    fn when_start_can_override_date_or_complete_datetime() {
        let source = "`= timezone +08:00\n\n`- 2026-05-02T08--09:20 Local hour\n\n `+ event\n\n`- 2026-05-02T08:22--09:20 Local minute\n\n `+ event\n\n`- 2026-05-02T08:22:31+08:00--09:20 Zoned\n\n `+ event\n\n`- 2026-05-02T23:22:31Z--09:20 Zoned overnight\n\n `+ event\n\n `= timezone invalid\n";
        let output = analyze(source);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(
            output
                .events
                .iter()
                .map(|event| (
                    event.start_datetime().unwrap().to_rfc3339(),
                    event.end_datetime().unwrap().to_rfc3339(),
                ))
                .collect::<Vec<_>>(),
            [
                (
                    "2026-05-02T08:00:00+08:00".to_string(),
                    "2026-05-02T09:20:00+08:00".to_string(),
                ),
                (
                    "2026-05-02T08:22:00+08:00".to_string(),
                    "2026-05-02T09:20:00+08:00".to_string(),
                ),
                (
                    "2026-05-02T08:22:31+08:00".to_string(),
                    "2026-05-02T09:20:00+08:00".to_string(),
                ),
                (
                    "2026-05-02T23:22:31+00:00".to_string(),
                    "2026-05-03T09:20:00+00:00".to_string(),
                ),
            ]
        );
    }

    #[test]
    fn dated_when_end_inherits_start_offset_and_supports_multi_day_intervals() {
        let source = "`- 2026-05-02T08:22:31+08:00--2026-05-05T09:20:00 Multi-day\n\n `+ event\n\n`- 2026-05-02T23:22:31Z--2026-05-03T09:20 UTC\n\n `+ event\n";
        let output = analyze(source);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(
            output
                .events
                .iter()
                .map(|event| event.end_datetime().unwrap().to_rfc3339())
                .collect::<Vec<_>>(),
            ["2026-05-05T09:20:00+08:00", "2026-05-03T09:20:00+00:00"]
        );
    }

    #[test]
    fn dated_when_end_rejects_timezone_and_non_increasing_intervals() {
        let source = "`- 2026-05-02T08:22:31+08:00--2026-05-03T09:20:00+08:00 Zoned end\n\n `+ event\n\n`- 2026-05-02T08:22:31+08:00--2026-05-01T09:20 Before start\n\n `+ event\n\n`- 2026-05-02T08:22:31+08:00--2026-05-02T08:22:31 Equal\n\n `+ event\n";
        let output = analyze(source);
        assert_eq!(
            output
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            [
                "event.invalid-when",
                "event.invalid-interval",
                "event.invalid-interval",
            ]
        );
        assert!(output.events.iter().all(|event| event.start.is_none()));
    }

    #[test]
    fn zoned_when_start_requires_full_rfc3339_time() {
        let source =
            "`- 2026-05-02T08+08:00 Hour\n\n `+ event\n\n`- 2026-05-02T08:22+08:00 Minute\n\n `+ event\n";
        let output = analyze(source);
        assert_eq!(
            output
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            ["event.invalid-when", "event.invalid-when"]
        );
        assert!(output.events.iter().all(|event| event.at.is_none()));
    }

    #[test]
    fn complete_when_point_needs_no_inherited_context() {
        let output = analyze("`- 2026-05-02T08:22:31+08:00 Point\n\n `+ event\n");
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(
            output
                .events
                .get(0)
                .unwrap()
                .at_datetime()
                .unwrap()
                .to_rfc3339(),
            "2026-05-02T08:22:31+08:00"
        );
    }

    #[test]
    fn metadata_uid_links_have_no_event_semantics() {
        let source = "`= date 2026-07-30\n`= timezone +08:00\n`= event-uids\n\n `- `->{mapped@example #review}\n\n  `+ event\n\n`- 09:00 Review\n\n `+ event\n\n `@ review\n\n `= uid inline@example\n";
        let output = analyze(source);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(output.events.get(0).unwrap().title, "Review");
        assert_eq!(
            output.events.get(0).unwrap().uid.unwrap().value,
            "inline@example"
        );
    }
}
