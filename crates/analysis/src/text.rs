use {ropey::Rope, tree_sitter::Node};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextPosition {
    pub line_index: usize,
    pub character_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRange {
    pub start: TextPosition,
    pub end: TextPosition,
}

// A `TextPosition` column is a UTF-8 byte offset within its line (tree-sitter `Point` semantics),
// so resolving it against the rope yields an absolute byte index.
pub fn line_character_to_byte_index(rope: &Rope, position: TextPosition) -> Option<usize> {
    let line_start_byte = rope.try_line_to_byte(position.line_index).ok()?;
    let line_text = rope.get_line(position.line_index)?;

    (position.character_index <= line_text.len_bytes())
        .then_some(line_start_byte + position.character_index)
}

pub fn node_text(rope: &Rope, node: Node<'_>) -> Option<String> {
    let byte_range = node.start_byte()..node.end_byte();
    if byte_range.start > byte_range.end {
        return None;
    }

    let start_character = rope.try_byte_to_char(byte_range.start).ok()?;
    let end_character = rope.try_byte_to_char(byte_range.end).ok()?;
    Some(rope.slice(start_character..end_character).to_string())
}

pub fn compact_node_text(rope: &Rope, node: Node<'_>) -> String {
    let Some(text) = node_text(rope, node) else {
        return "<unavailable>".to_owned();
    };

    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        "<empty>".to_owned()
    } else {
        compact
    }
}
