use lsp_types::{DocumentSymbol, SymbolKind};
use plumb_semantics::{
    AnchorRecord, EventRecordView, EventRecords, Heading, MetadataBlock, MetadataEntry,
    MetadataValue, SemanticRecordView, SemanticRecords, TaskRecord, TaskState,
};

use crate::position::PositionIndex;

pub(crate) fn heading(positions: &PositionIndex<'_>, heading: &Heading) -> DocumentSymbol {
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
        range: positions.byte_range_to_lsp(&heading.section_range),
        selection_range: positions.byte_range_to_lsp(&heading.selection_range),
        children: (!heading.children.is_empty()).then(|| {
            heading
                .children
                .iter()
                .map(|child| self::heading(positions, child))
                .collect()
        }),
    }
}

pub(crate) fn anchor(positions: &PositionIndex<'_>, anchor: &AnchorRecord) -> DocumentSymbol {
    #[allow(deprecated)]
    DocumentSymbol {
        name: format!("#{}", anchor.id.value),
        detail: Some("explicit anchor".to_string()),
        kind: SymbolKind::KEY,
        tags: None,
        deprecated: None,
        range: positions.byte_range_to_lsp(&anchor.range),
        selection_range: positions.byte_range_to_lsp(&anchor.id.range),
        children: None,
    }
}

pub(crate) fn metadata(positions: &PositionIndex<'_>, metadata: &MetadataBlock) -> DocumentSymbol {
    #[allow(deprecated)]
    DocumentSymbol {
        name: "metadata".to_string(),
        detail: Some("document metadata".to_string()),
        kind: SymbolKind::OBJECT,
        tags: None,
        deprecated: None,
        range: positions.byte_range_to_lsp(&metadata.range),
        selection_range: positions.byte_range_to_lsp(&metadata.selection_range),
        children: (!metadata.entries.is_empty()).then(|| {
            metadata
                .entries
                .iter()
                .map(|entry| metadata_entry(positions, entry))
                .collect()
        }),
    }
}

