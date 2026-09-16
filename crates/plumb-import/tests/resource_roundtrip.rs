use plumb_export::export;
use plumb_import::import_json;
use serde_json::json;

#[test]
fn resource_facets_round_trip_explicit_derived_empty_and_rich_labels_with_attributes() {
    for source in [
        "{static/derived.png `+{img}}\n",
        "{{} static/decorative.png `+{img}}\n",
        "{{Rich `!{alt}} {static/图 像.png} `+{img} `@{figure} `+{wide} `={loading lazy}}\n",
        "`->{{Demo `!{video}} {static/demo video.webm} `+{file} `@{demo} `+{wide} `={download yes}}\n",
        "`node{Picture static/photo.png `+{img}}\n",
    ] {
        let expected = export(source).unwrap();
        let imported = import_json(&expected.to_string()).unwrap();
        assert!(!imported.contains("`={src"), "{imported}");
        assert_eq!(export(&imported).unwrap(), expected, "{source}\n{imported}");
    }
}

#[test]
fn importing_resources_rejects_invalid_targets_and_conflicting_source_or_owner() {
    for (attrs, target) in [
        (json!(["", [], []]), ""),
        (json!(["", [], []]), "/absolute.png"),
        (json!(["", [], []]), "https://example.test/bad path.png"),
        (json!(["", [], [["src", "other.png"]]]), "picture.png"),
        (json!(["", ["file"], []]), "picture.png"),
        (json!(["", [], [["data-plumb-marker", "*"]]]), "picture.png"),
    ] {
        let document = json!({ "pandoc-api-version": [1,23,1], "meta": {}, "blocks": [
            {"t":"Para", "c":[{"t":"Image", "c":[attrs,[],[target,""]]}]}
        ] });
        assert!(import_json(&document.to_string()).is_err(), "{document}");
    }
}
