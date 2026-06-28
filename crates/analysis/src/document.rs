use {
    crate::{
        text::{TextPosition, TextRange, line_character_to_byte_index},
        tree::parse_rope,
    },
    ropey::Rope,
    tree_sitter::{InputEdit, Parser, Point, Tree},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocumentId(pub u32);

#[derive(Debug, Clone)]
pub struct Document {
    rope: Rope,
    tree: Tree,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentChange {
    pub range: TextRange,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentParseError {
    ParseFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentEditError {
    // A change range was stale or malformed relative to the live rope: a line past the document, a
    // column past the line, `start > end`, or an offset that lands inside a multi-byte codepoint.
    // Applying it as-is would index out of bounds in the rope, so the edit is rejected instead. The
    // caller decides the policy — the LSP sync path treats this as an unrecoverable desync and
    // panics, while best-effort callers (e.g. fuzzing) can tolerate it.
    InvalidRange,
}

impl Document {
    pub fn parse(parser: &mut Parser, source: &str) -> Result<Self, DocumentParseError> {
        let rope = Rope::from_str(source);
        let tree = parse_rope(parser, &rope, None).ok_or(DocumentParseError::ParseFailed)?;
        Ok(Self { rope, tree })
    }

    pub fn new(rope: Rope, tree: Tree) -> Self {
        Self { rope, tree }
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    pub fn edit(
        &mut self,
        parser: &mut Parser,
        changes: &[DocumentChange],
    ) -> Result<(), DocumentEditError> {
        // UPDATE ROPE AND TREE
        // based on: https://github.com/marceline-cramer/saturn-v/blob/93d1c8fd02/lsp/src/lib.rs
        // Each change's range is resolved against the rope *after* the previous changes in the batch
        // have been applied. A stale or malformed range is rejected before it mutates the rope; any
        // earlier changes in the batch stay applied and the tree is still reparsed below so the
        // document never escapes in a half-edited (rope/tree-desynced) state.
        let (rope, tree) = (&mut self.rope, &mut self.tree);
        let mut result = Ok(());
        for change in changes {
            // Columns are UTF-8 byte offsets within their line (tree-sitter `Point` semantics), so a
            // valid position maps directly onto a byte offset in the rope.
            let Some(start_byte) = resolve_edit_byte(rope, change.range.start) else {
                result = Err(DocumentEditError::InvalidRange);
                break;
            };
            let Some(old_end_byte) = resolve_edit_byte(rope, change.range.end) else {
                result = Err(DocumentEditError::InvalidRange);
                break;
            };
            if start_byte > old_end_byte {
                result = Err(DocumentEditError::InvalidRange);
                break;
            }
            let new_end_byte = start_byte + change.text.len();

            let start_character = rope.byte_to_char(start_byte);
            let end_character = rope.byte_to_char(old_end_byte);

            rope.remove(start_character..end_character);
            rope.insert(start_character, &change.text);

            let new_end_line = rope.byte_to_line(new_end_byte);
            let new_end_column = new_end_byte - rope.line_to_byte(new_end_line);

            tree.edit(&InputEdit {
                start_byte,
                old_end_byte,
                new_end_byte,
                start_position: Point::new(
                    change.range.start.line_index,
                    change.range.start.character_index,
                ),
                old_end_position: Point::new(
                    change.range.end.line_index,
                    change.range.end.character_index,
                ),
                new_end_position: Point::new(new_end_line, new_end_column),
            });
        }

        *tree = parse_rope(parser, rope, Some(tree)).expect("document reparse should succeed");
        result
    }
}

// Resolve a text position to an absolute rope byte offset, returning `None` for any out-of-bounds or
// mid-codepoint position so the edit path can reject it instead of panicking deep inside the rope.
fn resolve_edit_byte(rope: &Rope, position: TextPosition) -> Option<usize> {
    let byte = line_character_to_byte_index(rope, position)?;
    // `line_character_to_byte_index` bounds the column by the line's byte length but does not require
    // it to fall on a char boundary; a column landing inside a multi-byte codepoint would desync the
    // rope's byte/char mapping, so reject it here.
    let character = rope.try_byte_to_char(byte).ok()?;
    (rope.char_to_byte(character) == byte).then_some(byte)
}
