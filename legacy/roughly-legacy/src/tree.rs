// Shared parsing helpers and the node-kind/field id tables live in the `analysis` crate.
// This module keeps only the helpers that are specific to the CLI and formatter.
pub use analysis::{field, kind};
use tree_sitter::{Node, Parser, Tree, TreeCursor};

//
// PARSING
//

pub fn new_parser() -> Parser {
    analysis::tree::new_parser().expect("failed to load R parser")
}

pub fn parse(parser: &mut Parser, text: impl AsRef<[u8]>, maybe_tree: Option<&Tree>) -> Tree {
    parser.parse(text, maybe_tree).expect("failed to parse")
}

pub fn parse_rope(parser: &mut Parser, rope: &ropey::Rope, maybe_tree: Option<&Tree>) -> Tree {
    analysis::tree::parse_rope(parser, rope, maybe_tree).expect("failed to parse")
}

//
// TRAVERSAL
//

#[inline]
pub fn for_each_child<'a, E>(
    cursor: &mut TreeCursor<'a>,
    mut func: impl FnMut(usize, Node<'a>, Option<u16>, &mut TreeCursor<'a>) -> Result<(), E>,
) -> Result<(), E> {
    let mut i = 0;
    if cursor.goto_first_child() {
        loop {
            func(
                i,
                cursor.node(),
                cursor.field_id().map(|id| id.get()),
                cursor,
            )?;
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

//
// DISPLAY AST
//

pub fn display_ast(node: Node) -> String {
    fn traverse(cursor: &mut TreeCursor, output: &mut String) {
        let indent = "    ".repeat(cursor.depth() as usize);
        let node = cursor.node();

        let start = node.start_position();
        let end = node.end_position();

        output.push_str(
            &if node.kind() == "comment" {
                console::style(node.kind())
            } else {
                console::style(node.kind()).bold()
            }
            .to_string(),
        );
        output.push_str(
            &console::style(format!(
                " {}:{}..{}:{}",
                start.row, start.column, end.row, end.column
            ))
            .italic()
            .to_string(),
        );

        if node.is_missing() {
            output.push_str(" MISSING");
        } else if node.is_error() && node.kind() != "ERROR" {
            output.push_str(" ERROR");
        }

        if cursor.goto_first_child() {
            loop {
                output.push('\n');
                output.push_str(&indent);
                output.push_str("    ");

                if let Some(field_name) = cursor.field_name() {
                    output.push_str(&console::style(field_name).underlined().to_string());
                    output.push_str(": ");
                }

                traverse(cursor, output);

                if !cursor.goto_next_sibling() {
                    cursor.goto_parent();
                    break;
                }
            }
        }
    }

    let mut result = String::new();
    traverse(&mut node.walk(), &mut result);
    result
}
