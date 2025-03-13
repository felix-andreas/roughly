use tree_sitter::{Node, Tree, TreeCursor};

// todo: consider resusing global parser (maybe behind Mutex??)
pub fn parse(text: impl AsRef<[u8]>, maybe_tree: Option<&Tree>) -> Tree {
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

// #[macro_export]
// macro_rules! for_each_child {
//     ($cursor:expr, $block:block) => {{
//         let mut i = 0;
//         if $cursor.goto_first_child() {
//             loop {
//                 let child = $cursor.node();
//                 let field_name = $cursor.field_name();
//                 $block
//                 if !$cursor.goto_next_sibling() {
//                     $cursor.goto_parent();
//                     break;
//                 }
//                 i += 1;
//             }
//         }
//     }};
// }
