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

// Whether the rope's text contains `needle` as a byte substring, without materializing the rope.
// Chunk-wise `str::contains` plus a carry buffer covering matches that straddle a chunk boundary.
// The conservative text prefilter behind `IdeDatabase::candidate_document_ids`.
pub fn rope_contains(rope: &Rope, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if rope.len_bytes() < needle.len() {
        return false;
    }
    let needle_bytes = needle.as_bytes();
    let carry_length = needle_bytes.len() - 1;
    let mut carry: Vec<u8> = Vec::with_capacity(2 * carry_length.max(1));
    for chunk in rope.chunks() {
        if chunk.contains(needle) {
            return true;
        }
        if carry_length == 0 {
            continue;
        }
        // A straddling match must start inside `carry` (a match fully inside the chunk was found
        // above), so `carry` extended by the chunk's first `carry_length` bytes covers it.
        let boundary_take = chunk.len().min(carry_length);
        carry.extend_from_slice(&chunk.as_bytes()[..boundary_take]);
        if carry
            .windows(needle_bytes.len())
            .any(|window| window == needle_bytes)
        {
            return true;
        }
        // Slide: keep the last `carry_length` bytes of everything seen so far.
        carry.extend_from_slice(&chunk.as_bytes()[boundary_take..]);
        let excess = carry.len().saturating_sub(carry_length);
        carry.drain(..excess);
    }
    false
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
