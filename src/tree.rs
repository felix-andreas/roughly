use tree_sitter::{Node, Tree, TreeCursor};

// todo: consider resusing global parser (maybe behind Mutex??)
pub fn parse(text: &str, maybe_tree: Option<&Tree>) -> Tree {
    let mut parser = tree_sitter::Parser::new();
    let language = tree_sitter_r::LANGUAGE;
    parser
        .set_language(&language.into())
        .expect("Error loading R parser");
    parser.parse(text, maybe_tree).unwrap()
}

#[inline]
pub fn for_each_child<'a, E>(
    cursor: &mut TreeCursor<'a>,
    mut func: impl FnMut(Node<'a>, Option<&'static str>) -> Result<(), E>,
) -> Result<(), E> {
    Ok(if cursor.goto_first_child() {
        loop {
            func(cursor.node(), cursor.field_name())?;
            if !cursor.goto_next_sibling() {
                cursor.goto_parent();
                break;
            }
        }
    })
}

// #[macro_export]
// macro_rules! for_each_child {
//     ($cursor:expr, $body:block) => {{
//         $cursor.goto_first_child() {
//             loop {
//                 let
//                 result = $func($cursor.node(), $cursor.field_name());
//                 if !$cursor.goto_next_sibling() {
//                     $cursor.goto_parent();
//                     break;
//                 }
//             }
//         }
//     }};
// }
