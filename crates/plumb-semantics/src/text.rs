use plumb_syntax::{AttrItem, Attributes, Inline, InlineContent, InlineSlot};

pub(crate) fn plain_text(content: &InlineContent) -> String {
    let mut output = String::new();
    append_content(content, &mut output);
    output
}

fn append_content(content: &InlineContent, output: &mut String) {
    for inline in &content.items {
        append_inline(inline, output);
    }
}

fn append_inline(inline: &Inline, output: &mut String) {
    match inline {
        Inline::Text { text, .. } | Inline::Space { text, .. } | Inline::Verbatim { text, .. } => {
            output.push_str(text)
        }
        Inline::SoftBreak { .. } => output.push(' '),
        Inline::Element {
            kind, slots, attrs, ..
        } => {
            if kind == "->" {
                if let Some(label) = link_label(slots, attrs) {
                    for inline in label {
                        append_inline(inline, output);
                    }
                    return;
                }
            }
            for slot in slots {
                append_content(&slot.content, output);
            }
        }
    }
}

fn link_label<'a>(slots: &'a [InlineSlot], attrs: &Attributes) -> Option<&'a [Inline]> {
    let has_legacy_target = attrs
        .items
        .iter()
        .any(|item| matches!(item, AttrItem::Pair { key, .. } if key == "to"));
    match slots {
        [label] if has_legacy_target => Some(&label.content.items),
        [slot] if !has_legacy_target => {
            let [label, Inline::Space { .. }, target @ ..] = slot.content.items.as_slice() else {
                return None;
            };
            plain_target(target).then(|| std::slice::from_ref(label))
        }
        [label, target] if !has_legacy_target && plain_target(&target.content.items) => {
            Some(&label.content.items)
        }
        _ => None,
    }
}

fn plain_target(items: &[Inline]) -> bool {
    !items.is_empty()
        && items.iter().all(|inline| {
            matches!(
                inline,
                Inline::Text { .. } | Inline::Space { .. } | Inline::SoftBreak { .. }
            )
        })
}

#[cfg(test)]
mod tests {
    use plumb_syntax::{parse, Block};

    use super::plain_text;

    #[test]
    fn standard_links_contribute_only_their_label_to_container_text() {
        let source = concat!(
            "`node Expanded `->[guide page][target.plumb] tail\n",
            "`node Compact `->[guide target.plumb] tail\n",
            "`node Legacy `->[description]{`:[to target.plumb]} tail\n",
            "`node Generic `kind[first][second] tail\n",
        );
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let texts = parsed
            .syntax
            .blocks
            .iter()
            .map(|block| match block {
                Block::Parsed(block) => plain_text(&block.head),
                Block::Verbatim(_) => unreachable!(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            texts,
            [
                "Expanded guide page tail",
                "Compact guide tail",
                "Legacy description tail",
                "Generic firstsecond tail",
            ]
        );
    }
}
