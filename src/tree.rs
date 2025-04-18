use tree_sitter::{Node, Tree, TreeCursor};

// todo: consider resusing global parser (maybe behind Mutex??)
pub fn parse(text: impl AsRef<[u8]>, maybe_tree: Option<&Tree>) -> Tree {
    let mut parser = tree_sitter::Parser::new();
    let language = tree_sitter_r::LANGUAGE;
    parser.set_language(&language.into()).unwrap();
    parser.parse(text, maybe_tree).unwrap()
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
