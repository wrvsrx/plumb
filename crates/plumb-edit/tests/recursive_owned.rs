use plumb_edit::{
    align_block_arguments, apply_text_edits, FormatScope, OwnedBlock, OwnedInline,
    OwnedInlineMember,
};
use plumb_syntax::parse;

#[test]
fn owned_syntax_renders_recursive_groups_and_space_arguments() {
    let block = OwnedBlock::Parsed {
        marker: Some("note".to_string()),
        head: vec![OwnedInline::Element {
            kind: "->".to_string(),
            members: vec![
                OwnedInlineMember::ParsedArgument(vec![OwnedInline::Element {
                    kind: String::new(),
                    members: vec![OwnedInlineMember::ParsedArgument(vec![OwnedInline::Text(
                        "guide page".to_string(),
                    )])],
                }]),
                OwnedInlineMember::ParsedArgument(vec![OwnedInline::Verbatim {
                    kind: String::new(),
                    text: "Project Guide.plumb".to_string(),
                }]),
            ],
        }],
        children: Vec::new(),
        raw: None,
    };
    let formatted = block.format().unwrap();
    assert_eq!(
        formatted,
        "`note `->{{guide page} `\"Project Guide.plumb\"}\n"
    );
    assert!(plumb_syntax::parse(&formatted).is_valid());
}

#[test]
fn owned_marked_raw_has_no_intermediate_payload_node() {
    let block = OwnedBlock::Parsed {
        marker: Some("rust".to_string()),
        head: Vec::new(),
        children: Vec::new(),
        raw: Some("fn main() {}\n".to_string()),
    };
    assert_eq!(block.format().unwrap(), "`rust\"\n fn main() {}\n");
}

#[test]
fn owned_children_follow_head_sensitive_spacing() {
    let child = OwnedBlock::Parsed {
        marker: Some("+".to_string()),
        head: vec![OwnedInline::Text("task".to_string())],
        children: Vec::new(),
        raw: None,
    };
    let with_head = OwnedBlock::Parsed {
        marker: Some("-".to_string()),
        head: vec![OwnedInline::Text("Task".to_string())],
        children: vec![child.clone()],
        raw: None,
    };
    let empty_head = OwnedBlock::Parsed {
        marker: Some("table".to_string()),
        head: Vec::new(),
        children: vec![child],
        raw: None,
    };
    assert_eq!(with_head.format().unwrap(), "`- Task\n\n `+ task\n");
    assert_eq!(empty_head.format().unwrap(), "`table\n `+ task\n");
}

#[test]
fn explicit_argument_alignment_is_a_format_fixed_point() {
    let source = "`= a one\n`= long two\n";
    let parsed = parse(source);
    let edits = align_block_arguments(&parsed, source.find("a one").unwrap()).unwrap();
    let aligned = apply_text_edits(source.to_string(), edits).unwrap();
    assert_eq!(aligned, "`= a    one\n`= long two\n");

    let parsed = parse(aligned);
    assert!(
        align_block_arguments(&parsed, parsed.source.find("a    one").unwrap())
            .unwrap()
            .is_empty()
    );
    assert!(plumb_edit::format(&parsed, FormatScope::Document)
        .unwrap()
        .is_empty());
}
