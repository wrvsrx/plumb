use lsp_types::{Position, Range};

pub(crate) struct PositionIndex<'a> {
    text: &'a str,
    line_starts: Vec<usize>,
}

impl<'a> PositionIndex<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(
            text.match_indices('\n')
                .map(|(newline, _)| newline.saturating_add(1)),
        );
        Self { text, line_starts }
    }

    pub(crate) fn byte_range_to_lsp(&self, range: &std::ops::Range<usize>) -> Range {
        Range::new(
            self.offset_to_position(range.start),
            self.offset_to_position(range.end),
        )
    }

    fn offset_to_position(&self, offset: usize) -> Position {
        let offset = offset.min(self.text.len());
        let line = self.line_starts.partition_point(|start| *start <= offset) - 1;
        let line_start = self.line_starts[line];
        let mut character = 0;
        for (index, value) in self.text[line_start..].char_indices() {
            if line_start + index >= offset || value == '\n' {
                break;
            }
            character += value.len_utf16() as u32;
        }
        Position::new(line as u32, character)
    }
}

pub(crate) fn byte_range_to_lsp(text: &str, range: &std::ops::Range<usize>) -> Range {
    Range::new(
        offset_to_position(text, range.start),
        offset_to_position(text, range.end),
    )
}

pub(crate) fn position_to_offset(text: &str, position: Position) -> usize {
    let mut line = 0;
    let mut character = 0;
    for (index, value) in text.char_indices() {
        if line == position.line && character >= position.character {
            return index;
        }
        if value == '\n' {
            if line == position.line {
                return index;
            }
            line += 1;
            character = 0;
        } else {
            character += value.len_utf16() as u32;
        }
    }
    text.len()
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
        let index = PositionIndex::new("a学习\nb");
        assert_eq!(index.offset_to_position(7), Position::new(0, 3));
        assert_eq!(index.offset_to_position(8), Position::new(1, 0));
    }

    #[test]
    fn clamps_offsets_and_preserves_existing_positions_inside_utf8_scalars() {
        let index = PositionIndex::new("a学\r\nb");
        assert_eq!(index.offset_to_position(2), Position::new(0, 2));
        assert_eq!(index.offset_to_position(usize::MAX), Position::new(1, 1));
    }

    #[test]
    fn reuses_line_starts_for_ranges_across_a_document() {
        let text = (0..10_000)
            .map(|line| format!("line {line} 学习\n"))
            .collect::<String>();
        let index = PositionIndex::new(&text);
        for line in [0, 1, 4_999, 9_999] {
            let start = index.line_starts[line];
            let range = index.byte_range_to_lsp(&(start..start + 4));
            assert_eq!(range.start, Position::new(line as u32, 0));
            assert_eq!(range.end, Position::new(line as u32, 4));
        }
    }
}
