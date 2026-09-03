use std::ops::Range;

use plumb_syntax::{
    Block, Diagnostic, DiagnosticSeverity, InlineContent, ParsedBlock, ValidDocument,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableCellRecord {
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
    pub header: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRowRecord {
    pub range: Range<usize>,
    pub header: bool,
    pub compact: bool,
    pub cells: Vec<TableCellRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRecord {
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
    pub caption: InlineContent,
    pub column_count: usize,
    pub row_head_columns: usize,
    pub rows: Vec<TableRowRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TableOutput {
    pub tables: Vec<TableRecord>,
    pub diagnostics: Vec<Diagnostic>,
}

impl TableOutput {
    pub fn table_at_node_start(&self, start: usize) -> Option<&TableRecord> {
        self.tables.iter().find(|table| table.range.start == start)
    }
}

pub fn analyze_tables(valid: ValidDocument<'_>) -> TableOutput {
    let mut output = TableOutput::default();
    collect_tables(valid.syntax().blocks.iter(), &mut output);
    output.tables.sort_by_key(|table| table.range.start);
    output
}

fn collect_tables<'a>(blocks: impl IntoIterator<Item = &'a Block>, output: &mut TableOutput) {
    for block in blocks {
        let Block::Parsed(block) = block else {
            continue;
        };
        if block
            .mark
            .as_ref()
            .is_some_and(|mark| mark.marker == "table")
        {
            analyze_table(block, output);
            continue;
        }
        collect_tables(crate::body_children(block), output);
    }
}

fn analyze_table(table: &ParsedBlock, output: &mut TableOutput) {
    let mut rows = Vec::new();
    for child in crate::body_children(table) {
        let Some(row) = marked(child, "-") else {
            output.diagnostics.push(diagnostic(
                "table.invalid-child",
                "a table may contain only row '-' blocks and direct declarations",
                child.range().clone(),
            ));
            continue;
        };
        rows.push(analyze_row(row, output));
    }

    let column_count = rows.first().map_or(0, |row| row.cells.len());
    let mut body_started = false;
    for row in &rows {
        if row.cells.is_empty() {
            output.diagnostics.push(diagnostic(
                "table.empty-row",
                "a table row must contain at least one cell",
                row.range.clone(),
            ));
        } else if row.cells.len() != column_count {
            output.diagnostics.push(diagnostic(
                "table.column-count-mismatch",
                format!(
                    "table row has {} cells; expected {column_count}",
                    row.cells.len()
                ),
                row.range.clone(),
            ));
        }
        if row.header && body_started {
            output.diagnostics.push(diagnostic(
                "table.header-after-body",
                "header rows must form a consecutive prefix of the table",
                row.range.clone(),
            ));
        }
        body_started |= !row.header;
    }

    let mut row_head_columns = None;
    for row in rows.iter().filter(|row| !row.header) {
        let prefix = row.cells.iter().take_while(|cell| cell.header).count();
        let has_nonprefix_header = row.cells[prefix..].iter().any(|cell| cell.header);
        if has_nonprefix_header || row_head_columns.is_some_and(|expected| expected != prefix) {
            output.diagnostics.push(diagnostic(
                "table.inconsistent-row-headers",
                "body rows must use one consistent leading row-header cell count",
                row.range.clone(),
            ));
        } else {
            row_head_columns.get_or_insert(prefix);
        }
    }

    output.tables.push(TableRecord {
        range: table.range.clone(),
        selection_range: crate::inline_selection_range(&table.content),
        caption: table.content.clone(),
        column_count,
        row_head_columns: row_head_columns.unwrap_or(0),
        rows,
    });
}

fn analyze_row(row: &ParsedBlock, output: &mut TableOutput) -> TableRowRecord {
    let view = crate::owner_semantic_view(&row.content);
    let compact = !view.positional.is_empty();
    let cells = if compact {
        for child in crate::body_children(row) {
            output.diagnostics.push(diagnostic(
                "table.compact-row-child",
                "a compact table row cannot contain structural children",
                child.range().clone(),
            ));
        }
        view.positional
            .iter()
            .map(|content| TableCellRecord {
                selection_range: crate::element_selection_range(content),
                range: content.range.clone(),
                header: false,
            })
            .collect()
    } else {
        crate::body_children(row)
            .map(|child| {
                let (selection_range, header) = match child {
                    Block::Parsed(cell) => (
                        crate::inline_selection_range(&cell.content),
                        has_header_facet(cell),
                    ),
                    Block::Verbatim(cell) => (cell.range.clone(), false),
                };
                TableCellRecord {
                    range: child.range().clone(),
                    selection_range,
                    header,
                }
            })
            .collect()
    };
    TableRowRecord {
        range: row.range.clone(),
        header: has_header_facet(row),
        compact,
        cells,
    }
}

fn marked<'a>(block: &'a Block, marker: &str) -> Option<&'a ParsedBlock> {
    let Block::Parsed(block) = block else {
        return None;
    };
    block
        .mark
        .as_ref()
        .is_some_and(|mark| mark.marker == marker)
        .then_some(block)
}

fn has_header_facet(block: &ParsedBlock) -> bool {
    block
        .mark
        .as_ref()
        .is_some_and(|mark| mark.attrs.has_class("header"))
        || block.children.iter().any(|child| {
            let Block::Parsed(child) = child else {
                return false;
            };
            child.children.is_empty()
                && child.mark.as_ref().is_some_and(|mark| mark.marker == "+")
                && child.content.plain_text().trim() == "header"
        })
}

fn diagnostic(code: &'static str, message: impl Into<String>, range: Range<usize>) -> Diagnostic {
    Diagnostic {
        code,
        severity: DiagnosticSeverity::Warning,
        message: message.into(),
        range,
        related: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use plumb_syntax::parse;

    use super::*;

    fn analyze(source: &str) -> TableOutput {
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        analyze_tables(parsed.valid_syntax().unwrap())
    }

    #[test]
    fn recognizes_compact_rows_with_padding_headers_and_empty_cells() {
        let output = analyze(
            "`table People\n `- name  age note\n  `+ header\n `- Alice 10  {}\n `- Bob   20  active\n",
        );
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let table = &output.tables[0];
        assert_eq!(table.caption.plain_text(), "People");
        assert_eq!(table.column_count, 3);
        assert!(table.rows[0].header);
        assert!(table.rows.iter().all(|row| row.compact));
        assert_eq!(table.rows[1].cells[2].selection_range.len(), 0);
    }

    #[test]
    fn recognizes_expanded_rich_and_row_header_cells() {
        let output = analyze(
            "`table\n `-\n  `+ header\n  `- name\n  `- age\n `-\n  `- Alice\n   `+ header\n  `- 10\n\n   `note Approximate\n",
        );
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let table = &output.tables[0];
        assert_eq!(table.column_count, 2);
        assert_eq!(table.row_head_columns, 1);
        assert!(!table.rows[0].compact);
        assert!(table.rows[1].cells[0].header);
    }

    #[test]
    fn diagnoses_invalid_structure_and_inconsistent_columns() {
        let output = analyze(
            "`table\n `note invalid\n `- one two\n  `note invalid\n `-\n `- late\n  `+ header\n",
        );
        let codes = output
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();
        assert!(codes.contains(&"table.invalid-child"));
        assert!(codes.contains(&"table.compact-row-child"));
        assert!(codes.contains(&"table.empty-row"));
        assert!(codes.contains(&"table.column-count-mismatch"));
        assert!(codes.contains(&"table.header-after-body"));
    }
}
