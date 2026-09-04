use lsp_types::{Position, Range};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineIndex {
    line_starts: Vec<usize>,
}

impl LineIndex {
    pub(crate) fn new(text: &str) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(
            text.match_indices('\n')
                .map(|(newline, _)| newline.saturating_add(1)),
        );
        Self { line_starts }
    }

    pub(crate) fn position_to_offset(&self, text: &str, position: Position) -> Option<usize> {
        let line = usize::try_from(position.line).ok()?;
        let line_start = *self.line_starts.get(line)?;
        let mut line_end = self
            .line_starts
            .get(line + 1)
            .map_or(text.len(), |next| next.saturating_sub(1));
        if line_end > line_start && text.as_bytes()[line_end - 1] == b'\r' {
            line_end -= 1;
        }

        let mut character = 0_u32;
        for (relative, value) in text[line_start..line_end].char_indices() {
            if character == position.character {
                return Some(line_start + relative);
            }
            character = character.checked_add(value.len_utf16() as u32)?;
            if character > position.character {
                return None;
            }
        }
        (character == position.character).then_some(line_end)
    }

    pub(crate) fn apply_edit(&mut self, replaced: std::ops::Range<usize>, replacement: &str) {
        let removed = replaced.end - replaced.start;
        let inserted = replacement.len();
        self.line_starts
            .retain(|start| *start <= replaced.start || *start > replaced.end);
        for start in &mut self.line_starts {
            if *start > replaced.end {
                *start = if inserted >= removed {
                    start.saturating_add(inserted - removed)
                } else {
                    start.saturating_sub(removed - inserted)
                };
            }
        }
        self.line_starts.extend(
            replacement
                .match_indices('\n')
                .map(|(newline, _)| replaced.start + newline + 1),
        );
        self.line_starts.sort_unstable();
        self.line_starts.dedup();
    }
}

pub(crate) struct PositionIndex<'a> {
    text: &'a str,
    line_starts: Vec<usize>,
}

impl<'a> PositionIndex<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        let line_starts = LineIndex::new(text).line_starts;
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

    #[test]
    fn strict_positions_reject_missing_lines_columns_and_surrogate_boundaries() {
        let text = "a😀\r\nb";
        let index = LineIndex::new(text);
        assert_eq!(index.position_to_offset(text, Position::new(0, 0)), Some(0));
        assert_eq!(index.position_to_offset(text, Position::new(0, 1)), Some(1));
        assert_eq!(index.position_to_offset(text, Position::new(0, 2)), None);
        assert_eq!(index.position_to_offset(text, Position::new(0, 3)), Some(5));
        assert_eq!(index.position_to_offset(text, Position::new(0, 4)), None);
        assert_eq!(index.position_to_offset(text, Position::new(1, 1)), Some(8));
        assert_eq!(index.position_to_offset(text, Position::new(2, 0)), None);
    }

    #[test]
    fn line_index_updates_match_a_fresh_index() {
        let mut text = "first\nsecond\nthird".to_string();
        let mut index = LineIndex::new(&text);
        for (range, replacement) in [(6..12, "two\nlines"), (0..0, "before\n")] {
            index.apply_edit(range.clone(), replacement);
            text.replace_range(range, replacement);
            assert_eq!(index, LineIndex::new(&text));
        }
        let end = text.len();
        index.apply_edit(end..end, "\nafter");
        text.replace_range(end..end, "\nafter");
        assert_eq!(index, LineIndex::new(&text));
    }
}
