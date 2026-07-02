use {
    crate::lsp_types::{Position, PositionEncodingKind, Range},
    analysis::{TextPosition, TextRange},
    ropey::Rope,
};

// The analysis crate addresses text with tree-sitter `Point` semantics: a line index plus a UTF-8
// byte offset within that line. LSP positions instead count units of the negotiated encoding
// (UTF-16 code units by default, optionally UTF-8 bytes), so every position crossing the LSP
// boundary is converted here against the document rope, which is the only place both the encoding
// and the text are available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionEncoding {
    Utf8,
    Utf16,
}

impl PositionEncoding {
    // UTF-8 maps one-to-one onto tree-sitter byte offsets, so we prefer it when the client offers
    // it and otherwise fall back to UTF-16, which every client supports.
    pub fn negotiate(client_offered: Option<&[PositionEncodingKind]>) -> Self {
        match client_offered {
            Some(encodings) if encodings.contains(&PositionEncodingKind::UTF8) => Self::Utf8,
            _ => Self::Utf16,
        }
    }

    pub fn kind(self) -> PositionEncodingKind {
        match self {
            Self::Utf8 => PositionEncodingKind::UTF8,
            Self::Utf16 => PositionEncodingKind::UTF16,
        }
    }
}

// Clients may send positions past the end of a line or past the last line (stale requests racing
// an edit are protocol-legal), so both conversions clamp to the document: the line to the last
// line, the column to that line's length. Nothing past-EOF ever reaches analysis or the client.
pub fn lsp_position_to_internal(
    rope: &Rope,
    encoding: PositionEncoding,
    position: Position,
) -> TextPosition {
    // A rope always has at least one (possibly empty) line, so the clamped index is valid.
    let line_index = (position.line as usize).min(rope.len_lines().saturating_sub(1));
    let line = rope.line(line_index);
    let unit = position.character as usize;
    let character_index = match encoding {
        PositionEncoding::Utf8 => unit.min(line.len_bytes()),
        PositionEncoding::Utf16 => {
            let utf16 = unit.min(line.len_utf16_cu());
            line.char_to_byte(line.utf16_cu_to_char(utf16))
        }
    };
    TextPosition {
        line_index,
        character_index,
    }
}

pub fn internal_position_to_lsp(
    rope: &Rope,
    encoding: PositionEncoding,
    position: TextPosition,
) -> Position {
    let line_index = position.line_index.min(rope.len_lines().saturating_sub(1));
    let line = rope.line(line_index);
    let byte = position.character_index.min(line.len_bytes());
    let character = match encoding {
        PositionEncoding::Utf8 => byte,
        PositionEncoding::Utf16 => line.char_to_utf16_cu(line.byte_to_char(byte)),
    };
    Position::new(line_index as u32, character as u32)
}

pub fn lsp_range_to_internal(rope: &Rope, encoding: PositionEncoding, range: Range) -> TextRange {
    TextRange {
        start: lsp_position_to_internal(rope, encoding, range.start),
        end: lsp_position_to_internal(rope, encoding, range.end),
    }
}

pub fn internal_range_to_lsp(rope: &Rope, encoding: PositionEncoding, range: TextRange) -> Range {
    Range::new(
        internal_position_to_lsp(rope, encoding, range.start),
        internal_position_to_lsp(rope, encoding, range.end),
    )
}

pub fn tree_sitter_range_to_lsp(
    rope: &Rope,
    encoding: PositionEncoding,
    range: tree_sitter::Range,
) -> Range {
    internal_range_to_lsp(
        rope,
        encoding,
        TextRange {
            start: TextPosition {
                line_index: range.start_point.row,
                character_index: range.start_point.column,
            },
            end: TextPosition {
                line_index: range.end_point.row,
                character_index: range.end_point.column,
            },
        },
    )
}
