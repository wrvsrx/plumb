use plumb_export::export;
use plumb_import::import_json;
use serde_json::json;

#[test]
fn resource_facets_round_trip_explicit_derived_empty_and_rich_labels_with_attributes() {
    for source in [
        "`->{static/derived.png `+{embed}}\n",
        "`->{{} static/decorative.png `+{embed}}\n",
        "`->{{Rich `!{alt}} {static/图 像.png} `+{embed} `@{figure} `+{wide} `={loading lazy}}\n",
        "`->{{Demo `!{video}} {static/demo video.webm} `+{embed} `@{demo} `+{wide} `={download yes}}\n",
        "`->{Picture static/photo.png `+{embed}}\n",
        "`->{stream https://example.test/stream `+{embed} `={type {Audio/OGG; codecs=opus}}}\n",
        "`->{override static/photo.png `+{embed} `={type video/webm}}\n",
        "`->{unknown static/photo.png `+{embed} `={type image/unknown}}\n",
        "`->{unknown static/photo.png `+{embed} `={type {}}}\n",
        "`->{manual static/manual.pdf `+{embed}}\n",
    ] {
        let expected = export(source).unwrap();
        let imported = import_json(&expected.to_string()).unwrap();
        assert!(imported.starts_with("`->{"), "{imported}");
        assert!(!imported.contains("`={src"), "{imported}");
        assert_eq!(export(&imported).unwrap(), expected, "{source}\n{imported}");
    }
}

#[test]
fn plain_downloads_stay_plain_and_native_images_gain_embed_intent() {
    let plain = export("`->{Download static/manual.pdf}\n`->{Photo static/photo.png `={type image/png}}\n").unwrap();
    let imported = import_json(&plain.to_string()).unwrap();
    assert!(!imported.contains("`+{embed}"));
    assert_eq!(export(&imported).unwrap(), plain);
    let document = json!({ "pandoc-api-version": [1,23,1], "meta": {}, "blocks": [
        {"t":"Para", "c":[{"t":"Image", "c":[["",[],[["type","image/png"]]],[],["https://example.test/stream",""]]}]}
    ] });
    let imported = import_json(&document.to_string()).unwrap();
    assert!(imported.contains("`+{embed}"));
    let exported = export(&imported).unwrap();
    assert_eq!(exported["blocks"][0]["c"][0]["t"], "Image");
    assert_eq!(
        exported["blocks"][0]["c"][0]["c"][0][2],
        json!([["type", "image/png"], ["data-plumb-facet", "embed"]])
    );
}

#[test]
fn importing_resources_rejects_invalid_targets_and_conflicting_source_or_owner() {
    for (attrs, target) in [
        (json!(["", [], []]), ""),
        (json!(["", [], []]), "/absolute.png"),
        (json!(["", [], []]), "https://example.test/bad path.png"),
        (json!(["", [], [["src", "other.png"]]]), "picture.png"),
        (json!(["", [], [["data-plumb-marker", "*"]]]), "picture.png"),
        (
            json!(["", [], [["data-plumb-marker", "node"]]]),
            "picture.png",
        ),
        (
            json!(["", [], [["data-plumb-marker", "()"]]]),
            "picture.png",
        ),
        (json!(["", [], [["data-plumb-marker", ""]]]), "picture.png"),
    ] {
        let document = json!({ "pandoc-api-version": [1,23,1], "meta": {}, "blocks": [
            {"t":"Para", "c":[{"t":"Image", "c":[attrs,[],[target,""]]}]}
        ] });
        assert!(import_json(&document.to_string()).is_err(), "{document}");
    }
}
