use plumb_semantics::analyze_document;
use plumb_syntax::parse;

#[test]
fn category_is_owner_local_and_preserves_invalid_declarations() {
    let source = "`+ task\n`= category project\n`- Phone\n `@ phone\n `= category relax\n`- Child task\n `+ task\n `= category phd misc\n`- Invalid\n `@ invalid\n `= category\n  `+ relax\n`- Duplicate\n `@ duplicate\n `= category work\n `= category relax\n`- Missing\n `@ missing\n";
    let parsed = parse(source);
    let output = analyze_document(parsed.valid_syntax().unwrap());
    let tasks = &output.tasks().tasks;
    assert_eq!(
        tasks.get(0).unwrap().category.value.as_deref(),
        Some("project")
    );
    assert_eq!(
        tasks.get(1).unwrap().category.value.as_deref(),
        Some("phd misc")
    );
    let anchors = output.anchors();
    assert_eq!(
        anchors.get(0).unwrap().category.value.as_deref(),
        Some("relax")
    );
    assert!(anchors.get(0).unwrap().list_item);
    assert!(anchors.get(1).unwrap().category.invalid);
    assert!(anchors.get(2).unwrap().category.invalid);
    assert_eq!(anchors.get(2).unwrap().category.declarations.len(), 2);
    assert!(anchors.get(3).unwrap().category.value.is_none());
}

#[test]
fn accounting_links_only_include_direct_title_members() {
    let source = "`- 2026-09-22T10:00:00Z--11:00 `->{#a} `->\"#b\" `*{see `->{#c}}\n `+ event\n `= category override\n\n Reference `->{#d}\n";
    let parsed = parse(source);
    let output = analyze_document(parsed.valid_syntax().unwrap());
    let event = output.events().events.get(0).unwrap();
    assert_eq!(event.category.value.as_deref(), Some("override"));
    assert_eq!(
        event
            .accounting_links
            .iter()
            .map(|r| &source[r.clone()])
            .collect::<Vec<_>>(),
        ["`->{#a}", "`->\"#b\""]
    );
}
