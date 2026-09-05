use plumb_format::{format, format_block_range};
use plumb_syntax::parse;

#[test]
fn canonicalizes_recursive_owners_and_spaces() {
    let source = "`node   head   value\n\n   `child{  nested   value  }\n";
    let formatted = format(source).unwrap();
    assert_eq!(
        formatted,
        "`node head   value\n\n `child{  nested   value  }\n"
    );
    assert_eq!(format(&formatted).unwrap(), formatted);
}

#[test]
fn preserves_continuations_and_normalizes_their_structural_indentation() {
    assert_eq!(
        format("`= title\n    inline   value\n").unwrap(),
        "`= title\n inline   value\n"
    );
    assert_eq!(
        format("`= title\n\n block value\n").unwrap(),
        "`= title\n\n block value\n"
    );

    let crlf = "`node\r\n    head   value\r\n";
    let parsed = parse(crlf);
    assert_eq!(format(crlf).unwrap(), "`node\r\n head   value\r\n");
    let edit = format_block_range(crlf, parsed.syntax.blocks[0].range().clone()).unwrap();
    assert_eq!(edit.new_text, "`node\r\n head   value\r\n");
}

#[test]
fn separates_visible_heads_but_compacts_empty_head_containers() {
    assert_eq!(
        format("`- Task\n `+ task\n").unwrap(),
        "`- Task\n\n `+ task\n"
    );
    assert_eq!(format("`table\n `- row\n").unwrap(), "`table\n `- row\n");
    assert_eq!(
        format("`container\n\n child\n").unwrap(),
        "`container\n\n child\n"
    );
}

#[test]
fn preserves_all_existing_block_head_spacing() {
    let aligned = "`= a    one\n`= long two\n";
    assert_eq!(format(aligned).unwrap(), aligned);

    let ordinary = "`= a one\n`= long two\n";
    assert_eq!(format(ordinary).unwrap(), ordinary);

    let inconsistent = "`= a   one\n`= long two\n";
    assert_eq!(format(inconsistent).unwrap(), inconsistent);

    let adjacent_alignment_groups = concat!(
        "`= title   2026-09-04\n",
        "`= created 2026-09-05T16:31:21+08:00\n",
        "`= date     2026-09-04\n",
        "`= timezone +08:00\n",
    );
    assert_eq!(
        format(adjacent_alignment_groups).unwrap(),
        adjacent_alignment_groups
    );

    let parsed = parse(aligned);
    let mut range_formatted = aligned.to_string();
    let edit = format_block_range(aligned, parsed.syntax.blocks[0].range().clone()).unwrap();
    range_formatted.replace_range(edit.range, &edit.new_text);
    assert_eq!(range_formatted, aligned);
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
