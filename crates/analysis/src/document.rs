use {
    crate::{text::TextRange, tree::parse_rope},
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

    pub fn edit(&mut self, parser: &mut Parser, changes: &[DocumentChange]) {
        // UPDATE ROPE AND TREE
        // based on: https://github.com/marceline-cramer/saturn-v/blob/93d1c8fd02/lsp/src/lib.rs
        let (rope, tree) = (&mut self.rope, &mut self.tree);
        for change in changes {
            // Columns are UTF-8 byte offsets within their line (tree-sitter `Point` semantics),
            // so the change range maps directly onto byte offsets in the rope.
            let start_line = change.range.start.line_index;
            let start_column = change.range.start.character_index;
            let end_line = change.range.end.line_index;
            let end_column = change.range.end.character_index;

            let start_byte = rope.line_to_byte(start_line) + start_column;
            let old_end_byte = rope.line_to_byte(end_line) + end_column;
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
                start_position: Point::new(start_line, start_column),
                old_end_position: Point::new(end_line, end_column),
                new_end_position: Point::new(new_end_line, new_end_column),
            });
        }

        *tree = parse_rope(parser, rope, Some(tree)).expect("document reparse should succeed");
    }
}
