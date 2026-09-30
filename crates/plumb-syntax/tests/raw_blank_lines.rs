use plumb_syntax::{parse, Block, GreenDocument};

#[test]
fn intermediate_raw_blank_without_full_margin_preserves_all_bytes() {
    for ending in ["\n", "\r\n"] {
        for indent in [0, 1, 4] {
            for marker in ["", "plumb"] {
                for blank_width in 0..=indent + 1 {
                    let prefix = " ".repeat(indent);
                    let margin = " ".repeat(indent + 1);
                    let blank = " ".repeat(blank_width);
                    let parent = if indent == 0 {
                        String::new()
                    } else {
                        format!("`owner{ending}")
                    };
                    let source = format!(
                        "{parent}{prefix}`{marker}\"{ending}{margin}一{ending}{blank}{ending}{margin}二{ending}\n`after\n"
                    );
                    let parsed = parse(source.clone());
                    assert!(parsed.is_valid(), "{source:?}: {:?}", parsed.diagnostics);
                    let block = if indent == 0 {
                        &parsed.syntax.blocks[0]
                    } else {
                        let Block::Parsed(owner) = &parsed.syntax.blocks[0] else {
                            panic!("owner")
                        };
                        &owner.children[0]
                    };
                    let Block::Verbatim(raw) = block else {
                        panic!("raw")
                    };
                    let blank_payload = if blank_width <= indent {
                        blank.as_str()
                    } else {
                        ""
                    };
                    assert_eq!(
                        raw.text,
                        format!("一{ending}{blank_payload}{ending}二{ending}"),
                        "{source:?}"
                    );
                    assert_eq!(
                        &source[raw.text_range.clone()],
                        format!("{margin}一{ending}{blank}{ending}{margin}二{ending}")
                    );
                    assert_eq!(parsed.syntax.blocks.len(), 2);
                    assert_eq!(parsed.lossless.reconstruct(&source), source);
                    assert_eq!(GreenDocument::parse(source).materialize(), parsed);
                }
            }
        }
    }
}

#[test]
fn final_raw_blank_requires_margin_to_belong_to_payload() {
    for ending in ["\n", "\r\n"] {
        for suffix in ["", "`after\n"] {
            for (blank, payload) in [("", ""), (" ", ""), ("  ", ending)] {
                let source =
                    format!("`owner{ending} `\"{ending}  x{ending}{blank}{ending}{suffix}");
                let parsed = parse(source.clone());
                assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
                let Block::Parsed(owner) = &parsed.syntax.blocks[0] else {
                    panic!("owner")
                };
                let Block::Verbatim(raw) = &owner.children[0] else {
                    panic!("raw")
                };
                assert_eq!(raw.text, format!("x{ending}{payload}"));
                assert_eq!(parsed.lossless.reconstruct(&source), source);
                assert_eq!(GreenDocument::parse(source).materialize(), parsed);
            }
        }
    }
}
