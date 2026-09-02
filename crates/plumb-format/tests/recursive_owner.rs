use plumb_format::format;

#[test]
fn canonicalizes_recursive_owners_and_spaces() {
    let source = "`node   head   value\n\n   `child{  nested   value  }\n";
    let formatted = format(source).unwrap();
    assert_eq!(formatted, "`node head value\n\n `child{nested value}\n");
    assert_eq!(format(&formatted).unwrap(), formatted);
}

#[test]
fn canonicalizes_continuations_but_preserves_anonymous_child_boundaries() {
    assert_eq!(
        format("`= title\n inline value\n").unwrap(),
        "`= title inline value\n"
    );
    assert_eq!(
        format("`= title\n\n block value\n").unwrap(),
        "`= title\n\n block value\n"
    );
}

#[test]
fn preserves_verbatim_payload_while_normalizing_margin() {
    let source = "`rust\"\n   fn main() {}\n    indented\n";
    let formatted = format(source).unwrap();
    assert_eq!(formatted, source);
    assert_eq!(format(&formatted).unwrap(), formatted);
}

#[test]
fn chooses_compact_and_strengthened_inline_verbatim() {
    assert_eq!(format("`$\"\"\n").unwrap(), "`$\"\"\n");
    assert_eq!(
        format("`$\"\"{a }\" b}\"\"\n").unwrap(),
        "`$\"\"{a }\" b}\"\"\n"
    );
}
