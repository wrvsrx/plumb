use plumb_semantics::{
    analyze_document, citation_completion_context, construct_completion_context,
    image_completion_context, link_completion_context, ConstructCompletionContext, InlineStyleKind,
    LinkCompletionContext, MathKind, MetadataValue,
};
use plumb_syntax::parse;

fn analyze(source: &str) -> plumb_semantics::DocumentOutput {
    let parsed = parse(source);
    assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
    analyze_document(parsed.valid_syntax().unwrap())
}

#[test]
fn projects_metadata_tasks_events_and_document_structure() {
    let source = concat!(
        "`= title Project Notes\n",
        "`= tags\n `+ syntax\n `+ notes\n",
        "`# Heading\n `@ intro\n",
        "`- Implement parser\n `+ task\n `@ parser\n `= created 2026-09-02T09:00:00+08:00\n",
        "`- 14:00--15:00 Parser review\n `+ event\n `= date 2026-09-02\n `= timezone +08:00\n",
    );
    let output = analyze(source);
    assert_eq!(
        output.metadata.document_title().as_deref(),
        Some("Project Notes")
    );
    let metadata = output.metadata.metadata.as_ref().unwrap();
    assert!(matches!(
        metadata.entries[1].value,
        MetadataValue::List { .. }
    ));
    assert_eq!(output.headings.headings.len(), 1);
    assert_eq!(output.anchors.len(), 2);
    assert_eq!(output.tasks.tasks.len(), 1);
    assert_eq!(output.tasks.tasks[0].title, "Implement parser");
    assert_eq!(output.events.events.len(), 1);
    assert_eq!(output.events.events[0].title, "Parser review");
    assert!(output.events.events[0].start.is_some());
}

#[test]
fn projects_recursive_inline_forms_and_verbatim_math() {
    let source = concat!(
        "See `->{{guide page} {Project Guide.plumb}}, `->\"other.plumb\", and `cite{smith2004}.\n",
        "Use `*{emphasis}, `!{strong}, and `$\"x^2\".\n",
        "`$\"\n E = mc^2\n",
    );
    let output = analyze(source);
    assert_eq!(output.links.len(), 2);
    assert_eq!(output.links[0].target.value, "Project Guide.plumb");
    assert_eq!(output.citations.citations.len(), 1);
    assert_eq!(
        output
            .inline_styles
            .styles
            .iter()
            .map(|style| style.kind)
            .collect::<Vec<_>>(),
        [InlineStyleKind::Emphasis, InlineStyleKind::Strong]
    );
    assert_eq!(
        output
            .math
            .records
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [MathKind::Inline, MathKind::Display]
    );
}

#[test]
fn table_spaces_form_cells_and_expanded_rows_use_anonymous_children() {
    let compact = analyze(concat!(
        "`table\n",
        " `- name    age\n  `+ header\n",
        " `- {Alice Smith}    10\n",
    ));
    let table = &compact.tables.tables[0];
    assert_eq!(table.column_count, 2);
    assert_eq!(table.rows.len(), 2);
    assert!(table.rows[0].header);

    let expanded = analyze(concat!(
        "`table\n",
        " `-\n  `+ header\n  name\n  age\n",
        " `-\n\n  {Alice Smith}\n  10\n",
    ));
    let table = &expanded.tables.tables[0];
    assert_eq!(table.column_count, 2);
    assert!(table.rows.iter().all(|row| !row.compact));
}

#[test]
fn current_recovered_completion_contexts_follow_brace_data() {
    let citation = parse("See `cite{smi");
    assert_eq!(
        citation_completion_context(&citation, citation.source.len())
            .unwrap()
            .query,
        "smi"
    );

    let label = parse("See `->{Usage");
    assert!(matches!(
        link_completion_context(&label, label.source.len()),
        Some(LinkCompletionContext::Label { query, .. }) if query == "Usage"
    ));

    let path = parse("See `->{x doc.plumb#ta");
    assert!(matches!(
        link_completion_context(&path, path.source.len()),
        Some(LinkCompletionContext::Anchor { path, query, .. })
            if path == "doc.plumb" && query == "ta"
    ));

    let image = parse("`img{Alt `={src static/im");
    assert_eq!(
        image_completion_context(&image, image.source.len())
            .unwrap()
            .query,
        "static/im"
    );

    let construct = parse("Text `->{");
    assert!(matches!(
        construct_completion_context(&construct, construct.source.len()),
        Some(ConstructCompletionContext::Link { .. })
    ));
}
