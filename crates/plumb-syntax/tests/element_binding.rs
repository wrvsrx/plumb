use plumb_syntax::{parse, Block, Inline};

#[test]
fn adjacency_preserves_each_element_and_projects_direct_declarations() {
    let source = "`node{prefix`!{strong}suffix`@{stable}`={key first `@{nested} second}}\n";
    let parsed = parse(source);
    assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);

    let Block::Parsed(block) = &parsed.syntax.blocks[0] else {
        panic!("expected parsed block");
    };
    let Inline::Group {
        mark: Some(mark),
        content,
        ..
    } = &block.content.items[0]
    else {
        panic!("expected marked group");
    };
    assert_eq!(mark.marker, "node");
    assert_eq!(mark.attrs.id(), Some("stable"));
    assert_eq!(mark.attrs.value("key"), Some("first second"));
    assert_eq!(content.positional_elements().count(), 5);
    assert!(matches!(&content.items[0], Inline::Text { text, .. } if text == "prefix"));
    assert!(
        matches!(&content.items[1], Inline::Group { mark: Some(mark), .. } if mark.marker == "!")
    );
    assert!(matches!(&content.items[2], Inline::Text { text, .. } if text == "suffix"));
    assert!(
        matches!(&content.items[3], Inline::Group { mark: Some(mark), .. } if mark.marker == "@")
    );
}
