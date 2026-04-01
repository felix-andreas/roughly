use {
    crate::{
        text::{TextRange, line_character_to_character_index},
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

    pub(crate) fn tree_mut(&mut self) -> &mut Tree {
        &mut self.tree
    }

    pub(crate) fn rope_mut(&mut self) -> &mut Rope {
        &mut self.rope
    }

    pub fn edit_range(
        &mut self,
        parser: &mut Parser,
        range: TextRange,
        replacement_text: &str,
    ) -> Result<(), DocumentEditError> {
        let start_character = line_character_to_character_index(self.rope(), range.start)
            .ok_or(DocumentEditError::InvalidRange)?;
        let end_character = line_character_to_character_index(self.rope(), range.end)
            .ok_or(DocumentEditError::InvalidRange)?;

        if start_character > end_character {
            return Err(DocumentEditError::InvalidRange);
        }

        let start_byte = self
            .rope()
            .try_char_to_byte(start_character)
            .map_err(|_| DocumentEditError::InvalidRange)?;
        let old_end_byte = self
            .rope()
            .try_char_to_byte(end_character)
            .map_err(|_| DocumentEditError::InvalidRange)?;

        self.rope_mut().remove(start_character..end_character);
        self.rope_mut().insert(start_character, replacement_text);

        let new_end_byte = start_byte + replacement_text.len();
        let new_end_line = self
            .rope()
            .try_byte_to_line(new_end_byte)
            .map_err(|_| DocumentEditError::InvalidRange)?;
        let line_start_character = self
            .rope()
            .try_line_to_char(new_end_line)
            .map_err(|_| DocumentEditError::InvalidRange)?;
        let new_end_character = self
            .rope()
            .try_byte_to_char(new_end_byte)
            .map_err(|_| DocumentEditError::InvalidRange)?;

        self.tree_mut().edit(&InputEdit {
            start_byte,
            old_end_byte,
            new_end_byte,
            start_position: Point::new(range.start.line_index, range.start.character_index),
            old_end_position: Point::new(range.end.line_index, range.end.character_index),
            new_end_position: Point::new(
                new_end_line,
                new_end_character.saturating_sub(line_start_character),
            ),
        });

        let previous_tree = self.tree().clone();
        let reparsed_tree = parse_rope(parser, self.rope(), Some(&previous_tree))
            .ok_or(DocumentEditError::ParseFailed)?;
        *self.tree_mut() = reparsed_tree;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentEditError {
    InvalidRange,
    ParseFailed,
}
