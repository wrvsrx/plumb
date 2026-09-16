use plumb_semantics::{
    analyze_document, file_completion_context, image_completion_context, link_completion_context,
    semantic_plain_text as plain_text,
};
use plumb_syntax::{parse, Block};

#[test]
fn anonymous_and_marked_resource_facets_share_first_rest_binding_without_navigation_links() {
    let source = "See {{Alt text} static/a `*{b}.png `+{ img } `@{figure} `+{wide}} and `->{{Demo video} static/demo.webm `+{file}}.\n{static/derived.png `+{img}}\n{{} static/decorative.png `+{img}}\n";
    let parsed = parse(source);
    assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
    let output = analyze_document(parsed.valid_syntax().unwrap());
    assert!(
        output.diagnostics().is_empty(),
        "{:?}",
        output.diagnostics()
    );
    assert_eq!(output.images().len(), 3);
    assert_eq!(output.files().len(), 1);
    assert!(
        output.links().is_empty(),
        "resource-faceted -> does not create navigation records"
    );
    assert_eq!(output.anchors().len(), 1);
    let image = output.images().get(0).unwrap();
    assert_eq!(image.source.value, "static/a b.png");
    assert_eq!(&source[image.source.range.clone()], "static/a `*{b}.png");
    assert_eq!(
        output.files().get(0).unwrap().source.value,
        "static/demo.webm"
    );
    let texts = parsed
        .syntax
        .blocks
        .iter()
        .map(|block| match block {
            Block::Parsed(block) => plain_text(&block.content),
            _ => unreachable!(),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        texts,
        ["See Alt text and Demo video.", "static/derived.png", ""]
    );
}

#[test]
fn resource_facets_reject_conflicts_duplicate_target_source_and_incompatible_owner() {
    let source = "{path.png `+{img} `+{file}}\n{path.png `+{img} `={src other.png}}\n`*{path.png `+{img}}\n{`+{img}}\n{label {} `+{file}}\n`img{Old `={src old.png}}\n`file{Old `={src old.webm}}\n";
    let parsed = parse(source);
    let output = analyze_document(parsed.valid_syntax().unwrap());
    assert!(output.images().is_empty());
    assert!(
        output.files().is_empty(),
        "old marker spelling is generic, not implicit resource semantics"
    );
    assert_eq!(
        output
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        [
            "resource.conflicting-facets",
            "resource.unexpected-source",
            "resource.invalid-owner",
            "image.missing-source",
            "file.missing-source",
        ]
    );
}

#[test]
fn resource_completion_uses_positional_target_and_keeps_hash_literal() {
    for (facet, image) in [("img", true), ("file", false)] {
        for source in [
            format!("{{`+{{{facet}}} Alt static/a#b|.png}}\n"),
            format!("{{`+{{{facet}}} Alt static/a#b|"),
        ] {
            let offset = source.find('|').unwrap();
            let source = source.replace('|', "");
            let parsed = parse(&source);
            let context = if image {
                image_completion_context(&parsed, offset)
            } else {
                file_completion_context(&parsed, offset)
            }
            .unwrap();
            assert_eq!(context.query, "static/a#b");
            assert_eq!(context.replace.start, source.find("static/a#b").unwrap());
            assert!(link_completion_context(&parsed, offset).is_none());
        }
    }
}
