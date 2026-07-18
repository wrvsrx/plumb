use lsp_types::{Position, Range};

pub(crate) fn byte_range_to_lsp(text: &str, range: &std::ops::Range<usize>) -> Range {
    Range::new(
        offset_to_position(text, range.start),
        offset_to_position(text, range.end),
    )
}

fn offset_to_position(text: &str, offset: usize) -> Position {
    let mut line = 0;
    let mut character = 0;
    for (index, value) in text.char_indices() {
        if index >= offset {
            break;
        }
        if value == '\n' {
            line += 1;
            character = 0;
        } else {
            character += value.len_utf16() as u32;
        }
    }
    Position::new(line, character)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_utf8_offsets_to_utf16_positions() {
        assert_eq!(offset_to_position("a学习\nb", 7), Position::new(0, 3));
        assert_eq!(offset_to_position("a学习\nb", 8), Position::new(1, 0));
    }
}
