use plumb_syntax::{Inline, InlineContent};

pub fn plain_text(content: &InlineContent) -> String {
    let mut output = String::new();
    append_content(&content.trim_boundary_padding(), &mut output);
    output
}

fn append_content(content: &InlineContent, output: &mut String) {
    let view = crate::owner_semantic_view(content);
    if let Some(content) = view.visible_content() {
        for inline in &content.items {
            append_inline(inline, output);
        }
    }
}

fn append_inline(inline: &Inline, output: &mut String) {
    match inline {
        Inline::Text { text, .. } | Inline::Verbatim { text, .. } => output.push_str(text),
        Inline::Space { .. } | Inline::SoftBreak { .. } => output.push(' '),
        Inline::Group { mark, content, .. } => {
            if mark.as_ref().is_some_and(|mark| mark.marker == "->") {
                let view = crate::owner_semantic_view(content);
                if let Some(arguments) = view.split_first() {
                    append_content(arguments.first, output);
                }
                return;
            }
            append_content(content, output);
        }
    }
}

#[cfg(test)]
mod tests {
    use plumb_syntax::{parse, Block};

    use super::plain_text;

    #[test]
    fn standard_links_contribute_only_their_label_to_container_text() {
        let source = concat!(
            "`node Link `->{{guide page} target.plumb} tail\n",
            "`node Declared `->{`@{link-id} {guide page} Project Guide.plumb} tail\n",
            "`node Generic `kind{first second `note{child}} tail\n",
        );
        let parsed = parse(source);
        assert!(parsed.is_valid(), "{:?}", parsed.diagnostics);
        let texts = parsed
            .syntax
            .blocks
            .iter()
            .map(|block| match block {
                Block::Parsed(block) => plain_text(&block.content),
                Block::Verbatim(_) => unreachable!(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            texts,
            [
                "Link guide page tail",
                "Declared guide page tail",
                "Generic first second child tail"
            ]
        );
    }
}

pub(crate) fn shift_inline_content(content: &mut InlineContent, delta: isize) {
    let mut pending = vec![content];
    while let Some(content) = pending.pop() {
        shift_range(&mut content.range, delta);
        for inline in &mut content.items {
            match inline {
                Inline::Text { range, .. }
                | Inline::Space { range, .. }
                | Inline::SoftBreak { range } => shift_range(range, delta),
                Inline::Group {
                    range,
                    mark,
                    content,
                } => {
                    shift_range(range, delta);
                    if let Some(mark) = mark {
                        shift_range(&mut mark.range, delta);
                        shift_range(&mut mark.marker_range, delta);
                        shift_attributes(&mut mark.attrs, delta);
                    }
                    pending.push(content);
                }
                Inline::Verbatim {
                    range,
                    mark,
                    text_range,
                    ..
                } => {
                    shift_range(range, delta);
                    if let Some(mark) = mark {
                        shift_range(&mut mark.range, delta);
                        shift_range(&mut mark.marker_range, delta);
                        shift_attributes(&mut mark.attrs, delta);
                    }
                    shift_range(text_range, delta);
                }
            }
        }
    }
}

fn shift_attributes(attributes: &mut plumb_syntax::Attributes, delta: isize) {
    if let Some(range) = &mut attributes.range {
        shift_range(range, delta);
    }
    for item in &mut attributes.items {
        match item {
            plumb_syntax::AttrItem::Id {
                value_range, range, ..
            }
            | plumb_syntax::AttrItem::Class {
                value_range, range, ..
            } => {
                shift_range(value_range, delta);
                shift_range(range, delta);
            }
            plumb_syntax::AttrItem::Pair {
                key_range,
                value,
                range,
                ..
            } => {
                shift_range(key_range, delta);
                shift_range(&mut value.range, delta);
                shift_range(range, delta);
            }
        }
    }
}

fn shift_range(range: &mut std::ops::Range<usize>, delta: isize) {
    range.start = range.start.checked_add_signed(delta).unwrap();
    range.end = range.end.checked_add_signed(delta).unwrap();
}
