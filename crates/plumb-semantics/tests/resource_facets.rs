use plumb_semantics::{
    analyze_document, embed_completion_context, link_completion_context,
    semantic_plain_text as plain_text,
};
use plumb_syntax::{parse, Block};

#[test]
fn embed_records_share_media_classification_and_never_create_graph_links() {
    use plumb_semantics::MediaKind::{Audio, Image, Video};
    let source = concat!(
        "`->{absent.PNG `+{embed}}\n",
        "`->{https://example.test/movie.MP4?name=.png#clip `+{embed}}\n",
        "`->{stream `+{embed} `={type audio/ogg}}\n",
        "`->{photo.png `+{embed} `={type application/pdf}}\n",
        "`->{other.plumb `+{embed}}\n",
        "`->{image.png `+{embed} `={type video/webm}}\n",
        "`->{manual.pdf}\n",
        "`->{old.png `+{img}}\n",
        "`->{old.mp4 `+{file}}\n",
    );
    let parsed = parse(source);
    let output = analyze_document(parsed.valid_syntax().unwrap());
    assert!(
        output.diagnostics().is_empty(),
        "{:?}",
        output.diagnostics()
    );
    let media = output
        .embeds()
        .iter()
        .map(|record| record.media.map(|media| media.kind))
        .collect::<Vec<_>>();
    assert_eq!(
        media,
        [
            Some(Image),
            Some(Video),
            Some(Audio),
            None,
            None,
            Some(Video)
        ]
    );
    assert_eq!(
        output.links().len(),
        3,
        "only ordinary links, including opaque old facets, are navigation records"
    );
}

#[test]
fn link_resource_facets_share_first_rest_binding_without_navigation_links() {
    let source = "See `->{{Alt text} static/a `*{b}.png `+{ embed } `@{figure} `+{wide}} and `->{{Demo video} static/demo.webm `+{embed}}.\n\n`->{static/derived.png `+{embed}}\n\n`->{{} static/decorative.png `+{embed}}\n";
    let parsed = parse(source);
    assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
    let output = analyze_document(parsed.valid_syntax().unwrap());
    assert!(
        output.diagnostics().is_empty(),
        "{:?}",
        output.diagnostics()
    );
    assert_eq!(output.embeds().len(), 4);
    assert!(
        output.links().is_empty(),
        "resource-faceted -> does not create navigation records"
    );
    assert_eq!(output.anchors().len(), 1);
    let image = output.embeds().get(0).unwrap();
    assert_eq!(image.source.value, "static/a b.png");
    assert_eq!(&source[image.source.range.clone()], "static/a `*{b}.png");
    assert_eq!(
        output.embeds().get(1).unwrap().source.value,
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
fn embeds_reject_duplicate_target_source_and_incompatible_owner() {
    let source = "`->{path.png `+{embed} `={src other.png}}\n`*{path.png `+{embed}}\n`->{`+{embed}}\n`->{label {} `+{embed}}\n`img{Old `={src old.png}}\n`file{Old `={src old.webm}}\n";
    let parsed = parse(source);
    let output = analyze_document(parsed.valid_syntax().unwrap());
    assert!(
        output.embeds().is_empty(),
        "old marker spelling is generic, not implicit resource semantics"
    );
    assert_eq!(
        output
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        [
            "resource.unexpected-source",
            "resource.invalid-owner",
            "embed.missing-source",
            "embed.missing-source",
        ]
    );
}

#[test]
fn resource_facets_require_parsed_links_for_projection_text_and_completion() {
    for prefix in ["", "`()", "`node", "`*", "`cite", "`=", "`img", "`file"] {
        for facet in ["embed"] {
            let source = format!("{prefix}{{label path.png `+{{{facet}}}}}\n");
            let parsed = parse(&source);
            let output = analyze_document(parsed.valid_syntax().unwrap());
            assert!(output.embeds().is_empty(), "{source}");
            assert!(output.links().is_empty(), "{source}");
            assert!(
                output
                    .diagnostics()
                    .iter()
                    .any(|d| d.code == "resource.invalid-owner"),
                "{source}"
            );
            let offset = source.find("path.png").unwrap() + 4;
            assert!(
                embed_completion_context(&parsed, offset).is_none(),
                "{source}"
            );
            if prefix.is_empty() || prefix == "`()" || prefix == "`node" {
                let Block::Parsed(block) = &parsed.syntax.blocks[0] else {
                    unreachable!()
                };
                assert_eq!(plain_text(&block.content), "label path.png");
            }
        }
    }
}

#[test]
fn resource_completion_uses_positional_target_and_keeps_hash_literal() {
    for facet in ["embed"] {
        for source in [
            format!("`->{{`+{{{facet}}} Alt static/a#b|.png}}\n"),
            format!("`->{{`+{{{facet}}} Alt static/a#b|"),
        ] {
            let offset = source.find('|').unwrap();
            let source = source.replace('|', "");
            let parsed = parse(&source);
            let context = embed_completion_context(&parsed, offset).unwrap();
            assert_eq!(context.query, "static/a#b");
            assert_eq!(context.replace.start, source.find("static/a#b").unwrap());
            assert!(link_completion_context(&parsed, offset).is_none());
        }
    }
}
