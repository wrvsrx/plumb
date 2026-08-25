use plumb_syntax::{Inline, InlineArgumentRef, InlineContent, InlineMember};

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
            kind, members, ..
        } => {
            if kind == "->" {
                if let Some(label) = members.iter().find_map(InlineMember::argument) {
                    append_argument(label, output);
                    return;
                }
            }
            for member in members {
                match member {
                    InlineMember::ParsedArgument(argument) => {
                        append_content(&argument.content, output);
                    }
                    InlineMember::VerbatimArgument(argument) => output.push_str(&argument.text),
                    InlineMember::Child { inline, .. } => append_inline(inline, output),
                }
            }
        }
    }
}

fn append_argument(argument: InlineArgumentRef<'_>, output: &mut String) {
    match argument {
        InlineArgumentRef::Parsed(content) => append_content(content, output),
        InlineArgumentRef::Verbatim(argument) => output.push_str(&argument.text),
    }
}

#[cfg(test)]
mod tests {
    use plumb_syntax::{parse, Block};

    use super::plain_text;

    #[test]
    fn standard_links_contribute_only_their_label_to_container_text() {
        let source = concat!(
            "`node Link `->[guide page|target.plumb] tail\n",
            "`node Interleaved `->[guide|@[main]|target.plumb] tail\n",
            "`node Generic `kind[first|second|note[child]] tail\n",
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
                "Link guide page tail",
                "Interleaved guide tail",
                "Generic firstsecondchild tail",
            ]
        );
    }
}
