//! Line/column addressing over a document's text. Analysis speaks absolute
//! byte offsets; the CLI renders 1-based line and byte-column pairs, and the
//! language server converts to the negotiated LSP encoding (UTF-16 code units
//! or UTF-8 bytes) at the protocol edge — always against the target
//! document's own text.

use syntax::TextSize;

/// Newline table for one document: offset ↔ line/column in O(log lines).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex {
    /// Byte offset of each line's first byte; index 0 is always 0.
    line_starts: Vec<u32>,
    text_len: u32,
}

/// A zero-based line paired with a zero-based column in some unit (bytes or
/// UTF-16 code units, per the function that produced it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineColumn {
    pub line: u32,
    pub column: u32,
}

impl LineIndex {
    pub fn new(text: &str) -> LineIndex {
        let mut line_starts = vec![0u32];
        for (offset, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(offset as u32 + 1);
            }
        }
        LineIndex {
            line_starts,
            text_len: text.len() as u32,
        }
    }

    pub fn line_count(&self) -> u32 {
        self.line_starts.len() as u32
    }

    /// The byte offset of `line`'s first byte, clamped to the last line.
    pub fn line_start(&self, line: u32) -> u32 {
        let line = (line as usize).min(self.line_starts.len() - 1);
        self.line_starts[line]
    }

    /// The byte length of `line`, excluding its terminator.
    pub fn line_length(&self, line: u32, text: &str) -> u32 {
        let start = self.line_start(line);
        let end = self
            .line_starts
            .get(line as usize + 1)
            .copied()
            .unwrap_or(self.text_len);
        let line_text = &text[start as usize..end as usize];
        line_text.trim_end_matches(['\n', '\r']).len() as u32
    }

    /// Zero-based line and byte column of a byte offset (clamped to the text).
    pub fn line_column(&self, offset: TextSize) -> LineColumn {
        let offset = u32::from(offset).min(self.text_len);
        let line = self
            .line_starts
            .partition_point(|&start| start <= offset)
            .saturating_sub(1);
        LineColumn {
            line: line as u32,
            column: offset - self.line_starts[line],
        }
    }

    /// Zero-based line and UTF-16 column of a byte offset.
    pub fn line_column_utf16(&self, offset: TextSize, text: &str) -> LineColumn {
        let position = self.line_column(offset);
        let start = self.line_starts[position.line as usize] as usize;
        let column = text[start..start + position.column as usize]
            .chars()
            .map(|character| character.len_utf16() as u32)
            .sum();
        LineColumn {
            line: position.line,
            column,
        }
    }

    /// The byte offset of a zero-based line and byte column, both clamped.
    pub fn offset(&self, position: LineColumn, text: &str) -> TextSize {
        let line = position.line.min(self.line_count() - 1);
        let start = self.line_start(line);
        let column = position.column.min(self.line_length(line, text));
        // Never split a UTF-8 sequence: back up to the previous boundary.
        let mut offset = (start + column) as usize;
        while offset > 0 && !text.is_char_boundary(offset) {
            offset -= 1;
        }
        TextSize::from(offset as u32)
    }

    /// The byte offset of a zero-based line and UTF-16 column, both clamped.
    pub fn offset_utf16(&self, position: LineColumn, text: &str) -> TextSize {
        let line = position.line.min(self.line_count() - 1);
        let start = self.line_start(line) as usize;
        let line_end = start + self.line_length(line, text) as usize;
        let mut units = 0u32;
        for (index, character) in text[start..line_end].char_indices() {
            if units >= position.column {
                return TextSize::from((start + index) as u32);
            }
            units += character.len_utf16() as u32;
        }
        TextSize::from(line_end as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_columns_round_trip() {
        let text = "abc\ndef\n";
        let index = LineIndex::new(text);
        assert_eq!(index.line_count(), 3);
        let position = index.line_column(TextSize::from(5));
        assert_eq!((position.line, position.column), (1, 1));
        assert_eq!(index.offset(position, text), TextSize::from(5));
    }

    #[test]
    fn utf16_columns_count_code_units() {
        // 'é' is 2 UTF-8 bytes / 1 UTF-16 unit; '😀' is 4 bytes / 2 units.
        let text = "é😀x\n";
        let index = LineIndex::new(text);
        let x_offset = TextSize::from(6);
        let utf16 = index.line_column_utf16(x_offset, text);
        assert_eq!((utf16.line, utf16.column), (0, 3));
        assert_eq!(index.offset_utf16(utf16, text), x_offset);
    }

    #[test]
    fn out_of_bounds_positions_clamp() {
        let text = "ab\n";
        let index = LineIndex::new(text);
        assert_eq!(
            index.offset(LineColumn { line: 9, column: 9 }, text),
            TextSize::from(3)
        );
        let position = index.line_column(TextSize::from(99));
        assert_eq!((position.line, position.column), (1, 0));
    }
}
