use plumb_edit::{
    apply_text_edits, replace_green_inline, replace_owned_inline, EditError, OwnedInline,
    OwnedInlineMember,
};
use plumb_syntax::{parse, GreenDocument};

#[test]
fn replacing_a_complete_inline_owner_preserves_surrounding_source_and_green_parity() {
    let source = "Prelude\r\n\r\nSee `node{before}  tail.\r\n";
    let start = source.find("`node{").unwrap();
    let range = start..start + "`node{before}".len();
    let node = OwnedInline::Element {
        kind: String::new(),
        members: vec![OwnedInlineMember::ParsedArgument(vec![OwnedInline::Text(
            "after".into(),
        )])],
    };
    let parsed = parse(source);
    let edit = replace_owned_inline(&parsed, range.clone(), &node).unwrap();
    let green = GreenDocument::parse(source);
    assert_eq!(
        replace_green_inline(&green, range.clone(), &node).unwrap(),
        edit
    );
    assert_eq!(
        apply_text_edits(source.into(), vec![edit]).unwrap(),
        "Prelude\r\n\r\nSee {after}  tail.\r\n"
    );
    assert_eq!(
        replace_owned_inline(&parsed, range.start + 1..range.end, &node),
        Err(EditError::InvalidRange)
    );
    assert_eq!(
        replace_owned_inline(&parsed, range, &OwnedInline::Text("not an owner".into())),
        Err(EditError::GeneratedInvalid)
    );
}

#[test]
fn owned_inline_arguments_group_empty_and_multiword_arguments_without_changing_arity() {
    let parsed = parse("{before}\n");
    let node = OwnedInline::with_arguments(
        "",
        vec![
            vec![],
            vec![OwnedInline::Text("path with spaces.png".into())],
        ],
        vec![],
    );
    let edit = replace_owned_inline(&parsed, 0..8, &node).unwrap();
    assert_eq!(
        apply_text_edits(parsed.source.clone(), vec![edit]).unwrap(),
        "{{} {path with spaces.png}}\n"
    );
}

#[test]
fn inline_replacement_rejects_invalid_revision_and_cannot_cross_a_green_shard() {
    let invalid = parse("`node{broken\n");
    let node = OwnedInline::Element {
        kind: String::new(),
        members: vec![],
    };
    assert_eq!(
        replace_owned_inline(&invalid, 0..12, &node),
        Err(EditError::InvalidRange)
    );
    let green = GreenDocument::parse("{one}\n\n{two}\n");
    assert_eq!(
        replace_green_inline(&green, 0..12, &node),
        Err(EditError::InvalidRange)
    );
}
