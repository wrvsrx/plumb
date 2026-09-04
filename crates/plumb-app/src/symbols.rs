use lsp_types::{DocumentSymbol, SymbolKind};
use plumb_semantics::{
    AnchorRecord, EventRecord, EventRecords, Heading, MetadataBlock, MetadataEntry, MetadataValue,
    SemanticRecords, TaskRecord, TaskState,
};

use crate::position::byte_range_to_lsp;

pub(crate) fn heading(source: &str, heading: &Heading) -> DocumentSymbol {
    #[allow(deprecated)]
    DocumentSymbol {
        name: if heading.title.is_empty() {
            format!("Heading {}", heading.level)
        } else {
            heading.title.clone()
        },
        detail: Some(format!("level {}", heading.level)),
        kind: SymbolKind::STRING,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(source, &heading.section_range),
        selection_range: byte_range_to_lsp(source, &heading.selection_range),
        children: (!heading.children.is_empty()).then(|| {
            heading
                .children
                .iter()
                .map(|child| self::heading(source, child))
                .collect()
        }),
    }
}

pub(crate) fn anchor(source: &str, anchor: &AnchorRecord) -> DocumentSymbol {
    #[allow(deprecated)]
    DocumentSymbol {
        name: format!("#{}", anchor.id.value),
        detail: Some("explicit anchor".to_string()),
        kind: SymbolKind::KEY,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(source, &anchor.range),
        selection_range: byte_range_to_lsp(source, &anchor.id.range),
        children: None,
    }
}

pub(crate) fn metadata(source: &str, metadata: &MetadataBlock) -> DocumentSymbol {
    #[allow(deprecated)]
    DocumentSymbol {
        name: "metadata".to_string(),
        detail: Some("document metadata".to_string()),
        kind: SymbolKind::OBJECT,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(source, &metadata.range),
        selection_range: byte_range_to_lsp(source, &metadata.selection_range),
        children: (!metadata.entries.is_empty()).then(|| {
            metadata
                .entries
                .iter()
                .map(|entry| metadata_entry(source, entry))
                .collect()
        }),
    }
}

fn metadata_entry(source: &str, entry: &MetadataEntry) -> DocumentSymbol {
    let (detail, children) = match &entry.value {
        MetadataValue::Null { .. } => ("null".to_string(), None),
        MetadataValue::Scalar { content, .. } => (content.plain_text(), None),
        MetadataValue::List { items, .. } => (format!("list ({} items)", items.len()), None),
        MetadataValue::Map { entries, .. } => (
            "map".to_string(),
            (!entries.is_empty()).then(|| {
                entries
                    .iter()
                    .map(|entry| metadata_entry(source, entry))
                    .collect()
            }),
        ),
        MetadataValue::Verbatim { .. } => ("verbatim".to_string(), None),
        MetadataValue::Unsupported { .. } => ("unsupported value".to_string(), None),
    };
    #[allow(deprecated)]
    DocumentSymbol {
        name: entry.key.clone(),
        detail: Some(detail),
        kind: SymbolKind::PROPERTY,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(source, &entry.range),
        selection_range: byte_range_to_lsp(source, &entry.key_range),
        children,
    }
}

pub(crate) fn tasks(source: &str, tasks: &SemanticRecords<TaskRecord>) -> Vec<DocumentSymbol> {
    let tasks = tasks.iter().collect::<Vec<_>>();
    nested_symbols(&tasks, |task| task.depth, |task| task_symbol(source, task))
}

fn task_symbol(source: &str, task: &TaskRecord) -> DocumentSymbol {
    let id = task
        .id
        .as_ref()
        .map(|id| format!(" #{}", id.value))
        .unwrap_or_default();
    #[allow(deprecated)]
    DocumentSymbol {
        name: nonempty_title(&task.title, "Untitled task"),
        detail: Some(format!("{}{}", task_state_name(task.state()), id)),
        kind: SymbolKind::EVENT,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(source, &task.range),
        selection_range: byte_range_to_lsp(source, &task.selection_range),
        children: None,
    }
}

pub(crate) fn events(source: &str, events: &EventRecords) -> Vec<DocumentSymbol> {
    let events = events.iter().collect::<Vec<_>>();
    nested_symbols(
        &events,
        |event| event.depth,
        |event| event_symbol(source, event),
    )
}

fn event_symbol(source: &str, event: &EventRecord) -> DocumentSymbol {
    let id = event
        .id
        .as_ref()
        .map(|id| format!(" #{}", id.value))
        .unwrap_or_default();
    let start = event
        .start
        .as_ref()
        .map(|start| start.value.as_str())
        .unwrap_or("invalid start");
    #[allow(deprecated)]
    DocumentSymbol {
        name: nonempty_title(&event.title, "Untitled event"),
        detail: Some(format!("{start}{id}")),
        kind: SymbolKind::EVENT,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(source, &event.range),
        selection_range: byte_range_to_lsp(source, &event.selection_range),
        children: None,
    }
}

fn nested_symbols<T>(
    records: &[T],
    depth: impl Fn(&T) -> usize,
    symbol: impl Fn(&T) -> DocumentSymbol,
) -> Vec<DocumentSymbol> {
    let mut roots = Vec::new();
    let mut path = Vec::new();
    for record in records {
        while path.len() > depth(record) {
            path.pop();
        }
        let siblings = children_mut(&mut roots, &path);
        siblings.push(symbol(record));
        path.push(siblings.len() - 1);
    }
    roots
}

fn children_mut<'a>(
    roots: &'a mut Vec<DocumentSymbol>,
    path: &[usize],
) -> &'a mut Vec<DocumentSymbol> {
    let mut children = roots;
    for index in path {
        children = children[*index].children.get_or_insert_with(Vec::new);
    }
    children
}

pub(crate) fn insert(symbols: &mut Vec<DocumentSymbol>, symbol: DocumentSymbol) {
    let containing_heading = symbols.iter().position(|candidate| {
        candidate.kind == SymbolKind::STRING && range_contains(&candidate.range, &symbol.range)
    });
    if let Some(index) = containing_heading {
        insert(symbols[index].children.get_or_insert_with(Vec::new), symbol);
        return;
    }
    let start = symbol.range.start;
    let index = symbols
        .iter()
        .position(|candidate| position_key(candidate.range.start) > position_key(start))
        .unwrap_or(symbols.len());
    symbols.insert(index, symbol);
}

fn range_contains(outer: &lsp_types::Range, inner: &lsp_types::Range) -> bool {
    position_key(outer.start) <= position_key(inner.start)
        && position_key(inner.end) <= position_key(outer.end)
}

fn position_key(position: lsp_types::Position) -> (u32, u32) {
    (position.line, position.character)
}

pub(crate) fn task_state_name(state: TaskState) -> &'static str {
    match state {
        TaskState::Open => "open",
        TaskState::Done => "done",
        TaskState::Canceled => "canceled",
        TaskState::Conflicted => "conflicted",
    }
}

fn nonempty_title(title: &str, fallback: &str) -> String {
    if title.is_empty() {
        fallback.to_string()
    } else {
        title.to_string()
    }
}
