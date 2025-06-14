use {
    ropey::Rope,
    tree_sitter::{Node, Parser, Tree, TreeCursor},
};

pub fn new_parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_r::LANGUAGE.into())
        .expect("Error loading R parser");
    parser
}

pub fn parse(parser: &mut Parser, text: impl AsRef<[u8]>, maybe_tree: Option<&Tree>) -> Tree {
    parser.parse(text, maybe_tree).unwrap()
}

pub fn parse_rope(parser: &mut Parser, rope: &Rope, maybe_tree: Option<&Tree>) -> Tree {
    let mut lookup = |byte, _position| {
        let (chunk, chunk_byte, _, _) = rope.chunk_at_byte(byte);
        let offset = byte - chunk_byte;
        &chunk.as_bytes()[offset..]
    };
    parser
        .parse_with_options(&mut lookup, maybe_tree, None)
        .expect("failed to parse")
}

#[inline]
pub fn for_each_child<'a, E>(
    cursor: &mut TreeCursor<'a>,
    mut func: impl FnMut(usize, Node<'a>, Option<&'static str>, &mut TreeCursor<'a>) -> Result<(), E>,
) -> Result<(), E> {
    let mut i = 0;
    if cursor.goto_first_child() {
        loop {
            func(i, cursor.node(), cursor.field_name(), cursor)?;
            if !cursor.goto_next_sibling() {
                cursor.goto_parent();
                break;
            }
            i += 1;
        }
    };
    Ok(())
}

pub fn find_next_error(node: Node) -> Option<Node> {
    let mut cursor = node.walk();

    loop {
        let current = cursor.node();
        if current.is_error() || current.is_missing() {
            return Some(current);
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return None;
            }
        }
    }
}