fn metadata_entry(positions: &PositionIndex<'_>, entry: &MetadataEntry) -> DocumentSymbol {
    let (detail, children) = match &entry.value {
        MetadataValue::Null { .. } => ("null".to_string(), None),
        MetadataValue::Scalar { content, .. } => (content.plain_text(), None),
        MetadataValue::List { items, .. } => (format!("list ({} items)", items.len()), None),
        MetadataValue::Map { entries, .. } => (
            "map".to_string(),
            (!entries.is_empty()).then(|| {
                entries
                    .iter()
                    .map(|entry| metadata_entry(positions, entry))
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
        range: positions.byte_range_to_lsp(&entry.range),
        selection_range: positions.byte_range_to_lsp(&entry.key_range),
        children,
    }
}

pub(crate) fn tasks(
    positions: &PositionIndex<'_>,
    tasks: &SemanticRecords<TaskRecord>,
) -> Vec<DocumentSymbol> {
    if tasks.is_empty() {
        return Vec::new();
    }
    nested_symbols(
        tasks.views(),
        |task| task.depth(),
        |task| task_symbol(positions, task),
    )
}

fn task_symbol(
    positions: &PositionIndex<'_>,
    task: &SemanticRecordView<'_, TaskRecord>,
) -> DocumentSymbol {
    let id = task
        .id_value()
        .map(|id| format!(" #{id}"))
        .unwrap_or_default();
    #[allow(deprecated)]
    DocumentSymbol {
        name: nonempty_title(task.title(), "Untitled task"),
        detail: Some(format!("{}{}", task_state_name(task.state()), id)),
        kind: SymbolKind::EVENT,
        tags: None,
        deprecated: None,
        range: positions.byte_range_to_lsp(&task.range()),
        selection_range: positions.byte_range_to_lsp(&task.selection_range()),
        children: None,
    }
}

pub(crate) fn events(positions: &PositionIndex<'_>, events: &EventRecords) -> Vec<DocumentSymbol> {
    if events.is_empty() {
        return Vec::new();
    }
    nested_symbols(
        events.views(),
        |event| event.depth(),
        |event| event_symbol(positions, event),
    )
}

fn event_symbol(positions: &PositionIndex<'_>, event: &EventRecordView<'_>) -> DocumentSymbol {
    let id = event
        .id_value()
        .map(|id| format!(" #{id}"))
        .unwrap_or_default();
    let start = event.start_value().unwrap_or("invalid start");
    #[allow(deprecated)]
    DocumentSymbol {
        name: nonempty_title(event.title(), "Untitled event"),
        detail: Some(format!("{start}{id}")),
        kind: SymbolKind::EVENT,
        tags: None,
        deprecated: None,
        range: positions.byte_range_to_lsp(&event.range()),
        selection_range: positions.byte_range_to_lsp(&event.selection_range()),
        children: None,
    }
}

fn nested_symbols<T>(
    records: impl IntoIterator<Item = T>,
    depth: impl Fn(&T) -> usize,
    symbol: impl Fn(&T) -> DocumentSymbol,
) -> Vec<DocumentSymbol> {
    let mut roots = Vec::new();
    let mut path = Vec::new();
    for record in records {
        while path.len() > depth(&record) {
            path.pop();
        }
        let siblings = children_mut(&mut roots, &path);
        siblings.push(symbol(&record));
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

pub(crate) fn insert_all(symbols: &mut Vec<DocumentSymbol>, additional: Vec<DocumentSymbol>) {
    let headings = symbols
        .iter()
        .enumerate()
        .filter(|(_, symbol)| symbol.kind == SymbolKind::STRING)
        .map(|(index, symbol)| (index, symbol.range))
        .collect::<Vec<_>>();
    let mut children = vec![Vec::new(); headings.len()];
    let mut siblings = Vec::new();
    for symbol in additional {
        let candidate = headings
            .partition_point(|(_, range)| position_key(range.end) < position_key(symbol.range.end));
        if headings
            .get(candidate)
            .is_some_and(|(_, range)| range_contains(range, &symbol.range))
        {
            children[candidate].push(symbol);
        } else {
            siblings.push(symbol);
        }
    }
    for ((index, _), additions) in headings.into_iter().zip(children) {
        if !additions.is_empty() {
            insert_all(
                symbols[index].children.get_or_insert_with(Vec::new),
                additions,
            );
        }
    }
    symbols.extend(siblings);
    symbols.sort_by_key(|symbol| position_key(symbol.range.start));
}

#[cfg(test)]
fn insert(symbols: &mut Vec<DocumentSymbol>, symbol: DocumentSymbol) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_insertion_matches_sequential_heading_containment_and_ties() {
        #[allow(deprecated)]
        let symbol = |name: &str, start: u32, end: u32, kind| DocumentSymbol {
            name: name.to_owned(),
            detail: None,
            kind,
            tags: None,
            deprecated: None,
            range: lsp_types::Range::new(
                lsp_types::Position::new(start, 0),
                lsp_types::Position::new(end, 0),
            ),
            selection_range: lsp_types::Range::new(
                lsp_types::Position::new(start, 0),
                lsp_types::Position::new(start, 0),
            ),
            children: None,
        };
        let mut outer = symbol("outer", 10, 30, SymbolKind::STRING);
        outer.children = Some(vec![symbol("inner", 15, 20, SymbolKind::STRING)]);
        let original = vec![outer, symbol("next", 30, 50, SymbolKind::STRING)];
        let mut additions = Vec::new();
        for (index, (start, end)) in [
            (0, 0),
            (10, 11),
            (15, 19),
            (19, 21),
            (30, 30),
            (30, 31),
            (40, 45),
            (49, 55),
            (60, 61),
        ]
        .into_iter()
        .enumerate()
        {
            additions.push(symbol(
                &format!("item-{index}"),
                start,
                end,
                SymbolKind::EVENT,
            ));
            additions.push(symbol(
                &format!("same-start-{index}"),
                start,
                end,
                SymbolKind::KEY,
            ));
        }
        let mut expected = original.clone();
        for addition in additions.clone() {
            insert(&mut expected, addition);
        }
        let mut actual = original;
        insert_all(&mut actual, additions);
        assert_eq!(actual, expected);
    }
}
