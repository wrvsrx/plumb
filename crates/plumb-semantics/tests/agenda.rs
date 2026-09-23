use plumb_semantics::analyze_document;
use plumb_syntax::parse;

#[test]
fn category_is_owner_local_and_preserves_invalid_declarations() {
    let source = "`+ task\n`= event-category project\n`- Phone\n `@ phone\n `= event-category relax\n`- Child task\n `+ task\n `= event-category phd misc\n`- Invalid\n `@ invalid\n `= event-category\n  `+ relax\n`- Duplicate\n `@ duplicate\n `= event-category work\n `= event-category relax\n`- Missing\n `@ missing\n";
    let parsed = parse(source);
    let output = analyze_document(parsed.valid_syntax().unwrap());
    let tasks = &output.tasks().tasks;
    assert_eq!(
        tasks
            .get(0)
            .unwrap()
            .category
            .values
            .first()
            .map(String::as_str),
        Some("project")
    );
    assert_eq!(
        tasks
            .get(1)
            .unwrap()
            .category
            .values
            .first()
            .map(String::as_str),
        Some("phd misc")
    );
    let anchors = output.anchors();
    assert_eq!(
        anchors
            .get(0)
            .unwrap()
            .category
            .values
            .first()
            .map(String::as_str),
        Some("relax")
    );
    assert!(anchors.get(0).unwrap().list_item);
    assert!(anchors.get(1).unwrap().category.invalid);
    assert!(anchors.get(2).unwrap().category.invalid);
    assert_eq!(anchors.get(2).unwrap().category.declarations.len(), 2);
    assert!(anchors.get(3).unwrap().category.values.is_empty());
}

#[test]
fn accounting_links_only_include_direct_title_members() {
    let source = "`- 2026-09-22T10:00:00Z--11:00 `->{#a} `->\"#b\" `*{see `->{#c}}\n `+ event\n `= event-category override\n\n Reference `->{#d}\n";
    let parsed = parse(source);
    let output = analyze_document(parsed.valid_syntax().unwrap());
    let event = output.events().events.get(0).unwrap();
    assert_eq!(
        event.category.values.first().map(String::as_str),
        Some("override")
    );
    assert_eq!(
        event
            .accounting_links
            .iter()
            .map(|r| &source[r.clone()])
            .collect::<Vec<_>>(),
        ["`->{#a}", "`->\"#b\""]
    );
}

#[test]
fn category_accepts_plain_grouping_and_literal_spelling_but_not_rich_values() {
    for spelling in ["phd misc", "{phd misc}", "`\"phd misc\""] {
        let source = format!("`- Work\n `+ task\n `= event-category {spelling}\n");
        let parsed = parse(&source);
        let output = analyze_document(parsed.valid_syntax().unwrap());
        let category = output.tasks().tasks.get(0).unwrap().category;
        assert!(!category.invalid, "{spelling}");
        assert_eq!(
            category.values.first().map(String::as_str),
            Some("phd misc")
        );
    }
    let parsed = parse("`- Work\n `+ task\n `= event-category `!{work}\n");
    assert!(
        analyze_document(parsed.valid_syntax().unwrap())
            .tasks()
            .tasks
            .get(0)
            .unwrap()
            .category
            .invalid
    );
}

#[test]
fn category_list_preserves_multiword_values_and_deduplicates() {
    let parsed =
        parse("`- Work\n `+ task\n `= event-category\n  `- phd misc\n  `- others\n  `- phd misc\n");
    let output = analyze_document(parsed.valid_syntax().unwrap());
    let category = output.tasks().tasks.get(0).unwrap().category;
    assert!(!category.invalid);
    assert_eq!(category.values, ["phd misc", "others"]);
    for tail in ["", "  `-\n", "  `- work\n\n   nested\n", "  `+ work\n"] {
        let parsed = parse(&format!("`- Work\n `+ task\n `= event-category\n{tail}"));
        assert!(
            analyze_document(parsed.valid_syntax().unwrap())
                .tasks()
                .tasks
                .get(0)
                .unwrap()
                .category
                .invalid
        );
    }
}

#[test]
fn category_remains_generic_metadata_and_event_category_is_document_local() {
    let parsed = parse("`= category topic\n`= event-category phd misc\n\n`- Item\n `@ item\n `= category unrelated\n");
    let output = analyze_document(parsed.valid_syntax().unwrap());
    assert_eq!(output.document_category().values, ["phd misc"]);
    assert!(output.anchors().get(0).unwrap().category.values.is_empty());
}
